//! Headless simulation: 1M wandering agents, no window, no GPU.
//! Prints how many simulation ticks per second the machine sustains.
//!
//!   `cargo run --release --example headless`
//!   `AGENTS=10000000 TICKS=300 cargo run --release --example headless`

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::prelude::*;

#[derive(Component)]
struct Agent {
    heading: f32,
    speed: f32,
}

const WORLD: f32 = 4096.0;

/// Wander steering on all cores: the per-agent work of a typical sim tick.
fn wander(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    ctx.world
        .query::<(&mut d2::Transform, &mut Agent)>()
        .par_for_each(|(t, a)| {
            let p = &mut t.position;
            a.heading += (p.x * 0.013 + p.y * 0.007 + a.heading).sin() * 0.8 * dt;
            p.x = (p.x + a.heading.cos() * a.speed * dt).rem_euclid(WORLD);
            p.y = (p.y + a.heading.sin() * a.speed * dt).rem_euclid(WORLD);
            t.rotation = a.heading;
        });
}

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let agents = env_number("AGENTS", 1_000_000.0) as usize;
    let ticks = env_number("TICKS", 600.0) as u64;

    let spawn_start = ponko::perf::Instant::now();
    let mut setup_done = ponko::perf::Instant::now();
    let world = Game::new("headless", 1280, 720)
        .sim_system(wander)
        // The two-argument form may borrow locals, here the timers.
        .run_headless(ticks, |ctx| {
            let tex = ctx.gfx.load_texture("crab.png");
            ctx.world.spawn_batch((0..agents).map(|i| {
                let pos = vec2((i % 4096) as f32, (i / 4096 % 4096) as f32);
                (
                    d2::Transform::at(pos),
                    Sprite::new(tex),
                    Agent {
                        heading: i as f32,
                        speed: 30.0,
                    },
                )
            }));
            println!(
                "spawned {agents} agents in {:.0} ms",
                spawn_start.elapsed().as_secs_f64() * 1e3
            );
            setup_done = ponko::perf::Instant::now();
        });
    let secs = setup_done.elapsed().as_secs_f64();
    println!(
        "{ticks} ticks in {:.2} s: {:.0} ticks/s, {:.2} ms/tick ({} entities, {:.0} bytes/entity allocated)",
        secs,
        ticks as f64 / secs,
        secs * 1e3 / ticks as f64,
        world.len(),
        world.allocated_bytes() as f64 / world.len().max(1) as f64
    );
}
