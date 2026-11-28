# mold-geom — workstream WS-03

Spatial queries and validity checks. **Built, not wrapped**: geometric, cheap in Rust, and the actual product differentiator.

## Scope
- KD-tree over arena positions (`kiddo`).
- Clash detection, bond-length checks, contact profiling (H-bonds, salt bridges, π-stacking), pocket RMSD vs. a reference.
- PoseBusters-style physical validity checks: the kernels behind the Pose Gate node.

## May depend on
`mold-mol`, `kiddo`. **Never** GPUI, wgpu, async, Python.

## Acceptance tests
- [x] Property tests: RMSD is invariant under rigid transform; clash counts are symmetric. (`tests/properties.rs`, proptest, 256 cases each; also KD-tree vs. brute force and rigid invariance of the internal checks.)
- [x] Cross-validation of the gate against Python PoseBusters on a public set, with the tolerance written into the test. Run Python **offline** to produce a reference CSV committed to `fixtures/`; the Rust test only reads that CSV (CI has no Python). (`tests/posebusters.rs`: all 18 rows of `fixtures/posebusters/reference.csv`, tolerances from its README; plus the covalent 4G5J tests and exact golden numbers for the translated poses.)
- [x] Contact profiling (H-bonds, salt bridges, π-stacking, cation–π) with PLIP's definitions. `tests/contacts.rs`: literature (the hinge H-bond quinazoline N1 ← Met793 NH on 4WKQ gefitinib, 1M17 erlotinib (Met769) and 4G5J afatinib; no π-stacking and no direct N3–Thr790 H-bond in any crystal pose; Asp800 salt bridge of gefitinib's morpholine and afatinib's dimethylamine), cross-check against PLIP 3.0.1 on 9 rigid poses (`fixtures/plip/reference.csv`; crystal poses match exactly), explicit vs placed hydrogens, rigid-transform invariance of the whole complex (proptest, 24 cases), counts = list lengths.
- [x] Criterion: gate 500 poses in under 200 ms. (`benches/gate.rs`, 4WKQ receptor indexed once, 500 seeded perturbations of the crystal pose: ~85 ms median with `cargo bench`. `tests/budget.rs` asserts the budget and is ignored unless built with `--release`.)

## Note
Until `mol`'s arena API lands on `main`, develop kernels against plain `&[[f32; 3]]` slices. That's what the arena will hand you anyway.

