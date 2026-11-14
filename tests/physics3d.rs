//! 3D physics (feature `physics3d`): bodies, colliders, heightfields,
//! queries and the character controller.
#![cfg(feature = "physics3d")]

use std::cell::Cell;
use std::rc::Rc;

mod common;

use common::TempAsset;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform as Transform3D;
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::{
    Character as Character3D, Collider as Collider3D, Physics as Physics3D, QueryFilter as QueryFilter3D,
    RigidBody as RigidBody3D, Velocity as Velocity3D,
};
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::{CollisionGroups, CollisionMesh};
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::Mesh;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::terrain::Heightfield;

const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

fn position(world: &mut World, e: Entity) -> Vec3 {
    world.get::<Transform3D>(e).unwrap().position
}

/// Runs `setup`, steps physics `ticks` times, then calls `query` (in a sim
/// system, so it sees the physics world) and returns what it returned.
fn query_after<T: 'static>(
    ticks: u64,
    gravity: Vec3,
    setup: impl FnOnce(&mut Ctx) + 'static,
    mut query: impl FnMut(&mut Ctx) -> T + 'static,
) -> T {
    let out = Rc::new(std::cell::RefCell::new(None));
    let slot = out.clone();
    Game::new("t", 1, 1)
        .physics3d(gravity)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == ticks {
                *slot.borrow_mut() = Some(query(ctx));
            }
        })
        .run_headless(ticks + 1, setup);
    out.take().expect("ran the query")
}

/// A 40 × 1 × 40 m slab whose top is at y = 0.
fn floor() -> (Transform3D, RigidBody3D, Collider3D) {
    (
        Transform3D::at(vec3(0.0, -0.5, 0.0)),
        RigidBody3D::fixed(),
        Collider3D::cuboid(vec3(40.0, 1.0, 40.0)),
    )
}

#[test]
fn physics3d_steps_at_50_hz_unless_the_game_chose_a_rate() {
    let mut dt = 0.0;
    Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .run_headless(1, |ctx| dt = ctx.time.fixed_dt());
    assert!((dt - 0.02).abs() < 1e-6, "{dt}");
    Game::new("t", 1, 1)
        .fixed_hz(120)
        .physics3d(GRAVITY)
        .run_headless(1, |ctx| dt = ctx.time.fixed_dt());
    assert!((dt - 1.0 / 120.0).abs() < 1e-6, "{dt}");
}

#[test]
fn a_falling_ball_comes_to_rest_on_a_box() {
    let mut ball = None;
    let mut world = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(150, |ctx| {
        ctx.world.spawn(floor());
        ball = Some(ctx.world.spawn((
            Transform3D::at(vec3(1.0, 4.0, 2.0)),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.5),
        )));
    });
    let p = position(&mut world, ball.unwrap());
    assert!((p.y - 0.5).abs() < 0.02, "rests at {p}");
    assert!(
        (p.x - 1.0).abs() < 0.01 && (p.z - 2.0).abs() < 0.01,
        "fell straight: {p}"
    );
}

#[test]
fn a_falling_box_turns_and_writes_its_rotation_back() {
    let mut cube = None;
    let world = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(150, |ctx| {
        ctx.world.spawn(floor());
        // Dropped on a corner: it tips over onto a face.
        let tilted = Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.5);
        cube = Some(ctx.world.spawn((
            Transform3D::at(vec3(0.0, 3.0, 0.0)).rotated(tilted),
            RigidBody3D::dynamic(),
            Collider3D::cuboid(Vec3::ONE),
        )));
    });
    let t = *world.get::<Transform3D>(cube.unwrap()).unwrap();
    assert!((t.position.y - 0.5).abs() < 0.03, "lies flat: {}", t.position);
    // A face down: one of its axes points straight up or down.
    let up = [Vec3::X, Vec3::Y, Vec3::Z].map(|a| (t.rotation * a).y.abs());
    assert!(up.iter().any(|&u| u > 0.99), "{up:?}");
}

/// Heights that differ along X and along Z, so a flipped or transposed
/// grid shows.
fn bumpy(n: usize) -> Vec<f32> {
    let mut h = Vec::with_capacity(n * n);
    for z in 0..n {
        for x in 0..n {
            let (x, z) = (x as f32, z as f32);
            h.push(0.4 * x + 0.05 * z * z + ((x * 1.3).sin() * (z * 0.7).cos()));
        }
    }
    h
}

