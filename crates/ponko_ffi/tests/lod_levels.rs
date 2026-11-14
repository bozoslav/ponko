//! Levels of detail beyond groups from C: hysteresis (`LodHysteresis`) and
//! a group's levels for an instance batch (`LodGroup::instance_levels`).
mod common;

use PonkoStatus::*;
use common::{last_error, run_ctx};
use ponko_ffi::*;
use std::ptr::{null, null_mut};

fn level(screen_height: f32, renderers: &[PonkoLodRenderer]) -> PonkoLodLevel {
    PonkoLodLevel {
        screen_height,
        fade_width: 0.0,
        renderers: renderers.as_ptr(),
        renderer_count: renderers.len(),
        custom_shadow: false,
        shadow_casters: null(),
        shadow_caster_count: 0,
    }
}

const ORIGIN: PonkoVec3 = PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 };
const SETTINGS: PonkoLodSettings = PonkoLodSettings {
    bias: 1.0,
    crossfade: true,
};

#[test]
fn hysteresis_is_set_read_back_and_checked() {
    run_ctx(|ctx| unsafe {
        let w = ponko_ctx_world(ctx);
        let mut e = 0;
        assert_eq!(ponko_spawn(w, null(), 0, &mut e), PONKO_OK);
        let mut h = 9.0;
        assert_eq!(ponko_ctx_lod_hysteresis(ctx, e, &mut h), PONKO_OK);
        assert_eq!(h, 0.0, "none: switches at the thresholds");
        assert_eq!(ponko_ctx_set_lod_hysteresis(ctx, e, 0.1), PONKO_OK, "{}", last_error());
        assert_eq!(ponko_ctx_lod_hysteresis(ctx, e, &mut h), PONKO_OK);
        assert_eq!(h, 0.1);

        for bad in [-0.1, 1.5, f32::NAN] {
            assert_eq!(
                ponko_ctx_set_lod_hysteresis(ctx, e, bad),
                PONKO_INVALID_ARGUMENT,
                "{bad}"
            );
        }
        assert_eq!(ponko_ctx_lod_hysteresis(ctx, e, &mut h), PONKO_OK);
        assert_eq!(h, 0.1, "a refused value changes nothing");

        assert_eq!(ponko_despawn(w, e), PONKO_OK);
        assert_eq!(ponko_ctx_set_lod_hysteresis(ctx, e, 0.1), PONKO_INVALID_ARGUMENT);
        assert_eq!(ponko_ctx_lod_hysteresis(ctx, e, &mut h), PONKO_INVALID_ARGUMENT);
    });
}

#[test]
fn instance_levels_switch_where_the_group_would() {
    unsafe {
        let near = [ponko_lod_renderer(PonkoMesh { id: 4 })];
        let far = [ponko_lod_renderer(PonkoMesh { id: 5 })];
        let levels = [level(0.2, &near), level(0.05, &far)];
        let fov = 60f32.to_radians();
        let mut n = 0;
        assert_eq!(
            ponko_lod_instance_levels(
                2.0,
                ORIGIN,
                levels.as_ptr(),
                2,
                1.0,
                fov,
                SETTINGS,
                null_mut(),
                0,
                &mut n
            ),
            PONKO_OK,
            "{}",
            last_error()
        );
        assert_eq!(n, 2);
        let mut out = [PonkoInstanceLevel {
            mesh: PonkoMesh::default(),
            min_distance: -1.0,
            max_distance: -1.0,
            camera: false,
            shadows: false,
        }; 2];
        assert_eq!(
            ponko_lod_instance_levels(
                2.0,
                ORIGIN,
                levels.as_ptr(),
                2,
                1.0,
                fov,
                SETTINGS,
                out.as_mut_ptr(),
                2,
                &mut n
            ),
            PONKO_OK
        );
        let switch = ponko_lod_distance_at(2.0, 0.2, fov);
        let gone = ponko_lod_distance_at(2.0, 0.05, fov);
        assert_eq!(
            (out[0].mesh.id, out[0].min_distance, out[0].max_distance),
            (4, 0.0, switch)
        );
        assert_eq!(
            (out[1].mesh.id, out[1].min_distance, out[1].max_distance),
            (5, switch, gone)
        );
        assert!(out[0].camera && out[0].shadows);

        // Twice as large copies switch twice as far away.
        assert_eq!(
            ponko_lod_instance_levels(
                2.0,
                ORIGIN,
                levels.as_ptr(),
                2,
                2.0,
                fov,
                SETTINGS,
                out.as_mut_ptr(),
                2,
                &mut n
            ),
            PONKO_OK
        );
        assert!((out[0].max_distance - 2.0 * switch).abs() < 1e-3 * switch);
    }
}

#[test]
fn groups_that_cant_be_batches_are_not_found() {
    unsafe {
        let mut tinted = ponko_lod_renderer(PonkoMesh { id: 4 });
        tinted.color.g = 0.5;
        let plain = [ponko_lod_renderer(PonkoMesh { id: 4 })];
        let fov = 1.0;
        let mut n = 7;
        let tint = [tinted];
        assert_eq!(
            ponko_lod_instance_levels(
                1.0,
                ORIGIN,
                &level(0.1, &tint),
                1,
                1.0,
                fov,
                SETTINGS,
                null_mut(),
                0,
                &mut n
            ),
            PONKO_NOT_FOUND
        );
        let off_center = PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 };
        assert_eq!(
            ponko_lod_instance_levels(
                1.0,
                off_center,
                &level(0.1, &plain),
                1,
                1.0,
                fov,
                SETTINGS,
                null_mut(),
                0,
                &mut n
            ),
            PONKO_NOT_FOUND
        );
        assert!(last_error().contains("instance batch"), "{}", last_error());
        // The control: the same level, plain and centered, is a batch.
        assert_eq!(
            ponko_lod_instance_levels(
                1.0,
                ORIGIN,
                &level(0.1, &plain),
                1,
                1.0,
                fov,
                SETTINGS,
                null_mut(),
                0,
                &mut n
            ),
            PONKO_OK
        );
        assert_eq!(n, 1);
        assert_eq!(
            ponko_lod_instance_levels(1.0, ORIGIN, null(), 1, 1.0, fov, SETTINGS, null_mut(), 0, &mut n),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(
            ponko_lod_instance_levels(
                1.0,
                ORIGIN,
                &level(0.1, &plain),
                1,
                1.0,
                fov,
                SETTINGS,
                null_mut(),
                0,
                null_mut()
            ),
            PONKO_INVALID_ARGUMENT
        );
    }
}
