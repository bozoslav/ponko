//! Material shaders: the game's WGSL composed with the engine's prelude and
//! entry points, and the pipelines built from it.
//!
//! [`Shaders`] owns every material shader's pipeline sets, their two
//! pipeline layouts and the cache that shares one set between shaders with
//! the same code (`pipeline_cache.rs`). A set ([`ShaderPipelines`]) lives
//! as long as a shader slot or a pending GPU check holds its `Arc`; the
//! variants a frame first needs are built inside the set, lazily
//! (`variants.rs` lists them). Nothing here knows about a frame: passes
//! look a pipeline up by shader, variant and role while recording.
//!
//! The effect and compute pass builders live here too, because they are
//! built from the same prelude and layouts; who keeps the built pipelines
//! is the renderer's business.

use std::sync::Arc;

use super::pipeline_cache::{self, Features, MaterialCache, ModuleKey, TargetKey};
use super::shader_checks::{self, end_scope, rejected};
use super::{
    DEPTH_FORMAT, HDR_FORMAT, INSTANCE_ATTRIBUTES, MOVED_ATTRIBUTES, PREVIOUS_POSITION_ATTRIBUTES, VERTEX_ATTRIBUTES,
    Vertex,
};
use super::{Instancing, PipelineRole, ShaderVariant, Sides};
use crate::render::capabilities::GpuLimits;
use crate::render::taa;
use crate::three::motion::MovedInstance;
use crate::three::{Instance3D, ShaderDiagnostic, ShaderKind};

pub(super) use super::vertex_motion::VertexPast;

/// Every material shader's pipelines, and what they are built with.
pub(super) struct Shaders {
    /// Pipeline layouts of the material shaders: lit passes, and the
    /// shadow pass (which can't read the shadow map it draws into).
    pub(super) layout: wgpu::PipelineLayout,
    pub(super) shadow_layout: wgpu::PipelineLayout,
    /// Each shader's pipelines; 0 is the built-in shader. Shaders with the
    /// same code share one set (see `pipeline_cache.rs`).
    pub(super) sets: Vec<Arc<ShaderPipelines>>,
    /// The sets by key, and the counters behind `PipelineStats`.
    pub(super) cache: MaterialCache,
    /// Whether every shader builds all its variants up front (after
    /// [`Shaders::warm`]), instead of when first drawn.
    warm_variants: bool,
    /// The optional shader parts this device has room for.
    features: Features,
    /// Samples per pixel of the targets the pipelines draw into.
    samples: u32,
    /// Whether the scene depth is read as a float texture (see
    /// [`reads_depth_as_float`]).
    float_depth: bool,
}

impl Shaders {
    /// The layouts' owner, with the built-in shader (slot 0) built.
    pub(super) fn new(
        device: &wgpu::Device,
        layout: wgpu::PipelineLayout,
        shadow_layout: wgpu::PipelineLayout,
        features: Features,
        samples: u32,
        float_depth: bool,
    ) -> Self {
        let mut cache = MaterialCache::new(TargetKey {
            color: HDR_FORMAT,
            depth: DEPTH_FORMAT,
            velocity: taa::VELOCITY_FORMAT,
            samples,
        });
        let mut builtin = build_pipelines(
            device,
            &layout,
            &shadow_layout,
            "built-in",
            &for_samples(
                &compose_shader_storage("", features.decals, features.gi, features.storage),
                samples,
                float_depth,
            ),
            samples,
            &cache.counters,
            Default::default(),
        );
        builtin.in_use = true;
        // wgpu objects aren't `Send` on wasm (one thread): the `Arc` only shares them.
        #[allow(clippy::arc_with_non_send_sync)]
        let builtin = Arc::new(builtin);
        cache.insert(
            cache.key(ModuleKey {
                source: "".into(),
                features,
            }),
            &builtin,
        );
        Self {
            layout,
            shadow_layout,
            sets: vec![builtin],
            cache,
            warm_variants: false,
            features,
            samples,
            float_depth,
        }
    }

    /// Changes between supported multisampled counts. Both use the same
    /// WGSL and layouts; every pipeline (including lazy variants) must change.
    pub(super) fn set_samples(&mut self, samples: u32) {
        let mut cache = MaterialCache::new(TargetKey {
            samples,
            ..self.cache.targets
        });
        cache.counters = self.cache.counters.clone();
        for (key, old) in self.cache.live() {
            let builder = SideBuilder {
                samples,
                ..old.builder.clone()
            };
            let new = ShaderPipelines {
                sides: Sides::ALL
                    .map(|sides| builder.timed_build(ShaderVariant::new(sides, Instancing::Plain), &cache.counters)),
                fade: Default::default(),
                shared: Default::default(),
                builder,
                counters: cache.counters.clone(),
                in_use: true,
                reads_scene: old.reads_scene,
            };
            // Keep warmed variants warm, and retain sharing between shader ids.
            for variant in ShaderVariant::ALL {
                if old.built(variant) {
                    new.lookup(variant, false);
                }
            }
            #[allow(clippy::arc_with_non_send_sync)]
            let new = Arc::new(new);
            for slot in &mut self.sets {
                if Arc::ptr_eq(slot, &old) {
                    *slot = new.clone();
                }
            }
            cache.insert(cache.key(key.module.clone()), &new);
        }
        self.cache = cache;
        self.samples = samples;
    }

