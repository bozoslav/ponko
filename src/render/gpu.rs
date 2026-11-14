//! wgpu setup, textures, and the instanced sprite pass.

use crate::render::traffic::{self, Counted, MAX_WRITES, NewBuffer, Upload};
use std::sync::Arc;

use glam::Mat4;
use winit::window::Window;

use super::batch::{self, DrawList, PanelDraw, SpriteInstance};
use super::timer::GpuTimer;
use super::{FrameStatus, RenderStats, Screenshot};
use crate::color::Color;

pub(super) struct GpuTexture {
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) texture: wgpu::Texture,
    /// Sampled with hard edges (pixel art).
    pub(super) nearest: bool,
    /// Whether it has mipmaps (made once a 3D material uses it), and
    /// whether they average data (normals, ORM) rather than colors.
    #[cfg(feature = "3d")]
    pub(super) mips: Option<bool>,
}

pub(crate) struct Renderer {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    lifecycle: super::surface_lifecycle::SurfaceLifecycle,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The format we draw in: the surface's, or its sRGB view where the
    /// surface itself can't be sRGB (browser canvases).
    view_format: wgpu::TextureFormat,
    /// The extra view formats image textures declare: their linear
    /// (non-sRGB) twin, or none where the device can't reinterpret texture
    /// formats (WebGL2), so creating them doesn't fail validation there.
    image_view_formats: &'static [wgpu::TextureFormat],

    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    /// What `camera_buffer` and `ui_camera_buffer` hold: a camera that
    /// didn't move isn't uploaded again.
    camera_mirrors: [traffic::Mirror; 2],
    camera_bind_group: wgpu::BindGroup,
    /// Window-pixel projection for screen-space drawing (UI, text).
    ui_camera_buffer: wgpu::Buffer,
    ui_camera_bind_group: wgpu::BindGroup,
    /// For world-space UI panels: the sprite shader, its layout and the
    /// cameras' layout, a pipeline drawing into render targets (made on
    /// first use), and one camera per panel.
    sprite_shader: wgpu::ShaderModule,
    sprite_layout: wgpu::PipelineLayout,
    camera_layout: wgpu::BindGroupLayout,
    panel_pipeline: Option<wgpu::RenderPipeline>,
    panel_cameras: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// Blend modes and sprite materials: their pipelines and bind groups.
    sprite_pipelines: super::sprite_pipelines::SpritePipelines,
    /// 2D lighting's passes, made the first frame lighting is on.
    lights2d: Option<super::lights2d::Lights2D>,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    textures: Vec<GpuTexture>,
    /// CPU-uploaded alpha only; unknown GPU-written textures stay conservative.
    #[cfg(feature = "3d")]
    texture_alpha: std::collections::HashMap<u32, TextureAlpha>,
    /// A 1x1 white texture freed textures' slots show (made on first use).
    blank: Option<wgpu::Texture>,
    /// Ids of freed textures (their slots show `blank`).
    freed_textures: std::collections::HashSet<u32>,

    instance_buffer: wgpu::Buffer,
    /// Capacity of `instance_buffer`, in sprites.
    instance_capacity: usize,
    /// The last frame had more sprites than the device's largest buffer
    /// holds (reported once): none were drawn.
    too_many_sprites: bool,
    /// What `instance_buffer` holds: sprites that didn't change aren't
    /// uploaded again. Forgotten with the buffer it mirrors.
    sprite_mirror: traffic::Mirror,

    timer: Option<GpuTimer>,
    /// What this GPU can do and the fallbacks used (fixed once opened).
    capabilities: super::GpuCapabilities,
    /// Draw frames the window can't take into `hidden_target` instead of
    /// skipping them (benchmarks).
    draw_hidden: bool,
    /// Draw every frame into `hidden_target`, never to the window: the
    /// display's refresh rate then neither caps nor stalls a benchmark.
    only_hidden: bool,
    hidden_target: Option<wgpu::Texture>,
    /// Offscreen frames still on the GPU, oldest first: no more than a
    /// window's swapchain would allow, so the CPU can't run ahead.
    hidden_in_flight: std::collections::VecDeque<wgpu::SubmissionIndex>,
    stats: RenderStats,
    /// The 3D pass, made when the game first creates a mesh or material.
    #[cfg(feature = "3d")]
    three: Option<super::gpu3d::Renderer3D>,
    /// Samples per pixel of the 3D frame.
    #[cfg(feature = "3d")]
    msaa: u32,
    /// The count the game asked for last (`Post::msaa`), taken or not.
    #[cfg(feature = "3d")]
    msaa_requested: u32,
    #[cfg(feature = "3d")]
    supports_msaa2: bool,
    /// The GPU draws in tiles (see `Renderer3D::solid_prepass`).
    #[cfg(feature = "3d")]
    tiled: bool,
    #[cfg(feature = "3d")]
    mips: Option<super::mips::MipMaker>,
    /// Time spent collecting and uploading this frame's 3D models.
    #[cfg(feature = "3d")]
    prepare_3d_ms: f32,
    /// Of `prepare_3d_ms`, the time collecting models.
    #[cfg(feature = "3d")]
    collect_ms: f32,
    /// The last drawn frame's `prepare_3d_ms`, for `detail`.
    #[cfg(feature = "3d")]
    last_prepare_3d_ms: f32,
    /// How long the last frame waited for the window's next image.
    display_wait_ms: f32,
    /// The last captured frame, waiting to be picked up.
    screenshot: Option<Screenshot>,

    /// The inspector's drawing for the next frame, and egui's renderer
    /// (made the first time the inspector opens).
    #[cfg(feature = "inspector")]
    overlay: Option<crate::inspector::Overlay>,
    #[cfg(feature = "inspector")]
    egui: Option<egui_wgpu::Renderer>,
    /// egui textures to free once the frame that last used them is submitted.
    #[cfg(feature = "inspector")]
    egui_free: Vec<egui::TextureId>,
}

/// The sprite pipeline, drawing into `format`.
pub(super) fn sprite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    sprite_pipeline_with(
        device,
        layout,
        shader,
        format,
        wgpu::BlendState::ALPHA_BLENDING,
        "fs_main",
    )
}

/// A sprite pipeline drawing into `format` with fragment entry point
/// `fragment` and `blend` (blend modes and sprite materials).
pub(super) fn sprite_pipeline_with(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    blend: wgpu::BlendState,
    fragment: &str,
) -> wgpu::RenderPipeline {
    let instance_attributes = wgpu::vertex_attr_array![
        0 => Float32x2, // pos
        1 => Float32x2, // size
        2 => Float16x2, // pivot
        3 => Float32,   // rotation
        4 => Unorm8x4,  // color
        5 => Unorm16x4, // uv: u0, v0, width, height
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sprite pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<SpriteInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attributes,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// What one sprite pass draws with.
#[derive(Clone, Copy)]
pub(super) struct SpritePass<'a> {
    pub(super) viewport: [u32; 2],
    pub(super) scale: f32,
    pub(super) pipeline: &'a wgpu::RenderPipeline,
    pub(super) instances: &'a wgpu::Buffer,
    pub(super) textures: &'a [GpuTexture],
    /// Camera, batches, and where the layer's instances start in `instances`.
    pub(super) layers: &'a [(&'a wgpu::BindGroup, &'a [batch::Batch], u32)],
}

/// What 2D lighting draws over the world's sprites this frame.
pub(super) struct Lighting<'a> {
    pub(super) lights: &'a super::lights2d::Lights2D,
    pub(super) frame: &'a crate::light2d::LightFrame2D,
    pub(super) normal_maps: &'a [Option<crate::texture::Texture>],
}

/// An offscreen copy of one frame on its way back to the CPU.
struct Capture {
    texture: wgpu::Texture,
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
    bgra: bool,
}

impl Capture {
    fn new(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let padded_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgra = matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        Self {
            texture,
            buffer,
            width,
            height,
            padded_row,
            bgra,
        }
    }

    fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Waits for the GPU and returns the pixels.
    fn read(self, device: &wgpu::Device) -> Screenshot {
        self.buffer.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mut rgba = Vec::with_capacity((self.width * self.height * 4) as usize);
        if let Ok(view) = self.buffer.get_mapped_range(..) {
            for row in view.chunks_exact(self.padded_row as usize) {
                rgba.extend_from_slice(&row[..(self.width * 4) as usize]);
            }
        }
        if self.bgra {
            for px in rgba.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
        }
        Screenshot {
            width: self.width,
            height: self.height,
            rgba,
        }
    }
}

/// Two changed sprites with up to this many unchanged ones between go in
/// one upload.
const SPRITE_BRIDGE: usize = 15;
const INITIAL_INSTANCE_CAPACITY: usize = 1024;

impl Renderer {
    /// What drawing the last frame cost.
    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// The pipeline cache's counters; all zero before any 3D.
    pub fn pipeline_stats(&self) -> super::PipelineStats {
        #[cfg(feature = "3d")]
        if let Some(three) = &self.three {
            return three.pipeline_stats();
        }
        super::PipelineStats::default()
    }

