//! The local executor (ADR-0009 §2): command items as subprocesses on this machine,
//! WSL2 included (a `wsl.exe` prefix). Std threads, no async runtime.
//!
//! - `submit` queues the job; a job starts when its [`Needs`] fit the profile's free
//!   slots, GPUs and memory (later jobs that fit may start first: backfill).
//! - Each started job gets a thread and a fresh run directory
//!   `<run root>/<item key hex>/<attempt>`; then prepare → run → collect through
//!   [`Job::prepare`] / [`Job::run_command`].
//! - A worker item reports `started` (the item turns `Cooking` and the node's timeout
//!   starts) when the worker writes its first event, so prepare and process start-up
//!   never eat into the timeout; start-up has its own bound (60 s, or the timeout if
//!   longer). A Rust-native item reports it before cooking.
//! - Cancel and timeout (the engine sets the job's token and calls
//!   [`Executor::cancel`]) kill the worker's whole process tree.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mold_graph::{CommandJob, EventSink, Executor, Job, JobHandle, JobId, Needs, NodeError};

use crate::error::ConfigError;
use crate::manifest::Workers;
use crate::profile::{Profile, Target, Wsl};
use crate::protocol::JobSpec;
use crate::runner::{self, Launch};
use crate::wsl::wsl_command;

/// Something the executor wants the user to know, reported once (not an error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A Rust-native node was routed to this executor; its items cook in the app, as
    /// every Rust-native item does in v0.1 (ADR-0009 §2). Reported once per kind.
    NativeItemsCookInApp { kind: String, executor: String },
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NativeItemsCookInApp { kind, executor } => write!(
                f,
                "`{kind}` is a Rust-native node: its items cook in the app; \
                 the executor `{executor}` is ignored for it"
            ),
        }
    }
}

/// Runs command items as local subprocesses. See the [module docs](self).
pub struct LocalExecutor {
    inner: Arc<Inner>,
}

struct Inner {
    key: String,
    capacity: Needs,
    wsl: Option<Wsl>,
    workers: Arc<Workers>,
    run_root: PathBuf,
    state: Mutex<State>,
    idle: Condvar,
}

#[derive(Default)]
struct State {
    queue: VecDeque<(Job, JobHandle)>,
    used: Needs,
    /// Running jobs: their kill flag.
    running: HashMap<JobId, Arc<AtomicBool>>,
    dirs: HashSet<PathBuf>,
    threads: Vec<JoinHandle<()>>,
    notices: Vec<Notice>,
    noticed: BTreeSet<String>,
    shutdown: bool,
}

impl fmt::Debug for LocalExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalExecutor")
            .field("key", &self.inner.key)
            .field("capacity", &self.inner.capacity)
            .field("wsl", &self.inner.wsl)
            .finish_non_exhaustive()
    }
}

impl LocalExecutor {
    /// An executor for `profile` (a local or WSL2 one), launching `workers`, with run
    /// directories under `run_root`. An SSH profile is refused: not in this version.
    pub fn new(
        profile: &Profile,
        workers: Arc<Workers>,
        run_root: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let wsl = match &profile.target {
            Target::Local { wsl } => wsl.clone(),
            Target::Ssh(_) => {
                return Err(ConfigError::NotYet {
                    key: profile.key.clone(),
                    what: "SSH executors are not available yet (planned: stdio over SSH \
                           with run-directory staging, ADR-0009 §2-3)"
                        .into(),
                });
            }
        };
        if wsl.is_some() && !cfg!(windows) {
            return Err(ConfigError::NotYet {
                key: profile.key.clone(),
                what: "a `wsl` profile runs only on Windows".into(),
            });
        }
        Ok(Self {
            inner: Arc::new(Inner {
                key: profile.key.clone(),
                capacity: Needs {
                    slots: profile.slots,
                    gpus: profile.gpus,
                    mem_gb: profile.mem_gb,
                },
                wsl,
                workers,
                run_root: run_root.into(),
                // Not `Needs::default()`: that is one slot, and nothing is in use yet.
                state: Mutex::new(State {
                    used: Needs {
                        slots: 0,
                        gpus: 0,
                        mem_gb: None,
                    },
                    ..State::default()
                }),
                idle: Condvar::new(),
            }),
        })
    }

    pub fn key(&self) -> &str {
        &self.inner.key
    }

