//! What the frame's model collection keeps between frames, so a frame only
//! pays for what changed since the last one.
//!
//! Three parts, each owned by a [`Frame3D`](super::collect::Frame3D) (the
//! main view, every target camera and the global illumination capture keep
//! their own):
//!
//! - [`Lists`]: the instances each view draws (the camera, then every shadow
//!   view), grouped by mesh. Every drawn thing has a persistent id and, in
//!   each view that draws it, a slot in the frame's instance array that it
//!   keeps for as long as that view draws it with that mesh. A thing that
//!   enters a view takes the next free slot of its mesh's block; one that
//!   leaves hands its slot to the block's last instance. So a frame writes
//!   only the instances that changed, and the renderer, which uploads the
//!   instances that differ from the frame before, uploads little more than
//!   those.
//! - [`Tracked`]: every plain [`Model`] in the world, by entity: a digest
//!   of what it was collected from (transform, mesh, tint, shadow mesh,
//!   material override, decal layers), its bounding spheres in the world,
//!   and which views draw it. A frame compares each model with its digest;
//!   only models that differ are worked out again, and models are culled
//!   again only when they or the views changed. A still scene under a
//!   still camera costs one digest per model.
//! - [`Transients`]: things whose drawing depends on the camera or is posed
//!   each frame (level-of-detail renderers, terrain tiles, skinned models,
//!   shadow-only casters). They are worked out every frame as before, and
//!   matched by a key to the id (and so the slots) they had.
//!
//! `Tracked` collects in one of two ways, and a collection can change
//! between them from frame to frame:
//!
//! - **One thread** (a browser, a single core, `RAYON_NUM_THREADS=1`): one
//!   pass compares, works out, culls and writes each model's instance
//!   straight to its slots. Nothing is staged.
//! - **Several threads**: the compare runs on all cores. A task writes the
//!   instance of a changed model that keeps its places straight to its
//!   slots; for a model that enters or leaves a view, or changes mesh, it
//!   stages what the lists need, and they take it in afterwards on one
//!   thread, in the models' order (so a frame's lists don't depend on how
//!   the work was split).
//!
//! While more than half the models change every frame, comparing costs
//! more than it saves, so a frame **rushes**: it works every model out
//! and culls it from its components alone, as a frame that keeps nothing
//! would, and looks up only the models some view draws, to write them
//! where they were. One model in 64 is still compared; when fewer than a
//! quarter of those differ the rush ends, and the next frame works
//! everything out again and keeps it.
//!
//! Change detection compares values, so it needs nothing from the ECS and
//! holds for any way a component is written. The digest compare (in
//! `Tracked::settle` and `Pass::check`) is the one place a change tick
//! would replace: with ticks, whole archetypes whose tick is old can skip
//! the compare too. Two things rely on every model being visited every
//! frame, and need another source once archetypes are skipped: a model
//! that left the world (despawned, or without its components now) is
//! noticed because a frame finds fewer models than are tracked, so removal
//! then needs the ECS's removal events or a count per archetype; and a
//! rush samples the models it visits.
//!
//! The digest is 64 bits of the model's components. Two different states
//! of one model with the same digest are possible in principle (about one
//! change in 2^63); the model would then stay drawn as it was until it
//! changes again.
//!
//! Costs to know about:
//! - The first frame, and any frame after the views empty, build everything
//!   (as every frame did before) and what later frames keep, and pack the
//!   array by view and mesh without room to grow, as a sorted frame is.
//! - A change to a mesh's bounds, or to whether it casts shadows, is
//!   skinned or has morph targets, works the models of that mesh out again;
//!   a change to the numbering of material overrides, the models with one.
//! - A block that fills up doubles, and moves to the end of the array if
//!   it isn't the last. When the array is more than twice what its blocks
//!   need (space left behind, or kept by blocks that shrank) it is packed
//!   again with room for half as many more in each block, which uploads
//!   it whole once.
//! - Instances of one mesh are kept in no particular order: it changes as
//!   models enter and leave the view. The renderer draws the instances of
//!   blended meshes from a copy it sorts far to near each frame, so what
//!   is blended over what doesn't depend on it.
//! - Per model in the world, in view or not: about 80 bytes of `Tracked`
//!   on one thread and 150 on several (which also stage instances), 24 for
//!   its id in the lists and 4 more per view that draws any model; and the
//!   drawn instances with their room to grow. Every `Frame3D` keeps its
//!   own: a target camera and the global illumination capture each pay
//!   this again for every model in the world.

use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use glam::{Quat, Vec3};

use super::collect::{Culler, FADED, Instance3D, MAX_CASCADES, MAX_SHADOW_VIEWS};
use super::motion::{IdHasher, Motion, NO_KEY};
use super::{
    Assets3D, Model, ModelMaterial, ModelMaterials, ReceivesDecals, SLOT_MASK, ShadowMesh, ShadowOnly, Skeleton,
    SkinOf, Transform3D, linear,
};
use ponko_ecs::{With, Without, World};

/// The views a frame draws models into: the camera (view 0), then the
/// shadow views (view `1 + v`). A thing's pass flags have bit `view` set
/// for each view that draws it.
pub(crate) const VIEWS: usize = 1 + MAX_SHADOW_VIEWS;
const VIEW_BITS: u32 = (1 << VIEWS) - 1;
/// Set in [`Applied::flags`] of a crossfading instance (see
/// [`Instance3D::with_fade`]): it is drawn in runs of its own.
const FADING: u32 = 1 << 31;
const NONE: u32 = u32::MAX;

/// The mesh a thing draws in the camera's view, then in each sun cascade
/// (light views draw the first cascade's).
pub(crate) type Meshes = [u32; 1 + MAX_CASCADES];

/// (mesh id, first instance, count).
type Run = (u32, u32, u32);

/// What the lists hold of one thing: where it is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Applied {
    meshes: Meshes,
    /// Bit `view` for each view that draws it, and [`FADING`].
    flags: u32,
}

const NOWHERE: Applied = Applied {
    meshes: [0; 1 + MAX_CASCADES],
    flags: 0,
};

impl Applied {
    fn new(meshes: &Meshes, flags: u32, instance: &Instance3D) -> Self {
        let flags = flags & VIEW_BITS;
        if flags == 0 {
            return NOWHERE;
        }
        let fading = if instance.fade() != 0.0 { FADING } else { 0 };
        Self {
            meshes: *meshes,
            flags: flags | fading,
        }
    }

    /// The block `view` draws it in: one per mesh, and another for the
    /// mesh's crossfading instances.
    fn bucket(&self, view: usize) -> usize {
        let mesh = match view {
            0 => self.meshes[0],
            v if v <= MAX_CASCADES => self.meshes[v],
            _ => self.meshes[1],
        };
        mesh as usize * 2 + usize::from(self.flags & FADING != 0)
    }
}

/// One mesh's instances in one view: `len` instances from `start` in the
/// frame's instance array, with room for `cap`.
#[derive(Clone, Copy, Default)]
struct Block {
    start: u32,
    len: u32,
    cap: u32,
}

#[derive(Default)]
struct ViewList {
    /// By bucket (see [`Applied::bucket`]).
    blocks: Vec<Block>,
    /// By id: the thing's slot in the instance array, while this view
    /// draws it (stale otherwise).
    slots: Vec<u32>,
    /// How many instances the view draws.
    count: u32,
}

/// Each view's instances, kept between frames: see the module docs.
/// The instances themselves are the frame's (`Frame3D::instances`), passed
/// in as `arena`.
pub(crate) struct Lists {
    views: [ViewList; VIEWS],
    /// By id.
    applied: Vec<Applied>,
    /// Ids given back.
    free: Vec<u32>,
    /// By slot of the instance array: the id drawn there.
    owners: Vec<u32>,
    /// Whether no view drew anything when the frame began: it is built
    /// tightly packed, in view and mesh order.
    fresh: bool,
    /// How many ids some view draws.
    things: usize,
    /// The buffers the last packing read from, for the next.
    spare: (Vec<Instance3D>, Vec<u32>),
    /// How many times the array was packed again.
    pub repacks: u32,
}

impl Default for Lists {
    fn default() -> Self {
        Self {
            views: std::array::from_fn(|_| ViewList::default()),
            applied: Vec::new(),
            free: Vec::new(),
            owners: Vec::new(),
            fresh: true,
            things: 0,
            spare: Default::default(),
            repacks: 0,
        }
    }
}

impl Lists {
    /// Starts a frame.
    pub fn begin(&mut self, arena: &mut Vec<Instance3D>) {
        self.fresh = self.views.iter().all(|v| v.count == 0);
        if self.fresh {
            arena.clear();
            self.owners.clear();
            for view in &mut self.views {
                view.blocks.clear();
            }
        }
    }

    /// Room for `more` ids: grows by that, or an eighth if that is more.
    pub fn reserve(&mut self, more: usize) {
        let (more, len) = (more.saturating_sub(self.free.len()), self.applied.len());
        if self.applied.capacity() - len < more {
            self.applied.reserve_exact(more.max(len / 8));
        }
    }

    /// A new id, drawn nowhere.
    pub fn alloc(&mut self) -> u32 {
        self.free.pop().unwrap_or_else(|| {
            self.applied.push(NOWHERE);
            self.applied.len() as u32 - 1
        })
    }

    /// Takes `id` out of every view and gives the id back.
    pub fn release(&mut self, arena: &mut Vec<Instance3D>, id: u32) {
        self.clear(arena, id);
        self.free.push(id);
    }

    /// Takes `id` out of every view.
    pub fn clear(&mut self, arena: &mut Vec<Instance3D>, id: u32) {
        self.set(arena, id, &NOWHERE.meshes, 0, &Instance3D::default());
    }

    /// The views that draw `id` (bit `view` each).
    pub fn flags(&self, id: u32) -> u32 {
        self.applied[id as usize].flags & VIEW_BITS
    }

    /// Whether `id` is drawn in exactly these views with these meshes.
    pub fn holds(&self, id: u32, meshes: &Meshes, flags: u32, instance: &Instance3D) -> bool {
        self.applied[id as usize] == Applied::new(meshes, flags, instance)
    }

