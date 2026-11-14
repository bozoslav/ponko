//! Blend modes and custom shaders for sprites and immediate 2D draws:
//! [`BlendMode`], [`SpriteShader`], [`SpriteMaterial`] and
//! [`SpriteMaterialDesc`], re-exported from [`d2`](crate::d2) and
//! [`render`](crate::render).

use glam::Vec4;
use ponko_ecs::Component;

use crate::gfx::Graphics;
use crate::texture::Texture;

/// How a sprite's pixels combine with what is already drawn behind them.
///
/// Add it to an entity next to its [`Sprite`](crate::d2::Sprite), or pass
/// it with [`DrawParams::with_blend`](crate::texture::DrawParams::with_blend)
/// for an immediate draw. Sprites without one blend by their alpha.
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::render::BlendMode;
/// # fn setup(ctx: &mut Ctx, glow: Texture) {
/// ctx.world.spawn((
///     ponko::d2::Transform::at(vec2(200.0, 150.0)),
///     ponko::d2::Sprite::new(glow).with_color(Color::rgb(1.0, 0.6, 0.2)),
///     BlendMode::Additive, // brightens what's behind it
/// ));
/// # }
/// ```
///
/// Every mode works with the built-in look and with a [`SpriteMaterial`]'s
/// shader, on every GPU (WebGL2 included); each mode is its own batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Component)]
#[repr(u32)]
#[non_exhaustive]
pub enum BlendMode {
    /// Covers what's behind by the pixel's alpha (the default).
    #[default]
    Alpha = 0,
    /// Adds the color, weighted by alpha, to what's behind: glows, fire,
    /// sparks, light. Never darkens.
    Additive = 1,
    /// Multiplies what's behind by the color: shadows, tinted glass,
    /// darkening. White leaves it unchanged; transparent pixels too.
    Multiply = 2,
    /// Like `Alpha`, for colors already multiplied by their alpha: textures
    /// exported premultiplied, or shaders that return such colors. The
    /// tint of the built-in look is premultiplied for you.
    Premultiplied = 3,
}

impl BlendMode {
    /// Every mode, in order.
    pub const ALL: [BlendMode; 4] = [
        BlendMode::Alpha,
        BlendMode::Additive,
        BlendMode::Multiply,
        BlendMode::Premultiplied,
    ];

    /// The mode numbered `n` (as `n as u32` gives), or `None`.
    ///
    /// ```
    /// # use ponko::render::BlendMode;
    /// assert_eq!(BlendMode::from_u32(1), Some(BlendMode::Additive));
    /// assert_eq!(BlendMode::from_u32(9), None);
    /// ```
    #[must_use]
    pub fn from_u32(n: u32) -> Option<Self> {
        Self::ALL.get(n as usize).copied()
    }
}

/// A sprite shader: WGSL with a `fragment` hook that decides each pixel's
/// color. Make one with [`Graphics::load_sprite_shader`] (hot reloaded) or
/// [`Graphics::create_sprite_shader`], and use it through a
/// [`SpriteMaterial`].
///
/// The hook is an ordinary function (not an entry point) with exactly this
/// signature (the parameter name is yours):
///
/// ```wgsl
/// fn fragment(p: SpritePixel) -> vec4<f32>
/// ```
///
/// `p.color` is what the built-in shader would draw (texture × tint); `p.texture`,
/// `p.tint`, `p.uv`, `p.local` (0..1 across the sprite) and `p.world` are
/// there too. It returns a linear color with straight alpha (premultiplied
/// for [`BlendMode::Premultiplied`]). It can read `material.params[0..4]`
/// (four `vec4<f32>`) and call `sprite_sample(uv)`, `sprite_texel()`,
/// `material_sample0(uv)` and `material_sample1(uv)` (the material's extra
/// textures, white if missing). All bindings are the engine's: a shader
/// declares no `@group`/`@binding` and no entry points. Shaders are checked
/// against what every supported GPU can run (WebGL2 included), so errors
/// show up on the developer's machine, with lines in the shader's own code
/// ([`Graphics::shader_status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)] // shared with C/C++ (`PonkoSpriteShader`)
pub struct SpriteShader {
    pub(crate) id: u32,
}

