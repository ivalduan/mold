//! What the executor tests share: the test-only worker node, its graph, and the
//! fixtures around them. Used by both `local_executor.rs` and `ssh_executor.rs`, so
//! the two flavours are measured against the *same* node and the same assertions.
//!
//! ```text
//! ligands (n items, Scalar = index, Rust-native) -> cofold (PoseSet, worker node)
//! ```
//!
//! [`Cofold`] is a test-only worker node: `prepare` copies the EGFR reference ligand
//! into the run directory and writes a `JobSpec` for mock-cofold; `collect_event`
//! turns each `pose` event into a partial output; `collect_finish` returns the
//! poses. It also has a Rust `cook` computing the same result in-process (the parity
//! reference). The real CoFold node is WS-07's.
//!
//! No helper here waits on a sleep: the mock's `gate` param holds a worker mid-stream
//! until the test's observer opens it, so every ordering asserted is forced.

// Each test binary compiles this module on its own, so one of them not using a
// helper is normal.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mold_graph::*;
use mold_jobs::protocol::JobSpec;
use mold_jobs::{Executors, Profiles, Workers};
use serde_json::json;

// ---- Fixtures -----------------------------------------------------------------------

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn ligand_sdf() -> PathBuf {
    repo().join("fixtures/egfr/reference_ligand.sdf")
}

/// The directory cargo built this package's binaries into (the worker and the SSH
/// test doubles).
pub fn bin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mock-cofold"))
        .parent()
        .expect("a binary has a directory")
        .to_path_buf()
}

/// The shipped manifests, with the freshly built mock first on the search path.
/// Also arms a watchdog: a scheduling bug must fail the run, never hang CI.
pub fn workers() -> Arc<Workers> {
    static WATCHDOG: std::sync::Once = std::sync::Once::new();
    WATCHDOG.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(300));
            eprintln!("executor tests: still running after 300 s, aborting");
            std::process::abort();
        });
    });
    Arc::new(
        Workers::load_dir(&repo().join("workers"))
            .unwrap()
            .with_search_path([bin_dir()]),
    )
}

/// A unique directory under the system temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("mold-jobs-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    /// The path with `/` separators, for a profile field that names a remote path.
    pub fn slashed(&self) -> String {
        self.0.to_string_lossy().replace('\\', "/")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // On Windows a just-emptied run directory can stay "in use" for a moment after
        // its processes are gone (antivirus, indexer); retry briefly instead of leaking
        // it into %TEMP%.
        for _ in 0..40 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

pub fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

/// How long a condition on other processes may take. Generous (a loaded machine can
/// take seconds to start a process); it only matters when something is broken, and
/// the watchdog (300 s) is the last resort.
pub const PATIENCE: Duration = Duration::from_secs(120);

/// Poll `cond` until it holds or `timeout` passes. For conditions on other
/// processes (a killed PID disappearing), never for ordering.
pub fn eventually(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

// ---- The nodes ----------------------------------------------------------------------

/// `n` ligands, each a Scalar `i64` equal to its index. Rust-native.
pub struct Ligands(pub i64);

impl Processor for Ligands {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("ligands").output("lig", PortType::Scalar)
    }
    fn generate(&self, _: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
        Ok((0..self.0)
            .map(|i| NewItem::work().attr("lig", i))
            .collect())
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let Some(AttrValue::Int(i)) = ctx.attrs.get("lig") else {
            return Err(NodeError::msg("no lig attr"));
        };
        Ok(Outputs::new().with("lig", Value::new(PortType::Scalar, *i)))
    }
}

/// One pose as the test node stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    pub index: u32,
    pub path: String,
    pub scores: BTreeMap<String, f64>,
}

pub fn poses_of(o: &Outputs) -> Vec<Pose> {
    o.get("poses")
        .and_then(|v| v.downcast_ref::<Vec<Pose>>())
        .cloned()
        .unwrap_or_default()
}

/// What the test node shares with the test: run directories by item, prepare counts.
#[derive(Debug, Default)]
pub struct Shared {
    pub run_dirs: Mutex<HashMap<String, Vec<PathBuf>>>,
}

impl Shared {
    pub fn dirs(&self, item: &str) -> Vec<PathBuf> {
        self.run_dirs
            .lock()
            .unwrap()
            .get(item)
            .cloned()
            .unwrap_or_default()
    }
    pub fn all_dirs(&self) -> Vec<PathBuf> {
        self.run_dirs
            .lock()
            .unwrap()
            .values()
            .flatten()
            .cloned()
            .collect()
    }
    pub fn prepares(&self) -> usize {
        self.all_dirs().len()
    }
}

/// The test-only worker node. Params: `worker`, `samples`, `seed`, `delay_ms`,
/// `gpus` (needs), `fail` + `fail_after` + `fail_lig` (-1 = every ligand) +
/// `fail_attempts` (inject only in the first n attempts; 0 = always), `gate` +
/// `gate_after`.
pub struct Cofold(pub Arc<Shared>);

