//! Acceptance tests from `crates/ui-graph/CLAUDE.md`, headless: the interaction
//! state machine and layout run on `&Graph` + view state, with no window.

use std::sync::Arc;

use mold_graph::{
    CookContext, Edit, Engine, MemoryCache, NodeError, NodeId, NodeSpec, OutputRef, Outputs,
    ParamSpec, ParamValue, PortType, Processor, Registry, Value,
};
use mold_ui_graph::canvas::{
    Button, CanvasModel, CanvasState, DefaultNodeMetrics, DropVerdict, EvalState, GraphDoc,
    Modifiers, NodeMetrics, Outcome, PortDir, Refusal, RendererSlot, Renderers, Vec2, layout,
    metrics,
};

/// A node kind that is only a spec plus a trivial cook (unit payload per output).
struct Kind(NodeSpec);

impl Processor for Kind {
    fn spec(&self) -> NodeSpec {
        self.0.clone()
    }
    fn cook(&self, _: &CookContext<'_>) -> Result<Outputs, NodeError> {
        Ok(self.0.outputs.iter().fold(Outputs::new(), |o, p| {
            o.with(p.name.clone(), Value::new(p.ty, ()))
        }))
    }
}

fn registry(extra: &[NodeSpec]) -> Arc<Registry> {
    let mut reg = Registry::new();
    let specs = [
        NodeSpec::new("table_source").output("rows", PortType::Table),
        NodeSpec::new("pose_source").output("poses", PortType::PoseSet),
        NodeSpec::new("pose_gate")
            .input("poses", PortType::PoseSet)
            .output("kept", PortType::PoseSet)
            .param(ParamSpec::new("max_clashes", 2i64)),
    ];
    for spec in specs.into_iter().chain(extra.iter().cloned()) {
        reg.processor(Kind(spec)).expect("valid spec");
    }
    Arc::new(reg)
}

struct Fixture {
    doc: GraphDoc,
    state: CanvasState,
    renderers: Renderers<dyn NodeMetrics>,
}

impl Fixture {
    fn new(reg: Arc<Registry>, nodes: &[(&str, &str)]) -> Self {
        let mut doc = GraphDoc::empty(reg);
        let mut state = CanvasState::new();
        for (i, (id, kind)) in nodes.iter().enumerate() {
            doc.apply(Edit::AddNode {
                id: NodeId::from(*id),
                node: mold_graph::NodeData::new(*kind),
            })
            .expect("add node");
            // Far apart, so ports never overlap.
            state.set_position(NodeId::from(*id), Vec2::new(i as f32 * 400.0, 100.0));
        }
        // Setting up the fixture is not part of what the tests undo.
        let graph = doc.graph().clone();
        let doc = GraphDoc::new(
            mold_graph::Editor::from_graph(graph, doc.registry().clone()).expect("valid"),
        );
        Self {
            doc,
            state,
            renderers: Renderers::default(),
        }
    }

    fn model(&mut self) -> CanvasModel {
        layout(
            &mut self.state,
            self.doc.graph(),
            self.doc.registry(),
            &self.renderers,
            self.doc.eval(),
        )
    }

    fn port(&mut self, node: &str, port: &str, dir: PortDir) -> Vec2 {
        let model = self.model();
        let n = model.node(&NodeId::from(node)).expect("node laid out");
        let p = match dir {
            PortDir::Input => n.input(port),
            PortDir::Output => n.output(port),
        };
        p.expect("port laid out").center
    }

    fn down(&mut self, at: Vec2) -> Outcome {
        let model = self.model();
        let (g, r) = (self.doc.graph().clone(), self.doc.registry().clone());
        self.state
            .pointer_down(&model, &g, &r, at, Button::Primary, Modifiers::default())
    }

    fn drag(&mut self, at: Vec2) -> Outcome {
        let model = self.model();
        let (g, r) = (self.doc.graph().clone(), self.doc.registry().clone());
        self.state.pointer_move(&model, &g, &r, at)
    }

    fn up(&mut self, at: Vec2) -> Outcome {
        let model = self.model();
        let (g, r) = (self.doc.graph().clone(), self.doc.registry().clone());
        self.state.pointer_up(&model, &g, &r, at)
    }
}

