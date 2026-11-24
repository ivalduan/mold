//! SPIKE ONLY (WS-00). The viewport's own wgpu device and render thread,
//! drawing the spike scene as sphere impostors into the surface the
//! [`ViewportBackend`](crate::backend::ViewportBackend) made.
//!
//! This is the shape `mold-render` (WS-08) takes over, not its code: one
//! device, one thread, commands in over a channel, events out over another.
//! Nothing here sees GPUI. A slow frame blocks this thread only; the UI
//! thread only ever sends messages.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glam::Vec2;

use super::camera::OrbitCamera;
use super::pick::{Sphere, pick};
use super::scene::{SphereInstance, SpikeScene};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerKind {
    Down(Button),
    Up(Button),
    Move,
    /// Lines; positive zooms in.
    Scroll(f32),
    Leave,
}

/// A pointer event, already in the surface's physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub kind: PointerKind,
    pub x: f32,
    pub y: f32,
    /// In the window's logical pixels, for placing GPUI popups.
    pub window_x: f32,
    pub window_y: f32,
    /// When the UI thread received the event: where click latency starts.
    pub at: Instant,
}

/// UI thread → render thread.
#[derive(Debug)]
pub enum Msg {
    /// The surface, already configured once on the UI thread.
    Attach(wgpu::Surface<'static>),
    Resize {
        width: u32,
        height: u32,
    },
    Pointer(Pointer),
    SetVisible(bool),
    /// Keep turning the camera, so every frame is a new one.
    SetSpin(bool),
    /// Sleep this long in every frame, to show a slow frame does not block
    /// the UI thread.
    SetFrameDelay(Duration),
    /// Self-test: click where atom `index` is drawn, as if at `at`.
    ClickAtom {
        index: usize,
        at: Instant,
    },
    Shutdown,
}

/// Render thread → UI thread.
#[derive(Clone, Debug)]
pub enum Event {
    /// The first frame has been presented.
    Ready,
    /// A newly configured swap chain has presented: on platforms whose
    /// backend says so, the UI thread must commit the composition now.
    CommitComposition,
    /// The viewport was pressed: dismiss open popups.
    Pressed,
    ContextMenu {
        window_x: f32,
        window_y: f32,
    },
    Picked(PickReport),
    Stats(Stats),
    Failed(String),
}

#[derive(Clone, Copy, Debug)]
pub struct PickReport {
    pub atom: Option<usize>,
    /// CPU ray cast over all atoms.
    pub pick_us: f32,
    /// From the UI thread receiving the press to `present` returning for the
    /// first frame with the re-aimed camera. `None` on a miss.
    pub click_to_present_ms: Option<f32>,
    /// How far the picked atom's centre lands from the viewport centre after
    /// re-aiming, in physical pixels.
    pub centring_error_px: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub frames_total: u64,
    pub fps: f32,
    /// Render-thread time per frame, including the wait for a swap-chain
    /// image (vsync), so not a GPU or encode cost.
    pub cpu_frame_ms: f32,
    pub width: u32,
    pub height: u32,
}

/// The UI thread's side: the device surfaces are created on, and the channel.
#[derive(Debug)]
pub struct RenderHost {
    instance: wgpu::Instance,
    device: wgpu::Device,
    config: wgpu::SurfaceConfiguration,
    commands: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    pub adapter: String,
}

impl RenderHost {
    pub fn new(scene: SpikeScene, events: Sender<Event>) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: crate::backend::wgpu_backends(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| {
            format!(
                "no wgpu adapter for {:?}: {e}",
                crate::backend::wgpu_backends()
            )
        })?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mold viewport (spike)"),
            ..Default::default()
        }))
        .map_err(|e| format!("creating the viewport's wgpu device: {e}"))?;
        let config = surface_config();
        let (commands, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("mold-viewport".into())
            .spawn({
                let (device, config) = (device.clone(), config.clone());
                move || {
                    let failures = events.clone();
                    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
                        eprintln!("wgpu error: {error}");
                        failures.send(Event::Failed(error.to_string())).ok();
                    }));
                    // Pipelines compile here, off the UI thread.
                    let renderer = Renderer::new(device, queue, config, scene, events);
                    renderer.run(receiver);
                }
            })
            .map_err(|e| format!("spawning the render thread: {e}"))?;
        Ok(Self {
            instance,
            device,
            config,
            commands,
            thread: Some(thread),
            adapter: adapter_name,
        })
    }

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// Configures `surface` on this thread and hands it to the render thread.
    /// The first configure is what attaches the swap chain to the native
    /// surface, so the caller commits right after when the backend asks for it.
    pub fn attach(&self, surface: wgpu::Surface<'static>) {
        surface.configure(&self.device, &self.config);
        self.send(Msg::Attach(surface));
    }

    pub fn send(&self, msg: Msg) {
        self.commands.send(msg).ok();
    }

    pub fn sender(&self) -> Sender<Msg> {
        self.commands.clone()
    }
}

