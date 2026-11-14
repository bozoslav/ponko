//! Queries: loop over every entity that has a set of components.

use std::marker::PhantomData;

use crate::bundle::Component;
use crate::change::{self, Clock, RowFilter, Tick};
use crate::entity::Entity;
use crate::seal::Seal;
use crate::storage::{Archetype, ComponentId};
use crate::view::SharedFilters;
use crate::world::World;

mod cache;
pub(crate) use cache::{MatchCache, signature};
pub use cache::{QueryCache, QueryCachePolicy, QueryCacheStats};
mod dynamic;
pub use dynamic::{CompiledFilter, DynFilter, FilterError, FilterShape, FilterTree, MAX_FILTER_DEPTH, compile_tree};

/// What a query fetches per entity: `&T`, `&mut T`, `Option<&T>`,
/// `Option<&mut T>`, [`Entity`], or a tuple of those (up to 12; nest tuples
/// for more).
///
/// Fetching `&mut T` (or `Option<&mut T>`) counts as a change of `T` for
/// every entity the query visits, written or not: see
/// [change detection](World::track_changes).
///
/// Implemented by this crate only: the trait is sealed, so the contract
/// below is one this crate keeps, not one callers can be asked to keep.
///
/// # Safety
/// `state`/`fetch` may only create references into columns listed by `access`,
/// with the mutability listed there. `state` returns `Some` exactly when the
/// archetype has every component that `access` reports as required.
///
/// `fetch` must be callable from several threads at once for different rows
/// of one archetype, each with a copy of the same `State`, and the items it
/// returns must be usable on the thread that fetched them: the parallel
/// loops ([`Query::par_for_each`]) and views that share a world
/// ([`ViewGroup`](crate::ViewGroup)) do exactly that. The implementations
/// here only make `&T` and `&mut T` to [`Component`]s, which are
/// `Send + Sync`, and copy [`Entity`] ids.
pub unsafe trait QueryData {
    #[doc(hidden)]
    const SEAL: Seal;
    /// What the loop body receives.
    type Item<'a>;
    /// Parallel queries read the ids from every worker thread.
    #[doc(hidden)]
    type Ids: Copy + Send + Sync;
    #[doc(hidden)]
    type State: Copy;
    #[doc(hidden)]
    fn ids(world: &mut World) -> Self::Ids;
    /// The ids without registering anything: `None` if a component isn't
    /// registered yet, or if this query data doesn't support shared lookup
    /// (what [`WorldView`](crate::WorldView) queries need).
    #[doc(hidden)]
    fn ids_in(world: &World) -> Option<Self::Ids> {
        let _ = world;
        None
    }
    /// Reports `(component, mutable, required)` for each component touched.
    #[doc(hidden)]
    fn access(ids: &Self::Ids, f: &mut impl FnMut(ComponentId, bool, bool));
    #[doc(hidden)]
    fn state(arch: &Archetype, ids: &Self::Ids) -> Option<Self::State>;
    #[doc(hidden)]
    unsafe fn fetch<'a>(state: Self::State, row: usize) -> Self::Item<'a>;
}

unsafe impl<T: Component> QueryData for &T {
    const SEAL: Seal = Seal;
    type Item<'a> = &'a T;
    type Ids = ComponentId;
    type State = *const T;
    fn ids(world: &mut World) -> ComponentId {
        world.register::<T>()
    }
    fn ids_in(world: &World) -> Option<ComponentId> {
        world.component_id::<T>()
    }
    fn access(id: &ComponentId, f: &mut impl FnMut(ComponentId, bool, bool)) {
        f(*id, false, true)
    }
    #[inline]
    fn state(arch: &Archetype, id: &ComponentId) -> Option<*const T> {
        arch.column_base(*id).map(|p| p as *const T)
    }
    #[inline(always)]
    unsafe fn fetch<'a>(state: *const T, row: usize) -> &'a T {
        unsafe { &*state.add(row) }
    }
}

unsafe impl<T: Component> QueryData for &mut T {
    const SEAL: Seal = Seal;
    type Item<'a> = &'a mut T;
    type Ids = ComponentId;
    type State = *mut T;
    fn ids(world: &mut World) -> ComponentId {
        world.register::<T>()
    }
    fn ids_in(world: &World) -> Option<ComponentId> {
        world.component_id::<T>()
    }
    fn access(id: &ComponentId, f: &mut impl FnMut(ComponentId, bool, bool)) {
        f(*id, true, true)
    }
    #[inline]
    fn state(arch: &Archetype, id: &ComponentId) -> Option<*mut T> {
        arch.column_base(*id).map(|p| p as *mut T)
    }
    #[inline(always)]
    unsafe fn fetch<'a>(state: *mut T, row: usize) -> &'a mut T {
        unsafe { &mut *state.add(row) }
    }
}

unsafe impl<T: Component> QueryData for Option<&T> {
    const SEAL: Seal = Seal;
    type Item<'a> = Option<&'a T>;
    type Ids = ComponentId;
    type State = Option<*const T>;
    fn ids(world: &mut World) -> ComponentId {
        world.register::<T>()
    }
    fn ids_in(world: &World) -> Option<ComponentId> {
        world.component_id::<T>()
    }
    fn access(id: &ComponentId, f: &mut impl FnMut(ComponentId, bool, bool)) {
        f(*id, false, false)
    }
    #[inline]
    fn state(arch: &Archetype, id: &ComponentId) -> Option<Option<*const T>> {
        Some(arch.column_base(*id).map(|p| p as *const T))
    }
    #[inline(always)]
    unsafe fn fetch<'a>(state: Option<*const T>, row: usize) -> Option<&'a T> {
        state.map(|p| unsafe { &*p.add(row) })
    }
}

unsafe impl<T: Component> QueryData for Option<&mut T> {
    const SEAL: Seal = Seal;
    type Item<'a> = Option<&'a mut T>;
    type Ids = ComponentId;
    type State = Option<*mut T>;
    fn ids(world: &mut World) -> ComponentId {
        world.register::<T>()
    }
    fn ids_in(world: &World) -> Option<ComponentId> {
        world.component_id::<T>()
    }
    fn access(id: &ComponentId, f: &mut impl FnMut(ComponentId, bool, bool)) {
        f(*id, true, false)
    }
    #[inline]
    fn state(arch: &Archetype, id: &ComponentId) -> Option<Option<*mut T>> {
        Some(arch.column_base(*id).map(|p| p as *mut T))
    }
    #[inline(always)]
    unsafe fn fetch<'a>(state: Option<*mut T>, row: usize) -> Option<&'a mut T> {
        state.map(|p| unsafe { &mut *p.add(row) })
    }
}

unsafe impl QueryData for Entity {
    const SEAL: Seal = Seal;
    type Item<'a> = Entity;
    type Ids = ();
    type State = *const Entity;
    fn ids(_: &mut World) {}
    fn ids_in(_: &World) -> Option<()> {
        Some(())
    }
    fn access(_: &(), _: &mut impl FnMut(ComponentId, bool, bool)) {}
    #[inline]
    fn state(arch: &Archetype, _: &()) -> Option<*const Entity> {
        Some(arch.entities.as_ptr())
    }
    #[inline(always)]
    unsafe fn fetch<'a>(state: *const Entity, row: usize) -> Self::Item<'a> {
        unsafe { *state.add(row) }
    }
}

macro_rules! query_tuple {
    ($($t:ident $i:tt),*) => {
        unsafe impl<$($t: QueryData),*> QueryData for ($($t,)*) {
            const SEAL: Seal = Seal;
            type Item<'a> = ($($t::Item<'a>,)*);
            type Ids = ($($t::Ids,)*);
            type State = ($($t::State,)*);
            fn ids(world: &mut World) -> Self::Ids {
                ($($t::ids(world),)*)
            }
            fn ids_in(world: &World) -> Option<Self::Ids> {
                Some(($($t::ids_in(world)?,)*))
            }
            fn access(ids: &Self::Ids, f: &mut impl FnMut(ComponentId, bool, bool)) {
                $($t::access(&ids.$i, f);)*
            }
            #[inline]
            fn state(arch: &Archetype, ids: &Self::Ids) -> Option<Self::State> {
                Some(($($t::state(arch, &ids.$i)?,)*))
            }
            #[inline(always)]
            unsafe fn fetch<'a>(state: Self::State, row: usize) -> Self::Item<'a> {
                unsafe { ($($t::fetch(state.$i, row),)*) }
            }
        }
    };
}
query_tuple!(A 0);
query_tuple!(A 0, B 1);
query_tuple!(A 0, B 1, C 2);
query_tuple!(A 0, B 1, C 2, D 3);
query_tuple!(A 0, B 1, C 2, D 3, E 4);
query_tuple!(A 0, B 1, C 2, D 3, E 4, F 5);
query_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
query_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
query_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8);
query_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9);
query_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10);
query_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11);

