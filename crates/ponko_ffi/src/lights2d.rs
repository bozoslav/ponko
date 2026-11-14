//! 2D lights from C (Rust `d2::PointLight`, `d2::SpotLight`,
//! `d2::DirectionalLight`, `d2::Occluder`): the lights are plain
//! components (register them with `ponko_point_light2d_component` and the
//! others, start from their `_default()`), occluders are set on an entity
//! with a 2D transform, and the ambient light and normal maps are settings
//! of the context. Lighting stays off, at no cost, until a 2D light exists
//! or an ambient light is set.

use ponko::d2::{DirectionalLight, Occluder, PointLight, SpotLight};
use ponko::prelude::{Color, Ctx, Vec2};
use ponko::tilemap::TileMap;

use crate::ctx::{ctx_access, out};
use crate::particles::entity;
use crate::types::{PonkoColor, PonkoTexture, PonkoVec2, SameLayout, same_layout, texture_from_c, texture_to_c, to_c};
use crate::{Failure, Kind, PonkoCtx, PonkoStatus, PonkoWorld, boundary, builtin, invalid};

/// The built-in `PointLight2D` component: light from the entity's 2D
/// transform position, out to `radius`. Start from
/// `ponko_point_light2d_default()`.
/// ```
/// let l = ponko_ffi::ponko_point_light2d_default();
/// assert_eq!((l.radius, l.shadows), (200.0, false));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoPointLight2D {
    /// sRGB color; alpha ignored.
    pub color: PonkoColor,
    /// Brightness multiplier. Default 1.
    pub intensity: f32,
    /// Reach in world units. Default 200.
    pub radius: f32,
    /// The light is `(1 - distance / radius) ^ falloff`. Default 2.
    pub falloff: f32,
    /// Height above the scene, for normal maps. Default 64.
    pub height: f32,
    /// Occluders cast shadows from it. Default false.
    pub shadows: bool,
}

/// The built-in `SpotLight2D` component: a cone of light from the entity's
/// 2D transform, along its rotation (0 points along +x). Start from
/// `ponko_spot_light2d_default()`.
/// ```
/// let l = ponko_ffi::ponko_spot_light2d_default();
/// assert!((l.angle - std::f32::consts::FRAC_PI_6).abs() < 1e-6);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoSpotLight2D {
    /// sRGB color; alpha ignored.
    pub color: PonkoColor,
    /// Brightness multiplier. Default 1.
    pub intensity: f32,
    /// Reach in world units. Default 300.
    pub radius: f32,
    /// As `PonkoPointLight2D::falloff`. Default 2.
    pub falloff: f32,
    /// As `PonkoPointLight2D::height`. Default 64.
    pub height: f32,
    /// Half the cone's width in radians. Default pi/6.
    pub angle: f32,
    /// How much of the cone's edge fades, 0 (hard) to 1. Default 0.2.
    pub softness: f32,
    /// Occluders cast shadows from it. Default false.
    pub shadows: bool,
}

/// The built-in `DirectionalLight2D` component: light falling evenly
/// along `direction` (y down), such as sunlight; needs no transform.
/// Start from `ponko_directional_light2d_default()`.
/// ```
/// let l = ponko_ffi::ponko_directional_light2d_default();
/// assert_eq!((l.direction.x, l.direction.y), (0.0, 1.0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoDirectionalLight2D {
    /// sRGB color; alpha ignored.
    pub color: PonkoColor,
    /// Brightness multiplier. Default 1.
    pub intensity: f32,
    /// Where the light travels (any length). Default {0, 1}.
    pub direction: PonkoVec2,
    /// Angle above the scene in radians, for normal maps. Default pi/4.
    pub elevation: f32,
    /// Occluders cast shadows from it. Default false.
    pub shadows: bool,
}

same_layout!(
    PonkoPointLight2D,
    PointLight,
    color = color,
    intensity = intensity,
    radius = radius,
    falloff = falloff,
    height = height,
    shadows = shadows
);
same_layout!(
    PonkoSpotLight2D,
    SpotLight,
    color = color,
    intensity = intensity,
    radius = radius,
    falloff = falloff,
    height = height,
    angle = angle,
    softness = softness,
    shadows = shadows
);
same_layout!(
    PonkoDirectionalLight2D,
    DirectionalLight,
    color = color,
    intensity = intensity,
    direction = direction,
    elevation = elevation,
    shadows = shadows
);

