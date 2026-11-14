//! The 3D passes: instanced meshes with a depth buffer and 4x multisampling,
//! drawn before sprites. Made the first time a game creates a mesh or
//! material.
//!
//! The frame, in order:
//! 0. **sky texture**: with an atmosphere, the sky in every direction;
//! 1. **shadows**: one pass per cascade of the sun's shadow map;
//! 2. **depth prepass**: the nearest depth of every pixel, so the next pass
//!    lights each pixel once, and later passes can read the depth; GTAO and
//!    screen-space reflections (see `screen.rs`, `ssr.rs`) trace it here;
//! 3. **opaque**: lit models into an HDR (16-bit float) target, keeping only
//!    the fragments the prepass found nearest, then the sky behind them and
//!    the transparent models over them;
//! 4. **bloom**: bright light is blurred through a chain of ever smaller
//!    images and back up (skipped when bloom is off);
//! 5. **post**: exposure, bloom, color grading and tone mapping, onto the
//!    screen.
//!
//! With temporal antialiasing, the camera also jitters by a sub-pixel amount
//! each frame, the lit frame goes into its own image, models that moved
//! since the last frame are drawn again for their motion vectors (after the
//! depth prepass, shared with temporal AO), and the post pass starts by resolving the frame with the
//! history into the HDR target (see `taa.rs`).
//!
//! Sprites and UI are drawn on top afterwards, without tone mapping.
//!
//! # Who owns what
//!
//! [`Renderer3D`] is the frame's coordinator: it holds the subsystems
//! below, hands each what it needs (never itself), and decides the order.
//! Each subsystem owns its pipelines, GPU objects and state, in its own
//! file; the fields are private to it unless another one reads them.
//!
//! | Responsibility | Owner | File | Lives |
//! |---|---|---|---|
//! | Meshes, materials, texture arrays, GPU buffers | `Resources` | `resources.rs` | until the game frees them |
//! | Material shaders: composing, variants, pipelines, warming | `Shaders`, `PipelineCache` | `shaders.rs`, `variants.rs`, `pipeline_cache.rs` | until the shader is freed or replaced |
//! | The GPU's answers about game shaders | `PendingCheck` | `shader_checks.rs` | until answered |
//! | The game's render targets, target cameras' images, compute state | `RenderTargets` | `targets.rs` | until freed |
//! | The scene's images (depth, HDR, resolves) and frame bind groups | `Targets`, `FrameSources` | `targets.rs` | until the size or the passes change |
//! | The sun's shadow map, the lights' atlas, what each view keeps | `ShadowMaps`, `ShadowCache` | `shadows.rs`, `shadow_cache.rs` | the map: until resized; the cache: frame to frame |
//! | Static and instanced batches, GPU culling | `StaticBatch`, `Batching`, `Culling` | `instances.rs` | until the batch is freed |
//! | Pass preparation: the frame's steps, in order | `Renderer3D::prepare` | `prepare.rs`, `frame.rs` | one frame |
//! | This frame's draw lists | `FrameLists` | `lists.rs`, `draw_order.rs` | one frame (cleared, capacity kept) |
//! | The frame's description: each pass's steps, the resources they use, their formats, and the order | `FRAME_PASSES` | `passes.rs` | constant |
//! | Holding a drawn frame to that description, in debug builds | `FrameChecks` | `frame_checks.rs` | one frame |
//! | Pass encoding: the frame's parts, recorded in parallel | `Renderer3D::draw` | `record.rs` | one frame |
//! | Temporal history: the jitter index, last frame's matrices, cuts | `History` | `history.rs` | frame to frame, until a reset |
//! | Game effects: screen and target effects | `Effects` | `effects.rs` | pipelines: until freed; lists: one frame |
//! | The game's passes as declared reads and writes, checked for hazards | `CustomPasses` | `custom_passes.rs` | one frame (warnings: for good) |
//! | The post chain: bloom, tone mapping, antialiasing, scaling | `PostChain`, `PostTargets` | `post.rs` | pipelines: for good; images: with the scene's |
//! | Screen-space passes (temporal antialiasing, ambient occlusion, reflections) and which run this frame | `ScreenSpace` over `Taa`, `Ao`, `Ssr` | `screen_space.rs`, `../taa.rs`, `../screen.rs`, `../ssr.rs` | pipelines: made when first used; flags: one frame |
//! | Global illumination's probes, the sky, particles, skinning, occlusion culling | the renderer's own fields, each a type of its own | `../gi.rs`, `../environment.rs`, `../particles.rs`, `skinning.rs`, `../occlusion.rs` | made when first used |
//!
//! A frame crosses them in two calls. `prepare` (`&mut self`) uploads and
//! decides everything: after it nothing changes. `draw` (`&self`) only
//! records, so the frame's parts are encoded on several threads. What must
//! outlive the frame is in `History` and the passes' own histories, and is
//! dropped together when the frame's history is no longer valid
//! (`History::decide`).
//!
//! Adding a post effect touches `post.rs` and its declaration in
//! `passes.rs` alone (plus `frame_checks.rs` when it brings an image of
//! its own); its module documentation walks through one. The screen-space
//! passes are not that separate yet:
//! `ScreenSpace` holds their state, but `prepare` decides its flags and
//! `targets.rs` makes their images, so a new one of those still touches
//! pass preparation, the targets and `passes.rs`.

mod custom_passes;
mod depth_copy;
mod draw_order;
mod effects;
mod frame;
mod frame_checks;
mod history;
mod instances;
mod lists;
mod mesh_arena;
mod passes;
#[cfg(test)]
pub(crate) use frame_checks::{depth_after_opaque, last_features};
#[cfg(test)]
pub(crate) use passes::Features as FrameFeatures;
pub(in crate::render) use passes::mark as mark_step;
mod pipeline_cache;
mod post;
mod prepare;
mod record;
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "bench-internals")))]
mod record_bench;
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "bench-internals")))]
pub use record_bench::{RecordBench, RecordScene};
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "bench-internals")))]
mod lod_bench;
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "bench-internals")))]
pub use lod_bench::{LodBench, LodFrame, LodMemory, LodPath, LodScene};
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "bench-internals")))]
mod scene_bench;
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "bench-internals")))]
pub use scene_bench::{SceneBench, SceneFrame, SceneKind, SceneSetup};
mod resources;
mod screen_space;
mod shader_checks;
mod shaders;
mod shadow_cache;
mod shadows;
#[cfg(test)]
pub(in crate::render) mod sky_render_test;
mod targets;
mod variants;
mod vertex_motion;
use super::capabilities::GpuLimits;
use crate::render::traffic::{self, Counted, MAX_WRITES, MIRRORED, Mirror, NewBuffer, Synced, Upload};
pub(crate) use draw_order::StateChanges;
use effects::{EffectPass, Effects};
use frame::*;
use history::History;
#[cfg(test)]
pub(crate) use history::HistoryReset;
#[cfg(test)]
pub(crate) use instances::{BatchPath, Route};
use instances::{Batching, Culling, StaticBatch};
use lists::FrameLists;
use pipeline_cache::Features;
use post::{PostChain, PostTargets};
use record::*;
use resources::*;
use screen_space::ScreenSpace;
pub(crate) use shader_checks::LateShaderError;
use shader_checks::{Swap, end_scope, rejected};
pub(crate) use shaders::*;
use shadow_cache::ShadowCache;
use shadows::ShadowMaps;
use std::ops::Range;
use std::sync::atomic::Ordering;
pub(in crate::render) use targets::*;
pub(crate) use variants::{Instancing, Pass, PipelineRole, ShaderVariant, Sides};

use glam::{Mat4, Vec3};

use super::gpu::GpuTexture;
use super::lights::LightTextures;
use super::taa::{self, Taa};
use super::timer::GpuTimer;

