//! Benchmarks: ponko_ecs and the M3 spike ECS vs bevy_ecs vs hecs, plus
//! simulation at scale.
//!
//! Run with: cargo run --release -p ponko_bench [-- filter]
//! Each benchmark builds its state untimed, then times one operation; we report
//! the median and minimum over several runs.

mod actions;
mod alloc;
mod archetypes;
mod ecs_suite;
mod extraction;
#[cfg(feature = "render")]
mod lod_split;
mod membership;
mod nav3d;
mod net;
#[allow(dead_code)] // spike: API surface kept for the M3 port
mod own;
mod par_archetypes;
mod particles2d;
mod physics;
mod physics3d;
mod query_cache;
mod query_filters;
mod query_policy;
#[cfg(feature = "render")]
mod record;
#[allow(dead_code)] // the comparison is used by `render_compare`, the rest behind `render`
mod render_report;
#[cfg(feature = "render")]
mod render_scenes;
mod report;
mod schedule;
mod sparse_set;
mod sprites;
mod streaming;
mod text;
mod tilemap;

use bevy_ecs::component::Component;
use std::hint::black_box;
use std::time::{Duration, Instant};

/// Counts heap allocations, so every result reports them per timed run.
#[global_allocator]
static ALLOCATOR: alloc::Counting = alloc::Counting;

const N: usize = 1_000_000;
const ADD_REMOVE_N: usize = 100_000;
const DT: f32 = 1.0 / 60.0;

#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct Pos {
    x: f32,
    y: f32,
}
#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct Vel {
    x: f32,
    y: f32,
}
/// Stand-in for a sprite component: texture id + tint.
#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct Spr {
    _tex: u32,
    _tint: [f32; 4],
}
#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct Tag;
#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct M0;
#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct M1;
#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct M2;
#[derive(Component, Clone, Copy, Default, ponko_ecs::Component)]
struct M3;

fn ent(i: usize) -> (Pos, Vel, Spr) {
    (Pos { x: i as f32, y: 0.0 }, Vel { x: 1.0, y: 0.5 }, Spr::default())
}

/// Deterministic shuffled order for random-access benchmarks.
fn shuffled(n: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    let mut s = 0x9E3779B97F4A7C15u64;
    for i in (1..n).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.swap(i, (s % (i as u64 + 1)) as usize);
    }
    v
}

struct Bench {
    filter: Option<String>,
    records: Vec<report::Record>,
}

impl Bench {
    fn new(filter: Option<String>) -> Self {
        Bench {
            filter,
            records: Vec::new(),
        }
    }

    /// The filter is a substring of the names to run; `a|b` runs names
    /// containing either.
    fn skips(&self, name: &str) -> bool {
        self.filter.as_ref().is_some_and(|f| !matches_filter(f, name))
    }

    /// Builds the state once, then times `op` repeatedly on it (like a system
    /// running every frame). Use for iteration benchmarks.
    fn steady<S>(&mut self, name: &str, runs: usize, setup: impl FnOnce() -> S, op: impl FnMut(&mut S)) {
        self.steady_with_bytes(name, runs, setup, op, |_| None);
    }

    /// [`Bench::steady`], also recording `bytes(&state)` after the timed runs
    /// (the ECS world's storage size, where the library reports it).
    fn steady_with_bytes<S>(
        &mut self,
        name: &str,
        runs: usize,
        setup: impl FnOnce() -> S,
        mut op: impl FnMut(&mut S),
        bytes: impl FnOnce(&S) -> Option<usize>,
    ) {
        const WARMUP: usize = 5;
        if self.skips(name) {
            return;
        }
        let mut s = setup();
        for _ in 0..WARMUP {
            op(&mut s);
        }
        let mut times = Vec::with_capacity(runs);
        let allocs = alloc::Counts::now();
        for _ in 0..runs {
            let t = Instant::now();
            op(&mut s);
            times.push(t.elapsed());
        }
        let allocs = alloc::Counts::now().since(allocs);
        black_box(&s);
        let mut record = report::Record::new(name, report::Kind::Steady, WARMUP, &mut times);
        record.world_bytes = bytes(&s);
        record.set_allocs(allocs, runs);
        self.report(record);
    }

    fn report(&mut self, r: report::Record) {
        let bytes = r.world_bytes.map(|b| format!("   {:>8.1} KiB", b as f64 / 1024.0));
        let allocs = r
            .allocs_per_run
            .filter(|&a| a > 0.0)
            .map(|a| format!("   {a:>9.2} allocs/run"));
        println!(
            "{:<32} median {:>9.3} ms   p95 {:>9.3} ms   min {:>9.3} ms{}{}",
            r.name,
            ms(r.p50),
            ms(r.p95),
            ms(r.min),
            bytes.unwrap_or_default(),
            allocs.unwrap_or_default()
        );
        self.records.push(r);
    }

