//! What a scene load replaces and what it keeps (`Ctx::load_scene`'s table):
//! scenes that reuse entity ids, pending commands and events, physics,
//! resources and the failure contract.

#[allow(unused_imports)]
#[cfg(feature = "physics")]
use ponko::d2::{Collider, RigidBody};
#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform as Transform3D;
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::{Collider as Collider3D, RigidBody as RigidBody3D};
#[allow(unused_imports)]
#[cfg(feature = "navmesh")]
use ponko::d3::{NavAgent as NavAgent3D, NavMesh as NavMesh3D};
#[allow(unused_imports)]
#[cfg(feature = "navmesh")]
use ponko::nav::{NavBakeSettings, NavSource};
#[allow(unused_imports)]
use ponko::net::{Desync, Net, NetState};
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::MeshData;
use serde::{Deserialize, Serialize};

/// A file under `assets/` that is deleted when the test ends, pass or fail.
struct TempAsset(&'static str);
impl Drop for TempAsset {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(self.0),
        );
    }
}

fn write_asset(name: &str, json: &serde_json::Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::write(path, serde_json::to_string_pretty(json).unwrap()).unwrap();
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Component)]
struct Tag(u32);

/// Reflected, so scenes save it.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Level(u32);

/// Not reflected: game state that scenes don't touch.
struct Coins(u32);

#[derive(Component)]
struct Marker;

#[test]
fn commands_queued_before_a_load_are_dropped_and_later_ones_apply() {
    let file = TempAsset("test-lifecycle-commands.json");
    let mut kept = None;
    let mut world = Game::new("t", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() != 1 {
                return;
            }
            let e = ctx.world.query::<(Entity, &Tag)>().iter().next().unwrap().0;
            // Meant for the old world: after the load `e` names the scene's entity.
            ctx.commands.despawn(e);
            ctx.commands.spawn((Marker,));
            ctx.load_scene("test-lifecycle-commands.json").unwrap();
            ctx.commands.spawn((Tag(99),));
        })
        .run_headless(2, |ctx| {
            ctx.world.reflect::<Tag>();
            kept = Some(ctx.world.spawn((Tag(1),)));
            ctx.assets.save_scene(&ctx.world, file.0).unwrap();
        });
    let e = kept.unwrap();
    assert_eq!(
        world.get::<Tag>(e),
        Some(&Tag(1)),
        "the scene's entity survives the stale despawn"
    );
    assert_eq!(
        world.query::<&Marker>().iter().count(),
        0,
        "spawn queued before the load is dropped"
    );
    let mut tags: Vec<u32> = world.query::<&Tag>().iter().map(|t| t.0).collect();
    tags.sort_unstable();
    assert_eq!(tags, [1, 99], "commands queued after the load apply");
}

#[derive(Clone, Copy)]
struct Ping(u32);

#[test]
fn events_sent_before_a_load_can_still_be_read() {
    let file = TempAsset("test-lifecycle-events.json");
    let got = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = got.clone();
    Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 1 {
                ctx.events.send(Ping(7));
                ctx.load_scene("test-lifecycle-events.json").unwrap();
            }
        })
        .sim_system(move |ctx: &mut Ctx| {
            if let Some(p) = ctx.events.read::<Ping>().first() {
                seen.set(Some(p.0));
            }
        })
        .run_headless(2, |ctx| {
            ctx.world.reflect::<Tag>();
            ctx.world.spawn((Tag(1),));
            ctx.assets.save_scene(&ctx.world, file.0).unwrap();
        });
    assert_eq!(got.get(), Some(7));
}

#[test]
fn reflected_resources_follow_the_scene_and_other_state_stays() {
    let file = TempAsset("test-lifecycle-resources.json");
    let mut world = Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.world.reflect::<Tag>();
        ctx.world.reflect_resource::<Level>();
        ctx.world.insert_resource(Level(1));
        ctx.world.spawn((Tag(1),));
        ctx.assets.save_scene(&ctx.world, file.0).unwrap();

        ctx.world.insert_resource(Level(5));
        ctx.world.insert_resource(Coins(42));
        ctx.camera.zoom = 3.0;
        ctx.world.spawn((Tag(2), Marker));
        ctx.load_scene(file.0).unwrap();

        assert_eq!(ctx.camera.zoom, 3.0, "the camera belongs to the game");
    });
    assert_eq!(*world.resource::<Level>(), Level(1), "reflected: the scene's value");
    assert_eq!(world.resource::<Coins>().0, 42, "not reflected: kept");
    assert_eq!(world.query::<&Tag>().iter().count(), 1);
    assert_eq!(world.query::<&Marker>().iter().count(), 0);
}

