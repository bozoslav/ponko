//! Streaming big worlds: the cell streamer, terrain tiles built on any
//! thread, far terrain beyond them, and freeing meshes and tiles. Background work itself is the
//! game's (C++ has `std::async` and threads); Rust's `Job` has no C API.

use ponko::terrain::FarTerrain;
use ponko::terrain::TerrainTile;
use ponko::terrain::TerrainTileData;
use ponko::{prelude::vec3, streaming::CellStreamer};

use crate::ctx::{ctx_access, out};
use crate::three::{PonkoMaterial, PonkoMesh, PonkoTerrainTile, PonkoVec3, square_grid};
use crate::types::PonkoVec2;
use crate::types::to_c;
use crate::{PonkoCtx, PonkoStatus, boundary, invalid, invalid_owned};

/// One cell of a `PonkoCellStreamer`'s grid: it covers `x * cell_size`
/// to `(x + 1) * cell_size` along X, and the same along Z.
/// ```
/// let c = ponko_ffi::PonkoCell { x: 2, z: -1 };
/// assert_eq!(c.z, -1);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoCell {
    /// Along X.
    pub x: i32,
    /// Along Z.
    pub z: i32,
}

/// Which cells of a square grid on the ground to have loaded around a
/// point (Rust `CellStreamer`). Make it with `ponko_cells_new`.
pub struct PonkoCellStreamer {
    cells: CellStreamer,
    to_load: Vec<PonkoCell>,
    to_drop: Vec<PonkoCell>,
}

/// Makes a streamer of cells `cell_size` wide, loading those within
/// `load_radius` of the point and dropping those beyond `keep_radius`.
/// Returns null (see `ponko_last_error`) if `cell_size` isn't above 0 or
/// a radius is negative.
/// ```
/// let s = ponko_ffi::ponko_cells_new(256.0, 600.0, 800.0);
/// assert!(!s.is_null());
/// unsafe { ponko_ffi::ponko_cells_free(s) };
/// assert!(ponko_ffi::ponko_cells_new(0.0, 1.0, 1.0).is_null());
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_cells_new(cell_size: f32, load_radius: f32, keep_radius: f32) -> *mut PonkoCellStreamer {
    let mut made = std::ptr::null_mut();
    let status = boundary(|| {
        if !(cell_size > 0.0 && cell_size.is_finite()) {
            return Err(invalid("cell size must be above 0"));
        }
        if !(load_radius >= 0.0 && keep_radius >= 0.0) {
            return Err(invalid("the radii can't be negative"));
        }
        made = Box::into_raw(Box::new(PonkoCellStreamer {
            cells: CellStreamer::new(cell_size, load_radius, keep_radius),
            to_load: Vec::new(),
            to_drop: Vec::new(),
        }));
        Ok(())
    });
    if status == PonkoStatus::PONKO_OK {
        made
    } else {
        std::ptr::null_mut()
    }
}

/// Frees a cell streamer. Null is ignored.
/// ```
/// unsafe { ponko_ffi::ponko_cells_free(std::ptr::null_mut()) };
/// ```
/// # Safety
/// `cells` must be null or live, and not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_cells_free(cells: *mut PonkoCellStreamer) {
    if !cells.is_null() {
        // SAFETY: caller passes a streamer from ponko_cells_new, once.
        drop(unsafe { Box::from_raw(cells) });
    }
}

unsafe fn cells_mut<'a>(cells: *mut PonkoCellStreamer) -> Result<&'a mut PonkoCellStreamer, crate::Failure> {
    // SAFETY: caller supplies a live streamer with no concurrent use.
    unsafe { cells.as_mut() }.ok_or_else(|| invalid("cell streamer is null"))
}