use crate::render::Material;
use crate::three::gi::GI_SHADOW_SIZE;
use crate::three::motion::MovedInstance;
use crate::three::{
    FADED, GpuMaterialDesc, INSTANCE_BRIDGE, Instance3D, MAX_CASCADES, MAX_LIGHT_SHADOW_VIEWS, MAX_SHADOW_VIEWS,
    MeshData, linear, scaled_size, shadow_views,
};
use crate::three::{Frame3D, MaterialOverride, Scene3D};
use crate::three::{ShaderDiagnostic, ShaderKind};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The usage the sun's shadow map adds in tests, so they can read it back
/// (`Renderer3D::shadow_map_bytes`); otherwise none.
const SHADOW_READBACK: wgpu::TextureUsages = if cfg!(test) {
    wgpu::TextureUsages::COPY_SRC
} else {
    wgpu::TextureUsages::empty()
};
/// The lit frame before tone mapping: light brighter than white survives.
pub(super) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Samples per pixel: smooth edges.
/// Whether this GPU can multisample scene color, depth and velocity `samples`
/// times (nonstandard counts also require adapter-specific format features). 4 always works (WebGPU guarantees it).
pub(crate) fn multisamples(adapter: &wgpu::Adapter, samples: u32) -> bool {
    [DEPTH_FORMAT, HDR_FORMAT, taa::VELOCITY_FORMAT].iter().all(|&f| {
        adapter
            .get_texture_format_features(f)
            .flags
            .sample_count_supported(samples)
    })
}

/// Whether `backend` is the browser's WebGL2, which wgpu can't give
/// multisampled textures (`texStorage2DMultisample` is missing from WebGL2, and
/// glow's browser binding panics on it). The 3D frame then uses one sample.
pub(crate) fn browser_gl_has_no_multisampled_textures(backend: wgpu::Backend) -> bool {
    cfg!(target_arch = "wasm32") && backend == wgpu::Backend::Gl
}

#[path = "skinning.rs"]
mod skinning;

const SKY: &str = include_str!("sky.wgsl");
/// The sky texture: every direction around the camera.
const SKY_SIZE: (u32, u32) = (256, 128);

/// Whether `device` runs compute passes: WebGL2 has no compute shaders
/// or storage textures.
fn can_compute(device: &wgpu::Device) -> bool {
    GpuLimits::of(device).compute()
}

/// Most bloom steps: the smallest image is 1/128 of the screen.
const BLOOM_MIPS: u32 = 7;
/// How much each bloom step keeps of the wider glow below it.
const BLOOM_SCATTER: f64 = 0.7;

/// (mesh, first instance, count): one instanced draw of a mesh.
type Run = (u32, u32, u32);

/// What `Renderer3D::prepare` needs for a frame.
pub(crate) struct Prepare3D<'a> {
    pub scene: &'a Scene3D<'a>,
    /// The lookup table's texture and its size, when there is one.
    pub lut: Option<(u32, u32)>,
    pub size: (u32, u32),
}

pub(crate) struct Renderer3D {
    // ---- Made once, for the renderer's life ----
    /// Samples per pixel of the frame and target cameras (2 or 4).
    samples: u32,
    texture_layout: wgpu::BindGroupLayout,
    sky_module: wgpu::ShaderModule,
    sky_layout: wgpu::PipelineLayout,
    /// Whether solid surfaces go through the depth prepass too. GPUs that
    /// draw in tiles (Apple's) drop hidden solid fragments themselves, so
    /// there they skip it and write their depth while lit; only cut-out
    /// ones, which those GPUs can't drop early, are laid down first.
    solid_prepass: bool,
    /// Whether shaders read the scene depth as a float texture (the GL
    /// backend: see [`shaders::reads_depth_as_float`]).
    float_depth: bool,
    /// With `float_depth`: the copy of the scene depth that transparent
    /// surfaces read (see `depth_copy.rs`).
    depth_copy: Option<depth_copy::DepthCopy>,
    /// With `float_depth` and one sample: the sampler the scene depth is
    /// read through (see [`shaders::samples_scene_depth`]).
    scene_depth_sampler: Option<wgpu::Sampler>,
    scene_layout: wgpu::BindGroupLayout,
    /// The scene bind group of passes that can't see the scene.
    no_scene: wgpu::BindGroup,
    /// The global illumination atlas's stand-in while it's off.
    no_gi: wgpu::TextureView,
    dummy_depth: wgpu::TextureView,
    no_decal_view: wgpu::TextureView,
    /// The physical sky: made into `sky_view` each frame, then drawn behind
    /// everything. Only with an atmosphere.
    sky_view: wgpu::TextureView,
    sky_texture: wgpu::Texture,
    sky_lut: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    /// The compute passes' bind group and pipeline layouts; `None` where
    /// the GPU can't run compute shaders (WebGL2).
    compute_layouts: Option<(wgpu::BindGroupLayout, wgpu::PipelineLayout)>,

    // ---- Resources: live until the game frees them ----
    /// Meshes, materials, texture arrays and GPU buffers (see `resources.rs`).
    resources: Resources,
    /// Every material shader's pipelines (see `shaders.rs`).
    shaders: Shaders,
    /// The game's render targets, and what target cameras and compute
    /// passes made for them (see `targets.rs`).
    render_targets: RenderTargets,
    /// Compute pass pipelines, by compute id.
    compute_pipelines: Vec<Option<wgpu::ComputePipeline>>,
    /// Which of them write a GPU buffer.
    buffer_computes: Vec<bool>,
    pub(super) static_batches: Vec<Option<StaticBatch>>,
    static_shadowless: Vec<u32>,
    static_culling: Option<Culling>,
    /// The batches' path choice, draw order and per-frame scratch.
    pub(crate) batching: Batching,
    /// The game's shader pipelines in use before the GPU checked them (a
    /// browser's WebGPU answers later; see `shader_checks.rs`).
    shader_checks: Vec<shader_checks::PendingCheck>,
    /// How many times each game shader's pipelines were put in use, by
    /// kind and id: tells a late GPU rejection whether newer code runs.
    shader_versions: std::collections::HashMap<(ShaderKind, u32), u64>,
    /// Tests: treat every GPU check as one that answers later.
    #[cfg(test)]
    pub(crate) defer_checks: bool,

    // ---- Subsystems: each owns its pipelines, images and frame state ----
    /// The sun's shadow map, the lights' shadow atlas and the frame
    /// uniforms their passes draw with (see `shadows.rs`).
    shadows: ShadowMaps,
    /// Which shadow views keep what they hold (see [`ShadowCache`]).
    shadow_cache: ShadowCache,
    /// Bloom, tone mapping, antialiasing and scaling (see `post.rs`).
    post: PostChain,
    /// The game's screen and target effects (see `effects.rs`).
    effects: Effects,
    /// The hazard check over the game's passes (see `custom_passes.rs`).
    custom_passes: custom_passes::CustomPasses,
    /// Temporal antialiasing, ambient occlusion and reflections: their
    /// passes and whether each runs this frame (see `screen_space.rs`).
    screen: ScreenSpace,
    /// Global illumination: the probes (made when first used).
    gi: Option<super::gi::Gi>,
    environment: super::environment::Environment,
    lights: LightTextures,
    /// This frame's skinned models, drawn as mesh ids past `meshes`.
    skinning: skinning::Skinning,
    /// The depth grid occlusion culling tests against.
    occlusion: super::occlusion::Occlusion,
    particles: super::particles::Particles,

    // ---- Scene targets: replaced when the size or the passes change ----
    targets: Option<Targets>,
    frame_bind_group: wgpu::BindGroup,
    /// What frame bind groups are made of, for each target camera's own.
    frame_sources: FrameSources,
    /// The decal texture array the frame bind groups hold (`None`: the stand-in).
    decal_array: Option<u32>,