#[test]
fn a_scene_with_an_unreflected_component_changes_nothing() {
    let file = TempAsset("test-lifecycle-invalid.json");
    write_asset(
        file.0,
        &serde_json::json!({
            "format": "ponko-world", "version": 1,
            "entities": [{ "entity": 0, "components": { "NoSuchComponent": {} } }],
        }),
    );
    let mut world = Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.world.reflect::<Tag>();
        let e = ctx.world.spawn((Tag(3),));
        ctx.commands.insert(e, Marker);
        let err = ctx.load_scene(file.0).unwrap_err();
        assert!(err.message().contains("NoSuchComponent"), "{err}");
        assert_eq!(ctx.commands.len(), 1, "pending commands are kept on failure");
    });
    assert_eq!(
        world.query::<(&Tag, &Marker)>().iter().count(),
        1,
        "world untouched, command applied"
    );
}

#[test]
fn a_scene_with_a_missing_texture_changes_nothing_but_keeps_loaded_ones() {
    let file = TempAsset("test-lifecycle-missing-asset.json");
    let sprite =
        |texture: &str| serde_json::json!({ "texture": texture, "size": [8.0, 8.0], "color": [1.0, 1.0, 1.0, 1.0] });
    write_asset(
        file.0,
        &serde_json::json!({
            "format": "ponko-world", "version": 1,
            "entities": [
                { "entity": 0, "components": { "Sprite": sprite("crab.png") } },
                { "entity": 1, "components": { "Sprite": sprite("no-such-texture.png") } },
            ],
        }),
    );
    let mut world = Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.world.reflect::<Tag>();
        ctx.world.spawn((Tag(4),));
        let err = ctx.load_scene(file.0).unwrap_err();
        assert!(err.message().contains("no-such-texture.png"), "{err}");
        // Preloading is not undone: the texture that did load is cached and
        // named, so loading it again gives the same handle.
        let crab = ctx.gfx.load_texture("crab.png");
        assert_eq!(ctx.gfx.load_texture("crab.png"), crab);
    });
    assert_eq!(world.query::<&Tag>().iter().map(|t| t.0).collect::<Vec<_>>(), [4]);
    assert_eq!(world.query::<&Sprite>().iter().count(), 0);
}

#[cfg(feature = "physics")]
mod physics {
    use super::*;

    const GRAVITY: Vec2 = Vec2::new(0.0, 980.0);

    fn ball_at(world: &mut World) -> Vec2 {
        world
            .query::<&Transform2D>()
            .filter::<With<RigidBody>>()
            .iter()
            .next()
            .unwrap()
            .position
    }

    /// Scene A: a dynamic ball at the origin. Scene B: the same entity id,
    /// moved to x = 500. Both are saved from one world, so they share ids.
    fn two_scenes(ctx: &mut Ctx, a: &str, b: &str) -> Entity {
        let ball = ctx.world.spawn((
            Transform2D::at(Vec2::ZERO),
            RigidBody::dynamic(),
            Collider::circle(10.0),
        ));
        ctx.assets.save_scene(&ctx.world, a).unwrap();
        ctx.world.get_mut::<Transform2D>(ball).unwrap().position = vec2(500.0, 0.0);
        ctx.assets.save_scene(&ctx.world, b).unwrap();
        ctx.load_scene(a).unwrap();
        ball
    }

