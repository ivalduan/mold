//! Recipe serialization: the visual graph as clean, human-readable, Git-diffable
//! TOML, and back. Serialization is a projection of the graph model, not a
//! translation. See `CLAUDE.md` and `FORMAT.md` in this crate.
//!
//! ```
//! use mold_graph::{Graph, NodeData};
//! use mold_recipe::{Recipe, from_toml};
//!
//! // The table key is the node's fixed id; `kind` is its type, `label` its name.
//! let mut graph = Graph::new();
//! let mut node = NodeData::new("load_structure");
//! node.label = Some("EGFR 4WKQ".into());
//! node.params.set("path", "fixtures/egfr/receptor.cif");
//! graph.nodes.insert(graph.fresh_id("load_structure"), node);
//!
//! let text = Recipe::new(graph.clone()).to_toml();
//! assert_eq!(
//!     text,
//!     "[recipe]\nformat = 1\n\n\
//!      [node.load_structure-1]\nkind = \"load_structure\"\nlabel = \"EGFR 4WKQ\"\n\n\
//!      [node.load_structure-1.params]\npath = \"fixtures/egfr/receptor.cif\"\n"
//! );
//! assert_eq!(from_toml(&text).unwrap().graph, graph);
//! ```

mod error;
mod probe;
mod read;
mod write;

use std::collections::BTreeMap;

use mold_graph::{Graph, NodeId, ParamValue, Registry};

pub use error::{ErrorKind, RecipeError};
pub use write::to_toml;

/// The `[recipe] format` this build writes and reads. Bump on an incompatible change
/// to the text format (not on new defaulted graph fields: those are additive).
pub const FORMAT_VERSION: u32 = 1;

/// Everything a recipe file holds: the graph (what replays) plus metadata that never
/// affects a result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recipe {
    pub graph: Graph,
    /// Canvas metadata. Optional in the file; ignored by headless replay; never part
    /// of a cache key (it is not in `Graph`).
    pub layout: Layout,
    /// What produced the results last time (MSA provider, worker/model versions).
    /// Data only: stored and round-tripped, filled by `nodes`/`jobs` later.
    pub provenance: Provenance,
}

impl Recipe {
    /// A recipe with no layout and no provenance.
    pub fn new(graph: Graph) -> Self {
        Self {
            graph,
            ..Self::default()
        }
    }

    /// The canonical text; see [`to_toml`].
    pub fn to_toml(&self) -> String {
        to_toml(&self.graph, &self.layout, &self.provenance)
    }
}

/// Canvas positions, keyed by node id: `[layout.positions]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// World position of each node's top-left corner (`ui-graph`'s
    /// `CanvasState::positions`, whose `Vec2` is `f32` too).
    pub positions: BTreeMap<NodeId, Position>,
}

/// A canvas position in world units.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

/// Per-node provenance records: `[provenance.<id>]`. Keys are owned by whoever
/// writes them (e.g. `msa_provider`, `worker`, `worker_version`, `model_version`);
/// the format only fixes the shape. An empty record is not written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Provenance {
    pub nodes: BTreeMap<NodeId, BTreeMap<String, ParamValue>>,
}

/// Parse a recipe, checking its shape and the graph's structure (every connection
/// names an existing node, no cycles) but not node kinds, params or port types: for
/// that, see [`from_toml_with`].
pub fn from_toml(text: &str) -> Result<Recipe, RecipeError> {
    read::read(text, None)
}

/// [`from_toml`], plus [`Graph::validate`] against `registry` (unknown kinds,
/// params, ports, wrong param types, mismatched connections), each error placed at
/// the offending key or value.
pub fn from_toml_with(text: &str, registry: &Registry) -> Result<Recipe, RecipeError> {
    read::read(text, Some(registry))
}
