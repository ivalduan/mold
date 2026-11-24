//! The coordinate arena: every atom of every pose, struct-of-arrays.
//!
//! # Layout
//! A pose is a **full copy** of one complex (receptor + ligand) occupying one
//! contiguous atom range. Poses are appended back to back, so
//! `positions()[pose.atoms.clone()]` is a sub-slice of the single positions
//! buffer: no per-pose allocation, no copy, and the whole buffer (or any pose)
//! uploads to the GPU as-is.
//!
//! Every per-atom array has the same length and the same index space. Residue and
//! chain indices stored per atom are **global** (index into [`Arena::residues`] /
//! [`Arena::chains`]), so consumers never need to add per-pose offsets.

use std::fmt;
use std::ops::Range;

use crate::{Element, PoseId};

/// An atom name of at most four ASCII characters (`"CA"`, `"OXT"`, `"CL1"`),
/// stored inline so the per-atom name array holds no heap pointers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AtomName([u8; 4]);

impl AtomName {
    pub const MAX_LEN: usize = 4;

    /// `None` if `name` is empty after trimming, longer than four bytes, or not ASCII.
    pub fn new(name: &str) -> Option<AtomName> {
        let name = name.trim();
        if name.is_empty() || name.len() > Self::MAX_LEN || !name.is_ascii() {
            return None;
        }
        let mut bytes = [0u8; 4];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        Some(AtomName(bytes))
    }

    pub fn as_str(&self) -> &str {
        let len = self.0.iter().position(|&b| b == 0).unwrap_or(4);
        // Only ASCII is ever stored.
        std::str::from_utf8(&self.0[..len]).unwrap_or("")
    }
}

impl fmt::Debug for AtomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AtomName({:?})", self.as_str())
    }
}

impl fmt::Display for AtomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<&str> for AtomName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// One residue (amino acid, nucleotide, ligand, water) in the arena.
#[derive(Debug, Clone, PartialEq)]
pub struct Residue {
    /// Component name (`"ALA"`, `"AQ4"`, `"HOH"`).
    pub name: String,
    /// Author sequence number.
    pub seq_num: i32,
    pub insertion_code: Option<char>,
    /// `true` if the atoms were HETATM records (ligands, waters, modified residues).
    pub hetero: bool,
    /// Global chain index.
    pub chain: u32,
    /// Global atom range.
    pub atoms: Range<usize>,
}

/// One chain of one pose.
#[derive(Debug, Clone, PartialEq)]
pub struct Chain {
    /// Author chain id (`"A"`).
    pub id: String,
    pub pose: PoseId,
    /// Global residue range.
    pub residues: Range<usize>,
    /// Global atom range.
    pub atoms: Range<usize>,
}

/// One pose: a complete complex, one contiguous block of the arena.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    pub name: String,
    pub atoms: Range<usize>,
    pub residues: Range<usize>,
    pub chains: Range<usize>,
}

/// Input record for [`PoseBuilder::add_atom`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtomRecord {
    pub name: AtomName,
    pub element: Element,
    pub position: [f32; 3],
    pub b_factor: f32,
    pub occupancy: f32,
    pub formal_charge: i8,
}

/// All atoms of all poses, struct-of-arrays. See the [module docs](self).
#[derive(Debug, Clone, Default)]
pub struct Arena {
    positions: Vec<[f32; 3]>,
    elements: Vec<Element>,
    atom_names: Vec<AtomName>,
    residue_indices: Vec<u32>,
    chain_indices: Vec<u32>,
    b_factors: Vec<f32>,
    occupancies: Vec<f32>,
    formal_charges: Vec<i8>,
    residues: Vec<Residue>,
    chains: Vec<Chain>,
    poses: Vec<Pose>,
}

impl Arena {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve room for `atoms` more atoms in every per-atom array.
    pub fn reserve_atoms(&mut self, atoms: usize) {
        self.positions.reserve(atoms);
        self.elements.reserve(atoms);
        self.atom_names.reserve(atoms);
        self.residue_indices.reserve(atoms);
        self.chain_indices.reserve(atoms);
        self.b_factors.reserve(atoms);
        self.occupancies.reserve(atoms);
        self.formal_charges.reserve(atoms);
    }

    // ---- Sizes -------------------------------------------------------------

    pub fn atom_count(&self) -> usize {
        self.positions.len()
    }

    pub fn pose_count(&self) -> usize {
        self.poses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty() && self.poses.is_empty()
    }

    // ---- Per-atom arrays (all the same length, global atom index) ----------

