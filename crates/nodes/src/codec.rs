//! Payload codecs, so a [`RunDirCache`](mold_graph::RunDirCache) survives a restart.
//!
//! `graph` registers nothing by default; this crate owns the port-type convention
//! (see [`data`](crate::data)) and therefore the codecs. [`crate::codecs`] builds the
//! registry the app and the CLI pass to `RunDirCache`.
//!
//! Three codecs live here; the rest are `graph`'s own (`ArenaCodec` for `Structure`,
//! `StringCodec` for `Sequence`, `I64Codec`/`F64Codec` for `Scalar`).
//!
//! | Codec | Payload | Bytes |
//! |---|---|---|
//! | `mold-nodes/pose-table` | `DataFrame` | column name, dtype, then one tagged value per row |
//! | `mold-nodes/ligand-set` | [`LigandSet`] | arena (`mold_mol::cache`) + ids and properties |
//! | `mold-nodes/pose-set` | [`PoseSet`] | receptor arena + poses arena + pose table |
//!
//! All integers little-endian; a string is `u32` length + UTF-8. Values are tagged per
//! row (present byte, then the value) rather than carried in a validity bitmap: pose
//! tables are small next to the arenas beside them, and this stays obviously correct.
//!
//! Every version is `(layout << 8) | mold_mol::cache::FORMAT_VERSION`, so a bump on
//! either side turns old entries into plain cache misses, never into decode errors
//! (the same trick as `graph`'s `ArenaCodec`).

use std::sync::Arc;

use mold_graph::{Codec, CodecError};
use mold_mol::{Arena, PoseId};
use polars::prelude::{AnyValue, Column, DataFrame, DataType, NamedFrom, Series};

use crate::data::{Ligand, LigandSet, PoseSet};

const fn version(layout: u32) -> u32 {
    (layout << 8) | mold_mol::cache::FORMAT_VERSION
}

// ---- Writing ------------------------------------------------------------------------

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn put_blob(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn put_arena(out: &mut Vec<u8>, arena: &Arena) -> Result<(), CodecError> {
    let mut bytes = Vec::new();
    mold_mol::cache::save(arena, &mut bytes).map_err(|e| CodecError::invalid(e.to_string()))?;
    put_blob(out, &bytes);
    Ok(())
}

// ---- Reading ------------------------------------------------------------------------

/// A bounds-checked cursor: every read either consumes bytes or fails. Never panics,
/// never trusts a length (`graph`'s cache policy).
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if self.bytes.len() < n {
            return Err(CodecError::invalid(format!(
                "entry is truncated: {n} bytes wanted, {} left",
                self.bytes.len()
            )));
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn f64(&mut self) -> Result<f64, CodecError> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn len(&mut self) -> Result<usize, CodecError> {
        usize::try_from(self.u64()?).map_err(|_| CodecError::invalid("length exceeds this machine"))
    }

    /// A count read from the entry, checked against what is left: `each` is the
    /// smallest number of bytes one element can take. Nothing is ever allocated from
    /// an unchecked length (`graph`'s cache policy: never trust a length).
    fn bounded(&self, n: usize, each: usize, what: &str) -> Result<usize, CodecError> {
        if n.saturating_mul(each.max(1)) > self.bytes.len() {
            return Err(CodecError::invalid(format!(
                "{what}: {n} declared, only {} bytes left",
                self.bytes.len()
            )));
        }
        Ok(n)
    }

    fn str(&mut self) -> Result<String, CodecError> {
        let n = self.u32()? as usize;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| CodecError::invalid(format!("string: {e}")))
    }

    fn blob(&mut self) -> Result<&'a [u8], CodecError> {
        let n = self.len()?;
        self.take(n)
    }

    fn arena(&mut self) -> Result<Arena, CodecError> {
        let bytes = self.blob()?;
        mold_mol::cache::load(bytes).map_err(|e| CodecError::invalid(e.to_string()))
    }

    fn end(&self) -> Result<(), CodecError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(CodecError::invalid(format!(
                "{} trailing bytes",
                self.bytes.len()
            )))
        }
    }
}

