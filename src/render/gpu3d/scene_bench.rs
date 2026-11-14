//! Offscreen 3D scenes for the rendering performance baseline (`ponko_bench`,
//! `render/scene`): eight reproducible worlds drawn with the real
//! `Renderer3D` into a texture, without a window, each frame split into the
//! steps a game's frame has (collect, prepare, encode, submit, the GPU's
//! work) and counted (draw calls, triangles, uploads, GPU memory).
//!
//! The scenes are made of engine content only: boxes, spheres, planes, a
//! height field, generated textures and a generated skinned figure, placed
//! by a fixed pseudo-random sequence, so every run draws the same thing.
//!
//! This is the `Renderer3D` a game uses, driven the way `Ctx::prepare_3d`
//! and `Renderer::render` drive it, but it is not a whole game frame: there
//! are no systems, no 2D or UI layer, no presentation and no display wait.
//! Nothing here changes what a game's frame runs; the harness only calls
//! the renderer and reads counters it already keeps. Not part of the public
//! API. See `crates/ponko_bench/RENDER_SCENES.md`.

use std::time::{Duration, Instant};

use glam::{Mat4, Quat, Vec2, Vec3};

use super::{Prepare3D, Renderer3D, StateChanges};
use crate::d3::{PointLight as PointLight3D, SpotLight as SpotLight3D, Transform as Transform3D};
use crate::render::gpu::GpuTexture;
use crate::render::timer::GpuTimer;
use crate::render::{
    GpuCapabilities, GpuTier, GpuUploads, GraphicsQuality, InstanceBatchOptions, InstanceBatchStats, InstanceLevel,
    Material, MaterialDesc, Mesh, MeshData, Model, ShadowFilter,
};
use crate::texture::Texture;
use crate::three::{Blend, GpuMaterialDesc, Joint, Scene3D, Skeleton, SkyLight};
use crate::{color::Color, ctx::Ctx};
use ponko_ecs::Component;

/// A scene of the rendering baseline.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneKind {
    /// One mesh and material, to detect simple-scene regressions.
    SingleMaterial,
    /// A large world that never changes: eight instance batches and a
    /// ground, under a moving camera.
    StaticWorld,
    /// Ordinary models that all move every frame.
    DynamicWorld,
    /// Many meshes, each with a material of its own: textures, cut-outs,
    /// two-sided and metallic surfaces.
    MultiMaterial,
    /// Sun cascades at the highest resolution redrawn every frame, shadowed
    /// point and spot lights, moving casters.
    ShadowHeavy,
    /// Overlapping see-through and cut-out surfaces, sorted every frame.
    AlphaHeavy,
    /// Skinned figures, every skeleton posed every frame.
    Animated,
    /// A small town: terrain, batched buildings and trees with distance
    /// levels, moving vehicles, skinned walkers, lamps, glass and water,
    /// with occlusion culling.
    GameLike,
}

impl SceneKind {
    /// Every scene, in the order the baseline lists them.
    pub const ALL: [SceneKind; 8] = [
        SceneKind::SingleMaterial,
        SceneKind::StaticWorld,
        SceneKind::DynamicWorld,
        SceneKind::MultiMaterial,
        SceneKind::ShadowHeavy,
        SceneKind::AlphaHeavy,
        SceneKind::Animated,
        SceneKind::GameLike,
    ];

    /// The name used in commands, reports and file names.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SceneKind::SingleMaterial => "single-material",
            SceneKind::StaticWorld => "static-world",
            SceneKind::DynamicWorld => "dynamic-world",
            SceneKind::MultiMaterial => "multi-material",
            SceneKind::ShadowHeavy => "shadow-heavy",
            SceneKind::AlphaHeavy => "alpha-heavy",
            SceneKind::Animated => "animated",
            SceneKind::GameLike => "game-like",
        }
    }

    /// What the scene holds and what changes from frame to frame.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            SceneKind::SingleMaterial => "one instanced cube mesh and material, 100 ordinary models",
            SceneKind::StaticWorld => {
                "eight static instance batches (boxes and spheres) and a ground; only the camera moves"
            }
            SceneKind::DynamicWorld => "ordinary models of 16 meshes, every one moved and turned every frame",
            SceneKind::MultiMaterial => {
                "static models over many meshes, one material each: 16 generated textures, cut-out, two-sided and metallic surfaces"
            }
            SceneKind::ShadowHeavy => {
                "four 4096 sun cascades redrawn every frame, three shadowed point lights and six shadowed spot lights, half of the casters moving, and a batch of casting pillars"
            }
            SceneKind::AlphaHeavy => {
                "see-through panes, boxes and spheres in a dense cluster, turning every frame, with cut-out cards and a ground"
            }
            SceneKind::Animated => "skinned figures of 16 joints, every skeleton posed every frame",
            SceneKind::GameLike => {
                "a small town: height-field terrain, batched buildings on a street grid, trees with two distance levels, vehicles driving the streets, skinned walkers, lamps, glass, water, bloom and occlusion culling"
            }
        }
    }
}

/// How a [`SceneBench`] is opened.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct SceneSetup {
    /// The output's width and height, in pixels.
    pub size: (u32, u32),
    /// Samples per pixel asked for (1, 2, 4 or 8). Where the adapter can't
    /// multisample the 3D targets that much, 4 is used, as a game's window does.
    pub samples: u32,
    /// Multiplies every scene's object counts: 1 for the baseline, less for
    /// smoke tests. Every count keeps a small minimum.
    pub scale: f32,
    /// The preset every scene starts from (`GraphicsQuality`); a scene then
    /// changes what it is about (see `SceneBench::quality_json`).
    pub quality: GraphicsQuality,
    /// Draw only what another engine's copy of the scene can draw too: a
    /// plain sky instead of the atmosphere, no bloom, ambient occlusion or
    /// screen-space reflections, FXAA, and every shadow cascade redrawn
    /// every frame. For the side-by-side scenes in `bench/bevy_scenes`.
    pub matched: bool,
}

impl Default for SceneSetup {
    fn default() -> Self {
        Self {
            size: (1280, 720),
            samples: 4,
            scale: 1.0,
            quality: GraphicsQuality::High,
            matched: false,
        }
    }
}

/// One frame of a [`SceneBench`]: what each step cost and what was drawn.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SceneFrame {
    /// Moving the scene's own objects (transforms, skeleton poses, the
    /// camera): the game's side, not the renderer's.
    pub update: Duration,
    /// Finding what the camera and the shadow maps see: skeletons, target
    /// views and `Frame3D::collect` (culling every model).
    pub collect: Duration,
    /// `Renderer3D::prepare`: uploads, draw lists and the frame's decisions.
    pub prepare: Duration,
    /// `Renderer3D::draw`: recording every pass into command buffers.
    pub encode: Duration,
    /// `Queue::submit`.
    pub submit: Duration,
    /// From the start of `collect` until the GPU finished the frame.
    pub frame: Duration,
    /// The frame's GPU passes were timed (see `SceneBench::gpu_passes`).
    pub gpu_timed: bool,
    /// The passes' GPU times added up, in ms, if timed.
    pub gpu_ms: Option<f64>,
    /// From the first pass's start to the last one's end on the GPU, in ms.
    pub gpu_span_ms: Option<f64>,
    /// `RenderStats::draw_calls` for the 3D frame.
    pub draw_calls: usize,
    /// Draws the model passes recorded (instance batches included).
    pub recorded_draws: u32,
    /// Pipelines, bind groups, vertex and index buffers the model passes set.
    pub state_changes: [u32; 4],
    /// Models and triangles the camera draws.
    pub instances: usize,
    pub triangles: usize,
    /// Models and triangles drawn into the shadow maps.
    pub shadow_instances: usize,
    pub shadow_triangles: usize,
    /// Models in view left out because something hid them.
    pub occluded: usize,
    /// Pipeline variants built during this frame.
    pub pipelines_built: usize,
    /// Command buffers submitted.
    pub command_buffers: usize,
    /// Bytes uploaded and buffers created during this frame.
    pub uploads: GpuUploads,
}

/// What a scene put into the world.
#[derive(Clone, Copy, Debug, Default)]
struct Content {
    models: usize,
    moving_models: usize,
    skinned_models: usize,
    joints_per_skeleton: usize,
    transparent_models: usize,
    meshes: usize,
    materials: usize,
    textures: usize,
    static_batches: usize,
    static_instances: usize,
    point_lights: usize,
    spot_lights: usize,
    shadowed_lights: usize,
}

/// How a model moves each frame.
#[derive(Clone, Copy)]
enum Motion {
    /// Up and down around its origin, turning.
    Bob,
    /// Back and forth through its origin along X or Z (a street).
    Street { along_x: bool },
}

/// A model the scene moves every frame.
#[derive(Clone, Copy, Component)]
struct Mover {
    origin: Vec3,
    phase: f32,
    radius: f32,
    speed: f32,
    motion: Motion,
}

/// A skeleton the scene poses every frame.
#[derive(Clone, Copy, Component)]
struct Sway {
    phase: f32,
}

/// The bounds of the scenes' random angles: about half a turn and a turn.
/// Any bound would do; these are the ones the recorded baselines were placed
/// with, and changing them moves every object.
#[allow(clippy::approx_constant)]
const HALF_TURN: f32 = 3.14;
#[allow(clippy::approx_constant)]
const TURN: f32 = 6.28;

/// The fixed pseudo-random sequence the scenes are placed with.
struct Sequence(u64);

