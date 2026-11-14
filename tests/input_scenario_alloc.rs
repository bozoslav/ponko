//! A regression check that applying scenario input steps allocates nothing.
//!
//! One test per binary, and a thread-local counter: only the thread running
//! the game counts, so other threads in the process can't disturb the
//! zero-allocation assertion.
#[allow(unused_imports)]
use ponko::actions::InputEvent;
use ponko::prelude::*;
#[allow(unused_imports)]
use ponko::scenario::InputScenario;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}
fn record_allocation() {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
    }
}
// SAFETY: all allocation and deallocation operations delegate unchanged to System.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: the allocator caller supplies a valid layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: the allocator caller supplies a valid layout.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        // SAFETY: the allocation and layout come from the allocator caller.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the allocation and layout come from the allocator caller.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn applying_scenario_input_steps_allocates_nothing_once_the_run_started() {
    // A press and a release on every tick; tracking starts after tick 0's
    // systems and ends in the last tick's, so the steps of ticks 1..=N-1 are
    // applied (by the engine, between systems) inside the tracked window.
    let mut scenario = InputScenario::new().duration(200);
    for tick in 0..200 {
        scenario = scenario
            .at(tick, InputEvent::Key(Key::Space, true))
            .at(tick, InputEvent::Key(Key::Space, false));
    }
    let mut pressed = 0;
    Game::new("allocations", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            match ctx.time.sim_tick() {
                0 => {
                    ALLOCS.set(0);
                    TRACK.set(true);
                }
                199 => {
                    TRACK.set(false);
                    assert_eq!(ALLOCS.get(), 0, "scenario steps allocated while applying");
                    assert_eq!(pressed, 199);
                }
                _ => {}
            }
            // A tap on every tick keeps both edges.
            pressed += u32::from(ctx.input.key_pressed(Key::Space));
        })
        .input_scenario(scenario)
        .run_headless(200, |_| {});
}
