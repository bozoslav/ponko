//! Input scenarios in the windowed frame loop's shape: several simulation
//! ticks per frame, driven without a window (`begin_frame`, `run_systems`,
//! then the input's end of frame, as `window.rs` does each frame).
//! Headless runs, one tick per frame, are covered in `tests/input_scenario.rs`.

use super::*;
use crate::actions::InputEvent;
use crate::scenario::InputScenario;
use crate::scenario::ScenarioCheck;
use crate::scenario::ScenarioStatus;
use crate::{
    actions::{Action, ActionMap, ButtonBinding},
    input::Key,
};

/// What the "jump" action looked like to a sim system on one tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    tick: u64,
    pressed: bool,
    down: bool,
    released: bool,
}

#[derive(Default)]
struct Trace(Vec<Seen>);

fn seen(tick: u64, pressed: bool, down: bool, released: bool) -> Seen {
    Seen {
        tick,
        pressed,
        down,
        released,
    }
}

/// A game without the built-in drawing systems that records the action per tick.
fn game(scenario: InputScenario) -> (Game, Ctx) {
    let (game, ctx) = game_without_scenario();
    (game.input_scenario(scenario), ctx)
}

/// Like [`game`], with no scenario installed by the game itself.
fn game_without_scenario() -> (Game, Ctx) {
    let game = Game::new("t", 1, 1)
        .remove(Systems::ANIMATE_SPRITES)
        .remove(Systems::DRAW_TILEMAPS)
        .remove(Systems::DRAW_SPRITES)
        .remove(Systems::DRAW_PARTICLES)
        .sim_system(|ctx: &mut Ctx| {
            let s = Seen {
                tick: ctx.time.sim_tick(),
                pressed: ctx.input.action_pressed("jump"),
                down: ctx.input.action_down("jump"),
                released: ctx.input.action_released("jump"),
            };
            ctx.world.resource_mut::<Trace>().0.push(s);
        });
    let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(800.0, 600.0));
    ctx.world.insert_resource(Trace::default());
    let mut map = ActionMap::default();
    map.set("jump", Action::Button(vec![ButtonBinding::Key(Key::Space)]));
    ctx.input.set_actions(map);
    (game, ctx)
}

/// One windowed frame running `ticks` simulation ticks.
fn frame(game: &mut Game, ctx: &mut Ctx, ticks: u32) {
    ctx.begin_frame(FrameTiming {
        dt: 1.0 / 60.0,
        fixed_steps: ticks,
        alpha: 0.0,
    });
    game.run_systems(ctx, ticks);
    ctx.input.end_frame();
}

fn trace(ctx: &mut Ctx) -> Vec<Seen> {
    std::mem::take(&mut ctx.world.resource_mut::<Trace>().0)
}

#[test]
fn input_steps_land_on_their_own_tick_when_a_frame_runs_several_ticks() {
    // Frames of three ticks: 0-2, 3-5, 6-8. Press on tick 1, release on tick 4.
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .at(4, InputEvent::Key(Key::Space, false));
    let (mut game, mut ctx) = game(scenario);
    for _ in 0..3 {
        frame(&mut game, &mut ctx, 3);
    }
    assert_eq!(
        trace(&mut ctx),
        [
            seen(0, false, false, false),
            seen(1, true, true, false),  // fed before tick 1's systems
            seen(2, true, true, false),  // edges last to the end of the frame, as with live input
            seen(3, false, true, false), // the next frame starts clean; the key is still held
            seen(4, false, false, true), // fed before tick 4's systems
            seen(5, false, false, true),
            seen(6, false, false, false),
            seen(7, false, false, false),
            seen(8, false, false, false),
        ]
    );
}

#[test]
fn a_press_and_release_in_one_multi_tick_frame_keep_both_edges() {
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .at(2, InputEvent::Key(Key::Space, false));
    let (mut game, mut ctx) = game(scenario);
    frame(&mut game, &mut ctx, 4);
    assert_eq!(
        trace(&mut ctx),
        [
            seen(0, false, false, false),
            seen(1, true, true, false),
            seen(2, true, false, true),
            seen(3, true, false, true),
        ]
    );
}

