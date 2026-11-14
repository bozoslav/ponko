//! Change detection: which components were added, changed or removed, and when.
//!
//! It is opt-in per component, and always explicit: call
//! [`World::track_changes`] for each component whose changes matter. Nothing
//! starts tracking on its own; an [`Added`] or [`Changed`] filter on a
//! component that isn't tracked panics and says so. A world that tracks
//! nothing keeps no stamps and does no extra work: every hook sits behind
//! one flag per archetype.
//!
//! # How it works
//!
//! The world counts [`Tick`]s. A tracked component keeps two stamps per
//! entity: the tick it was added and the tick it last changed. A reader
//! remembers the tick it last looked and asks for everything stamped after
//! it, so readers never clear anything and can't hide changes from each other.
//!
//! # What counts as a change
//!
//! | Operation | Added | Changed |
//! |---|---|---|
//! | `spawn`, `spawn_batch`, `spawn_at`, `spawn_dynamic`, `spawn_at_dynamic` | yes | yes |
//! | `insert`, `insert_dynamic` of a component the entity lacks | yes | yes |
//! | `insert`, `insert_dynamic` replacing a value | no | yes |
//! | `get_mut` | no | yes |
//! | a query fetching `&mut T` or `Option<&mut T>` | no | yes, every entity it visits |
//! | the same in a `WorldView` query, and its `get_mut` | no | yes |
//! | `for_each_chunk` | no | yes, every requested column of every chunk |
//! | `mark_changed`, `mark_changed_dynamic` | no | yes |
//! | `get`, `has`, `&T` in a query, `count`, `sort_by` | no | no |
//! | `get_dynamic`, `get_dynamic_mut` (raw pointers) | no | no: call `mark_changed_dynamic` after writing |
//! | `remove`, `remove_dynamic`, `despawn`, `clear`, `reset` | logged as a removal; the entity's other components keep their stamps |
//!
//! A mutable borrow counts as a change whether or not the value is written:
//! the world can't see writes through a `&mut T`, so it assumes one. To
//! change only some entities, read with `&T` and use `get_mut` (or
//! [`Commands`](crate::Commands)) for the ones that need it.
//!
//! Commands are stamped when they are applied, not when they are recorded.
//! Stamps and removal logs aren't saved: restoring a world spawns its
//! entities again, so everything in it counts as added.

use std::marker::PhantomData;

use crate::bundle::Component;
use crate::entity::Entity;
use crate::query::{Filters, OrTerm, QueryCache, QueryData, QueryFilter};
use crate::storage::{Archetype, ComponentId, ComponentInfo};
use crate::view::SharedFilters;
use crate::world::{Chunk, World};

/// A point in a world's change history, from [`World::change_tick`] or
/// [`World::advance_change_tick`].
///
/// A reader keeps the tick at which it last looked and passes it to
/// [`Query::since`](crate::Query::since) or [`World::removed_since`] to see
/// what happened after it. `Tick::default()` is before everything: every
/// tracked value counts as added and changed since then.
///
/// Ticks belong to one world and one run: don't save them or use them with
/// another world. A tick the world hasn't reached yet (which can only come
/// from another world) is treated like `Tick::default()`: readers are shown
/// everything rather than nothing.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Tick(pub(crate) u64);

impl Tick {
    /// The tick as a plain number, for C/C++ and tools.
    #[must_use]
    pub fn to_bits(self) -> u64 {
        self.0
    }

    /// A tick from its number, as returned by [`to_bits`](Self::to_bits).
    #[must_use]
    pub fn from_bits(bits: u64) -> Self {
        Tick(bits)
    }
}

/// Filter: component `T` was added to the entity since the reader last
/// looked: the entity was spawned with it, or it was inserted on an entity
/// that didn't have one. Replacing a value isn't adding one.
///
/// `T`'s changes must be tracked: call [`World::track_changes`] first (in a
/// game, in setup). The filter panics on a component that isn't, in a world
/// query and in a [`WorldView`](crate::WorldView) query alike.
///
/// ```
/// # use ponko_ecs::*;
/// #[derive(Component)]
/// struct Health(i32);
/// let mut world = World::new();
/// world.track_changes::<Health>();
/// world.spawn((Health(10),));
/// let seen = world.advance_change_tick();
/// let fresh = world.spawn((Health(5),));
///
/// let mut added = world.query::<Entity>().filter::<Added<Health>>().since(seen);
/// assert_eq!(added.iter().collect::<Vec<_>>(), [fresh]);
/// ```
pub struct Added<T>(PhantomData<fn() -> T>);

/// Filter: component `T` was added or changed since the reader last looked.
/// See the [list of what counts as a change](World::track_changes).
///
/// `T`'s changes must be tracked: call [`World::track_changes`] first (in a
/// game, in setup). The filter panics on a component that isn't, in a world
/// query and in a [`WorldView`](crate::WorldView) query alike. Combine
/// filters as usual: `Or<(Changed<A>, Changed<B>)>` matches when either
/// changed.
///
/// A query that fetches `&mut T` marks `T` changed for every entity it
/// visits, written or not. So a system that filters on `Changed<T>` and
/// fetches `&mut T` keeps every entity it sees changed for every other
/// reader; read with `&T` and use [`World::get_mut`] where a value really
/// changes.
///
/// ```
/// # use ponko_ecs::*;
/// #[derive(Component)]
/// struct Pos(f32);
/// #[derive(Component)]
/// struct Vel(f32);
/// let mut world = World::new();
/// world.track_changes::<Pos>();
/// let still = world.spawn((Pos(0.0),));
/// let moving = world.spawn((Pos(0.0), Vel(1.0)));
/// let seen = world.advance_change_tick();
///
/// // Only entities with a `Vel` are borrowed mutably, so only they change.
/// world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| p.0 += v.0);
///
/// let mut changed = world.query::<Entity>().filter::<Changed<Pos>>().since(seen);
/// assert_eq!(changed.iter().collect::<Vec<_>>(), [moving]);
/// # let _ = still;
/// ```
pub struct Changed<T>(PhantomData<fn() -> T>);

