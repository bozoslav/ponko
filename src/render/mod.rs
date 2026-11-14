//! Rendering: GPU capabilities and frame statistics, and with the `3d`
//! feature the home of the 3D rendering types: meshes, materials, models,
//! shaders and effects, levels of detail, instancing and render settings.
//! [`d3`](crate::d3) re-exports them for `use ponko::d3::*;`.

// The GPU side (wgpu setup, textures, the instanced sprite pass) lives in
// `gpu`, behind the `window` feature; without it, `none` stands in.

pub(crate) mod batch;
#[cfg(all(feature = "window", feature = "compressed-textures"))]
pub(crate) mod blocks;
pub(crate) mod capabilities;
#[cfg(all(feature = "window", feature = "3d"))]
mod gi;
#[cfg(feature = "window")]
mod gpu;
#[cfg(all(feature = "window", feature = "3d"))]
mod gpu3d;
#[cfg(all(feature = "window", feature = "3d"))]
mod lights;
#[cfg(feature = "window")]
mod lights2d;
#[cfg(all(
    feature = "window",
    not(target_arch = "wasm32"),
    any(test, feature = "bench-internals")
))]
mod lights2d_bench;
#[cfg(all(feature = "window", feature = "3d"))]
mod mips;
#[cfg(not(feature = "window"))]
mod none;
#[cfg(all(feature = "window", feature = "3d"))]
mod occlusion;
#[cfg(all(feature = "window", feature = "3d"))]
mod particles;
mod pipeline_stats;
#[cfg(all(feature = "window", feature = "3d"))]
mod screen;
#[cfg(feature = "window")]
mod shader_diagnostics;
mod shader_status;
#[cfg(all(feature = "window", feature = "3d"))]
mod smaa;
#[cfg(feature = "window")]
mod sprite_pipelines;
#[cfg(all(feature = "window", feature = "3d"))]
mod ssr;
#[cfg(feature = "window")]
pub(crate) mod surface_lifecycle;
#[cfg(all(feature = "window", feature = "3d"))]
mod taa;
#[cfg(feature = "window")]
mod timer;
#[cfg(feature = "window")]
pub(crate) mod traffic;

pub use crate::sprite_material::{BlendMode, SpriteMaterial, SpriteMaterialDesc, SpriteShader};
#[cfg(feature = "3d")]
pub use crate::three::{QualityAutotune, QualityAutotuneReason, QualityAutotuneStatus, QualityChange};
pub use capabilities::{GpuAdapter, GpuBatchLimits, GpuCapabilities, GpuFeature, GpuFeatureStatus, GpuLimits, GpuTier};
#[cfg(feature = "window")]
pub(crate) use gpu::Renderer;
#[cfg(all(feature = "window", feature = "3d"))]
pub(crate) use gpu3d::{
    COMPUTE_VARIANT, EFFECT_VARIANT, FULL_MATERIAL_VARIANT, compose_compute, compose_effect, compose_shader,
};
/// Unstable benchmark drivers for `ponko_bench` (feature `bench-internals`).
#[doc(hidden)]
#[cfg(all(
    feature = "window",
    feature = "3d",
    not(target_arch = "wasm32"),
    any(test, feature = "bench-internals")
))]
pub use gpu3d::{
    LodBench, LodFrame, LodMemory, LodPath, LodScene, RecordBench, RecordScene, SceneBench, SceneFrame, SceneKind,
    SceneSetup,
};
/// Unstable 2D lighting benchmark driver for `ponko_bench` (feature `bench-internals`).
#[doc(hidden)]
#[cfg(all(
    feature = "window",
    not(target_arch = "wasm32"),
    any(test, feature = "bench-internals")
))]
pub use lights2d_bench::{Lights2DBench, Lights2DFrame, Lights2DScene};
#[cfg(not(feature = "window"))]
pub(crate) use none::Renderer;
pub use pipeline_stats::PipelineStats;
#[cfg(feature = "window")]
pub(crate) use shader_diagnostics::{diagnose, without_location};
pub use shader_status::{ShaderDiagnostic, ShaderKind, ShaderOutcome, ShaderStatus};