/// The 2D lights' C layouts: repr(C), checked against the C structs; any
/// values draw (non-positive radii or intensities switch a light off).
pub(crate) fn light_kind(t: std::any::TypeId) -> Option<Kind> {
    use std::any::TypeId;
    [
        TypeId::of::<PointLight>(),
        TypeId::of::<SpotLight>(),
        TypeId::of::<DirectionalLight>(),
    ]
    .contains(&t)
    .then_some(Kind::Plain)
}

/// A point light with the engine's defaults (Rust `d2::PointLight::default()`).
/// ```
/// assert_eq!(ponko_ffi::ponko_point_light2d_default().intensity, 1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_point_light2d_default() -> PonkoPointLight2D {
    to_c(PointLight::default())
}

/// A spot light with the engine's defaults (Rust `d2::SpotLight::default()`).
/// ```
/// assert_eq!(ponko_ffi::ponko_spot_light2d_default().radius, 300.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_spot_light2d_default() -> PonkoSpotLight2D {
    to_c(SpotLight::default())
}

/// A directional light with the engine's defaults (Rust
/// `d2::DirectionalLight::default()`).
/// ```
/// assert!(!ponko_ffi::ponko_directional_light2d_default().shadows);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_directional_light2d_default() -> PonkoDirectionalLight2D {
    to_c(DirectionalLight::default())
}

/// Registers the built-in `PointLight2D` for entity insertion and query columns.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_point_light2d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_point_light2d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<PointLight>(world, out_id) }
}

/// Registers the built-in `SpotLight2D` for entity insertion and query columns.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_spot_light2d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_spot_light2d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<SpotLight>(world, out_id) }
}

/// Registers the built-in `DirectionalLight2D` for entity insertion and
/// query columns.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_directional_light2d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_directional_light2d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<DirectionalLight>(world, out_id) }
}

fn finite(values: &[f32]) -> Result<(), Failure> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(invalid("occluder sizes and points must be finite"))
    }
}

/// Gives `entity` `occluder`, replacing any it had.
unsafe fn set_occluder(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    occluder: impl FnOnce(&mut Ctx, ponko_ecs::Entity) -> Result<Occluder, Failure>,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            let o = occluder(c, e)?;
            c.world.insert(e, o);
            Ok(())
        })
    })
}

/// Gives `entity` a rectangle `size` wide and tall, centered on it, that
/// blocks 2D light (Rust `d2::Occluder::rect`), replacing any occluder it had.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_occluder2d_rect(std::ptr::null_mut(), 0, Default::default()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_occluder2d_rect(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    size: PonkoVec2,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        set_occluder(ctx, entity_bits, |_, _| {
            finite(&[size.x, size.y])?;
            Ok(Occluder::rect(to_c(size)))
        })
    }
}

/// Gives `entity` a circle of `radius` (a 16-sided polygon) that blocks 2D
/// light (Rust `d2::Occluder::circle`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_occluder2d_circle(std::ptr::null_mut(), 0, 1.0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_occluder2d_circle(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    radius: f32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        set_occluder(ctx, entity_bits, |_, _| {
            finite(&[radius])?;
            Ok(Occluder::circle(radius))
        })
    }
}

/// Gives `entity` a closed polygon through `count` `points` (relative to
/// it, either winding, not crossing itself) that blocks 2D light (Rust
/// `d2::Occluder::polygon`). The points are copied.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_occluder2d_polygon(std::ptr::null_mut(), 0, std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `points` holds `count` points (or is null with 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_occluder2d_polygon(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    points: *const PonkoVec2,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        set_occluder(ctx, entity_bits, |_, _| {
            if count > 0 && points.is_null() {
                return Err(invalid("points is null"));
            }
            let points: Vec<Vec2> = if count == 0 {
                Vec::new()
            } else {
                // SAFETY: the caller supplies `count` points.
                std::slice::from_raw_parts(points, count)
                    .iter()
                    .map(|&p| to_c(p))
                    .collect()
            };
            finite(&points.iter().flat_map(|p| [p.x, p.y]).collect::<Vec<_>>())?;
            Ok(Occluder::polygon(&points))
        })
    }
}

/// Gives `entity` a thin wall from `a` to `b` (relative to it) that blocks
/// 2D light toward both sides (Rust `d2::Occluder::line`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_occluder2d_line(std::ptr::null_mut(), 0, Default::default(), Default::default()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_occluder2d_line(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    a: PonkoVec2,
    b: PonkoVec2,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        set_occluder(ctx, entity_bits, |_, _| {
            finite(&[a.x, a.y, b.x, b.y])?;
            Ok(Occluder::line(to_c(a), to_c(b)))
        })
    }
}

