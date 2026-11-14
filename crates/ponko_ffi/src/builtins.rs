//! The engine's built-in systems from C: adding one back in another place
//! (Rust `ponko::systems::*` with `Game::add`) and what each declares it
//! touches (`systems::step_physics_access` and the other `*_access`).
use crate::access::PonkoAccess;
use crate::game::{
    PONKO_SYSTEM_ANIMATE_SPRITES, PONKO_SYSTEM_DRAW_PARTICLES, PONKO_SYSTEM_DRAW_SPRITES, PONKO_SYSTEM_DRAW_TILEMAPS,
    PONKO_SYSTEM_FOLLOW_ATTACHMENTS, PONKO_SYSTEM_INTERPOLATE_PHYSICS3D, PONKO_SYSTEM_PHYSICS, PONKO_SYSTEM_PHYSICS3D,
    PONKO_SYSTEM_REBUILD_SPATIAL_GRID, PONKO_SYSTEM_STEP_NAVMESH3D, PONKO_SYSTEM_UPDATE_ANIMATORS,
    PONKO_SYSTEM_UPDATE_AUDIO3D, PONKO_SYSTEM_UPDATE_PARTICLES, PonkoGame, build, phase,
};
use crate::{Failure, PonkoStatus, boundary, invalid};
use ponko::access::SystemAccess;
use ponko::prelude::{Game, IntoSystem, Phase};
use ponko::systems;

/// What built-in `id` declares it touches. Built-ins without a declaration
/// may use anything: exclusive.
fn builtin_access(id: u32) -> Result<SystemAccess, Failure> {
    Ok(match id {
        #[cfg(feature = "physics")]
        PONKO_SYSTEM_PHYSICS => systems::step_physics_access(),
        #[cfg(not(feature = "physics"))]
        PONKO_SYSTEM_PHYSICS => return Err(invalid("this library was built without 2D physics")),
        #[cfg(feature = "physics3d")]
        PONKO_SYSTEM_PHYSICS3D => systems::step_physics3d_access(),
        #[cfg(feature = "physics3d")]
        PONKO_SYSTEM_INTERPOLATE_PHYSICS3D => systems::interpolate_physics3d_access(),
        #[cfg(not(feature = "physics3d"))]
        PONKO_SYSTEM_PHYSICS3D | PONKO_SYSTEM_INTERPOLATE_PHYSICS3D => {
            return Err(invalid("this library was built without 3D physics"));
        }
        PONKO_SYSTEM_STEP_NAVMESH3D => systems::step_navmesh3d_access(),
        PONKO_SYSTEM_REBUILD_SPATIAL_GRID
        | PONKO_SYSTEM_ANIMATE_SPRITES
        | PONKO_SYSTEM_DRAW_TILEMAPS
        | PONKO_SYSTEM_DRAW_SPRITES
        | PONKO_SYSTEM_FOLLOW_ATTACHMENTS
        | PONKO_SYSTEM_UPDATE_ANIMATORS
        | PONKO_SYSTEM_UPDATE_PARTICLES
        | PONKO_SYSTEM_UPDATE_AUDIO3D
        | PONKO_SYSTEM_DRAW_PARTICLES => SystemAccess::exclusive(),
        _ => return Err(invalid("not a PONKO_SYSTEM_* built-in")),
    })
}

/// Adds built-in `id` to `g` in `p`, with its declaration where it has one;
/// an unknown id leaves `g` as it was and says why.
fn add_builtin(g: Game, p: Phase, id: u32) -> (Game, Result<(), Failure>) {
    let g = match id {
        #[cfg(feature = "physics")]
        PONKO_SYSTEM_PHYSICS => g.add(p, systems::step_physics.access(systems::step_physics_access())),
        #[cfg(feature = "physics3d")]
        PONKO_SYSTEM_PHYSICS3D => g.add(p, systems::step_physics3d.access(systems::step_physics3d_access())),
        #[cfg(feature = "physics3d")]
        PONKO_SYSTEM_INTERPOLATE_PHYSICS3D => g.add(
            p,
            systems::interpolate_physics3d.access(systems::interpolate_physics3d_access()),
        ),
        PONKO_SYSTEM_STEP_NAVMESH3D => g.add(p, systems::step_navmesh3d.access(systems::step_navmesh3d_access())),
        PONKO_SYSTEM_REBUILD_SPATIAL_GRID => g.add(p, systems::rebuild_spatial_grid),
        PONKO_SYSTEM_ANIMATE_SPRITES => g.add(p, systems::animate_sprites),
        PONKO_SYSTEM_DRAW_TILEMAPS => g.add(p, systems::draw_tilemaps),
        PONKO_SYSTEM_DRAW_SPRITES => g.add(p, systems::draw_sprites),
        PONKO_SYSTEM_FOLLOW_ATTACHMENTS => g.add(p, systems::follow_attachments),
        PONKO_SYSTEM_UPDATE_ANIMATORS => g.add(p, systems::update_animators),
        PONKO_SYSTEM_UPDATE_PARTICLES => g.add(p, systems::update_particles),
        PONKO_SYSTEM_UPDATE_AUDIO3D => g.add(p, systems::update_audio3d),
        PONKO_SYSTEM_DRAW_PARTICLES => g.add(p, systems::draw_particles),
        _ => {
            // Says why: not a built-in, or one this build doesn't have.
            let why = builtin_access(id)
                .err()
                .unwrap_or_else(|| invalid("not a PONKO_SYSTEM_* built-in"));
            return (g, Err(why));
        }
    };
    (g, Ok(()))
}

