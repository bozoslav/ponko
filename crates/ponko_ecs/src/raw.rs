//! Explicit access to the dynamic ECS interface used by native bindings.
//!
//! The typed API (`spawn`, `query`, `get`) is the way to use a [`World`] from
//! Rust. What C, C++ and tools need on top of it (components by id and
//! pointer, resources by name and layout, whole archetype chunks) goes
//! through [`World::raw`] and [`World::raw_ref`], and nowhere else: `World`
//! itself has no method that takes a component id and a pointer.
//!
//! The split follows the borrow. Everything that only reads (pointers for
//! reading, resource layouts, names, removal logs, archetype tables) is on
//! the shared view, [`World::raw_ref`] ([`RawWorldRef`]), and hands out
//! `*const`. Everything that changes the world or hands out pointers for
//! writing is on the exclusive one, [`World::raw`] ([`RawWorld`]). Neither
//! derefs to `World`: the typed API is called on the world itself.
use crate::{Archetype, Chunk, ChunkQuery, ComponentId, DropFn, Entity, Tick, World};
use std::alloc::Layout;

/// Exclusive view of a world's dynamic API: registering, inserting,
/// removing and writing components and resources by id or name, the chunk
/// loops and change tracking. The calls that only read are on
/// [`RawWorldRef`] ([`World::raw_ref`]).
///
/// This borrows the same world; it does not copy storage or change the safety
/// requirements of dynamic methods. In particular, raw pointers remain subject
/// to the documented layout, lifetime and structural-mutation requirements.
pub struct RawWorld<'a>(&'a mut World);

/// Shared view of a world's dynamic API: pointers for reading, resource
/// layouts, component names, removal logs and the archetype tables. What
/// it returns borrows the world, not the view. The calls that change the
/// world are on [`RawWorld`] ([`World::raw`]).
#[derive(Clone, Copy)]
pub struct RawWorldRef<'a>(&'a World);
impl World {
    /// The dynamic API's calls that change the world or write through
    /// pointers. The read-only ones are on [`raw_ref`](Self::raw_ref).
    #[inline]
    pub fn raw(&mut self) -> RawWorld<'_> {
        RawWorld(self)
    }
    /// The dynamic API's read-only calls. The ones that change the world
    /// are on [`raw`](Self::raw).
    #[inline]
    pub fn raw_ref(&self) -> RawWorldRef<'_> {
        RawWorldRef(self)
    }
}

impl<'a> RawWorld<'a> {
    /// Registers a component known only by name, size and alignment: the path
    /// for components defined in C/C++ or loaded at runtime.
    ///
    /// Registering the same name again with the same layout returns the same id.
    ///
    /// ```
    /// use ponko_ecs::World;
    ///
    /// let mut world = World::new();
    /// // SAFETY: f32 is plain bytes.
    /// let speed = unsafe { world.raw().register_component("Speed", 4, 4, None) };
    /// assert_eq!(world.component_info(speed).name(), "Speed");
    /// assert_eq!(world.raw_ref().component_by_name("Speed"), Some(speed));
    /// ```
    ///
    /// # Safety
    /// Values of this component must be safe to move with `memcpy`, and to send
    /// and share between threads. `drop`, if given, must accept any value
    /// written as this component.
    ///
    /// # Panics
    /// If the name is taken with a different layout, or the layout is invalid.
    #[inline]
    pub unsafe fn register_component(
        &mut self,
        name: &str,
        size: usize,
        align: usize,
        drop: Option<DropFn>,
    ) -> ComponentId {
        // SAFETY: forwarded from the caller.
        unsafe { self.0.register_component_inner(name, size, align, drop) }
    }

    /// Pointer to component `id` of entity `e`, for reading and writing, or
    /// `None`. It stays valid until the world changes structurally (a spawn,
    /// despawn, insert or remove may move the value).
    ///
    /// Writing through it doesn't count as a change: the world can't see the
    /// write. Call [`mark_changed_dynamic`](Self::mark_changed_dynamic)
    /// afterwards if the component is tracked.
    ///
    /// ```
    /// use ponko_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Speed(f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn((Speed(1.5),));
    /// let id = world.component_id::<Speed>().unwrap();
    /// let p = world.raw().get_dynamic_mut(e, id).unwrap().cast::<Speed>();
    /// // SAFETY: the pointer is to this entity's live `Speed`, and the world
    /// // isn't used between getting it and writing through it.
    /// unsafe { (*p).0 = 3.0 };
    /// assert_eq!(world.get::<Speed>(e).unwrap().0, 3.0);
    /// ```
    #[must_use]
    #[inline]
    pub fn get_dynamic_mut(&mut self, e: Entity, id: ComponentId) -> Option<*mut u8> {
        self.0.get_dynamic_inner(e, id, true)
    }

