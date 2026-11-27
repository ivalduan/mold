# 0009. Execution flavours: in-process, subprocess, external

- Status: **Accepted** (2026-11-27). The four core choices are the product owner's (marked **PO** below); the rest are defaults proposed with them and open to amendment in review.
- Date: 2026-11-24 (opened), 2026-11-27 (decided)
- Workstream: WS-05 jobs (unblocked by this), WS-04 graph (executor seam, node-trait change)

## Context
Node work has to run in three flavours with **parity**: the same graph, recipe and results whichever one runs it. This mirrors Houdini TOPs/PDG schedulers ([overview](https://www.sidefx.com/docs/houdini/tops/schedulers.html), [custom schedulers](https://www.sidefx.com/docs/houdini/tops/custom_scheduler.html)):

| Houdini | Mold flavour | Typical Mold work |
|---|---|---|
| In Process Scheduler: items run inside the host process, concurrency capped by "Max Work Items" | **In-process**: threads in the app or CLI | Rust-native nodes: gate, profiler, align, join, I/O |
| Local Scheduler (default): process queue on the local machine, "Total Slots", "Slots per Work Item", "Minimum Available Memory" | **Subprocess** on this machine (incl. WSL2, ADR-0005) | Boltz-2, Vina, RDKit-heavy or GPL tools |
| HQueue, Deadline, Tractor, Royal Render, custom (Python) | **External orchestration**: SSH box, Slurm, cloud, other farms | 500-ligand co-folding campaigns |

Houdini concepts worth carrying over (not yet decided):
- **Default scheduler per graph, overridable per node** ("Default TOP Scheduler" / "Override TOP Scheduler"), plus per-node job-parameter overrides.
- **Work items**: a node's work is split into items that are scheduled individually (e.g. one per ligand), not one opaque job per node.
- **Resource accounting**: slots per item, total slots, memory gates. A GPU is a resource like any other.
- **Failure policy** per node: report error / warning / retry / ignore; max run time with a timeout action.
- **Result channel**: remote jobs push status and outputs back to the controller (Houdini: an RPC callback server, with a message-queue relay to cross firewalls; item data via a shared file or an RPC message when there's no shared filesystem).
- **Minimal scheduler contract**: Houdini's custom scheduler needs only `onSchedule(work_item)`, plus recommended `onTick` (polling and the only safe point to cancel), `onStartCook`/`onStopCook`, `onCancelWorkItems`, and notifications back (`onWorkItemStartCook`, `onWorkItemSucceeded`, `onWorkItemFailed`, `onWorkItemCanceled`, `onWorkItemAddOutput`).
- **Detached cooks**: submit the whole graph as a farm job and close the session ("Submit As Job").

Already decided elsewhere and unchanged:
- Nothing that loads a GPU model is linked into a binary; GPL tools only as subprocesses (ADR-0008). In-process is for Rust-native work.
- The graph decides **what** is ready; the executor decides **where and how** it runs. `mold-graph` ships an executor seam with one in-process implementation (WS-04).

## Open questions
1. ~~Work-item granularity~~ Settled by [ADR-0010](0010-work-item-graph.md): nodes generate work items (static or dynamic); executors run items. Still open: batching items into one job for external schedulers.
2. Is the worker protocol (docs/worker-protocol.md, JSON over stdio) the subprocess flavour's contract only, or also the external flavour's payload?
3. Result channel for external jobs: polling a shared run directory, a callback server (like PDG's RPC), or both? Firewalls and no-shared-filesystem cases.
4. Shared filesystem assumption: Houdini requires one; does Mold? This affects artifact paths and cache keys.
5. Resource model: slots, memory, GPUs. How a node declares its needs, including occupancy for items that parallelise internally (loaders, Polars transforms): see [ADR-0011](0011-loaders-and-parallel-transforms.md).
6. Per-node override: where does it live in the recipe so that switching local ↔ cluster doesn't change cache keys?
7. Detached runs: can a recipe run on a cluster with the app closed and be reattached later?
8. Which external backends to target first (SSH box, Slurm), and what v0.1 ships beyond stubs.

## Decision

### 1. Worker nodes cook in three phases: prepare → run → collect (PO)
A node is either **Rust-native** (its `cook` runs in the app, as today) or a **worker node** (co-folding, docking, anything Python/GPU/GPL). A worker node's item is split:

| Phase | Where | Does |
|---|---|---|
| **prepare** | always in the app | writes the item's inputs into its run directory, builds the `JobSpec` (worker name from its manifest + params + input paths relative to `run_dir`) |
| **run** | wherever the executor says | launches the worker, feeds it the `JobSpec`, relays its `WorkerEvent` stream; stages files if the target has no shared filesystem |
| **collect** | always in the app | turns events into outputs: each `pose` event becomes a **partial output** the moment it arrives (pose streaming is unchanged), `finished` + artifacts become the item's final outputs |

Only **run** differs between flavours, so the same graph, recipe and `JobSpec` produce the same results everywhere: parity by construction. The worker protocol (docs/worker-protocol.md) is therefore the payload of **every** flavour, not just local subprocesses (answers Q2).

**Contract change, on `main` first** (before `jobs` code): the node trait gains an optional "this item is a command" path, defaulting to "cook in Rust", so every existing processor is unaffected. Shape, to be finalised in that PR:
- `Processor` gets defaulted methods to **prepare** a command job for an item (`None` = Rust-native) and to **collect** (fold one worker event into partial outputs; finish into final outputs).
- `mold-graph` must not depend on `mold-jobs` (it's the other way round), so at the graph boundary a command job is a worker name + run dir + the `JobSpec` as `serde_json::Value`, and events arrive as `serde_json::Value`. `mold-jobs` owns parsing and validation against `protocol.rs`; graph never interprets the protocol.
- Prepare and collect run on the controller side, where the executor object lives; the executor already holds the `Job` (and so the processor).

### 2. v0.1 ships three executors: in-process, local, SSH (PO)
| Executor | Runs | Notes |
|---|---|---|
| **in-process** (`mold-graph`, exists) | Rust-native items on threads | concurrency cap = slots |
| **local** (`mold-jobs`) | command items as subprocesses on this machine | slot/GPU/memory accounting (§5); **WSL2 is the same executor with a `wsl.exe` command prefix** and path translation in one place (ADR-0005) |
| **ssh** (`mold-jobs`) | command items on a remote host | stdio is carried by the SSH connection, so the event stream is identical; staging per §3; cancellation kills the remote process group |

Rust-native items always run in the app in v0.1; an executor override on a Rust-native node is reported and ignored. Running Rust-native items remotely (e.g. `mold cook-item` on the target) is a later extension behind the same seam. **Slurm, cloud and other farms come after v0.1**, behind the same seam (answers Q8).

### 3. No shared filesystem is assumed; the run directory is staged (PO)
The executor copies the item's declared inputs into the remote run directory before **run** and copies its artifacts back after, so a standalone SSH GPU box works with no NFS/SMB. This is already possible because event paths are relative to `run_dir` (ADR-0005) and cache keys never contain paths (ADR-0010). A site whose hosts do share a filesystem may skip staging per profile (`shared_fs = "<remote path>"`), an optimisation, not a requirement (answers Q4).

### 4. Executor profiles live in user/site configuration, not the recipe (PO)
The recipe stores only executor **keys** (`Graph.executor` default, `NodeData.executor` override, as already implemented). What a key means (host, user, slots, GPUs, staging, `wsl` prefix) lives in user/site configuration (e.g. `~/.mold/executors.toml`, plus an optional site file). Consequences:
- Recipes stay portable and diffable, and carry no hostnames, paths or credentials.
- Switching local ↔ remote changes no cache key: the executor was never in the key (answers Q6).
- A profile that sends data off this machine is marked **remote**, and the UI says so before the first item is sent (product principle #4). Credentials come from the SSH agent/config, never from Mold files.
- A recipe naming a key the machine doesn't define fails before cooking, with a clear message (no silent fallback to local).

### 5. Resources: slots, GPUs, memory (default)
An item declares what it occupies (`slots`, `gpus`, `mem_gb`; default 1 slot), from its node (e.g. a manifest's `needs_gpu`, or a loader that parallelises internally declaring several slots: ADR-0011). Each profile declares capacity; the executor starts an item only when its needs fit, like Houdini's Local Scheduler (answers Q5). The in-process executor's thread cap is its slot count.

### 6. Result channel: the stdio pipe (default)
Local, WSL2 and SSH all have a live pipe, so v0.1 needs no callback server: the controller reads `WorkerEvent` lines as they come. For batch schedulers later (no live pipe), the worker wrapper appends the same lines to `run_dir/events.ndjson` and the controller tails it; a callback/relay server (PDG's RPC, for firewalls) only if a real site needs it (answers Q3).

### 7. Failure policy per node (default)
Per node, outside cache keys: `retries` (default 0), `timeout` (default none), `on_failure` = `error` (default, stops dependants) | `warn` | `ignore`. Cancellation and timeout kill the whole process group (local) or remote process group (SSH); the item's cache entry is never written. A failed item is retried as a fresh run directory.

### 8. Deferred (defaults)
- **Detached runs** (Q7): not in v0.1. The run directory holds the `JobSpec`, event log and artifacts per item, so a later session can re-attach to a remote run by reading them; no design work spent now.
- **Batching** several items into one job (Q1 remainder): later, as batch items, for farms that dislike many small jobs.
- Persistent worker pools/services (ADR-0010's deferred list), remote caches, cloud backends.

## Consequences
- `jobs` (WS-05) is **unblocked**: local + SSH executors, manifest validation, staging, slot accounting, cancellation, profile loading. First PR: local executor running `mock-cofold` end to end through prepare/run/collect, with poses streaming as partial outputs.
- A small PR to `main` changes the node trait (§1) before `jobs` and `nodes` build on it.
- `nodes` (WS-07) writes worker nodes as prepare/collect pairs and registers payload codecs (graph's run-directory cache) for every port.
- The `mock-cofold` replay set (WS-13) can now be specified: it replays the event stream defined here.
- Test of the design, from ADR-0005: the same `JobSpec` and event stream pass unchanged through local, WSL2 and SSH.
