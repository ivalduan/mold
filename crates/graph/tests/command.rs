//! The command path (ADR-0009 §1): prepare → run → collect through the executor seam.
//!
//! A test-only "fake command executor" stands in for `mold-jobs`: it prepares each
//! item in a run directory, then replays a scripted worker event stream through
//! collect from its own thread. graph never interprets those events; the test's
//! processor does.
//!
//! ```text
//! ligands (3 items, Scalar) -> dock (PoseSet: `samples` poses per ligand)
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use mold_graph::*;
use serde_json::json;

// ---- Fixtures -----------------------------------------------------------------------

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "mold-graph-cmd-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A one-shot signal with a timeout, so a scheduling bug fails instead of hanging.
#[derive(Clone, Default)]
struct Signal(Arc<(Mutex<bool>, Condvar)>);

impl Signal {
    fn set(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
    fn wait(&self, timeout: Duration) -> bool {
        let g = self.0.0.lock().unwrap();
        let (g, _) = self
            .0
            .1
            .wait_timeout_while(g, timeout, |set| !*set)
            .unwrap();
        *g
    }
}

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

/// Three ligands, each a Scalar `i64` equal to its index.
struct Ligands;

impl Processor for Ligands {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("ligands").output("lig", PortType::Scalar)
    }
    fn generate(&self, _ctx: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
        Ok((0..3)
            .map(|i| NewItem::work().attr("lig", i as i64))
            .collect())
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let Some(AttrValue::Int(i)) = ctx.attrs.get("lig") else {
            return Err(NodeError::msg("no lig attr"));
        };
        Ok(Outputs::new().with("lig", Value::new(PortType::Scalar, *i)))
    }
}

fn dock_spec(kind: &str) -> NodeSpec {
    NodeSpec::new(kind)
        .input("lig", PortType::Scalar)
        .output("poses", PortType::PoseSet)
        .param(ParamSpec::new("samples", 3i64))
}

/// The reference result: `samples` pose scores `10 * lig + k`, best = the largest.
fn reference(lig: i64, samples: i64) -> (Vec<i64>, i64) {
    let poses: Vec<i64> = (0..samples).map(|k| 10 * lig + k).collect();
    let best = *poses.iter().max().unwrap();
    (poses, best)
}

fn poses_of(o: &Outputs) -> Vec<i64> {
    o.get("poses")
        .and_then(|v| v.downcast_ref::<Vec<i64>>())
        .cloned()
        .unwrap_or_default()
}

/// The worker node's prepare: write the ligand to `input.txt`, ask for `mock-dock`.
fn dock_prepare(ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
    let lig: i64 = *ctx.inputs.one_as("lig")?;
    let samples = ctx.params.int("samples")?;
    std::fs::write(ctx.run_dir.join("input.txt"), lig.to_string())
        .map_err(|e| NodeError::msg(e.to_string()))?;
    let spec = json!({ "ligand": "input.txt", "samples": samples });
    Ok(Some(
        CommandJob::new("mock-dock", ctx.run_dir, spec).stage("input.txt"),
    ))
}

/// Fold a `pose` event into the partial `poses`.
fn dock_collect_event(event: &serde_json::Value, partial: &mut Outputs) -> bool {
    if event["type"] != "pose" {
        return false;
    }
    let mut poses = poses_of(partial);
    poses.push(event["score"].as_i64().unwrap());
    partial
        .values
        .insert("poses".into(), Value::new(PortType::PoseSet, poses));
    true
}

/// The finish: poses as streamed, `best` read from the artifact the worker wrote.
fn dock_collect_finish(
    ctx: &CommandContext<'_>,
    finished: &serde_json::Value,
    partial: Outputs,
) -> Result<Outputs, NodeError> {
    let file = finished["best_file"]
        .as_str()
        .ok_or_else(|| NodeError::msg("no best_file"))?;
    let best: i64 = std::fs::read_to_string(ctx.run_dir.join(file))
        .map_err(|e| NodeError::msg(e.to_string()))?
        .trim()
        .parse()
        .map_err(|_| NodeError::msg("bad best"))?;
    Ok(Outputs::new()
        .with("poses", Value::new(PortType::PoseSet, poses_of(&partial)))
        .attr("best", best))
}

/// A worker node with a Rust reference cook too (the parity reference).
struct Dock;

