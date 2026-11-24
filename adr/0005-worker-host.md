# 0005. Worker host: WSL2 first, then SSH

- Status: Accepted
- Workstream: WS-05 jobs, WS-12 workers-py

## Context
Windows is the dev platform. The UI side is fine there; the scientific side is not: RDKit's C++ build is harder, gnina is effectively Linux-only, and most co-folding/docking pipelines assume POSIX. Boltz-2 fits a desktop GPU (~11 GB VRAM structure, 7–8 GB affinity; 24 GB workable minimum).

## Decision
Both, in this order:
1. **WSL2** hosts the co-folding worker on the Windows dev box, reached through the ordinary `LocalProcess` backend. No second code path.
2. **SSH** follows, pointing at a shared Linux GPU box (48–96 GB VRAM).

## Consequences
The job spec and event stream must be identical across both, which is the test that the protocol is right. Paths in events are relative to `run_dir` so WSL/Windows path translation happens in one place (the backend).
