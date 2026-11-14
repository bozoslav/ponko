//! Wide archetypes: queries over them and warm structural changes on them do
//! not allocate, and removing a component with a destructor drops it once.
//! Its own test binary: it replaces the global allocator. Only allocations on
//! the thread inside `allocations_in` are counted, so libtest's other threads
//! (its slow-test warning under Miri, for one) can't fail it.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use ponko_ecs::*;

struct Counting;

thread_local! {
    /// Allocations on this thread while `allocations_in` runs here; `None` otherwise.
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Counts an allocation if this thread is inside `allocations_in`.
fn note() {
    // `try_with`: the allocator may run while this thread's locals are torn down.
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get().map(|n| n + 1)));
}

// SAFETY: forwards to the system allocator, only counting calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's contract, forwarded.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, forwarded.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: the caller's contract, forwarded.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Heap allocations (and reallocations) `f` makes on this thread.
fn allocations_in(f: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|n| n.set(Some(0)));
    f();
    ALLOCATIONS.with(|n| n.take()).unwrap()
}

#[derive(Clone, Copy, Component)]
struct P(f32);
#[derive(Clone, Copy, Component)]
struct V(f32);
#[derive(Component)]
struct Tag;
/// A Rust component with a destructor.
#[derive(Component)]
struct Name(#[allow(dead_code)] String);

const WIDTH: usize = 40;
const ARCHETYPES: usize = 64;

/// Drops of the runtime-registered components below, by kind.
static SMALL_DROPS: AtomicUsize = AtomicUsize::new(0);
static LARGE_DROPS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C-unwind" fn drop_small(_: *mut u8) {
    SMALL_DROPS.fetch_add(1, Ordering::Relaxed);
}

/// Checks that the value arrives intact: every byte is its index.
unsafe extern "C-unwind" fn drop_large(p: *mut u8) {
    // SAFETY: called on a 256-byte value of this component.
    let bytes = unsafe { std::slice::from_raw_parts(p, 256) };
    assert!(bytes.iter().enumerate().all(|(i, &b)| b == i as u8));
    LARGE_DROPS.fetch_add(1, Ordering::Relaxed);
}

/// `ARCHETYPES` archetypes of `WIDTH` components each (`P`, `V` and 38 of 64
/// runtime components), two entities in each.
fn wide_world() -> (World, Vec<ComponentId>, ComponentId, ComponentId) {
    let mut w = World::new();
    // SAFETY: plain 4-byte values without a destructor.
    let pool: Vec<ComponentId> = (0..64)
        .map(|i| unsafe { w.raw().register_component(&format!("c{i}"), 4, 4, None) })
        .collect();
    let (p, v) = (w.register::<P>(), w.register::<V>());
    w.register::<Tag>();
    let (pv, vv, zero) = (P(1.0), V(0.5), 0u32);
    for a in 0..ARCHETYPES {
        // Archetype `a` leaves out pool ids a..a+26 (mod 64): 38 of 64, all different.
        let mut parts: Vec<(ComponentId, *const u8)> = vec![(p, (&raw const pv).cast()), (v, (&raw const vv).cast())];
        parts.extend(
            (0..64)
                .filter(|i| (i + 64 - a) % 64 >= 26)
                .map(|i| (pool[i], (&raw const zero).cast::<u8>())),
        );
        assert_eq!(parts.len(), WIDTH);
        for _ in 0..2 {
            // SAFETY: every pointer is to a valid `Copy` value of its component.
            unsafe { w.raw().spawn_dynamic(&parts) };
        }
    }
    (w, pool, p, v)
}

