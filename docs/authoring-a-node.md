# Authoring a node

A node is one file. Declare its ports, declare its parameters, implement `cook`, register it. Nothing else — no UI code, no scheduler code, no cache code. If adding a node requires touching any crate in the UI band, the abstraction is wrong; stop and fix it.

The code below is real: it is [`crates/nodes/tests/twelfth_node.rs`](../crates/nodes/tests/twelfth_node.rs), a complete twelfth node with its test, and that file is asserted to stay under 100 lines.

**Granularity first.** A step earns its own node when it has its own cache key, its own failure mode, or its own runtime. A step that can never re-run without its neighbour is a **parameter** on that neighbour, not a node.

## 1. The three methods

```rust
use mold_graph::{CookContext, NodeError, NodeSpec, Outputs, ParamSpec, PortType, Processor, Value};
use mold_nodes::{PoseSet, table};

/// Counts the poses of a set, optionally only the ones the gate passed.
struct PoseCount;

impl Processor for PoseCount {
    // 1. Ports: a type per input and output. The editor refuses a mismatched edge.
    // 2. Parameters: these appear in the recipe and the inspector automatically.
    fn spec(&self) -> NodeSpec {
        NodeSpec::new("pose_count")
            .input("poses", PortType::PoseSet)
            .output("count", PortType::Scalar)
            .param(ParamSpec::new("passing_only", false).doc("count only poses that passed"))
    }

    // 3. Cook: a pure function of (params, inputs). Its result is cached per item.
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let set: &PoseSet = ctx.input("poses")?;
        let rows = table::from_frame(&set.table).map_err(NodeError::msg)?;
        let n = match ctx.params.bool("passing_only")? {
            true => rows.iter().filter(|r| r.gate_pass == Some(true)).count(),
            false => rows.len(),
        };
        Ok(Outputs::new().with("count", Value::new(PortType::Scalar, n as i64)))
    }
}
```

That is the whole node. Register it and it is in the palette, in the recipe format, in the cache and under the scheduler:

```rust
let mut registry = mold_nodes::registry();
registry.processor(PoseCount).unwrap();
```

`registry.processor` errs only if the spec breaks the rules of its role (a duplicate port or parameter name, more than one per-item input, a default that does not match its declared kind). That is a programmer error, so `mold_nodes::registry` panics on it and a unit test covers every shipped spec.

## 2. What the pieces mean

### Ports

`PortType` is a contract type (`Structure`, `Sequence`, `LigandSet`, `Msa`, `PoseSet`, `Table`, `Scalar`). v0.1 connects ports only when the types are identical, and the editor refuses a bad edge at drag time, not at evaluation time. Adding a variant is a PR to `main`.

The Rust type behind each port type is a convention that `mold-nodes` owns — the table is at the top of [`crates/nodes/src/data.rs`](../crates/nodes/src/data.rs). Produce and consume those types, or a downstream node's `ctx.input::<T>()` fails with "input `poses` is not a …".

Inputs have a **mode**:

| Mode | | |
|---|---|---|
| `.input(..)` | `PerItem`, at most one per node | One item of this node per upstream item, which becomes its **parent**; attributes are inherited from it. |
| `.broadcast_input(..)` | `Broadcast`, any number | The upstream node must end up with **exactly one** item (a receptor, an MSA); that item feeds every item of this node. A dependency, not a parent. |

Every declared input must be connected before a graph can be evaluated (`EngineError::UnboundInput`). There is no optional input yet, which is why the co-fold node does not declare its `Msa` port.

### Parameters

`ParamSpec::new(name, default)` infers the kind from the default (`Bool`, `Int`, `Float`, `Str`, `List`), and `.doc(..)` is the one-line help the inspector shows. Give every parameter a default: `Registry::default_params` is what a freshly added node starts with, and a node nobody has configured must still cook.

Parameters are **part of the cache key**. That is the whole point: changing a gate threshold re-cooks that node's items and their descendants, and nothing else. Read them with the typed getters (`ctx.params.float("search_distance")?`), which accept an `Int` where a `Float` is declared so a hand-edited recipe (`threshold = 1`) still loads.

What must *not* be a parameter: anything that belongs to the environment rather than the result. A worker's **model version** is a field of its manifest (`workers/<name>/manifest.toml`), never a param — see the co-fold node.

### Cook

`cook` must be a pure function of (params, inputs, `fingerprint`). The engine caches its result under a hash of exactly those, so a cook that reads the clock, a global, or a file nobody declared will serve stale results and be right to be blamed.

- Return `Err(NodeError::msg(..))` with a message that says what to do about it.
- A long cook should poll `ctx.cancel.is_cancelled()` and return `NodeError::Cancelled`.
- `ctx.partial(outputs)` reports progress to observers; downstream items still wait for the final result.
- Add attributes with `Outputs::attr` (they flow to child items and can be partitioned on), and artifacts with `Outputs::file` (recorded by path, not copied — if the file is gone later, the entry is a miss).

