//! Text shaping (feature `text-shaping`): strings that leave the
//! per-character fast path are split into lines, ordered with the Unicode
//! bidirectional algorithm, itemized into runs of one script and one font
//! (walking each font's fallback chain), and shaped with harfrust, the
//! HarfBuzz project's Rust port. fontdue still rasterizes, by glyph index.
//!
//! Shaped strings are cached in em units, so a string drawn every frame is
//! shaped once; a cache hit costs a hash of the string and one glyph lookup
//! per glyph, as on the fast path.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use glam::Vec2;
use unicode_bidi::BidiInfo;
use unicode_script::UnicodeScript;

use super::{Font, Glyph, Placed, Text};

/// Shaped strings kept before the cache starts over.
const CACHE_MAX: usize = 2048;

/// A font file's bytes, for the shaper.
#[derive(Clone)]
enum Bytes {
    /// The built-in font.
    Builtin,
    Owned(Arc<[u8]>),
}

impl Bytes {
    fn get(&self) -> &[u8] {
        match self {
            Bytes::Builtin => epaint_default_fonts::UBUNTU_LIGHT,
            Bytes::Owned(b) => b,
        }
    }
}

/// What the shaper keeps for one font.
struct Face {
    bytes: Bytes,
    /// Face index in a collection.
    index: u32,
    /// harfrust's per-font tables, made on first use.
    data: Option<harfrust::ShaperData>,
}

/// One shaped glyph, in em units (multiply by the pixel size).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShapedGlyph {
    pub font: u32,
    pub id: u16,
    /// Pen position, from the line's left edge.
    pub x: f32,
    /// The glyph's offset right of the pen (marks placed on a letter).
    pub dx: f32,
    /// Offset up from the baseline.
    pub y: f32,
    pub advance: f32,
    /// Byte index in the string of the first character it shows.
    pub cluster: u32,
    pub line: u32,
    pub rtl: bool,
}

/// A shaped string.
pub(crate) struct Shaped {
    font: u32,
    text: Box<str>,
    pub glyphs: Vec<ShapedGlyph>,
    /// Each line's width in em units.
    pub widths: Vec<f32>,
}

#[derive(Default)]
pub(crate) struct Shaper {
    faces: Vec<Option<Face>>,
    cache: HashMap<u64, Shaped>,
    buffer: Option<harfrust::UnicodeBuffer>,
    /// Bitmap glyphs by font, glyph index and pixel size.
    pub glyphs: HashMap<(u32, u16, u32), Glyph>,
    /// Signed-distance glyphs by font and glyph index.
    pub sdf_glyphs: HashMap<(u32, u16), Glyph>,
    /// Scratch for caret queries: (start, end, left, right, rtl) per cluster.
    clusters: Vec<(u32, u32, f32, f32, bool)>,
    #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
    pub system: super::system::SystemFonts,
}

impl Shaper {
    /// Keeps a font's bytes for shaping.
    pub fn set_bytes(&mut self, id: u32, bytes: &[u8], index: u32) {
        let i = id as usize;
        if self.faces.len() <= i {
            self.faces.resize_with(i + 1, || None);
        }
        self.faces[i] = Some(Face {
            bytes: Bytes::Owned(bytes.into()),
            index,
            data: None,
        });
        self.forget_shapes();
    }

    /// Drops what was kept for font `id` (freed or replaced).
    pub fn forget_font(&mut self, id: u32) {
        if let Some(face) = self.faces.get_mut(id as usize) {
            *face = None;
        }
        self.glyphs.retain(|k, _| k.0 != id);
        self.sdf_glyphs.retain(|k, _| k.0 != id);
        self.forget_shapes();
    }

    /// Forgets every shaped string (fonts or fallbacks changed).
    pub fn forget_shapes(&mut self) {
        self.cache.clear();
    }

