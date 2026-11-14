//! 2D lights: a dark tile dungeon lit by wandering coloured lights with
//! shadows, a lantern (a spot light) that follows the mouse, normal-mapped
//! floor, walls and crates, and a dim blue ambient light.
//!
//!   `cargo run --example lights2d`
//!
//! - Move the mouse: the lantern follows it and points away from the
//!   middle of the room.
//! - L: a hundred more lights (shadowless), for a look at the cost.
//! - N: normal maps on and off.
//!
//! The textures and their normal maps are generated in code; only the crab
//! is a file (`assets/crab.png`). The walls cast shadows through one occluder made
//! from the tile map (`TileMap::occluder`); each crate has its own. In a
//! browser it runs on WebGL2 as on WebGPU; it also publishes its state for
//! the browser smoke test (`web/smoke/`, fixture `lights2d`).

use ponko::d2::*;
use ponko::prelude::*;
use ponko::tilemap::TileMap;
use serde_json::json;

#[path = "common/web_smoke.rs"]
mod smoke;

const TILE: u32 = 16;
const FLOOR: u16 = 0;
const WALL: u16 = 1;
const MAP: (u32, u32) = (50, 38);

/// What setup made.
struct Dungeon {
    smoke: smoke::Smoke,
    /// Each colour texture and its normal map, for the N key.
    normal_maps: Vec<(Texture, Texture)>,
    normals_on: bool,
    lantern: Entity,
    many: Vec<Entity>,
    crab: Texture,
}

/// Fixed-step ticks so far, for the smoke test's progress check.
#[derive(Default)]
struct Ticks(u64);

/// A wandering light: where it circles and how fast.
#[derive(Component)]
struct Wander {
    center: Vec2,
    radius: f32,
    speed: f32,
    phase: f32,
}

fn main() {
    Game::new("Ponko: 2D lights", 800, 600)
        .fixed_hz(60)
        .sim_system(|ctx| ctx.world.resource_mut::<Ticks>().0 += 1)
        .setup(setup)
        .system(wander)
        .system(controls)
        .run();
}

fn setup(ctx: &mut Ctx) {
    let smoke = smoke::Smoke::new("lights2d");
    ctx.world.insert_resource(Ticks::default());
    ctx.gfx.clear(Color::BLACK);
    ctx.gfx.set_ambient_light2d(Some(Color::rgb(0.10, 0.11, 0.18)));

    // The tile map: a floor, walls round the edge and a few inside.
    let (tiles, tile_normals) = tileset(ctx);
    let mut map = TileMap::new(SpriteSheet::new(tiles, TILE, TILE), MAP.0, MAP.1);
    for y in 0..MAP.1 {
        for x in 0..MAP.0 {
            let edge = x == 0 || y == 0 || x == MAP.0 - 1 || y == MAP.1 - 1;
            let pillar = (x % 12 == 6) && (y % 10 == 5);
            let wall = (x == 25 && (8..30).contains(&y) && !(17..21).contains(&y)) || (y == 12 && (8..18).contains(&x));
            map.set(x, y, Some(if edge || pillar || wall { WALL } else { FLOOR }));
        }
    }
    let shadows = map.occluder(|tile| tile == WALL);
    ctx.world.spawn((Transform::at(Vec2::ZERO), map, shadows));

    // Crates: sprites that cast shadows of their own.
    let (crate_texture, crate_normals) = crate_box(ctx);
    let mut rng = Rng::new(7);
    for _ in 0..10 {
        let at = vec2(rng.range(60.0, 740.0), rng.range(60.0, 540.0));
        let mut t = Transform::at(at);
        t.rotation = rng.range(0.0, 1.5);
        let sprite = Sprite::new(crate_texture).with_z(1.0);
        ctx.world.spawn((t, sprite, Occluder::rect(vec2(20.0, 20.0))));
    }

    // A crab in the middle of the room, lit flat (it has no normal map).
    let crab = ctx.gfx.load_texture("crab.png");
    ctx.world
        .spawn((Transform::at(vec2(400.0, 300.0)), Sprite::new(crab).with_z(1.0)));

    // Wandering lights with shadows.
    let colors = [
        Color::rgb(1.0, 0.55, 0.25),
        Color::rgb(0.35, 0.6, 1.0),
        Color::rgb(0.5, 1.0, 0.45),
        Color::rgb(1.0, 0.35, 0.7),
        Color::rgb(1.0, 0.9, 0.5),
        Color::rgb(0.6, 0.4, 1.0),
    ];
    for (i, &color) in colors.iter().enumerate() {
        let center = vec2(130.0 + (i % 3) as f32 * 270.0, 160.0 + (i / 3) as f32 * 280.0);
        let light = PointLight {
            color,
            radius: 230.0,
            intensity: 1.3,
            shadows: true,
            ..Default::default()
        };
        let wander = Wander {
            center,
            radius: 60.0 + i as f32 * 8.0,
            speed: 0.5 + i as f32 * 0.13,
            phase: i as f32,
        };
        ctx.world.spawn((Transform::at(center), light, wander));
    }

    // The lantern: a spot light at the mouse.
    let lantern = ctx.world.spawn((
        Transform::at(vec2(400.0, 300.0)),
        SpotLight {
            color: Color::rgb(1.0, 0.95, 0.8),
            radius: 360.0,
            angle: 0.45,
            intensity: 1.6,
            shadows: true,
            ..Default::default()
        },
    ));

    let normal_maps = vec![(tiles, tile_normals), (crate_texture, crate_normals)];
    for &(texture, normals) in &normal_maps {
        ctx.gfx.set_normal_map(texture, Some(normals));
    }
    ctx.world.insert_resource(Dungeon {
        smoke,
        normal_maps,
        normals_on: true,
        lantern,
        many: Vec::new(),
        crab,
    });
}

