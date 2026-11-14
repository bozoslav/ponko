//! Named actions and their serializable binding configuration.
use crate::ctx::{ctx_access, out};
use crate::keys::{button_from_c, gamepad_axis_from_c, gamepad_button_from_c, key_from_c};
use crate::types::{PonkoVec2, to_c};
use crate::{Failure, PonkoCtx, PonkoStatus, boundary, engine_error, invalid, invalid_owned};
use ponko::actions::InputEvent;
use ponko::input::Gamepad;
use ponko::input::GamepadStick;
use ponko::prelude::{Action, ActionId, ActionMap, Axis2Binding, AxisBinding, ButtonBinding};
use std::ffi::{CStr, c_char};

/// Owned binding configuration. Install a copy with `ponko_ctx_set_actions`.
/// ```
/// let map = ponko_ffi::ponko_action_map_new();
/// unsafe { ponko_ffi::ponko_action_map_free(map); }
/// ```
pub struct PonkoActionMap(ActionMap);

impl PonkoActionMap {
    pub(crate) fn inner(&self) -> &ActionMap {
        &self.0
    }
}

/// Source kinds for `PonkoActionBinding::kind`.
/// ```
/// assert_eq!(ponko_ffi::PonkoActionSource::PONKO_ACTION_KEY as u32, 0);
/// ```
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
#[allow(non_camel_case_types)]
pub enum PonkoActionSource {
    /// Digital key (`first` = PonkoKey).
    PONKO_ACTION_KEY = 0,
    /// Digital mouse button (`first` = PonkoMouseButton).
    PONKO_ACTION_MOUSE = 1,
    /// Digital gamepad button (`first` = PonkoGamepadButton).
    PONKO_ACTION_GAMEPAD_BUTTON = 2,
    /// Scalar key pair (`first` negative, `second` positive).
    PONKO_ACTION_KEYS = 3,
    /// Scalar gamepad axis (`first` = PonkoGamepadAxis).
    PONKO_ACTION_GAMEPAD_AXIS = 4,
    /// Vector keys (`first..fourth` = left, right, up, down).
    PONKO_ACTION_KEYS2 = 5,
    /// Vector gamepad stick (`first` = 0 left, 1 right).
    PONKO_ACTION_GAMEPAD_STICK = 6,
}

/// One source: initialize unused fields to zero. `pad` is a controller slot.
/// ```
/// let binding = ponko_ffi::PonkoActionBinding { kind: 0, pad: 0, first: 0, second: 0, third: 0, fourth: 0, deadzone: 0.0 };
/// assert_eq!(binding.kind, 0);
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PonkoActionBinding {
    /// PonkoActionSource.
    pub kind: u32,
    /// Gamepad slot, for gamepad sources.
    pub pad: u32,
    /// First source parameter (see PonkoActionSource).
    pub first: u32,
    /// Second source parameter.
    pub second: u32,
    /// Third source parameter.
    pub third: u32,
    /// Fourth source parameter.
    pub fourth: u32,
    /// Gamepad deadzone in `0..1`.
    pub deadzone: f32,
}

unsafe fn text<'a>(ptr: *const c_char) -> Result<&'a str, Failure> {
    if ptr.is_null() {
        return Err(invalid("string is null"));
    }
    // SAFETY: caller supplies a NUL-terminated string, alive for the call.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| invalid("string is not UTF-8"))
}
pub(crate) unsafe fn map_mut<'a>(map: *mut PonkoActionMap) -> Result<&'a mut ActionMap, Failure> {
    // SAFETY: caller supplies a live, exclusively accessed handle.
    unsafe { map.as_mut() }
        .map(|m| &mut m.0)
        .ok_or_else(|| invalid("action map is null"))
}
fn pad(slot: u32) -> Result<Gamepad, Failure> {
    Gamepad::from_slot(slot as usize).ok_or_else(|| invalid("gamepad slot out of range"))
}
fn key(value: u32) -> Result<ponko::prelude::Key, Failure> {
    key_from_c(value).ok_or_else(|| invalid("unknown key"))
}
fn mouse(value: u32) -> Result<ponko::prelude::MouseButton, Failure> {
    button_from_c(value).ok_or_else(|| invalid("unknown mouse button"))
}
fn button(value: u32) -> Result<ponko::input::GamepadButton, Failure> {
    gamepad_button_from_c(value).ok_or_else(|| invalid("unknown gamepad button"))
}
fn axis(value: u32) -> Result<ponko::input::GamepadAxis, Failure> {
    gamepad_axis_from_c(value).ok_or_else(|| invalid("unknown gamepad axis"))
}

