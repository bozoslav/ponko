//! Type-erased storage: component descriptions, columns and archetype tables.

use std::alloc::{self, Layout};
use std::any::TypeId;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::ptr::NonNull;

use crate::entity::Entity;

/// Identifies a component type inside one [`World`](crate::World).
///
/// Rust types get one automatically the first time they are used; other
/// languages get one from [`RawWorld::register_component`](crate::RawWorld::register_component).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Default)]
pub struct ComponentId(pub(crate) u32);

impl ComponentId {
    /// The id as a plain number (stable for the lifetime of the world).
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Destructor for one component value. `C-unwind` so C/C++ can supply one too.
pub type DropFn = unsafe extern "C-unwind" fn(*mut u8);

pub(crate) unsafe extern "C-unwind" fn drop_ptr<T>(p: *mut u8) {
    unsafe { p.cast::<T>().drop_in_place() }
}

/// What the world knows about a component type.
#[derive(Clone, Debug)]
pub struct ComponentInfo {
    pub(crate) name: String,
    pub(crate) layout: Layout,
    pub(crate) drop: Option<DropFn>,
    pub(crate) type_id: Option<TypeId>,
    /// Change stamps are kept for this component (see `World::track_changes`).
    pub(crate) tracked: bool,
}

impl ComponentInfo {
    /// Type name for Rust components, the registered name otherwise.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Size and alignment of one value.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }
    /// The Rust type, if this component was registered from Rust.
    #[must_use]
    pub fn type_id(&self) -> Option<TypeId> {
        self.type_id
    }
    /// Whether values have a destructor. Values without one are plain bytes
    /// as far as the world is concerned (C/C++ may only touch those).
    #[must_use]
    pub fn needs_drop(&self) -> bool {
        self.drop.is_some()
    }
    /// Whether the world records when values of this component are added,
    /// changed and removed: [`World::track_changes`](crate::World::track_changes).
    #[must_use]
    pub fn tracks_changes(&self) -> bool {
        self.tracked
    }
}

/// One column of values of a single component type. Its capacity is owned by
/// the archetype (all columns of an archetype share it).
///
/// A column whose component is tracked ([`World::track_changes`]) keeps two
/// `u32` change stamps per row in the same buffer, after the values: when
/// each value was added, and when it last changed. Tracking adds nothing to
/// this struct, and an untracked column (every column, unless a game asks)
/// is laid out and handled exactly as if change detection didn't exist:
/// loops over many archetypes are sensitive to the size of this struct.
///
/// [`World::track_changes`]: crate::World::track_changes
pub(crate) struct Column {
    data: NonNull<u8>,
    /// Size of one value.
    size: usize,
    drop: Option<DropFn>,
    /// Alignment of one value.
    align: u32,
    /// Whether the buffer also holds the change stamps (see [`Column::stamps`]).
    tracked: bool,
}

/// Bytes of change stamps per row of a tracked column: `added` and `changed`.
const STAMP_BYTES: usize = 2 * std::mem::size_of::<u32>();

impl Column {
    fn new(item: Layout, drop: Option<DropFn>) -> Self {
        // Dangling but aligned: valid for zero-sized items and empty columns.
        let data = NonNull::new(std::ptr::without_provenance_mut(item.align())).unwrap();
        Column {
            data,
            size: item.size(),
            drop,
            align: u32::try_from(item.align()).expect("component alignment too large"),
            tracked: false,
        }
    }

    /// Bytes of change stamps per row: two `u32`s if tracked.
    fn stamp_bytes(&self) -> usize {
        if self.tracked { STAMP_BYTES } else { 0 }
    }

    /// Where the stamps start in a tracked column's buffer for `cap` rows:
    /// after the values, aligned for `u32`. [`Column::array`] checked that
    /// this doesn't overflow when the buffer was made.
    #[inline]
    fn stamps_offset(&self, cap: usize) -> usize {
        (self.size * cap).next_multiple_of(std::mem::align_of::<u32>())
    }

    /// The `added` and `changed` stamp arrays, `cap` stamps each, if this
    /// component is tracked. `cap` must be the column's capacity.
    #[inline]
    pub(crate) fn stamps(&self, cap: usize) -> Option<(*mut u32, *mut u32)> {
        if !self.tracked {
            return None;
        }
        // SAFETY: a tracked column's buffer for `cap` rows holds the values,
        // then both arrays. With no capacity the offset is 0 and the
        // (dangling) pointers are never read.
        unsafe {
            let added = self.data.as_ptr().add(self.stamps_offset(cap)).cast::<u32>();
            Some((added, added.add(cap)))
        }
    }

