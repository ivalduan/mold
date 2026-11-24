# vina worker — workstream WS-12

Python wrapper around AutoDock Vina (CPU-only, runs natively on Windows) speaking the [worker protocol](../../docs/worker-protocol.md). Gives a dock path that needs no GPU, useful on the development-tier machine.

gnina is GPL: if it's ever added, it **must** stay a separate process (see ADR-0008).
