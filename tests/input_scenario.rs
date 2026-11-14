//! Tick-indexed input scenarios driving named actions in headless runs.
#[allow(unused_imports)]
use ponko::actions::InputEvent;
#[allow(unused_imports)]
use ponko::input::{Gamepad, GamepadAxis, GamepadButton};
use ponko::prelude::*;
#[allow(unused_imports)]
use ponko::scenario::{InputScenario, ScenarioCheck, ScenarioReport, ScenarioStatus};
use std::cell::RefCell;
use std::rc::Rc;

/// What a sim system saw of the "jump" action on one tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    tick: u64,
    pressed: bool,
    down: bool,
    released: bool,
}

fn jump_bindings(ctx: &mut Ctx) {
    let mut map = ActionMap::default();
    map.set("jump", Action::Button(vec![ButtonBinding::Key(Key::Space)]));
    ctx.input.set_actions(map);
}

/// Runs `ticks` headless ticks with `scenario` and records the jump action per tick.
fn record(scenario: InputScenario, ticks: u64) -> Vec<Seen> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    Game::new("scenario", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            log.borrow_mut().push(Seen {
                tick: ctx.time.sim_tick(),
                pressed: ctx.input.action_pressed("jump"),
                down: ctx.input.action_down("jump"),
                released: ctx.input.action_released("jump"),
            });
        })
        .input_scenario(scenario)
        .run_headless(ticks, jump_bindings);
    Rc::try_unwrap(seen).unwrap().into_inner()
}

fn seen(tick: u64, pressed: bool, down: bool, released: bool) -> Seen {
    Seen {
        tick,
        pressed,
        down,
        released,
    }
}

#[test]
fn two_transition_scenario_presses_holds_and_releases_on_its_ticks() {
    let scenario = InputScenario::new()
        .at(2, InputEvent::Key(Key::Space, true))
        .at(5, InputEvent::Key(Key::Space, false));
    assert_eq!(
        record(scenario, 7),
        [
            seen(0, false, false, false),
            seen(1, false, false, false),
            seen(2, true, true, false), // pressed edge on exactly one tick
            seen(3, false, true, false),
            seen(4, false, true, false),
            seen(5, false, false, true), // released edge on exactly one tick
            seen(6, false, false, false),
        ]
    );
}

#[test]
fn steps_apply_in_tick_order_and_a_same_tick_tap_keeps_both_edges() {
    // Added out of order; the press and release on tick 3 keep their order.
    let scenario = InputScenario::new()
        .at(3, InputEvent::Key(Key::Space, true))
        .at(1, InputEvent::Key(Key::Space, true))
        .at(3, InputEvent::Key(Key::Space, false))
        .at(2, InputEvent::Key(Key::Space, false));
    assert_eq!(
        record(scenario, 5),
        [
            seen(0, false, false, false),
            seen(1, true, true, false),
            seen(2, false, false, true),
            seen(3, true, false, true),
            seen(4, false, false, false),
        ]
    );
}

#[test]
fn steps_wait_for_their_tick_and_steps_past_the_run_never_apply() {
    let scenario = InputScenario::new()
        .at(0, InputEvent::Key(Key::Space, true))
        .at(100, InputEvent::Key(Key::Space, false));
    let trace = record(scenario, 3);
    assert_eq!(trace[0], seen(0, true, true, false));
    assert!(trace[1..].iter().all(|s| s.down && !s.pressed && !s.released));
}

/// A gameplay interaction: a body that jumps when the action is pressed and
/// falls under gravity.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
struct Body {
    y: f32,
    vy: f32,
    jumps: u32,
}

fn jump_and_fall(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let jump = ctx.input.action_pressed("jump");
    let drift = ctx.random().stream(0).range(-0.01, 0.01);
    ctx.world.query::<&mut Body>().for_each(|b| {
        if jump && b.y <= 0.0 {
            b.vy = 5.0;
            b.jumps += 1;
        }
        b.vy -= 9.81 * dt;
        b.y = (b.y + (b.vy + drift) * dt).max(0.0);
    });
}

fn play(scenario: InputScenario) -> Body {
    let mut world = Game::new("jumping", 1, 1)
        .seed(7)
        .sim_system(jump_and_fall)
        .input_scenario(scenario)
        .run_headless(120, |ctx| {
            jump_bindings(ctx);
            ctx.world.spawn((Body {
                y: 0.0,
                vy: 0.0,
                jumps: 0,
            },));
        });
    *world.query::<&Body>().iter().next().unwrap()
}

