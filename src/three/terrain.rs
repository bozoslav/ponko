//! [`TerrainTile`]: a square of heightfield terrain, drawn more coarsely
//! further from the camera.

use glam::{Vec2, Vec3, Vec4};

use super::{Material, MaterialDesc, Mesh, MeshData, TextureArray};
use crate::gfx::Graphics;
use ponko_ecs::Component;

/// The most ground layers a terrain material blends.
pub const TERRAIN_LAYERS: usize = 8;

/// What a terrain material is made from: ground layers (grass, dirt, rock,
/// ...) and a weight map saying how much of each shows where. Made into a
/// [`Material`] with [`Graphics::create_terrain_material`](crate::gfx::Graphics::create_terrain_material) and used for
/// [`Graphics::create_terrain_tile`](crate::gfx::Graphics::create_terrain_tile).
///
/// The layers' textures are lined up with the world, not with each tile,
/// so they run on across tile edges; each repeats every
/// [`layer_size`](Self::layer_size) world units.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// let ground = ctx.gfx.load_texture_array(&["grass.png", "dirt.png", "rock.png"], ponko::terrain::TextureKind::Color);
/// // One RGBA weight map for the whole 512 m: red grass, green dirt, blue rock.
/// let weights = ctx.gfx.load_texture_array(&["weights.png"], ponko::terrain::TextureKind::Data);
/// let desc = ponko::terrain::TerrainMaterialDesc::new(ground, weights, vec2(0.0, 0.0), vec2(512.0, 512.0));
/// let terrain = ctx.gfx.create_terrain_material(&desc);
/// # }
/// ```
///
/// New fields may be added in any release: build one with
/// [`TerrainMaterialDesc::new`] and the `with_*` methods.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TerrainMaterialDesc {
    /// Each layer's color texture ([`TextureKind::Color`](crate::terrain::TextureKind::Color)),
    /// up to [`TerrainTile::LAYERS`](crate::terrain::TerrainTile::LAYERS).
    pub layers: TextureArray,
    /// The layers' normal maps, one each in the same order
    /// ([`TextureKind::Data`](crate::terrain::TextureKind::Data), OpenGL style).
    /// Default none.
    pub normals: Option<TextureArray>,
    /// How strong the normal maps are. Default 1.
    pub normal_strength: f32,
    /// How much of each layer shows where ([`TextureKind::Data`](crate::terrain::TextureKind::Data)):
    /// its first image's red, green, blue and alpha are layers 0 to 3, its
    /// second image's are layers 4 to 7. Weights are evened out to add up
    /// to 1; where they are all 0, the first layer shows.
    pub weights: TextureArray,
    /// The world corner (x, z) where the weight map's top-left pixel
    /// lies: its rows run along +Z.
    pub weights_corner: Vec2,
    /// How much ground (x, z) the weight map covers, in world units.
    pub weights_size: Vec2,
    /// How many world units each layer's texture covers before it repeats.
    /// Default 4.
    pub layer_size: [f32; TERRAIN_LAYERS],
    /// 0 is mirror-smooth, 1 fully rough. Default 0.9.
    pub roughness: f32,
}

impl TerrainMaterialDesc {
    /// Ground `layers`, blended by a `weights` map lying over `size` world
    /// units from `corner`; everything else as by default.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ground: ponko::terrain::TextureArray, weights: ponko::terrain::TextureArray) {
    /// let desc = ponko::terrain::TerrainMaterialDesc::new(ground, weights, Vec2::ZERO, vec2(256.0, 256.0)).with_roughness(0.8);
    /// # }
    /// ```
    #[must_use]
    pub fn new(layers: TextureArray, weights: TextureArray, corner: Vec2, size: Vec2) -> Self {
        Self {
            layers,
            normals: None,
            normal_strength: 1.0,
            weights,
            weights_corner: corner,
            weights_size: size,
            layer_size: [4.0; TERRAIN_LAYERS],
            roughness: 0.9,
        }
    }

    with_fields! {
        /// Sets [`normals`](Self::normals), the layers' normal maps.
        normals: Option<TextureArray> => with_normals;
        /// Sets [`normal_strength`](Self::normal_strength).
        normal_strength: f32 => with_normal_strength;
        /// Sets [`layer_size`](Self::layer_size), the world units each layer's texture covers.
        layer_size: [f32; TERRAIN_LAYERS] => with_layer_size;
        /// Sets [`roughness`](Self::roughness).
        roughness: f32 => with_roughness;
    }