/// Panics if a query asks for the same component twice with at least one `&mut`.
pub(crate) fn check_aliasing<Q: QueryData>(world: &World, ids: &Q::Ids) {
    const MAX: usize = 32;
    let mut seen = [(ComponentId(0), false); MAX];
    let mut n = 0;
    Q::access(ids, &mut |id, mutable, _| {
        for &(other, other_mut) in &seen[..n] {
            if other == id && (mutable || other_mut) {
                panic!(
                    "query `{}` accesses `{}` twice, once mutably; that would alias",
                    std::any::type_name::<Q>(),
                    world.component_info(id).name()
                );
            }
        }
        assert!(
            n < MAX,
            "query `{}` touches more than {MAX} components",
            std::any::type_name::<Q>()
        );
        seen[n] = (id, mutable);
        n += 1;
    });
}

/// Filters added with [`Query::filter`]. Inline storage, so running a
/// filtered query doesn't allocate.
///
/// An archetype matches when it has every `with` component, none of the
/// `without` components, and satisfies each `Or` clause: clause `c` holds
/// the terms `any[start..any_end[c]]`, and term `i` asks for presence when
/// bit `i` of `any_has` is set, absence otherwise.
///
/// `Added` and `Changed` filters also pick rows inside a matching archetype
/// (see [`Filters::row_filter`]). One that must hold is a `with` whose bit
/// is set in `with_added` or `with_changed`; inside an `Or`, bit `i` of
/// `any_added` or `any_changed` marks term `i` (which then asks for presence
/// too).
///
/// Public but not exported: it can't be named outside this crate, which
/// seals [`QueryFilter`], [`OrTerm`] and [`OrTerms`].
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub struct Filters {
    with: [ComponentId; MAX_WITH],
    without: [ComponentId; MAX_WITH],
    n_with: u8,
    n_without: u8,
    any: [ComponentId; MAX_OR_TERMS],
    any_has: u16,
    any_end: [u8; MAX_OR],
    n_any: u8,
    n_terms: u8,
    with_added: u8,
    with_changed: u8,
    any_added: u16,
    any_changed: u16,
}

const MAX_WITH: usize = 8;
const MAX_TICKS: usize = 4;
const MAX_OR: usize = 4;
const MAX_OR_TERMS: usize = 16;

impl Filters {
    /// Whether an `Added` or `Changed` filter here reads the stamps of
    /// component `id`.
    pub(crate) fn reads_stamps_of(&self, id: ComponentId) -> bool {
        let ticks = self.with_added | self.with_changed;
        let any_ticks = self.any_added | self.any_changed;
        (0..self.n_with as usize).any(|i| ticks >> i & 1 != 0 && self.with[i] == id)
            || (0..self.n_terms as usize).any(|i| any_ticks >> i & 1 != 0 && self.any[i] == id)
    }

    /// Always inlined: the query loops rely on it to specialize their
    /// archetype loop for the filters in use.
    #[inline(always)]
    pub(crate) fn matches(&self, arch: &Archetype) -> bool {
        if !(self.with[..self.n_with as usize].iter().all(|&id| arch.has(id))
            && !self.without[..self.n_without as usize].iter().any(|&id| arch.has(id)))
        {
            return false;
        }
        let mut start = 0;
        for &end in &self.any_end[..self.n_any as usize] {
            let end = end as usize;
            let any = (start..end).any(|i| arch.has(self.any[i]) == (self.any_has >> i & 1 != 0));
            if !any {
                return false;
            }
            start = end;
        }
        true
    }

    /// [`matches`](Self::matches) as a call, for the loops of worlds that
    /// track changes: they exist next to the plain ones at every call site,
    /// so they stay small.
    #[inline(never)]
    fn matches_tracked(&self, arch: &Archetype) -> bool {
        self.matches(arch)
    }

    pub(crate) fn add_with(&mut self, id: ComponentId) {
        if self.with[..self.n_with as usize].contains(&id) {
            return;
        }
        assert!(
            (self.n_with as usize) < MAX_WITH,
            "at most {MAX_WITH} `with` filters per query"
        );
        self.with[self.n_with as usize] = id;
        self.n_with += 1;
    }

    pub(crate) fn add_without(&mut self, id: ComponentId) {
        if self.without[..self.n_without as usize].contains(&id) {
            return;
        }
        assert!(
            (self.n_without as usize) < MAX_WITH,
            "at most {MAX_WITH} `without` filters per query"
        );
        self.without[self.n_without as usize] = id;
        self.n_without += 1;
    }

    /// Adds a term to the `Or` clause being built: the terms after the last
    /// closed clause.
    pub(crate) fn add_or_term(&mut self, id: ComponentId, has: bool) {
        let n = self.n_terms as usize;
        assert!(
            n < MAX_OR_TERMS,
            "at most {MAX_OR_TERMS} components across a query's `Or` filters"
        );
        self.any[n] = id;
        if has {
            self.any_has |= 1 << n;
        }
        self.n_terms += 1;
    }

    /// Adds an `Added` (`added`) or `Changed` filter on component `id` that
    /// must hold; the component must be tracked.
    pub(crate) fn add_tick(&mut self, id: ComponentId, added: bool) {
        self.add_with(id);
        let n = self.n_with as usize;
        let i = self.with[..n].iter().position(|&with| with == id).expect("just added");
        let filters = self.with_added.count_ones() + self.with_changed.count_ones();
        let mask = if added {
            &mut self.with_added
        } else {
            &mut self.with_changed
        };
        if *mask >> i & 1 != 0 {
            return;
        }
        assert!(
            filters < MAX_TICKS as u32,
            "at most {MAX_TICKS} `Added`/`Changed` filters per query outside `Or`"
        );
        *mask |= 1 << i;
    }

    /// Adds an `Added` (`added`) or `Changed` term on component `id` to the
    /// `Or` clause being built; the component must be tracked.
    pub(crate) fn add_or_tick(&mut self, id: ComponentId, added: bool) {
        let n = self.n_terms as usize;
        self.add_or_term(id, true);
        if added {
            self.any_added |= 1 << n;
        } else {
            self.any_changed |= 1 << n;
        }
    }

    /// The row test `Added`/`Changed` filters apply inside matching archetype
    /// `arch`, or `None` if every row passes: no such filters, or each one
    /// sits in an `Or` that another term already satisfies for all of `arch`.
    #[inline]
    pub(crate) fn row_filter(&self, arch: &Archetype, clock: Clock) -> Option<RowFilter> {
        let in_or = self.any_added | self.any_changed;
        if self.with_added | self.with_changed == 0 && in_or == 0 {
            return None;
        }
        let mut rows = RowFilter::new(clock);
        for i in 0..self.n_with as usize {
            for (mask, added) in [(self.with_added, true), (self.with_changed, false)] {
                if mask >> i & 1 != 0 {
                    let stamps = arch.read_stamps(self.with[i], added);
                    rows.term(stamps.expect("an `Added`/`Changed` filter's component is present and tracked"));
                    rows.close();
                }
            }
        }
        let mut start = 0;
        for &end in &self.any_end[..self.n_any as usize] {
            let end = end as usize;
            let whole =
                (start..end).any(|i| in_or >> i & 1 == 0 && arch.has(self.any[i]) == (self.any_has >> i & 1 != 0));
            if !whole {
                for i in (start..end).filter(|i| in_or >> i & 1 != 0) {
                    if let Some(stamps) = arch.read_stamps(self.any[i], self.any_added >> i & 1 != 0) {
                        rows.term(stamps);
                    }
                }
                rows.close();
            }
            start = end;
        }
        (!rows.is_empty()).then_some(rows)
    }

