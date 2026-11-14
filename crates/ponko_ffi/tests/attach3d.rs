//! 3D attachments from C: PonkoAttached3D, placed by
//! ponko_game_follow_attachments.
use ponko_ffi::*;
use std::ffi::c_void;
use std::ptr::null_mut;

use PonkoStatus::*;

#[derive(Default)]
struct Ids {
    transform: u32,
    attached: u32,
    base: u64,
    arm: u64,
}

fn value<T>(component: u32, v: &T) -> PonkoValue {
    PonkoValue {
        component,
        data: (v as *const T).cast(),
        size: size_of::<T>(),
    }
}

fn at(x: f32, y: f32, z: f32) -> PonkoTransform3D {
    ponko_transform3d_at(PonkoVec3 { x, y, z })
}

unsafe extern "C" fn setup(ctx: *mut PonkoCtx, user: *mut c_void) {
    unsafe {
        let ids = &mut *user.cast::<Ids>();
        let w = ponko_ctx_world(ctx);
        assert_eq!(ponko_transform3d_component(w, &mut ids.transform), PONKO_OK);
        assert_eq!(ponko_attached3d_component(w, &mut ids.attached), PONKO_OK);
        // The base at x = 10, turned a quarter to the left about +Y.
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let mut base = at(10.0, 0.0, 0.0);
        base.rotation = PonkoQuat {
            x: 0.0,
            y: half,
            z: 0.0,
            w: half,
        };
        assert_eq!(ponko_spawn(w, &value(ids.transform, &base), 1, &mut ids.base), PONKO_OK);
        let arm = PonkoAttached3D {
            to: PonkoEntity {
                index: ids.base as u32,
                generation: (ids.base >> 32) as u32,
            },
            reserved: [0; 2],
            offset: at(0.0, 2.0, -5.0),
        };
        let values = [value(ids.transform, &at(0.0, 0.0, 0.0)), value(ids.attached, &arm)];
        assert_eq!(ponko_spawn(w, values.as_ptr(), 2, &mut ids.arm), PONKO_OK);
    }
}

#[test]
fn c_3d_attachments_follow_their_parent_turned_with_it() {
    let mut ids = Ids::default();
    let user = (&raw mut ids).cast::<c_void>();
    let mut world = null_mut();
    unsafe {
        let g = ponko_game_new(c"attach3d".as_ptr(), 1, 1);
        assert_eq!(ponko_game_follow_attachments(g), PONKO_OK);
        assert_eq!(ponko_game_run_headless(g, 1, Some(setup), user, &mut world), PONKO_OK);
        let mut t = at(0.0, 0.0, 0.0);
        let size = size_of::<PonkoTransform3D>();
        assert_eq!(
            ponko_get(world, ids.arm, ids.transform, (&raw mut t).cast(), size),
            PONKO_OK
        );
        // (0, 2, -5) turned a quarter about +Y is (-5, 2, 0); plus the base.
        let p = t.position;
        assert!(
            (p.x - 5.0).abs() < 1e-4 && (p.y - 2.0).abs() < 1e-4 && p.z.abs() < 1e-4,
            "{p:?}"
        );
        let mut read = PonkoAttached3D {
            to: PonkoEntity::default(),
            reserved: [0; 2],
            offset: t,
        };
        assert_eq!(
            ponko_get(
                world,
                ids.arm,
                ids.attached,
                (&raw mut read).cast(),
                size_of::<PonkoAttached3D>()
            ),
            PONKO_OK
        );
        assert_eq!(read.offset.position.z, -5.0, "the offset reads back as written");
        ponko_world_free(world);
    }
}
