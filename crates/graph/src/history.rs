//! Undo/redo: a command pattern over node-graph edits only. Work items and cached
//! results are derived state; they are regenerated or read from the cache, never undone.

use std::sync::Arc;

use crate::model::{Edit, Graph, GraphError, NodeData, NodeId, OutputRef};
use crate::node::Registry;
use crate::param::ParamValue;

#[derive(Debug, Clone)]
struct Entry {
    forward: Edit,
    inverse: Edit,
}

/// Undo and redo stacks of applied edits.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply `edit` to `graph` and record it. Clears the redo stack.
    pub fn apply(
        &mut self,
        graph: &mut Graph,
        reg: &Registry,
        edit: Edit,
    ) -> Result<(), GraphError> {
        let inverse = graph.apply(reg, edit.clone())?;
        self.undo.push(Entry {
            forward: edit,
            inverse,
        });
        self.redo.clear();
        Ok(())
    }

    /// Undo the last edit. `Ok(false)` if there was nothing to undo.
    pub fn undo(&mut self, graph: &mut Graph, reg: &Registry) -> Result<bool, GraphError> {
        let Some(e) = self.undo.pop() else {
            return Ok(false);
        };
        match graph.apply(reg, e.inverse.clone()) {
            Ok(_) => {
                self.redo.push(e);
                Ok(true)
            }
            Err(err) => {
                self.undo.push(e);
                Err(err)
            }
        }
    }

    /// Redo the last undone edit. `Ok(false)` if there was nothing to redo.
    pub fn redo(&mut self, graph: &mut Graph, reg: &Registry) -> Result<bool, GraphError> {
        let Some(e) = self.redo.pop() else {
            return Ok(false);
        };
        match graph.apply(reg, e.forward.clone()) {
            Ok(inverse) => {
                self.undo.push(Entry {
                    forward: e.forward,
                    inverse,
                });
                Ok(true)
            }
            Err(err) => {
                self.redo.push(e);
                Err(err)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

/// A graph, the registry it is checked against, and its history: the API an editor
/// (the node canvas, a script) drives. Every method is one undoable step.
#[derive(Debug, Clone)]
pub struct Editor {
    graph: Graph,
    registry: Arc<Registry>,
    history: History,
}

impl Editor {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self {
            graph: Graph::new(),
            registry,
            history: History::new(),
        }
    }

    /// Start from an existing graph (e.g. a loaded recipe), validated first.
    pub fn from_graph(graph: Graph, registry: Arc<Registry>) -> Result<Self, GraphError> {
        graph.validate(&registry)?;
        Ok(Self {
            graph,
            registry,
            history: History::new(),
        })
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    pub fn into_graph(self) -> Graph {
        self.graph
    }

    pub fn apply(&mut self, edit: Edit) -> Result<(), GraphError> {
        self.history.apply(&mut self.graph, &self.registry, edit)
    }

    /// Add a `kind` node with a fresh id and every param set to its default (so the
    /// saved recipe is explicit).
    pub fn add_node(&mut self, kind: &str) -> Result<NodeId, GraphError> {
        let id = self.graph.fresh_id(kind);
        self.add_node_with_id(id.clone(), kind)?;
        Ok(id)
    }

    pub fn add_node_with_id(
        &mut self,
        id: impl Into<NodeId>,
        kind: &str,
    ) -> Result<(), GraphError> {
        let params = self
            .registry
            .default_params(kind)
            .ok_or_else(|| GraphError::UnknownKind(kind.to_owned()))?;
        let node = NodeData {
            params,
            ..NodeData::new(kind)
        };
        self.apply(Edit::AddNode {
            id: id.into(),
            node,
        })
    }

    pub fn remove_node(&mut self, id: &NodeId) -> Result<(), GraphError> {
        self.apply(Edit::RemoveNode { id: id.clone() })
    }

    pub fn set_param(
        &mut self,
        node: &NodeId,
        name: &str,
        value: impl Into<ParamValue>,
    ) -> Result<(), GraphError> {
        self.apply(Edit::SetParam {
            node: node.clone(),
            name: name.to_owned(),
            value: Some(value.into()),
        })
    }

    /// Connect `from` (an upstream output) to `to.input`. Refuses mismatched types and
    /// cycles.
    pub fn connect(
        &mut self,
        from: impl Into<OutputRef>,
        to: &NodeId,
        input: &str,
    ) -> Result<(), GraphError> {
        self.apply(Edit::Connect {
            from: from.into(),
            to: to.clone(),
            input: input.to_owned(),
        })
    }

    pub fn disconnect(&mut self, node: &NodeId, input: &str) -> Result<(), GraphError> {
        self.apply(Edit::Disconnect {
            node: node.clone(),
            input: input.to_owned(),
        })
    }

    pub fn undo(&mut self) -> Result<bool, GraphError> {
        self.history.undo(&mut self.graph, &self.registry)
    }

    pub fn redo(&mut self) -> Result<bool, GraphError> {
        self.history.redo(&mut self.graph, &self.registry)
    }

    pub fn history(&self) -> &History {
        &self.history
    }
}
