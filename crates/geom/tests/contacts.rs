//! Contact profiling on the three EGFR complexes of the PoseBusters fixture set:
//! literature interactions, PLIP cross-check, and properties.
//!
//! Sources for the expected interactions:
//! - Hinge H-bond of 4-anilinoquinazolines: quinazoline N1 accepts an H-bond
//!   from the backbone NH of Met793 (Met769 in 1M17's older numbering).
//!   Erlotinib/1M17: Stamos, Sliwkowski & Eigenbrot, J. Biol. Chem. 277,
//!   46265 (2002). Gefitinib: Yun et al., Cancer Cell 11, 217 (2007).
//!   Afatinib/4G5J: Solca et al., J. Pharmacol. Exp. Ther. 343, 342 (2012),
//!   which also describes the covalent bond to Cys797.
//! - Quinazoline N3 binds Thr790 (Thr766 in 1M17) only through a water
//!   (Stamos 2002; Yun 2007). The fixture receptors carry no waters, so there
//!   is no direct N3–Thr790 H-bond to find.
//! - No aromatic side chain stacks on the quinazoline: it sits between
//!   Leu718/Val726 above and Leu844 below (Stamos 2002, Yun 2007), so there
//!   is no π-stacking in any of the three crystal poses.
//! - The basic solubilising groups (gefitinib's morpholine, afatinib's
//!   dimethylamine) point to the solvent front by Asp800; PLIP reports a salt
//!   bridge to Asp800 for both (fixtures/plip/reference.csv).

mod common;

use std::collections::BTreeSet;

use mold_geom::contacts::{CationPi, ContactConfig, Contacts, PiStack, Side, contacts};
use mold_geom::{Receptor, Topology};
use mold_mol::{Arena, PoseId};
use proptest::prelude::*;

/// A receptor + one ligand pose, both from the fixture files, read through mol.
struct Complex {
    arena: Arena,
    receptor_pose: PoseId,
    ligand_pose: PoseId,
    receptor: Receptor,
    topology: Topology,
}

impl Complex {
    fn load(pdb: &str, lig: &str, pose: &str) -> Complex {
        let (arena, receptor_pose, ligand_pose) = common::load_complex(
            &format!("inputs/{pdb}_protein.pdb"),
            &format!("inputs/{pdb}_{lig}_{pose}.sdf"),
        );
        let receptor = Receptor::from_arena(&arena, receptor_pose).expect("receptor pose");
        let topology = Topology::from_arena(&arena, ligand_pose).unwrap();
        Complex {
            arena,
            receptor_pose,
            ligand_pose,
            receptor,
            topology,
        }
    }

    fn positions(&self) -> &[[f32; 3]] {
        self.arena.pose_positions(self.ligand_pose).unwrap()
    }

    fn contacts(&self) -> Contacts {
        contacts(
            &self.receptor,
            &self.topology,
            self.positions(),
            &ContactConfig::default(),
        )
    }

    /// "A:MET:793 N" for a receptor atom of a contact.
    fn receptor_atom(&self, atom: u32) -> String {
        format!(
            "{} {}",
            self.receptor.residue_of(atom).expect("typed receptor"),
            self.receptor.atom_name(atom).expect("typed receptor")
        )
    }
}

/// Quinazoline N1: the aromatic ring N bonded to a ring-fusion atom (C8a).
/// N3 sits between C2 and C4, neither of which is a fusion atom.
fn quinazoline_n1(t: &Topology) -> u32 {
    let in_rings = |a: u32| t.rings().iter().filter(|r| r.contains(&a)).count();
    let candidates: Vec<u32> = t
        .aromatic_rings()
        .flatten()
        .copied()
        .filter(|&a| t.elements()[a as usize] == mold_mol::Element::N)
        .filter(|&a| t.neighbours(a).iter().any(|&(x, _)| in_rings(x) == 2))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(candidates.len(), 1, "exactly one N1: {candidates:?}");
    candidates[0]
}

