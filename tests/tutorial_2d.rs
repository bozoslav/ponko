//! The finished game of the guide's "Tutorial: a first 2D game", played
//! headlessly: the title screen, walking, jumping, landing on the level's
//! tiles, walls, coins, the flag, falling into the water and the saved best
//! score. The game's own systems run, with scheduled key presses and clicks.
#![cfg(feature = "physics")]

#[path = "../examples/tutorial_2d/game.rs"]
mod game;

use game::*; // in your project: `use <your crate>::*;`
// ANCHOR: uses
use ponko::actions::InputEvent;
use ponko::d2::Transform;
use ponko::prelude::*;
use ponko::scenario::{InputScenario, ScenarioCheck};
use std::path::PathBuf;
// ANCHOR_END: uses

// ANCHOR: play
/// A storage folder of the test's own, empty.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-tutorial-2d-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The crab's position.
fn crab(world: &World) -> Vec2 {
    let crab = world.resource::<Stage>().crab;
    world.get::<Transform>(crab).expect("the crab").position
}

/// Plays `scenario` with frames (so the menu and the controls run), keeping
/// the saves in `dir`. Tick 1 clicks Play.
fn play(dir: &PathBuf, scenario: InputScenario) -> World {
    let scenario = scenario.click(1, "menu.play");
    let (world, report) = game()
        .data_dir(dir)
        .headless_frames(true)
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
fn the_crab_lands_on_the_ground() {
    let world = play(&temp_dir("lands"), InputScenario::new().duration(120));
    // The grass is at y = 208 (row 13 of 16-pixel tiles); the crab stands on it.
    let at = crab(&world);
    assert!((at.y - 201.0).abs() < 1.5, "standing on the grass: {at}");
}

#[test]
fn holding_right_walks_the_crab_right() {
    let world = play(
        &temp_dir("walk"),
        hold(InputScenario::new(), Key::Right, 60, 120).duration(130),
    );
    let walked = crab(&world).x - 40.0;
    // One second at 110 pixels per second, give or take the start and stop.
    assert!((90.0..130.0).contains(&walked), "walked {walked} pixels");
}
// ANCHOR_END: play

#[test]
fn the_wall_stops_the_crab() {
    let world = play(
        &temp_dir("wall"),
        hold(InputScenario::new(), Key::Left, 60, 240).duration(250),
    );
    // The stone wall is the first column of tiles, 16 pixels wide.
    let x = crab(&world).x;
    assert!((20.0..26.0).contains(&x), "stopped at the wall: {x}");
}

#[test]
fn space_jumps_from_the_ground_only() {
    let scenario = InputScenario::new()
        .at(60, InputEvent::Key(Key::Space, true))
        .at(62, InputEvent::Key(Key::Space, false))
        .duration(75);
    let world = play(&temp_dir("jump"), scenario);
    assert!(crab(&world).y < 170.0, "in the air: {}", crab(&world));

    // Pressing again in the air does nothing; the crab comes back down.
    let scenario = InputScenario::new()
        .at(60, InputEvent::Key(Key::Space, true))
        .at(62, InputEvent::Key(Key::Space, false))
        .at(75, InputEvent::Key(Key::Space, true))
        .at(77, InputEvent::Key(Key::Space, false))
        .duration(150);
    let world = play(&temp_dir("jump-twice"), scenario);
    assert!((crab(&world).y - 201.0).abs() < 1.5, "landed: {}", crab(&world));
}

/// Moves the crab: each `(tick, place)` puts it there on that tick (setting
/// a body's transform moves it). Play (tick 1) starts the level over, so
/// moves come after it.
fn moves(list: &[(u64, Vec2)]) -> impl FnMut(&mut Ctx) + 'static {
    let list = list.to_vec();
    let mut tick = 0;
    move |ctx: &mut Ctx| {
        for &(at, to) in &list {
            if at == tick {
                let crab = ctx.world.resource::<Stage>().crab;
                ctx.world.get_mut::<Transform>(crab).unwrap().position = to;
            }
        }
        tick += 1;
    }
}

/// [`play`] with the crab moved by [`moves`].
fn play_moved(dir: &PathBuf, scenario: InputScenario, list: &[(u64, Vec2)]) -> World {
    let scenario = scenario.click(1, "menu.play");
    let (world, report) = game()
        .data_dir(dir)
        .headless_frames(true)
        .sim_system(moves(list))
        .run_headless_scenario(scenario);
    report.assert_passed();
    world
}