/// The id of `T` for an [`Added`] or [`Changed`] filter named `filter`:
/// `T` must be tracked already. Records the filter as reading `T` for
/// declared-access checks (its stamps are written by whoever writes `T`).
#[track_caller]
fn filtered<T: Component>(world: &mut World, filter: &str) -> ComponentId {
    let id = world.register::<T>();
    if !world.infos[id.index()].tracked {
        untracked_filter(None, filter, std::any::type_name::<T>());
    }
    world.recorder.note(|l| l.component(id, false));
    id
}

/// The panic of an `Added`/`Changed` filter (`filter`) on component `name`,
/// which isn't tracked: the same in a world query and in a view's (which
/// names itself with `label`).
#[cold]
#[track_caller]
pub(crate) fn untracked_filter(label: Option<&str>, filter: &str, name: &str) -> ! {
    let who = label.map(|label| format!("{label}: ")).unwrap_or_default();
    panic!(
        "{who}`{filter}<{name}>` needs `{name}`'s changes tracked; \
         call `world.track_changes::<{name}>()` first (in a game: in setup or a `&mut Ctx` system)"
    );
}

impl<T: Component> QueryFilter for Added<T> {
    #[track_caller]
    fn add(world: &mut World, filters: &mut Filters) {
        filters.add_tick(filtered::<T>(world, "Added"), true);
    }
    fn add_shared(cx: &mut SharedFilters<'_>) {
        cx.tick::<T>("Added", true);
    }
}

impl<T: Component> QueryFilter for Changed<T> {
    #[track_caller]
    fn add(world: &mut World, filters: &mut Filters) {
        filters.add_tick(filtered::<T>(world, "Changed"), false);
    }
    fn add_shared(cx: &mut SharedFilters<'_>) {
        cx.tick::<T>("Changed", false);
    }
}

impl<T: Component> OrTerm for Added<T> {}
impl<T: Component> OrTerm for Changed<T> {}

/// Stamps older than this many ticks are aged to exactly this old, so their
/// 32 bits never wrap. A reader that last looked longer ago than this sees
/// every tracked value as changed (never fewer than really changed).
const MAX_AGE: u32 = 1 << 31;
/// Stamps are aged every this many ticks. Between two passes a stamp gets at
/// most `MAX_AGE + AGE_EVERY` ticks old, which still fits in 32 bits.
const AGE_EVERY: u64 = 1 << 30;

/// The world's change-detection state.
pub(crate) struct Changes {
    /// The current tick. Stamps written now carry its low 32 bits. It starts
    /// at 1: tick 0 is `Tick::default()`, before everything.
    now: u64,
    /// What `Added`/`Changed` filters and `removed` compare against, unless
    /// the caller names a tick.
    since: u64,
    /// The tick of the last aging pass.
    aged_at: u64,
    /// One log per tracked component, in the order they were tracked.
    removed: Vec<Removed>,
}

/// Entities that lost a tracked component, oldest first.
struct Removed {
    id: ComponentId,
    entities: Vec<Entity>,
    /// `ticks[i]` is when `entities[i]` lost it; ascending.
    ticks: Vec<u64>,
}

impl Default for Changes {
    fn default() -> Self {
        Changes {
            now: 1,
            since: 0,
            aged_at: 1,
            removed: Vec::new(),
        }
    }
}

impl Changes {
    /// The stamp of a change made now.
    #[inline(always)]
    pub(crate) fn stamp(&self) -> u32 {
        self.now as u32
    }

    /// Logs that `e` lost tracked component `id`.
    pub(crate) fn log_removal(&mut self, id: ComponentId, e: Entity) {
        if let Some(log) = self.removed.iter_mut().find(|log| log.id == id) {
            log.entities.push(e);
            log.ticks.push(self.now);
        }
    }

    /// Logs that `e`, a row of `arch`, is being despawned.
    #[cold]
    pub(crate) fn log_despawn(&mut self, infos: &[ComponentInfo], arch: &Archetype, e: Entity) {
        for &id in arch.component_ids() {
            if infos[id.index()].tracked {
                self.log_removal(id, e);
            }
        }
    }
}

/// The current tick and a reader's last-seen tick, as stamps compare them.
#[derive(Clone, Copy)]
pub(crate) struct Clock {
    /// The current tick's low 32 bits: what a change made now is stamped with.
    now: u32,
    /// A stamp is newer than the reader's tick if it is less than this many
    /// ticks old. Ages fit in 32 bits (see `MAX_AGE`), so `1 << 32` means
    /// every stamp.
    newer_than: u64,
}

impl Clock {
    #[inline]
    pub(crate) fn new(now: Tick, since: Tick) -> Self {
        // A `since` this world hasn't reached is another world's tick:
        // report everything rather than nothing.
        let age = now.0.checked_sub(since.0).unwrap_or(u64::MAX);
        Clock {
            now: now.0 as u32,
            newer_than: age.min(1 << 32),
        }
    }

    #[inline(always)]
    fn is_newer(self, stamp: u32) -> bool {
        u64::from(self.now.wrapping_sub(stamp)) < self.newer_than
    }
}

