//! The `shard_grove` 3D reference game, played headlessly: the glTF level
//! (drawn, solid and baked into a navigation mesh), the skinned glTF
//! characters and their animation state machines, walking relative to the
//! view, crawlers that patrol and chase on the navigation mesh, the pulse,
//! hits and losing, shards and the gate, shrine saves, continuing and a
//! damaged save, the quality and volume settings, pausing, the menus by
//! keyboard, the sign in the world, the crawlers' positional hum, and a
//! whole game played by the autopilot. No window, no GPU.
//!
//! The tests drive the game's own plugin (`examples/shard_grove/game.rs`)
//! with a scripted player that writes the `Intent` before the rules run.
#![cfg(all(feature = "navmesh", feature = "physics3d"))]

#[path = "../examples/shard_grove/game.rs"]
mod game;

use game::*;
use ponko::actions::InputEvent;
use ponko::anim::{Animator, Skeleton};
use ponko::d3::{AudioSource, Character, NavAgent, NavMesh, Transform, UiPanel};
use ponko::prelude::*;
use ponko::render::GraphicsQuality;
use ponko::scenario::{InputScenario, ScenarioCheck};
use std::path::{Path, PathBuf};

/// A fresh data folder for one test.
fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-shard-grove-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// ANCHOR: script
/// The game with its files in `dir` and `script` deciding, each tick before
/// the rules, what the player does.
fn scripted(dir: &Path, script: impl FnMut(&mut Ctx) + 'static) -> Game {
    Game::new("shard grove test", 1280, 720)
        .data_dir(dir)
        .sim_system(script)
        .plugin(ShardGrove)
}
// ANCHOR_END: script

fn intent(ctx: &mut Ctx) -> &mut Intent {
    ctx.world.resource_mut::<Intent>()
}

fn screen(world: &World) -> Screen {
    *world.resource::<Screen>()
}

/// Puts the warden at `at` (the body teleports there).
fn teleport(ctx: &mut Ctx, at: Vec3) {
    if let Some((e, _)) = warden(&mut ctx.world) {
        ctx.world.get_mut::<Transform>(e).expect("a transform").position = at;
    }
}

fn warden_at(world: &mut World) -> Vec3 {
    warden(world).expect("a warden").1
}

/// Takes every crawler out of the way, far in the south-west corner.
fn park_crawlers(ctx: &mut Ctx) {
    ctx.world.query::<(&mut Transform, &mut Crawler)>().for_each(|(t, c)| {
        t.position = vec3(-20.0, 0.0, 20.0);
        c.route = [vec3(-20.0, 0.0, 20.0); 2];
        c.last = t.position;
    });
}

#[test]
fn the_game_starts_on_the_title_with_the_level_walkable_everywhere() {
    let dir = data_dir("title");
    let mut world = scripted(&dir, |_| {}).run_headless(2, |_| {});
    assert_eq!(screen(&world), Screen::Title);
    assert!(warden(&mut world).is_none(), "no warden on the title");
    // The level's own triangles, baked: every shard, shrine and the gate can
    // be reached from the start.
    let nav = world.resource::<NavMesh>();
    let start = nav.sample_position(START, 1.0).expect("the start is on the mesh");
    for goal in SHARD_SPOTS.into_iter().chain(SHRINE_SPOTS).chain([GATE]) {
        let path = nav
            .find_path(start, goal)
            .unwrap_or_else(|| panic!("no path to {goal}"));
        let end = *path.last().expect("a path has corners");
        assert!(flat_distance(end, goal) < 0.6, "the path to {goal} ends at {end}");
    }
    // The shard on the platform is reached by the ramp, 1.5 m up.
    let up = nav.find_path(start, SHARD_SPOTS[0]).unwrap();
    assert!((up.last().unwrap().y - 1.5).abs() < 0.3, "on the platform: {up:?}");
}

