//! Data panels: the pose table, the scatter plot and the 2D SAR view. All of
//! them read the pose table and publish/subscribe to the shared `Selection`.
//! See `CLAUDE.md` in this crate.
//!
//! - [`bus`]: the selection bus, GPUI-free. The `app` crate owns the one instance.
//! - [`table`]: the pose table's view model (sort, filter, row ⇄ `PoseId`,
//!   selection ⇄ rows, formatting), GPUI-free.
//! - [`synthetic`]: seeded demo pose tables for examples, tests and benches.
//! - `view` (Windows only for now): the GPUI selection hub and the pose table
//!   view on GPUI Kit's virtualized table.

pub mod bus;
pub mod synthetic;
pub mod table;

#[cfg(windows)]
pub mod view;

pub use bus::{PanelId, Publication, SelectionBus};
