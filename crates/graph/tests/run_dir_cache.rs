//! Run-directory cache: failure policy (corrupt, truncated, wrong-version entries are
//! misses), missing codecs (memory only, reported), atomicity (stray temp files are
//! ignored) and thread safety. The restart acceptance test is in `acceptance.rs`.
//!
//! Graph: `src` (static, `count` items, an `i64` on a `Table` port) -> `double` (1:1),
//! and `src` -> `opaque` (1:1, a payload type nobody registered a codec for).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mold_graph::*;

type Log = Arc<Mutex<Vec<String>>>;

/// A unique directory under the system temp dir, removed on drop.
struct TempRunDir(PathBuf);

impl TempRunDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("mold-graph-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempRunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// No codec is registered for this.
#[derive(Debug)]
struct Opaque(#[allow(dead_code)] i64);

struct Node {
    spec: NodeSpec,
    log: Log,
    source: bool,
}

impl Processor for Node {
    fn spec(&self) -> NodeSpec {
        self.spec.clone()
    }
    fn generate(&self, ctx: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
        if self.source {
            Ok((0..ctx.params.int("count")?)
                .map(|i| NewItem::work().attr("i", i))
                .collect())
        } else {
            Ok(vec![NewItem::work()])
        }
    }
    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        self.log.lock().unwrap().push(ctx.item.to_string());
        let t = PortType::Table;
        Ok(match self.spec.kind.as_str() {
            "src" => {
                let Some(ParamValue::Int(i)) = ctx.attrs.get("i") else {
                    return Err(NodeError::msg("no i"));
                };
                Outputs::new()
                    .with("out", Value::new(t, *i))
                    .attr("seen", 1i64)
            }
            "double" => Outputs::new().with("out", Value::new(t, ctx.input::<i64>("in")? * 2)),
            _ => Outputs::new().with(
                "out",
                Value::new(PortType::Structure, Opaque(*ctx.input::<i64>("in")?)),
            ),
        })
    }
}

fn registry(log: &Log) -> Arc<Registry> {
    let mut r = Registry::new();
    let t = PortType::Table;
    r.processor(Node {
        spec: NodeSpec::new("src")
            .output("out", t)
            .param(ParamSpec::new("count", 3i64)),
        log: log.clone(),
        source: true,
    })
    .unwrap();
    r.processor(Node {
        spec: NodeSpec::new("double").input("in", t).output("out", t),
        log: log.clone(),
        source: false,
    })
    .unwrap();
    r.processor(Node {
        spec: NodeSpec::new("opaque")
            .input("in", t)
            .output("out", PortType::Structure),
        log: log.clone(),
        source: false,
    })
    .unwrap();
    Arc::new(r)
}

fn codecs() -> Arc<CodecRegistry> {
    let mut c = CodecRegistry::new();
    c.register(PortType::Table, I64Codec).unwrap();
    Arc::new(c)
}

fn graph(reg: &Arc<Registry>) -> Graph {
    let mut ed = Editor::new(reg.clone());
    for n in ["src", "double", "opaque"] {
        ed.add_node_with_id(n, n).unwrap();
    }
    ed.connect("src.out", &NodeId::new("double"), "in").unwrap();
    ed.connect("src.out", &NodeId::new("opaque"), "in").unwrap();
    ed.into_graph()
}

/// A fresh "process": new registry, new cache over `dir`, new engine.
struct Session {
    log: Log,
    graph: Graph,
    cache: Arc<RunDirCache>,
    engine: Engine,
}

fn session(dir: &Path, codecs: Arc<CodecRegistry>) -> Session {
    let log = Log::default();
    let reg = registry(&log);
    let cache = Arc::new(RunDirCache::open(dir, codecs).unwrap());
    let engine =
        Engine::new(reg.clone(), cache.clone()).with_executor(Arc::new(ThreadExecutor::new(2)));
    Session {
        log,
        graph: graph(&reg),
        cache,
        engine,
    }
}

fn cooked(s: &Session) -> Vec<String> {
    let mut v = std::mem::take(&mut *s.log.lock().unwrap());
    v.sort();
    v
}

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

fn keys(items: &ItemGraph, node: &str) -> Vec<ContentHash> {
    items.items(&id(node)).iter().map(|i| i.key).collect()
}

#[test]
fn persisted_results_are_on_disk_and_reported_as_such() {
    let dir = TempRunDir::new("disk");
    let s = session(&dir.0, codecs());
    let items = s.engine.cook(&s.graph, &[id("double")]).unwrap();
    assert_eq!(cooked(&s).len(), 6);
    for k in keys(&items, "double").iter().chain(&keys(&items, "src")) {
        assert_eq!(s.cache.persistence(k), Persistence::Disk);
        let p = s.cache.entry_path(k);
        assert!(p.is_file());
        // <run-dir>/cache/<aa>/<hash>
        let hex = k.to_hex();
        assert_eq!(p, dir.0.join("cache").join(&hex[..2]).join(&hex));
    }
    assert!(s.cache.take_events().is_empty());

    // Attributes added by the cook survive the restart.
    let s = session(&dir.0, codecs());
    let items = s.engine.generate(&s.graph, &[id("double")]).unwrap();
    let src0 = &items.items(&id("src"))[0];
    assert_eq!(src0.state, ItemState::Cached);
    assert_eq!(src0.attrs.get("seen"), Some(&ParamValue::Int(1)));
    let d2 = &items.items(&id("double"))[2];
    assert_eq!(
        d2.result
            .as_ref()
            .unwrap()
            .get("out")
            .unwrap()
            .downcast_ref::<i64>(),
        Some(&4)
    );
}

#[test]
fn corrupt_truncated_and_wrong_version_entries_are_misses_and_recook() {
    let dir = TempRunDir::new("corrupt");
    let s = session(&dir.0, codecs());
    let items = s.engine.cook(&s.graph, &[id("double")]).unwrap();
    cooked(&s);
    let ks = keys(&items, "double");
    let path = |i: usize| s.cache.entry_path(&ks[i]);

    // double[0.0]: truncated.
    let b = std::fs::read(path(0)).unwrap();
    std::fs::write(path(0), &b[..b.len() / 2]).unwrap();
    // double[1.0]: one bit flipped in the payload.
    let mut b = std::fs::read(path(1)).unwrap();
    let last = b.len() - 1;
    b[last] ^= 0x10;
    std::fs::write(path(1), &b).unwrap();
    // double[2.0]: another entry format version.
    let mut b = std::fs::read(path(2)).unwrap();
    b[16..20].copy_from_slice(&99u32.to_le_bytes());
    std::fs::write(path(2), &b).unwrap();
    drop(s);

    let s = session(&dir.0, codecs());
    let items = s.engine.cook(&s.graph, &[id("double")]).unwrap();
    assert_eq!(
        cooked(&s),
        ["double[0.0]", "double[1.0]", "double[2.0]"],
        "only the damaged entries re-cook"
    );
    assert_eq!(items.count(&id("src"), ItemState::Cached), 3);
    assert_eq!(items.count(&id("double"), ItemState::Cooked), 3);
    let mut errors: Vec<EntryError> = s
        .cache
        .take_events()
        .into_iter()
        .map(|e| match e {
            CacheEvent::Unreadable { error, .. } => error,
            e => panic!("unexpected {e:?}"),
        })
        .collect();
    errors.sort_by_key(|e| format!("{e:?}"));
    assert_eq!(
        errors,
        [
            EntryError::Checksum,
            EntryError::Truncated,
            EntryError::Version {
                found: 99,
                supported: ENTRY_FORMAT_VERSION
            }
        ]
    );
    drop(s);

    // The re-cooked results replaced the bad entries.
    let s = session(&dir.0, codecs());
    let items = s.engine.generate(&s.graph, &[id("double")]).unwrap();
    assert_eq!(items.count(&id("double"), ItemState::Cached), 3);
    assert!(s.cache.take_events().is_empty());
}

/// Same id and payload type as `I64Codec`, next version.
#[derive(Debug, Clone, Copy)]
struct I64CodecV2;

impl Codec for I64CodecV2 {
    type Payload = i64;
    fn id(&self) -> &str {
        I64Codec.id()
    }
    fn version(&self) -> u32 {
        2
    }
    fn encode(&self, p: &i64, out: &mut Vec<u8>) -> Result<(), CodecError> {
        I64Codec.encode(p, out)
    }
    fn decode(&self, bytes: &[u8]) -> Result<i64, CodecError> {
        I64Codec.decode(bytes)
    }
}

#[test]
fn a_codec_version_bump_turns_old_entries_into_misses() {
    let dir = TempRunDir::new("codec-version");
    let s = session(&dir.0, codecs());
    s.engine.cook(&s.graph, &[id("double")]).unwrap();
    drop(s);

    let mut v2 = CodecRegistry::new();
    v2.register(PortType::Table, I64CodecV2).unwrap();
    let s = session(&dir.0, Arc::new(v2));
    let items = s.engine.generate(&s.graph, &[id("double")]).unwrap();
    assert_eq!(items.count(&id("src"), ItemState::Uncooked), 3);
    assert_eq!(items.count(&id("double"), ItemState::Uncooked), 3);
    let events = s.cache.take_events();
    assert_eq!(
        events.len(),
        3 + 3,
        "src and (static) double entries: {events:?}"
    );
    assert!(events.iter().all(|e| matches!(
        e,
        CacheEvent::Unreadable {
            error: EntryError::Codec {
                error: CodecError::Version {
                    found: 1,
                    supported: 2,
                    ..
                },
                ..
            },
            ..
        }
    )));
}

#[test]
fn missing_codec_keeps_results_in_memory_reports_them_and_a_later_run_recooks() {
    let dir = TempRunDir::new("no-codec");
    let s = session(&dir.0, codecs());
    let items = s.engine.cook(&s.graph, &[id("opaque")]).unwrap();
    assert_eq!(cooked(&s).len(), 6);
    assert_eq!(items.count(&id("opaque"), ItemState::Cooked), 3);

    let want = Persistence::NotPersisted(Unpersisted::NoCodec {
        port: "out".into(),
        ty: PortType::Structure,
    });
    for k in keys(&items, "opaque") {
        assert_eq!(s.cache.persistence(&k), want);
        assert!(!s.cache.entry_path(&k).exists());
        assert!(s.cache.get(&k).is_some(), "still served from memory");
    }
    let events = s.cache.take_events();
    assert_eq!(events.len(), 3);
    assert!(
        events
            .iter()
            .all(|e| matches!(e, CacheEvent::NotPersisted { .. }))
    );
    // Same process: memory hits.
    s.engine.cook(&s.graph, &[id("opaque")]).unwrap();
    assert!(cooked(&s).is_empty());
    drop(s);

    let s = session(&dir.0, codecs());
    let items = s.engine.generate(&s.graph, &[id("opaque")]).unwrap();
    assert_eq!(items.count(&id("src"), ItemState::Cached), 3);
    assert_eq!(items.count(&id("opaque"), ItemState::Uncooked), 3);
    s.engine.cook(&s.graph, &[id("opaque")]).unwrap();
    assert_eq!(cooked(&s), ["opaque[0.0]", "opaque[1.0]", "opaque[2.0]"]);
}

#[test]
fn leftover_temp_files_from_a_crashed_write_are_ignored() {
    let dir = TempRunDir::new("crash");
    let s = session(&dir.0, codecs());
    let items = s.engine.generate(&s.graph, &[id("double")]).unwrap();
    // A write that died before its rename: a half-written temp file, no entry.
    for k in keys(&items, "src") {
        let tmp = s.cache.tmp_dir().join(format!("{}.4242.0.tmp", k.to_hex()));
        std::fs::write(&tmp, b"\x89MOLDITEM\r\n\x1a\n\0\0\0\x01\0").unwrap();
    }
    s.engine.cook(&s.graph, &[id("double")]).unwrap();
    assert_eq!(cooked(&s).len(), 6, "a temp file is not an entry");
    assert!(s.cache.take_events().is_empty(), "and is never read");
    drop(s);

    let s = session(&dir.0, codecs());
    let items = s.engine.generate(&s.graph, &[id("double")]).unwrap();
    assert_eq!(items.count(&id("src"), ItemState::Cached), 3);
    assert_eq!(items.count(&id("double"), ItemState::Cached), 3);
    let stray = std::fs::read_dir(s.cache.tmp_dir())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".4242.")
        })
        .count();
    assert_eq!(
        stray, 3,
        "strays are left alone (another process may own them)"
    );
}

