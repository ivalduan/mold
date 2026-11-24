//! **Contract.** The worker wire protocol shared by `jobs`, `mock-cofold` and the
//! Python workers. Any change here is a PR to `main` that also updates
//! `docs/worker-protocol.md` and every worker in `workers/`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Protocol version, sent in every [`JobSpec`]. Bump on any breaking change.
pub const PROTOCOL_VERSION: u32 = 1;

/// Written to the worker's stdin as one JSON document.
///
/// DRAFT: the engineering plan fixes the transport, not these fields. The `jobs`
/// workstream owns the final shape; keep it backend-neutral (Local, SSH, Slurm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub protocol: u32,
    pub job_id: String,
    /// Directory the worker writes artifacts into. Paths in events are relative to it.
    pub run_dir: PathBuf,
    /// Named input artifacts (receptor, ligands, msa), as paths.
    pub inputs: BTreeMap<String, PathBuf>,
    /// Worker-specific parameters, passed through untouched.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// One line of the worker's stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkerEvent {
    Started {
        job_id: String,
        total: u32,
    },
    Progress {
        job_id: String,
        completed: u32,
        total: u32,
    },
    /// Appended to the pose table and the arena the moment it arrives.
    Pose {
        job_id: String,
        pose_index: u32,
        cif_path: PathBuf,
        scores: BTreeMap<String, f64>,
    },
    Log {
        job_id: String,
        level: LogLevel,
        message: String,
    },
    Finished {
        job_id: String,
        status: JobStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Ok,
    Failed,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_event_wire_format_is_stable() {
        let line = r#"{"event":"pose","job_id":"j1","pose_index":3,"cif_path":"poses/3.cif","scores":{"iptm":0.84,"plddt":0.91}}"#;
        let ev: WorkerEvent = serde_json::from_str(line).unwrap();
        assert_eq!(serde_json::to_string(&ev).unwrap(), line);
    }

    #[test]
    fn finished_without_error_omits_field() {
        let ev = WorkerEvent::Finished {
            job_id: "j1".into(),
            status: JobStatus::Ok,
            error: None,
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"finished","job_id":"j1","status":"ok"}"#
        );
    }
}
