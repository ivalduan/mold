//! Protein–ligand contact profiling: hydrogen bonds, salt bridges, π-stacking
//! and cation–π, with PLIP's geometric definitions (Salentin et al., NAR 43,
//! W443, 2015; Adasme et al., NAR 49, W530, 2021). The thresholds are PLIP
//! 3.0.1's `plip/basic/config.py`; see [`ContactConfig`] and the crate's
//! `CLAUDE.md` for the table and for where geom deliberately differs.
//!
//! Typing:
//! - **Ligand** from elements, bonds, formal charges and [`Topology`]
//!   (hybridisation, aromatic rings), computed once per topology.
//! - **Receptor** from residue and atom names ([`Receptor::from_arena`]),
//!   because PDB/mmCIF receptors carry no bonds. Computed once per receptor.
//!   A receptor built with [`Receptor::new`] has no names and so no contacts.
//!
//! Hydrogens: an explicit hydrogen on a donor is used as is. Without one (the
//! usual crystal receptor, a heavy-atom SDF), the hydrogen is placed from the
//! donor's heavy-atom neighbours and hybridisation, or, where its position is
//! not fixed by them (OH, NH3+, NH2: one heavy neighbour), on its cone at the
//! best rotamer for the acceptor. The H-bond test is then PLIP's either way:
//! donor–acceptor distance and D–H···A angle.
//!
//! The per-pose cost is one KD-tree radius query on the receptor's existing
//! tree per ligand polar atom, plus loops over the few charged groups and rings.

use mold_mol::{AtomName, Element};

use crate::linalg::{V3, add, angle_deg, cross, dot, mean, norm, plane_fit, scale, sub, unit, v3};
use crate::radii::covalent_radius;
use crate::receptor::Receptor;
use crate::topology::{BondOrder, Hybridization, Topology};

/// Thresholds of [`contacts`]. Defaults are PLIP 3.0.1's (`plip/basic/config.py`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactConfig {
    /// Every interaction distance must exceed this, Å. PLIP `MIN_DIST` 0.5.
    pub min_distance: f32,
    /// H-bond: donor–acceptor heavy-atom distance below this, Å. PLIP
    /// `HBOND_DIST_MAX` 4.1.
    pub hbond_max_distance: f32,
    /// H-bond: D–H···A angle above this, degrees. PLIP `HBOND_DON_ANGLE_MIN` 100.
    pub hbond_min_angle: f32,
    /// Salt bridge: distance between charge centres below this, Å. PLIP
    /// `SALTBRIDGE_DIST_MAX` 5.5.
    pub saltbridge_max_distance: f32,
    /// π-stacking: ring-centroid distance below this, Å. PLIP `PISTACK_DIST_MAX` 5.5.
    pub pistack_max_distance: f32,
    /// π-stacking: ring planes within this many degrees of parallel (or of
    /// perpendicular, for T-shaped). PLIP `PISTACK_ANG_DEV` 30.
    pub pistack_max_angle_deviation: f32,
    /// π-stacking and cation–π: offset of a centre projected into the ring
    /// plane from the ring centroid below this, Å. PLIP `PISTACK_OFFSET_MAX` 2.0.
    pub pistack_max_offset: f32,
    /// Cation–π: charge centre to ring centroid below this, Å. PLIP
    /// `PICATION_DIST_MAX` 6.0.
    pub cation_pi_max_distance: f32,
    /// Cation–π with a ligand tertiary amine: the amine's plane within this
    /// many degrees of the ring plane, so its N–H points at the ring. PLIP's
    /// hard-coded 30.
    pub cation_pi_amine_max_angle: f32,
}

impl Default for ContactConfig {
    fn default() -> Self {
        ContactConfig {
            min_distance: 0.5,
            hbond_max_distance: 4.1,
            hbond_min_angle: 100.0,
            saltbridge_max_distance: 5.5,
            pistack_max_distance: 5.5,
            pistack_max_angle_deviation: 30.0,
            pistack_max_offset: 2.0,
            cation_pi_max_distance: 6.0,
            cation_pi_amine_max_angle: 30.0,
        }
    }
}

/// Which partner plays a role (donor, cation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Ligand,
    Receptor,
}

/// Where the hydrogen of an H-bond came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hydrogen {
    /// An explicit hydrogen atom of the input.
    Explicit,
    /// Placed from the donor's heavy-atom geometry (see the module docs).
    Placed,
}

/// Sign of a charged group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Charge {
    Positive,
    Negative,
}

/// Geometry of a π-stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StackKind {
    /// Face to face: planes within `pistack_max_angle_deviation` of parallel.
    Parallel,
    /// Edge to face: planes within `pistack_max_angle_deviation` of perpendicular.
    TShaped,
}