/// What drawing the last frame cost, from [`Perf::render_stats`](crate::perf::Perf::render_stats).
///
/// Compare `cpu_ms` and `gpu_ms` with the frame time to see what limits the
/// frame rate: if `gpu_ms` is close to the frame time, the GPU is the
/// bottleneck (too many pixels or sprites); if neither is, the time goes
/// into your systems or into waiting for the display.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RenderStats {
    /// Sprites drawn.
    pub sprites: usize,
    /// GPU draw calls: one per run of sprites sharing a texture, and with
    /// the `3d` feature, one per mesh part drawn (all its models at once).
    pub draw_calls: usize,
    /// CPU time spent sorting, batching, uploading and submitting, in ms.
    pub cpu_ms: f32,
    /// GPU time of the frame in ms (smoothed), from the start of its first
    /// pass to the end of its last, 3D and shadows included, measured with
    /// timestamp queries. It includes any time the GPU waits between
    /// passes, so it's an upper bound. `None` when the GPU doesn't support them, and in headless runs.
    pub gpu_ms: Option<f32>,
    /// 3D: of `cpu_ms`, the time spent finding what the camera and the
    /// shadow maps see (culling every model), in ms.
    pub collect_ms: f32,
    /// Of `cpu_ms`, the time turning the frame's draws into GPU command
    /// buffers (recording, on all cores), in ms.
    pub record_ms: f32,
    /// Of `cpu_ms`, the time handing the command buffers to the GPU, in ms.
    pub submit_ms: f32,
    /// 3D models the camera draws (one per drawn [`Model`]).
    pub instances: usize,
    /// Triangles the camera's passes draw (each once, though the depth
    /// prepass draws the solid ones a second time).
    pub triangles: usize,
    /// 3D models drawn into the shadow maps, all cascades together.
    pub shadow_instances: usize,
    /// Triangles drawn into the shadow maps, all cascades together.
    pub shadow_triangles: usize,
    /// 3D models in the camera's view that weren't drawn because something
    /// hid them (see `Camera3D::occlusion_culling`).
    pub occluded: usize,
    /// 3D pipeline variants built while drawing (crossfading level-of-detail
    /// models, buffer-driven instances), each the first time a frame needed
    /// it. Each one costs several milliseconds inside its frame; call
    /// [`Graphics::warm_render_pipelines`](crate::gfx::Graphics::warm_render_pipelines)
    /// while loading to build them up front. Always 0 without the `3d` feature.
    pub pipelines_built: usize,
    /// What the renderer sent to the GPU since the frame before: bytes
    /// uploaded, by kind, and buffers created. In a scene where nothing
    /// changes, only a few small per-frame uniforms should be uploaded and
    /// no buffers created.
    pub uploads: GpuUploads,
    /// How the instance batches are culled and drawn: how many take the
    /// GPU-driven path, how many the CPU path and why, and what the
    /// culling pass did last frame. All zero without the `3d` feature.
    pub instance_batches: InstanceBatchStats,
}

/// How the instance batches ([`InstanceBatch`]) are culled and drawn:
/// [`RenderStats::instance_batches`].
///
/// A batch is *GPU-driven* (culled per instance by one compute dispatch
/// shared with all other such batches, drawn with indirect draws) where
/// the device has compute culling ([`GpuFeature::ComputeCulling`]) and the
/// batch has at least [`GpuBatchLimits::min_gpu_instances`] instances.
/// Every other batch is culled on the CPU, cell by cell, and drawn with
/// direct draws; both paths draw the same picture. The counts say which
/// batches are on which path and why.
///
/// `#[non_exhaustive]`: new counts can arrive in a minor release. Read the
/// fields; to make one (in a test), start from `InstanceBatchStats::default()`.
///
/// ```
/// # use ponko::prelude::*;
/// fn report(ctx: &mut Ctx) {
///     let b = ctx.perf.render_stats().instance_batches;
///     if b.cpu_over_limit + b.dropped + b.views_skipped > 0 {
///         println!("instance batches hit a device limit: {b:?}");
///     }
/// }
/// # Game::new("t", 1, 1).system(report).run_headless(1, |_| {});
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstanceBatchStats {
    /// Live batches with at least one instance.
    pub batches: u32,
    /// Their instances, all together.
    pub instances: u64,
    /// Batches on the GPU-driven path.
    pub gpu_driven: u32,
    /// The instances of the GPU-driven batches.
    pub gpu_instances: u64,
    /// Batches culled on the CPU by choice: below the measured crossover
    /// (CPU culling of a few cells costs less than the GPU path's fixed
    /// cost per batch), or because the game asked with
    /// [`BatchCulling::PreferCpu`].
    pub cpu_small: u32,
    /// Batches culled on the CPU because the device has no compute
    /// culling (WebGL2 and other devices below the baseline tier).
    pub cpu_no_compute: u32,
    /// Batches culled on the CPU because their records are more than one
    /// storage buffer binding holds
    /// ([`GpuBatchLimits::max_gpu_instances`]). Logged once per batch.
    pub cpu_over_limit: u32,
    /// Batches not drawn at all: their instances are more than one GPU
    /// buffer holds ([`GpuBatchLimits::max_cpu_instances`]). Logged
    /// once per batch; split such a batch.
    pub dropped: u32,
    /// Buffer pools the GPU-driven batches' records share. More than one
    /// only past a storage binding's size.
    pub pools: u32,
    /// GPU memory of the pools' buffers (records, visible lists and draw
    /// arguments), in bytes.
    pub pool_bytes: u64,
    /// Culling pages recorded for the last frame: one per page with
    /// anything to cull. Each records four compute stages.
    pub cull_dispatches: u32,
    /// What the last frame culled: one job per batch, view and distance
    /// level that could be in sight.
    pub cull_jobs: u32,
    /// Batch views not drawn last frame because of a device limit (more
    /// cells than one culling dispatch takes; logged once). 0 in a
    /// healthy scene: a view always has room for its visible list.
    pub views_skipped: u32,
    /// Buffers of visible lists the GPU-driven batches' views fill: each
    /// records one culling job table. More than one when the views' lists
    /// and stable compaction scratch (about 8 bytes an instance, per view
    /// and level, plus cell counters) outgrow one buffer.
    pub pages: u32,
}

