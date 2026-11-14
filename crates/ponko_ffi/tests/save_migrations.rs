//! Save migrations through the C API: the game's save version, each kind of
//! change, C conversions (with their user pointers freed), saves from a newer
//! game refused with their error kind, and checking a folder of old saves.
mod common;

use ponko_ffi::PonkoFieldType::*;
use ponko_ffi::*;
use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::{null, null_mut};

use PonkoStatus::*;
use common::{last_error, run_ctx};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Health {
    now: f32,
    max: f32,
}

/// A world with `Health` reflected.
fn world() -> (*mut PonkoWorld, u32) {
    let w = ponko_world_new();
    (w, reflect_health(w))
}

fn reflect_health(w: *mut PonkoWorld) -> u32 {
    let mut id = 0;
    let fields = [
        PonkoField {
            name: c"now".as_ptr(),
            kind: PONKO_FIELD_F32 as u32,
            offset: std::mem::offset_of!(Health, now),
        },
        PonkoField {
            name: c"max".as_ptr(),
            kind: PONKO_FIELD_F32 as u32,
            offset: std::mem::offset_of!(Health, max),
        },
    ];
    unsafe {
        assert_eq!(
            ponko_component_register(
                w,
                c"Health".as_ptr(),
                size_of::<Health>(),
                align_of::<Health>(),
                &mut id
            ),
            PONKO_OK
        );
        assert_eq!(ponko_component_reflect(w, id, fields.as_ptr(), fields.len()), PONKO_OK);
    }
    id
}

fn get(w: *mut PonkoWorld, id: u32, e: u64) -> Health {
    let mut h = Health::default();
    let status = unsafe { ponko_get(w, e, id, (&mut h as *mut Health).cast(), size_of::<Health>()) };
    assert_eq!(status, PONKO_OK, "{}", last_error());
    h
}

/// A world save of one entity (id 0) with `components`.
fn save_of(game_version: Option<u32>, components: serde_json::Value) -> CString {
    let mut save = serde_json::json!({
        "format": "ponko-world",
        "version": 1,
        "entities": [{ "entity": 0, "components": components }],
    });
    if let Some(v) = game_version {
        save["game_version"] = v.into();
    }
    CString::new(save.to_string()).unwrap()
}

fn migrate(w: *mut PonkoWorld, save: &CStr) -> Result<serde_json::Value, (PonkoStatus, PonkoErrorKind, String)> {
    let mut len = 0;
    unsafe {
        let status = ponko_world_migrate_save(w, save.as_ptr(), null_mut(), 0, &mut len);
        if status != PONKO_OK {
            return Err((status, ponko_last_error_kind(), last_error()));
        }
        let mut buf = vec![0u8; len + 1];
        assert_eq!(
            ponko_world_migrate_save(w, save.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), &mut len),
            PONKO_OK
        );
        Ok(serde_json::from_slice(&buf[..len]).unwrap())
    }
}

/// A conversion that divides a number by four, rounding up (hit points to
/// hearts), keeping its text in `user` (a `Box<Converter>`).
struct Converter {
    text: CString,
    freed: *const Cell<u32>,
}
unsafe extern "C" fn quarter(user: *mut c_void, json: *const c_char, out: *mut *const c_char) -> u32 {
    let me = unsafe { &mut *user.cast::<Converter>() };
    let text = unsafe { CStr::from_ptr(json) }.to_str().unwrap();
    match text.parse::<f64>() {
        Ok(points) => {
            me.text = CString::new(format!("{}", (points / 4.0).ceil())).unwrap();
            unsafe { *out = me.text.as_ptr() };
            PONKO_OK as u32
        }
        Err(_) => {
            me.text = CString::new(format!("{text} is not a number")).unwrap();
            unsafe { *out = me.text.as_ptr() };
            PONKO_INVALID_ARGUMENT as u32
        }
    }
}
/// Wraps a value in `{"coins": value}`.
unsafe extern "C" fn wrap_coins(user: *mut c_void, json: *const c_char, out: *mut *const c_char) -> u32 {
    let me = unsafe { &mut *user.cast::<Converter>() };
    let text = unsafe { CStr::from_ptr(json) }.to_str().unwrap();
    me.text = CString::new(format!("{{\"coins\":{text}}}")).unwrap();
    unsafe { *out = me.text.as_ptr() };
    PONKO_OK as u32
}
/// Adds a `Boss` component to every entity (the components object).
unsafe extern "C" fn add_boss(user: *mut c_void, json: *const c_char, out: *mut *const c_char) -> u32 {
    let me = unsafe { &mut *user.cast::<Converter>() };
    let mut components: serde_json::Value =
        serde_json::from_str(unsafe { CStr::from_ptr(json) }.to_str().unwrap()).unwrap();
    components["Boss"] = serde_json::json!({});
    me.text = CString::new(components.to_string()).unwrap();
    unsafe { *out = me.text.as_ptr() };
    PONKO_OK as u32
}
/// Marks the whole save.
unsafe extern "C" fn mark_save(user: *mut c_void, json: *const c_char, out: *mut *const c_char) -> u32 {
    let me = unsafe { &mut *user.cast::<Converter>() };
    let mut save: serde_json::Value = serde_json::from_str(unsafe { CStr::from_ptr(json) }.to_str().unwrap()).unwrap();
    save["resources"]["Marked"] = true.into();
    me.text = CString::new(save.to_string()).unwrap();
    unsafe { *out = me.text.as_ptr() };
    PONKO_OK as u32
}
/// Fails without a message.
unsafe extern "C" fn refuse(_: *mut c_void, _: *const c_char, _: *mut *const c_char) -> u32 {
    PONKO_NOT_FOUND as u32
}
unsafe extern "C" fn free_converter(user: *mut c_void) {
    let me = unsafe { Box::from_raw(user.cast::<Converter>()) };
    let freed = unsafe { &*me.freed };
    freed.set(freed.get() + 1);
}
fn converter(freed: &Cell<u32>) -> *mut c_void {
    Box::into_raw(Box::new(Converter {
        text: CString::default(),
        freed,
    }))
    .cast()
}

