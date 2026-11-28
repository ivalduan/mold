//! Cross-validation of geom's checks against Python PoseBusters 0.6.5 on the
//! 18 rows of `fixtures/posebusters/reference.csv` (3 EGFR complexes × 6 poses).
//!
//! Tolerances are the ones `fixtures/posebusters/README.md` prescribes:
//! - `rmsd`, `kabsch_rmsd`, `centroid_distance`, `smallest_distance_protein`,
//!   `aromatic_ring_maximum_distance_from_plane`: 1e-3 Å;
//! - `volume_overlap_protein`: 0.05 absolute (PoseBusters integrates on a grid);
//! - boolean verdicts: exact match;
//! - `num_pairwise_clashes_protein`: exact (same vdW radii, same cutoff).
//!
//! Two PoseBusters behaviours the test mirrors rather than "fixes":
//! - **Protein = polymer.** PoseBusters splits the receptor into protein,
//!   cofactors and waters, and HETATM residues (4WKQ's modified Cys, CSX 797)
//!   land in the cofactors. Its `_protein` checks therefore never see them, so
//!   the receptor here is built from ATOM records only. With CSX included the
//!   4WKQ clash pose has 65 clashes and 0.367 overlap instead of 57 and 0.315.
//! - **Signed flatness.** PoseBusters reports, per ring, the largest *signed*
//!   distance from the plane (`_get_distances_to_plane(X).max()`), and the
//!   sign of an SVD normal is arbitrary. geom reports the largest |distance|
//!   (the geometric quantity, and never smaller). The test accepts the
//!   reference if it equals PoseBusters' value for *some* choice of normal
//!   signs over the rings, at 1e-3 Å, and requires ours >= the reference.
//!
//! Empty cells mean "PoseBusters did not run the check" (the `far` rows: no
//! protein atom within the 6 Å search radius). There we assert the reason
//! instead: our smallest distance is beyond the search radius and our volume
//! overlap is zero.
//!
//! `most_extreme_clash_protein` is a descriptor with the opposite polarity of a
//! verdict (README trap 2) and is deliberately not compared. Energies,
//! identity/stereo checks and PoseBusters' `*_relative_*` ratios (which divide
//! by RDKit's distance-geometry bounds, not geom's references) are out of scope.

mod common;

use std::collections::HashMap;
use std::fmt::Write as _;

use mold_geom::ligand::{
    aromatic_ring_flatness, bond_angles, bond_lengths, internal_clashes, plane_distances,
};
use mold_geom::receptor::{
    DistanceParams, Receptor, VolumeParams, protein_ligand_distances, volume_overlap,
};
use mold_geom::rmsd::{centroid_distance, kabsch_rmsd, rmsd};
use mold_geom::{CovalentAttachment, GateConfig, PoseGate, Topology};

const TOL_GEOMETRY: f64 = 1e-3;
const TOL_VOLUME: f64 = 0.05;

const FLOAT_COLUMNS: [(&str, f64); 6] = [
    ("rmsd", TOL_GEOMETRY),
    ("kabsch_rmsd", TOL_GEOMETRY),
    ("centroid_distance", TOL_GEOMETRY),
    ("smallest_distance_protein", TOL_GEOMETRY),
    ("aromatic_ring_maximum_distance_from_plane", TOL_GEOMETRY),
    ("volume_overlap_protein", TOL_VOLUME),
];

const COUNT_COLUMNS: [&str; 1] = ["num_pairwise_clashes_protein"];

/// Every value PoseBusters' signed per-ring maximum can take, over all 2^k
/// choices of normal sign for the k rings.
fn posebusters_flatness_candidates(rings: &[Vec<f64>]) -> Vec<f64> {
    let k = rings.len();
    (0..1u32 << k)
        .map(|mask| {
            rings
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let s = if mask & (1 << i) != 0 { -1.0 } else { 1.0 };
                    d.iter().map(|x| s * x).fold(f64::MIN, f64::max)
                })
                .fold(0.0, f64::max)
        })
        .collect()
}

struct Ours {
    floats: HashMap<&'static str, Option<f64>>,
    bools: HashMap<&'static str, bool>,
    counts: HashMap<&'static str, usize>,
    /// Signed plane distances per aromatic ring.
    ring_planes: Vec<Vec<f64>>,
}

