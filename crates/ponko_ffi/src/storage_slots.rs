//! Save slots and the state of the player's storage (Rust
//! `Storage::world_slots`, `Storage::delete_world`, `Storage::is_persistent`,
//! `Storage::is_saving`).
use crate::ctx::{ctx_access, out, text};
use crate::perf::write_text;
use crate::{PonkoCtx, PonkoStatus, boundary, engine_error, invalid};
use std::ffi::c_char;

/// The save slots `ponko_ctx_save_world` wrote, sorted, one per line (`\n`
/// between them), as NUL-terminated text: `*out_len` is always the length;
/// the text is copied only when it fits.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_storage_world_slots(std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `buffer` writable for `capacity` bytes (or null with
/// capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_storage_world_slots(
    ctx: *mut PonkoCtx,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let slots = ctx_access(ctx, |c| c.storage.world_slots().map_err(engine_error))?;
        if out_len.is_null() || (buffer.is_null() && capacity > 0) {
            return Err(invalid("buffer or out_len is null"));
        }
        write_text(buffer, capacity, out_len, &slots.join("\n"));
        Ok(())
    })
}

/// Deletes save slot `slot` and its backup; a missing slot is fine.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_delete_world(std::ptr::null_mut(), c"slot1".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `slot` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_delete_world(ctx: *mut PonkoCtx, slot: *const c_char) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| c.storage.delete_world(text(slot)?).map_err(engine_error)) })
}

/// Whether stored files outlive the run: `true` for a folder, and on the web
/// when the page's database opened (some private windows refuse it; files
/// then last only until the page closes). `false` without an app id.
/// ```
/// let mut p = true;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_storage_is_persistent(std::ptr::null_mut(), &mut p) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_persistent` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_storage_is_persistent(ctx: *mut PonkoCtx, out_persistent: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_persistent, c.storage.is_persistent())) })
}

/// Whether writes are still on their way to the disk: on the web, while the
/// browser hasn't confirmed every stored change; always `false` on the
/// desktop, where a write returns once the file is there. A game can show
/// "saving..." while it is `true`.
/// ```
/// let mut s = true;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_storage_is_saving(std::ptr::null_mut(), &mut s) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_saving` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_storage_is_saving(ctx: *mut PonkoCtx, out_saving: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_saving, c.storage.is_saving())) })
}
