//! The executor seam: how a scheduled work item's cook gets run.
//!
//! The engine decides *what* is ready (generation, dependencies, cache); an
//! [`Executor`] decides *where and how* it runs. Completion comes back through a
//! [`JobHandle`], from any thread, in any order, at any later time.

use std::collections::VecDeque;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::hash::ContentHash;
use crate::item::ItemId;
use crate::node::{CancelToken, CookContext, Inputs, NodeError, Outputs, Processor};
use crate::param::{Attributes, Params};

/// Identifies one submitted job within a cook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u64);

/// One work item's cook, with everything it needs, owned.
pub struct Job {
    pub id: JobId,
    pub item: ItemId,
    /// The item's cache key: a stable name for its result (run dirs, remote jobs).
    pub key: ContentHash,
    pub kind: String,
    pub processor: Arc<dyn Processor>,
    pub params: Arc<Params>,
    pub attrs: Attributes,
    pub inputs: Inputs,
    pub cancel: CancelToken,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job")
            .field("id", &self.id)
            .field("item", &self.item)
            .field("kind", &self.kind)
            .finish()
    }
}

impl Job {
    /// Cook in the current thread: calls [`Processor::cook`], turning a panic into
    /// [`NodeError::Panicked`] and honouring a cancel that arrived before the start.
    /// Partial outputs are forwarded to `handle`.
    pub fn cook_here(&self, handle: &JobHandle) -> Result<Outputs, NodeError> {
        if self.cancel.is_cancelled() {
            return Err(NodeError::Cancelled);
        }
        let partial = |o: Outputs| handle.partial(o);
        let ctx = CookContext {
            item: &self.item,
            params: &self.params,
            attrs: &self.attrs,
            inputs: &self.inputs,
            cancel: &self.cancel,
            partial: Some(&partial),
        };
        match catch_unwind(AssertUnwindSafe(|| self.processor.cook(&ctx))) {
            Ok(r) => r,
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                Err(NodeError::Panicked(msg))
            }
        }
    }
}

/// What a job reports back.
#[derive(Debug)]
pub enum JobEvent {
    Started,
    Partial(Outputs),
    Finished(Result<Outputs, NodeError>),
}

/// The way back to the scheduler for one job. `Send`; may be moved to any thread and
/// kept for as long as the job takes. Dropping it unfinished reports
/// [`NodeError::Lost`], so a crashed executor never hangs a cook. Reports after the
/// cook has ended (e.g. it was cancelled) are discarded.
pub struct JobHandle {
    id: JobId,
    tx: Option<Sender<(JobId, JobEvent)>>,
}

impl fmt::Debug for JobHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JobHandle({:?})", self.id)
    }
}

impl JobHandle {
    pub(crate) fn new(id: JobId, tx: Sender<(JobId, JobEvent)>) -> Self {
        Self { id, tx: Some(tx) }
    }

    pub fn id(&self) -> JobId {
        self.id
    }

    fn send(&self, ev: JobEvent) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((self.id, ev));
        }
    }

    pub fn started(&self) {
        self.send(JobEvent::Started);
    }

    pub fn partial(&self, outputs: Outputs) {
        self.send(JobEvent::Partial(outputs));
    }

    pub fn finish(mut self, result: Result<Outputs, NodeError>) {
        self.send(JobEvent::Finished(result));
        self.tx = None;
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.tx.is_some() {
            self.send(JobEvent::Finished(Err(NodeError::Lost)));
        }
    }
}

/// Runs work items. Deliberately small: in-process threads, a local subprocess and a
/// farm/cluster scheduler must all fit behind it.
///
/// `submit` must not block on the work: queue it and return. Completion is reported
/// through the handle from whatever thread, whenever.
pub trait Executor: Send + Sync + fmt::Debug {
    fn submit(&self, job: Job, handle: JobHandle);

    /// Best-effort stop of a submitted job. The job's [`CancelToken`] is already set
    /// when this is called; in-process executors need nothing more.
    fn cancel(&self, _job: JobId) {}
}

type Queue = Mutex<(VecDeque<(Job, JobHandle)>, bool)>;

/// The one shipped executor: a fixed pool of std threads cooking jobs in-process.
pub struct ThreadExecutor {
    shared: Arc<(Queue, Condvar)>,
    threads: Vec<JoinHandle<()>>,
}

impl fmt::Debug for ThreadExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThreadExecutor({} threads)", self.threads.len())
    }
}

impl ThreadExecutor {
    /// `threads` workers (at least one).
    pub fn new(threads: usize) -> Self {
        let shared: Arc<(Queue, Condvar)> =
            Arc::new((Mutex::new((VecDeque::new(), false)), Condvar::new()));
        let threads = (0..threads.max(1))
            .map(|n| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("mold-cook-{n}"))
                    .spawn(move || worker(&shared))
                    .expect("spawn cook thread")
            })
            .collect();
        Self { shared, threads }
    }

    /// Default size: the machine's parallelism, at least 4 (many cooks wait on I/O
    /// or on a worker process rather than burn CPU).
    pub fn with_default_threads() -> Self {
        Self::new(
            std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .max(4),
        )
    }
}

fn worker(shared: &(Queue, Condvar)) {
    let (lock, cv) = shared;
    loop {
        let (job, handle) = {
            let mut q = lock.lock().expect("queue lock");
            loop {
                if let Some(j) = q.0.pop_front() {
                    break j;
                }
                if q.1 {
                    return;
                }
                q = cv.wait(q).expect("queue lock");
            }
        };
        handle.started();
        let r = job.cook_here(&handle);
        handle.finish(r);
    }
}

impl Executor for ThreadExecutor {
    fn submit(&self, job: Job, handle: JobHandle) {
        let (lock, cv) = &*self.shared;
        lock.lock().expect("queue lock").0.push_back((job, handle));
        cv.notify_one();
    }
}

impl Drop for ThreadExecutor {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        if let Ok(mut q) = lock.lock() {
            q.1 = true;
            // Queued jobs are dropped: their handles report `Lost`.
            q.0.clear();
        }
        cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
