# Third-party software

The interaction mode is doing real work here: GPL tools are acceptable only as **subprocesses**, never **linked** into our Apache-2.0 binaries (ADR-0008). Add a row in the same commit that adds the dependency. Licences marked _verify_ came from planning notes and must be checked against the upstream repo before release.

| Component | Used by | Licence | Mode | Status |
|---|---|---|---|---|
| serde, serde_json | graph, jobs | MIT OR Apache-2.0 | linked | in use |
| blake3 | graph (content-hash cache keys) | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | linked | in use |
| GPUI, GPUI Kit (gpui-fast feature) | ui-* , app | Apache-2.0 | linked | planned (ADR-0002) |
| wgpu | render | MIT OR Apache-2.0 | linked | planned |
| pdbtbx 0.12 | mol | MIT (verified: crates.io metadata + upstream LICENSE) | linked | in use |
| polars 0.53 | mol, nodes, ui-panels | MIT (verified: crates.io metadata + upstream LICENSE) | linked | in use (mol); held below 0.54, which links tokio |
| arrow | nodes, ui-panels | Apache-2.0 (_verify_) | linked | planned |
| kiddo | geom | MIT OR Apache-2.0 (_verify_) | linked | planned |
| rdkit-sys → RDKit | chem (feature `rdkit`) | BSD-3-Clause (_verify_) | linked (FFI) | planned (ADR-0006) |
| gpui-flow (fork) | ui-graph | _verify_ | linked | candidate (ADR-0004) |
| gpui-plot | ui-panels | _verify_ | linked | candidate |
| Boltz-2 | workers/boltz2 | MIT | subprocess | planned |
| AutoDock Vina | workers/vina | Apache-2.0 | subprocess | planned |
| OpenMM | worker (optional) | MIT / LGPL (_verify_) | subprocess | optional |
| gnina | not used | GPL | subprocess only, if ever | excluded from v0.1 |
| ColabFold MSA server | nodes (MSA Query) | network service | remote call | planned; sends sequence off-box |
