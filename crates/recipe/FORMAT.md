# Recipe format (`format = 1`)

A recipe is a Mold graph as TOML: what the canvas shows, in a form that reads well, diffs line by line in Git and replays headless (product principle #2). It is a projection of `mold_graph::Graph` (one `[node.<id>]` per `NodeData`), plus two optional sections that never affect a result: canvas layout and provenance.

Code: `mold_recipe::{to_toml, from_toml, from_toml_with}`. Fixture: [`tests/fixtures/egfr_cofold.toml`](tests/fixtures/egfr_cofold.toml) (hand-written, with comments).

## At a glance

```toml
[recipe]
format = 1                    # required; this build reads 1
executor = "local"            # optional: default executor key for every node

[node.cofold]                 # one table per node; the id is the key
kind = "cofold"               # required: the node kind
executor = "gpu-box"          # optional: overrides [recipe] executor for this node
inputs = { ligands = "prep.ligands", receptor = "receptor.structure" }

[node.cofold.params]          # optional; only explicitly set params
samples = 5
model = "boltz-2"

[layout.positions]            # optional: canvas positions
cofold = { x = 560, y = 60 }

[provenance.cofold]           # optional: what produced the results
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
The table key is the node's stable id (`NodeId`); quote it if it is not a bare key (`[node."ligand set 2"]`). Renaming a node does not invalidate its cache (keys never contain ids).

| Key | | |
|---|---|---|
| `kind` | string, required | The node kind, as registered in the node `Registry`. |
| `executor` | string, optional | Per-node executor key override. |
| `inputs` | inline table, optional | Connections, see below. |
| `[node.<id>.params]` | sub-table, optional | Parameters, see below. |

Every other field of `mold_graph::NodeData` maps here by its serde name with no recipe code, so ADR-0009 §7's failure policy will read as `retries = 2`, `timeout = ...`, `on_failure = "warn"` on the node table once graph adds those fields. A field at its default is not written.

**Inputs.** `inputs = { <input port> = "<node>.<output port>" }`: the consumer names its upstream output, as the graph stores edges (on the consumer, so an input has at most one source). The string splits at the **last** dot, so node ids may contain dots. A port name that contains a dot (or an empty name) uses the long form, `{ node = "...", port = "..." }`; both forms are accepted everywhere. Unconnected inputs are simply absent.

**Params.** One `name = value` line per explicitly set parameter. Values are booleans, integers, floats, strings or (nested) arrays of those, as in `mold_graph::ParamValue`; tables and datetimes are refused. An integer is accepted where a float is declared (`pocket_radius = 10`). Params left at their declared default are absent, so a new default in a node kind applies to old recipes.

### `[layout.positions]`: canvas positions (optional)
`<id> = { x = <number>, y = <number> }`: the world position of each node's top-left corner (`ui-graph`'s `CanvasState::positions`, `f32`). Written as integers when whole. Headless replay ignores the section, it is not part of `Graph` and therefore of no cache key, and a recipe without it loads (the canvas places unpositioned nodes by dependency depth). The camera is not stored: panning would otherwise dirty the file.

### `[provenance.<id>]`: what produced the results (optional)
Free-form records per node: `key = value` with parameter-style values. The format fixes only the shape; the writers (the MSA node, worker nodes, `jobs`) own the keys, e.g. `provider` for the MSA provider that answered (architecture: "the recipe records which provider answered"), `worker`, `worker_version`, `model_version`. Nothing fills it yet; it is stored and round-tripped as data. Not part of any cache key.

Layout and provenance may only name nodes of the recipe; entries for other ids are an error on reading and are left out on writing (the canvas keeps positions of removed nodes for undo; the file does not).

## Canonical form

`to_toml` writes one canonical text, so that re-saving an unchanged graph is byte-identical and one change is a one-line diff:
- `[recipe]` first, then nodes **sorted by id**, then `[layout.positions]`, then provenance. Not topological order: a rewiring would then move whole blocks.
- Within a node: `kind`, `executor`, other `NodeData` fields in declaration order, `inputs`; then the `params` sub-table. Params, inputs, positions and provenance keys are sorted.
- Each param, position and provenance value is one line. All connections of a node are one line.
- Strings are basic strings (`"..."`) with escapes; floats always have a `.0` or an exponent (`8.0`, `1e-7`, `inf`, `nan`); keys are bare when TOML allows.
- Empty sections are not written; a node with no params has no `params` table.

Hand-written recipes may use any order, inline or header tables, and comments; they load the same.

**Comments are not preserved** by a load/save cycle (`to_toml` regenerates the text). The canonical form exists to make saved files diff cleanly; keeping comments would need a `toml_edit` document carried alongside the graph, which can come later without a format change.

## Errors

Every refusal is a `RecipeError { line, column, span, kind, message }` pointing at the key or value at fault: TOML syntax, an unknown key (typos are caught, never ignored), a missing `kind`/`format`/`[recipe]`, an unsupported `format`, a value of the wrong shape, a connection to a node that does not exist, a cycle. `from_toml_with(text, &registry)` also runs `Graph::validate` and places its errors: unknown node kind, unknown param, wrong param type, unknown input or output port, mismatched port types.

```text
line 41, column 77: input `cofold.msa` is connected to `msaa.msa`, but there is no node `msaa`
```

## Canonical example

The fixture above, saved by `to_toml` (a test keeps this block in sync):

```toml
[recipe]
format = 1
executor = "local"

[node.cofold]
kind = "cofold"
executor = "gpu-box"
inputs = { ligands = "prep.ligands", msa = "msa.msa", receptor = "receptor.structure" }

[node.cofold.params]
model = "boltz-2"
samples = 5
seed = 42

[node.gate]
kind = "pose_gate"
inputs = { poses = "cofold.poses", receptor = "receptor.structure" }

[node.gate.params]
max_clashes = 2
min_contacts = 4
pocket_radius = 8.0

[node.ligands]
kind = "load_ligands"

[node.ligands.params]
path = "fixtures/egfr/ligands.smi"

[node.msa]
kind = "msa_query"
inputs = { structure = "receptor.structure" }

[node.msa.params]
provider = "colabfold-public"

[node.prep]
kind = "prepare_ligands"
inputs = { ligands = "ligands.ligands" }

[node.prep.params]
ph = 7.4

[node.receptor]
kind = "load_structure"

[node.receptor.params]
chains = ["A"]
path = "fixtures/egfr/receptor.cif"

[layout.positions]
cofold = { x = 560, y = 60 }
gate = { x = 840, y = 60.5 }
ligands = { x = 0, y = 220 }
msa = { x = 280, y = -120 }
prep = { x = 280, y = 220 }
receptor = { x = 0, y = 0 }

[provenance.cofold]
model_version = "boltz-2.0.3"
worker = "mock-cofold"
worker_version = "0.1.0"

[provenance.msa]
endpoint = "https://api.colabfold.com"
provider = "colabfold-public"
```
