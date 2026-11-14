//! Save migrations from C (Rust's `ponko::reflect::SaveMigrations`): the
//! game's own save version, written into world saves, and the changes that
//! bring a save of an older version up to date before it loads.
//!
//! Each `ponko_save_migration_*` call adds one change to the step from save
//! version `from_version` to `from_version + 1`; changes of one step run in
//! the order they were added. Set the game's version first with
//! `ponko_world_set_save_version`.
use std::ffi::{CStr, CString, c_char, c_void};

use ponko::prelude::Error;
use ponko::reflect::{Migration, ReflectError, SaveMigrations};
use ponko_ecs::World;
use serde_json::Value;

use crate::asset_sources::PonkoUserFreeFn;
use crate::ctx::{PonkoCtx, ctx_access, out, text};
use crate::{Failure, PonkoStatus, PonkoWorld, access, boundary, engine_error, invalid, invalid_owned, write_text};

/// Converts one saved value for a save migration: `json` is the value's
/// JSON (valid during the call). Return `PONKO_OK` (a `PonkoStatus` value,
/// as `uint32_t`) with `*out_json` set to the new value's JSON, NUL-terminated
/// and valid until the function is called again (the engine copies it right
/// away). Any other status fails the load; `*out_json` may then point to a
/// message saying why (or stay null). Runs inside the load (or
/// `ponko_world_migrate_save`) on its thread; it must not call into the
/// engine.
/// ```
/// let none: ponko_ffi::PonkoJsonConvertFn = None;
/// assert!(none.is_none());
/// ```
pub type PonkoJsonConvertFn =
    Option<unsafe extern "C" fn(user: *mut c_void, json: *const c_char, out_json: *mut *const c_char) -> u32>;

/// A C conversion with its user pointer, kept by the world's migrations.
struct Convert {
    f: unsafe extern "C" fn(*mut c_void, *const c_char, *mut *const c_char) -> u32,
    user: *mut c_void,
    free: PonkoUserFreeFn,
}

// SAFETY: the world calls the conversion only inside a load or migrate call,
// which holds the world's handle exclusively, so two calls never overlap.
// The caller of the `ponko_save_migration_*` function promised `user` stays
// usable until `free` is called, on whichever thread drops the world.
unsafe impl Send for Convert {}
// SAFETY: as for `Send`: calls never overlap.
unsafe impl Sync for Convert {}

impl Drop for Convert {
    fn drop(&mut self) {
        if let Some(free) = self.free {
            // SAFETY: the caller handed `user` over to be freed once, here.
            unsafe { free(self.user) };
        }
    }
}

impl Convert {
    fn call(&self, value: Value) -> Result<Value, String> {
        // JSON text escapes control characters, so it has no NUL inside.
        let input = CString::new(value.to_string()).map_err(|e| e.to_string())?;
        let mut result: *const c_char = std::ptr::null();
        // SAFETY: `input` outlives the call; `result` is writable; the caller
        // promised `f` and `user` stay valid.
        let status = unsafe { (self.f)(self.user, input.as_ptr(), &mut result) };
        let returned = if result.is_null() {
            None
        } else {
            // SAFETY: the conversion promised a NUL-terminated string.
            Some(unsafe { CStr::from_ptr(result) }.to_string_lossy().into_owned())
        };
        if status != PonkoStatus::PONKO_OK as u32 {
            return Err(match returned {
                Some(message) if !message.is_empty() => message,
                _ => format!("the C conversion returned status {status}"),
            });
        }
        let json = returned.ok_or("the C conversion returned PONKO_OK without a value")?;
        serde_json::from_str(&json).map_err(|e| format!("the C conversion returned bad JSON: {e}"))
    }
}

/// Takes the conversion, calling `free` right away if `f` is null.
fn convert(f: PonkoJsonConvertFn, user: *mut c_void, free: PonkoUserFreeFn) -> Result<Convert, Failure> {
    match f {
        Some(f) => Ok(Convert { f, user, free }),
        None => {
            if let Some(free) = free {
                // SAFETY: ownership of `user` passed to this call.
                unsafe { free(user) };
            }
            Err(invalid("convert is null"))
        }
    }
}

/// Adds `migration` as part of the step from `from`, checked against the
/// game's version.
fn add(w: &mut World, from: u32, migration: Migration) -> Result<(), Failure> {
    let version = w.get_resource::<SaveMigrations>().map_or(0, SaveMigrations::version);
    if from >= version {
        return Err(invalid_owned(format!(
            "a step from save version {from} needs a game save version above {from} (it is {version}): call ponko_world_set_save_version first"
        )));
    }
    w.resource_mut::<SaveMigrations>().add_step(from, migration);
    Ok(())
}

