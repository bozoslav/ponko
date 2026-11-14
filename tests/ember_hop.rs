//! The `ember_hop` reference platformer, played headlessly: running and
//! jumping on the level's colliders, sparks, the flare and stomping drips,
//! hazards, braziers that save and losses that load, the way out to the next
//! level and the end of a run, continuing a saved run, the progress file, the
//! camera, the input mapping (keys, gamepad and "any key" through headless
//! frames), and a whole run played by the autopilot. No window, no GPU.
//!
//! The tests drive the game's own plugin (`examples/ember_hop/game.rs`) with
//! a scripted player that writes the `Intent` before the rules run.
#![cfg(feature = "physics")]

#[path = "../examples/ember_hop/game.rs"]
mod game;

use game::*;
use ponko::actions::InputEvent;
use ponko::d2::{LevelObject, LevelRoot, TileMap, Transform, Velocity};
use ponko::input::{Gamepad, GamepadAxis, GamepadButton};
use ponko::prelude::*;
use ponko::scenario::InputScenario;
use std::path::PathBuf;

/// A fresh data folder for one test.
fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-ember-hop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// ANCHOR: script
/// The game with its files in `dir` and `script` deciding, each tick before
/// the rules, what the player does.
fn scripted(dir: &std::path::Path, script: impl FnMut(&mut Ctx) + 'static) -> Game {
    Game::new("ember hop test", 960, 540)
        .data_dir(dir)
        .seed(SEED)
        .sim_system(script)
        .plugin(EmberHop)
}
// ANCHOR_END: script

fn hero(ctx: &mut Ctx) -> Option<(Entity, Vec2)> {
    ctx.world
        .query::<(Entity, &Transform, &Player)>()
        .iter()
        .next()
        .map(|(e, t, _)| (e, t.position))
}

fn hero_in(world: &mut World) -> Option<Vec2> {
    world
        .query::<(&Transform, &Player)>()
        .iter()
        .next()
        .map(|(t, _)| t.position)
}

/// Puts the hero at `at`, at rest (the body teleports there).
fn teleport(ctx: &mut Ctx, at: Vec2) {
    if let Some((e, _)) = hero(ctx) {
        ctx.world.get_mut::<Transform>(e).expect("a transform").position = at;
        ctx.world.get_mut::<Velocity>(e).expect("a velocity").linear = Vec2::ZERO;
    }
}

/// Where the first entity with component `T` is.
fn first<T: Component>(ctx: &mut Ctx) -> Option<Vec2> {
    ctx.world
        .query::<(&Transform, &T)>()
        .iter()
        .next()
        .map(|(t, _)| t.position)
}

fn screen(world: &World) -> Screen {
    *world.resource::<Screen>()
}

fn intent(ctx: &mut Ctx) -> &mut Intent {
    ctx.world.resource_mut::<Intent>()
}

#[test]
fn the_game_starts_on_the_title_scene() {
    let dir = data_dir("title");
    let mut world = scripted(&dir, |_| {}).run_headless(2, |_| {});
    assert_eq!(screen(&world), Screen::Title);
    // The title is a scene file: its sprites, a lit brazier and the embers.
    assert!(world.query::<&Sprite>().iter().count() > 20);
    assert_eq!(world.query::<&Checkpoint>().iter().count(), 1);
    assert_eq!(world.query::<&ponko::d2::ParticleEmitter>().iter().count(), 1);
    assert!(hero_in(&mut world).is_none(), "no hero on the title");
}