/// A look for sprites: a [`SpriteShader`] (or the built-in look), its
/// parameters and extra textures. Make one with
/// [`Graphics::create_sprite_material`], change it with
/// [`Graphics::update_sprite_material`].
///
/// Add it to an entity next to its [`Sprite`](crate::d2::Sprite), or pass
/// it with [`DrawParams::with_material`](crate::texture::DrawParams::with_material).
/// Sprites with the same material, texture and [`BlendMode`] in a row are
/// drawn together; each material is its own batch.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # use ponko::render::{SpriteMaterial, SpriteMaterialDesc};
/// # fn setup(ctx: &mut Ctx, crab: Texture) {
/// let outline = ctx.gfx.load_sprite_shader("shaders/outline.wgsl");
/// let material = ctx.gfx.create_sprite_material(
///     SpriteMaterialDesc::new(outline).with_param(0, vec4(1.0, 0.9, 0.2, 1.0)),
/// );
/// ctx.world.spawn((ponko::d2::Transform::at(vec2(100.0, 100.0)), ponko::d2::Sprite::new(crab), material));
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Component)]
#[repr(C)] // shared with C/C++ (`PonkoSpriteMaterial`)
pub struct SpriteMaterial {
    pub(crate) id: u32,
}

/// What a [`SpriteMaterial`] is made of.
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::render::SpriteMaterialDesc;
/// let desc = SpriteMaterialDesc::default().with_param(1, vec4(0.5, 0.0, 0.0, 0.0));
/// assert_eq!(desc.shader, None);
/// assert_eq!(desc.params[1].x, 0.5);
/// ```
///
/// New fields may be added in any release: start from
/// [`SpriteMaterialDesc::new`] or `Default` and chain the `with_*` methods.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct SpriteMaterialDesc {
    /// The shader. `None` (default): the built-in look, texture × tint.
    pub shader: Option<SpriteShader>,
    /// Four values the shader reads as `material.params[0..4]`. Default zero.
    pub params: [Vec4; 4],
    /// Extra textures the shader reads with `material_sample0` and
    /// `material_sample1` (noise, palettes, masks). `None`: white.
    pub textures: [Option<Texture>; 2],
}

impl Default for SpriteMaterialDesc {
    fn default() -> Self {
        Self {
            shader: None,
            params: [Vec4::ZERO; 4],
            textures: [None; 2],
        }
    }
}

impl SpriteMaterialDesc {
    /// A material drawn with `shader`.
    #[must_use]
    pub fn new(shader: SpriteShader) -> Self {
        Self {
            shader: Some(shader),
            ..Default::default()
        }
    }

    /// The same with `material.params[index]` set to `value`.
    ///
    /// # Panics
    ///
    /// If `index` is 4 or more.
    #[must_use]
    pub fn with_param(mut self, index: usize, value: Vec4) -> Self {
        assert!(index < 4, "ponko: a sprite material has params 0 to 3, not {index}");
        self.params[index] = value;
        self
    }

    /// The same with extra texture `index` (0 or 1) set to `texture`.
    ///
    /// # Panics
    ///
    /// If `index` is 2 or more.
    #[must_use]
    pub fn with_texture(mut self, index: usize, texture: Texture) -> Self {
        assert!(index < 2, "ponko: a sprite material has textures 0 and 1, not {index}");
        self.textures[index] = Some(texture);
        self
    }
}

/// The number the renderer batches a draw's look by: 0 for the built-in
/// look with alpha blending (plain sprites), otherwise the material
/// (`id + 1`, 0 for none) above the blend mode's two bits. Below
/// `batch::CLIPPED`.
#[inline]
pub(crate) fn style_key(blend: BlendMode, material: Option<SpriteMaterial>) -> u32 {
    let material = material.map_or(0, |m| m.id.wrapping_add(1));
    // Ids that would reach the clip bit can't happen (a game would have
    // to make half a billion materials); they draw with the built-in look.
    let material = if material < 1 << 29 { material } else { 0 };
    (material << 2) | blend as u32
}

/// The blend mode and material (`id`, `None` for the built-in look) of a
/// [`style_key`].
#[cfg_attr(not(feature = "window"), allow(dead_code))]
#[inline]
pub(crate) fn split_style(key: u32) -> (BlendMode, Option<u32>) {
    let blend = BlendMode::from_u32(key & 3).unwrap_or_default();
    let material = key >> 2;
    (blend, material.checked_sub(1))
}

