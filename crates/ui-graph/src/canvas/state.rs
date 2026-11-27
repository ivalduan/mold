//! The canvas's view state and its interaction state machine.
//!
//! [`CanvasState`] holds only view state: camera, node positions, selection and the
//! gesture in progress. Pointer handlers read the graph and the last laid-out
//! [`CanvasModel`] and return a [`Outcome`]; graph changes come out as
//! `mold_graph::Edit`s for the host to apply through `mold_graph::Editor` (see
//! [`GraphDoc`](super::GraphDoc)). Nothing here mutates the graph.

use std::collections::{BTreeMap, BTreeSet};

use mold_graph::{Edit, Graph, GraphError, NodeData, NodeId, OutputRef, PortType, Registry};

use super::geom::{Camera, Vec2};
use super::layout::{CanvasModel, NodeMetrics, Renderers, metrics, node_height};

/// Why a connection is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// `PortType::accepts` said no.
    TypeMismatch { from: PortType, to: PortType },
    /// The edge would close a cycle (including an output wired to its own node).
    Cycle,
    /// The node or port does not exist (the graph changed under the drag).
    Unknown,
}

/// What dropping the wire on an input would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropVerdict {
    Accept,
    Refuse(Refusal),
}

impl DropVerdict {
    pub fn is_accept(self) -> bool {
        self == Self::Accept
    }
}

/// Would `Edit::Connect { from, to, input }` be accepted? Asks `PortType::accepts`
/// (the same rule `Graph::apply` enforces) and checks for cycles, so the answer is
/// known while the wire is still being dragged. `None` if a node or port named here
/// does not exist.
pub fn check_connection(
    graph: &Graph,
    registry: &Registry,
    from: &OutputRef,
    to: &NodeId,
    input: &str,
) -> Option<DropVerdict> {
    let from_ty = registry
        .spec(&graph.node(&from.node)?.kind)?
        .output_port(&from.port)?
        .ty;
    let to_ty = registry.spec(&graph.node(to)?.kind)?.input_port(input)?.ty;
    if !to_ty.accepts(from_ty) {
        return Some(DropVerdict::Refuse(Refusal::TypeMismatch {
            from: from_ty,
            to: to_ty,
        }));
    }
    if graph.downstream(to).contains(&from.node) {
        return Some(DropVerdict::Refuse(Refusal::Cycle));
    }
    Some(DropVerdict::Accept)
}

/// Pointer buttons the canvas distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    /// Select, move nodes, drag wires, pan on empty canvas.
    Primary,
    /// Pan.
    Secondary,
    /// Pan.
    Middle,
}

/// Modifier keys relevant to the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Shift, Ctrl or Cmd: toggle a node in the selection instead of replacing it.
    pub extend_selection: bool,
}

/// The input a dragged wire is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverTarget {
    pub node: NodeId,
    pub input: String,
    pub verdict: DropVerdict,
}

/// A wire being dragged out of an output (or picked up from a bound input).
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectDrag {
    pub from: OutputRef,
    pub from_ty: PortType,
    /// The edge this drag picked up from its input, if any: (node, input).
    pub lifted: Option<(NodeId, String)>,
    /// Pointer, screen px.
    pub cursor: Vec2,
    /// The input under the pointer, with its verdict.
    pub hover: Option<HoverTarget>,
}

impl ConnectDrag {
    /// The drop verdict for `to.input` (used to mark every input during the drag).
    /// Dropping a lifted edge back where it came from is always accepted.
    pub fn verdict_for(
        &self,
        graph: &Graph,
        registry: &Registry,
        to: &NodeId,
        input: &str,
    ) -> DropVerdict {
        if self.is_lifted_from(to, input) {
            return DropVerdict::Accept;
        }
        check_connection(graph, registry, &self.from, to, input)
            .unwrap_or(DropVerdict::Refuse(Refusal::Unknown))
    }

