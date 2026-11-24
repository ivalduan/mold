//! Generation and cooking of the work-item graph.
//!
//! The engine owns the decisions: which items exist (generation), which are ready
//! (per-item dependencies), which are already known (cache). It hands ready items to an
//! [`Executor`] and reacts to completions as they arrive. The loop runs on the
//! caller's thread; no work runs on it except generation.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::cache::Cache;
use crate::exec::{Executor, Job, JobEvent, JobHandle, JobId, ThreadExecutor};
use crate::hash::KeyHasher;
use crate::item::{ItemGraph, ItemId, ItemRole, ItemState, WorkItem};
use crate::model::{Graph, GraphError, NodeId};
use crate::node::{
    CancelToken, GenerateContext, GenerateWhen, ItemView, NewItem, NodeError, NodeImpl, NodeSpec,
    Registry,
};
use crate::param::{Attributes, Params, hash_attrs};

/// Why a cook could not start.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// The graph is invalid (unknown kind, bad edge, cycle).
    Graph(GraphError),
    /// A node needed for the requested targets has an unconnected input.
    UnboundInput { node: NodeId, input: String },
    /// A node names an executor the engine doesn't have.
    UnknownExecutor { node: NodeId, executor: String },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(e) => e.fmt(f),
            Self::UnboundInput { node, input } => {
                write!(f, "input `{node}.{input}` is not connected")
            }
            Self::UnknownExecutor { node, executor } => {
                write!(f, "node `{node}` asks for unknown executor `{executor}`")
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl From<GraphError> for EngineError {
    fn from(e: GraphError) -> Self {
        Self::Graph(e)
    }
}

/// Called on the scheduler thread whenever an item is created or changes state.
pub type Observer<'a> = Box<dyn FnMut(&WorkItem) + 'a>;

/// Per-cook options.
#[derive(Default)]
pub struct CookOptions<'a> {
    /// Cancel the cook from another thread. Also handed to every job.
    pub cancel: CancelToken,
    pub observer: Option<Observer<'a>>,
}

impl fmt::Debug for CookOptions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookOptions")
            .field("cancel", &self.cancel)
            .finish_non_exhaustive()
    }
}

/// Generates and cooks work-item graphs from node graphs.
#[derive(Debug, Clone)]
pub struct Engine {
    registry: Arc<Registry>,
    cache: Arc<dyn Cache>,
    default_executor: Arc<dyn Executor>,
    executors: BTreeMap<String, Arc<dyn Executor>>,
}

impl Engine {
    /// An engine cooking in-process on a [`ThreadExecutor`] of default size.
    pub fn new(registry: Arc<Registry>, cache: Arc<dyn Cache>) -> Self {
        Self {
            registry,
            cache,
            default_executor: Arc::new(ThreadExecutor::with_default_threads()),
            executors: BTreeMap::new(),
        }
    }

    /// Replace the default executor (the one used by nodes that name none).
    pub fn with_executor(mut self, executor: Arc<dyn Executor>) -> Self {
        self.default_executor = executor;
        self
    }

