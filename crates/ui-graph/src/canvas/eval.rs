//! Per-node evaluation state shown as a badge on each node.
//!
//! The canvas does not run the engine. The host keeps an [`EvalStates`] map up to
//! date, either by setting node states directly or by feeding it work items:
//! [`EvalStates::observe`] fits `mold_graph::Observer` as is (`Box<dyn FnMut(&WorkItem)>`),
//! and [`EvalStates::from_item_graph`] reads the result of `Engine::generate` (which
//! reports exactly which items are dirty versus cached, without cooking).

use std::collections::{BTreeMap, BTreeSet};

use mold_graph::{Graph, ItemGraph, ItemId, ItemState, NodeId, WorkItem};

/// What a node's badge shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvalState {
    /// Needs (re-)evaluation: never cooked, changed since, or cancelled.
    Dirty,
    /// At least one item is waiting, scheduled or cooking.
    Running,
    /// Every item has a result (cooked in this run or taken from the cache).
    Cached,
    /// At least one item failed.
    Failed,
}

impl EvalState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dirty => "dirty",
            Self::Running => "running",
            Self::Cached => "cached",
            Self::Failed => "failed",
        }
    }

    /// Fold a node's item states into one badge. `None` when the node has no items.
    /// Precedence: failed > running > dirty > cached.
    pub fn from_items(states: impl IntoIterator<Item = ItemState>) -> Option<Self> {
        let mut out: Option<Self> = None;
        for s in states {
            let this = match s {
                ItemState::Failed => Self::Failed,
                ItemState::Waiting | ItemState::Scheduled | ItemState::Cooking => Self::Running,
                ItemState::Uncooked | ItemState::Cancelled => Self::Dirty,
                ItemState::Cooked | ItemState::Cached => Self::Cached,
            };
            out = Some(match out {
                None => this,
                Some(prev) => prev.max_precedence(this),
            });
        }
        out
    }

    fn rank(self) -> u8 {
        match self {
            Self::Cached => 0,
            Self::Dirty => 1,
            Self::Running => 2,
            Self::Failed => 3,
        }
    }

    fn max_precedence(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// Node id → badge, plus the item states it was derived from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvalStates {
    nodes: BTreeMap<NodeId, EvalState>,
    items: BTreeMap<NodeId, BTreeMap<ItemId, ItemState>>,
}

impl EvalStates {
    pub fn new() -> Self {
        Self::default()
    }

    /// The state of every node of a generated (or cooked) item graph.
    pub fn from_item_graph(items: &ItemGraph) -> Self {
        let mut out = Self::new();
        for item in items.all() {
            out.observe(item);
        }
        out
    }

    pub fn get(&self, node: &NodeId) -> Option<EvalState> {
        self.nodes.get(node).copied()
    }

    /// Set a node's badge directly (forgets any item states recorded for it).
    pub fn set(&mut self, node: NodeId, state: EvalState) {
        self.items.remove(&node);
        self.nodes.insert(node, state);
    }

    /// Record one work item's state; the badge of its node is recomputed. Shaped to
    /// be called from a `mold_graph::Observer`.
    pub fn observe(&mut self, item: &WorkItem) {
        let node = item.id.node.clone();
        let items = self.items.entry(node.clone()).or_default();
        items.insert(item.id.clone(), item.state);
        if let Some(s) = EvalState::from_items(items.values().copied()) {
            self.nodes.insert(node, s);
        }
    }

    /// Mark `nodes` and everything downstream of them in `graph` dirty.
    pub fn mark_dirty<'a>(&mut self, graph: &Graph, nodes: impl IntoIterator<Item = &'a NodeId>) {
        let mut all = BTreeSet::new();
        for n in nodes {
            all.extend(graph.downstream(n));
        }
        for n in all {
            self.set(n, EvalState::Dirty);
        }
    }

    /// Forget nodes that are no longer in `graph`.
    pub fn retain_in(&mut self, graph: &Graph) {
        self.nodes.retain(|n, _| graph.contains(n));
        self.items.retain(|n, _| graph.contains(n));
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.items.clear();
    }
}

/// Nodes whose data (kind, params, inputs, executor) differ between two graphs,
/// including nodes present in only one of them.
pub fn changed_nodes(before: &Graph, after: &Graph) -> BTreeSet<NodeId> {
    let mut out = BTreeSet::new();
    for (id, data) in &after.nodes {
        if before.nodes.get(id) != Some(data) {
            out.insert(id.clone());
        }
    }
    for id in before.nodes.keys() {
        if !after.nodes.contains_key(id) {
            out.insert(id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_states_fold_with_failed_first() {
        use ItemState::*;
        assert_eq!(EvalState::from_items([]), None);
        assert_eq!(
            EvalState::from_items([Cooked, Cached]),
            Some(EvalState::Cached)
        );
        assert_eq!(
            EvalState::from_items([Cached, Uncooked]),
            Some(EvalState::Dirty)
        );
        assert_eq!(
            EvalState::from_items([Uncooked, Cooking, Cached]),
            Some(EvalState::Running)
        );
        assert_eq!(
            EvalState::from_items([Cooking, Failed, Cached]),
            Some(EvalState::Failed)
        );
    }
}
