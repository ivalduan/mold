# mold-ui-viewport — workstream WS-00 (week-0 spike), then WS-08 integration

**Week 0 is done.** [ADR-0002](../../adr/0002-gpui-lineage.md) and [ADR-0003](../../adr/0003-viewport-path.md) are **Accepted** (2026-11-24): the lineage is pinned and the viewport path is proven on Windows. The app shell is no longer blocked — see [app/CLAUDE.md](../app/CLAUDE.md) (WS-11).

What is in here is a **spike**, not the product: `src/spike/` is a stand-in for `mold-render` (WS-08) and is meant to be replaced, not grown. The seam (`backend.rs`) and the element (`element.rs`) are the parts that stay.

## Reference
`javagg/gpui-fast-wgpu-demo`: a CAD viewport as a custom GPUI `Element` rendered by wgpu, using gpui-fast's public API, no fork. Its `src/viewport.rs` is the Windows path and the best-tested one.

## Week-0 spike tasks
- [x] Port the demo's `src/viewport.rs` (Windows): `src/element.rs` + `src/platform_windows.rs`.
- [x] Draw 500 impostor spheres through it (`examples/spike.rs`, spike renderer in `src/spike/`).
- [x] Confirm GPUI menus still paint above the surface (context menu + HUD by screenshot; GPUI Kit dropdown confirmed by the product owner).
- [x] Confirm pointer picking round-trips (synthetic clicks 10/10; a real mouse click confirmed by the product owner).
- [x] Write ADR-0003 with the result. **Accepted 2026-11-24**; the W1 gate passed, so no platform cut was needed.

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
- [ ] Click a pose → camera re-aimed in under 100 ms. *(Spike: click an atom → re-aimed frame presented in ~1 ms, 10/10 synthetic clicks; real poses need `mold-render` + `Selection`, WS-08/WS-11.)*
- [x] Hiding/showing a viewport tab works on Windows (empty-bounds path). *(Self-test plus the product owner's hand check, including across a window resize, 2026-11-24.)*

## Design notes (WS-00 spike)
- **Layout.** `backend.rs` (seam, no GPUI, all platforms) · `platform_windows.rs` (the one `Any` downcast) · `element.rs` (GPUI element + native surface; Windows-only for now) · `spike/` (camera, CPU picking, 500-atom scene, impostor renderer: **spike code standing in for `mold-render`**, to be replaced, not grown) · `examples/spike.rs`.
- **The seam takes the handle as `Box<dyn Any>`**, not a GPUI type, so it compiles and is tested headless; the element calls `platform_surface()?.platform_handle()?` and passes it on. The wgpu instance must be created with `backend::wgpu_backends()` (DX12 on Windows).
- **GPUI is a `cfg(windows)` dependency** of this crate until a macOS/Linux backend exists. The Linux CI job then builds only the seam and the spike math, with no system packages. A new backend adds its target table and widens the `cfg` on `element`.
- **Pin guard.** `backend::GPUI_FAST_TESTED` must equal the `=` pin of `gpui-fast` in the root manifest (unit test). Bumping it means re-running `--self-test` (ADR-0002).
- **Overlay rule.** Only `deferred(..)` content paints above the surface; plain children land in GPUI's base layer, behind it.
- **Threads.** The UI thread only sends messages (pointer events carry the `Instant` they arrived); the render thread owns device, queue, camera and picking, and sends back `CommitComposition`, picks and stats. Events reach GPUI through a background task blocking on the channel, so the UI thread never waits on the renderer.
- **Commit.** After the first present following each `configure` (attach, resize, lost/outdated), the render thread asks the UI thread to call `Viewport::commit` (`set_parent_origin`). `set_bounds` alone does not commit while bounds hold still.
- **Hide.** `Viewport::set_visible(false)` sets empty bounds and tells the renderer to stop drawing and drop its depth target; the swap chain keeps the last frame. The caller also stops rendering the element.
- **Picking** is CPU ray-sphere in the spike (exact for impostors). `mold-render` owns real picking (GPU readback; sticks, ribbons).
- **Latency** is measured to `Queue::present` returning; scan-out adds one or two refreshes.
- **Not done in the spike:** tabs / reusing closed surfaces, device-lost recovery (`set_compositor_recreated_callback` exists in gpui-fast 0.1.19), MSAA, dismissing a GPUI Kit dropdown on a viewport click.
