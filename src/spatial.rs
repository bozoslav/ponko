//! [`SpatialGrid`]: fast "who is near this point?" queries for huge numbers of entities.

use glam::Vec2;
use ponko_ecs::{Entity, World};

use crate::ctx::Ctx;
use crate::ecs::Transform2D;

/// One entity in the grid: its handle, where it was at the last rebuild,
/// and optional data stored with it (see [`SpatialGrid::rebuild_with_data`]).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)] // C/C++ read query results directly
pub struct GridEntry<T = ()> {
    /// The entity.
    pub entity: Entity,
    /// Where it was at the last rebuild.
    pub position: Vec2,
    /// Data stored with it (`()` unless rebuilt with data).
    pub data: T,
}

/// A uniform grid over the whole (unbounded) world, rebuilt from scratch
/// each tick: answers radius, rectangle and nearest-neighbor queries without
/// checking every entity.
///
/// Enable it with [`Game::spatial_grid`](crate::game::Game::spatial_grid): every
/// entity with a [`d2::Transform`](crate::d2::Transform) is then indexed at the start of each
/// simulation tick, and you query it through `ctx.spatial`, even from inside a
/// (parallel) query:
///
/// ```
/// use ponko::prelude::*;
///
/// #[derive(Component)]
/// struct Unit;
/// #[derive(Component)]
/// struct Crowded(bool);
///
/// fn crowding(ctx: &mut Ctx) {
///     let grid = &ctx.spatial;
///     ctx.world.query::<(Entity, &ponko::d2::Transform, &mut Crowded)>().par_for_each(|(me, t, crowded)| {
///         let mut near = 0;
///         grid.for_each_in_radius(t.position, 50.0, |other| {
///             if other.entity != me {
///                 near += 1;
///             }
///         });
///         crowded.0 = near >= 3;
///     });
/// }
///
/// let mut world = Game::new("sim", 800, 600)
///     .spatial_grid(50.0)
///     .sim_system(crowding)
///     .run_headless(1, |ctx| {
///         for i in 0..5 {
///             ctx.world.spawn((ponko::d2::Transform::at(vec2(i as f32, 0.0)), Unit, Crowded(false)));
///         }
///         ctx.world.spawn((ponko::d2::Transform::at(vec2(500.0, 500.0)), Unit, Crowded(false)));
///     });
/// assert_eq!(world.query::<&Crowded>().iter().filter(|c| c.0).count(), 5);
/// ```
///
/// Pick a cell size close to your typical query radius. Positions are those
/// at the last rebuild; entities that move during a tick are found where they
/// were when it started.
///
/// To index only some entities, remove the built-in
/// [`rebuild_spatial_grid`](crate::systems::rebuild_spatial_grid) system and call
/// [`rebuild_from`](Self::rebuild_from) yourself.
///
/// A grid can also carry data per entry (a velocity, a team), for queries
/// that need neighbors' state while the world is busy with a query. Keep
/// such a grid in your system's closure:
///
/// ```
/// use ponko::prelude::*;
///
/// #[derive(Component)]
/// struct Vel(Vec2);
///
/// let align = {
///     let mut grid = ponko::spatial::SpatialGrid::with_data(20.0);
///     move |ctx: &mut Ctx| {
///         grid.rebuild_with_data(
///             ctx.world.query::<(Entity, &ponko::d2::Transform, &Vel)>().iter().map(|(e, t, v)| (e, t.position, v.0)),
///         );
///         let grid = &grid;
///         ctx.world.query::<(&ponko::d2::Transform, &mut Vel)>().par_for_each(|(t, v)| {
///             let mut sum = Vec2::ZERO;
///             grid.for_each_in_radius(t.position, 20.0, |n| sum += n.data);
///             v.0 = v.0.lerp(sum.normalize_or_zero() * v.0.length(), 0.1);
///         });
///     }
/// };
/// Game::new("flock", 800, 600).sim_system(align).run_headless(1, |_| {});
/// ```
#[derive(Debug, Clone)]
pub struct SpatialGrid<T = ()> {
    cell_size: f32,
    inv_cell: f32,
    buckets_of: Buckets,
    /// `starts[b]..starts[b + 1]` is bucket `b`'s range in `entries`.
    starts: Vec<u32>,
    /// All entries, sorted by bucket.
    entries: Vec<GridEntry<T>>,
    /// Packed cell of each entry (same order), so skipping hash-colliding
    /// cells is one integer compare instead of recomputing the cell.
    cells: Vec<u64>,
    // Reused between rebuilds so steady-state rebuilds don't allocate.
    unsorted: Vec<GridEntry<T>>,
    buckets: Vec<u32>,
    cursor: Vec<u32>,
    order: Vec<u32>,
}

