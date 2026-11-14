//! Text: fonts, a glyph atlas filled on demand, and layout.
//!
//! Glyphs are rasterized at the window's physical pixel size the first time
//! they're drawn, packed into one atlas texture, and drawn as sprites in
//! screen space. Steady-state frames only look glyphs up.

use std::collections::HashMap;

use glam::Vec2;

use crate::color::Color;

#[cfg(feature = "text-shaping")]
mod shape;
#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
mod system;

/// A loaded font. Get one from [`Graphics::load_font`](crate::gfx::Graphics::load_font);
/// [`Font::DEFAULT`] is always there.
///
/// ```
/// # use ponko::prelude::*;
/// # fn hud(ctx: &mut Ctx) {
/// let params = TextParams::default().with_font(Font::DEFAULT).with_size(24.0);
/// ctx.gfx.draw_text_ex("Score: 120", vec2(16.0, 16.0), params);
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct Font(pub(crate) u32);

impl Font {
    /// The built-in font (Ubuntu Light, Ubuntu Font Licence), used by
    /// [`Ctx::draw_text`](crate::ctx::Ctx::draw_text).
    pub const DEFAULT: Font = Font(0);
}

/// Optional settings for [`Graphics::draw_text_ex`](crate::gfx::Graphics::draw_text_ex).
///
/// ```
/// # use ponko::prelude::*;
/// let title = TextParams::default().with_size(48.0).with_color(Color::hex(0xffcc00));
/// assert_eq!((title.size, title.z), (48.0, 0.0));
/// ```
///
/// It is `#[non_exhaustive]`, so later versions can add settings: start
/// from `TextParams::default()` and chain the `with_*` methods (or set the
/// public fields on a mutable value).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TextParams {
    /// The font to draw with.
    pub font: Font,
    /// Height of a line in window pixels. Default 16.
    pub size: f32,
    /// Default white.
    pub color: Color,
    /// Draw order among screen-space drawing; higher is on top. Default 0.
    pub z: f32,
}

impl Default for TextParams {
    fn default() -> Self {
        Self {
            font: Font::DEFAULT,
            size: 16.0,
            color: Color::WHITE,
            z: 0.0,
        }
    }
}

impl TextParams {
    /// Same settings drawn with `font`.
    #[must_use]
    pub fn with_font(self, font: Font) -> Self {
        Self { font, ..self }
    }
    /// Same settings with line height `size` in window pixels.
    #[must_use]
    pub fn with_size(self, size: f32) -> Self {
        Self { size, ..self }
    }
    /// Same settings with text color `color`.
    #[must_use]
    pub fn with_color(self, color: Color) -> Self {
        Self { color, ..self }
    }
    /// Same settings with draw order `z` among screen-space drawing.
    #[must_use]
    pub fn with_z(self, z: f32) -> Self {
        Self { z, ..self }
    }
}

/// How text is laid out with the `text-shaping` cargo feature: which
/// strings go through the shaper (ligatures, OpenType kerning, complex
/// scripts, right-to-left and mixed-direction lines) and which take the
/// per-character fast path. Set it with
/// [`Graphics::set_text_shaping`](crate::gfx::Graphics::set_text_shaping).
///
/// Without the feature nothing is shaped and this setting changes nothing.
///
/// ```
/// # use ponko::prelude::*;
/// # fn setup(ctx: &mut Ctx) {
/// // Latin text with the font's ligatures ("fi", "ffl") and kerning pairs.
/// ctx.gfx.set_text_shaping(TextShaping::Always);
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u32)]
pub enum TextShaping {
    /// ASCII strings keep the per-character fast path (the font's `kern`
    /// pairs, no ligatures); every other string is shaped. The default.
    #[default]
    Auto = 0,
    /// Every string is shaped, ASCII ones too.
    Always = 1,
}

/// Atlas side length in pixels.
const ATLAS: u32 = 1024;
/// Pixels of empty space around each glyph, so linear filtering doesn't
/// bleed neighbors in.
const PAD: u32 = 1;

