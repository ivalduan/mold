//! Executor profiles (ADR-0009 §4): what an executor key in a recipe means on this
//! machine. They live in user/site configuration, never in the recipe, so recipes
//! carry no hostnames, paths or credentials.
//!
//! ```toml
//! # ~/.mold/executors.toml (and optionally a site file, read first)
//! [executor.local]            # overrides the built-in `local`
//! slots = 8
//! gpus = 1
//! mem_gb = 64
//!
//! [executor.wsl]              # the local executor through `wsl.exe` (ADR-0005)
//! slots = 4
//! gpus = 1
//! wsl = { distro = "Ubuntu-24.04" }   # `wsl = {}` = the default distro
//!
//! [executor.gpu-box]
//! type = "ssh"
//! host = "gpu-box.lab.example"   # resolved by the user's own ~/.ssh/config
//! user = "alice"                 # optional; the config's user otherwise
//! port = 2222                    # optional
//! slots = 8
//! gpus = 2
//! run_root = "/scratch/mold"     # where run directories are staged (default ~/.mold/runs)
//! # shared_fs = "/net/ws/runs"   # this machine's run root as the host sees it: skip staging
//! # ssh_program = "ssh"          # the client to run; "double quotes" group words
//! # scp_program = "scp"
//! # connect_timeout_s = 15
//! # keep_run_dir = false         # true: never delete a remote run directory
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ConfigError;

/// The key that always exists, even with no profile file.
pub const LOCAL: &str = "local";

/// Environment variable naming an optional site profile file (read before the user's).
pub const SITE_FILE_VAR: &str = "MOLD_EXECUTORS_SITE";

/// Where profiles come from. Injectable, so tests never read the real home directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileSources {
    /// Site file, read first. Named explicitly, so it must exist.
    pub site: Option<PathBuf>,
    /// User file, read second (a key here replaces the site's). May be missing.
    pub user: Option<PathBuf>,
}

impl ProfileSources {
    /// `~/.mold/executors.toml` (home = `USERPROFILE` on Windows, `HOME` elsewhere)
    /// and the site file named by [`SITE_FILE_VAR`], if set.
    pub fn from_env() -> Self {
        let home = if cfg!(windows) {
            std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        } else {
            std::env::var_os("HOME")
        };
        Self {
            site: std::env::var_os(SITE_FILE_VAR)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            user: home.map(|h| PathBuf::from(h).join(".mold").join("executors.toml")),
        }
    }
}

/// Where a profile was defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    File(PathBuf),
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuiltIn => f.write_str("built in"),
            Self::File(p) => write!(f, "{}", p.display()),
        }
    }
}

/// Run through WSL2 (ADR-0005): the local executor with a `wsl.exe` prefix.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wsl {
    /// `None` = the default distribution.
    pub distro: Option<String>,
}

/// A remote host over SSH, run by [`SshExecutor`](crate::SshExecutor) through the
/// system `ssh` and `scp` clients (ADR-0009 §2-4), so the user's own `~/.ssh/config`
/// and agent apply and no credential ever passes through Mold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// Remote directory under which run directories are staged.
    /// `None` = [`DEFAULT_RUN_ROOT`].
    pub run_root: Option<String>,
    /// This machine's run root as the host sees it (a shared filesystem): staging is
    /// skipped and the worker runs in the run directory itself. An optimisation, not
    /// a requirement (ADR-0009 §3).
    pub shared_fs: Option<String>,
    /// The `ssh` client: program plus fixed arguments (`ssh_program`). Default `ssh`.
    pub ssh: Vec<String>,
    /// The `scp` client, used for staging (`scp_program`). Default `scp`.
    pub scp: Vec<String>,
    /// `ConnectTimeout` for every `ssh`/`scp` this executor runs.
    pub connect_timeout_s: u32,
    /// Keep remote run directories even after an item succeeds (for inspection).
    pub keep_run_dir: bool,
}

/// Where run directories go on a host whose profile names no `run_root`. Relative,
/// so the login shell resolves it in the remote home directory.
pub const DEFAULT_RUN_ROOT: &str = ".mold/runs";

/// `ConnectTimeout` when a profile names none.
pub const DEFAULT_CONNECT_TIMEOUT_S: u32 = 15;

