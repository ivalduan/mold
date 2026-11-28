//! The SSH executor (ADR-0009 §2-§4, §6, §7): command items on a remote host, with
//! the run directory staged out and the artifacts staged back.
//!
//! **The system `ssh` and `scp` clients, as subprocesses, not an SSH crate.** The
//! worker protocol is already a stdio protocol, and `ssh` is exactly a transport for
//! stdio, so the whole of [`runner`](crate::runner) is reused unchanged: the same
//! `JobSpec` bytes on stdin, the same NDJSON on stdout, the same `stderr.log`. It
//! adds no dependency, it honours the user's own `~/.ssh/config`, `known_hosts`,
//! agent and `Match`/`ProxyJump` rules, and **no credential ever passes through
//! Mold** (`BatchMode=yes`: a host that would prompt fails instead). A library would
//! have to re-implement that configuration surface and would want the key material.
//!
//! What this module adds to a local run:
//!
//! | | |
//! |---|---|
//! | **staging** (§3) | one `ssh` to make a fresh remote run directory, one `scp` out for the declared inputs, one `scp` back for the artifacts. `shared_fs` skips all three. |
//! | **remote process group** (§7) | the worker runs as a job-control background job of the login shell, so it leads its own process group; cancel and timeout run a second `ssh` that kills that group. Killing the local client is *not* enough. |
//! | **cleanup** | the remote run directory is removed after the item succeeds, and **kept after a failure or a cancel** for inspection (`keep_run_dir = true` keeps it always). A stale directory for the same (item key, attempt) is removed before the attempt starts. |
//!
//! Everything else — slots, GPUs, memory, run directories, the timeout starting at
//! the worker's first event — is the shared [`Scheduler`](crate::sched::Scheduler),
//! so the two flavours cannot drift apart.
//!
//! The remote side needs a POSIX login shell (`sh`, `bash`, `dash`, `ksh`, `zsh`),
//! `cat`/`rm`/`mkdir`/`kill` and nothing installed by Mold.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mold_graph::{CommandJob, EventSink, Job, NodeError};

use crate::error::ConfigError;
use crate::manifest::Workers;
use crate::process::{ProcessTree, describe_exit};
use crate::profile::{Profile, SshTarget, Target};
use crate::runner::{self, Launch};
use crate::sched::{Flavour, RunSpec, Scheduler, command_executor, manifest_for};

/// Everything `ssh`/`scp` print, appended, in the run directory.
pub const SSH_LOG_FILE: &str = "ssh.log";
/// What the remote process-group kill did, in the run directory. Written only when a
/// run is cancelled, times out or fails mid-stream.
pub const SSH_KILL_FILE: &str = "ssh-kill.log";

/// How long one `scp` may take (a big ligand set over a slow link).
const STAGE_LIMIT: Duration = Duration::from_secs(600);
/// How long a small control command (`mkdir`, `rm`, the kill) may take, on top of the
/// profile's connect timeout.
const CONTROL_GRACE: Duration = Duration::from_secs(20);
/// How often a staging client is polled for cancellation.
const POLL: Duration = Duration::from_millis(20);

/// Runs command items on a remote host over `ssh`. See the [module docs](self).
pub struct SshExecutor {
    sched: Arc<Scheduler>,
}

command_executor!(SshExecutor);

impl SshExecutor {
    /// An executor for an `ssh` profile, launching `workers` on the host, with local
    /// run directories under `run_root`. A local profile is refused.
    pub fn new(
        profile: &Profile,
        workers: Arc<Workers>,
        run_root: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let target = match &profile.target {
            Target::Ssh(t) => t.clone(),
            Target::Local { .. } => {
                return Err(ConfigError::NotYet {
                    key: profile.key.clone(),
                    what: "a local profile needs the local executor, not the SSH one".into(),
                });
            }
        };
        Ok(Self {
            sched: Scheduler::new(
                profile,
                run_root.into(),
                Box::new(Ssh {
                    target,
                    workers,
                    key: profile.key.clone(),
                }),
            ),
        })
    }

    pub fn key(&self) -> &str {
        self.sched.key()
    }

