#![allow(dead_code)] // Upstream items we do not use (other handle sides, label point).

//! From [gpui-flow](https://github.com/pacifio/gpui-flow) @ `cfa5787`, MIT (see
//! `LICENSE` here). Only the bezier edge geometry, which is plain `f32` maths.

pub mod bezier;

/// Which side of a node a handle sits on. Upstream: `types::HandlePosition` (the
/// rest of that module is not vendored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandlePosition {
    Top,
    Right,
    Bottom,
    Left,
}
