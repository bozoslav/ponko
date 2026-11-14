//! What world saves leave out: the `NotSaved` marker, and spawning a level
//! again after a load without the objects the save holds. See
//! `ponko::reflect::NotSaved` and `LevelOptions::without_saved_objects` in Rust.
use std::ffi::c_char;

use ponko::reflect::NotSaved;
use ponko_ecs::Entity;

use crate::ctx::{PonkoCtx, ctx_access, out, text};
use crate::levels::{PonkoLevelOptions, options, vec};
use crate::types::PonkoVec2;
use crate::{PonkoStatus, PonkoWorld, access, boundary, engine_error, error, invalid};

/// Whether world saves (`ponko_world_save`, `ponko_ctx_save_world`, scene
/// saves) hold `entity`: `saved` false leaves it out, true puts it back
/// (the default). `ponko_ctx_spawn_level` leaves out everything it spawns
/// but the prefab objects. `PONKO_NOT_FOUND` for a dead entity.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { assert_eq!(ponko_world_set_saved(w, 42, false), PonkoStatus::PONKO_NOT_FOUND); ponko_world_free(w); }
/// ```
/// # Safety
/// `world` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_world_set_saved(world: *mut PonkoWorld, entity: u64, saved: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        access(world, |w, _| {
            let e = Entity::from_bits(entity);
            if !w.is_alive(e) {
                return Err(error(PonkoStatus::PONKO_NOT_FOUND, "entity not found"));
            }
            if saved {
                w.remove::<NotSaved>(e);
            } else {
                w.insert(e, NotSaved);
            }
            Ok(())
        })
    })
}

/// Writes whether world saves hold `entity` (see `ponko_world_set_saved`).
/// `PONKO_NOT_FOUND` for a dead entity.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut saved = false; assert_eq!(ponko_world_is_saved(w, 42, &mut saved), PonkoStatus::PONKO_NOT_FOUND); ponko_world_free(w); }
/// ```
/// # Safety
/// `world` must be null or live; `out_saved` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_world_is_saved(
    world: *mut PonkoWorld,
    entity: u64,
    out_saved: *mut bool,
) -> PonkoStatus {
    boundary(|| {
        if out_saved.is_null() {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract.
        unsafe {
            access(world, |w, _| {
                let e = Entity::from_bits(entity);
                if !w.is_alive(e) {
                    return Err(error(PonkoStatus::PONKO_NOT_FOUND, "entity not found"));
                }
                out(out_saved, !w.has::<NotSaved>(e))
            })
        }
    })
}

/// `ponko_ctx_spawn_level` without the objects a world save holds (those
/// spawned as prefabs): for spawning the level again after loading a save,
/// which brings them back as they were. `options` may be null.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_spawn_level_without_saved_objects(std::ptr::null_mut(), c"a.tmx".as_ptr(), Default::default(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// As `ponko_ctx_spawn_level`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_spawn_level_without_saved_objects(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    at: PonkoVec2,
    options: *const PonkoLevelOptions,
    out_root: *mut u64,
) -> PonkoStatus {
    boundary(|| {
        if out_root.is_null() {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract.
        unsafe {
            let options = self::options(options)?.without_saved_objects();
            let path = text(path)?;
            ctx_access(ctx, |c| {
                let root = c.spawn_level_with(path, vec(at), options).map_err(engine_error)?;
                out(out_root, root.to_bits())
            })
        }
    })
}
