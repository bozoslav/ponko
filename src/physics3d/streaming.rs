//! Collision meshes built on any thread, colliders for terrain tiles, and
//! keeping static colliders only near the places that need them, for big
//! streamed worlds.

use rapier3d::prelude as rp;
use serde::{Deserialize, Serialize};

use super::{CollisionGroups, CollisionMesh, Physics3D, pose_to_rp, to_rp};
use crate::d3::Transform as Transform3D;
use glam::{Vec2, Vec3};
use ponko_ecs::{Component, Entity, World};

/// A convex hull, triangle mesh or heightfield built on the CPU, on any
/// thread: the slow half of [`Physics3D::heightfield`] and friends. Build
/// it in a [`Job`](crate::streaming::Job) with the rest of a streamed cell, then hand
/// it to [`Physics3D::add_mesh`] on the game's thread, which is quick.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(0, |ctx| {
///     let job = ponko::streaming::Job::spawn(|| ponko::d3::CollisionMeshData::heightfield(&[0.0; 33 * 33], 64.0));
///     let ground = ctx.physics3d.add_mesh(&job.wait());
///     assert!(ctx.physics3d.is_live(ground));
/// });
/// ```
#[derive(Clone)]
pub struct CollisionMeshData {
    pub(super) shape: rp::SharedShape,
    /// From the entity's position to the shape's center.
    pub(super) offset: Vec3,
}

impl std::fmt::Debug for CollisionMeshData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollisionMeshData")
            .field("shape", &self.shape.shape_type())
            .field("offset", &self.offset)
            .finish()
    }
}

impl CollisionMeshData {
    /// Ground for a terrain tile, from the same n × n heights and size as
    /// [`TerrainTileData::new`](crate::terrain::TerrainTileData::new) (row after row
    /// along +Z, NaN for a hole), as [`Physics3D::heightfield`] makes it.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let ground = ponko::d3::CollisionMeshData::heightfield(&[0.0, 1.0, 0.0, 1.0], 4.0);
    /// # let _ = ground;
    /// ```
    ///
    /// # Panics
    ///
    /// If `heights` isn't a square grid of at least 2 × 2 or `size` isn't above 0.
    #[track_caller]
    #[must_use]
    pub fn heightfield(heights: &[f32], size: f32) -> Self {
        Self::heightfield_named(heights, size, "CollisionMeshData::heightfield")
    }

    #[track_caller]
    pub(super) fn heightfield_named(heights: &[f32], size: f32, name: &str) -> Self {
        let n = (heights.len() as f64).sqrt() as usize;
        assert!(
            n >= 2 && n * n == heights.len(),
            "{name}: {} heights isn't a square grid of at least 2 x 2",
            heights.len()
        );
        assert!(
            size > 0.0 && size.is_finite(),
            "{name}: size must be above 0 (got {size})"
        );
        // rapier's rows run along Z and its storage is by column: element
        // (z, x) sits at z + x * n, where ours is at z * n + x.
        let mut data = vec![0.0; n * n];
        for z in 0..n {
            for x in 0..n {
                let h = heights[z * n + x];
                data[z + x * n] = if h.is_finite() { h } else { 0.0 };
            }
        }
        let grid = rapier3d::parry::utils::Array2::new(n, n, data);
        let mut field = rapier3d::parry::shape::HeightField::with_flags(
            grid,
            rp::Vector::new(size, 1.0, size),
            rapier3d::parry::shape::HeightFieldFlags::FIX_INTERNAL_EDGES,
        );
        // Cells with a hole in any corner aren't drawn, so they aren't solid.
        for z in 0..n - 1 {
            for x in 0..n - 1 {
                let corners = [(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)];
                if corners.iter().any(|&(x, z)| !heights[z * n + x].is_finite()) {
                    field.set_cell_status(z, x, rapier3d::parry::shape::HeightFieldCellStatus::CELL_REMOVED);
                }
            }
        }
        let half = size * 0.5;
        Self {
            shape: rp::SharedShape::new(field),
            offset: Vec3::new(half, 0.0, half),
        }
    }

    /// A convex hull around `points`, as [`Physics3D::convex_hull`] makes
    /// it. `None` if the points are all on one plane or not finite.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let corners = [vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0)];
    /// assert!(ponko::d3::CollisionMeshData::convex_hull(&corners).is_some());
    /// ```
    #[must_use]
    pub fn convex_hull(points: &[Vec3]) -> Option<Self> {
        if points.len() < 4 || !points.iter().all(|p| p.is_finite()) {
            return None;
        }
        let points: Vec<rp::Vector> = points.iter().map(|&p| to_rp(p)).collect();
        let shape = rp::SharedShape::convex_hull(&points)?;
        // A hull of points on one plane has no volume.
        let mass = shape.mass_properties(1.0).mass();
        if !(mass > 0.0 && mass.is_finite()) {
            return None;
        }
        Some(Self {
            shape,
            offset: Vec3::ZERO,
        })
    }