pub fn cofold_spec() -> NodeSpec {
    NodeSpec::new("cofold")
        .input("lig", PortType::Scalar)
        .output("poses", PortType::PoseSet)
        .param(ParamSpec::new("worker", "mock-cofold"))
        .param(ParamSpec::new("samples", 3i64))
        .param(ParamSpec::new("seed", 0i64))
        .param(ParamSpec::new("delay_ms", 0i64))
        .param(ParamSpec::new("gpus", 0i64))
        .param(ParamSpec::new("fail", ""))
        .param(ParamSpec::new("fail_after", 1i64))
        .param(ParamSpec::new("fail_lig", -1i64))
        .param(ParamSpec::new("fail_attempts", 0i64))
        .param(ParamSpec::new("gate", ""))
        .param(ParamSpec::new("gate_after", 1i64))
}

impl Processor for Cofold {
    fn spec(&self) -> NodeSpec {
        cofold_spec()
    }

    fn needs(&self, params: &Params, _: &Attributes) -> Needs {
        Needs {
            gpus: params.int("gpus").unwrap_or(0) as u32,
            ..Needs::default()
        }
    }

    /// The parity reference: what mock-cofold computes, in Rust, in-process.
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let lig: i64 = *ctx.input("lig")?;
        let seed = (ctx.params.int("seed")? + lig) as u64;
        let samples = ctx.params.int("samples")? as u32;
        let poses: Vec<Pose> = (0..samples)
            .map(|k| Pose {
                index: k,
                path: mold_mock_cofold::pose_path(0, k),
                scores: mold_mock_cofold::scores(seed, 0, u64::from(k)),
            })
            .collect();
        Ok(Outputs::new().with("poses", Value::new(PortType::PoseSet, poses)))
    }

    fn prepare(&self, ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
        let lig: i64 = *ctx.inputs.one_as("lig")?;
        let p = ctx.params;
        let attempt = {
            let mut dirs = self.0.run_dirs.lock().unwrap();
            let mine = dirs.entry(ctx.item.to_string()).or_default();
            mine.push(ctx.run_dir.to_path_buf());
            mine.len() as i64
        };
        std::fs::copy(ligand_sdf(), ctx.run_dir.join("ligands.sdf"))
            .map_err(|e| NodeError::msg(e.to_string()))?;
        let mut params = json!({
            "samples": p.int("samples")?,
            "seed": p.int("seed")? + lig,
        });
        for k in ["delay_ms", "fail_after", "gate_after"] {
            params[k] = p.int(k)?.into();
        }
        let fail_lig = p.int("fail_lig")?;
        let fail_attempts = p.int("fail_attempts")?;
        let fail = p.str("fail")?;
        if !fail.is_empty()
            && (fail_lig < 0 || fail_lig == lig)
            && (fail_attempts == 0 || attempt <= fail_attempts)
        {
            params["fail"] = fail.into();
        }
        if !p.str("gate")?.is_empty() {
            params["gate"] = p.str("gate")?.into();
        }
        let spec = JobSpec::new(ctx.item.to_string())
            .input("ligands", "ligands.sdf")
            .params(params);
        Ok(Some(
            CommandJob::new(p.str("worker")?, ctx.run_dir, spec.to_value()).stage("ligands.sdf"),
        ))
    }

    fn collect_event(
        &self,
        _: &CommandContext<'_>,
        event: &serde_json::Value,
        partial: &mut Outputs,
    ) -> Result<bool, NodeError> {
        if event["event"] != "pose" {
            return Ok(false);
        }
        let pose = Pose {
            index: event["pose_index"].as_u64().unwrap() as u32,
            path: event["cif_path"].as_str().unwrap().to_string(),
            scores: serde_json::from_value(event["scores"].clone())
                .map_err(|e| NodeError::msg(e.to_string()))?,
        };
        let mut poses = poses_of(partial);
        poses.push(pose);
        partial
            .values
            .insert("poses".into(), Value::new(PortType::PoseSet, poses));
        Ok(true)
    }

    fn collect_finish(
        &self,
        ctx: &CommandContext<'_>,
        finished: &serde_json::Value,
        partial: Outputs,
    ) -> Result<Outputs, NodeError> {
        assert_eq!(finished["status"], "ok", "the runner only returns ok");
        let poses = poses_of(&partial);
        for p in &poses {
            if !ctx.run_dir.join(&p.path).is_file() {
                return Err(NodeError::msg(format!("missing pose file {}", p.path)));
            }
        }
        Ok(Outputs::new().with("poses", Value::new(PortType::PoseSet, poses)))
    }
}

/// Fan-in for the `on_failure` test: counts the poses of every member.
pub struct CountPoses;