    /// Makes room for stamps in the buffer of `cap` rows, and stamps the
    /// first `len` rows.
    ///
    /// # Safety
    /// The column is untracked, and `len <= cap`, the archetype's capacity.
    unsafe fn start_tracking(&mut self, cap: usize, len: usize, stamp: u32) {
        let old = self.array(cap);
        self.tracked = true;
        let new = self.array(cap);
        if new.size() == 0 {
            return;
        }
        let p = unsafe {
            if old.size() == 0 {
                alloc::alloc(new)
            } else if old.align() == new.align() {
                alloc::realloc(self.data.as_ptr(), old, new.size())
            } else {
                // Stamps need a stricter alignment than the values: a new buffer.
                let p = alloc::alloc(new);
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(self.data.as_ptr(), p, old.size());
                    alloc::dealloc(self.data.as_ptr(), old);
                }
                p
            }
        };
        self.data = NonNull::new(p).unwrap_or_else(|| alloc::handle_alloc_error(new));
        let (added, changed) = self.stamps(cap).expect("tracked");
        for row in 0..len {
            // SAFETY: `row < cap`, the length of both arrays.
            unsafe {
                added.add(row).write(stamp);
                changed.add(row).write(stamp);
            }
        }
    }

    #[inline(always)]
    pub(crate) fn base(&self) -> *mut u8 {
        self.data.as_ptr()
    }

    #[inline(always)]
    pub(crate) fn ptr(&self, row: usize) -> *mut u8 {
        unsafe { self.data.as_ptr().add(row * self.size) }
    }

    #[inline]
    pub(crate) fn size(&self) -> usize {
        self.size
    }

    pub(crate) fn drop_fn(&self) -> Option<DropFn> {
        self.drop
    }

    /// The buffer for `cap` rows: the values, then the stamps of a tracked
    /// column.
    fn array(&self, cap: usize) -> Layout {
        let values = self.size.checked_mul(cap).expect("column too large");
        let (size, align) = if self.tracked {
            let size = values
                .checked_next_multiple_of(std::mem::align_of::<u32>())
                .and_then(|values| values.checked_add(cap.checked_mul(STAMP_BYTES)?))
                .expect("column too large");
            (size, (self.align as usize).max(std::mem::align_of::<u32>()))
        } else {
            (values, self.align as usize)
        };
        Layout::from_size_align(size, align).expect("column too large")
    }

    /// Grows the buffer from `old_cap` to `new_cap` rows. Takes a buffer of
    /// the new size from `pool` if it has one (one of an archetype that
    /// was reclaimed, or that grew), and gives the old one to it.
    unsafe fn grow(&mut self, old_cap: usize, new_cap: usize, pool: &mut BufferPool) {
        let (old, new) = (self.array(old_cap), self.array(new_cap));
        if new.size() == 0 {
            return;
        }
        if let Some(fresh) = pool.take(new) {
            if old.size() != 0 {
                // SAFETY: both buffers hold their capacity of values (and,
                // tracked, stamps after them); `fresh` is a separate buffer.
                unsafe {
                    std::ptr::copy_nonoverlapping(self.data.as_ptr(), fresh.as_ptr(), self.size * old_cap);
                    if let Some((added, changed)) = self.stamps(old_cap) {
                        let to = fresh.as_ptr().add(self.stamps_offset(new_cap)).cast::<u32>();
                        std::ptr::copy_nonoverlapping(added, to, old_cap);
                        std::ptr::copy_nonoverlapping(changed, to.add(new_cap), old_cap);
                    }
                    if !pool.put(old, self.data) {
                        alloc::dealloc(self.data.as_ptr(), old);
                    }
                }
            }
            self.data = fresh;
            return;
        }
        let p = unsafe {
            if old.size() == 0 {
                alloc::alloc(new)
            } else {
                alloc::realloc(self.data.as_ptr(), old, new.size())
            }
        };
        self.data = NonNull::new(p).unwrap_or_else(|| alloc::handle_alloc_error(new));
        if self.tracked && old_cap != 0 {
            // The stamps still sit after `old_cap` values: move them behind
            // `new_cap` values. `changed` goes first; both arrays move up,
            // and it lands past where `added` still is.
            unsafe {
                let from = self.data.as_ptr().add(self.stamps_offset(old_cap)).cast::<u32>();
                let (added, changed) = self.stamps(new_cap).expect("tracked");
                std::ptr::copy(from.add(old_cap), changed, old_cap);
                std::ptr::copy(from, added, old_cap);
            }
        }
    }

    /// Moves the value at `last` into `row` without dropping `row`.
    #[inline]
    pub(crate) unsafe fn swap_remove_forget(&mut self, row: usize, last: usize) {
        if row != last {
            unsafe { std::ptr::copy_nonoverlapping(self.ptr(last), self.ptr(row), self.size) }
        }
    }

