//! `scp`-shaped **test double** for the SSH executor's staging
//! (crates/jobs/src/ssh.rs): it takes the command line the executor builds and
//! copies the files on this machine, so a staging round trip is tested with no
//! network and no `sshd`.
//!
//! ```text
//! mock-scp -q -B -r -o ConnectTimeout=15 [-P PORT] SOURCE… DEST
//! ```
//!
//! A `host:path` argument is read exactly as `scp` reads it (a single letter is a
//! Windows drive, not a host), and the host name says what to pretend
//! ([`HostMode`]). `DEST` must be an existing directory: that is the only shape the
//! SSH executor uses, and `scp` itself fails otherwise.

use mold_mock_cofold::double::{HostMode, copy_into, split_host};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = match parse(&args) {
        Ok(p) => p,
        Err(e) => die(&e, 2),
    };
    let (dest, sources) = paths.split_last().expect("parse checked");
    for arg in &paths {
        let (host, _) = split_host(arg);
        if let Some(h) = host
            && let Some((message, code)) = HostMode::of(h).refuse(h)
        {
            die(&message, code);
        }
    }
    let dest = split_host(dest).1;
    if !std::path::Path::new(dest).is_dir() {
        die(&format!("mock-scp: {dest}: No such file or directory"), 1);
    }
    for source in sources {
        let path = std::path::Path::new(split_host(source).1);
        if let Err(e) = copy_into(path, std::path::Path::new(dest)) {
            die(&format!("mock-scp: {}: {e}", path.display()), 1);
        }
    }
}

/// The sources and the destination, in order, with the options dropped.
fn parse(args: &[String]) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "-o" | "-P" | "-i" | "-F" | "-l" | "-J" | "-c" => {
                rest.next()
                    .ok_or_else(|| format!("mock-scp: {a} needs a value"))?;
            }
            "-q" | "-B" | "-r" | "-p" | "-C" | "-v" | "-3" | "-O" | "-s" | "-4" | "-6" => {}
            other if other.starts_with('-') => {
                return Err(format!("mock-scp: unexpected option {other}"));
            }
            other => paths.push(other.to_string()),
        }
    }
    if paths.len() < 2 {
        return Err("mock-scp: needs at least one source and a destination".into());
    }
    Ok(paths)
}

fn die(message: &str, code: i32) -> ! {
    eprintln!("{message}");
    std::process::exit(code);
}
