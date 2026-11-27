//! What an item occupies (ADR-0009 §5) and what happens when it fails (§7).
//!
//! Neither is part of any cache key: like the executor, they decide *how* an item
//! runs, never *what* it computes.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The resources one work item occupies while it runs, from
/// [`Processor::needs`](crate::Processor::needs). An executor starts an item only
/// when its needs fit the free capacity (Houdini's "slots per work item").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Needs {
    /// Scheduler slots (≈ CPU cores). Default 1; an item that parallelises internally
    /// (a loader, a Polars transform: ADR-0011) declares more.
    pub slots: u32,
    /// GPUs. Default 0.
    pub gpus: u32,
    /// Memory in GB the item needs free; `None` (default) = no limit declared.
    pub mem_gb: Option<f64>,
}

impl Default for Needs {
    fn default() -> Self {
        Self {
            slots: 1,
            gpus: 0,
            mem_gb: None,
        }
    }
}

/// What to do when an item of a node fails, after its retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnFailure {
    /// The item is `Failed`; everything that depends on it is cancelled, including a
    /// partition (fan-in) it belongs to.
    #[default]
    Error,
    /// The item is `Failed` and shown as a warning; its own descendants are cancelled,
    /// but fan-in goes on without it: a partition drops it (and items cancelled because
    /// of it) and cooks with the remaining members.
    Warn,
    /// Like `Warn`, without the warning: a failure the user expects (filters, flaky
    /// optional steps).
    Ignore,
}

impl OnFailure {
    pub fn is_error(&self) -> bool {
        *self == Self::Error
    }

    /// Whether fan-in tolerates the failure.
    pub fn tolerated(self) -> bool {
        self != Self::Error
    }
}

/// A node's failure policy (ADR-0009 §7), resolved from its
/// [`NodeData`](crate::NodeData) by [`NodeData::failure_policy`](crate::NodeData::failure_policy)
/// and handed to the executor with each [`Job`](crate::Job).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FailurePolicy {
    /// Extra attempts after a failure (not after a cancel). Each attempt is a fresh
    /// run directory ([`Job::attempt`](crate::Job::attempt)).
    pub retries: u32,
    /// Maximum run time of one attempt, counted from the executor's `started` report.
    /// When it passes, the job is cancelled and the attempt fails with
    /// [`NodeError::TimedOut`](crate::NodeError::TimedOut).
    pub timeout: Option<Duration>,
    pub on_failure: OnFailure,
}
