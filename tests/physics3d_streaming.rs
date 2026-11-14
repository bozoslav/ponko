//! 3D physics for big worlds: terrain colliders that match their tiles,
//! collision meshes built on worker threads, and static colliders kept only
//! near focus points.
#![cfg(feature = "physics3d")]

use std::cell::RefCell;
use std::rc::Rc;

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform;
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::{Character, Collider, QueryFilter, RigidBody};
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::{ColliderFocus, CollisionMeshData, TerrainCollider};
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::Material;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::terrain::{Heightfield, TerrainTileData};

const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

fn position(world: &mut World, e: Entity) -> Vec3 {
    world.get::<Transform>(e).unwrap().position
}

#[test]
fn a_ball_rests_on_a_turned_and_scaled_terrain_tile_at_the_drawn_height_and_falls_through_its_hole() {
    // A slope rising 0.15 per meter along the tile's X, stretched twice as
    // high and turned 30 degrees: 0.3 per meter along the turned X.
    let n = 9;
    let size = 16.0;
    let mut heights: Vec<f32> = (0..n * n).map(|i| (i % n) as f32 * 2.0 * 0.15).collect();
    // A hole at grid point (6, 6): the cells from 10 m to 14 m are open.
    heights[6 * n + 6] = f32::NAN;
    let corner = vec3(40.0, -3.0, 20.0);
    let tile_transform = Transform {
        position: corner,
        rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_6),
        scale: vec3(1.0, 2.0, 1.0),
    };
    let ground = Heightfield::new(&heights, size);
    // Where a point of the tile (from its corner) is in the world.
    let world_at = move |x: f32, z: f32| tile_transform.position + tile_transform.rotation * vec3(x, 0.0, z);
    let radius = 0.3;
    let (mut ball, mut over_hole) = (None, None);
    let mut world = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(150, |ctx| {
        let material = ctx.gfx.default_material();
        let data = TerrainTileData::new(&heights, size);
        let tile = ctx.gfx.upload_terrain_tile(&data, material);
        let mesh = ctx.physics3d.heightfield(&heights, size);
        ctx.world
            .spawn((tile_transform, tile, TerrainCollider::new(mesh).friction(1.0)));
        // A ball that can't roll and grips: it stays where it lands.
        let start = world_at(4.0, 4.0) + vec3(0.0, 8.0, 0.0);
        let grippy = RigidBody {
            lock_rotation: true,
            ..RigidBody::dynamic()
        };
        ball = Some(
            ctx.world
                .spawn((Transform::at(start), grippy, Collider::sphere(radius).friction(1.0))),
        );
        over_hole = Some(ctx.world.spawn((
            Transform::at(world_at(12.0, 12.0) + vec3(0.0, 8.0, 0.0)),
            RigidBody::dynamic(),
            Collider::sphere(radius),
        )));
    });
    // The drawn ground under the ball's center, from the tile's heights.
    let p = position(&mut world, ball.unwrap());
    let local = tile_transform.rotation.inverse() * (p - corner);
    let drawn = corner.y + 2.0 * ground.height_at(local.x, local.z).unwrap();
    // On a plane of slope 0.3, the center sits r / cos(slope) above it.
    let want = drawn + radius * (1.0f32 + 0.3 * 0.3).sqrt();
    assert!(
        (p.y - want).abs() < 0.02,
        "rests at {}, the drawn ground puts it at {want}",
        p.y
    );
    let y = position(&mut world, over_hole.unwrap()).y;
    assert!(y < corner.y - 5.0, "fell through the hole: {y}");
}

