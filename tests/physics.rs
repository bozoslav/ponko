//! 2D physics (feature `physics`): bodies, colliders, events, queries.
#![cfg(feature = "physics")]

#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
#[allow(unused_imports)]
#[cfg(feature = "physics")]
use ponko::d2::{Collider, Hit as RayHit, RigidBody, Shape, Velocity};
use ponko::prelude::*;

const GRAVITY: Vec2 = Vec2::new(0.0, 980.0);

fn ball(at: Vec2) -> (Transform2D, RigidBody, Collider) {
    (Transform2D::at(at), RigidBody::dynamic(), Collider::circle(10.0))
}
fn ground(at: Vec2) -> (Transform2D, RigidBody, Collider) {
    (
        Transform2D::at(at),
        RigidBody::fixed(),
        Collider::rect(vec2(400.0, 20.0)),
    )
}
fn position(world: &mut World, e: Entity) -> Vec2 {
    world.get::<Transform2D>(e).unwrap().position
}

#[test]
fn a_falling_ball_comes_to_rest_on_the_ground() {
    let mut ball_entity = None;
    let mut world = Game::new("t", 1, 1).physics(GRAVITY).run_headless(180, |ctx| {
        ball_entity = Some(ctx.world.spawn(ball(vec2(0.0, 0.0))));
        ctx.world.spawn(ground(vec2(0.0, 200.0)));
    });
    // Ground top at 190, ball radius 10: it rests near y = 180.
    let y = position(&mut world, ball_entity.unwrap()).y;
    assert!((y - 180.0).abs() < 1.5, "rests at {y}");
}

#[test]
fn moving_the_transform_teleports_the_body() {
    let mut world = Game::new("t", 1, 1)
        .physics(Vec2::ZERO)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 5 {
                ctx.world
                    .query::<&mut Transform2D>()
                    .filter::<With<RigidBody>>()
                    .for_each(|t| t.position = vec2(500.0, 7.0));
            }
        })
        .run_headless(10, |ctx| {
            ctx.world.spawn(ball(vec2(0.0, 0.0)));
        });
    let p = world.query::<&Transform2D>().iter().next().unwrap().position;
    assert_eq!(p, vec2(500.0, 7.0), "no gravity, no speed: it stays where it was put");
}

#[test]
fn velocity_sets_and_reports_speed() {
    let mut world = Game::new("t", 1, 1).physics(Vec2::ZERO).run_headless(60, |ctx| {
        let (t, r, c) = ball(vec2(0.0, 0.0));
        ctx.world.spawn((t, r, c, Velocity::linear(vec2(120.0, 0.0))));
    });
    let (x, speed) = world
        .query::<(&Transform2D, &Velocity)>()
        .iter()
        .map(|(t, v)| (t.position.x, v.linear.x))
        .next()
        .unwrap();
    assert!((x - 120.0).abs() < 1.0, "one second at 120 px/s: {x}");
    assert!((speed - 120.0).abs() < 1e-3);
}

#[test]
fn bodies_without_a_velocity_component_keep_their_speed() {
    let mut world = Game::new("t", 1, 1).physics(GRAVITY).run_headless(60, |ctx| {
        ctx.world.spawn(ball(vec2(0.0, 0.0)));
    });
    let y = world.query::<&Transform2D>().iter().next().unwrap().position.y;
    assert!((y - 490.0).abs() < 10.0, "free fall for a second: {y}");
}

#[test]
fn body_counts_follow_the_world() {
    let counts = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = counts.clone();
    Game::new("t", 1, 1)
        .add(Phase::BeforePhysics, |ctx: &mut Ctx| {
            let bodies: Vec<Entity> = ctx.world.query::<Entity>().filter::<With<RigidBody>>().iter().collect();
            match ctx.time.sim_tick() {
                1 => {
                    ctx.world.despawn(bodies[0]);
                }
                2 => {
                    ctx.world.remove::<Collider>(bodies[0]);
                }
                3 => {
                    // Reuses the despawned entity's index: a new body, not the old one.
                    ctx.world.spawn(ball(vec2(50.0, 0.0)));
                }
                _ => {}
            }
        })
        .physics(Vec2::ZERO)
        .sim_system(move |ctx: &mut Ctx| seen.borrow_mut().push(ctx.physics.body_count()))
        .run_headless(4, |ctx| {
            for x in 0..3 {
                ctx.world.spawn(ball(vec2(x as f32 * 100.0, 0.0)));
            }
        });
    assert_eq!(*counts.borrow(), [3, 2, 1, 2]);
}

