//! Worker manifests (`workers/<name>/manifest.toml`): validated, never installed or
//! repaired. A manifest says what a worker is and how to launch it; the environment
//! it names must already exist on the target.
//!
//! ```toml
//! name = "boltz2"
//! executable = "python -m mold_worker_boltz2"  # argv; "double quotes" group words
//! environment = "conda:mold-boltz2"            # "none" | "conda:<env name>"
//! needs_gpu = true
//! model = "boltz-2"
//! model_version = "2.0.3"                      # optional, pinned deliberately
//! license = "MIT"                              # optional
//! ```

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mold_graph::Needs;
use serde::Deserialize;

use crate::error::ConfigError;

/// The file name inside a worker's directory.
pub const MANIFEST_FILE: &str = "manifest.toml";

/// Where a worker's executable runs. Declared only: Mold never creates or repairs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Environment {
    /// The executable runs as is.
    None,
    /// Inside a conda environment: launched as `conda run --no-capture-output -n <env> …`.
    Conda(String),
}

/// A validated worker manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// The worker's name: what `CommandJob::worker` refers to.
    pub name: String,
    /// The command line, split into words.
    pub executable: Vec<String>,
    pub environment: Environment,
    pub needs_gpu: bool,
    pub model: String,
    pub model_version: Option<String>,
    pub license: Option<String>,
    /// The directory the manifest was read from (relative executables resolve here).
    pub dir: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: String,
    executable: String,
    environment: String,
    needs_gpu: bool,
    model: String,
    model_version: Option<String>,
    license: Option<String>,
}

impl Manifest {
    /// Parse and validate a manifest's text. `dir` is the worker's directory.
    pub fn parse(text: &str, dir: &Path) -> Result<Self, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string().trim().to_string())?;
        if !valid_name(&raw.name) {
            return Err(format!(
                "`name` {:?} must be lowercase letters, digits, `-` or `_`",
                raw.name
            ));
        }
        let executable = split_words(&raw.executable)?;
        if executable.is_empty() {
            return Err("`executable` is empty".into());
        }
        let environment = match raw.environment.as_str() {
            "none" => Environment::None,
            e => match e.strip_prefix("conda:") {
                Some(env) if !env.trim().is_empty() && !env.contains(char::is_whitespace) => {
                    Environment::Conda(env.to_string())
                }
                _ => {
                    return Err(format!(
                        "`environment` {e:?} is not one of \"none\" or \"conda:<env name>\""
                    ));
                }
            },
        };
        for (field, value) in [
            ("model", Some(&raw.model)),
            ("model_version", raw.model_version.as_ref()),
            ("license", raw.license.as_ref()),
        ] {
            if value.is_some_and(|v| v.trim().is_empty()) {
                return Err(format!("`{field}` is empty"));
            }
        }
        Ok(Self {
            name: raw.name,
            executable,
            environment,
            needs_gpu: raw.needs_gpu,
            model: raw.model,
            model_version: raw.model_version,
            license: raw.license,
            dir: dir.to_path_buf(),
        })
    }

    /// Read and validate `path` (a `manifest.toml`).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let err = |message: String| ConfigError::Manifest {
            path: path.to_path_buf(),
            message,
        };
        let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        Self::parse(&text, dir).map_err(err)
    }

    /// What one item of this worker occupies: one slot, and a GPU if `needs_gpu`.
    /// For a worker node's [`Processor::needs`](mold_graph::Processor::needs).
    pub fn needs(&self) -> Needs {
        Needs {
            gpus: u32::from(self.needs_gpu),
            ..Needs::default()
        }
    }

    /// The full argv: the environment's prefix, then `executable`.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = match &self.environment {
            Environment::None => Vec::new(),
            Environment::Conda(env) => ["conda", "run", "--no-capture-output", "-n", env]
                .map(String::from)
                .to_vec(),
        };
        argv.extend(self.executable.iter().cloned());
        argv
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Split a command line on whitespace; `"double quotes"` group words (no escapes).
/// Used for a manifest's `executable` and for a profile's `ssh_program`/`scp_program`.
pub(crate) fn split_words(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    words.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if quoted {
        return Err("`executable` has an unclosed quote".into());
    }
    if any {
        words.push(cur);
    }
    Ok(words)
}

/// The workers this machine knows, by name, and where to find their executables.
#[derive(Debug, Clone, Default)]
pub struct Workers {
    manifests: BTreeMap<String, Manifest>,
    search_path: Vec<PathBuf>,
}

