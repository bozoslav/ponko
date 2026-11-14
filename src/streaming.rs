//! GPU asset loading by name: residency budgets, asynchronous streaming
//! with a per-frame upload budget, deterministic asset states, and textures
//! that stream in from a small stand-in up to full resolution.
//!
//! [`Graphics`](crate::gfx::Graphics) (`ctx.gfx`) is the one service that
//! owns GPU assets. Which loader to use:
//!
//! | Call | Waits for the file | Under a residency budget |
//! |---|---|---|
//! | [`load_texture`](crate::gfx::Graphics::load_texture), `load_mesh` | yes | no: kept until freed |
//! | [`resident_texture`](crate::gfx::Graphics::resident_texture), `resident_mesh` | the first time | yes |
//! | [`stream_texture`](crate::gfx::Graphics::stream_texture), `stream_mesh` | never | yes |
//!
//! Two budgets, both on `Graphics`:
//! [`set_upload_budget`](crate::gfx::Graphics::set_upload_budget) limits
//! the bytes uploaded per frame, and
//! [`set_residency_budget`](crate::gfx::Graphics::set_residency_budget) the
//! bytes of each [`ResidencyKind`] kept on the GPU. The calls that can evict
//! take the [`World`] too, because eviction never takes what an entity shows.
//!
//! [`crate::stream`] is different: it holds no assets, only the tools for a
//! game's own background work ([`Job`]) and cell grids
//! (`CellStreamer`).
//!
//! [`Graphics::stream_texture`](crate::gfx::Graphics::stream_texture) (and, with the `3d` feature,
//! [`Graphics::stream_mesh`](crate::gfx::Graphics::stream_mesh)) never wait: the first call starts decoding the file
//! on a [`Job`] thread and returns `None`; once decoded, the
//! asset is uploaded at the start of a frame, within the
//! [upload budget](crate::gfx::Graphics::set_upload_budget). A texture appears first as a
//! stand-in at most 32 pixels a side and doubles in resolution each upload
//! until it is full size or its [residency budget](crate::gfx::Graphics::set_residency_budget)
//! is reached. Evicted textures drop back to that stand-in; evicted meshes
//! keep their id for when they come back.
//!
//! Each asset's [`AssetState`] changes only in that upload step, and
//! uploads run in the order the assets were asked for. With
//! [`Graphics::set_deterministic_uploads`](crate::gfx::Graphics::set_deterministic_uploads) the step waits for decoding instead of
//! moving on, so which frame each state changes in depends only on the
//! order of requests and the budget: the same run gives the same frames.
//! States are fixed per frame, not per simulation tick (a frame may run
//! several ticks or none). Without it, decodes finish when the workers do:
//! the frame an asset arrives in, and so the order new textures get their
//! ids in, can differ between runs.

use std::collections::{BTreeMap, VecDeque};

use crate::assets::{self, Origin};
pub use crate::residency::{ResidencyKind, ResidencyStats};
use crate::{ctx::Ctx, stream::Job, texture::Texture};
use ponko_ecs::World;

/// The longest side of a streamed texture's stand-in.
pub(crate) const STAND_IN: u32 = 32;

/// The default [upload budget](crate::gfx::Graphics::set_upload_budget): 16 MiB a frame.
pub const DEFAULT_UPLOAD_BUDGET: u64 = 16 << 20;

/// Where a streamed asset is ([`Graphics::asset_state`](crate::gfx::Graphics::asset_state)). States change only
/// in the upload step at the start of a frame, in the order assets were
/// asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AssetState {
    /// Never asked for through streaming or residency (or freed since).
    Unknown,
    /// Being read and decoded on a worker thread.
    Loading,
    /// Decoded, waiting for its turn within the upload budget.
    Uploading,
    /// A texture shown at less than full resolution, getting sharper as
    /// uploads go on (or held there by its residency budget).
    Streaming,
    /// Fully loaded and on the GPU.
    Resident,
    /// Evicted to fit its residency budget: a texture shows its small
    /// stand-in, a mesh draws nothing. Asking for it again brings it back
    /// under the same handle.
    Evicted,
    /// Loading failed; [`Graphics::asset_error`](crate::gfx::Graphics::asset_error) says why. Streaming doesn't
    /// retry it by itself; loading it with `resident_*` does.
    Failed,
}

impl AssetState {
    /// Its name in reports, such as `"streaming"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            AssetState::Unknown => "unknown",
            AssetState::Loading => "loading",
            AssetState::Uploading => "uploading",
            AssetState::Streaming => "streaming",
            AssetState::Resident => "resident",
            AssetState::Evicted => "evicted",
            AssetState::Failed => "failed",
        }
    }
}

/// The upload queue's budget and counters ([`Graphics::upload_stats`](crate::gfx::Graphics::upload_stats)).
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let uploads = ctx.gfx.upload_stats();
///     assert_eq!(uploads.budget, Some(ponko::streaming::DEFAULT_UPLOAD_BUDGET));
///     assert_eq!(uploads.pending, 0);
/// });
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct UploadStats {
    /// Bytes uploaded per frame at most; `None` for no limit. One upload
    /// larger than the budget still goes, alone in its frame.
    pub budget: Option<u64>,
    /// Assets decoding or waiting to upload.
    pub pending: usize,
    /// Bytes uploaded in the last upload step.
    pub frame_bytes: u64,
    /// Uploads in the last upload step.
    pub frame_uploads: u32,
    /// Bytes uploaded since the game started.
    pub total_bytes: u64,
    /// Uploads since the game started.
    pub total_uploads: u64,
    /// Steps that went over the budget because a single upload was larger.
    pub oversized: u64,
    /// Times the game thread waited for a decode: only with
    /// [deterministic uploads](crate::gfx::Graphics::set_deterministic_uploads) or in
    /// [`Ctx::finish_uploads`](crate::ctx::Ctx::finish_uploads).
    pub decode_waits: u64,
    /// Texture uploads held back because the texture budget was full.
    pub held_by_budget: u64,
    /// Loads that failed.
    pub failures: u64,
    /// Milliseconds the last upload step took on the game thread.
    pub step_ms: f32,
    /// The longest upload step so far, in milliseconds.
    pub max_step_ms: f32,
}

