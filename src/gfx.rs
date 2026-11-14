//! [`Graphics`]: drawing and everything that lives on the GPU, as `ctx.gfx`.
//!
//! **What lives here.** The frame's draw queues (sprites, text, shapes),
//! textures and fonts, screenshots, and with the `3d` feature the meshes,
//! materials, shaders, effects, compute shaders, GPU buffers, instance
//! batches, terrain, render targets, texture arrays and reflection probes.
//! The cameras, lights, sky and post-processing settings are plain fields of
//! [`Ctx`](crate::ctx::Ctx) (`ctx.scene3d.camera`, `ctx.scene3d.lighting`, `ctx.scene3d.post`): the
//! renderer reads them when the frame is drawn.
//!
//! **When to call it.** Load and create in setup (or any system: a file is
//! read once, then cached by name). Draw calls queue work for the current
//! frame, so call them every frame from a frame system or `.draw(...)`; a
//! draw queued in a simulation system is drawn once per step. Handles
//! (`Texture`, `Mesh`, `Material`, ...) are small `Copy` ids: keep them in
//! components and resources, and free them with
//! [`Ctx::free`](crate::ctx::Ctx::free).
//!
//! **What it needs.** A window for anything to appear. In a headless run
//! (tests, servers) loading and creating still give working handles with
//! the right sizes, draws are dropped, and nothing touches a GPU. It never
//! needs the world: the few calls that read or write entities
//! (`write_animated_morphs`, the reflection probe calls) take `&mut World`
//! explicitly, so `ctx.gfx` borrows apart from `ctx.world`.
//!
//! ```
//! use ponko::prelude::*;
//!
//! #[derive(Component)]
//! struct Crab(Texture);
//!
//! fn draw(ctx: &mut Ctx) {
//!     // `ctx.gfx` is its own field: draw while a query borrows the world.
//!     for (crab, at) in ctx.world.query::<(&Crab, &ponko::d2::Transform)>().iter() {
//!         ctx.gfx.draw(crab.0, at.position);
//!     }
//! }
//!
//! Game::new("gfx", 320, 240).sim_system(draw).run_headless(1, |ctx| {
//!     let pixel = ctx.gfx.create_texture(1, 1, &[255; 4]);
//!     ctx.world.spawn((Crab(pixel), ponko::d2::Transform::at(vec2(10.0, 20.0))));
//! });
//! ```

use glam::Vec2;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use crate::asset_cache::{AssetCache, AssetKind};
use crate::assets;
use crate::camera::Rect;
use crate::color::Color;
use crate::ctx::region;
use crate::hot_reload::Asset;
use crate::render::Renderer;
use crate::render::batch::{DrawList, SpriteInstance, uv_region};
use crate::text::{Font, Text, TextParams, TextShaping};
use crate::texture::{DrawParams, Texture, TextureFilter};

/// A screenshot someone asked for: where to save it, and who to tell.
pub(crate) struct ScreenshotRequest {
    pub(crate) path: PathBuf,
    /// Scale down so neither side exceeds this (0: full size).
    pub(crate) max_size: u32,
    pub(crate) reply: Option<Sender<Result<Value, String>>>,
    /// The file the control endpoint claimed for it, written through this
    /// handle (and removed if the screenshot fails).
    pub(crate) claimed: Option<crate::control::ClaimedFile>,
}

/// Side of the circle texture behind [`Graphics::draw_circle`], in pixels.
const CIRCLE: u32 = 128;

/// Which queue a shape goes to: the world (under the camera) or the screen.
#[derive(Clone, Copy)]
enum Layer {
    World,
    Screen,
}

