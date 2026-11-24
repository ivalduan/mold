//! The node author's contract.
//!
//! A node kind is either a [`Processor`] (generates work items and cooks them) or a
//! [`Partitioner`] (groups upstream items into one item each; does no work). Both
//! declare their ports and parameters in a [`NodeSpec`].

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::PortType;
use crate::item::ItemId;
use crate::param::{AttrValue, Attributes, ParamSpec, Params};
use crate::value::Value;

/// How an input port relates the node's items to the upstream node's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputMode {
    /// Items are generated per upstream item, which becomes their parent (attributes
    /// are inherited from it). At most one per node.
    PerItem,
    /// The upstream node must have exactly one item; it feeds every item of this node
    /// (a receptor, an MSA). A dependency, not a parent.
    Broadcast,
}

/// A named, typed port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSpec {
    pub name: String,
    pub ty: PortType,
    /// Meaningful on inputs only; outputs are always `PerItem`.
    pub mode: InputMode,
}

/// Ports and parameters of a node kind.
///
/// ```
/// # use mold_graph::{NodeSpec, PortType, ParamSpec};
/// let spec = NodeSpec::new("cofold")
///     .input("ligands", PortType::LigandSet)
///     .broadcast_input("receptor", PortType::Structure)
///     .output("poses", PortType::PoseSet)
///     .param(ParamSpec::new("samples", 5i64).doc("poses per ligand"));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSpec {
    pub kind: String,
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
    pub params: Vec<ParamSpec>,
}

impl NodeSpec {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            params: Vec::new(),
        }
    }

    /// A [`PerItem`](InputMode::PerItem) input.
    pub fn input(mut self, name: impl Into<String>, ty: PortType) -> Self {
        self.inputs.push(PortSpec {
            name: name.into(),
            ty,
            mode: InputMode::PerItem,
        });
        self
    }

    /// A [`Broadcast`](InputMode::Broadcast) input.
    pub fn broadcast_input(mut self, name: impl Into<String>, ty: PortType) -> Self {
        self.inputs.push(PortSpec {
            name: name.into(),
            ty,
            mode: InputMode::Broadcast,
        });
        self
    }

    pub fn output(mut self, name: impl Into<String>, ty: PortType) -> Self {
        self.outputs.push(PortSpec {
            name: name.into(),
            ty,
            mode: InputMode::PerItem,
        });
        self
    }

    pub fn param(mut self, p: ParamSpec) -> Self {
        self.params.push(p);
        self
    }

    pub fn input_port(&self, name: &str) -> Option<&PortSpec> {
        self.inputs.iter().find(|p| p.name == name)
    }

    pub fn output_port(&self, name: &str) -> Option<&PortSpec> {
        self.outputs.iter().find(|p| p.name == name)
    }

    pub fn param_spec(&self, name: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.name == name)
    }

    /// The `PerItem` input, if any.
    pub fn per_item_input(&self) -> Option<&PortSpec> {
        self.inputs.iter().find(|p| p.mode == InputMode::PerItem)
    }
}

/// When a processor generates its items (Houdini's "Generate When").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GenerateWhen {
    /// **Static.** Per upstream item, as soon as that item exists, before it cooks.
    /// `generate` sees the parent's attributes, not its outputs. Behind static nodes
    /// this gives item counts (progress) before cooking; behind a dynamic node items
    /// appear as their parents do, so a slow sibling never holds them back.
    ///
    /// Houdini's "all upstream items generated" is deliberately absent: `generate` runs
    /// per parent, so waiting for all parents would only add latency.
    #[default]
    EachUpstreamGenerated,
    /// **Dynamic.** Once every upstream item has cooked; `generate` sees outputs.
    AllUpstreamCooked,
    /// **Dynamic.** Per upstream item, as soon as that item cooks. Streams.
    EachUpstreamCooked,
}

/// What `generate` sees.
#[derive(Debug)]
pub struct GenerateContext<'a> {
    pub node: &'a crate::NodeId,
    /// Declared params, defaults filled in.
    pub params: &'a Params,
    /// The upstream item this call generates children for; `None` for a node with no
    /// `PerItem` input (it generates once).
    pub parent: Option<ItemView<'a>>,
}

