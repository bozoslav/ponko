//! Sprite blend modes and sprite shaders: an additive glow, an outline and
//! a dissolve, on entities and on immediate draws.
//!
//! Run with: `cargo run --example sprite_effects`
//!
//! - The glow under the mouse is an immediate draw with additive blending
//! - Edit `assets/shaders/sprite_outline.wgsl` or `sprite_dissolve.wgsl`
//!   while it runs: the crabs change as you save (errors are printed, and
//!   the last good version stays)
//! - Escape: quit

use ponko::d2::{Sprite, Transform};
use ponko::prelude::*;
use ponko::render::{BlendMode, SpriteMaterialDesc};

/// A white disc fading out to its edge: the glow.
fn soft_circle(ctx: &mut Ctx, size: u32) -> Texture {
    let center = Vec2::splat(size as f32 / 2.0);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = vec2(x as f32 + 0.5, y as f32 + 0.5).distance(center) / center.x;
            let a = (1.0 - d).clamp(0.0, 1.0).powi(2);
            pixels.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
        }
    }
    ctx.gfx.create_texture(size, size, &pixels)
}

/// Smooth value noise: where the dissolve burns first.
fn noise(ctx: &mut Ctx, size: u32) -> Texture {
    let cell = |x: u32, y: u32| {
        let mut h = (x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263)).wrapping_add(0x9E37_79B9);
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        (h >> 24) as f32 / 255.0
    };
    let grid = 8;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (
                x as f32 * grid as f32 / size as f32,
                y as f32 * grid as f32 / size as f32,
            );
            let (cx, cy) = (fx as u32, fy as u32);
            let (tx, ty) = (fx.fract(), fy.fract());
            let (tx, ty) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
            let top = cell(cx, cy) * (1.0 - tx) + cell(cx + 1, cy) * tx;
            let bottom = cell(cx, cy + 1) * (1.0 - tx) + cell(cx + 1, cy + 1) * tx;
            let v = ((top * (1.0 - ty) + bottom * ty) * 255.0) as u8;
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
    }
    ctx.gfx.create_texture(size, size, &pixels)
}

fn main() {
    Game::new("Ponko: sprite effects", 800, 600).run(|ctx| {
        ctx.gfx.clear(Color::hex(0x141B26));
        let crab = ctx.gfx.load_texture("crab.png");
        ctx.gfx.set_texture_filter(crab, TextureFilter::Nearest);
        let glow = soft_circle(ctx, 64);
        let noise = noise(ctx, 64);

        // Additive glow: an entity with a blend mode, behind a crab.
        let left = vec2(200.0, 300.0);
        ctx.world.spawn((
            Transform::at(left).scaled(Vec2::splat(4.0)),
            Sprite::new(glow).with_color(Color::rgb(1.0, 0.55, 0.15)),
            BlendMode::Additive,
        ));
        ctx.world.spawn((
            Transform::at(left).scaled(Vec2::splat(4.0)),
            Sprite::new(crab).with_z(1.0),
        ));

        // Outline: a sprite material with a shader and two parameters.
        let outline = ctx.gfx.load_sprite_shader("shaders/sprite_outline.wgsl");
        let outlined = ctx.gfx.create_sprite_material(
            SpriteMaterialDesc::new(outline)
                .with_param(0, vec4(1.0, 0.85, 0.2, 1.0))
                .with_param(1, vec4(1.0, 0.0, 0.0, 0.0)),
        );
        ctx.world.spawn((
            Transform::at(vec2(400.0, 300.0)).scaled(Vec2::splat(4.0)),
            Sprite::new(crab),
            outlined,
        ));

        // Dissolve: a material with a noise texture, its progress animated.
        let dissolve = ctx.gfx.load_sprite_shader("shaders/sprite_dissolve.wgsl");
        let burn = SpriteMaterialDesc::new(dissolve)
            .with_param(1, vec4(1.0, 0.45, 0.1, 1.0))
            .with_texture(0, noise);
        let burning = ctx.gfx.create_sprite_material(burn);
        ctx.world.spawn((
            Transform::at(vec2(600.0, 300.0)).scaled(Vec2::splat(4.0)),
            Sprite::new(crab),
            burning,
        ));

        move |ctx: &mut Ctx| {
            let t = ctx.time.elapsed() as f32;
            // Burn away over two seconds, wait, come back.
            let burned = ((t * 0.4).fract() * 1.4 - 0.2).clamp(0.0, 1.0);
            ctx.gfx
                .update_sprite_material(burning, burn.with_param(0, vec4(burned, 0.0, 0.0, 0.0)));

            // An immediate additive draw: light where the mouse is.
            ctx.gfx.draw_ex(
                glow,
                ctx.input.mouse_pos(),
                DrawParams::default()
                    .with_blend(BlendMode::Additive)
                    .with_scale(Vec2::splat(2.0 + (t * 4.0).sin() * 0.3))
                    .with_color(Color::rgb(0.3, 0.6, 1.0))
                    .with_z(2.0),
            );
            for (x, label) in [(150.0, "additive glow"), (360.0, "outline"), (560.0, "dissolve")] {
                ctx.gfx.draw_text(label, vec2(x, 400.0), 18.0, Color::WHITE);
            }
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
        }
    });
}