#[derive(Clone, Copy)]
pub(crate) struct Glyph {
    /// Where in the atlas, in pixels (`None` for glyphs with no pixels, like space).
    pub at: Option<[u32; 2]>,
    /// Bitmap size in physical pixels.
    pub size: [u32; 2],
    /// Bitmap offset from the pen position: x right, y up from the baseline.
    pub offset: [f32; 2],
}

/// One glyph to draw, in physical pixels relative to the text's origin.
pub(crate) struct Placed {
    pub pos: Vec2,
    pub glyph: Glyph,
    /// Which line it's on, from 0.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub line: u32,
}

/// Line height in pixels SDF glyphs are made at.
#[cfg_attr(not(feature = "3d"), allow(dead_code))]
pub(crate) const SDF_PX: f32 = 64.0;
/// How far, in those pixels, SDF glyphs record the distance to the edge:
/// outlines and soft edges can reach this far out.
#[cfg_attr(not(feature = "3d"), allow(dead_code))]
pub(crate) const SDF_SPREAD: u32 = 8;

/// Signed-distance glyphs, made once per font and character at
/// [`SDF_PX`] and drawn at any size: the alpha channel is the distance to
/// the glyph's edge, 0.5 on it, more inside.
#[cfg_attr(not(feature = "3d"), allow(dead_code))]
#[derive(Default)]
pub(crate) struct SdfAtlas {
    glyphs: HashMap<(u32, char), Glyph>,
    cursor: [u32; 2],
    row: u32,
    /// The atlas is full: start over next frame.
    full: bool,
    /// Glyphs made but not yet on the GPU: atlas position, size, RGBA.
    pub uploads: Vec<([u32; 2], [u32; 2], Vec<u8>)>,
}

pub(crate) struct Text {
    fonts: Vec<Option<fontdue::Font>>,
    /// Which fonts are live (the built-in one always is).
    live: crate::handle::LiveSet,
    glyphs: HashMap<(u32, char, u32), Glyph>,
    /// Shelf packer: next free spot and the current row's height.
    cursor: [u32; 2],
    row: u32,
    /// The atlas is full: start over next frame.
    full: bool,
    /// Glyphs rasterized but not yet on the GPU: atlas position, size, RGBA.
    pub uploads: Vec<([u32; 2], [u32; 2], Vec<u8>)>,
    /// Reused per draw call.
    placed: Vec<Placed>,
    /// Each line's width in the last layout, in its pixels.
    line_widths: Vec<f32>,
    /// Signed-distance glyphs for text drawn at any size (3D text).
    pub sdf: SdfAtlas,
    /// Each font's fallback chain, by font id: fonts tried in order for a
    /// character the font lacks.
    fallbacks: Vec<Vec<Font>>,
    /// Which strings are shaped (with `text-shaping`).
    shaping: TextShaping,
    /// Shaping state: font data, shaped strings, glyphs by index.
    #[cfg(feature = "text-shaping")]
    shaper: shape::Shaper,
}

/// The bytes of a font file as a font; not one this engine reads is
/// [`ErrorKind::Decode`](crate::error::ErrorKind::Decode).
fn decode_font(bytes: &[u8]) -> Result<fontdue::Font, crate::error::Error> {
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).map_err(|e| {
        crate::error::Error::new(
            crate::error::ErrorKind::Decode,
            format!("not a font file this engine can read: {e}"),
        )
    })
}

impl Text {
    pub fn new() -> Self {
        Self {
            fonts: vec![None],
            live: {
                let mut live = crate::handle::LiveSet::default();
                live.insert(0);
                live
            },
            glyphs: HashMap::new(),
            cursor: [PAD, PAD],
            row: 0,
            full: false,
            uploads: Vec::new(),
            placed: Vec::new(),
            line_widths: Vec::new(),
            sdf: SdfAtlas::default(),
            fallbacks: Vec::new(),
            shaping: TextShaping::Auto,
            #[cfg(feature = "text-shaping")]
            shaper: shape::Shaper::default(),
        }
    }

    pub const ATLAS_SIZE: u32 = ATLAS;

    /// Whether `font` is the built-in one or a loaded one not freed.
    pub fn has_font(&self, font: Font) -> bool {
        self.live.contains(font.0)
    }