    /// See `Renderer3D::mesh_costs`; empty before any 3D.
    #[cfg(feature = "3d")]
    pub fn mesh_costs(&self) -> Vec<(u32, usize, usize, usize, usize)> {
        self.three.as_ref().map(|t| t.mesh_costs()).unwrap_or_default()
    }

    /// Draw frames that the window can't take offscreen instead of skipping
    /// them, so benchmarks measure every frame; with `always`, draw every
    /// frame offscreen.
    pub fn set_draw_hidden(&mut self, on: bool, always: bool) {
        self.draw_hidden = on || always;
        self.only_hidden = always;
    }

    /// A view of the offscreen stand-in for the window, made at its size.
    fn hidden_view(&mut self) -> wgpu::TextureView {
        let (width, height) = (self.config.width, self.config.height);
        let fits = self
            .hidden_target
            .as_ref()
            .is_some_and(|t| t.width() == width && t.height() == height);
        if !fits {
            self.hidden_target = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.view_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            }));
        }
        self.hidden_target
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .expect("made above")
    }

    /// The GPU time of each pass, smoothed (empty without timestamp queries).
    pub fn gpu_passes(&self) -> &[crate::profiler::GpuPass] {
        self.timer.as_ref().map_or(&[], |t| &t.passes)
    }

    /// What the render-pass inspector needs beyond [`RenderStats`].
    pub fn detail(&self) -> super::RenderDetail {
        super::RenderDetail {
            #[cfg(feature = "3d")]
            state_changes: self.three.as_ref().map(|r| r.state_changes()),
            #[cfg(not(feature = "3d"))]
            state_changes: None,
            gpu_timestamps: self.timer.is_some(),
            gpu_frames_timed: self.timer.as_ref().map_or(0, |t| t.frames_read),
            #[cfg(feature = "3d")]
            prepare_ms: self.last_prepare_3d_ms,
            #[cfg(not(feature = "3d"))]
            prepare_ms: 0.0,
            gpu_latest: self.timer.as_ref().and_then(GpuTimer::latest_frame),
        }
    }

    /// What this GPU can do, and what the renderer does where it can't.
    pub fn capabilities(&self) -> &super::GpuCapabilities {
        &self.capabilities
    }

    /// Read the last collected GPU quality sample before scene preparation.
    #[cfg(feature = "3d")]
    pub(crate) fn quality_sample(&self) -> Option<crate::three::autotune::Sample> {
        self.timer.as_ref()?.quality_sample()
    }

    /// The latest GPU frame's passes and how many frames the timer has read.
    pub fn gpu_frame(&self) -> Option<(&[crate::profiler::PassSpan], u64)> {
        self.timer.as_ref().map(|t| (&t.last_frame[..], t.frames_read))
    }

    /// The last captured frame, if one is waiting.
    pub fn take_screenshot(&mut self) -> Option<Screenshot> {
        self.screenshot.take()
    }

    /// Opens the GPU and sets up drawing to `window`, waiting for the GPU
    /// (desktop; the web build calls [`Self::new_async`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(window: Arc<Window>, display: winit::event_loop::OwnedDisplayHandle, vsync: bool, msaa: u32) -> Self {
        pollster::block_on(Self::new_async(window, display, vsync, msaa))
    }

    pub async fn new_async(
        window: Arc<Window>,
        display: winit::event_loop::OwnedDisplayHandle,
        vsync: bool,
        msaa: u32,
    ) -> Self {
        let descriptor = wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display));
        // In a browser, use WebGPU where it works and WebGL2 elsewhere.
        #[cfg(target_arch = "wasm32")]
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        #[cfg(not(target_arch = "wasm32"))]
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance
            .create_surface(window.clone())
            .unwrap_or_else(|e| panic!("ponko: could not create a GPU surface: {e}"));

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("ponko: no compatible GPU found: {e}"));

        // `PONKO_GPU_TIER` opens the device as a weaker GPU would be.
        let forced_tier = super::GpuTier::forced();
        if let Some(tier) = forced_tier {
            eprintln!("ponko: PONKO_GPU_TIER forces the {tier} GPU tier");
        }
        #[cfg_attr(not(feature = "compressed-textures"), allow(unused_mut))]
        let mut required_features = adapter.features()
            & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);
        // Block-compressed textures upload as they are where the GPU samples them.
        #[cfg(feature = "compressed-textures")]
        {
            required_features |= adapter.features() & super::blocks::FEATURES;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ponko device"),
                required_features: forced_tier.map_or(required_features, |t| t.device_features(required_features)),
                // The defaults ask for more than WebGL2 has; we need little.
                required_limits: if let Some(tier) = forced_tier {
                    // The tier's limits, but a surface as large as the window.
                    tier.window_limits(wgpu::Limits {
                        max_sampled_textures_per_shader_stage: adapter
                            .limits()
                            .max_sampled_textures_per_shader_stage
                            .clamp(16, 24),
                        ..wgpu::Limits::default()
                    })
                } else if cfg!(target_arch = "wasm32") {
                    adapter.limits()
                } else {
                    // One more sampled texture than the default 16, where
                    // the adapter has it: projected decals' images.
                    wgpu::Limits {
                        max_sampled_textures_per_shader_stage: adapter
                            .limits()
                            .max_sampled_textures_per_shader_stage
                            .clamp(16, 24),
                        ..wgpu::Limits::default()
                    }
                },
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("ponko: could not open the GPU device: {e}"));

        // The 3D frame's samples per pixel: the game's choice where this GPU
        // can multisample both 3D targets that much, else 4 (always works).
        let requested_msaa = msaa;
        #[cfg(feature = "3d")]
        let msaa = if super::gpu3d::browser_gl_has_no_multisampled_textures(adapter.get_info().backend) {
            // WebGL2 can't make multisampled textures through wgpu (glow's
            // browser binding panics): draw the 3D frame unsmoothed, and
            // leave edges to FXAA or SMAA.
            1
        } else if msaa == 4
            || (device
                .features()
                .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
                && super::gpu3d::multisamples(&adapter, msaa))
        {
            msaa
        } else {
            eprintln!("ponko: this GPU can't do {msaa}x multisampling here; using 4x");
            4
        };
        let mut capabilities = super::capabilities::of_device(&adapter.get_info(), &device, (requested_msaa, msaa));
        capabilities.forced_tier = forced_tier;
        // Apple's GPUs (Metal) draw in tiles and drop hidden solid fragments
        // themselves: there solid surfaces skip the depth prepass.
        #[cfg(feature = "3d")]
        let tiled = adapter.get_info().backend == wgpu::Backend::Metal;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("ponko: the GPU can't draw to this window");
        // Prefer an sRGB target so blending happens in linear light.
        let caps = surface.get_capabilities(&adapter);
        if let Some(srgb) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = srgb;
        }
        // Browser canvases are never sRGB themselves; draw through an sRGB view.
        let view_format = config.format.add_srgb_suffix();
        if view_format != config.format {
            config.view_formats.push(view_format);
        }
        config.present_mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);

        // --- pipeline ---
        let shader = device.create_shader_module(wgpu::include_wgsl!("sprite.wgsl"));

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let pipeline = sprite_pipeline(&device, &layout, &shader, view_format);
        let sprite_pipelines = super::sprite_pipelines::SpritePipelines::new(
            &device,
            &queue,
            &shader,
            &layout,
            &camera_layout,
            &texture_layout,
        );

        let camera = |label| {
            let buffer = device.new_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size_of::<Mat4>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &camera_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            (buffer, bind_group)
        };
        let (camera_buffer, camera_bind_group) = camera("camera");
        let (ui_camera_buffer, ui_camera_bind_group) = camera("ui camera");

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pixel art sampler"),
            ..Default::default() // nearest
        });
        let instance_buffer = Self::create_instance_buffer(&device, INITIAL_INSTANCE_CAPACITY);
        let timer = GpuTimer::new(&device, &queue);
        let image_view_formats: &'static [wgpu::TextureFormat] = if adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::VIEW_FORMATS)
        {
            &[wgpu::TextureFormat::Rgba8Unorm]
        } else {
            &[]
        };

        Self {
            window,
            instance,
            surface,
            lifecycle: Default::default(),
            device,
            queue,
            config,
            view_format,
            image_view_formats,
            pipeline,
            camera_buffer,
            camera_mirrors: Default::default(),
            camera_bind_group,
            ui_camera_buffer,
            ui_camera_bind_group,
            sprite_shader: shader,
            sprite_layout: layout,
            camera_layout,
            panel_pipeline: None,
            panel_cameras: Vec::new(),
            sprite_pipelines,
            lights2d: None,
            texture_layout,
            sampler,
            nearest_sampler,
            textures: Vec::new(),
            #[cfg(feature = "3d")]
            texture_alpha: Default::default(),
            blank: None,
            freed_textures: Default::default(),
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            too_many_sprites: false,
            sprite_mirror: traffic::Mirror::default(),
            timer,
            capabilities,
            draw_hidden: false,
            only_hidden: false,
            hidden_target: None,
            hidden_in_flight: std::collections::VecDeque::with_capacity(4),
            stats: RenderStats::default(),
            #[cfg(feature = "3d")]
            three: None,
            #[cfg(feature = "3d")]
            msaa,
            #[cfg(feature = "3d")]
            msaa_requested: requested_msaa,
            #[cfg(feature = "3d")]
            supports_msaa2: msaa != 1
                && required_features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
                && super::gpu3d::multisamples(&adapter, 2),
            #[cfg(feature = "3d")]
            tiled,
            #[cfg(feature = "3d")]
            mips: None,
            display_wait_ms: 0.0,
            #[cfg(feature = "3d")]
            prepare_3d_ms: 0.0,
            #[cfg(feature = "3d")]
            collect_ms: 0.0,
            #[cfg(feature = "3d")]
            last_prepare_3d_ms: 0.0,
            screenshot: None,
            #[cfg(feature = "inspector")]
            overlay: None,
            #[cfg(feature = "inspector")]
            egui: None,
            #[cfg(feature = "inspector")]
            egui_free: Vec::new(),
        }
    }

    fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite instances"),
            size: (capacity * size_of::<SpriteInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Compiles sprite shader `id` (new or hot reloaded) from its checked,
    /// composed WGSL. On an error the old version stays.
    pub fn set_sprite_shader(
        &mut self,
        id: u32,
        name: &str,
        composed: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        self.sprite_pipelines
            .set_shader(&self.device, id, name, composed, self.view_format)
    }

    /// Sprite shader `id` was freed.
    pub fn free_sprite_shader(&mut self, id: u32) {
        self.sprite_pipelines.free_shader(id);
    }

    /// Makes or changes sprite material `id`.
    pub fn set_sprite_material(
        &mut self,
        id: u32,
        shader: Option<u32>,
        params: &[glam::Vec4; 4],
        textures: [Option<u32>; 2],
    ) {
        self.sprite_pipelines
            .set_material(&self.device, &self.queue, id, shader, params, textures);
    }

    /// Sprite material `id` was freed.
    pub fn free_sprite_material(&mut self, id: u32) {
        self.sprite_pipelines.free_material(id);
    }

    /// Follows `Post::msaa`: switches the 3D frame and target cameras to
    /// `requested` samples when it is a new count this GPU can take, keeping
    /// meshes, materials and shaders (temporal history restarts). Before the
    /// 3D pass is made this only records the count. A count it can't take
    /// (other than 2 or 4, 2 where it has only 4, any on WebGL2) keeps the
    /// current one, says so once on stderr, and shows as the Multisampling
    /// fallback in the capabilities. While an asynchronous shader check is
    /// pending it waits, and the next call tries again.
    #[cfg(feature = "3d")]
    pub(crate) fn follow_msaa(&mut self, requested: u32) {
        if requested == self.msaa_requested {
            return;
        }
        if msaa_switch(self.msaa, requested, self.supports_msaa2) {
            if let Some(three) = &mut self.three
                && !three.set_samples(&self.device, requested)
            {
                return;
            }
            self.msaa = requested;
        } else if let Some(why) = msaa_refusal(self.msaa, requested, self.supports_msaa2) {
            eprintln!("ponko: {why}");
        }
        self.msaa_requested = requested;
        super::capabilities::set_multisampling(&mut self.capabilities, requested, self.msaa);
    }

    /// The 3D pass, made on first use (with its white texture).
    #[cfg(feature = "3d")]
    fn three(&mut self) -> &mut super::gpu3d::Renderer3D {
        if self.three.is_none() {
            let white = self.create_texture(1, 1, &[255; 4]);
            let white_array = self.upload_texture_array(1, 1, &[&[255; 4]], false);
            let renderer = super::gpu3d::Renderer3D::new(
                &self.device,
                &self.texture_layout,
                self.view_format,
                white,
                &white_array,
                self.msaa,
                !self.tiled,
            );
            // No view formats (WebGL2): sprite textures can't be read through a linear view.
            self.three = Some(if self.image_view_formats.is_empty() {
                renderer.without_linear_views()
            } else {
                renderer
            });
        }
        self.three.as_mut().expect("just made")
    }

    #[cfg(feature = "3d")]
    pub fn create_material(&mut self, desc: &crate::three::GpuMaterialDesc) -> u32 {
        self.material_mips(desc);
        let device = self.device.clone();
        self.three();
        let three = self.three.as_mut().expect("just made");
        three.create_material(&device, desc, &self.textures)
    }

    #[cfg(feature = "3d")]
    pub fn update_material(
        &mut self,
        id: u32,
        desc: &crate::three::GpuMaterialDesc,
        batches: &[Option<crate::three::instances::Batch>],
    ) {
        self.material_mips(desc);
        if let Some(three) = &mut self.three
            && three.update_material(&self.device, &self.queue, id, desc, &self.textures)
        {
            three.refresh_batch_material(&self.device, &self.queue, id, batches);
        }
    }

    /// Builds the 3D pipeline variants frames would build when first drawn
    /// (see `Renderer3D::warm_pipelines`).
    #[cfg(feature = "3d")]
    pub fn warm_pipelines(&mut self) {
        self.three().warm_pipelines();
    }

    /// Whether projected decals can be drawn (see `gpu3d::supports_decals`).
    #[cfg(feature = "3d")]
    pub fn supports_decals(&self) -> bool {
        super::gpu3d::supports_decals(&self.device)
    }

    /// Whether global illumination can be drawn (see `gpu3d::supports_gi`).
    #[cfg(feature = "3d")]
    pub fn supports_gi(&self) -> bool {
        super::gpu3d::supports_gi(&self.device)
    }

    /// The largest texture array this GPU takes: width and height, and layers.
    #[cfg(feature = "3d")]
    pub fn max_texture_array_size(&self) -> (u32, u32) {
        let limits = self.device.limits();
        (limits.max_texture_dimension_2d, limits.max_texture_array_layers)
    }

    /// Uploads a texture array (RGBA8, one image per layer, sRGB unless
    /// `linear`) with mipmaps; returns its id among the 3D arrays.
    #[cfg(feature = "3d")]
    pub fn create_texture_array(&mut self, width: u32, height: u32, layers: &[&[u8]], linear: bool) -> u32 {
        let texture = self.upload_texture_array(width, height, layers, linear);
        self.three().add_texture_array(&texture)
    }

    #[cfg(feature = "3d")]
    fn upload_texture_array(&mut self, width: u32, height: u32, layers: &[&[u8]], linear: bool) -> wgpu::Texture {
        let layers = &*stored_layers(layers, self.device.adapter_info().backend == wgpu::Backend::Gl);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers.len() as u32,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("3d texture array"),
            size,
            mip_level_count: super::mips::levels(width, height),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if linear {
                wgpu::TextureFormat::Rgba8Unorm
            } else {
                wgpu::TextureFormat::Rgba8UnormSrgb
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        for (i, rgba) in layers.iter().enumerate() {
            self.queue.uploading(Upload::Textures).write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    depth_or_array_layers: 1,
                    ..size
                },
            );
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mips = self
            .mips
            .get_or_insert_with(|| super::mips::MipMaker::new(&self.device));
        mips.generate(&self.device, &mut encoder, &texture, linear);
        self.queue.submit([encoder.finish()]);
        texture
    }

    /// Mipmaps for the textures a material reads.
    #[cfg(feature = "3d")]
    fn material_mips(&mut self, desc: &crate::three::GpuMaterialDesc) {
        // Normal and ORM maps (the 2nd and 3rd) hold data, not colors.
        let linear = [false, true, true, false, false, false];
        for (map, linear) in desc.textures.iter().zip(linear) {
            if let Some(id) = *map {
                self.ensure_mips(id, linear);
            }
        }
    }

    #[cfg(feature = "3d")]
    pub fn create_gpu_buffer(&mut self, id: u32, len: u32, data: &[glam::Vec4]) {
        let device = self.device.clone();
        self.three().create_gpu_buffer(&device, id, len, data);
    }
    #[cfg(feature = "3d")]
    pub fn free_gpu_buffer(&mut self, id: u32) {
        self.three().free_gpu_buffer(id);
    }
    #[cfg(feature = "3d")]
    pub fn write_gpu_buffer(&mut self, id: u32, offset: u32, data: &[glam::Vec4]) {
        let queue = self.queue.clone();
        self.three().write_gpu_buffer(&queue, id, offset, data);
    }
    #[cfg(feature = "3d")]
    pub fn set_instance_batch(&mut self, id: u32, batch: Option<&crate::three::instances::Batch>) {
        let (device, queue) = (self.device.clone(), self.queue.clone());
        self.three().set_instance_batch(&device, &queue, id, batch);
    }
    #[cfg(feature = "3d")]
    pub fn set_mesh_gpu_buffer(&mut self, mesh: u32, b: u32, count: u32) {
        self.three().set_mesh_gpu_buffer(mesh, b, count);
    }
    /// Compiles a compute pass; its id, or the compiler's message.
    #[cfg(feature = "3d")]
    pub fn create_compute(&mut self, name: &str, source: &str) -> Result<u32, Box<crate::render::ShaderDiagnostic>> {
        let device = self.device.clone();
        self.three().create_compute(&device, name, source)
    }

    /// Recompiles compute pass `id` in place (hot reload).
    #[cfg(feature = "3d")]
    pub fn replace_compute(
        &mut self,
        id: u32,
        name: &str,
        source: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        let device = self.device.clone();
        self.three().replace_compute(&device, id, name, source)
    }

    /// Compiles a screen effect; its id, or the compiler's message.
    #[cfg(feature = "3d")]
    pub fn create_effect(&mut self, name: &str, source: &str) -> Result<u32, Box<crate::render::ShaderDiagnostic>> {
        let device = self.device.clone();
        self.three().create_effect(&device, name, source)
    }

    /// Recompiles screen effect `id` (hot reload).
    #[cfg(feature = "3d")]
    pub fn replace_effect(
        &mut self,
        id: u32,
        name: &str,
        source: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        let device = self.device.clone();
        self.three().replace_effect(&device, id, name, source)
    }

    /// Shaders whose pipelines the GPU rejected after they were put in use
    /// (a browser's WebGPU answers late): the previous ones are back.
    #[cfg(feature = "3d")]
    pub fn poll_shader_checks(&mut self, out: &mut Vec<super::gpu3d::LateShaderError>) {
        let device = self.device.clone();
        if let Some(three) = self.three.as_mut() {
            three.poll_shader_checks(&device, out);
        }
    }

    /// Compiles a material shader; its index, or the compiler's message.
    #[cfg(feature = "3d")]
    pub fn create_shader(&mut self, name: &str, source: &str) -> Result<u32, Box<crate::render::ShaderDiagnostic>> {
        let device = self.device.clone();
        self.three().create_shader(&device, name, source)
    }

    #[cfg(feature = "3d")]
    pub fn replace_shader(
        &mut self,
        id: u32,
        name: &str,
        source: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        let device = self.device.clone();
        self.three().replace_shader(&device, id, name, source)
    }

    #[cfg(feature = "3d")]
    /// Makes 3D mesh `id`: a freed mesh's id, or the next new one.
    #[cfg(feature = "3d")]
    pub fn create_mesh(
        &mut self,
        id: u32,
        data: &crate::render::MeshData,
        parts: &[(std::ops::Range<u32>, crate::render::Material)],
        skinned: bool,
    ) {
        let device = self.device.clone();
        self.three().create_mesh_at(&device, id, data, parts, skinned);
    }

    /// Frees 3D mesh `id`'s GPU memory.
    #[cfg(feature = "3d")]
    pub fn free_mesh(&mut self, id: u32) {
        self.three().free_mesh(id);
    }

    /// Uploads the frame's 3D models, camera and light for the next `render`.
    /// `collect_ms`: how long collecting the models took, counted in `cpu_ms`.
    /// The newest depth grid for occlusion culling, if one came back.
    #[cfg(feature = "3d")]
    pub fn take_occlusion(&mut self) -> Option<crate::three::occlusion::OcclusionGrid> {
        self.three.as_mut()?.take_occlusion()
    }

    #[cfg(feature = "3d")]
    pub fn prepare_3d(&mut self, scene: &crate::three::Scene3D<'_>, collect_ms: f32) {
        let start = web_time::Instant::now();
        let size = (self.config.width, self.config.height);
        // A lookup table must be a strip of n squares of n x n.
        let lut = scene.post.lut.filter(|t| {
            let n = t.height();
            (2..=64).contains(&n)
                && t.width() == n * n
                && (t.id as usize) < self.textures.len()
                && !self.freed_textures.contains(&t.id)
        });
        if let Some(three) = &mut self.three {
            let lut = lut.map(|t| (t.id, t.height()));
            let input = super::gpu3d::Prepare3D { scene, lut, size };
            three.prepare(&self.device, &self.queue, &input);
        }
        self.prepare_3d_ms = collect_ms + start.elapsed().as_secs_f32() * 1000.0;
        self.collect_ms = collect_ms;
    }

    pub fn display_wait_ms(&self) -> f32 {
        self.display_wait_ms
    }

    /// What this renderer holds on the GPU, counted from its own tables
    /// (`Ctx::asset_inventory`).
    pub fn residency(&self) -> Option<crate::inventory::GpuResidency> {
        #[cfg_attr(not(feature = "3d"), allow(unused_mut))]
        let mut r = crate::inventory::GpuResidency {
            textures: self.textures.len(),
            texture_bytes: self.textures.iter().map(|t| texture_bytes(&t.texture)).sum(),
            ..Default::default()
        };
        #[cfg(feature = "3d")]
        if let Some(three) = &self.three {
            (r.meshes, r.mesh_bytes, r.gpu_buffers, r.gpu_buffer_bytes) = three.residency();
        }
        Some(r)
    }

    pub fn max_texture_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// A texture target cameras draw into (linear HDR); returns its id.
    #[cfg(feature = "3d")]
    pub fn create_render_target(&mut self, width: u32, height: u32) -> u32 {
        // Made first: making the 3D renderer adds textures of its own, and
        // the id below must be this one's.
        self.three();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: super::gpu3d::HDR_FORMAT,
            // Drawn into by target cameras, copied into by compute passes.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let bind_group = self.sprite_bind_group(&texture, false);
        let id = self.textures.len() as u32;
        self.three().add_render_target(id, &texture);
        self.textures.push(GpuTexture {
            bind_group,
            texture,
            nearest: false,
            // Drawn fresh each frame: no mipmaps to make.
            mips: Some(false),
        });
        id
    }

    /// Uploads RGBA8 (sRGB) pixels and returns the texture's id.
    pub fn create_texture(&mut self, width: u32, height: u32, rgba: &[u8]) -> u32 {
        let texture = self.upload_texture(width, height, rgba);
        let bind_group = self.sprite_bind_group(&texture, false);
        self.textures.push(GpuTexture {
            bind_group,
            texture,
            nearest: false,
            #[cfg(feature = "3d")]
            mips: None,
        });
        let id = (self.textures.len() - 1) as u32;
        #[cfg(feature = "3d")]
        self.texture_alpha.insert(id, TextureAlpha::new(width, height, rgba));
        id
    }

    /// The block-compressed formats this device samples.
    #[cfg(feature = "compressed-textures")]
    pub fn compressed_formats(&self) -> crate::compressed::GpuFormats {
        super::blocks::gpu_formats(self.device.features())
    }

    /// Uploads a block-compressed texture with its mipmaps and returns its
    /// id. It keeps its own mipmaps: none are made for 3D materials.
    #[cfg(feature = "compressed-textures")]
    pub fn create_compressed_texture(&mut self, image: &crate::compressed::CompressedImage) -> u32 {
        let texture = super::blocks::upload(&self.device, &self.queue, "compressed texture", &[image]);
        let bind_group = self.sprite_bind_group(&texture, false);
        self.textures.push(GpuTexture {
            bind_group,
            texture,
            nearest: false,
            #[cfg(feature = "3d")]
            mips: Some(!image.srgb),
        });
        (self.textures.len() - 1) as u32
    }

    /// Replaces texture `id` with a block-compressed image (resolution
    /// streaming, hot reload), like [`replace_texture`](Self::replace_texture).
    #[cfg(feature = "compressed-textures")]
    pub fn replace_compressed_texture(&mut self, id: u32, image: &crate::compressed::CompressedImage) {
        let Some(old) = self.textures.get(id as usize) else {
            return;
        };
        let nearest = old.nearest;
        let texture = super::blocks::upload(&self.device, &self.queue, "compressed texture", &[image]);
        let bind_group = self.sprite_bind_group(&texture, nearest);
        self.textures[id as usize] = GpuTexture {
            bind_group,
            texture,
            nearest,
            #[cfg(feature = "3d")]
            mips: Some(!image.srgb),
        };
        #[cfg(feature = "3d")]
        {
            // No CPU copy of its alpha to compare: count it as changed.
            self.texture_alpha.remove(&id);
            if let Some(three) = &mut self.three {
                three.texture_alpha_changed(id, true);
                three.texture_replaced(&self.device, &self.textures, id);
            }
        }
    }

    /// Uploads a texture array of block-compressed layers (one size and
    /// format) with their mipmaps; returns its id among the 3D arrays.
    #[cfg(all(feature = "3d", feature = "compressed-textures"))]
    pub fn create_compressed_texture_array(&mut self, layers: &[&crate::compressed::CompressedImage]) -> u32 {
        let texture = super::blocks::upload(&self.device, &self.queue, "3d compressed texture array", layers);
        self.three().add_texture_array(&texture)
    }

    /// Replaces texture `id` with `rgba` at a new size (resolution
    /// streaming): the same id, sampling and mipmap kind, so sprites and
    /// materials showing it see the new pixels at once.
    pub fn replace_texture(&mut self, id: u32, width: u32, height: u32, rgba: &[u8]) {
        let Some(old) = self.textures.get(id as usize) else {
            return;
        };
        let nearest = old.nearest;
        #[cfg(feature = "3d")]
        let mips = old.mips;
        let texture = self.upload_texture(width, height, rgba);
        let bind_group = self.sprite_bind_group(&texture, nearest);
        self.textures[id as usize] = GpuTexture {
            bind_group,
            texture,
            nearest,
            #[cfg(feature = "3d")]
            mips: None,
        };
        #[cfg(feature = "3d")]
        {
            if let Some(linear) = mips {
                self.ensure_mips(id, linear);
            }
            let next = TextureAlpha::new(width, height, rgba);
            let alpha_changed = self.texture_alpha.get(&id) != Some(&next);
            self.texture_alpha.insert(id, next);
            if let Some(three) = &mut self.three {
                three.texture_alpha_changed(id, alpha_changed);
                three.texture_replaced(&self.device, &self.textures, id);
            }
        }
    }

    /// Frees texture `id`: its slot shows a 1x1 white texture from now on,
    /// so sprites and materials still showing it draw white, and target
    /// cameras and compute passes let go of it if it was a render target.
    pub fn free_texture(&mut self, id: u32) {
        if id as usize >= self.textures.len() {
            return;
        }
        let blank = match &self.blank {
            Some(blank) => blank.clone(),
            None => {
                let blank = self.upload_texture(1, 1, &[255; 4]);
                self.blank = Some(blank.clone());
                blank
            }
        };
        let bind_group = self.sprite_bind_group(&blank, false);
        self.textures[id as usize] = GpuTexture {
            bind_group,
            texture: blank,
            nearest: false,
            // 1x1: no mipmaps to make.
            #[cfg(feature = "3d")]
            mips: Some(false),
        };
        self.freed_textures.insert(id);
        #[cfg(feature = "3d")]
        self.texture_alpha.remove(&id);
        #[cfg(feature = "3d")]
        if let Some(three) = &mut self.three {
            three.free_render_target(id);
            three.rebind_materials(&self.device, &self.queue, &self.textures, Some(id), None);
        }
    }

    /// Material `id` was freed (see `Renderer3D::free_material`).
    #[cfg(feature = "3d")]
    pub fn free_material(&mut self, id: u32) {
        if let Some(three) = &mut self.three {
            three.free_material(&self.device, &self.queue, id, &self.textures);
        }
    }

    /// Material shader `id` was freed (see `Renderer3D::free_shader`).
    #[cfg(feature = "3d")]
    pub fn free_shader(&mut self, id: u32) {
        if let Some(three) = &mut self.three {
            three.free_shader(id);
        }
    }

    /// Compute pass `id` was freed (see `Renderer3D::free_compute`).
    #[cfg(feature = "3d")]
    pub fn free_compute(&mut self, id: u32) {
        if let Some(three) = &mut self.three {
            three.free_compute(id);
        }
    }

    /// Texture array `id` was freed (see `Renderer3D::free_texture_array`).
    #[cfg(feature = "3d")]
    pub fn free_texture_array(&mut self, id: u32) {
        if let Some(three) = &mut self.three {
            three.free_texture_array(&self.device, &self.queue, id, &self.textures);
        }
    }

    /// An RGBA8 (sRGB) texture of `rgba`, not yet in `textures`.
    fn upload_texture(&self, width: u32, height: u32, rgba: &[u8]) -> wgpu::Texture {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            // Copied from when a 3D material first uses it and it gets mipmaps.
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            // Materials read normal maps (data, not colors) without sRGB decoding.
            view_formats: self.image_view_formats,
        });
        self.queue.uploading(Upload::Textures).write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );
        texture
    }

    /// The bind group sprites draw a texture with: its full-size level only,
    /// so mipmaps made for 3D don't change how sprites look.
    fn sprite_bind_group(&self, texture: &wgpu::Texture, nearest: bool) -> wgpu::BindGroup {
        // Its own format: sRGB for images, HDR for render targets.
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            mip_level_count: Some(1),
            ..Default::default()
        });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(if nearest {
                        &self.nearest_sampler
                    } else {
                        &self.sampler
                    }),
                },
            ],
        })
    }

    /// Gives texture `id` mipmaps, so 3D materials far away or at a slant
    /// read it smoothly instead of shimmering. `linear`: it holds data
    /// (normals, ORM), averaged as stored rather than as light.
    #[cfg(feature = "3d")]
    fn ensure_mips(&mut self, id: u32, linear: bool) {
        let Some(tex) = self.textures.get(id as usize) else {
            return;
        };
        if tex.mips.is_some() {
            return;
        }
        let (width, height) = (tex.texture.width(), tex.texture.height());
        let levels = super::mips::levels(width, height);
        // Without view formats (WebGL2) a data texture can't be read through
        // a linear view of its sRGB texture: it is remade as a linear one.
        let remake_as_data = linear && self.image_view_formats.is_empty();
        if levels == 1 && !remake_as_data {
            self.textures[id as usize].mips = Some(linear);
            return;
        }
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mips = self
            .mips
            .get_or_insert_with(|| super::mips::MipMaker::new(&self.device));
        let texture = if remake_as_data {
            mips.data_texture(&self.device, &mut encoder, &self.textures[id as usize].texture, levels)
        } else {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("material texture"),
                size,
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: self.image_view_formats,
            });
            encoder.copy_texture_to_texture(
                self.textures[id as usize].texture.as_image_copy(),
                texture.as_image_copy(),
                size,
            );
            mips.generate(&self.device, &mut encoder, &texture, linear);
            texture
        };
        self.queue.submit([encoder.finish()]);
        let bind_group = self.sprite_bind_group(&texture, self.textures[id as usize].nearest);
        let tex = &mut self.textures[id as usize];
        tex.texture = texture;
        tex.bind_group = bind_group;
        tex.mips = Some(linear);
    }

    /// Remakes texture `id`'s mipmaps after its pixels changed.
    fn refresh_mips(&mut self, id: u32) {
        #[cfg(feature = "3d")]
        if let Some(linear) = self.textures[id as usize].mips
            && let Some(mips) = &self.mips
        {
            let mut encoder = self.device.create_command_encoder(&Default::default());
            mips.generate(&self.device, &mut encoder, &self.textures[id as usize].texture, linear);
            self.queue.submit([encoder.finish()]);
        }
        #[cfg(not(feature = "3d"))]
        let _ = id;
    }

    /// Replaces every pixel of texture `id` (same size as created).
    pub fn update_texture(&mut self, id: u32, width: u32, height: u32, rgba: &[u8]) {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        // A block-compressed texture can't take pixels: it becomes RGBA8.
        if self.textures[id as usize].texture.format().is_compressed() {
            self.replace_texture(id, width, height, rgba);
            return;
        }
        self.queue.uploading(Upload::Textures).write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.textures[id as usize].texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        #[cfg(feature = "3d")]
        self.shadow_texture_update(id, [0, 0], [width, height], rgba);
        self.refresh_mips(id);
    }

    #[cfg(feature = "3d")]
    fn shadow_texture_update(&mut self, id: u32, at: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        let changed = self
            .texture_alpha
            .get_mut(&id)
            .is_none_or(|alpha| alpha.update(at, size, rgba));
        if let Some(three) = &mut self.three {
            three.texture_alpha_changed(id, changed);
        }
    }

    /// Switches texture `id` between smooth and hard-edged sampling.
    pub fn set_filter(&mut self, id: u32, nearest: bool) {
        let bind_group = self.sprite_bind_group(&self.textures[id as usize].texture, nearest);
        let tex = &mut self.textures[id as usize];
        tex.bind_group = bind_group;
        tex.nearest = nearest;
    }

    /// Replaces a `width` × `height` block of texture `id` at `(x, y)`.
    pub fn update_texture_region(&mut self, id: u32, at: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        debug_assert_eq!(rgba.len(), (size[0] * size[1] * 4) as usize);
        self.queue.uploading(Upload::Textures).write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.textures[id as usize].texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: at[0],
                    y: at[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size[0]),
                rows_per_image: Some(size[1]),
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        #[cfg(feature = "3d")]
        self.shadow_texture_update(id, at, size, rgba);
        self.refresh_mips(id);
    }

    pub(crate) fn surface_event(&mut self, event: super::surface_lifecycle::SurfaceEvent) {
        if self.lifecycle.update(event) {
            #[cfg(feature = "3d")]
            if let Some(three) = &mut self.three {
                three.invalidate_targets();
            }
        }
    }

    /// Call when the window's physical size changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.surface_event(super::surface_lifecycle::SurfaceEvent::Occluded(
            width == 0 || height == 0,
        ));
        let Some((width, height)) = super::surface_lifecycle::SurfaceLifecycle::nonzero_size(width, height) else {
            return; // Keep the last nonzero dimensions for offscreen capture.
        };
        self.config.width = width;
        self.config.height = height;
        if !self.lifecycle.suspended() {
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Draws everything in `list` and presents. `list` is left as-is; the caller clears it.
    ///
    /// With `capture`, the frame is also read back into `self.screenshot`
    /// (this waits for the GPU, so only do it when asked).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        world: &mut DrawList,
        ui: &mut DrawList,
        panels: &mut [PanelDraw],
        clear: Color,
        world_view: Mat4,
        ui_view: Mat4,
        capture: bool,
        lights: &crate::light2d::Lighting2D,
    ) -> FrameStatus {
        // `None` when the window can't be drawn to right now; a requested
        // screenshot is still drawn (offscreen), so it never waits on the
        // window being visible.
        let acquire_start = web_time::Instant::now();
        let acquired = if self.only_hidden || self.lifecycle.suspended() {
            wgpu::CurrentSurfaceTexture::Occluded
        } else {
            self.surface.get_current_texture()
        };
        self.display_wait_ms = acquire_start.elapsed().as_secs_f32() * 1000.0;
        let (frame, status) = match acquired {
            wgpu::CurrentSurfaceTexture::Success(t) => {
                self.surface_event(super::surface_lifecycle::SurfaceEvent::Occluded(false));
                (Some(t), FrameStatus::Presented)
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                // Draw this one, reconfigure for the next.
                self.surface_event(super::surface_lifecycle::SurfaceEvent::Recreated);
                self.surface.configure(&self.device, &self.config);
                (Some(t), FrameStatus::Presented)
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.surface_event(super::surface_lifecycle::SurfaceEvent::Occluded(true));
                (None, FrameStatus::Occluded)
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Validation => {
                (None, FrameStatus::Skipped)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface_event(super::surface_lifecycle::SurfaceEvent::Recreated);
                self.surface.configure(&self.device, &self.config);
                (None, FrameStatus::Skipped)
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface_event(super::surface_lifecycle::SurfaceEvent::Recreated);
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .unwrap_or_else(|e| panic!("ponko: lost the GPU surface: {e}"));
                self.surface.configure(&self.device, &self.config);
                (None, FrameStatus::Skipped)
            }
        };
        if !super::surface_lifecycle::SurfaceLifecycle::draw(frame.is_some(), capture, self.draw_hidden) {
            return status;
        }
        let hidden = frame.is_none() && self.draw_hidden;
        let status = if hidden { FrameStatus::Offscreen } else { status };
        let mut hidden_view = hidden.then(|| self.hidden_view());
        if hidden {
            // As a swapchain would: wait while two frames are on the GPU.
            let wait = web_time::Instant::now();
            while self.hidden_in_flight.len() >= 2 {
                if let Some(oldest) = self.hidden_in_flight.pop_front() {
                    let _ = self.device.poll(wgpu::PollType::Wait {
                        submission_index: Some(oldest),
                        timeout: Some(std::time::Duration::from_secs(1)),
                    });
                }
            }
            self.display_wait_ms += wait.elapsed().as_secs_f32() * 1000.0;
        }

        let cpu_start = web_time::Instant::now();
        if let Some(timer) = &mut self.timer {
            timer.collect(&self.device);
            timer.begin_frame();
        }
        let (world, world_batches) = world.finish();
        let (ui, ui_batches) = ui.finish();
        let panels: Vec<_> = panels
            .iter_mut()
            .filter(|p| (p.texture as usize) < self.textures.len())
            .map(|p| (p.texture, p.size, p.list.finish()))
            .collect();
        let panel_sprites: usize = panels.iter().map(|(_, _, (sprites, _))| sprites.len()).sum();
        let total = world.len() + ui.len() + panel_sprites;
        self.stats.sprites = total;
        self.stats.draw_calls = world_batches.len()
            + ui_batches.len()
            + panels.iter().map(|(_, _, (_, batches))| batches.len()).sum::<usize>();
        #[cfg(feature = "3d")]
        if let Some(three) = &self.three {
            self.stats.draw_calls += three.draw_calls();
            let [(instances, triangles), (shadow_instances, shadow_triangles)] = three.counts();
            self.stats.instances = instances;
            self.stats.triangles = triangles;
            self.stats.shadow_instances = shadow_instances;
            self.stats.shadow_triangles = shadow_triangles;
            self.stats.occluded = three.occluded;
            self.stats.pipelines_built = three.take_pipelines_built();
            self.stats.instance_batches = three.instance_batch_stats();
            self.stats.collect_ms = self.collect_ms;
        }

        let size = size_of::<SpriteInstance>();
        let most = traffic::most_bytes(&self.device, wgpu::BufferUsages::VERTEX) / size;
        if traffic::outgrown(&mut self.instance_capacity, total, most) {
            self.instance_buffer = Self::create_instance_buffer(&self.device, self.instance_capacity);
            self.sprite_mirror.forget();
        }
        // More sprites than the device's largest buffer holds: reported,
        // and none drawn.
        let too_many = total > self.instance_capacity;
        if too_many && !self.too_many_sprites {
            traffic::too_big("sprites", total * size, most * size, "no sprites are drawn");
        }
        self.too_many_sprites = too_many;
        let (world, world_batches, ui, ui_batches, panels) = if too_many {
            (&[][..], &[][..], &[][..], &[][..], Vec::new())
        } else {
            (world, world_batches, ui, ui_batches, panels)
        };
        // Only the sprites that changed since the last frame are uploaded,
        // with the few unchanged ones between two of them, in at most
        // `MAX_WRITES` writes a list.
        let page = size_of::<SpriteInstance>();
        let mut upload_sprites = |at: usize, sprites: &[SpriteInstance]| {
            let (buffer, uploads) = (&self.instance_buffer, self.queue.uploading(Upload::Sprites));
            let bytes = bytemuck::cast_slice(sprites);
            self.sprite_mirror
                .sync_capped(at, bytes, (page, SPRITE_BRIDGE), MAX_WRITES, |at, bytes| {
                    uploads.write_buffer(buffer, at as u64, bytes);
                });
        };
        upload_sprites(0, world);
        upload_sprites(std::mem::size_of_val(world), ui);
        // World-space UI panels' sprites go after the screen's, each panel
        // with a camera of its own pixels.
        let mut panel_base = (world.len() + ui.len()) as u32;
        let mut panel_starts = Vec::with_capacity(panels.len());
        for (i, (_, size, (sprites, _))) in panels.iter().enumerate() {
            if !sprites.is_empty() {
                let offset = panel_base as usize * page;
                let (buffer, uploads) = (&self.instance_buffer, self.queue.uploading(Upload::Sprites));
                self.sprite_mirror.sync_capped(
                    offset,
                    bytemuck::cast_slice(sprites),
                    (page, SPRITE_BRIDGE),
                    MAX_WRITES,
                    |at, bytes| {
                        uploads.write_buffer(buffer, at as u64, bytes);
                    },
                );
            }
            if i == self.panel_cameras.len() {
                self.panel_cameras.push(self.make_camera("ui panel camera"));
            }
            let view = crate::ctx::screen_projection(glam::Vec2::new(size[0] as f32, size[1] as f32));
            self.queue.uploading(Upload::Uniforms).write_buffer(
                &self.panel_cameras[i].0,
                0,
                bytemuck::cast_slice(&view.to_cols_array()),
            );
            panel_starts.push(panel_base);
            panel_base += sprites.len() as u32;
        }
        if !panels.is_empty() && self.panel_pipeline.is_none() {
            // Panels draw into render targets, which only 3D makes.
            #[cfg(feature = "3d")]
            let format = super::gpu3d::HDR_FORMAT;
            #[cfg(not(feature = "3d"))]
            let format = self.view_format;
            self.panel_pipeline = Some(sprite_pipeline(
                &self.device,
                &self.sprite_layout,
                &self.sprite_shader,
                format,
            ));
        }
        let [world_mirror, ui_mirror] = &mut self.camera_mirrors;
        let world_camera = world_view.to_cols_array();
        world_mirror.upload(
            &self.queue,
            Upload::Uniforms,
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&world_camera),
        );
        let ui_camera = ui_view.to_cols_array();
        ui_mirror.upload(
            &self.queue,
            Upload::Uniforms,
            &self.ui_camera_buffer,
            0,
            bytemuck::cast_slice(&ui_camera),
        );
        // Blend modes and sprite materials: their pipelines and bind groups.
        if DrawList::batches_styled(world_batches) || DrawList::batches_styled(ui_batches) {
            for batches in [world_batches, ui_batches] {
                self.sprite_pipelines
                    .prepare(&self.device, &self.textures, batches, self.view_format);
            }
        }
        #[cfg(feature = "3d")]
        let panel_format = super::gpu3d::HDR_FORMAT;
        #[cfg(not(feature = "3d"))]
        let panel_format = self.view_format;
        for (_, _, (_, batches)) in &panels {
            if DrawList::batches_styled(batches) {
                self.sprite_pipelines
                    .prepare(&self.device, &self.textures, batches, panel_format);
            }
        }
        // 2D lighting: its lights and shadows, for the passes after the
        // world's sprites. Nothing at all while it's off.
        let lit = lights.frame.on;
        if lit {
            let lights2d = self.lights2d.get_or_insert_with(|| {
                super::lights2d::Lights2D::new(
                    &self.device,
                    &self.queue,
                    self.view_format,
                    &self.camera_layout,
                    &self.texture_layout,
                )
            });
            lights2d.prepare(
                &self.device,
                &self.queue,
                &lights.frame,
                world_view,
                [self.config.width, self.config.height],
                &lights.normal_maps,
                &self.textures,
                world_batches,
            );
        }
        let lighting = self.lights2d.as_ref().filter(|_| lit).map(|l| Lighting {
            lights: l,
            frame: &lights.frame,
            normal_maps: &lights.normal_maps,
        });
        // World sprites first, then the UI on top.
        let layers = [
            (&self.camera_bind_group, world_batches, 0),
            (&self.ui_camera_bind_group, ui_batches, world.len() as u32),
        ];
        let pass = SpritePass {
            viewport: [self.config.width, self.config.height],
            scale: self.window.scale_factor() as f32,
            pipeline: &self.pipeline,
            instances: &self.instance_buffer,
            textures: &self.textures,
            layers: &layers,
        };
        // Panels are drawn first, in buffers of their own: the 3D frame
        // samples them.
        let panel_commands = self
            .panel_pipeline
            .as_ref()
            .filter(|_| !panels.is_empty())
            .map(|pipeline| {
                let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ui panels"),
                });
                for (i, (texture, size, (_, batches))) in panels.iter().enumerate() {
                    let view = self.textures[*texture as usize]
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    let layers = [(&self.panel_cameras[i].1, *batches, panel_starts[i])];
                    let pass = SpritePass {
                        viewport: *size,
                        scale: 1.0,
                        pipeline,
                        instances: &self.instance_buffer,
                        textures: &self.textures,
                        layers: &layers,
                    };
                    Self::sprite_pass_with(
                        &mut encoder,
                        &view,
                        Some(Color::TRANSPARENT),
                        None,
                        &pass,
                        Some((&self.sprite_pipelines, panel_format)),
                    );
                }
                encoder.finish()
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let view = match &frame {
            Some(frame) => Some(frame.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.view_format),
                ..Default::default()
            })),
            None => hidden_view.take(),
        };
        // The 3D passes come in command buffers of their own (recorded in
        // parallel), submitted before `encoder`'s sprites go on top.
        let mut buffers_3d = Vec::new();
        let record_start = web_time::Instant::now();
        if let Some(view) = &view {
            let timer = self.timer.as_ref();
            let sprite_clear = self.draw_3d(&mut buffers_3d, view, clear, timer);
            Self::sprites_lit(
                &mut encoder,
                view,
                sprite_clear,
                timer,
                &pass,
                Some((&self.sprite_pipelines, self.view_format)),
                lighting.as_ref(),
            );
        }
        // A screenshot draws the same sprites a second time, into a texture
        // that can be copied (the window's surface can't, on some GPUs).
        let capture = capture.then(|| {
            let target = Capture::new(&self.device, self.config.width, self.config.height, self.view_format);
            let target_view = target.texture.create_view(&wgpu::TextureViewDescriptor::default());
            let sprite_clear = self.draw_3d(&mut buffers_3d, &target_view, clear, None);
            Self::sprites_lit(
                &mut encoder,
                &target_view,
                sprite_clear,
                None,
                &pass,
                Some((&self.sprite_pipelines, self.view_format)),
                lighting.as_ref(),
            );
            target.copy(&mut encoder);
            target
        });
        // The inspector goes on the window only, not into screenshots.
        #[cfg(feature = "inspector")]
        let overlay_commands = match &view {
            Some(view) => self.draw_overlay(&mut encoder, view),
            None => Vec::new(),
        };
        #[cfg(not(feature = "inspector"))]
        let overlay_commands: [wgpu::CommandBuffer; 0] = [];
        if let Some(timer) = &mut self.timer {
            timer.resolve(&mut encoder);
        }
        let finished = encoder.finish();
        self.stats.record_ms = record_start.elapsed().as_secs_f32() * 1000.0;
        let submit_start = web_time::Instant::now();
        let submitted = self.queue.submit(
            panel_commands
                .into_iter()
                .chain(overlay_commands)
                .chain(buffers_3d)
                .chain([finished]),
        );
        self.stats.submit_ms = submit_start.elapsed().as_secs_f32() * 1000.0;
        if hidden {
            self.hidden_in_flight.push_back(submitted);
        }
        #[cfg(feature = "3d")]
        if let Some(three) = &mut self.three {
            three.submitted();
        }
        if let Some(timer) = &mut self.timer {
            timer.submitted();
            self.stats.gpu_ms = timer.smoothed_ms;
        }
        self.stats.cpu_ms = cpu_start.elapsed().as_secs_f32() * 1000.0;
        self.stats.uploads = super::traffic::take();
        #[cfg(feature = "3d")]
        {
            self.last_prepare_3d_ms = std::mem::take(&mut self.prepare_3d_ms);
            self.stats.cpu_ms += self.last_prepare_3d_ms;
        }
        if let Some(frame) = frame {
            self.window.pre_present_notify();
            self.queue.present(frame);
        }
        if let Some(capture) = capture {
            self.screenshot = Some(capture.read(&self.device));
        }
        status
    }

    /// Hands the renderer the inspector's drawing for the next frame. A
    /// drawing that wasn't shown yet (the window was hidden) is replaced,
    /// but its texture changes are kept.
    #[cfg(feature = "inspector")]
    pub fn set_overlay(&mut self, mut overlay: crate::inspector::Overlay) {
        if let Some(older) = self.overlay.take() {
            overlay.merge_older(older);
        }
        self.overlay = Some(overlay);
    }

    /// Records the inspector's pass on top of the frame. Returns egui's
    /// extra command buffers, to submit before the frame's.
    #[cfg(feature = "inspector")]
    fn draw_overlay(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(overlay) = self.overlay.take() else {
            return Vec::new();
        };
        let renderer = self.egui.get_or_insert_with(|| {
            let options = egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: true,
                predictable_texture_filtering: false,
            };
            egui_wgpu::Renderer::new(&self.device, self.view_format, options)
        });
        for id in self.egui_free.drain(..) {
            renderer.free_texture(&id);
        }
        for (id, deltas) in &overlay.textures.set {
            for delta in deltas {
                renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        self.egui_free.extend(overlay.textures.free.iter().copied());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: overlay.pixels_per_point,
        };
        let commands = renderer.update_buffers(&self.device, &self.queue, encoder, &overlay.primitives, &screen);
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("inspector"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        renderer.render(&mut pass, &overlay.primitives, &screen);
        commands
    }

    /// Draws the frame's 3D models (clearing to `clear`) when there are
    /// any. Returns the color the sprite pass should clear to: `None` when
    /// it draws over the 3D models.
    #[cfg_attr(not(feature = "3d"), allow(clippy::unused_self))]
    /// Adds the 3D frame's command buffers to `buffers`; the clear color
    /// the sprite pass still needs (none if 3D drew the background).
    fn draw_3d(
        &self,
        buffers: &mut Vec<wgpu::CommandBuffer>,
        view: &wgpu::TextureView,
        clear: Color,
        timer: Option<&GpuTimer>,
    ) -> Option<Color> {
        #[cfg(feature = "3d")]
        if let Some(three) = self.three.as_ref().filter(|t| t.has_draws()) {
            buffers.extend(three.draw(&self.device, view, clear.to_linear_wgpu(), &self.textures, timer));
            return None;
        }
        let _ = (buffers, view, timer);
        Some(clear)
    }

    /// A camera uniform and its bind group for the sprite pipeline.
    fn make_camera(&self, label: &str) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = self.device.new_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size_of::<Mat4>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        (buffer, bind_group)
    }

    #[cfg_attr(not(all(test, feature = "3d")), allow(dead_code))]
    pub(super) fn sprite_pass(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: Option<Color>,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
        p: &SpritePass<'_>,
    ) {
        Self::sprite_pass_with(encoder, view, clear, timestamp_writes, p, None);
    }

    /// The sprite pass, lit: without `lighting`, exactly
    /// [`sprite_pass_with`](Self::sprite_pass_with); with it, the first
    /// layer (the world), the 2D lighting passes over it, then the other
    /// layers (the UI) unlit on top.
    pub(super) fn sprites_lit(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: Option<Color>,
        timer: Option<&GpuTimer>,
        p: &SpritePass<'_>,
        looks: Option<(&super::sprite_pipelines::SpritePipelines, wgpu::TextureFormat)>,
        lighting: Option<&Lighting<'_>>,
    ) {
        let (Some(lighting), Some((world, rest))) = (lighting, p.layers.split_first()) else {
            Self::sprite_pass_with(encoder, view, clear, timer.and_then(|t| t.pass("sprites")), p, looks);
            return;
        };
        let world_only = SpritePass {
            layers: std::slice::from_ref(world),
            ..*p
        };
        Self::sprite_pass_with(
            encoder,
            view,
            clear,
            timer.and_then(|t| t.pass("sprites")),
            &world_only,
            looks,
        );
        lighting.lights.draw(
            encoder,
            view,
            lighting.frame,
            *world,
            p.instances,
            p.textures,
            lighting.normal_maps,
            timer.and_then(|t| t.pass("2d lights")),
        );
        let rest = SpritePass { layers: rest, ..*p };
        if !rest.layers.iter().all(|(_, batches, _)| batches.is_empty()) {
            Self::sprite_pass_with(encoder, view, None, None, &rest, looks);
        }
    }

    /// [`sprite_pass`](Self::sprite_pass) with the blend modes' and sprite
    /// materials' pipelines for `view`'s format; without them, batches with
    /// a look draw plain.
    pub(super) fn sprite_pass_with(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: Option<Color>,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
        p: &SpritePass<'_>,
        looks: Option<(&super::sprite_pipelines::SpritePipelines, wgpu::TextureFormat)>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprites"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, |c| wgpu::LoadOp::Clear(c.to_linear_wgpu())),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if p.layers.iter().all(|(_, batches, _)| batches.is_empty()) {
            return;
        }
        pass.set_pipeline(p.pipeline);
        pass.set_vertex_buffer(0, p.instances.slice(..));
        // The look the pass draws with now: plain sprites (0) until a batch
        // with a blend mode or material comes.
        let mut look = 0;
        for (camera, batches, base) in p.layers {
            pass.set_bind_group(0, *camera, &[]);
            for batch in *batches {
                let [width, height] = p.viewport;
                let (x, y, w, h) = batch.clip.map_or((0, 0, width, height), |r| {
                    let x = (r.min.x * p.scale).floor().clamp(0.0, width as f32) as u32;
                    let y = (r.min.y * p.scale).floor().clamp(0.0, height as f32) as u32;
                    let right = (r.max.x * p.scale).ceil().clamp(x as f32, width as f32) as u32;
                    let bottom = (r.max.y * p.scale).ceil().clamp(y as f32, height as f32) as u32;
                    (x, y, right - x, bottom - y)
                });
                if w == 0 || h == 0 {
                    continue;
                }
                pass.set_scissor_rect(x, y, w, h);
                // `Texture::NONE` (no texture) draws nothing.
                let Some(texture) = p.textures.get(batch.texture as usize) else {
                    continue;
                };
                if batch.material != look {
                    match looks.filter(|_| batch.material != 0) {
                        Some((looks, format)) => {
                            // Not made (a GPU error): skipped.
                            let Some((pipeline, material)) = looks.get(batch.material, format) else {
                                continue;
                            };
                            pass.set_pipeline(pipeline);
                            if let Some(material) = material {
                                pass.set_bind_group(2, material, &[]);
                            }
                        }
                        None => pass.set_pipeline(p.pipeline),
                    }
                    look = batch.material;
                }
                pass.set_bind_group(1, &texture.bind_group, &[]);
                pass.draw(0..6, base + batch.start..base + batch.end);
            }
        }
    }
}