    /// Reorders the first `perm.len()` values (and their stamps): new row
    /// `i` gets old row `perm[i]`. `perm` must be a permutation of
    /// `0..perm.len()`, and `cap` the column's capacity.
    unsafe fn permute(&mut self, perm: &[u32], cap: usize) {
        let size = self.size;
        let layout = self.array(cap);
        if layout.size() == 0 || perm.is_empty() {
            return;
        }
        let new = unsafe { alloc::alloc(layout) };
        let new = NonNull::new(new).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        if size != 0 {
            for (i, &old) in perm.iter().enumerate() {
                unsafe { std::ptr::copy_nonoverlapping(self.ptr(old as usize), new.as_ptr().add(i * size), size) };
            }
        }
        if let Some((added, changed)) = self.stamps(cap) {
            // SAFETY: the first `perm.len()` stamps of both arrays are live,
            // every `perm` entry is below that, and the new buffer has the
            // same layout.
            unsafe {
                let to = new.as_ptr().add(self.stamps_offset(cap)).cast::<u32>();
                for (i, &old) in perm.iter().enumerate() {
                    to.add(i).write(added.add(old as usize).read());
                    to.add(cap + i).write(changed.add(old as usize).read());
                }
            }
        }
        unsafe { alloc::dealloc(self.data.as_ptr(), layout) };
        self.data = new;
    }

    /// Hands the buffer for `cap` rows to `pool` (or frees it if the pool
    /// is full) and leaves the column with none, as a new one. The values
    /// in it must be dead (dropped or moved out).
    ///
    /// # Safety
    /// `cap` is the column's capacity.
    unsafe fn give_buffer(&mut self, cap: usize, pool: &mut BufferPool) {
        let layout = self.array(cap);
        if layout.size() != 0 && !pool.put(layout, self.data) {
            // SAFETY: the buffer was allocated with this layout.
            unsafe { alloc::dealloc(self.data.as_ptr(), layout) }
        }
        self.data = NonNull::new(std::ptr::without_provenance_mut(self.align as usize)).unwrap();
    }

    unsafe fn dealloc(&mut self, cap: usize) {
        let layout = self.array(cap);
        if layout.size() != 0 {
            unsafe { alloc::dealloc(self.data.as_ptr(), layout) }
        }
    }
}

/// The bit of [`Archetype::cap`] that says the archetype keeps change stamps.
/// Capacities never get near it: rows are indexed by `u32`.
const TRACKED: usize = 1 << (usize::BITS - 1);

/// All entities that have exactly the same set of components, stored as one
/// column per component (struct of arrays).
pub struct Archetype {
    pub(crate) ids: Box<[ComponentId]>,
    pub(crate) columns: Box<[Column]>,
    pub(crate) entities: Vec<Entity>,
    /// How many rows every column has room for, and in the top bit
    /// ([`TRACKED`]) whether any column here keeps change stamps: `false`
    /// skips all stamp work. One word for both, so that change tracking
    /// doesn't grow the archetype: at 128 bytes each, a loop over many
    /// archetypes reads one cache line of every archetype.
    cap: usize,
    pub(crate) add_edges: EdgeMap,
    pub(crate) remove_edges: EdgeMap,
}

// SAFETY: columns are plain owned buffers. Rust components are required to be
// `Send + Sync`; components registered at runtime promise the same.
unsafe impl Send for Archetype {}
unsafe impl Sync for Archetype {}

impl Archetype {
    pub(crate) fn new(ids: Box<[ComponentId]>, infos: &[ComponentInfo]) -> Self {
        let columns = ids.iter().map(|id| Self::column(&infos[id.index()])).collect();
        let mut arch = Archetype {
            ids,
            columns,
            entities: Vec::new(),
            cap: 0,
            add_edges: EdgeMap::default(),
            remove_edges: EdgeMap::default(),
        };
        arch.cap = if arch.columns.iter().any(|c| c.tracked) {
            TRACKED
        } else {
            0
        };
        arch
    }

    /// An empty column for a component described by `info`.
    fn column(info: &ComponentInfo) -> Column {
        let mut column = Column::new(info.layout, info.drop);
        if info.tracked {
            // SAFETY: a new, untracked column of an archetype with no capacity.
            unsafe { column.start_tracking(0, 0, 0) };
        }
        column
    }

