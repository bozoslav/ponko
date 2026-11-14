//! Motion vectors from C: dropping one model's motion for a frame (Rust
//! `Graphics::reset_motion`).
use ponko::prelude::Entity;

use crate::ctx::ctx_access;
use crate::{PonkoCtx, PonkoStatus, boundary, invalid};

/// Gives `entity`'s model no motion on the next drawn frame (Rust
/// `Graphics::reset_motion`), keeping the rest of the picture's temporal
/// history: call it after teleporting one object, or when its animation
/// snaps to another pose with no blend, so TAA and temporal AO don't smear
/// it along a path it never took. For a LOD group pass the group's entity;
/// for a mesh posed through another entity's skeleton, the mesh's. A stale
/// entity is an error.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_reset_motion(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_reset_motion(ctx: *mut PonkoCtx, entity: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            c.gfx.reset_motion(entity);
            Ok(())
        })
    })
}