/// One streamed asset, for diagnostics ([`Graphics::streamed_assets`](crate::gfx::Graphics::streamed_assets)).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct StreamedAsset {
    /// Its kind.
    pub kind: ResidencyKind,
    /// The name it was asked for by.
    pub name: String,
    /// Where it is.
    pub state: AssetState,
    /// Its handle's id (a texture or mesh id); `None` before it has one.
    pub id: Option<u32>,
    /// For a texture, the resolution on the GPU now; for a mesh, `[0, 0]`.
    pub size: [u32; 2],
}

/// One resolution of a texture: RGBA8 pixels, or block-compressed ones.
pub(crate) struct Level {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Block-compressed instead of `rgba` (then empty): this resolution
    /// and its mipmaps, from a KTX2 or DDS file's chain.
    #[cfg(feature = "compressed-textures")]
    pub blocks: Option<crate::compressed::CompressedImage>,
}

impl Level {
    fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
            #[cfg(feature = "compressed-textures")]
            blocks: None,
        }
    }

    /// Bytes on the GPU.
    fn bytes(&self) -> u64 {
        #[cfg(feature = "compressed-textures")]
        if let Some(blocks) = &self.blocks {
            return blocks.bytes();
        }
        crate::inventory::rgba8_bytes(self.width, self.height)
    }

    fn duplicate(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            rgba: self.rgba.clone(),
            #[cfg(feature = "compressed-textures")]
            blocks: self.blocks.clone(),
        }
    }
}

/// A texture's resolutions, full size first, halving down to the stand-in.
pub(crate) type Pyramid = Vec<Level>;

/// Halves `level` by averaging 2 × 2 blocks (edge pixels repeat for odd sizes).
fn halve(level: &Level) -> Level {
    let (w, h) = (level.width, level.height);
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut rgba = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (2 * x + dx).min(w - 1);
                let sy = (2 * y + dy).min(h - 1);
                let i = ((sy * w + sx) * 4) as usize;
                for (s, &v) in sum.iter_mut().zip(&level.rgba[i..i + 4]) {
                    *s += u32::from(v);
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for (d, s) in rgba[o..o + 4].iter_mut().zip(sum) {
                *d = ((s + 2) / 4) as u8;
            }
        }
    }
    Level::new(nw, nh, rgba)
}

/// The resolutions of `image`, from the largest that fits `max` down to
/// one whose longest side is at most [`STAND_IN`].
pub(crate) fn pyramid(image: assets::Image, max: u32) -> Pyramid {
    let mut levels = vec![Level::new(image.width, image.height, image.rgba)];
    loop {
        let last = levels.last().expect("one level");
        if last.width.max(last.height) <= STAND_IN {
            break;
        }
        let next = halve(last);
        levels.push(next);
    }
    // Levels larger than the GPU can hold never upload.
    let fits = levels
        .iter()
        .position(|l| l.width.max(l.height) <= max)
        .unwrap_or(levels.len() - 1);
    levels.drain(..fits);
    levels
}

/// The resolutions of a block-compressed image: its own mip levels, each
/// with the rest of the chain, from the largest that fits `max` down to one
/// whose longest side is at most [`STAND_IN`]. Only whole-block levels can
/// be a texture's largest, so the stand-in may be larger for odd sizes.
#[cfg(feature = "compressed-textures")]
pub(crate) fn block_pyramid(image: &crate::compressed::CompressedImage, max: u32) -> Pyramid {
    let mut levels = Vec::new();
    for i in 0..image.levels.len() {
        let (w, h) = image.level_size(i);
        if !crate::compressed::block_aligned(w, h) {
            break;
        }
        if w.max(h) <= max || i + 1 == image.levels.len() {
            let mut level = Level::new(w, h, Vec::new());
            level.blocks = Some(image.starting_at_level(i));
            levels.push(level);
        }
        if w.max(h) <= STAND_IN {
            break;
        }
    }
    levels
}

type Decoded<T> = Job<Result<T, crate::error::Error>>;

/// A streamed texture.
pub(crate) struct StreamedTexture {
    /// Its handle: the full size, whatever resolution is on the GPU. `None`
    /// until the stand-in is uploaded.
    pub texture: Option<Texture>,
    origin: Option<Origin>,
    /// The resolutions still to upload (all of them while streaming up).
    levels: Pyramid,
    /// Which of `levels` is on the GPU (0 is the largest).
    at: usize,
    /// The resolution on the GPU now.
    pub size: [u32; 2],
    /// The stand-in, kept for eviction.
    stand_in: Option<Level>,
    job: Option<Decoded<(Origin, Pyramid)>>,
    pub state: AssetState,
    /// In the upload queue now.
    queued: bool,
    /// Held by the texture budget: the budget and resident bytes then, and
    /// the bytes the next resolution needs. It is decoded again only once
    /// the budget changed, or the resident bytes did and that many could
    /// now fit.
    held: Option<(Option<u64>, u64, u64)>,
}

/// A mesh being streamed in.
#[cfg(feature = "3d")]
pub(crate) struct StreamedMesh {
    pub job: Option<Decoded<(Origin, crate::three::LoadedFile)>>,
    /// Decoded, waiting for room in the upload budget.
    pub ready: Option<(Origin, crate::three::LoadedFile)>,
    pub state: AssetState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Texture(String),
    #[cfg(feature = "3d")]
    Mesh(String),
}

