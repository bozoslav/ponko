//! The scene stage of the custom passes: compute passes that read this
//! frame's lit image (Rust `RenderPasses::scene_compute`) and effects that
//! write render targets instead of the frame (`RenderPasses::target_effects`).
use crate::PonkoMaterial;
use crate::ctx::{ctx_access, out};
use crate::three::{PonkoComputePass, PonkoEffect, compute_pass_to_c, compute_passes_from_c};
use crate::types::{PonkoTexture, texture_from_c, texture_to_c, to_c};
use crate::{PonkoCtx, PonkoStatus, boundary, invalid};
use ponko::__ffi;
use ponko::render::{Effect, TargetEffect};

/// A screen effect that writes a render target instead of the frame (Rust
/// `TargetEffect`): `effect` runs over the lit frame into `target`, reading
/// its values and textures from `material` when `has_material`.
/// ```
/// let e = ponko_ffi::PonkoTargetEffect {
///     effect: ponko_ffi::PonkoEffect { id: 1 },
///     target: ponko_ffi::PonkoTexture { id: 2, width: 64, height: 64 },
///     has_material: false,
///     material: Default::default(),
/// };
/// assert_eq!(e.target.width, 64);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTargetEffect {
    /// The shader, made like a screen effect's.
    pub effect: PonkoEffect,
    /// What it writes: from `ponko_ctx_create_render_target`.
    pub target: PonkoTexture,
    /// Whether `material` is used.
    pub has_material: bool,
    /// Where its values and textures come from.
    pub material: PonkoMaterial,
}

/// Replaces the scene-stage compute passes with `count` of them, in order
/// (Rust `ctx.scene3d.passes.scene_compute`); 0 runs none. They run after
/// the opaque and transparent surfaces and temporal antialiasing, before
/// the target effects, the screen effects, bloom and tone mapping, so their
/// shaders read this frame's lit image and depth (`scene_color`,
/// `scene_position`, `scene_distance`). Same entries as
/// `ponko_ctx_set_compute_passes`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_scene_compute_passes(std::ptr::null_mut(), std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `passes` holds `count` values (may be null when 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_scene_compute_passes(
    ctx: *mut PonkoCtx,
    passes: *const PonkoComputePass,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let list = compute_passes_from_c(c, passes, count)?;
            c.scene3d.passes.scene_compute = list;
            Ok(())
        })
    })
}

/// The scene-stage compute passes: writes up to `capacity` of them to
/// `out_passes` and how many there are to `out_count` (Rust
/// `ctx.scene3d.passes.scene_compute`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_scene_compute_passes(std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_passes` holds `capacity` values (may be null
/// when 0); `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_scene_compute_passes(
    ctx: *mut PonkoCtx,
    out_passes: *mut PonkoComputePass,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_count.is_null() || (out_passes.is_null() && capacity > 0) {
                return Err(invalid("output is null"));
            }
            for (i, p) in c.scene3d.passes.scene_compute.iter().take(capacity).enumerate() {
                out_passes.add(i).write(compute_pass_to_c(p));
            }
            out(out_count, c.scene3d.passes.scene_compute.len())
        })
    })
}

/// Replaces the target effects with `count` of them, in order (Rust
/// `ctx.scene3d.passes.target_effects`); 0 runs none. They run over the lit
/// frame after the transparent surfaces, before the screen effects, each
/// writing its own render target. An effect or material this game didn't
/// make, or a target that isn't one of its render targets, is an error.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_target_effects(std::ptr::null_mut(), std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `effects` holds `count` values (may be null when 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_target_effects(
    ctx: *mut PonkoCtx,
    effects: *const PonkoTargetEffect,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if effects.is_null() && count > 0 {
                return Err(invalid("effects is null"));
            }
            let given = if count == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(effects, count)
            };
            let mut list = Vec::with_capacity(count);
            for e in given {
                let effect = __ffi::from_id::<Effect>(e.effect.id);
                if !c.gfx.is_live(effect) {
                    return Err(invalid("that effect wasn't made by this game"));
                }
                let target = texture_from_c(e.target);
                if !c.gfx.is_render_target(target) {
                    return Err(invalid("that texture isn't a render target of this game"));
                }
                let pass = TargetEffect::new(effect, target);
                list.push(if e.has_material {
                    let material = to_c(e.material);
                    if !c.gfx.is_live(material) {
                        return Err(invalid("that material wasn't made by this game"));
                    }
                    pass.with_material(material)
                } else {
                    pass
                });
            }
            c.scene3d.passes.target_effects = list;
            Ok(())
        })
    })
}

/// The target effects: writes up to `capacity` of them to `out_effects`
/// and how many there are to `out_count` (Rust
/// `ctx.scene3d.passes.target_effects`).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_target_effects(std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_effects` holds `capacity` values (may be null
/// when 0); `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_target_effects(
    ctx: *mut PonkoCtx,
    out_effects: *mut PonkoTargetEffect,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_count.is_null() || (out_effects.is_null() && capacity > 0) {
                return Err(invalid("output is null"));
            }
            for (i, e) in c.scene3d.passes.target_effects.iter().take(capacity).enumerate() {
                out_effects.add(i).write(PonkoTargetEffect {
                    effect: PonkoEffect {
                        id: __ffi::id(e.effect),
                    },
                    target: texture_to_c(e.target),
                    has_material: e.material.is_some(),
                    material: e.material.map(to_c).unwrap_or_default(),
                });
            }
            out(out_count, c.scene3d.passes.target_effects.len())
        })
    })
}
