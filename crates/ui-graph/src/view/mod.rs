//! The GPUI view of the canvas (Windows only for now, like `ui-viewport`).
//!
//! [`GraphCanvas`] paints the [`CanvasModel`] that [`layout`] builds every frame and
//! forwards pointer and keyboard input to [`CanvasState`]. It holds view state and
//! an `Entity<GraphDoc>` owned by the host; edits go through
//! [`GraphDoc::apply`] (= `mold_graph::Editor`), so they are undoable.
//!
//! Shaped after gpui-flow's `FlowGraph` (ADR-0004): nodes are absolutely positioned
//! divs; edges and the dragged wire are painted with `PathBuilder` in `canvas`
//! layers. If this canvas is ever placed over the 3D viewport, its root must be
//! wrapped in `deferred(..)` (ADR-0003); as a dock panel of its own it needs nothing.

mod style;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mold_graph::{Edit, GraphError, NodeId};

use crate::canvas::{
    self as core, CanvasModel, CanvasState, Curve, DropVerdict, GraphDoc, NodeMetrics, NodeView,
    Outcome, PortDir, PortView, Refusal, Renderers, Vec2, layout, metrics,
};

pub use style::{eval_color, port_color};

/// Renders the body of a node (the area below its port rows). The canvas draws the
/// frame, header, evaluation badge, inspect button and ports itself.
///
/// Register one per node kind with [`GraphCanvas::set_renderer`]; kinds without one
/// use [`DefaultNodeRenderer`]. The body's height comes from [`NodeMetrics`], so
/// layout and hit-testing stay headless.
///
/// Interactive elements in a body must call `cx.stop_propagation()` in their
/// mouse-down handler, or the press also grabs the node for moving.
pub trait NodeRenderer: NodeMetrics + 'static {
    fn render_body(
        &self,
        node: &NodeView,
        edits: &EditSink,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}

/// One `name = value` row per parameter.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultNodeRenderer;

impl NodeMetrics for DefaultNodeRenderer {}

impl NodeRenderer for DefaultNodeRenderer {
    fn render_body(
        &self,
        node: &NodeView,
        _: &EditSink,
        _: &mut Window,
        _: &mut App,
    ) -> AnyElement {
        let z = node.zoom;
        div()
            .flex()
            .flex_col()
            .px(px(metrics::PADDING * 1.5 * z))
            .children(node.params.iter().map(|(name, value)| {
                div()
                    .h(px(metrics::ROW_HEIGHT * z))
                    .flex()
                    .items_center()
                    .gap(px(4.0 * z))
                    .text_size(px(11.0 * z))
                    .child(div().text_color(rgb(style::MUTED)).child(name.clone()))
                    .child(div().text_color(rgb(style::TEXT)).child(param_text(value)))
            }))
            .into_any_element()
    }
}

/// A param value as the default renderer shows it.
pub fn param_text(v: &mold_graph::ParamValue) -> String {
    use mold_graph::ParamValue::*;
    match v {
        Bool(b) => b.to_string(),
        Int(i) => i.to_string(),
        Float(f) => format!("{f}"),
        Str(s) => format!("\"{s}\""),
        List(l) => format!("[{} items]", l.len()),
    }
}

/// Hands edits from a node body (e.g. a parameter widget) to the canvas, which
/// applies them through `mold_graph::Editor`.
#[derive(Clone)]
pub struct EditSink {
    canvas: WeakEntity<GraphCanvas>,
}

impl std::fmt::Debug for EditSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditSink").finish_non_exhaustive()
    }
}

impl EditSink {
    /// Apply `edit` as one undoable step. A refused edit is shown in the canvas's
    /// status line and leaves the graph unchanged.
    pub fn apply(&self, edit: Edit, cx: &mut App) {
        let _ = self.canvas.update(cx, |c, cx| c.apply(edit, cx));
    }
}

type InspectFn = Rc<dyn Fn(&NodeId, &mut Window, &mut App)>;

/// The node canvas view.
pub struct GraphCanvas {
    doc: Entity<GraphDoc>,
    state: CanvasState,
    renderers: Renderers<dyn NodeRenderer>,
    on_inspect: Option<InspectFn>,
    /// The last layout: what pointer events are hit-tested against.
    model: Option<CanvasModel>,
    /// Window position of the canvas's top-left corner, from the last prepaint.
    origin: Rc<Cell<Point<Pixels>>>,
    focus: FocusHandle,
    status: Option<SharedString>,
    _doc_changed: Subscription,
}

impl std::fmt::Debug for GraphCanvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphCanvas")
            .field("state", &self.state)
            .field("renderers", &self.renderers)
            .finish_non_exhaustive()
    }
}

