//! The graph data model and its validated edits.
//!
//! Serialized shape (what `recipe` projects to TOML):
//! ```toml
//! [nodes.gate]
//! kind = "pose_gate"
//! params = { max_clashes = 2 }
//! inputs = { poses = { node = "cofold", port = "poses" } }
//! ```

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::PortType;
use crate::node::{NodeSpec, Registry};
use crate::param::{ParamKind, ParamValue, Params};
use crate::policy::{FailurePolicy, OnFailure};

/// Stable node id, unique within a graph. Survives save/load and undo.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// An upstream output: the source end of an edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OutputRef {
    pub node: NodeId,
    pub port: String,
}

impl OutputRef {
    pub fn new(node: impl Into<NodeId>, port: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            port: port.into(),
        }
    }
}

impl From<&str> for OutputRef {
    /// `"cofold.poses"` → node `cofold`, port `poses`. Panics without a dot; test sugar.
    fn from(s: &str) -> Self {
        let (n, p) = s.split_once('.').expect("expected `node.port`");
        Self::new(n, p)
    }
}

/// One node instance: its kind, params and input bindings. The id is the map key in
/// [`Graph::nodes`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeData {
    pub kind: String,
    /// Which executor runs this node (in-process threads, local subprocess, SSH, farm):
    /// a key resolved by [`Engine::add_executor`](crate::Engine::add_executor).
    /// `None` = the graph's default, else the engine's. Not part of any cache key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// Failure policy (ADR-0009 §7): extra attempts after a failure. Default 0.
    /// Like `executor`, the policy fields are not part of any cache key.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: u32,
    /// Failure policy: maximum run time of one attempt, in seconds (finite, > 0).
    /// `None` (default) = no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    /// Failure policy: what a failure means for the rest of the cook. Default `error`.
    #[serde(default, skip_serializing_if = "OnFailure::is_error")]
    pub on_failure: OnFailure,
    #[serde(default, skip_serializing_if = "params_empty")]
    pub params: Params,
    /// Input port name → the upstream output feeding it. An edge lives here, on its
    /// consumer, so one input has at most one source.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, OutputRef>,
}

fn params_empty(p: &Params) -> bool {
    p.0.is_empty()
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl NodeData {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            executor: None,
            retries: 0,
            timeout: None,
            on_failure: OnFailure::Error,
            params: Params::new(),
            inputs: BTreeMap::new(),
        }
    }

    /// The failure policy as executors and the engine use it. A `timeout` that is not
    /// finite and positive (refused by [`Graph::validate`]) counts as none.
    pub fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy {
            retries: self.retries,
            timeout: self
                .timeout
                .filter(|s| s.is_finite() && *s > 0.0)
                .and_then(|s| Duration::try_from_secs_f64(s).ok()),
            on_failure: self.on_failure,
        }
    }
}

/// The whole graph. Ordered maps throughout: serialization and hashing are stable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    /// **Reserved.** Default executor key for every node of this graph (ADR-0010);
    /// a node's own `executor` overrides it. `None` = the engine default. Not part of
    /// any cache key: switching local <-> farm keeps the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    #[serde(default)]
    pub nodes: BTreeMap<NodeId, NodeData>,
}

/// A single reversible graph edit. [`Graph::apply`] returns its inverse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    AddNode {
        id: NodeId,
        node: NodeData,
    },
    /// Also removes every edge out of the node.
    RemoveNode {
        id: NodeId,
    },
    /// `None` removes the explicit value (the default applies).
    SetParam {
        node: NodeId,
        name: String,
        value: Option<ParamValue>,
    },
    /// Binds `to.input` to `from`, replacing any existing binding.
    Connect {
        from: OutputRef,
        to: NodeId,
        input: String,
    },
    Disconnect {
        node: NodeId,
        input: String,
    },
    /// Applied in order, atomically: on failure nothing changes.
    Batch(Vec<Edit>),
}

