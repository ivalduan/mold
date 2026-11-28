//! Protein–ligand checks against a receptor indexed once: smallest distance,
//! pairwise clashes, maximum distance, volume overlap, and covalent attachments.

use std::collections::HashMap;
use std::fmt;

use mold_mol::{Arena, AtomName, Element, PoseId};

use crate::contacts::{ReceptorSites, type_receptor};
use crate::linalg::dist2_f32;
use crate::radii::{covalent_radius, vdw_radius};
use crate::spatial::SpatialIndex;
use crate::topology::{BondOrder, Topology};

/// Which residue a receptor atom belongs to, copied from the arena.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResidueId {
    /// Author chain id.
    pub chain: String,
    /// Residue name (`"MET"`).
    pub name: String,
    pub seq_num: i32,
    pub insertion_code: Option<char>,
}

impl fmt::Display for ResidueId {
    /// `A:MET:793` (`A:MET:793B` with an insertion code).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.chain, self.name, self.seq_num)?;
        if let Some(c) = self.insertion_code {
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

/// A receptor's heavy atoms with their radii and a KD-tree. Build once, check
/// every pose against it.
///
/// Protein atom indices in every result ([`Contact::protein`],
/// [`CovalentAttachment::protein_atom`], the [`crate::contacts`] lists) refer
/// to the slices passed to [`Receptor::new`] — or, for
/// [`Receptor::from_arena`], to the pose's atoms (pose-local, like
/// [`Arena::pose_positions`]) — not to the internal heavy-atom subset.
#[derive(Debug)]
pub struct Receptor {
    positions: Vec<[f32; 3]>,
    elements: Vec<Element>,
    radii: Vec<f32>,
    source: Vec<u32>,
    index: SpatialIndex,
    max_vdw: f32,
    /// Per heavy atom; empty for [`Receptor::new`] (no names, no residues).
    names: Vec<AtomName>,
    residue_of: Vec<u32>,
    residues: Vec<ResidueId>,
    /// Contact typing, built once. Empty for [`Receptor::new`].
    sites: ReceptorSites,
}

impl Receptor {
    /// A receptor from one arena pose, with atom names and residue identity, so
    /// that [`crate::contacts::contacts`] can type it (donors, acceptors,
    /// charged groups and aromatic rings by residue and atom name). Every atom
    /// of the pose is kept, waters and hetero residues included (they count
    /// for clashes; waters are not typed for contacts). `None` if the arena
    /// has no such pose.
    pub fn from_arena(arena: &Arena, pose: PoseId) -> Option<Receptor> {
        Receptor::from_arena_filtered(arena, pose, |_| true)
    }

    /// [`Receptor::from_arena`] keeping only the atoms (global arena indices)
    /// for which `keep` is true, e.g. polymer atoms only, as PoseBusters'
    /// protein checks see a receptor.
    pub fn from_arena_filtered(
        arena: &Arena,
        pose: PoseId,
        mut keep: impl FnMut(usize) -> bool,
    ) -> Option<Receptor> {
        let atoms = arena.pose(pose)?.atoms.clone();
        let start = atoms.start;
        let mut r = Receptor::empty(atoms.len());
        let mut hydrogens = Vec::new();
        let mut residue_map: HashMap<u32, u32> = HashMap::new();
        for i in atoms {
            if !keep(i) {
                continue;
            }
            let e = arena.elements()[i];
            let p = arena.positions()[i];
            if e == Element::H {
                hydrogens.push(p);
                continue;
            }
            let ri = arena.residue_indices()[i];
            let next = r.residues.len() as u32;
            let local = *residue_map.entry(ri).or_insert_with(|| {
                let res = &arena.residues()[ri as usize];
                r.residues.push(ResidueId {
                    chain: arena.chains()[res.chain as usize].id.clone(),
                    name: res.name.clone(),
                    seq_num: res.seq_num,
                    insertion_code: res.insertion_code,
                });
                next
            });
            r.push(p, e, (i - start) as u32);
            r.names.push(arena.atom_names()[i]);
            r.residue_of.push(local);
        }
        r.index = SpatialIndex::new(&r.positions);
        r.sites = type_receptor(&r, &hydrogens);
        Some(r)
    }

    fn empty(capacity: usize) -> Receptor {
        Receptor {
            positions: Vec::with_capacity(capacity),
            elements: Vec::with_capacity(capacity),
            radii: Vec::with_capacity(capacity),
            source: Vec::with_capacity(capacity),
            index: SpatialIndex::new(&[]),
            max_vdw: 0.0,
            names: Vec::new(),
            residue_of: Vec::new(),
            residues: Vec::new(),
            sites: ReceptorSites::default(),
        }
    }

    fn push(&mut self, p: [f32; 3], e: Element, source: u32) {
        self.positions.push(p);
        self.elements.push(e);
        let rad = vdw_radius(e);
        self.max_vdw = self.max_vdw.max(rad);
        self.radii.push(rad);
        self.source.push(source);
    }

    /// Residues of the typed atoms ([`Receptor::from_arena`]; empty for
    /// [`Receptor::new`]). Contact lists index into this.
    pub fn residues(&self) -> &[ResidueId] {
        &self.residues
    }

    /// Residue of a receptor atom (source index), if the receptor has names.
    pub fn residue_of(&self, source: u32) -> Option<&ResidueId> {
        let k = self.heavy_of_source(source)? as usize;
        let r = *self.residue_of.get(k)?;
        self.residues.get(r as usize)
    }

    /// Atom name of a receptor atom (source index), if the receptor has names.
    pub fn atom_name(&self, source: u32) -> Option<AtomName> {
        let k = self.heavy_of_source(source)? as usize;
        self.names.get(k).copied()
    }

    pub(crate) fn heavy_position(&self, k: u32) -> [f32; 3] {
        self.positions[k as usize]
    }

    pub(crate) fn heavy_element(&self, k: u32) -> Element {
        self.elements[k as usize]
    }

    pub(crate) fn heavy_name(&self, k: u32) -> Option<AtomName> {
        self.names.get(k as usize).copied()
    }

    pub(crate) fn heavy_residue(&self, k: u32) -> u32 {
        self.residue_of[k as usize]
    }

    pub(crate) fn sites(&self) -> &ReceptorSites {
        &self.sites
    }

    /// Index the heavy atoms of `positions`/`elements` (hydrogens are dropped,
    /// as PoseBusters does with `ignore_types={"hydrogens"}`).
    ///
    /// # Panics
    /// If the slices differ in length.
    pub fn new(positions: &[[f32; 3]], elements: &[Element]) -> Receptor {
        assert_eq!(
            positions.len(),
            elements.len(),
            "positions/elements length mismatch"
        );
        let mut r = Receptor::empty(positions.len());
        for (i, (&p, &e)) in positions.iter().zip(elements).enumerate() {
            if e == Element::H {
                continue;
            }
            r.push(p, e, i as u32);
        }
        r.index = SpatialIndex::new(&r.positions);
        r
    }

    /// Number of indexed (heavy) atoms.
    pub fn atom_count(&self) -> usize {
        self.positions.len()
    }

    /// The KD-tree over the heavy atoms (items index the heavy-atom subset;
    /// map them back with [`Receptor::source_index`]).
    pub fn index(&self) -> &SpatialIndex {
        &self.index
    }

    /// Index into the slices given to [`Receptor::new`] of heavy atom `k`.
    pub fn source_index(&self, k: u32) -> u32 {
        self.source[k as usize]
    }

    fn heavy_of_source(&self, source: u32) -> Option<u32> {
        self.source.binary_search(&source).ok().map(|k| k as u32)
    }
}

/// One ligand–protein atom pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    /// Ligand atom index.
    pub ligand: u32,
    /// Protein atom index (into the slices given to [`Receptor::new`]).
    pub protein: u32,
    pub ligand_element: Element,
    pub protein_element: Element,
    /// Distance in Å.
    pub distance: f32,
    /// `distance / (radius_scale × (r_vdw,ligand + r_vdw,protein))`.
    pub relative_distance: f32,
}

/// Parameters of [`protein_ligand_distances`]. Defaults are PoseBusters 0.6's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistanceParams {
    /// A pair clashes if its relative distance is below this. Default 0.75.
    pub clash_cutoff: f32,
    /// Scale on the van der Waals radius sum. Default 1.0.
    pub radius_scale: f32,
    /// Only pairs within this many Å are considered for the most extreme
    /// contact (PoseBusters' `search_distance`). Default 6.0.
    pub search_distance: f32,
}

