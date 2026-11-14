//! What 3D physics allocates per tick once a scene is running. rapier's own
//! solver (and its character controller) allocate a few small buffers per
//! step, which the engine can't avoid; the engine's part (syncing
//! components, events, queries, vehicles, characters) must add nothing.
//!
//! The counter is thread-local: only allocations made by the thread that is
//! measuring count. A process-wide counter also saw other threads (libtest's
//! main thread recording the previous test's result, a logger, a worker
//! waking up), and these comparisons have zero tolerance, so one stray
//! allocation failed the run now and then. This is sound here because the
//! engine's 3D physics runs on the calling thread: rapier's `parallel`
//! feature is off, and nothing in these scenes uses the character
//! controller's parallel path. A future parallel path would need a different
//! approach, such as a process-wide count in a binary with a single test (see
//! `tests/alloc_counting.rs`).
#![cfg(feature = "physics3d")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Mutex;

use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform as Transform3D;
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::{
    Character as Character3D, Collider as Collider3D, QueryFilter as QueryFilter3D, RigidBody as RigidBody3D,
    Vehicle as Vehicle3D, Wheel as Wheel3D,
};
#[allow(unused_imports)]
use ponko::net::Pose;
use ponko::prelude::*;
use rapier3d::prelude as rp;

struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// Counts one allocation if this thread is measuring. `try_with`: a thread
/// that is shutting down has no counter any more.
fn record_allocation() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
    }
}