    /// The slots of `id` in the instance array, if it is drawn in exactly
    /// these views (some) with these meshes: writing `instance` to them is
    /// then all that [`set`](Self::set) would do.
    fn held(&self, id: u32, meshes: &Meshes, flags: u32, instance: &Instance3D) -> Option<impl Iterator<Item = usize>> {
        let flags = flags & VIEW_BITS;
        (flags != 0 && self.holds(id, meshes, flags, instance)).then(|| {
            (0..VIEWS)
                .filter(move |view| flags & (1 << view) != 0)
                .map(move |view| self.views[view].slots[id as usize] as usize)
        })
    }

    /// Where `id` is in the instance array for `view`, which draws it.
    pub fn slot(&self, view: usize, id: u32) -> usize {
        debug_assert!(self.applied[id as usize].flags & (1 << view) != 0);
        self.views[view].slots[id as usize] as usize
    }

    /// Draws `id` as `instance` in the views of `flags` (bit `view` each;
    /// none: nowhere), with `meshes`: it keeps its slot in every view that
    /// drew it with the same mesh, leaves the others and enters the new.
    pub fn set(&mut self, arena: &mut Vec<Instance3D>, id: u32, meshes: &Meshes, flags: u32, instance: &Instance3D) {
        let old = self.applied[id as usize];
        let new = Applied::new(meshes, flags, instance);
        self.things = self.things + usize::from(new.flags != 0) - usize::from(old.flags != 0);
        let mut views = (old.flags | new.flags) & VIEW_BITS;
        while views != 0 {
            let view = views.trailing_zeros() as usize;
            views &= views - 1;
            let bit = 1 << view;
            match (old.flags & bit != 0, new.flags & bit != 0) {
                (true, true) => {
                    let (from, to) = (old.bucket(view), new.bucket(view));
                    if from == to {
                        arena[self.views[view].slots[id as usize] as usize] = *instance;
                    } else {
                        self.leave(arena, view, from, id);
                        self.enter(arena, view, to, id, instance);
                    }
                }
                (true, false) => self.leave(arena, view, old.bucket(view), id),
                _ => self.enter(arena, view, new.bucket(view), id, instance),
            }
        }
        self.applied[id as usize] = new;
    }

    fn enter(&mut self, arena: &mut Vec<Instance3D>, view: usize, bucket: usize, id: u32, instance: &Instance3D) {
        let ids = self.applied.len();
        let (list, owners) = (&mut self.views[view], &mut self.owners);
        if list.blocks.len() <= bucket {
            list.blocks.resize(bucket + 1, Block::default());
        }
        if list.slots.len() < ids {
            list.slots.resize(ids, NONE);
        }
        let mut block = list.blocks[bucket];
        if block.len == block.cap {
            let cap = (block.cap * 2).max(2);
            let (from, n) = (block.start as usize, block.len as usize);
            if block.cap > 0 && from + block.cap as usize == arena.len() {
                // The array's last block grows where it is.
                arena.resize(from + cap as usize, Instance3D::default());
                owners.resize(from + cap as usize, NONE);
            } else {
                // The others move to its end, leaving their place behind.
                let to = arena.len();
                assert!(to + (cap as usize) < NONE as usize, "too many 3D instances");
                arena.resize(to + cap as usize, Instance3D::default());
                owners.resize(to + cap as usize, NONE);
                arena.copy_within(from..from + n, to);
                owners.copy_within(from..from + n, to);
                for (k, &owner) in owners[to..to + n].iter().enumerate() {
                    list.slots[owner as usize] = (to + k) as u32;
                }
                block.start = to as u32;
            }
            block.cap = cap;
        }
        let at = block.start + block.len;
        arena[at as usize] = *instance;
        owners[at as usize] = id;
        list.slots[id as usize] = at;
        block.len += 1;
        list.blocks[bucket] = block;
        list.count += 1;
    }

    fn leave(&mut self, arena: &mut [Instance3D], view: usize, bucket: usize, id: u32) {
        let (list, owners) = (&mut self.views[view], &mut self.owners);
        let block = &mut list.blocks[bucket];
        let at = list.slots[id as usize] as usize;
        let last = (block.start + block.len - 1) as usize;
        debug_assert_eq!(owners[at], id);
        if at != last {
            // The block's last instance takes the freed slot.
            arena[at] = arena[last];
            let moved = owners[last];
            owners[at] = moved;
            list.slots[moved as usize] = at as u32;
        }
        block.len -= 1;
        list.count -= 1;
    }

    /// Ends a frame: packs the array again when it was built from nothing,
    /// or is more than twice what its blocks need with room to grow (space
    /// left behind by blocks that moved, or kept by blocks that shrank).
    pub fn finish(&mut self, arena: &mut Vec<Instance3D>) {
        if self.fresh {
            if !arena.is_empty() {
                self.repack(arena, true);
            }
            return;
        }
        let packed: usize = (self.views.iter().flat_map(|v| &v.blocks))
            .filter(|b| b.len > 0)
            .map(|b| room(b.len as usize))
            .sum();
        if arena.len() > 2 * packed + 1024 {
            self.repack(arena, false);
        }
    }

    /// Lays the blocks out again without holes: the camera's by mesh, then
    /// each shadow view's, every block's instances in their order. `tight`
    /// leaves no room to grow (a frame built from nothing is laid out as if
    /// it was sorted); otherwise each block gets room for half as many
    /// again (see [`room`]).
    fn repack(&mut self, arena: &mut Vec<Instance3D>, tight: bool) {
        let (mut packed, mut owners) = std::mem::take(&mut self.spare);
        packed.clear();
        owners.clear();
        for list in &mut self.views {
            for faded in 0..2 {
                for bucket in (faded..list.blocks.len()).step_by(2) {
                    let block = &mut list.blocks[bucket];
                    let (from, n) = (block.start as usize, block.len as usize);
                    if n == 0 {
                        *block = Block::default();
                        continue;
                    }
                    let start = packed.len();
                    packed.extend_from_slice(&arena[from..from + n]);
                    owners.extend_from_slice(&self.owners[from..from + n]);
                    for (k, &owner) in owners[start..].iter().enumerate() {
                        list.slots[owner as usize] = (start + k) as u32;
                    }
                    let cap = if tight { n } else { room(n) };
                    packed.resize(start + cap, Instance3D::default());
                    owners.resize(start + cap, NONE);
                    *block = Block {
                        start: start as u32,
                        len: n as u32,
                        cap: cap as u32,
                    };
                }
            }
        }
        std::mem::swap(arena, &mut packed);
        std::mem::swap(&mut self.owners, &mut owners);
        // What is kept for the next packing is no larger than this one.
        packed.clear();
        owners.clear();
        packed.shrink_to(arena.len());
        owners.shrink_to(arena.len());
        self.spare = (packed, owners);
        self.repacks += 1;
    }

    /// How many things some view draws, each counted once.
    pub fn things(&self) -> usize {
        self.things
    }

    /// How many instances `view` draws.
    pub fn count(&self, view: usize) -> usize {
        self.views[view].count as usize
    }

    /// Replaces `runs` with `view`'s: its meshes' whole instances in mesh
    /// order, then its crossfading ones (with [`FADED`] set in the mesh id).
    pub fn runs(&self, view: usize, runs: &mut Vec<Run>) {
        runs.clear();
        let blocks = &self.views[view].blocks;
        for (first, faded) in [(0, 0), (1, FADED)] {
            for bucket in (first..blocks.len()).step_by(2) {
                let block = blocks[bucket];
                if block.len > 0 {
                    runs.push(((bucket / 2) as u32 | faded, block.start, block.len));
                }
            }
        }
    }

    /// Bytes kept between frames, without the instance array itself.
    #[cfg(any(test, feature = "bench-internals"))]
    pub fn bytes(&self) -> usize {
        let views: usize = self
            .views
            .iter()
            .map(|v| v.blocks.capacity() * size_of::<Block>() + v.slots.capacity() * size_of::<u32>())
            .sum();
        views
            + self.applied.capacity() * size_of::<Applied>()
            + (self.free.capacity() + self.owners.capacity() + self.spare.1.capacity()) * size_of::<u32>()
            + self.spare.0.capacity() * size_of::<Instance3D>()
    }
}

/// The room a block of `n` instances is packed with: half as many again
/// can enter before it has to move. (A block that fills up doubles.)
fn room(n: usize) -> usize {
    n + n / 2 + 1
}

/// What a [`Model`] is collected from: its components, read where they
/// are (copying them into one value first costs more than the compare). A
/// 64-bit digest of them is kept per model, to tell when it changed.
#[derive(Clone, Copy)]
struct Source<'a> {
    transform: &'a Transform3D,
    /// Its mesh and its tint, as set (not yet linear).
    model: &'a Model,
    /// The decal layers it takes (see `projected::receiver_mask`).
    receives: f32,
    /// Its [`ShadowMesh`]: [`OWN_SHADOW`] without one, [`NO_SHADOW`] for
    /// `ShadowMesh(None)`.
    shadow: u32,
    /// Its [`ModelMaterial`], or [`NONE`].
    material: u32,
    /// Its [`ModelMaterials`], if it lists any part.
    parts: Option<&'a ModelMaterials>,
    /// Whether it has a [`Skeleton`].
    skeleton: bool,
    /// Whether it has [`ShadowOnly`]: no view but the shadow maps draws it.
    shadow_only: bool,
}

/// What a model's instance is made from: rotation, position, scale, tint,
/// decal layers.
const KEPT: usize = 15;

const OWN_SHADOW: u32 = u32::MAX;
const NO_SHADOW: u32 = u32::MAX - 1;

type ModelItem<'a> = (
    ponko_ecs::Entity,
    &'a Transform3D,
    &'a Model,
    Option<&'a ShadowMesh>,
    Option<&'a Skeleton>,
    Option<&'a ReceivesDecals>,
    Option<&'a ModelMaterial>,
    Option<&'a ModelMaterials>,
    Option<&'a ShadowOnly>,
);

impl<'a> Source<'a> {
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn of(
        transform: &'a Transform3D,
        model: &'a Model,
        shadow: Option<&ShadowMesh>,
        skeleton: Option<&Skeleton>,
        receives: Option<&ReceivesDecals>,
        material: Option<&ModelMaterial>,
        parts: Option<&'a ModelMaterials>,
        shadow_only: Option<&ShadowOnly>,
    ) -> Self {
        Self {
            transform,
            model,
            receives: super::projected::receiver_mask(receives),
            shadow: match shadow {
                None => OWN_SHADOW,
                Some(ShadowMesh(None)) => NO_SHADOW,
                Some(ShadowMesh(Some(mesh))) => mesh.id,
            },
            material: material.map_or(NONE, |o| o.0.id),
            parts: parts.filter(|p| !p.raw().is_empty()),
            skeleton: skeleton.is_some(),
            shadow_only: shadow_only.is_some(),
        }
    }