    #[test]
    fn a_body_in_a_same_id_scene_starts_from_rest() {
        let (a, b) = (
            TempAsset("test-lifecycle-fall-a.json"),
            TempAsset("test-lifecycle-fall-b.json"),
        );
        let mut ball = None;
        let mut world = Game::new("t", 1, 1)
            .physics(GRAVITY)
            .sim_system(|ctx: &mut Ctx| {
                // Half a second of falling, then the next scene.
                if ctx.time.sim_tick() == 30 {
                    ctx.load_scene("test-lifecycle-fall-b.json").unwrap();
                }
            })
            .run_headless(60, |ctx| ball = Some(two_scenes(ctx, a.0, b.0)));
        let p = ball_at(&mut world);
        assert!(world.get::<Transform2D>(ball.unwrap()).is_some(), "same id");
        assert!((p.x - 500.0).abs() < 1e-3, "{p}");
        // From rest, half a second of gravity: about 122 px. Carrying the old
        // body's speed (about 490 px/s) would put it near 370.
        assert!(p.y > 80.0 && p.y < 170.0, "falls from rest: {p}");
    }

    #[test]
    fn a_direct_world_load_also_rebuilds_bodies_at_the_next_step() {
        let (a, b) = (
            TempAsset("test-lifecycle-raw-a.json"),
            TempAsset("test-lifecycle-raw-b.json"),
        );
        let mut world = Game::new("t", 1, 1)
            .physics(GRAVITY)
            .sim_system(|ctx: &mut Ctx| {
                if ctx.time.sim_tick() == 30 {
                    let text = std::fs::read_to_string(
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/test-lifecycle-raw-b.json"),
                    )
                    .unwrap();
                    ctx.world.load(&serde_json::from_str(&text).unwrap()).unwrap();
                }
            })
            .run_headless(60, |ctx| {
                two_scenes(ctx, a.0, b.0);
            });
        let p = ball_at(&mut world);
        assert!(p.y > 80.0 && p.y < 170.0, "falls from rest: {p}");
    }

    #[test]
    fn queries_follow_the_new_scene_from_the_next_step() {
        let (a, b) = (
            TempAsset("test-lifecycle-query-a.json"),
            TempAsset("test-lifecycle-query-b.json"),
        );
        let checked = std::rc::Rc::new(std::cell::Cell::new(0));
        let done = checked.clone();
        Game::new("t", 1, 1)
            .physics(Vec2::ZERO)
            .sim_system(move |ctx: &mut Ctx| {
                let hit = |ctx: &Ctx| {
                    ctx.physics.cast_ray(
                        vec2(-100.0, 0.0),
                        Vec2::X,
                        1000.0,
                        ponko::d2::QueryFilter::default().with_sensors(),
                    )
                };
                match ctx.time.sim_tick() {
                    5 => {
                        let before = hit(ctx).expect("scene A's ball");
                        assert!((before.point.x + 10.0).abs() < 0.5, "{:?}", before.point);
                        ctx.load_scene("test-lifecycle-query-b.json").unwrap();
                        assert!(hit(ctx).is_none(), "no stale bodies between the load and the next step");
                        assert_eq!(ctx.physics.body_count(), 0);
                        done.set(done.get() + 1);
                    }
                    6 => {
                        // Systems run before the physics step: tick 6's step built them.
                        let after = hit(ctx).expect("scene B's ball after the next step");
                        assert!((after.point.x - 490.0).abs() < 0.5, "{:?}", after.point);
                        assert_eq!(ctx.physics.body_count(), 1);
                        done.set(done.get() + 1);
                    }
                    _ => {}
                }
            })
            .run_headless(7, |ctx| {
                let ball = ctx
                    .world
                    .spawn((Transform2D::at(Vec2::ZERO), RigidBody::fixed(), Collider::circle(10.0)));
                ctx.assets
                    .save_scene(&ctx.world, "test-lifecycle-query-a.json")
                    .unwrap();
                ctx.world.get_mut::<Transform2D>(ball).unwrap().position = vec2(500.0, 0.0);
                ctx.assets
                    .save_scene(&ctx.world, "test-lifecycle-query-b.json")
                    .unwrap();
                ctx.load_scene("test-lifecycle-query-a.json").unwrap();
            });
        drop((a, b));
        assert_eq!(checked.get(), 2);
    }

