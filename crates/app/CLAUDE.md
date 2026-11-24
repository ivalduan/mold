# mold-app — workstream WS-11

The shell: GPUI Kit dock layout with resizable panels, theme, menus; wires the node canvas, viewport and panels to the graph executor and the selection bus.

**Don't start this before the W1 viewport ADR is signed.** An agent left alone will build the app shell because it feels like progress; it isn't the critical path.

## Scope
- Dock layout hosting `ui-graph`, `ui-viewport`, `ui-panels`.
- Owns the runtime: graph executor, job supervisor, selection bus instance.
- Open/save recipes through `mold-recipe`.

## Rules
- No domain logic here. If you write chemistry, geometry or scheduling code in this crate, it belongs further down.

## Acceptance tests (the v0.1 definition of done, end to end)
- [ ] Fresh clone to running demo in under 10 minutes (one command).
- [ ] The 11-node graph runs on the fixture; 300 co-folded poses scrub at 60 FPS.
- [ ] Graph saves to TOML and `mold run` replays it with identical outputs.
