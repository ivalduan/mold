//! The shared parts of the `ssh`/`scp` **test doubles** (`mock-ssh`, `mock-scp`):
//! `ssh`-shaped stand-ins that let the SSH executor's whole path — staging, the
//! remote wrapper script, the remote process-group kill, the failure modes — be
//! tested with no network, no `sshd` and no second machine.
//!
//! Two deliberate choices keep those tests free of environment variables, control
//! files and sleeps:
//!
//! - **the host name is the switch.** A profile's `host` says what the double should
//!   pretend ([`HostMode`]), so failure injection is just configuration.
//! - **the "remote" command runs in a real POSIX shell** ([`posix_shell`]), and the
//!   profile's `run_root` is a real local directory. So the wrapper script is
//!   executed as written, with real process groups — the double stands in for the
//!   transport, not for the remote side's semantics.
//!
//! Nothing here is used by the worker itself. It lives in this package because only
//! its own integration tests see `CARGO_BIN_EXE_*` for all three binaries.

use std::path::{Path, PathBuf};

/// How many stdout lines [`HostMode::Drop`] forwards before it breaks the
/// connection. A count, not a delay: the test must not race the worker.
pub const DROP_AFTER: usize = 2;

/// `mock-ssh` found no POSIX shell: the test skips rather than fails.
pub const NO_SHELL: i32 = 70;

/// The "remote" command was killed by a signal, which has no exit code.
pub const SIGNALLED: i32 = 137;

/// What a destination asks the double to pretend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostMode {
    /// Run the command (any host name without a prefix below).
    Ok,
    /// `ssh: Could not resolve hostname …`; exit 255, as `ssh` reports its own
    /// failures.
    Unreachable,
    /// `Permission denied (publickey).`; exit 255.
    Denied,
    /// Forward this many lines of the worker's stdout, then die like a dropped
    /// connection, leaving the "remote" side running.
    Drop(usize),
}

impl HostMode {
    /// From a `[user@]host` destination (or a `host:path` argument's host).
    pub fn of(destination: &str) -> Self {
        let host = destination.rsplit('@').next().unwrap_or(destination);
        if host.starts_with("unreachable") {
            Self::Unreachable
        } else if host.starts_with("denied") {
            Self::Denied
        } else if host.starts_with("drop") {
            Self::Drop(DROP_AFTER)
        } else {
            Self::Ok
        }
    }

    /// `ssh`'s message and exit code for a connection that never happened.
    pub fn refuse(self, destination: &str) -> Option<(String, i32)> {
        match self {
            Self::Unreachable => Some((
                format!("ssh: Could not resolve hostname {destination}: Name or service not known"),
                255,
            )),
            Self::Denied => Some((
                format!("{destination}: Permission denied (publickey,keyboard-interactive)."),
                255,
            )),
            Self::Ok | Self::Drop(_) => None,
        }
    }
}

/// Split an `scp` argument into its host and its path. A single-letter prefix is a
/// Windows drive, not a host, and a path may itself hold a drive
/// (`box:C:/runs/x`) — exactly `scp`'s own rule.
pub fn split_host(arg: &str) -> (Option<&str>, &str) {
    match arg.split_once(':') {
        Some((host, path)) if host.len() > 1 && !host.contains(['/', '\\']) => (Some(host), path),
        _ => (None, arg),
    }
}

/// A POSIX shell to run a "remote" command with: `$MOLD_TEST_SH`, then `sh` on
/// `PATH`, then `/bin/sh`, then the shell Git for Windows ships (every machine that
/// cloned this repository has one, CI included).
pub fn posix_shell() -> Option<PathBuf> {
    if let Some(sh) = std::env::var_os("MOLD_TEST_SH").filter(|s| !s.is_empty()) {
        let p = PathBuf::from(sh);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = on_path("sh") {
        return Some(p);
    }
    let bin = Path::new("/bin/sh");
    if bin.is_file() {
        return Some(bin.to_path_buf());
    }
    if cfg!(windows) {
        // `C:\Program Files\Git\cmd\git.exe` → `C:\Program Files\Git\usr\bin\sh.exe`.
        let roots = on_path("git")
            .and_then(|g| g.parent().and_then(Path::parent).map(Path::to_path_buf))
            .into_iter()
            .chain([
                PathBuf::from(r"C:\Program Files\Git"),
                PathBuf::from(r"C:\Program Files (x86)\Git"),
            ]);
        for root in roots {
            for rel in ["usr/bin/sh.exe", "bin/sh.exe"] {
                let p = root.join(rel);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// `program` (plus a Windows executable extension) in `PATH`.
pub fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{program}.exe"), program.to_string()]
    } else {
        vec![program.to_string()]
    };
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

/// Copy `src` (a file or a directory) into the existing directory `dest_dir`, under
/// its own name, merging into whatever is already there: `scp`'s behaviour, and the
/// only one the SSH executor relies on.
pub fn copy_into(src: &Path, dest_dir: &Path) -> std::io::Result<()> {
    let name = src.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} has no file name", src.display()),
        )
    })?;
    copy_tree(src, &dest_dir.join(name))
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dest.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dest).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_name_selects_what_the_double_pretends() {
        assert_eq!(HostMode::of("gpu-box"), HostMode::Ok);
        assert_eq!(HostMode::of("me@gpu-box"), HostMode::Ok);
        assert_eq!(HostMode::of("me@unreachable-1"), HostMode::Unreachable);
        assert_eq!(HostMode::of("denied"), HostMode::Denied);
        assert_eq!(HostMode::of("drop"), HostMode::Drop(DROP_AFTER));
        assert_eq!(HostMode::of("ok").refuse("ok"), None);
        assert_eq!(HostMode::of("denied").refuse("x").unwrap().1, 255);
    }

    #[test]
    fn an_scp_argument_splits_into_host_and_path() {
        assert_eq!(split_host("box:/runs/x"), (Some("box"), "/runs/x"));
        assert_eq!(split_host("box:C:/runs/x"), (Some("box"), "C:/runs/x"));
        assert_eq!(split_host(r"C:\runs\x"), (None, r"C:\runs\x"));
        assert_eq!(split_host("C:/runs/x"), (None, "C:/runs/x"));
        assert_eq!(split_host("ligands.sdf"), (None, "ligands.sdf"));
    }
}
