//! Mock co-fold worker: a deterministic stand-in for Boltz-2 with no GPU and no
//! Python. Owned by workstream WS-05.
//!
//! Reads a `JobSpec` on stdin (run directory = the working directory) and, for each
//! ligand record in `inputs.ligands` (an SDF), emits `samples` `pose` events with
//! seeded, plausible scores, writing each pose (the ligand, jittered) to
//! `poses/lig<i>_s<k>.cif`. Then `finished`.
//!
//! Params (all optional):
//!
//! | param | default | |
//! |---|---|---|
//! | `samples` | 5 | poses per ligand |
//! | `seed` | 0 | mixed into every score and coordinate |
//! | `delay_ms` | 0 | sleep before each pose |
//! | `write_files` | true | write pose files (else `cif_path` names a file that does not exist) |
//! | `fail` | none | failure injection, see below |
//! | `fail_after` | 1 | poses emitted before the failure |
//! | `gate` | none | a file (relative to the run directory) to wait for before pose `gate_after` |
//! | `gate_after` | 1 | poses emitted before waiting for `gate` |
//!
//! `fail`: `"crash"` (abort), `"exit"` (exit code 3, no `finished`), `"error"`
//! (`finished` with status `failed`), `"hang"` (sleep forever), `"malformed"` (a
//! non-JSON line), `"orphan"` (spawn a child that sleeps forever, write `pids.json`,
//! then hang), `"linger"` (spawn a child that keeps stdout open, then finish normally).
//!
//! The replay of recorded fixture runs is WS-13's (not here).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use mold_jobs::protocol::{JobSpec, JobStatus, LogLevel, WorkerEvent};
use mold_mock_cofold::{Rng, pose_path, scores};
use mold_mol::Arena;

/// Run as the grandchild in the `orphan` and `linger` modes.
const SLEEP_FOREVER: &str = "--sleep-forever";

fn main() {
    if std::env::args().nth(1).as_deref() == Some(SLEEP_FOREVER) {
        sleep_forever();
    }
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("mock-cofold: cannot read stdin: {e}");
        std::process::exit(2);
    }
    let spec: JobSpec = match serde_json::from_str(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mock-cofold: stdin is not a JobSpec: {e}");
            std::process::exit(2);
        }
    };
    let mut out = Out {
        w: io::stdout().lock(),
        job_id: spec.job_id.clone(),
    };
    let code = match run(&spec, &mut out) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mock-cofold: {e}");
            out.emit(WorkerEvent::Finished {
                job_id: spec.job_id.clone(),
                status: JobStatus::Failed,
                error: Some(e),
            });
            1
        }
    };
    std::process::exit(code);
}

struct Params {
    samples: u32,
    seed: u64,
    delay: Duration,
    write_files: bool,
    fail: Option<String>,
    fail_after: u32,
    gate: Option<String>,
    gate_after: u32,
}

impl Params {
    fn read(v: &serde_json::Value) -> Result<Self, String> {
        let int = |k: &str, d: u64| match v.get(k) {
            None => Ok(d),
            Some(x) => x
                .as_u64()
                .ok_or(format!("param `{k}` must be a non-negative integer")),
        };
        Ok(Self {
            samples: int("samples", 5)? as u32,
            seed: int("seed", 0)?,
            delay: Duration::from_millis(int("delay_ms", 0)?),
            write_files: match v.get("write_files") {
                None => true,
                Some(x) => x.as_bool().ok_or("param `write_files` must be a boolean")?,
            },
            fail: match v.get("fail") {
                None | Some(serde_json::Value::Null) => None,
                Some(x) => Some(
                    x.as_str()
                        .ok_or("param `fail` must be a string")?
                        .to_string(),
                ),
            },
            fail_after: int("fail_after", 1)? as u32,
            gate: match v.get("gate") {
                None | Some(serde_json::Value::Null) => None,
                Some(x) => Some(
                    x.as_str()
                        .ok_or("param `gate` must be a string")?
                        .to_string(),
                ),
            },
            gate_after: int("gate_after", 1)? as u32,
        })
    }
}

struct Out<W: Write> {
    w: W,
    job_id: String,
}

impl<W: Write> Out<W> {
    fn emit(&mut self, ev: WorkerEvent) {
        let line = serde_json::to_string(&ev).expect("events serialize");
        self.raw(&line);
    }

    fn raw(&mut self, line: &str) {
        // A closed stdout means the supervisor is gone: stop quietly.
        if writeln!(self.w, "{line}")
            .and_then(|()| self.w.flush())
            .is_err()
        {
            std::process::exit(0);
        }
    }

    fn log(&mut self, message: String) {
        let job_id = self.job_id.clone();
        self.emit(WorkerEvent::Log {
            job_id,
            level: LogLevel::Info,
            message,
        });
    }
}