const BOOL_COLUMNS: [&str; 8] = [
    "bond_lengths",
    "bond_angles",
    "internal_steric_clash",
    "aromatic_ring_flatness",
    "minimum_distance_to_protein",
    "protein_ligand_maximum_distance",
    "volume_overlap_with_protein",
    "rmsd_le_2a",
];

/// Our value for every compared column of one row, computed with the raw
/// kernels (no covalent exclusion), exactly as PoseBusters runs them.
fn ours(row: &HashMap<String, String>) -> Ours {
    let cfg = GateConfig::default();
    let pose = common::load_ligand(&row["ligand_file"]);
    let truth = common::load_ligand(&row["true_ligand_file"]);
    let (pp, pe) = common::load_protein_polymer(&row["protein_file"]);
    let receptor = Receptor::new(&pp, &pe);
    let topo = Topology::new(&pose.elements, &pose.bonds).unwrap();

    let params = DistanceParams::default();
    let d = protein_ligand_distances(&receptor, &pose.positions, &pose.elements, &params, &[]);
    let overlap = volume_overlap(
        &receptor,
        &pose.positions,
        &pose.elements,
        &VolumeParams::default(),
    );
    let flat = aromatic_ring_flatness(&topo, &pose.positions, cfg.aromatic_flatness_threshold);
    let r = rmsd(&pose.positions, &truth.positions).unwrap();

    let mut f = HashMap::new();
    f.insert("rmsd", Some(r));
    f.insert(
        "kabsch_rmsd",
        kabsch_rmsd(&pose.positions, &truth.positions),
    );
    f.insert(
        "centroid_distance",
        centroid_distance(&pose.positions, &truth.positions),
    );
    f.insert(
        "smallest_distance_protein",
        d.smallest_distance().map(f64::from),
    );
    f.insert(
        "aromatic_ring_maximum_distance_from_plane",
        Some(flat.max_distance_from_plane),
    );
    f.insert("volume_overlap_protein", Some(overlap));

    let mut b = HashMap::new();
    b.insert(
        "bond_lengths",
        bond_lengths(&topo, &pose.positions, cfg.bond_length_tolerance).pass(),
    );
    b.insert(
        "bond_angles",
        bond_angles(&topo, &pose.positions, cfg.bond_angle_tolerance).pass(),
    );
    b.insert(
        "internal_steric_clash",
        internal_clashes(&topo, &pose.positions, cfg.internal_clash_ratio).pass(),
    );
    b.insert("aromatic_ring_flatness", flat.pass());
    b.insert("minimum_distance_to_protein", d.no_clashes());
    b.insert(
        "protein_ligand_maximum_distance",
        d.within_max_distance(cfg.max_distance_to_protein),
    );
    b.insert(
        "volume_overlap_with_protein",
        overlap <= f64::from(cfg.volume_overlap_cutoff),
    );
    b.insert("rmsd_le_2a", r <= f64::from(cfg.rmsd_threshold));

    let counts = HashMap::from([("num_pairwise_clashes_protein", d.clashes)]);
    let ring_planes = topo
        .aromatic_rings()
        .map(|ring| {
            let pts: Vec<[f32; 3]> = ring.iter().map(|&i| pose.positions[i as usize]).collect();
            plane_distances(&pts)
        })
        .collect();
    Ours {
        floats: f,
        bools: b,
        counts,
        ring_planes,
    }
}

fn parse_bool(s: &str) -> bool {
    match s {
        "True" => true,
        "False" => false,
        other => panic!("not a bool: {other:?}"),
    }
}

