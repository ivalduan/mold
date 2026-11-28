//! **Pose Gate**: `PoseSet` → `PoseSet`, `Table`. Physical validity, per pose.
//!
//! Every parameter is a field of [`GateConfig`](mold_geom::GateConfig) — the gate's
//! thresholds are `geom`'s, with `geom`'s provenance (PoseBusters 0.6 where it
//! computes the same quantity) — plus `protein`, which decides what the protein
//! checks see (see [`ProteinRule`]).
//!
//! The node is **non-destructive**: no pose is dropped. It fills `clash_count` and
//! `gate_pass` in the pose table and passes the whole set on.
//!
//! The receptor rides in the `PoseSet` (one shared receptor for every pose, the
//! 2026-11-27 decision), so this node needs no `Structure` input of its own.

use std::sync::Arc;

use mold_geom::{GateConfig, PoseGate as Gate, Topology};
use mold_graph::{
    CookContext, NodeError, NodeSpec, Outputs, ParamSpec, Params, PortType, Processor, Value,
};
use mold_mol::{Arena, PoseId};

use crate::data::PoseSet;
use crate::protein::ProteinRule;
use crate::table::{self, PoseRow};

pub const KIND: &str = "pose_gate";

/// Runs `geom`'s pose gate over every pose of a set.
#[derive(Debug, Clone, Copy, Default)]
pub struct PoseGate;

/// Every threshold, with `geom`'s default. `volume` (the overlap grid) is left at
/// `VolumeParams::default()`: it is a numerical detail of one check, not a knob.
fn config(params: &Params) -> Result<GateConfig, NodeError> {
    let f = |name: &str| -> Result<f32, NodeError> { Ok(params.float(name)? as f32) };
    Ok(GateConfig {
        bond_length_tolerance: f("bond_length_tolerance")?,
        bond_angle_tolerance: f("bond_angle_tolerance")?,
        internal_clash_ratio: f("internal_clash_ratio")?,
        aromatic_flatness_threshold: f("aromatic_flatness_threshold")?,
        protein_clash_cutoff: f("protein_clash_cutoff")?,
        protein_radius_scale: f("protein_radius_scale")?,
        max_distance_to_protein: f("max_distance_to_protein")?,
        search_distance: f("search_distance")?,
        volume_overlap_cutoff: f("volume_overlap_cutoff")?,
        rmsd_threshold: f("rmsd_threshold")?,
        detect_covalent: params.bool("detect_covalent")?,
        ..GateConfig::default()
    })
}

/// The ligand topology of a pose, and a key that tells two ligands apart. Poses of one
/// ligand share both, so a `Topology` (and the receptor index behind it) is built once
/// per ligand, not once per pose — what `geom`'s 200 ms budget assumes.
pub(crate) type TopologyKey = (Vec<mold_mol::Element>, Vec<u32>);

/// One ligand's poses: the pose its topology was built from, and every pose sharing it.
pub(crate) type Group = (PoseId, Vec<PoseId>);

pub(crate) fn topology_key(arena: &Arena, pose: PoseId) -> TopologyKey {
    let Some(p) = arena.pose(pose) else {
        return (Vec::new(), Vec::new());
    };
    let start = p.atoms.start as u32;
    let bonds = arena
        .pose_bonds(pose)
        .unwrap_or(&[])
        .iter()
        .flat_map(|b| [b.a - start, b.b - start, b.order.mdl_code() as u32])
        .collect();
    (arena.elements()[p.atoms.clone()].to_vec(), bonds)
}

pub(crate) fn topology(arena: &Arena, pose: PoseId) -> Result<Topology, NodeError> {
    Topology::from_arena(arena, pose).map_err(|e| {
        NodeError::msg(format!(
            "pose {}: {e} (a pose needs the ligand's bonds; co-fold copies them)",
            pose.0
        ))
    })
}

/// Poses of `set` grouped by ligand topology, in pose order.
pub(crate) fn groups(set: &PoseSet) -> Vec<Group> {
    let mut out: Vec<(TopologyKey, Group)> = Vec::new();
    for pose in set.poses.pose_ids() {
        let key = topology_key(&set.poses, pose);
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, (_, members))) => members.push(pose),
            None => out.push((key, (pose, vec![pose]))),
        }
    }
    out.into_iter().map(|(_, g)| g).collect()
}