impl<T: Copy + Send + Sync> Default for SpatialGrid<T> {
    fn default() -> Self {
        Self::with_data(64.0)
    }
}

impl SpatialGrid {
    /// An empty grid with square cells of `cell_size` world units.
    ///
    /// # Panics
    /// If `cell_size` isn't a positive, finite number.
    #[track_caller]
    #[must_use]
    pub fn new(cell_size: f32) -> Self {
        Self::with_data(cell_size)
    }

    /// Re-indexes every entity that has a [`d2::Transform`](crate::d2::Transform).
    pub fn rebuild(&mut self, world: &mut World) {
        let mut unsorted = std::mem::take(&mut self.unsorted);
        unsorted.clear();
        world.query::<(Entity, &Transform2D)>().for_each(|(entity, t)| {
            unsorted.push(GridEntry {
                entity,
                position: t.position,
                data: (),
            })
        });
        self.unsorted = unsorted;
        self.sort_unsorted();
    }

    /// Re-indexes from your own list of `(entity, position)`, replacing
    /// everything indexed before.
    pub fn rebuild_from(&mut self, items: impl IntoIterator<Item = (Entity, Vec2)>) {
        self.rebuild_with_data(items.into_iter().map(|(e, p)| (e, p, ())));
    }
}

impl<T: Copy + Send + Sync> SpatialGrid<T> {
    /// An empty grid that stores a `T` with every entry (see
    /// [`rebuild_with_data`](Self::rebuild_with_data)), with square cells of
    /// `cell_size` world units.
    ///
    /// # Panics
    /// If `cell_size` isn't a positive, finite number.
    #[track_caller]
    #[must_use]
    pub fn with_data(cell_size: f32) -> Self {
        assert!(
            cell_size.is_finite() && cell_size > 0.0,
            "SpatialGrid: cell size must be a positive number (got {cell_size})"
        );
        SpatialGrid {
            cell_size,
            inv_cell: 1.0 / cell_size,
            buckets_of: Buckets::new(1),
            starts: vec![0, 0],
            entries: Vec::new(),
            cells: Vec::new(),
            unsorted: Vec::new(),
            buckets: Vec::new(),
            cursor: Vec::new(),
            order: Vec::new(),
        }
    }

    /// Side length of one cell.
    #[must_use]
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Number of indexed entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if nothing is indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every indexed entity, grouped by cell.
    #[must_use]
    pub fn entries(&self) -> &[GridEntry<T>] {
        &self.entries
    }

    /// Re-indexes from `(entity, position, data)`, replacing everything
    /// indexed before. Queries hand each entry's `data` back.
    pub fn rebuild_with_data(&mut self, items: impl IntoIterator<Item = (Entity, Vec2, T)>) {
        self.unsorted.clear();
        self.unsorted.extend(
            items
                .into_iter()
                .map(|(entity, position, data)| GridEntry { entity, position, data }),
        );
        self.sort_unsorted();
    }

    /// Counting sort of `unsorted` into `entries` by bucket. The per-entry
    /// steps run on all cores for big grids; counting is one pass.
    fn sort_unsorted(&mut self) {
        let n = self.unsorted.len();
        assert!(n < u32::MAX as usize, "SpatialGrid: too many entities");
        // About 4 entries per bucket: with locality-preserving buckets this
        // is fastest (smaller arrays to clear and sum), and dense worlds
        // still get a bucket per occupied cell.
        let bucket_count = (n / 4).max(16).next_power_of_two();
        self.buckets_of = Buckets::new(bucket_count);
        let (buckets_of, inv_cell) = (self.buckets_of, self.inv_cell);
        let bucket_of = |e: &GridEntry<T>| {
            let (cx, cy) = cell_of(e.position, inv_cell);
            buckets_of.of(cx, cy) as u32
        };
        let parallel = n >= PARALLEL_REBUILD_MIN;
        map_into(&self.unsorted, &mut self.buckets, parallel, bucket_of);

        self.starts.clear();
        self.starts.resize(bucket_count + 1, 0);
        for &b in &self.buckets {
            self.starts[b as usize + 1] += 1;
        }
        for i in 0..bucket_count {
            self.starts[i + 1] += self.starts[i];
        }

        self.cursor.clear();
        self.cursor.extend_from_slice(&self.starts[..bucket_count]);
        self.order.clear();
        self.order.resize(n, 0);
        for (i, &b) in self.buckets.iter().enumerate() {
            let slot = &mut self.cursor[b as usize];
            self.order[*slot as usize] = i as u32;
            *slot += 1;
        }
        let unsorted = &self.unsorted;
        map_into(&self.order, &mut self.entries, parallel, |&i| unsorted[i as usize]);
        map_into(&self.entries, &mut self.cells, parallel, |e| {
            let (cx, cy) = cell_of(e.position, inv_cell);
            pack(cx, cy)
        });
    }

