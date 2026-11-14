//! A glTF file made solid in 3D physics in one call (Rust's
//! `Ctx::load_collision_mesh` and `Ctx::load_collision_mesh_with`).

use std::ffi::c_char;

use crate::physics3d::PonkoCollisionMesh;
use crate::three::PonkoVec3;
use crate::{PonkoCtx, PonkoStatus, boundary};

/// Makes the glTF file at `path` (under `assets/`) a collision mesh for a
/// mesh collider, as a scene's `{"Mesh": {"file": ...}}` collider does: its
/// triangles scaled by `scale` (`{1, 1, 1}` for the file as it is), or a
/// convex hull around its corners when `convex` is true (right for dynamic
/// bodies). Each file and options is read once; the same call again, or a
/// scene naming the same file, gives the same mesh. `PONKO_NOT_FOUND` for a
/// missing file, `PONKO_INVALID_ARGUMENT` for one that isn't a glTF file or
/// has no triangles (or a hull that would be flat).
/// ```
/// let mut mesh = Default::default();
/// let one = ponko_ffi::PonkoVec3 { x: 1.0, y: 1.0, z: 1.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_collision_mesh(std::ptr::null_mut(), c"a.glb".as_ptr(), one, false, &mut mesh) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `path` a NUL-terminated string; `out_mesh` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_collision_mesh(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    scale: PonkoVec3,
    convex: bool,
    out_mesh: *mut PonkoCollisionMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { imp::load(ctx, path, scale, convex, out_mesh) })
}

#[cfg(feature = "physics3d")]
mod imp {
    use std::ffi::c_char;

    use ponko::d3::CollisionMeshOptions;

    use crate::ctx::{ctx_access, out, text};
    use crate::physics3d::{PonkoCollisionMesh, mesh_to_c};
    use crate::three::PonkoVec3;
    use crate::types::to_c;
    use crate::{Failure, PonkoCtx, engine_error, invalid};

    /// # Safety
    /// Forwards the caller's contract of `ponko_ctx_load_collision_mesh`.
    pub(super) unsafe fn load(
        ctx: *mut PonkoCtx,
        path: *const c_char,
        scale: PonkoVec3,
        convex: bool,
        out_mesh: *mut PonkoCollisionMesh,
    ) -> Result<(), Failure> {
        if out_mesh.is_null() {
            return Err(invalid("output is null"));
        }
        let scale: ponko::prelude::Vec3 = to_c(scale);
        if !scale.is_finite() {
            return Err(invalid("the scale must be finite"));
        }
        // SAFETY: forwarded caller contract.
        unsafe {
            ctx_access(ctx, |c| {
                let path = text(path)?;
                let mesh = c
                    .load_collision_mesh_with(path, CollisionMeshOptions { scale, convex })
                    .map_err(engine_error)?;
                out(out_mesh, mesh_to_c(mesh))
            })
        }
    }
}

#[cfg(not(feature = "physics3d"))]
mod imp {
    use std::ffi::c_char;

    use crate::physics3d::PonkoCollisionMesh;
    use crate::three::PonkoVec3;
    use crate::{Failure, PonkoCtx, invalid};

    /// # Safety
    /// Nothing is read: every call fails, saying why.
    pub(super) unsafe fn load(
        _: *mut PonkoCtx,
        _: *const c_char,
        _: PonkoVec3,
        _: bool,
        _: *mut PonkoCollisionMesh,
    ) -> Result<(), Failure> {
        Err(invalid("ponko_ffi was built without the `physics3d` feature"))
    }
}
