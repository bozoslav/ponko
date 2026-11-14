//! The finished game of the guide's "Tutorial: a first 3D game", walked
//! headlessly: standing on the glTF yard, walking, turning, a wall that
//! stops the walker, the ramp, lanterns lit by walking into them, the HUD
//! and starting over. The game's own systems run, with scheduled key
//! presses and clicks.
#![cfg(feature = "physics3d")]

#[path = "../examples/tutorial_3d/game.rs"]
mod game;

use game::*; // in your project: `use <your crate>::*;`
// ANCHOR: uses
use ponko::actions::InputEvent;
use ponko::d3::{Character, Transform};
use ponko::prelude::*;
use ponko::scenario::{InputScenario, ScenarioCheck};
// ANCHOR_END: uses

// ANCHOR: walk
/// The walker's feet, and whether it stands on something.
fn walker(world: &World) -> (Vec3, bool) {
    let e = world.resource::<Player>().entity;
    let at = world.get::<Transform>(e).expect("the walker").position;
    (at, world.get::<Character>(e).expect("a character").grounded)
}

/// Plays `scenario` with frames, so the controls and the HUD run. `start`
/// moves the walker before the first tick.
fn play(scenario: InputScenario, start: Vec3) -> World {
    let (world, report) = game()
        .headless_frames(true)
        .setup(move |ctx| {
            let e = ctx.world.resource::<Player>().entity;
            ctx.world.get_mut::<Transform>(e).unwrap().position = start;
        })
        .run_headless_scenario(scenario);
    report.assert_passed();
    world
}

/// Holds `key` from tick `from` to tick `to`.
fn hold(s: InputScenario, key: Key, from: u64, to: u64) -> InputScenario {
    s.at(from, InputEvent::Key(key, true))
        .at(to, InputEvent::Key(key, false))
}

#[test]
fn the_walker_stands_on_the_yard() {
    let world = play(InputScenario::new().duration(60), START + Vec3::Y);
    let (at, grounded) = walker(&world);
    assert!(grounded && at.y.abs() < 0.05, "standing on the ground: {at}");
}

#[test]
fn holding_w_walks_forward() {
    // 50 ticks along -Z, the way the walker faces at the start. With 3D
    // physics the game ticks 50 times a second: one second at 4 m/s.
    let world = play(hold(InputScenario::new(), Key::W, 10, 60).duration(70), START);
    let (at, grounded) = walker(&world);
    let walked = START.z - at.z;
    assert!((3.8..4.2).contains(&walked), "walked {walked} m: {at}");
    assert!(grounded && at.x.abs() < 0.01, "straight, on the ground: {at}");
}

#[test]
fn the_pillar_stops_the_walker() {
    // Four seconds straight ahead: the pillar's face is at z = -6.
    let world = play(hold(InputScenario::new(), Key::W, 10, 250).duration(260), START);
    let (at, _) = walker(&world);
    assert!((-5.7..-5.5).contains(&at.z), "stopped at the pillar: {at}");
}
// ANCHOR_END: walk

#[test]
fn turning_changes_where_forward_is() {
    // Turn left for a while, then walk: forward is no longer straight -Z.
    let scenario = hold(InputScenario::new(), Key::Q, 5, 35);
    let scenario = hold(scenario, Key::W, 40, 100).duration(110);
    let world = play(scenario, START);
    let (at, _) = walker(&world);
    assert!(at.x < -1.0, "walked off to the left: {at}");
}

#[test]
fn the_ramp_leads_up_to_the_platform_and_its_lantern() {
    // In front of the ramp (x 5..9, rising toward -Z), walking ahead.
    let scenario = hold(InputScenario::new(), Key::W, 10, 160)
        .expect(5, ScenarioCheck::ui("hud.count").text("Lanterns lit: 0 / 5"))
        .duration(170);
    let mut world = play(scenario, vec3(7.0, 0.0, 2.0));
    let (at, grounded) = walker(&world);
    assert!(grounded && (at.y - 1.5).abs() < 0.05, "on the platform: {at}");
    assert_eq!(lanterns_lit(&mut world), (1, 5), "walked through its lantern");
}

