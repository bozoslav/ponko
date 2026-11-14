//! Projected decals: a box in the world whose image is drawn onto the opaque
//! surfaces inside it, every frame, without building meshes. They ride on the
//! light clustering: each frame the decals the camera sees are put into the
//! same froxel grid the point and spot lights use, and the mesh shader
//! blends the ones in a pixel's froxel into its surface before lighting.

use super::lights::{GRID, LIST_WIDTH};
use super::{Camera3D, Transform3D, linear};
use crate::color::Color;
use glam::{Mat4, Vec2, Vec3};
use ponko_ecs::Component;
use ponko_ecs::World;

/// The most decals drawn in a frame. When more are in view, the ones
/// nearest the camera are kept.
/// ```
/// assert_eq!(ponko::d3::Decal::MAX, 4096);
/// ```
pub const MAX_DECALS: usize = 4096;

/// The most decals one screen cell blends (the screen is 16 x 9 cells, each
/// split into 24 slices of depth). Where more overlap, the ones with the
/// highest [`Decal3D::order`] are kept.
/// ```
/// assert_eq!(ponko::d3::Decal::MAX_PER_CLUSTER, 64);
/// ```
pub const MAX_DECALS_PER_CLUSTER: usize = 64;

/// A box that paints an image onto the opaque surfaces inside it. The
/// entity's [`d3::Transform`](crate::d3::Transform) places the box (its scale multiplies `size`).
/// The image is projected along the box's **-Z** (looking from +Z), its top
/// along +Y, as with [`MeshData::decal`](crate::render::MeshData::decal); only
/// surfaces turned towards +Z get it.
///
/// Unlike a mesh decal, nothing is built: the decal is drawn onto whatever
/// is inside the box each frame, so it follows moving and skinned models,
/// costs the same however much geometry is under it, and is lit with the
/// surface it lands on. Put hundreds on screen ([`Decal::MAX`](crate::d3::Decal::MAX) at most).
/// Parent a decal to a model (see `Attached`) to carry it along.
///
/// Images come from the texture array set with
/// [`Graphics::set_decal_textures`](crate::gfx::Graphics::set_decal_textures); without
/// one a decal is a flat `color` rectangle. Surfaces whose model has no
/// matching [`ReceivesDecals`] layer are skipped, as are transparent
/// materials.
///
/// ```
/// use ponko::prelude::*;
/// // A 2 x 2 m mark, 1 m deep, from the first image of the decal textures.
/// let mark = ponko::d3::Decal { size: vec3(2.0, 2.0, 1.0), image: 0, ..Default::default() };
/// let mut world = World::new();
/// // Looking down: the box's +Z turned to point up.
/// let down = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
/// world.spawn((ponko::d3::Transform::at(vec3(0.0, 0.5, 0.0)).rotated(down), mark));
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[serde(default)]
pub struct Decal3D {
    /// Width (x), height (y) and depth (z) of the box, in world units.
    /// Default `(1, 1, 1)`.
    pub size: Vec3,
    /// The layer of the decal textures holding the color image, or
    /// [`Decal3D::NO_IMAGE`] for a flat color. Colors are sRGB, as images
    /// are painted. Default 0.
    pub image: u32,
    /// The layer holding a tangent-space normal map, or [`Decal3D::NO_IMAGE`]
    /// for none (the surface's own normals). Default none.
    pub normal_image: u32,
    /// The top left of the image in its layer, as a fraction of it: use a
    /// part of a layer as an atlas. Default `(0, 0)`.
    pub uv_min: Vec2,
    /// The bottom right of the image in its layer. Default `(1, 1)`.
    pub uv_max: Vec2,
    /// Multiplies the image (sRGB); its alpha multiplies `opacity`. Default
    /// white.
    pub color: Color,
    /// How much of the decal shows, 0 to 1, times the image's alpha.
    /// Default 1.
    pub opacity: f32,
    /// How strongly the normal map tilts the surface. Default 1.
    pub normal_strength: f32,
    /// The surface's roughness where the decal is fully opaque, or a
    /// negative number to leave it as it is (the default). Wet blood is low,
    /// dust high.
    pub roughness: f32,
    /// The surface's metallic value under the decal, or a negative number
    /// to leave it (the default). Dirt on metal is `0`.
    pub metallic: f32,
    /// Angle from the projection axis, in radians, up to which the decal is
    /// fully visible; beyond it, it fades out so it doesn't stretch over
    /// steep surfaces. Default 1.0 (about 57 degrees).
    pub angle_fade_start: f32,
    /// Angle where it has faded out completely. Default 1.45 (about 83
    /// degrees); surfaces at 90 degrees or more get none.
    pub angle_fade_end: f32,
    /// How much of the box's depth, from each end, the decal fades over
    /// (0 to 1): 0 is a hard edge, 1 fades from the middle. Default 0.25.
    pub depth_fade: f32,
    /// Distance from the camera past which the decal is not drawn; it fades
    /// out over the last quarter. 0 is unlimited (the default).
    pub max_distance: f32,
    /// Decals with a higher order are drawn over those with a lower one.
    /// Equal orders keep the entities' order in the world, which is not
    /// stable across despawns. Default 0.
    pub order: i32,
    /// Which models get it: it shows on a model whose [`ReceivesDecals`] has
    /// any of these bits. Only the low 16 bits count. Default all.
    pub layers: u32,
}

