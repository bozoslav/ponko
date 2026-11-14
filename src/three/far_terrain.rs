//! [`FarTerrain`]: cheap low-detail ground for everything beyond the
//! streamed terrain tiles.

use std::collections::HashSet;

use glam::{Vec2, Vec3};

use super::{Material, Mesh, MeshData, Model, ShadowMesh};
use crate::ctx::Ctx;
use crate::d3::Transform as Transform3D;
use ponko_ecs::Entity;

/// Grid squares along each side of one of a far terrain's meshes.
const BLOCK: usize = 64;

/// Low-detail ground for the whole world, drawn beyond the terrain tiles
/// that are loaded: a few large meshes made from a coarse grid of heights
/// the game provides (the world's heights, sampled every `spacing` world
/// units), with the terrain material. Where a cell has its own tile, the
/// far ground is cut away ([`FarTerrain::cut`]), so the two never overlap
/// or fight over depth. A short skirt hangs down from every cut edge, and
/// the tiles' own skirts hang down beside it, so there's no crack between
/// the fine and coarse ground.
///
/// The grid's squares must line up with the cells: `cell_size` is a whole
/// number of `spacing`s, and `corner` lies on the grid of `spacing`s (so a
/// cell's edges fall on grid lines). Height `i` of the grid is at
/// `corner + (i % columns, i / columns) × spacing`, row after row along +Z;
/// NaN leaves a hole.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     // 8 km of world sampled every 64 m, from (-4096, -4096); tiles 256 m wide.
///     let n = 129;
///     let heights: Vec<f32> = (0..n * n).map(|i| ((i % n) as f32 * 0.3).sin() * 40.0).collect();
///     let ground = ctx.gfx.default_material();
///     let mut far = ponko::terrain::FarTerrain::new(&heights, n, vec2(-4096.0, -4096.0), 64.0, 256.0, ground);
///     far.cut((0, 0)); // the tile of cell (0, 0) is loaded
///     far.update(ctx);
///     assert_eq!(far.mesh_count(), 4);
/// });
/// ```
#[derive(Debug, Clone)]
pub struct FarTerrain {
    heights: Vec<f32>,
    columns: usize,
    rows: usize,
    corner: Vec2,
    spacing: f32,
    cell_size: f32,
    material: Material,
    cut: HashSet<(i32, i32)>,
    /// Row after row of meshes along +Z.
    blocks: Vec<Block>,
    blocks_x: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct Block {
    /// Its entity and mesh, when it has any ground.
    shown: Option<(Entity, Mesh)>,
    /// Needs building again at the next update.
    dirty: bool,
}

impl FarTerrain {
    /// Far ground from `heights`, `columns` per row, the first at `corner`
    /// (x, z) and the rest `spacing` apart, drawn with `material` (the
    /// terrain material, or any other); cut away a cell at a time for
    /// tiles `cell_size` wide (the [`CellStreamer`](crate::streaming::CellStreamer)'s
    /// cells). Nothing is made until [`FarTerrain::update`].
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ground: ponko::render::Material) {
    /// let far = ponko::terrain::FarTerrain::new(&[0.0; 4 * 4], 4, Vec2::ZERO, 100.0, 100.0, ground);
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If there aren't at least 2 × 2 heights in whole rows of `columns`,
    /// `spacing` or `cell_size` isn't above 0, `cell_size` isn't a whole
    /// number of `spacing`s, or `corner` isn't on the grid of `spacing`s.
    #[must_use]
    pub fn new(
        heights: &[f32],
        columns: usize,
        corner: Vec2,
        spacing: f32,
        cell_size: f32,
        material: Material,
    ) -> Self {
        Self::try_new(heights, columns, corner, spacing, cell_size, material)
            .unwrap_or_else(|e| panic!("ponko: FarTerrain::new: {e}"))
    }

    /// [`FarTerrain::new`] that returns errors (for the C API).
    #[doc(hidden)]
    pub fn try_new(
        heights: &[f32],
        columns: usize,
        corner: Vec2,
        spacing: f32,
        cell_size: f32,
        material: Material,
    ) -> Result<Self, String> {
        if columns < 2 || heights.len() < 2 * columns || !heights.len().is_multiple_of(columns) {
            return Err(format!(
                "{} heights aren't whole rows of {columns} (at least 2 x 2)",
                heights.len()
            ));
        }
        let positive = |v: f32| v > 0.0 && v.is_finite();
        if !positive(spacing) || !positive(cell_size) {
            return Err(format!(
                "spacing and cell size must be above 0 (got {spacing} and {cell_size})"
            ));
        }
        let whole = |v: f32| (v - v.round()).abs() < 1e-3;
        if !whole(cell_size / spacing) || cell_size < spacing * 0.999 {
            return Err(format!(
                "the cell size ({cell_size}) must be a whole number of spacings ({spacing})"
            ));
        }
        if !corner.is_finite() || !whole(corner.x / spacing) || !whole(corner.y / spacing) {
            return Err(format!(
                "the corner ({corner}) must lie on the grid of spacings ({spacing})"
            ));
        }
        let rows = heights.len() / columns;
        let blocks_x = (columns - 1).div_ceil(BLOCK);
        let blocks_z = (rows - 1).div_ceil(BLOCK);
        Ok(Self {
            heights: heights.to_vec(),
            columns,
            rows,
            corner,
            spacing,
            cell_size,
            material,
            cut: HashSet::new(),
            blocks: vec![
                Block {
                    shown: None,
                    dirty: true
                };
                blocks_x * blocks_z
            ],
            blocks_x,
        })
    }

    /// Cuts the far ground away over `cell`, whose own tile is now shown.
    /// Call it once the tile is in the world (not when it starts loading),
    /// so there's never a gap. Takes effect at the next update.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ground: ponko::render::Material) {
    /// let mut far = ponko::terrain::FarTerrain::new(&[0.0; 4 * 4], 4, Vec2::ZERO, 100.0, 100.0, ground);
    /// far.cut((1, 2));
    /// assert!(far.is_cut((1, 2)));
    /// # }
    /// ```
    pub fn cut(&mut self, cell: (i32, i32)) {
        if self.cut.insert(cell) {
            self.touch(cell);
        }
    }

    /// Fills `cell` with far ground again, when its tile goes. Takes
    /// effect at the next update.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ground: ponko::render::Material) {
    /// let mut far = ponko::terrain::FarTerrain::new(&[0.0; 4 * 4], 4, Vec2::ZERO, 100.0, 100.0, ground);
    /// far.cut((1, 2));
    /// far.uncut((1, 2));
    /// assert!(!far.is_cut((1, 2)));
    /// # }
    /// ```
    pub fn uncut(&mut self, cell: (i32, i32)) {
        if self.cut.remove(&cell) {
            self.touch(cell);
        }
    }

    /// Whether `cell` is cut away.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ground: ponko::render::Material) {
    /// let far = ponko::terrain::FarTerrain::new(&[0.0; 4 * 4], 4, Vec2::ZERO, 100.0, 100.0, ground);
    /// assert!(!far.is_cut((0, 0)));
    /// # }
    /// ```
    #[must_use]
    pub fn is_cut(&self, cell: (i32, i32)) -> bool {
        self.cut.contains(&cell)
    }

    /// Builds the meshes that changed since the last update (all of them
    /// the first time) and puts them in the world: one entity per mesh,
    /// with a [`Model`] and no shadow ([`ShadowMesh`]`(None)`), since far
    /// ground is beyond the shadows anyway. Cutting a cell rebuilds only
    /// the mesh or two around it.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let ground = ctx.gfx.default_material();
    ///     let mut far = ponko::terrain::FarTerrain::new(&[0.0; 4 * 4], 4, Vec2::ZERO, 100.0, 100.0, ground);
    ///     far.update(ctx);
    ///     assert_eq!(far.mesh_count(), 1);
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// If the material comes from another game.
    pub fn update(&mut self, ctx: &mut Ctx) {
        for b in 0..self.blocks.len() {
            if !self.blocks[b].dirty {
                continue;
            }
            self.blocks[b].dirty = false;
            let data = self.block_data(b);
            let old = self.blocks[b].shown.take();
            if let Some((_, mesh)) = old {
                ctx.gfx.free(mesh);
            }
            if data.indices.is_empty() {
                if let Some((entity, _)) = old {
                    ctx.world.despawn(entity);
                }
                continue;
            }
            let mesh = ctx.gfx.create_mesh(&data, self.material);
            let entity = match old {
                Some((entity, _)) if ctx.world.get::<Model>(entity).is_some() => {
                    if let Some(model) = ctx.world.get_mut::<Model>(entity) {
                        model.mesh = mesh;
                    }
                    entity
                }
                _ => ctx.world.spawn((
                    Transform3D::at(self.block_corner(b)),
                    Model::new(mesh),
                    ShadowMesh(None),
                )),
            };
            self.blocks[b].shown = Some((entity, mesh));
        }
    }

    /// Despawns the far ground's entities and frees its meshes. The next
    /// [`FarTerrain::update`] builds them again.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let ground = ctx.gfx.default_material();
    ///     let mut far = ponko::terrain::FarTerrain::new(&[0.0; 4 * 4], 4, Vec2::ZERO, 100.0, 100.0, ground);
    ///     far.update(ctx);
    ///     far.clear(ctx);
    ///     assert_eq!(far.mesh_count(), 0);
    /// });
    /// ```
    pub fn clear(&mut self, ctx: &mut Ctx) {
        for block in &mut self.blocks {
            if let Some((entity, mesh)) = block.shown.take() {
                ctx.world.despawn(entity);
                ctx.gfx.free(mesh);
            }
            block.dirty = true;
        }
    }

    /// How many meshes (and entities) the far ground has in the world now.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ground: ponko::render::Material) {
    /// let far = ponko::terrain::FarTerrain::new(&[0.0; 4 * 4], 4, Vec2::ZERO, 100.0, 100.0, ground);
    /// assert_eq!(far.mesh_count(), 0, "nothing before the first update");
    /// # }
    /// ```
    #[must_use]
    pub fn mesh_count(&self) -> usize {
        self.blocks.iter().filter(|b| b.shown.is_some()).count()
    }

    /// The entities and meshes in the world, in the order they were made
    /// (for render tests).
    #[cfg(all(test, feature = "window"))]
    pub(crate) fn shown(&self) -> impl Iterator<Item = (usize, Entity, Mesh)> + '_ {
        self.blocks
            .iter()
            .enumerate()
            .filter_map(|(b, block)| block.shown.map(|(e, m)| (b, e, m)))
    }