fn two_jumps() -> InputScenario {
    InputScenario::new()
        .at(10, InputEvent::Key(Key::Space, true))
        .at(20, InputEvent::Key(Key::Space, false))
        .at(90, InputEvent::Key(Key::Space, true))
        .at(91, InputEvent::Key(Key::Space, false))
}

#[test]
fn same_seed_and_scenario_give_bit_identical_results() {
    let (a, b) = (play(two_jumps()), play(two_jumps()));
    assert_eq!(a.jumps, 2);
    assert_eq!((a.y.to_bits(), a.vy.to_bits()), (b.y.to_bits(), b.vy.to_bits()));
    assert_eq!(a.jumps, b.jumps);
}

#[test]
fn holding_the_action_jumps_once() {
    // Held from tick 10 to the end: one pressed edge, so one jump.
    let body = play(InputScenario::new().at(10, InputEvent::Key(Key::Space, true)));
    assert_eq!(body.jumps, 1);
}

#[test]
fn without_a_scenario_nothing_is_pressed() {
    assert_eq!(play(InputScenario::new()).jumps, 0);
    assert!(record(InputScenario::new(), 3).iter().all(|s| !s.down && !s.pressed));
}

// ------------------------------------------------- checks, reports, release

/// Runs the jump game with `scenario` for its own length; returns the report.
fn run_jump_game(scenario: InputScenario) -> (World, ScenarioReport) {
    Game::new("jumping", 1, 1)
        .seed(7)
        .sim_system(jump_and_fall)
        .sim_system(count_jumps)
        .run_headless_scenario(scenario, |ctx| {
            jump_bindings(ctx);
            ctx.world.reflect_resource::<Progress>();
            ctx.world.insert_resource(Progress { jumps: 0 });
            ctx.world.reflect::<Body>();
            ctx.world.spawn((Body {
                y: 0.0,
                vy: 0.0,
                jumps: 0,
            },));
        })
}

/// Counts jump presses in a reflected resource, for checks.
#[derive(serde::Serialize, serde::Deserialize)]
struct Progress {
    jumps: u32,
}

fn count_jumps(ctx: &mut Ctx) {
    if ctx.input.action_pressed("jump") {
        ctx.world.resource_mut::<Progress>().jumps += 1;
    }
}

#[test]
fn a_deliberately_wrong_check_fails_with_its_step_and_tick() {
    let scenario = InputScenario::new()
        .named("jump-once")
        .at(10, InputEvent::Key(Key::Space, true)) // step 0
        .expect(10, ScenarioCheck::action("jump").pressed(true)) // step 1, holds
        .expect(11, ScenarioCheck::count("Body", 1)) // step 2, holds
        .expect(12, ScenarioCheck::action("jump").pressed(true)) // step 3: the edge is gone
        .expect(13, ScenarioCheck::count("Body", 9)) // step 4: never reached
        .duration(20);
    let (_world, report) = run_jump_game(scenario);
    assert_eq!(report.status, ScenarioStatus::Failed);
    assert_eq!(report.id.as_deref(), Some("jump-once"));
    assert_eq!((report.inputs_applied, report.checks_passed), (1, 2));
    assert_eq!(report.ticks_run, 13, "the run ends at the failing tick");
    let failure = report.failure.clone().unwrap();
    assert_eq!((failure.step, failure.tick, failure.sim_tick), (3, 12, 12));
    assert_eq!(failure.check, "action jump");
    assert_eq!(failure.expected, serde_json::json!({ "pressed": true }));
    assert_eq!(failure.actual, serde_json::json!({ "pressed": false }));
    assert!(
        failure.message.contains("pressed: expected true, saw false"),
        "{}",
        failure.message
    );
    let panic = std::panic::catch_unwind(|| report.assert_passed()).unwrap_err();
    let text = panic.downcast_ref::<String>().unwrap();
    assert!(text.contains("step 3 (tick 12)"), "{text}");
}