/// Create an empty map. Free once with `ponko_action_map_free`.
/// ```
/// let m = ponko_ffi::ponko_action_map_new();
/// unsafe { ponko_ffi::ponko_action_map_free(m); }
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_action_map_new() -> *mut PonkoActionMap {
    let mut result = std::ptr::null_mut();
    boundary(|| {
        result = Box::into_raw(Box::new(PonkoActionMap(ActionMap::default())));
        Ok(())
    });
    result
}

/// Free a map; null is ignored.
/// ```
/// unsafe { ponko_ffi::ponko_action_map_free(std::ptr::null_mut()); }
/// ```
/// # Safety
/// Handle must be null or owned, live, and not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_free(map: *mut PonkoActionMap) {
    if !map.is_null() {
        // SAFETY: caller transfers the owned handle exactly once.
        drop(unsafe { Box::from_raw(map) });
    }
}

/// Replace a named action. `kind`: 0 button, 1 scalar, 2 vector.
/// Sources must match the action kind. Invalid configuration leaves the map unchanged.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_action_map_set(std::ptr::null_mut(), c"jump".as_ptr(), 0, std::ptr::null(), 0) } as u32, 0);
/// ```
/// # Safety
/// Handle must be live or null, name NUL-terminated, bindings readable for count
/// elements (or null with count 0); pointers must not alias the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_set(
    map: *mut PonkoActionMap,
    name: *const c_char,
    kind: u32,
    bindings: *const PonkoActionBinding,
    count: usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: caller supplies live non-overlapping handles and strings.
        let (map, name) = unsafe { (map_mut(map)?, text(name)?) };
        // SAFETY: caller supplies `count` readable descriptions.
        let action = unsafe { action_from_c(kind, bindings, count)? };
        map.set(name, action);
        Ok(())
    })
}

/// The action `count` C descriptions describe, for action kind `kind`.
/// # Safety
/// `bindings` readable for `count` elements (or null with count 0).
pub(crate) unsafe fn action_from_c(
    kind: u32,
    bindings: *const PonkoActionBinding,
    count: usize,
) -> Result<Action, Failure> {
    if bindings.is_null() && count != 0 {
        return Err(invalid("bindings is null"));
    }
    let bindings = if count == 0 {
        &[]
    } else {
        // SAFETY: caller supplies count readable descriptions.
        unsafe { std::slice::from_raw_parts(bindings, count) }
    };
    let mut buttons = Vec::new();
    let mut axes = Vec::new();
    let mut vectors = Vec::new();
    for b in bindings {
        match (kind, b.kind) {
            (0, 0) => buttons.push(ButtonBinding::Key(key(b.first)?)),
            (0, 1) => buttons.push(ButtonBinding::Mouse(mouse(b.first)?)),
            (0, 2) => buttons.push(ButtonBinding::Gamepad {
                pad: pad(b.pad)?,
                button: button(b.first)?,
            }),
            (1, 3) => axes.push(AxisBinding::Keys {
                negative: key(b.first)?,
                positive: key(b.second)?,
            }),
            (1, 4) => axes.push(AxisBinding::Gamepad {
                pad: pad(b.pad)?,
                axis: axis(b.first)?,
                deadzone: b.deadzone,
            }),
            (2, 5) => vectors.push(Axis2Binding::Keys {
                left: key(b.first)?,
                right: key(b.second)?,
                up: key(b.third)?,
                down: key(b.fourth)?,
            }),
            (2, 6) => vectors.push(Axis2Binding::Gamepad {
                pad: pad(b.pad)?,
                stick: match b.first {
                    0 => GamepadStick::Left,
                    1 => GamepadStick::Right,
                    _ => return Err(invalid("unknown stick")),
                },
                deadzone: b.deadzone,
            }),
            _ => return Err(invalid("source does not match action kind")),
        }
    }
    let action = match kind {
        0 => Action::Button(buttons),
        1 => Action::Axis(axes),
        2 => Action::Axis2(vectors),
        _ => return Err(invalid("unknown action kind")),
    };
    Ok(action)
}