    fn run<S>(&mut self, name: &str, runs: usize, mut setup: impl FnMut() -> S, mut op: impl FnMut(&mut S)) {
        const WARMUP: usize = 1;
        if self.skips(name) {
            return;
        }
        let mut times = Vec::with_capacity(runs);
        let mut allocs = alloc::Counts::default();
        // Warmup runs, then measured runs.
        for i in 0..runs + WARMUP {
            let mut s = setup();
            let before = alloc::Counts::now();
            let t = Instant::now();
            op(&mut s);
            let el = t.elapsed();
            let counted = alloc::Counts::now().since(before);
            if i >= WARMUP {
                times.push(el);
                allocs.count += counted.count;
                allocs.bytes += counted.bytes;
            }
            drop(black_box(s));
        }
        let mut record = report::Record::new(name, report::Kind::Fresh, WARMUP, &mut times);
        record.set_allocs(allocs, runs);
        self.report(record);
    }
}

/// `name` contains one of the `|`-separated parts of `filter`.
fn matches_filter(filter: &str, name: &str) -> bool {
    filter.split('|').any(|part| name.contains(part))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() {
    let filter = std::env::args().nth(1);
    let mut b = Bench::new(filter.clone());
    // Before any benchmark runs, so the load average is the machine's, not ours.
    let env = std::env::var_os("PONKO_BENCH_JSON").map(|_| report::Env::detect(filter));
    println!("N = {N}, add/remove N = {ADD_REMOVE_N}\n");
    membership::run(b.filter.as_deref());
    physics::run(&mut b);
    physics3d::run(&mut b);
    nav3d::run(&mut b);
    extraction::run(&mut b);
    net::run(&mut b);
    actions::run(&mut b);
    query_cache::run(&mut b);
    query_filters::run(&mut b);
    query_policy::run(&mut b);
    par_archetypes::run(&mut b);
    #[cfg(feature = "render")]
    record::run(&mut b);
    #[cfg(feature = "render")]
    lod_split::run(&mut b);
    #[cfg(feature = "render")]
    let render = render_scenes::run(&mut b);
    #[cfg(not(feature = "render"))]
    let render: Option<String> = None;
    schedule::run(&mut b);
    streaming::run(&mut b);
    tilemap::run(&mut b);
    text::run(&mut b);
    sparse_set::run(&mut b);
    sprites::run(&mut b);
    particles2d::run(&mut b);

    // Resource scoping versus the previous public take/put workaround.
    for scoped in [false, true] {
        b.steady(
            if scoped {
                "resource/scope-100k"
            } else {
                "resource/take-put-100k"
            },
            200,
            || {
                let mut w = ponko_ecs::World::new();
                w.insert_resource(0u64);
                w
            },
            |w| {
                for _ in 0..100_000 {
                    if scoped {
                        w.resource_scope::<u64, _>(|world, value| {
                            *value = black_box(*value + 1);
                            black_box(world);
                        });
                    } else {
                        let mut value = w.remove_resource::<u64>().unwrap();
                        value = black_box(value + 1);
                        black_box(&mut *w);
                        w.insert_resource(value);
                    }
                }
            },
        );
    }

    // ---- borrowing several things at once ------------------------------------
    // Each pair is the workaround and its one-statement replacement.
    struct Speed(f32);
    struct Hits(u64);
    struct Sum(u64);
    for combined in [false, true] {
        b.steady(
            if combined {
                "borrow/resources-mut-100k"
            } else {
                "borrow/resources-take-put-100k"
            },
            200,
            || {
                let mut w = ponko_ecs::World::new();
                w.insert_resource(Speed(1.0));
                w.insert_resource(Hits(0));
                w.insert_resource(Sum(0));
                w
            },
            |w| {
                for _ in 0..100_000 {
                    if combined {
                        let (speed, hits, sum) = w.resources_mut::<(Speed, Hits, Sum)>();
                        hits.0 += black_box(speed.0) as u64;
                        sum.0 += hits.0;
                    } else {
                        let mut hits = w.remove_resource::<Hits>().unwrap();
                        let mut sum = w.remove_resource::<Sum>().unwrap();
                        hits.0 += black_box(w.resource::<Speed>().0) as u64;
                        sum.0 += hits.0;
                        w.insert_resource(hits);
                        w.insert_resource(sum);
                    }
                }
            },
        );
    }
    // A query that also needs resources: copied out before and written back
    // after, or carried into the loop.
    for carried in [false, true] {
        b.steady(
            if carried {
                "iter/ponko-with-resources"
            } else {
                "iter/ponko-copy-out"
            },
            200,
            || {
                let mut w = ponko_ecs::World::new();
                w.spawn_batch((0..N).map(ent));
                w.insert_resource(Speed(DT));
                w.insert_resource(Hits(0));
                w
            },
            |w| {
                if carried {
                    w.query::<(&mut Pos, &Vel)>()
                        .with_resources::<(&Speed, &mut Hits)>()
                        .for_each(|(p, v), (speed, hits)| {
                            p.x += v.x * speed.0;
                            p.y += v.y * speed.0;
                            hits.0 += 1;
                        });
                } else {
                    let dt = black_box(w.resource::<Speed>().0);
                    let mut hits = w.resource::<Hits>().0;
                    w.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
                        p.x += v.x * dt;
                        p.y += v.y * dt;
                        hits += 1;
                    });
                    w.resource_mut::<Hits>().0 = hits;
                }
            },
        );
    }
    // 100k entities each change another one: collected and applied after the
    // loop, or reached through a lookup inside it.
    #[derive(ponko_ecs::Component)]
    struct Aim(ponko_ecs::Entity);
    #[derive(ponko_ecs::Component)]
    struct Hp(i64);
    #[derive(ponko_ecs::Component)]
    struct Damage(i64);
    for lookup in [false, true] {
        b.steady(
            if lookup {
                "borrow/lookup-100k"
            } else {
                "borrow/collect-then-mutate-100k"
            },
            200,
            || {
                let mut w = ponko_ecs::World::new();
                let targets: Vec<_> = (0..1000).map(|_| w.spawn((Hp(0),))).collect();
                let order = shuffled(ADD_REMOVE_N);
                w.spawn_batch(order.iter().map(|&i| (Aim(targets[i % targets.len()]), Damage(1))));
                w
            },
            |w| {
                if lookup {
                    w.query::<(&Aim, &Damage)>()
                        .with_lookup::<&mut Hp>()
                        .for_each(|(aim, damage), others| {
                            if let Some(hp) = others.get_mut(aim.0) {
                                hp.0 -= damage.0;
                            }
                        });
                } else {
                    let mut hits = Vec::new();
                    w.query::<(&Aim, &Damage)>()
                        .for_each(|(aim, damage)| hits.push((aim.0, damage.0)));
                    for (target, damage) in hits {
                        if let Some(hp) = w.get_mut::<Hp>(target) {
                            hp.0 -= damage;
                        }
                    }
                }
            },
        );
    }
    b.steady(
        "borrow/get-many-mut-pairs-100k",
        200,
        || {
            let mut w = ponko_ecs::World::new();
            let es: Vec<_> = (0..1000).map(|i| w.spawn((Hp(i),))).collect();
            (w, es)
        },
        |(w, es)| {
            for i in 0..100_000 {
                let pair = [es[i % 1000], es[(i + 1) % 1000]];
                if let Some([a, b]) = w.get_many_mut::<Hp, 2>(pair) {
                    std::mem::swap(&mut a.0, &mut b.0);
                }
            }
        },
    );

    // ---- baseline: plain Vecs (upper bound for iteration) -------------------
    b.steady(
        "iter/raw-vec",
        200,
        // Non-zero data: zeroed vecs get lazily mapped pages that fault on first touch.
        || {
            (
                (0..N).map(|i| ent(i).0).collect::<Vec<_>>(),
                vec![Vel { x: 1.0, y: 0.5 }; N],
            )
        },
        |(p, v)| {
            let dt = black_box(DT);
            for (p, v) in p.iter_mut().zip(v.iter()) {
                p.x += v.x * dt;
                p.y += v.y * dt;
            }
        },
    );

    b.steady(
        "iter/raw-vec-grown",
        200,
        // Same data, but grown by push like ECS columns (realloc'd buffers).
        || {
            let (mut p, mut v) = (Vec::new(), Vec::new());
            for i in 0..N {
                p.push(ent(i).0);
                v.push(ent(i).1);
            }
            (p, v)
        },
        |(p, v)| {
            let dt = black_box(DT);
            for (p, v) in p.iter_mut().zip(v.iter()) {
                p.x += v.x * dt;
                p.y += v.y * dt;
            }
        },
    );

    // ---- simulation at scale: single core vs all cores (ponko_ecs) ----------
    /// Agent with heading; "heavy" work = steering with trig, like a real sim tick.
    #[derive(Clone, Copy, ponko_ecs::Component)]
    struct Agent {
        heading: f32,
        speed: f32,
    }
    fn heavy(p: &mut Pos, a: &mut Agent, dt: f32) {
        let wander = (p.x * 0.013 + p.y * 0.007 + a.heading).sin() * 0.8;
        a.heading += wander * dt;
        let (s, c) = a.heading.sin_cos();
        p.x = (p.x + c * a.speed * dt).rem_euclid(4096.0);
        p.y = (p.y + s * a.speed * dt).rem_euclid(4096.0);
    }
    for &n in &[1_000_000usize, 10_000_000] {
        let world = |n: usize| {
            let mut w = ponko_ecs::World::new();
            w.spawn_batch((0..n).map(|i| {
                (
                    Pos {
                        x: (i % 4096) as f32,
                        y: (i / 4096 % 4096) as f32,
                    },
                    Vel { x: 1.0, y: 0.5 },
                    Agent {
                        heading: i as f32,
                        speed: 30.0,
                    },
                )
            }));
            w
        };
        let tag = if n >= 10_000_000 { "10M" } else { "1M" };
        let runs = if n >= 10_000_000 { 30 } else { 100 };
        b.steady(
            &format!("sim-light-{tag}/1-core"),
            runs,
            || world(n),
            |w| {
                let dt = black_box(DT);
                w.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
                    p.x += v.x * dt;
                    p.y += v.y * dt;
                });
            },
        );
        b.steady(
            &format!("sim-light-{tag}/all-cores"),
            runs,
            || world(n),
            |w| {
                let dt = black_box(DT);
                w.query::<(&mut Pos, &Vel)>().par_for_each(|(p, v)| {
                    p.x += v.x * dt;
                    p.y += v.y * dt;
                });
            },
        );
        b.steady(
            &format!("sim-heavy-{tag}/1-core"),
            runs,
            || world(n),
            |w| {
                let dt = black_box(DT);
                w.query::<(&mut Pos, &mut Agent)>().for_each(|(p, a)| heavy(p, a, dt));
            },
        );
        b.steady(
            &format!("sim-heavy-{tag}/all-cores"),
            runs,
            || world(n),
            |w| {
                let dt = black_box(DT);
                w.query::<(&mut Pos, &mut Agent)>()
                    .par_for_each(|(p, a)| heavy(p, a, dt));
            },
        );
    }

    // Reuse output capacity; cover both trivially dropped and owning results.
    for &n in &[256usize, 1_000_000] {
        for &owning in &[false, true] {
            let kind = if owning { "owning" } else { "plain" };
            b.steady(
                &format!("par-collect-{n}/{kind}"),
                200,
                || {
                    let mut w = ponko_ecs::World::new();
                    w.spawn_batch((0..n).map(ent));
                    (w, Vec::with_capacity(n), Vec::with_capacity(n))
                },
                |(w, plain, owned)| {
                    if owning {
                        // A nontrivial destructor without per-element allocations.
                        struct Owned(f32);
                        impl Drop for Owned {
                            fn drop(&mut self) {
                                black_box(self.0);
                            }
                        }
                        w.query::<&Pos>()
                            .par_collect_into(owned, |p| Owned(p.x * black_box(2.0)));
                    } else {
                        w.query::<&Pos>().par_collect_into(plain, |p| p.x * black_box(2.0));
                    }
                },
            );
        }
    }

    // ---- spawn ------------------------------------------------------------------
    b.run("spawn/own", 10, own::World::new, |w| {
        for i in 0..N {
            w.spawn(ent(i));
        }
    });
    b.run("spawn/own-batch", 10, own::World::new, |w| {
        w.spawn_batch((0..N).map(ent))
    });
    b.run("spawn/ponko", 10, ponko_ecs::World::new, |w| {
        for i in 0..N {
            w.spawn(ent(i));
        }
    });
    b.run("spawn/ponko-batch", 10, ponko_ecs::World::new, |w| {
        w.spawn_batch((0..N).map(ent))
    });
    b.run("spawn/hecs", 10, hecs::World::new, |w| {
        for i in 0..N {
            w.spawn(ent(i));
        }
    });
    b.run("spawn/hecs-batch", 10, hecs::World::new, |w| {
        w.spawn_batch((0..N).map(ent));
    });
    b.run("spawn/bevy", 10, bevy_ecs::world::World::new, |w| {
        for i in 0..N {
            w.spawn(ent(i));
        }
    });
    b.run("spawn/bevy-batch", 10, bevy_ecs::world::World::new, |w| {
        w.spawn_batch((0..N).map(ent));
    });

    // ---- deferred commands: record, then apply (one reused Commands) ---------
    // A system that queues a component change for many entities every frame.
    b.steady(
        "commands/insert-remove",
        50,
        || {
            let mut w = ponko_ecs::World::new();
            let es: Vec<_> = (0..ADD_REMOVE_N).map(|i| w.spawn(ent(i))).collect();
            (w, es, ponko_ecs::Commands::new())
        },
        |(w, es, cmd)| {
            for &e in es.iter() {
                cmd.insert(e, Tag);
            }
            w.apply(cmd);
            for &e in es.iter() {
                cmd.remove::<Tag>(e);
            }
            w.apply(cmd);
        },
    );
    // Spawns and despawns through commands: pooled particles, streamed cells.
    b.steady(
        "commands/spawn-despawn",
        50,
        || (ponko_ecs::World::new(), Vec::new(), ponko_ecs::Commands::new()),
        |(w, es, cmd)| {
            for i in 0..ADD_REMOVE_N {
                cmd.spawn(ent(i));
            }
            w.apply(cmd);
            es.clear();
            w.query::<ponko_ecs::Entity>().for_each(|e| es.push(e));
            for &e in es.iter() {
                cmd.despawn(e);
            }
            w.apply(cmd);
        },
    );

    // ---- iterate: pos += vel * dt over 1M entities, one archetype -------------
    b.steady(
        "iter/own",
        200,
        || {
            let mut w = own::World::new();
            w.spawn_batch((0..N).map(ent));
            w
        },
        |w| {
            let dt = black_box(DT);
            w.for_each::<(&mut Pos, &Vel)>(|(p, v)| {
                p.x += v.x * dt;
                p.y += v.y * dt;
            });
        },
    );
    b.steady(
        "iter/ponko",
        200,
        || {
            let mut w = ponko_ecs::World::new();
            w.spawn_batch((0..N).map(ent));
            w
        },
        |w| {
            let dt = black_box(DT);
            w.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
                p.x += v.x * dt;
                p.y += v.y * dt;
            });
        },
    );
    // Change detection (opt-in): what tracking one component costs. The
    // `untracked` entries are the same loops without it, for the same run.
    for (name, tracked) in [
        ("changes/ponko-iter-mut-untracked", false),
        ("changes/ponko-iter-mut-tracked", true),
    ] {
        b.steady_with_bytes(
            name,
            200,
            || {
                let mut w = ponko_ecs::World::new();
                if tracked {
                    w.track_changes::<Pos>();
                }
                w.spawn_batch((0..N).map(ent));
                w
            },
            |w| {
                let dt = black_box(DT);
                w.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
                    p.x += v.x * dt;
                    p.y += v.y * dt;
                });
            },
            |w| Some(w.allocated_bytes()),
        );
    }
    // Reading a tracked component marks nothing.
    for (name, tracked) in [
        ("changes/ponko-iter-read-untracked", false),
        ("changes/ponko-iter-read-tracked", true),
    ] {
        b.steady(
            name,
            200,
            || {
                let mut w = ponko_ecs::World::new();
                if tracked {
                    w.track_changes::<Pos>();
                }
                w.spawn_batch((0..N).map(ent));
                w
            },
            |w| {
                let mut sum = 0.0;
                w.query::<(&Pos, &Vel)>().for_each(|(p, v)| sum += p.x + v.x);
                black_box(sum);
            },
        );
    }
    // A `Changed` filter over a world where nothing, 1% or everything changed
    // since the reader last looked.
    for (name, every) in [
        ("changes/ponko-filter-0pct", 0),
        ("changes/ponko-filter-1pct", 100),
        ("changes/ponko-filter-100pct", 1),
    ] {
        b.steady(
            name,
            200,
            || {
                let mut w = ponko_ecs::World::new();
                w.track_changes::<Pos>();
                let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
                let seen = w.advance_change_tick();
                if every > 0 {
                    for e in es.iter().step_by(every) {
                        w.mark_changed::<Pos>(*e);
                    }
                }
                (w, seen)
            },
            |(w, seen)| {
                let mut sum = 0.0;
                w.query::<&Pos>()
                    .filter::<ponko_ecs::Changed<Pos>>()
                    .since(*seen)
                    .for_each(|p| sum += p.x);
                black_box(sum);
            },
        );
    }
    b.run(
        "changes/ponko-spawn-tracked",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            w.track_changes::<Pos>();
            w
        },
        |w| {
            for i in 0..N {
                w.spawn(ent(i));
            }
        },
    );
    b.run(
        "changes/ponko-spawn-batch-tracked",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            w.track_changes::<Pos>();
            w
        },
        |w| w.spawn_batch((0..N).map(ent)),
    );
    b.run(
        "changes/ponko-get-mut-random-tracked",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            w.track_changes::<Pos>();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            let es: Vec<_> = shuffled(N).iter().map(|&i| es[i]).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.get_mut::<Pos>(e).unwrap().x += 1.0;
            }
        },
    );
    b.run(
        "changes/ponko-get-mut-random-untracked",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            let es: Vec<_> = shuffled(N).iter().map(|&i| es[i]).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.get_mut::<Pos>(e).unwrap().x += 1.0;
            }
        },
    );
    // Moving rows between archetypes carries the stamps of `Pos`; the tracked
    // `Tag` also logs 100k removals.
    b.run(
        "changes/ponko-add-remove-tracked",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            w.track_changes::<Pos>();
            w.track_changes::<Tag>();
            let es: Vec<_> = (0..ADD_REMOVE_N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.insert(e, Tag);
            }
            for &e in es.iter() {
                w.remove::<Tag>(e);
            }
        },
    );
    b.run(
        "changes/ponko-despawn-tracked",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            w.track_changes::<Pos>();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.despawn(e);
            }
        },
    );
    b.steady(
        "iter/ponko-iter",
        200,
        || {
            let mut w = ponko_ecs::World::new();
            w.spawn_batch((0..N).map(ent));
            w
        },
        |w| {
            let dt = black_box(DT);
            for (p, v) in w.query::<(&mut Pos, &Vel)>().iter() {
                p.x += v.x * dt;
                p.y += v.y * dt;
            }
        },
    );
    b.steady(
        "iter/own-dynamic-chunk",
        200,
        || {
            // Components registered by name/size/align only, as the C API would.
            let mut w = own::World::new();
            let p = w.register_dynamic("Pos", 8, 4, None);
            let v = w.register_dynamic("Vel", 8, 4, None);
            let s = w.register_dynamic("Spr", 20, 4, None);
            for i in 0..N {
                let (pp, vv, ss) = ent(i);
                unsafe {
                    w.spawn_dynamic(&[
                        (p, (&pp as *const Pos).cast()),
                        (v, (&vv as *const Vel).cast()),
                        (s, (&ss as *const Spr).cast()),
                    ]);
                }
            }
            (w, [p, v])
        },
        |(w, ids)| {
            let dt = black_box(DT);
            w.for_each_chunk(ids, |len, cols| {
                // What a C++ system does: cast the columns, run a plain loop.
                let pos = unsafe { std::slice::from_raw_parts_mut(cols[0].cast::<[f32; 2]>(), len) };
                let vel = unsafe { std::slice::from_raw_parts(cols[1].cast::<[f32; 2]>(), len) };
                for (p, v) in pos.iter_mut().zip(vel) {
                    p[0] += v[0] * dt;
                    p[1] += v[1] * dt;
                }
            });
        },
    );
    b.steady(
        "iter/hecs",
        200,
        || {
            let mut w = hecs::World::new();
            w.spawn_batch((0..N).map(ent));
            w
        },
        |w| {
            let dt = black_box(DT);
            for (p, v) in w.query_mut::<(&mut Pos, &Vel)>() {
                p.x += v.x * dt;
                p.y += v.y * dt;
            }
        },
    );
    b.steady(
        "iter/bevy",
        200,
        || {
            let mut w = bevy_ecs::world::World::new();
            w.spawn_batch((0..N).map(ent));
            let q = w.query::<(&mut Pos, &Vel)>();
            (w, q)
        },
        |(w, q)| {
            let dt = black_box(DT);
            q.iter_mut(w).for_each(|(mut p, v)| {
                p.x += v.x * dt;
                p.y += v.y * dt;
            });
        },
    );

    // ---- fragmented iterate: same 1M spread over 16 archetypes ----------------
    fn markers(i: usize) -> [bool; 4] {
        [i & 1 != 0, i & 2 != 0, i & 4 != 0, i & 8 != 0]
    }
    b.steady(
        "iter-frag16/own",
        200,
        || {
            let mut w = own::World::new();
            for i in 0..N {
                let e = w.spawn(ent(i));
                let m = markers(i);
                if m[0] {
                    w.insert(e, M0);
                }
                if m[1] {
                    w.insert(e, M1);
                }
                if m[2] {
                    w.insert(e, M2);
                }
                if m[3] {
                    w.insert(e, M3);
                }
            }
            w
        },
        |w| {
            let dt = black_box(DT);
            w.for_each::<(&mut Pos, &Vel)>(|(p, v)| {
                p.x += v.x * dt;
                p.y += v.y * dt;
            });
        },
    );
    b.steady(
        "iter-frag16/hecs",
        200,
        || {
            let mut w = hecs::World::new();
            for i in 0..N {
                let e = w.spawn(ent(i));
                let m = markers(i);
                if m[0] {
                    w.insert_one(e, M0).unwrap();
                }
                if m[1] {
                    w.insert_one(e, M1).unwrap();
                }
                if m[2] {
                    w.insert_one(e, M2).unwrap();
                }
                if m[3] {
                    w.insert_one(e, M3).unwrap();
                }
            }
            w
        },
        |w| {
            let dt = black_box(DT);
            for (p, v) in w.query_mut::<(&mut Pos, &Vel)>() {
                p.x += v.x * dt;
                p.y += v.y * dt;
            }
        },
    );
    b.steady(
        "iter-frag16/bevy",
        200,
        || {
            let mut w = bevy_ecs::world::World::new();
            for i in 0..N {
                let mut e = w.spawn(ent(i));
                let m = markers(i);
                if m[0] {
                    e.insert(M0);
                }
                if m[1] {
                    e.insert(M1);
                }
                if m[2] {
                    e.insert(M2);
                }
                if m[3] {
                    e.insert(M3);
                }
            }
            let q = w.query::<(&mut Pos, &Vel)>();
            (w, q)
        },
        |(w, q)| {
            let dt = black_box(DT);
            q.iter_mut(w).for_each(|(mut p, v)| {
                p.x += v.x * dt;
                p.y += v.y * dt;
            });
        },
    );

    // ---- random access: get<Pos> for 1M entities in shuffled order ----------------
    let order = shuffled(N);
    b.run(
        "get-random/own",
        10,
        || {
            let mut w = own::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            let es: Vec<_> = order.iter().map(|&i| es[i]).collect();
            (w, es)
        },
        |(w, es)| {
            let mut s = 0.0;
            for &e in es.iter() {
                s += w.get::<Pos>(e).unwrap().x;
            }
            black_box(s);
        },
    );
    b.run(
        "get-random/ponko",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            let es: Vec<_> = order.iter().map(|&i| es[i]).collect();
            (w, es)
        },
        |(w, es)| {
            let mut s = 0.0;
            for &e in es.iter() {
                s += w.get::<Pos>(e).unwrap().x;
            }
            black_box(s);
        },
    );
    b.run(
        "get-random/hecs",
        10,
        || {
            let mut w = hecs::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            let es: Vec<_> = order.iter().map(|&i| es[i]).collect();
            (w, es)
        },
        |(w, es)| {
            let mut s = 0.0;
            for &e in es.iter() {
                s += w.get::<&Pos>(e).unwrap().x;
            }
            black_box(s);
        },
    );
    b.run(
        "get-random/bevy",
        10,
        || {
            let mut w = bevy_ecs::world::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i)).id()).collect();
            let es: Vec<_> = order.iter().map(|&i| es[i]).collect();
            (w, es)
        },
        |(w, es)| {
            let mut s = 0.0;
            for &e in es.iter() {
                s += w.get::<Pos>(e).unwrap().x;
            }
            black_box(s);
        },
    );

    // ---- add + remove a component on 100k entities ----------------------------
    b.run(
        "add-remove/own",
        10,
        || {
            let mut w = own::World::new();
            let es: Vec<_> = (0..ADD_REMOVE_N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.insert(e, Tag);
            }
            for &e in es.iter() {
                w.remove::<Tag>(e);
            }
        },
    );
    b.run(
        "add-remove/ponko",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            let es: Vec<_> = (0..ADD_REMOVE_N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.insert(e, Tag);
            }
            for &e in es.iter() {
                w.remove::<Tag>(e);
            }
        },
    );
    b.run(
        "add-remove/hecs",
        10,
        || {
            let mut w = hecs::World::new();
            let es: Vec<_> = (0..ADD_REMOVE_N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.insert_one(e, Tag).unwrap();
            }
            for &e in es.iter() {
                w.remove_one::<Tag>(e).unwrap();
            }
        },
    );
    b.run(
        "add-remove/bevy",
        10,
        || {
            let mut w = bevy_ecs::world::World::new();
            let es: Vec<_> = (0..ADD_REMOVE_N).map(|i| w.spawn(ent(i)).id()).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.entity_mut(e).insert(Tag);
            }
            for &e in es.iter() {
                w.entity_mut(e).remove::<Tag>();
            }
        },
    );

    // ---- despawn 1M -----------------------------------------------------------------
    b.run(
        "despawn/own",
        10,
        || {
            let mut w = own::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.despawn(e);
            }
        },
    );
    b.run(
        "despawn/ponko",
        10,
        || {
            let mut w = ponko_ecs::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.despawn(e);
            }
        },
    );
    b.run(
        "despawn/hecs",
        10,
        || {
            let mut w = hecs::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i))).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.despawn(e).unwrap();
            }
        },
    );
    b.run(
        "despawn/bevy",
        10,
        || {
            let mut w = bevy_ecs::world::World::new();
            let es: Vec<_> = (0..N).map(|i| w.spawn(ent(i)).id()).collect();
            (w, es)
        },
        |(w, es)| {
            for &e in es.iter() {
                w.despawn(e);
            }
        },
    );

    archetypes::run(&mut b);
    ecs_suite::run(&mut b);
    flow_benches(&mut b);

    if let Some(path) = std::env::var_os("PONKO_BENCH_JSON") {
        let json = report::to_json(env.as_ref().expect("detected at start"), &b.records, render.as_deref());
        match std::fs::write(&path, json) {
            Ok(()) => println!("\n{} results written to {}", b.records.len(), path.to_string_lossy()),
            Err(e) => {
                eprintln!("could not write {}: {e}", path.to_string_lossy());
                std::process::exit(1);
            }
        }
    }
}

