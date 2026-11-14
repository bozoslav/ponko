//! The parallel system scheduler (`Game::parallel_systems`) against running
//! systems one at a time (`schedule/...`). See `SCHEDULE.md`.
//!
//! Every benchmark is a headless game; the time is one whole simulation
//! tick, measured inside the game from before the first system to after the
//! last. Modes: `seq` is the scheduler off, `auto` is on with the default
//! threshold, `threads` is on with a threshold of zero (every batch goes to
//! the thread pool).
use std::hint::black_box;
use std::time::{Duration, Instant};

use ponko::access::SystemAccess;
use ponko::prelude::*;
use ponko::system_view::SystemView;

use crate::{Bench, report};

/// Two floats per entity; every `N` is its own component, so systems over
/// different `N` don't conflict.
#[derive(Clone, Copy, ponko_ecs::Component)]
struct Val<const N: usize>(f32, f32);

/// What every entity starts with, before its `Val`s are inserted.
#[derive(ponko_ecs::Component)]
struct Row;

/// When the tick started, and every tick's time so far.
struct Timer {
    start: Instant,
    ticks: Vec<Duration>,
}

const MODES: [&str; 3] = ["seq", "auto", "threads"];

fn game(mode: &str) -> Game {
    let game = Game::new("schedule bench", 1, 1);
    let game = match mode {
        // The scheduler is never mentioned: what an existing game does.
        "default" => game,
        "seq" => game.parallel_systems(false),
        "auto" => game.parallel_systems(true),
        _ => game.parallel_systems(true).parallel_threshold(Duration::ZERO),
    };
    game.add(Phase::Input, |ctx: &mut Ctx| {
        ctx.world.resource_mut::<Timer>().start = Instant::now();
    })
    .add(Phase::SimLast, |ctx: &mut Ctx| {
        let timer = ctx.world.resource_mut::<Timer>();
        let took = timer.start.elapsed();
        timer.ticks.push(took);
    })
}

/// `v += w * dt`: well under a nanosecond per entity.
fn light<const N: usize>(sys: &mut SystemView) {
    let dt = black_box(1.0 / 60.0);
    sys.world.query::<&mut Val<N>>().for_each(|v| v.0 += v.1 * dt);
}

/// `light` as a `&mut Ctx` system.
fn light_ctx<const N: usize>(ctx: &mut Ctx) {
    let dt = black_box(1.0 / 60.0);
    ctx.world.query::<&mut Val<N>>().for_each(|v| v.0 += v.1 * dt);
}

/// A few transcendental calls: roughly 20 ns per entity.
fn heavy<const N: usize>(sys: &mut SystemView) {
    let dt = black_box(1.0 / 60.0);
    sys.world.query::<&mut Val<N>>().for_each(|v| {
        for _ in 0..4 {
            let (s, c) = (v.0 * 0.001 + v.1).sin_cos();
            v.0 += (c - v.1 * s) * dt;
            v.1 = (v.1 + (c + v.0 * s) * dt).rem_euclid(4096.0);
        }
    });
}

/// Reads `Val<N>` and writes `Val<M>`: conflicts with the writers of both.
fn link<const N: usize, const M: usize>(sys: &mut SystemView) {
    let dt = black_box(1.0 / 60.0);
    sys.world
        .query::<(&Val<N>, &mut Val<M>)>()
        .for_each(|(a, b)| b.0 += a.0 * dt);
}

/// Runs `$body` once for each of the first `$n` component numbers (up to 64).
macro_rules! for_each_val {
    ($n:expr, $N:ident => $body:expr) => {
        for_each_val!(@go $n, $N => $body;
            0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15
            16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
            32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47
            48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63)
    };
    (@go $n:expr, $N:ident => $body:expr; $($i:literal)*) => {$(
        if $i < $n {
            const $N: usize = $i;
            $body
        }
    )*};
}