    /// Removes component `id` from `e`, dropping the value. Returns `false` if
    /// `e` is dead or didn't have it.
    #[inline]
    pub fn remove_dynamic(&mut self, e: Entity, id: ComponentId) -> bool {
        self.0.remove_dynamic_inner(e, id)
    }

    /// Adds (or replaces) component `id` on `e` from a raw value.
    /// Returns `false` if `e` is dead; the value is then still the caller's.
    ///
    /// # Safety
    /// `value` must point to a valid value of component `id`; on success the
    /// world takes ownership of it.
    #[inline]
    pub unsafe fn insert_dynamic(&mut self, e: Entity, id: ComponentId, value: *const u8) -> bool {
        // SAFETY: forwarded from the caller.
        unsafe { self.0.insert_dynamic_inner(e, id, value) }
    }

    /// Spawns an entity from raw component values.
    ///
    /// # Safety
    /// Each pointer must point to a valid value of its component; the world
    /// takes ownership (the caller must not drop it). Ids must be from this world.
    ///
    /// # Panics
    /// If a component id appears twice.
    #[inline]
    pub unsafe fn spawn_dynamic(&mut self, components: &[(ComponentId, *const u8)]) -> Entity {
        // SAFETY: forwarded from the caller.
        unsafe { self.0.spawn_dynamic_inner(components) }
    }

    /// [`spawn_dynamic`](Self::spawn_dynamic) with a given id, like
    /// [`spawn_at`](crate::World::spawn_at): returns `false` (and takes nothing) if
    /// that slot is taken.
    ///
    /// ```
    /// use ponko_ecs::{Entity, World};
    ///
    /// let mut world = World::new();
    /// // SAFETY: f32 is plain bytes.
    /// let id = unsafe { world.raw().register_component("Speed", 4, 4, None) };
    /// let saved = Entity::from_bits(5 | 2 << 32);
    /// let speed = 1.5f32;
    /// // SAFETY: points to a valid f32, which needs no drop.
    /// assert!(unsafe { world.raw().spawn_at_dynamic(saved, &[(id, (&speed as *const f32).cast())]) });
    /// assert!(!unsafe { world.raw().spawn_at_dynamic(saved, &[(id, (&speed as *const f32).cast())]) });
    /// assert_eq!(world.len(), 1);
    /// ```
    ///
    /// # Safety
    /// As for [`spawn_dynamic`](Self::spawn_dynamic). When this returns
    /// `false`, the values stay the caller's.
    ///
    /// # Panics
    /// If a component id appears twice.
    #[inline]
    pub unsafe fn spawn_at_dynamic(&mut self, e: Entity, components: &[(ComponentId, *const u8)]) -> bool {
        // SAFETY: forwarded from the caller.
        unsafe { self.0.spawn_at_dynamic_inner(e, components) }
    }

    /// Pointer to resource `name`, for reading and writing, or `None`. It
    /// stays valid until the resource is replaced or removed.
    ///
    /// ```
    /// use ponko_ecs::World;
    /// use std::alloc::Layout;
    ///
    /// let mut world = World::new();
    /// let score = 3u32;
    /// // SAFETY: `score` is 4 readable bytes.
    /// unsafe { world.raw().insert_resource_dynamic("Score", Layout::new::<u32>(), (&score as *const u32).cast()) };
    /// let p = world.raw().get_resource_dynamic_mut("Score").unwrap().cast::<u32>();
    /// // SAFETY: the resource holds a u32, and the world isn't used in between.
    /// unsafe { *p += 1 };
    /// let p = world.raw_ref().get_resource_dynamic("Score").unwrap().cast::<u32>();
    /// // SAFETY: as above.
    /// assert_eq!(unsafe { *p }, 4);
    /// ```
    #[must_use]
    #[inline]
    pub fn get_resource_dynamic_mut(&mut self, name: &str) -> Option<*mut u8> {
        self.0.get_resource_dynamic_inner(name)
    }

