//! Payload codecs: how a type-erased [`Value`] becomes bytes and back.
//!
//! Payload types live in other crates (`mol` arenas, polars tables, `chem` ligand
//! sets) and plugins bring their own, so `graph` cannot serialise them. The crate that
//! owns a payload type implements [`Codec`] for it, and the host registers the codec in
//! a [`CodecRegistry`] under every [`PortType`] that carries that type. The run-directory
//! cache ([`RunDirCache`](crate::RunDirCache)) persists an item only when every output
//! port's payload has a codec.
//!
//! **Keying.** A codec is registered under a `(PortType, codec id)` pair, not under a
//! `PortType` alone: the Rust type behind a port type is a convention, not a guarantee
//! (a `Scalar` can be an `f64` or an `i64`; a `Table` a polars frame or, in a test, a
//! `Vec<i64>`), and plugins may carry their own type on a shared port type. Encoding
//! picks the codec whose payload type matches the value (a downcast); the entry records
//! the codec id and version; decoding looks the pair up again.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use crate::PortType;
use crate::value::Value;

/// Serialises one payload type. Implemented by the crate that owns the type.
///
/// - [`id`](Self::id) is stable forever (it is written into every cache entry) and
///   globally unique: prefix it with the owning crate, e.g. `"mold-mol/arena"`.
/// - [`version`](Self::version) changes whenever the byte layout changes. An entry
///   written by another version is a cache miss (re-cook), never a migration.
/// - `decode` must reject bad input with an error, never panic: entries are checksummed,
///   but a codec can still be handed bytes from a different build.
pub trait Codec: Send + Sync + 'static {
    /// The Rust type this codec reads and writes.
    type Payload: Any + Send + Sync;

    fn id(&self) -> &str;
    fn version(&self) -> u32;
    fn encode(&self, payload: &Self::Payload, out: &mut Vec<u8>) -> Result<(), CodecError>;
    fn decode(&self, bytes: &[u8]) -> Result<Self::Payload, CodecError>;
}

/// Why a value could not be encoded or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// No codec registered for this port type and payload type.
    NoCodec { ty: PortType },
    /// The entry names a codec this registry doesn't have for the port type.
    UnknownCodec { ty: PortType, id: String },
    /// The entry was written by another version of the codec.
    Version {
        id: String,
        found: u32,
        supported: u32,
    },
    /// The codec itself refused the payload or the bytes.
    Invalid(String),
}

impl CodecError {
    pub fn invalid(m: impl Into<String>) -> Self {
        Self::Invalid(m.into())
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCodec { ty } => write!(f, "no codec registered for this {ty:?} payload type"),
            Self::UnknownCodec { ty, id } => write!(f, "no codec `{id}` registered for {ty:?}"),
            Self::Version {
                id,
                found,
                supported,
            } => write!(
                f,
                "codec `{id}` version {found}, this build reads version {supported}"
            ),
            Self::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// A registration that conflicts with an earlier one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecConflict(pub String);

impl fmt::Display for CodecConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecConflict {}

/// An encoded value: which codec wrote it, and the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub codec: String,
    pub version: u32,
    pub bytes: Vec<u8>,
}

/// The type-erased side of a [`Codec`].
trait ErasedCodec: Send + Sync {
    fn id(&self) -> &str;
    fn version(&self) -> u32;
    fn payload(&self) -> TypeId;
    fn payload_name(&self) -> &'static str;
    /// `None`: the value's payload is not this codec's type.
    fn encode(&self, v: &Value, out: &mut Vec<u8>) -> Option<Result<(), CodecError>>;
    fn decode(&self, ty: PortType, bytes: &[u8]) -> Result<Value, CodecError>;
}

struct Erased<C>(C);

impl<C: Codec> ErasedCodec for Erased<C> {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn version(&self) -> u32 {
        self.0.version()
    }
    fn payload(&self) -> TypeId {
        TypeId::of::<C::Payload>()
    }
    fn payload_name(&self) -> &'static str {
        std::any::type_name::<C::Payload>()
    }
    fn encode(&self, v: &Value, out: &mut Vec<u8>) -> Option<Result<(), CodecError>> {
        v.downcast_ref::<C::Payload>()
            .map(|p| self.0.encode(p, out))
    }
    fn decode(&self, ty: PortType, bytes: &[u8]) -> Result<Value, CodecError> {
        self.0.decode(bytes).map(|p| Value::new(ty, p))
    }
}