    /// A triangle mesh, as [`Physics3D::trimesh`] makes it: `indices` are
    /// three per triangle into `vertices`. `None` if there are no
    /// triangles, an index is out of range or a vertex isn't finite.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let floor = ponko::d3::CollisionMeshData::trimesh(&[Vec3::ZERO, Vec3::X, Vec3::Z], &[[0, 2, 1]]);
    /// assert!(floor.is_some());
    /// ```
    #[must_use]
    pub fn trimesh(vertices: &[Vec3], indices: &[[u32; 3]]) -> Option<Self> {
        if indices.is_empty()
            || !vertices.iter().all(|p| p.is_finite())
            || indices.iter().flatten().any(|&i| i as usize >= vertices.len())
        {
            return None;
        }
        let vertices: Vec<rp::Vector> = vertices.iter().map(|&p| to_rp(p)).collect();
        let shape =
            rp::SharedShape::trimesh_with_flags(vertices, indices.to_vec(), rp::TriMeshFlags::FIX_INTERNAL_EDGES)
                .ok()?;
        Some(Self {
            shape,
            offset: Vec3::ZERO,
        })
    }
}

/// Makes an entity's terrain tile solid: a fixed collider of a
/// [heightfield](Physics3D::heightfield) made from the tile's heights, at
/// the entity's [`d3::Transform`](crate::d3::Transform) (the tile's corner), turned and scaled with
/// it, so it matches the drawn ground triangle for triangle. Holes are open.
/// Put it on the entity with the [`TerrainTile`](crate::terrain::TerrainTile) (not
/// with a [`RigidBody3D`](super::RigidBody3D)): despawning the tile removes
/// its collider, and giving it a new mesh after an edit swaps the collider
/// at the next physics step.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).physics3d(vec3(0.0, -9.81, 0.0)).run_headless(1, |ctx| {
///     let heights = [0.0; 33 * 33];
///     let ground = ctx.gfx.default_material();
///     let tile = ctx.gfx.create_terrain_tile(&heights, 64.0, ground);
///     let solid = ponko::d3::TerrainCollider::new(ctx.physics3d.heightfield(&heights, 64.0));
///     ctx.world.spawn((ponko::d3::Transform::at(vec3(-32.0, 0.0, -32.0)), tile, solid));
/// });
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Component)]
pub struct TerrainCollider {
    /// The heightfield, from [`Physics3D::heightfield`] or
    /// [`Physics3D::add_mesh`] (any collision mesh works). Serde writes it
    /// as `null` and reads it back as [`CollisionMesh::NONE`](crate::handle::Handle::NONE).
    #[serde(with = "super::unsaved_mesh")]
    pub mesh: CollisionMesh,
    /// 0 ice, 1 rubber. Default 0.5.
    pub friction: f32,
    /// Bounciness: 0 none (the default), 1 keeps all its speed.
    pub restitution: f32,
    /// Which colliders it touches. Default all.
    pub groups: CollisionGroups,
}

impl TerrainCollider {
    /// The ground of `mesh`, with the default surface.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(mesh: ponko::d3::CollisionMesh) {
    /// let ice = ponko::d3::TerrainCollider::new(mesh).friction(0.05);
    /// # }
    /// ```
    #[must_use]
    pub fn new(mesh: CollisionMesh) -> Self {
        Self {
            mesh,
            friction: 0.5,
            restitution: 0.0,
            groups: CollisionGroups::ALL,
        }
    }
    /// With this friction.
    #[must_use]
    pub fn friction(self, friction: f32) -> Self {
        Self { friction, ..self }
    }
    /// With this bounciness.
    #[must_use]
    pub fn restitution(self, restitution: f32) -> Self {
        Self { restitution, ..self }
    }
    /// In these collision groups.
    #[must_use]
    pub fn groups(self, groups: CollisionGroups) -> Self {
        Self { groups, ..self }
    }
}

