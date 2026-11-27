//! Node identity vs display label: the id is assigned at creation and frozen; the
//! label is free text, undoable, and never part of a cache key.

use std::sync::{Arc, Mutex};

use mold_graph::*;

type Log = Arc<Mutex<Vec<String>>>;

/// One item, Scalar 1; logs every cook.
struct One(Log);

impl Processor for One {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("one").output("x", PortType::Scalar)
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        self.0.lock().unwrap().push(ctx.item.to_string());
        Ok(Outputs::new().with("x", Value::new(PortType::Scalar, 1i64)))
    }
}

/// Input + 1; logs every cook.
struct Inc(Log);

impl Processor for Inc {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("inc")
            .input("x", PortType::Scalar)
            .output("x", PortType::Scalar)
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        self.0.lock().unwrap().push(ctx.item.to_string());
        let x: i64 = *ctx.input("x")?;
        Ok(Outputs::new().with("x", Value::new(PortType::Scalar, x + 1)))
    }
}

fn editor(log: &Log) -> (Editor, NodeId, NodeId) {
    let mut r = Registry::new();
    r.processor(One(log.clone()))
        .unwrap()
        .processor(Inc(log.clone()))
        .unwrap();
    let mut ed = Editor::new(Arc::new(r));
    let a = ed.add_node("one").unwrap();
    let b = ed.add_node("inc").unwrap();
    ed.connect(OutputRef::new(a.clone(), "x"), &b, "x").unwrap();
    (ed, a, b)
}

fn keys(engine: &Engine, g: &Graph) -> Vec<ContentHash> {
    engine
        .generate(g, &[])
        .unwrap()
        .all()
        .map(|i| i.key)
        .collect()
}

#[test]
fn the_label_is_not_in_cache_keys_and_recooks_nothing() {
    let log = Log::default();
    let (mut ed, a, b) = editor(&log);
    let engine = Engine::new(ed.registry().clone(), Arc::new(MemoryCache::new()))
        .with_executor(Arc::new(ThreadExecutor::new(2)));
    let before = keys(&engine, ed.graph());
    engine.cook(ed.graph(), &[]).unwrap();
    assert_eq!(log.lock().unwrap().len(), 2);

    ed.set_label(&a, Some("Receptor prep")).unwrap();
    ed.set_label(&b, Some("Score")).unwrap();
    assert_eq!(keys(&engine, ed.graph()), before);
    let items = engine.cook(ed.graph(), &[]).unwrap();
    assert_eq!(items.count(&b, ItemState::Cached), 1);
    assert_eq!(log.lock().unwrap().len(), 2, "relabelling re-cooks nothing");
}

#[test]
fn set_label_and_its_undo_round_trip_and_the_id_never_changes() {
    let (mut ed, a, b) = editor(&Log::default());
    let original = ed.graph().clone();
    assert_eq!(a, NodeId::new("one-1"));
    assert_eq!(ed.graph().nodes[&a].label, None);

    ed.set_label(&a, Some("first")).unwrap();
    ed.set_label(&a, Some("second")).unwrap();
    assert_eq!(ed.graph().nodes[&a].label.as_deref(), Some("second"));
    // Same id, same edges.
    assert!(ed.graph().contains(&a));
    assert_eq!(ed.graph().nodes[&b].inputs["x"].node, a);

    assert!(ed.undo().unwrap());
    assert_eq!(ed.graph().nodes[&a].label.as_deref(), Some("first"));
    assert!(ed.undo().unwrap());
    assert_eq!(*ed.graph(), original);
    assert!(ed.redo().unwrap());
    assert!(ed.redo().unwrap());
    assert_eq!(ed.graph().nodes[&a].label.as_deref(), Some("second"));

    // Clearing goes back to `None` (the kind's display name), and is undoable.
    ed.set_label(&a, None).unwrap();
    assert_eq!(ed.graph().nodes[&a].label, None);
    ed.undo().unwrap();
    assert_eq!(ed.graph().nodes[&a].label.as_deref(), Some("second"));

    // Unknown node.
    assert_eq!(
        ed.set_label(&NodeId::new("nope"), Some("x")),
        Err(GraphError::UnknownNode(NodeId::new("nope")))
    );
}

#[test]
fn two_nodes_may_share_a_label() {
    let (mut ed, a, b) = editor(&Log::default());
    ed.set_label(&a, Some("same")).unwrap();
    ed.set_label(&b, Some("same")).unwrap();
    let g = ed.graph();
    assert_eq!(g.nodes[&a].label, g.nodes[&b].label);
    assert_ne!(a, b);
    g.validate(ed.registry()).unwrap();
}

#[test]
fn graphs_without_a_label_load_and_the_label_serializes_after_kind() {
    let old = r#"{ "nodes": { "one-1": { "kind": "one" } } }"#;
    let g: Graph = serde_json::from_str(old).unwrap();
    assert_eq!(g.nodes[&NodeId::new("one-1")].label, None);
    let text = serde_json::to_string(&g).unwrap();
    assert!(!text.contains("label"), "{text}");

    let mut labelled = g.clone();
    labelled.nodes.get_mut(&NodeId::new("one-1")).unwrap().label = Some("Ligands".into());
    labelled
        .nodes
        .get_mut(&NodeId::new("one-1"))
        .unwrap()
        .executor = Some("local".into());
    let text = serde_json::to_string(&labelled).unwrap();
    assert!(
        text.contains(r#"{"kind":"one","label":"Ligands","executor":"local"}"#),
        "{text}"
    );
    assert_eq!(serde_json::from_str::<Graph>(&text).unwrap(), labelled);
}

#[test]
fn executor_and_failure_policy_edits_are_undoable() {
    let (mut ed, a, _) = editor(&Log::default());
    let original = ed.graph().clone();
    ed.apply(Edit::SetExecutor {
        node: a.clone(),
        executor: Some("local".into()),
    })
    .unwrap();
    ed.apply(Edit::SetFailurePolicy {
        node: a.clone(),
        retries: 2,
        timeout: Some(30.0),
        on_failure: OnFailure::Warn,
    })
    .unwrap();
    let n = &ed.graph().nodes[&a];
    assert_eq!(n.executor.as_deref(), Some("local"));
    assert_eq!(
        (n.retries, n.timeout, n.on_failure),
        (2, Some(30.0), OnFailure::Warn)
    );

    // A bad timeout is refused and changes nothing.
    let before = ed.graph().clone();
    assert!(matches!(
        ed.apply(Edit::SetFailurePolicy {
            node: a.clone(),
            retries: 0,
            timeout: Some(-1.0),
            on_failure: OnFailure::Error,
        }),
        Err(GraphError::InvalidTimeout { .. })
    ));
    assert_eq!(*ed.graph(), before);

    ed.undo().unwrap();
    ed.undo().unwrap();
    assert_eq!(*ed.graph(), original);
}
