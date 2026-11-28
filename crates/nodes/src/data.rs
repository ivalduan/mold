//! The Rust type behind each [`PortType`](mold_graph::PortType).
//!
//! `mold_graph::Value` is type-erased: which Rust type a port type carries is a
//! convention, and this crate owns it (see `DESIGN.md` in `graph`). The convention:
//!
//! | `PortType` | Rust payload | Codec |
//! |---|---|---|
//! | `Structure` | [`mold_mol::Arena`] (one pose per model, usually one) | `mold-mol/arena` |
//! | `Sequence` | `String`: one-letter chains joined by `/` | `mold-graph/string` |
//! | `LigandSet` | [`LigandSet`] | `mold-nodes/ligand-set` |
//! | `PoseSet` | [`PoseSet`] | `mold-nodes/pose-set` |
//! | `Table` | [`polars::prelude::DataFrame`] | `mold-nodes/pose-table` |
//! | `Scalar` | `i64` or `f64` | `mold-graph/{i64,f64}` |
//! | `Msa` | — (MSA Query is not in this build) | — |

use std::sync::Arc;

use mold_mol::{Arena, AtomRecord, Bond, PoseId};
use polars::prelude::DataFrame;

use crate::table;

/// One ligand of a [`LigandSet`]: its id, its pose in the set's arena, and the data
/// items it came with (SDF `> <name>` fields).
#[derive(Debug, Clone, PartialEq)]
pub struct Ligand {
    /// Stable identifier, used as the pose table's `ligand_id`.
    pub id: String,
    /// The ligand's pose in [`LigandSet::arena`].
    pub pose: PoseId,
    pub properties: Vec<(String, String)>,
}

impl Ligand {
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A set of ligands: one arena pose each, **with bonds** (everything topological in
/// `geom` needs them), plus their ids and properties.
#[derive(Debug, Clone, PartialEq)]
pub struct LigandSet {
    pub arena: Arc<Arena>,
    pub ligands: Vec<Ligand>,
}

impl LigandSet {
    pub fn new(arena: Arena, ligands: Vec<Ligand>) -> Self {
        Self {
            arena: Arc::new(arena),
            ligands,
        }
    }

    pub fn len(&self) -> usize {
        self.ligands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ligands.is_empty()
    }

    /// The only ligand, for a one-ligand set.
    pub fn single(&self) -> Option<&Ligand> {
        match self.ligands.as_slice() {
            [l] => Some(l),
            _ => None,
        }
    }
}

/// Predicted poses of one or more ligands, superimposed over a **shared receptor**
/// (the 2026-11-27 decision: poses hold the ligand only, full complexes on demand).
///
/// `table` has one row per pose and `pose_id` indexes `poses`, so a `PoseSet` is
/// self-describing: the Pose Gate and the Interaction Profiler need no second input.
/// Ids are local to this set; a sink that merges several sets renumbers them.
#[derive(Debug, Clone, PartialEq)]
pub struct PoseSet {
    /// The receptor these poses sit in, as a one-pose arena. Empty for a pose set
    /// with no receptor (then the protein checks have nothing to compare against).
    pub receptor: Arc<Arena>,
    /// One pose per predicted ligand pose, with bonds.
    pub poses: Arc<Arena>,
    /// The pose table ([`mold_mol::pose_columns`]), one row per pose.
    pub table: Arc<DataFrame>,
}

impl PoseSet {
    /// An empty set over `receptor`.
    pub fn empty(receptor: Arc<Arena>) -> Self {
        Self {
            receptor,
            poses: Arc::new(Arena::new()),
            table: Arc::new(table::to_frame(&[])),
        }
    }

    pub fn len(&self) -> usize {
        self.poses.pose_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The receptor's pose, if it has one.
    pub fn receptor_pose(&self) -> Option<PoseId> {
        (self.receptor.pose_count() > 0).then_some(PoseId(0))
    }
}

/// Copy one pose of `src` into `dst` (chains, residues, atoms and bonds), optionally
/// replacing its name and its bonds. Returns the new pose's id.
///
/// `mold_mol` has no arena-to-arena copy yet; this is the one place that needs it.
pub fn append_pose(
    dst: &mut Arena,
    src: &Arena,
    pose: PoseId,
    name: Option<&str>,
    bonds: Option<&[Bond]>,
) -> Option<PoseId> {
    let p = src.pose(pose)?;
    let start = p.atoms.start;
    let mut b = dst.begin_pose(name.unwrap_or(&p.name).to_string());
    let mut first = None;
    let mut chain = None;
    let mut residue = None;
    for i in p.atoms.clone() {
        let ci = src.chain_indices()[i];
        if chain != Some(ci) {
            b.add_chain(src.chains()[ci as usize].id.clone());
            chain = Some(ci);
            residue = None;
        }
        let ri = src.residue_indices()[i];
        if residue != Some(ri) {
            let r = &src.residues()[ri as usize];
            b.add_residue(r.name.clone(), r.seq_num, r.insertion_code, r.hetero);
            residue = Some(ri);
        }
        let new = b.add_atom(AtomRecord {
            name: src.atom_names()[i],
            element: src.elements()[i],
            position: src.positions()[i],
            b_factor: src.b_factors()[i],
            occupancy: src.occupancies()[i],
            formal_charge: src.formal_charges()[i],
        });
        first = first.or(Some(new));
    }
    let base = first.unwrap_or(0);
    let copied: Vec<Bond> = match bonds {
        // Pose-local indices, as `Bond::relative_to` produces them.
        Some(bs) => bs
            .iter()
            .map(|x| Bond {
                a: x.a + base,
                b: x.b + base,
                ..*x
            })
            .collect(),
        None => src
            .pose_bonds(pose)
            .unwrap_or(&[])
            .iter()
            .map(|x| Bond {
                a: x.a - start as u32 + base,
                b: x.b - start as u32 + base,
                ..*x
            })
            .collect(),
    };
    for x in copied {
        b.add_bond(x.a, x.b, x.order);
    }
    Some(b.finish())
}

/// A one-pose arena holding a copy of `pose`.
pub fn single_pose_arena(src: &Arena, pose: PoseId, name: Option<&str>) -> Option<Arena> {
    let mut out = Arena::new();
    append_pose(&mut out, src, pose, name, None)?;
    Some(out)
}
