//! Layout: `&Graph` + `&Registry` + view state → a view model in screen space.
//!
//! The view model ([`CanvasModel`]) is what the GPUI view paints and what pointer
//! events are hit-tested against. It is rebuilt on every frame from the graph; it
//! holds copies of names and types, never a reference into the graph.

use std::collections::BTreeMap;
use std::sync::Arc;

use mold_graph::{
    Graph, InputMode, NodeData, NodeId, NodeSpec, OutputRef, ParamValue, PortType, Registry,
};

use super::eval::{EvalState, EvalStates};
use super::geom::{Camera, Rect, Vec2};
use super::state::{CanvasState, DropVerdict};
use crate::vendor::gpui_flow::HandlePosition;
use crate::vendor::gpui_flow::bezier::get_bezier_path;

/// Node geometry in world units (scaled by the camera's zoom on screen).
pub mod metrics {
    pub const NODE_WIDTH: f32 = 232.0;
    pub const HEADER_HEIGHT: f32 = 28.0;
    pub const ROW_HEIGHT: f32 = 20.0;
    pub const PADDING: f32 = 6.0;
    /// Radius of a port dot.
    pub const PORT_RADIUS: f32 = 5.0;
    /// Side of the square "inspect output" button in the header.
    pub const INSPECT_SIZE: f32 = 18.0;
    /// How close (screen px) the pointer must be to a port to grab or drop on it.
    pub const PORT_HIT_RADIUS: f32 = 10.0;
    /// Bezier curvature for edges (xyflow's default).
    pub const EDGE_CURVATURE: f32 = 0.25;
    /// Spacing of the automatic placement of nodes that have no position yet.
    pub const AUTO_COLUMN: f32 = 260.0;
    pub const AUTO_GAP: f32 = 36.0;
}

use metrics::*;

/// The GPUI-free half of a node renderer: how tall the node's body is. The GPUI
/// half (`view::NodeRenderer`) extends this trait, so layout can be computed and
/// tested without a window.
pub trait NodeMetrics {
    /// Height of the body area, below the port rows, in world units. Default: one
    /// row per parameter (what the default renderer shows).
    fn body_height(&self, spec: &NodeSpec, node: &NodeData) -> f32 {
        let _ = node;
        spec.params.len() as f32 * ROW_HEIGHT
    }
}

/// The default renderer's metrics, for headless use (tests, a layout preview).
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultNodeMetrics;

impl NodeMetrics for DefaultNodeMetrics {}

/// Which renderer drew a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RendererSlot {
    /// No renderer registered for the kind: the default one.
    Default,
    /// A renderer the host registered for this kind.
    Custom,
}

/// Renderers by node kind, with a default for every kind not listed. Generic over
/// the renderer type so the same table serves headless layout (`dyn NodeMetrics`)
/// and the GPUI view (`dyn view::NodeRenderer`).
pub struct Renderers<R: ?Sized> {
    default: Arc<R>,
    by_kind: BTreeMap<String, Arc<R>>,
}

impl<R: ?Sized> Renderers<R> {
    pub fn new(default: Arc<R>) -> Self {
        Self {
            default,
            by_kind: BTreeMap::new(),
        }
    }

    /// Use `renderer` for nodes of `kind`.
    pub fn insert(&mut self, kind: impl Into<String>, renderer: Arc<R>) -> &mut Self {
        self.by_kind.insert(kind.into(), renderer);
        self
    }

    pub fn with(mut self, kind: impl Into<String>, renderer: Arc<R>) -> Self {
        self.insert(kind, renderer);
        self
    }

    pub fn resolve(&self, kind: &str) -> (&Arc<R>, RendererSlot) {
        match self.by_kind.get(kind) {
            Some(r) => (r, RendererSlot::Custom),
            None => (&self.default, RendererSlot::Default),
        }
    }
}

impl<R: ?Sized> Clone for Renderers<R> {
    fn clone(&self) -> Self {
        Self {
            default: self.default.clone(),
            by_kind: self.by_kind.clone(),
        }
    }
}