/// Keeps static colliders around an entity (the player, a vehicle) once
/// [`Physics3D::stream_colliders`] is on: the colliders of fixed bodies and
/// [`TerrainCollider`]s within `radius` of it (on the ground, X and Z) are
/// in the physics world, nearest first, and stay until they're beyond
/// `keep_radius` of every focus. The gap between the two stops a collider
/// at the edge from coming and going as the focus moves back and forth.
///
/// ```
/// # use ponko::prelude::*;
/// let player = ponko::d3::ColliderFocus::new(150.0, 200.0);
/// assert_eq!(player.keep_radius, 200.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Component)]
#[repr(C)] // shared with C/C++ (`PonkoColliderFocus`)
pub struct ColliderFocus {
    /// Colliders nearer than this come in.
    pub radius: f32,
    /// Colliders beyond this from every focus go (raised to `radius` when less).
    pub keep_radius: f32,
}

impl Default for ColliderFocus {
    fn default() -> Self {
        Self::new(100.0, 150.0)
    }
}

impl ColliderFocus {
    /// Colliders within `radius` come in, and go beyond `keep_radius`.
    #[must_use]
    pub fn new(radius: f32, keep_radius: f32) -> Self {
        Self { radius, keep_radius }
    }
}

/// A rectangle on the ground: min X, min Z, max X, max Z.
pub(super) type Rect = [f32; 4];