#[test]
fn collision_meshes_build_on_a_worker_thread() {
    let ray_hits = Rc::new(RefCell::new(None));
    let out = ray_hits.clone();
    Game::new("t", 1, 1)
        .physics3d(Vec3::ZERO)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 1 {
                let hit = ctx
                    .physics3d
                    .cast_ray(vec3(5.0, 10.0, 5.0), Vec3::NEG_Y, 20.0, QueryFilter::default());
                *out.borrow_mut() = hit.map(|h| h.point.y);
            }
        })
        .run_headless(2, |ctx| {
            let job = ponko::streaming::Job::spawn(|| CollisionMeshData::heightfield(&[2.0; 5 * 5], 10.0));
            let mesh = ctx.physics3d.add_mesh(&job.wait());
            ctx.world.spawn((Transform::default(), TerrainCollider::new(mesh)));
        });
    let y = ray_hits.take().expect("the ray hit the ground");
    assert!((y - 2.0).abs() < 1e-4, "{y}");
}

#[test]
fn editing_a_tile_swaps_its_collider_and_despawning_removes_it() {
    let hits = Rc::new(RefCell::new(Vec::new()));
    let out = hits.clone();
    let tile = Rc::new(RefCell::new(None));
    let tile_in = tile.clone();
    Game::new("t", 1, 1)
        .physics3d(Vec3::ZERO)
        .sim_system(move |ctx: &mut Ctx| {
            let ray = ctx
                .physics3d
                .cast_ray(vec3(3.0, 50.0, 3.0), Vec3::NEG_Y, 100.0, QueryFilter::default());
            out.borrow_mut().push(ray.map(|h| h.point.y));
            let e = tile_in.borrow().unwrap();
            match ctx.time.sim_tick() {
                0 => {
                    // Dig the ground down to 1 m: a new mesh, the old one freed.
                    let lower = ctx.physics3d.heightfield(&[1.0; 4 * 4], 6.0);
                    let terrain = ctx.world.get_mut::<TerrainCollider>(e).unwrap();
                    let old = std::mem::replace(&mut terrain.mesh, lower);
                    ctx.free(old);
                }
                1 => {
                    ctx.world.despawn(e);
                }
                _ => {}
            }
        })
        .run_headless(4, |ctx| {
            let mesh = ctx.physics3d.heightfield(&[4.0; 4 * 4], 6.0);
            *tile.borrow_mut() = Some(ctx.world.spawn((Transform::default(), TerrainCollider::new(mesh))));
        });
    let hits = hits.take();
    let near = |got: Option<f32>, want: f32| got.is_some_and(|y| (y - want).abs() < 1e-4);
    assert!(near(hits[0], 4.0), "{hits:?}");
    assert!(near(hits[1], 1.0), "edited: {hits:?}");
    assert_eq!(hits[2], None, "despawned: {hits:?}");
}

/// Fixed 1 m boxes along +X every 20 m, from x = 0, and a focus that walks
/// along X: reports, after each step, which boxes are in.
fn boxes_in_as_the_focus_moves(path: &'static [f32], max_new: usize) -> Vec<Vec<bool>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let out = seen.clone();
    let boxes = Rc::new(RefCell::new(Vec::new()));
    let boxes_in = boxes.clone();
    Game::new("t", 1, 1)
        .physics3d(Vec3::ZERO)
        .sim_system(move |ctx: &mut Ctx| {
            let tick = ctx.time.sim_tick() as usize;
            let row: Vec<bool> = boxes_in
                .borrow()
                .iter()
                .map(|&b| ctx.physics3d.is_collider_active(b))
                .collect();
            out.borrow_mut().push(row);
            if let Some(&x) = path.get(tick + 1) {
                ctx.world
                    .query::<&mut Transform>()
                    .filter::<With<ColliderFocus>>()
                    .for_each(|t| t.position.x = x);
            }
        })
        .run_headless(path.len() as u64, |ctx| {
            ctx.physics3d.stream_colliders(Some(max_new));
            ctx.world
                .spawn((Transform::at(vec3(path[0], 0.0, 0.0)), ColliderFocus::new(15.0, 25.0)));
            for i in 0..6 {
                let b = ctx.world.spawn((
                    Transform::at(vec3(i as f32 * 20.0, 0.0, 0.0)),
                    RigidBody::fixed(),
                    Collider::cuboid(Vec3::ONE),
                ));
                boxes.borrow_mut().push(b);
            }
        });
    seen.take()
}