/// Builds the migration (with the strings checked) and adds it.
///
/// # Safety
/// `world` null or live.
unsafe fn step(world: *mut PonkoWorld, from: u32, build: impl FnOnce() -> Result<Migration, Failure>) -> PonkoStatus {
    boundary(|| {
        let migration = build()?;
        // SAFETY: access checks the handle and guards the world.
        unsafe { access(world, |w, _| add(w, from, migration)) }
    })
}

/// A load or migrate failure, with the error kind of the Rust error
/// (`PONKO_ERROR_UNSUPPORTED` for a save from a newer game version).
pub(crate) fn reflect_failure(e: ReflectError) -> Failure {
    invalid_owned(Error::from(e))
}

/// Sets the game's save version: what world saves record as
/// `"game_version"`, and the newest version a load accepts (saves without
/// one are version 0). Raise it, and add the changes from the old version,
/// whenever a change to the game's components would break older saves.
/// Fails without a change if a step from `version` or later is registered.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe {
///     assert_eq!(ponko_world_set_save_version(w, 2), PonkoStatus::PONKO_OK);
///     let mut v = 0;
///     assert_eq!(ponko_world_save_version(w, &mut v), PonkoStatus::PONKO_OK);
///     assert_eq!(v, 2);
///     ponko_world_free(w);
/// }
/// ```
/// # Safety
/// `world` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_world_set_save_version(world: *mut PonkoWorld, version: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        access(world, |w, _| {
            let Some(migrations) = w.get_resource_mut::<SaveMigrations>() else {
                w.insert_resource(SaveMigrations::new(version));
                return Ok(());
            };
            if let Some(newest) = migrations.newest_step().filter(|&s| s >= version) {
                return Err(invalid_owned(format!(
                    "a step from save version {newest} is registered, so the version must be above {newest}"
                )));
            }
            migrations.set_version(version);
            Ok(())
        })
    })
}

/// Writes the game's save version (0 before `ponko_world_set_save_version`).
/// ```
/// let mut v = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_world_save_version(std::ptr::null_mut(), &mut v) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `out_version` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_world_save_version(world: *mut PonkoWorld, out_version: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        access(world, |w, _| {
            out(
                out_version,
                w.get_resource::<SaveMigrations>().map_or(0, SaveMigrations::version),
            )
        })
    })
}

/// Component `from` is called `to` since save version `from_version + 1`.
/// An entity that has both fails the load.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_rename_component(std::ptr::null_mut(), 0, c"Hp".as_ptr(), c"Health".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; strings NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_rename_component(
    world: *mut PonkoWorld,
    from_version: u32,
    from: *const c_char,
    to: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            Ok(Migration::new().rename_component(text(from)?, text(to)?))
        })
    }
}

/// Component `name` is gone: dropped from every entity; an entity left with
/// no components is dropped from the save.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_remove_component(std::ptr::null_mut(), 0, c"Old".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `name` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_remove_component(
    world: *mut PonkoWorld,
    from_version: u32,
    name: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            Ok(Migration::new().remove_component(text(name)?))
        })
    }
}

/// Field `from` of component `component` is called `to`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_rename_field(std::ptr::null_mut(), 0, c"Health".as_ptr(), c"current".as_ptr(), c"now".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; strings NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_rename_field(
    world: *mut PonkoWorld,
    from_version: u32,
    component: *const c_char,
    from: *const c_char,
    to: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            Ok(Migration::new().rename_field(text(component)?, text(from)?, text(to)?))
        })
    }
}

/// Component `component` has a new field `field`: older saves get the value
/// `value_json` (JSON text) for it; components that have it keep theirs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_add_field(std::ptr::null_mut(), 0, c"Health".as_ptr(), c"shield".as_ptr(), c"0".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; strings NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_add_field(
    world: *mut PonkoWorld,
    from_version: u32,
    component: *const c_char,
    field: *const c_char,
    value_json: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            let value: Value = serde_json::from_str(text(value_json)?)
                .map_err(|e| invalid_owned(format!("value_json is not JSON: {e}")))?;
            Ok(Migration::new().add_field(text(component)?, text(field)?, value))
        })
    }
}

/// Component `component` lost its field `field`: dropped from older saves.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_remove_field(std::ptr::null_mut(), 0, c"Movement".as_ptr(), c"drag".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; strings NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_remove_field(
    world: *mut PonkoWorld,
    from_version: u32,
    component: *const c_char,
    field: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            Ok(Migration::new().remove_field(text(component)?, text(field)?))
        })
    }
}