    /// Takes everything this archetype owns out of it, leaving an archetype
    /// with no components, rows or edges: what a reclaimed archetype slot
    /// holds until it is used again. Column buffers go to `pool` (or are
    /// freed); the rest is returned for reuse by [`Archetype::reuse`].
    /// The caller has taken the edges out and unlinked them.
    pub(crate) fn retire(&mut self, pool: &mut BufferPool) -> Shell {
        assert!(self.is_empty(), "only an empty archetype is reclaimed");
        let cap = self.cap();
        for c in self.columns.iter_mut() {
            // SAFETY: `cap` is the capacity, and no row is live.
            unsafe { c.give_buffer(cap, pool) };
        }
        self.cap = 0;
        give_entities(std::mem::take(&mut self.entities), pool);
        Shell {
            ids: std::mem::take(&mut self.ids),
            key: None,
            columns: std::mem::take(&mut self.columns),
            add_edges: std::mem::take(&mut self.add_edges),
            remove_edges: std::mem::take(&mut self.remove_edges),
        }
    }

    /// A new, empty archetype of `ids` (sorted, unique) built in the storage
    /// of a reclaimed one, and its key for the world's archetype map. Boxes
    /// of the same length are written over, and the edge maps keep their
    /// capacity. Doesn't allocate when `shell` held as many components as
    /// `ids`.
    pub(crate) fn reuse(shell: Shell, ids: &[ComponentId], infos: &[ComponentInfo]) -> (Self, Box<[ComponentId]>) {
        let Shell {
            ids: mut own,
            key,
            mut columns,
            mut add_edges,
            mut remove_edges,
        } = shell;
        let boxed = |mut b: Box<[ComponentId]>| {
            if b.len() == ids.len() {
                b.copy_from_slice(ids);
                b
            } else {
                ids.into()
            }
        };
        own = boxed(own);
        let key = boxed(key.unwrap_or_default());
        if columns.len() == ids.len() {
            for (c, id) in columns.iter_mut().zip(ids) {
                *c = Self::column(&infos[id.index()]);
            }
        } else {
            columns = ids.iter().map(|id| Self::column(&infos[id.index()])).collect();
        }
        add_edges.clear();
        remove_edges.clear();
        let mut arch = Archetype {
            ids: own,
            columns,
            entities: Vec::new(),
            cap: 0,
            add_edges,
            remove_edges,
        };
        arch.cap = if arch.columns.iter().any(|c| c.tracked) {
            TRACKED
        } else {
            0
        };
        (arch, key)
    }

    /// Number of entities in this archetype.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// `true` if no entity has this exact component set right now.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Bytes per entity in this archetype: the size of every component plus
    /// the entity handle, and 8 bytes of change stamps for each component
    /// whose changes are tracked
    /// ([`World::track_changes`](crate::World::track_changes)). What one more
    /// entity costs, with no slack.
    ///
    /// ```
    /// use ponko_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Pos(f32, f32);
    /// let mut world = World::new();
    /// world.spawn((Pos(0.0, 0.0),));
    /// let a = world.raw_ref().archetypes().iter().find(|a| a.len() == 1).unwrap();
    /// assert_eq!(a.bytes_per_entity(), 8 + 8); // Pos + the entity handle
    ///
    /// world.track_changes::<Pos>();
    /// let a = world.raw_ref().archetypes().iter().find(|a| a.len() == 1).unwrap();
    /// assert_eq!(a.bytes_per_entity(), 8 + 8 + 8); // and two 4-byte stamps
    /// ```
    pub fn bytes_per_entity(&self) -> usize {
        self.columns.iter().map(|c| c.size() + c.stamp_bytes()).sum::<usize>() + std::mem::size_of::<Entity>()
    }

