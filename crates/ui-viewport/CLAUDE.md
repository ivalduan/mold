# mold-ui-viewport — workstream WS-08 (was WS-00, the week-0 spike)

**Week 0 is done.** [ADR-0002](../../adr/0002-gpui-lineage.md) and [ADR-0003](../../adr/0003-viewport-path.md) are **Accepted** (2026-11-24): the lineage is pinned and the viewport path is proven on Windows. The app shell is no longer blocked — see [app/CLAUDE.md](../app/CLAUDE.md) (WS-11).

**The spike is gone.** `src/spike/` (camera, CPU picking, 500-atom scene, impostor renderer, render thread) was replaced by `mold-render` (WS-08): the render thread is `mold_render::host::RenderHost`, and the example is `examples/viewport.rs`. The seam (`backend.rs`), the element (`element.rs`) and the Windows backend (`platform_windows.rs`) stayed; only the element's imports changed.

## Reference
`javagg/gpui-fast-wgpu-demo`: a CAD viewport as a custom GPUI `Element` rendered by wgpu, using gpui-fast's public API, no fork. Its `src/viewport.rs` is the Windows path and the best-tested one.

## Week-0 spike tasks
- [x] Port the demo's `src/viewport.rs` (Windows): `src/element.rs` + `src/platform_windows.rs`.
- [x] Draw 500 impostor spheres through it (then `examples/spike.rs` + `src/spike/`; both replaced by `mold-render` in WS-08).
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
- [ ] Click a pose → camera re-aimed in under 100 ms. *(Click an atom → GPU pick → re-aimed frame presented: measured by `--self-test`, see Design notes; "a pose" from the plot/table needs the `Selection` bus, WS-10/WS-11.)*
- [x] Hiding/showing a viewport tab works on Windows (empty-bounds path). *(Self-test plus the product owner's hand check, including across a window resize, 2026-11-24.)*

## Design notes (WS-00 spike, adapted in WS-08)
- **Layout.** `backend.rs` (seam, no GPUI, all platforms) · `platform_windows.rs` (the one `Any` downcast) · `element.rs` (GPUI element + native surface; Windows-only for now) · `examples/viewport.rs`. Camera, scene, pipelines, picking and the render thread are `mold-render` (`mold_render::host`), which never sees GPUI.
- **The seam takes the handle as `Box<dyn Any>`**, not a GPUI type, so it compiles and is tested headless; the element calls `platform_surface()?.platform_handle()?` and passes it on. The wgpu instance must be created with `backend::wgpu_backends()` (DX12 on Windows).
- **GPUI is a `cfg(windows)` dependency** of this crate until a macOS/Linux backend exists. The Linux CI job then builds only the seam, with no system packages. A new backend adds its target table and widens the `cfg` on `element`.
- **Pin guard.** `backend::GPUI_FAST_TESTED` must equal the `=` pin of `gpui-fast` in the root manifest (unit test). Bumping it means re-running `cargo run -p mold-ui-viewport --example viewport --release -- --self-test` (ADR-0002; it was `--example spike`).
- **Overlay rule.** Only `deferred(..)` content paints above the surface; plain children land in GPUI's base layer, behind it.
- **Threads.** The UI thread only sends messages (pointer events carry the `Instant` they arrived); the render thread owns device, queue, camera and picking, and sends back `CommitComposition`, picks and stats. Events reach GPUI through a background task blocking on the channel, so the UI thread never waits on the renderer.
- **Commit.** After the first present following each `configure` (attach, resize, lost/outdated), the render thread asks the UI thread to call `Viewport::commit` (`set_parent_origin`). `set_bounds` alone does not commit while bounds hold still.
- **Hide.** `Viewport::set_visible(false)` sets empty bounds and tells the renderer to stop drawing and drop its depth target; the swap chain keeps the last frame. The caller also stops rendering the element.
- **Picking** is `mold-render`'s GPU readback: the frame writes an atom-id target; a click reads one texel (blocking the render thread only) → `(PoseId, atom)`, then re-aims on the atom's arena position. If the camera moved since the last frame, the host draws first so the id target matches what is on screen.
- **Latency** is measured to `Queue::present` returning; scan-out adds one or two refreshes. With `mold-render` (2026-11-27, RTX 5060 Laptop, DX12, two EGFR poses, 4,696 atoms): `--self-test` → SELF-TEST PASSED, 10/10 synthetic clicks picked an atom through the GPU id readback (0.5–6.7 ms) and re-aimed it to 0.00 px; click → re-aimed frame presented **min 21.5 / median 26.9 / max 36.3 ms** (budget 100 ms). Higher than the spike's ~1 ms because each synthetic click follows a `Fit`, so the host first presents an up-to-date frame for the id target, then the re-aimed one: two vsync waits (Fifo). A real click on a still camera skips the first.
- **Wiring.** The UI side creates `RenderHost::new(backend::wgpu_backends(), events)`, sends `Msg::LoadScene { arena: Arc<Arena>, bonds }` (bonds from `mold_render::guess_bonds` until `mold-mol` has bonds), then `Viewport::new(window, backend, &host)`. Representation, pose visibility and fit are `Msg`s.
- **Not done yet:** tabs / reusing closed surfaces, device-lost recovery (`set_compositor_recreated_callback` exists in gpui-fast 0.1.19), MSAA, dismissing a GPUI Kit dropdown on a viewport click.