impl Workers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load every `<dir>/<name>/manifest.toml`. Any invalid manifest (or one whose
    /// `name` is not its directory's name) fails the whole load, naming every bad file.
    pub fn load_dir(dir: &Path) -> Result<Self, ConfigError> {
        let entries = std::fs::read_dir(dir).map_err(|e| ConfigError::Manifest {
            path: dir.to_path_buf(),
            message: format!("cannot read the workers directory: {e}"),
        })?;
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path().join(MANIFEST_FILE))
            .filter(|p| p.is_file())
            .collect();
        paths.sort();
        let mut workers = Self::new();
        let mut errors = Vec::new();
        for path in paths {
            let r = Manifest::load(&path).and_then(|m| {
                let dir_name = path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if m.name != dir_name {
                    return Err(ConfigError::Manifest {
                        path: path.clone(),
                        message: format!(
                            "`name` is {:?} but the directory is {dir_name:?}",
                            m.name
                        ),
                    });
                }
                workers.insert(m)
            });
            if let Err(e) = r {
                errors.push(e);
            }
        }
        match errors.len() {
            0 => Ok(workers),
            1 => Err(errors.remove(0)),
            _ => Err(ConfigError::Manifest {
                path: dir.to_path_buf(),
                message: errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            }),
        }
    }

    /// Add one manifest. A second worker with the same name is refused.
    pub fn insert(&mut self, m: Manifest) -> Result<(), ConfigError> {
        if let Some(old) = self.manifests.get(&m.name) {
            return Err(ConfigError::Manifest {
                path: m.dir.join(MANIFEST_FILE),
                message: format!(
                    "worker `{}` is already defined in {}",
                    m.name,
                    old.dir.display()
                ),
            });
        }
        self.manifests.insert(m.name.clone(), m);
        Ok(())
    }

    /// Directories searched first for a bare executable name (e.g. `target/debug`),
    /// before the manifest's own directory and `PATH`.
    pub fn with_search_path(mut self, dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        self.search_path.extend(dirs);
        self
    }

    pub fn get(&self, name: &str) -> Option<&Manifest> {
        self.manifests.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.manifests.keys().map(String::as_str)
    }

    /// Resolve a program name on this machine: a path with a directory part is taken
    /// relative to the manifest; a bare name is looked up in the search path, the
    /// manifest's directory, then `PATH`.
    pub(crate) fn resolve_program(&self, program: &str, m: &Manifest) -> Result<PathBuf, String> {
        let p = Path::new(program);
        if p.is_absolute() || p.components().count() > 1 {
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                m.dir.join(p)
            };
            return executable_in(&full)
                .ok_or_else(|| format!("executable {} does not exist", full.display()));
        }
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let dirs = self
            .search_path
            .iter()
            .cloned()
            .chain([m.dir.clone()])
            .chain(std::env::split_paths(&path_var));
        for dir in dirs {
            if let Some(found) = executable_in(&dir.join(program)) {
                return Ok(found);
            }
        }
        Err(format!(
            "executable `{program}` not found in the worker search path, {} or PATH",
            m.dir.display()
        ))
    }
}

/// `path` itself, or with a Windows executable extension, if it is a file.
fn executable_in(path: &Path) -> Option<PathBuf> {
    let mut candidates = vec![path.to_path_buf()];
    if cfg!(windows) && path.extension().is_none() {
        for ext in ["exe", "com", "bat", "cmd"] {
            let mut s = OsString::from(path.as_os_str());
            s.push(".");
            s.push(ext);
            candidates.insert(candidates.len() - 1, PathBuf::from(s));
        }
    }
    candidates.into_iter().find(|c| c.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCK: &str = r#"
name = "mock-cofold"
executable = "mock-cofold"
environment = "none"
needs_gpu = false
model = "fixture-replay"
"#;

    #[test]
    fn the_shipped_manifests_are_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers");
        let w = Workers::load_dir(&dir).unwrap();
        assert_eq!(
            w.names().collect::<Vec<_>>(),
            ["boltz2", "mock-cofold", "vina"]
        );
        let boltz = w.get("boltz2").unwrap();
        assert_eq!(boltz.environment, Environment::Conda("mold-boltz2".into()));
        assert_eq!(boltz.needs().gpus, 1);
        assert_eq!(
            boltz.argv(),
            [
                "conda",
                "run",
                "--no-capture-output",
                "-n",
                "mold-boltz2",
                "python",
                "-m",
                "mold_worker_boltz2"
            ]
        );
        assert_eq!(w.get("mock-cofold").unwrap().needs(), Needs::default());
    }

    #[test]
    fn invalid_manifests_say_what_is_wrong() {
        let dir = Path::new("w");
        let cases = [
            (MOCK.replace("needs_gpu = false\n", ""), "needs_gpu"),
            (MOCK.replace("\"none\"", "\"docker:x\""), "environment"),
            (MOCK.replace("\"none\"", "\"conda:\""), "environment"),
            (
                MOCK.replace("\"mock-cofold\"\nexec", "\"Mock Cofold\"\nexec"),
                "name",
            ),
            (
                MOCK.replace("executable = \"mock-cofold\"", "executable = \"  \""),
                "empty",
            ),
            (
                MOCK.replace("executable = \"mock-cofold\"", "executable = \"\\\"a b\""),
                "quote",
            ),
            (format!("{MOCK}gpu = true\n"), "gpu"),
            (
                MOCK.replace("needs_gpu = false", "needs_gpu = \"no\""),
                "needs_gpu",
            ),
            (MOCK.replace("\"fixture-replay\"", "\"\""), "model"),
        ];
        for (text, want) in cases {
            let e = Manifest::parse(&text, dir).unwrap_err();
            assert!(e.contains(want), "{e:?} should mention {want:?}");
        }
        Manifest::parse(MOCK, dir).unwrap();
    }

    #[test]
    fn executable_words_and_quotes() {
        assert_eq!(
            split_words(r#""C:\Program Files\w\w.exe" --flag  x"#).unwrap(),
            [r"C:\Program Files\w\w.exe", "--flag", "x"]
        );
        assert_eq!(split_words(r#"a "" b"#).unwrap(), ["a", "", "b"]);
    }

    #[test]
    fn a_bad_manifest_in_a_workers_dir_fails_the_load_and_names_the_file() {
        let root = std::env::temp_dir().join(format!("mold-jobs-manifests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, text) in [
            ("mock-cofold", MOCK.to_string()),
            ("broken", MOCK.replace("model = \"fixture-replay\"\n", "")),
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join(MANIFEST_FILE), text).unwrap();
        }
        let e = Workers::load_dir(&root).unwrap_err().to_string();
        let _ = std::fs::remove_dir_all(&root);
        assert!(e.contains("broken"), "{e}");
        assert!(e.contains("model"), "{e}");
    }
}
