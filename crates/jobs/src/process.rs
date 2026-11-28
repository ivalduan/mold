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

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
        STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, GetExitCodeProcess, OpenProcess, OpenThread,
        PROCESS_QUERY_LIMITED_INFORMATION, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    use super::ProcessTree;

    /// An owned Job Object handle.
    #[derive(Debug)]
    pub(super) struct Job(HANDLE);

    // SAFETY: a kernel handle is usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        fn new() -> io::Result<Self> {
            // SAFETY: null attributes and name are allowed; the handle is owned below.
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Job(h);
            // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is valid.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `info` is the struct this information class expects, with its size.
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
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

        /// Poll until the job has no active process, or `timeout` passes.
        pub(super) fn wait_empty(&self, timeout: std::time::Duration) {
            let t = std::time::Instant::now();
            loop {
                // SAFETY: an all-zero accounting struct is valid; the API fills it.
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                    unsafe { std::mem::zeroed() };
                // SAFETY: valid job handle; buffer and size match the class.
                let ok = unsafe {
                    QueryInformationJobObject(
                        self.0,
                        JobObjectBasicAccountingInformation,
                        (&raw mut info).cast(),
                        size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 || info.ActiveProcesses == 0 || t.elapsed() > timeout {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }

        pub(super) fn terminate(&self) {
            // SAFETY: valid job handle; terminating an empty job is a no-op.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: we own the handle. KILL_ON_JOB_CLOSE kills what is left.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub(super) fn spawn(cmd: &mut Command) -> io::Result<ProcessTree> {
        let job = Job::new()?;
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
        let mut child = cmd.spawn()?;
        // SAFETY: both handles are valid; the child is suspended, so it has run no code
        // and cannot have spawned anything outside the job yet.
        let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
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
        // SAFETY: the handle is checked and closed; the exit code is a plain out value.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                // No such process; any other failure (access denied) means it exists.
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == STILL_ACTIVE as u32
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
