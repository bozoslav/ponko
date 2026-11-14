### Added
- Text shaping, the `text-shaping` cargo feature: strings that aren't plain
  ASCII are shaped with harfrust (the HarfBuzz project's Rust port, the
  shaper the inspector's egui already uses): Arabic joining, Devanagari
  conjuncts and vowel reordering, marks, ligatures and OpenType kerning;
  lines are ordered with the Unicode bidirectional algorithm, so
  right-to-left and mixed-direction text reads correctly; wrapped UI text
  breaks where Unicode's line-breaking rules allow (between Japanese
  characters too). It applies to `draw_text`, `measure_text`, retained and
  immediate UI, text fields (caret, clicks and selections follow the
  visual order) and 3D text. ASCII strings keep the per-character path,
  whose cost is unchanged (`text/layout-ascii-400`: 0.152 to 0.155 ms off,
  0.153 to 0.156 ms on, release, Apple M4 Max);
  `Graphics::set_text_shaping(TextShaping::Always)` shapes them too.
  Shaped strings are cached, so redrawing one costs a lookup per glyph.
  The feature adds about 1.3 MB to a window-only release build (desktop
  and web) and 0.5 MB to one with default features. On by default in the
  C library (`ponko_ffi` feature `text-shaping`).
- Font fallback chains: `Graphics::set_font_fallbacks(font, &[..])` and
  `font_fallbacks`; characters a font lacks come from the first font in its
  chain that has them. Works with or without shaping.
- System fonts, the `system-fonts` cargo feature (desktop):
  `Graphics::load_system_font` / `try_load_system_font` by family name, and
  `set_system_font_fallback(true)` to search installed fonts for characters
  no loaded font has. Without it, and on the web, they report
  `ErrorKind::Unsupported`.
- C and C++: `ponko_ctx_set_font_fallbacks`, `ponko_ctx_font_fallbacks`,
  `ponko_ctx_set_text_shaping`, `ponko_ctx_text_shaping`
  (`PonkoTextShaping`), `ponko_ctx_try_load_system_font` and
  `ponko_ctx_set_system_font_fallback`; in C++ `ctx::set_font_fallbacks`,
  `font_fallbacks`, `set_text_shaping`, `text_shaping`,
  `load_system_font`, `try_load_system_font` and
  `set_system_font_fallback`.
- OFL-licensed Noto subsets for Arabic, Devanagari and Japanese in
  `assets/fonts` (a few KB each), used by the tests and the guide.

### Changed
- With `text-shaping`, a retained UI paragraph that reads right to left
  starts at the right edge for `UiTextAlign::Start` (and the left for
  `End`). Text fields keep their text at the left edge.
