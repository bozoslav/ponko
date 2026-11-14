//! The system's fonts (feature `system-fonts`, desktop only), through
//! fontdb: a font by family name, and a font for a character no loaded
//! font has. The font folders are read on first use, not at start-up.

use std::collections::HashMap;

use read_fonts::TableProvider;

use super::{Font, Text};
use crate::error::{Error, ErrorKind};

#[derive(Default)]
pub(crate) struct SystemFonts {
    /// The system's font list, read on first use.
    db: Option<fontdb::Database>,
    /// Faces already loaded as engine fonts.
    loaded: HashMap<fontdb::ID, Font>,
    /// The system font found for a character (`None`: no system font has it).
    by_char: HashMap<char, Option<Font>>,
    /// Characters no font has are looked up among the system's fonts.
    pub fallback: bool,
}

impl SystemFonts {
    fn db(&mut self) -> &fontdb::Database {
        self.db.get_or_insert_with(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            db
        })
    }
}

/// Whether face `index` of `data` maps `c` to a glyph.
fn covers(data: &[u8], index: u32, c: char) -> bool {
    read_fonts::FontRef::from_index(data, index)
        .ok()
        .and_then(|f| f.cmap().ok())
        .and_then(|cmap| cmap.map_codepoint(c))
        .is_some_and(|g| g.to_u32() != 0)
}

impl Text {
    /// Loads face `id` of the system's font list as an engine font (once).
    fn load_system_face(&mut self, id: fontdb::ID) -> Result<Font, Error> {
        if let Some(&font) = self.shaper.system.loaded.get(&id)
            && self.has_font(font)
        {
            return Ok(font);
        }
        let db = self.shaper.system.db();
        let (bytes, index) = db
            .with_face_data(id, |data, index| (data.to_vec(), index))
            .ok_or_else(|| Error::new(ErrorKind::Io, "the system font could not be read"))?;
        let font = self.add_font_at(&bytes, index)?;
        self.shaper.system.loaded.insert(id, font);
        Ok(font)
    }

    /// The system font of `family` ("Noto Sans Arabic", "Arial"), regular
    /// weight and style if it has one.
    pub(crate) fn load_system_font(&mut self, family: &str) -> Result<Font, Error> {
        let db = self.shaper.system.db();
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            ..Default::default()
        };
        let id = db.query(&query).ok_or_else(|| {
            Error::new(
                ErrorKind::NotFound,
                format!("no system font of the family \"{family}\""),
            )
            .with_path(family)
        })?;
        self.load_system_face(id)
    }

    /// Turns the search of system fonts for missing characters on or off.
    pub(crate) fn set_system_fallback(&mut self, on: bool) {
        self.shaper.system.fallback = on;
        self.shaper.forget_shapes();
    }

    /// A system font with a glyph for `c`, appended to `font`'s fallback
    /// chain so the next characters find it there; `None` if the search is
    /// off or no system font has it. Each character is searched once.
    pub(super) fn system_font_for(&mut self, font: Font, c: char) -> Option<Font> {
        if !self.shaper.system.fallback || c.is_control() || c.is_whitespace() {
            return None;
        }
        let found = match self.shaper.system.by_char.get(&c) {
            Some(found) => *found,
            None => {
                let db = self.shaper.system.db();
                // Regular faces first, then any.
                let regular =
                    |f: &&fontdb::FaceInfo| f.style == fontdb::Style::Normal && f.weight == fontdb::Weight::NORMAL;
                let pick = |only_regular: bool| {
                    db.faces()
                        .filter(|f| !only_regular || regular(f))
                        .find(|f| {
                            db.with_face_data(f.id, |data, index| covers(data, index, c))
                                .unwrap_or(false)
                        })
                        .map(|f| f.id)
                };
                let id = pick(true).or_else(|| pick(false));
                let found = id.and_then(|id| self.load_system_face(id).ok());
                self.shaper.system.by_char.insert(c, found);
                found
            }
        };
        let found = found.filter(|&f| self.has_font(f))?;
        let i = font.0 as usize;
        if self.fallbacks.len() <= i {
            self.fallbacks.resize_with(i + 1, Vec::new);
        }
        if !self.fallbacks[i].contains(&found) && found != font {
            self.fallbacks[i].push(found);
        }
        Some(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Text` whose "system" fonts are the ones in `assets/fonts`.
    fn text_with_test_fonts() -> Text {
        let mut t = Text::new();
        let mut db = fontdb::Database::new();
        db.load_fonts_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts"));
        assert_eq!(db.len(), 3);
        t.shaper.system.db = Some(db);
        t
    }

    #[test]
    fn a_family_is_found_by_name_and_loaded_once() {
        let mut t = text_with_test_fonts();
        let arabic = t.load_system_font("Noto Sans Arabic").unwrap();
        assert_eq!(t.load_system_font("Noto Sans Arabic").unwrap(), arabic);
        assert_eq!(t.loaded_fonts(), 1);
        let err = t.load_system_font("Missing Family").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn missing_characters_are_found_in_system_fonts_only_when_asked() {
        let mut t = text_with_test_fonts();
        let unshaped = |t: &mut Text| t.with_shaped("日本", Font::DEFAULT, |_, s| s.glyphs.iter().all(|g| g.id == 0));
        assert!(unshaped(&mut t), "off by default");
        t.set_system_fallback(true);
        t.with_shaped("日本", Font::DEFAULT, |_, s| {
            assert!(s.glyphs.iter().all(|g| g.id != 0 && g.font != 0), "{:?}", s.glyphs);
        });
        // The font found for the first character joined the chain, so the
        // second was found there without another search.
        assert_eq!(t.fallbacks(Font::DEFAULT).len(), 1);
        assert_eq!(t.shaper.system.by_char.len(), 1);
        // A character no font has is remembered as missing.
        t.with_shaped("ก", Font::DEFAULT, |_, _| ());
        assert_eq!(t.shaper.system.by_char.get(&'ก'), Some(&None));
    }
}
