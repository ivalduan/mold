# Architecture decision records

One numbered file per fork in the road. Copy [0000-template.md](0000-template.md). Status: `Proposed` → `Accepted` (or `Rejected` / `Superseded by NNNN`). Never rewrite an accepted ADR; supersede it.

| # | Decision | Status | Owner |
|---|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted | n/a |
| [0002](0002-gpui-lineage.md) | GPUI lineage to pin | **Proposed (blocks week 0)** | WS-00 |
| [0003](0003-viewport-path.md) | Viewport path | **Proposed (W1 gate)** | WS-00 |
| [0004](0004-node-canvas-base.md) | Node canvas base: fork, don't depend | Proposed | WS-09 |
| [0005](0005-worker-host.md) | Worker host: WSL2 first, then SSH | Accepted | WS-05 |
| [0006](0006-rdkit-build.md) | RDKit via rdkit-sys, feature-gated | Accepted (build steps pending) | WS-02 |
| [0007](0007-benchmark-target.md) | Benchmark target and ligand series | Proposed | WS-13 |
| [0008](0008-process-boundary-licensing.md) | Solvers are subprocesses; licence boundary | Accepted | n/a |