    /// Bytes this archetype has allocated for entity data (including spare
    /// capacity, which is why it can exceed `len * bytes_per_entity`).
    ///
    /// ```
    /// use ponko_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Pos(f32, f32);
    /// let mut world = World::new();
    /// world.spawn_batch((0..100).map(|i| (Pos(i as f32, 0.0),)));
    /// let a = world.raw_ref().archetypes().iter().find(|a| a.len() == 100).unwrap();
    /// assert!(a.allocated_bytes() >= 100 * a.bytes_per_entity());
    /// ```
    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        self.columns
            .iter()
            .map(|c| (c.size() + c.stamp_bytes()) * self.cap())
            .sum::<usize>()
            + self.entities.capacity() * std::mem::size_of::<Entity>()
    }

    /// The components every entity here has, sorted by id.
    #[must_use]
    pub fn component_ids(&self) -> &[ComponentId] {
        &self.ids
    }

    /// The entities stored here; row `i` of every column belongs to `entities()[i]`.
    #[must_use]
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// `true` if entities here have component `id`.
    #[inline]
    #[must_use]
    pub fn has(&self, id: ComponentId) -> bool {
        self.ids.contains(&id)
    }

    #[inline]
    pub(crate) fn column_index(&self, id: ComponentId) -> Option<usize> {
        // A linear scan, kept on purpose (issue #144; numbers and the alternatives tried are in
        // `crates/ponko_bench/MEMBERSHIP.md`). Its cost grows with where `id` sits in the sorted
        // list: in a query over 4096 archetypes it is about as cheap as the loop itself when the
        // queried components are the first ids, and 40-80 ns per archetype at 32-64 components
        // when they are the last. Binary search is slower than the scan for early ids even on its
        // own. An 8-wide block compare, and hybrids that keep this scan for short lists, fixed the
        // late case in the engine but each made 4-16 component archetypes 2-10 ns slower per
        // archetype in the same query: code larger than this scan changes how the query loop
        // compiles (at first, `column_ptr` was no longer inlined). A bitset or id-indexed table
        // costs 50-640 B per archetype. Structural changes do not scan per column: `move_row`
        // merges the two sorted id lists.
        self.ids.iter().position(|&c| c == id)
    }

    /// Start of the column for component `id` (`len()` values, tightly packed),
    /// or `None` if entities here don't have it: for tools that read whole
    /// columns, such as the inspector.
    ///
    /// The pointer is for reading only: an archetype reached through a shared
    /// world doesn't give the right to write its values. To write whole
    /// columns, use [`RawWorld::for_each_chunk`](crate::RawWorld::for_each_chunk)
    /// or [`RawWorld::query_chunks`](crate::RawWorld::query_chunks), which
    /// hold the world exclusively and hand out `*mut` pointers.
    ///
    /// ```
    /// use ponko_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.spawn((Score(7),));
    /// let id = world.component_id::<Score>().unwrap();
    /// let table = world.raw_ref().archetypes().iter().find(|a| a.has(id)).unwrap();
    /// let column = table.column_ptr(id).unwrap().cast::<Score>();
    /// // SAFETY: the column holds `table.len()` (one) `Score`, and nothing
    /// // changes the world while it is read.
    /// assert_eq!(unsafe { (*column).0 }, 7);
    /// ```
    #[inline]
    #[must_use]
    pub fn column_ptr(&self, id: ComponentId) -> Option<*const u8> {
        self.column_base(id).map(<*mut u8>::cast_const)
    }

    /// [`column_ptr`](Self::column_ptr) for this crate's own writers: queries
    /// that fetch `&mut T` and the chunk loops. A column is a raw allocation,
    /// so its pointer allows writing whatever borrow of the archetype it came
    /// through; whoever writes must hold the world exclusively, or be a view
    /// that declared writing the component.
    #[inline]
    pub(crate) fn column_base(&self, id: ComponentId) -> Option<*mut u8> {
        self.column_index(id).map(|i| self.columns[i].base())
    }

    /// Reorders all rows: new row `i` gets old row `perm[i]`. `perm` must be
    /// a permutation of `0..len()`. The caller updates entity locations.
    pub(crate) fn permute(&mut self, perm: &[u32]) {
        assert_eq!(perm.len(), self.len());
        debug_assert!({
            let mut seen = vec![false; perm.len()];
            perm.iter().all(|&p| !std::mem::replace(&mut seen[p as usize], true))
        });
        let cap = self.cap();
        for c in self.columns.iter_mut() {
            unsafe { c.permute(perm, cap) };
        }
        let old = std::mem::take(&mut self.entities);
        self.entities = perm.iter().map(|&p| old[p as usize]).collect();
    }

    #[inline]
    pub(crate) fn reserve(&mut self, additional: usize, pool: &mut BufferPool) {
        let needed = self.len().checked_add(additional).expect("too many entities");
        if needed > self.cap() {
            self.grow(needed, pool);
        }
    }

    /// The rare half of [`reserve`](Self::reserve): room for `needed` rows.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, needed: usize, pool: &mut BufferPool) {
        let cap = self.cap();
        let new_cap = needed.max(cap * 2).max(8);
        for c in self.columns.iter_mut() {
            unsafe { c.grow(cap, new_cap, pool) }
        }
        self.cap = new_cap | (self.cap & TRACKED);
        self.grow_entities(new_cap, pool);
    }

    /// Grows the entity list to `cap` handles, with a buffer from `pool` if
    /// it has one, giving the old one to it.
    fn grow_entities(&mut self, cap: usize, pool: &mut BufferPool) {
        let len = self.entities.len();
        if self.entities.capacity() >= cap {
            return;
        }
        let Some(p) = pool.take(Layout::array::<Entity>(cap).expect("too many entities")) else {
            self.entities.reserve_exact(cap - len);
            return;
        };
        // SAFETY: `p` was allocated by the global allocator with the layout
        // of `cap` handles, and nothing else points to it.
        let mut fresh = unsafe { Vec::from_raw_parts(p.as_ptr().cast::<Entity>(), 0, cap) };
        fresh.extend_from_slice(&self.entities);
        give_entities(std::mem::replace(&mut self.entities, fresh), pool);
    }

    /// Makes room for one more row and, if this archetype tracks changes,
    /// stamps that row as added at `changes`' current tick: what a spawn
    /// does before it writes the row.
    ///
    /// For an archetype that tracks nothing and has room, this is the one
    /// comparison `reserve(1)` makes: the tracked flag is the sign bit of
    /// `cap`, so a tracked archetype fails the signed comparison and takes
    /// the second path together with a full one. (A length can't reach the
    /// sign bit: a `Vec` holds at most `isize::MAX` bytes.)
    pub(crate) fn reserve_row(&mut self, changes: &crate::change::Changes, pool: &mut BufferPool) {
        if (self.len() + 1) as isize <= self.cap as isize {
            return;
        }
        self.reserve_row_slow(changes.stamp(), pool);
    }

    /// The rare half of [`reserve_row`](Self::reserve_row): the archetype is
    /// full, or it is tracked.
    #[inline(never)]
    fn reserve_row_slow(&mut self, stamp: u32, pool: &mut BufferPool) {
        self.reserve(1, pool);
        if self.tracked() {
            self.stamp_row(self.len(), stamp);
        }
    }

    /// Removes `row` by moving the last row into it, dropping the removed
    /// values. Returns the entity that moved into `row`, if any.
    pub(crate) unsafe fn swap_remove_drop(&mut self, row: usize) -> Option<Entity> {
        let last = self.len() - 1;
        for c in self.columns.iter_mut() {
            if let Some(d) = c.drop {
                unsafe { d(c.ptr(row)) }
            }
            unsafe { c.swap_remove_forget(row, last) }
        }
        if self.tracked() {
            self.swap_remove_stamps(row, last);
        }
        self.entities.swap_remove(row);
        (row != last).then(|| self.entities[row])
    }

    // ---- change stamps (tracked components only) ---------------------------

    /// `true` if any component here keeps change stamps.
    #[inline(always)]
    pub(crate) fn tracked(&self) -> bool {
        self.cap & TRACKED != 0
    }

    /// Rows every column has room for.
    #[inline]
    fn cap(&self) -> usize {
        self.cap & !TRACKED
    }

    /// Starts keeping stamps for column `col`; rows already here count as
    /// added and changed at `stamp`.
    pub(crate) fn start_tracking(&mut self, col: usize, stamp: u32) {
        let (cap, len) = (self.cap(), self.len());
        let column = &mut self.columns[col];
        if !column.tracked {
            // SAFETY: untracked, and `len <= cap` rows are in use.
            unsafe { column.start_tracking(cap, len, stamp) };
            self.cap |= TRACKED;
        }
    }

    /// Stamps every tracked column of `row` as added and changed at `stamp`.
    /// `row` must be within the capacity (a row being spawned).
    pub(crate) fn stamp_row(&self, row: usize, stamp: u32) {
        let cap = self.cap();
        debug_assert!(row < cap);
        for c in self.columns.iter() {
            if let Some((added, changed)) = c.stamps(cap) {
                // SAFETY: `row < cap`, the length of both stamp arrays.
                unsafe {
                    added.add(row).write(stamp);
                    changed.add(row).write(stamp);
                }
            }
        }
    }

    /// Stamps column `col` of `row`: changed at `stamp`, and added too if
    /// `added`. Does nothing for an untracked column.
    #[inline]
    pub(crate) fn stamp(&self, col: usize, row: usize, stamp: u32, added: bool) {
        debug_assert!(row < self.cap());
        if let Some((a, c)) = self.columns[col].stamps(self.cap()) {
            // SAFETY: `row < cap`, the length of both stamp arrays.
            unsafe {
                c.add(row).write(stamp);
                if added {
                    a.add(row).write(stamp);
                }
            }
        }
    }

    /// The live `changed` stamps of component `id`, to mark rows as changed;
    /// `None` if it isn't here or isn't tracked.
    pub(crate) fn changed_stamps(&self, id: ComponentId) -> Option<*mut u32> {
        self.columns[self.column_index(id)?].stamps(self.cap()).map(|s| s.1)
    }

    /// The live stamps of component `id` for reading: its `added` array if
    /// `added`, else its `changed` array.
    pub(crate) fn read_stamps(&self, id: ComponentId, added: bool) -> Option<*const u32> {
        let (a, c) = self.columns[self.column_index(id)?].stamps(self.cap())?;
        Some(if added { a.cast_const() } else { c.cast_const() })
    }

    /// Moves the stamps of row `last` into `row` (the stamp half of a swap-remove).
    pub(crate) fn swap_remove_stamps(&self, row: usize, last: usize) {
        if row == last {
            return;
        }
        let cap = self.cap();
        for c in self.columns.iter() {
            if let Some((added, changed)) = c.stamps(cap) {
                // SAFETY: both rows are live rows of this archetype.
                unsafe {
                    added.add(row).write(added.add(last).read());
                    changed.add(row).write(changed.add(last).read());
                }
            }
        }
    }

    /// Copies the stamps of `row` to `dst_row` of `dst`, for every tracked
    /// component both archetypes have: moving an entity between archetypes
    /// isn't a change to the components it keeps. `dst_row` must be within
    /// `dst`'s capacity.
    pub(crate) fn copy_stamps_to(&self, row: usize, dst: &Archetype, dst_row: usize) {
        debug_assert!(row < self.len() && dst_row < dst.cap());
        let mut j = 0;
        let cap = self.cap();
        for (i, c) in self.columns.iter().enumerate() {
            let Some((added, changed)) = c.stamps(cap) else {
                continue;
            };
            let id = self.ids[i];
            while j < dst.ids.len() && dst.ids[j] < id {
                j += 1;
            }
            if j < dst.ids.len() && dst.ids[j] == id {
                // A component is tracked in every archetype or in none.
                let (dst_added, dst_changed) = dst.columns[j].stamps(dst.cap()).expect("tracked in both");
                // SAFETY: `row` is live here; `dst_row` is within `dst`'s capacity.
                unsafe {
                    dst_added.add(dst_row).write(added.add(row).read());
                    dst_changed.add(dst_row).write(changed.add(row).read());
                }
            }
        }
    }

    /// Ages stamps so they stay comparable after the tick counter's low 32
    /// bits wrap: any stamp older than `max_age` ticks becomes exactly
    /// `max_age` ticks old. `now` is the current tick's low 32 bits.
    pub(crate) fn age_stamps(&self, now: u32, max_age: u32) {
        let (len, cap) = (self.len(), self.cap());
        for c in self.columns.iter() {
            let Some((added, changed)) = c.stamps(cap) else {
                continue;
            };
            for stamps in [added, changed] {
                for row in 0..len {
                    // SAFETY: `row` is a live row of a tracked column.
                    unsafe {
                        let stamp = stamps.add(row);
                        if now.wrapping_sub(stamp.read()) > max_age {
                            stamp.write(now.wrapping_sub(max_age));
                        }
                    }
                }
            }
        }
    }
}

