# mold-jobs — workstream WS-05 (with `workers/mock-cofold`)

Worker supervision. Read [docs/worker-protocol.md](../../docs/worker-protocol.md) first; `src/protocol.rs` is its code form.

## Scope
- **Backend trait**: `LocalProcess` ships in v0.1. `Ssh` and `Slurm` are defined in the same trait, stubbed, unimplemented. The job spec and event stream must already fit them.
- **Supervision**: spawn, feed `JobSpec` on stdin, parse NDJSON events from stdout, surface them as a stream to `nodes`.
- **Cancellation**: kill the process group and invalidate the cache entry.
- **Manifests**: each worker ships `manifest.toml` (executable, required environment, needs GPU, model version pin). Validate and report a clear error if something is missing. **Never** install, activate or repair a Python environment.
- **WSL2** (ADR-0005): on the Windows dev box the co-fold worker runs in WSL2 and is reached through `LocalProcess` (e.g. `wsl.exe -e ...` as the executable). No second code path.

## Build the mock first
`workers/mock-cofold` is part of this workstream and comes **before** any real worker. It replays a fixture directory of real poses with realistic timing and emits the same event stream. It's what lets the graph, table and viewport be built and tested with no GPU and no Python.

## May depend on
`serde`, `serde_json`, process/signal crates. **Never** GPUI. Never link a GPU model.

## Acceptance tests
- [ ] End-to-end with `mock-cofold`: spawn, receive `started` → N × `pose` → `finished`, in order, with the right N.
- [ ] Cancellation kills the whole process group (no orphan children) on Windows and Linux.
- [ ] A malformed stdout line becomes a `log`/error, not a panic.
- [ ] Missing executable in a manifest gives a clear, actionable error.
