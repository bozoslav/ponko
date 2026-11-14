//! `PonkoCtx`: what a C system gets, the C side of Rust's `Ctx`.
//!
//! One busy flag (the lent world's) guards the world and every context call,
//! so nothing can reach the `Ctx` while a query borrows its world. Commands
//! and the spatial grid are separate fields of `Ctx`, reached through raw
//! field pointers, so they stay usable inside query callbacks. Events too,
//! behind their own flag.
use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

use ponko::input::Gamepad;
use ponko::input::GamepadButton;
use ponko::prelude::Ctx;
use ponko::spatial::SpatialGrid;
use ponko_ecs::{Commands, Entity};

use crate::keys::{button_from_c, gamepad_axis_from_c, gamepad_button_from_c, key_from_c};
use crate::types::*;
use crate::{Failure, Kind, PonkoStatus, PonkoValue, PonkoWorld, boundary, error, insert_value, invalid, spawn_values};

/// What a system gets: the world, the clock, input, drawing and commands.
/// Valid only during the callback it was passed to.
pub struct PonkoCtx {
    /// The context's world, lent for this callback. Its busy flag guards `ctx` too.
    pub(crate) world: PonkoWorld,
    ctx: *mut Ctx,
    commands_busy: AtomicBool,
    events_busy: AtomicBool,
    dt: f32,
    sim_tick: u64,
    sim_time: f64,
    time: f64,
    frame: u64,
    fps: f32,
    random_bits: u64,
    window: PonkoVec2,
}

/// Lends `ctx` to C for one callback.
pub(crate) fn with_ctx<R>(ctx: &mut Ctx, f: impl FnOnce(*mut PonkoCtx) -> R) -> R {
    let snapshot = (
        ctx.time.dt(),
        ctx.time.sim_tick(),
        ctx.time.sim_time(),
        ctx.time.elapsed(),
        ctx.time.frame(),
        ctx.time.fps(),
        ctx.random().to_bits(),
        to_c(ctx.window_size()),
    );
    let raw: *mut Ctx = ctx;
    // SAFETY: `raw` comes from a live `&mut Ctx`; from here on it is only used
    // through raw pointers, and the busy flags keep accesses from overlapping.
    let world = unsafe { &raw mut (*raw).world };
    let mut handle = PonkoCtx {
        world: PonkoWorld::new(world, false),
        ctx: raw,
        commands_busy: AtomicBool::new(false),
        events_busy: AtomicBool::new(false),
        dt: snapshot.0,
        sim_tick: snapshot.1,
        sim_time: snapshot.2,
        time: snapshot.3,
        frame: snapshot.4,
        fps: snapshot.5,
        random_bits: snapshot.6,
        window: snapshot.7,
    };
    f(&mut handle)
}

pub(crate) unsafe fn ctx_access<R>(
    handle: *const PonkoCtx,
    f: impl FnOnce(&mut Ctx) -> Result<R, Failure>,
) -> Result<R, Failure> {
    if handle.is_null() {
        return Err(invalid("ctx is null"));
    }
    // SAFETY: caller supplies a live ctx from a running callback.
    let handle = unsafe { &*handle };
    let Some(_guard) = handle.world.lock() else {
        return Err(error(PonkoStatus::PONKO_BUSY, "ctx is borrowed (a query is running?)"));
    };
    // SAFETY: the busy flag grants exclusive access to the whole Ctx.
    f(unsafe { &mut *handle.ctx })
}

unsafe fn commands<R>(
    handle: *const PonkoCtx,
    f: impl FnOnce(&mut Commands) -> Result<R, Failure>,
) -> Result<R, Failure> {
    if handle.is_null() {
        return Err(invalid("ctx is null"));
    }
    // SAFETY: caller supplies a live ctx from a running callback.
    let handle = unsafe { &*handle };
    if handle.world.parallel.load(Ordering::Acquire) {
        return Err(error(
            PonkoStatus::PONKO_BUSY,
            "commands are off during ponko_query_par",
        ));
    }
    if handle.commands_busy.swap(true, Ordering::Acquire) {
        return Err(error(PonkoStatus::PONKO_BUSY, "commands are in use"));
    }
    struct Reset<'a>(&'a AtomicBool);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _reset = Reset(&handle.commands_busy);
    // SAFETY: `commands` is a field no other ABI call touches while this flag
    // is set; ctx-wide calls never run while a C callback (our caller) runs
    // inside them, because they don't call back into C.
    f(unsafe { &mut (*handle.ctx).commands })
}