/// Remove a named action.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_action_map_remove(std::ptr::null_mut(), c"jump".as_ptr()) } as u32, 0);
/// ```
/// # Safety
/// Handle must be live or null, name readable NUL-terminated and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_remove(map: *mut PonkoActionMap, name: *const c_char) -> PonkoStatus {
    // SAFETY: requirements are forwarded to the helpers.
    boundary(|| unsafe {
        map_mut(map)?.remove(text(name)?);
        Ok(())
    })
}

/// Load Rust-compatible JSON. Errors leave the map unchanged.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_action_map_load(std::ptr::null_mut(), c"{}".as_ptr()) } as u32, 0);
/// ```
/// # Safety
/// Handle must be live or null, JSON readable NUL-terminated and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_load(map: *mut PonkoActionMap, json: *const c_char) -> PonkoStatus {
    boundary(|| {
        // SAFETY: caller supplies live, non-overlapping handle and string.
        let (map, json) = unsafe { (map_mut(map)?, text(json)?) };
        *map = ActionMap::from_json(json).map_err(|e| invalid_owned(e.to_string()))?;
        Ok(())
    })
}

/// Save JSON. `out_len` excludes NUL. With capacity <= length, writes an empty
/// string (if capacity > 0); allocate length+1 and call again. Null buffer is
/// accepted only with capacity 0.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_action_map_save(std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut 0) } as u32, 0);
/// ```
/// # Safety
/// Handle must be live or null; buffer writable for capacity bytes and out_len
/// writable, with no overlapping pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_action_map_save(
    map: *mut PonkoActionMap,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        if out_len.is_null() || (buffer.is_null() && capacity != 0) {
            return Err(invalid("output is null"));
        }
        // SAFETY: caller supplies live handle.
        let json = unsafe { map_mut(map)? }
            .to_json()
            .map_err(|e| invalid_owned(e.to_string()))?;
        // SAFETY: validated non-null outputs; caller supplies stated capacity.
        unsafe {
            out_len.write(json.len());
            if json.len() < capacity {
                std::ptr::copy_nonoverlapping(json.as_ptr(), buffer.cast(), json.len());
                buffer.add(json.len()).write(0);
            } else if capacity != 0 {
                buffer.write(0);
            }
        }
        Ok(())
    })
}

/// Install a copy of the bindings; rebinding generates no edges. Map remains owned by caller.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_set_actions(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, map live, exclusively accessed and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_actions(ctx: *mut PonkoCtx, map: *mut PonkoActionMap) -> PonkoStatus {
    // SAFETY: caller supplies callback context and independent map.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.input.set_actions(map_mut(map)?.clone());
            Ok(())
        })
    })
}

/// Copy current bindings into a map.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_actions(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 0);
/// ```
/// # Safety
/// Context and destination must be live or null, exclusively accessed, non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_actions(ctx: *mut PonkoCtx, map: *mut PonkoActionMap) -> PonkoStatus {
    // SAFETY: caller supplies callback context and independent map.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            *map_mut(map)? = c.input.actions().clone();
            Ok(())
        })
    })
}

/// Feed manual input: kind 0 key, 1 mouse, 2 gamepad button, 3 gamepad axis,
/// 4 focus lost, 5 gamepad disconnected. Digital value is 0 released, nonzero held.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_feed_input(std::ptr::null_mut(), 0, 0, 0, 1.0) } as u32, 0);
/// ```
/// # Safety
/// Context must be live in its callback, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_feed_input(
    ctx: *mut PonkoCtx,
    kind: u32,
    slot: u32,
    input: u32,
    value: f32,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.input.feed(input_event(kind, slot, input, value)?);
            Ok(())
        })
    })
}