    /// Notices since the last call (each is reported once per executor).
    pub fn take_notices(&self) -> Vec<crate::sched::Notice> {
        self.sched.take_notices()
    }

    /// Wait until nothing is queued or running. `false` on timeout.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        self.sched.wait_idle(timeout)
    }
}

/// Phase *run* on a remote host.
struct Ssh {
    target: SshTarget,
    workers: Arc<Workers>,
    key: String,
}

impl fmt::Debug for Ssh {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ssh")
            .field("key", &self.key)
            .field("host", &self.target.destination())
            .field("shared_fs", &self.target.shared_fs)
            .finish()
    }
}

/// Why a staging or control client did not do its job. A cancel (or timeout) during
/// staging must reach the engine as a cancellation, not as a failure, or a timed-out
/// item would be reported as merely broken.
#[derive(Debug)]
enum ToolError {
    Cancelled,
    Failed(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(m) => f.write_str(m),
        }
    }
}

/// Where one attempt lives on the host. Paths use `/` and are quoted into the
/// remote shell, never concatenated into a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Remote {
    /// `<root>/<item key>/<attempt>`: the worker's working directory.
    dir: String,
    /// `<root>/<item key>`: where `scp -r` puts (and gets) the run directory itself.
    parent: String,
    /// `<dir>.pgid`: the worker's process group, written by the wrapper before the
    /// worker can produce anything. A sibling, so the run directory holds only the
    /// item's own files and staging never carries ours.
    pgid: String,
    /// `<dir>.cancel`: the sentinel the kill writes first (see [`kill_script`]).
    cancel: String,
    /// False with `shared_fs`: the run directory *is* the local one, so there is
    /// nothing to stage (ADR-0009 §3).
    staged: bool,
}

impl Flavour for Ssh {
    fn run(
        &self,
        r: &RunSpec<'_>,
        events: &mut EventSink<'_>,
    ) -> Result<serde_json::Value, NodeError> {
        let worker = &r.cmd.worker;
        let host = self.target.destination();
        let fail = |m: String| NodeError::Failed(format!("worker `{worker}` on `{host}`: {m}"));
        // Staging is part of the run: a cancel there is a cancel, not a failure.
        let staging = |e: ToolError| match e {
            ToolError::Cancelled => NodeError::Cancelled,
            ToolError::Failed(m) => fail(m),
        };
        let m = manifest_for(&self.workers, r.job, worker)?;
        // The remote side resolves the executable on its own PATH: this machine's
        // search path says nothing about the host.
        let argv = m.argv();
        let dir = r.cmd.run_dir.as_path();
        let remote = self.remote(r.job, dir)?;
        let stage = self.stage_paths(r.job, r.cmd)?;

        if (r.stop)() {
            return Err(NodeError::Cancelled);
        }
        if remote.staged {
            self.setup(&remote, &stage, dir, r.stop).map_err(&staging)?;
            self.stage_in(&remote, &stage, dir, r.stop)
                .map_err(&staging)?;
        }

        // Only inputs we *copied* are trimmed after a successful run: on a shared
        // filesystem the run directory holds the node's own files, not a copy.
        let trim: &[String] = if remote.staged { &stage } else { &[] };
        let script = run_script(&remote, &argv, trim);
        let launch = self.ssh_launch(self.target.connect_timeout_s, &script);
        let control = runner::Control {
            stop: r.stop,
            up: r.up,
            startup_limit: r.startup_limit,
            // The local `ssh` client is only the near end of the worker: kill the
            // remote process group, over a second connection, before letting go.
            reap: &|| self.kill_remote(&remote, dir),
            hint: &|status| hint(status, &host, worker),
        };
        let finished = runner::run(worker, &launch, r.cmd, r.spec, events, &control)?;

        // Only a successful item needs its artifacts here; a failed attempt's remote
        // directory is kept as it stands, for inspection.
        if remote.staged {
            self.stage_out(&remote, dir, r.stop).map_err(&staging)?;
            if !self.target.keep_run_dir {
                self.cleanup(&remote, dir, r.stop);
            }
        }
        Ok(finished)
    }
}