    /// What of `flags` (see [`Culler::classify`]) draws it: all of them,
    /// or only the shadow maps' for a [`ShadowOnly`] model.
    #[inline]
    fn shown(&self, flags: u32) -> u32 {
        if self.shadow_only { flags & !1 } else { flags }
    }

    fn mesh(&self) -> u32 {
        self.model.mesh.id
    }

    /// What its instance is made from.
    #[inline]
    fn kept(&self) -> [f32; KEPT] {
        let t = self.transform;
        let ([qx, qy, qz, qw], [px, py, pz], [sx, sy, sz]) =
            (t.rotation.to_array(), t.position.to_array(), t.scale.to_array());
        let c = self.model.color;
        [
            qx,
            qy,
            qz,
            qw,
            px,
            py,
            pz,
            sx,
            sy,
            sz,
            c.r,
            c.g,
            c.b,
            c.a,
            self.receives,
        ]
    }

    /// A digest of every field, never zero. Two sources that differ have
    /// the same one about once in 2^63 times; the model then stays as it
    /// was drawn until it changes again. Each pair of 64-bit words is mixed
    /// on its own (a 128-bit product folded in half), so the pairs don't
    /// wait for each other.
    #[inline]
    fn digest(&self) -> u64 {
        const KEYS: [u64; 10] = [
            0x9E37_79B9_7F4A_7C15,
            0xBF58_476D_1CE4_E5B9,
            0x94D0_49BB_1331_11EB,
            0x2545_F491_4F6C_DD1D,
            0xD6E8_FEB8_6659_FD93,
            0xA076_1D64_78BD_642F,
            0xE703_7ED1_A0B4_28DB,
            0x8EBC_6AF0_9C88_C6E3,
            0x5899_65CC_7537_4CC3,
            0x1D8E_4E27_C47D_124F,
        ];
        let join = |a: u32, b: u32| u64::from(a) | u64::from(b) << 32;
        let pair = |a: f32, b: f32| join(a.to_bits(), b.to_bits());
        let [qx, qy, qz, qw, px, py, pz, sx, sy, sz, r, g, b, a, receives] = self.kept();
        let words = [
            pair(qx, qy),
            pair(qz, qw),
            pair(px, py),
            pair(pz, sx),
            pair(sy, sz),
            pair(r, g),
            pair(b, a),
            join(receives.to_bits(), self.mesh()),
            join(self.shadow, self.material),
            // The part list by its content (most models have none).
            u64::from(self.skeleton)
                | u64::from(self.shadow_only) << 1
                | self.parts.map_or(0, |p| super::collect::Overrides::hash(p.raw()) << 2),
        ];
        let mut h = 0u64;
        for pair in 0..KEYS.len() / 2 {
            let (a, b) = (
                words[2 * pair] ^ KEYS[2 * pair],
                words[2 * pair + 1] ^ KEYS[2 * pair + 1],
            );
            let product = u128::from(a) * u128::from(b);
            h = h.wrapping_add(product as u64 ^ (product >> 64) as u64);
        }
        h | 1
    }

    /// The model as the GPU reads it: what [`instance_of`] makes of
    /// [`kept`](Self::kept).
    #[inline]
    fn instance(&self) -> Instance3D {
        let mut model = self.transform.matrix().to_cols_array_2d();
        model[0][3] = self.receives;
        Instance3D {
            model,
            color: linear(self.model.color),
        }
    }
}

/// The model as the GPU reads it, from what was kept to make its instance
/// (see [`Source::kept`]). The matrix's fourth row is always (0, 0, 0, 1): its first
/// slot holds the decal layers the model takes.
fn instance_of(kept: [f32; KEPT]) -> Instance3D {
    let [qx, qy, qz, qw, px, py, pz, sx, sy, sz, r, g, b, a, receives] = kept;
    let transform = Transform3D {
        rotation: Quat::from_xyzw(qx, qy, qz, qw),
        position: Vec3::new(px, py, pz),
        scale: Vec3::new(sx, sy, sz),
    };
    let mut model = transform.matrix().to_cols_array_2d();
    model[0][3] = receives;
    Instance3D {
        model,
        color: linear(crate::color::Color { r, g, b, a }),
    }
}

/// What culling needs of a model, worked out from its [`Source`] and the
/// meshes: its bounding spheres in the world (`xyz` center, `w` radius).
#[derive(Clone, Copy)]
struct Derived {
    /// Its own mesh's; negative radius: nothing to draw (a freed mesh, or a
    /// model that a skeleton or morph targets pose, drawn with the skins).
    sphere: [f32; 4],
    /// Its shadow caster's; negative radius: it casts none.
    caster_sphere: [f32; 4],
    /// The mesh id the camera draws (a material override's id, with one).
    mesh: u32,
    /// The mesh id the shadow views draw.
    caster: u32,
}

const NOTHING: Derived = Derived {
    sphere: [0.0, 0.0, 0.0, -1.0],
    caster_sphere: [0.0, 0.0, 0.0, -1.0],
    mesh: 0,
    caster: 0,
};

/// What of a mesh the models' [`Derived`] is worked out from: its bounds,
/// and whether it casts no shadow, is skinned, has morph targets.
type Fact = (Vec3, f32, bool, bool, bool, u32);

/// The material overrides' numbering: the frame's table of them (each
/// mesh with its overriding materials once, in order, which also tells
/// when the numbering changes), and the mesh id of the first.
pub(crate) struct OverrideIds<'a> {
    pub table: &'a super::collect::Overrides,
    pub base: u32,
}

#[inline]
fn derive(source: Source<'_>, assets: &Assets3D, overrides: &OverrideIds<'_>) -> Derived {
    let mesh = source.mesh();
    // Skinned models posed by a skeleton are drawn from the skins instead.
    // So are meshes with morph targets that aren't skinned.
    let posed = match assets.skin(mesh) {
        Some(_) => source.skeleton,
        None => assets.morph(mesh).is_some(),
    };
    let Some(bounds) = assets.live_bounds(mesh).filter(|_| !posed) else {
        return NOTHING;
    };
    let caster = match source.shadow {
        OWN_SHADOW => Some(mesh),
        NO_SHADOW => None,
        other => Some(other),
    }
    .filter(|&c| assets.casts_shadows(c));
    let caster_bounds = caster.and_then(|c| assets.live_bounds(c));
    // Checked: from here on the meshes go by slot, as the renderer and the
    // override table know them.
    let mesh = mesh & SLOT_MASK;
    let caster = caster.map(|c| c & SLOT_MASK);
    let t = source.transform;
    let widest = t.scale.abs().max_element();
    let matrix = t.matrix();
    let world = |(center, radius): (Vec3, f32)| matrix.transform_point3(center).extend(radius * widest).to_array();
    // The model's own mesh, wherever it is drawn (not a separate
    // `ShadowMesh`), takes the override.
    let own = if source.material == NONE && source.parts.is_none() {
        mesh
    } else {
        (overrides.table.find_ids(mesh, source.material, source.parts)).map_or(mesh, |k| overrides.base + k)
    };
    let sphere = world(bounds);
    Derived {
        sphere,
        caster_sphere: match caster_bounds {
            Some(_) if caster == Some(mesh) => sphere,
            Some(bounds) => world(bounds),
            None => NOTHING.caster_sphere,
        },
        mesh: own,
        caster: match caster.filter(|_| caster_bounds.is_some()) {
            Some(c) if c != mesh => c,
            _ => own,
        },
    }
}

/// What tells whether a model changed, and where it is drawn. Like the rest
/// of what is kept per model it is atomic, so the tasks of the parallel
/// compare can write the models they find changed in place (plain loads and
/// stores: every model is found by one task). One core has the whole
/// collection to itself and reads and writes it as plain memory.
struct Hot {
    /// Its [`Source::digest`]; 0 (no source's) until it is worked out.
    digest: AtomicU64,
    /// The views that draw it in the lists.
    applied: AtomicU32,
    /// Its id in the lists.
    id: u32,
}

/// What is kept of a model to draw it in other views when only they change.
struct Cold {
    /// See [`Derived`].
    mesh: AtomicU32,
    caster: AtomicU32,
    /// The views that see it, as the parallel compare last worked out.
    pending: AtomicU32,
}

fn zeroes<const N: usize>() -> [AtomicU32; N] {
    std::array::from_fn(|_| AtomicU32::new(0))
}

fn load<const N: usize>(words: &[AtomicU32; N]) -> [f32; N] {
    std::array::from_fn(|i| f32::from_bits(words[i].load(Ordering::Relaxed)))
}

/// In `Tracked::results`: nothing to do for the model.
const UNCHANGED: u32 = u32::MAX;
/// In `Tracked::results`: the model isn't tracked yet.
const NEW: u32 = u32::MAX - 1;
/// In `Tracked::results`: the model changed, and no view drew or draws it.
const DIFFERED: u32 = u32::MAX - 2;
/// Set in a result that names an object: its model changed since it was
/// last compared.
const DIFF: u32 = 1 << 31;
/// Set in a result that names an object: the lists are up to date with it
/// (a rush has to know that it is still drawn).
const SAME: u32 = 1 << 30;
const DEAD: u64 = u64::MAX;
/// More new models than this in a frame are worked out on all cores.
const MANY_NEW: usize = 512;
/// While most models change every frame (see `Tracked::rush`), one model
/// in this many is still compared with its digest, to tell when they stop.
const SAMPLE: u32 = 64;
/// Fewer tracked models than this never rush: there is nothing to gain.
const RUSH_MIN: usize = 4096;
/// A rush ends when fewer models than this are sampled: too few to tell.
const SAMPLE_MIN: usize = 16;

/// Whether a rush still compares `entity`'s model: one entity in
/// [`SAMPLE`], picked by a hash of its index (games spawn entities in
/// patterns, which a plain "every sixteenth" could miss altogether).
#[inline]
fn sampled(entity: ponko_ecs::Entity) -> bool {
    entity.index().wrapping_mul(0x9E37_79B1) >> 16 & (SAMPLE - 1) == 0
}

