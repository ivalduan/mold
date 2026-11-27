//! Data panels: the pose table, the scatter plot and the 2D SAR view. All of
//! them read the pose table and publish/subscribe to the shared `Selection`.
//! See `CLAUDE.md` in this crate.
//!
//! - [`bus`]: the selection bus, GPUI-free. The `app` crate owns the one instance.

pub mod bus;

pub use bus::{PanelId, Publication, SelectionBus};
