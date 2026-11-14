//! 3D particles from C (Rust `ParticleEmitter3D`, `ParticleSettings3D`).
//! Fill a `PonkoParticleSettings3D` (start from
//! `ponko_particle_settings3d_default()`), then give an entity with a 3D
//! transform an emitter with `ponko_ctx_set_particle_emitter3d`.

use ponko::d3::ParticleBlend as ParticleBlend3D;
use ponko::d3::ParticleEmitter as ParticleEmitter3D;
use ponko::d3::ParticleSettings as ParticleSettings3D;
use ponko::d3::ParticleShape as ParticleShape3D;
use ponko::particles::ParticleSpace as ParticleSpace3D;
use ponko::{
    particles::ParticleCurve,
    particles::ParticleGradient,
    prelude::{Color, Ctx, Entity, Vec3},
};

use crate::ctx::{ctx_access, out};
use crate::three::PonkoVec3;
use crate::types::{PonkoColor, PonkoTexture, texture_from_c, to_c};
use crate::{Failure, PonkoCtx, PonkoStatus, boundary, invalid, invalid_owned};

/// Where particles start (`PonkoParticleSettings3D::shape`).
/// ```
/// assert_eq!(ponko_ffi::PonkoParticleShape3D::PONKO_PARTICLES_CONE as u32, 2);
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoParticleShape3D {
    /// The emitter's origin, moving along +Y.
    PONKO_PARTICLES_POINT,
    /// Inside a ball of `shape_size.x` radius, moving outward.
    PONKO_PARTICLES_SPHERE,
    /// The origin, moving inside a cone around +Y with half angle `shape_size.x` (radians).
    PONKO_PARTICLES_CONE,
    /// Inside a box with half extents `shape_size`, moving along +Y.
    PONKO_PARTICLES_BOX,
}

/// How particles combine with the scene (`PonkoParticleSettings3D::blend`).
/// ```
/// assert_eq!(ponko_ffi::PonkoParticleBlend3D::PONKO_PARTICLES_ADDITIVE as u32, 1);
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoParticleBlend3D {
    /// Ordinary transparency, sorted back to front.
    PONKO_PARTICLES_ALPHA,
    /// Adds light to the scene (fire, sparks, magic).
    PONKO_PARTICLES_ADDITIVE,
}

/// Whether living particles follow the emitter (`PonkoParticleSettings3D::space`).
/// ```
/// assert_eq!(ponko_ffi::PonkoParticleSpace3D::PONKO_PARTICLES_LOCAL as u32, 1);
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoParticleSpace3D {
    /// Particles stay where they were born; gravity is world down.
    PONKO_PARTICLES_WORLD,
    /// Particles move with the entity.
    PONKO_PARTICLES_LOCAL,
}

/// One key of a size curve: `value` at normalized lifetime `t` (0..1).
/// ```
/// let k = ponko_ffi::PonkoParticleKey { t: 1.0, value: 0.0 };
/// assert_eq!(k.t, 1.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoParticleKey {
    /// Normalized lifetime, 0 at birth and 1 at death.
    pub t: f32,
    /// Size multiplier at `t`.
    pub value: f32,
}

/// One key of a color gradient: `color` at normalized lifetime `t` (0..1).
/// ```
/// let k = ponko_ffi::PonkoParticleColorKey { t: 0.0, color: ponko_ffi::PonkoColor { r: 1.0, g: 0.5, b: 0.0, a: 1.0 } };
/// assert_eq!(k.color.g, 0.5);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoParticleColorKey {
    /// Normalized lifetime, 0 at birth and 1 at death.
    pub t: f32,
    /// Tint at `t`.
    pub color: PonkoColor,
}

/// An emitter's settings (Rust `ParticleSettings3D`). Ranges are `_min`/`_max`
/// pairs. The key pointers are only read during the call that takes the
/// settings; null with a count of 0 keeps the default curve.
/// ```
/// let s = ponko_ffi::ponko_particle_settings3d_default();
/// assert_eq!(s.max_particles, 1024);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoParticleSettings3D {
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
    /// Start speed range, units per second. Default 1..2.
    pub speed_min: f32,
    /// See `speed_min`.
    pub speed_max: f32,
    /// A `PonkoParticleShape3D`. Default point.
    pub shape: u32,
    /// Sphere radius (x), cone half angle (x) or box half extents.
    pub shape_size: PonkoVec3,
    /// Acceleration. Default (0, -9.81, 0).
    pub gravity: PonkoVec3,
    /// Velocity damping per second. Default 0.
    pub drag: f32,
    /// Start size range in world units. Default 0.1..0.2.
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
    /// Tint over lifetime; null for white.
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
    /// A `PonkoParticleBlend3D`. Default alpha.
    pub blend: u32,
    /// Most living particles; storage is made up front. Default 1024.
    pub max_particles: u32,
    /// A `PonkoParticleSpace3D`. Default world.
    pub space: u32,
    /// Random seed: equal seeds give equal particles. Default 1.
    pub seed: u64,
}