    /// Calls `f` for every entry in cell `(cx, cy)`. Buckets can hold several
    /// cells (hash collisions), so entries from other cells are skipped.
    #[inline]
    fn for_each_in_cell(&self, cx: i32, cy: i32, mut f: impl FnMut(&GridEntry<T>)) {
        let b = self.buckets_of.of(cx, cy);
        let range = self.starts[b] as usize..self.starts[b + 1] as usize;
        let key = pack(cx, cy);
        for (e, &c) in self.entries[range.clone()].iter().zip(&self.cells[range]) {
            if c == key {
                f(e);
            }
        }
    }

    /// `true` when a query box spans more cells than there are entries, so
    /// walking every entry beats visiting every cell (huge radii).
    #[inline]
    fn cheaper_to_scan(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
        let cells = (i64::from(x1) - i64::from(x0) + 1) * (i64::from(y1) - i64::from(y0) + 1);
        cells > self.entries.len() as i64
    }

    /// The grid's arrays, for language bindings that walk the grid inline
    /// (the C++ header). Valid until the next rebuild.
    #[doc(hidden)]
    #[must_use]
    pub fn raw_view(&self) -> RawGridView<'_, T> {
        RawGridView {
            entries: &self.entries,
            cells: &self.cells,
            starts: &self.starts,
            inv_cell: self.inv_cell,
            bucket_mask: self.buckets_of.mask as u64,
            x_bits: self.buckets_of.x_bits,
            y_bits: self.buckets_of.y_bits,
        }
    }

    /// Calls `f` for every entity within `radius` of `center` (inclusive).
    pub fn for_each_in_radius(&self, center: Vec2, radius: f32, mut f: impl FnMut(&GridEntry<T>)) {
        if self.entries.is_empty() || radius.is_nan() || radius < 0.0 {
            return;
        }
        let r2 = radius * radius;
        let (x0, y0) = cell_of(center - Vec2::splat(radius), self.inv_cell);
        let (x1, y1) = cell_of(center + Vec2::splat(radius), self.inv_cell);
        if self.cheaper_to_scan(x0, y0, x1, y1) {
            self.entries
                .iter()
                .filter(|e| e.position.distance_squared(center) <= r2)
                .for_each(f);
            return;
        }
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                self.for_each_in_cell(cx, cy, |e| {
                    if e.position.distance_squared(center) <= r2 {
                        f(e);
                    }
                });
            }
        }
    }

    /// Calls `f` for every entity inside the rectangle `min..=max`.
    pub fn for_each_in_rect(&self, min: Vec2, max: Vec2, mut f: impl FnMut(&GridEntry<T>)) {
        if self.entries.is_empty() || min.x > max.x || min.y > max.y {
            return;
        }
        let (x0, y0) = cell_of(min, self.inv_cell);
        let (x1, y1) = cell_of(max, self.inv_cell);
        let inside = |e: &&GridEntry<T>| e.position.cmpge(min).all() && e.position.cmple(max).all();
        if self.cheaper_to_scan(x0, y0, x1, y1) {
            self.entries.iter().filter(inside).for_each(f);
            return;
        }
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                self.for_each_in_cell(cx, cy, |e| {
                    if inside(&e) {
                        f(e);
                    }
                });
            }
        }
    }

    /// Number of entities within `radius` of `center`.
    #[must_use]
    pub fn count_in_radius(&self, center: Vec2, radius: f32) -> usize {
        let mut n = 0;
        self.for_each_in_radius(center, radius, |_| n += 1);
        n
    }

    /// The closest entity to `center` within `max_radius` for which `accept`
    /// returns `true` (use it to skip yourself or pick a team), if any.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(grid: &ponko::spatial::SpatialGrid, me: Entity, pos: Vec2) {
    /// let target = grid.nearest(pos, 200.0, |e| e.entity != me);
    /// # }
    /// ```
    pub fn nearest(
        &self,
        center: Vec2,
        max_radius: f32,
        mut accept: impl FnMut(&GridEntry<T>) -> bool,
    ) -> Option<GridEntry<T>> {
        let mut best: Option<(f32, GridEntry<T>)> = None;
        self.for_each_in_radius(center, max_radius, |e| {
            let d = e.position.distance_squared(center);
            if best.is_none_or(|(bd, _)| d < bd) && accept(e) {
                best = Some((d, *e));
            }
        });
        best.map(|(_, e)| e)
    }
}

