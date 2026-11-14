//! Sprite blend modes and sprite materials from C (Rust `BlendMode`,
//! `SpriteShader`, `SpriteMaterial`, `SpriteMaterialDesc`): how sprites and
//! immediate draws combine with what's behind them, and custom 2D looks.

use std::ffi::c_char;

use ponko::prelude::{Ctx, Vec4};
use ponko::render::{BlendMode, SpriteMaterial, SpriteMaterialDesc, SpriteShader};

use crate::ctx::{ctx_access, out, text};
use crate::types::{SameLayout, draw_params_from_c, same_layout, texture_from_c, texture_to_c, to_c};
use crate::{
    CMirror, Failure, Kind, Mirror, PONKO_HANDLE_NONE, PonkoCtx, PonkoDrawParams, PonkoStatus, PonkoTexture, PonkoVec2,
    PonkoWorld, boundary, builtin, invalid, invalid_owned, mirror,
};

/// Blend mode (Rust `BlendMode::Alpha`): covers what's behind by the
/// pixel's alpha. The default.
pub const PONKO_BLEND_ALPHA: u32 = 0;
/// Adds the color, weighted by alpha, to what's behind: glows, fire,
/// sparks, light. Never darkens.
pub const PONKO_BLEND_ADDITIVE: u32 = 1;
/// Multiplies what's behind by the color: shadows, tinted glass. White
/// (or a transparent pixel) leaves it unchanged.
pub const PONKO_BLEND_MULTIPLY: u32 = 2;
/// Like `PONKO_BLEND_ALPHA`, for colors already multiplied by their alpha.
pub const PONKO_BLEND_PREMULTIPLIED: u32 = 3;

/// A sprite shader (Rust `SpriteShader`): WGSL with a
/// `fn fragment(p: SpritePixel) -> vec4<f32>` hook. Free it with
/// `ponko_ctx_handle_free(ctx, PONKO_HANDLE_SPRITE_SHADER, id)`.
/// ```
/// let s = ponko_ffi::PonkoSpriteShader { id: 0 };
/// assert_eq!(s.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PonkoSpriteShader {
    /// Engine-internal id; `PONKO_HANDLE_NONE` for none.
    pub id: u32,
}

/// A look for sprites (Rust `SpriteMaterial`): a sprite shader, its
/// parameters and extra textures. Also a component: an entity with a
/// sprite and one draws with it (`ponko_sprite_material_component`). Free
/// it with `ponko_ctx_handle_free(ctx, PONKO_HANDLE_SPRITE_MATERIAL, id)`;
/// sprites that still use it then draw with the built-in look.
/// ```
/// let m = ponko_ffi::PonkoSpriteMaterial { id: ponko_ffi::PONKO_HANDLE_NONE };
/// assert_eq!(m.id, u32::MAX);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PonkoSpriteMaterial {
    /// Engine-internal id; `PONKO_HANDLE_NONE` for the built-in look.
    pub id: u32,
}
// SpriteShader and SpriteMaterial are repr(C) { u32 id }; every id is a
// valid value (unknown ones draw with the built-in look).
same_layout!(PonkoSpriteShader, SpriteShader);
same_layout!(PonkoSpriteMaterial, SpriteMaterial);

/// What a sprite material is made of (Rust `SpriteMaterialDesc`). Start
/// from `ponko_sprite_material_desc_default()`.
/// ```
/// let d = ponko_ffi::ponko_sprite_material_desc_default();
/// assert!(!d.has_shader && d.params == [0.0; 16]);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoSpriteMaterialDesc {
    /// Whether `shader` is used (else the built-in look, texture × tint).
    pub has_shader: bool,
    /// The sprite shader.
    pub shader: PonkoSpriteShader,
    /// What the shader reads as `material.params[0..4]`, four floats each.
    pub params: [f32; 16],
    /// Whether each of `textures` is used (else white).
    pub has_textures: [bool; 2],
    /// What `material_sample0` and `material_sample1` read in the shader.
    pub textures: [PonkoTexture; 2],
}

/// A sprite material description with the built-in look, zero params and
/// no extra textures.
/// ```
/// assert_eq!(ponko_ffi::ponko_sprite_material_desc_default().shader.id, ponko_ffi::PONKO_HANDLE_NONE);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_sprite_material_desc_default() -> PonkoSpriteMaterialDesc {
    desc_to_c(&SpriteMaterialDesc::default())
}