    /// Notices since the last call (each is reported once per executor).
    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut self.inner.lock().notices)
    }

    /// Wait until nothing is queued or running. `false` on timeout.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut s = self.inner.lock();
        while !(s.queue.is_empty() && s.running.is_empty()) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = self.inner.idle.wait_timeout(s, left).expect("state lock").0;
        }
        true
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("state lock")
    }

    /// Why `needs` can never fit this profile, if so.
    fn never_fits(&self, needs: &Needs) -> Option<String> {
        let cap = &self.capacity;
        let what = if needs.slots > cap.slots {
            format!("{} slots, and it has {}", needs.slots, cap.slots)
        } else if needs.gpus > cap.gpus {
            format!("{} GPU(s), and it has {}", needs.gpus, cap.gpus)
        } else {
            match (needs.mem_gb, cap.mem_gb) {
                (Some(n), Some(c)) if n > c => format!("{n} GB of memory, and it has {c}"),
                _ => return None,
            }
        };
        Some(format!(
            "the item needs {what}; declare more on executor `{}` in its profile \
             (~/.mold/executors.toml)",
            self.key
        ))
    }

    fn fits_now(&self, used: &Needs, needs: &Needs) -> bool {
        let cap = &self.capacity;
        let mem_ok = match (needs.mem_gb, cap.mem_gb) {
            (Some(n), Some(c)) => used.mem_gb.unwrap_or(0.0) + n <= c + 1e-9,
            _ => true,
        };
        used.slots + needs.slots <= cap.slots && used.gpus + needs.gpus <= cap.gpus && mem_ok
    }

    /// Start every queued job that fits, in queue order.
    fn dispatch(self: &Arc<Self>, s: &mut State) {
        s.threads.retain(|t| !t.is_finished());
        let mut i = 0;
        while i < s.queue.len() {
            let (job, _) = &s.queue[i];
            if job.cancel.is_cancelled() {
                let (_, handle) = s.queue.remove(i).expect("index in range");
                handle.finish(Err(NodeError::Cancelled));
                continue;
            }
            if !self.fits_now(&s.used, &job.needs) {
                i += 1;
                continue;
            }
            let (job, handle) = s.queue.remove(i).expect("index in range");
            reserve(&mut s.used, &job.needs, 1.0);
            let kill = Arc::new(AtomicBool::new(false));
            s.running.insert(job.id, kill.clone());
            let dir = self.run_dir(&job, &mut s.dirs);
            let me = self.clone();
            let t = std::thread::Builder::new()
                .name(format!("mold-local-{}", job.id.0))
                .spawn(move || me.run_job(job, handle, dir, kill))
                .expect("spawn job thread");
            s.threads.push(t);
        }
    }

    /// `<root>/<key hex>/<attempt>`, unique among running jobs (two items may share a
    /// key; the second gets `<attempt>.1`, …).
    fn run_dir(&self, job: &Job, dirs: &mut HashSet<PathBuf>) -> PathBuf {
        let base = self.run_root.join(job.key.to_hex());
        let mut dir = base.join(job.attempt.to_string());
        let mut n = 0;
        while dirs.contains(&dir) {
            n += 1;
            dir = base.join(format!("{}.{n}", job.attempt));
        }
        dirs.insert(dir.clone());
        dir
    }

    fn run_job(self: Arc<Self>, job: Job, handle: JobHandle, dir: PathBuf, kill: Arc<AtomicBool>) {
        // Released however this thread ends (a panic included).
        let _release = Release {
            inner: self.clone(),
            id: job.id,
            needs: job.needs,
            dir: dir.clone(),
        };
        // `started` (which starts the node's timeout) is reported in `cook`: for a
        // worker, once it has written its first event.
        let r = self.cook(&job, &handle, &dir, &kill);
        // Finish before releasing capacity, so the next job's `started` reaches the
        // engine after this one's result.
        handle.finish(r);
    }

    fn cook(
        &self,
        job: &Job,
        handle: &JobHandle,
        dir: &Path,
        kill: &AtomicBool,
    ) -> Result<mold_graph::Outputs, NodeError> {
        // A fresh run directory per (key, attempt): clear what an earlier session left.
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| {
                NodeError::msg(format!("cannot clear run directory {}: {e}", dir.display()))
            })?;
        }
        match job.prepare(dir)? {
            Some(cmd) => job.run_command(&cmd, handle, |cmd, events| {
                self.run_worker(job, cmd, events, kill, &|| handle.started())
            }),
            None => {
                let _ = std::fs::remove_dir(dir);
                self.notice_native(&job.kind);
                handle.started();
                job.cook_here(handle)
            }
        }
    }

    fn notice_native(&self, kind: &str) {
        let mut s = self.lock();
        if s.noticed.insert(kind.to_string()) {
            s.notices.push(Notice::NativeItemsCookInApp {
                kind: kind.to_string(),
                executor: self.key.clone(),
            });
        }
    }

    fn run_worker(
        &self,
        job: &Job,
        cmd: &CommandJob,
        events: &mut EventSink<'_>,
        kill: &AtomicBool,
        up: &dyn Fn(),
    ) -> Result<serde_json::Value, NodeError> {
        let spec = JobSpec::from_value(&cmd.spec)
            .map_err(|e| NodeError::msg(format!("node `{}` prepared {e}", job.kind)))?;
        for (name, path) in &spec.inputs {
            if !cmd.run_dir.join(path).exists() {
                return Err(NodeError::msg(format!(
                    "node `{}` prepared input `{name}` ({}) but it is not in the run directory",
                    job.kind,
                    path.display()
                )));
            }
        }
        let launch = self.launch(job, cmd)?;
        let cancel = job.cancel.clone();
        let stop = move || cancel.is_cancelled() || kill.load(Ordering::Relaxed);
        let startup_limit = job
            .policy
            .timeout
            .map_or(runner::STARTUP_LIMIT, |t| t.max(runner::STARTUP_LIMIT));
        let control = runner::Control {
            stop: &stop,
            up,
            startup_limit,
        };
        runner::run(&cmd.worker, &launch, cmd, &spec, events, &control)
    }

    /// The command line for `cmd.worker` on this profile.
    fn launch(&self, job: &Job, cmd: &CommandJob) -> Result<Launch, NodeError> {
        let worker = &cmd.worker;
        let m = self.workers.get(worker).ok_or_else(|| {
            let known: Vec<&str> = self.workers.names().collect();
            NodeError::msg(format!(
                "unknown worker `{worker}` (known: {})",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ))
        })?;
        if m.needs_gpu && job.needs.gpus == 0 {
            return Err(NodeError::msg(format!(
                "worker `{worker}` needs a GPU (manifest `needs_gpu`), but node `{}` \
                 declares no GPU in `Processor::needs`; use `Manifest::needs`",
                job.kind
            )));
        }
        let argv = m.argv();
        if let Some(wsl) = &self.wsl {
            let (program, args) = wsl_command(wsl, &cmd.run_dir, &argv)
                .map_err(|e| NodeError::msg(format!("worker `{worker}` via WSL: {e}")))?;
            return Ok(Launch { program, args });
        }
        let program = self
            .workers
            .resolve_program(&argv[0], m)
            .map_err(|e| NodeError::msg(format!("worker `{worker}`: {e}")))?;
        Ok(Launch {
            program: program.into_os_string(),
            args: argv[1..].iter().map(Into::into).collect(),
        })
    }
}

