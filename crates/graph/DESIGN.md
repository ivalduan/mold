# mold-graph design

Work-item model after Houdini PDG/TOPs ([intro](https://www.sidefx.com/docs/houdini/tops/intro.html), [schedulers](https://www.sidefx.com/docs/houdini/tops/schedulers.html)). The unit of generation, caching, dirtying and execution is the **work item**, not the node.

## Two levels

| Level | Type | Who owns it | Persisted |
|---|---|---|---|
| Node graph | `Graph` (`NodeId → NodeData { kind, executor?, retries, timeout?, on_failure, params, inputs }`) | the user, via `Editor`/`History` | yes: `recipe` projects it to TOML |
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

- **Processor** (`trait Processor`): `spec()`, `generate_when()`, `generate(ctx) -> Vec<NewItem>` (default: one work item per parent, i.e. 1:1), `cook(ctx) -> Outputs`, optional `fingerprint(params)`, `needs(params, attrs)`. A worker node implements `prepare`/`collect_event`/`collect_finish` instead of `cook` (see [Command path](#command-path-adr-0009-1)).
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

`Cache` is a trait (`get`/`put` of `Arc<Outputs>` by key, plus `persistence(key)`). `MemoryCache` (this process) and `RunDirCache` (memory, then the run directory) ship. The executor is never part of a key, so switching in-process ↔ subprocess ↔ farm keeps the cache.

## Run-directory cache and payload codecs

### Codecs (`codec.rs`)

```rust
pub trait Codec: Send + Sync + 'static {
    type Payload: Any + Send + Sync;
    fn id(&self) -> &str;      // stable forever, globally unique: "mold-mol/arena"
    fn version(&self) -> u32;  // bump on any byte-layout change
    fn encode(&self, p: &Self::Payload, out: &mut Vec<u8>) -> Result<(), CodecError>;
    fn decode(&self, bytes: &[u8]) -> Result<Self::Payload, CodecError>; // never panics
}
CodecRegistry::register(PortType, codec) -> Result<&mut Self, CodecConflict>
CodecRegistry::encode(&Value) -> Result<Encoded { codec, version, bytes }, CodecError>
CodecRegistry::decode(PortType, codec_id, version, &[u8]) -> Result<Value, CodecError>
```

- **Typed trait, erased registry.** Codec authors write against their own Rust type, never `Value` or `Any`; the registry wraps them. The registry is filled by the host: the crate that owns a payload type ships its codec, `nodes` (or the app/CLI) registers it under every port type that carries it. `graph` registers nothing by default.
- **Keyed by `(PortType, codec id)`, not by `PortType` alone.** The Rust type behind a port type is a convention (`nodes` owns it), not something `PortType` enforces: a `Scalar` can be `f64` or `i64`, a `Table` a polars frame or (in tests) a `Vec<i64>`, and plugins may bring their own type to an existing port type. So encoding picks, among the codecs registered for the value's port type, the one whose payload type the value downcasts to; the entry records the codec id and version per port; decoding looks up `(port type, id)` and checks the version. One codec may serve several port types; an id is bound to one payload type everywhere; a port type has at most one codec per payload type. No change to `PortType` or `Value` was needed.
- **Shipped codecs** (types `graph` can name; none registered by default): `I64Codec`, `F64Codec` (bit-exact), `StringCodec`, `I64VecCodec`, `F64VecCodec`, and `ArenaCodec` for `mold_mol::Arena` in `mol`'s binary cache format (`mold_mol::cache`), whose codec version *is* `mol`'s `FORMAT_VERSION`, so a format bump there invalidates entries here.

### On-disk layout (`run_dir.rs`)

```text
<run-dir>/cache/<aa>/<hash>   one entry per work item; <hash> = 64 hex of the item key,
                              <aa> = its first two hex chars (256 shards)
<run-dir>/cache/tmp/          in-flight writes: <hash>.<pid>.<n>.tmp
```

Entry (format v1, little-endian): 16-byte magic `\x89MOLDITEM\r\n\x1a\n\0\0\0`, `u32` format version, the 32-byte key (must match the file name), `u64` payload length, 32-byte BLAKE3 of the payload, payload. Payload = the item's cook-added attributes, its artifact paths, and per output port: port name, port type name, codec id, codec version, codec bytes. Partition and no-work items are never stored (they are rebuilt at generation).

**Writes are atomic**: write the temp file, then `rename` over the final path (same volume). A reader sees a whole entry or none; a crashed write leaves a stray `.tmp` that is never read (and never deleted: another process may own it). No `fsync`: a torn entry after a power loss fails its checksum and is a miss. Within one process the cache is `Sync` (memory layer behind a mutex; disk I/O outside it; concurrent writers of one key write identical bytes and the last rename wins). On Windows, std opens files with `FILE_SHARE_DELETE`, so a rename over an entry that is being read succeeds.

`RunDirCache::get` = memory, then disk (a disk hit is decoded once and kept in memory, so later readers share the `Arc`). `put` = memory, then disk, synchronously on the scheduler thread (a write-behind thread is a follow-up if large payloads show up in profiles).

### Failure policy

- **Reading never panics and never trusts a length.** A missing entry is a plain miss. An unreadable one — I/O error, bad magic, other format version, truncated, checksum, wrong key, unknown port type, unknown codec, other codec version, codec refusal, a listed artifact file that no longer exists — is a miss **and** a `CacheEvent::Unreadable { key, path, error }`. The item re-cooks; its new entry replaces the bad one. No migration, ever: a cache is derived data.
- **Not persisted is observable, never silent.** If any output's payload has no codec (or the codec refuses it, an artifact path is not UTF-8, or the write fails), the whole result stays in memory only — a partial entry would be useless, since a cached item must provide every port. `Cache::persistence(key)` returns `Persistence::NotPersisted(Unpersisted::NoCodec { port, ty } | Encode | NonUtf8File | Io)`, and a `CacheEvent::NotPersisted` is queued (`RunDirCache::take_events`). The next process re-cooks that item and, through the Merkle keys, nothing else is affected: its descendants' keys don't depend on whether it was persisted.
- **Artifacts** (`Outputs::files`) are recorded by path, not copied. An entry whose artifact is gone is a miss.

### What is not persisted, and why
Partitions and no-work items (rebuilt from generation, cost nothing); results with an uncodable port (see above); `Outputs::partial` (transient progress). Not built (noted for later): eviction / size limits; locking or sharing between processes on one run directory (safe but uncoordinated today: two processes may both cook an item); a cache shared across run directories or remote; garbage collection of stray temp files.


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
                 params: Arc<Params>, attrs: Attributes, inputs: Inputs, cancel: CancelToken,
                 needs: Needs, policy: FailurePolicy, attempt: u32 }
impl JobHandle { fn started(&self); fn partial(&self, Outputs); fn finish(self, Result<Outputs, NodeError>) }
// Dropping a JobHandle unfinished reports NodeError::Lost.
```

- The scheduler loop runs on the caller's thread and waits on a channel; completions arrive from any thread, in any order, at any time. Nothing assumes the work runs on a scheduler-owned thread. No async runtime.
- `ThreadExecutor` (a fixed pool of std threads, `Job::cook_here`) is the one shipped implementation. It ignores `Job.needs` (one thread per item whatever `slots` says: deferred).
- Default per graph, override per node (ADR-0010): `Graph.executor` and `NodeData.executor` (`Option<String>`, serialized only when set). The node key wins, then the graph key, then the engine default; keys resolve through `Engine::add_executor(key, …)`. The executor is not part of any cache key. Selection policy beyond that lookup is for `jobs` to design.
- Cancel: each job gets `CookOptions.cancel.child()`: cancelled with the cook, or alone (timeout). On cancel the engine calls `Executor::cancel` for in-flight jobs and returns without waiting for them. Late reports are discarded, so a cancelled or timed-out job never writes the cache.

## Command path (ADR-0009 §1)

A worker node (co-folding, docking: Python/GPU/GPL) cooks an item in three phases. graph owns the two that run in the app; the executor owns *run*.

```rust
// Processor, all defaulted:
fn cook(&self, ctx: &CookContext) -> Result<Outputs, NodeError>        // default: Err(NoCommandExecutor)
fn prepare(&self, ctx: &CommandContext) -> Result<Option<CommandJob>, NodeError>   // default: Ok(None) = Rust-native
fn collect_event(&self, ctx: &CommandContext, event: &serde_json::Value,
                 partial: &mut Outputs) -> Result<bool /* changed */, NodeError>   // default: ignore
fn collect_finish(&self, ctx: &CommandContext, finished: &serde_json::Value,
                  partial: Outputs) -> Result<Outputs, NodeError>                  // default: error
fn needs(&self, params: &Params, attrs: &Attributes) -> Needs                      // default: 1 slot

pub struct CommandContext<'a> { item, params, attrs, inputs, run_dir: &Path, cancel }
pub struct CommandJob { worker: String, run_dir: PathBuf, spec: serde_json::Value, stage: Vec<PathBuf> }

// Job, for executors:
fn prepare(&self, run_dir: &Path) -> Result<Option<CommandJob>, NodeError>  // creates run_dir
fn run_command(&self, cmd: &CommandJob, handle: &JobHandle, run: R) -> Result<Outputs, NodeError>
fn cook_command(&self, run_dir: &Path, handle: &JobHandle, run: R) -> Result<Outputs, NodeError>
//   R: FnOnce(&CommandJob, &mut EventSink) -> Result<serde_json::Value /* final event */, NodeError>
impl EventSink { fn event(&mut self, &serde_json::Value) -> Result<(), NodeError>; fn cancel(&self) -> &CancelToken }
```

- **How `mold-jobs` plugs in**: it implements `Executor`. In `submit` it queues the job until its `needs` fit; then, on its own thread: `handle.started()`, pick a fresh run dir from (`job.key`, `job.attempt`), `job.cook_command(run_dir, &handle, |cmd, events| { stage, launch cmd.worker with cmd.spec, events.event(&line)? per worker event, kill on events.cancel(), return the final event or the failure })`, `handle.finish(r)`. `Executor::cancel(id)` kills the process group. graph never parses the JSON: what a "final" or "failed" event is belongs to the runner.
- **Rust-native items on a command executor**: `prepare` returns `None` and `cook_command` cooks in place (`cook_here`), so the executor needs no second path; it may report the ignored override (ADR-0009 §2).
- **Worker nodes in-process**: `ThreadExecutor` only calls `cook`; a worker node has none, so its items fail with `NodeError::NoCommandExecutor` ("no executor for command jobs"). In-process never calls `prepare` (it has no run directory, and prepare writes files).
- **A node implementing both** `cook` and `prepare` is a parity reference: cooked in Rust in-process, as a command elsewhere; it must give identical outputs. The key is identical either way: the executor, the policy and the needs are never in a key.
- **Partials**: `collect_event` folds into one accumulator per attempt; when it returns `true`, a clone goes to `JobHandle::partial` (the existing partial path), so poses stream to observers before the item finishes. `collect_finish` receives the accumulator.
- **Errors**: a `collect_event` error is returned to the runner (stop the worker) and fails the job even if the runner carries on. A cancel seen during or after the run gives `Cancelled`. Prepare/collect panics become `Panicked`.

## Needs and failure policy (ADR-0009 §5, §7)

- **Needs** (`Processor::needs` → `Job.needs`): `Needs { slots: 1, gpus: 0, mem_gb: None }` by default. Data for executors with capacity (`mold-jobs`); graph does no accounting.
- **Policy** on the node: `NodeData.{retries: u32, timeout: Option<f64> /* seconds */, on_failure: error|warn|ignore}`, each serde-defaulted and skipped when default, so old graphs load unchanged and untouched recipes don't change. Not in any cache key (like `executor`). `Graph::validate` refuses a timeout that is not finite and positive (`GraphError::InvalidTimeout`). `NodeData::failure_policy()` → `FailurePolicy { retries, timeout: Option<Duration>, on_failure }` → `Job.policy`.
- **Enforced by the engine**, so every executor gets it for free:
  - *retries*: a failed attempt (anything but `Cancelled`, including `Lost` and `TimedOut`) is resubmitted as a new job with `attempt + 1`, until `retries` run out. A retry must use a fresh run directory (the executor's job: name it after `attempt`).
  - *timeout*: counted from the executor's `started` report, per attempt. On expiry the engine cancels that job's token, calls `Executor::cancel`, and the attempt fails with `NodeError::TimedOut`; its late report is discarded. An in-process cook stops only if it polls `ctx.cancel`.
  - *on_failure*: `error` = today's behaviour (descendants cancelled; a partition with a failed member is cancelled). `warn`/`ignore` = the item is still `Failed` and its own descendants cancelled, but fan-in goes on without it: a partition drops members that failed this way, or were cancelled only because of such failures, and cooks with the rest. Independent items finish under every policy. graph treats `warn` and `ignore` alike; the UI reads `NodeData.on_failure` to show a warning or not.
- Not built: undoable edits of executor/policy fields (no `Edit` variant yet; recipes set them); slot accounting in `ThreadExecutor`.

## Deferred (follow-ups)

Subprocess/external executors (`mold-jobs`, behind the command path above); services/persistent worker pools; batch items; mappers; `@attr` parameter expressions; partitions that close before all upstream items finish; cache eviction, cross-process locking and remote caches; incremental re-cook of a live item graph (today each cook regenerates, O(items) per event); `Edit` variants for `executor` and the failure policy; honouring `Needs.slots` in `ThreadExecutor`.
