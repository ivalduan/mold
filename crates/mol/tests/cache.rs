//! Binary arena cache: round trip on every golden structure and on SDF ligands
//! with bonds, and rejection (never a panic) of truncated, bit-flipped and
//! wrong-version input.

use std::path::PathBuf;

use mold_mol::cache::{self, CacheError};
use mold_mol::io::{self, AltLocPolicy, LoadOptions};
use mold_mol::{Arena, sdf};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn structure(name: &str, options: &LoadOptions) -> Arena {
    let mut arena = Arena::new();
    io::read_file(
        &mut arena,
        fixtures().join("structures").join(name),
        options,
    )
    .unwrap();
    arena
}

fn ligands() -> Arena {
    let mut arena = Arena::new();
    for name in ["1M17_AQ4_crystal", "4G5J_0WN_crystal", "4WKQ_IRE_far"] {
        let path = fixtures().join(format!("posebusters/inputs/{name}.sdf"));
        let read = sdf::read_sdf_file(&mut arena, path).unwrap();
        assert!(read.errors.is_empty());
    }
    assert_eq!(arena.bonds().len(), 31 + 37 + 34);
    arena
}

fn to_bytes(arena: &Arena) -> Vec<u8> {
    let mut buf = Vec::new();
    cache::save(arena, &mut buf).unwrap();
    buf
}

/// Field by field, positions and floats compared bit for bit.
fn assert_identical(a: &Arena, b: &Arena, what: &str) {
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(a.positions_flat()),
        bits(b.positions_flat()),
        "{what}: positions"
    );
    assert_eq!(bits(a.b_factors()), bits(b.b_factors()), "{what}: B");
    assert_eq!(
        bits(a.occupancies()),
        bits(b.occupancies()),
        "{what}: occupancy"
    );
    assert_eq!(a.elements(), b.elements(), "{what}: elements");
    assert_eq!(a.atom_names(), b.atom_names(), "{what}: names");
    assert_eq!(a.residue_indices(), b.residue_indices(), "{what}");
    assert_eq!(a.chain_indices(), b.chain_indices(), "{what}");
    assert_eq!(a.formal_charges(), b.formal_charges(), "{what}");
    assert_eq!(a.residues(), b.residues(), "{what}: residues");
    assert_eq!(a.chains(), b.chains(), "{what}: chains");
    assert_eq!(a.poses(), b.poses(), "{what}: poses");
    assert_eq!(a.bonds(), b.bonds(), "{what}: bonds");
    for id in a.pose_ids() {
        assert_eq!(a.pose_bonds(id), b.pose_bonds(id), "{what}: pose bonds");
    }
    assert_eq!(a, b, "{what}");
}

fn golden_arenas() -> Vec<(&'static str, Arena)> {
    let first = LoadOptions {
        altloc: AltLocPolicy::First,
        ..LoadOptions::default()
    };
    let mut everything = structure("1M17.cif", &LoadOptions::default());
    io::read_file(
        &mut everything,
        fixtures().join("structures/4G5J.pdb"),
        &LoadOptions::default(),
    )
    .unwrap();
    for name in ["1M17_AQ4_crystal", "4G5J_0WN_crystal"] {
        sdf::read_sdf_file(
            &mut everything,
            fixtures().join(format!("posebusters/inputs/{name}.sdf")),
        )
        .unwrap();
    }
    vec![
        (
            "1EJG altlocs",
            structure("1EJG.cif", &LoadOptions::default()),
        ),
        ("1EJG first altloc", structure("1EJG.pdb", &first)),
        ("1M17", structure("1M17.cif", &LoadOptions::default())),
        (
            "4G5J covalent",
            structure("4G5J.cif", &LoadOptions::default()),
        ),
        ("SDF ligands", ligands()),
        ("mixed", everything),
        ("empty", Arena::new()),
    ]
}

#[test]
fn round_trip_is_identical() {
    for (what, arena) in golden_arenas() {
        let back = cache::load(to_bytes(&arena).as_slice()).unwrap();
        assert_identical(&arena, &back, what);
    }
}

#[test]
fn round_trip_through_a_file() {
    let arena = ligands();
    let dir = std::env::temp_dir().join(format!("mold-mol-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("ligands.{}", cache::EXTENSION));
    cache::save_file(&arena, &path).unwrap();
    let back = cache::load_file(&path).unwrap();
    assert_identical(&arena, &back, "file");
    // A file that is not a cache.
    let not = fixtures().join("structures/1M17.cif");
    assert!(matches!(cache::load_file(not), Err(CacheError::NotACache)));
    assert!(matches!(
        cache::load_file(dir.join("missing")),
        Err(CacheError::Io(_))
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn every_truncation_is_an_error() {
    let good = to_bytes(&ligands());
    for len in 0..good.len() {
        let result = cache::load(&good[..len]);
        assert!(result.is_err(), "truncated to {len} bytes loaded");
        assert!(
            matches!(result, Err(CacheError::Truncated | CacheError::NotACache)),
            "truncated to {len}: {result:?}"
        );
    }
    // load_file sees the same truncations through the file-size path.
    let dir = std::env::temp_dir().join(format!("mold-mol-trunc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.moldarena");
    for len in [0, 10, 59, 60, 61, good.len() / 2, good.len() - 1] {
        std::fs::write(&path, &good[..len]).unwrap();
        assert!(cache::load_file(&path).is_err(), "file truncated to {len}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// SplitMix64: deterministic positions for the bit-flip loop.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[test]
fn bit_flips_are_errors() {
    let good = to_bytes(&ligands());
    // Every bit of the 60-byte header, then 2,000 random payload bits.
    let mut positions: Vec<usize> = (0..60 * 8).collect();
    let mut state = 20261127;
    positions.extend(
        (0..2000).map(|_| 60 * 8 + (splitmix(&mut state) as usize) % ((good.len() - 60) * 8)),
    );
    for bit in positions {
        let mut bad = good.clone();
        bad[bit / 8] ^= 1 << (bit % 8);
        let result = cache::load(bad.as_slice());
        assert!(result.is_err(), "flipping bit {bit} still loaded");
        if bit >= 60 * 8 {
            assert!(
                matches!(result, Err(CacheError::Checksum)),
                "bit {bit}: {result:?}"
            );
        }
    }
}

#[test]
fn wrong_version_and_extra_bytes() {
    let good = to_bytes(&ligands());
    for version in [0u32, 2, 1 << 16, u32::MAX] {
        let mut bad = good.clone();
        bad[16..20].copy_from_slice(&version.to_le_bytes());
        match cache::load(bad.as_slice()) {
            Err(CacheError::Version { found, supported }) => {
                assert_eq!((found, supported), (version, cache::FORMAT_VERSION));
            }
            other => panic!("version {version}: {other:?}"),
        }
    }
    // A header claiming a huge payload over a short stream: truncated, no huge allocation.
    let mut bad = good.clone();
    bad[20..28].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(
        cache::load(bad.as_slice()),
        Err(CacheError::Truncated)
    ));
    // Bytes after the payload are ignored by `load` (it reads what the header
    // declares), so a cache can be embedded in a larger stream.
    let mut longer = good.clone();
    longer.extend_from_slice(b"trailing");
    assert!(cache::load(longer.as_slice()).is_ok());
    // The error message says what to do.
    let mut bad = good.clone();
    bad[16..20].copy_from_slice(&9u32.to_le_bytes());
    let msg = cache::load(bad.as_slice()).unwrap_err().to_string();
    assert!(msg.contains("rebuild"), "{msg}");
}