#[test]
fn touching_a_coin_collects_it_and_touching_the_flag_finishes() {
    let dir = temp_dir("coins");
    // The first two coins float over the stone platform at x 128..192:
    // drop the crab onto it, then walk right along it.
    let walk = hold(InputScenario::new(), Key::Right, 20, 30).duration(60);
    let mut world = play_moved(&dir, walk, &[(5, vec2(150.0, 140.0))]);
    assert_eq!(world.resource::<Score>().coins, 2, "both coins on the platform");
    assert_eq!(coins_left(&mut world), 10);

    // Next to the flag: touching it ends the level, starts it over and
    // shows the result on the title screen.
    let scenario = InputScenario::new()
        .expect(
            40,
            ScenarioCheck::ui("menu.message")
                .shown(true)
                .text("You found 0 of 12 coins!"),
        )
        .duration(41);
    let world = play_moved(&dir, scenario, &[(5, vec2(902.0, 190.0))]);
    assert!(crab(&world).x < 100.0, "back at the start: {}", crab(&world));
}

#[test]
fn falling_into_the_water_starts_over() {
    // Over the first pit of water (x 288..352).
    let world = play_moved(
        &temp_dir("water"),
        InputScenario::new().duration(90),
        &[(5, vec2(320.0, 180.0))],
    );
    let at = crab(&world);
    assert!(at.x < 100.0 && (at.y - 201.0).abs() < 1.5, "back at the start: {at}");
}

// ANCHOR: save_test
#[test]
fn the_best_score_is_saved_and_loaded() {
    let dir = temp_dir("save");
    // Take the two coins over the platform, then touch the flag.
    let scenario = hold(InputScenario::new(), Key::Right, 20, 30)
        .expect(3, ScenarioCheck::ui("menu.best").text(""))
        .duration(60);
    let route = [(5, vec2(150.0, 140.0)), (40, vec2(902.0, 190.0))];
    let world = play_moved(&dir, scenario, &route);
    assert_eq!(world.resource::<Progress>(), &Progress { best: 2, wins: 1 });

    // The file is in the game's storage folder...
    let saved = ponko::storage::Storage::in_folder(&dir)
        .read_json::<Progress>(PROGRESS_FILE)
        .unwrap();
    assert_eq!(saved, Progress { best: 2, wins: 1 });

    // ...and the next run loads it and shows it on the title screen.
    let scenario = InputScenario::new()
        .expect(0, ScenarioCheck::ui("menu.best").shown(true).text("Best: 2 coins"))
        .duration(1);
    let (world, report) = game()
        .data_dir(&dir)
        .headless_frames(true)
        .run_headless_scenario(scenario);
    report.assert_passed();
    assert_eq!(world.resource::<Progress>().best, 2);
}
// ANCHOR_END: save_test

#[test]
fn the_game_waits_on_the_title_screen_until_play() {
    let scenario = InputScenario::new()
        .expect(0, ScenarioCheck::ui("menu.play").shown(true))
        .click(3, "menu.play")
        .expect(3, ScenarioCheck::ui("menu.play").shown(false))
        // Escape opens it again, and closes it.
        .at(10, InputEvent::Key(Key::Escape, true))
        .at(11, InputEvent::Key(Key::Escape, false))
        .expect(11, ScenarioCheck::ui("menu.message").shown(true).text("Paused"))
        .at(20, InputEvent::Key(Key::Escape, true))
        .at(21, InputEvent::Key(Key::Escape, false))
        .expect(21, ScenarioCheck::ui("menu.play").shown(false));
    let (_world, report) = game()
        .data_dir(temp_dir("menu"))
        .headless_frames(true)
        .run_headless_scenario(scenario);
    report.assert_passed();
}

#[test]
fn the_title_screen_opens_on_play_so_enter_starts_the_game() {
    let scenario = InputScenario::new()
        .expect(0, ScenarioCheck::ui("menu.play").shown(true))
        .at(3, InputEvent::Key(Key::Enter, true))
        .at(4, InputEvent::Key(Key::Enter, false))
        .expect(3, ScenarioCheck::ui("menu.play").shown(false))
        // Opened again by Escape, it starts on Play again.
        .at(10, InputEvent::Key(Key::Escape, true))
        .at(11, InputEvent::Key(Key::Escape, false))
        .at(14, InputEvent::Key(Key::Enter, true))
        .at(15, InputEvent::Key(Key::Enter, false))
        .expect(14, ScenarioCheck::ui("menu.play").shown(false));
    let (_world, report) = game()
        .data_dir(temp_dir("enter"))
        .headless_frames(true)
        .run_headless_scenario(scenario);
    report.assert_passed();
}
