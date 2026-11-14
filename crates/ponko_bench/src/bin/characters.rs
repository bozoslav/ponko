//! How long `step_physics3d` takes per tick with N `Character3D`s walking on
//! a flat floor, to place the character controller's parallel threshold
//! (`PARALLEL_FROM` in `src/physics3d/character.rs`).
//!
//! Run with: cargo run --release -p ponko_bench --bin characters [-- 8 16 32]
//! Prints the best of several runs of the per-tick average, per count.
use ponko::d3::Character as Character3D;
use ponko::d3::Collider as Collider3D;
use ponko::d3::RigidBody as RigidBody3D;
use ponko::prelude::*;

const WARMUP: u64 = 60;
const TICKS: u64 = 300;
const RUNS: usize = 7;

fn step_ms(n: usize) -> f32 {
    let (_, report) = Game::new("characters", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            // Walk in circles at 2 m/s, each a little out of phase.
            let t = ctx.time.sim_tick() as f32 * 0.02;
            ctx.world.query::<&mut Character3D>().for_each(|c| {
                let a = t + c.radius * 100.0;
                c.velocity.x = 2.0 * a.cos();
                c.velocity.z = 2.0 * a.sin();
            });
        })
        .physics3d(vec3(0.0, -9.81, 0.0))
        .profile_headless(WARMUP, TICKS, move |ctx| {
            ctx.world.spawn((
                d3::Transform::at(vec3(0.0, -0.5, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::cuboid(vec3(500.0, 0.5, 500.0)),
            ));
            let side = (n as f32).sqrt().ceil() as usize;
            for i in 0..n {
                let (x, z) = ((i % side) as f32, (i / side) as f32);
                let mut c = Character3D::new(1.8, 0.3 + (i % 7) as f32 * 0.001);
                c.push_mass = 0.0;
                ctx.world.spawn((d3::Transform::at(vec3(x * 2.0, 0.1, z * 2.0)), c));
            }
        });
    report
        .systems
        .iter()
        .find(|s| s.name.ends_with("step_physics3d"))
        .map_or(f32::NAN, |s| s.avg_ms)
}

fn main() {
    let counts: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let counts = if counts.is_empty() {
        vec![1, 2, 4, 8, 16, 32, 64, 256]
    } else {
        counts
    };
    println!("characters  step_physics3d ms/tick (best of {RUNS}, {TICKS} ticks)");
    for n in counts {
        let best = (0..RUNS).map(|_| step_ms(n)).fold(f32::INFINITY, f32::min);
        println!("{n:>10}  {best:.4}");
    }
}