    /// Make `executor` available to nodes whose `executor` field is `key`.
    pub fn add_executor(mut self, key: impl Into<String>, executor: Arc<dyn Executor>) -> Self {
        self.executors.insert(key.into(), executor);
        self
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    pub fn cache(&self) -> &Arc<dyn Cache> {
        &self.cache
    }

    /// Generate every item that can be known without cooking: static items, plus
    /// dynamic ones whose parents are already in the cache. Items found in the cache
    /// are `Cached`; the rest `Uncooked` (that is, dirty). `targets` empty = all nodes.
    pub fn generate(&self, graph: &Graph, targets: &[NodeId]) -> Result<ItemGraph, EngineError> {
        let (tx, _rx) = mpsc::channel();
        let mut run = Run::new(self, graph, targets, CookOptions::default(), false, tx)?;
        run.pump();
        Ok(run.items)
    }

    /// Generate and cook everything `targets` needs. Blocks until every item is
    /// cooked, failed or cancelled. `targets` empty = all nodes.
    pub fn cook(&self, graph: &Graph, targets: &[NodeId]) -> Result<ItemGraph, EngineError> {
        self.cook_with(graph, targets, CookOptions::default())
    }

    pub fn cook_with(
        &self,
        graph: &Graph,
        targets: &[NodeId],
        opts: CookOptions<'_>,
    ) -> Result<ItemGraph, EngineError> {
        let (tx, rx) = mpsc::channel();
        let mut run = Run::new(self, graph, targets, opts, true, tx)?;
        run.cook(&rx);
        Ok(run.items)
    }
}

/// One input binding resolved to node indices in the run.
#[derive(Debug)]
struct Bound {
    port: String,
    up: usize,
    up_port: String,
}

struct NodeRun {
    id: NodeId,
    spec: Arc<NodeSpec>,
    imp: NodeImpl,
    params: Arc<Params>,
    fingerprint: Vec<u8>,
    per_item: Option<Bound>,
    broadcast: Vec<Bound>,
    executor: Arc<dyn Executor>,
    /// Generation finished: no more items will appear.
    done: bool,
    /// Upstream items already handled (`EachUpstreamCooked`).
    processed: BTreeSet<usize>,
}

struct Run<'e, 'o> {
    engine: &'e Engine,
    nodes: Vec<NodeRun>,
    items: ItemGraph,
    item_node: Vec<usize>,
    cooking: bool,
    opts: CookOptions<'o>,
    tx: Sender<(JobId, JobEvent)>,
    in_flight: HashMap<JobId, usize>,
    next_job: u64,
}

impl<'e, 'o> Run<'e, 'o> {
    fn new(
        engine: &'e Engine,
        graph: &Graph,
        targets: &[NodeId],
        opts: CookOptions<'o>,
        cooking: bool,
        tx: Sender<(JobId, JobEvent)>,
    ) -> Result<Self, EngineError> {
        let reg = &engine.registry;
        graph.validate(reg)?;
        for t in targets {
            if !graph.contains(t) {
                return Err(GraphError::UnknownNode(t.clone()).into());
            }
        }
        let scope: BTreeSet<NodeId> = if targets.is_empty() {
            graph.nodes.keys().cloned().collect()
        } else {
            graph.upstream(targets)
        };
        let order: Vec<NodeId> = graph
            .topo_order()?
            .into_iter()
            .filter(|n| scope.contains(n))
            .collect();
        let pos: HashMap<&NodeId, usize> = order.iter().enumerate().map(|(i, n)| (n, i)).collect();

        let mut nodes = Vec::with_capacity(order.len());
        let mut items = ItemGraph::default();
        for id in &order {
            let data = &graph.nodes[id];
            let kind = reg.get(&data.kind).expect("validated");
            let spec = kind.spec.clone();
            let mut per_item = None;
            let mut broadcast = Vec::new();
            for p in &spec.inputs {
                let src = data
                    .inputs
                    .get(&p.name)
                    .ok_or_else(|| EngineError::UnboundInput {
                        node: id.clone(),
                        input: p.name.clone(),
                    })?;
                let b = Bound {
                    port: p.name.clone(),
                    up: pos[&src.node],
                    up_port: src.port.clone(),
                };
                match p.mode {
                    crate::node::InputMode::PerItem => per_item = Some(b),
                    crate::node::InputMode::Broadcast => broadcast.push(b),
                }
            }
            let executor = match data.executor.as_ref().or(graph.executor.as_ref()) {
                None => engine.default_executor.clone(),
                Some(key) => engine.executors.get(key).cloned().ok_or_else(|| {
                    EngineError::UnknownExecutor {
                        node: id.clone(),
                        executor: key.clone(),
                    }
                })?,
            };
            let params = Arc::new(data.params.resolved(&spec.params));
            let fingerprint = match &kind.imp {
                NodeImpl::Processor(p) => p.fingerprint(&params),
                NodeImpl::Partitioner(_) => Vec::new(),
            };
            if let (NodeImpl::Partitioner(_), Some(b)) = (&kind.imp, &per_item) {
                items.partition_src.insert(id.clone(), b.up_port.clone());
            }
            items.by_node.insert(id.clone(), Vec::new());
            nodes.push(NodeRun {
                id: id.clone(),
                spec,
                imp: kind.imp.clone(),
                params,
                fingerprint,
                per_item,
                broadcast,
                executor,
                done: false,
                processed: BTreeSet::new(),
            });
        }
        Ok(Self {
            engine,
            nodes,
            items,
            item_node: Vec::new(),
            cooking,
            opts,
            tx,
            in_flight: HashMap::new(),
            next_job: 0,
        })
    }