/// Events, usable inside `ponko_query` callbacks like commands.
pub(crate) unsafe fn events<R>(
    handle: *const PonkoCtx,
    f: impl FnOnce(&mut ponko::prelude::Events) -> Result<R, Failure>,
) -> Result<R, Failure> {
    if handle.is_null() {
        return Err(invalid("ctx is null"));
    }
    // SAFETY: caller supplies a live ctx from a running callback.
    let handle = unsafe { &*handle };
    if handle.world.parallel.load(Ordering::Acquire) {
        return Err(error(PonkoStatus::PONKO_BUSY, "events are off during ponko_query_par"));
    }
    if handle.events_busy.swap(true, Ordering::Acquire) {
        return Err(error(PonkoStatus::PONKO_BUSY, "events are in use"));
    }
    struct Reset<'a>(&'a AtomicBool);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _reset = Reset(&handle.events_busy);
    // SAFETY: `events` is a field only these calls touch, one at a time
    // (the flag), and ctx-wide calls never run while a C callback runs.
    f(unsafe { &mut (*handle.ctx).events })
}

/// The built-in grid, read-only. Usable from any thread during a callback.
unsafe fn spatial<'a>(handle: *const PonkoCtx) -> Result<&'a SpatialGrid, Failure> {
    if handle.is_null() {
        return Err(invalid("ctx is null"));
    }
    // SAFETY: the engine only rebuilds the grid between systems; no ABI call writes it.
    Ok(unsafe { &(*(*handle).ctx).spatial })
}

pub(crate) unsafe fn out<T>(ptr: *mut T, value: T) -> Result<(), Failure> {
    if ptr.is_null() {
        return Err(invalid("output is null"));
    }
    // SAFETY: caller supplies writable output (checked non-null).
    unsafe { ptr.write(value) };
    Ok(())
}

pub(crate) unsafe fn text<'a>(ptr: *const c_char) -> Result<&'a str, Failure> {
    if ptr.is_null() {
        return Err(invalid("string is null"));
    }
    // SAFETY: caller supplies a NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| invalid("string is not UTF-8"))
}

/// Seconds this system covers: the fixed step in sim systems, the frame time otherwise.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_dt(std::ptr::null()) }, 0.0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_dt(ctx: *const PonkoCtx) -> f32 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).dt }
}
/// Simulation ticks completed so far.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_sim_tick(std::ptr::null()) }, 0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_sim_tick(ctx: *const PonkoCtx) -> u64 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).sim_tick }
}
/// Simulated seconds so far (exact: ticks × fixed step).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_sim_time(std::ptr::null()) }, 0.0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_sim_time(ctx: *const PonkoCtx) -> f64 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).sim_time }
}
/// Wall-clock seconds since the game started.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_time(std::ptr::null()) }, 0.0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_time(ctx: *const PonkoCtx) -> f64 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).time }
}
/// Frames drawn so far.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_frame(std::ptr::null()) }, 0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_frame(ctx: *const PonkoCtx) -> u64 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).frame }
}
/// Frames per second, averaged.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_fps(std::ptr::null()) }, 0.0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_fps(ctx: *const PonkoCtx) -> f32 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).fps }
}
/// Base of this tick and system's random streams (Rust `ctx.random().to_bits()`).
/// The C++ header's `ponko::rng` derives the same numbers as Rust from it.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_random_bits(std::ptr::null()) }, 0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_random_bits(ctx: *const PonkoCtx) -> u64 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).random_bits }
}
/// Window size in pixels.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_window_size(std::ptr::null()) }.x, 0.0);
/// ```
/// Returns 0 for a null ctx. Safe to call from any thread during the callback.
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_window_size(ctx: *const PonkoCtx) -> PonkoVec2 {
    if ctx.is_null() {
        return Default::default();
    }
    // SAFETY: live ctx; snapshot fields are never written after creation.
    unsafe { (*ctx).window }
}

/// The world of this context. Valid until the callback returns; never free it.
/// # Safety
/// `ctx` must be null or live.
/// ```
/// assert!(unsafe { ponko_ffi::ponko_ctx_world(std::ptr::null_mut()) }.is_null());
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_world(ctx: *mut PonkoCtx) -> *mut PonkoWorld {
    if ctx.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: live ctx.
    unsafe { &raw mut (*ctx).world }
}