    fn is_lifted_from(&self, to: &NodeId, input: &str) -> bool {
        self.lifted
            .as_ref()
            .is_some_and(|(n, i)| n == to && i == input)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
enum Gesture {
    #[default]
    Idle,
    Panning {
        last: Vec2,
    },
    Moving {
        last: Vec2,
        nodes: Vec<NodeId>,
    },
    Connecting(ConnectDrag),
}

/// What a pointer event asks the host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Nothing changed.
    None,
    /// View state changed; repaint.
    Redraw,
    /// The "inspect output" button of this node was clicked (principle #3).
    Inspect(NodeId),
    /// Apply this edit through `mold_graph::Editor` (undoable), then repaint.
    Edit(Edit),
}

/// View state of one canvas. Never holds the graph.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanvasState {
    pub camera: Camera,
    /// World position of each node's top-left corner. Kept for removed nodes too, so
    /// that undoing a removal puts the node back where it was.
    positions: BTreeMap<NodeId, Vec2>,
    selection: BTreeSet<NodeId>,
    gesture: Gesture,
}

impl CanvasState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn position(&self, id: &NodeId) -> Option<Vec2> {
        self.positions.get(id).copied()
    }

    /// Place a node (world units). Positions are view state for now; see the
    /// design notes in this crate's `CLAUDE.md` on where they should persist.
    pub fn set_position(&mut self, id: NodeId, world: Vec2) {
        self.positions.insert(id, world);
    }

    pub fn positions(&self) -> &BTreeMap<NodeId, Vec2> {
        &self.positions
    }

    pub fn selection(&self) -> &BTreeSet<NodeId> {
        &self.selection
    }

    pub fn is_selected(&self, id: &NodeId) -> bool {
        self.selection.contains(id)
    }

    pub fn select_only(&mut self, id: NodeId) {
        self.selection.clear();
        self.selection.insert(id);
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// The wire being dragged, if any.
    pub fn connecting(&self) -> Option<&ConnectDrag> {
        match &self.gesture {
            Gesture::Connecting(d) => Some(d),
            _ => None,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.gesture == Gesture::Idle
    }

    /// Give every node of `graph` without a position one: column by dependency depth,
    /// stacked below whatever is already in that column.
    pub fn place_new_nodes<R: NodeMetrics + ?Sized>(
        &mut self,
        graph: &Graph,
        registry: &Registry,
        renderers: &Renderers<R>,
    ) {
        if graph.nodes.keys().all(|id| self.positions.contains_key(id)) {
            return;
        }
        let order = graph
            .topo_order()
            .unwrap_or_else(|_| graph.nodes.keys().cloned().collect());
        let mut depth: BTreeMap<&NodeId, usize> = BTreeMap::new();
        for id in &order {
            let d = graph.nodes[id]
                .inputs
                .values()
                .filter_map(|src| depth.get(&src.node))
                .map(|d| d + 1)
                .max()
                .unwrap_or(0);
            depth.insert(id, d);
        }
        for id in &order {
            if self.positions.contains_key(id) {
                continue;
            }
            let x = metrics::AUTO_GAP + depth[id] as f32 * metrics::AUTO_COLUMN;
            // Below the lowest node already in this column.
            let y = graph
                .nodes
                .iter()
                .filter_map(|(other, data)| {
                    let p = self.positions.get(other)?;
                    ((p.x - x).abs() < 1.0).then(|| {
                        let (r, _) = renderers.resolve(&data.kind);
                        p.y + node_height(registry.spec(&data.kind), data, &**r)
                    })
                })
                .fold(0.0, f32::max)
                + metrics::AUTO_GAP;
            self.positions.insert(id.clone(), Vec2::new(x, y));
        }
    }

    /// Primary press: grab a port, a node or the background. Other buttons pan.
    pub fn pointer_down(
        &mut self,
        model: &CanvasModel,
        graph: &Graph,
        registry: &Registry,
        at: Vec2,
        button: Button,
        mods: Modifiers,
    ) -> Outcome {
        if button != Button::Primary {
            self.gesture = Gesture::Panning { last: at };
            return Outcome::None;
        }
        // Ports first (they overhang the node's edge), topmost node first.
        if let Some((node, port, is_output)) = hit_port(model, at) {
            if is_output {
                let from = OutputRef::new(node.clone(), port.clone());
                let Some(from_ty) = output_type(graph, registry, &from) else {
                    return Outcome::None;
                };
                self.gesture = Gesture::Connecting(ConnectDrag {
                    from,
                    from_ty,
                    lifted: None,
                    cursor: at,
                    hover: None,
                });
                return Outcome::Redraw;
            }
            // A bound input: pick its edge up (drop elsewhere to move it, on empty
            // canvas to disconnect).
            if let Some(src) = graph.node(&node).and_then(|d| d.inputs.get(&port)) {
                let Some(from_ty) = output_type(graph, registry, src) else {
                    return Outcome::None;
                };
                let mut drag = ConnectDrag {
                    from: src.clone(),
                    from_ty,
                    lifted: Some((node, port)),
                    cursor: at,
                    hover: None,
                };
                drag.hover = hover_at(model, graph, registry, &drag, at);
                self.gesture = Gesture::Connecting(drag);
                return Outcome::Redraw;
            }
            return Outcome::None;
        }
        if let Some(n) = model.nodes.iter().rev().find(|n| n.rect.contains(at)) {
            if n.inspect.contains(at) {
                return Outcome::Inspect(n.id.clone());
            }
            if mods.extend_selection {
                if !self.selection.remove(&n.id) {
                    self.selection.insert(n.id.clone());
                }
            } else if !self.selection.contains(&n.id) {
                self.select_only(n.id.clone());
            }
            let nodes = if self.selection.contains(&n.id) {
                self.selection.iter().cloned().collect()
            } else {
                Vec::new()
            };
            self.gesture = Gesture::Moving { last: at, nodes };
            return Outcome::Redraw;
        }
        if !mods.extend_selection {
            self.selection.clear();
        }
        self.gesture = Gesture::Panning { last: at };
        Outcome::Redraw
    }

    /// Pointer moved (with or without a button down).
    pub fn pointer_move(
        &mut self,
        model: &CanvasModel,
        graph: &Graph,
        registry: &Registry,
        at: Vec2,
    ) -> Outcome {
        match &mut self.gesture {
            Gesture::Idle => Outcome::None,
            Gesture::Panning { last } => {
                self.camera.pan = self.camera.pan + (at - *last);
                *last = at;
                Outcome::Redraw
            }
            Gesture::Moving { last, nodes } => {
                let delta = (at - *last) * (1.0 / self.camera.zoom);
                *last = at;
                for id in nodes.iter() {
                    if let Some(p) = self.positions.get_mut(id) {
                        *p = *p + delta;
                    }
                }
                Outcome::Redraw
            }
            Gesture::Connecting(drag) => {
                drag.cursor = at;
                drag.hover = hover_at(model, graph, registry, drag, at);
                Outcome::Redraw
            }
        }
    }

    /// Release: finish the gesture. A wire dropped on an accepted input yields one
    /// `Connect` edit (or `Disconnect` + `Connect` for a moved edge); dropped on a
    /// refused input it yields nothing.
    pub fn pointer_up(
        &mut self,
        model: &CanvasModel,
        graph: &Graph,
        registry: &Registry,
        at: Vec2,
    ) -> Outcome {
        let gesture = std::mem::take(&mut self.gesture);
        let Gesture::Connecting(mut drag) = gesture else {
            return match gesture {
                Gesture::Idle => Outcome::None,
                _ => Outcome::Redraw,
            };
        };
        drag.hover = hover_at(model, graph, registry, &drag, at);
        let connect = |to: &NodeId, input: &str| Edit::Connect {
            from: drag.from.clone(),
            to: to.clone(),
            input: input.to_owned(),
        };
        match (&drag.hover, &drag.lifted) {
            (Some(h), lifted) if h.verdict.is_accept() => match lifted {
                Some((n, i)) if *n == h.node && *i == h.input => Outcome::Redraw,
                Some((n, i)) => Outcome::Edit(Edit::Batch(vec![
                    Edit::Disconnect {
                        node: n.clone(),
                        input: i.clone(),
                    },
                    connect(&h.node, &h.input),
                ])),
                None => Outcome::Edit(connect(&h.node, &h.input)),
            },
            // Refused target: the drag is cancelled, the graph is not touched.
            (Some(_), _) => Outcome::Redraw,
            // Dropped on nothing: a lifted edge is disconnected.
            (None, Some((n, i))) => Outcome::Edit(Edit::Disconnect {
                node: n.clone(),
                input: i.clone(),
            }),
            (None, None) => Outcome::Redraw,
        }
    }

    /// Abandon the gesture in progress (Escape, focus lost).
    pub fn cancel_gesture(&mut self) -> Outcome {
        if std::mem::take(&mut self.gesture) == Gesture::Idle {
            Outcome::None
        } else {
            Outcome::Redraw
        }
    }

    /// Wheel: zoom about the pointer. `steps` > 0 zooms in.
    pub fn zoom(&mut self, at: Vec2, steps: f32) -> Outcome {
        self.camera.zoom_at(at, 1.1f32.powf(steps));
        Outcome::Redraw
    }

    /// An edit adding a `kind` node with a fresh id and default params, placed with
    /// its top-left corner at `at` (screen px). The position is recorded now; the
    /// node is selected once it exists.
    pub fn add_node(
        &mut self,
        graph: &Graph,
        registry: &Registry,
        kind: &str,
        at: Vec2,
    ) -> Result<Edit, GraphError> {
        let params = registry
            .default_params(kind)
            .ok_or_else(|| GraphError::UnknownKind(kind.to_owned()))?;
        let id = graph.fresh_id(kind);
        self.positions.insert(id.clone(), self.camera.to_world(at));
        self.select_only(id.clone());
        Ok(Edit::AddNode {
            id,
            node: NodeData {
                params,
                ..NodeData::new(kind)
            },
        })
    }

    /// An edit removing every selected node that is in `graph`; `None` if none is.
    pub fn delete_selection(&mut self, graph: &Graph) -> Option<Edit> {
        let mut edits: Vec<Edit> = self
            .selection
            .iter()
            .filter(|id| graph.contains(id))
            .map(|id| Edit::RemoveNode { id: id.clone() })
            .collect();
        self.selection.clear();
        match edits.len() {
            0 => None,
            1 => edits.pop(),
            _ => Some(Edit::Batch(edits)),
        }
    }

    /// Drop selection entries for nodes that no longer exist.
    pub fn retain_selection(&mut self, graph: &Graph) {
        self.selection.retain(|id| graph.contains(id));
    }
}

fn output_type(graph: &Graph, registry: &Registry, from: &OutputRef) -> Option<PortType> {
    Some(
        registry
            .spec(&graph.node(&from.node)?.kind)?
            .output_port(&from.port)?
            .ty,
    )
}

/// The port nearest to `at` within the hit radius: (node, port, is_output).
fn hit_port(model: &CanvasModel, at: Vec2) -> Option<(NodeId, String, bool)> {
    let mut best: Option<(f32, (NodeId, String, bool))> = None;
    for n in model.nodes.iter().rev() {
        let ports = n
            .inputs
            .iter()
            .map(|p| (p, false))
            .chain(n.outputs.iter().map(|p| (p, true)));
        for (p, is_output) in ports {
            let d = p.center.distance(at);
            if d <= metrics::PORT_HIT_RADIUS && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, (n.id.clone(), p.name.clone(), is_output)));
            }
        }
    }
    best.map(|(_, hit)| hit)
}

/// The input under `at` during a wire drag: the nearest input port within the hit
/// radius, or else an input row of the node under the pointer whose label row
/// contains it.
fn hover_at(
    model: &CanvasModel,
    graph: &Graph,
    registry: &Registry,
    drag: &ConnectDrag,
    at: Vec2,
) -> Option<HoverTarget> {
    let mut best: Option<(f32, &NodeId, &str)> = None;
    for n in model.nodes.iter().rev() {
        for p in &n.inputs {
            let d = p.center.distance(at);
            let on_row = p.row.contains(at) && p.row.center().x > at.x;
            if (d <= metrics::PORT_HIT_RADIUS || on_row)
                && best.as_ref().is_none_or(|(bd, _, _)| d < *bd)
            {
                best = Some((d, &n.id, &p.name));
            }
        }
    }
    let (_, node, input) = best?;
    Some(HoverTarget {
        node: node.clone(),
        input: input.to_owned(),
        verdict: drag.verdict_for(graph, registry, node, input),
    })
}
