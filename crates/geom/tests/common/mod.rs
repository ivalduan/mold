//! Test-only helpers: a minimal V2000 molfile reader, a minimal CSV reader, and
//! fixture loading. Not public API — SDF/molfile reading belongs in `mol` or
//! `chem`; this exists only so geom's tests can load ligands with their bonds.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mold_geom::{Bond, BondOrder};
use mold_mol::{Arena, Element, LoadOptions};

/// A ligand as read from a V2000 molfile: positions, elements, bonds.
#[derive(Debug, Clone)]
pub struct Molfile {
    pub positions: Vec<[f32; 3]>,
    pub elements: Vec<Element>,
    pub bonds: Vec<Bond>,
}

fn field(line: &str, range: std::ops::Range<usize>) -> &str {
    let end = range.end.min(line.len());
    line.get(range.start.min(end)..end).unwrap_or("").trim()
}

/// Read the first record of a V2000 molfile/SDF. Panics on anything malformed:
/// fixtures are trusted, and a loud failure is what a test wants.
pub fn read_molfile(text: &str) -> Molfile {
    let lines: Vec<&str> = text.lines().collect();
    let counts = lines[3];
    assert!(
        counts.contains("V2000"),
        "only V2000 molfiles are supported"
    );
    let n_atoms: usize = field(counts, 0..3).parse().expect("atom count");
    let n_bonds: usize = field(counts, 3..6).parse().expect("bond count");
    let mut m = Molfile {
        positions: Vec::with_capacity(n_atoms),
        elements: Vec::with_capacity(n_atoms),
        bonds: Vec::with_capacity(n_bonds),
    };
    for line in &lines[4..4 + n_atoms] {
        let x: f32 = field(line, 0..10).parse().expect("x");
        let y: f32 = field(line, 10..20).parse().expect("y");
        let z: f32 = field(line, 20..30).parse().expect("z");
        let symbol = field(line, 31..34);
        m.positions.push([x, y, z]);
        m.elements
            .push(Element::from_symbol(symbol).unwrap_or_else(|| panic!("element {symbol:?}")));
    }
    for line in &lines[4 + n_atoms..4 + n_atoms + n_bonds] {
        let a: u32 = field(line, 0..3).parse().expect("bond atom 1");
        let b: u32 = field(line, 3..6).parse().expect("bond atom 2");
        let order = match field(line, 6..9) {
            "1" => BondOrder::Single,
            "2" => BondOrder::Double,
            "3" => BondOrder::Triple,
            "4" => BondOrder::Aromatic,
            other => panic!("unsupported bond type {other:?}"),
        };
        m.bonds.push((a - 1, b - 1, order));
    }
    m
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

pub fn load_ligand(relative: &str) -> Molfile {
    let path = posebusters_dir().join(relative);
    read_molfile(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}")))
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
