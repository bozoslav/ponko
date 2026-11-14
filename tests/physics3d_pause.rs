//! Holding only the 3D physics step (`ctx.physics3d.set_paused`): bodies,
//! characters and events stop, every other sim system keeps running, and a
//! save taken meanwhile holds the paused state.
#![cfg(feature = "physics3d")]

use std::cell::RefCell;
use std::rc::Rc;

use ponko::d3::{
    Character as Character3D, Collider as Collider3D, RigidBody as RigidBody3D, Transform as Transform3D,
    Velocity as Velocity3D,
};
use ponko::prelude::*;
use serde_json::Value;

const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

fn ball(at: Vec3) -> (Transform3D, RigidBody3D, Collider3D, Velocity3D) {
    (
        Transform3D::at(at),
        RigidBody3D::dynamic(),
        Collider3D::sphere(0.5),
        Velocity3D::default(),
    )
}

/// A 40 × 1 × 40 m slab whose top is at y = 0.
fn floor() -> (Transform3D, RigidBody3D, Collider3D) {
    (
        Transform3D::at(vec3(0.0, -0.5, 0.0)),
        RigidBody3D::fixed(),
        Collider3D::cuboid(vec3(40.0, 1.0, 40.0)),
    )
}

fn position(world: &World, e: Entity) -> Vec3 {
    world.get::<Transform3D>(e).unwrap().position
}

/// Holds 3D physics for ticks 10..30 from a sim system, which keeps running.
fn pause_menu(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    *ctx.world.resource_mut::<u64>() += 1;
    if tick == 10 {
        ctx.physics3d.set_paused(true);
    }
    if tick == 30 {
        ctx.physics3d.set_paused(false);
    }
}

/// What a sim system after the step saw each tick: the ball's height and
/// its velocity.
struct Seen(Vec<(f32, Vec3)>);

fn record(ctx: &mut Ctx) {
    let (y, v) = ctx
        .world
        .query::<(&Transform3D, &Velocity3D)>()
        .iter()
        .map(|(t, v)| (t.position.y, v.linear))
        .next()
        .unwrap();
    ctx.world.resource_mut::<Seen>().0.push((y, v));
}

#[test]
fn pausing_3d_physics_holds_bodies_while_sim_systems_keep_running() {
    let mut entity = None;
    let world = Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(pause_menu)
        .add(Phase::SimLast, |ctx: &mut Ctx| {
            assert_eq!(
                ctx.physics3d.is_paused(),
                (10..30).contains(&ctx.time.sim_tick()),
                "flag at tick {}",
                ctx.time.sim_tick()
            );
            record(ctx);
        })
        .run_headless(60, |ctx| {
            ctx.world.insert_resource(0_u64);
            ctx.world.insert_resource(Seen(Vec::new()));
            entity = Some(ctx.world.spawn(ball(vec3(0.0, 50.0, 0.0))));
        });
    assert_eq!(*world.resource::<u64>(), 60, "the sim system ran every tick");
    let seen = &world.resource::<Seen>().0;
    // Sim systems run after the step, so the pause set on tick 10 holds
    // ticks 11 to 30. The ball falls before, holds still with its speed
    // through the pause, and falls on after.
    assert!(seen[10].0 < seen[0].0);
    assert!(seen[10].1.y < 0.0, "falling at the pause: {:?}", seen[10]);
    assert!(seen[11..=30].iter().all(|s| *s == seen[10]), "{seen:?}");
    assert!(seen[31].0 < seen[30].0);

    // Twenty held ticks change nothing else: the ball ends where an
    // unpaused run puts it twenty ticks earlier.
    let mut free = None;
    let unpaused = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(40, |ctx| {
        free = Some(ctx.world.spawn(ball(vec3(0.0, 50.0, 0.0))));
    });
    assert_eq!(position(&world, entity.unwrap()), position(&unpaused, free.unwrap()));
}

#[test]
fn no_collision_events_arrive_while_3d_physics_is_paused() {
    // The ball starts inside the floor: any step would report the touch.
    let run = |paused: bool| {
        let mut entity = None;
        let world = Game::new("t", 1, 1)
            .physics3d(GRAVITY)
            .sim_system(|ctx: &mut Ctx| {
                let started = ctx.events.read::<CollisionStarted>().len() as u32;
                *ctx.world.resource_mut::<u32>() += started;
            })
            .run_headless(30, |ctx| {
                ctx.world.insert_resource(0_u32);
                ctx.physics3d.set_paused(paused);
                entity = Some(ctx.world.spawn(ball(vec3(0.0, 0.3, 0.0))));
                ctx.world.spawn(floor());
            });
        (*world.resource::<u32>(), position(&world, entity.unwrap()))
    };
    let (events, at) = run(true);
    assert_eq!((events, at), (0, vec3(0.0, 0.3, 0.0)), "held: no events, no movement");
    assert!(run(false).0 > 0, "the same scene unpaused reports the touch");
}

