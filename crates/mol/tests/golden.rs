//! Golden-file parser tests on real RCSB structures (fixtures/structures/README.md).
//! Expected values were derived from the PDB text with awk, independently of pdbtbx.

use std::path::PathBuf;

use mold_mol::io::{self, AltLocPolicy, LoadOptions};
use mold_mol::{Arena, Element, PoseId, Residue};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/structures")
        .join(name)
}

fn load(name: &str, options: &LoadOptions) -> Arena {
    let mut arena = Arena::new();
    let ids = io::read_file(&mut arena, fixture(name), options).expect(name);
    assert_eq!(ids, vec![PoseId(0)], "{name}: one model, one pose");
    arena
}

fn load_default(name: &str) -> Arena {
    load(name, &LoadOptions::default())
}

fn find_residue<'a>(arena: &'a Arena, chain: &str, seq: i32) -> (usize, &'a Residue) {
    arena
        .residues()
        .iter()
        .enumerate()
        .find(|(_, r)| r.seq_num == seq && arena.chains()[r.chain as usize].id == chain)
        .unwrap_or_else(|| panic!("residue {chain} {seq} missing"))
}

/// Global index of the atom `atom` in residue `chain`/`seq`.
fn find_atom(arena: &Arena, chain: &str, seq: i32, atom: &str) -> usize {
    let (_, r) = find_residue(arena, chain, seq);
    r.atoms
        .clone()
        .find(|&i| arena.atom_names()[i] == atom)
        .unwrap_or_else(|| panic!("atom {atom} of {chain} {seq} missing"))
}