    /// Builds every shader's pipeline variants now, and from now on with
    /// each new shader (see `Renderer3D::warm_pipelines`).
    pub(super) fn warm(&mut self) {
        self.warm_variants = true;
        for shader in &self.sets {
            shader.warm();
        }
    }

    /// Shader `id` was freed: its slot draws with the built-in shader, and
    /// its pipelines go unless another shader shares them.
    pub(super) fn free(&mut self, id: u32) {
        let builtin = self.sets[0].clone();
        if let Some(slot) = self.sets.get_mut(id as usize) {
            *slot = builtin;
        }
    }

    /// Checks and builds a material shader's pipelines for this device's
    /// variant, or shares the ones a shader with the same code already
    /// built (see `pipeline_cache.rs`). Pipeline errors (a binding the
    /// layout doesn't have) come back as an error instead of breaking the
    /// device.
    pub(super) fn material_pipelines(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        user: &str,
        replacing: Option<u32>,
        defer: bool,
    ) -> Result<(Arc<ShaderPipelines>, shader_checks::LaterVerdict), Box<ShaderDiagnostic>> {
        let storage = self.features.storage;
        let (decals, gi) = (self.features.decals, self.features.gi);
        let key = self.cache.key(ModuleKey {
            source: user.into(),
            features: Features { decals, gi, storage },
        });
        if let Some(shared) = self.cache.get(&key) {
            // A reload with unchanged code finds its own set: nothing shared.
            let own = replacing
                .and_then(|id| self.sets.get(id as usize))
                .is_some_and(|current| Arc::ptr_eq(current, &shared));
            if !own {
                self.cache.counters.shared();
            }
            if self.warm_variants {
                shared.warm();
            }
            return Ok((shared, None));
        }
        let start = crate::perf::Instant::now();
        let built = self.compile_material(device, name, user, defer);
        self.cache.counters.compiled(start.elapsed());
        let (mut built, verdict) = built?;
        // Its builds count once the GPU accepted it; rejected code's don't.
        self.cache.counters.add_builds(&built.counters);
        built.counters = self.cache.counters.clone();
        built.in_use = true;
        // wgpu objects aren't `Send` on wasm (one thread): the `Arc` only shares them.
        #[allow(clippy::arc_with_non_send_sync)]
        let built = Arc::new(built);
        self.cache.insert(key, &built);
        Ok((built, verdict))
    }

    /// Compiles a material shader that no other shader shares.
    pub(super) fn compile_material(
        &self,
        device: &wgpu::Device,
        name: &str,
        user: &str,
        defer: bool,
    ) -> Result<(ShaderPipelines, shader_checks::LaterVerdict), Box<ShaderDiagnostic>> {
        let storage = self.features.storage;
        let (decals, gi) = (self.features.decals, self.features.gi);
        let variant = material_variant(decals, gi, storage);
        if !storage {
            crate::render::shader_diagnostics::require_material_storage(name, &variant, user)?;
        }
        // Checked as the game wrote it, so messages quote its own code;
        // drawn with its reads of the last frame (see `vertex_motion.rs`).
        let drawn = super::vertex_motion::with_past(user);
        let source = compose_storage(user, decals, gi, storage);
        crate::render::diagnose(name, ShaderKind::Material, &variant, &source)?;
        let source = match drawn {
            std::borrow::Cow::Borrowed(_) => source,
            std::borrow::Cow::Owned(ref code) => compose_storage(code, decals, gi, storage),
        };
        let past = super::vertex_motion::vertex_past(&drawn, &source);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut pipelines = build_pipelines(
            device,
            &self.layout,
            &self.shadow_layout,
            name,
            &for_samples(&source, self.samples, self.float_depth),
            self.samples,
            // Its own until the GPU accepts it (see `material_pipelines`).
            &Arc::default(),
            past,
        );
        pipelines.reads_scene = reads_scene(user);
        if self.warm_variants {
            pipelines.warm();
        }
        match end_scope(scope, defer) {
            Ok(None) => Ok((pipelines, None)),
            Ok(Some(error)) => Err(rejected(name, ShaderKind::Material, &variant, &error)),
            Err(verdict) => Ok((pipelines, Some((variant, verdict)))),
        }
    }
}

