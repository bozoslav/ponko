//! Reading an action map back from C (`ActionMap::get`, `ActionMap::iter`)
//! and changing one installed action (`Input::set_action`).
mod common;

use PonkoStatus::*;
use common::run_ctx;
use ponko_ffi::*;
use std::ffi::CStr;
use std::ptr::{null, null_mut};

fn binding(kind: PonkoActionSource, first: u32) -> PonkoActionBinding {
    PonkoActionBinding {
        kind: kind as u32,
        pad: 0,
        first,
        second: 0,
        third: 0,
        fourth: 0,
        deadzone: 0.0,
    }
}

fn key(k: PonkoKey) -> u32 {
    k as u32
}

/// The bindings of `name`, with its kind.
unsafe fn get(map: *const PonkoActionMap, name: &CStr) -> (u32, Vec<PonkoActionBinding>) {
    unsafe {
        let (mut kind, mut n) = (9, 0);
        assert_eq!(
            ponko_action_map_get(map, name.as_ptr(), &mut kind, null_mut(), 0, &mut n),
            PONKO_OK
        );
        let mut list = vec![binding(PonkoActionSource::PONKO_ACTION_KEY, 0); n];
        assert_eq!(
            ponko_action_map_get(map, name.as_ptr(), &mut kind, list.as_mut_ptr(), list.len(), &mut n),
            PONKO_OK
        );
        (kind, list)
    }
}

unsafe fn name_at(map: *const PonkoActionMap, index: usize) -> String {
    unsafe {
        let mut len = 0;
        assert_eq!(ponko_action_map_name(map, index, null_mut(), 0, &mut len), PONKO_OK);
        let mut buffer = vec![0 as std::ffi::c_char; len + 1];
        assert_eq!(
            ponko_action_map_name(map, index, buffer.as_mut_ptr(), buffer.len(), &mut len),
            PONKO_OK
        );
        CStr::from_ptr(buffer.as_ptr()).to_str().unwrap().to_owned()
    }
}