/// What a frame on one core works every model out with.
struct Serial<'a> {
    assets: &'a Assets3D,
    culler: &'a Culler<'a>,
    overrides: &'a OverrideIds<'a>,
    how: How<'a>,
    /// Whether `how` reworks any model whose source didn't change.
    reworks: bool,
    /// Whether moved models are listed for motion vectors.
    tracking: bool,
}

/// Models a frame compared with a digest they had, and those that differed.
#[derive(Default)]
struct Tally {
    compared: usize,
    differed: usize,
}

/// What makes a frame work models out again besides their own changes.
#[derive(Clone, Copy)]
struct How<'a> {
    /// Work every model out again.
    force: bool,
    /// By mesh id, when some meshes' facts changed: work their models out
    /// again.
    stale: Option<&'a [bool]>,
    /// Work the models with a material override out again.
    renumbered: bool,
    /// Cull every model again.
    cull: bool,
}

impl How<'_> {
    /// Whether a model whose source didn't change is worked out again.
    fn reworks(&self, source: Source<'_>) -> bool {
        let stale = |mesh: u32| {
            self.stale
                .is_some_and(|s| s.get((mesh & SLOT_MASK) as usize).copied().unwrap_or(false))
        };
        self.force
            || stale(source.mesh())
            || stale(source.shadow)
            || (self.renumbered && (source.material != NONE || source.parts.is_some()))
    }
}

/// Every plain [`Model`] in the world: see the module docs.
#[derive(Default)]
pub(crate) struct Tracked {
    /// By entity index: the entity's generation (high half) and object, or
    /// [`DEAD`].
    slot_of: Vec<u64>,
    /// By object, like everything below: the entity's bits, or [`DEAD`].
    entities: Vec<u64>,
    hot: Vec<Hot>,
    /// Its [`Derived`] spheres: own, then caster's.
    spheres: Vec<[AtomicU32; 8]>,
    cold: Vec<Cold>,
    /// Whether its entity is still among the models: worked out only in a
    /// frame that finds fewer models than are tracked (see `sweep`).
    seen: Vec<bool>,
    /// On several cores only (empty on one): what its instance is made
    /// from, written by the compare when the lists need the instance
    /// (stale otherwise).
    kept: Vec<[AtomicU32; KEPT]>,
    free: Vec<u32>,
    live: usize,
    /// On several cores: the objects a frame added, with their entities,
    /// when they are few.
    fresh: Vec<(u32, ponko_ecs::Entity)>,
    /// On several cores: per model, in query order, [`UNCHANGED`], [`NEW`],
    /// or the object whose lists need bringing up to date.
    results: Vec<u32>,
    /// What the models were last worked out and culled with.
    facts: Vec<Fact>,
    /// By mesh id: whether its facts changed this frame (see `sync`).
    stale: Vec<bool>,
    /// The overrides' numbering the models were last worked out with: the
    /// first id, the table's entries and their part lists.
    overrides: (u32, Vec<super::collect::MaterialOverride>, Vec<(u32, u32)>),
    view: Vec<f32>,
    /// While more than half the models change every frame: every model is
    /// worked out each frame as if nothing was kept (the digest and what
    /// is kept would cost more than they save), except one in [`SAMPLE`],
    /// which says when the models have settled. What is kept of the others
    /// is stale meanwhile, but for the views that draw them.
    rush: bool,
    /// In a rush: the objects the last frame found drawn, and (free
    /// between frames) those this frame did.
    drawn: Vec<u32>,
    fresh_drawn: Vec<u32>,
    /// In a rush, by object: whether this frame found it drawn.
    marked: Vec<bool>,
    /// Work every model out again in the next frame: what is kept is stale.
    renew: bool,
    /// On several cores: the models a frame found that aren't tracked, if
    /// they are no more than [`MANY_NEW`], and how many it found.
    news: Mutex<Vec<ponko_ecs::Entity>>,
    new_count: AtomicUsize,
    /// On several cores, in a rush: how many models were compared.
    sampled: AtomicUsize,
}

#[cfg(test)]
thread_local! {
    /// One pass on this thread (`Some(true)`) or the compare on all cores
    /// (`Some(false)`), whatever the thread pool has, for the collections
    /// this thread runs.
    pub(crate) static SERIAL: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// How many words an instance is.
const WORDS: usize = size_of::<Instance3D>() / size_of::<u32>();

/// The instance array as words that the tasks of the parallel compare can
/// write through a shared borrow: each writes only the slots of the models
/// it finds, and every model is found by one task.
fn shared(instances: &mut [Instance3D]) -> &[AtomicU32] {
    let words: &mut [u32] = bytemuck::cast_slice_mut(instances);
    // SAFETY: `AtomicU32` has the size, alignment and bit validity of
    // `u32`, and the words are borrowed exclusively for as long as the
    // result lives, so they are read and written through it alone.
    unsafe { &*(std::ptr::from_mut(words) as *const [AtomicU32]) }
}

/// What the parallel compare reads and (through atomics) writes.
struct Pass<'a> {
    slot_of: &'a [u64],
    hot: &'a [Hot],
    spheres: &'a [[AtomicU32; 8]],
    cold: &'a [Cold],
    kept: &'a [[AtomicU32; KEPT]],
    assets: &'a Assets3D,
    culler: &'a Culler<'a>,
    overrides: &'a OverrideIds<'a>,
    how: How<'a>,
    /// See `Tracked::rush`.
    rush: bool,
    news: &'a Mutex<Vec<ponko_ecs::Entity>>,
    new_count: &'a AtomicUsize,
    sampled: &'a AtomicUsize,
    lists: &'a Lists,
    /// The instance array (see [`shared`]); empty to stage every instance.
    arena: &'a [AtomicU32],
}

impl Pass<'_> {
    /// Writes the instance of object `k`'s model to its slots, if it is
    /// drawn in the views of `flags` with these meshes already (as most
    /// models that changed are): the lists are then up to date with it.
    /// Otherwise they take it in on one thread, in order.
    fn write_in_place(&self, k: usize, source: Source<'_>, flags: u32, mesh: u32, caster: u32) -> bool {
        if self.arena.is_empty() {
            return false;
        }
        let mut meshes = [caster; 1 + MAX_CASCADES];
        meshes[0] = mesh;
        let instance = source.instance();
        let Some(slots) = self.lists.held(self.hot[k].id, &meshes, flags, &instance) else {
            return false;
        };
        let words: &[u32] = bytemuck::cast_slice(std::slice::from_ref(&instance));
        for slot in slots {
            for (to, &word) in self.arena[slot * WORDS..][..WORDS].iter().zip(words) {
                to.store(word, Ordering::Relaxed);
            }
        }
        true
    }

    /// One model: compared with its digest; worked out again if it
    /// differs, and culled again if it or the views changed. In a rush,
    /// only one model in [`SAMPLE`] is; the others are worked out and
    /// culled from their components alone. Returns what goes into
    /// `Tracked::results`.
    fn check(&self, (entity, t, m, shadow, skeleton, receives, material, parts, shadow_only): ModelItem<'_>) -> u32 {
        let source = Source::of(t, m, shadow, skeleton, receives, material, parts, shadow_only);
        let find = || {
            let bits = entity.to_bits();
            match self.slot_of.get(entity.index() as usize) {
                Some(&slot) if slot >> 32 == bits >> 32 && (slot as u32 as usize) < self.hot.len() => {
                    Some(slot as u32 as usize)
                }
                _ => None,
            }
        };
        let new = || {
            // A few new models are listed, to save finding them again.
            if self.new_count.fetch_add(1, Ordering::Relaxed) < MANY_NEW
                && let Ok(mut news) = self.news.lock()
            {
                news.push(entity);
            }
            NEW
        };
        let keep = |k: usize| {
            for (kept, value) in self.kept[k].iter().zip(source.kept()) {
                kept.store(value.to_bits(), Ordering::Relaxed);
            }
        };
        if self.rush && !sampled(entity) {
            let derived = derive(source, self.assets, self.overrides);
            let flags = source.shown(self.culler.classify_spheres(derived.sphere, derived.caster_sphere));
            if flags == 0 {
                return UNCHANGED;
            }
            let Some(k) = find() else { return new() };
            if self.write_in_place(k, source, flags, derived.mesh, derived.caster) {
                return k as u32 | SAME;
            }
            let cold = &self.cold[k];
            cold.mesh.store(derived.mesh, Ordering::Relaxed);
            cold.caster.store(derived.caster, Ordering::Relaxed);
            cold.pending.store(flags, Ordering::Relaxed);
            keep(k);
            return k as u32;
        }
        let Some(k) = find() else { return new() };
        if self.rush {
            self.sampled.fetch_add(1, Ordering::Relaxed);
        }
        let hot = &self.hot[k];
        let digest = source.digest();
        let (cold, was) = (&self.cold[k], hot.applied.load(Ordering::Relaxed));
        let keep = || keep(k);
        // With ECS change ticks, this compare is what a tick test replaces.
        let before = hot.digest.load(Ordering::Relaxed);
        let diff = if before != digest && before != 0 { DIFF } else { 0 };
        if before != digest || self.how.reworks(source) {
            let derived = derive(source, self.assets, self.overrides);
            let spheres = derived.sphere.into_iter().chain(derived.caster_sphere);
            for (kept, value) in self.spheres[k].iter().zip(spheres) {
                kept.store(value.to_bits(), Ordering::Relaxed);
            }
            cold.mesh.store(derived.mesh, Ordering::Relaxed);
            cold.caster.store(derived.caster, Ordering::Relaxed);
            hot.digest.store(digest, Ordering::Relaxed);
            let flags = source.shown(self.culler.classify_spheres(derived.sphere, derived.caster_sphere));
            cold.pending.store(flags, Ordering::Relaxed);
            if flags != 0 {
                if self.write_in_place(k, source, flags, derived.mesh, derived.caster) {
                    return k as u32 | diff | SAME;
                }
                keep();
            }
            if flags != 0 || was != 0 {
                return k as u32 | diff;
            }
            if diff != 0 {
                return DIFFERED;
            }
        } else if self.how.cull {
            let [x, y, z, r, cx, cy, cz, cr] = load(&self.spheres[k]);
            let flags = source.shown(self.culler.classify_spheres([x, y, z, r], [cx, cy, cz, cr]));
            if flags != was {
                cold.pending.store(flags, Ordering::Relaxed);
                if flags != 0 {
                    keep();
                }
                return k as u32;
            }
        }
        if self.rush && was != 0 {
            return k as u32 | SAME;
        }
        UNCHANGED
    }
}