impl Processor for CountPoses {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("count")
            .input("poses", PortType::PoseSet)
            .output("n", PortType::Scalar)
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let n: usize = ctx
            .inputs
            .all_as::<Vec<Pose>>("poses")?
            .iter()
            .map(|p| p.len())
            .sum();
        Ok(Outputs::new().with("n", Value::new(PortType::Scalar, n as i64)))
    }
}

// ---- Graphs, profiles, engines ------------------------------------------------------

pub fn registry(ligands: i64, shared: &Arc<Shared>) -> Arc<Registry> {
    let mut r = Registry::new();
    r.processor(Ligands(ligands))
        .unwrap()
        .processor(Cofold(shared.clone()))
        .unwrap()
        .processor(CountPoses)
        .unwrap()
        .partitioner(WaitForAll::new("all_poses", PortType::PoseSet))
        .unwrap();
    Arc::new(r)
}

/// `ligands -> cofold`, cofold on executor `executor` with `params`; a 60 s timeout
/// on cofold so a broken test fails instead of hanging.
pub fn graph(reg: &Arc<Registry>, executor: Option<&str>, params: &[(&str, ParamValue)]) -> Graph {
    let mut ed = Editor::new(reg.clone());
    ed.add_node_with_id("ligands", "ligands").unwrap();
    ed.add_node_with_id("cofold", "cofold").unwrap();
    ed.connect("ligands.lig", &id("cofold"), "lig").unwrap();
    let mut g = ed.graph().clone();
    let node = g.nodes.get_mut(&id("cofold")).unwrap();
    node.executor = executor.map(String::from);
    node.timeout = Some(60.0);
    for (k, v) in params {
        node.params.set(*k, v.clone());
    }
    g
}

pub fn profiles(toml: &str) -> Profiles {
    let mut p = Profiles::builtin();
    p.add_toml(toml, Path::new("test-executors.toml")).unwrap();
    p
}

/// `[executor.mock]` with the given capacity.
pub fn mock_profile(slots: u32, gpus: u32) -> Profiles {
    profiles(&format!(
        "[executor.mock]\nslots = {slots}\ngpus = {gpus}\n"
    ))
}

pub fn engine(
    executors: &Executors,
    reg: &Arc<Registry>,
    cache: Arc<dyn Cache>,
    g: &Graph,
) -> Engine {
    let e = Engine::new(reg.clone(), cache).with_executor(Arc::new(ThreadExecutor::new(2)));
    executors.attach(e, g).unwrap()
}

// ---- Reading what happened ----------------------------------------------------------

/// What the observer saw: (item, state, partial pose count), in order.
pub type Timeline = Arc<Mutex<Vec<(String, ItemState, usize)>>>;
pub type Log = Vec<(String, ItemState, usize)>;

pub fn cofold_only(log: &Log) -> Log {
    log.iter()
        .filter(|(n, ..)| n.starts_with("cofold"))
        .cloned()
        .collect()
}

/// Most cofold items in `Cooking` at once, from the timeline.
pub fn peak_cooking(log: &Log) -> usize {
    let mut state: HashMap<&str, ItemState> = HashMap::new();
    let mut peak = 0;
    for (n, s, _) in log {
        state.insert(n, *s);
        peak = peak.max(state.values().filter(|s| **s == ItemState::Cooking).count());
    }
    peak
}

/// Open `item`'s gate (the mock waits for `<run dir>/go`).
pub fn open_gate(shared: &Shared, item: &str) {
    for d in shared.dirs(item) {
        std::fs::write(d.join("go"), b"").unwrap();
    }
}

pub fn keys(items: &ItemGraph) -> Vec<(String, ContentHash)> {
    items.all().map(|it| (it.id.to_string(), it.key)).collect()
}

pub fn results(items: &ItemGraph) -> Vec<Vec<Pose>> {
    items
        .items(&id("cofold"))
        .iter()
        .map(|it| poses_of(it.result.as_ref().expect("cooked")))
        .collect()
}

/// `(item, job.json bytes, events.ndjson bytes)` for every cofold run: what must be
/// byte-identical across runs and across flavours.
pub fn run_files(shared: &Shared) -> Vec<(String, Vec<u8>, Vec<u8>)> {
    let mut v: Vec<_> = shared
        .run_dirs
        .lock()
        .unwrap()
        .iter()
        .map(|(item, dirs)| {
            let d = &dirs[0];
            (
                item.clone(),
                std::fs::read(d.join(mold_jobs::SPEC_FILE)).unwrap(),
                std::fs::read(d.join(mold_jobs::EVENTS_FILE)).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

/// `(worker pid, child pid)` from the `pids.json` the mock's `orphan` mode writes.
pub fn pids(dir: &Path) -> Option<(u32, u32)> {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("pids.json")).ok()?).ok()?;
    Some((v["worker"].as_u64()? as u32, v["child"].as_u64()? as u32))
}
