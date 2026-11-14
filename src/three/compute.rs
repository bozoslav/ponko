//! Compute passes: a game's WGSL run on the GPU once per texel of a render
//! target each frame, for simulations kept in textures (snow and mud
//! trails, wet ground, ripples, heat, fog of war).

use super::Material;
use crate::gfx::Graphics;
use crate::texture::Texture;
use glam::Vec3;

/// A compute pass shader, made with [`Graphics::load_compute`](crate::gfx::Graphics::load_compute) or
/// [`Graphics::create_compute`](crate::gfx::Graphics::create_compute); a cheap handle. Run it by adding a
/// [`ComputePass`] to [`RenderPasses::compute`](crate::render::RenderPasses::compute).
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// let melt = ctx.gfx.load_compute("compute/melt.wgsl");
/// let snow = ctx.gfx.create_render_target(512, 512);
/// ctx.scene3d.passes.compute.push(ponko::render::ComputePass::new(melt, snow));
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)] // shared with C/C++ (`PonkoCompute`)
pub struct Compute {
    pub(crate) id: u32,
}

/// One entry of [`RenderPasses::compute`](crate::render::RenderPasses::compute) or
/// [`RenderPasses::scene_compute`](crate::render::RenderPasses::scene_compute): a
/// shader, the render target (or [`GpuBuffer`]) it updates, and a material
/// whose custom values and textures it reads.
///
/// **Stages.** The list it is in decides when it runs, once per frame, in
/// list order:
/// - `compute_passes`: before anything is drawn. This frame's materials,
///   target cameras, target effects and screen effects see its result; its
///   shader sees no scene (`scene_color` is black).
/// - `scene_compute_passes`: after the opaque and transparent surfaces and
///   temporal antialiasing, before target effects, screen effects, bloom
///   and tone mapping. Its shader reads this frame's lit image and depth
///   (`scene_color`, `scene_position`, `scene_distance`, at a pixel of the
///   frame: `frame.screen.xy` is its size). Target effects and screen
///   effects see its result this frame; materials, target cameras and
///   `compute_passes` see last frame's.
///
/// **Resources.** A pass declares what it writes (its `target` or `buffer`)
/// and reads (its material's textures, its own target through `c.value` and
/// `cell_at`, and the scene at the scene stage). It updates its target in
/// place: a later pass writing the same target (a compute pass of either
/// stage) reads what it left. A [`TargetCamera`](crate::render::TargetCamera)
/// or [`TargetEffect`](crate::render::TargetEffect) instead replaces all of
/// its target, so it can't come after another writer of that target in the
/// frame: one result would be lost. Such a pair never runs: the target
/// effect is skipped, or, under a target camera, the compute pass, with a
/// warning naming the pass and the render target. A scene compute pass
/// after a target camera reads the camera's picture, which is allowed. GPU
/// buffers are always updated in place, in stage and list order.
///
/// **GPUs without compute shaders** (WebGL2,
/// [`GpuFeature::Compute`](crate::render::GpuFeature::Compute) not
/// [available](crate::render::GpuCapabilities::available)):
/// [`Graphics::try_create_compute`] returns an error for a texture pass, and
/// buffer passes do nothing, at both stages; the rest of the frame draws as
/// without them.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx, trails: ponko::render::Compute, snow: Texture, player: Vec3) {
/// let feet = ctx.gfx.create_material(
///     ponko::render::MaterialDesc::default().with_custom([player.extend(0.5), Vec4::ZERO, Vec4::ZERO, Vec4::ZERO]),
/// );
/// ctx.scene3d.passes.compute.push(ponko::render::ComputePass::new(trails, snow).with_material(feet));
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputePass {
    /// The shader.
    pub compute: Compute,
    /// What it updates: made by [`Graphics::create_render_target`](crate::gfx::Graphics::create_render_target); other
    /// textures are skipped.
    pub target: Texture,
    /// Storage buffer target, instead of `target`, when present.
    pub buffer: Option<GpuBuffer>,
    /// Where its values and textures come from. `None`: the default
    /// material (all custom values 0).
    pub material: Option<Material>,
}