impl Default for DistanceParams {
    fn default() -> Self {
        DistanceParams {
            clash_cutoff: 0.75,
            radius_scale: 1.0,
            search_distance: 6.0,
        }
    }
}

/// Result of [`protein_ligand_distances`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProteinLigandDistances {
    /// The closest heavy-atom pair at any distance (no search cutoff), `None`
    /// if either side has no heavy atoms. PoseBusters leaves its
    /// `smallest_distance_protein` empty when this is beyond `search_distance`.
    pub nearest: Option<Contact>,
    /// Pairs with relative distance below `clash_cutoff`.
    pub clashes: usize,
    /// The pair with the smallest relative distance within `search_distance`.
    pub most_extreme: Option<Contact>,
}

impl ProteinLigandDistances {
    /// Smallest heavy-atom distance in Å (`None` if either side is empty).
    pub fn smallest_distance(&self) -> Option<f32> {
        self.nearest.map(|c| c.distance)
    }

    /// PoseBusters' `minimum_distance_to_protein`: no pair clashes.
    pub fn no_clashes(&self) -> bool {
        self.clashes == 0
    }

    /// PoseBusters' `protein_ligand_maximum_distance`: the ligand is no further
    /// than `max_distance` Å from the protein.
    pub fn within_max_distance(&self, max_distance: f32) -> bool {
        self.smallest_distance().is_some_and(|d| d <= max_distance)
    }
}

