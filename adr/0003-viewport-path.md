# 0003. Viewport path

- Status: **Proposed (W1 gate)**. Spike done; the product owner accepts after the checklist below.
- Date: 2026-11-24
- Deciders: product owner (accepts); WS-00 spike (proposes)
- Workstream: WS-00 spike

## Context
The risk that would kill the PoC was the 3D viewport. `javagg/gpui-fast-wgpu-demo` now shows a custom GPUI `Element` rendered by wgpu with GPUI UI above it, no fork (reported 104 FPS at 1796×1522, 4× MSAA on M1 Max; 5.7 ms per offscreen frame for 5.4 M triangles).

Platform-neutral (gpui-fast): `enable_window_composition` → `create_native_surface` → `set_bounds` / `set_visible` / `on_input`.
Ours per platform (~40 lines each): native handle → wgpu surface. Windows: composition visual; macOS: NSView pointer + `CAMetalLayer`; Linux: `RawWindowHandle` → Vulkan.

Known Windows issues: commit only via `set_parent_origin`; `set_visible`/`remove_surface` fail with `E_NOINTERFACE` → hide with empty bounds.

## Proposed decision
Develop on **Windows** first. Week 0 = port the demo's `src/viewport.rs` plus the two workarounds. macOS and Linux ≈ a day each later, not a parallel track. Browser out of scope for v0.1 (needs unmerged gpui-fast PR #102 and can't click-pick).

## What the spike built
On the lineage pinned by ADR-0002 (GPUI Kit 0.11.0 + GPUI Fast 0.1.19, wgpu 30.0.1), in `crates/ui-viewport`:

- `backend.rs`: the `ViewportBackend` seam, two methods (`create_surface(instance, platform_handle: Box<dyn Any>)`, `needs_commit_after_present()`), no GPUI types, builds on every platform.
- `platform_windows.rs`: the Windows backend. The one downcast of `platform_handle()` to `windows::core::IUnknown`, cast to `IDCompositionVisual`, `SurfaceTargetUnsafe::CompositionVisual` on a DX12 instance. A wrong handle type fails with a message naming the tested gpui-fast and ADR-0002 (unit-tested).
- `element.rs`: the GPUI `Element` + native surface, ported from the demo's `viewport.rs`/`viewport_element.rs`: placement in prepaint, pointer input via `composition.on_input` forwarded to the render thread, **hide = empty bounds**, **commit = `set_parent_origin`** after the first present following each configure.
- `spike/` (**spike-only**, stands in for `mold-render`): orbit camera, CPU ray picking, a deterministic 500-atom scene, a one-pipeline sphere-impostor renderer (instanced quads, ray-sphere in the fragment shader, exact depth, reversed Z) on its **own DX12 device and render thread**.
- `examples/spike.rs`: a GPUI Kit window with a toolbar, a GPUI Kit dropdown menu ("View ▾") and a right-click context menu over the viewport, a HUD drawn with `deferred`, a hide/show button, and a `--self-test` mode.

## Verified by the spike author (Windows 11, RTX 5060 Laptop GPU, DX12; 2026-11-24)
`cargo run -p mold-ui-viewport --example spike --release -- --self-test` → **SELF-TEST PASSED** (exit 0):

