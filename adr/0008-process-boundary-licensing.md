# 0008. Solvers are subprocesses; the licence boundary

- Status: Accepted

## Decision
Rust owns everything inside the interactive loop (parsing, geometry, filtering, rendering). Python owns anything with a neural network or force field inside it, and runs in a separate process. **Nothing that loads a GPU model is ever linked into an app binary.**

## Why it is also a licensing decision
Mold is Apache-2.0. Vina is Apache-2.0 and Boltz is MIT, but gnina is GPL. Invoking GPL tools as separate processes keeps their licence off our codebase. Any proposal to link a solver in-process must answer the licence question first.

## Consequences
- Every third-party component is listed in `THIRD_PARTY.md` with mode **linked** or **subprocess**.
- The worker protocol (docs/worker-protocol.md) is the only integration surface for solvers.
