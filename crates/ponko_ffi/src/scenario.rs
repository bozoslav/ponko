//! Input scenarios from C: raw input and checks scheduled by simulation tick,
//! for reproducible headless tests (Rust's `InputScenario`,
//! `Game::input_scenario`, `Input::start_scenario` and `ScenarioReport`).
use std::ffi::c_char;

use ponko::scenario::{InputScenario, ScenarioCheck, ScenarioReport, ScenarioStatus};

use crate::actions::input_event;
use crate::ctx::{ctx_access, out, text};
use crate::game::build;
use crate::{
    Failure, PonkoCtx, PonkoGame, PonkoStatus, PonkoWorld, access, boundary, error, invalid, invalid_owned, write_text,
};

/// A scenario being built: input steps and checks by tick. Free it with
/// `ponko_input_scenario_free`; games and contexts take copies.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
pub struct PonkoInputScenario(InputScenario);

/// Where a scenario run stands (`PonkoScenarioReport::status`).
/// ```
/// assert_eq!(ponko_ffi::PonkoScenarioStatus::PONKO_SCENARIO_FAILED as u32, 3);
/// ```
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum PonkoScenarioStatus {
    /// Installed; its first tick has not run.
    PONKO_SCENARIO_PENDING = 0,
    /// Running.
    PONKO_SCENARIO_RUNNING = 1,
    /// Ran its whole duration and every check held.
    PONKO_SCENARIO_PASSED = 2,
    /// A check failed; the run ended there.
    PONKO_SCENARIO_FAILED = 3,
    /// Ended early: cancelled, or replaced by another scenario.
    PONKO_SCENARIO_CANCELLED = 4,
}

/// The outcome of the latest scenario run, as numbers (Rust's
/// `ScenarioReport`); `ponko_world_scenario_report_json` has all of it,
/// with the failed check's message, expected and actual values.
/// ```
/// let r = ponko_ffi::PonkoScenarioReport { status: 0, failed: false, duration: 0, ticks_run: 0, start_tick: 0, steps: 0, inputs_applied: 0, checks_passed: 0, failure_step: 0, failure_tick: 0, failure_sim_tick: 0, released: 0, still_held: 0 };
/// assert!(!r.failed);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PonkoScenarioReport {
    /// A `PonkoScenarioStatus`.
    pub status: u32,
    /// Whether a check failed; the `failure_*` fields are 0 otherwise.
    pub failed: bool,
    /// Ticks the run lasts.
    pub duration: u64,
    /// Ticks that have run.
    pub ticks_run: u64,
    /// The simulation tick that is the scenario's tick 0.
    pub start_tick: u64,
    /// Steps (inputs and checks) in the scenario.
    pub steps: u64,
    /// Input steps fed so far.
    pub inputs_applied: u64,
    /// Checks that held so far.
    pub checks_passed: u64,
    /// The failed check's step number (the order steps were added in, from 0).
    pub failure_step: u64,
    /// The scenario tick the failed check was scheduled at.
    pub failure_tick: u64,
    /// The simulation tick it ran on.
    pub failure_sim_tick: u64,
    /// Held inputs the run released when it ended early or failed.
    pub released: u64,
    /// Inputs a run that finished normally left held.
    pub still_held: u64,
}

fn report_to_c(r: &ScenarioReport) -> PonkoScenarioReport {
    let status = match r.status {
        ScenarioStatus::Pending => PonkoScenarioStatus::PONKO_SCENARIO_PENDING,
        ScenarioStatus::Running => PonkoScenarioStatus::PONKO_SCENARIO_RUNNING,
        ScenarioStatus::Passed => PonkoScenarioStatus::PONKO_SCENARIO_PASSED,
        ScenarioStatus::Failed => PonkoScenarioStatus::PONKO_SCENARIO_FAILED,
        ScenarioStatus::Cancelled => PonkoScenarioStatus::PONKO_SCENARIO_CANCELLED,
    };
    let (step, tick, sim_tick) = r
        .failure
        .as_ref()
        .map_or((0, 0, 0), |f| (f.step as u64, f.tick, f.sim_tick));
    PonkoScenarioReport {
        status: status as u32,
        failed: r.failure.is_some(),
        duration: r.duration,
        ticks_run: r.ticks_run,
        start_tick: r.start_tick,
        steps: r.steps as u64,
        inputs_applied: r.inputs_applied as u64,
        checks_passed: r.checks_passed as u64,
        failure_step: step,
        failure_tick: tick,
        failure_sim_tick: sim_tick,
        released: r.released as u64,
        still_held: r.still_held as u64,
    }
}