    /// Every atom position of every pose, in Å, one contiguous buffer.
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }

    /// The same buffer as `[x0, y0, z0, x1, ...]`, for APIs that want flat floats.
    pub fn positions_flat(&self) -> &[f32] {
        self.positions.as_flattened()
    }

    /// Mutable positions, for in-place coordinate updates (e.g. superposition).
    /// Topology cannot change through this.
    pub fn positions_mut(&mut self) -> &mut [[f32; 3]] {
        &mut self.positions
    }

    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    pub fn atom_names(&self) -> &[AtomName] {
        &self.atom_names
    }

    /// Global residue index of each atom (into [`Arena::residues`]).
    pub fn residue_indices(&self) -> &[u32] {
        &self.residue_indices
    }

    /// Global chain index of each atom (into [`Arena::chains`]).
    pub fn chain_indices(&self) -> &[u32] {
        &self.chain_indices
    }

    pub fn b_factors(&self) -> &[f32] {
        &self.b_factors
    }

    pub fn occupancies(&self) -> &[f32] {
        &self.occupancies
    }

    pub fn formal_charges(&self) -> &[i8] {
        &self.formal_charges
    }

    // ---- Tables -------------------------------------------------------------

    pub fn residues(&self) -> &[Residue] {
        &self.residues
    }

    pub fn chains(&self) -> &[Chain] {
        &self.chains
    }

    pub fn poses(&self) -> &[Pose] {
        &self.poses
    }

    pub fn pose_ids(&self) -> impl ExactSizeIterator<Item = PoseId> + '_ {
        (0..self.poses.len()).map(|i| PoseId(i as u32))
    }

    pub fn pose(&self, id: PoseId) -> Option<&Pose> {
        self.poses.get(id.0 as usize)
    }

    /// The pose's positions: a sub-slice of [`Arena::positions`], not a copy.
    pub fn pose_positions(&self, id: PoseId) -> Option<&[[f32; 3]]> {
        self.pose(id).map(|p| &self.positions[p.atoms.clone()])
    }

    pub fn pose_positions_mut(&mut self, id: PoseId) -> Option<&mut [[f32; 3]]> {
        let range = self.pose(id)?.atoms.clone();
        Some(&mut self.positions[range])
    }

    /// The pose that owns a global atom index (e.g. from a GPU pick).
    pub fn pose_of_atom(&self, atom: usize) -> Option<PoseId> {
        if atom >= self.atom_count() {
            return None;
        }
        let i = self.poses.partition_point(|p| p.atoms.end <= atom);
        (i < self.poses.len()).then_some(PoseId(i as u32))
    }

    // ---- Building ------------------------------------------------------------

    /// Start appending a pose. Atoms go straight into the arena's arrays; nothing
    /// is staged. Call [`PoseBuilder::finish`] to commit; dropping the builder
    /// without finishing rolls the arena back.
    pub fn begin_pose(&mut self, name: impl Into<String>) -> PoseBuilder<'_> {
        PoseBuilder {
            start_atoms: self.positions.len(),
            start_residues: self.residues.len(),
            start_chains: self.chains.len(),
            arena: self,
            name: name.into(),
            finished: false,
        }
    }

    fn truncate(&mut self, atoms: usize, residues: usize, chains: usize) {
        self.positions.truncate(atoms);
        self.elements.truncate(atoms);
        self.atom_names.truncate(atoms);
        self.residue_indices.truncate(atoms);
        self.chain_indices.truncate(atoms);
        self.b_factors.truncate(atoms);
        self.occupancies.truncate(atoms);
        self.formal_charges.truncate(atoms);
        self.residues.truncate(residues);
        self.chains.truncate(chains);
    }
}

fn index_u32(i: usize, what: &str) -> u32 {
    u32::try_from(i).unwrap_or_else(|_| panic!("arena exceeds u32::MAX {what}"))
}

/// Appends one pose to an [`Arena`]: chains, then residues, then atoms, in order.
#[derive(Debug)]
pub struct PoseBuilder<'a> {
    arena: &'a mut Arena,
    name: String,
    start_atoms: usize,
    start_residues: usize,
    start_chains: usize,
    finished: bool,
}

