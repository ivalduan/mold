//! **Contract.** The worker wire protocol shared by `jobs`, `mock-cofold` and the
//! Python workers. Any change here is a PR to `main` that also updates
//! `docs/worker-protocol.md` and every worker in `workers/`.
//!
//! The types are the wire format. The functions here are the runner's reading of
//! it: [`JobSpec::from_value`] validates what a node prepared, [`parse_event`]
//! validates one stdout line. graph hands both over as `serde_json::Value` and never
//! interprets them (ADR-0009 §1).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Protocol version, sent in every [`JobSpec`]. Bump on any breaking change.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest stdout line the runner accepts (1 MiB). Artifacts go to files, never
/// through the pipe, so a longer line is a broken worker.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// Written to the worker's stdin as one JSON document, then EOF.
///
/// Backend-neutral: the same bytes go to a local process, a WSL2 process or an SSH
/// host (ADR-0009). That is why every path in it is **relative**: the runner starts
/// the worker with the run directory as its working directory, so `run_dir` is
/// always `"."` and inputs are relative to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub protocol: u32,
    /// Echoed in every event. Must be stable across runs of the same item (e.g. the
    /// item id), so the spec is byte-identical run to run.
    pub job_id: String,
    /// Always `"."`: the worker's working directory, which the runner sets to the
    /// item's run directory. Artifacts go here; paths in events are relative to it.
    pub run_dir: PathBuf,
    /// Named input artifacts (receptor, ligands, msa), as paths relative to `run_dir`
    /// (use `/` separators so the bytes match on every platform).
    pub inputs: BTreeMap<String, PathBuf>,
    /// Worker-specific parameters, passed through untouched.
    #[serde(default)]
    pub params: serde_json::Value,
}

impl JobSpec {
    /// A spec for `job_id` with no inputs and no params.
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            job_id: job_id.into(),
            run_dir: PathBuf::from("."),
            inputs: BTreeMap::new(),
            params: serde_json::Value::Null,
        }
    }

    /// Add an input, relative to the run directory.
    pub fn input(mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        self.inputs.insert(name.into(), path.into());
        self
    }

    pub fn params(mut self, params: serde_json::Value) -> Self {
        self.params = params;
        self
    }

    /// As JSON, for [`mold_graph::CommandJob::new`].
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("a JobSpec always serializes")
    }

    /// Parse and validate a prepared spec (what a node put in `CommandJob::spec`).
    pub fn from_value(value: &serde_json::Value) -> Result<Self, ProtocolError> {
        let spec: JobSpec = serde_json::from_value(value.clone())
            .map_err(|e| ProtocolError::Spec(format!("not a JobSpec: {e}")))?;
        spec.validate()?;
        Ok(spec)
    }

    /// The rules every runner relies on: this protocol version, a non-empty single-line
    /// `job_id`, `run_dir = "."`, relative input paths that stay inside the run directory.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(ProtocolError::Spec(format!(
                "protocol {} is not supported (this runner speaks {PROTOCOL_VERSION})",
                self.protocol
            )));
        }
        if self.job_id.is_empty() || self.job_id.contains(['\n', '\r']) {
            return Err(ProtocolError::Spec(
                "job_id must be a non-empty single line".into(),
            ));
        }
        if self.run_dir != Path::new(".") {
            return Err(ProtocolError::Spec(format!(
                "run_dir must be \".\" (the worker runs in its run directory), not {:?}",
                self.run_dir
            )));
        }
        for (name, path) in &self.inputs {
            check_relative(path)
                .map_err(|why| ProtocolError::Spec(format!("input `{name}` {why}")))?;
        }
        Ok(())
    }
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
        /// Relative to the run directory.
        cif_path: PathBuf,
        scores: BTreeMap<String, f64>,
    },
    Log {
        job_id: String,
        level: LogLevel,
        message: String,
    },
    /// The last event. `ok` = the item succeeded; anything else fails it.
    Finished {
        job_id: String,
        status: JobStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

impl WorkerEvent {
    pub fn job_id(&self) -> &str {
        match self {
            Self::Started { job_id, .. }
            | Self::Progress { job_id, .. }
            | Self::Pose { job_id, .. }
            | Self::Log { job_id, .. }
            | Self::Finished { job_id, .. } => job_id,
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Finished { .. })
    }
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

/// A spec or an event that breaks the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The prepared `JobSpec` is invalid (a node bug, caught before launch).
    Spec(String),
    /// A stdout line is not a valid event.
    Event(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spec(m) => write!(f, "invalid JobSpec: {m}"),
            Self::Event(m) => write!(f, "invalid worker event: {m}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// The bytes a runner sends for a spec: compact JSON with every object's keys sorted.
///
/// `serde_json::Value`'s key order depends on a cargo feature (`preserve_order`,
/// which GPUI turns on workspace-wide), so plain `to_vec` would give different bytes
/// in the app and the CLI. This does not: the `JobSpec` is byte-identical across
/// binaries, runs and flavours.
pub fn canonical_json(value: &serde_json::Value) -> Vec<u8> {
    fn write(v: &serde_json::Value, out: &mut Vec<u8>) {
        match v {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push(b'{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, k).expect("a string serializes");
                    out.push(b':');
                    write(&map[k], out);
                }
                out.push(b'}');
            }
            serde_json::Value::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write(item, out);
                }
                out.push(b']');
            }
            scalar => serde_json::to_writer(&mut *out, scalar).expect("a scalar serializes"),
        }
    }
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

/// Parse one stdout line (without its newline) into the JSON value handed to the
/// node and the typed event the runner reads. Unknown fields are allowed (and reach
/// the node); an unknown `event`, a missing field, or a path that leaves the run
/// directory is an error.
pub fn parse_event(line: &[u8]) -> Result<(serde_json::Value, WorkerEvent), ProtocolError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(ProtocolError::Event(format!(
            "line of {} bytes (limit {MAX_LINE_BYTES})",
            line.len()
        )));
    }
    let text =
        std::str::from_utf8(line).map_err(|_| ProtocolError::Event("line is not UTF-8".into()))?;
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| ProtocolError::Event(format!("not JSON ({e}): {}", snippet(text))))?;
    let event: WorkerEvent = serde_json::from_value(value.clone())
        .map_err(|e| ProtocolError::Event(format!("{e}: {}", snippet(text))))?;
    if let WorkerEvent::Pose { cif_path, .. } = &event {
        check_relative(cif_path)
            .map_err(|why| ProtocolError::Event(format!("pose `cif_path` {why}")))?;
    }
    Ok((value, event))
}