/// `entities` entities with the first `components` `Val` components each.
fn spawn(ctx: &mut Ctx, entities: usize, components: usize) {
    ctx.world.insert_resource(Timer {
        start: Instant::now(),
        ticks: Vec::new(),
    });
    let made: Vec<Entity> = (0..entities).map(|_| ctx.world.spawn((Row,))).collect();
    for_each_val!(components, N => {
        for (i, &e) in made.iter().enumerate() {
            ctx.world.insert(e, Val::<N>(i as f32, 1.0));
        }
    });
}

/// Runs `game` headless and reports its ticks after `warmup`.
fn measure(b: &mut Bench, name: &str, game: Game, warmup: usize, ticks: usize, setup: impl FnOnce(&mut Ctx)) {
    let mut world = game.run_headless((warmup + ticks) as u64, setup);
    let mut times = world.remove_resource::<Timer>().expect("the timer").ticks;
    let mut times = times.split_off(warmup);
    b.report(report::Record::new(name, report::Kind::Steady, warmup, &mut times));
}

/// `systems` independent systems, each over its own component of
/// `entities` entities.
fn independent(b: &mut Bench, group: &str, systems: usize, entities: usize, work: &str) {
    let ticks = if systems * entities >= 1_000_000 { 100 } else { 400 };
    for mode in MODES {
        let name = format!("schedule/{group}/{systems}x{entities}/{work}/{mode}");
        if b.skips(&name) {
            continue;
        }
        let mut game = game(mode);
        for_each_val!(systems, N => {
            let access = SystemAccess::new().write::<Val<N>>();
            game = if work == "light" {
                game.add(Phase::AfterPhysics, light::<N>.access(access))
            } else {
                game.add(Phase::AfterPhysics, heavy::<N>.access(access))
            };
        });
        measure(b, &name, game, 60, ticks, |ctx| spawn(ctx, entities, systems));
    }
}

/// The path every existing game takes: the scheduler is never turned on.
/// `ctx` systems take `&mut Ctx`, `views` are view systems. These use
/// nothing newer than view systems, so the same benchmarks run on a
/// revision without the scheduler (`schedule-off/`), for before and after.
fn scheduler_off(b: &mut Bench) {
    for (systems, entities) in [(64, 100), (8, 16_000), (8, 250_000)] {
        for kind in ["ctx", "views"] {
            let name = format!("schedule-off/{kind}/{systems}x{entities}/light");
            if b.skips(&name) {
                continue;
            }
            let mut game = game("default");
            for_each_val!(systems, N => {
                game = if kind == "ctx" {
                    game.add(Phase::AfterPhysics, light_ctx::<N>)
                } else {
                    game.add(Phase::AfterPhysics, light::<N>.access(SystemAccess::new().write::<Val<N>>()))
                };
            });
            let ticks = if systems * entities >= 1_000_000 { 100 } else { 400 };
            measure(b, &name, game, 60, ticks, |ctx| spawn(ctx, entities, systems));
        }
    }
}