#[test]
fn the_characters_are_skinned_gltf_files_with_named_clips() {
    let dir = data_dir("characters");
    let world = scripted(&dir, |_| {}).run_headless(1, |_| {});
    let assets = world.resource::<GroveAssets>();
    // Every node of the file is a joint: the mesh's own node, then the bones.
    let warden_joints: Vec<&str> = assets
        .warden_skeleton
        .joints()
        .iter()
        .map(|j| j.name.as_str())
        .collect();
    assert_eq!(
        warden_joints,
        [
            "warden", "root", "hips", "chest", "head", "arm_l", "arm_r", "leg_l", "leg_r"
        ]
    );
    assert_eq!(assets.crawler_skeleton.joints().len(), 7);
    // Four states for the warden, three for the crawlers, each a clip of its file.
    assert_eq!(Animator::new(assets.warden_graph.clone()).state_count(), 4);
    assert_eq!(Animator::new(assets.crawler_graph.clone()).state_count(), 3);
    Game::new("clips", 1, 1).run_headless(0, |ctx| {
        let clips: Vec<String> = ctx
            .gfx
            .load_clips(WARDEN)
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        assert_eq!(clips, ["idle", "walk", "pulse", "hurt"]);
        let walk = ctx.gfx.load_clip(WARDEN, "walk");
        assert!(
            (walk.duration() - 0.8).abs() < 1e-4,
            "a 0.8 s step: {}",
            walk.duration()
        );
        let clips: Vec<String> = ctx
            .gfx
            .load_clips(CRAWLER)
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        assert_eq!(clips, ["idle", "crawl", "stunned"]);
    });
}

#[test]
fn a_new_game_spawns_the_warden_standing_with_shards_and_crawlers() {
    let dir = data_dir("new");
    let mut world = scripted(&dir, |ctx| {
        if ctx.time.sim_tick() == 1 {
            intent(ctx).menu = Some(Menu::NewGame);
        }
    })
    .run_headless(60, |_| {});
    assert_eq!(screen(&world), Screen::Playing);
    assert_eq!(world.query::<&Shard>().iter().count(), 6);
    assert_eq!(
        world
            .query::<(&Crawler, &NavAgent, &Skeleton, &Animator)>()
            .iter()
            .count(),
        4
    );
    let (e, at) = warden(&mut world).expect("a warden");
    assert!(world.get::<Character>(e).unwrap().grounded, "standing: {at}");
    assert!(
        at.y.abs() < 0.05 && flat_distance(at, START) < 0.05,
        "at the start: {at}"
    );
    assert_eq!(world.resource::<Run>().health, MAX_HEALTH);
}

#[test]
fn walking_follows_the_view() {
    let dir = data_dir("walk");
    let mut world = scripted(&dir, |ctx| {
        match ctx.time.sim_tick() {
            1 => intent(ctx).menu = Some(Menu::NewGame),
            2 => park_crawlers(ctx),
            // 0.6 s forward: along -Z, the way the view looks at the start
            // (a broken wall stands 3.5 m ahead).
            10 => intent(ctx).movement = vec2(0.0, 1.0),
            40 => {
                intent(ctx).movement = Vec2::ZERO;
                // Turn the view a quarter to the right, then walk forward again:
                // along +X now.
                ctx.world.resource_mut::<View>().yaw = -std::f32::consts::FRAC_PI_2;
            }
            50 => intent(ctx).movement = vec2(0.0, 1.0),
            70 => intent(ctx).movement = Vec2::ZERO,
            _ => {}
        }
    })
    .run_headless(80, |_| {});
    let at = warden_at(&mut world);
    // 30 ticks at 4.5 m/s, then 20 ticks to the right.
    assert!((START.z - at.z - 2.7).abs() < 0.2, "walked forward 2.7 m: {at}");
    assert!((at.x - 1.8).abs() < 0.2, "then 1.8 m along +X: {at}");
    let (e, _) = warden(&mut world).unwrap();
    let facing = world.get::<Warden>(e).unwrap().facing;
    assert!(
        (facing + std::f32::consts::FRAC_PI_2).abs() < 0.01,
        "faces +X: {facing}"
    );
}

