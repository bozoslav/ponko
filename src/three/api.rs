//! The 3D methods on [`Ctx`]: making meshes and materials, loading models.

use std::ops::Range;

use glam::Vec3;

use super::{
    AnimationClip, Blend, Material, MaterialDesc, Mesh, MeshData, MeshMorph, MeshSkin, Shader, SharedImage, Skeleton,
    gltf, linear,
};
use crate::gfx::Graphics;
use crate::{assets, ctx::Ctx};
use ponko_ecs::World;

/// A material as the GPU reads it (one uniform block).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct MaterialUniform {
    /// Linear RGBA.
    pub color: [f32; 4],
    /// Linear RGB, then unused.
    pub emissive: [f32; 4],
    /// roughness, metallic, normal map strength, unused.
    pub params: [f32; 4],
    /// Which maps it has (1/0): normal, ORM, emissive, unused. A
    /// two-channel normal map is 2 (X and Y in red and green) or 3 (in red
    /// and alpha): the shader rebuilds Z.
    pub maps: [u32; 4],
    /// For custom shaders.
    pub custom: [[f32; 4]; 4],
}

/// A material's textures, in binding order: base color, normal map, ORM
/// map, emissive map and two for custom shaders. `None`: the default.
pub(crate) type MaterialTextures = [Option<u32>; 6];

/// Everything the GPU needs to make or change a material.
pub(crate) struct GpuMaterialDesc {
    pub uniform: MaterialUniform,
    pub textures: MaterialTextures,
    /// Texture arrays for custom shaders, by id (`None`: a white one).
    pub arrays: [Option<u32>; 3],
    /// Index into the renderer's shaders (0: the built-in one).
    pub shader: u32,
    pub transparent: bool,
    pub cutout: bool,
    pub double_sided: bool,
}

impl GpuMaterialDesc {
    pub(crate) fn new(desc: &MaterialDesc) -> Self {
        let [r, g, b, _] = linear(desc.emissive);
        let id = |t: Option<crate::texture::Texture>| t.map(|t| t.id);
        Self {
            uniform: MaterialUniform {
                color: linear(desc.color),
                emissive: [r, g, b, 0.0],
                params: [desc.roughness.max(0.03), desc.metallic, desc.normal_strength, 0.0],
                maps: [
                    u32::from(desc.normal_map.is_some()),
                    u32::from(desc.orm_map.is_some()),
                    u32::from(desc.emissive_map.is_some()),
                    0,
                ],
                custom: desc.custom.map(|v| v.to_array()),
            },
            textures: [
                id(desc.texture),
                id(desc.normal_map),
                id(desc.orm_map),
                id(desc.emissive_map),
                id(desc.custom_textures[0]),
                id(desc.custom_textures[1]),
            ],
            arrays: desc.custom_arrays.map(|a| a.map(|a| a.id)),
            shader: desc.shader.map_or(0, |s| s.id + 1),
            transparent: desc.blend == Blend::Transparent,
            cutout: desc.blend == Blend::Cutout,
            double_sided: desc.double_sided,
        }
    }
}