/// The upload queue and every streamed asset (a `Ctx` field).
pub(crate) struct Streaming {
    budget: Option<u64>,
    deterministic: bool,
    queue: VecDeque<Request>,
    pub textures: BTreeMap<String, StreamedTexture>,
    /// Streamed texture names by texture id.
    pub texture_names: std::collections::HashMap<u32, String>,
    #[cfg(feature = "3d")]
    pub meshes: BTreeMap<String, StreamedMesh>,
    errors: BTreeMap<(u8, String), crate::error::Error>,
    pub stats: UploadStats,
    /// Streamed LOD groups this frame: reused, so it doesn't allocate.
    #[cfg(feature = "3d")]
    pub lod_scratch: Vec<(ponko_ecs::Entity, std::sync::Arc<[String]>, u64)>,
}

impl Default for Streaming {
    fn default() -> Self {
        Self {
            budget: Some(DEFAULT_UPLOAD_BUDGET),
            deterministic: false,
            queue: VecDeque::new(),
            textures: BTreeMap::new(),
            texture_names: std::collections::HashMap::new(),
            #[cfg(feature = "3d")]
            meshes: BTreeMap::new(),
            errors: BTreeMap::new(),
            stats: UploadStats {
                budget: Some(DEFAULT_UPLOAD_BUDGET),
                ..UploadStats::default()
            },
            #[cfg(feature = "3d")]
            lod_scratch: Vec::new(),
        }
    }
}

fn kind_key(kind: ResidencyKind) -> u8 {
    match kind {
        ResidencyKind::Mesh => 0,
        ResidencyKind::Texture => 1,
    }
}

impl Streaming {
    fn pending(&self) -> usize {
        self.queue.len()
    }

    /// The resolution texture `id` has on the GPU, if it is a streamed one
    /// at less than its full size.
    pub(crate) fn reduced_size(&self, id: u32) -> Option<[u32; 2]> {
        let name = self.texture_names.get(&id)?;
        let t = self.textures.get(name)?;
        let full = t.texture.map(|t| [t.width(), t.height()])?;
        (t.size != full).then_some(t.size)
    }

    pub(crate) fn fail(&mut self, kind: ResidencyKind, name: &str, error: crate::error::Error) {
        eprintln!("ponko: could not stream {name}: {error}");
        self.stats.failures += 1;
        self.errors.insert((kind_key(kind), name.to_string()), error);
    }

    /// Queues a mesh's upload step.
    #[cfg(feature = "3d")]
    pub(crate) fn queue_mesh(&mut self, name: &str) {
        self.queue.push_back(Request::Mesh(name.to_string()));
    }
}

/// What one upload did: its bytes, or nothing.
pub(crate) enum Step {
    /// Uploaded this many bytes; keep the request queued if `again`.
    Uploaded { bytes: u64, again: bool },
    /// Not decoded yet: try again later.
    NotReady,
    /// Dropped from the queue (done, failed, held or freed).
    Dropped,
}

impl Ctx {
    /// The engine's per-frame streaming work, before the game's systems:
    /// streamed LOD groups ask for their levels, then the upload step.
    pub(crate) fn pump_streaming(&mut self) {
        #[cfg(feature = "3d")]
        self.stream_lod_levels();
        // What entities stopped showing (LOD levels, despawned models) can
        // go now. While a kind is over its budget this looks, every frame,
        // at each entity that shows an asset of that kind (it allocates
        // nothing); under budget it costs a comparison.
        for kind in ResidencyKind::ALL {
            self.gfx.enforce_residency(&mut self.world, kind);
        }
        if !self.gfx.streaming.queue.is_empty() {
            self.gfx.upload_step(&mut self.world);
        } else {
            self.gfx.streaming.stats.frame_bytes = 0;
            self.gfx.streaming.stats.frame_uploads = 0;
        }
    }

    /// Waits until every streamed asset asked for so far is loaded (or
    /// failed, or held by its residency budget), uploading without the
    /// per-frame budget, and evicting what the residency budgets need gone
    /// as the frame's streaming step does. For loading screens and tests; it
    /// blocks the game thread. On `Ctx` because eviction looks at what the
    /// game's world shows.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     assert_eq!(ctx.gfx.stream_texture("missing.png"), None);
    ///     ctx.finish_uploads(); // a loading screen; games just keep asking
    ///     let state = ctx.gfx.asset_state(ponko::streaming::ResidencyKind::Texture, "missing.png");
    ///     assert_eq!(state, ponko::streaming::AssetState::Failed);
    /// });
    /// ```
    pub fn finish_uploads(&mut self) {
        self.gfx.finish_uploads(&mut self.world);
    }
}

