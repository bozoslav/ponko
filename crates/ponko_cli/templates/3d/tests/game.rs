//! Headless tests: the game runs without a window or GPU, as fast as the
//! CPU allows. `cargo test` runs them.

use ponko::actions::InputEvent;
use ponko::d3::Transform;
use ponko::prelude::*;
use ponko::scenario::InputScenario;

use {{crate_name}}::{Settings, Spin, game};

/// How far the cube has turned, in radians.
fn cube_angle(world: &mut World) -> f32 {
    let mut angles = Vec::new();
    world
        .query::<(&Transform, &Spin)>()
        .for_each(|(t, _)| angles.push(t.rotation.angle_between(Quat::IDENTITY)));
    assert_eq!(angles.len(), 1, "one spinning cube");
    angles[0]
}

#[test]
fn the_cube_turns_at_the_configured_speed() {
    // One second at the default 60 ticks per second.
    let mut world = game().run_headless(60);
    let speed = world.resource::<Settings>().spin_speed;
    let angle = cube_angle(&mut world);
    assert!(
        (angle - speed).abs() < 1e-3,
        "turned {angle} rad, expected {speed}"
    );
}

#[test]
fn escape_quits() {
    // Presses Escape on the tenth frame of sixty. Headless frames run the
    // frame systems too (`controls`), and the game's files go to a folder of
    // the test's own, so the player's saves are never touched.
    let scenario = InputScenario::new()
        .at(10, InputEvent::Key(Key::Escape, true))
        .duration(60);
    let (mut world, _report) = game()
        .headless_frames(true)
        .data_dir(std::env::temp_dir().join("{{crate_name}}-test-data"))
        .run_headless_scenario(scenario);
    let speed = world.resource::<Settings>().spin_speed;
    let angle = cube_angle(&mut world);
    assert!(angle > 0.0, "it turned before Escape");
    assert!(angle < speed * 0.5, "and the game ended: turned {angle} rad, not a second's worth");
}
