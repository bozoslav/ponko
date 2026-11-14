//! Levels from level editors (Tiled, LDtk) and the tilemap extras: parallax,
//! animated and flipped tiles, autotiling and colliders from tiles. See
//! `Ctx::spawn_level` and `ponko::tilemap` in Rust.
use std::ffi::c_char;

use ponko::d2::{AutoTile, LevelLayer, LevelObject, LevelOptions, LevelRoot, ObjectShape, TileMap};
use ponko::prelude::Vec2;
use ponko_ecs::Entity;

use crate::ctx::{PonkoCtx, ctx_access, out, text};
use crate::types::{PonkoRect, PonkoVec2};
use crate::{Failure, PonkoStatus, PonkoWorld, access, boundary, engine_error, error, invalid, write_text};

/// How `ponko_ctx_spawn_level` spawns a level (all zero: the defaults).
/// Arrays may be null when their count is 0.
/// ```
/// assert_eq!(std::mem::size_of::<ponko_ffi::PonkoLevelOptions>() > 0, true);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoLevelOptions {
    /// Object classes spawned as prefabs, `prefab_count` of them.
    pub prefab_classes: *const *const c_char,
    /// The prefab file for each class.
    pub prefab_paths: *const *const c_char,
    /// How many classes and prefab files.
    pub prefab_count: usize,
    /// More solid things by name: layers, LDtk IntGrid values, tile classes or tags.
    pub solid: *const *const c_char,
    /// How many solid names.
    pub solid_count: usize,
    /// For an LDtk project: the level (identifier or iid); null for the first.
    pub level: *const c_char,
    /// Spawn no colliders.
    pub no_colliders: bool,
    /// The `z` of the objects' layer; layers below one less each, above one more.
    pub z: f32,
}

/// What `ponko_level_object_get` reads from a spawned level object
/// (`LevelObject` in Rust). Its name, class and layer come from
/// `ponko_level_object_name` / `_class` / `_layer`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoLevelObject {
    /// Tiled's object id, or the entity's index in its LDtk layer.
    pub id: u64,
    /// Where it is in level pixels (the center of rectangles and tiles).
    pub position: PonkoVec2,
    /// Width and height in pixels.
    pub size: PonkoVec2,
    /// Radians, clockwise.
    pub rotation: f32,
    /// One of `PONKO_LEVEL_SHAPE_*`.
    pub shape: u32,
    /// It shows a tile: `tileset`, `tile` and `flip` say which.
    pub has_tile: bool,
    /// Index of the tileset in the level.
    pub tileset: u32,
    /// The tile in its tileset.
    pub tile: u32,
    /// `PONKO_TILE_FLIP_*` bits.
    pub flip: u32,
    /// Shown in the editor.
    pub visible: bool,
}

/// A rectangle object.
pub const PONKO_LEVEL_SHAPE_RECT: u32 = 0;
/// An ellipse object.
pub const PONKO_LEVEL_SHAPE_ELLIPSE: u32 = 1;
/// A point object.
pub const PONKO_LEVEL_SHAPE_POINT: u32 = 2;
/// A polygon object.
pub const PONKO_LEVEL_SHAPE_POLYGON: u32 = 3;
/// A polyline object.
pub const PONKO_LEVEL_SHAPE_POLYLINE: u32 = 4;
/// Mirror the tile left to right.
pub const PONKO_TILE_FLIP_X: u8 = 1;
/// Mirror the tile top to bottom.
pub const PONKO_TILE_FLIP_Y: u8 = 2;
/// Swap the tile's axes (Tiled's diagonal flip), before the other two.
pub const PONKO_TILE_FLIP_DIAGONAL: u8 = 4;
const _: () = assert!(
    PONKO_TILE_FLIP_X == TileMap::FLIP_X
        && PONKO_TILE_FLIP_Y == TileMap::FLIP_Y
        && PONKO_TILE_FLIP_DIAGONAL == TileMap::FLIP_DIAGONAL
);

pub(crate) fn vec(v: PonkoVec2) -> Vec2 {
    Vec2::new(v.x, v.y)
}
fn c_vec(v: Vec2) -> PonkoVec2 {
    PonkoVec2 { x: v.x, y: v.y }
}

/// `count` strings from `list`.
unsafe fn strings(list: *const *const c_char, count: usize) -> Result<Vec<String>, Failure> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if list.is_null() {
        return Err(invalid("a string array is null"));
    }
    // SAFETY: the caller supplies `count` readable string pointers.
    let items = unsafe { std::slice::from_raw_parts(list, count) };
    // SAFETY: each is NUL-terminated (caller contract).
    items.iter().map(|&s| unsafe { text(s) }.map(str::to_string)).collect()
}

