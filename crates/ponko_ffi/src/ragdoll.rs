//! Ragdolls from C (the `physics3d` feature): a skeleton handed over to
//! physics and back (Rust `RagdollBone`, `Ragdoll3D`, `Ctx::start_ragdoll`,
//! `Ctx::stop_ragdoll`). Without the feature the ctx calls fail with an
//! error naming it; `ponko_ragdoll_bone` still works.
#![cfg_attr(not(feature = "physics3d"), allow(unused_variables))]

use std::ffi::c_char;

use crate::{PonkoCtx, PonkoStatus};

/// One bone of a ragdoll (Rust `RagdollBone`): the joint it starts at, and
/// the capsule that stands in for it, running to the joint's first child.
/// Start from `ponko_ragdoll_bone` and change what you need.
/// ```
/// let b = ponko_ffi::ponko_ragdoll_bone(c"spine".as_ptr(), 0.1);
/// assert_eq!((b.radius, b.swing, b.twist, b.density), (0.1, 0.7, 0.3, 1000.0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoRagdollBone {
    /// The joint's name on the skeleton (a C string, read during the call).
    pub joint: *const c_char,
    /// The capsule's radius in meters.
    pub radius: f32,
    /// How far the bone may swing away from its parent, in radians. Default 0.7.
    pub swing: f32,
    /// How far it may twist around itself, either way, in radians. Default 0.3.
    pub twist: f32,
    /// kg per cubic meter. Default 1000 (water, close to a body).
    pub density: f32,
}

/// A ragdoll bone at `joint` with the given capsule radius and the default
/// limits and density.
/// ```
/// let b = ponko_ffi::ponko_ragdoll_bone(c"hips".as_ptr(), 0.12);
/// assert_eq!(b.radius, 0.12);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_ragdoll_bone(joint: *const c_char, radius: f32) -> PonkoRagdollBone {
    PonkoRagdollBone {
        joint,
        radius,
        swing: 0.7,
        twist: 0.3,
        density: 1000.0,
    }
}

/// Hands `entity`'s skeleton over to physics (Rust `Ctx::start_ragdoll`):
/// one dynamic body per bone, jointed to the nearest chosen ancestor, posed
/// where the skeleton is now. From then on the bodies pose the skeleton and
/// its animator pauses. The bodies are linked at once, so you can push them
/// straight away (`ponko_ctx_ragdoll_body`, then
/// `ponko_ctx_apply_impulse3d`). Starting again replaces the old ragdoll.
/// `PONKO_INVALID_ARGUMENT` if the entity has no skeleton and transform, a
/// joint name is unknown or repeated, or a radius isn't positive.
/// # Safety
/// `ctx` null or live, outside query callbacks; `bones` points to `count`
/// bones whose `joint` are C strings.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_start_ragdoll(std::ptr::null_mut(), 0, std::ptr::null(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_start_ragdoll(
    ctx: *mut PonkoCtx,
    entity: u64,
    bones: *const PonkoRagdollBone,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::start(ctx, entity, bones, count) }
}

/// Ends `entity`'s ragdoll (Rust `Ctx::stop_ragdoll`): removes its bodies
/// and joints, and blends the skeleton back to its animated pose over
/// `seconds` (0 snaps). Call it before despawning the entity: the bodies
/// are entities of their own. Does nothing if `entity` isn't ragdolled.
/// # Safety
/// `ctx` null or live, outside query callbacks.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_stop_ragdoll(std::ptr::null_mut(), 0, 0.0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_stop_ragdoll(ctx: *mut PonkoCtx, entity: u64, seconds: f32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::stop(ctx, entity, seconds) }
}

/// The body entity standing in for skeleton joint `joint` (an index, from
/// `ponko_ctx_find_joint`) of a ragdolled `entity` (Rust
/// `Ragdoll3D::body`). `PONKO_NOT_FOUND` if `entity` isn't ragdolled or
/// that joint has no body.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ragdoll_body(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ragdoll_body(
    ctx: *mut PonkoCtx,
    entity: u64,
    joint: usize,
    out_body: *mut u64,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::body(ctx, entity, joint, out_body) }
}