## Layout
| Module | What |
|---|---|
| `spatial` | `SpatialIndex`: immutable `kiddo` KD-tree; `nearest`, `within`, `for_each_within`, `count_within` |
| `rmsd` | `rmsd` (no superposition), `kabsch_rmsd` (Horn quaternion), `centroid`, `centroid_distance` |
| `topology` | `Topology::new(elements, &[Bond])` or `Topology::from_arena(arena, pose)` (mol's `Bond`/`BondOrder`, pose-local; `from_arena` also takes the formal charges, `with_formal_charges` otherwise): rings, aromaticity, hybridisation, reference bond lengths / 1-3 distances, non-bonded pairs. Built once per ligand |
| `ligand` | `bond_lengths`, `bond_angles`, `internal_clashes`, `aromatic_ring_flatness`, `plane_distances` |
| `receptor` | `Receptor` (heavy atoms + KD-tree, built once; `Receptor::from_arena(arena, pose)` also keeps atom names, `ResidueId`s and hydrogens, and types the receptor for contacts), `protein_ligand_distances`, `volume_overlap`, `detect_covalent_attachments`, `covalent_exclusions` |
| `gate` | `PoseGate::new(receptor, topology, GateConfig).with_reference(..)`, `evaluate(positions, covalent) -> GateReport` |
| `contacts` | `contacts(receptor, topology, positions, &ContactConfig) -> Contacts`: `hbonds`, `salt_bridges`, `pi_stacks`, `cation_pi`, `n_hbonds()`, `n_saltbridge()`. Typing: ligand from `Topology` (cached in it), receptor from residue/atom names (cached in `Receptor`) |
| `radii` | vdW (RDKit table) and covalent (Pyykkö) radii |

Test-only: `tests/common/mod.rs` loads fixture ligands through `mold_mol::sdf` and proteins through `mold_mol::io`, and has a minimal CSV reader. It is **not** public API.

## Design notes

**Inputs are plain slices.** Positions `&[[f32; 3]]`, `mold_mol::Element`s, and, where topology matters, a bond list: `mold_mol::{Bond, BondOrder}` (re-exported as `mold_geom::{Bond, BondOrder}`) with pose-local indices. `Topology::from_arena(arena, pose)` builds it from `Arena::pose_bonds` (SDF poses); PDB/mmCIF poses have no bonds in the arena. Everything that depends only on connectivity lives in `Topology` and is computed once per ligand, so gating N poses of one ligand costs N × geometry only. Internally all arithmetic is `f64`. Hydrogens are ignored by every check (PoseBusters' `ignore_hydrogens`), but kept in the graph.

**Thresholds** (`GateConfig`, what the Pose Gate node exposes):

| Field | Default | Provenance |
|---|---|---|
| `bond_length_tolerance` | 0.25 | PoseBusters `threshold_bad_bond_length` |
| `bond_angle_tolerance` | 0.25 | PoseBusters `threshold_bad_angle` (on the 1-3 distance, as PoseBusters does) |
| `internal_clash_ratio` | 0.5 | geom's own, see below |
| `aromatic_flatness_threshold` | 0.25 Å | PoseBusters `threshold_flatness` |
| `protein_clash_cutoff` | 0.75 | PoseBusters `clash_cutoff` (relative to vdW sum) |
| `protein_radius_scale` | 1.0 | PoseBusters `radius_scale` |
| `max_distance_to_protein` | 5.0 Å | PoseBusters `max_distance` |
| `search_distance` | 6.0 Å | PoseBusters `search_distance` |
| `volume_overlap_cutoff` | 0.075 | PoseBusters volume-overlap `clash_cutoff` |
| `volume` | vdW × 0.8, 0.5 Å grid, 0.25 Å layers | RDKit `ShapeTverskyIndex` defaults as PoseBusters calls it |
| `rmsd_threshold` | 2.0 Å | PoseBusters `rmsd_threshold` |
| `detect_covalent` | true | geom's own, see covalent ligands |

**Reference geometry diverges from PoseBusters by design.** PoseBusters judges bonds, angles and internal clashes against RDKit's distance-geometry bounds matrix (UFF rest lengths, triangle smoothing). geom uses its own references: bond length = sum of Pyykkö covalent radii for the bond's effective order (bonds in perceived aromatic rings count as aromatic); angle = law of cosines over those lengths and an ideal angle from hybridisation (sp 180°, sp2 120°, sp3 109.47°, 108°/90°/60° inside 5/4/3-rings), compared on the 1-3 distance with the same ±25%. Hence the verdicts are cross-validated (all match) but PoseBusters' `*_relative_*` ratios and outlier counts are not comparable and are not tested. Fixture margins: crystal bonds 0.93–1.12 of reference, crystal angles ≤ 1.12; every breakage ≥ 1.5 or ≤ 0.70.

**Internal steric clash.** Heavy-atom pairs three or more bonds apart (1-2 and 1-3 excluded, 1-4 included) closer than `0.5 × (r_vdw,a + r_vdw,b)`. PoseBusters flags a pair below 0.7 × its RDKit lower bound, and that bound is 0.7 × the vdW sum for 1-5 pairs (so ≈ 0.49 × vdW sum), larger for more distant pairs and anything triangle smoothing raised; 0.5 is the closest single number. On the fixture: crystal poses ≥ 0.81, 4G5J `stretched` (not a clash for PoseBusters, at 0.714 of its bound) 0.79, 1M17 `stretched` (a clash) 0.36, `internal-clash` poses ≤ 0.19.

**Rings and aromaticity.** Rings are the smallest cycle through each bond, deduplicated (the SSSR for drug-like, non-cage molecules). A 5- or 6-ring is aromatic if all its bonds are given as aromatic, or by Hückel 4n+2 over the Kekulé orders: 1 π electron per atom with a double bond to a ring atom, 0 for an exocyclic double bond (C=O), 2 for an N/O/S/P/Se with only single bonds, and an sp3 carbon or a triple bond disqualifies the ring. Matches RDKit on the fixture (3 aromatic rings per ligand, non-aromatic morpholine and THF).

**Ring flatness reports |distance|; PoseBusters reports a signed maximum.** PoseBusters takes `_get_distances_to_plane(X).max()` per ring: the largest *signed* distance, where the SVD normal's sign is arbitrary, so it can under-report (4WKQ: 0.0082 Å reported, 0.0157 Å true). geom reports the true max |distance|, never smaller. The test accepts the reference when it equals PoseBusters' value for some choice of normal signs, at 1e-3 Å, and requires ours ≥ it.

**Volume overlap** encodes both molecules on one grid as RDKit's shape encoder does (atom spheres of 0.8 × vdW at level 3, two 0.25 Å soft layers at levels 2 and 1, max over atoms) and returns Σmin / Σligand (Tversky α = 1, β = 0). Largest deviation on the fixture: 0.004 (tolerance 0.05).

**What counts as protein is the caller's choice.** PoseBusters splits the receptor into protein / cofactors / waters and files HETATM residues (4WKQ's modified Cys, CSX 797) as cofactors, so its `_protein` checks never see them. The cross-validation mirrors that by passing ATOM records only (with CSX: 65 clashes and 0.367 overlap instead of 57 and 0.315). The kernels take whatever atoms they are given; the Pose Gate node has to pick (physically, a modified residue is protein).

**Smallest distance has no search cutoff.** geom returns the true nearest heavy-atom pair at any distance (one KD-tree nearest query per ligand atom); PoseBusters leaves the cell empty beyond its 6 Å search radius. The test asserts ours > 6 Å exactly where the reference is empty, and overlap = 0.

**Covalent ligands (4G5J).** The raw kernel reproduces PoseBusters exactly: `protein_ligand_distances(.., exclude = &[])` finds afatinib's C–S(Cys797) bond at 1.841 Å as one clash, so `minimum_distance_to_protein` is false, as in the reference. The gate does not stop there: `evaluate(positions, covalent)` takes declared `CovalentAttachment`s, and with `detect_covalent` (default) also looks for one itself when the raw check found clashes: a plausible pair (ligand C/B/P/S/Si to protein S/O/N/Se) within 0.85–1.15 × its single-bond covalent radius sum; at most one, the closest to ideal, because a pose with several such pairs is far likelier to be clashing. For each attachment the bonded pair and the 1-3 pairs across the bond (ligand atom × protein atom's covalent neighbours found by distance, ligand atom's bonded neighbours × protein atom) are excluded and the distances are recomputed. `GateReport` keeps both: `protein_raw` (PoseBusters' numbers) and `protein` (what the verdict uses). Tested: raw false; declared → pass; detected → pass; the `clash` pose still fails with the attachment declared; non-covalent crystals detect nothing.