impl crate::gfx::Graphics {
    /// Texture `path`, streamed in without waiting: `None` the first time
    /// (it starts loading on a worker thread) and until a small stand-in
    /// (at most 32 pixels a side) is uploaded; from then on the same
    /// [`Texture`], at its full size, while its pixels get sharper over the
    /// next frames. Ask for it each tick or frame you need it, as with
    /// [`resident_texture`](Self::resident_texture): asking marks it used
    /// for the [texture budget](Self::set_residency_budget), and asking for
    /// an evicted one streams it back in under the same handle.
    ///
    /// A missing or broken file gives `None` (or, coming back from
    /// eviction, keeps the stand-in), with [`AssetState::Failed`] and the
    /// error in [`asset_error`](crate::gfx::Graphics::asset_error). Asking again doesn't
    /// retry, so a missing file costs nothing each frame;
    /// [`resident_texture`](Self::resident_texture) loads it again. Files from added [asset sources](crate::assets::Assets::add_source)
    /// are read on the game thread (their trait runs there) and decoded on
    /// a worker. In a browser, decoding runs when the load starts.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let mut png = Vec::new();
    ///     image::RgbaImage::from_pixel(256, 128, image::Rgba([90, 160, 60, 255]))
    ///         .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    ///         .unwrap();
    ///     ctx.assets.add_source(MemorySource::new("mem").with("grass.png", png));
    ///     assert_eq!(ctx.gfx.stream_texture("grass.png"), None); // loading
    ///     ctx.finish_uploads(); // a loading screen; games just keep asking
    ///     let grass = ctx.gfx.stream_texture("grass.png").unwrap();
    ///     assert_eq!((grass.width(), grass.height()), (256, 128));
    ///     assert_eq!(ctx.gfx.asset_state(ponko::streaming::ResidencyKind::Texture, "grass.png"), ponko::streaming::AssetState::Resident);
    /// });
    /// ```
    pub fn stream_texture(&mut self, path: &str) -> Option<Texture> {
        let now = self.residency_now();
        self.forget_freed_texture(path);
        let state = self
            .streaming
            .textures
            .get(path)
            .map(|t| (t.state, t.texture, t.queued));
        match state {
            None => {
                self.start_texture_decode(path);
                self.streaming.textures.get(path).and_then(|t| t.texture)
            }
            Some((AssetState::Evicted, texture, queued)) => {
                // Asked for again while its decode runs: wait for that one.
                if !queued {
                    self.start_texture_decode(path);
                }
                texture
            }
            // Not retried each frame: see `asset_error`.
            Some((AssetState::Failed, texture, _)) => texture,
            Some((AssetState::Streaming, texture, queued)) => {
                self.residency.set_mut(ResidencyKind::Texture).touch(path, now);
                if !queued && self.held_texture_may_fit(path, now) {
                    self.start_texture_decode(path);
                }
                texture
            }
            Some((_, texture, _)) => {
                self.residency.set_mut(ResidencyKind::Texture).touch(path, now);
                texture
            }
        }
    }

    /// Texture `path` at full resolution, loaded now if it isn't (decoding
    /// on this thread, as [`load_texture`](Self::load_texture) does), under
    /// the [texture budget](Self::set_residency_budget). Ask for it each
    /// tick or frame you need it: when resident textures go over the
    /// budget, the least recently used ones that no [`Sprite`](crate::d2::Sprite)
    /// shows drop to a small stand-in (at most 32 pixels a side) until they
    /// are asked for again. The handle stays the same throughout and is
    /// never freed by the budget.
    ///
    /// The budget is checked when a texture loads and at the start of every
    /// frame. Never evicted: textures asked for in this or the previous
    /// frame or tick, ones a `Sprite` shows, and [pinned](Self::pin_resident_texture) ones.
    /// Textures that only materials use are kept while asked for; evicted,
    /// those materials sample the stand-in.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let mut png = Vec::new();
    ///     image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 30, 30, 255]))
    ///         .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    ///         .unwrap();
    ///     ctx.assets.add_source(MemorySource::new("mem").with("brick.png", png));
    ///     ctx.gfx.set_residency_budget(ponko::streaming::ResidencyKind::Texture, Some(64 << 20));
    ///     let brick = ctx.gfx.resident_texture("brick.png");
    ///     assert_eq!(brick.width(), 64);
    ///     assert!(ctx.gfx.is_resident(ponko::streaming::ResidencyKind::Texture, "brick.png"));
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// With the loading error if the file is missing or isn't an image, as
    /// `load_texture` does.
    pub fn resident_texture(&mut self, path: &str) -> Texture {
        self.try_resident_texture(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`resident_texture`](Self::resident_texture) with errors as values,
    /// the same ones [`try_load_texture`](crate::gfx::Graphics::try_load_texture) returns.
    ///
    /// # Errors
    ///
    /// As `try_load_texture`.
    pub fn try_resident_texture(&mut self, path: &str) -> Result<Texture, crate::error::Error> {
        let now = self.residency_now();
        self.forget_freed_texture(path);
        if let Some(t) = self.streaming.textures.get(path)
            && t.state == AssetState::Resident
            && let Some(texture) = t.texture
        {
            self.residency.set_mut(ResidencyKind::Texture).touch(path, now);
            return Ok(texture);
        }
        let (origin, levels) = self.decode_texture_now(path)?;
        let entry = self.texture_entry(path);
        entry.levels = levels;
        entry.origin = Some(origin);
        entry.job = None;
        self.upload_texture_level(path, 0);
        let texture = self.streaming.textures[path].texture.expect("uploaded");
        Ok(texture)
    }

    /// Like [`resident_texture`](Self::resident_texture), and never evicted
    /// until [`unpin_resident_texture`](Self::unpin_resident_texture).
    ///
    /// # Panics
    ///
    /// As `resident_texture`.
    pub fn pin_resident_texture(&mut self, path: &str) -> Texture {
        let texture = self.resident_texture(path);
        self.residency.set_mut(ResidencyKind::Texture).set_pinned(path, true);
        texture
    }

    /// Lets a [pinned](Self::pin_resident_texture) texture be evicted
    /// again, from the start of the next frame if the budget needs it gone.
    pub fn unpin_resident_texture(&mut self, path: &str) {
        self.residency.set_mut(ResidencyKind::Texture).set_pinned(path, false);
    }

    /// Uploads everything queued, without the per-frame budget
    /// ([`Ctx::finish_uploads`]).
    pub(crate) fn finish_uploads(&mut self, world: &mut World) {
        let (budget, deterministic) = (self.streaming.budget, self.streaming.deterministic);
        self.streaming.budget = None;
        self.streaming.deterministic = true;
        while !self.streaming.queue.is_empty() {
            self.upload_step(world);
        }
        self.streaming.budget = budget;
        self.streaming.deterministic = deterministic;
    }

    /// Uploads queued assets in order, within the budget.
    pub(crate) fn upload_step(&mut self, world: &mut World) {
        let start = crate::perf::Instant::now();
        let budget = self.streaming.budget.unwrap_or(u64::MAX);
        let (mut spent, mut uploads) = (0u64, 0u32);
        let mut later = Vec::new();
        while let Some(request) = self.streaming.queue.pop_front() {
            if uploads > 0 && spent >= budget {
                self.streaming.queue.push_front(request);
                break;
            }
            let step = match &request {
                Request::Texture(name) => self.texture_step(world, name, budget.saturating_sub(spent), uploads == 0),
                #[cfg(feature = "3d")]
                Request::Mesh(name) => self.mesh_step(world, name, budget.saturating_sub(spent), uploads == 0),
            };
            match step {
                Step::Uploaded { bytes, again } => {
                    if uploads == 0 && bytes > budget {
                        self.streaming.stats.oversized += 1;
                    }
                    spent += bytes;
                    uploads += 1;
                    if again {
                        later.push(request);
                    }
                }
                Step::NotReady => later.push(request),
                Step::Dropped => {}
            }
        }
        // Requests that go on (or aren't ready) wait behind the rest, in
        // their order.
        self.streaming.queue.extend(later);
        let stats = &mut self.streaming.stats;
        stats.frame_bytes = spent;
        stats.frame_uploads = uploads;
        stats.total_bytes += spent;
        stats.total_uploads += u64::from(uploads);
        stats.step_ms = start.elapsed().as_secs_f32() * 1000.0;
        stats.max_step_ms = stats.max_step_ms.max(stats.step_ms);
    }

    /// Takes `job`'s result if it is done, or waits for it in
    /// deterministic mode; `None` if it isn't done.
    pub(crate) fn take_decoded<T: Send + 'static>(&mut self, job: &mut Option<Job<T>>) -> Option<T> {
        let ready = job.as_mut()?.take();
        if ready.is_some() {
            *job = None;
            return ready;
        }
        if !self.streaming.deterministic {
            return None;
        }
        self.streaming.stats.decode_waits += 1;
        job.take().map(Job::wait)
    }