#[test]
fn a_character_paused_mid_jump_goes_on_where_it_stopped() {
    // Jumps on tick 1; paused on 10..30 (ticks 11 to 30 held).
    let run = |pause: bool| {
        let heights = Rc::new(RefCell::new(Vec::new()));
        let seen = heights.clone();
        Game::new("t", 1, 1)
            .physics3d(GRAVITY)
            .sim_system(move |ctx: &mut Ctx| {
                let tick = ctx.time.sim_tick();
                if pause && tick == 10 {
                    ctx.physics3d.set_paused(true);
                }
                if pause && tick == 30 {
                    ctx.physics3d.set_paused(false);
                }
                let mut query = ctx.world.query::<(&mut Character3D, &Transform3D)>();
                let (c, t) = query.iter().next().unwrap();
                if tick == 1 {
                    c.velocity.y = 6.0;
                }
                seen.borrow_mut().push((t.position.y, c.velocity.y));
            })
            .run_headless(80, |ctx| {
                ctx.world.spawn(floor());
                ctx.world
                    .spawn((Transform3D::at(vec3(0.0, 0.05, 0.0)), Character3D::new(1.8, 0.3)));
            });
        heights.take()
    };
    let held = run(true);
    let free = run(false);
    assert!(
        held[10].0 > held[1].0 && held[10].1 > 0.0,
        "rising at the pause: {:?}",
        held[10]
    );
    assert!(held[11..=30].iter().all(|s| *s == held[10]), "held mid-air: {held:?}");
    // After the pause the jump goes on exactly as if it never happened.
    for k in 11..60 {
        let (a, b) = (held[k + 20], free[k]);
        assert!(
            (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4,
            "tick {k}: {a:?} vs {b:?}"
        );
    }
}

#[test]
fn a_save_taken_while_3d_physics_is_paused_holds_the_paused_state() {
    #[derive(Default)]
    struct Out {
        save: Option<Value>,
        held: (Vec3, Vec3),
        after: Vec<Vec3>,
    }
    let out = Rc::new(RefCell::new(Out::default()));
    let slot = out.clone();
    let mut entity = None;
    Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| {
            let tick = ctx.time.sim_tick();
            let (p, v) = ctx
                .world
                .query::<(&Transform3D, &Velocity3D)>()
                .iter()
                .map(|(t, v)| (t.position, v.linear))
                .next()
                .unwrap();
            let mut out = slot.borrow_mut();
            match tick {
                10 => {
                    ctx.physics3d.set_paused(true);
                    out.held = (p, v);
                }
                20 => out.save = Some(ctx.world.save()),
                // Loading the save while paused keeps the pause and the pose.
                25 => {
                    let save = out.save.clone().unwrap();
                    ctx.world.load(&save).unwrap();
                    assert!(ctx.physics3d.is_paused(), "loading doesn't clear the flag");
                }
                30 => {
                    assert_eq!((p, v), out.held, "still held after loading");
                    ctx.physics3d.set_paused(false);
                }
                31.. => out.after.push(p),
                _ => {}
            }
        })
        .run_headless(50, |ctx| {
            entity = Some(ctx.world.spawn(ball(vec3(0.0, 50.0, 0.0))));
        });
    let out = out.take();
    let save = out.save.unwrap();
    let (held_at, held_speed) = out.held;
    assert!(held_speed.y < 0.0, "falling when paused");

    // The save holds the pose and speed the pause kept.
    let mut fresh = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(0, |_| {});
    fresh.load(&save).unwrap();
    let e = entity.unwrap();
    assert_eq!(position(&fresh, e), held_at);
    assert_eq!(fresh.get::<Velocity3D>(e).unwrap().linear, held_speed);

    // A game loaded from it falls on the same way the resumed one did.
    let mut loaded = Vec::new();
    let positions = Rc::new(RefCell::new(Vec::new()));
    let seen = positions.clone();
    Game::new("t", 1, 1)
        .physics3d(GRAVITY)
        .sim_system(move |ctx: &mut Ctx| seen.borrow_mut().push(position(&ctx.world, e)))
        .run_headless(out.after.len() as u64, |ctx| {
            ctx.world.load(&save).unwrap();
        });
    loaded.extend(positions.take());
    assert_eq!(loaded.len(), out.after.len());
    for (k, (a, b)) in out.after.iter().zip(&loaded).enumerate() {
        assert!(a.distance(*b) < 1e-4, "tick {k} after resuming: {a} vs {b}");
    }
}