#[test]
fn hinge_hbond_quinazoline_n1_to_met793_backbone_nh() {
    for (pdb, lig, met) in [
        ("4WKQ", "IRE", 793),
        ("1M17", "AQ4", 769),
        ("4G5J", "0WN", 793),
    ] {
        let c = Complex::load(pdb, lig, "crystal");
        let n1 = quinazoline_n1(&c.topology);
        let found = c.contacts();
        let hinge: Vec<_> = found
            .hbonds
            .iter()
            .filter(|h| h.ligand_atom == n1 && h.donor == Side::Receptor)
            .collect();
        assert_eq!(hinge.len(), 1, "{pdb}: {:?}", found.hbonds);
        let h = hinge[0];
        assert_eq!(
            c.receptor_atom(h.receptor_atom),
            format!("A:MET:{met} N"),
            "{pdb}"
        );
        // Hinge N···N distances in these structures are 2.7–3.4 Å, the NH
        // pointing almost straight at N1.
        assert!((2.6..3.5).contains(&h.distance), "{pdb}: {}", h.distance);
        assert!(h.angle > 150.0, "{pdb}: {}", h.angle);
        assert_eq!(h.hydrogen, mold_geom::contacts::Hydrogen::Placed);
    }
}

#[test]
fn quinazolines_neither_stack_nor_bind_thr790_directly_in_the_crystal() {
    for (pdb, lig) in [("4WKQ", "IRE"), ("1M17", "AQ4"), ("4G5J", "0WN")] {
        let c = Complex::load(pdb, lig, "crystal");
        let found = c.contacts();
        assert!(found.pi_stacks.is_empty(), "{pdb}: {:?}", found.pi_stacks);
        let n1 = quinazoline_n1(&c.topology);
        // N3: the other aromatic N of the quinazoline's pyrimidine ring.
        let pyrimidine = c
            .topology
            .aromatic_rings()
            .find(|r| r.contains(&n1))
            .unwrap()
            .to_vec();
        let n3 = pyrimidine
            .iter()
            .copied()
            .find(|&a| a != n1 && c.topology.elements()[a as usize] == mold_mol::Element::N)
            .unwrap();
        for h in found.hbonds.iter().filter(|h| h.ligand_atom == n3) {
            let partner = c.receptor_atom(h.receptor_atom);
            assert!(
                !partner.contains(":THR:"),
                "{pdb}: direct N3–Thr H-bond {partner}"
            );
        }
    }
}

#[test]
fn basic_amines_form_a_salt_bridge_with_asp800() {
    for (pdb, lig) in [("4WKQ", "IRE"), ("4G5J", "0WN")] {
        let c = Complex::load(pdb, lig, "crystal");
        let found = c.contacts();
        let asp800: Vec<_> = found
            .salt_bridges
            .iter()
            .filter(|s| c.receptor.residues()[s.residue as usize].to_string() == "A:ASP:800")
            .collect();
        assert_eq!(asp800.len(), 1, "{pdb}: {:?}", found.salt_bridges);
        assert_eq!(
            asp800[0].ligand_charge,
            mold_geom::contacts::Charge::Positive
        );
        assert!(asp800[0].distance < 5.0, "{pdb}: {}", asp800[0].distance);
    }
}

#[test]
fn counts_map_to_the_pose_table_columns() {
    for (pdb, lig) in [("4WKQ", "IRE"), ("1M17", "AQ4"), ("4G5J", "0WN")] {
        for pose in ["crystal", "clash", "far"] {
            let found = Complex::load(pdb, lig, pose).contacts();
            assert_eq!(found.n_hbonds(), found.hbonds.len());
            assert_eq!(found.n_saltbridge(), found.salt_bridges.len());
            if pose == "far" {
                assert!(found.is_empty(), "{pdb} far: {found:?}");
            }
        }
    }
}

// ---- PLIP cross-check -------------------------------------------------------

/// One interaction, keyed the way `fixtures/plip/reference.csv` writes it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    interaction: &'static str,
    role: String,
    ligand_atoms: String,
    receptor: String,
}

fn join(atoms: impl IntoIterator<Item = u32>) -> String {
    let mut v: Vec<u32> = atoms.into_iter().collect();
    v.sort_unstable();
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")
}

