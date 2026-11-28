//! `ssh`-shaped **test double** for the SSH executor (crates/jobs/src/ssh.rs): it
//! takes the command line the executor builds, enforces the parts of it that matter,
//! and runs the "remote" command in a real POSIX shell on this machine.
//!
//! ```text
//! mock-ssh -T -o BatchMode=yes -o ConnectTimeout=15 [-p PORT] [user@]host COMMAND…
//! ```
//!
//! - It **refuses** a command line without `-T` or `BatchMode=yes`: a pty would
//!   rewrite the worker's bytes, and a prompt would mean Mold handling credentials.
//! - The host name says what to pretend ([`HostMode`]): run it, be unreachable,
//!   refuse authentication, or drop the connection mid-stream.
//! - The command runs under a real `sh`, so the wrapper script's job control, pgid
//!   file, cancel sentinel and process-group kill are the real ones. The child is put
//!   in **its own process group**, so killing this client (as the executor's local
//!   process-tree kill does) leaves the "remote" side running — which is exactly the
//!   case the remote kill has to handle, and what this double exists to prove.
//! - `PATH` for the child starts with this binary's own directory, so the "remote"
//!   host has `mock-cofold` and nothing else of ours.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use mold_mock_cofold::double::{HostMode, NO_SHELL, SIGNALLED, posix_shell};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (destination, command, flags) = match parse(&args) {
        Ok(parsed) => parsed,
        Err(e) => die(&e, 2),
    };
    for want in ["-T", "BatchMode=yes"] {
        if !flags.iter().any(|f| f == want) {
            die(
                &format!("mock-ssh: the executor must pass {want} (got {flags:?})"),
                2,
            );
        }
    }
    let mode = HostMode::of(&destination);
    if let Some((message, code)) = mode.refuse(&destination) {
        die(&message, code);
    }
    let Some(sh) = posix_shell() else {
        die(
            "mock-ssh: no POSIX shell on this machine (set MOLD_TEST_SH)",
            NO_SHELL,
        )
    };

    let mut cmd = Command::new(&sh);
    cmd.arg("-c")
        .arg(&command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![dir.to_path_buf()];
        dirs.extend(std::env::split_paths(&path));
        if let Ok(joined) = std::env::join_paths(dirs) {
            cmd.env("PATH", joined);
        }
    }
    // The "remote" side must outlive this client being killed, like a real one.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => die(&format!("mock-ssh: cannot run {}: {e}", sh.display()), 255),
    };

    let mut out = child.stdout.take().expect("piped stdout");
    let limit = match mode {
        HostMode::Drop(after) => Some(after),
        _ => None,
    };
    // A `drop…` host breaks the connection only once it has carried that many lines,
    // so it hits the *run* (which streams events) and not the staging and control
    // commands (which print nothing and end first). No command sniffing, and nothing
    // for the test to race.
    let code = if relay(&mut out, limit) {
        eprintln!("mock-ssh: Connection to {destination} closed by remote host.");
        255
    } else {
        child
            .wait()
            .ok()
            .and_then(|s| s.code())
            .unwrap_or(SIGNALLED)
    };
    std::process::exit(code);
}

/// Copy the child's stdout to ours, byte for byte, flushing every line so the
/// executor sees events as they happen. `true` if it stopped at `limit` rather than
/// at end of output.
fn relay(out: &mut impl Read, limit: Option<usize>) -> bool {
    let mut reader = BufReader::new(out);
    let mut stdout = std::io::stdout().lock();
    let mut lines = 0;
    loop {
        if limit.is_some_and(|l| lines >= l) {
            return true;
        }
        let mut buf = Vec::new();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {
                lines += 1;
                if stdout
                    .write_all(&buf)
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    return false;
                }
            }
        }
    }
}

/// `(destination, command, every flag and option value seen)`. Like `ssh`: the first
/// non-option argument is the destination and the rest is the command, joined with
/// spaces.
type Parsed = (String, String, Vec<String>);

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut flags = Vec::new();
    let mut rest = args.iter();
    let mut destination = None;
    while let Some(a) = rest.next() {
        match a.as_str() {
            // Options that take a value, as the executor and a user's extra
            // arguments may use them.
            "-o" | "-p" | "-F" | "-i" | "-l" | "-J" => {
                flags.push(a.clone());
                let v = rest
                    .next()
                    .ok_or_else(|| format!("mock-ssh: {a} needs a value"))?;
                flags.push(v.clone());
            }
            "-T" | "-t" | "-q" | "-v" | "-n" | "-4" | "-6" | "-C" => flags.push(a.clone()),
            other if other.starts_with('-') => {
                return Err(format!("mock-ssh: unexpected option {other}"));
            }
            other => {
                destination = Some(other.to_string());
                break;
            }
        }
    }
    let destination = destination.ok_or("mock-ssh: no destination")?;
    let command: Vec<&str> = rest.map(String::as_str).collect();
    if command.is_empty() {
        return Err("mock-ssh: no command (this double never opens a session)".into());
    }
    Ok((destination, command.join(" "), flags))
}

fn die(message: &str, code: i32) -> ! {
    eprintln!("{message}");
    std::process::exit(code);
}
