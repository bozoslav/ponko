//! GPU buffers that keep their last frame for the motion pass, from C
//! (Rust `Graphics::set_gpu_buffer_motion`).
use ponko::__ffi;
use ponko::render::GpuBuffer;

use crate::ctx::ctx_access;
use crate::{PonkoCtx, PonkoGpuBuffer, PonkoStatus, boundary, invalid};

/// Makes `buffer` keep a copy of the elements the last frame drew (`keep`),
/// or stop (Rust `Graphics::set_gpu_buffer_motion`): a material's `vertex`
/// reading `items[i]` then reads that copy when the motion pass (TAA,
/// temporal AO) runs it as of the last frame, so each element writes its
/// own motion. It doubles the buffer's memory and copies it once a frame
/// on the GPU while motion vectors are on. An invalid or freed buffer is
/// an error.
/// ```
/// let b = ponko_ffi::PonkoGpuBuffer { id: 0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_gpu_buffer_motion(std::ptr::null_mut(), b, true) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_gpu_buffer_motion(
    ctx: *mut PonkoCtx,
    buffer: PonkoGpuBuffer,
    keep: bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let b = __ffi::from_id::<GpuBuffer>(buffer.id);
            c.gfx.gpu_buffer_len(b).ok_or_else(|| invalid("invalid GPU buffer"))?;
            c.gfx.set_gpu_buffer_motion(b, keep);
            Ok(())
        })
    })
}
