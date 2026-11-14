//! `PonkoGame`: the C side of Rust's `Game` builder. Systems are C function
//! pointers plus a `user` pointer; they get a `PonkoCtx`.
use std::ffi::{CStr, c_char, c_void};

use ponko::prelude::{Ctx, Game, IntoSystem, Phase, Plugin, SystemId};

use crate::ctx::{PonkoCtx, with_ctx};
use crate::{Failure, PonkoStatus, PonkoWorld, boundary, error, invalid, owned_world};

/// A game being set up. Run it with `ponko_game_run` (window) or
/// `ponko_game_run_headless`; both consume it. Free it only if never run.
pub struct PonkoGame {
    game: Option<Game>,
}

/// A system or setup callback. `ctx` is valid until it returns.
/// ```
/// let none: ponko_ffi::PonkoSystemFn = None;
/// assert!(none.is_none());
/// ```
pub type PonkoSystemFn = Option<unsafe extern "C" fn(ctx: *mut PonkoCtx, user: *mut c_void)>;

/// Starts a game with a window title and size (ignored when headless). Like
/// Rust's `Game::new`, it includes the built-in systems (sprite drawing,
/// animation, tilemaps); remove them with `ponko_game_remove_system`.
/// Returns null on bad arguments (see `ponko_last_error`).
/// # Safety
/// `title` must be a NUL-terminated string.
/// ```
/// let g = unsafe { ponko_ffi::ponko_game_new(c"My game".as_ptr(), 800, 600) };
/// unsafe { ponko_ffi::ponko_game_free(g); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_new(title: *const c_char, width: u32, height: u32) -> *mut PonkoGame {
    let mut result = std::ptr::null_mut();
    boundary(|| {
        // SAFETY: caller supplies a NUL-terminated string.
        let title = unsafe { name(title)? };
        result = Box::into_raw(Box::new(PonkoGame {
            game: Some(Game::new(title, width, height)),
        }));
        Ok(())
    });
    result
}

/// Frees a game that was never run. Null is ignored.
/// # Safety
/// `game` must be null or a live, never-run game, not used afterwards.
/// ```
/// unsafe { ponko_ffi::ponko_game_free(std::ptr::null_mut()); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_free(game: *mut PonkoGame) {
    if !game.is_null() {
        // SAFETY: caller passes a game from ponko_game_new, once.
        drop(unsafe { Box::from_raw(game) });
    }
}

unsafe fn name<'a>(ptr: *const c_char) -> Result<&'a str, Failure> {
    if ptr.is_null() {
        return Err(invalid("string is null"));
    }
    // SAFETY: caller supplies a NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| invalid("string is not UTF-8"))
}

/// Applies a builder step. `f` hands the game back with the result, so a
/// rejected argument leaves the game as it was (only a panic loses it).
pub(crate) unsafe fn build(game: *mut PonkoGame, f: impl FnOnce(Game) -> (Game, Result<(), Failure>)) -> PonkoStatus {
    boundary(|| {
        // SAFETY: caller supplies a live game.
        let game = unsafe { game.as_mut() }.ok_or_else(|| invalid("game is null"))?;
        let g = game.game.take().ok_or_else(|| invalid("game was already run"))?;
        let (g, result) = f(g);
        game.game = Some(g);
        result
    })
}
pub(crate) fn ok(g: Game) -> (Game, Result<(), Failure>) {
    (g, Ok(()))
}
fn reject(g: Game, message: &'static str) -> (Game, Result<(), Failure>) {
    (g, Err(invalid(message)))
}

/// Simulation ticks per second (default 60). Must be at least 1.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_fixed_hz(std::ptr::null_mut(), 30) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_fixed_hz(game: *mut PonkoGame, hz: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        build(game, |g| {
            if hz == 0 {
                reject(g, "hz must be at least 1")
            } else {
                ok(g.fixed_hz(hz))
            }
        })
    }
}
/// Seed of `ponko_ctx_random_bits` streams (runs with the same seed repeat exactly).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_seed(std::ptr::null_mut(), 7) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_seed(game: *mut PonkoGame, seed: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.seed(seed))) }
}
/// Wait for the display between frames (default on).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_vsync(std::ptr::null_mut(), false) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_vsync(game: *mut PonkoGame, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.vsync(on))) }
}
/// Samples per pixel of the 3D frame: 4 (default) or 2, which costs about a
/// third less GPU time on a large screen. GPUs that can't take 2 keep 4.
/// Sets Rust `ctx.scene3d.post.msaa` at startup, before the C setup.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_msaa(std::ptr::null_mut(), 2) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_msaa(game: *mut PonkoGame, samples: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        build(game, |g| match samples {
            2 | 4 => ok(g.plugin(Msaa(samples))),
            _ => reject(g, "msaa takes 2 or 4 samples"),
        })
    }
}
/// The startup MSAA sample count, set on the scene's post settings.
struct Msaa(u32);