#[test]
fn frames_with_no_ticks_do_not_advance_the_scenario() {
    let scenario = InputScenario::new().at(1, InputEvent::Key(Key::Space, true));
    let (mut game, mut ctx) = game(scenario);
    frame(&mut game, &mut ctx, 1);
    frame(&mut game, &mut ctx, 0); // a slow frame that fell behind the clock
    frame(&mut game, &mut ctx, 0);
    assert_eq!(trace(&mut ctx), [seen(0, false, false, false)]);
    frame(&mut game, &mut ctx, 2);
    assert_eq!(
        trace(&mut ctx),
        [seen(1, true, true, false), seen(2, true, true, false)]
    );
}

#[test]
fn checks_see_the_state_of_their_own_tick_in_a_multi_tick_frame() {
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .expect(1, ScenarioCheck::action("jump").pressed(true))
        .expect(3, ScenarioCheck::action("jump").pressed(false).down(true))
        .expect(5, ScenarioCheck::action("jump").down(false)); // wrong: still held
    let (mut game, mut ctx) = game(scenario);
    frame(&mut game, &mut ctx, 3);
    frame(&mut game, &mut ctx, 3);
    let report = ctx.input.scenario_report(&ctx.world).unwrap();
    assert_eq!(report.status, ScenarioStatus::Failed);
    let failure = report.failure.as_ref().unwrap();
    assert_eq!((failure.step, failure.tick), (3, 5));
    assert_eq!(report.checks_passed, 2);
}

#[test]
fn a_scenario_ending_mid_frame_stops_applying_for_the_rest_of_the_frame() {
    let scenario = InputScenario::new()
        .at(0, InputEvent::Key(Key::Space, true))
        .at(1, InputEvent::Key(Key::Space, false))
        .release_when_done();
    let (mut game, mut ctx) = game(scenario);
    frame(&mut game, &mut ctx, 4);
    let report = ctx.input.scenario_report(&ctx.world).unwrap();
    assert_eq!((report.status, report.ticks_run), (ScenarioStatus::Passed, 2));
    assert_eq!(report.released, 0, "already released by its own step");
}

#[test]
fn a_failed_check_takes_back_only_the_steps_the_scenario_queued_itself() {
    use crate::scenario::RunOrigin;
    let scenario = InputScenario::new()
        .expect(1, ScenarioCheck::count("Nothing", 0)) // fails on tick 1
        .duration(10);
    let (mut game, mut ctx) = game_without_scenario();
    ctx.time.pause();
    // Another client queued three ticks, then a tool queued the run's ten.
    for _ in 0..3 {
        ctx.time.step();
    }
    ctx.input.install_scenario(
        &mut ctx.world,
        &mut ctx.time,
        scenario,
        0,
        RunOrigin {
            owner: None,
            tool: true,
            queue_ticks: true,
        },
    );
    // A paused game uses one queued step per tick, as the headless loop does.
    for _ in 0..2 {
        assert!(ctx.take_one_step());
        frame(&mut game, &mut ctx, 1);
    }
    let report = ctx.input.scenario_report(&ctx.world).unwrap();
    assert_eq!(report.status, ScenarioStatus::Failed);
    // 13 queued - 2 run = 11; the 8 unrun ticks of the scenario are gone, the 3 others stay.
    assert_eq!(ctx.take_pending_steps(), 3);
}

#[test]
fn cancelling_takes_back_the_unrun_ticks_the_scenario_queued() {
    use crate::scenario::RunOrigin;
    let (mut game, mut ctx) = game_without_scenario();
    ctx.time.pause();
    ctx.time.step();
    ctx.input.install_scenario(
        &mut ctx.world,
        &mut ctx.time,
        InputScenario::new().duration(10),
        0,
        RunOrigin {
            owner: None,
            tool: true,
            queue_ticks: true,
        },
    );
    assert!(ctx.take_one_step());
    frame(&mut game, &mut ctx, 1);
    assert!(ctx.cancel_scenario());
    assert_eq!(ctx.take_pending_steps(), 1, "only the other client's step is left");
}

