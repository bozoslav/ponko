//! One lifecycle for every GPU, audio and physics resource handle:
//! [`Handle`], `is_live`, `free` and `live_counts`, on the service that
//! holds the handle ([`Graphics`] for everything drawn,
//! [`Audio`](crate::audio::Audio) for sounds, `Physics3D` for collision
//! meshes) and on [`Ctx`] for any kind.
//!
//! A handle is a small `Copy` value naming something the engine holds for
//! the game: a texture, a mesh, a sound. Pass it by value. Only the engine
//! makes handles: there is no way to turn a number into one, so a handle
//! always names something this game made. Each kind answers the same three
//! questions the same way:
//!
//! - Is it still valid? [`Graphics::is_live`](crate::gfx::Graphics::is_live),
//!   [`Audio::is_live`](crate::audio::Audio::is_live), or [`Ctx::is_live`] for any kind.
//! - Release it: [`Graphics::free`](crate::gfx::Graphics::free),
//!   [`Audio::free`](crate::audio::Audio::free), or [`Ctx::free`]. A freed
//!   handle stays dead for the rest of the run, and freeing it again does
//!   nothing.
//! - An empty handle: [`Handle::NONE`], which is never live.
//!
//! No handle is ever given out twice. Kinds that streamed worlds make and
//! free all the time (meshes, GPU buffers and collision meshes) reuse a
//! freed one's table slot, so their tables don't grow, but under a new
//! handle: it counts how often its slot was given out (up to 4095 times;
//! then the slot is retired rather than counting from zero again). The
//! other kinds never reuse a slot.
//!
//! Freeing something others still use is defined: they draw with the
//! default. A model whose mesh was freed draws nothing (as with
//! [`Mesh::NONE`](crate::render::Mesh::NONE)); a material whose texture was freed
//! samples white, as if it had none; a mesh part whose material was freed
//! draws with the default material; text in a freed font uses the built-in
//! one. Nothing is reference counted: freeing is explicit, and handles
//! stay `Copy`.

use std::hash::Hash;

use crate::ctx::Ctx;
use crate::gfx::Graphics;

mod sealed {
    pub trait Sealed {}
}

/// A handle to a resource the engine holds for the game: [`Texture`](crate::texture::Texture),
/// [`Sound`](crate::audio::Sound), [`Font`](crate::text::Font),
/// [`SpriteShader`](crate::render::SpriteShader), [`SpriteMaterial`](crate::render::SpriteMaterial), and with the `3d` feature [`Mesh`](crate::render::Mesh),
/// [`Material`](crate::render::Material), [`Shader`](crate::render::Shader),
/// [`Compute`](crate::render::Compute), [`Effect`](crate::render::Effect),
/// [`GpuBuffer`](crate::render::GpuBuffer), [`InstanceBatch`](crate::render::InstanceBatch),
/// [`TextureArray`](crate::terrain::TextureArray) and [`TerrainTile`](crate::terrain::TerrainTile)
/// (with `physics3d`, [`CollisionMesh`](crate::d3::CollisionMesh) too).
/// Render targets are textures.
///
/// Check one with [`Ctx::is_live`] and release it with [`Ctx::free`].
/// Implemented by the engine's handle types only.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let pixel = ctx.gfx.create_texture(1, 1, &[255; 4]);
///     assert!(ctx.is_live(pixel));
///     ctx.free(pixel);
///     assert!(!ctx.is_live(pixel));
///     assert!(!ctx.is_live(Texture::NONE));
/// });
/// ```
///
/// Handles come only from the engine: a number can't be turned into one,
/// and a handle doesn't hand out its number.
///
/// ```compile_fail,E0599
/// # use ponko::prelude::*;
/// let forged = Texture::from_id(3);
/// ```
///
/// ```compile_fail,E0599
/// # use ponko::prelude::*;
/// fn number(t: Texture) -> u32 {
///     t.id()
/// }
/// ```
///
/// ```compile_fail,E0603
/// // The engine's own raw-id trait is private to it.
/// use ponko::handle::raw::RawHandle;
/// ```
pub trait Handle: Copy + Eq + Hash + std::fmt::Debug + sealed::Sealed {
    /// No resource: never live, and freeing it does nothing. Whatever shows
    /// it draws the default (nothing for a mesh, white for a texture, the
    /// built-in font for text).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     ctx.free(Texture::NONE); // does nothing
    ///     assert!(!ctx.is_live(Texture::NONE));
    /// });
    /// ```
    const NONE: Self;

