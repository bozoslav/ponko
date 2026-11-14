//! [`MeshData`]: triangles on the CPU, before they go to the GPU.

use crate::color::Color;
use glam::{Vec2, Vec3};

/// Triangles to make a [`Mesh`](crate::render::Mesh) from: corners with their
/// normals and texture coordinates, and three indices per triangle
/// (counter-clockwise seen from the front).
///
/// ```
/// # use ponko::prelude::*;
/// // One triangle facing +Z.
/// let tri = ponko::render::MeshData {
///     positions: vec![vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)],
///     normals: vec![Vec3::Z; 3],
///     uvs: vec![vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(0.0, 0.0)],
///     indices: vec![0, 1, 2],
///     ..Default::default()
/// };
/// assert!(tri.check().is_ok());
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    /// Corner positions.
    pub positions: Vec<Vec3>,
    /// One unit normal per corner. Empty: computed from the triangles.
    pub normals: Vec<Vec3>,
    /// One texture coordinate per corner (0..1, top-left origin). Empty:
    /// all zero.
    pub uvs: Vec<Vec2>,
    /// Three corner indices per triangle.
    pub indices: Vec<u32>,
    /// One color per corner, multiplying the material's color (and
    /// readable in shaders, for wind weights and other masks: `v.color`
    /// and `s.color`). Empty: all white. Kept to 8 bits per channel.
    pub colors: Vec<Color>,
    /// A second texture coordinate per corner (glTF's `TEXCOORD_1`), for
    /// the game's own shaders: `v.uv1` and `s.uv1` (lightmaps, texture
    /// layers, masks). The built-in shading doesn't use it. Empty: all zero.
    pub uvs1: Vec<Vec2>,
    /// For skinned meshes: up to four joints per corner that move it (see
    /// [`Graphics::create_skinned_mesh`](crate::gfx::Graphics::create_skinned_mesh)). Empty
    /// for meshes that don't bend.
    pub joints: Vec<[u16; 4]>,
    /// How much each of `joints` moves the corner; they're scaled to add
    /// up to 1. One per corner when `joints` is given.
    pub weights: Vec<[f32; 4]>,
    /// Shapes the mesh can blend toward (blend shapes: a smile, a blink, a
    /// dent), each as offsets from these corners. A model's
    /// [`MorphWeights`](crate::anim::MorphWeights) say how much of each to add.
    /// Empty for meshes that don't change shape.
    pub morph_targets: Vec<MorphTarget>,
    /// Optional piecewise linear channels. Each entry links a target's deltas
    /// at a positive weight to the controlling target index. Unlisted targets
    /// remain ordinary single-frame glTF targets.
    pub morph_frames: Vec<MorphFrame>,
}

/// One shape a mesh can blend toward (see [`MeshData::morph_targets`]):
/// per corner, how far it moves and how its normal changes at full weight.
///
/// ```
/// # use ponko::prelude::*;
/// let mut quad = ponko::render::MeshData::plane(1.0);
/// // "raise": lifts the first corner 0.5 up.
/// let mut up = vec![Vec3::ZERO; quad.positions.len()];
/// up[0] = vec3(0.0, 0.5, 0.0);
/// quad.morph_targets.push(ponko::anim::MorphTarget { name: "raise".into(), positions: up, normals: Vec::new() });
/// assert!(quad.check().is_ok());
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MorphTarget {
    /// What it's called, to find it by name ([`Graphics::morph_target`](crate::gfx::Graphics::morph_target)).
    pub name: String,
    /// One offset per corner.
    pub positions: Vec<Vec3>,
    /// One normal offset per corner; empty when normals don't change.
    pub normals: Vec<Vec3>,
}

/// One frame of a piecewise linear morph channel. The channel's weight is
/// stored at `channel` in MorphWeights; `target` selects this frame's deltas.
/// Every participating target must belong to exactly one channel.
/// ```
/// use ponko::prelude::*;
/// let frame = ponko::anim::MorphFrame { channel: 0, target: 1, weight: 0.5 };
/// assert_eq!(frame.weight, 0.5);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MorphFrame {
    /// Index controlled by animation or game code.
    pub channel: usize,
    /// Index of the frame's offsets in MeshData::morph_targets.
    pub target: usize,
    /// Positive, finite frame weight (1 means 100 percent).
    pub weight: f32,
}

