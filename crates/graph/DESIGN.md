# mold-graph design

Work-item model after Houdini PDG/TOPs ([intro](https://www.sidefx.com/docs/houdini/tops/intro.html), [schedulers](https://www.sidefx.com/docs/houdini/tops/schedulers.html)). The unit of generation, caching, dirtying and execution is the **work item**, not the node.

## Two levels

| Level | Type | Who owns it | Persisted |
|---|---|---|---|
| Node graph | `Graph` (`NodeId → NodeData { kind, executor?, params, inputs }`) | the user, via `Editor`/`History` | yes: `recipe` projects it to TOML |
| Work-item graph | `ItemGraph` (`WorkItem`s) | the `Engine`, regenerated per cook | no: derived; results live in the `Cache` |

Edges live on the consumer (`inputs: port → OutputRef { node, port }`), so an input has at most one source. Serialized shape:

```toml
[nodes.gate]
kind = "pose_gate"
params = { max_clashes = 2 }
inputs = { poses = { node = "cofold", port = "poses" } }
```

Edits (`Edit::{AddNode, RemoveNode, SetParam, Connect, Disconnect, Batch}`) are validated against the `Registry`: unknown kind/param/port, wrong param kind, `PortType::accepts` mismatch and cycles are refused; a refused edit leaves the graph untouched. `Graph::apply` returns the inverse edit; `History` keeps (forward, inverse) pairs. Undo never touches items or the cache: going back to an earlier graph makes its items' keys match again, so they come back as cache hits.

## Node roles

- **Processor** (`trait Processor`): `spec()`, `generate_when()`, `generate(ctx) -> Vec<NewItem>` (default: one work item per parent, i.e. 1:1), `cook(ctx) -> Outputs`, optional `fingerprint(params)`.
- **Partitioner** (`trait Partitioner`): `spec()` (one per-item input, one output, same type) and `partition(params, items) -> Vec<Partition>`. It runs once every upstream item is finished; each partition becomes one item that does no work. Built-ins: `WaitForAll`, `PartitionByAttribute` (param `attribute`). They are registered per `PortType` under a kind name the caller picks, because `PortType` has no "any" type.

Inputs have a mode:
- `PerItem` (at most one per node): one generation call per upstream item, which becomes the parent. Attributes are inherited from it.
- `Broadcast`: the upstream node must end up with exactly one item (receptor, MSA); that item feeds every item of this node. A dependency, not a parent.

A `NewItem` either does work (`NewItem::work()`, cooked later) or carries its outputs already (`NewItem::done(outputs)`: an attribute/no-work item, cooked at generation).

## Generation timing (`GenerateWhen`)

| Mode | Runs `generate` for a parent... | Sees parent outputs |
|---|---|---|
| `EachUpstreamGenerated` (default, static) | as soon as the parent exists | no |
| `EachUpstreamCooked` (dynamic) | as soon as the parent finished cooking | yes |
| `AllUpstreamCooked` (dynamic) | once every upstream item finished | yes |

Static chains are fully known before any cook (`Engine::generate`), which gives progress counts. Behind a dynamic node, static items appear as their parents do. Houdini's "all upstream generated" is deliberately absent: `generate` is per parent, so waiting for all parents would only add latency. A node with no per-item input generates once (after its broadcast inputs are generated, or cooked for dynamic modes).

Item ids are `node[index path]`: the parent's path plus the child's position (`cofold[3.1]` = second pose of ligand 3). They do not depend on cook order.

## Item lifecycle

```
generated ──► Cached (key in cache)                       (work items)
          ──► Uncooked (Engine::generate only)
          ──► Waiting ──► Scheduled ──► Cooking ──► Cooked
                     │                         └──► Failed / Cancelled
                     └──► Cancelled (a dependency failed, or the cook was cancelled)
no-work and partition items: Cooked at generation (a partition with a failed member: Cancelled)
```

An item is submitted as soon as **its own** dependencies (parent + broadcast items) are done, not when its upstream node is. That is what lets poses stream: `cofold[0.*]` can be gated while `cofold[1.*]` is still running. A failure cancels only that item's descendants.

## Cache keys and dirtying

```
item key      = BLAKE3("mold-graph/item/v1", kind, resolved params, fingerprint,
                       child index, generated attributes, (input port, dependency key)*)
partition key = BLAKE3("mold-graph/partition/v1", kind, resolved params, partition index,
                       partition attributes, member keys*)
```

Keys are Merkle-style: a dependency contributes its key, not a hash of its payload, because its result is a pure function of its key. So the whole static item graph is keyed without cooking. Resolved params = declared defaults overlaid by explicit values (unknown names dropped). The node id is not in the key: renaming a node keeps its cache.

Dirtying is implicit: a param change changes the key of every item of that node and, through the Merkle chain, of every descendant item; nothing upstream changes. Each cook regenerates the item graph (cheap) and looks every work item up in the cache at generation. `Engine::generate` therefore reports exactly which items are dirty (`Uncooked`) versus `Cached`, without cooking. Generation that depends on a param re-runs because generation always re-runs.

`Cache` is a trait (`get`/`put` of `Arc<Outputs>` by key). `MemoryCache` ships. A run-directory store needs a payload codec per `PortType` (payloads are type-erased); that is a follow-up.

## Port values

`Value` = `PortType` tag + `Arc<dyn Any + Send + Sync>`. Type erasure because the payload types (mol arena, polars tables, chem ligand sets) live in other crates, some not written yet, and plugins bring their own. Producers and consumers agree on the Rust type per `PortType` (a convention `nodes` owns). `Arc` makes fan-out and "every intermediate is visible" free. `Outputs` = values per port + attributes added by the cook + files written (artifacts).

## Executor seam

The engine decides what is ready; an executor decides where it runs.

```rust
pub trait Executor: Send + Sync + fmt::Debug {
    fn submit(&self, job: Job, handle: JobHandle); // must not block on the work
    fn cancel(&self, _job: JobId) {}               // best effort; job.cancel is already set
}
pub struct Job { id, item: ItemId, key: ContentHash, kind, processor: Arc<dyn Processor>,
                 params: Arc<Params>, attrs: Attributes, inputs: Inputs, cancel: CancelToken }
impl JobHandle { fn started(&self); fn partial(&self, Outputs); fn finish(self, Result<Outputs, NodeError>) }
// Dropping a JobHandle unfinished reports NodeError::Lost.
```

- The scheduler loop runs on the caller's thread and waits on a channel; completions arrive from any thread, in any order, at any time. Nothing assumes the work runs on a scheduler-owned thread. No async runtime.
- `ThreadExecutor` (a fixed pool of std threads, `Job::cook_here`) is the one shipped implementation.
- Per-node override: `NodeData.executor: Option<String>` (serialized only when set) is looked up in `Engine::add_executor(key, …)`; `None` uses the engine default. Selection policy beyond that lookup is for `jobs` to design.
- Cancel: `CookOptions.cancel` is shared with every job; on cancel the engine calls `Executor::cancel` for in-flight jobs and returns without waiting for them. Late reports are discarded.

## Deferred (follow-ups)

Run-directory (persistent) cache and payload codecs; subprocess/external executors; services/persistent worker pools; batch items; mappers; `@attr` parameter expressions; partitions that close before all upstream items finish; cache eviction; incremental re-cook of a live item graph (today each cook regenerates, O(items) per event).