#[test]
fn a_component_rename_plus_a_field_conversion_loads_from_a_save_written_before_both() {
    let freed = Cell::new(0);
    let (w, id) = world();
    unsafe {
        assert_eq!(ponko_world_set_save_version(w, 1), PONKO_OK);
        assert_eq!(
            ponko_save_migration_rename_component(w, 0, c"Hp".as_ptr(), c"Health".as_ptr()),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_convert_field(
                w,
                0,
                c"Health".as_ptr(),
                c"max".as_ptr(),
                Some(quarter),
                converter(&freed),
                Some(free_converter)
            ),
            PONKO_OK
        );
        let old = save_of(None, serde_json::json!({ "Hp": { "now": 3.0, "max": 13.0 } }));
        assert_eq!(ponko_world_load(w, old.as_ptr()), PONKO_OK, "{}", last_error());
        assert_eq!(get(w, id, 0), Health { now: 3.0, max: 4.0 });
        let mut v = 0;
        assert_eq!(ponko_world_save_version(w, &mut v), PONKO_OK);
        assert_eq!(v, 1);
        assert_eq!(freed.get(), 0, "the world keeps the conversion");
        ponko_world_free(w);
    }
    assert_eq!(freed.get(), 1, "freed once with the world");
}

#[test]
fn every_declarative_change_runs_in_order_across_chained_versions() {
    let w = ponko_world_new();
    unsafe {
        assert_eq!(ponko_world_set_save_version(w, 3), PONKO_OK);
        // 0 -> 1
        assert_eq!(
            ponko_save_migration_remove_component(w, 0, c"Legacy".as_ptr()),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_split_component(
                w,
                0,
                c"Stats".as_ptr(),
                c"[[\"Regen\", [\"hp_regen\"]], [\"Speed\", [\"speed\"]]]".as_ptr()
            ),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_rename_resource(w, 0, c"Gold".as_ptr(), c"Purse".as_ptr()),
            PONKO_OK
        );
        assert_eq!(ponko_save_migration_remove_resource(w, 0, c"Old".as_ptr()), PONKO_OK);
        // 1 -> 2
        let merged = [c"Speed".as_ptr(), c"Jump".as_ptr()];
        assert_eq!(
            ponko_save_migration_merge_components(w, 1, merged.as_ptr(), merged.len(), c"Movement".as_ptr()),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_add_field(w, 1, c"Movement".as_ptr(), c"drag".as_ptr(), c"0.5".as_ptr()),
            PONKO_OK
        );
        // 2 -> 3
        assert_eq!(
            ponko_save_migration_rename_field(w, 2, c"Regen".as_ptr(), c"hp_regen".as_ptr(), c"per_second".as_ptr()),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_remove_field(w, 2, c"Movement".as_ptr(), c"jump".as_ptr()),
            PONKO_OK
        );
    }
    let mut save: serde_json::Value = serde_json::from_slice(
        save_of(
            None,
            serde_json::json!({ "Stats": { "hp_regen": 1, "speed": 2, "armor": 3 }, "Jump": { "jump": 4 } }),
        )
        .as_bytes(),
    )
    .unwrap();
    save["entities"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "entity": 1, "components": { "Legacy": {} } }));
    save["resources"] = serde_json::json!({ "Gold": 7, "Old": 1 });
    let out = migrate(w, &CString::new(save.to_string()).unwrap()).unwrap();
    assert_eq!(
        out["entities"],
        serde_json::json!([{ "entity": 0, "components": {
            "Regen": { "per_second": 1 },
            "Movement": { "speed": 2, "drag": 0.5 },
        } }])
    );
    assert_eq!(out["resources"], serde_json::json!({ "Purse": 7 }));
    assert_eq!(out["game_version"], 3);
    // A version-2 save runs only the last step.
    let v2 = save_of(
        Some(2),
        serde_json::json!({ "Movement": { "speed": 1, "jump": 2, "drag": 9 } }),
    );
    let out = migrate(w, &v2).unwrap();
    assert_eq!(
        out["entities"][0]["components"],
        serde_json::json!({ "Movement": { "speed": 1, "drag": 9 } })
    );
    unsafe { ponko_world_free(w) };
}

