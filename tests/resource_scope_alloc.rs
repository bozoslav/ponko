//! The steady-state allocations of `resource_scope` and a query.
//!
//! A single test in its own binary: `CountingAllocator` counts every thread
//! in the process (profiling relies on that), so nothing else may run, or
//! allocate on another thread, while this one measures.
#[allow(unused_imports)]
use ponko::perf::CountingAllocator;
use ponko::prelude::*;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Component)]
struct Count(u32);

fn scoped_tick(ctx: &mut Ctx) {
    let before = ponko::perf::alloc_counts();
    ctx.world.resource_scope::<Vec<u32>, _>(|world, totals| {
        world.query::<&mut Count>().for_each(|n| {
            n.0 += 1;
            totals[0] += n.0;
        });
    });
    assert_eq!(
        ponko::perf::alloc_counts(),
        before,
        "resource scope and query must not allocate"
    );
}

#[test]
fn scoped_resource_and_query_allocate_nothing_in_steady_state() {
    let (_, report) = Game::new("scope", 1, 1)
        .sim_system(scoped_tick)
        .profile_headless(10, 100, |ctx| {
            ctx.world.insert_resource(vec![0u32]);
            ctx.world.spawn_batch((0..100).map(|n| (Count(n as u32),)));
        });
    assert!(report.allocs.is_some(), "allocation counting must be enabled");
    assert_eq!(report.system("scoped_tick").unwrap().allocs_per_frame, 0.0);
}