| Check | Result |
|---|---|
| Handle downcast on gpui-fast 0.1.19 | works; swap chain attached, first frame presented |
| 500 impostor spheres through the composition surface | drawn (screenshot) |
| Deferred GPUI content above the surface | HUD and context menu visibly above the spheres (screenshot) |
| Click → re-aim, 10 synthetic clicks (pick at the atom's projected pixel, on the render thread) | 10/10 hit an atom and re-aimed; click → re-aimed frame presented **min 0.8 / median 1.0 / max 1.4 ms** (budget 100 ms); ray cast over 500 atoms 2–14 µs; picked atom lands at the viewport centre (0.00 px error; screenshot shows the highlighted atom centred) |
| Slow frames do not block GPUI | render thread sleeping 150 ms per frame: worst UI-thread tick delay 16.0 ms, the same as idle (the ~16 ms is Windows timer granularity, not a stall) |
| Hide (empty bounds) | surface disappears, placeholder visible (screenshot); hidden viewport drew **0 frames** while asked to spin |
| Show | surface returns at the same place, HUD above it, frames resume (screenshot) |
| Headless unit tests (camera ray ⇄ projection, ray-sphere, nearest hit, occlusion, click-on-visible-atom picks it and re-aims on it for every visible atom of the scene, pick cost, handle error) | 13 tests, run on CI |

Measurement notes: latency starts when the click reaches the UI thread (GPUI's `on_input`; for self-test clicks, when the UI thread sends the click) and stops when `Queue::present` returns for the first frame with the re-aimed camera. The compositor then needs up to one or two refreshes (~16–33 ms at 60 Hz) to scan it out. Still well inside 100 ms. The re-aim is instant (no glide); a glide would start within the same budget.

## For the product owner to confirm (visual, by hand)
Run on the Windows dev box, from the `ws/spike` worktree:

```sh
cargo run -p mold-ui-viewport --example spike --release
```

(First build ~5 min. `-- --self-test` runs the automatic sequence above and exits; ~20 s.)

- [ ] **500 spheres** fill the viewport, shaded, with the HUD box (amber title "GPUI overlay (deferred), above the wgpu surface") in the top-left corner *over* the spheres.
- [ ] **Menus paint above the surface**: click "View ▾": the GPUI Kit popup menu drops down *over* the spheres, not behind them; its items work ("Spin", "Stop spinning"). Right-click on the spheres: the context menu appears at the pointer, over the spheres.
- [ ] **Picking round-trips**: left-click an atom: it turns green, the camera turns so it is at the centre of the viewport, and the terminal prints `pick: atom #… | click -> re-aimed frame presented … ms (budget 100 ms)`. Click empty space: `pick: nothing under the pointer`. Drag orbits around the picked atom; the wheel zooms.
- [ ] **Hide/show**: "Hide viewport" leaves the text "viewport hidden (empty-bounds path)" and no spheres; "Show viewport" brings them back at the same size and place. Repeat a few times; also resize the window while shown and after showing.
- [ ] **Slow frames don't freeze the UI**: "View ▾" → "Spin with 150 ms frames": the spheres turn jerkily, but menus, buttons and the HUD stay responsive; "Stop spinning" ends it.
- [ ] Not black on start, not black after a resize (the commit workaround).

Known rough edges, not part of the exit criteria: clicking the viewport while the GPUI Kit dropdown is open may not close it (viewport input goes to the render thread, not GPUI's dismiss logic; the context menu is closed explicitly); no MSAA on the spike impostors.

## Spike exit criteria (all must hold to accept)
- [x] 500 impostor spheres draw through the ported surface. *(verified by screenshot)*
- [ ] GPUI menus paint above the surface. *(context menu + HUD verified by screenshot; GPUI Kit dropdown to be confirmed by the product owner)*
- [ ] Pointer picking round-trips. *(verified with synthetic clicks; real mouse click to be confirmed by the product owner)*
- [ ] Hide/show a viewport tab works on Windows. *(verified by self-test and screenshots; product owner confirms by hand)*

If not met by W1: cut the platform list rather than spend another week.

## Decision (proposed)
Adopt the path: a GPUI `Element` over a GPUI Fast native surface, with wgpu (own DX12 device, own render thread) behind the two-method `ViewportBackend` seam, **Windows only for v0.1 development**; macOS (`CAMetalLayer`) and Linux (`RawWindowHandle` → Vulkan) each later as one backend file plus a target table in `ui-viewport/Cargo.toml`. Keep both Windows workarounds (empty-bounds hide, `set_parent_origin` commit) until gpui-fast fixes them upstream. Nothing fundamental failed, so no platform cut is needed.

## Consequences
- Rule for every UI crate: **anything that must appear over the viewport is drawn with `deferred(..)`** (GPUI Kit popups, menus and dialogs already are). A plain child placed "on top" in the element tree is painted in GPUI's base layer, behind the swap chain.
- GPUI does not hide a native surface when its element stops being drawn; the owner of the viewport (later the app's dock/tab) must call `Viewport::set_visible(false)`. Closed surfaces can't be removed on Windows; reuse them (as the demo does) rather than create new ones per tab.
- `mold-render` (WS-08) takes over `spike/` in shape, not code: owns device + render thread, receives a `wgpu::Surface` from `ui-viewport`, emits "commit now" after the first present following a configure, takes pointer events in surface pixels with a timestamp, emits pick results. Its instance must use `backend::wgpu_backends()` (DX12 on Windows: only DX12 can target a composition visual). It should depend on the workspace `wgpu =30.0.1`.
- The commit relies on an implementation detail of gpui-fast's Windows backend; ADR-0002's upgrade policy makes every gpui-fast bump re-run this spike.
- **CI**: `ui-viewport` depends on GPUI only on Windows, so the Linux job builds and tests the crate headless with no system packages. The Windows job compiles GPUI Kit + GPUI Fast (no extra packages; longer build). When a UI crate builds GPUI on Linux (ui-graph, ui-panels, app, or a Linux viewport backend), the Ubuntu job needs, for the current feature set (Wayland only, no `x11`): `pkg-config libfontconfig1-dev libfreetype-dev libwayland-dev libxkbcommon-dev` (from the `-sys` crates under `gpui-*-linux`: `yeslogic-fontconfig-sys`, `freetype-sys`, `wayland-sys`, `xkbcommon`). If `x11` is enabled, add `libxkbcommon-x11-dev libx11-xcb-dev libxcb1-dev`. GPUI Kit's own CI installs `gcc g++ clang libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev libasound2-dev libvulkan1` on Ubuntu 24.04, a safe superset. Not yet verified on a Linux machine.
- Upstream issues to file with gpui-fast: see ADR-0002.