/// The game's sprite shaders and materials (`ctx.gfx.sprites`).
#[derive(Default)]
pub(crate) struct SpriteAssets {
    /// Shaders made so far; ids are `0..shaders`.
    shaders: u32,
    pub(crate) shaders_live: crate::handle::LiveSet,
    loaded_shaders: std::collections::HashMap<crate::assets::Origin, SpriteShader>,
    /// Every material made, by id (freed ones too).
    materials: Vec<SpriteMaterialDesc>,
    pub(crate) materials_live: crate::handle::LiveSet,
}

/// The game's WGSL between the engine's code: what is compiled.
#[must_use]
pub(crate) fn compose_sprite_shader(user: &str) -> String {
    let mut out = String::with_capacity(user.len() + 4096);
    out.push_str(include_str!("render/sprite_prelude.wgsl"));
    // The markers are what `render::diagnose` finds the game's code by.
    out.push_str("\n// ---- the game's sprite shader\n");
    out.push_str(user);
    out.push_str("\n// ---- the engine's entry points\n");
    out.push_str(include_str!("render/sprite_entries.wgsl"));
    out
}

/// The variant name in sprite shaders' diagnostics.
pub(crate) const SPRITE_VARIANT: &str = "sprite";

/// What a sprite shader file that doesn't compile draws with until fixed.
const SPRITE_STAND_IN: &str = "fn fragment(p: SpritePixel) -> vec4<f32> { return p.color; }";