#[test]
fn the_animation_state_machines_follow_the_game() {
    use game::{crawler_anim, warden_anim};
    // With frames: the animators advance in the frame's animation step.
    let dir = data_dir("anim");
    let states = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = states.clone();
    scripted(&dir, move |ctx| {
        let tick = ctx.time.sim_tick();
        match tick {
            1 => intent(ctx).menu = Some(Menu::NewGame),
            2 => park_crawlers(ctx),
            // With frames, the keyboard (nothing held) writes the axes every
            // frame: a script holds a direction by writing it every tick.
            10..40 => intent(ctx).movement = vec2(0.0, 1.0),
            60 => intent(ctx).pulse = true,
            _ => {}
        }
        if let Some((e, _)) = warden(&mut ctx.world) {
            let state = ctx.world.get::<Animator>(e).unwrap().state();
            seen.borrow_mut().push((tick, state));
        }
    })
    .headless_frames(true)
    .run_headless(120, |_| {});
    let at = |tick: u64| states.borrow().iter().find(|(t, _)| *t == tick).map(|s| s.1);
    assert_eq!(at(8), Some(warden_anim::IDLE));
    assert_eq!(at(35), Some(warden_anim::WALK), "walking");
    assert_eq!(at(55), Some(warden_anim::IDLE), "stopped");
    assert_eq!(at(63), Some(warden_anim::CAST), "the pulse plays");
    assert_eq!(at(110), Some(warden_anim::IDLE), "and goes back to idle");

    // The crawlers: crawling on patrol, dazed when stunned.
    let dir = data_dir("anim-crawler");
    let mut world = scripted(&dir, |ctx| match ctx.time.sim_tick() {
        1 => intent(ctx).menu = Some(Menu::NewGame),
        40 => {
            let first = CRAWLER_ROUTES[0][0];
            ctx.world.query::<&mut Crawler>().for_each(|c| {
                if c.route[0] == first {
                    c.stun = 2.0;
                }
            });
        }
        _ => {}
    })
    .headless_frames(true)
    .run_headless(60, |_| {});
    let states: Vec<_> = world
        .query::<(&Crawler, &Animator)>()
        .iter()
        .map(|(c, a)| (c.stun > 0.0, a.state()))
        .collect();
    for (stunned, state) in states {
        let expected = if stunned {
            crawler_anim::DAZED
        } else {
            crawler_anim::CRAWL
        };
        assert_eq!(state, expected, "stunned: {stunned}");
    }
}

#[test]
fn crawlers_patrol_then_chase_on_the_navmesh_and_a_touch_hurts_once() {
    let dir = data_dir("chase");
    let route = CRAWLER_ROUTES[0];
    let mut world = scripted(&dir, move |ctx| match ctx.time.sim_tick() {
        1 => intent(ctx).menu = Some(Menu::NewGame),
        // Beside the first crawler's route, behind the broken wall's end.
        2 => teleport(ctx, vec3(-6.0, 0.0, -11.0)),
        _ => {}
    })
    .run_headless(200, |_| {});
    let hunter = world
        .query::<(&Transform, &Crawler)>()
        .iter()
        .find(|(_, c)| c.route == route)
        .map(|(t, c)| (t.position, c.chasing))
        .expect("the first crawler");
    let tally = world.resource::<Tally>().clone();
    assert!(tally.hits >= 1, "caught: {tally:?}, crawler at {:?}", hunter);
    // Hits are spaced by the invulnerability (1.5 s) at most: 4 s of contact
    // can't cost more than three.
    assert!(tally.hits <= 3, "{tally:?}");
    assert_eq!(world.resource::<Run>().health, MAX_HEALTH - tally.hits.min(MAX_HEALTH));
    // It walks on the ground of the level, not through walls.
    assert!(hunter.0.y.abs() < 0.2, "on the ground: {:?}", hunter.0);
}

#[test]
fn the_pulse_stuns_close_crawlers_and_waits_for_its_cooldown() {
    let dir = data_dir("pulse");
    let mut world = scripted(&dir, |ctx| match ctx.time.sim_tick() {
        1 => intent(ctx).menu = Some(Menu::NewGame),
        2 => {
            // One crawler right beside the warden, the rest far away.
            park_crawlers(ctx);
            let near = START + vec3(2.0, 0.0, 0.0);
            if let Some((t, c)) = ctx.world.query::<(&mut Transform, &mut Crawler)>().iter().next() {
                t.position = near;
                c.route = [near; 2];
                c.last = near;
            }
        }
        3 | 4 => intent(ctx).pulse = true, // the second press is in the cooldown
        _ => {}
    })
    .run_headless(20, |_| {});
    let tally = world.resource::<Tally>().clone();
    assert_eq!((tally.pulses, tally.stuns), (1, 1), "{tally:?}");
    let stunned: Vec<f32> = world.query::<&Crawler>().iter().map(|c| c.stun).collect();
    assert_eq!(stunned.iter().filter(|&&s| s > 0.0).count(), 1, "{stunned:?}");
    assert_eq!(
        world.resource::<Run>().health,
        MAX_HEALTH,
        "a stunned crawler doesn't hurt"
    );
}