    /// Converts required predicates into singleton clauses. Only construction
    /// uses this representation; the matching loops and cache key stay unchanged.
    pub(crate) fn into_clauses(self) -> Self {
        let mut out = Self::default();
        for i in 0..self.n_with as usize {
            let added = self.with_added >> i & 1 != 0;
            let changed = self.with_changed >> i & 1 != 0;
            if !added && !changed {
                out.add_or_term(self.with[i], true);
                out.close_or();
            }
            for (enabled, added) in [(added, true), (changed, false)] {
                if enabled {
                    out.add_or_tick(self.with[i], added);
                    out.close_or();
                }
            }
        }
        for &id in &self.without[..self.n_without as usize] {
            out.add_or_term(id, false);
            out.close_or();
        }
        out.append_clauses(&self);
        out
    }

    /// An empty disjunction is false; an empty conjunction (Default) is true.
    pub(crate) fn never() -> Self {
        let mut out = Self::default();
        out.close_or();
        out
    }

    fn copy_terms(&mut self, from: &Self, start: usize, end: usize) {
        for i in start..end {
            let n = self.n_terms;
            self.add_or_term(from.any[i], from.any_has >> i & 1 != 0);
            self.any_added |= ((from.any_added >> i) & 1) << n;
            self.any_changed |= ((from.any_changed >> i) & 1) << n;
        }
    }

    pub(crate) fn append_clauses(&mut self, other: &Self) {
        let mut start = 0;
        for &end in &other.any_end[..other.n_any as usize] {
            self.copy_terms(other, start, end as usize);
            self.close_or();
            start = end as usize;
        }
    }

    /// Distribute OR over AND: (a AND b) OR c = (a OR c) AND (b OR c).
    /// Both operands contain only clauses. Bounds are checked before each
    /// write, so expansion stays inline and fails with the usual limit errors.
    pub(crate) fn disjoin(&self, other: &Self) -> Self {
        let mut out = Self::default();
        let mut left = 0;
        for &left_end in &self.any_end[..self.n_any as usize] {
            let mut right = 0;
            for &right_end in &other.any_end[..other.n_any as usize] {
                out.copy_terms(self, left, left_end as usize);
                out.copy_terms(other, right, right_end as usize);
                out.close_or();
                right = right_end as usize;
            }
            left = left_end as usize;
        }
        out
    }

    /// Closes the `Or` clause being built.
    pub(crate) fn close_or(&mut self) {
        assert!(
            (self.n_any as usize) < MAX_OR,
            "at most {MAX_OR} `Or` filters per query"
        );
        self.any_end[self.n_any as usize] = self.n_terms;
        self.n_any += 1;
    }
}

pub(crate) struct Archetypes<'a> {
    pub(crate) all: &'a [Archetype],
    pub(crate) indices: Option<std::slice::Iter<'a, u32>>,
    pub(crate) next: usize,
}

impl<'a> Archetypes<'a> {
    /// The next archetype and its index in the world's archetype list.
    #[inline]
    pub(crate) fn next_indexed(&mut self) -> Option<(usize, &'a Archetype)> {
        let index = if let Some(indices) = &mut self.indices {
            *indices.next()? as usize
        } else {
            let index = self.next;
            self.next += 1;
            index
        };
        Some((index, self.all.get(index)?))
    }
}

impl<'a> Iterator for Archetypes<'a> {
    type Item = &'a Archetype;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.next_indexed().map(|(_, arch)| arch)
    }
}

/// One entry of a parallel query's work plan: rows `start..end` of archetype
/// `arch`, which are the query's positions `base..base + (end - start)`.
#[cfg(feature = "parallel")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ParTask {
    arch: usize,
    start: usize,
    end: usize,
    base: usize,
}

/// The storage of the parallel work plan, owned by the world and reused by
/// every parallel query, so a steady-state parallel query doesn't allocate.
/// Holds only archetype indices and row ranges between queries, never
/// component pointers.
///
/// It shrinks after a burst (a `Changed` query with many short runs, say):
/// every `PAR_PLAN_SHRINK_WINDOW` plans, storage over `PAR_PLAN_RETAIN`
/// entries that is more than four times the largest plan of that window is
/// cut to twice that plan (at least `PAR_PLAN_RETAIN`). Plans of a steady
/// size never reallocate.
#[cfg(feature = "parallel")]
#[derive(Default)]
pub(crate) struct ParPlan {
    tasks: Vec<ParTask>,
    /// The largest plan, and the number of plans, since the last check.
    peak: usize,
    plans: usize,
}

/// Plans between checks whether the plan storage can shrink.
#[cfg(feature = "parallel")]
const PAR_PLAN_SHRINK_WINDOW: usize = 64;
/// Plan entries kept whatever the recent plans used (32 bytes each).
#[cfg(feature = "parallel")]
const PAR_PLAN_RETAIN: usize = 4096;

#[cfg(feature = "parallel")]
impl ParPlan {
    /// The storage for a new plan, empty.
    fn take(&mut self) -> Vec<ParTask> {
        let mut tasks = std::mem::take(&mut self.tasks);
        tasks.clear();
        tasks
    }

    /// Takes the storage back after a plan ran.
    fn give_back(&mut self, mut tasks: Vec<ParTask>) {
        self.peak = self.peak.max(tasks.len());
        self.plans += 1;
        if self.plans >= PAR_PLAN_SHRINK_WINDOW {
            if tasks.capacity() > PAR_PLAN_RETAIN && tasks.capacity() / 4 > self.peak {
                tasks.clear();
                tasks.shrink_to((2 * self.peak).max(PAR_PLAN_RETAIN));
            }
            self.peak = 0;
            self.plans = 0;
        }
        self.tasks = tasks;
    }

    /// Entries the storage holds room for.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.tasks.capacity()
    }
}

/// Fewest rows per parallel chunk: cheap loops stay vectorized and
/// scheduling stays negligible. Tiny under Miri, so tests reach the
/// multi-chunk path with few rows.
#[cfg(feature = "parallel")]
const PAR_MIN_CHUNK: usize = if cfg!(miri) { 64 } else { 512 };
/// Most rows per parallel chunk, so big queries split into enough chunks to
/// balance uneven per-entity work.
#[cfg(feature = "parallel")]
const PAR_MAX_CHUNK: usize = if cfg!(miri) { 64 } else { 8192 };
/// A query whose archetypes are all small (none bigger than `PAR_MIN_CHUNK`
/// rows) used to run on the calling thread. It still does when its first
/// `PAR_MIN_CHUNK` rows, timed, project the whole query to under this many
/// nanoseconds: below that, waking other threads costs more than it saves.
/// The measured crossover is in `crates/ponko_bench/PAR_ARCHETYPES.md`.
/// Zero under Miri, so tests always reach the parallel path.
#[cfg(feature = "parallel")]
const PAR_SERIAL_BUDGET_NS: u128 = if cfg!(miri) { 0 } else { 50_000 };

/// A parallel query's work plan: its rows in query order, how many there
/// are, the chunk size to split them into (`None`: run on this thread), and
/// whether to time a first chunk on this thread before deciding.
#[cfg(feature = "parallel")]
struct Plan {
    tasks: Vec<ParTask>,
    total: usize,
    chunk: Option<usize>,
    probe: bool,
}

#[cfg(feature = "parallel")]
impl Plan {
    /// Runs the positions that stay on this thread, by calling
    /// `serial(lo, hi)`, and returns where the parallel part starts and its
    /// chunk size, if there is one.
    fn run_serial_part(&self, mut serial: impl FnMut(usize, usize)) -> Option<(usize, usize)> {
        let chunk = match self.chunk {
            None => {
                serial(0, self.total);
                return None;
            }
            Some(chunk) => chunk,
        };
        if !self.probe {
            return Some((0, chunk));
        }
        // The rows of small archetypes: time the first ones (as the old
        // path ran them, on this thread) and keep going here if the whole
        // query is cheap.
        let probe = PAR_MIN_CHUNK.min(self.total);
        let start = std::time::Instant::now();
        serial(0, probe);
        let projected = start.elapsed().as_nanos() * self.total as u128 / probe as u128;
        if projected < PAR_SERIAL_BUDGET_NS {
            serial(probe, self.total);
            return None;
        }
        Some((probe, chunk))
    }
}