    #[test]
    fn gravity_survives_a_load_and_a_failed_load_keeps_the_bodies() {
        let file = TempAsset("test-lifecycle-gravity.json");
        let bad = TempAsset("test-lifecycle-gravity-bad.json");
        write_asset(
            bad.0,
            &serde_json::json!({ "format": "ponko-world", "version": 99, "entities": [] }),
        );
        Game::new("t", 1, 1)
            .physics(GRAVITY)
            .sim_system(|ctx: &mut Ctx| match ctx.time.sim_tick() {
                2 => {
                    ctx.physics.set_gravity(vec2(0.0, -50.0));
                    assert!(ctx.load_scene("test-lifecycle-gravity-bad.json").is_err());
                    assert_eq!(ctx.physics.body_count(), 1, "a failed load leaves physics alone");
                    ctx.load_scene("test-lifecycle-gravity.json").unwrap();
                }
                3 => assert_eq!(
                    ctx.physics.gravity(),
                    vec2(0.0, -50.0),
                    "physics settings are the game's"
                ),
                _ => {}
            })
            .run_headless(4, |ctx| {
                ctx.world.spawn((
                    Transform2D::at(Vec2::ZERO),
                    RigidBody::dynamic(),
                    Collider::circle(10.0),
                ));
                ctx.assets.save_scene(&ctx.world, file.0).unwrap();
            });
    }
}

#[cfg(feature = "physics3d")]
mod physics3d {
    use super::*;

    #[test]
    fn a_3d_body_in_a_same_id_scene_starts_from_rest() {
        let (a, b) = (
            TempAsset("test-lifecycle-fall3d-a.json"),
            TempAsset("test-lifecycle-fall3d-b.json"),
        );
        let mut world = Game::new("t", 1, 1)
            .physics3d(vec3(0.0, -9.81, 0.0))
            .sim_system(|ctx: &mut Ctx| {
                if ctx.time.sim_tick() == 30 {
                    ctx.load_scene("test-lifecycle-fall3d-b.json").unwrap();
                }
            })
            .run_headless(60, |ctx| {
                let ball = ctx.world.spawn((
                    Transform3D::at(Vec3::ZERO),
                    RigidBody3D::dynamic(),
                    Collider3D::sphere(0.5),
                ));
                ctx.assets.save_scene(&ctx.world, a.0).unwrap();
                ctx.world.get_mut::<Transform3D>(ball).unwrap().position = vec3(50.0, 0.0, 0.0);
                ctx.assets.save_scene(&ctx.world, b.0).unwrap();
                ctx.load_scene(a.0).unwrap();
            });
        let p = world
            .query::<&Transform3D>()
            .filter::<With<RigidBody3D>>()
            .iter()
            .next()
            .unwrap()
            .position;
        assert!((p.x - 50.0).abs() < 1e-3, "{p}");
        // From rest: about 1.2 m in half a second; with the old speed, about 3.7 m.
        assert!(p.y < -0.8 && p.y > -1.7, "falls from rest: {p}");
    }
}

#[test]
fn prefabs_stay_cached_across_a_load() {
    let prefab = TempAsset("test-lifecycle-prefab.json");
    let scene = TempAsset("test-lifecycle-prefab-scene.json");
    let mut world = Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.world.reflect::<Tag>();
        let model = ctx.world.spawn((Transform2D::default(), Tag(5)));
        ctx.assets.save_prefab(&ctx.world, model, prefab.0).unwrap();
        ctx.spawn_prefab(prefab.0, vec2(1.0, 0.0)).unwrap();
        ctx.assets.save_scene(&ctx.world, scene.0).unwrap();
        // The file is gone, but the prefab was read before the load.
        std::fs::remove_file(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(prefab.0),
        )
        .unwrap();
        ctx.load_scene(scene.0).unwrap();
        ctx.spawn_prefab(prefab.0, vec2(2.0, 0.0))
            .expect("cached across the load");
    });
    assert_eq!(world.query::<&Tag>().iter().count(), 3);
}