#[test]
fn c_conversions_change_components_resources_entities_and_the_whole_save() {
    let freed = Cell::new(0);
    let w = ponko_world_new();
    unsafe {
        assert_eq!(ponko_world_set_save_version(w, 1), PONKO_OK);
        let f = Some(free_converter as unsafe extern "C" fn(*mut c_void));
        assert_eq!(
            ponko_save_migration_convert_component(w, 0, c"Level".as_ptr(), Some(quarter), converter(&freed), f),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_convert_resource(w, 0, c"Purse".as_ptr(), Some(wrap_coins), converter(&freed), f),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_change_entities(w, 0, Some(add_boss), converter(&freed), f),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_change_save(w, 0, Some(mark_save), converter(&freed), f),
            PONKO_OK
        );
    }
    let mut save: serde_json::Value =
        serde_json::from_slice(save_of(None, serde_json::json!({ "Level": 9 })).as_bytes()).unwrap();
    save["resources"] = serde_json::json!({ "Purse": 7 });
    let out = migrate(w, &CString::new(save.to_string()).unwrap()).unwrap();
    assert_eq!(
        out["entities"][0]["components"],
        serde_json::json!({ "Level": 3, "Boss": {} })
    );
    assert_eq!(
        out["resources"],
        serde_json::json!({ "Purse": { "coins": 7 }, "Marked": true })
    );
    unsafe { ponko_world_free(w) };
    assert_eq!(freed.get(), 4);
}

#[test]
fn a_failing_c_conversion_fails_the_load_with_its_message_and_leaves_the_world_unchanged() {
    let freed = Cell::new(0);
    let (w, id) = world();
    unsafe {
        assert_eq!(ponko_world_set_save_version(w, 1), PONKO_OK);
        assert_eq!(
            ponko_save_migration_convert_field(
                w,
                0,
                c"Health".as_ptr(),
                c"max".as_ptr(),
                Some(quarter),
                converter(&freed),
                Some(free_converter)
            ),
            PONKO_OK
        );
        assert_eq!(
            ponko_save_migration_convert_field(
                w,
                0,
                c"Health".as_ptr(),
                c"now".as_ptr(),
                Some(refuse),
                null_mut(),
                None
            ),
            PONKO_OK
        );
        let mut e = 0;
        let h = Health { now: 1.0, max: 2.0 };
        let value = PonkoValue {
            component: id,
            data: (&h as *const Health).cast(),
            size: size_of::<Health>(),
        };
        assert_eq!(ponko_spawn(w, &value, 1, &mut e), PONKO_OK);

        let bad = save_of(None, serde_json::json!({ "Health": { "now": 1.0, "max": "lots" } }));
        assert_eq!(ponko_world_load(w, bad.as_ptr()), PONKO_INVALID_ARGUMENT);
        assert_eq!(ponko_last_error_kind(), PonkoErrorKind::PONKO_ERROR_JSON);
        let message = last_error();
        assert!(
            message.contains("game save version 0 to 1") && message.contains("\"lots\" is not a number"),
            "{message}"
        );
        let refused = save_of(None, serde_json::json!({ "Health": { "now": 1.0, "max": 8.0 } }));
        assert_eq!(ponko_world_load(w, refused.as_ptr()), PONKO_INVALID_ARGUMENT);
        assert!(
            last_error().contains("the C conversion returned status 2"),
            "{}",
            last_error()
        );
        assert_eq!(get(w, id, e), h);
        ponko_world_free(w);
    }
}

#[test]
fn a_save_from_a_newer_game_is_refused_as_unsupported() {
    let (w, _) = world();
    unsafe {
        assert_eq!(ponko_world_set_save_version(w, 2), PONKO_OK);
        let newer = save_of(Some(5), serde_json::json!({}));
        assert_eq!(ponko_world_load(w, newer.as_ptr()), PONKO_INVALID_ARGUMENT);
        assert_eq!(ponko_last_error_kind(), PonkoErrorKind::PONKO_ERROR_UNSUPPORTED);
        assert!(
            last_error().contains("game save version 5; this build reads up to 2"),
            "{}",
            last_error()
        );
        let (status, kind, _) = migrate(w, &newer).unwrap_err();
        assert_eq!(
            (status, kind),
            (PONKO_INVALID_ARGUMENT, PonkoErrorKind::PONKO_ERROR_UNSUPPORTED)
        );
        ponko_world_free(w);
    }
}

