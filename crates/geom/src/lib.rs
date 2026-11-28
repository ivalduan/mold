//! Geometry kernels over the coordinate arena: spatial queries, clash detection,
//! pocket RMSD and the physical sanity gate checks.
//!
//! This is the product differentiator: built, not wrapped. See `CLAUDE.md`.
//!
//! Every kernel takes plain slices — positions as `&[[f32; 3]]` (what
//! [`mold_mol::Arena::pose_positions`] hands out), [`mold_mol::Element`]s, and,
//! where topology matters, a [`Bond`] list (`mold_mol`'s type, pose-local
//! indices; [`Topology::from_arena`] takes them straight from an SDF pose).
//!
//! - [`spatial`]: KD-tree ([`SpatialIndex`]) for neighbour and radius queries.
//! - [`rmsd`]: plain RMSD, Kabsch RMSD, centroid distance.
//! - [`topology`]: bond graph, rings, aromaticity, reference geometry ([`Topology`]).
//! - [`ligand`]: bond lengths, bond angles, internal clashes, aromatic ring flatness.
//! - [`receptor`]: protein–ligand distance, clashes, volume overlap, covalent attachments.
//! - [`gate`]: the checks above per pose, with thresholds ([`PoseGate`], [`GateConfig`]).
//! - [`contacts`]: H-bonds, salt bridges, π-stacking, cation–π, separate from the gate ([`contacts()`], [`ContactConfig`]).

mod linalg;

pub mod contacts;
pub mod gate;
pub mod ligand;
pub mod radii;
pub mod receptor;
pub mod rmsd;
pub mod spatial;
pub mod topology;

pub use contacts::{ContactConfig, Contacts, contacts};
pub use gate::{Accuracy, GateConfig, GateError, GateReport, PoseGate, Verdicts};
pub use receptor::{CovalentAttachment, Receptor, ResidueId};
pub use spatial::{Neighbour, SpatialIndex};
pub use topology::{Bond, BondOrder, Topology, TopologyError};

/// Re-exported so callers can use [`ligand::max_distance_from_plane`] etc. without
/// reaching into modules.
pub use ligand::max_distance_from_plane;
