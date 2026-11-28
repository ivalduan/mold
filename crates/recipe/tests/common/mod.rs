//! Test-only stand-ins for the v0.1 nodes (the `nodes` crate is still empty): specs
//! only, enough for `Graph::validate`. They never cook.

use mold_graph::{
    CookContext, NodeError, NodeSpec, Outputs, ParamSpec, PortType, Processor, Registry,
};

pub const FIXTURE: &str = include_str!("../fixtures/egfr_cofold.toml");

struct Stub(NodeSpec);

impl Processor for Stub {
    fn spec(&self) -> NodeSpec {
        self.0.clone()
    }
    fn cook(&self, _: &CookContext<'_>) -> Result<Outputs, NodeError> {
        Err(NodeError::msg("test stub: specs only"))
    }
}

/// The node kinds the fixture uses, with the v0.1 port types.
pub fn registry() -> Registry {
    let specs = [
        NodeSpec::new("load_structure")
            .output("structure", PortType::Structure)
            .param(ParamSpec::new("path", ""))
            .param(ParamSpec::new(
                "chains",
                mold_graph::ParamValue::List(vec![]),
            )),
        NodeSpec::new("msa_query")
            .broadcast_input("structure", PortType::Structure)
            .output("msa", PortType::Msa)
            .param(ParamSpec::new("provider", "colabfold-public"))
            .param(ParamSpec::new("max_sequences", 4096i64)),
        NodeSpec::new("load_ligands")
            .output("ligands", PortType::LigandSet)
            .param(ParamSpec::new("path", "")),
        NodeSpec::new("prepare_ligands")
            .input("ligands", PortType::LigandSet)
            .output("ligands", PortType::LigandSet)
            .param(ParamSpec::new("ph", 7.4))
            .param(ParamSpec::new("protonate", true)),
        NodeSpec::new("cofold")
            .input("ligands", PortType::LigandSet)
            .broadcast_input("receptor", PortType::Structure)
            .broadcast_input("msa", PortType::Msa)
            .output("poses", PortType::PoseSet)
            .param(ParamSpec::new("model", "boltz-2"))
            .param(ParamSpec::new("samples", 5i64))
            .param(ParamSpec::new("seed", 0i64)),
        NodeSpec::new("pose_gate")
            .input("poses", PortType::PoseSet)
            .broadcast_input("receptor", PortType::Structure)
            .output("poses", PortType::PoseSet)
            .output("metrics", PortType::Table)
            .param(ParamSpec::new("max_clashes", 0i64))
            .param(ParamSpec::new("min_contacts", 0i64))
            .param(ParamSpec::new("pocket_radius", 8.0)),
    ];
    let mut reg = Registry::new();
    for s in specs {
        reg.processor(Stub(s)).expect("valid stub spec");
    }
    reg
}

/// Replace exactly one occurrence of `from` in `text`.
pub fn edit_once(text: &str, from: &str, to: &str) -> String {
    assert_eq!(text.matches(from).count(), 1, "`{from}` must occur once");
    text.replacen(from, to, 1)
}
