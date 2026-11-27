//! The run-directory cache: item results persisted on disk, so a later process (the
//! app reopened, a headless replay) finds them.
//!
//! # Layout
//! ```text
//! <run-dir>/cache/<aa>/<hash>   one entry per work item, <hash> = 64 hex chars of its
//!                               key, <aa> = the first two (256 shards)
//! <run-dir>/cache/tmp/          entries being written: <hash>.<pid>.<n>.tmp
//! ```
//! A write goes to `tmp/`, then is renamed over its final path (atomic on one volume),
//! so a reader sees a whole entry or none. A crash leaves at most a stray `.tmp`, which
//! readers never look at. There is no `fsync`: a torn entry after a power loss fails its
//! checksum and is a miss, which is all a cache needs.
//!
//! # Entry format (version 1)
//! All integers little-endian.
//!
//! | Bytes | Field |
//! |---|---|
//! | 16 | magic [`ENTRY_MAGIC`] |
//! | 4  | entry format version, `u32` ([`ENTRY_FORMAT_VERSION`]) |
//! | 32 | the item key (must match the file name) |
//! | 8  | payload length, `u64` |
//! | 32 | BLAKE3 hash of the payload |
//! | …  | payload |
//!
//! Payload: attributes (`u32` count, then name + [`AttrValue`] each), artifact paths
//! (`u32` count, then strings), output values (`u32` count, then per port: port name,
//! [`PortType`] name, codec id, codec version `u32`, `u64` length + the codec's bytes).
//! Strings are `u32` length + UTF-8. An `AttrValue` is a tag byte (0 bool, 1 int,
//! 2 float, 3 string, 4 list) and its value (floats bit-exact, lists `u32` count).
//!
//! # Failure policy
//! Reading never panics and never trusts a length. A missing entry is a plain miss; an
//! unreadable one (I/O error, bad magic, other format version, truncated, checksum,
//! wrong key, unknown codec, other codec version, codec refusal, artifact file gone) is
//! a miss **and** a [`CacheEvent::Unreadable`]. The item re-cooks and its new entry
//! replaces the bad one.
//!
//! A result with an output whose payload has no codec, or that a codec refuses, stays in
//! memory only: [`Cache::persistence`] reports [`Persistence::NotPersisted`] for its
//! key, and a [`CacheEvent::NotPersisted`] is queued.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::PortType;
use crate::cache::{Cache, MemoryCache, Persistence, Unpersisted};
use crate::codec::{CodecError, CodecRegistry};
use crate::hash::ContentHash;
use crate::node::Outputs;
use crate::param::{AttrValue, Attributes};

/// Entry file magic, 16 bytes (`\x89` and `\r\n\x1a\n` catch text-mode mangling).
pub const ENTRY_MAGIC: [u8; 16] = *b"\x89MOLDITEM\r\n\x1a\n\0\0\0";
/// The only entry format version this build reads and writes.
pub const ENTRY_FORMAT_VERSION: u32 = 1;

const HEADER_LEN: usize = 16 + 4 + 32 + 8 + 32;
/// Nesting limit for list attributes, so a crafted entry cannot overflow the stack.
const MAX_DEPTH: u32 = 64;

/// Something the cache noticed that a caller may want to show or log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheEvent {
    /// A result was kept in memory only.
    NotPersisted {
        key: ContentHash,
        reason: Unpersisted,
    },
    /// An entry exists but could not be read; it was treated as a miss.
    Unreadable {
        key: ContentHash,
        path: PathBuf,
        error: EntryError,
    },
}

/// Why an entry on disk could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    Io(String),
    /// Does not start with [`ENTRY_MAGIC`].
    NotAnEntry,
    /// Another entry format version.
    Version {
        found: u32,
        supported: u32,
    },
    /// Shorter than its header declares.
    Truncated,
    /// The payload does not match its hash.
    Checksum,
    /// The entry belongs to another key (a misplaced file).
    KeyMismatch,
    /// The hash matched but the content is inconsistent.
    Corrupt(String),
    /// An output value could not be decoded (unknown codec, other codec version, or the
    /// codec refused the bytes).
    Codec {
        port: String,
        error: CodecError,
    },
    /// An artifact the result lists no longer exists.
    MissingFile(PathBuf),
}