impl Sequence {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Between `-half` and `half`.
    fn around(&mut self, half: f32) -> f32 {
        (self.next() * 2.0 - 1.0) * half
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        low + self.next() * (high - low)
    }
}

/// An offscreen 3D renderer with one of the baseline's scenes.
#[doc(hidden)]
pub struct SceneBench {
    kind: SceneKind,
    setup: SceneSetup,
    info: wgpu::AdapterInfo,
    capabilities: GpuCapabilities,
    features: wgpu::Features,
    samples: u32,
    solid_prepass: bool,
    device: wgpu::Device,
    queue: wgpu::Queue,
    output: wgpu::Texture,
    target: wgpu::TextureView,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: Vec<GpuTexture>,
    texture_bytes: u64,
    mips: Option<crate::render::mips::MipMaker>,
    renderer: Renderer3D,
    timer: Option<GpuTimer>,
    gpu_frames_read: u64,
    /// The latest timed frame's passes: name and ms, same names added up.
    gpu_passes: Vec<(&'static str, f64)>,
    ctx: Ctx,
    content: Content,
    /// The camera circles this point at this distance and height.
    orbit: (Vec3, f32, f32),
    frames: u32,
    shadow_workload: Option<String>,
}

/// Bytes of an RGBA8 texture with a full mip chain.
fn mipped_bytes(width: u32, height: u32) -> u64 {
    let (mut w, mut h, mut total) = (u64::from(width), u64::from(height), 0);
    loop {
        total += w * h * 4;
        if w == 1 && h == 1 {
            return total;
        }
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
}

impl SceneBench {
    /// The optional features the scenes use where the adapter has them, and
    /// the limits a game's device is opened with.
    fn descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
        wgpu::DeviceDescriptor {
            label: Some("ponko scene benchmark"),
            required_features: adapter.features()
                & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES),
            required_limits: wgpu::Limits {
                max_sampled_textures_per_shader_stage: adapter
                    .limits()
                    .max_sampled_textures_per_shader_stage
                    .clamp(16, 24),
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }
    }

    /// The sample count a scene draws with: what `setup` asks for where the
    /// adapter can multisample the 3D targets that much, otherwise 4, as a
    /// game's window does.
    fn samples(adapter: &wgpu::Adapter, setup: &SceneSetup) -> u32 {
        if setup.samples == 1 || super::multisamples(adapter, setup.samples) {
            setup.samples
        } else {
            4
        }
    }

    /// Opens the GPU as a game does (the high-performance adapter, the
    /// engine's limits and optional features, `PONKO_GPU_TIER` honored) and
    /// builds `kind`. `Err` says why there is no GPU to draw with.
    ///
    /// This is the benchmark's way in: it picks the adapter it then reports.
    /// Tests open their device through `render_test_support` and use
    /// [`SceneBench::on_device`].
    pub fn new(kind: SceneKind, setup: SceneSetup) -> Result<Self, String> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.flags = wgpu::InstanceFlags::from_env_or_default();
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter ({e})"))?;
        let info = adapter.get_info();
        let mut descriptor = Self::descriptor(&adapter);
        if let Some(tier) = GpuTier::forced() {
            descriptor.required_features = tier.device_features(descriptor.required_features);
            descriptor.required_limits = tier.device_limits(descriptor.required_limits);
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))
            .map_err(|e| format!("no GPU device on {} ({e})", info.name))?;
        let samples = Self::samples(&adapter, &setup);
        Self::on_device(kind, setup, info, device, queue, samples)
    }

    /// Builds `kind` on a device that is already open, drawing with
    /// `samples` samples per pixel (see `SceneBench::samples`). `Err` when
    /// the target doesn't fit the device.
    fn on_device(
        kind: SceneKind,
        setup: SceneSetup,
        info: wgpu::AdapterInfo,
        device: wgpu::Device,
        queue: wgpu::Queue,
        samples: u32,
    ) -> Result<Self, String> {
        let most = device.limits().max_texture_dimension_2d;
        if setup.size.0 == 0 || setup.size.1 == 0 || setup.size.0 > most || setup.size.1 > most {
            return Err(format!(
                "a {} x {} target doesn't fit this device (textures up to {most})",
                setup.size.0, setup.size.1
            ));
        }
        let mut capabilities = crate::render::capabilities::of_device(&info, &device, (setup.samples, samples));
        capabilities.forced_tier = GpuTier::forced();
        // As a game's window: Apple's GPUs skip the solid depth prepass.
        let solid_prepass = info.backend != wgpu::Backend::Metal;
        let (layout, sampler, white) = super::record_bench::white_texture(&device, &queue);
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene benchmark output"),
            size: wgpu::Extent3d {
                width: setup.size.0,
                height: setup.size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target = output.create_view(&Default::default());
        let renderer = Renderer3D::new(
            &device,
            &layout,
            output.format(),
            0,
            &white.texture,
            samples,
            solid_prepass,
        );
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(setup.size.0 as f32, setup.size.1 as f32));
        ctx.scene3d.set_quality(setup.quality);
        ctx.gfx.assets3d.decals_supported = super::supports_decals(&device);
        let timer = GpuTimer::new(&device, &queue);
        let mut bench = Self {
            kind,
            setup,
            features: device.features(),
            info,
            capabilities,
            samples,
            solid_prepass,
            device,
            queue,
            output,
            target,
            layout,
            sampler,
            textures: vec![white],
            texture_bytes: 4,
            mips: None,
            renderer,
            timer,
            gpu_frames_read: 0,
            gpu_passes: Vec::new(),
            ctx,
            content: Content::default(),
            orbit: (Vec3::ZERO, 10.0, 5.0),
            frames: 0,
            shadow_workload: std::env::var("PONKO_SHADOW_WORKLOAD").ok(),
        };
        bench.build();
        if setup.matched {
            let ctx = &mut bench.ctx;
            ctx.scene3d.sky.atmosphere = false;
            ctx.scene3d.post.bloom = 0.0;
            ctx.scene3d.post.ambient_occlusion.enabled = false;
            ctx.scene3d.post.ssr = 0.0;
            ctx.scene3d.post.anti_aliasing = crate::render::AntiAliasing::Fxaa;
            ctx.scene3d.lighting.shadow_refresh = 1;
        }
        if bench.shadow_workload.is_some() {
            bench.ctx.scene3d.lighting.shadow_refresh = 1;
        }
        // TAA, for what motion vectors cost (no preset turns it on).
        if std::env::var("PONKO_RENDER_TAA").is_ok_and(|v| v.trim() == "1") {
            bench.ctx.scene3d.post.anti_aliasing = crate::render::AntiAliasing::Taa;
        }
        bench.place_camera();
        let _ = bench.device.poll(wgpu::PollType::wait_indefinitely());
        // The scene's own uploads are not a frame's.
        crate::render::traffic::take();
        Ok(bench)
    }

    // ------------------------------------------------------------ building

    /// `base` scaled by the setup, at least `least`.
    fn count(&self, base: usize, least: usize) -> usize {
        ((base as f32 * self.setup.scale).round() as usize).max(least)
    }

    /// A material, on the CPU and the GPU.
    fn material(&mut self, desc: MaterialDesc) -> Material {
        let material = self.ctx.gfx.create_material(desc);
        let id = self
            .renderer
            .create_material(&self.device, &GpuMaterialDesc::new(&desc), &self.textures);
        assert_eq!(id, material.id, "the renderer's materials follow the game's");
        self.content.materials += 1;
        material
    }

    /// A mesh drawn with `material`, on the CPU and the GPU.
    fn mesh(&mut self, data: &MeshData, material: Material) -> Mesh {
        let mesh = self.ctx.gfx.create_mesh(data, material);
        self.renderer.create_mesh_at(
            &self.device,
            mesh.slot(),
            data,
            &[(0..data.indices.len() as u32, material)],
            false,
        );
        self.content.meshes += 1;
        mesh
    }

    /// A mesh that bends with `skeleton`.
    fn skinned_mesh(&mut self, data: &MeshData, material: Material, skeleton: &Skeleton) -> Mesh {
        let mesh = self.ctx.gfx.create_skinned_mesh(data, material, skeleton);
        // The figure's weights already add up to 1, as the game's copy has them.
        self.renderer.create_mesh_at(
            &self.device,
            mesh.slot(),
            data,
            &[(0..data.indices.len() as u32, material)],
            true,
        );
        self.content.meshes += 1;
        mesh
    }

    /// A 64 x 64 generated color texture with a full mip chain. `holes`:
    /// every other square is see-through (for cut-out materials).
    fn texture(&mut self, index: u32, holes: bool) -> Texture {
        const SIDE: u32 = 64;
        let squares = 2 + index % 4 * 2;
        let (a, b) = (
            [
                200 - (index * 23 % 120) as u8,
                120 + (index * 41 % 120) as u8,
                90 + (index * 67 % 150) as u8,
            ],
            [
                70 + (index * 13 % 60) as u8,
                60 + (index * 29 % 80) as u8,
                50 + (index * 53 % 90) as u8,
            ],
        );
        let mut rgba = Vec::with_capacity((SIDE * SIDE * 4) as usize);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let odd = (x * squares / SIDE + y * squares / SIDE) % 2 == 1;
                let [r, g, bl] = if odd { b } else { a };
                rgba.extend_from_slice(&[r, g, bl, if odd && holes { 0 } else { 255 }]);
            }
        }
        let levels = crate::render::mips::levels(SIDE, SIDE);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene benchmark texture"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIDE * 4),
                rows_per_image: None,
            },
            texture.size(),
        );
        let mips = self
            .mips
            .get_or_insert_with(|| crate::render::mips::MipMaker::new(&self.device));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        mips.generate(&self.device, &mut encoder, &texture, false);
        self.queue.submit([encoder.finish()]);
        let view = texture.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let id = self.textures.len() as u32;
        self.textures.push(GpuTexture {
            texture,
            nearest: false,
            mips: Some(false),
            bind_group,
        });
        self.texture_bytes += mipped_bytes(SIDE, SIDE);
        self.content.textures += 1;
        Texture::new(id, SIDE, SIDE)
    }

    /// Hands the newest batch the game made to the renderer.
    fn upload_batch(&mut self, id: u32, instances: usize) {
        self.renderer.set_instance_batch(
            &self.device,
            &self.queue,
            id,
            self.ctx.gfx.assets3d.instance_batches[id as usize].as_ref(),
        );
        self.content.static_batches += 1;
        self.content.static_instances += instances;
    }

    /// A static batch of `mesh` at `transforms`.
    fn batch(&mut self, mesh: Mesh, transforms: &[Mat4]) {
        let batch = self
            .ctx
            .gfx
            .create_instance_batch(mesh, transforms, InstanceBatchOptions::default());
        self.upload_batch(batch.id, transforms.len());
    }

    /// A static batch drawing `near` up to `split` away and `far` beyond.
    fn batch_levels(&mut self, near: Mesh, far: Mesh, split: f32, transforms: &[Mat4]) {
        let level = |mesh, min_distance, max_distance| InstanceLevel {
            mesh,
            min_distance,
            max_distance,
            camera: true,
            shadows: true,
        };
        let levels = [level(near, 0.0, split), level(far, split, f32::INFINITY)];
        let batch =
            self.ctx
                .gfx
                .create_instance_batch_levels(&levels, transforms, &[], InstanceBatchOptions::default());
        self.upload_batch(batch.id, transforms.len());
    }

    /// A model that stays where it is.
    fn model(&mut self, mesh: Mesh, at: Transform3D) {
        self.ctx.world.spawn((at, Model::new(mesh)));
        self.content.models += 1;
    }

    /// A model the scene moves every frame.
    fn mover(&mut self, mesh: Mesh, mover: Mover) {
        self.ctx
            .world
            .spawn((Transform3D::at(mover.origin), Model::new(mesh), mover));
        self.content.models += 1;
        self.content.moving_models += 1;
    }

    /// A ground `size` wide at height 0.
    fn ground(&mut self, size: f32, color: Color) {
        let material = self.material(MaterialDesc {
            color,
            roughness: 0.9,
            ..Default::default()
        });
        let mesh = self.mesh(&MeshData::plane(size), material);
        self.model(mesh, Transform3D::default());
    }

    /// The skinned figure: a column 1.8 tall of 16 rings on a chain of 16
    /// joints, each corner following its ring's joint and the next.
    fn figure(&mut self, color: Color) -> (Mesh, Skeleton) {
        const JOINTS: usize = 16;
        const SIDES: u32 = 12;
        let height = 1.8;
        let skeleton = Skeleton::new(
            (0..JOINTS)
                .map(|j| Joint {
                    name: format!("joint {j}"),
                    parent: j.checked_sub(1),
                    rest: Transform3D::at(Vec3::new(0.0, if j == 0 { 0.0 } else { height / JOINTS as f32 }, 0.0)),
                })
                .collect(),
        );
        let mut data = MeshData::default();
        let rings = JOINTS as u32 + 1;
        for ring in 0..rings {
            let along = ring as f32 / (rings - 1) as f32;
            // Wider in the middle, like a body.
            let radius = 0.12 + 0.14 * (along * std::f32::consts::PI).sin();
            let joint = (ring as usize).min(JOINTS - 1) as u16;
            let below = joint.saturating_sub(1);
            for side in 0..SIDES {
                let angle = side as f32 / SIDES as f32 * std::f32::consts::TAU;
                let out = Vec3::new(angle.cos(), 0.0, angle.sin());
                data.positions.push(out * radius + Vec3::Y * along * height);
                data.normals.push(out);
                data.uvs.push(Vec2::new(side as f32 / SIDES as f32, along));
                data.joints.push([joint, below, 0, 0]);
                data.weights.push([0.7, 0.3, 0.0, 0.0]);
            }
        }
        for ring in 0..rings - 1 {
            for side in 0..SIDES {
                let (a, b) = (ring * SIDES + side, ring * SIDES + (side + 1) % SIDES);
                let (c, d) = (a + SIDES, b + SIDES);
                data.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        let material = self.material(MaterialDesc {
            color,
            roughness: 0.6,
            ..Default::default()
        });
        self.content.joints_per_skeleton = JOINTS;
        (self.skinned_mesh(&data, material, &skeleton), skeleton)
    }

    /// A figure standing at `at`, swaying from `phase`.
    fn walker(&mut self, mesh: Mesh, skeleton: &Skeleton, at: Vec3, phase: f32) {
        self.ctx
            .world
            .spawn((Transform3D::at(at), Model::new(mesh), skeleton.clone(), Sway { phase }));
        self.content.models += 1;
        self.content.skinned_models += 1;
    }

    fn point_light(&mut self, at: Vec3, color: Color, range: f32, shadows: bool) {
        self.ctx.world.spawn((
            Transform3D::at(at),
            PointLight3D {
                color,
                intensity: 4.0,
                range,
                shadows,
                ..Default::default()
            },
        ));
        self.content.point_lights += 1;
        self.content.shadowed_lights += usize::from(shadows);
    }

    /// A spot light at `at` shining straight down.
    fn spot_light(&mut self, at: Vec3, color: Color, range: f32, shadows: bool) {
        self.ctx.world.spawn((
            Transform3D::at(at).rotated(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            SpotLight3D {
                color,
                intensity: 8.0,
                range,
                inner_angle: 0.4,
                outer_angle: 0.7,
                shadows,
                ..Default::default()
            },
        ));
        self.content.spot_lights += 1;
        self.content.shadowed_lights += usize::from(shadows);
    }

    /// `kinds` meshes, boxes and spheres in turn (`size` scales them), each
    /// with a plain material of its own color.
    fn simple_meshes(&mut self, kinds: usize, size: f32) -> Vec<Mesh> {
        (0..kinds)
            .map(|i| {
                let shade = i as f32 / kinds as f32;
                let material = self.material(MaterialDesc {
                    color: Color::rgb(
                        0.35 + 0.6 * shade,
                        0.75 - 0.4 * shade,
                        0.4 + 0.3 * ((i % 3) as f32 / 2.0),
                    ),
                    roughness: 0.4 + 0.5 * shade,
                    ..Default::default()
                });
                let data = if i % 2 == 0 {
                    MeshData::cube(size * (0.6 + 0.05 * (i % 5) as f32))
                } else {
                    MeshData::sphere(size * (0.35 + 0.03 * (i % 4) as f32), 10)
                };
                self.mesh(&data, material)
            })
            .collect()
    }

    fn build(&mut self) {
        let mut seq = Sequence(0x9E37_79B9_7F4A_7C15);
        match self.kind {
            SceneKind::SingleMaterial => {
                let material = self.material(MaterialDesc::default());
                let mesh = self.mesh(&MeshData::cube(1.0), material);
                for i in 0..100 {
                    self.ctx.world.spawn((
                        Transform3D::at(Vec3::new((i % 10) as f32 * 1.5 - 7.0, 0.5, (i / 10) as f32 * -1.5)),
                        Model::new(mesh),
                    ));
                }
                self.orbit = (Vec3::new(0.0, 0.0, -5.0), 18.0, 10.0);
            }
            SceneKind::StaticWorld => {
                let half = 200.0;
                self.ground(half * 2.0, Color::rgb(0.32, 0.4, 0.3));
                let meshes = self.simple_meshes(8, 1.0);
                let each = self.count(25_000, 16);
                for mesh in meshes {
                    let transforms: Vec<Mat4> = (0..each)
                        .map(|_| {
                            let scale = Vec3::new(seq.between(0.6, 2.0), seq.between(0.6, 4.0), seq.between(0.6, 2.0));
                            Mat4::from_scale_rotation_translation(
                                scale,
                                Quat::from_rotation_y(seq.around(HALF_TURN)),
                                Vec3::new(seq.around(half), scale.y * 0.3, seq.around(half)),
                            )
                        })
                        .collect();
                    self.batch(mesh, &transforms);
                }
                self.ctx.scene3d.lighting.shadow_distance = 80.0;
                self.orbit = (Vec3::ZERO, 60.0, 25.0);
            }
            SceneKind::DynamicWorld => {
                self.ground(200.0, Color::rgb(0.3, 0.33, 0.4));
                let meshes = self.simple_meshes(16, 1.0);
                let models = self.count(20_000, 16);
                let side = (models as f32).sqrt().ceil() as usize;
                for i in 0..models {
                    let origin = Vec3::new(
                        (i % side) as f32 * 1.2 - side as f32 * 0.6,
                        1.0,
                        (i / side) as f32 * 1.2 - side as f32 * 0.6,
                    );
                    self.mover(
                        meshes[i % meshes.len()],
                        Mover {
                            origin,
                            phase: seq.next() * TURN,
                            radius: seq.between(0.2, 0.8),
                            speed: seq.between(0.5, 2.0),
                            motion: Motion::Bob,
                        },
                    );
                }
                let reach = side as f32 * 0.6;
                self.orbit = (Vec3::ZERO, reach * 0.9 + 6.0, reach * 0.5 + 4.0);
            }
            SceneKind::MultiMaterial => {
                self.ground(200.0, Color::rgb(0.4, 0.38, 0.34));
                let textures: Vec<(Texture, Texture)> = (0..16)
                    .map(|i| (self.texture(i, false), self.texture(i, true)))
                    .collect();
                let kinds = self.count(512, 8);
                let meshes: Vec<Mesh> = (0..kinds)
                    .map(|i| {
                        let cutout = i % 5 == 4;
                        let (plain, holed) = textures[i % textures.len()];
                        let material = self.material(MaterialDesc {
                            color: Color::rgb(
                                0.5 + 0.5 * (i % 7) as f32 / 6.0,
                                0.5 + 0.5 * (i % 11) as f32 / 10.0,
                                0.5 + 0.5 * (i % 13) as f32 / 12.0,
                            ),
                            texture: (i % 4 != 3).then_some(if cutout { holed } else { plain }),
                            roughness: 0.15 + 0.8 * (i % 9) as f32 / 8.0,
                            metallic: if i % 6 == 0 { 1.0 } else { 0.0 },
                            emissive: if i % 17 == 0 {
                                Color::rgb(0.6, 0.3, 0.1)
                            } else {
                                Color::BLACK
                            },
                            blend: if cutout { Blend::Cutout } else { Blend::Opaque },
                            double_sided: i % 10 == 9,
                            ..Default::default()
                        });
                        let data = match i % 3 {
                            0 => MeshData::cube(0.5 + 0.02 * (i % 8) as f32),
                            1 => MeshData::sphere(0.3 + 0.01 * (i % 8) as f32, 12),
                            _ => MeshData::plane(0.7),
                        };
                        self.mesh(&data, material)
                    })
                    .collect();
                let models = self.count(6_000, 16);
                let side = (models as f32).sqrt().ceil() as usize;
                for i in 0..models {
                    let mesh = meshes[(i as u32).wrapping_mul(2_654_435_761) as usize % meshes.len()];
                    self.model(
                        mesh,
                        Transform3D::at(Vec3::new(
                            (i % side) as f32 * 1.1 - side as f32 * 0.55,
                            0.5 + (i % 3) as f32 * 0.4,
                            (i / side) as f32 * 1.1 - side as f32 * 0.55,
                        ))
                        .rotated(Quat::from_rotation_y(seq.around(HALF_TURN))),
                    );
                }
                let reach = side as f32 * 0.55;
                self.orbit = (Vec3::ZERO, reach * 0.9 + 5.0, reach * 0.5 + 4.0);
            }
            SceneKind::ShadowHeavy => {
                let half = 45.0;
                self.ground(half * 2.4, Color::rgb(0.45, 0.45, 0.45));
                let lighting = &mut self.ctx.scene3d.lighting;
                lighting.shadows = true;
                lighting.shadow_cascades = 4;
                lighting.shadow_resolution = 4096;
                lighting.shadow_refresh = 1;
                lighting.shadow_distance = 120.0;
                lighting.shadow_filter = ShadowFilter::Softest;
                lighting.light_shadow_views = 24;
                lighting.light_shadow_resolution = 1024;
                lighting.light_shadow_atlas = 4096;
                let meshes = self.simple_meshes(8, 1.0);
                let casters = self.count(4_000, 16);
                for i in 0..casters {
                    let origin = Vec3::new(seq.around(half), seq.between(0.5, 3.0), seq.around(half));
                    if i % 2 == 0 {
                        self.mover(
                            meshes[i % meshes.len()],
                            Mover {
                                origin,
                                phase: seq.next() * TURN,
                                radius: 0.5,
                                speed: 1.0,
                                motion: Motion::Bob,
                            },
                        );
                    } else {
                        self.model(meshes[i % meshes.len()], Transform3D::at(origin));
                    }
                }
                let pillar_material = self.material(MaterialDesc::default());
                let pillar = self.mesh(&MeshData::cube(1.0), pillar_material);
                let pillars: Vec<Mat4> = (0..self.count(6_000, 16))
                    .map(|_| {
                        let tall = seq.between(1.0, 6.0);
                        Mat4::from_scale_rotation_translation(
                            Vec3::new(0.3, tall, 0.3),
                            Quat::IDENTITY,
                            Vec3::new(seq.around(half), tall * 0.5, seq.around(half)),
                        )
                    })
                    .collect();
                self.batch(pillar, &pillars);
                for i in 0..3 {
                    let angle = i as f32 * 2.1;
                    self.point_light(
                        Vec3::new(angle.cos() * 12.0, 5.0, angle.sin() * 12.0),
                        Color::rgb(1.0, 0.8, 0.6),
                        30.0,
                        true,
                    );
                }
                for i in 0..6 {
                    let angle = i as f32 * 1.05;
                    self.spot_light(
                        Vec3::new(angle.cos() * 24.0, 12.0, angle.sin() * 24.0),
                        Color::rgb(0.7, 0.8, 1.0),
                        30.0,
                        true,
                    );
                }
                self.orbit = (Vec3::ZERO, 40.0, 18.0);
            }
            SceneKind::AlphaHeavy => {
                self.ground(120.0, Color::rgb(0.25, 0.27, 0.3));
                let meshes: Vec<Mesh> = (0..8)
                    .map(|i| {
                        let material = self.material(MaterialDesc {
                            color: Color::rgba(
                                0.3 + 0.7 * (i % 3) as f32 / 2.0,
                                0.4 + 0.5 * (i % 4) as f32 / 3.0,
                                0.9 - 0.5 * (i % 5) as f32 / 4.0,
                                0.25 + 0.05 * i as f32,
                            ),
                            blend: Blend::Transparent,
                            double_sided: true,
                            roughness: 0.2,
                            ..Default::default()
                        });
                        let data = match i % 3 {
                            0 => MeshData::plane(2.5),
                            1 => MeshData::cube(1.2),
                            _ => MeshData::sphere(0.8, 12),
                        };
                        self.mesh(&data, material)
                    })
                    .collect();
                let see_through = self.count(4_000, 16);
                for i in 0..see_through {
                    self.ctx.world.spawn((
                        Transform3D::at(Vec3::ZERO).rotated(Quat::from_rotation_x(seq.around(1.5))),
                        Model::new(meshes[i % meshes.len()]),
                        Mover {
                            origin: Vec3::new(seq.around(14.0), seq.between(0.8, 9.0), seq.around(14.0)),
                            phase: seq.next() * TURN,
                            radius: 0.6,
                            speed: seq.between(0.3, 1.2),
                            motion: Motion::Bob,
                        },
                    ));
                }
                self.content.models += see_through;
                self.content.moving_models += see_through;
                self.content.transparent_models += see_through;
                let holed = self.texture(3, true);
                let card_material = self.material(MaterialDesc {
                    texture: Some(holed),
                    blend: Blend::Cutout,
                    double_sided: true,
                    ..Default::default()
                });
                let card = self.mesh(&MeshData::plane(1.5), card_material);
                for _ in 0..self.count(1_000, 8) {
                    self.model(
                        card,
                        Transform3D::at(Vec3::new(seq.around(20.0), seq.between(0.5, 4.0), seq.around(20.0)))
                            .rotated(Quat::from_rotation_x(1.2) * Quat::from_rotation_z(seq.around(HALF_TURN))),
                    );
                }
                self.orbit = (Vec3::new(0.0, 4.0, 0.0), 26.0, 9.0);
            }
            SceneKind::Animated => {
                self.ground(160.0, Color::rgb(0.36, 0.36, 0.3));
                let figures: Vec<(Mesh, Skeleton)> = [
                    Color::rgb(0.85, 0.5, 0.35),
                    Color::rgb(0.4, 0.65, 0.85),
                    Color::rgb(0.6, 0.8, 0.45),
                ]
                .into_iter()
                .map(|color| self.figure(color))
                .collect();
                let count = self.count(400, 2);
                let side = (count as f32).sqrt().ceil() as usize;
                for i in 0..count {
                    let (mesh, skeleton) = &figures[i % figures.len()];
                    let at = Vec3::new(
                        (i % side) as f32 * 1.6 - side as f32 * 0.8,
                        0.0,
                        (i / side) as f32 * 1.6 - side as f32 * 0.8,
                    );
                    let (mesh, skeleton) = (*mesh, skeleton.clone());
                    self.walker(mesh, &skeleton, at, seq.next() * TURN);
                }
                let reach = side as f32 * 0.8;
                self.orbit = (Vec3::new(0.0, 1.0, 0.0), reach + 5.0, reach * 0.4 + 3.0);
            }
            SceneKind::GameLike => self.build_town(&mut seq),
        }
    }

    /// The game-like scene: see `SceneKind::GameLike`.
    fn build_town(&mut self, seq: &mut Sequence) {
        const HALF: f32 = 200.0;
        const GRID: usize = 129;
        // Rolling hills, flat in the middle where the town stands.
        let height = |x: f32, z: f32| {
            let away = ((x * x + z * z).sqrt() / HALF).min(1.0);
            ((x * 0.021).sin() * (z * 0.017).cos() * 9.0 + (x * 0.05 + z * 0.04).sin() * 2.0) * away * away
        };
        let heights: Vec<f32> = (0..GRID * GRID)
            .map(|i| {
                let (x, z) = ((i % GRID) as f32, (i / GRID) as f32);
                let step = HALF * 2.0 / (GRID - 1) as f32;
                height(x * step - HALF, z * step - HALF)
            })
            .collect();
        let grass = self.texture(5, false);
        let terrain_material = self.material(MaterialDesc {
            color: Color::rgb(0.55, 0.75, 0.45),
            texture: Some(grass),
            roughness: 0.95,
            ..Default::default()
        });
        let terrain = self.mesh(&MeshData::heightfield(&heights, HALF * 2.0, 1), terrain_material);
        self.model(terrain, Transform3D::at(Vec3::new(-HALF, 0.0, -HALF)));

        // Buildings: one box per block of a street grid, four materials.
        // Streets run between the blocks, `BLOCK` apart.
        const BLOCK: f32 = 14.0;
        let wall = self.texture(9, false);
        let blocks = (self.count(576, 16) as f32).sqrt().ceil() as i32;
        let town = blocks as f32 * BLOCK * 0.5;
        let mut lots: [Vec<Mat4>; 4] = Default::default();
        for i in 0..blocks * blocks {
            // One block in six stays empty: squares and parks.
            if seq.next() < 1.0 / 6.0 {
                continue;
            }
            let (x, z) = (
                (i % blocks) as f32 * BLOCK - town + BLOCK * 0.5,
                (i / blocks) as f32 * BLOCK - town + BLOCK * 0.5,
            );
            let size = Vec3::new(seq.between(6.0, 9.0), seq.between(4.0, 22.0), seq.between(6.0, 9.0));
            lots[i as usize % 4].push(Mat4::from_scale_rotation_translation(
                size,
                Quat::IDENTITY,
                Vec3::new(x, height(x, z) + size.y * 0.5, z),
            ));
        }
        for (kind, transforms) in lots.iter().enumerate() {
            let material = self.material(MaterialDesc {
                color: Color::rgb(0.75 + 0.08 * kind as f32, 0.78, 0.8 - 0.1 * kind as f32),
                texture: Some(wall),
                roughness: 0.8,
                ..Default::default()
            });
            let mesh = self.mesh(&MeshData::cube(1.0), material);
            self.batch(mesh, transforms);
        }
        // A point on a street: on a line between two rows of blocks.
        let street = |seq: &mut Sequence| -> (Vec3, bool) {
            let line = (seq.next() * (blocks + 1) as f32).floor() * BLOCK - town;
            let along = seq.around(town);
            let along_x = seq.next() < 0.5;
            let (x, z) = if along_x { (along, line) } else { (line, along) };
            (Vec3::new(x, height(x, z), z), along_x)
        };

        // Trees: a detailed crown near the camera, a coarse one far away.
        let leaves = self.material(MaterialDesc {
            color: Color::rgb(0.25, 0.5, 0.22),
            roughness: 0.9,
            ..Default::default()
        });
        let near = self.mesh(&MeshData::sphere(1.0, 16), leaves);
        let far = self.mesh(&MeshData::sphere(1.0, 6), leaves);
        let trees: Vec<Mat4> = (0..self.count(30_000, 16))
            .map(|_| {
                // Around the town, not in it.
                let (mut x, mut z) = (seq.around(HALF), seq.around(HALF));
                if x.abs() < town && z.abs() < town {
                    if x.abs() > z.abs() {
                        x = (town + seq.next() * (HALF - town).max(1.0)).copysign(x);
                    } else {
                        z = (town + seq.next() * (HALF - town).max(1.0)).copysign(z);
                    }
                }
                let size = seq.between(1.0, 2.6);
                Mat4::from_scale_rotation_translation(
                    Vec3::new(size, size * 1.4, size),
                    Quat::IDENTITY,
                    Vec3::new(x, height(x, z) + size, z),
                )
            })
            .collect();
        self.batch_levels(near, far, 60.0, &trees);

        // Vehicles driving up and down the streets.
        let vehicles = self.simple_meshes(6, 3.0);
        for i in 0..self.count(400, 8) {
            let (at, along_x) = street(seq);
            self.mover(
                vehicles[i % vehicles.len()],
                Mover {
                    origin: at + Vec3::Y * 1.0,
                    phase: seq.next() * TURN,
                    radius: seq.between(8.0, 40.0),
                    speed: seq.between(0.15, 0.5),
                    motion: Motion::Street { along_x },
                },
            );
        }

        // Walkers on the streets.
        let (figure, skeleton) = self.figure(Color::rgb(0.8, 0.6, 0.5));
        for _ in 0..self.count(80, 2) {
            let (at, _) = street(seq);
            self.walker(figure, &skeleton, at, seq.next() * TURN);
        }

        // Street lamps: the first two cast shadows, as do four spot lights.
        for i in 0..self.count(48, 2) {
            let (at, _) = street(seq);
            self.point_light(at + Vec3::Y * 4.0, Color::rgb(1.0, 0.85, 0.6), 14.0, i < 2);
        }
        for i in 0..self.count(8, 1) {
            let (at, _) = street(seq);
            self.spot_light(at + Vec3::Y * 9.0, Color::rgb(0.8, 0.9, 1.0), 20.0, i < 4);
        }

        // Glass panes and a lake.
        let glass = self.material(MaterialDesc {
            color: Color::rgba(0.6, 0.8, 0.9, 0.35),
            blend: Blend::Transparent,
            double_sided: true,
            roughness: 0.05,
            ..Default::default()
        });
        let pane = self.mesh(&MeshData::plane(2.0), glass);
        let panes = self.count(300, 4);
        for _ in 0..panes {
            self.model(
                pane,
                Transform3D::at(street(seq).0 + Vec3::Y * seq.between(1.0, 3.0)).rotated(
                    Quat::from_rotation_y(seq.around(HALF_TURN)) * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                ),
            );
        }
        let water = self.material(MaterialDesc {
            color: Color::rgba(0.15, 0.35, 0.5, 0.6),
            blend: Blend::Transparent,
            roughness: 0.05,
            ..Default::default()
        });
        let lake = self.mesh(&MeshData::plane(120.0), water);
        self.model(lake, Transform3D::at(Vec3::new(town + 80.0, -1.5, town + 60.0)));
        self.content.transparent_models += panes + 1;

        self.ctx.scene3d.post.bloom = 0.3;
        self.ctx.scene3d.camera.occlusion_culling = true;
        self.ctx.scene3d.lighting.shadow_distance = 100.0;
        self.orbit = (Vec3::new(0.0, 3.0, 0.0), town * 0.55 + 12.0, 26.0);
    }

    // -------------------------------------------------------------- frames

    /// Puts the camera where this frame sees the scene from: on a slow
    /// circle around it, so every frame's view differs a little.
    fn place_camera(&mut self) {
        let (center, distance, height) = self.orbit;
        let step = match self.shadow_workload.as_deref() {
            Some("static" | "actors") => 0.0,
            Some("slow-camera") => 0.00001,
            _ => 0.004,
        };
        let angle = 0.6 + self.frames as f32 * step;
        let camera = &mut self.ctx.scene3d.camera;
        camera.position = center + Vec3::new(angle.cos() * distance, height, angle.sin() * distance);
        camera.look_at(center);
        camera.far = (distance * 6.0).max(300.0);
    }

    /// Moves what the scene moves this frame.
    fn animate(&mut self) {
        let time = self.frames as f32 / 60.0;
        self.place_camera();
        if matches!(self.shadow_workload.as_deref(), Some("static" | "slow-camera")) {
            return;
        }
        if self.shadow_workload.as_deref() == Some("thrash") {
            self.ctx.scene3d.lighting.sun_direction = Vec3::new(time.sin(), -1.0, time.cos());
        }
        let mut movers = self.ctx.world.query::<(&mut Transform3D, &Mover)>();
        movers.for_each(|(transform, mover)| {
            let at = time * mover.speed + mover.phase;
            match mover.motion {
                Motion::Bob => {
                    transform.position = mover.origin + Vec3::Y * at.sin() * mover.radius;
                    transform.rotation = Quat::from_rotation_y(at);
                }
                Motion::Street { along_x } => {
                    let way = if along_x { Vec3::X } else { Vec3::Z };
                    transform.position = mover.origin + way * at.sin() * mover.radius;
                }
            }
        });
        let mut skeletons = self.ctx.world.query::<(&mut Skeleton, &Sway)>();
        skeletons.for_each(|(skeleton, sway)| {
            for (j, joint) in skeleton.pose.iter_mut().enumerate().skip(1) {
                joint.rotation = Quat::from_rotation_z((time * 2.0 + sway.phase + j as f32 * 0.4).sin() * 0.12);
            }
        });
    }

    /// What `Ctx::prepare_3d` does before handing the frame to the
    /// renderer: skeletons, target views, then everything the camera and
    /// the shadow maps see.
    fn collect(&mut self) -> SkyLight {
        let ctx = &mut self.ctx;
        let motion = ctx.scene3d.post.anti_aliasing == crate::render::AntiAliasing::Taa
            || (ctx.scene3d.post.ao_temporal && ctx.scene3d.post.ambient_occlusion.enabled);
        let mut skins = std::mem::take(&mut ctx.gfx.frame3d.skins);
        skins.motion = motion;
        skins.collect(&mut ctx.world, &ctx.gfx.assets3d);
        ctx.collect_views(&skins);
        let frame = &mut ctx.gfx.frame3d;
        frame.size_limit = self.device.limits().max_texture_dimension_2d;
        frame.lod = ctx.scene3d.lod;
        frame.motion.enabled = motion;
        if !ctx.scene3d.camera.occlusion_culling {
            frame.occlusion = None;
        } else if let Some(grid) = self.renderer.take_occlusion() {
            frame.occlusion = Some(grid);
        }
        frame.collect(
            &mut ctx.world,
            &ctx.gfx.assets3d,
            &skins,
            &ctx.scene3d.camera,
            &ctx.scene3d.lighting,
        );
        frame.skins = skins;
        if ctx.scene3d.sky.atmosphere {
            match frame.sky_light {
                Some((lighting, sky, light)) if lighting == ctx.scene3d.lighting && sky == ctx.scene3d.sky => light,
                _ => {
                    let light = SkyLight::new(&ctx.scene3d.lighting, &ctx.scene3d.sky);
                    frame.sky_light = Some((ctx.scene3d.lighting, ctx.scene3d.sky, light));
                    light
                }
            }
        } else {
            let rgb = |c| {
                let [r, g, b, _] = crate::three::linear(c);
                Vec3::new(r, g, b)
            };
            SkyLight {
                sun: rgb(ctx.scene3d.lighting.sun_color) * ctx.scene3d.lighting.sun_intensity,
                sky: rgb(ctx.scene3d.sky.sky_color),
                ground: rgb(ctx.scene3d.sky.ground_color),
            }
        }
    }

    /// Moves the scene, draws one frame, waits for the GPU and says what
    /// each step cost.
    pub fn frame(&mut self) -> SceneFrame {
        let started = Instant::now();
        self.animate();
        let updated = Instant::now();
        let light = self.collect();
        let collected = Instant::now();
        let ctx = &mut self.ctx;
        let scene = Scene3D {
            frame: &ctx.gfx.frame3d,
            lighting: &ctx.scene3d.lighting,
            sky: &ctx.scene3d.sky,
            light,
            camera: ctx.scene3d.camera.position,
            post: &ctx.scene3d.post,
            time: (self.frames as f32 / 60.0, 1.0 / 60.0),
            occlusion: ctx.scene3d.camera.occlusion_culling,
        };
        self.renderer.prepare(
            &self.device,
            &self.queue,
            &Prepare3D {
                scene: &scene,
                lut: None,
                size: self.setup.size,
            },
        );
        ctx.gfx.frame3d.motion.reset = false;
        let prepared = Instant::now();
        if let Some(timer) = &mut self.timer {
            timer.begin_frame();
        }
        let mut buffers = self.renderer.draw(
            &self.device,
            &self.target,
            wgpu::Color::BLACK,
            &self.textures,
            self.timer.as_ref(),
        );
        if let Some(timer) = &mut self.timer {
            let mut encoder = self.device.create_command_encoder(&Default::default());
            timer.resolve(&mut encoder);
            buffers.push(encoder.finish());
        }
        let command_buffers = buffers.len();
        let encoded = Instant::now();
        self.queue.submit(buffers);
        let submitted = Instant::now();
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
        self.renderer.submitted();
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let done = Instant::now();

        let (mut gpu_timed, mut gpu_ms, mut gpu_span_ms) = (false, None, None);
        if let Some(timer) = &mut self.timer {
            timer.collect(&self.device);
            if timer.frames_read != self.gpu_frames_read {
                self.gpu_frames_read = timer.frames_read;
                self.gpu_passes.clear();
                let (mut first, mut last) = (f64::MAX, 0.0f64);
                for span in &timer.last_frame {
                    let ms = f64::from(span.ms);
                    first = first.min(f64::from(span.start_ms));
                    last = last.max(f64::from(span.start_ms) + ms);
                    match self.gpu_passes.iter_mut().find(|(name, _)| *name == span.name) {
                        Some((_, total)) => *total += ms,
                        None => self.gpu_passes.push((span.name, ms)),
                    }
                }
                gpu_timed = true;
                gpu_ms = Some(self.gpu_passes.iter().map(|(_, ms)| ms).sum());
                gpu_span_ms = Some((last - first).max(0.0));
            }
        }
        let StateChanges {
            pipelines,
            bind_groups,
            vertex_buffers,
            index_buffers,
            draws,
        } = self.renderer.take_state_changes();
        let [(instances, triangles), (shadow_instances, shadow_triangles)] = self.renderer.counts();
        self.frames += 1;
        SceneFrame {
            update: updated - started,
            collect: collected - updated,
            prepare: prepared - collected,
            encode: encoded - prepared,
            submit: submitted - encoded,
            frame: done - updated,
            gpu_timed,
            gpu_ms,
            gpu_span_ms,
            draw_calls: self.renderer.draw_calls(),
            recorded_draws: draws,
            state_changes: [pipelines, bind_groups, vertex_buffers, index_buffers],
            instances,
            triangles,
            shadow_instances,
            shadow_triangles,
            occluded: self.renderer.occluded,
            pipelines_built: self.renderer.take_pipelines_built(),
            command_buffers,
            uploads: crate::render::traffic::take(),
        }
    }

    // ------------------------------------------------------------ reporting

    /// The latest timed frame's GPU passes: name and milliseconds, passes
    /// of one name added together. Empty until a frame was timed.
    #[must_use]
    pub fn gpu_passes(&self) -> &[(&'static str, f64)] {
        &self.gpu_passes
    }

    /// Why GPU pass times are missing, or `None` when the device measures them.
    #[must_use]
    pub fn gpu_timing_unavailable(&self) -> Option<String> {
        if self.timer.is_some() {
            return None;
        }
        Some(match self.capabilities.forced_tier {
            Some(tier) if tier != GpuTier::Desktop => {
                format!("PONKO_GPU_TIER forces the {tier} tier, which opens the device without timestamp queries")
            }
            _ => format!(
                "the adapter ({}, {}) has no timestamp queries",
                self.capabilities.adapter.name, self.capabilities.adapter.backend
            ),
        })
    }

    /// The adapter: name, backend, type and driver.
    #[must_use]
    pub fn adapter(&self) -> String {
        let a = &self.capabilities.adapter;
        let driver = format!("{} {}", a.driver, a.driver_info);
        let driver = if driver.trim().is_empty() {
            "unknown"
        } else {
            driver.trim()
        };
        format!("{} ({}, {}, driver {driver})", a.name, a.backend, a.device_type)
    }

    /// How the instance batches are routed in the frame last drawn.
    #[must_use]
    pub fn instance_batches(&self) -> InstanceBatchStats {
        self.renderer.instance_batch_stats()
    }

    /// The GPU and how the device was opened, as JSON: the adapter, backend
    /// and driver, the limits, every optional renderer feature with why it
    /// is missing and what stands in, the tier, the wgpu features asked
    /// for, and the target (size, format, samples asked for and used).
    #[must_use]
    pub fn gpu_json(&self) -> String {
        let driver_date = &self.info.driver_info;
        serde_json::json!({
            "capabilities": self.capabilities,
            "vendor_id": self.info.vendor,
            "device_id": self.info.device,
            "driver": if self.info.driver.is_empty() && driver_date.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(format!("{} {}", self.info.driver, driver_date).trim().to_owned())
            },
            "wgpu_features": format!("{:?}", self.features),
            "timestamp_queries": self.timer.is_some(),
            "timestamp_period_ns": self.timer.is_some().then(|| self.queue.get_timestamp_period()),
            "target": {
                "width": self.setup.size.0,
                "height": self.setup.size.1,
                "format": format!("{:?}", self.output.format()),
                "samples_requested": self.setup.samples,
                "samples": self.samples,
                "solid_depth_prepass": self.solid_prepass,
            },
        })
        .to_string()
    }

    /// The scene as JSON: its name, what it holds, and the quality
    /// settings its frames are drawn with.
    #[must_use]
    pub fn scene_json(&self) -> String {
        let (ctx, c) = (&self.ctx, &self.content);
        let (l, p) = (&ctx.scene3d.lighting, &ctx.scene3d.post);
        let level = |q: Option<GraphicsQuality>| q.map_or("custom".to_owned(), |q| format!("{q:?}").to_lowercase());
        serde_json::json!({
            "name": self.kind.name(),
            "shadow_workload": self.shadow_workload,
            "description": self.kind.description(),
            "scale": self.setup.scale,
            "content": {
                "models": c.models,
                "moving_models": c.moving_models,
                "skinned_models": c.skinned_models,
                "joints_per_skeleton": c.joints_per_skeleton,
                "transparent_models": c.transparent_models,
                "meshes": c.meshes,
                "materials": c.materials,
                "textures": c.textures,
                "static_batches": c.static_batches,
                "static_instances": c.static_instances,
                "point_lights": c.point_lights,
                "spot_lights": c.spot_lights,
                "shadowed_lights": c.shadowed_lights,
            },
            "quality": {
                "preset": level(Some(self.setup.quality)),
                "matched": self.setup.matched,
                "level": level(ctx.scene3d.quality()),
                "render_scale": p.render_scale,
                "sharpen": p.sharpen,
                "shadows": l.shadows,
                "shadow_distance": l.shadow_distance,
                "shadow_cascades": l.shadow_cascades,
                "shadow_resolution": l.shadow_resolution,
                "shadow_refresh": l.shadow_refresh,
                "shadow_filter": l.shadow_filter as u32,
                "light_shadow_views": l.light_shadow_views,
                "light_shadow_resolution": l.light_shadow_resolution,
                "light_shadow_atlas": l.light_shadow_atlas,
                "ambient_occlusion": p.ambient_occlusion.enabled,
                "ao_half_resolution": p.ao_half_resolution,
                "ao_temporal": p.ao_temporal,
                "ao_slices": p.ambient_occlusion.slices,
                "ao_steps": p.ambient_occlusion.steps,
                "ssr": p.ssr,
                "gi": l.gi.enabled,
                "anti_aliasing": format!("{:?}", p.anti_aliasing).to_lowercase(),
                "bloom": ctx.scene3d.post.bloom,
                "tonemap": format!("{:?}", ctx.scene3d.post.tonemap).to_lowercase(),
                "atmosphere": ctx.scene3d.sky.atmosphere,
                "occlusion_culling": ctx.scene3d.camera.occlusion_culling,
            },
        })
        .to_string()
    }

    /// GPU memory as far as it is counted, as JSON. Meshes, the game's GPU
    /// buffers and the instance batches' pools come from the renderer's
    /// own tables and the harness's textures from their sizes. The device's
    /// allocator totals are there where wgpu reports them. Render targets,
    /// shadow maps, history textures and the per-frame buffers are in no
    /// table: `uncounted` says so.
    #[must_use]
    pub fn memory_json(&self) -> String {
        let (meshes, mesh_bytes, gpu_buffers, gpu_buffer_bytes) = self.renderer.residency();
        let batches = self.renderer.instance_batch_stats();
        let allocator = self.device.generate_allocator_report();
        // From the sizes, formats and sample counts the renderer made them
        // with: the scene-sized color (multisampled), depth (multisampled),
        // HDR and scene-copy targets, plus the harness's output.
        let samples = u64::from(self.renderer.samples);
        let render_target_bytes = self.renderer.targets.as_ref().map_or(0, |t| {
            let pixels = u64::from(t.size.0) * u64::from(t.size.1);
            // Rgba16Float is 8 bytes a sample, Depth32Float 4.
            pixels * (8 * (2 + samples) + 4 * samples)
        }) + u64::from(self.setup.size.0) * u64::from(self.setup.size.1) * 4;
        let (side, layers) = self.renderer.shadows.dims;
        let shadow_map_bytes = u64::from(side) * u64::from(side) * u64::from(layers) * 4;
        let light_shadow_atlas_bytes = self
            .renderer
            .shadows
            .light_dims
            .map_or(0, |side| u64::from(side) * u64::from(side) * 4);
        serde_json::json!({
            "meshes": meshes,
            "mesh_bytes": mesh_bytes,
            "gpu_buffers": gpu_buffers,
            "gpu_buffer_bytes": gpu_buffer_bytes,
            "instance_batch_pool_bytes": batches.pool_bytes,
            "textures": self.textures.len(),
            "texture_bytes": self.texture_bytes,
            "counted_bytes": mesh_bytes + gpu_buffer_bytes + batches.pool_bytes + self.texture_bytes,
            "render_target_bytes": render_target_bytes,
            "shadow_map_bytes": shadow_map_bytes,
            "light_shadow_atlas_bytes": light_shadow_atlas_bytes,
            "computed_bytes": render_target_bytes + shadow_map_bytes + light_shadow_atlas_bytes,
            "computed": "render_target_bytes, shadow_map_bytes and light_shadow_atlas_bytes are computed from the sizes, formats and sample counts of the textures as last drawn with (0 before the first frame), not read from the device: the scene's color, depth, HDR and scene-copy targets and the output; the sun's cascades; the lights' atlas",
            "device_allocated_bytes": allocator.as_ref().map(|a| a.total_allocated_bytes),
            "device_reserved_bytes": allocator.as_ref().map(|a| a.total_reserved_bytes),
            "device_unavailable": allocator.is_none().then(|| format!(
                "wgpu reports no allocator totals on the {} backend",
                self.capabilities.adapter.backend
            )),
            "uncounted": "the effects' own targets (ambient occlusion, reflections, antialiasing, motion vectors, bloom and the post chain), history textures, the instance buffers of static batches culled on the CPU and the per-frame instance, uniform and light buffers are neither in the renderer's tables nor computed here",
        })
        .to_string()
    }

    /// The frame last drawn, as RGBA8 sRGB pixels, row by row from the top.
    #[must_use]
    pub fn pixels(&self) -> Vec<u8> {
        let (width, height) = self.setup.size;
        // Rows are padded to wgpu's copy alignment.
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene benchmark readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            self.output.size(),
        );
        self.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(view) = readback.get_mapped_range(..) else {
            return Vec::new();
        };
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for row in view.chunks(stride as usize) {
            rgba.extend_from_slice(&row[..(width * 4) as usize]);
        }
        rgba
    }
}

/// The harness on a real device: every scene draws something, reports its
/// steps and counts, and says why a metric is missing instead of leaving
/// it out. Runs on any adapter (a software one included) and on every
/// forced tier; nothing here looks at how long anything took.
#[cfg(test)]
mod render_test {
    use super::*;
    use crate::render::render_test_support as support;

    /// The test size: a small target and a few objects of each kind.
    fn small() -> SceneSetup {
        SceneSetup {
            size: (96, 64),
            scale: 0.004,
            ..Default::default()
        }
    }

    /// `kind` on a device opened through the shared helpers (so a missing
    /// adapter is logged or, under `PONKO_REQUIRE_GPU`, fails, and
    /// `PONKO_GPU_TIER` applies), or `None` without a GPU. `Some(Err)` when
    /// the scene refuses the setup.
    fn open(kind: SceneKind, setup: SceneSetup) -> Option<Result<SceneBench, String>> {
        let samples = std::cell::Cell::new(1);
        let gpu = support::gpu_with(|adapter| {
            // The descriptor asks for at least the 16 texture slots the 3D
            // shaders bind; an adapter with fewer can't open this device.
            let slots = adapter.limits().max_sampled_textures_per_shader_stage;
            if slots < 16 {
                return Err(format!(
                    "needs 16 sampled textures per shader stage, the adapter has {slots}"
                ));
            }
            samples.set(SceneBench::samples(adapter, &setup));
            Ok(SceneBench::descriptor(adapter))
        })?;
        Some(
            SceneBench::on_device(kind, setup, gpu.info, gpu.device, gpu.queue, samples.get()).map(|mut bench| {
                // Small shadow maps: the benchmark's 4096² cascades are slow
                // and large on a software adapter, and no test needs them.
                let lighting = &mut bench.ctx.scene3d.lighting;
                lighting.shadow_resolution = lighting.shadow_resolution.min(256);
                lighting.light_shadow_resolution = lighting.light_shadow_resolution.min(128);
                lighting.light_shadow_atlas = lighting.light_shadow_atlas.min(512);
                bench
            }),
        )
    }

    /// A scene small enough for a software adapter, or `None` without a GPU.
    fn tiny(kind: SceneKind) -> Option<SceneBench> {
        Some(open(kind, small())?.expect("the small setup fits any device"))
    }

    /// Draws three frames of `kind` and checks what every scene must report.
    fn draws(kind: SceneKind) -> Option<(SceneBench, SceneFrame)> {
        let mut bench = tiny(kind)?;
        let name = kind.name();
        let mut last = SceneFrame::default();
        let mut timed = 0;
        for _ in 0..3 {
            last = bench.frame();
            timed += usize::from(last.gpu_timed);
            assert_eq!(last.gpu_timed, last.gpu_ms.is_some(), "{name}: {last:?}");
        }
        assert!(
            last.instances > 0 && last.triangles > 0,
            "{name} draws models: {last:?}"
        );
        assert!(
            last.draw_calls > 0 && last.recorded_draws > 0,
            "{name} records draws: {last:?}"
        );
        assert!(last.command_buffers > 0, "{name} submits command buffers");
        let pixels = bench.pixels();
        assert_eq!(pixels.len(), 96 * 64 * 4);
        let first = &pixels[..4];
        assert!(
            pixels.chunks(4).any(|p| p != first),
            "{name} draws a picture, not one flat color"
        );
        // GPU times are there only when the device measures them, and
        // their absence has a reason. A device with timestamp queries may
        // still hand back nothing usable (some software and virtual GPUs
        // return zero or equal timestamps): that is logged, not a failure.
        match bench.gpu_timing_unavailable() {
            None if timed > 0 => {
                assert!(!bench.gpu_passes().is_empty(), "{name} names its GPU passes");
            }
            None => {
                eprintln!(
                    "render test [{name}]: {} has timestamp queries, but no frame's times came back; GPU pass times not checked",
                    bench.adapter()
                );
                assert!(bench.gpu_passes().is_empty());
            }
            Some(reason) => {
                assert!(timed == 0 && bench.gpu_passes().is_empty());
                assert!(reason.contains("timestamp queries"), "{reason}");
            }
        }
        Some((bench, last))
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("the harness writes valid JSON")
    }

    #[test]
    fn the_static_world_draws_its_batches_without_uploading_instances_again() {
        let Some((mut bench, _)) = draws(SceneKind::StaticWorld) else {
            return;
        };
        let stats = bench.instance_batches();
        assert_eq!(stats.batches, 8);
        assert_eq!(
            json(&bench.scene_json())["content"]["static_instances"],
            stats.instances
        );
        // A steady frame of a world that doesn't change sends no batch data.
        assert_eq!(bench.frame().uploads.static_batches, 0);
    }

    #[test]
    fn the_dynamic_world_uploads_its_moved_models_every_frame() {
        let Some((mut bench, frame)) = draws(SceneKind::DynamicWorld) else {
            return;
        };
        assert!(frame.uploads.instances > 0, "{frame:?}");
        assert!(bench.frame().uploads.instances > 0);
    }

    #[test]
    fn the_multi_material_scene_changes_state_between_its_materials() {
        let Some((bench, frame)) = draws(SceneKind::MultiMaterial) else {
            return;
        };
        let scene = json(&bench.scene_json());
        assert_eq!(scene["content"]["textures"], 32);
        assert!(scene["content"]["materials"].as_u64().unwrap() >= 8);
        assert!(frame.state_changes[1] > 1, "several bind groups are set: {frame:?}");
    }

    #[test]
    fn the_shadow_heavy_scene_draws_into_the_shadow_maps() {
        let Some((bench, frame)) = draws(SceneKind::ShadowHeavy) else {
            return;
        };
        assert!(frame.shadow_instances > 0 && frame.shadow_triangles > 0, "{frame:?}");
        let scene = json(&bench.scene_json());
        // The tests draw small maps (see `tiny`); the benchmark's 4096² ones
        // are checked in the committed baseline (`ponko_bench`).
        assert_eq!(scene["quality"]["shadow_resolution"], 256);
        assert_eq!(scene["quality"]["shadow_cascades"], 4);
        assert_eq!(scene["quality"]["shadow_refresh"], 1);
        assert_eq!(scene["quality"]["level"], "custom");
        let memory = json(&bench.memory_json());
        assert_eq!(memory["shadow_map_bytes"], 4 * 256 * 256 * 4);
        // The lights' atlas is a square of 4-byte texels; its side is the
        // renderer's choice for the views it has to fit.
        let atlas = memory["light_shadow_atlas_bytes"].as_u64().unwrap();
        let side = ((atlas / 4) as f64).sqrt() as u64;
        assert!(atlas > 0 && side * side * 4 == atlas, "{memory}");
        assert_eq!(scene["content"]["shadowed_lights"], 9);
    }

    #[test]
    fn the_alpha_heavy_scene_draws_see_through_models() {
        let Some((bench, _)) = draws(SceneKind::AlphaHeavy) else {
            return;
        };
        let scene = json(&bench.scene_json());
        assert!(scene["content"]["transparent_models"].as_u64().unwrap() >= 16);
    }

    #[test]
    fn the_animated_scene_poses_its_skeletons_every_frame() {
        let Some((mut bench, _)) = draws(SceneKind::Animated) else {
            return;
        };
        let scene = json(&bench.scene_json());
        assert!(scene["content"]["skinned_models"].as_u64().unwrap() >= 2);
        assert_eq!(scene["content"]["joints_per_skeleton"], 16);
        assert!(bench.frame().uploads.skins > 0, "posed joints reach the GPU");
    }

    #[test]
    fn the_game_like_scene_has_every_kind_of_content() {
        let Some((bench, frame)) = draws(SceneKind::GameLike) else {
            return;
        };
        let scene = json(&bench.scene_json());
        let content = &scene["content"];
        for key in [
            "models",
            "moving_models",
            "skinned_models",
            "transparent_models",
            "static_batches",
            "static_instances",
            "point_lights",
            "spot_lights",
            "shadowed_lights",
            "textures",
        ] {
            assert!(content[key].as_u64().unwrap() > 0, "{key} in {content}");
        }
        assert_eq!(scene["quality"]["occlusion_culling"], true);
        assert!(frame.shadow_instances > 0, "{frame:?}");
    }

    #[test]
    fn the_matched_setup_draws_only_what_another_engine_can_draw_too() {
        let setup = SceneSetup {
            matched: true,
            ..small()
        };
        let Some(bench) = open(SceneKind::MultiMaterial, setup) else {
            return;
        };
        let mut bench = bench.expect("the small setup fits any device");
        let frame = bench.frame();
        assert!(frame.draw_calls > 0, "{frame:?}");
        let quality = &json(&bench.scene_json())["quality"];
        assert_eq!(quality["matched"], true);
        assert_eq!(quality["atmosphere"], false);
        assert_eq!(quality["bloom"], 0.0);
        assert_eq!(quality["ambient_occlusion"], false);
        assert_eq!(quality["ssr"], 0.0);
        assert_eq!(quality["anti_aliasing"], "fxaa");
        assert_eq!(quality["shadow_refresh"], 1);
        // The plain run keeps the preset.
        let Some(plain) = tiny(SceneKind::MultiMaterial) else {
            return;
        };
        assert_eq!(json(&plain.scene_json())["quality"]["matched"], false);
        assert_eq!(json(&plain.scene_json())["quality"]["atmosphere"], true);
    }

    #[test]
    fn the_gpu_report_names_the_adapter_the_target_and_every_feature() {
        let Some(bench) = tiny(SceneKind::DynamicWorld) else {
            return;
        };
        let gpu = json(&bench.gpu_json());
        let capabilities = &gpu["capabilities"];
        assert!(!capabilities["adapter"]["name"].as_str().unwrap().is_empty());
        assert!(!capabilities["adapter"]["backend"].as_str().unwrap().is_empty());
        let features = capabilities["features"].as_array().unwrap();
        assert_eq!(features.len(), crate::render::GpuFeature::ALL.len());
        for feature in features {
            // A missing feature says why and what stands in.
            if feature["available"] == false {
                assert!(
                    feature["reason"].is_string() && feature["fallback"].is_string(),
                    "{feature}"
                );
            }
        }
        assert_eq!(gpu["target"]["width"], 96);
        assert_eq!(gpu["target"]["samples_requested"], 4);
        assert!(gpu["target"]["samples"].as_u64().unwrap() >= 1);
        assert_eq!(gpu["timestamp_queries"], bench.gpu_timing_unavailable().is_none());
        assert_eq!(
            gpu["timestamp_period_ns"].is_null(),
            bench.gpu_timing_unavailable().is_some()
        );
        if let Some(tier) = GpuTier::forced() {
            assert_eq!(capabilities["forced_tier"], serde_json::to_value(tier).unwrap());
        }
    }

    #[test]
    fn the_memory_report_counts_what_it_can_and_names_what_it_cannot() {
        let Some((bench, _)) = draws(SceneKind::MultiMaterial) else {
            return;
        };
        let memory = json(&bench.memory_json());
        assert!(memory["mesh_bytes"].as_u64().unwrap() > 0);
        assert_eq!(memory["textures"], 33);
        assert!(memory["texture_bytes"].as_u64().unwrap() > 32 * 64 * 64 * 4);
        // The device's own totals, or the reason there are none.
        assert_ne!(
            memory["device_allocated_bytes"].is_null(),
            memory["device_unavailable"].is_null()
        );
        // What the tables don't hold is computed from sizes and labelled so:
        // 96 x 64 at the device's sample count, plus the output.
        let samples = json(&bench.gpu_json())["target"]["samples"].as_u64().unwrap();
        assert_eq!(
            memory["render_target_bytes"],
            96 * 64 * (8 * (2 + samples) + 4 * samples) + 96 * 64 * 4
        );
        assert!(memory["shadow_map_bytes"].as_u64().unwrap() > 0);
        assert_eq!(
            memory["computed_bytes"].as_u64().unwrap(),
            memory["render_target_bytes"].as_u64().unwrap()
                + memory["shadow_map_bytes"].as_u64().unwrap()
                + memory["light_shadow_atlas_bytes"].as_u64().unwrap()
        );
        assert!(
            memory["computed"]
                .as_str()
                .unwrap()
                .contains("not read from the device")
        );
        assert!(memory["uncounted"].as_str().unwrap().contains("history textures"));
    }

    #[test]
    fn a_scene_is_the_same_every_time_it_is_built() {
        for kind in SceneKind::ALL {
            let (Some(mut a), Some(mut b)) = (tiny(kind), tiny(kind)) else {
                return;
            };
            let name = kind.name();
            assert_eq!(a.scene_json(), b.scene_json(), "{name}");
            // Occlusion culling drops what the GPU's depth from earlier
            // frames hides, and when that depth comes back is the device's
            // business: this compares what the scene itself hands over.
            a.ctx.scene3d.camera.occlusion_culling = false;
            b.ctx.scene3d.camera.occlusion_culling = false;
            let (x, y) = (a.frame(), b.frame());
            assert_eq!(
                (x.instances, x.triangles, x.shadow_instances, x.draw_calls),
                (y.instances, y.triangles, y.shadow_instances, y.draw_calls),
                "{name}"
            );
        }
    }

    #[test]
    fn a_target_larger_than_the_device_allows_is_refused_with_a_reason() {
        let setup = SceneSetup {
            size: (1 << 20, 64),
            ..Default::default()
        };
        let Some(refused) = open(SceneKind::StaticWorld, setup) else {
            return;
        };
        match refused {
            Ok(_) => panic!("no device has textures a million pixels wide"),
            Err(reason) => assert!(reason.contains("doesn't fit this device"), "{reason}"),
        }
    }

    #[test]
    fn scene_names_are_distinct_and_described() {
        let names: std::collections::HashSet<_> = SceneKind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(names.len(), SceneKind::ALL.len());
        assert!(SceneKind::ALL.iter().all(|k| !k.description().is_empty()));
    }

    #[test]
    fn the_placement_sequence_is_fixed() {
        let mut seq = Sequence(0x9E37_79B9_7F4A_7C15);
        // Each value is a 24-bit integer over 2^24: these are the first three.
        let first: Vec<u32> = (0..3).map(|_| (seq.next() * (1u32 << 24) as f32) as u32).collect();
        assert_eq!(first, [14_424_951, 6_615_278, 8_062_926]);
        assert!((0..1000).all(|_| (0.0..1.0).contains(&seq.next())));
        assert!((0..1000).all(|_| (-2.0..2.0).contains(&seq.around(2.0))));
        assert!((0..1000).all(|_| (3.0..5.0).contains(&seq.between(3.0, 5.0))));
        assert_eq!(mipped_bytes(4, 4), (16 + 4 + 1) * 4);
    }
}
