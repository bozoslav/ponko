//! The ECS diagnostics snapshot from C (Rust `Ctx::ecs_diagnostics`).
use crate::ctx::ctx_access;
use crate::perf::write_text;
use crate::{PonkoCtx, PonkoStatus, boundary, invalid, invalid_owned};
use std::ffi::c_char;

/// A snapshot of the world and the schedule as JSON (Rust
/// `Ctx::ecs_diagnostics`, the same as the `ecs.diagnostics` control
/// method): entity count and memory, archetypes, components, and every
/// system with its phase, declared access, stage and timings. Writes it to
/// `buffer` (NUL-terminated when it fits in `capacity`) and its length to
/// `*out_len`; call with a null buffer and capacity 0 to learn the length.
/// ```
/// let mut n = 0;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ecs_diagnostics(std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `buffer` writable for `capacity` bytes (or null with
/// capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ecs_diagnostics(
    ctx: *mut PonkoCtx,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let json = ctx_access(ctx, |c| {
            serde_json::to_string(&c.ecs_diagnostics()).map_err(|e| invalid_owned(e.to_string()))
        })?;
        if out_len.is_null() || (buffer.is_null() && capacity > 0) {
            return Err(invalid("buffer or out_len is null"));
        }
        write_text(buffer, capacity, out_len, &json);
        Ok(())
    })
}