    /// Fonts loaded and not freed, not counting the built-in one.
    pub fn loaded_fonts(&self) -> usize {
        self.live.len() - 1
    }

    /// Adds a font from the bytes of a font file.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Decode`](crate::error::ErrorKind::Decode) if they aren't a font
    /// this engine can read.
    pub fn add_font(&mut self, bytes: &[u8]) -> Result<Font, crate::error::Error> {
        self.add_font_at(bytes, 0)
    }

    /// [`add_font`](Self::add_font) for face `index` of a font collection
    /// (`.ttc`); 0 for a single font.
    pub fn add_font_at(&mut self, bytes: &[u8], index: u32) -> Result<Font, crate::error::Error> {
        let font = if index == 0 {
            decode_font(bytes)?
        } else {
            fontdue::Font::from_bytes(
                bytes,
                fontdue::FontSettings {
                    collection_index: index,
                    ..Default::default()
                },
            )
            .map_err(|e| {
                crate::error::Error::new(
                    crate::error::ErrorKind::Decode,
                    format!("not a font file this engine can read: {e}"),
                )
            })?
        };
        self.fonts.push(Some(font));
        let id = self.fonts.len() as u32 - 1;
        self.live.insert(id);
        #[cfg(feature = "text-shaping")]
        self.shaper.set_bytes(id, bytes, index);
        Ok(Font(id))
    }

    /// Sets the fonts tried, in order, for characters `font` has no glyph
    /// for. Freed fonts in the chain are skipped.
    pub fn set_fallbacks(&mut self, font: Font, fallbacks: &[Font]) {
        let i = font.0 as usize;
        if self.fallbacks.len() <= i {
            self.fallbacks.resize_with(i + 1, Vec::new);
        }
        self.fallbacks[i].clear();
        self.fallbacks[i].extend(fallbacks.iter().copied().filter(|&f| f != font));
        #[cfg(feature = "text-shaping")]
        self.shaper.forget_shapes();
    }

    /// `font`'s fallback chain.
    pub fn fallbacks(&self, font: Font) -> &[Font] {
        self.fallbacks.get(font.0 as usize).map_or(&[], Vec::as_slice)
    }

    /// Which strings are shaped.
    pub fn shaping(&self) -> TextShaping {
        self.shaping
    }

    /// Sets which strings are shaped.
    pub fn set_shaping(&mut self, shaping: TextShaping) {
        self.shaping = shaping;
    }

    /// Whether `text` goes through the shaper.
    #[cfg(feature = "text-shaping")]
    fn shaped_path(&self, text: &str) -> bool {
        self.shaping == TextShaping::Always || !text.is_ascii()
    }

    /// Whether characters of `font` may be drawn with another font: it has
    /// a fallback chain (or system fonts are searched).
    fn chained(&self, font: Font) -> bool {
        #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
        if self.shaper.system.fallback {
            return true;
        }
        !self.fallbacks(font).is_empty()
    }

    /// The font `c` is drawn with when the text's font is `font`: itself if
    /// it has the glyph, else the first font in its chain that does (then
    /// a system font, if those are searched), else itself.
    fn font_for(&mut self, font: Font, c: char) -> Font {
        if self.font(font).lookup_glyph_index(c) != 0 {
            return font;
        }
        let chain = self.fallbacks.get(font.0 as usize).map_or(0, Vec::len);
        for i in 0..chain {
            let f = self.fallbacks[font.0 as usize][i];
            if self.has_font(f) && self.font(f).lookup_glyph_index(c) != 0 {
                return f;
            }
        }
        #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
        if let Some(f) = self.system_font_for(font, c) {
            return f;
        }
        font
    }

    /// Drops a loaded font and its glyphs; true if it was live. Text in it
    /// uses the built-in font from now on, which is never freed.
    pub fn free_font(&mut self, font: Font) -> bool {
        if font == Font::DEFAULT || !self.live.remove(font.0) {
            return false;
        }
        self.fonts[font.0 as usize] = None;
        self.glyphs.retain(|k, _| k.0 != font.0);
        self.sdf.glyphs.retain(|k, _| k.0 != font.0);
        #[cfg(feature = "text-shaping")]
        self.shaper.forget_font(font.0);
        true
    }