impl Default for Decal3D {
    fn default() -> Self {
        Self {
            size: Vec3::ONE,
            image: 0,
            normal_image: Self::NO_IMAGE,
            uv_min: Vec2::ZERO,
            uv_max: Vec2::ONE,
            color: Color::WHITE,
            opacity: 1.0,
            normal_strength: 1.0,
            roughness: -1.0,
            metallic: -1.0,
            angle_fade_start: 1.0,
            angle_fade_end: 1.45,
            depth_fade: 0.25,
            max_distance: 0.0,
            order: 0,
            layers: ReceivesDecals::ALL.0,
        }
    }
}

/// Which decal layers a model takes (see [`Decal3D::layers`]). Models
/// without it take every decal. Only the low 16 bits count.
///
/// ```
/// use ponko::prelude::*;
/// let mut ctx_world = World::new();
/// // A character that no decal lands on:
/// let hero = ctx_world.spawn((ponko::d3::Transform::default(), ponko::render::ReceivesDecals::NONE));
/// // Only blood (layer bit 1):
/// let wall = ctx_world.spawn((ponko::d3::Transform::default(), ponko::render::ReceivesDecals(0b10)));
/// # let _ = (hero, wall);
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Component)]
pub struct ReceivesDecals(pub u32);

impl ReceivesDecals {
    /// Every decal lands on it (what a model without the component does).
    pub const ALL: Self = Self(0xFFFF);
    /// No decal lands on it.
    pub const NONE: Self = Self(0);
}

impl Default for ReceivesDecals {
    fn default() -> Self {
        Self::ALL
    }
}

/// The mask a model carries to the shader, as an exact float.
pub(crate) fn receiver_mask(r: Option<&ReceivesDecals>) -> f32 {
    r.map_or(ReceivesDecals::ALL.0, |r| r.0 & 0xFFFF) as f32
}

/// Eight RGBA32Float texels, one row per decal (see `mesh.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GpuDecal {
    /// World to the box: rows of the affine matrix taking the box to
    /// -0.5..0.5 on each axis.
    pub to_box: [[f32; 4]; 3],
    /// Linear tint; opacity.
    pub color: [f32; 4],
    /// Image corners: u0, v0, u1, v1.
    pub rect: [f32; 4],
    /// Color layer (-1: none), normal layer (-1: none), normal strength, receiver mask.
    pub images: [f32; 4],
    /// cos of the angle where fading starts, cos where it ends, depth fade.
    pub fades: [f32; 4],
    /// Roughness (negative: unchanged), metallic (negative: unchanged).
    pub surface: [f32; 4],
}

/// Texels in a decal's row.
pub(crate) const DECAL_TEXELS: usize = 8;

/// A decal before the grid: sorted by `key`, with the cells it covers.
#[derive(Clone, Copy)]
struct Candidate {
    key: (i32, u32),
    distance: f32,
    record: GpuDecal,
    lo: [usize; 3],
    hi: [usize; 3],
}