#[test]
fn cross_validates_against_posebusters_reference() {
    let rows = common::reference_rows();
    assert_eq!(rows.len(), 18, "3 complexes x 6 poses");
    let search = f64::from(DistanceParams::default().search_distance);
    let mut misses = Vec::new();
    let mut table = String::new();
    for row in &rows {
        let id = format!("{} {}", row["pdb_id"], row["pose"]);
        let o = ours(row);
        let (f, b) = (&o.floats, &o.bools);
        let _ = writeln!(table, "{id}");
        for (col, tol) in FLOAT_COLUMNS {
            let theirs = row[col].as_str();
            let mine = f[col];
            let ok = if theirs.is_empty() {
                // PoseBusters did not run: nothing within its search radius.
                match col {
                    "smallest_distance_protein" => mine.is_some_and(|m| m > search),
                    "volume_overlap_protein" => mine.is_some_and(|m| m == 0.0),
                    _ => false,
                }
            } else if col == "aromatic_ring_maximum_distance_from_plane" {
                let t: f64 = theirs.parse().unwrap();
                posebusters_flatness_candidates(&o.ring_planes)
                    .iter()
                    .any(|c| (c - t).abs() <= tol)
                    && mine.is_some_and(|m| m >= t - tol)
            } else {
                let t: f64 = theirs.parse().unwrap();
                mine.is_some_and(|m| (m - t).abs() <= tol)
            };
            let _ = writeln!(
                table,
                "  {col:<42} ours {:>12} ref {:>12} {}",
                mine.map_or("-".into(), |m| format!("{m:.5}")),
                if theirs.is_empty() { "(empty)" } else { theirs },
                if ok { "ok" } else { "MISS" }
            );
            if !ok {
                misses.push(format!("{id}: {col} ours {mine:?} ref {theirs:?}"));
            }
        }
        for col in COUNT_COLUMNS {
            let theirs: usize = row[col].parse().unwrap();
            let mine = o.counts[col];
            let ok = mine == theirs;
            let _ = writeln!(
                table,
                "  {col:<42} ours {mine:>12} ref {theirs:>12} {}",
                if ok { "ok" } else { "MISS" }
            );
            if !ok {
                misses.push(format!("{id}: {col} ours {mine} ref {theirs}"));
            }
        }
        for col in BOOL_COLUMNS {
            let theirs = parse_bool(&row[col]);
            let mine = b[col];
            let ok = mine == theirs;
            let _ = writeln!(
                table,
                "  {col:<42} ours {mine:>12} ref {theirs:>12} {}",
                if ok { "ok" } else { "MISS" }
            );
            if !ok {
                misses.push(format!("{id}: {col} ours {mine} ref {theirs}"));
            }
        }
    }
    println!("{table}");
    assert!(
        misses.is_empty(),
        "{} misses:\n{}",
        misses.len(),
        misses.join("\n")
    );
}

/// The perturbations are exact rigid translations, so the golden numbers are
/// known without PoseBusters: centroid distance and plain RMSD are exactly the
/// shift, and the superposed RMSD is zero.
#[test]
fn translated_poses_have_exact_golden_numbers() {
    for pdb in ["1M17_AQ4", "4WKQ_IRE", "4G5J_0WN"] {
        let crystal = common::load_ligand(&format!("inputs/{pdb}_crystal.sdf"));
        for (pose, shift) in [("clash", 4.0), ("far", 30.0)] {
            let moved = common::load_ligand(&format!("inputs/{pdb}_{pose}.sdf"));
            let c = centroid_distance(&moved.positions, &crystal.positions).unwrap();
            let r = rmsd(&moved.positions, &crystal.positions).unwrap();
            let k = kabsch_rmsd(&moved.positions, &crystal.positions).unwrap();
            assert!(
                (c - shift).abs() < TOL_GEOMETRY,
                "{pdb} {pose}: centroid {c}"
            );
            assert!((r - shift).abs() < TOL_GEOMETRY, "{pdb} {pose}: rmsd {r}");
            assert!(k < TOL_GEOMETRY, "{pdb} {pose}: kabsch {k}");
        }
    }
}

fn gate_for(pdb: &str, lig: &str, config: GateConfig) -> (PoseGate, common::Ligand) {
    let crystal = common::load_ligand(&format!("inputs/{pdb}_{lig}_crystal.sdf"));
    let (pp, pe) = common::load_protein_polymer(&format!("inputs/{pdb}_protein.pdb"));
    let gate = PoseGate::new(
        Receptor::new(&pp, &pe),
        Topology::new(&crystal.elements, &crystal.bonds).unwrap(),
        config,
    )
    .with_reference(&crystal.positions)
    .unwrap();
    (gate, crystal)
}