fn assert_pos(arena: &Arena, i: usize, expected: [f32; 3]) {
    let p = arena.positions()[i];
    for k in 0..3 {
        assert!(
            (p[k] - expected[k]).abs() < 1e-3,
            "atom {i}: {p:?} != {expected:?}"
        );
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn element_counts(arena: &Arena, r: &Residue) -> Vec<(Element, usize)> {
    let mut counts: Vec<(Element, usize)> = Vec::new();
    for e in &arena.elements()[r.atoms.clone()] {
        match counts.iter_mut().find(|(k, _)| k == e) {
            Some((_, n)) => *n += 1,
            None => counts.push((*e, 1)),
        }
    }
    counts.sort();
    counts
}

// ---- Counts, in both formats --------------------------------------------------

/// (file stem, atoms after altloc policy, residues, chains)
const COUNTS: [(&str, usize, usize, usize); 3] = [
    ("1EJG", 637, 46, 1),
    ("1M17", 2546, 333, 1),
    ("4G5J", 2599, 336, 1),
];

#[test]
fn atom_residue_chain_counts_pdb_and_mmcif() {
    for (id, atoms, residues, chains) in COUNTS {
        for ext in ["pdb", "cif"] {
            let name = format!("{id}.{ext}");
            let arena = load_default(&name);
            assert_eq!(arena.atom_count(), atoms, "{name} atoms");
            assert_eq!(arena.residues().len(), residues, "{name} residues");
            assert_eq!(arena.chains().len(), chains, "{name} chains");
            assert_eq!(arena.pose(PoseId(0)).unwrap().atoms, 0..atoms);
            assert_eq!(arena.pose(PoseId(0)).unwrap().name, id);
            assert!(arena.elements().iter().all(|e| !e.is_unknown()), "{name}");
        }
    }
}

#[test]
fn pdb_and_mmcif_give_identical_arenas() {
    for (id, ..) in COUNTS {
        let a = load_default(&format!("{id}.pdb"));
        let b = load_default(&format!("{id}.cif"));
        assert_eq!(a.atom_names(), b.atom_names(), "{id}");
        assert_eq!(a.elements(), b.elements(), "{id}");
        assert_eq!(a.residue_indices(), b.residue_indices(), "{id}");
        for (r, s) in a.residues().iter().zip(b.residues()) {
            assert_eq!(
                (&r.name, r.seq_num, r.hetero),
                (&s.name, s.seq_num, s.hetero),
                "{id}"
            );
        }
        for (i, (p, q)) in a.positions().iter().zip(b.positions()).enumerate() {
            assert!(dist(*p, *q) < 1e-3, "{id} atom {i}");
        }
        for (i, (p, q)) in a.occupancies().iter().zip(b.occupancies()).enumerate() {
            assert!((p - q).abs() < 1e-3, "{id} atom {i} occupancy");
        }
    }
}

// ---- EGFR + erlotinib (1M17) ---------------------------------------------------

#[test]
fn egfr_erlotinib_known_atoms() {
    for file in ["1M17.pdb", "1M17.cif"] {
        let arena = load_default(file);
        let ca = find_atom(&arena, "A", 769, "CA");
        assert_pos(&arena, ca, [20.091, -1.872, 58.436]);
        assert_eq!(arena.elements()[ca], Element::C);
        assert!((arena.b_factors()[ca] - 41.73).abs() < 1e-3);
        assert_eq!(
            arena.residues()[arena.residue_indices()[ca] as usize].name,
            "MET"
        );

        let (_, lig) = find_residue(&arena, "A", 999);
        assert_eq!(lig.name, "AQ4");
        assert!(lig.hetero);
        assert_eq!(lig.atoms.len(), 29);
        assert_eq!(
            element_counts(&arena, lig),
            vec![(Element::C, 22), (Element::N, 3), (Element::O, 4)]
        );
        let c1 = find_atom(&arena, "A", 999, "C1");
        assert_pos(&arena, c1, [29.552, -1.989, 53.496]);
    }
}

// ---- Covalent ligand: EGFR + afatinib bound to Cys797 (4G5J) --------------------

#[test]
fn covalent_ligand_atoms_present() {
    for file in ["4G5J.pdb", "4G5J.cif"] {
        let arena = load_default(file);
        let (_, lig) = find_residue(&arena, "A", 1101);
        assert_eq!(lig.name, "0WN");
        assert!(lig.hetero);
        assert_eq!(lig.atoms.len(), 34, "{file}: no covalent-ligand atom lost");
        assert_eq!(
            element_counts(&arena, lig),
            vec![
                (Element::C, 24),
                (Element::N, 5),
                (Element::O, 3),
                (Element::F, 1),
                (Element::CL, 1)
            ]
        );
        // Same chain as the protein it is bonded to.
        let (_, cys) = find_residue(&arena, "A", 797);
        assert_eq!(cys.name, "CYS");
        assert_eq!(cys.chain, lig.chain);

        let sg = find_atom(&arena, "A", 797, "SG");
        let c30 = find_atom(&arena, "A", 1101, "C30");
        assert_eq!(arena.elements()[sg], Element::S);
        assert_pos(&arena, sg, [50.463, 7.017, -17.184]);
        assert_pos(&arena, c30, [49.026, 6.397, -18.154]);
        // LINK record: SG-C30 1.84 A.
        let d = dist(arena.positions()[sg], arena.positions()[c30]);
        assert!((d - 1.84).abs() < 0.01, "{file}: SG-C30 = {d}");
        let cl = find_atom(&arena, "A", 1101, "CL1");
        assert_eq!(arena.elements()[cl], Element::CL);

        // The two non-covalent afatinib copies are kept too.
        let n_0wm = arena.residues().iter().filter(|r| r.name == "0WM").count();
        assert_eq!(n_0wm, 2, "{file}");
    }
}

// ---- Alternate locations (1EJG, crambin 0.54 A) --------------------------------

#[test]
fn altloc_highest_occupancy_is_per_residue() {
    for file in ["1EJG.pdb", "1EJG.cif"] {
        let arena = load_default(file);
        // ASP 43: B (0.75) beats A (0.25). Blank atoms N CA C O H HA kept.
        let (_, asp) = find_residue(&arena, "A", 43);
        let names: Vec<&str> = arena.atom_names()[asp.atoms.clone()]
            .iter()
            .map(|n| n.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "N", "CA", "C", "O", "CB", "CG", "OD1", "OD2", "H", "HA", "HB2", "HB3"
            ],
            "{file}: one conformer, file order"
        );
        let cg = find_atom(&arena, "A", 43, "CG");
        assert_pos(&arena, cg, [13.085, 14.014, 18.859]);
        assert!((arena.occupancies()[cg] - 0.75).abs() < 1e-3);
        assert!((arena.b_factors()[cg] - 2.63).abs() < 1e-3);
        // Blank-altloc atom inside an altloc residue keeps its file occupancy.
        let n = find_atom(&arena, "A", 43, "N");
        assert_pos(&arena, n, [14.940, 11.626, 18.615]);
        assert!((arena.occupancies()[n] - 1.0).abs() < 1e-3, "{file}");

        // THR 1: A (0.82) beats B (0.18).
        let n1 = find_atom(&arena, "A", 1, "N");
        assert_pos(&arena, n1, [16.885, 14.078, 3.427]);

        // Microheterogeneity: PRO 22 (A 0.57) over SER 22 (B, C); LEU 25 over ILE 25.
        assert_eq!(find_residue(&arena, "A", 22).1.name, "PRO");
        assert_eq!(find_residue(&arena, "A", 25).1.name, "LEU");

        // No duplicated atom names in any residue: exactly one conformer kept.
        for r in arena.residues() {
            let mut names: Vec<_> = arena.atom_names()[r.atoms.clone()].to_vec();
            names.sort();
            names.dedup();
            assert_eq!(
                names.len(),
                r.atoms.len(),
                "{file}: {} {}",
                r.name,
                r.seq_num
            );
        }
    }
}

#[test]
fn altloc_first_policy() {
    let options = LoadOptions {
        altloc: AltLocPolicy::First,
        ..LoadOptions::default()
    };
    let arena = load("1EJG.pdb", &options);
    let cg = find_atom(&arena, "A", 43, "CG");
    assert_pos(&arena, cg, [13.560, 14.348, 18.472]);
    assert!((arena.occupancies()[cg] - 0.25).abs() < 1e-3);
    assert_eq!(arena.residues().len(), 46);
}