/// A read-only view of an item, for generation and partitioning.
#[derive(Debug, Clone)]
pub struct ItemView<'a> {
    pub id: &'a ItemId,
    /// Attributes including inherited ones.
    pub attrs: Attributes,
    /// `Some` once the item has cooked (always `None` under `EachUpstreamGenerated`).
    pub outputs: Option<&'a Outputs>,
}

/// One item to create, returned by [`Processor::generate`].
#[derive(Debug, Clone, Default)]
pub struct NewItem {
    /// Attributes set on the new item, on top of those inherited from its parent.
    pub attrs: Attributes,
    /// `Some`: the item does no work and is cooked at generation with these outputs.
    /// `None`: the item is cooked by [`Processor::cook`].
    pub outputs: Option<Outputs>,
}

impl NewItem {
    /// An item that will be cooked.
    pub fn work() -> Self {
        Self::default()
    }

    /// An item that does no work: its outputs are known now.
    pub fn done(outputs: Outputs) -> Self {
        Self {
            attrs: Attributes::new(),
            outputs: Some(outputs),
        }
    }

    pub fn attr(mut self, name: impl Into<String>, value: impl Into<AttrValue>) -> Self {
        self.attrs.insert(name.into(), value.into());
        self
    }
}

/// A node that generates work items and cooks them.
///
/// `generate` and `cook` must be pure functions of (params, parent/inputs,
/// [`fingerprint`](Self::fingerprint)): results are cached per item under a hash of
/// exactly those.
pub trait Processor: Send + Sync + 'static {
    /// Ports and parameters. Called once at registration.
    fn spec(&self) -> NodeSpec;

    fn generate_when(&self) -> GenerateWhen {
        GenerateWhen::EachUpstreamGenerated
    }

    /// Items for one parent (or the only call, with no `PerItem` input).
    /// Default: exactly one work item, i.e. a 1:1 processor.
    fn generate(&self, _ctx: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
        Ok(vec![NewItem::work()])
    }

    /// Cook one item: compute every declared output.
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError>;

    /// Extra bytes mixed into every item key of this node: state outside params and
    /// inputs that results depend on (a source file's content hash, an implementation
    /// version). Called once per cook, on the scheduler thread; keep it cheap.
    fn fingerprint(&self, _params: &Params) -> Vec<u8> {
        Vec::new()
    }
}

/// One group made by a [`Partitioner`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Partition {
    /// Indices into the `items` slice given to `partition`. In output order.
    pub members: Vec<usize>,
    /// Set on the partition item, over the attributes all members agree on.
    pub attrs: Attributes,
}

/// A node that fans in: it waits until every upstream item has cooked, then groups
/// them; each group becomes one item that does no work. Downstream, an input fed by a
/// partition item receives every member's value ([`Inputs::all`]).
///
/// Spec: exactly one `PerItem` input and one output, of the same type.
pub trait Partitioner: Send + Sync + 'static {
    fn spec(&self) -> NodeSpec;

    /// `items` in index order. Items that failed have `outputs: None`.
    fn partition(
        &self,
        params: &Params,
        items: &[ItemView<'_>],
    ) -> Result<Vec<Partition>, NodeError>;
}

/// What `cook` sees.
pub struct CookContext<'a> {
    pub item: &'a ItemId,
    /// Declared params, defaults filled in.
    pub params: &'a Params,
    /// Attributes including inherited ones.
    pub attrs: &'a Attributes,
    pub inputs: &'a Inputs,
    /// Long-running cooks should poll this and return [`NodeError::Cancelled`].
    pub cancel: &'a CancelToken,
    pub(crate) partial: Option<&'a dyn Fn(Outputs)>,
}

impl fmt::Debug for CookContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookContext")
            .field("item", self.item)
            .finish_non_exhaustive()
    }
}

impl CookContext<'_> {
    /// The single payload on input `name`, downcast to `T`.
    pub fn input<T: Any>(&self, name: &str) -> Result<&T, NodeError> {
        self.inputs.one_as(name)
    }

    /// Report outputs so far (e.g. the first poses of a co-fold). Shown to observers;
    /// downstream items still wait for the final result.
    pub fn partial(&self, outputs: Outputs) {
        if let Some(f) = self.partial {
            f(outputs);
        }
    }
}