impl ComputePass {
    /// Runs `fn update(i: u32)` once per buffer element, in groups of 64.
    /// The shader can read and write `items[i]` and read `frame` and `material`.
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, c: ponko::render::Compute) {
    /// let b = ctx.gfx.create_gpu_buffer(64, &[]);
    /// ctx.scene3d.passes.compute.push(ponko::render::ComputePass::for_buffer(c, b));
    /// # }
    pub fn for_buffer(compute: Compute, buffer: GpuBuffer) -> Self {
        Self {
            compute,
            target: Texture::new(0, 0, 0),
            buffer: Some(buffer),
            material: None,
        }
    }

    /// Runs `compute` on `target` with no values of its own.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, c: ponko::render::Compute, t: Texture) {
    /// ctx.scene3d.passes.compute.push(ponko::render::ComputePass::new(c, t));
    /// # }
    /// ```
    #[must_use]
    pub fn new(compute: Compute, target: Texture) -> Self {
        Self {
            compute,
            target,
            buffer: None,
            material: None,
        }
    }

    /// The same, reading its values and textures from `material`; change
    /// them with [`Graphics::update_material`](crate::gfx::Graphics::update_material) to steer the simulation.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(c: ponko::render::Compute, t: Texture, m: ponko::render::Material) {
    /// let pass = ponko::render::ComputePass::new(c, t).with_material(m);
    /// # }
    /// ```
    #[must_use]
    pub fn with_material(self, material: Material) -> Self {
        Self {
            material: Some(material),
            ..self
        }
    }
}

