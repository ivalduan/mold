//! The node canvas on fake node kinds, for trying it by hand.
//!
//! ```text
//! cargo run -p mold-ui-graph --example canvas
//! ```
//!
//! Five fake kinds with different port types (`Structure`, `LigandSet`, `PoseSet`,
//! `Table`, `Scalar`). Drag from an output dot to an input dot to connect; inputs
//! that would refuse the wire turn red while you drag. Drag a bound input's dot to
//! move or (dropped on empty canvas) remove its edge. Ctrl+Z / Ctrl+Y undo and
//! redo, Delete removes the selected nodes, wheel zooms, drag the background (or
//! right/middle drag) pans. "Evaluate" cooks the graph on a background thread
//! through `mold_graph::Engine` and feeds the badges through its `Observer`.
//! `pose_gate` uses a custom body renderer (- / + on `max_clashes`); its cook fails
//! on purpose when `max_clashes` > 5. The "→" button in a node header is the
//! inspect-output hook: here it only prints.

#[cfg(not(windows))]
fn main() {
    eprintln!("The canvas view is built on Windows only for now (like ui-viewport, ADR-0003).");
}

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(windows)]
mod app {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use gpui_kit::*;
    use mold_graph::{
        CookContext, CookOptions, Edit, Editor, Engine, GenerateContext, MemoryCache, NewItem,
        NodeError, NodeId, NodeSpec, OutputRef, Outputs, ParamSpec, ParamValue, PortType,
        Processor, Registry, Value, WorkItem,
    };
    use mold_ui_graph::canvas::{GraphDoc, NodeMetrics, NodeView, metrics};
    use mold_ui_graph::view::{EditSink, GraphCanvas, NodeRenderer};

    /// A fake node kind: a spec, a delay per cooked item, unit payloads.
    struct Fake {
        spec: NodeSpec,
        delay: Duration,
    }

    impl Processor for Fake {
        fn spec(&self) -> NodeSpec {
            self.spec.clone()
        }

        fn generate(&self, ctx: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
            let n = match ctx.params.get("count") {
                Some(ParamValue::Int(n)) => (*n).clamp(1, 64) as usize,
                _ => 1,
            };
            Ok((0..n).map(|_| NewItem::work()).collect())
        }

        fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
            std::thread::sleep(self.delay);
            if let Some(ParamValue::Int(n)) = ctx.params.get("max_clashes")
                && *n > 5
            {
                return Err(NodeError::msg("max_clashes > 5 fails on purpose"));
            }
            Ok(self.spec.outputs.iter().fold(Outputs::new(), |o, p| {
                o.with(p.name.clone(), Value::new(p.ty, ()))
            }))
        }
    }

    fn registry() -> Arc<Registry> {
        let ms = Duration::from_millis;
        let kinds = [
            (
                NodeSpec::new("load_receptor")
                    .output("receptor", PortType::Structure)
                    .param(ParamSpec::new("path", "receptor.pdb")),
                ms(150),
            ),
            (
                NodeSpec::new("load_ligands")
                    .output("ligands", PortType::LigandSet)
                    .param(ParamSpec::new("count", 4i64)),
                ms(100),
            ),
            (
                NodeSpec::new("cofold")
                    .input("ligands", PortType::LigandSet)
                    .broadcast_input("receptor", PortType::Structure)
                    .output("poses", PortType::PoseSet)
                    .param(ParamSpec::new("samples", 3i64)),
                ms(600),
            ),
            (
                NodeSpec::new("pose_gate")
                    .input("poses", PortType::PoseSet)
                    .output("kept", PortType::PoseSet)
                    .output("report", PortType::Table)
                    .param(ParamSpec::new("max_clashes", 2i64)),
                ms(200),
            ),
            (
                NodeSpec::new("summarize")
                    .input("table", PortType::Table)
                    .output("score", PortType::Scalar),
                ms(200),
            ),
        ];
        let mut reg = Registry::new();
        for (spec, delay) in kinds {
            reg.processor(Fake { spec, delay }).expect("valid spec");
        }
        Arc::new(reg)
    }

    /// A start graph: inputs of `cofold` wired, `pose_gate` and `summarize` not.
    fn seed(reg: Arc<Registry>) -> Editor {
        let mut ed = Editor::new(reg);
        for (id, kind) in [
            ("receptor", "load_receptor"),
            ("ligands", "load_ligands"),
            ("cofold", "cofold"),
            ("gate", "pose_gate"),
            ("summary", "summarize"),
        ] {
            ed.add_node_with_id(id, kind).expect("seed node");
        }
        ed.connect(
            OutputRef::new("ligands", "ligands"),
            &"cofold".into(),
            "ligands",
        )
        .expect("seed edge");
        ed.connect(
            OutputRef::new("receptor", "receptor"),
            &"cofold".into(),
            "receptor",
        )
        .expect("seed edge");
        // The seed is the starting point, not an undo step.
        Editor::from_graph(ed.graph().clone(), ed.registry().clone()).expect("valid")
    }

    /// Custom body for `pose_gate`: `max_clashes` with - / + buttons, each one
    /// undoable `SetParam` edit.
    struct GateBody;

    impl NodeMetrics for GateBody {
        fn body_height(&self, _: &NodeSpec, _: &mold_graph::NodeData) -> f32 {
            metrics::ROW_HEIGHT * 1.4
        }
    }

    impl NodeRenderer for GateBody {
        fn render_body(
            &self,
            node: &NodeView,
            edits: &EditSink,
            _: &mut Window,
            _: &mut App,
        ) -> AnyElement {
            let z = node.zoom;
            let value = node
                .params
                .iter()
                .find_map(|(k, v)| match v {
                    ParamValue::Int(i) if k == "max_clashes" => Some(*i),
                    _ => None,
                })
                .unwrap_or(0);
            let step = |label: &'static str, delta: i64| {
                let edits = edits.clone();
                let node = node.id.clone();
                div()
                    .w(px(18.0 * z))
                    .h(px(16.0 * z))
                    .rounded(px(3.0 * z))
                    .bg(rgb(0x4a5160))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(label)
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        // Do not let the press also grab the node.
                        cx.stop_propagation();
                        edits.apply(
                            Edit::SetParam {
                                node: node.clone(),
                                name: "max_clashes".into(),
                                value: Some(ParamValue::Int((value + delta).max(0))),
                            },
                            cx,
                        );
                    })
            };
            div()
                .h_full()
                .px(px(metrics::PADDING * 1.5 * z))
                .flex()
                .items_center()
                .gap(px(6.0 * z))
                .text_size(px(11.0 * z))
                .text_color(rgb(0xe6e9ef))
                .child(div().text_color(rgb(0x9aa3b2)).child("max_clashes"))
                .child(step("-", -1))
                .child(value.to_string())
                .child(step("+", 1))
                .child(div().text_color(rgb(0x9aa3b2)).child(if value > 5 {
                    "(will fail)"
                } else {
                    ""
                }))
                .into_any_element()
        }
    }

    enum Msg {
        Item(Box<WorkItem>),
        Done(Result<usize, String>),
    }

    struct Demo {
        doc: Entity<GraphDoc>,
        canvas: Entity<GraphCanvas>,
        engine: Engine,
        log: SharedString,
        cooking: bool,
    }

    impl Demo {
        fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
            let reg = registry();
            let engine = Engine::new(reg.clone(), Arc::new(MemoryCache::new()));
            let doc = cx.new(|_| GraphDoc::new(seed(reg)));
            cx.new(|cx| {
                let weak = cx.entity().downgrade();
                let canvas = cx.new(|cx| {
                    let mut c = GraphCanvas::new(doc.clone(), cx);
                    c.set_renderer("pose_gate", Arc::new(GateBody));
                    c.on_inspect(move |id, _, cx| {
                        println!("inspect output of `{id}` (hook only; the viewer is not this PR)");
                        let _ = weak.update(cx, |d: &mut Demo, cx| {
                            d.log = format!("inspect output of `{id}` requested").into();
                            cx.notify();
                        });
                    });
                    c
                });
                let focus = canvas.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                cx.observe(&canvas, |_, _, cx| cx.notify()).detach();
                Self {
                    doc,
                    canvas,
                    engine,
                    log: "drag from an output dot to an input dot".into(),
                    cooking: false,
                }
            })
        }

        /// Cook every node whose inputs (and upstream) are all bound, on a
        /// background thread; badges follow the engine's observer.
        fn evaluate(&mut self, cx: &mut Context<Self>) {
            if self.cooking {
                return;
            }
            let doc = self.doc.read(cx);
            let graph = doc.graph().clone();
            let unbound: Vec<NodeId> = graph
                .unbound_inputs(doc.registry())
                .into_iter()
                .map(|(n, _)| n)
                .collect();
            let targets: Vec<NodeId> = graph
                .nodes
                .keys()
                .filter(|n| {
                    !graph
                        .upstream(std::slice::from_ref(*n))
                        .iter()
                        .any(|u| unbound.contains(u))
                })
                .cloned()
                .collect();
            if targets.is_empty() {
                self.log = "nothing to evaluate: every node has an unbound input upstream".into();
                cx.notify();
                return;
            }
            self.cooking = true;
            self.log = format!("evaluating {} node(s)...", targets.len()).into();
            let (tx, rx) = mpsc::channel();
            let engine = self.engine.clone();
            std::thread::spawn(move || {
                let items = tx.clone();
                let opts = CookOptions {
                    observer: Some(Box::new(move |item: &WorkItem| {
                        let _ = items.send(Msg::Item(Box::new(item.clone())));
                    })),
                    ..Default::default()
                };
                let result = engine
                    .cook_with(&graph, &targets, opts)
                    .map(|g| g.all().count())
                    .map_err(|e| e.to_string());
                let _ = tx.send(Msg::Done(result));
            });
            cx.spawn(async move |this, cx| {
                let mut rx = rx;
                loop {
                    let (back, msg) = cx
                        .background_spawn(async move {
                            let msg = rx.recv();
                            (rx, msg)
                        })
                        .await;
                    rx = back;
                    let Ok(msg) = msg else { break };
                    let done = matches!(msg, Msg::Done(_));
                    let alive = this.update(cx, |this, cx| this.on_msg(msg, cx)).is_ok();
                    if done || !alive {
                        break;
                    }
                }
            })
            .detach();
            cx.notify();
        }

        fn on_msg(&mut self, msg: Msg, cx: &mut Context<Self>) {
            match msg {
                Msg::Item(item) => self.doc.update(cx, |d, cx| {
                    d.eval_mut().observe(&item);
                    cx.notify();
                }),
                Msg::Done(r) => {
                    self.cooking = false;
                    self.log = match r {
                        Ok(n) => format!("evaluated: {n} work items").into(),
                        Err(e) => format!("evaluation failed: {e}").into(),
                    };
                    cx.notify();
                }
            }
        }
    }

    fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
        div()
            .id(id.into())
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(rgb(0x343a45))
            .hover(|s| s.bg(rgb(0x4a5160)))
            .cursor_pointer()
            .text_size(px(12.0))
            .child(label.into())
    }

    impl Render for Demo {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let kinds: Vec<String> = self
                .doc
                .read(cx)
                .registry()
                .kinds()
                .map(|s| s.kind.clone())
                .collect();
            let doc = self.doc.read(cx);
            let (can_undo, can_redo) = (
                doc.editor().history().can_undo(),
                doc.editor().history().can_redo(),
            );
            let canvas_status = self.canvas.read(cx).status().cloned();
            let toolbar = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .p(px(8.0))
                .bg(rgb(0x252932))
                .text_color(rgb(0xe6e9ef))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(0x9aa3b2))
                        .child("add:"),
                )
                .children(kinds.into_iter().map(|k| {
                    let kind = k.clone();
                    button(SharedString::from(format!("add-{k}")), k).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let r = this.canvas.update(cx, |c, cx| c.add_node(&kind, None, cx));
                            if let Err(e) = r {
                                this.log = format!("{e}").into();
                            }
                        },
                    ))
                }))
                .child(div().w(px(12.0)))
                .child(
                    button("undo", if can_undo { "Undo" } else { "Undo (empty)" }).on_click(
                        cx.listener(|this, _, _, cx| this.canvas.update(cx, |c, cx| c.undo(cx))),
                    ),
                )
                .child(
                    button("redo", if can_redo { "Redo" } else { "Redo (empty)" }).on_click(
                        cx.listener(|this, _, _, cx| this.canvas.update(cx, |c, cx| c.redo(cx))),
                    ),
                )
                .child(button("delete", "Delete selected").on_click(cx.listener(
                    |this, _, _, cx| this.canvas.update(cx, |c, cx| c.delete_selection(cx)),
                )))
                .child(
                    button(
                        "eval",
                        if self.cooking {
                            "Evaluating..."
                        } else {
                            "Evaluate"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.evaluate(cx))),
                );
            let footer = div()
                .px(px(8.0))
                .py(px(4.0))
                .bg(rgb(0x252932))
                .text_size(px(12.0))
                .text_color(rgb(0x9aa3b2))
                .child(match canvas_status {
                    Some(s) => format!("{}  |  {s}", self.log),
                    None => self.log.to_string(),
                });
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(0x1b1e24))
                .child(toolbar)
                .child(div().flex_1().min_h_0().child(self.canvas.clone()))
                .child(footer)
        }
    }

    pub fn run() {
        application().run(|cx: &mut App| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Mold · node canvas (WS-09)".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(error) = gpui_kit::open_window(options, cx, Demo::new) {
                eprintln!("failed to open the window: {error:#}");
                std::process::exit(2);
            }
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    }
}