impl ponko::prelude::Plugin for Msaa {
    fn build(&self, game: Game) -> Game {
        game
    }

    fn setup(&self, ctx: &mut Ctx) {
        ctx.scene3d.post.msaa = self.0;
    }
}

/// Fill the screen instead of opening a window (default off). On macOS,
/// windowed games are shown at most once per display refresh even with vsync
/// off; fullscreen ones aren't.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_fullscreen(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_fullscreen(game: *mut PonkoGame, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.fullscreen(on))) }
}
/// Turns on the built-in spatial grid with cells of `cell_size`, rebuilt
/// from every `d2::Transform` at the start of each tick (`ponko_ctx_spatial_*`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_spatial_grid(std::ptr::null_mut(), 32.0) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_spatial_grid(game: *mut PonkoGame, cell_size: f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        build(game, |g| {
            if cell_size > 0.0 && cell_size.is_finite() {
                ok(g.spatial_grid(cell_size))
            } else {
                reject(g, "cell size must be positive")
            }
        })
    }
}
/// Adds the built-in camera controls as a frame system: right or middle drag
/// pans, the wheel zooms toward the mouse (not while it is over UI).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_camera_controls(std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_camera_controls(game: *mut PonkoGame) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.system(ponko::camera::camera_controls))) }
}
/// Opens the control endpoint for tools and agents (ponko-mcp); also on
/// with `PONKO_CONTROL=1` in a debug build of this library, or a release
/// build with its `control-env` feature.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_control_endpoint(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_control_endpoint(game: *mut PonkoGame, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.control_endpoint(on))) }
}

/// Reloads textures from `assets/` when their files change, under the same
/// handles. On by default in debug builds of the engine, off in release;
/// `PONKO_HOT_RELOAD=1` or `0` overrides. See `ponko_ctx_reloaded_textures`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_hot_reload(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_hot_reload(game: *mut PonkoGame, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.hot_reload(on))) }
}

/// Turns the runtime inspector on or off: an overlay that the backquote key
/// (left of 1) opens, with every entity and its components (reflected ones
/// editable), reflected resources, system times, and pause / step / speed.
/// On by default in debug builds of the engine, off in release;
/// `PONKO_INSPECTOR=1`, `0` or `open` (on and open at start) overrides.
/// Without the `inspector` feature of ponko_ffi this does nothing.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_inspector(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_inspector(game: *mut PonkoGame, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.inspector(on))) }
}

/// Systems are named by `&'static str`, so C names are interned: each
/// distinct name is stored once for the life of the program.
pub(crate) unsafe fn system_name(ptr: *const c_char, fallback: &'static str) -> Result<&'static str, Failure> {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    if ptr.is_null() {
        return Ok(fallback);
    }
    // SAFETY: forwarded caller contract.
    let name = unsafe { name(ptr)? };
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&known) = names.iter().find(|n| **n == name) {
        return Ok(known);
    }
    let interned: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(interned);
    Ok(interned)
}

pub(crate) fn system(
    f: unsafe extern "C" fn(*mut PonkoCtx, *mut c_void),
    user: *mut c_void,
) -> impl FnMut(&mut Ctx) + 'static {
    move |ctx: &mut Ctx| {
        // SAFETY: the caller of ponko_game_*_system promised `f` and `user` stay valid while the game runs.
        with_ctx(ctx, |c| unsafe { f(c, user) })
    }
}

