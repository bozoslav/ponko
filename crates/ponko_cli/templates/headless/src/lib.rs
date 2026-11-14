//! The simulation, shared by the program (`src/main.rs`) and its tests
//! (`tests/sim.rs`). No window, GPU or sound: it runs as fast as the CPU
//! allows, on servers and CI machines too.

use ponko::prelude::*;
use serde::{Deserialize, Serialize};

/// Numbers to tune without recompiling, loaded from `assets/settings.json`.
/// In debug builds, saving the file changes them in the running game.
#[derive(Serialize, Deserialize)]
pub struct Settings {
    /// How many walkers to start with.
    pub walkers: u32,
    /// The side of the square world.
    pub size: f32,
    /// How fast walkers move, in units per second.
    pub speed: f32,
}

/// Something that walks around the world and bounces off its edges.
#[derive(Component, Serialize, Deserialize)]
pub struct Walker {
    /// Where it is.
    pub position: Vec2,
    /// Where it goes, a unit vector.
    pub heading: Vec2,
}

/// How many times walkers bounced off an edge so far.
#[derive(Default, Serialize, Deserialize)]
pub struct Bounces(pub u64);

/// The name agents see.
pub const TITLE: &str = "{{project-name}}";

/// The whole simulation, ready to `run_headless(ticks)`.
pub fn simulation() -> ConfiguredGame {
    Game::new(TITLE, 1, 1).sim_system(walk).setup(setup)
}

fn setup(ctx: &mut Ctx) {
    ctx.assets
        .load_resource::<Settings>(&mut ctx.world, "settings.json");
    ctx.world.insert_resource(Bounces::default());
    // Reflected types are what agents can read and change through ponko-mcp.
    ctx.world.reflect::<Walker>();
    ctx.world.reflect_resource::<Settings>();
    ctx.world.reflect_resource::<Bounces>();

    let settings = ctx.world.resource::<Settings>();
    let (count, size) = (settings.walkers, settings.size);
    // Spread out evenly, each heading another way: the same start every run.
    let walkers: Vec<Walker> = (0..count)
        .map(|i| {
            let f = i as f32 / count.max(1) as f32;
            Walker {
                position: vec2(f * size, (f * 7.0).fract() * size),
                heading: Vec2::from_angle(f * std::f32::consts::TAU * 13.0),
            }
        })
        .collect();
    for w in walkers {
        ctx.world.spawn((w,));
    }
}

/// Sim system: moves every walker and bounces it off the world's edges.
pub fn walk(ctx: &mut Ctx) {
    let settings = ctx.world.resource::<Settings>();
    let (size, step) = (settings.size, settings.speed * ctx.time.dt());
    let mut bounces = 0;
    ctx.world.query::<&mut Walker>().for_each(|w| {
        w.position += w.heading * step;
        for axis in 0..2 {
            if w.position[axis] < 0.0 || w.position[axis] > size {
                w.position[axis] = w.position[axis].clamp(0.0, size);
                w.heading[axis] = -w.heading[axis];
                bounces += 1;
            }
        }
    });
    ctx.world.resource_mut::<Bounces>().0 += bounces;
}
