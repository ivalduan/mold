//! *Run*, the middle phase (ADR-0009 §1): start the worker, feed it the `JobSpec`,
//! relay its events as they arrive, and decide how the run ended. This module owns
//! what counts as success or failure; graph never reads the protocol.
//!
//! Files the runner writes into the run directory (reserved names):
//! - `job.json`: the exact bytes sent on stdin;
//! - `events.ndjson`: every stdout line, as received (ADR-0009 §6, §8);
//! - `stderr.log`: the worker's stderr.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use mold_graph::{CommandJob, EventSink, NodeError};

use crate::process::{ProcessTree, describe_exit};
use crate::protocol::{
    JobSpec, JobStatus, MAX_LINE_BYTES, WorkerEvent, canonical_json, parse_event,
};

pub const SPEC_FILE: &str = "job.json";
pub const EVENTS_FILE: &str = "events.ndjson";
pub const STDERR_FILE: &str = "stderr.log";

/// How often the runner checks for cancellation while the worker is quiet.
const POLL: Duration = Duration::from_millis(20);
/// After the worker exits, how long a descendant may keep stdout open before the
/// tree is killed (a worker that left a background child behind).
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// After stdout closes, how long the worker may take to exit.
const FINISH_GRACE: Duration = Duration::from_secs(5);
/// How long a worker may take, from being spawned, to write its first event (or the
/// node's timeout, if longer). Start-up is outside the timeout, which counts from the
/// first event, but it is never unbounded.
pub(crate) const STARTUP_LIMIT: Duration = Duration::from_secs(60);

/// What to start: a program and its arguments. The working directory is always the
/// run directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
}

/// How the executor steers one run.
pub(crate) struct Control<'a> {
    /// Turns true on cancel or timeout: kill the tree and stop.
    pub stop: &'a dyn Fn() -> bool,
    /// Called once, when the worker writes its first line: the attempt is running
    /// from then on (the executor reports `started`, which starts the timeout).
    pub up: &'a dyn Fn(),
    /// A worker that writes nothing within this long of being spawned is killed.
    pub startup_limit: Duration,
    /// Called once, before the local process tree is killed, on every way out of a
    /// run *except* a clean finish (cancel, timeout, a protocol failure, a child
    /// still holding stdout). The SSH flavour kills the **remote** process group
    /// here: the local `ssh` client is only the near end of the worker.
    pub reap: &'a dyn Fn(),
    /// Extra words for a run that ended without a `finished` event, read from the
    /// transport's exit status: `ssh`'s own 255, a remote shell's 127. Empty for a
    /// local worker, whose exit status says it all.
    pub hint: &'a dyn Fn(Option<ExitStatus>) -> String,
}

enum Msg {
    Line(Vec<u8>),
    Eof,
    Error(std::io::Error),
}