/// Drawing and GPU assets: `ctx.gfx`.
///
/// `ctx.draw`, `ctx.draw_text` and `ctx.load_texture` are shortcuts for
/// [`draw`](Self::draw), [`draw_text`](Self::draw_text) and
/// [`load_texture`](Self::load_texture); everything else is here.
///
/// - **What lives here:** the frame's draw queues (sprites, text, shapes),
///   textures, fonts and screenshots, and with the `3d` feature meshes,
///   materials, shaders, effects, compute shaders, GPU buffers, instance
///   batches, terrain, render targets, texture arrays and reflection
///   probes. Cameras, lights, sky and post-processing are plain fields of
///   [`Ctx`](crate::ctx::Ctx).
/// - **When to call it:** load and create in setup (or any system: a file
///   is read once, then cached by name). Draw calls queue work for the
///   current frame, so make them every frame from a frame system; a draw
///   queued in a simulation system is drawn once per step.
/// - **What it needs:** a window for anything to appear. Headless, loading
///   and creating still give working handles with the right sizes, draws
///   are dropped and nothing touches a GPU. The few calls that read or
///   write entities take `&mut World`, so `ctx.gfx` borrows apart from
///   `ctx.world`.
///
/// ```
/// use ponko::prelude::*;
///
/// #[derive(Component)]
/// struct Crab(Texture);
///
/// fn draw(ctx: &mut Ctx) {
///     // `ctx.gfx` is its own field: draw while a query borrows the world.
///     for (crab, at) in ctx.world.query::<(&Crab, &ponko::d2::Transform)>().iter() {
///         ctx.gfx.draw(crab.0, at.position);
///     }
/// }
///
/// Game::new("gfx", 320, 240).sim_system(draw).run_headless(1, |ctx| {
///     let pixel = ctx.gfx.create_texture(1, 1, &[255; 4]);
///     ctx.world.spawn((Crab(pixel), ponko::d2::Transform::at(vec2(10.0, 20.0))));
/// });
/// ```
pub struct Graphics {
    /// Assets loaded under residency budgets (`resident_mesh`,
    /// `resident_texture`), by kind. Calls that can evict take the world,
    /// since eviction never takes what an entity shows.
    pub(crate) residency: crate::residency::Residency,
    /// The upload queue and streamed assets (`stream_texture`, `stream_mesh`).
    pub(crate) streaming: crate::streaming::Streaming,
    /// The simulation tick when the running system started (residency
    /// orders assets by the frame and tick they were last used in).
    pub(crate) sim_tick: u64,
    /// No window and no GPU: handles are made up, draws are dropped.
    pub(crate) headless: bool,
    /// The frame being built (for shader status reports).
    pub(crate) frame: u64,
    /// Texture ids handed out while headless (no GPU to allocate them).
    pub(crate) headless_textures: u32,
    /// The textures the game made and hasn't freed.
    pub(crate) textures_live: crate::handle::LiveSet,
    /// What the live assets cost (see `Ctx::asset_inventory`).
    pub(crate) asset_ledger: crate::inventory::Ledger,
    /// Shared with the other services that load files.
    pub(crate) asset_sources: assets::SharedSources,
    /// Hot reload's watch list, shared the same way.
    pub(crate) watcher: crate::hot_reload::SharedWatcher,
    /// Textures loaded from files, by where they came from.
    pub(crate) loaded_textures: AssetCache<Texture>,
    /// Fonts loaded from files, by where they came from.
    pub(crate) loaded_fonts: AssetCache<Font>,
    /// Texture file names for scene saves, shared with the world's copy.
    pub(crate) texture_names: crate::reflect::TextureNames,
    /// Mesh file names, the same way.
    #[cfg(feature = "3d")]
    pub(crate) mesh_names: crate::reflect::MeshNames,
    /// The materials of loaded mesh files, the same way.
    #[cfg(feature = "3d")]
    pub(crate) material_names: crate::reflect::MaterialNames,
    pub(crate) renderer: Option<Renderer>,
    /// Sprite shaders and materials.
    pub(crate) sprites: crate::sprite_material::SpriteAssets,
    /// 2D lighting: the ambient light, normal maps and this frame's lights.
    pub(crate) lights2d: crate::light2d::Lighting2D,
    /// The latest compile of each of the game's shaders (`Graphics::shader_status`).
    pub(crate) shader_statuses: Vec<crate::render::ShaderStatus>,
    pub(crate) screenshots: Vec<ScreenshotRequest>,
    pub(crate) draw_list: DrawList,
    pub(crate) ui_list: DrawList,
    pub(crate) clear_color: Color,
    pub(crate) text: Text,
    /// Reused for a text field's selection spans.
    pub(crate) selection_scratch: Vec<[f32; 2]>,
    font_atlas: Option<Texture>,
    pub(crate) scale_factor: f32,
    /// A 1x1 white texture, for rectangles and lines.
    white: Option<Texture>,
    /// A white circle with a soft edge, for circles.
    circle: Option<Texture>,
    #[cfg(feature = "3d")]
    pub(crate) assets3d: crate::three::Assets3D,
    #[cfg(feature = "3d")]
    pub(crate) frame3d: crate::three::Frame3D,
    /// Automatic quality's running controller, following the policy in
    /// `Ctx::scene3d` (`Ctx::update_quality_autotune`).
    #[cfg(feature = "3d")]
    pub(crate) autotune: Option<crate::three::autotune::Controller>,
    /// The textures in a block-compressed format on the GPU, and where a
    /// two-channel normal map keeps its X and Y (materials rebuild Z).
    #[cfg(feature = "compressed-textures")]
    pub(crate) compressed: std::collections::HashMap<u32, crate::compressed::NormalChannels>,
}

impl Graphics {
    pub(crate) fn new(
        asset_sources: assets::SharedSources,
        watcher: crate::hot_reload::SharedWatcher,
        texture_names: crate::reflect::TextureNames,
        #[cfg(feature = "3d")] mesh_names: crate::reflect::MeshNames,
        #[cfg(feature = "3d")] material_names: crate::reflect::MaterialNames,
    ) -> Self {
        Self {
            residency: Default::default(),
            streaming: Default::default(),
            sim_tick: 0,
            headless: false,
            frame: 0,
            headless_textures: 0,
            textures_live: Default::default(),
            asset_ledger: Default::default(),
            asset_sources,
            watcher,
            loaded_textures: AssetCache::default(),
            loaded_fonts: AssetCache::default(),
            texture_names,
            #[cfg(feature = "3d")]
            mesh_names,
            #[cfg(feature = "3d")]
            material_names,
            renderer: None,
            sprites: Default::default(),
            lights2d: Default::default(),
            shader_statuses: Vec::new(),
            screenshots: Vec::new(),
            draw_list: DrawList::default(),
            ui_list: DrawList::default(),
            clear_color: Color::PONKO_DARK,
            text: Text::new(),
            selection_scratch: Vec::new(),
            font_atlas: None,
            scale_factor: 1.0,
            white: None,
            circle: None,
            #[cfg(feature = "3d")]
            assets3d: Default::default(),
            #[cfg(feature = "3d")]
            frame3d: Default::default(),
            #[cfg(feature = "3d")]
            autotune: None,
            #[cfg(feature = "compressed-textures")]
            compressed: Default::default(),
        }
    }