impl Graphics {
    /// Loads a sprite shader (WGSL with a `fragment` hook, see
    /// [`SpriteShader`]) from an [added asset
    /// source](crate::assets::Assets::add_source) or `assets/`. With hot
    /// reload on, saving the file updates every material using it (an
    /// error keeps the old version, and [`shader_status`](Self::shader_status)
    /// says where it is). Loading the same file again returns the same shader.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let dissolve = ctx.gfx.load_sprite_shader("shaders/dissolve.wgsl");
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If the file can't be read or doesn't compile (the message says
    /// where). With hot reload on, a file that doesn't compile gets a
    /// stand-in (the built-in look) and stays watched until a saved fix compiles.
    pub fn load_sprite_shader(&mut self, path: &str) -> SpriteShader {
        self.try_load_sprite_shader(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_sprite_shader`](Self::load_sprite_shader) with errors as
    /// values: [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound),
    /// [`Io`](crate::error::ErrorKind::Io), [`Decode`](crate::error::ErrorKind::Decode)
    /// or [`Shader`](crate::error::ErrorKind::Shader) (with the compiler's log).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_load_sprite_shader("shaders/missing.wgsl").unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::NotFound);
    /// });
    /// ```
    pub fn try_load_sprite_shader(&mut self, path: &str) -> Result<SpriteShader, crate::error::Error> {
        let found = match self
            .asset_sources
            .lookup(path, |origin| self.sprites.loaded_shaders.get(origin).copied())?
        {
            crate::assets::Lookup::Cached(_, shader) => return Ok(shader),
            crate::assets::Lookup::Found(found) => found,
        };
        let origin = found.origin.clone();
        let source = found.into_text(path)?;
        let shader = match self.try_create_sprite_shader(path, &source) {
            Ok(shader) => shader,
            Err(e) if self.keeps_failed_load(&origin) => {
                let e = e.with_path(path);
                Self::report_stand_in(path, &e);
                self.compile_sprite_shader(path, SPRITE_STAND_IN, None).map_err(|_| e)?;
                self.new_sprite_shader()
            }
            Err(e) => return Err(e.with_path(path)),
        };
        self.sprites.loaded_shaders.insert(origin.clone(), shader);
        if let crate::assets::Origin::File(file) = origin {
            self.watcher
                .watch(path, file, crate::hot_reload::Asset::SpriteShader(shader.id));
        }
        Ok(shader)
    }

    /// Makes a sprite shader from WGSL source (see [`SpriteShader`]);
    /// `name` is for error messages.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let flash = ctx.gfx.create_sprite_shader("flash", "
    ///         fn fragment(p: SpritePixel) -> vec4<f32> {
    ///             return vec4<f32>(mix(p.color.rgb, vec3<f32>(1.0), material.params[0].x), p.color.a);
    ///         }");
    ///     assert!(ctx.gfx.is_live(flash));
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// If it doesn't compile (the message says where).
    pub fn create_sprite_shader(&mut self, name: &str, source: &str) -> SpriteShader {
        self.try_create_sprite_shader(name, source)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`create_sprite_shader`](Self::create_sprite_shader) with errors as
    /// values: [`ErrorKind::Shader`](crate::error::ErrorKind::Shader) with
    /// the compiler's log; [`shader_status`](Self::shader_status) has it as data.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_create_sprite_shader("broken", "fn fragment(p: SpritePixel) -> vec4<f32> { return 1.0; }").unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::Shader);
    /// });
    /// ```
    pub fn try_create_sprite_shader(&mut self, name: &str, source: &str) -> Result<SpriteShader, crate::error::Error> {
        let compiled = self.compile_sprite_shader(name, source, None);
        self.record_shader(name, crate::render::ShaderKind::Sprite, false, compiled)?;
        Ok(self.new_sprite_shader())
    }

    fn new_sprite_shader(&mut self) -> SpriteShader {
        let id = self.sprites.shaders;
        self.sprites.shaders += 1;
        self.sprites.shaders_live.insert(id);
        SpriteShader { id }
    }

    /// Recompiles a sprite shader whose file changed (hot reload). On an
    /// error the last good version stays in use.
    pub(crate) fn reload_sprite_shader(&mut self, id: u32, name: &str, source: &str) -> Result<(), String> {
        if !self.sprites.shaders_live.contains(id) {
            return Ok(());
        }
        let compiled = self.compile_sprite_shader(name, source, Some(id));
        Ok(self.record_shader(name, crate::render::ShaderKind::Sprite, true, compiled)?)
    }

    /// Checks sprite shader `source` and hands it to the GPU, as shader
    /// `replace` or as the next one.
    fn compile_sprite_shader(
        &mut self,
        name: &str,
        source: &str,
        replace: Option<u32>,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        let composed = compose_sprite_shader(source);
        #[cfg(feature = "window")]
        crate::render::diagnose(name, crate::render::ShaderKind::Sprite, SPRITE_VARIANT, &composed)?;
        let id = replace.unwrap_or(self.sprites.shaders);
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.set_sprite_shader(id, name, &composed)?;
        }
        let _ = (name, composed, id);
        Ok(())
    }

    /// Frees sprite shader `id`: its materials draw with the built-in look.
    pub(crate) fn free_sprite_shader_id(&mut self, id: u32) {
        if !self.sprites.shaders_live.remove(id) {
            return;
        }
        self.sprites.loaded_shaders.retain(|_, s| s.id != id);
        self.watcher
            .forget(|a| matches!(a, crate::hot_reload::Asset::SpriteShader(s) if *s == id));
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_sprite_shader(id);
        }
    }

    /// Makes a sprite material (see [`SpriteMaterial`]).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use ponko::render::SpriteMaterialDesc;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let tint = ctx.gfx.create_sprite_shader("tint", "
    ///         fn fragment(p: SpritePixel) -> vec4<f32> { return p.color * material.params[0]; }");
    ///     let red = ctx.gfx.create_sprite_material(SpriteMaterialDesc::new(tint).with_param(0, vec4(1.0, 0.0, 0.0, 1.0)));
    ///     assert_eq!(ctx.gfx.sprite_material_desc(red).unwrap().params[0], vec4(1.0, 0.0, 0.0, 1.0));
    /// });
    /// ```
    pub fn create_sprite_material(&mut self, desc: SpriteMaterialDesc) -> SpriteMaterial {
        let id = self.sprites.materials.len() as u32;
        self.sprites.materials.push(desc);
        self.sprites.materials_live.insert(id);
        self.send_sprite_material(id, &desc);
        SpriteMaterial { id }
    }

    /// Changes what `material` is made of; everything drawn with it from
    /// now on looks the new way. Cheap enough to call every frame (to
    /// animate a parameter). Does nothing for a freed material.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use ponko::render::{SpriteMaterial, SpriteMaterialDesc};
    /// fn fade(ctx: &mut Ctx, dissolve: SpriteMaterial) {
    ///     let Some(desc) = ctx.gfx.sprite_material_desc(dissolve) else { return };
    ///     let t = ctx.time.elapsed() as f32;
    ///     ctx.gfx.update_sprite_material(dissolve, desc.with_param(0, vec4(t.sin() * 0.5 + 0.5, 0.0, 0.0, 0.0)));
    /// }
    /// ```
    pub fn update_sprite_material(&mut self, material: SpriteMaterial, desc: SpriteMaterialDesc) {
        if !self.sprites.materials_live.contains(material.id) {
            return;
        }
        self.sprites.materials[material.id as usize] = desc;
        self.send_sprite_material(material.id, &desc);
    }

    /// What `material` is made of now; `None` once freed.
    #[must_use]
    pub fn sprite_material_desc(&self, material: SpriteMaterial) -> Option<SpriteMaterialDesc> {
        if !self.sprites.materials_live.contains(material.id) {
            return None;
        }
        self.sprites.materials.get(material.id as usize).copied()
    }

    fn send_sprite_material(&mut self, id: u32, desc: &SpriteMaterialDesc) {
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            let shader = desc
                .shader
                .filter(|s| self.sprites.shaders_live.contains(s.id))
                .map(|s| s.id);
            let textures = desc.textures.map(|t| t.map(|t| t.id));
            renderer.set_sprite_material(id, shader, &desc.params, textures);
        }
    }