/// The shader source for a frame of `samples` per pixel. Sources are written
/// for the multisampled frame; with one sample the scene depth the mesh
/// prelude declares is an ordinary depth texture (`textureLoad` takes a mip
/// level there instead of a sample index, and the engine's shaders pass 0 for
/// both). A game's own shader must read the scene depth at index 0 (or through
/// `scene_position`), the only value that means the same in both: the book's
/// shader section says so.
///
/// With `float_depth` (the GL backend, see [`reads_depth_as_float`]) the
/// scene depth is declared as a float texture instead (see [`depth_as_float`]).
pub(super) fn for_samples(source: &str, samples: u32, float_depth: bool) -> std::borrow::Cow<'_, str> {
    if float_depth {
        std::borrow::Cow::Owned(depth_as_float(source, samples))
    } else if samples > 1 {
        std::borrow::Cow::Borrowed(source)
    } else {
        std::borrow::Cow::Owned(source.replace("texture_depth_multisampled_2d", "texture_depth_2d"))
    }
}

/// Whether `device` reads the scene depth as an unfilterable float texture:
/// on the GL backend (WebGL2 and native OpenGL ES), which can't read a
/// depth texture the way the other backends do. Its shading language has no
/// way to load a texel of a depth texture (naga rejects WGSL's `textureLoad`
/// on one), and OpenGL ES 3.0 counts a depth texture read without a
/// comparison as incomplete unless its filter is `NEAREST` (wgpu leaves it
/// linear), so even a plain `texelFetch` would read 0. The scene depth is
/// then bound as a float texture and sampled through a non-filtering
/// sampler (see [`depth_as_float`]). WebGPU allows binding a depth format as
/// an unfilterable float texture on every backend; the other backends keep
/// the depth binding, so their shaders stay as they were.
pub(super) fn reads_depth_as_float(device: &wgpu::Device) -> bool {
    #[cfg(test)]
    if FORCE_FLOAT_DEPTH.get() {
        return true;
    }
    device.adapter_info().backend == wgpu::Backend::Gl
}

#[cfg(test)]
thread_local! {
    /// Tests: renderers made on this thread read the scene depth as a float
    /// texture on any backend, so the GL path runs on the test machine's GPU.
    pub(crate) static FORCE_FLOAT_DEPTH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a renderer with `float_depth` and `samples` per pixel samples the
/// scene depth through a non-filtering sampler (binding 2 of the scene
/// group): with one sample. A multisampled texture has no filter to make it
/// incomplete, so it is loaded per sample (`texelFetch`), and has no sampler.
pub(super) fn samples_scene_depth(float_depth: bool, samples: u32) -> bool {
    float_depth && samples <= 1
}

/// The sample type the scene depth binding (`scene_depth_texture`) is
/// declared with: see [`reads_depth_as_float`].
pub(super) fn scene_depth_sample_type(float_depth: bool) -> wgpu::TextureSampleType {
    if float_depth {
        wgpu::TextureSampleType::Float { filterable: false }
    } else {
        wgpu::TextureSampleType::Depth
    }
}

/// What [`depth_as_float`] adds at the end of a single-sampled shader: the
/// sampler and the texel read through it (at the texel's center, with
/// `NEAREST` filtering: the stored value).
pub(super) const SAMPLED_DEPTH: &str = "
// ---- the scene depth, sampled (the GL backend) ----
@group(2) @binding(2) var scene_depth_sampler: sampler;
fn scene_depth_texel(at: vec2<i32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(scene_depth_texture));
    return textureSampleLevel(scene_depth_texture, scene_depth_sampler, (vec2<f32>(at) + 0.5) / size, 0.0);
}
";

/// `source` with the scene depth (`scene_depth_texture`, declared
/// `texture_depth_multisampled_2d` by the mesh prelude) read as a float
/// texture of `samples` per pixel, the same values:
///
/// - With one sample the declaration becomes `texture_2d<f32>`, and each
///   `textureLoad(scene_depth_texture, at, i)` becomes
///   `scene_depth_texel(vec2<i32>(at)).x`, which samples it through a
///   non-filtering sampler (see [`reads_depth_as_float`] for why a load
///   won't do); the sampler and that function are added at the end.
/// - Multisampled, the declaration becomes `texture_multisampled_2d<f32>`,
///   and each load takes the red channel (`.x`) of the `vec4<f32>` it now
///   returns.
///
/// A game's shader that loads the scene depth itself keeps working: its
/// calls are rewritten the same way. Calls in `//` comments are left alone;
/// lines don't move, so error messages point at the same lines.
pub(super) fn depth_as_float(source: &str, samples: u32) -> String {
    let sampled = samples <= 1;
    let declared = if sampled {
        "texture_2d<f32>"
    } else {
        "texture_multisampled_2d<f32>"
    };
    let source = source.replace("texture_depth_multisampled_2d", declared);
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(source.len() + SAMPLED_DEPTH.len() + 64);
    let mut rest = source.as_str();
    while let Some(at) = rest.find("textureLoad") {
        let after = at + "textureLoad".len();
        let line_start = rest[..at].rfind('\n').map_or(0, |n| n + 1);
        let in_comment = rest[line_start..at].contains("//");
        let standalone = !rest[..at].ends_with(ident);
        let reads_depth = rest[after..]
            .trim_start()
            .strip_prefix('(')
            .map(str::trim_start)
            .and_then(|a| a.strip_prefix("scene_depth_texture"))
            .is_some_and(|tail| !tail.starts_with(ident));
        let call = (standalone && !in_comment && reads_depth)
            .then(|| load_arguments(rest, after))
            .flatten();
        match call {
            Some(call) => {
                out.push_str(&rest[..at]);
                if sampled {
                    out.push_str("scene_depth_texel(vec2<i32>(");
                    out.push_str(rest[call.coordinate.clone()].trim());
                    out.push_str(")).x");
                } else {
                    out.push_str(&rest[at..call.end]);
                    out.push_str(".x");
                }
                rest = &rest[call.end..];
            }
            // Not a scene depth load (or unbalanced: the compiler reports it).
            None => {
                out.push_str(&rest[..after]);
                rest = &rest[after..];
            }
        }
    }
    out.push_str(rest);
    if sampled {
        out.push_str(SAMPLED_DEPTH);
    }
    out
}