    /// Stores a resource that has no Rust type, by name: a copy of
    /// `layout.size()` bytes from `value`, replacing any resource of that
    /// name. This is how C and C++ games keep global values.
    ///
    /// ```
    /// use ponko_ecs::*;
    /// use std::alloc::Layout;
    ///
    /// let mut world = World::new();
    /// let score: [u32; 2] = [3, 1];
    /// // SAFETY: `score` is 8 readable bytes.
    /// unsafe { world.raw().insert_resource_dynamic("Score", Layout::new::<[u32; 2]>(), score.as_ptr().cast()) };
    /// let p = world.raw_ref().get_resource_dynamic("Score").unwrap().cast::<[u32; 2]>();
    /// // SAFETY: the resource holds the bytes of a [u32; 2].
    /// assert_eq!(unsafe { *p }, [3, 1]);
    /// ```
    ///
    /// # Safety
    /// `value` points to `layout.size()` readable bytes (any pointer for size 0).
    ///
    /// # Panics
    /// If a resource of that name exists with a different layout.
    #[track_caller]
    #[inline]
    pub unsafe fn insert_resource_dynamic(&mut self, name: &str, layout: Layout, value: *const u8) {
        // SAFETY: forwarded from the caller.
        unsafe { self.0.insert_resource_dynamic_inner(name, layout, value) }
    }

    /// Removes resource `name`; `false` if there was none.
    #[inline]
    pub fn remove_resource_dynamic(&mut self, name: &str) -> bool {
        self.0.remove_resource_dynamic_inner(name)
    }

    /// Calls `f` once per archetype that has all of `ids`, with raw column
    /// pointers in `ids` order. This is how C/C++ systems see the world: whole
    /// columns, never one call per entity.
    ///
    /// # Panics
    ///
    /// With more than 32 component ids.
    #[inline]
    pub fn for_each_chunk(&mut self, ids: &[ComponentId], f: impl FnMut(Chunk<'_>)) {
        self.0.for_each_chunk_inner(ids, f)
    }

    /// [`for_each_chunk`](Self::for_each_chunk) with what a [`ChunkQuery`]
    /// adds: per-column access (only the columns in its `writes` count as
    /// changed), change filters (with `changed` or `added` set, `f` gets the
    /// runs of consecutive rows that pass instead of whole archetypes), an
    /// archetype [`filter`](ChunkQuery::filter) and a
    /// [`cache`](ChunkQuery::cache) hint.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// let mut world = World::new();
    /// // SAFETY: f32 is plain bytes.
    /// let speed = unsafe { world.raw().register_component("Speed", 4, 4, None) };
    /// world.raw().track_changes_dynamic(speed);
    /// let spawn = |world: &mut World, value: f32| {
    ///     // SAFETY: points to a valid f32, which needs no drop.
    ///     unsafe { world.raw().spawn_dynamic(&[(speed, (&value as *const f32).cast())]) }
    /// };
    /// let (a, b) = (spawn(&mut world, 1.0), spawn(&mut world, 2.0));
    /// let seen = world.advance_change_tick();
    /// assert!(world.raw().mark_changed_dynamic(b, speed));
    ///
    /// // Read-only, and only the rows whose `Speed` changed after `seen`.
    /// let ids = [speed];
    /// let query = ChunkQuery::new(&ids).writes(0).changed(1).since(seen);
    /// let mut changed = Vec::new();
    /// world.raw().query_chunks(query, |chunk| changed.extend_from_slice(chunk.entities));
    /// assert_eq!(changed, [b]);
    /// # let _ = a;
    /// ```
    ///
    /// # Panics
    ///
    /// With more than 32 component ids, more than 8 `changed` and `added`
    /// bits together, or a `changed` or `added` bit on a component that
    /// isn't tracked ([`track_changes_dynamic`](Self::track_changes_dynamic)):
    /// tracking is explicit, as for [`Changed`](crate::Changed).
    #[track_caller]
    #[inline]
    pub fn query_chunks(&mut self, query: ChunkQuery<'_>, f: impl FnMut(Chunk<'_>)) {
        self.0.query_chunks_inner(query, f)
    }

