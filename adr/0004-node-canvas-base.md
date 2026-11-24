# 0004. Node canvas base

- Status: Proposed
- Workstream: WS-09 ui-graph

## Options
| Reference | Gives | Verdict so far |
|---|---|---|
| gpui-flow | React-Flow-style canvas: nodes, edges, handles, pan/zoom, custom renderers, GPUI components inside nodes | Closest fit, evaluate first |
| ferrum-flow | Node editor with plugin architecture and undo/redo command system | Second look, mainly for its command model |
| Write our own | Full control | Only if both fail the rubric |

## Rubric: reject any base that
owns the application's data model · owns the event loop · hard-codes a node payload instead of being generic over user data · doesn't render nodes through a supplied callback · pins a GPUI version we can't move.

## Default if nobody decides
**Fork gpui-flow** rather than depend on it: young crates, we need typed ports and per-node evaluation state; a fork costs a week, a wrong dependency is a permanent tax.

## Decision
_Pending evaluation._