impl MeshData {
    /// A cube `size` wide, centered on the origin, with each face's texture
    /// coordinates covering the whole image.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let cube = ponko::render::MeshData::cube(2.0);
    /// assert_eq!(cube.indices.len(), 36);
    /// ```
    #[must_use]
    pub fn cube(size: f32) -> Self {
        let h = size / 2.0;
        let mut mesh = Self::default();
        // Each face: its normal and two axes along it (right, up).
        let faces = [
            (Vec3::X, Vec3::NEG_Z, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        ];
        for (normal, right, up) in faces {
            let base = mesh.positions.len() as u32;
            for (u, v) in [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
                let at = normal * h + right * (u * 2.0 - 1.0) * h + up * (1.0 - v * 2.0) * h;
                mesh.positions.push(at);
                mesh.normals.push(normal);
                mesh.uvs.push(Vec2::new(u, v));
            }
            mesh.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        mesh
    }

    /// A flat square `size` wide on the XZ plane, facing up (+Y): ground.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let ground = ponko::render::MeshData::plane(100.0);
    /// assert!(ground.normals.iter().all(|n| *n == Vec3::Y));
    /// ```
    #[must_use]
    pub fn plane(size: f32) -> Self {
        let h = size / 2.0;
        Self {
            positions: vec![
                Vec3::new(-h, 0.0, h),
                Vec3::new(h, 0.0, h),
                Vec3::new(h, 0.0, -h),
                Vec3::new(-h, 0.0, -h),
            ],
            normals: vec![Vec3::Y; 4],
            uvs: vec![
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, 0.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        }
    }

    /// A sphere of `radius` around the origin, made of `segments` slices
    /// around and half as many from pole to pole (at least 3).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let ball = ponko::render::MeshData::sphere(0.5, 24);
    /// assert!(ball.positions.iter().all(|p| (p.length() - 0.5).abs() < 1e-5));
    /// ```
    #[must_use]
    pub fn sphere(radius: f32, segments: u32) -> Self {
        let around = segments.max(3);
        let rings = (around / 2).max(2);
        let mut mesh = Self::default();
        for ring in 0..=rings {
            let v = ring as f32 / rings as f32;
            let polar = v * std::f32::consts::PI;
            for step in 0..=around {
                let u = step as f32 / around as f32;
                let azimuth = u * std::f32::consts::TAU;
                let normal = Vec3::new(polar.sin() * azimuth.cos(), polar.cos(), -polar.sin() * azimuth.sin());
                mesh.positions.push(normal * radius);
                mesh.normals.push(normal);
                mesh.uvs.push(Vec2::new(u, v));
            }
        }
        let row = around + 1;
        for ring in 0..rings {
            for step in 0..around {
                let a = ring * row + step;
                let b = a + row;
                if ring > 0 {
                    mesh.indices.extend([a, b, a + 1]);
                }
                if ring + 1 < rings {
                    mesh.indices.extend([a + 1, b, b + 1]);
                }
            }
        }
        mesh
    }

    /// A square of terrain `size` wide from a grid of heights: `heights`
    /// holds n × n of them (n at least 2), row after row, x along +X and
    /// rows along +Z, from the corner at the origin to `(size, size)`. A
    /// NaN height leaves a hole (for caves and tunnels). Texture
    /// coordinates run 0 to 1 across the square.
    ///
    /// `step` thins it out for distant terrain: 1 uses every height, 2
    /// every other one, and so on (normals still come from every height, so
    /// lighting matches between details). The edges get a short skirt
    /// hanging down, which hides the cracks between neighbouring squares
    /// drawn at different details.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// // 3 x 3 heights: a little hill in the middle of a 10-unit square.
    /// let hill = ponko::render::MeshData::heightfield(&[0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0], 10.0, 1);
    /// assert!(hill.positions.contains(&vec3(5.0, 2.0, 5.0)));
    /// ```
    ///
    /// # Panics
    ///
    /// If `heights` isn't a square grid of at least 2 x 2, or `step` is 0.
    #[must_use]
    pub fn heightfield(heights: &[f32], size: f32, step: usize) -> Self {
        let n = (heights.len() as f64).sqrt() as usize;
        assert!(
            n >= 2 && n * n == heights.len(),
            "heightfield: {} heights isn't a square grid of at least 2 x 2",
            heights.len()
        );
        assert!(step > 0, "heightfield: step must be at least 1");
        let cell = size / (n - 1) as f32;
        let at = |x: usize, z: usize| heights[z * n + x];
        // Every step-th column and row, and always the last.
        let mut picks: Vec<usize> = (0..n).step_by(step).collect();
        if picks.last() != Some(&(n - 1)) {
            picks.push(n - 1);
        }
        let side = picks.len();
        let mut mesh = Self::default();
        for &z in &picks {
            for &x in &picks {
                // A hole's neighbours count as level with this corner, and a
                // hole itself as 0 (it's in no triangle but the data is finite).
                let h = at(x, z);
                let here = if h.is_nan() { 0.0 } else { h };
                let solid = |v: f32| if v.is_nan() { here } else { v };
                // Slopes from the neighbours on each side (one side at an edge).
                let (x0, x1) = (x.saturating_sub(1), (x + 1).min(n - 1));
                let (z0, z1) = (z.saturating_sub(1), (z + 1).min(n - 1));
                let dx = (solid(at(x1, z)) - solid(at(x0, z))) / ((x1 - x0) as f32 * cell);
                let dz = (solid(at(x, z1)) - solid(at(x, z0))) / ((z1 - z0) as f32 * cell);
                mesh.positions.push(Vec3::new(x as f32 * cell, here, z as f32 * cell));
                mesh.normals.push(Vec3::new(-dx, 1.0, -dz).normalize());
                mesh.uvs
                    .push(Vec2::new(x as f32 / (n - 1) as f32, z as f32 / (n - 1) as f32));
            }
        }
        let open = |i: usize, j: usize| !at(picks[i], picks[j]).is_nan();
        let index = |i: usize, j: usize| (j * side + i) as u32;
        for j in 0..side - 1 {
            for i in 0..side - 1 {
                if open(i, j) && open(i + 1, j) && open(i, j + 1) && open(i + 1, j + 1) {
                    let [a, b, c, d] = [index(i, j), index(i + 1, j), index(i, j + 1), index(i + 1, j + 1)];
                    mesh.indices.extend([a, c, b, b, c, d]);
                }
            }
        }
        // The skirt: around the edge, each corner again a little lower,
        // facing out. Walking the edge with the outside on the left of
        // the way down makes every wall face out.
        let depth = cell * step as f32 * 2.0;
        let last = side - 1;
        let edge: Vec<(usize, usize)> = (0..last)
            .map(|i| (i, 0))
            .chain((0..last).map(|j| (last, j)))
            .chain((0..last).map(|i| (last - i, last)))
            .chain((0..last).map(|j| (0, last - j)))
            .collect();
        let first_skirt = mesh.positions.len() as u32;
        for &(i, j) in &edge {
            let top = index(i, j) as usize;
            mesh.positions.push(mesh.positions[top] - Vec3::Y * depth);
            mesh.normals.push(mesh.normals[top]);
            mesh.uvs.push(mesh.uvs[top]);
        }
        for k in 0..edge.len() {
            let next = (k + 1) % edge.len();
            let ((i0, j0), (i1, j1)) = (edge[k], edge[next]);
            if !open(i0, j0) || !open(i1, j1) {
                continue;
            }
            let (a, b) = (index(i0, j0), index(i1, j1));
            let (a_low, b_low) = (first_skirt + k as u32, first_skirt + next as u32);
            mesh.indices.extend([a, b, a_low, b, b_low, a_low]);
        }
        mesh
    }

    /// The triangles: [`indices`](Self::indices) three at a time, the
    /// shape [`Physics::trimesh`](crate::d3::Physics::trimesh) takes. Extra
    /// indices at the end that make no whole triangle are left out.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let quad = ponko::render::MeshData::plane(1.0);
    /// assert_eq!(quad.triangles().len(), 2);
    /// assert_eq!(quad.triangles()[0], [quad.indices[0], quad.indices[1], quad.indices[2]]);
    /// ```
    #[must_use]
    pub fn triangles(&self) -> &[[u32; 3]] {
        self.indices.as_chunks::<3>().0
    }

    /// Checks the data fits together: whole triangles, indices in range,
    /// one normal and texture coordinate per corner when given, and finite
    /// numbers.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let broken = ponko::render::MeshData { positions: vec![Vec3::ZERO], indices: vec![0, 1, 2], ..Default::default() };
    /// let short = ponko::render::MeshData { positions: vec![Vec3::ZERO; 3], indices: vec![0, 1, 2], uvs1: vec![Vec2::ZERO], ..Default::default() };
    /// let err = short.check().unwrap_err();
    /// assert_eq!(err.kind(), ErrorKind::Invalid);
    /// assert!(err.message().contains("second texture coordinates"));
    /// let unweighted = ponko::render::MeshData { positions: vec![Vec3::ZERO; 3], indices: vec![0, 1, 2], joints: vec![[0; 4]; 3], ..Default::default() };
    /// assert!(unweighted.check().unwrap_err().message().contains("weights"));
    /// assert!(broken.check().is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid), saying what doesn't fit.
    pub fn check(&self) -> Result<(), crate::error::Error> {
        self.problem()
            .map_err(|e| crate::error::Error::new(crate::error::ErrorKind::Invalid, e))
    }

    /// What doesn't fit in the data, for [`check`](Self::check).
    pub(crate) fn problem(&self) -> Result<(), String> {
        let corners = self.positions.len();
        if self.indices.is_empty() || !self.indices.len().is_multiple_of(3) {
            return Err(format!("{} indices: needs 3 per triangle", self.indices.len()));
        }
        if let Some(&bad) = self.indices.iter().find(|&&i| i as usize >= corners) {
            return Err(format!("index {bad} is past the last of {corners} corners"));
        }
        if !self.normals.is_empty() && self.normals.len() != corners {
            return Err(format!("{} normals for {corners} corners", self.normals.len()));
        }
        if !self.uvs.is_empty() && self.uvs.len() != corners {
            return Err(format!("{} texture coordinates for {corners} corners", self.uvs.len()));
        }
        if !self.uvs1.is_empty() && self.uvs1.len() != corners {
            return Err(format!(
                "{} second texture coordinates for {corners} corners",
                self.uvs1.len()
            ));
        }
        if !self.colors.is_empty() && self.colors.len() != corners {
            return Err(format!("{} colors for {corners} corners", self.colors.len()));
        }
        if self.joints.len() != self.weights.len() || !self.joints.is_empty() && self.joints.len() != corners {
            return Err(format!(
                "{} joints and {} weights for {corners} corners: needs one of each per corner, or none",
                self.joints.len(),
                self.weights.len()
            ));
        }
        if !self.positions.iter().all(|p| p.is_finite()) || !self.normals.iter().all(|n| n.is_finite()) {
            return Err("a position or normal isn't a finite number".into());
        }
        for (i, f) in self.morph_frames.iter().enumerate() {
            if f.channel >= self.morph_targets.len()
                || f.target >= self.morph_targets.len()
                || !f.weight.is_finite()
                || f.weight <= 0.
                || self.morph_frames[..i]
                    .iter()
                    .any(|p| p.target == f.target || (p.channel == f.channel && p.weight == f.weight))
                || self
                    .morph_frames
                    .iter()
                    .any(|p| p.target == f.channel && p.channel != f.channel)
            {
                return Err("invalid morph frame mapping".into());
            }
        }
        for t in &self.morph_targets {
            if t.positions.len() != corners || !t.normals.is_empty() && t.normals.len() != corners {
                return Err(format!(
                    "morph target {:?}: {} offsets and {} normal offsets for {corners} corners",
                    t.name,
                    t.positions.len(),
                    t.normals.len()
                ));
            }
            if !t.positions.iter().chain(&t.normals).all(|v| v.is_finite()) {
                return Err(format!("morph target {:?}: an offset isn't a finite number", t.name));
            }
        }
        Ok(())
    }

    /// The smallest sphere around the origin-centered box that holds every
    /// corner: center and radius. For culling.
    pub(crate) fn bounds(&self) -> (Vec3, f32) {
        let (min, max) = self
            .positions
            .iter()
            .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| {
                (lo.min(*p), hi.max(*p))
            });
        if min.x > max.x {
            return (Vec3::ZERO, 0.0);
        }
        let center = (min + max) / 2.0;
        let radius = self.positions.iter().map(|p| p.distance(center)).fold(0.0, f32::max);
        (center, radius)
    }

    /// Normals for every corner, averaged from the triangles around it,
    /// where none were given.
    pub(crate) fn normals_or_computed(&self) -> std::borrow::Cow<'_, [Vec3]> {
        if !self.normals.is_empty() {
            return std::borrow::Cow::Borrowed(&self.normals);
        }
        let mut normals = vec![Vec3::ZERO; self.positions.len()];
        for tri in self.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| self.positions[i as usize]);
            let n = (b - a).cross(c - a); // area-weighted
            for &i in tri {
                normals[i as usize] += n;
            }
        }
        for n in &mut normals {
            *n = n.normalize_or(Vec3::Y);
        }
        std::borrow::Cow::Owned(normals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    /// Every triangle's winding agrees with its corners' normals (faces out).
    fn faces_out(mesh: &MeshData) {
        for tri in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| mesh.positions[i as usize]);
            let face = (b - a).cross(c - a);
            if face.length() < 1e-9 {
                continue; // degenerate at a pole
            }
            let normal = mesh.normals[tri[0] as usize] + mesh.normals[tri[1] as usize] + mesh.normals[tri[2] as usize];
            assert!(face.dot(normal) > 0.0, "triangle {tri:?} faces in");
        }
    }

    #[test]
    fn built_in_shapes_are_valid_and_face_out() {
        for mesh in [MeshData::cube(1.0), MeshData::plane(3.0), MeshData::sphere(1.0, 16)] {
            mesh.check().unwrap();
            faces_out(&mesh);
        }
    }

    /// 9 x 9 heights over a 16-unit square: a ramp along x plus bumps.
    fn ramp() -> Vec<f32> {
        (0..81).map(|i| (i % 9) as f32 * 0.5 + ((i / 9) as f32).sin()).collect()
    }

    /// The heightfield's triangles: the top ones (up) and the skirt's (sideways).
    fn split(mesh: &MeshData) -> (Vec<[Vec3; 3]>, Vec<[Vec3; 3]>) {
        mesh.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| t.map(|i| mesh.positions[i as usize]))
            .partition(|[a, b, c]| (*b - *a).cross(*c - *a).normalize().y > 0.1)
    }

    #[test]
    fn heightfields_face_up_and_their_skirts_face_out() {
        for step in [1, 2, 3, 4, 8, 100] {
            let mesh = MeshData::heightfield(&ramp(), 16.0, step);
            mesh.check().unwrap();
            let (top, skirt) = split(&mesh);
            assert!(!top.is_empty() && !skirt.is_empty());
            for [a, b, c] in skirt {
                let out = (b - a).cross(c - a);
                let from_middle = (a + b) / 2.0 - Vec3::new(8.0, 0.0, 8.0);
                assert!(out.y.abs() < 1e-4, "skirt walls are upright");
                assert!(out.dot(from_middle) > 0.0, "skirt wall at {a} faces in (step {step})");
            }
        }
    }

    #[test]
    fn every_detail_keeps_the_edge_heights() {
        // Neighbouring tiles meet only if the edge corners stay put.
        let full = MeshData::heightfield(&ramp(), 16.0, 1);
        for step in [2, 3, 4, 8] {
            let coarse = MeshData::heightfield(&ramp(), 16.0, step);
            assert!(coarse.positions.len() < full.positions.len());
            for corner in [
                vec3(0.0, 0.0, 0.0),
                vec3(16.0, 0.0, 0.0),
                vec3(0.0, 0.0, 16.0),
                vec3(16.0, 0.0, 16.0),
            ] {
                let found = |m: &MeshData| m.positions.iter().find(|p| p.x == corner.x && p.z == corner.z).copied();
                assert_eq!(found(&coarse), found(&full), "step {step}");
            }
        }
    }

    #[test]
    fn nan_heights_cut_holes() {
        let mut heights = vec![0.0; 25];
        let whole = MeshData::heightfield(&heights, 4.0, 1);
        heights[12] = f32::NAN; // the middle corner: the 4 cells around it go
        let holed = MeshData::heightfield(&heights, 4.0, 1);
        holed.check().unwrap();
        assert_eq!(split(&whole).0.len() - split(&holed).0.len(), 8);
    }

    #[test]
    fn heightfield_normals_follow_the_slope() {
        // Rising 1 per unit along x: normals lean back 45 degrees.
        let heights: Vec<f32> = (0..16).map(|i| (i % 4) as f32).collect();
        let mesh = MeshData::heightfield(&heights, 3.0, 1);
        let lean = Vec3::new(-1.0, 1.0, 0.0).normalize();
        assert!(mesh.normals.iter().all(|n| (*n - lean).length() < 1e-5));
    }

    #[test]
    #[should_panic(expected = "isn't a square grid")]
    fn heightfields_need_a_square_grid() {
        let _ = MeshData::heightfield(&[0.0; 6], 1.0, 1);
    }

    #[test]
    fn bounds_hold_every_corner() {
        let mesh = MeshData::cube(2.0);
        let (center, radius) = mesh.bounds();
        assert!(center.length() < 1e-6);
        assert!((radius - 3f32.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn missing_normals_are_computed_from_the_triangles() {
        let mut plane = MeshData::plane(1.0);
        plane.normals.clear();
        assert!(
            plane
                .normals_or_computed()
                .iter()
                .all(|n| (*n - Vec3::Y).length() < 1e-6)
        );
    }
}