impl PoseBuilder<'_> {
    /// The id this pose will get when finished.
    pub fn pose_id(&self) -> PoseId {
        PoseId(index_u32(self.arena.poses.len(), "poses"))
    }

    /// Open a new chain; subsequent residues belong to it. Returns its global index.
    pub fn add_chain(&mut self, id: impl Into<String>) -> u32 {
        let pose = self.pose_id();
        let a = &mut *self.arena;
        let index = index_u32(a.chains.len(), "chains");
        a.chains.push(Chain {
            id: id.into(),
            pose,
            residues: a.residues.len()..a.residues.len(),
            atoms: a.positions.len()..a.positions.len(),
        });
        index
    }

    /// Open a new residue in the current chain. Returns its global index.
    ///
    /// # Panics
    /// If no chain has been added to this pose yet.
    pub fn add_residue(
        &mut self,
        name: impl Into<String>,
        seq_num: i32,
        insertion_code: Option<char>,
        hetero: bool,
    ) -> u32 {
        assert!(
            self.arena.chains.len() > self.start_chains,
            "add_chain before add_residue"
        );
        let a = &mut *self.arena;
        let chain = a.chains.len() - 1;
        let index = index_u32(a.residues.len(), "residues");
        a.residues.push(Residue {
            name: name.into(),
            seq_num,
            insertion_code,
            hetero,
            chain: chain as u32,
            atoms: a.positions.len()..a.positions.len(),
        });
        a.chains[chain].residues.end = a.residues.len();
        index
    }

    /// Append an atom to the current residue. Returns its global index.
    ///
    /// # Panics
    /// If no residue has been added to this pose yet.
    pub fn add_atom(&mut self, atom: AtomRecord) -> u32 {
        assert!(
            self.arena.residues.len() > self.start_residues,
            "add_residue before add_atom"
        );
        let a = &mut *self.arena;
        let index = index_u32(a.positions.len(), "atoms");
        let residue = a.residues.len() - 1;
        let chain = a.chains.len() - 1;
        a.positions.push(atom.position);
        a.elements.push(atom.element);
        a.atom_names.push(atom.name);
        a.residue_indices.push(residue as u32);
        a.chain_indices.push(chain as u32);
        a.b_factors.push(atom.b_factor);
        a.occupancies.push(atom.occupancy);
        a.formal_charges.push(atom.formal_charge);
        let end = a.positions.len();
        a.residues[residue].atoms.end = end;
        a.chains[chain].atoms.end = end;
        index
    }

    /// Number of atoms added to this pose so far.
    pub fn atom_count(&self) -> usize {
        self.arena.positions.len() - self.start_atoms
    }

    /// Commit the pose and return its id.
    pub fn finish(mut self) -> PoseId {
        let id = self.pose_id();
        let a = &mut *self.arena;
        a.poses.push(Pose {
            name: std::mem::take(&mut self.name),
            atoms: self.start_atoms..a.positions.len(),
            residues: self.start_residues..a.residues.len(),
            chains: self.start_chains..a.chains.len(),
        });
        self.finished = true;
        id
    }
}

impl Drop for PoseBuilder<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.arena
                .truncate(self.start_atoms, self.start_residues, self.start_chains);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(name: &str, x: f32) -> AtomRecord {
        AtomRecord {
            name: AtomName::new(name).unwrap(),
            element: Element::C,
            position: [x, 0.0, 0.0],
            b_factor: 0.0,
            occupancy: 1.0,
            formal_charge: 0,
        }
    }

    fn two_atom_pose(arena: &mut Arena, name: &str) -> PoseId {
        let mut b = arena.begin_pose(name);
        b.add_chain("A");
        b.add_residue("GLY", 1, None, false);
        b.add_atom(atom("CA", 1.0));
        b.add_residue("LIG", 2, None, true);
        b.add_atom(atom("C1", 2.0));
        b.finish()
    }

    #[test]
    fn atom_name_is_inline_and_bounded() {
        assert_eq!(AtomName::new(" CA ").unwrap().as_str(), "CA");
        assert_eq!(AtomName::new("CL12").unwrap(), "CL12");
        assert!(AtomName::new("C1234").is_none());
        assert!(AtomName::new("").is_none());
        assert_eq!(std::mem::size_of::<AtomName>(), 4);
    }

    #[test]
    fn poses_are_back_to_back_with_global_indices() {
        let mut arena = Arena::new();
        let p0 = two_atom_pose(&mut arena, "a");
        let p1 = two_atom_pose(&mut arena, "b");
        assert_eq!((p0, p1), (PoseId(0), PoseId(1)));
        assert_eq!(arena.atom_count(), 4);
        assert_eq!(arena.pose(p1).unwrap().atoms, 2..4);
        assert_eq!(arena.residue_indices(), &[0, 1, 2, 3]);
        assert_eq!(arena.chain_indices(), &[0, 0, 1, 1]);
        assert_eq!(arena.chains()[1].pose, p1);
        assert_eq!(arena.residues()[3].atoms, 3..4);
        assert_eq!(arena.pose_of_atom(0), Some(p0));
        assert_eq!(arena.pose_of_atom(2), Some(p1));
        assert_eq!(arena.pose_of_atom(4), None);
        assert_eq!(arena.positions_flat().len(), 12);
    }

    #[test]
    fn dropped_builder_rolls_back() {
        let mut arena = Arena::new();
        two_atom_pose(&mut arena, "kept");
        {
            let mut b = arena.begin_pose("abandoned");
            b.add_chain("B");
            b.add_residue("ALA", 1, None, false);
            b.add_atom(atom("N", 0.0));
        }
        assert_eq!(arena.atom_count(), 2);
        assert_eq!(arena.residues().len(), 2);
        assert_eq!(arena.chains().len(), 1);
        assert_eq!(arena.pose_count(), 1);
    }
}
