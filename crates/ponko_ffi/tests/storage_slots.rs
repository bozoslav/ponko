//! Save slots and the storage's state from C (`Storage::world_slots`,
//! `Storage::delete_world`, `Storage::is_persistent`, `Storage::is_saving`).
mod common;

use PonkoStatus::*;
use common::{last_error, new_game, run_game};
use ponko_ffi::*;
use std::ffi::{CStr, CString};
use std::ptr::null_mut;

unsafe fn slots(ctx: *mut PonkoCtx) -> Vec<String> {
    unsafe {
        let mut len = 0;
        assert_eq!(
            ponko_ctx_storage_world_slots(ctx, null_mut(), 0, &mut len),
            PONKO_OK,
            "{}",
            last_error()
        );
        let mut buffer = vec![0 as std::ffi::c_char; len + 1];
        assert_eq!(
            ponko_ctx_storage_world_slots(ctx, buffer.as_mut_ptr(), buffer.len(), &mut len),
            PONKO_OK
        );
        let text = CStr::from_ptr(buffer.as_ptr()).to_str().unwrap();
        text.split('\n').filter(|s| !s.is_empty()).map(String::from).collect()
    }
}

#[test]
fn save_slots_are_listed_deleted_and_the_folder_persists() {
    let dir = std::env::temp_dir().join(format!("ponko-ffi-storage-slots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir = CString::new(dir.to_str().unwrap()).unwrap();
    let game = new_game();
    // SAFETY: a live game and a NUL-terminated path.
    assert_eq!(unsafe { ponko_game_data_dir(game, dir.as_ptr()) }, PONKO_OK);
    let world = run_game(game, 0, |ctx| unsafe {
        assert!(slots(ctx).is_empty());
        assert_eq!(ponko_ctx_save_world(ctx, c"b".as_ptr()), PONKO_OK);
        assert_eq!(ponko_ctx_save_world(ctx, c"a".as_ptr()), PONKO_OK);
        // A stored file that isn't a slot is not listed.
        assert_eq!(
            ponko_ctx_storage_write(ctx, c"settings.json".as_ptr(), b"{}".as_ptr(), 2),
            PONKO_OK
        );
        assert_eq!(slots(ctx), ["a", "b"]);

        assert_eq!(ponko_ctx_delete_world(ctx, c"a".as_ptr()), PONKO_OK);
        assert_eq!(slots(ctx), ["b"]);
        assert_eq!(
            ponko_ctx_delete_world(ctx, c"a".as_ptr()),
            PONKO_OK,
            "a missing slot is fine"
        );
        assert_eq!(ponko_ctx_delete_world(ctx, c"../up".as_ptr()), PONKO_INVALID_ARGUMENT);

        let (mut persistent, mut saving) = (false, true);
        assert_eq!(ponko_ctx_storage_is_persistent(ctx, &mut persistent), PONKO_OK);
        assert_eq!(ponko_ctx_storage_is_saving(ctx, &mut saving), PONKO_OK);
        assert!(persistent, "a folder outlives the run");
        assert!(!saving, "desktop writes are done when they return");
        assert_eq!(ponko_ctx_storage_is_saving(ctx, null_mut()), PONKO_INVALID_ARGUMENT);
        let mut len = 0;
        assert_eq!(
            ponko_ctx_storage_world_slots(ctx, null_mut(), 4, &mut len),
            PONKO_INVALID_ARGUMENT
        );
    });
    // SAFETY: the world the run returned, freed once.
    unsafe { ponko_world_free(world) };
}

#[test]
fn a_game_without_storage_is_not_persistent() {
    let game = new_game();
    let world = run_game(game, 0, |ctx| unsafe {
        let mut persistent = true;
        assert_eq!(ponko_ctx_storage_is_persistent(ctx, &mut persistent), PONKO_OK);
        assert!(!persistent);
    });
    // SAFETY: the world the run returned, freed once.
    unsafe { ponko_world_free(world) };
}