    /// Whether `ctx` holds this resource (see [`Ctx::is_live`]).
    #[doc(hidden)]
    fn live_in(self, ctx: &Ctx) -> bool;

    /// Releases this resource in `ctx` (see [`Ctx::free`]).
    #[doc(hidden)]
    fn free_in(self, ctx: &mut Ctx);
}

/// A [`Handle`] the graphics service holds: every kind except
/// [`Sound`](crate::audio::Sound) and the collision meshes of 3D physics. Check one with [`Graphics::is_live`] and release
/// it with [`Graphics::free`]. Implemented by the engine's handle types only.
pub trait GfxHandle: Handle {
    /// Whether `gfx` holds this resource (see [`Graphics::is_live`]).
    #[doc(hidden)]
    fn live_in_gfx(self, gfx: &Graphics) -> bool;

    /// Releases this resource in `gfx` (see [`Graphics::free`]).
    #[doc(hidden)]
    fn free_in_gfx(self, gfx: &mut Graphics);
}

/// How many handles of each kind are live, from [`Ctx::live_counts`]: for
/// tests of streaming code and leak checks. Kinds the build doesn't have
/// (without the `3d` or `physics3d` feature) count 0.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let before = ctx.live_counts();
///     let t = ctx.gfx.create_texture(1, 1, &[255; 4]);
///     assert_eq!(ctx.live_counts().textures, before.textures + 1);
///     ctx.free(t);
///     assert_eq!(ctx.live_counts(), before);
/// });
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LiveCounts {
    /// Textures the game made or loaded, render targets included.
    pub textures: usize,
    /// Of `textures`, the render targets.
    pub render_targets: usize,
    /// Meshes (each detail level of a terrain tile is one).
    pub meshes: usize,
    /// Materials, the default one included once it was made.
    pub materials: usize,
    /// Material shaders (not the built-in one).
    pub shaders: usize,
    /// Compute passes.
    pub computes: usize,
    /// Screen effects.
    pub effects: usize,
    /// GPU buffers.
    pub gpu_buffers: usize,
    /// Instance batches.
    pub instance_batches: usize,
    /// Texture arrays.
    pub texture_arrays: usize,
    /// Sounds, streams included.
    pub sounds: usize,
    /// Fonts the game loaded (not the built-in one).
    pub fonts: usize,
    /// Collision meshes (feature `physics3d`).
    pub collision_meshes: usize,
    /// Sprite shaders.
    pub sprite_shaders: usize,
    /// Sprite materials.
    pub sprite_materials: usize,
}

impl Ctx {
    /// Whether `handle` names something this game made and hasn't freed.
    /// False for [`Handle::NONE`] and for handles from another game.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let font = Font::DEFAULT;
    ///     assert!(ctx.is_live(font));
    ///     assert!(!ctx.is_live(Font::NONE));
    /// });
    /// ```
    #[must_use]
    pub fn is_live<H: Handle>(&self, handle: H) -> bool {
        handle.live_in(self)
    }

    /// Releases what `handle` names: its GPU memory (or decoded audio, or
    /// collision shape), and the file cache entry if it was loaded, so
    /// loading the file again loads it afresh. No later handle is ever the
    /// same, so the handle stays dead; freeing it again, or freeing
    /// [`Handle::NONE`], does nothing.
    ///
    /// Whatever still uses it draws with the default: models showing a
    /// freed mesh draw nothing, materials sampling a freed texture see
    /// white, mesh parts with a freed material draw with the default
    /// material, a freed shader's materials draw with the built-in shader,
    /// target cameras, compute passes and screen effects using a freed
    /// target, pass or effect are skipped, sounds stop, text in a freed
    /// font uses the built-in font, and colliders keep a freed collision
    /// mesh until they are removed. Despawn or update them to stop that.
    /// The built-in font is never freed.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let beep = ctx.audio.create(8_000, &[0.0; 800]);
    ///     ctx.free(beep);
    ///     assert!(!ctx.is_live(beep));
    ///     let again = ctx.audio.create(8_000, &[0.0; 800]);
    ///     assert_ne!(again, beep); // handles are never given out twice
    ///     assert!(!ctx.is_live(beep));
    /// });
    /// ```
    pub fn free<H: Handle>(&mut self, handle: H) {
        handle.free_in(self);
        #[cfg(feature = "3d")]
        self.unlink_freed_gpu_buffers();
    }