    /// Marks the meshes over `cell` for building again, and those of the
    /// squares beside it (their skirts change).
    fn touch(&mut self, (cx, cz): (i32, i32)) {
        let per = (self.cell_size / self.spacing).round() as i64;
        // The meshes with the cell's squares or the ones beside them.
        let blocks = |c: i32, corner: f32, squares: usize| {
            let first = i64::from(c) * per - (corner / self.spacing).round() as i64;
            let (lo, hi) = ((first - 1).max(0), (first + per).min(squares as i64 - 1));
            (lo <= hi).then_some((lo as usize / BLOCK, hi as usize / BLOCK))
        };
        let (Some((bx0, bx1)), Some((bz0, bz1))) = (
            blocks(cx, self.corner.x, self.columns - 1),
            blocks(cz, self.corner.y, self.rows - 1),
        ) else {
            return;
        };
        for bz in bz0..=bz1 {
            for bx in bx0..=bx1 {
                self.blocks[bz * self.blocks_x + bx].dirty = true;
            }
        }
    }

    /// Which cell grid square `(i, j)` lies in.
    fn cell_of(&self, i: usize, j: usize) -> (i32, i32) {
        let center = self.corner + (Vec2::new(i as f32, j as f32) + 0.5) * self.spacing;
        let cell = (center / self.cell_size).floor();
        (cell.x as i32, cell.y as i32)
    }