/// How far `p` is from the nearest point of `r`, on the ground.
fn away(r: Rect, p: Vec2) -> f32 {
    let dx = (r[0] - p.x).max(p.x - r[2]).max(0.0);
    let dz = (r[1] - p.y).max(p.y - r[3]).max(0.0);
    (dx * dx + dz * dz).sqrt()
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Moving bodies keep colliders within this of their bounds (in X and Z):
/// wheels and feet reach a little beyond a body's own shape.
const HOLD_MARGIN: f32 = 1.0;

/// Reused memory for [`stream`].
#[derive(Default)]
pub(super) struct Scratch {
    /// Each focus: where it is, its radius and its keep radius.
    focus: Vec<(Vec2, f32, f32)>,
    /// The ground under each moving body, widened by [`HOLD_MARGIN`].
    holders: Vec<Rect>,
    /// Static colliders in reach and not in: distance, link index.
    wanted: Vec<(f32, u32)>,
}

impl Physics3D {
    /// Turns collider streaming on: from the next step, the colliders of
    /// fixed bodies and [`TerrainCollider`]s are only in the physics world
    /// near entities with a [`ColliderFocus`] (and a [`d3::Transform`](crate::d3::Transform)), at
    /// most `max_new` more each step, nearest first, so a big world's
    /// colliders come in over a few steps. `None` turns it off: every
    /// collider comes back at the next step. Drawing isn't affected, so
    /// the world can be drawn much further out than it's solid.
    ///
    /// A collider never goes while a moving body (dynamic, kinematic, a
    /// character) is over it, so a crate left behind doesn't fall through
    /// the ground. Bodies far from every focus find no new ground, though:
    /// give anything that travels far on its own (vehicles) a focus too.
    /// Ray casts and other queries only see colliders that are in.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).physics3d(vec3(0.0, -9.81, 0.0)).run_headless(1, |ctx| {
    ///     ctx.physics3d.stream_colliders(Some(16));
    ///     ctx.world.spawn((ponko::d3::Transform::default(), ponko::d3::Character::new(1.0, 0.3), ponko::d3::ColliderFocus::new(150.0, 200.0)));
    /// });
    /// ```
    pub fn stream_colliders(&mut self, max_new: Option<usize>) {
        self.streaming = max_new;
    }

    /// Whether collider streaming is on, and how many colliders may come in per step.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut physics = ponko::d3::Physics::default();
    /// physics.stream_colliders(Some(8));
    /// assert_eq!(physics.collider_streaming(), Some(8));
    /// ```
    #[must_use]
    pub fn collider_streaming(&self) -> Option<usize> {
        self.streaming
    }

    /// Whether `e`'s collider is in the physics world now: false for an
    /// entity that isn't a body (yet: bodies join at the next physics
    /// step), and for a static collider that [collider
    /// streaming](Physics3D::stream_colliders) left out.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let physics = ponko::d3::Physics::default();
    /// assert!(!physics.is_collider_active(Entity::from_bits(0)));
    /// ```
    #[must_use]
    pub fn is_collider_active(&self, e: Entity) -> bool {
        self.link(e)
            .and_then(|l| self.world.colliders.get(l.collider))
            .is_some_and(rp::Collider::is_enabled)
    }

    /// A collision mesh built by [`CollisionMeshData`] (on any thread),
    /// for [`Collider3D::mesh`](super::Collider3D::mesh) and
    /// [`TerrainCollider`]. Quick: the data's shape is shared, not copied.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut physics = ponko::d3::Physics::default();
    /// let rock = ponko::d3::CollisionMeshData::convex_hull(&[Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z]).unwrap();
    /// let mesh = physics.add_mesh(&rock);
    /// assert!(physics.is_live(mesh));
    /// ```
    pub fn add_mesh(&mut self, data: &CollisionMeshData) -> CollisionMesh {
        self.add_shape(data.shape.clone(), data.offset)
    }

    /// Where a static link's collider lies on the ground, with its body at `t`.
    pub(super) fn rect_of(&self, collider: rp::ColliderHandle, t: &Transform3D) -> Rect {
        let Some(c) = self.world.colliders.get(collider) else {
            return [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        };
        let local = c.position_wrt_parent().copied().unwrap_or_default();
        let aabb = c.shape().compute_aabb(&(pose_to_rp(t.position, t.rotation) * local));
        [aabb.mins.x, aabb.mins.z, aabb.maxs.x, aabb.maxs.z]
    }

    pub(super) fn set_active(&mut self, i: usize, active: bool) {
        let Some(link) = self.links[i].as_mut() else { return };
        link.active = active;
        if let Some(c) = self.world.colliders.get_mut(link.collider) {
            c.set_enabled(active);
        }
    }
}

/// A shape `scale`d (heightfields and triangle meshes; others as they are).
pub(super) fn scaled(shape: &rp::SharedShape, scale: Vec3) -> rp::SharedShape {
    let scale = to_rp(scale.abs().max(Vec3::splat(1e-6)));
    if let Some(field) = shape.as_heightfield() {
        rp::SharedShape::new(field.clone().scaled(scale))
    } else if let Some(mesh) = shape.as_trimesh() {
        rp::SharedShape::new(mesh.clone().scaled(scale))
    } else {
        shape.clone()
    }
}

/// Brings static colliders near a focus in (nearest first, at most the
/// budget) and takes those far from every focus out, unless a moving body
/// is over them. With streaming off, brings back any that are out.
pub(super) fn stream(physics: &mut Physics3D, world: &mut World) {
    physics.moving();
    let Some(max_new) = physics.streaming else {
        if physics.streamed_out {
            physics.streamed_out = false;
            for k in 0..physics.statics.len() {
                let i = physics.statics[k] as usize;
                if physics.links[i].is_some_and(|l| !l.active) {
                    physics.set_active(i, true);
                }
            }
        }
        return;
    };
    physics.streamed_out = true;
    let mut scratch = std::mem::take(&mut physics.stream_scratch);
    scratch.focus.clear();
    world.query::<(&Transform3D, &ColliderFocus)>().for_each(|(t, f)| {
        let at = Vec2::new(t.position.x, t.position.z);
        scratch.focus.push((at, f.radius, f.keep_radius.max(f.radius)));
    });
    scratch.wanted.clear();
    let mut holders_found = false;
    for k in 0..physics.statics.len() {
        let i = physics.statics[k];
        let Some(link) = physics.links[i as usize] else {
            continue;
        };
        if link.active {
            if scratch.focus.iter().any(|&(at, _, keep)| away(link.rect, at) <= keep) {
                continue;
            }
            if !holders_found {
                holders_found = true;
                find_holders(physics, &mut scratch.holders);
            }
            if scratch.holders.iter().any(|&h| overlaps(h, link.rect)) {
                continue;
            }
            physics.set_active(i as usize, false);
        } else {
            let nearest = scratch
                .focus
                .iter()
                .map(|&(at, radius, _)| (away(link.rect, at), radius))
                .filter(|&(d, radius)| d <= radius)
                .map(|(d, _)| d)
                .reduce(f32::min);
            if let Some(d) = nearest {
                scratch.wanted.push((d, i));
            }
        }
    }
    if scratch.wanted.len() > max_new {
        // Nearest first; equal distances in a fixed order.
        scratch
            .wanted
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    for &(_, i) in scratch.wanted.iter().take(max_new) {
        physics.set_active(i as usize, true);
    }
    physics.stream_scratch = scratch;
}

/// The ground under every moving body, widened a little.
fn find_holders(physics: &mut Physics3D, holders: &mut Vec<Rect>) {
    holders.clear();
    let n = physics.moving().len();
    for k in 0..n {
        let i = physics.moving[k] as usize;
        let Some(link) = physics.links[i] else { continue };
        let Some(c) = physics.world.colliders.get(link.collider) else {
            continue;
        };
        let aabb = c.compute_aabb();
        holders.push([
            aabb.mins.x - HOLD_MARGIN,
            aabb.mins.z - HOLD_MARGIN,
            aabb.maxs.x + HOLD_MARGIN,
            aabb.maxs.z + HOLD_MARGIN,
        ]);
    }
}