    /// One upload of streamed texture `name`: its stand-in first, then each
    /// larger resolution.
    fn texture_step(&mut self, world: &mut World, name: &str, room: u64, first: bool) -> Step {
        self.forget_freed_texture(name);
        let Some(t) = self.streaming.textures.get_mut(name) else {
            return Step::Dropped;
        };
        if t.job.is_some() {
            let mut job = t.job.take();
            let decoded = self.take_decoded(&mut job);
            let t = self.streaming.textures.get_mut(name).expect("still there");
            t.job = job;
            match decoded {
                None => return Step::NotReady,
                Some(Err(e)) => {
                    t.state = AssetState::Failed;
                    t.queued = false;
                    self.streaming.fail(ResidencyKind::Texture, name, e);
                    return Step::Dropped;
                }
                Some(Ok((origin, levels))) => {
                    t.origin = Some(origin);
                    t.levels = levels;
                    // Coming back (evicted, or held by the budget): carry on
                    // from the resolution on the GPU.
                    let size = t.size;
                    t.at = t
                        .levels
                        .iter()
                        .position(|l| [l.width, l.height] == size)
                        .unwrap_or(t.levels.len().saturating_sub(1));
                    if t.texture.is_none() {
                        t.state = AssetState::Uploading;
                    }
                }
            }
        }
        let t = self.streaming.textures.get_mut(name).expect("still there");
        if t.levels.is_empty() {
            t.queued = false;
            return Step::Dropped;
        }
        let next = if t.texture.is_none() {
            t.levels.len() - 1
        } else {
            t.at.min(t.levels.len()).saturating_sub(1)
        };
        let bytes = t.levels[next].bytes();
        let current = if t.texture.is_some() && t.at < t.levels.len() {
            t.levels[t.at].bytes()
        } else {
            0
        };
        if !first && bytes > room {
            return Step::NotReady;
        }
        // The texture budget: make room, or stay at this resolution.
        if t.texture.is_some() && !self.texture_fits(world, name, bytes.saturating_sub(current)) {
            let set = self.residency.set(ResidencyKind::Texture);
            let held = (set.budget(), set.resident_bytes(), bytes.saturating_sub(current));
            let t = self.streaming.textures.get_mut(name).expect("still there");
            t.queued = false;
            t.levels.clear();
            t.held = Some(held);
            self.streaming.stats.held_by_budget += 1;
            return Step::Dropped;
        }
        self.upload_texture_level(name, next);
        self.enforce_residency(world, ResidencyKind::Texture);
        let t = &self.streaming.textures[name];
        let again = t.state == AssetState::Streaming;
        if !again && let Some(t) = self.streaming.textures.get_mut(name) {
            t.queued = false;
        }
        Step::Uploaded { bytes, again }
    }

    /// Whether `extra` more texture bytes fit the texture budget, evicting
    /// others (never `name`) to make room.
    fn texture_fits(&mut self, world: &mut World, name: &str, extra: u64) -> bool {
        let set = self.residency.set(ResidencyKind::Texture);
        let Some(budget) = set.budget() else {
            return true;
        };
        if set.resident_bytes() + extra <= budget {
            return true;
        }
        let now = self.residency_now();
        let keep = self.streaming.textures.get(name).and_then(|t| t.texture).map(|t| t.id);
        let shown = self.shown_textures(world);
        // Evict as if the budget were lower by `extra`.
        let set = self.residency.set_mut(ResidencyKind::Texture);
        set.set_budget(Some(budget.saturating_sub(extra)));
        let evicted = set.evict(now, |id| Some(id) == keep || shown.contains(&id));
        set.set_budget(Some(budget));
        self.residency.shown = shown;
        self.drop_to_stand_ins(evicted);
        self.residency.set(ResidencyKind::Texture).resident_bytes() + extra <= budget
    }

