//! A big tilemap with animated sprites on it.
//!
//!   `cargo run --release --example tiles`
//!   `MAP=4000 cargo run --release --example tiles` (16 million tiles)
//!
//! - Right or middle mouse drag: pan. Scroll: zoom.
//! - Left mouse: paint stone tiles.
//!
//! The tileset and the creature's animation frames are generated in code,
//! so the example needs no image files. Only tiles the camera can see are
//! drawn, so the map size barely affects the frame time.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::prelude::*;
use ponko::sheet::Animation;

const TILE: u32 = 16;
/// Tile frames in the generated tileset.
const WATER: u16 = 0;
const SAND: u16 = 1;
const GRASS: u16 = 2;
const FOREST: u16 = 3;
const STONE: u16 = 4;

/// A 5-tile strip: water, sand, grass, forest, stone, with some texture.
fn tileset(ctx: &mut Ctx) -> SpriteSheet {
    let colors: [[u8; 3]; 5] = [
        [38, 84, 160],
        [222, 196, 132],
        [86, 160, 70],
        [40, 102, 48],
        [128, 128, 136],
    ];
    let (w, h) = (TILE * colors.len() as u32, TILE);
    let mut rng = Rng::new(1);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _y in 0..h {
        for x in 0..w {
            let c = colors[(x / TILE) as usize];
            let shade = 0.88 + rng.f32() * 0.24;
            rgba.extend(c.iter().map(|&v| (v as f32 * shade).min(255.0) as u8));
            rgba.push(255);
        }
    }
    let tex = ctx.gfx.create_texture(w, h, &rgba);
    ctx.gfx.set_texture_filter(tex, TextureFilter::Nearest);
    SpriteSheet::new(tex, TILE, TILE)
}

/// 8 frames of a blob that squashes and stretches.
fn creature(ctx: &mut Ctx) -> SpriteSheet {
    let (frames, size) = (8u32, 16u32);
    let mut rgba = vec![0u8; (frames * size * size * 4) as usize];
    for f in 0..frames {
        let phase = f as f32 / frames as f32 * std::f32::consts::TAU;
        let (rx, ry) = (5.0 + phase.sin() * 1.5, 5.0 - phase.sin() * 1.5);
        for y in 0..size {
            for x in 0..size {
                let d = vec2((x as f32 + 0.5 - 8.0) / rx, (y as f32 + 0.5 - 9.0) / ry);
                if d.length() <= 1.0 {
                    let i = ((y * frames * size + f * size + x) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&[240, 110, 80, 255]);
                }
            }
        }
    }
    let tex = ctx.gfx.create_texture(frames * size, size, &rgba);
    ctx.gfx.set_texture_filter(tex, TextureFilter::Nearest);
    SpriteSheet::new(tex, size, size)
}

/// Smooth-ish terrain from a few sine waves: no noise library needed.
fn terrain(x: f32, y: f32) -> u16 {
    let h = (x * 0.031).sin()
        + (y * 0.027).cos()
        + (x * 0.011 + y * 0.013).sin() * 1.5
        + (x * 0.093 - y * 0.071).sin() * 0.3;
    match h {
        h if h < -0.6 => WATER,
        h if h < -0.35 => SAND,
        h if h < 0.8 => GRASS,
        h if h < 1.6 => FOREST,
        _ => STONE,
    }
}

#[derive(Component)]
struct Wander(Vec2);

// ANCHOR: setup
fn setup(ctx: &mut Ctx, side: u32) {
    let tiles = tileset(ctx);
    let mut map = ponko::tilemap::TileMap::new(tiles, side, side);
    for y in 0..side {
        for x in 0..side {
            map.set(x, y, Some(terrain(x as f32, y as f32)));
        }
    }
    ctx.world.spawn((d2::Transform::default(), map));

    let blob = creature(ctx);
    let mut rng = Rng::new(7);
    let world = (side * TILE) as f32;
    ctx.world.spawn_batch((0..20_000).map(|_| {
        let at = vec2(rng.range(0.0, world), rng.range(0.0, world));
        let mut walk = Animation::new(blob, 0..8, rng.range(6.0, 12.0));
        walk.time = rng.f32(); // out of step with each other
        (
            d2::Transform::at(at),
            blob.sprite(0),
            walk,
            Wander(rng.unit_vec2() * 20.0),
        )
    }));
    ctx.camera.zoom = 2.0;
    ctx.camera.look_at(vec2(world, world) / 2.0);
}
// ANCHOR_END: setup

fn wander(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    ctx.world
        .query::<(&mut d2::Transform, &Wander)>()
        .par_for_each(|(t, w)| t.position += w.0 * dt);
}

// ANCHOR: paint
fn paint(ctx: &mut Ctx) {
    if !ctx.input.mouse_down(MouseButton::Left) || ctx.ui.wants_pointer() {
        return;
    }
    let at = ctx.mouse_world();
    ctx.world
        .query::<(&d2::Transform, &mut ponko::tilemap::TileMap)>()
        .for_each(|(t, map)| {
            if let Some(tile) = map.tile_at(t.position, at) {
                map.set(tile.x, tile.y, Some(STONE));
            }
        });
}
// ANCHOR_END: paint

fn hud(ctx: &mut Ctx) {
    let r = ctx.perf.render_stats();
    let text = format!(
        "{} sprites drawn ({} draw calls)\nzoom {:.2}x, {:.0} FPS\nleft mouse: paint, right drag: pan, wheel: zoom",
        r.sprites,
        r.draw_calls,
        ctx.camera.zoom,
        ctx.time.fps()
    );
    let size = ctx.gfx.measure_text(&text, ctx.ui.immediate.style.text_size) + 2.0 * ctx.ui.immediate.style.padding;
    let panel = Rect::new(12.0, 12.0, size.x, size.y);
    let at = panel.min + ctx.ui.immediate.style.padding;
    let mut ui = ctx.immediate_ui();
    ui.panel(panel);
    ui.label(at, &text);
}

fn main() {
    let side = std::env::var("MAP").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
    Game::new("Ponko: tiles", 1280, 720)
        .vsync(false)
        .system(ponko::camera::camera_controls)
        .system(wander)
        .system(paint)
        .system(hud)
        .setup(move |ctx| setup(ctx, side))
        .run();
}
