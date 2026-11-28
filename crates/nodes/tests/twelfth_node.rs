//! The node author's contract: a twelfth node is **one file under 100 lines** — this
//! one, test included. `docs/authoring-a-node.md` walks through exactly this code.

mod common;

use std::sync::Arc;

use mold_graph::{
    CookContext, Editor, Engine, MemoryCache, NodeError, NodeId, NodeSpec, Outputs, ParamSpec,
    PortType, Processor, Value,
};
use mold_nodes::{PoseSet, table};

/// Counts the poses of a set, optionally only the ones the gate passed.
struct PoseCount;

impl Processor for PoseCount {
    // 1. Ports: a type per input and output. The editor refuses a mismatched edge.
    // 2. Parameters: these appear in the recipe and the inspector automatically.
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("pose_count")
            .input("poses", PortType::PoseSet)
            .output("count", PortType::Scalar)
            .param(ParamSpec::new("passing_only", false).doc("count only poses that passed"))
    }

    // 3. Cook: a pure function of (params, inputs). Its result is cached per item.
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let set: &PoseSet = ctx.input("poses")?;
        let rows = table::from_frame(&set.table).map_err(NodeError::msg)?;
        let n = match ctx.params.bool("passing_only")? {
            true => rows.iter().filter(|r| r.gate_pass == Some(true)).count(),
            false => rows.len(),
        };
        Ok(Outputs::new().with("count", Value::new(PortType::Scalar, n as i64)))
    }
}

/// Cook `test_poses -> pose_count` with `passing_only` set, and read the count.
fn count(passing_only: bool) -> i64 {
    // The node set, the node under test and a source for its input.
    let mut registry = mold_nodes::registry();
    registry.processor(PoseCount).unwrap();
    registry
        .processor(common::Poses(common::two_poses()))
        .unwrap();
    let registry = Arc::new(registry);

    let mut ed = Editor::new(registry.clone());
    ed.add_node_with_id(common::POSES_KIND, common::POSES_KIND)
        .unwrap();
    ed.add_node_with_id("count", "pose_count").unwrap();
    ed.connect(common::poses_out(), &NodeId::new("count"), "poses")
        .unwrap();
    ed.set_param(&NodeId::new("count"), "passing_only", passing_only)
        .unwrap();
    let graph = ed.into_graph();
    let items = Engine::new(registry, Arc::new(MemoryCache::new()))
        .cook(&graph, &[])
        .unwrap();
    let it = items.items(&NodeId::new("count"))[0];
    assert_eq!(it.state, mold_graph::ItemState::Cooked, "{:?}", it.error);
    *it.result
        .as_ref()
        .unwrap()
        .get("count")
        .unwrap()
        .downcast_ref::<i64>()
        .unwrap()
}

#[test]
fn a_twelfth_node_is_one_file_under_a_hundred_lines() {
    assert_eq!(count(false), 2);
    assert_eq!(count(true), 1, "the parameter is honoured");
    let me = include_str!("twelfth_node.rs");
    let lines = me.lines().count();
    assert!(
        lines < 100,
        "this file is {lines} lines: the node is the budget"
    );
}