/// The codecs a host knows, per `(PortType, codec id)`. Filled by the host (the crate
/// that owns each payload type registers its codec); empty by default.
#[derive(Default, Clone)]
pub struct CodecRegistry {
    by_port: HashMap<PortType, Vec<Arc<dyn ErasedCodec>>>,
    /// Codec id → payload type: an id names one Rust type everywhere.
    ids: BTreeMap<String, (TypeId, &'static str)>,
}

impl fmt::Debug for CodecRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut m = f.debug_map();
        for (ty, cs) in &self.by_port {
            let ids: Vec<String> = cs
                .iter()
                .map(|c| format!("{}@{}", c.id(), c.version()))
                .collect();
            m.entry(ty, &ids);
        }
        m.finish()
    }
}

impl CodecRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `codec` for values on `ty`. Refused if `ty` already has a codec with the
    /// same id or for the same payload type, or if the id is already bound to another
    /// payload type. The same codec may be registered under several port types.
    pub fn register<C: Codec>(
        &mut self,
        ty: PortType,
        codec: C,
    ) -> Result<&mut Self, CodecConflict> {
        let c: Arc<dyn ErasedCodec> = Arc::new(Erased(codec));
        let id = c.id().to_owned();
        if id.is_empty() || id.len() > u16::MAX as usize {
            return Err(CodecConflict(format!(
                "codec id `{id}` must be 1..=65535 bytes"
            )));
        }
        if let Some((t, name)) = self.ids.get(&id)
            && *t != c.payload()
        {
            return Err(CodecConflict(format!(
                "codec id `{id}` is already bound to payload type `{name}`"
            )));
        }
        let list = self.by_port.entry(ty).or_default();
        if let Some(old) = list
            .iter()
            .find(|o| o.id() == id || o.payload() == c.payload())
        {
            return Err(CodecConflict(format!(
                "{ty:?} already has codec `{}` for payload type `{}`",
                old.id(),
                old.payload_name()
            )));
        }
        self.ids.insert(id, (c.payload(), c.payload_name()));
        list.push(c);
        Ok(self)
    }

    /// Whether `value` can be encoded (a codec is registered for its port and payload type).
    pub fn can_encode(&self, value: &Value) -> bool {
        self.by_port.get(&value.ty()).is_some_and(|cs| {
            cs.iter()
                .any(|c| c.encode(value, &mut Vec::new()).is_some())
        })
    }

    /// Encode `value` with the codec registered for its port type and payload type.
    pub fn encode(&self, value: &Value) -> Result<Encoded, CodecError> {
        let ty = value.ty();
        let mut bytes = Vec::new();
        for c in self.by_port.get(&ty).into_iter().flatten() {
            if let Some(r) = c.encode(value, &mut bytes) {
                r?;
                return Ok(Encoded {
                    codec: c.id().to_owned(),
                    version: c.version(),
                    bytes,
                });
            }
        }
        Err(CodecError::NoCodec { ty })
    }

    /// Decode bytes written by codec `id` at `version` into a value on `ty`.
    pub fn decode(
        &self,
        ty: PortType,
        id: &str,
        version: u32,
        bytes: &[u8],
    ) -> Result<Value, CodecError> {
        let c = self
            .by_port
            .get(&ty)
            .and_then(|cs| cs.iter().find(|c| c.id() == id))
            .ok_or_else(|| CodecError::UnknownCodec {
                ty,
                id: id.to_owned(),
            })?;
        if c.version() != version {
            return Err(CodecError::Version {
                id: id.to_owned(),
                found: version,
                supported: c.version(),
            });
        }
        c.decode(ty, bytes)
    }
}

// ---- Built-in codecs ---------------------------------------------------------------
//
// Only for types `graph` can name. Nothing is registered by default: which Rust type a
// port type carries is the `nodes` convention, so the host registers these where they
// fit (e.g. `F64Codec` under `PortType::Scalar`).

fn exact<const N: usize>(bytes: &[u8], what: &str) -> Result<[u8; N], CodecError> {
    bytes.try_into().map_err(|_| {
        CodecError::invalid(format!("{what}: expected {N} bytes, got {}", bytes.len()))
    })
}

fn chunks<const N: usize>(
    bytes: &[u8],
    what: &str,
) -> Result<impl Iterator<Item = [u8; N]>, CodecError> {
    let (whole, rest) = bytes.as_chunks::<N>();
    if !rest.is_empty() {
        return Err(CodecError::invalid(format!(
            "{what}: length {} is not a multiple of {N}",
            bytes.len()
        )));
    }
    Ok(whole.iter().copied())
}

/// `i64`, 8 bytes little-endian.
#[derive(Debug, Clone, Copy, Default)]
pub struct I64Codec;

