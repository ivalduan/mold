//! Mock co-fold worker. Reads a `JobSpec` on stdin, replays the poses in a
//! fixture directory with realistic timing, and emits the same NDJSON event
//! stream as the real workers. Owned by workstream WS-05.
//!
//! First-class deliverable: it makes the whole graph, table and viewport
//! developable and testable with no GPU and no Python.

use std::io::{self, Read, Write};

use mold_jobs::protocol::{JobSpec, JobStatus, WorkerEvent};

fn main() -> io::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let spec: JobSpec = serde_json::from_str(&input).map_err(io::Error::other)?;

    let mut out = io::stdout().lock();
    // TODO(WS-05): replay fixture poses (`spec.params.fixture_dir`) with timing.
    emit(
        &mut out,
        &WorkerEvent::Finished {
            job_id: spec.job_id,
            status: JobStatus::Failed,
            error: Some("mock-cofold: fixture replay not implemented yet".into()),
        },
    )
}

fn emit(out: &mut impl Write, ev: &WorkerEvent) -> io::Result<()> {
    serde_json::to_writer(&mut *out, ev).map_err(io::Error::other)?;
    out.write_all(b"\n")?;
    out.flush()
}
