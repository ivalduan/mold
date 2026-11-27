# mold-jobs — workstream WS-05 (with `workers/mock-cofold`)

Where and how work items run. The design is [ADR-0009](../../adr/0009-execution-flavours.md) (accepted 2026-11-27): read it first. `mold-graph` decides *what* is ready; this crate decides *where and how* it runs, behind graph's `Executor` seam.

## Scope (v0.1)
- **Worker nodes cook in three phases**: prepare (in the app: inputs into the run dir, `JobSpec`) → **run** (this crate) → collect (in the app: each `pose` event becomes a partial output as it arrives). Only *run* differs between flavours. Depends on the node-trait change landing on `main` first (ADR-0009 §1).
- **Executors**: `local` (subprocesses on this machine; WSL2 = the same executor with a `wsl.exe` prefix and path translation in one place) and `ssh` (remote host; stdio over the SSH connection; run directory staged in and out). The in-process executor is graph's. Slurm/cloud after v0.1.
- **Profiles**: executor keys from the recipe resolve through user/site configuration (`~/.mold/executors.toml` + optional site file), never the recipe. Remote profiles are flagged so the UI can announce that data leaves the machine. An unknown key fails before cooking.
- **Resources**: items declare `slots`/`gpus`/`mem_gb`; profiles declare capacity; an item starts only when it fits.
- **Failure policy** per node: `retries`, `timeout`, `on_failure` (`error`/`warn`/`ignore`), outside cache keys.
- **Manifests** (`workers/<name>/manifest.toml`): validated, environments declared, never installed or repaired.
- Protocol types in `src/protocol.rs` + [docs/worker-protocol.md](../../docs/worker-protocol.md) are a contract: the same `JobSpec` and event stream in every flavour.

## Fixed constraints
- Never link a GPU model or GPL tool (ADR-0008).
- Cancellation and timeout kill the whole process group (local) or remote process group (SSH); the item's cache entry is never written.
- `workers/mock-cofold` comes before any real worker: it keeps everything testable with no GPU and no Python.

## May depend on
`mold-graph` (the executor seam), `serde`, `serde_json`, process/signal crates, an SSH client crate or the system `ssh` binary (decide in the first PR; record it in THIRD_PARTY.md). **Never** GPUI, never an async runtime.

## Acceptance tests (from ADR-0009)
- [ ] `mock-cofold` runs end to end through the local executor: prepare → run → collect; poses arrive as partial outputs **before** the item finishes.
- [ ] Parity: the same graph cooked with the in-process default, the local executor and (where a host is configured) SSH produces identical outputs and identical cache keys; the `JobSpec` and event stream are byte-identical across flavours.
- [ ] Switching a node's executor key (recipe) does not change any cache key; a cached item is not re-run.
- [ ] Cancellation kills the process group (no orphan `mock-cofold` processes) and writes no cache entry.
- [ ] Timeout and `retries` behave as declared; `on_failure = "warn"` lets independent items finish.
- [ ] Slot accounting: with 2 slots, a third item waits; a `gpus = 1` item waits for a free GPU slot.
- [ ] Unknown executor key and an invalid manifest each fail before any item runs, with a clear message.
- [ ] SSH staging round trip (inputs out, artifacts back) against a local `sshd` or a loopback test double; skipped cleanly when none is available.