/// Ends the game after this frame (or the headless run after this tick).
/// ```
/// use ponko_ffi::*;
/// assert_eq!(unsafe { ponko_ctx_quit(std::ptr::null_mut()) }, PonkoStatus::PONKO_INVALID_ARGUMENT);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_quit(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.quit();
            Ok(())
        })
    })
}
/// Pauses the simulation (frame systems keep running).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_pause(std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_pause(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.time.pause();
            Ok(())
        })
    })
}
/// Resumes a paused simulation.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_resume(std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_resume(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.time.resume();
            Ok(())
        })
    })
}
/// Runs one simulation tick while paused.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_step(std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_step(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.time.step();
            Ok(())
        })
    })
}
/// Simulation speed multiplier (1..64); results don't depend on it.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_sim_speed(std::ptr::null_mut(), 2.0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_sim_speed(ctx: *mut PonkoCtx, speed: f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if !speed.is_finite() || speed < 0.0 {
                return Err(invalid("speed must be finite and not negative"));
            }
            c.time.set_speed(speed);
            Ok(())
        })
    })
}
/// Current simulation speed.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_sim_speed(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_sim_speed(ctx: *mut PonkoCtx, out_speed: *mut f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_speed, c.time.speed())) })
}
/// Whether the simulation is paused.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_is_paused(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_is_paused(ctx: *mut PonkoCtx, out_paused: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_paused, c.time.is_paused())) })
}

/// Whether a key (`PonkoKey`) is held.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_key_down(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_key_down(ctx: *mut PonkoCtx, input: u32, out_state: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let input = key_from_c(input).ok_or_else(|| invalid("unknown key or mouse button"))?;
            out(out_state, c.input.key_down(input))
        })
    })
}
/// Whether a key went down this frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_key_pressed(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_key_pressed(ctx: *mut PonkoCtx, input: u32, out_state: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let input = key_from_c(input).ok_or_else(|| invalid("unknown key or mouse button"))?;
            out(out_state, c.input.key_pressed(input))
        })
    })
}
/// Whether a key went up this frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_key_released(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_key_released(ctx: *mut PonkoCtx, input: u32, out_state: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let input = key_from_c(input).ok_or_else(|| invalid("unknown key or mouse button"))?;
            out(out_state, c.input.key_released(input))
        })
    })
}
/// Whether a mouse button (`PonkoMouseButton`) is held.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_down(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_down(ctx: *mut PonkoCtx, input: u32, out_state: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let input = button_from_c(input).ok_or_else(|| invalid("unknown key or mouse button"))?;
            out(out_state, c.input.mouse_down(input))
        })
    })
}
/// Whether a mouse button went down this frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_pressed(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_pressed(ctx: *mut PonkoCtx, input: u32, out_state: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let input = button_from_c(input).ok_or_else(|| invalid("unknown key or mouse button"))?;
            out(out_state, c.input.mouse_pressed(input))
        })
    })
}
/// Whether a mouse button went up this frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_released(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_released(ctx: *mut PonkoCtx, input: u32, out_state: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let input = button_from_c(input).ok_or_else(|| invalid("unknown key or mouse button"))?;
            out(out_state, c.input.mouse_released(input))
        })
    })
}
/// How many gamepads are connected; read each with `ponko_ctx_gamepad_at`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_gamepad_count(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_gamepad_count(ctx: *mut PonkoCtx, out_count: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_count, c.input.gamepads().len() as u32)) })
}
/// The slot of the `index`th connected gamepad (`0..count`), by slot order:
/// the number every other gamepad call takes as `pad`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_gamepad_at(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_pad` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_gamepad_at(ctx: *mut PonkoCtx, index: u32, out_pad: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let pad = c
                .input
                .gamepads()
                .get(index as usize)
                .ok_or_else(|| invalid("no gamepad at that index"))?;
            out(out_pad, pad.slot() as u32)
        })
    })
}

fn gamepad_args(pad: u32, button: u32) -> Result<(Gamepad, GamepadButton), Failure> {
    let pad = Gamepad::from_slot(pad as usize).ok_or_else(|| invalid("gamepad slot out of range"))?;
    let button = gamepad_button_from_c(button).ok_or_else(|| invalid("unknown gamepad button"))?;
    Ok((pad, button))
}

