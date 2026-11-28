//! **Co-fold / Dock Runner**: `LigandSet` + `Structure` → `PoseSet`, through a worker.
//!
//! The only worker node in this build, and the only node that cooks through the
//! command path (ADR-0009 §1) rather than `cook`:
//!
//! 1. [`prepare`](Processor::prepare) writes `ligands.sdf` and `receptor.cif` into the
//!    item's run directory and returns the `JobSpec` for the worker named by the
//!    `worker` param;
//! 2. the executor runs it (`mold-jobs`' local executor runs `mock-cofold` here);
//! 3. [`collect_event`](Processor::collect_event) turns each `pose` event into a pose
//!    appended to the item's `PoseSet` — so poses reach observers as they arrive —
//!    and [`collect_finish`](Processor::collect_finish) returns the final set.
//!
//! **The model version is not a parameter**: it is a field of the worker's manifest
//! (`workers/<name>/manifest.toml`), so the pin lives next to the environment that
//! honours it and never in this code. [`needs`](Processor::needs) comes from the same
//! manifest, which is why the node is built with the [`Workers`] it will run against.
//!
//! **One ligand per item.** The protocol's `pose` event names no ligand
//! (`docs/worker-protocol.md`), so a run whose SDF held several ligands could not be
//! attributed. Load Ligands already generates one item per ligand; this node refuses
//! anything else rather than guessing.

use std::path::PathBuf;
use std::sync::Arc;

use mold_graph::{
    CommandContext, CommandJob, Needs, NodeError, NodeSpec, Outputs, ParamSpec, PortType,
    Processor, Value,
};
use mold_jobs::Workers;
use mold_jobs::protocol::JobSpec;
use mold_mol::{Arena, PoseId};

use crate::data::{LigandSet, PoseSet, append_pose};
use crate::table::{self, PoseRow};

pub const KIND: &str = "cofold";

/// File names inside the run directory. Relative, so the same `JobSpec` bytes travel
/// to a local process, WSL2 or an SSH host (ADR-0005).
const LIGANDS: &str = "ligands.sdf";
const RECEPTOR: &str = "receptor.cif";

/// Runs a co-folding or docking worker, one ligand per item.
#[derive(Debug, Clone, Default)]
pub struct CoFold {
    workers: Option<Arc<Workers>>,
}

impl CoFold {
    /// Without manifests: every item asks for one slot and no GPU.
    pub fn new() -> Self {
        Self::default()
    }

    /// With the machine's worker manifests, so [`Processor::needs`] can report what
    /// the worker declares (`needs_gpu`).
    pub fn with_workers(workers: Arc<Workers>) -> Self {
        Self {
            workers: Some(workers),
        }
    }
}

/// The one ligand of this item, and the receptor behind it.
fn inputs<'a>(ctx: &'a CommandContext<'a>) -> Result<(&'a LigandSet, Arc<Arena>), NodeError> {
    let ligands: &LigandSet = ctx.inputs.one_as("ligands")?;
    if ligands.single().is_none() {
        return Err(NodeError::msg(format!(
            "this item carries {} ligands; co-fold runs one ligand per item (connect it \
             to `{}`, which generates one item per ligand)",
            ligands.len(),
            crate::load_ligands::KIND
        )));
    }
    let receptor = ctx
        .inputs
        .one("receptor")?
        .downcast_arc::<Arena>()
        .ok_or_else(|| NodeError::msg("`receptor` is not a mol arena"))?;
    if receptor.pose_count() == 0 {
        return Err(NodeError::msg("`receptor` holds no pose"));
    }
    Ok((ligands, receptor))
}

/// Read one pose file the worker wrote (a ligand-only mmCIF) and append it to `poses`,
/// taking the bonds from the ligand it was folded from — a pose file has none, and
/// everything topological in `geom` needs them.
fn append_predicted(
    poses: &mut Arena,
    cif: &PathBuf,
    ligand: &LigandSet,
    name: &str,
) -> Result<PoseId, NodeError> {
    let mut read = Arena::new();
    let ids = mold_mol::io::read_file(&mut read, cif, &mold_mol::LoadOptions::default())
        .map_err(|e| NodeError::msg(format!("cannot read pose {}: {e}", cif.display())))?;
    let [pose] = ids[..] else {
        return Err(NodeError::msg(format!(
            "pose {} holds {} models, expected one",
            cif.display(),
            ids.len()
        )));
    };
    let lig = ligand.single().expect("checked in prepare");
    let source = ligand.arena.pose(lig.pose).expect("the ligand's pose");
    let predicted = read.pose(pose).expect("just read");
    let want = &ligand.arena.elements()[source.atoms.clone()];
    let got = &read.elements()[predicted.atoms.clone()];
    if want != got {
        return Err(NodeError::msg(format!(
            "pose {} has {} atoms against the ligand's {} (or another element order): \
             the worker must keep the input atom order",
            cif.display(),
            got.len(),
            want.len()
        )));
    }
    let bonds: Vec<mold_mol::Bond> = ligand
        .arena
        .pose_bonds(lig.pose)
        .unwrap_or(&[])
        .iter()
        .map(|b| b.relative_to(source.atoms.start))
        .collect();
    append_pose(poses, &read, pose, Some(name), Some(&bonds))
        .ok_or_else(|| NodeError::msg("the pose vanished from its arena"))
}