/// What every thread of a parallel query reads: the archetypes, the plan
/// and the query's component ids.
#[cfg(feature = "parallel")]
struct ParWork<'a, Q: QueryData> {
    archetypes: &'a [Archetype],
    tasks: &'a [ParTask],
    ids: Q::Ids,
}

// SAFETY: every field is `Sync`: archetypes (whose components are
// `Send + Sync`), tasks, and `Q::Ids` (bounded `Send + Sync` by
// `QueryData`). Only the `Q` parameter keeps this from being automatic, and
// no `Q` value is stored. Fetching rows through the shared archetypes is what
// `ParWork::visit`'s contract covers; that `Q::fetch` may run on any thread
// is part of `QueryData`'s contract, which only this crate implements (the
// trait is sealed).
#[cfg(feature = "parallel")]
unsafe impl<Q: QueryData> Sync for ParWork<'_, Q> {}

#[cfg(feature = "parallel")]
impl<Q: QueryData> ParWork<'_, Q> {
    /// Calls `f(i, item)` for the query positions `lo..hi`, in order.
    ///
    /// # Safety
    /// The plan was built under the query's (still held) world borrow, whose
    /// access was validated, and no other thread visits any of these
    /// positions at the same time.
    #[inline(always)]
    unsafe fn visit(&self, lo: usize, hi: usize, mut f: impl FnMut(usize, Q::Item<'_>)) {
        // The task holding position `lo`: tasks are in position order.
        let mut t = self.tasks.partition_point(|t| t.base + (t.end - t.start) <= lo);
        let mut pos = lo;
        while pos < hi {
            let task = self.tasks[t];
            let state = Q::state(&self.archetypes[task.arch], &self.ids)
                .expect("a planned archetype matches the query while it runs");
            let first = task.start + (pos - task.base);
            let last = task.end.min(task.start + (hi - task.base));
            for row in first..last {
                // SAFETY: `row` is below the archetype's length (planned under
                // the same borrow) and only this thread visits it now.
                f(task.base + row - task.start, unsafe { Q::fetch(state, row) });
            }
            pos = task.base + (last - task.start);
            t += 1;
        }
    }
}

/// A typed query filter for [`Query::filter`]: [`With<T>`], [`Without<T>`],
/// [`Added<T>`](crate::Added), [`Changed<T>`](crate::Changed), [`Or`], or a
/// tuple of filters that must all hold (up to 8).
///
/// Name a filter you reuse across systems:
///
/// ```
/// # use ponko_ecs::*;
/// #[derive(Component)]
/// struct Health(i32);
/// #[derive(Component)]
/// struct Player;
/// #[derive(Component)]
/// struct Enemy;
/// #[derive(Component)]
/// struct Dead;
///
/// /// Living characters: players or enemies, not dead.
/// type Living = (Or<(With<Player>, With<Enemy>)>, Without<Dead>);
///
/// let mut world = World::new();
/// world.spawn((Health(10), Player));
/// world.spawn((Health(5), Enemy, Dead));
/// world.spawn((Health(1),));
///
/// assert_eq!(world.query::<&Health>().filter::<Living>().count(), 1);
/// ```
///
/// Implemented by this crate only: combine the provided filters.
pub trait QueryFilter {
    #[doc(hidden)]
    #[track_caller]
    fn add(world: &mut World, filters: &mut Filters);

    /// Adds the filter for a [`ViewQuery`](crate::ViewQuery): from a
    /// shared world, checked against the view's declaration.
    #[doc(hidden)]
    #[track_caller]
    fn add_shared(cx: &mut SharedFilters<'_>);
}

/// An arm of an [`Or`] filter: a provided filter or a tuple whose filters
/// must all hold. Tuples and `Or`s can nest within one another.
///
/// Implemented by this crate only: combine the provided filters.
pub trait OrTerm: QueryFilter {}

/// The terms of an [`Or`] filter: a tuple of 0 to 8 [`OrTerm`]s.
pub trait OrTerms {
    #[doc(hidden)]
    #[track_caller]
    fn add_terms(world: &mut World, filters: &mut Filters);

    #[doc(hidden)]
    #[track_caller]
    fn add_shared_terms(cx: &mut SharedFilters<'_>);
}

/// Filter: the entity has component `T` (it isn't fetched).
pub struct With<T>(PhantomData<fn() -> T>);

/// Filter: the entity doesn't have component `T`.
pub struct Without<T>(PhantomData<fn() -> T>);

/// Filter: at least one arm holds. A tuple arm requires all its filters:
/// `Or<((With<A>, With<B>), Without<C>)>` means "A and B, or no C".
/// Tuples (up to 8 filters) and `Or`s can nest.
///
/// Filters use inline storage. An `Or` is expanded into ANDed clauses:
/// `(A AND B) OR C` becomes `(A OR C) AND (B OR C)`. A query takes at
/// most 4 such clauses with 16 component predicates between them, including
/// repeated predicates introduced by expansion. Exceeding either limit panics.
/// `Or<()>` matches nothing; an empty tuple arm `Or<((), With<A>)>`
/// matches everything.
///
/// ```
/// # use ponko_ecs::*;
/// #[derive(Component)]
/// struct A;
/// #[derive(Component)]
/// struct B;
/// #[derive(Component)]
/// struct C;
/// let mut world = World::new();
/// world.spawn((A, B, C));
/// world.spawn((C,));
/// assert_eq!(world.query::<Entity>().filter::<Or<((With<A>, With<B>), Without<C>)>>().count(), 1);
/// ```
pub struct Or<T>(PhantomData<fn() -> T>);

impl<T: Component> QueryFilter for With<T> {
    fn add(world: &mut World, filters: &mut Filters) {
        filters.add_with(world.register::<T>());
    }
    fn add_shared(cx: &mut SharedFilters<'_>) {
        cx.with(cx.world.component_id::<T>());
    }
}

impl<T: Component> QueryFilter for Without<T> {
    fn add(world: &mut World, filters: &mut Filters) {
        filters.add_without(world.register::<T>());
    }
    fn add_shared(cx: &mut SharedFilters<'_>) {
        cx.without(cx.world.component_id::<T>());
    }
}

impl<T: OrTerms> QueryFilter for Or<T> {
    fn add(world: &mut World, filters: &mut Filters) {
        T::add_terms(world, filters);
    }
    fn add_shared(cx: &mut SharedFilters<'_>) {
        T::add_shared_terms(cx);
    }
}

impl<T: Component> OrTerm for With<T> {}
impl<T: Component> OrTerm for Without<T> {}
impl<T: OrTerms> OrTerm for Or<T> {}

impl QueryFilter for () {
    fn add(_: &mut World, _: &mut Filters) {}
    fn add_shared(_: &mut SharedFilters<'_>) {}
}
impl OrTerm for () {}
impl OrTerms for () {
    fn add_terms(_: &mut World, filters: &mut Filters) {
        filters.append_clauses(&Filters::never());
    }
    fn add_shared_terms(cx: &mut SharedFilters<'_>) {
        cx.append_clauses(&Filters::never());
    }
}

macro_rules! filter_tuple {
    ($($t:ident),*) => {
        impl<$($t: QueryFilter),*> QueryFilter for ($($t,)*) {
            fn add(world: &mut World, filters: &mut Filters) {
                $($t::add(world, filters);)*
            }
            fn add_shared(cx: &mut SharedFilters<'_>) {
                $($t::add_shared(cx);)*
            }
        }
        impl<$($t: OrTerm),*> OrTerm for ($($t,)*) {}
        impl<$($t: OrTerm),*> OrTerms for ($($t,)*) {
            fn add_terms(world: &mut World, filters: &mut Filters) {
                let mut union = Filters::never();
                $(
                    let mut arm = Filters::default();
                    $t::add(world, &mut arm);
                    union = union.disjoin(&arm.into_clauses());
                )*
                filters.append_clauses(&union);
            }
            fn add_shared_terms(cx: &mut SharedFilters<'_>) {
                let mut union = Filters::never();
                $(union = union.disjoin(&cx.or_arm::<$t>());)*
                cx.append_clauses(&union);
            }
        }
    };
}
filter_tuple!(A);
filter_tuple!(A, B);
filter_tuple!(A, B, C);
filter_tuple!(A, B, C, D);
filter_tuple!(A, B, C, D, E);
filter_tuple!(A, B, C, D, E, F);
filter_tuple!(A, B, C, D, E, F, G);
filter_tuple!(A, B, C, D, E, F, G, H);

/// A query over a [`World`], created by [`World::query`].
///
/// ```
/// # use ponko_ecs::*;
/// #[derive(Component)]
/// struct Pos(f32);
/// #[derive(Component)]
/// struct Vel(f32);
/// #[derive(Component)]
/// struct Frozen;
///
/// let mut world = World::new();
/// world.spawn((Pos(0.0), Vel(1.0)));
/// world.spawn((Pos(0.0), Vel(1.0), Frozen));
///
/// world.query::<(&mut Pos, &Vel)>().filter::<Without<Frozen>>().for_each(|(p, v)| p.0 += v.0);
///
/// let moved = world.query::<&Pos>().iter().filter(|p| p.0 > 0.0).count();
/// assert_eq!(moved, 1);
/// ```
#[must_use = "a query does nothing until iterated"]
pub struct Query<'w, Q: QueryData> {
    pub(crate) world: &'w mut World,
    pub(crate) ids: Q::Ids,
    filters: Filters,
    cache: QueryCache,
    /// What `Added`/`Changed` filters compare against, if not the world's
    /// [`changes_since`](World::changes_since).
    since: Option<Tick>,
}

impl<'w, Q: QueryData> Query<'w, Q> {
    pub(crate) fn new(world: &'w mut World) -> Self {
        let ids = Q::ids(world);
        check_aliasing::<Q>(world, &ids);
        world
            .recorder
            .note(|l| Q::access(&ids, &mut |id, write, _| l.component(id, write)));
        Query {
            world,
            ids,
            filters: Filters::default(),
            cache: QueryCache::Auto,
            since: None,
        }
    }

