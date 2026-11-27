# Workstreams

Each workstream runs in its own worktree on branch `ws/<name>` (`scripts/new-worktree.ps1 <name>`). It **owns** the listed paths and edits nothing else. Its brief is the `CLAUDE.md` in the crate it owns; the sections below cover what doesn't live in a crate.

| ID | Name (branch `ws/…`) | Owns | Needs from others (contract) | Can start | Brief |
|---|---|---|---|---|---|
| WS-00 | `spike` | `adr/0002`, `adr/0003`, `crates/ui-viewport` (spike code) | none | **done** — ADRs accepted 2026-11-24 | [ui-viewport/CLAUDE.md](../crates/ui-viewport/CLAUDE.md) |
| WS-13 | `fixtures` | `fixtures/`, `adr/0007` | none | **now** | [below](#fixtures) |
| WS-01 | `mol` | `crates/mol` | none | **now** | [mol/CLAUDE.md](../crates/mol/CLAUDE.md) |
| WS-02 | `chem` | `crates/chem`, `adr/0006` | `mol` arena (later) | **now** (build smoke test) | [chem/CLAUDE.md](../crates/chem/CLAUDE.md) |
| WS-03 | `geom` | `crates/geom` | `mol` arena read API | **in progress** — first PR merged (#9) | [geom/CLAUDE.md](../crates/geom/CLAUDE.md) |
| WS-04 | `graph` | `crates/graph` | `PortType` (on main) | **now** | [graph/CLAUDE.md](../crates/graph/CLAUDE.md) |
| WS-05 | `jobs` | `crates/jobs`, `workers/mock-cofold`, `docs/worker-protocol.md`, `adr/0009` | ADR-0009; executor seam from `graph` | **on hold** (ADR-0009 design first) | [jobs/CLAUDE.md](../crates/jobs/CLAUDE.md) |
| WS-08 | `render` | `crates/render` | `mol` arena positions | W1 | [render/CLAUDE.md](../crates/render/CLAUDE.md) |
| WS-06 | `recipe` | `crates/recipe`, `crates/cli` | `graph` model + node trait | W2 (after WS-04 lands its model) | [recipe/CLAUDE.md](../crates/recipe/CLAUDE.md) |
| WS-09 | `ui-graph` | `crates/ui-graph`, `adr/0004` | ADR-0002 ✅; `graph` model (on `main`) | **in progress** — first PR merged (#8); ADR-0004 awaiting acceptance | [ui-graph/CLAUDE.md](../crates/ui-graph/CLAUDE.md) |
| WS-12 | `workers-py` | `workers/boltz2`, `workers/vina` | protocol; ADR-0005 | W3 (after mock works) | [below](#workers-py) |
| WS-07 | `nodes` | `crates/nodes`, `docs/authoring-a-node.md` | node trait, `mol`, `chem`, `geom`, `jobs` | W3 | [nodes/CLAUDE.md](../crates/nodes/CLAUDE.md) |
| WS-10 | `ui-panels` | `crates/ui-panels` | ADR-0002 ✅; pose table; `Selection` | **now** for the table shell; pose data needs `nodes` | [ui-panels/CLAUDE.md](../crates/ui-panels/CLAUDE.md) |
| WS-11 | `app` | `crates/app`, `CONTRIBUTING.md` (demo section) | all UI crates, `nodes`, `recipe` | W4 (ADR-0002/0003 ✅, but still wants the UI crates) | [app/CLAUDE.md](../crates/app/CLAUDE.md) |

## Status (2026-11-27)
How these are run in parallel: [orchestration.md](orchestration.md).

| Workstream | State | Next |
|---|---|---|
| WS-04 `graph` | First PR merged ([#1](https://github.com/ivalduan/mold/pull/1)): work-item model, generation, per-item cache, executor seam, undo | Run-directory cache + per-`PortType` codecs |
| WS-01 `mol` | First PR merged ([#2](https://github.com/ivalduan/mold/pull/2)): arena, PDB/mmCIF I/O, pose-table schema, fixtures in `fixtures/structures/` | Criterion load bench: the 300-pose generator is in WS-13's PR. **The 2 s budget needs a binary cache** — pdbtbx takes ~3 s to re-parse the 59 MB mmCIF |
| WS-00 `spike` | **Done.** PR merged ([#3](https://github.com/ivalduan/mold/pull/3)) and **ADR-0002 + ADR-0003 accepted 2026-11-24**: lineage pinned, viewport path proven on Windows, every exit criterion confirmed by hand. Click → re-aimed frame ~1.6 ms against a 100 ms budget | Nothing. The crate now belongs to WS-08: `mold-render` replaces `src/spike/`, keeping `backend.rs` and `element.rs`. **WS-09, WS-10 and WS-11 are unblocked** |
| WS-13 `fixtures` | PR merged ([#4](https://github.com/ivalduan/mold/pull/4)): EGFR 4WKQ receptor + reference, 395-ligand congeneric series with 6 deliberate failures, PoseBusters reference CSV, checksummed fetch script | **ADR-0007 still `Proposed`**: product owner accepts. Open: no cached MSA (every route either sends the sequence off-box or needs a local alignment); `mock-cofold` replay set deferred |
| WS-05 `jobs` | On hold (ADR-0009) | Design the execution flavours |
| WS-03 `geom` | First PR merged ([#9](https://github.com/ivalduan/mold/pull/9)): kiddo KD-tree, RMSD (plain/Kabsch/centroid), ligand checks (bond lengths, angles, internal clash, aromatic flatness), protein–ligand checks (distance, clashes, volume overlap), covalent-attachment handling, `PoseGate` + `GateConfig`. **All 18 PoseBusters rows match**, 8/8 verdicts; gate 500 poses ~76–85 ms (release) vs 200 ms budget | Contact profiling (H-bonds, salt bridges, π-stacking); symmetry-aware RMSD; non-aromatic ring and double-bond flatness. Test-only V2000 reader stands in for real SDF I/O (belongs in `mol` or `chem`) |
| WS-09 `ui-graph` | First PR merged ([#8](https://github.com/ivalduan/mold/pull/8)): GPUI-free canvas core (view state, layout, drag state machine with drag-time `PortType` refusal), `GraphDoc` over `mold_graph::Editor` (undo/redo), eval badges from `Observer`, Windows-only GPUI view, `examples/canvas.rs`. Only gpui-flow's bezier geometry vendored (MIT) | **ADR-0004 still `Proposed`**: product owner accepts. GPUI smoke test deferred (GPUI Kit `test-support` would carry over into every Windows test build). Node positions are view state until WS-06 persists them |
| WS-02 `chem` | Not started, can start now | Fixture data is on `main` (the 395-ligand series). Risk is the `rdkit-sys` Windows build (ADR-0006), not the chemistry. Natural home for SDF/molfile reading, which `geom` and `nodes` need |
| `recipe` | Unblocked: the `graph` model is on `main` | Only the TOML⇄graph half can start; CLI replay needs `nodes`, `jobs` and the mock worker. Should carry canvas node positions as layout metadata that headless replay ignores |
| `render`, `ui-panels` | Not started, can start now | `render` takes over the spike's `ui-viewport` code; `ui-panels` can build the table shell |
| infrastructure | [#5](https://github.com/ivalduan/mold/pull/5) + [#6](https://github.com/ivalduan/mold/pull/6) merged: [ADR-0012](../adr/0012-fixture-data-storage.md) (no Git LFS, fixture budget) and the CI rework — `ci` profile, path-gated GPUI build, `--locked`, `checkout@v5`. Windows job 28m31s → **5m26s** on its worst path. #8/#9 Windows jobs: 8m34s / 7m26s | Measure a non-UI PR that touches no root files (#9 edited `Cargo.toml`/`Cargo.lock`, so it is no test of skipping the GPUI tree). A `--release` job for the CLAUDE.md performance budgets, with the GPU runner |

Open product decisions that touch upcoming work:
- Polars held below 0.54 (it links tokio); loaders and parallel transforms are to be revisited in [ADR-0011](../adr/0011-loaders-and-parallel-transforms.md).
- Generic/"any" `PortType` for partitioners: deferred until `nodes` shows a need.
- Executor choice is not part of cache keys (proposed answer to ADR-0009 Q6; not yet confirmed).
- **Awaiting acceptance: [ADR-0007](../adr/0007-benchmark-target.md)** (the benchmark is on `main` and in use, so nothing is blocked; it just isn't formally frozen) **and [ADR-0004](../adr/0004-node-canvas-base.md)** (the narrow gpui-flow fork is on `main` and in use).
- Pose Gate node (WS-07) must decide what counts as protein: PoseBusters treats HETATM residues (e.g. 4WKQ's CSX 797) as cofactors; `geom` mirrors that in its cross-validation only. Two PoseBusters behaviours deserve a "trap" note in `fixtures/posebusters/README.md` (WS-13): that HETATM rule, and ring flatness reported as a *signed* max (under-reports on 4WKQ: 0.0082 vs a true 0.0157 Å). [ADR-0002](../adr/0002-gpui-lineage.md) and [ADR-0003](../adr/0003-viewport-path.md) were accepted 2026-11-24.
- Still open from ADR-0003, for whoever builds the app shell: a viewport click does not dismiss an open GPUI Kit dropdown (viewport input goes to the render thread, not GPUI's dismiss logic), there is no MSAA on the spike impostors, and device-lost recovery is unwired although `set_compositor_recreated_callback` exists in gpui-fast 0.1.19.
- `mock-cofold` replay set: deferred until ADR-0009 settles, since its format follows the event stream. The 300-pose generator plus perturbed crystal poses are the raw material ([ADR-0012](../adr/0012-fixture-data-storage.md) keeps it out of a commit).
- Three upstream issues to file against GPUI (not yet filed, needs the owner's go-ahead): `gpui-kit` builds **both** GPUI lineages because its `gpui` (= `gpui-pre`) dependency is not optional and Cargo features are additive — 14 `gpui-pre*` crates compile unused on Windows; plus gpui-fast's Windows `set_visible`/`remove_surface` `E_NOINTERFACE` and the missing public composition commit (we rely on `set_parent_origin` as a side effect). Decided **not** to fork: upstream moves ~7 commits/day, and ADR-0002's upgrade policy plus the spike self-test already catch a break.

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

**Wave 1 (start today, fully parallel):** `spike`, `fixtures`, `mol`, `chem`, `graph`, `geom`. Six worktrees, no shared files except the contract types already on `main`. `jobs` waits for the execution-flavours design ([ADR-0009](../adr/0009-execution-flavours.md)).

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
- Keep the repo light: large sets go behind a fetch script with checksums; the CI subset is committed. The budget and the no-LFS decision are [ADR-0012](../adr/0012-fixture-data-storage.md).

## workers-py
**WS-12.** Python workers under `workers/boltz2` and `workers/vina`, speaking [the protocol](worker-protocol.md). Only after `mock-cofold` works end to end. Boltz-2 runs in WSL2 on the Windows dev box (ADR-0005). Never installs environments; the manifest declares them.