/// Where a `textureLoad(scene_depth_texture, coordinate, ..)` call's parts
/// are in `source`, the call's name ending at `after`.
struct LoadCall {
    /// The coordinate argument (the second).
    coordinate: std::ops::Range<usize>,
    /// Just past the closing parenthesis.
    end: usize,
}

/// The parts of the call whose name ends at `after`: `None` if its
/// parentheses don't close or it has fewer than two arguments.
fn load_arguments(source: &str, after: usize) -> Option<LoadCall> {
    let open = after + source[after..].find('(')?;
    let mut depth = 0usize;
    let mut commas = Vec::new();
    for (i, c) in source[open..].char_indices() {
        let i = open + i;
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    let start = *commas.first()? + 1;
                    let stop = commas.get(1).copied().unwrap_or(i);
                    return Some(LoadCall {
                        coordinate: start..stop,
                        end: i + 1,
                    });
                }
            }
            ',' if depth == 1 => commas.push(i),
            _ => {}
        }
    }
    None
}

/// One material shader's pipelines: a [`Pipelines`] set per
/// [`ShaderVariant`] (see `variants.rs` for the whole list).
pub(super) struct ShaderPipelines {
    /// [`Instancing::Plain`], one-sided and double-sided: built with the shader.
    pub(super) sides: [Pipelines; 2],
    /// [`Instancing::Crossfade`], built when first drawn (or all at once by
    /// [`Renderer3D::warm_pipelines`]).
    pub(super) fade: [std::sync::OnceLock<Pipelines>; 2],
    /// [`Instancing::Shared`]: buffer-driven instances, which all share
    /// their entity's one instance record (a zero-stride instance buffer);
    /// built when first drawn (or by [`Renderer3D::warm_pipelines`]).
    pub(super) shared: [std::sync::OnceLock<Pipelines>; 2],
    pub(super) builder: SideBuilder,
    /// The cache's counters: builds, misses, and the sets this one drops.
    pub(super) counters: Arc<pipeline_cache::Counters>,
    /// Whether shaders draw with it: sets built for code the GPU then
    /// rejected were never used, so dropping them invalidates nothing.
    pub(super) in_use: bool,
    /// Whether it reads the scene behind it (`scene_color`, `scene_position`,
    /// `scene_distance`): its transparent surfaces then need a copy of it.
    pub(super) reads_scene: bool,
}

impl ShaderPipelines {
    /// The pipelines of `variant`. One not built yet is built now, and
    /// counted in `built`.
    pub(super) fn variant(&self, variant: ShaderVariant) -> &Pipelines {
        self.lookup(variant, true)
    }

    /// The pipelines of `variant`, built now if needed; `drawing`: a
    /// frame needs them (a miss), not warming.
    pub(super) fn lookup(&self, variant: ShaderVariant, drawing: bool) -> &Pipelines {
        let side = variant.sides.index();
        let lazy = match variant.instancing {
            Instancing::Plain => return &self.sides[side],
            Instancing::Crossfade => &self.fade[side],
            Instancing::Shared => &self.shared[side],
        };
        lazy.get_or_init(|| {
            if drawing {
                self.counters.missed();
            }
            self.builder.timed_build(variant, &self.counters)
        })
    }

    /// Whether `variant` is built.
    pub(super) fn built(&self, variant: ShaderVariant) -> bool {
        let side = variant.sides.index();
        match variant.instancing {
            Instancing::Plain => true,
            Instancing::Crossfade => self.fade[side].get().is_some(),
            Instancing::Shared => self.shared[side].get().is_some(),
        }
    }

    /// Builds every variant not built yet.
    pub(super) fn warm(&self) {
        for variant in ShaderVariant::ALL {
            self.lookup(variant, false);
        }
    }

    /// How many of its variant sets are built.
    pub(super) fn built_count(&self) -> usize {
        ShaderVariant::ALL.iter().filter(|&&v| self.built(v)).count()
    }

    /// Render pipelines in each of its sets: one per [`PipelineRole`] some
    /// pass draws with at its sample count.
    pub(super) fn pipelines_per_set(&self) -> usize {
        PipelineRole::per_set(self.builder.samples > 1) + usize::from(self.builder.past.any())
    }