#[test]
fn a_heightfield_collider_matches_the_drawn_ground() {
    let n = 9;
    let heights = bumpy(n);
    let size = 16.0;
    let corner = vec3(30.0, -2.0, -50.0);
    let ground = Heightfield::new(&heights, size);
    // Rays straight down, at points inside cells on both sides of each
    // cell's diagonal, compared with the CPU heights the renderer uses.
    let mut game_points = Vec::new();
    for i in 0..40 {
        let x = (i as f32 * 0.37) % size;
        let z = (i as f32 * 0.61 + 0.13) % size;
        game_points.push((x, z));
    }
    let points = game_points.clone();
    Game::new("t", 1, 1)
        .physics3d(Vec3::ZERO)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() != 1 {
                return;
            }
            let mut found = Vec::new();
            for &(x, z) in &points {
                let from = corner + vec3(x, 100.0, z);
                let hit = ctx
                    .physics3d
                    .cast_ray(from, Vec3::NEG_Y, 200.0, QueryFilter3D::default());
                found.push(hit.map(|h| h.point.y - corner.y));
            }
            let drawn: Vec<Option<f32>> = points.iter().map(|&(x, z)| ground.height_at(x, z)).collect();
            for ((x, z), (hit, want)) in points.iter().zip(found.iter().zip(&drawn)) {
                let (hit, want) = (hit.expect("hit the ground"), want.unwrap());
                assert!((hit - want).abs() < 1e-3, "at ({x}, {z}): collider {hit}, drawn {want}");
            }
        })
        .run_headless(2, |ctx| {
            let shape = ctx.physics3d.heightfield(&heights, size);
            ctx.world
                .spawn((Transform3D::at(corner), RigidBody3D::fixed(), Collider3D::mesh(shape)));
        });
}

#[test]
fn a_ball_rests_on_a_heightfield_and_falls_through_its_holes() {
    let n = 5;
    let mut heights = vec![3.0; n * n];
    // A hole at grid point (x 3, z 3): the four cells around it are open.
    heights[3 * n + 3] = f32::NAN;
    let (mut on, mut over_hole) = (None, None);
    let mut world = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(120, |ctx| {
        let shape = ctx.physics3d.heightfield(&heights, 8.0);
        ctx.world
            .spawn((Transform3D::default(), RigidBody3D::fixed(), Collider3D::mesh(shape)));
        on = Some(ctx.world.spawn((
            Transform3D::at(vec3(1.0, 6.0, 1.0)),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.25),
        )));
        // Grid points are 2 m apart: (6, 6) is the hole.
        over_hole = Some(ctx.world.spawn((
            Transform3D::at(vec3(6.0, 6.0, 6.0)),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.25),
        )));
    });
    let y = position(&mut world, on.unwrap()).y;
    assert!((y - 3.25).abs() < 0.02, "rests at {y}");
    let y = position(&mut world, over_hole.unwrap()).y;
    assert!(y < 0.0, "fell through the hole: {y}");
}

#[test]
#[should_panic(expected = "isn't a square grid")]
fn a_heightfield_needs_a_square_grid() {
    let mut physics = Physics3D::default();
    let _ = physics.heightfield(&[0.0; 6], 1.0);
}

#[test]
fn moving_the_transform_teleports_the_body() {
    let mut world = Game::new("t", 1, 1)
        .physics3d(Vec3::ZERO)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 5 {
                ctx.world
                    .query::<&mut Transform3D>()
                    .filter::<With<RigidBody3D>>()
                    .for_each(|t| t.position = vec3(50.0, 7.0, -3.0));
            }
        })
        .run_headless(10, |ctx| {
            ctx.world
                .spawn((Transform3D::default(), RigidBody3D::dynamic(), Collider3D::sphere(1.0)));
        });
    let p = world.query::<&Transform3D>().iter().next().unwrap().position;
    assert_eq!(p, vec3(50.0, 7.0, -3.0));
}

#[test]
fn velocity_sets_and_reports_speed() {
    let mut world = Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(50, |ctx| {
        ctx.world.spawn((
            Transform3D::default(),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.5),
            Velocity3D::linear(vec3(0.0, 0.0, -3.0)),
        ));
    });
    let (t, v) = world
        .query::<(&Transform3D, &Velocity3D)>()
        .iter()
        .map(|(t, v)| (*t, *v))
        .next()
        .unwrap();
    assert!((t.position.z + 3.0).abs() < 0.01, "one second at 3 m/s: {}", t.position);
    assert!((v.linear.z + 3.0).abs() < 1e-3);
}

#[test]
fn bodies_leave_the_world_when_despawned() {
    let count = Rc::new(Cell::new(usize::MAX));
    let seen = count.clone();
    Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 2 {
                let all: Vec<Entity> = ctx
                    .world
                    .query::<Entity>()
                    .filter::<With<RigidBody3D>>()
                    .iter()
                    .collect();
                for e in all {
                    ctx.world.despawn(e);
                }
            }
            seen.set(ctx.physics3d.body_count());
        })
        .run_headless(5, |ctx| {
            ctx.world.spawn(floor());
            ctx.world.spawn((
                Transform3D::at(Vec3::Y),
                RigidBody3D::dynamic(),
                Collider3D::sphere(0.5),
            ));
        });
    assert_eq!(count.get(), 0);
}

#[test]
fn collisions_are_events() {
    let started = Rc::new(Cell::new(0));
    let seen = started.clone();
    Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| seen.set(seen.get() + ctx.events.read::<CollisionStarted>().len()))
        .run_headless(100, |ctx| {
            ctx.world.spawn(floor());
            ctx.world.spawn((
                Transform3D::at(vec3(0.0, 2.0, 0.0)),
                RigidBody3D::dynamic(),
                Collider3D::sphere(0.5),
            ));
        });
    assert_eq!(started.get(), 1);
}

