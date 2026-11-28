# Worker protocol (v1)

**Contract.** Code form: [crates/jobs/src/protocol.rs](../crates/jobs/src/protocol.rs). Changes are a PR to `main` that updates this file, that module, and every worker in `workers/`.

## Transport
- The runner (`mold-jobs`) spawns the worker's executable (from its manifest) **with the item's run directory as its working directory**.
- **stdin**: exactly one JSON document, the `JobSpec`, then EOF.
- **Write the first event (normally `started`) as soon as the `JobSpec` is read.** The attempt counts as running from that event: the node's `timeout` starts there, so process start-up never eats into it.
- **stdout**: newline-delimited JSON, one `WorkerEvent` object per line, flushed per line. At most 1 MiB per line. Blank lines are ignored.
- **stderr**: free-form logs, captured to `stderr.log` in the run directory, not parsed. Its last lines are quoted when the run fails.
- **Artifacts** (pose `.cif` files etc.) are written into the run directory and referenced by **relative path**. Never stream artifact bytes through the pipe.
- **Exit code 0 after `finished`.** Cancellation and timeout: the runner kills the worker's whole process tree (Windows Job Object, Unix process group); no cache entry is written. Whatever the outcome, nothing the worker started outlives the run.

### Reserved names in the run directory
The runner writes these; a worker must not: `job.json` (the exact bytes sent on stdin), `events.ndjson` (every stdout line as received), `stderr.log`.

## JobSpec
```json
{"inputs":{"ligands":"ligands.sdf","msa":"target.a3m","receptor":"receptor.cif"},"job_id":"cofold-1[3]","params":{"model":"boltz-2","samples":5},"protocol":1,"run_dir":"."}
```
- Sent as **canonical JSON**: compact, every object's keys sorted (`mold_jobs::protocol::canonical_json`), so the bytes do not depend on how the sending binary was built (GPUI turns on serde_json's `preserve_order`).
- `protocol`: `1`. Anything else is refused before launch.
- `job_id`: non-empty, one line, echoed by every event. Make it stable for the item (e.g. the item id) so the spec is byte-identical run to run.
- `run_dir`: always `"."`. The worker runs in its run directory, so no machine path appears in the spec, and the same bytes go to a local, WSL2 or SSH worker.
- `inputs`: name → path **relative to the run directory**, with `/` separators. Absolute paths (`/x`, `C:\x`) and paths with `..` are refused before launch. Each input must exist in the run directory when the worker starts (the node's `prepare` puts it there).
- `params`: worker-specific, passed through untouched.

Rust nodes build it with `mold_jobs::protocol::JobSpec::new(job_id).input(..).params(..)`, then `to_value()`.

## Events
| `event` | Fields |
|---|---|
| `started` | `job_id`, `total` |
| `progress` | `job_id`, `completed`, `total` |
| `pose` | `job_id`, `pose_index`, `cif_path` (relative, stays inside the run directory), `scores` (map name → number) |
| `log` | `job_id`, `level` (`debug`/`info`/`warn`/`error`), `message` |
| `finished` | `job_id`, `status` (`ok`/`failed`/`cancelled`), `error`? |

```json
{"event":"pose","job_id":"cofold-1[3]","pose_index":3,"cif_path":"poses/3.cif","scores":{"affinity_pred":7.2,"iptm":0.84,"plddt":0.91}}
```

Extra fields are allowed and reach the node. An unknown `event`, a missing field, a `job_id` that is not the spec's, or an event after `finished` fails the item.

A `pose` event is appended to the pose table and the arena **the moment it arrives**: poses stream into the workspace as the GPU finishes them. That behaviour is most of the demo. Every event except `finished` goes to the node's `collect_event` as it arrives; `finished` goes to `collect_finish`.

Score keys should use the pose-table column names (`plddt`, `iptm`, `affinity_pred`, `binder_prob`) where they mean the same thing.

## How a run ends
| Worker does | Item |
|---|---|
| `finished` with `ok`, exits 0 | succeeds |
| `finished` with `failed`/`cancelled` | fails with the `error` text |
| `finished` with `ok`, non-zero exit | fails |
| exits (any code) or crashes without `finished` | fails, quoting the exit status and stderr |
| writes a malformed line | fails at once, naming the line; the tree is killed |
| exits but a child keeps stdout open | after 2 s the tree is killed; the run ends on what was received |
| hangs | the node's `timeout` (engine) cancels it; the tree is killed |
| writes no event within 60 s of starting (or the node's `timeout`, if longer) | fails; the tree is killed |

## Manifest (`workers/<name>/manifest.toml`)
Schema and validation: [crates/jobs/src/manifest.rs](../crates/jobs/src/manifest.rs). Unknown keys are refused.

| Key | Required | |
|---|---|---|
| `name` | yes | lowercase letters, digits, `-`, `_`; must equal the directory name |
| `executable` | yes | the command line; `"double quotes"` group words. A bare program name is looked up in the worker search path, the manifest's directory, then `PATH`; a relative path is relative to the manifest |
| `environment` | yes | `"none"` or `"conda:<env>"` (launched as `conda run --no-capture-output -n <env> …`). Declared, never installed or repaired by the app |
| `needs_gpu` | yes | `true` → each item needs one GPU (`Manifest::needs`) |
| `model` | yes | |
| `model_version` | no | pinned deliberately |
| `license` | no | |

An invalid manifest fails the load of the workers directory, naming the file and the problem, before any executor exists.

## Backends
Decided in [ADR-0009](../adr/0009-execution-flavours.md): this protocol is the payload of **every** execution flavour. The `local` executor (incl. WSL2 via a `wsl.exe` prefix: only the run directory's path is translated) and the `ssh` executor (stdio over the SSH connection, run directory staged in and out; not yet built) send the same `JobSpec` and read the same event stream; Slurm and other batch schedulers later append the same lines to `run_dir/events.ndjson`. The job spec and event stream must not change between local and remote execution: that's the test of whether this protocol is designed right.

## Changes in the first `jobs` PR (v1 stays v1: no worker implemented the draft)
- `run_dir` is always `"."`; the worker's working directory is its run directory (was: an absolute path).
- Input and `cif_path` paths must be relative and stay inside the run directory (was: unstated).
- The spec is sent as canonical JSON (sorted keys).
- Defined how a run ends (table above), the 1 MiB line limit, the reserved file names, and that extra event fields are allowed.
- Manifest schema finalised: `executable` quoting, `environment` forms, `name` = directory name, unknown keys refused.
