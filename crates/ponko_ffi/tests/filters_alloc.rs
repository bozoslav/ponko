//! A repeated filtered query allocates nothing: the filter is compiled into
//! inline storage each time. Its own test binary, with a single test, because
//! the library's counting allocator (the one `ponko_ffi` installs, so this
//! file can't install a per-thread one as `ponko_ecs/tests/parallel_alloc.rs`
//! does) counts the whole process while profiling is on. The parallel part
//! therefore compares the quietest of several rounds, which the thread
//! pool's own occasional allocations can't change.
//!
//! The measuring runs inside a game's setup callback (profiling needs a
//! ctx), so nothing is asserted there: a failed assertion would abort the
//! process. It records what it saw, and the test checks that afterwards.
use ponko_ffi::*;
use std::ffi::c_void;
use std::ptr::null;
use std::sync::atomic::{AtomicUsize, Ordering};

use PonkoStatus::*;

unsafe extern "C" fn rows(chunk: *const PonkoChunk, user: *mut c_void) {
    unsafe { *user.cast::<usize>() += (*chunk).count };
}

/// What the parallel callback saw: rows and how many slices it was given.
#[derive(Default)]
struct Slices {
    rows: AtomicUsize,
    calls: AtomicUsize,
}
unsafe extern "C" fn slice_rows(chunk: *const PonkoChunk, user: *mut c_void) {
    let slices = unsafe { &*user.cast::<Slices>() };
    slices.rows.fetch_add(unsafe { (*chunk).count }, Ordering::Relaxed);
    slices.calls.fetch_add(1, Ordering::Relaxed);
}

fn allocations() -> u64 {
    ponko::perf::alloc_counts().count
}

const MARKERS: [&std::ffi::CStr; 5] = [c"C0", c"C1", c"C2", c"C3", c"C4"];
/// Entities with only marker 0, beside one of each combination of the four
/// markers: 2052 rows in one archetype, over what the parallel path runs inline.
const BIG: usize = 2048;

fn has(mask: u32, marker: u32) -> bool {
    mask >> marker & 1 != 0
}

#[derive(Default)]
struct Measured {
    /// The mask of every entity spawned, to compute what each filter matches.
    masks: Vec<u32>,
    failures: Vec<String>,
    serial_rows: Vec<usize>,
    serial_allocations: u64,
    check_allocations: u64,
    /// Per filter: the fewest allocations in any round of parallel queries, and
    /// the rows and slices of one more.
    parallel: Vec<(u64, usize, usize)>,
}

unsafe fn spawn(w: *mut PonkoWorld, out: &mut Measured) -> [u32; 5] {
    let mut ids = [0; 5];
    unsafe {
        for (id, name) in ids.iter_mut().zip(MARKERS) {
            assert_eq!(ponko_component_register(w, name.as_ptr(), 4, 4, id), PONKO_OK);
        }
        let masks = (0..64u32).map(|bits| bits & 15).chain(std::iter::repeat_n(1, BIG));
        for (n, mask) in masks.enumerate() {
            let value = n as u32;
            let mut values = vec![PonkoValue {
                component: ids[4],
                data: (&raw const value).cast(),
                size: 4,
            }];
            for (marker, &component) in ids.iter().enumerate().take(4) {
                if has(mask, marker as u32) {
                    values.push(PonkoValue {
                        component,
                        data: (&raw const value).cast(),
                        size: 4,
                    });
                }
            }
            let mut e = 0;
            assert_eq!(ponko_spawn(w, values.as_ptr(), values.len(), &mut e), PONKO_OK);
            out.masks.push(mask);
        }
    }
    ids
}