    /// Says what's wrong, if anything.
    pub(crate) fn check(&self) -> Result<(), String> {
        let count = self.layers.layers() as usize;
        if count > TERRAIN_LAYERS {
            return Err(format!(
                "a terrain material blends at most {TERRAIN_LAYERS} layers (got {count})"
            ));
        }
        let needed = count.div_ceil(4) as u32;
        if self.weights.layers() < needed {
            return Err(format!(
                "{count} terrain layers need {needed} weight images (4 layers each), got {}",
                self.weights.layers()
            ));
        }
        if let Some(normals) = self.normals
            && normals.layers() as usize != count
        {
            return Err(format!(
                "{count} terrain layers need {count} normal maps, got {}",
                normals.layers()
            ));
        }
        let positive = |v: f32| v > 0.0 && v.is_finite();
        if !positive(self.weights_size.x) || !positive(self.weights_size.y) {
            return Err(format!(
                "the weight map's size must be above 0 (got {})",
                self.weights_size
            ));
        }
        if let Some(bad) = self.layer_size[..count].iter().find(|s| !positive(**s)) {
            return Err(format!("each terrain layer's size must be above 0 (got {bad})"));
        }
        if !(0.0..=1.0).contains(&self.roughness) {
            return Err(format!("roughness goes from 0 to 1 (got {})", self.roughness));
        }
        Ok(())
    }
}

/// How many detail levels a [`TerrainTile`] has: every height, then every
/// 2nd, 4th and 8th.
pub const TERRAIN_LODS: usize = 4;

/// A square of terrain from a grid of heights, made with
/// [`Graphics::create_terrain_tile`](crate::gfx::Graphics::create_terrain_tile). Spawn it with a [`d3::Transform`](crate::d3::Transform)
/// at the tile's corner, instead of a [`Model`](crate::render::Model). The renderer
/// picks a detail level by how far the camera is: the full grid nearer than
/// `lod_distance`, then every 2nd height to twice that, every 4th to four
/// times that, and every 8th beyond. Skirts hide the cracks where tiles of
/// different detail meet.
///
/// A [`Handle`](crate::handle::Handle): check it with `ctx.is_live(tile)` (live
/// while all its meshes are) and free all its meshes with `ctx.free(tile)`.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// let grass = ctx.gfx.create_material(ponko::render::MaterialDesc::color(Color::hex(0x5A8A3C)).with_roughness(0.95));
/// // 65 x 65 heights, 2 m apart: a 128 m tile with gentle hills.
/// let n = 65;
/// let heights: Vec<f32> = (0..n * n)
///     .map(|i| ((i % n) as f32 * 0.1).sin() * 3.0 + ((i / n) as f32 * 0.07).cos() * 4.0)
///     .collect();
/// let tile = ctx.gfx.create_terrain_tile(&heights, 128.0, grass);
/// ctx.world.spawn((ponko::d3::Transform::at(vec3(-64.0, 0.0, -64.0)), tile));
/// # }
/// ```
#[derive(Debug, Clone, Copy, Component)]
#[repr(C)] // shared with C/C++ (`PonkoTerrainTile`): the meshes, then the distance
pub struct TerrainTile {
    /// The meshes, finest first.
    pub(crate) lods: [Mesh; TERRAIN_LODS],
    /// Up to how far from the camera the finest mesh is drawn, in world
    /// units. Default: the tile's size.
    pub(crate) lod_distance: f32,
}

// The C library reads a tile as `PonkoTerrainTile` and can only check its
// size and alignment (the fields are private), which a reorder would keep:
// pin the offsets here, and the same numbers on the C struct there.
const _: () =
    assert!(core::mem::offset_of!(TerrainTile, lods) == 0 && core::mem::offset_of!(TerrainTile, lod_distance) == 16);

// A handle: equal (and hashed) by the meshes it frees, whatever its
// distance, so `tile.with_lod_distance(d) == tile`.
impl PartialEq for TerrainTile {
    fn eq(&self, other: &Self) -> bool {
        self.lods == other.lods
    }
}
impl Eq for TerrainTile {}
impl std::hash::Hash for TerrainTile {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.lods.hash(state);
    }
}

