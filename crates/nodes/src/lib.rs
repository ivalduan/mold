//! The v0.1 node set. Each node is one file implementing the `mold-graph` node
//! trait: declare ports, declare parameters, implement `cook` (or, for a worker node,
//! `prepare`/`collect_event`/`collect_finish`).
//!
//! See `CLAUDE.md` in this crate for what is built and what is not, and
//! `docs/authoring-a-node.md` to add one.
//!
//! # What a host needs
//!
//! ```no_run
//! use std::sync::Arc;
//! use mold_graph::{Engine, RunDirCache};
//!
//! let registry = Arc::new(mold_nodes::registry());
//! let codecs = Arc::new(mold_nodes::codecs());
//! let cache = Arc::new(RunDirCache::open("run", codecs)?);
//! let engine = Engine::new(registry, cache);
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! [`registry`] is enough to load, validate and replay a recipe. To actually run the
//! co-fold node, build it with the machine's worker manifests
//! ([`registry_with_workers`]) and attach `mold_jobs::Executors`.

mod codec;
mod cofold;
mod data;
mod load_ligands;
mod load_structure;
mod pose_gate;
mod profiler;
mod protein;
mod seq;
pub mod table;

use std::sync::Arc;

use mold_graph::{ArenaCodec, CodecRegistry, F64Codec, I64Codec, PortType, Registry, StringCodec};
use mold_jobs::Workers;

pub use codec::{LigandSetCodec, PoseSetCodec, PoseTableCodec};
pub use cofold::CoFold;
pub use data::{Ligand, LigandSet, PoseSet, append_pose, single_pose_arena};
pub use load_ligands::LoadLigands;
pub use load_structure::LoadStructure;
pub use pose_gate::PoseGate;
pub use profiler::InteractionProfiler;
pub use protein::ProteinRule;
pub use seq::sequence;

/// The `kind` string of every node in this build: what a recipe names.
pub mod kinds {
    pub use crate::cofold::KIND as COFOLD;
    pub use crate::load_ligands::KIND as LOAD_LIGANDS;
    pub use crate::load_structure::KIND as LOAD_STRUCTURE;
    pub use crate::pose_gate::KIND as POSE_GATE;
    pub use crate::profiler::KIND as INTERACTION_PROFILER;

    /// Every kind, for a palette or a test.
    pub const ALL: [&str; 5] = [
        LOAD_STRUCTURE,
        LOAD_LIGANDS,
        COFOLD,
        POSE_GATE,
        INTERACTION_PROFILER,
    ];
}

/// Every node kind of this build. The co-fold node reports default [`Needs`] because
/// it has no manifests; use [`registry_with_workers`] to run it.
///
/// [`Needs`]: mold_graph::Needs
pub fn registry() -> Registry {
    build(CoFold::new())
}

/// [`registry`] with the co-fold node built against this machine's worker manifests,
/// so its items declare what the worker needs (`needs_gpu`).
pub fn registry_with_workers(workers: Arc<Workers>) -> Registry {
    build(CoFold::with_workers(workers))
}

fn build(cofold: CoFold) -> Registry {
    let mut r = Registry::new();
    // A spec that breaks the rules of its role is a bug in this crate, not a condition
    // a host can handle; `every_kind_is_registered_with_its_ports_and_params` covers it.
    r.processor(LoadStructure)
        .and_then(|r| r.processor(LoadLigands))
        .and_then(|r| r.processor(cofold))
        .and_then(|r| r.processor(PoseGate))
        .and_then(|r| r.processor(InteractionProfiler))
        .expect("the node specs of this crate are valid");
    r
}

/// A codec per port type, so item results survive in a
/// [`RunDirCache`](mold_graph::RunDirCache). See [`codec`](crate::codec) for the
/// payload conventions; `Msa` has no payload type in this build and no codec.
pub fn codecs() -> CodecRegistry {
    let mut c = CodecRegistry::new();
    c.register(PortType::Structure, ArenaCodec)
        .and_then(|c| c.register(PortType::Sequence, StringCodec))
        .and_then(|c| c.register(PortType::LigandSet, LigandSetCodec))
        .and_then(|c| c.register(PortType::PoseSet, PoseSetCodec))
        .and_then(|c| c.register(PortType::Table, PoseTableCodec))
        .and_then(|c| c.register(PortType::Scalar, I64Codec))
        .and_then(|c| c.register(PortType::Scalar, F64Codec))
        .expect("the codecs of this crate do not conflict");
    c
}

/// A hash of `path`'s contents, for [`Processor::fingerprint`](mold_graph::Processor::fingerprint):
/// a node that reads a file re-cooks when the file changes, which no parameter can see.
/// An unreadable path hashes to nothing — the cook reports that properly.
pub(crate) fn file_fingerprint(path: &str) -> Vec<u8> {
    match std::fs::read(path) {
        Ok(bytes) => blake3::hash(&bytes).as_bytes().to_vec(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_registered_with_its_ports_and_params() {
        let r = registry();
        let kinds: Vec<&str> = r.kinds().map(|s| s.kind.as_str()).collect();
        let mut expected = kinds::ALL.to_vec();
        expected.sort_unstable();
        assert_eq!(kinds, expected, "registered in kind order");
        for kind in kinds::ALL {
            let spec = r.spec(kind).expect(kind);
            assert!(!spec.outputs.is_empty(), "{kind} produces nothing");
            // Every param has a default, so `default_params` is complete and a
            // freshly added node cooks without the user typing anything.
            let defaults = r.default_params(kind).expect(kind);
            assert_eq!(defaults.iter().count(), spec.params.len(), "{kind}");
        }
    }

    #[test]
    fn every_port_type_a_node_uses_has_a_codec() {
        let r = registry();
        let c = codecs();
        // `can_encode` needs a value; the registry's own `Debug` lists the port types.
        let registered = format!("{c:?}");
        for spec in r.kinds() {
            for port in spec.inputs.iter().chain(&spec.outputs) {
                assert!(
                    registered.contains(&format!("{:?}", port.ty)),
                    "{}.{}: {:?} has no codec",
                    spec.kind,
                    port.name,
                    port.ty
                );
            }
        }
    }
}