impl Graphics {
    /// Loads a compute pass shader (WGSL) from an [added asset
    /// source](crate::assets::Assets::add_source) or `assets/`. It defines
    /// `fn compute(c: Cell) -> vec4<f32>`: the new value of each texel of
    /// the render target, from what it holds now (`c.value`, all 0 at
    /// first), where it is (`c.pixel`, `c.size`, `c.uv`) and its
    /// neighbours (`cell_at(pixel)`, which reads the target as it was
    /// before this pass). It reads `frame` (`frame.time.y` is the frame's
    /// seconds) and `material` as material shaders do; run as a
    /// [scene compute pass](crate::render::RenderPasses::scene_compute), it also reads
    /// this frame's lit image and depth (`scene_color(pixel)`,
    /// `scene_position(pixel)`). With hot reload on,
    /// saving the file in `assets/` updates the game (an error keeps the old one).
    /// Loading the same file again returns the same compute pass.
    ///
    /// For GPU buffers, define `fn update(i: u32)` instead, accessing the
    /// read-write `items: array<vec4<f32>>`. Schedule it with
    /// [`crate::render::ComputePass::for_buffer`]; it shares the texture passes' frame uniforms
    /// and ordering. WebGL2 skips buffer compute passes; shaders reading
    /// `items` and GPU-buffer draw setup report the storage requirement.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let ripples = ctx.gfx.load_compute("compute/ripples.wgsl");
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If the file can't be read or doesn't compile (the message says where).
    /// With hot reload on, a file that doesn't compile doesn't panic: it
    /// gets a stand-in and stays watched until a saved fix compiles
    /// ([`crate::render::ShaderOutcome::Failed`], [`Graphics::shader_status`](crate::gfx::Graphics::shader_status)).
    pub fn load_compute(&mut self, path: &str) -> Compute {
        self.try_load_compute(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_compute`](Self::load_compute) with errors as values:
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file is
    /// missing, [`Io`](crate::error::ErrorKind::Io) if it can't be read,
    /// [`Decode`](crate::error::ErrorKind::Decode) if it isn't UTF-8 text and
    /// [`Shader`](crate::error::ErrorKind::Shader) if it doesn't compile (the
    /// message has the compiler's log). With hot reload on, a file that
    /// doesn't compile still returns a stand-in, as `load_compute` does.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_load_compute("missing.wgsl").unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::NotFound);
    /// });
    /// ```
    pub fn try_load_compute(&mut self, path: &str) -> Result<Compute, crate::error::Error> {
        let found = match self
            .asset_sources
            .lookup(path, |origin| self.assets3d.loaded_computes.get(origin).copied())?
        {
            crate::assets::Lookup::Cached(_, compute) => return Ok(compute),
            crate::assets::Lookup::Found(found) => found,
        };
        let origin = found.origin.clone();
        let source = found.into_text(path)?;
        let compute = match self.try_create_compute(path, &source) {
            Ok(compute) => compute,
            // With hot reload on, it is skipped until a saved fix compiles;
            // its status says what's wrong.
            Err(e) if self.keeps_failed_load(&origin) => {
                let e = e.with_path(path);
                Self::report_stand_in(path, &e);
                let id = self.assets3d.computes;
                self.compile_compute(path, super::COMPUTE_STAND_IN, None)
                    .map_err(|_| e)?;
                if let Some(renderer) = self.renderer.as_mut()
                    && !self.headless
                {
                    renderer.free_compute(id);
                }
                self.assets3d.computes += 1;
                self.assets3d.computes_live.insert(id);
                self.asset_ledger.made(crate::inventory::AssetClass::Compute, id);
                Compute { id }
            }
            Err(e) => return Err(e.with_path(path)),
        };
        self.assets3d.loaded_computes.insert(origin.clone(), compute);
        self.asset_ledger
            .own(crate::inventory::AssetClass::Compute, compute.id, path);
        if let crate::assets::Origin::File(file) = origin {
            self.watcher
                .watch(path, file, crate::hot_reload::Asset::Compute(compute.id));
        }
        Ok(compute)
    }

    /// Makes a compute pass shader from WGSL source; `name` is for error
    /// messages. See [`Graphics::load_compute`](crate::gfx::Graphics::load_compute) for what it defines.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     // Fades toward 0 by half a second.
    ///     let fade = ctx.gfx.create_compute("fade", "
    ///         fn compute(c: Cell) -> vec4<f32> {
    ///             return c.value * max(1.0 - 2.0 * frame.time.y, 0.0);
    ///         }");
    ///     let target = ctx.gfx.create_render_target(64, 64);
    ///     ctx.scene3d.passes.compute.push(ponko::render::ComputePass::new(fade, target));
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// If it doesn't compile (the message says where).
    pub fn create_compute(&mut self, name: &str, source: &str) -> Compute {
        self.try_create_compute(name, source).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`create_compute`](Self::create_compute) with errors as values:
    /// [`ErrorKind::Shader`](crate::error::ErrorKind::Shader) if it doesn't
    /// compile, with the compiler's log in the message.
    pub fn try_create_compute(&mut self, name: &str, source: &str) -> Result<Compute, crate::error::Error> {
        let id = self.assets3d.computes;
        let compiled = self.compile_compute(name, source, None);
        self.record_shader(name, super::ShaderKind::Compute, false, compiled)?;
        self.assets3d.computes += 1;
        self.assets3d.computes_live.insert(id);
        self.asset_ledger.made(crate::inventory::AssetClass::Compute, id);
        Ok(Compute { id })
    }

    /// Checks compute `source` and builds its pipeline, as `replace` or as the next one.
    fn compile_compute(
        &mut self,
        name: &str,
        source: &str,
        replace: Option<u32>,
    ) -> Result<(), Box<super::ShaderDiagnostic>> {
        #[cfg(feature = "window")]
        crate::render::diagnose(
            name,
            super::ShaderKind::Compute,
            crate::render::COMPUTE_VARIANT,
            &crate::render::compose_compute(source),
        )?;
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            let built = match replace {
                Some(id) => renderer.replace_compute(id, name, source),
                None => renderer
                    .create_compute(name, source)
                    .map(|gpu_id| debug_assert_eq!(gpu_id, self.assets3d.computes)),
            };
            built?;
        }
        let _ = (name, source, replace);
        Ok(())
    }

    /// Recompiles a compute pass whose file changed (hot reload).
    pub(crate) fn reload_compute(&mut self, id: u32, name: &str, source: &str) -> Result<(), String> {
        if !self.assets3d.computes_live.contains(id) {
            return Ok(());
        }
        let compiled = self.compile_compute(name, source, Some(id));
        Ok(self.record_shader(name, super::ShaderKind::Compute, true, compiled)?)
    }

    /// Frees compute pass `id` (see [`Ctx::free`](crate::ctx::Ctx::free)): entries of
    /// [`RenderPasses::compute`](crate::render::RenderPasses::compute) running it are skipped from now on.
    pub(crate) fn free_compute_id(&mut self, id: u32) {
        if !self.assets3d.computes_live.remove(id) {
            return;
        }
        self.asset_ledger.freed(crate::inventory::AssetClass::Compute, id);
        self.assets3d.loaded_computes.retain(|_, c| c.id != id);
        self.watcher
            .forget(|a| matches!(a, crate::hot_reload::Asset::Compute(c) if *c == id));
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_compute(id);
        }
    }
}

