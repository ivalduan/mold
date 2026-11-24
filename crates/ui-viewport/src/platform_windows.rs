//! The Windows backend: GPUI Fast's native surface is a DirectComposition
//! visual in GPUI's composition tree; wgpu (DX12, its own device) puts a swap
//! chain on it. Ported from `javagg/gpui-fast-wgpu-demo` `src/viewport.rs`.

use std::any::Any;

use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
use windows::core::{IUnknown, Interface as _};

use crate::backend::{BackendError, ViewportBackend};

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsBackend;

impl ViewportBackend for WindowsBackend {
    fn create_surface(
        &self,
        instance: &wgpu::Instance,
        platform_handle: Box<dyn Any>,
    ) -> Result<wgpu::Surface<'static>, BackendError> {
        // The one downcast of the untyped handle. It only matches if this
        // crate's `windows` resolves to the same `windows-core` as
        // gpui-fast-windows (both 0.62 today).
        let unknown =
            platform_handle
                .downcast::<IUnknown>()
                .map_err(|_| BackendError::UnexpectedHandle {
                    platform: "Windows",
                    expected: "windows::core::IUnknown (an IDCompositionVisual)",
                })?;
        let visual: IDCompositionVisual = unknown.cast().map_err(|e| {
            BackendError::Platform(format!(
                "gpui-fast's native surface is a COM object but not an IDCompositionVisual: {e}"
            ))
        })?;
        // SAFETY: `visual` is a live IDCompositionVisual. wgpu takes its own
        // reference and holds it for as long as the surface lives, so dropping
        // ours at the end of this function is fine.
        unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                visual.as_raw(),
            ))
        }
        .map_err(|e| {
            BackendError::Platform(format!(
                "creating a wgpu surface on the composition visual: {e}"
            ))
        })
    }

    /// wgpu sets the swap chain as the visual's content but cannot commit:
    /// GPUI owns the DirectComposition device. Without a commit after the first
    /// present the viewport stays black until something else commits.
    fn needs_commit_after_present(&self) -> bool {
        true
    }
}
