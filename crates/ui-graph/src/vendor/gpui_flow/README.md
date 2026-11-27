# Vendored from gpui-flow

- Upstream: https://github.com/pacifio/gpui-flow
- Revision: `cfa578758ee251657f15f281d893a08b85aedbff` (2026-03-23, "init v0.1.0")
- Licence: MIT, see `LICENSE` in this directory (kept as required by its terms).
- Why: ADR-0004 (narrow fork). Only the edge geometry is vendored.

| File here | Upstream path |
|---|---|
| `bezier.rs` | `src/edges/bezier.rs` (itself a port of xyflow's `getBezierPath`, MIT) |

The first commit adding this directory holds the file byte-for-byte as upstream; every
later change is a separate commit, so `git log -p -- crates/ui-graph/src/vendor/` is the
diff from upstream.