#[test]
fn concurrent_readers_and_writers_in_one_process() {
    let dir = TempRunDir::new("threads");
    let cache = Arc::new(RunDirCache::open(&dir.0, codecs()).unwrap());
    let key = |i: u8| ContentHash([i; 32]);
    std::thread::scope(|sc| {
        for t in 0..8u8 {
            let cache = cache.clone();
            sc.spawn(move || {
                for i in 0..32u8 {
                    // Every thread writes every key (same content per key) and reads it back.
                    let o = Outputs::new().with("out", Value::new(PortType::Table, i as i64));
                    cache.put(key(i), Arc::new(o));
                    let got = cache.get(&key(i.wrapping_add(t) % 32));
                    if let Some(got) = got {
                        assert_eq!(
                            got.get("out").unwrap().downcast_ref::<i64>(),
                            Some(&((i.wrapping_add(t) % 32) as i64))
                        );
                    }
                }
            });
        }
    });
    assert!(cache.take_events().is_empty());
    let fresh = RunDirCache::open(&dir.0, codecs()).unwrap();
    for i in 0..32u8 {
        let got = fresh.get(&key(i)).expect("persisted");
        assert_eq!(
            got.get("out").unwrap().downcast_ref::<i64>(),
            Some(&(i as i64))
        );
    }
}

#[test]
fn an_entry_whose_artifact_is_gone_is_a_miss() {
    let dir = TempRunDir::new("artifact");
    let artifact = dir.0.join("pose.cif");
    std::fs::write(&artifact, "data_x").unwrap();
    let cache = RunDirCache::open(&dir.0, codecs()).unwrap();
    let k = ContentHash([1; 32]);
    let o = Outputs::new()
        .with("out", Value::new(PortType::Table, 1i64))
        .file(&artifact);
    cache.put(k, Arc::new(o));
    assert!(
        RunDirCache::open(&dir.0, codecs())
            .unwrap()
            .get(&k)
            .is_some()
    );

    std::fs::remove_file(&artifact).unwrap();
    let fresh = RunDirCache::open(&dir.0, codecs()).unwrap();
    assert!(fresh.get(&k).is_none());
    assert!(matches!(
        fresh.take_events().as_slice(),
        [CacheEvent::Unreadable {
            error: EntryError::MissingFile(_),
            ..
        }]
    ));
}