/// One hydrogen bond.
#[derive(Debug, Clone, PartialEq)]
pub struct HBond {
    /// Which partner donates.
    pub donor: Side,
    /// The ligand's donor or acceptor atom.
    pub ligand_atom: u32,
    /// The receptor's donor or acceptor atom (receptor source index).
    pub receptor_atom: u32,
    /// Index into [`Receptor::residues`].
    pub residue: u32,
    /// Donor–acceptor heavy-atom distance, Å.
    pub distance: f32,
    /// D–H···A angle, degrees.
    pub angle: f32,
    pub hydrogen: Hydrogen,
    /// The donor's hydrogen position is not fixed by its heavy atoms (one
    /// heavy neighbour: OH, NH3+, NH2) and no explicit one was given, so the
    /// angle is for the best rotamer.
    pub rotatable_hydrogen: bool,
}

/// One salt bridge: a charged ligand group and an oppositely charged residue.
#[derive(Debug, Clone, PartialEq)]
pub struct SaltBridge {
    pub ligand_charge: Charge,
    /// The ligand group's atoms (an amine N, a carboxylate's two O, …).
    pub ligand_atoms: Vec<u32>,
    /// The residue's charged atoms (receptor source indices).
    pub receptor_atoms: Vec<u32>,
    pub residue: u32,
    /// Distance between the charge centres, Å.
    pub distance: f32,
}

/// One π-stack between a ligand aromatic ring and a receptor aromatic ring.
#[derive(Debug, Clone, PartialEq)]
pub struct PiStack {
    pub kind: StackKind,
    pub ligand_atoms: Vec<u32>,
    pub receptor_atoms: Vec<u32>,
    pub residue: u32,
    /// Ring-centroid distance, Å.
    pub distance: f32,
    /// Angle between the ring planes, degrees in `[0, 90]`.
    pub angle: f32,
    /// The smaller of the two centroid offsets (each centroid projected into
    /// the other ring's plane), Å.
    pub offset: f32,
}

/// One cation–π interaction.
#[derive(Debug, Clone, PartialEq)]
pub struct CationPi {
    /// Which partner carries the cation (the other carries the ring).
    pub cation: Side,
    /// The ligand's ring or charged group.
    pub ligand_atoms: Vec<u32>,
    /// The receptor's ring or charged atoms.
    pub receptor_atoms: Vec<u32>,
    pub residue: u32,
    /// Charge centre to ring centroid, Å.
    pub distance: f32,
    /// Charge centre projected into the ring plane, to the ring centroid, Å.
    pub offset: f32,
}

/// Every interaction of one pose, in a canonical order (by ligand atoms, then
/// receptor atoms).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contacts {
    pub hbonds: Vec<HBond>,
    pub salt_bridges: Vec<SaltBridge>,
    pub pi_stacks: Vec<PiStack>,
    pub cation_pi: Vec<CationPi>,
}

impl Contacts {
    /// The pose table's `n_hbonds` (`mold_mol::pose_columns::N_HBONDS`).
    pub fn n_hbonds(&self) -> usize {
        self.hbonds.len()
    }

    /// The pose table's `n_saltbridge` (`mold_mol::pose_columns::N_SALTBRIDGE`).
    pub fn n_saltbridge(&self) -> usize {
        self.salt_bridges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hbonds.is_empty()
            && self.salt_bridges.is_empty()
            && self.pi_stacks.is_empty()
            && self.cation_pi.is_empty()
    }
}

// ---- Typing -----------------------------------------------------------------

/// A charged group: its atoms, and the atoms whose centroid is its charge centre.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LigandGroup {
    atoms: Vec<u32>,
    centre: Vec<u32>,
    /// For a tertiary amine: its three heavy neighbours (PLIP's extra
    /// cation–π angle check).
    amine_neighbours: Option<[u32; 3]>,
}

/// Ligand contact typing, derived from a [`Topology`] once.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LigandSites {
    donor: Vec<bool>,
    acceptor: Vec<bool>,
    /// Atoms that are donors or acceptors.
    polar: Vec<u32>,
    positive: Vec<LigandGroup>,
    negative: Vec<LigandGroup>,
    rings: Vec<Vec<u32>>,
}

fn order_x2(o: BondOrder) -> i32 {
    match o {
        BondOrder::Single => 2,
        BondOrder::Double => 4,
        BondOrder::Triple => 6,
        BondOrder::Aromatic => 3,
    }
}

/// Implicit hydrogens from the default valence, adjusted by the formal
/// charge (N+ 4, O− 1, C± 3), minus the bond orders (aromatic counts 1.5).
fn implicit_hydrogens(e: Element, charge: i8, bond_order_x2: i32) -> u32 {
    let charge = i32::from(charge);
    let valence = match e.atomic_number() {
        5 => 3 - charge.abs(),
        6 => 4 - charge.abs(),
        7 | 15 => 3 + charge,
        8 | 16 => 2 + charge,
        9 | 17 | 35 | 53 => 1 + charge,
        _ => return 0,
    };
    ((2 * valence - bond_order_x2).max(0) / 2) as u32
}