/// Distances and clashes between a ligand pose and the receptor (heavy atoms
/// only). Pairs listed in `exclude` (`(ligand_atom, protein_atom)`, protein
/// index as given to [`Receptor::new`]) are skipped entirely — that is how a
/// declared covalent bond is kept out of the clash count; pass `&[]` for the
/// raw PoseBusters check.
pub fn protein_ligand_distances(
    receptor: &Receptor,
    ligand_positions: &[[f32; 3]],
    ligand_elements: &[Element],
    params: &DistanceParams,
    exclude: &[(u32, u32)],
) -> ProteinLigandDistances {
    let mut out = ProteinLigandDistances {
        nearest: None,
        clashes: 0,
        most_extreme: None,
    };
    let excluded = |l: u32, p: u32| exclude.iter().any(|&(el, ep)| el == l && ep == p);
    let contact = |l: u32, k: u32, d: f32| {
        let le = ligand_elements[l as usize];
        let sum = params.radius_scale * (vdw_radius(le) + receptor.radii[k as usize]);
        Contact {
            ligand: l,
            protein: receptor.source[k as usize],
            ligand_element: le,
            protein_element: receptor.elements[k as usize],
            distance: d,
            relative_distance: d / sum,
        }
    };
    for (l, (&p, &e)) in ligand_positions.iter().zip(ligand_elements).enumerate() {
        if e == Element::H {
            continue;
        }
        let l = l as u32;
        // Nearest pair, at any distance. With exclusions the nearest may be an
        // excluded atom, so fall back to a radius search over the search sphere.
        if exclude.is_empty()
            && let Some(n) = receptor.index.nearest(p)
            && out.nearest.is_none_or(|c| n.distance < c.distance)
        {
            out.nearest = Some(contact(l, n.index, n.distance));
        }
        let radius = params.search_distance;
        receptor.index.for_each_within(p, radius, |n| {
            if excluded(l, receptor.source[n.index as usize]) {
                return;
            }
            let c = contact(l, n.index, n.distance);
            if c.relative_distance < params.clash_cutoff {
                out.clashes += 1;
            }
            if out
                .most_extreme
                .is_none_or(|m| c.relative_distance < m.relative_distance)
            {
                out.most_extreme = Some(c);
            }
            if !exclude.is_empty() && out.nearest.is_none_or(|m| c.distance < m.distance) {
                out.nearest = Some(c);
            }
        });
    }
    if !exclude.is_empty() && out.nearest.is_none() {
        // Nothing within the search sphere: the true nearest is beyond it and
        // cannot be an excluded (bonded) pair.
        for (l, (&p, &e)) in ligand_positions.iter().zip(ligand_elements).enumerate() {
            if e == Element::H {
                continue;
            }
            if let Some(n) = receptor.index.nearest(p)
                && out.nearest.is_none_or(|c| n.distance < c.distance)
            {
                out.nearest = Some(contact(l as u32, n.index, n.distance));
            }
        }
    }
    out
}