    fn notify(&mut self, i: usize) {
        if let Some(obs) = self.opts.observer.as_mut() {
            obs(&self.items.items[i]);
        }
    }

    fn set_state(&mut self, i: usize, state: ItemState, error: Option<NodeError>) {
        let it = &mut self.items.items[i];
        it.state = state;
        if error.is_some() {
            it.error = error;
        }
        self.notify(i);
    }

    /// Generate and submit until nothing changes.
    fn pump(&mut self) {
        loop {
            let mut changed = false;
            for n in 0..self.nodes.len() {
                changed |= self.generate_node(n);
            }
            if self.cooking {
                changed |= self.submit_ready();
            }
            if !changed {
                break;
            }
        }
    }

    fn node_items(&self, n: usize) -> Vec<usize> {
        let mut v = self.items.by_node[&self.nodes[n].id].clone();
        v.sort_by(|&a, &b| {
            self.items.items[a]
                .id
                .index
                .cmp(&self.items.items[b].id.index)
        });
        v
    }

    fn all_terminal(&self, n: usize) -> bool {
        self.items.by_node[&self.nodes[n].id]
            .iter()
            .all(|&i| self.items.items[i].state.is_terminal())
    }

    fn generation_error(&mut self, n: usize, e: NodeError) {
        self.items
            .generation_errors
            .push((self.nodes[n].id.clone(), e));
    }