#[test]
fn dragging_a_table_output_onto_a_poseset_input_is_refused_before_drop() {
    let mut f = Fixture::new(
        registry(&[]),
        &[("table", "table_source"), ("gate", "pose_gate")],
    );
    let before = f.doc.graph().clone();
    let from = f.port("table", "rows", PortDir::Output);
    let to = f.port("gate", "poses", PortDir::Input);

    assert_eq!(f.down(from), Outcome::Redraw);
    assert!(f.state.connecting().is_some(), "a wire drag started");

    // Every input is marked while dragging, before the pointer gets there.
    let model = f.model();
    let refused = DropVerdict::Refuse(Refusal::TypeMismatch {
        from: PortType::Table,
        to: PortType::PoseSet,
    });
    let gate = model.node(&NodeId::from("gate")).unwrap();
    assert_eq!(gate.input("poses").unwrap().drop, Some(refused));

    // Hovering over the input: the state machine reports the refusal.
    f.drag(Vec2::new(to.x + 2.0, to.y - 1.0));
    let hover = f
        .state
        .connecting()
        .unwrap()
        .hover
        .clone()
        .expect("over an input");
    assert_eq!(hover.node, NodeId::from("gate"));
    assert_eq!(hover.input, "poses");
    assert_eq!(hover.verdict, refused);
    let wire = f.model().wire.expect("wire drawn");
    assert_eq!(wire.verdict, Some(refused));
    // A refused wire does not snap to the port.
    assert_ne!(wire.curve.to, to);

    // Dropping issues no edit, and the graph is unchanged.
    let out = f.up(to);
    assert!(!matches!(out, Outcome::Edit(_)), "drop issued {out:?}");
    assert!(f.state.is_idle());
    assert_eq!(f.doc.graph(), &before);
    assert!(!f.doc.editor().history().can_undo());
}

#[test]
fn a_compatible_drop_is_exactly_one_connect_edit_and_undo_removes_it() {
    let mut f = Fixture::new(
        registry(&[]),
        &[("poses", "pose_source"), ("gate", "pose_gate")],
    );
    let before = f.doc.graph().clone();
    let from = f.port("poses", "poses", PortDir::Output);
    let to = f.port("gate", "poses", PortDir::Input);

    f.down(from);
    f.drag(to);
    assert_eq!(
        f.state
            .connecting()
            .unwrap()
            .hover
            .as_ref()
            .unwrap()
            .verdict,
        DropVerdict::Accept
    );
    // An accepted wire snaps to the port.
    assert_eq!(f.model().wire.unwrap().curve.to, to);

    let edit = match f.up(to) {
        Outcome::Edit(e) => e,
        other => panic!("expected an edit, got {other:?}"),
    };
    assert_eq!(
        edit,
        Edit::Connect {
            from: OutputRef::new("poses", "poses"),
            to: NodeId::from("gate"),
            input: "poses".into(),
        }
    );
    f.doc.apply(edit).expect("accepted by mold-graph too");
    assert_eq!(
        f.doc.graph().node(&NodeId::from("gate")).unwrap().inputs["poses"],
        OutputRef::new("poses", "poses")
    );
    assert_eq!(f.model().edges.len(), 1);
    // The changed node is dirty.
    assert_eq!(
        f.doc.eval().get(&NodeId::from("gate")),
        Some(EvalState::Dirty)
    );

    // One undo step removes it; there is no second step.
    assert!(f.doc.undo().unwrap());
    assert_eq!(f.doc.graph(), &before);
    assert!(f.model().edges.is_empty());
    assert!(!f.doc.undo().unwrap());
    // And redo brings it back.
    assert!(f.doc.redo().unwrap());
    assert_eq!(f.model().edges.len(), 1);
}