    /// Sets the background color. It stays until you change it. In 3D it
    /// shows only behind a flat sky (`ctx.scene3d.sky.atmosphere` off): the
    /// default atmosphere draws the whole sky itself.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// ctx.gfx.clear(Color::hex(0x203040));
    /// # }
    /// ```
    pub fn clear(&mut self, color: Color) {
        self.clear_color = color;
    }

    /// Draws `texture` centered at `pos`, in world units (window pixels until
    /// you move or zoom [`camera`](crate::ctx::Ctx::camera)).
    ///
    /// Drawing is cheap: calls are collected and sent to the GPU in big
    /// batches at the end of the frame. Call it every frame for everything
    /// you want visible.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, crab: Texture) {
    /// ctx.gfx.draw(crab, vec2(400.0, 300.0));
    /// # }
    /// ```
    pub fn draw(&mut self, texture: Texture, pos: Vec2) {
        push_sprite(
            &mut self.draw_list,
            texture,
            pos,
            Vec2::ONE,
            0.0,
            0.0,
            Vec2::splat(0.5),
            [255; 4],
            None,
            0,
        );
    }

    /// Draws `texture` with rotation, scale, tint, draw order or pivot.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, crab: Texture) {
    /// let params = DrawParams::default()
    ///     .with_rotation(ctx.time.elapsed() as f32)
    ///     .with_color(Color::RED.with_alpha(0.5))
    ///     .with_z(1.0); // on top of everything at z = 0
    /// ctx.gfx.draw_ex(crab, vec2(400.0, 300.0), params);
    /// # }
    /// ```
    pub fn draw_ex(&mut self, texture: Texture, pos: Vec2, params: DrawParams) {
        push_sprite(
            if params.screen {
                &mut self.ui_list
            } else {
                &mut self.draw_list
            },
            texture,
            pos,
            params.scale,
            params.rotation,
            params.z,
            params.pivot,
            params.color.to_rgba8(),
            params.region,
            crate::sprite_material::style_key(params.blend, params.material),
        );
    }

    /// Draws `text` with its top-left corner at `pos`, in window pixels (on
    /// top of the world, ignoring the camera). `size` is the line height in
    /// pixels. `\n` starts a new line.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn hud(ctx: &mut Ctx) {
    /// let fps = ctx.time.fps();
    /// ctx.gfx.draw_text(&format!("{fps:.0} FPS"), vec2(10.0, 10.0), 18.0, Color::WHITE);
    /// # }
    /// ```
    pub fn draw_text(&mut self, text: &str, pos: Vec2, size: f32, color: Color) {
        self.draw_text_ex(
            text,
            pos,
            TextParams {
                size,
                color,
                ..Default::default()
            },
        );
    }

    /// [`draw_text`](Self::draw_text) with a font and draw order.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn hud(ctx: &mut Ctx, title: Font) {
    /// let params = TextParams::default().with_font(title).with_size(40.0).with_z(1.0);
    /// ctx.gfx.draw_text_ex("Colony", vec2(20.0, 20.0), params);
    /// # }
    /// ```
    pub fn draw_text_ex(&mut self, text: &str, pos: Vec2, params: TextParams) {
        if self.headless || text.is_empty() {
            return;
        }
        let atlas = match self.font_atlas {
            Some(t) => t,
            None => {
                let side = Text::ATLAS_SIZE;
                let t = self.make_texture(side, side, &vec![0; (side * side * 4) as usize]);
                self.font_atlas = Some(t);
                t
            }
        };
        let scale = self.scale_factor;
        self.text.layout(text, params.font, params.size * scale, true);
        if !self.text.uploads.is_empty()
            && let Some(renderer) = self.renderer.as_mut()
        {
            for (at, size, rgba) in self.text.uploads.drain(..) {
                renderer.update_texture_region(atlas.id, at, size, &rgba);
            }
        }
        let side = Text::ATLAS_SIZE as f32;
        let color = params.color.to_rgba8();
        for p in self.text.placed() {
            let Some(at) = p.glyph.at else { continue };
            let size = Vec2::new(p.glyph.size[0] as f32, p.glyph.size[1] as f32);
            let uv = uv_region([at[0] as f32 / side, at[1] as f32 / side], (size / side).into());
            self.ui_list.push(
                atlas.id,
                params.z,
                SpriteInstance::new(
                    (pos + p.pos / scale).into(),
                    (size / scale).into(),
                    [0.0, 0.0],
                    0.0,
                    color,
                )
                .with_uv(uv),
            );
        }
    }

    /// The size `text` would take with [`draw_text`](Self::draw_text) at
    /// `size`, in window pixels. Works headless too.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn centered(ctx: &mut Ctx) {
    /// let label = "Paused";
    /// let size = ctx.gfx.measure_text(label, 32.0);
    /// let at = (ctx.window_size() - size) / 2.0;
    /// ctx.gfx.draw_text(label, at, 32.0, Color::WHITE);
    /// # }
    /// ```
    pub fn measure_text(&mut self, text: &str, size: f32) -> Vec2 {
        self.measure_text_ex(text, Font::DEFAULT, size)
    }

    /// [`measure_text`](Self::measure_text) for another font.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, title: Font) {
    /// let width = ctx.gfx.measure_text_ex("Colony", title, 40.0).x;
    /// # }
    /// ```
    pub fn measure_text_ex(&mut self, text: &str, font: Font, size: f32) -> Vec2 {
        let scale = self.scale_factor;
        self.text.layout(text, font, size * scale, false) / scale
    }

