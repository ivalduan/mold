//! Molecular rendering in pure `wgpu`: camera, scene, impostor pipelines,
//! picking, offscreen rendering. Never sees GPUI and owns no window or
//! surface; it is handed a device (or creates one headless) and draws into a
//! texture or a surface someone else made.
//!
//! - [`Gpu`]: device + queue, created headless or from an instance.
//! - [`OrbitCamera`]: orbit / pan / zoom, fit to bounds, re-aim on a point.
//! - [`Scene`]: an [`Arena`](mold_mol::Arena) on the GPU (positions as-is),
//!   bonds as atom index pairs, per-pose visibility as draw ranges.
//! - [`Renderer`]: sphere and stick impostors (two triangles each, ray-cast
//!   per fragment, exact depth) into a colour target and an atom-id target.
//! - [`pick`] / [`FrameTargets::read_id`]: GPU readback of the atom under a
//!   pixel → `(PoseId, atom)`.
//! - [`OffscreenTarget`]: render to a texture and read back RGBA.
//! - [`host`]: the interactive render thread the viewport drives.
//!
//! See `CLAUDE.md` in this crate.

mod bonds;
mod camera;
mod gpu;
pub mod hash;
pub mod host;
pub mod ray;
mod renderer;
mod scene;
mod style;

pub use bonds::{BOND_TOLERANCE, MIN_BOND_LENGTH, guess_bonds, guess_bonds_in};
pub use camera::{BoundingSphere, OrbitCamera, Ray};
pub use gpu::{Gpu, GpuError};
pub use renderer::{
    DEFAULT_CLEAR, DEPTH_FORMAT, FrameParams, FrameTargets, HIGHLIGHT, ID_FORMAT, Image,
    OFFSCREEN_FORMAT, OffscreenTarget, Pick, Renderer, pick,
};
pub use scene::{DrawRun, Scene, SceneError};
pub use style::{Radii, Representation, STICK_RADIUS, covalent_radius, element_color, vdw_radius};
