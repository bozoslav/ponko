//! 3D attachments from C: `PonkoAttached3D`. The system that places them is
//! the 2D one's, `ponko_game_follow_attachments`: it places both.
use ponko::attach::Attached3D;

use crate::types::PonkoEntity;
use crate::{PonkoStatus, PonkoTransform3D, PonkoWorld, builtin};

/// Keeps an entity's `PonkoTransform3D` at `offset` (in entity `to`'s
/// space: turned, stretched and moved with it) from `to`'s; the built-in
/// `Attached3D` component, same layout as in Rust, so it works as a query
/// column. Chains place parents first. Placed by the system
/// `ponko_game_follow_attachments` adds; when `to` is despawned, the
/// attached entity is too.
/// ```
/// assert_eq!(std::mem::size_of::<ponko_ffi::PonkoAttached3D>(), 64);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoAttached3D {
    /// The entity to follow.
    pub to: PonkoEntity,
    /// Unused; keeps `offset` where Rust has it.
    pub reserved: [u32; 2],
    /// Where, in the followed entity's space.
    pub offset: PonkoTransform3D,
}

const _: () = {
    assert!(size_of::<PonkoAttached3D>() == size_of::<Attached3D>());
    assert!(std::mem::offset_of!(PonkoAttached3D, to) == std::mem::offset_of!(Attached3D, to));
    assert!(std::mem::offset_of!(PonkoAttached3D, offset) == std::mem::offset_of!(Attached3D, offset));
};

/// The id of the built-in `Attached3D` component (`PonkoAttached3D` in C),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_attached3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_attached3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<Attached3D>(world, out_id) }
}
