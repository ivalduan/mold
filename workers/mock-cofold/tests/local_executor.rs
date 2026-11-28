//! The local executor running `mock-cofold` end to end (crates/jobs/CLAUDE.md,
//! acceptance tests). These live in the mock's package because only its own tests
//! see `CARGO_BIN_EXE_mock-cofold`: cargo builds the binary before running them, on
//! every platform, under plain `cargo test --workspace`.
//!
//! ```text
//! ligands (n items, Scalar = index, Rust-native) -> cofold (PoseSet, worker node)
//! ```
//!
//! `cofold` is a test-only worker node: `prepare` copies the EGFR reference ligand
//! into the run directory and writes a `JobSpec` for mock-cofold; `collect_event`
//! turns each `pose` event into a partial output; `collect_finish` returns the
//! poses. It also has a Rust `cook` computing the same result in-process (the parity
//! reference). The real CoFold node is WS-07's.
//!
//! No test waits on a sleep: the mock's `gate` param holds a worker mid-stream until
//! the test's observer opens it, so every ordering asserted here is forced.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mold_graph::*;
use mold_jobs::protocol::JobSpec;
use mold_jobs::{ConfigError, Executors, Notice, Profiles, Workers, pid_alive};
use serde_json::json;

// ---- Fixtures -----------------------------------------------------------------------

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ligand_sdf() -> PathBuf {
    repo().join("fixtures/egfr/reference_ligand.sdf")
}