/// Adds built-in system `system_id` (a `PONKO_SYSTEM_*` constant) to
/// `phase`: the C side of adding a function of Rust's `ponko::systems`
/// with `Game::add`. Take it out where the engine put it with
/// `ponko_game_remove` first, then add it where you want it, or add one of
/// the built-ins the engine never adds by itself
/// (`PONKO_SYSTEM_FOLLOW_ATTACHMENTS`, `PONKO_SYSTEM_STEP_NAVMESH3D`). The
/// physics and navigation steps come with their declarations
/// (`ponko_builtin_access`). `ponko_game_remove` with the same id takes it
/// out again. An unknown id, or a physics step the library was built
/// without, is `PONKO_INVALID_ARGUMENT`.
/// ```
/// use ponko_ffi::*;
/// unsafe {
///     let g = ponko_game_new(c"t".as_ptr(), 1, 1);
///     assert_eq!(ponko_game_remove(g, PONKO_SYSTEM_DRAW_TILEMAPS), PonkoStatus::PONKO_OK);
///     assert_eq!(ponko_game_add_builtin(g, PonkoPhase::PONKO_PHASE_DRAW as u32, PONKO_SYSTEM_DRAW_TILEMAPS), PonkoStatus::PONKO_OK);
///     ponko_game_free(g);
/// }
/// ```
/// # Safety
/// `game` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_add_builtin(game: *mut PonkoGame, phase_raw: u32, system_id: u32) -> PonkoStatus {
    let p = match phase(phase_raw) {
        Ok(p) => p,
        Err(e) => return boundary(|| Err(e)),
    };
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| add_builtin(g, p, system_id)) }
}

/// What built-in system `system_id` (a `PONKO_SYSTEM_*` constant) declares
/// it touches (Rust `systems::step_physics_access` and the others), as a
/// new declaration written to `*out_access`: free it with
/// `ponko_access_free`. Compare your own systems' declarations with it
/// (`ponko_access_conflicts`) to see whether they could run beside the
/// physics or navigation step. The built-ins without a declaration of
/// their own take the whole `ctx`: theirs is exclusive. An unknown id, or a
/// physics step the library was built without, is
/// `PONKO_INVALID_ARGUMENT` (and `*out_access` is null).
/// ```
/// use ponko_ffi::*;
/// let mut a = std::ptr::null_mut();
/// unsafe {
///     assert_eq!(ponko_builtin_access(PONKO_SYSTEM_STEP_NAVMESH3D, &mut a), PonkoStatus::PONKO_OK);
///     ponko_access_free(a);
/// }
/// ```
/// # Safety
/// `out_access` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_builtin_access(system_id: u32, out_access: *mut *mut PonkoAccess) -> PonkoStatus {
    boundary(|| {
        if out_access.is_null() {
            return Err(invalid("out_access is null"));
        }
        // SAFETY: checked non-null; caller supplies writable storage.
        unsafe { out_access.write(std::ptr::null_mut()) };
        let access = builtin_access(system_id)?;
        // SAFETY: as above.
        unsafe { out_access.write(Box::into_raw(Box::new(PonkoAccess(access)))) };
        Ok(())
    })
}

/// A new copy of `access` (a declaration of nothing for null) in
/// `*out_access`: built-in declarations name engine types, which the
/// `ponko_access_*` calls can't rebuild, so copying one takes this. Free the
/// copy with `ponko_access_free`.
/// ```
/// use ponko_ffi::*;
/// let mut copy = std::ptr::null_mut();
/// assert_eq!(unsafe { ponko_access_clone(std::ptr::null(), &mut copy) }, PonkoStatus::PONKO_OK);
/// unsafe { ponko_access_free(copy) };
/// ```
/// # Safety
/// `access` null or live; `out_access` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_access_clone(
    access: *const PonkoAccess,
    out_access: *mut *mut PonkoAccess,
) -> PonkoStatus {
    boundary(|| {
        if out_access.is_null() {
            return Err(invalid("out_access is null"));
        }
        // SAFETY: caller supplies a live handle or null.
        let copy = unsafe { access.as_ref() }.map_or_else(SystemAccess::new, |a| a.0.clone());
        // SAFETY: checked non-null above; the caller supplies a writable pointer.
        unsafe { out_access.write(Box::into_raw(Box::new(PonkoAccess(copy)))) };
        Ok(())
    })
}
