# Contributing to Mold

## Sign-off (DCO), not a CLA
Every commit carries a `Signed-off-by:` line (`git commit -s`), certifying the [Developer Certificate of Origin](https://developercertificate.org/). No CLA: lower friction for academic contributors.

## Running the demo without reading Rust
_To be written by WS-11 before v0.1. Target: fresh clone to running demo in under 10 minutes, one command, on a workstation with an NVIDIA GPU._

## Developer setup
- Rust stable (pinned by `rust-toolchain.toml`).
- **Windows is the v0.1 development platform.** Co-folding workers run under **WSL2** (ADR-0005).
- RDKit for `--features rdkit`: steps to be recorded in ADR-0006 after the week-0 smoke test (vcpkg or conda-forge on Windows).

## Before you push
```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/check-layering.sh
```

## Pull requests
- One workstream per branch (`ws/<name>`); see [docs/worktrees.md](docs/worktrees.md).
- Contract changes (see root `CLAUDE.md`) go in their own small PR.
- New third-party dependency → add a line to [THIRD_PARTY.md](THIRD_PARTY.md) with its licence and whether it is **linked** or a **subprocess**.
- A design fork → an ADR in [adr/](adr/).
