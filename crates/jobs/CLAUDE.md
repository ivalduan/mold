# mold-jobs — workstream WS-05 (with `workers/mock-cofold`)

Where and how work items run. The design is [ADR-0009](../../adr/0009-execution-flavours.md) (accepted 2026-11-27): read it first. `mold-graph` decides *what* is ready; this crate decides *where and how* it runs, behind graph's `Executor` seam.

## Scope (v0.1)
- **Worker nodes cook in three phases**: prepare (in the app: inputs into the run dir, `JobSpec`) → **run** (this crate) → collect (in the app: each `pose` event becomes a partial output as it arrives). Only *run* differs between flavours. The graph side is on `main` (#19): `Processor::{prepare, collect_event, collect_finish, needs}`, `CommandJob`, and `Job::cook_command` with an `EventSink`. An executor here implements `mold_graph::Executor` and calls `cook_command` per job; see `crates/graph/DESIGN.md` "Command path".
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
Process tests live in [workers/mock-cofold/tests/local_executor.rs](../../workers/mock-cofold/tests/local_executor.rs) (see Design notes for why).
- [x] `mock-cofold` runs end to end through the local executor: prepare → run → collect; poses arrive as partial outputs **before** the item finishes. (`mock_cofold_streams_poses_through_the_local_executor`: the worker is held mid-stream until the observer has seen the pose.)
- [x] Parity: the same graph cooked with the in-process default and the local executor produces identical outputs and identical cache keys; the `JobSpec` and event stream are byte-identical across runs. *SSH parity: unchecked until the SSH executor exists.*
- [x] Switching a node's executor key (recipe) does not change any cache key; a cached item is not re-run.
- [x] Cancellation kills the process group (worker **and** a grandchild it spawned; PIDs asserted gone) and writes no cache entry.
- [x] Timeout and `retries` behave as declared (timeout kills the tree; each attempt gets a fresh run dir); `on_failure = "warn"` lets independent items and the fan-in finish.
- [x] Slot accounting: with 2 slots, a third item waits; a `gpus = 1` item waits for a free GPU slot. An item that can never fit fails at once.
- [x] Unknown executor key and an invalid manifest each fail before any item runs, with a clear message.
- [ ] SSH staging round trip (inputs out, artifacts back) against a local `sshd` or a loopback test double; skipped cleanly when none is available. *Next PR. Today an `ssh` profile parses and is refused before cooking ("not available yet").*

## Design notes
- **Public API.** `Executors::new(profiles, workers, run_root)` → `check(graph)` / `attach(engine, graph)` (registers one `LocalExecutor` per key the graph names; unknown or unsupported keys fail here, before any item runs), `remote_keys(graph)` (for the UI's "data leaves this machine" notice), `take_notices()`. `Profiles::{builtin, load(&ProfileSources), add_toml}`, `ProfileSources::from_env()`. `Workers::load_dir(dir).with_search_path(..)`, `Manifest::{load, parse, needs, argv}`. `protocol::{JobSpec::new(..).input(..).params(..).to_value(), JobSpec::from_value, parse_event}`. `pid_alive(pid)`, `wsl_path(..)`.
- **Run directories**: `<run root>/<executor key>/<item key hex>/<attempt>`. Fresh per (key, attempt): a directory left by an earlier session is cleared first. Two running items with the same key get `<attempt>.1`, … Kept after the run (the cache's artifacts live there; failed attempts stay for inspection). The runner writes `job.json`, `events.ndjson` and `stderr.log` into it.
- **The worker runs in its run directory**, so the `JobSpec` holds only relative paths (`run_dir = "."`). This is what makes it byte-identical across runs and flavours; WSL2 translates only the working directory (`wsl.exe [-d distro] --cd /mnt/c/… -- argv`), in `wsl.rs`.
- **Process-tree kill.** Windows: the worker is created suspended, assigned to a Job Object with `KILL_ON_JOB_CLOSE`, then resumed, so it cannot spawn outside the job; kill = `TerminateJobObject` (raw `windows-sys` FFI). Unix: `process_group(0)` + `killpg(SIGKILL)` (`libc`). A worker that calls `setsid` escapes the group; none of ours do. On Windows the kill is made synchronous: the runner waits (up to 5 s) until the job reports no active process, since a dying child still holds the run directory open. Every way out of a run kills the tree, including success (a child left behind is killed) and a child holding stdout open (killed after a 2 s grace).
- **Cancellation**: the runner polls the job's cancel token (set by graph's engine on cancel and timeout) and the executor's own flag (`Executor::cancel`) every 20 ms while waiting on stdout. A queued job that is cancelled is removed from the queue. The engine discards late reports, so nothing is cached.
- **Scheduling**: std threads, one per running job; no dispatcher thread. A job starts when `Job.needs` fit the free slots/GPUs/memory (memory only when the profile declares `mem_gb`); queue order with backfill (a later job that fits may start before a larger one: starvation is possible, acceptable for v0.1). Capacity is released *after* `handle.finish`, so the observer never sees more items cooking than slots. Needs that exceed the profile fail the item at once with a message pointing at `executors.toml`.
- **What counts as failure** is decided in `runner.rs` (table in docs/worker-protocol.md). A `collect_event` error stops the worker (graph also fails the job).
- **Profiles**: built-in `local` (slots = machine parallelism, `gpus = 0`: a GPU item fails clearly until the user declares a GPU). Site file (`$MOLD_EXECUTORS_SITE`, must exist if named) then user file (`~/.mold/executors.toml`, optional); a key in a later file replaces the whole profile. `ssh` profiles are always `remote`.
- **Manifests**: validated, never installed. `Workers::load_dir` fails on any bad manifest. A worker with `needs_gpu` run by a node that declares no GPU fails with a message pointing at `Manifest::needs`. Executables resolve at launch (search path, manifest dir, `PATH`), so a machine without Python can still load the Boltz manifest.
- **Rust-native items on this executor** cook in place (`cook_here`), one `Notice::NativeItemsCookInApp` per kind (ADR-0009 §2).
- **How tests find `mock-cofold`**: `CARGO_BIN_EXE_mock-cofold` exists only for its own package's integration tests, so the executor's process tests live in `workers/mock-cofold/tests/`, with `mold-graph` as a dev-dependency. Cargo builds the binary before running them, under plain `cargo test --workspace`, on Windows and Linux, with no env var and no nested `cargo build`. Unit tests (protocol, manifests, profiles, WSL paths, process tree) stay in this crate. The mock's scoring is a small lib (`mold_mock_cofold::scores`) so the parity test's in-process reference cook computes the same numbers.
- **Not yet**: SSH (proposal: the system `ssh` binary as a subprocess, no SSH crate; staging per ADR-0009 §3), Slurm, `events.ndjson` tailing for batch schedulers, WSL2 run on a real distro (only path translation and command line are unit-tested; whether killing `wsl.exe` kills the Linux process tree must be checked on a provisioned box).