    /// Uploads resolution `index` of streamed texture `name` (making the
    /// texture if it has none yet) and updates its state and bytes.
    fn upload_texture_level(&mut self, name: &str, index: usize) {
        let now = self.residency_now();
        let t = self.streaming.textures.get_mut(name).expect("a streamed texture");
        let levels = std::mem::take(&mut t.levels);
        let existing = t.texture;
        if t.stand_in.is_none() {
            let small = levels.last().expect("a level");
            t.stand_in = Some(small.duplicate());
        }
        // The size the game sees: the file's (or the largest the GPU holds).
        let full = (levels[0].width, levels[0].height);
        let level = &levels[index];
        let (w, h) = (level.width, level.height);
        let bytes = level.bytes();
        let id = match existing {
            Some(texture) => {
                self.set_texture_level(texture.id, level);
                texture.id
            }
            None => {
                let texture = self.make_streamed_texture(full, level);
                self.name_streamed_texture(name, texture);
                texture.id
            }
        };
        let t = self.streaming.textures.get_mut(name).expect("a streamed texture");
        t.levels = levels;
        t.at = index;
        t.size = [w, h];
        t.state = if index == 0 {
            // Full size: the other resolutions aren't needed any more.
            t.levels.clear();
            AssetState::Resident
        } else {
            AssetState::Streaming
        };
        let origin = t.origin.clone().unwrap_or(Origin::File(name.into()));
        let set = self.residency.set_mut(ResidencyKind::Texture);
        if set.get(name).is_some() {
            set.set_bytes(name, bytes);
            set.touch(name, now);
        } else {
            set.insert(name, (id, origin), bytes, false, now);
        }
    }

    /// Makes a game texture of `logical` size holding `level`.
    fn make_streamed_texture(&mut self, logical: (u32, u32), level: &Level) -> Texture {
        #[cfg(feature = "compressed-textures")]
        if let Some(blocks) = &level.blocks {
            let small = self.make_compressed_texture(blocks);
            self.textures_live.insert(small.id);
            return Texture::new(small.id, logical.0, logical.1);
        }
        let small = self.make_texture(level.width, level.height, &level.rgba);
        self.textures_live.insert(small.id);
        Texture::new(small.id, logical.0, logical.1)
    }

    fn name_streamed_texture(&mut self, name: &str, texture: Texture) {
        let t = self.streaming.textures.get_mut(name).expect("a streamed texture");
        t.texture = Some(texture);
        let file = match &t.origin {
            Some(Origin::File(file)) => Some(file.clone()),
            _ => None,
        };
        self.streaming.texture_names.insert(texture.id, name.to_string());
        self.texture_names.add(texture.id, name, texture);
        self.asset_ledger
            .own(crate::inventory::AssetClass::Texture, texture.id, name);
        if let Some(file) = file {
            self.watcher
                .watch(name, file, crate::hot_reload::Asset::Texture(texture));
        }
    }

    /// Puts `level` into texture `id`, which may change its resolution.
    fn set_texture_level(&mut self, id: u32, level: &Level) {
        self.asset_ledger.textures.insert(id, level.bytes());
        #[cfg(feature = "compressed-textures")]
        if let Some(blocks) = &level.blocks {
            self.compressed.insert(id, blocks.normal);
            if !self.headless
                && let Some(renderer) = self.renderer.as_mut()
            {
                renderer.replace_compressed_texture(id, blocks);
            }
            return;
        }
        #[cfg(feature = "compressed-textures")]
        self.compressed.remove(&id);
        if !self.headless
            && let Some(renderer) = self.renderer.as_mut()
        {
            renderer.replace_texture(id, level.width, level.height, &level.rgba);
        }
    }

    /// Drops the evicted textures to their stand-ins.
    fn drop_to_stand_ins(&mut self, evicted: Vec<(String, crate::residency::Entry)>) {
        for (name, entry) in evicted {
            if !self.textures_live.contains(entry.handle) {
                continue;
            }
            let Some(t) = self.streaming.textures.get_mut(&name) else {
                continue;
            };
            let Some(stand_in) = t.stand_in.take() else {
                continue;
            };
            t.levels.clear();
            t.job = None;
            t.state = AssetState::Evicted;
            t.size = [stand_in.width, stand_in.height];
            self.set_texture_level(entry.handle, &stand_in);
            self.streaming.textures.get_mut(&name).expect("there").stand_in = Some(stand_in);
        }
    }

    /// Evicts textures to fit the budget (`enforce_residency`).
    pub(crate) fn evict_textures(&mut self, world: &mut World, now: (u64, u64)) {
        let shown = self.shown_textures(world);
        let evicted = self
            .residency
            .set_mut(ResidencyKind::Texture)
            .evict(now, |id| shown.contains(&id));
        self.residency.shown = shown;
        self.drop_to_stand_ins(evicted);
    }

    /// Ids of the textures sprites show, in the reused set taken from
    /// `residency.shown` (put it back after use, so it doesn't allocate).
    fn shown_textures(&mut self, world: &mut World) -> std::collections::HashSet<u32> {
        let mut ids = std::mem::take(&mut self.residency.shown);
        ids.clear();
        world.query::<&crate::d2::Sprite>().for_each(|s| {
            ids.insert(s.texture.id);
        });
        ids
    }

    /// Whether texture `name`, held at a lower resolution by the budget, is
    /// worth decoding again: the budget changed, or the resident bytes did
    /// and its next resolution could now fit (in free room or in what could
    /// be evicted). Otherwise retrying would decode the file only to be
    /// held again.
    fn held_texture_may_fit(&self, name: &str, now: (u64, u64)) -> bool {
        let Some((budget, resident, extra)) = self.streaming.textures.get(name).and_then(|t| t.held) else {
            return true;
        };
        let set = self.residency.set(ResidencyKind::Texture);
        if set.budget() != budget {
            return true;
        }
        if set.resident_bytes() == resident {
            return false;
        }
        let room = set
            .budget()
            .map_or(u64::MAX, |b| b.saturating_sub(set.resident_bytes()));
        room.saturating_add(set.evictable_bytes(now, name)) >= extra
    }