/// Adds a simulation system: runs on the fixed tick, every `every` ticks
/// (1 = every tick; larger = simulation LOD, like Rust's `IntoSystem::every`).
/// `name` (may be null) shows up in timings and tools.
/// # Safety
/// Live game; `system` and `user` must stay valid while the game runs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_sim_system(std::ptr::null_mut(), std::ptr::null(), 1, None, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_sim_system(
    game: *mut PonkoGame,
    name: *const c_char,
    every: u32,
    system_fn: PonkoSystemFn,
    user: *mut c_void,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        let checked = (|| {
            let f = system_fn.ok_or_else(|| invalid("system is null"))?;
            if every == 0 {
                return Err(invalid("every must be at least 1"));
            }
            Ok((f, system_name(name, "C sim system")?))
        })();
        build(game, |g| match checked {
            Ok((f, name)) => ok(g.add(Phase::AfterPhysics, system(f, user).named(name).every(every))),
            Err(e) => (g, Err(e)),
        })
    }
}

/// An execution phase for `ponko_game_add_system`, in the order they run.
/// `INPUT` through `SIM_LAST` run once per fixed tick (also headless);
/// `FRAME` through `DRAW` once per rendered frame. Systems in the same phase
/// run in the order they were added. `ponko_game_sim_system` uses
/// `AFTER_PHYSICS`, `ponko_game_system` `FRAME` and
/// `ponko_game_late_system` `LATE`; the physics step is always in `PHYSICS`.
/// ```
/// assert_eq!(ponko_ffi::PonkoPhase::PONKO_PHASE_BEFORE_PHYSICS as u32, 1);
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoPhase {
    /// Fixed-tick input preparation, before all other simulation work.
    PONKO_PHASE_INPUT,
    /// Fixed-tick gameplay that feeds this tick's physics step (forces,
    /// velocities, wheel torques, steering).
    PONKO_PHASE_BEFORE_PHYSICS,
    /// Fixed-tick physics integration.
    PONKO_PHASE_PHYSICS,
    /// Fixed-tick gameplay that sees this tick's physics result.
    PONKO_PHASE_AFTER_PHYSICS,
    /// Final fixed-tick work.
    PONKO_PHASE_SIM_LAST,
    /// Per-frame gameplay, after the due simulation ticks.
    PONKO_PHASE_FRAME,
    /// Per-frame pose work after frame gameplay.
    PONKO_PHASE_ANIMATE,
    /// Per-frame work after animation and before drawing.
    PONKO_PHASE_LATE,
    /// Per-frame drawing.
    PONKO_PHASE_DRAW,
}

pub(crate) fn phase(raw: u32) -> Result<Phase, Failure> {
    Ok(match raw {
        0 => Phase::Input,
        1 => Phase::BeforePhysics,
        2 => Phase::Physics,
        3 => Phase::AfterPhysics,
        4 => Phase::SimLast,
        5 => Phase::Frame,
        6 => Phase::Animate,
        7 => Phase::Late,
        8 => Phase::Draw,
        _ => return Err(invalid("unknown phase")),
    })
}

/// Adds a system in an explicit phase (a `PonkoPhase`), like Rust's
/// `Game::add`. Fixed-tick phases run it every `every` ticks (1 = every
/// tick); frame phases need `every` = 1. Use `PONKO_PHASE_BEFORE_PHYSICS`
/// for logic the current tick's physics step must see. `name` (may be null)
/// shows up in timings and tools. An unknown phase, `every` = 0 or a frame
/// phase with `every` > 1 is `PONKO_INVALID_ARGUMENT`.
/// # Safety
/// Live game; `system` and `user` must stay valid while the game runs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_add_system(std::ptr::null_mut(), 1, std::ptr::null(), 1, None, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_add_system(
    game: *mut PonkoGame,
    phase: u32,
    name: *const c_char,
    every: u32,
    system_fn: PonkoSystemFn,
    user: *mut c_void,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        let checked = (|| {
            let f = system_fn.ok_or_else(|| invalid("system is null"))?;
            let phase = self::phase(phase)?;
            if every == 0 {
                return Err(invalid("every must be at least 1"));
            }
            if every > 1 && phase > Phase::SimLast {
                return Err(invalid("only fixed-tick phases take an interval"));
            }
            Ok((f, phase, system_name(name, "C system")?))
        })();
        build(game, |g| match checked {
            Ok((f, phase, name)) => ok(g.add(phase, system(f, user).named(name).every(every))),
            Err(e) => (g, Err(e)),
        })
    }
}

