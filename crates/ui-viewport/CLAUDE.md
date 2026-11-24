# mold-ui-viewport — workstream WS-00 (week-0 spike), then WS-08 integration

**Week 0 is a spike whose only deliverable is written decisions** ([ADR-0002](../../adr/0002-gpui-lineage.md), [ADR-0003](../../adr/0003-viewport-path.md)). Do not build the app shell.

## Reference
`javagg/gpui-fast-wgpu-demo`: a CAD viewport as a custom GPUI `Element` rendered by wgpu, using gpui-fast's public API, no fork. Its `src/viewport.rs` is the Windows path and the best-tested one.

## Week-0 spike tasks
1. Port the demo's `src/viewport.rs` (Windows).
2. Draw 500 impostor spheres through it.
3. Confirm GPUI menus still paint above the surface.
4. Confirm pointer picking round-trips.
5. Write ADR-0003 with the result. **If no decision by W1, cut the platform list; don't spend another week.**

## The seam (keep it thin)
`ViewportBackend` answers two questions only: *give me a wgpu surface for this GPUI native surface* and *does this platform need an explicit commit after presenting*. Everything above it (camera, scene, pipelines, picking) is `mold-render` and never sees GPUI.

gpui-fast flow (same on every platform): `window.enable_window_composition()` → `composition.create_native_surface()` → `set_bounds`, `set_visible`, `composition.on_input`.

## Known Windows workarounds (carry over from the demo)
- `platform_handle()` is untyped (`Any`) → wrap the downcast once, fail loudly with a clear message; pin the gpui-fast revision in `Cargo.lock`.
- Windows must commit, and only `set_parent_origin` does it (backend side effect, not a guarantee).
- `set_visible` and `remove_surface` fail with `E_NOINTERFACE` on Windows → hide by giving the surface **empty bounds** until gpui-fast visuals implement `IDCompositionVisual3`. Side-by-side pose comparison depends on this.
- The viewport owns its own device and render thread (DX12 beside GPUI's D3D11). A slow frame never blocks GPUI.

## Later
macOS (`CAMetalLayer` on the NSView) and Linux (`RawWindowHandle` → Vulkan) are ~40 lines each, about a day each, after the Windows PoC. Browser/WASM is out of scope for v0.1.

## Acceptance tests
- [ ] Click a pose → camera re-aimed in under 100 ms.
- [ ] Hiding/showing a viewport tab works on Windows (empty-bounds path).