/// Why an edit, or a loaded graph, was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    UnknownNode(NodeId),
    DuplicateNode(NodeId),
    UnknownKind(String),
    UnknownParam {
        node: NodeId,
        name: String,
    },
    ParamKind {
        node: NodeId,
        name: String,
        expected: ParamKind,
        got: ParamKind,
    },
    UnknownInput {
        node: NodeId,
        port: String,
    },
    UnknownOutput {
        node: NodeId,
        port: String,
    },
    TypeMismatch {
        from: OutputRef,
        from_ty: PortType,
        to: NodeId,
        input: String,
        to_ty: PortType,
    },
    /// The edge would close a cycle (or the graph already has one, through `at`).
    Cycle {
        at: NodeId,
    },
    NotConnected {
        node: NodeId,
        input: String,
    },
    /// A node's `timeout` is not a finite, positive number of seconds.
    InvalidTimeout {
        node: NodeId,
        seconds: f64,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNode(n) => write!(f, "no node `{n}`"),
            Self::DuplicateNode(n) => write!(f, "node `{n}` already exists"),
            Self::UnknownKind(k) => write!(f, "unknown node kind `{k}`"),
            Self::UnknownParam { node, name } => {
                write!(f, "node `{node}` has no parameter `{name}`")
            }
            Self::ParamKind {
                node,
                name,
                expected,
                got,
            } => {
                write!(f, "parameter `{node}.{name}` is {expected:?}, got {got:?}")
            }
            Self::UnknownInput { node, port } => write!(f, "node `{node}` has no input `{port}`"),
            Self::UnknownOutput { node, port } => write!(f, "node `{node}` has no output `{port}`"),
            Self::TypeMismatch {
                from,
                from_ty,
                to,
                input,
                to_ty,
            } => write!(
                f,
                "cannot connect `{}.{}` ({from_ty:?}) to `{to}.{input}` ({to_ty:?})",
                from.node, from.port
            ),
            Self::Cycle { at } => write!(f, "connection would create a cycle through `{at}`"),
            Self::NotConnected { node, input } => write!(f, "`{node}.{input}` is not connected"),
            Self::InvalidTimeout { node, seconds } => write!(
                f,
                "node `{node}`: timeout must be a positive number of seconds, not {seconds}"
            ),
        }
    }
}

