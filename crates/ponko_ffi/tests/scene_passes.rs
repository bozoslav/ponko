//! The scene stage of the custom passes from C: compute passes that read
//! this frame's lit image (`RenderPasses::scene_compute`) and effects that
//! write render targets (`RenderPasses::target_effects`).
mod common;

use PonkoStatus::*;
use common::{last_error, run_ctx};
use ponko_ffi::*;
use std::ptr::null_mut;

unsafe fn target(ctx: *mut PonkoCtx, w: u32, h: u32) -> PonkoTexture {
    unsafe {
        let mut t = PonkoTexture {
            id: 0,
            width: 0,
            height: 0,
        };
        assert_eq!(ponko_ctx_create_render_target(ctx, w, h, &mut t), PONKO_OK);
        t
    }
}

#[test]
fn scene_compute_passes_are_set_apart_from_the_early_ones_and_read_back() {
    run_ctx(|ctx| unsafe {
        let light = target(ctx, 16, 9);
        let mut meter = PonkoCompute::default();
        let source = c"fn compute(c: Cell) -> vec4<f32> { let seen = scene_color(c.uv * frame.screen.xy); return mix(c.value, vec4<f32>(seen, 1.0), 0.05); }";
        assert_eq!(
            ponko_ctx_create_compute(ctx, c"meter".as_ptr(), source.as_ptr(), &mut meter),
            PONKO_OK,
            "{}",
            last_error()
        );
        let pass = PonkoComputePass {
            has_buffer: false,
            buffer: Default::default(),
            compute: meter,
            target: light,
            has_material: false,
            material: PonkoMaterial::default(),
        };
        assert_eq!(
            ponko_ctx_set_scene_compute_passes(ctx, &pass, 1),
            PONKO_OK,
            "{}",
            last_error()
        );
        let mut n = 9;
        assert_eq!(ponko_ctx_scene_compute_passes(ctx, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 1);
        let mut read = [PonkoComputePass {
            target: PonkoTexture {
                id: 0,
                width: 0,
                height: 0,
            },
            ..pass
        }];
        assert_eq!(
            ponko_ctx_scene_compute_passes(ctx, read.as_mut_ptr(), 1, &mut n),
            PONKO_OK
        );
        assert_eq!(read[0], pass);
        // The early stage is a separate list.
        assert_eq!(ponko_ctx_compute_passes(ctx, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 0);

        // A plain texture or an unknown shader is refused and changes nothing.
        let mut plain = PonkoTexture {
            id: 0,
            width: 0,
            height: 0,
        };
        let pixel = [255u8; 4];
        assert_eq!(
            ponko_ctx_create_texture(ctx, 1, 1, pixel.as_ptr(), 4, &mut plain),
            PONKO_OK
        );
        let bad = [
            PonkoComputePass { target: plain, ..pass },
            PonkoComputePass {
                compute: PonkoCompute { id: 99 },
                ..pass
            },
        ];
        for b in &bad {
            assert_eq!(ponko_ctx_set_scene_compute_passes(ctx, b, 1), PONKO_INVALID_ARGUMENT);
        }
        assert_eq!(
            ponko_ctx_set_scene_compute_passes(ctx, std::ptr::null(), 1),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(ponko_ctx_scene_compute_passes(ctx, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 1);
        assert_eq!(ponko_ctx_set_scene_compute_passes(ctx, std::ptr::null(), 0), PONKO_OK);
        assert_eq!(ponko_ctx_scene_compute_passes(ctx, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 0);
    });
}

#[test]
fn target_effects_are_set_checked_and_read_back() {
    run_ctx(|ctx| unsafe {
        let mask = target(ctx, 64, 64);
        let mut far = PonkoEffect::default();
        let source = c"fn effect(p: Pixel) -> vec3<f32> { return vec3<f32>(p.distance); }";
        assert_eq!(
            ponko_ctx_create_effect(ctx, c"far".as_ptr(), source.as_ptr(), &mut far),
            PONKO_OK,
            "{}",
            last_error()
        );
        let mut values = PonkoMaterial::default();
        let white = PonkoColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        assert_eq!(
            ponko_ctx_create_material(ctx, &ponko_material_desc(white), &mut values),
            PONKO_OK
        );
        let effects = [
            PonkoTargetEffect {
                effect: far,
                target: mask,
                has_material: false,
                material: PonkoMaterial::default(),
            },
            PonkoTargetEffect {
                effect: far,
                target: target(ctx, 32, 32),
                has_material: true,
                material: values,
            },
        ];
        assert_eq!(
            ponko_ctx_set_target_effects(ctx, effects.as_ptr(), 2),
            PONKO_OK,
            "{}",
            last_error()
        );
        let mut n = 0;
        let mut read = [effects[1], effects[0]];
        assert_eq!(ponko_ctx_target_effects(ctx, read.as_mut_ptr(), 2, &mut n), PONKO_OK);
        assert_eq!((n, read), (2, effects));
        // The screen effects are a separate list.
        assert_eq!(ponko_ctx_effects(ctx, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 0);

        let mut plain = PonkoTexture {
            id: 0,
            width: 0,
            height: 0,
        };
        let pixel = [255u8; 4];
        assert_eq!(
            ponko_ctx_create_texture(ctx, 1, 1, pixel.as_ptr(), 4, &mut plain),
            PONKO_OK
        );
        let bad = [
            PonkoTargetEffect {
                target: plain,
                ..effects[0]
            },
            PonkoTargetEffect {
                effect: PonkoEffect { id: 99 },
                ..effects[0]
            },
            PonkoTargetEffect {
                material: PonkoMaterial { id: 999 },
                ..effects[1]
            },
        ];
        for b in &bad {
            assert_eq!(ponko_ctx_set_target_effects(ctx, b, 1), PONKO_INVALID_ARGUMENT);
        }
        assert_eq!(ponko_ctx_target_effects(ctx, null_mut(), 0, &mut n), PONKO_OK);
        assert_eq!(n, 2, "refused lists change nothing");
        assert_eq!(
            ponko_ctx_target_effects(ctx, null_mut(), 1, &mut n),
            PONKO_INVALID_ARGUMENT
        );
    });
}
