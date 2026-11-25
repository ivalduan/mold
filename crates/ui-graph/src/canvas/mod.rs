//! The GPUI-free canvas: view state, layout, hit-testing and the interaction
//! state machine. Builds and tests on every OS with no window.
//!
//! Flow per frame: [`layout`] turns `&Graph` + `&Registry` + [`CanvasState`] into a
//! [`CanvasModel`] (screen space); the view paints it and feeds pointer events to
//! [`CanvasState`], which answers with an [`Outcome`]; an [`Outcome::Edit`] is
//! applied through [`GraphDoc`] (that is, `mold_graph::Editor`), so it is undoable.

mod doc;
mod eval;
mod geom;
mod layout;
mod state;

pub use doc::GraphDoc;
pub use eval::{EvalState, EvalStates, changed_nodes};
pub use geom::{Camera, Rect, Vec2};
pub use layout::{
    CanvasModel, Curve, DefaultNodeMetrics, EdgeView, NodeMetrics, NodeView, PortDir, PortView,
    RendererSlot, Renderers, WireView, layout, metrics, node_height,
};
pub use state::{
    Button, CanvasState, ConnectDrag, DropVerdict, HoverTarget, Modifiers, Outcome, Refusal,
    check_connection,
};