/// Works out the cells to load and drop around `at` (its height is
/// ignored): at most `max_new` new ones, nearest first (Rust
/// `CellStreamer::update`). Read them with `ponko_cells_to_load` and
/// `ponko_cells_to_drop`.
/// ```
/// use ponko_ffi::*;
/// let s = ponko_cells_new(1.0, 0.4, 0.4);
/// let (mut cells, mut len) = (std::ptr::null(), 0);
/// unsafe {
///     assert_eq!(ponko_cells_update(s, PonkoVec3 { x: 0.5, y: 0.0, z: 0.5 }, 10), PonkoStatus::PONKO_OK);
///     assert_eq!(ponko_cells_to_load(s, &mut cells, &mut len), PonkoStatus::PONKO_OK);
///     assert_eq!((len, *cells), (1, PonkoCell { x: 0, z: 0 }));
///     ponko_cells_free(s);
/// }
/// ```
/// # Safety
/// `cells` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_cells_update(
    cells: *mut PonkoCellStreamer,
    at: PonkoVec3,
    max_new: usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let s = unsafe { cells_mut(cells)? };
        s.cells.update(vec3(at.x, at.y, at.z), max_new);
        let cell = |&(x, z): &(i32, i32)| PonkoCell { x, z };
        s.to_load.clear();
        s.to_load.extend(s.cells.to_load().iter().map(cell));
        s.to_drop.clear();
        s.to_drop.extend(s.cells.to_drop().iter().map(cell));
        Ok(())
    })
}

unsafe fn cell_list(
    cells: *mut PonkoCellStreamer,
    out_cells: *mut *const PonkoCell,
    out_len: *mut usize,
    pick: impl FnOnce(&PonkoCellStreamer) -> &[PonkoCell],
) -> PonkoStatus {
    boundary(|| {
        if out_cells.is_null() || out_len.is_null() {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract; outputs checked non-null.
        unsafe {
            let list = pick(cells_mut(cells)?);
            out_cells.write(list.as_ptr());
            out_len.write(list.len());
        }
        Ok(())
    })
}

/// The cells the last `ponko_cells_update` started loading, nearest
/// first. Valid until the next update or free.
/// ```
/// let (mut c, mut n) = (std::ptr::null(), 0);
/// assert_eq!(unsafe { ponko_ffi::ponko_cells_to_load(std::ptr::null_mut(), &mut c, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `cells` must be null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_cells_to_load(
    cells: *mut PonkoCellStreamer,
    out_cells: *mut *const PonkoCell,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { cell_list(cells, out_cells, out_len, |s| &s.to_load) }
}

/// The cells the last `ponko_cells_update` dropped, in grid order. Valid
/// until the next update or free.
/// ```
/// use ponko_ffi::*;
/// let s = ponko_cells_new(1.0, 0.4, 0.4);
/// let (mut cells, mut len) = (std::ptr::null(), 0);
/// unsafe {
///     ponko_cells_update(s, PonkoVec3 { x: 0.5, y: 0.0, z: 0.5 }, 10);
///     ponko_cells_update(s, PonkoVec3 { x: 10.5, y: 0.0, z: 0.5 }, 10);
///     ponko_cells_to_drop(s, &mut cells, &mut len);
///     assert_eq!((len, *cells), (1, PonkoCell { x: 0, z: 0 }));
///     ponko_cells_free(s);
/// }
/// ```
/// # Safety
/// `cells` must be null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_cells_to_drop(
    cells: *mut PonkoCellStreamer,
    out_cells: *mut *const PonkoCell,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { cell_list(cells, out_cells, out_len, |s| &s.to_drop) }
}

