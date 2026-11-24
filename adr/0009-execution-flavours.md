# 0009. Execution flavours: in-process, subprocess, external

- Status: **Proposed (open; design not started)**
- Date: 2026-11-24
- Workstream: WS-05 jobs (on hold until this is accepted), WS-04 graph (executor seam)

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
_Pending. Until then, `mold-jobs` keeps only the protocol types already on `main`; no backend code._