#[derive(Default)]
struct Seen(Vec<(Entity, Entity, bool, bool)>);

#[test]
fn collisions_and_sensor_overlaps_arrive_as_events() {
    let mut ids = (None, None, None);
    let world = Game::new("t", 1, 1)
        .physics(GRAVITY)
        .sim_system(|ctx: &mut Ctx| {
            let mut seen = Vec::new();
            for c in ctx.events.read::<CollisionStarted>() {
                seen.push((c.a, c.b, c.sensor, true));
            }
            ctx.world.resource_mut::<Seen>().0.extend(seen);
        })
        .run_headless(120, |ctx| {
            ctx.world.insert_resource(Seen::default());
            ids.0 = Some(ctx.world.spawn(ball(vec2(0.0, 0.0))));
            ids.1 = Some(ctx.world.spawn((
                Transform2D::at(vec2(0.0, 100.0)),
                RigidBody::fixed(),
                Collider::rect(vec2(80.0, 20.0)).sensor(),
            )));
            ids.2 = Some(ctx.world.spawn(ground(vec2(0.0, 300.0))));
        });
    let (ball, trigger, floor) = (ids.0.unwrap(), ids.1.unwrap(), ids.2.unwrap());
    let seen = &world.resource::<Seen>().0;
    let pair = |x: Entity, y: Entity| {
        seen.iter()
            .find(|s| (s.0 == x && s.1 == y) || (s.0 == y && s.1 == x))
            .copied()
    };
    assert!(
        pair(ball, trigger).expect("passed through the sensor").2,
        "flagged as a sensor"
    );
    assert!(!pair(ball, floor).expect("landed on the floor").2);
    let y = world.get::<Transform2D>(ball).unwrap().position.y;
    assert!(y > 250.0, "the sensor didn't stop it: {y}");
}

#[derive(Default)]
struct Found {
    ray: Option<RayHit>,
    at: Vec<Entity>,
    near: Vec<Entity>,
    behind: Option<RayHit>,
}

#[test]
fn rays_and_overlap_queries_find_entities() {
    let mut wall = None;
    let mut world = Game::new("t", 1, 1)
        .physics(Vec2::ZERO)
        .sim_system(|ctx: &mut Ctx| {
            // Queries see the bodies once they have been stepped.
            if ctx.time.sim_tick() == 1 {
                let sensors = ponko::d2::QueryFilter::default().with_sensors();
                let mut found = Found {
                    ray: ctx.physics.cast_ray(vec2(0.0, 0.0), vec2(1.0, 0.0), 1000.0, sensors),
                    behind: ctx.physics.cast_ray(vec2(0.0, 0.0), vec2(-1.0, 0.0), 1000.0, sensors),
                    ..Found::default()
                };
                ctx.physics.overlap_point(vec2(300.0, 5.0), sensors, &mut found.at);
                ctx.physics
                    .overlap_circle(vec2(200.0, 0.0), 95.0, sensors, &mut found.near);
                ctx.world.insert_resource(found);
            }
        })
        .run_headless(2, |ctx| {
            wall = Some(ctx.world.spawn((
                Transform2D::at(vec2(100.0, 0.0)),
                RigidBody::fixed(),
                Collider::rect(vec2(20.0, 200.0)),
            )));
            ctx.world.spawn((
                Transform2D::at(vec2(300.0, 0.0)),
                RigidBody::fixed(),
                Collider::circle(10.0),
            ));
        });
    let found = world.remove_resource::<Found>().unwrap();
    let hit = found.ray.expect("the ray hits the wall");
    assert_eq!(hit.entity, wall.unwrap());
    assert!((hit.distance - 90.0).abs() < 1e-3, "{}", hit.distance);
    assert!((hit.point - vec2(90.0, 0.0)).length() < 1e-3);
    assert!((hit.normal - vec2(-1.0, 0.0)).length() < 1e-3);
    assert_eq!(found.at.len(), 1, "the point is inside the circle only");
    assert_eq!(found.near.len(), 2, "the circle reaches both");
    assert!(found.behind.is_none(), "nothing behind");
}