impl Drop for RenderHost {
    fn drop(&mut self) {
        self.send(Msg::Shutdown);
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

fn surface_config() -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        // DX12 offers it on every surface.
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: 1,
        height: 1,
        // vsync blocks only the render thread.
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 1,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        color_space: wgpu::SurfaceColorSpace::Auto,
        view_formats: Vec::new(),
    }
}

/// wgpu's native futures are ready on first poll; no executor needed.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::thread::yield_now();
    }
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    selected: [i32; 4],
}

struct Press {
    last: Vec2,
    dragged: bool,
}

/// A click waiting for the frame that shows its result.
struct PendingPick {
    at: Instant,
    atom: usize,
    pick_us: f32,
}

struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    surface: Option<wgpu::Surface<'static>>,
    events: Sender<Event>,
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    depth: Option<wgpu::TextureView>,
    spheres: Vec<Sphere>,
    camera: OrbitCamera,
    selected: Option<usize>,
    visible: bool,
    dirty: bool,
    spin: bool,
    frame_delay: Duration,
    commit_pending: bool,
    announced_ready: bool,
    press: Option<Press>,
    pending_pick: Option<PendingPick>,
    frames_total: u64,
    frames_since_report: u32,
    cpu_ms_since_report: f32,
    last_report: Instant,
}