    // ---- This frame: written by `prepare`, read by `draw` ----
    /// This frame's draw lists (see `lists.rs`).
    lists: FrameLists,
    atmosphere: bool,
    /// How many models in view the frame culled as hidden.
    pub occluded: usize,
    /// This frame has more instances than the device's largest buffer
    /// holds (reported once): it isn't drawn.
    too_many: bool,
    /// Whether order-independent passes draw their runs sorted by state
    /// (on; off draws them in mesh order, for comparing).
    pub(crate) group_draws: bool,
    /// The model passes' state changes since the last `take_state_changes`.
    state_counters: draw_order::StateCounters,

    // ---- Kept from frame to frame ----
    /// What the temporal effects keep (see `history.rs`).
    history: History,
    frame_buffer: wgpu::Buffer,
    /// What `frame_buffer` holds (see `sync_frame`).
    frame_mirror: Mirror,
    instances: wgpu::Buffer,
    /// What `instances` holds, so each frame uploads only what changed.
    instance_mirror: Mirror,
    /// Capacity of `instances`, in instances.
    capacity: usize,
    /// Models that moved: their instances (see `MovedInstance`).
    /// Written a vec4 at a time: a model that only moved (and didn't turn)
    /// changes two of its record's nine, the rest are as the last frame's.
    moved: Synced,
}