pub(crate) fn type_ligand(t: &Topology) -> LigandSites {
    let n = t.atom_count();
    let el = t.elements();
    let charge = t.formal_charges();
    let z = |a: u32| el[a as usize].atomic_number();
    let heavy_nb = |a: u32| -> Vec<u32> {
        t.neighbours(a)
            .iter()
            .map(|&(x, _)| x)
            .filter(|&x| z(x) != 1)
            .collect()
    };
    let aromatic_atom: Vec<bool> = {
        let mut v = vec![false; n];
        for r in t.aromatic_rings() {
            for &a in r {
                v[a as usize] = true;
            }
        }
        v
    };
    let mut hydrogens = vec![0u32; n];
    for (a, h) in hydrogens.iter_mut().enumerate() {
        let a = a as u32;
        let explicit = t.neighbours(a).iter().filter(|&&(x, _)| z(x) == 1).count() as u32;
        let orders: i32 = t.neighbours(a).iter().map(|&(_, o)| order_x2(o)).sum();
        *h = explicit
            + if charge.is_empty() {
                0
            } else {
                implicit_hydrogens(el[a as usize], charge[a as usize], orders)
            };
    }

    let mut s = LigandSites {
        donor: vec![false; n],
        acceptor: vec![false; n],
        ..LigandSites::default()
    };
    for a in 0..n as u32 {
        let i = a as usize;
        let q = charge.get(i).copied().unwrap_or(0);
        // Heavy neighbours plus every hydrogen (explicit and implicit), once each.
        let degree = heavy_nb(a).len() as u32 + hydrogens[i];
        match z(a) {
            7 => {
                s.donor[i] = hydrogens[i] > 0;
                // Open Babel's rule, as PLIP uses it: not an ammonium (sp3,
                // four neighbours) and not an amide/aniline/pyrrole N (sp2,
                // three neighbours), and not positively charged.
                let hyb = t.hybridization(a);
                s.acceptor[i] = q <= 0
                    && !(hyb == Hybridization::Sp3 && degree >= 4)
                    && !(hyb == Hybridization::Sp2 && degree >= 3);
            }
            8 => {
                s.donor[i] = hydrogens[i] > 0;
                s.acceptor[i] = q <= 0;
            }
            _ => {}
        }
        if s.donor[i] || s.acceptor[i] {
            s.polar.push(a);
        }
    }

    // Charged groups (PLIP's functional groups, see CLAUDE.md).
    let mut grouped = vec![false; n];
    for a in 0..n as u32 {
        let i = a as usize;
        let q = charge.get(i).copied().unwrap_or(0);
        let hn = heavy_nb(a);
        match z(a) {
            7 if q >= 0 => {
                let quaternary = hn.len() == 4;
                let amine = t.hybridization(a) == Hybridization::Sp3
                    && !aromatic_atom[i]
                    && hn.len() + hydrogens[i] as usize >= 3;
                if quaternary || amine {
                    s.positive.push(LigandGroup {
                        atoms: vec![a],
                        centre: vec![a],
                        amine_neighbours: (hn.len() == 3).then(|| [hn[0], hn[1], hn[2]]),
                    });
                    grouped[i] = true;
                }
            }
            6 => {
                let o: Vec<u32> = hn.iter().copied().filter(|&x| z(x) == 8).collect();
                let c = hn.iter().filter(|&&x| z(x) == 6).count();
                let nn: Vec<u32> = hn.iter().copied().filter(|&x| z(x) == 7).collect();
                if o.len() == 2
                    && c == 1
                    && hn.len() == 3
                    && o.iter().all(|&x| heavy_nb(x).len() == 1)
                {
                    // Carboxylic acid / carboxylate (esters excluded).
                    s.negative.push(LigandGroup {
                        atoms: o.clone(),
                        centre: o.clone(),
                        amine_neighbours: None,
                    });
                    for x in o {
                        grouped[x as usize] = true;
                    }
                } else if nn.len() == 3
                    && hn.len() == 3
                    && nn.iter().any(|&x| heavy_nb(x).len() == 1)
                {
                    // Guanidine: charge centre on the carbon, PLIP's choice.
                    s.positive.push(LigandGroup {
                        atoms: nn.clone(),
                        centre: vec![a],
                        amine_neighbours: None,
                    });
                    for x in nn {
                        grouped[x as usize] = true;
                    }
                }
            }
            16 | 15 => {
                let o: Vec<u32> = hn.iter().copied().filter(|&x| z(x) == 8).collect();
                let acid = if z(a) == 16 {
                    o.len() == 3 || o.len() == 4 // sulfonate, sulfate
                } else {
                    !hn.is_empty() && o.len() == hn.len() // phosphate
                };
                if acid {
                    let mut atoms = vec![a];
                    atoms.extend(&o);
                    s.negative.push(LigandGroup {
                        atoms,
                        centre: vec![a],
                        amine_neighbours: None,
                    });
                    grouped[i] = true;
                    for x in o {
                        grouped[x as usize] = true;
                    }
                }
            }
            _ => {}
        }
    }
    // Explicit formal charges not covered above, unless compensated by a
    // neighbour (nitro N+/O−, N-oxides).
    for a in 0..n as u32 {
        let i = a as usize;
        let q = charge.get(i).copied().unwrap_or(0);
        if q == 0 || grouped[i] {
            continue;
        }
        let compensated = t
            .neighbours(a)
            .iter()
            .any(|&(x, _)| charge[x as usize].signum() == -q.signum());
        if compensated {
            continue;
        }
        let g = LigandGroup {
            atoms: vec![a],
            centre: vec![a],
            amine_neighbours: None,
        };
        if q > 0 {
            s.positive.push(g);
        } else {
            s.negative.push(g);
        }
    }
    s.rings = t.aromatic_rings().map(<[u32]>::to_vec).collect();
    s
}

