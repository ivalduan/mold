use std::collections::BTreeSet;

use crate::PoseId;

/// What is selected, application-wide: a set of poses plus an optional atom mask.
///
/// **Contract type.** The plot publishes one when a point is clicked, the table
/// when a row is clicked; the viewport subscribes and re-aims the camera. The
/// broadcast mechanism (the "selection bus") lives in the UI layer; the value
/// lives here so headless code and tests can use it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub poses: BTreeSet<PoseId>,
    /// Arena atom indices within the selected poses. `None` = whole poses.
    /// The exact representation (bitset vs. index list) is owned by the `mol` workstream.
    pub atom_mask: Option<Vec<u32>>,
}

impl Selection {
    pub fn single(pose: PoseId) -> Self {
        Self {
            poses: BTreeSet::from([pose]),
            atom_mask: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.poses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_selection_holds_one_pose() {
        let s = Selection::single(PoseId(7));
        assert_eq!(s.poses.len(), 1);
        assert!(s.atom_mask.is_none());
        assert!(!s.is_empty());
    }
}
