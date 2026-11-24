# Worker protocol (v1)

**Contract.** Code form: [crates/jobs/src/protocol.rs](../crates/jobs/src/protocol.rs). Changes are a PR to `main` that updates this file, that module, and every worker in `workers/`.

## Transport
- The supervisor spawns the worker's executable (from its manifest).
- **stdin**: exactly one JSON document, the `JobSpec`, then EOF.
- **stdout**: newline-delimited JSON, one `WorkerEvent` object per line, flushed per line.
- **stderr**: free-form logs, captured but not parsed.
- **Artifacts** (pose `.cif` files etc.) are written into `run_dir` and referenced by **relative path**. Never stream artifact bytes through the pipe.
- Exit code 0 after `finished`. Cancellation = the supervisor kills the process group; the cache entry is invalidated.

## JobSpec (draft; owned by WS-05)
```json
{"protocol":1,"job_id":"j-7f3a","run_dir":"/runs/abc/cofold","inputs":{"receptor":"receptor.cif","ligands":"ligands.sdf","msa":"target.a3m"},"params":{"model":"boltz-2","samples":5}}
```

## Events
| `event` | Fields |
|---|---|
| `started` | `job_id`, `total` |
| `progress` | `job_id`, `completed`, `total` |
| `pose` | `job_id`, `pose_index`, `cif_path`, `scores` (map name → number) |
| `log` | `job_id`, `level` (`debug`/`info`/`warn`/`error`), `message` |
| `finished` | `job_id`, `status` (`ok`/`failed`/`cancelled`), `error`? |

```json
{"event":"pose","job_id":"j-7f3a","pose_index":3,"cif_path":"poses/3.cif","scores":{"iptm":0.84,"plddt":0.91,"affinity_pred":7.2}}
```

A `pose` event is appended to the pose table and the arena **the moment it arrives**: poses stream into the workspace as the GPU finishes them. That behaviour is most of the demo.

Score keys should use the pose-table column names (`plddt`, `iptm`, `affinity_pred`, `binder_prob`) where they mean the same thing.

## Manifest (`workers/<name>/manifest.toml`, draft)
`name`, `executable`, `environment` (declared, never installed by the app), `needs_gpu`, `model`, `model_version` (pinned deliberately), `license`. The app validates it and reports a clear error if something is missing.

## Backends
`LocalProcess` (v0.1; also how WSL2 is reached), `Ssh`, `Slurm` (stubbed). The job spec and event stream must not change between backends: that's the test of whether this protocol is designed right.
