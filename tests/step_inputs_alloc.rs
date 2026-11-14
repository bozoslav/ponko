//! What declaring the physics and navigation steps costs in allocations:
//! nothing. The built-in steps are registered with their access
//! declarations, which builds with debug assertions check on every run;
//! that check must not allocate in steady state, so a declared step makes
//! exactly the allocations of the same step registered without one.
//!
//! A single test in its own binary: `CountingAllocator` counts every thread
//! in the process, so nothing else may run while this one measures.
#![cfg(all(feature = "physics", feature = "physics3d", feature = "navmesh"))]

use std::cell::Cell;
use std::rc::Rc;

use ponko::d3::{NavAgent as NavAgent3D, NavMesh as NavMesh3D};
use ponko::nav::{NavBakeSettings, NavSource};
#[allow(unused_imports)]
use ponko::perf::CountingAllocator;
use ponko::prelude::*;
use ponko::render::MeshData;
use ponko::{prelude::Systems, systems};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Ticks before the profiler (and so the counter) is on, and ticks measured.
const WARM_UP: u64 = 100;
const TICKS: u64 = 200;

/// Allocations made by the measured ticks (but the first few) of a game with 2D physics,
/// 3D physics (bodies and a character) and navigation, with the steps
/// registered with their declarations or without.
fn allocations(declared: bool) -> u64 {
    let (start, end) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let (at_start, at_end) = (Rc::clone(&start), Rc::clone(&end));
    let game = Game::new("t", 1, 1)
        .fixed_hz(60)
        .physics(vec2(0.0, 980.0))
        .physics3d(vec3(0.0, -9.81, 0.0));
    let game = if declared {
        game.add(
            Phase::BeforePhysics,
            systems::step_navmesh3d.access(systems::step_navmesh3d_access()),
        )
    } else {
        game.remove(Systems::PHYSICS)
            .remove(Systems::PHYSICS3D)
            .add(Phase::Physics, systems::step_physics)
            .add(Phase::Physics, systems::step_physics3d)
            .add(Phase::BeforePhysics, systems::step_navmesh3d)
    };
    assert_eq!(
        game.access_report()
            .systems
            .iter()
            .filter(|s| s.phase <= Phase::SimLast && s.access.is_some())
            .count(),
        if declared { 3 } else { 0 }
    );
    game.add(Phase::Input, move |ctx: &mut Ctx| {
        // Counting starts with the profiler, after the warm-up.
        if ctx.time.sim_tick() == WARM_UP + 10 {
            at_start.set(ponko::perf::alloc_counts().count);
        }
    })
    .add(Phase::SimLast, move |_: &mut Ctx| {
        at_end.set(ponko::perf::alloc_counts().count)
    })
    .profile_headless(WARM_UP, TICKS, |ctx| {
        use ponko::d2;
        use ponko::d3;
        ctx.world.spawn((
            d2::Transform::at(vec2(0.0, 200.0)),
            d2::RigidBody::fixed(),
            d2::Collider::rect(vec2(400.0, 20.0)),
        ));
        for i in 0..8 {
            ctx.world.spawn((
                d2::Transform::at(vec2(i as f32 * 12.0 - 40.0, -(i as f32) * 11.0)),
                d2::RigidBody::dynamic(),
                d2::Collider::circle(5.0),
            ));
        }
        ctx.world.spawn((
            d3::Transform::at(vec3(0.0, -0.5, 0.0)),
            d3::RigidBody::fixed(),
            d3::Collider::cuboid(vec3(40.0, 1.0, 40.0)),
        ));
        for i in 0..8 {
            ctx.world.spawn((
                d3::Transform::at(vec3(i as f32 * 1.5 - 6.0, 2.0 + i as f32, 6.0)),
                d3::RigidBody::dynamic(),
                d3::Collider::sphere(0.5),
            ));
        }
        ctx.world
            .spawn((d3::Transform::at(vec3(0.0, 0.05, -6.0)), d3::Character::new(1.8, 0.3)));
        let floor = NavSource::from_mesh(&MeshData::plane(20.0), d3::Transform::default());
        ctx.world
            .insert_resource(NavMesh3D::bake(&[floor], NavBakeSettings::default()).unwrap());
        for i in 0..4 {
            let mut agent = NavAgent3D::default();
            agent.speed = 0.5;
            agent.destination = Some(vec3(8.0, 0.0, i as f32 - 2.0));
            ctx.world
                .spawn((d3::Transform::at(vec3(-8.0, 0.0, i as f32 - 2.0)), agent));
        }
    });
    end.get() - start.get()
}

#[test]
fn declaring_the_steps_adds_no_allocations_to_them() {
    let plain = allocations(false);
    let declared = allocations(true);
    assert!(plain > 0, "the counter is on: rapier's solver allocates every step");
    assert_eq!(
        declared, plain,
        "{TICKS} ticks after {WARM_UP}: the declared steps (and their debug check) allocate what the plain steps do"
    );
}
