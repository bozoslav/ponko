//! The retained UI's focus set from code, and HUDs under the screens (Rust's
//! `Ui::set_focus`, `Ui::clear_focus`, `Ui::set_initial_focus`,
//! `Ui::show_hud`, `Ui::hide_hud` and `Ui::is_hud`).

use crate::ctx::out;
use crate::ui_retained::{existing, with_ui};
use crate::{PonkoCtx, PonkoStatus, boundary, invalid};

/// Moves the keyboard and gamepad focus to node `id`, as Tab or a click
/// would: Enter (or a gamepad's South button) then activates it. Only an
/// enabled, shown button, text input, toggle or slider on the top screen can
/// have it; anything else fails with `PONKO_INVALID_ARGUMENT` and leaves the
/// focus as it was. `PONKO_NOT_FOUND` for a node that doesn't exist.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_set_focus(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_set_focus(ctx: *mut PonkoCtx, target: u64, id: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_ui(ctx, target, |ui| {
            let id = existing(ui, id)?;
            if ui.set_focus(id) {
                Ok(())
            } else {
                Err(invalid(
                    "only an enabled, shown button, text input, toggle or slider on the top screen takes the focus",
                ))
            }
        })
    })
}

/// Takes the focus away: no node has it until Tab, an arrow key, a click or
/// `ponko_ctx_ui_set_focus`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_clear_focus(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_clear_focus(ctx: *mut PonkoCtx, target: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_ui(ctx, target, |ui| {
            ui.clear_focus();
            Ok(())
        })
    })
}

/// Marks node `id` as the one its screen opens on (or unmarks it, `on`
/// false): every `ponko_ctx_ui_push_screen` of the screen focuses it, so a
/// menu opens ready for Enter, and opens on it again when it is pushed
/// again. If several nodes of a screen are marked, the first (depth-first)
/// that can take the focus wins. `PONKO_NOT_FOUND` for a node that doesn't
/// exist.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_set_initial_focus(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0, true) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_set_initial_focus(
    ctx: *mut PonkoCtx,
    target: u64,
    id: u64,
    on: bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_ui(ctx, target, |ui| {
            let id = existing(ui, id)?;
            ui.set_initial_focus(id, on);
            Ok(())
        })
    })
}

/// Shows the tree under a detached `root` as a HUD: display-only UI (a
/// score, a health bar) drawn below every screen. Escape (or a gamepad's
/// East button) never closes it, it takes no pointer or focus, and it
/// doesn't make `ponko_ctx_ui_wants_keyboard` true. Showing it again
/// changes nothing. `PONKO_INVALID_ARGUMENT` for a node with a parent or one
/// on the screen stack; `PONKO_NOT_FOUND` for a node that doesn't exist.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_show_hud(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_show_hud(ctx: *mut PonkoCtx, target: u64, root: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_ui(ctx, target, |ui| {
            let root = existing(ui, root)?;
            if ui.parent(root).is_some() {
                return Err(invalid("UI HUD must be a detached root"));
            }
            if ui.is_screen(root) {
                return Err(invalid("UI HUD is already a screen"));
            }
            ui.show_hud(root);
            Ok(())
        })
    })
}

/// Takes a HUD off the screen; its nodes stay, so it can be shown again.
/// Does nothing for a node that isn't a HUD. `PONKO_NOT_FOUND` for a node
/// that doesn't exist.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_hide_hud(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_hide_hud(ctx: *mut PonkoCtx, target: u64, root: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_ui(ctx, target, |ui| {
            let root = existing(ui, root)?;
            ui.hide_hud(root);
            Ok(())
        })
    })
}

/// Writes whether node `id` is a HUD's root right now.
/// ```
/// let mut hud = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_is_hud(std::ptr::null_mut(), ponko_ffi::PONKO_UI_SCREEN, 0, &mut hud) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_hud` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_is_hud(
    ctx: *mut PonkoCtx,
    target: u64,
    id: u64,
    out_hud: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_ui(ctx, target, |ui| {
            let id = existing(ui, id)?;
            out(out_hud, ui.is_hud(id))
        })
    })
}
