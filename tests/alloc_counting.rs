//! Counting heap allocations per frame and per system. This file has its own
//! process (and a single test), so nothing else allocates while it counts.

#[allow(unused_imports)]
use ponko::perf::CountingAllocator;
#[allow(unused_imports)]
use ponko::perf::{Budget, WarningKind};
use ponko::prelude::*;

// ANCHOR: counting_allocator
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
// ANCHOR_END: counting_allocator

/// Allocates every tick, the thing the report should flag.
fn leaky(_ctx: &mut Ctx) {
    std::hint::black_box(Box::new([0u8; 64]));
    std::hint::black_box(vec![1u32; 10]);
}

/// Keeps its buffer between ticks.
fn tidy(_ctx: &mut Ctx) {
    let mut buf = [0u32; 8];
    buf[0] += 1;
    std::hint::black_box(buf);
}

#[test]
fn the_report_names_the_system_that_allocates() {
    let (_world, report) = Game::new("t", 1, 1)
        .sim_system(leaky)
        .sim_system(tidy)
        .profile_headless(10, 100, |_| {});
    let allocs = report.allocs.expect("the counting allocator is installed");
    assert!(allocs.per_frame >= 2.0, "{report}");
    assert!(report.system("leaky").unwrap().allocs_per_frame >= 2.0, "{report}");
    assert_eq!(report.system("tidy").unwrap().allocs_per_frame, 0.0, "{report}");
    let warning = report
        .warnings
        .iter()
        .find(|w| w.kind == WarningKind::AllocatesEveryFrame);
    assert!(warning.is_some_and(|w| w.message.contains("leaky")), "{report}");
    let broken = report.check(&Budget::new().allocs_per_frame(0.0));
    assert_eq!(broken.len(), 1);
    // The counts are exact for a tidy system: it is the one to hold to zero.
    let (_w, quiet) = Game::new("t", 1, 1).sim_system(tidy).profile_headless(10, 100, |_| {});
    assert_eq!(quiet.system("tidy").unwrap().allocs_per_frame, 0.0);
}
