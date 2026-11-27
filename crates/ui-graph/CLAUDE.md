# mold-ui-graph — workstream WS-09

The node canvas. Runs W2–W4, in parallel with the renderer.

## Base (ADR-0004, default if nobody decides: fork gpui-flow)
**Evaluated 2026-11-25; ADR-0004 is Proposed** with a narrow fork: gpui-flow's bezier edge geometry is vendored (`src/vendor/gpui_flow/`), the rest is our own, shaped after gpui-flow's `graph.rs`. Both candidates own their graph, hard-code the payload and sit on upstream Zed GPUI. Original brief:
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
Headless, in `tests/acceptance.rs` (run on every OS):
- [x] Dragging a `Table` output onto a `PoseSet` input is refused before drop (`dragging_a_table_output_onto_a_poseset_input_is_refused_before_drop`: refused on hover, every input marked during the drag, drop emits no edit, graph and history unchanged).
- [x] A compatible drag + drop is exactly one `Connect` edit, and one undo removes it (`a_compatible_drop_is_exactly_one_connect_edit_and_undo_removes_it`).
- [x] Adding a new node kind requires no change in this crate (`a_new_node_kind_needs_no_change_in_this_crate`: a kind defined only in the test is laid out through the default renderer path; a host renderer is picked up by kind name).
- [ ] GPUI smoke test that builds the canvas view: not yet (see Design notes).

## Layout of the crate
- `src/canvas/` — GPUI-free, builds everywhere: `CanvasState` (view state + gesture state machine), `layout` (`&Graph` + `&Registry` → `CanvasModel` in screen px), `GraphDoc` (`Editor` + `EvalStates`), `EvalStates`/`EvalState`, geometry.
- `src/view/` — GPUI view, `cfg(windows)` like `ui-viewport`: `GraphCanvas`, `NodeRenderer`, `DefaultNodeRenderer`, `EditSink`. The only base-specific code besides `src/vendor/`.
- `src/vendor/gpui_flow/` — vendored MIT source; every change after the unmodified commit is its own commit.
- `examples/canvas.rs` — five fake kinds, custom renderer, background evaluation through `Engine::cook_with` + `Observer`.

## Design notes
- **The canvas holds no graph.** `CanvasState` is camera, node positions, selection and the gesture; `layout` rebuilds the view model from `&Graph` every frame. The GPUI view holds an `Entity<GraphDoc>` the host owns (and can share with other panels).
- **Every graph change is a `mold_graph::Edit`** returned by the state machine (`Outcome::Edit`) or built by a node body (`EditSink`), applied through `GraphDoc::apply` → `Editor::apply`. Undo/redo is `Editor`'s; the canvas has no history of its own.
- **Refusal at drag time**: `check_connection` asks `PortType::accepts` (the same rule `Graph::apply` enforces) and `Graph::downstream` for cycles, on every hover; during a drag every input port carries a `DropVerdict`. A refused drop yields no edit. `Graph::apply` still validates whatever reaches it.
- **Edge pick-up**: pressing a bound input lifts its edge; dropping it on another accepted input is one `Batch[Disconnect, Connect]` (one undo step), on empty canvas a `Disconnect`, back on its own input nothing.
- **Node positions are view state** (`CanvasState::positions`), kept for removed nodes so undoing a removal restores the position. Nodes without one are placed by dependency depth. Moving nodes is not an undoable edit. **They should persist as recipe metadata** (e.g. a `[layout]` table keyed by node id, ignored by headless replay): a WS-06 `recipe` need, reported, not done here. Whether moves become undoable then depends on where that metadata lives (an `Edit` variant would be a `mold-graph` contract change).
- **Evaluation badges**: `EvalStates` is a host-fed map. `EvalStates::observe(&WorkItem)` fits `mold_graph::Observer` unchanged (the example cooks on a background thread and forwards items over a channel); `EvalStates::from_item_graph` reads `Engine::generate`/`cook` results. Node state = failed > running > dirty > cached over its items. `GraphDoc` marks changed nodes and their downstream dirty after every edit/undo/redo; the next `Engine::generate` gives the exact picture (an undo can make items cache hits again).
- **Inspect output** (principle #3): a header button; `Outcome::Inspect(node)` → the host's `GraphCanvas::on_inspect` callback. No viewer here.
- **Node kinds are generic**: ports and params come from `NodeSpec`; bodies go through `Renderers<dyn NodeRenderer>` keyed by kind with `DefaultNodeRenderer` as fallback. `NodeRenderer: NodeMetrics`, and `NodeMetrics::body_height` is GPUI-free, so layout and hit-testing are testable headless.
- **Interactive node bodies** must `cx.stop_propagation()` on mouse-down, or the press also grabs the node (the canvas does its own hit-testing on the root element).
- **Viewport overlap (ADR-0003)**: the canvas paints no popups. If it is ever placed over the 3D viewport, wrap it in `deferred(..)`.
- **Node selection** here is canvas view state, not `mold_mol::Selection` (atoms/poses).
- **GPUI smoke test**: GPUI Kit has a `test-support` feature (`TestAppContext::add_window`), but enabling it as a dev-dependency feature unifies into every `cargo test --workspace` build of the GPUI tree on Windows (`ui-viewport` included), so it is left for the app/CI owners to decide; the view is exercised by the example meanwhile.
- **Linux/macOS**: the view is `cfg(windows)` like `ui-viewport`, so non-Windows CI builds and tests the core with no system packages. Nothing in `src/view/` is Windows-specific; lifting the gate needs only the Linux CI packages listed in ADR-0003.