    fn height(&self, i: usize, j: usize) -> f32 {
        self.heights[j * self.columns + i]
    }

    /// Where mesh `b` starts, in the world.
    fn block_corner(&self, b: usize) -> Vec3 {
        let (bx, bz) = (b % self.blocks_x, b / self.blocks_x);
        let at = self.corner + Vec2::new(bx as f32, bz as f32) * (BLOCK as f32 * self.spacing);
        Vec3::new(at.x, 0.0, at.y)
    }

    /// Mesh `b`'s ground, from its corner: every solid grid square, and a
    /// skirt down from each edge beside a cut one.
    pub(crate) fn block_data(&self, b: usize) -> MeshData {
        let (bx, bz) = (b % self.blocks_x, b / self.blocks_x);
        let (i0, j0) = (bx * BLOCK, bz * BLOCK);
        let (i1, j1) = ((i0 + BLOCK).min(self.columns - 1), (j0 + BLOCK).min(self.rows - 1));
        let side = i1 - i0 + 1;
        let (n, s) = (self.columns, self.spacing);
        let mut mesh = MeshData::default();
        for j in j0..=j1 {
            for i in i0..=i1 {
                let h = self.height(i, j);
                let here = if h.is_finite() { h } else { 0.0 };
                let solid = |v: f32| if v.is_finite() { v } else { here };
                // Slopes from the neighbours on each side (one side at an edge).
                let (x0, x1) = (i.saturating_sub(1), (i + 1).min(n - 1));
                let (z0, z1) = (j.saturating_sub(1), (j + 1).min(self.rows - 1));
                let dx = (solid(self.height(x1, j)) - solid(self.height(x0, j))) / ((x1 - x0) as f32 * s);
                let dz = (solid(self.height(i, z1)) - solid(self.height(i, z0))) / ((z1 - z0) as f32 * s);
                mesh.positions
                    .push(Vec3::new((i - i0) as f32 * s, here, (j - j0) as f32 * s));
                mesh.normals.push(Vec3::new(-dx, 1.0, -dz).normalize());
                mesh.uvs
                    .push(Vec2::new(i as f32 / (n - 1) as f32, j as f32 / (self.rows - 1) as f32));
            }
        }
        let index = |i: usize, j: usize| ((j - j0) * side + (i - i0)) as u32;
        let depth = s * 2.0;
        // Which of the cells under these squares and the ones around them
        // are cut, looked up once each.
        let (lo_i, lo_j) = (i0.saturating_sub(1), j0.saturating_sub(1));
        let (lo, hi) = (
            self.cell_of(lo_i, lo_j),
            self.cell_of(i1.min(n - 2), j1.min(self.rows - 2)),
        );
        let width = (hi.0 - lo.0 + 1) as usize;
        let cells: Vec<bool> = (lo.1..=hi.1)
            .flat_map(|z| (lo.0..=hi.0).map(move |x| (x, z)))
            .map(|cell| self.cut.contains(&cell))
            .collect();
        let cut = |x: usize, z: usize| {
            let (cx, cz) = self.cell_of(x, z);
            cells[(cz - lo.1) as usize * width + (cx - lo.0) as usize]
        };
        let open = |i: usize, j: usize| {
            [(i, j), (i + 1, j), (i, j + 1), (i + 1, j + 1)]
                .iter()
                .all(|&(x, z)| self.height(x, z).is_finite())
        };
        for j in j0..j1 {
            for i in i0..i1 {
                if !open(i, j) || cut(i, j) {
                    continue;
                }
                let [a, b, c, d] = [index(i, j), index(i + 1, j), index(i, j + 1), index(i + 1, j + 1)];
                mesh.indices.extend([a, c, b, b, c, d]);
                // Skirts toward cut neighbours: (edge corners, way out).
                let mut edges: [Option<([u32; 2], Vec3)>; 4] = [None; 4];
                if i > 0 && cut(i - 1, j) {
                    edges[0] = Some(([a, c], Vec3::NEG_X));
                }
                if i + 1 < n - 1 && cut(i + 1, j) {
                    edges[1] = Some(([b, d], Vec3::X));
                }
                if j > 0 && cut(i, j - 1) {
                    edges[2] = Some(([a, b], Vec3::NEG_Z));
                }
                if j + 1 < self.rows - 1 && cut(i, j + 1) {
                    edges[3] = Some(([c, d], Vec3::Z));
                }
                for ([p, q], out) in edges.into_iter().flatten() {
                    skirt(&mut mesh, p, q, out, depth);
                }
            }
        }
        mesh
    }
}