unsafe fn scenario_mut<'a>(scenario: *mut PonkoInputScenario) -> Result<&'a mut InputScenario, Failure> {
    // SAFETY: caller supplies a live, exclusively accessed handle.
    unsafe { scenario.as_mut() }
        .map(|s| &mut s.0)
        .ok_or_else(|| invalid("scenario is null"))
}

unsafe fn scenario_ref<'a>(scenario: *const PonkoInputScenario) -> Result<&'a InputScenario, Failure> {
    // SAFETY: caller supplies a live handle.
    unsafe { scenario.as_ref() }
        .map(|s| &s.0)
        .ok_or_else(|| invalid("scenario is null"))
}

/// Applies a builder step to the scenario in place. `prepare` validates the
/// arguments and returns the step; on bad arguments the scenario is left as
/// it was. The step itself can't fail, so the scenario is moved through it
/// rather than copied (a copy per step would make building O(n²)).
pub(crate) unsafe fn edit<B: FnOnce(InputScenario) -> InputScenario>(
    scenario: *mut PonkoInputScenario,
    prepare: impl FnOnce() -> Result<B, Failure>,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let s = unsafe { scenario_mut(scenario)? };
        let step = prepare()?;
        *s = step(std::mem::replace(s, InputScenario::new()));
        Ok(())
    })
}

fn json_error(message: String) -> Failure {
    invalid_owned(ponko::prelude::Error::new(ponko::prelude::ErrorKind::Json, message))
}

/// An empty scenario. Free it with `ponko_input_scenario_free`.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert!(!s.is_null());
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_input_scenario_new() -> *mut PonkoInputScenario {
    Box::into_raw(Box::new(PonkoInputScenario(InputScenario::new())))
}

/// Frees a scenario; null is ignored.
/// ```
/// unsafe { ponko_ffi::ponko_input_scenario_free(std::ptr::null_mut()) };
/// ```
/// # Safety
/// `scenario` null or owned, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_free(scenario: *mut PonkoInputScenario) {
    if !scenario.is_null() {
        // SAFETY: caller passes an owned handle, once.
        drop(unsafe { Box::from_raw(scenario) });
    }
}

