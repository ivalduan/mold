# mold-nodes — workstream WS-07

The eleven v0.1 nodes. Each row is a checkable claim; write its test first.

A step earns a node when it has its own cache key, failure mode or runtime. A step that can never re-run without its neighbour is a **parameter** on that neighbour, not a node.

| Node | In → out | Built on |
|---|---|---|
| Load Structure | path → `Structure`, `Sequence` | `mol` (pdbtbx) |
| Prepare Receptor | `Structure` → `Structure` | `mol` (protonation, altlocs, waters, chains) |
| Load Ligands | path → `LigandSet` | `chem` (SMILES, SDF, CSV) |
| Prepare Ligands | `LigandSet` → `LigandSet` | `chem` (standardize, stereo, tautomers) |
| MSA Query | `Sequence` → `Msa` | MSA provider trait (below) |
| Co-fold / Dock Runner | `Structure`, `LigandSet`, `Msa` → `PoseSet` | `jobs` + a worker. The only GPU node; checkpoint per pose |
| Pose Gate | `PoseSet` → `PoseSet`, `Table` | `geom` |
| Interaction Profiler | `PoseSet`, `Structure` → `Table` | `geom` |
| Align to Reference | `PoseSet`, `Structure` → `PoseSet`, `Table` | `geom` |
| Join Metrics | `Table`, `Table` → `Table` | polars |
| Linked 3D + SAR | `PoseSet`, `Table` → terminal | output sink; the UI renders it |

## MSA Query specifics (decided, see docs/architecture.md#msa)
- MSA belongs to this node, cached on a **receptor sequence hash**, not hidden inside co-fold.
- `MsaProvider` trait from the first commit. v0.1 implementation: public ColabFold MSA server, **serial, rate-limited, never fans out**; `.a3m` cached beside the recipe.
- `.a3m` import is always available and makes no network call.
- Before sending, the node states where the sequence is going; the recipe records which provider answered.

## Co-fold specifics
Model version is a **worker manifest field** (pin open-weights Boltz-2; Boltz 2.1 is reportedly closed/API-only). Never bury the pin in code.

## Rules
- Adding a node must not touch any UI crate.
- Write `docs/authoring-a-node.md` before the fifth node, not after.

## May depend on
`mol`, `chem`, `geom`, `graph`, `jobs`, an HTTP client for MSA. **Never** GPUI.

## Acceptance tests
- [ ] The full 11-node graph runs end-to-end against `mock-cofold` and the fixture, with no GPU and no Python.
- [ ] Changing a gate threshold re-runs the gate only (not co-fold, not MSA).
- [ ] A twelfth node can be added in one file under 100 lines (prove it with a test node).