#[test]
fn checks_on_actions_resources_and_counts_hold_after_the_systems_ran() {
    let scenario = InputScenario::new()
        .at(10, InputEvent::Key(Key::Space, true))
        .expect(
            10,
            ScenarioCheck::action("jump").pressed(true).down(true).released(false),
        )
        .expect(10, ScenarioCheck::resource("Progress", "/jumps", 1))
        .expect(11, ScenarioCheck::action("jump").pressed(false).down(true))
        .at(12, InputEvent::Key(Key::Space, false))
        .expect(12, ScenarioCheck::action("jump").down(false).released(true))
        .expect(12, ScenarioCheck::count("Body", 1))
        .duration(15);
    let (_world, report) = run_jump_game(scenario);
    report.assert_passed();
    assert_eq!(
        (report.ticks_run, report.checks_passed, report.inputs_applied),
        (15, 5, 2)
    );
}

#[test]
fn unknown_actions_resources_and_components_fail_instead_of_passing() {
    for (check, needle) in [
        (ScenarioCheck::action("jmup").down(false), "no action `jmup`"),
        (ScenarioCheck::resource("Nothing", "", 1), "Nothing"),
        (ScenarioCheck::resource("Progress", "/9", 1), "nothing at `/9`"),
        (ScenarioCheck::count("Nothing", 0), "not a reflected component"),
    ] {
        let (_world, report) = run_jump_game(InputScenario::new().expect(0, check));
        let message = report.failure.expect("must fail").message;
        assert!(message.contains(needle), "{needle}: {message}");
    }
}

/// Runs a game that records whether Space is held on each tick, with `scenario`.
fn held_trace(scenario: InputScenario, ticks: u64, cancel_at: Option<u64>) -> (Vec<bool>, ScenarioReport) {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let log = trace.clone();
    let (_world, report) = {
        let game = Game::new("held", 1, 1).sim_system(move |ctx: &mut Ctx| {
            if cancel_at == Some(ctx.time.sim_tick()) {
                assert!(ctx.cancel_scenario());
            }
            log.borrow_mut().push(ctx.input.key_down(Key::Space));
        });
        let world = game.input_scenario(scenario).run_headless(ticks, |_| {});
        let report = world.get_resource::<ScenarioReport>().cloned().unwrap();
        (world, report)
    };
    (Rc::try_unwrap(trace).unwrap().into_inner(), report)
}

#[test]
fn cancelling_releases_the_inputs_the_scenario_holds() {
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .at(1, InputEvent::Key(Key::Enter, true))
        .at(2, InputEvent::Key(Key::Enter, false))
        .duration(10);
    let (held, report) = held_trace(scenario, 6, Some(3));
    assert_eq!(
        held,
        [false, true, true, false, false, false],
        "released on the cancel tick"
    );
    assert_eq!(report.status, ScenarioStatus::Cancelled);
    assert_eq!(report.released, 1, "Enter was already up");
    assert_eq!(report.cancel_reason.as_deref(), Some("cancelled"));
}

#[test]
fn a_finished_scenario_keeps_its_input_held_unless_it_says_otherwise() {
    let hold = || InputScenario::new().at(1, InputEvent::Key(Key::Space, true));
    let (held, report) = held_trace(hold(), 5, None);
    assert_eq!(held, [false, true, true, true, true]);
    assert_eq!((report.status, report.released), (ScenarioStatus::Passed, 0));

    // Released right after the last tick of the run.
    let (held, report) = held_trace(hold().duration(3).release_when_done(), 5, None);
    assert_eq!(held, [false, true, true, false, false]);
    assert_eq!((report.status, report.released), (ScenarioStatus::Passed, 1));
}

#[test]
fn a_failed_check_releases_the_inputs_it_was_holding() {
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .expect(2, ScenarioCheck::count("Nothing", 0));
    let (held, report) = held_trace(scenario, 5, None);
    assert_eq!(held, [false, true, true, false, false]);
    assert_eq!((report.status, report.released), (ScenarioStatus::Failed, 1));
}

#[test]
fn a_replaced_scenario_releases_what_the_old_one_held() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let log = trace.clone();
    let world = Game::new("replace", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 2 {
                ctx.start_scenario(InputScenario::new().named("second").duration(2));
            }
            log.borrow_mut().push(ctx.input.key_down(Key::Space));
        })
        .input_scenario(
            InputScenario::new()
                .named("first")
                .at(0, InputEvent::Key(Key::Space, true))
                .duration(10),
        )
        .run_headless(6, |_| {});
    assert_eq!(*trace.borrow(), [true, true, false, false, false, false]);
    let report = world.resource::<ScenarioReport>();
    assert_eq!(
        (report.id.as_deref(), report.status),
        (Some("second"), ScenarioStatus::Passed)
    );
    assert_eq!(report.start_tick, 3, "started during tick 2, so it begins on tick 3");
}