#[test]
fn a_new_node_kind_needs_no_change_in_this_crate() {
    // Defined only here: ports of types the fixture registry does not use, a
    // broadcast input, more outputs than inputs, and parameters.
    let spec = NodeSpec::new("test_only_fold")
        .input("sequence", PortType::Sequence)
        .broadcast_input("msa", PortType::Msa)
        .output("structure", PortType::Structure)
        .output("confidence", PortType::Scalar)
        .output("report", PortType::Table)
        .param(ParamSpec::new("seeds", 3i64))
        .param(ParamSpec::new("relax", true));
    let mut f = Fixture::new(registry(std::slice::from_ref(&spec)), &[]);
    // Added the way the canvas adds nodes: fresh id, default params, placed at a point.
    let edit = f
        .state
        .add_node(
            f.doc.graph(),
            f.doc.registry(),
            "test_only_fold",
            Vec2::new(50.0, 60.0),
        )
        .expect("kind is registered");
    f.doc.apply(edit).unwrap();

    let model = f.model();
    assert_eq!(model.nodes.len(), 1);
    let n = &model.nodes[0];
    assert_eq!(n.id, NodeId::from("test_only_fold-1"));
    assert!(n.known_kind);
    assert_eq!(n.renderer, RendererSlot::Default);
    assert!(n.selected, "a node just added is selected");
    assert_eq!(n.rect.origin, Vec2::new(50.0, 60.0));

    // Ports come from the spec, inputs on the left edge, outputs on the right.
    let names = |ps: &[mold_ui_graph::canvas::PortView]| {
        ps.iter()
            .map(|p| (p.name.clone(), p.ty))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&n.inputs),
        [
            ("sequence".to_string(), PortType::Sequence),
            ("msa".to_string(), PortType::Msa)
        ]
    );
    assert_eq!(
        names(&n.outputs),
        [
            ("structure".to_string(), PortType::Structure),
            ("confidence".to_string(), PortType::Scalar),
            ("report".to_string(), PortType::Table)
        ]
    );
    assert!(n.inputs.iter().all(|p| p.center.x == n.rect.origin.x));
    assert!(n.outputs.iter().all(|p| p.center.x == n.rect.max().x));
    assert_eq!(n.inputs[1].mode, mold_graph::InputMode::Broadcast);

    // The default renderer's body holds one row per param, below three port rows.
    let expected_h = metrics::HEADER_HEIGHT
        + 3.0 * metrics::ROW_HEIGHT
        + 2.0 * metrics::ROW_HEIGHT
        + metrics::PADDING;
    assert_eq!(n.rect.size, Vec2::new(metrics::NODE_WIDTH, expected_h));
    assert_eq!(
        n.params,
        [
            ("seeds".to_string(), ParamValue::Int(3)),
            ("relax".to_string(), ParamValue::Bool(true))
        ]
    );
    assert!(n.body.origin.y >= n.inputs.last().unwrap().row.max().y - 1e-3);

    // A host renderer for this kind is picked up by kind name, again with no change
    // in this crate.
    struct Tall;
    impl NodeMetrics for Tall {
        fn body_height(&self, _: &NodeSpec, _: &mold_graph::NodeData) -> f32 {
            100.0
        }
    }
    f.renderers = Renderers::new(Arc::new(DefaultNodeMetrics) as Arc<dyn NodeMetrics>)
        .with("test_only_fold", Arc::new(Tall));
    let n = f.model().nodes.remove(0);
    assert_eq!(n.renderer, RendererSlot::Custom);
    assert_eq!(n.body.size.y, 100.0 + metrics::PADDING);
}

#[test]
fn picking_up_a_bound_input_and_dropping_on_empty_canvas_disconnects() {
    let mut f = Fixture::new(
        registry(&[]),
        &[("poses", "pose_source"), ("gate", "pose_gate")],
    );
    f.doc
        .apply(Edit::Connect {
            from: OutputRef::new("poses", "poses"),
            to: NodeId::from("gate"),
            input: "poses".into(),
        })
        .unwrap();
    let to = f.port("gate", "poses", PortDir::Input);
    f.down(to);
    assert!(f.model().edges[0].lifted);
    f.drag(Vec2::new(to.x - 150.0, to.y + 200.0));
    let out = f.up(Vec2::new(to.x - 150.0, to.y + 200.0));
    assert_eq!(
        out,
        Outcome::Edit(Edit::Disconnect {
            node: NodeId::from("gate"),
            input: "poses".into()
        })
    );
    // Dropping it back where it came from does nothing.
    f.down(to);
    assert_eq!(f.up(to), Outcome::Redraw);
}