#[test]
fn losing_every_heart_ends_the_run_and_try_again_starts_over() {
    let dir = data_dir("lose");
    let mut world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        if tick == 1 {
            intent(ctx).menu = Some(Menu::NewGame);
        }
        if tick == 2 {
            park_crawlers(ctx);
        }
        // A crawler sits on the warden; it may hurt every 1.5 s.
        if tick < 400
            && *ctx.world.resource::<Screen>() == Screen::Playing
            && let Some((_, at)) = warden(&mut ctx.world)
            && let Some((t, c)) = ctx.world.query::<(&mut Transform, &mut Crawler)>().iter().next()
        {
            t.position = at;
            c.route = [at; 2];
        }
        if tick == 400 {
            assert_eq!(*ctx.world.resource::<Screen>(), Screen::Lost);
            assert_eq!(ctx.world.resource::<Tally>().hits, MAX_HEALTH);
            intent(ctx).menu = Some(Menu::TryAgain);
        }
    })
    .run_headless(402, |_| {});
    assert_eq!(world.resource::<Tally>().losses, 1);
    // No save: trying again is a new game.
    assert_eq!(screen(&world), Screen::Playing);
    assert_eq!(world.resource::<Run>().health, MAX_HEALTH);
    assert!(flat_distance(warden_at(&mut world), START) < 0.1);
}

#[test]
fn every_shard_opens_the_gate_and_the_gate_ends_the_game() {
    let dir = data_dir("win");
    let mut world = scripted(&dir, |ctx| {
        let tick = ctx.time.sim_tick();
        match tick {
            1 => intent(ctx).menu = Some(Menu::NewGame),
            2 => park_crawlers(ctx),
            // A shard every 5 ticks.
            5..=30 if tick % 5 == 0 => teleport(ctx, SHARD_SPOTS[(tick / 5 - 1) as usize]),
            35 => {
                assert!(ctx.world.resource::<Run>().complete());
                assert_eq!(
                    *ctx.world.resource::<Screen>(),
                    Screen::Playing,
                    "the gate isn't reached yet"
                );
                teleport(ctx, GATE);
            }
            _ => {}
        }
    })
    .run_headless(40, |_| {});
    assert_eq!(screen(&world), Screen::Won);
    assert_eq!(world.resource::<Tally>().shards, 6);
    let records = world.resource::<Records>().clone();
    assert_eq!(records.wins, 1);
    assert!(records.best_time.is_some_and(|t| t > 0.5));
    assert!(warden(&mut world).is_none(), "the run's entities are gone");
    // The records are a file; the finished run's save is deleted.
    let on_disk: Records = serde_json::from_slice(&std::fs::read(dir.join(RECORDS_FILE)).unwrap()).unwrap();
    assert_eq!(on_disk, records);
    assert!(!dir.join(SAVE_FILE).exists());
}

#[test]
fn a_shrine_saves_and_continue_brings_the_run_back() {
    let dir = data_dir("save");
    let world = scripted(&dir, |ctx| match ctx.time.sim_tick() {
        1 => intent(ctx).menu = Some(Menu::NewGame),
        2 => park_crawlers(ctx),
        5 => teleport(ctx, SHARD_SPOTS[2]),
        10 => teleport(ctx, SHARD_SPOTS[5]),
        15 => teleport(ctx, SHRINE_SPOTS[0]),
        20 => intent(ctx).pause = true,
        25 => intent(ctx).menu = Some(Menu::ToTitle),
        _ => {}
    })
    .run_headless(30, |_| {});
    assert_eq!(screen(&world), Screen::Title);
    assert_eq!(world.resource::<Tally>().saves, 1);
    let saved: Save = serde_json::from_slice(&std::fs::read(dir.join(SAVE_FILE)).unwrap()).unwrap();
    assert_eq!(saved.run.shards, [2, 5]);
    assert_eq!(saved.run.shrine, Some(0));

    // A new session continues it.
    let mut world = scripted(&dir, |ctx| {
        if ctx.time.sim_tick() == 1 {
            intent(ctx).menu = Some(Menu::Continue);
        }
    })
    .run_headless(20, |_| {});
    assert_eq!(screen(&world), Screen::Playing);
    assert_eq!(*world.resource::<SaveStatus>(), SaveStatus::Loaded);
    assert_eq!(world.resource::<Run>().shards, [2, 5]);
    let left: Vec<u8> = world.query::<&Shard>().iter().map(|s| s.id).collect();
    assert_eq!(left.len(), 4);
    assert!(!left.contains(&2) && !left.contains(&5));
    let at = warden_at(&mut world);
    assert!(
        flat_distance(at, SHRINE_SPOTS[0] + SHRINE_STEP) < 0.2,
        "beside the shrine: {at}"
    );
    // Standing at the shrine it was saved at doesn't save again.
    assert_eq!(world.resource::<Tally>().saves, 0);
}