#[test]
fn attachments_follow_the_new_scenes_entities() {
    let (a, b) = (
        TempAsset("test-lifecycle-attach-a.json"),
        TempAsset("test-lifecycle-attach-b.json"),
    );
    let mut ids = None;
    let world = Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 2 {
                ctx.load_scene("test-lifecycle-attach-b.json").unwrap();
            }
        })
        .sim_system(systems::follow_attachments)
        .run_headless(4, |ctx| {
            let tank = ctx.world.spawn((Transform2D::at(vec2(0.0, 0.0)),));
            let other = ctx.world.spawn((Transform2D::at(vec2(0.0, 0.0)),));
            let turret = ctx.world.spawn((
                Transform2D::default(),
                ponko::attach::Attached::to(tank).offset(vec2(0.0, -4.0)),
            ));
            let ring = ctx
                .world
                .spawn((Transform2D::default(), ponko::attach::Attached::to(other)));
            ctx.assets.save_scene(&ctx.world, a.0).unwrap();
            // Scene B: the same ids; the tank moved, the turret sits elsewhere
            // on it, and the ring's entity isn't in it.
            ctx.world.get_mut::<Transform2D>(tank).unwrap().position = vec2(100.0, 0.0);
            *ctx.world.get_mut::<ponko::attach::Attached>(turret).unwrap() =
                ponko::attach::Attached::to(tank).offset(vec2(10.0, 0.0));
            ctx.world.despawn(other);
            ctx.assets.save_scene(&ctx.world, b.0).unwrap();
            ctx.load_scene(a.0).unwrap();
            ids = Some((turret, ring));
        });
    let (turret, ring) = ids.unwrap();
    assert_eq!(world.get::<Transform2D>(turret).unwrap().position, vec2(110.0, 0.0));
    assert!(!world.is_alive(ring), "its entity isn't in the new scene");
}

#[derive(Serialize, Deserialize)]
struct Step(u32);

/// Two peers in lockstep: loads in setup and in simulation systems run on
/// every peer, with `load_scene` or its lockstep-named twin, and the worlds
/// stay the same. (Refused loads from frame systems and tools are tested
/// inside the crate, in `src/scene.rs`.)
#[test]
fn loads_in_setup_and_simulation_systems_run_on_every_lockstep_peer() {
    let file = TempAsset("test-lifecycle-lockstep.json");
    // Written first: both peers read it.
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.world.reflect::<Tag>();
        ctx.world.spawn((Tag(7),));
        ctx.assets.save_scene(&ctx.world, file.0).unwrap();
    });
    let runs: Vec<_> = Net::loopback(2)
        .into_iter()
        .map(|net| {
            std::thread::spawn(move || {
                let mut world = Game::new("t", 1, 1)
                    .lockstep(net.checksum_every(5))
                    .sim_system(|ctx: &mut Ctx| {
                        ctx.net.set_input(&Step(ctx.time.sim_tick() as u32));
                        if ctx.time.sim_tick() == 10 {
                            assert_eq!(ctx.world.query::<&Tag>().iter().next().unwrap().0, 1);
                            ctx.load_scene("test-lifecycle-lockstep.json").unwrap();
                        }
                        if ctx.time.sim_tick() == 29 {
                            let desync = ctx.net.desync().cloned();
                            ctx.world.insert_resource(Some(desync));
                        }
                    })
                    .run_headless(30, |ctx| {
                        ctx.world.reflect::<Tag>();
                        assert_eq!(ctx.net.state(), NetState::Connecting);
                        // Setup runs on every peer before the first tick.
                        ctx.load_scene_lockstep("test-lifecycle-lockstep.json").unwrap();
                        ctx.world.query::<&mut Tag>().for_each(|t| t.0 = 1);
                    });
                let desync = world
                    .remove_resource::<Option<Option<Desync>>>()
                    .expect("ran every tick");
                let tags: Vec<u32> = world.query::<&Tag>().iter().map(|t| t.0).collect();
                (desync.flatten(), tags)
            })
        })
        .collect();
    for run in runs {
        let (desync, tags) = run.join().unwrap();
        assert_eq!(desync, None, "both peers loaded at the same tick");
        assert_eq!(tags, [7], "the scene's world");
    }
}

#[cfg(feature = "navmesh")]
mod navigation {
    use super::*;

    fn crowd_agent() -> NavAgent3D {
        let mut agent = NavAgent3D::default();
        agent.avoidance = true;
        agent
    }

    /// How far the walker strayed from its straight line.
    struct Swerve(f32);

