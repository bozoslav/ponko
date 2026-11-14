//! IME composition in the retained UI's text inputs, driven headless by
//! scripted IME events (`InputScenario::ime`), as Japanese, Chinese pinyin
//! and Korean Hangul input methods report them.
use ponko::actions::InputEvent;
use ponko::input::ImeEvent;
use ponko::prelude::*;
use ponko::scenario::{InputScenario, ScenarioCheck};
use ponko::ui::{UiEvent, UiLayout};
use std::cell::RefCell;
use std::rc::Rc;

/// What a frame system saw after the UI's frame of each tick.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    tick: u64,
    /// The preedit `field` shows.
    preedit: Option<String>,
    /// What `field` holds.
    text: String,
    ime_allowed: bool,
    area: Option<Rect>,
    submitted: bool,
}

type Log = Rc<RefCell<Vec<Seen>>>;

/// Two text fields (`a`, `b`), and a field that doesn't take IME (`pin`).
fn form(ctx: &mut Ctx) {
    let root = ctx.ui.root().layout(UiLayout::fill().gap(8.0)).name("form").id();
    for name in ["a", "b"] {
        ctx.ui.text_input(root).layout(UiLayout::fixed(300.0, 30.0)).name(name);
    }
    ctx.ui
        .text_input(root)
        .layout(UiLayout::fixed(300.0, 30.0))
        .ime_allowed(false)
        .name("pin");
    ctx.ui.push_screen(root, false);
}

/// A game with [`form`] that logs what field `field` shows each frame.
fn game(log: &Log, field: &'static str) -> Game {
    let log = log.clone();
    Game::new("ime", 640, 480)
        .headless_frames(true)
        .system(move |ctx: &mut Ctx| {
            let id = ctx.ui.find(field).unwrap();
            let mut submitted = false;
            while let Some(event) = ctx.ui.next_event() {
                submitted |= event == UiEvent::Submitted(id);
            }
            log.borrow_mut().push(Seen {
                tick: ctx.time.sim_tick() - 1,
                preedit: ctx.ui.preedit(id).map(str::to_string),
                text: ctx.ui.text(id).unwrap_or_default().to_string(),
                ime_allowed: ctx.input.ime_allowed(),
                area: ctx.input.ime_cursor_area(),
                submitted,
            });
        })
}

fn at(log: &Log, tick: u64) -> Seen {
    log.borrow().iter().find(|s| s.tick == tick).cloned().unwrap()
}

#[test]
fn japanese_composition_shows_inline_and_commits_into_the_focused_field() {
    let log = Log::default();
    let scenario = InputScenario::new()
        .click(0, "a")
        .ime(1, ImeEvent::Enabled)
        .ime(1, ImeEvent::preedit("に", Some((3, 3))))
        .ime(2, ImeEvent::preedit("にほ", Some((6, 6))))
        .ime(3, ImeEvent::preedit("にほん", Some((9, 9))))
        // Space converts: the whole clause is selected for conversion.
        .ime(4, ImeEvent::preedit("日本", Some((0, 6))))
        .ime(5, ImeEvent::Commit("日本".into()))
        .expect(4, ScenarioCheck::ui("a").focused(true).text(""))
        .expect(5, ScenarioCheck::ui("a").text("日本"))
        .duration(6);
    let (_, report) = game(&log, "a").run_headless_scenario(scenario, form);
    report.assert_passed();

    assert!(at(&log, 0).ime_allowed, "focusing a text input allows IME");
    assert_eq!(at(&log, 0).preedit, None);
    assert_eq!(at(&log, 1).preedit.as_deref(), Some("に"));
    assert_eq!(at(&log, 3).preedit.as_deref(), Some("にほん"));
    assert_eq!(at(&log, 4).preedit.as_deref(), Some("日本"));
    assert_eq!(at(&log, 5).preedit, None, "the commit ends the composition");
    assert_eq!(at(&log, 5).text, "日本");

    // The candidate window follows the composition's caret.
    let x = |tick| at(&log, tick).area.expect("the caret area is set").min.x;
    assert!(x(2) > x(1) && x(3) > x(2), "{} {} {}", x(1), x(2), x(3));
    assert!(x(4) < x(3), "converting puts the caret at the clause's start");
    assert!(x(5) > x(0), "after the commit the caret is after the text");
}

