//! Font fallback chains, text shaping and system fonts (#409): the C side
//! of Rust's `Graphics::set_font_fallbacks`, `font_fallbacks`,
//! `set_text_shaping`, `text_shaping`, `try_load_system_font` and
//! `set_system_font_fallback`. Shaping needs the library's `text-shaping`
//! feature (on by default), system fonts its `system-fonts` feature.
use std::ffi::c_char;

use ponko::prelude::{Font, TextShaping};

use crate::ctx::{ctx_access, out, text};
use crate::media::{PonkoFont, font_from_c, font_to_c};
use crate::{PonkoCtx, PonkoStatus, boundary, engine_error, invalid};

/// Which strings are shaped (Rust's `TextShaping`), for
/// `ponko_ctx_set_text_shaping`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum PonkoTextShaping {
    /// ASCII strings keep the per-character fast path; every other string
    /// is shaped. The default.
    PONKO_TEXT_SHAPING_AUTO = 0,
    /// Every string is shaped, ASCII ones too (ligatures and OpenType
    /// kerning in Latin text).
    PONKO_TEXT_SHAPING_ALWAYS = 1,
}

/// Sets the fonts tried, in order, for characters `font` has no glyph for
/// (Rust's `gfx.set_font_fallbacks`): `count` fonts from `fallbacks`, each
/// from `ponko_ctx_load_font` or `ponko_ctx_try_load_system_font`. Replaces
/// the chain set before; `count` 0 turns fallback off. A font this game
/// didn't load is an error and changes nothing.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_font_fallbacks(std::ptr::null_mut(), Default::default(), std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `fallbacks` readable for `count` fonts (may be null
/// when `count` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_font_fallbacks(
    ctx: *mut PonkoCtx,
    font: PonkoFont,
    fallbacks: *const PonkoFont,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if fallbacks.is_null() && count > 0 {
                return Err(invalid("fallbacks is null"));
            }
            let chain: Vec<Font> = if count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(fallbacks, count)
                    .iter()
                    .map(|&f| font_from_c(f))
                    .collect()
            };
            let font = font_from_c(font);
            if !c.gfx.is_live(font) || chain.iter().any(|&f| !c.gfx.is_live(f)) {
                return Err(invalid("that font wasn't loaded by this game"));
            }
            c.gfx.set_font_fallbacks(font, &chain);
            Ok(())
        })
    })
}

/// `font`'s fallback chain (Rust's `gfx.font_fallbacks`), with the system
/// fonts found for it: writes up to `capacity` fonts to `out_fonts` and how
/// many there are to `*out_count`.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_font_fallbacks(std::ptr::null_mut(), Default::default(), std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_fonts` writable for `capacity` fonts (or null
/// with capacity 0); `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_font_fallbacks(
    ctx: *mut PonkoCtx,
    font: PonkoFont,
    out_fonts: *mut PonkoFont,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_count.is_null() || (out_fonts.is_null() && capacity > 0) {
                return Err(invalid("output is null"));
            }
            let chain = c.gfx.font_fallbacks(font_from_c(font));
            for (i, &f) in chain.iter().take(capacity).enumerate() {
                out_fonts.add(i).write(font_to_c(f));
            }
            out(out_count, chain.len())
        })
    })
}

/// Sets which strings are shaped (Rust's `gfx.set_text_shaping`): a
/// `PonkoTextShaping` value. Without the library's `text-shaping` feature
/// nothing is shaped and the setting changes nothing. Another value is an
/// error.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_text_shaping(std::ptr::null_mut(), 1) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_text_shaping(ctx: *mut PonkoCtx, mode: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let shaping = match mode {
                m if m == PonkoTextShaping::PONKO_TEXT_SHAPING_AUTO as u32 => TextShaping::Auto,
                m if m == PonkoTextShaping::PONKO_TEXT_SHAPING_ALWAYS as u32 => TextShaping::Always,
                _ => return Err(invalid("not a PonkoTextShaping value")),
            };
            c.gfx.set_text_shaping(shaping);
            Ok(())
        })
    })
}

/// Which strings are shaped (Rust's `gfx.text_shaping`), as a
/// `PonkoTextShaping` value.
/// ```
/// let mut m = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_text_shaping(std::ptr::null_mut(), &mut m) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_mode` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_text_shaping(ctx: *mut PonkoCtx, out_mode: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let mode = match c.gfx.text_shaping() {
                TextShaping::Auto => PonkoTextShaping::PONKO_TEXT_SHAPING_AUTO,
                TextShaping::Always => PonkoTextShaping::PONKO_TEXT_SHAPING_ALWAYS,
            };
            out(out_mode, mode as u32)
        })
    })
}

/// Loads a font installed on the computer by family name (Rust's
/// `gfx.try_load_system_font`), its regular face if it has one. A family
/// no installed font has returns `PONKO_NOT_FOUND`; without the library's
/// `system-fonts` feature, and on the web, the call fails with
/// `ponko_last_error_kind()` `PONKO_ERROR_UNSUPPORTED`. `*out_font` is
/// unchanged on failure.
/// ```
/// let mut f = ponko_ffi::PonkoFont { id: 0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_try_load_system_font(std::ptr::null_mut(), c"Arial".as_ptr(), &mut f) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `family` NUL-terminated; `out_font` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_try_load_system_font(
    ctx: *mut PonkoCtx,
    family: *const c_char,
    out_font: *mut PonkoFont,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let family = text(family)?;
            if out_font.is_null() {
                return Err(invalid("output is null"));
            }
            let font = c.gfx.try_load_system_font(family).map_err(engine_error)?;
            out(out_font, font_to_c(font))
        })
    })
}

/// With `on`, a character that neither a font nor its fallback chain has
/// is looked up among the computer's fonts and the first with a glyph for
/// it joins the chain (Rust's `gfx.set_system_font_fallback`). Writes
/// whether this build can search system fonts (the `system-fonts` feature,
/// desktop) to `*out_available`, if not null.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_system_font_fallback(std::ptr::null_mut(), true, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_available` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_system_font_fallback(
    ctx: *mut PonkoCtx,
    on: bool,
    out_available: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let available = c.gfx.set_system_font_fallback(on);
            if !out_available.is_null() {
                out(out_available, available)?;
            }
            Ok(())
        })
    })
}