impl TerrainTile {
    /// Which of [`lods`](Self::lods) is drawn `distance` away from the
    /// camera (measured to the nearest point of the tile's bounds).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(tile: ponko::terrain::TerrainTile) {
    /// assert_eq!(tile.lod(0.0), 0);
    /// assert_eq!(tile.lod(tile.lod_distance() * 10.0), 3);
    /// # }
    /// ```
    #[must_use]
    pub fn lod(&self, distance: f32) -> usize {
        lod_at(distance, self.lod_distance)
    }

    /// Its meshes, one per detail level, finest first: to set their
    /// shadows or look up what they cost. Free the tile, not them.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let ground = ctx.gfx.default_material();
    ///     let tile = ctx.gfx.create_terrain_tile(&[0.0; 9 * 9], 16.0, ground);
    ///     assert!(tile.lods().iter().all(|&m| ctx.is_live(m)));
    /// });
    /// ```
    #[must_use]
    pub fn lods(&self) -> [Mesh; TERRAIN_LODS] {
        self.lods
    }

    /// Up to how far from the camera the finest mesh is drawn, in world
    /// units. The tile's size unless changed with
    /// [`with_lod_distance`](Self::with_lod_distance).
    #[must_use]
    pub fn lod_distance(&self) -> f32 {
        self.lod_distance
    }

    /// The same tile, drawn at full detail up to `distance` (and each
    /// coarser level to twice the last). It is still equal to `self`: the
    /// same meshes, so freeing either frees both.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(tile: ponko::terrain::TerrainTile) {
    /// let sharper = tile.with_lod_distance(2.0 * tile.lod_distance());
    /// assert_eq!(sharper.lod(tile.lod_distance()), 0);
    /// assert_eq!(sharper, tile);
    /// # }
    /// ```
    #[must_use]
    pub fn with_lod_distance(self, distance: f32) -> Self {
        Self {
            lod_distance: distance,
            ..self
        }
    }
}

/// 0 nearer than `lod_distance`, then one more for every doubling.
pub(crate) fn lod_at(distance: f32, lod_distance: f32) -> usize {
    if lod_distance <= 0.0 || distance.is_nan() || distance < lod_distance {
        return 0;
    }
    let doublings = (distance / lod_distance).log2().floor() as usize;
    (doublings + 1).min(TERRAIN_LODS - 1)
}

impl Graphics {
    /// Makes a material for terrain from ground layers and a weight map
    /// (see [`TerrainMaterialDesc`]). It's an ordinary [`Material`]: give
    /// it to [`Graphics::create_terrain_tile`](crate::gfx::Graphics::create_terrain_tile), or to any mesh.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, desc: ponko::terrain::TerrainMaterialDesc, heights: &[f32]) {
    /// let ground = ctx.gfx.create_terrain_material(&desc);
    /// let tile = ctx.gfx.create_terrain_tile(heights, 256.0, ground);
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If there are more than [`TerrainTile::LAYERS`](crate::terrain::TerrainTile::LAYERS) layers, too few weight
    /// images or normal maps, a size isn't above 0, or an array comes from
    /// another game.
    pub fn create_terrain_material(&mut self, desc: &TerrainMaterialDesc) -> Material {
        self.try_create_terrain_material(desc)
            .unwrap_or_else(|e| panic!("{}", e.context("create_terrain_material")))
    }

