//! 2D lights from C: the light components keep the Rust layout and
//! defaults, occluders of every shape (and from a tilemap's walls) go on
//! and come off entities, bad input is refused, and the ambient light and
//! normal maps round-trip through the context.
use ponko_ffi::*;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

use PonkoStatus::*;

const OK: PonkoStatus = PONKO_OK;
const BAD: PonkoStatus = PONKO_INVALID_ARGUMENT;
const NO_TEXTURE: PonkoTexture = PonkoTexture {
    id: 0,
    width: 0,
    height: 0,
};

struct Run {
    done: bool,
}

fn v(x: f32, y: f32) -> PonkoVec2 {
    PonkoVec2 { x, y }
}

unsafe fn value<T>(component: u32, data: &T) -> PonkoValue {
    PonkoValue {
        component,
        data: (data as *const T).cast(),
        size: size_of::<T>(),
    }
}

unsafe fn edges(ctx: *mut PonkoCtx, e: u64) -> usize {
    let mut n = 99;
    assert_eq!(unsafe { ponko_ctx_occluder2d_edge_count(ctx, e, &mut n) }, OK);
    n
}

#[test]
fn the_light_defaults_match_rust() {
    let p = ponko_point_light2d_default();
    assert_eq!(
        (p.intensity, p.radius, p.falloff, p.height, p.shadows),
        (1.0, 200.0, 2.0, 64.0, false)
    );
    assert_eq!((p.color.r, p.color.g, p.color.b), (1.0, 1.0, 1.0));
    let s = ponko_spot_light2d_default();
    assert_eq!((s.radius, s.softness, s.shadows), (300.0, 0.2, false));
    let d = ponko_directional_light2d_default();
    assert_eq!((d.direction.x, d.direction.y, d.intensity), (0.0, 1.0, 1.0));
    assert!((d.elevation - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
}

unsafe extern "C" fn setup(ctx: *mut PonkoCtx, user: *mut c_void) {
    unsafe {
        let run = &mut *user.cast::<Run>();
        let w = ponko_ctx_world(ctx);
        let (mut transform, mut point, mut spot, mut sun) = (0, 0, 0, 0);
        assert_eq!(ponko_transform_component(w, &mut transform), OK);
        assert_eq!(ponko_point_light2d_component(w, &mut point), OK);
        assert_eq!(ponko_spot_light2d_component(w, &mut spot), OK);
        assert_eq!(ponko_directional_light2d_component(w, &mut sun), OK);
        assert_eq!(ponko_point_light2d_component(null_mut(), &mut point), BAD);
        assert_eq!(ponko_spot_light2d_component(w, null_mut()), BAD);

        // Lights spawn as plain values and read back unchanged.
        let at = PonkoTransform2D {
            position: v(10.0, 20.0),
            ..Default::default()
        };
        let mut torch = ponko_point_light2d_default();
        torch.radius = 90.0;
        torch.shadows = true;
        let mut lamp = ponko_spot_light2d_default();
        lamp.angle = 0.4;
        let mut moon = ponko_directional_light2d_default();
        moon.direction = v(1.0, 1.0);
        let parts = [
            value(transform, &at),
            value(point, &torch),
            value(spot, &lamp),
            value(sun, &moon),
        ];
        let mut e = 0u64;
        assert_eq!(ponko_spawn(w, parts.as_ptr(), parts.len(), &mut e), OK);
        let mut back = ponko_point_light2d_default();
        assert_eq!(
            ponko_get(w, e, point, (&raw mut back).cast(), size_of::<PonkoPointLight2D>()),
            OK
        );
        assert_eq!(back, torch);
        let mut back = ponko_spot_light2d_default();
        assert_eq!(
            ponko_get(w, e, spot, (&raw mut back).cast(), size_of::<PonkoSpotLight2D>()),
            OK
        );
        assert_eq!(back.angle, 0.4);
        let mut back = ponko_directional_light2d_default();
        assert_eq!(
            ponko_get(w, e, sun, (&raw mut back).cast(), size_of::<PonkoDirectionalLight2D>()),
            OK
        );
        assert_eq!((back.direction.x, back.direction.y), (1.0, 1.0));

        // Occluders of each shape, replacing one another.
        assert_eq!(edges(ctx, e), 0);
        assert_eq!(ponko_ctx_set_occluder2d_rect(ctx, e, v(8.0, 4.0)), OK);
        assert_eq!(edges(ctx, e), 4);
        assert_eq!(ponko_ctx_set_occluder2d_circle(ctx, e, 5.0), OK);
        assert_eq!(edges(ctx, e), 16);
        let triangle = [v(0.0, 0.0), v(10.0, 0.0), v(0.0, 10.0)];
        assert_eq!(ponko_ctx_set_occluder2d_polygon(ctx, e, triangle.as_ptr(), 3), OK);
        assert_eq!(edges(ctx, e), 3);
        assert_eq!(ponko_ctx_set_occluder2d_line(ctx, e, v(0.0, 0.0), v(5.0, 0.0)), OK);
        assert_eq!(edges(ctx, e), 2, "one edge per side");
        // Bad input changes nothing.
        assert_eq!(ponko_ctx_set_occluder2d_polygon(ctx, e, null(), 3), BAD);
        assert_eq!(ponko_ctx_set_occluder2d_rect(ctx, e, v(f32::NAN, 1.0)), BAD);
        assert_eq!(ponko_ctx_set_occluder2d_circle(ctx, e, f32::INFINITY), BAD);
        assert_eq!(ponko_ctx_set_occluder2d_line(ctx, 12345, v(0.0, 0.0), v(1.0, 0.0)), BAD);
        assert_eq!(edges(ctx, e), 2);
        assert_eq!(ponko_ctx_occluder2d_edge_count(ctx, e, null_mut()), BAD);
        // Taking it off, twice.
        assert_eq!(ponko_ctx_remove_occluder2d(ctx, e), OK);
        assert_eq!(edges(ctx, e), 0);
        assert_eq!(ponko_ctx_remove_occluder2d(ctx, e), OK);
        assert_eq!(ponko_ctx_remove_occluder2d(null_mut(), e), BAD);

        // A tilemap's walls: a 2 x 1 block in a 4 x 3 map is one outline.
        let mut pixels = vec![255u8; 32 * 16 * 4];
        pixels[3] = 255;
        let mut tex = NO_TEXTURE;
        assert_eq!(
            ponko_ctx_create_texture(ctx, 32, 16, pixels.as_ptr(), pixels.len(), &mut tex),
            OK
        );
        let mut sheet = std::mem::MaybeUninit::uninit();
        assert_eq!(ponko_sprite_sheet(tex, 16, 16, sheet.as_mut_ptr()), OK);
        let mut room = 0u64;
        assert_eq!(ponko_spawn(w, [value(transform, &at)].as_ptr(), 1, &mut room), OK);
        assert_eq!(ponko_tilemap_add(w, room, sheet.assume_init(), 4, 3), OK);
        let mut map = std::mem::MaybeUninit::uninit();
        assert_eq!(ponko_tilemap_get(w, room, map.as_mut_ptr()), OK);
        let map = map.assume_init();
        *map.tiles.add(5) = 1; // (1, 1)
        *map.tiles.add(6) = 1; // (2, 1)
        let wall = [1u16];
        assert_eq!(ponko_ctx_set_tilemap_occluder(ctx, room, wall.as_ptr(), 1), OK);
        assert_eq!(edges(ctx, room), 4);
        let floor = [0u16];
        assert_eq!(ponko_ctx_set_tilemap_occluder(ctx, room, floor.as_ptr(), 1), OK);
        assert_eq!(edges(ctx, room), 0, "no tile is 0: nothing to outline");
        assert_eq!(ponko_ctx_set_tilemap_occluder(ctx, room, null(), 1), BAD);
        assert_eq!(
            ponko_ctx_set_tilemap_occluder(ctx, e, wall.as_ptr(), 1),
            BAD,
            "no tilemap"
        );

        // The ambient light.
        let (mut on, mut color) = (
            true,
            PonkoColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            },
        );
        assert_eq!(ponko_ctx_ambient_light2d(ctx, &mut on, &mut color), OK);
        assert!(!on);
        let night = PonkoColor {
            r: 0.1,
            g: 0.1,
            b: 0.2,
            a: 1.0,
        };
        assert_eq!(ponko_ctx_set_ambient_light2d(ctx, true, night), OK);
        assert_eq!(ponko_ctx_ambient_light2d(ctx, &mut on, &mut color), OK);
        assert!(on && color == night);
        assert_eq!(ponko_ctx_set_ambient_light2d(ctx, false, night), OK);
        assert_eq!(ponko_ctx_ambient_light2d(ctx, &mut on, &mut color), OK);
        assert!(!on);
        assert_eq!(ponko_ctx_ambient_light2d(ctx, &mut on, null_mut()), BAD);

        // Normal maps.
        let mut bumps = NO_TEXTURE;
        assert_eq!(
            ponko_ctx_create_texture(ctx, 32, 16, pixels.as_ptr(), pixels.len(), &mut bumps),
            OK
        );
        let (mut has, mut got) = (true, NO_TEXTURE);
        assert_eq!(ponko_ctx_normal_map(ctx, tex, &mut has, &mut got), OK);
        assert!(!has);
        assert_eq!(ponko_ctx_set_normal_map(ctx, tex, true, bumps), OK);
        assert_eq!(ponko_ctx_normal_map(ctx, tex, &mut has, &mut got), OK);
        assert!(has && got == bumps);
        assert_eq!(ponko_ctx_set_normal_map(ctx, tex, false, bumps), OK);
        assert_eq!(ponko_ctx_normal_map(ctx, tex, &mut has, &mut got), OK);
        assert!(!has);
        assert_eq!(ponko_ctx_normal_map(ctx, tex, &mut has, null_mut()), BAD);
        assert_eq!(ponko_ctx_set_normal_map(null_mut(), tex, true, bumps), BAD);
        run.done = true;
    }
}

#[test]
fn lights_occluders_ambient_and_normal_maps_work_from_c() {
    let mut run = Run { done: false };
    unsafe {
        let g = ponko_game_new(c"lights 2d".as_ptr(), 1, 1);
        let mut world = null_mut();
        let user = (&raw mut run).cast();
        assert_eq!(ponko_game_run_headless(g, 1, Some(setup), user, &mut world), OK);
        ponko_world_free(world);
    }
    assert!(run.done);
}