#[test]
fn a_map_reads_back_every_action_and_source_it_was_given() {
    use PonkoActionSource::*;
    unsafe {
        let map = ponko_action_map_new();
        let jump = [
            binding(PONKO_ACTION_KEY, key(PonkoKey::PONKO_KEY_SPACE)),
            binding(PONKO_ACTION_MOUSE, PonkoMouseButton::PONKO_MOUSE_RIGHT as u32),
            PonkoActionBinding {
                pad: 1,
                ..binding(
                    PONKO_ACTION_GAMEPAD_BUTTON,
                    PonkoGamepadButton::PONKO_GAMEPAD_SOUTH as u32,
                )
            },
        ];
        let throttle = [
            PonkoActionBinding {
                second: key(PonkoKey::PONKO_KEY_W),
                ..binding(PONKO_ACTION_KEYS, key(PonkoKey::PONKO_KEY_S))
            },
            PonkoActionBinding {
                deadzone: 0.25,
                ..binding(
                    PONKO_ACTION_GAMEPAD_AXIS,
                    PonkoGamepadAxis::PONKO_GAMEPAD_RIGHT_TRIGGER_AXIS as u32,
                )
            },
        ];
        let movement = [
            PonkoActionBinding {
                second: key(PonkoKey::PONKO_KEY_D),
                third: key(PonkoKey::PONKO_KEY_W),
                fourth: key(PonkoKey::PONKO_KEY_S),
                ..binding(PONKO_ACTION_KEYS2, key(PonkoKey::PONKO_KEY_A))
            },
            PonkoActionBinding {
                deadzone: 0.1,
                ..binding(PONKO_ACTION_GAMEPAD_STICK, 1)
            },
        ];
        // Set out of name order: reading back is by name.
        assert_eq!(
            ponko_action_map_set(map, c"throttle".as_ptr(), 1, throttle.as_ptr(), 2),
            PONKO_OK
        );
        assert_eq!(
            ponko_action_map_set(map, c"jump".as_ptr(), 0, jump.as_ptr(), 3),
            PONKO_OK
        );
        assert_eq!(
            ponko_action_map_set(map, c"move".as_ptr(), 2, movement.as_ptr(), 2),
            PONKO_OK
        );

        let mut n = 0;
        assert_eq!(ponko_action_map_count(map, &mut n), PONKO_OK);
        assert_eq!(n, 3);
        let names: Vec<String> = (0..n).map(|i| name_at(map, i)).collect();
        assert_eq!(names, ["jump", "move", "throttle"]);

        let as_tuple = |b: &PonkoActionBinding| (b.kind, b.pad, b.first, b.second, b.third, b.fourth, b.deadzone);
        for (name, kind, given) in [
            (c"jump", 0, &jump[..]),
            (c"throttle", 1, &throttle[..]),
            (c"move", 2, &movement[..]),
        ] {
            let (got_kind, got) = get(map, name);
            assert_eq!(got_kind, kind, "{name:?}");
            assert_eq!(
                got.iter().map(as_tuple).collect::<Vec<_>>(),
                given.iter().map(as_tuple).collect::<Vec<_>>(),
                "{name:?}"
            );
        }

        // A short buffer gets the first sources and the full count.
        let (mut kind, mut count) = (0, 0);
        let mut one = [binding(PONKO_ACTION_KEY, 0)];
        assert_eq!(
            ponko_action_map_get(map, c"jump".as_ptr(), &mut kind, one.as_mut_ptr(), 1, &mut count),
            PONKO_OK
        );
        assert_eq!((count, one[0].first), (3, key(PonkoKey::PONKO_KEY_SPACE)));

        // Unknown names and indexes are not found; missing outputs are invalid.
        assert_eq!(
            ponko_action_map_get(map, c"fire".as_ptr(), &mut kind, null_mut(), 0, &mut count),
            PONKO_NOT_FOUND
        );
        assert_eq!(
            ponko_action_map_name(map, 3, null_mut(), 0, &mut count),
            PONKO_NOT_FOUND
        );
        assert_eq!(
            ponko_action_map_get(map, c"jump".as_ptr(), null_mut(), null_mut(), 0, &mut count),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(ponko_action_map_count(null(), &mut count), PONKO_INVALID_ARGUMENT);
        ponko_action_map_free(map);
    }
}

#[test]
fn set_action_changes_one_action_and_keeps_its_id_and_held_state_without_an_edge() {
    use PonkoActionSource::*;
    run_ctx(|ctx| unsafe {
        let map = ponko_action_map_new();
        let space = [binding(PONKO_ACTION_KEY, key(PonkoKey::PONKO_KEY_SPACE))];
        let k = [binding(PONKO_ACTION_KEY, key(PonkoKey::PONKO_KEY_K))];
        assert_eq!(
            ponko_action_map_set(map, c"jump".as_ptr(), 0, space.as_ptr(), 1),
            PONKO_OK
        );
        assert_eq!(ponko_action_map_set(map, c"fire".as_ptr(), 0, k.as_ptr(), 1), PONKO_OK);
        assert_eq!(ponko_ctx_set_actions(ctx, map), PONKO_OK);
        let mut before = 0;
        assert_eq!(ponko_ctx_action_id(ctx, c"jump".as_ptr(), &mut before), PONKO_OK);

        // J is held before it is bound to jump.
        assert_eq!(
            ponko_ctx_feed_input(ctx, 0, 0, key(PonkoKey::PONKO_KEY_J), 1.0),
            PONKO_OK
        );
        let both = [space[0], binding(PONKO_ACTION_KEY, key(PonkoKey::PONKO_KEY_J))];
        assert_eq!(
            ponko_ctx_set_action(ctx, c"jump".as_ptr(), 0, both.as_ptr(), 2),
            PONKO_OK
        );

        let mut after = 0;
        assert_eq!(ponko_ctx_action_id(ctx, c"jump".as_ptr(), &mut after), PONKO_OK);
        assert_eq!(after, before, "the name keeps its id");
        let (mut down, mut pressed) = (false, true);
        assert_eq!(ponko_ctx_action_down(ctx, c"jump".as_ptr(), &mut down), PONKO_OK);
        assert_eq!(ponko_ctx_action_pressed(ctx, c"jump".as_ptr(), &mut pressed), PONKO_OK);
        assert!(down, "the held key now holds jump");
        assert!(!pressed, "rebinding fires no pressed edge");

        // The installed map has the new jump and the old fire.
        let installed = ponko_action_map_new();
        assert_eq!(ponko_ctx_actions(ctx, installed), PONKO_OK);
        assert_eq!(get(installed, c"jump").1.len(), 2);
        assert_eq!(get(installed, c"fire").1[0].first, key(PonkoKey::PONKO_KEY_K));

        // Invalid bindings change nothing.
        let wrong = [binding(PONKO_ACTION_KEYS, 0)];
        assert_eq!(
            ponko_ctx_set_action(ctx, c"jump".as_ptr(), 0, wrong.as_ptr(), 1),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(ponko_ctx_actions(ctx, installed), PONKO_OK);
        assert_eq!(get(installed, c"jump").1.len(), 2);
        ponko_action_map_free(installed);
        ponko_action_map_free(map);
    });
}