impl Processor for Dock {
    fn spec(&self) -> NodeSpec {
        dock_spec("dock")
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let lig: i64 = *ctx.input("lig")?;
        let (poses, best) = reference(lig, ctx.params.int("samples")?);
        Ok(Outputs::new()
            .with("poses", Value::new(PortType::PoseSet, poses))
            .attr("best", best))
    }
    fn prepare(&self, ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
        dock_prepare(ctx)
    }
    fn collect_event(
        &self,
        _ctx: &CommandContext<'_>,
        event: &serde_json::Value,
        partial: &mut Outputs,
    ) -> Result<bool, NodeError> {
        Ok(dock_collect_event(event, partial))
    }
    fn collect_finish(
        &self,
        ctx: &CommandContext<'_>,
        finished: &serde_json::Value,
        partial: Outputs,
    ) -> Result<Outputs, NodeError> {
        dock_collect_finish(ctx, finished, partial)
    }
}

/// A pure worker node: prepare and collect, no Rust cook.
struct DockCmd;

impl Processor for DockCmd {
    fn spec(&self) -> NodeSpec {
        dock_spec("dock_cmd")
    }
    fn prepare(&self, ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
        dock_prepare(ctx)
    }
    fn collect_event(
        &self,
        _ctx: &CommandContext<'_>,
        event: &serde_json::Value,
        partial: &mut Outputs,
    ) -> Result<bool, NodeError> {
        Ok(dock_collect_event(event, partial))
    }
    fn collect_finish(
        &self,
        ctx: &CommandContext<'_>,
        finished: &serde_json::Value,
        partial: Outputs,
    ) -> Result<Outputs, NodeError> {
        dock_collect_finish(ctx, finished, partial)
    }
}

fn registry() -> Arc<Registry> {
    let mut r = Registry::new();
    r.processor(Ligands)
        .unwrap()
        .processor(Dock)
        .unwrap()
        .processor(DockCmd)
        .unwrap();
    Arc::new(r)
}

fn graph(dock_kind: &str) -> Graph {
    let mut ed = Editor::new(registry());
    ed.add_node_with_id("ligands", "ligands").unwrap();
    ed.add_node_with_id("dock", dock_kind).unwrap();
    ed.connect("ligands.lig", &id("dock"), "lig").unwrap();
    ed.graph().clone()
}

// ---- The fake command executor -------------------------------------------------------

/// Hooks into the fake worker's event stream, for the streaming and cancel tests.
#[derive(Clone, Default)]
struct Hooks {
    /// Set after the first event of every job has been sent.
    first_sent: Signal,
    /// If set, the worker waits for this after its first event (up to 5 s) and records
    /// whether it came.
    resume: Option<Signal>,
    /// Whether `resume` came before the worker finished.
    resumed: Arc<AtomicBool>,
    /// If set, the worker ignores cancellation and finishes anyway (a stubborn worker).
    stubborn: bool,
}

/// Stands in for `mold-jobs`: prepare in a run directory, run a scripted worker on its
/// own thread, collect. The "worker" reads the staged input and replays `pose` events.
#[derive(Default)]
struct FakeCommandExecutor {
    root: PathBuf,
    hooks: Hooks,
    commands: Arc<Mutex<Vec<CommandJob>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for FakeCommandExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeCommandExecutor")
    }
}

impl FakeCommandExecutor {
    fn new(root: &Path, hooks: Hooks) -> Arc<Self> {
        Arc::new(Self {
            root: root.to_owned(),
            hooks,
            ..Default::default()
        })
    }

    fn join(&self) {
        for t in self.threads.lock().unwrap().drain(..) {
            t.join().unwrap();
        }
    }
}