/// Like `ponko_game_add_system`, and writes the new system's id to
/// `*out_id`, for `ponko_game_remove` (Rust's `Game::add_system`). Ids are
/// never 0 and never reused. On failure `*out_id` is 0.
/// # Safety
/// Live game; writable `out_id`; `system` and `user` must stay valid while
/// the game runs.
/// ```
/// let mut id = 7;
/// assert_eq!(unsafe { ponko_ffi::ponko_game_add_system_id(std::ptr::null_mut(), 1, std::ptr::null(), 1, None, std::ptr::null_mut(), &mut id) } as u32, 1);
/// assert_eq!(id, 0);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_add_system_id(
    game: *mut PonkoGame,
    phase: u32,
    name: *const c_char,
    every: u32,
    system_fn: PonkoSystemFn,
    user: *mut c_void,
    out_id: *mut u32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        if out_id.is_null() {
            return boundary(|| Err(invalid("out_id is null")));
        }
        out_id.write(0);
        let checked = (|| {
            let f = system_fn.ok_or_else(|| invalid("system is null"))?;
            let phase = self::phase(phase)?;
            if every == 0 {
                return Err(invalid("every must be at least 1"));
            }
            if every > 1 && phase > Phase::SimLast {
                return Err(invalid("only fixed-tick phases take an interval"));
            }
            Ok((f, phase, system_name(name, "C system")?))
        })();
        build(game, |mut g| match checked {
            Ok((f, phase, name)) => {
                let id = g.add_system(phase, system(f, user).named(name).every(every));
                out_id.write(id.to_bits());
                ok(g)
            }
            Err(e) => (g, Err(e)),
        })
    }
}

/// Fixed-tick 2D physics step (`ponko_game_physics`), for `ponko_game_remove`.
pub const PONKO_SYSTEM_PHYSICS: u32 = 1;
/// Fixed-tick 3D physics step (`ponko_game_physics3d`).
pub const PONKO_SYSTEM_PHYSICS3D: u32 = 2;
/// Per-frame blending of 3D physics transforms between steps.
pub const PONKO_SYSTEM_INTERPOLATE_PHYSICS3D: u32 = 3;
/// Spatial grid rebuild (`ponko_game_spatial_grid`).
pub const PONKO_SYSTEM_REBUILD_SPATIAL_GRID: u32 = 4;
/// Sprite animation.
pub const PONKO_SYSTEM_ANIMATE_SPRITES: u32 = 5;
/// Tilemap drawing.
pub const PONKO_SYSTEM_DRAW_TILEMAPS: u32 = 6;
/// Sprite drawing.
pub const PONKO_SYSTEM_DRAW_SPRITES: u32 = 7;
/// Attachment transforms (`ponko_game_follow_attachments`).
pub const PONKO_SYSTEM_FOLLOW_ATTACHMENTS: u32 = 8;
/// Navigation agents (`ponko_game_navmesh3d`).
pub const PONKO_SYSTEM_STEP_NAVMESH3D: u32 = 9;
/// Animators posing skeletons, at the end of `PONKO_PHASE_ANIMATE`.
pub const PONKO_SYSTEM_UPDATE_ANIMATORS: u32 = 10;
/// Particle emitters and trails, at the end of `PONKO_PHASE_ANIMATE`.
pub const PONKO_SYSTEM_UPDATE_PARTICLES: u32 = 11;
/// Positional audio, at the end of `PONKO_PHASE_DRAW` (after the drawing systems).
pub const PONKO_SYSTEM_UPDATE_AUDIO3D: u32 = 12;
/// 2D particle drawing, after the sprites.
pub const PONKO_SYSTEM_DRAW_PARTICLES: u32 = 13;

/// Removes systems by id (Rust's `Game::remove`): an id from
/// `ponko_game_add_system_id`, or a `PONKO_SYSTEM_*` constant for a
/// built-in, which removes every instance of it. An id that names nothing
/// (already removed, or a built-in that isn't installed) is not an error.
/// # Safety
/// `game` must be null or a live game.
/// ```
/// use ponko_ffi::*;
/// unsafe {
///     let g = ponko_game_new(c"t".as_ptr(), 1, 1);
///     assert_eq!(ponko_game_remove(g, PONKO_SYSTEM_DRAW_SPRITES), PonkoStatus::PONKO_OK);
///     assert_eq!(ponko_game_remove(g, PONKO_SYSTEM_DRAW_SPRITES), PonkoStatus::PONKO_OK);
///     ponko_game_free(g);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_remove(game: *mut PonkoGame, system_id: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| ok(g.remove(SystemId::from_bits(system_id)))) }
}

