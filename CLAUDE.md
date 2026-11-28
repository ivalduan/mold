# Mold — agent guide

Mold is an open-source, Rust-powered, node-based workbench for structural biology (v0.1), growing into a hub for chemistry and bioinformatics. Read [docs/mission.md](docs/mission.md) once, [docs/architecture.md](docs/architecture.md) before touching code, and the `CLAUDE.md` of the crate you are working in.

## You are probably in a worktree
Each workstream runs in its own git worktree on branch `ws/<name>` ([docs/worktrees.md](docs/worktrees.md)). Find your workstream in [docs/workstreams.md](docs/workstreams.md). **Stay inside the crates your workstream owns.** If you need a change in another crate, stop and say so; don't make it.

If you are the **orchestrator** in the main checkout (briefing agents, reviewing and landing their PRs), follow [docs/orchestration.md](docs/orchestration.md) and check the status table in [docs/workstreams.md](docs/workstreams.md#status-2026-11-28).

## Non-negotiables (product principles)
1. **Hub, not rewrite.** Rust builds the hub; chemistry/physics come from wrapped engines (RDKit via `rdkit-sys`, OpenMM, Boltz-2, Vina).
2. **Graph ⇄ text, both ways.** Every graph serializes to a readable TOML recipe that diffs in Git and replays headless.
3. **Every intermediate is visible.** Any node's output can be inspected; non-destructive by default.
4. **Local-first, no IP leakage.** Proprietary structures never have to leave the user's network. Anything that sends data out (e.g. MSA query) says so in the UI first.
5. **Thin core, plugin edges.** Python/CUDA models are worker processes behind a stable protocol.
6. **One codebase, native + web** (web is out of scope for v0.1).
7. **Open source, all of it.** Apache-2.0.

## Architecture rules (CI enforces the first)
- **No crate below the UI band depends on GPUI**: `mol`, `chem`, `geom`, `render`, `graph`, `nodes`, `jobs`, `recipe`, `cli` build and test headless. `scripts/check-layering.sh` fails the build otherwise.
- **Rust owns the interactive loop** (parsing, geometry, filtering, rendering). **Python owns anything with a neural network or force field**, always in a separate process. Nothing that loads a GPU model is linked into a binary.
- GPL tools (e.g. gnina) are only ever invoked as subprocesses. Record every third-party dependency in [THIRD_PARTY.md](THIRD_PARTY.md) with "linked" or "subprocess".
- Coordinates live only in the `mol` arena; metrics live only in the pose table. One `Selection` value, owned by no panel.
- **No Git LFS** ([ADR-0012](adr/0012-fixture-data-storage.md)). Test data a test reads is committed under `fixtures/`, ≤5 MB per file; anything larger or generated goes behind a script in `fixtures/scripts/` into git-ignored `fixtures/cache/`, with its SHA-256 in `fixtures/checksums.sha256`. Re-run `uv run fixtures/fetch.py` after editing a fixture.

## Contract types: change on `main` first
These are shared across worktrees. Change them in a small PR to `main`, merge, then rebase your worktree:
- `mold_mol::{PoseId, Selection, pose_columns}` ([crates/mol/src](crates/mol/src))
- `mold_graph::PortType`, the node traits `Processor`/`Partitioner` ([crates/graph/src/node.rs](crates/graph/src/node.rs)) and the `Executor` seam ([crates/graph/src/exec.rs](crates/graph/src/exec.rs)); see [crates/graph/DESIGN.md](crates/graph/DESIGN.md)
- `mold_jobs::protocol` + [docs/worker-protocol.md](docs/worker-protocol.md)
- Root `Cargo.toml` `[workspace.dependencies]`: add a dependency in its own commit, with its THIRD_PARTY.md line

## How to work
- **Tests first.** Each crate's `CLAUDE.md` lists acceptance tests; write the test, then the implementation.
- **Build bottom-up.** `mol`, `geom`, `graph` before any UI.
- **Mock before real.** Use `workers/mock-cofold`; never block on a GPU, CUDA driver or Python env.
- ~~**Week 0 produces ADRs, not code.** Don't build the app shell before ADR-0002/0003 are accepted.~~ **Week 0 is done**: [ADR-0002](adr/0002-gpui-lineage.md) and [ADR-0003](adr/0003-viewport-path.md) were accepted 2026-11-24, so the UI band (`ui-graph`, `ui-panels`, `app`) is open. Anything that must paint over the viewport uses `deferred(..)` — a plain child lands behind the swap chain.
- A decision that forks the design → write an ADR in [adr/](adr/) (copy `0000-template.md`).

## Commands
```sh
cargo test --workspace                       # must pass with no GPU, no Python, no RDKit
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
bash scripts/check-layering.sh
cargo test -p mold-chem --features rdkit     # only where RDKit is installed (ADR-0006)
```
On this Windows machine cargo is at `%USERPROFILE%\.cargo\bin` and may not be on the Git Bash PATH: `export PATH="$HOME/.cargo/bin:$PATH"`.

## Performance budget (tests should be able to fail on these)
16.6 ms frame at 500 poses × ~3,000 atoms, superimposed over a shared receptor (decided 2026-11-27; full complexes on demand) · <100 ms click → re-aimed camera · <200 ms to gate 500 poses · <2 s to load and show 300 ligands from cache.
