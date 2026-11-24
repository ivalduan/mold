# mold-ui-panels — workstream WS-10

The linked views. Runs W4–W6.

## Scope
- **Pose table**: GPUI Kit virtualized table over the Polars pose table; sort and filter read the table only.
- **Scatter**: affinity vs. confidence (candidate: `gpui-plot` with its `plotters` backend).
- **2D SAR / R-group** view of ligands (2D depiction from `chem`).
- **Selection bus**: one `mold_mol::Selection` broadcast to every panel; no panel owns it. The plot publishes on point click, the table on row click; the viewport subscribes. Define the bus in this crate (or `app`) on day one; linked views can't be bolted on later.

## May depend on
`mold-mol`, GPUI Kit, `gpui-plot`. Panels never touch the coordinate arena; only the viewport does.

## Acceptance tests
- [ ] Clicking a scatter point snaps the viewport to that pose in under 100 ms (with `ui-viewport`).
- [ ] 300-ligand result set loads from cache and displays in under 2 s.
- [ ] Table and plot stay in sync both directions.