#[test]
fn a_damaged_save_is_set_aside_and_said() {
    let dir = data_dir("damaged");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(SAVE_FILE), b"{ not json").unwrap();
    let world = scripted(&dir, |ctx| {
        if ctx.time.sim_tick() == 1 {
            intent(ctx).menu = Some(Menu::Continue);
        }
    })
    .run_headless(3, |_| {});
    assert_eq!(screen(&world), Screen::Title, "nothing to continue");
    assert!(matches!(world.resource::<SaveStatus>(), SaveStatus::Recovered(_)));
    assert!(world.resource::<Notice>().text.contains("damaged"));
    assert!(!dir.join(SAVE_FILE).exists());
    assert_eq!(
        std::fs::read(dir.join(format!("{SAVE_FILE}.bad"))).unwrap(),
        b"{ not json"
    );

    // A save from a run that can't be (a seventh shard) is set aside too.
    let bad = Save {
        version: SAVE_VERSION,
        run: Run {
            shards: vec![9],
            ..Run::default()
        },
    };
    std::fs::write(dir.join(SAVE_FILE), serde_json::to_vec(&bad).unwrap()).unwrap();
    let world = scripted(&dir, |ctx| {
        if ctx.time.sim_tick() == 1 {
            intent(ctx).menu = Some(Menu::Continue);
        }
    })
    .run_headless(3, |_| {});
    assert_eq!(
        *world.resource::<SaveStatus>(),
        SaveStatus::Recovered("unexpected values".into())
    );
}

#[test]
fn quality_and_volume_settings_apply_and_persist() {
    let dir = data_dir("settings");
    let world = scripted(&dir, |ctx| match ctx.time.sim_tick() {
        1 => intent(ctx).menu = Some(Menu::Settings),
        2 => intent(ctx).menu = Some(Menu::CycleQuality), // Low -> Medium
        3 => intent(ctx).menu = Some(Menu::CycleQuality), // -> High
        4 => intent(ctx).menu = Some(Menu::CycleVolume),
        5 => intent(ctx).menu = Some(Menu::Back),
        _ => {}
    })
    .run_headless(6, |_| {});
    assert_eq!(screen(&world), Screen::Title);
    let settings = *world.resource::<Settings>();
    assert_eq!(settings.quality, Some(2));
    assert_eq!(settings.volume, 1.0, "80% steps up to 100%");
    let saved: Settings = serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
    assert_eq!(saved, settings);

    // The next session starts with them.
    Game::new("settings again", 64, 64)
        .data_dir(&dir)
        .plugin(ShardGrove)
        .run_headless(1, |ctx| {
            assert_eq!(ctx.scene3d.quality(), Some(GraphicsQuality::High));
            assert_eq!(quality_name(ctx), "High");
        });
}

#[test]
fn without_a_choice_the_quality_is_the_recommended_one() {
    let dir = data_dir("quality-default");
    Game::new("quality", 64, 64)
        .data_dir(&dir)
        .plugin(ShardGrove)
        .run_headless(1, |ctx| {
            // No GPU in a headless run: the middle preset.
            assert_eq!(ctx.scene3d.quality(), Some(GraphicsQuality::Medium));
            assert_eq!(ctx.world.resource::<Settings>().quality, None);
        });
}

#[test]
fn pausing_holds_the_crawlers_and_the_warden() {
    let dir = data_dir("pause");
    let snapshots = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = snapshots.clone();
    scripted(&dir, move |ctx| {
        let tick = ctx.time.sim_tick();
        match tick {
            1 => intent(ctx).menu = Some(Menu::NewGame),
            30 => intent(ctx).pause = true,
            31 | 90 => {
                let mut at: Vec<Vec3> = ctx
                    .world
                    .query::<(&Transform, &Crawler)>()
                    .iter()
                    .map(|(t, _)| t.position)
                    .collect();
                at.push(warden_at(&mut ctx.world));
                assert_eq!(*ctx.world.resource::<Screen>(), Screen::Paused);
                seen.borrow_mut().push(at);
            }
            // Holding a direction while paused does nothing.
            40 => intent(ctx).movement = vec2(1.0, 0.0),
            _ => {}
        }
    })
    .run_headless(91, |_| {});
    let snapshots = snapshots.borrow();
    for (a, b) in snapshots[0].iter().zip(&snapshots[1]) {
        assert!(a.distance(*b) < 1e-3, "moved while paused: {a} -> {b}");
    }
}