impl Renderer3D {
    pub fn new(
        device: &wgpu::Device,
        texture_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        white: u32,
        white_array: &wgpu::Texture,
        samples: u32,
        solid_prepass: bool,
    ) -> Self {
        debug_assert_eq!(passes::check_passes(&passes::FRAME_PASSES), Ok(()));
        // The material shaders' bindings are seen by compute passes too
        // (they read the frame and their material), where there are any.
        let compute = can_compute(device);
        let decals = supports_decals(device);
        let float_depth = shaders::reads_depth_as_float(device);
        // Where the scene depth is sampled (the GL backend, one sample).
        let scene_depth_sampler = shaders::samples_scene_depth(float_depth, samples).then(|| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("3d scene depth"),
                ..Default::default()
            })
        });
        let all_stages = if compute {
            wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE
        } else {
            wgpu::ShaderStages::VERTEX_FRAGMENT
        };
        let uniform_entry = |visibility| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let gi = supports_gi(device);
        let frame_bindings = frame_entries(all_stages, false, decals, gi);
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d frame layout"),
            entries: &frame_bindings,
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d shadow layout"),
            entries: &frame_entries(all_stages, true, decals, gi),
        });
        let material_texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: all_stages,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let material_array = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: all_stages,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d material layout"),
            entries: &[
                uniform_entry(all_stages),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: all_stages,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                material_texture(2),
                material_texture(3),
                material_texture(4),
                material_texture(5),
                material_texture(6),
                material_texture(7),
                material_array(8),
                material_array(9),
                material_array(10),
            ],
        });
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d scene layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: all_stages,
                    ty: wgpu::BindingType::Texture {
                        // Filtered for the screen-space reflections (see `ssr.rs`).
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: all_stages,
                    ty: wgpu::BindingType::Texture {
                        sample_type: shaders::scene_depth_sample_type(float_depth),
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: samples > 1,
                    },
                    count: None,
                },
            ]
            .into_iter()
            .chain(scene_depth_sampler.as_ref().map(|_| wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: all_stages,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            }))
            .collect::<Vec<_>>(),
        });
        let storage_layout = |read_only| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("GPU buffer layout"),
                entries: &(0..if read_only { 2 } else { 1 })
                    .map(|binding| wgpu::BindGroupLayoutEntry {
                        binding,
                        visibility: if read_only {
                            wgpu::ShaderStages::VERTEX_FRAGMENT
                        } else {
                            wgpu::ShaderStages::COMPUTE
                        },
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    })
                    .collect::<Vec<_>>(),
            })
        };
        let storage_supported = GpuLimits::of(device).storage_buffers();
        let storage_read = storage_supported.then(|| storage_layout(true));
        let storage_dummy = storage_read.as_ref().map(|layout| {
            let buffer = device.new_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &(0..2)
                    .map(|binding| wgpu::BindGroupEntry {
                        binding,
                        resource: buffer.as_entire_binding(),
                    })
                    .collect::<Vec<_>>(),
            })
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d pipeline layout"),
            bind_group_layouts: &if let Some(s) = &storage_read {
                vec![
                    Some(&frame_layout),
                    Some(&material_layout),
                    Some(&scene_layout),
                    Some(s),
                ]
            } else {
                vec![Some(&frame_layout), Some(&material_layout), Some(&scene_layout)]
            },
            immediate_size: 0,
        });
        let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d shadow pipeline layout"),
            bind_group_layouts: &if let Some(s) = &storage_read {
                vec![
                    Some(&shadow_layout),
                    Some(&material_layout),
                    Some(&scene_layout),
                    Some(s),
                ]
            } else {
                vec![Some(&shadow_layout), Some(&material_layout), Some(&scene_layout)]
            },
            immediate_size: 0,
        });
        // Passes that can't see the scene bind a 1x1 stand-in (black, far).
        let (no_scene, dummy_depth) = {
            let tiny = |label, format, samples, usage| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: samples,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let color = tiny("3d no scene color", HDR_FORMAT, 1, wgpu::TextureUsages::TEXTURE_BINDING);
            let depth = tiny(
                "3d no scene depth",
                DEPTH_FORMAT,
                samples,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            (
                scene_bind_group(device, &scene_layout, &color, &depth, scene_depth_sampler.as_ref()),
                depth,
            )
        };
        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3d material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let shaders = Shaders::new(
            device,
            layout,
            shadow_pipeline_layout,
            Features {
                decals,
                gi,
                storage: storage_supported,
            },
            samples,
            float_depth,
        );
        let post = PostChain::new(device, texture_layout, format, white);
        let frame_buffer = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("3d frame"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | MIRRORED,
            mapped_at_creation: false,
        });
        let shadow_map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("3d shadow map"),
            // A stand-in until a frame casts shadows (see `set_shadow_map`).
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | SHADOW_READBACK,
            view_formats: &[],
        });
        let shadow_view = shadow_map.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let shadow_layers = (0..1)
            .map(|layer| {
                shadow_map.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("3d shadow cascade"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let shadow_stride = (size_of::<FrameUniform>() as u64).div_ceil(align) * align;
        let shadow_frames = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("3d shadow frames"),
            // And one for global illumination's shadow map (see `gi.rs`).
            size: shadow_stride * (MAX_SHADOW_VIEWS as u64 + 1),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | MIRRORED,
            mapped_at_creation: false,
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3d shadow sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let sky_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("3d sky"),
            size: wgpu::Extent3d {
                width: SKY_SIZE.0,
                height: SKY_SIZE.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let sky_view = sky_texture.create_view(&Default::default());
        let environment = super::environment::Environment::new(device, &sky_view);
        let sky_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3d sky"),
            // Packed regions bound their own filter footprints; the sky wraps
            // longitude explicitly inside its region (sample_sky).
            address_mode_u: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sky_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("3d sky"),
            source: wgpu::ShaderSource::Wgsl(
                for_samples(&format!("{MESH_PRELUDE}\n{SKY}"), samples, float_depth)
                    .into_owned()
                    .into(),
            ),
        });
        let sky_pipeline = |label, layout: &wgpu::PipelineLayout, entry_point, samples, depth| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &sky_module,
                    entry_point: Some("vs_far"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &sky_module,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(HDR_FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sky_lut = sky_pipeline(
            "3d sky texture",
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("3d sky texture"),
                bind_group_layouts: &[Some(&shadow_layout)],
                immediate_size: 0,
            }),
            "fs_lut",
            1,
            None,
        );
        // Behind everything: only where the depth is still the far plane.
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d sky"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let sky = sky_pipeline(
            "3d sky",
            &sky_layout,
            "fs_sky",
            samples,
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
        );
        let stand_in = |label, format, layers| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(layers),
                    ..Default::default()
                })
        };
        let no_light_shadow = stand_in("3d no light shadows", DEPTH_FORMAT, wgpu::TextureViewDimension::D2);
        let no_decal_view = stand_in(
            "3d no decal images",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureViewDimension::D2Array,
        );
        let no_gi = stand_in("3d no gi", HDR_FORMAT, wgpu::TextureViewDimension::D2);
        let frame_sources = FrameSources {
            gi,
            gi_view: no_gi.clone(),
            layout: frame_layout,
            shadow_view,
            light_shadow_view: no_light_shadow.clone(),
            shadow_sampler,
            sky_view: environment.atlas.create_view(&Default::default()),
            sky_sampler,
            decals,
            // Never sampled: without images every decal reads as flat color.
            decal_view: no_decal_view.clone(),
            decal_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("3d decals"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
        };
        let lights = LightTextures::new(device);
        let frame_bind_group = frame_sources.bind_group(device, &frame_buffer, &lights);
        // A compute pass reads its render target and writes a scratch copy.
        let compute_layouts = compute.then(|| {
            let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("3d compute layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: HDR_FORMAT,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
            });
            let compute_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("3d compute pipeline layout"),
                bind_group_layouts: &[
                    Some(&frame_sources.layout),
                    Some(&material_layout),
                    Some(&scene_layout),
                    Some(&compute_layout),
                ],
                immediate_size: 0,
            });
            (compute_layout, compute_pipeline_layout)
        });
        let storage_write = storage_supported.then(|| {
            let binding = storage_layout(false);
            let pipeline = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GPU buffer compute"),
                bind_group_layouts: &[
                    Some(&frame_sources.layout),
                    Some(&material_layout),
                    Some(&scene_layout),
                    Some(&binding),
                ],
                immediate_size: 0,
            });
            (binding, pipeline)
        });
        let no_shadow = stand_in("3d no shadow", DEPTH_FORMAT, wgpu::TextureViewDimension::D2Array);
        let no_sky = stand_in("3d no sky", HDR_FORMAT, wgpu::TextureViewDimension::D2);
        let no_sky_sampler = device.create_sampler(&Default::default());
        let no_shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let no_lights = LightTextures::new(device);
        let mut shadow_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_frames,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<FrameUniform>() as u64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&no_shadow),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&no_shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&no_sky),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&no_sky_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&no_lights.records_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&no_lights.indices_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&no_light_shadow),
            },
        ];
        if decals {
            shadow_entries.extend(decal_entries(&frame_sources.decal_view, &frame_sources.decal_sampler));
        }
        if gi {
            shadow_entries.push(wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&no_gi),
            });
        }
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d shadow"),
            layout: &shadow_layout,
            entries: &shadow_entries,
        });
        let capacity = 1024;
        Self {
            texture_layout: texture_layout.clone(),
            sky_module,
            sky_layout,
            samples,
            solid_prepass,
            float_depth,
            depth_copy: float_depth.then(|| depth_copy::DepthCopy::new(device, samples)),
            scene_depth_sampler,
            resources: Resources {
                meshes: Vec::new(),
                mesh_arena: Default::default(),
                materials: Vec::new(),
                material_layout,
                material_sampler,
                linear_views: true,
                white,
                arrays: Vec::new(),
                white_array: array_view(white_array),
                storage_read,
                storage_write,
                storage_buffers: Vec::new(),
                storage_dummy,
                past_copy: None,
            },
            shaders,
            scene_layout,
            no_scene,
            history: History::default(),
            post,
            effects: Effects::default(),
            custom_passes: Default::default(),
            screen: ScreenSpace::new(can_compute(device)),
            lists: FrameLists::default(),
            moved: Synced::new(
                device,
                "3d moved instances",
                wgpu::BufferUsages::VERTEX,
                16 * size_of::<MovedInstance>(),
                Upload::Instances,
                (16, 0, MAX_WRITES),
            ),
            gi: None,
            no_gi,
            dummy_depth,
            frame_buffer,
            sky_view,
            sky_texture,
            environment,
            sky_lut,
            sky,
            atmosphere: false,
            frame_bind_group,
            lights,
            frame_sources,
            decal_array: None,
            no_decal_view,
            render_targets: RenderTargets::default(),
            compute_pipelines: Vec::new(),
            static_batches: Vec::new(),
            static_shadowless: Vec::new(),
            static_culling: None,
            batching: Batching::default(),
            buffer_computes: Vec::new(),
            compute_layouts,
            skinning: skinning::Skinning::new(device, GpuLimits::of(device).gpu_skinning()),
            particles: super::particles::Particles::new(device, texture_layout, samples),
            occlusion: super::occlusion::Occlusion::new(device, compute),
            occluded: 0,
            shadows: ShadowMaps {
                frames: shadow_frames,
                mirrors: (0..=MAX_SHADOW_VIEWS).map(|_| Mirror::default()).collect(),
                stride: shadow_stride,
                bind_group: shadow_bind_group,
                layers: shadow_layers,
                dims: (1, 1),
                #[cfg(test)]
                map: shadow_map.clone(),
                light_dims: None,
                light_atlas: None,
                tile_clear: None,
                no_light_shadow,
            },
            instances: Self::instance_buffer(device, capacity),
            too_many: false,
            instance_mirror: Default::default(),
            frame_mirror: Mirror::default(),
            capacity,
            targets: None,
            shader_checks: Vec::new(),
            shader_versions: std::collections::HashMap::new(),
            #[cfg(test)]
            defer_checks: false,
            group_draws: true,
            state_counters: Default::default(),
            shadow_cache: ShadowCache::default(),
        }
    }

    /// Keeps render target `id` (`texture`, in `HDR_FORMAT`), for target
    /// cameras and compute passes.
    pub fn add_render_target(&mut self, id: u32, texture: &wgpu::Texture) {
        self.render_targets.add(id, texture);
    }

    /// Render target `id` was freed: target cameras and compute passes let
    /// go of it and of what they made for it.
    pub fn free_render_target(&mut self, id: u32) {
        self.render_targets.free(id);
    }

    /// Whether the scene depth is read as a float texture, and the frame's
    /// transparent surfaces read a copy of it (the GL backend; for tests).
    #[cfg(test)]
    pub(crate) fn copies_scene_depth(&self) -> (bool, bool) {
        let copy = self.targets.as_ref().is_some_and(|t| t.depth_copy.is_some());
        (self.float_depth, copy)
    }

    /// How many target effects run this frame (for tests).
    #[cfg(test)]
    pub(crate) fn target_effect_count(&self) -> usize {
        self.effects.target_frame.len()
    }

    /// How many scene compute passes run this frame, how many frames ran
    /// the hazard check over the game's passes, and this frame's hazards
    /// (for tests).
    #[cfg(test)]
    pub(crate) fn custom_pass_state(&self) -> (usize, usize, Vec<String>) {
        let passes = &self.custom_passes;
        (
            self.lists.scene_computes.len(),
            passes.checked,
            passes.hazards.iter().map(ToString::to_string).collect(),
        )
    }

    /// Compiles a compute pass from the game's WGSL; returns its id.
    pub fn create_compute(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        user: &str,
    ) -> Result<u32, Box<ShaderDiagnostic>> {
        let start = crate::perf::Instant::now();
        let made = self.compile_compute(device, name, user);
        self.shaders.cache.counters.compiled(start.elapsed());
        made
    }

    fn compile_compute(&mut self, device: &wgpu::Device, name: &str, user: &str) -> Result<u32, Box<ShaderDiagnostic>> {
        let source = compose_compute(user);
        super::diagnose(name, ShaderKind::Compute, COMPUTE_VARIANT, &source)?;
        let layouts = if defines(user, "update") {
            &self.resources.storage_write
        } else {
            &self.compute_layouts
        };
        if layouts.is_none() && defines(user, "update") {
            self.buffer_computes.push(true);
            self.compute_pipelines.push(None);
            return Ok(self.compute_pipelines.len() as u32 - 1);
        }
        let Some((_, layout)) = layouts else {
            return Err(super::without_location(
                name,
                ShaderKind::Compute,
                COMPUTE_VARIANT,
                "this GPU can't run compute passes (WebGL2 has no compute shaders)".to_string(),
            ));
        };
        let (pipeline, verdict) = self.checked_compute(
            device,
            layout,
            name,
            &for_samples(&source, self.samples, self.float_depth),
        )?;
        let id = self.compute_pipelines.len() as u32;
        self.installed(ShaderKind::Compute, id);
        if let Some(verdict) = verdict {
            let swap = Swap::Compute {
                new: pipeline.clone(),
                old: None,
            };
            self.wait_for_verdict(id, name, COMPUTE_VARIANT, swap, verdict);
        }
        self.buffer_computes.push(defines(user, "update"));
        self.compute_pipelines.push(Some(pipeline));
        Ok(id)
    }

    /// Builds a compute pass's pipeline from its checked `source` in a GPU
    /// error scope; the verdict comes with it if the GPU answers later.
    fn checked_compute(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        name: &str,
        source: &str,
    ) -> Result<(wgpu::ComputePipeline, Option<shader_checks::Verdict>), Box<ShaderDiagnostic>> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = build_compute(device, layout, name, source);
        match end_scope(scope, self.defer_checks()) {
            Ok(None) => Ok((pipeline, None)),
            Ok(Some(error)) => Err(rejected(name, ShaderKind::Compute, COMPUTE_VARIANT, &error)),
            Err(verdict) => Ok((pipeline, Some(verdict))),
        }
    }

    /// Recompiles compute pass `id` (hot reload). On an error the old one stays.
    pub fn replace_compute(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        name: &str,
        user: &str,
    ) -> Result<(), Box<ShaderDiagnostic>> {
        let start = crate::perf::Instant::now();
        let made = self.recompile_compute(device, id, name, user);
        self.shaders.cache.counters.compiled(start.elapsed());
        made
    }

    fn recompile_compute(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        name: &str,
        user: &str,
    ) -> Result<(), Box<ShaderDiagnostic>> {
        let source = compose_compute(user);
        super::diagnose(name, ShaderKind::Compute, COMPUTE_VARIANT, &source)?;
        let layouts = if defines(user, "update") {
            &self.resources.storage_write
        } else {
            &self.compute_layouts
        };
        let Some((_, layout)) = layouts else {
            return Ok(());
        };
        let (pipeline, verdict) = self.checked_compute(
            device,
            layout,
            name,
            &for_samples(&source, self.samples, self.float_depth),
        )?;
        let i = id as usize;
        if i >= self.compute_pipelines.len() {
            return Ok(());
        }
        let old = self.compute_pipelines[i].replace(pipeline.clone());
        let old_buffer = std::mem::replace(&mut self.buffer_computes[i], defines(user, "update"));
        self.installed(ShaderKind::Compute, id);
        if let Some(verdict) = verdict {
            let swap = Swap::Compute {
                new: pipeline,
                old: Some((old, old_buffer)),
            };
            self.wait_for_verdict(id, name, COMPUTE_VARIANT, swap, verdict);
        }
        Ok(())
    }

    fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("3d instances"),
            size: (capacity * size_of::<Instance3D>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST | MIRRORED,
            mapped_at_creation: false,
        })
    }

    /// Makes GPU buffer `id`: a freed one's slot, or the next new one.
    pub fn create_gpu_buffer(&mut self, device: &wgpu::Device, id: u32, len: u32, data: &[glam::Vec4]) {
        self.resources.create_gpu_buffer(device, id, len, data);
    }

    /// Frees GPU buffer `id`; passes and meshes using it do nothing from now on.
    pub fn free_gpu_buffer(&mut self, id: u32) {
        self.resources.free_gpu_buffer(id);
    }

    pub fn write_gpu_buffer(&self, queue: &wgpu::Queue, id: u32, offset: u32, data: &[glam::Vec4]) {
        self.resources.write_gpu_buffer(queue, id, offset, data);
    }

    /// Makes GPU buffer `id` keep the elements the last frame drew, for
    /// the motion pass, or stop (see `Graphics::set_gpu_buffer_motion`).
    pub fn set_gpu_buffer_motion(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: u32, keep: bool) {
        self.resources.set_gpu_buffer_motion(device, queue, id, keep);
    }

    pub fn set_mesh_gpu_buffer(&mut self, mesh: u32, b: u32, count: u32) {
        self.shadow_cache.invalidate();
        self.resources.set_mesh_gpu_buffer(mesh, b, count);
    }

    /// Keeps a texture array for materials; returns its id.
    pub fn add_texture_array(&mut self, texture: &wgpu::Texture) -> u32 {
        self.resources.add_texture_array(texture)
    }

    /// Binds again the materials that read texture `texture` or texture
    /// array `array`, which were freed (see `Resources::rebind_materials`).
    pub fn rebind_materials(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &[GpuTexture],
        texture: Option<u32>,
        array: Option<u32>,
    ) {
        if let Some(texture) = texture {
            self.texture_alpha_changed(texture, true);
        }
        self.resources.rebind_materials(device, queue, textures, texture, array);
    }

    /// Binds the materials that read texture `texture` again, keeping their
    /// maps on (see `Resources::texture_replaced`).
    pub fn texture_replaced(&mut self, device: &wgpu::Device, textures: &[GpuTexture], texture: u32) {
        self.resources.texture_replaced(device, textures, texture);
    }

    pub fn create_material(&mut self, device: &wgpu::Device, desc: &GpuMaterialDesc, textures: &[GpuTexture]) -> u32 {
        self.shadow_cache.invalidate();
        let shaders = self.shaders.sets.len() as u32;
        self.resources.create_material(device, desc, textures, shaders)
    }

    /// Changes material `id` in place: every model using it changes.
    pub fn update_material(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: u32,
        desc: &GpuMaterialDesc,
        textures: &[GpuTexture],
    ) -> bool {
        if (id as usize) < self.resources.materials.len() {
            self.shadow_cache.invalidate();
        }
        let shaders = self.shaders.sets.len() as u32;
        self.resources
            .update_material(device, queue, id, desc, textures, shaders)
    }

    /// Makes mesh `id`: a freed one's slot, or the next new one.
    pub fn create_mesh_at(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        data: &MeshData,
        parts: &[(Range<u32>, Material)],
        skinned: bool,
    ) {
        self.shadow_cache.invalidate();
        self.resources
            .create_mesh_at(device, id, data, parts, skinned, &mut self.skinning);
    }

    /// Meshes with GPU buffers and their bytes, then GPU buffers and
    /// theirs (`Ctx::asset_inventory`).
    pub fn residency(&self) -> (usize, u64, usize, u64) {
        self.resources.residency()
    }

    /// Frees mesh `id`'s GPU memory; it draws nothing from now on.
    pub fn free_mesh(&mut self, id: u32) {
        self.shadow_cache.invalidate();
        self.resources.free_mesh(id);
    }

    /// Compiles a material shader from the game's WGSL; returns its index.
    pub fn create_shader(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        user: &str,
    ) -> Result<u32, Box<ShaderDiagnostic>> {
        let (pipelines, verdict) = self
            .shaders
            .material_pipelines(device, name, user, None, self.defer_checks())?;
        let id = self.shaders.sets.len() as u32;
        self.installed(ShaderKind::Material, id);
        if let Some((variant, verdict)) = verdict {
            let swap = Swap::Material {
                new: pipelines.clone(),
                old: None,
            };
            self.wait_for_verdict(id, name, &variant, swap, verdict);
        }
        self.shaders.sets.push(pipelines);
        self.shadow_cache.shaders.push(ShadowCache::shader_traits(user));
        Ok(id)
    }

    /// Recompiles shader `id` (hot reload). On an error the old one stays.
    /// Only `id`'s pipelines change: its old set is dropped (unless another
    /// shader shares it), and every other shader keeps its own.
    pub fn replace_shader(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        name: &str,
        user: &str,
    ) -> Result<(), Box<ShaderDiagnostic>> {
        let (pipelines, verdict) =
            self.shaders
                .material_pipelines(device, name, user, Some(id), self.defer_checks())?;
        let old = std::mem::replace(&mut self.shaders.sets[id as usize], pipelines.clone());
        let old_traits = std::mem::replace(
            &mut self.shadow_cache.shaders[id as usize],
            ShadowCache::shader_traits(user),
        );
        for (material, m) in self.resources.materials.iter().enumerate() {
            if m.shader == id && (old_traits.vertex || self.shadow_cache.shaders[id as usize].vertex || m.cutout) {
                self.shadow_cache.material_changed(material as u32);
            }
        }
        self.installed(ShaderKind::Material, id);
        if let Some((variant, verdict)) = verdict {
            let swap = Swap::Material {
                new: pipelines,
                old: Some((old, old_traits)),
            };
            self.wait_for_verdict(id, name, &variant, swap, verdict);
        }
        self.history.content_changed = true;
        Ok(())
    }

    /// Material `id` was freed: it draws as the default material from now
    /// on, and lets go of its textures.
    pub fn free_material(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: u32, textures: &[GpuTexture]) {
        let default = GpuMaterialDesc::new(&crate::three::MaterialDesc::default());
        self.update_material(device, queue, id, &default, textures);
    }

    /// Material shader `id` (an index into `shaders`) was freed: its
    /// materials draw with the built-in shader from now on.
    pub fn free_shader(&mut self, id: u32) {
        if id == 0 {
            return;
        }
        let vertex = self.shadow_cache.shaders.get(id as usize).is_none_or(|s| s.vertex);
        for (index, material) in self.resources.materials.iter_mut().enumerate() {
            if material.shader == id {
                if vertex || material.cutout {
                    self.shadow_cache.material_changed(index as u32);
                }
                material.shader = 0;
            }
        }
        self.shaders.free(id);
    }

    /// Compute pass `id` was freed: its pipeline goes, and passes naming it
    /// are skipped.
    pub fn free_compute(&mut self, id: u32) {
        if let Some(pipeline) = self.compute_pipelines.get_mut(id as usize) {
            *pipeline = None;
        }
    }

    /// Texture array `id` was freed: it reads as the white one from now on.
    pub fn free_texture_array(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: u32, textures: &[GpuTexture]) {
        let Some(view) = self.resources.arrays.get_mut(id as usize) else {
            return;
        };
        *view = self.resources.white_array.clone();
        if self.decal_array == Some(id) {
            self.decal_array = None;
            self.frame_sources.decal_view = self.no_decal_view.clone();
            self.rebind_frames(device);
        }
        self.rebind_materials(device, queue, textures, None, Some(id));
    }

    /// Builds every material shader's pipeline variants that would
    /// otherwise be built the first time a frame draws them (crossfading
    /// level-of-detail instances, buffer-driven instances), and from now on
    /// builds them with each new shader. Building one takes several
    /// milliseconds, a visible hitch inside a frame.
    pub fn warm_pipelines(&mut self) {
        self.shaders.warm();
    }

    /// Whether every variant of shader `id` is built.
    #[cfg(test)]
    pub fn shader_warm(&self, id: u32) -> bool {
        let shader = &self.shaders.sets[id as usize];
        ShaderVariant::ALL.iter().all(|&v| shader.built(v))
    }

    /// The pipeline shader `id` draws `role` of `variant` with, building
    /// the variant if needed (tests check every documented pair builds).
    #[cfg(test)]
    pub fn shader_pipeline(
        &self,
        id: u32,
        variant: ShaderVariant,
        role: PipelineRole,
    ) -> Option<&wgpu::RenderPipeline> {
        self.shaders.sets[id as usize].lookup(variant, false).role(role)
    }

    /// How many pipeline variants frames built while drawing since the last
    /// call (each one a hitch; see [`Renderer3D::warm_pipelines`]).
    pub fn take_pipelines_built(&self) -> usize {
        self.shaders.cache.counters.frame_misses.swap(0, Ordering::Relaxed) as usize
    }

    /// The pipeline cache's counters and what it holds now.
    pub fn pipeline_stats(&self) -> crate::render::PipelineStats {
        self.shaders.cache.stats()
    }

    /// Panics unless every buffer the renderer writes only the changes of
    /// holds what its mirror says: the frame uniforms (every view's copy,
    /// shadow views' and probe faces'), the post, ambient occlusion,
    /// reflection, antialiasing, probe-update and environment settings,
    /// the instances, the motion records, particles, skinning, and the
    /// static batches' views and culling. Call after a frame was submitted.
    #[cfg(test)]
    pub(crate) fn check_mirrors(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.frame_mirror
            .check(device, queue, &self.frame_buffer, 0, "frame uniform");
        for (i, mirror) in self.shadows.mirrors.iter().enumerate() {
            let at = self.shadows.stride * i as u64;
            mirror.check(device, queue, &self.shadows.frames, at, "shadow view frame uniform");
        }
        for (_, target) in &self.render_targets.views {
            let buffer = &target.frame_buffer;
            target
                .frame_mirror
                .check(device, queue, buffer, 0, "target camera frame uniform");
        }
        self.post
            .mirror
            .check(device, queue, &self.post.buffer, 0, "post uniform");
        self.instance_mirror
            .check(device, queue, &self.instances, 0, "instances");
        self.moved.check(device, queue);
        self.particles.check_mirrors(device, queue);
        self.skinning.check_mirrors(device, queue);
        self.environment.check_mirrors(device, queue);
        if let Some(ao) = &self.screen.ao {
            ao.check_mirrors(device, queue);
        }
        if let Some(ssr) = &self.screen.ssr {
            ssr.check_mirrors(device, queue);
        }
        if let Some(taa) = &self.screen.taa {
            taa.check_mirrors(device, queue);
        }
        if let Some(gi) = &self.gi {
            gi.check_mirrors(device, queue);
        }
        self.check_batch_mirrors(device, queue);
    }

    /// What the last prepared frame hands the GPU, by name: its runs, the
    /// instances and uniforms as uploaded, and the shadow views it keeps.
    /// Two frames that should draw the same picture have the same inputs,
    /// so a test can tell a CPU-side difference from the GPU's.
    #[cfg(test)]
    pub(crate) fn frame_inputs(&self) -> Vec<(&'static str, Vec<u8>)> {
        let shadow_frames: Vec<u8> = self.shadows.mirrors.iter().flat_map(|m| m.bytes().to_vec()).collect();
        vec![
            ("runs", format!("{:?}", self.lists.runs).into_bytes()),
            ("shadow runs", format!("{:?}", self.lists.shadow_runs).into_bytes()),
            ("solid runs", format!("{:?}", self.lists.solid_runs).into_bytes()),
            ("blended runs", format!("{:?}", self.lists.blended_runs).into_bytes()),
            ("blended instances", bytemuck::cast_slice(&self.lists.blended).to_vec()),
            ("instances", self.instance_mirror.bytes().to_vec()),
            ("frame uniform", self.frame_mirror.bytes().to_vec()),
            ("shadow view uniforms", shadow_frames),
            ("post uniform", self.post.mirror.bytes().to_vec()),
            (
                "kept shadow views",
                format!("{:#b}", self.shadow_cache.kept).into_bytes(),
            ),
        ]
    }

    /// The sun's shadow map as the GPU holds it now: every cascade's depth,
    /// layer after layer (tests: what a kept shadow view still shows).
    #[cfg(test)]
    pub(crate) fn shadow_map_bytes(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        let map = &self.shadows.map;
        let (width, height, layers) = (map.width(), map.height(), map.depth_or_array_layers());
        let row = width * 4;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * height * layers),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: map,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            map.size(),
        );
        queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        readback.get_mapped_range(..).unwrap().to_vec()
    }

    /// Every material pipeline key built so far (tests).
    #[cfg(test)]
    pub(crate) fn built_pipeline_keys(&self) -> Vec<pipeline_cache::PipelineKey> {
        self.shaders.cache.built_keys()
    }

    /// Makes the next new mesh (tests make meshes this way).
    #[cfg(test)]
    pub fn create_mesh(
        &mut self,
        device: &wgpu::Device,
        data: &MeshData,
        parts: &[(Range<u32>, Material)],
        skinned: bool,
    ) -> u32 {
        let id = self.resources.meshes.len() as u32;
        self.create_mesh_at(device, id, data, parts, skinned);
        id
    }

    /// The newest depth grid read back for occlusion culling, if one came
    /// back since the last call.
    pub fn take_occlusion(&mut self) -> Option<crate::three::occlusion::OcclusionGrid> {
        self.occlusion.latest.take()
    }

    /// Starts reading this frame's occlusion depth back. Call after submitting.
    pub fn submitted(&mut self) {
        self.occlusion.submitted();
    }

    /// For a device without view formats (WebGL2): a sprite texture is read
    /// as it is, in its own (sRGB) format, in the material slots that
    /// otherwise read it linearly. Data textures (normal, ORM maps) aren't
    /// affected: they are remade as linear textures when a material first
    /// uses them (`Gpu::ensure_mips`). What is still read decoded there is the
    /// default white image (the same either way) and a texture that was first
    /// used as a color and then as data.
    pub(super) fn without_linear_views(mut self) -> Self {
        self.resources.linear_views = false;
        self
    }

    /// The size the scene is drawn at (test hook).
    #[cfg(test)]
    pub(super) fn scene_size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|t| t.size)
    }

    /// The sun's shadow map: width and layers (test hook).
    #[cfg(test)]
    pub(super) fn shadow_map(&self) -> (u32, u32) {
        self.shadows.dims
    }

    /// The point and spot lights' shadow atlas: its width, once made (test
    /// hook).
    #[cfg(test)]
    pub(super) fn light_shadow_map(&self) -> Option<u32> {
        self.shadows.light_dims
    }

    /// Whether the scaling pass has its target (test hook).
    #[cfg(test)]
    pub(super) fn scales_output(&self) -> bool {
        self.targets.as_ref().is_some_and(|t| t.post.scales_output())
    }

    /// Draw calls the frame's models take: two per mesh part (depth
    /// prepass and lit), one per mesh part in each shadow cascade, one per
    /// screen effect, and the post chain's own (see `PostChain::draw_calls`).
    pub fn draw_calls(&self) -> usize {
        let parts: usize = self
            .lists
            .runs
            .iter()
            .map(|&(mesh, _, _)| self.resources.meshes[self.base(mesh) as usize].parts.len())
            .sum();
        let view_parts: usize = self
            .lists
            .views
            .iter()
            .flat_map(|(_, runs, _)| runs)
            .map(|&(mesh, _, _)| self.resources.meshes[self.base(mesh) as usize].parts.len())
            .sum();
        self.lists.computes.len()
            + (parts + view_parts) * 2
            + self.lists.views.len() * usize::from(self.atmosphere)
            + self.particles.draw_count()
            + self.effects.frame.len()
            + self.effects.target_frame.len()
            + usize::from(self.screen.ao_enabled) * 2
            + usize::from(self.screen.ssr_on) * 2
            + usize::from(self.screen.taa_on)
            + if self.screen.velocity_on {
                // The motion pass (one draw per moved part).
                self.lists
                    .moved_runs
                    .iter()
                    .map(|&(mesh, _, _)| self.resources.meshes[self.base(mesh) as usize].parts.len())
                    .sum::<usize>()
            } else {
                0
            }
            + if self.screen.still_motion {
                self.still_motion_parts()
            } else {
                0
            }
            + if self.atmosphere { 2 } else { 0 }
            + shadow_views(self.lists.cascades, self.lists.light_views)
                .map(|v| self.lists.shadow_runs[v].len())
                .sum::<usize>()
            + self.post.draw_calls(self.targets.as_ref().map(|t| &t.post))
    }

    /// The camera's mesh parts the motion pass draws although they hold
    /// still: their shader's `vertex` moves them (see `vertex_motion.rs`).
    fn still_motion_parts(&self) -> usize {
        let mut parts = 0;
        for &(run_mesh, _, _) in &self.lists.runs {
            let run_mesh = run_mesh & !FADED;
            let mesh = &self.resources.meshes[self.base(run_mesh) as usize];
            let kept = mesh.gpu_buffer.is_some_and(|(b, _)| {
                self.resources
                    .storage_buffers
                    .get(b as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(resources::StorageBuffer::keeps_past)
            });
            for (part, &(_, own)) in mesh.parts.iter().enumerate() {
                let m = &self.resources.materials[self.part_material(run_mesh, part, own) as usize];
                parts +=
                    usize::from(!m.transparent && record::still_moves(&self.shaders.sets[m.shader as usize], kept));
            }
        }
        parts
    }

    /// (instances, triangles) the camera draws, then the same for the shadow maps.
    pub fn counts(&self) -> [(usize, usize); 2] {
        let count = |runs: &[(u32, u32, u32)]| {
            runs.iter().fold((0, 0), |(n, tris), &(mesh, _, k)| {
                let per: u32 = self.resources.meshes[self.base(mesh) as usize]
                    .parts
                    .iter()
                    .map(|(range, _)| range.len() as u32 / 3)
                    .sum();
                (n + k as usize, tris + per as usize * k as usize)
            })
        };
        let shadows = shadow_views(self.lists.cascades, self.lists.light_views)
            .map(|v| count(&self.lists.shadow_runs[v]))
            .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        [count(&self.lists.runs), shadows]
    }

    /// Per mesh: (mesh, camera models, camera triangles, shadow models,
    /// shadow triangles), for meshes drawn this frame, in mesh order.
    pub fn mesh_costs(&self) -> Vec<(u32, usize, usize, usize, usize)> {
        let mut out: Vec<(u32, usize, usize, usize, usize)> = Vec::new();
        let tris = |mesh: u32| -> usize {
            self.resources.meshes[self.base(mesh) as usize]
                .parts
                .iter()
                .map(|(range, _)| range.len() / 3)
                .sum()
        };
        let mut add = |mesh: u32, count: u32, shadow: bool| {
            // A material override counts towards its mesh.
            let mesh = self.material_override(mesh).map_or(mesh & !FADED, |o| o.mesh);
            let at = match out.binary_search_by_key(&mesh, |e| e.0) {
                Ok(i) => i,
                Err(i) => {
                    out.insert(i, (mesh, 0, 0, 0, 0));
                    i
                }
            };
            let (n, t) = (count as usize, tris(mesh) * count as usize);
            if shadow {
                out[at].3 += n;
                out[at].4 += t;
            } else {
                out[at].1 += n;
                out[at].2 += t;
            }
        };
        for &(mesh, _, count) in &self.lists.runs {
            add(mesh, count, false);
        }
        for view in shadow_views(self.lists.cascades, self.lists.light_views) {
            for &(mesh, _, count) in &self.lists.shadow_runs[view] {
                add(mesh, count, true);
            }
        }
        out
    }

    /// Whether this frame has anything to draw.
    pub fn has_draws(&self) -> bool {
        self.static_batches.iter().any(Option::is_some)
            || !self.lists.runs.is_empty()
            || self.particles.has_draws()
            || !self.lists.computes.is_empty()
            || !self.lists.scene_computes.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn assert_target_configuration(&self, size: (u32, u32), samples: u32) {
        let targets = self.targets.as_ref().expect("drawn scene targets");
        assert_eq!(targets.size, size);
        assert_eq!(targets.samples, samples);
        assert_eq!(targets.depth.texture().sample_count(), samples);
        assert_eq!(targets.color.texture().sample_count(), samples);
        for (_, view) in &self.render_targets.views {
            assert_eq!(view.color.texture().sample_count(), samples);
            assert_eq!(view.depth.texture().sample_count(), samples);
        }
    }

    /// The motion vector image of the last frame (for tests).
    #[cfg(test)]
    pub(crate) fn velocity_texture(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref()?.velocity.as_ref().map(|t| &t.velocity_texture)
    }

    /// Why the last prepared frame dropped the temporal history (for tests).
    #[cfg(test)]
    pub(crate) fn history_reset(&self) -> HistoryReset {
        self.history.reset
    }

    /// Whether this frame is resolved by temporal antialiasing (for tests).
    #[cfg(test)]
    pub(crate) fn taa_active(&self) -> bool {
        self.screen.taa_on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_uniform_matches_shader_layout() {
        let module = wgpu::naga::front::wgsl::parse_str(&compose_shader("")).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Frame"))
            .unwrap();
        let wgpu::naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("Frame must be a struct");
        };
        assert_eq!(*span as usize, size_of::<FrameUniform>());
        for (name, offset) in [
            ("fog_far", std::mem::offset_of!(FrameUniform, fog_far)),
            ("probes", std::mem::offset_of!(FrameUniform, probes)),
            ("gi", std::mem::offset_of!(FrameUniform, gi)),
            (
                "unjittered_view_proj",
                std::mem::offset_of!(FrameUniform, unjittered_view_proj),
            ),
            (
                "previous_view_proj",
                std::mem::offset_of!(FrameUniform, previous_view_proj),
            ),
            ("jitter", std::mem::offset_of!(FrameUniform, jitter)),
            ("cluster_depth", std::mem::offset_of!(FrameUniform, cluster_depth)),
            (
                "cluster_view_proj",
                std::mem::offset_of!(FrameUniform, cluster_view_proj),
            ),
        ] {
            assert_eq!(
                members.iter().find(|m| m.name.as_deref() == Some(name)).unwrap().offset as usize,
                offset,
                "{name}"
            );
        }
    }

    #[test]
    fn custom_materials_can_evaluate_punctual_lighting() {
        check_shader(
            "punctual",
            &compose_shader(
                "fn surface(s: Surface) -> Surface { var o = s; o.emissive = punctual_light(s); return o; }",
            ),
        )
        .unwrap();
    }

    #[test]
    fn finds_defined_functions() {
        assert!(defines("fn vertex(v: Vertex) -> Vertex { return v; }", "vertex"));
        assert!(defines("fn  surface (s: Surface) -> Surface", "surface"));
        assert!(!defines("fn vertex_helper() {}", "vertex"));
        assert!(!defines("fn myvertex() {}", "vertex"));
        assert!(!defines("let fnvertex = 1;", "vertex"));
    }

    #[test]
    fn the_built_in_shader_is_valid() {
        check_shader("built-in", &compose_shader("")).unwrap();
    }

    #[test]
    fn compute_passes_compose_into_valid_shaders() {
        let blur = "fn compute(c: Cell) -> vec4<f32> {
            let around = cell_at(c.pixel + vec2<i32>(1, 0)) + cell_at(c.pixel - vec2<i32>(1, 0));
            return mix(c.value, around * 0.5, material.custom[0].x * frame.time.y);
        }";
        check_shader("blur", &compose_compute(blur)).unwrap();
        let err = check_shader(
            "bad.wgsl",
            &compose_compute("fn compute(c: Cell) -> f32 { return 1.0; }"),
        )
        .unwrap_err();
        assert!(err.contains("bad.wgsl"), "{err}");
    }

    #[test]
    fn every_built_in_shader_is_valid() {
        // Compiled only on a GPU, so naga checks them here (a reserved word
        // once crashed every textured material at startup).
        check_shader("mipmaps", super::super::mips::SHADER).unwrap();
        check_shader("post", include_str!("post.wgsl")).unwrap();
        check_shader("sprite", include_str!("sprite.wgsl")).unwrap();
    }

    #[test]
    fn the_sky_shader_is_valid() {
        check_shader("sky", &format!("{MESH_PRELUDE}\n{SKY}")).unwrap();
    }

    #[test]
    fn the_environment_shader_is_valid() {
        check_shader("environment", super::super::environment::SHADER).unwrap();
    }

    #[test]
    fn a_shader_with_both_hooks_is_valid() {
        let user = "fn vertex(v: Vertex) -> Vertex { var o = v; o.position.y += sin(frame.time.x); return o; }\n\
                    fn surface(s: Surface) -> Surface { var o = s; o.albedo *= noise2(s.uv * 8.0); return o; }";
        check_shader("both", &compose_shader(user)).unwrap();
    }

    #[test]
    fn shadow_passes_bind_everything_a_material_shader_reads() {
        let user = "fn surface(s: Surface) -> Surface { var o = s; o.emissive = sky_radiance(s.normal) * sunlight(s.world, s.normal); return o; }";
        let module = wgpu::naga::front::wgsl::parse_str(&compose_shader(user)).unwrap();
        let shadow: Vec<u32> = frame_entries(wgpu::ShaderStages::VERTEX_FRAGMENT, true, true, true)
            .iter()
            .map(|e| e.binding)
            .collect();
        let frame: Vec<u32> = frame_entries(wgpu::ShaderStages::VERTEX_FRAGMENT, false, true, true)
            .iter()
            .map(|e| e.binding)
            .collect();
        for (_, g) in module.global_variables.iter() {
            if let Some(b) = &g.binding
                && b.group == 0
            {
                assert!(
                    shadow.contains(&b.binding),
                    "group 0 binding {} is not in the shadow passes",
                    b.binding
                );
                assert!(
                    frame.contains(&b.binding),
                    "group 0 binding {} is not in the frame",
                    b.binding
                );
            }
        }
    }

    #[test]
    fn shaders_read_the_second_uv_set() {
        let user = "fn vertex(v: Vertex) -> Vertex { var o = v; o.position.y += v.uv1.x; return o; }\n\
                    fn surface(s: Surface) -> Surface { var o = s; o.albedo *= s.uv1.y; return o; }";
        check_shader("uv1", &compose_shader(user)).unwrap();
    }

    #[test]
    fn shaders_read_the_model_origin() {
        let user = "fn vertex(v: Vertex) -> Vertex { var o = v; o.position.x += sin(v.origin.x); return o; }\n\
                    fn surface(s: Surface) -> Surface { var o = s; o.albedo *= fract(s.origin.x * 0.37); return o; }";
        check_shader("origin", &compose_shader(user)).unwrap();
    }

    #[test]
    fn shader_errors_name_the_file() {
        let e = check_shader(
            "water.wgsl",
            &compose_shader("fn surface(s: Surface) -> Surface { return 1; }"),
        )
        .unwrap_err();
        assert!(e.contains("water.wgsl"), "{e}");
    }
}

#[cfg(test)]
#[path = "material_render_test.rs"]
mod material_render_test;

#[cfg(test)]
#[path = "gpu3d/webgl2_shaders_test.rs"]
mod webgl2_shaders_test;

#[cfg(test)]
#[path = "capabilities_render_test.rs"]
mod capabilities_render_test;
#[cfg(test)]
#[path = "post_links_render_test.rs"]
mod post_links_render_test;
#[cfg(test)]
#[path = "shader_reload_render_test.rs"]
mod shader_reload_render_test;
#[cfg(test)]
#[path = "skin_motion_render_test.rs"]
mod skin_motion_render_test;
#[cfg(test)]
#[path = "vertex_motion_render_test.rs"]
mod vertex_motion_render_test;
