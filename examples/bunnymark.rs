//! Stress test: lots of bouncing crabs. Prints FPS and frame time once a second.
//!
//! Run in release mode, or the numbers mean nothing:
//!   `cargo run --release --example bunnymark`
//!   `CRABS=100000 cargo run --release --example bunnymark`
//!
//! - Hold left mouse: add 1,000 crabs per frame
//! - Escape: quit
//!
//! Vsync is off so the FPS shows what the engine can actually do.
//! Set `SECONDS=10` to quit automatically after that long (for scripted runs).

use ponko::prelude::*;

struct Crab {
    pos: Vec2,
    vel: Vec2,
    color: Color,
}

/// Tiny deterministic RNG (xorshift) so runs are comparable.
struct Rng(u64);
impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let start_count = env_number("CRABS", 10_000.0) as usize;
    let quit_after = env_number("SECONDS", 0.0);

    Game::new("Ponko: bunnymark", 1280, 720).vsync(false).run(move |ctx| {
        let crab = ctx.gfx.load_texture("crab.png");
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut crabs: Vec<Crab> = Vec::new();

        let mut spawn = move |crabs: &mut Vec<Crab>, n: usize, at: Vec2| {
            crabs.reserve(n);
            for _ in 0..n {
                crabs.push(Crab {
                    pos: at,
                    vel: vec2(rng.next_f32() * 400.0 - 200.0, rng.next_f32() * 400.0 - 300.0),
                    color: Color::rgb(0.6 + rng.next_f32() * 0.4, 0.6 + rng.next_f32() * 0.4, 0.6 + rng.next_f32() * 0.4),
                });
            }
        };
        spawn(&mut crabs, start_count, vec2(20.0, 20.0));

        let mut report_at = 1.0;
        let mut frames_since = 0u32;
        let mut worst_ms = 0.0f32;
        let mut frame_start = ponko::perf::Instant::now();
        let mut update_ms_sum = 0.0f64;

        move |ctx: &mut Ctx| {
            let now = ponko::perf::Instant::now();
            let frame_ms = now.duration_since(frame_start).as_secs_f32() * 1000.0;
            frame_start = now;
            worst_ms = worst_ms.max(frame_ms);
            frames_since += 1;

            if ctx.input.mouse_down(MouseButton::Left) {
                spawn(&mut crabs, 1000, ctx.input.mouse_pos());
            }

            let update_start = ponko::perf::Instant::now();
            let dt = ctx.time.dt();
            let bounds = ctx.window_size();
            let gravity = 600.0;
            for c in &mut crabs {
                c.vel.y += gravity * dt;
                c.pos += c.vel * dt;
                if c.pos.x < 0.0 { c.pos.x = 0.0; c.vel.x = c.vel.x.abs(); }
                if c.pos.x > bounds.x { c.pos.x = bounds.x; c.vel.x = -c.vel.x.abs(); }
                if c.pos.y > bounds.y { c.pos.y = bounds.y; c.vel.y = -c.vel.y.abs() * 0.85; }
                if c.pos.y < 0.0 { c.pos.y = 0.0; c.vel.y = 0.0; }
                ctx.gfx.draw_ex(crab, c.pos, DrawParams::default().with_color(c.color));
            }
            update_ms_sum += update_start.elapsed().as_secs_f64() * 1000.0;

            if ctx.time.elapsed() >= report_at {
                println!(
                    "crabs: {:>8}  fps: {:>6.1}  avg frame: {:>6.2} ms  worst: {:>6.2} ms  update+draw calls: {:>6.2} ms",
                    crabs.len(),
                    frames_since as f64 / 1.0,
                    1000.0 / frames_since as f64,
                    worst_ms,
                    update_ms_sum / frames_since as f64,
                );
                report_at += 1.0;
                frames_since = 0;
                worst_ms = 0.0;
                update_ms_sum = 0.0;
            }

            if ctx.input.key_pressed(Key::Escape) || (quit_after > 0.0 && ctx.time.elapsed() >= quit_after) {
                ctx.quit();
            }
        }
    });
}