impl SshTarget {
    /// `user@host`, or `host` when the profile names no user (`~/.ssh/config` may).
    pub fn destination(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// The remote directory run directories are staged under.
    pub fn root(&self) -> &str {
        self.run_root.as_deref().unwrap_or(DEFAULT_RUN_ROOT)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Subprocesses on this machine, optionally through WSL2.
    Local {
        wsl: Option<Wsl>,
    },
    Ssh(SshTarget),
}

/// One executor key's meaning on this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub key: String,
    /// Concurrent slots (≈ cores). Default: this machine's parallelism.
    pub slots: u32,
    /// GPUs. Default 0: an item needing a GPU fails until the profile declares one.
    pub gpus: u32,
    /// Memory for items, in GB; `None` = not accounted.
    pub mem_gb: Option<f64>,
    /// Data sent to this executor leaves the machine; the UI must say so first.
    pub remote: bool,
    pub target: Target,
    pub origin: Origin,
}

impl Profile {
    /// The built-in `local` profile.
    pub fn builtin_local() -> Self {
        Self {
            key: LOCAL.into(),
            slots: default_slots(),
            gpus: 0,
            mem_gb: None,
            remote: false,
            target: Target::Local { wsl: None },
            origin: Origin::BuiltIn,
        }
    }
}

fn default_slots() -> u32 {
    std::thread::available_parallelism().map_or(4, |n| n.get() as u32)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    executor: BTreeMap<String, Raw>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    #[default]
    Local,
    Ssh,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(rename = "type", default)]
    kind: Kind,
    slots: Option<u32>,
    gpus: Option<u32>,
    mem_gb: Option<f64>,
    remote: Option<bool>,
    wsl: Option<Wsl>,
    host: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    run_root: Option<String>,
    shared_fs: Option<String>,
    ssh_program: Option<String>,
    scp_program: Option<String>,
    connect_timeout_s: Option<u32>,
    keep_run_dir: Option<bool>,
}

/// Every executor key this machine defines.
#[derive(Debug, Clone, PartialEq)]
pub struct Profiles {
    map: BTreeMap<String, Profile>,
    user_file: Option<PathBuf>,
}

impl Default for Profiles {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Profiles {
    /// Only the built-in `local` profile.
    pub fn builtin() -> Self {
        Self {
            map: BTreeMap::from([(LOCAL.to_string(), Profile::builtin_local())]),
            user_file: None,
        }
    }

    /// The built-ins, then the site file, then the user file. A missing user file is
    /// fine (most machines have none); a named site file must exist.
    pub fn load(sources: &ProfileSources) -> Result<Self, ConfigError> {
        let mut p = Self::builtin();
        if let Some(site) = &sources.site {
            p.read(site)?;
        }
        if let Some(user) = &sources.user {
            if user.is_file() {
                p.read(user)?;
            }
            p.user_file = Some(user.clone());
        }
        Ok(p)
    }

    fn read(&mut self, path: &Path) -> Result<(), ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Profiles {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        self.add_toml(&text, path)
    }

