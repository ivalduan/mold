//! **Load Ligands**: an SDF path → one `LigandSet` **work item per ligand**.
//!
//! The fan-out is here, not in Co-fold: the item count is then known before anything
//! cooks (progress), and one ligand is the unit that re-runs, caches and fails on its
//! own. Each item's `LigandSet` holds exactly one ligand.
//!
//! SMILES (`fixtures/egfr/ligands.smi`) needs `chem`, which is not in this build:
//! this node reads what `mol` reads today, which is SDF/molfile with explicit bonds.

use std::path::{Path, PathBuf};

use mold_graph::{
    AttrValue, CookContext, GenerateContext, NewItem, NodeError, NodeSpec, Outputs, ParamSpec,
    Params, PortType, Processor, Value,
};
use mold_mol::Arena;
use mold_mol::sdf::{SdfRead, SdfRecord};

use crate::data::{Ligand, LigandSet, single_pose_arena};

/// Reads an SDF and generates one item per record.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadLigands;

pub const KIND: &str = "load_ligands";

/// The attribute every item carries: its record's position in the file.
pub const INDEX_ATTR: &str = "ligand_index";
/// The attribute every item carries: its ligand's id.
pub const ID_ATTR: &str = "ligand_id";

fn read(params: &Params) -> Result<(PathBuf, Arena, SdfRead), NodeError> {
    let path = PathBuf::from(params.str("path")?);
    if path.as_os_str().is_empty() {
        return Err(NodeError::msg("`path` is empty: name an SDF file"));
    }
    let mut arena = Arena::new();
    let read = mold_mol::sdf::read_sdf_file(&mut arena, &path)
        .map_err(|e| NodeError::msg(format!("cannot read {}: {e}", path.display())))?;
    if !params.bool("skip_bad_records")?
        && let Some(bad) = read.errors.first()
    {
        return Err(NodeError::msg(format!(
            "{}: {bad} ({} record(s) failed; set `skip_bad_records` to ignore them)",
            path.display(),
            read.errors.len()
        )));
    }
    if read.records.is_empty() {
        return Err(NodeError::msg(format!(
            "{} holds no readable record",
            path.display()
        )));
    }
    Ok((path, arena, read))
}

/// The ligand's id: a named data item if `id_property` asks for one, else the record's
/// title, else its position in the file.
fn ligand_id(record: &SdfRecord, id_property: &str, path: &Path) -> String {
    if !id_property.is_empty()
        && let Some(v) = record.property(id_property)
    {
        return v.trim().to_string();
    }
    if !record.title.trim().is_empty() {
        return record.title.trim().to_string();
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ligands");
    format!("{stem}-{}", record.index)
}

impl Processor for LoadLigands {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(KIND)
            .output("ligands", PortType::LigandSet)
            .param(ParamSpec::new("path", "").doc("the .sdf/.mol file to read"))
            .param(
                ParamSpec::new("id_property", "")
                    .doc("take each ligand's id from this data item instead of the title"),
            )
            .param(
                ParamSpec::new("skip_bad_records", true)
                    .doc("skip records that do not parse instead of failing the node"),
            )
    }

    fn fingerprint(&self, params: &Params) -> Vec<u8> {
        crate::file_fingerprint(params.str("path").unwrap_or_default())
    }

    /// One item per record, with its id and index as attributes.
    fn generate(&self, ctx: &GenerateContext<'_>) -> Result<Vec<NewItem>, NodeError> {
        let (path, _, read) = read(ctx.params)?;
        let id_property = ctx.params.str("id_property")?;
        Ok(read
            .records
            .iter()
            .map(|r| {
                NewItem::work()
                    .attr(INDEX_ATTR, r.index as i64)
                    .attr(ID_ATTR, ligand_id(r, id_property, &path))
            })
            .collect())
    }

    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let Some(AttrValue::Int(index)) = ctx.attrs.get(INDEX_ATTR) else {
            return Err(NodeError::msg(format!("the item has no `{INDEX_ATTR}`")));
        };
        let (path, arena, read) = read(ctx.params)?;
        let record = read
            .records
            .iter()
            .find(|r| r.index as i64 == *index)
            .ok_or_else(|| {
                NodeError::msg(format!("{} no longer holds record {index}", path.display()))
            })?;
        let id = ligand_id(record, ctx.params.str("id_property")?, &path);
        let one = single_pose_arena(&arena, record.pose, Some(&id))
            .ok_or_else(|| NodeError::msg("the record's pose vanished from the arena"))?;
        let atoms = one.atom_count() as i64;
        let ligand = Ligand {
            id: id.clone(),
            pose: mold_mol::PoseId(0),
            properties: record.properties.clone(),
        };
        Ok(Outputs::new()
            .with(
                "ligands",
                Value::new(PortType::LigandSet, LigandSet::new(one, vec![ligand])),
            )
            .attr(ID_ATTR, id)
            .attr("atoms", atoms))
    }
}
