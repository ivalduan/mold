//! The per-item result cache, keyed by item content hash.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::hash::ContentHash;
use crate::node::Outputs;

/// Where cooked item results are kept. In-memory now; a run-directory store (needs a
/// value codec per `PortType`) implements the same trait later.
pub trait Cache: Send + Sync + fmt::Debug {
    fn get(&self, key: &ContentHash) -> Option<Arc<Outputs>>;
    fn put(&self, key: ContentHash, outputs: Arc<Outputs>);
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
}
