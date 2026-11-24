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
- [x] Golden-file parser tests on real structures in `fixtures/structures/`, including one with altlocs (1EJG) and one with a covalent ligand (4G5J). `tests/golden.rs`.
- [x] Round-trip: read mmCIF → arena → write → read gives the same atoms (also PDB). `tests/roundtrip.rs`.
- [x] Arena positions are one contiguous slice (`&[[f32; 3]]`) with no per-pose copy. `tests/arena_layout.rs`.
- [ ] Loading and displaying a 300-ligand result set from cache takes under 2 s (budget shared with `ui-panels`; measure the `mol` part with Criterion). Deferred: needs the Criterion dev-dependency and a 300-pose fixture set (WS-13).

## Design notes
- **Pose layout: full copies.** A pose is one complete complex (receptor + ligand) in one contiguous atom range; poses are appended back to back. Co-folding (Boltz-2) moves the receptor too (hence `pocket_rmsd`), so "receptor once + ligand per pose" would be wrong for this product. Cost at the budget (500 poses × ~3,000 atoms = 1.5 M atoms): positions 18 MB, all per-atom arrays together 34 B/atom ≈ 51 MB. GPU: upload `positions()` once as one buffer; draw a pose with its `atoms` range as base offset/count, or upload just `pose_positions(id)` (a sub-slice, zero copy). If memory ever matters, deduplicate topology (names, elements, residue tables) per pose template; positions stay as they are.
- **Read API for `geom` / `render`**: `Arena::positions() -> &[[f32; 3]]` (also `positions_flat() -> &[f32]`), parallel `elements() -> &[Element]` (`u8` atomic number), `atom_names() -> &[AtomName]` (4 inline bytes), `residue_indices() / chain_indices() -> &[u32]`, `b_factors() / occupancies() -> &[f32]`, `formal_charges() -> &[i8]`; tables `residues()`, `chains()`, `poses()`; `pose_positions(PoseId)`, `pose_of_atom(usize)`. All indices are global (arena-wide), `u32` per atom, like `Selection::atom_mask`. `positions_mut()` edits coordinates in place; topology only grows through `begin_pose` → `PoseBuilder` → `finish` (dropping the builder rolls back).
- **Altloc policy** (`AltLocPolicy`, default `HighestOccupancy`): per residue, never per atom, keep the altloc with the highest mean occupancy (ties: first in file); `First` keeps the first altloc. Atoms without an altloc id are always kept with their file occupancy. Workaround: pdbtbx copies blank-altloc atoms into every conformer and divides their occupancy; `io::pick_conformer` detects and undoes that.
- **What a read keeps**: everything in each model (protein, ligands, waters, ions, H), one pose per model, chains by author id (so a ligand sits in its protein's chain, as in PDB files). `pdbtbx` runs with `only_atomic_coords` so SEQRES does not add atom-less residues. Covalent ligands are kept whole, but `pdbtbx` 0.12 does not parse `LINK`/`struct_conn`, so the bond itself is not stored.
- **Writing**: mmCIF through `pdbtbx`; PDB with our own fixed-column writer, because `pdbtbx::save_pdb_raw` strips leading zeros from every field (`0WN` → `WN`). Fields that don't fit PDB columns are an error ("write mmCIF").
- **Pose table**: `pose_table::SCHEMA` (name + `ColumnType`, canonical order) and `pose_table::empty() -> DataFrame`. Polars is held at 0.53 with no default features: 0.54+ links tokio unconditionally.
