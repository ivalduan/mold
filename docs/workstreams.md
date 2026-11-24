# Workstreams

Each workstream runs in its own worktree on branch `ws/<name>` (`scripts/new-worktree.ps1 <name>`). It **owns** the listed paths and edits nothing else. Its brief is the `CLAUDE.md` in the crate it owns; the sections below cover what doesn't live in a crate.

| ID | Name (branch `ws/…`) | Owns | Needs from others (contract) | Can start | Brief |
|---|---|---|---|---|---|
| WS-00 | `spike` | `adr/0002`, `adr/0003`, `crates/ui-viewport` (spike code) | none | **now** | [ui-viewport/CLAUDE.md](../crates/ui-viewport/CLAUDE.md) |
| WS-13 | `fixtures` | `fixtures/`, `adr/0007` | none | **now** | [below](#fixtures) |
| WS-01 | `mol` | `crates/mol` | none | **now** | [mol/CLAUDE.md](../crates/mol/CLAUDE.md) |
| WS-02 | `chem` | `crates/chem`, `adr/0006` | `mol` arena (later) | **now** (build smoke test) | [chem/CLAUDE.md](../crates/chem/CLAUDE.md) |
| WS-03 | `geom` | `crates/geom` | `mol` arena read API | **now** (on raw slices) | [geom/CLAUDE.md](../crates/geom/CLAUDE.md) |
| WS-04 | `graph` | `crates/graph` | `PortType` (on main) | **now** | [graph/CLAUDE.md](../crates/graph/CLAUDE.md) |
| WS-05 | `jobs` | `crates/jobs`, `workers/mock-cofold`, `docs/worker-protocol.md` | protocol (on main) | **now** | [jobs/CLAUDE.md](../crates/jobs/CLAUDE.md) |
| WS-08 | `render` | `crates/render` | `mol` arena positions | W1 | [render/CLAUDE.md](../crates/render/CLAUDE.md) |
| WS-06 | `recipe` | `crates/recipe`, `crates/cli` | `graph` model + node trait | W2 (after WS-04 lands its model) | [recipe/CLAUDE.md](../crates/recipe/CLAUDE.md) |
| WS-09 | `ui-graph` | `crates/ui-graph`, `adr/0004` | ADR-0002; `graph` model | W2 | [ui-graph/CLAUDE.md](../crates/ui-graph/CLAUDE.md) |
| WS-12 | `workers-py` | `workers/boltz2`, `workers/vina` | protocol; ADR-0005 | W3 (after mock works) | [below](#workers-py) |
| WS-07 | `nodes` | `crates/nodes`, `docs/authoring-a-node.md` | node trait, `mol`, `chem`, `geom`, `jobs` | W3 | [nodes/CLAUDE.md](../crates/nodes/CLAUDE.md) |
| WS-10 | `ui-panels` | `crates/ui-panels` | ADR-0002; pose table; `Selection` | W4 | [ui-panels/CLAUDE.md](../crates/ui-panels/CLAUDE.md) |
| WS-11 | `app` | `crates/app`, `CONTRIBUTING.md` (demo section) | all UI crates, `nodes`, `recipe` | W4 | [app/CLAUDE.md](../crates/app/CLAUDE.md) |

## Dependency order

```mermaid
flowchart LR
  spike[WS-00 spike<br/>ADR-0002/0003]
  fix[WS-13 fixtures]
  mol[WS-01 mol]; chem[WS-02 chem]; geom[WS-03 geom]
  graph[WS-04 graph]; jobs[WS-05 jobs + mock]
  render[WS-08 render]; recipe[WS-06 recipe + cli]
  uig[WS-09 ui-graph]; py[WS-12 workers-py]
  nodes[WS-07 nodes]; uip[WS-10 ui-panels]; app[WS-11 app]

  mol --> geom & chem & render & nodes & uip
  fix --> geom & nodes
  graph --> recipe & uig & nodes
  jobs --> nodes & py
  chem --> nodes
  geom --> nodes
  spike --> uig & uip & app
  render --> app
  nodes --> app
  recipe --> app
  uig --> app
  uip --> app
```

**Wave 1 (start today, fully parallel):** `spike`, `fixtures`, `mol`, `chem`, `graph`, `jobs`, `geom`. Seven worktrees, no shared files except the contract types already on `main`.

**Critical path:** `mol` + `graph` + `jobs` → `nodes` → first pose streamed (W4 gate) → `ui-panels` + `app` → demo (W6).

## How to unblock early
A workstream that needs another's API before it lands: code against the contract types on `main`, and against a local trait/fake for anything not yet there. Don't reach into the other crate. When the real API merges, rebase and delete the fake.

The first PR of `mol`, `graph` and `jobs` should be **their public API with tests**, merged fast, so downstream streams can rebase onto it. Implementation follows in later PRs.

---

## fixtures
**WS-13.** Freeze the benchmark in week 1; choosing the dataset late is a common way to lose a week. Every acceptance test runs against it.
- Pick one public congeneric series on a well-characterised target (default: EGFR kinase), record it in ADR-0007 with sources and licences.
- Commit: receptor mmCIF, a reference crystal for pocket RMSD, 200–500 ligand SMILES (include a few that fail to parse on purpose), a small `mock-cofold` replay set of real poses with timings, and the PoseBusters reference CSV for `geom`'s cross-validation.
- Plus parser golden files: one structure with altlocs, one with a covalent ligand.
- Keep the repo light: large sets go behind a fetch script with checksums; the CI subset is committed.

## workers-py
**WS-12.** Python workers under `workers/boltz2` and `workers/vina`, speaking [the protocol](worker-protocol.md). Only after `mock-cofold` works end to end. Boltz-2 runs in WSL2 on the Windows dev box (ADR-0005). Never installs environments; the manifest declares them.