    /// The query's filters.
    pub(crate) fn filters(&self) -> Filters {
        self.filters
    }

    /// The world's current tick and this query's `since`, for change stamps.
    #[inline]
    fn clock(&self) -> Clock {
        let since = self.since.unwrap_or(self.world.changes_since());
        Clock::new(self.world.change_tick(), since)
    }

    /// Whether any component of the world is tracked. The loops test this
    /// before an archetype's own flag, so a world that tracks nothing doesn't
    /// even read that flag.
    #[inline]
    fn tracking(&self) -> bool {
        self.world.tracks_changes()
    }

    /// The archetypes to visit: every one, or the candidates the world's
    /// index holds for this query's signature (see [`QueryCachePolicy`]).
    /// The loops test their filters and resolve their columns either way.
    #[inline]
    fn archetypes(&mut self) -> Archetypes<'_> {
        // Field by field, never the world as a whole: a `Lookup` reads the
        // entity table and the archetypes through raw pointers while this
        // query loops, and a `&mut World` made here would end them.
        // The whole cost of the index for a world it doesn't apply to.
        // In a world with many empty archetypes, a scan visits the list of
        // those with entities (`World::slots`).
        let scan = self.world.slots.scan();
        if self.cache == QueryCache::Never || self.world.archetypes.len() < self.world.query_matches.min_archetypes() {
            return Archetypes {
                all: &self.world.archetypes,
                indices: scan.map(<[u32]>::iter),
                next: 0,
            };
        }
        let key = cache::signature::<Q>(&self.ids, &self.filters);
        self.world
            .query_matches
            .archetypes(&self.world.archetypes, &key, self.cache, scan)
    }

    /// Index this query's signature in the world now, instead of leaving it
    /// to the world's [`QueryCachePolicy`]: on its first use, and however
    /// many archetypes it matches.
    ///
    /// Most queries don't need this: a world with many archetypes (64 by
    /// default) indexes the signatures that repeat and that leave archetypes
    /// out, by itself. The hint is for a query the policy would skip, such
    /// as one that runs rarely but must not scan when it does. It changes
    /// no result: the same entities come back in the same order.
    ///
    /// The hint is ignored where the index isn't used at all: in worlds
    /// with fewer archetypes than
    /// [`min_archetypes`](QueryCachePolicy::min_archetypes), when the index
    /// is [off](QueryCachePolicy::off) or full. Remembering a signature and
    /// listing newly created archetypes may allocate; repeating a query
    /// doesn't.
    ///
    /// Only archetype indices are kept, never component pointers. Archetypes
    /// created later are matched before the next traversal, and an archetype
    /// that empties stays listed for when it is populated again. Optional
    /// components and mutability aren't part of the signature.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Health(i32);
    /// #[derive(Component)]
    /// struct Boss;
    /// let mut world = World::new();
    /// world.spawn((Health(100), Boss));
    /// assert_eq!(world.query::<&Health>().filter::<With<Boss>>().cached().count(), 1);
    /// ```
    pub fn cached(mut self) -> Self {
        self.cache = QueryCache::Always;
        self
    }

    /// Never use the world's archetype index for this query: test every
    /// archetype, as a world with the index
    /// [off](QueryCachePolicy::off) does for all of them. The results are
    /// the same; use it to compare, or for a query that is known to match
    /// nearly every archetype.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Health(i32);
    /// let mut world = World::new();
    /// world.spawn((Health(100),));
    /// assert_eq!(world.query::<&Health>().uncached().count(), 1);
    /// ```
    pub fn uncached(mut self) -> Self {
        self.cache = QueryCache::Never;
        self
    }

    /// Only match entities that pass the typed filter `F`: [`With`],
    /// [`Without`], [`Added`](crate::Added), [`Changed`](crate::Changed),
    /// [`Or`], or a tuple of them that must all hold. Calls combine: every
    /// filter must hold. `Added` and `Changed` pick single entities, and
    /// their component must be tracked (see [`World::track_changes`]); the
    /// others pick whole archetypes.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// #[derive(Component)]
    /// struct Player;
    /// #[derive(Component)]
    /// struct Frozen;
    /// let mut world = World::new();
    /// world.spawn((Pos(0.0), Player));
    /// world.spawn((Pos(0.0), Player, Frozen));
    ///
    /// // Has `Player` (not fetched), and no `Frozen`.
    /// let moving = world.query::<&Pos>().filter::<(With<Player>, Without<Frozen>)>().count();
    /// assert_eq!(moving, 1);
    /// ```
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// #[derive(Component)]
    /// struct Player;
    /// #[derive(Component)]
    /// struct Enemy;
    /// let mut world = World::new();
    /// world.spawn((Pos(0.0), Player));
    /// world.spawn((Pos(0.0), Enemy));
    /// world.spawn((Pos(0.0),));
    ///
    /// let n = world.query::<&Pos>().filter::<Or<(With<Player>, With<Enemy>)>>().count();
    /// assert_eq!(n, 2);
    /// ```
    ///
    /// # Panics
    ///
    /// With more than 8 different `With` or `Without` components, more than
    /// 4 expanded `Or` clauses, more than 16 terms across those clauses
    /// (see [`Or`]), or more than 4 `Added`/`Changed` filters outside an `Or`.
    /// With an `Added<T>` or `Changed<T>` whose `T` isn't tracked.
    #[track_caller]
    pub fn filter<F: QueryFilter>(mut self) -> Self {
        F::add(self.world, &mut self.filters);
        self
    }

    /// Makes [`Added`](crate::Added) and [`Changed`](crate::Changed) filters
    /// of this query report what happened after `tick`, instead of after the
    /// world's [`changes_since`](World::changes_since). Keep the
    /// [`Tick`] from [`World::advance_change_tick`] to continue where a
    /// reader left off; `Tick::default()` reports everything.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// let mut world = World::new();
    /// world.track_changes::<Pos>();
    /// world.spawn((Pos(0.0),));
    /// let seen = world.advance_change_tick();
    /// let moved = world.spawn((Pos(0.0),));
    /// world.get_mut::<Pos>(moved).unwrap().0 = 1.0;
    ///
    /// let mut changed = world.query::<Entity>().filter::<Changed<Pos>>().since(seen);
    /// assert_eq!(changed.iter().collect::<Vec<_>>(), [moved]);
    /// ```
    pub fn since(mut self, tick: Tick) -> Self {
        self.since = Some(tick);
        self
    }