    /// How many handles of each kind are live now (see [`LiveCounts`]).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     assert_eq!(ctx.live_counts().sounds, 0);
    /// });
    /// ```
    #[must_use]
    pub fn live_counts(&self) -> LiveCounts {
        LiveCounts {
            sounds: self.audio.live_count(),
            #[cfg(feature = "physics3d")]
            collision_meshes: self.physics3d.live_count(),
            ..self.gfx.live_counts()
        }
    }

    /// Returns freed GPU buffer ids to the pool once the compute passes
    /// that wrote to them are gone, so a pass never reaches the next buffer
    /// given the same id. Runs from [`Ctx::free`] and before each frame's
    /// passes are collected.
    #[cfg(feature = "3d")]
    pub(crate) fn unlink_freed_gpu_buffers(&mut self) {
        let unlinking = &mut self.gfx.assets3d.unlinking_gpu_buffers;
        if unlinking.is_empty() {
            return;
        }
        self.scene3d
            .passes
            .compute
            .retain(|p| p.buffer.is_none_or(|b| !unlinking.contains(&b.id)));
        self.scene3d
            .passes
            .scene_compute
            .retain(|p| p.buffer.is_none_or(|b| !unlinking.contains(&b.id)));
        for id in unlinking.drain(..) {
            self.gfx.assets3d.gpu_buffer_ids.release(id);
        }
    }
}

impl Graphics {
    /// Whether `handle` names something this game made and hasn't freed:
    /// a texture (render targets included), font, mesh, material, shader,
    /// compute pass, effect, GPU buffer, instance batch, texture array or
    /// terrain tile (live while all its meshes are).
    /// False for [`Handle::NONE`] and for handles from another game.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let t = ctx.gfx.create_texture(1, 1, &[255; 4]);
    ///     assert!(ctx.gfx.is_live(t) && ctx.gfx.is_live(Font::DEFAULT));
    ///     assert!(!ctx.gfx.is_live(Texture::NONE));
    /// });
    /// ```
    #[must_use]
    pub fn is_live<H: GfxHandle>(&self, handle: H) -> bool {
        handle.live_in_gfx(self)
    }

    /// Releases what `handle` names, as [`Ctx::free`] describes: whatever
    /// still uses it draws with the default. Freeing it again, or freeing
    /// [`Handle::NONE`], does nothing. Meshes and GPU buffers give their
    /// table slots to the next ones made (streamed worlds make and free
    /// them all the time) under new handles, so the freed handle stays dead
    /// and freeing it again leaves the new resource alone; a freed buffer's
    /// [compute passes](crate::render::RenderPasses::compute) (and
    /// [scene compute passes](crate::render::RenderPasses::scene_compute)) are removed
    /// before its slot is reused.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let t = ctx.gfx.create_texture(1, 1, &[255; 4]);
    ///     ctx.gfx.free(t);
    ///     assert!(!ctx.gfx.is_live(t));
    /// });
    /// ```
    pub fn free<H: GfxHandle>(&mut self, handle: H) {
        handle.free_in_gfx(self);
    }

    /// How many graphics handles of each kind are live now (see
    /// [`LiveCounts`]; `sounds` reads 0 here, [`Ctx::live_counts`] fills it).
    #[must_use]
    pub fn live_counts(&self) -> LiveCounts {
        #[cfg_attr(not(feature = "3d"), allow(unused_mut))]
        let mut counts = LiveCounts {
            textures: self.textures_live.len(),
            fonts: self.text.loaded_fonts(),
            sprite_shaders: self.sprites.shaders_live.len(),
            sprite_materials: self.sprites.materials_live.len(),
            ..LiveCounts::default()
        };
        #[cfg(feature = "3d")]
        self.count_live_3d(&mut counts);
        counts
    }
}

