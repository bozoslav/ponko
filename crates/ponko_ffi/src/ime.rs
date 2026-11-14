//! IME composition (#410): the C side of Rust's `Input::typed_text`, `feed_ime`,
//! `preedit`, `preedit_cursor`, `composing`, `set_ime_allowed`,
//! `ime_allowed`, `set_ime_cursor_area`, `ime_cursor_area` and
//! `cancel_composition`; the retained UI's `set_ime_allowed`,
//! `ime_allowed`, `wants_ime`, `preedit`, `ime_area` and
//! `composition_interrupted`; a UI frame with a composition; and the input
//! scenario's IME steps.
use std::ffi::c_char;

use ponko::input::ImeEvent;
use ponko::scenario::InputScenario;

use crate::ctx::{ctx_access, out};
use crate::scenario::{PonkoInputScenario, edit};
use crate::types::{PonkoRect, PonkoVec2, to_c};
use crate::ui_retained::{PonkoUiInput, check_output, copy_string, existing, optional_text, ui_input_from_c, with_ui};
use crate::{Failure, PonkoCtx, PonkoStatus, boundary, invalid};

/// What kind of IME step a `PonkoImeEvent` is (Rust's `ImeEvent`).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum PonkoImeEventKind {
    /// The window system turned IME on.
    PONKO_IME_ENABLED = 0,
    /// The composition so far is `text`; an empty `text` cancels it.
    PONKO_IME_PREEDIT = 1,
    /// The composition ended with `text`, which is typed.
    PONKO_IME_COMMIT = 2,
    /// The window system turned IME off; a composition is dropped.
    PONKO_IME_DISABLED = 3,
}

/// One IME step, for `ponko_ctx_feed_ime` and `ponko_input_scenario_ime`.
/// ```
/// let e = ponko_ffi::PonkoImeEvent::default();
/// assert_eq!(e.kind, 0);
/// assert!(e.text.is_null() && !e.has_cursor);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoImeEvent {
    /// A `PonkoImeEventKind` value.
    pub kind: u32,
    /// The preedit or committed text, NUL-terminated UTF-8, read only during
    /// the call; null for none (empty).
    pub text: *const c_char,
    /// A preedit shows a caret (`cursor_start..cursor_end`, bytes of `text`).
    pub has_cursor: bool,
    /// Where the preedit's caret or converted part starts, in bytes.
    pub cursor_start: usize,
    /// Where it ends, in bytes (`== cursor_start` for a plain caret).
    pub cursor_end: usize,
}

impl Default for PonkoImeEvent {
    fn default() -> Self {
        Self {
            kind: PonkoImeEventKind::PONKO_IME_ENABLED as u32,
            text: std::ptr::null(),
            has_cursor: false,
            cursor_start: 0,
            cursor_end: 0,
        }
    }
}

/// The IME composition handed to a UI frame (Rust's `UiInput::preedit`,
/// `preedit_cursor` and `composing`), for `ponko_ctx_ui_frame_ime`.
/// ```
/// let c = ponko_ffi::PonkoUiComposition::default();
/// assert!(c.preedit.is_null() && !c.composing);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoUiComposition {
    /// The composition so far, NUL-terminated; null or empty for none.
    pub preedit: *const c_char,
    /// The preedit has a caret (`cursor_start..cursor_end`, bytes).
    pub has_cursor: bool,
    /// Where the preedit's caret or converted part starts, in bytes.
    pub cursor_start: usize,
    /// Where it ends, in bytes.
    pub cursor_end: usize,
    /// A composition went on this frame: the focused text input leaves its
    /// editing keys to the IME.
    pub composing: bool,
}

impl Default for PonkoUiComposition {
    fn default() -> Self {
        Self {
            preedit: std::ptr::null(),
            has_cursor: false,
            cursor_start: 0,
            cursor_end: 0,
            composing: false,
        }
    }
}

