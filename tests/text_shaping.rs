//! Font fallback chains, text shaping and system fonts through the public
//! API, headless. The shaping checks need the `text-shaping` feature; the
//! fallback chain works without it.
use ponko::error::ErrorKind;
use ponko::prelude::*;

#[cfg(feature = "text-shaping")]
const ARABIC: &str = "fonts/NotoSansArabic-Subset.ttf";
const JAPANESE: &str = "fonts/NotoSansJP-Subset.otf";

#[test]
fn a_fallback_chain_draws_what_the_font_lacks() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let jp = ctx.gfx.load_font(JAPANESE);
        let boxes = ctx.gfx.measure_text("日本語", 20.0).x;
        ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[jp]);
        assert_eq!(ctx.gfx.font_fallbacks(Font::DEFAULT), &[jp]);
        let kanji = ctx.gfx.measure_text("日本語", 20.0).x;
        // Three full-width characters from the Japanese font: 20 px each.
        assert!((kanji - 60.0).abs() < 1.0, "{kanji}");
        assert_ne!(kanji, boxes, "the built-in font's boxes are another width");
        // Latin text still comes from the built-in font.
        let latin = ctx.gfx.measure_text("Score", 20.0).x;
        ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[]);
        assert_eq!(ctx.gfx.measure_text("Score", 20.0).x, latin);
        assert!(ctx.gfx.font_fallbacks(Font::DEFAULT).is_empty());
    });
}

#[test]
fn the_shaping_mode_is_kept_and_leaves_ascii_widths_close() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        assert_eq!(ctx.gfx.text_shaping(), TextShaping::Auto);
        let fast = ctx.gfx.measure_text("Wave 12: Office", 24.0).x;
        ctx.gfx.set_text_shaping(TextShaping::Always);
        assert_eq!(ctx.gfx.text_shaping(), TextShaping::Always);
        let shaped = ctx.gfx.measure_text("Wave 12: Office", 24.0).x;
        if cfg!(feature = "text-shaping") {
            // The same font's advances; only kerning and ligatures differ.
            assert!((fast - shaped).abs() < 0.05 * fast, "{fast} {shaped}");
        } else {
            assert_eq!(fast, shaped, "nothing is shaped without the feature");
        }
    });
}

#[cfg(feature = "text-shaping")]
#[test]
fn arabic_through_a_fallback_is_shaped_narrower_than_its_isolated_letters() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let arabic = ctx.gfx.load_font(ARABIC);
        ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[arabic]);
        let joined = ctx.gfx.measure_text("مرحبا بالعالم", 24.0).x;
        let apart: String = "مرحبا بالعالم".chars().flat_map(|c| [c, '\u{200C}']).collect();
        let isolated = ctx.gfx.measure_text(&apart, 24.0).x;
        assert!(joined > 50.0 && joined < isolated, "{joined} {isolated}");
    });
}

#[test]
fn system_fonts_report_why_they_are_missing() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let missing = ctx.gfx.try_load_system_font("No Such Family 409");
        let can_search = ctx.gfx.set_system_font_fallback(false);
        if cfg!(all(feature = "system-fonts", not(target_arch = "wasm32"))) {
            assert!(can_search);
            assert_eq!(missing.unwrap_err().kind(), ErrorKind::NotFound);
        } else {
            assert!(!can_search);
            let err = missing.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Unsupported);
            assert!(err.message().contains("system-fonts"), "{}", err.message());
        }
    });
}