/// Default particle settings (same as Rust's `ParticleSettings3D::default()`).
/// ```
/// let s = ponko_ffi::ponko_particle_settings3d_default();
/// assert_eq!(s.gravity.y, -9.81);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_particle_settings3d_default() -> PonkoParticleSettings3D {
    let d = ParticleSettings3D::default();
    PonkoParticleSettings3D {
        rate: d.rate,
        burst: d.burst as u32,
        duration: d.duration,
        looping: d.looping,
        lifetime_min: d.lifetime.0,
        lifetime_max: d.lifetime.1,
        speed_min: d.speed.0,
        speed_max: d.speed.1,
        shape: PonkoParticleShape3D::PONKO_PARTICLES_POINT as u32,
        shape_size: PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 },
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
        blend: PonkoParticleBlend3D::PONKO_PARTICLES_ALPHA as u32,
        max_particles: d.max_particles as u32,
        space: PonkoParticleSpace3D::PONKO_PARTICLES_WORLD as u32,
        seed: d.seed,
    }
}

/// A `PonkoParticleBlend3D` value.
pub(crate) fn blend_from_c(blend: u32) -> Result<ParticleBlend3D, Failure> {
    match blend {
        0 => Ok(ParticleBlend3D::Alpha),
        1 => Ok(ParticleBlend3D::Additive),
        _ => Err(invalid("unknown particle blend")),
    }
}

/// A curve from C keys, or `None` for no keys (keep the default).
pub(crate) fn curve_from_c(keys: &[PonkoParticleKey]) -> Result<Option<ParticleCurve>, Failure> {
    if !keys.iter().all(|k| finite(&[k.t, k.value])) {
        return Err(invalid("particle curve keys must be finite"));
    }
    ordered(keys.iter().map(|k| k.t))?;
    Ok((!keys.is_empty()).then(|| ParticleCurve::new(keys.iter().map(|k| (k.t, k.value)).collect())))
}

/// A gradient from C keys, or `None` for no keys (keep the default).
pub(crate) fn gradient_from_c(keys: &[PonkoParticleColorKey]) -> Result<Option<ParticleGradient>, Failure> {
    if !keys
        .iter()
        .all(|k| finite(&[k.t, k.color.r, k.color.g, k.color.b, k.color.a]))
    {
        return Err(invalid("particle curve keys must be finite"));
    }
    ordered(keys.iter().map(|k| k.t))?;
    Ok((!keys.is_empty())
        .then(|| ParticleGradient::new(keys.iter().map(|k| (k.t, to_c::<_, Color>(k.color))).collect())))
}

/// Key times the engine accepts: inside 0..=1 and strictly increasing (it panics otherwise).
fn ordered(times: impl Iterator<Item = f32>) -> Result<(), Failure> {
    let times: Vec<f32> = times.collect();
    if times.iter().all(|t| (0.0..=1.0).contains(t)) && times.windows(2).all(|w| w[0] < w[1]) {
        Ok(())
    } else {
        Err(invalid("particle key times must be in 0..=1 and increasing"))
    }
}