impl<R: ?Sized> std::fmt::Debug for Renderers<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderers")
            .field("custom_kinds", &self.by_kind.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for Renderers<dyn NodeMetrics> {
    fn default() -> Self {
        Self::new(Arc::new(DefaultNodeMetrics))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortDir {
    Input,
    Output,
}

/// One port as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct PortView {
    pub name: String,
    pub ty: PortType,
    pub dir: PortDir,
    /// `Broadcast` inputs are drawn differently (square) from per-item ones.
    pub mode: InputMode,
    /// Centre of the port dot, screen px.
    pub center: Vec2,
    /// Row of the label, screen px.
    pub row: Rect,
    /// Inputs: bound to an upstream output. Outputs: feeds at least one input.
    pub connected: bool,
    /// While a wire is being dragged: what dropping it here would do. `None` for
    /// ports that are not drop targets (outputs, or no drag in progress).
    pub drop: Option<DropVerdict>,
}

/// One node as drawn. Rectangles are in screen px.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeView {
    pub id: NodeId,
    pub kind: String,
    /// `false` if the registry has no such kind (a graph loaded against another
    /// registry); such a node is drawn without ports.
    pub known_kind: bool,
    pub rect: Rect,
    pub header: Rect,
    pub inspect: Rect,
    /// Area handed to the node's renderer, below the port rows.
    pub body: Rect,
    pub inputs: Vec<PortView>,
    pub outputs: Vec<PortView>,
    /// Resolved params: declared defaults overlaid by the node's explicit values, in
    /// declaration order.
    pub params: Vec<(String, ParamValue)>,
    pub eval: Option<EvalState>,
    pub selected: bool,
    pub renderer: RendererSlot,
    /// The zoom the rectangles were computed at (renderers scale text with it).
    pub zoom: f32,
}

impl NodeView {
    pub fn input(&self, name: &str) -> Option<&PortView> {
        self.inputs.iter().find(|p| p.name == name)
    }

    pub fn output(&self, name: &str) -> Option<&PortView> {
        self.outputs.iter().find(|p| p.name == name)
    }
}

/// A cubic bezier in screen px: start, two control points, end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub from: Vec2,
    pub c1: Vec2,
    pub c2: Vec2,
    pub to: Vec2,
}

impl Curve {
    /// Left-to-right edge from an output (right side) to an input (left side).
    pub fn edge(from: Vec2, to: Vec2) -> Self {
        let p = get_bezier_path(
            from.x,
            from.y,
            HandlePosition::Right,
            to.x,
            to.y,
            HandlePosition::Left,
            EDGE_CURVATURE,
        );
        Self {
            from: Vec2::new(p.source.0, p.source.1),
            c1: Vec2::new(p.source_control.0, p.source_control.1),
            c2: Vec2::new(p.target_control.0, p.target_control.1),
            to: Vec2::new(p.target.0, p.target.1),
        }
    }
}

/// One bound input, as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeView {
    pub from: OutputRef,
    pub to: NodeId,
    pub input: String,
    pub ty: PortType,
    pub curve: Curve,
    /// This edge has been picked up by a drag from its input and is drawn as the
    /// wire instead.
    pub lifted: bool,
}

/// The wire being dragged.
#[derive(Debug, Clone, PartialEq)]
pub struct WireView {
    pub ty: PortType,
    pub curve: Curve,
    /// What dropping now would do; `None` = over no input (drop does nothing, or
    /// disconnects a lifted edge).
    pub verdict: Option<DropVerdict>,
}

/// Everything the canvas draws, in screen space.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasModel {
    pub camera: Camera,
    /// In paint order: selected nodes last (on top).
    pub nodes: Vec<NodeView>,
    pub edges: Vec<EdgeView>,
    pub wire: Option<WireView>,
}

impl CanvasModel {
    pub fn node(&self, id: &NodeId) -> Option<&NodeView> {
        self.nodes.iter().find(|n| &n.id == id)
    }
}

