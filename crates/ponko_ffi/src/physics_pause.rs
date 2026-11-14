//! Holding only the 2D physics step (the `physics` feature): a pause screen
//! whose own systems keep running. Without the feature both calls fail with
//! an error naming it.
use crate::PonkoStatus;

/// Holds the 2D physics step (`paused` true) or lets it run again, while
/// every other system keeps running and the tick counter goes on. While
/// held, bodies keep their transforms and velocities, no collision events
/// are sent, and ray and overlap queries see the last step's state.
/// # Safety
/// `ctx` must be null or live, outside query callbacks.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_physics_paused(std::ptr::null_mut(), true) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_physics_paused(ctx: *mut crate::PonkoCtx, paused: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::set_paused(ctx, paused) }
}

/// Whether `ponko_ctx_set_physics_paused` holds the 2D physics step.
/// # Safety
/// `ctx` must be null or live, outside query callbacks; `out_paused` writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_is_physics_paused(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_is_physics_paused(ctx: *mut crate::PonkoCtx, out_paused: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::is_paused(ctx, out_paused) }
}

#[cfg(feature = "physics")]
mod imp {
    use crate::ctx::{ctx_access, out};
    use crate::{PonkoCtx, PonkoStatus, boundary};

    pub(super) unsafe fn set_paused(ctx: *mut PonkoCtx, paused: bool) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        boundary(|| unsafe {
            ctx_access(ctx, |c| {
                c.physics.set_paused(paused);
                Ok(())
            })
        })
    }

    pub(super) unsafe fn is_paused(ctx: *mut PonkoCtx, out_paused: *mut bool) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        boundary(|| unsafe { ctx_access(ctx, |c| out(out_paused, c.physics.is_paused())) })
    }
}

#[cfg(not(feature = "physics"))]
mod imp {
    //! Without the feature: every call fails, saying why.
    use crate::{PonkoCtx, PonkoStatus, boundary, invalid};

    fn off() -> Result<(), crate::Failure> {
        Err(invalid("ponko_ffi was built without the `physics` feature"))
    }
    pub(super) unsafe fn set_paused(_: *mut PonkoCtx, _: bool) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn is_paused(_: *mut PonkoCtx, _: *mut bool) -> PonkoStatus {
        boundary(off)
    }
}
