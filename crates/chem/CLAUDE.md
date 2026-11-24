# mold-chem — workstream WS-02

Ligand chemistry: SMILES/SDF parsing, standardization, stereochemistry, tautomers, 2D depiction, single-conformer strain (RDKit UFF/MMFF).

**Decided:** wrap RDKit via `rdkit-sys` (C++ FFI), not a pure-Rust toolkit. See [ADR-0006](../../adr/0006-rdkit-build.md).

## Week-0 task (blocking)
Smoke-test the `rdkit-sys` build on the Windows dev box (vcpkg or conda-forge) **and** on Linux, against our actual ligand set in `fixtures/`. Write up the exact steps in ADR-0006 and `CONTRIBUTING.md`.

## Scope
- Ligand set type for the `LigandSet` port (parse 500 SMILES; a few will fail: report per-ligand errors, never abort the set).
- Standardize, assign stereo, enumerate/choose tautomer: these are the parameters users tweak.
- Strain energy in kcal/mol for one conformer (feeds `strain_kcal`).

## Rules
- All RDKit code sits behind `#[cfg(feature = "rdkit")]`. Without the feature the crate still compiles and exposes the types, returning a clear "built without RDKit" error.
- No GPU model, no force-field *simulation*: OpenMM is a worker, not a dependency.

## May depend on
`mold-mol`, `rdkit-sys` (feature-gated). **Never** GPUI, wgpu, async.

## Acceptance tests
- [ ] `cargo test -p mold-chem` passes with no RDKit installed.
- [ ] With `--features rdkit`: parse the fixture ligand series; the known-bad SMILES are reported, the rest parse.
- [ ] Stereo and standardization are deterministic (same canonical SMILES across runs and platforms).