impl Ssh {
    /// Where this attempt runs on the host. The remote directory mirrors the local
    /// one's last two components, so the two are easy to line up by eye.
    fn remote(&self, job: &Job, local_dir: &Path) -> Result<Remote, NodeError> {
        let attempt = local_dir
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| NodeError::msg("the run directory has no name".to_string()))?;
        Ok(remote_paths(&self.target, &job.key.to_hex(), attempt))
    }

    /// The inputs to copy to the host: what the node declared (`CommandJob::stage`),
    /// or the whole run directory (an empty declaration). Checked before anything
    /// reaches a remote shell: a staged path is relative and stays inside the run
    /// directory, like every other path in the protocol.
    fn stage_paths(&self, job: &Job, cmd: &CommandJob) -> Result<Vec<String>, NodeError> {
        let mut out = Vec::new();
        for p in &cmd.stage {
            crate::protocol::check_relative(p).map_err(|why| {
                NodeError::msg(format!(
                    "node `{}` declared a staged path that {why}",
                    job.kind
                ))
            })?;
            if !cmd.run_dir.join(p).exists() {
                return Err(NodeError::msg(format!(
                    "node `{}` declared `{}` for staging but did not write it into the \
                     run directory",
                    job.kind,
                    p.display()
                )));
            }
            // `/` on the wire, whatever this machine writes.
            out.push(p.to_string_lossy().replace('\\', "/"));
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// A fresh remote run directory (and the parents any nested staged input needs).
    fn setup(
        &self,
        remote: &Remote,
        stage: &[String],
        dir: &Path,
        stop: &dyn Fn() -> bool,
    ) -> Result<(), ToolError> {
        let mut dirs = vec![sq(&remote.dir)];
        for p in stage {
            if let Some((parent, _)) = p.rsplit_once('/') {
                dirs.push(sq(&format!("{}/{parent}", remote.dir)));
            }
        }
        dirs.sort();
        dirs.dedup();
        // A retry gets a fresh directory, like the local flavour (ADR-0009 §7).
        let script = format!(
            "rm -rf {dir} {pgid} {cancel} && mkdir -p {dirs}",
            dir = sq(&remote.dir),
            pgid = sq(&remote.pgid),
            cancel = sq(&remote.cancel),
            dirs = dirs.join(" "),
        );
        let launch = self.ssh_launch(self.target.connect_timeout_s, &script);
        self.tool(
            "preparing the remote run directory",
            &launch,
            dir,
            SSH_LOG_FILE,
            stop,
            self.control_limit(),
        )
    }

    /// The declared inputs out to the host (ADR-0009 §3).
    fn stage_in(
        &self,
        remote: &Remote,
        stage: &[String],
        dir: &Path,
        stop: &dyn Fn() -> bool,
    ) -> Result<(), ToolError> {
        let dest = self.target.destination();
        if stage.is_empty() {
            // The whole run directory, as a directory: it lands as `<parent>/<name>`.
            let launch = self.scp_launch(
                &[dir.as_os_str().into()],
                &format!("{dest}:{}", remote.parent),
            );
            return self.tool(
                "staging the run directory to the host",
                &launch,
                dir,
                SSH_LOG_FILE,
                stop,
                STAGE_LIMIT,
            );
        }
        // One `scp` per distinct parent directory: scp flattens, so files from
        // `in/x.sdf` must be sent to `<run dir>/in/`.
        let mut groups: std::collections::BTreeMap<&str, Vec<OsString>> = Default::default();
        for p in stage {
            let (parent, _) = p.rsplit_once('/').unwrap_or(("", p.as_str()));
            groups
                .entry(parent)
                .or_default()
                .push(dir.join(p).into_os_string());
        }
        for (parent, sources) in groups {
            let into = if parent.is_empty() {
                remote.dir.clone()
            } else {
                format!("{}/{parent}", remote.dir)
            };
            let launch = self.scp_launch(&sources, &format!("{dest}:{into}"));
            self.tool(
                "staging inputs to the host",
                &launch,
                dir,
                SSH_LOG_FILE,
                stop,
                STAGE_LIMIT,
            )?;
        }
        Ok(())
    }

    /// The artifacts back. The remote directory's name is the local one's, so it
    /// merges into the local run directory; the runner's own files (`job.json`,
    /// `events.ndjson`, `stderr.log`) are reserved names no worker may write.
    fn stage_out(
        &self,
        remote: &Remote,
        dir: &Path,
        stop: &dyn Fn() -> bool,
    ) -> Result<(), ToolError> {
        let parent = dir
            .parent()
            .ok_or_else(|| ToolError::Failed("the run directory has no parent".to_string()))?;
        let src = format!("{}:{}", self.target.destination(), remote.dir);
        let launch = self.scp_launch(&[src.into()], &parent.to_string_lossy());
        self.tool(
            "copying the artifacts back from the host",
            &launch,
            dir,
            SSH_LOG_FILE,
            stop,
            STAGE_LIMIT,
        )
    }

    /// Remove a finished item's remote run directory. Best effort: the artifacts are
    /// already here, so a failure to tidy up must not fail the item.
    fn cleanup(&self, remote: &Remote, dir: &Path, stop: &dyn Fn() -> bool) {
        let script = format!(
            "rm -rf {} {} {}",
            sq(&remote.dir),
            sq(&remote.pgid),
            sq(&remote.cancel)
        );
        let launch = self.ssh_launch(self.target.connect_timeout_s, &script);
        let _ = self.tool(
            "removing the remote run directory",
            &launch,
            dir,
            SSH_LOG_FILE,
            stop,
            self.control_limit(),
        );
    }

    /// Kill the worker's **remote** process group (cancel, timeout, or a failure
    /// mid-stream). Best effort and never interruptible: it runs while the item is
    /// already being abandoned, so it ignores the cancel flag and bounds itself.
    fn kill_remote(&self, remote: &Remote, dir: &Path) {
        let launch = self.ssh_launch(self.target.connect_timeout_s.min(10), &kill_script(remote));
        let r = self.tool(
            "killing the remote process group",
            &launch,
            dir,
            SSH_KILL_FILE,
            &|| false,
            self.control_limit(),
        );
        let note = match &r {
            Ok(()) => "mold: killed the remote process group\n".to_string(),
            Err(e) => format!("mold: {e}\n"),
        };
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(SSH_KILL_FILE))
            .map(|mut f| std::io::Write::write_all(&mut f, note.as_bytes()));
    }

    fn control_limit(&self) -> Duration {
        Duration::from_secs(u64::from(self.target.connect_timeout_s)) + CONTROL_GRACE
    }

    /// `ssh [fixed args] -T -o BatchMode=yes -o ConnectTimeout=n [-p port] dest cmd`.
    fn ssh_launch(&self, timeout_s: u32, command: &str) -> Launch {
        let t = &self.target;
        let mut args: Vec<OsString> = t.ssh[1..].iter().map(OsString::from).collect();
        // -T: no pty. A pty would rewrite the worker's bytes (CRLF, control chars)
        // and merge stderr into stdout; the event stream must stay byte-identical.
        args.push("-T".into());
        for o in [
            // Credentials come from the user's agent and config, never from Mold: a
            // host that would ask for one fails instead of waiting for a password.
            "BatchMode=yes".to_string(),
            format!("ConnectTimeout={timeout_s}"),
        ] {
            args.push("-o".into());
            args.push(o.into());
        }
        if let Some(p) = t.port {
            args.push("-p".into());
            args.push(p.to_string().into());
        }
        args.push(t.destination().into());
        args.push(command.into());
        Launch {
            program: t.ssh[0].clone().into(),
            args,
        }
    }

    /// `scp -q -B -r [fixed args] -o ConnectTimeout=n [-P port] sources… dest`.
    fn scp_launch(&self, sources: &[OsString], dest: &str) -> Launch {
        let t = &self.target;
        let mut args: Vec<OsString> = t.scp[1..].iter().map(OsString::from).collect();
        args.push("-q".into()); // no progress meter: this is not a terminal
        args.push("-B".into()); // batch: never prompt, like ssh's BatchMode
        args.push("-r".into()); // an input may be a directory
        args.push("-o".into());
        args.push(format!("ConnectTimeout={}", t.connect_timeout_s).into());
        if let Some(p) = t.port {
            // scp spells the port `-P`.
            args.push("-P".into());
            args.push(p.to_string().into());
        }
        args.extend(sources.iter().cloned());
        args.push(dest.into());
        Launch {
            program: t.scp[0].clone().into(),
            args,
        }
    }

    /// Run one client to completion, its output appended to `log` in the run
    /// directory. Polls `stop` (cancel, timeout) and gives up after `limit`, so a
    /// stalled transfer cannot hold a slot for ever.
    fn tool(
        &self,
        what: &str,
        launch: &Launch,
        dir: &Path,
        log: &str,
        stop: &dyn Fn() -> bool,
        limit: Duration,
    ) -> Result<(), ToolError> {
        let broke = ToolError::Failed;
        let path = dir.join(log);
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| broke(format!("{what}: cannot write {}: {e}", path.display())))?;
        let errs = out
            .try_clone()
            .map_err(|e| broke(format!("{what}: cannot write {}: {e}", path.display())))?;
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(errs);
        let mut tree = ProcessTree::spawn(&mut command).map_err(|e| {
            broke(format!(
                "{what}: cannot start {}: {e}",
                Path::new(&launch.program).display()
            ))
        })?;
        let start = Instant::now();
        let status = loop {
            match tree.child_mut().try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {}
                Err(e) => {
                    return Err(broke(format!("{what}: cannot wait for the client: {e}")));
                }
            }
            if stop() {
                tree.kill();
                let _ = tree.wait();
                return Err(ToolError::Cancelled);
            }
            if start.elapsed() > limit {
                tree.kill();
                let _ = tree.wait();
                return Err(broke(format!(
                    "{what}: gave up after {} s{}",
                    limit.as_secs(),
                    tail(&path)
                )));
            }
            std::thread::sleep(POLL);
        };
        let _ = tree.wait();
        if status.success() {
            return Ok(());
        }
        Err(broke(format!(
            "{what}: {}{}",
            describe_exit(status),
            tail(&path)
        )))
    }
}

