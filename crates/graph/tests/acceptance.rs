//! Acceptance tests from `crates/graph/CLAUDE.md`, on the work-item model.
//!
//! Test pipeline (a miniature of the v0.1 graph):
//! ```text
//! ligands (static, `count` items) -> prep (1:1) -> cofold (dynamic, `samples` per ligand)
//! receptor (1 item) ---------------------------------^ broadcast
//! cofold -> gate (1:1, `threshold`) -> collect (wait for all) -> summary
//! ```

use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use mold_graph::*;

type Log = Arc<Mutex<Vec<String>>>;
type GenFn = dyn Fn(&GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> + Send + Sync;
type CookFn = dyn Fn(&CookContext<'_>) -> Result<Outputs, NodeError> + Send + Sync;

/// A processor assembled from closures. Logs every cook by item id.
struct TestNode {
    spec: NodeSpec,
    when: GenerateWhen,
    generate: Option<Box<GenFn>>,
    cook: Box<CookFn>,
    log: Log,
}

impl Processor for TestNode {
    fn spec(&self) -> NodeSpec {
        self.spec.clone()
    }
    fn generate_when(&self) -> GenerateWhen {
        self.when
    }
    fn generate(&self, ctx: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
        match &self.generate {
            Some(g) => g(ctx),
            None => Ok(vec![NewItem::work()]),
        }
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        self.log.lock().unwrap().push(ctx.item.to_string());
        (self.cook)(ctx)
    }
}

fn node(
    spec: NodeSpec,
    log: &Log,
    cook: impl Fn(&CookContext<'_>) -> Result<Outputs, NodeError> + Send + Sync + 'static,
) -> TestNode {
    TestNode {
        spec,
        when: GenerateWhen::EachUpstreamGenerated,
        generate: None,
        cook: Box::new(cook),
        log: log.clone(),
    }
}

fn int(ty: PortType, v: i64) -> Value {
    Value::new(ty, v)
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

/// Hooks individual tests use to slow down or fail specific items.
#[derive(Clone, Default)]
struct Hooks {
    /// prep[0] waits for this before finishing (streaming test).
    hold_prep0: Option<Signal>,
    /// Set when gate[1.*] cooks.
    gate1_cooked: Option<Signal>,
}

fn registry(log: &Log, hooks: Hooks) -> Registry {
    let mut r = Registry::new();
    let lt = PortType::LigandSet;
    let ps = PortType::PoseSet;

    // Static source: `count` items, known from params alone.
    let mut ligands = node(
        NodeSpec::new("ligands")
            .output("ligands", lt)
            .param(ParamSpec::new("count", 3i64)),
        log,
        move |ctx| {
            let i = ctx.attrs.get("ligand").and_then(|v| {
                if let ParamValue::Int(i) = v {
                    Some(*i)
                } else {
                    None
                }
            });
            Ok(Outputs::new().with("ligands", int(lt, i.unwrap())))
        },
    );
    ligands.generate = Some(Box::new(|ctx| {
        let n = ctx.params.int("count")?;
        Ok((0..n)
            .map(|i| NewItem::work().attr("ligand", i).attr("batch", "a"))
            .collect())
    }));
    r.processor(ligands).unwrap();

    let hold = hooks.hold_prep0.clone();
    let gate1 = hooks.gate1_cooked.clone();
    r.processor(node(
        NodeSpec::new("prep")
            .input("in", lt)
            .output("out", lt)
            .param(ParamSpec::new("mode", "std")),
        log,
        move |ctx| {
            if let (Some(h), Some(g)) = (&hold, &gate1)
                && ctx.item.index == [0, 0]
            {
                // Slow item: only finishes once a *downstream* item of its fast sibling
                // has cooked. A node-at-a-time scheduler times out here.
                h.set();
                if !g.wait(Duration::from_secs(10)) {
                    return Err(NodeError::msg(
                        "sibling's downstream never cooked: not streaming",
                    ));
                }
            }
            Ok(Outputs::new().with("out", int(lt, ctx.input::<i64>("in")? * 10)))
        },
    ))
    .unwrap();

    r.processor(node(
        NodeSpec::new("receptor").output("structure", PortType::Structure),
        log,
        |_| {
            Ok(Outputs::new().with(
                "structure",
                Value::new(PortType::Structure, "rec".to_string()),
            ))
        },
    ))
    .unwrap();

    // Dynamic fan-out: `samples` poses per ligand, known only once the ligand cooked.
    let mut cofold = node(
        NodeSpec::new("cofold")
            .input("ligand", lt)
            .broadcast_input("receptor", PortType::Structure)
            .output("poses", ps)
            .param(ParamSpec::new("samples", 2i64)),
        log,
        move |ctx| {
            let lig = *ctx.input::<i64>("ligand")?;
            let rec = ctx.input::<String>("receptor")?;
            assert_eq!(rec, "rec");
            let pose = ctx
                .params
                .int("samples")
                .map(|_| ctx.item.index.last().copied().unwrap() as i64)?;
            Ok(Outputs::new()
                .with("poses", int(ps, lig + pose))
                .attr("score", (lig + pose) as f64))
        },
    );
    cofold.when = GenerateWhen::EachUpstreamCooked;
    cofold.generate = Some(Box::new(|ctx| {
        let parent = ctx.parent.as_ref().expect("per-item input");
        assert!(
            parent.outputs.is_some(),
            "dynamic generation sees the cooked parent"
        );
        let n = ctx.params.int("samples")?;
        Ok((0..n).map(|j| NewItem::work().attr("pose", j)).collect())
    }));
    r.processor(cofold).unwrap();

    let gate1 = hooks.gate1_cooked.clone();
    r.processor(node(
        NodeSpec::new("gate")
            .input("in", ps)
            .output("out", ps)
            .param(ParamSpec::new("threshold", 0.5)),
        log,
        move |ctx| {
            if let Some(g) = &gate1
                && ctx.item.index[0] == 1
            {
                g.set();
            }
            Ok(Outputs::new().with("out", ctx.inputs.one("in")?.clone()))
        },
    ))
    .unwrap();

    r.partitioner(WaitForAll::new("collect", ps)).unwrap();
    r.partitioner(PartitionByAttribute::new("by_attr", ps))
        .unwrap();

    r.processor(node(
        NodeSpec::new("summary")
            .input("in", ps)
            .output("table", PortType::Table),
        log,
        |ctx| {
            let all: Vec<i64> = ctx
                .inputs
                .all_as::<i64>("in")?
                .into_iter()
                .copied()
                .collect();
            Ok(Outputs::new().with("table", Value::new(PortType::Table, all)))
        },
    ))
    .unwrap();
    r
}

struct Fixture {
    log: Log,
    editor: Editor,
    engine: Engine,
    cache: Arc<MemoryCache>,
}

fn fixture(hooks: Hooks) -> Fixture {
    let log: Log = Log::default();
    let reg = Arc::new(registry(&log, hooks));
    let mut ed = Editor::new(reg.clone());
    for (id, kind) in [
        ("ligands", "ligands"),
        ("prep", "prep"),
        ("receptor", "receptor"),
        ("cofold", "cofold"),
        ("gate", "gate"),
        ("collect", "collect"),
        ("summary", "summary"),
    ] {
        ed.add_node_with_id(id, kind).unwrap();
    }
    ed.connect("ligands.ligands", &"prep".into(), "in").unwrap();
    ed.connect("prep.out", &"cofold".into(), "ligand").unwrap();
    ed.connect("receptor.structure", &"cofold".into(), "receptor")
        .unwrap();
    ed.connect("cofold.poses", &"gate".into(), "in").unwrap();
    ed.connect("gate.out", &"collect".into(), "in").unwrap();
    ed.connect("collect.out", &"summary".into(), "in").unwrap();
    let cache = Arc::new(MemoryCache::new());
    let engine = Engine::new(reg, cache.clone()).with_executor(Arc::new(ThreadExecutor::new(4)));
    Fixture {
        log,
        editor: ed,
        engine,
        cache,
    }
}

fn take_log(log: &Log) -> Vec<String> {
    let mut v = std::mem::take(&mut *log.lock().unwrap());
    v.sort();
    v
}

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

#[test]
fn static_items_are_counted_before_cooking() {
    let mut f = fixture(Hooks::default());
    f.editor.set_param(&id("ligands"), "count", 5i64).unwrap();
    let items = f.engine.generate(f.editor.graph(), &[]).unwrap();

    assert_eq!(items.items(&id("ligands")).len(), 5);
    assert_eq!(
        items.items(&id("prep")).len(),
        5,
        "1:1 static items follow statically"
    );
    assert_eq!(items.count(&id("prep"), ItemState::Uncooked), 5);
    assert!(
        items.items(&id("cofold")).is_empty(),
        "dynamic items need cooked parents"
    );
    assert!(
        f.log.lock().unwrap().is_empty(),
        "generation cooked nothing"
    );
}

#[test]
fn dynamic_fan_out_generates_children_only_after_the_parent_cooks() {
    let f = fixture(Hooks::default());
    let cooked: Arc<Mutex<BTreeSet<ItemId>>> = Default::default();
    let violations: Arc<Mutex<Vec<String>>> = Default::default();
    let (c, v) = (cooked.clone(), violations.clone());
    let opts = CookOptions {
        observer: Some(Box::new(move |it: &WorkItem| {
            if it.state == ItemState::Cooked {
                c.lock().unwrap().insert(it.id.clone());
            }
            if it.id.node.as_str() == "cofold" && it.state == ItemState::Waiting {
                let parent = &it.parents[0];
                if !c.lock().unwrap().contains(parent) {
                    v.lock()
                        .unwrap()
                        .push(format!("{} created before {parent} cooked", it.id));
                }
            }
        })),
        ..Default::default()
    };
    let items = f
        .engine
        .cook_with(f.editor.graph(), &[id("cofold")], opts)
        .unwrap();

    assert!(
        violations.lock().unwrap().is_empty(),
        "{:?}",
        violations.lock().unwrap()
    );
    let cofold = items.items(&id("cofold"));
    assert_eq!(cofold.len(), 3 * 2);
    assert!(cofold.iter().all(|i| i.state == ItemState::Cooked));
    // Children inherit parent attributes and keep stable index paths.
    let c = items
        .item(&ItemId {
            node: id("cofold"),
            index: vec![2, 0, 1],
        })
        .unwrap();
    let attrs = items.attrs(&c.id).unwrap();
    assert_eq!(attrs.get("ligand"), Some(&ParamValue::Int(2)));
    assert_eq!(attrs.get("pose"), Some(&ParamValue::Int(1)));
    assert_eq!(
        attrs.get("score"),
        Some(&ParamValue::Float(21.0)),
        "attribute added by the cook"
    );
}

#[test]
fn streaming_fast_items_downstream_cooks_before_slow_sibling_finishes() {
    let hooks = Hooks {
        hold_prep0: Some(Signal::default()),
        gate1_cooked: Some(Signal::default()),
    };
    let f = fixture(hooks);
    let mut ed = f.editor;
    ed.set_param(&id("ligands"), "count", 2i64).unwrap();
    let items = f.engine.cook(ed.graph(), &[id("gate")]).unwrap();

    let prep0 = items
        .item(&ItemId {
            node: id("prep"),
            index: vec![0, 0],
        })
        .unwrap();
    assert_eq!(prep0.state, ItemState::Cooked, "{:?}", prep0.error);
    assert_eq!(items.count(&id("gate"), ItemState::Cooked), 4);
}

#[test]
fn wait_for_all_waits_for_every_item_and_merges() {
    let f = fixture(Hooks::default());
    let items = f.engine.cook(f.editor.graph(), &[id("summary")]).unwrap();

    let parts = items.items(&id("collect"));
    assert_eq!(parts.len(), 1);
    let p = parts[0];
    assert_eq!(p.role, ItemRole::Partition);
    assert_eq!(p.parents.len(), 6, "every gate item is a member");
    assert_eq!(
        p.attrs.get("batch"),
        Some(&ParamValue::Str("a".into())),
        "agreed attribute kept"
    );
    assert!(
        !p.attrs.contains_key("ligand"),
        "disagreeing attribute dropped"
    );

    let summary = items.items(&id("summary"));
    assert_eq!(summary.len(), 1);
    let table = summary[0].result.as_ref().unwrap().get("table").unwrap();
    // ligand*10 + pose, in index order.
    assert_eq!(
        table.downcast_ref::<Vec<i64>>().unwrap(),
        &vec![0, 1, 10, 11, 20, 21]
    );
    let log = take_log(&f.log);
    assert_eq!(log.iter().filter(|s| s.starts_with("summary")).count(), 1);
}

#[test]
fn partition_by_attribute_groups_per_value() {
    let f = fixture(Hooks::default());
    let mut ed = f.editor;
    ed.add_node_with_id("per_ligand", "by_attr").unwrap();
    ed.set_param(&id("per_ligand"), "attribute", "ligand")
        .unwrap();
    ed.connect("gate.out", &id("per_ligand"), "in").unwrap();
    let items = f.engine.cook(ed.graph(), &[id("per_ligand")]).unwrap();

    let parts = items.items(&id("per_ligand"));
    assert_eq!(parts.len(), 3);
    for (i, p) in parts.iter().enumerate() {
        assert_eq!(p.attrs.get("ligand"), Some(&ParamValue::Int(i as i64)));
        assert_eq!(items.values(&p.id, "out").len(), 2);
    }
}

#[test]
fn downstream_param_change_recooks_only_affected_items() {
    let f = fixture(Hooks::default());
    let mut ed = f.editor;
    f.engine.cook(ed.graph(), &[]).unwrap();
    let first = take_log(&f.log);
    assert_eq!(
        first.len(),
        3 + 3 + 1 + 6 + 6 + 1,
        "every work item cooked once: {first:?}"
    );

    ed.set_param(&id("gate"), "threshold", 0.9).unwrap();
    let items = f.engine.cook(ed.graph(), &[]).unwrap();
    let second = take_log(&f.log);

    // Only gate items (once each) and what consumes them.
    let gate: Vec<&String> = second.iter().filter(|s| s.starts_with("gate[")).collect();
    assert_eq!(gate.len(), 6);
    assert_eq!(
        gate.iter().collect::<BTreeSet<_>>().len(),
        6,
        "each gate item cooked once"
    );
    assert_eq!(second.len(), 6 + 1, "gate items + summary only: {second:?}");
    for n in ["ligands", "prep", "cofold", "receptor"] {
        let all = items.items(&id(n));
        assert!(
            all.iter().all(|i| i.state == ItemState::Cached),
            "{n} items are cache hits"
        );
    }
}

#[test]
fn undo_redo_restores_graph_exactly_and_leaves_the_cache_alone() {
    let f = fixture(Hooks::default());
    // A fresh history: only the edits below are undoable.
    let mut ed = Editor::from_graph(f.editor.into_graph(), f.engine.registry().clone()).unwrap();
    f.engine.cook(ed.graph(), &[]).unwrap();
    take_log(&f.log);
    let cached = f.cache.len();

    let mut snapshots = vec![ed.graph().clone()];
    ed.set_param(&id("gate"), "threshold", 0.1).unwrap();
    snapshots.push(ed.graph().clone());
    ed.disconnect(&id("summary"), "in").unwrap();
    snapshots.push(ed.graph().clone());
    ed.remove_node(&id("gate")).unwrap(); // also drops gate -> collect
    snapshots.push(ed.graph().clone());
    let extra = ed.add_node("summary").unwrap();
    snapshots.push(ed.graph().clone());
    ed.connect("collect.out", &extra, "in").unwrap();
    snapshots.push(ed.graph().clone());

    for want in snapshots.iter().rev().skip(1) {
        assert!(ed.undo().unwrap());
        assert_eq!(ed.graph(), want);
    }
    assert!(!ed.undo().unwrap());
    for want in snapshots.iter().skip(1) {
        assert!(ed.redo().unwrap());
        assert_eq!(ed.graph(), want);
    }
    while ed.undo().unwrap() {}
    assert_eq!(ed.graph(), &snapshots[0]);

    assert_eq!(f.cache.len(), cached, "undo/redo never touches the cache");
    let items = f.engine.cook(ed.graph(), &[]).unwrap();
    assert!(
        take_log(&f.log).is_empty(),
        "back at the original graph: everything is a cache hit"
    );
    assert_eq!(items.count(&id("gate"), ItemState::Cached), 6);
}

#[test]
fn mismatched_ports_and_cycles_are_refused() {
    let f = fixture(Hooks::default());
    let mut ed = f.editor;
    let before = ed.graph().clone();

    // Table output into a PoseSet input.
    let err = ed.connect("summary.table", &id("gate"), "in").unwrap_err();
    assert!(matches!(
        err,
        GraphError::TypeMismatch {
            from_ty: PortType::Table,
            to_ty: PortType::PoseSet,
            ..
        }
    ));

    // gate.out -> cofold? Type mismatch too; build a real cycle: gate -> a new gate -> gate.
    ed.add_node_with_id("gate2", "gate").unwrap();
    ed.connect("gate.out", &id("gate2"), "in").unwrap();
    let err = ed.connect("gate2.out", &id("gate"), "in").unwrap_err();
    assert_eq!(err, GraphError::Cycle { at: id("gate") });
    let err = ed.connect("gate2.out", &id("gate2"), "in").unwrap_err();
    assert!(matches!(err, GraphError::Cycle { .. }), "self-loop");

    // Refused edits leave no trace in the graph or the history.
    ed.undo().unwrap();
    ed.undo().unwrap();
    assert_eq!(ed.graph(), &before);
}

#[test]
fn a_failed_item_cancels_its_descendants_only() {
    let log = Log::default();
    let mut r = registry(&log, Hooks::default());
    r.processor(node(
        NodeSpec::new("flaky")
            .input("in", PortType::LigandSet)
            .output("out", PortType::LigandSet),
        &log,
        |ctx| {
            let v = *ctx.input::<i64>("in")?;
            if v == 10 {
                return Err(NodeError::msg("bad ligand"));
            }
            Ok(Outputs::new().with("out", int(PortType::LigandSet, v)))
        },
    ))
    .unwrap();
    let reg = Arc::new(r);
    let mut ed = Editor::new(reg.clone());
    ed.add_node_with_id("ligands", "ligands").unwrap();
    ed.add_node_with_id("prep", "prep").unwrap();
    ed.add_node_with_id("flaky", "flaky").unwrap();
    ed.add_node_with_id("prep2", "prep").unwrap();
    ed.connect("ligands.ligands", &id("prep"), "in").unwrap();
    ed.connect("prep.out", &id("flaky"), "in").unwrap();
    ed.connect("flaky.out", &id("prep2"), "in").unwrap();
    let engine = Engine::new(reg, Arc::new(MemoryCache::new()));
    let items = engine.cook(ed.graph(), &[]).unwrap();

    assert_eq!(items.count(&id("flaky"), ItemState::Failed), 1);
    assert_eq!(items.count(&id("prep2"), ItemState::Cancelled), 1);
    assert_eq!(items.count(&id("prep2"), ItemState::Cooked), 2);
}

#[test]
fn cancel_stops_the_cook() {
    let log = Log::default();
    let mut r = Registry::new();
    let started = Signal::default();
    let s = started.clone();
    r.processor(node(
        NodeSpec::new("wait").output("out", PortType::Scalar),
        &log,
        move |ctx| {
            s.set();
            while !ctx.cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(NodeError::Cancelled)
        },
    ))
    .unwrap();
    let reg = Arc::new(r);
    let mut ed = Editor::new(reg.clone());
    ed.add_node_with_id("w", "wait").unwrap();
    let engine = Engine::new(reg, Arc::new(MemoryCache::new()));
    let cancel = CancelToken::new();
    let c = cancel.clone();
    std::thread::spawn(move || {
        started.wait(Duration::from_secs(10));
        c.cancel();
    });
    let items = engine
        .cook_with(
            ed.graph(),
            &[],
            CookOptions {
                cancel,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(items.cancelled);
    assert_eq!(items.count(&id("w"), ItemState::Cancelled), 1);
}
