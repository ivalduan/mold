# Mold

**Codename.** An open-source, local-first, node-based workbench for structural biology, built in Rust. v0.1 proves one idea end to end: batch co-folding triage of protein–ligand poses with linked 3D, table and plot views, where every visual graph is also a Git-diffable TOML recipe that replays headless.

> Status: pre-v0.1. Week 0 (spike + ADRs) has not started. Nothing runs yet.

## Read in this order
1. [docs/mission.md](docs/mission.md): why Mold exists, principles, what we're not.
2. [docs/architecture.md](docs/architecture.md): crate layers, data model, execution engine, node set.
3. [docs/v0.1-plan.md](docs/v0.1-plan.md): definition of done, milestones, budgets, open decisions.
4. [docs/workstreams.md](docs/workstreams.md): the independent work items and their order.
5. [docs/worktrees.md](docs/worktrees.md): how we develop them in parallel.
6. [adr/](adr/): decisions, numbered.

## Layout
```
crates/
  mol  chem  geom  render            domain core: no GPUI, no async
  graph  nodes  jobs  recipe  cli    orchestration: no GPUI, testable headless
  app  ui-graph  ui-viewport  ui-panels   UI: the only crates that see GPUI Kit
workers/
  mock-cofold/   Rust fixture-replay worker (no GPU, no Python)
  boltz2/  vina/ Python workers, separate processes, never linked
fixtures/        frozen public benchmark (receptor + ligand series)
adr/             architecture decision records
docs/            plans and specs
scripts/         layering check, worktree helpers
```

## Build
```sh
cargo test --workspace     # no GPU, no Python, no RDKit required
```

## Licence
Apache-2.0. Contributions under DCO sign-off; see [CONTRIBUTING.md](CONTRIBUTING.md).