    /// The entry for streamed texture `name`, made if new.
    fn texture_entry(&mut self, name: &str) -> &mut StreamedTexture {
        self.streaming
            .textures
            .entry(name.to_string())
            .or_insert_with(|| StreamedTexture {
                texture: None,
                origin: None,
                levels: Vec::new(),
                at: 0,
                size: [0, 0],
                stand_in: None,
                job: None,
                state: AssetState::Loading,
                queued: false,
                held: None,
            })
    }

    /// Forgets streamed texture `name` if the game freed it.
    fn forget_freed_texture(&mut self, name: &str) {
        let Some(t) = self.streaming.textures.get(name) else {
            return;
        };
        if let Some(texture) = t.texture
            && !self.textures_live.contains(texture.id)
        {
            self.streaming.texture_names.remove(&texture.id);
            self.streaming.textures.remove(name);
            self.residency.set_mut(ResidencyKind::Texture).forget(name);
        }
    }

    /// Reads and decodes texture `name` on this thread.
    fn decode_texture_now(&mut self, name: &str) -> Result<(Origin, Pyramid), crate::error::Error> {
        let found = self.asset_sources.find(name)?;
        let max = self.max_texture_side();
        let gpu = self.texture_formats();
        decode_texture(name, found.origin.clone(), found.into_bytes(), max, gpu)
    }

    /// The block formats streamed textures may stay in.
    #[cfg(feature = "compressed-textures")]
    fn texture_formats(&self) -> TextureFormats {
        self.compressed_formats()
    }

    /// Nothing to choose without the `compressed-textures` feature.
    #[cfg(not(feature = "compressed-textures"))]
    fn texture_formats(&self) -> TextureFormats {
        TextureFormats
    }

    /// Starts decoding texture `name` on a worker, and queues its uploads.
    fn start_texture_decode(&mut self, name: &str) {
        self.streaming
            .errors
            .remove(&(kind_key(ResidencyKind::Texture), name.to_string()));
        let max = self.max_texture_side();
        let gpu = self.texture_formats();
        // Find it here: added sources are read on the game thread.
        let job = match self.asset_sources.find(name) {
            Ok(found) => {
                let origin = found.origin.clone();
                let name_owned = name.to_string();
                match origin {
                    Origin::File(_) => {
                        Job::spawn(move || decode_texture(&name_owned, origin, found.into_bytes(), max, gpu))
                    }
                    Origin::Source { .. } => {
                        let bytes = found.into_bytes();
                        Job::spawn(move || decode_texture(&name_owned, origin, bytes, max, gpu))
                    }
                }
            }
            Err(e) => Job::spawn(move || Err(e)),
        };
        let entry = self.texture_entry(name);
        entry.job = Some(job);
        entry.held = None;
        // One with a texture keeps showing it (and its state) meanwhile.
        if entry.texture.is_none() {
            entry.state = AssetState::Loading;
        }
        if !entry.queued {
            entry.queued = true;
            self.streaming.queue.push_back(Request::Texture(name.to_string()));
        }
    }

    fn max_texture_side(&self) -> u32 {
        if self.headless {
            return u32::MAX;
        }
        self.renderer
            .as_ref()
            .map_or(u32::MAX, crate::render::Renderer::max_texture_size)
    }
}

impl crate::gfx::Graphics {
    /// Texture `id` was freed (`Ctx::free`): forget its stream and its
    /// bytes in the texture budget now, not when its name is next asked for.
    pub(crate) fn forget_streamed_texture_id(&mut self, id: u32) {
        if let Some(name) = self.streaming.texture_names.remove(&id) {
            self.streaming.textures.remove(&name);
            self.residency.set_mut(ResidencyKind::Texture).forget(&name);
        }
    }

    /// Where the streamed or resident asset of `kind` called `name` is.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     assert_eq!(ctx.gfx.asset_state(ponko::streaming::ResidencyKind::Texture, "sky.png"), ponko::streaming::AssetState::Unknown);
    ///     ctx.gfx.stream_texture("sky.png"); // not there: fails on a worker
    ///     ctx.finish_uploads();
    ///     assert_eq!(ctx.gfx.asset_state(ponko::streaming::ResidencyKind::Texture, "sky.png"), ponko::streaming::AssetState::Failed);
    ///     assert_eq!(ctx.gfx.asset_error(ponko::streaming::ResidencyKind::Texture, "sky.png").unwrap().kind(), ErrorKind::NotFound);
    /// });
    /// ```
    #[must_use]
    pub fn asset_state(&self, kind: ResidencyKind, name: &str) -> AssetState {
        match kind {
            ResidencyKind::Texture => match self.streaming.textures.get(name) {
                Some(t) if t.texture.is_some_and(|tex| !self.textures_live.contains(tex.id)) => AssetState::Unknown,
                Some(t) => t.state,
                None => AssetState::Unknown,
            },
            #[cfg(feature = "3d")]
            ResidencyKind::Mesh => self.mesh_state(name),
            #[cfg(not(feature = "3d"))]
            ResidencyKind::Mesh => AssetState::Unknown,
        }
    }

    /// Why loading the asset of `kind` called `name` failed, while it is
    /// [`AssetState::Failed`].
    #[must_use]
    pub fn asset_error(&self, kind: ResidencyKind, name: &str) -> Option<&crate::error::Error> {
        if self.asset_state(kind, name) != AssetState::Failed {
            return None;
        }
        self.streaming.errors.get(&(kind_key(kind), name.to_string()))
    }

    /// Every asset streamed or loaded under residency, by kind then name:
    /// its state, handle id and GPU resolution. For diagnostics (`perf.get`
    /// lists them too).
    #[must_use]
    pub fn streamed_assets(&self) -> Vec<StreamedAsset> {
        let mut out = Vec::new();
        #[cfg(feature = "3d")]
        self.list_streamed_meshes(&mut out);
        for (name, t) in &self.streaming.textures {
            out.push(StreamedAsset {
                kind: ResidencyKind::Texture,
                name: name.clone(),
                state: self.asset_state(ResidencyKind::Texture, name),
                id: t.texture.map(|t| t.id),
                size: t.size,
            });
        }
        out
    }

