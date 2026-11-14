//! The tracked workloads: small, single-threaded versions of the main
//! `ponko_bench` groups, sized so a run under cachegrind takes seconds.
//!
//! Every workload runs `iterations` times, either with the measured operation
//! (`measured = true`) or without it. Both runs build the same state, do the
//! same warm-up and loop the same number of times, so the difference between
//! their instruction counts, divided by `iterations`, is what one operation
//! costs. Setup, warm-up, process start and exit cancel out.
//!
//! The workloads use only engine APIs that 0.1.0 already had, so older
//! releases can be measured with today's workloads (`bench_track` is copied
//! onto the older tree; see the crate README).

use std::hint::black_box;

use ponko::d3::Camera as Camera3D;
use ponko::d3::Transform as Transform3D;
use ponko::prelude::{Ctx, Entity, Game, World, vec3};
use ponko::render::{ExtractionBench, Lighting, Model};
use ponko_ecs::With;

/// Entities (or models) in every workload.
const N: usize = 10_000;
const DT: f32 = 1.0 / 60.0;

#[derive(Clone, Copy, Default, ponko_ecs::Component)]
struct Pos {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Default, ponko_ecs::Component)]
struct Vel {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Default, ponko_ecs::Component)]
struct Tag;

#[derive(Clone, Copy, Default, ponko_ecs::Component)]
struct Extra;

/// One tracked workload.
pub struct Workload {
    pub name: &'static str,
    /// What one operation is.
    pub about: &'static str,
    /// Operations per measured run: enough that the per-operation count is
    /// well above the run-to-run differences of setup.
    pub iterations: usize,
    /// Runs the workload `iterations` times, with or without the operation.
    pub run: fn(iterations: usize, measured: bool),
}

pub static WORKLOADS: &[Workload] = &[
    Workload {
        name: "ecs/iter/10k",
        about: "`pos += vel * dt` over 10k entities with a typed query",
        iterations: 50,
        run: iter,
    },
    Workload {
        name: "ecs/query-filtered/10k",
        about: "a `With<Tag>` query over 10k entities in 4 archetypes (half match)",
        iterations: 50,
        run: query_filtered,
    },
    Workload {
        name: "ecs/get/10k",
        about: "10k `World::get` lookups in a scattered order",
        iterations: 20,
        run: get,
    },
    Workload {
        name: "ecs/spawn/10k",
        about: "spawning 10k `(Pos, Vel)` entities into an empty world",
        iterations: 10,
        run: spawn,
    },
    Workload {
        name: "ecs/despawn/10k",
        about: "despawning 10k entities one by one (spawning them and dropping the world aren't measured)",
        iterations: 10,
        run: despawn,
    },
    Workload {
        name: "sim/tick/10k",
        about: "one headless game tick: three systems over 10k entities, scheduler off",
        iterations: 20,
        run: sim_tick,
    },
    Workload {
        name: "render/collect/10k",
        about: "collecting a 3D frame of 10k models (culling and shadows) as the camera moves",
        iterations: 10,
        run: collect,
    },
];

/// The workload called `name`.
pub fn find(name: &str) -> Option<&'static Workload> {
    WORKLOADS.iter().find(|w| w.name == name)
}

/// A hash of this file, recorded with every result: numbers taken with
/// different workloads aren't comparable.
pub fn source_hash() -> String {
    // FNV-1a, 64 bit: stable across Rust versions, unlike `DefaultHasher`.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in include_str!("workloads.rs").bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// `N` moving entities, and their handles in spawn order.
fn moving_world() -> (World, Vec<Entity>) {
    let mut world = World::new();
    let all = (0..N)
        .map(|i| world.spawn((Pos { x: i as f32, y: 0.0 }, Vel { x: 1.0, y: 0.5 })))
        .collect();
    (world, all)
}

fn step(world: &mut World) {
    let dt = black_box(DT);
    world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
        p.x += v.x * dt;
        p.y += v.y * dt;
    });
}

/// A steady workload: the state is built once, the operation warms up twice
/// (in both modes, so the warm-up cancels) and then repeats.
fn steady<S>(iterations: usize, measured: bool, mut state: S, op: impl Fn(&mut S)) {
    for _ in 0..2 {
        op(&mut state);
    }
    for _ in 0..iterations {
        if measured {
            op(&mut state);
        }
        black_box(&mut state);
    }
}

/// A fresh workload: the state is built before every operation. It is
/// leaked afterwards, so dropping it isn't part of the operation.
fn fresh<S>(iterations: usize, measured: bool, build: impl Fn() -> S, op: impl Fn(&mut S)) {
    for _ in 0..iterations {
        let mut state = build();
        if measured {
            op(&mut state);
        }
        std::mem::forget(black_box(state));
    }
}

fn iter(iterations: usize, measured: bool) {
    steady(iterations, measured, moving_world().0, step);
}

