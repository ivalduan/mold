# mold-recipe + mold-cli — workstream WS-06

Graph ⇄ text, both ways (product principle #2). The recipe is TOML; the CLI replays it headless.

## Scope
- `recipe`: serialize/deserialize the `mold-graph` model to TOML. Stable key ordering, readable diffs, comments survive where possible. Records provenance: MSA provider used, worker model versions.
- `cli` (`crates/cli`): `mold run recipe.toml` replays a recipe with **no window and no GPU compositor**. Reaches only into the orchestration and domain bands.

The format is specified in [FORMAT.md](FORMAT.md).

## May depend on
`recipe`: `graph`, `serde`, `toml`. `cli`: `recipe`, `graph`, `nodes`, `jobs`. **Never** GPUI or any `ui-*` crate.

## Acceptance tests
- [x] graph → TOML → graph is the identity (property test over generated graphs): `tests/acceptance.rs::graph_to_toml_to_graph_is_the_identity` (512 cases: params of every kind incl. nested lists, ±inf, -0.0; executors; layout; provenance; ids/keys with dots, quotes, newlines, non-ASCII), plus `every_finite_f32_position_survives`.
- [ ] CLI replay of the fixture recipe with `mock-cofold` produces **byte-identical** tables to the in-app run. *Deferred until `nodes`, `jobs` and the mock worker exist.*
- [x] A hand-edited recipe (param changed in a text editor) loads back into the graph: `a_param_changed_in_a_text_editor_loads_back_with_only_that_param_changed` (string edit of the fixture; the result equals the original with that one param set).

Also covered (from the WS-06 brief):
- [x] Byte stability: serialize → parse → serialize is byte-identical; one param / executor / connection / position change is a one-line diff (`serialize_parse_serialize_is_byte_identical`, `one_change_is_a_one_line_diff`).
- [x] Errors: every `RecipeError` case reports the line and column of the offending key or value and a useful message (`tests/errors.rs`, one test per case).
- [x] Layout: a recipe without `[layout]` loads with the same graph; layout changes leave `Graph` (hence every cache key) unchanged.
- [x] `FORMAT.md`'s canonical example is the fixture's actual `to_toml` output (`format_md_shows_the_canonical_form_of_the_fixture`).

## Layout of the crate
- `src/lib.rs` — `Recipe { graph, layout, provenance }`, `Layout`/`Position`, `Provenance`, `to_toml`, `from_toml`, `from_toml_with`, `FORMAT_VERSION`.
- `src/write.rs` — the canonical emitter. `src/read.rs` — span-aware reader. `src/error.rs` — `RecipeError`/`ErrorKind`. `src/probe.rs` — field names and defaults from serde derives.
- `tests/fixtures/egfr_cofold.toml` — the hand-written fixture (receptor → MSA, ligands → prep → co-fold → gate). `tests/common` — test-only stub specs for its node kinds (`nodes` is still empty).

## Design notes
- **API: a `Recipe` struct, not a tuple.** `from_toml(&str) -> Result<Recipe, RecipeError>` returns graph, layout and provenance together; `to_toml(&Graph, &Layout, &Provenance) -> String` takes borrowed parts so the app can save straight from its `Editor` and `CanvasState` without cloning the graph (`Recipe::to_toml` for the owned case). `from_toml_with(&str, &Registry)` adds `Graph::validate`. Without a registry the reader still refuses dangling connections and cycles (they need no specs).
- **Nodes as `[node.<id>]` tables, sorted by id; params as a `[node.<id>.params]` sub-table, one per line; inputs as one inline `inputs = { port = "node.port" }` line.** Id order (not topological) so rewiring never moves blocks; one line per param so a param edit is a one-line diff. The connection string splits at the last dot, so node ids may contain dots; the long form `{ node, port }` covers ports with dots and is always accepted.
- **Hand-rolled writer, `toml`'s `DeTable` reader.** The text layout *is* the format, so a generic serializer would not do: the writer decides headers vs inline tables, order and number formatting (its own float text: `toml`'s `Display` writes `1e300` as 301 digits). The reader uses `toml` 1.x's span-carrying `DeTable` so every error has a position. `toml` was already in the lock (a gpui-fast build-dependency); `toml_edit` was not needed.
- **Comments are not preserved** across load/save (the writer regenerates canonical text). Cheap preservation does not exist with a regenerating writer; carrying a `toml_edit` document alongside the graph and patching it can come later without a format change.
- **Forward compatibility with `NodeData`/`Graph` fields (ADR-0009 §7).** `probe::struct_fields` reads a type's field names from its serde derive (a probe `Deserializer` that records `deserialize_struct`'s `fields`). Reading: any listed field other than `kind`/`params`/`inputs` is passed through serde, anything else is an unknown-key error, so `retries`/`timeout`/`on_failure` need no code here and typos are still caught. Writing: fields go out in declaration order, and a field equal to its default (computed by `probe::default_table`) is left out even without `skip_serializing_if`. A serde error is placed at the right value by re-trying each field alone on top of `kind`. A `#[serde(flatten)]` on `NodeData` would defeat the probe; a unit test guards that.
- **Registry errors are placed by mapping, not by re-implementing validation.** The reader records spans per node/kind/param/input, runs `Graph::validate` and maps the `GraphError` to the matching span, so the rules stay in `mold-graph`.
- **Layout is `[layout.positions]`, `f32`, positions only.** Matches `ui-graph`'s `Vec2`; whole numbers are written as integers (`x = 320`), others as shortest `f32` text, which reads back exactly through TOML's `f64` (property-tested). The camera is not stored, so panning does not dirty a file. Layout is not in `Graph`, so it is in no cache key by construction.
- **Provenance is `[provenance.<id>]`, free-form `ParamValue` records.** Nothing fills it yet; its writers (MSA node, `jobs`) own the keys. Graph-level provenance (e.g. Mold version) can be added as a sibling table later.
- **Strictness vs. stale metadata.** Layout/provenance entries for ids that are not nodes are an error on reading (a renamed node with a stale position is a typo to fix), and are dropped on writing (the canvas keeps removed nodes' positions for undo). Empty provenance records are not written.
- **Fixture location.** The fixture lives in `crates/recipe/tests/fixtures/` (crate-owned, a few KB). If the CLI replay test wants it under the shared `fixtures/` tree (ADR-0012), move it then.