const MAX_TERMS: usize = 20;
const MAX_CLAUSES: usize = 8;

/// Which rows of one archetype pass a query's `Added`/`Changed` filters:
/// every clause must hold, and a clause holds when any of its terms (a
/// stamp array) is newer than the reader's tick.
#[derive(Clone, Copy)]
pub(crate) struct RowFilter {
    terms: [*const u32; MAX_TERMS],
    /// Clause `c` holds the terms `terms[ends[c - 1]..ends[c]]`.
    ends: [u8; MAX_CLAUSES],
    n_terms: u8,
    n_clauses: u8,
    clock: Clock,
}

impl RowFilter {
    pub(crate) fn new(clock: Clock) -> Self {
        RowFilter {
            terms: [std::ptr::null(); MAX_TERMS],
            ends: [0; MAX_CLAUSES],
            n_terms: 0,
            n_clauses: 0,
            clock,
        }
    }

    /// Adds a term to the clause being built: the live stamps of one column.
    pub(crate) fn term(&mut self, stamps: *const u32) {
        self.terms[self.n_terms as usize] = stamps;
        self.n_terms += 1;
    }

    /// Closes the clause being built.
    pub(crate) fn close(&mut self) {
        self.ends[self.n_clauses as usize] = self.n_terms;
        self.n_clauses += 1;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.n_clauses == 0
    }

    /// Whether `row` passes. `row` must be a live row of the archetype the
    /// terms point into.
    #[inline]
    fn passes(&self, row: usize) -> bool {
        let mut start = 0;
        for &end in &self.ends[..self.n_clauses as usize] {
            let end = end as usize;
            // SAFETY: each term points at one stamp per live row.
            let any = self.terms[start..end]
                .iter()
                .any(|&stamps| self.clock.is_newer(unsafe { stamps.add(row).read() }));
            if !any {
                return false;
            }
            start = end;
        }
        true
    }
}

/// The rows of one tracked archetype that pass a query's row filter, as
/// runs of consecutive rows.
pub(crate) struct Runs<'a> {
    arch: &'a Archetype,
    rows: RowFilter,
    next: usize,
}

impl Runs<'_> {
    /// The next run `start..end` of passing rows, stamped as changed for
    /// every tracked component `Q` may write.
    #[inline(never)]
    pub(crate) fn next<Q: QueryData>(&mut self, ids: &Q::Ids) -> Option<(usize, usize)> {
        let (start, end) = self.next_run()?;
        mark::<Q>(self.arch, ids, self.rows.clock.now, start, end);
        Some((start, end))
    }

    /// The next run `start..end` of passing rows.
    fn next_run(&mut self) -> Option<(usize, usize)> {
        let len = self.arch.len();
        let mut start = self.next;
        while start < len && !self.rows.passes(start) {
            start += 1;
        }
        if start >= len {
            self.next = len;
            return None;
        }
        let mut end = start + 1;
        while end < len && self.rows.passes(end) {
            end += 1;
        }
        // Row `end` (if any) failed: skip it next time.
        self.next = (end + 1).min(len);
        Some((start, end))
    }
}

/// Stamps rows `start..end` of `arch` as changed for every tracked
/// component `Q` may write.
fn mark<Q: QueryData>(arch: &Archetype, ids: &Q::Ids, stamp: u32, start: usize, end: usize) {
    Q::access(ids, &mut |id, mutable, _| {
        if mutable {
            mark_rows(arch, id, stamp, start, end);
        }
    });
}

/// Stamps `row` of `arch` as changed for every tracked component `Q` may
/// write, as a query does for the rows it visits. The caller holds the
/// world mutably and no one else reads or writes these stamps meanwhile.
pub(crate) fn mark_row<Q: QueryData>(arch: &Archetype, ids: &Q::Ids, stamp: u32, row: usize) {
    mark::<Q>(arch, ids, stamp, row, row + 1);
}

/// Stamps rows `start..end` of component `id` in `arch` as changed, if the
/// component is there and tracked. The rows must be live, and nothing else
/// may read or write these stamps meanwhile: the caller holds the world
/// mutably, or is a view that declared writing the component.
fn mark_rows(arch: &Archetype, id: ComponentId, stamp: u32, start: usize, end: usize) {
    debug_assert!(start <= end && end <= arch.len());
    if let Some(changed) = arch.changed_stamps(id) {
        // SAFETY: `start..end` are live rows, with one stamp each.
        unsafe { std::slice::from_raw_parts_mut(changed.add(start), end - start).fill(stamp) };
    }
}

/// Prepares a query's pass over tracked archetype `arch`. Without a row
/// filter here, stamps every row as changed for every tracked component `Q`
/// may write, and returns `None`: the caller visits every row. Otherwise
/// returns the runs of rows to visit.
///
/// Cold and out of line, like the other entry points the query loops call
/// for tracked archetypes: the compiler then keeps the loops of worlds that
/// track nothing as they were, with their state in registers.
#[cold]
#[inline(never)]
pub(crate) fn visit<'a, Q: QueryData>(
    arch: &'a Archetype,
    ids: &Q::Ids,
    filters: &Filters,
    clock: Clock,
) -> Option<Runs<'a>> {
    match filters.row_filter(arch, clock) {
        Some(rows) => Some(Runs { arch, rows, next: 0 }),
        None => {
            mark::<Q>(arch, ids, clock.now, 0, arch.len());
            None
        }
    }
}

