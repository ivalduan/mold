//! The per-item result cache, keyed by item content hash.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::PortType;
use crate::codec::CodecError;
use crate::hash::ContentHash;
use crate::node::Outputs;

/// Where cooked item results are kept: [`MemoryCache`] (this process only) or
/// [`RunDirCache`](crate::RunDirCache) (memory first, then the run directory on disk).
pub trait Cache: Send + Sync + fmt::Debug {
    fn get(&self, key: &ContentHash) -> Option<Arc<Outputs>>;
    fn put(&self, key: ContentHash, outputs: Arc<Outputs>);

    /// Where the result for `key` lives, so a UI can mark items that will not survive a
    /// restart. The default suits a memory-only cache: [`Persistence::Memory`] if
    /// [`get`](Self::get) finds it.
    fn persistence(&self, key: &ContentHash) -> Persistence {
        if self.get(key).is_some() {
            Persistence::Memory
        } else {
            Persistence::Absent
        }
    }
}

/// Where a cached result lives. See [`Cache::persistence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persistence {
    /// Not cached.
    Absent,
    /// In memory, by design: the cache does not persist anything.
    Memory,
    /// On disk: a later process finds it.
    Disk,
    /// The cache persists results, but not this one: it is in memory only, and a later
    /// process re-cooks the item.
    NotPersisted(Unpersisted),
}

/// Why a result could not be persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unpersisted {
    /// No codec is registered for this port's type and payload type.
    NoCodec { port: String, ty: PortType },
    /// The codec refused the payload.
    Encode { port: String, error: CodecError },
    /// An artifact path is not valid UTF-8, so the entry could not record it.
    NonUtf8File(PathBuf),
    /// Writing the entry failed.
    Io(String),
}

impl fmt::Display for Unpersisted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCodec { port, ty } => {
                write!(f, "output `{port}`: no codec for its {ty:?} payload type")
            }
            Self::Encode { port, error } => write!(f, "output `{port}`: {error}"),
            Self::NonUtf8File(p) => write!(f, "artifact path {} is not UTF-8", p.display()),
            Self::Io(e) => write!(f, "write failed: {e}"),
        }
    }
}

/// Unbounded in-memory cache. Eviction is a follow-up.
#[derive(Debug, Default)]
pub struct MemoryCache {
    map: Mutex<HashMap<ContentHash, Arc<Outputs>>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.lock().expect("cache lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, key: &ContentHash) -> bool {
        self.map.lock().expect("cache lock").contains_key(key)
    }
}

impl Cache for MemoryCache {
    fn get(&self, key: &ContentHash) -> Option<Arc<Outputs>> {
        self.map.lock().expect("cache lock").get(key).cloned()
    }

    fn put(&self, key: ContentHash, outputs: Arc<Outputs>) {
        self.map.lock().expect("cache lock").insert(key, outputs);
    }

    fn persistence(&self, key: &ContentHash) -> Persistence {
        if self.contains(key) {
            Persistence::Memory
        } else {
            Persistence::Absent
        }
    }
}
