//! The v0.1 node set. Each node is one file implementing the `mold-graph` node
//! trait: declare ports, declare parameters, implement `cook`.
//!
//! See `CLAUDE.md` in this crate and `docs/authoring-a-node.md`.
//!
//! This module layer is what the nodes are built on: the Rust type behind each
//! [`PortType`] (see [`data`]), the pose table as rows ([`table`]), the codecs that
//! persist them ([`codecs`]), what counts as protein ([`ProteinRule`]) and one-letter
//! sequences ([`sequence`]).

mod codec;
mod data;
mod protein;
mod seq;
pub mod table;

use mold_graph::{ArenaCodec, CodecRegistry, F64Codec, I64Codec, PortType, StringCodec};

pub use codec::{LigandSetCodec, PoseSetCodec, PoseTableCodec};
pub use data::{Ligand, LigandSet, PoseSet, append_pose, single_pose_arena};
pub use protein::ProteinRule;
pub use seq::sequence;

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