/// [`visit`] for the iterator: whether the rows of `arch` come in runs
/// (see [`next_run`]). It and `next_run` are called out of line with
/// everything by value, so the iterator's own state stays in registers and
/// worlds that track nothing iterate exactly as before.
#[cold]
#[inline(never)]
pub(crate) fn enter<Q: QueryData>(arch: &Archetype, ids: Q::Ids, filters: Filters, clock: Clock) -> bool {
    visit::<Q>(arch, &ids, &filters, clock).is_some()
}

/// The first run `start..end` of rows of `arch` at or after row `from` that
/// pass the row filter, stamped as [`Runs::next`] does; `None` once there
/// are none left. Keeps no state between calls: it finds the filter's stamp
/// arrays again each time.
#[cold]
#[inline(never)]
pub(crate) fn next_run<Q: QueryData>(
    arch: &Archetype,
    ids: Q::Ids,
    filters: Filters,
    clock: Clock,
    from: usize,
) -> Option<(usize, usize)> {
    let rows = filters.row_filter(arch, clock)?;
    Runs { arch, rows, next: from }.next::<Q>(&ids)
}

/// [`visit`] for one row: `false` if the row filter rejects it; otherwise
/// stamps it for every tracked component `Q` may write.
pub(crate) fn visit_row<Q: QueryData>(
    arch: &Archetype,
    ids: &Q::Ids,
    filters: &Filters,
    clock: Clock,
    row: usize,
) -> bool {
    if filters.row_filter(arch, clock).is_some_and(|rows| !rows.passes(row)) {
        return false;
    }
    mark::<Q>(arch, ids, clock.now, row, row + 1);
    true
}

/// How many rows of tracked archetype `arch` pass the row filter.
#[cold]
#[inline(never)]
pub(crate) fn count(arch: &Archetype, filters: &Filters, clock: Clock) -> usize {
    match filters.row_filter(arch, clock) {
        Some(rows) => (0..arch.len()).filter(|&row| rows.passes(row)).count(),
        None => arch.len(),
    }
}

/// [`count`], but it stops looking once it has found `max` rows.
pub(crate) fn count_at_most(arch: &Archetype, filters: &Filters, clock: Clock, max: usize) -> usize {
    match filters.row_filter(arch, clock) {
        Some(rows) => (0..arch.len()).filter(|&row| rows.passes(row)).take(max).count(),
        None => arch.len().min(max),
    }
}

/// Stamps every row of the tracked columns `ids` of `arch` as changed. The
/// caller must hold the world mutably.
pub(crate) fn mark_columns(arch: &Archetype, ids: &[ComponentId], stamp: u32) {
    for &id in ids {
        mark_rows(arch, id, stamp, 0, arch.len());
    }
}

/// A chunk query that says which columns it writes and which rows it wants:
/// what a typed query's `&T`/`&mut T`, its filters and its
/// [`cached`](crate::Query::cached) hint say, for components known only by
/// id (the C/C++ path). Run it with
/// [`RawWorld::query_chunks`](crate::RawWorld::query_chunks).
///
/// Start from [`new`](Self::new) and chain what differs. Bit `i` of each
/// mask is about `ids[i]`.
///
/// ```
/// # use ponko_ecs::*;
/// #[derive(Component)]
/// struct Health(i32);
///
/// let mut world = World::new();
/// let health = world.track_changes::<Health>();
/// world.spawn((Health(10),));
/// let seen = world.advance_change_tick();
/// let hurt = world.spawn((Health(3),));
///
/// // Read-only, and only the rows whose value was added after `seen`.
/// let ids = [health];
/// let query = ChunkQuery::new(&ids).writes(0).added(1).since(seen);
/// let mut found = Vec::new();
/// world.raw().query_chunks(query, |chunk| found.extend_from_slice(chunk.entities));
/// assert_eq!(found, [hurt]);
/// ```
#[derive(Clone, Copy, Debug)]
#[must_use = "a chunk query does nothing until it is run with `RawWorld::query_chunks`"]
pub struct ChunkQuery<'a> {
    ids: &'a [ComponentId],
    writes: u32,
    changed: u32,
    added: u32,
    any: bool,
    since: Option<Tick>,
    filter: Option<&'a crate::CompiledFilter>,
    cache: QueryCache,
}

impl<'a> ChunkQuery<'a> {
    /// Every row of every archetype with all of `ids` (every entity visited
    /// has all of them), with every column writable: what
    /// [`RawWorld::for_each_chunk`](crate::RawWorld::for_each_chunk) does.
    pub fn new(ids: &'a [ComponentId]) -> Self {
        ChunkQuery {
            ids,
            writes: u32::MAX,
            changed: 0,
            added: 0,
            any: false,
            since: None,
            filter: None,
            cache: QueryCache::Auto,
        }
    }

