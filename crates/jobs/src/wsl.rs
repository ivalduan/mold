//! WSL2 (ADR-0005): the local executor with a `wsl.exe` prefix. Path translation
//! lives here and nowhere else.
//!
//! Only the run directory is translated: the `JobSpec` and every event path are
//! relative to it, so they cross unchanged.

use std::ffi::OsString;
use std::path::Path;

use crate::profile::Wsl;

/// The Linux path WSL2 sees for a Windows path.
///
/// - `C:\Users\me\run` (or `C:/…`, or verbatim `\\?\C:\…`) → `/mnt/c/Users/me/run`
/// - `\\wsl$\Ubuntu\home\me` or `\\wsl.localhost\Ubuntu\home\me` → `/home/me`
///
/// Other UNC shares and relative paths have no WSL path and are refused.
pub fn wsl_path(windows_path: &str) -> Result<String, String> {
    let p = windows_path.replace('/', "\\");
    let p = match p.strip_prefix(r"\\?\") {
        Some(rest) => match rest.strip_prefix(r"UNC\") {
            Some(unc) => format!(r"\\{unc}"),
            None => rest.to_string(),
        },
        None => p,
    };
    let join = |parts: &mut dyn Iterator<Item = &str>| -> String {
        parts
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    };
    if let Some(unc) = p.strip_prefix(r"\\") {
        let mut parts = unc.split('\\');
        let server = parts.next().unwrap_or_default().to_ascii_lowercase();
        if server == "wsl$" || server == "wsl.localhost" {
            let _distro = parts
                .next()
                .filter(|d| !d.is_empty())
                .ok_or_else(|| format!("{windows_path:?} names no WSL distribution"))?;
            return Ok(format!("/{}", join(&mut parts)));
        }
        return Err(format!(
            "{windows_path:?} is a network share; WSL2 cannot see it under /mnt"
        ));
    }
    let bytes = p.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = &p[2..];
        if !rest.is_empty() && !rest.starts_with('\\') {
            return Err(format!("{windows_path:?} is drive-relative"));
        }
        let tail = join(&mut rest.split('\\'));
        return Ok(if tail.is_empty() {
            format!("/mnt/{drive}")
        } else {
            format!("/mnt/{drive}/{tail}")
        });
    }
    Err(format!("{windows_path:?} is not an absolute Windows path"))
}

/// The `wsl.exe` command line that runs `argv` in `run_dir` (a Windows path).
pub(crate) fn wsl_command(
    wsl: &Wsl,
    run_dir: &Path,
    argv: &[String],
) -> Result<(OsString, Vec<OsString>), String> {
    let cd = wsl_path(&run_dir.to_string_lossy())?;
    let mut args: Vec<OsString> = Vec::new();
    if let Some(d) = &wsl.distro {
        args.extend(["-d".into(), d.into()]);
    }
    args.extend(["--cd".into(), cd.into(), "--".into()]);
    args.extend(argv.iter().map(OsString::from));
    Ok(("wsl.exe".into(), args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_translate_to_wsl_paths() {
        for (win, linux) in [
            (r"C:\Users\me\runs\ab\0", "/mnt/c/Users/me/runs/ab/0"),
            ("D:/data/run", "/mnt/d/data/run"),
            (r"c:\", "/mnt/c"),
            (r"C:\Program Files\x\\y\", "/mnt/c/Program Files/x/y"),
            (r"\\?\C:\long\path", "/mnt/c/long/path"),
            (r"\\wsl$\Ubuntu\home\me\run", "/home/me/run"),
            (r"\\wsl.localhost\Ubuntu-24.04\tmp", "/tmp"),
            (r"\\?\UNC\wsl.localhost\Ubuntu\srv", "/srv"),
        ] {
            assert_eq!(wsl_path(win).as_deref(), Ok(linux), "{win}");
        }
    }

    #[test]
    fn untranslatable_paths_are_refused() {
        for (win, want) in [
            (r"\\fileserver\share\run", "network share"),
            (r"\\wsl$\", "distribution"),
            (r"runs\ab", "absolute"),
            (r"C:runs", "drive-relative"),
            ("/home/me", "absolute"),
        ] {
            let e = wsl_path(win).unwrap_err();
            assert!(e.contains(want), "{win}: {e}");
        }
    }

    #[test]
    fn the_command_line_runs_argv_in_the_translated_run_dir() {
        let wsl = Wsl {
            distro: Some("Ubuntu".into()),
        };
        let argv = ["python".to_string(), "-m".into(), "w".into()];
        let (prog, args) = wsl_command(&wsl, Path::new(r"C:\runs\ab\0"), &argv).unwrap();
        assert_eq!(prog, "wsl.exe");
        assert_eq!(
            args,
            [
                "-d",
                "Ubuntu",
                "--cd",
                "/mnt/c/runs/ab/0",
                "--",
                "python",
                "-m",
                "w"
            ]
            .map(OsString::from)
        );
        let (_, args) = wsl_command(&Wsl::default(), Path::new(r"C:\r"), &argv).unwrap();
        assert_eq!(args[0], "--cd");
    }
}