    /// The font text in `font` is drawn with: itself, or the built-in one
    /// for a freed font or `Font::NONE`.
    fn resolve(&self, font: Font) -> Font {
        if self.has_font(font) { font } else { Font::DEFAULT }
    }

    /// Replaces a loaded font's data (hot reload); its glyphs are drawn again.
    ///
    /// # Errors
    ///
    /// As [`add_font`](Self::add_font); the old font stays then.
    pub fn replace_font(&mut self, font: Font, bytes: &[u8]) -> Result<(), crate::error::Error> {
        let new = decode_font(bytes)?;
        if !self.has_font(font) {
            return Ok(());
        }
        self.fonts[font.0 as usize] = Some(new);
        self.glyphs.retain(|k, _| k.0 != font.0);
        self.sdf.glyphs.retain(|k, _| k.0 != font.0);
        #[cfg(feature = "text-shaping")]
        {
            self.shaper.forget_font(font.0);
            self.shaper.set_bytes(font.0, bytes, 0);
        }
        Ok(())
    }

    fn font(&mut self, font: Font) -> &fontdue::Font {
        let slot = &mut self.fonts[font.0 as usize];
        slot.get_or_insert_with(|| {
            fontdue::Font::from_bytes(epaint_default_fonts::UBUNTU_LIGHT, fontdue::FontSettings::default())
                .expect("the built-in font is valid")
        })
    }

    /// Call once per frame before drawing: forgets every glyph if the atlas
    /// filled up last frame, so it fills again with what's in use now.
    pub fn begin_frame(&mut self) -> bool {
        if self.sdf.full {
            self.sdf.full = false;
            self.sdf.glyphs.clear();
            #[cfg(feature = "text-shaping")]
            self.shaper.sdf_glyphs.clear();
            self.sdf.cursor = [PAD, PAD];
            self.sdf.row = 0;
        }
        if !self.full {
            return false;
        }
        self.full = false;
        self.glyphs.clear();
        #[cfg(feature = "text-shaping")]
        self.shaper.glyphs.clear();
        self.cursor = [PAD, PAD];
        self.row = 0;
        true
    }

    /// Lays out `text` at `px` physical pixels per line. With `rasterize`,
    /// missing glyphs are drawn into the atlas (queued in `uploads`).
    /// Returns the size in physical pixels; the glyphs are in `placed()`.
    pub fn layout(&mut self, text: &str, font: Font, px: f32, rasterize: bool) -> Vec2 {
        let font = self.resolve(font);
        let key_px = px.round().max(1.0) as u32;
        #[cfg(feature = "text-shaping")]
        if self.shaped_path(text) {
            return self.layout_shaped(text, font, px, |t, f, id| {
                match t.shaper.glyphs.get(&(f.0, id, key_px)) {
                    Some(g) => Some(*g),
                    None if rasterize => t.rasterize_indexed(f, id, px, key_px),
                    None => None,
                }
            });
        }
        self.layout_with(text, font, px, |t, f, c| match t.glyphs.get(&(f.0, c, key_px)) {
            Some(g) => Some(*g),
            None if rasterize => t.rasterize(f, c, px, key_px),
            None => None,
        })
    }