/// The event `ponko_ctx_feed_input` (and a scenario's input step) describes.
pub(crate) fn input_event(kind: u32, slot: u32, input: u32, value: f32) -> Result<InputEvent, Failure> {
    Ok(match kind {
        0 => InputEvent::Key(key(input)?, value != 0.0),
        1 => InputEvent::Mouse(mouse(input)?, value != 0.0),
        2 => InputEvent::GamepadButton(pad(slot)?, button(input)?, value != 0.0),
        3 => InputEvent::GamepadAxis(pad(slot)?, axis(input)?, value),
        4 => InputEvent::FocusLost,
        5 => InputEvent::GamepadDisconnected(pad(slot)?),
        _ => return Err(invalid("unknown input event kind")),
    })
}

/// Query a named action's down. Unknown/wrong-kind actions read zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_down(std::ptr::null_mut(), c"move".as_ptr(), &mut false) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, name NUL-terminated, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_down(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    value: *mut bool,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context, string and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            out(value, c.input.action_down(name))
        })
    })
}

/// Query a named action's pressed. Unknown/wrong-kind actions read zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_pressed(std::ptr::null_mut(), c"move".as_ptr(), &mut false) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, name NUL-terminated, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_pressed(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    value: *mut bool,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context, string and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            out(value, c.input.action_pressed(name))
        })
    })
}

/// Query a named action's released. Unknown/wrong-kind actions read zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_released(std::ptr::null_mut(), c"move".as_ptr(), &mut false) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, name NUL-terminated, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_released(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    value: *mut bool,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context, string and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            out(value, c.input.action_released(name))
        })
    })
}

/// Query a named action's axis. Unknown/wrong-kind actions read zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_axis(std::ptr::null_mut(), c"move".as_ptr(), &mut 0.0) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, name NUL-terminated, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_axis(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    value: *mut f32,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context, string and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            out(value, c.input.action_axis(name))
        })
    })
}

/// Query a named action's axis2. Unknown/wrong-kind actions read zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_axis2(std::ptr::null_mut(), c"move".as_ptr(), &mut ponko_ffi::PonkoVec2 { x: 0.0, y: 0.0 }) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, name NUL-terminated, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_axis2(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    value: *mut PonkoVec2,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context, string and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            out(value, to_c(c.input.action_axis2(name)))
        })
    })
}

/// An unknown action name: `PONKO_NOT_FOUND`, naming the known actions.
fn unknown_action(e: &ponko::actions::UnknownAction) -> Failure {
    engine_error(ponko::prelude::Error::new(
        ponko::prelude::ErrorKind::NotFound,
        e.to_string(),
    ))
}

/// The action id of no action (Rust's `ActionId::NONE`): queries with it read
/// false/zero. `ponko_ctx_action_id` gives it for an unknown name.
pub const PONKO_ACTION_NONE: u32 = u32::MAX;

/// Resolves action `name` once into an id for the `ponko_ctx_action_*_id`
/// queries, which then skip the name lookup (Rust's `ctx.input.action_id`).
/// Resolve in setup, after `ponko_ctx_set_actions`; rebinding later keeps the
/// id valid. An id belongs to the game it came from. An unknown name writes
/// `PONKO_ACTION_NONE` and returns `PONKO_NOT_FOUND`, with the known names in
/// `ponko_last_error()`.
/// ```
/// let mut id = 0u32;
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_id(std::ptr::null_mut(), c"jump".as_ptr(), &mut id) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, name NUL-terminated, output writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_id(ctx: *mut PonkoCtx, name: *const c_char, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: caller supplies callback context, string and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            if out_id.is_null() {
                return Err(invalid("output is null"));
            }
            match c.input.try_action_id(name) {
                Ok(id) => out(out_id, id.to_bits()),
                Err(e) => {
                    out(out_id, PONKO_ACTION_NONE)?;
                    Err(unknown_action(&e))
                }
            }
        })
    })
}

