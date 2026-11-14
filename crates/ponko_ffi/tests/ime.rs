//! IME composition through the C API (#410): feeding compositions, the
//! IME requests, the retained UI's preedit and caret area, and scenario
//! steps, in headless runs.
use ponko_ffi::*;
use std::ffi::{CStr, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};

use PonkoStatus::*;

type Ctx = *mut PonkoCtx;

struct Scenario {
    body: unsafe fn(Ctx),
    failure: Option<String>,
}

unsafe extern "C" fn trampoline(ctx: Ctx, user: *mut c_void) {
    let scenario = unsafe { &mut *user.cast::<Scenario>() };
    let body = scenario.body;
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| unsafe { body(ctx) })) {
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()));
        scenario.failure = Some(message.unwrap_or_else(|| "panicked".into()));
    }
}

/// Runs `body` once as a sim system of a headless game.
fn run(body: unsafe fn(Ctx)) {
    let mut scenario = Scenario { body, failure: None };
    unsafe {
        let game = ponko_game_new(c"ime".as_ptr(), 1, 1);
        let user = (&mut scenario as *mut Scenario).cast();
        assert_eq!(
            ponko_game_sim_system(game, c"s".as_ptr(), 1, Some(trampoline), user),
            PONKO_OK
        );
        let mut world = null_mut();
        assert_eq!(ponko_game_run_headless(game, 1, None, null_mut(), &mut world), PONKO_OK);
        ponko_world_free(world);
    }
    if let Some(failure) = scenario.failure {
        panic!("{failure}");
    }
}

fn event(kind: PonkoImeEventKind, text: &CStr, cursor: Option<(usize, usize)>) -> PonkoImeEvent {
    PonkoImeEvent {
        kind: kind as u32,
        text: text.as_ptr(),
        has_cursor: cursor.is_some(),
        cursor_start: cursor.map_or(0, |c| c.0),
        cursor_end: cursor.map_or(0, |c| c.1),
    }
}

/// Reads a string through a `(buffer, capacity, out_len)` call.
unsafe fn read(call: impl Fn(*mut c_char, usize, *mut usize) -> PonkoStatus) -> String {
    let mut len = 0;
    assert_eq!(call(null_mut(), 0, &mut len), PONKO_OK);
    let mut buffer = vec![0u8; len + 1];
    assert_eq!(call(buffer.as_mut_ptr().cast(), buffer.len(), &mut len), PONKO_OK);
    unsafe { CStr::from_ptr(buffer.as_ptr().cast()) }
        .to_string_lossy()
        .into_owned()
}