## 3. The three things you may also need

### Fan-out: `generate`

By default a node makes exactly one item per parent (1:1). Override `generate` to make several — this is how Load Ligands turns one file into one item per ligand:

```rust
fn generate(&self, ctx: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
    Ok(records(ctx.params)?
        .map(|r| NewItem::work().attr("ligand_index", r.index as i64))
        .collect())
}
```

`generate` re-runs on every cook, on the scheduler thread, so keep it cheap. `generate_when` decides whether it sees the parent's outputs: `EachUpstreamGenerated` (the default, static — item counts are known before anything cooks, which is what gives progress), or `EachUpstreamCooked` / `AllUpstreamCooked` (dynamic). Prefer static: a static chain can be counted and cache-checked without cooking.

### Outside state: `fingerprint`

Anything a cook depends on that is not a parameter or an input must go into `fingerprint`, or the cache will hand back a result for a file that has since changed:

```rust
fn fingerprint(&self, params: &Params) -> Vec<u8> {
    crate::file_fingerprint(params.str("path").unwrap_or_default()) // blake3 of the file
}
```

It is called once per cook on the scheduler thread, so keep it cheap. An implementation version belongs here too, when a fix must invalidate old entries.

### A worker: `prepare` / `collect_event` / `collect_finish`

A node that runs a neural network, a force field or a GPL tool is a **worker node**: it does not implement `cook` at all (ADR-0009 §1). Instead it cooks in three phases, of which the app owns two:

```rust
fn prepare(&self, ctx: &CommandContext<'_>) -> Result<Option<CommandJob>, NodeError> {
    // Write the item's inputs into ctx.run_dir and return the worker + its JobSpec.
    // Every path in the spec is *relative* to the run directory, so the same bytes
    // travel to a local process, WSL2 or an SSH host.
}
fn collect_event(&self, ctx: &CommandContext<'_>, event: &serde_json::Value,
                 partial: &mut Outputs) -> Result<bool, NodeError> {
    // Fold one worker event into the outputs so far; `true` = show it now.
}
fn collect_finish(&self, ctx: &CommandContext<'_>, finished: &serde_json::Value,
                  partial: Outputs) -> Result<Outputs, NodeError> {
    // Turn the final event plus what you folded into the item's outputs.
}
fn needs(&self, params: &Params, attrs: &Attributes) -> Needs {
    // Slots, GPUs, memory — read it off the worker's manifest, don't invent it.
}
```

The accumulator between events is `partial` itself: `collect_event` gets no other state. The worker protocol is `mold_jobs::protocol` and [`docs/worker-protocol.md`](worker-protocol.md); `mold-graph` never parses the JSON. `mold_nodes::CoFold` is the worked example.

A node that implements **both** `cook` and `prepare` is a parity reference: cooked in Rust in-process, as a command elsewhere, and it must give identical outputs either way.

## 4. Persistence: register a codec

A node's result survives a restart only if every output port's payload has a codec. If one does not, the whole item stays in memory and `Cache::persistence` reports `NotPersisted` — observable, never silent, and the next process re-cooks it.

If your node introduces a new payload type, implement `mold_graph::Codec` for it in the crate that owns the type and register it in [`mold_nodes::codecs`](../crates/nodes/src/lib.rs):

```rust
c.register(PortType::PoseSet, PoseSetCodec)?;
```

Codecs are keyed by `(PortType, codec id)`. The id is stable forever (it is written into every cache entry); the version is bumped on any byte-layout change, which turns old entries into plain misses. `decode` must reject bad bytes with an error and **never panic or trust a length** — it can be handed bytes from another build, and `crates/nodes/src/codec.rs` shows the bounds-checked reader that makes that easy.

## 5. The test to write first

Each crate's `CLAUDE.md` lists its acceptance tests; write the test, then the node. For a node that is, at minimum:

1. **It cooks.** Build a one- or two-node graph with `Editor`, cook it with `Engine::new(registry, Arc::new(MemoryCache::new()))`, and assert the item is `Cooked` and the output is right. Do not call `Processor::cook` directly — `CookContext` is the engine's to build, and going through the engine is what proves the node is wired up.
2. **Its parameters do something.** Cook twice with different values and assert the results differ. That also proves the parameter is in the cache key.
3. **It fails well.** Feed it the input it cannot handle and assert the message says what to do.

[`crates/nodes/tests/common/mod.rs`](../crates/nodes/tests/common/mod.rs) has the plumbing: fixture paths, a temp directory, the worker search path, and a source node that hands a prepared `PoseSet` to the node under test.