fn desc_to_c(d: &SpriteMaterialDesc) -> PonkoSpriteMaterialDesc {
    let none = PonkoTexture {
        id: PONKO_HANDLE_NONE,
        width: 0,
        height: 0,
    };
    let mut params = [0.0; 16];
    for (i, v) in d.params.iter().enumerate() {
        params[i * 4..i * 4 + 4].copy_from_slice(&v.to_array());
    }
    PonkoSpriteMaterialDesc {
        has_shader: d.shader.is_some(),
        shader: d.shader.map_or(PonkoSpriteShader { id: PONKO_HANDLE_NONE }, to_c),
        params,
        has_textures: d.textures.map(|t| t.is_some()),
        textures: d.textures.map(|t| t.map_or(none, texture_to_c)),
    }
}

fn desc_from_c(c: &Ctx, d: &PonkoSpriteMaterialDesc) -> Result<SpriteMaterialDesc, Failure> {
    if !d.params.iter().all(|p| p.is_finite()) {
        return Err(invalid("sprite material params must be finite"));
    }
    let mut desc = SpriteMaterialDesc::default();
    if d.has_shader {
        let shader: SpriteShader = to_c(d.shader);
        if !c.is_live(shader) {
            return Err(invalid("that sprite shader wasn't made by this game"));
        }
        desc = SpriteMaterialDesc::new(shader);
    }
    for (i, p) in d.params.as_chunks::<4>().0.iter().enumerate() {
        desc = desc.with_param(i, Vec4::from_array(*p));
    }
    for i in 0..2 {
        if d.has_textures[i] {
            desc = desc.with_texture(i, texture_from_c(d.textures[i]));
        }
    }
    Ok(desc)
}

fn blend_from_c(blend: u32) -> Result<BlendMode, Failure> {
    BlendMode::from_u32(blend)
        .ok_or_else(|| invalid("blend must be 0 (alpha), 1 (additive), 2 (multiply) or 3 (premultiplied)"))
}

/// Loads a sprite shader (WGSL) from `assets/` (Rust
/// `Graphics::try_load_sprite_shader`); with hot reload on, saving the
/// file updates the game, and an edit that doesn't compile keeps the last
/// good one. A file that can't be read or doesn't compile is an error;
/// `ponko_last_error` says where. With hot reload on, a file that doesn't
/// compile still gives a shader, drawing with the built-in look until a
/// saved fix compiles (`ponko_ctx_shader_status` says what's wrong).
/// # Safety
/// `ctx` null or live; `path` a C string; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_sprite_shader(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_sprite_shader(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_shader: *mut PonkoSpriteShader,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_shader.is_null() {
                return Err(invalid("output is null"));
            }
            let shader = c.gfx.try_load_sprite_shader(path).map_err(invalid_owned)?;
            out(out_shader, to_c(shader))
        })
    })
}

/// Makes a sprite shader from WGSL source (Rust
/// `Graphics::try_create_sprite_shader`); `name` is for error messages.
/// Source that doesn't compile is an error; `ponko_last_error` says where.
/// # Safety
/// `ctx` null or live; `name` and `source` C strings; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_sprite_shader(std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_sprite_shader(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    source: *const c_char,
    out_shader: *mut PonkoSpriteShader,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            let source = text(source)?;
            if out_shader.is_null() {
                return Err(invalid("output is null"));
            }
            let shader = c.gfx.try_create_sprite_shader(name, source).map_err(invalid_owned)?;
            out(out_shader, to_c(shader))
        })
    })
}

/// Makes a sprite material (Rust `Graphics::create_sprite_material`).
/// A shader this game didn't make, or params that aren't finite, are an
/// error.
/// # Safety
/// `ctx` null or live; `desc` readable; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_sprite_material(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_sprite_material(
    ctx: *mut PonkoCtx,
    desc: *const PonkoSpriteMaterialDesc,
    out_material: *mut PonkoSpriteMaterial,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if desc.is_null() || out_material.is_null() {
                return Err(invalid("desc or output is null"));
            }
            let desc = desc_from_c(c, &*desc)?;
            let material = c.gfx.create_sprite_material(desc);
            out(out_material, to_c(material))
        })
    })
}