/// Adds a startup callback (Rust's `Game::setup`): it runs once, after the
/// engine started and before any system, also with zero ticks. Callbacks run
/// in the order they were added, together with plugin setups, and before the
/// `setup` passed to `ponko_game_run` and friends; commands are applied
/// after each one.
/// # Safety
/// Live game; `setup` and `user` must stay valid until the game ran them.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_setup(std::ptr::null_mut(), None, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_setup(
    game: *mut PonkoGame,
    setup: PonkoSystemFn,
    user: *mut c_void,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        build(game, |g| match setup {
            Some(_) => ok(g.plugin(Startup { setup, user })),
            None => reject(g, "setup is null"),
        })
    }
}

/// The build step of a plugin: adds systems and settings to `game`.
/// ```
/// let none: ponko_ffi::PonkoPluginBuildFn = None;
/// assert!(none.is_none());
/// ```
pub type PonkoPluginBuildFn = Option<unsafe extern "C" fn(game: *mut PonkoGame, user: *mut c_void)>;

/// Adds a plugin (Rust's `Plugin`): one module of the game, as a `build`
/// callback and a `setup` callback (either may be null, not both). `build`
/// runs now, with `game`, to add the module's systems and settings; `setup`
/// is queued like `ponko_game_setup`, so plugins start in the order they
/// were added. Each system names its phase, so the order plugins are added
/// in only matters between systems of the same phase.
/// # Safety
/// Live game; `setup` and `user` must stay valid until the game ran them.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_plugin(std::ptr::null_mut(), None, None, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_plugin(
    game: *mut PonkoGame,
    build_fn: PonkoPluginBuildFn,
    setup: PonkoSystemFn,
    user: *mut c_void,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        // Check before `build_fn` runs: it must not see a null or spent game.
        let status = build(game, |g| {
            if build_fn.is_none() && setup.is_none() {
                reject(g, "a plugin needs a build or a setup callback")
            } else {
                ok(g)
            }
        });
        if status != PonkoStatus::PONKO_OK {
            return status;
        }
        if let Some(f) = build_fn {
            // The game is back in its box: `f` adds to it through the C API.
            let status = boundary(|| {
                f(game, user);
                Ok(())
            });
            if status != PonkoStatus::PONKO_OK {
                return status;
            }
        }
        match setup {
            Some(_) => build(game, |g| ok(g.plugin(Startup { setup, user }))),
            None => PonkoStatus::PONKO_OK,
        }
    }
}

/// Adds a late system: runs once per frame after the engine posed animated
/// skeletons and before drawing, for IK, look-at and other pose fixes. Not
/// run by `ponko_game_run_headless`.
/// # Safety
/// Live game; `system` and `user` must stay valid while the game runs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_late_system(std::ptr::null_mut(), std::ptr::null(), None, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_late_system(
    game: *mut PonkoGame,
    name: *const c_char,
    system_fn: PonkoSystemFn,
    user: *mut c_void,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        let checked = (|| {
            Ok((
                system_fn.ok_or_else(|| invalid("system is null"))?,
                system_name(name, "C late system")?,
            ))
        })();
        build(game, |g| match checked {
            Ok((f, name)) => ok(g.add(Phase::Late, system(f, user).named(name))),
            Err(e) => (g, Err(e)),
        })
    }
}

/// Adds a frame system: runs once per frame before drawing (input, camera, UI).
/// Not run by `ponko_game_run_headless`.
/// # Safety
/// Live game; `system` and `user` must stay valid while the game runs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_system(std::ptr::null_mut(), std::ptr::null(), None, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_system(
    game: *mut PonkoGame,
    name: *const c_char,
    system_fn: PonkoSystemFn,
    user: *mut c_void,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        let checked = (|| {
            Ok((
                system_fn.ok_or_else(|| invalid("system is null"))?,
                system_name(name, "C system")?,
            ))
        })();
        build(game, |g| match checked {
            Ok((f, name)) => ok(g.add(Phase::Frame, system(f, user).named(name))),
            Err(e) => (g, Err(e)),
        })
    }
}