**RMSD is per-index, not symmetry-aware.** `rmsd`/`kabsch_rmsd` pair atom *i* with atom *i*. PoseBusters uses RDKit's symmetry-aware `CalcRMS`/`GetBestRMS`; on this fixture every row matches at 1e-3 because no perturbation swaps symmetry-equivalent atoms, but for a flipped symmetric group (phenyl, carboxylate, dimethylamine) geom's RMSD is ≥ PoseBusters'. Automorphism search over `Topology` is the fix (next).

**`gate_pass` = physical validity.** `GateReport::pass()` is every verdict except `rmsd_le_2a`; accuracy needs a crystal reference and is reported separately (`Verdicts::rmsd_le_2a: Option<bool>`).

**Contacts follow PLIP.** `contacts` implements PLIP's geometric definitions (Salentin et al., NAR 2015; Adasme et al., NAR 2021), thresholds from PLIP 3.0.1's `plip/basic/config.py` and the comparisons from `plip/structure/detection.py` (strict `<`/`>` as PLIP writes them):

| `ContactConfig` field | Default | PLIP | Test |
|---|---|---|---|
| `min_distance` | 0.5 Å | `MIN_DIST` | every interaction: distance > this |
| `hbond_max_distance` | 4.1 Å | `HBOND_DIST_MAX` | donor–acceptor **heavy-atom** distance < this |
| `hbond_min_angle` | 100° | `HBOND_DON_ANGLE_MIN` | D–H···A angle > this |
| `saltbridge_max_distance` | 5.5 Å | `SALTBRIDGE_DIST_MAX` | between charge centres |
| `pistack_max_distance` | 5.5 Å | `PISTACK_DIST_MAX` | between ring centroids |
| `pistack_max_angle_deviation` | 30° | `PISTACK_ANG_DEV` | plane angle < 30° parallel, > 60° T-shaped |
| `pistack_max_offset` | 2.0 Å | `PISTACK_OFFSET_MAX` | π-stack (smaller of the two projected offsets) and cation–π |
| `cation_pi_max_distance` | 6.0 Å | `PICATION_DIST_MAX` | charge centre to ring centroid |
| `cation_pi_amine_max_angle` | 30° | hard-coded in `pication` | ligand tertiary amine plane vs ring plane |

Also PLIP's: one H-bond per donor atom (the largest angle), no H-bond between two groups that already form a salt bridge, and no His cation–π where the same His ring stacks on the same ligand ring.

