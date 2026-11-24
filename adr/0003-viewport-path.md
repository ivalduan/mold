# 0003. Viewport path

- Status: **Proposed (W1 gate)**
- Workstream: WS-00 spike

## Context
The risk that would kill the PoC was the 3D viewport. `javagg/gpui-fast-wgpu-demo` now shows a custom GPUI `Element` rendered by wgpu with GPUI UI above it, no fork (reported 104 FPS at 1796×1522, 4× MSAA on M1 Max; 5.7 ms per offscreen frame for 5.4 M triangles).

Platform-neutral (gpui-fast): `enable_window_composition` → `create_native_surface` → `set_bounds` / `set_visible` / `on_input`.
Ours per platform (~40 lines each): native handle → wgpu surface. Windows: composition visual; macOS: NSView pointer + `CAMetalLayer`; Linux: `RawWindowHandle` → Vulkan.

Known Windows issues: commit only via `set_parent_origin`; `set_visible`/`remove_surface` fail with `E_NOINTERFACE` → hide with empty bounds.

## Proposed decision
Develop on **Windows** first. Week 0 = port the demo's `src/viewport.rs` plus the two workarounds. macOS and Linux ≈ a day each later, not a parallel track. Browser out of scope for v0.1 (needs unmerged gpui-fast PR #102 and can't click-pick).

## Spike exit criteria (all must hold to accept)
- [ ] 500 impostor spheres draw through the ported surface.
- [ ] GPUI menus paint above the surface.
- [ ] Pointer picking round-trips.
- [ ] Hide/show a viewport tab works on Windows.

If not met by W1: cut the platform list rather than spend another week.

## Decision
_Pending spike._