impl Codec for I64Codec {
    type Payload = i64;
    fn id(&self) -> &str {
        "mold-graph/i64"
    }
    fn version(&self) -> u32 {
        1
    }
    fn encode(&self, p: &i64, out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.extend_from_slice(&p.to_le_bytes());
        Ok(())
    }
    fn decode(&self, bytes: &[u8]) -> Result<i64, CodecError> {
        Ok(i64::from_le_bytes(exact(bytes, "i64")?))
    }
}

/// `f64`, 8 bytes little-endian, bit-exact (NaN payloads and -0.0 survive).
#[derive(Debug, Clone, Copy, Default)]
pub struct F64Codec;

impl Codec for F64Codec {
    type Payload = f64;
    fn id(&self) -> &str {
        "mold-graph/f64"
    }
    fn version(&self) -> u32 {
        1
    }
    fn encode(&self, p: &f64, out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.extend_from_slice(&p.to_bits().to_le_bytes());
        Ok(())
    }
    fn decode(&self, bytes: &[u8]) -> Result<f64, CodecError> {
        Ok(f64::from_bits(u64::from_le_bytes(exact(bytes, "f64")?)))
    }
}

/// `String`, raw UTF-8.
#[derive(Debug, Clone, Copy, Default)]
pub struct StringCodec;

impl Codec for StringCodec {
    type Payload = String;
    fn id(&self) -> &str {
        "mold-graph/string"
    }
    fn version(&self) -> u32 {
        1
    }
    fn encode(&self, p: &String, out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.extend_from_slice(p.as_bytes());
        Ok(())
    }
    fn decode(&self, bytes: &[u8]) -> Result<String, CodecError> {
        String::from_utf8(bytes.to_vec()).map_err(|e| CodecError::invalid(format!("string: {e}")))
    }
}

/// `Vec<i64>`, each 8 bytes little-endian.
#[derive(Debug, Clone, Copy, Default)]
pub struct I64VecCodec;

impl Codec for I64VecCodec {
    type Payload = Vec<i64>;
    fn id(&self) -> &str {
        "mold-graph/vec-i64"
    }
    fn version(&self) -> u32 {
        1
    }
    fn encode(&self, p: &Vec<i64>, out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.reserve(p.len() * 8);
        for x in p {
            out.extend_from_slice(&x.to_le_bytes());
        }
        Ok(())
    }
    fn decode(&self, bytes: &[u8]) -> Result<Vec<i64>, CodecError> {
        Ok(chunks::<8>(bytes, "vec-i64")?
            .map(i64::from_le_bytes)
            .collect())
    }
}

/// `Vec<f64>`, each 8 bytes little-endian, bit-exact.
#[derive(Debug, Clone, Copy, Default)]
pub struct F64VecCodec;

impl Codec for F64VecCodec {
    type Payload = Vec<f64>;
    fn id(&self) -> &str {
        "mold-graph/vec-f64"
    }
    fn version(&self) -> u32 {
        1
    }
    fn encode(&self, p: &Vec<f64>, out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.reserve(p.len() * 8);
        for x in p {
            out.extend_from_slice(&x.to_bits().to_le_bytes());
        }
        Ok(())
    }
    fn decode(&self, bytes: &[u8]) -> Result<Vec<f64>, CodecError> {
        Ok(chunks::<8>(bytes, "vec-f64")?
            .map(|b| f64::from_bits(u64::from_le_bytes(b)))
            .collect())
    }
}

/// [`mold_mol::Arena`] in `mol`'s binary arena format ([`mold_mol::cache`]), which is
/// versioned and checksummed on its own. The codec version is `mol`'s format version,
/// so a format bump in `mol` turns old entries into misses.
///
/// Which port types carry a bare `Arena` is for `nodes` to decide; nothing registers
/// this codec by default.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArenaCodec;