/// Whether `cell` is loaded (or being loaded), and how many are.
/// `out_count` may be null.
/// ```
/// use ponko_ffi::*;
/// let s = ponko_cells_new(1.0, 0.4, 0.4);
/// let (mut loaded, mut count) = (false, 0);
/// unsafe {
///     ponko_cells_update(s, PonkoVec3 { x: 0.5, y: 0.0, z: 0.5 }, 10);
///     ponko_cells_is_loaded(s, PonkoCell { x: 0, z: 0 }, &mut loaded, &mut count);
///     assert!(loaded);
///     assert_eq!(count, 1);
///     ponko_cells_free(s);
/// }
/// ```
/// # Safety
/// `cells` must be null or live; `out_loaded` writable, `out_count` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_cells_is_loaded(
    cells: *mut PonkoCellStreamer,
    cell: PonkoCell,
    out_loaded: *mut bool,
    out_count: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        unsafe {
            let s = cells_mut(cells)?;
            out(out_loaded, s.cells.is_loaded((cell.x, cell.z)))?;
            if !out_count.is_null() {
                out_count.write(s.cells.loaded_count());
            }
        }
        Ok(())
    })
}

/// Forgets every cell, as if none were loaded.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_cells_clear(std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `cells` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_cells_clear(cells: *mut PonkoCellStreamer) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let s = unsafe { cells_mut(cells)? };
        s.cells.clear();
        s.to_load.clear();
        s.to_drop.clear();
        Ok(())
    })
}

/// A terrain tile's meshes at every detail level, built on the CPU (Rust
/// `TerrainTileData`): make it on any thread with
/// `ponko_terrain_tile_data_new`, then upload it on the game's with
/// `ponko_ctx_upload_terrain_tile`.
pub struct PonkoTerrainTileData(TerrainTileData);

/// Builds a tile `size` wide from `count` heights (n x n, row after row
/// along +Z, NaN for a hole). Safe to call from any thread. Returns null
/// (see `ponko_last_error`) for bad input.
/// ```
/// let h = [0.0f32; 9];
/// let d = unsafe { ponko_ffi::ponko_terrain_tile_data_new(h.as_ptr(), 9, 4.0) };
/// assert!(!d.is_null());
/// unsafe { ponko_ffi::ponko_terrain_tile_data_free(d) };
/// assert!(unsafe { ponko_ffi::ponko_terrain_tile_data_new(h.as_ptr(), 8, 4.0) }.is_null());
/// ```
/// # Safety
/// `heights` holds `count` floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_terrain_tile_data_new(
    heights: *const f32,
    count: usize,
    size: f32,
) -> *mut PonkoTerrainTileData {
    let mut made = std::ptr::null_mut();
    let status = boundary(|| {
        if heights.is_null() {
            return Err(invalid("heights is null"));
        }
        if !square_grid(count) {
            return Err(invalid("the heights aren't a square grid of at least 2 x 2"));
        }
        if !(size > 0.0 && size.is_finite()) {
            return Err(invalid("size must be above 0"));
        }
        // SAFETY: caller supplies `count` floats.
        let heights = unsafe { std::slice::from_raw_parts(heights, count) };
        made = Box::into_raw(Box::new(PonkoTerrainTileData(TerrainTileData::new(heights, size))));
        Ok(())
    });
    if status == PonkoStatus::PONKO_OK {
        made
    } else {
        std::ptr::null_mut()
    }
}

/// Frees tile data. Null is ignored.
/// ```
/// unsafe { ponko_ffi::ponko_terrain_tile_data_free(std::ptr::null_mut()) };
/// ```
/// # Safety
/// `data` must be null or live, and not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_terrain_tile_data_free(data: *mut PonkoTerrainTileData) {
    if !data.is_null() {
        // SAFETY: caller passes data from ponko_terrain_tile_data_new, once.
        drop(unsafe { Box::from_raw(data) });
    }
}

/// Puts a tile built by `ponko_terrain_tile_data_new` on the GPU (Rust
/// `Graphics::upload_terrain_tile`). The data stays the caller's to free.
/// # Safety
/// `ctx` null or live; `data` live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_upload_terrain_tile(std::ptr::null_mut(), std::ptr::null(), Default::default(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_upload_terrain_tile(
    ctx: *mut PonkoCtx,
    data: *const PonkoTerrainTileData,
    material: PonkoMaterial,
    out_tile: *mut PonkoTerrainTile,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(data) = data.as_ref() else {
                return Err(invalid("tile data is null"));
            };
            if !c.gfx.is_live(to_c(material)) {
                return Err(invalid("that material wasn't made by this game"));
            }
            out(out_tile, to_c(c.gfx.upload_terrain_tile(&data.0, to_c(material))))
        })
    })
}

