# 0011. Loaders as job items, and parallel data transforms

- Status: **Proposed (deferred, revisit after v0.1 core lands)**
- Date: 2026-11-24
- Related: ADR-0009 (execution flavours), ADR-0010 (work-item graph)

## Context
The first `mol` PR (#2) put the PDB/mmCIF importers and the Polars pose table inside `mold-mol`. Importers and data engines are naturally parallel, and they bring their own concurrency:
- `pdbtbx` and Polars (`rayon`) use internal thread pools.
- `polars-core` 0.54+ always links `tokio` (rt-multi-thread). PR #2 holds Polars at 0.53 to keep the current "domain core: no async runtime" rule.

The current design treats `mol` as a passive data crate and assumes the graph's executor owns all concurrency. That stops holding once loaders and transforms parallelise themselves.

## Direction to evaluate
- **Loaders and importers as independent crates**, outside the domain core, each wrapped as a work-item processor (ADR-0010). The core keeps the arena and the read API; I/O moves to the edge.
- **Occupancy declared by the loader.** A work item states how many slots it uses (cf. Houdini "Slots per Work Item"), and the scheduler accounts for it. A single-threaded loader takes 1 slot; a parallel one (rayon, Polars) takes N, or "all". This avoids oversubscription when the executor runs many items at once, each with its own internal pool.
- **Efficient data transforms**: Polars/Arrow-native transforms over the pose table, run as items, without copying between nodes. Ties into how `Value` payloads are passed (zero-copy `Arc`) and the per-`PortType` codecs needed for subprocess/external execution.
- **Revisit the "no async runtime" rule**: keep it for the hot interactive core (arena, geometry, render), but allow it in loader crates if their dependencies need it. That would also let Polars move past 0.53.

## Questions
1. Which crates split out: `mol-io` (PDB/mmCIF), `chem` I/O (SMILES/SDF), table I/O?
2. How a processor declares occupancy, and how in-process, subprocess and external executors honour it (ADR-0009 Q5).
3. Do we share one global thread pool (configure rayon/Polars to use it) or cap each library's pool per item?
4. Which crates may link an async runtime, and how the layering check enforces the boundary.

## Decision
_Deferred. For now, `mold-mol` keeps its importers and Polars stays below 0.54._