/// A donor's heavy-atom environment, for placing its hydrogen.
#[derive(Debug, Clone, PartialEq)]
struct ReceptorDonor {
    heavy: u32,
    hybridization: Hybridization,
    neighbours: Vec<V3>,
    hydrogens: Vec<V3>,
}

#[derive(Debug, Clone, PartialEq)]
struct ReceptorGroup {
    heavy: Vec<u32>,
    centre: V3,
    residue: u32,
    histidine: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct ReceptorRing {
    heavy: Vec<u32>,
    centre: V3,
    normal: V3,
    residue: u32,
}

/// Receptor contact typing, built once per receptor.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ReceptorSites {
    /// Per heavy atom: index into `donors`, or `u32::MAX`.
    donor_of: Vec<u32>,
    acceptor: Vec<bool>,
    donors: Vec<ReceptorDonor>,
    positive: Vec<ReceptorGroup>,
    negative: Vec<ReceptorGroup>,
    rings: Vec<ReceptorRing>,
}

const WATERS: [&str; 3] = ["HOH", "WAT", "DOD"];
const STANDARD: [&str; 20] = [
    "ALA", "ARG", "ASN", "ASP", "CYS", "GLN", "GLU", "GLY", "HIS", "ILE", "LEU", "LYS", "MET",
    "PHE", "PRO", "SER", "THR", "TRP", "TYR", "VAL",
];

/// Side-chain donors of the standard residues, with the donor's hybridisation.
fn side_chain_donor(residue: &str, atom: &str) -> Option<Hybridization> {
    use Hybridization::{Sp2, Sp3};
    Some(match (residue, atom) {
        ("ARG", "NE" | "NH1" | "NH2") => Sp2,
        ("ASN", "ND2") | ("GLN", "NE2") => Sp2,
        // Histidine's tautomer is unknown without hydrogens: both N may donate.
        ("HIS", "ND1" | "NE2") => Sp2,
        ("TRP", "NE1") => Sp2,
        ("LYS", "NZ") => Sp3,
        ("SER", "OG") | ("THR", "OG1") | ("TYR", "OH") => Sp3,
        _ => return None,
    })
}

/// Side-chain acceptors of the standard residues.
fn side_chain_acceptor(residue: &str, atom: &str) -> bool {
    matches!(
        (residue, atom),
        ("ASP", "OD1" | "OD2")
            | ("GLU", "OE1" | "OE2")
            | ("ASN", "OD1")
            | ("GLN", "OE1")
            | ("SER", "OG")
            | ("THR", "OG1")
            | ("TYR", "OH")
            | ("HIS", "ND1" | "NE2")
    )
}

/// PLIP's charged side chains: Arg, His, Lys positive; Asp, Glu negative.
fn charged_atoms(residue: &str) -> Option<(Charge, &'static [&'static str])> {
    Some(match residue {
        "ARG" => (Charge::Positive, &["NE", "NH1", "NH2"]),
        "HIS" => (Charge::Positive, &["ND1", "NE2"]),
        "LYS" => (Charge::Positive, &["NZ"]),
        "ASP" => (Charge::Negative, &["OD1", "OD2"]),
        "GLU" => (Charge::Negative, &["OE1", "OE2"]),
        _ => return None,
    })
}

fn aromatic_rings_of(residue: &str) -> &'static [&'static [&'static str]] {
    match residue {
        "PHE" | "TYR" => &[&["CG", "CD1", "CD2", "CE1", "CE2", "CZ"]],
        "HIS" => &[&["CG", "ND1", "CD2", "CE1", "NE2"]],
        "TRP" => &[
            &["CG", "CD1", "NE1", "CE2", "CD2"],
            &["CD2", "CE2", "CE3", "CZ2", "CZ3", "CH2"],
        ],
        _ => &[],
    }
}

/// Bonded heavy neighbours of receptor heavy atom `k`, found by distance
/// (within the covalent radius sum + 0.45 Å, the usual connect-the-dots rule).
fn receptor_neighbours(r: &Receptor, k: u32) -> Vec<V3> {
    let p = r.heavy_position(k);
    let rk = covalent_radius(r.heavy_element(k), BondOrder::Single);
    let mut out = Vec::new();
    r.index().for_each_within(p, 2.2, |n| {
        if n.index == k {
            return;
        }
        let rn = covalent_radius(r.heavy_element(n.index), BondOrder::Single);
        if n.distance <= rk + rn + 0.45 {
            out.push(v3(r.heavy_position(n.index)));
        }
    });
    out
}