/// Bytes `texture` takes on the GPU, every mipmap level and layer
/// (block-compressed formats by whole blocks).
pub(super) fn texture_bytes(texture: &wgpu::Texture) -> u64 {
    let block = u64::from(texture.format().block_copy_size(None).unwrap_or(4));
    let (bw, bh) = texture.format().block_dimensions();
    let layers = u64::from(texture.depth_or_array_layers());
    let samples = u64::from(texture.sample_count());
    (0..texture.mip_level_count())
        .map(|level| {
            let w = u64::from((texture.width() >> level).max(1).div_ceil(bw));
            let h = u64::from((texture.height() >> level).max(1).div_ceil(bh));
            w * h * block
        })
        .sum::<u64>()
        * layers
        * samples
}

/// Exact alpha comparison avoids invalidating built-in cutout shadows for
/// RGB-only uploads. Dimensions participate because filtering depends on them.
#[cfg(feature = "3d")]
#[derive(Debug, PartialEq)]
struct TextureAlpha {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}
#[cfg(feature = "3d")]
impl TextureAlpha {
    fn new(width: u32, height: u32, rgba: &[u8]) -> Self {
        Self {
            width,
            height,
            pixels: rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect(),
        }
    }

    fn update(&mut self, at: [u32; 2], size: [u32; 2], rgba: &[u8]) -> bool {
        let mut changed = false;
        for (i, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let x = at[0] as usize + i % size[0] as usize;
            let y = at[1] as usize + i / size[0] as usize;
            let old = &mut self.pixels[y * self.width as usize + x];
            changed |= *old != pixel[3];
            *old = pixel[3];
        }
        changed
    }
}