// ---- Pose table ---------------------------------------------------------------------

/// A Polars `DataFrame` of pose-table columns (`UInt32`, `Float64`, `Boolean`,
/// `String`). Any other dtype is refused, so the item stays in memory and
/// `Cache::persistence` says why, rather than being silently dropped.
#[derive(Debug, Clone, Copy, Default)]
pub struct PoseTableCodec;

const TAG_U32: u8 = 1;
const TAG_F64: u8 = 2;
const TAG_BOOL: u8 = 3;
const TAG_STR: u8 = 4;

impl PoseTableCodec {
    pub(crate) fn write(df: &DataFrame, out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.extend_from_slice(&(df.width() as u32).to_le_bytes());
        out.extend_from_slice(&(df.height() as u64).to_le_bytes());
        for c in df.columns() {
            put_str(out, c.name());
            let tag = match c.dtype() {
                DataType::UInt32 => TAG_U32,
                DataType::Float64 => TAG_F64,
                DataType::Boolean => TAG_BOOL,
                DataType::String => TAG_STR,
                other => {
                    return Err(CodecError::invalid(format!(
                        "column `{}` is {other:?}; a pose table holds UInt32, Float64, \
                         Boolean or String",
                        c.name()
                    )));
                }
            };
            out.push(tag);
            for i in 0..df.height() {
                let v = c
                    .get(i)
                    .map_err(|e| CodecError::invalid(format!("column `{}`: {e}", c.name())))?;
                match v {
                    AnyValue::Null => out.push(0),
                    AnyValue::UInt32(x) if tag == TAG_U32 => {
                        out.push(1);
                        out.extend_from_slice(&x.to_le_bytes());
                    }
                    AnyValue::Float64(x) if tag == TAG_F64 => {
                        out.push(1);
                        out.extend_from_slice(&x.to_bits().to_le_bytes());
                    }
                    AnyValue::Boolean(x) if tag == TAG_BOOL => {
                        out.push(1);
                        out.push(u8::from(x));
                    }
                    AnyValue::String(s) if tag == TAG_STR => {
                        out.push(1);
                        put_str(out, s);
                    }
                    AnyValue::StringOwned(s) if tag == TAG_STR => {
                        out.push(1);
                        put_str(out, s.as_str());
                    }
                    other => {
                        return Err(CodecError::invalid(format!(
                            "column `{}` row {i}: unexpected value {other:?}",
                            c.name()
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn read(r: &mut Reader<'_>) -> Result<DataFrame, CodecError> {
        let n = r.u32()? as usize;
        let width = r.bounded(n, 6, "column count")?;
        let n = r.len()?;
        let height = r.bounded(n, width.max(1), "row count")?;
        let mut columns: Vec<Column> = Vec::with_capacity(width);
        for _ in 0..width {
            let name = r.str()?;
            let tag = r.u8()?;
            let column: Column = match tag {
                TAG_U32 => {
                    let mut v = Vec::with_capacity(height);
                    for _ in 0..height {
                        v.push(match r.u8()? {
                            0 => None,
                            _ => Some(r.u32()?),
                        });
                    }
                    Series::new(name.as_str().into(), v).into()
                }
                TAG_F64 => {
                    let mut v = Vec::with_capacity(height);
                    for _ in 0..height {
                        v.push(match r.u8()? {
                            0 => None,
                            _ => Some(r.f64()?),
                        });
                    }
                    Series::new(name.as_str().into(), v).into()
                }
                TAG_BOOL => {
                    let mut v = Vec::with_capacity(height);
                    for _ in 0..height {
                        v.push(match r.u8()? {
                            0 => None,
                            _ => Some(r.u8()? != 0),
                        });
                    }
                    Series::new(name.as_str().into(), v).into()
                }
                TAG_STR => {
                    let mut v: Vec<Option<String>> = Vec::with_capacity(height);
                    for _ in 0..height {
                        v.push(match r.u8()? {
                            0 => None,
                            _ => Some(r.str()?),
                        });
                    }
                    Series::new(name.as_str().into(), v).into()
                }
                other => {
                    return Err(CodecError::invalid(format!(
                        "column `{name}`: unknown dtype tag {other}"
                    )));
                }
            };
            columns.push(column);
        }
        DataFrame::new(height, columns).map_err(|e| CodecError::invalid(e.to_string()))
    }
}

impl Codec for PoseTableCodec {
    type Payload = DataFrame;

    fn id(&self) -> &str {
        "mold-nodes/pose-table"
    }

    fn version(&self) -> u32 {
        version(1)
    }

    fn encode(&self, p: &DataFrame, out: &mut Vec<u8>) -> Result<(), CodecError> {
        Self::write(p, out)
    }

    fn decode(&self, bytes: &[u8]) -> Result<DataFrame, CodecError> {
        let mut r = Reader::new(bytes);
        let df = Self::read(&mut r)?;
        r.end()?;
        Ok(df)
    }
}

// ---- Ligand set ---------------------------------------------------------------------

/// A [`LigandSet`]: its arena in `mol`'s binary format, then each ligand's id, pose
/// and properties.
#[derive(Debug, Clone, Copy, Default)]
pub struct LigandSetCodec;

impl Codec for LigandSetCodec {
    type Payload = LigandSet;

    fn id(&self) -> &str {
        "mold-nodes/ligand-set"
    }

    fn version(&self) -> u32 {
        version(1)
    }

    fn encode(&self, p: &LigandSet, out: &mut Vec<u8>) -> Result<(), CodecError> {
        put_arena(out, &p.arena)?;
        out.extend_from_slice(&(p.ligands.len() as u32).to_le_bytes());
        for l in &p.ligands {
            put_str(out, &l.id);
            out.extend_from_slice(&l.pose.0.to_le_bytes());
            out.extend_from_slice(&(l.properties.len() as u32).to_le_bytes());
            for (k, v) in &l.properties {
                put_str(out, k);
                put_str(out, v);
            }
        }
        Ok(())
    }

    fn decode(&self, bytes: &[u8]) -> Result<LigandSet, CodecError> {
        let mut r = Reader::new(bytes);
        let arena = r.arena()?;
        let declared = r.u32()? as usize;
        let n = r.bounded(declared, 12, "ligand count")?;
        let mut ligands = Vec::with_capacity(n);
        for _ in 0..n {
            let id = r.str()?;
            let pose = PoseId(r.u32()?);
            if arena.pose(pose).is_none() {
                return Err(CodecError::invalid(format!(
                    "ligand `{id}` names pose {} and the arena has {}",
                    pose.0,
                    arena.pose_count()
                )));
            }
            let declared = r.u32()? as usize;
            let props = r.bounded(declared, 8, "property count")?;
            let mut properties = Vec::with_capacity(props);
            for _ in 0..props {
                properties.push((r.str()?, r.str()?));
            }
            ligands.push(Ligand {
                id,
                pose,
                properties,
            });
        }
        r.end()?;
        Ok(LigandSet {
            arena: Arc::new(arena),
            ligands,
        })
    }
}

// ---- Pose set -----------------------------------------------------------------------

/// A [`PoseSet`]: the shared receptor arena, the poses arena, then the pose table.
#[derive(Debug, Clone, Copy, Default)]
pub struct PoseSetCodec;

impl Codec for PoseSetCodec {
    type Payload = PoseSet;

    fn id(&self) -> &str {
        "mold-nodes/pose-set"
    }

    fn version(&self) -> u32 {
        version(1)
    }

    fn encode(&self, p: &PoseSet, out: &mut Vec<u8>) -> Result<(), CodecError> {
        put_arena(out, &p.receptor)?;
        put_arena(out, &p.poses)?;
        let mut table = Vec::new();
        PoseTableCodec::write(&p.table, &mut table)?;
        put_blob(out, &table);
        Ok(())
    }

    fn decode(&self, bytes: &[u8]) -> Result<PoseSet, CodecError> {
        let mut r = Reader::new(bytes);
        let receptor = r.arena()?;
        let poses = r.arena()?;
        let table = {
            let mut t = Reader::new(r.blob()?);
            let df = PoseTableCodec::read(&mut t)?;
            t.end()?;
            df
        };
        r.end()?;
        if table.height() != poses.pose_count() {
            return Err(CodecError::invalid(format!(
                "pose table has {} rows and the arena {} poses",
                table.height(),
                poses.pose_count()
            )));
        }
        Ok(PoseSet {
            receptor: Arc::new(receptor),
            poses: Arc::new(poses),
            table: Arc::new(table),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{self, PoseRow};
    use mold_graph::{CodecRegistry, PortType, Value};

    fn ligand_arena() -> Arena {
        let mut arena = Arena::new();
        mold_mol::sdf::read_sdf_file(
            &mut arena,
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/egfr/reference_ligand.sdf"),
        )
        .unwrap();
        arena
    }

    fn round_trip<C: Codec + Copy>(c: C, ty: PortType, p: C::Payload) -> C::Payload
    where
        C::Payload: Clone,
    {
        let mut reg = CodecRegistry::new();
        reg.register(ty, c).unwrap();
        let e = reg.encode(&Value::new(ty, p)).unwrap();
        assert_eq!(e.codec, c.id());
        let back = reg.decode(ty, &e.codec, e.version, &e.bytes).unwrap();
        back.downcast_ref::<C::Payload>()
            .expect("the payload type")
            .clone()
    }

    #[test]
    fn a_ligand_set_round_trips() {
        let set = LigandSet::new(
            ligand_arena(),
            vec![Ligand {
                id: "IRE".into(),
                pose: PoseId(0),
                properties: vec![("comp_id".into(), "IRE".into())],
            }],
        );
        assert_eq!(
            round_trip(LigandSetCodec, PortType::LigandSet, set.clone()),
            set
        );
    }

    #[test]
    fn a_pose_set_round_trips_with_its_nulls() {
        let rows = vec![
            PoseRow {
                pose_id: 0,
                ligand_id: "IRE".into(),
                model_rank: 0,
                plddt: Some(0.91),
                gate_pass: Some(true),
                ..PoseRow::default()
            },
            PoseRow {
                pose_id: 1,
                ligand_id: "IRE".into(),
                model_rank: 1,
                ..PoseRow::default()
            },
        ];
        let ligand = ligand_arena();
        let mut poses = ligand.clone();
        crate::data::append_pose(&mut poses, &ligand, PoseId(0), Some("pose 1"), None).unwrap();
        let set = PoseSet {
            receptor: Arc::new(Arena::new()),
            poses: Arc::new(poses),
            table: Arc::new(table::to_frame(&rows)),
        };
        let back = round_trip(PoseSetCodec, PortType::PoseSet, set.clone());
        assert_eq!(back, set);
        assert_eq!(table::from_frame(&back.table).unwrap(), rows);
    }

    #[test]
    fn decoders_refuse_garbage_without_panicking() {
        for n in [0usize, 1, 7, 64, 200] {
            let bytes = vec![0xA5u8; n];
            assert!(PoseTableCodec.decode(&bytes).is_err());
            assert!(LigandSetCodec.decode(&bytes).is_err());
            assert!(PoseSetCodec.decode(&bytes).is_err());
        }
    }

    #[test]
    fn a_table_column_with_no_pose_table_dtype_is_refused() {
        let df = DataFrame::new(1, vec![Series::new("x".into(), [1i64]).into()]).unwrap();
        let e = PoseTableCodec.encode(&df, &mut Vec::new()).unwrap_err();
        assert!(e.to_string().contains("pose table holds"), "{e}");
    }
}