    /// [`create_terrain_material`](Self::create_terrain_material) with errors
    /// as values: [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) for a
    /// description that doesn't fit together (too many layers, too few weight
    /// images or normal maps, a size not above 0),
    /// [`BadHandle`](crate::error::ErrorKind::BadHandle) for an array this game
    /// didn't make.
    pub fn try_create_terrain_material(&mut self, desc: &TerrainMaterialDesc) -> Result<Material, crate::error::Error> {
        desc.check()
            .map_err(|e| crate::error::Error::new(crate::error::ErrorKind::Invalid, e))?;
        let arrays = [Some(desc.layers), desc.normals, Some(desc.weights)];
        if arrays.into_iter().flatten().any(|a| !self.has_texture_array(a)) {
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::BadHandle,
                "that texture array wasn't made by this game",
            ));
        }
        let shader = match self.assets3d.terrain_shader {
            Some(shader) => shader,
            None => {
                let shader = self.try_create_shader("terrain.wgsl (built in)", include_str!("terrain.wgsl"))?;
                self.assets3d.terrain_shader = Some(shader);
                shader
            }
        };
        let inverse = desc.layer_size.map(|s| if s > 0.0 { 1.0 / s } else { 0.0 });
        let custom = [
            Vec4::new(
                desc.layers.layers() as f32,
                desc.normal_strength,
                if desc.normals.is_some() { 1.0 } else { 0.0 },
                0.0,
            ),
            Vec4::from_slice(&inverse[..4]),
            Vec4::from_slice(&inverse[4..]),
            Vec4::new(
                desc.weights_corner.x,
                desc.weights_corner.y,
                1.0 / desc.weights_size.x,
                1.0 / desc.weights_size.y,
            ),
        ];
        Ok(self.create_material(MaterialDesc {
            roughness: desc.roughness,
            shader: Some(shader),
            custom,
            custom_arrays: arrays,
            ..Default::default()
        }))
    }

    /// Makes a [`TerrainTile`] `size` wide from a square grid of heights
    /// (see [`MeshData::heightfield`]: row after row along +Z, NaN for a
    /// hole), drawn with `material`. Neighbouring tiles share their edge
    /// heights, so a tile of n × n heights starts where the last one ended,
    /// `size` further on.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, heights: &[f32], ground: ponko::render::Material) {
    /// let tile = ctx.gfx.create_terrain_tile(heights, 256.0, ground);
    /// ctx.world.spawn((ponko::d3::Transform::at(vec3(256.0, 0.0, 0.0)), tile));
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If `heights` isn't a square grid of at least 2 × 2, `size` isn't
    /// above 0, or the material comes from another game.
    pub fn create_terrain_tile(&mut self, heights: &[f32], size: f32, material: Material) -> TerrainTile {
        self.upload_terrain_tile(&TerrainTileData::new(heights, size), material)
    }

    /// Puts a tile's meshes, made by [`TerrainTileData::new`] (on any
    /// thread), on the GPU: the fast half of [`Graphics::create_terrain_tile`](crate::gfx::Graphics::create_terrain_tile),
    /// for tiles streamed in while the game runs.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let ground = ctx.gfx.default_material();
    ///     let job = ponko::streaming::Job::spawn(|| ponko::terrain::TerrainTileData::new(&[0.0; 33 * 33], 64.0));
    ///     let tile = ctx.gfx.upload_terrain_tile(&job.wait(), ground);
    ///     assert_eq!(tile.lod_distance(), 64.0);
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// If the material comes from another game.
    pub fn upload_terrain_tile(&mut self, data: &TerrainTileData, material: Material) -> TerrainTile {
        let lods = std::array::from_fn(|i| self.create_mesh(&data.lods[i], material));
        TerrainTile {
            lods,
            lod_distance: data.size(),
        }
    }
}

/// The meshes of a terrain tile at every detail level, made on the CPU
/// from its heights: build them on a worker thread with a [`Job`](crate::streaming::Job),
/// then [`Graphics::upload_terrain_tile`](crate::gfx::Graphics::upload_terrain_tile) on the game's.
///
/// ```
/// # use ponko::prelude::*;
/// let data = ponko::terrain::TerrainTileData::new(&[0.0; 5 * 5], 8.0);
/// assert_eq!(data.size(), 8.0);
/// ```
#[derive(Debug, Clone)]
pub struct TerrainTileData {
    lods: [MeshData; TERRAIN_LODS],
    ground: Heightfield,
}

impl TerrainTileData {
    /// A tile `size` wide from a square grid of heights, as
    /// [`Graphics::create_terrain_tile`](crate::gfx::Graphics::create_terrain_tile) takes them.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let flat = ponko::terrain::TerrainTileData::new(&[1.0; 3 * 3], 4.0);
    /// assert_eq!(flat.size(), 4.0);
    /// ```
    ///
    /// # Panics
    ///
    /// If `heights` isn't a square grid of at least 2 × 2 or `size` isn't above 0.
    #[must_use]
    pub fn new(heights: &[f32], size: f32) -> Self {
        assert!(
            size > 0.0 && size.is_finite(),
            "ponko: create_terrain_tile: size must be above 0 (got {size})"
        );
        Self {
            lods: std::array::from_fn(|i| MeshData::heightfield(heights, size, 1 << i)),
            ground: Heightfield::new(heights, size),
        }
    }

    /// How wide the tile is.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::terrain::TerrainTileData::new(&[0.0; 4], 2.0).size(), 2.0);
    /// ```
    #[must_use]
    pub fn size(&self) -> f32 {
        self.ground.size()
    }

