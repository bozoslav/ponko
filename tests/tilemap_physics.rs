//! Colliders from tiles (feature `physics`): bodies land on a level's solid
//! tiles, on merged rectangles and on per-tile collision shapes.
#![cfg(feature = "physics")]

use ponko::d2::{Collider, RigidBody, Transform as Transform2D};
use ponko::prelude::*;
use ponko::tilemap::{LevelOptions, TileMap};

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/tilemaps/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Drops balls of radius 4 at each `x` from `y = 0` onto the level and
/// returns where they are after two seconds.
fn drop_on(name: &str, options: LevelOptions, xs: &[f32]) -> Vec<Vec2> {
    let path = fixture(name);
    let xs = xs.to_vec();
    let mut balls = Vec::new();
    let world = Game::new("drop", 1, 1)
        .physics(vec2(0.0, 980.0))
        .run_headless(120, |ctx| {
            ctx.spawn_level_with(&path, Vec2::ZERO, options).unwrap();
            for x in xs {
                balls.push(ctx.world.spawn((
                    Transform2D::at(vec2(x, 0.0)),
                    RigidBody::dynamic(),
                    Collider::circle(4.0),
                )));
            }
        });
    balls
        .iter()
        .map(|&e| world.get::<Transform2D>(e).unwrap().position)
        .collect()
}

#[test]
fn bodies_land_on_solid_tiles_and_on_partial_tile_shapes() {
    // x 40: the wall row (its top at y 80); x 72: the half block (top at 72).
    let at = drop_on("level.tmx", LevelOptions::default(), &[40.0, 72.0]);
    assert!((at[0].y - 76.0).abs() < 1.5, "on the wall row: {}", at[0]);
    assert!((at[1].y - 68.0).abs() < 1.5, "on the half block: {}", at[1]);
}

#[test]
fn a_level_spawned_without_colliders_lets_bodies_fall_through() {
    let at = drop_on("level.tmx", LevelOptions::default().without_colliders(), &[40.0]);
    assert!(at[0].y > 200.0, "{}", at[0]);
}

#[test]
fn ldtk_int_grid_walls_hold_bodies_and_spawn_few_colliders() {
    let at = drop_on("world.ldtk", LevelOptions::default().solid("wall"), &[40.0]);
    assert!((at[0].y - 76.0).abs() < 1.5, "{}", at[0]);
    let path = fixture("world.ldtk");
    let mut world = Game::new("count", 1, 1)
        .physics(Vec2::ZERO)
        .run_headless(0, move |ctx| {
            ctx.spawn_level_with(&path, Vec2::ZERO, LevelOptions::default().solid("wall"))
                .unwrap();
        });
    assert_eq!(
        world.query::<&RigidBody>().iter().count(),
        1,
        "eight wall cells, one rectangle"
    );
}

#[test]
fn tile_map_colliders_cover_the_solid_tiles() {
    Game::new("map", 1, 1).run_headless(0, |ctx| {
        let tex = ctx.gfx.create_texture(32, 16, &[255; 32 * 16 * 4]);
        let mut map = TileMap::new(SpriteSheet::new(tex, 16, 16), 6, 2);
        for x in 0..6 {
            map.set(x, 1, Some(1));
        }
        let bodies = map.colliders(vec2(10.0, 0.0), |t| t == 1);
        assert_eq!(bodies.len(), 1);
        let (t, body, collider) = bodies[0];
        assert_eq!(t.position, vec2(10.0 + 48.0, 24.0));
        assert_eq!(body.kind, BodyKind::Fixed);
        assert_eq!(collider.shape, ponko::d2::Shape::Rect { size: vec2(96.0, 16.0) });
    });
}

#[test]
fn the_example_level_holds_its_player_up() {
    let mut player = None;
    let world = Game::new("meadow", 1, 1)
        .physics(vec2(0.0, 700.0))
        .run_headless(120, |ctx| {
            ctx.spawn_level("levels/meadow.tmx", Vec2::ZERO).unwrap();
            player = Some(ctx.world.spawn((
                Transform2D::at(vec2(48.0, 120.0)),
                RigidBody::dynamic(),
                Collider::circle(4.0),
            )));
        });
    let y = world.get::<Transform2D>(player.unwrap()).unwrap().position.y;
    assert!((y - 172.0).abs() < 1.5, "on the grass (its top at 176): {y}");
}
