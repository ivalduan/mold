//! GPUI views (Windows only for now, like `ui-viewport` and `ui-graph`).
//!
//! - [`SelectionHub`]: the selection bus as a GPUI entity. The `app` crate creates
//!   the one instance; panels (and later the viewport) connect to it.
//! - [`PoseTableView`]: GPUI Kit's virtualized table over a
//!   [`PoseTableModel`](crate::table::PoseTableModel), connected to the hub.
//!
//! Nothing here paints popups. If a panel is ever placed over the 3D viewport,
//! its root must be wrapped in `deferred(..)` (ADR-0003).

mod hub;
mod pose_table;

pub use hub::{Connection, SelectionHub};
pub use pose_table::{PoseTableDelegate, PoseTableView};
