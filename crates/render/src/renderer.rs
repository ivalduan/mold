//! The impostor pipelines, the per-frame targets (atom ids + depth), the
//! offscreen colour target, and the readbacks (RGBA image, picked id).

use std::num::NonZeroU64;

use mold_mol::PoseId;

use crate::camera::OrbitCamera;
use crate::scene::Scene;
use crate::style::{ElementStyle, Representation, element_table};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Atom index + 1 per pixel; 0 is background.
pub const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// What [`OffscreenTarget`] renders to and [`Image`] holds.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The background, linear RGB.
pub const DEFAULT_CLEAR: [f32; 3] = [0.015, 0.02, 0.035];
/// The selected atom's colour, linear RGB.
pub const HIGHLIGHT: [f32; 3] = [0.2, 1.0, 0.45];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    viewport: [f32; 4],
    radii: [f32; 4],
    selected: [u32; 4],
    highlight: [f32; 4],
}

/// Everything about one frame that is not the scene or the targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameParams {
    pub camera: OrbitCamera,
    pub representation: Representation,
    /// A global atom index drawn in [`HIGHLIGHT`].
    pub selected: Option<u32>,
    pub clear: [f32; 3],
}

impl FrameParams {
    pub fn new(camera: OrbitCamera, representation: Representation) -> Self {
        Self {
            camera,
            representation,
            selected: None,
            clear: DEFAULT_CLEAR,
        }
    }
}

/// The pipelines for one colour format. Create once per device and target
/// format; it holds the per-frame uniforms, so one renderer draws one frame
/// at a time.
#[derive(Debug)]
pub struct Renderer {
    color_format: wgpu::TextureFormat,
    spheres: wgpu::RenderPipeline,
    sticks: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    pub(crate) scene_layout: wgpu::BindGroupLayout,
}

