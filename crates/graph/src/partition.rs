//! Built-in partitioners. Each is registered per port type, under a kind name the
//! caller picks (`PortType` has no "any" type; see DESIGN.md).

use std::collections::BTreeMap;

use crate::PortType;
use crate::node::{ItemView, NodeError, NodeSpec, Partition, Partitioner};
use crate::param::{ParamSpec, ParamValue, Params};

/// One partition holding every upstream item ("wait for all").
#[derive(Debug, Clone)]
pub struct WaitForAll {
    pub kind: String,
    pub ty: PortType,
}

impl WaitForAll {
    pub fn new(kind: impl Into<String>, ty: PortType) -> Self {
        Self {
            kind: kind.into(),
            ty,
        }
    }
}

impl Partitioner for WaitForAll {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(self.kind.clone())
            .input("in", self.ty)
            .output("out", self.ty)
    }

    fn partition(
        &self,
        _params: &Params,
        items: &[ItemView<'_>],
    ) -> Result<Vec<Partition>, NodeError> {
        Ok(vec![Partition {
            members: (0..items.len()).collect(),
            attrs: Default::default(),
        }])
    }
}

/// One partition per distinct value of attribute `attribute`, in order of first
/// appearance. Items without the attribute are an error.
#[derive(Debug, Clone)]
pub struct PartitionByAttribute {
    pub kind: String,
    pub ty: PortType,
}

impl PartitionByAttribute {
    pub fn new(kind: impl Into<String>, ty: PortType) -> Self {
        Self {
            kind: kind.into(),
            ty,
        }
    }
}

impl Partitioner for PartitionByAttribute {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(self.kind.clone())
            .input("in", self.ty)
            .output("out", self.ty)
            .param(ParamSpec::new("attribute", "").doc("attribute to group by"))
    }

    fn partition(
        &self,
        params: &Params,
        items: &[ItemView<'_>],
    ) -> Result<Vec<Partition>, NodeError> {
        let name = params.str("attribute")?;
        let mut groups: Vec<(ParamValue, Vec<usize>)> = Vec::new();
        for (i, it) in items.iter().enumerate() {
            let v = it
                .attrs
                .get(name)
                .ok_or_else(|| NodeError::msg(format!("{} has no attribute `{name}`", it.id)))?;
            match groups.iter_mut().find(|(g, _)| g == v) {
                Some((_, m)) => m.push(i),
                None => groups.push((v.clone(), vec![i])),
            }
        }
        Ok(groups
            .into_iter()
            .map(|(v, members)| Partition {
                members,
                attrs: BTreeMap::from([(name.to_owned(), v)]),
            })
            .collect())
    }
}