/// Bytes the renderer uploaded to the GPU, by kind, and the buffers it
/// created, over one frame: [`RenderStats::uploads`].
///
/// `#[non_exhaustive]`: new counts can arrive in a minor release. Read the
/// fields; to make one (in a test), start from `GpuUploads::default()`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GpuUploads {
    /// 3D models' per-frame records (transforms and colors), for the
    /// camera, target cameras and probe views.
    pub instances: u64,
    /// 2D sprites' records.
    pub sprites: u64,
    /// Instance batches (`Ctx::set_instance_batch`) and their culling data.
    pub static_batches: u64,
    /// Small per-frame uniforms: cameras, post-processing and effect settings.
    pub uniforms: u64,
    /// Lights and decals.
    pub lights: u64,
    /// Material parameters.
    pub materials: u64,
    /// Skinned models: joint matrices and weights.
    pub skins: u64,
    /// Mesh vertices and indices, and GPU buffers the game made.
    pub meshes: u64,
    /// Texture pixels.
    pub textures: u64,
    /// Everything else: particles, 3D text, occlusion grids.
    pub other: u64,
    /// Buffers created (a buffer outgrown is replaced by a new one).
    pub buffers_created: u32,
    /// Buffers and textures replaced by bigger ones because they were
    /// outgrown (instances, sprites, particles, skinning, light grids and
    /// the like). The new buffers are also in `buffers_created`; the
    /// textures are not. They grow to the next power of two, so a scene
    /// that keeps growing reallocates a logarithmic number of times, and a
    /// steady one none.
    pub reallocations: u32,
    /// The bytes of the buffers created.
    pub buffer_bytes_created: u64,
}

impl GpuUploads {
    pub(crate) const ZERO: Self = Self {
        instances: 0,
        sprites: 0,
        static_batches: 0,
        uniforms: 0,
        lights: 0,
        materials: 0,
        skins: 0,
        meshes: 0,
        textures: 0,
        other: 0,
        buffers_created: 0,
        reallocations: 0,
        buffer_bytes_created: 0,
    };

    /// The counts as JSON, for benchmark reports and the control endpoint.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "bytes": self.bytes(),
            "instances": self.instances,
            "sprites": self.sprites,
            "static_batches": self.static_batches,
            "uniforms": self.uniforms,
            "lights": self.lights,
            "materials": self.materials,
            "skins": self.skins,
            "meshes": self.meshes,
            "textures": self.textures,
            "other": self.other,
            "buffers_created": self.buffers_created,
            "reallocations": self.reallocations,
            "buffer_bytes_created": self.buffer_bytes_created,
        })
    }

    /// Every byte uploaded, all kinds together.
    pub fn bytes(&self) -> u64 {
        self.instances
            + self.sprites
            + self.static_batches
            + self.uniforms
            + self.lights
            + self.materials
            + self.skins
            + self.meshes
            + self.textures
            + self.other
    }
}

/// Renderer facts behind the render-pass inspector (`perf.get`'s
/// `render_inspector`) that [`RenderStats`] doesn't carry. Read once per
/// drawn frame; a plain copy, so it costs nothing when nobody asks.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RenderDetail {
    /// The device has timestamp queries, so GPU times can be measured.
    pub gpu_timestamps: bool,
    /// Frames whose GPU timestamps have come back so far.
    pub gpu_frames_timed: u64,
    /// CPU time preparing the 3D frame (collecting models and uploading
    /// their data) in ms; includes `RenderStats::collect_ms`.
    pub prepare_ms: f32,
    pub state_changes: Option<[u32; 5]>,
    /// The newest frame whose GPU timestamps came back: its own GPU ms and
    /// how many drawn frames ago it was drawn (see
    /// `GpuTimer::latest_frame`). `None` without timestamps or before the
    /// first came back.
    pub gpu_latest: Option<(f32, u64)>,
}