fn rows(c: &Complex, found: &Contacts) -> Vec<(Row, bool, String)> {
    let receptor = |atoms: &[u32], residue: u32| {
        let mut names: Vec<String> = atoms
            .iter()
            .map(|&a| c.receptor.atom_name(a).unwrap().to_string())
            .collect();
        names.sort();
        format!(
            "{} {}",
            c.receptor.residues()[residue as usize],
            names.join(" ")
        )
    };
    let mut out = Vec::new();
    for h in &found.hbonds {
        let role = if h.donor == Side::Ligand {
            "donor"
        } else {
            "acceptor"
        };
        // PLIP places rotatable hydrogens (OH, NH3+, NH2) at one arbitrary
        // dihedral; geom takes the best one (see CLAUDE.md), so an H-bond from
        // such a donor may be ours alone.
        let rotatable = h.rotatable_hydrogen;
        out.push((
            Row {
                interaction: "hbond",
                role: role.into(),
                ligand_atoms: h.ligand_atom.to_string(),
                receptor: receptor(&[h.receptor_atom], h.residue),
            },
            rotatable,
            format!("{:.2} A, {:.1} deg", h.distance, h.angle),
        ));
    }
    for s in &found.salt_bridges {
        let role = match s.ligand_charge {
            mold_geom::contacts::Charge::Positive => "positive",
            mold_geom::contacts::Charge::Negative => "negative",
        };
        out.push((
            Row {
                interaction: "saltbridge",
                role: role.into(),
                ligand_atoms: join(s.ligand_atoms.iter().copied()),
                receptor: receptor(&s.receptor_atoms, s.residue),
            },
            false,
            String::new(),
        ));
    }
    for p in &found.pi_stacks {
        let PiStack { kind, .. } = p;
        out.push((
            Row {
                interaction: "pistack",
                role: match kind {
                    mold_geom::contacts::StackKind::Parallel => "P".into(),
                    mold_geom::contacts::StackKind::TShaped => "T".into(),
                },
                ligand_atoms: join(p.ligand_atoms.iter().copied()),
                receptor: receptor(&p.receptor_atoms, p.residue),
            },
            false,
            String::new(),
        ));
    }
    for p in &found.cation_pi {
        let CationPi { cation, .. } = p;
        out.push((
            Row {
                interaction: "pication",
                role: if *cation == Side::Ligand {
                    "cation"
                } else {
                    "ring"
                }
                .into(),
                ligand_atoms: join(p.ligand_atoms.iter().copied()),
                receptor: receptor(&p.receptor_atoms, p.residue),
            },
            false,
            String::new(),
        ));
    }
    out.sort();
    out
}