impl std::fmt::Display for EntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::NotAnEntry => f.write_str("not a mold cache entry (bad magic)"),
            Self::Version { found, supported } => write!(
                f,
                "entry format version {found}, this build reads only version {supported}"
            ),
            Self::Truncated => f.write_str("entry is truncated"),
            Self::Checksum => f.write_str("entry checksum mismatch"),
            Self::KeyMismatch => f.write_str("entry belongs to another key"),
            Self::Corrupt(m) => write!(f, "entry is corrupt: {m}"),
            Self::Codec { port, error } => write!(f, "output `{port}`: {error}"),
            Self::MissingFile(p) => write!(f, "artifact {} is gone", p.display()),
        }
    }
}

impl std::error::Error for EntryError {}

/// A [`Cache`] over a run directory: memory first, disk second.
///
/// Safe to share between threads of one process. Two processes on one run directory
/// do not corrupt each other (writes are atomic renames of identical content per key)
/// but nothing coordinates them; locking, eviction and remote caches are follow-ups.
#[derive(Debug)]
pub struct RunDirCache {
    run_dir: PathBuf,
    dir: PathBuf,
    codecs: Arc<CodecRegistry>,
    memory: MemoryCache,
    unpersisted: Mutex<HashMap<ContentHash, Unpersisted>>,
    events: Mutex<Vec<CacheEvent>>,
    next_tmp: AtomicU64,
}

impl RunDirCache {
    /// Open (creating it if needed) the cache of `run_dir`, decoding with `codecs`.
    pub fn open(run_dir: impl Into<PathBuf>, codecs: Arc<CodecRegistry>) -> io::Result<Self> {
        let run_dir = run_dir.into();
        let dir = run_dir.join("cache");
        fs::create_dir_all(dir.join("tmp"))?;
        Ok(Self {
            run_dir,
            dir,
            codecs,
            memory: MemoryCache::new(),
            unpersisted: Mutex::default(),
            events: Mutex::default(),
            next_tmp: AtomicU64::new(0),
        })
    }

    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// `<run-dir>/cache`.
    pub fn cache_dir(&self) -> &Path {
        &self.dir
    }

    /// Where the entry for `key` lives: `<run-dir>/cache/<aa>/<hash>`.
    pub fn entry_path(&self, key: &ContentHash) -> PathBuf {
        let hex = key.to_hex();
        self.dir.join(&hex[..2]).join(hex)
    }

    /// Where interrupted writes are left (never read back).
    pub fn tmp_dir(&self) -> PathBuf {
        self.dir.join("tmp")
    }

    pub fn codecs(&self) -> &Arc<CodecRegistry> {
        &self.codecs
    }

    /// The in-memory layer.
    pub fn memory(&self) -> &MemoryCache {
        &self.memory
    }

    /// Events since the last call, oldest first.
    pub fn take_events(&self) -> Vec<CacheEvent> {
        std::mem::take(&mut *self.events.lock().expect("events lock"))
    }

    fn event(&self, e: CacheEvent) {
        self.events.lock().expect("events lock").push(e);
    }

    fn read(&self, key: &ContentHash) -> Option<Outputs> {
        let path = self.entry_path(key);
        let result = match fs::read(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => Err(EntryError::Io(e.to_string())),
            Ok(bytes) => decode_entry(&bytes, key, &self.codecs),
        };
        match result {
            Ok(o) => Some(o),
            Err(error) => {
                self.event(CacheEvent::Unreadable {
                    key: *key,
                    path,
                    error,
                });
                None
            }
        }
    }

    fn write(&self, key: &ContentHash, bytes: &[u8]) -> io::Result<()> {
        let dest = self.entry_path(key);
        let tmp = self.tmp_dir().join(format!(
            "{}.{}.{}.tmp",
            key.to_hex(),
            std::process::id(),
            self.next_tmp.fetch_add(1, Ordering::Relaxed)
        ));
        let r = (|| {
            fs::create_dir_all(self.tmp_dir())?;
            fs::create_dir_all(dest.parent().expect("entry has a shard dir"))?;
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            drop(f);
            fs::rename(&tmp, &dest)
        })();
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r
    }

    fn not_persisted(&self, key: ContentHash, reason: Unpersisted) {
        self.unpersisted
            .lock()
            .expect("unpersisted lock")
            .insert(key, reason.clone());
        self.event(CacheEvent::NotPersisted { key, reason });
    }
}