    /// What its `vertex` reads of the last frame.
    pub(super) fn past(&self) -> VertexPast {
        self.builder.past
    }
}

impl Drop for ShaderPipelines {
    /// The last shader using these let go (a reload or a replaced shader):
    /// its built sets are invalidated, no other shader's.
    fn drop(&mut self) {
        if self.in_use {
            self.counters.invalidated(self.built_count() as u64);
        }
    }
}

pub(super) struct Pipelines {
    pub(super) prepass: wgpu::RenderPipeline,
    pub(super) opaque: wgpu::RenderPipeline,
    pub(super) transparent: wgpu::RenderPipeline,
    pub(super) shadow: wgpu::RenderPipeline,
    /// The depth passes of cut-out materials, which run the material's
    /// shader for its alpha. Solid ones write depth without any.
    pub(super) prepass_cutout: wgpu::RenderPipeline,
    pub(super) shadow_cutout: wgpu::RenderPipeline,
    /// Cut-out materials with multisampling: alpha to coverage in the
    /// prepass (with a color target it doesn't write) and the opaque pass.
    /// `None` without multisampling.
    pub(super) coverage: Option<[wgpu::RenderPipeline; 2]>,
    /// Solid surfaces lit without a prepass: they write their own depth.
    pub(super) opaque_solid: wgpu::RenderPipeline,
    /// Motion vectors of moved models, over the opaque pass's depth.
    pub(super) velocity: wgpu::RenderPipeline,
    /// Motion vectors of the models a shader's `vertex` moves while their
    /// matrix holds still: only for shaders whose `vertex` reads the last
    /// frame ([`SideBuilder::past`]), `None` for every other.
    pub(super) velocity_still: Option<wgpu::RenderPipeline>,
}

impl Pipelines {
    /// The pipeline a `kind` draw takes for `role`: the still motion draw
    /// takes [`Pipelines::velocity_still`] (`None` without one), every other
    /// the role's.
    pub(super) fn for_kind(&self, kind: super::record::Kind, role: PipelineRole) -> Option<&wgpu::RenderPipeline> {
        if kind == super::record::Kind::VelocityStill {
            self.velocity_still.as_ref()
        } else {
            self.role(role)
        }
    }

    /// The pipeline of `role`; `None` for the coverage roles without
    /// multisampling.
    pub(super) fn role(&self, role: PipelineRole) -> Option<&wgpu::RenderPipeline> {
        Some(match role {
            PipelineRole::Prepass => &self.prepass,
            PipelineRole::PrepassCutout => &self.prepass_cutout,
            PipelineRole::PrepassCoverage => &self.coverage.as_ref()?[0],
            PipelineRole::Opaque => &self.opaque,
            PipelineRole::OpaqueCoverage => &self.coverage.as_ref()?[1],
            PipelineRole::OpaqueSolid => &self.opaque_solid,
            PipelineRole::Transparent => &self.transparent,
            PipelineRole::Shadow => &self.shadow,
            PipelineRole::ShadowCutout => &self.shadow_cutout,
            PipelineRole::Velocity => &self.velocity,
        })
    }
}

/// The built-in parts of every material shader.
pub(super) const MESH_PRELUDE: &str = concat!(include_str!("../sky_sampling.wgsl"), "\n", include_str!("../mesh.wgsl"));

pub(super) const MESH_MAIN: &str = include_str!("../mesh_main.wgsl");

/// Projected decals in the mesh shader (see `Decal3D`), or the stand-in
/// where the GPU has no texture slot for them.
pub(super) const DECALS: &str = include_str!("../decal.wgsl");

pub(super) const NO_DECALS: &str = include_str!("../decal_off.wgsl");

/// Whether `device` has a texture slot to spare for projected decals.
/// Native devices ask for more than the default when the adapter has it;
/// WebGL2 doesn't, and skips decals.
pub(crate) fn supports_decals(device: &wgpu::Device) -> bool {
    GpuLimits::of(device).decals()
}

/// Global illumination's probes in the mesh shader (see `gi.rs`), or the
/// stand-in where the GPU can't have them.
pub(super) const GI: &str = include_str!("../gi.wgsl");

pub(super) const NO_GI: &str = include_str!("../gi_off.wgsl");

/// Global illumination's probe update pass (see `gi.rs`).
pub(super) const GI_UPDATE: &str = include_str!("../gi_update.wgsl");

/// The probe update shader: the mesh prelude (the frame, the sky's light)
/// and the update.
pub(in crate::render) fn gi_update_source() -> String {
    format!("{MESH_PRELUDE}\n{GI_UPDATE}")
}

/// Whether `device` can draw global illumination: compute shaders, and a
/// texture slot past the decals' for the probes (the 18th).
pub(crate) fn supports_gi(device: &wgpu::Device) -> bool {
    GpuLimits::of(device).gi()
}

pub(super) const EFFECT_PRELUDE: &str = include_str!("../effect.wgsl");

pub(super) const EFFECT_MAIN: &str = include_str!("../effect_main.wgsl");

