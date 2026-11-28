//! The scheduler every command executor shares (ADR-0009 §2, §5).
//!
//! One queue per executor key, slot/GPU/memory accounting against the profile's
//! capacity, a fresh run directory per (item key, attempt), and the
//! prepare → run → collect cycle per job. Std threads, one per running job; no
//! dispatcher thread, no async runtime.
//!
//! Only phase *run* differs between flavours, so it is the only thing behind a
//! trait ([`Flavour`]): [`LocalExecutor`](crate::LocalExecutor) launches the worker
//! here (optionally through `wsl.exe`), [`SshExecutor`](crate::SshExecutor) launches
//! it on a remote host over `ssh`. Everything else — queueing, capacity, run
//! directories, the `JobSpec` check, cancellation, notices — is in this module, so
//! the flavours cannot drift apart.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mold_graph::{CommandJob, EventSink, Job, JobHandle, JobId, Needs, NodeError, Outputs};

use crate::manifest::{Manifest, Workers};
use crate::profile::Profile;
use crate::protocol::JobSpec;
use crate::runner;

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

/// An executor built from one profile: [`LocalExecutor`](crate::LocalExecutor) or
/// [`SshExecutor`](crate::SshExecutor). What [`Executors`](crate::Executors) hands
/// back for a key.
pub trait CommandExecutor: mold_graph::Executor {
    /// The executor key this was built for.
    fn key(&self) -> &str;
    /// Whether data sent here leaves this machine (the UI must say so first).
    fn is_remote(&self) -> bool;
    /// Notices since the last call (each is reported once per executor).
    fn take_notices(&self) -> Vec<Notice>;
    /// Wait until nothing is queued or running. `false` on timeout.
    fn wait_idle(&self, timeout: Duration) -> bool;
}

/// Phase *run* (ADR-0009 §1): where and how the worker itself is launched.
pub(crate) trait Flavour: Send + Sync + fmt::Debug {
    /// Launch the worker for one prepared job, relay its events into `events` as they
    /// arrive, and return its `finished` event or the failure. Every path out must
    /// leave nothing the worker started running, here or on the far side.
    fn run(
        &self,
        r: &RunSpec<'_>,
        events: &mut EventSink<'_>,
    ) -> Result<serde_json::Value, NodeError>;
}

/// Everything a [`Flavour`] needs for one run. The run directory is `r.cmd.run_dir`.
pub(crate) struct RunSpec<'a> {
    pub job: &'a Job,
    pub cmd: &'a CommandJob,
    /// `cmd.spec`, parsed and validated.
    pub spec: &'a JobSpec,
    /// Turns true on cancel or timeout: kill the worker and stop.
    pub stop: &'a dyn Fn() -> bool,
    /// Called once, at the worker's first event: the attempt is running from then on
    /// (this is what starts the node's timeout).
    pub up: &'a dyn Fn(),
    /// How long the worker may take, from being launched, to write its first event.
    pub startup_limit: Duration,
}

/// The queue, capacity and run directories of one executor key.
pub(crate) struct Scheduler {
    key: String,
    remote: bool,
    capacity: Needs,
    run_root: PathBuf,
    flavour: Box<dyn Flavour>,
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

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scheduler")
            .field("key", &self.key)
            .field("capacity", &self.capacity)
            .field("flavour", &self.flavour)
            .finish_non_exhaustive()
    }
}

