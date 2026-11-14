//! Ray casts, shape casts and overlap tests against the 3D physics world.

use rapier3d::prelude as rp;

use super::{CollisionGroups, Physics3D, from_rp, pose_to_rp, to_rp};
use glam::{Quat, Vec3};
use ponko_ecs::Entity;

/// What a ray or a shape cast hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit3D {
    /// The entity whose collider was hit.
    pub entity: Entity,
    /// Where it was hit, in the world. For a shape cast, the first point
    /// the shape touched.
    pub point: Vec3,
    /// The surface's outward direction at `point`.
    pub normal: Vec3,
    /// How far the ray or shape went before the hit (0 if a shape started
    /// out touching it).
    pub distance: f32,
}

/// Which colliders a query sees. The default sees every solid collider
/// and no sensors.
///
/// ```
/// use ponko::prelude::*;
/// # let player = Entity::from_bits(1);
/// const TERRAIN: u32 = 1 << 2;
/// let ground_only = ponko::d3::QueryFilter::default().groups(ponko::d3::CollisionGroups::new(u32::MAX, TERRAIN));
/// let not_me = ponko::d3::QueryFilter::default().exclude(player);
/// let triggers_too = ponko::d3::QueryFilter::default().with_sensors();
/// assert!(triggers_too.sensors && !not_me.sensors);
/// # let _ = ground_only;
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueryFilter3D {
    /// An entity to look through (the one asking, usually).
    pub exclude: Option<Entity>,
    /// The query touches colliders as a collider with these groups would.
    pub groups: CollisionGroups,
    /// Sees sensors as well.
    pub sensors: bool,
}

impl QueryFilter3D {
    /// Looking through `entity`.
    #[must_use]
    pub fn exclude(self, entity: Entity) -> Self {
        Self {
            exclude: Some(entity),
            ..self
        }
    }
    /// Seeing only what these groups touch.
    #[must_use]
    pub fn groups(self, groups: CollisionGroups) -> Self {
        Self { groups, ..self }
    }
    /// Seeing sensors too.
    #[must_use]
    pub fn with_sensors(self) -> Self {
        Self { sensors: true, ..self }
    }
}