/// Whether `button` (`PonkoGamepadButton`) is held on gamepad `pad` (a slot).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_gamepad_down(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_gamepad_down(
    ctx: *mut PonkoCtx,
    pad: u32,
    button: u32,
    out_state: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let (pad, button) = gamepad_args(pad, button)?;
            out(out_state, c.input.gamepad_down(pad, button))
        })
    })
}
/// Whether `button` went down on `pad` this frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_gamepad_pressed(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_gamepad_pressed(
    ctx: *mut PonkoCtx,
    pad: u32,
    button: u32,
    out_state: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let (pad, button) = gamepad_args(pad, button)?;
            out(out_state, c.input.gamepad_pressed(pad, button))
        })
    })
}
/// Whether `button` went up on `pad` this frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_gamepad_released(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_state` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_gamepad_released(
    ctx: *mut PonkoCtx,
    pad: u32,
    button: u32,
    out_state: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let (pad, button) = gamepad_args(pad, button)?;
            out(out_state, c.input.gamepad_released(pad, button))
        })
    })
}
/// One axis (`PonkoGamepadAxis`) of gamepad `pad`: sticks `-1..1` (y down),
/// triggers `0..1`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_gamepad_axis(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_value` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_gamepad_axis(
    ctx: *mut PonkoCtx,
    pad: u32,
    axis: u32,
    out_value: *mut f32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let pad = Gamepad::from_slot(pad as usize).ok_or_else(|| invalid("gamepad slot out of range"))?;
            let axis = gamepad_axis_from_c(axis).ok_or_else(|| invalid("unknown gamepad axis"))?;
            out(out_value, c.input.gamepad_axis(pad, axis))
        })
    })
}
/// Mouse position in window pixels.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_pos(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_pos(ctx: *mut PonkoCtx, out_pos: *mut PonkoVec2) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_pos, to_c(c.input.mouse_pos()))) })
}
/// Mouse position in world units (through the camera).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_world(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_world(ctx: *mut PonkoCtx, out_pos: *mut PonkoVec2) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_pos, to_c(c.mouse_world()))) })
}
/// Scroll this frame (lines; positive y = away from the user).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_wheel(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_wheel(ctx: *mut PonkoCtx, out_wheel: *mut PonkoVec2) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_wheel, to_c(c.input.mouse_wheel()))) })
}
/// How far the mouse moved this frame, in raw device units (+y = down),
/// even while the cursor is locked: for mouse look.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_motion(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_motion(ctx: *mut PonkoCtx, out_motion: *mut PonkoVec2) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_motion, to_c(c.input.mouse_motion()))) })
}
/// Locks (hides and holds) the cursor in the window, or lets it go. Takes
/// effect after this frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_cursor_locked(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_cursor_locked(ctx: *mut PonkoCtx, locked: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.input.set_cursor_locked(locked);
            Ok(())
        })
    })
}
/// Whether the game asked for the cursor to be locked.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_cursor_locked(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_cursor_locked(ctx: *mut PonkoCtx, out_locked: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_locked, c.input.cursor_locked())) })
}