    /// Scene A has a crowd agent standing in the walker's way; scene B has
    /// the same ids without it. At tick 5 the game loads `next` and puts
    /// crowd agents on the loaded entities again. Returns how far the
    /// walker swerved.
    fn walk(next: &'static str, a: &'static str, b: &'static str) -> f32 {
        let mut walker = None;
        let world = Game::new("t", 1, 1)
            .sim_system(move |ctx: &mut Ctx| {
                if ctx.time.sim_tick() == 5 {
                    ctx.load_scene(next).unwrap();
                    let loaded: Vec<(Entity, u32)> = ctx
                        .world
                        .query::<(Entity, &Tag)>()
                        .iter()
                        .map(|(e, t)| (e, t.0))
                        .collect();
                    for (e, kind) in loaded {
                        let mut agent = crowd_agent();
                        agent.destination = (kind == 1).then_some(vec3(6.0, 0.0, 0.0));
                        ctx.world.insert(e, agent);
                    }
                }
            })
            .sim_system(systems::step_navmesh3d)
            .sim_system(|ctx: &mut Ctx| {
                let z = ctx
                    .world
                    .query::<(&Transform3D, &Tag)>()
                    .iter()
                    .find(|(_, t)| t.0 == 1)
                    .map_or(0.0, |(at, _)| at.position.z.abs());
                let s = &mut ctx.world.resource_mut::<Swerve>().0;
                *s = s.max(z);
            })
            .run_headless(300, |ctx| {
                ctx.world.reflect::<Tag>();
                ctx.world.insert_resource(Swerve(0.0));
                let plane = NavSource::from_mesh(&MeshData::plane(20.0), Transform3D::default());
                ctx.world
                    .insert_resource(NavMesh3D::bake(&[plane], NavBakeSettings::default()).unwrap());
                let w = ctx.world.spawn((Transform3D::at(vec3(-6.0, 0.0, 0.0)), Tag(1)));
                let blocker = ctx.world.spawn((Transform3D::at(vec3(0.0, 0.0, 0.05)), Tag(2)));
                ctx.assets.save_scene(&ctx.world, a).unwrap();
                ctx.world.despawn(blocker);
                ctx.assets.save_scene(&ctx.world, b).unwrap();
                ctx.load_scene(a).unwrap();
                // Scene A's crowd before the load: both agents stand.
                ctx.world.insert(w, crowd_agent());
                ctx.world.insert(blocker, crowd_agent());
                walker = Some(w);
            });
        let at = world.get::<Transform3D>(walker.unwrap()).unwrap().position;
        assert!(at.x > 5.0, "arrived: {at}");
        world.resource::<Swerve>().0
    }

    #[test]
    fn crowd_agents_of_the_old_scene_leave_the_crowd() {
        let (a, b) = (
            TempAsset("test-lifecycle-nav-a.json"),
            TempAsset("test-lifecycle-nav-b.json"),
        );
        let (a2, b2) = (
            TempAsset("test-lifecycle-nav-a2.json"),
            TempAsset("test-lifecycle-nav-b2.json"),
        );
        // Control: reloading scene A keeps the blocker, and the walker goes round it.
        let round = walk(a2.0, a2.0, b2.0);
        assert!(round > 0.1, "the blocker is in the way: {round}");
        // Scene B has no blocker: no crowd slot of scene A's is left to avoid.
        let straight = walk(b.0, a.0, b.0);
        assert!(straight < 0.02, "went straight: {straight}");
        assert!(straight < round);
    }

    #[test]
    fn the_baked_navigation_mesh_survives_a_load() {
        let file = TempAsset("test-lifecycle-nav-mesh.json");
        let world = Game::new("t", 1, 1).run_headless(0, |ctx| {
            ctx.world.reflect::<Tag>();
            let plane = NavSource::from_mesh(&MeshData::plane(20.0), Transform3D::default());
            ctx.world
                .insert_resource(NavMesh3D::bake(&[plane], NavBakeSettings::default()).unwrap());
            ctx.world.spawn((Tag(1),));
            ctx.assets.save_scene(&ctx.world, file.0).unwrap();
            ctx.load_scene(file.0).unwrap();
        });
        let nav = world.resource::<NavMesh3D>();
        assert!(nav.find_path(vec3(-5.0, 0.0, 0.0), vec3(5.0, 0.0, 0.0)).is_some());
    }
}