fn queries_over_wide_archetypes_do_not_allocate() {
    let (mut w, pool, p, v) = wide_world();
    let frame = |w: &mut World| {
        let mut n = 0;
        w.query::<(&mut P, &V)>().for_each(|(a, b)| {
            a.0 += b.0;
            n += 1;
        });
        assert_eq!(n, 2 * ARCHETYPES);
        assert_eq!(
            w.query::<(&P, &V)>().filter::<Without<Tag>>().iter().count(),
            2 * ARCHETYPES
        );
        assert_eq!(w.query::<&P>().filter::<With<Tag>>().count(), 0);
        let mut rows = 0;
        w.raw().for_each_chunk(&[v, p, pool[63]], |c| rows += c.len());
        assert!(rows > 0 && rows < 2 * ARCHETYPES);
        let arch = w.raw_ref().archetypes().iter().find(|a| a.len() == 2).unwrap();
        assert!(arch.has(p) && arch.column_ptr(v).is_some() && arch.component_ids().len() == WIDTH);
    };
    frame(&mut w);
    let allocs = allocations_in(|| {
        for _ in 0..10 {
            frame(&mut w);
        }
    });
    assert_eq!(allocs, 0, "queries over {WIDTH}-component archetypes allocated");
}

fn warm_transitions_on_a_wide_entity_do_not_allocate() {
    let (mut w, pool, _, _) = wide_world();
    // SAFETY: 8-byte and 256-byte values; the drop functions only read them.
    let small = unsafe { w.raw().register_component("small-drop", 8, 8, Some(drop_small)) };
    let large = unsafe { w.raw().register_component("large-drop", 256, 16, Some(drop_large)) };
    let e = w.query::<Entity>().iter().next().unwrap();
    let plain = *pool
        .iter()
        .find(|&&id| w.raw_ref().get_dynamic(e, id).is_none())
        .unwrap();
    let small_value = [7u8; 8];
    let large_value: [u8; 256] = std::array::from_fn(|i| i as u8);
    let toggle = |w: &mut World| {
        let value = 1u32;
        // SAFETY: each value matches its component's layout; the world owns it after.
        unsafe {
            assert!(w.raw().insert_dynamic(e, plain, (&raw const value).cast()));
            assert!(w.raw().insert_dynamic(e, small, small_value.as_ptr()));
        }
        assert!(w.raw().remove_dynamic(e, plain));
        assert!(w.raw().remove_dynamic(e, small));
        assert!(w.insert(e, Name(String::new())));
        let name = w.remove::<Name>(e).unwrap();
        drop(name);
    };
    // Warm-up: creates the archetypes and edges, and grows their columns.
    toggle(&mut w);
    toggle(&mut w);
    let drops = SMALL_DROPS.load(Ordering::Relaxed);
    let allocs = allocations_in(|| {
        for _ in 0..100 {
            toggle(&mut w);
        }
    });
    // This also shows that every transition-edge lookup hit: a miss copies the
    // id list (`to_vec`/`collect` in world.rs), which allocates for any
    // archetype of more than one component, even when the archetype it finds
    // already exists (so the archetype count can't tell a miss from a hit).
    assert_eq!(allocs, 0, "warm add/remove on a {WIDTH}-component entity allocated");
    assert_eq!(
        SMALL_DROPS.load(Ordering::Relaxed) - drops,
        100,
        "each removed value is dropped once"
    );

    // A value too large for the stack slot still arrives intact and is dropped once.
    // SAFETY: a 256-byte value of the 256-byte component.
    assert!(unsafe { w.raw().insert_dynamic(e, large, large_value.as_ptr()) });
    assert!(w.raw().remove_dynamic(e, large));
    assert_eq!(LARGE_DROPS.load(Ordering::Relaxed), 1);
    assert!(w.raw_ref().get_dynamic(e, large).is_none());
}

#[test]
fn wide_archetypes_query_and_change_without_allocating() {
    queries_over_wide_archetypes_do_not_allocate();
    warm_transitions_on_a_wide_entity_do_not_allocate();
}

// ---- a world that walks through component sets ------------------------------

