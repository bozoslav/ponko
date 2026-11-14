//! What world saves leave out, through the C API: a spawned level is not
//! saved but its prefab objects are, entities can be left out by hand, and
//! the level spawns again without the objects a save holds.
use ponko_ffi::*;
use std::ffi::{CString, c_void};
use std::ptr::null_mut;

use PonkoStatus::*;

fn fixture(name: &str) -> CString {
    CString::new(format!(
        "{}/../../tests/fixtures/tilemaps/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The ids of the entities in a world save.
fn saved(w: *mut PonkoWorld) -> Vec<u64> {
    let mut len = 0;
    unsafe { assert_eq!(ponko_world_save(w, null_mut(), 0, &mut len), PONKO_OK) };
    let mut buf = vec![0u8; len + 1];
    unsafe {
        assert_eq!(
            ponko_world_save(w, buf.as_mut_ptr().cast(), buf.len(), &mut len),
            PONKO_OK
        )
    };
    let json: serde_json::Value = serde_json::from_slice(&buf[..len]).unwrap();
    let mut ids: Vec<u64> = json["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["entity"].as_u64().unwrap())
        .collect();
    ids.sort_unstable();
    ids
}

fn entities(w: *mut PonkoWorld, root: u64) -> Vec<u64> {
    let mut n = 0;
    unsafe {
        assert_eq!(ponko_level_entities(w, root, null_mut(), 0, &mut n), PONKO_OK);
        let mut list = vec![0; n];
        assert_eq!(ponko_level_entities(w, root, list.as_mut_ptr(), n, &mut n), PONKO_OK);
        list
    }
}

fn is_saved(w: *mut PonkoWorld, e: u64) -> bool {
    let mut saved = false;
    unsafe { assert_eq!(ponko_world_is_saved(w, e, &mut saved), PONKO_OK) };
    saved
}

struct Run {
    path: CString,
    prefab: CString,
    done: std::cell::Cell<bool>,
}

unsafe extern "C" fn play(ctx: *mut PonkoCtx, user: *mut c_void) {
    unsafe {
        let run = &*user.cast::<Run>();
        let class = c"Coin";
        let classes = [class.as_ptr()];
        let paths = [run.prefab.as_ptr()];
        let options = PonkoLevelOptions {
            prefab_classes: classes.as_ptr(),
            prefab_paths: paths.as_ptr(),
            prefab_count: 1,
            solid: std::ptr::null(),
            solid_count: 0,
            level: std::ptr::null(),
            no_colliders: false,
            z: 0.0,
        };
        let at = PonkoVec2 { x: 0.0, y: 0.0 };
        let w = ponko_ctx_world(ctx);
        let mut root = 0;
        assert_eq!(
            ponko_ctx_spawn_level(ctx, run.path.as_ptr(), at, &options, &mut root),
            PONKO_OK
        );
        let level = entities(w, root);
        // Only the two coins, spawned as prefabs, are saved.
        let mut coins: Vec<u64> = level.iter().copied().filter(|&e| is_saved(w, e)).collect();
        coins.sort_unstable();
        assert_eq!(coins.len(), 2);
        assert!(!is_saved(w, root));
        assert_eq!(saved(w), coins);

        // Left out by hand, and back.
        assert_eq!(ponko_world_set_saved(w, coins[0], false), PONKO_OK);
        assert_eq!(saved(w), [coins[1]]);
        assert_eq!(ponko_world_set_saved(w, coins[0], true), PONKO_OK);
        assert_eq!(saved(w), coins);
        assert_eq!(ponko_world_set_saved(w, u64::MAX, true), PONKO_NOT_FOUND);
        let mut flag = false;
        assert_eq!(ponko_world_is_saved(w, u64::MAX, &mut flag), PONKO_NOT_FOUND);
        assert_eq!(ponko_world_is_saved(w, coins[0], null_mut()), PONKO_INVALID_ARGUMENT);

        // The level again, as after a load: everything but the coins.
        let mut again = 0;
        assert_eq!(
            ponko_ctx_spawn_level_without_saved_objects(ctx, run.path.as_ptr(), at, &options, &mut again),
            PONKO_OK
        );
        assert_eq!(entities(w, again).len(), level.len() - 2);
        assert_eq!(saved(w), coins, "nothing more to save");
        assert_eq!(
            ponko_ctx_spawn_level_without_saved_objects(ctx, run.path.as_ptr(), at, &options, null_mut()),
            PONKO_INVALID_ARGUMENT
        );
        run.done.set(true);
    }
}

#[test]
fn a_level_is_left_out_of_saves_but_its_prefab_objects_are_not() {
    let dir = std::env::temp_dir().join(format!("ponko-ffi-level-saves-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let prefab = dir.join("coin.json");
    std::fs::write(
        &prefab,
        r#"{"format": "ponko-prefab", "version": 1, "components": {"Transform2D": {}}}"#,
    )
    .unwrap();
    let run = Run {
        path: fixture("level.tmx"),
        prefab: CString::new(prefab.to_str().unwrap()).unwrap(),
        done: std::cell::Cell::new(false),
    };
    unsafe {
        let game = ponko_game_new(c"levels".as_ptr(), 64, 64);
        let mut world = null_mut();
        let user = (&run as *const Run).cast_mut().cast();
        assert_eq!(ponko_game_run_headless(game, 0, Some(play), user, &mut world), PONKO_OK);
        ponko_world_free(world);
    }
    assert!(run.done.get(), "the setup ran to its end");
    let _ = std::fs::remove_dir_all(dir);
}