#[test]
fn a_level_spawns_from_its_file_with_autotiled_ground_prefabs_and_sensors() {
    let dir = data_dir("level");
    let mut world = scripted(&dir, |ctx| {
        if ctx.time.sim_tick() == 1 {
            new_run(ctx);
        }
    })
    .run_headless(3, |_| {});
    assert_eq!(screen(&world), Screen::Playing);
    assert_eq!(world.resource::<CurrentLevel>().title, "The Mossy Hollow");
    assert_eq!(world.resource::<CurrentLevel>().size, vec2(64.0 * 16.0, 18.0 * 16.0));
    // Every spark of the file is a prefab with its component.
    let sparks = world.query::<&Spark>().iter().count();
    assert_eq!(sparks, 11);
    assert_eq!(world.resource::<Run>().level_total, 11);
    assert_eq!(world.query::<&Drip>().iter().count(), 3);
    assert_eq!(world.query::<&Checkpoint>().iter().count(), 1);
    // The way out is a prefab with children: the lantern and its embers.
    let parts = world
        .query::<(&Exit, &PrefabParts)>()
        .iter()
        .map(|(_, p)| p.0.len())
        .next();
    assert_eq!(parts, Some(2));
    // The ground is one rock tile in the file; autotiling picks the piece by
    // its neighbours (N 1, E 2, S 4, W 8). The start stands on open ground: the
    // cell under it has rock east, south and west but not north.
    let ground = world
        .query::<(&TileMap, &ponko::d2::LevelLayer)>()
        .iter()
        .find(|(_, l)| l.name == "Ground")
        .map(|(m, _)| m.clone())
        .expect("a ground layer");
    assert_eq!(ground.get(3, 15), Some(2 | 4 | 8));
    assert_eq!(ground.get(3, 16), Some(15), "inside the rock: all four neighbours");
    // Hazards are sensors.
    let hazards = world.query::<(&Hazard, &ponko::d2::Collider)>().iter().count();
    assert!(hazards >= 3, "{hazards} hazard sensors");
    assert!(world.query::<&LevelRoot>().iter().count() == 1);
}

#[test]
fn the_hero_runs_on_the_ground_and_faces_where_it_goes() {
    let dir = data_dir("run");
    let start = std::rc::Rc::new(std::cell::Cell::new(Vec2::ZERO));
    let seen = start.clone();
    let mut world = scripted(&dir, move |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 10 {
            seen.set(hero(ctx).expect("a hero").1);
        }
        if (10..70).contains(&tick) {
            intent(ctx).move_x = 1.0;
        } else if (70..90).contains(&tick) {
            intent(ctx).move_x = -1.0;
        }
    })
    .run_headless(90, |_| {});
    let start = start.get();
    let at = hero_in(&mut world).expect("a hero");
    assert!(at.x > start.x + 50.0, "ran right: {start} -> {at}");
    assert!((at.y - start.y).abs() < 1.0, "stayed on the ground: {start} -> {at}");
    let (player, scale) = world
        .query::<(&Player, &Transform)>()
        .iter()
        .map(|(p, t)| (p.clone(), t.scale.x))
        .next()
        .unwrap();
    assert!(player.grounded);
    assert_eq!(player.facing, -1.0, "turned to run back left");
    assert_eq!(scale, -1.0, "the sprite is mirrored");
}

/// Highest point of a jump held for `hold` ticks, and where it landed.
fn jump_height(name: &str, hold: u64) -> (f32, Vec2, Vec2, Tally) {
    let dir = data_dir(name);
    let top = std::rc::Rc::new(std::cell::Cell::new((f32::MAX, Vec2::ZERO)));
    let seen = top.clone();
    let mut world = scripted(&dir, move |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 20 {
            let at = hero(ctx).unwrap().1;
            seen.set((f32::MAX, at));
            intent(ctx).jump = true;
        }
        intent(ctx).jump_held = (20..20 + hold).contains(&tick);
        if tick > 20
            && let Some((_, at)) = hero(ctx)
        {
            let (best, start) = seen.get();
            seen.set((best.min(at.y), start));
        }
    })
    .run_headless(120, |_| {});
    let (best, start) = top.get();
    let tally = world.resource::<Tally>().clone();
    (start.y - best, start, hero_in(&mut world).unwrap(), tally)
}

#[test]
fn a_held_jump_clears_three_tiles_and_lands_again() {
    let (height, start, end, tally) = jump_height("jump", 60);
    assert_eq!(tally.jumps, 1);
    assert!(height > 48.0 && height < 60.0, "jumped {height} pixels");
    assert!(
        (end.y - start.y).abs() < 1.0,
        "landed where it started: {start} -> {end}"
    );
}