#[test]
fn steps_need_a_version_above_them_and_the_version_cannot_drop_below_a_step() {
    let freed = Cell::new(0);
    let w = ponko_world_new();
    unsafe {
        assert_eq!(
            ponko_save_migration_rename_component(w, 0, c"A".as_ptr(), c"B".as_ptr()),
            PONKO_INVALID_ARGUMENT
        );
        assert!(
            last_error().contains("call ponko_world_set_save_version first"),
            "{}",
            last_error()
        );
        // A refused conversion is freed at once.
        assert_eq!(
            ponko_save_migration_convert_field(
                w,
                0,
                c"A".as_ptr(),
                c"x".as_ptr(),
                Some(quarter),
                converter(&freed),
                Some(free_converter)
            ),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(freed.get(), 1);
        assert_eq!(
            ponko_save_migration_change_save(w, 0, None, converter(&freed), Some(free_converter)),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(freed.get(), 2, "a null conversion frees user too");
        assert_eq!(ponko_world_set_save_version(w, 2), PONKO_OK);
        assert_eq!(ponko_save_migration_remove_component(w, 1, c"A".as_ptr()), PONKO_OK);
        assert_eq!(ponko_world_set_save_version(w, 1), PONKO_INVALID_ARGUMENT);
        assert!(
            last_error().contains("a step from save version 1 is registered"),
            "{}",
            last_error()
        );
        assert_eq!(ponko_world_set_save_version(w, 4), PONKO_OK);
        let mut v = 0;
        assert_eq!(ponko_world_save_version(w, &mut v), PONKO_OK);
        assert_eq!(v, 4);
        // Bad arguments.
        assert_eq!(
            ponko_save_migration_add_field(w, 0, c"A".as_ptr(), c"x".as_ptr(), c"{nope".as_ptr()),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(
            ponko_save_migration_split_component(w, 0, c"A".as_ptr(), c"{}".as_ptr()),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(
            ponko_save_migration_merge_components(w, 0, null(), 1, c"A".as_ptr()),
            PONKO_INVALID_ARGUMENT
        );
        ponko_world_free(w);
    }
}

#[test]
fn saves_record_the_game_version() {
    let (w, _) = world();
    unsafe {
        assert_eq!(ponko_world_set_save_version(w, 7), PONKO_OK);
        let mut len = 0;
        assert_eq!(ponko_world_save(w, null_mut(), 0, &mut len), PONKO_OK);
        let mut buf = vec![0u8; len + 1];
        assert_eq!(
            ponko_world_save(w, buf.as_mut_ptr().cast(), buf.len(), &mut len),
            PONKO_OK
        );
        let save: serde_json::Value = serde_json::from_slice(&buf[..len]).unwrap();
        assert_eq!(save["game_version"], 7);
        ponko_world_free(w);
    }
}

#[test]
fn check_old_saves_loads_a_folder_through_the_migrations() {
    let dir = std::env::temp_dir().join(format!("ponko-ffi-old-saves-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("v0.json"),
        save_of(None, serde_json::json!({ "Hp": { "now": 1.0, "max": 8.0 } })).as_bytes(),
    )
    .unwrap();
    std::fs::write(
        dir.join("v1.json"),
        save_of(Some(1), serde_json::json!({ "Health": { "now": 1.0, "max": 2.0 } })).as_bytes(),
    )
    .unwrap();
    let folder = CString::new(dir.to_str().unwrap()).unwrap();
    let empty = CString::new(dir.join("none").to_str().unwrap()).unwrap();
    run_ctx(|ctx| unsafe {
        let w = ponko_ctx_world(ctx);
        let id = reflect_health(w);
        assert_eq!(ponko_world_set_save_version(w, 1), PONKO_OK);
        assert_eq!(
            ponko_save_migration_rename_component(w, 0, c"Hp".as_ptr(), c"Health".as_ptr()),
            PONKO_OK
        );
        let mut n = 0;
        assert_eq!(
            ponko_ctx_check_old_saves(ctx, folder.as_ptr(), &mut n),
            PONKO_OK,
            "{}",
            last_error()
        );
        assert_eq!(n, 2);
        assert_eq!(get(w, id, 0), Health { now: 1.0, max: 2.0 });
        assert_eq!(ponko_ctx_check_old_saves(ctx, empty.as_ptr(), &mut n), PONKO_NOT_FOUND);
        assert_eq!(
            ponko_ctx_check_old_saves(ctx, folder.as_ptr(), null_mut()),
            PONKO_INVALID_ARGUMENT
        );
    });
    let _ = std::fs::remove_dir_all(dir);
}