/// Reads a scenario from the versioned JSON format that Rust, tools and the
/// control endpoint use. On success `*out` is a new scenario to free; bad
/// JSON is `PONKO_INVALID_ARGUMENT` with kind `PONKO_ERROR_JSON`.
/// ```
/// let mut s = std::ptr::null_mut();
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_from_json(c"{\"version\": 1, \"steps\": []}".as_ptr(), &mut s) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `json` NUL-terminated; `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_from_json(
    json: *const c_char,
    out_scenario: *mut *mut PonkoInputScenario,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: caller supplies a NUL-terminated string.
        let json = unsafe { text(json)? };
        if out_scenario.is_null() {
            return Err(invalid("output is null"));
        }
        let scenario = InputScenario::from_json(json).map_err(|e| json_error(e.to_string()))?;
        // SAFETY: checked non-null; caller supplies writable output.
        unsafe { out_scenario.write(Box::into_raw(Box::new(PonkoInputScenario(scenario)))) };
        Ok(())
    })
}

/// Writes the scenario as JSON (`ponko_input_scenario_from_json` reads it
/// back). `*out_len` gets the length without the NUL; the text is written
/// only if it fits in `capacity`: call with capacity 0 for the length, then
/// with length + 1.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// let mut len = 0usize;
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_to_json(s, std::ptr::null_mut(), 0, &mut len) } as u32, 0);
/// assert!(len > 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live; `buffer` writable for `capacity` bytes (or null
/// with capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_to_json(
    scenario: *const PonkoInputScenario,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let json = unsafe { scenario_ref(scenario)? }.to_json().to_string();
        // SAFETY: forwarded caller contract.
        unsafe { write_text(buffer, capacity, out_len, &json) }
    })
}

/// Adds an input step at simulation tick `tick`, fed before that tick's
/// simulation systems run. The event is described as for
/// `ponko_ctx_feed_input`: `kind` 0 key, 1 mouse, 2 gamepad button, 3
/// gamepad axis, 4 focus lost, 5 gamepad disconnected.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// let space = ponko_ffi::PonkoKey::PONKO_KEY_SPACE as u32;
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_at(s, 2, 0, 0, space, 1.0) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_at(
    scenario: *mut PonkoInputScenario,
    tick: u64,
    kind: u32,
    slot: u32,
    input: u32,
    value: f32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        edit(scenario, || {
            let event = input_event(kind, slot, input, value)?;
            Ok(move |s: InputScenario| s.at(tick, event))
        })
    }
}

pub(crate) fn expectation(value: i32) -> Result<Option<bool>, Failure> {
    match value {
        -1 => Ok(None),
        0 => Ok(Some(false)),
        1 => Ok(Some(true)),
        _ => Err(invalid("expected state must be -1 (not checked), 0 or 1")),
    }
}

/// Adds a check of action `name` at tick `tick`, after that tick's
/// simulation systems ran. `pressed`, `down` and `released` are each 1
/// (expected true), 0 (expected false) or -1 (not checked).
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_expect_action(s, 2, c"jump".as_ptr(), 1, 1, -1) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live; `name` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_expect_action(
    scenario: *mut PonkoInputScenario,
    tick: u64,
    name: *const c_char,
    pressed: i32,
    down: i32,
    released: i32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        edit(scenario, || {
            let name = text(name)?;
            let check = ScenarioCheck::Action {
                name: name.to_owned(),
                pressed: expectation(pressed)?,
                down: expectation(down)?,
                released: expectation(released)?,
            };
            Ok(move |s: InputScenario| s.expect(tick, check))
        })
    }
}

/// Adds a check that the reflected resource `name` (as tools see it, e.g. a
/// resource described with `ponko_resource_reflect`) has the JSON value
/// `equals_json` at JSON pointer `pointer` (`"/points"`; empty for the whole
/// resource). Numbers compare by value.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_expect_resource(s, 3, c"Score".as_ptr(), c"/points".as_ptr(), c"3".as_ptr()) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live; strings NUL-terminated (`pointer` may be null for empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_expect_resource(
    scenario: *mut PonkoInputScenario,
    tick: u64,
    name: *const c_char,
    pointer: *const c_char,
    equals_json: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        edit(scenario, || {
            let name = text(name)?;
            let pointer = if pointer.is_null() { "" } else { text(pointer)? };
            let equals: serde_json::Value =
                serde_json::from_str(text(equals_json)?).map_err(|e| json_error(format!("expected value: {e}")))?;
            let check = ScenarioCheck::resource(name, pointer, equals);
            Ok(move |s: InputScenario| s.expect(tick, check))
        })
    }
}

/// Adds a check that `count` entities have the reflected component `component`.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_expect_count(s, 3, c"Coin".as_ptr(), 2) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live; `component` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_expect_count(
    scenario: *mut PonkoInputScenario,
    tick: u64,
    component: *const c_char,
    count: u64,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        edit(scenario, || {
            let component = text(component)?;
            let count = usize::try_from(count).map_err(|_| invalid("count out of range"))?;
            let check = ScenarioCheck::count(component, count);
            Ok(move |s: InputScenario| s.expect(tick, check))
        })
    }
}

/// Names the scenario in its report (copied; empty is allowed).
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_named(s, c"jump test".as_ptr()) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live; `id` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_named(
    scenario: *mut PonkoInputScenario,
    id: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        edit(scenario, || {
            let id = text(id)?;
            Ok(move |s: InputScenario| s.named(id))
        })
    }
}

/// Makes the run last `ticks` simulation ticks (default: through the last
/// step's tick). Steps at or after `ticks` never run.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_duration(s, 10) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_duration(scenario: *mut PonkoInputScenario, ticks: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { edit(scenario, || Ok(move |s: InputScenario| s.duration(ticks))) }
}

/// Releases the inputs still held when the run finishes normally (cancelled
/// and failed runs always release them).
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_release_when_done(s) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_release_when_done(scenario: *mut PonkoInputScenario) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { edit(scenario, || Ok(|s: InputScenario| s.release_when_done())) }
}

/// How many ticks a run of the scenario lasts, and how many steps it holds:
/// pass `ticks` to `ponko_game_run_headless` to run all of it. Either
/// output may be null.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// let (mut ticks, mut steps) = (1u64, 1usize);
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_info(s, &mut ticks, &mut steps) } as u32, 0);
/// assert_eq!((ticks, steps), (0, 0));
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live; outputs null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_info(
    scenario: *const PonkoInputScenario,
    out_ticks: *mut u64,
    out_steps: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let s = unsafe { scenario_ref(scenario)? };
        // SAFETY: outputs are null or writable.
        unsafe {
            if !out_ticks.is_null() {
                out_ticks.write(s.ticks());
            }
            if !out_steps.is_null() {
                out_steps.write(s.step_count());
            }
        }
        Ok(())
    })
}

/// Runs a copy of `scenario` with the game, from its first tick (Rust's
/// `Game::input_scenario`). Run it with `ponko_game_run_headless` for the
/// scenario's ticks (`ponko_input_scenario_info`), then read
/// `ponko_world_scenario_report` from the world it returns.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_game_input_scenario(std::ptr::null_mut(), s) } as u32, 1);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `game` null or live, never run; `scenario` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_input_scenario(
    game: *mut PonkoGame,
    scenario: *const PonkoInputScenario,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    let scenario = match unsafe { scenario.as_ref() } {
        Some(s) => s.0.clone(),
        None => return boundary(|| Err(invalid("scenario is null"))),
    };
    // SAFETY: forwarded caller contract.
    unsafe { build(game, |g| (g.input_scenario(scenario), Ok(()))) }
}

/// Starts a copy of `scenario` now, replacing (and cancelling) a running one
/// (Rust's `Input::start_scenario`). Started from a system, its tick 0 is
/// the next simulation tick.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_start_scenario(std::ptr::null_mut(), s) } as u32, 1);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `ctx` null or live; `scenario` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_start_scenario(
    ctx: *mut PonkoCtx,
    scenario: *const PonkoInputScenario,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let scenario = scenario_ref(scenario)?.clone();
        ctx_access(ctx, |c| {
            c.start_scenario(scenario);
            Ok(())
        })
    })
}

/// Ends the running scenario, releasing the inputs it holds (Rust's
/// `Input::cancel_scenario`). `*out_was_running` (may be null) says
/// whether one was running.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_cancel_scenario(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_was_running` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_cancel_scenario(ctx: *mut PonkoCtx, out_was_running: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let was = c.cancel_scenario();
            if out_was_running.is_null() {
                Ok(())
            } else {
                out(out_was_running, was)
            }
        })
    })
}

