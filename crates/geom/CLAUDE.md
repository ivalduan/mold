# mold-geom — workstream WS-03

Spatial queries and validity checks. **Built, not wrapped**: geometric, cheap in Rust, and the actual product differentiator.

## Scope
- KD-tree over arena positions (`kiddo`).
- Clash detection, bond-length checks, contact profiling (H-bonds, salt bridges, π-stacking), pocket RMSD vs. a reference.
- PoseBusters-style physical validity checks: the kernels behind the Pose Gate node.

## May depend on
`mold-mol`, `kiddo`. **Never** GPUI, wgpu, async, Python.

## Acceptance tests
- [ ] Property tests: RMSD is invariant under rigid transform; clash counts are symmetric.
- [ ] Cross-validation of the gate against Python PoseBusters on a public set, with the tolerance written into the test. Run Python **offline** to produce a reference CSV committed to `fixtures/`; the Rust test only reads that CSV (CI has no Python).
- [ ] Criterion: gate 500 poses in under 200 ms.

## Note
Until `mol`'s arena API lands on `main`, develop kernels against plain `&[[f32; 3]]` slices. That's what the arena will hand you anyway.
