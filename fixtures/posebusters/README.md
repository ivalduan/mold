# fixtures/posebusters

The reference `mold-geom` cross-validates its pose gate against. Produced
offline by **Python PoseBusters 0.6.5** (RDKit 2026.09.2, gemmi 0.7.5,
Python 3.12) with `../scripts/make_posebusters_reference.py`. CI has no Python:
the Rust test only reads `reference.csv`.

```
reference.csv            18 rows (3 complexes x 6 poses), 88 columns
inputs/<pdb>_protein.pdb       receptor, protein only
inputs/<pdb>_<lig>_<pose>.sdf  the ligand pose, with explicit bonds
inputs/manifest.toml           every pose, what was done to it, and its sha256
```

## The set

Three public EGFR complexes (PDB, **CC0 1.0**), chosen so that the covalent
case is covered and two of the three receptors are already in the repository:

| Entry | Ligand | Why |
|---|---|---|
| 1M17 | `AQ4` erlotinib, non-covalent, 2.60 Å | the series' chemotype |
| 4WKQ | `IRE` gefitinib, non-covalent, 1.85 Å | the benchmark receptor (`../egfr/`) |
| 4G5J | `0WN` afatinib, **covalent** to Cys797, 2.80 Å | a covalent ligand behaves differently |

The receptors keep all polymer chains and drop solvent, crystallisation
additives and every ligand, so PoseBusters sees protein only. ANISOU records
are dropped. The crystal ligands come from the RCSB model server, which gives
them with explicit bonds — `mold-mol`'s arena has no bonds, so geom needs the
SDF for anything topological.

## The poses

Six per complex: the deposited pose, then five deliberate breakages, each
aimed at one check family. The exact operation is in `manifest.toml` and in the
CSV's `perturbation` column.

| `pose` | What was done | What PoseBusters then reports as failing |
|---|---|---|
| `crystal` | nothing | nothing — except 4G5J, see below |
| `clash` | whole ligand translated 4 Å towards the protein centroid | `minimum_distance_to_protein`, `volume_overlap_with_protein`, `rmsd_le_2a` |
| `far` | whole ligand translated 30 Å away from it | `protein_ligand_maximum_distance`, `rmsd_le_2a` |
| `stretched` | first acyclic single bond stretched to 3.0 Å | `bond_lengths`, `bond_angles` (+ `internal_steric_clash`, `internal_energy` on 1M17) |
| `internal-clash` | a terminal heavy atom moved to 0.6 Å from a non-bonded atom | `internal_steric_clash`, `bond_lengths`, `bond_angles`, `internal_energy` |
| `puckered` | one aromatic ring atom pushed 0.7 Å out of plane | `aromatic_ring_flatness` |

**4G5J fails `minimum_distance_to_protein` in its crystal pose.** That is
correct, not a bug: afatinib is covalently bonded to Cys797, so a ligand atom
sits 1.84 Å from a protein atom, well inside any non-bonded clash threshold.
Our gate must not call a covalent crystal pose invalid — this row is the test
that it doesn't.

**Not every boolean column is a verdict.** `most_extreme_clash_protein` is a
*descriptor* of the most extreme contact found, and its polarity is the
opposite of a verdict's: `False` means "that contact is not a clash" (the good
case, as in 1M17's and 4WKQ's crystal poses) and `True` means "it is" (4G5J's
covalent bond). A Rust test that loops over every `True`/`False` column and
demands `True` for a valid pose will therefore reject all three crystal poses.
Compare the verdict columns listed in the table below — the `bool verdicts`
rows plus `rmsd_le_2a` and the loading/identity families — and treat
`most_extreme_*_protein` as one of "the numbers behind them".

## Columns

`pdb_id`, `ligand_comp_id`, `pose`, `ligand_file`, `protein_file`,
`true_ligand_file`, `perturbation` are ours; `ligand_file` and friends are
paths relative to this directory. Everything after them is PoseBusters'
`bust(..., full_report=True)` output, in its own order, with the header
normalised to lowercase ASCII snake_case (`rmsd_≤_2å` → `rmsd_le_2a`,
`protein-ligand_maximum_distance` → `protein_ligand_maximum_distance`).