pub(crate) unsafe fn options(o: *const PonkoLevelOptions) -> Result<LevelOptions, Failure> {
    let mut options = LevelOptions::default();
    if o.is_null() {
        return Ok(options);
    }
    // SAFETY: non-null; the caller supplies a readable struct.
    let o = unsafe { *o };
    // SAFETY: forwarded caller contract for the arrays and strings.
    unsafe {
        let classes = strings(o.prefab_classes, o.prefab_count)?;
        let paths = strings(o.prefab_paths, o.prefab_count)?;
        for (class, path) in classes.into_iter().zip(paths) {
            options = options.prefab(class, path);
        }
        for name in strings(o.solid, o.solid_count)? {
            options = options.solid(name);
        }
        if !o.level.is_null() {
            options = options.level(text(o.level)?);
        }
    }
    if o.no_colliders {
        options = options.without_colliders();
    }
    Ok(options.at_z(o.z))
}

/// Spawns the level at `path` (a Tiled `.tmx` / `.tmj` map or an LDtk
/// `.ldtk` project) with its top-left at `at`: its layers as tilemaps, its
/// objects as entities (or prefabs), and with 2D physics its colliders.
/// `options` may be null for the defaults. Editing the files with hot
/// reload on spawns the level again in place.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_spawn_level(std::ptr::null_mut(), c"a.tmx".as_ptr(), Default::default(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// Live ctx; `path` NUL-terminated; `options` null or readable (with its
/// arrays and strings); `out_root` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_spawn_level(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    at: PonkoVec2,
    options: *const PonkoLevelOptions,
    out_root: *mut u64,
) -> PonkoStatus {
    boundary(|| {
        if out_root.is_null() {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract.
        unsafe {
            let options = self::options(options)?;
            let path = text(path)?;
            ctx_access(ctx, |c| {
                let root = c.spawn_level_with(path, vec(at), options).map_err(engine_error)?;
                out(out_root, root.to_bits())
            })
        }
    })
}

/// Despawns a level spawned with `ponko_ctx_spawn_level`: its root and
/// everything spawned for it. `PONKO_NOT_FOUND` if `root` isn't a level's root.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_despawn_level(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
/// # Safety
/// Live ctx.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_despawn_level(ctx: *mut PonkoCtx, root: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if c.despawn_level(Entity::from_bits(root)) {
                Ok(())
            } else {
                Err(error(PonkoStatus::PONKO_NOT_FOUND, "not the root of a spawned level"))
            }
        })
    })
}

/// The entities spawned for the level whose root is `root` (layers,
/// objects and colliders): writes up to `capacity` of them and the total to
/// `out_len`. `PONKO_NOT_FOUND` if `root` isn't a level's root.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_level_entities(std::ptr::null_mut(), 0, std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `out_entities` holds `capacity` (or is null with
/// 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_level_entities(
    world: *mut PonkoWorld,
    root: u64,
    out_entities: *mut u64,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        if out_len.is_null() || (out_entities.is_null() && capacity != 0) {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract.
        unsafe {
            access(world, |w, _| {
                let level = w
                    .get::<LevelRoot>(Entity::from_bits(root))
                    .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "not the root of a spawned level"))?;
                for (i, e) in level.entities.iter().take(capacity).enumerate() {
                    // SAFETY: i < capacity (caller-sized buffer).
                    out_entities.add(i).write(e.to_bits());
                }
                out(out_len, level.entities.len())
            })
        }
    })
}

unsafe fn with_object<R>(
    world: *mut PonkoWorld,
    entity: u64,
    f: impl FnOnce(&LevelObject) -> Result<R, Failure>,
) -> Result<R, Failure> {
    // SAFETY: forwarded caller contract.
    unsafe {
        access(world, |w, _| {
            let obj = w
                .get::<LevelObject>(Entity::from_bits(entity))
                .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "entity is not a level object"))?;
            f(obj)
        })
    }
}