    /// [`World::track_changes`] for a component by id: the path for
    /// components registered with
    /// [`register_component`](Self::register_component).
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// let mut world = World::new();
    /// // SAFETY: f32 is plain bytes.
    /// let speed = unsafe { world.raw().register_component("Speed", 4, 4, None) };
    /// world.raw().track_changes_dynamic(speed);
    /// assert!(world.component_info(speed).tracks_changes());
    /// ```
    ///
    /// # Panics
    /// If `id` came from a different world.
    #[inline]
    pub fn track_changes_dynamic(&mut self, id: ComponentId) {
        self.0.track_changes_dynamic_inner(id)
    }

    /// [`World::mark_changed`] by component id: call it after writing
    /// through a pointer from [`get_dynamic_mut`](Self::get_dynamic_mut).
    /// Returns `false` if `e` is dead or doesn't have the component; does
    /// nothing (and returns `true`) if the component isn't tracked.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// let mut world = World::new();
    /// // SAFETY: f32 is plain bytes.
    /// let speed = unsafe { world.raw().register_component("Speed", 4, 4, None) };
    /// world.raw().track_changes_dynamic(speed);
    /// let value = 1.5f32;
    /// // SAFETY: points to a valid f32, which needs no drop.
    /// let e = unsafe { world.raw().spawn_dynamic(&[(speed, (&value as *const f32).cast())]) };
    /// let seen = world.advance_change_tick();
    ///
    /// let p = world.raw().get_dynamic_mut(e, speed).unwrap().cast::<f32>();
    /// // SAFETY: the pointer is to this entity's live f32.
    /// unsafe { *p = 3.0 };
    /// assert!(world.raw().mark_changed_dynamic(e, speed));
    /// # let _ = seen;
    /// ```
    #[inline]
    pub fn mark_changed_dynamic(&mut self, e: Entity, id: ComponentId) -> bool {
        self.0.mark_changed_dynamic_inner(e, id)
    }
}

impl<'a> RawWorldRef<'a> {
    /// Pointer to component `id` of entity `e`, for reading, or `None`. A
    /// shared world doesn't give the right to write: use
    /// [`RawWorld::get_dynamic_mut`] for that.
    #[must_use]
    #[inline]
    pub fn get_dynamic(&self, e: Entity, id: ComponentId) -> Option<*const u8> {
        self.0.get_dynamic_inner(e, id, false).map(<*mut u8>::cast_const)
    }

    /// Pointer to resource `name` (one stored with
    /// [`insert_resource_dynamic`](RawWorld::insert_resource_dynamic)), for
    /// reading, or `None`. It stays valid until the resource is replaced or
    /// removed. To write, use [`RawWorld::get_resource_dynamic_mut`].
    #[must_use]
    #[inline]
    pub fn get_resource_dynamic(&self, name: &str) -> Option<*const u8> {
        self.0.get_resource_dynamic_inner(name).map(<*mut u8>::cast_const)
    }

    /// [`World::removed_since`] by component id: the entities that lost
    /// tracked component `id` after `since`, oldest first.
    ///
    /// ```
    /// # use ponko_ecs::*;
    /// # #[derive(Component)]
    /// # struct Level(u8);
    /// let mut world = World::new();
    /// let id = world.track_changes::<Level>();
    /// let e = world.spawn((Level(1),));
    /// world.despawn(e);
    /// assert_eq!(world.raw_ref().removed_dynamic(id, Tick::default()), [e]);
    /// ```
    ///
    /// # Panics
    /// If `id` isn't tracked, or came from a different world.
    #[track_caller]
    #[must_use]
    #[inline]
    pub fn removed_dynamic(&self, id: ComponentId, since: Tick) -> &'a [Entity] {
        self.0.removed_dynamic_inner(id, since)
    }

    /// The size and alignment of resource `name`, if it exists.
    #[must_use]
    #[inline]
    pub fn resource_layout_dynamic(&self, name: &str) -> Option<Layout> {
        self.0.resource_layout_dynamic_inner(name)
    }

    /// Looks a component up by name (registered name, or Rust type name).
    #[must_use]
    #[inline]
    pub fn component_by_name(&self, name: &str) -> Option<ComponentId> {
        self.0.component_by_name_inner(name)
    }

    /// The archetype tables, for tools such as the inspector.
    ///
    /// An index into this slice names a table only until the next
    /// structural change: a world reclaims tables that stayed empty and
    /// builds new component sets in their slots. A reclaimed slot shows as
    /// an empty table without components.
    #[must_use]
    #[inline]
    pub fn archetypes(&self) -> &'a [Archetype] {
        self.0.archetypes_inner()
    }
}
