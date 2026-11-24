# 0007. Benchmark target and ligand series

- Status: Proposed (product owner accepts)
- Date: 2026-11-24
- Deciders: product owner, WS-13 `fixtures`
- Workstream: WS-13 fixtures

## Context
Every acceptance test runs against one frozen public dataset. Choosing it late
is a common way to lose a week. It is also the public Track B showcase for
design partners, so the data has to be redistributable and the provenance has
to survive scrutiny.

What the benchmark has to serve:

- `mold-mol`: parser goldens (altlocs, a covalent ligand, a modified residue)
  and a 300-pose set for the "300 ligands from cache in under 2 s" budget.
- `mold-chem`: 200–500 ligand SMILES where a known handful must fail to parse,
  so per-ligand error reporting is testable.
- `mold-geom`: a PoseBusters reference produced by the Python implementation,
  with real failures, plus a pocket definition for pocket RMSD.
- `mold-nodes` / `recipe`: a receptor and a ligand set an end-to-end recipe can
  actually run on.

The context brief named **EGFR** as the default and KRAS as the alternative.
Nobody argued for KRAS, and EGFR wins on data: a large 4-anilinoquinazoline
chemotype with published IC50 values, many crystal structures of the kinase
domain, covalent and non-covalent ligands, and two clinical drugs (erlotinib,
gefitinib) inside the same series.

## Options
1. **EGFR kinase domain, 4-anilinoquinazoline series.** Best-populated public
   congeneric chemotype; crystal structures at 1.07–3.4 Å; covalent (afatinib)
   and non-covalent cases; the resistance mutations make a good demo story.
2. **KRAS G12C and its covalent inhibitors.** Fashionable, but the public
   congeneric series are smaller, and almost every ligand is covalent, which
   makes the pose-validity checks the hard case rather than the common one.
3. **A docking benchmark set (PDBbind / the PoseBusters set).** Broad but not
   congeneric: no single target to show a recipe on, and PDBbind's licence does
   not permit redistribution.

## Decision
**EGFR kinase domain**, receptor **PDB 4WKQ**, with a 395-member
4-anilinoquinazoline series assembled from PubChem's EGFR IC50 data. Frozen
under `fixtures/`, every file produced by a committed script, every file
checksummed.

### Structures (all PDB, CC0 1.0)

| Role | Entry | Resolution | Note |
|---|---|---|---|
| receptor, reference crystal | **4WKQ** | 1.85 Å | wild-type human EGFR kinase domain (UniProt P00533, no mutations), one chain, gefitinib (`IRE`) |
| cross-reference crystal, parser golden | **1M17** | 2.60 Å | erlotinib (`AQ4`); already committed |
| covalent case, parser golden | **4G5J** | 2.80 Å | afatinib (`0WN`) bonded to Cys797 |
| altloc stress, parser golden | **1EJG** | 0.54 Å | crambin; 17 of 46 residues with altlocs |

**Why 4WKQ as the receptor.** It is the best-resolved *wild-type* EGFR kinase
domain carrying a ligand of the series' own chemotype. Everything better
resolved is a mutant with a different chemotype (8A27 1.07 Å, 8A2D 1.11 Å:
pyrrolo-imidazoles, L858R/V948R; 5UG9 1.33 Å, 5HG8 1.42 Å:
L858R/T790M/V948R), and the obvious quinazoline alternatives are worse
resolved (1M17 2.60 Å, 2ITY 3.42 Å, 4I23 2.80 Å). 4WKQ also brings two useful
awkwardnesses for a parser: altlocs on two residues, and Cys797 deposited as
the S-oxidised residue `CSX`.

`receptor.cif` is 4WKQ chain A polymer only; `reference.cif` is the same chain
with the gefitinib pose. Solvent, crystallisation additives (MES, Na⁺) and
ANISOU records are removed; nothing else is touched. Both read cleanly with
`pdbtbx` 0.12.