/// The scripted worker: what `mock-dock` would print, one event per pose.
fn fake_worker(
    cmd: &CommandJob,
    events: &mut EventSink<'_>,
    hooks: &Hooks,
) -> Result<serde_json::Value, NodeError> {
    let lig: i64 = std::fs::read_to_string(cmd.run_dir.join(cmd.spec["ligand"].as_str().unwrap()))
        .unwrap()
        .parse()
        .unwrap();
    let samples = cmd.spec["samples"].as_i64().unwrap();
    let (poses, best) = reference(lig, samples);
    events.event(&json!({ "type": "progress", "fraction": 0.0 }))?;
    for (k, score) in poses.iter().enumerate() {
        let sent = events.event(&json!({ "type": "pose", "score": score }));
        if !hooks.stubborn {
            sent?;
        }
        if k == 0 {
            hooks.first_sent.set();
            if let Some(resume) = &hooks.resume {
                hooks
                    .resumed
                    .store(resume.wait(Duration::from_secs(5)), Ordering::SeqCst);
            }
            if hooks.stubborn {
                // Wait for the cancel, then carry on as if nothing happened.
                let t = std::time::Instant::now();
                while !events.cancel().is_cancelled() && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
    std::fs::write(cmd.run_dir.join("best.txt"), best.to_string()).unwrap();
    Ok(json!({ "type": "finished", "best_file": "best.txt" }))
}

impl Executor for FakeCommandExecutor {
    fn submit(&self, job: Job, handle: JobHandle) {
        // A fresh run directory per job (mold-jobs: per key and attempt).
        let run_dir = self.root.join(job.key.to_hex()).join(job.id.0.to_string());
        let hooks = self.hooks.clone();
        let commands = self.commands.clone();
        let t = std::thread::spawn(move || {
            handle.started();
            let r = job.cook_command(&run_dir, &handle, |cmd, events| {
                commands.lock().unwrap().push(cmd.clone());
                fake_worker(cmd, events, &hooks)
            });
            handle.finish(r);
        });
        self.threads.lock().unwrap().push(t);
    }
}

// ---- Tests --------------------------------------------------------------------------

fn keys(items: &ItemGraph) -> Vec<(String, ContentHash)> {
    items.all().map(|it| (it.id.to_string(), it.key)).collect()
}

fn dock_results(items: &ItemGraph) -> Vec<(Vec<i64>, Attributes)> {
    items
        .items(&id("dock"))
        .iter()
        .map(|it| {
            let r = it.result.as_ref().expect("dock item cooked");
            assert!(r.files.is_empty());
            (poses_of(r), r.attrs.clone())
        })
        .collect()
}

#[test]
fn command_items_cook_like_their_rust_equivalent_with_identical_keys() {
    let tmp = TempDir::new("parity");
    let reg = registry();
    let g = graph("dock");

    // Rust-native, in-process.
    let mem_cache = Arc::new(MemoryCache::new());
    let native = Engine::new(reg.clone(), mem_cache.clone())
        .with_executor(Arc::new(ThreadExecutor::new(2)))
        .cook(&g, &[])
        .unwrap();
    assert_eq!(native.count(&id("dock"), ItemState::Cooked), 3);

    // The same graph, `dock` routed to the command executor by its node key.
    let fake = FakeCommandExecutor::new(&tmp.0, Hooks::default());
    let cmd_cache = Arc::new(MemoryCache::new());
    let mut routed = g.clone();
    routed.nodes.get_mut(&id("dock")).unwrap().executor = Some("cmd".into());
    let engine = Engine::new(reg.clone(), cmd_cache.clone())
        .with_executor(Arc::new(ThreadExecutor::new(2)))
        .add_executor("cmd", fake.clone());
    let command = engine.cook(&routed, &[]).unwrap();
    fake.join();
    assert_eq!(command.count(&id("dock"), ItemState::Cooked), 3);

    // Identical keys whichever executor cooked, and identical outputs.
    assert_eq!(keys(&native), keys(&command));
    assert_eq!(dock_results(&native), dock_results(&command));
    for (lig, (poses, attrs)) in dock_results(&command).into_iter().enumerate() {
        let (want, best) = reference(lig as i64, 3);
        assert_eq!(poses, want);
        assert_eq!(attrs.get("best"), Some(&AttrValue::Int(best)));
    }
    // Identical cache entries.
    for it in command.items(&id("dock")) {
        let a = mem_cache.get(&it.key).expect("native entry");
        let b = cmd_cache.get(&it.key).expect("command entry");
        assert_eq!(poses_of(&a), poses_of(&b));
        assert_eq!(a.attrs, b.attrs);
        assert_eq!(a.files, b.files);
    }

    // Prepare wrote the input into the run dir and described the job.
    let cmds = fake.commands.lock().unwrap().clone();
    assert_eq!(cmds.len(), 3, "only `dock` items are commands");
    for c in &cmds {
        assert_eq!(c.worker, "mock-dock");
        assert_eq!(c.stage, vec![PathBuf::from("input.txt")]);
        assert_eq!(c.spec["samples"], 3);
        assert!(c.run_dir.join("input.txt").is_file());
        assert!(c.run_dir.starts_with(&tmp.0));
    }

    // Changing the executor (node key, graph key, engine default) changes no key.
    let mut by_graph = g.clone();
    by_graph.executor = Some("cmd".into());
    let fresh = |g: &Graph, default: Arc<dyn Executor>| {
        Engine::new(reg.clone(), Arc::new(MemoryCache::new()))
            .with_executor(default)
            .add_executor("cmd", Arc::new(ThreadExecutor::new(1)))
            .generate(g, &[])
            .unwrap()
    };
    let base = keys(&fresh(&g, Arc::new(ThreadExecutor::new(1))));
    assert_eq!(base, keys(&native));
    assert_eq!(
        base,
        keys(&fresh(&routed, Arc::new(ThreadExecutor::new(1))))
    );
    assert_eq!(
        base,
        keys(&fresh(&by_graph, Arc::new(ThreadExecutor::new(1))))
    );
    assert_eq!(base, keys(&fresh(&g, fake.clone())));

    // And the command engine finds the native cook's results in a shared cache.
    let again = Engine::new(reg, mem_cache)
        .add_executor("cmd", fake.clone())
        .cook(&routed, &[])
        .unwrap();
    assert_eq!(again.count(&id("dock"), ItemState::Cached), 3);
    assert_eq!(
        fake.commands.lock().unwrap().len(),
        3,
        "cached: nothing ran"
    );
}

#[test]
fn partial_outputs_arrive_before_the_item_finishes() {
    let tmp = TempDir::new("partial");
    let resume = Signal::default();
    let hooks = Hooks {
        resume: Some(resume.clone()),
        ..Default::default()
    };
    let fake = FakeCommandExecutor::new(&tmp.0, hooks.clone());
    let engine =
        Engine::new(registry(), Arc::new(MemoryCache::new())).add_executor("cmd", fake.clone());
    let mut g = graph("dock_cmd");
    g.nodes.get_mut(&id("dock")).unwrap().executor = Some("cmd".into());

    // Observer log: (item, state, number of partial poses).
    type Seen = Vec<(String, ItemState, usize)>;
    let log: Arc<Mutex<Seen>> = Arc::default();
    let l = log.clone();
    let observer: Observer = Box::new(move |it: &WorkItem| {
        if it.id.node.as_str() != "dock" {
            return;
        }
        let n = it.partial.as_ref().map_or(0, |p| poses_of(p).len());
        l.lock().unwrap().push((it.id.to_string(), it.state, n));
        // The worker waits after its first pose until the observer has seen it.
        if n > 0 && !it.state.is_terminal() {
            resume.set();
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
    fake.join();
    assert!(
        hooks.resumed.load(Ordering::SeqCst),
        "a partial reached the observer while the worker was still running"
    );
    assert_eq!(items.count(&id("dock"), ItemState::Cooked), 3);

    let log = log.lock().unwrap();
    for it in items.items(&id("dock")) {
        let name = it.id.to_string();
        let mine: Vec<_> = log.iter().filter(|(n, ..)| *n == name).collect();
        let first_partial = mine.iter().position(|(_, _, n)| *n > 0).expect("a partial");
        let cooked = mine
            .iter()
            .position(|(_, s, _)| *s == ItemState::Cooked)
            .expect("cooked");
        assert!(first_partial < cooked, "{name}: {mine:?}");
        // Poses stream one by one: 1, 2, 3 (the progress event changes nothing).
        let counts: Vec<usize> = mine
            .iter()
            .filter(|(_, s, n)| *s == ItemState::Cooking && *n > 0)
            .map(|(_, _, n)| *n)
            .collect();
        assert_eq!(counts, vec![1, 2, 3], "{name}");
    }
}

#[test]
fn in_process_executor_refuses_command_items_clearly() {
    let engine = Engine::new(registry(), Arc::new(MemoryCache::new()))
        .with_executor(Arc::new(ThreadExecutor::new(2)));
    let items = engine.cook(&graph("dock_cmd"), &[]).unwrap();
    assert_eq!(items.count(&id("ligands"), ItemState::Cooked), 3);
    assert_eq!(items.count(&id("dock"), ItemState::Failed), 3);
    for it in items.items(&id("dock")) {
        let e = it.error.as_ref().unwrap();
        assert_eq!(e, &NodeError::NoCommandExecutor);
        assert!(
            e.to_string().contains("no executor for command jobs"),
            "{e}"
        );
        assert!(engine.cache().get(&it.key).is_none());
    }
}

#[test]
fn a_command_executor_cooks_rust_native_items_in_place() {
    // `ligands` is Rust-native: on the command executor, prepare returns None and the
    // item is cooked with `cook_here`; no worker runs for it.
    let tmp = TempDir::new("native");
    let fake = FakeCommandExecutor::new(&tmp.0, Hooks::default());
    let engine = Engine::new(registry(), Arc::new(MemoryCache::new())).with_executor(fake.clone());
    let items = engine.cook(&graph("dock_cmd"), &[]).unwrap();
    fake.join();
    assert_eq!(items.count(&id("ligands"), ItemState::Cooked), 3);
    assert_eq!(items.count(&id("dock"), ItemState::Cooked), 3);
    assert_eq!(fake.commands.lock().unwrap().len(), 3);
}

#[test]
fn cancel_during_a_command_run_writes_no_cache_entry() {
    let tmp = TempDir::new("cancel");
    let hooks = Hooks {
        stubborn: true,
        ..Default::default()
    };
    let fake = FakeCommandExecutor::new(&tmp.0, hooks.clone());
    let cache = Arc::new(MemoryCache::new());
    let engine = Engine::new(registry(), cache.clone()).add_executor("cmd", fake.clone());
    let mut g = graph("dock_cmd");
    g.nodes.get_mut(&id("dock")).unwrap().executor = Some("cmd".into());

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let first = hooks.first_sent.clone();
    let canceller = std::thread::spawn(move || {
        assert!(first.wait(Duration::from_secs(10)));
        c.cancel();
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
    canceller.join().unwrap();
    // The stubborn workers finish anyway; their results must still not be cached.
    fake.join();
    assert!(items.cancelled);
    for it in items.items(&id("dock")) {
        assert_eq!(it.state, ItemState::Cancelled, "{}", it.id);
        assert!(cache.get(&it.key).is_none(), "{} was cached", it.id);
    }
    assert_eq!(
        cache.len(),
        items.count(&id("ligands"), ItemState::Cooked),
        "only the ligands were cached"
    );
}

#[test]
fn a_refused_event_fails_the_job_even_if_the_runner_carries_on() {
    // Direct use of the seam, as mold-jobs would.
    struct Picky;
    impl Processor for Picky {
        fn spec(&self) -> NodeSpec {
            NodeSpec::new("picky").output("poses", PortType::PoseSet)
        }
        fn prepare(&self, ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
            Ok(Some(CommandJob::new("w", ctx.run_dir, json!({}))))
        }
        fn collect_event(
            &self,
            _: &CommandContext<'_>,
            e: &serde_json::Value,
            _: &mut Outputs,
        ) -> Result<bool, NodeError> {
            match e["type"].as_str() {
                Some("bad") => Err(NodeError::msg("bad event")),
                _ => Ok(false),
            }
        }
    }
    type Sent = Mutex<Vec<Result<(), NodeError>>>;
    #[derive(Debug)]
    struct Direct(PathBuf, Sent);
    impl Executor for Direct {
        fn submit(&self, job: Job, handle: JobHandle) {
            let run_dir = self.0.join(job.id.0.to_string());
            let r = job.cook_command(&run_dir, &handle, |_, events| {
                let sent = events.event(&json!({"type": "bad"}));
                self.1.lock().unwrap().push(sent);
                // A sloppy runner that carries on regardless.
                Ok(json!({"type": "finished"}))
            });
            handle.finish(r);
        }
    }
    let tmp = TempDir::new("picky");
    let mut r = Registry::new();
    r.processor(Picky).unwrap();
    let mut ed = Editor::new(Arc::new(r));
    ed.add_node_with_id("p", "picky").unwrap();
    let direct = Arc::new(Direct(tmp.0.clone(), Mutex::default()));
    let engine = Engine::new(ed.registry().clone(), Arc::new(MemoryCache::new()))
        .with_executor(direct.clone());
    let items = engine.cook(ed.graph(), &[]).unwrap();
    let it = &items.items(&id("p"))[0];
    assert_eq!(it.state, ItemState::Failed);
    assert_eq!(it.error, Some(NodeError::msg("bad event")));
    assert_eq!(
        direct.1.lock().unwrap().as_slice(),
        &[Err(NodeError::msg("bad event"))]
    );
}
