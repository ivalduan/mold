//! The device and queue the renderer draws with: created headless here, or
//! handed in by whoever owns them (the viewport creates its instance with the
//! backends its surface needs).

use std::fmt;

/// A wgpu device, its queue and what it runs on.
#[derive(Debug, Clone)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::AdapterInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuError {
    /// No adapter on these backends: expected on CI runners without a GPU.
    NoAdapter(String),
    /// The adapter lacks something the impostor pipelines need.
    Unsupported(String),
    /// `request_device` failed.
    Device(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(why) => write!(f, "no wgpu adapter: {why}"),
            Self::Unsupported(why) => write!(f, "the wgpu adapter cannot run mold-render: {why}"),
            Self::Device(why) => write!(f, "creating the wgpu device: {why}"),
        }
    }
}

impl std::error::Error for GpuError {}

impl Gpu {
    /// A device for offscreen rendering (tests, CLI) on the native primary
    /// backends (Vulkan, Metal, DX12; not GL), or those named in
    /// `WGPU_BACKEND` (e.g. `dx12`, to measure what the Windows viewport
    /// uses). Fails with [`GpuError::NoAdapter`] where there is no GPU, e.g.
    /// on CI runners.
    pub fn headless() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        Self::from_instance(&instance, None, "mold-render (headless)")
    }

    /// A device on `instance`'s high-performance adapter, able to present to
    /// `compatible_surface` if one is given.
    pub fn from_instance(
        instance: &wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
        label: &str,
    ) -> Result<Self, GpuError> {
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface,
            ..Default::default()
        }))
        .map_err(|e| GpuError::NoAdapter(e.to_string()))?;
        let downlevel = adapter.get_downlevel_capabilities();
        if !downlevel
            .flags
            .contains(wgpu::DownlevelFlags::VERTEX_STORAGE)
        {
            return Err(GpuError::Unsupported(format!(
                "{} has no storage buffers in vertex shaders",
                adapter.get_info().name
            )));
        }
        let limits = adapter.limits();
        // The scene's buffers are storage bindings as large as the arena.
        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_buffer_size: limits.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some(label),
            required_limits,
            ..Default::default()
        }))
        .map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(Self {
            device,
            queue,
            adapter: adapter.get_info(),
        })
    }

    /// "NVIDIA GeForce RTX 5060 Laptop GPU (Dx12)".
    pub fn adapter_name(&self) -> String {
        format!("{} ({:?})", self.adapter.name, self.adapter.backend)
    }

    /// Blocks until all submitted work has finished.
    pub fn wait(&self) {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("waiting for the GPU");
    }
}

/// wgpu's native futures are ready on first poll or soon after; no executor
/// needed.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::thread::yield_now();
    }
}
