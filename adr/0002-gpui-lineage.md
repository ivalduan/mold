# 0002. GPUI lineage to pin

- Status: **Accepted** 2026-11-24, together with ADR-0003
- Date: 2026-11-24
- Deciders: product owner (accepts); WS-00 spike (proposes)
- Workstream: WS-00 spike

## Context
Every UI crate compiles against GPUI. GPUI Kit is a single dependency that pins a matching GPUI release and re-exports the component layer (dock layout with resizable panels, virtualized tables, themes), with the `gpui-fast` backend as an additive Cargo feature. The viewport reference (`javagg/gpui-fast-wgpu-demo`) uses gpui-fast's public composition API.

Default if nobody decides: `gpui-kit` with the `gpui-fast` feature.

What the spike found (2026-11-24):

- **The reference demo does not build against GPUI Kit.** `javagg/gpui-fast-wgpu-demo` (checked at `b0de647`) vendors `longbridge/gpui-fast` as a git submodule at **`86a8228229c8646b552d747434ffd690216a872c`** (2026-11-05, "render directly into WGPU dynamic textures (#99)", between gpui-fast v0.1.13 and v0.1.14), uses path dependencies on `vendor/gpui-fast/crates/{gpui,gpui_platform}`, wgpu from git (`cddb2f5d`), and repeats gpui-fast's `[patch.crates-io]` entries. A fork-shaped setup we don't want to inherit.
- **GPUI Kit 0.11.0** (crates.io, 2026-11-23) with feature `gpui-fast` depends on the *published* GPUI Fast crates `gpui-fast*` **0.1.19** (= gpui-fast tag `v0.1.19`, `fd15f82cd04839f1fd5e00111dd0cd9c9fe6468a`, 2026-11-23). `gpui_kit::*` then re-exports `gpui_fast::*`, and `gpui_kit::platform` is `gpui_fast_platform`. No `[patch]`, no git dependencies.
- gpui-fast `86a8228..v0.1.19` is 131 commits. For the API the viewport uses (`Window::enable_window_composition`, `WindowComposition::{create_native_surface, on_input, remove_surface}`, `WindowCompositionSurface::platform_surface`, `PlatformSurfaceAttachment::{set_bounds, set_parent_origin, set_visible, platform_handle}`, and `gpui_windows/src/fast/composition.rs`) the diff has unchanged signatures and additions only (a placement counter, native views, compositor-recreated callbacks). On Windows `platform_handle()` still returns `Box<dyn Any>` holding a `windows::core::IUnknown` (windows 0.62) cast from the `IDCompositionVisual`; `set_visible` still casts to `IDCompositionVisual3` (E_NOINTERFACE); `set_parent_origin` still re-applies the bounds through the platform attachment, which commits.
- The Kit always compiles the upstream GPUI snapshot too: `gpui-pre*` `=0.3.14` are non-optional dependencies of `gpui-kit`, and `gpui-fast` is additive. Cost: two GPUIs in the build (a clean release build of the UI dependency tree took ~4 min on the dev box), one in use.
- `wgpu`: GPUI Fast's published Linux renderer (`gpui-fast-wgpu` 0.1.19) pins `wgpu =30.0.1`. On Windows GPUI renders with Direct3D 11 and links no wgpu; the viewport's own wgpu device (DX12) is independent of it.
- The untyped-handle downcast only works if our `windows` crate resolves to the same `windows-core` as `gpui-fast-windows`. Today both are `windows-core 0.62.2`, shared with `wgpu-hal` and `accesskit_windows`. `Cargo.lock` holds other `windows-core` majors as well (0.57, 0.58 under `gpui-base`, 0.61.2), which is harmless: only the copy on the `gpui-fast-windows` ⇄ `mold-ui-viewport` path has to match, and `cargo tree -i windows-core:0.62.2` is what verifies it after an upgrade.

## Options
1. **GPUI Kit 0.11.0 + `gpui-fast` feature from crates.io** (the default).
2. gpui-fast as a git submodule / path dependency at the demo's revision `86a8228`, as the demo does.
3. Upstream Zed GPUI (crates.io `gpui` 0.2.x or the Kit without `gpui-fast`): no window composition, so no native surface for the viewport. Ruled out by ADR-0003's premise.