impl Graphics {
    /// Puts a material on the GPU: a color (and texture), how rough and how
    /// metallic, and light it gives off.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let bricks = ctx.load_texture("bricks.png");
    /// let wall = ctx.gfx.create_material(ponko::render::MaterialDesc::default().with_texture(bricks).with_roughness(0.9));
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If roughness or metallic is outside 0..=1.
    pub fn create_material(&mut self, desc: MaterialDesc) -> Material {
        self.try_create_material(desc).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`create_material`](Self::create_material) with errors as values:
    /// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) if roughness or
    /// metallic is outside 0..=1, [`BadHandle`](crate::error::ErrorKind::BadHandle)
    /// if its shader or texture arrays weren't made by this game.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_create_material(ponko::render::MaterialDesc::default().with_roughness(2.0)).unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::Invalid);
    /// });
    /// ```
    pub fn try_create_material(&mut self, desc: MaterialDesc) -> Result<Material, crate::error::Error> {
        self.check_material(&desc)?;
        let id = self.assets3d.materials;
        self.assets3d.materials += 1;
        self.assets3d.materials_live.insert(id);
        self.asset_ledger.made(crate::inventory::AssetClass::Material, id);
        let gpu = self.gpu_material(&desc);
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            let gpu_id = renderer.create_material(&gpu);
            debug_assert_eq!(gpu_id, id);
        }
        Ok(Material { id })
    }

    /// Changes a material: every model drawn with it changes too. Handy for
    /// animating a custom shader's values, or a glow.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, lamp: ponko::render::Material) {
    /// let on = ponko::render::MaterialDesc::default().with_emissive(Color::hex(0xFFD080));
    /// ctx.gfx.update_material(lamp, on);
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// As [`Graphics::create_material`](crate::gfx::Graphics::create_material), or if the material comes from another
    /// game. Updating a freed material does nothing: it stays freed.
    pub fn update_material(&mut self, material: Material, desc: MaterialDesc) {
        assert!(
            material.id < self.assets3d.materials,
            "ponko: update_material: that material wasn't made by this game"
        );
        self.check_material(&desc).unwrap_or_else(|e| panic!("{e}"));
        if !self.assets3d.materials_live.contains(material.id) {
            return;
        }
        let gpu = self.gpu_material(&desc);
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.update_material(material.id, &gpu, &self.assets3d.instance_batches);
        }
    }

    /// What the GPU makes of `desc`: freed shaders, textures and arrays
    /// are left out, so it looks as it would without them.
    fn gpu_material(&self, desc: &MaterialDesc) -> GpuMaterialDesc {
        let mut gpu = GpuMaterialDesc::new(desc);
        if desc.shader.is_some_and(|s| !self.assets3d.shaders_live.contains(s.id)) {
            gpu.shader = 0;
        }
        let freed = |id: u32| self.textures_live.was_made(id) && !self.textures_live.contains(id);
        for (slot, texture) in gpu.textures.iter_mut().enumerate() {
            if texture.is_some_and(freed) {
                *texture = None;
                // Slots 1 to 3 (normal, ORM, emissive) have flags.
                if (1..=3).contains(&slot) {
                    gpu.uniform.maps[slot - 1] = 0;
                }
            }
        }
        for array in &mut gpu.arrays {
            if array.is_some_and(|a| !self.assets3d.arrays_live.contains(a)) {
                *array = None;
            }
        }
        #[cfg(feature = "compressed-textures")]
        if let Some(normal) = gpu.textures[1] {
            gpu.uniform.maps[0] = match self.compressed.get(&normal) {
                Some(crate::compressed::NormalChannels::Rg) => 2,
                Some(crate::compressed::NormalChannels::Ra) => 3,
                _ => 1,
            };
        }
        gpu
    }

    /// Adds the live 3D handles to `counts` (see [`Ctx::live_counts`]).
    pub(crate) fn count_live_3d(&self, counts: &mut crate::handle::LiveCounts) {
        let a = &self.assets3d;
        counts.render_targets = a
            .render_targets
            .iter()
            .filter(|t| self.textures_live.contains(t.id))
            .count();
        counts.meshes = a.bounds.iter().filter(|&&(_, r)| r >= 0.0).count();
        counts.materials = a.materials_live.len();
        counts.shaders = a.shaders_live.len();
        counts.computes = a.computes_live.len();
        counts.effects = a.effects_live.len();
        counts.gpu_buffers = a.gpu_buffers.iter().filter(|&&len| len > 0).count();
        counts.instance_batches = a.instance_batches.iter().flatten().count();
        counts.texture_arrays = a.arrays_live.len();
    }

    /// Frees material `id` (see [`Ctx::free`]): mesh parts made with it
    /// draw with the default material's look from now on.
    pub(crate) fn free_material_id(&mut self, id: u32) {
        if !self.assets3d.materials_live.remove(id) {
            return;
        }
        self.asset_ledger.freed(crate::inventory::AssetClass::Material, id);
        if self.assets3d.default_material == Some(Material { id }) {
            self.assets3d.default_material = None;
        }
        self.material_names.remove(id);
        for materials in self.assets3d.loaded_materials.values_mut() {
            materials.retain(|(m, _)| m.id != id);
        }
        self.assets3d.loaded_materials.retain(|_, m| !m.is_empty());
        // A file whose material was freed makes only that one again when
        // reloaded (`file_materials`).
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_material(id);
        }
    }

    /// Frees material shader `id` (see [`Ctx::free`]): its materials draw
    /// with the built-in shader from now on.
    pub(crate) fn free_shader_id(&mut self, id: u32) {
        if !self.assets3d.shaders_live.remove(id) {
            return;
        }
        self.asset_ledger.freed(crate::inventory::AssetClass::Shader, id);
        self.assets3d.loaded_shaders.retain(|_, s| s.id != id);
        if self.assets3d.terrain_shader == Some(Shader { id }) {
            self.assets3d.terrain_shader = None;
        }
        self.watcher
            .forget(|a| matches!(a, crate::hot_reload::Asset::Shader(s) if *s == id));
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_shader(id + 1);
        }
    }

    /// Builds, now, the render pipelines the renderer would otherwise build
    /// the first time a frame draws them: the variants of every material
    /// shader for crossfading [`LodGroup`](crate::render::LodGroup) levels and for
    /// buffer-driven instances. Each takes several milliseconds, a visible
    /// hitch if it happens mid-game (the first time a crossfade shows). Call
    /// it at the end of loading; shaders made afterwards then build theirs
    /// when they're made. Startup takes longer, so games that use neither
    /// can skip it. [`RenderStats::pipelines_built`](crate::render::RenderStats)
    /// counts the ones still built while drawing. Does nothing in headless runs.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// // ... load meshes, materials and shaders ...
    /// ctx.gfx.warm_render_pipelines();
    /// # }
    /// ```
    pub fn warm_render_pipelines(&mut self) {
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.warm_pipelines();
        }
    }

    fn check_material(&self, desc: &MaterialDesc) -> Result<(), crate::error::Error> {
        use crate::error::{Error, ErrorKind};
        if !((0.0..=1.0).contains(&desc.roughness) && (0.0..=1.0).contains(&desc.metallic)) {
            return Err(Error::new(
                ErrorKind::Invalid,
                format!(
                    "material roughness and metallic go from 0 to 1 (got {} and {})",
                    desc.roughness, desc.metallic
                ),
            ));
        }
        // Freed handles are allowed: they draw as the default.
        if let Some(shader) = desc.shader
            && shader.id >= self.assets3d.shaders
        {
            return Err(Error::new(ErrorKind::BadHandle, "that shader wasn't made by this game"));
        }
        for array in desc.custom_arrays.into_iter().flatten() {
            if self.assets3d.arrays.get(array.id as usize) != Some(&array) {
                return Err(Error::new(
                    ErrorKind::BadHandle,
                    "that texture array wasn't made by this game",
                ));
            }
        }
        Ok(())
    }

    /// Loads a material shader (WGSL) from an [added asset
    /// source](crate::assets::Assets::add_source) or `assets/`. It can define
    /// `fn vertex(v: Vertex) -> Vertex`, to move vertices (wind, waves), and
    /// `fn surface(s: Surface) -> Surface`, to change how each pixel looks;
    /// the engine lights it, shadows it and does the rest. Hooks are ordinary
    /// functions with exactly those types (parameter names may differ), not
    /// stage entry points. All `@group`/`@binding` declarations are engine-owned;
    /// use the material's custom values/textures or `items` instead. Contract
    /// errors name the expected rule and fix in [`Graphics::shader_status`].
    /// Set `s.unlit = true` in `surface` for albedo plus emissive without
    /// lighting; alpha, face mode, skinning and instancing still work as usual.
    /// With hot reload
    /// on, saving the file in `assets/` updates the game (an error keeps the old one).
    /// Loading the same file again returns the same shader.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let wind = ctx.gfx.load_shader("shaders/wind.wgsl");
    /// let leaves = ctx.gfx.create_material(ponko::render::MaterialDesc::default().with_shader(wind).with_double_sided(true));
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If the file can't be read or doesn't compile (the message says where).
    /// With hot reload on, a file that doesn't compile doesn't panic: it
    /// gets a stand-in and stays watched until a saved fix compiles
    /// ([`ShaderOutcome::Failed`](crate::render::ShaderOutcome::Failed), [`Graphics::shader_status`](crate::gfx::Graphics::shader_status)).
    pub fn load_shader(&mut self, path: &str) -> Shader {
        self.try_load_shader(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_shader`](Self::load_shader) with errors as values:
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file is
    /// missing, [`Io`](crate::error::ErrorKind::Io) if it can't be read,
    /// [`Decode`](crate::error::ErrorKind::Decode) if it isn't UTF-8 text and
    /// [`Shader`](crate::error::ErrorKind::Shader) if it doesn't compile (the
    /// message has the compiler's log). With hot reload on, a file that
    /// doesn't compile still returns a stand-in, as `load_shader` does.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_load_shader("shaders/missing.wgsl").unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::NotFound);
    /// });
    /// ```
    pub fn try_load_shader(&mut self, path: &str) -> Result<Shader, crate::error::Error> {
        let found = match self
            .asset_sources
            .lookup(path, |origin| self.assets3d.loaded_shaders.get(origin).copied())?
        {
            crate::assets::Lookup::Cached(_, shader) => return Ok(shader),
            crate::assets::Lookup::Found(found) => found,
        };
        let origin = found.origin.clone();
        let source = found.into_text(path)?;
        let shader = match self.try_create_shader(path, &source) {
            Ok(shader) => shader,
            // With hot reload on, it draws with the built-in look until a
            // saved fix compiles; its status says what's wrong.
            Err(e) if self.keeps_failed_load(&origin) => {
                let e = e.with_path(path);
                Self::report_stand_in(path, &e);
                let id = self.assets3d.shaders;
                self.compile_shader(path, "", None).map_err(|_| e)?;
                self.assets3d.shaders += 1;
                self.assets3d.shaders_live.insert(id);
                self.asset_ledger.made(crate::inventory::AssetClass::Shader, id);
                Shader { id }
            }
            Err(e) => return Err(e.with_path(path)),
        };
        self.assets3d.loaded_shaders.insert(origin.clone(), shader);
        self.asset_ledger
            .own(crate::inventory::AssetClass::Shader, shader.id, path);
        if let crate::assets::Origin::File(file) = origin {
            self.watcher
                .watch(path, file, crate::hot_reload::Asset::Shader(shader.id));
        }
        Ok(shader)
    }

    /// Makes a material shader from WGSL source; `name` is for error
    /// messages. See [`Graphics::load_shader`](crate::gfx::Graphics::load_shader) for what it can define.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let stripes = ctx.gfx.create_shader("stripes", "
    ///     fn surface(s: Surface) -> Surface {
    ///         var out = s;
    ///         out.albedo *= step(0.5, fract(s.uv.x * 8.0));
    ///         return out;
    ///     }");
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If it doesn't compile (the message says where).
    pub fn create_shader(&mut self, name: &str, source: &str) -> Shader {
        self.try_create_shader(name, source).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`create_shader`](Self::create_shader) with errors as values:
    /// [`ErrorKind::Shader`](crate::error::ErrorKind::Shader) if it doesn't
    /// compile, with the compiler's log in the message.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_create_shader("broken", "fn surface(s: Surface) -> Surface { return 1; }").unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::Shader);
    /// });
    /// ```
    pub fn try_create_shader(&mut self, name: &str, source: &str) -> Result<Shader, crate::error::Error> {
        let id = self.assets3d.shaders;
        let compiled = self.compile_shader(name, source, None);
        self.record_shader(name, super::ShaderKind::Material, false, compiled)?;
        self.assets3d.shaders += 1;
        self.assets3d.shaders_live.insert(id);
        self.asset_ledger.made(crate::inventory::AssetClass::Shader, id);
        Ok(Shader { id })
    }

    /// Recompiles a shader whose file changed (hot reload). On an error the
    /// last good version stays in use.
    pub(crate) fn reload_shader(&mut self, id: u32, name: &str, source: &str) -> Result<(), String> {
        if !self.assets3d.shaders_live.contains(id) {
            return Ok(());
        }
        let compiled = self.compile_shader(name, source, Some(id));
        Ok(self.record_shader(name, super::ShaderKind::Material, true, compiled)?)
    }

    /// Checks material shader `source` and builds its pipelines, as shader
    /// `replace` or as the next one.
    fn compile_shader(
        &mut self,
        name: &str,
        source: &str,
        replace: Option<u32>,
    ) -> Result<(), Box<super::ShaderDiagnostic>> {
        #[cfg(feature = "window")]
        crate::render::diagnose(
            name,
            super::ShaderKind::Material,
            crate::render::FULL_MATERIAL_VARIANT,
            &crate::render::compose_shader(source),
        )?;
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            match replace {
                Some(id) => renderer.replace_shader(id + 1, name, source)?,
                None => {
                    let gpu_id = renderer.create_shader(name, source)?;
                    debug_assert_eq!(gpu_id, self.assets3d.shaders + 1);
                }
            }
        }
        let _ = (name, source, replace);
        Ok(())
    }

    /// A plain white material, made once and shared.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let white = ctx.gfx.default_material();
    /// let ball = ctx.gfx.create_mesh(&ponko::render::MeshData::sphere(0.5, 32), white);
    /// # }
    /// ```
    pub fn default_material(&mut self) -> Material {
        if let Some(m) = self.assets3d.default_material {
            return m;
        }
        let m = self.create_material(MaterialDesc::default());
        self.assets3d.default_material = Some(m);
        m
    }

    /// Puts triangles on the GPU as a [`Mesh`] of one material. Draw it by
    /// spawning entities with a [`Model`](crate::render::Model) of it.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let stone = ctx.gfx.create_material(ponko::render::MaterialDesc::color(Color::hex(0x8A8A8A)));
    /// let block = ctx.gfx.create_mesh(&ponko::render::MeshData::cube(1.0), stone);
    /// ctx.world.spawn((ponko::d3::Transform::default(), ponko::render::Model::new(block)));
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If the data doesn't fit together (see [`MeshData::check`]), or the
    /// material comes from another game.
    pub fn create_mesh(&mut self, data: &MeshData, material: Material) -> Mesh {
        self.try_create_mesh(data, material).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`create_mesh`](Self::create_mesh) with errors as values:
    /// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) if the data doesn't
    /// fit together, [`BadHandle`](crate::error::ErrorKind::BadHandle) if the
    /// material comes from another game.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let stone = ctx.gfx.create_material(ponko::render::MaterialDesc::default());
    ///     let mut broken = ponko::render::MeshData::cube(1.0);
    ///     broken.indices.pop();
    ///     assert_eq!(ctx.gfx.try_create_mesh(&broken, stone).unwrap_err().kind(), ErrorKind::Invalid);
    /// });
    /// ```
    pub fn try_create_mesh(&mut self, data: &MeshData, material: Material) -> Result<Mesh, crate::error::Error> {
        use crate::error::{Error, ErrorKind};
        data.problem()
            .map_err(|e| Error::new(ErrorKind::Invalid, format!("create_mesh: {e}")))?;
        if material.id >= self.assets3d.materials {
            return Err(Error::new(
                ErrorKind::BadHandle,
                "create_mesh: that material wasn't made by this game",
            ));
        }
        Ok(self.mesh_from_parts(data, &[(0..data.indices.len() as u32, material)], None, Vec::new()))
    }

    /// Puts triangles on the GPU as a [`Mesh`] with no material of its own:
    /// [`create_mesh`](Self::create_mesh) with the
    /// [default material](Self::default_material). For meshes whose look
    /// comes from each model, with a [`ModelMaterial`](crate::render::ModelMaterial):
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let ball = ctx.gfx.create_plain_mesh(&ponko::render::MeshData::sphere(0.5, 32));
    /// let gold = ctx.gfx.create_material(ponko::render::MaterialDesc::color(Color::hex(0xFFC83D)).with_metallic(1.0));
    /// ctx.world.spawn((ponko::d3::Transform::default(), ponko::render::Model::new(ball)));                        // plain white
    /// ctx.world.spawn((ponko::d3::Transform::at(Vec3::X), ponko::render::Model::new(ball), ponko::render::ModelMaterial(gold))); // gold
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If the data doesn't fit together (see [`MeshData::check`]).
    pub fn create_plain_mesh(&mut self, data: &MeshData) -> Mesh {
        self.try_create_plain_mesh(data).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`create_plain_mesh`](Self::create_plain_mesh) with errors as values:
    /// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) if the data doesn't
    /// fit together.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let cube = ctx.gfx.try_create_plain_mesh(&ponko::render::MeshData::cube(1.0)).unwrap();
    ///     assert!(ctx.gfx.is_live(cube));
    ///     let mut broken = ponko::render::MeshData::cube(1.0);
    ///     broken.indices.pop();
    ///     assert_eq!(ctx.gfx.try_create_plain_mesh(&broken).unwrap_err().kind(), ErrorKind::Invalid);
    /// });
    /// ```
    pub fn try_create_plain_mesh(&mut self, data: &MeshData) -> Result<Mesh, crate::error::Error> {
        // Checked first, so bad data doesn't make the default material.
        data.problem().map_err(|e| {
            crate::error::Error::new(crate::error::ErrorKind::Invalid, format!("create_plain_mesh: {e}"))
        })?;
        let material = self.default_material();
        self.try_create_mesh(data, material)
    }

    /// Makes a mesh that bends with `skeleton`: each corner of `data` has
    /// up to four [`joints`](MeshData::joints) (indices into the
    /// skeleton's joints) and how much each moves it. `data` is modelled
    /// with the skeleton at rest. Draw it with a [`Model`](crate::render::Model)
    /// and a [`Skeleton`] (this one, or any made from the same joints, or
    /// with joints of the same names) on the entity.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let skeleton = ponko::anim::Skeleton::new(vec![
    ///         ponko::anim::Joint { name: "base".into(), parent: None, rest: ponko::d3::Transform::default() },
    ///         ponko::anim::Joint { name: "top".into(), parent: Some(0), rest: ponko::d3::Transform::at(vec3(0.0, 1.0, 0.0)) },
    ///     ]);
    ///     let mut post = ponko::render::MeshData::cube(0.2);
    ///     // Corners above the middle follow the top joint.
    ///     post.joints = post.positions.iter().map(|p| [if p.y > 0.0 { 1 } else { 0 }, 0, 0, 0]).collect();
    ///     post.weights = vec![[1.0, 0.0, 0.0, 0.0]; post.positions.len()];
    ///     let material = ctx.gfx.default_material();
    ///     let mesh = ctx.gfx.create_skinned_mesh(&post, material, &skeleton);
    ///     ctx.world.spawn((ponko::d3::Transform::default(), ponko::render::Model::new(mesh), skeleton));
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// If the data doesn't fit together (see [`MeshData::check`]), has no
    /// joints, names a joint the skeleton doesn't have, or the material
    /// comes from another game.
    pub fn create_skinned_mesh(&mut self, data: &MeshData, material: Material, skeleton: &Skeleton) -> Mesh {
        if let Err(e) = data.problem() {
            panic!("ponko: create_skinned_mesh: {e}");
        }
        assert!(
            !data.joints.is_empty(),
            "ponko: create_skinned_mesh: the mesh data has no joints (use create_mesh)"
        );
        let n = skeleton.joints().len();
        if let Some(bad) = data.joints.iter().flatten().find(|&&j| usize::from(j) >= n) {
            panic!("ponko: create_skinned_mesh: joint {bad} is past the skeleton's {n} joints");
        }
        assert!(
            material.id < self.assets3d.materials,
            "ponko: create_skinned_mesh: that material wasn't made by this game"
        );
        let rest = Skeleton::from_shared(skeleton.shared_joints().clone());
        let inverse_binds = (0..n).map(|j| rest.joint_matrix(j).inverse()).collect();
        let mut data = data.clone();
        for w in &mut data.weights {
            *w = super::skin::normalized(*w);
        }
        let skin = MeshSkin::new(
            skeleton.shared_joints().clone(),
            (0..n as u16).collect(),
            inverse_binds,
            &data,
        );
        self.mesh_from_parts(
            &data,
            &[(0..data.indices.len() as u32, material)],
            Some(skin),
            Vec::new(),
        )
    }

    /// The [`Skeleton`] of a glTF file with skins (loading its mesh with
    /// [`load_mesh`](Self::load_mesh) if that hasn't happened), at rest:
    /// every node of the file's scene is a joint, named as in the file.
    /// Put it on the entity with the file's [`Model`](crate::render::Model) and
    /// change its `pose` to bend the model.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let fox = ctx.gfx.load_mesh("models/fox.glb");
    /// let skeleton = ctx.gfx.load_skeleton("models/fox.glb");
    /// ctx.world.spawn((ponko::d3::Transform::default(), ponko::render::Model::new(fox), skeleton));
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// With a clear message if the file is missing, can't be read or has
    /// no skins.
    pub fn load_skeleton(&mut self, path: &str) -> Skeleton {
        self.try_load_skeleton(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_skeleton`](Self::load_skeleton) with errors as values: those
    /// of [`try_load_mesh`](Self::try_load_mesh), or
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file has
    /// no skins.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_load_skeleton("models/missing.glb").unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::NotFound);
    /// });
    /// ```
    pub fn try_load_skeleton(&mut self, path: &str) -> Result<Skeleton, crate::error::Error> {
        let (_, origin) = self.load_mesh_from(path)?;
        self.assets3d.loaded_skeletons.get(&origin).cloned().ok_or_else(|| {
            crate::error::Error::new(
                crate::error::ErrorKind::NotFound,
                format!("{path} has no skins or animations, so no skeleton"),
            )
            .with_path(path)
        })
    }

    /// Loads all animation clips from a glTF file, cached per file.
    /// Panics with the loading error if the file cannot be loaded.
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let clips = ctx.gfx.load_clips("models/character.glb");
    /// # }
    /// ```
    pub fn load_clips(&mut self, path: &str) -> Vec<AnimationClip> {
        self.try_load_clips(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_clips`](Self::load_clips) with errors as values: those of
    /// [`try_load_mesh`](Self::try_load_mesh), and
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) for a file without
    /// skins or animations.
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_load_clips("models/missing.glb").unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::NotFound);
    /// });
    /// ```
    pub fn try_load_clips(&mut self, path: &str) -> Result<Vec<AnimationClip>, crate::error::Error> {
        let (_, origin) = self.load_mesh_from(path)?;
        self.assets3d
            .loaded_skeletons
            .contains_key(&origin)
            .then(|| self.assets3d.loaded_clips.get(&origin).cloned().unwrap_or_default())
            .ok_or_else(|| {
                crate::error::Error::new(
                    crate::error::ErrorKind::NotFound,
                    format!("{path} has no skins or animations, so no skeleton"),
                )
                .with_path(path)
            })
    }

    /// Loads a named clip. Panics with available names if it is missing.
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let walk = ctx.gfx.load_clip("models/character.glb", "walk");
    /// # }
    /// ```
    pub fn load_clip(&mut self, path: &str, name: &str) -> AnimationClip {
        self.try_load_clip(path, name).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_clip`](Self::load_clip) with errors as values: those of
    /// [`try_load_clips`](Self::try_load_clips), and
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file has no
    /// clip called `name` (the message lists the ones it has).
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let walk = ctx.gfx.try_load_clip("models/character.glb", "walk");
    /// # }
    /// ```
    pub fn try_load_clip(&mut self, path: &str, name: &str) -> Result<AnimationClip, crate::error::Error> {
        let clips = self.try_load_clips(path)?;
        clips.iter().find(|c| c.name() == name).cloned().ok_or_else(|| {
            let names: Vec<_> = clips.iter().map(AnimationClip::name).collect();
            let message = format!("{path}: no animation {name:?}; available animations: {names:?}");
            crate::error::Error::new(crate::error::ErrorKind::NotFound, message).with_path(path)
        })
    }

    /// Loads a glTF model (`.glb`, or `.gltf` with its files next to it)
    /// from an [added asset source](crate::assets::Assets::add_source) or `assets/` as
    /// one [`Mesh`]: every mesh in the file's scene, where the file puts it, with its materials and their base color, normal,
    /// occlusion-roughness-metalness and emissive maps.
    /// Loading the same file again returns the same mesh. After freeing its
    /// geometry, loading it again reuses its materials and textures, keeping
    /// edits made with [`update_material`](Self::update_material).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Forest", 1280, 720).run(|ctx| {
    ///     let tree = ctx.gfx.load_mesh("models/tree.glb");
    ///     for i in 0..100 {
    ///         let at = vec3((i % 10) as f32 * 4.0, 0.0, (i / 10) as f32 * -4.0);
    ///         ctx.world.spawn((ponko::d3::Transform::at(at), ponko::render::Model::new(tree)));
    ///     }
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// With a clear message if the file is missing or can't be read.
    pub fn load_mesh(&mut self, path: &str) -> Mesh {
        self.try_load_mesh(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_mesh`](Self::load_mesh) with errors as values:
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file is
    /// missing, [`Io`](crate::error::ErrorKind::Io) if it can't be read,
    /// [`Decode`](crate::error::ErrorKind::Decode) if it isn't a glTF model this
    /// engine can read (including files it names that are missing).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let hat = ctx.gfx.try_load_mesh("models/hat.glb").unwrap_or(ponko::render::Mesh::NONE);
    ///     assert_eq!(hat, ponko::render::Mesh::NONE);
    /// });
    /// ```
    pub fn try_load_mesh(&mut self, path: &str) -> Result<Mesh, crate::error::Error> {
        self.load_mesh_from(path).map(|(mesh, _)| mesh)
    }

    /// Loads (or returns the cached) mesh `path`, and says where it was
    /// found: the key of its skeleton, clips and materials.
    pub(crate) fn load_mesh_from(&mut self, path: &str) -> Result<(Mesh, assets::Origin), crate::error::Error> {
        let found = match self
            .asset_sources
            .lookup(path, |origin| self.assets3d.loaded.get(origin).copied())?
        {
            assets::Lookup::Cached(origin, mesh) => return Ok((mesh, origin)),
            assets::Lookup::Found(found) => found,
        };
        let file = found.origin.clone();
        let bytes = found.into_bytes().map_err(|e| e.with_path(path))?;
        let (loaded, image_keys) = self.decode_gltf(path, &bytes)?;
        Ok((self.mesh_from_loaded(path, file.clone(), loaded, image_keys), file))
    }

    /// Decodes the glTF file `path` from `bytes` on the game thread, without
    /// the pixels of images another model already shares, and works out
    /// each image's key (see [`image_keys`](Self::image_keys)).
    pub(crate) fn decode_gltf(
        &self,
        path: &str,
        bytes: &[u8],
    ) -> Result<(gltf::Loaded, Vec<Option<SharedImage>>), crate::error::Error> {
        let shared = &self.assets3d.shared_images;
        let sources = &self.asset_sources;
        // Each image file's key: where it is found, so the same name in two
        // sources is two images. Worked out once per file.
        let keys = std::cell::RefCell::new(std::collections::HashMap::<String, Option<SharedImage>>::new());
        let key_of = |source: &str| {
            keys.borrow_mut()
                .entry(source.to_string())
                .or_insert_with(|| SharedImage::of(sources, source))
                .clone()
        };
        let loaded = gltf::load(path, bytes, &mut |name| sources.read(name), &|source| {
            key_of(source).is_some_and(|key| shared.contains_key(&key))
        })
        .map_err(|e| crate::error::Error::decode(path, e))?;
        let image_keys = loaded
            .image_sources
            .iter()
            .map(|source| source.as_deref().and_then(key_of))
            .collect();
        Ok((loaded, image_keys))
    }

    /// The shared-image key of each image of a decoded glTF file (a file
    /// decoded on a worker, which can't look at the asset sources).
    pub(crate) fn image_keys(&self, loaded: &gltf::Loaded) -> Vec<Option<SharedImage>> {
        loaded
            .image_sources
            .iter()
            .map(|source| {
                source
                    .as_deref()
                    .and_then(|source| SharedImage::of(&self.asset_sources, source))
            })
            .collect()
    }

    /// Makes the mesh, materials and textures of a decoded glTF file
    /// `path` found at `file`, and caches it (the game-thread half of
    /// `load_mesh_from`; streaming decodes on a worker first). `image_keys`
    /// has the shared-image key of each of the file's images.
    pub(crate) fn mesh_from_loaded(
        &mut self,
        path: &str,
        file: assets::Origin,
        loaded: gltf::Loaded,
        image_keys: Vec<Option<SharedImage>>,
    ) -> Mesh {
        let materials = self.file_materials(path, &file, &loaded, image_keys);
        let parts: Vec<(Range<u32>, Material)> = loaded
            .parts
            .iter()
            .map(|(range, m)| (range.clone(), materials[*m]))
            .collect();
        self.assets3d.loaded_clips.insert(file.clone(), loaded.clips);
        let skin = loaded.skin.map(|skin| {
            let skeleton = Skeleton::from_shared(skin.joints);
            let mesh_skin = MeshSkin::new(
                skeleton.shared_joints().clone(),
                skin.slots,
                skin.inverse_binds,
                &loaded.data,
            );
            self.assets3d.loaded_skeletons.insert(file.clone(), skeleton);
            mesh_skin
        });
        let mesh = self.mesh_from_parts(&loaded.data, &parts, skin, loaded.morph_defaults);
        self.assets3d.loaded.insert(file.clone(), mesh);
        self.asset_ledger.own(crate::inventory::AssetClass::Mesh, mesh.id, path);
        self.mesh_names.set(mesh.id, path, mesh);
        let named = materials
            .iter()
            .zip(&loaded.materials)
            .map(|(m, l)| (*m, l.name.clone()));
        self.material_names.add(path, named);
        mesh
    }

    /// The materials of glTF file `file` (loaded as `path`): the ones made
    /// when it was first loaded, which survive freeing its mesh and keep
    /// edits made with `update_material`. Only those the game freed since
    /// are made again, from the file's textures that are still live, so a
    /// file loaded, partly freed and loaded again never leaves unreachable
    /// materials or textures behind.
    fn file_materials(
        &mut self,
        path: &str,
        file: &assets::Origin,
        loaded: &gltf::Loaded,
        image_keys: Vec<Option<SharedImage>>,
    ) -> Vec<Material> {
        let live = |gfx: &Self, m: &Material| gfx.assets3d.materials_live.contains(m.id);
        let kept = self.assets3d.file_materials.get(file).cloned().unwrap_or_default();
        if kept.len() == loaded.materials.len() && kept.iter().all(|m| live(self, m)) {
            return kept;
        }
        let old_textures = self.assets3d.file_textures.get(file).cloned().unwrap_or_default();
        let textures: Vec<crate::texture::Texture> = loaded
            .images
            .iter()
            .zip(image_keys)
            .enumerate()
            .map(|(i, ((w, h, rgba), key))| {
                if let Some(&t) = old_textures.get(i).filter(|t| self.textures_live.contains(t.id)) {
                    return t;
                }
                match key {
                    Some(key) => match self.assets3d.shared_images.get(&key) {
                        Some(&t) => t,
                        None => {
                            let t = self.create_texture(*w, *h, rgba);
                            self.assets3d.shared_images.insert(key, t);
                            t
                        }
                    },
                    None => self.create_texture(*w, *h, rgba),
                }
            })
            .collect();
        for t in &textures {
            self.asset_ledger.own(crate::inventory::AssetClass::Texture, t.id, path);
        }
        self.assets3d.file_textures.insert(file.clone(), textures.clone());
        let materials: Vec<Material> = loaded
            .materials
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if let Some(&old) = kept.get(i).filter(|old| live(self, old)) {
                    return old;
                }
                let texture = |i: Option<usize>| i.map(|i| textures[i]);
                let desc = MaterialDesc {
                    color: m.color,
                    texture: texture(m.image),
                    roughness: m.roughness.clamp(0.0, 1.0),
                    metallic: m.metallic.clamp(0.0, 1.0),
                    emissive: m.emissive,
                    normal_map: texture(m.normal_map),
                    normal_strength: m.normal_strength,
                    orm_map: texture(m.orm_map),
                    emissive_map: texture(m.emissive_map),
                    blend: m.blend,
                    double_sided: m.double_sided,
                    ..Default::default()
                };
                let material = self.create_material(desc);
                self.asset_ledger
                    .own(crate::inventory::AssetClass::Material, material.id, path);
                if !m.name.is_empty() {
                    self.assets3d
                        .loaded_materials
                        .entry(m.name.clone())
                        .or_default()
                        .push((material, desc));
                }
                material
            })
            .collect();
        self.assets3d.file_materials.insert(file.clone(), materials.clone());
        materials
    }

    /// Frees everything loading glTF file `path` made: its mesh, its
    /// materials and its textures, except images another loaded file
    /// shares. For a long-running game that loads many different models:
    /// freeing only the mesh keeps the materials and textures for the next
    /// load of the same file. Whatever still shows them draws with the
    /// default, as [`free`](Self::free) describes. False if `path` was not
    /// loaded (or was unloaded already).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let before = ctx.live_counts();
    ///     let house = ctx.gfx.load_mesh("models/house.glb");
    ///     ctx.gfx.free(house);
    ///     assert_ne!(ctx.live_counts(), before, "its materials and textures stay");
    ///     assert!(ctx.gfx.unload_mesh("models/house.glb"));
    ///     assert_eq!(ctx.live_counts(), before);
    ///     assert!(!ctx.gfx.unload_mesh("models/house.glb"));
    /// });
    /// ```
    pub fn unload_mesh(&mut self, path: &str) -> bool {
        let Some(file) = self.asset_sources.locate(path) else {
            return false;
        };
        let mesh = self.assets3d.loaded.get(&file).copied();
        let materials = self.assets3d.file_materials.remove(&file);
        let textures = self.assets3d.file_textures.remove(&file);
        if mesh.is_none() && materials.is_none() && textures.is_none() {
            return false;
        }
        if let Some(mesh) = mesh {
            self.free(mesh);
        }
        for material in materials.unwrap_or_default() {
            self.free(material);
        }
        for texture in textures.unwrap_or_default() {
            let shared = self.assets3d.file_textures.values().any(|ts| ts.contains(&texture));
            if !shared {
                self.free(texture);
            }
        }
        self.material_names.forget_file(path);
        true
    }

    /// A glTF file's mesh on the CPU (all its parts in one, placed as in
    /// the file), without making anything on the GPU: to cut
    /// [decals](MeshData::decal) from, build collision shapes, or read its
    /// corners. Loads the file each call; keep the result.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let wall = ctx.gfx.load_mesh_data("models/wall.glb");
    /// let hole = wall.decal(Mat4::from_scale(Vec3::splat(0.1)), 0.002);
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If the file is missing or isn't a glTF file with triangles; use
    /// [`try_load_mesh_data`](Self::try_load_mesh_data) to handle that.
    pub fn load_mesh_data(&self, path: &str) -> MeshData {
        self.try_load_mesh_data(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_mesh_data`](Self::load_mesh_data) with errors as values:
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file is
    /// missing, [`Io`](crate::error::ErrorKind::Io) if it can't be read,
    /// [`Decode`](crate::error::ErrorKind::Decode) if it isn't a glTF file with
    /// triangles (including files it names that are missing).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// fn wall(ctx: &Ctx) -> Result<ponko::render::MeshData, Box<dyn std::error::Error>> {
    ///     Ok(ctx.gfx.try_load_mesh_data("models/wall.glb")?)
    /// }
    /// Game::new("t", 1, 1).run_headless(0, |ctx| assert!(wall(ctx).is_err()));
    /// ```
    pub fn try_load_mesh_data(&self, path: &str) -> Result<MeshData, crate::error::Error> {
        let bytes = self
            .asset_sources
            .find(path)?
            .into_bytes()
            .map_err(|e| e.with_path(path))?;
        let sources = &self.asset_sources;
        let loaded = gltf::load(path, &bytes, &mut |name| sources.read(name), &|_| true)
            .map_err(|e| crate::error::Error::decode(path, e))?;
        Ok(loaded.data)
    }

    /// A mesh whose index ranges each have their own material.
    /// With `skin`, it bends with a skeleton. Its morph targets, if any,
    /// show `morph_defaults` on models without [`MorphWeights`](crate::anim::MorphWeights).
    fn mesh_from_parts(
        &mut self,
        data: &MeshData,
        parts: &[(Range<u32>, Material)],
        skin: Option<MeshSkin>,
        morph_defaults: Vec<f32>,
    ) -> Mesh {
        // A freed mesh's id first, so streaming meshes in and out doesn't
        // grow the tables (and the per-frame sort, which spans every id).
        // A resident mesh coming back after eviction takes its old id, so
        // handles the game kept stay good.
        // A freed slot comes back under a new id (its generation counted
        // up), so handles to the freed mesh stay dead.
        let id = match self.assets3d.reuse_mesh_id.take() {
            Some(id) => {
                self.assets3d.mesh_ids.remake(id);
                id
            }
            None => self.assets3d.mesh_ids.make(self.assets3d.bounds.len(), "mesh"),
        };
        let slot = id & super::SLOT_MASK;
        if slot as usize == self.assets3d.bounds.len() {
            self.assets3d.bounds.push(data.bounds());
        } else {
            self.assets3d.gpu_mesh_bounds.remove(&slot);
            self.assets3d.bounds[slot as usize] = data.bounds();
        }
        self.asset_ledger.made(crate::inventory::AssetClass::Mesh, id);
        let mesh = Mesh { id };
        let slot = mesh.slot() as usize;
        if self.assets3d.mesh_bytes.len() <= slot {
            self.assets3d.mesh_bytes.resize(slot + 1, 0);
        }
        self.assets3d.mesh_bytes[slot] = super::mesh_gpu_bytes(data);
        let posed = skin.is_some() || !data.morph_targets.is_empty();
        if let Some(skin) = skin {
            if self.assets3d.skins.len() <= slot {
                self.assets3d.skins.resize(slot + 1, None);
            }
            self.assets3d.skins[slot] = Some(std::sync::Arc::new(skin));
        }
        if let Some(morph) = MeshMorph::new(data, morph_defaults) {
            if self.assets3d.morphs.len() <= slot {
                self.assets3d.morphs.resize_with(slot + 1, || None);
            }
            self.assets3d.morphs[slot] = Some(std::sync::Arc::new(morph));
        }
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.create_mesh(mesh.slot(), data, parts, posed);
        }
        mesh
    }

    /// The index of `mesh`'s morph target called `name`, for
    /// [`MorphWeights`](crate::anim::MorphWeights); `None` if it has no such
    /// target (or was freed).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let mut quad = ponko::render::MeshData::plane(1.0);
    ///     let up = vec![vec3(0.0, 0.1, 0.0); quad.positions.len()];
    ///     quad.morph_targets.push(ponko::anim::MorphTarget { name: "lift".into(), positions: up, normals: Vec::new() });
    ///     let material = ctx.gfx.default_material();
    ///     let mesh = ctx.gfx.create_mesh(&quad, material);
    ///     assert_eq!(ctx.gfx.morph_target(mesh, "lift"), Some(0));
    ///     assert_eq!(ctx.gfx.morph_target(mesh, "sag"), None);
    /// });
    /// ```
    #[must_use]
    pub fn morph_target(&self, mesh: Mesh, name: &str) -> Option<usize> {
        self.assets3d.live_bounds(mesh.id)?;
        self.assets3d.morph(mesh.id)?.names.iter().position(|n| n == name)
    }

    /// Every material loaded from glTF files so far that the files call
    /// `name`, with what it was made from: to give materials of imported
    /// models a shader of your own, keep their textures and change the
    /// rest with [`Graphics::update_material`](crate::gfx::Graphics::update_material). Models loaded later aren't in it.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.load_scene("town/Town.json").unwrap();
    /// let brick = ctx.gfx.load_shader("shaders/brick.wgsl");
    /// for (material, desc) in ctx.gfx.loaded_materials("Brick") {
    ///     ctx.gfx.update_material(material, desc.with_shader(brick));
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn loaded_materials(&self, name: &str) -> Vec<(Material, MaterialDesc)> {
        self.assets3d.loaded_materials.get(name).cloned().unwrap_or_default()
    }

    /// Frees mesh `id`: the engine side of [`Graphics::free`] with a mesh.
    pub(crate) fn free_mesh_id(&mut self, id: u32) {
        let mesh = Mesh { id };
        // A stale handle (its slot given to a newer mesh) frees nothing.
        let Some(slot) = self.assets3d.mesh_slot(id) else {
            return;
        };
        let radius = self.assets3d.bounds[slot].1;
        // Already freed: maybe an evicted resident mesh's kept id, which
        // freeing its handle releases.
        if radius < 0.0 {
            self.release_resident_mesh_id(mesh.id, false);
            return;
        }
        self.release_resident_mesh_id(mesh.id, true);
        self.asset_ledger.freed(crate::inventory::AssetClass::Mesh, mesh.id);
        self.assets3d.bounds[slot] = (Vec3::ZERO, -1.0);
        if let Some(bytes) = self.assets3d.mesh_bytes.get_mut(slot) {
            *bytes = 0;
        }
        self.assets3d.gpu_mesh_bounds.remove(&mesh.slot());
        self.assets3d.mesh_ids.release(mesh.id);
        if let Some(shadowless) = self.assets3d.shadowless.get_mut(slot) {
            *shadowless = false;
        }
        if let Some(skin) = self.assets3d.skins.get_mut(slot) {
            *skin = None;
        }
        if let Some(morph) = self.assets3d.morphs.get_mut(slot) {
            *morph = None;
        }
        let freed: Vec<_> = self
            .assets3d
            .loaded
            .iter()
            .filter(|(_, m)| **m == mesh)
            .map(|(f, _)| f.clone())
            .collect();
        for file in freed {
            self.assets3d.loaded_skeletons.remove(&file);
            self.assets3d.loaded_clips.remove(&file);
        }
        self.assets3d.loaded.retain(|_, m| *m != mesh);
        self.mesh_names.remove(mesh.id);
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_mesh(mesh.slot());
        }
    }

    /// Whether models showing `mesh` cast shadows: on for every mesh at
    /// first. Turn it off for small things that crowd the ground, such as
    /// grass and pebbles: their shadows are too small to see but drawing
    /// them into the shadow map costs as much as drawing them.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let material = ctx.gfx.default_material();
    ///     let tuft = ctx.gfx.create_mesh(&ponko::render::MeshData::cube(0.2), material);
    ///     ctx.gfx.set_mesh_shadows(tuft, false);
    ///     assert!(!ctx.gfx.mesh_shadows(tuft));
    /// });
    /// ```
    pub fn set_mesh_shadows(&mut self, mesh: Mesh, casts: bool) {
        let Some(id) = self.assets3d.live_bounds(mesh.id).and(self.assets3d.mesh_slot(mesh.id)) else {
            return;
        };
        let shadowless = &mut self.assets3d.shadowless;
        if shadowless.len() <= id {
            if casts {
                return;
            }
            shadowless.resize(id + 1, false);
        }
        shadowless[id] = !casts;
    }

    /// Whether models showing `mesh` cast shadows (see
    /// [`set_mesh_shadows`](Self::set_mesh_shadows)).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let material = ctx.gfx.default_material();
    ///     let house = ctx.gfx.create_mesh(&ponko::render::MeshData::cube(4.0), material);
    ///     assert!(ctx.gfx.mesh_shadows(house));
    /// });
    /// ```
    #[must_use]
    pub fn mesh_shadows(&self, mesh: Mesh) -> bool {
        // A stale handle reads the default, not the setting of the mesh
        // made after it in its slot.
        self.assets3d.mesh_slot(mesh.id).is_none() || self.assets3d.casts_shadows(mesh.id)
    }

    /// How many 3D models were drawn last frame: those the camera could
    /// see, and those out of its view whose shadow can fall into it. Each
    /// model counts once. A model in a shadow cascade whose map is kept
    /// this frame ([`crate::render::Lighting::shadow_refresh`]) counts too.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// println!("{} models on screen", ctx.gfx.models_drawn());
    /// # }
    /// ```
    #[must_use]
    pub fn models_drawn(&self) -> usize {
        self.frame3d.models
    }

    /// Makes TAA, temporal ambient occlusion and screen-space reflections
    /// forget the frames before this one: call it when the camera cuts to
    /// somewhere else (a teleport, a new scene), so the old view does not
    /// blend into the new one. Call this after teleporting objects too:
    /// on this frame no model has motion (the old position is not its past).
    ///
    /// The engine resets by itself on a window resize or render scale
    /// change, when temporal antialiasing or another screen effect is
    /// turned on or off, after MSAA changes, shader reloads, presentation
    /// interruptions or surface recreation, and when the camera jumps far
    /// or turns sharply in one frame. A reset asked for on a frame that draws nothing applies
    /// to the next frame that does.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(1, |ctx| {
    ///     ctx.scene3d.post.anti_aliasing = ponko::render::AntiAliasing::Taa;
    ///     ctx.scene3d.camera.position = vec3(500.0, 2.0, 0.0);
    ///     ctx.gfx.reset_temporal_history();
    /// });
    /// ```
    pub fn reset_temporal_history(&mut self) {
        self.frame3d.motion.reset = true;
    }

    /// Gives `entity`'s model no motion on the next drawn frame, as
    /// [`reset_temporal_history`](Self::reset_temporal_history) does for
    /// every model, but leaving the rest of the picture's history alone.
    /// Call it after teleporting one object, or when its animation snaps to
    /// another pose (a cut between clips with no blend), so TAA and
    /// temporal AO don't smear it along a path it never took. The frame
    /// after, its motion is measured from where it was drawn on this one.
    /// For a level-of-detail group, pass the group's entity; for a model
    /// posed through [`SkinOf`](crate::anim::SkinOf), the model's.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(1, |ctx| {
    ///     let hero = ctx.world.spawn((ponko::d3::Transform::default(),));
    ///     ctx.world.get_mut::<ponko::d3::Transform>(hero).unwrap().position = vec3(100.0, 0.0, 0.0);
    ///     ctx.gfx.reset_motion(hero);
    /// });
    /// ```
    pub fn reset_motion(&mut self, entity: ponko_ecs::Entity) {
        let motion = &mut self.frame3d.motion;
        // Kept until a frame draws. Past this many (a crowd, or frames that
        // draw nothing), the whole history is reset instead.
        if motion.forget.len() >= super::motion::MAX_FORGET {
            motion.forget.clear();
            motion.reset = true;
        } else if !motion.reset {
            motion.forget.push(entity.to_bits());
        }
    }

    /// What each mesh cost to draw in the last frame: the models and
    /// triangles the camera drew with it, and the same for the shadow maps,
    /// most triangles first. For finding what makes a frame slow; it
    /// allocates, so call it now and then, not every frame. Empty headless.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// fn heaviest(ctx: &mut Ctx) {
    ///     for c in ctx.gfx.mesh_costs().iter().take(5) {
    ///         let name = c.name.as_deref().unwrap_or("(made in code)");
    ///         println!("{name}: {} models, {} + {} shadow triangles", c.models, c.triangles, c.shadow_triangles);
    ///     }
    /// }
    /// # Game::new("t", 1, 1).system(heaviest).run_headless(1, |_| {});
    /// ```
    pub fn mesh_costs(&self) -> Vec<super::MeshCost> {
        let Some(renderer) = self.renderer.as_ref() else {
            return Vec::new();
        };
        let names: std::collections::HashMap<u32, String> = self
            .assets3d
            .loaded
            .iter()
            .map(|(origin, mesh)| (mesh.id, origin.display()))
            .collect();
        let mut costs: Vec<super::MeshCost> = renderer
            .mesh_costs()
            .into_iter()
            .map(|(slot, models, triangles, shadow_models, shadow_triangles)| {
                let mesh = self.assets3d.mesh_in_slot(slot);
                super::MeshCost {
                    mesh,
                    name: names.get(&mesh.id).cloned(),
                    models,
                    triangles,
                    shadow_models,
                    shadow_triangles,
                }
            })
            .collect();
        costs.sort_by_key(|c| std::cmp::Reverse(c.triangles + c.shadow_triangles));
        costs
    }

    /// Writes sampled skeleton morph channels to the models of `world`,
    /// including SkinOf attachments. Automatic animators call this each
    /// frame; call it after manual sampling. Unanimated weights retain their
    /// current values.
    /// ```
    /// use ponko::prelude::*;
    /// Game::new("morph", 1, 1).run_headless(0, |ctx| ctx.gfx.write_animated_morphs(&mut ctx.world));
    /// ```
    pub fn write_animated_morphs(&mut self, world: &mut World) {
        let mut entities = std::mem::take(&mut self.assets3d.morph_entities);
        entities.clear();
        // Only models whose mesh has morph targets: this runs every frame,
        // and a large world has hundreds of thousands of models.
        let morphs = &self.assets3d.morphs;
        if morphs.iter().all(Option::is_none) {
            self.assets3d.morph_bases.clear();
            self.assets3d.morph_entities = entities;
            return;
        }
        world.query::<(ponko_ecs::Entity, &super::Model)>().for_each(|(e, m)| {
            if morphs.get(m.mesh.slot() as usize).is_some_and(Option::is_some) {
                entities.push(e);
            }
        });
        for e in entities.iter().copied() {
            let mesh = world.get::<super::Model>(e).unwrap().mesh;
            let Some(morph) = self.assets3d.morphs.get(mesh.slot() as usize).and_then(|m| m.clone()) else {
                continue;
            };
            let owner = world.get::<super::SkinOf>(e).map_or(e, |s| s.0);
            if world.get::<Skeleton>(owner).is_none_or(|s| s.morphs.is_empty()) {
                continue;
            }
            let mut weights = world
                .get_mut::<super::MorphWeights>(e)
                .map(|w| std::mem::take(&mut *w))
                .unwrap_or_else(|| super::MorphWeights(morph.defaults.clone()));
            let base = self
                .assets3d
                .morph_bases
                .entry(e)
                .or_insert_with(|| (mesh.id, weights.clone(), weights.clone()));
            if base.0 != mesh.id {
                base.0 = mesh.id;
                base.1.clone_from(&weights);
            }
            for i in 0..weights.0.len().max(base.2.0.len()) {
                if weights.get(i) != base.2.get(i) {
                    base.1.set(i, weights.get(i));
                }
            }
            world
                .get::<Skeleton>(owner)
                .unwrap()
                .morphs
                .write_base(&morph.names, &base.1, &mut weights);
            base.2.clone_from(&weights);
            if world.get::<super::MorphWeights>(e).is_some() {
                *world.get_mut::<super::MorphWeights>(e).unwrap() = weights;
            } else {
                world.insert(e, weights);
            }
        }
        self.assets3d.morph_bases.retain(|e, _| world.is_alive(*e));
        self.assets3d.morph_entities = entities;
    }
}

impl Ctx {
    /// Engine side, before drawing: collect the visible models and hand
    /// them, the camera and the light to the renderer.
    /// What each target camera into a real render target sees.
    pub(crate) fn collect_views(&mut self, skins: &super::SkinFrame) {
        let targets = &self.gfx.assets3d.render_targets;
        let mut capture_cameras = Vec::new();
        self.gfx.frame3d.probe_bakes.clear();
        self.gfx.frame3d.probe_uploads.clear();
        self.gfx.assets3d.probes.take_requests();
        self.gfx
            .frame3d
            .probe_uploads
            .extend(self.gfx.assets3d.probes.in_flight_uploads.iter().cloned());
        for &(slot, center) in &self.gfx.assets3d.probes.in_flight {
            let faces = self.gfx.assets3d.probes.captures[slot as usize - 1];
            self.gfx.frame3d.probe_bakes.push((slot, faces.map(|t| t.id)));
            for (face, target) in faces.into_iter().enumerate() {
                let (direction, up) = match face {
                    0 => (Vec3::X, Vec3::Y),
                    1 => (-Vec3::X, Vec3::Y),
                    2 => (Vec3::Y, -Vec3::Z),
                    3 => (-Vec3::Y, Vec3::Z),
                    4 => (Vec3::Z, Vec3::Y),
                    _ => (-Vec3::Z, Vec3::Y),
                };
                let mut camera = self.scene3d.camera;
                camera.position = center;
                camera.rotation = glam::Quat::from_mat3(&glam::Mat3::from_cols(direction.cross(up), up, -direction));
                camera.fov_y = std::f32::consts::FRAC_PI_2;
                camera.near = 0.05;
                capture_cameras.push(super::TargetCamera::new(camera, target));
            }
        }
        self.gfx.frame3d.probes.fill(Default::default());
        for (t, p) in self
            .world
            .query::<(&super::Transform3D, &super::ReflectionProbe3D)>()
            .iter()
        {
            if (1..=4).contains(&p.content) && p.weight(t.position, t.position) > 0.0 {
                self.gfx.frame3d.probes[p.content as usize - 1] = super::probe::ProbeUniform {
                    center: t.position.extend(p.content as f32).to_array(),
                    size: (p.size * 0.5).extend(p.blend_distance.max(0.0)).to_array(),
                    params: [p.intensity, f32::from(p.box_projection), 0.0, 0.0],
                };
            }
        }
        let cameras = self
            .scene3d
            .passes
            .target_cameras
            .iter()
            .chain(capture_cameras.iter())
            .filter(|c| targets.iter().any(|t| t.id == c.target.id));
        let count = cameras.clone().count();
        let views = &mut self.gfx.frame3d.views;
        views.resize_with(count, Default::default);
        let lighting = super::Lighting {
            shadows: false,
            gi: super::GlobalIllumination {
                enabled: false,
                ..self.scene3d.lighting.gi
            },
            ..self.scene3d.lighting
        };
        for (view, camera) in views.iter_mut().zip(cameras) {
            let size = (camera.target.width(), camera.target.height());
            let mut eye = camera.camera;
            eye.set_viewport(glam::Vec2::new(size.0 as f32, size.1 as f32));
            view.frame.lod = self.scene3d.lod;
            view.frame
                .collect(&mut self.world, &self.gfx.assets3d, skins, &eye, &lighting);
            view.target = camera.target.id;
            view.size = size;
            view.camera = eye.position;
            view.view_proj = match camera.clip_plane {
                Some(plane) => super::target::oblique(eye.view_proj(), eye.view(), plane),
                None => eye.view_proj(),
            };
        }
    }

    /// Advances visual particle pools after frame systems.
    pub(crate) fn update_particles(&mut self) {
        let dt = self.dt();
        let mut emitters = self
            .world
            .query::<(&mut super::ParticleEmitter3D, &super::Transform3D)>();
        let mut any = emitters.count() > 0;
        emitters.par_for_each(|(emitter, transform)| emitter.update(dt, transform));
        let mut trails = self.world.query::<(&mut super::Trail3D, &super::Transform3D)>();
        any |= trails.count() > 0;
        trails.for_each(|(trail, transform)| trail.update(dt, transform.position));
        any |= self.world.query::<&super::Line3D>().count() > 0;
        if any && !self.headless && self.gfx.renderer.is_some() {
            self.gfx.default_material(); // also initializes a particle-only 3D renderer
        }
    }

    /// Advances every animator not in manual mode by this frame's time. The
    /// game runs it after the frame systems, so draw systems (and the
    /// renderer) see this frame's pose.
    pub(crate) fn update_animators(&mut self) {
        let dt = self.dt();
        #[cfg(feature = "physics3d")]
        let mut finished: Vec<ponko_ecs::Entity> = Vec::new();
        #[cfg(feature = "physics3d")]
        {
            // Ragdolls pose their skeletons (after this frame's interpolation);
            // their animators wait, except while blending back.
            crate::physics3d::pose_ragdolls(&mut self.world, &mut self.physics3d.ragdolls);
            self.world
                .query::<(
                    ponko_ecs::Entity,
                    &mut super::Animator,
                    &mut Skeleton,
                    &mut crate::d3::Ragdoll,
                )>()
                .for_each(|(e, animator, skeleton, ragdoll)| {
                    if ragdoll.is_blending() {
                        if !animator.manual {
                            animator.update(dt, skeleton);
                        }
                        if crate::physics3d::blend_back(ragdoll, skeleton, dt) {
                            finished.push(e);
                        }
                    }
                });
            self.world
                .query::<(ponko_ecs::Entity, &mut Skeleton, &mut crate::d3::Ragdoll)>()
                .filter::<ponko_ecs::Without<super::Animator>>()
                .for_each(|(e, skeleton, ragdoll)| {
                    if ragdoll.is_blending() && crate::physics3d::blend_back(ragdoll, skeleton, dt) {
                        finished.push(e);
                    }
                });
            for e in finished.drain(..) {
                self.world.remove::<crate::d3::Ragdoll>(e);
            }
        }
        #[allow(unused_mut)]
        let mut animated = self
            .world
            .query::<(&mut super::Animator, &mut Skeleton, Option<&mut super::Transform3D>)>();
        #[cfg(feature = "physics3d")]
        let mut animated = animated.filter::<ponko_ecs::Without<crate::d3::Ragdoll>>();
        animated.for_each(|(animator, skeleton, transform)| {
            if animator.manual {
                return;
            }
            animator.update(dt, skeleton);
            if let (true, Some(t)) = (animator.apply_root_motion, transform) {
                let motion = animator.root_motion();
                t.position += t.rotation * (motion.position * t.scale);
                t.rotation = (t.rotation * motion.rotation).normalize();
            }
        });
        self.gfx.write_animated_morphs(&mut self.world);
    }

    /// The screen effects, target effects and compute passes (of both
    /// stages) this frame runs (with their
    /// materials, `fallback` for none): freed ones, and passes into a freed
    /// target or buffer, are left out.
    pub(crate) fn collect_passes(&mut self, fallback: Option<Material>) {
        self.unlink_freed_gpu_buffers();
        self.gfx.frame3d.effects.clear();
        let effects_live = &self.gfx.assets3d.effects_live;
        self.gfx
            .frame3d
            .effects
            .extend(self.scene3d.passes.effects.iter().filter_map(|e| {
                // A freed effect is skipped.
                if !effects_live.contains(e.effect.id) {
                    return None;
                }
                let material = e.material.or(fallback)?;
                Some((e.effect.id, material.id))
            }));
        let targets = &self.gfx.assets3d.render_targets;
        self.gfx.frame3d.target_effects.clear();
        self.gfx
            .frame3d
            .target_effects
            .extend(self.scene3d.passes.target_effects.iter().filter_map(|e| {
                // A freed effect, or one into a freed target, is skipped.
                if !effects_live.contains(e.effect.id) {
                    return None;
                }
                let material = e.material.or(fallback)?;
                targets
                    .iter()
                    .any(|t| t.id == e.target.id)
                    .then_some((e.effect.id, e.target.id, material.id))
            }));
        let mut computes = std::mem::take(&mut self.gfx.frame3d.computes);
        self.list_computes(&mut computes, false, fallback);
        self.gfx.frame3d.computes = computes;
        let mut scene_computes = std::mem::take(&mut self.gfx.frame3d.scene_computes);
        self.list_computes(&mut scene_computes, true, fallback);
        self.gfx.frame3d.scene_computes = scene_computes;
    }

    /// This frame's entries of [`RenderPasses::compute`] (or, with `scene`,
    /// [`RenderPasses::scene_compute`]) into `list`: (compute, target texture
    /// or GPU buffer slot with the top bit set, material). Freed passes, and
    /// passes into a freed target or buffer, are left out.
    fn list_computes(&self, list: &mut Vec<(u32, u32, u32)>, scene: bool, fallback: Option<Material>) {
        list.clear();
        let passes = if scene {
            &self.scene3d.passes.scene_compute
        } else {
            &self.scene3d.passes.compute
        };
        let assets = &self.gfx.assets3d;
        list.extend(passes.iter().filter_map(|c| {
            // A freed pass is skipped.
            if !assets.computes_live.contains(c.compute.id) {
                return None;
            }
            let material = c.material.or(fallback)?;
            if let Some(b) = c.buffer {
                // A freed buffer's passes do nothing.
                assets
                    .gpu_buffer_slot(b.id)
                    .filter(|&slot| assets.gpu_buffers[slot] > 0)?;
                Some((c.compute.id, b.slot() | (1 << 31), material.id))
            } else {
                assets.render_targets.iter().any(|t| t.id == c.target.id).then_some((
                    c.compute.id,
                    c.target.id,
                    material.id,
                ))
            }
        }));
    }

    pub(crate) fn prepare_3d(&mut self) {
        if self.gfx.renderer.is_none()
            || (self.gfx.assets3d.bounds.is_empty()
                && self.scene3d.passes.compute.is_empty()
                && self.scene3d.passes.scene_compute.is_empty()
                && self.gfx.frame3d.particles.runs.is_empty()
                && self.gfx.frame3d.particles.strips.runs.is_empty()
                && self.world.query::<&super::ParticleEmitter3D>().count() == 0
                && self.world.query::<&super::Trail3D>().count() == 0
                && self.world.query::<&super::Line3D>().count() == 0
                && self.gfx.frame3d.particles.texts.vertices.is_empty()
                && self.world.query::<&super::Text3D>().count() == 0)
        {
            return; // no drawable 3D scene
        }
        let time = (self.time.elapsed() as f32, self.dt());
        self.gfx.assets3d.decals_supported = self.gfx.supports_decals();
        self.gfx.assets3d.gi_supported = self.gfx.renderer.as_ref().is_some_and(|r| r.supports_gi());
        let fallback = if self.scene3d.passes.effects.iter().any(|e| e.material.is_none())
            || self.scene3d.passes.target_effects.iter().any(|e| e.material.is_none())
            || self.scene3d.passes.compute.iter().any(|c| c.material.is_none())
            || self.scene3d.passes.scene_compute.iter().any(|c| c.material.is_none())
        {
            Some(self.gfx.default_material())
        } else {
            None
        };
        self.collect_passes(fallback);
        self.gfx.validate_instance_batches();
        let start = web_time::Instant::now();
        let motion = self.scene3d.post.anti_aliasing == super::AntiAliasing::Taa
            || (self.scene3d.post.ao_temporal && self.scene3d.post.ambient_occlusion.enabled);
        let mut skins = std::mem::take(&mut self.gfx.frame3d.skins);
        // The last frame's poses are kept only while something reads motion.
        skins.motion = motion;
        skins.collect(&mut self.world, &self.gfx.assets3d);
        self.collect_views(&skins);
        self.collect_text3d();
        let Some(renderer) = self.gfx.renderer.as_mut() else {
            return;
        };
        self.gfx.frame3d.size_limit = renderer.max_texture_size();
        self.gfx.frame3d.lod = self.scene3d.lod;
        self.gfx.frame3d.motion.enabled = motion;
        if !self.scene3d.camera.occlusion_culling {
            self.gfx.frame3d.occlusion = None;
        } else if let Some(grid) = renderer.take_occlusion() {
            self.gfx.frame3d.occlusion = Some(grid);
        }
        self.gfx.frame3d.collect(
            &mut self.world,
            &self.gfx.assets3d,
            &skins,
            &self.scene3d.camera,
            &self.scene3d.lighting,
        );
        self.gfx.frame3d.skins = skins;
        let light = if self.scene3d.sky.atmosphere {
            match self.gfx.frame3d.sky_light {
                Some((lighting, sky, light)) if lighting == self.scene3d.lighting && sky == self.scene3d.sky => light,
                _ => {
                    let light = super::SkyLight::new(&self.scene3d.lighting, &self.scene3d.sky);
                    self.gfx.frame3d.sky_light = Some((self.scene3d.lighting, self.scene3d.sky, light));
                    light
                }
            }
        } else {
            let rgb = |c| {
                let [r, g, b, _] = super::linear(c);
                Vec3::new(r, g, b)
            };
            super::SkyLight {
                sun: rgb(self.scene3d.lighting.sun_color) * self.scene3d.lighting.sun_intensity,
                sky: rgb(self.scene3d.sky.sky_color),
                ground: rgb(self.scene3d.sky.ground_color),
            }
        };
        let collect_ms = start.elapsed().as_secs_f32() * 1000.0;
        let scene = super::Scene3D {
            frame: &self.gfx.frame3d,
            lighting: &self.scene3d.lighting,
            sky: &self.scene3d.sky,
            light,
            camera: self.scene3d.camera.position,
            post: &self.scene3d.post,
            time,
            occlusion: self.scene3d.camera.occlusion_culling,
        };
        renderer.prepare_3d(&scene, collect_ms);
        self.gfx.frame3d.motion.reset = false;
    }
}

#[cfg(test)]
mod clip_tests {
    use super::*;

    fn cached_clips() -> (Ctx, String) {
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::ONE);
        // A real existing path, with its contents bypassed by the asset cache.
        let path = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
        let file = assets::Origin::File(assets::resolve(&path).unwrap());
        let skeleton = Skeleton::new(vec![]);
        let clips = vec![
            AnimationClip::new("idle", &skeleton, vec![]),
            AnimationClip::new("walk", &skeleton, vec![]),
        ];
        ctx.gfx.assets3d.loaded.insert(file.clone(), Mesh { id: 0 });
        ctx.gfx.assets3d.loaded_skeletons.insert(file.clone(), skeleton);
        ctx.gfx.assets3d.loaded_clips.insert(file, clips);
        (ctx, path)
    }

    #[test]
    fn cached_clips_load_by_name_and_missing_names_list_choices() {
        let (mut ctx, path) = cached_clips();
        let clips = ctx.gfx.load_clips(&path);
        assert_eq!(clips.len(), 2);
        assert_eq!(ctx.gfx.load_clip(&path, "walk").name(), "walk");
        assert!(std::sync::Arc::ptr_eq(
            clips[0].source_joints(),
            ctx.gfx.load_clip(&path, "idle").source_joints()
        ));
        let error = ctx.gfx.try_load_clip(&path, "run").unwrap_err();
        assert_eq!(error.kind(), crate::error::ErrorKind::NotFound);
        let message = error.message();
        assert!(message.contains("idle") && message.contains("walk") && message.contains("run"));
    }

    #[test]
    fn clips_cached_from_a_source_are_found_by_the_source_and_not_the_folder() {
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::ONE);
        ctx.assets
            .add_source(crate::assets::MemorySource::new("pack").with("hero.glb", b"unread".to_vec()));
        let origin = assets::Origin::Source {
            source: "pack".into(),
            name: "hero.glb".into(),
        };
        let skeleton = Skeleton::new(vec![]);
        let clips = vec![AnimationClip::new("idle", &skeleton, vec![])];
        ctx.gfx.assets3d.loaded.insert(origin.clone(), Mesh { id: 0 });
        ctx.gfx.assets3d.loaded_skeletons.insert(origin.clone(), skeleton);
        ctx.gfx.assets3d.loaded_clips.insert(origin, clips);
        // The source's bytes are never read: the cache is asked first.
        assert_eq!(ctx.gfx.load_clip("hero.glb", "idle").name(), "idle");
        assert_eq!(ctx.gfx.load_clips("hero.glb").len(), 1);
        let error = ctx.gfx.try_load_clip("hero.glb", "run").unwrap_err();
        assert!(
            error.message().contains("idle") && error.message().contains("run"),
            "{error}"
        );
    }

    #[test]
    #[should_panic(expected = "available animations: [\"idle\", \"walk\"]")]
    fn missing_named_clip_panics_with_available_names() {
        let (mut ctx, path) = cached_clips();
        ctx.gfx.load_clip(&path, "run");
    }
}