    /// Sets how many bytes the upload step may send to the GPU each frame;
    /// `None` for no limit. Default [`DEFAULT_UPLOAD_BUDGET`]. A smaller
    /// budget spreads streaming over more frames and keeps each one short;
    /// one upload larger than the budget still goes, alone in its frame.
    pub fn set_upload_budget(&mut self, bytes_per_frame: Option<u64>) {
        self.streaming.budget = bytes_per_frame;
        self.streaming.stats.budget = bytes_per_frame;
    }

    /// With `on`, the upload step waits for each queued asset's decoding
    /// instead of moving on to the next, so every [`AssetState`] changes in
    /// a frame that depends only on the order assets were asked for and the
    /// upload budget: replays, lockstep games and tests see the same
    /// frames each run. Off by default, so frames never wait for a decode;
    /// then the frame an asset arrives in, and the order new textures get
    /// their ids in, follow when decodes finish and can differ between runs.
    ///
    /// States are fixed per frame, not per simulation tick: a frame can run
    /// several ticks or none, so for the same ticks each run, also run the
    /// same frames (as headless runs and fixed frame rates do).
    pub fn set_deterministic_uploads(&mut self, on: bool) {
        self.streaming.deterministic = on;
    }

    /// The upload queue's budget and counters.
    #[must_use]
    pub fn upload_stats(&self) -> UploadStats {
        let mut stats = self.streaming.stats;
        stats.budget = self.streaming.budget;
        stats.pending = self.streaming.pending();
        stats
    }

    /// Texture `id` was given pixels at its full size (`update_texture`, hot
    /// reload): if streamed, it is resident at full resolution now.
    pub(crate) fn streamed_texture_restored(&mut self, id: u32) {
        let Some(name) = self.streaming.texture_names.get(&id).cloned() else {
            return;
        };
        let now = (self.frame, self.sim_tick);
        let Some(t) = self.streaming.textures.get_mut(&name) else {
            return;
        };
        let Some(texture) = t.texture else {
            return;
        };
        t.size = [texture.width(), texture.height()];
        t.levels.clear();
        t.job = None;
        t.at = 0;
        t.state = AssetState::Resident;
        let bytes = self
            .asset_ledger
            .textures
            .get(&id)
            .copied()
            .unwrap_or_else(|| crate::inventory::rgba8_bytes(texture.width(), texture.height()));
        let set = self.residency.set_mut(ResidencyKind::Texture);
        if set.get(&name).is_some() {
            set.set_bytes(&name, bytes);
        } else {
            let origin = self.streaming.textures[&name]
                .origin
                .clone()
                .unwrap_or(Origin::File(name.clone().into()));
            set.insert(&name, (id, origin), bytes, false, now);
        }
    }
}

/// The block formats a streamed texture may stay in (nothing to choose
/// without the `compressed-textures` feature).
#[cfg(feature = "compressed-textures")]
type TextureFormats = crate::compressed::GpuFormats;
#[cfg(not(feature = "compressed-textures"))]
#[derive(Clone, Copy)]
struct TextureFormats;

/// Decodes texture `name` from `bytes` into its resolutions: a KTX2 or DDS
/// file in a block format `gpu` samples into its own mip levels.
fn decode_texture(
    name: &str,
    origin: Origin,
    bytes: Result<Vec<u8>, crate::error::Error>,
    max: u32,
    gpu: TextureFormats,
) -> Result<(Origin, Pyramid), crate::error::Error> {
    let bytes = bytes.map_err(|e| e.with_path(name))?;
    let what = origin.describe(name);
    #[cfg(feature = "compressed-textures")]
    if let Some(decoded) = crate::compressed::decode(&bytes, name, &what, gpu) {
        return Ok(match decoded? {
            crate::compressed::Decoded::Rgba(image) => (origin, pyramid(image, max)),
            crate::compressed::Decoded::Blocks(image) => {
                let levels = block_pyramid(&image, max);
                if levels.is_empty() {
                    return Err(crate::error::Error::new(
                        crate::error::ErrorKind::Limit,
                        format!("{what} is larger than this GPU's textures and has no smaller mip level to use"),
                    )
                    .with_path(name));
                }
                (origin, levels)
            }
        });
    }
    #[cfg(not(feature = "compressed-textures"))]
    let TextureFormats = gpu;
    let image = assets::decode_image(&bytes, name, &what)?;
    Ok((origin, pyramid(image, max)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> assets::Image {
        assets::Image {
            width: w,
            height: h,
            rgba: vec![128; (w * h * 4) as usize],
        }
    }

    #[test]
    fn pyramid_halves_down_to_the_stand_in() {
        let p = pyramid(image(256, 64), u32::MAX);
        let sizes: Vec<(u32, u32)> = p.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(sizes, [(256, 64), (128, 32), (64, 16), (32, 8)]);
        assert!(p.iter().all(|l| l.rgba.len() == (l.width * l.height * 4) as usize));
    }

    #[test]
    fn pyramid_skips_levels_larger_than_the_gpu_allows() {
        let p = pyramid(image(1024, 1024), 256);
        assert_eq!((p[0].width, p[0].height), (256, 256));
        assert_eq!(p.last().map(|l| l.width), Some(32));
    }

    #[test]
    fn small_images_are_their_own_stand_in() {
        let p = pyramid(image(3, 5), u32::MAX);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn halving_averages_blocks() {
        let level = Level::new(
            2,
            2,
            [
                [0, 0, 0, 255],
                [255, 255, 255, 255],
                [0, 0, 0, 255],
                [255, 255, 255, 255],
            ]
            .concat(),
        );
        let half = halve(&level);
        assert_eq!((half.width, half.height), (1, 1));
        assert_eq!(half.rgba, [128, 128, 128, 255]);
    }
}