/// Input values by port name. One value per port, except where the port is fed by a
/// partition item: then one per member.
#[derive(Debug, Clone, Default)]
pub struct Inputs(pub BTreeMap<String, Vec<Value>>);

impl Inputs {
    pub fn all(&self, name: &str) -> Result<&[Value], NodeError> {
        self.0
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| NodeError::msg(format!("no input `{name}`")))
    }

    pub fn one(&self, name: &str) -> Result<&Value, NodeError> {
        match self.all(name)? {
            [v] => Ok(v),
            vs => Err(NodeError::msg(format!(
                "input `{name}` has {} values, expected 1",
                vs.len()
            ))),
        }
    }

    pub fn one_as<T: Any>(&self, name: &str) -> Result<&T, NodeError> {
        downcast(self.one(name)?, name)
    }

    pub fn all_as<T: Any>(&self, name: &str) -> Result<Vec<&T>, NodeError> {
        self.all(name)?.iter().map(|v| downcast(v, name)).collect()
    }
}

fn downcast<'v, T: Any>(v: &'v Value, name: &str) -> Result<&'v T, NodeError> {
    v.downcast_ref::<T>().ok_or_else(|| {
        NodeError::msg(format!(
            "input `{name}` is not a {}",
            std::any::type_name::<T>()
        ))
    })
}

/// What cooking an item produced: a value per declared output port, attributes to add
/// to the item, and files written (artifacts, for out-of-process executors and the UI).
#[derive(Debug, Clone, Default)]
pub struct Outputs {
    pub values: BTreeMap<String, Value>,
    pub attrs: Attributes,
    pub files: Vec<PathBuf>,
}

