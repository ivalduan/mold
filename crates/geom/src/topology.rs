//! Ligand topology: the bond graph the arena deliberately does not carry.
//!
//! A [`Topology`] is built once per ligand (elements + an explicit bond list,
//! e.g. from an SDF) and then reused for every pose of that ligand: it
//! precomputes everything the ligand-internal checks need that depends only
//! on connectivity — rings, aromaticity, hybridisation, reference bond lengths
//! and 1-3 distances, and the non-bonded pair list.
//!
//! Hydrogens are kept in the graph (they matter for ring perception and
//! hybridisation) but every precomputed check list holds heavy atoms only, as
//! PoseBusters does with `ignore_hydrogens=True`.

use std::collections::{HashSet, VecDeque};
use std::fmt;

use mold_mol::Element;

use crate::radii::{reference_bond_length, vdw_radius};

/// Bond order as given by the input (V2000 bond types 1–4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BondOrder {
    Single,
    Double,
    Triple,
    Aromatic,
}

/// One bond: two atom indices into the ligand's position/element slices, and its order.
pub type Bond = (u32, u32, BondOrder);

/// Orbital hybridisation as far as geometry needs it: it sets the ideal bond angle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hybridization {
    /// Linear, 180°.
    Sp,
    /// Trigonal planar, 120°.
    Sp2,
    /// Tetrahedral, 109.47°.
    Sp3,
}

/// The bond list does not fit the atoms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyError {
    /// A bond names an atom index `>= atom_count`.
    AtomOutOfRange { bond: usize, atom: u32 },
    /// A bond joins an atom to itself.
    SelfBond { bond: usize },
    /// The same atom pair is bonded twice.
    DuplicateBond { bond: usize },
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopologyError::AtomOutOfRange { bond, atom } => {
                write!(f, "bond {bond} references atom {atom}, out of range")
            }
            TopologyError::SelfBond { bond } => write!(f, "bond {bond} joins an atom to itself"),
            TopologyError::DuplicateBond { bond } => write!(f, "bond {bond} is a duplicate"),
        }
    }
}

impl std::error::Error for TopologyError {}

/// A bond with its precomputed reference length (heavy atoms only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BondRef {
    pub a: u32,
    pub b: u32,
    /// The order used for geometry: `Aromatic` for bonds in perceived aromatic rings.
    pub order: BondOrder,
    /// Reference length in Å (sum of covalent radii for `order`).
    pub length: f32,
}

/// A bond angle `a–center–b` with its reference 1-3 distance (heavy atoms only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngleRef {
    pub a: u32,
    pub center: u32,
    pub b: u32,
    /// Ideal angle in degrees (from hybridisation and small-ring membership).
    pub ideal_degrees: f32,
    /// Reference `a`–`b` distance in Å: law of cosines over the reference bond
    /// lengths and the ideal angle.
    pub distance: f32,
}

/// A heavy-atom pair separated by three or more bonds (or disconnected): the
/// pairs the internal steric clash check looks at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NonBondedPair {
    pub a: u32,
    pub b: u32,
    /// Bonds on the shortest path, `u8::MAX` if not connected.
    pub separation: u8,
    /// Sum of the two van der Waals radii, Å.
    pub vdw_sum: f32,
}

/// The bond graph of one ligand plus everything derived from it. See the module docs.
#[derive(Debug, Clone)]
pub struct Topology {
    elements: Vec<Element>,
    bonds: Vec<Bond>,
    neighbours: Vec<Vec<(u32, BondOrder)>>,
    rings: Vec<Vec<u32>>,
    aromatic: Vec<bool>,
    hybridization: Vec<Hybridization>,
    bond_refs: Vec<BondRef>,
    angle_refs: Vec<AngleRef>,
    non_bonded: Vec<NonBondedPair>,
}

fn is_hydrogen(e: Element) -> bool {
    e == Element::H
}