/// Loads a PNG/JPEG texture (cached by path). Headless runs return a
/// size-only handle. Errors (missing file) come back as a failed status.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_texture(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_texture(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_texture: *mut PonkoTexture,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_texture.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_texture, texture_to_c(c.gfx.load_texture(path)))
        })
    })
}
/// Loads a JSON file from `assets/` into reflected component `component` of
/// `entity` (object fields merge; reflect it with `ponko_component_reflect`
/// first). With hot reload on, editing the file updates the component while
/// the game runs; a file that doesn't parse keeps the old values. C's way to
/// keep tunable game data (speeds, costs, waves) in a file.
/// # Safety
/// `ctx` must be null or live; `path` a NUL-terminated string.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_json(std::ptr::null_mut(), std::ptr::null(), 0, 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_json(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    entity: u64,
    component: u32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            let id = c
                .world
                .component_at(component as usize)
                .ok_or_else(|| invalid("component id is not registered in this world"))?;
            // C components are reflected under their full name ("game::Rules"),
            // Rust ones under their short name.
            let full = c.world.component_info(id).name();
            let name = if ponko::prelude::WorldReflect::reflected_components(&c.world).contains(&full) {
                full.to_string()
            } else {
                full.rsplit("::").next().unwrap_or(full).to_string()
            };
            c.assets
                .load_component_json_named(&mut c.world, path, Entity::from_bits(entity), &name)
                .map_err(crate::invalid_owned)
        })
    })
}
/// Loads a JSON file from `assets/` into the resource called `name`, which
/// `ponko_resource_reflect` described (object fields merge; it's added if
/// missing). With hot reload on, editing the file updates the resource while
/// the game runs; a file that doesn't parse keeps the old values.
/// # Safety
/// `ctx` must be null or live; `path` and `name` NUL-terminated strings.
/// ```
/// let status = unsafe { ponko_ffi::ponko_ctx_load_resource_json(std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) };
/// assert_eq!(status, ponko_ffi::PonkoStatus::PONKO_INVALID_ARGUMENT);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_resource_json(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    name: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            let name = text(name)?;
            c.assets
                .load_resource_json_named(&mut c.world, path, name)
                .map_err(crate::invalid_owned)
        })
    })
}
/// Whether the file `path` (as it was loaded) changed and was reloaded at the
/// start of this frame: a texture or a `ponko_ctx_load_json` file. Use it
/// to redo work that depends on the file.
/// # Safety
/// `ctx` must be null or live; `path` a NUL-terminated string; `out` writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_was_reloaded(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_was_reloaded(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_reloaded: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            out(out_reloaded, c.assets.reloaded_files().iter().any(|f| f == path))
        })
    })
}
/// Makes a texture from `width * height * 4` RGBA bytes.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_texture(std::ptr::null_mut(), 1, 1, std::ptr::null(), 4, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_texture(
    ctx: *mut PonkoCtx,
    width: u32,
    height: u32,
    rgba: *const u8,
    len: usize,
    out_texture: *mut PonkoTexture,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let pixels = pixels(width, height, rgba, len)?;
            if out_texture.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_texture, texture_to_c(c.gfx.create_texture(width, height, pixels)))
        })
    })
}
/// Replaces a texture's pixels (same size, `width * height * 4` bytes).
/// ```
/// use ponko_ffi::*;
/// let t = PonkoTexture { id: 0, width: 1, height: 1 };
/// assert_eq!(unsafe { ponko_ctx_update_texture(std::ptr::null_mut(), t, std::ptr::null(), 4) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_update_texture(
    ctx: *mut PonkoCtx,
    texture: PonkoTexture,
    rgba: *const u8,
    len: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let pixels = pixels(texture.width, texture.height, rgba, len)?;
            c.gfx.update_texture(texture_from_c(texture), pixels);
            Ok(())
        })
    })
}

fn pixels<'a>(width: u32, height: u32, rgba: *const u8, len: usize) -> Result<&'a [u8], Failure> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if rgba.is_null() || width == 0 || height == 0 || expected != Some(len) {
        return Err(invalid("pixels must be width * height * 4 RGBA bytes"));
    }
    // SAFETY: caller supplies `len` readable bytes.
    Ok(unsafe { std::slice::from_raw_parts(rgba, len) })
}