/// Frees `mesh`'s GPU memory (Rust `Graphics::free_mesh`). Models still showing
/// it draw nothing. Freeing it again does nothing.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_free_mesh(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_free_mesh(ctx: *mut PonkoCtx, mesh: PonkoMesh) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.free(to_c(mesh));
            Ok(())
        })
    })
}

/// Whether `mesh` was made by this game and not freed.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// let mut live = true;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_is_mesh_live(std::ptr::null_mut(), Default::default(), &mut live) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_is_mesh_live(
    ctx: *mut PonkoCtx,
    mesh: PonkoMesh,
    out_live: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_live, c.gfx.is_live(to_c(mesh)))) })
}

/// Frees every detail level of `tile` (Rust `Graphics::free` with the
/// tile); despawn its entity too.
/// # Safety
/// `ctx` null or live.
/// ```
/// let t = ponko_ffi::PonkoTerrainTile { lods: [Default::default(); 4], lod_distance: 1.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_free_terrain_tile(std::ptr::null_mut(), t) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_free_terrain_tile(ctx: *mut PonkoCtx, tile: PonkoTerrainTile) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.free::<TerrainTile>(to_c(tile));
            Ok(())
        })
    })
}

/// Low-detail ground for the whole world beyond the loaded terrain tiles
/// (Rust `FarTerrain`). Make it with `ponko_far_terrain_new`.
pub struct PonkoFarTerrain {
    far: FarTerrain,
    material: ponko::render::Material,
}

/// Makes far ground from `count` heights, `columns` per row (row after row
/// along +Z, NaN for a hole), the first at `corner` (x, z) and the rest
/// `spacing` apart, drawn with `material`, cut a cell at a time for tiles
/// `cell_size` wide. `cell_size` must be a whole number of `spacing`s and
/// `corner` lie on the grid of `spacing`s. Nothing is made until
/// `ponko_far_terrain_update`. Null (see `ponko_last_error`) for bad input.
/// ```
/// use ponko_ffi::*;
/// let h = [0.0f32; 16];
/// let f = unsafe { ponko_far_terrain_new(h.as_ptr(), 16, 4, PonkoVec2::default(), 10.0, 20.0, Default::default()) };
/// assert!(!f.is_null());
/// unsafe { ponko_far_terrain_free(f) };
/// assert!(unsafe { ponko_far_terrain_new(h.as_ptr(), 16, 4, PonkoVec2::default(), 10.0, 25.0, Default::default()) }.is_null());
/// ```
/// # Safety
/// `heights` holds `count` floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_far_terrain_new(
    heights: *const f32,
    count: usize,
    columns: usize,
    corner: PonkoVec2,
    spacing: f32,
    cell_size: f32,
    material: PonkoMaterial,
) -> *mut PonkoFarTerrain {
    let mut made = std::ptr::null_mut();
    let status = boundary(|| {
        if heights.is_null() {
            return Err(invalid("heights is null"));
        }
        // SAFETY: caller supplies `count` floats.
        let heights = unsafe { std::slice::from_raw_parts(heights, count) };
        let material = to_c(material);
        let far =
            FarTerrain::try_new(heights, columns, to_c(corner), spacing, cell_size, material).map_err(invalid_owned)?;
        made = Box::into_raw(Box::new(PonkoFarTerrain { far, material }));
        Ok(())
    });
    if status == PonkoStatus::PONKO_OK {
        made
    } else {
        std::ptr::null_mut()
    }
}