#[cfg(all(test, feature = "3d"))]
mod shadow_alpha_tests {
    use super::TextureAlpha;
    #[test]
    fn alpha_updates_compare_exact_pixels_and_region_strides() {
        let mut alpha = TextureAlpha::new(2, 2, &[255; 16]);
        assert!(!alpha.update([0, 0], [2, 2], &[64, 80, 90, 255].repeat(4)));
        assert!(alpha.update([1, 0], [1, 2], &[255, 255, 255, 0].repeat(2)));
        assert!(!alpha.update([1, 0], [1, 2], &[0; 8]));
        assert_eq!(alpha.pixels, [255, 0, 255, 0]);
        assert_ne!(alpha, TextureAlpha::new(1, 4, &[255; 16]));
    }
}

/// Whether the 3D frame switches from `current` MSAA samples to `wanted`:
/// only to a different count of 2 or 4 the GPU takes, and never on a GPU
/// without multisampling (`current` 1, WebGL2).
#[cfg(feature = "3d")]
fn msaa_switch(current: u32, wanted: u32, supports_msaa2: bool) -> bool {
    wanted != current && current != 1 && (wanted == 4 || (wanted == 2 && supports_msaa2))
}

/// Why a requested sample count is not used, worth a line on stderr: a
/// count no GPU takes, or 2 on a GPU with only 4. `None` when it is used,
/// and on a GPU without multisampling (its capabilities say so).
#[cfg(feature = "3d")]
fn msaa_refusal(current: u32, wanted: u32, supports_msaa2: bool) -> Option<String> {
    if !matches!(wanted, 2 | 4) {
        Some(format!(
            "post.msaa takes 2 or 4 samples, not {wanted}; keeping {current}x"
        ))
    } else if wanted == 2 && current != 1 && !supports_msaa2 {
        Some(format!("this GPU can't do 2x multisampling here; using {current}x"))
    } else {
        None
    }
}