/// Whether `entity` is ragdolled (`out_ragdolled`) and, if so, whether it is
/// blending back to its animation after `ponko_ctx_stop_ragdoll`
/// (`out_blending`, Rust `Ragdoll3D::is_blending`).
/// # Safety
/// `ctx` null or live; outputs writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ragdoll_state(std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ragdoll_state(
    ctx: *mut PonkoCtx,
    entity: u64,
    out_ragdolled: *mut bool,
    out_blending: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::state(ctx, entity, out_ragdolled, out_blending) }
}

#[cfg(feature = "physics3d")]
mod imp {
    use super::*;
    use crate::ctx::{ctx_access, out, text};
    use crate::{boundary, error, invalid, invalid_owned};
    use ponko::d3::Ragdoll as Ragdoll3D;
    use ponko::d3::RagdollBone;
    use ponko::prelude::Entity;

    pub(super) unsafe fn start(
        ctx: *mut PonkoCtx,
        entity: u64,
        bones: *const PonkoRagdollBone,
        count: usize,
    ) -> PonkoStatus {
        boundary(|| {
            if bones.is_null() && count > 0 {
                return Err(invalid("bones is null"));
            }
            let raw = if count == 0 {
                &[][..]
            } else {
                // SAFETY: caller guarantees `count` readable bones.
                unsafe { std::slice::from_raw_parts(bones, count) }
            };
            let mut list = Vec::with_capacity(count);
            for b in raw {
                // SAFETY: caller guarantees C strings.
                let name = unsafe { text(b.joint) }?;
                list.push(RagdollBone {
                    joint: name.into(),
                    radius: b.radius,
                    swing: b.swing,
                    twist: b.twist,
                    density: b.density,
                });
            }
            // SAFETY: forwarded caller contract.
            unsafe {
                ctx_access(ctx, |c| {
                    c.start_ragdoll(Entity::from_bits(entity), &list).map_err(invalid_owned)
                })
            }
        })
    }

    pub(super) unsafe fn stop(ctx: *mut PonkoCtx, entity: u64, seconds: f32) -> PonkoStatus {
        boundary(|| {
            if seconds.is_nan() || seconds < 0.0 {
                return Err(invalid("seconds must be 0 or more"));
            }
            // SAFETY: forwarded caller contract.
            unsafe {
                ctx_access(ctx, |c| {
                    c.stop_ragdoll(Entity::from_bits(entity), seconds);
                    Ok(())
                })
            }
        })
    }

    pub(super) unsafe fn body(ctx: *mut PonkoCtx, entity: u64, joint: usize, out_body: *mut u64) -> PonkoStatus {
        boundary(|| {
            // SAFETY: forwarded caller contract.
            let body = unsafe {
                ctx_access(ctx, |c| {
                    Ok(c.world
                        .get::<Ragdoll3D>(Entity::from_bits(entity))
                        .and_then(|r| r.body(joint)))
                })?
            }
            .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "no ragdoll body for that joint"))?;
            // SAFETY: forwarded caller contract.
            unsafe { out(out_body, body.to_bits()) }
        })
    }

    pub(super) unsafe fn state(
        ctx: *mut PonkoCtx,
        entity: u64,
        out_ragdolled: *mut bool,
        out_blending: *mut bool,
    ) -> PonkoStatus {
        boundary(|| {
            // SAFETY: forwarded caller contract.
            let state = unsafe {
                ctx_access(ctx, |c| {
                    Ok(c.world
                        .get::<Ragdoll3D>(Entity::from_bits(entity))
                        .map(|r| r.is_blending()))
                })?
            };
            // SAFETY: forwarded caller contract.
            unsafe {
                out(out_ragdolled, state.is_some())?;
                out(out_blending, state == Some(true))
            }
        })
    }
}

#[cfg(not(feature = "physics3d"))]
mod imp {
    //! Without the feature: every call fails, saying why.
    use super::*;
    use crate::{Failure, boundary, invalid};

    fn off() -> Result<(), Failure> {
        Err(invalid("ponko_ffi was built without the `physics3d` feature"))
    }
    pub(super) unsafe fn start(_: *mut PonkoCtx, _: u64, _: *const PonkoRagdollBone, _: usize) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn stop(_: *mut PonkoCtx, _: u64, _: f32) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn body(_: *mut PonkoCtx, _: u64, _: usize, _: *mut u64) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn state(_: *mut PonkoCtx, _: u64, _: *mut bool, _: *mut bool) -> PonkoStatus {
        boundary(off)
    }
}