#[test]
fn pinyin_keys_belong_to_the_ime_while_composing() {
    let log = Log::default();
    let scenario = InputScenario::new()
        .type_text(0, "a", "Hi ")
        .ime(1, ImeEvent::preedit("z", Some((1, 1))))
        .ime(2, ImeEvent::preedit("zh", Some((2, 2))))
        .ime(3, ImeEvent::preedit("zhong", Some((5, 5))))
        // Backspace edits the pinyin, not the field.
        .at(4, InputEvent::Key(Key::Backspace, true))
        .ime(4, ImeEvent::preedit("zhon", Some((4, 4))))
        .at(5, InputEvent::Key(Key::Backspace, false))
        .ime(5, ImeEvent::preedit("zhongwen", Some((8, 8))))
        // Left moves inside the pinyin.
        .at(6, InputEvent::Key(Key::Left, true))
        .ime(6, ImeEvent::preedit("zhongwen", Some((5, 5))))
        .at(7, InputEvent::Key(Key::Left, false))
        // Enter picks the candidate: it commits, and doesn't submit the field.
        .at(8, InputEvent::Key(Key::Enter, true))
        .ime(8, ImeEvent::Commit("中文".into()))
        .at(9, InputEvent::Key(Key::Enter, false))
        .expect(8, ScenarioCheck::ui("a").text("Hi 中文"))
        // Without a composition, Enter submits again.
        .at(10, InputEvent::Key(Key::Enter, true))
        .at(11, InputEvent::Key(Key::Enter, false))
        .duration(12);
    let (_, report) = game(&log, "a").run_headless_scenario(scenario, form);
    report.assert_passed();
    assert_eq!(at(&log, 4).text, "Hi ", "Backspace mid-composition deletes nothing");
    assert_eq!(at(&log, 4).preedit.as_deref(), Some("zhon"));
    assert_eq!(at(&log, 6).text, "Hi ");
    assert!(!at(&log, 8).submitted, "the Enter that committed doesn't submit");
    assert!(at(&log, 10).submitted, "a later Enter does");
}

#[test]
fn hangul_commits_syllable_by_syllable_while_the_next_one_composes() {
    let log = Log::default();
    let scenario = InputScenario::new()
        .click(0, "a")
        .ime(1, ImeEvent::preedit("ㅎ", Some((3, 3))))
        .ime(2, ImeEvent::preedit("하", Some((3, 3))))
        .ime(3, ImeEvent::preedit("한", Some((3, 3))))
        // The next consonant starts a new syllable: the IME commits 한 and
        // composes ㄱ in the same frame.
        .ime(4, ImeEvent::Commit("한".into()))
        .ime(4, ImeEvent::preedit("ㄱ", Some((3, 3))))
        .ime(5, ImeEvent::preedit("그", Some((3, 3))))
        .ime(6, ImeEvent::preedit("글", Some((3, 3))))
        .ime(7, ImeEvent::Commit("글".into()))
        .expect(7, ScenarioCheck::ui("a").text("한글"))
        .duration(8);
    let (_, report) = game(&log, "a").run_headless_scenario(scenario, form);
    report.assert_passed();
    assert_eq!(at(&log, 4).text, "한");
    assert_eq!(at(&log, 4).preedit.as_deref(), Some("ㄱ"));
    assert_eq!(at(&log, 6).preedit.as_deref(), Some("글"));
    assert_eq!(at(&log, 7).preedit, None);
}

#[test]
fn a_cancelled_composition_leaves_the_field_and_escape_with_the_ime() {
    let log = Log::default();
    let scenario = InputScenario::new()
        .type_text(0, "a", "ok")
        .ime(1, ImeEvent::preedit("にほん", Some((9, 9))))
        // Escape cancels the composition; it doesn't close the screen.
        .at(2, InputEvent::Key(Key::Escape, true))
        .ime(2, ImeEvent::cancel())
        .at(3, InputEvent::Key(Key::Escape, false))
        .expect(3, ScenarioCheck::ui("a").shown(true).text("ok"))
        // A composition the IME turns off is dropped as well.
        .ime(4, ImeEvent::preedit("zhong", None))
        .ime(5, ImeEvent::Disabled)
        .expect(5, ScenarioCheck::ui("a").text("ok"))
        .duration(6);
    let (_, report) = game(&log, "a").run_headless_scenario(scenario, form);
    report.assert_passed();
    assert_eq!(at(&log, 1).preedit.as_deref(), Some("にほん"));
    assert_eq!(at(&log, 2).preedit, None);
    assert_eq!(at(&log, 4).preedit.as_deref(), Some("zhong"));
    assert_eq!(at(&log, 5).preedit, None);
}

#[test]
fn clicking_another_field_mid_composition_drops_it_instead_of_moving_it() {
    let log = Log::default();
    let b = Log::default();
    let scenario = InputScenario::new()
        .type_text(0, "a", "x")
        .ime(1, ImeEvent::preedit("にほ", Some((6, 6))))
        .click(2, "b")
        .expect(2, ScenarioCheck::ui("b").focused(true).text(""))
        .expect(2, ScenarioCheck::ui("a").text("x"))
        // A new composition goes to the field that has the focus now.
        .ime(3, ImeEvent::preedit("ㅎ", None))
        .ime(4, ImeEvent::Commit("한".into()))
        .expect(4, ScenarioCheck::ui("b").text("한"))
        .expect(4, ScenarioCheck::ui("a").text("x"))
        .duration(5);
    let b_log = b.clone();
    let game = game(&log, "a").system(move |ctx: &mut Ctx| {
        let id = ctx.ui.find("b").unwrap();
        b_log.borrow_mut().push(Seen {
            tick: ctx.time.sim_tick() - 1,
            preedit: ctx.ui.preedit(id).map(str::to_string),
            text: ctx.ui.text(id).unwrap_or_default().to_string(),
            ime_allowed: ctx.input.ime_allowed(),
            area: ctx.input.ime_cursor_area(),
            submitted: false,
        });
        if ctx.time.sim_tick() - 1 == 2 {
            assert!(ctx.ui.composition_interrupted());
            assert_eq!(ctx.input.preedit(), "", "the engine cancelled the composition");
        }
    });
    let (_, report) = game.run_headless_scenario(scenario, form);
    report.assert_passed();
    assert_eq!(at(&log, 1).preedit.as_deref(), Some("にほ"));
    assert_eq!(at(&log, 2).preedit, None, "the old field stops showing it");
    assert_eq!(at(&b, 2).preedit, None, "the new field doesn't get it");
    assert_eq!(at(&b, 3).preedit.as_deref(), Some("ㅎ"));
    assert!(at(&b, 2).ime_allowed, "the new field takes IME too");
}