#[test]
fn collision_groups_let_bodies_pass_through_each_other() {
    const GROUND: u32 = 1 << 0;
    const GHOST: u32 = 1 << 1;
    let (mut ghost, mut ball) = (None, None);
    let mut world = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(60, |ctx| {
        let (t, r, c) = floor();
        ctx.world
            .spawn((t, r, c.groups(CollisionGroups::new(GROUND, u32::MAX))));
        ghost = Some(ctx.world.spawn((
            Transform3D::at(vec3(0.0, 2.0, 0.0)),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.5).groups(CollisionGroups::new(GHOST, !GROUND)),
        )));
        ball = Some(ctx.world.spawn((
            Transform3D::at(vec3(3.0, 2.0, 0.0)),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.5),
        )));
    });
    let y = position(&mut world, ghost.unwrap()).y;
    assert!(y < -1.0, "the ghost doesn't touch the ground: {y}");
    let y = position(&mut world, ball.unwrap()).y;
    assert!((y - 0.5).abs() < 0.02, "the ball does: {y}");
}

#[test]
fn restitution_bounces_and_friction_slows() {
    // Each surface is averaged with what it touches.
    let peak = Rc::new(Cell::new(0.0f32));
    let seen = peak.clone();
    let (mut ice, mut rubber) = (None, None);
    let mut world = Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() > 50 {
                ctx.world
                    .query::<(&Transform3D, &Collider3D)>()
                    .iter()
                    .filter(|(_, c)| c.restitution > 0.5)
                    .for_each(|(t, _)| seen.set(seen.get().max(t.position.y)));
            }
        })
        .run_headless(100, |ctx| {
            let (t, r, c) = floor();
            ctx.world.spawn((t, r, c.restitution(0.9)));
            // A bouncy ball dropped from 3 m.
            ctx.world.spawn((
                Transform3D::at(vec3(-3.0, 3.0, 5.0)),
                RigidBody3D::dynamic(),
                Collider3D::sphere(0.5).restitution(0.9),
            ));
            // Two crates sliding at 4 m/s.
            for (z, friction, slot) in [(0.0, 0.0, &mut ice), (-3.0, 1.0, &mut rubber)] {
                *slot = Some(ctx.world.spawn((
                    Transform3D::at(vec3(0.0, 0.5, z)),
                    RigidBody3D::dynamic(),
                    Collider3D::cuboid(Vec3::ONE).friction(friction),
                    Velocity3D::linear(vec3(4.0, 0.0, 0.0)),
                )));
            }
        });
    assert!(peak.get() > 1.5, "bounced back up to {}", peak.get());
    let (ice, rubber) = (
        position(&mut world, ice.unwrap()).x,
        position(&mut world, rubber.unwrap()).x,
    );
    assert!(ice > rubber + 1.0, "ice slides further: {ice} vs {rubber}");
}

