//! Attachments from C: `PonkoAttached` and the system that places them.
use ponko::attach::Attached;

use crate::types::{PonkoEntity, PonkoVec2};
use crate::{PonkoGame, PonkoStatus, PonkoWorld, builtin};

/// Keeps an entity's `PonkoTransform2D` position and rotation at `offset`
/// (turned and stretched with it) from entity `to`'s; the built-in
/// `Attached` component, same layout as in Rust, so it works as a query
/// column. Placed by the system `ponko_game_follow_attachments` adds; when
/// `to` is despawned, the attached entity is too.
/// ```
/// assert_eq!(std::mem::size_of::<ponko_ffi::PonkoAttached>(), 20);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoAttached {
    /// The entity to follow.
    pub to: PonkoEntity,
    /// Where, in the followed entity's space.
    pub offset: PonkoVec2,
    /// Added to the followed entity's rotation (radians).
    pub rotation: f32,
}

const _: () = {
    assert!(size_of::<PonkoAttached>() == size_of::<Attached>());
    assert!(align_of::<PonkoAttached>() == align_of::<Attached>());
    assert!(std::mem::offset_of!(PonkoAttached, to) == std::mem::offset_of!(Attached, to));
    assert!(std::mem::offset_of!(PonkoAttached, offset) == std::mem::offset_of!(Attached, offset));
    assert!(std::mem::offset_of!(PonkoAttached, rotation) == std::mem::offset_of!(Attached, rotation));
};

/// The id of the built-in `Attached` component (`PonkoAttached` in C),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_attached_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_attached_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<Attached>(world, out_id) }
}

/// Adds the built-in system that moves attached entities next to theirs, as
/// a simulation system at this point in the order: add it after the
/// systems that move things.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_follow_attachments(std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `game` must be null or a live game.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_follow_attachments(game: *mut PonkoGame) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        crate::game::build(game, |g| {
            crate::game::ok(g.sim_system(ponko::systems::follow_attachments))
        })
    }
}
