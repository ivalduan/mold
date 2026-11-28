# workers/

Solvers that run as **separate processes**, never linked into the app. Python owns anything with a neural network or a force field inside it.

- `mock-cofold/`: Rust, in the Cargo workspace. WS-05. Build first. This package also carries the executors' process tests and the `ssh`/`scp` **test doubles** (`mock-ssh`, `mock-scp`, `src/double.rs`): not workers, but they have to be binaries of this package for `CARGO_BIN_EXE_*` to reach them from one test. See [crates/jobs/CLAUDE.md](../crates/jobs/CLAUDE.md).
- `boltz2/`, `vina/`: Python, **not** in the Cargo workspace. WS-12.

Every worker: reads one `JobSpec` JSON on stdin, writes NDJSON `WorkerEvent`s on stdout, writes artifacts to the run dir and refers to them by path, ships a `manifest.toml`. Spec: [docs/worker-protocol.md](../docs/worker-protocol.md).
