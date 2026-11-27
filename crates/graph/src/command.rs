//! The command path of a [`Job`] (ADR-0009 §1): prepare and collect, run on the
//! controller side by whichever executor holds the job.
//!
//! An executor that runs command jobs (`mold-jobs`: local, WSL2, SSH) does, per job:
//!
//! ```text
//! handle.started();
//! let run_dir = /* fresh per (job.key, job.attempt); the executor's choice */;
//! let r = job.cook_command(&run_dir, &handle, |cmd, events| {
//!     // launch cmd.worker with cmd.spec, stage cmd.stage if needed,
//!     // events.event(&json)? for each worker event as it arrives,
//!     // stop (kill the process group) when events.cancel() is set,
//!     // and return Ok(final event) or Err(the failure)
//! });
//! handle.finish(r);
//! ```
//!
//! `cook_command` cooks a Rust-native item (`prepare` returned `None`) in place, so
//! such an executor needs no second code path. graph never interprets the JSON.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use crate::exec::{Job, JobHandle};
use crate::node::{CancelToken, CommandContext, CommandJob, NodeError, Outputs};

/// Where a running command job's worker events go: each one is folded by
/// [`Processor::collect_event`](crate::Processor::collect_event) into the item's
/// partial outputs, which reach observers at once.
pub struct EventSink<'a> {
    job: &'a Job,
    run_dir: &'a Path,
    handle: &'a JobHandle,
    partial: Outputs,
    error: Option<NodeError>,
}

impl fmt::Debug for EventSink<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventSink")
            .field("job", &self.job.id)
            .field("failed", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

impl EventSink<'_> {
    /// Fold one worker event. An error (the node refused the event, or the cook was
    /// cancelled) means the runner should stop the worker; the job fails with that
    /// error whatever the runner returns.
    pub fn event(&mut self, event: &serde_json::Value) -> Result<(), NodeError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if self.job.cancel.is_cancelled() {
            return Err(NodeError::Cancelled);
        }
        let ctx = self.job.command_context(self.run_dir);
        let p = &self.job.processor;
        let partial = &mut self.partial;
        match guarded(|| p.collect_event(&ctx, event, partial)) {
            Ok(true) => {
                self.handle.partial(self.partial.clone());
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(e) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// The job's cancel token: set on cook cancel or timeout. A runner polls it (or
    /// relies on [`Executor::cancel`](crate::Executor::cancel)) to kill the worker.
    pub fn cancel(&self) -> &CancelToken {
        &self.job.cancel
    }

    /// The outputs folded so far.
    pub fn partial(&self) -> &Outputs {
        &self.partial
    }
}

impl Job {
    pub(crate) fn command_context<'a>(&'a self, run_dir: &'a Path) -> CommandContext<'a> {
        CommandContext {
            item: &self.item,
            params: &self.params,
            attrs: &self.attrs,
            inputs: &self.inputs,
            run_dir,
            cancel: &self.cancel,
        }
    }

    /// Phase 1: create `run_dir` and call [`Processor::prepare`](crate::Processor::prepare)
    /// in it. `None` = a Rust-native item (cook it with [`cook_here`](Self::cook_here)).
    /// Panics become [`NodeError::Panicked`].
    pub fn prepare(&self, run_dir: &Path) -> Result<Option<CommandJob>, NodeError> {
        if self.cancel.is_cancelled() {
            return Err(NodeError::Cancelled);
        }
        std::fs::create_dir_all(run_dir).map_err(|e| {
            NodeError::msg(format!(
                "cannot create run directory {}: {e}",
                run_dir.display()
            ))
        })?;
        let ctx = self.command_context(run_dir);
        guarded(|| self.processor.prepare(&ctx))
    }

    /// Phases 2 and 3 for a prepared job. `run` launches the worker wherever the
    /// executor says, passes each event to the [`EventSink`] as it arrives, and returns
    /// the final event (or the failure: crash, timeout, a failed event, as the runner
    /// reads them). Then [`Processor::collect_finish`](crate::Processor::collect_finish)
    /// turns it into the outputs. Partial outputs go to `handle` as they are folded.
    /// A cancel seen after the run returns gives [`NodeError::Cancelled`].
    pub fn run_command<R>(
        &self,
        cmd: &CommandJob,
        handle: &JobHandle,
        run: R,
    ) -> Result<Outputs, NodeError>
    where
        R: FnOnce(&CommandJob, &mut EventSink<'_>) -> Result<serde_json::Value, NodeError>,
    {
        let mut sink = EventSink {
            job: self,
            run_dir: &cmd.run_dir,
            handle,
            partial: Outputs::new(),
            error: None,
        };
        let finished = run(cmd, &mut sink);
        if let Some(e) = sink.error {
            return Err(e);
        }
        let finished = finished?;
        if self.cancel.is_cancelled() {
            return Err(NodeError::Cancelled);
        }
        let ctx = self.command_context(&cmd.run_dir);
        let partial = sink.partial;
        guarded(|| self.processor.collect_finish(&ctx, &finished, partial))
    }

    /// The whole item on a command executor: [`prepare`](Self::prepare) in `run_dir`,
    /// then [`run_command`](Self::run_command) with `run`, or, for a Rust-native item,
    /// [`cook_here`](Self::cook_here) (`run` is not called).
    pub fn cook_command<R>(
        &self,
        run_dir: &Path,
        handle: &JobHandle,
        run: R,
    ) -> Result<Outputs, NodeError>
    where
        R: FnOnce(&CommandJob, &mut EventSink<'_>) -> Result<serde_json::Value, NodeError>,
    {
        match self.prepare(run_dir)? {
            Some(cmd) => self.run_command(&cmd, handle, run),
            None => self.cook_here(handle),
        }
    }
}

/// Run node code, turning a panic into [`NodeError::Panicked`].
pub(crate) fn guarded<T>(f: impl FnOnce() -> Result<T, NodeError>) -> Result<T, NodeError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Err(NodeError::Panicked(msg))
    })
}
