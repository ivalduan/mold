//! Domain core: the three structures that carry the whole application.
//!
//! 1. **Coordinate arena**: atoms in struct-of-arrays form. Positions are one
//!    contiguous `Vec<[f32; 3]>` that uploads to the GPU without a transform pass.
//! 2. **Pose table**: one Polars `DataFrame`, one row per pose, joined to the
//!    arena by `pose_id`. It never holds coordinates; the arena never holds metrics.
//! 3. **Selection**: one value broadcast to every panel; no panel owns it.
//!
//! No GPUI, no async. See `CLAUDE.md` in this crate for scope and acceptance tests.

pub mod arena;
pub mod cache;
mod element;
pub mod io;
pub mod pose_table;
pub mod sdf;
mod selection;

pub use arena::{Arena, AtomName, AtomRecord, Bond, BondOrder, Chain, Pose, PoseBuilder, Residue};
pub use element::Element;
pub use io::{AltLocPolicy, Format, IoError, LoadOptions};
pub use selection::Selection;

/// Index of a pose in the coordinate arena, and the join key of the pose table.
///
/// **Contract type.** Every panel, node and the viewport agree on this. Changes
/// land on `main` in their own PR before any worktree depends on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoseId(pub u32);

/// Column names of the pose table. The columns are the product.
///
/// **Contract.** Nodes write these columns, panels read them.
pub mod pose_columns {
    pub const POSE_ID: &str = "pose_id";
    pub const LIGAND_ID: &str = "ligand_id";
    pub const MODEL_RANK: &str = "model_rank";
    pub const PLDDT: &str = "plddt";
    pub const IPTM: &str = "iptm";
    pub const AFFINITY_PRED: &str = "affinity_pred";
    pub const BINDER_PROB: &str = "binder_prob";
    pub const CLASH_COUNT: &str = "clash_count";
    pub const STRAIN_KCAL: &str = "strain_kcal";
    pub const N_HBONDS: &str = "n_hbonds";
    pub const N_SALTBRIDGE: &str = "n_saltbridge";
    pub const POCKET_RMSD: &str = "pocket_rmsd";
    pub const GATE_PASS: &str = "gate_pass";
}
