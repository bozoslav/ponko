//! A level made in the Tiled map editor (`assets/levels/meadow.tmx`):
//! parallax clouds, animated water, coins placed as objects, and ground
//! that the player stands on through colliders made from its tiles. Edit
//! the map in Tiled while the game runs and it reloads in place.
//!
//!   `cargo run --release --example level --features physics`
//!
//! - Left and right arrows: walk. Space: jump. Collect the coins.
//! - Left mouse: build stone walls (they join up by themselves, an
//!   autotile rule); right mouse: knock them down.

use ponko::d2::*;
use ponko::prelude::*;
use ponko::tilemap::{AutoTile, LevelObject, TileMap};

/// The player.
#[derive(Component)]
struct Player;

/// Coins collected.
struct Score(i64);
/// The wall layer built in code, and the colliders made from it.
#[derive(Component)]
struct Walls;
#[derive(Component)]
struct WallBody;

// ANCHOR: spawn
fn setup(ctx: &mut Ctx) {
    // Every layer, tileset, object and collider of the map.
    ctx.spawn_level("levels/meadow.tmx", Vec2::ZERO)
        .expect("the level loads");

    // Objects keep their class, place and properties: start at "Spawn".
    let start = ctx
        .world
        .query::<&LevelObject>()
        .iter()
        .find(|o| o.class == "Spawn")
        .map_or(vec2(48.0, 120.0), |o| o.position);
    let crab = ctx.gfx.load_texture("crab.png");
    ctx.world.spawn((
        Transform {
            scale: Vec2::splat(0.25),
            ..Transform::at(start)
        },
        Sprite::new(crab),
        RigidBody {
            lock_rotation: true,
            ..RigidBody::dynamic()
        },
        Collider::rect(vec2(12.0, 12.0)).friction(0.0),
        Velocity::default(),
        Player,
    ));
    ctx.world.insert_resource(Score(0));
    ctx.camera.zoom = 2.0;
    ctx.camera.look_at(start);
}
// ANCHOR_END: spawn

// ANCHOR: walls
/// A wall layer built in code: 16 stone pieces picked by their four
/// neighbours. The meadow tileset has one stone tile, so every mask shows
/// it here; a real wall set has a piece per mask.
fn wall_rule() -> AutoTile {
    AutoTile::four([7; 16])
}

fn add_walls(ctx: &mut Ctx) {
    let tiles = ctx.world.query::<&TileMap>().iter().next().map(|m| m.sheet);
    if let Some(sheet) = tiles {
        let mut walls = TileMap::new(sheet, 40, 15);
        walls.z = -0.5;
        ctx.world.spawn((Transform::default(), walls, Walls));
    }
}

fn build(ctx: &mut Ctx) {
    let (left, right) = (
        ctx.input.mouse_down(MouseButton::Left),
        ctx.input.mouse_down(MouseButton::Right),
    );
    if !(left || right) {
        return;
    }
    let at = ctx.mouse_world();
    let rule = wall_rule();
    let mut bodies = Vec::new();
    ctx.world
        .query::<(&Transform, &mut TileMap, &Walls)>()
        .for_each(|(t, map, _)| {
            if let Some(cell) = map.tile_at(t.position, at) {
                map.paint_autotile(&rule, cell.x, cell.y, left);
                // New colliders for the walls, merged into few rectangles.
                bodies = map.colliders(t.position, |tile| rule.owns(tile));
            }
        });
    let old: Vec<Entity> = ctx
        .world
        .query::<(Entity, &WallBody)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in old {
        ctx.world.despawn(e);
    }
    ctx.world.spawn_batch(
        bodies
            .into_iter()
            .map(|(t, body, collider)| (t, body, collider, WallBody)),
    );
}
// ANCHOR_END: walls

fn walk(ctx: &mut Ctx) {
    let dir = f32::from(u8::from(ctx.input.key_down(Key::Right))) - f32::from(u8::from(ctx.input.key_down(Key::Left)));
    let jump = ctx.input.key_pressed(Key::Space);
    let mut at = None;
    ctx.world
        .query::<(&Transform, &mut Velocity, &Player)>()
        .for_each(|(t, v, _)| {
            v.linear.x = dir * 90.0;
            if jump && v.linear.y.abs() < 1.0 {
                v.linear.y = -260.0;
            }
            at = Some(t.position);
        });
    if let Some(at) = at {
        ctx.camera.look_at(at);
    }
}

fn collect(ctx: &mut Ctx) {
    let Some(player) = ctx
        .world
        .query::<(&Transform, &Player)>()
        .iter()
        .next()
        .map(|(t, _)| t.position)
    else {
        return;
    };
    let taken: Vec<(Entity, i64)> = ctx
        .world
        .query::<(Entity, &Transform, &LevelObject)>()
        .iter()
        .filter(|(_, t, o)| o.class == "Coin" && t.position.distance(player) < 12.0)
        .map(|(e, _, o)| (e, o.properties.i64("value").unwrap_or(1)))
        .collect();
    for (coin, value) in taken {
        ctx.world.despawn(coin);
        ctx.world.resource_mut::<Score>().0 += value;
    }
}

fn hud(ctx: &mut Ctx) {
    let text = format!(
        "coins: {}\narrows: walk, space: jump\nleft/right mouse: build/knock down walls",
        ctx.world.resource::<Score>().0
    );
    let style = &ctx.ui.immediate.style;
    let size = ctx.gfx.measure_text(&text, style.text_size) + 2.0 * style.padding;
    let panel = Rect::new(12.0, 12.0, size.x, size.y);
    let padding = ctx.ui.immediate.style.padding;
    ctx.immediate_ui().panel(panel);
    ctx.immediate_ui().label(panel.min + padding, &text);
}

fn main() {
    Game::new("Ponko: a Tiled level", 1280, 720)
        .physics(vec2(0.0, 700.0))
        .hot_reload(true)
        .system(walk)
        .system(build)
        .system(collect)
        .system(hud)
        .setup(|ctx| {
            setup(ctx);
            add_walls(ctx);
        })
        .run();
}