/// Removes systems by name: your own, or a built-in such as
/// `"ponko::ecs::draw_sprites"`. `PONKO_NOT_FOUND` if none matched.
/// Kept for existing code; new code removes by id with `ponko_game_remove`
/// (`PONKO_SYSTEM_DRAW_SPRITES`), which also tells apart two systems that
/// share a name.
/// # Safety
/// Live game; NUL-terminated name.
/// ```
/// unsafe {
///     let g = ponko_ffi::ponko_game_new(c"t".as_ptr(), 1, 1);
///     assert_eq!(ponko_ffi::ponko_game_remove_system(g, c"ponko::ecs::draw_sprites".as_ptr()), ponko_ffi::PonkoStatus::PONKO_OK);
///     ponko_ffi::ponko_game_free(g);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_remove_system(game: *mut PonkoGame, name_ptr: *const c_char) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        let checked = name(name_ptr);
        // Name removal stays in ABI version 2; Rust removes by id only, so
        // the names are looked up here.
        build(game, |g| match checked {
            Ok(name) => {
                let Some(first) = ponko::__ffi::system_named(&g, name) else {
                    return (g, Err(error(PonkoStatus::PONKO_NOT_FOUND, "no system with that name")));
                };
                let mut g = g.remove(first);
                while let Some(id) = ponko::__ffi::system_named(&g, name) {
                    g = g.remove(id);
                }
                ok(g)
            }
            Err(e) => (g, Err(e)),
        })
    }
}

unsafe fn take(game: *mut PonkoGame) -> Result<Game, Failure> {
    if game.is_null() {
        return Err(invalid("game is null"));
    }
    // SAFETY: caller passes a game from ponko_game_new; running consumes it.
    let game = unsafe { Box::from_raw(game) };
    game.game.ok_or_else(|| invalid("game was already run"))
}

/// A startup callback on a game that still takes systems: a plugin with no
/// build step. `Game::plugin` puts its setup in the same ordered queue as
/// `Game::setup` and the Rust plugins.
struct Startup {
    setup: crate::PonkoSystemFn,
    user: *mut c_void,
}

impl Plugin for Startup {
    fn build(&self, game: Game) -> Game {
        game
    }

    fn setup(&self, ctx: &mut Ctx) {
        setup_fn(self.setup, self.user)(ctx);
    }
}

fn setup_fn(setup: crate::PonkoSystemFn, user: *mut c_void) -> impl FnOnce(&mut Ctx) {
    move |ctx: &mut Ctx| {
        if let Some(f) = setup {
            // SAFETY: the caller promised `f` and `user` are valid for the run.
            with_ctx(ctx, |c| unsafe { f(c, user) });
        }
    }
}

/// Like `ponko_game_run_headless`, but times it: the first `warmup` ticks
/// aren't measured, the next `ticks` are. Read the performance report (JSON)
/// afterwards with `ponko_last_perf_report`, and format and check it with
/// `ponko_perf_report_text`, `ponko_perf_check` and `ponko_perf_compare`.
/// Rust's allocations are counted; C++'s need `PONKO_COUNT_CPP_ALLOCATIONS`
/// (or `ponko_note_allocation`).
/// # Safety
/// As `ponko_game_run_headless`.
/// ```
/// use ponko_ffi::*;
/// unsafe {
///     let g = ponko_game_new(c"sim".as_ptr(), 1, 1);
///     let mut world = std::ptr::null_mut();
///     let s = ponko_game_profile_headless(g, 2, 10, None, std::ptr::null_mut(), &mut world);
///     assert_eq!(s, PonkoStatus::PONKO_OK);
///     let mut len = 0;
///     ponko_last_perf_report(std::ptr::null_mut(), 0, &mut len);
///     assert!(len > 10);
///     ponko_world_free(world);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_profile_headless(
    game: *mut PonkoGame,
    warmup: u64,
    ticks: u64,
    setup: PonkoSystemFn,
    user: *mut c_void,
    out_world: *mut *mut PonkoWorld,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let game = unsafe { take(game)? };
        if out_world.is_null() {
            return Err(invalid("out_world is null"));
        }
        // SAFETY: checked non-null; caller supplies writable storage.
        unsafe { out_world.write(std::ptr::null_mut()) };
        let (world, report) = game.profile_headless(warmup, ticks, setup_fn(setup, user));
        crate::perf::keep_report(report.to_json());
        // SAFETY: as above.
        unsafe { out_world.write(owned_world(world)) };
        Ok(())
    })
}

