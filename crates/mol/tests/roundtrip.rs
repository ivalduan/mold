//! Round-trip: read -> arena -> write -> read gives the same atoms.

use std::path::PathBuf;

use mold_mol::io::{self, Format, LoadOptions};
use mold_mol::{Arena, PoseId};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/structures")
        .join(name)
}

fn assert_same_atoms(a: &Arena, b: &Arena, what: &str) {
    assert_eq!(a.atom_count(), b.atom_count(), "{what}: atom count");
    assert_eq!(a.atom_names(), b.atom_names(), "{what}: names");
    assert_eq!(a.elements(), b.elements(), "{what}: elements");
    assert_eq!(
        a.residue_indices(),
        b.residue_indices(),
        "{what}: residue index"
    );
    assert_eq!(a.chain_indices(), b.chain_indices(), "{what}: chain index");
    assert_eq!(a.formal_charges(), b.formal_charges(), "{what}: charges");
    assert_eq!(a.residues().len(), b.residues().len(), "{what}: residues");
    for (r, s) in a.residues().iter().zip(b.residues()) {
        assert_eq!(
            (&r.name, r.seq_num, r.insertion_code, r.hetero, &r.atoms),
            (&s.name, s.seq_num, s.insertion_code, s.hetero, &s.atoms),
            "{what}: residue"
        );
    }
    for (c, d) in a.chains().iter().zip(b.chains()) {
        assert_eq!(c.id, d.id, "{what}: chain id");
    }
    let close = |x: f32, y: f32, tol: f32| (x - y).abs() <= tol;
    for i in 0..a.atom_count() {
        let (p, q) = (a.positions()[i], b.positions()[i]);
        assert!(
            (0..3).all(|k| close(p[k], q[k], 1e-3)),
            "{what}: atom {i} {p:?} vs {q:?}"
        );
        assert!(
            close(a.b_factors()[i], b.b_factors()[i], 1e-2),
            "{what}: B atom {i}"
        );
        assert!(
            close(a.occupancies()[i], b.occupancies()[i], 1e-2),
            "{what}: occ atom {i}"
        );
    }
}

fn round_trip(file: &str, format: Format) {
    let opts = LoadOptions::default();
    let mut first = Arena::new();
    let ids = io::read_file(&mut first, fixture(file), &opts).unwrap();
    let mut buf = Vec::new();
    io::write_to(&first, ids[0], format, &mut buf).unwrap();

    let mut second = Arena::new();
    let ids2 = io::read_from(&mut second, buf.as_slice(), format, "rt", &opts).unwrap();
    assert_eq!(ids2, vec![PoseId(0)]);
    assert_same_atoms(&first, &second, &format!("{file} -> {format:?}"));
}

#[test]
fn mmcif_round_trip() {
    for id in ["1M17", "4G5J", "1EJG"] {
        round_trip(&format!("{id}.cif"), Format::Mmcif);
    }
}

#[test]
fn pdb_round_trip() {
    for id in ["1M17", "4G5J", "1EJG"] {
        round_trip(&format!("{id}.pdb"), Format::Pdb);
    }
}

#[test]
fn write_file_picks_format_from_extension() {
    let mut arena = Arena::new();
    io::read_file(&mut arena, fixture("4G5J.cif"), &LoadOptions::default()).unwrap();
    let dir = std::env::temp_dir().join(format!("mold-mol-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for ext in ["cif", "pdb"] {
        let path = dir.join(format!("out.{ext}"));
        io::write_file(&arena, PoseId(0), &path).unwrap();
        let mut back = Arena::new();
        io::read_file(&mut back, &path, &LoadOptions::default()).unwrap();
        assert_same_atoms(&arena, &back, ext);
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(matches!(
        io::write_file(&arena, PoseId(0), dir.join("x.sdf")),
        Err(io::IoError::UnknownFormat(_))
    ));
    assert!(matches!(
        io::write_to(&arena, PoseId(9), Format::Pdb, Vec::new()),
        Err(io::IoError::NoSuchPose(PoseId(9)))
    ));
}

#[test]
fn each_model_becomes_a_pose() {
    let mut one = Arena::new();
    io::read_file(&mut one, fixture("1EJG.pdb"), &LoadOptions::default()).unwrap();
    let mut model = Vec::new();
    io::write_to(&one, PoseId(0), Format::Pdb, &mut model).unwrap();
    let atoms = String::from_utf8(model).unwrap().replace("END\n", "");
    let text = format!("MODEL        1\n{atoms}ENDMDL\nMODEL        2\n{atoms}ENDMDL\nEND\n");

    let mut arena = Arena::new();
    let ids = io::read_from(
        &mut arena,
        text.as_bytes(),
        Format::Pdb,
        "nmr",
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(ids, vec![PoseId(0), PoseId(1)]);
    assert_eq!(arena.poses()[1].name, "nmr model 2");
    assert_eq!(arena.atom_count(), 2 * one.atom_count());
    assert_eq!(arena.pose_positions(PoseId(1)).unwrap(), one.positions());

    let first_only = LoadOptions {
        first_model_only: true,
        ..LoadOptions::default()
    };
    let mut arena = Arena::new();
    let ids = io::read_from(&mut arena, text.as_bytes(), Format::Pdb, "nmr", &first_only).unwrap();
    assert_eq!(ids, vec![PoseId(0)]);
}