/// Frees a far terrain. Null is ignored. Its entities and meshes stay in
/// the world: `ponko_far_terrain_clear` them first.
/// ```
/// unsafe { ponko_ffi::ponko_far_terrain_free(std::ptr::null_mut()) };
/// ```
/// # Safety
/// `far` must be null or live, and not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_far_terrain_free(far: *mut PonkoFarTerrain) {
    if !far.is_null() {
        // SAFETY: caller passes a far terrain from ponko_far_terrain_new, once.
        drop(unsafe { Box::from_raw(far) });
    }
}

unsafe fn far_mut<'a>(far: *mut PonkoFarTerrain) -> Result<&'a mut PonkoFarTerrain, crate::Failure> {
    // SAFETY: caller supplies a live far terrain with no concurrent use.
    unsafe { far.as_mut() }.ok_or_else(|| invalid("far terrain is null"))
}

/// Cuts the far ground away over `cell` (`cut` true), whose own tile is now
/// shown, or fills it in again (`cut` false) when the tile goes (Rust
/// `FarTerrain::cut` and `uncut`). Takes effect at the next update.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_far_terrain_cut(std::ptr::null_mut(), Default::default(), true) } as u32, 1);
/// ```
/// # Safety
/// `far` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_far_terrain_cut(far: *mut PonkoFarTerrain, cell: PonkoCell, cut: bool) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let f = unsafe { far_mut(far)? };
        if cut {
            f.far.cut((cell.x, cell.z));
        } else {
            f.far.uncut((cell.x, cell.z));
        }
        Ok(())
    })
}

/// Whether `cell` is cut away, and how many meshes the far ground has in
/// the world. `out_meshes` may be null.
/// ```
/// use ponko_ffi::*;
/// let h = [0.0f32; 16];
/// let f = unsafe { ponko_far_terrain_new(h.as_ptr(), 16, 4, PonkoVec2::default(), 10.0, 10.0, Default::default()) };
/// let (mut cut, mut meshes) = (false, 9);
/// unsafe {
///     ponko_far_terrain_cut(f, PonkoCell { x: 1, z: 1 }, true);
///     assert_eq!(ponko_far_terrain_is_cut(f, PonkoCell { x: 1, z: 1 }, &mut cut, &mut meshes), PonkoStatus::PONKO_OK);
///     ponko_far_terrain_free(f);
/// }
/// assert_eq!((cut, meshes), (true, 0));
/// ```
/// # Safety
/// `far` must be null or live; `out_cut` writable, `out_meshes` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_far_terrain_is_cut(
    far: *mut PonkoFarTerrain,
    cell: PonkoCell,
    out_cut: *mut bool,
    out_meshes: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        unsafe {
            let f = far_mut(far)?;
            out(out_cut, f.far.is_cut((cell.x, cell.z)))?;
            if !out_meshes.is_null() {
                out_meshes.write(f.far.mesh_count());
            }
        }
        Ok(())
    })
}

/// Builds the far ground's meshes that changed since the last update (all
/// of them the first time) and puts them in the world (Rust
/// `FarTerrain::update`).
/// # Safety
/// `far` and `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_far_terrain_update(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_far_terrain_update(far: *mut PonkoFarTerrain, ctx: *mut PonkoCtx) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let f = unsafe { far_mut(far)? };
        // SAFETY: forwarded caller contract.
        unsafe {
            ctx_access(ctx, |c| {
                if !c.gfx.is_live(f.material) {
                    return Err(invalid("that material wasn't made by this game"));
                }
                f.far.update(c);
                Ok(())
            })
        }
    })
}

/// Despawns the far ground's entities and frees its meshes (Rust
/// `FarTerrain::clear`); the next update builds them again.
/// # Safety
/// `far` and `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_far_terrain_clear(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_far_terrain_clear(far: *mut PonkoFarTerrain, ctx: *mut PonkoCtx) -> PonkoStatus {
    boundary(|| {
        // SAFETY: forwarded caller contract.
        let f = unsafe { far_mut(far)? };
        // SAFETY: forwarded caller contract.
        unsafe {
            ctx_access(ctx, |c| {
                f.far.clear(c);
                Ok(())
            })
        }
    })
}