    /// Layer one file's profiles on top: a key defined here replaces any earlier one.
    pub fn add_toml(&mut self, text: &str, path: &Path) -> Result<(), ConfigError> {
        let err = |message: String| ConfigError::Profiles {
            path: path.to_path_buf(),
            message,
        };
        let file: File = toml::from_str(text).map_err(|e| err(e.to_string().trim().into()))?;
        for (key, raw) in file.executor {
            let profile = validate(&key, raw, Origin::File(path.to_path_buf()))
                .map_err(|m| err(format!("[executor.{key}]: {m}")))?;
            self.map.insert(key, profile);
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Profile> {
        self.map.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Profile> {
        self.map.values()
    }

    /// The user profile file, if one was configured (where a missing key goes).
    pub fn user_file(&self) -> Option<&Path> {
        self.user_file.as_deref()
    }

    /// The profile for `key`, or the "unknown executor" error naming `used_by`.
    pub fn resolve(&self, key: &str, used_by: Vec<String>) -> Result<&Profile, ConfigError> {
        self.get(key).ok_or_else(|| ConfigError::UnknownExecutor {
            key: key.to_string(),
            used_by,
            known: self
                .map
                .values()
                .map(|p| format!("`{}` ({})", p.key, p.origin))
                .collect(),
            user_file: self.user_file.clone(),
        })
    }
}

fn validate(key: &str, raw: Raw, origin: Origin) -> Result<Profile, String> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("a key is letters, digits, `-` or `_`".into());
    }
    let slots = raw.slots.unwrap_or_else(default_slots);
    if slots == 0 {
        return Err("`slots` must be at least 1".into());
    }
    if let Some(m) = raw.mem_gb
        && !(m.is_finite() && m > 0.0)
    {
        return Err(format!("`mem_gb` must be a positive number, not {m}"));
    }
    let ssh_fields = [
        ("host", raw.host.is_some()),
        ("user", raw.user.is_some()),
        ("port", raw.port.is_some()),
        ("run_root", raw.run_root.is_some()),
        ("shared_fs", raw.shared_fs.is_some()),
        ("ssh_program", raw.ssh_program.is_some()),
        ("scp_program", raw.scp_program.is_some()),
        ("connect_timeout_s", raw.connect_timeout_s.is_some()),
        ("keep_run_dir", raw.keep_run_dir.is_some()),
    ];
    let (target, remote) = match raw.kind {
        Kind::Local => {
            if let Some((f, _)) = ssh_fields.iter().find(|(_, set)| *set) {
                return Err(format!("`{f}` is for `type = \"ssh\"` profiles"));
            }
            (Target::Local { wsl: raw.wsl }, raw.remote.unwrap_or(false))
        }
        Kind::Ssh => {
            if raw.wsl.is_some() {
                return Err("`wsl` is for local profiles".into());
            }
            if raw.remote == Some(false) {
                return Err("an ssh profile is always remote: data leaves this machine".into());
            }
            let host = raw
                .host
                .filter(|h| !h.trim().is_empty())
                .ok_or("an ssh profile needs `host`")?;
            // A `host` that looks like an option or a destination would land in ssh's
            // argv; refuse it here rather than build a surprising command line.
            if host.starts_with('-') || host.contains([' ', '\t', '\n', ':', '/']) {
                return Err(format!(
                    "`host` {host:?} must be a host name or alias from ~/.ssh/config, \
                     with no option, path or port in it"
                ));
            }
            if let Some(u) = &raw.user
                && (u.trim().is_empty() || u.contains(['@', ' ', '\t', '\n']))
            {
                return Err(format!("`user` {u:?} is not a user name"));
            }
            for (field, path) in [
                ("run_root", raw.run_root.as_ref()),
                ("shared_fs", raw.shared_fs.as_ref()),
            ] {
                if path.is_some_and(|p| p.trim().is_empty() || p.contains(['\n', '\'']))
                    || path.is_some_and(|p| p.ends_with('/'))
                {
                    return Err(format!(
                        "`{field}` must be a remote path with no trailing slash, \
                         newline or quote"
                    ));
                }
            }
            let client = |field: &str, raw: Option<String>, default: &str| {
                let text = raw.unwrap_or_else(|| default.to_string());
                match crate::manifest::split_words(&text) {
                    Ok(words) if words.is_empty() => Err(format!("`{field}` is empty")),
                    Ok(words) => Ok(words),
                    Err(_) => Err(format!("`{field}` has an unclosed quote")),
                }
            };
            let timeout = raw.connect_timeout_s.unwrap_or(DEFAULT_CONNECT_TIMEOUT_S);
            if timeout == 0 {
                return Err("`connect_timeout_s` must be at least 1".into());
            }
            let target = Target::Ssh(SshTarget {
                host,
                user: raw.user,
                port: raw.port,
                run_root: raw.run_root,
                shared_fs: raw.shared_fs,
                ssh: client("ssh_program", raw.ssh_program, "ssh")?,
                scp: client("scp_program", raw.scp_program, "scp")?,
                connect_timeout_s: timeout,
                keep_run_dir: raw.keep_run_dir.unwrap_or(false),
            });
            (target, true)
        }
    };
    Ok(Profile {
        key: key.to_string(),
        slots,
        gpus: raw.gpus.unwrap_or(0),
        mem_gb: raw.mem_gb,
        remote,
        target,
        origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Profiles, ConfigError> {
        let mut p = Profiles::builtin();
        p.add_toml(text, Path::new("executors.toml"))?;
        Ok(p)
    }

    #[test]
    fn local_is_built_in_and_needs_no_file() {
        let p = Profiles::load(&ProfileSources {
            site: None,
            user: Some(PathBuf::from("/definitely/not/here/executors.toml")),
        })
        .unwrap();
        let local = p.get(LOCAL).unwrap();
        assert_eq!(local.origin, Origin::BuiltIn);
        assert!(local.slots >= 1);
        assert_eq!(local.gpus, 0);
        assert!(!local.remote);
        assert_eq!(local.target, Target::Local { wsl: None });
    }

    #[test]
    fn profiles_parse_local_wsl_and_ssh() {
        let p = parse(
            r#"
[executor.local]
slots = 8
gpus = 1
mem_gb = 64

[executor.wsl]
slots = 2
wsl = { distro = "Ubuntu-24.04" }

[executor.wsl-default]
wsl = {}

[executor.gpu-box]
type = "ssh"
host = "gpu-box.lab"
user = "alice"
port = 2222
gpus = 2
"#,
        )
        .unwrap();
        let local = p.get("local").unwrap();
        assert_eq!((local.slots, local.gpus, local.mem_gb), (8, 1, Some(64.0)));
        assert_eq!(local.origin, Origin::File("executors.toml".into()));
        let wsl = p.get("wsl").unwrap();
        assert_eq!(
            wsl.target,
            Target::Local {
                wsl: Some(Wsl {
                    distro: Some("Ubuntu-24.04".into())
                })
            }
        );
        assert_eq!(
            p.get("wsl-default").unwrap().target,
            Target::Local {
                wsl: Some(Wsl::default())
            }
        );
        let ssh = p.get("gpu-box").unwrap();
        assert!(ssh.remote, "ssh is always remote");
        let Target::Ssh(t) = &ssh.target else {
            panic!("ssh target")
        };
        assert_eq!(
            (t.host.as_str(), t.user.as_deref(), t.port),
            ("gpu-box.lab", Some("alice"), Some(2222))
        );
    }

    #[test]
    fn invalid_profiles_name_the_key_and_the_problem() {
        for (text, want) in [
            ("[executor.a]\nslots = 0\n", "slots"),
            ("[executor.a]\nmem_gb = -1.0\n", "mem_gb"),
            ("[executor.a]\nslot = 2\n", "slot"),
            ("[executor.a]\nhost = \"h\"\n", "ssh"),
            ("[executor.a]\ntype = \"ssh\"\n", "host"),
            (
                "[executor.a]\ntype = \"ssh\"\nhost = \"h\"\nremote = false\n",
                "remote",
            ),
            (
                "[executor.a]\ntype = \"ssh\"\nhost = \"h\"\nwsl = {}\n",
                "wsl",
            ),
            ("[executor.a]\ntype = \"slurm\"\n", "slurm"),
            ("[executor.\"a b\"]\n", "key"),
            ("[executors.a]\n", "executors"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(want), "{e:?} should mention {want:?}");
            assert!(e.contains("executors.toml"), "{e:?} names the file");
        }
    }

    #[test]
    fn the_user_file_overrides_the_site_file_per_key() {
        let dir = std::env::temp_dir().join(format!("mold-jobs-profiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let site = dir.join("site.toml");
        let user = dir.join("user.toml");
        std::fs::write(&site, "[executor.a]\nslots = 2\n[executor.b]\nslots = 3\n").unwrap();
        std::fs::write(&user, "[executor.b]\nslots = 5\n").unwrap();
        let p = Profiles::load(&ProfileSources {
            site: Some(site.clone()),
            user: Some(user.clone()),
        })
        .unwrap();
        let missing_site = Profiles::load(&ProfileSources {
            site: Some(dir.join("nope.toml")),
            user: None,
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(p.get("a").unwrap().slots, 2);
        assert_eq!(p.get("b").unwrap().slots, 5);
        assert_eq!(p.get("b").unwrap().origin, Origin::File(user.clone()));
        assert_eq!(p.user_file(), Some(user.as_path()));
        assert!(missing_site.is_err(), "a named site file must exist");
    }

    #[test]
    fn an_unknown_key_says_where_to_define_it() {
        let mut p = Profiles::builtin();
        p.user_file = Some(PathBuf::from("/home/me/.mold/executors.toml"));
        let e = p
            .resolve("gpu-box", vec!["node `cofold-1`".into()])
            .unwrap_err()
            .to_string();
        for want in [
            "unknown executor `gpu-box`",
            "node `cofold-1`",
            "`local` (built in)",
            "[executor.gpu-box]",
            "executors.toml",
        ] {
            assert!(e.contains(want), "{e:?} should mention {want:?}");
        }
    }
}
