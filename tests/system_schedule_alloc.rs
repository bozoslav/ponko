//! The steady-state allocations of a batch of view systems.
//!
//! Profiling runs the batch on the calling thread, even when parallelism is
//! requested. Count that thread for exact assertions: idle Rayon workers can
//! still allocate when parking, after the pool has started and warm-up ended.
//! Keep the engine's process-wide counter installed for the profiling report.
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::time::Duration;

use ponko::access::SystemAccess;
#[allow(unused_imports)]
use ponko::perf::CountingAllocator;
use ponko::prelude::*;
use ponko::system_view::SystemView;

struct ThreadCountingAllocator;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn thread_allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

fn note_allocation() {
    // Const-initialized TLS neither allocates nor needs a destructor. Ignore
    // threads whose TLS is unavailable during teardown.
    let _ = ALLOCS.try_with(|count| count.set(count.get() + 1));
}

// SAFETY: all operations forward unchanged to the engine's counting allocator;
// the extra accounting only updates an allocation-free thread-local Cell.
unsafe impl GlobalAlloc for ThreadCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: the caller supplies the allocator's layout contract.
        unsafe { CountingAllocator.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: the caller supplies the allocator's layout contract.
        unsafe { CountingAllocator.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        note_allocation();
        // SAFETY: the pointer, layout and size retain the caller's contract.
        unsafe { CountingAllocator.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout retain the caller's contract.
        unsafe { CountingAllocator.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: ThreadCountingAllocator = ThreadCountingAllocator;

const WARMUP: u64 = 50;
const TICKS: u64 = 250;

#[derive(Component)]
struct Ticks(u64);
#[derive(Component)]
struct Pos(f32);
#[derive(Component)]
struct Vel(f32);
struct Total(f64);
#[derive(Clone, Copy)]
struct Moved(u32);

/// The allocation count when the batch was about to start.
struct Before(u64);

/// How many ticks' batches allocated, after warm-up.
struct Allocating(u32);

fn movement(sys: &mut SystemView) {
    let mut n = 0;
    sys.world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
        p.0 += v.0;
        n += 1;
    });
    sys.events.send(Moved(n));
}

fn total(sys: &mut SystemView) {
    let moved: u32 = sys.events.read::<Moved>().iter().map(|m| m.0).sum();
    sys.world.resource_mut::<Total>().0 += f64::from(moved);
}

fn run(threshold: Duration, allocate: bool) -> (u32, f64) {
    let (world, report) = Game::new("schedule allocations", 1, 1)
        .parallel_systems(true)
        .parallel_threshold(threshold)
        .parallel_systems_env(false)
        .sim_system(|ctx: &mut Ctx| {
            let now = thread_allocs();
            ctx.world.resource_mut::<Before>().0 = now;
        })
        // One batch: a reader before the sender, and a writer of its own.
        .add(
            Phase::AfterPhysics,
            total.access(SystemAccess::new().read_events::<Moved>().write_resource::<Total>()),
        )
        .add(
            Phase::AfterPhysics,
            (move |sys: &mut SystemView| {
                // Positive control inside the same measured batch.
                if allocate {
                    std::hint::black_box(Box::new([0u8; 64]));
                }
                movement(sys);
            })
            .access(SystemAccess::new().write::<Pos>().read::<Vel>().send_events::<Moved>()),
        )
        .add(
            Phase::AfterPhysics,
            (|sys: &mut SystemView| {
                sys.world.query::<&mut Ticks>().for_each(|n| n.0 += 1);
            })
            .access(SystemAccess::new().write::<Ticks>()),
        )
        // Runs right after the batch's barrier.
        .sim_system(|ctx: &mut Ctx| {
            let made = thread_allocs() - ctx.world.resource::<Before>().0;
            // Buffers grow in the first ticks.
            if ctx.time.sim_tick() >= WARMUP && made > 0 {
                ctx.world.resource_mut::<Allocating>().0 += 1;
            }
        })
        .profile_headless(WARMUP, TICKS, |ctx| {
            ctx.world.insert_resource(Before(0));
            ctx.world.insert_resource(Total(0.0));
            ctx.world.insert_resource(Allocating(0));
            ctx.world
                .spawn_batch((0..500).map(|i| (Pos(i as f32), Vel(1.0), Ticks(i as u64))));
        });
    assert_eq!(report.frames, TICKS as usize);
    assert!(report.allocs.is_some(), "allocation counting must be enabled");
    (world.resource::<Allocating>().0, world.resource::<Total>().0)
}

#[test]
fn a_batch_allocates_nothing_in_steady_state() {
    // Start every worker before profiling; parking may still allocate later,
    // so exact assertions use only the profiling thread's counter.
    rayon::broadcast(|_| ());
    // Both modes must count every allocating tick and reject any allocation
    // in a steady-state batch. Profiling keeps these systems on this thread.
    for (threshold, mode) in [(Duration::MAX, "in order"), (Duration::ZERO, "parallel requested")] {
        let (allocating, control_total) = run(threshold, true);
        assert_eq!(
            allocating, TICKS as u32,
            "positive control must count every measured tick, {mode}"
        );

        let (allocating, total) = run(threshold, false);
        assert_eq!(
            total,
            500.0 * (WARMUP + TICKS - 1) as f64,
            "the reader saw every tick's event one tick later"
        );
        assert_eq!(control_total, total);
        assert_eq!(allocating, 0, "ticks whose batch allocated, {mode}");
    }
}

#[test]
fn the_counter_tracks_alloc_zeroed_and_realloc_only_on_the_calling_thread() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static START: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);
    let worker = std::thread::spawn(|| {
        while !START.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        let before = thread_allocs();
        let allocated = std::hint::black_box(Box::new([1u8; 64]));
        let mut zeroed = std::hint::black_box(vec![0u8; 64]);
        zeroed.reserve(128);
        std::hint::black_box(&zeroed);
        let made = thread_allocs() - before;
        // Publish completion before asserting so a failure cannot strand the
        // measuring thread. Keep the buffers live through the snapshot.
        DONE.store(true, Ordering::Release);
        assert_eq!(made, 3, "alloc, alloc_zeroed and realloc must each count");
        drop((allocated, zeroed));
    });
    // Thread creation and joining may allocate on this thread, so keep both
    // outside the readings. The worker's allocations must not change ours.
    let before = thread_allocs();
    START.store(true, Ordering::Release);
    while !DONE.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    let after = thread_allocs();
    worker.join().unwrap();
    assert_eq!(after, before, "another thread must not contaminate the count");
}