    /// The components this query visits, in the order of the chunk's columns.
    #[must_use]
    pub fn ids(&self) -> &'a [ComponentId] {
        self.ids
    }

    /// The columns the callback may write (every one, unless set). Their
    /// rows count as changed, written or not; the other columns must only
    /// be read.
    pub fn writes(mut self, mask: u32) -> Self {
        self.writes = mask;
        self
    }

    /// Only visit rows where these columns changed after
    /// [`since`](Self::since). Their components must be tracked
    /// ([`RawWorld::track_changes_dynamic`](crate::RawWorld::track_changes_dynamic)).
    /// At most 8 `changed` and `added` bits together.
    pub fn changed(mut self, mask: u32) -> Self {
        self.changed = mask;
        self
    }

    /// Only visit rows where these columns were added after
    /// [`since`](Self::since). Their components must be tracked. At most 8
    /// `changed` and `added` bits together.
    pub fn added(mut self, mask: u32) -> Self {
        self.added = mask;
        self
    }

    /// `false` (the default): a row must pass every `changed` and `added`
    /// bit. `true`: one of them is enough.
    pub fn match_any(mut self, any: bool) -> Self {
        self.any = any;
        self
    }

    /// The reader's tick, instead of the world's
    /// [`changes_since`](World::changes_since). `None` keeps the world's,
    /// so a caller that may or may not have its own tick passes an
    /// `Option<Tick>` as it is.
    pub fn since(mut self, tick: impl Into<Option<Tick>>) -> Self {
        self.since = tick.into();
        self
    }

    /// Only visit the archetypes that also pass `filter`, a
    /// [`DynFilter`](crate::DynFilter) compiled once (the runtime form of
    /// [`With`](crate::With), [`Without`](crate::Without) and
    /// [`Or`](crate::Or)). The entities are the ones the typed query with
    /// the same filters visits. `None` filters nothing, so an optional
    /// filter is passed as it is.
    pub fn filter(mut self, filter: impl Into<Option<&'a crate::CompiledFilter>>) -> Self {
        self.filter = filter.into();
        self
    }

    /// Whether this query uses the world's archetype index: what
    /// [`Query::cached`](crate::Query::cached) and
    /// [`Query::uncached`](crate::Query::uncached) say for a typed query.
    /// Unset, the world's [`QueryCachePolicy`](crate::QueryCachePolicy)
    /// decides, as [`QueryCache::Auto`] says. The chunks are the same either
    /// way.
    pub fn cache(mut self, cache: QueryCache) -> Self {
        self.cache = cache;
        self
    }
}

impl World {
    /// Starts recording when values of component `T` are added, changed and
    /// removed, and returns its id. Tracking stays on for the life of the
    /// world; calling this again does nothing.
    ///
    /// Values that already exist count as added and changed now. Tracking
    /// is always explicit: the [`Added`] and [`Changed`] filters and
    /// [`removed`](Self::removed) need the component tracked and panic
    /// otherwise, so call this once, before the first query that filters on
    /// `T` (in a game, in setup).
    ///
    /// Tracking costs 8 bytes per entity that has `T`, and a stamp whenever
    /// one of the operations below touches `T`. Components that aren't
    /// tracked cost nothing.
    ///
    /// What counts, for a tracked component:
    ///
    /// | Operation | Added | Changed |
    /// |---|---|---|
    /// | [`spawn`](Self::spawn), [`spawn_batch`](Self::spawn_batch), [`spawn_at`](Self::spawn_at), [`spawn_dynamic`](crate::RawWorld::spawn_dynamic), [`spawn_at_dynamic`](crate::RawWorld::spawn_at_dynamic) | yes | yes |
    /// | [`insert`](Self::insert), [`insert_dynamic`](crate::RawWorld::insert_dynamic) on an entity without one | yes | yes |
    /// | `insert`, `insert_dynamic` replacing a value | no | yes |
    /// | [`get_mut`](Self::get_mut) | no | yes |
    /// | a query fetching `&mut T` or `Option<&mut T>` | no | yes, for every entity it visits |
    /// | the same in a [`WorldView`](crate::WorldView) query, and its `get_mut` | no | yes |
    /// | [`for_each_chunk`](crate::RawWorld::for_each_chunk) | no | yes, every requested column of every chunk |
    /// | [`mark_changed`](Self::mark_changed), [`mark_changed_dynamic`](crate::RawWorld::mark_changed_dynamic) | no | yes |
    /// | [`get`](Self::get), [`has`](Self::has), `&T` in a query, [`Query::count`](crate::Query::count), [`sort_by`](Self::sort_by) | no | no |
    /// | [`get_dynamic`](crate::RawWorldRef::get_dynamic), [`get_dynamic_mut`](crate::RawWorld::get_dynamic_mut) | no | no: they return raw pointers, so call `mark_changed_dynamic` after writing through the `_mut` one |
    ///
    /// - **A mutable borrow counts as a change**, written or not: the world
    ///   can't see writes through a `&mut T`. A query marks the entities it
    ///   visits: all it matches for `for_each` and the parallel loops, the
    ///   archetypes it reached for [`Query::iter`](crate::Query::iter) (or
    ///   only the rows that pass an `Added`/`Changed` filter), the one
    ///   entity for [`Query::get`](crate::Query::get). To change only some
    ///   entities, read with `&T` and use `get_mut` on those.
    /// - **Removal** ([`remove`](Self::remove), [`remove_dynamic`](crate::RawWorld::remove_dynamic),
    ///   [`despawn`](Self::despawn), [`clear`](Self::clear), [`reset`](Self::reset))
    ///   is logged for [`removed`](Self::removed). The entity's other
    ///   components keep their stamps, as they do when a component is
    ///   inserted next to them. Inserting `T` again later counts as added.
    /// - **[`Commands`](crate::Commands)** are stamped when they are applied,
    ///   with the tick current then.
    /// - **Saving** doesn't store stamps or removal logs. A restored world
    ///   spawns its entities again, so everything in it counts as added.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Health(i32);
    /// let mut world = World::new();
    /// world.track_changes::<Health>();
    /// let a = world.spawn((Health(10),));
    /// let b = world.spawn((Health(10),));
    ///
    /// // A reader keeps the tick at which it last looked.
    /// let seen = world.advance_change_tick();
    /// world.get_mut::<Health>(b).unwrap().0 -= 1;
    ///
    /// let mut hurt = world.query::<Entity>().filter::<Changed<Health>>().since(seen);
    /// assert_eq!(hurt.iter().collect::<Vec<_>>(), [b]);
    /// # let _ = a;
    /// ```
    pub fn track_changes<T: Component>(&mut self) -> ComponentId {
        let id = self.register::<T>();
        self.track_changes_dynamic_inner(id);
        id
    }