impl Drop for Archetype {
    fn drop(&mut self) {
        let (len, cap) = (self.len(), self.cap());
        for c in self.columns.iter_mut() {
            if let Some(d) = c.drop {
                for row in 0..len {
                    unsafe { d(c.ptr(row)) }
                }
            }
            unsafe { c.dealloc(cap) }
        }
    }
}

/// The storage a reclaimed archetype leaves behind, kept for the next new
/// archetype: its id lists, column headers and edge maps, all empty, the
/// maps with their capacity. Its column buffers and entity list went to a
/// [`BufferPool`].
pub(crate) struct Shell {
    ids: Box<[ComponentId]>,
    /// The boxed key of the world's archetype map, once taken back from it.
    pub(crate) key: Option<Box<[ComponentId]>>,
    columns: Box<[Column]>,
    add_edges: EdgeMap,
    remove_edges: EdgeMap,
}

impl Shell {
    /// Components the archetype had: how many ids and columns its boxes hold.
    pub(crate) fn width(&self) -> usize {
        self.ids.len()
    }
}

// SAFETY: as for `Archetype`: the column headers own no buffer any more.
unsafe impl Send for Shell {}
unsafe impl Sync for Shell {}

/// Most bytes of column buffers a world keeps for reuse.
const POOL_BYTES: usize = 4 << 20;
/// Most different buffer layouts it keeps them for.
const POOL_LAYOUTS: usize = 64;

