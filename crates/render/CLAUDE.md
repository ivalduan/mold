# mold-render — workstream WS-08

Pure-`wgpu` molecular renderer. Everything above the `ViewportBackend` seam: camera, scene, pipelines, picking. Runs W1–W4, in parallel with the node canvas; they share no code.

## Scope
- Representations: **spheres, sticks** (GPU impostors: two triangles + fragment-shader ray intersection, not meshes), **cartoon ribbons**, **pocket surface**.
- Upload arena positions directly (contiguous `[f32; 3]`, no transform pass).
- Picking via GPU readback (desktop only; fine for v0.1).
- An offscreen target for tests and for the CLI.

## Rules
- Owns no window and no surface. `ui-viewport` hands it a `wgpu::Surface`/texture.
- Own device and render thread. A slow frame must never block GPUI's thread.
- Reference to read, not adopt: `mogura` (Bevy) for representation code; `molar_vis`.

## May depend on
`mold-mol`, `wgpu`, `bytemuck`, `glam`. **Never** GPUI (CI enforces this). Dev: `criterion`.

## Acceptance tests
- [x] Headless render test: draw a known fixture structure offscreen and compare a perceptual hash (`tests/golden.rs`: EGFR + gefitinib in spheres, ball & stick and sticks, 256-bit dHash + covered fraction vs committed goldens; a BMP for humans goes to `target/render-tests/`, never committed). Skips cleanly with no adapter (prints `SKIPPED <test>: no wgpu adapter: …` and passes); `MOLD_RENDER_REQUIRE_GPU=1` turns the skip into a failure for a future GPU CI job.
- [ ] Criterion: 500 poses of ~3,000 atoms in under 16.6 ms per frame with spheres, sticks and cartoon. *(`benches/frame.rs`, spheres + sticks only — cartoon is not built yet. Met side by side, not superimposed; see "Frame budget" below.)*
- [x] Pick round-trip returns the right `PoseId` + atom (`tests/pick.rs`: GPU id readback vs an exact CPU ray cast over a pixel grid — spheres 0 mismatches in 1,196 pixels, sticks 15 edge pixels in 3,207 hits; a known atom of the second of two poses → `PoseId(1)` + that atom; a hidden pose is not picked; outside the target is a miss).

