//! Test-only helpers: fixture loading (ligands through `mold_mol::sdf`,
//! proteins through `mold_mol::io`), a minimal CSV reader, and seeded poses.
//! Not public API.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mold_geom::Bond;
use mold_mol::{Arena, Element, LoadOptions};

/// A ligand as `mold_mol::sdf` reads it: positions, elements and pose-local bonds.
#[derive(Debug, Clone)]
pub struct Ligand {
    pub positions: Vec<[f32; 3]>,
    pub elements: Vec<Element>,
    pub bonds: Vec<Bond>,
}

/// The first record of an SDF file, through mol's reader. Panics on anything
/// malformed: fixtures are trusted, and a loud failure is what a test wants.
pub fn read_ligand(path: &Path) -> Ligand {
    let mut arena = Arena::new();
    let read =
        mold_mol::sdf::read_sdf_file(&mut arena, path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    assert!(read.errors.is_empty(), "{path:?}: {:?}", read.errors);
    let pose = read.records[0].pose;
    let range = arena.pose(pose).expect("pose").atoms.clone();
    Ligand {
        positions: arena.positions()[range.clone()].to_vec(),
        elements: arena.elements()[range.clone()].to_vec(),
        bonds: arena
            .pose_bonds(pose)
            .expect("bonds")
            .iter()
            .map(|b| b.relative_to(range.start))
            .collect(),
    }
}

/// Parse CSV with a header row into one map per record. Handles quoted fields
/// (embedded commas, `""` escapes); no embedded newlines.
pub fn read_csv(text: &str) -> Vec<HashMap<String, String>> {
    let mut lines = text.lines().filter(|l| !l.is_empty());
    let header = split_csv_line(lines.next().expect("header"));
    lines
        .map(|l| {
            let cells = split_csv_line(l);
            assert_eq!(cells.len(), header.len(), "ragged CSV row: {l}");
            header.iter().cloned().zip(cells).collect()
        })
        .collect()
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(std::mem::take(&mut cur)),
            (c, _) => cur.push(c),
        }
    }
    out.push(cur);
    out
}

pub fn posebusters_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/posebusters")
}

pub fn load_ligand(relative: &str) -> Ligand {
    read_ligand(&posebusters_dir().join(relative))
}

/// A protein PDB loaded through `mold_mol::io`: every atom of the single pose.
pub fn load_protein(relative: &str) -> (Vec<[f32; 3]>, Vec<Element>) {
    let mut arena = Arena::new();
    let ids = mold_mol::io::read_file(
        &mut arena,
        posebusters_dir().join(relative),
        &LoadOptions::default(),
    )
    .expect("protein loads");
    let pose = arena.pose_positions(ids[0]).expect("pose");
    let range = arena.pose(ids[0]).expect("pose").atoms.clone();
    (pose.to_vec(), arena.elements()[range].to_vec())
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// A receptor PDB and a ligand SDF (paths relative to `fixtures/posebusters`)
/// read into one arena: `(arena, receptor pose, ligand pose)`.
pub fn load_complex(protein: &str, ligand: &str) -> (Arena, mold_mol::PoseId, mold_mol::PoseId) {
    let mut arena = Arena::new();
    let ids = mold_mol::io::read_file(
        &mut arena,
        posebusters_dir().join(protein),
        &LoadOptions::default(),
    )
    .expect("protein loads");
    let read = mold_mol::sdf::read_sdf_file(&mut arena, posebusters_dir().join(ligand))
        .expect("ligand loads");
    assert!(read.errors.is_empty(), "{:?}", read.errors);
    (arena, ids[0], read.records[0].pose)
}

/// The protein as PoseBusters' `_protein` checks see it: ATOM records only.
/// PoseBusters files HETATM residues (e.g. 4WKQ's CSX 797) under cofactors.
pub fn load_protein_polymer(relative: &str) -> (Vec<[f32; 3]>, Vec<Element>) {
    let mut arena = Arena::new();
    let ids = mold_mol::io::read_file(
        &mut arena,
        posebusters_dir().join(relative),
        &LoadOptions::default(),
    )
    .expect("protein loads");
    let range = arena.pose(ids[0]).expect("pose").atoms.clone();
    range
        .filter(|&i| !arena.residues()[arena.residue_indices()[i] as usize].hetero)
        .map(|i| (arena.positions()[i], arena.elements()[i]))
        .unzip()
}

pub fn reference_rows() -> Vec<HashMap<String, String>> {
    read_csv(&std::fs::read_to_string(posebusters_dir().join("reference.csv")).unwrap())
}

/// SplitMix64: a tiny seeded generator, so the bench and the budget test need
/// no RNG dependency and generate identical poses on every platform.
#[derive(Debug, Clone)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[-1, 1)`.
    pub fn symmetric(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// `n` docking-like poses of `crystal`: each a random rotation of up to ~35°
/// about the ligand centroid, a translation of up to 2 Å per axis, and 0.15 Å
/// of per-atom jitter. Deterministic for a given `seed`.
pub fn perturbed_poses(crystal: &[[f32; 3]], n: usize, seed: u64) -> Vec<Vec<[f32; 3]>> {
    let mut rng = SplitMix64(seed);
    let c = crystal.len() as f64;
    let centroid: [f64; 3] =
        std::array::from_fn(|k| crystal.iter().map(|p| f64::from(p[k])).sum::<f64>() / c);
    (0..n)
        .map(|_| {
            // Small-angle quaternion: w = 1, vector part up to 0.3 per axis.
            let v = [
                0.3 * rng.symmetric(),
                0.3 * rng.symmetric(),
                0.3 * rng.symmetric(),
            ];
            let norm = (1.0 + v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let (w, x, y, z) = (1.0 / norm, v[0] / norm, v[1] / norm, v[2] / norm);
            let m = [
                [
                    1.0 - 2.0 * (y * y + z * z),
                    2.0 * (x * y - w * z),
                    2.0 * (x * z + w * y),
                ],
                [
                    2.0 * (x * y + w * z),
                    1.0 - 2.0 * (x * x + z * z),
                    2.0 * (y * z - w * x),
                ],
                [
                    2.0 * (x * z - w * y),
                    2.0 * (y * z + w * x),
                    1.0 - 2.0 * (x * x + y * y),
                ],
            ];
            let t = [
                2.0 * rng.symmetric(),
                2.0 * rng.symmetric(),
                2.0 * rng.symmetric(),
            ];
            crystal
                .iter()
                .map(|p| {
                    let d: [f64; 3] = std::array::from_fn(|k| f64::from(p[k]) - centroid[k]);
                    std::array::from_fn(|i| {
                        let r = m[i][0] * d[0] + m[i][1] * d[1] + m[i][2] * d[2];
                        (centroid[i] + t[i] + r + 0.15 * rng.symmetric()) as f32
                    })
                })
                .collect()
        })
        .collect()
}