/// Parameters of [`volume_overlap`]. Defaults mirror RDKit's
/// `ShapeTverskyIndex` as PoseBusters 0.6 calls it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeParams {
    /// Scale on van der Waals radii. Default 0.8.
    pub vdw_scale: f32,
    /// Grid spacing in Å. Default 0.5.
    pub grid_spacing: f32,
    /// Width of each soft boundary layer outside the sphere, Å. Default 0.25.
    pub layer_step: f32,
}

impl Default for VolumeParams {
    fn default() -> Self {
        VolumeParams {
            vdw_scale: 0.8,
            grid_spacing: 0.5,
            layer_step: 0.25,
        }
    }
}

/// Grid occupancy levels: 3 inside the scaled sphere, then 2 and 1 in two soft
/// layers outside it (RDKit's 2-bit shape encoding).
const MAX_LEVEL: u8 = 3;

/// Fraction of the ligand's volume that overlaps the protein, in `[0, 1]`
/// (Tversky index with α = 1, β = 0: `|L ∩ P| / |L|`).
///
/// Both molecules are encoded on one grid the way RDKit's shape encoder does
/// it — each atom a sphere of `vdw_scale × r_vdw` at level 3 with two soft
/// layers at levels 2 and 1, a grid point taking the maximum over atoms — and
/// the intersection is the sum over points of the smaller level. Hydrogens are
/// ignored. Approximate by construction; PoseBusters' own value comes from the
/// same kind of grid, so agreement is to a few hundredths, not exact.
pub fn volume_overlap(
    receptor: &Receptor,
    ligand_positions: &[[f32; 3]],
    ligand_elements: &[Element],
    params: &VolumeParams,
) -> f64 {
    let ligand: Vec<([f32; 3], f32)> = ligand_positions
        .iter()
        .zip(ligand_elements)
        .filter(|(_, e)| **e != Element::H)
        .map(|(&p, &e)| (p, vdw_radius(e) * params.vdw_scale))
        .collect();
    if ligand.is_empty() {
        return 0.0;
    }
    let pad = f32::from(MAX_LEVEL - 1) * params.layer_step;
    let reach = |r: f32| r + pad;
    let h = params.grid_spacing;

    // Grid box: the ligand's soft spheres. Protein density outside it cannot
    // overlap ligand density, so the box bounds both.
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for (p, r) in &ligand {
        for d in 0..3 {
            lo[d] = lo[d].min(p[d] - reach(*r));
            hi[d] = hi[d].max(p[d] + reach(*r));
        }
    }
    // Lattice aligned to multiples of the spacing.
    let i0 = lo.map(|v| (v / h).floor() as i64);
    let i1 = hi.map(|v| (v / h).ceil() as i64);
    let dims = [0, 1, 2].map(|d| (i1[d] - i0[d] + 1) as usize);
    let cells = dims[0] * dims[1] * dims[2];
    let mut lig_grid = vec![0u8; cells];
    let mut prot_grid = vec![0u8; cells];

    let splat = |grid: &mut [u8], center: [f32; 3], radius: f32| {
        let outer = reach(radius);
        // Squared level boundaries: inside the sphere, then each soft layer.
        let mut bounds = [0.0f32; MAX_LEVEL as usize];
        for (k, b) in bounds.iter_mut().enumerate() {
            let r = radius + k as f32 * params.layer_step;
            *b = r * r;
        }
        let mut a = [0usize; 3];
        let mut b = [0usize; 3];
        for d in 0..3 {
            let lo_i = ((center[d] - outer) / h).floor() as i64 - i0[d];
            let hi_i = ((center[d] + outer) / h).ceil() as i64 - i0[d];
            if hi_i < 0 || lo_i >= dims[d] as i64 {
                return;
            }
            a[d] = lo_i.max(0) as usize;
            b[d] = (hi_i.min(dims[d] as i64 - 1)) as usize;
        }
        for x in a[0]..=b[0] {
            let px = (x as i64 + i0[0]) as f32 * h - center[0];
            for y in a[1]..=b[1] {
                let py = (y as i64 + i0[1]) as f32 * h - center[1];
                let row = (x * dims[1] + y) * dims[2];
                for z in a[2]..=b[2] {
                    let pz = (z as i64 + i0[2]) as f32 * h - center[2];
                    let d2 = px * px + py * py + pz * pz;
                    let Some(k) = bounds.iter().position(|&b| d2 < b) else {
                        continue;
                    };
                    let level = MAX_LEVEL - k as u8;
                    let cell = &mut grid[row + z];
                    if level > *cell {
                        *cell = level;
                    }
                }
            }
        }
    };

    for (p, r) in &ligand {
        splat(&mut lig_grid, *p, *r);
    }
    // Protein atoms whose soft sphere can reach a ligand soft sphere.
    let max_prot = reach(receptor.max_vdw * params.vdw_scale);
    let mut near = Vec::new();
    for (p, r) in &ligand {
        receptor
            .index
            .for_each_within(*p, reach(*r) + max_prot, |n| near.push(n.index));
    }
    near.sort_unstable();
    near.dedup();
    for k in near {
        let k = k as usize;
        splat(
            &mut prot_grid,
            receptor.positions[k],
            receptor.radii[k] * params.vdw_scale,
        );
    }
    let (mut inter, mut total) = (0u64, 0u64);
    for (l, p) in lig_grid.iter().zip(&prot_grid) {
        total += u64::from(*l);
        inter += u64::from((*l).min(*p));
    }
    if total == 0 {
        0.0
    } else {
        inter as f64 / total as f64
    }
}