pub(super) const COMPUTE_PRELUDE: &str = include_str!("../compute.wgsl");

pub(super) const COMPUTE_MAIN: &str = include_str!("../compute_main.wgsl");

/// Whether WGSL `source` defines a function called `name`.
pub(super) fn defines(source: &str, name: &str) -> bool {
    crate::render::shader_diagnostics::contract_tokens(source)
        .windows(3)
        .any(|t| t[0].0 == "fn" && t[1].0 == name && t[2].0 == "(")
}

/// Whether a shader reads the scene behind it.
pub(super) fn reads_scene(user: &str) -> bool {
    ["scene_color", "scene_position", "scene_distance"]
        .iter()
        .any(|f| user.contains(f))
}

/// A whole material shader: the engine's prelude, the game's code (with
/// pass-through `vertex` and `surface` functions when it has none) and the
/// engine's entry points.
pub(crate) fn compose_shader(user: &str) -> String {
    compose_material(&super::vertex_motion::with_past(user), true, true)
}

/// [`compose_shader`] for a device with (`decals`) or without a texture
/// slot for projected decals (and none for global illumination either).
#[cfg(test)]
pub(crate) fn compose_shader_with(user: &str, decals: bool) -> String {
    compose_material(&super::vertex_motion::with_past(user), decals, decals)
}

/// [`compose_shader`] for a device with or without the texture slots for
/// projected decals and global illumination, with the game's code `user` as
/// it is (see [`super::vertex_motion::with_past`] for the code it draws with).
pub(super) fn compose_material(user: &str, decals: bool, gi: bool) -> String {
    let decal_code = if decals { DECALS } else { NO_DECALS };
    let gi_code = if gi { GI } else { NO_GI };
    let mut source = String::with_capacity(
        MESH_PRELUDE.len() + decal_code.len() + gi_code.len() + user.len() + MESH_MAIN.len() + 200,
    );
    source.push_str(MESH_PRELUDE);
    source.push_str("@group(3) @binding(0) var<storage, read> items: array<vec4<f32>>;\n@group(3) @binding(1) var<storage, read> static_indices: array<u32>;\n");
    source.push_str(super::vertex_motion::ITEM_HELPER);
    source.push_str(decal_code);
    source.push_str(gi_code);
    source.push_str("\n// ---- the game's shader ----\n");
    source.push_str(user);
    source.push_str("\n// ---- the engine's entry points ----\n");
    if !defines(user, "vertex") {
        source.push_str("fn vertex(v: Vertex) -> Vertex { return v; }\n");
    }
    if !defines(user, "surface") {
        source.push_str("fn surface(s: Surface) -> Surface { return s; }\n");
    }
    source.push_str(&MESH_MAIN.replace("// STATIC_INSTANCE_HELPER", include_str!("instance_vertex.wgsl")));
    source
}

/// The material shader a device with or without instance storage draws
/// with: the game's code `user` reading the last frame where the motion
/// pass asks it to (see `vertex_motion.rs`).
pub(super) fn compose_shader_storage(user: &str, decals: bool, gi: bool, storage: bool) -> String {
    compose_storage(&super::vertex_motion::with_past(user), decals, gi, storage)
}

/// [`compose_shader_storage`] with the game's code as it is.
fn compose_storage(user: &str, decals: bool, gi: bool, storage: bool) -> String {
    let source = compose_material(user, decals, gi);
    if storage {
        source
    } else {
        source.replace(
            include_str!("instance_vertex.wgsl"),
            "fn static_instance(v: VertexIn) -> VertexIn { return v; }",
        )
    }
}

/// A whole screen effect shader from the game's part: the mesh prelude
/// (bindings and helpers), `Pixel`, the game's `effect`, the entry points.
pub(crate) fn compose_effect(user: &str) -> String {
    let mut source =
        String::with_capacity(MESH_PRELUDE.len() + EFFECT_PRELUDE.len() + user.len() + EFFECT_MAIN.len() + 100);
    source.push_str(MESH_PRELUDE);
    source.push_str(EFFECT_PRELUDE);
    source.push_str("\n// ---- the game's effect ----\n");
    source.push_str(user);
    source.push_str("\n// ---- the engine's entry points ----\n");
    source.push_str(EFFECT_MAIN);
    source
}

/// A whole compute pass shader from the game's part: the mesh prelude
/// (bindings and helpers), `Cell`, the game's `compute`, the entry point.
pub(crate) fn compose_compute(user: &str) -> String {
    let mut source =
        String::with_capacity(MESH_PRELUDE.len() + COMPUTE_PRELUDE.len() + user.len() + COMPUTE_MAIN.len() + 100);
    source.push_str(MESH_PRELUDE);
    if defines(user, "update") {
        source.push_str("@group(3) @binding(0) var<storage, read_write> items: array<vec4<f32>>;\n");
    } else {
        source.push_str(COMPUTE_PRELUDE);
    }
    source.push_str("\n// ---- the game's compute pass ----\n");
    source.push_str(user);
    source.push_str("\n// ---- the engine's entry point ----\n");
    if defines(user, "update") {
        source.push_str("@compute @workgroup_size(64) fn cs_compute(@builtin(global_invocation_id) id: vec3<u32>) { if id.x < arrayLength(&items) { update(id.x); } }");
    } else {
        source.push_str(COMPUTE_MAIN);
    }
    source
}