impl Outputs {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Outputs::new().with("table", Value::new(PortType::Table, t))`.
    pub fn with(mut self, port: impl Into<String>, value: Value) -> Self {
        self.values.insert(port.into(), value);
        self
    }

    pub fn attr(mut self, name: impl Into<String>, value: impl Into<AttrValue>) -> Self {
        self.attrs.insert(name.into(), value.into());
        self
    }

    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.push(path.into());
        self
    }

    pub fn get(&self, port: &str) -> Option<&Value> {
        self.values.get(port)
    }

    /// Check port names and types against the declaration.
    pub(crate) fn check(&self, spec: &NodeSpec) -> Result<(), NodeError> {
        for p in &spec.outputs {
            match self.values.get(&p.name) {
                None => {
                    return Err(NodeError::InvalidOutput(format!(
                        "output `{}` not set",
                        p.name
                    )));
                }
                Some(v) if v.ty() != p.ty => {
                    return Err(NodeError::InvalidOutput(format!(
                        "output `{}` is {:?}, declared {:?}",
                        p.name,
                        v.ty(),
                        p.ty
                    )));
                }
                Some(_) => {}
            }
        }
        match self.values.keys().find(|k| spec.output_port(k).is_none()) {
            Some(extra) => Err(NodeError::InvalidOutput(format!(
                "undeclared output `{extra}`"
            ))),
            None => Ok(()),
        }
    }
}

/// Why an item (or a generation step) produced no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeError {
    /// The node's own failure, in its words.
    Failed(String),
    /// Stopped by a [`CancelToken`], or an upstream item failed.
    Cancelled,
    /// The node panicked; the message if it was a string.
    Panicked(String),
    /// Outputs don't match the declaration.
    InvalidOutput(String),
    /// The executor dropped the job without completing it (crashed worker, lost host).
    Lost,
}

impl NodeError {
    pub fn msg(m: impl Into<String>) -> Self {
        Self::Failed(m.into())
    }
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(m) => write!(f, "{m}"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Panicked(m) => write!(f, "panicked: {m}"),
            Self::InvalidOutput(m) => write!(f, "invalid output: {m}"),
            Self::Lost => write!(f, "job lost by its executor"),
        }
    }
}

impl std::error::Error for NodeError {}

/// A shared cancellation flag. Clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// A registered node kind's implementation.
#[derive(Clone)]
pub enum NodeImpl {
    Processor(Arc<dyn Processor>),
    Partitioner(Arc<dyn Partitioner>),
}

impl fmt::Debug for NodeImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Processor(_) => "Processor",
            Self::Partitioner(_) => "Partitioner",
        })
    }
}

/// A registered kind: implementation plus its spec, computed once.
#[derive(Clone)]
pub struct NodeKind {
    pub imp: NodeImpl,
    pub spec: Arc<NodeSpec>,
}

impl fmt::Debug for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let role = match self.imp {
            NodeImpl::Processor(_) => "processor",
            NodeImpl::Partitioner(_) => "partitioner",
        };
        write!(f, "NodeKind({}, {role})", self.spec.kind)
    }
}

/// A node spec that breaks the rules of its role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError(pub String);

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid node spec: {}", self.0)
    }
}

impl std::error::Error for SpecError {}

/// The node kinds available to a graph, by `kind` string.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    kinds: BTreeMap<String, NodeKind>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a processor under its spec's kind, replacing any previous one.
    pub fn processor(&mut self, p: impl Processor) -> Result<&mut Self, SpecError> {
        let spec = p.spec();
        check_common(&spec)?;
        if spec
            .inputs
            .iter()
            .filter(|i| i.mode == InputMode::PerItem)
            .count()
            > 1
        {
            return Err(SpecError(format!(
                "`{}`: more than one per-item input",
                spec.kind
            )));
        }
        Ok(self.insert(NodeImpl::Processor(Arc::new(p)), spec))
    }

    /// Register a partitioner under its spec's kind, replacing any previous one.
    pub fn partitioner(&mut self, p: impl Partitioner) -> Result<&mut Self, SpecError> {
        let spec = p.spec();
        check_common(&spec)?;
        let ok = matches!(
            (spec.inputs.as_slice(), spec.outputs.as_slice()),
            ([i], [o]) if i.mode == InputMode::PerItem && i.ty == o.ty
        );
        if !ok {
            return Err(SpecError(format!(
                "`{}`: a partitioner has one per-item input and one output of the same type",
                spec.kind
            )));
        }
        Ok(self.insert(NodeImpl::Partitioner(Arc::new(p)), spec))
    }

    fn insert(&mut self, imp: NodeImpl, spec: NodeSpec) -> &mut Self {
        let spec = Arc::new(spec);
        self.kinds.insert(spec.kind.clone(), NodeKind { imp, spec });
        self
    }

    pub fn get(&self, kind: &str) -> Option<&NodeKind> {
        self.kinds.get(kind)
    }

    pub fn spec(&self, kind: &str) -> Option<&NodeSpec> {
        self.kinds.get(kind).map(|k| &*k.spec)
    }

    /// Specs in kind order, for a node palette.
    pub fn kinds(&self) -> impl Iterator<Item = &NodeSpec> {
        self.kinds.values().map(|k| &*k.spec)
    }

    /// Default values of `kind`'s params, for a freshly added node.
    pub fn default_params(&self, kind: &str) -> Option<Params> {
        let spec = self.spec(kind)?;
        Some(Params(
            spec.params
                .iter()
                .map(|p| (p.name.clone(), p.default.clone()))
                .collect(),
        ))
    }
}

fn check_common(spec: &NodeSpec) -> Result<(), SpecError> {
    let dup = |ports: &[PortSpec]| {
        ports
            .iter()
            .enumerate()
            .any(|(i, p)| ports[..i].iter().any(|q| q.name == p.name))
    };
    if dup(&spec.inputs) || dup(&spec.outputs) {
        return Err(SpecError(format!("`{}`: duplicate port name", spec.kind)));
    }
    if spec
        .params
        .iter()
        .enumerate()
        .any(|(i, p)| spec.params[..i].iter().any(|q| q.name == p.name))
    {
        return Err(SpecError(format!(
            "`{}`: duplicate parameter name",
            spec.kind
        )));
    }
    if let Some(p) = spec.params.iter().find(|p| !p.default.fits(p.kind)) {
        return Err(SpecError(format!(
            "`{}`: default of `{}` is not {:?}",
            spec.kind, p.name, p.kind
        )));
    }
    Ok(())
}