#[test]
fn impulses_push_dynamic_bodies() {
    let mut world = Game::new("t", 1, 1)
        .physics(Vec2::ZERO)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 1 {
                let e = ctx
                    .world
                    .query::<Entity>()
                    .filter::<With<RigidBody>>()
                    .iter()
                    .next()
                    .unwrap();
                let mass = ctx.physics.mass(e).unwrap();
                assert!(
                    (mass - std::f32::consts::PI * 100.0).abs() < 1.0,
                    "density 1 x area: {mass}"
                );
                assert!(ctx.physics.apply_impulse(e, vec2(mass * 60.0, 0.0)));
            }
        })
        .run_headless(61, |ctx| {
            ctx.world.spawn(ball(vec2(0.0, 0.0)));
        });
    let x = world.query::<&Transform2D>().iter().next().unwrap().position.x;
    assert!((x - 60.0).abs() < 2.0, "60 px/s for about a second: {x}");
}

fn pile(seed_offset: f32) -> Vec<(Vec2, f32)> {
    let mut world = Game::new("t", 1, 1).physics(GRAVITY).run_headless(240, |ctx| {
        ctx.world.spawn(ground(vec2(0.0, 300.0)));
        for i in 0..40 {
            let x = (i % 8) as f32 * 19.0 - 70.0 + seed_offset;
            let y = -(i / 8) as f32 * 25.0;
            let (t, r, _) = ball(vec2(x, y));
            ctx.world
                .spawn((t, r, Collider::rect(vec2(16.0, 16.0)).restitution(0.2)));
        }
    });
    world
        .query::<&Transform2D>()
        .filter::<With<RigidBody>>()
        .iter()
        .map(|t| (t.position, t.rotation))
        .collect()
}

#[test]
fn the_same_start_gives_the_same_pile() {
    let a = pile(0.0);
    let b = pile(0.0);
    assert_eq!(a, b, "bit-identical");
    assert_ne!(a, pile(0.5), "and the start matters");
}

#[test]
fn physics_components_are_reflected() {
    let mut world = Game::new("t", 1, 1).physics(Vec2::ZERO).run_headless(1, |ctx| {
        let (t, r, c) = ball(vec2(1.0, 2.0));
        ctx.world.spawn((t, r, c, Velocity::default()));
    });
    let e = world
        .query::<Entity>()
        .filter::<With<RigidBody>>()
        .iter()
        .next()
        .unwrap();
    let json = world.entity_json(e).unwrap();
    assert_eq!(json["RigidBody"]["kind"], "Dynamic");
    assert_eq!(json["Collider"]["shape"]["Circle"]["radius"], 10.0);
    world.set_json(e, "Velocity", json!({ "linear": [5.0, 0.0] })).unwrap();
    assert_eq!(world.get::<Velocity>(e).unwrap().linear, vec2(5.0, 0.0));
}

#[test]
fn changing_a_collider_rebuilds_it() {
    let mut world = Game::new("t", 1, 1)
        .physics(GRAVITY)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 10 {
                // The ball turns into a sensor and falls through the ground.
                ctx.world
                    .query::<&mut Collider>()
                    .filter::<With<RigidBody>>()
                    .for_each(|c| {
                        if matches!(c.shape, Shape::Circle { .. }) {
                            c.sensor = true;
                        }
                    });
            }
        })
        .run_headless(120, |ctx| {
            ctx.world.spawn(ball(vec2(0.0, 150.0)));
            ctx.world.spawn(ground(vec2(0.0, 200.0)));
        });
    let lowest = world
        .query::<&Transform2D>()
        .iter()
        .map(|t| t.position.y)
        .fold(f32::MIN, f32::max);
    assert!(lowest > 300.0, "fell through: {lowest}");
}

/// A wedge, flat side down: 60 wide, 30 high, corners around its position.
fn wedge() -> Collider {
    Collider::polygon(&[vec2(-30.0, 15.0), vec2(30.0, 15.0), vec2(-30.0, -15.0)])
}

#[test]
fn a_falling_polygon_comes_to_rest_on_its_flat_side() {
    let mut piece = None;
    let world = Game::new("t", 1, 1).physics(GRAVITY).run_headless(240, |ctx| {
        piece = Some(
            ctx.world
                .spawn((Transform2D::at(vec2(0.0, 0.0)), RigidBody::dynamic(), wedge())),
        );
        ctx.world.spawn(ground(vec2(0.0, 200.0)));
    });
    let t = *world.get::<Transform2D>(piece.unwrap()).unwrap();
    // Ground top at 190, flat side 15 below the position: rests near y = 175.
    assert!((t.position.y - 175.0).abs() < 1.5, "rests at {}", t.position.y);
    assert!(t.rotation.abs() < 0.01, "stays flat: {}", t.rotation);
}