/// geom against PLIP 3.0.1 (`fixtures/plip/reference.csv`, made by
/// `fixtures/scripts/make_plip_reference.py`) on the nine rigid poses:
/// - salt bridges, π-stacking and cation–π, every pose: the same set, exactly;
/// - H-bonds, `crystal` and `far`: the same set, with the same roles and
///   partner atoms. An H-bond PLIP lacks is allowed only from a donor whose
///   hydrogen position is not fixed by its heavy atoms (geom takes the best
///   rotamer, PLIP one arbitrary one); on this fixture there are none.
/// - H-bonds, `clash` (ligand pushed 4 Å into the protein): reported, not
///   asserted. PLIP protonates Asp side chains (Open Babel's neutral acid), so
///   it has Asp776/Asp831 donating to 1M17's clash pose where geom, with
///   charged Asp/Glu, does not; 1M17's Cys773 N···N10 sits at 102° against a
///   100° cut-off, where the two hydrogen placements disagree; and PLIP reports
///   nothing at all for 4G5J's clash pose (its atoms overlap the protein's).
#[test]
fn cross_checks_against_plip() {
    let text = std::fs::read_to_string(common::fixtures_dir().join("plip/reference.csv")).unwrap();
    let reference = common::read_csv(&text);
    let mut report = String::new();
    let mut misses = Vec::new();
    for (pdb, lig) in [("1M17", "AQ4"), ("4WKQ", "IRE"), ("4G5J", "0WN")] {
        for pose in ["crystal", "clash", "far"] {
            let c = Complex::load(pdb, lig, pose);
            let found = c.contacts();
            let ours = rows(&c, &found);
            let theirs: Vec<Row> = reference
                .iter()
                .filter(|r| r["complex"] == pdb && r["pose"] == pose)
                .map(|r| Row {
                    interaction: match r["interaction"].as_str() {
                        "hbond" => "hbond",
                        "saltbridge" => "saltbridge",
                        "pistack" => "pistack",
                        "pication" => "pication",
                        other => panic!("unknown interaction {other}"),
                    },
                    role: r["ligand_role"].clone(),
                    ligand_atoms: r["ligand_atoms"].clone(),
                    receptor: format!("{} {}", r["receptor_residue"], r["receptor_atoms"]),
                })
                .collect();
            report += &format!("{pdb} {pose}\n");
            let asserted = |r: &Row| pose != "clash" || r.interaction != "hbond";
            for t in &theirs {
                let hit = ours.iter().any(|(o, _, _)| o == t);
                report += &format!("  plip {t:?} {}\n", if hit { "ok" } else { "MISSING" });
                if !hit && asserted(t) {
                    misses.push(format!("{pdb} {pose}: PLIP has {t:?}, geom does not"));
                }
            }
            for (o, rotatable, detail) in &ours {
                if theirs.contains(o) {
                    continue;
                }
                let allowed = o.interaction == "hbond" && *rotatable;
                report += &format!(
                    "  geom {o:?} {detail} {}\n",
                    if allowed {
                        "extra (rotatable H)"
                    } else {
                        "EXTRA"
                    }
                );
                if !allowed && asserted(o) {
                    misses.push(format!("{pdb} {pose}: geom has {o:?}, PLIP does not"));
                }
            }
        }
    }
    println!("{report}");
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}

// ---- Properties -------------------------------------------------------------

/// A contact list reduced to what must not change under a rigid move: the
/// interacting atoms (exactly) and the numbers (within float noise). The
/// lists come out of `contacts` in a canonical order, so numbers line up.
fn fingerprint(c: &Complex, found: &Contacts) -> (Vec<Row>, Vec<f32>) {
    let keys = rows(c, found).into_iter().map(|(r, _, _)| r).collect();
    let mut numbers = Vec::new();
    numbers.extend(
        found
            .hbonds
            .iter()
            .flat_map(|h| [h.distance, h.angle / 100.0]),
    );
    numbers.extend(found.salt_bridges.iter().map(|s| s.distance));
    numbers.extend(
        found
            .pi_stacks
            .iter()
            .flat_map(|p| [p.distance, p.angle / 100.0, p.offset]),
    );
    numbers.extend(found.cation_pi.iter().flat_map(|p| [p.distance, p.offset]));
    (keys, numbers)
}

fn rotate(q: [f64; 4], t: [f64; 3], p: [f32; 3]) -> [f32; 3] {
    let n = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    let [w, x, y, z] = q.map(|c| c / n);
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
    let v = p.map(f64::from);
    std::array::from_fn(|i| (m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + t[i]) as f32)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Moving receptor and ligand together changes nothing: same interactions,
    /// same distances and angles. The receptor is re-read (and re-typed) from
    /// the moved arena.
    #[test]
    fn contacts_are_invariant_under_rigid_transform_of_the_complex(
        q in prop::array::uniform4(-1.0f64..1.0).prop_filter("degenerate", |q| q.iter().map(|c| c * c).sum::<f64>() > 0.01),
        t in prop::array::uniform3(-40.0f64..40.0),
        which in 0usize..3,
    ) {
        let (pdb, lig) = [("4WKQ", "IRE"), ("1M17", "AQ4"), ("4G5J", "0WN")][which];
        let c = Complex::load(pdb, lig, "crystal");
        let before = fingerprint(&c, &c.contacts());
        let mut moved = Complex::load(pdb, lig, "crystal");
        for p in moved.arena.positions_mut() {
            *p = rotate(q, t, *p);
        }
        moved.receptor = Receptor::from_arena(&moved.arena, moved.receptor_pose).unwrap();
        let after = fingerprint(&moved, &moved.contacts());
        prop_assert_eq!(&before.0, &after.0);
        prop_assert_eq!(before.1.len(), after.1.len());
        for (x, y) in before.1.iter().zip(&after.1) {
            // f32 coordinates near 200 Å carry ~1e-5 Å of rounding; angles are
            // scaled by 1/100 above, so 1e-3 here is 0.1°.
            prop_assert!((x - y).abs() < 1e-3, "{x} vs {y}");
        }
    }
}

