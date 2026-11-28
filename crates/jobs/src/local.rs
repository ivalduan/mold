//! The local executor (ADR-0009 §2): command items as subprocesses on this machine,
//! WSL2 included (a `wsl.exe` prefix). Std threads, no async runtime.
//!
//! Queueing, capacity, run directories and cancellation are the shared
//! [`Scheduler`](crate::sched::Scheduler); this module is only phase *run*: resolve
//! the worker's executable on this machine and hand the process to
//! [`runner`](crate::runner), which owns the stdio protocol and the process-tree kill.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use mold_graph::{CommandJob, EventSink, Job, NodeError};

use crate::error::ConfigError;
use crate::manifest::Workers;
use crate::profile::{Profile, Target, Wsl};
use crate::runner::{self, Launch};
use crate::sched::{Flavour, RunSpec, Scheduler, command_executor, manifest_for};
use crate::wsl::wsl_command;

/// Runs command items as subprocesses on this machine. See the [module docs](self).
pub struct LocalExecutor {
    sched: Arc<Scheduler>,
}

command_executor!(LocalExecutor);

impl LocalExecutor {
    /// An executor for `profile` (a local or WSL2 one), launching `workers`, with run
    /// directories under `run_root`. An SSH profile is refused: not in this version.
    pub fn new(
        profile: &Profile,
        workers: Arc<Workers>,
        run_root: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let wsl = match &profile.target {
            Target::Local { wsl } => wsl.clone(),
            Target::Ssh(_) => {
                return Err(ConfigError::NotYet {
                    key: profile.key.clone(),
                    what: "SSH executors are not available yet (planned: stdio over SSH                            with run-directory staging, ADR-0009 §2-3)"
                        .into(),
                });
            }
        };
        if wsl.is_some() && !cfg!(windows) {
            return Err(ConfigError::NotYet {
                key: profile.key.clone(),
                what: "a `wsl` profile runs only on Windows".into(),
            });
        }
        Ok(Self {
            sched: Scheduler::new(
                profile,
                run_root.into(),
                Box::new(Local {
                    wsl,
                    workers,
                    key: profile.key.clone(),
                }),
            ),
        })
    }

    pub fn key(&self) -> &str {
        self.sched.key()
    }

    /// Notices since the last call (each is reported once per executor).
    pub fn take_notices(&self) -> Vec<crate::sched::Notice> {
        self.sched.take_notices()
    }

    /// Wait until nothing is queued or running. `false` on timeout.
    pub fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        self.sched.wait_idle(timeout)
    }
}

/// Phase *run* on this machine.
struct Local {
    wsl: Option<Wsl>,
    workers: Arc<Workers>,
    key: String,
}

impl fmt::Debug for Local {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("local")
            .field("key", &self.key)
            .field("wsl", &self.wsl)
            .finish()
    }
}

impl Flavour for Local {
    fn run(
        &self,
        r: &RunSpec<'_>,
        events: &mut EventSink<'_>,
    ) -> Result<serde_json::Value, NodeError> {
        let launch = self.launch(r.job, r.cmd)?;
        let control = runner::Control {
            stop: r.stop,
            up: r.up,
            startup_limit: r.startup_limit,
            // A local worker's whole tree dies with `ProcessTree::kill`; there is
            // nothing on a far side to reap.
            reap: &|| {},
            hint: &|_| String::new(),
        };
        runner::run(&r.cmd.worker, &launch, r.cmd, r.spec, events, &control)
    }
}

impl Local {
    /// The command line for `cmd.worker` on this machine.
    fn launch(&self, job: &Job, cmd: &CommandJob) -> Result<Launch, NodeError> {
        let worker = &cmd.worker;
        let m = manifest_for(&self.workers, job, worker)?;
        let argv = m.argv();
        if let Some(wsl) = &self.wsl {
            let (program, args) = wsl_command(wsl, &cmd.run_dir, &argv)
                .map_err(|e| NodeError::msg(format!("worker `{worker}` via WSL: {e}")))?;
            return Ok(Launch { program, args });
        }
        let program = self
            .workers
            .resolve_program(&argv[0], m)
            .map_err(|e| NodeError::msg(format!("worker `{worker}`: {e}")))?;
        Ok(Launch {
            program: program.into_os_string(),
            args: argv[1..].iter().map(OsString::from).collect(),
        })
    }
}