    /// Run whatever generation step node `n` is ready for. Returns whether anything changed.
    fn generate_node(&mut self, n: usize) -> bool {
        if self.nodes[n].done {
            return false;
        }
        // Broadcast inputs: the upstream node must be fully generated, with one item.
        for b in 0..self.nodes[n].broadcast.len() {
            let up = self.nodes[n].broadcast[b].up;
            if !self.nodes[up].done {
                return false;
            }
            let count = self.items.by_node[&self.nodes[up].id].len();
            if count != 1 {
                let port = self.nodes[n].broadcast[b].port.clone();
                self.generation_error(
                    n,
                    NodeError::msg(format!(
                        "broadcast input `{port}` needs exactly 1 upstream item, has {count}"
                    )),
                );
                self.nodes[n].done = true;
                return true;
            }
        }
        let imp = self.nodes[n].imp.clone();
        match imp {
            NodeImpl::Partitioner(p) => {
                let up = self.nodes[n]
                    .per_item
                    .as_ref()
                    .expect("partitioner has an input")
                    .up;
                if !self.nodes[up].done || !self.all_terminal(up) {
                    return false;
                }
                let ups = self.node_items(up);
                let attrs: Vec<Attributes> = ups.iter().map(|&i| self.items.attrs_of(i)).collect();
                let views: Vec<ItemView<'_>> = ups
                    .iter()
                    .zip(&attrs)
                    .map(|(&i, a)| ItemView {
                        id: &self.items.items[i].id,
                        attrs: a.clone(),
                        outputs: self.items.items[i].result.as_deref(),
                    })
                    .collect();
                let parts = p.partition(&self.nodes[n].params, &views);
                drop(views);
                self.nodes[n].done = true;
                match parts {
                    Err(e) => self.generation_error(n, e),
                    Ok(parts) => {
                        for (pi, part) in parts.into_iter().enumerate() {
                            if let Some(&bad) = part.members.iter().find(|&&m| m >= ups.len()) {
                                self.generation_error(
                                    n,
                                    NodeError::msg(format!("partition member {bad} out of range")),
                                );
                                continue;
                            }
                            let members: Vec<usize> =
                                part.members.iter().map(|&m| ups[m]).collect();
                            let mut merged = agreed(part.members.iter().map(|&m| &attrs[m]));
                            merged.extend(part.attrs.clone());
                            self.add_partition(n, pi as u32, members, &part.attrs, merged);
                        }
                    }
                }
                true
            }
            NodeImpl::Processor(p) => {
                let when = p.generate_when();
                let Some(up) = self.nodes[n].per_item.as_ref().map(|b| b.up) else {
                    // No per-item input: generate once. Dynamic generation waits for the
                    // broadcast items to cook.
                    if when != GenerateWhen::EachUpstreamGenerated {
                        let bs: Vec<usize> = self.nodes[n].broadcast.iter().map(|b| b.up).collect();
                        if !bs.iter().all(|&u| self.all_terminal(u)) {
                            return false;
                        }
                    }
                    self.nodes[n].done = true;
                    self.generate_for(n, None);
                    return true;
                };
                match when {
                    GenerateWhen::AllUpstreamCooked => {
                        if !self.nodes[up].done || !self.all_terminal(up) {
                            return false;
                        }
                        self.nodes[n].done = true;
                        for parent in self.node_items(up) {
                            if self.items.items[parent].state.is_done() {
                                self.generate_for(n, Some(parent));
                            }
                        }
                        true
                    }
                    GenerateWhen::EachUpstreamGenerated | GenerateWhen::EachUpstreamCooked => {
                        // Per parent: as soon as it exists (static) or has finished (dynamic).
                        let static_ = when == GenerateWhen::EachUpstreamGenerated;
                        let mut changed = false;
                        for parent in self.node_items(up) {
                            let st = self.items.items[parent].state;
                            let ready = static_ || st.is_terminal();
                            if ready && self.nodes[n].processed.insert(parent) {
                                changed = true;
                                if static_ || st.is_done() {
                                    self.generate_for(n, Some(parent));
                                }
                            }
                        }
                        let total = self.items.by_node[&self.nodes[up].id].len();
                        if self.nodes[up].done && self.nodes[n].processed.len() == total {
                            self.nodes[n].done = true;
                            changed = true;
                        }
                        changed
                    }
                }
            }
        }
    }

    /// Call the processor's `generate` for one parent (or none) and add its items.
    fn generate_for(&mut self, n: usize, parent: Option<usize>) {
        let NodeImpl::Processor(p) = self.nodes[n].imp.clone() else {
            unreachable!()
        };
        let new = {
            let view = parent.map(|i| ItemView {
                id: &self.items.items[i].id,
                attrs: self.items.attrs_of(i),
                outputs: if self.items.items[i].state.is_done() {
                    self.items.items[i].result.as_deref()
                } else {
                    None
                },
            });
            let ctx = GenerateContext {
                node: &self.nodes[n].id,
                params: &self.nodes[n].params,
                parent: view,
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.generate(&ctx)))
                .unwrap_or_else(|_| Err(NodeError::Panicked("in generate".into())))
        };
        match new {
            Err(e) => self.generation_error(n, e),
            Ok(new) => {
                for (j, ni) in new.into_iter().enumerate() {
                    self.add_item(n, parent, j as u32, ni);
                }
            }
        }
    }

    fn push(&mut self, n: usize, item: WorkItem) -> usize {
        let i = self.items.items.len();
        self.items.items.push(item);
        self.item_node.push(n);
        self.items
            .by_node
            .get_mut(&self.nodes[n].id)
            .expect("node in run")
            .push(i);
        self.notify(i);
        i
    }

