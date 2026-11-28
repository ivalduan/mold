//! Where and how work items run (ADR-0009). `mold-graph` decides *what* is ready;
//! this crate decides *where and how*, behind graph's [`Executor`] seam.
//!
//! - [`protocol`]: the worker wire protocol (a contract; `docs/worker-protocol.md`).
//! - [`Manifest`] / [`Workers`]: `workers/<name>/manifest.toml`, validated.
//! - [`Profiles`]: what an executor key means on this machine (`~/.mold/executors.toml`
//!   plus an optional site file; a built-in `local`).
//! - [`LocalExecutor`]: command items as subprocesses here, WSL2 included, with slot,
//!   GPU and memory accounting and process-tree kill.
//! - [`Executors`]: builds the executors a graph names, failing **before** the cook on
//!   an unknown key.
//!
//! Workers are separate processes, never linked in. See `CLAUDE.md` in this crate.

mod error;
mod local;
mod manifest;
mod process;
mod profile;
pub mod protocol;
mod runner;
mod sched;
mod wsl;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mold_graph::{Engine, Executor, Graph};

pub use error::ConfigError;
pub use local::LocalExecutor;
pub use manifest::{Environment, MANIFEST_FILE, Manifest, Workers};
pub use process::pid_alive;
pub use profile::{
    LOCAL, Origin, Profile, ProfileSources, Profiles, SITE_FILE_VAR, SshTarget, Target, Wsl,
};
pub use runner::{EVENTS_FILE, SPEC_FILE, STDERR_FILE};
pub use sched::{CommandExecutor, Notice};
pub use wsl::wsl_path;

/// The executors of one machine: profiles, workers and a run root, turned into
/// [`Executor`]s on demand, one per key (so slot accounting spans cooks).
#[derive(Debug)]
pub struct Executors {
    profiles: Profiles,
    workers: Arc<Workers>,
    run_root: PathBuf,
    built: Mutex<BTreeMap<String, Arc<dyn CommandExecutor>>>,
}

impl Executors {
    pub fn new(profiles: Profiles, workers: Arc<Workers>, run_root: impl Into<PathBuf>) -> Self {
        Self {
            profiles,
            workers,
            run_root: run_root.into(),
            built: Mutex::default(),
        }
    }

    pub fn profiles(&self) -> &Profiles {
        &self.profiles
    }

    /// The executor for `key`, built on first use. `used_by` names who asks, for the
    /// error message.
    pub fn executor(
        &self,
        key: &str,
        used_by: Vec<String>,
    ) -> Result<Arc<dyn CommandExecutor>, ConfigError> {
        let mut built = self.built.lock().expect("executors lock");
        if let Some(e) = built.get(key) {
            return Ok(e.clone());
        }
        let profile = self.profiles.resolve(key, used_by)?;
        let workers = self.workers.clone();
        let run_root = self.run_root.join(key);
        let e: Arc<dyn CommandExecutor> = Arc::new(LocalExecutor::new(profile, workers, run_root)?);
        built.insert(key.to_string(), e.clone());
        Ok(e)
    }

    /// Every executor key `graph` names (graph default and node overrides) with who
    /// names it.
    pub fn keys(graph: &Graph) -> BTreeMap<String, Vec<String>> {
        let mut keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some(k) = &graph.executor {
            keys.entry(k.clone())
                .or_default()
                .push("the graph default".into());
        }
        for (id, node) in &graph.nodes {
            if let Some(k) = &node.executor {
                keys.entry(k.clone())
                    .or_default()
                    .push(format!("node `{id}`"));
            }
        }
        keys
    }

    /// Check every key `graph` names, building its executor. Call before cooking: an
    /// unknown key, or one this build cannot run, fails here and no item runs.
    pub fn check(&self, graph: &Graph) -> Result<(), ConfigError> {
        for (key, used_by) in Self::keys(graph) {
            self.executor(&key, used_by)?;
        }
        Ok(())
    }

    /// [`check`](Self::check), then register each key's executor on `engine`.
    pub fn attach(&self, engine: Engine, graph: &Graph) -> Result<Engine, ConfigError> {
        let mut engine = engine;
        for (key, used_by) in Self::keys(graph) {
            let e: Arc<dyn Executor> = self.executor(&key, used_by)?;
            engine = engine.add_executor(key, e);
        }
        Ok(engine)
    }

    /// The remote profiles `graph` would send data to: the UI must say so before
    /// the first item is sent (product principle 4).
    pub fn remote_keys(&self, graph: &Graph) -> Vec<String> {
        Self::keys(graph)
            .into_keys()
            .filter(|k| self.profiles.get(k).is_some_and(|p| p.remote))
            .collect()
    }

    /// Notices from every executor built so far.
    pub fn take_notices(&self) -> Vec<Notice> {
        let built = self.built.lock().expect("executors lock");
        built.values().flat_map(|e| e.take_notices()).collect()
    }
}