    /// The tile's heights, for finding the ground under a point (keep it
    /// when the meshes go to the GPU).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let data = ponko::terrain::TerrainTileData::new(&[3.0; 3 * 3], 4.0);
    /// assert_eq!(data.heightfield().height_at(1.0, 1.0), Some(3.0));
    /// ```
    #[must_use]
    pub fn heightfield(&self) -> &Heightfield {
        &self.ground
    }
}

/// A terrain tile's heights, for finding the ground under a point on the
/// CPU: where to put a tree, keep a camera above the hills, or stand a
/// character. Heights between the grid points follow the same triangles
/// the tile is drawn with, so what's found is exactly what's seen.
/// Coordinates are from the tile's corner, X and Z from 0 to its size.
///
/// ```
/// # use ponko::prelude::*;
/// // 3 x 3 heights over 10 m: a ridge along Z in the middle.
/// let ground = ponko::terrain::Heightfield::new(&[0.0, 4.0, 0.0, 0.0, 4.0, 0.0, 0.0, 4.0, 0.0], 10.0);
/// assert_eq!(ground.height_at(5.0, 3.0), Some(4.0));
/// assert_eq!(ground.height_at(2.5, 5.0), Some(2.0));
/// assert_eq!(ground.height_at(11.0, 5.0), None, "off the tile");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Heightfield {
    heights: Vec<f32>,
    n: usize,
    size: f32,
}

impl Heightfield {
    /// A tile `size` wide from n × n heights, as
    /// [`Graphics::create_terrain_tile`](crate::gfx::Graphics::create_terrain_tile) takes them (NaN for a hole).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let flat = ponko::terrain::Heightfield::new(&[1.0; 4], 2.0);
    /// assert_eq!(flat.size(), 2.0);
    /// ```
    ///
    /// # Panics
    ///
    /// If `heights` isn't a square grid of at least 2 × 2 or `size` isn't above 0.
    #[must_use]
    pub fn new(heights: &[f32], size: f32) -> Self {
        let n = (heights.len() as f64).sqrt() as usize;
        assert!(
            n >= 2 && n * n == heights.len(),
            "ponko: Heightfield: {} heights isn't a square grid of at least 2 x 2",
            heights.len()
        );
        assert!(
            size > 0.0 && size.is_finite(),
            "ponko: Heightfield: size must be above 0 (got {size})"
        );
        Self {
            heights: heights.to_vec(),
            n,
            size,
        }
    }

    /// How wide the tile is.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::terrain::Heightfield::new(&[0.0; 9], 8.0).size(), 8.0);
    /// ```
    #[must_use]
    pub fn size(&self) -> f32 {
        self.size
    }

    /// The heights it was made from.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::terrain::Heightfield::new(&[0.0; 9], 8.0).heights().len(), 9);
    /// ```
    #[must_use]
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// The triangle under `(x, z)`: its three corners, from the tile's
    /// corner; `None` off the tile or in a hole.
    fn triangle(&self, x: f32, z: f32) -> Option<[Vec3; 3]> {
        if !(0.0..=self.size).contains(&x) || !(0.0..=self.size).contains(&z) {
            return None;
        }
        let cells = self.n - 1;
        let cell = self.size / cells as f32;
        let (gx, gz) = (x / cell, z / cell);
        let (i, j) = ((gx as usize).min(cells - 1), (gz as usize).min(cells - 1));
        let corner = |i: usize, j: usize| {
            let h = self.heights[j * self.n + i];
            Vec3::new(i as f32 * cell, h, j as f32 * cell)
        };
        let [a, b, c, d] = [corner(i, j), corner(i + 1, j), corner(i, j + 1), corner(i + 1, j + 1)];
        // A cell with a hole in any corner isn't drawn.
        if [a, b, c, d].iter().any(|p| p.y.is_nan()) {
            return None;
        }
        // Split along b-c, as the mesh is.
        if (gx - i as f32) + (gz - j as f32) <= 1.0 {
            Some([a, c, b])
        } else {
            Some([b, c, d])
        }
    }

