//! Per-node failure policy (ADR-0009 §7) and per-item needs (§5).
//!
//! ```text
//! src (3 items, Scalar = index) -> flaky (1:1) -> plus1 (1:1) -> all (wait for all) -> sum
//! fast (1 item, independent)
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mold_graph::*;

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

fn scalar(o: &Outputs, port: &str) -> i64 {
    *o.get(port).unwrap().downcast_ref::<i64>().unwrap()
}

struct Src;

impl Processor for Src {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("src").output("x", PortType::Scalar)
    }
    fn generate(&self, _: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
        Ok((0..3)
            .map(|i| NewItem::work().attr("i", i as i64))
            .collect())
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let Some(AttrValue::Int(i)) = ctx.attrs.get("i") else {
            unreachable!()
        };
        Ok(Outputs::new().with("x", Value::new(PortType::Scalar, *i)))
    }
}

/// Attempts per input value, shared with the test.
type Attempts = Arc<Mutex<HashMap<i64, u32>>>;

/// Passes its input through. The item whose input is `bad` fails its first `fails`
/// attempts (`u32::MAX`: always); with `hang`, a failing attempt instead hangs until
/// cancelled. Declares one GPU per item.
struct Flaky {
    bad: i64,
    fails: u32,
    hang: bool,
    attempts: Attempts,
}

impl Processor for Flaky {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("flaky")
            .input("x", PortType::Scalar)
            .output("x", PortType::Scalar)
    }
    fn needs(&self, _: &Params, _: &Attributes) -> Needs {
        Needs {
            gpus: 1,
            ..Needs::default()
        }
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let x: i64 = *ctx.input("x")?;
        let attempt = {
            let mut a = self.attempts.lock().unwrap();
            let n = a.entry(x).or_insert(0);
            *n += 1;
            *n
        };
        if x == self.bad && attempt <= self.fails {
            if self.hang {
                let t = Instant::now();
                while !ctx.cancel.is_cancelled() {
                    assert!(t.elapsed() < Duration::from_secs(10), "never cancelled");
                    std::thread::sleep(Duration::from_millis(2));
                }
                return Err(NodeError::Cancelled);
            }
            return Err(NodeError::msg(format!("flaky on {x}, attempt {attempt}")));
        }
        Ok(Outputs::new().with("x", Value::new(PortType::Scalar, x)))
    }
}

struct Plus1;

impl Processor for Plus1 {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("plus1")
            .input("x", PortType::Scalar)
            .output("x", PortType::Scalar)
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let x: i64 = *ctx.input("x")?;
        Ok(Outputs::new().with("x", Value::new(PortType::Scalar, x + 1)))
    }
}

struct Sum;

impl Processor for Sum {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("sum")
            .input("x", PortType::Scalar)
            .output("sum", PortType::Scalar)
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let xs: Vec<&i64> = ctx.inputs.all_as("x")?;
        let s: i64 = xs.into_iter().sum();
        Ok(Outputs::new().with("sum", Value::new(PortType::Scalar, s)))
    }
}

struct Fast;

impl Processor for Fast {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("fast").output("x", PortType::Scalar)
    }
    fn cook(&self, _: &CookContext<'_>) -> Result<Outputs, NodeError> {
        Ok(Outputs::new().with("x", Value::new(PortType::Scalar, 7i64)))
    }
}

struct Fixture {
    graph: Graph,
    registry: Arc<Registry>,
    attempts: Attempts,
}

fn fixture(bad: i64, fails: u32, hang: bool) -> Fixture {
    let attempts = Attempts::default();
    let mut r = Registry::new();
    r.processor(Src)
        .unwrap()
        .processor(Flaky {
            bad,
            fails,
            hang,
            attempts: attempts.clone(),
        })
        .unwrap()
        .processor(Plus1)
        .unwrap()
        .processor(Sum)
        .unwrap()
        .processor(Fast)
        .unwrap()
        .partitioner(WaitForAll::new("all", PortType::Scalar))
        .unwrap();
    let registry = Arc::new(r);
    let mut ed = Editor::new(registry.clone());
    for (n, k) in [
        ("src", "src"),
        ("flaky", "flaky"),
        ("plus1", "plus1"),
        ("all", "all"),
        ("sum", "sum"),
        ("fast", "fast"),
    ] {
        ed.add_node_with_id(n, k).unwrap();
    }
    ed.connect("src.x", &id("flaky"), "x").unwrap();
    ed.connect("flaky.x", &id("plus1"), "x").unwrap();
    ed.connect("plus1.x", &id("all"), "in").unwrap();
    ed.connect("all.out", &id("sum"), "x").unwrap();
    Fixture {
        graph: ed.graph().clone(),
        registry,
        attempts,
    }
}