/// Opens the window and runs the game until it quits, then frees `game`.
/// `setup` (may be null) runs once first: load textures, spawn entities.
/// On macOS it must be called from the main thread.
/// # Safety
/// Live game (consumed even on failure); callbacks and `user` valid until it returns.
/// ```no_run
/// unsafe {
///     let g = ponko_ffi::ponko_game_new(c"My game".as_ptr(), 800, 600);
///     ponko_ffi::ponko_game_run(g, None, std::ptr::null_mut());
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_run(game: *mut PonkoGame, setup: PonkoSystemFn, user: *mut c_void) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let game = unsafe { take(game)? };
        game.run(setup_fn(setup, user));
        Ok(())
    })
}

/// Runs up to `ticks` simulation ticks with no window, GPU or audio, as fast
/// as possible, then frees `game` and gives you the world in `*out_world`
/// (free it with `ponko_world_free`). `setup` runs once first; frame systems
/// don't run. A system can stop early with `ponko_ctx_quit`.
/// # Safety
/// Live game (consumed even on failure); writable `out_world`; callbacks and
/// `user` valid until it returns.
/// ```
/// use ponko_ffi::*;
/// unsafe {
///     let g = ponko_game_new(c"sim".as_ptr(), 1, 1);
///     let mut world = std::ptr::null_mut();
///     assert_eq!(ponko_game_run_headless(g, 10, None, std::ptr::null_mut(), &mut world), PonkoStatus::PONKO_OK);
///     ponko_world_free(world);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_run_headless(
    game: *mut PonkoGame,
    ticks: u64,
    setup: PonkoSystemFn,
    user: *mut c_void,
    out_world: *mut *mut PonkoWorld,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let game = unsafe { take(game)? };
        if out_world.is_null() {
            return Err(invalid("out_world is null"));
        }
        // SAFETY: checked non-null; caller supplies writable storage.
        unsafe { out_world.write(std::ptr::null_mut()) };
        let world = game.run_headless(ticks, setup_fn(setup, user));
        // SAFETY: as above.
        unsafe { out_world.write(owned_world(world)) };
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use crate::{PonkoStatus, ponko_game_free, ponko_game_new, ponko_game_parallel_systems, ponko_game_schedule_plan};
    use ponko::prelude::Phase;
    use std::ffi::CStr;

    #[test]
    fn a_plan_that_cannot_be_made_leaves_the_game_in_its_handle() {
        unsafe {
            let handle = ponko_game_new(c"plan".as_ptr(), 1, 1);
            // What a Rust plugin sharing the game can do: an order against
            // the order the systems run in.
            let mut game = (*handle).game.take().unwrap();
            let first = game.add_system(Phase::AfterPhysics, |_: &mut ponko::prelude::Ctx| {});
            let second = game.add_system(Phase::AfterPhysics, |_: &mut ponko::prelude::Ctx| {});
            (*handle).game = Some(game.order(second, first));

            let mut len = 0;
            let status = ponko_game_schedule_plan(handle, std::ptr::null_mut(), 0, &mut len);
            assert_eq!(status, PonkoStatus::PONKO_INVALID_ARGUMENT);
            let message = CStr::from_ptr(crate::ponko_last_error()).to_str().unwrap();
            assert!(message.contains("Game::order"), "{message}");
            // The handle still owns its game: later calls work, and say the
            // same thing instead of "game was already run".
            assert!((*handle).game.is_some());
            assert_eq!(ponko_game_parallel_systems(handle, true), PonkoStatus::PONKO_OK);
            let status = ponko_game_schedule_plan(handle, std::ptr::null_mut(), 0, &mut len);
            assert_eq!(status, PonkoStatus::PONKO_INVALID_ARGUMENT);
            ponko_game_free(handle);
        }
    }
}