## Design notes (WS-08, first PR)
- **Layout.** `gpu.rs` (`Gpu`: device + queue, `Gpu::headless()` on the primary backends, or `from_instance`) · `camera.rs` (`OrbitCamera`: orbit, pan, zoom, `fit`, `re_aim`, `ray_through`, `project`; `BoundingSphere`) · `style.rs` (`Representation`, vdW / covalent radii, CPK colours) · `bonds.rs` (`guess_bonds`) · `scene.rs` (`Scene`: the arena on the GPU, per-pose visibility, draw runs) · `renderer.rs` (`Renderer`, `FrameTargets`, `OffscreenTarget`, `Image`, `pick`) · `impostor.wgsl` · `ray.rs` (CPU ray-sphere oracle) · `hash.rs` (dHash) · `host.rs` (`RenderHost`: the viewport's device + render thread, moved here from the ui-viewport spike).
- **Upload.** Positions go up as `arena.positions_flat()`, unchanged, as a storage buffer read in the vertex shader (`array<f32>`, index × 3: `vec3` arrays would need 16-byte stride, i.e. a transform pass). Elements go up as one byte per atom (packed four to a `u32`); colour and vdW radius come from a 256-entry uniform table indexed by atomic number, so recolouring or changing a representation re-uploads nothing per atom. The scene keeps **no CPU copy of coordinates** (they live only in the arena); `Scene::update_positions` re-uploads after an in-place change.
- **Impostors.** Spheres: one instanced quad per atom, in front of the centre and grown for perspective (the spike's), ray-sphere per fragment, exact `frag_depth`. Sticks: one instanced quad per bond in **screen space** around the projected segment, padded by a conservative projected radius; the fragment builds its view ray from `@builtin(position)` and intersects an open cylinder (iq's formula); each half takes its atom's colour and id; the atom spheres are the joints. `Representation::{SpaceFilling, BallAndStick, Sticks}` only changes three uniforms. Reversed Z, `Depth32Float`, infinite far plane.
- **Draw ranges.** `instance_index` *is* the global atom index (first instance = run start) and the bond index for sticks. Bonds are sorted by first atom, so a pose's bonds are one range; consecutive visible poses merge into one draw call (all visible = 2 draw calls total). Bonds that cross poses are rejected (`SceneError::BondCrossesPoses`).
- **Picking.** The pass writes a second target, `R32Uint` = atom index + 1, stored after the pass. `pick()` copies one texel to a 256-byte buffer and blocks on the map (render thread only). `Scene::pose_of_atom` maps the atom to its `PoseId` by binary search over pose ranges. No per-pick redraw: the id target is the last frame's, and the host draws first if the camera changed since.
- **Bonds.** `guess_bonds` is a stop-gap: distance only (`0.4 Å ≤ d ≤ r_cov(a) + r_cov(b) + 0.45 Å`, no H–H), grid-hashed per pose. **Bond perception belongs in `mold-mol`** (struct_conn / CCD / SDF bonds, distance fallback); the renderer takes plain `&[[u32; 2]]` and will not care where they come from.
- **Golden hashes.** A 256-bit dHash (17×16 box-filtered luma cells, with a 1.5-luma margin so flat background stays 0) at 320×240, tolerance 12 bits, **plus the covered fraction** (±1.5 points): the dHash sees the silhouette, which all three representations share; coverage (23% / 12% / 7%) is what tells them apart. Recorded on the dev box with Vulkan; the same goldens pass on DX12. Re-record only after a deliberate change to the picture, and look at the BMP first.
- **Frame budget** (`cargo bench -p mold-render --bench frame`, `--release`, RTX 5060 Laptop, 1920×1080 offscreen, submit + wait; 500 copies of `fixtures/egfr/reference.cif` = 1,174,000 atoms, 1,137,075 guessed bonds; spheres + sticks, **no cartoon**):

  | Layout | Representation | Vulkan | DX12 (`WGPU_BACKEND=dx12`, the viewport's) |
  |---|---|---|---|
  | 25 × 20 grid, side by side | ball & stick | 3.1 ms | 5.1 ms |
  | 25 × 20 grid, side by side | space-filling | 1.8 ms | 2.3 ms |
  | superimposed (one receptor frame, ±0.5 Å) | ball & stick | 28 ms | 42 ms |
  | superimposed | space-filling | 148 ms | 172 ms |

  Side by side is far inside 16.6 ms. **Superimposed full complexes are fill-bound** and miss it: every impostor writes `frag_depth`, which turns off early-Z, so 500 overlapping receptors shade every covered fragment 500× over. Options, in order: draw a shared receptor once and only the ligands per pose (what a pose viewer shows anyway); conservative depth (`@early_depth_test(less_equal)`, quad placed in front of the surface) — **Vulkan/GLES only in wgpu 30, not DX12**, so it would not help the Windows viewport; a depth pre-pass with a cheap shader; per-pose LOD.
- **Headless backends.** `Gpu::headless()` uses `WGPU_BACKEND` if set, else `Backends::PRIMARY` (no GL; on Windows wgpu then picks Vulkan): CI runners then have no adapter and the GPU tests skip. Adapters without storage buffers in the vertex stage are refused with `GpuError::Unsupported` (also a skip).
- **Not in this PR (later):** cartoon ribbons, pocket surface, MSAA, device-lost recovery, per-pose representations/colours, `Selection` wiring (WS-10/11), conservative depth (`@early_depth_test`) to keep early-Z with `frag_depth`, sticks whose end is behind the eye are dropped (their spheres stay).