#[cfg(all(test, feature = "3d"))]
mod msaa_tests {
    use super::msaa_switch;

    #[test]
    fn msaa_switches_only_to_another_count_the_gpu_takes() {
        assert!(msaa_switch(4, 2, true));
        assert!(msaa_switch(2, 4, false));
        assert!(!msaa_switch(4, 4, true), "the same count");
        assert!(!msaa_switch(4, 2, false), "2 where the GPU has only 4");
        for wanted in [0, 1, 3, 8, 16] {
            assert!(!msaa_switch(4, wanted, true), "{wanted} samples");
        }
        for wanted in [2, 4] {
            assert!(!msaa_switch(1, wanted, true), "no multisampling");
        }
    }

    #[test]
    fn refused_counts_say_why_once_they_are_asked_for() {
        use super::msaa_refusal;
        assert_eq!(msaa_refusal(4, 4, true), None);
        assert_eq!(msaa_refusal(4, 2, true), None);
        assert_eq!(
            msaa_refusal(4, 8, true).as_deref(),
            Some("post.msaa takes 2 or 4 samples, not 8; keeping 4x")
        );
        assert_eq!(
            msaa_refusal(4, 2, false).as_deref(),
            Some("this GPU can't do 2x multisampling here; using 4x")
        );
        assert_eq!(msaa_refusal(1, 2, false), None, "WebGL2: the capabilities say so");
        assert!(msaa_refusal(1, 0, false).is_some());
    }
}

/// The layers a texture array is made with. wgpu's GL backend (WebGL2, `gl`)
/// makes a one-layer array an ordinary 2D texture, which shaders can't read
/// as an array (it reads as nothing): there it gets a second layer, a copy
/// of the first, which a read past the last layer returns anyway.
#[cfg(feature = "3d")]
fn stored_layers<'s, 'a>(layers: &'s [&'a [u8]], gl: bool) -> std::borrow::Cow<'s, [&'a [u8]]> {
    if gl && layers.len() == 1 {
        std::borrow::Cow::Owned(vec![layers[0], layers[0]])
    } else {
        std::borrow::Cow::Borrowed(layers)
    }
}

#[cfg(all(test, feature = "3d"))]
mod stored_layer_tests {
    use super::stored_layers;

    #[test]
    fn one_layer_arrays_get_a_second_copy_on_gl_only() {
        let (a, b): (&[u8], &[u8]) = (&[1, 2, 3, 4], &[5, 6, 7, 8]);
        assert_eq!(&*stored_layers(&[a], true), &[a, a]);
        assert_eq!(&*stored_layers(&[a], false), &[a]);
        assert_eq!(
            &*stored_layers(&[a, b], true),
            &[a, b],
            "two layers are an array already"
        );
    }
}