unsafe extern "C" fn measure(ctx: *mut PonkoCtx, user: *mut c_void) {
    let out = unsafe { &mut *user.cast::<Measured>() };
    unsafe {
        let w = ponko_ctx_world(ctx);
        let ids = spawn(w, out);
        let with = ponko_filter_with(ids[0]);
        let without = ponko_filter_without(ids[1]);
        let all = ponko_filter_all([with.cast_const(), without.cast_const()].as_ptr(), 2);
        // `(A AND B) OR NOT C`: expanded into clauses.
        let (a, b, not_c) = (
            ponko_filter_with(ids[0]),
            ponko_filter_with(ids[1]),
            ponko_filter_without(ids[2]),
        );
        let ab = ponko_filter_all([a.cast_const(), b.cast_const()].as_ptr(), 2);
        let any = ponko_filter_any([ab.cast_const(), not_c.cast_const()].as_ptr(), 2);
        // What the C++ wrapper builds for `each<with<T>>`: an `all` of one.
        let wrapped = ponko_filter_all([with.cast_const()].as_ptr(), 1);
        let (always, never) = (ponko_filter_always(), ponko_filter_never());
        // Filters in a fixed order: `with`, `all`, `any`, `wrapped`, `always`, `never`, none.
        let filters = [with, all, any, wrapped, always, never, null()];

        let mut options = ponko_query_options_default();
        options.writes = 0;
        let columns = [ids[4]];
        let run = |filter: *const PonkoFilter, options: &PonkoQueryOptions| -> (PonkoStatus, usize) {
            let mut n = 0usize;
            let status = ponko_query_filtered(w, columns.as_ptr(), 1, filter, options, Some(rows), (&raw mut n).cast());
            (status, n)
        };
        // Warm up once, then count: each of these is a repeated query.
        for filter in filters {
            run(filter, &options);
        }
        out.serial_rows.reserve(filters.len());
        out.failures.reserve(8);
        if ponko_ctx_set_profiling(ctx, true) != PONKO_OK {
            out.failures.push("profiling didn't start".into());
        }
        let before = allocations();
        for filter in filters {
            let mut n = 0;
            for _ in 0..50 {
                let (status, rows) = run(filter, &options);
                if status != PONKO_OK {
                    out.failures.push(format!("serial query: {status:?}"));
                }
                n = rows;
            }
            out.serial_rows.push(n);
        }
        let after_queries = allocations();
        let checked = ponko_filter_check(w, any);
        let after_check = allocations();
        ponko_ctx_set_profiling(ctx, false);
        out.serial_allocations = after_queries - before;
        out.check_allocations = after_check - after_queries;
        if checked != PONKO_OK {
            out.failures.push("ponko_filter_check failed".into());
        }

        // Parallel: ten rounds of twenty queries per filter; the filter adds
        // nothing to the quietest round of the unfiltered query.
        let mut par = options;
        par.parallel = true;
        out.parallel.reserve(4);
        for filter in [null(), with, any, wrapped] {
            let slices = Slices::default();
            let call = || {
                let status = ponko_query_filtered(
                    w,
                    columns.as_ptr(),
                    1,
                    filter,
                    &par,
                    Some(slice_rows),
                    (&raw const slices).cast_mut().cast(),
                );
                if status != PONKO_OK {
                    return Err(status);
                }
                Ok(())
            };
            for _ in 0..5 {
                let _ = call();
            }
            let mut quietest = u64::MAX;
            for _ in 0..10 {
                ponko_ctx_set_profiling(ctx, true);
                let before = allocations();
                for _ in 0..20 {
                    let _ = call();
                }
                let made = allocations() - before;
                ponko_ctx_set_profiling(ctx, false);
                quietest = quietest.min(made);
            }
            slices.rows.store(0, Ordering::Relaxed);
            slices.calls.store(0, Ordering::Relaxed);
            if call().is_err() {
                out.failures.push("parallel query failed".into());
            }
            out.parallel.push((
                quietest,
                slices.rows.load(Ordering::Relaxed),
                slices.calls.load(Ordering::Relaxed),
            ));
        }

        for f in [with, without, all, a, b, not_c, ab, any, wrapped, always, never] {
            ponko_filter_free(f);
        }
    }
}

#[test]
fn repeated_filtered_queries_allocate_nothing() {
    let mut measured = Measured::default();
    unsafe {
        let game = ponko_game_new(c"filters".as_ptr(), 1, 1);
        assert!(!game.is_null());
        let mut world = std::ptr::null_mut();
        let status = ponko_game_run_headless(game, 0, Some(measure), (&raw mut measured).cast(), &mut world);
        assert_eq!(status, PONKO_OK);
        ponko_world_free(world);
    }
    assert!(measured.failures.is_empty(), "{:?}", measured.failures);

    let count = |predicate: fn(u32) -> bool| measured.masks.iter().filter(|&&m| predicate(m)).count();
    let n = measured.masks.len();
    assert_eq!(n, 64 + BIG);
    let want = [
        count(|m| has(m, 0)),                              // with
        count(|m| has(m, 0) && !has(m, 1)),                // all
        count(|m| (has(m, 0) && has(m, 1)) || !has(m, 2)), // any
        count(|m| has(m, 0)),                              // wrapped
        n,                                                 // always
        0,                                                 // never
        n,                                                 // no filter
    ];
    assert_eq!(measured.serial_rows, want);
    assert_eq!(
        measured.serial_allocations, 0,
        "350 serial filtered queries (plain, nested, wrapped, empty, none) allocated"
    );
    assert_eq!(measured.check_allocations, 0, "ponko_filter_check allocated");

    // The parallel path really ran: the big archetype was cut into slices, so
    // the callback got more calls than there are matching archetypes.
    let archetypes = |predicate: fn(u32) -> bool| {
        let mut masks: Vec<u32> = measured.masks.iter().copied().filter(|&m| predicate(m)).collect();
        masks.sort();
        masks.dedup();
        masks.len()
    };
    let [plain, with, any, wrapped] = measured.parallel[..] else {
        panic!("{:?}", measured.parallel)
    };
    let expected = [
        (plain, n, archetypes(|_| true)),
        (with, want[0], archetypes(|m| has(m, 0))),
        (any, want[2], archetypes(|m| (has(m, 0) && has(m, 1)) || !has(m, 2))),
        (wrapped, want[0], archetypes(|m| has(m, 0))),
    ];
    for ((_, rows, slices), want_rows, matching) in expected {
        assert_eq!(rows, want_rows);
        assert!(
            slices > matching,
            "{slices} slices over {matching} archetypes: the big archetype wasn't cut up"
        );
    }
    assert!(
        with.0 <= plain.0 && any.0 <= plain.0 && wrapped.0 <= plain.0,
        "quietest round of 20 parallel queries: plain {}, with {}, nested {}, wrapped {}",
        plain.0,
        with.0,
        any.0,
        wrapped.0
    );
}
