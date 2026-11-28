# Recipe format (`format = 1`)

A recipe is a Mold graph as TOML: what the canvas shows, in a form that reads well, diffs line by line in Git and replays headless (product principle #2). It is a projection of `mold_graph::Graph`, plus two optional sections that never affect a result: canvas layout and provenance.

Code: `mold_recipe::{to_toml, from_toml, from_toml_with}`. Fixture: [`tests/fixtures/egfr_cofold.toml`](tests/fixtures/egfr_cofold.toml) (hand-written, with comments).

## The model: id, kind, label

Every node is one `[node.<id>]` table:
- **The key is the node's id**: fixed when the node is created and never changed afterwards. The canvas makes ids as `<kind>-<n>` (`cofold-1`, `pose_gate-2`; `Graph::fresh_id`); a hand-written recipe may use any id that is unique in the file. Connections, layout and provenance refer to nodes by id.
- **`kind` is the node's type** (`cofold`, `pose_gate`): which registered implementation runs, with which ports and params.
- **`label` is its display name**: free text, optional, editable at any time, and it may repeat.

Several nodes of one kind are normal: each has its own params, executor, failure policy and wiring. Nothing about a node's identity comes from its kind or its label.

## At a glance

Two co-fold runs of one kind compare models, and each feeds its own gate:

```toml
[recipe]
format = 1                    # required; this build reads 1
executor = "local"            # optional: default executor key for every node

[node.cofold-1]               # the key is the node's id
kind = "cofold"               # its type
label = "Boltz-2 with MSA"    # its display name (optional, may repeat)
inputs = { ligands = "prepare_ligands-1.ligands", msa = "msa_query-1.msa", receptor = "load_structure-1.structure" }

[node.cofold-1.params]        # only explicitly set params
model = "boltz-2"
samples = 5

[node.cofold-2]               # same kind, different configuration and wiring
kind = "cofold"
label = "Boltz-1, single sequence"
executor = "gpu-box"          # overrides [recipe] executor for this node
timeout = 3600.0              # failure policy: seconds per attempt
on_failure = "warn"
inputs = { ligands = "prepare_ligands-1.ligands", receptor = "load_structure-1.structure" }

[node.cofold-2.params]
model = "boltz-1"
samples = 10

[node.pose_gate-1]
kind = "pose_gate"
label = "Pose gate"
inputs = { poses = "cofold-1.poses" }

[node.pose_gate-1.params]
max_clashes = 0

[node.pose_gate-2]
kind = "pose_gate"
label = "Pose gate"           # the same label is fine; the id tells them apart
inputs = { poses = "cofold-2.poses" }

[node.pose_gate-2.params]
max_clashes = 2

[layout.positions]            # optional: canvas positions
cofold-1 = { x = 560, y = 0 }

[provenance.cofold-1]         # optional: what produced the results
worker_version = "0.1.0"
```

## Sections

Only these four top-level tables exist; anything else is an error.

### `[recipe]`: the header (required)
| Key | | |
|---|---|---|
| `format` | integer, required | Text-format version. A file with another number is refused, never guessed at. Bumped only for incompatible changes. |
| `executor` | string, optional | `Graph.executor`: the default executor **key** for every node ([ADR-0009 §4](../../adr/0009-execution-flavours.md)). A key is a name resolved through `~/.mold/executors.toml` or a site file, never a host, path or credential. Not part of any cache key. |

Any later graph-level field of `mold_graph::Graph` appears here too, under its serde name.

### `[node.<id>]`: one per node
Quote the id if it is not a bare key (`[node."ligand set 2"]`; bare keys allow letters, digits, `_` and `-`). Ids are not part of cache keys.

| Key | | |
|---|---|---|
| `kind` | string, required | The node's type, as registered in the node `Registry`. |
| `label` | string, optional | Display name. Free text, may repeat across nodes. Changing it changes that one line and nothing else: no connection, cache key or result depends on it. Absent = the UI shows the kind's name. |
| `executor` | string, optional | Per-node executor key, overriding `[recipe] executor`. |
| `retries` | integer ≥ 0, optional | Failure policy ([ADR-0009 §7](../../adr/0009-execution-flavours.md)): extra attempts after a failure. Default `0`. |
| `timeout` | number, optional | Failure policy: maximum run time of one attempt, in **seconds** (finite, > 0; `30`, `3600.0`). Default: no limit. |
| `on_failure` | `"error"` \| `"warn"` \| `"ignore"`, optional | Failure policy: `error` (default) stops everything that depends on the failed item; `warn` and `ignore` let fan-in go on without it (`warn` also flags it). |
| `inputs` | inline table, optional | Connections, see below. |
| `[node.<id>.params]` | sub-table, optional | Parameters, see below. |

The executor and failure-policy fields are not part of any cache key: changing them never invalidates a result. Fields at their default are not written. Any other field of `mold_graph::NodeData` maps here by its serde name with no recipe code.

**Inputs.** `inputs = { <input port> = "<node id>.<output port>" }`: the consumer names its upstream output, as the graph stores edges (on the consumer, so an input has at most one source). The string splits at the **last** dot, so ids may contain dots. A port name that contains a dot (or an empty name) uses the long form, `{ node = "...", port = "..." }`; both forms are accepted everywhere. Unconnected inputs are simply absent.

**Params.** One `name = value` line per explicitly set parameter. Values are booleans, integers, floats, strings or (nested) arrays of those, as in `mold_graph::ParamValue`; tables and datetimes are refused. An integer is accepted where a float is declared (`pocket_radius = 10`). Params left at their declared default are absent, so a new default in a node kind applies to old recipes.

### `[layout.positions]`: canvas positions (optional)
`<id> = { x = <number>, y = <number> }`: the world position of each node's top-left corner (`ui-graph`'s `CanvasState::positions`, `f32`). Written as integers when whole. Headless replay ignores the section, it is not part of `Graph` and therefore of no cache key, and a recipe without it loads (the canvas places unpositioned nodes by dependency depth). The camera is not stored: panning would otherwise dirty the file.

