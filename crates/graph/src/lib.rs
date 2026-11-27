//! The graph model, its work-item generator and its scheduler.
//!
//! Two levels (see `DESIGN.md`):
//! - the **node graph** ([`Graph`]): what the user edits and the recipe stores. Nodes
//!   with a stable `id`, a `kind`, a parameter map and input bindings; typed ports;
//!   undoable edits ([`Editor`], [`History`]).
//! - the **work-item graph** ([`ItemGraph`]): generated from the node graph by the
//!   [`Engine`]. Work items are what is cached, scheduled and executed.
//!
//! See `CLAUDE.md` in this crate for scope and acceptance tests.

mod cache;
mod codec;
mod command;
mod engine;
mod exec;
mod hash;
mod history;
mod item;
mod model;
mod node;
mod param;
mod partition;
mod run_dir;
mod value;

pub use cache::{Cache, MemoryCache, Persistence, Unpersisted};
pub use codec::{
    ArenaCodec, Codec, CodecConflict, CodecError, CodecRegistry, Encoded, F64Codec, F64VecCodec,
    I64Codec, I64VecCodec, StringCodec,
};
pub use command::EventSink;
pub use engine::{CookOptions, Engine, EngineError, Observer};
pub use exec::{Executor, Job, JobEvent, JobHandle, JobId, ThreadExecutor};
pub use hash::ContentHash;
pub use history::{Editor, History};
pub use item::{ItemGraph, ItemId, ItemRole, ItemState, WorkItem};
pub use model::{Edit, Graph, GraphError, NodeData, NodeId, OutputRef};
pub use node::{
    CancelToken, CommandContext, CommandJob, CookContext, GenerateContext, GenerateWhen, InputMode,
    Inputs, ItemView, NewItem, NodeError, NodeImpl, NodeKind, NodeSpec, Outputs, Partition,
    Partitioner, PortSpec, Processor, Registry, SpecError,
};
pub use param::{AttrValue, Attributes, ParamKind, ParamSpec, ParamValue, Params};
pub use partition::{PartitionByAttribute, WaitForAll};
pub use run_dir::{CacheEvent, ENTRY_FORMAT_VERSION, ENTRY_MAGIC, EntryError, RunDirCache};
pub use value::Value;

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
