//! SDF/molfile golden tests on fixtures/posebusters/inputs/*.sdf and
//! fixtures/egfr/reference_ligand.sdf, plus multi-record error isolation and
//! a write → read round trip.
//!
//! Expected counts are taken from the raw text by `raw_counts`, which only looks
//! at line shapes (an atom line's fourth token is an element symbol; bond lines
//! run from the atom block to `M  END`), never through the parser under test.
//! Spot values were copied from the files by hand (line numbers in comments).

use std::path::{Path, PathBuf};

use mold_mol::sdf::{self, SdfError};
use mold_mol::{Arena, Bond, BondOrder, Element, PoseId};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn posebusters_sdfs() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(fixtures().join("posebusters/inputs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sdf"))
        .collect();
    files.sort();
    files
}

/// (atom lines, bond lines) of the first record, by line shape only.
fn raw_counts(text: &str) -> (usize, usize) {
    let mut atoms = 0;
    let mut bonds = 0;
    for line in text.lines().skip(4) {
        if line.starts_with("M  END") {
            break;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() >= 4 && tokens[3].chars().all(|c| c.is_ascii_alphabetic()) {
            atoms += 1;
        } else if tokens.len() >= 3 && tokens.iter().all(|t| t.parse::<i32>().is_ok()) {
            bonds += 1;
        }
    }
    (atoms, bonds)
}

fn read(path: &Path) -> (Arena, sdf::SdfRead) {
    let mut arena = Arena::new();
    let read = sdf::read_sdf_file(&mut arena, path).unwrap();
    (arena, read)
}

fn bond(arena: &Arena, i: usize) -> Bond {
    arena.pose_bonds(PoseId(0)).unwrap()[i]
}

#[test]
fn every_posebusters_sdf_matches_its_raw_counts() {
    let files = posebusters_sdfs();
    assert_eq!(files.len(), 18);
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let (atoms, bonds) = raw_counts(&text);
        let (arena, read) = read(&path);
        let what = path.display();
        assert!(read.errors.is_empty(), "{what}: {:?}", read.errors);
        assert_eq!(read.poses(), vec![PoseId(0)], "{what}");
        assert_eq!(arena.atom_count(), atoms, "{what}: atoms");
        assert_eq!(arena.pose_bonds(PoseId(0)).unwrap().len(), bonds, "{what}");
        assert_eq!(read.records[0].title, text.lines().next().unwrap().trim());
        assert!(arena.elements().iter().all(|e| !e.is_unknown()), "{what}");
    }
}

#[test]
fn spot_values_1m17() {
    let (arena, _) = read(&fixtures().join("posebusters/inputs/1M17_AQ4_crystal.sdf"));
    assert_eq!((arena.atom_count(), arena.bonds().len()), (29, 31));
    // Line 5 and line 33 (atom 29).
    assert_eq!(arena.positions()[0], [29.552, -1.989, 53.496]);
    assert_eq!(arena.positions()[28], [28.508, 1.41, 53.344]);
    // "  1  2  3  0" and the last bond " 28 29  2  0".
    assert_eq!(
        bond(&arena, 0),
        Bond {
            a: 0,
            b: 1,
            order: BondOrder::Triple
        }
    );
    assert_eq!(
        bond(&arena, 30),
        Bond {
            a: 27,
            b: 28,
            order: BondOrder::Double
        }
    );
    assert_eq!(arena.poses()[0].name, "1M17_AQ4");
}

#[test]
fn spot_values_4g5j() {
    let (arena, _) = read(&fixtures().join("posebusters/inputs/4G5J_0WN_crystal.sdf"));
    assert_eq!((arena.atom_count(), arena.bonds().len()), (34, 37));
    assert_eq!(arena.positions()[0], [53.179, -1.616, -21.729]);
    // Atom 19: "   51.9050   -2.1620  -27.2040 Cl".
    assert_eq!(arena.positions()[18], [51.905, -2.162, -27.204]);
    assert_eq!(arena.elements()[18], Element::CL);
    assert_eq!(arena.atom_names()[18], "CL1");
    assert_eq!(arena.elements()[17], Element::F);
    assert_eq!(
        bond(&arena, 0),
        Bond {
            a: 0,
            b: 1,
            order: BondOrder::Double
        }
    );
    // " 21 20  1  6": order kept as written (a = 21, b = 20), stereo ignored.
    assert_eq!(
        bond(&arena, 23),
        Bond {
            a: 20,
            b: 19,
            order: BondOrder::Single
        }
    );
    assert!(arena.formal_charges().iter().all(|&c| c == 0));
}