    /// Lays out `text` with signed-distance glyphs, at [`SDF_PX`] pixels
    /// per line (missing glyphs are made and queued in `sdf.uploads`).
    /// Returns the size in those pixels; the glyphs are in `placed()`,
    /// each line's width in `line_widths()`.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub fn layout_sdf(&mut self, text: &str, font: Font) -> Vec2 {
        let font = self.resolve(font);
        #[cfg(feature = "text-shaping")]
        if self.shaped_path(text) {
            return self.layout_shaped(text, font, SDF_PX, |t, f, id| {
                match t.shaper.sdf_glyphs.get(&(f.0, id)) {
                    Some(g) => Some(*g),
                    None => t.make_sdf_indexed(f, id),
                }
            });
        }
        self.layout_with(text, font, SDF_PX, |t, f, c| match t.sdf.glyphs.get(&(f.0, c)) {
            Some(g) => Some(*g),
            None => t.make_sdf(f, c),
        })
    }

    /// The line metrics of `font` at `px`: ascent and line height.
    fn line_metrics(&mut self, font: Font, px: f32) -> (f32, f32) {
        let lines = self.font(font).horizontal_line_metrics(px);
        lines.map_or((px * 0.8, px), |m| (m.ascent, m.new_line_size))
    }

    /// The per-character fast path: each character's glyph from the font's
    /// character map, the `kern` table's pairs, no shaping.
    fn layout_with(
        &mut self,
        text: &str,
        font: Font,
        px: f32,
        mut glyph_of: impl FnMut(&mut Self, Font, char) -> Option<Glyph>,
    ) -> Vec2 {
        let mut placed = std::mem::take(&mut self.placed);
        placed.clear();
        self.line_widths.clear();
        let (ascent, line_height) = self.line_metrics(font, px);
        let chained = self.chained(font);
        let (mut x, mut y, mut width) = (0.0f32, 0.0f32, 0.0f32);
        let mut line = 0;
        let mut prev: Option<(char, Font)> = None;
        for c in text.chars() {
            if c == '\n' {
                width = width.max(x);
                self.line_widths.push(x);
                x = 0.0;
                y += line_height;
                line += 1;
                prev = None;
                continue;
            }
            let used = if chained { self.font_for(font, c) } else { font };
            let f = self.font(used);
            if let Some((p, pf)) = prev
                && pf == used
            {
                x += f.horizontal_kern(p, c, px).unwrap_or(0.0);
            }
            let advance = f.metrics(c, px).advance_width;
            if let Some(glyph) = glyph_of(self, used, c) {
                let pos = Vec2::new(
                    (x + glyph.offset[0]).round(),
                    (y + ascent - glyph.offset[1] - glyph.size[1] as f32).round(),
                );
                placed.push(Placed { pos, glyph, line });
            }
            x += advance;
            prev = Some((c, used));
        }
        width = width.max(x);
        self.line_widths.push(x);
        self.placed = placed;
        Vec2::new(width, y + line_height)
    }

    /// Each line's width in the last layout.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub fn line_widths(&self) -> &[f32] {
        &self.line_widths
    }

    pub fn placed(&self) -> &[Placed] {
        &self.placed
    }

    /// The caret's x, in pixels at `px`, before byte `index` of the
    /// one-line `text` (`index == text.len()`: after the last character).
    /// Shaped text puts it on the visual edge of the character there, so
    /// it moves left through right-to-left runs.
    pub fn caret_x(&mut self, text: &str, font: Font, px: f32, index: usize) -> f32 {
        #[cfg(feature = "text-shaping")]
        if self.shaped_path(text) {
            let font = self.resolve(font);
            return self.caret_x_shaped(text, font, px, index);
        }
        self.layout(&text[..index], font, px, false).x
    }

    /// The pixel spans, left to right, that show bytes `lo..hi` of the
    /// one-line `text` selected: one span, or several where a selection
    /// crosses a change of direction.
    pub fn selection(&mut self, text: &str, font: Font, px: f32, lo: usize, hi: usize, out: &mut Vec<[f32; 2]>) {
        out.clear();
        #[cfg(feature = "text-shaping")]
        if self.shaped_path(text) {
            let font = self.resolve(font);
            return self.selection_shaped(text, font, px, lo..hi, out);
        }
        let a = self.layout(&text[..lo], font, px, false).x;
        let b = self.layout(&text[..hi], font, px, false).x;
        out.push([a, b]);
    }

    /// The byte index of the character boundary of the one-line `text`
    /// nearest to `x` pixels from its left edge (a click in a text field).
    pub fn hit(&mut self, text: &str, font: Font, px: f32, x: f32) -> usize {
        #[cfg(feature = "text-shaping")]
        if self.shaped_path(text) {
            let font = self.resolve(font);
            return self.hit_shaped(text, font, px, x);
        }
        let mut prev = 0.0;
        for (i, ch) in text.char_indices() {
            let next = i + ch.len_utf8();
            let width = self.layout(&text[..next], font, px, false).x;
            if x < (prev + width) * 0.5 {
                return i;
            }
            prev = width;
        }
        text.len()
    }

    /// Where shaped text wraps: the end of `line`'s first line at `width`
    /// pixels, or `None` for text that takes the fast path (it wraps at
    /// spaces).
    #[cfg_attr(not(feature = "text-shaping"), allow(unused_variables))]
    pub fn wrap_end_shaped_or_none(&mut self, line: &str, font: Font, px: f32, width: f32) -> Option<usize> {
        #[cfg(feature = "text-shaping")]
        if self.shaped_path(line) {
            let font = self.resolve(font);
            return Some(self.wrap_end_shaped(line, font, px, width));
        }
        None
    }

    /// The widest piece of `text` that can't be wrapped, in pixels: the
    /// widest word, or for shaped text the widest piece between Unicode
    /// line-break opportunities (each character of Japanese text).
    pub fn min_content_width(&mut self, text: &str, font: Font, px: f32) -> f32 {
        #[cfg(feature = "text-shaping")]
        if self.shaped_path(text) {
            let font = self.resolve(font);
            return self.min_content_shaped(text, font, px);
        }
        text.split_whitespace()
            .map(|word| self.layout(word, font, px, false).x)
            .fold(0.0, f32::max)
    }

    /// Whether `line` reads right to left (its first strong character is
    /// right to left). Always false without `text-shaping`.
    #[cfg_attr(not(feature = "text-shaping"), allow(unused_variables))]
    pub fn is_rtl(line: &str) -> bool {
        #[cfg(feature = "text-shaping")]
        return shape::is_rtl(line);
        #[cfg(not(feature = "text-shaping"))]
        false
    }

    /// Loads the system font of `family` (feature `system-fonts`, desktop).
    ///
    /// # Errors
    ///
    /// [`NotFound`](crate::error::ErrorKind::NotFound) if the system has no
    /// such family; [`Unsupported`](crate::error::ErrorKind::Unsupported)
    /// without the feature and on the web.
    pub fn system_font(&mut self, family: &str) -> Result<Font, crate::error::Error> {
        #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
        return self.load_system_font(family);
        #[cfg(not(all(feature = "system-fonts", not(target_arch = "wasm32"))))]
        Err(crate::error::Error::new(
            crate::error::ErrorKind::Unsupported,
            if cfg!(target_arch = "wasm32") {
                "system fonts can't be read on the web: load the font from the game's assets".to_owned()
            } else {
                "system fonts need the `system-fonts` cargo feature".to_owned()
            },
        )
        .with_path(family))
    }

    /// Turns the search of system fonts for characters no loaded font has
    /// on or off; true if it can search (feature `system-fonts`, desktop).
    #[cfg_attr(
        not(all(feature = "system-fonts", not(target_arch = "wasm32"))),
        allow(unused_variables, clippy::unused_self)
    )]
    pub fn system_fallback(&mut self, on: bool) -> bool {
        #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
        {
            self.set_system_fallback(on);
            true
        }
        #[cfg(not(all(feature = "system-fonts", not(target_arch = "wasm32"))))]
        false
    }

    fn rasterize(&mut self, font: Font, c: char, px: f32, key_px: u32) -> Option<Glyph> {
        let (m, coverage) = self.font(font).rasterize(c, px);
        let glyph = self.pack(&m, &coverage)?;
        self.glyphs.insert((font.0, c, key_px), glyph);
        Some(glyph)
    }

    /// Packs a rasterized glyph into the atlas and queues its upload;
    /// `None` if the atlas is full.
    fn pack(&mut self, m: &fontdue::Metrics, coverage: &[u8]) -> Option<Glyph> {
        let size = [m.width as u32, m.height as u32];
        let offset = [m.xmin as f32, m.ymin as f32];
        let at = if size[0] == 0 || size[1] == 0 {
            None
        } else {
            if self.cursor[0] + size[0] + PAD > ATLAS {
                self.cursor = [PAD, self.cursor[1] + self.row + PAD];
                self.row = 0;
            }
            if self.cursor[1] + size[1] + PAD > ATLAS {
                self.full = true;
                return None;
            }
            let at = self.cursor;
            self.cursor[0] += size[0] + PAD;
            self.row = self.row.max(size[1]);
            // White pixels, coverage as alpha: the sprite's color tints it.
            let rgba = coverage.iter().flat_map(|&a| [255, 255, 255, a]).collect();
            self.uploads.push((at, size, rgba));
            Some(at)
        };
        Some(Glyph { at, size, offset })
    }
}

