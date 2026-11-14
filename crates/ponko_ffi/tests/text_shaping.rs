//! Font fallback chains, the shaping mode and system fonts through the C
//! API (#409).
use ponko_ffi::*;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

use PonkoStatus::*;

fn run(system: PonkoSystemFn, user: *mut c_void) {
    unsafe {
        let game = ponko_game_new(c"text".as_ptr(), 1, 1);
        assert_eq!(ponko_game_sim_system(game, c"s".as_ptr(), 1, system, user), PONKO_OK);
        let mut world = null_mut();
        assert_eq!(ponko_game_run_headless(game, 1, None, null_mut(), &mut world), PONKO_OK);
        ponko_world_free(world);
    }
}

/// The Japanese test font, from an in-memory asset source.
unsafe fn japanese(ctx: *mut PonkoCtx) -> PonkoFont {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/NotoSansJP-Subset.otf"
    ))
    .unwrap();
    unsafe {
        let source = ponko_memory_source_new(c"fonts".as_ptr());
        assert_eq!(
            ponko_memory_source_insert(source, c"jp.otf".as_ptr(), bytes.as_ptr(), bytes.len()),
            PONKO_OK
        );
        assert_eq!(ponko_ctx_add_memory_source(ctx, source), PONKO_OK);
        ponko_memory_source_free(source);
        let mut font = PonkoFont::default();
        assert_eq!(ponko_ctx_try_load_font(ctx, c"jp.otf".as_ptr(), &mut font), PONKO_OK);
        font
    }
}

#[derive(Default)]
struct Seen {
    boxes: f32,
    kanji: f32,
    chain: Vec<PonkoFont>,
    jp: PonkoFont,
}

unsafe extern "C" fn fallbacks(ctx: *mut PonkoCtx, user: *mut c_void) {
    let seen = unsafe { &mut *user.cast::<Seen>() };
    let builtin = PonkoFont { id: 0 };
    let measure = |ctx| {
        let mut size = PonkoVec2::default();
        assert_eq!(
            unsafe { ponko_ctx_measure_text_ex(ctx, c"日本語".as_ptr(), builtin, 20.0, &mut size) },
            PONKO_OK
        );
        size.x
    };
    unsafe {
        seen.jp = japanese(ctx);
        seen.boxes = measure(ctx);
        assert_eq!(ponko_ctx_set_font_fallbacks(ctx, builtin, &seen.jp, 1), PONKO_OK);
        seen.kanji = measure(ctx);
        let mut n = 0;
        assert_eq!(ponko_ctx_font_fallbacks(ctx, builtin, null_mut(), 0, &mut n), PONKO_OK);
        seen.chain = vec![PonkoFont::default(); n];
        assert_eq!(
            ponko_ctx_font_fallbacks(ctx, builtin, seen.chain.as_mut_ptr(), n, &mut n),
            PONKO_OK
        );
        // A font the game never loaded changes nothing.
        let unknown = PonkoFont { id: 999 };
        assert_eq!(
            ponko_ctx_set_font_fallbacks(ctx, builtin, &unknown, 1),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(
            ponko_ctx_set_font_fallbacks(ctx, builtin, null(), 1),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(ponko_ctx_font_fallbacks(ctx, builtin, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 1, "the chain set before stays");
        assert_eq!(ponko_ctx_set_font_fallbacks(ctx, builtin, null(), 0), PONKO_OK);
        assert_eq!(ponko_ctx_font_fallbacks(ctx, builtin, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 0);
    }
}

#[test]
fn a_fallback_chain_set_from_c_draws_what_the_font_lacks() {
    let mut seen = Seen::default();
    run(Some(fallbacks), (&raw mut seen).cast());
    assert_eq!(seen.chain, [seen.jp]);
    assert!(
        (seen.kanji - 60.0).abs() < 1.0,
        "three 20 px characters: {}",
        seen.kanji
    );
    assert_ne!(seen.kanji, seen.boxes);
}

unsafe extern "C" fn shaping(ctx: *mut PonkoCtx, user: *mut c_void) {
    let modes = unsafe { &mut *user.cast::<Vec<u32>>() };
    unsafe {
        let mut mode = 9;
        assert_eq!(ponko_ctx_text_shaping(ctx, &mut mode), PONKO_OK);
        modes.push(mode);
        assert_eq!(
            ponko_ctx_set_text_shaping(ctx, PonkoTextShaping::PONKO_TEXT_SHAPING_ALWAYS as u32),
            PONKO_OK
        );
        assert_eq!(ponko_ctx_text_shaping(ctx, &mut mode), PONKO_OK);
        modes.push(mode);
        assert_eq!(ponko_ctx_set_text_shaping(ctx, 7), PONKO_INVALID_ARGUMENT);
        assert_eq!(ponko_ctx_text_shaping(ctx, &mut mode), PONKO_OK);
        modes.push(mode);
    }
}

#[test]
fn the_shaping_mode_round_trips_and_rejects_unknown_values() {
    let mut modes: Vec<u32> = Vec::new();
    run(Some(shaping), (&raw mut modes).cast());
    assert_eq!(modes, [0, 1, 1]);
}

unsafe extern "C" fn system_fonts(ctx: *mut PonkoCtx, user: *mut c_void) {
    let seen = unsafe { &mut *user.cast::<(PonkoStatus, u32, bool)>() };
    unsafe {
        let mut font = PonkoFont { id: 77 };
        seen.0 = ponko_ctx_try_load_system_font(ctx, c"No Such Family 409".as_ptr(), &mut font);
        seen.1 = font.id;
        assert_eq!(ponko_ctx_set_system_font_fallback(ctx, false, &mut seen.2), PONKO_OK);
        assert_eq!(ponko_ctx_set_system_font_fallback(ctx, false, null_mut()), PONKO_OK);
    }
}

#[test]
fn a_missing_system_font_is_a_failure_that_leaves_the_output_alone() {
    let mut seen = (PONKO_OK, 0, true);
    run(Some(system_fonts), (&raw mut seen).cast());
    assert_ne!(seen.0, PONKO_OK);
    assert_eq!(seen.1, 77, "unchanged on failure");
    // The library's default features have no system-fonts.
    assert_eq!(seen.2, cfg!(feature = "system-fonts"));
}
