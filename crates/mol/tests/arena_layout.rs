//! The arena's positions are one contiguous `&[[f32; 3]]`; poses are sub-slices of
//! it, never copies.

use std::path::PathBuf;

use mold_mol::io::{self, LoadOptions};
use mold_mol::{Arena, PoseId};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/structures")
        .join(name)
}

/// Several poses (as from a co-folding run) in one arena.
fn multi_pose_arena() -> Arena {
    let mut arena = Arena::new();
    let opts = LoadOptions::default();
    for file in ["1M17.cif", "4G5J.cif", "1M17.pdb", "1EJG.cif"] {
        io::read_file(&mut arena, fixture(file), &opts).unwrap();
    }
    arena
}

#[test]
fn positions_are_one_contiguous_slice_and_poses_are_views_into_it() {
    let arena = multi_pose_arena();
    assert_eq!(arena.pose_count(), 4);
    let all: &[[f32; 3]] = arena.positions();
    assert_eq!(all.len(), 2546 + 2599 + 2546 + 637);
    assert_eq!(arena.positions_flat().len(), all.len() * 3);
    assert_eq!(arena.positions_flat().as_ptr(), all.as_ptr().cast::<f32>());

    // Byte layout: tightly packed, 12 bytes per atom, uploadable as-is.
    assert_eq!(std::mem::size_of_val(all), all.len() * 12);

    let mut expected_start = 0;
    for id in arena.pose_ids() {
        let pose = arena.pose(id).unwrap();
        assert_eq!(pose.atoms.start, expected_start, "poses are back to back");
        expected_start = pose.atoms.end;
        let view = arena.pose_positions(id).unwrap();
        // Same memory, not a copy.
        assert_eq!(view.as_ptr(), all[pose.atoms.clone()].as_ptr());
        assert_eq!(view.len(), pose.atoms.len());
        assert_eq!(arena.pose_of_atom(pose.atoms.start), Some(id));
        assert_eq!(arena.pose_of_atom(pose.atoms.end - 1), Some(id));
    }
    assert_eq!(expected_start, all.len());
}

#[test]
fn every_per_atom_array_has_the_same_length_and_global_indices() {
    let arena = multi_pose_arena();
    let n = arena.atom_count();
    assert_eq!(arena.elements().len(), n);
    assert_eq!(arena.atom_names().len(), n);
    assert_eq!(arena.residue_indices().len(), n);
    assert_eq!(arena.chain_indices().len(), n);
    assert_eq!(arena.b_factors().len(), n);
    assert_eq!(arena.occupancies().len(), n);
    assert_eq!(arena.formal_charges().len(), n);

    for (i, (&r, &c)) in arena
        .residue_indices()
        .iter()
        .zip(arena.chain_indices())
        .enumerate()
    {
        let residue = &arena.residues()[r as usize];
        assert!(residue.atoms.contains(&i));
        assert_eq!(residue.chain, c);
        let chain = &arena.chains()[c as usize];
        assert!(chain.atoms.contains(&i));
        assert_eq!(Some(chain.pose), arena.pose_of_atom(i));
    }
}

#[test]
fn positions_mut_updates_in_place() {
    let mut arena = multi_pose_arena();
    let ptr = arena.positions().as_ptr();
    let range = arena.pose(PoseId(1)).unwrap().atoms.clone();
    for p in arena.pose_positions_mut(PoseId(1)).unwrap() {
        p[0] += 1.0;
    }
    assert_eq!(arena.positions().as_ptr(), ptr);
    let before = {
        let mut a = Arena::new();
        io::read_file(&mut a, fixture("4G5J.cif"), &LoadOptions::default()).unwrap();
        a.positions()[0]
    };
    assert!((arena.positions()[range.start][0] - (before[0] + 1.0)).abs() < 1e-4);
}
