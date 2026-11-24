# mold-recipe + mold-cli — workstream WS-06

Graph ⇄ text, both ways (product principle #2). The recipe is TOML; the CLI replays it headless.

## Scope
- `recipe`: serialize/deserialize the `mold-graph` model to TOML. Stable key ordering, readable diffs, comments survive where possible. Records provenance: MSA provider used, worker model versions.
- `cli` (`crates/cli`): `mold run recipe.toml` replays a recipe with **no window and no GPU compositor**. Reaches only into the orchestration and domain bands.

## May depend on
`recipe`: `graph`, `serde`, `toml`. `cli`: `recipe`, `graph`, `nodes`, `jobs`. **Never** GPUI or any `ui-*` crate.

## Acceptance tests
- [ ] graph → TOML → graph is the identity (property test over generated graphs).
- [ ] CLI replay of the fixture recipe with `mock-cofold` produces **byte-identical** tables to the in-app run.
- [ ] A hand-edited recipe (param changed in a text editor) loads back into the graph.