    /// Frees sprite material `id`: what still uses it draws with the
    /// built-in look.
    pub(crate) fn free_sprite_material_id(&mut self, id: u32) {
        if !self.sprites.materials_live.remove(id) {
            return;
        }
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_sprite_material(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_keys_round_trip_and_plain_sprites_are_zero() {
        assert_eq!(style_key(BlendMode::Alpha, None), 0);
        for blend in BlendMode::ALL {
            for material in [None, Some(SpriteMaterial { id: 0 }), Some(SpriteMaterial { id: 77 })] {
                let key = style_key(blend, material);
                assert!(key < crate::render::batch::CLIPPED);
                assert_eq!(split_style(key), (blend, material.map(|m| m.id)));
            }
        }
        // The empty handle and ids past the key's room draw the built-in look.
        assert_eq!(style_key(BlendMode::Additive, Some(SpriteMaterial { id: u32::MAX })), 1);
        assert_eq!(style_key(BlendMode::Additive, Some(SpriteMaterial { id: 1 << 30 })), 1);
    }

    #[test]
    fn entities_and_draws_with_a_look_are_batched_by_it() {
        use crate::d2::{Sprite, Transform};
        use crate::prelude::*;
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(64.0, 64.0));
        let pixel = Texture::new(0, 1, 1);
        let material = SpriteMaterial { id: 3 };
        let at = Transform::at(Vec2::splat(8.0));
        ctx.world.spawn((at, Sprite::new(pixel)));
        ctx.world.spawn((at, Sprite::new(pixel), BlendMode::Additive));
        ctx.world.spawn((at, Sprite::new(pixel), material));
        ctx.world.spawn((at, Sprite::new(pixel), material, BlendMode::Multiply));
        ctx.world.spawn((at, Sprite::new(pixel)));
        ctx.draw_world_sprites();
        ctx.gfx.draw_ex(
            pixel,
            Vec2::ONE,
            DrawParams::default()
                .with_blend(BlendMode::Premultiplied)
                .with_material(material),
        );
        let (sprites, batches) = ctx.gfx.draw_list.finish();
        assert_eq!(sprites.len(), 6);
        let mut looks: Vec<_> = batches.iter().map(|b| (b.material, b.end - b.start)).collect();
        // Plain sprites come first, in one batch; the order of the others
        // follows the world's storage.
        assert_eq!(looks[0], (0, 2));
        looks.sort_unstable();
        let some = Some(material);
        assert_eq!(
            looks,
            [
                (0, 2),
                (style_key(BlendMode::Additive, None), 1),
                (style_key(BlendMode::Alpha, some), 1),
                (style_key(BlendMode::Multiply, some), 1),
                (style_key(BlendMode::Premultiplied, some), 1),
            ]
        );
    }

    #[test]
    fn the_game_range_of_a_composed_sprite_shader_is_its_code() {
        let user = "fn fragment(p: SpritePixel) -> vec4<f32> { return p.color; }";
        let composed = compose_sprite_shader(user);
        let start = composed.find(user).unwrap();
        assert!(composed[..start].ends_with("// ---- the game's sprite shader\n"));
        assert!(composed[start + user.len()..].starts_with("\n// ---- the engine's entry points\n"));
    }
}