impl Tracked {
    /// Brings every model up to date with the world, the meshes and the
    /// views, and the lists with them.
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        world: &mut World,
        assets: &Assets3D,
        culler: &Culler<'_>,
        overrides: &OverrideIds<'_>,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
    ) {
        // Models are worked out again when what they are worked out from
        // changed: the facts of their meshes, or (for models with a
        // material override) the overrides' numbering.
        let fact = |id: usize| -> Fact {
            let (center, radius) = assets.bounds[id];
            let shadowless = !assets.casts_shadows(id as u32);
            let (skin, morph) = (assets.skin(id as u32).is_some(), assets.morph(id as u32).is_some());
            // The id the slot is given out as: a slot freed and reused within
            // one frame, with the same bounds, must still drop the models
            // holding the old id.
            let handle = assets.mesh_in_slot(id as u32).id;
            (center, radius, shadowless, skin, morph, handle)
        };
        let meshes = assets.bounds.len();
        let same = self.facts.len() == meshes && (0..meshes).all(|id| self.facts[id] == fact(id));
        if !same {
            self.stale.clear();
            self.stale.resize(meshes.max(self.facts.len()), true);
            for id in 0..meshes.min(self.facts.len()) {
                self.stale[id] = self.facts[id] != fact(id);
            }
            self.facts.clear();
            self.facts.extend((0..meshes).map(fact));
        }
        let table = overrides.table;
        let renumbered =
            self.overrides.0 != overrides.base || self.overrides.1 != table.list || self.overrides.2 != table.parts;
        if renumbered {
            self.overrides.0 = overrides.base;
            self.overrides.1.clear();
            self.overrides.1.extend_from_slice(&table.list);
            self.overrides.2.clear();
            self.overrides.2.extend_from_slice(&table.parts);
        }
        // Everything is culled again when the views changed.
        let cull = culler.changed_since(&mut self.view);
        let stale = std::mem::take(&mut self.stale);
        let how = How {
            force: std::mem::take(&mut self.renew),
            stale: (!same).then_some(&stale[..]),
            renumbered,
            cull,
        };
        // With one thread (a browser, a single core) splitting the work up
        // only costs: one pass does everything.
        let serial = rayon::current_num_threads() == 1;
        #[cfg(test)]
        let serial = SERIAL.get().unwrap_or(serial);
        let (found, new) = if serial {
            self.serial(world, assets, culler, overrides, how, lists, arena, motion)
        } else {
            self.parallel(world, assets, culler, overrides, how, lists, arena, motion)
        };
        self.stale = stale;
        if found != self.live {
            self.sweep(world, lists, arena);
        }
        self.live = found + new;
    }

    /// One thread: every model is compared, worked out, culled and its
    /// instances written in one pass, with nothing staged in between.
    /// Returns how many tracked models it found, and how many new ones.
    #[allow(clippy::too_many_arguments)]
    fn serial(
        &mut self,
        world: &mut World,
        assets: &Assets3D,
        culler: &Culler<'_>,
        overrides: &OverrideIds<'_>,
        how: How<'_>,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
    ) -> (usize, usize) {
        let count = world.query::<ModelItem<'_>>().filter::<Without<SkinOf>>().count();
        let rush = self.rush && !how.force;
        self.rush = rush;
        if !rush {
            if self.live == 0 {
                // A world loaded at once takes no more room than it needs.
                self.reserve(count, lists, false);
            }
            if count > self.live + MANY_NEW {
                self.live += self.admit_all(world, lists);
            }
        }
        let frame = Serial {
            assets,
            culler,
            overrides,
            how,
            reworks: how.force || how.stale.is_some() || how.renumbered,
            tracking: motion.tracking(),
        };
        let (mut found, mut new) = (0, 0);
        let mut tally = Tally::default();
        let mut models = world.query::<ModelItem<'_>>().filter::<Without<SkinOf>>();
        if !rush {
            models.for_each(
                |(entity, t, m, shadow, skeleton, receives, material, parts, shadow_only)| {
                    let k = match self.find(entity) {
                        Some(k) => {
                            found += 1;
                            k
                        }
                        None => {
                            new += 1;
                            self.admit(entity, lists)
                        }
                    };
                    let source = Source::of(t, m, shadow, skeleton, receives, material, parts, shadow_only);
                    self.settle(k, source, &frame, &mut tally, lists, arena, motion);
                },
            );
            if !how.force {
                self.rush_if_most_changed(&tally);
            }
            return (found, new);
        }
        // A rush: every model is worked out and culled from its components
        // alone, as if nothing was kept, and only the ones a view draws are
        // looked up (to write their instances where they were). One model
        // in `SAMPLE` is still compared, to tell when the models settle.
        let mut now = std::mem::take(&mut self.fresh_drawn);
        now.clear();
        models.for_each(
            |(entity, t, m, shadow, skeleton, receives, material, parts, shadow_only)| {
                let source = Source::of(t, m, shadow, skeleton, receives, material, parts, shadow_only);
                if sampled(entity) {
                    new += self.rush_sampled(entity, source, &frame, &mut tally, &mut now, lists, arena, motion);
                    return;
                }
                let derived = derive(source, assets, overrides);
                let flags = source.shown(culler.classify_spheres(derived.sphere, derived.caster_sphere));
                if flags != 0 {
                    let seen = (flags, derived.mesh, derived.caster);
                    new += self.rush_drawn(entity, source, seen, &frame, &mut now, lists, arena, motion);
                }
            },
        );
        self.end_rush_frame(now, &tally, lists, arena);
        // Which tracked models are gone is found when the rush ends.
        (self.live, new)
    }

    /// After a frame that compared every model: starts a rush, from the
    /// next frame on until the models settle, if most of them changed.
    fn rush_if_most_changed(&mut self, tally: &Tally) {
        if tally.compared < RUSH_MIN || tally.differed * 2 <= tally.compared {
            return;
        }
        self.rush = true;
        self.marked.clear();
        self.marked.resize(self.entities.len(), false);
        self.drawn.clear();
        for (k, hot) in self.hot.iter_mut().enumerate() {
            if *hot.applied.get_mut() != 0 {
                self.drawn.push(k as u32);
            }
        }
    }

    /// Ends a frame of a rush that found the objects in `now` drawn.
    fn end_rush_frame(&mut self, now: Vec<u32>, tally: &Tally, lists: &mut Lists, arena: &mut Vec<Instance3D>) {
        // The models drawn before that this frame didn't find drawn left
        // the views, or the world.
        for &k in &self.drawn {
            if !self.marked[k as usize] {
                let hot = &mut self.hot[k as usize];
                if std::mem::take(hot.applied.get_mut()) != 0 {
                    lists.clear(arena, hot.id);
                }
            }
        }
        for &k in &now {
            self.marked[k as usize] = false;
        }
        self.fresh_drawn = std::mem::replace(&mut self.drawn, now);
        // Settled (or too few are sampled to tell): the next frame works
        // everything out and keeps it.
        if tally.compared < SAMPLE_MIN || tally.differed * 4 <= tally.compared {
            self.rush = false;
            self.renew = true;
        }
    }

    /// In a rush: a sampled model, compared as in any other frame.
    /// Returns 1 if it wasn't tracked. (Out of line, like `rush_drawn`:
    /// most models need neither, and the loop over them stays small.)
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn rush_sampled(
        &mut self,
        entity: ponko_ecs::Entity,
        source: Source<'_>,
        frame: &Serial<'_>,
        tally: &mut Tally,
        now: &mut Vec<u32>,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
    ) -> usize {
        let found = self.find(entity);
        let k = found.unwrap_or_else(|| self.admit(entity, lists));
        self.settle(k, source, frame, tally, lists, arena, motion);
        if *self.hot[k].applied.get_mut() != 0 {
            self.mark(k, now);
        }
        usize::from(found.is_none())
    }

    /// In a rush: a model that some view draws (`seen`: the views, its
    /// mesh and its caster's). Returns 1 if it wasn't tracked.
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn rush_drawn(
        &mut self,
        entity: ponko_ecs::Entity,
        source: Source<'_>,
        (flags, mesh, caster): (u32, u32, u32),
        frame: &Serial<'_>,
        now: &mut Vec<u32>,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
    ) -> usize {
        let found = self.find(entity);
        let k = found.unwrap_or_else(|| self.admit(entity, lists));
        self.mark(k, now);
        self.draw(k, source, flags, mesh, caster, frame, lists, arena, motion);
        usize::from(found.is_none())
    }

    /// The object of `entity`, if it is tracked.
    #[inline]
    fn find(&self, entity: ponko_ecs::Entity) -> Option<usize> {
        let bits = entity.to_bits();
        match self.slot_of.get(entity.index() as usize) {
            Some(&slot) if slot >> 32 == bits >> 32 && (slot as u32 as usize) < self.hot.len() => {
                Some(slot as u32 as usize)
            }
            _ => None,
        }
    }

    /// In a rush: notes that this frame found object `k` drawn.
    #[inline]
    fn mark(&mut self, k: usize, now: &mut Vec<u32>) {
        if self.marked.len() <= k {
            self.marked.resize(k + 1, false);
        }
        if !std::mem::replace(&mut self.marked[k], true) {
            now.push(k as u32);
        }
    }

    /// One core: object `k` is compared with its digest, worked out again
    /// if it differs, culled again if it or the views changed, and its
    /// instances written if they changed.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn settle(
        &mut self,
        k: usize,
        source: Source<'_>,
        frame: &Serial<'_>,
        tally: &mut Tally,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
    ) {
        let hot = &mut self.hot[k];
        let (was, id) = (*hot.applied.get_mut(), hot.id);
        // With ECS change ticks, this compare is what a tick test replaces.
        let digest = source.digest();
        let before = std::mem::replace(hot.digest.get_mut(), digest);
        tally.compared += usize::from(before != 0);
        tally.differed += usize::from(before != 0 && before != digest);
        let (flags, mesh, caster);
        if before != digest || (frame.reworks && frame.how.reworks(source)) {
            let derived = derive(source, frame.assets, frame.overrides);
            let spheres = derived.sphere.into_iter().chain(derived.caster_sphere);
            for (kept, value) in self.spheres[k].iter_mut().zip(spheres) {
                *kept.get_mut() = value.to_bits();
            }
            let cold = &mut self.cold[k];
            *cold.mesh.get_mut() = derived.mesh;
            *cold.caster.get_mut() = derived.caster;
            flags = source.shown((frame.culler).classify_spheres(derived.sphere, derived.caster_sphere));
            (mesh, caster) = (derived.mesh, derived.caster);
        } else if frame.how.cull {
            let [x, y, z, r, cx, cy, cz, cr] = self.spheres[k].each_mut().map(|w| f32::from_bits(*w.get_mut()));
            flags = source.shown(frame.culler.classify_spheres([x, y, z, r], [cx, cy, cz, cr]));
            if flags == was {
                return;
            }
            let cold = &mut self.cold[k];
            (mesh, caster) = (*cold.mesh.get_mut(), *cold.caster.get_mut());
        } else {
            return;
        }
        if flags == 0 {
            if was != 0 {
                lists.clear(arena, id);
                *self.hot[k].applied.get_mut() = 0;
            }
            return;
        }
        self.draw(k, source, flags, mesh, caster, frame, lists, arena, motion);
    }

    /// One core: writes object `k`'s instance, made once, here, to its
    /// slots in the views of `flags` (not none).
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        k: usize,
        source: Source<'_>,
        flags: u32,
        mesh: u32,
        caster: u32,
        frame: &Serial<'_>,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
    ) {
        let hot = &mut self.hot[k];
        let (was, id) = (*hot.applied.get_mut(), hot.id);
        let instance = source.instance();
        if frame.tracking && was & flags & 1 != 0 {
            motion.moved_by(self.entities[k], mesh, &instance, arena[lists.slot(0, id)].model);
        }
        let mut meshes = [caster; 1 + MAX_CASCADES];
        meshes[0] = mesh;
        lists.set(arena, id, &meshes, flags, &instance);
        *self.hot[k].applied.get_mut() = flags;
    }

    /// One core, a frame with many more models than are tracked (a world
    /// or a level loaded at once): gives every model that isn't tracked an
    /// object, all at once. Returns how many.
    fn admit_all(&mut self, world: &mut World, lists: &mut Lists) -> usize {
        let before = self.entities.len() - self.free.len();
        world
            .query::<ponko_ecs::Entity>()
            .filter::<With<Transform3D>>()
            .filter::<With<Model>>()
            .filter::<Without<SkinOf>>()
            .for_each(|entity| {
                if self.find(entity).is_none() {
                    self.add(entity);
                }
            });
        self.grow(lists, false);
        self.entities.len() - self.free.len() - before
    }

    /// Several threads: each model is compared with what is kept of it on
    /// all cores, which stage what the lists need; then the lists are
    /// brought up to date on this one. Returns how many tracked models it
    /// found, and how many new ones.
    #[allow(clippy::too_many_arguments)]
    fn parallel(
        &mut self,
        world: &mut World,
        assets: &Assets3D,
        culler: &Culler<'_>,
        overrides: &OverrideIds<'_>,
        how: How<'_>,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
    ) -> (usize, usize) {
        let rush = self.rush && !how.force;
        self.rush = rush;
        self.grow(lists, true);
        let mut results = std::mem::take(&mut self.results);
        *self.new_count.get_mut() = 0;
        *self.sampled.get_mut() = 0;
        if let Ok(news) = self.news.get_mut() {
            news.clear();
        }
        // Where a moved model was is listed on this thread.
        let tracking = motion.tracking();
        type Out<'a> = (&'a mut Vec<u32>, &'a Lists, &'a mut Vec<Instance3D>);
        let pass = |tracked: &Self, how: How<'_>, rush: bool, (results, lists, arena): Out<'_>, world: &mut World| {
            let arena = if tracking { &[] } else { shared(arena) };
            let pass = tracked.pass(assets, culler, overrides, how, rush, lists, arena);
            world
                .query::<ModelItem<'_>>()
                .filter::<Without<SkinOf>>()
                .par_collect_into(results, |item| pass.check(item));
        };
        // In a rush: the objects this frame finds drawn.
        let mut now = std::mem::take(&mut self.fresh_drawn);
        now.clear();
        let (mut new, mut differed) = (0, 0);
        // With nothing tracked, every model is new.
        let nothing = self.live == 0;
        if nothing {
            results.clear();
            new = world.query::<ModelItem<'_>>().filter::<Without<SkinOf>>().count();
        } else {
            pass(self, how, rush, (&mut results, lists, arena), world);
            for &result in &results {
                match result {
                    UNCHANGED => {}
                    NEW => new += 1,
                    DIFFERED => differed += 1,
                    result => {
                        let k = (result & !(DIFF | SAME)) as usize;
                        differed += usize::from(result & DIFF != 0);
                        if result & SAME == 0 {
                            self.apply(k, lists, arena, motion);
                        }
                        if rush && *self.hot[k].applied.get_mut() != 0 {
                            self.mark(k, &mut now);
                        }
                    }
                }
            }
        }
        // A rush doesn't look for the tracked models that are gone.
        let found = match (nothing, rush) {
            (true, _) => 0,
            (false, true) => self.live,
            (false, false) => results.len() - new,
        };
        if new > 0 {
            // Each new model (spawned, or with the components now) is given
            // an object.
            self.reserve(new, lists, true);
            // Many are worked out on all cores, like the rest (in a rush
            // they can't be: what is kept of the rest is stale).
            let many = new > MANY_NEW && !rush;
            let mut fresh = std::mem::take(&mut self.fresh);
            fresh.clear();
            if !nothing && new <= MANY_NEW {
                // The compare listed them, in no order of its own.
                let mut news = self.news.get_mut().map(std::mem::take).unwrap_or_default();
                debug_assert_eq!(news.len(), new, "every new model is listed");
                news.sort_unstable_by_key(|entity| entity.to_bits());
                for &entity in &news {
                    fresh.push((self.add(entity) as u32, entity));
                }
                if let Ok(kept) = self.news.get_mut() {
                    *kept = news;
                }
            } else {
                // They are found again in order, by entity alone.
                let mut at = 0;
                world
                    .query::<ponko_ecs::Entity>()
                    .filter::<With<Transform3D>>()
                    .filter::<With<Model>>()
                    .filter::<Without<SkinOf>>()
                    .for_each(|entity| {
                        at += 1;
                        if nothing || results[at - 1] == NEW {
                            let k = self.add(entity) as u32;
                            if !many {
                                fresh.push((k, entity));
                            }
                        }
                    });
                debug_assert!(nothing || at == results.len(), "both queries find the same models");
            }
            self.grow(lists, true);
            // The rest was just worked out: only the new ones' digests
            // match nothing yet.
            let rest = How {
                force: false,
                stale: None,
                renumbered: false,
                cull: how.cull,
            };
            if many {
                pass(self, rest, false, (&mut results, lists, arena), world);
                for &result in &results {
                    if result < DIFFERED && result & SAME == 0 {
                        self.apply((result & !DIFF) as usize, lists, arena, motion);
                    }
                }
            } else {
                let mut models = world.query::<ModelItem<'_>>().filter::<Without<SkinOf>>();
                for &(k, entity) in &fresh {
                    let Some(item) = models.get(entity) else { continue };
                    if self
                        .pass(assets, culler, overrides, rest, false, lists, &[])
                        .check(item)
                        == k
                    {
                        self.apply(k as usize, lists, arena, motion);
                        if rush {
                            self.mark(k as usize, &mut now);
                        }
                    }
                }
            }
            self.fresh = fresh;
        }
        self.results = results;
        if rush {
            let tally = Tally {
                compared: *self.sampled.get_mut(),
                differed,
            };
            self.end_rush_frame(now, &tally, lists, arena);
        } else {
            self.fresh_drawn = now;
            if !how.force {
                // Every model found had been compared before.
                let compared = found;
                self.rush_if_most_changed(&Tally { compared, differed });
            }
        }
        (found, new)
    }

    #[allow(clippy::too_many_arguments)]
    fn pass<'a>(
        &'a self,
        assets: &'a Assets3D,
        culler: &'a Culler<'a>,
        overrides: &'a OverrideIds<'a>,
        how: How<'a>,
        rush: bool,
        lists: &'a Lists,
        arena: &'a [AtomicU32],
    ) -> Pass<'a> {
        Pass {
            slot_of: &self.slot_of,
            hot: &self.hot,
            spheres: &self.spheres,
            cold: &self.cold,
            kept: &self.kept,
            assets,
            culler,
            overrides,
            how,
            rush,
            news: &self.news,
            new_count: &self.new_count,
            sampled: &self.sampled,
            lists,
            arena,
        }
    }

    /// Room for `new` more objects. The buffers grow by what is asked for,
    /// or an eighth if that is more: a world loaded at once takes no more
    /// than it needs, and one that grows a model at a time isn't copied
    /// for each. `kept`: with what several cores stage instances in.
    fn reserve(&mut self, new: usize, lists: &mut Lists, kept: bool) {
        let more = new.saturating_sub(self.free.len());
        let len = self.entities.len();
        if self.entities.capacity() - len < more {
            let room = more.max(len / 8);
            self.entities.reserve_exact(room);
            self.hot.reserve_exact(room);
            self.spheres.reserve_exact(room);
            self.cold.reserve_exact(room);
            if kept {
                self.kept.reserve_exact(room);
            }
        }
        lists.reserve(more);
    }

    /// A new object for `entity`, with a digest that isn't its source's.
    /// [`grow`](Self::grow) follows, before the object is used.
    fn add(&mut self, entity: ponko_ecs::Entity) -> usize {
        let bits = entity.to_bits();
        let k = match self.free.pop() {
            Some(k) => {
                let k = k as usize;
                self.entities[k] = bits;
                let hot = &mut self.hot[k];
                *hot.digest.get_mut() = 0; // no source's
                *hot.applied.get_mut() = 0;
                *self.cold[k].pending.get_mut() = 0;
                k
            }
            None => {
                self.entities.push(bits);
                self.entities.len() - 1
            }
        };
        let index = entity.index() as usize;
        if self.slot_of.len() <= index {
            self.slot_of.resize(index + 1, DEAD);
        }
        self.slot_of[index] = bits & !0xFFFF_FFFF | k as u64;
        k
    }

    /// Gives the objects added since the last call the rest of what is
    /// kept of them, all at once. `kept`: with what several cores stage
    /// instances in.
    fn grow(&mut self, lists: &mut Lists, kept: bool) {
        let n = self.entities.len();
        while self.hot.len() < n {
            self.hot.push(Hot {
                digest: AtomicU64::new(0),
                applied: AtomicU32::new(0),
                id: lists.alloc(),
            });
        }
        self.spheres.resize_with(n, zeroes);
        self.cold.resize_with(n, || Cold {
            mesh: AtomicU32::new(0),
            caster: AtomicU32::new(0),
            pending: AtomicU32::new(0),
        });
        if kept {
            self.kept.resize_with(n, zeroes);
        }
    }

    /// One new object for `entity`, ready to use (one core adds the models
    /// it finds as it goes).
    #[cold]
    fn admit(&mut self, entity: ponko_ecs::Entity, lists: &mut Lists) -> usize {
        self.reserve(1, lists, false);
        let k = self.add(entity);
        self.grow(lists, false);
        k
    }

    /// Brings the lists up to date with object `k`, from what the parallel
    /// compare staged.
    fn apply(&mut self, k: usize, lists: &mut Lists, arena: &mut Vec<Instance3D>, motion: &mut Motion) {
        let cold = &self.cold[k];
        let flags = cold.pending.load(Ordering::Relaxed);
        let (was, id) = (*self.hot[k].applied.get_mut(), self.hot[k].id);
        if flags == 0 {
            if was != 0 {
                lists.clear(arena, id);
            }
        } else {
            let instance = instance_of(load(&self.kept[k]));
            let (mesh, caster) = (cold.mesh.load(Ordering::Relaxed), cold.caster.load(Ordering::Relaxed));
            if was & flags & 1 != 0 && motion.tracking() {
                let before = arena[lists.slot(0, id)].model;
                motion.moved_by(self.entities[k], mesh, &instance, before);
            }
            let mut meshes = [caster; 1 + MAX_CASCADES];
            meshes[0] = mesh;
            lists.set(arena, id, &meshes, flags, &instance);
        }
        *self.hot[k].applied.get_mut() = flags;
    }

    /// Some tracked models are gone from the world (despawned, or without
    /// the components now): this frame found fewer than are tracked. Finds
    /// which, by going through the models once more, and takes them out of
    /// the lists.
    fn sweep(&mut self, world: &mut World, lists: &mut Lists, arena: &mut Vec<Instance3D>) {
        let mut seen = std::mem::take(&mut self.seen);
        seen.clear();
        seen.resize(self.entities.len(), false);
        world
            .query::<ponko_ecs::Entity>()
            .filter::<With<Transform3D>>()
            .filter::<With<Model>>()
            .filter::<Without<SkinOf>>()
            .for_each(|entity| {
                let bits = entity.to_bits();
                if let Some(&slot) = self.slot_of.get(entity.index() as usize)
                    && slot >> 32 == bits >> 32
                    && let Some(seen) = seen.get_mut(slot as u32 as usize)
                {
                    *seen = true;
                }
            });
        for (k, _) in seen.iter().enumerate().filter(|&(_, &seen)| !seen) {
            if self.entities[k] == DEAD {
                continue;
            }
            let hot = &mut self.hot[k];
            *hot.applied.get_mut() = 0;
            lists.clear(arena, hot.id);
            let index = self.entities[k] as u32 as usize;
            if self.slot_of[index] == self.entities[k] & !0xFFFF_FFFF | k as u64 {
                self.slot_of[index] = DEAD;
            }
            self.entities[k] = DEAD;
            self.free.push(k as u32);
        }
        self.seen = seen;
    }

    /// Whether most models change every frame, so few are compared.
    #[cfg(test)]
    pub fn rushing(&self) -> bool {
        self.rush
    }

    /// How many tracked models with a mesh to cull have their bounding
    /// sphere (`[x, y, z, radius]` in the world, as last culled) picked by
    /// `pick`. Worked out when asked, from what is kept per model, so a
    /// frame pays nothing for it and a still camera gives the same answer
    /// every frame. Models whose mesh isn't loaded, or that skins or morph
    /// targets draw instead, have no sphere and are never picked. `None`
    /// while the kept spheres are stale: in a rush (see [`Tracked::rush`])
    /// and in the frame after one.
    pub fn count_spheres(&self, pick: impl Fn([f32; 4]) -> bool) -> Option<usize> {
        if self.rush || self.renew {
            return None;
        }
        let kept = (self.entities.iter().zip(&self.spheres)).filter(|&(&e, _)| e != DEAD);
        let picked = kept
            .map(|(_, sphere)| std::array::from_fn(|i| f32::from_bits(sphere[i].load(Ordering::Relaxed))))
            .filter(|s: &[f32; 4]| s[3] >= 0.0 && pick(*s))
            .count();
        Some(picked)
    }

    /// Bytes kept between frames.
    #[cfg(any(test, feature = "bench-internals"))]
    pub fn bytes(&self) -> usize {
        self.entities.capacity() * size_of::<u64>()
            + self.hot.capacity() * size_of::<Hot>()
            + self.spheres.capacity() * size_of::<[AtomicU32; 8]>()
            + self.cold.capacity() * size_of::<Cold>()
            + self.kept.capacity() * size_of::<[AtomicU32; KEPT]>()
            + self.seen.capacity()
            + self.marked.capacity()
            + (self.drawn.capacity() + self.fresh_drawn.capacity()) * size_of::<u32>()
            + self.slot_of.capacity() * size_of::<u64>()
            + (self.free.capacity() + self.results.capacity()) * size_of::<u32>()
            + self.facts.capacity() * size_of::<Fact>()
            + self.stale.capacity()
            + self.fresh.capacity() * size_of::<(u32, ponko_ecs::Entity)>()
            + self.news.lock().map_or(0, |news| news.capacity()) * size_of::<ponko_ecs::Entity>()
            + self.overrides.1.capacity() * size_of::<super::collect::MaterialOverride>()
            + self.overrides.2.capacity() * size_of::<(u32, u32)>()
            + self.view.capacity() * size_of::<f32>()
    }
}