fn query_filtered(iterations: usize, measured: bool) {
    let mut world = World::new();
    for i in 0..N {
        let pos = Pos { x: i as f32, y: 0.0 };
        match i % 4 {
            0 => world.spawn((pos, Tag)),
            1 => world.spawn((pos,)),
            2 => world.spawn((pos, Tag, Extra)),
            _ => world.spawn((pos, Extra)),
        };
    }
    steady(iterations, measured, world, |w| {
        let dt = black_box(DT);
        w.query::<&mut Pos>().filter::<With<Tag>>().for_each(|p| p.y += dt);
    });
}

fn get(iterations: usize, measured: bool) {
    let (world, order) = moving_world();
    // A fixed shuffle (a multiplicative step coprime with N).
    let order: Vec<Entity> = (0..N).map(|i| order[(i * 7919) % N]).collect();
    steady(iterations, measured, (world, order), |(w, order)| {
        let mut sum = 0.0;
        for &e in order.iter() {
            sum += w.get::<Pos>(e).map_or(0.0, |p| p.x);
        }
        black_box(sum);
    });
}

fn spawn(iterations: usize, measured: bool) {
    fresh(iterations, measured, World::new, |w| {
        for i in 0..N {
            w.spawn((Pos { x: i as f32, y: 0.0 }, Vel { x: 1.0, y: 0.0 }));
        }
    });
}

fn despawn(iterations: usize, measured: bool) {
    fresh(iterations, measured, moving_world, |(w, all)| {
        for &e in all.iter() {
            w.despawn(e);
        }
    });
}

fn sim_tick(iterations: usize, measured: bool) {
    // `run_headless` builds the game, runs the ticks and stops: the extra
    // ticks of the measured run are the only difference.
    let warmup = 2;
    let ticks = warmup + if measured { iterations } else { 0 };
    let world = Game::new("bench_track", 1, 1)
        .parallel_systems(false)
        .sim_system(|ctx: &mut Ctx| step(&mut ctx.world))
        .sim_system(|ctx: &mut Ctx| {
            ctx.world.query::<(&Pos, &mut Vel)>().for_each(|(p, v)| {
                if p.x > 1000.0 || p.x < 0.0 {
                    v.x = -v.x;
                }
            });
        })
        .sim_system(|ctx: &mut Ctx| {
            let mut far = 0u32;
            ctx.world.query::<&Pos>().for_each(|p| far += u32::from(p.y > 100.0));
            black_box(far);
        })
        .run_headless(ticks as u64, |ctx: &mut Ctx| {
            for i in 0..N {
                ctx.world.spawn((
                    Pos {
                        x: i as f32 % 1000.0,
                        y: 0.0,
                    },
                    Vel { x: 1.0, y: 0.5 },
                ));
            }
        });
    black_box(world);
}

fn collect(iterations: usize, measured: bool) {
    const SIDE: usize = 100;
    const MESHES: u32 = 16;
    let mut extraction = ExtractionBench::new(MESHES);
    let mut world = World::new();
    for i in 0..N {
        let (x, z) = ((i % SIDE) as f32 - 50.0, (i / SIDE) as f32 - 50.0);
        let mesh = extraction.mesh((i as u32).wrapping_mul(2_654_435_761) % MESHES);
        world.spawn((Transform3D::at(vec3(x * 4.0, 0.0, z * 4.0)), Model::new(mesh)));
    }
    let mut camera: Camera3D = ExtractionBench::camera(1280.0, 720.0);
    camera.far = 300.0;
    let lighting = Lighting::default();
    let mut frame = 0usize;
    let mut fly = |camera: &mut Camera3D| {
        let angle = frame as f32 * 0.01;
        camera.position = vec3(angle.cos() * 60.0, 12.0, angle.sin() * 60.0);
        camera.look_at(vec3((angle + 0.5).cos() * 80.0, 0.0, (angle + 0.5).sin() * 80.0));
        frame += 1;
    };
    for _ in 0..2 {
        fly(&mut camera);
        black_box(extraction.collect(&mut world, &camera, &lighting));
    }
    for _ in 0..iterations {
        fly(&mut camera);
        if measured {
            black_box(extraction.collect(&mut world, &camera, &lighting));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workload_runs_with_and_without_its_operation() {
        for w in WORKLOADS {
            (w.run)(1, false);
            (w.run)(1, true);
        }
    }

    #[test]
    fn workload_names_are_unique_and_findable() {
        for w in WORKLOADS {
            assert!(std::ptr::eq(find(w.name).unwrap(), w), "{}", w.name);
            assert!(w.iterations > 0, "{}", w.name);
        }
        assert!(find("nope").is_none());
    }

    #[test]
    fn the_source_hash_is_stable_hex() {
        let h = source_hash();
        assert_eq!(h.len(), 16);
        assert_eq!(h, source_hash());
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