pub(crate) fn type_receptor(r: &Receptor, hydrogens: &[[f32; 3]]) -> ReceptorSites {
    let n = r.atom_count();
    let mut s = ReceptorSites {
        donor_of: vec![u32::MAX; n],
        acceptor: vec![false; n],
        ..ReceptorSites::default()
    };
    if r.residues().is_empty() {
        return s;
    }
    // Heavy atoms are contiguous per residue (arena order).
    let mut k = 0u32;
    while (k as usize) < n {
        let res = r.heavy_residue(k);
        let mut end = k;
        while (end as usize) < n && r.heavy_residue(end) == res {
            end += 1;
        }
        let atoms: Vec<(u32, AtomName)> = (k..end)
            .filter_map(|a| r.heavy_name(a).map(|nm| (a, nm)))
            .collect();
        type_residue(r, &mut s, res, &atoms);
        k = end;
    }
    // Explicit hydrogens: attach each to its nearest heavy atom if that is a donor.
    for &h in hydrogens {
        if let Some(nb) = r.index().nearest(h)
            && nb.distance <= 1.3
        {
            let d = s.donor_of[nb.index as usize];
            if d != u32::MAX {
                s.donors[d as usize].hydrogens.push(v3(h));
            }
        }
    }
    s
}

fn type_residue(r: &Receptor, s: &mut ReceptorSites, res: u32, atoms: &[(u32, AtomName)]) {
    let name = r.residues()[res as usize].name.as_str();
    if WATERS.contains(&name) {
        return;
    }
    let standard = STANDARD.contains(&name);
    let find = |want: &str| atoms.iter().find(|(_, nm)| *nm == want).map(|&(k, _)| k);
    for &(k, atom_name) in atoms {
        let atom = atom_name.as_str();
        let mut donor = None;
        let mut acceptor = false;
        match atom {
            "N" if name != "PRO" => donor = Some(Hybridization::Sp2),
            "O" | "OXT" => acceptor = true,
            _ if standard => {
                donor = side_chain_donor(name, atom);
                acceptor = side_chain_acceptor(name, atom);
            }
            // A modified residue's side chain: every O accepts (as Open Babel
            // types it for PLIP); no donors without a template.
            _ => acceptor = r.heavy_element(k) == Element::O,
        }
        s.acceptor[k as usize] = acceptor;
        if let Some(mut hyb) = donor {
            let neighbours = receptor_neighbours(r, k);
            if atom == "N" && neighbours.len() == 1 {
                hyb = Hybridization::Sp3; // a free N-terminus, NH3+
            }
            s.donor_of[k as usize] = s.donors.len() as u32;
            s.donors.push(ReceptorDonor {
                heavy: k,
                hybridization: hyb,
                neighbours,
                hydrogens: Vec::new(),
            });
        }
    }
    if let Some((charge, names)) = charged_atoms(name) {
        let heavy: Vec<u32> = names.iter().filter_map(|nm| find(nm)).collect();
        if !heavy.is_empty() {
            let centre = mean(heavy.iter().map(|&k| r.heavy_position(k)));
            let g = ReceptorGroup {
                heavy,
                centre,
                residue: res,
                histidine: name == "HIS",
            };
            match charge {
                Charge::Positive => s.positive.push(g),
                Charge::Negative => s.negative.push(g),
            }
        }
    }
    for ring in aromatic_rings_of(name) {
        let heavy: Option<Vec<u32>> = ring.iter().map(|nm| find(nm)).collect();
        if let Some(heavy) = heavy {
            let pts: Vec<[f32; 3]> = heavy.iter().map(|&k| r.heavy_position(k)).collect();
            let (centre, normal) = plane_fit(&pts);
            s.rings.push(ReceptorRing {
                heavy,
                centre,
                normal,
                residue: res,
            });
        }
    }
}

// ---- Geometry ---------------------------------------------------------------

/// Covalent N–H / O–H length used for placed hydrogens, Å.
const H_BOND_LENGTH: f64 = 1.0;
/// Half the tetrahedral angle: the two H of an sp3 donor with two heavy
/// neighbours lie this far out of the X–D–X plane, around the bisector.
const HALF_TETRAHEDRAL: f64 = 54.735_610_317_245_35;