    /// The face of font `id`, with its shaper tables made.
    fn face(&mut self, id: u32) -> Option<&Face> {
        let i = id as usize;
        if id == 0 && self.faces.first().is_none_or(Option::is_none) {
            if self.faces.is_empty() {
                self.faces.push(None);
            }
            self.faces[0] = Some(Face {
                bytes: Bytes::Builtin,
                index: 0,
                data: None,
            });
        }
        let face = self.faces.get_mut(i)?.as_mut()?;
        if face.data.is_none() {
            let font = harfrust::FontRef::from_index(face.bytes.get(), face.index).ok()?;
            face.data = Some(harfrust::ShaperData::new(&font));
        }
        Some(face)
    }
}

fn key(font: u32, text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    font.hash(&mut h);
    text.hash(&mut h);
    h.finish()
}

/// `c`'s script for itemizing, `None` for characters that take their
/// neighbours' (spaces, punctuation, digits, combining marks).
fn script_of(c: char) -> Option<unicode_script::Script> {
    use unicode_script::Script;
    match c.script() {
        Script::Common | Script::Inherited | Script::Unknown => None,
        s => Some(s),
    }
}

fn hb_script(s: unicode_script::Script) -> Option<harfrust::Script> {
    let name = s.short_name().as_bytes();
    let tag: [u8; 4] = name.try_into().ok()?;
    harfrust::Script::from_iso15924_tag(harfrust::Tag::new(&tag))
}

/// One run to shape: a byte range of a line in one font, one script and one
/// direction.
struct Item {
    range: Range<usize>,
    font: Font,
    script: Option<unicode_script::Script>,
}

impl Text {
    /// Shapes `text` in `font` (cached) and returns its cache key.
    fn shaped(&mut self, text: &str, font: Font) -> u64 {
        let k = key(font.0, text);
        if let Some(s) = self.shaper.cache.get(&k)
            && s.font == font.0
            && &*s.text == text
        {
            return k;
        }
        let shaped = self.shape_uncached(text, font);
        if self.shaper.cache.len() >= CACHE_MAX {
            self.shaper.cache.clear();
        }
        self.shaper.cache.insert(k, shaped);
        k
    }

    /// Runs `f` on `text`'s shaped form (shaped now if it isn't cached).
    pub(crate) fn with_shaped<R>(&mut self, text: &str, font: Font, f: impl FnOnce(&mut Self, &Shaped) -> R) -> R {
        let k = self.shaped(text, font);
        let shaped = self.shaper.cache.remove(&k).expect("just shaped");
        let r = f(self, &shaped);
        self.shaper.cache.insert(k, shaped);
        r
    }

    fn shape_uncached(&mut self, text: &str, font: Font) -> Shaped {
        let mut out = Shaped {
            font: font.0,
            text: text.into(),
            glyphs: Vec::new(),
            widths: Vec::new(),
        };
        let mut start = 0;
        for (line, paragraph) in text.split('\n').enumerate() {
            let x = self.shape_line(paragraph, start, line as u32, font, &mut out.glyphs);
            out.widths.push(x);
            start += paragraph.len() + 1;
        }
        out
    }