/// Lay out `graph` for drawing. Nodes without a position get one (placed by
/// dependency depth); that is the only change to `state`.
pub fn layout<R: NodeMetrics + ?Sized>(
    state: &mut CanvasState,
    graph: &Graph,
    registry: &Registry,
    renderers: &Renderers<R>,
    eval: &EvalStates,
) -> CanvasModel {
    state.place_new_nodes(graph, registry, renderers);
    let cam = state.camera;
    let drag = state.connecting();

    let mut nodes = Vec::with_capacity(graph.nodes.len());
    for (id, data) in &graph.nodes {
        let pos = state.position(id).unwrap_or_default();
        let spec = registry.spec(&data.kind);
        let (renderer, slot) = renderers.resolve(&data.kind);
        let world = Rect::new(
            pos,
            Vec2::new(NODE_WIDTH, node_height(spec, data, &**renderer)),
        );
        let rect = cam.rect_to_screen(world);
        let z = cam.zoom;
        let header = Rect::new(rect.origin, Vec2::new(rect.size.x, HEADER_HEIGHT * z));
        let inspect = Rect::new(
            Vec2::new(
                rect.max().x - (PADDING + INSPECT_SIZE) * z,
                rect.origin.y + (HEADER_HEIGHT - INSPECT_SIZE) * 0.5 * z,
            ),
            Vec2::new(INSPECT_SIZE * z, INSPECT_SIZE * z),
        );
        let rows = spec.map_or(0, |s| s.inputs.len().max(s.outputs.len()));
        let body_top = rect.origin.y + (HEADER_HEIGHT + rows as f32 * ROW_HEIGHT) * z;
        let body = Rect::new(
            Vec2::new(rect.origin.x, body_top),
            Vec2::new(rect.size.x, rect.max().y - body_top),
        );

        let port_row = |i: usize| {
            Rect::new(
                Vec2::new(
                    rect.origin.x,
                    rect.origin.y + (HEADER_HEIGHT + i as f32 * ROW_HEIGHT) * z,
                ),
                Vec2::new(rect.size.x, ROW_HEIGHT * z),
            )
        };
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        if let Some(spec) = spec {
            for (i, p) in spec.inputs.iter().enumerate() {
                let row = port_row(i);
                let drop = drag.map(|d| d.verdict_for(graph, registry, id, &p.name));
                inputs.push(PortView {
                    name: p.name.clone(),
                    ty: p.ty,
                    dir: PortDir::Input,
                    mode: p.mode,
                    center: Vec2::new(rect.origin.x, row.center().y),
                    row,
                    connected: data.inputs.contains_key(&p.name),
                    drop,
                });
            }
            let consumers = graph.consumers(id);
            for (i, p) in spec.outputs.iter().enumerate() {
                let row = port_row(i);
                outputs.push(PortView {
                    name: p.name.clone(),
                    ty: p.ty,
                    dir: PortDir::Output,
                    mode: InputMode::PerItem,
                    center: Vec2::new(rect.max().x, row.center().y),
                    row,
                    connected: consumers.iter().any(|(_, _, src)| src.port == p.name),
                    drop: None,
                });
            }
        }
        let params = match spec {
            Some(spec) => spec
                .params
                .iter()
                .map(|p| {
                    let v = data.params.get(&p.name).unwrap_or(&p.default).clone();
                    (p.name.clone(), v)
                })
                .collect(),
            None => data
                .params
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        nodes.push(NodeView {
            id: id.clone(),
            kind: data.kind.clone(),
            known_kind: spec.is_some(),
            rect,
            header,
            inspect,
            body,
            inputs,
            outputs,
            params,
            eval: eval.get(id),
            selected: state.is_selected(id),
            renderer: slot,
            zoom: z,
        });
    }
    // Stable sort: selected nodes on top, otherwise id order.
    nodes.sort_by_key(|n| n.selected);

    let lifted = drag.and_then(|d| d.lifted.clone());
    let mut edges = Vec::new();
    for (to, data) in &graph.nodes {
        for (input, from) in &data.inputs {
            let (Some(src), Some(dst)) = (find(&nodes, &from.node), find(&nodes, to)) else {
                continue;
            };
            let (Some(out), Some(inp)) = (src.output(&from.port), dst.input(input)) else {
                continue;
            };
            edges.push(EdgeView {
                from: from.clone(),
                to: to.clone(),
                input: input.clone(),
                ty: out.ty,
                curve: Curve::edge(out.center, inp.center),
                lifted: lifted.as_ref() == Some(&(to.clone(), input.clone())),
            });
        }
    }

    let wire = drag.and_then(|d| {
        let src = find(&nodes, &d.from.node)?.output(&d.from.port)?;
        let end = d
            .hover
            .as_ref()
            .filter(|h| h.verdict.is_accept())
            .and_then(|h| find(&nodes, &h.node)?.input(&h.input))
            .map_or(d.cursor, |p| p.center);
        Some(WireView {
            ty: d.from_ty,
            curve: Curve::edge(src.center, end),
            verdict: d.hover.as_ref().map(|h| h.verdict),
        })
    });

    CanvasModel {
        camera: cam,
        nodes,
        edges,
        wire,
    }
}

fn find<'a>(nodes: &'a [NodeView], id: &NodeId) -> Option<&'a NodeView> {
    nodes.iter().find(|n| &n.id == id)
}

/// Height of a node in world units.
pub fn node_height<R: NodeMetrics + ?Sized>(
    spec: Option<&NodeSpec>,
    data: &NodeData,
    renderer: &R,
) -> f32 {
    match spec {
        Some(spec) => {
            let rows = spec.inputs.len().max(spec.outputs.len()) as f32;
            HEADER_HEIGHT + rows * ROW_HEIGHT + renderer.body_height(spec, data) + PADDING
        }
        None => HEADER_HEIGHT + PADDING,
    }
}