impl Codec for ArenaCodec {
    type Payload = mold_mol::Arena;
    fn id(&self) -> &str {
        "mold-mol/arena"
    }
    fn version(&self) -> u32 {
        mold_mol::cache::FORMAT_VERSION
    }
    fn encode(&self, p: &mold_mol::Arena, out: &mut Vec<u8>) -> Result<(), CodecError> {
        mold_mol::cache::save(p, out).map_err(|e| CodecError::invalid(e.to_string()))
    }
    fn decode(&self, bytes: &[u8]) -> Result<mold_mol::Arena, CodecError> {
        mold_mol::cache::load(bytes).map_err(|e| CodecError::invalid(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn round_trip<C: Codec + Copy>(c: C, ty: PortType, p: C::Payload) -> C::Payload
    where
        C::Payload: Clone,
    {
        let mut reg = CodecRegistry::new();
        reg.register(ty, c).unwrap();
        let v = Value::new(ty, p);
        let e = reg.encode(&v).unwrap();
        assert_eq!(e.codec, c.id());
        let back = reg.decode(ty, &e.codec, e.version, &e.bytes).unwrap();
        assert_eq!(back.ty(), ty);
        back.downcast_ref::<C::Payload>().unwrap().clone()
    }

    proptest! {
        #[test]
        fn i64_round_trips(x in any::<i64>()) {
            prop_assert_eq!(round_trip(I64Codec, PortType::Scalar, x), x);
        }

        #[test]
        fn f64_round_trips_bit_exact(bits in any::<u64>()) {
            let x = f64::from_bits(bits);
            prop_assert_eq!(round_trip(F64Codec, PortType::Scalar, x).to_bits(), bits);
        }

        #[test]
        fn string_round_trips(s in any::<String>()) {
            prop_assert_eq!(round_trip(StringCodec, PortType::Sequence, s.clone()), s);
        }

        #[test]
        fn vec_i64_round_trips(v in proptest::collection::vec(any::<i64>(), 0..64)) {
            prop_assert_eq!(round_trip(I64VecCodec, PortType::Table, v.clone()), v);
        }

        #[test]
        fn vec_f64_round_trips_bit_exact(v in proptest::collection::vec(any::<u64>(), 0..64)) {
            let xs: Vec<f64> = v.iter().map(|b| f64::from_bits(*b)).collect();
            let back = round_trip(F64VecCodec, PortType::Table, xs);
            prop_assert_eq!(back.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), v);
        }

        #[test]
        fn decoders_never_panic_on_garbage(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = I64Codec.decode(&bytes);
            let _ = F64Codec.decode(&bytes);
            let _ = StringCodec.decode(&bytes);
            let _ = I64VecCodec.decode(&bytes);
            let _ = F64VecCodec.decode(&bytes);
            let _ = ArenaCodec.decode(&bytes);
        }
    }

    #[test]
    fn arena_round_trips_through_mol_format() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/posebusters/inputs/1M17_AQ4_crystal.sdf");
        let mut arena = mold_mol::Arena::new();
        mold_mol::sdf::read_sdf_file(&mut arena, path).unwrap();
        assert!(arena.atom_count() > 0);
        let back = round_trip(ArenaCodec, PortType::Structure, arena.clone());
        assert_eq!(back, arena);
    }

    #[test]
    fn payload_type_selects_the_codec_on_a_shared_port_type() {
        let mut reg = CodecRegistry::new();
        reg.register(PortType::Scalar, I64Codec).unwrap();
        reg.register(PortType::Scalar, F64Codec).unwrap();
        let e = reg.encode(&Value::new(PortType::Scalar, 2.5f64)).unwrap();
        assert_eq!(e.codec, "mold-graph/f64");
        let e = reg.encode(&Value::new(PortType::Scalar, 7i64)).unwrap();
        assert_eq!(e.codec, "mold-graph/i64");
        assert_eq!(
            reg.encode(&Value::new(PortType::Scalar, 1u8)),
            Err(CodecError::NoCodec {
                ty: PortType::Scalar
            })
        );
        // Registered for Scalar only.
        assert!(!reg.can_encode(&Value::new(PortType::Table, 7i64)));
        assert!(matches!(
            reg.decode(PortType::Table, "mold-graph/i64", 1, &[0; 8]),
            Err(CodecError::UnknownCodec { .. })
        ));
        assert!(matches!(
            reg.decode(PortType::Scalar, "mold-graph/i64", 2, &[0; 8]),
            Err(CodecError::Version {
                found: 2,
                supported: 1,
                ..
            })
        ));
    }

    #[derive(Clone, Copy)]
    struct Imposter;
    impl Codec for Imposter {
        type Payload = u32;
        fn id(&self) -> &str {
            "mold-graph/i64"
        }
        fn version(&self) -> u32 {
            1
        }
        fn encode(&self, _: &u32, _: &mut Vec<u8>) -> Result<(), CodecError> {
            Ok(())
        }
        fn decode(&self, _: &[u8]) -> Result<u32, CodecError> {
            Ok(0)
        }
    }

    #[test]
    fn conflicting_registrations_are_refused() {
        let mut reg = CodecRegistry::new();
        reg.register(PortType::Scalar, I64Codec).unwrap();
        reg.register(PortType::Table, I64Codec).unwrap();
        assert!(
            reg.register(PortType::Scalar, I64Codec).is_err(),
            "same port, same id"
        );
        assert!(
            reg.register(PortType::Msa, Imposter).is_err(),
            "id bound to another type"
        );
    }
}