    /// Runs `f` for every matching entity. The fastest way to loop.
    #[inline]
    pub fn for_each(&mut self, mut f: impl FnMut(Q::Item<'_>)) {
        if !self.tracking() {
            // The loop of a world that tracks nothing: kept apart from the
            // tracked one, so it is compiled as it would be without change
            // detection.
            let ids = self.ids;
            let filters = self.filters;
            for arch in self.archetypes() {
                let len = arch.len();
                if len == 0 || !filters.matches(arch) {
                    continue;
                }
                let Some(state) = Q::state(arch, &ids) else {
                    continue;
                };
                for row in 0..len {
                    f(unsafe { Q::fetch(state, row) });
                }
            }
            return;
        }
        self.for_each_tracked(f);
    }

    /// [`for_each`](Self::for_each) in a world that tracks changes: a
    /// tracked archetype stamps what the query may write, and with an
    /// `Added`/`Changed` filter yields runs of rows instead of all of them.
    ///
    /// Always inlined: `f` must not be handed to a call the compiler can't
    /// see into, or the variables it captures stop living in registers in
    /// the caller's untracked loop too.
    #[inline(always)]
    fn for_each_tracked(&mut self, mut f: impl FnMut(Q::Item<'_>)) {
        let ids = self.ids;
        let filters = self.filters;
        let clock = self.clock();
        for arch in self.archetypes() {
            let len = arch.len();
            if len == 0 || !filters.matches_tracked(arch) {
                continue;
            }
            let Some(state) = Q::state(arch, &ids) else {
                continue;
            };
            let mut runs = if arch.tracked() {
                change::visit::<Q>(arch, &ids, &filters, clock)
            } else {
                None
            };
            let (mut start, mut end) = (0, len);
            loop {
                if let Some(runs) = &mut runs {
                    match runs.next::<Q>(&ids) {
                        Some(run) => (start, end) = run,
                        None => break,
                    }
                }
                for row in start..end {
                    f(unsafe { Q::fetch(state, row) });
                }
                if runs.is_none() {
                    break;
                }
            }
        }
    }

    /// Like [`for_each`](Self::for_each), but spread over all CPU cores.
    ///
    /// The matching rows of every archetype are lined up in query order and
    /// cut into chunks of rows, and the chunks run in parallel: one chunk can
    /// hold many small archetypes or part of a big one, so many small
    /// archetypes use every core too. Use it for heavy per-entity work over
    /// many entities; for a few thousand cheap updates, `for_each` is just as
    /// fast. A query whose archetypes are all small times its first rows and
    /// stays on the calling thread if the whole loop would take under about
    /// 50 µs.
    ///
    /// `f` can't capture `&mut` state (it runs on many threads at once). To
    /// spawn or despawn, collect what you need and use commands afterwards.
    ///
    /// Which entities run together, on which thread and in what order is not
    /// fixed. For small-archetype queries even the choice between one thread
    /// and many depends on measured time, so it can change from run to run on
    /// small worlds too. Side effects that depend on order (pushing into a
    /// shared `Vec`, summing floats into shared state) are not reproducible;
    /// per-entity results, the positions of
    /// [`par_for_each_indexed`](Self::par_for_each_indexed) and the order of
    /// [`par_collect_into`](Self::par_collect_into) are.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// #[derive(Component)]
    /// struct Vel(f32);
    /// let mut world = World::new();
    /// world.spawn_batch((0..100_000).map(|i| (Pos(i as f32), Vel(1.0))));
    ///
    /// world.query::<(&mut Pos, &Vel)>().par_for_each(|(p, v)| p.0 += v.0);
    /// ```
    #[cfg(feature = "parallel")]
    pub fn par_for_each(&mut self, f: impl Fn(Q::Item<'_>) + Send + Sync) {
        self.par_for_each_indexed(|_, item| f(item));
    }

    /// Like [`par_for_each`](Self::par_for_each), but also passes each
    /// entity's position in the query, `0..count()`, in the same order as
    /// [`for_each`](Self::for_each) and [`iter`](Self::iter).
    ///
    /// Use it to fill a buffer from all cores: every entity writes its own
    /// slot, so the result comes out in a stable order, whichever threads
    /// ran it (see [`par_for_each`](Self::par_for_each) for what isn't fixed).
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// use std::sync::atomic::{AtomicU32, Ordering};
    ///
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// let mut world = World::new();
    /// world.spawn_batch((0..10_000).map(|i| (Pos(i as f32),)));
    ///
    /// let mut query = world.query::<&Pos>();
    /// let out: Vec<AtomicU32> = (0..query.count()).map(|_| AtomicU32::new(0)).collect();
    /// query.par_for_each_indexed(|i, p| out[i].store(p.0 as u32 * 2, Ordering::Relaxed));
    /// assert_eq!(out[1234].load(Ordering::Relaxed), 2468);
    /// ```
    #[cfg(feature = "parallel")]
    pub fn par_for_each_indexed(&mut self, f: impl Fn(usize, Q::Item<'_>) + Send + Sync) {
        let plan = self.par_plan();
        self.par_run(plan, f);
    }

    /// Lines up the matching rows of every archetype for a parallel loop, in
    /// the order of [`for_each`](Self::for_each), and decides how to split
    /// them. As in `for_each`, stamps are written and row filters are read
    /// here, on the calling thread, before any row is shared.
    ///
    /// Takes the world's plan storage; [`par_run`](Self::par_run) returns it.
    #[cfg(feature = "parallel")]
    fn par_plan(&mut self) -> Plan {
        let mut tasks = self.world.par_plan.take();
        let ids = self.ids;
        let filters = self.filters;
        let clock = self.clock();
        let tracking = self.tracking();
        let (mut total, mut longest) = (0, 0);
        let mut archetypes = self.archetypes();
        while let Some((arch_index, arch)) = archetypes.next_indexed() {
            let len = arch.len();
            if len == 0 || !filters.matches(arch) {
                continue;
            }
            // `Q::state` is `Some` exactly when every required component is
            // here (`QueryData`'s contract): check that without resolving
            // columns, which `ParWork::visit` does once per chunk.
            let mut has_required = true;
            Q::access(&ids, &mut |id, _, required| has_required &= !required || arch.has(id));
            if !has_required {
                continue;
            }
            let mut push = |start: usize, end: usize| {
                tasks.push(ParTask {
                    arch: arch_index,
                    start,
                    end,
                    base: total,
                });
                total += end - start;
                longest = longest.max(end - start);
            };
            let runs = if tracking && arch.tracked() {
                change::visit::<Q>(arch, &ids, &filters, clock)
            } else {
                None
            };
            match runs {
                None => push(0, len),
                Some(mut runs) => {
                    while let Some((start, end)) = runs.next::<Q>(&ids) {
                        push(start, end);
                    }
                }
            }
        }
        // About 4 chunks per thread, so heavy per-entity work (neighbor
        // search) uses every core even for a few thousand entities, but at
        // least `PAR_MIN_CHUNK` rows so cheap loops stay vectorized and
        // scheduling stays negligible.
        let threads = rayon::current_num_threads().max(1);
        let chunk = (total / (threads * 4)).clamp(PAR_MIN_CHUNK, PAR_MAX_CHUNK);
        // Rows of small archetypes only (none bigger than `PAR_MIN_CHUNK`)
        // used to run on the calling thread: time the first ones to decide
        // (see `Plan::run_serial_part`). The web has no clock to time them
        // with, so they keep running here.
        let small = longest <= PAR_MIN_CHUNK;
        let serial = total <= chunk || (small && cfg!(target_family = "wasm"));
        Plan {
            tasks,
            total,
            chunk: (!serial).then_some(chunk),
            probe: small,
        }
    }

    /// Calls `f(i, item)` for every position `i` of `plan`, on all cores if
    /// the plan has a chunk size, then gives the plan storage back to the world.
    #[cfg(feature = "parallel")]
    fn par_run(&mut self, plan: Plan, f: impl Fn(usize, Q::Item<'_>) + Send + Sync) {
        use rayon::prelude::*;
        let work = ParWork::<Q> {
            archetypes: &self.world.archetypes,
            tasks: &plan.tasks,
            ids: self.ids,
        };
        let total = plan.total;
        // SAFETY: the plan was built under this query's world borrow, whose
        // access was validated; the serial part runs before the parallel one.
        let parallel = plan.run_serial_part(|lo, hi| unsafe { work.visit(lo, hi, |i, item| f(i, item)) });
        if let Some((from, chunk)) = parallel {
            (0..(total - from).div_ceil(chunk)).into_par_iter().for_each(|c| {
                let lo = from + c * chunk;
                // SAFETY: as above; chunks are disjoint ranges of positions,
                // and every position is one row of one archetype.
                unsafe { work.visit(lo, (lo + chunk).min(total), |i, item| f(i, item)) };
            });
        }
        self.world.par_plan.give_back(plan.tasks);
    }

    /// Replaces the contents of `out` with `f` of every matching entity,
    /// computed on all cores, in the same order as [`for_each`](Self::for_each).
    /// `out` keeps its memory between calls, so collecting every tick
    /// allocates nothing once it's big enough. If `f` panics, completed results
    /// are dropped and `out` is left empty with its allocation available for reuse.
    ///
    /// The order of `out` is fixed; the threads that call `f`, and the order
    /// of those calls, are not (see [`par_for_each`](Self::par_for_each)).
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// let mut world = World::new();
    /// world.spawn_batch((0..10_000).map(|i| (Pos(i as f32),)));
    ///
    /// let mut doubled = Vec::new();
    /// world.query::<&Pos>().par_collect_into(&mut doubled, |p| p.0 * 2.0);
    /// assert_eq!(doubled[1234], 2468.0);
    /// ```
    #[cfg(feature = "parallel")]
    pub fn par_collect_into<R: Send>(&mut self, out: &mut Vec<R>, f: impl Fn(Q::Item<'_>) -> R + Send + Sync) {
        /// `out`'s buffer, shared with worker threads.
        struct Slots<R>(*mut R);
        impl<R> Clone for Slots<R> {
            fn clone(&self) -> Self {
                *self
            }
        }
        impl<R> Copy for Slots<R> {}
        impl<R> Slots<R> {
            /// A method, so closures capture the whole `Slots` (which is
            /// `Sync`), not its raw pointer field.
            fn at(self, i: usize) -> *mut R {
                // SAFETY (of callers): `i` is within the reserved capacity.
                self.0.wrapping_add(i)
            }
        }
        // SAFETY: each query index is written by exactly one task.
        unsafe impl<R: Send> Send for Slots<R> {}
        unsafe impl<R: Send> Sync for Slots<R> {}

        out.clear();
        let plan = self.par_plan();
        let n = plan.total;
        out.reserve(n);
        let slots = Slots(out.as_mut_ptr());
        if !std::mem::needs_drop::<R>() {
            self.par_run(plan, |i, item| {
                debug_assert!(i < n);
                // SAFETY: every index in the reserved buffer is written once.
                // On panic these values need no cleanup and `out.len()` stays 0.
                unsafe { slots.at(i).write(f(item)) };
            });
        } else {
            /// Owns only the initialized prefix of a disjoint output range.
            struct Initialized<R> {
                slots: Slots<R>,
                len: usize,
            }
            impl<R> Drop for Initialized<R> {
                fn drop(&mut self) {
                    // SAFETY: only this guard owns these initialized values;
                    // sibling tasks write disjoint ranges. Vec's length is 0.
                    unsafe { std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.slots.at(0), self.len)) };
                }
            }

            // Return ownership of completed ranges through `join`: if either
            // branch panics, rayon waits for its sibling and drops its result.
            // No shared counter, bitmap, or per-call allocation is needed.
            // `fill` covers positions `start..start + len`; `leaf` fills a
            // range that is at most one chunk long, recording each write.
            fn fill<R: Send>(
                mut initialized: Initialized<R>,
                start: usize,
                len: usize,
                chunk: usize,
                leaf: &(impl Fn(usize, usize, &mut Initialized<R>) + Sync),
            ) -> Initialized<R> {
                if len > chunk {
                    let mid = (len.div_ceil(chunk) / 2) * chunk;
                    let right = Initialized {
                        slots: Slots(initialized.slots.at(mid)),
                        len: 0,
                    };
                    let (mut left, mut right) = rayon::join(
                        || fill(initialized, start, mid, chunk, leaf),
                        || fill(right, start + mid, len - mid, chunk, leaf),
                    );
                    // Both adjacent ranges are complete; transfer ownership.
                    left.len += right.len;
                    right.len = 0;
                    left
                } else {
                    leaf(start, start + len, &mut initialized);
                    initialized
                }
            }

            let work = ParWork::<Q> {
                archetypes: &self.world.archetypes,
                tasks: &plan.tasks,
                ids: self.ids,
            };
            let leaf = |lo: usize, hi: usize, initialized: &mut Initialized<R>| {
                // SAFETY: the plan was built under this query's world borrow,
                // whose access was validated; the serial part and `fill` hand
                // out disjoint ranges, and every position is one row of one
                // archetype.
                unsafe {
                    work.visit(lo, hi, |_, item| {
                        let value = f(item);
                        // SAFETY: this task exclusively owns these reserved
                        // slots. Record each write before calling `f` again.
                        initialized.slots.at(initialized.len).write(value);
                        initialized.len += 1;
                    })
                }
            };
            // The serial part fills a prefix of `out`, the parallel part the rest.
            let mut initialized = Initialized { slots, len: 0 };
            let parallel = plan.run_serial_part(|lo, hi| leaf(lo, hi, &mut initialized));
            if let Some((from, chunk)) = parallel {
                debug_assert_eq!(initialized.len, from);
                let rest = Initialized {
                    slots: Slots(slots.at(from)),
                    len: 0,
                };
                let mut rest = fill(rest, from, n - from, chunk, &leaf);
                initialized.len += rest.len;
                rest.len = 0;
            }
            debug_assert_eq!(initialized.len, n);
            // All results now belong to `out`; disarm before setting its length.
            initialized.len = 0;
            self.world.par_plan.give_back(plan.tasks);
        }
        // SAFETY: both paths wrote all n slots; unwinding never reaches here.
        unsafe { out.set_len(n) };
    }

    /// An iterator over matching entities, for `map`/`filter`/`sum` and friends.
    ///
    /// Convenient, but several times slower than [`for_each`](Self::for_each)
    /// on tight math loops (the compiler can't vectorize across archetypes).
    /// Use `for_each` for per-tick simulation work.
    pub fn iter(&mut self) -> QueryIter<'_, Q> {
        let ids = self.ids;
        let filters = self.filters;
        let clock = self.clock();
        let tracking = self.tracking();
        QueryIter {
            archetypes: self.archetypes(),
            ids,
            filters,
            state: None,
            row: 0,
            len: 0,
            clock,
            tracking,
            filtered: None,
            _q: PhantomData,
        }
    }

    /// Number of matching entities. It doesn't visit them one by one, except
    /// to test [`Added`](crate::Added) and [`Changed`](crate::Changed)
    /// filters, and it doesn't count as a change to anything.
    pub fn count(&mut self) -> usize {
        let ids = self.ids;
        let filters = self.filters;
        if !self.tracking() {
            return self
                .archetypes()
                .filter(|a| !a.is_empty() && filters.matches(a) && Q::state(a, &ids).is_some())
                .map(|a| a.len())
                .sum();
        }
        let clock = self.clock();
        self.archetypes()
            .filter(|a| !a.is_empty() && filters.matches(a) && Q::state(a, &ids).is_some())
            .map(|a| {
                if a.tracked() {
                    change::count(a, &filters, clock)
                } else {
                    a.len()
                }
            })
            .sum()
    }

    /// The query's item for one entity, or `None` if it's dead or doesn't match.
    pub fn get(&mut self, e: Entity) -> Option<Q::Item<'_>> {
        let ids = self.ids;
        let filters = self.filters;
        let loc = self.world.entities.get(e)?;
        let arch = &self.world.archetypes[loc.archetype as usize];
        if !filters.matches(arch) {
            return None;
        }
        let state = Q::state(arch, &ids)?;
        if self.tracking()
            && arch.tracked()
            && !change::visit_row::<Q>(arch, &ids, &filters, self.clock(), loc.row as usize)
        {
            return None;
        }
        Some(unsafe { Q::fetch(state, loc.row as usize) })
    }

    /// The item of the only matching entity: for the things a game has one
    /// of, such as the player or the camera's target. `Err` says whether
    /// none matched or several did.
    ///
    /// It ends the query, so the item can be kept for as long as the world
    /// stays borrowed.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// #[derive(Component)]
    /// struct Player;
    /// let mut world = World::new();
    /// assert_eq!(world.query::<&Pos>().single().err(), Some(SingleError::NoEntities));
    /// world.spawn((Pos(1.0), Player));
    /// world.spawn((Pos(2.0),));
    ///
    /// let pos = world.query::<&mut Pos>().filter::<With<Player>>().single().unwrap();
    /// pos.0 += 10.0;
    /// assert_eq!(world.query::<&Pos>().single().err(), Some(SingleError::MultipleEntities));
    /// ```
    ///
    /// Nothing counts as changed when it returns `Err`. With `Ok`, a
    /// `&mut T` in the query counts as a change, as in every query.
    ///
    /// # Errors
    /// If no entity matches, or more than one does.
    pub fn single(mut self) -> Result<Q::Item<'w>, SingleError>
    where
        Q: 'w,
    {
        let ids = self.ids;
        let filters = self.filters;
        let clock = self.clock();
        let tracking = self.tracking();
        // One pass over the archetypes the query matches (through the
        // world's index where it applies), stopping at a second match.
        let index = single_archetype::<Q>(self.archetypes(), &ids, &filters, tracking, clock)?;
        // The query is over: what it borrowed of the world goes to the
        // item, read from the one archetype that holds it.
        let world: &'w World = self.world;
        let mut matches = QueryIter::<'w, Q> {
            archetypes: Archetypes {
                all: &world.archetypes,
                indices: None,
                next: index,
            },
            ids,
            filters,
            state: None,
            row: 0,
            len: 0,
            clock,
            tracking,
            filtered: None,
            _q: PhantomData,
        };
        matches.next().ok_or(SingleError::NoEntities)
    }
}

/// The world's index of the archetype that holds the query's only match,
/// for [`Query::single`] and [`ViewQuery::single`](crate::ViewQuery::single).
/// It stops at a second match.
pub(crate) fn single_archetype<Q: QueryData>(
    mut archetypes: Archetypes<'_>,
    ids: &Q::Ids,
    filters: &Filters,
    tracking: bool,
    clock: Clock,
) -> Result<usize, SingleError> {
    let mut found = None;
    while let Some((index, arch)) = archetypes.next_indexed() {
        if arch.is_empty() || !filters.matches(arch) || Q::state(arch, ids).is_none() {
            continue;
        }
        let matches = if tracking && arch.tracked() {
            change::count_at_most(arch, filters, clock, 2)
        } else {
            arch.len().min(2)
        };
        match (matches, found) {
            (0, _) => {}
            (1, None) => found = Some(index),
            _ => return Err(SingleError::MultipleEntities),
        }
    }
    found.ok_or(SingleError::NoEntities)
}

/// Why [`Query::single`] has no item: the query didn't match exactly one
/// entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SingleError {
    /// No entity matches the query.
    NoEntities,
    /// More than one entity matches the query. The query stops looking at
    /// the second match, so how many match is not known.
    MultipleEntities,
}

impl std::fmt::Display for SingleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SingleError::NoEntities => f.write_str("no entity matches the query"),
            SingleError::MultipleEntities => f.write_str("more than one entity matches the query"),
        }
    }
}