/// Column buffers and entity lists of reclaimed archetypes, and the ones
/// archetypes outgrew, by layout, for archetypes that grow: a world whose
/// component combinations come and go then reaches a state where it
/// allocates nothing (issue #347). Bounded by
/// [`POOL_BYTES`] and [`POOL_LAYOUTS`]; what doesn't fit is freed.
#[derive(Default)]
pub(crate) struct BufferPool {
    lists: Vec<(Layout, Vec<NonNull<u8>>)>,
    bytes: usize,
}

// SAFETY: the pool owns plain buffers no one else points to.
unsafe impl Send for BufferPool {}
unsafe impl Sync for BufferPool {}

impl BufferPool {
    /// A buffer allocated with exactly `layout`, if the pool holds one.
    fn take(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        let (_, list) = self.lists.iter_mut().find(|(l, _)| *l == layout)?;
        let p = list.pop()?;
        self.bytes -= layout.size();
        Some(p)
    }

    /// Keeps `p`, a buffer allocated with `layout`; `false` if the pool is
    /// full and the caller must free it.
    fn put(&mut self, layout: Layout, p: NonNull<u8>) -> bool {
        if self.bytes + layout.size() > POOL_BYTES {
            return false;
        }
        let list = match self.lists.iter().position(|(l, _)| *l == layout) {
            Some(i) => &mut self.lists[i].1,
            None if self.lists.len() < POOL_LAYOUTS => {
                self.lists.push((layout, Vec::new()));
                &mut self.lists.last_mut().expect("just pushed").1
            }
            None => return false,
        };
        list.push(p);
        self.bytes += layout.size();
        true
    }