/// Reads the level object `entity` (spawned by `ponko_ctx_spawn_level`).
/// `PONKO_NOT_FOUND` if it isn't one.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_level_object_get(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `out_object` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_level_object_get(
    world: *mut PonkoWorld,
    entity: u64,
    out_object: *mut PonkoLevelObject,
) -> PonkoStatus {
    boundary(|| {
        if out_object.is_null() {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract; output checked non-null.
        unsafe {
            with_object(world, entity, |o| {
                let shape = match o.shape {
                    ObjectShape::Rect => PONKO_LEVEL_SHAPE_RECT,
                    ObjectShape::Ellipse => PONKO_LEVEL_SHAPE_ELLIPSE,
                    ObjectShape::Point => PONKO_LEVEL_SHAPE_POINT,
                    ObjectShape::Polygon(_) => PONKO_LEVEL_SHAPE_POLYGON,
                    _ => PONKO_LEVEL_SHAPE_POLYLINE,
                };
                let (tileset, tile, flip) = o.tile.unwrap_or_default();
                out(
                    out_object,
                    PonkoLevelObject {
                        id: o.id,
                        position: c_vec(o.position),
                        size: c_vec(o.size),
                        rotation: o.rotation,
                        shape,
                        has_tile: o.tile.is_some(),
                        tileset: u32::from(tileset),
                        tile,
                        flip: u32::from(flip),
                        visible: o.visible,
                    },
                )
            })
        }
    })
}

/// Writes one of a level object's names (`(buffer, capacity, out_len)`, as
/// `ponko_last_error_text`: the length always, the text when it fits).
unsafe fn object_text(
    world: *mut PonkoWorld,
    entity: u64,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
    pick: fn(&LevelObject) -> &str,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { with_object(world, entity, |o| write_text(buffer, capacity, out_len, pick(o))) })
}

/// A level object's name (Tiled's object name; empty for LDtk entities).
/// Writes its length to `out_len` and the text to `buffer` when it fits.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_level_object_name(std::ptr::null_mut(), 0, std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `buffer` holds `capacity` bytes (or null with 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_level_object_name(
    world: *mut PonkoWorld,
    entity: u64,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { object_text(world, entity, buffer, capacity, out_len, |o| &o.name) }
}

/// A level object's class (Tiled) or entity identifier (LDtk), as
/// `ponko_level_object_name`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_level_object_class(std::ptr::null_mut(), 0, std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// As `ponko_level_object_name`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_level_object_class(
    world: *mut PonkoWorld,
    entity: u64,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { object_text(world, entity, buffer, capacity, out_len, |o| &o.class) }
}

/// The name of a level object's layer, as `ponko_level_object_name`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_level_object_layer(std::ptr::null_mut(), 0, std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// As `ponko_level_object_name`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_level_object_layer(
    world: *mut PonkoWorld,
    entity: u64,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { object_text(world, entity, buffer, capacity, out_len, |o| &o.layer) }
}

/// A custom property of a spawned level's object, layer or root (whichever
/// `entity` is), as JSON text: `5`, `"coin.wav"`, `true`, `{"lives": 3}`.
/// Writes its length to `out_len` and the text to `buffer` when it fits.
/// `PONKO_NOT_FOUND` if the entity has no such property.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_level_property_json(std::ptr::null_mut(), 0, c"x".as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `name` NUL-terminated; `buffer` holds `capacity`
/// bytes (or null with 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_level_property_json(
    world: *mut PonkoWorld,
    entity: u64,
    name: *const c_char,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let name = unsafe { text(name)? };
        // SAFETY: forwarded caller contract.
        unsafe {
            access(world, |w, _| {
                let e = Entity::from_bits(entity);
                let value = w
                    .get::<LevelObject>(e)
                    .and_then(|o| o.properties.get(name))
                    .or_else(|| w.get::<LevelLayer>(e).and_then(|l| l.properties.get(name)))
                    .or_else(|| w.get::<LevelRoot>(e).and_then(|r| r.properties.get(name)))
                    .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "no such level property here"))?;
                write_text(buffer, capacity, out_len, &value.to_string())
            })
        }
    })
}

/// Runs `f` on `entity`'s tilemap in place, so the tile array a C caller
/// read with `ponko_tilemap_get` stays valid (unless `f` resizes it).
unsafe fn with_map<R>(
    world: *mut PonkoWorld,
    entity: u64,
    f: impl FnOnce(&mut TileMap) -> Result<R, Failure>,
) -> Result<R, Failure> {
    // SAFETY: forwarded caller contract.
    unsafe {
        access(world, |w, _| {
            let map = w
                .get_mut::<TileMap>(Entity::from_bits(entity))
                .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "entity has no tilemap"))?;
            f(map)
        })
    }
}