impl GraphCanvas {
    /// A canvas over `doc`, which the host owns (and may share with other panels).
    pub fn new(doc: Entity<GraphDoc>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&doc, |_, _, cx| cx.notify());
        Self {
            doc,
            state: CanvasState::new(),
            renderers: Renderers::new(Arc::new(DefaultNodeRenderer)),
            on_inspect: None,
            model: None,
            origin: Rc::new(Cell::new(Point::default())),
            focus: cx.focus_handle(),
            status: None,
            _doc_changed: sub,
        }
    }

    /// Use `renderer` for nodes of `kind`.
    pub fn set_renderer(&mut self, kind: impl Into<String>, renderer: Arc<dyn NodeRenderer>) {
        self.renderers.insert(kind, renderer);
    }

    /// Called when a node's "inspect output" button is pressed (principle #3). The
    /// host opens whatever viewer fits the node's output.
    pub fn on_inspect(&mut self, f: impl Fn(&NodeId, &mut Window, &mut App) + 'static) {
        self.on_inspect = Some(Rc::new(f));
    }

    pub fn doc(&self) -> &Entity<GraphDoc> {
        &self.doc
    }

    pub fn state(&self) -> &CanvasState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut CanvasState {
        &mut self.state
    }

    pub fn status(&self) -> Option<&SharedString> {
        self.status.as_ref()
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Apply one undoable edit through the document's `Editor`.
    pub fn apply(&mut self, edit: Edit, cx: &mut Context<Self>) -> Result<(), GraphError> {
        let result = self.doc.update(cx, |d, cx| {
            let r = d.apply(edit);
            cx.notify();
            r
        });
        self.after_change(result.as_ref().err(), cx);
        result
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let r = self.doc.update(cx, |d, cx| {
            let r = d.undo();
            cx.notify();
            r
        });
        self.after_change(r.as_ref().err(), cx);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let r = self.doc.update(cx, |d, cx| {
            let r = d.redo();
            cx.notify();
            r
        });
        self.after_change(r.as_ref().err(), cx);
    }

    /// Add a `kind` node with default params. `at` = canvas position of its top-left
    /// corner; `None` places it near the top-left of the visible area, offset per
    /// node so new nodes do not stack exactly.
    pub fn add_node(
        &mut self,
        kind: &str,
        at: Option<Vec2>,
        cx: &mut Context<Self>,
    ) -> Result<(), GraphError> {
        let n = self.doc.read(cx).graph().nodes.len() as f32;
        let at = at.unwrap_or(Vec2::new(40.0 + 24.0 * (n % 8.0), 40.0 + 24.0 * (n % 8.0)));
        let doc = self.doc.read(cx);
        let edit = self.state.add_node(doc.graph(), doc.registry(), kind, at)?;
        self.apply(edit, cx)
    }

    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let edit = self.state.delete_selection(self.doc.read(cx).graph());
        if let Some(edit) = edit {
            let _ = self.apply(edit, cx);
        }
    }

    fn after_change(&mut self, err: Option<&GraphError>, cx: &mut Context<Self>) {
        self.status = err.map(|e| format!("refused: {e}").into());
        let graph = self.doc.read(cx).graph().clone();
        self.state.retain_selection(&graph);
        cx.notify();
    }

    fn local(&self, p: Point<Pixels>) -> Vec2 {
        let o = self.origin.get();
        Vec2::new(f32::from(p.x - o.x), f32::from(p.y - o.y))
    }

    fn handle(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        match outcome {
            Outcome::None => {}
            Outcome::Redraw => cx.notify(),
            Outcome::Inspect(id) => {
                if let Some(f) = self.on_inspect.clone() {
                    f(&id, window, cx);
                }
            }
            Outcome::Edit(edit) => {
                let _ = self.apply(edit, cx);
            }
        }
    }

    fn mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        button: core::Button,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let at = self.local(ev.position);
        let mods = core::Modifiers {
            extend_selection: ev.modifiers.shift || ev.modifiers.control || ev.modifiers.platform,
        };
        let doc = self.doc.read(cx);
        let out = self
            .state
            .pointer_down(model, doc.graph(), doc.registry(), at, button, mods);
        self.handle(out, window, cx);
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_idle() {
            return;
        }
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let at = self.local(ev.position);
        let doc = self.doc.read(cx);
        let out = self
            .state
            .pointer_move(model, doc.graph(), doc.registry(), at);
        self.handle(out, window, cx);
    }

    fn mouse_up(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_idle() {
            return;
        }
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let at = self.local(position);
        let doc = self.doc.read(cx);
        let out = self
            .state
            .pointer_up(model, doc.graph(), doc.registry(), at);
        self.handle(out, window, cx);
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let cmd = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "z" if cmd && k.modifiers.shift => self.redo(cx),
            "z" if cmd => self.undo(cx),
            "y" if cmd => self.redo(cx),
            "delete" | "backspace" => self.delete_selection(cx),
            "escape" => {
                let out = self.state.cancel_gesture();
                self.handle(out, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_node(
        &self,
        n: &NodeView,
        sink: &EditSink,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let z = n.zoom;
        let o = n.rect.origin;
        let rel = |p: Vec2| (p.x - o.x, p.y - o.y);
        let (renderer, _) = self.renderers.resolve(&n.kind);
        let body = renderer.render_body(n, sink, window, cx);

        let mut header = div()
            .absolute()
            .left_0()
            .top_0()
            .w(px(n.header.size.x))
            .h(px(n.header.size.y))
            .px(px(metrics::PADDING * z))
            .flex()
            .items_center()
            .gap(px(5.0 * z))
            .rounded_t(px(6.0 * z))
            .bg(rgb(style::HEADER))
            .text_size(px(12.0 * z))
            .child(
                div()
                    .text_color(rgb(style::TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(n.id.to_string()),
            );
        if n.id.as_str() != n.kind && !n.id.as_str().starts_with(&n.kind) {
            header = header.child(div().text_color(rgb(style::MUTED)).child(n.kind.clone()));
        }
        if !n.known_kind {
            header = header.child(div().text_color(rgb(style::REFUSED)).child("unknown kind"));
        }
        if let Some(e) = n.eval {
            header = header.child(
                div()
                    .px(px(5.0 * z))
                    .rounded(px(4.0 * z))
                    .bg(rgb(eval_color(e)))
                    .text_size(px(10.0 * z))
                    .text_color(rgb(style::BADGE_TEXT))
                    .child(e.label()),
            );
        }
        let (ix, iy) = rel(n.inspect.origin);
        let inspect = div()
            .absolute()
            .left(px(ix))
            .top(px(iy))
            .size(px(n.inspect.size.x))
            .rounded(px(4.0 * z))
            .border_1()
            .border_color(rgb(style::MUTED))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.0 * z))
            .text_color(rgb(style::TEXT))
            .child("\u{2192}"); // →, "inspect output"

        let ports = n
            .inputs
            .iter()
            .chain(n.outputs.iter())
            .map(|p| port(p, o, z));

        let (bx, by) = rel(n.body.origin);
        div()
            .absolute()
            .left(px(o.x))
            .top(px(o.y))
            .w(px(n.rect.size.x))
            .h(px(n.rect.size.y))
            .rounded(px(6.0 * z))
            .bg(rgb(style::NODE))
            .border_1()
            .border_color(rgb(if n.selected {
                style::SELECTED
            } else {
                style::BORDER
            }))
            .child(header)
            .child(inspect)
            .children(ports)
            .child(
                div()
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .w(px(n.body.size.x))
                    .h(px(n.body.size.y))
                    .overflow_hidden()
                    .child(body),
            )
            .into_any_element()
    }
}

/// A port: its label row and its dot (round = per-item, square = broadcast).
fn port(p: &PortView, node_origin: Vec2, z: f32) -> AnyElement {
    let r = metrics::PORT_RADIUS * z;
    let (cx, cy) = (p.center.x - node_origin.x, p.center.y - node_origin.y);
    let (ring, label_color) = match p.drop {
        Some(DropVerdict::Accept) => (Some(style::ACCEPTED), style::TEXT),
        Some(DropVerdict::Refuse(_)) => (Some(style::REFUSED), style::DIMMED),
        None => (None, style::TEXT),
    };
    let grow = if ring.is_some() { 1.6 } else { 1.0 };
    let r = r * grow;
    let mut dot = div()
        .absolute()
        .left(px(cx - r))
        .top(px(cy - r))
        .size(px(2.0 * r))
        .bg(rgb(port_color(p.ty)));
    dot = if p.mode == mold_graph::InputMode::Broadcast && p.dir == PortDir::Input {
        dot.rounded(px(1.5 * z))
    } else {
        dot.rounded_full()
    };
    dot = match ring {
        Some(c) => dot.border_2().border_color(rgb(c)),
        None if p.connected => dot.border_1().border_color(rgb(style::TEXT)),
        None => dot.border_1().border_color(rgb(style::NODE)),
    };
    let row_top = p.row.origin.y - node_origin.y;
    let mut label = div()
        .absolute()
        .top(px(row_top))
        .h(px(p.row.size.y))
        .w(px(p.row.size.x * 0.5 - 2.0 * metrics::PADDING * z))
        .flex()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(11.0 * z))
        .text_color(rgb(label_color))
        .child(format!("{} \u{00b7} {:?}", p.name, p.ty));
    label = match p.dir {
        PortDir::Input => label.left(px(2.0 * metrics::PADDING * z)),
        PortDir::Output => label.right(px(2.0 * metrics::PADDING * z)).justify_end(),
    };
    div().child(label).child(dot).into_any_element()
}

fn paint_curve(c: &Curve, origin: Point<Pixels>, width: f32, color: u32, window: &mut Window) {
    let p = |v: Vec2| point(origin.x + px(v.x), origin.y + px(v.y));
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(p(c.from));
    b.cubic_bezier_to(p(c.to), p(c.c1), p(c.c2));
    if let Ok(path) = b.build() {
        window.paint_path(path, rgb(color));
    }
}

fn refusal_text(r: &Refusal) -> String {
    match r {
        Refusal::TypeMismatch { from, to } => format!("refused: {from:?} output -> {to:?} input"),
        Refusal::Cycle => "refused: the connection would create a cycle".into(),
        Refusal::Unknown => "refused: no such port".into(),
    }
}

impl Render for GraphCanvas {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.doc.read(cx);
        let model = layout(
            &mut self.state,
            doc.graph(),
            doc.registry(),
            &self.renderers,
            doc.eval(),
        );
        let sink = EditSink {
            canvas: cx.entity().downgrade(),
        };
        let nodes: Vec<AnyElement> = model
            .nodes
            .iter()
            .map(|n| self.render_node(n, &sink, window, cx))
            .collect();
        let zoom = model.camera.zoom;
        let edges: Vec<(Curve, u32, bool)> = model
            .edges
            .iter()
            .filter(|e| !e.lifted)
            .map(|e| (e.curve, port_color(e.ty), false))
            .collect();
        let wire = model.wire.as_ref().map(|w| {
            let color = match w.verdict {
                Some(DropVerdict::Refuse(_)) => style::REFUSED,
                Some(DropVerdict::Accept) => style::ACCEPTED,
                None => port_color(w.ty),
            };
            (w.curve, color)
        });
        let hint: Option<SharedString> =
            match self.state.connecting().and_then(|d| d.hover.as_ref()) {
                Some(h) => match h.verdict {
                    DropVerdict::Refuse(r) => Some(refusal_text(&r).into()),
                    DropVerdict::Accept => {
                        Some(format!("connect to {}.{}", h.node, h.input).into())
                    }
                },
                None => None,
            };
        let status = hint.or_else(|| self.status.clone());
        self.model = Some(model);

        let origin = self.origin.clone();
        let edge_layer = canvas(
            move |bounds, _, _| origin.set(bounds.origin),
            move |bounds, (), window, _| {
                for (c, color, _) in &edges {
                    paint_curve(c, bounds.origin, 2.0 * zoom.max(0.5), *color, window);
                }
            },
        )
        .absolute()
        .size_full();
        let wire_layer = canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                if let Some((c, color)) = &wire {
                    paint_curve(c, bounds.origin, 2.5 * zoom.max(0.5), *color, window);
                }
            },
        )
        .absolute()
        .size_full();

        div()
            .id("mold-graph-canvas")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(style::BACKGROUND))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev, w, cx| this.mouse_down(ev, core::Button::Primary, w, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev, w, cx| this.mouse_down(ev, core::Button::Secondary, w, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev, w, cx| this.mouse_down(ev, core::Button::Middle, w, cx)),
            )
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, w, cx| this.mouse_up(ev.position, w, cx)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseUpEvent, w, cx| this.mouse_up(ev.position, w, cx)),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, ev: &MouseUpEvent, w, cx| this.mouse_up(ev.position, w, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, w, cx| this.mouse_up(ev.position, w, cx)),
            )
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, w, cx| {
                let d = ev.delta.pixel_delta(w.line_height());
                let at = this.local(ev.position);
                let out = this.state.zoom(at, f32::from(d.y) / 60.0);
                this.handle(out, w, cx);
            }))
            .on_key_down(cx.listener(Self::key_down))
            .child(edge_layer)
            .children(nodes)
            .child(wire_layer)
            .when_some(status, |d, s| {
                d.child(
                    div()
                        .absolute()
                        .left(px(10.0))
                        .bottom(px(8.0))
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(style::HEADER))
                        .text_size(px(12.0))
                        .text_color(rgb(style::TEXT))
                        .child(s),
                )
            })
    }
}