#[test]
fn letting_go_early_makes_a_short_hop() {
    let (high, ..) = jump_height("hop-high", 60);
    let (low, ..) = jump_height("hop-low", 3);
    assert!(low < high * 0.6, "a tap jumps {low}, a hold {high}");
}

#[test]
fn touching_a_spark_collects_it() {
    let dir = data_dir("spark");
    let world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 5 {
            let spark = first::<Spark>(ctx).expect("a spark");
            teleport(ctx, spark);
        }
    })
    .run_headless(12, |_| {});
    let run = world.resource::<Run>();
    assert_eq!((run.sparks, run.level_sparks), (1, 1));
    assert_eq!(run.level_total, 11);
}

#[test]
fn the_flare_burns_drips_close_by() {
    let dir = data_dir("flare");
    let mut world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 5 {
            let drip = first::<Drip>(ctx).expect("a drip");
            teleport(ctx, drip + vec2(-20.0, -8.0));
        }
        if tick == 7 {
            intent(ctx).flare = true;
        }
    })
    .run_headless(9, |_| {});
    assert_eq!(world.resource::<Tally>().burned, 1);
    assert_eq!(world.query::<&Drip>().iter().count(), 2);
    assert_eq!(screen(&world), Screen::Playing, "the hero is fine");
}

#[test]
fn landing_on_a_drip_stomps_it_and_bounces() {
    let dir = data_dir("stomp");
    let world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 5 {
            let drip = first::<Drip>(ctx).expect("a drip");
            teleport(ctx, drip + vec2(0.0, -40.0));
            // Hold the drips still so the hero lands right on this one.
            for (_, d) in ctx.world.query::<(Entity, &mut Drip)>().iter() {
                d.speed = 0.0;
            }
        }
    })
    .run_headless(40, |_| {});
    let tally = world.resource::<Tally>().clone();
    assert_eq!((tally.stomps, tally.burned), (1, 1));
    assert_eq!(screen(&world), Screen::Playing);
    assert_eq!(world.resource::<Run>().deaths, 0);
}

#[test]
fn walking_into_a_drip_loses_the_hero_and_starts_the_level_again() {
    let dir = data_dir("drip");
    let start = std::rc::Rc::new(std::cell::Cell::new(Vec2::ZERO));
    let seen = start.clone();
    let mut world = scripted(&dir, move |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
            seen.set(hero(ctx).unwrap().1);
        }
        if tick == 5 {
            let spark = first::<Spark>(ctx).unwrap();
            teleport(ctx, spark);
        }
        if tick == 10 {
            let drip = first::<Drip>(ctx).expect("a drip");
            teleport(ctx, drip + vec2(-2.0, 0.0));
        }
    })
    .run_headless(80, |_| {});
    assert_eq!(screen(&world), Screen::Playing, "back after the loss");
    let run = world.resource::<Run>().clone();
    assert_eq!(run.deaths, 1);
    assert_eq!(
        (run.sparks, run.level_sparks),
        (0, 0),
        "the spark is back: the level's start was saved"
    );
    assert_eq!(world.query::<&Spark>().iter().count(), 11);
    assert_eq!(world.resource::<Tally>().loads, 1);
    let at = hero_in(&mut world).unwrap();
    assert!(at.distance(start.get()) < 2.0, "back at the start: {at}");
}

#[test]
fn spikes_water_and_the_fall_are_hazards() {
    for (name, kind) in [("spikes", 0), ("water", 1), ("fall", 2)] {
        let dir = data_dir(name);
        let world = scripted(&dir, move |ctx| {
            let tick = ctx.time.sim_tick();
            if tick == 1 {
                new_run(ctx);
            }
            if tick == 5 {
                let at = match kind {
                    // The spike pit is the hazard below the brazier's floor.
                    0 | 1 => {
                        let hazards: Vec<LevelObject> = ctx
                            .world
                            .query::<&LevelObject>()
                            .iter()
                            .filter(|o| o.class == "Hazard")
                            .cloned()
                            .collect();
                        // Spikes are half a tile high, water three quarters.
                        let pick = hazards
                            .iter()
                            .find(|o| (o.size.y < 10.0) == (kind == 0))
                            .expect("a hazard of that kind");
                        pick.position
                    }
                    _ => vec2(16.0 * 11.0, ctx.world.resource::<CurrentLevel>().size.y + 48.0),
                };
                teleport(ctx, at);
            }
        })
        .run_headless(8, |_| {});
        assert!(
            matches!(screen(&world), Screen::Dying { .. }),
            "{name}: {:?}",
            screen(&world)
        );
    }
}