impl std::error::Error for SingleError {}

/// Iterator returned by [`Query::iter`].
pub struct QueryIter<'a, Q: QueryData> {
    pub(crate) archetypes: Archetypes<'a>,
    pub(crate) ids: Q::Ids,
    pub(crate) filters: Filters,
    pub(crate) state: Option<Q::State>,
    pub(crate) row: usize,
    pub(crate) len: usize,
    pub(crate) clock: Clock,
    /// Whether the world tracks any component at all.
    pub(crate) tracking: bool,
    /// The current archetype, when an `Added`/`Changed` filter picks its
    /// rows: they then come in runs, and `len` is the current run's end.
    pub(crate) filtered: Option<&'a Archetype>,
    pub(crate) _q: PhantomData<&'a mut Q>,
}

impl<'a, Q: QueryData> Iterator for QueryIter<'a, Q> {
    type Item = Q::Item<'a>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(state) = self.state
                && self.row < self.len
            {
                let row = self.row;
                self.row += 1;
                return Some(unsafe { Q::fetch(state, row) });
            }
            if let Some(arch) = self.filtered {
                // The next run starts at or after the end of this one.
                match change::next_run::<Q>(arch, self.ids, self.filters, self.clock, self.len) {
                    Some((start, end)) => {
                        self.row = start;
                        self.len = end;
                        continue;
                    }
                    None => self.filtered = None,
                }
            }
            let arch = self.archetypes.next()?;
            self.state = if arch.is_empty() || !self.filters.matches(arch) {
                None
            } else {
                Q::state(arch, &self.ids)
            };
            self.row = 0;
            self.len = arch.len();
            if self.tracking
                && arch.tracked()
                && self.state.is_some()
                && change::enter::<Q>(arch, self.ids, self.filters, self.clock)
            {
                // The rows come from runs.
                self.filtered = Some(arch);
                self.len = 0;
            }
        }
    }
}

