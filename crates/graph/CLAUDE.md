# mold-graph — workstream WS-04

The typed DAG, its evaluator and its cache. **No existing crate models typed scientific DAGs well; we build it.**
Read [docs/architecture.md](../../docs/architecture.md#execution-engine) first.

## Scope
- **Model**: nodes with stable `id`, `kind` string, parameter map, input bindings; typed ports (`PortType`). Same shape in memory and in the recipe.
- **Evaluation**: pull-based, dirty-propagating, concurrent. A parameter change dirties its node and everything downstream; requesting an output walks up the inputs.
- **Scheduler**: a **work queue over ready nodes**, not a recursive walk. Otherwise the MSA's minutes serialize in front of ligand prep.
- **Cache**: each node's result keyed by a content hash of (its params + its inputs' hashes), written to a run directory. Changing only the gate threshold re-runs only the gate.
- **Undo**: command pattern over graph edits only (node added, param changed, edge connected). Pose data is derived state: never undone, recomputed or read from cache.
- **Node trait**: the author's contract: declare ports, declare parameters, implement `evaluate`. Target: one file, under 100 lines, to add a node. If adding a node touches a UI crate, the abstraction is wrong.

## Contract (changes go to `main` first)
`PortType`, the node trait, the graph data model that `recipe` serializes and `ui-graph` renders.

## May depend on
`mold-mol`, `serde`, a hashing crate, a thread pool. **Never** GPUI.

## Acceptance tests
- [ ] Changing a downstream parameter does not re-evaluate upstream nodes (count evaluations).
- [ ] Two independent branches evaluate concurrently (a slow mock node does not block a fast sibling).
- [ ] Cache hit across process restarts from the run directory.
- [ ] Undo/redo restores the graph exactly; does not touch cached results.