/// The Rust event for a C one.
///
/// # Safety
/// `e.text` null or NUL-terminated.
unsafe fn ime_event(e: &PonkoImeEvent) -> Result<ImeEvent, Failure> {
    // SAFETY: caller contract.
    let text = unsafe { optional_text(e.text) }?.to_owned();
    let cursor = e.has_cursor.then_some((e.cursor_start, e.cursor_end));
    Ok(match e.kind {
        k if k == PonkoImeEventKind::PONKO_IME_ENABLED as u32 => ImeEvent::Enabled,
        k if k == PonkoImeEventKind::PONKO_IME_PREEDIT as u32 => ImeEvent::Preedit { text, cursor },
        k if k == PonkoImeEventKind::PONKO_IME_COMMIT as u32 => ImeEvent::Commit(text),
        k if k == PonkoImeEventKind::PONKO_IME_DISABLED as u32 => ImeEvent::Disabled,
        _ => return Err(invalid("not a PonkoImeEventKind value")),
    })
}

/// Records an IME step as the window system would (Rust's
/// `input.feed_ime`), for custom hosts, tests and replays. A commit's text
/// is typed. A preedit caret off a character boundary is dropped.
/// ```
/// let e = ponko_ffi::PonkoImeEvent::default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_feed_ime(std::ptr::null_mut(), &e) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `event` null or readable, its `text` null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_feed_ime(ctx: *mut PonkoCtx, event: *const PonkoImeEvent) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let event = ime_event(event.as_ref().ok_or_else(|| invalid("event is null"))?)?;
        ctx_access(ctx, |c| {
            c.input.feed_ime(event);
            Ok(())
        })
    })
}

/// The text being composed (Rust's `input.preedit`), NUL-terminated into
/// `buffer` of `capacity` bytes, its length to `out_len` (as
/// `ponko_ctx_ui_text` does). Empty when nothing is being composed.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_preedit(std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `buffer` writable for `capacity` bytes (or null with
/// capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_preedit(
    ctx: *mut PonkoCtx,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        check_output(buffer, capacity, out_len)?;
        ctx_access(ctx, |c| copy_string(c.input.preedit(), buffer, capacity, out_len))
    })
}

/// The text typed this frame (Rust's `input.typed_text`): committed
/// Unicode from the keyboard and from IME commits, NUL-terminated into
/// `buffer` as `ponko_ctx_preedit` does. For a game's own text widgets.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_typed_text(std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `buffer` writable for `capacity` bytes (or null with
/// capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_typed_text(
    ctx: *mut PonkoCtx,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        check_output(buffer, capacity, out_len)?;
        ctx_access(ctx, |c| copy_string(c.input.typed_text(), buffer, capacity, out_len))
    })
}

/// The caret inside the preedit (Rust's `input.preedit_cursor`): whether
/// there is one to `out_has`, and its byte range to `out_start`/`out_end`
/// (0 when there is none).
/// ```
/// let (mut has, mut a, mut b) = (false, 0, 0);
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_preedit_cursor(std::ptr::null_mut(), &mut has, &mut a, &mut b) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_preedit_cursor(
    ctx: *mut PonkoCtx,
    out_has: *mut bool,
    out_start: *mut usize,
    out_end: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_has.is_null() || out_start.is_null() || out_end.is_null() {
            return Err(invalid("output is null"));
        }
        ctx_access(ctx, |c| {
            let cursor = c.input.preedit_cursor();
            let (a, b) = cursor.unwrap_or_default();
            out(out_has, cursor.is_some())?;
            out(out_start, a)?;
            out(out_end, b)
        })
    })
}

/// Whether a composition went on this frame (Rust's `input.composing`):
/// then Enter, Escape, Backspace and the arrows belong to the IME.
/// ```
/// let mut on = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_composing(std::ptr::null_mut(), &mut on) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_composing` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_composing(ctx: *mut PonkoCtx, out_composing: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_composing, c.input.composing())) })
}

/// Lets the window system compose text with an IME, or not (Rust's
/// `input.set_ime_allowed`). Allow it while your own text widget has the
/// focus; the retained UI does this for its text inputs. Applied after the
/// frame.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_ime_allowed(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_ime_allowed(ctx: *mut PonkoCtx, allowed: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.input.set_ime_allowed(allowed);
            Ok(())
        })
    })
}