pub fn run(b: &mut Bench) {
    scheduler_off(b);
    // Many tiny systems: what a simple game looks like. Other threads can
    // only cost here.
    for systems in [4, 16, 64] {
        independent(b, "tiny", systems, 100, "light");
    }
    // The crossover: eight independent systems of growing size.
    for entities in [100, 1_000, 4_000, 16_000, 64_000, 250_000] {
        independent(b, "sweep", 8, entities, "light");
    }
    for entities in [100, 1_000, 4_000, 16_000, 64_000] {
        independent(b, "sweep", 8, entities, "heavy");
    }
    // Two big systems: the smallest batch there is.
    independent(b, "pair", 2, 64_000, "heavy");

    // A chain: every system reads what the one before wrote, so there is
    // nothing to run together. Shows what the scheduler costs on its own.
    for mode in MODES {
        let name = format!("schedule/chain/8x16000/light/{mode}");
        if b.skips(&name) {
            continue;
        }
        let mut game = game(mode);
        macro_rules! links {
            ($($from:literal $to:literal)*) => {$(
                game = game.add(
                    Phase::AfterPhysics,
                    link::<$from, $to>.access(SystemAccess::new().read::<Val<$from>>().write::<Val<$to>>()),
                );
            )*};
        }
        links!(0 1  1 2  2 3  3 4  4 5  5 6  6 7  7 0);
        measure(b, &name, game, 60, 400, |ctx| spawn(ctx, 16_000, 8));
    }

    // Producers and consumers registered in pairs, as code grouped by
    // feature often is: `p0 c0 p1 c1 ...`, where `ck` reads what `pk`
    // writes. Batching only neighbours gives five stages (`p0`, `c0 p1`,
    // `c1 p2`, `c2 p3`, `c3`); batching by dependencies gives two (every
    // producer, then every consumer).
    for mode in MODES {
        let name = format!("schedule/interleaved/4x16000/heavy/{mode}");
        if b.skips(&name) {
            continue;
        }
        let mut game = game(mode);
        macro_rules! pairs {
            ($($p:literal $c:literal)*) => {$(
                game = game
                    .add(Phase::AfterPhysics, heavy::<$p>.access(SystemAccess::new().write::<Val<$p>>()))
                    .add(
                        Phase::AfterPhysics,
                        link::<$p, $c>.access(SystemAccess::new().read::<Val<$p>>().write::<Val<$c>>()),
                    );
            )*};
        }
        pairs!(0 4  1 5  2 6  3 7);
        measure(b, &name, game, 60, 200, |ctx| spawn(ctx, 16_000, 8));
    }

    // A mixed simulation: one heavy system, a fan of four medium ones that
    // read what it wrote, two cheap ones, and a `Ctx` system in between.
    for mode in MODES {
        let name = format!("schedule/mixed/50000/{mode}");
        if b.skips(&name) {
            continue;
        }
        let read0 = || SystemAccess::new().read::<Val<0>>();
        let game = game(mode)
            .add(
                Phase::BeforePhysics,
                heavy::<0>.access(SystemAccess::new().write::<Val<0>>()),
            )
            .add(Phase::AfterPhysics, link::<0, 1>.access(read0().write::<Val<1>>()))
            .add(Phase::AfterPhysics, link::<0, 2>.access(read0().write::<Val<2>>()))
            .add(
                Phase::AfterPhysics,
                heavy::<3>.access(SystemAccess::new().write::<Val<3>>()),
            )
            .add(
                Phase::AfterPhysics,
                heavy::<4>.access(SystemAccess::new().write::<Val<4>>()),
            )
            .add(Phase::AfterPhysics, |ctx: &mut Ctx| {
                let n = ctx.world.query::<&Val<1>>().count();
                black_box(n);
            })
            .add(
                Phase::AfterPhysics,
                light::<5>.access(SystemAccess::new().write::<Val<5>>()),
            )
            .add(
                Phase::AfterPhysics,
                light::<6>.access(SystemAccess::new().write::<Val<6>>()),
            );
        measure(b, &name, game, 60, 200, |ctx| spawn(ctx, 50_000, 7));
    }

    // One big system next to seven tiny ones: the batch has almost nothing
    // to gain, so `auto` should stay on the game's thread.
    for mode in MODES {
        let name = format!("schedule/uneven/1x64000+7x100/{mode}");
        if b.skips(&name) {
            continue;
        }
        let mut game = game(mode).add(
            Phase::AfterPhysics,
            heavy::<0>.access(SystemAccess::new().write::<Val<0>>()),
        );
        macro_rules! small {
            ($($n:literal)*) => {$(
                game = game.add(Phase::AfterPhysics, light::<$n>.access(SystemAccess::new().write::<Val<$n>>()));
            )*};
        }
        small!(1 2 3 4 5 6 7);
        measure(b, &name, game, 60, 200, |ctx| {
            spawn(ctx, 64_000, 1);
            // The small systems' components live on 100 other entities.
            let few: Vec<Entity> = (0..100).map(|_| ctx.world.spawn((Row,))).collect();
            macro_rules! give {
                ($($n:literal)*) => {$(
                    for &e in &few {
                        ctx.world.insert(e, Val::<$n>(1.0, 1.0));
                    }
                )*};
            }
            give!(1 2 3 4 5 6 7);
        });
    }
}