impl Text {
    /// Makes `c`'s signed-distance glyph and packs it into the SDF atlas.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    fn make_sdf(&mut self, font: Font, c: char) -> Option<Glyph> {
        let (m, coverage) = self.font(font).rasterize(c, SDF_PX);
        let glyph = self.pack_sdf(&m, &coverage)?;
        self.sdf.glyphs.insert((font.0, c), glyph);
        Some(glyph)
    }

    /// Turns a glyph rasterized at [`SDF_PX`] into a distance field, packs
    /// it into the SDF atlas and queues its upload; `None` if it is full.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    fn pack_sdf(&mut self, m: &fontdue::Metrics, coverage: &[u8]) -> Option<Glyph> {
        let spread = SDF_SPREAD;
        let (w, h) = (m.width as u32, m.height as u32);
        let size = if w == 0 || h == 0 {
            [0, 0]
        } else {
            [w + 2 * spread, h + 2 * spread]
        };
        let offset = [m.xmin as f32 - spread as f32, m.ymin as f32 - spread as f32];
        let at = if size[0] == 0 {
            None
        } else {
            let atlas = &mut self.sdf;
            if atlas.cursor == [0, 0] {
                atlas.cursor = [PAD, PAD];
            }
            if atlas.cursor[0] + size[0] + PAD > ATLAS {
                atlas.cursor = [PAD, atlas.cursor[1] + atlas.row + PAD];
                atlas.row = 0;
            }
            if atlas.cursor[1] + size[1] + PAD > ATLAS {
                atlas.full = true;
                return None;
            }
            let at = atlas.cursor;
            atlas.cursor[0] += size[0] + PAD;
            atlas.row = atlas.row.max(size[1]);
            let distance = signed_distance(coverage, w, h, spread);
            let rgba = distance.iter().flat_map(|&a| [255, 255, 255, a]).collect();
            atlas.uploads.push((at, size, rgba));
            Some(at)
        };
        Some(Glyph { at, size, offset })
    }
}