    fn add_item(&mut self, n: usize, parent: Option<usize>, j: u32, ni: NewItem) {
        let node = &self.nodes[n];
        let mut index = parent
            .map(|p| self.items.items[p].id.index.clone())
            .unwrap_or_default();
        index.push(j);
        let mut deps = Vec::new();
        if let (Some(b), Some(p)) = (&node.per_item, parent) {
            deps.push((b.port.clone(), p));
        }
        for b in &node.broadcast {
            deps.push((b.port.clone(), self.items.by_node[&self.nodes[b.up].id][0]));
        }
        let mut h = KeyHasher::new("mold-graph/item/v1");
        h.str(&node.spec.kind);
        node.params.hash_into(&mut h);
        h.bytes(&node.fingerprint).bytes(&j.to_le_bytes());
        hash_attrs(&ni.attrs, &mut h);
        for (port, d) in &deps {
            h.str(port).bytes(&self.items.items[*d].key.0);
        }
        let key = h.finish();

        let mut attrs = ni.attrs;
        let (role, state, result, error) = match ni.outputs {
            Some(o) => match o.check(&node.spec) {
                Ok(()) => {
                    attrs.extend(o.attrs.clone());
                    (ItemRole::NoWork, ItemState::Cooked, Some(Arc::new(o)), None)
                }
                Err(e) => (ItemRole::NoWork, ItemState::Failed, None, Some(e)),
            },
            None => match self.engine.cache.get(&key) {
                Some(r) => {
                    attrs.extend(r.attrs.clone());
                    (ItemRole::Work, ItemState::Cached, Some(r), None)
                }
                None => {
                    let st = if self.cooking {
                        ItemState::Waiting
                    } else {
                        ItemState::Uncooked
                    };
                    (ItemRole::Work, st, None, None)
                }
            },
        };
        let item = WorkItem {
            id: ItemId {
                node: node.id.clone(),
                index,
            },
            role,
            key,
            parents: parent
                .map(|p| vec![self.items.items[p].id.clone()])
                .unwrap_or_default(),
            attrs,
            state,
            error,
            result,
            partial: None,
            inherit: parent,
            deps,
            members: Vec::new(),
        };
        self.push(n, item);
    }

    fn add_partition(
        &mut self,
        n: usize,
        p: u32,
        members: Vec<usize>,
        own: &Attributes,
        merged: Attributes,
    ) {
        let node = &self.nodes[n];
        let mut h = KeyHasher::new("mold-graph/partition/v1");
        h.str(&node.spec.kind);
        node.params.hash_into(&mut h);
        h.bytes(&p.to_le_bytes());
        hash_attrs(own, &mut h);
        for &m in &members {
            h.bytes(&self.items.items[m].key.0);
        }
        let ok = members.iter().all(|&m| self.items.items[m].state.is_done());
        let item = WorkItem {
            id: ItemId {
                node: node.id.clone(),
                index: vec![p],
            },
            role: ItemRole::Partition,
            key: h.finish(),
            parents: members
                .iter()
                .map(|&m| self.items.items[m].id.clone())
                .collect(),
            attrs: merged,
            state: if ok {
                ItemState::Cooked
            } else {
                ItemState::Cancelled
            },
            error: if ok { None } else { Some(NodeError::Cancelled) },
            result: None,
            partial: None,
            inherit: None,
            deps: Vec::new(),
            members,
        };
        self.push(n, item);
    }

    /// Submit every waiting item whose dependencies are done; cancel those whose
    /// dependencies failed.
    fn submit_ready(&mut self) -> bool {
        let mut changed = false;
        for i in 0..self.items.items.len() {
            if self.items.items[i].state != ItemState::Waiting {
                continue;
            }
            let deps = &self.items.items[i].deps;
            if deps.iter().any(|(_, d)| {
                matches!(
                    self.items.items[*d].state,
                    ItemState::Failed | ItemState::Cancelled
                )
            }) {
                self.set_state(i, ItemState::Cancelled, Some(NodeError::Cancelled));
                changed = true;
            } else if deps
                .iter()
                .all(|(_, d)| self.items.items[*d].state.is_done())
            {
                self.submit(i);
                changed = true;
            }
        }
        changed
    }

