//! The `ViewportBackend` seam: the only platform-specific part of the
//! viewport. It answers two questions and nothing else:
//!
//! 1. *Give me a wgpu surface for this GPUI native surface.* GPUI Fast hands
//!    out the native surface as an untyped `Box<dyn Any>`
//!    (`PlatformSurfaceAttachment::platform_handle`); the backend downcasts it
//!    once, here, and fails loudly if the type is not the one it was built
//!    against.
//! 2. *Does this platform need an explicit commit after presenting?* On
//!    Windows wgpu attaches its swap chain to a DirectComposition visual it
//!    does not own, so GPUI has to commit the composition after the first
//!    present following each configure.
//!
//! The trait sees neither GPUI nor the renderer: it takes the handle as
//! `Any` and returns a `wgpu::Surface`, so it builds (and is tested) on every
//! platform. Camera, scene, pipelines and picking live above it.

use std::any::Any;
use std::fmt;

/// See the module docs. Implemented once per platform.
pub trait ViewportBackend {
    /// Creates a wgpu surface on the platform handle of a GPUI Fast native
    /// surface (`surface.platform_surface()?.platform_handle()?`).
    ///
    /// `instance` must have been created with [`wgpu_backends`].
    fn create_surface(
        &self,
        instance: &wgpu::Instance,
        platform_handle: Box<dyn Any>,
    ) -> Result<wgpu::Surface<'static>, BackendError>;

    /// Whether the composition must be committed after the first present
    /// following each `Surface::configure`.
    fn needs_commit_after_present(&self) -> bool;
}

/// The gpui-fast release this crate's handle downcasts were tested against.
/// Must equal the `=` pin in the root `Cargo.toml` (ADR-0002).
pub const GPUI_FAST_TESTED: &str = "0.1.19";

#[derive(Debug)]
pub enum BackendError {
    /// `platform_handle()` returned a type this backend was not built for.
    /// This is a run-time failure of a dependency upgrade, not a user error.
    UnexpectedHandle {
        platform: &'static str,
        expected: &'static str,
    },
    /// The platform call failed.
    Platform(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedHandle { platform, expected } => write!(
                f,
                "the viewport cannot attach to this window: gpui-fast's native surface \
                 handle on {platform} is not a {expected}. This crate was tested against \
                 gpui-fast {GPUI_FAST_TESTED}; a gpui-fast (or `windows` crate) upgrade has \
                 changed `PlatformSurfaceAttachment::platform_handle()`. Pin the tested \
                 version or update the backend (see adr/0002-gpui-lineage.md)."
            ),
            Self::Platform(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for BackendError {}

/// The wgpu backends whose surfaces the platform's backend can create.
/// Windows: DX12, the only wgpu backend that can target a DirectComposition
/// visual.
pub fn wgpu_backends() -> wgpu::Backends {
    if cfg!(windows) {
        wgpu::Backends::DX12
    } else if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else {
        wgpu::Backends::VULKAN
    }
}

/// The backend for the platform this was built for, if one exists yet.
/// Only Windows does in v0.1 (ADR-0003).
pub fn native_backend() -> Option<Box<dyn ViewportBackend>> {
    #[cfg(windows)]
    {
        Some(Box::new(crate::platform_windows::WindowsBackend))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_error_names_the_tested_gpui_fast() {
        let message = BackendError::UnexpectedHandle {
            platform: "Windows",
            expected: "windows::core::IUnknown",
        }
        .to_string();
        assert!(message.contains(GPUI_FAST_TESTED));
        assert!(message.contains("adr/0002"));
    }

    /// The `=` pin in the root manifest and the tested version must agree.
    #[test]
    fn tested_version_matches_the_workspace_pin() {
        let manifest = include_str!("../../../Cargo.toml");
        let pin = format!("gpui-fast = {{ version = \"={GPUI_FAST_TESTED}\"");
        assert!(manifest.contains(&pin), "root Cargo.toml lacks `{pin}`");
    }

    #[cfg(windows)]
    #[test]
    fn windows_backend_rejects_a_foreign_handle_loudly() {
        let backend = native_backend().expect("Windows has a backend");
        assert!(backend.needs_commit_after_present());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu_backends(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        // What a changed gpui-fast might hand out instead, e.g. a raw pointer.
        let error = match backend.create_surface(&instance, Box::new(0usize)) {
            Ok(_) => panic!("a usize is not a composition visual"),
            Err(error) => error,
        };
        assert!(
            matches!(error, BackendError::UnexpectedHandle { .. }),
            "{error}"
        );
    }
}
