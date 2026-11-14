//! 2D particles from C (Rust `d2::ParticleEmitter`, `d2::ParticleSettings`).
//! Fill a `PonkoParticleSettings2D` (start from
//! `ponko_particle_settings2d_default()`), then give an entity with a 2D
//! transform an emitter with `ponko_ctx_set_particle_emitter2d`. Particles
//! draw with the sprites at the settings' `z`, with a `PONKO_BLEND_*` mode
//! and optionally a sprite material.

use ponko::d2::{ParticleEmitter, ParticleSettings, ParticleShape};
use ponko::particles::ParticleSpace;
use ponko::prelude::{Ctx, Vec2};
use ponko::render::SpriteMaterial;

use crate::ctx::{ctx_access, out};
use crate::particles::{PonkoParticleColorKey, PonkoParticleKey, curve_from_c, entity, finite, gradient_from_c, keys};
use crate::sprite_material::PonkoSpriteMaterial;
use crate::types::{PonkoTexture, PonkoVec2, texture_from_c, to_c};
use crate::{Failure, PonkoCtx, PonkoStatus, boundary, invalid, invalid_owned};

/// Where 2D particles start (`PonkoParticleSettings2D::shape`). Up is toward
/// the top of the screen (negative y).
/// ```
/// assert_eq!(ponko_ffi::PonkoParticleShape2D::PONKO_PARTICLES2D_CONE as u32, 2);
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoParticleShape2D {
    /// The emitter's origin, moving up.
    PONKO_PARTICLES2D_POINT,
    /// Inside a disc of `shape_size.x` radius, moving outward.
    PONKO_PARTICLES2D_CIRCLE,
    /// The origin, moving up to `shape_size.x` radians away from up.
    PONKO_PARTICLES2D_CONE,
    /// Inside a rectangle with half extents `shape_size`, moving up.
    PONKO_PARTICLES2D_RECT,
}

/// A 2D emitter's settings (Rust `d2::ParticleSettings`), in pixels. Ranges
/// are `_min`/`_max` pairs. The key pointers are only read during the call
/// that takes the settings; null with a count of 0 keeps the default curve.
/// ```
/// let s = ponko_ffi::ponko_particle_settings2d_default();
/// assert_eq!(s.max_particles, 1024);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoParticleSettings2D {
    /// Particles per second. 0 emits only bursts. Default 10.
    pub rate: f32,
    /// Particles emitted at once on the first frame. Default 0.
    pub burst: u32,
    /// Seconds of emission before stopping or looping. Default 1.
    pub duration: f32,
    /// Emit again after `duration`. Default true.
    pub looping: bool,
    /// Lifetime range in seconds; the minimum must be positive. Default 1..2.
    pub lifetime_min: f32,
    /// See `lifetime_min`.
    pub lifetime_max: f32,
    /// Start speed range, pixels per second. Default 50..100.
    pub speed_min: f32,
    /// See `speed_min`.
    pub speed_max: f32,
    /// A `PonkoParticleShape2D`. Default point.
    pub shape: u32,
    /// Circle radius (x), cone half angle (x) or rectangle half extents.
    pub shape_size: PonkoVec2,
    /// Acceleration, pixels per second squared (y down). Default (0, 200).
    pub gravity: PonkoVec2,
    /// Velocity damping per second. Default 0.
    pub drag: f32,
    /// Start width range in pixels; the height follows the image. Default 4..8.
    pub size_min: f32,
    /// See `size_min`.
    pub size_max: f32,
    /// Start rotation range in radians. Default 0..0.
    pub rotation_min: f32,
    /// See `rotation_min`.
    pub rotation_max: f32,
    /// Size multiplier over lifetime; null for a constant 1.
    pub size_keys: *const PonkoParticleKey,
    /// Number of `size_keys`.
    pub size_key_count: usize,
    /// Tint over lifetime; null for white fading out.
    pub color_keys: *const PonkoParticleColorKey,
    /// Number of `color_keys`.
    pub color_key_count: usize,
    /// Use `texture`; otherwise particles are plain squares. Default false.
    pub has_texture: bool,
    /// The particle image (or a sprite sheet).
    pub texture: PonkoTexture,
    /// Sprite sheet columns and rows; frames play over lifetime. Default 1, 1.
    pub sheet_columns: u32,
    /// See `sheet_columns`.
    pub sheet_rows: u32,
    /// A `PONKO_BLEND_*` mode, as for sprites. Default alpha.
    pub blend: u32,
    /// A sprite material for the particles; `PONKO_HANDLE_NONE` (default)
    /// draws the built-in look.
    pub material: PonkoSpriteMaterial,
    /// Draw order among sprites: higher is on top. Default 0.
    pub z: f32,
    /// Most living particles; storage is made up front. Default 1024.
    pub max_particles: u32,
    /// A `PonkoParticleSpace3D` value (`PONKO_PARTICLES_WORLD` or
    /// `PONKO_PARTICLES_LOCAL`, shared with 3D). Default world.
    pub space: u32,
    /// Random seed: equal seeds give equal particles. Default 1.
    pub seed: u64,
}