#[test]
fn spot_values_4wkq_and_reference_properties() {
    let (arena, _) = read(&fixtures().join("posebusters/inputs/4WKQ_IRE_crystal.sdf"));
    assert_eq!((arena.atom_count(), arena.bonds().len()), (31, 34));
    assert_eq!(arena.positions()[0], [6.423, 190.308, 17.402]);
    assert_eq!(arena.positions()[30], [1.887, 195.846, 24.849]);
    assert_eq!(
        bond(&arena, 0),
        Bond {
            a: 0,
            b: 1,
            order: BondOrder::Single
        }
    );
    assert_eq!(
        bond(&arena, 33),
        Bond {
            a: 29,
            b: 30,
            order: BondOrder::Double
        }
    );

    let (arena, read) = read(&fixtures().join("egfr/reference_ligand.sdf"));
    assert_eq!((arena.atom_count(), arena.bonds().len()), (31, 34));
    let r = &read.records[0];
    assert_eq!(r.title, "4WKQ_IRE");
    let names: Vec<&str> = r.properties.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["pdb_id", "comp_id", "smiles"]);
    assert_eq!(r.property("comp_id"), Some("IRE"));
    assert_eq!(
        r.property("smiles"),
        Some("COc1cc2ncnc(Nc3ccc(F)c(Cl)c3)c2cc1OCCCN1CCOCC1")
    );
}

fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixtures().join("posebusters/inputs").join(name)).unwrap()
}

