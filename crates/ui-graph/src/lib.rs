//! The node canvas. Renders the `mold-graph` model; never owns it.
//! See `CLAUDE.md` in this crate and ADR-0004.
//!
//! - [`canvas`]: GPUI-free view state, layout, hit-testing and the drag state
//!   machine. Every graph change leaves it as a `mold_graph::Edit`, applied through
//!   `mold_graph::Editor` ([`canvas::GraphDoc`]), so every change is undoable.
//! - `view` (Windows only for now, like `ui-viewport`): the GPUI view that paints
//!   the canvas view model and forwards input to it.
//! - `vendor`: third-party source kept with its licence (gpui-flow's edge geometry).

pub mod canvas;
mod vendor;

#[cfg(windows)]
pub mod view;
