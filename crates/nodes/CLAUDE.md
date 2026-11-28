# mold-nodes — workstream WS-07

The eleven v0.1 nodes. Each row is a checkable claim; write its test first.

A step earns a node when it has its own cache key, failure mode or runtime. A step that can never re-run without its neighbour is a **parameter** on that neighbour, not a node.

| Node | In → out | Built on | State |
|---|---|---|---|
| Load Structure | path → `Structure`, `Sequence` | `mol` (pdbtbx) | **built** (`load_structure`) |
| Prepare Receptor | `Structure` → `Structure` | `mol` (protonation, altlocs, waters, chains) | not built |
| Load Ligands | path → `LigandSet` | `chem` (SMILES, SDF, CSV) | **built, SDF only** (`load_ligands`) |
| Prepare Ligands | `LigandSet` → `LigandSet` | `chem` (standardize, stereo, tautomers) | not built (needs `chem`) |
| MSA Query | `Sequence` → `Msa` | MSA provider trait (below) | not built (needs the trait + network) |
| Co-fold / Dock Runner | `Structure`, `LigandSet`, `Msa` → `PoseSet` | `jobs` + a worker. The only GPU node; checkpoint per pose | **built** (`cofold`), no `Msa` port yet |
| Pose Gate | `PoseSet` → `PoseSet`, `Table` | `geom` | **built** (`pose_gate`) |
| Interaction Profiler | `PoseSet`, `Structure` → `Table` | `geom` | **built** (`interaction_profiler`), no `Structure` port |
| Align to Reference | `PoseSet`, `Structure` → `PoseSet`, `Table` | `geom` | not built |
| Join Metrics | `Table`, `Table` → `Table` | polars | not built |
| Linked 3D + SAR | `PoseSet`, `Table` → terminal | output sink; the UI renders it | not built |

`mold_nodes::registry()` returns a `mold_graph::Registry` with every kind above that is built; `registry_with_workers(workers)` is the same with the co-fold node able to read `Needs` off a worker manifest. `mold_nodes::codecs()` returns the `CodecRegistry` a `RunDirCache` needs. Kind strings are in `mold_nodes::kinds`.

## Port payloads (this crate owns the convention)