impl Cache for RunDirCache {
    fn get(&self, key: &ContentHash) -> Option<Arc<Outputs>> {
        if let Some(o) = self.memory.get(key) {
            return Some(o);
        }
        let o = Arc::new(self.read(key)?);
        self.memory.put(*key, o.clone());
        Some(o)
    }

    fn put(&self, key: ContentHash, outputs: Arc<Outputs>) {
        self.memory.put(key, outputs.clone());
        let written = encode_entry(&key, &outputs, &self.codecs).and_then(|bytes| {
            self.write(&key, &bytes)
                .map_err(|e| Unpersisted::Io(e.to_string()))
        });
        match written {
            Ok(()) => {
                self.unpersisted
                    .lock()
                    .expect("unpersisted lock")
                    .remove(&key);
            }
            Err(reason) => self.not_persisted(key, reason),
        }
    }

    fn persistence(&self, key: &ContentHash) -> Persistence {
        if let Some(r) = self.unpersisted.lock().expect("unpersisted lock").get(key) {
            return Persistence::NotPersisted(r.clone());
        }
        if self.entry_path(key).is_file() {
            Persistence::Disk
        } else if self.memory.contains(key) {
            Persistence::Memory
        } else {
            Persistence::Absent
        }
    }
}

// ---- Entry encoding ----------------------------------------------------------------

fn port_type_name(ty: PortType) -> &'static str {
    match ty {
        PortType::Structure => "Structure",
        PortType::Sequence => "Sequence",
        PortType::LigandSet => "LigandSet",
        PortType::Msa => "Msa",
        PortType::PoseSet => "PoseSet",
        PortType::Table => "Table",
        PortType::Scalar => "Scalar",
    }
}

fn port_type_from_name(s: &str) -> Option<PortType> {
    Some(match s {
        "Structure" => PortType::Structure,
        "Sequence" => PortType::Sequence,
        "LigandSet" => PortType::LigandSet,
        "Msa" => PortType::Msa,
        "PoseSet" => PortType::PoseSet,
        "Table" => PortType::Table,
        "Scalar" => PortType::Scalar,
        _ => return None,
    })
}

fn put_u32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len());
    out.extend_from_slice(s.as_bytes());
}

fn put_attr(out: &mut Vec<u8>, v: &AttrValue) {
    match v {
        AttrValue::Bool(b) => out.extend_from_slice(&[0, *b as u8]),
        AttrValue::Int(i) => {
            out.push(1);
            out.extend_from_slice(&i.to_le_bytes());
        }
        AttrValue::Float(f) => {
            out.push(2);
            out.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        AttrValue::Str(s) => {
            out.push(3);
            put_str(out, s);
        }
        AttrValue::List(l) => {
            out.push(4);
            put_u32(out, l.len());
            for x in l {
                put_attr(out, x);
            }
        }
    }
}

/// Serialise one result, or say why it can't be persisted.
pub(crate) fn encode_entry(
    key: &ContentHash,
    o: &Outputs,
    codecs: &CodecRegistry,
) -> Result<Vec<u8>, Unpersisted> {
    let mut p = Vec::new();
    put_u32(&mut p, o.attrs.len());
    for (k, v) in &o.attrs {
        put_str(&mut p, k);
        put_attr(&mut p, v);
    }
    put_u32(&mut p, o.files.len());
    for f in &o.files {
        let s = f
            .to_str()
            .ok_or_else(|| Unpersisted::NonUtf8File(f.clone()))?;
        put_str(&mut p, s);
    }
    put_u32(&mut p, o.values.len());
    for (port, v) in &o.values {
        let e = codecs.encode(v).map_err(|error| match error {
            CodecError::NoCodec { ty } => Unpersisted::NoCodec {
                port: port.clone(),
                ty,
            },
            error => Unpersisted::Encode {
                port: port.clone(),
                error,
            },
        })?;
        put_str(&mut p, port);
        put_str(&mut p, port_type_name(v.ty()));
        put_str(&mut p, &e.codec);
        p.extend_from_slice(&e.version.to_le_bytes());
        p.extend_from_slice(&(e.bytes.len() as u64).to_le_bytes());
        p.extend_from_slice(&e.bytes);
    }

    let mut out = Vec::with_capacity(HEADER_LEN + p.len());
    out.extend_from_slice(&ENTRY_MAGIC);
    out.extend_from_slice(&ENTRY_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&key.0);
    out.extend_from_slice(&(p.len() as u64).to_le_bytes());
    out.extend_from_slice(blake3::hash(&p).as_bytes());
    out.extend_from_slice(&p);
    Ok(out)
}

/// A bounds-checked reader over a checksummed payload.
struct Cur<'a>(&'a [u8]);