#[test]
fn static_colliders_come_and_go_with_the_focus_and_the_gap_between_radii_keeps_them() {
    let rows = boxes_in_as_the_focus_moves(&[0.0, 20.0, 40.0, 40.0, 100.0, 100.0], 100);
    // Within 15 m in, beyond 25 m out; in between as they were.
    assert_eq!(rows[0], [true, false, false, false, false, false], "at 0");
    assert_eq!(
        rows[1],
        [true, true, false, false, false, false],
        "at 20: box 0 is 19.5 away, kept"
    );
    assert_eq!(rows[2], [false, true, true, false, false, false], "at 40");
    assert_eq!(rows[4], [false, false, false, false, false, true], "at 100");
}

#[test]
fn colliders_come_in_nearest_first_within_the_budget() {
    let rows = boxes_in_as_the_focus_moves(&[50.0, 50.0, 50.0, 50.0], 1);
    // Boxes 2 (at 40) and 3 (at 60) are both 9.5 away, 1 and 4 are 29.5.
    assert_eq!(rows[0].iter().filter(|&&b| b).count(), 1, "one per step: {rows:?}");
    assert!(rows[0][2], "the nearest first, ties in a fixed order: {rows:?}");
    assert_eq!(rows[1], [false, false, true, true, false, false]);
    assert_eq!(rows[3], rows[1], "nothing else is in reach");
}

#[test]
fn ground_stays_under_a_body_left_behind_and_queries_only_see_colliders_that_are_in() {
    let report = Rc::new(RefCell::new(None));
    let out = report.clone();
    let ids = Rc::new(RefCell::new(None));
    let ids_in = ids.clone();
    Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| {
            let (focus, under_crate, empty, crate_body) = ids_in.borrow().unwrap();
            match ctx.time.sim_tick() {
                // Settled: walk away, 1 km.
                60 => ctx.world.get_mut::<Transform>(focus).unwrap().position.x = 1000.0,
                150 => {
                    let ray = |ctx: &Ctx, x: f32| {
                        ctx.physics3d
                            .cast_ray(vec3(x, 20.0, 5.0), Vec3::NEG_Y, 40.0, QueryFilter::default())
                            .map(|h| h.entity)
                    };
                    *out.borrow_mut() = Some((
                        ctx.physics3d.is_collider_active(under_crate),
                        ctx.physics3d.is_collider_active(empty),
                        ctx.world.get::<Transform>(crate_body).unwrap().position.y,
                        ray(ctx, 2.0),
                        ray(ctx, 25.0),
                    ));
                }
                _ => {}
            }
        })
        .run_headless(151, |ctx| {
            ctx.physics3d.stream_colliders(Some(8));
            let focus = ctx
                .world
                .spawn((Transform::at(vec3(10.0, 0.0, 5.0)), ColliderFocus::new(50.0, 60.0)));
            let flat = ctx.physics3d.heightfield(&[0.0; 3 * 3], 10.0);
            let under_crate = ctx.world.spawn((Transform::default(), TerrainCollider::new(flat)));
            let empty = ctx
                .world
                .spawn((Transform::at(vec3(20.0, 0.0, 0.0)), TerrainCollider::new(flat)));
            let crate_body = ctx.world.spawn((
                Transform::at(vec3(5.0, 2.0, 5.0)),
                RigidBody::dynamic(),
                Collider::cuboid(Vec3::ONE),
            ));
            *ids.borrow_mut() = Some((focus, under_crate, empty, crate_body));
        });
    let (crate_ground, empty_ground, crate_y, ray_crate, ray_empty) = report.take().unwrap();
    assert!(crate_ground, "the crate's ground stays");
    assert!(!empty_ground, "the empty tile went");
    assert!((crate_y - 0.5).abs() < 0.05, "the crate still stands: {crate_y}");
    assert!(ray_crate.is_some(), "the crate (or its ground) is hit");
    assert_eq!(ray_empty, None, "a ray passes where the ground went");
}