#[test]
fn the_menus_work_with_the_keyboard() {
    let dir = data_dir("keys");
    let press = |s: InputScenario, key: Key, at: u64| {
        s.at(at, InputEvent::Key(key, true))
            .at(at + 1, InputEvent::Key(key, false))
    };
    // Enter presses the focused "New game", Escape pauses, Enter resumes.
    let scenario = InputScenario::new().expect(3, ScenarioCheck::ui("title.new").shown(true));
    let scenario = press(scenario, Key::Enter, 5)
        .expect(12, ScenarioCheck::ui("hud.shards").shown(true))
        .expect(12, ScenarioCheck::ui("title.new").shown(false));
    let scenario = press(scenario, Key::Escape, 20).expect(30, ScenarioCheck::ui("pause.resume").shown(true));
    let scenario = press(scenario, Key::Enter, 35)
        .expect(45, ScenarioCheck::ui("pause.resume").shown(false))
        .duration(50);
    let (world, report) = Game::new("keys", 1280, 720)
        .data_dir(&dir)
        .plugin(ShardGrove)
        .headless_frames(true)
        .run_headless_scenario(scenario, |_| {});
    report.assert_passed();
    assert_eq!(screen(&world), Screen::Playing);
}

#[test]
fn the_sign_over_the_gate_is_ui_in_the_world() {
    let dir = data_dir("sign");
    let mut world = scripted(&dir, |ctx| match ctx.time.sim_tick() {
        1 => intent(ctx).menu = Some(Menu::NewGame),
        2 => park_crawlers(ctx),
        5 => teleport(ctx, SHARD_SPOTS[3]),
        _ => {}
    })
    .headless_frames(true)
    .run_headless(12, |_| {});
    let texts: Vec<String> = world
        .query::<(&UiPanel, &GateSign)>()
        .iter()
        .map(|(p, s)| p.ui.text(s.text).unwrap_or_default().to_string())
        .collect();
    assert_eq!(texts, ["Shards 1 / 6"]);
}

#[test]
fn the_crawlers_hum_from_where_they_are() {
    let dir = data_dir("hum");
    let mut world = scripted(&dir, |ctx| {
        if ctx.time.sim_tick() == 1 {
            intent(ctx).menu = Some(Menu::NewGame);
        }
    })
    .headless_frames(true)
    .run_headless(5, |_| {});
    let hums: Vec<(bool, f32)> = world
        .query::<(&AudioSource, &Crawler)>()
        .iter()
        .map(|(s, _)| (s.settings.sound.looping, s.settings.max_distance))
        .collect();
    assert_eq!(hums, vec![(true, 14.0); 4]);
}

#[test]
fn the_autopilot_plays_the_whole_game() {
    use game::smoke::*;
    let dir = data_dir("autopilot");
    let game = smoke_game(Game::new("autopilot", 1280, 720).data_dir(&dir)).setup(|ctx| setup_smoke(ctx, None, "test"));
    let world = game.run_headless(MAX_TICKS + 1);
    let smoke = world.resource::<Smoke>();
    let report = smoke.report();
    assert!(smoke.finished(), "{report:#?}");
    assert!(report.tally.shards >= 6 && report.tally.wins == 1, "{report:#?}");
    assert!(report.tally.saves >= 1 && report.tally.loads >= 1, "{report:#?}");
    assert_eq!(report.warden_joints, 9);
    assert!(report.path_corners >= 2);
    let screens: Vec<&str> = report.screens.iter().map(String::as_str).collect();
    let first_play = screens.iter().position(|s| *s == "Playing").unwrap();
    let won = screens.iter().position(|s| *s == "Won").unwrap();
    assert!(screens[first_play..won].contains(&"Paused"), "{screens:?}");
    assert_eq!(screens.last(), Some(&"Title"));
}

#[test]
fn every_asset_is_listed_in_the_content_notices() {
    let notices = include_str!("../examples/shard_grove/CONTENT-NOTICES.txt");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut stack = vec![root.join("shard_grove")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                assert!(notices.contains(&name), "{name} is not in CONTENT-NOTICES.txt");
            }
        }
    }
}