/// Extra words for a run that ended with no `finished` event, from the exit status.
/// `ssh` reports its own failures as 255 and otherwise passes the remote status on.
fn hint(status: Option<ExitStatus>, host: &str, worker: &str) -> String {
    let Some(code) = status.and_then(|s| s.code()) else {
        return String::new();
    };
    match code {
        255 => format!(
            "; `ssh` could not run the worker on `{host}` (host unreachable, \
             authentication refused, or the connection dropped mid-run)"
        ),
        127 => format!(
            "; `{worker}`'s executable was not found on `{host}` (its manifest's \
             `executable` must be on the remote PATH)"
        ),
        126 => format!("; the remote run directory or `{worker}` could not be used on `{host}`"),
        143 => "; the run was cancelled before the worker started".into(),
        _ => String::new(),
    }
}

/// The last lines of a client log, for an error message.
fn tail(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    let tail = lines[lines.len().saturating_sub(5)..].join(" | ");
    let tail: String = if tail.chars().count() > 500 {
        tail.chars().skip(tail.chars().count() - 500).collect()
    } else {
        tail
    };
    format!("; {tail}")
}

/// One argument for a POSIX shell: single-quoted, with embedded quotes closed and
/// reopened. The only quoting rule the remote side needs.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Where one attempt lives on the host: `<root>/<item key>/<attempt>`, mirroring the
/// local run directory under the executor's run root. With `shared_fs`, the root is
/// the host's view of *this machine's* run root, so the paths name the same files
/// and nothing is staged.
fn remote_paths(target: &SshTarget, key_hex: &str, attempt: &str) -> Remote {
    let root = match &target.shared_fs {
        Some(shared) => shared.trim_end_matches('/'),
        None => target.root().trim_end_matches('/'),
    };
    let parent = format!("{root}/{key_hex}");
    Remote {
        dir: format!("{parent}/{attempt}"),
        pgid: format!("{parent}/{attempt}.pgid"),
        cancel: format!("{parent}/{attempt}.cancel"),
        parent,
        staged: target.shared_fs.is_none(),
    }
}