impl crate::audio::Audio {
    /// Whether `sound` names a sound this game made or loaded and hasn't
    /// freed. False for [`Handle::NONE`].
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let beep = ctx.audio.create(8_000, &[0.0; 80]);
    ///     assert!(ctx.audio.is_live(beep));
    ///     ctx.audio.free(beep);
    ///     assert!(!ctx.audio.is_live(beep));
    /// });
    /// ```
    #[must_use]
    pub fn is_live(&self, sound: crate::audio::Sound) -> bool {
        self.sounds_live.contains(sound.id)
    }

    /// Releases `sound`'s decoded audio and stops its voices; playing it
    /// again does nothing. No later sound is ever the same handle. Freeing
    /// it again does nothing.
    pub fn free(&mut self, sound: crate::audio::Sound) {
        self.free_id(sound.id);
    }

    /// How many sounds are live.
    pub(crate) fn live_count(&self) -> usize {
        self.sounds_live.len()
    }
}

/// Which ids of one kind were made, and which of those were freed. Ids
/// index a byte each, so a kind made a million times costs a megabyte.
#[derive(Debug, Default)]
pub(crate) struct LiveSet {
    state: Vec<u8>,
    live: usize,
}

const MADE: u8 = 1;
const FREED: u8 = 2;

impl LiveSet {
    /// Marks `id` made and live (once: a freed id stays freed).
    pub(crate) fn insert(&mut self, id: u32) {
        let i = id as usize;
        if i >= self.state.len() {
            self.state.resize(i + 1, 0);
        }
        if self.state[i] == 0 {
            self.state[i] = MADE;
            self.live += 1;
        }
    }

    /// Whether `id` was made and not freed.
    pub(crate) fn contains(&self, id: u32) -> bool {
        self.state.get(id as usize) == Some(&MADE)
    }

    /// Whether `id` was made, freed or not.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn was_made(&self, id: u32) -> bool {
        self.state.get(id as usize).is_some_and(|&s| s != 0)
    }

    /// Marks `id` freed; true if it was live.
    pub(crate) fn remove(&mut self, id: u32) -> bool {
        match self.state.get_mut(id as usize) {
            Some(s) if *s == MADE => {
                *s = FREED;
                self.live -= 1;
                true
            }
            _ => false,
        }
    }

    /// How many are live.
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    /// The live ids, in order.
    pub(crate) fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.state
            .iter()
            .enumerate()
            .filter(|&(_, &s)| s == MADE)
            .map(|(id, _)| id as u32)
    }
}

/// Bits of a [`SlotIds`] id that name its slot in the kind's tables; the
/// bits above count how often the slot was given out before. 20 bits allow
/// about a million slots at once, far more meshes or buffers than a GPU
/// holds; the 12 bits above let one slot be given out 4095 times before it
/// is retired, so a slot that streaming hammers costs one table entry per
/// 4095 makes instead of one per make.
#[cfg(feature = "3d")]
pub(crate) const INDEX_BITS: u32 = 20;
/// The slot bits of a [`SlotIds`] id (see [`INDEX_BITS`]).
#[cfg(feature = "3d")]
pub(crate) const INDEX_MASK: u32 = (1 << INDEX_BITS) - 1;
/// The last generation, never given out: `NONE` (`u32::MAX`) and the
/// engine's other sentinel ids (`u32::MAX - 1`, ...) have it.
#[cfg(feature = "3d")]
const LAST_GENERATION: u32 = u32::MAX >> INDEX_BITS;

