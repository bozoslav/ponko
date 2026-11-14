//! The ECS diagnostics snapshot from C (`Ctx::ecs_diagnostics`).
mod common;

use PonkoStatus::*;
use common::{last_error, new_game, run_game};
use ponko_ffi::*;
use std::ffi::{CStr, c_void};
use std::ptr::{null, null_mut};

/// A system that takes the snapshot into the `String` at `user`.
unsafe extern "C" fn report(ctx: *mut PonkoCtx, user: *mut c_void) {
    unsafe {
        let out = &mut *user.cast::<String>();
        let mut len = 0;
        assert_eq!(
            ponko_ctx_ecs_diagnostics(ctx, null_mut(), 0, &mut len),
            PONKO_OK,
            "{}",
            last_error()
        );
        let mut buffer = vec![0 as std::ffi::c_char; len + 1];
        assert_eq!(
            ponko_ctx_ecs_diagnostics(ctx, buffer.as_mut_ptr(), buffer.len(), &mut len),
            PONKO_OK
        );
        *out = CStr::from_ptr(buffer.as_ptr()).to_str().unwrap().to_owned();

        // A short buffer gets an empty string and the length.
        let mut short = [1 as std::ffi::c_char; 4];
        assert_eq!(
            ponko_ctx_ecs_diagnostics(ctx, short.as_mut_ptr(), 4, &mut len),
            PONKO_OK
        );
        assert_eq!(short[0], 0);
        assert_eq!(
            ponko_ctx_ecs_diagnostics(ctx, null_mut(), 4, &mut len),
            PONKO_INVALID_ARGUMENT
        );
    }
}

#[test]
fn diagnostics_list_entities_and_systems_as_json() {
    let mut json = String::new();
    let game = new_game();
    // SAFETY: a live game; `json` outlives the run.
    assert_eq!(
        unsafe { ponko_game_sim_system(game, c"report".as_ptr(), 1, Some(report), (&raw mut json).cast()) },
        PONKO_OK
    );
    let world = run_game(game, 1, |ctx| unsafe {
        let w = ponko_ctx_world(ctx);
        for _ in 0..3 {
            let mut e = 0;
            assert_eq!(ponko_spawn(w, null(), 0, &mut e), PONKO_OK);
        }
    });
    // SAFETY: the world the run returned, freed once.
    unsafe { ponko_world_free(world) };
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["entities"], 3);
    let systems = json["systems"].as_array().unwrap();
    let mine = systems
        .iter()
        .find(|s| s["name"] == "report")
        .unwrap_or_else(|| panic!("the C system is listed: {systems:?}"));
    assert_eq!(mine["phase"], "AfterPhysics");
}
