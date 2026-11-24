# 0010. Work-item graph model (PDG-style)

- Status: Accepted
- Date: 2026-11-24
- Workstream: WS-04 graph

## Context
The v0.1 plan assumed whole-node evaluation: a node is dirty or clean, cached as a whole. Houdini PDG/TOPs ([intro](https://www.sidefx.com/docs/houdini/tops/intro.html)) instead schedules, caches and runs **work items**: nodes generate items, each item cooks as soon as its own prerequisites are done, and only changed items re-cook. Mold's core workload (one receptor × 500 ligands → poses → gate → profile) is naturally per item, and pose streaming, per-ligand farm jobs and parity of execution flavours (ADR-0009) all follow from it.

## Decision
`mold-graph` is item-based from the start.
- **Two levels**: the *node graph* (what users edit and recipes store: nodes, typed ports, params, undo) generates a *work-item graph* (what is scheduled, cached and executed).
- **Work items** carry an id, owning node, index, parents, inherited typed **attributes**, **outputs** and a state.
- **Processors** generate items (from params, or fan-out from upstream items); **partitioners** fan in (wait-for-all, group by attribute) and merge.
- **Static and dynamic generation**, chosen per processor in the spirit of Houdini's "Generate When": when all upstream items are generated, when all are cooked, or as each one cooks.
- **Dependencies, cache keys and dirtying are per item.** Key = hash(node kind + params + parent items' attribute/output hashes).
- **Executor seam per item**, async completion, cancellation; in-process executor first. The default executor is set per graph and can be overridden per node.

## Consequences
- "A step earns its own node when it has its own cache key, failure mode or runtime" still holds; inside a node, items give per-ligand / per-pose granularity.
- Typed ports describe what each item on an edge carries.
- The `nodes` workstream authors item-generating nodes; `docs/authoring-a-node.md` must cover generation.
- The `jobs` workstream (ADR-0009) builds subprocess and external executors on the item seam.
- Deferred: services/persistent worker pools, batch items, mappers, `@attr` parameter expressions, persistent on-disk cache.
