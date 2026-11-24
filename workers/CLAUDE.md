# workers/

Solvers that run as **separate processes**, never linked into the app. Python owns anything with a neural network or a force field inside it.

- `mock-cofold/`: Rust, in the Cargo workspace. WS-05. Build first.
- `boltz2/`, `vina/`: Python, **not** in the Cargo workspace. WS-12.

Every worker: reads one `JobSpec` JSON on stdin, writes NDJSON `WorkerEvent`s on stdout, writes artifacts to the run dir and refers to them by path, ships a `manifest.toml`. Spec: [docs/worker-protocol.md](../docs/worker-protocol.md).