impl Scheduler {
    pub(crate) fn new(
        profile: &Profile,
        run_root: PathBuf,
        flavour: Box<dyn Flavour>,
    ) -> Arc<Self> {
        Arc::new(Self {
            key: profile.key.clone(),
            remote: profile.remote,
            capacity: Needs {
                slots: profile.slots,
                gpus: profile.gpus,
                mem_gb: profile.mem_gb,
            },
            run_root,
            flavour,
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
        })
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn is_remote(&self) -> bool {
        self.remote
    }

    pub(crate) fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut self.lock().notices)
    }

    pub(crate) fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut s = self.lock();
        while !(s.queue.is_empty() && s.running.is_empty()) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = self.idle.wait_timeout(s, left).expect("state lock").0;
        }
        true
    }

    /// [`Executor::submit`](mold_graph::Executor::submit).
    pub(crate) fn submit(self: &Arc<Self>, job: Job, handle: JobHandle) {
        if let Some(why) = self.never_fits(&job.needs) {
            handle.finish(Err(NodeError::msg(why)));
            return;
        }
        let mut s = self.lock();
        if s.shutdown {
            return; // dropping the handle reports `Lost`
        }
        s.queue.push_back((job, handle));
        self.dispatch(&mut s);
    }

    /// [`Executor::cancel`](mold_graph::Executor::cancel).
    pub(crate) fn cancel(&self, id: JobId) {
        let mut s = self.lock();
        if let Some(i) = s.queue.iter().position(|(j, _)| j.id == id) {
            let (_, handle) = s.queue.remove(i).expect("index in range");
            handle.finish(Err(NodeError::Cancelled));
        } else if let Some(kill) = s.running.get(&id) {
            // The flavour polls this (and the job's token) and kills the worker.
            kill.store(true, Ordering::Relaxed);
        }
        drop(s);
        self.idle.notify_all();
    }

    /// Stop taking work, kill what is running, and join every job thread.
    pub(crate) fn shutdown(&self) {
        let threads = {
            let mut s = self.lock();
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
                .name(format!("mold-job-{}", job.id.0))
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
            sched: self.clone(),
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
    ) -> Result<Outputs, NodeError> {
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

    /// Check what the node prepared, then hand the run to the flavour.
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
        let cancel = job.cancel.clone();
        let stop = move || cancel.is_cancelled() || kill.load(Ordering::Relaxed);
        let startup_limit = job
            .policy
            .timeout
            .map_or(runner::STARTUP_LIMIT, |t| t.max(runner::STARTUP_LIMIT));
        let r = RunSpec {
            job,
            cmd,
            spec: &spec,
            stop: &stop,
            up,
            startup_limit,
        };
        self.flavour.run(&r, events)
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
    sched: Arc<Scheduler>,
    id: JobId,
    needs: Needs,
    dir: PathBuf,
}

impl Drop for Release {
    fn drop(&mut self) {
        let mut s = self.sched.lock();
        reserve(&mut s.used, &self.needs, -1.0);
        s.running.remove(&self.id);
        s.dirs.remove(&self.dir);
        if !s.shutdown {
            self.sched.dispatch(&mut s);
        }
        drop(s);
        self.sched.idle.notify_all();
    }
}

/// The manifest for `worker`, checked against what the node declared it occupies.
pub(crate) fn manifest_for<'a>(
    workers: &'a Workers,
    job: &Job,
    worker: &str,
) -> Result<&'a Manifest, NodeError> {
    let m = workers.get(worker).ok_or_else(|| {
        let known: Vec<&str> = workers.names().collect();
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
    Ok(m)
}

/// The public face of an executor: the thin wrapper around a [`Scheduler`] that
/// `Executors` builds. Both flavours get the same scheduling behaviour by
/// construction.
macro_rules! command_executor {
    ($t:ident) => {
        impl std::fmt::Debug for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_tuple(stringify!($t)).field(&self.sched).finish()
            }
        }

        impl $crate::sched::CommandExecutor for $t {
            fn key(&self) -> &str {
                self.sched.key()
            }
            fn is_remote(&self) -> bool {
                self.sched.is_remote()
            }
            fn take_notices(&self) -> Vec<$crate::sched::Notice> {
                self.sched.take_notices()
            }
            fn wait_idle(&self, timeout: std::time::Duration) -> bool {
                self.sched.wait_idle(timeout)
            }
        }

        impl mold_graph::Executor for $t {
            fn submit(&self, job: mold_graph::Job, handle: mold_graph::JobHandle) {
                self.sched.submit(job, handle);
            }
            fn cancel(&self, id: mold_graph::JobId) {
                self.sched.cancel(id);
            }
        }

        impl Drop for $t {
            fn drop(&mut self) {
                self.sched.shutdown();
            }
        }
    };
}

pub(crate) use command_executor;
