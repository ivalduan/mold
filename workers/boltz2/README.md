# boltz2 worker — workstream WS-12

Python wrapper that runs Boltz-2 co-folding (pose + affinity) as a subprocess and speaks the [worker protocol](../../docs/worker-protocol.md).

- Runs in **WSL2** on the Windows dev box, reached through the `LocalProcess` backend; later on a shared Linux box over SSH. Nothing in the job spec changes between them (ADR-0005).
- Writes each pose's `.cif` into the run directory and emits a `pose` event **as soon as each finishes**, never in a batch at the end.
- Needs ~11 GB VRAM for structure, 7–8 GB for affinity; 24 GB is the workable minimum.
- Pin the open-weights Boltz-2 version in `manifest.toml`. Never import the model into the Rust app.
- Build only after `mock-cofold` works end to end.