#[test]
fn turning_streaming_off_brings_every_collider_back() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let out = seen.clone();
    let far = Rc::new(RefCell::new(None));
    let far_in = far.clone();
    Game::new("t", 1, 1)
        .physics3d(Vec3::ZERO)
        .sim_system(move |ctx: &mut Ctx| {
            let e = far_in.borrow().unwrap();
            out.borrow_mut().push(ctx.physics3d.is_collider_active(e));
            if ctx.time.sim_tick() == 1 {
                ctx.physics3d.stream_colliders(None);
            }
        })
        .run_headless(4, |ctx| {
            ctx.physics3d.stream_colliders(Some(4));
            *far.borrow_mut() = Some(ctx.world.spawn((
                Transform::at(vec3(500.0, 0.0, 0.0)),
                RigidBody::fixed(),
                Collider::sphere(1.0),
            )));
        });
    assert_eq!(seen.take(), [false, false, true, true]);
}

// ANCHOR: collider_streaming
/// A streamed tile, built on a worker: its meshes and its collider, from
/// the same heights.
fn build_tile(heights: Vec<f32>) -> ponko::streaming::Job<(TerrainTileData, CollisionMeshData)> {
    ponko::streaming::Job::spawn(move || {
        let drawn = TerrainTileData::new(&heights, 128.0);
        let solid = CollisionMeshData::heightfield(&heights, 128.0);
        (drawn, solid)
    })
}

/// Once built, on the game's thread: drawn and solid. Free both when the
/// tile goes (`ctx.free(tile)` and `ctx.free(mesh)`).
fn place_tile(ctx: &mut Ctx, corner: Vec3, built: (TerrainTileData, CollisionMeshData), ground: Material) -> Entity {
    let (drawn, solid) = built;
    let tile = ctx.gfx.upload_terrain_tile(&drawn, ground);
    let mesh = ctx.physics3d.add_mesh(&solid);
    ctx.world
        .spawn((Transform::at(corner), tile, TerrainCollider::new(mesh)))
}

/// Drawn out to the streamer's 600 m, but solid only near the player and
/// the car (further ahead of the car: it's faster), 4 new colliders a step.
fn solid_near(ctx: &mut Ctx, player: Entity, car: Entity) {
    ctx.physics3d.stream_colliders(Some(4));
    ctx.world.insert(player, ColliderFocus::new(150.0, 200.0));
    ctx.world.insert(car, ColliderFocus::new(300.0, 350.0));
}
// ANCHOR_END: collider_streaming

#[test]
fn the_guide_tiles_are_solid_only_near_the_player_and_the_car() {
    let seen = Rc::new(RefCell::new(None));
    let out = seen.clone();
    let tiles = Rc::new(RefCell::new(Vec::new()));
    let tiles_in = tiles.clone();
    Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 5 {
                let active: Vec<bool> = tiles_in
                    .borrow()
                    .iter()
                    .map(|&t| ctx.physics3d.is_collider_active(t))
                    .collect();
                *out.borrow_mut() = Some(active);
            }
        })
        .run_headless(6, |ctx| {
            let ground = ctx.gfx.default_material();
            // Tiles every 128 m along X, out to 1 km.
            for k in 0..8 {
                let built = build_tile(vec![0.0; 33 * 33]).wait();
                let tile = place_tile(ctx, vec3(k as f32 * 128.0, 0.0, 0.0), built, ground);
                tiles.borrow_mut().push(tile);
            }
            let player = ctx
                .world
                .spawn((Transform::at(vec3(10.0, 0.0, 10.0)), Character::new(1.8, 0.3)));
            let car = ctx.world.spawn((
                Transform::at(vec3(900.0, 1.0, 10.0)),
                RigidBody::dynamic(),
                Collider::cuboid(vec3(2.0, 1.0, 4.0)),
            ));
            solid_near(ctx, player, car);
        });
    let active = seen.take().unwrap();
    // The player's 150 m reaches tiles 0 and 1; the car's 300 m tiles 4 to 7.
    assert_eq!(active, [true, true, false, false, true, true, true, true]);
}
