//! GPU buffers that keep their last frame, from C (`Graphics::set_gpu_buffer_motion`).
mod common;

use PonkoStatus::*;
use common::{last_error, run_ctx};
use ponko_ffi::*;
use std::ptr::{null, null_mut};

#[test]
fn a_buffer_keeps_its_last_frame_and_a_freed_one_is_refused() {
    run_ctx(|ctx| unsafe {
        let mut b = PonkoGpuBuffer { id: 0 };
        assert_eq!(ponko_ctx_create_gpu_buffer(ctx, 4, null(), 0, &mut b), PONKO_OK);
        assert_eq!(
            ponko_ctx_set_gpu_buffer_motion(ctx, b, true),
            PONKO_OK,
            "{}",
            last_error()
        );
        assert_eq!(ponko_ctx_set_gpu_buffer_motion(ctx, b, false), PONKO_OK);
        assert_eq!(ponko_ctx_free_gpu_buffer(ctx, b), PONKO_OK);
        assert_eq!(ponko_ctx_set_gpu_buffer_motion(ctx, b, true), PONKO_INVALID_ARGUMENT);
        assert!(last_error().contains("invalid GPU buffer"), "{}", last_error());
    });
    let b = PonkoGpuBuffer { id: 0 };
    assert_eq!(
        unsafe { ponko_ctx_set_gpu_buffer_motion(null_mut(), b, true) },
        PONKO_INVALID_ARGUMENT
    );
}