/// Makes `entity`'s tilemap move with the camera by `parallax` (`{1, 1}`
/// moves with the world, `{0, 0}` stays on the screen), lining up when the
/// camera's center is at `origin`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_set_parallax(std::ptr::null_mut(), 0, Default::default(), Default::default()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_tilemap_set_parallax(
    world: *mut PonkoWorld,
    entity: u64,
    parallax: PonkoVec2,
    origin: PonkoVec2,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_map(world, entity, |m| {
            m.parallax = vec(parallax);
            m.parallax_origin = vec(origin);
            Ok(())
        })
    })
}

/// Animates tile `tile` of `entity`'s tilemap: wherever the map holds it,
/// `frames[i]` shows for `seconds[i]`, in a loop. `count` 0 removes the
/// animation.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_animate(std::ptr::null_mut(), 0, 0, std::ptr::null(), std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `frames` and `seconds` hold `count` values (or
/// are null with 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_tilemap_animate(
    world: *mut PonkoWorld,
    entity: u64,
    tile: u16,
    frames: *const u16,
    seconds: *const f32,
    count: usize,
) -> PonkoStatus {
    boundary(|| {
        if count > 0 && (frames.is_null() || seconds.is_null()) {
            return Err(invalid("frames or seconds is null"));
        }
        let list: Vec<(u16, f32)> = if count == 0 {
            Vec::new()
        } else {
            // SAFETY: the caller supplies `count` values in each.
            let (f, s) = unsafe {
                (
                    std::slice::from_raw_parts(frames, count),
                    std::slice::from_raw_parts(seconds, count),
                )
            };
            f.iter().copied().zip(s.iter().copied()).collect()
        };
        // SAFETY: forwarded caller contract.
        unsafe {
            with_map(world, entity, |m| {
                m.animate(tile, &list);
                Ok(())
            })
        }
    })
}

/// Sets tile `(x, y)` of `entity`'s tilemap to `frame`
/// (`PONKO_TILE_EMPTY` for none), flipped by `flip` (`PONKO_TILE_FLIP_*`
/// bits). Out-of-bounds positions are ignored.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_set_flipped(std::ptr::null_mut(), 0, 0, 0, 0, 0) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_tilemap_set_flipped(
    world: *mut PonkoWorld,
    entity: u64,
    x: u32,
    y: u32,
    frame: u16,
    flip: u8,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        with_map(world, entity, |m| {
            m.set_flipped(x, y, (frame != TileMap::EMPTY).then_some(frame), flip);
            Ok(())
        })
    })
}

/// How tile `(x, y)` of `entity`'s tilemap is flipped (0 if not or out of bounds).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_flip(std::ptr::null_mut(), 0, 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `out_flip` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_tilemap_flip(
    world: *mut PonkoWorld,
    entity: u64,
    x: u32,
    y: u32,
    out_flip: *mut u8,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { with_map(world, entity, |m| out(out_flip, m.flip(x, y))) })
}

/// An autotile rule: `neighbours` 4 with 16 frames (by mask) or 8 with 47
/// (by `AutoTile::BLOB_MASKS`).
unsafe fn rule(neighbours: u32, frames: *const u16, frames_len: usize, outside: bool) -> Result<AutoTile, Failure> {
    let want = match neighbours {
        4 => 16,
        8 => 47,
        _ => return Err(invalid("neighbours is 4 or 8")),
    };
    if frames.is_null() || frames_len != want {
        return Err(invalid("frames: 16 for 4 neighbours, 47 for 8"));
    }
    // SAFETY: checked non-null; the caller supplies `frames_len` values.
    let f = unsafe { std::slice::from_raw_parts(frames, frames_len) };
    let rule = if neighbours == 4 {
        AutoTile::four(std::array::from_fn(|i| f[i]))
    } else {
        AutoTile::blob(std::array::from_fn(|i| f[i]))
    };
    Ok(rule.outside(outside))
}

/// Fills `entity`'s tilemap with an autotile rule: every cell `filled` (one
/// bool per cell, row by row) marks gets the frame for its filled
/// neighbours; other cells are left alone. `neighbours` is 4 (`frames`: 16,
/// by mask N=1 E=2 S=4 W=8) or 8 (`frames`: 47, one per blob mask in
/// increasing order). `outside_filled`: cells off the map count as filled.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_autotile(std::ptr::null_mut(), 0, 4, std::ptr::null(), 0, true, std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `frames` holds `frames_len` values; `filled` holds
/// one bool per cell of the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_tilemap_autotile(
    world: *mut PonkoWorld,
    entity: u64,
    neighbours: u32,
    frames: *const u16,
    frames_len: usize,
    outside_filled: bool,
    filled: *const bool,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let rule = unsafe { rule(neighbours, frames, frames_len, outside_filled)? };
        if filled.is_null() {
            return Err(invalid("filled is null"));
        }
        // SAFETY: forwarded caller contract.
        unsafe {
            with_map(world, entity, |m| {
                let w = m.size.x;
                // SAFETY: the caller supplies one bool per cell.
                let cells = std::slice::from_raw_parts(filled, m.tiles.len());
                m.autotile(&rule, |x, y| cells[(y * w + x) as usize]);
                Ok(())
            })
        }
    })
}