#[test]
fn focus_loss_and_disconnects_in_a_scenario_are_not_released_twice() {
    let pad = Gamepad::from_slot(0).unwrap();
    let scenario = InputScenario::new()
        .at(0, InputEvent::Key(Key::Space, true))
        .at(0, InputEvent::GamepadButton(pad, GamepadButton::South, true))
        .at(0, InputEvent::GamepadAxis(pad, GamepadAxis::LeftStickX, 0.5))
        .at(1, InputEvent::FocusLost)
        .duration(10);
    let (_, report) = held_trace(scenario, 3, Some(2));
    assert_eq!(report.released, 2, "only the pad's button and axis are still held");
}

// ------------------------------------------------------------- JSON format

const JUMP_JSON: &str = r#"{
    "version": 1,
    "id": "jump-twice",
    "duration": 120,
    "steps": [
        { "tick": 20, "type": "key", "key": "Space", "down": false },
        { "tick": 10, "type": "key", "key": "Space", "down": true },
        { "tick": 10, "type": "expect_action", "action": "jump", "pressed": true },
        { "tick": 90, "type": "key", "key": "Space", "down": true },
        { "tick": 91, "type": "key", "key": "Space", "down": false },
        { "tick": 119, "type": "expect_resource", "resource": "Progress", "pointer": "/jumps", "equals": 2 }
    ]
}"#;

#[test]
fn a_json_scenario_runs_like_the_one_built_in_code() {
    let from_file = InputScenario::from_json(JUMP_JSON).unwrap();
    let (mut world, report) = run_jump_game(from_file);
    report.assert_passed();
    assert_eq!(report.id.as_deref(), Some("jump-twice"));
    assert_eq!((report.ticks_run, report.checks_passed), (120, 2));
    let by_hand = play(two_jumps());
    let body = *world.query::<&Body>().iter().next().unwrap();
    assert_eq!(
        (body.y.to_bits(), body.vy.to_bits(), body.jumps),
        (by_hand.y.to_bits(), by_hand.vy.to_bits(), 2)
    );
}

#[test]
fn a_scenario_survives_a_round_trip_through_json_with_steps_in_file_order() {
    let scenario = InputScenario::from_json(JUMP_JSON).unwrap();
    let json = scenario.to_json();
    assert_eq!(
        json["steps"][0]["tick"], 20,
        "step 0 stays first although it is at tick 20"
    );
    assert_eq!(json["steps"][2]["type"], "expect_action");
    let again = InputScenario::from_value(&json).unwrap();
    assert_eq!(again.to_json(), json);
    assert_eq!(
        (again.ticks(), again.step_count(), again.id()),
        (120, 6, Some("jump-twice"))
    );
}

