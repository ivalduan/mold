//! Configuration errors: everything that must fail **before** any item runs.

use std::fmt;
use std::path::PathBuf;

/// A manifest, profile or executor key that is wrong. Raised while building
/// executors, so a cook never starts on a misconfigured machine.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// A worker manifest (`workers/<name>/manifest.toml`) is invalid.
    Manifest { path: PathBuf, message: String },
    /// An executor profile file is invalid.
    Profiles { path: PathBuf, message: String },
    /// The graph names an executor key this machine does not define.
    UnknownExecutor {
        key: String,
        /// Who asks for it: "node `cofold-1`", "the graph default".
        used_by: Vec<String>,
        /// Keys that are defined, with where each comes from.
        known: Vec<String>,
        /// The user profile file, where the key could be added.
        user_file: Option<PathBuf>,
    },
    /// The key's profile is valid but this build cannot run it yet.
    NotYet { key: String, what: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest { path, message } => {
                write!(f, "invalid worker manifest {}: {message}", path.display())
            }
            Self::Profiles { path, message } => {
                write!(
                    f,
                    "invalid executor profiles in {}: {message}",
                    path.display()
                )
            }
            Self::UnknownExecutor {
                key,
                used_by,
                known,
                user_file,
            } => {
                write!(
                    f,
                    "unknown executor `{key}` (asked for by {}); this machine defines: {}",
                    used_by.join(", "),
                    known.join(", ")
                )?;
                match user_file {
                    Some(p) => write!(f, ". Define it as [executor.{key}] in {}", p.display()),
                    None => write!(
                        f,
                        ". Define it as [executor.{key}] in ~/.mold/executors.toml"
                    ),
                }
            }
            Self::NotYet { key, what } => write!(f, "executor `{key}`: {what}"),
        }
    }
}

impl std::error::Error for ConfigError {}