/// A wall hanging `depth` down from the edge `p`–`q`, facing `out`.
fn skirt(mesh: &mut MeshData, p: u32, q: u32, out: Vec3, depth: f32) {
    let (top_p, top_q) = (mesh.positions[p as usize], mesh.positions[q as usize]);
    // Wound so the wall faces `out` (front faces turn counterclockwise).
    let facing = (top_q - top_p).cross(Vec3::NEG_Y).dot(out);
    let (p, q) = if facing > 0.0 { (p, q) } else { (q, p) };
    let low = mesh.positions.len() as u32;
    for top in [p, q] {
        let t = top as usize;
        mesh.positions.push(mesh.positions[t] - Vec3::Y * depth);
        mesh.normals.push(mesh.normals[t]);
        mesh.uvs.push(mesh.uvs[t]);
    }
    let (p_low, q_low) = (low, low + 1);
    mesh.indices.extend([p, q, p_low, q, q_low, p_low]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn far(heights: &[f32], columns: usize) -> FarTerrain {
        FarTerrain::new(heights, columns, Vec2::ZERO, 1.0, 2.0, Material { id: 0 })
    }

    #[test]
    fn cut_cells_have_no_ground_and_skirts_face_into_them() {
        // 6 x 6 squares, cells of 2 x 2 squares: cut the middle cell.
        let mut f = far(&[1.0; 7 * 7], 7);
        let whole = f.block_data(0);
        assert_eq!(whole.indices.len(), 36 * 6);
        f.cut((1, 1));
        let data = f.block_data(0);
        data.check().unwrap();
        // 32 squares left, and 8 skirt walls around the hole (2 per side).
        assert_eq!(data.indices.len(), 32 * 6 + 8 * 6);
        let hole = Vec3::new(3.0, 1.0, 3.0);
        let mut walls = 0;
        for t in data.indices.chunks(3) {
            let [p, q, r] = [0, 1, 2].map(|k| data.positions[t[k] as usize]);
            let normal = (q - p).cross(r - p);
            // Nothing lies over the hole.
            let center = (p + q + r) / 3.0;
            let inside = (2.0..=4.0).contains(&center.x) && (2.0..=4.0).contains(&center.z);
            assert!(
                !(inside && normal.y.abs() > 1e-6),
                "ground over the cut cell at {center}"
            );
            if normal.y.abs() < 1e-6 {
                walls += 1;
                // Each wall faces the hole's middle.
                let toward = Vec3::new(hole.x - center.x, 0.0, hole.z - center.z);
                assert!(normal.dot(toward) > 0.0, "a wall at {center} faces away");
                assert!(p.y.min(q.y).min(r.y) < 0.0, "walls hang down");
            }
        }
        assert_eq!(walls, 16);
        f.uncut((1, 1));
        assert_eq!(f.block_data(0).indices.len(), whole.indices.len());
    }

    #[test]
    fn holes_and_the_grid_edge_get_no_skirts() {
        let mut heights = vec![0.0; 5 * 5];
        heights[12] = f32::NAN;
        let data = far(&heights, 5).block_data(0);
        assert_eq!(data.indices.len(), 12 * 6, "the four squares around the hole are open");
        assert!(data.positions.iter().all(|p| p.y == 0.0), "no skirts");
    }

    #[test]
    fn cutting_rebuilds_only_the_meshes_around_the_cell() {
        // 3 x 2 meshes of 64 squares; cells of 16.
        let columns = BLOCK * 3 + 1;
        let mut f = FarTerrain::new(
            &vec![0.0; columns * (BLOCK * 2 + 1)],
            columns,
            Vec2::ZERO,
            1.0,
            16.0,
            Material { id: 0 },
        );
        let dirty = |f: &mut FarTerrain, cell| {
            for b in &mut f.blocks {
                b.dirty = false;
            }
            f.cut(cell);
            (0..6).filter(|&b| f.blocks[b].dirty).collect::<Vec<usize>>()
        };
        // Squares 80 to 95 along X, with skirts in 79 and 96: the middle mesh.
        assert_eq!(dirty(&mut f, (5, 0)), [1], "only the middle mesh");
        // Squares 64 to 79 both ways: the skirts left and below are in other meshes.
        assert_eq!(dirty(&mut f, (4, 4)), [0, 1, 3, 4]);
        assert!(dirty(&mut f, (40, 0)).is_empty(), "off the grid");
    }

    #[test]
    fn the_grid_must_line_up_with_the_cells() {
        let m = Material { id: 0 };
        assert!(FarTerrain::try_new(&[0.0; 9], 3, Vec2::ZERO, 1.0, 2.5, m).is_err());
        assert!(FarTerrain::try_new(&[0.0; 9], 3, Vec2::new(0.5, 0.0), 1.0, 2.0, m).is_err());
        assert!(FarTerrain::try_new(&[0.0; 8], 3, Vec2::ZERO, 1.0, 2.0, m).is_err());
        assert!(FarTerrain::try_new(&[0.0; 9], 3, Vec2::new(-3.0, 7.0), 1.0, 2.0, m).is_ok());
    }
}
