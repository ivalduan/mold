//! **SPIKE ONLY (WS-00, week 0).** Stand-ins for what `mold-render` (WS-08)
//! owns: an orbit camera, CPU sphere picking, a 500-atom scene and a
//! one-pipeline impostor renderer on its own device and thread. They exist to
//! prove the viewport path (ADR-0003) and the latency budget, and are to be
//! replaced by `mold-render`, not grown. Nothing here depends on GPUI.

pub mod camera;
pub mod pick;
pub mod renderer;
pub mod scene;
