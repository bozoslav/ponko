//! The immediate widgets' font, border and scale (Rust `UiImmediate::font`,
//! `UiImmediate::border_width`, `UiImmediate::scale`).
use crate::ctx::{ctx_access, out};
use crate::media::{PonkoFont, font_from_c, font_to_c};
use crate::{PonkoCtx, PonkoStatus, boundary, invalid};

/// The font of the immediate labels and buttons (Rust
/// `ctx.ui.immediate.font`): the screen's theme font unless set with
/// `ponko_ctx_set_ui_immediate_font`.
/// ```
/// let mut f = ponko_ffi::PonkoFont::default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_immediate_font(std::ptr::null_mut(), &mut f) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_font` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_immediate_font(ctx: *mut PonkoCtx, out_font: *mut PonkoFont) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_font, font_to_c(c.ui.immediate.font))) })
}

/// Sets the font of the immediate labels and buttons (from
/// `ponko_ctx_load_font`; `{0}` the built-in one). It stays through a later
/// theme change. A font this game didn't load is an error.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_ui_immediate_font(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_ui_immediate_font(ctx: *mut PonkoCtx, font: PonkoFont) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let font = font_from_c(font);
            if !c.gfx.is_live(font) {
                return Err(invalid("that font wasn't loaded by this game"));
            }
            c.ui.immediate.font = font;
            Ok(())
        })
    })
}

/// The thickness of an immediate button's border in UI units, drawn in the
/// accent color (Rust `ctx.ui.immediate.border_width`): the theme's border
/// width when it sets one, 0 otherwise.
/// ```
/// let mut w = 1.0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_immediate_border_width(std::ptr::null_mut(), &mut w) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_width` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_immediate_border_width(ctx: *mut PonkoCtx, out_width: *mut f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_width, c.ui.immediate.border_width)) })
}

/// Sets the thickness of an immediate button's border, in UI units (0: no
/// border, only the outline of a hovered button). Negative or non-finite
/// widths are an error.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_ui_immediate_border_width(std::ptr::null_mut(), 2.0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_ui_immediate_border_width(ctx: *mut PonkoCtx, width: f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if !width.is_finite() || width < 0.0 {
                return Err(invalid("border width must be finite and not negative"));
            }
            c.ui.immediate.border_width = width;
            Ok(())
        })
    })
}

/// The scale the immediate widgets are drawn at (Rust
/// `UiImmediate::scale`): the screen's `ponko_ctx_ui_set_scale`, 1 by
/// default. Multiply a size in UI units by it for window pixels.
/// ```
/// let mut s = 0.0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_immediate_scale(std::ptr::null_mut(), &mut s) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_scale` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_immediate_scale(ctx: *mut PonkoCtx, out_scale: *mut f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_scale, c.ui.immediate.scale())) })
}