/// Gives `entity`, which has a tilemap, an occluder outlining its tiles
/// whose value is in `solid_tiles` (Rust `TileMap::occluder`): the walls
/// cast shadows and stay lit. Call it again after changing the walls.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_tilemap_occluder(std::ptr::null_mut(), 0, std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `solid_tiles` holds `count` values (or is null with 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_tilemap_occluder(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    solid_tiles: *const u16,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        set_occluder(ctx, entity_bits, |c, e| {
            if count > 0 && solid_tiles.is_null() {
                return Err(invalid("solid_tiles is null"));
            }
            let solid: &[u16] = if count == 0 {
                &[]
            } else {
                // SAFETY: the caller supplies `count` values.
                std::slice::from_raw_parts(solid_tiles, count)
            };
            let map = c
                .world
                .get::<TileMap>(e)
                .ok_or_else(|| invalid("entity has no tilemap"))?;
            Ok(map.occluder(|t| solid.contains(&t)))
        })
    }
}

/// Takes `entity`'s occluder away (nothing happens without one).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_remove_occluder2d(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_remove_occluder2d(ctx: *mut PonkoCtx, entity_bits: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            c.world.remove::<Occluder>(e);
            Ok(())
        })
    })
}

/// How many edges `entity`'s occluder has (0 without one): a rectangle
/// has 4, a line 2 (one per side).
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_occluder2d_edge_count(std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_occluder2d_edge_count(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            let n = c.world.get::<Occluder>(e).map_or(0, |o| o.edges().len());
            out(out_count, n)
        })
    })
}

/// Sets the 2D ambient light (Rust `Graphics::set_ambient_light2d`): with
/// `on`, `color` (sRGB) lights every place and turns 2D lighting on; off
/// (the default), the ambient is black wherever 2D lights exist.
/// ```
/// let night = ponko_ffi::PonkoColor { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_ambient_light2d(std::ptr::null_mut(), false, night) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_ambient_light2d(ctx: *mut PonkoCtx, on: bool, color: PonkoColor) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.set_ambient_light2d(on.then(|| to_c::<_, Color>(color)));
            Ok(())
        })
    })
}

/// The 2D ambient light: whether one is set, and its color (left alone
/// when none is).
/// ```
/// let (mut on, mut color) = (false, ponko_ffi::PonkoColor { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ambient_light2d(std::ptr::null_mut(), &mut on, &mut color) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ambient_light2d(
    ctx: *mut PonkoCtx,
    out_on: *mut bool,
    out_color: *mut PonkoColor,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_color.is_null() {
                return Err(invalid("output is null"));
            }
            let ambient = c.gfx.ambient_light2d();
            if let Some(color) = ambient {
                *out_color = to_c(color);
            }
            out(out_on, ambient.is_some())
        })
    })
}

/// Gives every sprite, tile and particle drawn with `texture` the normal
/// map `normal_map` (Rust `Graphics::set_normal_map`); `has_normal_map`
/// false takes it away. Same size and layout as `texture`, green up.
/// ```
/// let t = ponko_ffi::PonkoTexture { id: 0, width: 0, height: 0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_normal_map(std::ptr::null_mut(), t, false, t) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_normal_map(
    ctx: *mut PonkoCtx,
    texture: PonkoTexture,
    has_normal_map: bool,
    normal_map: PonkoTexture,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.set_normal_map(
                texture_from_c(texture),
                has_normal_map.then(|| texture_from_c(normal_map)),
            );
            Ok(())
        })
    })
}

/// `texture`'s normal map: whether it has one, and which (left alone
/// without one).
/// ```
/// let t = ponko_ffi::PonkoTexture { id: 0, width: 0, height: 0 };
/// let (mut has, mut n) = (false, t);
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_normal_map(std::ptr::null_mut(), t, &mut has, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_normal_map(
    ctx: *mut PonkoCtx,
    texture: PonkoTexture,
    out_has: *mut bool,
    out_normal_map: *mut PonkoTexture,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_normal_map.is_null() {
                return Err(invalid("output is null"));
            }
            let map = c.gfx.normal_map(texture_from_c(texture));
            if let Some(map) = map {
                *out_normal_map = texture_to_c(map);
            }
            out(out_has, map.is_some())
        })
    })
}