/// Run `cmd` (already validated into `spec`) with `launch`, until it finishes, fails
/// or `stop()` turns true. Returns the `finished` event (status `ok`) or the failure.
/// Every path out kills the whole process tree, so nothing outlives the run.
pub(crate) fn run(
    worker: &str,
    launch: &Launch,
    cmd: &CommandJob,
    spec: &JobSpec,
    events: &mut EventSink<'_>,
    control: &Control<'_>,
) -> Result<serde_json::Value, NodeError> {
    let Control {
        stop,
        up,
        startup_limit,
        reap,
        hint,
    } = *control;
    let fail = |m: String| NodeError::Failed(format!("worker `{worker}`: {m}"));
    let dir = &cmd.run_dir;
    let spec_bytes = canonical_json(&cmd.spec);
    let io = |what: &str, e: std::io::Error| fail(format!("cannot write {what}: {e}"));
    std::fs::write(dir.join(SPEC_FILE), &spec_bytes).map_err(|e| io(SPEC_FILE, e))?;
    let stderr = File::create(dir.join(STDERR_FILE)).map_err(|e| io(STDERR_FILE, e))?;
    let mut log = File::create(dir.join(EVENTS_FILE)).map_err(|e| io(EVENTS_FILE, e))?;

    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr);
    if stop() {
        return Err(NodeError::Cancelled);
    }
    let mut tree = ProcessTree::spawn(&mut command).map_err(|e| {
        fail(format!(
            "cannot start {}: {e}",
            Path::new(&launch.program).display()
        ))
    })?;

    // stdin: the spec, then EOF. On its own thread so a worker that writes before it
    // reads can never deadlock us; a worker that never reads gets killed below.
    let mut stdin = tree.child_mut().stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&spec_bytes);
    });
    let stdout = tree.child_mut().stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    // Detached: if a process outside the tree held stdout open it could block forever.
    std::thread::spawn(move || read_lines(stdout, &tx));

    let mut finished: Option<(serde_json::Value, JobStatus, Option<String>)> = None;
    let mut failure: Option<NodeError> = None;
    let mut cancelled = false;
    let mut exited: Option<Instant> = None;
    let mut line_no = 0usize;
    let spawned = Instant::now();
    let mut is_up = false;
    loop {
        if stop() {
            cancelled = true;
            break;
        }
        match rx.recv_timeout(POLL) {
            Ok(Msg::Line(line)) => {
                line_no += 1;
                if !is_up {
                    is_up = true;
                    up();
                }
                let _ = log.write_all(&line).and_then(|()| log.write_all(b"\n"));
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                if finished.is_some() {
                    failure = Some(fail(format!(
                        "stdout line {line_no}: an event after `finished`"
                    )));
                    break;
                }
                let (value, event) = match parse_event(&line) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        failure = Some(fail(format!("stdout line {line_no}: {e}")));
                        break;
                    }
                };
                if event.job_id() != spec.job_id {
                    failure = Some(fail(format!(
                        "stdout line {line_no}: event for job `{}`, expected `{}`",
                        event.job_id(),
                        spec.job_id
                    )));
                    break;
                }
                match event {
                    WorkerEvent::Finished { status, error, .. } => {
                        finished = Some((value, status, error));
                    }
                    _ => match events.event(&value) {
                        Ok(()) => {}
                        Err(NodeError::Cancelled) => {
                            cancelled = true;
                            break;
                        }
                        Err(e) => {
                            failure = Some(e);
                            break;
                        }
                    },
                }
            }
            Ok(Msg::Eof) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Msg::Error(e)) => {
                failure = Some(fail(format!("reading stdout: {e}")));
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                if !is_up && spawned.elapsed() > startup_limit {
                    failure = Some(fail(format!(
                        "wrote no event within {} s of starting{}{}",
                        startup_limit.as_secs(),
                        hint(None),
                        stderr_tail(dir)
                    )));
                    break;
                }
                if exited.is_none() && matches!(tree.child_mut().try_wait(), Ok(Some(_))) {
                    exited = Some(Instant::now());
                }
                if exited.is_some_and(|t| t.elapsed() > EXIT_GRACE) {
                    break;
                }
            }
        }
    }

    // stdout is closed (or we stop reading): give the worker a moment to exit on its
    // own, unless we are killing it anyway.
    let mut lingered = false;
    if !cancelled && failure.is_none() {
        let t = Instant::now();
        while matches!(tree.child_mut().try_wait(), Ok(None)) {
            if stop() {
                cancelled = true;
                break;
            }
            if t.elapsed() > FINISH_GRACE {
                lingered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // Whatever happened, nothing the worker started outlives the run. Anything but a
    // clean `finished` may have left work on the far side of the transport (an SSH
    // host), which killing the local client would not touch: reap that, while the
    // connection is still up. A clean run needs no second connection — there the
    // remote wrapper ends its own process group.
    let clean = !cancelled
        && failure.is_none()
        && !lingered
        && matches!(finished, Some((_, JobStatus::Ok, _)));
    if !clean {
        reap();
    }
    tree.kill();
    let status = tree.wait();
    let _ = writer.join();
    drop(log);

    if cancelled || stop() {
        return Err(NodeError::Cancelled);
    }
    if let Some(e) = failure {
        return Err(e);
    }
    let status = status.map_err(|e| fail(format!("cannot wait for the worker: {e}")))?;
    let stderr = || stderr_tail(dir);
    match finished {
        Some((value, JobStatus::Ok, _)) if status.success() => Ok(value),
        Some((_, JobStatus::Ok, _)) if lingered => Err(fail(format!(
            "reported `finished` but did not exit within {} s{}",
            FINISH_GRACE.as_secs(),
            stderr()
        ))),
        Some((_, JobStatus::Ok, _)) => Err(fail(format!(
            "reported `finished` but {}{}",
            describe_exit(status),
            stderr()
        ))),
        Some((_, s, error)) => Err(fail(format!(
            "finished with status `{}`: {}{}",
            if s == JobStatus::Cancelled {
                "cancelled"
            } else {
                "failed"
            },
            error.as_deref().unwrap_or("no error message"),
            stderr()
        ))),
        None if status.success() => Err(fail(format!(
            "exited without a `finished` event{}{}",
            hint(Some(status)),
            stderr()
        ))),
        None => Err(fail(format!(
            "{} before `finished`{}{}",
            describe_exit(status),
            hint(Some(status)),
            stderr()
        ))),
    }
}

/// Send each stdout line (without its line ending) until EOF. A line longer than
/// the protocol allows is sent truncated; the parser refuses it.
fn read_lines(stdout: impl Read, tx: &mpsc::Sender<Msg>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut buf = Vec::new();
        match (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 2)
            .read_until(b'\n', &mut buf)
        {
            Ok(0) => {
                let _ = tx.send(Msg::Eof);
                return;
            }
            Ok(_) => {
                let complete = buf.last() == Some(&b'\n');
                if complete {
                    buf.pop();
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                }
                let too_long = !complete && buf.len() > MAX_LINE_BYTES;
                if tx.send(Msg::Line(buf)).is_err() || too_long {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(Msg::Error(e));
                return;
            }
        }
    }
}

/// `"; stderr (<path>): <last lines>"`, or nothing if stderr is empty.
fn stderr_tail(dir: &Path) -> String {
    let path = dir.join(STDERR_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    let tail = lines[lines.len().saturating_sub(10)..].join(" | ");
    let tail: String = if tail.chars().count() > 2000 {
        let skip = tail.chars().count() - 2000;
        format!("…{}", tail.chars().skip(skip).collect::<String>())
    } else {
        tail
    };
    format!("; stderr ({}): {tail}", path.display())
}
