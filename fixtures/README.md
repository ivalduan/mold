# fixtures/

Frozen public benchmark data. Owned by workstream WS-13; see
[docs/workstreams.md#fixtures](../docs/workstreams.md#fixtures) and
[ADR-0007](../adr/0007-benchmark-target.md), which records the choice of
target, the sources, the licences and the checksums.

The benchmark target is the **EGFR kinase domain** with a 4-anilinoquinazoline
(erlotinib/gefitinib) ligand series.

```
fixtures/
  checksums.sha256     sha256 of every committed file, sha256sum format
  fetch.py             verify the committed files; fetch what is too large to commit
  structures/          parser golden files for mol (1M17, 4G5J covalent, 1EJG altlocs)
  egfr/                the benchmark target
    receptor.cif       EGFR kinase domain, protein only (PDB 4WKQ chain A, 1.85 A)
    reference.cif      the same chain with the gefitinib crystal pose, for pocket RMSD
    reference_ligand.sdf  gefitinib crystal pose with explicit bonds
    pocket.toml        the 22 pocket residues, and how they were chosen
    ligands.smi        395 ligands + 6 deliberately unparsable entries
    ligands.bad.txt    which ids must fail to parse, and why
    ligands.provenance.toml  upstream snapshot, counts, versions
  posebusters/
    reference.csv      PoseBusters full report over 18 poses: the geom reference
    inputs/            the poses and receptors that CSV refers to, + manifest.toml
  scripts/             every generation script (see below)
  cache/               downloads and generated large files; NOT committed
```

## Who reads what

| Crate | Reads | For |
|---|---|---|
| `mold-mol` | `structures/`, `cache/egfr-300poses.cif` | parser goldens; the 300-pose load bench |
| `mold-chem` | `egfr/ligands.smi`, `egfr/ligands.bad.txt` | `LigandSet` parsing, per-ligand error reporting |
| `mold-geom` | `posebusters/reference.csv` (+ `inputs/`) | cross-validating the pose gate; pocket RMSD via `egfr/pocket.toml` |
| `mold-nodes` | `egfr/receptor.cif`, `egfr/ligands.smi` | end-to-end recipes |

`cargo test` needs no Python and makes no network call: every file a test reads
is committed, except the 300-pose set, which is generated (below).

## Checking it

```sh
uv run fixtures/fetch.py                       # verify every committed file's sha256
uv run fixtures/fetch.py fetch                 # re-download the upstream snapshots into cache/
uv run fixtures/scripts/validate_ligands.py    # RDKit: 395 parse, exactly 6 fail
```

## Regenerating

Python runs through [uv](https://docs.astral.sh/uv/) only; nothing is installed
globally. Each script carries its own pinned PEP 723 dependency block, so
`uv run <script>` is all that is needed. Re-running a script rewrites its
outputs byte-for-byte (checked: identical sha256 across runs).

```sh
uv run fixtures/scripts/make_egfr_receptor.py          # egfr/receptor.cif, reference.cif, reference_ligand.sdf, pocket.toml
uv run fixtures/scripts/make_ligands.py                # egfr/ligands.smi, ligands.bad.txt, ligands.provenance.toml
uv run fixtures/scripts/make_posebusters_reference.py  # posebusters/reference.csv + inputs/
uv run fixtures/scripts/make_pose_set.py               # cache/egfr-300poses.cif (59 MB, not committed)
uv run fixtures/fetch.py update-checksums              # after any of the above
```

The upstream services (RCSB, PubChem) are live: a PDB entry can be re-versioned
and PubChem is updated continuously. The scripts pin the upstream checksums
they can (`fetch.py`, `egfr/ligands.provenance.toml`) and fail loudly when one
moves. The committed files are the frozen benchmark; upstream moving is a
decision to take, not a silent update.

## Large data

Committed total: about 2.9 MB, of which 1.8 MB is the pre-existing
`structures/`. Anything larger stays out of the repository, behind a script,
with its sha256 in `checksums.sha256`:

| Artifact | Size | How |
|---|---|---|
| `cache/egfr-300poses.cif` | 59 MB | `uv run fixtures/scripts/make_pose_set.py` |

## Licences

Everything here is public data that may be redistributed. The per-dataset
detail, with attribution, is in each directory's README and in ADR-0007.

- PDB entries (1M17, 4G5J, 1EJG, 4WKQ) and everything derived from them:
  **CC0 1.0** (wwPDB [usage policies](https://www.wwpdb.org/about/usage-policies)).
- `egfr/ligands.smi`: structures and IC50 values from **PubChem**, which
  redistributes depositor data; the EGFR assays include ChEMBL deposits, whose
  data is **CC BY-SA 3.0**. The file is therefore redistributed under
  CC BY-SA 3.0 with attribution — see `egfr/README.md`.
- `posebusters/reference.csv`: produced by us with
  [PoseBusters](https://github.com/maabuu/posebusters) (BSD-3-Clause) over CC0
  PDB data. PoseBusters is run offline as a tool; it is never linked into a
  mold binary and we redistribute none of it.

## Not here yet

- **`egfr/target.a3m`** (cached MSA) — open. An MSA for EGFR is not something
  we can copy from a licensed public file as it stands: the realistic sources
  are a ColabFold MSA-server query (a network call that sends the public EGFR
  sequence, exactly the thing the UI has to warn about) or an alignment built
  from UniProt/Pfam locally. Until that is decided, the MSA Query node has no
  cached fixture and its tests must mock. Raised in ADR-0007.
- **`mock-cofold/`** — the replay set of real poses with timings for
  `workers/mock-cofold` is deferred: the product owner has not decided what it
  should hold (real Boltz-2 output versus perturbed crystal poses, and which
  timings to replay). WS-05 does not need it to design the protocol, and
  `make_pose_set.py` already produces a realistic multi-pose file if a stand-in
  is wanted meanwhile.
