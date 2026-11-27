# mold-graph — workstream WS-04

The typed node graph, the work-item graph it generates, the scheduler and the cache. **No existing crate models typed scientific DAGs well; we build it.**
Read [docs/architecture.md](../../docs/architecture.md#execution-engine) first, then [DESIGN.md](DESIGN.md).

## Scope
- **Model**: nodes with stable `id`, `kind` string, parameter map, input bindings; typed ports (`PortType`). Same shape in memory and in the recipe.
- **Work items** (PDG/TOPs-style, decided): the node graph *generates* a work-item graph. The unit of generation, caching, dirtying and execution is the work item. Processors generate items (statically from params, or dynamically as upstream items cook); partitioners fan in.
- **Evaluation**: pull-based, concurrent, per item. A parameter change dirties that node's items and every descendant item; an item cooks as soon as its own dependencies are done, so results stream.
- **Scheduler**: decides what is ready (generation, per-item dependencies, cache) and hands items to an **executor** (seam: in-process threads now; subprocess and farm executors belong to `jobs`).
- **Cache**: each item's result keyed by a content hash of (kind + params + its dependencies' keys). Changing only the gate threshold re-cooks only gate items and their descendants.
- **Undo**: command pattern over graph edits only (node added, param changed, edge connected). Items and cached results are derived state: never undone, regenerated or read from cache.
- **Node trait**: the author's contract: declare ports, declare parameters, implement `cook` (and `generate` to fan out). Target: one file, under 100 lines, to add a node. If adding a node touches a UI crate, the abstraction is wrong.

## Contract (changes go to `main` first)
`PortType`, the node traits (`Processor`, `Partitioner`), the graph data model that `recipe` serializes and `ui-graph` renders, the `Executor` trait that `jobs` implements.

## May depend on
`mold-mol`, `serde`, a hashing crate (`blake3`), a thread pool (std threads today). **Never** GPUI, never an async runtime.

## Acceptance tests ([tests/acceptance.rs](tests/acceptance.rs))
- [x] A processor generating static items from params produces the right count before cooking.
- [x] A dynamic fan-out generates children only after the parent cooks.
- [x] Streaming: a slow item does not hold back its fast sibling's downstream items.
- [x] Wait-for-all waits for every upstream item and merges outputs (plus group-by-attribute).
- [x] Changing a downstream parameter re-cooks only the affected items (cooks counted per item); upstream items are cache hits.
- [x] Undo/redo restores the graph exactly; does not touch cached results.
- [x] Mismatched-port and cycle connections are refused.
- [x] Cache hit across process restarts from the run directory (`RunDirCache` + payload codecs; the test re-runs its own binary as the first process).
- [x] Command path ([tests/command.rs](tests/command.rs)): a fake command executor replays a worker event stream; partials arrive before the item finishes; outputs, cache entries and keys match the Rust-native cook whichever executor runs it; the in-process executor refuses a command item clearly; cancel mid-run writes no cache entry.
- [x] Id vs label ([tests/label.rs](tests/label.rs)): ids are frozen at creation; `label` is not in keys (relabelling re-cooks nothing), `SetLabel` undo round-trips, labels may repeat, old graphs load.
- [x] Failure policy ([tests/failure_policy.rs](tests/failure_policy.rs)): old graphs load with defaults; policy not in keys; `warn`/`ignore` let fan-in finish; retries; timeout (alone and retried).

## Design notes
Details in [DESIGN.md](DESIGN.md). In short:
- **Id vs label**: `NodeId` is assigned at creation and never changes; `NodeData.label` is free, optional, repeatable display text (`Edit::SetLabel`), outside cache keys.
- **Values** are a `PortType` tag plus `Arc<dyn Any + Send + Sync>`: payload types live in other crates (some not written yet) and plugins add their own; `Arc` makes fan-out and inspection free.
- **Input modes**: one `PerItem` input (parent, attribute inheritance) plus any number of `Broadcast` inputs (exactly one upstream item, e.g. the receptor).
- **Generate when**: `EachUpstreamGenerated` (static, default), `EachUpstreamCooked` and `AllUpstreamCooked` (dynamic). Houdini's "all upstream generated" is omitted on purpose.
- **Keys are Merkle-style** (a dependency contributes its key), so static item graphs are keyed and checked against the cache without cooking; dirtying is implicit in key changes.
- **Executor seam**: `Executor::submit(Job, JobHandle)`; completion through the handle from any thread, any time; dropped handle = `Lost`. Default per graph (`Graph.executor`), override per node (`NodeData.executor`); not in cache keys.
- **Command path** (ADR-0009 §1): a worker node implements `Processor::{prepare, collect_event, collect_finish}` instead of `cook`. A command executor (`mold-jobs`) calls `Job::cook_command(run_dir, &handle, run)`: prepare and collect run there, on the controller side; `run` gets the `CommandJob` (worker, run dir, `JobSpec` as `serde_json::Value`, files to stage) and an `EventSink`, and returns the final event. graph never parses the JSON. The in-process `ThreadExecutor` reports `NodeError::NoCommandExecutor` for a node without a Rust cook.
- **Needs and failure policy** (ADR-0009 §5, §7): `Processor::needs` → `Job.needs` (slots/GPUs/memory; for executors with capacity). `NodeData.{retries, timeout, on_failure}` (serde-defaulted, outside cache keys) → `Job.policy`; the engine enforces all three: retries resubmit with `Job.attempt + 1`, timeout cancels that job's own token, `warn`/`ignore` let fan-in go on without the item.
- **Run-directory cache**: `RunDirCache` (memory, then `<run-dir>/cache/<aa>/<hash>`, atomic temp+rename writes, checksummed entries; anything unreadable is a reported miss). Payloads persist through `Codec`s registered per `(PortType, codec id)` by the crate that owns the payload type; a result with an uncodable port stays in memory and `Cache::persistence` says so.