#[test]
fn bad_records_are_reported_and_good_ones_load() {
    let good_a = fixture_text("1M17_AQ4_crystal.sdf");
    let good_b = fixture_text("4G5J_0WN_crystal.sdf");
    let good_c = fixture_text("4WKQ_IRE_crystal.sdf");
    // Record 1: a bond names atom 99. Record 3: the atom block is cut short.
    let bad_bond = good_b.replacen("  1  2  2  0", " 99  2  2  0", 1);
    let truncated: String = good_c.lines().take(10).map(|l| format!("{l}\n")).collect();
    let text = format!("{good_a}{bad_bond}{good_c}{truncated}$$$$\n{good_b}");

    let mut arena = Arena::new();
    let read = sdf::read_sdf_str(&mut arena, &text, "series");
    let indices: Vec<usize> = read.records.iter().map(|r| r.index).collect();
    assert_eq!(indices, [0, 2, 4]);
    assert_eq!(read.poses(), [PoseId(0), PoseId(1), PoseId(2)]);
    assert_eq!(arena.atom_count(), 29 + 31 + 34);
    assert_eq!(arena.bonds().len(), 31 + 34 + 37);

    assert_eq!(read.errors.len(), 2);
    assert_eq!(read.errors[0].index, 1);
    match &read.errors[0].error {
        SdfError::Malformed { line, message } => {
            // Record 1 starts on the line after record 0's `$$$$`.
            let start = good_a.lines().count() + 1;
            assert_eq!(*line, start + 4 + 34, "first bond line of record 1");
            assert!(message.contains("atom 99"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(read.errors[1].index, 3);
    assert!(matches!(read.errors[1].error, SdfError::Malformed { .. }));
    // Every pose's bonds stay inside its own atoms.
    for id in arena.pose_ids() {
        let atoms = arena.pose(id).unwrap().atoms.clone();
        for b in arena.pose_bonds(id).unwrap() {
            assert!(atoms.contains(&(b.a as usize)) && atoms.contains(&(b.b as usize)));
        }
    }
}

#[test]
fn empty_record_between_delimiters_is_reported() {
    let good = fixture_text("1M17_AQ4_crystal.sdf");
    let text = format!("{good}$$$$\n{good}\n\n");
    let mut arena = Arena::new();
    let read = sdf::read_sdf_str(&mut arena, &text, "x");
    assert_eq!(read.records.len(), 2);
    assert_eq!(read.errors.len(), 1);
    assert_eq!(
        (read.errors[0].index, &read.errors[0].error),
        (1, &SdfError::Empty)
    );
}

#[test]
fn molfile_without_delimiter_and_crlf() {
    let good = fixture_text("4WKQ_IRE_crystal.sdf");
    let molfile = good.replace("$$$$\n", "").replace('\n', "\r\n");
    let mut arena = Arena::new();
    let read = sdf::read_sdf_str(&mut arena, &molfile, "m");
    assert!(read.errors.is_empty(), "{:?}", read.errors);
    assert_eq!(arena.atom_count(), 31);
    assert_eq!(arena.bonds().len(), 34);
}

#[test]
fn write_then_read_round_trips() {
    let mut first = Arena::new();
    let mut props = Vec::new();
    for name in ["1M17_AQ4_crystal.sdf", "4G5J_0WN_crystal.sdf"] {
        sdf::read_sdf_str(&mut first, &fixture_text(name), "x");
    }
    let r = sdf::read_sdf_file(&mut first, fixtures().join("egfr/reference_ligand.sdf")).unwrap();
    props.push(Vec::new());
    props.push(vec![("ligand_id".to_string(), "L-0002".to_string())]);
    props.push(r.records[0].properties.clone());

    // Give one atom a charge so M  CHG is exercised.
    let mut charged = Arena::new();
    let mut text = Vec::new();
    for id in first.pose_ids() {
        sdf::write_sdf(&first, id, &props[id.0 as usize], &mut text).unwrap();
    }
    let text =
        String::from_utf8(text)
            .unwrap()
            .replacen("M  END", "M  CHG  2   1   1   2  -1\nM  END", 1);
    let back = sdf::read_sdf_str(&mut charged, &text, "rt");
    assert!(back.errors.is_empty(), "{:?}", back.errors);
    assert_eq!(charged.atom_count(), first.atom_count());
    assert_eq!(charged.bonds(), first.bonds());
    assert_eq!(charged.elements(), first.elements());
    assert_eq!(charged.atom_names(), first.atom_names());
    // 4 decimals written; f32 cannot hold every 7-digit decimal exactly.
    for (p, q) in charged.positions().iter().zip(first.positions()) {
        assert!((0..3).all(|k| (p[k] - q[k]).abs() < 1e-4), "{p:?} vs {q:?}");
    }
    assert_eq!(&charged.formal_charges()[..3], &[1, -1, 0]);
    for (i, r) in back.records.iter().enumerate() {
        assert_eq!(r.title, first.poses()[i].name);
        assert_eq!(r.properties, props[i]);
    }

    // Charges written by write_sdf come back too.
    let mut text = Vec::new();
    sdf::write_sdf(&charged, PoseId(0), &[], &mut text).unwrap();
    let mut again = Arena::new();
    sdf::read_sdf_str(&mut again, std::str::from_utf8(&text).unwrap(), "rt");
    assert_eq!(&again.formal_charges()[..3], &[1, -1, 0]);
}

#[test]
fn write_refuses_oversized_poses_and_missing_ids() {
    let mut arena = Arena::new();
    mold_mol::io::read_file(
        &mut arena,
        fixtures().join("structures/1M17.cif"),
        &mold_mol::LoadOptions::default(),
    )
    .unwrap();
    assert!(matches!(
        sdf::write_sdf(&arena, PoseId(0), &[], Vec::new()),
        Err(mold_mol::IoError::Write(_))
    ));
    assert!(matches!(
        sdf::write_sdf(&arena, PoseId(5), &[], Vec::new()),
        Err(mold_mol::IoError::NoSuchPose(PoseId(5)))
    ));
}