    /// Bytes of buffers held.
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for BufferPool {
    fn drop(&mut self) {
        for (layout, list) in &self.lists {
            for p in list {
                // SAFETY: every buffer in the list was allocated with its layout.
                unsafe { alloc::dealloc(p.as_ptr(), *layout) }
            }
        }
    }
}

/// Gives the buffer of an entity list to `pool`, or frees it.
fn give_entities(entities: Vec<Entity>, pool: &mut BufferPool) {
    if entities.capacity() == 0 {
        return;
    }
    let layout = Layout::array::<Entity>(entities.capacity()).expect("a list's own layout");
    let mut entities = std::mem::ManuallyDrop::new(entities);
    // `Entity` is `Copy`: forgetting the handles drops nothing.
    let p = NonNull::new(entities.as_mut_ptr().cast::<u8>()).expect("a list with capacity");
    if !pool.put(layout, p) {
        drop(std::mem::ManuallyDrop::into_inner(entities));
    }
}

/// `TypeId` is already a good hash; skip SipHash for type lookups.
#[derive(Default)]
pub(crate) struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 ^= n;
    }
}

pub(crate) type TypeMap<V> = HashMap<TypeId, V, BuildHasherDefault<TypeIdHasher>>;

/// Component ids are small dense integers: one multiply is a good hash, and
/// SipHash was most of the cost of a cached structural change.
#[derive(Default)]
pub(crate) struct ComponentIdHasher(u64);

impl Hasher for ComponentIdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// Transition-edge cache of an archetype: component id -> destination archetype.
pub(crate) type EdgeMap = HashMap<ComponentId, Edge, BuildHasherDefault<ComponentIdHasher>>;

/// A transition edge: the archetype that adding or removing a component
/// leads to, and the generation its slot had then. Reclaiming an archetype
/// moves its slot to the next generation, which makes every edge to it
/// stale without touching its neighbours (`Slots::leads`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edge {
    pub(crate) to: u32,
    pub(crate) generation: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loops over many archetypes read these structs once per archetype, so
    /// their size is part of the cost of every query. Change tracking keeps
    /// its flag in `Archetype::cap` and its stamps in the column buffers so
    /// that worlds which don't use it see the same structs as before.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn change_tracking_adds_no_bytes_to_columns_or_archetypes() {
        assert_eq!(std::mem::size_of::<Column>(), 32);
        assert_eq!(std::mem::size_of::<Archetype>(), 128);
    }

    #[test]
    fn a_tracked_buffer_holds_the_values_then_aligned_stamps() {
        // Three 1-byte values, then (after one byte of padding) 3 + 3 stamps.
        let mut column = Column::new(Layout::new::<u8>(), None);
        assert_eq!(column.array(3), Layout::from_size_align(3, 1).unwrap());
        assert_eq!(column.stamps(3), None);
        // SAFETY: untracked, and no rows are in use.
        unsafe { column.start_tracking(0, 0, 0) };
        assert_eq!(column.array(0).size(), 0);
        assert_eq!(column.array(3), Layout::from_size_align(4 + 3 * 8, 4).unwrap());
        assert_eq!(column.stamps_offset(3), 4);
        // A wider alignment is kept; zero-sized values leave only stamps.
        let mut wide = Column::new(Layout::new::<u128>(), None);
        let mut empty = Column::new(Layout::new::<()>(), None);
        // SAFETY: as above.
        unsafe {
            wide.start_tracking(0, 0, 0);
            empty.start_tracking(0, 0, 0);
        }
        assert_eq!(wide.array(2), Layout::from_size_align(32 + 16, 16).unwrap());
        assert_eq!(empty.array(5), Layout::from_size_align(40, 4).unwrap());
    }
}