pub(crate) fn finite(values: &[f32]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// Checks C settings, so bad values are errors rather than panics.
///
/// # Safety
/// The key pointers are null or point to their counts of keys.
unsafe fn settings_from_c(s: &PonkoParticleSettings3D) -> Result<ParticleSettings3D, Failure> {
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
    if !finite(&[
        s.rate, s.duration, s.drag, size.x, size.y, size.z, gravity.x, gravity.y, gravity.z,
    ]) || s.rate < 0.0
        || s.duration < 0.0
        || s.drag < 0.0
    {
        return Err(invalid(
            "particle rate, duration and drag must be finite and not negative",
        ));
    }
    if s.max_particles == 0 || s.sheet_columns == 0 || s.sheet_rows == 0 {
        return Err(invalid("max_particles and sheet size must be at least 1"));
    }
    let shape = match s.shape {
        0 => ParticleShape3D::Point,
        1 => ParticleShape3D::Sphere { radius: size.x },
        2 => ParticleShape3D::Cone { angle: size.x },
        3 => ParticleShape3D::Box {
            half_extents: to_c(size),
        },
        _ => return Err(invalid("unknown particle shape")),
    };
    let blend = blend_from_c(s.blend)?;
    let space = match s.space {
        0 => ParticleSpace3D::World,
        1 => ParticleSpace3D::Local,
        _ => return Err(invalid("unknown particle space")),
    };
    // SAFETY: caller contract.
    let size_keys = unsafe { keys(s.size_keys, s.size_key_count, "size_keys")? };
    let color_keys = unsafe { keys(s.color_keys, s.color_key_count, "color_keys")? };
    let (size_curve, color_gradient) = (curve_from_c(size_keys)?, gradient_from_c(color_keys)?);
    let defaults = ParticleSettings3D::default();
    Ok(ParticleSettings3D::default()
        .with_rate(s.rate)
        .with_burst(s.burst as usize)
        .with_duration(s.duration)
        .with_looping(s.looping)
        .with_lifetime(s.lifetime_min, s.lifetime_max)
        .with_speed(s.speed_min, s.speed_max)
        .with_shape(shape)
        .with_gravity(to_c::<_, Vec3>(gravity))
        .with_drag(s.drag)
        .with_size(s.size_min, s.size_max)
        .with_rotation(s.rotation_min, s.rotation_max)
        .with_size_over_life(size_curve.unwrap_or(defaults.size_over_life))
        .with_color_over_life(color_gradient.unwrap_or(defaults.color_over_life))
        .with_texture(s.has_texture.then(|| texture_from_c(s.texture)))
        .with_sheet(s.sheet_columns, s.sheet_rows)
        .with_blend(blend)
        .with_max_particles(s.max_particles as usize)
        .with_space(space)
        .with_seed(s.seed))
}

/// # Safety
/// `ptr` is null or points to `count` values.
pub(crate) unsafe fn keys<'a, T>(ptr: *const T, count: usize, name: &str) -> Result<&'a [T], Failure> {
    if count == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err(invalid_owned(format!("{name} is null with a nonzero count")));
    }
    // SAFETY: caller contract.
    Ok(unsafe { std::slice::from_raw_parts(ptr, count) })
}

pub(crate) fn entity(c: &Ctx, bits: u64) -> Result<Entity, Failure> {
    let e = Entity::from_bits(bits);
    if c.world.is_alive(e) {
        Ok(e)
    } else {
        Err(invalid("no such entity"))
    }
}

fn emitter(c: &mut Ctx, bits: u64) -> Result<&mut ParticleEmitter3D, Failure> {
    let e = entity(c, bits)?;
    c.world
        .get_mut::<ParticleEmitter3D>(e)
        .ok_or_else(|| invalid("entity has no particle emitter"))
}

/// Gives `entity` (which needs a 3D transform to draw) a new particle
/// emitter, replacing any it had along with its living particles.
/// ```
/// let s = ponko_ffi::ponko_particle_settings3d_default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_particle_emitter3d(std::ptr::null_mut(), 0, &s) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `settings` null or valid, its key pointers as documented.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_particle_emitter3d(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    settings: *const PonkoParticleSettings3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            let s = settings.as_ref().ok_or_else(|| invalid("settings is null"))?;
            let emitter = ParticleEmitter3D::new(settings_from_c(s)?);
            c.world.insert(e, emitter);
            Ok(())
        })
    })
}

/// Emits `count` particles from `entity`'s emitter on the next frame (capped
/// by its `max_particles`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_particle_burst(std::ptr::null_mut(), 0, 10) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_particle_burst(ctx: *mut PonkoCtx, entity_bits: u64, count: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            emitter(c, entity_bits)?.burst(count as usize);
            Ok(())
        })
    })
}

/// How many of `entity`'s particles are alive.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_particle_live_count(std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_particle_live_count(
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

/// Removes `entity`'s particle emitter and its particles.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_remove_particle_emitter3d(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_remove_particle_emitter3d(ctx: *mut PonkoCtx, entity_bits: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            c.world.remove::<ParticleEmitter3D>(e);
            Ok(())
        })
    })
}