impl Fixture {
    fn set(&mut self, f: impl FnOnce(&mut NodeData)) -> &mut Self {
        f(self.graph.nodes.get_mut(&id("flaky")).unwrap());
        self
    }

    fn engine(&self, cache: Arc<MemoryCache>) -> Engine {
        Engine::new(self.registry.clone(), cache).with_executor(Arc::new(ThreadExecutor::new(4)))
    }

    fn cook(&self) -> (ItemGraph, Arc<MemoryCache>) {
        let cache = Arc::new(MemoryCache::new());
        let items = self.engine(cache.clone()).cook(&self.graph, &[]).unwrap();
        (items, cache)
    }

    fn attempts(&self, x: i64) -> u32 {
        self.attempts.lock().unwrap().get(&x).copied().unwrap_or(0)
    }
}

fn sum_of(items: &ItemGraph) -> Option<i64> {
    let it = items.items(&id("sum")).into_iter().next()?;
    it.result.as_ref().map(|r| scalar(r, "sum"))
}

// ---- Serde: old graphs load, defaults are not written ---------------------------------

#[test]
fn old_graphs_load_with_the_default_policy() {
    // A graph as written before the policy fields existed.
    let old = r#"{
        "executor": "farm",
        "nodes": {
            "a": { "kind": "src" },
            "b": { "kind": "flaky", "executor": "local",
                   "inputs": { "x": { "node": "a", "port": "x" } } }
        }
    }"#;
    let g: Graph = serde_json::from_str(old).unwrap();
    for n in g.nodes.values() {
        assert_eq!(n.retries, 0);
        assert_eq!(n.timeout, None);
        assert_eq!(n.on_failure, OnFailure::Error);
        assert_eq!(n.failure_policy(), FailurePolicy::default());
    }
    assert_eq!(g.nodes[&id("b")].executor.as_deref(), Some("local"));

    // Defaults are not serialized, so an untouched graph's text is unchanged.
    let text = serde_json::to_string(&g).unwrap();
    for field in ["retries", "timeout", "on_failure"] {
        assert!(!text.contains(field), "{text}");
    }
    assert_eq!(serde_json::from_str::<Graph>(&text).unwrap(), g);

    // Set fields round-trip, in lowercase.
    let mut set = g.clone();
    let b = set.nodes.get_mut(&id("b")).unwrap();
    b.retries = 2;
    b.timeout = Some(1.5);
    b.on_failure = OnFailure::Warn;
    let text = serde_json::to_string(&set).unwrap();
    assert!(text.contains(r#""on_failure":"warn""#), "{text}");
    let back: Graph = serde_json::from_str(&text).unwrap();
    assert_eq!(back, set);
    assert_eq!(
        back.nodes[&id("b")].failure_policy(),
        FailurePolicy {
            retries: 2,
            timeout: Some(Duration::from_millis(1500)),
            on_failure: OnFailure::Warn,
        }
    );
    let ignore: NodeData = serde_json::from_str(r#"{"kind":"x","on_failure":"ignore"}"#).unwrap();
    assert_eq!(ignore.on_failure, OnFailure::Ignore);
}

#[test]
fn a_bad_timeout_is_refused_before_cooking() {
    let mut f = fixture(-1, 0, false);
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        f.set(|n| n.timeout = Some(bad));
        assert!(matches!(
            f.graph.validate(&f.registry),
            Err(GraphError::InvalidTimeout { .. })
        ));
        assert!(f.engine(Arc::default()).cook(&f.graph, &[]).is_err());
    }
}

#[test]
fn the_policy_is_not_in_cache_keys() {
    let mut f = fixture(-1, 0, false);
    let keys = |f: &Fixture| -> Vec<ContentHash> {
        f.engine(Arc::default())
            .generate(&f.graph, &[])
            .unwrap()
            .all()
            .map(|it| it.key)
            .collect()
    };
    let base = keys(&f);
    f.set(|n| {
        n.retries = 3;
        n.timeout = Some(9.0);
        n.on_failure = OnFailure::Ignore;
        n.executor = Some("elsewhere".into());
    });
    f.graph.executor = Some("farm".into());
    let engine = |f: &Fixture| {
        f.engine(Arc::default())
            .add_executor("elsewhere", Arc::new(ThreadExecutor::new(1)))
            .add_executor("farm", Arc::new(ThreadExecutor::new(1)))
    };
    let changed: Vec<ContentHash> = engine(&f)
        .generate(&f.graph, &[])
        .unwrap()
        .all()
        .map(|it| it.key)
        .collect();
    assert_eq!(base, changed);
}

// ---- on_failure ----------------------------------------------------------------------

#[test]
fn on_failure_error_stops_dependants_including_fan_in() {
    let f = fixture(1, u32::MAX, false);
    let (items, _) = f.cook();
    assert_eq!(items.count(&id("flaky"), ItemState::Cooked), 2);
    assert_eq!(items.count(&id("flaky"), ItemState::Failed), 1);
    // Independent items finish, as they always did.
    assert_eq!(items.count(&id("plus1"), ItemState::Cooked), 2);
    assert_eq!(items.count(&id("fast"), ItemState::Cooked), 1);
    // The fan-in does not go on without the failed item.
    assert_eq!(items.count(&id("all"), ItemState::Cancelled), 1);
    assert_eq!(items.count(&id("sum"), ItemState::Cancelled), 1);
    assert_eq!(sum_of(&items), None);
}

#[test]
fn on_failure_warn_lets_independent_items_and_fan_in_finish() {
    for policy in [OnFailure::Warn, OnFailure::Ignore] {
        let mut f = fixture(1, u32::MAX, false);
        f.set(|n| n.on_failure = policy);
        let (items, _) = f.cook();
        let failed = &items.items(&id("flaky"))[1];
        assert_eq!(failed.state, ItemState::Failed, "{policy:?}");
        assert!(matches!(failed.error, Some(NodeError::Failed(_))));
        // Its own descendant is cancelled (it has no input)...
        assert_eq!(items.items(&id("plus1"))[1].state, ItemState::Cancelled);
        // ...everything independent finishes, and the fan-in goes on without it.
        assert_eq!(items.count(&id("plus1"), ItemState::Cooked), 2);
        assert_eq!(items.count(&id("fast"), ItemState::Cooked), 1);
        assert_eq!(items.count(&id("all"), ItemState::Cooked), 1);
        assert_eq!(items.count(&id("sum"), ItemState::Cooked), 1);
        assert_eq!(sum_of(&items), Some(1 + 3), "{policy:?}");
    }
}

#[test]
fn a_warned_failure_upstream_does_not_excuse_an_erroring_one() {
    // `plus1` itself fails hard on one item (error policy) while `flaky` (warn) fails
    // on another: the fan-in must still stop.
    struct Bad;
    impl Processor for Bad {
        fn spec(&self) -> NodeSpec {
            NodeSpec::new("plus1")
                .input("x", PortType::Scalar)
                .output("x", PortType::Scalar)
        }
        fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
            let x: i64 = *ctx.input("x")?;
            if x == 2 {
                return Err(NodeError::msg("hard failure"));
            }
            Ok(Outputs::new().with("x", Value::new(PortType::Scalar, x + 1)))
        }
    }
    let mut f = fixture(1, u32::MAX, false);
    let mut r = (*f.registry).clone();
    r.processor(Bad).unwrap();
    f.registry = Arc::new(r);
    f.set(|n| n.on_failure = OnFailure::Warn);
    let (items, _) = f.cook();
    assert_eq!(items.count(&id("plus1"), ItemState::Failed), 1);
    assert_eq!(items.count(&id("all"), ItemState::Cancelled), 1);
    assert_eq!(sum_of(&items), None);
}