/// This frame's decals, in drawing order, and the froxels that see them:
/// the same grid as the lights (see [`super::lights::LightGrid`]).
#[derive(Default)]
pub(crate) struct DecalGrid {
    pub records: Vec<GpuDecal>,
    /// R32Uint words: (offset, count) for each cell, then the decals'
    /// indices (`first_row` + the decal's number). Offsets count from the
    /// start of the light lists' texture, after `base` words.
    pub lists: Vec<u32>,
    candidates: Vec<Candidate>,
    counts: Vec<u32>,
    taken: Vec<u32>,
}

const CELLS: usize = GRID[0] * GRID[1] * GRID[2];

impl DecalGrid {
    /// Works out which decals the camera sees and which cells each covers.
    /// `base` is where the lists will sit in the light lists' texture (in
    /// words, a multiple of its width), `first_row` the row of the first
    /// decal in the light records' texture, `layers` how many layers the
    /// decal textures have, and `depth` the lights' (near, slices per log
    /// depth) mapping.
    pub fn collect(
        &mut self,
        world: &mut World,
        camera: &Camera3D,
        depth: [f32; 4],
        layers: u32,
        base: usize,
        first_row: usize,
    ) {
        self.records.clear();
        self.lists.clear();
        self.candidates.clear();
        let near = depth[0];
        let far = camera.far.max(near * 1.001);
        let view = camera.view();
        let proj = camera.view_proj() * view.inverse();
        let mut seq = 0u32;
        for (t, d) in world.query::<(&Transform3D, &Decal3D)>().iter() {
            seq += 1;
            let key = (d.order, seq);
            if let Some(c) = candidate(t, d, key, camera, view, proj, near, far, depth[1], layers) {
                self.candidates.push(c);
            }
        }
        if self.candidates.is_empty() {
            return;
        }
        if self.candidates.len() > MAX_DECALS {
            // Keep the nearest.
            self.candidates
                .select_nth_unstable_by(MAX_DECALS, |a, b| a.distance.total_cmp(&b.distance));
            self.candidates.truncate(MAX_DECALS);
        }
        self.candidates.sort_unstable_by_key(|c| c.key);
        self.records.extend(self.candidates.iter().map(|c| c.record));

        // The cells' lists. Decals are visited from the top down so a full
        // cell keeps the highest orders, and each list is filled from its
        // end, so the shader blends it bottom up.
        self.counts.clear();
        self.counts.resize(CELLS, 0);
        for c in self.candidates.iter().rev() {
            visit(c.lo, c.hi, |cell| {
                if (self.counts[cell] as usize) < MAX_DECALS_PER_CLUSTER {
                    self.counts[cell] += 1;
                }
            });
        }
        let total: usize = self.counts.iter().map(|&n| n as usize).sum();
        let used = CELLS * 2 + total;
        self.lists.resize(used.div_ceil(LIST_WIDTH) * LIST_WIDTH, 0);
        let mut offset = base + CELLS * 2;
        for cell in 0..CELLS {
            self.lists[cell * 2] = offset as u32;
            self.lists[cell * 2 + 1] = self.counts[cell];
            offset += self.counts[cell] as usize;
        }
        self.taken.clear();
        self.taken.resize(CELLS, 0);
        let count = self.candidates.len();
        for (rank, c) in self.candidates.iter().enumerate().rev() {
            let id = (first_row + rank) as u32;
            debug_assert!(rank < count);
            visit(c.lo, c.hi, |cell| {
                let n = self.counts[cell];
                let taken = self.taken[cell];
                if taken < n {
                    let start = self.lists[cell * 2] as usize - base;
                    self.lists[start + (n - 1 - taken) as usize] = id;
                    self.taken[cell] = taken + 1;
                }
            });
        }
    }
}