#[cfg(all(test, feature = "parallel"))]
mod par_plan_tests {
    use super::*;
    use crate::{Changed, Component};

    #[derive(Component)]
    struct Value(u32);

    /// A tracked world where every other row changed since the returned
    /// tick: a `Changed` query plans one entry per changed row.
    fn many_runs(rows: u32) -> (World, Tick) {
        let mut w = World::new();
        w.track_changes::<Value>();
        let entities: Vec<Entity> = (0..rows).map(|i| w.spawn((Value(i),))).collect();
        let seen = w.advance_change_tick();
        for e in entities.iter().step_by(2) {
            w.get_mut::<Value>(*e).unwrap().0 += 1;
        }
        w.advance_change_tick();
        (w, seen)
    }

    #[test]
    fn plan_storage_shrinks_after_a_burst_of_many_runs() {
        let runs = 4 * PAR_PLAN_RETAIN;
        let (mut w, seen) = many_runs(2 * runs as u32);
        w.query::<&Value>()
            .filter::<Changed<Value>>()
            .since(seen)
            .par_for_each(|v| assert!(v.0 % 2 == 1));
        let big = w.par_plan.capacity();
        assert!(big >= runs, "one entry per run: {big}");
        // A whole window of small plans (the burst's own window ends first)
        // gives the storage back.
        for _ in 0..2 * PAR_PLAN_SHRINK_WINDOW {
            w.query::<&Value>().par_for_each(|v| {
                std::hint::black_box(v.0);
            });
        }
        let small = w.par_plan.capacity();
        assert!(small <= PAR_PLAN_RETAIN, "{big} entries shrank to {small}");
        assert!(small >= 1, "the storage stays usable");
    }

    #[test]
    fn plan_storage_of_a_steady_size_is_kept() {
        let (mut w, seen) = many_runs(2 * 4 * PAR_PLAN_RETAIN as u32);
        let q = |w: &mut World| {
            w.query::<&Value>()
                .filter::<Changed<Value>>()
                .since(seen)
                .par_for_each(|v| assert!(v.0 % 2 == 1));
        };
        q(&mut w);
        let capacity = w.par_plan.capacity();
        let buffer = w.par_plan.tasks.as_ptr();
        for _ in 0..3 * PAR_PLAN_SHRINK_WINDOW {
            q(&mut w);
        }
        assert_eq!(w.par_plan.capacity(), capacity);
        assert_eq!(w.par_plan.tasks.as_ptr(), buffer);
    }
}