impl Physics3D {
    fn rp_filter(&self, f: QueryFilter3D) -> rp::QueryFilter<'static> {
        let mut filter = rp::QueryFilter::default().groups(f.groups.to_rp());
        if !f.sensors {
            filter = filter.exclude_sensors();
        }
        if let Some(link) = f.exclude.and_then(|e| self.link(e)) {
            filter = filter.exclude_rigid_body(link.body);
        }
        filter
    }

    /// The closest collider a ray from `origin` towards `direction` hits
    /// within `max_distance`. Up to date as of the last physics step.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// fn look_down(ctx: &mut Ctx) {
    ///     let filter = ponko::d3::QueryFilter::default();
    ///     if let Some(hit) = ctx.physics3d.cast_ray(vec3(0.0, 5.0, 0.0), Vec3::NEG_Y, 100.0, filter) {
    ///         assert!((hit.distance - 4.5).abs() < 1e-4, "the ground's top is at 0.5");
    ///     }
    /// }
    /// Game::new("t", 1, 1).physics3d(Vec3::ZERO).sim_system(look_down).run_headless(2, |ctx| {
    ///     ctx.world.spawn((ponko::d3::Transform::default(), ponko::d3::RigidBody::fixed(), ponko::d3::Collider::cuboid(vec3(10.0, 1.0, 10.0))));
    /// });
    /// ```
    pub fn cast_ray(&self, origin: Vec3, direction: Vec3, max_distance: f32, filter: QueryFilter3D) -> Option<Hit3D> {
        let dir = direction.normalize_or_zero();
        if dir == Vec3::ZERO || !origin.is_finite() {
            return None;
        }
        let ray = rp::Ray::new(to_rp(origin), to_rp(dir));
        let (handle, hit) = self
            .world
            .cast_ray_and_get_normal(&ray, max_distance, true, self.rp_filter(filter))?;
        Some(Hit3D {
            entity: self.entity_of(handle)?,
            point: origin + dir * hit.time_of_impact,
            normal: from_rp(hit.normal),
            distance: hit.time_of_impact,
        })
    }

    /// Every collider a ray hits within `max_distance`, nearest first, into
    /// `out` (cleared first; reuse it so casts don't allocate).
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let physics = ponko::d3::Physics::default();
    /// let mut hits = Vec::new();
    /// physics.cast_ray_all(Vec3::ZERO, Vec3::X, 50.0, ponko::d3::QueryFilter::default(), &mut hits);
    /// assert!(hits.is_empty());
    /// ```
    pub fn cast_ray_all(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        filter: QueryFilter3D,
        out: &mut Vec<Hit3D>,
    ) {
        out.clear();
        let dir = direction.normalize_or_zero();
        if dir == Vec3::ZERO || !origin.is_finite() {
            return;
        }
        let ray = rp::Ray::new(to_rp(origin), to_rp(dir));
        for (handle, _, hit) in self
            .world
            .intersect_ray(ray, max_distance, true, self.rp_filter(filter))
        {
            if let Some(entity) = self.entity_of(handle) {
                out.push(Hit3D {
                    entity,
                    point: origin + dir * hit.time_of_impact,
                    normal: from_rp(hit.normal),
                    distance: hit.time_of_impact,
                });
            }
        }
        out.sort_unstable_by(|a, b| a.distance.total_cmp(&b.distance));
    }

    fn cast(
        &self,
        shape: &dyn rp::Shape,
        pose: rp::Pose,
        direction: Vec3,
        max_distance: f32,
        filter: QueryFilter3D,
    ) -> Option<Hit3D> {
        let dir = direction.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let options = rapier3d::parry::query::ShapeCastOptions {
            max_time_of_impact: max_distance,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        let (handle, hit) = self
            .world
            .cast_shape(&pose, to_rp(dir), shape, options, self.rp_filter(filter))?;
        Some(Hit3D {
            entity: self.entity_of(handle)?,
            point: from_rp(hit.witness1),
            normal: from_rp(hit.normal1),
            distance: hit.time_of_impact,
        })
    }

    /// Sweeps a ball of `radius` from `origin` towards `direction` and
    /// returns the first collider it touches within `max_distance` (thick
    /// rays: bullets with size, "is there room to walk there").
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let physics = ponko::d3::Physics::default();
    /// assert!(physics.cast_sphere(Vec3::ZERO, 0.5, Vec3::Z, 10.0, ponko::d3::QueryFilter::default()).is_none());
    /// ```
    pub fn cast_sphere(
        &self,
        origin: Vec3,
        radius: f32,
        direction: Vec3,
        max_distance: f32,
        filter: QueryFilter3D,
    ) -> Option<Hit3D> {
        let ball = rp::Ball::new(radius.max(f32::EPSILON));
        self.cast(
            &ball,
            pose_to_rp(origin, Quat::IDENTITY),
            direction,
            max_distance,
            filter,
        )
    }

    /// Sweeps an upright capsule centered on `origin` (`height` between the
    /// centers of its round ends, as [`Collider3D::capsule`](super::Collider3D::capsule))
    /// and returns the first collider it touches within `max_distance`.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let physics = ponko::d3::Physics::default();
    /// assert!(physics.cast_capsule(Vec3::ZERO, 1.2, 0.3, Vec3::X, 10.0, ponko::d3::QueryFilter::default()).is_none());
    /// ```
    pub fn cast_capsule(
        &self,
        origin: Vec3,
        height: f32,
        radius: f32,
        direction: Vec3,
        max_distance: f32,
        filter: QueryFilter3D,
    ) -> Option<Hit3D> {
        let capsule = rp::Capsule::new_y((height * 0.5).max(0.0), radius.max(f32::EPSILON));
        self.cast(
            &capsule,
            pose_to_rp(origin, Quat::IDENTITY),
            direction,
            max_distance,
            filter,
        )
    }

    fn overlap(&self, shape: &dyn rp::Shape, pose: rp::Pose, filter: QueryFilter3D, out: &mut Vec<Entity>) {
        out.clear();
        for (handle, _) in self.world.intersect_shape(pose, shape, self.rp_filter(filter)) {
            out.extend(self.entity_of(handle));
        }
    }

    /// Every entity whose collider overlaps the ball, into `out` (cleared first).
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let physics = ponko::d3::Physics::default();
    /// let mut near = Vec::new();
    /// physics.overlap_sphere(Vec3::ZERO, 5.0, ponko::d3::QueryFilter::default(), &mut near);
    /// assert!(near.is_empty());
    /// ```
    pub fn overlap_sphere(&self, center: Vec3, radius: f32, filter: QueryFilter3D, out: &mut Vec<Entity>) {
        let ball = rp::Ball::new(radius.max(f32::EPSILON));
        self.overlap(&ball, pose_to_rp(center, Quat::IDENTITY), filter, out);
    }

    /// Every entity whose collider overlaps the box of `size` centered on
    /// `center` and turned by `rotation`, into `out` (cleared first).
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let physics = ponko::d3::Physics::default();
    /// let mut inside = Vec::new();
    /// physics.overlap_box(Vec3::ZERO, Vec3::ONE, Quat::IDENTITY, ponko::d3::QueryFilter::default(), &mut inside);
    /// assert!(inside.is_empty());
    /// ```
    pub fn overlap_box(&self, center: Vec3, size: Vec3, rotation: Quat, filter: QueryFilter3D, out: &mut Vec<Entity>) {
        let h = (size * 0.5).max(Vec3::splat(f32::EPSILON));
        let cuboid = rp::Cuboid::new(rp::Vector::new(h.x, h.y, h.z));
        self.overlap(&cuboid, pose_to_rp(center, rotation), filter, out);
    }

    /// Every entity whose collider overlaps the upright capsule centered on
    /// `center`, into `out` (cleared first). Can a character stand up here?
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let physics = ponko::d3::Physics::default();
    /// let mut blocking = Vec::new();
    /// physics.overlap_capsule(Vec3::ZERO, 1.2, 0.3, ponko::d3::QueryFilter::default(), &mut blocking);
    /// assert!(blocking.is_empty());
    /// ```
    pub fn overlap_capsule(
        &self,
        center: Vec3,
        height: f32,
        radius: f32,
        filter: QueryFilter3D,
        out: &mut Vec<Entity>,
    ) {
        let capsule = rp::Capsule::new_y((height * 0.5).max(0.0), radius.max(f32::EPSILON));
        self.overlap(&capsule, pose_to_rp(center, Quat::IDENTITY), filter, out);
    }
}
