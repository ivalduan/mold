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
`mold-mol`, `wgpu`, `bytemuck`, `glam`. **Never** GPUI (CI enforces this).

## Acceptance tests
- [ ] Headless render test: draw a known fixture structure to PNG and compare a perceptual hash (runs in the optional GPU CI job; must skip cleanly with no adapter).
- [ ] Criterion: 500 poses of ~3,000 atoms in under 16.6 ms per frame with spheres, sticks and cartoon.
- [ ] Pick round-trip returns the right `PoseId` + atom.