/// The remote command: run the worker in its run directory as its own process group,
/// and leave nothing behind.
///
/// Why each line is there:
/// - `exec 3<&0` + `<&3`: a background command's stdin is `/dev/null` in a shell
///   without job control, and the `JobSpec` arrives on stdin. The explicit
///   redirection settles it in every shell.
/// - `set -m` + `&`: job control gives the worker its **own process group**, whose
///   id is its pid, so one `kill` reaches everything it spawns.
/// - the two `cancel` checks, around writing the pgid file, close the race with
///   [`kill_script`] from both sides: whichever of the two is first, the worker is
///   either killed by its group or never started (see `kill_script`).
/// - `kill` after `wait`: whatever the worker left running dies with it, remotely as
///   locally.
/// - `rm -rf` of the staged inputs, only on success: the copy back then carries the
///   artifacts and not the inputs we just sent.
fn run_script(remote: &Remote, argv: &[String], stage: &[String]) -> String {
    let dir = sq(&remote.dir);
    let cancel = sq(&remote.cancel);
    let pgid = sq(&remote.pgid);
    let argv = argv.iter().map(|a| sq(a)).collect::<Vec<_>>().join(" ");
    // `set +e` and `|| :`: the login shell's own configuration must not be able to
    // abort the wrapper half way and leave the worker unsupervised.
    let mut s = format!(
        "set +e\n\
         exec 3<&0\n\
         cd {dir} || {{ echo \"mold: no run directory {dir}\" >&2; exit 126; }}\n\
         set -m\n\
         if [ -e {cancel} ]; then exit 143; fi\n\
         {argv} <&3 &\n\
         w=$!\n\
         echo \"$w\" > {pgid} || :\n\
         if [ -e {cancel} ]; then kill -9 -\"$w\" 2>/dev/null || :; fi\n\
         wait \"$w\"\n\
         r=$?\n\
         kill -9 -\"$w\" 2>/dev/null || :\n"
    );
    if !stage.is_empty() {
        let paths: Vec<String> = stage.iter().map(|p| sq(p)).collect();
        s.push_str(&format!(
            "if [ \"$r\" = 0 ]; then rm -rf {} || :; fi\n",
            paths.join(" ")
        ));
    }
    s.push_str("exit \"$r\"\n");
    s
}

