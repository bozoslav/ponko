//! Levels from Tiled and LDtk, parallax, animated and flipped tiles,
//! autotiling and tile colliders through the C API. The levels are the
//! engine's test fixtures (`tests/fixtures/tilemaps`).
mod common;
use common::{last_error, run_game};
use ponko_ffi::*;
use std::ffi::{CStr, CString, c_char};
use std::ptr::{null, null_mut};

use PonkoStatus::*;

fn fixture(name: &str) -> CString {
    CString::new(format!(
        "{}/../../tests/fixtures/tilemaps/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[derive(Default)]
struct Run {
    path: Option<CString>,
    options: Option<PonkoLevelOptions>,
    root: u64,
    entities: Vec<u64>,
    status: Option<PonkoStatus>,
}

/// Spawns `run`'s level in `ctx`, keeping its status, root and entities.
/// Called through [`run_game`], so a failed assertion here is an ordinary
/// test failure, not a panic across the C boundary.
unsafe fn spawn(ctx: *mut PonkoCtx, run: &mut Run) {
    unsafe {
        let options = run.options.as_ref().map_or(null(), |o| o as *const _);
        let at = PonkoVec2 { x: 0.0, y: 0.0 };
        let status = ponko_ctx_spawn_level(ctx, run.path.as_ref().unwrap().as_ptr(), at, options, &mut run.root);
        run.status = Some(status);
        if status != PONKO_OK {
            return;
        }
        let w = ponko_ctx_world(ctx);
        let mut n = 0;
        assert_eq!(ponko_level_entities(w, run.root, null_mut(), 0, &mut n), PONKO_OK);
        run.entities = vec![0; n];
        assert_eq!(
            ponko_level_entities(w, run.root, run.entities.as_mut_ptr(), n, &mut n),
            PONKO_OK
        );
    }
}

/// Spawns `run`'s level headless with 2D physics; returns the world.
fn spawn_level(run: &mut Run) -> *mut PonkoWorld {
    unsafe {
        let game = ponko_game_new(c"levels".as_ptr(), 64, 64);
        assert_eq!(ponko_game_physics(game, PonkoVec2 { x: 0.0, y: 0.0 }), PONKO_OK);
        run_game(game, 0, |ctx| spawn(ctx, run))
    }
}

type TextFn = unsafe extern "C" fn(*mut PonkoWorld, u64, *mut c_char, usize, *mut usize) -> PonkoStatus;

fn read(w: *mut PonkoWorld, e: u64, f: TextFn) -> String {
    let mut buf = [0 as c_char; 64];
    let mut len = 0;
    assert_eq!(unsafe { f(w, e, buf.as_mut_ptr(), buf.len(), &mut len) }, PONKO_OK);
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

fn property(w: *mut PonkoWorld, e: u64, name: &CStr) -> Result<String, PonkoStatus> {
    let mut buf = [0 as c_char; 64];
    let mut len = 0;
    match unsafe { ponko_level_property_json(w, e, name.as_ptr(), buf.as_mut_ptr(), buf.len(), &mut len) } {
        PONKO_OK => Ok(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()),
        status => Err(status),
    }
}

/// The level objects among `entities`, with their class.
/// Whether `e` has a rigid body (component id `body`).
fn has_body(w: *mut PonkoWorld, e: u64, body: u32) -> bool {
    let mut b = std::mem::MaybeUninit::<PonkoRigidBody>::uninit();
    unsafe { ponko_get(w, e, body, b.as_mut_ptr().cast(), size_of::<PonkoRigidBody>()) == PONKO_OK }
}

fn objects(w: *mut PonkoWorld, entities: &[u64]) -> Vec<(u64, PonkoLevelObject, String)> {
    let mut out = Vec::new();
    for &e in entities {
        let mut o = PonkoLevelObject::default();
        if unsafe { ponko_level_object_get(w, e, &mut o) } == PONKO_OK {
            out.push((e, o, read(w, e, ponko_level_object_class)));
        }
    }
    out
}

#[test]
fn a_tiled_level_spawns_with_its_objects_properties_and_colliders() {
    let mut run = Run {
        path: Some(fixture("level.tmx")),
        ..Run::default()
    };
    let w = spawn_level(&mut run);
    assert_eq!(run.status, Some(PONKO_OK));
    let objs = objects(w, &run.entities);
    assert_eq!(objs.len(), 5);
    let (door, o, class) = objs
        .iter()
        .find(|(e, ..)| read(w, *e, ponko_level_object_name) == "Door")
        .unwrap();
    assert_eq!(class, "Door");
    assert_eq!(
        (o.position.x, o.position.y, o.shape),
        (104.0, 64.0, PONKO_LEVEL_SHAPE_RECT)
    );
    assert_eq!(read(w, *door, ponko_level_object_layer), "Objects");
    assert_eq!(property(w, *door, c"locked"), Ok("true".into()));
    assert_eq!(property(w, *door, c"target"), Ok("\"level2.tmj\"".into()));
    assert_eq!(property(w, *door, c"nope"), Err(PONKO_NOT_FOUND));
    assert_eq!(
        property(w, run.root, c"music"),
        Ok("\"theme.ogg\"".into()),
        "the root has the map's"
    );
    let coin = objs.iter().find(|(_, o, _)| o.id == 5).unwrap();
    assert!(coin.1.has_tile && coin.1.tile == 1);

    // The colliders: fixed bodies among the level's entities.
    let mut body = 0;
    let rigid: Vec<u64> = unsafe {
        assert_eq!(ponko_rigid_body_component(w, &mut body), PONKO_OK);
        run.entities.iter().copied().filter(|&e| has_body(w, e, body)).collect()
    };
    assert!(!rigid.is_empty());
    unsafe { ponko_world_free(w) };
}

#[test]
fn ldtk_options_pick_the_level_and_name_solid_values() {
    let solid = [c"wall".as_ptr()];
    let mut run = Run {
        path: Some(fixture("world.ldtk")),
        options: Some(PonkoLevelOptions {
            prefab_classes: null(),
            prefab_paths: null(),
            prefab_count: 0,
            solid: solid.as_ptr(),
            solid_count: 1,
            level: c"Level_1".as_ptr(),
            no_colliders: false,
            z: 0.0,
        }),
        ..Run::default()
    };
    let w = spawn_level(&mut run);
    assert_eq!(run.status, Some(PONKO_OK));
    let mut body = 0;
    let bodies = unsafe {
        assert_eq!(ponko_rigid_body_component(w, &mut body), PONKO_OK);
        run.entities.iter().filter(|&&e| has_body(w, e, body)).count()
    };
    assert_eq!(bodies, 1, "Level_1's wall row, one rectangle");
    unsafe { ponko_world_free(w) };

    let mut missing = Run {
        path: Some(fixture("nope.tmx")),
        ..Run::default()
    };
    let w = spawn_level(&mut missing);
    assert_eq!(missing.status, Some(PONKO_NOT_FOUND));
    unsafe { ponko_world_free(w) };
}

#[test]
fn despawning_a_level_removes_everything_it_spawned() {
    let mut run = Run {
        path: Some(fixture("level.tmx")),
        ..Run::default()
    };
    unsafe {
        let game = ponko_game_new(c"levels".as_ptr(), 64, 64);
        let w = run_game(game, 0, |ctx| {
            spawn(ctx, &mut run);
            assert_eq!(run.status, Some(PONKO_OK), "{}", last_error());
            assert_eq!(ponko_ctx_despawn_level(ctx, run.root), PONKO_OK);
            assert_eq!(ponko_ctx_despawn_level(ctx, run.root), PONKO_NOT_FOUND);
        });
        for &e in &run.entities {
            let mut alive = true;
            assert_eq!(ponko_is_alive(w, e, &mut alive), PONKO_OK);
            assert!(!alive);
        }
        let mut n = 0;
        assert_eq!(
            ponko_level_entities(w, run.root, null_mut(), 0, &mut n),
            PONKO_NOT_FOUND
        );
        ponko_world_free(w);
    }
}

fn map_world() -> (*mut PonkoWorld, u64) {
    let w = ponko_world_new();
    let sheet = PonkoSpriteSheet {
        texture: PonkoTexture {
            id: 0,
            width: 256,
            height: 256,
        },
        frame_size: PonkoVec2 { x: 16.0, y: 16.0 },
        columns: 16,
        len: 256,
    };
    let mut e = 0;
    unsafe {
        assert_eq!(ponko_spawn(w, null(), 0, &mut e), PONKO_OK);
        assert_eq!(ponko_tilemap_add(w, e, sheet, 4, 3), PONKO_OK);
    }
    (w, e)
}

fn tiles(w: *mut PonkoWorld, e: u64) -> Vec<u16> {
    let mut m = std::mem::MaybeUninit::uninit();
    unsafe {
        assert_eq!(ponko_tilemap_get(w, e, m.as_mut_ptr()), PONKO_OK);
        let m = m.assume_init();
        std::slice::from_raw_parts(m.tiles, (m.width * m.height) as usize).to_vec()
    }
}

#[test]
fn tilemaps_take_parallax_animations_and_flips() {
    let (w, e) = map_world();
    unsafe {
        let half = PonkoVec2 { x: 0.5, y: 0.5 };
        assert_eq!(ponko_tilemap_set_parallax(w, e, half, PonkoVec2::default()), PONKO_OK);
        let (frames, secs) = ([3u16, 4], [0.1f32, 0.2]);
        assert_eq!(
            ponko_tilemap_animate(w, e, 3, frames.as_ptr(), secs.as_ptr(), 2),
            PONKO_OK
        );
        assert_eq!(
            ponko_tilemap_animate(w, e, 3, null(), null(), 1),
            PONKO_INVALID_ARGUMENT
        );
        assert_eq!(ponko_tilemap_set_flipped(w, e, 1, 1, 7, PONKO_TILE_FLIP_Y), PONKO_OK);
        let mut flip = 0;
        assert_eq!(ponko_tilemap_flip(w, e, 1, 1, &mut flip), PONKO_OK);
        assert_eq!(flip, PONKO_TILE_FLIP_Y);
        assert_eq!(tiles(w, e)[4 + 1], 7);
        assert_eq!(ponko_tilemap_set_flipped(w, e, 1, 1, PONKO_TILE_EMPTY, 0), PONKO_OK);
        assert_eq!(tiles(w, e)[4 + 1], PONKO_TILE_EMPTY);
        assert_eq!(ponko_tilemap_set_parallax(w, 12345, half, half), PONKO_NOT_FOUND);
        ponko_world_free(w);
    }
}

#[test]
fn tilemaps_autotile_and_paint_by_neighbours() {
    let (w, e) = map_world();
    let frames: [u16; 16] = std::array::from_fn(|i| 100 + i as u16);
    // A 4 x 3 map, the middle row filled.
    let filled = [
        false, false, false, false, true, true, true, true, false, false, false, false,
    ];
    unsafe {
        assert_eq!(
            ponko_tilemap_autotile(w, e, 4, frames.as_ptr(), 16, false, filled.as_ptr()),
            PONKO_OK
        );
        let t = tiles(w, e);
        assert_eq!(t[4], 100 + 2, "east neighbour only");
        assert_eq!(t[5], 100 + 2 + 8, "east and west");
        assert_eq!(t[0], PONKO_TILE_EMPTY);
        assert_eq!(
            ponko_tilemap_paint_autotile(w, e, 4, frames.as_ptr(), 16, false, 1, 0, true),
            PONKO_OK
        );
        assert_eq!(tiles(w, e)[5], 100 + 1 + 2 + 8, "the cell above is filled now");
        assert_eq!(
            ponko_tilemap_autotile(w, e, 5, frames.as_ptr(), 16, false, filled.as_ptr()),
            PONKO_INVALID_ARGUMENT
        );
        let blob = [0u16; 47];
        assert_eq!(
            ponko_tilemap_autotile(w, e, 8, blob.as_ptr(), 47, true, filled.as_ptr()),
            PONKO_OK
        );
        assert_eq!(
            ponko_tilemap_autotile(w, e, 8, blob.as_ptr(), 16, true, filled.as_ptr()),
            PONKO_INVALID_ARGUMENT
        );
        ponko_world_free(w);
    }
}

#[test]
fn tilemap_solid_tiles_merge_into_rects_and_colliders() {
    let (w, e) = map_world();
    let filled = [true; 12];
    let frames = [9u16; 16];
    unsafe {
        assert_eq!(
            ponko_tilemap_autotile(w, e, 4, frames.as_ptr(), 16, true, filled.as_ptr()),
            PONKO_OK
        );
        let solid = [9u16];
        let mut rects = [PonkoRect::default(); 4];
        let mut n = 0;
        assert_eq!(
            ponko_tilemap_solid_rects(w, e, solid.as_ptr(), 1, rects.as_mut_ptr(), 4, &mut n),
            PONKO_OK
        );
        assert_eq!(n, 1);
        assert_eq!((rects[0].max.x, rects[0].max.y), (64.0, 48.0));
        let mut spawned = 0;
        assert_eq!(
            ponko_tilemap_spawn_colliders(w, e, solid.as_ptr(), 1, &mut spawned),
            PONKO_OK
        );
        assert_eq!(spawned, 1);
        assert_eq!(
            ponko_tilemap_solid_rects(w, e, null(), 1, rects.as_mut_ptr(), 4, &mut n),
            PONKO_INVALID_ARGUMENT
        );
        ponko_world_free(w);
    }
}