**Pocket** (`fixtures/egfr/pocket.toml`): the 22 polymer residues with a heavy
atom within **5.0 Å** of a gefitinib heavy atom — the EGFR ATP site, including
Leu718, Gly719, Val726, Ala743, Lys745, Thr790, Gln791, Met793, CSX797,
Leu844, Thr854, Asp855. Author numbering, which equals UniProt numbering here.

### Ligand series
`fixtures/egfr/ligands.smi`: **395 ligands + 6 deliberately unparsable
entries**, tab-separated as `SMILES, id, pIC50, source`, sorted by id.

Query, run 2026-11-24 (`fixtures/scripts/make_ligands.py`):

1. PubChem BioAssay, all bioactivities for **UniProt P00533**
   (`/rest/pug/protein/accession/P00533/concise/CSV`, 53,569 rows); keep
   `IC50`, relation `=`, value > 0 — 8,190 distinct CIDs.
2. ∩ PubChem fast-substructure search for **4-anilinoquinazoline**
   (`c1ccc(Nc2ncnc3ccccc23)cc1`, 98,828 hits) — 1,957 candidates.
3. Single fragment; elements ⊆ {H, C, N, O, F, S, Cl, Br}; RDKit-parsable.
4. **Bemis–Murcko scaffold exactly equal to the core scaffold** — this is what
   makes the set congeneric rather than "EGFR inhibitors". 1,434 candidates
   fail this, 90 are salts or mixtures, 38 fail the element filter.
5. Activity = median of that compound's qualifying IC50 values, as pIC50
   (range 4.00–11.22, median 7.32; 140 compounds have more than one
   measurement). Ids `EGFR-0001`…`EGFR-0395` in ascending CID order.

**Why PubChem rather than the ChEMBL API.** The brief suggested ChEMBL
(CHEMBL203). ChEMBL's REST service returned HTTP 500 for every request on
2026-11-24, including `/status`. PubChem redistributes the same depositor rows
— many of these EGFR assays *are* ChEMBL deposits — so the series is built
through PubChem and the licence note credits both. If the ChEMBL API comes
back, a `make_ligands.py` variant keyed on CHEMBL203 would be a strictly nicer
provenance story; it was not worth blocking the freeze on.

**The six bad SMILES** are chemically plausible-looking quinazolines, one per
failure mode, each verified to return `None` from RDKit 2026.09.2: unclosed
ring bond, unbalanced parenthesis, five-bonded nitrogen, unknown element
symbol, non-kekulizable aromatic five-ring, and a compound name instead of
SMILES. Their ids (`EGFR-0007-BAD`, `-0063-BAD`, `-0121-BAD`, `-0200-BAD`,
`-0288-BAD`, `-0377-BAD`) interleave with the real ligands under a plain sort,
so a reader that aborts on the first failure fails early rather than at the end.

### PoseBusters reference
`fixtures/posebusters/reference.csv`: 18 rows × 88 columns from **Python
PoseBusters 0.6.5** (RDKit 2026.09.2, Python 3.12) over three EGFR complexes
(1M17, 4WKQ, 4G5J) × six poses — the crystal pose plus five deliberate
breakages (translated 4 Å into the protein, translated 30 Å away, one bond
stretched to 3.0 Å, a terminal atom moved onto a non-bonded neighbour, one
aromatic ring atom pushed 0.7 Å out of plane). So the reference contains real
failures, not only passes. The inputs it refers to are committed. The covalent
4G5J crystal pose legitimately fails `minimum_distance_to_protein`, which is
the row that stops us from calling covalent crystal poses invalid. Column
meanings and suggested tolerances: `fixtures/posebusters/README.md`.

### Metrics the benchmark reports
`pocket_rmsd` (Cα of the 22 pocket residues, after superposition on
`reference.cif`), ligand `rmsd` / `kabsch_rmsd` / `centroid_distance` against
the crystal pose, the PoseBusters-style validity verdicts (bond lengths, bond
angles, internal steric clash, ring flatness, protein–ligand minimum distance
and volume overlap), `strain_kcal` from `mold-chem`, and pIC50 as the
experimental column to correlate against.

### Licences and attribution