## Decision
Option 1, pinned exactly:

| What | Pin | Where |
|---|---|---|
| `gpui-kit` | `=0.11.0`, `default-features = false`, `features = ["gpui-fast"]` | root `[workspace.dependencies]` |
| `gpui-fast` (and through it `gpui-fast-platform`, `-windows`, `-macros`, `-scheduler`, …) | `=0.1.19` = git `fd15f82` | root `[workspace.dependencies]` (explicit, although the Kit requires `^0.1.19`) + `Cargo.lock` |
| GPUI snapshot under the Kit | `gpui-pre*` `=0.3.14` | pinned by `gpui-kit` itself |
| `gpui-component`, `gpui-base` | 0.11.0 | `Cargo.lock`; members opt into `component` |
| `wgpu` | `=30.0.1` | root `[workspace.dependencies]`; what gpui-fast-wgpu pins, so Linux gets one wgpu |
| `windows` | `0.62` (lock: 0.62.2, `windows-core` 0.62.2) | root `[workspace.dependencies]`, Windows only |

**The `platform_handle()` downcast was tested against gpui-fast 0.1.19 (`fd15f82`) on Windows 11** with `cargo run -p mold-ui-viewport --example spike --release -- --self-test` (results in ADR-0003). Since WS-08 replaced the spike, re-run it as `cargo run -p mold-ui-viewport --example viewport --release -- --self-test`. The demo was tested against `86a8228`; we depend on neither that revision nor the demo. `mold_ui_viewport::backend::GPUI_FAST_TESTED` records the tested version; a unit test fails if it drifts from the `=` pin, and a wrong handle type fails at startup with a message naming the tested version and this ADR.

Members choose Kit layers themselves: the workspace entry has no default features (no `component`, `assets` or `x11`); `ui-viewport`'s example adds `component`. `app` decides `assets` and `x11` when it lands (feature unification then applies to every UI crate in that build).

## Upgrade policy
- Never through `cargo update` alone: the `=` pins make a bump a deliberate edit of the root `Cargo.toml`, in its own PR with `Cargo.lock`, never mixed with feature work.
- Bump `gpui-kit` and `gpui-fast` together, to the pair the Kit release names (the Kit's own `Cargo.toml` lists the `gpui-fast` version it was published with).
- A bump PR must (1) read the diff of `crates/gpui/src/fast/composition.rs` and `crates/gpui_windows/src/fast/composition.rs` between the two gpui-fast tags; (2) run the spike's `--self-test` on Windows and the product-owner checklist in ADR-0003 (later: the app's viewport smoke test); (3) update `GPUI_FAST_TESTED` and the table above. A wrong downcast is a run-time failure, so step 2 is not optional.
- Keep `windows` on the minor `gpui-fast-windows` uses, and `wgpu` on the exact version `gpui-fast-wgpu` uses.
- Cadence: at most once per milestone, unless a fix we need lands upstream (e.g. the Windows `set_visible`/commit issues below). gpui-fast releases about weekly; we don't follow every release.
- Move to a git pin of gpui-fast only to pick up an unreleased fix, recorded here with the rev and the reason, and back to crates.io at the next release.

## Consequences
- Every UI crate depends on `gpui-kit = { workspace = true }`; nobody depends on `gpui`, `gpui-pre*` or `gpui_platform` directly.
- Building any UI crate compiles both the GPUI snapshot and GPUI Fast; CI time goes up accordingly.
- Linux builds of GPUI need system packages (X11/Wayland/xkbcommon/fontconfig; see ADR-0003, "CI"). `ui-viewport` gates GPUI to `cfg(windows)` until its macOS/Linux backends exist, so for now this ADR changes only the Windows CI job.
- Upstream issues to file with gpui-fast (from the demo's `docs/upstream-issues.md`, still present at v0.1.19): `set_visible` / `remove_surface` fail with E_NOINTERFACE on Windows; no public way to commit after an external producer presents.
- Transitive licences of the Windows tree were scanned (`cargo metadata`): all permissive except `option-ext` (MPL-2.0, file-level copyleft, fine to link). Rows in `THIRD_PARTY.md`.