/// Checks a composed shader, with an error message that points at the
/// line, as WGSL compilers print them.
#[cfg(test)]
pub(crate) fn check_shader(name: &str, source: &str) -> Result<(), String> {
    use wgpu::naga;
    let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string_with_path(source, name))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| e.emit_to_string_with_path(source, name))?;
    Ok(())
}

/// The variant the engine checks every material shader against first:
/// every optional part on.
pub(crate) const FULL_MATERIAL_VARIANT: &str = "material shader with decals, global illumination and instance storage";

pub(crate) const EFFECT_VARIANT: &str = "screen effect";

pub(crate) const COMPUTE_VARIANT: &str = "compute pass";

/// The material shader variant this device compiles.
pub(super) fn material_variant(decals: bool, gi: bool, storage: bool) -> String {
    let on = |b: bool| if b { "on" } else { "off" };
    format!(
        "material shader on this GPU: decals {}, global illumination {}, instance storage {}",
        on(decals),
        on(gi),
        on(storage)
    )
}

/// A material shader's pipelines: depth prepass, opaque, transparent and
/// shadow, each for one-sided and double-sided surfaces.
/// The pipeline of a compute pass.
pub(super) fn build_compute(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    name: &str,
    source: &str,
) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(name),
        layout: Some(layout),
        module: &module,
        entry_point: Some("cs_compute"),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// The pipeline of a screen effect: a full-screen triangle into the HDR
/// frame, with the material pipelines' bindings.
pub(super) fn build_effect(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    name: &str,
    source: &str,
) -> (wgpu::ShaderModule, wgpu::RenderPipeline) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = effect_pipeline(device, layout, name, &module, "vs_effect", "fs_effect");
    (module, pipeline)
}

/// A full-screen pipeline of an effect shader's entry points, writing `HDR_FORMAT`.
pub(super) fn effect_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    name: &str,
    module: &wgpu::ShaderModule,
    vertex: &str,
    fragment: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(name),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(HDR_FORMAT.into())],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// `source` is ready for the targets: through [`for_samples`].
#[allow(clippy::too_many_arguments)]
pub(super) fn build_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shadow_layout: &wgpu::PipelineLayout,
    name: &str,
    source: &str,
    samples: u32,
    counters: &Arc<pipeline_cache::Counters>,
    past: VertexPast,
) -> ShaderPipelines {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let builder = SideBuilder {
        device: device.clone(),
        module,
        layout: layout.clone(),
        shadow_layout: shadow_layout.clone(),
        samples,
        past,
    };
    ShaderPipelines {
        sides: Sides::ALL.map(|sides| builder.timed_build(ShaderVariant::new(sides, Instancing::Plain), counters)),
        fade: Default::default(),
        shared: Default::default(),
        builder,
        counters: counters.clone(),
        in_use: false,
        reads_scene: false,
    }
}

/// What it takes to build a material shader's pipelines, kept to build the
/// crossfade variants when a frame first needs them.
#[derive(Clone)]
pub(super) struct SideBuilder {
    pub(super) device: wgpu::Device,
    pub(super) module: wgpu::ShaderModule,
    pub(super) layout: wgpu::PipelineLayout,
    pub(super) shadow_layout: wgpu::PipelineLayout,
    pub(super) samples: u32,
    /// What its `vertex` reads of the last frame: with anything, each set
    /// also has a still motion pipeline ([`Pipelines::velocity_still`]).
    pub(super) past: VertexPast,
}

impl SideBuilder {
    /// [`SideBuilder::build`], timed into `counters`.
    pub(super) fn timed_build(&self, variant: ShaderVariant, counters: &pipeline_cache::Counters) -> Pipelines {
        let start = crate::perf::Instant::now();
        let pipelines = self.build(variant);
        counters.built(start.elapsed());
        pipelines
    }

