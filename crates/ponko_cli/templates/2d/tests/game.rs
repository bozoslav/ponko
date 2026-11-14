//! Headless tests: the game runs without a window or GPU, as fast as the
//! CPU allows. `cargo test` runs them.

use ponko::actions::InputEvent;
use ponko::prelude::*;
use ponko::scenario::InputScenario;

use {{crate_name}}::{ARENA, Intent, Player, game};

fn player_position(world: &mut World) -> Vec2 {
    let mut at = Vec::new();
    world
        .query::<(&ponko::d2::Transform, &Player)>()
        .for_each(|(t, _)| at.push(t.position));
    assert_eq!(at.len(), 1, "one player");
    at[0]
}

#[test]
fn the_player_starts_in_the_middle() {
    let mut world = game().run_headless(0);
    assert_eq!(player_position(&mut world), ARENA / 2.0);
}

#[test]
fn holding_right_moves_the_player_right() {
    // One second at the default 60 ticks per second.
    let mut world = game()
        .setup(|ctx| ctx.world.resource_mut::<Intent>().0 = vec2(1.0, 0.0))
        .run_headless(60);
    let moved = player_position(&mut world) - ARENA / 2.0;
    assert!(moved.x > 1.0, "moved right: {moved}");
    assert!(moved.y.abs() < 1e-3, "not up or down: {moved}");
}

#[test]
fn the_player_stays_in_the_arena() {
    let mut world = game()
        .setup(|ctx| ctx.world.resource_mut::<Intent>().0 = vec2(-1.0, -1.0))
        .run_headless(60 * 30);
    let p = player_position(&mut world);
    assert!(
        p.x >= 0.0 && p.y >= 0.0 && p.x <= ARENA.x && p.y <= ARENA.y,
        "inside: {p}"
    );
}

#[test]
fn the_right_arrow_key_moves_the_player_right() {
    // Holds Right for half a second. Headless frames run the frame systems
    // too (`read_input`), and the game's files go to a folder of the test's
    // own, so the player's saves are never touched.
    let scenario = InputScenario::new()
        .at(1, InputEvent::Key(Key::Right, true))
        .at(31, InputEvent::Key(Key::Right, false))
        .duration(40);
    let (mut world, report) = game()
        .headless_frames(true)
        .data_dir(std::env::temp_dir().join("{{crate_name}}-test-data"))
        .run_headless_scenario(scenario);
    report.assert_passed();
    let moved = player_position(&mut world) - ARENA / 2.0;
    assert!(moved.x > 1.0, "moved right: {moved}");
}
