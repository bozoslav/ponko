//! Bunnymark on the ECS: the same bouncing crabs as `bunnymark`, stored as
//! entities and drawn by the built-in `draw_sprites` system. Compare the two
//! to see what the ECS costs.
//!
//!   `cargo run --release --example bunnymark_ecs`
//!   `CRABS=100000 cargo run --release --example bunnymark_ecs`
//!   `CRABS=1000000 SCALE=0.25 cargo run --release --example bunnymark_ecs` (smaller crabs: fewer pixels for the GPU to fill)
//!
//! - Hold left mouse: add 1,000 crabs per frame
//! - Escape: quit
//!
//! Vsync is off. `SECONDS=10` quits after that long (for scripted runs).
//! Movement runs once per frame (not on the fixed tick) to match `bunnymark`.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::prelude::*;

#[derive(Component)]
struct Vel(Vec2);

struct Rng(u64);
impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

struct Bench {
    crab: Texture,
    rng: Rng,
    report_at: f64,
    frames: u32,
    worst_ms: f32,
    frame_start: ponko::perf::Instant,
    quit_after: f64,
    scale: f32,
}

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn spawn(ctx: &mut Ctx, n: usize, at: Vec2) {
    let b = ctx.world.resource_mut::<Bench>();
    let crab = b.crab;
    let mut batch = Vec::with_capacity(n);
    for _ in 0..n {
        let r = &mut b.rng;
        let vel = vec2(r.next_f32() * 400.0 - 200.0, r.next_f32() * 400.0 - 300.0);
        let color = Color::rgb(
            0.6 + r.next_f32() * 0.4,
            0.6 + r.next_f32() * 0.4,
            0.6 + r.next_f32() * 0.4,
        );
        batch.push((
            d2::Transform {
                scale: Vec2::splat(b.scale),
                ..d2::Transform::at(at)
            },
            Sprite::new(crab).with_color(color),
            Vel(vel),
        ));
    }
    ctx.world.spawn_batch(batch);
}

fn bounce(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let bounds = ctx.window_size();
    let gravity = 600.0;
    ctx.world.query::<(&mut d2::Transform, &mut Vel)>().for_each(|(t, v)| {
        let (p, v) = (&mut t.position, &mut v.0);
        v.y += gravity * dt;
        *p += *v * dt;
        if p.x < 0.0 {
            p.x = 0.0;
            v.x = v.x.abs();
        }
        if p.x > bounds.x {
            p.x = bounds.x;
            v.x = -v.x.abs();
        }
        if p.y > bounds.y {
            p.y = bounds.y;
            v.y = -v.y.abs() * 0.85;
        }
        if p.y < 0.0 {
            p.y = 0.0;
            v.y = 0.0;
        }
    });
}

fn report(ctx: &mut Ctx) {
    if ctx.input.mouse_down(MouseButton::Left) {
        let at = ctx.input.mouse_pos();
        spawn(ctx, 1000, at);
    }
    // "update + draw calls" = every system except this one, as measured by the
    // engine (smoothed averages; draw_sprites hasn't run yet this frame).
    let systems_ms: f32 = ctx
        .perf
        .system_timings()
        .iter()
        .filter(|t| !t.name.ends_with("report"))
        .map(|t| t.avg_ms)
        .sum();
    let (time, count, render) = (ctx.time.elapsed(), ctx.world.len(), ctx.perf.render_stats());
    let b = ctx.world.resource_mut::<Bench>();
    let now = ponko::perf::Instant::now();
    let frame_ms = now.duration_since(b.frame_start).as_secs_f32() * 1000.0;
    b.frame_start = now;
    b.worst_ms = b.worst_ms.max(frame_ms);
    b.frames += 1;
    if time >= b.report_at {
        println!(
            "crabs: {:>8}  fps: {:>6.1}  avg frame: {:>6.2} ms  worst: {:>6.2} ms  update+draw calls: {:>6.2} ms  render cpu: {:>5.2} ms  gpu: {}",
            count,
            b.frames,
            1000.0 / b.frames as f64,
            b.worst_ms,
            systems_ms,
            render.cpu_ms,
            render.gpu_ms.map_or("n/a".into(), |g| format!("{g:.2} ms")),
        );
        b.report_at += 1.0;
        b.frames = 0;
        b.worst_ms = 0.0;
    }
    let quit_after = b.quit_after;
    if ctx.input.key_pressed(Key::Escape) || (quit_after > 0.0 && time >= quit_after) {
        ctx.quit();
    }
}

fn main() {
    let start_count = env_number("CRABS", 10_000.0) as usize;
    let quit_after = env_number("SECONDS", 0.0);

    Game::new("Ponko: bunnymark (ECS)", 1280, 720)
        .vsync(false)
        .system(bounce)
        .system(report)
        .setup(move |ctx| {
            let crab = ctx.gfx.load_texture("crab.png");
            ctx.world.insert_resource(Bench {
                crab,
                rng: Rng(0x9E37_79B9_7F4A_7C15),
                report_at: 1.0,
                frames: 0,
                worst_ms: 0.0,
                frame_start: ponko::perf::Instant::now(),
                quit_after,
                scale: env_number("SCALE", 1.0) as f32,
            });
            spawn(ctx, start_count, vec2(20.0, 20.0));
        })
        .run();
}