impl Topology {
    /// Build the graph for atoms with these `elements` and `bonds`.
    pub fn new(elements: &[Element], bonds: &[Bond]) -> Result<Topology, TopologyError> {
        let n = elements.len();
        let mut neighbours = vec![Vec::new(); n];
        let mut seen = HashSet::new();
        for (i, &(a, b, order)) in bonds.iter().enumerate() {
            for atom in [a, b] {
                if atom as usize >= n {
                    return Err(TopologyError::AtomOutOfRange { bond: i, atom });
                }
            }
            if a == b {
                return Err(TopologyError::SelfBond { bond: i });
            }
            if !seen.insert((a.min(b), a.max(b))) {
                return Err(TopologyError::DuplicateBond { bond: i });
            }
            neighbours[a as usize].push((b, order));
            neighbours[b as usize].push((a, order));
        }
        let mut t = Topology {
            elements: elements.to_vec(),
            bonds: bonds.to_vec(),
            neighbours,
            rings: Vec::new(),
            aromatic: Vec::new(),
            hybridization: Vec::new(),
            bond_refs: Vec::new(),
            angle_refs: Vec::new(),
            non_bonded: Vec::new(),
        };
        t.rings = t.perceive_rings();
        t.aromatic = t.rings.iter().map(|r| t.ring_is_aromatic(r)).collect();
        t.hybridization = (0..n).map(|i| t.perceive_hybridization(i)).collect();
        t.bond_refs = t.build_bond_refs();
        t.angle_refs = t.build_angle_refs();
        t.non_bonded = t.build_non_bonded();
        Ok(t)
    }

    pub fn atom_count(&self) -> usize {
        self.elements.len()
    }

    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    pub fn bonds(&self) -> &[Bond] {
        &self.bonds
    }

    /// Bonded neighbours of `atom`, with the bond order as given.
    pub fn neighbours(&self, atom: u32) -> &[(u32, BondOrder)] {
        &self.neighbours[atom as usize]
    }

