# fixtures/plip

The reference `mold-geom`'s contact profiler (`crates/geom/src/contacts.rs`) is
cross-checked against. Produced offline by **PLIP 3.0.1** (Open Babel 3.1 via
`openbabel-wheel` 3.1.1.23, Python 3.12) with
`../scripts/make_plip_reference.py`. CI has no Python: the Rust test
(`crates/geom/tests/contacts.rs`, `cross_checks_against_plip`) only reads
`reference.csv`.

PLIP is **GPL-2.0**. It is run here once, offline, to produce data; it is never
linked, shipped or run in CI (ADR-0008). `THIRD_PARTY.md` records it as
"offline, reference data only". The CSV is derived from CC0 PDB entries.

```
reference.csv   one row per interaction PLIP reports, 9 poses
```

## Inputs

The PoseBusters fixture set (`../posebusters/inputs/`), so both references
describe exactly the same atoms: receptors 1M17, 4WKQ and 4G5J (protein only,
no waters, no hydrogens) and the ligand poses `crystal`, `clash` (whole ligand
moved 4 Å into the protein) and `far` (moved 30 Å out). The other three
perturbations are skipped: PLIP reads the ligand from PDB records and
re-perceives bond orders from geometry, which is only trustworthy for an
undistorted ligand.

The script writes each complex as one PDB (receptor records as they are, then
the ligand as `HETATM` residue `<code> Z 900` with `CONECT` records) into the
git-ignored `../cache/plip/`, runs `PDBComplex.load_pdb` +
`characterize_complex`, and maps every reported atom back by its coordinates
(PLIP's own atom indices go through its PDB fixing and altloc handling).
`openbabel-wheel` lacks the InChI plugin, which PLIP only uses for a label; the
script answers that one format with an empty string.

## Columns

| Column | Meaning |
|---|---|
| `complex`, `pose` | which fixture pose |
| `interaction` | `hbond`, `saltbridge`, `pistack`, `pication` |
| `ligand_role` | hbond: `donor`/`acceptor`; saltbridge: the ligand group's sign, `positive`/`negative`; pistack: PLIP's type, `P` (parallel) / `T` (T-shaped); pication: `cation` (ligand cation, receptor ring) / `ring` (ligand ring, receptor cation) |
| `ligand_atoms` | 0-based atom indices in the SDF (= mol's pose-local indices), space-separated, sorted |
| `receptor_residue` | `chain:resname:resnum` |
| `receptor_atoms` | atom names, space-separated, sorted |
| `distance` | hbond: donor–acceptor heavy atoms; others: between centres, Å |
| `angle` | hbond: D–H···A; pistack: between ring planes; degrees |
| `offset` | pistack/pication offset, Å |

## What it contains

| Pose | PLIP reports |
|---|---|
| 1M17 crystal | N1 ← Met769 N (hinge H-bond) |
| 4WKQ crystal | N1 ← Met793 N (hinge); morpholine N⁺ – Asp800 salt bridge |
| 4G5J crystal | N1 ← Met793 N (hinge); acceptors ← Lys728 NZ and Cys797 N; dimethylamine N⁺ – Asp800 salt bridge |
| `far` poses | nothing |
| 1M17 clash | two acceptors ← Asp776/Asp831 (PLIP protonates Asp as a neutral acid) |
| 4WKQ clash | Thr854 H-bond, Lys745 cation–π, Asp800 salt bridge |
| 4G5J clash | nothing (the ligand overlaps the protein) |

How geom compares, and why the `clash` H-bonds are reported but not asserted,
is in the test's doc comment and in `crates/geom/CLAUDE.md`.

## Regenerating

```sh
uv run fixtures/scripts/make_plip_reference.py
uv run fixtures/fetch.py update-checksums
```

Re-running rewrites `reference.csv` byte-for-byte.