unsafe fn with_report<R>(
    world: *mut PonkoWorld,
    f: impl FnOnce(&ScenarioReport) -> Result<R, Failure>,
) -> Result<R, Failure> {
    // SAFETY: forwarded caller contract.
    unsafe {
        access(world, |w, _| {
            let report = w
                .get_resource::<ScenarioReport>()
                .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "no input scenario has run in this world"))?;
            f(report)
        })
    }
}

/// The latest scenario run's report, kept in the world (from a game's
/// `ponko_ctx_world`, or the world `ponko_game_run_headless` returned).
/// `PONKO_NOT_FOUND` if no scenario ran there.
/// ```
/// let w = ponko_ffi::ponko_world_new();
/// let mut r = std::mem::MaybeUninit::<ponko_ffi::PonkoScenarioReport>::zeroed();
/// assert_eq!(unsafe { ponko_ffi::ponko_world_scenario_report(w, r.as_mut_ptr()) }, ponko_ffi::PonkoStatus::PONKO_NOT_FOUND);
/// unsafe { ponko_ffi::ponko_world_free(w) };
/// ```
/// # Safety
/// `world` null or live; `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_world_scenario_report(
    world: *mut PonkoWorld,
    out_report: *mut PonkoScenarioReport,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_report.is_null() {
            return Err(invalid("output is null"));
        }
        with_report(world, |r| out(out_report, report_to_c(r)))
    })
}

/// The latest scenario run's report as JSON, with the failed check's
/// message, expected and actual values (the same object the control
/// endpoint's `input.scenario.status` returns). Writes like
/// `ponko_input_scenario_to_json`; `PONKO_NOT_FOUND` if no scenario ran.
/// ```
/// let w = ponko_ffi::ponko_world_new();
/// let mut len = 0usize;
/// assert_eq!(unsafe { ponko_ffi::ponko_world_scenario_report_json(w, std::ptr::null_mut(), 0, &mut len) }, ponko_ffi::PonkoStatus::PONKO_NOT_FOUND);
/// unsafe { ponko_ffi::ponko_world_free(w) };
/// ```
/// # Safety
/// `world` null or live; `buffer` writable for `capacity` bytes (or null
/// with capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_world_scenario_report_json(
    world: *mut PonkoWorld,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_report(world, |r| {
            let json = serde_json::to_string(r).map_err(|e| json_error(e.to_string()))?;
            write_text(buffer, capacity, out_len, &json)
        })
    })
}