/// Resolves `count` names at once into `out_ids` (Rust's
/// `ctx.input.action_ids`): every unknown one becomes `PONKO_ACTION_NONE`,
/// and the call returns `PONKO_NOT_FOUND` naming the first.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_ids(std::ptr::null_mut(), std::ptr::null(), 0, std::ptr::null_mut()) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null; `names` holds `count` NUL-terminated
/// strings and `out_ids` room for `count` ids (either may be null with count 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_ids(
    ctx: *mut PonkoCtx,
    names: *const *const c_char,
    count: usize,
    out_ids: *mut u32,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context, string array and output array.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if count != 0 && (names.is_null() || out_ids.is_null()) {
                return Err(invalid("names or output is null"));
            }
            let mut first_unknown = None;
            for i in 0..count {
                let name = text(*names.add(i))?;
                let id = match c.input.try_action_id(name) {
                    Ok(id) => id.to_bits(),
                    Err(e) => {
                        first_unknown.get_or_insert(e);
                        PONKO_ACTION_NONE
                    }
                };
                out_ids.add(i).write(id);
            }
            match first_unknown {
                None => Ok(()),
                Some(e) => Err(unknown_action(&e)),
            }
        })
    })
}

/// `ponko_ctx_action_down` by an id from `ponko_ctx_action_id`. An id that
/// names no action (`PONKO_ACTION_NONE`) reads zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_down_id(std::ptr::null_mut(), 0, &mut false) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_down_id(ctx: *mut PonkoCtx, action: u32, value: *mut bool) -> PonkoStatus {
    // SAFETY: caller supplies callback context and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let id = ActionId::from_bits(action);
            out(value, c.input.action_down(id))
        })
    })
}

/// `ponko_ctx_action_pressed` by an id from `ponko_ctx_action_id`. An id that
/// names no action (`PONKO_ACTION_NONE`) reads zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_pressed_id(std::ptr::null_mut(), 0, &mut false) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_pressed_id(ctx: *mut PonkoCtx, action: u32, value: *mut bool) -> PonkoStatus {
    // SAFETY: caller supplies callback context and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let id = ActionId::from_bits(action);
            out(value, c.input.action_pressed(id))
        })
    })
}

/// `ponko_ctx_action_released` by an id from `ponko_ctx_action_id`. An id that
/// names no action (`PONKO_ACTION_NONE`) reads zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_released_id(std::ptr::null_mut(), 0, &mut false) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_released_id(
    ctx: *mut PonkoCtx,
    action: u32,
    value: *mut bool,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let id = ActionId::from_bits(action);
            out(value, c.input.action_released(id))
        })
    })
}

/// `ponko_ctx_action_axis` by an id from `ponko_ctx_action_id`. An id that
/// names no action (`PONKO_ACTION_NONE`) reads zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_axis_id(std::ptr::null_mut(), 0, &mut 0.0) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_axis_id(ctx: *mut PonkoCtx, action: u32, value: *mut f32) -> PonkoStatus {
    // SAFETY: caller supplies callback context and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let id = ActionId::from_bits(action);
            out(value, c.input.action_axis(id))
        })
    })
}

/// `ponko_ctx_action_axis2` by an id from `ponko_ctx_action_id`. An id that
/// names no action (`PONKO_ACTION_NONE`) reads zero.
/// ```
/// assert_ne!(unsafe { ponko_ffi::ponko_ctx_action_axis2_id(std::ptr::null_mut(), 0, &mut ponko_ffi::PonkoVec2 { x: 0.0, y: 0.0 }) } as u32, 0);
/// ```
/// # Safety
/// Context must be live or null, output writable and non-overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_action_axis2_id(
    ctx: *mut PonkoCtx,
    action: u32,
    value: *mut PonkoVec2,
) -> PonkoStatus {
    // SAFETY: caller supplies callback context and independent output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let id = ActionId::from_bits(action);
            out(value, to_c(c.input.action_axis2(id)))
        })
    })
}