#[test]
fn a_lit_brazier_saves_the_run_and_a_loss_returns_there() {
    let dir = data_dir("brazier");
    let brazier = std::rc::Rc::new(std::cell::Cell::new(Vec2::ZERO));
    let seen = brazier.clone();
    let mut world = scripted(&dir, move |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 5 {
            // A spark before the brazier counts in the save.
            let spark = first::<Spark>(ctx).unwrap();
            teleport(ctx, spark);
        }
        if tick == 10 {
            let at = first::<Checkpoint>(ctx).unwrap();
            seen.set(at);
            teleport(ctx, at);
        }
        if tick == 20 {
            // One after it doesn't: it comes back.
            let at = seen.get();
            let spark = ctx
                .world
                .query::<(&Transform, &Spark)>()
                .iter()
                .map(|(t, _)| t.position)
                .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
                .unwrap();
            teleport(ctx, spark);
        }
        if tick == 30 {
            let drip = first::<Drip>(ctx).unwrap();
            teleport(ctx, drip);
        }
    })
    .run_headless(100, |_| {});
    assert_eq!(screen(&world), Screen::Playing);
    let run = world.resource::<Run>().clone();
    assert_eq!(run.deaths, 1);
    assert_eq!(run.sparks, 1, "the spark collected before the brazier stays");
    assert!(run.time > 0.7, "the clock kept counting through the load: {}", run.time);
    let tally = world.resource::<Tally>().clone();
    assert_eq!((tally.checkpoints, tally.saves, tally.loads), (1, 2, 1));
    let lit: Vec<bool> = world.query::<&Checkpoint>().iter().map(|c| c.lit).collect();
    assert_eq!(lit, [true], "the brazier is still lit");
    let at = hero_in(&mut world).unwrap();
    assert!(at.distance(brazier.get()) < 4.0, "back at the brazier: {at}");
    // The save is the engine's world save, in the slot's file.
    let save: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("saves/run.json")).expect("a save slot")).unwrap();
    assert_eq!(save["format"], "ponko-world");
    assert_eq!(save["resources"]["Run"]["sparks"], 1);
    // It holds the hero and the prefab objects, not the level's tiles and
    // colliders (the engine leaves those out), and their animations.
    for e in save["entities"].as_array().unwrap() {
        let c = &e["components"];
        let ours = [
            "Player",
            "Spark",
            "Drip",
            "Checkpoint",
            "Exit",
            "Attached",
            "ParticleEmitter2D",
        ];
        assert!(
            ours.iter().any(|name| c.get(name).is_some()),
            "level geometry saved: {c}"
        );
        if c.get("Spark").is_some() || c.get("Player").is_some() {
            assert!(c.get("Animation").is_some(), "the animation is saved: {c}");
        }
    }
    // After the load: the level once, the lit brazier still burning.
    assert_eq!(world.query::<&LevelRoot>().iter().count(), 1);
    let brazier_frames: Vec<(u32, u32)> = world
        .query::<(&Checkpoint, &ponko::d2::Animation)>()
        .iter()
        .map(|(_, a)| (a.first, a.frames))
        .collect();
    assert_eq!(
        brazier_frames,
        [(1, 2)],
        "the lit brazier's flames came back with the save"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_way_out_leads_to_the_next_level() {
    let dir = data_dir("exit");
    let mut world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 5 {
            let exit = first::<Exit>(ctx).unwrap();
            teleport(ctx, exit);
        }
    })
    .run_headless(12, |_| {});
    assert!(matches!(screen(&world), Screen::Transition { to: Some(1), .. }));
    // Halfway through the fade the next level is there.
    let dir2 = data_dir("exit2");
    let mut world2 = scripted(&dir2, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 5 {
            let exit = first::<Exit>(ctx).unwrap();
            teleport(ctx, exit);
        }
    })
    .run_headless(80, |_| {});
    assert_eq!(screen(&world2), Screen::Playing);
    assert_eq!(world2.resource::<Run>().level, 1);
    assert_eq!(world2.resource::<CurrentLevel>().title, "The Dripping Deep");
    assert_eq!(world2.resource::<Tally>().levels_cleared, 1);
    assert_eq!(
        world2.query::<&LevelRoot>().iter().count(),
        1,
        "the first level is gone"
    );
    assert!(hero_in(&mut world2).is_some());
    let _ = hero_in(&mut world);
}

