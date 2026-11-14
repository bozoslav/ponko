//! The engine's built-in systems from C: adding one back in another phase
//! (`ponko::systems` with `Game::add`) and their declarations
//! (`systems::step_physics_access` and the others).
mod common;

use PonkoStatus::*;
use common::last_error;
use ponko_ffi::*;
use std::ffi::CStr;
use std::ptr::null_mut;

unsafe fn plan(game: *mut PonkoGame) -> String {
    unsafe {
        let mut len = 0;
        assert_eq!(ponko_game_schedule_plan(game, null_mut(), 0, &mut len), PONKO_OK);
        let mut buffer = vec![0 as std::ffi::c_char; len + 1];
        assert_eq!(
            ponko_game_schedule_plan(game, buffer.as_mut_ptr(), buffer.len(), &mut len),
            PONKO_OK
        );
        CStr::from_ptr(buffer.as_ptr()).to_str().unwrap().to_owned()
    }
}

/// The lines of `phase`'s part of a schedule plan: its systems, up to the
/// next phase's name (a line that isn't indented).
fn phase_part(plan: &str, phase: &str) -> String {
    plan.lines()
        .skip_while(|l| *l != phase)
        .skip(1)
        .take_while(|l| l.starts_with(' '))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_removed_builtin_comes_back_in_the_phase_it_is_added_to() {
    unsafe {
        let game = ponko_game_new(c"builtins".as_ptr(), 1, 1);
        assert!(phase_part(&plan(game), "Draw").contains("draw_tilemaps"));
        assert_eq!(ponko_game_remove(game, PONKO_SYSTEM_DRAW_TILEMAPS), PONKO_OK);
        assert!(!plan(game).contains("draw_tilemaps"));

        let late = PonkoPhase::PONKO_PHASE_LATE as u32;
        assert_eq!(
            ponko_game_add_builtin(game, late, PONKO_SYSTEM_DRAW_TILEMAPS),
            PONKO_OK,
            "{}",
            last_error()
        );
        let now = plan(game);
        assert!(phase_part(&now, "Late").contains("draw_tilemaps"), "{now}");
        assert!(!phase_part(&now, "Draw").contains("draw_tilemaps"), "{now}");

        // Its id takes it out again, wherever it is.
        assert_eq!(ponko_game_remove(game, PONKO_SYSTEM_DRAW_TILEMAPS), PONKO_OK);
        assert!(!plan(game).contains("draw_tilemaps"));

        // A built-in the engine never adds by itself.
        assert!(!plan(game).contains("follow_attachments"));
        assert_eq!(
            ponko_game_add_builtin(game, late, PONKO_SYSTEM_FOLLOW_ATTACHMENTS),
            PONKO_OK
        );
        assert!(phase_part(&plan(game), "Late").contains("follow_attachments"));

        assert_eq!(ponko_game_add_builtin(game, late, 999), PONKO_INVALID_ARGUMENT);
        assert!(last_error().contains("PONKO_SYSTEM_"), "{}", last_error());
        assert_eq!(
            ponko_game_add_builtin(game, 99, PONKO_SYSTEM_DRAW_SPRITES),
            PONKO_INVALID_ARGUMENT
        );
        ponko_game_free(game);
    }
}

#[test]
fn a_readded_physics_step_keeps_its_declaration() {
    unsafe {
        let game = ponko_game_new(c"builtins".as_ptr(), 1, 1);
        assert_eq!(ponko_game_physics(game, PonkoVec2 { x: 0.0, y: 0.0 }), PONKO_OK);
        assert_eq!(ponko_game_remove(game, PONKO_SYSTEM_PHYSICS), PONKO_OK);
        let physics = PonkoPhase::PONKO_PHASE_PHYSICS as u32;
        assert_eq!(ponko_game_add_builtin(game, physics, PONKO_SYSTEM_PHYSICS), PONKO_OK);
        let now = plan(game);
        let part = phase_part(&now, "Physics");
        assert!(part.contains("step_physics"), "{now}");
        assert!(
            !part.contains("undeclared"),
            "the step runs with its declaration: {now}"
        );
        // The control: a built-in without a declaration says so.
        assert!(phase_part(&now, "Draw").contains("undeclared"), "{now}");
        ponko_game_free(game);
    }
}

#[test]
fn builtin_declarations_say_what_the_steps_touch() {
    unsafe {
        let nothing = ponko_access_new();
        let count = |a: *const PonkoAccess, b: *const PonkoAccess| {
            let mut n = 99;
            assert_eq!(ponko_access_conflicts(a, b, false, &mut n), PONKO_OK);
            n
        };
        for id in [
            PONKO_SYSTEM_PHYSICS,
            PONKO_SYSTEM_PHYSICS3D,
            PONKO_SYSTEM_INTERPOLATE_PHYSICS3D,
            PONKO_SYSTEM_STEP_NAVMESH3D,
        ] {
            let mut step = null_mut();
            assert_eq!(ponko_builtin_access(id, &mut step), PONKO_OK, "{}", last_error());
            assert!(!step.is_null());
            assert_eq!(
                count(step, nothing),
                0,
                "built-in {id} could run beside a system that uses nothing"
            );
            assert!(count(step, step) > 0, "built-in {id} writes what it reads");
            ponko_access_free(step);
        }
        // A copy declares the same.
        let mut navmesh = null_mut();
        assert_eq!(
            ponko_builtin_access(PONKO_SYSTEM_STEP_NAVMESH3D, &mut navmesh),
            PONKO_OK
        );
        let mut copy = null_mut();
        assert_eq!(ponko_access_clone(navmesh, &mut copy), PONKO_OK);
        assert_eq!(count(copy, nothing), 0);
        assert_eq!(count(copy, navmesh), count(navmesh, navmesh));
        ponko_access_free(copy);
        let mut empty = null_mut();
        assert_eq!(ponko_access_clone(std::ptr::null(), &mut empty), PONKO_OK);
        assert_eq!(count(empty, nothing), 0);
        ponko_access_free(empty);
        assert_eq!(ponko_access_clone(navmesh, null_mut()), PONKO_INVALID_ARGUMENT);
        ponko_access_free(navmesh);

        // Built-ins without a declaration take the whole ctx.
        let mut draw = null_mut();
        assert_eq!(ponko_builtin_access(PONKO_SYSTEM_DRAW_SPRITES, &mut draw), PONKO_OK);
        assert!(count(draw, nothing) > 0);
        ponko_access_free(draw);

        let mut unknown = std::ptr::dangling_mut::<PonkoAccess>();
        assert_eq!(ponko_builtin_access(999, &mut unknown), PONKO_INVALID_ARGUMENT);
        assert!(unknown.is_null());
        assert_eq!(
            ponko_builtin_access(PONKO_SYSTEM_PHYSICS, null_mut()),
            PONKO_INVALID_ARGUMENT
        );
        ponko_access_free(nothing);
    }
}
