# Third-party software

The interaction mode is doing real work here: GPL tools are acceptable only as **subprocesses**, never **linked** into our Apache-2.0 binaries (ADR-0008). Add a row in the same commit that adds the dependency. Licences marked _verify_ came from planning notes and must be checked against the upstream repo before release.

| Component | Used by | Licence | Mode | Status |
|---|---|---|---|---|
| serde, serde_json | graph, jobs | MIT OR Apache-2.0 | linked | in use |
| blake3 | graph (content-hash cache keys) | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | linked | in use |
| GPUI Kit 0.11.0 (`gpui-kit`, feature `gpui-fast`) → GPUI Fast 0.1.19 (`gpui-fast*`), GPUI snapshot `gpui-pre*` =0.3.14, `gpui-base`, `gpui-component` | ui-* , app | Apache-2.0 (verified: crates.io metadata + upstream LICENSE-APACHE in longbridge/gpui-kit and longbridge/gpui-fast) | linked | in use (ui-viewport, Windows only); pinned by ADR-0002 |
| wgpu 30.0.1 | render, ui-viewport | MIT OR Apache-2.0 (verified: crates.io metadata) | linked | in use (ui-viewport); exact pin matches gpui-fast-wgpu |
| bytemuck, glam 0.34 | render, ui-viewport | bytemuck: Zlib OR Apache-2.0 OR MIT; glam: MIT OR Apache-2.0 (verified: crates.io metadata) | linked | in use (ui-viewport) |
| windows 0.62 (Microsoft windows-rs) | ui-viewport (Windows) | MIT OR Apache-2.0 (verified: crates.io metadata) | linked | in use; must match gpui-fast-windows |
| pdbtbx 0.12 | mol | MIT (verified: crates.io metadata + upstream LICENSE) | linked | in use |
| polars 0.53 | mol, nodes, ui-panels | MIT (verified: crates.io metadata + upstream LICENSE) | linked | in use (mol); held below 0.54, which links tokio |
| arrow | nodes, ui-panels | Apache-2.0 (_verify_) | linked | planned |
| kiddo 6.3 (default features off: no rayon) | geom | MIT OR Apache-2.0 (verified: crates.io metadata) | linked | in use |
| proptest 1.12 | geom (dev-dependency) | MIT OR Apache-2.0 (verified: crates.io metadata) | linked into tests only | in use |
| criterion 0.8 (default features off: no rayon, no plotters) | geom (dev-dependency) | Apache-2.0 OR MIT (verified: crates.io metadata) | linked into benches only | in use |
| rdkit-sys → RDKit | chem (feature `rdkit`) | BSD-3-Clause (_verify_) | linked (FFI) | planned (ADR-0006) |
| gpui-flow (pacifio/gpui-flow @ `cfa5787`), edge geometry only (`src/edges/bezier.rs`, a port of xyflow's `getBezierPath`) | ui-graph | MIT (verified: upstream `LICENSE`, kept in `crates/ui-graph/src/vendor/gpui_flow/`) | vendored snippet (one function, compiled in; upstream not tracked) | in use; ADR-0004 (accepted: our own canvas) |
| gpui-plot | ui-panels | _verify_ | linked | candidate |
| Boltz-2 | workers/boltz2 | MIT | subprocess | planned |
| AutoDock Vina | workers/vina | Apache-2.0 | subprocess | planned |
| OpenMM | worker (optional) | MIT / LGPL (_verify_) | subprocess | optional |
| gnina | not used | GPL | subprocess only, if ever | excluded from v0.1 |
| ColabFold MSA server | nodes (MSA Query) | network service | remote call | planned; sends sequence off-box |
| PoseBusters 0.6.5, RDKit, gemmi | `fixtures/scripts` only | BSD-3-Clause / BSD-3-Clause / MPL-2.0 | neither: offline fixture tooling, run via `uv` | in use (WS-13); not linked, not shipped, not in CI |

## Redistributed data

Not software, but shipped in this repo and licensed separately. Per-dataset
source, query and attribution are in [adr/0007-benchmark-target.md](adr/0007-benchmark-target.md)
and the `README.md` beside each dataset.

| Dataset | Where | Licence | Attribution required |
|---|---|---|---|
| PDB entries (1M17, 4G5J, 1EJG, 4WKQ), derived receptor/reference/pose files | `fixtures/structures`, `fixtures/egfr`, `fixtures/posebusters/inputs` | CC0 1.0 (wwPDB) | no |
| EGFR 4-anilinoquinazoline series (`ligands.smi`) | `fixtures/egfr` | CC BY-SA 3.0 | **yes**: PubChem (NIH/NLM/NCBI) and ChEMBL (EMBL-EBI) |
