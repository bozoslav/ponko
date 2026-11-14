//! Counting heap allocations, to find code that allocates every frame.
//!
//! Install [`CountingAllocator`] as the global allocator and the profiler
//! counts allocations per frame and per system (the overlay, benchmark
//! reports and [`Perf::report`](crate::perf::Perf::report) show them). It
//! counts only while profiling is on, so a game that leaves it installed pays
//! one relaxed atomic load per allocation.
//!
//! Counts are for the whole process: allocations on rayon workers or other
//! threads count too, and a system that allocates inside `par_for_each` is
//! charged for it. Tests that assert on them should not run next to other
//! tests that allocate (give them a test file of their own).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use crate::scope::ACTIVE;

static COUNT: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static SEEN: AtomicBool = AtomicBool::new(false);

/// Heap allocations made while profiling was on, from [`alloc_counts`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocCounts {
    /// Allocations (and reallocations).
    pub count: u64,
    /// Bytes requested by them.
    pub bytes: u64,
}

/// The system allocator, counting what it hands out while profiling is on.
///
/// ```ignore
/// use ponko::perf::CountingAllocator;
///
/// #[global_allocator]
/// static ALLOCATOR: ponko::perf::CountingAllocator = ponko::perf::CountingAllocator;
/// ```
pub struct CountingAllocator;

#[inline]
fn note(bytes: usize) {
    if ACTIVE.load(Relaxed) > 0 {
        // Not `fetch_add` on a flag: one plain store the first time only.
        if !SEEN.load(Relaxed) {
            SEEN.store(true, Relaxed);
        }
        COUNT.fetch_add(1, Relaxed);
        BYTES.fetch_add(bytes as u64, Relaxed);
    }
}

// SAFETY: every method forwards to `System` with the same arguments; the
// counting only touches atomics and never allocates.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Counts one allocation of `bytes` made by something that isn't Rust's
/// allocator, such as C++'s `operator new` (see `PONKO_COUNT_CPP_ALLOCATIONS`
/// in `ponko.hpp`). Counts only while profiling is on.
pub fn note_allocation(bytes: usize) {
    note(bytes);
}

/// The allocations counted so far (while profiling was on). Take two readings
/// and subtract to count a stretch of code. All zero if
/// [`CountingAllocator`] isn't installed and nothing calls
/// [`note_allocation`].
///
/// ```
/// use ponko::perf::alloc_counts;
/// let before = ponko::perf::alloc_counts();
/// let after = ponko::perf::alloc_counts();
/// assert!(after.count >= before.count);
/// ```
pub fn alloc_counts() -> AllocCounts {
    AllocCounts {
        count: COUNT.load(Relaxed),
        bytes: BYTES.load(Relaxed),
    }
}

/// `true` once an allocation has been counted: the counting allocator is
/// installed (or C++ reports its own).
pub(crate) fn counting() -> bool {
    SEEN.load(Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_count_only_while_profiling() {
        let before = alloc_counts();
        crate::scope::enable();
        note_allocation(100);
        crate::scope::disable();
        let after = alloc_counts();
        // Other tests may be counting too, so only check the lower bound.
        assert!(after.count > before.count && after.bytes >= before.bytes + 100);
        assert!(counting());
    }
}