    /// Implementation of [`RawWorld::track_changes_dynamic`](crate::RawWorld::track_changes_dynamic).
    pub(crate) fn track_changes_dynamic_inner(&mut self, id: ComponentId) {
        let info = &mut self.infos[id.index()];
        if info.tracked {
            return;
        }
        info.tracked = true;
        let stamp = self.changes.stamp();
        for arch in &mut self.archetypes {
            if let Some(col) = arch.column_index(id) {
                arch.start_tracking(col, stamp);
            }
        }
        self.changes.removed.push(Removed {
            id,
            entities: Vec::new(),
            ticks: Vec::new(),
        });
    }

    /// Whether any component is tracked: each has a removal log.
    #[inline]
    pub(crate) fn tracks_changes(&self) -> bool {
        !self.changes.removed.is_empty()
    }

    /// The current tick: changes made now are stamped with it, and a reader
    /// that looked at this tick has seen them.
    #[inline]
    #[must_use]
    pub fn change_tick(&self) -> Tick {
        Tick(self.changes.now)
    }

    /// Ends the current tick and returns it; later changes get a newer one.
    ///
    /// This is how a reader remembers where it stopped: look, then keep the
    /// returned tick for next time. Changes made between the look and this
    /// call (the reader's own) are part of the returned tick, so the reader
    /// won't be shown them again; advance before looking to see them too.
    ///
    /// The game loop does this after every system, so that each system sees
    /// what changed since it last ran. A world used on its own:
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// let mut world = World::new();
    /// world.track_changes::<Pos>();
    /// let e = world.spawn((Pos(0.0),));
    /// let mut seen = Tick::default();
    /// let mut moved = Vec::new();
    /// for step in 0..3 {
    ///     if step == 1 {
    ///         world.get_mut::<Pos>(e).unwrap().0 = 1.0;
    ///     }
    ///     let n = world.query::<&Pos>().filter::<Changed<Pos>>().since(seen).count();
    ///     moved.push(n);
    ///     seen = world.advance_change_tick();
    /// }
    /// // Spawned before the first look, changed before the second.
    /// assert_eq!(moved, [1, 1, 0]);
    /// ```
    pub fn advance_change_tick(&mut self) -> Tick {
        let ended = self.changes.now;
        self.changes.now += 1;
        if self.changes.now - self.changes.aged_at >= AGE_EVERY {
            self.age_stamps();
        }
        Tick(ended)
    }

    /// Keeps stamps comparable as the tick counter passes 32 bits (stamps
    /// hold its low 32 bits): everything older than `MAX_AGE` ticks becomes
    /// exactly that old.
    #[cold]
    fn age_stamps(&mut self) {
        let now = self.changes.stamp();
        for arch in self.archetypes.iter().filter(|a| a.tracked()) {
            arch.age_stamps(now, MAX_AGE);
        }
        self.changes.aged_at = self.changes.now;
    }

    /// The tick [`Added`] and [`Changed`] filters and
    /// [`removed`](Self::removed) compare against when the caller doesn't
    /// name one: they report what happened after it. `Tick::default()`
    /// (everything) until [`set_changes_since`](Self::set_changes_since).
    #[inline]
    #[must_use]
    pub fn changes_since(&self) -> Tick {
        Tick(self.changes.since)
    }

    /// Sets what [`changes_since`](Self::changes_since) returns. The game
    /// loop sets it before each system to the tick that system last ran at.
    pub fn set_changes_since(&mut self, tick: Tick) {
        self.changes.since = tick.0;
    }

    /// Marks component `T` of `e` as changed now, for code that changes a
    /// value in a way the world can't see. Returns `false` if `e` is dead or
    /// has no `T`. Does nothing (and returns `true`) if `T` isn't tracked.
    pub fn mark_changed<T: Component>(&mut self, e: Entity) -> bool {
        self.component_id::<T>()
            .is_some_and(|id| self.mark_changed_dynamic_inner(e, id))
    }

    /// Implementation of [`RawWorld::mark_changed_dynamic`](crate::RawWorld::mark_changed_dynamic).
    pub(crate) fn mark_changed_dynamic_inner(&mut self, e: Entity, id: ComponentId) -> bool {
        let Some(loc) = self.entities.get(e) else { return false };
        let arch = &self.archetypes[loc.archetype as usize];
        let Some(col) = arch.column_index(id) else { return false };
        if arch.tracked() {
            arch.stamp(col, loc.row as usize, self.changes.stamp(), false);
        }
        true
    }

    /// The entities that lost a tracked component `T` (it was removed, or
    /// the entity was despawned) after [`changes_since`](Self::changes_since),
    /// oldest first.
    ///
    /// An entity is listed once per removal, so it can appear twice, and it
    /// may have a `T` again by now: check [`has`](Self::has) if that matters.
    /// Replacing a value isn't a removal.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Sprite(u32);
    /// let mut world = World::new();
    /// world.track_changes::<Sprite>();
    /// let a = world.spawn((Sprite(1),));
    /// let b = world.spawn((Sprite(2),));
    /// world.remove::<Sprite>(a);
    /// world.despawn(b);
    /// assert_eq!(world.removed::<Sprite>(), [a, b]);
    /// ```
    ///
    /// # Panics
    /// If `T` isn't tracked: removals of an untracked component aren't
    /// logged, so the answer would always be "none".
    #[track_caller]
    #[must_use]
    pub fn removed<T: Component>(&self) -> &[Entity] {
        self.removed_since::<T>(self.changes_since())
    }

