//! Data panels: the pose table, the scatter plot and the 2D SAR view. All of
//! them read the pose table and publish/subscribe to the shared `Selection`.
//! See `CLAUDE.md` in this crate.
//!
//! - [`bus`]: the selection bus, GPUI-free. The `app` crate owns the one instance.
//! - [`table`]: the pose table's view model (sort, filter, row ⇄ `PoseId`,
//!   selection ⇄ rows, formatting), GPUI-free.
//! - [`synthetic`]: seeded demo pose tables for examples, tests and benches.

pub mod bus;
pub mod synthetic;
pub mod table;

pub use bus::{PanelId, Publication, SelectionBus};
