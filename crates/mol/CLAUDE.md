# mold-mol — workstream WS-01

Domain core. The coordinate arena, PDB/mmCIF I/O, the pose-table schema and the `Selection` value.
Read [docs/architecture.md](../../docs/architecture.md#core-data-model) first.

## Scope
- **Arena**: struct-of-arrays atoms. `positions: Vec<[f32; 3]>` contiguous and GPU-uploadable as-is; element, residue index, chain index, B-factor as parallel arrays. `PoseId` indexes into it. Coordinates live here and nowhere else.
- **I/O**: wrap `pdbtbx` for PDB and mmCIF read/write into the arena. Handle alternate locations and covalent ligands.
- **Pose table**: one Polars `DataFrame`, one row per pose, columns in `pose_columns`. Never holds coordinates.
- **Selection**: the value type only. The broadcast bus is in the UI layer.

## Contract (changes go to `main` first, in their own PR)
`PoseId`, `Selection`, `pose_columns`, and the arena's public read API (other crates read positions and per-atom arrays).

## May depend on
std, `pdbtbx`, `polars`, `arrow`. **Never** GPUI, wgpu or an async runtime.

## Acceptance tests (write these first)
- [ ] Golden-file parser tests on real structures in `fixtures/`, including one with altlocs and one with a covalent ligand.
- [ ] Round-trip: read mmCIF → arena → write → read gives the same atoms.
- [ ] Arena positions are one contiguous slice (`&[[f32; 3]]`) with no per-pose copy.
- [ ] Loading and displaying a 300-ligand result set from cache takes under 2 s (budget shared with `ui-panels`; measure the `mol` part with Criterion).