/// Default 2D particle settings (same as Rust's `d2::ParticleSettings::default()`).
/// ```
/// let s = ponko_ffi::ponko_particle_settings2d_default();
/// assert_eq!(s.gravity.y, 200.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_particle_settings2d_default() -> PonkoParticleSettings2D {
    let d = ParticleSettings::default();
    PonkoParticleSettings2D {
        rate: d.rate,
        burst: d.burst as u32,
        duration: d.duration,
        looping: d.looping,
        lifetime_min: d.lifetime.0,
        lifetime_max: d.lifetime.1,
        speed_min: d.speed.0,
        speed_max: d.speed.1,
        shape: PonkoParticleShape2D::PONKO_PARTICLES2D_POINT as u32,
        shape_size: PonkoVec2 { x: 0.0, y: 0.0 },
        gravity: to_c(d.gravity),
        drag: d.drag,
        size_min: d.size.0,
        size_max: d.size.1,
        rotation_min: d.rotation.0,
        rotation_max: d.rotation.1,
        size_keys: std::ptr::null(),
        size_key_count: 0,
        color_keys: std::ptr::null(),
        color_key_count: 0,
        has_texture: false,
        texture: PonkoTexture {
            id: 0,
            width: 0,
            height: 0,
        },
        sheet_columns: d.sheet.0,
        sheet_rows: d.sheet.1,
        blend: d.blend as u32,
        material: PonkoSpriteMaterial {
            id: crate::PONKO_HANDLE_NONE,
        },
        z: d.z,
        max_particles: d.max_particles as u32,
        space: 0,
        seed: d.seed,
    }
}

/// Checks C settings, so bad values are errors rather than panics.
///
/// # Safety
/// The key pointers are null or point to their counts of keys.
unsafe fn settings_from_c(s: &PonkoParticleSettings2D) -> Result<ParticleSettings, Failure> {
    let ranges = [
        ("lifetime", s.lifetime_min, s.lifetime_max),
        ("speed", s.speed_min, s.speed_max),
        ("size", s.size_min, s.size_max),
        ("rotation", s.rotation_min, s.rotation_max),
    ];
    for (name, lo, hi) in ranges {
        if !(lo.is_finite() && hi.is_finite() && lo <= hi) {
            return Err(invalid_owned(format!(
                "particle {name} range must be finite with min <= max"
            )));
        }
        if name != "rotation" && lo < 0.0 {
            return Err(invalid_owned(format!("particle {name} must not be negative")));
        }
    }
    if s.lifetime_min <= 0.0 {
        return Err(invalid("particle lifetime must be positive"));
    }
    let (size, gravity) = (s.shape_size, s.gravity);
    if !finite(&[s.rate, s.duration, s.drag, size.x, size.y, gravity.x, gravity.y]) || s.rate < 0.0 || s.drag < 0.0 {
        return Err(invalid(
            "particle rate, duration and drag must be finite and not negative",
        ));
    }
    if s.duration <= 0.0 {
        return Err(invalid("particle duration must be positive"));
    }
    if !s.z.is_finite() {
        return Err(invalid("particle z must be finite"));
    }
    if s.max_particles == 0
        || s.sheet_columns == 0
        || s.sheet_rows == 0
        || s.sheet_columns.checked_mul(s.sheet_rows).is_none()
    {
        return Err(invalid("max_particles and sheet size must be at least 1"));
    }
    let shape = match s.shape {
        0 => ParticleShape::Point,
        1 => ParticleShape::Circle { radius: size.x },
        2 => ParticleShape::Cone { angle: size.x },
        3 => ParticleShape::Rect {
            half_extents: to_c::<_, Vec2>(size),
        },
        _ => return Err(invalid("unknown particle shape")),
    };
    let blend = ponko::render::BlendMode::from_u32(s.blend)
        .ok_or_else(|| invalid("blend must be 0 (alpha), 1 (additive), 2 (multiply) or 3 (premultiplied)"))?;
    let space = match s.space {
        0 => ParticleSpace::World,
        1 => ParticleSpace::Local,
        _ => return Err(invalid("unknown particle space")),
    };
    // SAFETY: caller contract.
    let size_keys = unsafe { keys(s.size_keys, s.size_key_count, "size_keys")? };
    let color_keys = unsafe { keys(s.color_keys, s.color_key_count, "color_keys")? };
    let (size_curve, color_gradient) = (curve_from_c(size_keys)?, gradient_from_c(color_keys)?);
    let defaults = ParticleSettings::default();
    let material = (s.material.id != crate::PONKO_HANDLE_NONE).then(|| to_c::<_, SpriteMaterial>(s.material));
    Ok(ParticleSettings::default()
        .with_rate(s.rate)
        .with_burst(s.burst as usize)
        .with_duration(s.duration)
        .with_looping(s.looping)
        .with_lifetime(s.lifetime_min, s.lifetime_max)
        .with_speed(s.speed_min, s.speed_max)
        .with_shape(shape)
        .with_gravity(to_c::<_, Vec2>(gravity))
        .with_drag(s.drag)
        .with_size(s.size_min, s.size_max)
        .with_rotation(s.rotation_min, s.rotation_max)
        .with_size_over_life(size_curve.unwrap_or(defaults.size_over_life))
        .with_color_over_life(color_gradient.unwrap_or(defaults.color_over_life))
        .with_texture(s.has_texture.then(|| texture_from_c(s.texture)))
        .with_sheet(s.sheet_columns, s.sheet_rows)
        .with_blend(blend)
        .with_material(material)
        .with_z(s.z)
        .with_max_particles(s.max_particles as usize)
        .with_space(space)
        .with_seed(s.seed))
}