/// Optional marker `K`.
#[derive(Component)]
struct Opt<const K: usize>;
/// Optional data `K`, tracked in one of the tests.
#[allow(dead_code)] // Only its layout matters.
#[derive(Clone, Copy, Component)]
struct OptData<const K: usize>(u64);

/// Gives (`add`) or takes optional component `c` (0..8) of entity `e`.
fn toggle(w: &mut World, e: Entity, c: usize, add: bool) {
    macro_rules! arms {
        ($($n:literal)*) => {
            match c {
                $($n => if add { w.insert(e, Opt::<$n>); } else { w.remove::<Opt<$n>>(e); },)*
                $(c if c == $n + 4 => if add { w.insert(e, OptData::<$n>(c as u64)); } else { w.remove::<OptData<$n>>(e); },)*
                _ => unreachable!(),
            }
        };
    }
    arms!(0 1 2 3)
}

/// Entities that walk through all 256 sets of eight optional components,
/// one change per tick (the bit a Gray code flips), each starting at
/// another point of the walk, next to entities that stay put. The world
/// keeps far fewer empty archetypes than the walk passes through, so it
/// reclaims some and builds new ones every tick, the same ones every 256
/// ticks.
struct Walk {
    world: World,
    walkers: Vec<Entity>,
    tick: usize,
}

impl Walk {
    fn new(walkers: usize, still: usize) -> Self {
        let mut world = World::new();
        world.track_changes::<OptData<0>>();
        for i in 0..still {
            world.spawn((P(i as f32), V(1.0)));
        }
        let walkers = (0..walkers)
            .map(|i| {
                let e = world.spawn((P(i as f32),));
                // Start at the Gray code of an offset into the walk.
                let start = i * 256 / walkers;
                let gray = start ^ (start >> 1);
                for c in (0..8).filter(|c| gray >> c & 1 != 0) {
                    toggle(&mut world, e, c, true);
                }
                e
            })
            .collect();
        Walk {
            world,
            walkers,
            tick: 0,
        }
    }

    fn tick(&mut self) {
        for (i, &e) in self.walkers.iter().enumerate() {
            let step = (self.tick + i * 256 / self.walkers.len()) % 256;
            // Going from `step` to `step + 1` flips the lowest set bit of `step + 1`.
            let c = ((step + 1) % 256).trailing_zeros().min(7) as usize;
            let had = {
                let g = step ^ (step >> 1);
                g >> c & 1 != 0
            };
            toggle(&mut self.world, e, c, !had);
        }
        self.tick += 1;
        let mut sum = 0.0;
        self.world.query::<&P>().for_each(|p| sum += p.0);
        assert!(sum >= 0.0);
        // What a game does every tick: start the next one, and drop the
        // removals of `OptData<0>` everyone has seen.
        let seen = self.world.advance_change_tick();
        self.world.forget_removed(seen);
    }
}

#[test]
fn walking_through_more_component_sets_than_are_kept_stops_allocating() {
    // Miri checks the unsafe code on a short walk; the counts need a long one.
    let (walkers, still, settle, measured) = if cfg!(miri) { (2, 3, 2, 1) } else { (6, 100, 10, 4) };
    let mut walk = Walk::new(walkers, still);
    // Rounds of the walk until every archetype it needs has been built,
    // and the storage kept for each width, every buffer and every edge map
    // has the size it will need (the reclaiming sweep isn't in step with
    // the walk, so that takes a few rounds).
    for _ in 0..settle * 256 {
        walk.tick();
    }
    let slots = walk.world.raw_ref().archetypes().len();
    let allocations = allocations_in(|| {
        for _ in 0..measured * 256 {
            walk.tick();
        }
    });
    if !cfg!(miri) {
        assert_eq!(allocations, 0, "settled rounds of the walk allocated");
    }
    // Reclaimed slots were reused: the walk passed through 256 sets in each
    // round, with no new slot.
    assert_eq!(walk.world.raw_ref().archetypes().len(), slots);
    assert!(slots < 256, "{slots} slots");
}