/// What happened when we tried to draw a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameStatus {
    Presented,
    /// The window is hidden or minimized; try again later.
    Occluded,
    /// Transient problem (timeout, outdated surface); just try the next frame.
    Skipped,
    /// The window couldn't take the frame, so it was drawn offscreen
    /// instead (benchmark runs, which must measure every frame).
    Offscreen,
}

impl FrameStatus {
    /// Whether the renderer drew the frame (to the window or offscreen):
    /// what the render inspector's history keeps.
    pub(crate) fn drew(self) -> bool {
        matches!(self, Self::Presented | Self::Offscreen)
    }
}

/// A captured frame: RGBA8 sRGB pixels, row by row from the top.
pub(crate) struct Screenshot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(all(test, feature = "window", feature = "3d"))]
mod atlas_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod buffer_render_test;
#[cfg(all(test, feature = "window", feature = "3d", feature = "compressed-textures"))]
mod compressed_bench;
#[cfg(all(test, feature = "window", feature = "3d", feature = "compressed-textures"))]
mod compressed_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod contact_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod coverage_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod draw_order_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod effect_shadow_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod far_terrain_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod frame_bench;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod gi_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod handle_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod light_shadow_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod lights2d_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod lights_bench;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod lod_motion_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod lod_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod mesh_shader_bench;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod mesh_shader_proto;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod mesh_shader_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod model_material_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod occlusion_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod regression_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod regression_sequence_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod regression_stage;
#[cfg(all(test, feature = "window", feature = "3d"))]
pub(crate) mod render_test_support;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod shadow_cache_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod shafts_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod single_sample_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod skin_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod sprite_material_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod ssr_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod static_batch_bench;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod static_batch_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod target_effect_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod text3d_render_test;
#[cfg(all(test, feature = "window", feature = "3d", feature = "text-shaping"))]
mod text_shaping_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod ui_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod upload_bench;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod upload_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod variant_render_test;

#[cfg(all(test, feature = "window", feature = "3d"))]
mod ao_aa_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod taa_render_test;

#[cfg(all(test, feature = "window", feature = "3d"))]
mod environment_render_test;

#[cfg(all(test, feature = "window", feature = "3d"))]
mod decal_render_test;

#[cfg(all(feature = "window", feature = "3d"))]
mod environment;

#[cfg(all(test, feature = "window", feature = "3d"))]
mod quality_render_test;

#[cfg(all(test, feature = "window", feature = "3d"))]
mod residency_render_test;

#[cfg(all(test, feature = "window", feature = "3d"))]
mod asset_lifetime_render_test;

#[cfg(all(test, feature = "window", feature = "3d"))]
mod ray_query_prototype_render_test;
#[cfg(all(test, feature = "window", feature = "3d"))]
mod ray_tracing_probe_render_test;

#[cfg(feature = "3d")]
pub use crate::three::{
    AmbientOcclusion, AntiAliasing, BatchCulling, Blend, Compute, ComputePass, Effect, GlobalIllumination, GpuBuffer,
    GraphicsQuality, InstanceBatch, InstanceBatchOptions, InstanceLevel, Lighting, LodGroup, LodHysteresis, LodLevel,
    LodPick, LodRenderer, LodSettings, LodShadow, Material, MaterialDesc, Mesh, MeshCost, MeshData, Model,
    ModelMaterial, ModelMaterials, Post, ReceivesDecals, RenderPasses, SceneSettings, ScreenEffect, Shader,
    ShadowFilter, ShadowMesh, Sky, StreamedLod, TargetCamera, TargetEffect, Tonemap,
};
/// Unstable extraction benchmark driver for `ponko_bench` (feature `bench-internals`).
#[doc(hidden)]
#[cfg(all(feature = "3d", any(test, feature = "bench-internals")))]
pub use crate::three::{ExtractionBench, ExtractionSample};

/// Shared additive JSON shape for the inspector and frame benchmarks.
pub(crate) fn state_changes_json(changes: Option<[u32; 5]>) -> serde_json::Value {
    changes.map_or(
        serde_json::Value::Null,
        |[pipelines, bind_groups, vertex_buffers, index_buffers, draws]| {
            serde_json::json!({
                "pipelines": pipelines, "bind_groups": bind_groups,
                "vertex_buffers": vertex_buffers, "index_buffers": index_buffers, "draws": draws,
            })
        },
    )
}