#[test]
fn a_json_scenario_that_could_silently_do_nothing_is_refused() {
    let bad = |steps: &str, extra: &str| {
        InputScenario::from_json(&format!(r#"{{ "version": 1, {extra} "steps": [{steps}] }}"#))
            .unwrap_err()
            .to_string()
    };
    let key = r#"{ "tick": 1, "type": "key", "key": "Space", "down": true"#;
    assert!(bad(&format!("{key}, \"dwon\": true }}"), "").contains("unknown field `dwon`"));
    assert!(bad(r#"{ "tick": 1, "type": "key", "key": "Spacee", "down": true }"#, "").contains("Spacee"));
    assert!(bad(r#"{ "tick": 1, "type": "tap", "key": "Space" }"#, "").contains("tap"));
    assert!(bad(r#"{ "type": "focus_lost" }"#, "").contains("tick"));
    assert!(bad(r#"{ "tick": 1, "type": "expect_action", "action": "jump" }"#, "").contains("at least one of"));
    assert!(bad(r#"{ "tick": 1, "type": "expect_resource", "resource": "R" }"#, "").contains("equals"));
    assert!(bad(&format!("{key} }}"), r#""duration": 1,"#).contains("outside the scenario's 1 ticks"));
    assert!(bad("", r#""duration": 99999999999,"#).contains("too long"));
    assert!(bad("", r#""colour": 1,"#).contains("unknown field `colour`"));
    let newer = InputScenario::from_json(r#"{ "version": 2, "steps": [] }"#)
        .unwrap_err()
        .to_string();
    assert!(newer.contains("unsupported scenario version 2"), "{newer}");
    assert!(InputScenario::from_json("not json").is_err());
}

#[test]
fn a_scenario_with_too_many_steps_is_refused() {
    let step = r#"{ "tick": 0, "type": "focus_lost" }"#;
    let steps = vec![step; ponko::scenario::MAX_SCENARIO_STEPS + 1].join(",");
    let error = InputScenario::from_json(&format!(r#"{{ "version": 1, "steps": [{steps}] }}"#)).unwrap_err();
    assert!(error.to_string().contains("too many"), "{error}");
}

#[test]
fn every_input_kind_round_trips_through_the_file_format() {
    let pad = Gamepad::from_slot(1).unwrap();
    let scenario = InputScenario::new()
        .at(0, InputEvent::Key(Key::W, true))
        .at(1, InputEvent::Mouse(MouseButton::Left, true))
        .at(2, InputEvent::GamepadButton(pad, GamepadButton::South, true))
        .at(3, InputEvent::GamepadAxis(pad, GamepadAxis::LeftStickX, -0.5))
        .at(4, InputEvent::FocusLost)
        .at(5, InputEvent::GamepadDisconnected(pad))
        .expect(6, ScenarioCheck::count("Body", 0));
    let json = scenario.to_json();
    assert_eq!(InputScenario::from_value(&json).unwrap().to_json(), json);
    assert_eq!(json["steps"][3]["axis"], "LeftStickX");
}

// ------------------------------------------- scenarios started by a system

/// Runs a game whose first system starts `scenario` during tick 3, and
/// records what the second system saw of the jump action on each tick.
fn started_on_tick_3(scenario: InputScenario, ticks: u64) -> (Vec<Seen>, ScenarioReport) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    let mut scenario = Some(scenario);
    let world = Game::new("started", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 3 {
                ctx.start_scenario(scenario.take().unwrap());
            }
        })
        .sim_system(move |ctx: &mut Ctx| {
            log.borrow_mut().push(Seen {
                tick: ctx.time.sim_tick(),
                pressed: ctx.input.action_pressed("jump"),
                down: ctx.input.action_down("jump"),
                released: ctx.input.action_released("jump"),
            });
        })
        .run_headless(ticks, jump_bindings);
    let report = world.resource::<ScenarioReport>().clone();
    (Rc::try_unwrap(seen).unwrap().into_inner(), report)
}

#[test]
fn a_scenario_started_by_a_system_begins_on_the_next_tick_and_checks_wait_for_it() {
    let scenario = InputScenario::new()
        .at(0, InputEvent::Key(Key::Space, true))
        // Tick 0 of the scenario is sim tick 4: the press is fed first, then
        // the check runs, so it holds. Run a tick early it would fail.
        .expect(0, ScenarioCheck::action("jump").pressed(true))
        .at(1, InputEvent::Key(Key::Space, false))
        .expect(1, ScenarioCheck::action("jump").released(true))
        .duration(3);
    let (seen, report) = started_on_tick_3(scenario, 8);
    report.assert_passed();
    assert_eq!(report.start_tick, 4);
    assert_eq!(
        (report.ticks_run, report.checks_passed, report.inputs_applied),
        (3, 2, 2)
    );
    assert!(
        seen[..4].iter().all(|s| !s.down && !s.pressed),
        "nothing before tick 4: {seen:?}"
    );
    assert_eq!(
        seen[4],
        Seen {
            tick: 4,
            pressed: true,
            down: true,
            released: false
        }
    );
    assert_eq!(
        seen[5],
        Seen {
            tick: 5,
            pressed: false,
            down: false,
            released: true
        }
    );
}

#[test]
fn a_one_tick_scenario_started_by_a_system_feeds_its_input_to_the_next_tick() {
    let scenario = InputScenario::new().at(0, InputEvent::Key(Key::Space, true));
    assert_eq!(scenario.ticks(), 1);
    let (seen, report) = started_on_tick_3(scenario, 7);
    report.assert_passed();
    assert_eq!((report.start_tick, report.ticks_run, report.inputs_applied), (4, 1, 1));
    assert_eq!(
        seen[3],
        Seen {
            tick: 3,
            pressed: false,
            down: false,
            released: false
        }
    );
    assert_eq!(
        seen[4],
        Seen {
            tick: 4,
            pressed: true,
            down: true,
            released: false
        }
    );
}