fn run<W: Write>(spec: &JobSpec, out: &mut Out<W>) -> Result<(), String> {
    let p = Params::read(&spec.params)?;
    let dir = &spec.run_dir;
    let lig_path = spec
        .inputs
        .get("ligands")
        .ok_or("no `ligands` input in the JobSpec")?;
    let mut arena = Arena::new();
    let read = mold_mol::sdf::read_sdf_file(&mut arena, dir.join(lig_path))
        .map_err(|e| format!("cannot read ligands {}: {e}", lig_path.display()))?;
    let ligands = read.poses();
    if ligands.is_empty() {
        return Err(format!("no ligand records in {}", lig_path.display()));
    }
    let total = ligands.len() as u32 * p.samples;
    let job_id = spec.job_id.clone();
    out.emit(WorkerEvent::Started {
        job_id: job_id.clone(),
        total,
    });
    out.log(format!(
        "mock-cofold: {} ligand(s) x {} sample(s), seed {}",
        ligands.len(),
        p.samples,
        p.seed
    ));
    if p.write_files {
        std::fs::create_dir_all(dir.join("poses")).map_err(|e| e.to_string())?;
    }

    let mut done = 0u32;
    for (i, &lig) in ligands.iter().enumerate() {
        for k in 0..p.samples {
            if p.fail.is_some() && done == p.fail_after {
                inject(p.fail.as_deref().unwrap_or_default(), dir, out)?;
            }
            if let Some(gate) = p.gate.as_ref().filter(|_| done == p.gate_after) {
                wait_for(&dir.join(gate))?;
            }
            std::thread::sleep(p.delay);
            let rel = PathBuf::from(pose_path(i, k));
            if p.write_files {
                let mut rng = Rng::new(p.seed ^ 0x5EED, i as u64, u64::from(k));
                write_pose(&arena, lig, &mut rng, &dir.join(&rel))?;
            }
            out.emit(WorkerEvent::Pose {
                job_id: job_id.clone(),
                pose_index: done,
                cif_path: rel,
                scores: scores(p.seed, i as u64, u64::from(k)),
            });
            done += 1;
            out.emit(WorkerEvent::Progress {
                job_id: job_id.clone(),
                completed: done,
                total,
            });
        }
    }
    if p.fail.is_some() && done == p.fail_after {
        inject(p.fail.as_deref().unwrap_or_default(), dir, out)?;
    }
    out.emit(WorkerEvent::Finished {
        job_id,
        status: JobStatus::Ok,
        error: None,
    });
    Ok(())
}

/// Failure injection, at the point `fail_after` poses have been sent.
fn inject<W: Write>(mode: &str, dir: &Path, out: &mut Out<W>) -> Result<(), String> {
    match mode {
        "crash" => {
            eprintln!("mock-cofold: injected crash");
            std::process::abort()
        }
        "exit" => {
            eprintln!("mock-cofold: injected exit 3");
            std::process::exit(3)
        }
        "error" => Err("injected failure".into()),
        "hang" => sleep_forever(),
        "malformed" => {
            out.raw("this line is not JSON");
            sleep_forever()
        }
        "orphan" => {
            let child = spawn_sleeper(Stdio::null())?;
            let pids = serde_json::json!({ "worker": std::process::id(), "child": child });
            std::fs::write(dir.join("pids.json"), pids.to_string()).map_err(|e| e.to_string())?;
            out.log(format!("mock-cofold: spawned child {child}"));
            sleep_forever()
        }
        "linger" => {
            // The child inherits stdout and outlives us: the supervisor must not wait
            // for EOF forever, and must not leave the child behind.
            let child = spawn_sleeper(Stdio::inherit())?;
            let pids = serde_json::json!({ "worker": std::process::id(), "child": child });
            std::fs::write(dir.join("pids.json"), pids.to_string()).map_err(|e| e.to_string())?;
            Ok(())
        }
        other => Err(format!("unknown `fail` mode {other:?}")),
    }
}

/// Wait (up to 60 s) until `path` exists: lets a test hold the worker mid-stream
/// without timing assumptions.
fn wait_for(path: &Path) -> Result<(), String> {
    let t = std::time::Instant::now();
    while !path.exists() {
        if t.elapsed() > Duration::from_secs(60) {
            return Err(format!("gate {} never opened", path.display()));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn spawn_sleeper(stdout: Stdio) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let child = Command::new(exe)
        .arg(SLEEP_FOREVER)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot spawn child: {e}"))?;
    Ok(child.id())
}

fn sleep_forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// The ligand with every atom jittered by up to 0.3 Å, as mmCIF.
fn write_pose(
    arena: &Arena,
    lig: mold_mol::PoseId,
    rng: &mut Rng,
    path: &Path,
) -> Result<(), String> {
    let mut a = Arena::new();
    let p = arena.pose(lig).ok_or("ligand pose missing")?;
    // Copy the pose into a fresh arena so each sample is written on its own.
    let mut b = a.begin_pose(p.name.clone());
    b.add_chain("L");
    b.add_residue("LIG", 1, None, true);
    for i in p.atoms.clone() {
        let [x, y, z] = arena.positions()[i];
        let j = |v: f32, r: &mut Rng| v + (0.6 * r.unit() - 0.3) as f32;
        b.add_atom(mold_mol::AtomRecord {
            name: arena.atom_names()[i],
            element: arena.elements()[i],
            position: [j(x, rng), j(y, rng), j(z, rng)],
            b_factor: 0.0,
            occupancy: 1.0,
            formal_charge: arena.formal_charges()[i],
        });
    }
    let id = b.finish();
    mold_mol::io::write_file(&a, id, path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}
