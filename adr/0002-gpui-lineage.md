# 0002. GPUI lineage to pin

- Status: **Proposed (blocks week 0)**
- Workstream: WS-00 spike

## Context
Every UI crate compiles against GPUI. GPUI Kit is a single dependency that pins a matching GPUI release and re-exports the component layer (dock layout with resizable panels, virtualized tables, themes), with the `gpui-fast` backend as an additive Cargo feature. The viewport reference (`javagg/gpui-fast-wgpu-demo`) uses gpui-fast's public composition API.

## Default if nobody decides
`gpui-kit` with the `gpui-fast` feature.

## To record
- Exact crate versions / git revisions, pinned in `Cargo.lock`.
- The gpui-fast revision the viewport handle downcast was tested against (`platform_handle()` is an untyped `Any`; a change there fails at runtime, not compile time).
- Upgrade policy.

## Decision
_Pending._