    /// [`removed`](Self::removed) after a tick the caller kept.
    ///
    /// # Panics
    /// If `T` isn't tracked.
    #[track_caller]
    #[must_use]
    pub fn removed_since<T: Component>(&self, since: Tick) -> &[Entity] {
        match self.component_id::<T>() {
            Some(id) if self.infos[id.index()].tracked => self.removed_dynamic_inner(id, since),
            _ => untracked(std::any::type_name::<T>()),
        }
    }

    /// Implementation of [`RawWorldRef::removed_dynamic`](crate::RawWorldRef::removed_dynamic).
    #[track_caller]
    #[must_use]
    pub(crate) fn removed_dynamic_inner(&self, id: ComponentId, since: Tick) -> &[Entity] {
        let Some(log) = self.changes.removed.iter().find(|log| log.id == id) else {
            untracked(self.infos[id.index()].name())
        };
        if since.0 > self.changes.now {
            // Another world's tick: everything, as for the filters.
            return &log.entities;
        }
        let first = log.ticks.partition_point(|&tick| tick <= since.0);
        &log.entities[first..]
    }

    /// Implementation of [`RawWorld::query_chunks`](crate::RawWorld::query_chunks).
    #[track_caller]
    pub(crate) fn query_chunks_inner(&mut self, query: ChunkQuery<'_>, mut f: impl FnMut(Chunk<'_>)) {
        let filter = query.filter.map(crate::CompiledFilter::filters);
        let cache = query.cache;
        const MAX: usize = 32;
        let ids = query.ids;
        assert!(ids.len() <= MAX, "query_chunks: at most {MAX} components");
        let used = if ids.len() == MAX {
            u32::MAX
        } else {
            (1 << ids.len()) - 1
        };
        assert!(
            (query.changed & used).count_ones() + (query.added & used).count_ones() <= MAX_CLAUSES as u32,
            "query_chunks: at most {MAX_CLAUSES} `changed` and `added` filters"
        );
        let bit = |mask: u32, i: usize| mask >> i & 1 != 0;
        self.recorder.note(|l| {
            for (i, &id) in ids.iter().enumerate() {
                l.component(id, bit(query.writes, i));
            }
        });
        let mut filtered = false;
        for (i, &id) in ids.iter().enumerate() {
            if bit(query.changed | query.added, i) {
                // Tracking is explicit here too, as for the typed filters.
                let info = &self.infos[id.index()];
                assert!(
                    info.tracked,
                    "query_chunks: a `changed` or `added` filter needs `{}`'s changes tracked; \
                     call `world.raw().track_changes_dynamic(id)` first",
                    info.name()
                );
                filtered = true;
            }
        }
        let clock = Clock::new(self.change_tick(), query.since.unwrap_or(self.changes_since()));
        let stamp = self.changes.stamp();
        let mut cols = [0; MAX];
        let mut ptrs = [std::ptr::null_mut(); MAX];
        let archetypes = self
            .query_matches
            .chunk_archetypes(&self.archetypes, ids, filter, cache, self.slots.scan());
        'arch: for arch in archetypes {
            if arch.is_empty() || filter.is_some_and(|filter| !filter.matches(arch)) {
                continue;
            }
            for (i, &id) in ids.iter().enumerate() {
                match arch.column_index(id) {
                    Some(col) => cols[i] = col,
                    None => continue 'arch,
                }
            }
            // `None`: every row. A filtered column is tracked and present,
            // so its archetype is tracked.
            let mut runs = (filtered && arch.tracked()).then(|| {
                let mut rows = RowFilter::new(clock);
                for (i, &id) in ids.iter().enumerate() {
                    for (mask, added) in [(query.added, true), (query.changed, false)] {
                        if bit(mask, i) {
                            rows.term(arch.read_stamps(id, added).expect("a filtered column is tracked"));
                            if !query.any {
                                rows.close();
                            }
                        }
                    }
                }
                if query.any {
                    rows.close();
                }
                Runs { arch, rows, next: 0 }
            });
            let (mut start, mut end) = (0, arch.len());
            loop {
                if let Some(runs) = &mut runs {
                    match runs.next_run() {
                        Some(run) => (start, end) = run,
                        None => break,
                    }
                }
                for (i, &id) in ids.iter().enumerate() {
                    if arch.tracked() && bit(query.writes, i) {
                        mark_rows(arch, id, stamp, start, end);
                    }
                    ptrs[i] = arch.columns[cols[i]].ptr(start);
                }
                f(Chunk {
                    entities: &arch.entities[start..end],
                    columns: &ptrs[..ids.len()],
                });
                if runs.is_none() {
                    break;
                }
            }
        }
    }

    /// Drops removals logged at or before `tick`: call it with the oldest
    /// tick any reader still needs, or the logs of tracked components grow
    /// with every removal. The game loop does this once per frame.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// #[derive(Component)]
    /// struct Sprite(u32);
    /// let mut world = World::new();
    /// world.track_changes::<Sprite>();
    /// let e = world.spawn((Sprite(1),));
    /// world.despawn(e);
    /// let seen = world.advance_change_tick();
    /// world.forget_removed(seen);
    /// assert!(world.removed_since::<Sprite>(Tick::default()).is_empty());
    /// ```
    pub fn forget_removed(&mut self, tick: Tick) {
        for log in &mut self.changes.removed {
            let n = log.ticks.partition_point(|&t| t <= tick.0);
            if n > 0 {
                log.ticks.drain(..n);
                log.entities.drain(..n);
            }
        }
    }
}

