//! A GPUI `Element` whose content `mold-render` draws through wgpu, with GPUI
//! painting menus and panels above it. Holds the thin `ViewportBackend` seam.
//! See `CLAUDE.md` in this crate and ADR-0002 / ADR-0003.
//!
//! - [`backend`]: the seam, platform-neutral, no GPUI.
//! - `element` (Windows only for now): the GPUI element and native surface,
//!   feeding a [`mold_render::host::RenderHost`] (own device + render thread).
//!
//! GPUI is a dependency on Windows only until the macOS and Linux backends
//! exist (ADR-0003), so the crate still builds and tests headless elsewhere.

pub mod backend;

#[cfg(windows)]
pub mod element;
#[cfg(windows)]
mod platform_windows;

#[cfg(windows)]
pub use platform_windows::WindowsBackend;