/// A thing drawn this frame that isn't a tracked [`Model`]: the entity it
/// belongs to (its bits), and which of the entity's things it is.
pub(crate) type Key = (u64, u32);

/// In a [`Key`]: a shadow-only caster, a skinned model, a terrain tile, or
/// (added to the renderer's number within its group) a level-of-detail
/// renderer.
pub(crate) const SHADOW_ONLY: u32 = 1 << 28;
pub(crate) const SKINNED: u32 = 2 << 28;
pub(crate) const TERRAIN: u32 = 3 << 28;
pub(crate) const LOD: u32 = 4 << 28;
/// Added to a key that was already used this frame, to tell them apart.
const AGAIN: u32 = 1 << 20;

struct Transient {
    key: Key,
    /// Its id in the lists.
    id: u32,
    /// The frame that last drew it.
    stamp: u32,
    /// Where it was in the frame's order of things.
    seq: u32,
    /// As last drawn.
    instance: Instance3D,
}

/// The things worked out every frame, matched to the ids they had: see the
/// module docs. A frame calls [`touch`](Self::touch) for each thing some
/// view draws, then [`finish`](Self::finish), which drops the rest.
#[derive(Default)]
pub(crate) struct Transients {
    items: Vec<Transient>,
    free: Vec<u32>,
    by_key: HashMap<Key, u32, BuildHasherDefault<IdHasher>>,
    /// The items the last frame drew, in its order, and this frame's so far.
    order: Vec<u32>,
    next: Vec<u32>,
    /// Where in `order` the next thing is expected: frames mostly draw the
    /// same things in the same order, which costs no lookup.
    cursor: usize,
    frame: u32,
}