    /// Indices of the heavy (non-hydrogen) atoms.
    pub fn heavy_atoms(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.elements.len() as u32).filter(|&i| !is_hydrogen(self.elements[i as usize]))
    }

    /// Rings: the smallest cycle through each bond, deduplicated, atoms in
    /// cycle order. For drug-like molecules (fused and spiro systems, no
    /// cages) this is the SSSR.
    pub fn rings(&self) -> &[Vec<u32>] {
        &self.rings
    }

    /// The perceived aromatic rings (5- and 6-membered, Hückel 4n+2 over the
    /// Kekulé bond orders, or all ring bonds given as aromatic).
    pub fn aromatic_rings(&self) -> impl Iterator<Item = &[u32]> + '_ {
        self.rings
            .iter()
            .zip(&self.aromatic)
            .filter(|(_, a)| **a)
            .map(|(r, _)| r.as_slice())
    }

    pub fn hybridization(&self, atom: u32) -> Hybridization {
        self.hybridization[atom as usize]
    }

    /// Heavy-atom bonds with reference lengths.
    pub fn bond_refs(&self) -> &[BondRef] {
        &self.bond_refs
    }

    /// Heavy-atom angles with reference 1-3 distances.
    pub fn angle_refs(&self) -> &[AngleRef] {
        &self.angle_refs
    }

    /// Heavy-atom pairs three or more bonds apart.
    pub fn non_bonded_pairs(&self) -> &[NonBondedPair] {
        &self.non_bonded
    }

    /// `true` if `a` and `b` are directly bonded.
    pub fn bonded(&self, a: u32, b: u32) -> bool {
        self.neighbours[a as usize].iter().any(|&(x, _)| x == b)
    }

    fn ring_membership(&self) -> Vec<Vec<usize>> {
        let mut m = vec![Vec::new(); self.elements.len()];
        for (ri, r) in self.rings.iter().enumerate() {
            for &a in r {
                m[a as usize].push(ri);
            }
        }
        m
    }

    /// Shortest path `from -> to` that does not use the direct bond between them.
    fn shortest_path_avoiding_edge(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        let n = self.elements.len();
        let mut prev = vec![u32::MAX; n];
        let mut queue = VecDeque::new();
        prev[from as usize] = from;
        queue.push_back(from);
        while let Some(x) = queue.pop_front() {
            for &(y, _) in &self.neighbours[x as usize] {
                if x == from && y == to {
                    continue;
                }
                if prev[y as usize] != u32::MAX {
                    continue;
                }
                prev[y as usize] = x;
                if y == to {
                    let mut path = vec![to];
                    let mut c = to;
                    while c != from {
                        c = prev[c as usize];
                        path.push(c);
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(y);
            }
        }
        None
    }

    fn perceive_rings(&self) -> Vec<Vec<u32>> {
        let mut rings: Vec<Vec<u32>> = Vec::new();
        let mut keys: HashSet<Vec<u32>> = HashSet::new();
        for &(a, b, _) in &self.bonds {
            if let Some(path) = self.shortest_path_avoiding_edge(a, b) {
                let mut key = path.clone();
                key.sort_unstable();
                if keys.insert(key) {
                    rings.push(path);
                }
            }
        }
        rings.sort_by(|x, y| x.len().cmp(&y.len()).then_with(|| x.cmp(y)));
        rings
    }

    /// Hückel test over Kekulé bond orders. Each ring atom contributes π
    /// electrons: 1 for a double bond to a ring atom (this ring or a fused
    /// one), 0 for an exocyclic double bond (C=O), 2 for a lone-pair
    /// heteroatom (N, O, S, Se, P) with only single bonds. A saturated carbon or
    /// a triple bond disqualifies the ring.
    fn ring_is_aromatic(&self, ring: &[u32]) -> bool {
        if !(5..=6).contains(&ring.len()) {
            return false;
        }
        let in_ring = |x: u32| ring.contains(&x);
        let all_aromatic = ring.iter().enumerate().all(|(k, &a)| {
            let b = ring[(k + 1) % ring.len()];
            self.neighbours[a as usize]
                .iter()
                .any(|&(x, o)| x == b && o == BondOrder::Aromatic)
        });
        if all_aromatic {
            return true;
        }
        let in_any_ring = |x: u32| self.rings.iter().any(|r| r.contains(&x));
        let mut electrons = 0u32;
        for &a in ring {
            let nb = &self.neighbours[a as usize];
            if nb.iter().any(|&(_, o)| o == BondOrder::Triple) {
                return false;
            }
            let doubles: Vec<u32> = nb
                .iter()
                .filter(|&&(_, o)| o == BondOrder::Double || o == BondOrder::Aromatic)
                .map(|&(x, _)| x)
                .collect();
            if let Some(&x) = doubles.first() {
                electrons += if in_ring(x) || in_any_ring(x) { 1 } else { 0 };
            } else {
                let z = self.elements[a as usize].atomic_number();
                let lone_pair = matches!(z, 7 | 8 | 15 | 16 | 34) && nb.len() <= 3;
                if !lone_pair {
                    return false;
                }
                electrons += 2;
            }
        }
        electrons >= 2 && (electrons - 2).is_multiple_of(4)
    }

    fn in_aromatic_ring(&self, a: u32, b: u32) -> bool {
        self.rings
            .iter()
            .zip(&self.aromatic)
            .any(|(r, &ar)| ar && r.contains(&a) && r.contains(&b))
    }

    /// Bond order for geometry: aromatic inside a perceived aromatic ring.
    pub fn effective_order(&self, a: u32, b: u32, given: BondOrder) -> BondOrder {
        if given != BondOrder::Triple && self.in_aromatic_ring(a, b) {
            BondOrder::Aromatic
        } else {
            given
        }
    }

    fn perceive_hybridization(&self, atom: usize) -> Hybridization {
        let nb = &self.neighbours[atom];
        let triples = nb.iter().filter(|(_, o)| *o == BondOrder::Triple).count();
        let doubles = nb.iter().filter(|(_, o)| *o == BondOrder::Double).count();
        let aromatic = nb.iter().any(|(_, o)| *o == BondOrder::Aromatic)
            || self
                .rings
                .iter()
                .zip(&self.aromatic)
                .any(|(r, &ar)| ar && r.contains(&(atom as u32)));
        if triples > 0 || doubles >= 2 {
            return Hybridization::Sp;
        }
        if doubles == 1 || aromatic {
            return Hybridization::Sp2;
        }
        // Conjugated lone-pair heteroatoms (amide/aniline N, aryl ether O) are
        // planar, as RDKit assigns them.
        let z = self.elements[atom].atomic_number();
        if matches!(z, 7 | 8) && nb.len() <= 3 {
            let conjugated = nb.iter().any(|&(x, _)| {
                let xn = &self.neighbours[x as usize];
                xn.iter().any(|(_, o)| *o != BondOrder::Single)
                    || self
                        .rings
                        .iter()
                        .zip(&self.aromatic)
                        .any(|(r, &ar)| ar && r.contains(&x))
            });
            if conjugated {
                return Hybridization::Sp2;
            }
        }
        Hybridization::Sp3
    }

    fn build_bond_refs(&self) -> Vec<BondRef> {
        self.bonds
            .iter()
            .filter(|(a, b, _)| {
                !is_hydrogen(self.elements[*a as usize]) && !is_hydrogen(self.elements[*b as usize])
            })
            .map(|&(a, b, given)| {
                let order = self.effective_order(a, b, given);
                BondRef {
                    a,
                    b,
                    order,
                    length: reference_bond_length(
                        self.elements[a as usize],
                        self.elements[b as usize],
                        order,
                    ),
                }
            })
            .collect()
    }

    fn bond_ref_length(&self, a: u32, b: u32) -> f32 {
        let given = self.neighbours[a as usize]
            .iter()
            .find(|&&(x, _)| x == b)
            .map_or(BondOrder::Single, |&(_, o)| o);
        let order = self.effective_order(a, b, given);
        reference_bond_length(self.elements[a as usize], self.elements[b as usize], order)
    }

    fn build_angle_refs(&self) -> Vec<AngleRef> {
        let membership = self.ring_membership();
        let mut out = Vec::new();
        for center in 0..self.elements.len() as u32 {
            if is_hydrogen(self.elements[center as usize]) {
                continue;
            }
            let heavy: Vec<u32> = self.neighbours[center as usize]
                .iter()
                .map(|&(x, _)| x)
                .filter(|&x| !is_hydrogen(self.elements[x as usize]))
                .collect();
            for i in 0..heavy.len() {
                for j in (i + 1)..heavy.len() {
                    let (a, b) = (heavy[i].min(heavy[j]), heavy[i].max(heavy[j]));
                    // Smallest ring containing a-center-b decides small-ring angles.
                    let ring_size = membership[center as usize]
                        .iter()
                        .map(|&ri| &self.rings[ri])
                        .filter(|r| r.contains(&a) && r.contains(&b))
                        .map(|r| r.len())
                        .min();
                    let ideal = match ring_size {
                        Some(3) => 60.0,
                        Some(4) => 90.0,
                        Some(5) => 108.0,
                        _ => match self.hybridization[center as usize] {
                            Hybridization::Sp => 180.0,
                            Hybridization::Sp2 => 120.0,
                            Hybridization::Sp3 => 109.47,
                        },
                    };
                    let la = self.bond_ref_length(center, a);
                    let lb = self.bond_ref_length(center, b);
                    let cos = (ideal as f32).to_radians().cos();
                    let distance = (la * la + lb * lb - 2.0 * la * lb * cos).max(0.0).sqrt();
                    out.push(AngleRef {
                        a,
                        center,
                        b,
                        ideal_degrees: ideal as f32,
                        distance,
                    });
                }
            }
        }
        out
    }

    /// Bond separation from `source` to every atom, capped at `cap`
    /// (atoms further away, or unreachable, get `u8::MAX`).
    fn separations_from(&self, source: u32, cap: u8) -> Vec<u8> {
        let mut d = vec![u8::MAX; self.elements.len()];
        d[source as usize] = 0;
        let mut queue = VecDeque::from([source]);
        while let Some(x) = queue.pop_front() {
            let dx = d[x as usize];
            if dx >= cap {
                continue;
            }
            for &(y, _) in &self.neighbours[x as usize] {
                if d[y as usize] == u8::MAX {
                    d[y as usize] = dx + 1;
                    queue.push_back(y);
                }
            }
        }
        d
    }

    fn build_non_bonded(&self) -> Vec<NonBondedPair> {
        let heavy: Vec<u32> = self.heavy_atoms().collect();
        let mut out = Vec::new();
        for (k, &a) in heavy.iter().enumerate() {
            let sep = self.separations_from(a, u8::MAX - 1);
            for &b in &heavy[k + 1..] {
                let s = sep[b as usize];
                if s >= 3 {
                    out.push(NonBondedPair {
                        a,
                        b,
                        separation: s,
                        vdw_sum: vdw_radius(self.elements[a as usize])
                            + vdw_radius(self.elements[b as usize]),
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Element = Element::C;
    const N: Element = Element::N;
    const O: Element = Element::O;

    fn ring_bonds(n: u32, orders: &[BondOrder]) -> Vec<Bond> {
        (0..n)
            .map(|i| (i, (i + 1) % n, orders[i as usize % orders.len()]))
            .collect()
    }

    #[test]
    fn kekule_benzene_is_aromatic() {
        use BondOrder::*;
        let t = Topology::new(&[C; 6], &ring_bonds(6, &[Single, Double])).unwrap();
        assert_eq!(t.rings().len(), 1);
        assert_eq!(t.aromatic_rings().count(), 1);
        assert!(t.bond_refs().iter().all(|b| b.order == Aromatic));
        assert_eq!(t.hybridization(0), Hybridization::Sp2);
    }

    #[test]
    fn cyclohexane_is_not_aromatic() {
        let t = Topology::new(&[C; 6], &ring_bonds(6, &[BondOrder::Single])).unwrap();
        assert_eq!(t.rings().len(), 1);
        assert_eq!(t.aromatic_rings().count(), 0);
        assert_eq!(t.hybridization(0), Hybridization::Sp3);
    }

    #[test]
    fn naphthalene_in_either_kekule_form_has_two_aromatic_rings() {
        use BondOrder::*;
        // Atoms 0-9; ring A: 0,1,2,3,4,9 ; ring B: 4,5,6,7,8,9 ; fusion bond 4-9.
        let form1 = vec![
            (0, 1, Double),
            (1, 2, Single),
            (2, 3, Double),
            (3, 4, Single),
            (4, 9, Double),
            (9, 0, Single),
            (4, 5, Single),
            (5, 6, Double),
            (6, 7, Single),
            (7, 8, Double),
            (8, 9, Single),
        ];
        let form2 = vec![
            (0, 1, Double),
            (1, 2, Single),
            (2, 3, Double),
            (3, 4, Single),
            (4, 9, Single),
            (9, 0, Single),
            (4, 5, Double),
            (5, 6, Single),
            (6, 7, Double),
            (7, 8, Single),
            (8, 9, Double),
        ];
        for bonds in [form1, form2] {
            let t = Topology::new(&[C; 10], &bonds).unwrap();
            assert_eq!(t.rings().len(), 2, "two six-rings, not the ten-ring");
            assert!(t.rings().iter().all(|r| r.len() == 6));
            assert_eq!(t.aromatic_rings().count(), 2);
        }
    }

    #[test]
    fn pyrrole_and_pyridone_follow_huckel() {
        use BondOrder::*;
        // pyrrole: N0 C1=C2 C3=C4
        let pyrrole = [
            (0, 1, Single),
            (1, 2, Double),
            (2, 3, Single),
            (3, 4, Double),
            (4, 0, Single),
        ];
        let t = Topology::new(&[N, C, C, C, C], &pyrrole).unwrap();
        assert_eq!(t.aromatic_rings().count(), 1);
        // 2-pyridone: N0 C1(=O6) C2=C3 C4=C5 ; RDKit calls it aromatic.
        let pyridone = [
            (0, 1, Single),
            (1, 2, Single),
            (2, 3, Double),
            (3, 4, Single),
            (4, 5, Double),
            (5, 0, Single),
            (1, 6, Double),
        ];
        let t = Topology::new(&[N, C, C, C, C, C, O], &pyridone).unwrap();
        assert_eq!(t.aromatic_rings().count(), 1);
    }

    #[test]
    fn non_bonded_pairs_exclude_one_two_and_one_three() {
        // Butane chain C0-C1-C2-C3-C4: pairs (0,3), (0,4), (1,4) only.
        let bonds: Vec<Bond> = (0..4).map(|i| (i, i + 1, BondOrder::Single)).collect();
        let t = Topology::new(&[C; 5], &bonds).unwrap();
        let pairs: Vec<(u32, u32, u8)> = t
            .non_bonded_pairs()
            .iter()
            .map(|p| (p.a, p.b, p.separation))
            .collect();
        assert_eq!(pairs, vec![(0, 3, 3), (0, 4, 4), (1, 4, 3)]);
        assert_eq!(t.angle_refs().len(), 3);
    }

    #[test]
    fn bad_bond_lists_are_rejected() {
        let s = BondOrder::Single;
        assert_eq!(
            Topology::new(&[C; 2], &[(0, 2, s)]).unwrap_err(),
            TopologyError::AtomOutOfRange { bond: 0, atom: 2 }
        );
        assert_eq!(
            Topology::new(&[C; 2], &[(1, 1, s)]).unwrap_err(),
            TopologyError::SelfBond { bond: 0 }
        );
        assert_eq!(
            Topology::new(&[C; 2], &[(0, 1, s), (1, 0, s)]).unwrap_err(),
            TopologyError::DuplicateBond { bond: 1 }
        );
    }
}
