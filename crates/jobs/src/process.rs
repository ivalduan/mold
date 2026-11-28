//! A worker and everything it spawns, killable as one (ADR-0009 §7).
//!
//! - **Windows**: the worker starts suspended, joins a fresh Job Object
//!   (`KILL_ON_JOB_CLOSE`), then resumes, so it cannot spawn a child outside the job.
//!   Kill = `TerminateJobObject`; dropping the tree also kills whatever is left.
//! - **Unix**: the worker leads a new process group (`process_group(0)`); its children
//!   inherit it. Kill = `killpg(SIGKILL)`. A worker that calls `setsid` escapes; none
//!   of ours do.

use std::io;
use std::process::{Child, Command, ExitStatus};

/// A spawned worker process and its descendants.
#[derive(Debug)]
pub(crate) struct ProcessTree {
    child: Child,
    #[cfg(windows)]
    job: win::Job,
}

impl ProcessTree {
    /// Spawn `cmd` as the root of a new tree.
    pub(crate) fn spawn(cmd: &mut Command) -> io::Result<Self> {
        #[cfg(windows)]
        {
            win::spawn(cmd)
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
            Ok(Self {
                child: cmd.spawn()?,
            })
        }
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    #[cfg(test)]
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Kill every process in the tree. Idempotent; never blocks.
    pub(crate) fn kill(&self) {
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(unix)]
        {
            // SAFETY: plain syscall; the group id is the worker's pid (it leads the
            // group). An empty group gives ESRCH, which is fine.
            unsafe {
                libc::killpg(self.child.id() as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    /// Wait for the root process (after [`kill`](Self::kill), returns promptly). On
    /// Windows, also wait (up to 5 s) until every process in the job has exited:
    /// termination is asynchronous there, and a dying child still holds its working
    /// directory (the run directory) open.
    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait()?;
        #[cfg(windows)]
        self.job.wait_empty(std::time::Duration::from_secs(5));
        Ok(status)
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        // No orphans, whatever path got us here. (On Windows, closing the job handle
        // kills too: KILL_ON_JOB_CLOSE.)
        self.kill();
        let _ = self.child.wait();
    }
}

/// How a worker process ended, in words.
pub(crate) fn describe_exit(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("was killed by signal {sig}");
        }
    }
    match status.code() {
        // NTSTATUS crash codes (0xC0000005 access violation, 0xC0000409 fast fail…).
        Some(c) if cfg!(windows) && c < 0 => format!("crashed (exit code {:#010X})", c as u32),
        Some(c) => format!("exited with code {c}"),
        None => "ended without an exit code".into(),
    }
}

/// Whether a process with this id is running (zombies count as gone). For tests and
/// diagnostics: asserting that a cancelled worker left no process behind.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        win::pid_alive(pid)
    }
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // State is the field after the parenthesised command name.
            Ok(stat) => stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .is_some_and(|state| state != "Z" && state != "X"),
            Err(_) => false,
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // SAFETY: signal 0 only checks for existence and permission.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_SYNCHRONIZE,
        ResumeThread, THREAD_SUSPEND_RESUME, WaitForSingleObject,
    };

    use super::ProcessTree;

    /// An owned Job Object handle, plus handles to the processes a kill is ending.
    #[derive(Debug)]
    pub(super) struct Job {
        handle: HANDLE,
        /// Process handles (as `usize`) opened by `terminate`, waited on by `wait_empty`.
        dying: Mutex<Vec<usize>>,
    }

    // SAFETY: kernel handles are usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    /// Room for this many process ids per query; a worker tree is far smaller.
    const MAX_IDS: usize = 1024;

    /// `JOBOBJECT_BASIC_PROCESS_ID_LIST` with room for `MAX_IDS` ids.
    #[repr(C)]
    struct IdList {
        assigned: u32,
        listed: u32,
        ids: [usize; MAX_IDS],
    }

    impl Job {
        fn new() -> io::Result<Self> {
            // SAFETY: null attributes and name are allowed; the handle is owned below.
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Job {
                handle: h,
                dying: Mutex::default(),
            };
            // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is valid.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `info` is the struct this information class expects, with its size.
            let ok = unsafe {
                SetInformationJobObject(
                    job.handle,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        /// The ids of the processes in the job now.
        fn process_ids(&self) -> Vec<u32> {
            let mut list = Box::new(IdList {
                assigned: 0,
                listed: 0,
                ids: [0; MAX_IDS],
            });
            // SAFETY: valid job handle; the buffer is a JOBOBJECT_BASIC_PROCESS_ID_LIST
            // with room for MAX_IDS ids, and its size is passed.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicProcessIdList,
                    (&raw mut *list).cast(),
                    size_of::<IdList>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Vec::new();
            }
            let n = (list.listed as usize).min(MAX_IDS);
            list.ids[..n].iter().map(|&id| id as u32).collect()
        }

        /// Kill every process in the job. Handles to them are kept first, so
        /// [`wait_empty`](Self::wait_empty) can wait until each has fully exited.
        pub(super) fn terminate(&self) {
            let mut dying = self.dying.lock().unwrap_or_else(|e| e.into_inner());
            for pid in self.process_ids() {
                // SAFETY: plain call; a null handle (already gone) is skipped.
                let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                if !h.is_null() {
                    dying.push(h as usize);
                }
            }
            // SAFETY: valid job handle; terminating an empty job is a no-op.
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }

        /// Wait (up to `timeout` in all) until every process `terminate` killed has
        /// fully exited. A process's exit code, and the job's process count, are set
        /// before it finishes tearing down; only its signalled handle means its own
        /// handles, its working directory (the run directory) included, are closed.
        pub(super) fn wait_empty(&self, timeout: std::time::Duration) {
            let deadline = std::time::Instant::now() + timeout;
            let dying = std::mem::take(&mut *self.dying.lock().unwrap_or_else(|e| e.into_inner()));
            for h in dying {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                // SAFETY: we own `h` (opened in `terminate`) and close it here.
                unsafe {
                    WaitForSingleObject(h as HANDLE, left.as_millis() as u32);
                    CloseHandle(h as HANDLE);
                }
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            let dying = std::mem::take(self.dying.get_mut().unwrap_or_else(|e| e.into_inner()));
            // SAFETY: we own every handle. KILL_ON_JOB_CLOSE kills what is left.
            unsafe {
                for h in dying {
                    CloseHandle(h as HANDLE);
                }
                CloseHandle(self.handle);
            }
        }
    }

    pub(super) fn spawn(cmd: &mut Command) -> io::Result<ProcessTree> {
        let job = Job::new()?;
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
        let mut child = cmd.spawn()?;
        // SAFETY: both handles are valid; the child is suspended, so it has run no code
        // and cannot have spawned anything outside the job yet.
        let assigned =
            unsafe { AssignProcessToJobObject(job.handle, child.as_raw_handle() as HANDLE) };
        let started = if assigned == 0 {
            Err(io::Error::last_os_error())
        } else {
            resume(child.id())
        };
        if let Err(e) = started {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        Ok(ProcessTree { child, job })
    }

    /// Resume the threads of a process created suspended (it has exactly one).
    fn resume(pid: u32) -> io::Result<()> {
        // SAFETY: snapshot handle checked and closed below; THREADENTRY32 is plain data
        // with its size set as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut resumed = 0;
            let mut more = Thread32First(snap, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !t.is_null() {
                        if ResumeThread(t) != u32::MAX {
                            resumed += 1;
                        }
                        CloseHandle(t);
                    }
                }
                more = Thread32Next(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
            if resumed == 0 {
                return Err(io::Error::other(format!(
                    "could not resume worker process {pid}"
                )));
            }
        }
        Ok(())
    }

    pub(super) fn pid_alive(pid: u32) -> bool {
        // SAFETY: the handle is checked and closed. A process counts as alive until its
        // handle is signalled: fully exited, its own handles (cwd included) closed.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if h.is_null() {
                // No such process; any other failure (access denied) means it exists.
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
            CloseHandle(h);
            running
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_a_reaped_one_is_not() {
        assert!(pid_alive(std::process::id()));
        // A process that has exited and been waited for.
        let mut c = if cfg!(windows) {
            Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap()
        } else {
            Command::new("true").spawn().unwrap()
        };
        let pid = c.id();
        c.wait().unwrap();
        assert!(!pid_alive(pid));
    }

    #[test]
    fn killing_a_tree_ends_its_root() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        let mut tree = ProcessTree::spawn(&mut cmd).unwrap();
        let pid = tree.pid();
        assert!(pid_alive(pid));
        tree.kill();
        let status = tree.wait().unwrap();
        assert!(!status.success());
        assert!(!pid_alive(pid));
        assert!(!describe_exit(status).is_empty());
    }
}