impl Renderer {
    fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        scene: SpikeScene,
        events: Sender<Event>,
    ) -> Self {
        use wgpu::util::DeviceExt as _;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("impostor (spike)"),
            source: wgpu::ShaderSource::Wgsl(include_str!("impostor.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<Globals>() as u64),
                },
                count: None,
            }],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("impostor (spike)"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("impostor (spike)"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<SphereInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                // Reversed Z.
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(config.format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spheres"),
            contents: bytemuck::cast_slice(&scene.atoms),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let camera = OrbitCamera::framing(scene.center, scene.radius);
        Self {
            device,
            queue,
            config,
            surface: None,
            events,
            pipeline,
            globals,
            bind_group,
            instances,
            depth: None,
            spheres: scene.spheres().collect(),
            camera,
            selected: None,
            visible: true,
            dirty: true,
            spin: false,
            frame_delay: Duration::ZERO,
            commit_pending: true,
            announced_ready: false,
            press: None,
            pending_pick: None,
            frames_total: 0,
            frames_since_report: 0,
            cpu_ms_since_report: 0.0,
            last_report: Instant::now(),
        }
    }

    fn run(mut self, commands: Receiver<Msg>) {
        loop {
            if !self.wants_frame() {
                self.report(true);
                match commands.recv() {
                    Ok(msg) => {
                        if !self.handle(msg) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
            while let Ok(msg) = commands.try_recv() {
                if !self.handle(msg) {
                    return;
                }
            }
            if self.wants_frame() {
                self.frame();
                self.report(false);
            }
        }
    }

    fn ready(&self) -> bool {
        self.surface.is_some() && self.config.width > 1 && self.config.height > 1
    }

    fn wants_frame(&self) -> bool {
        self.visible && self.ready() && (self.dirty || self.spin)
    }

    fn size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    /// Returns false on shutdown.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Attach(surface) => {
                self.surface = Some(surface);
                self.commit_pending = true;
                self.dirty = true;
            }
            Msg::Resize { width, height } => self.resize(width, height),
            Msg::Pointer(pointer) => self.pointer(pointer),
            Msg::SetVisible(visible) => {
                self.visible = visible;
                self.press = None;
                if visible {
                    self.dirty = true;
                } else {
                    // A hidden viewport gives up its depth target; the swap
                    // chain keeps the last frame for when it comes back.
                    self.depth = None;
                }
            }
            Msg::ClickAtom { index, at } => {
                let (width, height) = self.size();
                let Some(sphere) = self.spheres.get(index) else {
                    return true;
                };
                match self.camera.project(sphere.center, width, height) {
                    Some(pixel) => self.click(pixel, at),
                    None => eprintln!("self-test: atom {index} is behind the camera"),
                }
            }
            Msg::SetSpin(spin) => {
                self.spin = spin;
                self.dirty = true;
            }
            Msg::SetFrameDelay(delay) => self.frame_delay = delay,
            Msg::Shutdown => return false,
        }
        true
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
            self.commit_pending = true;
        }
        self.depth = None;
        self.dirty = true;
    }

    fn pointer(&mut self, pointer: Pointer) {
        let position = Vec2::new(pointer.x, pointer.y);
        match pointer.kind {
            PointerKind::Down(button) => {
                self.events.send(Event::Pressed).ok();
                match button {
                    Button::Left => {
                        self.press = Some(Press {
                            last: position,
                            dragged: false,
                        });
                        self.click(position, pointer.at);
                    }
                    Button::Right => {
                        self.events
                            .send(Event::ContextMenu {
                                window_x: pointer.window_x,
                                window_y: pointer.window_y,
                            })
                            .ok();
                    }
                    Button::Middle => {}
                }
            }
            PointerKind::Up(Button::Left) | PointerKind::Leave => self.press = None,
            PointerKind::Up(_) => {}
            PointerKind::Move => {
                if let Some(press) = &mut self.press {
                    let delta = position - press.last;
                    if press.dragged || delta.length() > 3.0 {
                        press.dragged = true;
                        press.last = position;
                        self.camera.orbit(delta.x, delta.y);
                        self.dirty = true;
                    }
                }
            }
            PointerKind::Scroll(lines) => {
                self.camera.zoom(lines);
                self.dirty = true;
            }
        }
    }

    /// Picks under `position` and, on a hit, re-aims at once: the next frame
    /// is the re-aimed one, and its present closes the latency measurement.
    fn click(&mut self, position: Vec2, at: Instant) {
        let (width, height) = self.size();
        let started = Instant::now();
        let ray = self
            .camera
            .ray_through(position.x, position.y, width, height);
        let hit = pick(&ray, self.spheres.iter().copied());
        let pick_us = started.elapsed().as_secs_f32() * 1e6;
        match hit {
            Some((atom, _)) => {
                self.camera.re_aim(self.spheres[atom].center);
                self.selected = Some(atom);
                self.dirty = true;
                self.pending_pick = Some(PendingPick { at, atom, pick_us });
            }
            None => {
                self.events
                    .send(Event::Picked(PickReport {
                        atom: None,
                        pick_us,
                        click_to_present_ms: None,
                        centring_error_px: None,
                    }))
                    .ok();
            }
        }
    }

    fn ensure_depth(&mut self) -> wgpu::TextureView {
        self.depth
            .get_or_insert_with(|| {
                self.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("depth"),
                        size: wgpu::Extent3d {
                            width: self.config.width,
                            height: self.config.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: DEPTH_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            })
            .clone()
    }

    fn frame(&mut self) {
        let started = Instant::now();
        let depth = self.ensure_depth();
        let Some(surface) = &self.surface else {
            return;
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&self.device, &self.config);
                self.commit_pending = true;
                return;
            }
            _ => {
                // Occluded or timed out: stay dirty and try again shortly.
                std::thread::sleep(Duration::from_millis(16));
                return;
            }
        };
        if self.spin {
            self.camera.orbit(2.0, 0.0);
        }
        if !self.frame_delay.is_zero() {
            std::thread::sleep(self.frame_delay);
        }
        let (width, height) = self.size();
        let globals = Globals {
            view: self.camera.view().to_cols_array_2d(),
            proj: self.camera.projection(width / height).to_cols_array_2d(),
            selected: [self.selected.map_or(-1, |i| i as i32), 0, 0, 0],
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("impostors (spike)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.015,
                            g: 0.02,
                            b: 0.035,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..4, 0..self.spheres.len() as u32);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        self.dirty = false;
        self.frames_total += 1;
        self.frames_since_report += 1;
        self.cpu_ms_since_report += started.elapsed().as_secs_f32() * 1000.0;

        // wgpu attaches the swap chain but cannot commit a composition it does
        // not own; whoever owns it must, after the first present following
        // each configure.
        if std::mem::take(&mut self.commit_pending) {
            self.events.send(Event::CommitComposition).ok();
        }
        if !self.announced_ready {
            self.announced_ready = true;
            self.events.send(Event::Ready).ok();
        }
        if let Some(pending) = self.pending_pick.take() {
            let click_to_present_ms = pending.at.elapsed().as_secs_f32() * 1000.0;
            let center = self.spheres[pending.atom].center;
            let centring_error_px = self
                .camera
                .project(center, width, height)
                .map(|p| (p - Vec2::new(width, height) * 0.5).length());
            self.events
                .send(Event::Picked(PickReport {
                    atom: Some(pending.atom),
                    pick_us: pending.pick_us,
                    click_to_present_ms: Some(click_to_present_ms),
                    centring_error_px,
                }))
                .ok();
        }
    }

    fn report(&mut self, idle: bool) {
        let elapsed = self.last_report.elapsed().as_secs_f32();
        if self.frames_since_report == 0 || (!idle && elapsed < 0.5) {
            return;
        }
        let frames = self.frames_since_report as f32;
        self.events
            .send(Event::Stats(Stats {
                frames_total: self.frames_total,
                fps: if idle {
                    0.0
                } else {
                    frames / elapsed.max(1e-3)
                },
                cpu_frame_ms: self.cpu_ms_since_report / frames,
                width: self.config.width,
                height: self.config.height,
            }))
            .ok();
        self.frames_since_report = 0;
        self.cpu_ms_since_report = 0.0;
        self.last_report = Instant::now();
    }
}