/// At most 200 characters of a line, for error messages.
fn snippet(text: &str) -> String {
    let t = text.trim_end();
    match t.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

/// A path relative to the run directory that cannot leave it (ADR-0005). Also what
/// the SSH flavour checks a staged path against before it reaches a remote shell.
pub(crate) fn check_relative(path: &Path) -> Result<(), String> {
    let s = path.to_string_lossy();
    if s.is_empty() {
        return Err("is empty".into());
    }
    // `Path` on Unix does not treat `C:\` as absolute; the protocol does, on every
    // platform, because the same bytes travel to Windows, WSL2 and SSH hosts.
    let windows_absolute = s.starts_with(['/', '\\']) || s.as_bytes().get(1) == Some(&b':');
    if path.is_absolute() || windows_absolute {
        return Err(format!("must be relative to the run directory, not {s:?}"));
    }
    let leaves = s.split(['/', '\\']).any(|c| c == "..")
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
    if leaves {
        return Err(format!("must stay inside the run directory: {s:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn spec_builder_validates_and_serializes_deterministically() {
        let spec = JobSpec::new("cofold[0]")
            .input("ligands", "ligands.sdf")
            .input("receptor", "receptor.cif")
            .params(json!({"seed": 7, "samples": 2, "list": [{"b": 1, "a": [true, null]}]}));
        let v = spec.to_value();
        assert_eq!(JobSpec::from_value(&v).unwrap(), spec);
        // Sorted keys whatever serde_json's `preserve_order` feature says.
        assert_eq!(
            String::from_utf8(canonical_json(&v)).unwrap(),
            r#"{"inputs":{"ligands":"ligands.sdf","receptor":"receptor.cif"},"job_id":"cofold[0]","params":{"list":[{"a":[true,null],"b":1}],"samples":2,"seed":7},"protocol":1,"run_dir":"."}"#
        );
        let back: serde_json::Value = serde_json::from_slice(&canonical_json(&v)).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn invalid_specs_are_refused_with_a_reason() {
        let ok = JobSpec::new("j").input("x", "in/x.sdf");
        let cases: Vec<(JobSpec, &str)> = vec![
            (
                JobSpec {
                    protocol: 2,
                    ..ok.clone()
                },
                "protocol 2",
            ),
            (JobSpec::new(""), "job_id"),
            (
                JobSpec {
                    run_dir: "/runs/abc".into(),
                    ..ok.clone()
                },
                "run_dir",
            ),
            (ok.clone().input("y", "/etc/passwd"), "input `y`"),
            (ok.clone().input("y", "C:\\data\\l.sdf"), "relative"),
            (ok.clone().input("y", "../other/l.sdf"), "inside the run"),
            (ok.clone().input("y", "a\\..\\..\\l.sdf"), "inside the run"),
        ];
        for (spec, want) in cases {
            let e = spec.validate().unwrap_err().to_string();
            assert!(e.contains(want), "{e:?} should mention {want:?}");
        }
        assert!(JobSpec::from_value(&json!({"job_id": "j"})).is_err());
        ok.validate().unwrap();
    }

    #[test]
    fn events_parse_into_value_and_type() {
        let line = br#"{"event":"progress","job_id":"j","completed":1,"total":4,"extra":"kept"}"#;
        let (v, ev) = parse_event(line).unwrap();
        assert_eq!(v["extra"], "kept", "unknown fields reach the node");
        assert_eq!(
            ev,
            WorkerEvent::Progress {
                job_id: "j".into(),
                completed: 1,
                total: 4
            }
        );
        assert!(!ev.is_finished());
        assert_eq!(ev.job_id(), "j");
    }

    #[test]
    fn malformed_events_are_refused() {
        for (line, want) in [
            (&b"not json"[..], "not JSON"),
            (br#"{"event":"teleport","job_id":"j"}"#, "teleport"),
            (br#"{"event":"started","job_id":"j"}"#, "total"),
            (
                br#"{"event":"pose","job_id":"j","pose_index":0,"cif_path":"/abs.cif","scores":{}}"#,
                "cif_path",
            ),
            (b"\xff\xfe", "UTF-8"),
        ] {
            let e = parse_event(line).unwrap_err().to_string();
            assert!(e.contains(want), "{e:?} should mention {want:?}");
        }
        let long = vec![b'x'; MAX_LINE_BYTES + 1];
        assert!(
            parse_event(&long)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }
}