fn uniform_entry(binding: u32, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Renderer {
    /// Compiles the pipelines for drawing into `color_format` (the surface's
    /// format, or [`OFFSCREEN_FORMAT`]).
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        use wgpu::util::DeviceExt as _;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("impostor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("impostor.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                uniform_entry(0, size_of::<Globals>()),
                uniform_entry(1, size_of::<[ElementStyle; 256]>()),
            ],
        });
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &[storage_entry(0), storage_entry(1), storage_entry(2)],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("element table"),
            contents: bytemuck::cast_slice(&element_table()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: table.as_entire_binding(),
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("impostor"),
            bind_group_layouts: &[Some(&globals_layout), Some(&scene_layout)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, vs: &str, fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
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
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(color_format.into()), Some(ID_FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            color_format,
            spheres: pipeline("spheres", "sphere_vs", "sphere_fs"),
            sticks: pipeline("sticks", "stick_vs", "stick_fs"),
            globals,
            globals_group,
            scene_layout,
        }
    }

    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    /// Records one frame of `scene` into `color` (a view of a texture in
    /// [`Renderer::color_format`]) and `targets`, which must be the same size.
    /// Writes the uniforms through `queue`, so submit before the next call.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        params: &FrameParams,
        color: &wgpu::TextureView,
        targets: &FrameTargets,
    ) {
        let (width, height) = (targets.width as f32, targets.height as f32);
        let radii = params.representation.radii();
        let camera = &params.camera;
        let globals = Globals {
            view: camera.view().to_cols_array_2d(),
            proj: camera.projection(width / height).to_cols_array_2d(),
            viewport: [width, height, 0.0, 0.0],
            radii: [
                radii.sphere_scale,
                radii.sphere_min,
                radii.stick_radius,
                0.0,
            ],
            selected: [params.selected.map_or(0, |a| a + 1), 0, 0, 0],
            highlight: [HIGHLIGHT[0], HIGHLIGHT[1], HIGHLIGHT[2], 1.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        let [r, g, b] = params.clear.map(f64::from);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("impostors"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &targets.id_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        // Kept for picking.
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth_view,
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
        let runs = scene.draw_runs();
        pass.set_bind_group(0, &self.globals_group, &[]);
        pass.set_bind_group(1, &scene.bind_group, &[]);
        pass.set_pipeline(&self.spheres);
        for run in &runs {
            if !run.atoms.is_empty() {
                pass.draw(0..4, run.atoms.clone());
            }
        }
        if params.representation.draws_sticks() {
            pass.set_pipeline(&self.sticks);
            for run in &runs {
                if !run.bonds.is_empty() {
                    pass.draw(0..4, run.bonds.clone());
                }
            }
        }
    }
}

/// The id and depth targets of one viewport size. The id target keeps the
/// last frame's atom ids for [`FrameTargets::read_id`].
#[derive(Debug)]
pub struct FrameTargets {
    pub width: u32,
    pub height: u32,
    id: wgpu::Texture,
    id_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

impl FrameTargets {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let size = (width.max(1), height.max(1));
        let id = texture(
            device,
            "atom ids",
            size,
            ID_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = texture(
            device,
            "depth",
            size,
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick readback"),
            size: u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            width: size.0,
            height: size.1,
            id_view: id.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            id,
            readback,
        }
    }

    /// The global atom index drawn at pixel `(x, y)` in the last frame, or
    /// `None` for background or outside. Blocks until the GPU has copied it
    /// (desktop only; about a frame's latency at most).
    pub fn read_id(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        x: f32,
        y: f32,
    ) -> Option<u32> {
        if !(x >= 0.0 && y >= 0.0 && x < self.width as f32 && y < self.height as f32) {
            return None;
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.id,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: x as u32,
                    y: y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None,
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let bytes = map_and_read(device, &self.readback, 4);
        let id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        id.checked_sub(1)
    }
}

fn map_and_read(device: &wgpu::Device, buffer: &wgpu::Buffer, len: u64) -> Vec<u8> {
    let slice = buffer.slice(..len);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting for a readback");
    rx.recv()
        .expect("map_async callback ran")
        .expect("mapping a readback buffer");
    let bytes = slice
        .get_mapped_range()
        .expect("reading the mapped range")
        .to_vec();
    buffer.unmap();
    bytes
}

/// What a pick found: the pose and the global atom index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub pose: PoseId,
    pub atom: u32,
}

/// Reads the atom under pixel `(x, y)` from the last frame drawn into
/// `targets` and maps it to its pose.
pub fn pick(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    targets: &FrameTargets,
    scene: &Scene,
    x: f32,
    y: f32,
) -> Option<Pick> {
    let atom = targets.read_id(device, queue, x, y)?;
    Some(Pick {
        pose: scene.pose_of_atom(atom)?,
        atom,
    })
}

/// An RGBA8 image, rows top to bottom, no padding. Bytes are sRGB-encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// A colour texture plus [`FrameTargets`]: for tests, benchmarks and the CLI.
#[derive(Debug)]
pub struct OffscreenTarget {
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    pub frame: FrameTargets,
}

impl OffscreenTarget {
    /// For a renderer created with [`OFFSCREEN_FORMAT`].
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let frame = FrameTargets::new(device, width, height);
        let color = texture(
            device,
            "offscreen colour",
            (frame.width, frame.height),
            OFFSCREEN_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        Self {
            color_view: color.create_view(&Default::default()),
            color,
            frame,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }

    /// Records and submits one frame. Does not wait for it.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &Renderer,
        scene: &Scene,
        params: &FrameParams,
    ) -> wgpu::SubmissionIndex {
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(
            queue,
            &mut encoder,
            scene,
            params,
            &self.color_view,
            &self.frame,
        );
        queue.submit([encoder.finish()])
    }

    /// Copies the colour target back (waits for the GPU).
    pub fn read_rgba(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Image {
        let (width, height) = self.size();
        let row = width * 4;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("colour readback"),
            size: u64::from(padded * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let bytes = map_and_read(device, &buffer, u64::from(padded * height));
        let mut rgba = Vec::with_capacity((row * height) as usize);
        for y in 0..height {
            let start = (y * padded) as usize;
            rgba.extend_from_slice(&bytes[start..start + row as usize]);
        }
        Image {
            width,
            height,
            rgba,
        }
    }
}