impl Processor for PoseGate {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(KIND)
            .input("poses", PortType::PoseSet)
            .output("poses", PortType::PoseSet)
            .output("table", PortType::Table)
            .param(ProteinRule::param_spec())
            .param(
                ParamSpec::new("bond_length_tolerance", 0.25)
                    .doc("bond length outlier: outside (1 ± tol) × reference"),
            )
            .param(
                ParamSpec::new("bond_angle_tolerance", 0.25)
                    .doc("bond angle outlier: 1-3 distance outside (1 ± tol) × reference"),
            )
            .param(
                ParamSpec::new("internal_clash_ratio", 0.5)
                    .doc("internal clash: a non-bonded pair below this fraction of its vdW sum"),
            )
            .param(
                ParamSpec::new("aromatic_flatness_threshold", 0.25)
                    .doc("aromatic ring flatness: max atom distance from the ring plane, Å"),
            )
            .param(
                ParamSpec::new("protein_clash_cutoff", 0.75)
                    .doc("protein–ligand clash: below this fraction of the vdW radius sum"),
            )
            .param(
                ParamSpec::new("protein_radius_scale", 1.0)
                    .doc("scale on the vdW radius sum for protein–ligand clashes"),
            )
            .param(
                ParamSpec::new("max_distance_to_protein", 5.0)
                    .doc("the ligand must come within this many Å of the protein"),
            )
            .param(
                ParamSpec::new("search_distance", 6.0)
                    .doc("radius of the protein neighbourhood examined around each ligand atom, Å"),
            )
            .param(
                ParamSpec::new("volume_overlap_cutoff", 0.075)
                    .doc("fail above this fraction of ligand volume inside the protein"),
            )
            .param(
                ParamSpec::new("rmsd_threshold", 2.0)
                    .doc("accuracy: RMSD to a reference pose at or below this, Å"),
            )
            .param(
                ParamSpec::new("detect_covalent", true)
                    .doc("detect a covalent ligand–protein bond and exclude it from the clashes"),
            )
    }

    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let set: &PoseSet = ctx.input("poses")?;
        let rule = ProteinRule::from_params(ctx.params)?;
        let config = config(ctx.params)?;
        let receptor_pose = set.receptor_pose().ok_or_else(|| {
            NodeError::msg("the pose set carries no receptor: nothing to gate against")
        })?;
        let mut rows = table::from_frame(&set.table).map_err(NodeError::msg)?;
        if rows.len() != set.len() {
            return Err(NodeError::msg(format!(
                "the pose set has {} poses and {} table rows",
                set.len(),
                rows.len()
            )));
        }

        for (first, members) in groups(set) {
            if ctx.cancel.is_cancelled() {
                return Err(NodeError::Cancelled);
            }
            let receptor = rule.receptor(&set.receptor, receptor_pose).ok_or_else(|| {
                NodeError::msg("the receptor arena has no pose to build a receptor from")
            })?;
            let gate = Gate::new(receptor, topology(&set.poses, first)?, config);
            for pose in members {
                let positions = set
                    .poses
                    .pose_positions(pose)
                    .ok_or_else(|| NodeError::msg(format!("no pose {}", pose.0)))?;
                let report = gate.evaluate(positions, &[]);
                let row: &mut PoseRow =
                    rows.iter_mut()
                        .find(|r| r.pose_id == pose.0)
                        .ok_or_else(|| {
                            NodeError::msg(format!("the pose table has no row for pose {}", pose.0))
                        })?;
                row.clash_count = Some(report.protein.clashes as u32);
                row.gate_pass = Some(report.pass());
            }
        }

        let passed = rows.iter().filter(|r| r.gate_pass == Some(true)).count();
        let table = Arc::new(table::to_frame(&rows));
        let gated = PoseSet {
            receptor: set.receptor.clone(),
            poses: set.poses.clone(),
            table: table.clone(),
        };
        Ok(Outputs::new()
            .with("poses", Value::new(PortType::PoseSet, gated))
            .with("table", Value::from_arc(PortType::Table, table))
            .attr("gate_pass", passed as i64)
            .attr("protein", rule.as_str()))
    }
}