*Ligand typing* (once per `Topology`, from elements, bonds, formal charges, hybridisation and aromatic rings). Hydrogen counts = explicit H neighbours + implicit H from the default valence adjusted by formal charge (N⁺ 4, O⁻ 1) minus the bond orders (aromatic = 1.5; Kekulé input is exact, an all-aromatic pyrrole without an explicit H reads as having none). Donors: N/O with ≥ 1 H. Acceptors: Open Babel's rule as PLIP uses it — every O, and N unless sp3 with four neighbours, sp2 with three (amide, aniline, pyrrole) or positively charged; halogens never. Positive groups: quaternary N; sp3 non-aromatic N (PLIP's "tertamine": primary/secondary/tertiary amines, so a morpholine N counts); guanidine (charge centre on the C); any other atom with a positive formal charge not compensated by a negative neighbour. Negative groups: carboxylate (C with two terminal O and one C; **esters excluded**, which PLIP's element count does not do), sulfonate/sulfate (S with 3/4 O), phosphate (P with only O), other uncompensated negative charges. Rings: `Topology`'s aromatic 5-/6-rings; PLIP also accepts planar non-aromatic rings (not done). For the cation–π amine angle geom checks amines with three heavy neighbours only (PLIP also uses H as neighbours, and its loop `break`s after the first amine, which is not reproduced).

*Receptor typing* by residue and atom name (PDB/mmCIF receptors have no bonds): backbone N donates (not Pro), O/OXT accept, on any non-water residue; standard side chains by table (Arg NE/NH1/NH2, Asn ND2, Gln NE2, His ND1/NE2, Trp NE1 sp2 donors; Lys NZ, Ser OG, Thr OG1, Tyr OH sp3 donors; Asp/Glu carboxylate O, Asn OD1, Gln OE1, Ser/Thr/Tyr O, His ND1/NE2 acceptors); a modified residue's side-chain O accept, nothing donates. **His**: both ring N are donors and acceptors (tautomer unknown) and the side chain is positive, as in PLIP. **Asp/Glu are charged acceptors, not donors**; PLIP protonates them through Open Babel as neutral acids, which is the one typing difference the cross-check shows. Charged groups and rings exactly PLIP's (Arg NE/NH1/NH2, His ND1/NE2, Lys NZ positive; Asp OD1/OD2, Glu OE1/OE2 negative; Phe/Tyr, His, Trp 5- and 6-ring). Waters (HOH/WAT/DOD) are not typed. A `Receptor::new` receptor has no names and gets no contacts.

*Hydrogens.* An explicit hydrogen is used as is (ligand: bonded H atoms; receptor: each H attached to its nearest heavy atom within 1.3 Å). Crystal receptors and heavy-atom SDFs have none, so the H is placed 1.0 Å from the donor using its heavy-atom neighbours (receptor neighbours found by distance, covalent radii + 0.45 Å): two neighbours, sp2 → on the external bisector (backbone NH, Trp, His, Arg NE, aniline NH); two, sp3 → the better of the two tetrahedral positions; three, sp3 → opposite their sum; **one neighbour** (OH, NH3⁺, NH2) → the H is a rotamer on a cone (X–D–H 109.47° sp3, 120° sp2) and geom takes the point on the cone nearest the acceptor, i.e. the best rotamer (`HBond::rotatable_hydrogen`). The H-bond test is then PLIP's, unchanged: heavy-atom D···A < 4.1 Å and D–H···A > 100°. That is the angle proxy: the placed H turns PLIP's angle into a constraint on the donor's heavy-atom frame (for a fixed H the acceptor must lie within ~80° of the ideal N–H direction; an acceptor straight down a C–O axis fails even for the best hydroxyl rotamer). Why not PLIP's own placement: Open Babel puts a rotatable H at an arbitrary dihedral, so PLIP misses or invents OH/NH3⁺ H-bonds depending on it; best-rotamer is deterministic and rigid-transform invariant, at the price of being permissive for rotatable donors (amide NH2 is treated as a cone, an upper bound on its two in-plane positions). No acceptor-side angle (PLIP has none).

*PLIP cross-check* (`tests/contacts.rs`, `fixtures/plip/`): on the three crystal poses geom and PLIP report the same interactions, exactly (three hinge H-bonds; 4G5J's Lys728 NZ and Cys797 N H-bonds; the two Asp800 salt bridges); `far` poses: nothing on either side. On the `clash` poses (ligand pushed 4 Å into the protein) salt bridges and cation–π match exactly (4WKQ: Asp800, Lys745); H-bonds are reported but not asserted: PLIP's protonated Asp776/Asp831 donate in 1M17, an N···N at 102° against the 100° cut-off flips with the H placement, and PLIP returns nothing at all for 4G5J's overlapping clash pose.

**Performance.** `cargo bench -p mold-geom`: building the 4WKQ receptor index (2,317 atoms) ~64 µs; gating 500 poses ~85 ms. The bench poses mostly clash (random 2 Å shifts into a pocket), which takes the slower path (covalent detection + a second distance pass), so this is close to the worst case.

**Not yet (next PRs):** symmetry-aware RMSD; contacts: hydrophobic contacts, halogen bonds, water bridges and metal complexation (PLIP has all four), planar non-aromatic rings, amidines as cations, in-plane NH2 placement, an acceptor-side angle; PoseBusters' `non_aromatic_ring_non_flatness` and `double_bond_flatness`. Never: energies (`internal_energy`, `mol_pred_energy`, …: force-field territory, a worker's job), stereo/identity/InChI checks (`chem`).