fn emitter(c: &mut Ctx, bits: u64) -> Result<&mut ParticleEmitter, Failure> {
    let e = entity(c, bits)?;
    c.world
        .get_mut::<ParticleEmitter>(e)
        .ok_or_else(|| invalid("entity has no 2D particle emitter"))
}

/// Gives `entity` (which needs a 2D transform to draw) a new 2D particle
/// emitter, replacing any it had along with its living particles.
/// ```
/// let s = ponko_ffi::ponko_particle_settings2d_default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_particle_emitter2d(std::ptr::null_mut(), 0, &s) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `settings` null or valid, its key pointers as documented.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_particle_emitter2d(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    settings: *const PonkoParticleSettings2D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            let s = settings.as_ref().ok_or_else(|| invalid("settings is null"))?;
            let emitter = ParticleEmitter::new(settings_from_c(s)?);
            c.world.insert(e, emitter);
            Ok(())
        })
    })
}

/// Emits `count` particles from `entity`'s 2D emitter on the next frame
/// (capped by its `max_particles`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_particle_burst2d(std::ptr::null_mut(), 0, 10) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_particle_burst2d(ctx: *mut PonkoCtx, entity_bits: u64, count: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            emitter(c, entity_bits)?.burst(count as usize);
            Ok(())
        })
    })
}

/// How many of `entity`'s 2D particles are alive.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_particle_live_count2d(std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_particle_live_count2d(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    out_count: *mut u32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let n = emitter(c, entity_bits)?.live_count();
            out(out_count, n as u32)
        })
    })
}

/// Advances `entity`'s 2D emitter by `dt` seconds by hand (Rust
/// `ParticleEmitter::update`), at the entity's 2D transform: for tests,
/// previews and headless runs, which skip the frame phase where the engine
/// advances emitters.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_particle_update2d(std::ptr::null_mut(), 0, 0.1) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_particle_update2d(ctx: *mut PonkoCtx, entity_bits: u64, dt: f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            let transform = c.world.get::<ponko::d2::Transform>(e).copied().unwrap_or_default();
            emitter(c, entity_bits)?.update(dt, &transform);
            Ok(())
        })
    })
}

/// Removes `entity`'s 2D particle emitter and its particles.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_remove_particle_emitter2d(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_remove_particle_emitter2d(ctx: *mut PonkoCtx, entity_bits: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            c.world.remove::<ParticleEmitter>(e);
            Ok(())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_c_settings_are_the_rust_defaults() {
        let defaults = ponko_particle_settings2d_default();
        // SAFETY: no key pointers.
        let back = unsafe { settings_from_c(&defaults) }.ok();
        assert_eq!(back, Some(ParticleSettings::default()));
    }

    #[test]
    fn bad_c_settings_are_errors() {
        let base = ponko_particle_settings2d_default();
        let cases = [
            PonkoParticleSettings2D {
                lifetime_min: 0.0,
                ..base
            },
            PonkoParticleSettings2D { blend: 9, ..base },
            PonkoParticleSettings2D { shape: 9, ..base },
            PonkoParticleSettings2D { space: 2, ..base },
            PonkoParticleSettings2D { sheet_rows: 0, ..base },
            PonkoParticleSettings2D { duration: 0.0, ..base },
            PonkoParticleSettings2D { z: f32::NAN, ..base },
            PonkoParticleSettings2D {
                size_key_count: 2,
                ..base
            },
        ];
        for s in cases {
            // SAFETY: the one bad pointer is null with a count, which is refused.
            assert!(unsafe { settings_from_c(&s) }.is_err(), "{s:?}");
        }
    }
}