#[inline(always)]
fn cell_of(p: Vec2, inv_cell: f32) -> (i32, i32) {
    // Saturating float→int casts keep far-away or non-finite points in edge cells.
    ((p.x * inv_cell).floor() as i32, (p.y * inv_cell).floor() as i32)
}

/// Z-order (Morton) key of the grid cell containing `position`: points that
/// are close in space usually get close keys. Sort entities by it so that
/// neighbors sit next to each other in memory:
///
/// ```
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// ctx.world.sort_by::<ponko::d2::Transform>(|t| ponko::spatial::spatial_key(t.position, 32.0));
/// # }
/// ```
#[must_use]
pub fn spatial_key(position: Vec2, cell_size: f32) -> u64 {
    let (cx, cy) = cell_of(position, 1.0 / cell_size);
    // Shift to unsigned so negative cells order correctly, then interleave bits.
    interleave((cx as u32) ^ 0x8000_0000) | interleave((cy as u32) ^ 0x8000_0000) << 1
}

/// Spreads the 32 bits of `v` to the even bits of a u64.
#[inline(always)]
fn interleave(v: u32) -> u64 {
    let mut x = v as u64;
    x = (x | x << 16) & 0x0000_FFFF_0000_FFFF;
    x = (x | x << 8) & 0x00FF_00FF_00FF_00FF;
    x = (x | x << 4) & 0x0F0F_0F0F_0F0F_0F0F;
    x = (x | x << 2) & 0x3333_3333_3333_3333;
    x = (x | x << 1) & 0x5555_5555_5555_5555;
    x
}

/// Grids at least this big build their per-entry arrays on all cores;
/// below it, waking the thread pool costs more than it saves.
const PARALLEL_REBUILD_MIN: usize = 200_000;

/// `out = input.map(f)`, reusing `out`'s allocation; on all cores if `parallel`.
fn map_into<A: Sync, B: Send>(input: &[A], out: &mut Vec<B>, parallel: bool, f: impl Fn(&A) -> B + Sync + Send) {
    out.clear();
    if parallel {
        use rayon::prelude::*;
        input.par_iter().map(f).collect_into_vec(out);
    } else {
        out.extend(input.iter().map(f));
    }
}

#[inline(always)]
fn pack(cx: i32, cy: i32) -> u64 {
    (cx as u32 as u64) << 32 | cy as u32 as u64
}

/// How cells map to buckets. Neighboring cells get neighboring buckets, so
/// building and querying the grid walks memory nearly in order: within a
/// tile of `2^x_bits × 2^y_bits` cells (one cell per bucket), buckets are
/// row-major. Each tile's position is hashed and XORed in, so far-apart
/// tiles (a long line of entities, say) spread over all buckets instead of
/// piling into a few. XOR with a constant keeps aligned blocks of buckets
/// together, so locality survives it.
/// See [`SpatialGrid::raw_view`]. Entry `i` lives in cell `cells[i]`
/// (`(cx as u32) << 32 | cy as u32`); bucket `b` holds entries
/// `starts[b]..starts[b + 1]`.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct RawGridView<'a, T> {
    pub entries: &'a [GridEntry<T>],
    pub cells: &'a [u64],
    pub starts: &'a [u32],
    pub inv_cell: f32,
    pub bucket_mask: u64,
    pub x_bits: u32,
    pub y_bits: u32,
}