/// The shipped manifests, with the freshly built mock first on the search path.
/// Also arms a watchdog: a scheduling bug must fail the run, never hang CI.
fn workers() -> Arc<Workers> {
    static WATCHDOG: std::sync::Once = std::sync::Once::new();
    WATCHDOG.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(300));
            eprintln!("local_executor tests: still running after 300 s, aborting");
            std::process::abort();
        });
    });
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_mock-cofold"));
    Arc::new(
        Workers::load_dir(&repo().join("workers"))
            .unwrap()
            .with_search_path([exe.parent().unwrap().to_path_buf()]),
    )
}

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("mold-jobs-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

/// Poll `cond` until it holds or `timeout` passes. For conditions on other
/// processes (a killed PID disappearing), never for ordering.
fn eventually(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

/// `n` ligands, each a Scalar `i64` equal to its index. Rust-native.
struct Ligands(i64);

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
struct Pose {
    index: u32,
    path: String,
    scores: BTreeMap<String, f64>,
}

fn poses_of(o: &Outputs) -> Vec<Pose> {
    o.get("poses")
        .and_then(|v| v.downcast_ref::<Vec<Pose>>())
        .cloned()
        .unwrap_or_default()
}

/// What the test node shares with the test: run directories by item, prepare counts.
#[derive(Debug, Default)]
struct Shared {
    run_dirs: Mutex<HashMap<String, Vec<PathBuf>>>,
}

impl Shared {
    fn dirs(&self, item: &str) -> Vec<PathBuf> {
        self.run_dirs
            .lock()
            .unwrap()
            .get(item)
            .cloned()
            .unwrap_or_default()
    }
    fn all_dirs(&self) -> Vec<PathBuf> {
        self.run_dirs
            .lock()
            .unwrap()
            .values()
            .flatten()
            .cloned()
            .collect()
    }
    fn prepares(&self) -> usize {
        self.all_dirs().len()
    }
}

/// The test-only worker node. Params: `samples`, `seed`, `delay_ms`, `gpus` (needs),
/// `fail` + `fail_after` + `fail_lig` (-1 = every ligand) + `fail_attempts` (inject
/// only in the first n attempts; 0 = always), `gate` + `gate_after`.
struct Cofold(Arc<Shared>);

fn cofold_spec() -> NodeSpec {
    NodeSpec::new("cofold")
        .input("lig", PortType::Scalar)
        .output("poses", PortType::PoseSet)
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
            CommandJob::new("mock-cofold", ctx.run_dir, spec.to_value()).stage("ligands.sdf"),
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
struct CountPoses;

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

fn registry(ligands: i64, shared: &Arc<Shared>) -> Arc<Registry> {
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
fn graph(reg: &Arc<Registry>, executor: Option<&str>, params: &[(&str, ParamValue)]) -> Graph {
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

fn profiles(toml: &str) -> Profiles {
    let mut p = Profiles::builtin();
    p.add_toml(toml, Path::new("test-executors.toml")).unwrap();
    p
}

/// `[executor.mock]` with the given capacity.
fn mock_profile(slots: u32, gpus: u32) -> Profiles {
    profiles(&format!(
        "[executor.mock]\nslots = {slots}\ngpus = {gpus}\n"
    ))
}

fn engine(executors: &Executors, reg: &Arc<Registry>, cache: Arc<dyn Cache>, g: &Graph) -> Engine {
    let e = Engine::new(reg.clone(), cache).with_executor(Arc::new(ThreadExecutor::new(2)));
    executors.attach(e, g).unwrap()
}

/// What the observer saw: (item, state, partial pose count), in order.
type Timeline = Arc<Mutex<Vec<(String, ItemState, usize)>>>;

fn cofold_only(log: &[(String, ItemState, usize)]) -> Vec<(String, ItemState, usize)> {
    log.iter()
        .filter(|(n, ..)| n.starts_with("cofold"))
        .cloned()
        .collect()
}

/// Most cofold items in `Cooking` at once, from the timeline.
fn peak_cooking(log: &[(String, ItemState, usize)]) -> usize {
    let mut state: HashMap<&str, ItemState> = HashMap::new();
    let mut peak = 0;
    for (n, s, _) in log {
        state.insert(n, *s);
        peak = peak.max(state.values().filter(|s| **s == ItemState::Cooking).count());
    }
    peak
}

/// Open `item`'s gate (the mock waits for `<run dir>/go`).
fn open_gate(shared: &Shared, item: &str) {
    for d in shared.dirs(item) {
        std::fs::write(d.join("go"), b"").unwrap();
    }
}

// ---- End to end ---------------------------------------------------------------------

#[test]
fn mock_cofold_streams_poses_through_the_local_executor() {
    let tmp = TempDir::new("e2e");
    let shared = Arc::new(Shared::default());
    let reg = registry(2, &shared);
    let ex = Executors::new(mock_profile(4, 0), workers(), &tmp.0);
    let g = graph(
        &reg,
        Some("mock"),
        &[("samples", 3i64.into()), ("gate", "go".into())],
    );
    let cache = Arc::new(MemoryCache::new());
    let engine = engine(&ex, &reg, cache.clone(), &g);

    // Each worker stops after its first pose until the observer has seen that pose
    // as a partial output of a still-running item: streaming, forced, not timed.
    let log: Timeline = Arc::default();
    let (l, s) = (log.clone(), shared.clone());
    let observer: Observer = Box::new(move |it: &WorkItem| {
        let n = it.partial.as_ref().map_or(0, |p| poses_of(p).len());
        let name = it.id.to_string();
        if name.starts_with("cofold") && n == 1 && it.state == ItemState::Cooking {
            open_gate(&s, &name);
        }
        l.lock().unwrap().push((name, it.state, n));
    });
    let items = engine
        .cook_with(
            &g,
            &[],
            CookOptions {
                observer: Some(observer),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        items.count(&id("cofold"), ItemState::Cooked),
        2,
        "{items:#?}"
    );

    let log = cofold_only(&log.lock().unwrap());
    for it in items.items(&id("cofold")) {
        let name = it.id.to_string();
        let mine: Vec<_> = log.iter().filter(|(n, ..)| *n == name).collect();
        let first = mine.iter().position(|(_, _, n)| *n > 0).expect("a partial");
        let cooked = mine
            .iter()
            .position(|(_, s, _)| *s == ItemState::Cooked)
            .unwrap();
        assert!(first < cooked, "{name}: {mine:?}");
        let counts: Vec<usize> = mine
            .iter()
            .filter(|(_, s, n)| *s == ItemState::Cooking && *n > 0)
            .map(|(_, _, n)| *n)
            .collect();
        assert_eq!(counts, [1, 2, 3], "{name}: poses stream one by one");

        // The result: three poses with files written by the worker, cached.
        let r = it.result.as_ref().unwrap();
        let poses = poses_of(r);
        assert_eq!(poses.len(), 3);
        let dir = &shared.dirs(&name)[0];
        assert_eq!(dir, &tmp.0.join("mock").join(it.key.to_hex()).join("0"));
        for p in &poses {
            let cif = std::fs::read_to_string(dir.join(&p.path)).unwrap();
            assert!(cif.contains("_atom_site"), "{} is mmCIF", p.path);
        }
        for f in [
            mold_jobs::SPEC_FILE,
            mold_jobs::EVENTS_FILE,
            mold_jobs::STDERR_FILE,
        ] {
            assert!(dir.join(f).is_file(), "{f} in the run dir");
        }
        assert!(cache.get(&it.key).is_some());
    }
    assert!(
        ex.take_notices()
            .iter()
            .all(|n| matches!(n, Notice::NativeItemsCookInApp { .. }))
    );
}

// ---- Parity -------------------------------------------------------------------------

fn keys(items: &ItemGraph) -> Vec<(String, ContentHash)> {
    items.all().map(|it| (it.id.to_string(), it.key)).collect()
}

fn results(items: &ItemGraph) -> Vec<Vec<Pose>> {
    items
        .items(&id("cofold"))
        .iter()
        .map(|it| poses_of(it.result.as_ref().expect("cooked")))
        .collect()
}

/// `(item, job.json bytes, events.ndjson bytes)` for every cofold run.
fn run_files(shared: &Shared) -> Vec<(String, Vec<u8>, Vec<u8>)> {
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

#[test]
fn in_process_and_local_cooks_agree_and_the_job_spec_is_byte_identical() {
    let params = [("samples", ParamValue::from(4i64)), ("seed", 11i64.into())];

    // In-process: cofold's Rust reference cook on the ThreadExecutor.
    let s0 = Arc::new(Shared::default());
    let reg = registry(3, &s0);
    let native = Engine::new(reg.clone(), Arc::new(MemoryCache::new()))
        .with_executor(Arc::new(ThreadExecutor::new(2)))
        .cook(&graph(&reg, None, &params), &[])
        .unwrap();
    assert_eq!(native.count(&id("cofold"), ItemState::Cooked), 3);
    assert_eq!(s0.prepares(), 0, "in-process never prepares");

    // The local executor, twice, in two run roots with fresh caches.
    let mut runs = Vec::new();
    for name in ["parity-a", "parity-b"] {
        let tmp = TempDir::new(name);
        let shared = Arc::new(Shared::default());
        let reg = registry(3, &shared);
        let g = graph(&reg, Some("mock"), &params);
        let ex = Executors::new(mock_profile(3, 0), workers(), &tmp.0);
        let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g)
            .cook(&g, &[])
            .unwrap();
        assert_eq!(items.count(&id("cofold"), ItemState::Cooked), 3);
        runs.push((items, run_files(&shared)));
    }

    for (local, _) in &runs {
        assert_eq!(keys(&native), keys(local), "identical cache keys");
        assert_eq!(results(&native), results(local), "identical outputs");
    }
    let (a, b) = (&runs[0].1, &runs[1].1);
    assert_eq!(a.len(), 3);
    assert_eq!(
        a, b,
        "JobSpec and event stream are byte-identical across runs"
    );
    let spec: serde_json::Value = serde_json::from_slice(&a[0].1).unwrap();
    assert_eq!(spec["run_dir"], ".", "no machine path in the JobSpec");
    assert_eq!(
        a[0].1,
        mold_jobs::protocol::canonical_json(&spec),
        "sent as canonical JSON: sorted keys, whatever serde_json features are on"
    );
}

// ---- Executor switch ----------------------------------------------------------------

#[test]
fn switching_the_executor_key_changes_no_cache_key_and_reruns_nothing() {
    let tmp = TempDir::new("switch");
    let shared = Arc::new(Shared::default());
    let reg = registry(2, &shared);
    let ex = Executors::new(
        profiles("[executor.mock]\nslots = 2\n[executor.other]\nslots = 1\n"),
        workers(),
        &tmp.0,
    );
    let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
    let g = graph(&reg, Some("mock"), &[]);
    let first = engine(&ex, &reg, cache.clone(), &g).cook(&g, &[]).unwrap();
    assert_eq!(first.count(&id("cofold"), ItemState::Cooked), 2);
    assert_eq!(shared.prepares(), 2);

    for key in [Some("other"), None] {
        let mut switched = g.clone();
        switched.nodes.get_mut(&id("cofold")).unwrap().executor = key.map(String::from);
        let e = engine(&ex, &reg, cache.clone(), &switched);
        assert_eq!(keys(&e.generate(&switched, &[]).unwrap()), keys(&first));
        let again = e.cook(&switched, &[]).unwrap();
        assert_eq!(again.count(&id("cofold"), ItemState::Cached), 2, "{key:?}");
    }
    assert_eq!(shared.prepares(), 2, "cached items never ran again");
}

// ---- Cancellation, timeout, retries, failure policy --------------------------------

/// `(worker pid, child pid)` from the `pids.json` the mock's `orphan` mode writes.
fn pids(dir: &Path) -> Option<(u32, u32)> {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("pids.json")).ok()?).ok()?;
    Some((v["worker"].as_u64()? as u32, v["child"].as_u64()? as u32))
}

#[test]
fn cancel_kills_the_process_tree_and_caches_nothing() {
    let tmp = TempDir::new("cancel");
    let shared = Arc::new(Shared::default());
    let reg = registry(2, &shared);
    let ex = Executors::new(mock_profile(2, 0), workers(), &tmp.0);
    // Each worker sends one pose, spawns a grandchild that sleeps forever, records
    // both PIDs, then hangs.
    let g = graph(&reg, Some("mock"), &[("fail", "orphan".into())]);
    let cache = Arc::new(MemoryCache::new());
    let engine = engine(&ex, &reg, cache.clone(), &g);

    let cancel = CancelToken::new();
    let (c, s) = (cancel.clone(), shared.clone());
    let canceller = std::thread::spawn(move || {
        let started = eventually(Duration::from_secs(30), || {
            let dirs = s.all_dirs();
            dirs.len() == 2 && dirs.iter().all(|d| pids(d).is_some())
        });
        c.cancel();
        started
    });
    let items = engine
        .cook_with(
            &g,
            &[],
            CookOptions {
                cancel,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        canceller.join().unwrap(),
        "both workers spawned their child"
    );
    assert!(items.cancelled);

    let all: Vec<u32> = shared
        .all_dirs()
        .iter()
        .flat_map(|d| {
            let (w, c) = pids(d).unwrap();
            [w, c]
        })
        .collect();
    assert_eq!(all.len(), 4);
    assert!(
        eventually(Duration::from_secs(10), || all
            .iter()
            .all(|&p| !pid_alive(p))),
        "worker and grandchild processes are gone: {:?}",
        all.iter().map(|&p| (p, pid_alive(p))).collect::<Vec<_>>()
    );
    for it in items.items(&id("cofold")) {
        assert_eq!(it.state, ItemState::Cancelled);
        assert!(cache.get(&it.key).is_none(), "{} was cached", it.id);
    }
    let ex = ex.executor("mock", vec![]).unwrap();
    assert!(ex.wait_idle(Duration::from_secs(10)));
}

#[test]
fn timeout_kills_the_worker_and_each_retry_gets_a_fresh_run_dir() {
    let tmp = TempDir::new("timeout");
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    let ex = Executors::new(mock_profile(2, 0), workers(), &tmp.0);
    let mut g = graph(&reg, Some("mock"), &[("fail", "orphan".into())]);
    let node = g.nodes.get_mut(&id("cofold")).unwrap();
    node.timeout = Some(1.0);
    node.retries = 1;
    let cache = Arc::new(MemoryCache::new());
    let items = engine(&ex, &reg, cache.clone(), &g).cook(&g, &[]).unwrap();

    let it = &items.items(&id("cofold"))[0];
    assert_eq!(it.state, ItemState::Failed);
    assert!(
        matches!(it.error, Some(NodeError::TimedOut(_))),
        "{:?}",
        it.error
    );
    assert!(cache.get(&it.key).is_none());
    let dirs = shared.dirs(&it.id.to_string());
    let key_dir = tmp.0.join("mock").join(it.key.to_hex());
    assert_eq!(
        dirs,
        [key_dir.join("0"), key_dir.join("1")],
        "attempt 0, then 1"
    );
    let ex = ex.executor("mock", vec![]).unwrap();
    assert!(ex.wait_idle(Duration::from_secs(10)));
    for d in &dirs {
        let (w, c) = pids(d).expect("each attempt ran");
        assert!(
            eventually(Duration::from_secs(10), || !pid_alive(w) && !pid_alive(c)),
            "attempt in {} left processes behind",
            d.display()
        );
    }
}

#[test]
fn a_failed_attempt_is_retried_and_succeeds() {
    let tmp = TempDir::new("retry");
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    let ex = Executors::new(mock_profile(1, 0), workers(), &tmp.0);
    let mut g = graph(
        &reg,
        Some("mock"),
        &[("fail", "exit".into()), ("fail_attempts", 1i64.into())],
    );
    g.nodes.get_mut(&id("cofold")).unwrap().retries = 2;
    let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g)
        .cook(&g, &[])
        .unwrap();
    let it = &items.items(&id("cofold"))[0];
    assert_eq!(it.state, ItemState::Cooked, "{:?}", it.error);
    assert_eq!(shared.dirs(&it.id.to_string()).len(), 2, "two attempts");
}

#[test]
fn on_failure_warn_lets_the_other_items_and_the_fan_in_finish() {
    let tmp = TempDir::new("warn");
    let shared = Arc::new(Shared::default());
    let reg = registry(3, &shared);
    let ex = Executors::new(mock_profile(3, 0), workers(), &tmp.0);
    let mut g = graph(
        &reg,
        Some("mock"),
        &[
            ("samples", 2i64.into()),
            ("fail", "crash".into()),
            ("fail_lig", 1i64.into()),
        ],
    );
    g.nodes.get_mut(&id("cofold")).unwrap().on_failure = OnFailure::Warn;
    // cofold -> all (wait for all) -> count
    let mut fan_in = g.clone();
    let mut all = NodeData::new("all_poses");
    all.inputs
        .insert("in".into(), OutputRef::new("cofold", "poses"));
    let mut count = NodeData::new("count");
    count
        .inputs
        .insert("poses".into(), OutputRef::new("all", "out"));
    fan_in.nodes.insert(id("all"), all);
    fan_in.nodes.insert(id("count"), count);
    fan_in.validate(&reg).unwrap();
    let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &fan_in)
        .cook(&fan_in, &[])
        .unwrap();

    let states: Vec<ItemState> = items.items(&id("cofold")).iter().map(|i| i.state).collect();
    assert_eq!(
        states,
        [ItemState::Cooked, ItemState::Failed, ItemState::Cooked]
    );
    let e = items.items(&id("cofold"))[1]
        .error
        .as_ref()
        .unwrap()
        .to_string();
    assert!(e.contains("before `finished`"), "{e}");
    let count = &items.items(&id("count"))[0];
    assert_eq!(
        count.state,
        ItemState::Cooked,
        "the fan-in went on without it"
    );
    let n = *count
        .result
        .as_ref()
        .unwrap()
        .get("n")
        .unwrap()
        .downcast_ref::<i64>()
        .unwrap();
    assert_eq!(n, 4, "two ligands x two poses");
}

// ---- Worker failures, as the runner reads them --------------------------------------

/// Cook one cofold item with `params`; return its state, its error message and the
/// PIDs its worker recorded (`pids.json`), if any.
fn cook_one(name: &str, params: &[(&str, ParamValue)]) -> (ItemState, String, Option<(u32, u32)>) {
    let tmp = TempDir::new(name);
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    let ex = Executors::new(mock_profile(1, 0), workers(), &tmp.0);
    let g = graph(&reg, Some("mock"), params);
    let cache = Arc::new(MemoryCache::new());
    let items = engine(&ex, &reg, cache.clone(), &g).cook(&g, &[]).unwrap();
    let it = &items.items(&id("cofold"))[0];
    if it.state != ItemState::Cooked {
        assert!(
            cache.get(&it.key).is_none(),
            "a failed item is never cached"
        );
    }
    let e = it
        .error
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    (it.state, e, pids(&shared.all_dirs()[0]))
}

#[test]
fn worker_failures_fail_the_item_with_a_clear_message() {
    let cases: [(&str, &[&str]); 4] = [
        (
            "exit",
            &[
                "worker `mock-cofold`",
                "exited with code 3",
                "before `finished`",
                "stderr",
                "injected exit 3",
            ],
        ),
        ("crash", &["before `finished`", "injected crash"]),
        (
            "error",
            &["finished with status `failed`", "injected failure"],
        ),
        (
            "malformed",
            &["stdout line", "not JSON", "this line is not JSON"],
        ),
    ];
    for (fail, wants) in cases {
        let (state, e, _) = cook_one(&format!("fail-{fail}"), &[("fail", fail.into())]);
        assert_eq!(state, ItemState::Failed, "{fail}");
        for w in wants {
            assert!(e.contains(w), "{fail}: {e:?} should mention {w:?}");
        }
    }
}

#[test]
fn a_child_left_holding_stdout_neither_hangs_the_run_nor_survives_it() {
    // The worker finishes normally but leaves a grandchild with its stdout open.
    let (state, e, pids) = cook_one(
        "linger",
        &[("fail", "linger".into()), ("fail_after", 0i64.into())],
    );
    assert_eq!(state, ItemState::Cooked, "{e}");
    let (w, c) = pids.expect("the worker recorded its child");
    assert!(
        eventually(Duration::from_secs(10), || !pid_alive(w) && !pid_alive(c)),
        "no orphan after a successful run"
    );
}

#[test]
fn an_invalid_job_spec_fails_before_launch() {
    struct Bad;
    impl Processor for Bad {
        fn spec(&self) -> NodeSpec {
            NodeSpec::new("bad").output("poses", PortType::PoseSet)
        }
        fn prepare(&self, ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
            let spec = JobSpec::new("bad").input("ligands", "../../etc/ligands.sdf");
            Ok(Some(CommandJob::new(
                "mock-cofold",
                ctx.run_dir,
                spec.to_value(),
            )))
        }
    }
    let tmp = TempDir::new("badspec");
    let mut r = Registry::new();
    r.processor(Bad).unwrap();
    let mut ed = Editor::new(Arc::new(r));
    ed.add_node_with_id("bad", "bad").unwrap();
    let mut g = ed.graph().clone();
    g.executor = Some("local".into());
    let ex = Executors::new(Profiles::builtin(), workers(), &tmp.0);
    let engine = Engine::new(ed.registry().clone(), Arc::new(MemoryCache::new()));
    let items = ex.attach(engine, &g).unwrap().cook(&g, &[]).unwrap();
    let e = items.items(&id("bad"))[0]
        .error
        .as_ref()
        .unwrap()
        .to_string();
    assert!(
        e.contains("invalid JobSpec") && e.contains("inside the run directory"),
        "{e}"
    );
}

// ---- Slots and GPUs -----------------------------------------------------------------

type Log = Vec<(String, ItemState, usize)>;

/// Cook `n` cofold items whose workers hold after their first pose until
/// `release(timeline)` says so; returns the items and the timeline.
fn cook_gated(
    name: &str,
    n: i64,
    profile: Profiles,
    params: &[(&str, ParamValue)],
    release: impl Fn(&Log) -> bool + 'static,
) -> (ItemGraph, Log) {
    let tmp = TempDir::new(name);
    let shared = Arc::new(Shared::default());
    let reg = registry(n, &shared);
    let ex = Executors::new(profile, workers(), &tmp.0);
    let mut all = vec![("gate", ParamValue::from("go")), ("samples", 2i64.into())];
    all.extend(params.iter().cloned());
    let g = graph(&reg, Some("mock"), &all);
    let engine = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g);

    let log: Timeline = Arc::default();
    let (l, s) = (log.clone(), shared.clone());
    let mut released = false;
    let observer: Observer = Box::new(move |it: &WorkItem| {
        let name = it.id.to_string();
        if !name.starts_with("cofold") {
            return;
        }
        let n = it.partial.as_ref().map_or(0, |p| poses_of(p).len());
        let mut l = l.lock().unwrap();
        l.push((name, it.state, n));
        released = released || release(&l);
        if released {
            // Open every gate a worker may be waiting at (later ones as they come).
            for (item, _, n) in l.iter() {
                if *n > 0 {
                    open_gate(&s, item);
                }
            }
        }
    });
    let items = engine
        .cook_with(
            &g,
            &[],
            CookOptions {
                observer: Some(observer),
                ..Default::default()
            },
        )
        .unwrap();
    let log = log.lock().unwrap().clone();
    (items, log)
}

/// The latest state of each cofold item in `log`.
fn current(log: &Log) -> BTreeMap<String, ItemState> {
    log.iter().map(|(n, s, _)| (n.clone(), *s)).collect()
}

fn held(log: &Log) -> usize {
    log.iter()
        .filter(|(_, s, n)| *s == ItemState::Cooking && *n == 1)
        .count()
}

#[test]
fn with_two_slots_a_third_item_waits() {
    // Release the workers only once two are cooking (each held at its first pose)
    // and the third has been submitted: it must be waiting, not running.
    let (items, log) = cook_gated("slots", 3, mock_profile(2, 0), &[], |log| {
        let now = current(log);
        let count = |state| now.values().filter(|s| **s == state).count();
        count(ItemState::Cooking) == 2 && count(ItemState::Scheduled) == 1 && held(log) >= 2
    });
    assert_eq!(items.count(&id("cofold"), ItemState::Cooked), 3, "{log:?}");
    assert_eq!(peak_cooking(&log), 2, "{log:?}");
}

#[test]
fn a_gpu_item_waits_for_the_free_gpu() {
    // 4 slots but 1 GPU, two items needing a GPU each: the second waits.
    let (items, log) = cook_gated(
        "gpus",
        2,
        mock_profile(4, 1),
        &[("gpus", 1i64.into())],
        |log| {
            let now = current(log);
            let waiting = now.values().filter(|s| **s == ItemState::Scheduled).count();
            held(log) >= 1 && waiting == 1
        },
    );
    assert_eq!(items.count(&id("cofold"), ItemState::Cooked), 2, "{log:?}");
    assert_eq!(peak_cooking(&log), 1, "{log:?}");
}

#[test]
fn an_item_needing_more_than_the_profile_has_fails_at_once() {
    // The built-in `local` declares no GPU: a GPU item fails, it does not wait forever.
    let tmp = TempDir::new("nogpu");
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    let ex = Executors::new(Profiles::builtin(), workers(), &tmp.0);
    let g = graph(&reg, Some("local"), &[("gpus", 1i64.into())]);
    let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g)
        .cook(&g, &[])
        .unwrap();
    let it = &items.items(&id("cofold"))[0];
    assert_eq!(it.state, ItemState::Failed);
    let e = it.error.as_ref().unwrap().to_string();
    assert!(
        e.contains("1 GPU(s), and it has 0") && e.contains("executors.toml"),
        "{e}"
    );
    assert_eq!(shared.prepares(), 0, "nothing ran");
}

// ---- Fail before running ------------------------------------------------------------

#[test]
fn an_unknown_executor_key_fails_before_any_item_runs() {
    let tmp = TempDir::new("unknown");
    let shared = Arc::new(Shared::default());
    let reg = registry(2, &shared);
    let ex = Executors::new(mock_profile(1, 0), workers(), &tmp.0);
    let g = graph(&reg, Some("gpu-box"), &[]);
    let err = ex
        .attach(Engine::new(reg.clone(), Arc::new(MemoryCache::new())), &g)
        .unwrap_err();
    assert!(matches!(err, ConfigError::UnknownExecutor { .. }));
    let msg = err.to_string();
    for want in [
        "unknown executor `gpu-box`",
        "node `cofold`",
        "`mock`",
        "`local` (built in)",
    ] {
        assert!(msg.contains(want), "{msg:?} should mention {want:?}");
    }
    assert_eq!(shared.prepares(), 0);
    assert_eq!(std::fs::read_dir(&tmp.0).unwrap().count(), 0, "no run dir");
}

#[test]
fn an_ssh_profile_is_refused_before_cooking_for_now() {
    let tmp = TempDir::new("ssh");
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    let ex = Executors::new(
        profiles("[executor.box]\ntype = \"ssh\"\nhost = \"gpu-box\"\n"),
        workers(),
        &tmp.0,
    );
    let g = graph(&reg, Some("box"), &[]);
    assert_eq!(ex.remote_keys(&g), ["box"], "the UI can announce it");
    let err = ex.check(&g).unwrap_err().to_string();
    assert!(err.contains("SSH executors are not available yet"), "{err}");
    assert_eq!(shared.prepares(), 0);
}

#[test]
fn an_invalid_manifest_fails_before_any_item_runs() {
    let tmp = TempDir::new("manifest");
    let dir = tmp.0.join("workers").join("mock-cofold");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.toml"),
        "name = \"mock-cofold\"\nexecutable = \"mock-cofold\"\n\
         environment = \"docker:x\"\nneeds_gpu = false\nmodel = \"m\"\n",
    )
    .unwrap();
    // Workers load before any executor exists, so no item can run.
    let err = Workers::load_dir(&tmp.0.join("workers"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("manifest.toml") && err.contains("environment"),
        "{err}"
    );
}

// ---- Rust-native items on the local executor ----------------------------------------

#[test]
fn rust_native_items_routed_to_the_local_executor_cook_in_place_with_one_notice() {
    let tmp = TempDir::new("native");
    let shared = Arc::new(Shared::default());
    let reg = registry(3, &shared);
    let mut g = graph(&reg, Some("mock"), &[("samples", 1i64.into())]);
    g.nodes.get_mut(&id("ligands")).unwrap().executor = Some("mock".into());
    let ex = Executors::new(mock_profile(2, 0), workers(), &tmp.0);
    let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g)
        .cook(&g, &[])
        .unwrap();
    assert_eq!(items.count(&id("ligands"), ItemState::Cooked), 3);
    assert_eq!(items.count(&id("cofold"), ItemState::Cooked), 3);
    assert_eq!(
        ex.take_notices(),
        [Notice::NativeItemsCookInApp {
            kind: "ligands".into(),
            executor: "mock".into()
        }],
        "reported once for three items"
    );
    assert!(ex.take_notices().is_empty());
}