### `[provenance.<id>]`: what produced the results (optional)
Free-form records per node: `key = value` with parameter-style values. The format fixes only the shape; the writers (the MSA node, worker nodes, `jobs`) own the keys, e.g. `provider` for the MSA provider that answered (architecture: "the recipe records which provider answered"), `worker`, `worker_version`, `model_version`. Nothing fills it yet; it is stored and round-tripped as data. Not part of any cache key.

Layout and provenance may only name nodes of the recipe; entries for other ids are an error on reading and are left out on writing (the canvas keeps positions of removed nodes for undo; the file does not).

## Canonical form

`to_toml` writes one canonical text, so that re-saving an unchanged graph is byte-identical and one change is a one-line diff:
- `[recipe]` first, then nodes **sorted by id**, then `[layout.positions]`, then provenance. Not topological order: a rewiring would then move whole blocks.
- Within a node: `kind`, `label`, `executor`, `retries`, `timeout`, `on_failure` (any later `NodeData` field in its declaration order), then `inputs`; then the `params` sub-table. Params, inputs, positions and provenance keys are sorted.
- Each param, position and provenance value is one line. All connections of a node are one line.
- Strings are basic strings (`"..."`) with escapes; floats always have a `.0` or an exponent (`8.0`, `1e-7`, `inf`, `nan`); keys are bare when TOML allows.
- Empty sections are not written; a node with no params has no `params` table.

Hand-written recipes may use any order, inline or header tables, and comments; they load the same.

**Comments are not preserved** by a load/save cycle (`to_toml` regenerates the text). The canonical form exists to make saved files diff cleanly; keeping comments would need a `toml_edit` document carried alongside the graph, which can come later without a format change.

## Errors

Every refusal is a `RecipeError { line, column, span, kind, message }` pointing at the key or value at fault: TOML syntax, an unknown key (typos are caught, never ignored), a missing `kind`/`format`/`[recipe]`, an unsupported `format`, a value of the wrong shape (including `label`, `executor`, `retries`, `on_failure`), a connection to a node that does not exist, a cycle, a `timeout` that is not a finite positive number of seconds. `from_toml_with(text, &registry)` also runs `Graph::validate` and places its errors: unknown node kind, unknown param, wrong param type, unknown input or output port, mismatched port types.

```text
line 51, column 98: input `cofold-1.msa` is connected to `msaa.msa`, but there is no node `msaa`
```

## Canonical example

The fixture, saved by `to_toml` (a test keeps this block in sync):

```toml
[recipe]
format = 1
executor = "local"

[node.cofold-1]
kind = "cofold"
label = "Boltz-2 with MSA"
inputs = { ligands = "prepare_ligands-1.ligands", msa = "msa_query-1.msa", receptor = "load_structure-1.structure" }

[node.cofold-1.params]
model = "boltz-2"
samples = 5
seed = 42

[node.cofold-2]
kind = "cofold"
label = "Boltz-1, single sequence"
executor = "gpu-box"
timeout = 3600.0
on_failure = "warn"
inputs = { ligands = "prepare_ligands-1.ligands", receptor = "load_structure-1.structure" }

[node.cofold-2.params]
model = "boltz-1"
samples = 10
seed = 7

[node.load_ligands-1]
kind = "load_ligands"
label = "Quinazoline series"

[node.load_ligands-1.params]
path = "fixtures/egfr/ligands.smi"

[node.load_structure-1]
kind = "load_structure"
label = "EGFR 4WKQ"

[node.load_structure-1.params]
chains = ["A"]
path = "fixtures/egfr/receptor.cif"

[node.msa_query-1]
kind = "msa_query"
label = "MSA (ColabFold)"
retries = 2
inputs = { structure = "load_structure-1.structure" }

[node.msa_query-1.params]
provider = "colabfold-public"

[node.pose_gate-1]
kind = "pose_gate"
label = "Pose gate"
inputs = { poses = "cofold-1.poses", receptor = "load_structure-1.structure" }

[node.pose_gate-1.params]
max_clashes = 0
min_contacts = 6
pocket_radius = 8.0

[node.pose_gate-2]
kind = "pose_gate"
label = "Pose gate"
inputs = { poses = "cofold-2.poses", receptor = "load_structure-1.structure" }

[node.pose_gate-2.params]
max_clashes = 2
min_contacts = 4
pocket_radius = 10.0

[node.prepare_ligands-1]
kind = "prepare_ligands"
inputs = { ligands = "load_ligands-1.ligands" }

[node.prepare_ligands-1.params]
ph = 7.4

[layout.positions]
cofold-1 = { x = 560, y = 0 }
cofold-2 = { x = 560, y = 240 }
load_ligands-1 = { x = 0, y = 220 }
load_structure-1 = { x = 0, y = 0 }
msa_query-1 = { x = 280, y = -120 }
pose_gate-1 = { x = 840, y = 0 }
pose_gate-2 = { x = 840, y = 240.5 }
prepare_ligands-1 = { x = 280, y = 220 }

[provenance.cofold-1]
model_version = "boltz-2.0.3"
worker = "mock-cofold"
worker_version = "0.1.0"

[provenance.msa_query-1]
endpoint = "https://api.colabfold.com"
provider = "colabfold-public"
```