#[test]
fn rays_hit_the_closest_and_all_hits_come_sorted() {
    let sensor = Rc::new(Cell::new(None));
    let sensor_set = sensor.clone();
    let (closest, hits, not_first, with_sensor) = query_after(
        1,
        Vec3::ZERO,
        move |ctx| {
            // Walls at x = 9, 3 and 6 (spawned out of order), 1 m thick.
            for x in [9.0, 3.0, 6.0] {
                ctx.world.spawn((
                    Transform3D::at(vec3(x, 0.0, 0.0)),
                    RigidBody3D::fixed(),
                    Collider3D::cuboid(vec3(1.0, 4.0, 4.0)),
                ));
            }
            sensor_set.set(Some(ctx.world.spawn((
                Transform3D::at(vec3(1.5, 0.0, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::cuboid(vec3(1.0, 4.0, 4.0)).sensor(),
            ))));
        },
        |ctx| {
            let f = QueryFilter3D::default();
            let closest = ctx.physics3d.cast_ray(Vec3::ZERO, Vec3::X, 100.0, f);
            let mut hits = Vec::new();
            ctx.physics3d.cast_ray_all(Vec3::ZERO, Vec3::X, 100.0, f, &mut hits);
            let first = closest.unwrap().entity;
            let not_first = ctx.physics3d.cast_ray(Vec3::ZERO, Vec3::X, 100.0, f.exclude(first));
            let mut with_sensor = Vec::new();
            ctx.physics3d
                .cast_ray_all(Vec3::ZERO, Vec3::X, 100.0, f.with_sensors(), &mut with_sensor);
            (closest, hits, not_first, with_sensor)
        },
    );
    let closest = closest.expect("hit a wall");
    assert!((closest.distance - 2.5).abs() < 1e-4, "{closest:?}");
    assert!((closest.normal - Vec3::NEG_X).length() < 1e-4, "{closest:?}");
    assert!((closest.point - vec3(2.5, 0.0, 0.0)).length() < 1e-4);
    let distances: Vec<f32> = hits.iter().map(|h| h.distance).collect();
    assert_eq!(distances.len(), 3, "sensors are left out by default: {distances:?}");
    for (d, want) in distances.iter().zip([2.5, 5.5, 8.5]) {
        assert!((d - want).abs() < 1e-4, "{distances:?}");
    }
    assert!((not_first.unwrap().distance - 5.5).abs() < 1e-4);
    assert_eq!(with_sensor.len(), 4);
    assert_eq!(Some(with_sensor[0].entity), sensor.get());
}

#[test]
fn shape_casts_stop_where_the_shape_first_touches() {
    let (sphere, capsule, missed, grouped) = query_after(
        1,
        Vec3::ZERO,
        |ctx| {
            ctx.world.spawn((
                Transform3D::at(vec3(5.0, 0.0, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::cuboid(vec3(1.0, 4.0, 4.0)).groups(CollisionGroups::new(1, u32::MAX)),
            ));
        },
        |ctx| {
            let f = QueryFilter3D::default();
            let p = &ctx.physics3d;
            (
                p.cast_sphere(Vec3::ZERO, 0.5, Vec3::X, 100.0, f),
                p.cast_capsule(Vec3::ZERO, 1.0, 0.25, Vec3::X, 100.0, f),
                // Too short to reach.
                p.cast_sphere(Vec3::ZERO, 0.5, Vec3::X, 3.0, f),
                // A query that doesn't touch the wall's group.
                p.cast_sphere(
                    Vec3::ZERO,
                    0.5,
                    Vec3::X,
                    100.0,
                    f.groups(CollisionGroups::new(u32::MAX, 1 << 3)),
                ),
            )
        },
    );
    let s = sphere.expect("the sphere hits the wall");
    assert!((s.distance - 4.0).abs() < 1e-3, "{s:?}");
    assert!(
        (s.point.x - 4.5).abs() < 1e-3 && (s.normal - Vec3::NEG_X).length() < 1e-3,
        "{s:?}"
    );
    let c = capsule.expect("the capsule hits the wall");
    assert!((c.distance - 4.25).abs() < 1e-3, "{c:?}");
    assert!(missed.is_none());
    assert!(grouped.is_none());
}

#[test]
fn overlap_tests_find_what_is_inside() {
    let near = Rc::new(Cell::new(None));
    let near_set = near.clone();
    let (found, boxed, capsuled) = query_after(
        1,
        Vec3::ZERO,
        move |ctx| {
            near_set.set(Some(ctx.world.spawn((
                Transform3D::at(vec3(1.5, 0.0, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::sphere(1.0),
            ))));
            ctx.world.spawn((
                Transform3D::at(vec3(10.0, 0.0, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::sphere(0.5),
            ));
            ctx.world.spawn((
                Transform3D::at(vec3(0.0, 2.2, 10.0)),
                RigidBody3D::fixed(),
                Collider3D::sphere(0.5),
            ));
            ctx.world.spawn((
                Transform3D::at(vec3(0.0, 40.0, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::sphere(1.0),
            ));
        },
        |ctx| {
            let f = QueryFilter3D::default();
            let (mut found, mut boxed, mut capsuled) = (Vec::new(), Vec::new(), Vec::new());
            ctx.physics3d.overlap_sphere(Vec3::ZERO, 2.0, f, &mut found);
            ctx.physics3d
                .overlap_box(vec3(10.0, 0.0, 0.0), Vec3::splat(2.0), Quat::IDENTITY, f, &mut boxed);
            ctx.physics3d
                .overlap_capsule(vec3(0.0, 0.0, 10.0), 4.0, 0.5, f, &mut capsuled);
            (found, boxed, capsuled)
        },
    );
    assert_eq!(found, vec![near.get().unwrap()]);
    assert_eq!(boxed.len(), 1);
    assert_eq!(capsuled.len(), 1);
}

#[test]
fn convex_hulls_and_trimeshes_collide() {
    let mut rock = None;
    let mut world = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(150, |ctx| {
        // A floor made of two triangles, and a pyramid dropped on it.
        let floor = ctx
            .physics3d
            .trimesh(
                &[
                    vec3(-10.0, 0.0, -10.0),
                    vec3(10.0, 0.0, -10.0),
                    vec3(-10.0, 0.0, 10.0),
                    vec3(10.0, 0.0, 10.0),
                ],
                &[[0, 2, 1], [1, 2, 3]],
            )
            .unwrap();
        ctx.world
            .spawn((Transform3D::default(), RigidBody3D::fixed(), Collider3D::mesh(floor)));
        let pyramid = ctx
            .physics3d
            .convex_hull(&[
                vec3(-0.5, 0.0, -0.5),
                vec3(0.5, 0.0, -0.5),
                vec3(-0.5, 0.0, 0.5),
                vec3(0.5, 0.0, 0.5),
                vec3(0.0, 1.0, 0.0),
            ])
            .unwrap();
        rock = Some(ctx.world.spawn((
            Transform3D::at(vec3(0.0, 3.0, 0.0)),
            RigidBody3D::dynamic(),
            Collider3D::mesh(pyramid),
        )));
    });
    let y = position(&mut world, rock.unwrap()).y;
    assert!(y.abs() < 0.02, "the pyramid stands on its base: {y}");
}

#[test]
fn freed_meshes_are_gone_and_their_handles_stay_dead() {
    let mut physics = Physics3D::default();
    let a = physics.heightfield(&[0.0; 4], 1.0);
    assert!(physics.is_live(a));
    physics.free(a);
    assert!(!physics.is_live(a));
    let b = physics.heightfield(&[0.0; 4], 1.0);
    assert_ne!(a, b, "the slot is reused under a new generation");
    physics.free(a);
    assert!(physics.is_live(b), "freeing a stale handle does nothing");
}

#[test]
fn impulses_push_dynamic_bodies() {
    let mut ball = None;
    let mut world = Game::new("t", 1, 1)
        .physics3d(Vec3::ZERO)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 1 {
                let e = ctx
                    .world
                    .query::<Entity>()
                    .filter::<With<RigidBody3D>>()
                    .iter()
                    .next()
                    .unwrap();
                let mass = ctx.physics3d.mass(e).unwrap();
                assert!(ctx.physics3d.apply_impulse(e, vec3(2.0 * mass, 0.0, 0.0)));
            }
        })
        .run_headless(51, |ctx| {
            ball = Some(
                ctx.world
                    .spawn((Transform3D::default(), RigidBody3D::dynamic(), Collider3D::sphere(0.5))),
            );
        });
    let x = position(&mut world, ball.unwrap()).x;
    assert!((x - 2.0).abs() < 0.05, "2 m/s for a second: {x}");
}

// --- the character controller ---

/// Flat ground for x < 0, then a slope rising at `degrees` along +X.
fn ramp(physics: &mut Physics3D, degrees: f32) -> CollisionMesh {
    let top = 20.0 * degrees.to_radians().tan();
    physics
        .trimesh(
            &[
                vec3(-20.0, 0.0, -10.0),
                vec3(0.0, 0.0, -10.0),
                vec3(-20.0, 0.0, 10.0),
                vec3(0.0, 0.0, 10.0),
                vec3(20.0, top, -10.0),
                vec3(20.0, top, 10.0),
            ],
            &[[0, 2, 1], [1, 2, 3], [1, 3, 4], [4, 3, 5]],
        )
        .unwrap()
}

/// A character at `start` that walks along +X at `speed` for `ticks`.
fn walk(start: Vec3, speed: f32, ticks: u64, level: impl FnOnce(&mut Ctx) + 'static) -> (Vec3, Character3D) {
    let mut player = None;
    let mut world = Game::new("t", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            ctx.world.query::<&mut Character3D>().for_each(|c| c.velocity.x = speed);
            // Facing where it walks: turning isn't moving.
            ctx.world
                .query::<&mut Transform3D>()
                .filter::<With<Character3D>>()
                .for_each(|t| t.rotation = Quat::from_rotation_y(0.1) * t.rotation);
        })
        .physics3d(GRAVITY)
        .run_headless(ticks, |ctx| {
            level(ctx);
            player = Some(ctx.world.spawn((Transform3D::at(start), Character3D::new(1.8, 0.3))));
        });
    let e = player.unwrap();
    (position(&mut world, e), *world.get::<Character3D>(e).unwrap())
}

#[test]
fn a_character_stands_on_the_ground() {
    let (p, c) = walk(vec3(0.0, 1.0, 0.0), 0.0, 100, |ctx| {
        ctx.world.spawn(floor());
    });
    assert!(c.grounded);
    assert!(p.y.abs() < 0.03, "feet on the floor: {p}");
    assert!(c.velocity.y.abs() < 1e-3, "not falling any more: {:?}", c.velocity);
}

#[test]
fn a_character_walks_up_a_30_degree_slope() {
    let (p, c) = walk(vec3(-2.0, 0.05, 0.0), 2.0, 200, |ctx| {
        let m = ramp(&mut ctx.physics3d, 30.0);
        ctx.world
            .spawn((Transform3D::default(), RigidBody3D::fixed(), Collider3D::mesh(m)));
    });
    assert!(p.x > 3.0, "walked up the slope: {p}");
    let on_slope = p.x * 30f32.to_radians().tan();
    assert!(
        (p.y - on_slope).abs() < 0.1,
        "feet on the slope: {p}, slope at {on_slope}"
    );
    assert!(c.grounded);
}

#[test]
fn a_character_cannot_walk_up_a_60_degree_slope() {
    let (p, _) = walk(vec3(-2.0, 0.05, 0.0), 2.0, 200, |ctx| {
        let m = ramp(&mut ctx.physics3d, 60.0);
        ctx.world
            .spawn((Transform3D::default(), RigidBody3D::fixed(), Collider3D::mesh(m)));
    });
    assert!(p.x < 0.5, "stopped at the foot of the slope: {p}");
    assert!(p.y < 0.6, "didn't climb: {p}");
}

#[test]
fn a_character_steps_up_a_30_cm_step() {
    let (p, c) = walk(vec3(-2.0, 0.05, 0.0), 2.0, 150, |ctx| {
        ctx.world.spawn(floor());
        // A 0.3 m step from x = 0 on.
        ctx.world.spawn((
            Transform3D::at(vec3(5.0, 0.15, 0.0)),
            RigidBody3D::fixed(),
            Collider3D::cuboid(vec3(10.0, 0.3, 10.0)),
        ));
    });
    assert!(p.x > 1.5, "went on: {p}");
    assert!((p.y - 0.3).abs() < 0.05, "stands on the step: {p}");
    assert!(c.grounded);
}

#[test]
fn a_character_does_not_climb_a_wall() {
    let (p, _) = walk(vec3(-2.0, 0.05, 0.0), 2.0, 150, |ctx| {
        ctx.world.spawn(floor());
        ctx.world.spawn((
            Transform3D::at(vec3(5.0, 0.5, 0.0)),
            RigidBody3D::fixed(),
            Collider3D::cuboid(vec3(10.0, 1.0, 10.0)),
        ));
    });
    assert!(p.x < 0.0 && p.y < 0.1, "blocked by a 1 m wall: {p}");
}

#[test]
fn a_character_stays_grounded_walking_downhill() {
    let airborne = Rc::new(Cell::new(0));
    let seen = airborne.clone();
    let mut player = None;
    let mut world = Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            // Down the slope: towards -X.
            ctx.world.query::<&mut Character3D>().for_each(|c| c.velocity.x = -4.0);
        })
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() > 10 {
                let c = *ctx.world.query::<&Character3D>().iter().next().unwrap();
                if !c.grounded {
                    seen.set(seen.get() + 1);
                }
            }
        })
        .run_headless(220, |ctx| {
            let m = ramp(&mut ctx.physics3d, 25.0);
            ctx.world
                .spawn((Transform3D::default(), RigidBody3D::fixed(), Collider3D::mesh(m)));
            let top = 15.0 * 25f32.to_radians().tan();
            player = Some(
                ctx.world
                    .spawn((Transform3D::at(vec3(15.0, top + 0.05, 0.0)), Character3D::new(1.8, 0.3))),
            );
        });
    let p = position(&mut world, player.unwrap());
    assert!(p.x < 0.0, "reached the bottom: {p}");
    assert_eq!(airborne.get(), 0, "never left the ground");
}

#[test]
fn a_character_jumps_and_lands() {
    let peak = Rc::new(Cell::new(0.0f32));
    let seen = peak.clone();
    let (p, c) = {
        let mut player = None;
        let mut world = Game::new("t", 1, 1)
            .sim_system(|ctx: &mut Ctx| {
                if ctx.time.sim_tick() == 30 {
                    ctx.world.query::<&mut Character3D>().for_each(|c| {
                        assert!(c.grounded);
                        c.velocity.y = 5.0;
                    });
                }
            })
            .physics3d(GRAVITY)
            .sim_system(move |ctx: &mut Ctx| {
                let y = ctx
                    .world
                    .query::<&Transform3D>()
                    .filter::<With<Character3D>>()
                    .iter()
                    .next()
                    .unwrap()
                    .position
                    .y;
                seen.set(seen.get().max(y));
            })
            .run_headless(150, |ctx| {
                ctx.world.spawn(floor());
                player = Some(
                    ctx.world
                        .spawn((Transform3D::at(vec3(0.0, 0.05, 0.0)), Character3D::new(1.8, 0.3))),
                );
            });
        let e = player.unwrap();
        (position(&mut world, e), *world.get::<Character3D>(e).unwrap())
    };
    // v² / 2g = 25 / 19.62 ≈ 1.27 m (a little less, stepped 50 times a second).
    assert!((peak.get() - 1.23).abs() < 0.06, "peak {}", peak.get());
    assert!(c.grounded && p.y.abs() < 0.03, "landed: {p}");
}

#[test]
fn a_character_pushes_dynamic_bodies() {
    let mut crate_box = None;
    let mut world = Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            ctx.world.query::<&mut Character3D>().for_each(|c| c.velocity.x = 2.0);
        })
        .physics3d(GRAVITY)
        .run_headless(150, |ctx| {
            ctx.world.spawn(floor());
            ctx.world
                .spawn((Transform3D::at(vec3(-1.0, 0.05, 0.0)), Character3D::new(1.8, 0.3)));
            // A 50 kg crate in the way.
            crate_box = Some(ctx.world.spawn((
                Transform3D::at(vec3(1.0, 0.25, 0.0)),
                RigidBody3D::dynamic(),
                Collider3D::cuboid(Vec3::splat(0.5)).density(400.0),
            )));
        });
    let x = position(&mut world, crate_box.unwrap()).x;
    assert!(x > 2.0, "pushed along: {x}");
}

#[test]
fn a_character_sets_off_triggers() {
    let entered = Rc::new(Cell::new(0));
    let seen = entered.clone();
    Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            ctx.world.query::<&mut Character3D>().for_each(|c| c.velocity.x = 3.0);
        })
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| {
            let n = ctx
                .events
                .read::<CollisionStarted>()
                .iter()
                .filter(|c| c.sensor)
                .count();
            seen.set(seen.get() + n);
        })
        .run_headless(100, |ctx| {
            ctx.world.spawn(floor());
            ctx.world
                .spawn((Transform3D::at(vec3(-2.0, 0.05, 0.0)), Character3D::new(1.8, 0.3)));
            ctx.world.spawn((
                Transform3D::at(vec3(3.0, 1.0, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::cuboid(vec3(1.0, 2.0, 4.0)).sensor(),
            ));
        });
    assert_eq!(entered.get(), 1);
}

#[test]
fn turning_a_character_is_not_a_teleport() {
    let (p, _) = walk(vec3(0.0, 0.05, 0.0), 1.0, 50, |ctx| {
        ctx.world.spawn(floor());
    });
    // It turned every tick and still walked: one second at 1 m/s.
    assert!((p.x - 1.0).abs() < 0.05, "{p}");
}

#[test]
fn scenes_make_mesh_colliders_from_mesh_files_and_save_them_by_file() {
    let file = TempAsset::new("test-mesh-collider.json");
    let saved = TempAsset::new("test-mesh-collider-saved.json");
    let collider = |convex: bool, x: f32| {
        serde_json::json!({
            "Transform3D": { "position": [x, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
            "RigidBody3D": { "kind": "Fixed" },
            "Collider3D": { "shape": { "Mesh": { "file": "models/house.glb", "scale": [1.0, 3.0, 1.0], "convex": convex } } },
        })
    };
    let scene = serde_json::json!({
        "format": "ponko-world",
        "version": 1,
        "entities": [
            { "entity": 0, "components": collider(false, 0.0) },
            { "entity": 1, "components": collider(true, 10.0) },
            { "entity": 2, "components": collider(false, 20.0) },
        ],
        "resources": {},
    });
    file.write(&serde_json::to_string(&scene).unwrap());
    let (file_name, saved_name) = (file.name().to_string(), saved.name().to_string());
    let (tops, meshes) = query_after(
        1,
        Vec3::ZERO,
        move |ctx| {
            ctx.load_scene(&file_name).unwrap();
            ctx.assets.save_scene(&ctx.world, &saved_name).unwrap();
        },
        |ctx| {
            let top = |x: f32| {
                ctx.physics3d
                    .cast_ray(vec3(x, 50.0, 0.0), -Vec3::Y, 100.0, QueryFilter3D::default())
                    .map(|hit| hit.point.y)
            };
            let tops = [top(0.0), top(10.0), top(20.0), top(30.0)];
            let meshes: std::collections::HashSet<_> = ctx
                .world
                .query::<&Collider3D>()
                .iter()
                .map(|c| format!("{:?}", c.shape))
                .collect();
            (tops, meshes)
        },
    );
    // The house's walls are 1.6 m and its roof 2.7 m high, made 3 times taller.
    for top in &tops[..3] {
        let y = top.expect("the ray hits the house");
        assert!((4.7..=8.2).contains(&y), "{tops:?}");
    }
    assert_eq!(tops[3], None, "nothing beside the houses");
    assert_eq!(
        meshes.len(),
        2,
        "the triangle mesh is made once and shared, the hull is another: {meshes:?}"
    );
    let json: serde_json::Value = serde_json::from_str(&saved.read()).unwrap();
    let shapes: Vec<_> = json["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["components"]["Collider3D"]["shape"].clone())
        .collect();
    for (shape, entity) in shapes.iter().zip(scene["entities"].as_array().unwrap()) {
        assert_eq!(shape, &entity["components"]["Collider3D"]["shape"], "saved by its file");
    }
}

#[test]
fn load_collision_mesh_makes_a_gltf_file_solid_in_one_call() {
    use ponko::d3::CollisionMeshOptions;
    let saved = TempAsset::new("test-load-collision-mesh-saved.json");
    let saved_name = saved.name().to_string();
    let (tops, handles, missing) = query_after(
        1,
        Vec3::ZERO,
        move |ctx| {
            let house = ctx.load_collision_mesh("models/house.glb").unwrap();
            let tall = CollisionMeshOptions {
                scale: vec3(1.0, 3.0, 1.0),
                ..Default::default()
            };
            let hull = CollisionMeshOptions { convex: true, ..tall };
            let tall = ctx.load_collision_mesh_with("models/house.glb", tall).unwrap();
            let hull = ctx.load_collision_mesh_with("models/house.glb", hull).unwrap();
            for (x, mesh) in [(0.0, house), (10.0, tall), (20.0, hull)] {
                ctx.world.spawn((
                    Transform3D::at(vec3(x, 0.0, 0.0)),
                    RigidBody3D::fixed(),
                    Collider3D::mesh(mesh),
                ));
            }
            ctx.assets.save_scene(&ctx.world, &saved_name).unwrap();
            // Each file and options once: loading again, or a scene naming the
            // same file, gives the same mesh.
            let again = ctx.load_collision_mesh("models/house.glb").unwrap();
            let defaults = CollisionMeshOptions::default();
            let same = ctx.load_collision_mesh_with("models/house.glb", defaults).unwrap();
            ctx.world.insert_resource(vec![house, again, same, tall, hull]);
            let missing = ctx.load_collision_mesh("models/no-such-file.glb").unwrap_err();
            ctx.world.insert_resource(missing.path().map(str::to_string));
        },
        |ctx| {
            let top = |x: f32| {
                ctx.physics3d
                    .cast_ray(vec3(x, 50.0, 0.0), -Vec3::Y, 100.0, QueryFilter3D::default())
                    .map(|hit| hit.point.y)
            };
            let tops = [top(0.0), top(10.0), top(20.0), top(30.0)];
            let handles = ctx.world.resource::<Vec<CollisionMesh>>().clone();
            let missing = ctx.world.resource::<Option<String>>().clone();
            (tops, handles, missing)
        },
    );
    // The house's walls are 1.6 m and its roof 2.7 m high.
    let house = tops[0].expect("the ray hits the house");
    assert!((1.5..=2.8).contains(&house), "{tops:?}");
    for top in &tops[1..3] {
        let y = top.expect("the ray hits the tall house");
        assert!((4.7..=8.2).contains(&y), "made 3 times taller: {tops:?}");
    }
    assert_eq!(tops[3], None, "nothing beside the houses");
    assert_eq!(handles[0], handles[1], "made once");
    assert_eq!(handles[0], handles[2], "the default options are the plain call");
    assert_ne!(handles[0], handles[3]);
    assert_ne!(handles[3], handles[4]);
    assert!(missing.unwrap().contains("no-such-file.glb"));

    // Saved by its file, as a scene's mesh collider is.
    let json: serde_json::Value = serde_json::from_str(&saved.read()).unwrap();
    let shapes: Vec<_> = json["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["components"]["Collider3D"]["shape"]["Mesh"].clone())
        .collect();
    assert_eq!(
        shapes,
        vec![
            serde_json::json!({ "file": "models/house.glb" }),
            serde_json::json!({ "file": "models/house.glb", "scale": [1.0, 3.0, 1.0] }),
            serde_json::json!({ "file": "models/house.glb", "scale": [1.0, 3.0, 1.0], "convex": true }),
        ]
    );
}

#[test]
fn a_scene_and_load_collision_mesh_share_one_mesh() {
    let file = TempAsset::new("test-shared-mesh-collider.json");
    let scene = serde_json::json!({
        "format": "ponko-world",
        "version": 1,
        "entities": [{ "entity": 0, "components": {
            "Transform3D": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
            "RigidBody3D": { "kind": "Fixed" },
            // The defaults written out: the same mesh as the plain call.
            "Collider3D": { "shape": { "Mesh": { "file": "models/house.glb", "scale": [1.0, 1.0, 1.0], "convex": false } } },
        } }],
        "resources": {},
    });
    file.write(&serde_json::to_string(&scene).unwrap());
    let name = file.name().to_string();
    let (from_scene, from_code) = query_after(
        0,
        Vec3::ZERO,
        move |ctx| {
            ctx.load_scene(&name).unwrap();
            let mesh = ctx.load_collision_mesh("models/house.glb").unwrap();
            ctx.world.insert_resource(mesh);
        },
        |ctx| {
            let shape = ctx.world.query::<&Collider3D>().iter().next().unwrap().shape;
            (shape, *ctx.world.resource::<CollisionMesh>())
        },
    );
    assert_eq!(
        format!("{from_scene:?}"),
        format!("{:?}", ponko::d3::Shape::Mesh(from_code))
    );
}

/// A crowd big enough to plan its moves in parallel lands in exactly the same
/// places on every run.
#[test]
fn a_crowd_of_characters_moves_the_same_way_every_run() {
    let run = || {
        query_after(
            60,
            GRAVITY,
            |ctx| {
                ctx.world.spawn(floor());
                for i in 0..40 {
                    let (x, z) = ((i % 8) as f32 * 0.7 - 2.5, (i / 8) as f32 * 0.7 - 1.5);
                    let mut c = Character3D::new(1.8, 0.3);
                    // Everyone walks toward the middle, so they bump into each other.
                    c.velocity = vec3(-x, 0.0, -z);
                    ctx.world.spawn((Transform3D::at(vec3(x, 0.05, z)), c));
                }
            },
            |ctx| {
                let mut out: Vec<[u32; 3]> = Vec::new();
                ctx.world.query::<(&Transform3D, &Character3D)>().for_each(|(t, _)| {
                    out.push(t.position.to_array().map(f32::to_bits));
                });
                out
            },
        )
    };
    let first = run();
    assert_eq!(first.len(), 40);
    assert_eq!(first, run());
    assert_eq!(first, run());
}