#[test]
fn polygons_survive_save_and_load() {
    let mut world = World::new();
    world.reflect::<Collider>();
    let e = world.spawn((wedge(),));
    let save = world.save();
    world.get_mut::<Collider>(e).unwrap().shape = Collider::circle(1.0).shape;
    world.load(&save).unwrap();
    assert_eq!(world.get::<Collider>(e).unwrap().shape.points(), wedge().shape.points());
}

#[test]
#[should_panic(expected = "Collider::polygon: 3 to 8 points, got 2")]
fn a_polygon_needs_three_points() {
    let _ = Collider::polygon(&[Vec2::ZERO, Vec2::X]);
}

#[test]
#[should_panic(expected = "Collider::polygon: 3 to 8 points, got 9")]
fn a_polygon_has_at_most_eight_points() {
    let _ = Collider::polygon(&[Vec2::ONE; 9]);
}

#[test]
fn collider_json_needs_only_a_shape() {
    let mut world = World::new();
    world.reflect::<Collider>();
    let e = world.spawn((Transform2D::default(),));
    world
        .set_json(e, "Collider", json!({ "shape": { "Circle": { "radius": 4.0 } } }))
        .unwrap();
    assert_eq!(*world.get::<Collider>(e).unwrap(), Collider::circle(4.0));
}

/// Holds physics for ticks 10..30 from a sim system, which keeps running.
fn pause_menu(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    *ctx.world.resource_mut::<u64>() += 1;
    if tick == 10 {
        ctx.physics.set_paused(true);
    }
    if tick == 30 {
        ctx.physics.set_paused(false);
    }
}

#[test]
fn pausing_physics_holds_bodies_while_sim_systems_keep_running() {
    struct Heights(Vec<f32>);
    let mut entity = None;
    let world = Game::new("t", 1, 1)
        .physics(GRAVITY)
        .sim_system(pause_menu)
        .add(Phase::SimLast, move |ctx: &mut Ctx| {
            let y = ctx.world.query::<&Transform2D>().iter().next().unwrap().position.y;
            let paused = ctx.physics.is_paused();
            assert_eq!(
                paused,
                (10..30).contains(&ctx.time.sim_tick()),
                "flag at tick {}",
                ctx.time.sim_tick()
            );
            ctx.world.resource_mut::<Heights>().0.push(y);
        })
        .run_headless(60, |ctx| {
            ctx.world.insert_resource(0_u64);
            ctx.world.insert_resource(Heights(Vec::new()));
            entity = Some(ctx.world.spawn(ball(vec2(0.0, 0.0))));
        });
    assert_eq!(*world.resource::<u64>(), 60, "the sim system ran every tick");
    let heights = &world.resource::<Heights>().0;
    // Sim systems run after the step by default, so the menu's pause on
    // tick 10 holds ticks 11 to 30. The ball falls before the pause, holds
    // still through it, and falls on after.
    assert!(heights[10] > heights[0]);
    assert!(heights[11..=30].iter().all(|&y| y == heights[10]), "{heights:?}");
    assert!(heights[31] > heights[30]);

    // Twenty held ticks change nothing else: the ball ends where an
    // unpaused run puts it twenty ticks earlier.
    let mut unpaused = Game::new("t", 1, 1).physics(GRAVITY).run_headless(40, |ctx| {
        ctx.world.spawn(ball(vec2(0.0, 0.0)));
    });
    let mut world = world;
    let held = position(&mut world, entity.unwrap()).y;
    let free = unpaused.query::<&Transform2D>().iter().next().unwrap().position.y;
    assert_eq!(held, free);
}

#[test]
fn no_collision_events_arrive_while_physics_is_paused() {
    // The ball starts inside the ground: any step would report the touch.
    let run = |paused: bool| {
        let mut entity = None;
        let mut world = Game::new("t", 1, 1)
            .physics(GRAVITY)
            .sim_system(|ctx: &mut Ctx| {
                let started = ctx.events.read::<CollisionStarted>().len() as u32;
                *ctx.world.resource_mut::<u32>() += started;
            })
            .run_headless(30, |ctx| {
                ctx.world.insert_resource(0_u32);
                ctx.physics.set_paused(paused);
                entity = Some(ctx.world.spawn(ball(vec2(0.0, 185.0))));
                ctx.world.spawn(ground(vec2(0.0, 200.0)));
            });
        (*world.resource::<u32>(), position(&mut world, entity.unwrap()).y)
    };
    assert_eq!(run(true), (0, 185.0), "held: no events, no movement");
    assert!(run(false).0 > 0, "the same scene unpaused reports the touch");
}