| Family | Columns | Type |
|---|---|---|
| loading / sanity | `mol_pred_loaded`, `mol_true_loaded`, `mol_cond_loaded`, `sanitization`, `inchi_convertible`, `all_atoms_connected`, `no_radicals`, `passes_valence_checks`, `passes_kekulization` | bool |
| identity vs. the crystal ligand | `molecular_formula`, `molecular_bonds`, `double_bond_stereochemistry`, `tetrahedral_chirality`, `stereochemistry_preserved`, `inchi_*`, `hydrogens`, `net_charge`, `protons`, `stereo_*` | bool / text |
| ligand-internal geometry | `bond_lengths`, `bond_angles`, `internal_steric_clash`, `aromatic_ring_flatness`, `non_aromatic_ring_non_flatness`, `double_bond_flatness`, `internal_energy` | bool verdicts |
| the numbers behind them | `shortest_bond_relative_length`, `longest_bond_relative_length`, `number_short_outlier_bonds`, `number_long_outlier_bonds`, `most_extreme_relative_angle`, `number_outlier_angles`, `shortest_noncovalent_relative_distance`, `number_clashes`, `aromatic_ring_maximum_distance_from_plane`, `non_aromatic_ring_maximum_distance_from_plane`, `double_bond_maximum_distance_from_plane`, `number_*_checked`/`_pass` | float / int |
| protein–ligand | `protein_ligand_maximum_distance`, `minimum_distance_to_protein`, `volume_overlap_with_protein` | bool verdicts |
| the numbers behind them | `smallest_distance_protein`, `num_pairwise_clashes_protein`, `most_extreme_*_protein`, `volume_overlap_protein` | float / int / text |
| energy | `mol_pred_energy`, `ensemble_avg_energy`, `energy_ratio`, `num_h_added` | float |
| accuracy vs. the crystal pose | `rmsd_le_2a` (bool), `rmsd`, `kabsch_rmsd`, `centroid_distance` | bool / float |

`relative` lengths, angles and distances are ratios against PoseBusters'
reference values (UFF-derived bond lengths and angles, van der Waals radii),
not absolute Å. Empty cells mean the check did not run: in the `far` rows the
ligand is outside the protein search radius, so `smallest_distance_protein` and
`volume_overlap_protein` are blank while `protein_ligand_maximum_distance` is
`False`. Cofactor and water columns are dropped from the CSV entirely: the
receptors are protein-only, so those checks have no reference entities and
their values would be meaningless (in 4WKQ the modified residue CSX 797 was
even picked up as an "organic cofactor").

## Suggested tolerances for the Rust cross-validation

Run against these 18 rows; the tolerance belongs in the test, as
`crates/geom/CLAUDE.md` asks.

| Quantity | Tolerance | Why |
|---|---|---|
| `rmsd`, `kabsch_rmsd`, `centroid_distance` | 1e-3 Å | pure geometry; the translated poses are exactly 4.0 and 30.0 Å, so this also catches an RMSD that silently superposes |
| `smallest_distance_protein` | 1e-3 Å | pure geometry, same atom set |
| `aromatic_ring_maximum_distance_from_plane` | 1e-3 Å | plane fit; best-fit plane conventions could differ, hence not 1e-6 |
| `volume_overlap_protein` | 0.05 absolute | PoseBusters integrates on a grid; do not chase its exact value |
| bool verdicts (`bond_lengths`, `internal_steric_clash`, `aromatic_ring_flatness`, `minimum_distance_to_protein`, …) | exact match | this is what the Pose Gate node reports |
| `*_relative_*` ratios, `number_*_outlier*`, `number_clashes` | compare only if geom adopts PoseBusters' reference tables | otherwise the ratio denominators differ |
| `internal_energy`, `mol_pred_energy`, `ensemble_avg_energy`, `energy_ratio` | **do not cross-validate** | RDKit UFF over an ETKDG conformer ensemble; that is a force field, which per the architecture rules is not Rust's job |

## Licences

- Structures and crystal ligand poses: PDB 1M17, 4WKQ, 4G5J — **CC0 1.0**
  (wwPDB usage policies). The perturbed poses are our own derivatives of CC0
  data.
- `reference.csv` is output we generated with
  [PoseBusters](https://github.com/maabuu/posebusters) (**BSD-3-Clause**,
  Buttenschoen, Morris & Deane, *Chem. Sci.* 2024). PoseBusters is run offline
  as a tool; nothing of it is linked into a mold binary or redistributed here.