/// A declared covalent bond between a ligand atom and a protein atom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CovalentAttachment {
    pub ligand_atom: u32,
    /// Index into the slices given to [`Receptor::new`].
    pub protein_atom: u32,
}

/// Window, relative to the single-bond covalent radius sum, inside which a
/// ligand–protein pair is taken for a covalent bond by [`detect_covalent_attachments`].
pub const COVALENT_DETECTION_WINDOW: (f32, f32) = (0.85, 1.15);

/// Search radius for [`detect_covalent_attachments`], Å: above the largest
/// plausible pair (Si/Se, 2.32 Å ideal) times the window's upper bound.
const COVALENT_SEARCH_RADIUS: f32 = 2.8;

/// Ligand atoms that form covalent bonds to proteins in practice (warhead
/// carbons, boronic acids, phosphonates, sulfonyl fluorides): C, B, P, S, Si.
fn plausible_ligand_side(e: Element) -> bool {
    matches!(e.atomic_number(), 5 | 6 | 14 | 15 | 16)
}

/// Protein nucleophiles: S (Cys), O (Ser, Thr, Tyr), N (Lys, His, N-terminus), Se (Sec).
fn plausible_protein_side(e: Element) -> bool {
    matches!(e.atomic_number(), 7 | 8 | 16 | 34)
}