#[test]
fn clearing_the_last_level_records_the_run_and_deletes_its_save() {
    let dir = data_dir("finish");
    let world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
            enter_level(ctx, 1);
        }
        if tick == 5 {
            let exit = first::<Exit>(ctx).unwrap();
            teleport(ctx, exit);
        }
    })
    .run_headless(80, |_| {});
    assert_eq!(screen(&world), Screen::Finished);
    let progress = world.resource::<Progress>().clone();
    assert_eq!(progress.runs_finished, 1);
    assert!(progress.best_time.is_some());
    assert!(progress.last_new_best);
    let file: Progress =
        serde_json::from_slice(&std::fs::read(dir.join(PROGRESS_FILE)).expect("progress saved")).unwrap();
    assert_eq!(file.runs_finished, 1);
    assert!(!dir.join("saves/run.json").exists(), "the finished run's slot is gone");
    let lines = screen_lines(&world, &ponko::storage::Storage::in_folder(&dir));
    assert!(lines.iter().any(|l| l.text.contains("A new best")), "{lines:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn continue_picks_up_the_saved_run_from_the_title() {
    let dir = data_dir("continue");
    let brazier = std::rc::Rc::new(std::cell::Cell::new(Vec2::ZERO));
    let seen = brazier.clone();
    let mut world = scripted(&dir, move |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 3 {
            intent(ctx).confirm = true;
        }
        if tick == 8 {
            let spark = first::<Spark>(ctx).unwrap();
            teleport(ctx, spark);
        }
        if tick == 14 {
            let at = first::<Checkpoint>(ctx).unwrap();
            seen.set(at);
            teleport(ctx, at);
        }
        if tick == 20 {
            intent(ctx).pause = true;
        }
        if tick == 22 {
            intent(ctx).back = true;
        }
        if tick == 24 {
            assert_eq!(*ctx.world.resource::<Screen>(), Screen::Title);
            assert!(has_saved_run(&ctx.storage));
            intent(ctx).resume_run = true;
        }
    })
    .run_headless(30, |_| {});
    assert_eq!(screen(&world), Screen::Playing);
    assert_eq!(world.resource::<Run>().sparks, 1);
    let at = hero_in(&mut world).unwrap();
    assert!(at.distance(brazier.get()) < 4.0, "continued at the brazier: {at}");
    assert_eq!(world.query::<&LevelRoot>().iter().count(), 1);
    assert_eq!(
        world.query::<&Exit>().iter().count(),
        1,
        "one way out, not the saved one and the file's"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pausing_holds_the_physics_step() {
    let dir = data_dir("pause");
    let mut world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            new_run(ctx);
        }
        if tick == 5 {
            // In the air.
            let at = hero(ctx).unwrap().1;
            teleport(ctx, at + vec2(0.0, -40.0));
            intent(ctx).pause = true;
        }
    })
    .run_headless(60, |_| {});
    assert_eq!(screen(&world), Screen::Paused);
    let at = hero_in(&mut world).unwrap();
    let level = world.resource::<CurrentLevel>().clone();
    let _ = level;
    let start = 14.0 * 16.0 + 8.0 - 40.0;
    assert!((at.y - start).abs() < 3.0, "still in the air: {at}");
}

