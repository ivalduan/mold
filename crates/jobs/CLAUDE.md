# mold-jobs — workstream WS-05 (with `workers/mock-cofold`)

> **On hold.** The execution design is open; see [ADR-0009](../../adr/0009-execution-flavours.md). Don't write backend or scheduler code here until it's accepted. The protocol types in `src/protocol.rs` stay as the current draft.

## Requirement (known)
Parity between three execution flavours, like Houdini TOPs schedulers:
- **In-process**: threads in the app/CLI, for Rust-native nodes.
- **Subprocess**: workers on this machine (incl. WSL2, ADR-0005), speaking the [worker protocol](../../docs/worker-protocol.md).
- **External orchestration**: SSH box, Slurm, cloud and other farms.

Same graph, same recipe, same results whichever flavour runs a node. Default per graph, overridable per node.

## Fixed constraints
- `mold-graph` decides *what* is ready; this crate decides *where and how* it runs, behind the executor seam that `mold-graph` defines.
- Never link a GPU model or GPL tool (ADR-0008). Environments are declared in manifests, never installed or repaired by the app.
- Cancellation kills the whole process group / remote job and invalidates the cache entry.
- `workers/mock-cofold` comes before any real worker: it keeps everything testable with no GPU and no Python.

## May depend on
`mold-graph` (the executor seam), `serde`, `serde_json`, process/signal crates. **Never** GPUI.

## Acceptance tests
To be written from ADR-0009 once accepted.