#[test]
fn a_wire_that_would_close_a_cycle_is_refused() {
    let mut f = Fixture::new(registry(&[]), &[("a", "pose_gate"), ("b", "pose_gate")]);
    f.doc
        .apply(Edit::Connect {
            from: OutputRef::new("a", "kept"),
            to: NodeId::from("b"),
            input: "poses".into(),
        })
        .unwrap();
    let from = f.port("b", "kept", PortDir::Output);
    let to = f.port("a", "poses", PortDir::Input);
    f.down(from);
    f.drag(to);
    assert_eq!(
        f.state
            .connecting()
            .unwrap()
            .hover
            .as_ref()
            .unwrap()
            .verdict,
        DropVerdict::Refuse(Refusal::Cycle)
    );
    assert!(!matches!(f.up(to), Outcome::Edit(_)));
}

#[test]
fn moving_nodes_inspecting_and_deleting_with_undo() {
    let mut f = Fixture::new(registry(&[]), &[("gate", "pose_gate")]);
    let id = NodeId::from("gate");
    let n = f.model().node(&id).unwrap().clone();

    // The inspect button reports the node and does not start a gesture.
    assert_eq!(f.down(n.inspect.center()), Outcome::Inspect(id.clone()));
    assert!(f.state.is_idle());

    // Dragging the header moves the node (view state only: no edit).
    let grab = Vec2::new(n.header.origin.x + 20.0, n.header.center().y);
    f.down(grab);
    assert!(f.state.is_selected(&id));
    f.drag(grab + Vec2::new(30.0, 40.0));
    assert_eq!(f.up(grab + Vec2::new(30.0, 40.0)), Outcome::Redraw);
    assert_eq!(
        f.state.position(&id).unwrap(),
        Vec2::new(0.0 + 30.0, 100.0 + 40.0)
    );
    assert!(!f.doc.editor().history().can_undo());

    // Delete is an undoable edit; undo puts the node back where it was.
    let edit = f.state.delete_selection(f.doc.graph()).unwrap();
    f.doc.apply(edit).unwrap();
    assert!(f.model().nodes.is_empty());
    f.doc.undo().unwrap();
    assert_eq!(
        f.model().node(&id).unwrap().rect.origin,
        Vec2::new(30.0, 140.0)
    );
}

#[test]
fn evaluation_badges_follow_the_engine() {
    let mut f = Fixture::new(
        registry(&[]),
        &[("poses", "pose_source"), ("gate", "pose_gate")],
    );
    f.doc
        .apply(Edit::Connect {
            from: OutputRef::new("poses", "poses"),
            to: NodeId::from("gate"),
            input: "poses".into(),
        })
        .unwrap();
    let engine = Engine::new(f.doc.registry().clone(), Arc::new(MemoryCache::new()));
    let items = engine
        .generate(f.doc.graph(), &[NodeId::from("gate")])
        .expect("generates");
    *f.doc.eval_mut() = mold_ui_graph::canvas::EvalStates::from_item_graph(&items);
    let model = f.model();
    for n in &model.nodes {
        assert_eq!(n.eval, Some(EvalState::Dirty), "{} never cooked", n.id);
    }

    let cooked = engine
        .cook(f.doc.graph(), &[NodeId::from("gate")])
        .expect("cooks");
    *f.doc.eval_mut() = mold_ui_graph::canvas::EvalStates::from_item_graph(&cooked);
    assert!(
        f.model()
            .nodes
            .iter()
            .all(|n| n.eval == Some(EvalState::Cached))
    );

    // A param change dirties the node (and its downstream), not its upstream.
    f.doc
        .apply(Edit::SetParam {
            node: NodeId::from("gate"),
            name: "max_clashes".into(),
            value: Some(ParamValue::Int(5)),
        })
        .unwrap();
    let model = f.model();
    assert_eq!(
        model.node(&NodeId::from("gate")).unwrap().eval,
        Some(EvalState::Dirty)
    );
    assert_eq!(
        model.node(&NodeId::from("poses")).unwrap().eval,
        Some(EvalState::Cached)
    );
}
