//! Work items: the unit of generation, caching, scheduling and execution.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::hash::ContentHash;
use crate::model::NodeId;
use crate::node::{NodeError, Outputs};
use crate::param::Attributes;
use crate::value::Value;

/// Stable id of a work item: its node plus an index path.
///
/// The path is the parent's path plus this item's position among its parent's
/// children (`[j]` for an item with no parent, `[p]` for partition `p`). So ids do not
/// depend on cook order: `cofold[3.1]` is always the second pose of ligand 3.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId {
    pub node: NodeId,
    pub index: Vec<u32>,
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[", self.node)?;
        for (i, x) in self.index.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{x}")?;
        }
        f.write_str("]")
    }
}

/// Lifecycle of a work item (Houdini's work item states).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemState {
    /// Generated; no cook requested (static generation only).
    Uncooked,
    /// Waiting for its dependencies.
    Waiting,
    /// Handed to an executor.
    Scheduled,
    /// The executor reported that it started.
    Cooking,
    /// Cooked in this run (or did no work).
    Cooked,
    /// Result taken from the cache.
    Cached,
    Failed,
    /// Cancelled, or an item it depends on failed.
    Cancelled,
}

impl ItemState {
    /// Finished, successfully or not.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cooked | Self::Cached | Self::Failed | Self::Cancelled
        )
    }

    /// Finished with a result.
    pub fn is_done(self) -> bool {
        matches!(self, Self::Cooked | Self::Cached)
    }
}

/// What kind of item this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemRole {
    /// Cooked by its processor.
    Work,
    /// Generated with its outputs; does no work.
    NoWork,
    /// A partition: groups its parents; does no work.
    Partition,
}

/// A work item.
#[derive(Debug, Clone)]
pub struct WorkItem {
    pub id: ItemId,
    pub role: ItemRole,
    /// Cache key: hash of node kind, params, fingerprint, generated attributes and the
    /// keys of every item it depends on.
    pub key: ContentHash,
    /// The parent (one for a processor item with a per-item input, every member for a
    /// partition, none for a source item).
    pub parents: Vec<ItemId>,
    /// Attributes set on this item (generated, merged for partitions, or added by its
    /// cook). Use [`ItemGraph::attrs`] for the inherited view.
    pub attrs: Attributes,
    pub state: ItemState,
    pub error: Option<NodeError>,
    /// The result, once `Cooked`/`Cached` (partitions: `None`; their value is their
    /// members' values).
    pub result: Option<Arc<Outputs>>,
    /// The latest partial outputs reported while cooking.
    pub partial: Option<Outputs>,
    pub(crate) inherit: Option<usize>,
    pub(crate) deps: Vec<(String, usize)>,
    pub(crate) members: Vec<usize>,
}

/// The work-item graph produced by generation (and cooking) of a node graph.
#[derive(Debug, Clone, Default)]
pub struct ItemGraph {
    pub(crate) items: Vec<WorkItem>,
    pub(crate) by_node: BTreeMap<NodeId, Vec<usize>>,
    /// Port feeding each partitioner node's input: (upstream node's output port).
    pub(crate) partition_src: BTreeMap<NodeId, String>,
    /// Generation failures, per node. Such a node generates no further items.
    pub generation_errors: Vec<(NodeId, NodeError)>,
    /// Whether the cook was cancelled.
    pub cancelled: bool,
}

impl ItemGraph {
    /// Every item, in generation order.
    pub fn all(&self) -> impl Iterator<Item = &WorkItem> {
        self.items.iter()
    }

    /// The items of `node`, ordered by index.
    pub fn items(&self, node: &NodeId) -> Vec<&WorkItem> {
        let mut v: Vec<&WorkItem> = self
            .by_node
            .get(node)
            .into_iter()
            .flatten()
            .map(|&i| &self.items[i])
            .collect();
        v.sort_by(|a, b| a.id.index.cmp(&b.id.index));
        v
    }

    pub fn item(&self, id: &ItemId) -> Option<&WorkItem> {
        self.find(id).map(|i| &self.items[i])
    }

    fn find(&self, id: &ItemId) -> Option<usize> {
        self.by_node
            .get(&id.node)?
            .iter()
            .copied()
            .find(|&i| self.items[i].id == *id)
    }

    /// Attributes of `id`, inherited ones included (the item's own win).
    pub fn attrs(&self, id: &ItemId) -> Option<Attributes> {
        self.find(id).map(|i| self.attrs_of(i))
    }

    pub(crate) fn attrs_of(&self, mut i: usize) -> Attributes {
        let mut chain = vec![i];
        while let Some(p) = self.items[i].inherit {
            chain.push(p);
            i = p;
        }
        let mut out = Attributes::new();
        for i in chain.into_iter().rev() {
            out.extend(
                self.items[i]
                    .attrs
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
        }
        out
    }

    /// The values `id` provides on `port`: one for a processor item, every member's for a
    /// partition item. Empty if not cooked.
    pub fn values(&self, id: &ItemId, port: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if let Some(i) = self.find(id) {
            self.gather(i, port, &mut out);
        }
        out
    }

    pub(crate) fn gather(&self, i: usize, port: &str, out: &mut Vec<Value>) {
        let it = &self.items[i];
        if it.role == ItemRole::Partition {
            let src = &self.partition_src[&it.id.node];
            for &m in &it.members {
                self.gather(m, src, out);
            }
        } else if let Some(v) = it.result.as_ref().and_then(|r| r.values.get(port)) {
            out.push(v.clone());
        }
    }

    /// Number of items of `node` in `state`.
    pub fn count(&self, node: &NodeId, state: ItemState) -> usize {
        self.by_node
            .get(node)
            .into_iter()
            .flatten()
            .filter(|&&i| self.items[i].state == state)
            .count()
    }
}