#[test]
fn the_window_loop_runs_one_tick_a_frame_while_a_scenario_runs_so_edges_last_one_tick() {
    // A paused game told to run five ticks at once, as a tool does.
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .at(3, InputEvent::Key(Key::Space, false));
    let (mut game, mut ctx) = game_without_scenario();
    let mut clock = Clock::new(60);
    ctx.time.pause();
    ctx.start_scenario(scenario);
    ctx.time.queue_steps(5);
    let mut per_frame = Vec::new();
    for _ in 0..7 {
        let timing = frame_timing(&mut clock, &mut ctx, 1.0 / 60.0);
        per_frame.push(timing.fixed_steps);
        ctx.begin_frame(timing);
        game.run_systems(&mut ctx, timing.fixed_steps);
        ctx.input.end_frame();
    }
    // Five frames of one tick each: what a headless run sees.
    assert_eq!(
        trace(&mut ctx),
        [
            seen(0, false, false, false),
            seen(1, true, true, false),
            seen(2, false, true, false),
            seen(3, false, false, true),
            seen(4, false, false, false),
        ]
    );
    assert_eq!(per_frame, [1, 1, 1, 1, 1, 0, 0]);

    // With no scenario the queued ticks still run in one frame.
    ctx.time.queue_steps(3);
    assert_eq!(frame_timing(&mut clock, &mut ctx, 1.0 / 60.0).fixed_steps, 3);
}

/// One frame as the window loop runs it; returns how many ticks it ran.
fn window_frame(game: &mut Game, ctx: &mut Ctx, clock: &mut Clock, frame_dt: f64) -> u32 {
    let timing = frame_timing(clock, ctx, frame_dt);
    ctx.begin_frame(timing);
    game.run_systems(ctx, timing.fixed_steps);
    ctx.input.end_frame();
    timing.fixed_steps
}

#[test]
fn a_scenario_in_a_window_slower_than_the_tick_rate_leaves_no_backlog_and_no_burst_after_it() {
    // 60 ticks a second drawn at 30 frames a second: two ticks are due
    // every frame. The scenario lasts ten ticks.
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .at(9, InputEvent::Key(Key::Space, false));
    let (mut game, mut ctx) = game_without_scenario();
    let mut clock = Clock::new(60);
    ctx.start_scenario(scenario);
    let mut during = Vec::new();
    while ctx.input.scenarios.run.is_some() {
        during.push(window_frame(&mut game, &mut ctx, &mut clock, 1.0 / 30.0));
        assert_eq!(ctx.take_pending_steps(), 0, "nothing piles up while the scenario runs");
        assert!(during.len() <= 20, "the scenario ends");
    }
    assert_eq!(during, [1; 10], "one tick a frame, ten frames for ten ticks");
    // The frames after it run what is due, not what was skipped.
    let after: Vec<u32> = (0..5)
        .map(|_| window_frame(&mut game, &mut ctx, &mut clock, 1.0 / 30.0))
        .collect();
    assert_eq!(after, [2; 5], "two ticks a frame again, and no burst");
    assert_eq!(ctx.time.sim_tick(), 20);
    // Every tick of the scenario saw the edge once.
    let pressed: Vec<u64> = trace(&mut ctx).iter().filter(|s| s.pressed).map(|s| s.tick).collect();
    assert_eq!(pressed, [1]);
}

#[test]
fn pausing_during_a_scenario_stops_the_game() {
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Space, true))
        .at(50, InputEvent::Key(Key::Space, false));
    let (mut game, mut ctx) = game_without_scenario();
    let mut clock = Clock::new(60);
    ctx.start_scenario(scenario);
    for _ in 0..5 {
        window_frame(&mut game, &mut ctx, &mut clock, 1.0 / 30.0);
    }
    let tick = ctx.time.sim_tick();
    assert_eq!(tick, 5);
    ctx.time.pause();
    for _ in 0..10 {
        assert_eq!(
            window_frame(&mut game, &mut ctx, &mut clock, 1.0 / 30.0),
            0,
            "a paused game runs no tick"
        );
    }
    assert_eq!(ctx.time.sim_tick(), tick);
    assert!(ctx.input.scenarios.run.is_some(), "the scenario waits with the game");
    // One step asked for while paused runs, and only that one.
    ctx.time.step();
    assert_eq!(window_frame(&mut game, &mut ctx, &mut clock, 1.0 / 30.0), 1);
    assert_eq!(window_frame(&mut game, &mut ctx, &mut clock, 1.0 / 30.0), 0);
    assert_eq!(ctx.time.sim_tick(), tick + 1);
}