// ---- retries -------------------------------------------------------------------------

/// Delegates to a thread pool, recording what each submitted job carried.
#[derive(Debug)]
struct Recording {
    inner: ThreadExecutor,
    seen: Mutex<Vec<(ItemId, u32, Needs, FailurePolicy)>>,
}

impl Executor for Recording {
    fn submit(&self, job: Job, handle: JobHandle) {
        self.seen
            .lock()
            .unwrap()
            .push((job.item.clone(), job.attempt, job.needs, job.policy));
        self.inner.submit(job, handle);
    }
}

#[test]
fn retries_rerun_a_failed_item_as_a_new_attempt() {
    let mut f = fixture(1, 2, false);
    f.set(|n| n.retries = 2);
    let rec = Arc::new(Recording {
        inner: ThreadExecutor::new(2),
        seen: Mutex::default(),
    });
    let cache = Arc::new(MemoryCache::new());
    let items = Engine::new(f.registry.clone(), cache.clone())
        .with_executor(rec.clone())
        .cook(&f.graph, &[])
        .unwrap();
    assert_eq!(items.count(&id("flaky"), ItemState::Cooked), 3);
    assert_eq!(f.attempts(1), 3, "two failures, then success");
    assert_eq!(f.attempts(0), 1);
    assert_eq!(sum_of(&items), Some(1 + 2 + 3));
    assert!(cache.contains(&items.items(&id("flaky"))[1].key));

    let seen = rec.seen.lock().unwrap();
    let flaky1: Vec<_> = seen
        .iter()
        .filter(|(it, ..)| it.to_string() == "flaky[1.0]")
        .collect();
    assert_eq!(
        flaky1.iter().map(|(_, a, ..)| *a).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    for (it, _, needs, policy) in seen.iter() {
        let want = if it.node == id("flaky") {
            Needs {
                gpus: 1,
                ..Needs::default()
            }
        } else {
            Needs::default()
        };
        assert_eq!(*needs, want, "{it}");
        assert_eq!(policy.retries, if it.node == id("flaky") { 2 } else { 0 });
    }
}

#[test]
fn retries_run_out() {
    let mut f = fixture(1, 2, false);
    f.set(|n| n.retries = 1);
    let (items, _) = f.cook();
    let it = &items.items(&id("flaky"))[1];
    assert_eq!(it.state, ItemState::Failed);
    assert_eq!(it.error, Some(NodeError::msg("flaky on 1, attempt 2")));
    assert_eq!(f.attempts(1), 2);
}

// ---- timeout -------------------------------------------------------------------------

#[test]
fn a_timeout_cancels_only_that_job_and_caches_nothing() {
    let mut f = fixture(1, u32::MAX, true);
    f.set(|n| n.timeout = Some(0.1));
    let t = Instant::now();
    let (items, cache) = f.cook();
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    let it = &items.items(&id("flaky"))[1];
    assert_eq!(it.state, ItemState::Failed);
    assert_eq!(
        it.error,
        Some(NodeError::TimedOut(Duration::from_millis(100)))
    );
    assert!(!cache.contains(&it.key));
    // The cook itself was not cancelled: the others finished.
    assert!(!items.cancelled);
    assert_eq!(items.count(&id("flaky"), ItemState::Cooked), 2);
    assert_eq!(items.count(&id("fast"), ItemState::Cooked), 1);
}

#[test]
fn a_timed_out_attempt_is_retried() {
    let mut f = fixture(1, 1, true);
    f.set(|n| {
        n.timeout = Some(0.1);
        n.retries = 1;
    });
    let (items, cache) = f.cook();
    let it = &items.items(&id("flaky"))[1];
    assert_eq!(it.state, ItemState::Cooked);
    assert!(cache.contains(&it.key));
    assert_eq!(f.attempts(1), 2);
    assert_eq!(sum_of(&items), Some(1 + 2 + 3));
}