/// The best D–H···A angle over the donor's possible hydrogen positions, with
/// where the hydrogen came from and whether it was a rotamer choice. `None` if
/// the donor has no room for a hydrogen.
fn donor_angle(
    d: V3,
    neighbours: &[V3],
    hyb: Hybridization,
    hydrogens: &[V3],
    a: V3,
) -> Option<(f64, Hydrogen, bool)> {
    let angle_at = |h: V3| angle_deg(sub(d, h), sub(a, h));
    if !hydrogens.is_empty() {
        let best = hydrogens
            .iter()
            .map(|&h| angle_at(h))
            .fold(f64::MIN, f64::max);
        return Some((best, Hydrogen::Explicit, false));
    }
    let place = |dir: V3| add(d, scale(dir, H_BOND_LENGTH));
    let u: Vec<V3> = neighbours.iter().filter_map(|&x| unit(sub(x, d))).collect();
    let toward = unit(sub(a, d))?;
    match (u.len(), hyb) {
        // Nothing bonded: the hydrogen can point straight at the acceptor.
        (0, _) => Some((180.0, Hydrogen::Placed, true)),
        // One heavy neighbour: the H lies on a cone around the X→D axis at
        // 180° minus the ideal X–D–H angle; take the rotamer nearest the acceptor.
        (1, _) => {
            let axis = scale(u[0], -1.0);
            let ideal: f64 = match hyb {
                Hybridization::Sp => 180.0,
                Hybridization::Sp2 => 120.0,
                Hybridization::Sp3 => 109.47,
            };
            let theta = (180.0 - ideal).to_radians();
            let perp = sub(toward, scale(axis, dot(toward, axis)));
            let w = unit(perp).unwrap_or_else(|| any_perpendicular(axis));
            let dir = add(scale(axis, theta.cos()), scale(w, theta.sin()));
            Some((angle_at(place(dir)), Hydrogen::Placed, true))
        }
        // Two neighbours, planar: the H is on the external bisector.
        (2, Hybridization::Sp2) => {
            let b = unit(scale(add(u[0], u[1]), -1.0))?;
            Some((angle_at(place(b)), Hydrogen::Placed, false))
        }
        // Two neighbours, tetrahedral: either remaining tetrahedral position.
        (2, Hybridization::Sp3) => {
            let b = unit(scale(add(u[0], u[1]), -1.0))?;
            let nrm = unit(cross(u[0], u[1]))?;
            let (c, s) = (
                HALF_TETRAHEDRAL.to_radians().cos(),
                HALF_TETRAHEDRAL.to_radians().sin(),
            );
            let best = [1.0, -1.0]
                .into_iter()
                .map(|sign| angle_at(place(add(scale(b, c), scale(nrm, sign * s)))))
                .fold(f64::MIN, f64::max);
            Some((best, Hydrogen::Placed, false))
        }
        // Three neighbours, tetrahedral (a protonated tertiary amine).
        (3, Hybridization::Sp3) => {
            let b = unit(scale(add(add(u[0], u[1]), u[2]), -1.0))?;
            Some((angle_at(place(b)), Hydrogen::Placed, false))
        }
        _ => None,
    }
}

