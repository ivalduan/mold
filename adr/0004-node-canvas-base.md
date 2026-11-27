# 0004. Node canvas base

- Status: Accepted (2026-11-27): **our own canvas**, see [Decision](#decision)
- Date: 2026-11-25 (proposed), 2026-11-27 (accepted)
- Deciders: product owner (accepts); WS-09 ui-graph (proposes)
- Workstream: WS-09 ui-graph

## Context
`ui-graph` draws the `mold-graph` node graph, edits it through `mold_graph::Editor` (undoable `Edit`s), refuses mismatched `PortType`s while the wire is still being dragged, and shows per-node evaluation state and an "inspect output" hook (principle #3). It builds against the GPUI lineage pinned by ADR-0002 (GPUI Kit `=0.11.0` → GPUI Fast `=0.1.19`), which every UI crate reaches through `gpui-kit` only.

## Options
| Reference | Gives | Verdict so far |
|---|---|---|
| gpui-flow | React-Flow-style canvas: nodes, edges, handles, pan/zoom, custom renderers, GPUI components inside nodes | Closest fit, evaluate first |
| ferrum-flow | Node editor with plugin architecture and undo/redo command system | Second look, mainly for its command model |
| Write our own | Full control | Only if both fail the rubric |

## Rubric: reject any base that
owns the application's data model · owns the event loop · hard-codes a node payload instead of being generic over user data · doesn't render nodes through a supplied callback · pins a GPUI version we can't move.

## Default if nobody decides
**Fork gpui-flow** rather than depend on it: young crates, we need typed ports and per-node evaluation state; a fork costs a week, a wrong dependency is a permanent tax.

## Evaluation (2026-11-25, WS-09)
Both read at their default branch head, cloned outside the repo:

- **gpui-flow** = [`pacifio/gpui-flow`](https://github.com/pacifio/gpui-flow) at `cfa5787` (2026-03-23, "init v0.1.0"). The only crate by that name with a React-Flow API; `emojiiii/gpui-flow` (no licence, 0 stars, created 2026-09) was not considered.
- **ferrum-flow** = [`tu6ge/ferrum-flow`](https://github.com/tu6ge/ferrum-flow) at `43b762c` (2026-06-05), crates.io `ferrum-flow` 0.3.1.

| Criterion | gpui-flow | ferrum-flow |
|---|---|---|
| Owns the data model? | **Yes — fails.** `FlowState` holds `Vec<FlowNode>` / `Vec<FlowEdge>`; on drop it pushes a snapshot onto its own undo stack and inserts the edge itself (`graph.rs`, mouse-up); `on_connect` is only told afterwards. | **Yes — fails.** Its own `Graph` (nodes, ports, edges, parent/child tree); every change is a `Box<dyn Command>` mutating that graph through `CommandContext`. |
| Owns the event loop? | No — passes. A plain GPUI `Render` view with `on_mouse_*` / `on_scroll_wheel` listeners. | **Largely yes — fails.** `FlowCanvas` routes all input through a priority-ordered plugin chain (`Plugin::on_event` → `EventResult`) and its own `futures` channel; app behaviour lives in plugins, not in the host. |
| Generic over the node payload? | **No — fails.** `FlowNode` carries `label` + `node_type: Option<SharedString>`; no user data. | **No — fails.** `Node.data: serde_json::Value`. |
| Renders nodes through a supplied callback? | Yes — passes. `node_renderer(type, Fn(&FlowNode, &mut Window, &mut App) -> AnyElement)` and `default_renderer`. | Yes — passes. `trait NodeRenderer { render, port_render, port_offset }` per node type. |
| GPUI version we can move? | **No — fails as a dependency.** `gpui = { git = "https://github.com/zed-industries/zed" }`, unpinned, upstream Zed GPUI; examples need `gpui_macos`. Cannot coexist with GPUI Fast (different `App`/`Element` types; ADR-0002: nobody depends on `gpui` directly). | **No — fails as a dependency.** crates.io `gpui = "0.2.2"` (upstream Zed). Same conflict. |
| Typed ports, refusal *during* drag | No: `is_valid_connection` runs on mouse-up only; no hover verdict. Handles are untyped. | `EdgeValidator` runs at connect time; port types are free-form; `TypeMismatch` is "reserved". |
| Licence | MIT (`LICENSE`, © 2026 Adib): compatible with vendoring into Apache-2.0 with the notice kept. | Apache-2.0. |
| Size | ~2.4 k lines of library (`graph.rs` 1 083, `store.rs` 450, `types.rs` 383, `edges/` 528), 3 examples. | ~24 k lines across 5 crates (core, executor, shader, meili, collaboration via `yrs`). |
| Activity | 1 commit, 53 stars, no push since 2026-03-23. | 621 commits, 97 stars, last push 2026-06-05; none since. |

**ferrum-flow's command model, the reason for the second look.** `trait Command { execute, undo, to_ops }` over a `CommandContext` that borrows the canvas's graph, port-layout cache, viewport and renderers; `LocalHistory` keeps up to 100 boxed commands. `mold-graph` already has the better model for us: `Edit` is plain data (serializable, diffable, replayable headless), `Graph::apply` validates against the `Registry` and returns the inverse, `History`/`Editor` keep (forward, inverse) pairs, and undo never touches work items or the cache (ADR-0010). Nothing to take from it.

**What a fork of gpui-flow would keep.** Removing `FlowState`'s ownership of nodes/edges, its snapshot undo, its untyped handles and its drop-only validation leaves: the viewport transform (a dozen lines), the render structure (nodes as absolutely positioned divs, edges painted with `PathBuilder` in a `canvas` layer), and the edge geometry in `src/edges/`: `bezier.rs` is a port of xyflow's `getBezierPath`, pure `f32` maths with no GPUI types. A wholesale fork of `graph.rs`/`store.rs` would be rewritten line by line, which makes "unmodified upstream, then our diff" meaningless to review.

## Decision
**Accepted 2026-11-27 by the product owner: write our own canvas.** What gpui-flow contributes (one ~80-line bezier function) is too small to call a fork. The rule going forward: adopt a Rust crate only when it gives us something significant that passes the rubric; otherwise we build it ourselves. Re-evaluate only if such a crate appears.

Concretely:
1. The canvas is ours: the GPUI-free core (`mold_ui_graph::canvas`) and the GPUI view (`mold_ui_graph::view`), as described in point 2 of the proposal below.
2. The xyflow bezier function stays as an attributed MIT snippet in `crates/ui-graph/src/vendor/gpui_flow/`. It is a borrowed utility, not a base we track: upstream gpui-flow is not followed, and we may rewrite or replace the function freely.
3. Undo/redo stays `mold_graph::Editor`'s (point 3 below). ferrum-flow: not adopted.

### Proposal as evaluated (2026-11-25, kept for the record)
Neither candidate passes the rubric as a dependency: both own the graph and hard-code the payload, and both are built on upstream Zed GPUI, which cannot be mixed with the GPUI Fast lineage. Of the two, gpui-flow fails only on things a fork removes, so the default holds in a **narrow** form:

1. **Fork gpui-flow by vendoring what survives the rubric**: `src/edges/bezier.rs` (xyflow bezier edge path, MIT) into `crates/ui-graph/src/vendor/gpui_flow/`, licence and upstream revision preserved, committed unmodified and then adapted in a separate commit. Row in `THIRD_PARTY.md` as vendored source.
2. **Write the rest ourselves, shaped after gpui-flow's `graph.rs`** (div per node + painted edge layer), on GPUI Kit:
   - a GPUI-free core (`mold_ui_graph::core`): view state only (camera, node positions, selection, the gesture in progress), layout of a `&Graph` + `&Registry` into a view model, hit-testing, and the drag state machine that asks `PortType::accepts` on every hover and emits `mold_graph::Edit`s;
   - a thin GPUI view (Windows only for now, like `ui-viewport`) that paints the view model and forwards pointer and keyboard input to the core; every edit is applied through `mold_graph::Editor`.
3. **Undo/redo is `mold_graph::Editor`'s**; no canvas-side history. Node moves and pan/zoom are view state, not undoable edits (see `crates/ui-graph/CLAUDE.md`, Design notes).
4. ferrum-flow: not adopted, nothing vendored.

The base-specific code is confined to `crates/ui-graph/src/vendor/gpui_flow/` (geometry) and `crates/ui-graph/src/view/` (GPUI view), so a different base later replaces those modules without touching the core or its tests.

## Consequences
- The canvas never stores the graph; the tests for refusal, connect + undo and new node kinds run headless on every OS.
- We maintain the canvas code ourselves instead of tracking an inactive upstream. Upstream gpui-flow changes are not followed; the vendored file is small enough to diff by hand.
- Features gpui-flow has and we have not ported (minimap, box selection, smooth-step/straight edges, snap-to-grid, edge labels) stay out of v0.1 scope; any of them can be ported from the same upstream revision under the same vendoring rule.
- Minimap, box selection, other edge shapes, snap-to-grid and edge labels are features we write when we need them, not ports from gpui-flow.
- If a canvas crate worth adopting appears later, only `src/view/` should change; the core API stays.