| Dataset | Source | Licence | Attribution |
|---|---|---|---|
| 4WKQ, 1M17, 4G5J, 1EJG and every structure file derived from them | RCSB PDB and its model server | **CC0 1.0** (wwPDB usage policies) | PDB id per file; 4WKQ: Yosaatmadja Y., Squire C.J., McKeage M., Flanagan J.U. |
| `egfr/ligands.smi` | PubChem BioAssay + Compound | **CC BY-SA 3.0** (ChEMBL deposits; PubChem itself adds no restriction) | PubChem (NIH/NLM/NCBI) and ChEMBL (EMBL-EBI), recorded in `fixtures/egfr/README.md` |
| `posebusters/reference.csv` | our own run of PoseBusters over CC0 data | the output is ours; PoseBusters is **BSD-3-Clause** | Buttenschoen, Morris & Deane, *Chem. Sci.* 2024 |

The repository is Apache-2.0; `fixtures/egfr/ligands.smi` carries CC BY-SA 3.0
as a data file, recorded next to the data. No proprietary data is involved, and
nothing but public identifiers (a UniProt accession, PDB ids, CIDs, a SMILES
query) ever leaves the machine.

### Checksums
`fixtures/checksums.sha256` holds the sha256 of every committed file under
`fixtures/` in `sha256sum` format; `uv run fixtures/fetch.py` verifies them and
`… update-checksums` regenerates the list. Upstream snapshot checksums are
pinned in `fixtures/fetch.py` and `fixtures/egfr/ligands.provenance.toml`. The
anchors:

```
8ded500f8f69f4a0ebb2421154634ee0e4d7e9a404b41427efcf534ad2f404c8  egfr/receptor.cif
553163942822c9718f50a9b001f8f048e1db2ba7ec544b125777f4e9c4e238b8  egfr/reference.cif
1caa609ab761b73b37e4f25144952d8d0318d6e8be42c2d095a2f5e8e729eaec  egfr/ligands.smi
43e13a77234a635004dc45730d14f879404c043328e56a0caaf8208eec11d133  posebusters/reference.csv
e385a21bd305666d8039f68b6acc46a922441b9d220142e8e9ce8aa6882c8952  cache/egfr-300poses.cif (generated, not committed)
b23c36532327cc3a812e02a6ea0a6731e465538df24c1132b1e102a23dbdb73e  upstream 4WKQ.cif
```

## Consequences
- The benchmark is frozen: all four consuming crates can write their
  acceptance tests now, against committed files, with no Python and no network
  in CI.
- Committed weight stays small: about 1.1 MB added, 2.9 MB under `fixtures/`
  in total. The only large artifact, the 300-pose set (59 MB), is generated by
  `fixtures/scripts/make_pose_set.py` into the uncommitted `fixtures/cache/`
  and checksummed there.
- **Measured while building it:** `pdbtbx` reads that 300-pose mmCIF
  (710,700 atoms) in ≈3 s on the dev box. The "300 ligands from cache in under
  2 s" budget is therefore not reachable by re-parsing mmCIF: `mol`'s cache has
  to be a binary format, or the parse has to be parallel. Also, pdbtbx rejects
  a multi-model file whose `_atom_site.id` runs continuously across models
  ("Atoms in Models not corresponding"), so the generator restarts atom ids
  per model.
- `fixtures/egfr/ligands.smi` makes part of `fixtures/` CC BY-SA 3.0. Keep data
  licences per directory and never fold this data into source files.
- Python tooling is pinned per script (PEP 723, `uv run`), Python 3.12
  included: gemmi 0.7.5 has no wheel for 3.15 and falls back to building from
  source.
- **Open: `fixtures/egfr/target.a3m`.** No cached MSA is committed. The two
  routes are a ColabFold MSA-server query (a network call that sends the public
  EGFR sequence — precisely the "says so in the UI first" case from the product
  principles) or building an alignment locally from UniProt/Pfam. Until that is
  decided the MSA Query node has no fixture and its tests must mock.
- **Open: `fixtures/mock-cofold/`.** Deferred by the product owner: what the
  replay set should hold (real Boltz-2 output versus perturbed crystal poses)
  and which timings to replay. WS-05 does not need it to design the protocol.