// SAFETY: forwards to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: the caller's contract, passed on.
        unsafe { System.alloc(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        // SAFETY: the caller's contract, passed on.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Starts counting this thread's allocations from zero.
fn start_counting() {
    ALLOCATIONS.set(0);
    COUNTING.set(true);
}
/// Stops counting and returns this thread's count since `start_counting`.
fn stop_counting() -> usize {
    COUNTING.set(false);
    ALLOCATIONS.get()
}

const WARMUP: u64 = 60;
const MEASURED: u64 = 60;
const BOXES: usize = 200;

fn box_at(i: usize) -> Vec3 {
    vec3(
        (i % 10) as f32 * 1.2 - 6.0,
        1.0 + (i / 100) as f32 * 1.5,
        (i / 10 % 10) as f32 * 1.2 - 6.0,
    )
}

/// Collects events into a reused buffer, as the engine does.
struct Collector(Mutex<Vec<rp::CollisionEvent>>);
impl rp::EventHandler for Collector {
    fn handle_collision_event(
        &self,
        _: &rp::RigidBodySet,
        _: &rp::ColliderSet,
        e: rp::CollisionEvent,
        _: Option<&rp::ContactPair>,
    ) {
        self.0.lock().unwrap().push(e);
    }
    fn handle_contact_force_event(
        &self,
        _: f32,
        _: &rp::RigidBodySet,
        _: &rp::ColliderSet,
        _: &rp::ContactPair,
        _: f32,
    ) {
    }
    fn handle_soft_body_tear_event(&self, _: &rp::SoftBodySet, _: &rp::SoftBodyTearEvent) {}
}

/// The same scene stepped by rapier alone.
fn rapier_alone() -> usize {
    let mut w = rp::PhysicsWorld::new();
    w.integration_parameters.dt = 1.0 / ponko::d3::Physics::HZ as f32;
    w.gravity = rp::Vector::new(0.0, -9.81, 0.0);
    let ground = w.insert_body(rp::RigidBodyBuilder::fixed().translation(rp::Vector::new(0.0, -0.5, 0.0)));
    w.insert_collider(
        rp::ColliderBuilder::cuboid(20.0, 0.5, 20.0).active_events(rp::ActiveEvents::COLLISION_EVENTS),
        Some(ground),
    );
    for i in 0..BOXES {
        let p = box_at(i);
        let body = w.insert_body(rp::RigidBodyBuilder::dynamic().translation(rp::Vector::new(p.x, p.y, p.z)));
        w.insert_collider(
            rp::ColliderBuilder::cuboid(0.4, 0.4, 0.4)
                .density(1000.0)
                .active_events(rp::ActiveEvents::COLLISION_EVENTS),
            Some(body),
        );
    }
    let collector = Collector(Mutex::new(Vec::new()));
    let mut count = 0;
    for tick in 0..WARMUP + MEASURED {
        if tick == WARMUP {
            start_counting();
        }
        w.step_with_events(&(), &collector);
        collector.0.lock().unwrap().clear();
        if tick == WARMUP + MEASURED - 1 {
            count = stop_counting();
        }
    }
    count
}

/// The same scene through the engine's components.
fn through_the_engine(queries: bool) -> usize {
    let counted = Rc::new(Cell::new(usize::MAX));
    let result = counted.clone();
    let mut hits = Vec::with_capacity(64);
    let mut near = Vec::with_capacity(BOXES);
    Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == WARMUP {
                start_counting();
            }
        })
        .physics3d(vec3(0.0, -9.81, 0.0))
        .sim_system(move |ctx: &mut Ctx| {
            if queries {
                let f = QueryFilter3D::default();
                let _ = ctx.physics3d.cast_ray(vec3(0.0, 20.0, 0.0), Vec3::NEG_Y, 100.0, f);
                ctx.physics3d
                    .cast_ray_all(vec3(-20.0, 0.5, 0.0), Vec3::X, 100.0, f, &mut hits);
                let _ = ctx
                    .physics3d
                    .cast_sphere(vec3(0.0, 20.0, 0.0), 0.5, Vec3::NEG_Y, 100.0, f);
                let _ = ctx
                    .physics3d
                    .cast_capsule(vec3(0.0, 20.0, 0.0), 1.0, 0.3, Vec3::NEG_Y, 100.0, f);
                ctx.physics3d.overlap_sphere(Vec3::ZERO, 3.0, f, &mut near);
                ctx.physics3d
                    .overlap_box(Vec3::ZERO, Vec3::splat(4.0), Quat::IDENTITY, f, &mut near);
            }
            let _ = ctx.events.read::<CollisionStarted>();
            if ctx.time.sim_tick() == WARMUP + MEASURED - 1 {
                result.set(stop_counting());
            }
        })
        .run_headless(WARMUP + MEASURED, |ctx| {
            ctx.world.spawn((
                Transform3D::at(vec3(0.0, -0.5, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::cuboid(vec3(40.0, 1.0, 40.0)),
            ));
            for i in 0..BOXES {
                ctx.world.spawn((
                    Transform3D::at(box_at(i)),
                    RigidBody3D::dynamic(),
                    Collider3D::cuboid(Vec3::splat(0.8)),
                ));
            }
        });
    counted.get()
}

#[test]
fn the_engine_adds_no_allocations_to_rapiers_own() {
    let rapier = rapier_alone();
    let engine = through_the_engine(false);
    println!("{MEASURED} ticks: {rapier} allocations in rapier alone, {engine} through the engine");
    assert!(
        engine <= rapier,
        "{engine} allocations through the engine, {rapier} in rapier alone"
    );
}

#[test]
fn queries_do_not_allocate() {
    let without = through_the_engine(false);
    let with = through_the_engine(true);
    assert_eq!(
        with,
        without,
        "ray casts, shape casts and overlaps added allocations: {with} with queries, {without} without, \
         {:+.2} per tick over {MEASURED} ticks",
        (with as f64 - without as f64) / MEASURED as f64
    );
}

const CARS: usize = 20;

fn car_at(i: usize) -> Vec3 {
    vec3((i % 5) as f32 * 30.0 - 60.0, 0.8, (i / 5) as f32 * 30.0 - 60.0)
}

/// Rapier alone with the cars' chassis held up and pushed by forces set
/// every step, as the wheels do: what rapier allocates for bodies whose
/// forces change.
fn chassis_pushed_in_rapier() -> usize {
    let mut w = rp::PhysicsWorld::new();
    w.integration_parameters.dt = 1.0 / ponko::d3::Physics::HZ as f32;
    w.gravity = rp::Vector::new(0.0, -9.81, 0.0);
    let ground = w.insert_body(rp::RigidBodyBuilder::fixed().translation(rp::Vector::new(0.0, -0.5, 0.0)));
    w.insert_collider(rp::ColliderBuilder::cuboid(200.0, 0.5, 200.0), Some(ground));
    let mut cars = Vec::new();
    for i in 0..CARS {
        let p = car_at(i);
        let body = w.insert_body(rp::RigidBodyBuilder::dynamic().translation(rp::Vector::new(p.x, p.y, p.z)));
        w.insert_collider(rp::ColliderBuilder::cuboid(0.9, 0.3, 2.1).mass(1200.0), Some(body));
        cars.push(body);
    }
    let collector = Collector(Mutex::new(Vec::new()));
    let mut count = 0;
    for tick in 0..WARMUP + MEASURED {
        if tick == WARMUP {
            start_counting();
        }
        for &car in &cars {
            let b = w.bodies.get_mut(car).unwrap();
            b.reset_forces(false);
            b.reset_torques(false);
            let (m, at) = (b.mass(), b.center_of_mass() + rp::Vector::new(0.0, 0.0, -1.0));
            b.add_force_at_point(rp::Vector::new(0.0, m * 9.81, -m), at, false);
        }
        w.step_with_events(&(), &collector);
        collector.0.lock().unwrap().clear();
        if tick == WARMUP + MEASURED - 1 {
            count = stop_counting();
        }
    }
    count
}

/// The same cars on wheels, driving in circles through the engine.
fn cars_driving() -> usize {
    let counted = Rc::new(Cell::new(usize::MAX));
    let result = counted.clone();
    Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == WARMUP {
                start_counting();
            }
            ctx.world.query::<&mut Vehicle3D>().for_each(|v| {
                for (i, w) in v.wheels_mut().iter_mut().enumerate() {
                    if i < 2 {
                        w.steer_angle = 0.3;
                    } else {
                        w.motor_torque = 200.0;
                    }
                }
            });
        })
        .physics3d(vec3(0.0, -9.81, 0.0))
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == WARMUP + MEASURED - 1 {
                result.set(stop_counting());
            }
        })
        .run_headless(WARMUP + MEASURED, |ctx| {
            ctx.world.spawn((
                Transform3D::at(vec3(0.0, -0.5, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::cuboid(vec3(400.0, 1.0, 400.0)),
            ));
            for i in 0..CARS {
                let mut car = Vehicle3D::new();
                for (x, z) in [(-0.8, -1.3), (0.8, -1.3), (-0.8, 1.3), (0.8, 1.3)] {
                    car = car.with_wheel(Wheel3D::new(vec3(x, -0.3, z), 0.35));
                }
                ctx.world.spawn((
                    Transform3D::at(car_at(i)),
                    RigidBody3D::dynamic().mass(1200.0),
                    Collider3D::cuboid(vec3(1.8, 0.6, 4.2)),
                    car,
                ));
            }
        });
    counted.get()
}

#[test]
fn vehicles_add_no_allocations_to_rapiers_own() {
    let rapier = chassis_pushed_in_rapier();
    let engine = cars_driving();
    println!("{MEASURED} ticks of {CARS} cars: {rapier} allocations in rapier alone, {engine} through the engine");
    assert!(
        engine <= rapier,
        "{engine} allocations driving, {rapier} in rapier alone"
    );
}

/// Under `PARALLEL_FROM` (16), so the characters move on the calling thread
/// and its thread-local count sees everything.
const WALKERS: usize = 4;

fn walker_at(i: usize) -> Vec3 {
    vec3(i as f32 * 3.0 - 6.0, 0.0, 0.0)
}

/// Rapier alone: kinematic capsules walking on a floor, each moved with
/// `KinematicCharacterController::move_shape` (same settings as the
/// engine's defaults) and then stepped: what rapier allocates for them.
fn walkers_in_rapier() -> usize {
    let c = Character3D::new(1.8, 0.3);
    let radius = c.radius;
    let half_height = c.height * 0.5 - radius;
    let center = half_height + radius;
    let shape = rp::Capsule::new_y(half_height, radius);
    let controller = KinematicCharacterController {
        up: rp::Vector::new(0.0, 1.0, 0.0),
        offset: CharacterLength::Absolute(0.01),
        slide: true,
        autostep: Some(CharacterAutostep {
            max_height: CharacterLength::Absolute(c.step_height),
            min_width: CharacterLength::Absolute(radius),
            include_dynamic_bodies: false,
        }),
        max_slope_climb_angle: c.max_slope,
        min_slope_slide_angle: c.max_slope,
        snap_to_ground: Some(CharacterLength::Absolute(c.snap_distance)),
        ..KinematicCharacterController::default()
    };
    let dt = 1.0 / ponko::d3::Physics::HZ as f32;
    let mut w = rp::PhysicsWorld::new();
    w.integration_parameters.dt = dt;
    w.gravity = rp::Vector::new(0.0, -9.81, 0.0);
    let ground = w.insert_body(rp::RigidBodyBuilder::fixed().translation(rp::Vector::new(0.0, -0.5, 0.0)));
    w.insert_collider(rp::ColliderBuilder::cuboid(200.0, 0.5, 200.0), Some(ground));
    let mut walkers = Vec::new();
    for i in 0..WALKERS {
        let p = walker_at(i);
        let body = w.insert_body(
            rp::RigidBodyBuilder::kinematic_position_based()
                .translation(rp::Vector::new(p.x, p.y, p.z))
                .build(),
        );
        w.insert_collider(
            rp::ColliderBuilder::capsule_y(half_height, radius)
                .translation(rp::Vector::new(0.0, center, 0.0))
                .active_events(rp::ActiveEvents::COLLISION_EVENTS),
            Some(body),
        );
        // Feet, vertical speed, standing on something.
        walkers.push((body, p, 0.0f32, false));
    }
    let collector = Collector(Mutex::new(Vec::new()));
    let mut count = 0;
    for tick in 0..WARMUP + MEASURED {
        if tick == WARMUP {
            start_counting();
        }
        for (body, feet, fall, grounded) in &mut walkers {
            // Like the engine: standing takes no fall, otherwise gravity pulls.
            if *grounded {
                *fall = 0.0;
            } else {
                *fall -= 9.81 * dt;
            }
            let filter = rp::QueryFilter::default().exclude_rigid_body(*body).exclude_sensors();
            let moved = {
                let queries = w.query_pipeline_with_filter(filter);
                controller.move_shape(
                    dt,
                    &queries,
                    &shape,
                    &rp::Pose::from_translation(rp::Vector::new(feet.x, feet.y + center, feet.z)),
                    rp::Vector::new(2.0 * dt, *fall * dt, 0.0),
                    |_| {},
                )
            };
            *grounded = moved.grounded;
            feet.x += moved.translation.x;
            feet.y += moved.translation.y;
            feet.z += moved.translation.z;
            w.bodies
                .get_mut(*body)
                .unwrap()
                .set_next_kinematic_translation(rp::Vector::new(feet.x, feet.y, feet.z));
        }
        w.step_with_events(&(), &collector);
        collector.0.lock().unwrap().clear();
        if tick == WARMUP + MEASURED - 1 {
            count = stop_counting();
        }
    }
    count
}

/// The same walkers as `Character3D`s through the engine.
fn walkers_through_the_engine() -> usize {
    let counted = Rc::new(Cell::new(usize::MAX));
    let result = counted.clone();
    Game::new("t", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == WARMUP {
                start_counting();
            }
        })
        .physics3d(vec3(0.0, -9.81, 0.0))
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.time.sim_tick() == WARMUP + MEASURED - 1 {
                result.set(stop_counting());
            }
        })
        .run_headless(WARMUP + MEASURED, |ctx| {
            ctx.world.spawn((
                Transform3D::at(vec3(0.0, -0.5, 0.0)),
                RigidBody3D::fixed(),
                Collider3D::cuboid(vec3(400.0, 1.0, 400.0)),
            ));
            for i in 0..WALKERS {
                let mut c = Character3D::new(1.8, 0.3);
                // Walking on its own: the velocity's X and Z stay as set.
                c.velocity.x = 2.0;
                ctx.world.spawn((Transform3D::at(walker_at(i)), c));
            }
        });
    counted.get()
}

#[test]
fn characters_add_no_allocations_to_rapiers_own() {
    let rapier = walkers_in_rapier();
    let engine = walkers_through_the_engine();
    println!(
        "{MEASURED} ticks of {WALKERS} walking characters: {rapier} allocations in rapier alone \
         ({:.1} per character per tick), {engine} through the engine",
        rapier as f64 / (MEASURED as usize * WALKERS) as f64
    );
    assert_ne!(
        engine,
        usize::MAX,
        "the run never reached the end of the measured ticks"
    );
    assert!(
        engine <= rapier,
        "{engine} allocations walking, {rapier} in rapier alone"
    );
}