fn reserve(used: &mut Needs, needs: &Needs, sign: f64) {
    if sign > 0.0 {
        used.slots += needs.slots;
        used.gpus += needs.gpus;
    } else {
        used.slots -= needs.slots;
        used.gpus -= needs.gpus;
    }
    if let Some(m) = needs.mem_gb {
        used.mem_gb = Some((used.mem_gb.unwrap_or(0.0) + sign * m).max(0.0));
    }
}

/// Gives a running job's capacity and run directory back, then starts what now fits.
struct Release {
    inner: Arc<Inner>,
    id: JobId,
    needs: Needs,
    dir: PathBuf,
}

impl Drop for Release {
    fn drop(&mut self) {
        let mut s = self.inner.lock();
        reserve(&mut s.used, &self.needs, -1.0);
        s.running.remove(&self.id);
        s.dirs.remove(&self.dir);
        if !s.shutdown {
            self.inner.dispatch(&mut s);
        }
        drop(s);
        self.inner.idle.notify_all();
    }
}

impl Executor for LocalExecutor {
    fn submit(&self, job: Job, handle: JobHandle) {
        if let Some(why) = self.inner.never_fits(&job.needs) {
            handle.finish(Err(NodeError::msg(why)));
            return;
        }
        let mut s = self.inner.lock();
        if s.shutdown {
            return; // dropping the handle reports `Lost`
        }
        s.queue.push_back((job, handle));
        self.inner.dispatch(&mut s);
    }

    fn cancel(&self, id: JobId) {
        let mut s = self.inner.lock();
        if let Some(i) = s.queue.iter().position(|(j, _)| j.id == id) {
            let (_, handle) = s.queue.remove(i).expect("index in range");
            handle.finish(Err(NodeError::Cancelled));
        } else if let Some(kill) = s.running.get(&id) {
            // The runner polls this (and the job's token) and kills the tree.
            kill.store(true, Ordering::Relaxed);
        }
        drop(s);
        self.inner.idle.notify_all();
    }
}

impl Drop for LocalExecutor {
    fn drop(&mut self) {
        let threads = {
            let mut s = self.inner.lock();
            s.shutdown = true;
            // Queued handles are dropped: the engine (if still listening) sees `Lost`.
            s.queue.clear();
            for kill in s.running.values() {
                kill.store(true, Ordering::Relaxed);
            }
            std::mem::take(&mut s.threads)
        };
        for t in threads {
            let _ = t.join();
        }
    }
}
