//! Boids: flocking with separation, alignment and cohesion, using a spatial
//! grid for neighbor search and all CPU cores for the update.
//!
//!   `cargo run --release --example boids`
//!   `BOIDS=100000 cargo run --release --example boids`
//!   `HEADLESS=1 BOIDS=1000000 TICKS=300 cargo run --release --example boids`
//!
//! Keys: Space pause, Period single step, 1-4 speed (1x/4x/16x/64x), Escape quit.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::prelude::*;
use ponko::spatial::SpatialGrid;

/// Reflected, so tools and agents (ponko-mcp) can read and change velocities.
#[derive(serde::Serialize, serde::Deserialize, Component)]
struct Vel(Vec2);

const RADIUS: f32 = 24.0;
const MAX_SPEED: f32 = 140.0;
const MIN_SPEED: f32 = 60.0;

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

// ANCHOR: flocking
/// The flocking system. It owns a grid that stores each boid's velocity, so
/// neighbors' velocities can be read while the world is busy with the query.
fn flocking(bounds: Vec2) -> impl FnMut(&mut Ctx) {
    let mut grid = SpatialGrid::with_data(RADIUS);
    move |ctx: &mut Ctx| {
        let dt = ctx.time.dt();
        // Keep boids that are close in space close in memory, so neighbor
        // lookups hit the cache (1M boids: 62 -> 40 ms/tick).
        if ctx
            .time
            .sim_tick()
            .is_multiple_of(ponko::spatial::SpatialGrid::<()>::SORT_EVERY)
        {
            ctx.world
                .sort_by::<d2::Transform>(|t| ponko::spatial::spatial_key(t.position, RADIUS));
        }
        grid.rebuild_with_data(
            ctx.world
                .query::<(Entity, &d2::Transform, &Vel)>()
                .iter()
                .map(|(e, t, v)| (e, t.position, v.0)),
        );
        let grid = &grid;
        ctx.world
            .query::<(Entity, &mut d2::Transform, &mut Vel)>()
            .par_for_each(|(me, t, v)| {
                let (mut away, mut heading, mut center, mut n) = (Vec2::ZERO, Vec2::ZERO, Vec2::ZERO, 0.0);
                grid.for_each_in_radius(t.position, RADIUS, |o| {
                    if o.entity == me {
                        return;
                    }
                    let d = t.position - o.position;
                    away += d / d.length_squared().max(1.0);
                    heading += o.data;
                    center += o.position;
                    n += 1.0;
                });
                let mut steer = away * 900.0;
                if n > 0.0 {
                    steer += (heading / n - v.0) * 1.5 + (center / n - t.position) * 1.2;
                }
                let speed = (v.0 + steer * dt).length().clamp(MIN_SPEED, MAX_SPEED);
                v.0 = (v.0 + steer * dt).normalize_or(Vec2::X) * speed;
                t.position = (t.position + v.0 * dt).rem_euclid(bounds);
                t.rotation = v.0.to_angle();
            });
    }
}
// ANCHOR_END: flocking

fn controls(ctx: &mut Ctx) {
    if ctx.input.key_pressed(Key::Space) {
        if ctx.time.is_paused() {
            ctx.time.resume()
        } else {
            ctx.time.pause()
        }
    }
    if ctx.input.key_pressed(Key::Period) {
        ctx.time.step();
    }
    for (key, speed) in [(Key::Num1, 1.0), (Key::Num2, 4.0), (Key::Num3, 16.0), (Key::Num4, 64.0)] {
        if ctx.input.key_pressed(key) {
            ctx.time.set_speed(speed);
        }
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

fn spawn_boids(ctx: &mut Ctx, n: usize, bounds: Vec2) {
    ctx.world.reflect::<Vel>();
    let tex = ctx.gfx.load_texture("crab.png");
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    let mut rand = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 40) as f32 / (1u64 << 24) as f32
    };
    let mut batch = Vec::with_capacity(n);
    for _ in 0..n {
        let pos = vec2(rand(), rand()) * bounds;
        let vel = Vec2::from_angle(rand() * std::f32::consts::TAU) * MIN_SPEED;
        let tint = Color::rgb(0.5 + rand() * 0.5, 0.7 + rand() * 0.3, 0.9);
        batch.push((
            d2::Transform::at(pos).scaled(Vec2::splat(0.5)),
            Sprite::new(tex).with_color(tint),
            Vel(vel),
        ));
    }
    ctx.world.spawn_batch(batch);
}

fn main() {
    let n = env_number("BOIDS", 20_000.0) as usize;
    let headless = env_number("HEADLESS", 0.0) != 0.0;

    if headless {
        // Keep density like the windowed version: scale the world with the count.
        let side = (n as f32 / 20_000.0).sqrt() * 1280.0;
        let bounds = Vec2::splat(side);
        let ticks = env_number("TICKS", 300.0) as u64;
        let mut start = ponko::perf::Instant::now();
        let world = Game::new("boids", 1, 1)
            .sim_system(flocking(bounds))
            // The two-argument form may borrow a local, here the timer.
            .run_headless(ticks, |ctx| {
                spawn_boids(ctx, n, bounds);
                start = ponko::perf::Instant::now();
            });
        let secs = start.elapsed().as_secs_f64();
        println!(
            "{} boids, {ticks} ticks: {:.2} ms/tick ({:.0} ticks/s)",
            world.len(),
            secs * 1e3 / ticks as f64,
            ticks as f64 / secs
        );
        return;
    }

    let bounds = vec2(1280.0, 720.0);
    Game::new("Ponko: boids", 1280, 720)
        .sim_system(flocking(bounds))
        .system(controls)
        .system(report())
        .setup(move |ctx| spawn_boids(ctx, n, bounds))
        .run();
}

/// Frame system with its own state: prints the numbers once a second.
fn report() -> impl FnMut(&mut Ctx) {
    let mut next = 1.0;
    move |ctx: &mut Ctx| {
        if ctx.time.elapsed() >= next {
            next += 1.0;
            let sim_ms: f32 = ctx
                .perf
                .system_timings()
                .iter()
                .filter(|t| t.sim)
                .map(|t| t.avg_ms)
                .sum();
            println!(
                "boids: {}  fps: {:.0}  sim: {:.2} ms/frame  tick: {}",
                ctx.world.len(),
                ctx.time.fps(),
                sim_ms,
                ctx.time.sim_tick()
            );
        }
    }
}