fn corrupt(m: impl Into<String>) -> EntryError {
    EntryError::Corrupt(m.into())
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], EntryError> {
        if n > self.0.len() {
            return Err(corrupt("length runs past the end of the payload"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], EntryError> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }
    fn u8(&mut self) -> Result<u8, EntryError> {
        Ok(self.arr::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, EntryError> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64, EntryError> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    /// A count of items each at least `min` bytes long, checked against what is left.
    fn count(&mut self, min: usize) -> Result<usize, EntryError> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min) > self.0.len() {
            return Err(corrupt("count runs past the end of the payload"));
        }
        Ok(n)
    }
    fn str(&mut self) -> Result<&'a str, EntryError> {
        let n = self.u32()? as usize;
        std::str::from_utf8(self.take(n)?).map_err(|_| corrupt("string is not UTF-8"))
    }
    fn attr(&mut self, depth: u32) -> Result<AttrValue, EntryError> {
        if depth > MAX_DEPTH {
            return Err(corrupt("attribute nested too deeply"));
        }
        Ok(match self.u8()? {
            0 => match self.u8()? {
                0 => AttrValue::Bool(false),
                1 => AttrValue::Bool(true),
                b => return Err(corrupt(format!("bad bool {b}"))),
            },
            1 => AttrValue::Int(i64::from_le_bytes(self.arr()?)),
            2 => AttrValue::Float(f64::from_bits(self.u64()?)),
            3 => AttrValue::Str(self.str()?.to_owned()),
            4 => {
                let n = self.count(1)?;
                let mut l = Vec::with_capacity(n);
                for _ in 0..n {
                    l.push(self.attr(depth + 1)?);
                }
                AttrValue::List(l)
            }
            t => return Err(corrupt(format!("unknown attribute tag {t}"))),
        })
    }
}