/// Each perturbation is aimed at one family; the gate must reject every
/// perturbed pose and accept every crystal pose — including 4G5J's covalent one.
#[test]
fn gate_accepts_crystal_poses_and_rejects_every_breakage() {
    for (pdb, lig) in [("1M17", "AQ4"), ("4WKQ", "IRE"), ("4G5J", "0WN")] {
        let (gate, _) = gate_for(pdb, lig, GateConfig::default());
        for pose in [
            "crystal",
            "clash",
            "far",
            "stretched",
            "internal-clash",
            "puckered",
        ] {
            let m = common::load_ligand(&format!("inputs/{pdb}_{lig}_{pose}.sdf"));
            let report = gate.evaluate(&m.positions, &[]);
            assert_eq!(
                report.pass(),
                pose == "crystal",
                "{pdb} {pose}: {:?}",
                report.verdicts
            );
            if pose == "crystal" {
                assert_eq!(report.verdicts.rmsd_le_2a, Some(true));
            }
        }
    }
}

const CYS797_SG: &str = "SG";

/// Index of Cys797 SG in 4G5J's chain A, in the receptor slices
/// [`common::load_protein_polymer`] returns (ATOM records only).
fn cys797_sg() -> u32 {
    let mut arena = mold_mol::Arena::new();
    mold_mol::io::read_file(
        &mut arena,
        common::posebusters_dir().join("inputs/4G5J_protein.pdb"),
        &mold_mol::LoadOptions::default(),
    )
    .unwrap();
    let res = arena
        .residues()
        .iter()
        .find(|r| r.name == "CYS" && r.seq_num == 797)
        .expect("Cys797");
    let sg = res
        .atoms
        .clone()
        .find(|&i| arena.atom_names()[i] == CYS797_SG)
        .expect("SG");
    let hetero = |i: usize| arena.residues()[arena.residue_indices()[i] as usize].hetero;
    (0..sg).filter(|&i| !hetero(i)).count() as u32
}

/// 4G5J (afatinib, covalent to Cys797): PoseBusters' raw check says the crystal
/// pose clashes (1.84 Å C–S); geom's raw kernel must reproduce that, and the
/// gate must not call the pose invalid — by declaration or by detection.
#[test]
fn covalent_crystal_pose_clashes_raw_but_passes_the_gate() {
    let sg = cys797_sg();
    let no_detect = GateConfig {
        detect_covalent: false,
        ..GateConfig::default()
    };
    let (gate, crystal) = gate_for("4G5J", "0WN", no_detect);

    // Raw, exactly PoseBusters: one clash, the C–S bond, at 1.841 Å.
    let raw = gate.evaluate(&crystal.positions, &[]);
    assert_eq!(raw.protein_raw.clashes, 1);
    let worst = raw.protein_raw.most_extreme.unwrap();
    assert_eq!(worst.protein, sg);
    assert!((worst.distance - 1.84127).abs() < 1e-3);
    assert!(
        !raw.verdicts.minimum_distance_to_protein,
        "no declaration, no detection"
    );
    assert!(!raw.pass());

    // Declared attachment: excluded, gate passes, raw numbers unchanged.
    let declared = CovalentAttachment {
        ligand_atom: worst.ligand,
        protein_atom: sg,
    };
    let r = gate.evaluate(&crystal.positions, &[declared]);
    assert_eq!(r.protein_raw.clashes, 1);
    assert_eq!(r.protein.clashes, 0);
    assert!(r.verdicts.minimum_distance_to_protein);
    assert!(r.pass(), "{:?}", r.verdicts);

    // Detected (the default config): the same attachment is found.
    let (gate, _) = gate_for("4G5J", "0WN", GateConfig::default());
    let r = gate.evaluate(&crystal.positions, &[]);
    assert_eq!(r.covalent, vec![declared]);
    assert!(r.pass(), "{:?}", r.verdicts);

    // Detection never rescues a genuinely clashing pose.
    let clash = common::load_ligand("inputs/4G5J_0WN_clash.sdf");
    let r = gate.evaluate(&clash.positions, &[declared]);
    assert!(!r.verdicts.minimum_distance_to_protein);
}

/// The non-covalent receptors have no plausible covalent pair near the crystal ligand.
#[test]
fn non_covalent_crystal_poses_detect_nothing() {
    for (pdb, lig) in [("1M17", "AQ4"), ("4WKQ", "IRE")] {
        let (gate, crystal) = gate_for(pdb, lig, GateConfig::default());
        let r = gate.evaluate(&crystal.positions, &[]);
        assert!(r.covalent.is_empty(), "{pdb}: {:?}", r.covalent);
        assert_eq!(r.protein_raw.clashes, 0);
    }
}