/// A GPU array of `vec4<f32>`. Empty initial data creates zeroed elements.
/// ```
/// # use ponko::prelude::*;
/// Game::new("buffers", 1, 1).run_headless(0, |ctx| {
///     let b = ctx.gfx.create_gpu_buffer(128, &[]);
///     ctx.gfx.write_gpu_buffer(b, 0, &[Vec4::ONE]);
/// });
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct GpuBuffer {
    pub(crate) id: u32,
}
impl GpuBuffer {
    /// Its slot in the engine's buffer table and the renderer: the id
    /// without its generation.
    #[inline]
    pub(crate) fn slot(self) -> u32 {
        self.id & super::SLOT_MASK
    }
}
impl Graphics {
    /// Creates `len` elements. Initial data may be empty or exactly `len` elements.
    /// A freed buffer's slot is reused first, under a new id, so the freed
    /// handle stays dead (see [`Ctx::free`](crate::ctx::Ctx::free)).
    /// Panics for zero length or mismatched data. See [`crate::render::GpuBuffer`].
    pub fn create_gpu_buffer(&mut self, len: u32, data: &[glam::Vec4]) -> GpuBuffer {
        assert!(
            len > 0 && (data.is_empty() || data.len() == len as usize),
            "invalid GPU buffer length"
        );
        let id = (self.assets3d.gpu_buffer_ids).make(self.assets3d.gpu_buffers.len(), "GPU buffer");
        let b = GpuBuffer { id };
        if b.slot() as usize == self.assets3d.gpu_buffers.len() {
            self.assets3d.gpu_buffers.push(0);
        }
        self.assets3d.gpu_buffers[b.slot() as usize] = len;
        self.asset_ledger.made(crate::inventory::AssetClass::GpuBuffer, id);
        if let Some(r) = self.renderer.as_mut()
            && !self.headless
        {
            r.create_gpu_buffer(b.slot(), len, data);
        }
        b
    }
    /// Number of elements, or `None` for an invalid or freed handle.
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("b", 1, 1).run_headless(0, |ctx| {
    /// let b = ctx.gfx.create_gpu_buffer(2, &[]); assert_eq!(ctx.gfx.gpu_buffer_len(b), Some(2));
    /// });
    /// ```
    pub fn gpu_buffer_len(&self, b: GpuBuffer) -> Option<u32> {
        let slot = self.assets3d.gpu_buffer_slot(b.id)?;
        Some(self.assets3d.gpu_buffers[slot]).filter(|&len| len > 0)
    }
    /// Frees `b`'s GPU memory: the engine side of [`Graphics::free`](crate::gfx::Graphics::free) with a
    /// buffer. The compute passes that write to it are removed before its id
    /// is given to another buffer.
    pub(crate) fn free_gpu_buffer(&mut self, b: GpuBuffer) {
        // A stale handle (its slot given to a newer buffer) frees nothing.
        let Some(slot) = self.assets3d.gpu_buffer_slot(b.id) else {
            return;
        };
        let len = &mut self.assets3d.gpu_buffers[slot];
        if *len == 0 {
            return;
        }
        *len = 0;
        self.asset_ledger.freed(crate::inventory::AssetClass::GpuBuffer, b.id);
        // Back in the pool once its compute passes are gone
        // (`Ctx::unlink_freed_gpu_buffers`).
        self.assets3d.unlinking_gpu_buffers.push(b.id);
        if let Some(r) = self.renderer.as_mut()
            && !self.headless
        {
            r.free_gpu_buffer(b.slot());
        }
    }
    /// Updates a range before the next frame. Panics for invalid handles or ranges.
    /// See [`crate::render::GpuBuffer`] for an example.
    pub fn write_gpu_buffer(&mut self, b: GpuBuffer, offset: u32, data: &[glam::Vec4]) {
        let len = self.gpu_buffer_len(b).expect("invalid GPU buffer");
        assert!(
            offset <= len && data.len() <= (len - offset) as usize,
            "GPU buffer write out of bounds"
        );
        if !data.is_empty()
            && let Some(r) = self.renderer.as_mut()
            && !self.headless
        {
            r.write_gpu_buffer(b.slot(), offset, data);
        }
    }
    /// Makes `b` keep a copy of the elements the last frame drew, so the
    /// motion pass (temporal antialiasing, temporal AO) sees how each
    /// element moved: a material's `vertex` reading `items[i]` reads that
    /// copy when it runs as of the last frame, and particles moved by a
    /// compute pass reproject with their own motion instead of smearing.
    /// Opt-in because it doubles the buffer's memory and, while motion
    /// vectors are on, copies the buffer once a frame on the GPU (nothing
    /// while they are off). The copy starts as the buffer is, so turning it
    /// on adds no motion; `keep: false` gives the memory back. Elements
    /// written from the CPU ([`write_gpu_buffer`](Self::write_gpu_buffer))
    /// count as moved from where the last frame drew them. Panics for an
    /// invalid handle.
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("b", 1, 1).run_headless(0, |ctx| {
    ///     let sparks = ctx.gfx.create_gpu_buffer(256, &[]);
    ///     ctx.gfx.set_gpu_buffer_motion(sparks, true);
    /// });
    /// ```
    pub fn set_gpu_buffer_motion(&mut self, b: GpuBuffer, keep: bool) {
        self.gpu_buffer_len(b).expect("invalid GPU buffer");
        if let Some(r) = self.renderer.as_mut()
            && !self.headless
        {
            r.set_gpu_buffer_motion(b.slot(), keep);
        }
    }
    /// Draws a mesh as `count` GPU instances, using `items[v.instance_index]`
    /// in its custom vertex shader. Every instance is drawn at its entity's
    /// transform and tint, like a normal model: the shader receives the
    /// vertex already placed there (`v.position`, `v.origin` is the entity's
    /// position) and moves it by what it reads from `items`, in world axes. A
    /// particle emitter therefore follows its entity. One draw per entity;
    /// use one entity per emitter. Skinned meshes are unsupported and panic
    /// with a fix: use model entities with `Skeleton` or `SkinOf`. On a GPU
    /// without vertex storage buffers, setup panics with an instruction to
    /// use persistent instance batches (which have a CPU fallback) or entities.
    ///
    /// The entity is culled by the mesh's bounds, which don't know where the
    /// shader puts the instances: give the whole simulation's extent (in the
    /// entity's space, around the entity) with
    /// [`Graphics::set_mesh_gpu_bounds`](crate::gfx::Graphics::set_mesh_gpu_bounds). Disable occlusion culling for
    /// shader-displaced geometry. Unsupported GPUs draw nothing. Panics for
    /// invalid handles or count.
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, mesh: ponko::render::Mesh) {
    /// let b = ctx.gfx.create_gpu_buffer(100, &[]);
    /// ctx.gfx.set_mesh_gpu_buffer(mesh, b, 100);
    /// ctx.gfx.set_mesh_gpu_bounds(mesh, Vec3::ZERO, 10.0);
    /// # }
    /// ```
    pub fn set_mesh_gpu_buffer(&mut self, mesh: super::Mesh, b: GpuBuffer, count: u32) {
        assert!(self.is_live(mesh), "invalid mesh");
        assert!(
            self.assets3d
                .skins
                .get(mesh.slot() as usize)
                .is_none_or(Option::is_none),
            "skinned meshes cannot use GPU-buffer instances: spawn Model entities with Skeleton or SkinOf so each pose is evaluated"
        );
        assert!(
            count <= self.gpu_buffer_len(b).expect("invalid GPU buffer"),
            "GPU instance count out of bounds"
        );
        if let Some(r) = self.renderer.as_mut()
            && !self.headless
        {
            r.set_mesh_gpu_buffer(mesh.slot(), b.slot(), count);
        }
    }
    /// Grows `mesh`'s culling bounds to also hold the sphere at `center`
    /// with `radius`, in the mesh's space: where a buffer-driven
    /// simulation (see [`Graphics::set_mesh_gpu_buffer`](crate::gfx::Graphics::set_mesh_gpu_buffer)) can put its instances.
    /// The result is the mesh's own bounds united with the sphere; calling it
    /// again replaces the sphere. Panics for a freed mesh or a negative or
    /// non-finite radius.
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("b", 1, 1).run_headless(0, |ctx| {
    ///     let material = ctx.gfx.default_material();
    ///     let mesh = ctx.gfx.create_mesh(&ponko::render::MeshData::cube(0.1), material);
    ///     ctx.gfx.set_mesh_gpu_bounds(mesh, Vec3::Y * 5.0, 6.0);
    /// });
    /// ```
    pub fn set_mesh_gpu_bounds(&mut self, mesh: super::Mesh, center: Vec3, radius: f32) {
        assert!(
            radius >= 0.0 && radius.is_finite() && center.is_finite(),
            "invalid bounds"
        );
        assert!(self.is_live(mesh), "invalid mesh");
        let slot = mesh.slot();
        let own = *self
            .assets3d
            .gpu_mesh_bounds
            .entry(slot)
            .or_insert_with(|| self.assets3d.bounds[slot as usize]);
        self.assets3d.bounds[slot as usize] = union_spheres(own, (center, radius));
    }
}

/// The smallest sphere holding both spheres (center, radius).
fn union_spheres(a: (Vec3, f32), b: (Vec3, f32)) -> (Vec3, f32) {
    let d = b.0 - a.0;
    let distance = d.length();
    if distance + b.1 <= a.1 {
        return a;
    }
    if distance + a.1 <= b.1 {
        return b;
    }
    let radius = (distance + a.1 + b.1) * 0.5;
    (a.0 + d * ((radius - a.1) / distance), radius)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    #[test]
    fn gpu_bounds_grow_the_culling_sphere_of_a_buffer_mesh() {
        crate::game::Game::new("bounds", 1, 1).run_headless(0, |ctx| {
            let material = ctx.gfx.default_material();
            let mesh = ctx.gfx.create_mesh(&crate::render::MeshData::cube(0.5), material);
            let own = ctx.gfx.assets3d.bounds[mesh.slot() as usize];
            ctx.gfx.set_mesh_gpu_bounds(mesh, Vec3::Y * 10.0, 2.0);
            let grown = ctx.gfx.assets3d.bounds[mesh.slot() as usize];
            assert!(grown.1 > own.1 + 5.0);
            // Calling again replaces the sphere instead of piling on.
            ctx.gfx.set_mesh_gpu_bounds(mesh, Vec3::Y * 10.0, 2.0);
            assert_eq!(ctx.gfx.assets3d.bounds[mesh.slot() as usize], grown);
            // A freed mesh's slot starts over (under a new id).
            ctx.gfx.free(mesh);
            let again = ctx.gfx.create_mesh(&crate::render::MeshData::cube(0.5), material);
            assert_eq!(again.slot(), mesh.slot());
            assert_ne!(again, mesh);
            assert_eq!(ctx.gfx.assets3d.bounds[again.slot() as usize], own);
        });
    }

    #[test]
    fn scene_compute_passes_are_listed_apart_and_go_with_their_freed_buffer() {
        crate::game::Game::new("stages", 1, 1).run_headless(0, |ctx| {
            let simulate = ctx
                .gfx
                .create_compute("simulate", "fn update(i: u32) { items[i].x += 1.0; }");
            let meter = ctx.gfx.create_compute(
                "meter",
                "fn compute(c: Cell) -> vec4<f32> { return vec4<f32>(scene_color(c.uv * frame.screen.xy), 1.0); }",
            );
            let light = ctx.gfx.create_render_target(4, 4);
            let particles = ctx.gfx.create_gpu_buffer(8, &[]);
            let material = ctx.gfx.default_material();
            ctx.scene3d
                .passes
                .compute
                .push(ComputePass::for_buffer(simulate, particles));
            ctx.scene3d.passes.scene_compute.push(ComputePass::new(meter, light));
            ctx.scene3d
                .passes
                .scene_compute
                .push(ComputePass::for_buffer(simulate, particles));
            ctx.collect_passes(Some(material));
            let frame = &ctx.gfx.frame3d;
            let buffer = particles.slot() | (1 << 31);
            assert_eq!(frame.computes, [(simulate.id, buffer, material.id)]);
            assert_eq!(
                frame.scene_computes,
                [(meter.id, light.id, material.id), (simulate.id, buffer, material.id)]
            );
            // Freeing the buffer takes its passes out of both stages.
            ctx.gfx.free(particles);
            ctx.collect_passes(Some(material));
            assert!(ctx.scene3d.passes.compute.is_empty());
            assert_eq!(ctx.scene3d.passes.scene_compute.len(), 1);
            assert_eq!(ctx.gfx.frame3d.scene_computes, [(meter.id, light.id, material.id)]);
        });
    }

    #[test]
    fn union_holds_both_spheres() {
        let (a, b) = ((vec3(0.0, 0.0, 0.0), 1.0), (vec3(10.0, 0.0, 0.0), 2.0));
        let (c, r) = union_spheres(a, b);
        assert!(c.distance(a.0) + a.1 <= r + 1e-4 && c.distance(b.0) + b.1 <= r + 1e-4);
        assert!((r - 6.5).abs() < 1e-4);
        assert_eq!(union_spheres(a, (vec3(0.5, 0.0, 0.0), 0.1)), a);
        assert_eq!(union_spheres((vec3(0.5, 0.0, 0.0), 0.1), a), a);
    }
}