/// The one id scheme for handle kinds whose tables reuse freed slots
/// (meshes, GPU buffers, collision meshes): the id each slot was last given
/// out as, and the freed ones waiting to be given out again. An id is its
/// slot in the low [`INDEX_BITS`] and its generation above; a freed slot is
/// given out again under the next generation, so a handle kept past `free`
/// never names the next resource in its slot. A slot whose generations are
/// used up is retired instead of wrapping: no id is ever given out twice.
///
/// Whether an id is live is the kind's own table's business (a mesh's
/// bounds, a buffer's length); this only maps ids to slots.
#[cfg(feature = "3d")]
#[derive(Debug, Default)]
pub(crate) struct SlotIds {
    /// The id each slot was last given out as; shorter than the kind's
    /// table while its last slots are on their first id (a missing entry
    /// is the slot's own number, generation 0).
    ids: Vec<u32>,
    /// Freed ids whose slots are given out next (under a new generation).
    free: Vec<u32>,
}

#[cfg(feature = "3d")]
impl SlotIds {
    /// The id for a new resource: a freed slot under its next generation,
    /// or else slot `len` (the kind's table length: the caller adds the
    /// slot). Panics when every slot is taken, live or retired. The last
    /// slot is never given out: it is `NONE`'s, so code that reads a
    /// handle's slot without checking it never lands on a real resource.
    pub(crate) fn make(&mut self, len: usize, what: &str) -> u32 {
        let id = match self.free.pop() {
            Some(freed) => freed + (1 << INDEX_BITS),
            None => {
                assert!(
                    len < INDEX_MASK as usize,
                    "ponko: no {what} slot left: all {} are live or retired",
                    INDEX_MASK
                );
                len as u32
            }
        };
        self.set(id);
        id
    }

    /// Gives `id` out again as it is (a resident mesh reloaded under the
    /// handle it had before it was evicted; see [`keep`](Self::keep)).
    pub(crate) fn remake(&mut self, id: u32) {
        self.set(id);
    }

    /// Records that `id`'s slot is given out as `id`.
    fn set(&mut self, id: u32) {
        let slot = (id & INDEX_MASK) as usize;
        while self.ids.len() <= slot {
            self.ids.push(self.ids.len() as u32);
        }
        self.ids[slot] = id;
    }

    /// The slot of `id` if `id` is the id its slot was last given out as
    /// (live or freed); `None` for a stale id or `NONE`. The caller checks
    /// the slot is within its table.
    #[inline]
    pub(crate) fn slot(&self, id: u32) -> Option<usize> {
        let slot = id & INDEX_MASK;
        (self.ids.get(slot as usize).copied().unwrap_or(slot) == id).then_some(slot as usize)
    }

    /// The id `slot` was last given out as.
    #[inline]
    pub(crate) fn id_in(&self, slot: usize) -> u32 {
        self.ids.get(slot).copied().unwrap_or(slot as u32)
    }

    /// Lets the slot of freed `id` be given out again, unless its
    /// generations are used up: then the slot is retired.
    pub(crate) fn release(&mut self, id: u32) {
        if (id >> INDEX_BITS) + 1 < LAST_GENERATION {
            self.free.push(id);
        }
    }

    /// Takes freed `id` back out of the pool, so its slot is kept for
    /// [`remake`](Self::remake); false if it wasn't there (retired).
    pub(crate) fn keep(&mut self, id: u32) -> bool {
        match self.free.iter().rposition(|&f| f == id) {
            Some(i) => {
                self.free.swap_remove(i);
                true
            }
            None => false,
        }
    }
}

/// `Handle` for a handle type whose only field is `id: u32`.
macro_rules! id_handle {
    ($ty:ty, $live:expr, $free:expr) => {
        impl sealed::Sealed for $ty {}
        impl raw::RawHandle for $ty {
            fn raw_id(self) -> u32 {
                self.id
            }
            fn from_raw_id(id: u32) -> Self {
                Self { id }
            }
        }
        impl Handle for $ty {
            const NONE: Self = Self { id: u32::MAX };
            fn live_in(self, ctx: &Ctx) -> bool {
                ctx.gfx.is_live(self)
            }
            fn free_in(self, ctx: &mut Ctx) {
                ctx.gfx.free(self)
            }
        }
        impl GfxHandle for $ty {
            fn live_in_gfx(self, gfx: &Graphics) -> bool {
                #[allow(clippy::redundant_closure_call)]
                ($live)(gfx, self)
            }
            fn free_in_gfx(self, gfx: &mut Graphics) {
                #[allow(clippy::redundant_closure_call)]
                ($free)(gfx, self)
            }
        }
    };
}