/// The set folded so far, from the partial outputs.
fn accumulated(partial: &Outputs, receptor: Arc<Arena>) -> PoseSet {
    partial
        .get("poses")
        .and_then(|v| v.downcast_ref::<PoseSet>())
        .cloned()
        .unwrap_or_else(|| PoseSet::empty(receptor))
}

impl Processor for CoFold {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(KIND)
            .input("ligands", PortType::LigandSet)
            .broadcast_input("receptor", PortType::Structure)
            .output("poses", PortType::PoseSet)
            .param(
                ParamSpec::new("worker", "mock-cofold")
                    .doc("the worker to run; its manifest pins the model version"),
            )
            .param(ParamSpec::new("samples", 5i64).doc("poses per ligand"))
            .param(ParamSpec::new("seed", 0i64).doc("sampling seed"))
    }

    /// What the worker's manifest declares (a GPU worker asks for a GPU).
    fn needs(&self, params: &mold_graph::Params, _: &mold_graph::Attributes) -> Needs {
        let worker = params.str("worker").unwrap_or_default();
        self.workers
            .as_ref()
            .and_then(|w| w.get(worker))
            .map(|m| m.needs())
            .unwrap_or_default()
    }

    fn prepare(&self, ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
        let (ligands, receptor) = inputs(ctx)?;
        let lig = ligands.single().expect("checked");
        let mut sdf = Vec::new();
        mold_mol::sdf::write_sdf(&ligands.arena, lig.pose, &lig.properties, &mut sdf)
            .map_err(|e| NodeError::msg(format!("cannot write the ligand: {e}")))?;
        std::fs::write(ctx.run_dir.join(LIGANDS), &sdf)
            .map_err(|e| NodeError::msg(format!("cannot write {LIGANDS}: {e}")))?;
        mold_mol::io::write_file(&receptor, PoseId(0), ctx.run_dir.join(RECEPTOR))
            .map_err(|e| NodeError::msg(format!("cannot write {RECEPTOR}: {e}")))?;

        let spec = JobSpec::new(ctx.item.to_string())
            .input("ligands", LIGANDS)
            .input("receptor", RECEPTOR)
            .params(serde_json::json!({
                "samples": ctx.params.int("samples")?,
                "seed": ctx.params.int("seed")?,
            }));
        spec.validate().map_err(|e| NodeError::msg(e.to_string()))?;
        Ok(Some(
            CommandJob::new(ctx.params.str("worker")?, ctx.run_dir, spec.to_value())
                .stage(LIGANDS)
                .stage(RECEPTOR),
        ))
    }

    /// Each `pose` event becomes one pose of the item's `PoseSet`, so observers see
    /// poses while the worker is still running. Other events are ignored.
    fn collect_event(
        &self,
        ctx: &CommandContext<'_>,
        event: &serde_json::Value,
        partial: &mut Outputs,
    ) -> Result<bool, NodeError> {
        if event["event"] != "pose" {
            return Ok(false);
        }
        let (ligands, receptor) = inputs(ctx)?;
        let id = &ligands.single().expect("checked").id;
        let rank = event["pose_index"]
            .as_u64()
            .ok_or_else(|| NodeError::msg("a `pose` event without a `pose_index`"))?;
        let cif = event["cif_path"]
            .as_str()
            .ok_or_else(|| NodeError::msg("a `pose` event without a `cif_path`"))?;
        let path = ctx.run_dir.join(cif);
        let score = |name: &str| event["scores"][name].as_f64();

        let mut set = accumulated(partial, receptor);
        let poses = Arc::make_mut(&mut set.poses);
        let pose = append_predicted(poses, &path, ligands, &format!("{id} pose {rank}"))?;
        let mut rows = table::from_frame(&set.table).map_err(NodeError::msg)?;
        rows.push(PoseRow {
            pose_id: pose.0,
            ligand_id: id.clone(),
            model_rank: rank as u32,
            plddt: score("plddt"),
            iptm: score("iptm"),
            affinity_pred: score("affinity_pred"),
            binder_prob: score("binder_prob"),
            ..PoseRow::default()
        });
        set.table = Arc::new(table::to_frame(&rows));
        partial
            .values
            .insert("poses".into(), Value::new(PortType::PoseSet, set));
        partial.files.push(path);
        Ok(true)
    }

    fn collect_finish(
        &self,
        ctx: &CommandContext<'_>,
        _finished: &serde_json::Value,
        partial: Outputs,
    ) -> Result<Outputs, NodeError> {
        let (_, receptor) = inputs(ctx)?;
        let set = accumulated(&partial, receptor);
        if set.is_empty() {
            return Err(NodeError::msg(
                "the worker finished without sending a single `pose` event",
            ));
        }
        let mut out = Outputs::new()
            .with("poses", Value::new(PortType::PoseSet, set.clone()))
            .attr("poses", set.len() as i64);
        // The worker's own files, recorded (not copied) so the UI can open them.
        out.files = partial.files;
        Ok(out)
    }
}