/// Whether IME is allowed (Rust's `input.ime_allowed`).
/// ```
/// let mut on = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ime_allowed(std::ptr::null_mut(), &mut on) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_allowed` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ime_allowed(ctx: *mut PonkoCtx, out_allowed: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_allowed, c.input.ime_allowed())) })
}

/// Where your text widget's caret is, in logical window units, so the
/// candidate window opens next to it (Rust's `input.set_ime_cursor_area`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_ime_cursor_area(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_ime_cursor_area(ctx: *mut PonkoCtx, area: PonkoRect) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.input.set_ime_cursor_area(to_c(area));
            Ok(())
        })
    })
}

/// The caret area set last (Rust's `input.ime_cursor_area`): whether one
/// was set to `out_has`, the area to `out_area`.
/// ```
/// let (mut has, mut area) = (false, Default::default());
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ime_cursor_area(std::ptr::null_mut(), &mut has, &mut area) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ime_cursor_area(
    ctx: *mut PonkoCtx,
    out_has: *mut bool,
    out_area: *mut PonkoRect,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_has.is_null() || out_area.is_null() {
            return Err(invalid("output is null"));
        }
        ctx_access(ctx, |c| {
            let area = c.input.ime_cursor_area();
            out(out_has, area.is_some())?;
            out(out_area, area.map(to_c).unwrap_or_default())
        })
    })
}

/// Drops the composition in progress (Rust's `input.cancel_composition`):
/// the preedit is cleared and the window system told to forget it, for
/// your own text widget losing the focus mid-composition.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_cancel_composition(std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_cancel_composition(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.input.cancel_composition();
            Ok(())
        })
    })
}

/// Lets the text input `id` take IME composition, or not (for digits, a
/// key binding or a password); Rust's `ui.set_ime_allowed`. The default is
/// to take it.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_set_ime_allowed(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0, false) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_set_ime_allowed(
    ctx: *mut PonkoCtx,
    target: u64,
    id: u64,
    allowed: bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_ui(ctx, target, |ui| {
            let id = existing(ui, id)?;
            ui.set_ime_allowed(id, allowed);
            Ok(())
        })
    })
}

/// Whether the text input `id` takes IME (Rust's `ui.ime_allowed`).
/// ```
/// let mut on = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_ime_allowed(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0, &mut on) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_allowed` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_ime_allowed(
    ctx: *mut PonkoCtx,
    target: u64,
    id: u64,
    out_allowed: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_allowed.is_null() {
            return Err(invalid("output is null"));
        }
        with_ui(ctx, target, |ui| {
            let id = existing(ui, id)?;
            out(out_allowed, ui.ime_allowed(id))
        })
    })
}

/// Whether the focus is on a text input that takes IME (Rust's
/// `ui.wants_ime`), so a custom host should allow IME for its window.
/// ```
/// let mut on = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_wants_ime(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, &mut on) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_wants` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_wants_ime(ctx: *mut PonkoCtx, target: u64, out_wants: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_wants.is_null() {
            return Err(invalid("output is null"));
        }
        with_ui(ctx, target, |ui| out(out_wants, ui.wants_ime()))
    })
}

/// The preedit the text input `id` shows at its caret (Rust's
/// `ui.preedit`), into `buffer` as `ponko_ctx_ui_text` does; empty when it
/// shows none.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_preedit(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0, std::ptr::null_mut(), 0, &mut 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `buffer` writable for `capacity` bytes (or null with
/// capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_preedit(
    ctx: *mut PonkoCtx,
    target: u64,
    id: u64,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        check_output(buffer, capacity, out_len)?;
        with_ui(ctx, target, |ui| {
            let id = existing(ui, id)?;
            copy_string(ui.preedit(id).unwrap_or(""), buffer, capacity, out_len)
        })
    })
}