/// Changes what `material` is made of (Rust
/// `Graphics::update_sprite_material`); cheap enough every frame, to
/// animate a parameter. Does nothing for a freed material.
/// # Safety
/// `ctx` null or live; `desc` readable.
/// ```
/// let m = ponko_ffi::PonkoSpriteMaterial { id: 0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_update_sprite_material(std::ptr::null_mut(), m, std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_update_sprite_material(
    ctx: *mut PonkoCtx,
    material: PonkoSpriteMaterial,
    desc: *const PonkoSpriteMaterialDesc,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if desc.is_null() {
                return Err(invalid("desc is null"));
            }
            let desc = desc_from_c(c, &*desc)?;
            c.gfx.update_sprite_material(to_c(material), desc);
            Ok(())
        })
    })
}

/// What `material` is made of now (Rust `Graphics::sprite_material_desc`).
/// `*out_found` is false once it's freed (and `*out_desc` is untouched).
/// # Safety
/// `ctx` null or live; outputs writable.
/// ```
/// let m = ponko_ffi::PonkoSpriteMaterial { id: 0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_sprite_material_desc(std::ptr::null_mut(), m, std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_sprite_material_desc(
    ctx: *mut PonkoCtx,
    material: PonkoSpriteMaterial,
    out_desc: *mut PonkoSpriteMaterialDesc,
    out_found: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_desc.is_null() {
                return Err(invalid("output is null"));
            }
            let found = c.gfx.sprite_material_desc(to_c(material));
            if let Some(d) = &found {
                out_desc.write(desc_to_c(d));
            }
            out(out_found, found.is_some())
        })
    })
}

/// Draws a texture with options, a blend mode (`PONKO_BLEND_*`) and a
/// sprite material (`PONKO_HANDLE_NONE` id for the built-in look) (Rust
/// `DrawParams::with_blend` and `with_material`). `params` may be null for
/// the defaults.
/// # Safety
/// `ctx` null or live; `params` null or readable.
/// ```
/// use ponko_ffi::*;
/// let t = PonkoTexture { id: 0, width: 1, height: 1 };
/// let m = PonkoSpriteMaterial { id: PONKO_HANDLE_NONE };
/// assert_eq!(unsafe { ponko_ctx_draw_styled(std::ptr::null_mut(), t, PonkoVec2::default(), std::ptr::null(), PONKO_BLEND_ADDITIVE, m) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw_styled(
    ctx: *mut PonkoCtx,
    texture: PonkoTexture,
    pos: PonkoVec2,
    params: *const PonkoDrawParams,
    blend: u32,
    material: PonkoSpriteMaterial,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let blend = blend_from_c(blend)?;
            let mut p = if params.is_null() {
                ponko::prelude::DrawParams::default()
            } else {
                draw_params_from_c(&*params)
            };
            p = p.with_blend(blend);
            if material.id != PONKO_HANDLE_NONE {
                p = p.with_material(to_c(material));
            }
            c.gfx.draw_ex(texture_from_c(texture), to_c(pos), p);
            Ok(())
        })
    })
}

impl CMirror for BlendMode {
    type C = u32;
    fn from_c(c: &u32) -> Result<Self, Failure> {
        blend_from_c(*c)
    }
    fn to_c(&self) -> u32 {
        *self as u32
    }
}
static BLEND_MODE: Mirror = mirror::<BlendMode>();

/// How the C side sees the sprite components.
pub(crate) fn sprite_kind(t: std::any::TypeId) -> Option<Kind> {
    use std::any::TypeId;
    if t == TypeId::of::<BlendMode>() {
        // A `uint32_t` `PONKO_BLEND_*`, checked on every spawn and set.
        Some(Kind::Mirror(&BLEND_MODE))
    } else if t == TypeId::of::<SpriteMaterial>() {
        // repr(C), layout checked against PonkoSpriteMaterial; any id is
        // valid (unknown ones draw with the built-in look).
        Some(Kind::Plain)
    } else {
        None
    }
}

/// The id of the built-in blend mode component (Rust `BlendMode`),
/// registering it if needed. Values are a `uint32_t` `PONKO_BLEND_*` in
/// spawn/get/set; an entity with a sprite and one blends that way.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_blend_mode_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_blend_mode_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<BlendMode>(world, out_id) }
}

/// The id of the built-in sprite material component (Rust
/// `SpriteMaterial`), registering it if needed. Values are
/// `PonkoSpriteMaterial`; an entity with a sprite and one draws with it.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_sprite_material_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_sprite_material_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<SpriteMaterial>(world, out_id) }
}