#[test]
fn tab_or_a_system_moving_the_focus_mid_composition_drops_it() {
    let log = Log::default();
    let scenario = InputScenario::new()
        .click(0, "a")
        .ime(1, ImeEvent::preedit("zhong", None))
        .at(2, InputEvent::Key(Key::Tab, true))
        .at(3, InputEvent::Key(Key::Tab, false))
        .expect(2, ScenarioCheck::ui("b").focused(true).text(""))
        .ime(4, ImeEvent::preedit("wen", None))
        .duration(7);
    // On tick 5 a system moves the focus back to `a` between frames.
    let game = game(&log, "b").system(|ctx: &mut Ctx| {
        if ctx.time.sim_tick() - 1 == 5 {
            let a = ctx.ui.find("a").unwrap();
            ctx.ui.set_focus(a);
        }
    });
    let (world, report) = game.run_headless_scenario(scenario, form);
    report.assert_passed();
    assert_eq!(at(&log, 2).preedit, None);
    assert_eq!(at(&log, 4).preedit.as_deref(), Some("wen"));
    assert_eq!(at(&log, 6).preedit, None, "b lost the focus between frames");
    assert_eq!(at(&log, 6).text, "");
    drop(world);
}

#[test]
fn a_field_that_takes_no_ime_turns_it_off_and_shows_no_preedit() {
    let log = Log::default();
    let scenario = InputScenario::new()
        .click(0, "a")
        .click(1, "pin")
        .ime(2, ImeEvent::preedit("に", None))
        .type_text(3, "pin", "1234")
        .expect(3, ScenarioCheck::ui("pin").text("1234"))
        .duration(4);
    let (_, report) = game(&log, "pin").run_headless_scenario(scenario, form);
    report.assert_passed();
    assert!(at(&log, 0).ime_allowed);
    assert!(!at(&log, 1).ime_allowed, "focusing the PIN field turns IME off");
    assert_eq!(at(&log, 2).preedit, None);
}

#[test]
fn the_ui_leaves_a_games_own_ime_setting_alone_without_a_text_input() {
    let allowed = Rc::new(RefCell::new(Vec::new()));
    let seen = allowed.clone();
    Game::new("own widget", 640, 480)
        .headless_frames(true)
        .system(move |ctx: &mut Ctx| {
            let tick = ctx.time.sim_tick() - 1;
            if tick == 0 {
                // A game's own chat box.
                ctx.input.set_ime_allowed(true);
                ctx.input.set_ime_cursor_area(Rect::new(10.0, 400.0, 1.0, 16.0));
            }
            seen.borrow_mut()
                .push((ctx.input.ime_allowed(), ctx.input.preedit().to_string()));
            if tick == 1 {
                ctx.input.feed_ime(ImeEvent::preedit("にほ", None));
            }
        })
        .run_headless(3, form);
    assert_eq!(
        allowed.borrow().as_slice(),
        &[(true, String::new()), (true, String::new()), (true, "にほ".to_string())]
    );
}

#[test]
fn ime_steps_survive_the_json_format() {
    let scenario = InputScenario::from_json(
        r#"{ "version": 1, "steps": [
            { "tick": 0, "type": "ui_click", "node": "a" },
            { "tick": 1, "type": "ime_enabled" },
            { "tick": 1, "type": "ime_preedit", "text": "にほん", "cursor": [9, 9] },
            { "tick": 2, "type": "ime_preedit", "text": "日本", "cursor": [0, 6] },
            { "tick": 3, "type": "ime_commit", "text": "日本" },
            { "tick": 4, "type": "ime_preedit", "text": "ni" },
            { "tick": 4, "type": "ime_disabled" },
            { "tick": 5, "type": "expect_ui", "node": "a", "text": "日本" }
        ] }"#,
    )
    .unwrap();
    assert_eq!(
        InputScenario::from_value(&scenario.to_json()).unwrap().to_json(),
        scenario.to_json()
    );
    let log = Log::default();
    let (_, report) = game(&log, "a").run_headless_scenario(scenario, form);
    report.assert_passed();
    assert_eq!(at(&log, 2).preedit.as_deref(), Some("日本"));
    assert_eq!(at(&log, 4).preedit, None);
    assert!(
        InputScenario::from_json(r#"{ "version": 1, "steps": [{ "tick": 0, "type": "ime_commit" }] }"#).is_err(),
        "a commit needs its text"
    );
}