impl Transients {
    /// Starts a frame.
    pub fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1).max(1);
        self.cursor = 0;
    }

    /// Draws the thing `key` as `instance` in the views of `flags` (not
    /// none) with `meshes`. `motion_key` is [`NO_KEY`] for things without
    /// motion vectors of their own.
    #[allow(clippy::too_many_arguments)]
    pub fn touch(
        &mut self,
        lists: &mut Lists,
        arena: &mut Vec<Instance3D>,
        motion: &mut Motion,
        mut key: Key,
        motion_key: u64,
        meshes: &Meshes,
        flags: u32,
        instance: &Instance3D,
    ) {
        let frame = self.frame;
        let expected = self
            .order
            .get(self.cursor)
            .copied()
            .filter(|&t| self.items[t as usize].key == key && self.items[t as usize].stamp != frame);
        let t = if let Some(t) = expected {
            self.cursor += 1;
            t
        } else {
            let found = loop {
                match self.by_key.get(&key) {
                    // The same key twice in a frame: another thing.
                    Some(&t) if self.items[t as usize].stamp == frame => key.1 = key.1.wrapping_add(AGAIN),
                    found => break found.copied(),
                }
            };
            match found {
                Some(t) => {
                    // Things before it left or new ones came: carry on
                    // from where it was.
                    self.cursor = self.items[t as usize].seq as usize + 1;
                    t
                }
                None => {
                    let item = Transient {
                        key,
                        id: lists.alloc(),
                        stamp: 0,
                        seq: 0,
                        instance: Instance3D::default(),
                    };
                    let t = match self.free.pop() {
                        Some(t) => {
                            self.items[t as usize] = item;
                            t
                        }
                        None => {
                            self.items.push(item);
                            self.items.len() as u32 - 1
                        }
                    };
                    self.by_key.insert(key, t);
                    t
                }
            }
        };
        let item = &mut self.items[t as usize];
        item.stamp = frame;
        item.seq = self.next.len() as u32;
        self.next.push(t);
        if motion_key != NO_KEY && lists.flags(item.id) & flags & 1 != 0 {
            motion.moved_by(motion_key, meshes[0], instance, item.instance.model);
        }
        if !lists.holds(item.id, meshes, flags, instance)
            || bytemuck::bytes_of(&item.instance) != bytemuck::bytes_of(instance)
        {
            lists.set(arena, item.id, meshes, flags, instance);
            item.instance = *instance;
        }
    }

    /// Ends a frame: the things the last frame drew and this one didn't
    /// leave the lists.
    pub fn finish(&mut self, lists: &mut Lists, arena: &mut Vec<Instance3D>) {
        for &t in &self.order {
            let item = &self.items[t as usize];
            if item.stamp != self.frame {
                lists.release(arena, item.id);
                self.by_key.remove(&item.key);
                self.free.push(t);
            }
        }
        std::mem::swap(&mut self.order, &mut self.next);
        self.next.clear();
    }

    /// Bytes kept between frames.
    #[cfg(any(test, feature = "bench-internals"))]
    pub fn bytes(&self) -> usize {
        self.items.capacity() * size_of::<Transient>()
            + (self.free.capacity() + self.order.capacity() + self.next.capacity()) * size_of::<u32>()
            + self.by_key.capacity() * (size_of::<Key>() + size_of::<u32>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> Instance3D {
        let mut instance = Instance3D::default();
        instance.model[3][0] = x;
        instance
    }

    /// The `x` of every instance `view` draws, by run.
    fn drawn(lists: &Lists, arena: &[Instance3D], view: usize) -> Vec<(u32, Vec<f32>)> {
        let mut runs = Vec::new();
        lists.runs(view, &mut runs);
        runs.iter()
            .map(|&(mesh, start, count)| {
                let xs = arena[start as usize..(start + count) as usize]
                    .iter()
                    .map(|i| i.model[3][0])
                    .collect();
                (mesh, xs)
            })
            .collect()
    }

    #[test]
    fn a_frame_built_from_nothing_is_packed_by_view_then_mesh() {
        let (mut lists, mut arena) = (Lists::default(), Vec::new());
        lists.begin(&mut arena);
        for (x, mesh, flags) in [(0.0, 2, 0b11), (1.0, 0, 0b01), (2.0, 2, 0b10), (3.0, 0, 0b11)] {
            let id = lists.alloc();
            lists.set(&mut arena, id, &[mesh; 5], flags, &at(x));
        }
        lists.finish(&mut arena);
        let xs: Vec<f32> = arena.iter().map(|i| i.model[3][0]).collect();
        assert_eq!(
            xs,
            [1.0, 3.0, 0.0, 3.0, 0.0, 2.0],
            "the camera's by mesh, then the shadow view's"
        );
        let mut runs = Vec::new();
        lists.runs(0, &mut runs);
        assert_eq!(runs, [(0, 0, 2), (2, 2, 1)]);
        lists.runs(1, &mut runs);
        assert_eq!(runs, [(0, 3, 1), (2, 4, 2)]);
        assert_eq!((lists.count(0), lists.count(1)), (3, 3));
    }

    #[test]
    fn a_thing_keeps_its_slot_while_others_enter_and_leave() {
        let (mut lists, mut arena) = (Lists::default(), Vec::new());
        lists.begin(&mut arena);
        let ids: Vec<u32> = (0..6).map(|_| lists.alloc()).collect();
        for (i, &id) in ids.iter().enumerate() {
            lists.set(&mut arena, id, &[7; 5], 1, &at(i as f32));
        }
        lists.finish(&mut arena);
        let slot = lists.slot(0, ids[1]);
        lists.begin(&mut arena);
        // One leaves: the last takes its place, the rest stay put.
        lists.clear(&mut arena, ids[3]);
        assert_eq!(drawn(&lists, &arena, 0), [(7, vec![0.0, 1.0, 2.0, 5.0, 4.0])]);
        // New ones enter (the block outgrows its room), and the thing
        // moves: its slot's instance is rewritten.
        for x in [10.0, 11.0, 12.0] {
            let id = lists.alloc();
            lists.set(&mut arena, id, &[7; 5], 1, &at(x));
        }
        lists.set(&mut arena, ids[1], &[7; 5], 1, &at(-1.0));
        lists.finish(&mut arena);
        assert_eq!(
            drawn(&lists, &arena, 0),
            [(7, vec![0.0, -1.0, 2.0, 5.0, 4.0, 10.0, 11.0, 12.0])]
        );
        assert_eq!(arena[lists.slot(0, ids[1])].model[3][0], -1.0);
        assert_eq!(lists.slot(0, ids[1]), slot);
        assert_eq!(lists.count(0), 8);
    }

    #[test]
    fn a_new_mesh_or_fade_moves_a_thing_between_runs() {
        let (mut lists, mut arena) = (Lists::default(), Vec::new());
        lists.begin(&mut arena);
        let (a, b) = (lists.alloc(), lists.alloc());
        lists.set(&mut arena, a, &[1; 5], 0b11, &at(1.0));
        lists.set(&mut arena, b, &[1; 5], 0b11, &at(2.0));
        lists.finish(&mut arena);
        lists.begin(&mut arena);
        // Another mesh for the camera only: the shadow view keeps its run.
        lists.set(&mut arena, a, &[3, 1, 1, 1, 1], 0b11, &at(1.0));
        assert_eq!(drawn(&lists, &arena, 0), [(1, vec![2.0]), (3, vec![1.0])]);
        assert_eq!(drawn(&lists, &arena, 1), [(1, vec![1.0, 2.0])]);
        // Crossfading: runs of its own, after the whole ones.
        lists.set(&mut arena, b, &[1; 5], 0b11, &at(2.0).with_fade(0.5));
        assert_eq!(drawn(&lists, &arena, 0), [(3, vec![1.0]), (1 | FADED, vec![2.0])]);
        assert_eq!(drawn(&lists, &arena, 1), [(1, vec![1.0]), (1 | FADED, vec![2.0])]);
        // Released ids are drawn nowhere and handed out again.
        lists.release(&mut arena, a);
        assert_eq!(drawn(&lists, &arena, 0), [(1 | FADED, vec![2.0])]);
        assert_eq!(lists.alloc(), a);
        assert_eq!(lists.flags(a), 0);
    }

    #[test]
    fn a_block_that_outgrows_its_place_moves_and_its_things_follow() {
        let (mut lists, mut arena) = (Lists::default(), Vec::new());
        lists.begin(&mut arena);
        let (a, b) = (lists.alloc(), lists.alloc());
        lists.set(&mut arena, a, &[0; 5], 1, &at(1.0));
        lists.set(&mut arena, b, &[1; 5], 1, &at(2.0));
        lists.finish(&mut arena);
        assert_eq!((lists.slot(0, a), lists.slot(0, b), arena.len()), (0, 1, 2));
        // Mesh 0's block has mesh 1's behind it: it moves to the end, with
        // room for two.
        lists.begin(&mut arena);
        let c = lists.alloc();
        lists.set(&mut arena, c, &[0; 5], 1, &at(3.0));
        lists.finish(&mut arena);
        assert_eq!((lists.slot(0, a), lists.slot(0, c), lists.slot(0, b)), (2, 3, 1));
        assert_eq!(arena.len(), 4);
        assert_eq!(drawn(&lists, &arena, 0), [(0, vec![1.0, 3.0]), (1, vec![2.0])]);
        assert_eq!(lists.repacks, 1, "only the first frame was packed");
    }

    #[test]
    fn an_array_twice_what_it_needs_is_packed_again_with_room_to_grow() {
        let (mut lists, mut arena) = (Lists::default(), Vec::new());
        lists.begin(&mut arena);
        let ids: Vec<u32> = (0..3000).map(|_| lists.alloc()).collect();
        for (i, &id) in ids.iter().enumerate() {
            lists.set(&mut arena, id, &[(i % 3) as u32; 5], 1, &at(i as f32));
        }
        lists.finish(&mut arena);
        assert_eq!((arena.len(), lists.repacks), (3000, 1));
        // Most leave: the array is packed down to what stays, each block
        // with room for half as many again, in the order they had.
        lists.begin(&mut arena);
        for &id in &ids[60..] {
            lists.clear(&mut arena, id);
        }
        lists.finish(&mut arena);
        assert_eq!((arena.len(), lists.repacks), (3 * 31, 2));
        assert_eq!(lists.count(0), 60);
        let all = drawn(&lists, &arena, 0);
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].1[..4], [1.0, 4.0, 7.0, 10.0]);
        // Still, nothing more is packed or moved; and half as many again
        // enter without a block moving.
        let before = arena.clone();
        lists.begin(&mut arena);
        lists.finish(&mut arena);
        assert_eq!(lists.repacks, 2);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&before),
            bytemuck::cast_slice::<_, u8>(&arena)
        );
        lists.begin(&mut arena);
        for (i, &id) in ids[60..90].iter().enumerate() {
            lists.set(&mut arena, id, &[(i % 3) as u32; 5], 1, &at(0.5));
        }
        lists.finish(&mut arena);
        assert_eq!((arena.len(), lists.count(0), lists.repacks), (3 * 31, 90, 2));
    }

    #[test]
    fn transients_keep_their_ids_when_the_frame_draws_them_in_another_order() {
        let (mut lists, mut arena, mut motion) = (Lists::default(), Vec::new(), Motion::default());
        let mut things = Transients::default();
        let mut frame = |things: &mut Transients, lists: &mut Lists, arena: &mut Vec<Instance3D>, keys: &[u64]| {
            lists.begin(arena);
            things.begin();
            for &key in keys {
                let (key, instance) = ((key, LOD), at(key as f32));
                things.touch(lists, arena, &mut motion, key, NO_KEY, &[0; 5], 1, &instance);
            }
            things.finish(lists, arena);
            lists.finish(arena);
            drawn(lists, arena, 0)
        };
        assert_eq!(
            frame(&mut things, &mut lists, &mut arena, &[1, 2, 3, 4]),
            [(0, vec![1.0, 2.0, 3.0, 4.0])]
        );
        // 2 is gone and 5 is new: the others keep their slots (5, the
        // last by then, takes 2's).
        assert_eq!(
            frame(&mut things, &mut lists, &mut arena, &[1, 3, 4, 5]),
            [(0, vec![1.0, 5.0, 3.0, 4.0])]
        );
        // Another order draws the same.
        assert_eq!(
            frame(&mut things, &mut lists, &mut arena, &[5, 4, 3, 1]),
            [(0, vec![1.0, 5.0, 3.0, 4.0])]
        );
        // The same key twice in a frame is two things.
        assert_eq!(
            frame(&mut things, &mut lists, &mut arena, &[7, 7]),
            [(0, vec![7.0, 7.0])]
        );
        assert_eq!(frame(&mut things, &mut lists, &mut arena, &[]), []);
        assert_eq!(things.by_key.len(), 0);
    }
}