/// Background color for this frame.
/// ```
/// use ponko_ffi::*;
/// let black = PonkoColor { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
/// assert_eq!(unsafe { ponko_ctx_clear(std::ptr::null_mut(), black) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_clear(ctx: *mut PonkoCtx, color: PonkoColor) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.clear(to_c(color));
            Ok(())
        })
    })
}
/// Draws a texture centered at `pos` (world units) this frame.
/// ```
/// use ponko_ffi::*;
/// let t = PonkoTexture { id: 0, width: 1, height: 1 };
/// assert_eq!(unsafe { ponko_ctx_draw(std::ptr::null_mut(), t, PonkoVec2::default()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw(ctx: *mut PonkoCtx, texture: PonkoTexture, pos: PonkoVec2) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.draw(texture_from_c(texture), to_c(pos));
            Ok(())
        })
    })
}
/// Draws a texture with options (start from `ponko_draw_params_default()`).
/// ```
/// use ponko_ffi::*;
/// let t = PonkoTexture { id: 0, width: 1, height: 1 };
/// assert_eq!(unsafe { ponko_ctx_draw_ex(std::ptr::null_mut(), t, PonkoVec2::default(), std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw_ex(
    ctx: *mut PonkoCtx,
    texture: PonkoTexture,
    pos: PonkoVec2,
    params: *const PonkoDrawParams,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if params.is_null() {
                return Err(invalid("params is null"));
            }
            c.gfx
                .draw_ex(texture_from_c(texture), to_c(pos), draw_params_from_c(&*params));
            Ok(())
        })
    })
}
/// Draws UTF-8 text with its top-left at `pos`, `size` pixels tall, in window pixels.
/// ```
/// use ponko_ffi::*;
/// let white = PonkoColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
/// assert_eq!(unsafe { ponko_ctx_draw_text(std::ptr::null_mut(), c"hi".as_ptr(), PonkoVec2::default(), 16.0, white) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw_text(
    ctx: *mut PonkoCtx,
    string: *const c_char,
    pos: PonkoVec2,
    size: f32,
    color: PonkoColor,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.draw_text(text(string)?, to_c(pos), size, to_c(color));
            Ok(())
        })
    })
}
/// Size in pixels that `ponko_ctx_draw_text` would cover.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_measure_text(std::ptr::null_mut(), c"hi".as_ptr(), 16.0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_measure_text(
    ctx: *mut PonkoCtx,
    string: *const c_char,
    size: f32,
    out_size: *mut PonkoVec2,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let string = text(string)?;
            if out_size.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_size, to_c(c.gfx.measure_text(string, size)))
        })
    })
}

/// The camera: which world point is at the window's top-left, and the zoom
/// (window pixels per world unit).
/// ```
/// let cam = ponko_ffi::PonkoCamera { position: Default::default(), zoom: 1.0 };
/// assert_eq!(cam.zoom, 1.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoCamera {
    /// World point at the top-left corner.
    pub position: PonkoVec2,
    /// Window pixels per world unit.
    pub zoom: f32,
}

/// Reads the camera.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_camera(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_camera(ctx: *mut PonkoCtx, out_camera: *mut PonkoCamera) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            out(
                out_camera,
                PonkoCamera {
                    position: to_c(c.camera.position),
                    zoom: c.camera.zoom,
                },
            )
        })
    })
}
/// Moves or zooms the camera. Zoom must be positive.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_camera(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_camera(ctx: *mut PonkoCtx, camera: *const PonkoCamera) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if camera.is_null() {
                return Err(invalid("camera is null"));
            }
            let camera = *camera;
            if !(camera.zoom > 0.0 && camera.zoom.is_finite()) {
                return Err(invalid("zoom must be positive"));
            }
            c.camera.position = to_c(camera.position);
            c.camera.zoom = camera.zoom;
            Ok(())
        })
    })
}

// ---- commands -----------------------------------------------------------------

/// Values copied out of C memory for a later spawn or insert.
struct Owned {
    bytes: Vec<u8>,
    /// (component, offset, size) per value.
    parts: Vec<(u32, usize, usize)>,
}
impl Owned {
    unsafe fn copy(values: &[PonkoValue]) -> Result<Self, Failure> {
        let mut owned = Owned {
            bytes: Vec::new(),
            parts: Vec::with_capacity(values.len()),
        };
        for v in values {
            if v.data.is_null() {
                return Err(invalid("component data is null"));
            }
            let at = owned.bytes.len();
            // SAFETY: caller supplies `size` readable bytes.
            owned
                .bytes
                .extend_from_slice(unsafe { std::slice::from_raw_parts(v.data.cast(), v.size) });
            owned.parts.push((v.component, at, v.size));
        }
        Ok(owned)
    }
    fn values(&self) -> Vec<PonkoValue> {
        self.parts
            .iter()
            .map(|&(component, at, size)| PonkoValue {
                component,
                // Non-null even for zero-sized values.
                data: self.bytes.as_ptr().wrapping_add(at).cast(),
                size,
            })
            .collect()
    }
}

fn report(what: &str, failure: Failure) {
    eprintln!("ponko: {what} skipped: {}", failure.1);
}