/// A glyph's coverage (`w` x `h`) as a signed distance field `spread`
/// pixels larger on each side: 128 on the edge, up to 255 `spread` pixels
/// inside, down to 0 as far outside.
#[cfg_attr(not(feature = "3d"), allow(dead_code))]
fn signed_distance(coverage: &[u8], w: u32, h: u32, spread: u32) -> Vec<u8> {
    let (w, h, s) = (w as i32, h as i32, spread as i32);
    let at = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            f32::from(coverage[(y * w + x) as usize]) / 255.0
        }
    };
    let (out_w, out_h) = (w + 2 * s, h + 2 * s);
    let mut out = Vec::with_capacity((out_w * out_h) as usize);
    for oy in 0..out_h {
        for ox in 0..out_w {
            let (x, y) = (ox - s, oy - s);
            let a = at(x, y);
            let signed = if a > 0.0 && a < 1.0 {
                // On the edge: coverage says how far across it.
                a - 0.5
            } else {
                // The nearest pixel the edge runs through or past; it
                // crosses one half of coverage `b - 0.5` beyond that
                // pixel's center (for inside pixels; mirrored outside).
                let inside = a >= 1.0;
                let mut best = s as f32;
                for dy in -s..=s {
                    for dx in -s..=s {
                        let b = at(x + dx, y + dy);
                        let across = if inside { b - 0.5 } else { 0.5 - b };
                        if (inside && b >= 1.0) || (!inside && b <= 0.0) {
                            continue;
                        }
                        let d = ((dx * dx + dy * dy) as f32).sqrt() + across;
                        best = best.min(d);
                    }
                }
                if inside { best } else { -best }
            };
            let v = 0.5 + signed / (2.0 * s as f32);
            out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_measures_lines_and_kerning() {
        let mut t = Text::new();
        let one = t.layout("Hello", Font::DEFAULT, 20.0, false);
        let two = t.layout("Hello\nHello", Font::DEFAULT, 20.0, false);
        assert!(one.x > 30.0 && one.x < 70.0, "{one}");
        assert_eq!(two.x, one.x);
        assert!((two.y - 2.0 * one.y).abs() < 0.01, "{one} {two}");
        assert_eq!(t.layout("", Font::DEFAULT, 20.0, false).x, 0.0);
    }

    #[test]
    fn glyphs_are_rasterized_once_and_packed_apart() {
        let mut t = Text::new();
        t.layout("abca", Font::DEFAULT, 32.0, true);
        assert_eq!(t.uploads.len(), 3, "a, b, c once each");
        assert_eq!(t.placed().len(), 4);
        let spots: Vec<_> = t.uploads.iter().map(|(at, size, _)| (*at, *size)).collect();
        for (i, (a, sa)) in spots.iter().enumerate() {
            for (b, sb) in &spots[i + 1..] {
                let apart =
                    a[0] + sa[0] <= b[0] || b[0] + sb[0] <= a[0] || a[1] + sa[1] <= b[1] || b[1] + sb[1] <= a[1];
                assert!(apart, "glyphs overlap in the atlas");
            }
        }
        t.uploads.clear();
        t.layout("cab", Font::DEFAULT, 32.0, true);
        assert!(t.uploads.is_empty(), "cached");
    }

    #[test]
    fn a_full_atlas_starts_over_next_frame() {
        let mut t = Text::new();
        // Huge glyphs fill the atlas quickly.
        let many: String = ('A'..='Z').chain('a'..='z').collect();
        t.layout(&many, Font::DEFAULT, 400.0, true);
        assert!(t.full);
        assert!(t.begin_frame());
        assert!(!t.full && t.glyphs.is_empty());
        assert!(!t.begin_frame());
    }

    #[test]
    fn distance_fields_are_half_on_the_edge_and_rise_inside() {
        // A 4 x 4 square, fully covered, with a spread of 3.
        let field = signed_distance(&[255; 16], 4, 4, 3);
        let side = 10;
        let at = |x: usize, y: usize| field[y * side + x];
        // Center of the square: 2 px from the edge, inside.
        assert!(at(5, 5) > 128 + 20, "{}", at(5, 5));
        // Just inside and just outside the left edge (x = 3 is the first
        // covered column) are on either side of one half.
        assert!(at(3, 5) > 128 && at(2, 5) < 128, "{} {}", at(3, 5), at(2, 5));
        // Far outside: as low as the spread allows.
        assert_eq!(at(0, 0), 0);
        // Symmetric.
        assert_eq!(at(3, 5), at(6, 5));
    }

    #[test]
    fn sdf_glyphs_are_made_once_padded_by_the_spread() {
        let mut t = Text::new();
        let size = t.layout_sdf("Hi\nthere", Font::DEFAULT);
        assert_eq!(t.line_widths().len(), 2);
        assert!(size.y > SDF_PX * 1.5, "{size}");
        assert_eq!(t.placed().iter().filter(|p| p.line == 1).count(), 5);
        let made = t.sdf.uploads.len();
        assert_eq!(made, 6, "H, i, t, h, e, r (e twice)");
        let ([w, h], bitmap) = (t.sdf.uploads[0].1, t.font(Font::DEFAULT).rasterize('H', SDF_PX).0);
        assert_eq!(
            [w, h],
            [
                bitmap.width as u32 + 2 * SDF_SPREAD,
                bitmap.height as u32 + 2 * SDF_SPREAD
            ]
        );
        t.layout_sdf("there", Font::DEFAULT);
        assert_eq!(t.sdf.uploads.len(), made, "cached");
    }

    #[test]
    fn spaces_take_room_but_no_atlas_space() {
        let mut t = Text::new();
        let w = t.layout("a a", Font::DEFAULT, 20.0, true).x;
        assert!(w > t.layout("aa", Font::DEFAULT, 20.0, true).x);
        assert!(t.uploads.iter().all(|(_, size, _)| size[0] > 0));
    }
}