/// Culls one decal against the camera and works out its record and cells.
#[allow(clippy::too_many_arguments)]
fn candidate(
    t: &Transform3D,
    d: &Decal3D,
    key: (i32, u32),
    camera: &Camera3D,
    view: Mat4,
    proj: Mat4,
    near: f32,
    far: f32,
    slices_per_log: f32,
    layers: u32,
) -> Option<Candidate> {
    let mask = d.layers & 0xFFFF;
    if mask == 0
        || !t.position.is_finite()
        || !t.rotation.is_finite()
        || !t.scale.is_finite()
        || !d.size.is_finite()
        || d.size.min_element() <= 0.0
        || t.scale.min_element() == 0.0
        || !d.opacity.is_finite()
        || d.opacity <= 0.0
        || !d.uv_min.is_finite()
        || !d.uv_max.is_finite()
    {
        return None;
    }
    let linear_color = linear(d.color);
    if !linear_color[..3].iter().all(|v| v.is_finite()) {
        return None;
    }
    let distance = t.position.distance(camera.position);
    let mut opacity = d.opacity.clamp(0.0, 1.0)
        * if d.color.a.is_finite() {
            d.color.a.clamp(0.0, 1.0)
        } else {
            1.0
        };
    if d.max_distance > 0.0 {
        if distance > d.max_distance {
            return None;
        }
        let k = ((distance / d.max_distance - 0.75) / 0.25).clamp(0.0, 1.0);
        opacity *= 1.0 - k * k * (3.0 - 2.0 * k);
    }
    if t.rotation.length_squared() < 1e-8 {
        return None;
    }
    let rotation = t.rotation.normalize();
    let to_world = Mat4::from_scale_rotation_translation(t.scale * d.size, rotation, t.position);
    let to_box = to_world.inverse();
    if !to_box.is_finite() {
        return None;
    }
    // The cells the box reaches: its corners in view space.
    let mut lo = [0usize; 3];
    let mut hi = [GRID[0] - 1, GRID[1] - 1, GRID[2] - 1];
    let mut z_min = f32::INFINITY;
    let mut z_max = f32::NEG_INFINITY;
    let mut crosses_near = false;
    let mut corners = [Vec3::ZERO; 8];
    for (i, corner) in corners.iter_mut().enumerate() {
        let local = Vec3::new(
            if i & 1 == 0 { -0.5 } else { 0.5 },
            if i & 2 == 0 { -0.5 } else { 0.5 },
            if i & 4 == 0 { -0.5 } else { 0.5 },
        );
        let v = view.transform_point3(to_world.transform_point3(local));
        *corner = v;
        z_min = z_min.min(-v.z);
        z_max = z_max.max(-v.z);
        crosses_near |= -v.z <= near;
    }
    if z_max < near || z_min > far {
        return None;
    }
    if !crosses_near {
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for v in &corners {
            let z = -v.z;
            for axis in 0..2 {
                let scale = if axis == 0 { proj.x_axis.x } else { proj.y_axis.y };
                let p = v[axis] * scale / z;
                min[axis] = min[axis].min(p);
                max[axis] = max[axis].max(p);
            }
        }
        for axis in 0..2 {
            if min[axis] > 1.0 || max[axis] < -1.0 {
                return None;
            }
            let tile = |v: f32| {
                (((v * 0.5 + 0.5) * GRID[axis] as f32).floor() as i32).clamp(0, GRID[axis] as i32 - 1) as usize
            };
            lo[axis] = tile(min[axis]);
            hi[axis] = tile(max[axis]);
        }
    }
    let slice = |z: f32| {
        ((z.max(near) / near).ln() * slices_per_log)
            .floor()
            .clamp(0.0, (GRID[2] - 1) as f32) as usize
    };
    lo[2] = slice(z_min);
    hi[2] = slice(z_max);
    let in_layers = |i: u32| (i != Decal3D::NO_IMAGE && i < layers).then_some(i as f32);
    let (start, end) = (d.angle_fade_start, d.angle_fade_end);
    let (start, end) = if start.is_finite() && end.is_finite() {
        let start = start.clamp(0.0, std::f32::consts::FRAC_PI_2);
        (start, end.clamp(start, std::f32::consts::FRAC_PI_2))
    } else {
        (1.0, 1.45)
    };
    let rows = [to_box.row(0), to_box.row(1), to_box.row(2)];
    let record = GpuDecal {
        to_box: rows.map(|r| r.to_array()),
        color: [linear_color[0], linear_color[1], linear_color[2], opacity],
        rect: [d.uv_min.x, d.uv_min.y, d.uv_max.x, d.uv_max.y],
        images: [
            in_layers(d.image).unwrap_or(-1.0),
            in_layers(d.normal_image).unwrap_or(-1.0),
            if d.normal_strength.is_finite() {
                d.normal_strength
            } else {
                1.0
            },
            mask as f32,
        ],
        fades: [
            start.cos(),
            end.cos().max(0.0),
            if d.depth_fade.is_finite() {
                d.depth_fade.clamp(0.0, 1.0)
            } else {
                0.0
            },
            0.0,
        ],
        surface: [
            if d.roughness.is_finite() {
                d.roughness.min(1.0)
            } else {
                -1.0
            },
            if d.metallic.is_finite() {
                d.metallic.min(1.0)
            } else {
                -1.0
            },
            0.0,
            0.0,
        ],
    };
    Some(Candidate {
        key,
        distance,
        record,
        lo,
        hi,
    })
}

