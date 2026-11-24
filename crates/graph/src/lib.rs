//! The graph model and its evaluator.
//!
//! Every node has a stable `id`, a `kind` string, a parameter map and an input
//! binding list: the same shape in memory and in the saved recipe, so
//! serialization is a projection rather than a translation.
//!
//! See `CLAUDE.md` in this crate for scope and acceptance tests.

use serde::{Deserialize, Serialize};

/// The type carried by a node port.
///
/// **Contract type.** The editor refuses a connection between mismatched types at
/// drag time, not at evaluation time. Adding a variant is a PR to `main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PortType {
    Structure,
    Sequence,
    LigandSet,
    Msa,
    PoseSet,
    Table,
    Scalar,
}

impl PortType {
    /// v0.1 rule: ports connect only when the types are identical.
    pub fn accepts(self, from: PortType) -> bool {
        self == from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_ports_are_refused() {
        assert!(PortType::PoseSet.accepts(PortType::PoseSet));
        assert!(!PortType::PoseSet.accepts(PortType::Table));
    }
}