fn wander(ctx: &mut Ctx) {
    let t = ctx.time.elapsed() as f32;
    ctx.world.query::<(&mut Transform, &Wander)>().for_each(|(at, w)| {
        let angle = t * w.speed + w.phase;
        at.position = w.center + vec2(angle.cos(), (angle * 1.3).sin()) * w.radius;
    });
}

fn controls(ctx: &mut Ctx) {
    let mut dungeon = ctx.world.remove_resource::<Dungeon>().expect("made in setup");
    let mouse = ctx.input.mouse_pos();
    let mouse = ctx.camera.screen_to_world(mouse);
    if let Some(at) = ctx.world.get_mut::<Transform>(dungeon.lantern) {
        at.position = mouse;
        let away = mouse - vec2(400.0, 300.0);
        at.rotation = if away.length() > 1.0 { away.to_angle() } else { 0.0 };
    }
    if ctx.input.key_pressed(Key::N) {
        dungeon.normals_on = !dungeon.normals_on;
        for &(texture, normals) in &dungeon.normal_maps {
            ctx.gfx.set_normal_map(texture, dungeon.normals_on.then_some(normals));
        }
    }
    if ctx.input.key_pressed(Key::L) {
        if dungeon.many.is_empty() {
            let mut rng = Rng::new(3);
            for _ in 0..100 {
                let light = PointLight {
                    color: Color::rgb(rng.range(0.3, 1.0), rng.range(0.3, 1.0), rng.range(0.3, 1.0)),
                    radius: 70.0,
                    intensity: 0.8,
                    ..Default::default()
                };
                let center = vec2(rng.range(20.0, 780.0), rng.range(20.0, 580.0));
                let wander = Wander {
                    center,
                    radius: rng.range(10.0, 40.0),
                    speed: rng.range(0.5, 2.0),
                    phase: rng.range(0.0, 6.0),
                };
                dungeon
                    .many
                    .push(ctx.world.spawn((Transform::at(center), light, wander)));
            }
        } else {
            for e in dungeon.many.drain(..) {
                ctx.world.despawn(e);
            }
        }
    }
    let lights = ctx.world.query::<&PointLight>().count() + ctx.world.query::<&SpotLight>().count();
    let ticks = ctx.world.resource::<Ticks>().0;
    dungeon.smoke.frame(
        ctx,
        ticks,
        json!({
            "lights": lights,
            "normal_maps": dungeon.normals_on,
            "asset": [dungeon.crab.width(), dungeon.crab.height()],
        }),
    );
    ctx.world.insert_resource(dungeon);
}

/// Pixels of a `w` x `h` image, and its normal map: a bevel `bevel` pixels
/// wide around each `cell` x `cell` square, sloping outward.
fn bevelled(w: u32, h: u32, cell: u32, bevel: u32, color: impl Fn(u32, u32) -> [u8; 3]) -> (Vec<u8>, Vec<u8>) {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let mut normals = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (cx, cy) = (x % cell, y % cell);
            // Tangent space, green up the image: the left bevel faces left,
            // the top one up.
            let mut n = Vec3::Z;
            if cx < bevel {
                n.x -= 1.0;
            } else if cx >= cell - bevel {
                n.x += 1.0;
            }
            if cy < bevel {
                n.y += 1.0;
            } else if cy >= cell - bevel {
                n.y -= 1.0;
            }
            let n = n.normalize();
            let encoded = n * 0.5 + 0.5;
            normals.extend([encoded.x, encoded.y, encoded.z].map(|c| (c * 255.0) as u8));
            normals.push(255);
            let c = color(x, y);
            // Painted light from the top left, for when normal maps are off.
            let shade = 1.0 + 0.12 * (n.y - n.x);
            rgba.extend(c.map(|v| (f32::from(v) * shade).min(255.0) as u8));
            rgba.push(255);
        }
    }
    (rgba, normals)
}

/// Floor tiles (stone slabs) and wall tiles (bricks), with normal maps.
fn tileset(ctx: &mut Ctx) -> (Texture, Texture) {
    let mut rng = Rng::new(11);
    let noise: Vec<f32> = (0..TILE * 2 * TILE).map(|_| 0.9 + rng.f32() * 0.2).collect();
    let (rgba, normals) = bevelled(TILE * 2, TILE, TILE, 2, |x, y| {
        let n = noise[(y * TILE * 2 + x) as usize];
        let base = if x < TILE {
            [120.0, 116.0, 110.0]
        } else {
            [150.0, 84.0, 60.0]
        };
        base.map(|c| (c * n) as u8)
    });
    let tiles = ctx.gfx.create_texture(TILE * 2, TILE, &rgba);
    let normals = ctx.gfx.create_texture(TILE * 2, TILE, &normals);
    (tiles, normals)
}

/// A wooden crate and its normal map.
fn crate_box(ctx: &mut Ctx) -> (Texture, Texture) {
    let (rgba, normals) = bevelled(
        20,
        20,
        20,
        3,
        |_, y| {
            if y % 5 == 0 { [110, 70, 36] } else { [160, 108, 58] }
        },
    );
    (
        ctx.gfx.create_texture(20, 20, &rgba),
        ctx.gfx.create_texture(20, 20, &normals),
    )
}