impl sealed::Sealed for crate::texture::Texture {}
impl raw::RawHandle for crate::texture::Texture {
    fn raw_id(self) -> u32 {
        self.id
    }
    fn from_raw_id(id: u32) -> Self {
        crate::texture::Texture::new(id, 0, 0)
    }
}
impl Handle for crate::texture::Texture {
    const NONE: Self = crate::texture::Texture::new(u32::MAX, 0, 0);
    fn live_in(self, ctx: &Ctx) -> bool {
        ctx.gfx.is_live(self)
    }
    fn free_in(self, ctx: &mut Ctx) {
        ctx.gfx.free(self);
    }
}
impl GfxHandle for crate::texture::Texture {
    fn live_in_gfx(self, gfx: &Graphics) -> bool {
        gfx.textures_live.contains(self.id)
    }
    fn free_in_gfx(self, gfx: &mut Graphics) {
        gfx.free_texture_id(self.id);
    }
}

impl sealed::Sealed for crate::audio::Sound {}
impl raw::RawHandle for crate::audio::Sound {
    fn raw_id(self) -> u32 {
        self.id
    }
    fn from_raw_id(id: u32) -> Self {
        crate::audio::Sound::new(id, 0.0)
    }
}
impl Handle for crate::audio::Sound {
    const NONE: Self = crate::audio::Sound::new(u32::MAX, 0.0);
    fn live_in(self, ctx: &Ctx) -> bool {
        ctx.audio.is_live(self)
    }
    fn free_in(self, ctx: &mut Ctx) {
        ctx.audio.free(self);
    }
}

impl sealed::Sealed for crate::text::Font {}
impl raw::RawHandle for crate::text::Font {
    fn raw_id(self) -> u32 {
        self.0
    }
    fn from_raw_id(id: u32) -> Self {
        crate::text::Font(id)
    }
}
impl Handle for crate::text::Font {
    const NONE: Self = crate::text::Font(u32::MAX);
    fn live_in(self, ctx: &Ctx) -> bool {
        ctx.gfx.is_live(self)
    }
    fn free_in(self, ctx: &mut Ctx) {
        ctx.gfx.free(self);
    }
}
impl GfxHandle for crate::text::Font {
    fn live_in_gfx(self, gfx: &Graphics) -> bool {
        gfx.text.has_font(self)
    }
    fn free_in_gfx(self, gfx: &mut Graphics) {
        gfx.free_font_id(self.0);
    }
}

id_handle!(
    crate::sprite_material::SpriteShader,
    |g: &Graphics, h: crate::sprite_material::SpriteShader| g.sprites.shaders_live.contains(h.id),
    |g: &mut Graphics, h: crate::sprite_material::SpriteShader| g.free_sprite_shader_id(h.id)
);
id_handle!(
    crate::sprite_material::SpriteMaterial,
    |g: &Graphics, h: crate::sprite_material::SpriteMaterial| g.sprites.materials_live.contains(h.id),
    |g: &mut Graphics, h: crate::sprite_material::SpriteMaterial| g.free_sprite_material_id(h.id)
);

#[cfg(feature = "3d")]
mod three {
    use super::*;
    use crate::render::Compute;
    use crate::render::Effect;
    use crate::render::GpuBuffer;
    use crate::render::InstanceBatch;
    use crate::render::Material;
    use crate::render::Mesh;
    use crate::render::Shader;
    use crate::terrain::{TerrainTile, TextureArray};