unsafe fn input_side(ctx: Ctx) {
    use PonkoImeEventKind::*;
    unsafe {
        let mut on = true;
        assert_eq!(ponko_ctx_ime_allowed(ctx, &mut on), PONKO_OK);
        assert!(!on, "IME is off until something asks for it");
        assert_eq!(ponko_ctx_set_ime_allowed(ctx, true), PONKO_OK);
        assert_eq!(ponko_ctx_ime_allowed(ctx, &mut on), PONKO_OK);
        assert!(on);

        let (mut has, mut area) = (true, PonkoRect::default());
        assert_eq!(ponko_ctx_ime_cursor_area(ctx, &mut has, &mut area), PONKO_OK);
        assert!(!has);
        let caret = PonkoRect {
            min: PonkoVec2 { x: 10.0, y: 20.0 },
            max: PonkoVec2 { x: 11.0, y: 38.0 },
        };
        assert_eq!(ponko_ctx_set_ime_cursor_area(ctx, caret), PONKO_OK);
        assert_eq!(ponko_ctx_ime_cursor_area(ctx, &mut has, &mut area), PONKO_OK);
        assert!(has);
        assert_eq!(area, caret);

        assert_eq!(
            ponko_ctx_feed_ime(ctx, &event(PONKO_IME_PREEDIT, c"にほ", Some((6, 6)))),
            PONKO_OK
        );
        assert_eq!(read(|b, c, l| ponko_ctx_preedit(ctx, b, c, l)), "にほ");
        let (mut start, mut end) = (0, 0);
        assert_eq!(ponko_ctx_preedit_cursor(ctx, &mut has, &mut start, &mut end), PONKO_OK);
        assert_eq!((has, start, end), (true, 6, 6));
        let mut composing = false;
        assert_eq!(ponko_ctx_composing(ctx, &mut composing), PONKO_OK);
        assert!(composing);

        assert_eq!(
            ponko_ctx_feed_ime(ctx, &event(PONKO_IME_COMMIT, c"日本", None)),
            PONKO_OK
        );
        assert_eq!(read(|b, c, l| ponko_ctx_typed_text(ctx, b, c, l)), "日本");
        assert_eq!(read(|b, c, l| ponko_ctx_preedit(ctx, b, c, l)), "");
        assert_eq!(ponko_ctx_preedit_cursor(ctx, &mut has, &mut start, &mut end), PONKO_OK);
        assert!(!has);

        assert_eq!(
            ponko_ctx_feed_ime(ctx, &event(PONKO_IME_PREEDIT, c"zhong", None)),
            PONKO_OK
        );
        assert_eq!(ponko_ctx_cancel_composition(ctx), PONKO_OK);
        assert_eq!(read(|b, c, l| ponko_ctx_preedit(ctx, b, c, l)), "");

        // Bad arguments are errors and change nothing.
        let bad = PonkoImeEvent {
            kind: 7,
            ..Default::default()
        };
        assert_eq!(ponko_ctx_feed_ime(ctx, &bad), PONKO_INVALID_ARGUMENT);
        assert_eq!(ponko_ctx_feed_ime(ctx, null()), PONKO_INVALID_ARGUMENT);
        assert_eq!(ponko_ctx_composing(ctx, null_mut()), PONKO_INVALID_ARGUMENT);
    }
}

#[test]
fn compositions_and_ime_requests_go_through_the_context() {
    run(input_side);
}

unsafe fn text_input(ctx: Ctx, parent: u64) -> u64 {
    let layout = PonkoUiLayout {
        width: PonkoUiLength {
            kind: PonkoUiLengthKind::PONKO_UI_LENGTH_POINTS as u32,
            value: 300.0,
        },
        height: PonkoUiLength {
            kind: PonkoUiLengthKind::PONKO_UI_LENGTH_POINTS as u32,
            value: 30.0,
        },
        flex_shrink: 0.0,
        ..ponko_ui_layout_default()
    };
    let mut id = PONKO_UI_NONE;
    let status = unsafe { ponko_ctx_ui_add_text_input(ctx, PONKO_UI_SCREEN, parent, null(), &layout, &mut id) };
    assert_eq!(status, PONKO_OK);
    id
}

