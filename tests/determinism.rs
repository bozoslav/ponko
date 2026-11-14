//! Same seed, same simulation: everything that could make runs diverge
//! (threads, the grid, sorting, reduced rates, spawning) at once.

use ponko::access::{Service, SystemAccess};
#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
use ponko::prelude::*;
use ponko::system_view::SystemView;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Component)]
struct Walker {
    heading: f32,
    energy: f32,
    crowded: u32,
}

#[derive(Serialize, Deserialize, Component)]
struct Food(f32);

fn wander(ctx: &mut Ctx) {
    let (dt, random, grid) = (ctx.time.dt(), ctx.random(), &ctx.spatial);
    ctx.world
        .query::<(Entity, &mut Transform2D, &mut Walker)>()
        .par_for_each(|(e, t, w)| {
            let mut rng = random.of(e);
            let mut near = 0;
            grid.for_each_in_radius(t.position, 8.0, |o| near += (o.entity != e) as u32);
            w.crowded = near;
            w.heading += rng.range(-0.5, 0.5) + near as f32 * 0.1;
            t.position += Vec2::from_angle(w.heading) * 20.0 * dt;
            w.energy -= dt;
        });
}

fn eat(ctx: &mut Ctx) {
    let turn = ctx.time.turns(3);
    let grid = &ctx.spatial;
    let food: Vec<(Entity, Vec2)> = ctx
        .world
        .query::<(Entity, &Transform2D, &Food)>()
        .iter()
        .map(|(e, t, _)| (e, t.position))
        .collect();
    let mut eaten = Vec::new();
    for (f, p) in food {
        if !turn.includes(f) {
            continue;
        }
        if let Some(w) = grid.nearest(p, 6.0, |o| o.entity != f) {
            eaten.push((f, w.entity));
        }
    }
    for (f, w) in eaten {
        if let Some(walker) = ctx.world.get_mut::<Walker>(w) {
            walker.energy += 1.0;
            ctx.commands.despawn(f);
        }
    }
}

fn breed_and_starve(ctx: &mut Ctx) {
    let mut rng = ctx.random().stream(0);
    let cmds = &mut ctx.commands;
    ctx.world
        .query::<(Entity, &Transform2D, &Walker)>()
        .for_each(|(e, t, w)| {
            if w.energy <= 0.0 {
                cmds.despawn(e);
            } else if w.energy > 2.0 && rng.chance(0.3) {
                cmds.spawn((
                    Transform2D::at(t.position),
                    Walker {
                        heading: rng.angle(),
                        energy: 1.0,
                        crowded: 0,
                    },
                ));
            }
        });
    for _ in 0..3 {
        let p = vec2(rng.range(0.0, 200.0), rng.range(0.0, 200.0));
        cmds.spawn((Transform2D::at(p), Food(1.0)));
    }
}

/// View systems: walkers tire at random, food ripens. They touch different
/// components, so the parallel scheduler puts them in one batch.
fn tire(sys: &mut SystemView) {
    let (dt, random) = (sys.dt(), sys.random());
    sys.world.query::<(Entity, &mut Walker)>().for_each(|(e, w)| {
        w.energy -= random.of(e).range(0.0, 0.2) * dt;
    });
}

fn ripen(sys: &mut SystemView) {
    let dt = sys.dt();
    sys.world.query::<&mut Food>().for_each(|f| f.0 += dt);
}

/// How a run's systems are scheduled.
#[derive(Clone, Copy)]
enum Scheduler {
    /// Only `&mut Ctx` systems, as before the scheduler existed.
    NoViews,
    /// With the view systems; the scheduler is as the environment says
    /// (off unless `PONKO_PARALLEL_SYSTEMS` turns it on).
    Views,
    /// With the view systems, the scheduler off or on with every batch on
    /// the thread pool, whatever the environment says.
    ViewsExactly { on: bool },
}

fn run(seed: u64, ticks: u64) -> ponko::reflect::json::Value {
    run_with(seed, ticks, Scheduler::NoViews)
}

fn run_with(seed: u64, ticks: u64, scheduler: Scheduler) -> ponko::reflect::json::Value {
    let mut game = Game::new("det", 1, 1)
        .seed(seed)
        .spatial_grid(8.0)
        .sim_system(wander)
        .sim_system(eat)
        .add(Phase::AfterPhysics, breed_and_starve.every(5));
    if !matches!(scheduler, Scheduler::NoViews) {
        let clock = || {
            SystemAccess::new()
                .read_service(Service::Time)
                .read_service(Service::Random)
        };
        game = game
            .add(Phase::SimLast, tire.access(clock().write::<Walker>()))
            .add(Phase::SimLast, ripen.access(clock().write::<Food>()));
    }
    if let Scheduler::ViewsExactly { on } = scheduler {
        game = game
            .parallel_systems(on)
            .parallel_threshold(std::time::Duration::ZERO)
            .parallel_systems_env(false);
    }
    if !matches!(scheduler, Scheduler::NoViews) {
        // The two view systems share a stage, so with the scheduler on
        // they really run as a batch.
        let plan = game.schedule_plan();
        assert_eq!(plan.batches().count(), 1, "{plan}");
        assert_eq!(plan.batches().next().unwrap().len, 2, "{plan}");
    }
    let world = game.run_headless(ticks, |ctx| {
        ctx.world.reflect::<Walker>();
        ctx.world.reflect::<Food>();
        let mut rng = Rng::new(1);
        ctx.world.spawn_batch((0..3000).map(|_| {
            let p = vec2(rng.range(0.0, 200.0), rng.range(0.0, 200.0));
            (
                Transform2D::at(p),
                Walker {
                    heading: rng.angle(),
                    energy: 2.0,
                    crowded: 0,
                },
            )
        }));
        ctx.world.spawn_batch((0..500).map(|_| {
            (
                Transform2D::at(vec2(rng.range(0.0, 200.0), rng.range(0.0, 200.0))),
                Food(1.0),
            )
        }));
    });
    world.save()
}

#[test]
fn the_same_seed_gives_the_same_world() {
    // 100 ticks: three spatial sorts, despawns and births through commands.
    let a = run(7, 100);
    let b = run(7, 100);
    assert!(
        a["entities"].as_array().unwrap().len() > 100,
        "the sim should still be alive"
    );
    assert_eq!(a.to_string(), b.to_string());
}

#[test]
fn a_different_seed_gives_a_different_world() {
    assert_ne!(run(7, 30).to_string(), run(8, 30).to_string());
}

#[test]
fn view_systems_give_the_same_world_with_the_scheduler_off_on_and_as_the_environment_says() {
    let off = run_with(7, 60, Scheduler::ViewsExactly { on: false }).to_string();
    // The view systems do something: this is not the world without them.
    assert_ne!(off, run(7, 60).to_string());
    assert_eq!(run_with(7, 60, Scheduler::ViewsExactly { on: true }).to_string(), off);
    // And however this test run was told to schedule (CI runs this file
    // once more with `PONKO_PARALLEL_SYSTEMS=always`).
    assert_eq!(run_with(7, 60, Scheduler::Views).to_string(), off);
}