    /// Sets how `texture` is sampled. Use [`TextureFilter::Nearest`] for pixel
    /// art and tilesets; the default is smooth.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let tiles = ctx.gfx.load_texture("crab.png");
    /// ctx.gfx.set_texture_filter(tiles, TextureFilter::Nearest);
    /// # }
    /// ```
    pub fn set_texture_filter(&mut self, texture: Texture, filter: TextureFilter) {
        if !self.textures_live.contains(texture.id) {
            return;
        }
        if let Some(r) = self.renderer.as_mut() {
            r.set_filter(texture.id, filter == TextureFilter::Nearest);
        }
    }

    /// Loads a TrueType or OpenType font from the `assets/` folder.
    /// Loading the same file again returns the same handle.
    ///
    /// # Panics
    /// With a clear message if the file is missing or isn't a font.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let title = ctx.gfx.load_font("fonts/Title.ttf");
    /// # let _ = title;
    /// # }
    /// ```
    pub fn load_font(&mut self, path: &str) -> Font {
        self.try_load_font(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_font`](Self::load_font) with errors as values:
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound),
    /// [`Io`](crate::error::ErrorKind::Io), or [`Decode`](crate::error::ErrorKind::Decode)
    /// if it isn't a font.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let title = ctx.gfx.try_load_font("fonts/Fancy.ttf").unwrap_or(Font::DEFAULT);
    ///     # let _ = title;
    /// });
    /// ```
    pub fn try_load_font(&mut self, path: &str) -> Result<Font, crate::error::Error> {
        let found = match self.loaded_fonts.lookup(&self.asset_sources, path, AssetKind::Font)? {
            assets::Lookup::Cached(_, font) => return Ok(font),
            assets::Lookup::Found(found) => found,
        };
        let origin = found.origin.clone();
        let bytes = found.into_bytes().map_err(|e| e.with_path(path))?;
        let font = self.text.add_font(&bytes).map_err(|e| {
            let what = match &origin {
                assets::Origin::File(file) => file.display().to_string(),
                assets::Origin::Source { .. } => origin.describe(path),
            };
            crate::error::Error::decode(path, format!("{what}: {}", e.message()))
        })?;
        self.loaded_fonts.insert(origin.clone(), AssetKind::Font, font);
        if let assets::Origin::File(file) = origin {
            self.watcher.watch(path, file, Asset::Font(font.0));
        }
        Ok(font)
    }

    /// Sets the fonts tried, in order, for characters `font` has no glyph
    /// for: a game localized into Arabic, Hindi and Japanese gives its UI
    /// font one font per script. Replaces the chain set before; an empty
    /// one turns fallback off. A freed font in the chain is skipped.
    ///
    /// Fallback works with or without the `text-shaping` feature: without
    /// it each character is drawn on its own, which is right for scripts
    /// whose letters don't join (Japanese, Chinese, Korean); Arabic and
    /// Devanagari need shaping.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let arabic = ctx.gfx.load_font("fonts/NotoSansArabic.ttf");
    /// let japanese = ctx.gfx.load_font("fonts/NotoSansJP.otf");
    /// ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[arabic, japanese]);
    /// ctx.gfx.draw_text("スコア: 120", vec2(16.0, 16.0), 24.0, Color::WHITE);
    /// # }
    /// ```
    pub fn set_font_fallbacks(&mut self, font: Font, fallbacks: &[Font]) {
        self.text.set_fallbacks(font, fallbacks);
    }

    /// `font`'s fallback chain (see
    /// [`set_font_fallbacks`](Self::set_font_fallbacks)), with the system
    /// fonts found for it.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     assert!(ctx.gfx.font_fallbacks(Font::DEFAULT).is_empty());
    /// });
    /// ```
    pub fn font_fallbacks(&self, font: Font) -> &[Font] {
        self.text.fallbacks(font)
    }

    /// Sets which strings are shaped with the `text-shaping` feature (see
    /// [`TextShaping`]). Without the feature nothing is shaped.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// ctx.gfx.set_text_shaping(TextShaping::Always);
    /// assert_eq!(ctx.gfx.text_shaping(), TextShaping::Always);
    /// # }
    /// ```
    pub fn set_text_shaping(&mut self, shaping: TextShaping) {
        self.text.set_shaping(shaping);
    }

    /// Which strings are shaped; [`TextShaping::Auto`] unless set.
    pub fn text_shaping(&self) -> TextShaping {
        self.text.shaping()
    }

    /// Loads a font installed on the computer by family name ("Noto Sans
    /// Arabic", "Arial"), its regular face if it has one. Needs the
    /// `system-fonts` feature and a desktop system; the font folders are
    /// read on the first call. Loading the same family again returns the
    /// same handle.
    ///
    /// # Panics
    /// With a clear message if there is no such font, or no system fonts.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let arial = ctx.gfx.load_system_font("Arial");
    /// # let _ = arial;
    /// # }
    /// ```
    pub fn load_system_font(&mut self, family: &str) -> Font {
        self.try_load_system_font(family).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_system_font`](Self::load_system_font) with errors as values:
    /// [`NotFound`](crate::error::ErrorKind::NotFound) if no installed font
    /// has that family,
    /// [`Unsupported`](crate::error::ErrorKind::Unsupported) without the
    /// `system-fonts` feature and on the web.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let ui = ctx.gfx.try_load_system_font("Noto Sans").unwrap_or(Font::DEFAULT);
    ///     # let _ = ui;
    /// });
    /// ```
    pub fn try_load_system_font(&mut self, family: &str) -> Result<Font, crate::error::Error> {
        self.text.system_font(family)
    }

    /// With `on`, a character that neither a font nor its fallback chain
    /// has is looked up among the computer's fonts; the first one with a
    /// glyph for it is loaded and appended to the chain (feature
    /// `system-fonts`, desktop). For text the game can't know in advance,
    /// such as player names in chat. Each character is searched once; the
    /// first search reads the font folders. Returns whether system fonts
    /// can be searched in this build.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// if !ctx.gfx.set_system_font_fallback(true) {
    ///     // Ship fonts for every language and chain them instead.
    /// }
    /// # }
    /// ```
    pub fn set_system_font_fallback(&mut self, on: bool) -> bool {
        self.text.system_fallback(on)
    }

    /// The caret's x in window pixels before byte `index` of the one-line
    /// `text` drawn at `size` (see [`Text::caret_x`]).
    pub(crate) fn caret_x(&mut self, text: &str, font: Font, size: f32, index: usize) -> f32 {
        let scale = self.scale_factor;
        self.text.caret_x(text, font, size * scale, index) / scale
    }

    /// The window-pixel spans that show bytes `lo..hi` of `text` selected
    /// (see [`Text::selection`]).
    pub(crate) fn selection_spans(
        &mut self,
        text: &str,
        font: Font,
        size: f32,
        lo: usize,
        hi: usize,
        out: &mut Vec<[f32; 2]>,
    ) {
        let scale = self.scale_factor;
        self.text.selection(text, font, size * scale, lo, hi, out);
        for span in out {
            *span = [span[0] / scale, span[1] / scale];
        }
    }

    /// Loads an image from the `assets/` folder: PNG, and JPEG with the
    /// default `image-jpeg` feature. WebP, BMP, TGA, GIF, TIFF, ICO, QOI,
    /// PNM, DDS, HDR and EXR each have an opt-in `image-*` cargo feature
    /// (`image-formats` turns them all on), none of which needs `3d`. The
    /// format is found from the file's first bytes, not its extension (TGA,
    /// which has none, from the extension).
    ///
    /// Do this once in setup and keep the [`Texture`]; it's a cheap handle.
    /// Loading the same file again returns the same texture (use
    /// [`create_texture`](Self::create_texture) for a copy of your own).
    ///
    /// # Panics
    /// With a clear message if the file is missing, isn't a valid image, or is
    /// in a format the build doesn't read (the message names the feature to turn on).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Crabs", 800, 600).run(|ctx| {
    ///     let crab = ctx.gfx.load_texture("crab.png");
    ///     move |ctx: &mut Ctx| ctx.gfx.draw(crab, vec2(400.0, 300.0))
    /// });
    /// ```
    pub fn load_texture(&mut self, path: &str) -> Texture {
        self.try_load_texture(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_texture`](Self::load_texture) with errors as values, for
    /// optional assets: [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound)
    /// if no source has it, [`Io`](crate::error::ErrorKind::Io) if it can't be
    /// read, [`Decode`](crate::error::ErrorKind::Decode) if it isn't an image,
    /// [`Unsupported`](crate::error::ErrorKind::Unsupported) if it is an image in
    /// a format this build leaves out (the message names the `image-*`
    /// feature to enable). Like `load_texture`, it also names the texture, so
    /// sprites showing it can be saved.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     // A missing optional skin is not a bug: fall back to no skin.
    ///     let skin: Option<Texture> = ctx.gfx.try_load_texture("skins/gold.png").ok();
    ///     assert!(skin.is_none());
    /// });
    /// ```
    pub fn try_load_texture(&mut self, path: &str) -> Result<Texture, crate::error::Error> {
        let texture = match self
            .loaded_textures
            .lookup(&self.asset_sources, path, AssetKind::Texture)?
        {
            assets::Lookup::Cached(_, texture) => texture,
            assets::Lookup::Found(found) => {
                // Acquire, decode (CPU only), then create on the GPU (game thread).
                let origin = found.origin.clone();
                let bytes = found.into_bytes().map_err(|e| e.with_path(path))?;
                let texture = self.texture_from_file(&bytes, path, &origin.describe(path))?;
                self.asset_ledger
                    .own(crate::inventory::AssetClass::Texture, texture.id, path);
                self.loaded_textures.insert(origin.clone(), AssetKind::Texture, texture);
                if let assets::Origin::File(file) = origin {
                    self.watcher.watch(path, file, Asset::Texture(texture));
                }
                texture
            }
        };
        self.texture_names.add(texture.id, path, texture);
        Ok(texture)
    }

    /// Decodes a texture file's bytes and makes a game texture of it: a
    /// block-compressed one for KTX2 and DDS files the GPU samples (feature
    /// `compressed-textures`), RGBA8 otherwise.
    pub(crate) fn texture_from_file(
        &mut self,
        bytes: &[u8],
        path: &str,
        what: &str,
    ) -> Result<Texture, crate::error::Error> {
        #[cfg(feature = "compressed-textures")]
        if let Some(decoded) = crate::compressed::decode(bytes, path, what, self.compressed_formats()) {
            return Ok(match decoded? {
                crate::compressed::Decoded::Rgba(image) => self.create_texture(image.width, image.height, &image.rgba),
                crate::compressed::Decoded::Blocks(image) => {
                    let texture = self.make_compressed_texture(&image);
                    self.textures_live.insert(texture.id);
                    texture
                }
            });
        }
        let image = assets::decode_image(bytes, path, what)?;
        Ok(self.create_texture(image.width, image.height, &image.rgba))
    }

    /// Hot reload of texture file `file` (asset `name`) into `texture` when
    /// it is a KTX2 or DDS file; `None` for any other file. The size must
    /// stay the same.
    #[cfg(feature = "compressed-textures")]
    pub(crate) fn reload_compressed_texture(
        &mut self,
        texture: Texture,
        file: &std::path::Path,
        name: &str,
    ) -> Option<Result<(), String>> {
        let bytes = assets::read(file).ok()?;
        let decoded = crate::compressed::decode(&bytes, name, &format!("\"{name}\""), self.compressed_formats())?;
        let decoded = match decoded {
            Ok(decoded) => decoded,
            Err(e) => return Some(Err(format!("could not reload {name}: {e}"))),
        };
        let (w, h) = match &decoded {
            crate::compressed::Decoded::Rgba(image) => (image.width, image.height),
            crate::compressed::Decoded::Blocks(image) => (image.width, image.height),
        };
        if (w, h) != (texture.width(), texture.height()) {
            return Some(Err(format!(
                "{name} is now {w} x {h} (was {} x {}); restart the game to use the new size",
                texture.width(),
                texture.height()
            )));
        }
        match decoded {
            crate::compressed::Decoded::Rgba(image) => self.update_texture(texture, &image.rgba),
            crate::compressed::Decoded::Blocks(image) => {
                if self.textures_live.contains(texture.id) {
                    self.asset_ledger.textures.insert(texture.id, image.bytes());
                    self.compressed.insert(texture.id, image.normal);
                    if !self.headless {
                        self.renderer().replace_compressed_texture(texture.id, &image);
                    }
                    self.streamed_texture_restored(texture.id);
                }
            }
        }
        Some(Ok(()))
    }

    /// The block-compressed formats textures can upload in: the GPU's, or
    /// every one headless (nothing is uploaded, so nothing is decoded).
    #[cfg(feature = "compressed-textures")]
    pub(crate) fn compressed_formats(&self) -> crate::compressed::GpuFormats {
        match &self.renderer {
            Some(renderer) if !self.headless => renderer.compressed_formats(),
            _ => crate::compressed::GpuFormats::ALL,
        }
    }

    /// [`make_texture`](Self::make_texture) for a block-compressed image
    /// (whole blocks, in a format the GPU samples).
    #[cfg(feature = "compressed-textures")]
    pub(crate) fn make_compressed_texture(&mut self, image: &crate::compressed::CompressedImage) -> Texture {
        let (width, height) = (image.width, image.height);
        let bytes = image.bytes();
        let id = if self.headless {
            self.headless_textures += 1;
            self.headless_textures
        } else {
            let renderer = self.renderer();
            let max = renderer.max_texture_size();
            assert!(
                width <= max && height <= max,
                "ponko: {width} x {height} is larger than this GPU's maximum texture size ({max})"
            );
            renderer.create_compressed_texture(image)
        };
        self.asset_ledger.textures.insert(id, bytes);
        self.asset_ledger.made(crate::inventory::AssetClass::Texture, id);
        self.compressed.insert(id, image.normal);
        Texture::new(id, width, height)
    }

    /// Creates a texture from raw RGBA pixels (4 bytes per pixel, row by row,
    /// top row first). Handy for generated images.
    ///
    /// # Panics
    /// If `rgba.len()` isn't `width * height * 4`, or the size is 0 or larger
    /// than the GPU supports.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// // A 2x2 checkerboard.
    /// let w = [255, 255, 255, 255];
    /// let b = [0, 0, 0, 255];
    /// let pixels = [w, b, b, w].concat();
    /// let checker = ctx.gfx.create_texture(2, 2, &pixels);
    /// # }
    /// ```
    pub fn create_texture(&mut self, width: u32, height: u32, rgba: &[u8]) -> Texture {
        let texture = self.make_texture(width, height, rgba);
        self.textures_live.insert(texture.id);
        texture
    }

    /// [`create_texture`](Self::create_texture) for the engine's own
    /// textures (UI shapes, glyph atlases): not the game's to free, and not
    /// counted by [`live_counts`](crate::ctx::Ctx::live_counts).
    pub(crate) fn make_texture(&mut self, width: u32, height: u32, rgba: &[u8]) -> Texture {
        assert!(
            width > 0 && height > 0,
            "ponko: texture size must not be zero (got {width} x {height})"
        );
        let expected = width as usize * height as usize * 4;
        assert_eq!(
            rgba.len(),
            expected,
            "ponko: a {width} x {height} texture needs {expected} bytes of RGBA, got {}",
            rgba.len()
        );
        let bytes = crate::inventory::rgba8_bytes(width, height);
        if self.headless {
            // No GPU: a handle that remembers its size, so game logic still works.
            self.headless_textures += 1;
            self.asset_ledger.textures.insert(self.headless_textures, bytes);
            self.asset_ledger
                .made(crate::inventory::AssetClass::Texture, self.headless_textures);
            return Texture::new(self.headless_textures, width, height);
        }
        let renderer = self.renderer();
        let max = renderer.max_texture_size();
        assert!(
            width <= max && height <= max,
            "ponko: {width} x {height} is larger than this GPU's maximum texture size ({max})"
        );
        let id = renderer.create_texture(width, height, rgba);
        self.asset_ledger.textures.insert(id, bytes);
        self.asset_ledger.made(crate::inventory::AssetClass::Texture, id);
        Texture::new(id, width, height)
    }

    /// Frees game texture `id` (see [`Ctx::free`]): what still shows it
    /// sees the renderer's white stand-in.
    pub(crate) fn free_texture_id(&mut self, id: u32) {
        if !self.textures_live.remove(id) {
            return;
        }
        self.asset_ledger.textures.remove(&id);
        self.asset_ledger.freed(crate::inventory::AssetClass::Texture, id);
        #[cfg(feature = "compressed-textures")]
        self.compressed.remove(&id);
        #[cfg(feature = "3d")]
        self.asset_ledger.freed(crate::inventory::AssetClass::RenderTarget, id);
        self.forget_streamed_texture_id(id);
        self.loaded_textures.release(&[AssetKind::Texture], |t| t.id == id);
        self.watcher.forget(|a| matches!(a, Asset::Texture(t) if t.id == id));
        self.texture_names.remove(id);
        #[cfg(feature = "3d")]
        {
            self.assets3d.render_targets.retain(|t| t.id != id);
            self.assets3d.shared_images.retain(|_, t| t.id != id);
        }
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_texture(id);
        }
    }

    /// Frees game font `id` (see [`Ctx::free`]); never the built-in one.
    pub(crate) fn free_font_id(&mut self, id: u32) {
        if !self.text.free_font(Font(id)) {
            return;
        }
        self.loaded_fonts.release(&[AssetKind::Font], |f| f.0 == id);
        self.watcher.forget(|a| matches!(a, Asset::Font(f) if *f == id));
    }

    /// Replaces every pixel of `texture` with `rgba` (same size as it was
    /// created with, 4 bytes per pixel). For images that change while the
    /// game runs: a minimap, a heat map, a simulated field. Cheap enough to
    /// do every frame for textures up to a few megapixels.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let mut pixels = vec![0u8; 64 * 64 * 4];
    /// let heat = ctx.gfx.create_texture(64, 64, &pixels);
    /// pixels[0..4].copy_from_slice(&[255, 0, 0, 255]); // top-left pixel red
    /// ctx.gfx.update_texture(heat, &pixels);
    /// # }
    /// ```
    ///
    /// # Panics
    /// If `rgba` isn't `width * height * 4` bytes.
    #[track_caller]
    pub fn update_texture(&mut self, texture: Texture, rgba: &[u8]) {
        let (w, h) = (texture.width(), texture.height());
        let expected = w as usize * h as usize * 4;
        assert_eq!(
            rgba.len(),
            expected,
            "ponko: a {w} x {h} texture needs {expected} bytes of RGBA, got {}",
            rgba.len()
        );
        #[cfg(feature = "3d")]
        assert!(
            !self.is_render_target(texture),
            "ponko: a render target is drawn by its target camera, not updated with pixels"
        );
        // A freed texture (or `Texture::NONE`) has no pixels of its own.
        if !self.textures_live.contains(texture.id) {
            return;
        }
        // A block-compressed texture takes pixels by becoming RGBA8.
        #[cfg(feature = "compressed-textures")]
        if self.compressed.remove(&texture.id).is_some() {
            self.asset_ledger
                .textures
                .insert(texture.id, crate::inventory::rgba8_bytes(w, h));
        }
        // A streamed texture at less than full resolution: back to full.
        if self.streaming.reduced_size(texture.id).is_some() {
            self.asset_ledger
                .textures
                .insert(texture.id, crate::inventory::rgba8_bytes(w, h));
            if !self.headless {
                self.renderer().replace_texture(texture.id, w, h, rgba);
            }
            self.streamed_texture_restored(texture.id);
            return;
        }
        if self.headless {
            return;
        }
        self.renderer().update_texture(texture.id, w, h, rgba);
    }

    /// How many sprites have been drawn so far this frame.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// println!("{} sprites", ctx.gfx.sprite_count());
    /// # }
    /// ```
    pub fn sprite_count(&self) -> usize {
        self.draw_list.len() + self.ui_list.len()
    }

    /// Saves the next frame as a PNG at `path`, once it has been drawn.
    /// Does nothing in headless runs (there is no frame). Errors are printed.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// fn photo_mode(ctx: &mut Ctx) {
    ///     if ctx.input.key_pressed(Key::F12) {
    ///         ctx.gfx.save_screenshot("screenshot.png");
    ///     }
    /// }
    /// # Game::new("t", 1, 1).system(photo_mode).run_headless(1, |_| {});
    /// ```
    pub fn save_screenshot(&mut self, path: impl Into<PathBuf>) {
        if !self.headless {
            self.queue_screenshot(path.into(), 0, None);
        }
    }

    pub(crate) fn queue_screenshot(
        &mut self,
        path: PathBuf,
        max_size: u32,
        reply: Option<Sender<Result<Value, String>>>,
    ) {
        self.screenshots.push(ScreenshotRequest {
            path,
            max_size,
            reply,
            claimed: None,
        });
    }

    /// [`queue_screenshot`](Self::queue_screenshot) into a file the control
    /// endpoint claimed.
    pub(crate) fn queue_claimed_screenshot(
        &mut self,
        claimed: crate::control::ClaimedFile,
        max_size: u32,
        reply: Sender<Result<Value, String>>,
    ) {
        self.screenshots.push(ScreenshotRequest {
            path: claimed.path.clone(),
            max_size,
            reply: Some(reply),
            claimed: Some(claimed),
        });
    }

    pub(crate) fn renderer(&mut self) -> &mut Renderer {
        self.renderer
            .as_mut()
            .expect("ponko: textures can only be created once the game is running (in setup or update)")
    }

    /// What the GPU can do: the adapter, its main limits, and for each
    /// device-dependent renderer feature (compute passes, TAA, SSR, global
    /// illumination, GPU timestamps, ...) whether it's available and, if
    /// not, the fallback used and why. `None` in headless runs and before
    /// the window opens. See [`GpuCapabilities`](crate::render::GpuCapabilities).
    #[must_use]
    pub fn gpu_capabilities(&self) -> Option<&crate::render::GpuCapabilities> {
        self.renderer.as_ref().map(Renderer::capabilities)
    }

    /// Engine side: forget queued draws (headless runs never render).
    pub(crate) fn discard_draws(&mut self) {
        self.draw_list.clear();
        self.ui_list.clear();
    }

    // ----------------------------------------------------------------- shapes

    /// For offscreen test rigs that draw `ui_list` themselves, with no
    /// renderer: gives shapes and text their textures (ids 0 and 1), so
    /// drawing them works. The rig uploads `text.uploads` into the second.
    #[cfg(all(test, feature = "3d"))]
    pub(crate) fn stand_in_textures(&mut self) {
        self.white = Some(crate::texture::Texture::new(0, 1, 1));
        self.font_atlas = Some(crate::texture::Texture::new(1, Text::ATLAS_SIZE, Text::ATLAS_SIZE));
    }

    pub(crate) fn white_texture(&mut self) -> crate::texture::Texture {
        if let Some(t) = self.white {
            return t;
        }
        let t = self.make_texture(1, 1, &[255; 4]);
        self.white = Some(t);
        t
    }

    fn circle_texture(&mut self) -> crate::texture::Texture {
        if let Some(t) = self.circle {
            return t;
        }
        let r = CIRCLE as f32 / 2.0;
        let mut rgba = Vec::with_capacity((CIRCLE * CIRCLE * 4) as usize);
        for y in 0..CIRCLE {
            for x in 0..CIRCLE {
                let d = Vec2::new(x as f32 + 0.5 - r, y as f32 + 0.5 - r).length();
                let alpha = (r - d).clamp(0.0, 1.0); // one pixel of antialiasing
                rgba.extend_from_slice(&[255, 255, 255, (alpha * 255.0) as u8]);
            }
        }
        let t = self.make_texture(CIRCLE, CIRCLE, &rgba);
        self.circle = Some(t);
        t
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(
        &mut self,
        layer: Layer,
        texture: u32,
        pos: Vec2,
        size: Vec2,
        rotation: f32,
        pivot: Vec2,
        color: Color,
        z: f32,
    ) {
        let instance = SpriteInstance::new(pos.into(), size.into(), pivot.into(), rotation, color.to_rgba8());
        match layer {
            Layer::World => self.draw_list.push(texture, z, instance),
            Layer::Screen => self.ui_list.push(texture, z, instance),
        }
    }

    /// Fills `rect` (world units) with `color`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.gfx.draw_rect(Rect::new(0.0, 0.0, 200.0, 20.0), Color::hex(0x3a5a40));
    /// # }
    /// ```
    pub fn draw_rect(&mut self, rect: Rect, color: Color) {
        let white = self.white_texture().id;
        self.shape(Layer::World, white, rect.min, rect.size(), 0.0, Vec2::ZERO, color, 0.0);
    }

    /// A line from `a` to `b`, `width` world units thick.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, from: Vec2, to: Vec2) {
    /// ctx.gfx.draw_line(from, to, 2.0, Color::RED);
    /// # }
    /// ```
    pub fn draw_line(&mut self, a: Vec2, b: Vec2, width: f32, color: Color) {
        let white = self.white_texture().id;
        let d = b - a;
        self.shape(
            Layer::World,
            white,
            a,
            Vec2::new(d.length(), width),
            d.to_angle(),
            Vec2::new(0.0, 0.5),
            color,
            0.0,
        );
    }

    /// A filled circle (world units).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, tower: Vec2) {
    /// ctx.gfx.draw_circle(tower, 120.0, Color::BLUE.with_alpha(0.2)); // its range
    /// # }
    /// ```
    pub fn draw_circle(&mut self, center: Vec2, radius: f32, color: Color) {
        let circle = self.circle_texture().id;
        self.shape(
            Layer::World,
            circle,
            center,
            Vec2::splat(radius * 2.0),
            0.0,
            Vec2::splat(0.5),
            color,
            0.0,
        );
    }

    /// A filled screen-space rectangle (UI building block).
    pub(crate) fn ui_fill(&mut self, rect: Rect, color: Color, z: f32) {
        let white = self.white_texture().id;
        self.shape(Layer::Screen, white, rect.min, rect.size(), 0.0, Vec2::ZERO, color, z);
    }
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn push_sprite(
    draw_list: &mut DrawList,
    texture: Texture,
    pos: Vec2,
    scale: Vec2,
    rotation: f32,
    z: f32,
    pivot: Vec2,
    color: [u8; 4],
    region_px: Option<Rect>,
    style: u32,
) {
    let (size, uv) = region(texture, region_px);
    draw_list.push_styled(
        texture.id,
        style,
        z,
        SpriteInstance::new(pos.into(), (size * scale).into(), pivot.into(), rotation, color).with_uv(uv),
    );
}