// ---- Hydrogens --------------------------------------------------------------

fn atom(name: &str, element: mold_mol::Element, position: [f32; 3]) -> mold_mol::AtomRecord {
    mold_mol::AtomRecord {
        name: mold_mol::AtomName::new(name).unwrap(),
        element,
        position,
        b_factor: 0.0,
        occupancy: 1.0,
        formal_charge: 0,
    }
}

/// A serine (CB–OG, optionally with HG) and a ketone-like ligand O 2.83 Å
/// from OG, at a CB–OG···O angle of 135°.
fn serine_and_carbonyl(hg: Option<[f32; 3]>) -> Contacts {
    use mold_mol::{BondOrder, Element};
    let mut arena = Arena::new();
    let mut b = arena.begin_pose("receptor");
    b.add_chain("A");
    b.add_residue("SER", 1, None, false);
    b.add_atom(atom("CA", Element::C, [-2.0, 1.2, 0.0]));
    b.add_atom(atom("CB", Element::C, [-1.43, 0.0, 0.0]));
    b.add_atom(atom("OG", Element::O, [0.0, 0.0, 0.0]));
    if let Some(h) = hg {
        b.add_atom(atom("HG", Element::H, h));
    }
    let receptor_pose = b.finish();
    let mut b = arena.begin_pose("ligand");
    b.add_chain("L");
    b.add_residue("LIG", 1, None, true);
    let o = b.add_atom(atom("O1", Element::O, [2.0, 2.0, 0.0]));
    let c = b.add_atom(atom("C1", Element::C, [2.85, 2.85, 0.0]));
    b.add_bond(o, c, BondOrder::Double);
    let ligand_pose = b.finish();
    let receptor = Receptor::from_arena(&arena, receptor_pose).unwrap();
    let topology = Topology::from_arena(&arena, ligand_pose).unwrap();
    contacts(
        &receptor,
        &topology,
        arena.pose_positions(ligand_pose).unwrap(),
        &ContactConfig::default(),
    )
}

#[test]
fn explicit_receptor_hydrogens_are_used_and_missing_ones_are_placed() {
    use mold_geom::contacts::Hydrogen;
    // No HG: the hydroxyl H is placed at its best rotamer: on the cone 70.5°
    // off the CB→OG axis, in the plane of O1, i.e. 25.5° off OG→O1, which
    // makes OG–H···O1 about 142°.
    let placed = serine_and_carbonyl(None);
    assert_eq!(placed.hbonds.len(), 1);
    let h = &placed.hbonds[0];
    assert_eq!(
        (h.donor, h.hydrogen, h.rotatable_hydrogen),
        (Side::Receptor, Hydrogen::Placed, true)
    );
    assert!((h.angle - 141.9).abs() < 0.5, "{}", h.angle);

    // HG given and pointing at O1: used as is.
    let toward = serine_and_carbonyl(Some([0.679, 0.679, 0.0]));
    assert_eq!(toward.hbonds.len(), 1);
    assert_eq!(toward.hbonds[0].hydrogen, Hydrogen::Explicit);
    assert!((toward.hbonds[0].angle - 180.0).abs() < 0.1);

    // HG given and pointing away: the geometry rules the H-bond out.
    let away = serine_and_carbonyl(Some([0.3, -0.91, 0.0]));
    assert!(away.hbonds.is_empty(), "{:?}", away.hbonds);
}