/// Parse and check one entry. Never panics.
pub(crate) fn decode_entry(
    bytes: &[u8],
    key: &ContentHash,
    codecs: &CodecRegistry,
) -> Result<Outputs, EntryError> {
    if bytes.len() < 16 {
        return Err(if ENTRY_MAGIC.starts_with(bytes) {
            EntryError::Truncated
        } else {
            EntryError::NotAnEntry
        });
    }
    if bytes[..16] != ENTRY_MAGIC {
        return Err(EntryError::NotAnEntry);
    }
    if bytes.len() < HEADER_LEN {
        return Err(EntryError::Truncated);
    }
    let mut h = Cur(&bytes[16..HEADER_LEN]);
    let version = h.u32()?;
    if version != ENTRY_FORMAT_VERSION {
        return Err(EntryError::Version {
            found: version,
            supported: ENTRY_FORMAT_VERSION,
        });
    }
    if h.arr::<32>()? != key.0 {
        return Err(EntryError::KeyMismatch);
    }
    let len = h.u64()?;
    let hash: [u8; 32] = h.arr()?;
    let payload = &bytes[HEADER_LEN..];
    if (payload.len() as u64) < len {
        return Err(EntryError::Truncated);
    }
    if payload.len() as u64 != len {
        return Err(corrupt("trailing bytes after the payload"));
    }
    if blake3::hash(payload).as_bytes() != &hash {
        return Err(EntryError::Checksum);
    }

    let mut c = Cur(payload);
    let mut attrs = Attributes::new();
    for _ in 0..c.count(5)? {
        let k = c.str()?.to_owned();
        let v = c.attr(0)?;
        attrs.insert(k, v);
    }
    let mut files = Vec::new();
    for _ in 0..c.count(4)? {
        files.push(PathBuf::from(c.str()?));
    }
    let mut out = Outputs::new();
    for _ in 0..c.count(4 * 3 + 4 + 8)? {
        let port = c.str()?.to_owned();
        let ty_name = c.str()?;
        let ty = port_type_from_name(ty_name)
            .ok_or_else(|| corrupt(format!("unknown port type `{ty_name}`")))?;
        let codec = c.str()?;
        let version = c.u32()?;
        let n = usize::try_from(c.u64()?).map_err(|_| corrupt("value too large"))?;
        let v = codecs
            .decode(ty, codec, version, c.take(n)?)
            .map_err(|error| EntryError::Codec {
                port: port.clone(),
                error,
            })?;
        if out.values.insert(port, v).is_some() {
            return Err(corrupt("duplicate output port"));
        }
    }
    if !c.0.is_empty() {
        return Err(corrupt("trailing bytes in the payload"));
    }
    if let Some(gone) = files.iter().find(|f| !f.exists()) {
        return Err(EntryError::MissingFile(gone.clone()));
    }
    out.attrs = attrs;
    out.files = files;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{F64Codec, I64Codec, StringCodec};
    use crate::value::Value;
    use proptest::prelude::*;

    fn codecs() -> CodecRegistry {
        let mut r = CodecRegistry::new();
        r.register(PortType::Scalar, F64Codec).unwrap();
        r.register(PortType::Table, I64Codec).unwrap();
        r.register(PortType::Sequence, StringCodec).unwrap();
        r
    }

    fn attr_value() -> impl Strategy<Value = AttrValue> {
        let leaf = prop_oneof![
            any::<bool>().prop_map(AttrValue::Bool),
            any::<i64>().prop_map(AttrValue::Int),
            any::<u64>().prop_map(|b| AttrValue::Float(f64::from_bits(b))),
            ".{0,12}".prop_map(AttrValue::Str),
        ];
        leaf.prop_recursive(3, 16, 4, |inner| {
            proptest::collection::vec(inner, 0..4).prop_map(AttrValue::List)
        })
    }

    /// Floats compared by bit pattern (NaN != NaN otherwise).
    fn bits(v: &AttrValue) -> String {
        match v {
            AttrValue::Float(f) => format!("F{:x}", f.to_bits()),
            AttrValue::List(l) => format!("[{}]", l.iter().map(bits).collect::<Vec<_>>().join(",")),
            v => format!("{v:?}"),
        }
    }

    proptest! {
        #[test]
        fn entries_round_trip(
            attrs in proptest::collection::btree_map(".{0,8}", attr_value(), 0..6),
            scalar in any::<u64>(),
            n in any::<i64>(),
            s in ".{0,32}",
            key in any::<[u8; 32]>(),
        ) {
            let key = ContentHash(key);
            let mut o = Outputs::new()
                .with("x", Value::new(PortType::Scalar, f64::from_bits(scalar)))
                .with("n", Value::new(PortType::Table, n))
                .with("s", Value::new(PortType::Sequence, s.clone()));
            o.attrs = attrs.clone();
            let bytes = encode_entry(&key, &o, &codecs()).unwrap();
            let back = decode_entry(&bytes, &key, &codecs()).unwrap();
            prop_assert_eq!(
                back.attrs.iter().map(|(k, v)| (k.clone(), bits(v))).collect::<Vec<_>>(),
                attrs.iter().map(|(k, v)| (k.clone(), bits(v))).collect::<Vec<_>>()
            );
            prop_assert_eq!(back.get("x").unwrap().downcast_ref::<f64>().unwrap().to_bits(), scalar);
            prop_assert_eq!(back.get("n").unwrap().downcast_ref::<i64>(), Some(&n));
            prop_assert_eq!(back.get("s").unwrap().downcast_ref::<String>(), Some(&s));
        }

        #[test]
        fn mangled_entries_never_panic(
            n in any::<i64>(),
            cut in any::<prop::sample::Index>(),
            flip in any::<prop::sample::Index>(),
            bit in 0u8..8,
        ) {
            let key = ContentHash([7; 32]);
            let o = Outputs::new().attr("a", "b").with("n", Value::new(PortType::Table, n));
            let bytes = encode_entry(&key, &o, &codecs()).unwrap();
            let truncated = &bytes[..cut.index(bytes.len())];
            prop_assert!(decode_entry(truncated, &key, &codecs()).is_err());
            let mut flipped = bytes.clone();
            flipped[flip.index(bytes.len())] ^= 1 << bit;
            prop_assert!(decode_entry(&flipped, &key, &codecs()).is_err());
        }
    }

    #[test]
    fn port_type_names_round_trip() {
        for ty in [
            PortType::Structure,
            PortType::Sequence,
            PortType::LigandSet,
            PortType::Msa,
            PortType::PoseSet,
            PortType::Table,
            PortType::Scalar,
        ] {
            assert_eq!(port_type_from_name(port_type_name(ty)), Some(ty));
        }
    }
}
