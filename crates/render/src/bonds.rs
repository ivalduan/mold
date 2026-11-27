//! A distance-based bond guesser, so sticks can be drawn before `mold-mol`
//! has bonds.
//!
//! **Stop-gap.** Bond perception belongs in `mold-mol` (from the mmCIF
//! `struct_conn` / chemical component dictionary, SDF bond blocks, then
//! distance as the fallback). The renderer itself takes bonds as plain atom
//! index pairs and does not care where they came from.
//!
//! Two atoms are bonded when `0.4 Å <= d <= r_cov(a) + r_cov(b) + 0.45 Å`
//! (the usual tolerance, as in Jmol and OpenBabel), except H–H. Bonds never
//! cross poses.

use std::collections::HashMap;

use mold_mol::{Arena, Element};

use crate::style::covalent_radius;

/// Added to the sum of covalent radii.
pub const BOND_TOLERANCE: f32 = 0.45;
/// Closer than this is an overlap (e.g. a modelling artefact), not a bond.
pub const MIN_BOND_LENGTH: f32 = 0.4;

/// Bonds of every pose of `arena`, as global atom index pairs `[a, b]` with
/// `a < b`, sorted, pose by pose.
pub fn guess_bonds(arena: &Arena) -> Vec<[u32; 2]> {
    let mut bonds = Vec::new();
    for pose in arena.poses() {
        let range = pose.atoms.clone();
        guess_bonds_into(
            &arena.positions()[range.clone()],
            &arena.elements()[range.clone()],
            range.start as u32,
            &mut bonds,
        );
    }
    bonds
}

/// Bonds among `positions` (one pose or any atom set), with indices offset by
/// `offset`, sorted.
pub fn guess_bonds_in(positions: &[[f32; 3]], elements: &[Element], offset: u32) -> Vec<[u32; 2]> {
    let mut bonds = Vec::new();
    guess_bonds_into(positions, elements, offset, &mut bonds);
    bonds
}

fn guess_bonds_into(
    positions: &[[f32; 3]],
    elements: &[Element],
    offset: u32,
    bonds: &mut Vec<[u32; 2]>,
) {
    assert_eq!(positions.len(), elements.len(), "one element per position");
    if positions.len() < 2 {
        return;
    }
    let radii: Vec<f32> = elements.iter().map(|&e| covalent_radius(e)).collect();
    let max_radius = radii.iter().copied().fold(0.0, f32::max);
    // No bond is longer than a cell, so neighbours are in the 27 cells around.
    let cell = 2.0 * max_radius + BOND_TOLERANCE;
    let key = |p: &[f32; 3]| {
        [
            (p[0] / cell).floor() as i32,
            (p[1] / cell).floor() as i32,
            (p[2] / cell).floor() as i32,
        ]
    };
    let mut grid: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    for (i, p) in positions.iter().enumerate() {
        grid.entry(key(p)).or_default().push(i as u32);
    }
    let start = bonds.len();
    for (i, p) in positions.iter().enumerate() {
        let k = key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(cell) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &j in cell {
                        let j = j as usize;
                        if j <= i {
                            continue;
                        }
                        if elements[i] == Element::H && elements[j] == Element::H {
                            continue;
                        }
                        let q = &positions[j];
                        let d2 =
                            (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2);
                        let max = radii[i] + radii[j] + BOND_TOLERANCE;
                        if d2 >= MIN_BOND_LENGTH * MIN_BOND_LENGTH && d2 <= max * max {
                            bonds.push([offset + i as u32, offset + j as u32]);
                        }
                    }
                }
            }
        }
    }
    bonds[start..].sort_unstable();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ethanol_heavy_atoms_and_hydrogens() {
        // C-C 1.52, C-O 1.43, O-H 0.96, two H on C1 ~1.09; H...H 1.78 apart.
        let positions = [
            [0.0, 0.0, 0.0],      // 0 C1
            [1.52, 0.0, 0.0],     // 1 C2
            [2.0, 1.36, 0.0],     // 2 O  (1.44 from C2)
            [2.96, 1.36, 0.0],    // 3 H on O
            [-0.36, 1.03, 0.0],   // 4 H on C1
            [-0.36, -0.51, 0.89], // 5 H on C1
        ];
        let elements = [
            Element::C,
            Element::C,
            Element::O,
            Element::H,
            Element::H,
            Element::H,
        ];
        let bonds = guess_bonds_in(&positions, &elements, 10);
        assert_eq!(
            bonds,
            vec![[10, 11], [10, 14], [10, 15], [11, 12], [12, 13]],
            "no C1-O, no H-H"
        );
    }

    #[test]
    fn too_far_and_too_close_are_not_bonds() {
        let elements = [Element::C, Element::C];
        assert!(guess_bonds_in(&[[0.0; 3], [2.1, 0.0, 0.0]], &elements, 0).is_empty());
        assert!(guess_bonds_in(&[[0.0; 3], [0.2, 0.0, 0.0]], &elements, 0).is_empty());
        assert_eq!(
            guess_bonds_in(&[[0.0; 3], [1.9, 0.0, 0.0]], &elements, 0),
            vec![[0, 1]]
        );
    }

    #[test]
    fn negative_coordinates_and_cell_borders() {
        // Straddle the cell boundary at 0 and a negative one.
        let positions = [[-0.7, -0.1, -5.0], [0.7, 0.1, -5.0], [2.2, 0.1, -5.0]];
        let elements = [Element::C; 3];
        assert_eq!(
            guess_bonds_in(&positions, &elements, 0),
            vec![[0, 1], [1, 2]]
        );
    }

    #[test]
    fn fixture_protein_has_about_one_bond_per_heavy_atom() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/egfr/reference.cif"
        );
        let mut arena = Arena::new();
        mold_mol::io::read_file(&mut arena, path, &Default::default()).unwrap();
        let bonds = guess_bonds(&arena);
        let atoms = arena.atom_count();
        // A protein has ~1.03 bonds per heavy atom (rings add a few).
        let ratio = bonds.len() as f64 / atoms as f64;
        assert!(
            (0.95..1.15).contains(&ratio),
            "{} bonds / {atoms} atoms",
            bonds.len()
        );
        // Every bond is a plausible length.
        for &[a, b] in &bonds {
            let (p, q) = (arena.positions()[a as usize], arena.positions()[b as usize]);
            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
            assert!((0.9..2.1).contains(&d), "bond {a}-{b} is {d} Å");
        }
    }

    #[test]
    fn bonds_stay_inside_their_pose() {
        let mut arena = Arena::new();
        for name in ["a", "b"] {
            let mut pose = arena.begin_pose(name);
            pose.add_chain("A");
            pose.add_residue("LIG", 1, None, true);
            for x in [0.0, 1.5] {
                pose.add_atom(mold_mol::AtomRecord {
                    name: mold_mol::AtomName::new("C").unwrap(),
                    element: Element::C,
                    // Both poses at the same place: only intra-pose bonds count.
                    position: [x, 0.0, 0.0],
                    b_factor: 0.0,
                    occupancy: 1.0,
                    formal_charge: 0,
                });
            }
            pose.finish();
        }
        assert_eq!(guess_bonds(&arena), vec![[0, 1], [2, 3]]);
    }
}