#[test]
fn the_title_quits_on_escape() {
    let dir = data_dir("quit");
    let ticks = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = ticks.clone();
    scripted(&dir, move |ctx| {
        count.set(ctx.time.sim_tick());
        if ctx.time.sim_tick() == 3 {
            intent(ctx).pause = true;
        }
    })
    .run_headless(50, |_| {});
    assert!(ticks.get() < 10, "quit after {} ticks", ticks.get());
}

#[test]
fn a_damaged_progress_file_is_set_aside() {
    let dir = data_dir("damaged");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(PROGRESS_FILE), b"{ not json").unwrap();
    let world = scripted(&dir, |_| {}).run_headless(2, |_| {});
    assert!(matches!(world.resource::<LoadStatus>(), LoadStatus::Recovered(_)));
    assert_eq!(std::fs::read(dir.join("progress.json.bad")).unwrap(), b"{ not json");
    let lines = screen_lines(&world, &ponko::storage::Storage::in_folder(&dir));
    assert!(lines.iter().any(|l| l.text.contains("damaged")), "{lines:?}");
    let _ = std::fs::remove_dir_all(dir);
}

// ANCHOR: input_test
#[test]
fn keys_and_gamepad_drive_the_hero_through_headless_frames() {
    let dir = data_dir("input");
    let pad = Gamepad::from_slot(0).unwrap();
    let key = |tick, key, down| (tick, InputEvent::Key(key, down));
    let mut scenario = InputScenario::new();
    for (tick, event) in [
        key(5, Key::Enter, true), // the title: a new run
        key(6, Key::Enter, false),
        key(20, Key::Right, true), // run right
        key(60, Key::Right, false),
        key(70, Key::Space, true), // jump
        key(90, Key::Space, false),
    ] {
        scenario = scenario.at(tick, event);
    }
    scenario = scenario
        .at(130, InputEvent::GamepadAxis(pad, GamepadAxis::LeftStickX, -1.0)) // the stick: left
        .at(160, InputEvent::GamepadAxis(pad, GamepadAxis::LeftStickX, 0.0))
        .at(170, InputEvent::GamepadButton(pad, GamepadButton::South, true)) // jump
        .at(175, InputEvent::GamepadButton(pad, GamepadButton::South, false))
        .at(220, InputEvent::GamepadButton(pad, GamepadButton::Start, true)) // pause
        .at(221, InputEvent::GamepadButton(pad, GamepadButton::Start, false))
        .duration(230);
    let track = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = track.clone();
    let (world, report) = Game::new("ember hop input", 960, 540)
        .data_dir(&dir)
        .seed(SEED)
        .headless_frames(true) // frames too: `read_input` and the camera run
        .plugin(EmberHop)
        .sim_system(move |ctx: &mut Ctx| {
            if let Some((_, at)) = hero(ctx) {
                log.borrow_mut().push((ctx.time.sim_tick(), at, ctx.camera.center()));
            }
        })
        .run_headless_scenario(scenario, |_| {});
    report.assert_passed();
    let track = track.borrow();
    let at = |tick: u64| track.iter().find(|(t, ..)| *t == tick).map(|(_, p, _)| *p).unwrap();
    assert!(at(60).x > at(20).x + 40.0, "the keys ran right");
    assert!(at(80).y < at(69).y - 30.0, "Space jumped");
    assert!(at(160).x < at(130).x - 30.0, "the stick ran left");
    assert!(at(185).y < at(169).y - 20.0, "South jumped");
    assert_eq!(world.resource::<Tally>().jumps, 2);
    assert_eq!(screen(&world), Screen::Paused, "Start paused");
    // Near the level's left edge the camera stays inside the level.
    let half = vec2(WINDOW.x, WINDOW.y) / ZOOM / 2.0;
    for (_, _, c) in track.iter() {
        assert!(c.x >= half.x - 0.5 && c.y >= half.y - 0.5, "inside the level: {c}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
// ANCHOR_END: input_test

#[test]
fn the_camera_follows_the_hero_and_stays_inside_the_level() {
    let dir = data_dir("camera");
    let track = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = track.clone();
    let world = Game::new("ember hop camera", 960, 540)
        .data_dir(&dir)
        .seed(SEED)
        .headless_frames(true) // the camera is a frame system
        .sim_system(|ctx: &mut Ctx| match ctx.time.sim_tick() {
            1 => new_run(ctx),
            10 => teleport(ctx, vec2(37.0 * 16.0, 12.0 * 16.0)), // mid-level
            70 => teleport(ctx, vec2(59.0 * 16.0, 12.0 * 16.0)), // the right end, before the way out
            _ => {}
        })
        .plugin(EmberHop)
        .sim_system(move |ctx: &mut Ctx| {
            if let Some((_, at)) = hero(ctx) {
                log.borrow_mut().push((ctx.time.sim_tick(), at, ctx.camera.center()));
            }
        })
        .run_headless(130, |_| {});
    let track = track.borrow();
    let at = |tick: u64| *track.iter().find(|(t, ..)| *t == tick).unwrap();
    let (_, hero_mid, camera_mid) = at(60);
    assert!(
        (camera_mid.x - hero_mid.x).abs() < 32.0,
        "the camera eased to the hero: {camera_mid} for {hero_mid}"
    );
    let (_, hero_end, camera_end) = at(129);
    let size = world.resource::<CurrentLevel>().size;
    let half = vec2(WINDOW.x, WINDOW.y) / ZOOM / 2.0;
    assert!(
        (camera_end.x - (size.x - half.x)).abs() < 1.0,
        "stopped at the level's right edge: {camera_end}"
    );
    assert!(hero_end.x > camera_end.x, "the hero is right of the centre there");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn any_key_or_button_leaves_the_end_screen() {
    // `pressed_this_frame`: a key no action binds still counts.
    for (name, event) in [
        ("any-key", InputEvent::Key(Key::G, true)),
        (
            "any-button",
            InputEvent::GamepadButton(Gamepad::from_slot(0).unwrap(), GamepadButton::LeftBumper, true),
        ),
    ] {
        let dir = data_dir(name);
        let scenario = InputScenario::new().at(10, event).duration(14);
        let (world, report) = Game::new("ember hop any", 960, 540)
            .data_dir(&dir)
            .seed(SEED)
            .headless_frames(true)
            .plugin(EmberHop)
            .run_headless_scenario(scenario, |ctx| {
                *ctx.world.resource_mut::<Screen>() = Screen::Finished;
            });
        report.assert_passed();
        assert_eq!(screen(&world), Screen::Title, "{name}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn the_autopilot_plays_the_whole_game() {
    let dir = data_dir("autopilot");
    let world = game::smoke::smoke_game(Game::new("ember hop smoke", 960, 540).data_dir(&dir))
        .run_headless(game::smoke::MAX_TICKS + 1, |ctx| {
            game::smoke::setup_smoke(ctx, None, "test")
        });
    let smoke = world.resource::<game::smoke::Smoke>();
    let report = smoke.report();
    assert!(smoke.finished(), "{report:#?}");
    assert_eq!(report.levels_cleared, 2);
    assert_eq!(
        report.screens,
        [
            "Title",
            "Playing",
            "Transition",
            "Playing",
            "Paused",
            "Title",
            "Playing",
            "Transition",
            "Finished",
            "Title"
        ]
    );
    assert!(report.loads >= 1, "continued the saved run");
    assert!(report.drips_burned >= 3);
    assert_eq!(report.runs_finished, 1);
    assert_eq!(report.asset_size, [128, 16]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_file_the_game_ships_is_in_the_content_notices() {
    let notices = include_str!("../examples/ember_hop/CONTENT-NOTICES.txt");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut stack = vec![root.join("ember_hop")];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if !path.file_name().unwrap().to_string_lossy().starts_with('.') {
                files.push(path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    assert!(files.len() > 20, "{files:?}");
    for file in files {
        assert!(
            notices.contains(&format!("assets/{file}")),
            "{file} isn't in CONTENT-NOTICES.txt"
        );
    }
}
