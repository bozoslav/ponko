//! Levels of detail beyond `ponko_ctx_set_lod_group`: hysteresis (Rust
//! `LodHysteresis`) and a group's levels as instance batch levels (Rust
//! `LodGroup::instance_levels`).
use crate::PonkoVec3;
use crate::ctx::{ctx_access, out};
use crate::three::{PonkoInstanceLevel, PonkoLodLevel, PonkoLodSettings, lod_levels};
use crate::types::to_c;
use crate::{PonkoCtx, PonkoStatus, boundary, error, invalid};
use ponko::prelude::Entity;
use ponko::render::{LodGroup, LodHysteresis, LodSettings};

/// Holds each level of `entity`'s LOD group until the group's screen height
/// moves `fraction` (0 to 1) past the level's own range (Rust
/// `LodHysteresis`), so a camera resting near a threshold keeps one level
/// instead of switching every frame. 0 switches exactly at the thresholds,
/// as without it. Each camera holds its own choice. A fraction outside
/// 0..=1 (or NaN), or a stale entity, is an error.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_lod_hysteresis(std::ptr::null_mut(), 0, 0.1) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_lod_hysteresis(ctx: *mut PonkoCtx, entity: u64, fraction: f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            if !(0.0..=1.0).contains(&fraction) {
                return Err(invalid("hysteresis must be between 0 and 1"));
            }
            c.world.insert(entity, LodHysteresis(fraction));
            Ok(())
        })
    })
}

/// `entity`'s LOD hysteresis (`ponko_ctx_set_lod_hysteresis`); 0 when it
/// has none. A stale entity is an error.
/// ```
/// let mut f = 1.0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_lod_hysteresis(std::ptr::null_mut(), 0, &mut f) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_fraction` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_lod_hysteresis(
    ctx: *mut PonkoCtx,
    entity: u64,
    out_fraction: *mut f32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            out(out_fraction, c.world.get::<LodHysteresis>(entity).map_or(0.0, |h| h.0))
        })
    })
}

/// The levels of an instance batch that draws copies of a LOD group the way
/// the group itself would (Rust `LodGroup::instance_levels`): the group is
/// `count` levels for an object `size` tall, as for
/// `ponko_ctx_set_lod_group`, each copy scaled by `scale`, seen by a camera
/// with vertical field of view `fov_y` (radians) under `settings`. Writes
/// up to `capacity` levels to `out_levels` and how many there are to
/// `*out_count`, ready for `ponko_ctx_create_instance_batch_levels`.
///
/// `PONKO_NOT_FOUND` when the group can't be a batch: its `center` is away
/// from the origin, a renderer has an offset or a color other than white,
/// or nothing would ever be drawn. A batch doesn't crossfade.
/// ```
/// let mut n = 0;
/// let s = ponko_ffi::PonkoLodSettings { bias: 1.0, crossfade: true };
/// assert_eq!(unsafe { ponko_ffi::ponko_lod_instance_levels(1.0, Default::default(), std::ptr::null(), 0, 1.0, 1.0, s, std::ptr::null_mut(), 0, &mut n) } as u32, 2);
/// ```
/// # Safety
/// `levels` holds `count` values (may be null when 0), each pointing at as
/// many renderers as it says; `out_levels` writable for `capacity` values
/// (or null with capacity 0); `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_lod_instance_levels(
    size: f32,
    center: PonkoVec3,
    levels: *const PonkoLodLevel,
    count: usize,
    scale: f32,
    fov_y: f32,
    settings: PonkoLodSettings,
    out_levels: *mut PonkoInstanceLevel,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_count.is_null() || (out_levels.is_null() && capacity > 0) {
            return Err(invalid("output is null"));
        }
        let group = LodGroup::new(size, lod_levels(levels, count)?).with_center(to_c(center));
        let mut lod = LodSettings::default();
        lod.bias = settings.bias;
        lod.crossfade = settings.crossfade;
        let found = group.instance_levels(scale, fov_y, &lod).ok_or_else(|| {
            error(
                PonkoStatus::PONKO_NOT_FOUND,
                "that LOD group can't be an instance batch (off-center, offset or tinted renderers, or nothing drawn)",
            )
        })?;
        for (i, l) in found.iter().take(capacity).enumerate() {
            out_levels.add(i).write(PonkoInstanceLevel {
                mesh: to_c(l.mesh),
                min_distance: l.min_distance,
                max_distance: l.max_distance,
                camera: l.camera,
                shadows: l.shadows,
            });
        }
        out(out_count, found.len())
    })
}