impl std::error::Error for GraphError {}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeData> {
        self.nodes.get(id)
    }

    pub fn contains(&self, id: &NodeId) -> bool {
        self.nodes.contains_key(id)
    }

    /// A fresh id `"{kind}-{n}"`, smallest `n >= 1` not in use.
    pub fn fresh_id(&self, kind: &str) -> NodeId {
        (1..)
            .map(|n| NodeId(format!("{kind}-{n}")))
            .find(|id| !self.nodes.contains_key(id))
            .expect("unbounded")
    }

    /// Every (consumer, input) bound to an output of `id`.
    pub fn consumers(&self, id: &NodeId) -> Vec<(NodeId, String, OutputRef)> {
        let mut out = Vec::new();
        for (n, data) in &self.nodes {
            for (input, src) in &data.inputs {
                if &src.node == id {
                    out.push((n.clone(), input.clone(), src.clone()));
                }
            }
        }
        out
    }

    /// `id` and every node reachable downstream of it: what a change at `id` dirties.
    pub fn downstream(&self, id: &NodeId) -> BTreeSet<NodeId> {
        let mut rev: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
        for (n, data) in &self.nodes {
            for src in data.inputs.values() {
                rev.entry(&src.node).or_default().push(n);
            }
        }
        let mut seen = BTreeSet::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if seen.insert(n.clone()) {
                stack.extend(rev.get(n).into_iter().flatten().copied());
            }
        }
        seen
    }

    /// `ids` and every node upstream of them: what pulling `ids` needs.
    pub fn upstream(&self, ids: &[NodeId]) -> BTreeSet<NodeId> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&NodeId> = ids.iter().collect();
        while let Some(n) = stack.pop() {
            if seen.insert(n.clone())
                && let Some(d) = self.nodes.get(n)
            {
                stack.extend(d.inputs.values().map(|s| &s.node));
            }
        }
        seen
    }

    /// Nodes in dependency order (ties broken by id, so the order is deterministic).
    /// Edges to missing nodes are ignored here; [`validate`](Self::validate) reports them.
    pub fn topo_order(&self) -> Result<Vec<NodeId>, GraphError> {
        let mut pending: BTreeMap<&NodeId, usize> = BTreeMap::new();
        let mut rev: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
        for (n, data) in &self.nodes {
            let ups: BTreeSet<&NodeId> = data
                .inputs
                .values()
                .map(|s| &s.node)
                .filter(|u| self.nodes.contains_key(*u))
                .collect();
            pending.insert(n, ups.len());
            for u in ups {
                rev.entry(u).or_default().push(n);
            }
        }
        let mut ready: VecDeque<&NodeId> = pending
            .iter()
            .filter(|(_, c)| **c == 0)
            .map(|(n, _)| *n)
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(n) = ready.pop_front() {
            order.push(n.clone());
            for d in rev.get(n).into_iter().flatten() {
                let c = pending.get_mut(d).expect("known node");
                *c -= 1;
                if *c == 0 {
                    ready.push_back(d);
                }
            }
        }
        if order.len() != self.nodes.len() {
            let at = pending
                .iter()
                .find(|(n, c)| **c > 0 && !order.contains(n))
                .map(|(n, _)| (*n).clone());
            return Err(GraphError::Cycle {
                at: at.expect("a node is left"),
            });
        }
        Ok(order)
    }

    /// Inputs that are declared but not bound, per node. Allowed while editing; such a
    /// node (and everything downstream) cannot be evaluated.
    pub fn unbound_inputs(&self, reg: &Registry) -> Vec<(NodeId, String)> {
        let mut out = Vec::new();
        for (id, data) in &self.nodes {
            if let Some(spec) = reg.spec(&data.kind) {
                for p in &spec.inputs {
                    if !data.inputs.contains_key(&p.name) {
                        out.push((id.clone(), p.name.clone()));
                    }
                }
            }
        }
        out
    }

    /// Check a whole graph (e.g. one just deserialized): kinds known, params and
    /// bindings well typed, no cycles. Unbound inputs are allowed.
    pub fn validate(&self, reg: &Registry) -> Result<(), GraphError> {
        for (id, data) in &self.nodes {
            let spec = spec_of(reg, &data.kind)?;
            for (name, value) in data.params.iter() {
                check_param(id, spec, name, value)?;
            }
            for (input, src) in &data.inputs {
                self.check_edge(reg, src, id, spec, input)?;
            }
            check_policy(id, data)?;
        }
        self.topo_order().map(|_| ())
    }

    /// Apply a validated edit and return its inverse. On error the graph is unchanged.
    pub fn apply(&mut self, reg: &Registry, edit: Edit) -> Result<Edit, GraphError> {
        match edit {
            Edit::AddNode { id, node } => {
                if self.nodes.contains_key(&id) {
                    return Err(GraphError::DuplicateNode(id));
                }
                let spec = spec_of(reg, &node.kind)?;
                for (name, value) in node.params.iter() {
                    check_param(&id, spec, name, value)?;
                }
                check_policy(&id, &node)?;
                for (input, src) in &node.inputs {
                    if src.node == id {
                        return Err(GraphError::Cycle { at: id });
                    }
                    self.check_edge(reg, src, &id, spec, input)?;
                }
                self.nodes.insert(id.clone(), node);
                Ok(Edit::RemoveNode { id })
            }
            Edit::RemoveNode { id } => {
                let consumers = self.consumers(&id);
                let node = self
                    .nodes
                    .remove(&id)
                    .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
                let mut inverse = vec![Edit::AddNode { id, node }];
                for (to, input, from) in consumers {
                    self.nodes
                        .get_mut(&to)
                        .expect("consumer exists")
                        .inputs
                        .remove(&input);
                    inverse.push(Edit::Connect { from, to, input });
                }
                Ok(if inverse.len() == 1 {
                    inverse.pop().expect("one")
                } else {
                    Edit::Batch(inverse)
                })
            }
            Edit::SetParam { node, name, value } => {
                let data = self
                    .nodes
                    .get(&node)
                    .ok_or_else(|| GraphError::UnknownNode(node.clone()))?;
                let spec = spec_of(reg, &data.kind)?;
                match &value {
                    Some(v) => check_param(&node, spec, &name, v)?,
                    None if spec.param_spec(&name).is_none() => {
                        return Err(GraphError::UnknownParam { node, name });
                    }
                    None => {}
                }
                let params = &mut self.nodes.get_mut(&node).expect("checked").params;
                let old = match value {
                    Some(v) => params.set(name.clone(), v),
                    None => params.remove(&name),
                };
                Ok(Edit::SetParam {
                    node,
                    name,
                    value: old,
                })
            }
            Edit::Connect { from, to, input } => {
                let data = self
                    .nodes
                    .get(&to)
                    .ok_or_else(|| GraphError::UnknownNode(to.clone()))?;
                let spec = spec_of(reg, &data.kind)?;
                self.check_edge(reg, &from, &to, spec, &input)?;
                if self.downstream(&to).contains(&from.node) {
                    return Err(GraphError::Cycle { at: to });
                }
                let old = self
                    .nodes
                    .get_mut(&to)
                    .expect("checked")
                    .inputs
                    .insert(input.clone(), from);
                Ok(match old {
                    Some(from) => Edit::Connect { from, to, input },
                    None => Edit::Disconnect { node: to, input },
                })
            }
            Edit::Disconnect { node, input } => {
                let data = self
                    .nodes
                    .get_mut(&node)
                    .ok_or_else(|| GraphError::UnknownNode(node.clone()))?;
                match data.inputs.remove(&input) {
                    Some(from) => Ok(Edit::Connect {
                        from,
                        to: node,
                        input,
                    }),
                    None => Err(GraphError::NotConnected { node, input }),
                }
            }
            Edit::Batch(edits) => {
                let mut inverses = Vec::with_capacity(edits.len());
                for e in edits {
                    match self.apply(reg, e) {
                        Ok(inv) => inverses.push(inv),
                        Err(err) => {
                            for inv in inverses.into_iter().rev() {
                                self.apply(reg, inv)
                                    .expect("inverse of an applied edit applies");
                            }
                            return Err(err);
                        }
                    }
                }
                inverses.reverse();
                Ok(Edit::Batch(inverses))
            }
        }
    }

    /// `from` exists and has that output; `to` declares `input`; the types connect.
    fn check_edge(
        &self,
        reg: &Registry,
        from: &OutputRef,
        to: &NodeId,
        to_spec: &NodeSpec,
        input: &str,
    ) -> Result<(), GraphError> {
        let to_port = to_spec
            .input_port(input)
            .ok_or_else(|| GraphError::UnknownInput {
                node: to.clone(),
                port: input.to_owned(),
            })?;
        let src = self
            .nodes
            .get(&from.node)
            .ok_or_else(|| GraphError::UnknownNode(from.node.clone()))?;
        let from_port = spec_of(reg, &src.kind)?
            .output_port(&from.port)
            .ok_or_else(|| GraphError::UnknownOutput {
                node: from.node.clone(),
                port: from.port.clone(),
            })?;
        if !to_port.ty.accepts(from_port.ty) {
            return Err(GraphError::TypeMismatch {
                from: from.clone(),
                from_ty: from_port.ty,
                to: to.clone(),
                input: input.to_owned(),
                to_ty: to_port.ty,
            });
        }
        Ok(())
    }
}

fn spec_of<'r>(reg: &'r Registry, kind: &str) -> Result<&'r NodeSpec, GraphError> {
    reg.spec(kind)
        .ok_or_else(|| GraphError::UnknownKind(kind.to_owned()))
}

fn check_policy(id: &NodeId, node: &NodeData) -> Result<(), GraphError> {
    match node.timeout {
        Some(s) if !(s.is_finite() && s > 0.0 && Duration::try_from_secs_f64(s).is_ok()) => {
            Err(GraphError::InvalidTimeout {
                node: id.clone(),
                seconds: s,
            })
        }
        _ => Ok(()),
    }
}

fn check_param(
    node: &NodeId,
    spec: &NodeSpec,
    name: &str,
    value: &ParamValue,
) -> Result<(), GraphError> {
    let p = spec
        .param_spec(name)
        .ok_or_else(|| GraphError::UnknownParam {
            node: node.clone(),
            name: name.to_owned(),
        })?;
    if !value.fits(p.kind) {
        return Err(GraphError::ParamKind {
            node: node.clone(),
            name: name.to_owned(),
            expected: p.kind,
            got: value.kind(),
        });
    }
    Ok(())
}