#[test]
fn crates_fall_and_rest_on_the_ground() {
    let mut world = play(InputScenario::new().duration(180), START);
    let heights: Vec<f32> = world
        .query::<(&Transform, &ponko::d3::Velocity)>()
        .iter()
        .map(|(t, _)| t.position.y)
        .collect();
    assert_eq!(heights.len(), 3, "three crates");
    // Two on the ground, one on top of them.
    let resting = heights.iter().filter(|&&y| (y - 0.4).abs() < 0.05).count();
    assert_eq!(resting, 2, "two crates on the ground: {heights:?}");
}

// ANCHOR: all_lit
#[test]
fn lighting_every_lantern_opens_the_panel_and_play_again_starts_over() {
    // Step into each lantern's light in turn, a few ticks apart.
    let spots = LANTERN_SPOTS;
    let mut tick = 0;
    let visit = move |ctx: &mut Ctx| {
        tick += 1;
        if tick % 10 == 0 && tick / 10 <= spots.len() {
            let e = ctx.world.resource::<Player>().entity;
            ctx.world.get_mut::<Transform>(e).unwrap().position = spots[tick / 10 - 1];
        }
    };
    let scenario = InputScenario::new()
        .expect(45, ScenarioCheck::ui("hud.count").text("Lanterns lit: 4 / 5"))
        .expect(45, ScenarioCheck::ui("hud.done").shown(false))
        .expect(58, ScenarioCheck::ui("hud.count").text("Lanterns lit: 5 / 5"))
        .expect(58, ScenarioCheck::ui("hud.done").shown(true))
        .click(60, "hud.again")
        .expect(60, ScenarioCheck::ui("hud.done").shown(false))
        .expect(60, ScenarioCheck::ui("hud.count").text("Lanterns lit: 0 / 5"))
        .duration(62);
    let (mut world, report) = game()
        .headless_frames(true)
        .sim_system(visit)
        .run_headless_scenario(scenario);
    report.assert_passed();
    assert_eq!(lanterns_lit(&mut world), (0, 5));
    let (at, _) = walker(&world);
    assert!(at.distance(START) < 0.2, "back at the start: {at}");
}
// ANCHOR_END: all_lit

/// Lights every lantern at once, so the panel opens on the first frame.
fn light_all(ctx: &mut Ctx) {
    let posts: Vec<Entity> = ctx.world.query::<(Entity, &Lantern)>().iter().map(|(e, _)| e).collect();
    for post in posts {
        set_lit(ctx, post, true);
    }
}

#[test]
fn escape_closes_the_panel_and_leaves_the_counter_up() {
    // Escape closes the panel (and starts over); the counter, a HUD, stays.
    let scenario = InputScenario::new()
        .expect(3, ScenarioCheck::ui("hud.done").shown(true))
        .expect(3, ScenarioCheck::ui("hud.count").shown(true))
        .at(5, InputEvent::Key(Key::Escape, true))
        .at(6, InputEvent::Key(Key::Escape, false))
        .expect(5, ScenarioCheck::ui("hud.done").shown(false))
        .expect(
            6,
            ScenarioCheck::ui("hud.count").shown(true).text("Lanterns lit: 0 / 5"),
        )
        .duration(10);
    let (_world, report) = game()
        .headless_frames(true)
        .setup(light_all)
        .run_headless_scenario(scenario);
    report.assert_passed();
}

#[test]
fn the_panel_opens_on_play_again_so_enter_starts_over() {
    let scenario = InputScenario::new()
        .expect(3, ScenarioCheck::ui("hud.done").shown(true))
        .at(5, InputEvent::Key(Key::Enter, true))
        .at(6, InputEvent::Key(Key::Enter, false))
        .expect(5, ScenarioCheck::ui("hud.done").shown(false))
        .expect(5, ScenarioCheck::ui("hud.count").text("Lanterns lit: 0 / 5"))
        .duration(8);
    let (_world, report) = game()
        .headless_frames(true)
        .setup(light_all)
        .run_headless_scenario(scenario);
    report.assert_passed();
}