unsafe fn ui_side(ctx: Ctx) {
    const SCREEN: PonkoVec2 = PonkoVec2 { x: 800.0, y: 600.0 };
    unsafe {
        let mut root = PONKO_UI_NONE;
        let layout = ponko_ui_layout_default();
        let status = ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &layout, &mut root);
        assert_eq!(status, PONKO_OK);
        let name = text_input(ctx, root);
        let pin = text_input(ctx, root);
        assert_eq!(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, root, false), PONKO_OK);

        let mut flag = true;
        assert_eq!(ponko_ctx_ui_set_ime_allowed(ctx, PONKO_UI_SCREEN, pin, false), PONKO_OK);
        assert_eq!(ponko_ctx_ui_ime_allowed(ctx, PONKO_UI_SCREEN, pin, &mut flag), PONKO_OK);
        assert!(!flag);
        assert_eq!(
            ponko_ctx_ui_ime_allowed(ctx, PONKO_UI_SCREEN, name, &mut flag),
            PONKO_OK
        );
        assert!(flag, "text inputs take IME by default");

        assert_eq!(ponko_ctx_ui_wants_ime(ctx, PONKO_UI_SCREEN, &mut flag), PONKO_OK);
        assert!(!flag, "nothing has the focus");
        assert_eq!(ponko_ctx_ui_set_focus(ctx, PONKO_UI_SCREEN, name), PONKO_OK);
        assert_eq!(ponko_ctx_ui_wants_ime(ctx, PONKO_UI_SCREEN, &mut flag), PONKO_OK);
        assert!(flag);

        let composition = PonkoUiComposition {
            preedit: c"にほ".as_ptr(),
            has_cursor: true,
            cursor_start: 6,
            cursor_end: 6,
            composing: true,
        };
        let status = ponko_ctx_ui_frame_ime(ctx, PONKO_UI_SCREEN, SCREEN, null(), &composition);
        assert_eq!(status, PONKO_OK);
        let preedit = |id| read(|b, c, l| ponko_ctx_ui_preedit(ctx, PONKO_UI_SCREEN, id, b, c, l));
        assert_eq!(preedit(name), "にほ");
        assert_eq!(preedit(pin), "");
        let (mut has, mut area) = (false, PonkoRect::default());
        assert_eq!(
            ponko_ctx_ui_ime_area(ctx, PONKO_UI_SCREEN, &mut has, &mut area),
            PONKO_OK
        );
        assert!(has && area.max.y > area.min.y);
        assert_eq!(
            ponko_ctx_ui_composition_interrupted(ctx, PONKO_UI_SCREEN, &mut flag),
            PONKO_OK
        );
        assert!(!flag);

        // The focus moves to the PIN field mid-composition: it is dropped.
        assert_eq!(ponko_ctx_ui_set_focus(ctx, PONKO_UI_SCREEN, pin), PONKO_OK);
        let status = ponko_ctx_ui_frame_ime(ctx, PONKO_UI_SCREEN, SCREEN, null(), &composition);
        assert_eq!(status, PONKO_OK);
        assert_eq!(
            ponko_ctx_ui_composition_interrupted(ctx, PONKO_UI_SCREEN, &mut flag),
            PONKO_OK
        );
        assert!(flag);
        assert_eq!(preedit(name), "");
        assert_eq!(preedit(pin), "", "the PIN field takes no IME");
        assert_eq!(
            ponko_ctx_ui_ime_area(ctx, PONKO_UI_SCREEN, &mut has, &mut area),
            PONKO_OK
        );
        assert!(!has);
        assert_eq!(ponko_ctx_ui_wants_ime(ctx, PONKO_UI_SCREEN, &mut flag), PONKO_OK);
        assert!(!flag);

        assert_eq!(
            ponko_ctx_ui_set_ime_allowed(ctx, PONKO_UI_SCREEN, 9999, true),
            PONKO_NOT_FOUND
        );
    }
}

#[test]
fn the_retained_ui_shows_a_composition_and_drops_it_when_the_focus_moves() {
    run(ui_side);
}

#[test]
fn scenarios_record_ime_steps() {
    unsafe {
        let s = ponko_input_scenario_new();
        let steps = [
            event(PonkoImeEventKind::PONKO_IME_ENABLED, c"", None),
            event(PonkoImeEventKind::PONKO_IME_PREEDIT, c"한", Some((3, 3))),
            event(PonkoImeEventKind::PONKO_IME_COMMIT, c"한", None),
            event(PonkoImeEventKind::PONKO_IME_DISABLED, c"", None),
        ];
        for (tick, step) in steps.iter().enumerate() {
            assert_eq!(ponko_input_scenario_ime(s, tick as u64, step), PONKO_OK);
        }
        assert_eq!(ponko_input_scenario_ime(s, 9, null()), PONKO_INVALID_ARGUMENT);
        let json = read(|b, c, l| ponko_input_scenario_to_json(s, b, c, l));
        for kind in ["ime_enabled", "ime_preedit", "ime_commit", "ime_disabled"] {
            assert!(json.contains(kind), "{json}");
        }
        assert!(json.contains("\"cursor\":[3,3]"), "{json}");
        ponko_input_scenario_free(s);
    }
}