    /// The pipeline set of `variant`: its sides pick the culling, its
    /// instancing the instance buffer's stride and (crossfading) the
    /// dithering fragment entry points; each role's entry points come from
    /// [`PipelineRole`].
    pub(super) fn build(&self, variant: ShaderVariant) -> Pipelines {
        let cull = variant.sides.cull();
        let shared = variant.instancing.shared();
        let vs = PipelineRole::vertex_entry;
        let fs = |role: PipelineRole| role.fragment_entry(variant.instancing);
        let (device, module, layout, shadow_layout, samples) = (
            &self.device,
            &self.module,
            &self.layout,
            &self.shadow_layout,
            self.samples,
        );
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: if shared { 0 } else { size_of::<Instance3D>() as u64 },
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &INSTANCE_ATTRIBUTES,
            }),
        ];
        let depth = |write, compare, bias| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias,
            })
        };
        let msaa = wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        };
        let color = |blend| {
            [Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })]
        };
        let opaque_target = color(None);
        let transparent_target = color(Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        vertex: &str,
                        fragment: Option<&str>,
                        cull: Option<wgpu::Face>,
                        depth_stencil,
                        multisample,
                        targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil,
                multisample,
                fragment: fragment.map(|fragment| wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Moved models: only the motion, wherever the opaque pass drew them.
        let moved_buffers = [
            buffers[0].clone(),
            Some(wgpu::VertexBufferLayout {
                array_stride: if shared { 0 } else { size_of::<MovedInstance>() as u64 },
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &MOVED_ATTRIBUTES,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &PREVIOUS_POSITION_ATTRIBUTES,
            }),
        ];
        let velocity_target = [Some(wgpu::ColorTargetState {
            format: super::taa::VELOCITY_FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let velocity = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3d motion vectors"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(vs(PipelineRole::Velocity)),
                compilation_options: Default::default(),
                buffers: &moved_buffers,
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: cull,
                ..Default::default()
            },
            depth_stencil: depth(false, wgpu::CompareFunction::Equal, Default::default()),
            multisample: msaa,
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: fs(PipelineRole::Velocity),
                compilation_options: Default::default(),
                targets: &velocity_target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let shadow_bias = wgpu::DepthBiasState {
            constant: 2,
            slope_scale: 2.0,
            clamp: 0.0,
        };
        // Solid surfaces write depth with no fragment shader at all, so the
        // GPU can reject hidden ones early; cut-out ones run it for the alpha.
        let prepass = |role: PipelineRole| {
            pipeline(
                "3d depth prepass",
                layout,
                vs(role),
                fs(role),
                cull,
                depth(true, wgpu::CompareFunction::Less, Default::default()),
                msaa,
                &[],
            )
        };
        // Both faces cast: thin things (leaves, planes) too. The stored depth
        // is pushed back a little, against shadow acne.
        let shadow = |role: PipelineRole| {
            pipeline(
                "3d shadow",
                shadow_layout,
                vs(role),
                fs(role),
                None,
                depth(true, wgpu::CompareFunction::Less, shadow_bias),
                wgpu::MultisampleState::default(),
                &[],
            )
        };
        let coverage_msaa = wgpu::MultisampleState {
            alpha_to_coverage_enabled: true,
            ..msaa
        };
        let unwritten = [Some(wgpu::ColorTargetState {
            format: HDR_FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::empty(),
        })];
        let coverage = (samples > 1).then(|| {
            [
                pipeline(
                    "3d depth prepass coverage",
                    layout,
                    vs(PipelineRole::PrepassCoverage),
                    fs(PipelineRole::PrepassCoverage),
                    cull,
                    depth(true, wgpu::CompareFunction::Less, Default::default()),
                    coverage_msaa,
                    &unwritten,
                ),
                pipeline(
                    "3d opaque coverage",
                    layout,
                    vs(PipelineRole::OpaqueCoverage),
                    fs(PipelineRole::OpaqueCoverage),
                    cull,
                    depth(false, wgpu::CompareFunction::Equal, Default::default()),
                    coverage_msaa,
                    &opaque_target,
                ),
            ]
        });
        Pipelines {
            coverage,
            prepass: prepass(PipelineRole::Prepass),
            prepass_cutout: prepass(PipelineRole::PrepassCutout),
            // Only the fragments the prepass found nearest: each pixel is lit once.
            opaque: pipeline(
                "3d opaque",
                layout,
                vs(PipelineRole::Opaque),
                fs(PipelineRole::Opaque),
                cull,
                depth(false, wgpu::CompareFunction::Equal, Default::default()),
                msaa,
                &opaque_target,
            ),
            opaque_solid: pipeline(
                "3d opaque solid",
                layout,
                vs(PipelineRole::OpaqueSolid),
                fs(PipelineRole::OpaqueSolid),
                cull,
                depth(true, wgpu::CompareFunction::Less, Default::default()),
                msaa,
                &opaque_target,
            ),
            // Over the opaque frame, without hiding what's behind it.
            transparent: pipeline(
                "3d transparent",
                layout,
                vs(PipelineRole::Transparent),
                fs(PipelineRole::Transparent),
                cull,
                depth(false, wgpu::CompareFunction::Less, Default::default()),
                msaa,
                &transparent_target,
            ),
            shadow: shadow(PipelineRole::Shadow),
            shadow_cutout: shadow(PipelineRole::ShadowCutout),
            velocity,
            // Models that hold still but whose `vertex` moves them: drawn
            // from the frame's own instances, `vertex` run as of this frame
            // and the last (see `vertex_motion.rs`).
            velocity_still: self.past.any().then(|| {
                pipeline(
                    "3d still motion vectors",
                    layout,
                    "vs_velocity_still",
                    fs(PipelineRole::Velocity),
                    cull,
                    depth(false, wgpu::CompareFunction::Equal, Default::default()),
                    msaa,
                    &velocity_target,
                )
            }),
        }
    }
}