/// A `side` × `side` map with about 20% of cells walls (blobs, like terrain).
fn flow_map(side: u32) -> ponko::flow::NavGrid {
    let mut nav = ponko::flow::NavGrid::new(side, side, 1.0);
    let mut s = 0x2545F4914F6CDD1Du64;
    for _ in 0..(side * side / 60) {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let (cx, cy) = ((s % side as u64) as i64, ((s >> 32) % side as u64) as i64);
        for dy in -3..=3i64 {
            for dx in -3..=3i64 {
                if dx * dx + dy * dy <= 9 && cx + dx >= 0 && cy + dy >= 0 {
                    nav.set_cost((cx + dx) as u32, (cy + dy) as u32, ponko::flow::NavGrid::WALL);
                }
            }
        }
    }
    nav
}

fn flow_benches(b: &mut Bench) {
    use ponko::flow::FlowField;
    use ponko::prelude::Vec2;
    use rayon::prelude::*;
    for side in [256u32, 1024] {
        b.steady(
            &format!("flow/compute-{side}"),
            20,
            || (flow_map(side), FlowField::new()),
            |(nav, flow)| {
                let mut goal = Vec2::splat(side as f32 / 2.0);
                while !nav.walkable(goal) {
                    goal.x += 1.0;
                }
                flow.compute(nav, &[goal]);
            },
        );
    }
    b.steady(
        "flow/lookup-1M-par",
        20,
        || {
            let nav = flow_map(1024);
            let mut flow = FlowField::new();
            let mut goal = Vec2::splat(512.0);
            while !nav.walkable(goal) {
                goal.x += 1.0;
            }
            flow.compute(&nav, &[goal]);
            let mut s = 1u64;
            let agents: Vec<Vec2> = (0..1_000_000)
                .map(|_| {
                    s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    Vec2::new((s >> 40) as f32 / 16384.0, ((s >> 16) & 0xffffff) as f32 / 16384.0)
                })
                .collect();
            (flow, agents)
        },
        |(flow, agents)| {
            agents.par_iter_mut().for_each(|p| *p += flow.direction_at(*p) * 0.1);
        },
    );

    // ---- attachments: 100k entities following 100k others ----------------------
    // Spawning is the same in both; the difference is 10 ticks of following.
    for ticks in [0u64, 10] {
        b.steady(
            &format!("attach/100k-spawn+{ticks}-ticks"),
            20,
            || (),
            |_| {
                use ponko::d2::Transform as Transform2D;
                use ponko::systems::follow_attachments;
                use ponko::{
                    d2::Attached,
                    prelude::{Game, vec2},
                };
                let world = Game::new("bench", 1, 1)
                    .sim_system(|ctx| {
                        ctx.world
                            .query::<&mut Transform2D>()
                            .par_for_each(|t| t.position.x += 1.0);
                    })
                    .sim_system(follow_attachments)
                    .run_headless(ticks, |ctx| {
                        for i in 0..100_000 {
                            let base = ctx.world.spawn((Transform2D::at(vec2(i as f32, 0.0)),));
                            ctx.world
                                .spawn((Transform2D::default(), Attached::to(base).offset(vec2(0.0, 1.0))));
                        }
                    });
                black_box(world);
            },
        );
    }
    // ---- events: 100k sent and read per tick, 10 ticks ------------------------
    #[derive(Clone, Copy)]
    struct Hit(#[allow(dead_code)] ponko::prelude::Entity, f32);
    b.steady(
        "events/100k-per-tick-x10",
        20,
        || (),
        |_| {
            let world = ponko::prelude::Game::new("bench", 1, 1)
                .sim_system(|ctx| {
                    for i in 0..100_000u32 {
                        ctx.events
                            .send(Hit(ponko::prelude::Entity::from_bits(u64::from(i)), i as f32));
                    }
                })
                .sim_system(|ctx| {
                    let total: f32 = ctx.events.read::<Hit>().iter().map(|h| h.1).sum();
                    black_box(total);
                })
                .run_headless(10, |_| {});
            black_box(world);
        },
    );
    // ---- the headless loop itself: 10k ticks of a game that does nothing --------
    b.steady(
        "headless/sim-only-10k-ticks",
        20,
        || (),
        |_| {
            let world = ponko::prelude::Game::new("bench", 1, 1)
                .sim_system(|ctx| {
                    black_box(ctx.time.sim_tick());
                })
                .run_headless(10_000, |_| {});
            black_box(world);
        },
    );
    // The same with frames (`headless_frames`): the UI and the frame systems
    // run too, so this is what a menu test pays per frame.
    b.steady(
        "headless/frames-10k-ticks",
        20,
        || (),
        |_| {
            let world = ponko::prelude::Game::new("bench", 1, 1)
                .headless_frames(true)
                .sim_system(|ctx| {
                    black_box(ctx.time.sim_tick());
                })
                .run_headless(10_000, |_| {});
            black_box(world);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::matches_filter;

    #[test]
    fn filters_match_any_alternative() {
        assert!(matches_filter("iter", "ecs/iter-1k/ponko"));
        assert!(matches_filter("ecs/|archetypes-1024", "archetypes-1024/hecs"));
        assert!(matches_filter("ecs/|archetypes-1024", "ecs/game-10k/bevy"));
        assert!(!matches_filter("ecs/|archetypes-1024", "spawn/hecs"));
    }
}