/// Detect covalent ligand–protein bonds: plausible element pairs (ligand C/B/P/S/Si
/// to protein S/O/N/Se) whose distance is within [`COVALENT_DETECTION_WINDOW`]
/// of their single-bond covalent radius sum. At most one attachment is
/// returned — the pair closest to its ideal length — because a ligand with
/// several "bonds" to the protein is far likelier to be clashing than to be a
/// multiply-anchored covalent adduct.
pub fn detect_covalent_attachments(
    receptor: &Receptor,
    ligand_positions: &[[f32; 3]],
    ligand_elements: &[Element],
) -> Vec<CovalentAttachment> {
    let (lo, hi) = COVALENT_DETECTION_WINDOW;
    let mut best: Option<(f32, CovalentAttachment)> = None;
    for (l, (&p, &e)) in ligand_positions.iter().zip(ligand_elements).enumerate() {
        if !plausible_ligand_side(e) {
            continue;
        }
        let rl = covalent_radius(e, BondOrder::Single);
        receptor
            .index
            .for_each_within(p, COVALENT_SEARCH_RADIUS, |n| {
                let k = n.index as usize;
                let pe = receptor.elements[k];
                if !plausible_protein_side(pe) {
                    return;
                }
                let ideal = rl + covalent_radius(pe, BondOrder::Single);
                let ratio = n.distance / ideal;
                if ratio < lo || ratio > hi {
                    return;
                }
                let score = (ratio - 1.0).abs();
                if best.is_none_or(|(s, _)| score < s) {
                    best = Some((
                        score,
                        CovalentAttachment {
                            ligand_atom: l as u32,
                            protein_atom: receptor.source[k],
                        },
                    ));
                }
            });
    }
    best.map(|(_, a)| a).into_iter().collect()
}

/// The ligand–protein pairs a covalent bond makes non-clashing by construction:
/// the bonded pair itself, the ligand atom with the protein atom's covalent
/// neighbours (1-3 across the bond), and the ligand atom's bonded neighbours
/// with the protein atom (1-3 the other way). Protein neighbours are found by
/// distance (within 1.2 × the covalent radius sum), since PDB/mmCIF poses carry no bonds in the arena.
pub fn covalent_exclusions(
    receptor: &Receptor,
    topology: &Topology,
    attachments: &[CovalentAttachment],
) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for a in attachments {
        out.push((a.ligand_atom, a.protein_atom));
        for &(nb, _) in topology.neighbours(a.ligand_atom) {
            out.push((nb, a.protein_atom));
        }
        if let Some(k) = receptor.heavy_of_source(a.protein_atom) {
            let pk = receptor.positions[k as usize];
            let rk = covalent_radius(receptor.elements[k as usize], BondOrder::Single);
            receptor.index.for_each_within(pk, 2.5, |n| {
                if n.index == k {
                    return;
                }
                let rn = covalent_radius(receptor.elements[n.index as usize], BondOrder::Single);
                if dist2_f32(pk, receptor.positions[n.index as usize]).sqrt() <= 1.2 * (rk + rn) {
                    out.push((a.ligand_atom, receptor.source[n.index as usize]));
                }
            });
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_far_ligand_has_no_clashes_and_fails_max_distance() {
        let r = Receptor::new(
            &[[0.0, 0.0, 0.0], [1.5, 0.0, 0.0]],
            &[Element::C, Element::C],
        );
        let d = protein_ligand_distances(
            &r,
            &[[30.0, 0.0, 0.0]],
            &[Element::C],
            &DistanceParams::default(),
            &[],
        );
        assert_eq!(d.clashes, 0);
        assert!((d.smallest_distance().unwrap() - 28.5).abs() < 1e-5);
        assert!(d.most_extreme.is_none());
        assert!(!d.within_max_distance(5.0));
    }

    #[test]
    fn volume_overlap_of_a_ligand_inside_itself_is_one() {
        let pos = [[0.0, 0.0, 0.0], [1.4, 0.0, 0.0], [2.1, 1.2, 0.0]];
        let el = [Element::C; 3];
        let r = Receptor::new(&pos, &el);
        let v = volume_overlap(&r, &pos, &el, &VolumeParams::default());
        assert!((v - 1.0).abs() < 1e-12);
        let far = pos.map(|[x, y, z]| [x + 50.0, y, z]);
        assert_eq!(volume_overlap(&r, &far, &el, &VolumeParams::default()), 0.0);
    }

    #[test]
    fn hydrogens_are_not_indexed() {
        let r = Receptor::new(&[[0.0; 3], [1.0, 0.0, 0.0]], &[Element::H, Element::C]);
        assert_eq!(r.atom_count(), 1);
        assert_eq!(r.source_index(0), 1);
    }
}