/// The caret of the focused text input that takes IME, in logical window
/// units, as of the last frame (Rust's `ui.ime_area`): whether there is one
/// to `out_has`, the area to `out_area`.
/// ```
/// let (mut has, mut area) = (false, Default::default());
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_ime_area(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, &mut has, &mut area) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_ime_area(
    ctx: *mut PonkoCtx,
    target: u64,
    out_has: *mut bool,
    out_area: *mut PonkoRect,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_has.is_null() || out_area.is_null() {
            return Err(invalid("output is null"));
        }
        with_ui(ctx, target, |ui| {
            let area = ui.ime_area();
            out(out_has, area.is_some())?;
            out(out_area, area.map(to_c).unwrap_or_default())
        })
    })
}

/// Whether the last UI frame took the focus off a text input in the middle
/// of a composition (Rust's `ui.composition_interrupted`); a custom host
/// then cancels the composition (`ponko_ctx_cancel_composition`).
/// ```
/// let mut cut = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_composition_interrupted(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, &mut cut) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_interrupted` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_composition_interrupted(
    ctx: *mut PonkoCtx,
    target: u64,
    out_interrupted: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_interrupted.is_null() {
            return Err(invalid("output is null"));
        }
        with_ui(ctx, target, |ui| out(out_interrupted, ui.composition_interrupted()))
    })
}

/// `ponko_ctx_ui_frame` with an IME composition: the focused text input
/// that takes IME shows `composition->preedit` at its caret, and leaves its
/// editing keys to the IME while `composition->composing`. Null `input` or
/// `composition`: none.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_frame_ime(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, Default::default(), std::ptr::null(), std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `input` and `composition` null or readable, their
/// strings null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_frame_ime(
    ctx: *mut PonkoCtx,
    target: u64,
    viewport: PonkoVec2,
    input: *const PonkoUiInput,
    composition: *const PonkoUiComposition,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let mut frame = match input.as_ref() {
            Some(i) => ui_input_from_c(i)?,
            None => ponko::ui::UiInput::default(),
        };
        if let Some(c) = composition.as_ref() {
            frame.preedit = optional_text(c.preedit)?.to_owned();
            frame.preedit_cursor = c.has_cursor.then_some((c.cursor_start, c.cursor_end));
            frame.composing = c.composing;
        }
        with_ui(ctx, target, |ui| {
            ui.frame(to_c(viewport), &frame);
            Ok(())
        })
    })
}

/// Adds an IME step at tick `tick` (Rust's `InputScenario::ime`), fed after
/// that tick's raw input steps, so compositions can be replayed into the
/// focused text input.
/// ```
/// let s = ponko_ffi::ponko_input_scenario_new();
/// let e = ponko_ffi::PonkoImeEvent { kind: 2, text: c"日本".as_ptr(), ..Default::default() };
/// assert_eq!(unsafe { ponko_ffi::ponko_input_scenario_ime(s, 3, &e) } as u32, 0);
/// unsafe { ponko_ffi::ponko_input_scenario_free(s) };
/// ```
/// # Safety
/// `scenario` null or live; `event` null or readable, its `text` null or
/// NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_input_scenario_ime(
    scenario: *mut PonkoInputScenario,
    tick: u64,
    event: *const PonkoImeEvent,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        edit(scenario, || {
            let event = ime_event(event.as_ref().ok_or_else(|| invalid("event is null"))?)?;
            Ok(move |s: InputScenario| s.ime(tick, event))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ime_kinds_are_refused() {
        let e = PonkoImeEvent {
            kind: 9,
            ..Default::default()
        };
        // SAFETY: the text is null.
        assert!(unsafe { ime_event(&e) }.is_err());
    }

    #[test]
    fn a_preedit_event_keeps_its_text_and_cursor() {
        let e = PonkoImeEvent {
            kind: PonkoImeEventKind::PONKO_IME_PREEDIT as u32,
            text: c"にほ".as_ptr(),
            has_cursor: true,
            cursor_start: 6,
            cursor_end: 6,
        };
        // SAFETY: a NUL-terminated literal.
        assert!(unsafe { ime_event(&e) }.is_ok_and(|e| e == ImeEvent::preedit("にほ", Some((6, 6)))));
    }
}