| `PortType` | Rust payload | Codec |
|---|---|---|
| `Structure` | `mold_mol::Arena` (one pose per model) | `mold-mol/arena` (graph's) |
| `Sequence` | `String`, one-letter chains joined by `/` | `mold-graph/string` |
| `LigandSet` | `mold_nodes::LigandSet` | `mold-nodes/ligand-set` |
| `PoseSet` | `mold_nodes::PoseSet` | `mold-nodes/pose-set` |
| `Table` | `polars::prelude::DataFrame` | `mold-nodes/pose-table` |
| `Scalar` | `i64` or `f64` | `mold-graph/i64`, `mold-graph/f64` |
| `Msa` | — (no producer in this build) | — |

A `PoseSet` carries **the shared receptor, the ligand poses and the pose table together** (poses hold the ligand only, superimposed over one receptor: the 2026-11-27 decision, full complexes on demand). That is why the Pose Gate and the Interaction Profiler take no `Structure` input, against the table above: the receptor is already in the set, and a second input could disagree with it. `pose_id` indexes the set's own arena; a sink merging several sets renumbers.

## What counts as protein (product-owner decision, 2026-11-28)

A **node parameter**, `protein`, on the Pose Gate and the Interaction Profiler. It is a param, so it is part of the cache key: changing it re-gates, which is intended.

| `protein` | Protein is | 4WKQ clash pose |
|---|---|---|
| `polymer` (**default**) | every residue of a polymer chain, modified residues included | **65 clashes** |
| `posebusters` | ATOM records only; every HETATM residue is a cofactor | **57 clashes** |

The default is `polymer` because physically a modified residue *is* protein — 4WKQ deposits Cys797 as the S-oxidised `CSX` on HETATM records, and it is part of the chain. `posebusters` mirrors what Python PoseBusters sees, so numbers stay comparable with `crates/geom/tests/posebusters.rs` ("With CSX included the 4WKQ clash pose has 65 clashes and 0.367 overlap instead of 57 and 0.315"). Waters are never protein under either rule. A chain counts as a polymer when it has at least two non-water residues, one of them a standard amino acid or nucleotide.

Both numbers are asserted in `tests/protein_rule.rs` with `detect_covalent = false`, so `clash_count` is the raw count PoseBusters reports. With the gate's own covalent detection on (the default) that pose reports 62: the detector finds a plausible C–S pair in a pose shoved 4 Å into the protein and excludes its neighbourhood. `clash_count` is always the number the verdict used.

## MSA Query specifics (decided, see docs/architecture.md#msa)
- MSA belongs to this node, cached on a **receptor sequence hash**, not hidden inside co-fold.
- `MsaProvider` trait from the first commit. v0.1 implementation: public ColabFold MSA server, **serial, rate-limited, never fans out**; `.a3m` cached beside the recipe.
- `.a3m` import is always available and makes no network call.
- Before sending, the node states where the sequence is going; the recipe records which provider answered.

## Co-fold specifics
Model version is a **worker manifest field** (pin open-weights Boltz-2; Boltz 2.1 is reportedly closed/API-only). Never bury the pin in code. `Processor::needs` reads the same manifest.

**One ligand per item.** The protocol's `pose` event names no ligand, so a run whose SDF held several could not be attributed. Load Ligands generates one item per ligand; the node refuses a multi-ligand `LigandSet` rather than guessing.

**The mock worker returns the ligand only**, so the node builds each pose from the worker's mmCIF plus the input ligand's **bonds** (a pose file has none, and everything topological in `geom` needs them). It checks the element sequence matches and fails if the worker reordered atoms.

## Rules
- Adding a node must not touch any UI crate.
- Write `docs/authoring-a-node.md` before the fifth node, not after. **Done** — and it quotes `tests/twelfth_node.rs`, so it cannot drift.

## May depend on
`mol`, `chem`, `geom`, `graph`, `jobs`, an HTTP client for MSA. **Never** GPUI.

## Acceptance tests
- [ ] The full 11-node graph runs end-to-end against `mock-cofold` and the fixture, with no GPU and no Python. **Five of eleven nodes**: `tests/fixture_graph.rs` cooks `load_structure + load_ligands → cofold → pose_gate` on `mock-cofold` through `jobs`' local executor, with no GPU and no Python — the spine (load, fan out per ligand, run a worker, collect streamed poses, gate them). Tick this row when the remaining six nodes exist.
- [x] Changing a gate threshold re-runs the gate only (not co-fold, not MSA). (`changing_a_gate_threshold_re_runs_the_gate_only`: item states plus a count of `job.json` files under the run root, so "the worker did not run again" is observed, not inferred.)
- [x] A twelfth node can be added in one file under 100 lines (prove it with a test node). (`tests/twelfth_node.rs`: a complete `pose_count` node and its test, 82 lines, with the file's own line count asserted.)
- [x] A `RunDirCache` survives a restart: a second process reports every item cached and cooks nothing. (`a_second_process_finds_the_whole_graph_in_the_run_directory`, re-running the test binary as a real first process; an empty `take_events()` proves every payload has a codec.)
- [x] The fixture graph round-trips through a recipe against this registry, unchanged. (`the_fixture_graph_round_trips_through_a_recipe`: `to_toml` → `from_toml_with` → equal graph and equal text.)
- [x] Both protein rules are honoured and give different clash counts on 4WKQ. (`tests/protein_rule.rs`, 65 vs 57.)
- [x] Every `GateConfig` field is a parameter and reaches `geom`. (`the_gate_parameters_reach_geom`: with the ligand thresholds opened up, the mock's jittered poses pass.)

### Known, and deliberate
- **Mock poses fail the gate by default.** `mock-cofold` jitters every coordinate independently by up to 0.3 Å, which stretches bonds past the ±25% `geom` allows. `clash_count` is 0 (the pose is still in the pocket) and `gate_pass` is false. The gate is right; the mock's poses are not valid molecules. Replaying recorded Boltz-2 runs (WS-13) is what makes this row interesting.
- **`cargo test -p mold-nodes` needs `mock-cofold` built.** `CARGO_BIN_EXE_mock-cofold` is only visible to that package's own tests, so `tests/common/workers()` searches every ancestor of the test binary for the executable. `cargo test --workspace` builds it; on its own, run `cargo build -p mold-mock-cofold` first.
- `fixtures/egfr/README.md` says the receptor has 296 residues; the file has **297** residue groups (and 2,338 atom sites, 2,317 after one conformer per altloc residue is kept). The tests assert what the file contains.

## Not built, and what it needs
| | |
|---|---|
| Prepare Receptor, Align to Reference, Join Metrics, Linked 3D + SAR | nothing missing — straight `geom`/polars work |
| Prepare Ligands, SMILES in Load Ligands (`fixtures/egfr/ligands.smi`, 395 ligands) | `chem` (RDKit, ADR-0006) |
| MSA Query | the `MsaProvider` trait, an HTTP client, and an **optional input port** in `mold-graph` (today every declared input must be connected, so the `Msa` port is not declared at all) |
| `ContactConfig` as parameters on the Interaction Profiler | nothing — nine more `ParamSpec`s, left out to keep the first PR reviewable |
| `GateConfig::volume` (the overlap grid) as parameters | nothing — a numerical detail of one check, not a knob |
| Streaming partials without recopying | `collect_event` rebuilds the item's `PoseSet` per event (fine for `samples` ≈ 10, O(n²) for hundreds) |