/// Fills (`on`) or empties cell `(x, y)` of `entity`'s tilemap and redoes it
/// and its neighbours with the rule (as `ponko_tilemap_autotile`); a cell
/// holding one of the rule's frames counts as filled.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_paint_autotile(std::ptr::null_mut(), 0, 4, std::ptr::null(), 0, true, 0, 0, true) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `frames` holds `frames_len` values.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn ponko_tilemap_paint_autotile(
    world: *mut PonkoWorld,
    entity: u64,
    neighbours: u32,
    frames: *const u16,
    frames_len: usize,
    outside_filled: bool,
    x: u32,
    y: u32,
    on: bool,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let rule = unsafe { rule(neighbours, frames, frames_len, outside_filled)? };
        // SAFETY: forwarded caller contract.
        unsafe {
            with_map(world, entity, |m| {
                m.paint_autotile(&rule, x, y, on);
                Ok(())
            })
        }
    })
}

unsafe fn solid_set<'a>(tiles: *const u16, count: usize) -> Result<&'a [u16], Failure> {
    if count == 0 {
        return Ok(&[]);
    }
    if tiles.is_null() {
        return Err(invalid("solid_tiles is null"));
    }
    // SAFETY: the caller supplies `count` values.
    Ok(unsafe { std::slice::from_raw_parts(tiles, count) })
}

/// The tiles of `entity`'s tilemap whose value is in `solid_tiles`, merged
/// into few rectangles relative to the map's top-left (world units). Writes
/// up to `capacity` of them and the total to `out_len`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_solid_rects(std::ptr::null_mut(), 0, std::ptr::null(), 0, std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `solid_tiles` holds `count` values; `out_rects`
/// holds `capacity` (or is null with 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_tilemap_solid_rects(
    world: *mut PonkoWorld,
    entity: u64,
    solid_tiles: *const u16,
    count: usize,
    out_rects: *mut PonkoRect,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        if out_len.is_null() || (out_rects.is_null() && capacity != 0) {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract.
        let solid = unsafe { solid_set(solid_tiles, count)? };
        // SAFETY: forwarded caller contract.
        unsafe {
            with_map(world, entity, |m| {
                let rects = m.solid_rects(|t| solid.contains(&t));
                for (i, r) in rects.iter().take(capacity).enumerate() {
                    // SAFETY: i < capacity (caller-sized buffer).
                    out_rects.add(i).write(PonkoRect {
                        min: c_vec(r.min),
                        max: c_vec(r.max),
                    });
                }
                out(out_len, rects.len())
            })
        }
    })
}

/// Spawns fixed physics bodies for the tiles of `entity`'s tilemap whose
/// value is in `solid_tiles`, merged into few rectangles, where the map's
/// `Transform2D` puts it. Writes how many to `out_count`. Needs the
/// `physics` feature; despawn the bodies (rigid body + collider entities)
/// to take the collision away.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_tilemap_spawn_colliders(std::ptr::null_mut(), 0, std::ptr::null(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `world` null or live; `solid_tiles` holds `count` values; `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_tilemap_spawn_colliders(
    world: *mut PonkoWorld,
    entity: u64,
    solid_tiles: *const u16,
    count: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        if out_count.is_null() {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract.
        let solid = unsafe { solid_set(solid_tiles, count)? };
        #[cfg(feature = "physics")]
        {
            // SAFETY: forwarded caller contract.
            unsafe {
                access(world, |w, _| {
                    let e = Entity::from_bits(entity);
                    let map = w
                        .get::<TileMap>(e)
                        .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "entity has no tilemap"))?;
                    let at = w.get::<ponko::d2::Transform>(e).map_or(Vec2::ZERO, |t| t.position);
                    let bodies = map.colliders(at, |t| solid.contains(&t));
                    let n = bodies.len();
                    w.spawn_batch(bodies);
                    out(out_count, n)
                })
            }
        }
        #[cfg(not(feature = "physics"))]
        {
            let _ = (world, entity, solid);
            Err(invalid("ponko_ffi was built without the `physics` feature"))
        }
    })
}