/// Component `from` became several. `parts_json` lists them, each with the
/// fields of `from` it gets: `[["Regen", ["hp_regen"]], ["Speed", ["speed"]]]`.
/// `from` is removed (a part may keep its name); fields no part lists are
/// dropped. An entity that already has a part fails the load.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_split_component(std::ptr::null_mut(), 0, c"Stats".as_ptr(), c"[]".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; strings NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_split_component(
    world: *mut PonkoWorld,
    from_version: u32,
    from: *const c_char,
    parts_json: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            let parts: Vec<(String, Vec<String>)> = serde_json::from_str(text(parts_json)?)
                .map_err(|e| invalid_owned(format!("parts_json is not a list of [name, [fields]] pairs: {e}")))?;
            let fields: Vec<Vec<&str>> = parts
                .iter()
                .map(|(_, f)| f.iter().map(String::as_str).collect())
                .collect();
            let parts: Vec<(&str, &[&str])> = parts
                .iter()
                .zip(&fields)
                .map(|((name, _), f)| (name.as_str(), f.as_slice()))
                .collect();
            Ok(Migration::new().split_component(text(from)?, &parts))
        })
    }
}

/// Components `from` (`count` names) became one, `into`, with the fields of
/// all of them; the sources are removed. Two with the same field fail the
/// load.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_merge_components(std::ptr::null_mut(), 0, std::ptr::null(), 0, c"Movement".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `from` holds `count` NUL-terminated strings (null
/// with `count` 0); `into` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_merge_components(
    world: *mut PonkoWorld,
    from_version: u32,
    from: *const *const c_char,
    count: usize,
    into: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            if from.is_null() && count != 0 {
                return Err(invalid("from is null"));
            }
            let names = if count == 0 {
                Vec::new()
            } else {
                // SAFETY: caller supplies `count` readable pointers.
                std::slice::from_raw_parts(from, count)
                    .iter()
                    .map(|&name| text(name))
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(Migration::new().merge_components(&names, text(into)?))
        })
    }
}

/// Reflected resource `from` is called `to`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_rename_resource(std::ptr::null_mut(), 0, c"Gold".as_ptr(), c"Purse".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; strings NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_rename_resource(
    world: *mut PonkoWorld,
    from_version: u32,
    from: *const c_char,
    to: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            Ok(Migration::new().rename_resource(text(from)?, text(to)?))
        })
    }
}

/// Reflected resource `name` is gone.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_remove_resource(std::ptr::null_mut(), 0, c"Old".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `name` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_remove_resource(
    world: *mut PonkoWorld,
    from_version: u32,
    name: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(
            world,
            from_version,
            || Ok(Migration::new().remove_resource(text(name)?)),
        )
    }
}

/// Field `field` of component `component` changed its type or meaning:
/// `convert` turns each saved value into the new one. The world keeps
/// `user` and calls `free(user)` once when it drops the migration (`free`
/// may be null); if this call fails, `free` runs before it returns.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_convert_field(std::ptr::null_mut(), 0, c"Health".as_ptr(), c"max".as_ptr(), None, std::ptr::null_mut(), None) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; strings NUL-terminated; `convert` and `user`
/// usable until `free` is called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_convert_field(
    world: *mut PonkoWorld,
    from_version: u32,
    component: *const c_char,
    field: *const c_char,
    convert_fn: PonkoJsonConvertFn,
    user: *mut c_void,
    free: PonkoUserFreeFn,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            let c = convert(convert_fn, user, free)?;
            Ok(Migration::new().convert_field(text(component)?, text(field)?, move |v| c.call(v)))
        })
    }
}

/// Replaces each value of component `component` with what `convert` makes
/// of it (for a value that isn't an object, or several fields at once).
/// `user` and `free` as for `ponko_save_migration_convert_field`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_convert_component(std::ptr::null_mut(), 0, c"Team".as_ptr(), None, std::ptr::null_mut(), None) } as u32, 1);
/// ```
/// # Safety
/// As `ponko_save_migration_convert_field`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_convert_component(
    world: *mut PonkoWorld,
    from_version: u32,
    component: *const c_char,
    convert_fn: PonkoJsonConvertFn,
    user: *mut c_void,
    free: PonkoUserFreeFn,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            let c = convert(convert_fn, user, free)?;
            Ok(Migration::new().convert_component(text(component)?, move |v| c.call(v)))
        })
    }
}