#[cold]
#[track_caller]
fn untracked(name: &str) -> ! {
    panic!("removals of `{name}` aren't tracked; call `world.track_changes::<{name}>()` first")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;

    #[derive(Component)]
    struct Pos(u32);

    impl World {
        /// Moves the clock forward `ticks` ticks, aging stamps as often as
        /// that many `advance_change_tick` calls would.
        fn skip_change_ticks(&mut self, mut ticks: u64) {
            while ticks > 0 {
                let step = ticks.min(AGE_EVERY);
                self.changes.now += step - 1;
                self.advance_change_tick();
                ticks -= step;
            }
        }
    }

    fn changed(world: &mut World, since: Tick) -> Vec<u32> {
        let mut out = Vec::new();
        world
            .query::<&Pos>()
            .filter::<Changed<Pos>>()
            .since(since)
            .for_each(|p| out.push(p.0));
        out
    }

    #[test]
    fn stamps_compare_correctly_when_their_32_bits_wrap() {
        let mut world = World::new();
        world.track_changes::<Pos>();
        // Just below the point where the low 32 bits of the tick wrap to 0.
        world.skip_change_ticks(u64::from(u32::MAX) - 3);
        let old = world.spawn((Pos(1),));
        let seen = world.advance_change_tick();
        for _ in 0..8 {
            world.advance_change_tick();
        }
        assert!(world.change_tick().0 > u64::from(u32::MAX), "the stamp bits wrapped");
        assert_eq!(world.changes.stamp(), 6);
        // `old` was stamped before the wrap with a huge number, `new` after
        // it with a small one. Still: old is older than `seen`, new is newer.
        let new = world.spawn((Pos(2),));
        assert_eq!(changed(&mut world, seen), [2]);
        assert_eq!(changed(&mut world, Tick::default()), [1, 2]);
        // A change after the wrap to a value stamped before it is seen.
        let later = world.advance_change_tick();
        world.get_mut::<Pos>(old).unwrap().0 = 3;
        assert_eq!(changed(&mut world, later), [3]);
        let _ = new;
    }

    #[test]
    fn old_stamps_stay_old_after_more_than_2_pow_32_ticks() {
        let mut world = World::new();
        world.track_changes::<Pos>();
        world.spawn((Pos(1),));
        let seen = world.advance_change_tick();
        // Without aging, the stamp would look new again after 2^32 ticks.
        world.skip_change_ticks(1 << 32);
        let recent = world.advance_change_tick();
        assert_eq!(changed(&mut world, recent), []);
        world.skip_change_ticks(3 << 32);
        let recent = world.advance_change_tick();
        assert_eq!(changed(&mut world, recent), []);
        // Nothing is missed either: a new change is seen by a recent reader,
        // and a reader from before the first change still sees the value.
        world.spawn((Pos(2),));
        assert_eq!(changed(&mut world, recent), [2]);
        assert_eq!(changed(&mut world, Tick::default()), [1, 2]);
        // A reader that slept through more than `MAX_AGE` ticks is shown
        // the old value again: too much, never too little.
        assert_eq!(changed(&mut world, seen), [1, 2]);
    }

    #[test]
    fn readers_within_max_age_see_exactly_what_changed_across_aging_passes() {
        let mut world = World::new();
        world.track_changes::<Pos>();
        world.spawn((Pos(1),));
        // Old enough that aging rewrites its stamp.
        world.skip_change_ticks(u64::from(MAX_AGE) + 100);
        let before = world.advance_change_tick();
        world.spawn((Pos(2),));
        let after = world.advance_change_tick();
        for _ in 0..3 {
            // Each step crosses an aging pass; the readers stay younger than `MAX_AGE`.
            world.skip_change_ticks(AGE_EVERY / 2 + 10);
            assert_eq!(changed(&mut world, before), [2]);
            assert_eq!(changed(&mut world, after), []);
        }
        // Older than `MAX_AGE`, a reader may be shown more, never less.
        world.skip_change_ticks(u64::from(MAX_AGE));
        assert!(changed(&mut world, before).contains(&2));
        let now = world.change_tick();
        assert_eq!(changed(&mut world, now), []);
    }

    #[test]
    fn a_tick_the_world_has_not_reached_reports_everything() {
        // Such a tick can only be another world's (a reader that outlived
        // its world): showing everything can't miss a change.
        let mut world = World::new();
        world.track_changes::<Pos>();
        let e = world.spawn((Pos(1),));
        for future in [world.change_tick().0 + 1, 1 << 40] {
            assert_eq!(changed(&mut world, Tick::from_bits(future)), [1]);
        }
        world.despawn(e);
        assert_eq!(world.removed_since::<Pos>(Tick::from_bits(1 << 40)), [e]);
        let now = world.change_tick();
        assert_eq!(world.removed_since::<Pos>(now), []);
    }

    #[test]
    fn removal_logs_use_whole_ticks_and_never_wrap() {
        let mut world = World::new();
        world.track_changes::<Pos>();
        let a = world.spawn((Pos(1),));
        world.despawn(a);
        let seen = world.advance_change_tick();
        world.skip_change_ticks(5 << 32);
        let b = world.spawn((Pos(2),));
        world.despawn(b);
        assert_eq!(world.removed_since::<Pos>(seen), [b]);
        assert_eq!(world.removed_since::<Pos>(Tick::default()), [a, b]);
    }
}
