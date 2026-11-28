//! `Topology::from_arena`: the ligand graph straight from mol's bond table,
//! with mol's global bond indices made pose-local.

mod common;

use mold_geom::{BondOrder, Topology, TopologyError};
use mold_mol::{Arena, LoadOptions, PoseId};

/// Load a protein first so the ligand's atoms (and bonds) do not start at
/// global index 0: the constructor must subtract the pose offset.
fn arena_with_protein_then_ligand() -> (Arena, PoseId) {
    let mut arena = Arena::new();
    mold_mol::io::read_file(
        &mut arena,
        common::posebusters_dir().join("inputs/4WKQ_protein.pdb"),
        &LoadOptions::default(),
    )
    .unwrap();
    let read = mold_mol::sdf::read_sdf_file(
        &mut arena,
        common::posebusters_dir().join("inputs/4WKQ_IRE_crystal.sdf"),
    )
    .unwrap();
    assert!(read.errors.is_empty());
    (arena, read.records[0].pose)
}

#[test]
fn from_arena_matches_new_over_the_pose_local_bonds() {
    let (arena, pose) = arena_with_protein_then_ligand();
    let start = arena.pose(pose).unwrap().atoms.start;
    assert!(start > 0, "the ligand must not be the first pose");

    let t = Topology::from_arena(&arena, pose).unwrap();
    let range = arena.pose(pose).unwrap().atoms.clone();
    let local: Vec<_> = arena
        .pose_bonds(pose)
        .unwrap()
        .iter()
        .map(|b| b.relative_to(start))
        .collect();
    let direct = Topology::new(&arena.elements()[range], &local).unwrap();

    assert_eq!(t.atom_count(), 31, "gefitinib, 31 heavy atoms");
    assert_eq!(t.bonds(), direct.bonds());
    assert_eq!(t.bonds().len(), 34);
    assert_eq!(t.aromatic_rings().count(), 3);
    assert_eq!(t.rings().len(), direct.rings().len());
    assert!(t.bonds().iter().all(|b| (b.a as usize) < t.atom_count()));
    assert!(
        t.bonds().iter().any(|b| b.order == BondOrder::Double),
        "Kekulé input"
    );
}

#[test]
fn from_arena_on_a_pose_without_bonds_or_a_missing_pose() {
    let (arena, _) = arena_with_protein_then_ligand();
    // The PDB pose has no bonds: a valid, edgeless topology.
    let protein = Topology::from_arena(&arena, PoseId(0)).unwrap();
    assert!(protein.bonds().is_empty());
    assert_eq!(
        Topology::from_arena(&arena, PoseId(99)).unwrap_err(),
        TopologyError::NoSuchPose(PoseId(99))
    );
}

#[test]
fn the_fixture_loader_uses_mols_sdf_reader() {
    // The loader returns pose-local bonds that build a topology directly.
    let lig = common::load_ligand("inputs/1M17_AQ4_crystal.sdf");
    let t = Topology::new(&lig.elements, &lig.bonds).unwrap();
    assert_eq!(t.atom_count(), lig.positions.len());
    assert_eq!(t.aromatic_rings().count(), 3);
}