    /// The ground's height at `(x, z)` from the tile's corner; `None` off
    /// the tile or over a hole.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let slope = ponko::terrain::Heightfield::new(&[0.0, 2.0, 0.0, 2.0], 1.0);
    /// assert_eq!(slope.height_at(0.5, 0.0), Some(1.0));
    /// ```
    #[must_use]
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let [p, q, r] = self.triangle(x, z)?;
        // The plane through the triangle, solved for y.
        let normal = (q - p).cross(r - p);
        Some(p.y - (normal.x * (x - p.x) + normal.z * (z - p.z)) / normal.y)
    }

    /// Which way the ground faces at `(x, z)` (unit length, up the slope's
    /// side); `None` off the tile or over a hole. The drawn normals are
    /// smoothed, but this is the flat triangle's: what something resting
    /// there tilts to.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let flat = ponko::terrain::Heightfield::new(&[5.0; 4], 1.0);
    /// assert_eq!(flat.normal_at(0.5, 0.5), Some(Vec3::Y));
    /// ```
    #[must_use]
    pub fn normal_at(&self, x: f32, z: f32) -> Option<Vec3> {
        let [p, q, r] = self.triangle(x, z)?;
        Some((q - p).cross(r - p).normalize())
    }
}

impl TerrainTile {
    /// Available terrain LOD levels.
    pub const LODS: usize = TERRAIN_LODS;
    /// Maximum material layers.
    pub const LAYERS: usize = TERRAIN_LAYERS;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_match_the_drawn_triangles() {
        // Random-ish heights: the found height must lie on a mesh triangle.
        let n = 9;
        let heights: Vec<f32> = (0..n * n).map(|i| ((i * 37 % 11) as f32) * 0.7).collect();
        let ground = Heightfield::new(&heights, 16.0);
        let mesh = MeshData::heightfield(&heights, 16.0, 1);
        let on_mesh = |x: f32, z: f32, y: f32| {
            mesh.indices.chunks(3).any(|t| {
                let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[t[k] as usize]);
                // Inside the triangle seen from above, and on its plane.
                let area = |p: Vec3, q: Vec3, r: Vec3| (q.x - p.x) * (r.z - p.z) - (r.x - p.x) * (q.z - p.z);
                let at = Vec3::new(x, 0.0, z);
                let (w0, w1, w2) = (area(b, c, at), area(c, a, at), area(a, b, at));
                let inside = (w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4) || (w0 <= 1e-4 && w1 <= 1e-4 && w2 <= 1e-4);
                let total = w0 + w1 + w2;
                inside && total.abs() > 1e-6 && {
                    let h = (a.y * w0 + b.y * w1 + c.y * w2) / total;
                    (h - y).abs() < 1e-3
                }
            })
        };
        for k in 0..400 {
            let (x, z) = ((k * 13 % 160) as f32 * 0.1, (k * 29 % 160) as f32 * 0.1);
            let y = ground.height_at(x, z).unwrap();
            assert!(on_mesh(x, z, y), "({x}, {z}) -> {y}");
        }
        // Grid points are exact.
        assert!((ground.height_at(4.0, 6.0).unwrap() - heights[3 * n + 2]).abs() < 1e-5);
        assert!((ground.height_at(16.0, 16.0).unwrap() - heights[n * n - 1]).abs() < 1e-5);
    }

    #[test]
    fn holes_and_the_outside_have_no_ground() {
        let mut heights = vec![1.0; 9];
        heights[4] = f32::NAN; // the middle: all four cells touch it
        let ground = Heightfield::new(&heights, 2.0);
        assert_eq!(ground.height_at(0.5, 0.5), None);
        assert_eq!(ground.height_at(-0.1, 0.5), None);
        assert_eq!(ground.normal_at(1.5, 2.1), None);
    }

    #[test]
    fn normals_face_up_the_slope_side() {
        // Rises along +X: the ground faces back toward -X.
        let ground = Heightfield::new(&[0.0, 1.0, 0.0, 1.0], 1.0);
        let n = ground.normal_at(0.3, 0.3).unwrap();
        assert!(n.x < 0.0 && n.y > 0.0 && n.z.abs() < 1e-6);
        assert!((n.length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn detail_drops_once_per_doubling_of_distance() {
        let at = |d| lod_at(d, 100.0);
        assert_eq!(at(0.0), 0);
        assert_eq!(at(99.9), 0);
        assert_eq!(at(100.0), 1);
        assert_eq!(at(199.0), 1);
        assert_eq!(at(200.0), 2);
        assert_eq!(at(399.0), 2);
        assert_eq!(at(400.0), 3);
        assert_eq!(at(1e9), 3);
        assert_eq!(at(f32::NAN), 0);
        assert_eq!(lod_at(50.0, 0.0), 0, "no distance: always the finest");
    }
}
