//! **Interaction Profiler**: `PoseSet` → `Table`. H-bonds and salt bridges per pose.
//!
//! `geom`'s `contacts` with PLIP's definitions and thresholds. Like the Pose Gate it
//! reads the receptor out of the `PoseSet` and takes the same `protein` parameter, and
//! like it, it adds columns to the table it was given rather than inventing a new one,
//! so Join Metrics is not needed to see the numbers next to the gate's.
//!
//! Only the thresholds' defaults are used: `ContactConfig`'s nine fields are not
//! parameters yet (a follow-up — they are PLIP's, and PLIP does not expect them to be
//! tuned per run).

use std::sync::Arc;

use mold_geom::ContactConfig;
use mold_graph::{CookContext, NodeError, NodeSpec, Outputs, PortType, Processor, Value};

use crate::data::PoseSet;
use crate::pose_gate::{groups, topology};
use crate::protein::ProteinRule;
use crate::table;

pub const KIND: &str = "interaction_profiler";

/// Profiles every pose of a set against its receptor.
#[derive(Debug, Clone, Copy, Default)]
pub struct InteractionProfiler;

impl Processor for InteractionProfiler {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(KIND)
            .input("poses", PortType::PoseSet)
            .output("table", PortType::Table)
            .param(ProteinRule::param_spec())
    }

    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let set: &PoseSet = ctx.input("poses")?;
        let rule = ProteinRule::from_params(ctx.params)?;
        let receptor_pose = set
            .receptor_pose()
            .ok_or_else(|| NodeError::msg("the pose set carries no receptor to profile against"))?;
        let receptor = rule
            .receptor(&set.receptor, receptor_pose)
            .ok_or_else(|| NodeError::msg("the receptor arena has no pose"))?;
        let config = ContactConfig::default();
        let mut rows = table::from_frame(&set.table).map_err(NodeError::msg)?;

        for (first, members) in groups(set) {
            if ctx.cancel.is_cancelled() {
                return Err(NodeError::Cancelled);
            }
            let topology = topology(&set.poses, first)?;
            for pose in members {
                let positions = set
                    .poses
                    .pose_positions(pose)
                    .ok_or_else(|| NodeError::msg(format!("no pose {}", pose.0)))?;
                let found = mold_geom::contacts(&receptor, &topology, positions, &config);
                let row = rows
                    .iter_mut()
                    .find(|r| r.pose_id == pose.0)
                    .ok_or_else(|| {
                        NodeError::msg(format!("the pose table has no row for pose {}", pose.0))
                    })?;
                row.n_hbonds = Some(found.n_hbonds() as u32);
                row.n_saltbridge = Some(found.n_saltbridge() as u32);
            }
        }

        let hbonds: u32 = rows.iter().filter_map(|r| r.n_hbonds).sum();
        Ok(Outputs::new()
            .with(
                "table",
                Value::from_arc(PortType::Table, Arc::new(table::to_frame(&rows))),
            )
            .attr("n_hbonds", i64::from(hbonds))
            .attr("protein", rule.as_str()))
    }
}