    id_handle!(
        Mesh,
        |g: &Graphics, h: Mesh| g.assets3d.live_bounds(h.id).is_some(),
        |g: &mut Graphics, h: Mesh| g.free_mesh_id(h.id)
    );
    id_handle!(
        Material,
        |g: &Graphics, h: Material| g.assets3d.materials_live.contains(h.id),
        |g: &mut Graphics, h: Material| g.free_material_id(h.id)
    );
    id_handle!(
        Shader,
        |g: &Graphics, h: Shader| g.assets3d.shaders_live.contains(h.id),
        |g: &mut Graphics, h: Shader| g.free_shader_id(h.id)
    );
    id_handle!(
        Compute,
        |g: &Graphics, h: Compute| g.assets3d.computes_live.contains(h.id),
        |g: &mut Graphics, h: Compute| g.free_compute_id(h.id)
    );
    id_handle!(
        Effect,
        |g: &Graphics, h: Effect| g.assets3d.effects_live.contains(h.id),
        |g: &mut Graphics, h: Effect| g.free_effect_id(h.id)
    );
    // The buffer's id waits in `unlinking_gpu_buffers` until the compute
    // passes writing to it are removed (`Ctx::unlink_freed_gpu_buffers`).
    id_handle!(
        GpuBuffer,
        |g: &Graphics, h: GpuBuffer| g.gpu_buffer_len(h).is_some(),
        |g: &mut Graphics, h: GpuBuffer| g.free_gpu_buffer(h)
    );
    id_handle!(
        InstanceBatch,
        |g: &Graphics, h: InstanceBatch| g.instance_batch_len(h).is_some(),
        |g: &mut Graphics, h: InstanceBatch| g.free_instance_batch_id(h.id)
    );

    impl sealed::Sealed for TextureArray {}
    impl raw::RawHandle for TextureArray {
        fn raw_id(self) -> u32 {
            self.id
        }
        fn from_raw_id(id: u32) -> Self {
            TextureArray {
                id,
                width: 0,
                height: 0,
                layers: 0,
            }
        }
    }
    impl Handle for TextureArray {
        const NONE: Self = TextureArray {
            id: u32::MAX,
            width: 0,
            height: 0,
            layers: 0,
        };
        fn live_in(self, ctx: &Ctx) -> bool {
            ctx.gfx.is_live(self)
        }
        fn free_in(self, ctx: &mut Ctx) {
            ctx.gfx.free(self);
        }
    }
    impl GfxHandle for TextureArray {
        fn live_in_gfx(self, gfx: &Graphics) -> bool {
            gfx.assets3d.arrays_live.contains(self.id)
        }
        fn free_in_gfx(self, gfx: &mut Graphics) {
            gfx.free_texture_array_id(self.id);
        }
    }

    // A tile is live while all its detail levels are; freeing it frees them all.
    impl sealed::Sealed for TerrainTile {}
    impl Handle for TerrainTile {
        const NONE: Self = TerrainTile {
            lods: [Mesh::NONE; TerrainTile::LODS],
            lod_distance: 0.0,
        };
        fn live_in(self, ctx: &Ctx) -> bool {
            ctx.gfx.is_live(self)
        }
        fn free_in(self, ctx: &mut Ctx) {
            ctx.gfx.free(self);
        }
    }
    impl GfxHandle for TerrainTile {
        fn live_in_gfx(self, gfx: &Graphics) -> bool {
            self.lods.iter().all(|&mesh| gfx.is_live(mesh))
        }
        fn free_in_gfx(self, gfx: &mut Graphics) {
            for mesh in self.lods {
                gfx.free(mesh);
            }
        }
    }
}

#[cfg(feature = "physics3d")]
mod collision {
    use super::*;
    use crate::physics3d::CollisionMesh;

    impl sealed::Sealed for CollisionMesh {}
    impl raw::RawHandle for CollisionMesh {
        fn raw_id(self) -> u32 {
            self.id
        }
        fn from_raw_id(id: u32) -> Self {
            Self { id }
        }
    }
    impl Handle for CollisionMesh {
        const NONE: Self = Self { id: u32::MAX };
        fn live_in(self, ctx: &Ctx) -> bool {
            ctx.physics3d.is_live(self)
        }
        fn free_in(self, ctx: &mut Ctx) {
            ctx.physics3d.free(self);
        }
    }
}