fn any_perpendicular(a: V3) -> V3 {
    let trial = if a[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    unit(cross(a, trial)).unwrap_or([0.0, 0.0, 1.0])
}

/// Angle between two plane normals folded into `[0, 90]` degrees.
fn plane_angle(n1: V3, n2: V3) -> f64 {
    let a = angle_deg(n1, n2);
    a.min(180.0 - a)
}

/// Distance from the ring centre `c` (normal `n`) to `q` projected into the ring plane.
fn offset(c: V3, n: V3, q: V3) -> f64 {
    let d = sub(q, c);
    norm(sub(d, scale(n, dot(d, n))))
}

fn dist_v(a: V3, b: V3) -> f64 {
    norm(sub(a, b))
}

// ---- Profiling --------------------------------------------------------------

/// Profile one ligand pose against the receptor. `positions` is indexed like
/// the topology's atoms. Receptor atoms in the result are receptor source
/// indices; residues index [`Receptor::residues`].
///
/// # Panics
/// If `positions.len()` differs from the topology's atom count.
pub fn contacts(
    receptor: &Receptor,
    topology: &Topology,
    positions: &[[f32; 3]],
    config: &ContactConfig,
) -> Contacts {
    assert_eq!(
        positions.len(),
        topology.atom_count(),
        "pose has a different atom count from the ligand topology"
    );
    let rs = receptor.sites();
    let ls = topology.sites();
    let mut out = Contacts::default();
    if receptor.residues().is_empty() {
        return out;
    }
    let min = f64::from(config.min_distance);
    let pos = |a: u32| v3(positions[a as usize]);
    let centre_of = |g: &LigandGroup| mean(g.centre.iter().map(|&a| positions[a as usize]));
    let src =
        |heavy: &[u32]| -> Vec<u32> { heavy.iter().map(|&k| receptor.source_index(k)).collect() };
    let ligand_rings: Vec<(V3, V3)> = ls
        .rings
        .iter()
        .map(|r| {
            let pts: Vec<[f32; 3]> = r.iter().map(|&a| positions[a as usize]).collect();
            plane_fit(&pts)
        })
        .collect();

    // Salt bridges.
    let max_sb = f64::from(config.saltbridge_max_distance);
    for (groups, partners, charge) in [
        (&ls.positive, &rs.negative, Charge::Positive),
        (&ls.negative, &rs.positive, Charge::Negative),
    ] {
        for g in groups {
            let c = centre_of(g);
            for p in partners {
                let d = dist_v(c, p.centre);
                if d > min && d < max_sb {
                    out.salt_bridges.push(SaltBridge {
                        ligand_charge: charge,
                        ligand_atoms: g.atoms.clone(),
                        receptor_atoms: src(&p.heavy),
                        residue: p.residue,
                        distance: d as f32,
                    });
                }
            }
        }
    }

    // π-stacking.
    let max_pi = f64::from(config.pistack_max_distance);
    let dev = f64::from(config.pistack_max_angle_deviation);
    let max_off = f64::from(config.pistack_max_offset);
    let mut stacked: Vec<(u32, usize)> = Vec::new(); // (receptor residue, ligand ring)
    for (li, (lc, ln)) in ligand_rings.iter().enumerate() {
        for rr in &rs.rings {
            let d = dist_v(*lc, rr.centre);
            if !(d > min && d < max_pi) {
                continue;
            }
            let angle = plane_angle(*ln, rr.normal);
            let off = offset(*lc, *ln, rr.centre).min(offset(rr.centre, rr.normal, *lc));
            if off >= max_off {
                continue;
            }
            let kind = if angle < dev {
                StackKind::Parallel
            } else if angle > 90.0 - dev {
                StackKind::TShaped
            } else {
                continue;
            };
            stacked.push((rr.residue, li));
            out.pi_stacks.push(PiStack {
                kind,
                ligand_atoms: ls.rings[li].clone(),
                receptor_atoms: src(&rr.heavy),
                residue: rr.residue,
                distance: d as f32,
                angle: angle as f32,
                offset: off as f32,
            });
        }
    }

    // Cation–π.
    let max_cp = f64::from(config.cation_pi_max_distance);
    for g in &ls.positive {
        let c = centre_of(g);
        for rr in &rs.rings {
            let d = dist_v(c, rr.centre);
            let off = offset(rr.centre, rr.normal, c);
            if !(d > min && d < max_cp) || off >= max_off {
                continue;
            }
            if let Some([x, y, z]) = g.amine_neighbours {
                let amine_normal = cross(sub(pos(y), pos(x)), sub(pos(x), pos(z)));
                if plane_angle(rr.normal, amine_normal)
                    > f64::from(config.cation_pi_amine_max_angle)
                {
                    continue;
                }
            }
            out.cation_pi.push(CationPi {
                cation: Side::Ligand,
                ligand_atoms: g.atoms.clone(),
                receptor_atoms: src(&rr.heavy),
                residue: rr.residue,
                distance: d as f32,
                offset: off as f32,
            });
        }
    }
    for (li, (lc, ln)) in ligand_rings.iter().enumerate() {
        for p in &rs.positive {
            let d = dist_v(p.centre, *lc);
            let off = offset(*lc, *ln, p.centre);
            if !(d > min && d < max_cp) || off >= max_off {
                continue;
            }
            // PLIP: a histidine stacking on this ring is reported as stacking only.
            if p.histidine && stacked.contains(&(p.residue, li)) {
                continue;
            }
            out.cation_pi.push(CationPi {
                cation: Side::Receptor,
                ligand_atoms: ls.rings[li].clone(),
                receptor_atoms: src(&p.heavy),
                residue: p.residue,
                distance: d as f32,
                offset: off as f32,
            });
        }
    }

    // Hydrogen bonds: one radius query per ligand polar atom on the receptor's tree.
    let max_hb = config.hbond_max_distance;
    let min_angle = f64::from(config.hbond_min_angle);
    let mut hbonds: Vec<HBond> = Vec::new();
    for &l in &ls.polar {
        let li = l as usize;
        let p = positions[li];
        let lig_donor = ls.donor[li].then(|| {
            let mut heavy = Vec::new();
            let mut hyd = Vec::new();
            for &(x, _) in topology.neighbours(l) {
                if topology.elements()[x as usize] == Element::H {
                    hyd.push(pos(x));
                } else {
                    heavy.push(pos(x));
                }
            }
            (heavy, hyd)
        });
        receptor.index().for_each_within(p, max_hb, |nb| {
            let d = f64::from(nb.distance);
            if d <= min || nb.distance >= max_hb {
                return;
            }
            let k = nb.index as usize;
            let candidate = |donor: Side, found: Option<(f64, Hydrogen, bool)>| {
                found.and_then(|(angle, hydrogen, rotatable)| {
                    (angle > min_angle).then(|| HBond {
                        donor,
                        ligand_atom: l,
                        receptor_atom: receptor.source_index(nb.index),
                        residue: receptor.heavy_residue(nb.index),
                        distance: nb.distance,
                        angle: angle as f32,
                        hydrogen,
                        rotatable_hydrogen: rotatable,
                    })
                })
            };
            if ls.acceptor[li] && rs.donor_of[k] != u32::MAX {
                let rd = &rs.donors[rs.donor_of[k] as usize];
                debug_assert_eq!(rd.heavy, nb.index);
                let found = donor_angle(
                    v3(receptor.heavy_position(nb.index)),
                    &rd.neighbours,
                    rd.hybridization,
                    &rd.hydrogens,
                    v3(p),
                );
                hbonds.extend(candidate(Side::Receptor, found));
            }
            if let Some((heavy, hyd)) = &lig_donor
                && rs.acceptor[k]
            {
                let found = donor_angle(
                    v3(p),
                    heavy,
                    topology.hybridization(l),
                    hyd,
                    v3(receptor.heavy_position(nb.index)),
                );
                hbonds.extend(candidate(Side::Ligand, found));
            }
        });
    }
    // PLIP's refinement: no H-bond between two groups that already form a salt
    // bridge, and one H-bond per donor atom (the largest angle).
    hbonds.retain(|h| {
        !out.salt_bridges.iter().any(|s| {
            s.ligand_atoms.contains(&h.ligand_atom) && s.receptor_atoms.contains(&h.receptor_atom)
        })
    });
    hbonds.sort_by(|a, b| {
        let key = |h: &HBond| match h.donor {
            Side::Ligand => (0, h.ligand_atom),
            Side::Receptor => (1, h.receptor_atom),
        };
        key(a)
            .cmp(&key(b))
            .then(b.angle.total_cmp(&a.angle))
            .then((a.ligand_atom, a.receptor_atom).cmp(&(b.ligand_atom, b.receptor_atom)))
    });
    hbonds.dedup_by(|later, first| {
        later.donor == first.donor
            && match later.donor {
                Side::Ligand => later.ligand_atom == first.ligand_atom,
                Side::Receptor => later.receptor_atom == first.receptor_atom,
            }
    });
    out.hbonds = hbonds;

    out.hbonds
        .sort_by_key(|h| (h.ligand_atom, h.receptor_atom, h.donor == Side::Receptor));
    out.salt_bridges.sort_by(|a, b| {
        (&a.ligand_atoms, &a.receptor_atoms).cmp(&(&b.ligand_atoms, &b.receptor_atoms))
    });
    out.pi_stacks.sort_by(|a, b| {
        (&a.ligand_atoms, &a.receptor_atoms).cmp(&(&b.ligand_atoms, &b.receptor_atoms))
    });
    out.cation_pi.sort_by(|a, b| {
        (&a.ligand_atoms, &a.receptor_atoms).cmp(&(&b.ligand_atoms, &b.receptor_atoms))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_hydrogen_decides_over_placement() {
        // Donor O at the origin bonded to one C along -x; acceptor 2.9 Å away
        // at a C–O···A angle of 135°.
        let d = [0.0, 0.0, 0.0];
        let c = [-1.45, 0.0, 0.0];
        let a = [2.05, 2.05, 0.0];
        // Placed (cone): the best rotamer points nearly at the acceptor.
        let (placed, how, rot) = donor_angle(d, &[c], Hybridization::Sp3, &[], a).unwrap();
        assert_eq!((how, rot), (Hydrogen::Placed, true));
        assert!(placed > 100.0, "{placed}");
        // An explicit H pointing away from the acceptor: no H-bond geometry.
        let h_away = [0.3, -0.91, 0.0];
        let (explicit, how, _) = donor_angle(d, &[c], Hybridization::Sp3, &[h_away], a).unwrap();
        assert_eq!(how, Hydrogen::Explicit);
        assert!(explicit < 100.0, "{explicit}");
        // On the C–O axis no rotamer reaches 100°: C–O···A is 180°.
        let (on_axis, _, _) =
            donor_angle(d, &[c], Hybridization::Sp3, &[], [2.9, 0.0, 0.0]).unwrap();
        assert!(on_axis < 100.0, "{on_axis}");
    }

    #[test]
    fn sp2_donor_with_two_neighbours_points_along_the_bisector() {
        // Backbone-like N: neighbours at ±120° in the xy plane, H along +x.
        let d = [0.0, 0.0, 0.0];
        let x1 = [-0.7, 1.2, 0.0];
        let x2 = [-0.7, -1.2, 0.0];
        let (on_axis, _, rot) =
            donor_angle(d, &[x1, x2], Hybridization::Sp2, &[], [3.0, 0.0, 0.0]).unwrap();
        assert!(!rot);
        assert!(on_axis > 179.0, "{on_axis}");
        let (behind, _, _) =
            donor_angle(d, &[x1, x2], Hybridization::Sp2, &[], [-3.0, 0.5, 0.0]).unwrap();
        assert!(behind < 100.0, "{behind}");
    }

    #[test]
    fn implicit_hydrogens_follow_valence_and_charge() {
        assert_eq!(implicit_hydrogens(Element::N, 0, 4), 1); // aniline NH: 2 single bonds
        assert_eq!(implicit_hydrogens(Element::N, 0, 6), 0); // pyridine (Kekulé)
        assert_eq!(implicit_hydrogens(Element::N, 1, 6), 1); // R3NH+
        assert_eq!(implicit_hydrogens(Element::O, -1, 2), 0); // carboxylate O−
        assert_eq!(implicit_hydrogens(Element::O, 0, 2), 1); // hydroxyl
        assert_eq!(implicit_hydrogens(Element::C, 0, 3 + 3), 1); // aromatic CH
        assert_eq!(implicit_hydrogens(Element::S, 0, 8), 0); // sulfonyl, hypervalent
    }
}
