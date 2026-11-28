//! **Load Structure**: a PDB/mmCIF path → `Structure`, `Sequence`.

use std::path::PathBuf;

use mold_graph::{
    CookContext, NodeError, NodeSpec, Outputs, ParamSpec, Params, PortType, Processor, Value,
};
use mold_mol::{AltLocPolicy, Arena, LoadOptions, PoseId};

use crate::seq;

/// Reads one structure file into an arena (one pose per model) and reports its
/// sequence. Chain and water handling belong to Prepare Receptor, not here.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadStructure;

pub const KIND: &str = "load_structure";

impl LoadStructure {
    fn options(params: &Params) -> Result<LoadOptions, NodeError> {
        let altloc = match params.str("altloc")? {
            "highest_occupancy" => AltLocPolicy::HighestOccupancy,
            "first" => AltLocPolicy::First,
            other => {
                return Err(NodeError::msg(format!(
                    "`altloc` is {other:?}; it is `highest_occupancy` or `first`"
                )));
            }
        };
        Ok(LoadOptions {
            altloc,
            first_model_only: params.bool("first_model_only")?,
        })
    }
}

impl Processor for LoadStructure {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(KIND)
            .output("structure", PortType::Structure)
            .output("sequence", PortType::Sequence)
            .param(ParamSpec::new("path", "").doc("the .pdb/.cif/.ent file to read (.gz is fine)"))
            .param(
                ParamSpec::new("altloc", "highest_occupancy")
                    .doc("which alternate location to keep: `highest_occupancy` or `first`"),
            )
            .param(
                ParamSpec::new("first_model_only", true)
                    .doc("read only the first model instead of one pose per model"),
            )
    }

    /// The file's content, so editing it on disk re-cooks this node and everything
    /// downstream. Params alone cannot see that.
    fn fingerprint(&self, params: &Params) -> Vec<u8> {
        crate::file_fingerprint(params.str("path").unwrap_or_default())
    }

    fn cook(&self, ctx: &CookContext<'_>) -> Result<Outputs, NodeError> {
        let path = PathBuf::from(ctx.params.str("path")?);
        if path.as_os_str().is_empty() {
            return Err(NodeError::msg("`path` is empty: name a structure file"));
        }
        let options = Self::options(ctx.params)?;
        let mut arena = Arena::new();
        let poses = mold_mol::io::read_file(&mut arena, &path, &options)
            .map_err(|e| NodeError::msg(format!("cannot read {}: {e}", path.display())))?;
        if poses.is_empty() {
            return Err(NodeError::msg(format!(
                "{} holds no model with atoms",
                path.display()
            )));
        }
        let sequence = seq::sequence(&arena, PoseId(0));
        Ok(Outputs::new()
            .with("structure", Value::new(PortType::Structure, arena))
            .with("sequence", Value::new(PortType::Sequence, sequence))
            .attr("models", poses.len() as i64))
    }
}