/// The remote command that ends a run: **write the sentinel first**, then kill the
/// worker's process group.
///
/// That order is what makes cancellation airtight against [`run_script`], which
/// writes the pgid file and then re-reads the sentinel:
/// - if the pgid file is already there, the group is killed, worker or wrapper;
/// - if it is not, the sentinel is already there, so the wrapper either exits before
///   starting the worker or kills the group itself right after starting it.
///
/// It always exits 0: a run being abandoned must not be reported as a kill failure.
/// The group form comes first; the bare pid is the fallback for a remote shell
/// without job control.
fn kill_script(remote: &Remote) -> String {
    format!(
        "set +e\n\
         : > {cancel} 2>/dev/null || :\n\
         p=$(cat {pgid} 2>/dev/null)\n\
         if [ -n \"$p\" ]; then kill -9 -\"$p\" 2>/dev/null || :; kill -9 \"$p\" 2>/dev/null || :; fi\n\
         exit 0\n",
        cancel = sq(&remote.cancel),
        pgid = sq(&remote.pgid)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{DEFAULT_RUN_ROOT, Profiles};
    use std::path::Path;

    fn target(toml: &str) -> SshTarget {
        let mut p = Profiles::builtin();
        p.add_toml(toml, Path::new("t.toml")).unwrap();
        match &p.get("box").unwrap().target {
            Target::Ssh(t) => t.clone(),
            Target::Local { .. } => panic!("ssh target"),
        }
    }

    fn ssh(toml: &str) -> Ssh {
        Ssh {
            target: target(toml),
            workers: Arc::new(Workers::new()),
            key: "box".into(),
        }
    }

    const MIN: &str = "[executor.box]\ntype = \"ssh\"\nhost = \"gpu-box\"\n";

    #[test]
    fn a_profile_defaults_to_the_system_clients_and_the_home_run_root() {
        let t = target(MIN);
        assert_eq!(t.ssh, ["ssh"]);
        assert_eq!(t.scp, ["scp"]);
        assert_eq!(t.destination(), "gpu-box");
        assert_eq!(t.root(), DEFAULT_RUN_ROOT);
        assert_eq!(t.connect_timeout_s, 15);
        assert!(!t.keep_run_dir);

        let t = target(
            "[executor.box]\ntype = \"ssh\"\nhost = \"gpu-box\"\nuser = \"alice\"\n\
             port = 2222\nrun_root = \"/scratch/mold\"\n\
             ssh_program = \"\\\"C:/Program Files/x/ssh.exe\\\" -F cfg\"\n\
             scp_program = \"myscp\"\nconnect_timeout_s = 3\nkeep_run_dir = true\n",
        );
        assert_eq!(t.destination(), "alice@gpu-box");
        assert_eq!(t.root(), "/scratch/mold");
        assert_eq!(t.ssh, ["C:/Program Files/x/ssh.exe", "-F", "cfg"]);
        assert_eq!(t.scp, ["myscp"]);
        assert!(t.keep_run_dir);
    }

    #[test]
    fn a_hostile_profile_is_refused() {
        for (field, value, want) in [
            ("host", "\"-oProxyCommand=rm -rf /\"", "host"),
            ("host", "\"box:2222\"", "host"),
            ("host", "\"a b\"", "host"),
            ("user", "\"root@evil\"", "user"),
            ("run_root", "\"/runs/\"", "run_root"),
            ("run_root", "\"/runs/it's\"", "run_root"),
            ("connect_timeout_s", "0", "connect_timeout_s"),
            ("ssh_program", "\"\\\"ssh\"", "ssh_program"),
        ] {
            let text = format!("{MIN}{field} = {value}\n");
            let mut p = Profiles::builtin();
            let e = p
                .add_toml(&text, Path::new("t.toml"))
                .unwrap_err()
                .to_string();
            assert!(e.contains(want), "{field}={value}: {e:?} wants {want:?}");
        }
    }

    fn job_remote(toml: &str, attempt: &str) -> Remote {
        remote_paths(&target(toml), "ab12", attempt)
    }

    #[test]
    fn remote_paths_mirror_the_local_run_directory() {
        let r = job_remote(
            "[executor.box]\ntype=\"ssh\"\nhost=\"h\"\nrun_root=\"/s/m\"\n",
            "0",
        );
        assert_eq!(r.dir, "/s/m/ab12/0");
        assert_eq!(r.parent, "/s/m/ab12");
        assert_eq!(r.pgid, "/s/m/ab12/0.pgid");
        assert_eq!(r.cancel, "/s/m/ab12/0.cancel");
        assert!(r.staged);
        // Two items with the same key: the second attempt directory is `0.1`.
        assert_eq!(job_remote(MIN, "0.1").dir, ".mold/runs/ab12/0.1");
        // A shared filesystem: the host's view of our own run root, nothing to stage.
        let shared = job_remote(
            "[executor.box]\ntype=\"ssh\"\nhost=\"h\"\nshared_fs=\"/net/ws\"\n",
            "0",
        );
        assert_eq!(shared.dir, "/net/ws/ab12/0");
        assert!(!shared.staged);
    }

    #[test]
    fn the_remote_command_runs_the_worker_in_its_own_process_group() {
        let r = job_remote(MIN, "0");
        let script = run_script(
            &r,
            &["python".into(), "-m".into(), "w".into()],
            &["l.sdf".into()],
        );
        assert_eq!(
            script,
            "set +e\n\
             exec 3<&0\n\
             cd '.mold/runs/ab12/0' || { echo \"mold: no run directory '.mold/runs/ab12/0'\" >&2; exit 126; }\n\
             set -m\n\
             if [ -e '.mold/runs/ab12/0.cancel' ]; then exit 143; fi\n\
             'python' '-m' 'w' <&3 &\n\
             w=$!\n\
             echo \"$w\" > '.mold/runs/ab12/0.pgid' || :\n\
             if [ -e '.mold/runs/ab12/0.cancel' ]; then kill -9 -\"$w\" 2>/dev/null || :; fi\n\
             wait \"$w\"\n\
             r=$?\n\
             kill -9 -\"$w\" 2>/dev/null || :\n\
             if [ \"$r\" = 0 ]; then rm -rf 'l.sdf' || :; fi\n\
             exit \"$r\"\n"
        );
        // Nothing to trim when the node staged the whole run directory.
        assert!(!run_script(&r, &["w".into()], &[]).contains("rm -rf"));
    }

    #[test]
    fn the_kill_command_writes_the_sentinel_before_it_reads_the_pgid() {
        let s = kill_script(&job_remote(MIN, "0"));
        let sentinel = s.find("0.cancel'").expect("writes the sentinel");
        let read = s.find("cat '.mold").expect("reads the pgid");
        let group = s.find("kill -9 -").expect("kills the group");
        assert!(sentinel < read && read < group, "{s}");
        assert!(s.ends_with("exit 0\n"), "a kill never reports failure: {s}");
    }

    #[test]
    fn a_path_with_a_quote_in_it_cannot_break_out_of_the_remote_command() {
        assert_eq!(sq("a'b"), r"'a'\''b'");
        let r = Remote {
            dir: "/r/x'; rm -rf /; echo '".into(),
            parent: "/r".into(),
            pgid: "/r/x.pgid".into(),
            cancel: "/r/x.cancel".into(),
            staged: true,
        };
        let script = run_script(&r, &["w".into()], &[]);
        assert!(
            script.contains(r"'/r/x'\''; rm -rf /; echo '\'''"),
            "{script}"
        );
        assert!(!script.contains("; rm -rf /; echo \n"));
    }

    #[test]
    fn the_client_command_lines_are_what_openssh_expects() {
        let s = ssh(
            "[executor.box]\ntype=\"ssh\"\nhost=\"h\"\nuser=\"me\"\nport=22022\n\
             ssh_program=\"ssh -F cfg\"\nconnect_timeout_s=7\n",
        );
        let l = s.ssh_launch(7, "echo hi");
        assert_eq!(l.program, "ssh");
        let args: Vec<String> = l
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-F",
                "cfg",
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=7",
                "-p",
                "22022",
                "me@h",
                "echo hi"
            ]
        );
        // `ssh_program`'s arguments are the ssh client's alone: `scp` here is the
        // default one, and it spells the port `-P`.
        let l = s.scp_launch(&["a.sdf".into()], "me@h:/r/x");
        assert_eq!(l.program, "scp");
        let args: Vec<String> = l
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-q",
                "-B",
                "-r",
                "-o",
                "ConnectTimeout=7",
                "-P",
                "22022",
                "a.sdf",
                "me@h:/r/x"
            ]
        );
    }

    #[test]
    fn exit_codes_become_messages_that_name_the_cause() {
        let code = |c: i32| {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                ExitStatus::from_raw(c << 8)
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::ExitStatusExt;
                ExitStatus::from_raw(c as u32)
            }
        };
        for (c, want) in [
            (255, "could not run the worker"),
            (127, "was not found on `h`"),
            (126, "could not be used"),
            (143, "cancelled before the worker started"),
        ] {
            let h = hint(Some(code(c)), "h", "w");
            assert!(h.contains(want), "{c}: {h:?} wants {want:?}");
        }
        assert_eq!(hint(Some(code(3)), "h", "w"), "");
        assert_eq!(hint(None, "h", "w"), "");
    }

    #[test]
    fn an_ssh_executor_refuses_a_local_profile_and_the_local_one_refuses_an_ssh_profile() {
        let p = Profiles::builtin();
        let local = p.get(crate::profile::LOCAL).unwrap();
        let e = SshExecutor::new(local, Arc::new(Workers::new()), "runs").unwrap_err();
        assert!(e.to_string().contains("local executor"), "{e}");
        let mut p = Profiles::builtin();
        p.add_toml(MIN, Path::new("t.toml")).unwrap();
        let remote = p.get("box").unwrap();
        let e = crate::LocalExecutor::new(remote, Arc::new(Workers::new()), "runs").unwrap_err();
        assert!(e.to_string().contains("SSH executor"), "{e}");
    }
}
