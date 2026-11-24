# Authoring a node

> **To be written by WS-07 before the fifth node exists, not after.** This stub records the target.

The node author's contract is the real deliverable of the graph work. Target by end of W4: adding a twelfth node takes **one file and under 100 lines**:

1. Declare ports (`PortType` per input and output).
2. Declare parameters (these appear in the recipe and the UI automatically).
3. Implement `evaluate`.

If adding a node requires touching any UI crate, the abstraction is wrong; stop and fix it.

Rule for granularity: a step earns its own node when it has its own cache key, its own failure mode, or its own runtime. A step that can never re-run without its neighbour is a parameter on that neighbour.
