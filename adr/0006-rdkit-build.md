# 0006. RDKit via rdkit-sys, feature-gated

- Status: Accepted (build steps pending week-0 smoke test)
- Workstream: WS-02 chem

## Context
Ligand chemistry (SMILES, stereo, standardization, 2D depiction) needs RDKit-grade correctness. Pure-Rust chemistry hit a "chemistry wall" on aromaticity, ring perception, SMARTS and CIP stereo.

## Decision
Use the C++ bindings via `rdkit-sys`, not a pure-Rust toolkit. All RDKit code sits behind the `rdkit` Cargo feature of `mold-chem` (forwarded by `mold-nodes` and `mold-app`), so the default build and CI (no RDKit installed) still compile and test.

## To record after the week-0 smoke test
- [ ] Windows: vcpkg or conda-forge; exact commands, env vars, versions.
- [ ] Linux: package source and version.
- [ ] Result of parsing the fixture ligand set.

## Consequences
Two build configurations to keep green. The demo build (`--features rdkit`) needs a documented one-command setup for the "clone to demo in 10 minutes" target.