/// Despawns `entity` after this system (stale entities are ignored).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_despawn_later(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
/// Works inside `ponko_query` callbacks (not `ponko_query_par`).
/// Applied after the system returns.
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_despawn_later(ctx: *mut PonkoCtx, entity: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        commands(ctx, |c| {
            c.despawn(Entity::from_bits(entity));
            Ok(())
        })
    })
}
/// Spawns an entity with copies of `values` after this system. Values
/// are checked when applied; bad ones skip the spawn with a message on stderr.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_spawn_later(std::ptr::null_mut(), std::ptr::null(), 0) } as u32, 1);
/// ```
/// Works inside `ponko_query` callbacks (not `ponko_query_par`).
/// Applied after the system returns.
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_spawn_later(
    ctx: *mut PonkoCtx,
    values: *const PonkoValue,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        commands(ctx, |c| {
            if values.is_null() && count != 0 {
                return Err(invalid("values is null"));
            }
            let owned = if count == 0 {
                Owned::copy(&[])?
            } else {
                Owned::copy(std::slice::from_raw_parts(values, count))?
            };
            c.add(move |w| {
                // SAFETY: the values point into `owned`, which lives until this returns.
                if let Err(f) = spawn_values(w, &owned.values()) {
                    report("spawn_later", f)
                }
            });
            Ok(())
        })
    })
}
/// Adds or replaces a component of `entity` after this system.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_later(std::ptr::null_mut(), 0, std::ptr::null()) } as u32, 1);
/// ```
/// Works inside `ponko_query` callbacks (not `ponko_query_par`).
/// Applied after the system returns.
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_later(ctx: *mut PonkoCtx, entity: u64, value: *const PonkoValue) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        commands(ctx, |c| {
            if value.is_null() {
                return Err(invalid("value is null"));
            }
            let owned = Owned::copy(std::slice::from_raw_parts(value, 1))?;
            c.add(move |w| {
                let v = owned.values()[0];
                let applied = crate::value_layout(w, v.component, v.data, v.size)
                    // SAFETY: layout validated against the world just now.
                    .and_then(|(id, kind): (_, Kind)| insert_value(w, Entity::from_bits(entity), id, kind, v.data));
                if let Err(f) = applied {
                    report("set_later", f)
                }
            });
            Ok(())
        })
    })
}
/// Removes a component of `entity` after this system.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_remove_later(std::ptr::null_mut(), 0, 0) } as u32, 1);
/// ```
/// Works inside `ponko_query` callbacks (not `ponko_query_par`).
/// Applied after the system returns.
/// # Safety
/// `ctx` must be null or live; pointer arguments must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_remove_later(ctx: *mut PonkoCtx, entity: u64, component: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        commands(ctx, |c| {
            c.add(move |w| match crate::component(w, component) {
                Ok((id, _)) => {
                    w.raw().remove_dynamic(Entity::from_bits(entity), id);
                }
                Err(f) => report("remove_later", f),
            });
            Ok(())
        })
    })
}

// ---- events -------------------------------------------------------------------

/// Sends one event of type `type_id` (use the event struct's component id,
/// so every system agrees on the type): `size` bytes copied from `data`, at
/// most 16-byte aligned. The first send of a type fixes its size. Systems
/// read it with `ponko_ctx_read_events`. Works inside `ponko_query`
/// callbacks (not `ponko_query_par`).
/// # Safety
/// Live ctx; `data` readable for `size` bytes.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_send_event(std::ptr::null_mut(), 0, std::ptr::null(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_send_event(
    ctx: *mut PonkoCtx,
    type_id: u32,
    data: *const c_void,
    size: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        events(ctx, |ev| {
            if data.is_null() && size > 0 {
                return Err(invalid("event data is null"));
            }
            let bytes = if size == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(data.cast::<u8>(), size)
            };
            ev.send_raw(type_id, bytes).map_err(invalid)
        })
    })
}

/// Events of type `type_id` this system hasn't read yet, oldest first:
/// `*out_count` values of `size` bytes each at `*out_data` (16-byte aligned).
/// The next call from the same system returns only newer ones. Each system
/// that runs every tick sees each event once. The memory stays valid until
/// the next send of this type or the end of the system.
/// # Safety
/// Live ctx; writable outputs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_read_events(std::ptr::null_mut(), 0, 0, std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_read_events(
    ctx: *mut PonkoCtx,
    type_id: u32,
    size: usize,
    out_data: *mut *const c_void,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_data.is_null() || out_count.is_null() {
            return Err(invalid("output is null"));
        }
        let (ptr, count) = events(ctx, |ev| ev.read_raw(type_id, size).map_err(invalid))?;
        out(out_data, ptr.cast::<c_void>())?;
        out(out_count, count)
    })
}

// ---- spatial ------------------------------------------------------------------