/// Replaces the saved value of resource `resource` with what `convert`
/// makes of it. `user` and `free` as for
/// `ponko_save_migration_convert_field`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_convert_resource(std::ptr::null_mut(), 0, c"Purse".as_ptr(), None, std::ptr::null_mut(), None) } as u32, 1);
/// ```
/// # Safety
/// As `ponko_save_migration_convert_field`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_convert_resource(
    world: *mut PonkoWorld,
    from_version: u32,
    resource: *const c_char,
    convert_fn: PonkoJsonConvertFn,
    user: *mut c_void,
    free: PonkoUserFreeFn,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            let c = convert(convert_fn, user, free)?;
            Ok(Migration::new().convert_resource(text(resource)?, move |v| c.call(v)))
        })
    }
}

/// `convert` gets each saved entity's components (a JSON object keyed by
/// component name) and returns them changed, also an object: for what the
/// other changes don't cover. An entity left with no components is dropped.
/// `user` and `free` as for `ponko_save_migration_convert_field`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_change_entities(std::ptr::null_mut(), 0, None, std::ptr::null_mut(), None) } as u32, 1);
/// ```
/// # Safety
/// As `ponko_save_migration_convert_field`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_change_entities(
    world: *mut PonkoWorld,
    from_version: u32,
    convert_fn: PonkoJsonConvertFn,
    user: *mut c_void,
    free: PonkoUserFreeFn,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            let c = convert(convert_fn, user, free)?;
            Ok(Migration::new().entities(move |components| {
                match c.call(Value::Object(std::mem::take(components)))? {
                    Value::Object(changed) => {
                        *components = changed;
                        Ok(())
                    }
                    other => Err(format!("the components became {other}, not an object")),
                }
            }))
        })
    }
}

/// `convert` gets the whole save (the JSON of `ponko_world_save`) and
/// returns it changed: the last resort, for example to add an entity older
/// saves lack. `user` and `free` as for `ponko_save_migration_convert_field`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_save_migration_change_save(std::ptr::null_mut(), 0, None, std::ptr::null_mut(), None) } as u32, 1);
/// ```
/// # Safety
/// As `ponko_save_migration_convert_field`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_save_migration_change_save(
    world: *mut PonkoWorld,
    from_version: u32,
    convert_fn: PonkoJsonConvertFn,
    user: *mut c_void,
    free: PonkoUserFreeFn,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        step(world, from_version, || {
            let c = convert(convert_fn, user, free)?;
            Ok(Migration::new().save(move |save| {
                *save = c.call(save.take())?;
                Ok(())
            }))
        })
    }
}

/// Writes `json` (a world save) as the game's version would have written it:
/// the steps from its `"game_version"` up applied. The loads do this by
/// themselves; this is for looking at or keeping a migrated save. A save
/// from a newer game version fails with the error kind
/// `PONKO_ERROR_UNSUPPORTED`, a failing step with `PONKO_ERROR_JSON`.
/// Call with a null buffer to learn the length.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_world_migrate_save(std::ptr::null_mut(), c"{}".as_ptr(), std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `json` NUL-terminated; `buffer` holds `capacity`
/// bytes (null with 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_world_migrate_save(
    world: *mut PonkoWorld,
    json: *const c_char,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let save: Value = serde_json::from_str(text(json)?).map_err(|e| invalid_owned(format!("not JSON: {e}")))?;
        let migrated = access(world, |w, _| {
            let result = match w.get_resource::<SaveMigrations>() {
                Some(m) => m.migrate(&save),
                None => SaveMigrations::default().migrate(&save),
            };
            result.map(|v| v.to_string()).map_err(reflect_failure)
        })?;
        write_text(buffer, capacity, out_len, &migrated)
    })
}

/// Loads every world save (`*.json`) in the folder `folder` (a file-system
/// path, relative to the working directory) as `ponko_ctx_load_world` does,
/// migrations included, then saves and loads it again: the test that the
/// game's migrations still bring its old saves up to date. Writes how many
/// loaded; the world holds the last one. Fails with `PONKO_NOT_FOUND` for a
/// folder without `.json` files, otherwise with every file that failed and
/// why in `ponko_last_error()`.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_check_old_saves(std::ptr::null_mut(), c"old-saves".as_ptr(), &mut n) } as u32, 1);
/// ```
/// # Safety
/// Live ctx; `folder` NUL-terminated; `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_check_old_saves(
    ctx: *mut PonkoCtx,
    folder: *const c_char,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_count.is_null() {
            return Err(invalid("output is null"));
        }
        let folder = text(folder)?;
        let count = ctx_access(ctx, |c| c.check_old_saves(folder).map_err(engine_error))?;
        out(out_count, count)
    })
}