    /// Shapes one line (no `\n`) at byte `base` of the string into `out`;
    /// returns its width in em units.
    fn shape_line(&mut self, line_text: &str, base: usize, line: u32, font: Font, out: &mut Vec<ShapedGlyph>) -> f32 {
        let mut x = 0.0;
        if line_text.is_empty() {
            return x;
        }
        let bidi = BidiInfo::new(line_text, None);
        let mut items = Vec::new();
        for para in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(para, para.range.clone());
            for run in runs {
                let rtl = levels[run.start].is_rtl();
                items.clear();
                self.itemize(line_text, run, font, &mut items);
                if rtl {
                    items.reverse();
                }
                for item in &items {
                    x = self.shape_item(line_text, item, rtl, x, base, line, out);
                }
            }
        }
        x
    }

    /// Splits `range` of `line` into runs of one font and one script.
    fn itemize(&mut self, line: &str, range: Range<usize>, font: Font, items: &mut Vec<Item>) {
        let first = items.len();
        for (i, c) in line[range.clone()].char_indices() {
            let at = range.start + i;
            let script = script_of(c);
            // Characters with no script of their own (spaces, punctuation,
            // marks) stay in the current run when its font has them.
            if let Some(cur) = items[first..].last_mut()
                && (script.is_none() || cur.script.is_none() || cur.script == script)
                && (cur.font == self.font_for(font, c) || (script.is_none() && self.font(cur.font).has_glyph(c)))
            {
                cur.range.end = at + c.len_utf8();
                if cur.script.is_none() {
                    cur.script = script;
                }
                continue;
            }
            let f = self.font_for(font, c);
            items.push(Item {
                range: at..at + c.len_utf8(),
                font: f,
                script,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn shape_item(
        &mut self,
        line_text: &str,
        item: &Item,
        rtl: bool,
        mut x: f32,
        base: usize,
        line: u32,
        out: &mut Vec<ShapedGlyph>,
    ) -> f32 {
        let mut buffer = self.shaper.buffer.take().unwrap_or_default();
        let Some(face) = self.shaper.face(item.font.0) else {
            self.shaper.buffer = Some(buffer);
            return x;
        };
        let Ok(font_ref) = harfrust::FontRef::from_index(face.bytes.get(), face.index) else {
            self.shaper.buffer = Some(buffer);
            return x;
        };
        let shaper = face.data.as_ref().expect("made by face()").shaper(&font_ref).build();
        let upem = shaper.units_per_em().max(1) as f32;
        buffer.push_str(&line_text[item.range.clone()]);
        buffer.set_pre_context(&line_text[..item.range.start]);
        buffer.set_post_context(&line_text[item.range.end..]);
        buffer.set_direction(if rtl {
            harfrust::Direction::RightToLeft
        } else {
            harfrust::Direction::LeftToRight
        });
        if let Some(script) = item.script.and_then(hb_script) {
            buffer.set_script(script);
        }
        buffer.guess_segment_properties();
        let glyphs = shaper.shape(buffer, harfrust::ShapeOptions::new());
        for (info, pos) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            out.push(ShapedGlyph {
                font: item.font.0,
                id: info.glyph_id as u16,
                x,
                dx: pos.x_offset as f32 / upem,
                y: pos.y_offset as f32 / upem,
                advance: pos.x_advance as f32 / upem,
                cluster: (base + item.range.start) as u32 + info.cluster,
                line,
                rtl,
            });
            x += pos.x_advance as f32 / upem;
        }
        self.shaper.buffer = Some(glyphs.clear());
        x
    }

    /// [`layout`](Text::layout) for shaped text: places the shaped glyphs
    /// at `px`, getting each one's bitmap from `glyph_of`.
    pub(super) fn layout_shaped(
        &mut self,
        text: &str,
        font: Font,
        px: f32,
        mut glyph_of: impl FnMut(&mut Self, Font, u16) -> Option<Glyph>,
    ) -> Vec2 {
        let (ascent, line_height) = self.line_metrics(font, px);
        self.with_shaped(text, font, |t, s| {
            let mut placed = std::mem::take(&mut t.placed);
            placed.clear();
            t.line_widths.clear();
            for g in &s.glyphs {
                if let Some(glyph) = glyph_of(t, Font(g.font), g.id) {
                    let top = g.line as f32 * line_height;
                    let pos = Vec2::new(
                        ((g.x + g.dx) * px + glyph.offset[0]).round(),
                        (top + ascent - g.y * px - glyph.offset[1] - glyph.size[1] as f32).round(),
                    );
                    placed.push(Placed {
                        pos,
                        glyph,
                        line: g.line,
                    });
                }
            }
            t.placed = placed;
            let mut width = 0.0f32;
            for w in &s.widths {
                t.line_widths.push(w * px);
                width = width.max(w * px);
            }
            Vec2::new(width, (s.widths.len().max(1) - 1) as f32 * line_height + line_height)
        })
    }

    pub(super) fn rasterize_indexed(&mut self, font: Font, id: u16, px: f32, key_px: u32) -> Option<Glyph> {
        let (m, coverage) = self.font(font).rasterize_indexed(id, px);
        let glyph = self.pack(&m, &coverage)?;
        self.shaper.glyphs.insert((font.0, id, key_px), glyph);
        Some(glyph)
    }

    pub(super) fn make_sdf_indexed(&mut self, font: Font, id: u16) -> Option<Glyph> {
        let (m, coverage) = self.font(font).rasterize_indexed(id, super::SDF_PX);
        let glyph = self.pack_sdf(&m, &coverage)?;
        self.shaper.sdf_glyphs.insert((font.0, id), glyph);
        Some(glyph)
    }

    /// Fills the cluster scratch from the first line of `s`: each
    /// cluster's byte range and horizontal extent at `px`, in logical order.
    fn clusters(&mut self, s: &Shaped, px: f32) {
        let clusters = &mut self.shaper.clusters;
        clusters.clear();
        for g in s.glyphs.iter().filter(|g| g.line == 0) {
            let (l, r) = (g.x * px, (g.x + g.advance) * px);
            match clusters.iter_mut().find(|c| c.0 == g.cluster) {
                Some(c) => {
                    c.2 = c.2.min(l);
                    c.3 = c.3.max(r);
                }
                None => clusters.push((g.cluster, 0, l, r, g.rtl)),
            }
        }
        clusters.sort_by_key(|c| c.0);
        let line_end = s.text.find('\n').unwrap_or(s.text.len()) as u32;
        for i in 0..clusters.len() {
            clusters[i].1 = clusters.get(i + 1).map_or(line_end, |c| c.0);
        }
    }

    /// The caret's x before byte `index` of `text`'s first line, from the
    /// shaped clusters: the leading edge of the cluster that starts there
    /// (its left side left to right, its right side right to left), a
    /// share of a cluster that holds several characters (a ligature), the
    /// trailing edge of the last one at the end.
    fn caret_in(&self, text: &str, index: usize) -> f32 {
        let clusters = &self.shaper.clusters;
        let Some(last) = clusters.last() else { return 0.0 };
        let index = index as u32;
        if index >= last.1 {
            return if last.4 { last.2 } else { last.3 };
        }
        let Some(c) = clusters.iter().find(|c| c.0 <= index && index < c.1) else {
            return 0.0;
        };
        edge(text, c, index)
    }

    pub(super) fn caret_x_shaped(&mut self, text: &str, font: Font, px: f32, index: usize) -> f32 {
        self.with_shaped(text, font, |t, s| {
            t.clusters(s, px);
            t.caret_in(text, index)
        })
    }

    pub(super) fn selection_shaped(
        &mut self,
        text: &str,
        font: Font,
        px: f32,
        range: Range<usize>,
        out: &mut Vec<[f32; 2]>,
    ) {
        self.with_shaped(text, font, |t, s| {
            t.clusters(s, px);
            // Every cluster the range covers, merged where they touch: a
            // logical range can be several pieces on screen in mixed text.
            let mut spans: Vec<[f32; 2]> = t
                .shaper
                .clusters
                .iter()
                .filter(|c| (c.0 as usize) < range.end && (c.1 as usize) > range.start)
                .map(|c| {
                    let a = edge(text, c, (c.0 as usize).max(range.start) as u32);
                    let b = edge(text, c, (c.1 as usize).min(range.end) as u32);
                    [a.min(b), a.max(b)]
                })
                .collect();
            spans.sort_by(|a, b| a[0].total_cmp(&b[0]));
            for span in spans {
                match out.last_mut() {
                    Some(last) if span[0] <= last[1] + 0.5 => last[1] = last[1].max(span[1]),
                    _ => out.push(span),
                }
            }
        });
    }

    pub(super) fn hit_shaped(&mut self, text: &str, font: Font, px: f32, x: f32) -> usize {
        self.with_shaped(text, font, |t, s| {
            t.clusters(s, px);
            let end = text.find('\n').unwrap_or(text.len());
            let mut best = (f32::INFINITY, 0);
            for i in text[..end].char_indices().map(|(i, _)| i).chain([end]) {
                let d = (t.caret_in(text, i) - x).abs();
                if d < best.0 {
                    best = (d, i);
                }
            }
            best.1
        })
    }

    /// Where to end the first line of `line` wrapped at `width` pixels:
    /// the last Unicode line-break opportunity (UAX #14) that fits, with
    /// trailing spaces left out; inside the first word if none does.
    pub(super) fn wrap_end_shaped(&mut self, line: &str, font: Font, px: f32, width: f32) -> usize {
        self.with_shaped(line, font, |_, s| {
            // Width of the logical prefix that ends at byte `end`.
            let prefix = |end: usize| -> f32 {
                s.glyphs
                    .iter()
                    .filter(|g| g.line == 0 && (g.cluster as usize) < end)
                    .map(|g| g.advance)
                    .sum::<f32>()
                    * px
            };
            let mut best = None;
            for (at, _) in unicode_linebreak::linebreaks(line) {
                let end = line[..at].trim_end().len();
                if end == 0 {
                    continue;
                }
                if prefix(end) > width {
                    break;
                }
                best = Some(if at == line.len() { at } else { end });
            }
            best.unwrap_or_else(|| {
                // The first word alone is too wide: break inside it, after
                // at least one character.
                let mut end = line.chars().next().map_or(0, char::len_utf8);
                for (i, c) in line.char_indices().skip(1) {
                    if prefix(i + c.len_utf8()) > width {
                        break;
                    }
                    end = i + c.len_utf8();
                }
                end
            })
        })
    }

    /// The widest piece of `text` that can't be broken, in pixels.
    pub(super) fn min_content_shaped(&mut self, text: &str, font: Font, px: f32) -> f32 {
        self.with_shaped(text, font, |_, s| {
            let mut widest = 0.0f32;
            let mut start = 0;
            for (at, _) in unicode_linebreak::linebreaks(text) {
                let end = text[..at].trim_end().len();
                let w: f32 = s
                    .glyphs
                    .iter()
                    .filter(|g| (start..end).contains(&(g.cluster as usize)))
                    .map(|g| g.advance)
                    .sum();
                widest = widest.max(w * px);
                start = at;
            }
            widest
        })
    }
}

/// The x of byte `index` inside cluster `c` (start, end, left, right,
/// right to left): its leading edge at the start, its trailing edge at the
/// end, a share of the way for a cluster of several characters.
fn edge(text: &str, c: &(u32, u32, f32, f32, bool), index: u32) -> f32 {
    let chars = |a: u32, b: u32| text.get(a as usize..b as usize).map_or(1, |t| t.chars().count()) as f32;
    let share = chars(c.0, index) / chars(c.0, c.1).max(1.0);
    let (left, right) = (c.2, c.3);
    if c.4 {
        right - share * (right - left)
    } else {
        left + share * (right - left)
    }
}

/// Whether `line` reads right to left: its first strong character is
/// (UAX #9, rules P2 and P3).
pub(crate) fn is_rtl(line: &str) -> bool {
    unicode_bidi::get_base_direction(line) == unicode_bidi::Direction::Rtl
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/assets/fonts/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn ascii_takes_the_fast_path_unless_shaping_is_always() {
        let mut t = Text::new();
        let fast = t.layout("office", Font::DEFAULT, 32.0, false);
        assert!(t.shaper.cache.is_empty(), "ASCII is not shaped by default");
        t.set_shaping(super::super::TextShaping::Always);
        let shaped = t.layout("office", Font::DEFAULT, 32.0, false);
        assert_eq!(t.shaper.cache.len(), 1);
        // Same font, same advances: within a pixel or two of kerning.
        assert!((fast.x - shaped.x).abs() < 3.0, "{fast} {shaped}");
    }

    #[test]
    fn a_shaped_string_is_shaped_once() {
        let mut t = Text::new();
        t.layout("café", Font::DEFAULT, 20.0, true);
        let glyphs = t.placed().len();
        assert_eq!(t.shaper.cache.len(), 1);
        t.uploads.clear();
        t.layout("café", Font::DEFAULT, 20.0, true);
        assert_eq!(t.shaper.cache.len(), 1);
        assert!(t.uploads.is_empty(), "glyphs cached by index");
        assert_eq!(t.placed().len(), glyphs);
    }

    #[test]
    fn arabic_letters_join_into_contextual_forms() {
        let mut t = Text::new();
        let arabic = t.add_font(&fixture("NotoSansArabic-Subset.ttf")).unwrap();
        // The same letter alone and inside a word: shaping picks a
        // different glyph for its joined form.
        // (Noto draws a letter's dots as a separate mark glyph, so compare
        // the glyphs of each letter's cluster.)
        let ids = |t: &mut Text, s: &str, cluster: u32| {
            t.with_shaped(s, arabic, |_, s| {
                s.glyphs
                    .iter()
                    .filter(|g| g.cluster == cluster)
                    .map(|g| g.id)
                    .collect::<Vec<_>>()
            })
        };
        let alone = ids(&mut t, "ب", 0);
        let first = ids(&mut t, "ببب", 0);
        let middle = ids(&mut t, "ببب", 2);
        let last = ids(&mut t, "ببب", 4);
        assert!(!alone.is_empty());
        for joined in [&first, &middle, &last] {
            assert_ne!(*joined, alone, "a joined beh is drawn with its own form");
        }
        assert_ne!(first, last);
        // Right to left: the first letter (its cluster 0) is drawn rightmost.
        t.with_shaped("ببب", arabic, |_, s| {
            let first = s.glyphs.iter().find(|g| g.cluster == 0).unwrap();
            assert!(s.glyphs.iter().all(|g| g.x <= first.x), "{:?}", s.glyphs);
            assert!(first.rtl);
        });
    }

    #[test]
    fn devanagari_reorders_the_short_i_before_its_consonant() {
        let mut t = Text::new();
        let hindi = t.add_font(&fixture("NotoSansDevanagari-Subset.ttf")).unwrap();
        // "कि": KA then the vowel sign I, which is drawn to KA's left.
        t.with_shaped("कि", hindi, |_, s| {
            assert_eq!(s.glyphs.len(), 2, "{:?}", s.glyphs);
            assert!(s.glyphs[0].x <= s.glyphs[1].x);
            assert_eq!(s.glyphs[0].cluster, s.glyphs[1].cluster, "one cluster");
        });
    }

    #[test]
    fn mixed_direction_lines_put_each_run_in_visual_order() {
        let mut t = Text::new();
        let arabic = t.add_font(&fixture("NotoSansArabic-Subset.ttf")).unwrap();
        t.set_fallbacks(Font::DEFAULT, &[arabic]);
        let text = "Level 3: مرحبا";
        t.with_shaped(text, Font::DEFAULT, |_, s| {
            let latin = s.glyphs.iter().filter(|g| g.font == 0).count();
            let ar: Vec<_> = s.glyphs.iter().filter(|g| g.font == arabic.0).collect();
            assert!(latin >= 8 && !ar.is_empty());
            // The left-to-right base keeps "Level 3:" on the left, the
            // Arabic word after it, its first letter on its right.
            let latin_right = s
                .glyphs
                .iter()
                .filter(|g| g.font == 0 && !g.rtl)
                .map(|g| g.x)
                .fold(0.0, f32::max);
            assert!(ar.iter().all(|g| g.x > latin_right - 0.01 && g.rtl));
            let first = ar.iter().min_by_key(|g| g.cluster).unwrap();
            assert!(ar.iter().all(|g| g.x <= first.x));
        });
    }

    #[test]
    fn missing_glyphs_come_from_the_fallback_chain() {
        let mut t = Text::new();
        let jp = t.add_font(&fixture("NotoSansJP-Subset.otf")).unwrap();
        let without = t.with_shaped("日本語", Font::DEFAULT, |_, s| {
            s.glyphs.iter().filter(|g| g.id == 0).count()
        });
        assert_eq!(without, 3, "the built-in font has no kanji");
        t.set_fallbacks(Font::DEFAULT, &[jp]);
        t.with_shaped("日本語", Font::DEFAULT, |_, s| {
            assert!(s.glyphs.iter().all(|g| g.id != 0 && g.font == jp.0), "{:?}", s.glyphs);
        });
        // A freed fallback is skipped.
        t.free_font(jp);
        let freed = t.with_shaped("日本語", Font::DEFAULT, |_, s| {
            s.glyphs.iter().filter(|g| g.font == 0).count()
        });
        assert_eq!(freed, 3);
    }

    #[test]
    fn three_d_text_is_shaped_into_distance_field_glyphs() {
        let mut t = Text::new();
        let arabic = t.add_font(&fixture("NotoSansArabic-Subset.ttf")).unwrap();
        let size = t.layout_sdf("مرحبا", arabic);
        let glyphs = t.with_shaped("مرحبا", arabic, |_, s| s.glyphs.len());
        assert!(size.x > 0.0 && !t.shaper.sdf_glyphs.is_empty());
        assert!(t.sdf.glyphs.is_empty(), "made by glyph index, not by character");
        assert_eq!(t.placed().len(), glyphs);
        let made = t.sdf.uploads.len();
        t.layout_sdf("مرحبا", arabic);
        assert_eq!(t.sdf.uploads.len(), made, "cached");
    }

    #[test]
    fn japanese_wraps_between_characters_and_latin_at_spaces() {
        let mut t = Text::new();
        let jp = t.add_font(&fixture("NotoSansJP-Subset.otf")).unwrap();
        t.set_fallbacks(Font::DEFAULT, &[jp]);
        let text = "日本語のテキスト";
        let full = t.layout(text, Font::DEFAULT, 20.0, false).x;
        let end = t.wrap_end_shaped(text, Font::DEFAULT, 20.0, full * 0.5);
        // No spaces, yet it breaks between characters, about half way.
        assert!(end > 0 && end < text.len() && text.is_char_boundary(end), "{end}");
        let words = "été hiver";
        let full = t.layout(words, Font::DEFAULT, 20.0, false).x;
        assert_eq!(t.wrap_end_shaped(words, Font::DEFAULT, 20.0, full - 1.0), "été".len());
    }

    #[test]
    fn carets_follow_the_visual_order_of_right_to_left_text() {
        let mut t = Text::new();
        let arabic = t.add_font(&fixture("NotoSansArabic-Subset.ttf")).unwrap();
        let text = "مرحبا";
        let width = t.layout(text, arabic, 20.0, false).x;
        // Before the first letter is the right edge; after the last, the left.
        assert!((t.caret_x_shaped(text, arabic, 20.0, 0) - width).abs() < 0.5);
        assert!(t.caret_x_shaped(text, arabic, 20.0, text.len()).abs() < 0.5);
        let mut previous = width + 1.0;
        for (i, _) in text.char_indices() {
            let x = t.caret_x_shaped(text, arabic, 20.0, i);
            assert!(x < previous, "moving forward moves the caret left");
            previous = x;
        }
        // A click lands on the boundary nearest to it.
        assert_eq!(t.hit_shaped(text, arabic, 20.0, width + 5.0), 0);
        assert_eq!(t.hit_shaped(text, arabic, 20.0, -5.0), text.len());
    }

    #[test]
    fn a_selection_across_a_direction_change_is_two_spans() {
        let mut t = Text::new();
        let arabic = t.add_font(&fixture("NotoSansArabic-Subset.ttf")).unwrap();
        t.set_fallbacks(Font::DEFAULT, &[arabic]);
        // "ab مرحبا cd": select from inside the Latin into the Arabic word.
        let text = "ab مرحبا cd";
        let from = 1;
        let to = "ab مر".len();
        let mut spans = Vec::new();
        t.selection_shaped(text, Font::DEFAULT, 20.0, from..to, &mut spans);
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert!(spans.iter().all(|s| s[1] > s[0]));
    }
}
