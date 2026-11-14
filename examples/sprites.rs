//! Drawing sprites: position, rotation, scale, tint, draw order and pivot.
//!
//! Run with: `cargo run --example sprites`
//!
//! - Arrow keys / WASD: move the big crab
//! - The small crab follows the mouse
//! - Escape: quit

use ponko::prelude::*;

fn main() {
    Game::new("Ponko: sprites", 800, 600).run(|ctx| {
        ctx.gfx.clear(Color::hex(0x1E2A38));
        let crab = ctx.gfx.load_texture("crab.png");

        // A generated texture: a soft white circle.
        let size = 32;
        let mut pixels = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            for x in 0..size {
                let d = vec2(x as f32 + 0.5, y as f32 + 0.5).distance(Vec2::splat(size as f32 / 2.0));
                let a = ((size as f32 / 2.0 - d).clamp(0.0, 1.0) * 255.0) as u8;
                pixels.extend_from_slice(&[255, 255, 255, a]);
            }
        }
        let dot = ctx.gfx.create_texture(size as u32, size as u32, &pixels);

        let mut player = ctx.window_size() / 2.0;

        move |ctx: &mut Ctx| {
            let t = ctx.time.elapsed() as f32;

            let mut dir = Vec2::ZERO;
            if ctx.input.key_down(Key::Left) || ctx.input.key_down(Key::A) {
                dir.x -= 1.0;
            }
            if ctx.input.key_down(Key::Right) || ctx.input.key_down(Key::D) {
                dir.x += 1.0;
            }
            if ctx.input.key_down(Key::Up) || ctx.input.key_down(Key::W) {
                dir.y -= 1.0;
            }
            if ctx.input.key_down(Key::Down) || ctx.input.key_down(Key::S) {
                dir.y += 1.0;
            }
            player += dir.normalize_or_zero() * 250.0 * ctx.time.dt();

            // A ring of tinted, spinning dots around the player, drawn behind it (z = -1).
            for i in 0..12 {
                let angle = t + i as f32 * std::f32::consts::TAU / 12.0;
                let hue = i as f32 / 12.0;
                ctx.gfx.draw_ex(
                    dot,
                    player + Vec2::from_angle(angle) * 120.0,
                    DrawParams::default()
                        .with_color(Color::rgb(
                            0.5 + 0.5 * (hue * std::f32::consts::TAU).cos(),
                            0.6,
                            0.5 + 0.5 * (hue * std::f32::consts::TAU).sin(),
                        ))
                        .with_z(-1.0),
                );
            }

            // The player: big and wobbling. Drawn first in code but z = 1 puts it on top.
            ctx.gfx.draw_ex(
                crab,
                player,
                DrawParams::default()
                    .with_scale(Vec2::splat(4.0))
                    .with_rotation((t * 3.0).sin() * 0.2)
                    .with_z(1.0),
            );

            // A row of crabs: plain, flipped, tinted, half transparent, pivot at top-left.
            let row_y = 80.0;
            ctx.gfx.draw_ex(
                crab,
                vec2(100.0, row_y),
                DrawParams::default().with_scale(Vec2::splat(2.0)),
            );
            ctx.gfx.draw_ex(
                crab,
                vec2(200.0, row_y),
                DrawParams::default().with_scale(vec2(-2.0, 2.0)),
            );
            ctx.gfx.draw_ex(
                crab,
                vec2(300.0, row_y),
                DrawParams::default()
                    .with_scale(Vec2::splat(2.0))
                    .with_color(Color::hex(0x66CCFF)),
            );
            ctx.gfx.draw_ex(
                crab,
                vec2(400.0, row_y),
                DrawParams::default()
                    .with_scale(Vec2::splat(2.0))
                    .with_color(Color::WHITE.with_alpha(0.4)),
            );
            ctx.gfx.draw_ex(
                crab,
                vec2(500.0, row_y),
                DrawParams::default()
                    .with_scale(Vec2::splat(2.0))
                    .with_pivot(Vec2::ZERO)
                    .with_rotation(t),
            );

            // A small crab following the mouse, on top of everything.
            ctx.gfx
                .draw_ex(crab, ctx.input.mouse_pos(), DrawParams::default().with_z(2.0));

            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
        }
    });
}