/// Entities of the built-in grid (`ponko_game_spatial_grid`) within `radius`
/// of `center`. Writes up to `capacity` hits and sets `*out_count` to how many
/// there are in total, so a too-small buffer can be grown and the call repeated.
/// Safe inside query callbacks, also from `ponko_query_par` threads.
/// # Safety
/// Live ctx; `out_hits` writable for `capacity` hits (may be null if 0); writable count.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_spatial_radius(std::ptr::null(), Default::default(), 1.0, std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_spatial_radius(
    ctx: *const PonkoCtx,
    center: PonkoVec2,
    radius: f32,
    out_hits: *mut PonkoGridHit,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let grid = unsafe { spatial(ctx)? };
        let mut n = 0;
        // SAFETY: forwarded caller contract.
        unsafe {
            collect(out_hits, capacity, out_count, &mut n, |push| {
                grid.for_each_in_radius(to_c(center), radius, |e| push.push(to_c(*e)))
            })
        }
    })
}

/// Like `ponko_ctx_spatial_radius`, for the rectangle from `min` to `max`.
/// # Safety
/// As `ponko_ctx_spatial_radius`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_spatial_rect(std::ptr::null(), Default::default(), Default::default(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_spatial_rect(
    ctx: *const PonkoCtx,
    min: PonkoVec2,
    max: PonkoVec2,
    out_hits: *mut PonkoGridHit,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let grid = unsafe { spatial(ctx)? };
        let mut n = 0;
        // SAFETY: forwarded caller contract.
        unsafe {
            collect(out_hits, capacity, out_count, &mut n, |push| {
                grid.for_each_in_rect(to_c(min), to_c(max), |e| push.push(to_c(*e)))
            })
        }
    })
}

/// The nearest entity of the built-in grid within `max_radius`, other than
/// `exclude` (pass an entity's own id, or `UINT64_MAX` for none). Returns
/// `PONKO_NOT_FOUND` if there is none.
/// # Safety
/// Live ctx and writable output.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_spatial_nearest(std::ptr::null(), Default::default(), 1.0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_spatial_nearest(
    ctx: *const PonkoCtx,
    center: PonkoVec2,
    max_radius: f32,
    exclude: u64,
    out_hit: *mut PonkoGridHit,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let grid = unsafe { spatial(ctx)? };
        if out_hit.is_null() {
            return Err(invalid("output is null"));
        }
        let hit = grid
            .nearest(to_c(center), max_radius, |e| e.entity.to_bits() != exclude)
            .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "nothing in range"))?;
        // SAFETY: checked non-null.
        unsafe { out(out_hit, to_c(hit)) }
    })
}

/// A view of the built-in grid (entries are `PonkoGridHit`), valid during
/// this callback. Safe from `ponko_query_par` threads.
/// # Safety
/// Live ctx; writable output.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_spatial_view(std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_spatial_view(
    ctx: *const PonkoCtx,
    out_view: *mut crate::PonkoGridView,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let grid = unsafe { spatial(ctx)? };
        // SAFETY: forwarded caller contract.
        unsafe { out(out_view, crate::grid::view_of(grid)) }
    })
}

/// Shared buffer-filling logic of the spatial queries.
pub(crate) unsafe fn collect<T>(
    out_items: *mut T,
    capacity: usize,
    out_count: *mut usize,
    n: &mut usize,
    each: impl FnOnce(&mut Sink<T>),
) -> Result<(), Failure> {
    if out_count.is_null() || (out_items.is_null() && capacity != 0) {
        return Err(invalid("output buffer or count is null"));
    }
    let mut sink = Sink {
        out: out_items,
        capacity,
        n: 0,
    };
    each(&mut sink);
    *n = sink.n;
    // SAFETY: checked non-null.
    unsafe { out_count.write(sink.n) };
    Ok(())
}

/// Writes results into a caller buffer and counts all of them. Generic (not
/// `dyn`), so the grid's closure inlines: neighbor search is the hot path.
pub(crate) struct Sink<T> {
    out: *mut T,
    capacity: usize,
    n: usize,
}
impl<T> Sink<T> {
    #[inline(always)]
    pub(crate) fn push(&mut self, item: T) {
        if self.n < self.capacity {
            // SAFETY: the caller of `collect` supplies `capacity` writable items.
            unsafe { self.out.add(self.n).write(item) };
        }
        self.n += 1;
    }
}
