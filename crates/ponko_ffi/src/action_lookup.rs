//! Reading an action map back (Rust `ActionMap::get` and `ActionMap::iter`)
//! and changing one installed action (`Input::set_action`).
use crate::actions::{PonkoActionBinding, PonkoActionMap, PonkoActionSource, action_from_c};
use crate::ctx::{ctx_access, out, text};
use crate::perf::write_text;
use crate::{Failure, PonkoCtx, PonkoStatus, boundary, error, invalid};
use ponko::input::GamepadStick;
use ponko::prelude::{Action, ActionMap, Axis2Binding, AxisBinding, ButtonBinding};
use std::ffi::c_char;

/// # Safety
/// Null or a live map handle.
unsafe fn map_ref<'a>(map: *const PonkoActionMap) -> Result<&'a ActionMap, Failure> {
    // SAFETY: caller supplies a live handle or null.
    unsafe { map.as_ref() }
        .map(PonkoActionMap::inner)
        .ok_or_else(|| invalid("action map is null"))
}

/// `action`'s kind (0 button, 1 scalar, 2 vector) and its sources as the
/// descriptions `ponko_action_map_set` takes.
pub(crate) fn action_to_c(action: &Action) -> (u32, Vec<PonkoActionBinding>) {
    let blank = PonkoActionBinding {
        kind: 0,
        pad: 0,
        first: 0,
        second: 0,
        third: 0,
        fourth: 0,
        deadzone: 0.0,
    };
    let source = |kind: PonkoActionSource| PonkoActionBinding {
        kind: kind as u32,
        ..blank
    };
    use PonkoActionSource::*;
    match action {
        Action::Button(list) => (
            0,
            list.iter()
                .map(|b| match *b {
                    ButtonBinding::Key(k) => PonkoActionBinding {
                        first: k as u32,
                        ..source(PONKO_ACTION_KEY)
                    },
                    ButtonBinding::Mouse(m) => PonkoActionBinding {
                        first: m as u32,
                        ..source(PONKO_ACTION_MOUSE)
                    },
                    ButtonBinding::Gamepad { pad, button } => PonkoActionBinding {
                        pad: pad.slot() as u32,
                        first: button as u32,
                        ..source(PONKO_ACTION_GAMEPAD_BUTTON)
                    },
                })
                .collect(),
        ),
        Action::Axis(list) => (
            1,
            list.iter()
                .map(|b| match *b {
                    AxisBinding::Keys { negative, positive } => PonkoActionBinding {
                        first: negative as u32,
                        second: positive as u32,
                        ..source(PONKO_ACTION_KEYS)
                    },
                    AxisBinding::Gamepad { pad, axis, deadzone } => PonkoActionBinding {
                        pad: pad.slot() as u32,
                        first: axis as u32,
                        deadzone,
                        ..source(PONKO_ACTION_GAMEPAD_AXIS)
                    },
                })
                .collect(),
        ),
        Action::Axis2(list) => (
            2,
            list.iter()
                .map(|b| match *b {
                    Axis2Binding::Keys { left, right, up, down } => PonkoActionBinding {
                        first: left as u32,
                        second: right as u32,
                        third: up as u32,
                        fourth: down as u32,
                        ..source(PONKO_ACTION_KEYS2)
                    },
                    Axis2Binding::Gamepad { pad, stick, deadzone } => PonkoActionBinding {
                        pad: pad.slot() as u32,
                        first: match stick {
                            GamepadStick::Left => 0,
                            GamepadStick::Right => 1,
                        },
                        deadzone,
                        ..source(PONKO_ACTION_GAMEPAD_STICK)
                    },
                })
                .collect(),
        ),
    }
}

/// The number of actions in `map` (Rust `ActionMap::len`).
/// ```
/// let mut n = 9;
/// assert_eq!(unsafe { ponko_ffi::ponko_action_map_count(std::ptr::null(), &mut n) } as u32, 1);
/// ```
/// # Safety
/// `map` null or live; `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_count(map: *const PonkoActionMap, out_count: *mut usize) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let map = map_ref(map)?;
        out(out_count, map.len())
    })
}

/// The name of action `index` of `map`, in name order (Rust
/// `ActionMap::iter`): together with `ponko_action_map_count` and
/// `ponko_action_map_get`, how a rebinding screen lists every action.
/// Writes the name to `buffer` (NUL-terminated when it fits in `capacity`)
/// and its length to `*out_len`; call with a null buffer and capacity 0 to
/// learn the length. An index past the end is `PONKO_NOT_FOUND`.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_action_map_name(std::ptr::null(), 0, std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `map` null or live; `buffer` writable for `capacity` bytes (or null with
/// capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_name(
    map: *const PonkoActionMap,
    index: usize,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let map = map_ref(map)?;
        if out_len.is_null() || (buffer.is_null() && capacity > 0) {
            return Err(invalid("buffer or out_len is null"));
        }
        let (name, _) = map
            .iter()
            .nth(index)
            .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "no action at that index"))?;
        write_text(buffer, capacity, out_len, name);
        Ok(())
    })
}

/// The bindings of action `name` (Rust `ActionMap::get`): its kind (0
/// button, 1 scalar, 2 vector) to `*out_kind`, the number of sources to
/// `*out_count`, and up to `capacity` of them to `out_bindings`, as the
/// descriptions `ponko_action_map_set` takes. Call with a null
/// `out_bindings` and capacity 0 to learn the count. An unknown name is
/// `PONKO_NOT_FOUND`.
/// ```
/// let (mut kind, mut n) = (0, 0);
/// assert_eq!(unsafe { ponko_ffi::ponko_action_map_get(std::ptr::null(), c"jump".as_ptr(), &mut kind, std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `map` null or live; `name` NUL-terminated; `out_kind` and `out_count`
/// writable; `out_bindings` writable for `capacity` values (or null with
/// capacity 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_get(
    map: *const PonkoActionMap,
    name: *const c_char,
    out_kind: *mut u32,
    out_bindings: *mut PonkoActionBinding,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let map = map_ref(map)?;
        let name = text(name)?;
        if out_kind.is_null() || out_count.is_null() || (out_bindings.is_null() && capacity > 0) {
            return Err(invalid("output is null"));
        }
        let action = map
            .get(name)
            .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "no action of that name"))?;
        let (kind, bindings) = action_to_c(action);
        for (i, b) in bindings.iter().take(capacity).enumerate() {
            out_bindings.add(i).write(*b);
        }
        out_kind.write(kind);
        out_count.write(bindings.len());
        Ok(())
    })
}

/// Adds or replaces one installed action, leaving the others alone (Rust
/// `Input::set_action`): `kind` and `bindings` as for
/// `ponko_action_map_set`. The name keeps its id, the other actions keep
/// their state, and sources already held fire no pressed edge. Invalid
/// bindings leave the actions unchanged.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_action(std::ptr::null_mut(), c"jump".as_ptr(), 0, std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `name` NUL-terminated; `bindings` readable for
/// `count` values (or null with count 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_action(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    kind: u32,
    bindings: *const PonkoActionBinding,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let name = text(name)?;
        let action = action_from_c(kind, bindings, count)?;
        ctx_access(ctx, |c| {
            c.input.set_action(name, action);
            Ok(())
        })
    })
}