#[derive(Debug, Clone, Copy)]
struct Buckets {
    /// Bucket count minus one (bucket count is a power of two).
    mask: usize,
    x_bits: u32,
    y_bits: u32,
}

impl Buckets {
    fn new(count: usize) -> Self {
        debug_assert!(count.is_power_of_two());
        let bits = count.trailing_zeros();
        Self {
            mask: count - 1,
            x_bits: bits.div_ceil(2),
            y_bits: bits / 2,
        }
    }

    #[inline(always)]
    fn of(self, cx: i32, cy: i32) -> usize {
        let (ux, uy) = (cx as u32, cy as u32);
        let local = ((uy as u64 & ((1 << self.y_bits) - 1)) << self.x_bits) | (ux as u64 & ((1 << self.x_bits) - 1));
        // Hash the tile; the high half of each product depends on every
        // bit of the coordinate, so rows, columns and diagonals of tiles
        // all spread out.
        let (tx, ty) = (u64::from(ux >> self.x_bits), u64::from(uy >> self.y_bits));
        let h = tx.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ty.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        (local ^ (h >> 32)) as usize & self.mask
    }
}

pub(crate) const SPATIAL_SORT_EVERY: u64 = 32;

/// The system behind [`systems::rebuild_spatial_grid`](crate::systems::rebuild_spatial_grid); it keeps
/// this path for its name (`ponko::spatial::rebuild_spatial_grid`).
pub(crate) fn rebuild_spatial_grid(ctx: &mut Ctx) {
    if ctx.time.sim_tick().is_multiple_of(SPATIAL_SORT_EVERY) {
        let cell = ctx.spatial.cell_size();
        ctx.world.sort_by::<Transform2D>(|t| spatial_key(t.position, cell));
    }
    ctx.spatial.rebuild(&mut ctx.world);
}

impl<T> SpatialGrid<T> {
    /// How often (in ticks) the built-in grid also reorders entities in memory
    /// by [`spatial_key`]. Positions drift slowly, so occasional sorting keeps
    /// neighbors close in memory at a small amortized cost (1M entities:
    /// ~18 ms per sort, ~0.5 ms per tick).
    pub const SORT_EVERY: u64 = SPATIAL_SORT_EVERY;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Largest bucket when `cells` are mapped into `count` buckets.
    fn worst_bucket(count: usize, cells: impl Iterator<Item = (i32, i32)>) -> usize {
        let b = Buckets::new(count);
        let mut hist = vec![0usize; count];
        for (cx, cy) in cells {
            hist[b.of(cx, cy)] += 1;
        }
        hist.into_iter().max().unwrap()
    }

    #[test]
    fn a_block_of_cells_that_fits_a_tile_has_no_collisions() {
        // 4096 buckets = a 64x64 tile; a 64x64 block anywhere, even across
        // tile borders and negative coordinates, maps one cell per bucket.
        for (x0, y0) in [(0, 0), (-32, -32), (1000, -7000)] {
            let block = (0..64).flat_map(|y| (0..64).map(move |x| (x0 + x, y0 + y)));
            assert!(worst_bucket(4096, block) <= 4, "block at ({x0}, {y0})");
        }
        let aligned = (0..64).flat_map(|y| (0..64).map(move |x| (x, y)));
        assert_eq!(worst_bucket(4096, aligned), 1);
    }

    #[test]
    fn long_lines_spread_over_all_buckets() {
        // A row or column of cells much longer than a tile: without the
        // tile hash, every tile would reuse the same few buckets.
        let n = 1 << 16;
        let count = 1 << 12;
        let ideal = n / count;
        assert!(worst_bucket(count, (0..n as i32).map(|x| (x, 0))) <= ideal * 4);
        assert!(worst_bucket(count, (0..n as i32).map(|y| (5, y))) <= ideal * 4);
        assert!(worst_bucket(count, (0..n as i32).map(|i| (i, i))) <= ideal * 4);
    }

    #[test]
    fn neighboring_cells_get_nearby_buckets() {
        let b = Buckets::new(1 << 16); // 256x256 tile
        assert_eq!(b.of(11, 20).abs_diff(b.of(10, 20)), 1);
        assert_eq!(b.of(10, 21).abs_diff(b.of(10, 20)), 256);
    }
}