fn visit(lo: [usize; 3], hi: [usize; 3], mut f: impl FnMut(usize)) {
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                f((z * GRID[1] + y) * GRID[0] + x);
            }
        }
    }
}

impl Decal3D {
    /// Maximum decals in one frame.
    pub const MAX: usize = MAX_DECALS;
    /// Maximum decals affecting one cluster.
    pub const MAX_PER_CLUSTER: usize = MAX_DECALS_PER_CLUSTER;
}

impl Decal3D {
    /// `image` and `normal_image` value that selects no image.
    pub const NO_IMAGE: u32 = u32::MAX;
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, vec3};

    fn camera() -> Camera3D {
        let mut c = Camera3D::new(Vec2::new(800.0, 600.0));
        c.position = Vec3::ZERO;
        c.rotation = Quat::IDENTITY;
        c.far = 100.0;
        c
    }
    fn depth(c: &Camera3D) -> [f32; 4] {
        [c.near, GRID[2] as f32 / (c.far / c.near).ln(), 0.0, 0.0]
    }
    fn collect(w: &mut World, layers: u32) -> DecalGrid {
        let c = camera();
        let mut g = DecalGrid::default();
        g.collect(w, &c, depth(&c), layers, 4096, 100);
        g
    }

    #[test]
    fn only_decals_in_view_are_kept_and_sorted_by_order() {
        let mut w = World::new();
        let at = |z: f32| Transform3D::at(vec3(0.0, 0.0, z));
        w.spawn((
            at(-10.0),
            Decal3D {
                order: 5,
                ..Default::default()
            },
        ));
        w.spawn((
            at(-10.0),
            Decal3D {
                order: -3,
                ..Default::default()
            },
        ));
        w.spawn((at(10.0), Decal3D::default())); // behind the camera
        w.spawn((at(-500.0), Decal3D::default())); // past the far plane
        w.spawn((
            at(-10.0),
            Decal3D {
                opacity: 0.0,
                ..Default::default()
            },
        ));
        w.spawn((
            at(-10.0),
            Decal3D {
                layers: 0,
                ..Default::default()
            },
        ));
        w.spawn((
            at(-10.0),
            Decal3D {
                size: vec3(f32::NAN, 1.0, 1.0),
                ..Default::default()
            },
        ));
        let g = collect(&mut w, 0);
        assert_eq!(g.records.len(), 2);
        // The lower order comes first; the center cell lists both, bottom up.
        let cell = (0..CELLS)
            .find(|&c| g.lists[c * 2 + 1] == 2)
            .expect("a cell sees both decals");
        let start = g.lists[cell * 2] as usize - 4096;
        assert_eq!(g.lists[start..start + 2], [100, 101]);
        assert!(g.lists.len().is_multiple_of(LIST_WIDTH));
    }

    #[test]
    fn the_box_places_the_image_and_scale_multiplies_size() {
        let mut w = World::new();
        w.spawn((
            Transform3D::at(vec3(2.0, 0.0, -10.0)).scaled(2.0),
            Decal3D {
                size: vec3(1.0, 1.0, 0.5),
                ..Default::default()
            },
        ));
        let g = collect(&mut w, 0);
        let r = g.records[0].to_box;
        let to_box = |p: Vec3| {
            Vec3::new(
                r[0][..3].iter().zip(p.to_array()).map(|(a, b)| a * b).sum::<f32>() + r[0][3],
                r[1][..3].iter().zip(p.to_array()).map(|(a, b)| a * b).sum::<f32>() + r[1][3],
                r[2][..3].iter().zip(p.to_array()).map(|(a, b)| a * b).sum::<f32>() + r[2][3],
            )
        };
        assert!((to_box(vec3(2.0, 0.0, -10.0))).length() < 1e-5);
        // Box is 2 x 2 x 1 m: its edge is 1 m from the middle in x.
        assert!((to_box(vec3(3.0, 0.0, -10.0)) - Vec3::new(0.5, 0.0, 0.0)).length() < 1e-5);
        assert!((to_box(vec3(2.0, 0.0, -9.5)) - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-5);
    }

    #[test]
    fn a_distance_limit_culls_and_fades() {
        let mut w = World::new();
        let near = w.spawn((
            Transform3D::at(vec3(0.0, 0.0, -10.0)),
            Decal3D {
                max_distance: 40.0,
                ..Default::default()
            },
        ));
        w.spawn((
            Transform3D::at(vec3(0.0, 0.0, -38.0)),
            Decal3D {
                max_distance: 40.0,
                ..Default::default()
            },
        ));
        w.spawn((
            Transform3D::at(vec3(0.0, 0.0, -50.0)),
            Decal3D {
                max_distance: 40.0,
                ..Default::default()
            },
        ));
        let g = collect(&mut w, 0);
        assert_eq!(g.records.len(), 2);
        assert_eq!(g.records[0].color[3], 1.0);
        assert!(g.records[1].color[3] < 0.5, "fading near its limit");
        let _ = near;
    }

    #[test]
    fn images_outside_the_layers_draw_flat() {
        let mut w = World::new();
        w.spawn((
            Transform3D::at(vec3(0.0, 0.0, -5.0)),
            Decal3D {
                image: 3,
                normal_image: 1,
                ..Default::default()
            },
        ));
        let g = collect(&mut w, 2);
        assert_eq!(g.records[0].images[..2], [-1.0, 1.0]);
        let g = collect(&mut w, 0);
        assert_eq!(g.records[0].images[..2], [-1.0, -1.0]);
    }

    #[test]
    fn full_cells_keep_the_highest_orders() {
        let mut w = World::new();
        for i in 0..MAX_DECALS_PER_CLUSTER + 10 {
            w.spawn((
                Transform3D::at(vec3(0.0, 0.0, -10.0)),
                Decal3D {
                    order: i as i32,
                    ..Default::default()
                },
            ));
        }
        let g = collect(&mut w, 0);
        assert_eq!(g.records.len(), MAX_DECALS_PER_CLUSTER + 10);
        let cell = (0..CELLS).find(|&c| g.lists[c * 2 + 1] > 0).unwrap();
        assert_eq!(g.lists[cell * 2 + 1] as usize, MAX_DECALS_PER_CLUSTER);
        let start = g.lists[cell * 2] as usize - 4096;
        let ids = &g.lists[start..start + MAX_DECALS_PER_CLUSTER];
        assert_eq!(ids[0], 100 + 10, "the 10 lowest orders were dropped");
        assert_eq!(
            ids[MAX_DECALS_PER_CLUSTER - 1],
            100 + (MAX_DECALS_PER_CLUSTER + 9) as u32
        );
        assert!(ids.windows(2).all(|p| p[0] < p[1]));
    }

    #[test]
    fn more_than_the_cap_keeps_the_nearest() {
        let mut w = World::new();
        for i in 0..MAX_DECALS + 5 {
            let z = -6.0 - (i as f32) * 0.01;
            w.spawn((
                Transform3D::at(vec3((i % 7) as f32 * 0.2 - 0.6, 0.0, z)),
                Decal3D::default(),
            ));
        }
        let g = collect(&mut w, 0);
        assert_eq!(g.records.len(), MAX_DECALS);
    }

    #[test]
    fn a_decal_crossing_the_camera_covers_every_tile() {
        let mut w = World::new();
        w.spawn((
            Transform3D::at(Vec3::ZERO),
            Decal3D {
                size: Vec3::splat(4.0),
                ..Default::default()
            },
        ));
        let g = collect(&mut w, 0);
        let covered = (0..CELLS).filter(|&c| g.lists[c * 2 + 1] > 0).count();
        assert!(covered >= GRID[0] * GRID[1], "all tiles in the first slices: {covered}");
    }

    #[test]
    fn receivers_carry_their_layers_as_exact_floats() {
        assert_eq!(receiver_mask(None), 65535.0);
        assert_eq!(receiver_mask(Some(&ReceivesDecals::NONE)), 0.0);
        assert_eq!(receiver_mask(Some(&ReceivesDecals(0xFFFF_0002))), 2.0);
        assert_eq!(ReceivesDecals::default(), ReceivesDecals::ALL);
    }
}