    fn submit(&mut self, i: usize) {
        let n = self.item_node[i];
        let node = &self.nodes[n];
        let NodeImpl::Processor(processor) = node.imp.clone() else {
            unreachable!("only work items are submitted")
        };
        let mut inputs = crate::node::Inputs::default();
        for (port, d) in &self.items.items[i].deps {
            let up_port = node
                .per_item
                .iter()
                .chain(&node.broadcast)
                .find(|b| &b.port == port)
                .map(|b| b.up_port.as_str())
                .expect("dep port is bound");
            let mut vals = Vec::new();
            self.items.gather(*d, up_port, &mut vals);
            inputs.0.insert(port.clone(), vals);
        }
        let id = JobId(self.next_job);
        self.next_job += 1;
        let item = &self.items.items[i];
        let job = Job {
            id,
            item: item.id.clone(),
            key: item.key,
            kind: node.spec.kind.clone(),
            processor,
            params: node.params.clone(),
            attrs: self.items.attrs_of(i),
            inputs,
            cancel: self.opts.cancel.clone(),
        };
        let executor = node.executor.clone();
        self.in_flight.insert(id, i);
        self.set_state(i, ItemState::Scheduled, None);
        executor.submit(job, JobHandle::new(id, self.tx.clone()));
    }

    fn on_event(&mut self, id: JobId, ev: JobEvent) {
        let Some(&i) = self.in_flight.get(&id) else {
            return;
        };
        match ev {
            JobEvent::Started => self.set_state(i, ItemState::Cooking, None),
            JobEvent::Partial(o) => {
                self.items.items[i].partial = Some(o);
                self.notify(i);
            }
            JobEvent::Finished(r) => {
                self.in_flight.remove(&id);
                let spec = self.nodes[self.item_node[i]].spec.clone();
                match r.and_then(|o| o.check(&spec).map(|()| o)) {
                    Ok(o) => {
                        let o = Arc::new(o);
                        self.engine.cache.put(self.items.items[i].key, o.clone());
                        let it = &mut self.items.items[i];
                        it.attrs.extend(o.attrs.clone());
                        it.result = Some(o);
                        self.set_state(i, ItemState::Cooked, None);
                    }
                    Err(NodeError::Cancelled) => {
                        self.set_state(i, ItemState::Cancelled, Some(NodeError::Cancelled))
                    }
                    Err(e) => self.set_state(i, ItemState::Failed, Some(e)),
                }
            }
        }
    }

    fn cook(&mut self, rx: &Receiver<(JobId, JobEvent)>) {
        loop {
            if self.opts.cancel.is_cancelled() {
                self.cancel_all();
                return;
            }
            self.pump();
            if self.in_flight.is_empty() {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok((id, ev)) => {
                    self.on_event(id, ev);
                    while let Ok((id, ev)) = rx.try_recv() {
                        self.on_event(id, ev);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => unreachable!("the run holds a sender"),
            }
        }
        // Anything still waiting can never run (e.g. its generation was cut short).
        for i in 0..self.items.items.len() {
            if !self.items.items[i].state.is_terminal() {
                self.set_state(i, ItemState::Cancelled, Some(NodeError::Cancelled));
            }
        }
    }

    fn cancel_all(&mut self) {
        self.items.cancelled = true;
        let ids: Vec<(JobId, usize)> = self.in_flight.drain().collect();
        for (id, i) in ids {
            self.nodes[self.item_node[i]].executor.cancel(id);
        }
        for i in 0..self.items.items.len() {
            if !self.items.items[i].state.is_terminal() {
                self.set_state(i, ItemState::Cancelled, Some(NodeError::Cancelled));
            }
        }
    }
}

/// Attributes present with the same value in every set.
fn agreed<'a>(mut sets: impl Iterator<Item = &'a Attributes>) -> Attributes {
    let Some(first) = sets.next() else {
        return Attributes::new();
    };
    let mut out = first.clone();
    for s in sets {
        out.retain(|k, v| s.get(k) == Some(v));
    }
    out
}