/// The raw-id trait behind [`crate::__ffi`]. The trait is `pub` only so
/// `__ffi`'s functions can take it as a bound; this module is private to
/// the crate, so no path outside it (and so no compiler hint) names it.
pub(crate) mod raw {
    /// The id inside a handle of one id, and a handle from an id.
    // Not `id`/`from_id`: a compiler hint for `handle.id()` mustn't point here.
    pub trait RawHandle: super::Handle {
        /// The handle's id.
        fn raw_id(self) -> u32;
        /// A handle from an id from [`raw_id`](Self::raw_id).
        fn from_raw_id(id: u32) -> Self;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freed_id_stays_freed() {
        let mut set = LiveSet::default();
        set.insert(3);
        assert!(set.contains(3) && set.was_made(3) && !set.was_made(2));
        assert_eq!(set.len(), 1);
        assert!(set.remove(3));
        assert!(!set.remove(3));
        set.insert(3);
        assert!(!set.contains(3) && set.was_made(3));
        assert_eq!(set.len(), 0);
    }

    /// One kind's table as the engine keeps one: `make`, then add the slot.
    #[cfg(feature = "3d")]
    fn make(ids: &mut SlotIds, table: &mut usize) -> u32 {
        let id = ids.make(*table, "test");
        if (id & INDEX_MASK) as usize == *table {
            *table += 1;
        }
        id
    }

    #[cfg(feature = "3d")]
    #[test]
    fn a_freed_slot_comes_back_under_its_next_generation() {
        let (mut ids, mut table) = (SlotIds::default(), 0);
        let a = make(&mut ids, &mut table);
        let b = make(&mut ids, &mut table);
        assert_eq!((a, b), (0, 1));
        ids.release(a);
        let c = make(&mut ids, &mut table);
        assert_eq!((c & INDEX_MASK, c >> INDEX_BITS), (0, 1));
        assert_eq!(ids.slot(c), Some(0));
        assert_eq!(ids.slot(a), None, "the old id is stale");
        assert_eq!((ids.id_in(0), table), (c, 2));
    }

    /// A slot hammered by streaming is given out 4095 times, then retired:
    /// no id repeats, and none has the generation `NONE` and the engine's
    /// other sentinels use.
    #[cfg(feature = "3d")]
    #[test]
    fn a_slot_is_retired_instead_of_wrapping() {
        let (mut ids, mut table) = (SlotIds::default(), 0);
        let mut seen = std::collections::HashSet::new();
        for i in 0..10_000 {
            let id = make(&mut ids, &mut table);
            assert!(seen.insert(id), "{id:#x} given out twice");
            assert_ne!(id >> INDEX_BITS, LAST_GENERATION, "made {i}");
            ids.release(id);
        }
        // 4095 makes per slot: 10 000 makes took three slots.
        assert_eq!(table, 3);
        for sentinel in [u32::MAX, u32::MAX - 1, u32::MAX - 2] {
            assert_eq!(ids.slot(sentinel), None);
        }
    }

    #[cfg(feature = "3d")]
    #[test]
    fn the_none_slot_is_never_given_out() {
        let mut ids = SlotIds::default();
        let last = INDEX_MASK - 1;
        assert_eq!(ids.make(last as usize, "mesh"), last, "the last slot there is");
        let none = <crate::render::Mesh as Handle>::NONE.id;
        assert_eq!(none & INDEX_MASK, INDEX_MASK, "NONE's slot");
        let next = std::panic::catch_unwind(move || ids.make(INDEX_MASK as usize, "mesh"));
        let message = *next.expect_err("NONE's slot given out").downcast::<String>().unwrap();
        assert!(message.contains("no mesh slot left"), "{message}");
    }

    #[cfg(feature = "3d")]
    #[test]
    fn a_kept_id_is_given_out_again_as_it_was() {
        let (mut ids, mut table) = (SlotIds::default(), 0);
        let a = make(&mut ids, &mut table);
        ids.release(a);
        assert!(ids.keep(a), "taken out of the pool");
        let b = make(&mut ids, &mut table);
        assert_eq!(b & INDEX_MASK, 1, "a new slot: the kept one waits");
        ids.remake(a);
        assert_eq!(ids.slot(a), Some(0));
        // A retired slot's last id can be kept and remade too.
        let mut last = b;
        while (last >> INDEX_BITS) + 1 < LAST_GENERATION {
            ids.release(last);
            last = make(&mut ids, &mut table);
        }
        ids.release(last);
        assert!(!ids.keep(last), "retired: not in the pool");
        ids.remake(last);
        assert_eq!(ids.slot(last), Some(1));
    }
}
