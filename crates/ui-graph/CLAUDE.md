# mold-ui-graph — workstream WS-09

The node canvas. Runs W2–W4, in parallel with the renderer.

## Base (ADR-0004, default if nobody decides: fork gpui-flow)
Evaluate `gpui-flow` first (React-Flow-style: nodes, edges, handles, pan/zoom, custom renderers, GPUI components inside nodes). Second look: `ferrum-flow`, mainly for its undo/redo command model. **Expect to fork, not depend**: young crates; we need typed ports; a fork costs a week, a wrong dependency is a permanent tax.

## Adoption rubric: reject a base that fails any of these
- It owns the application's data model. **The canvas must not store our graph.** `mold-graph` does.
- It owns the event loop.
- Its node type hard-codes a payload instead of being generic over user data.
- It doesn't render nodes through a callback we supply.
- It pins a GPUI version we can't move.

## Scope
- Render `mold-graph` nodes and edges; edits go through `mold-graph`'s command/undo API.
- Refuse mismatched `PortType` connections **at drag time** (`PortType::accepts`).
- Show per-node evaluation state (dirty / running / cached / failed) and a way to inspect any node's output (product principle #3).

## May depend on
`mold-graph`, GPUI Kit. Never `nodes` internals: the canvas is generic over node kinds.

## Acceptance tests
- [ ] Dragging a `Table` output onto a `PoseSet` input is refused before drop.
- [ ] Adding a new node kind requires no change in this crate.
