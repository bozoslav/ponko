//! 2D physics with rapier (feature `physics`): rigid bodies and colliders as
//! components, stepped inside the simulation clock.
//!
//! [`d2::Transform`](crate::d2::Transform) stays the source of truth: set it and the body teleports;
//! dynamic bodies write their new position back after every step.
//! Physics is opt-in per entity, so the million-entity paths (spatial grid,
//! flow fields) stay as fast as before.

use std::sync::mpsc;

use rapier2d::prelude as rp;
use serde::{Deserialize, Serialize};

use crate::collision::{BodyKind, CollisionStarted, CollisionStopped};
use crate::d2::Transform as Transform2D;
use crate::{ctx::Ctx, events::Events};
use glam::Vec2;
use ponko_ecs::{Component, Entity, With, World};

/// Makes an entity a physics body. It also needs a [`Collider`] and a
/// [`d2::Transform`](crate::d2::Transform).
///
/// ```
/// use ponko::prelude::*;
/// let crate_body = ponko::d2::RigidBody::dynamic();
/// let wall = ponko::d2::RigidBody::fixed();
/// assert_eq!(crate_body.gravity_scale, 1.0);
/// assert_eq!(wall.kind, ponko::collision::BodyKind::Fixed);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct RigidBody {
    /// Dynamic, fixed or kinematic.
    pub kind: BodyKind,
    /// 1 normal, 0 floats, negative falls up.
    pub gravity_scale: f32,
    /// Slows movement down over time (air drag); 0 none.
    pub linear_damping: f32,
    /// Slows spinning down over time; 0 none.
    pub angular_damping: f32,
    /// Never rotates (characters).
    pub lock_rotation: bool,
    /// Continuous collision detection: fast bodies (bullets) don't pass
    /// through thin walls. Costs a little more.
    pub ccd: bool,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self::dynamic()
    }
}

impl RigidBody {
    /// A body moved by gravity and collisions.
    #[must_use]
    pub const fn dynamic() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            gravity_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            lock_rotation: false,
            ccd: false,
        }
    }
    /// A body that never moves.
    #[must_use]
    pub const fn fixed() -> Self {
        Self {
            kind: BodyKind::Fixed,
            ..Self::dynamic()
        }
    }
    /// A body only you move.
    #[must_use]
    pub const fn kinematic() -> Self {
        Self {
            kind: BodyKind::Kinematic,
            ..Self::dynamic()
        }
    }
}

/// Most corners a [`Shape::Polygon`] can have.
pub(crate) const MAX_POLYGON_POINTS: usize = 8;

/// A collider's shape, in pixels, centered on the entity (plus
/// [`Collider::offset`]) and turned with it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Shape {
    /// A circle.
    Circle {
        /// In pixels.
        radius: f32,
    },
    /// A rectangle `size` wide and high.
    Rect {
        /// Width and height in pixels.
        size: Vec2,
    },
    /// A vertical capsule: `height` between the centers of its round ends.
    Capsule {
        /// Distance between the centers of the round ends, in pixels.
        height: f32,
        /// Radius of the round ends, in pixels.
        radius: f32,
    },
    /// A convex polygon: the convex hull of its corners, in pixels around
    /// the entity's position. Make one with [`Collider::polygon`]; read the
    /// corners with [`Shape::points`].
    Polygon {
        /// The corners; only the first `count` are used.
        points: [Vec2; MAX_POLYGON_POINTS],
        /// How many corners: 3 to [`MAX_POLYGON_POINTS`](Shape::MAX_POLYGON_POINTS).
        count: u8,
    },
}

impl Shape {
    /// Most corners a [`Shape::Polygon`] can have.
    ///
    /// ```
    /// assert_eq!(ponko::d2::Shape::MAX_POLYGON_POINTS, 8);
    /// ```
    pub const MAX_POLYGON_POINTS: usize = MAX_POLYGON_POINTS;

    /// A polygon's corners (empty for other shapes).
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let wedge = ponko::d2::Collider::polygon(&[vec2(0.0, 0.0), vec2(40.0, 0.0), vec2(0.0, -20.0)]);
    /// assert_eq!(wedge.shape.points().len(), 3);
    /// assert!(ponko::d2::Collider::circle(5.0).shape.points().is_empty());
    /// ```
    #[must_use]
    pub fn points(&self) -> &[Vec2] {
        match self {
            Shape::Polygon { points, count } => &points[..usize::from(*count).min(MAX_POLYGON_POINTS)],
            _ => &[],
        }
    }
}

/// The solid shape of a body, and how it touches others.
///
/// ```
/// use ponko::prelude::*;
/// let ball = ponko::d2::Collider::circle(8.0).restitution(0.9);
/// let trigger = ponko::d2::Collider::rect(vec2(64.0, 64.0)).sensor();
/// assert!(trigger.sensor);
/// assert_eq!(ball.restitution, 0.9);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Component)]
pub struct Collider {
    /// Circle, rectangle, capsule or convex polygon.
    pub shape: Shape,
    /// Mass per square pixel (mass = density × area). Default 1.
    #[serde(default = "one")]
    pub density: f32,
    /// 0 ice, 1 rubber. Default 0.5.
    #[serde(default = "half")]
    pub friction: f32,
    /// Bounciness: 0 none, 1 keeps all its speed. Default 0.
    #[serde(default)]
    pub restitution: f32,
    /// Detects overlaps (sends [`CollisionStarted`]) but doesn't push.
    #[serde(default)]
    pub sensor: bool,
    /// Where the shape sits relative to the entity's position.
    #[serde(default)]
    pub offset: Vec2,
}

// Collider defaults for fields missing from JSON, as in `Collider::of`.
fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}

impl Collider {
    fn of(shape: Shape) -> Self {
        Self {
            shape,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            sensor: false,
            offset: Vec2::ZERO,
        }
    }
    /// A circle.
    #[must_use]
    pub fn circle(radius: f32) -> Self {
        Self::of(Shape::Circle { radius })
    }
    /// A rectangle `size` wide and high.
    #[must_use]
    pub fn rect(size: Vec2) -> Self {
        Self::of(Shape::Rect { size })
    }
    /// A vertical capsule (good for characters).
    #[must_use]
    pub fn capsule(height: f32, radius: f32) -> Self {
        Self::of(Shape::Capsule { height, radius })
    }
    /// A convex polygon around the entity's position: the convex hull of
    /// `points` (3 to 8 of them, any order). Ramps, wedges, rocks, ships.
    /// For a concave shape, put convex pieces on separate bodies.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let ramp = ponko::d2::Collider::polygon(&[vec2(-50.0, 10.0), vec2(50.0, 10.0), vec2(50.0, -10.0)]);
    /// assert_eq!(ramp.shape.points().len(), 3);
    /// ```
    ///
    /// # Panics
    ///
    /// With fewer than 3 or more than [`MAX_POLYGON_POINTS`](Shape::MAX_POLYGON_POINTS) points, or a
    /// point that isn't finite.
    #[must_use]
    #[track_caller]
    pub fn polygon(points: &[Vec2]) -> Self {
        assert!(
            (3..=MAX_POLYGON_POINTS).contains(&points.len()),
            "Collider::polygon: 3 to {MAX_POLYGON_POINTS} points, got {}",
            points.len()
        );
        assert!(
            points.iter().all(|p| p.is_finite()),
            "Collider::polygon: points must be finite"
        );
        let mut corners = [Vec2::ZERO; MAX_POLYGON_POINTS];
        corners[..points.len()].copy_from_slice(points);
        Self::of(Shape::Polygon {
            points: corners,
            count: points.len() as u8,
        })
    }
    /// With this density.
    #[must_use]
    pub fn density(self, density: f32) -> Self {
        Self { density, ..self }
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
    /// As a sensor: reports overlaps, doesn't collide.
    #[must_use]
    pub fn sensor(self) -> Self {
        Self { sensor: true, ..self }
    }
    /// Shifted from the entity's position.
    #[must_use]
    pub fn offset(self, offset: Vec2) -> Self {
        Self { offset, ..self }
    }
}

/// A body's speed, in pixels and radians per second. Read it after a step;
/// write it to set the speed (a jump, a thrown crate, a moving platform).
///
/// ```
/// use ponko::prelude::*;
/// let v = ponko::d2::Velocity::linear(vec2(0.0, -300.0)); // a jump, y is down
/// assert_eq!(v.angular, 0.0);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Component)]
#[serde(default)]
#[repr(C)] // C sees it as PonkoVelocity.
pub struct Velocity {
    /// Pixels per second.
    pub linear: Vec2,
    /// Radians per second, clockwise on screen.
    pub angular: f32,
}

impl Velocity {
    /// Moving at `linear`, not spinning.
    #[must_use]
    pub const fn linear(linear: Vec2) -> Self {
        Self { linear, angular: 0.0 }
    }
}

/// Which colliders a 2D query sees. Defaults to solid colliders, excluding sensors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueryFilter {
    /// Entity to look through.
    pub exclude: Option<Entity>,
    /// Include sensor colliders.
    pub sensors: bool,
}
impl QueryFilter {
    /// Excludes one entity.
    #[must_use]
    pub fn exclude(self, entity: Entity) -> Self {
        Self {
            exclude: Some(entity),
            ..self
        }
    }
    /// Includes sensors.
    #[must_use]
    pub fn with_sensors(self) -> Self {
        Self { sensors: true, ..self }
    }
}

/// What a ray hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// The entity whose collider was hit.
    pub entity: Entity,
    /// Where the ray hit, in world pixels.
    pub point: Vec2,
    /// The surface's outward direction at `point`.
    pub normal: Vec2,
    /// From the ray's start to `point`.
    pub distance: f32,
}

/// What the world last knew of one body, to notice your changes.
#[derive(Clone, Copy)]
struct Link {
    entity: Entity,
    body: rp::RigidBodyHandle,
    rigid: RigidBody,
    shape: Collider,
    transform: Transform2D,
    velocity: Velocity,
    seen: u64,
}

/// The physics world: `ctx.physics`. Add it to a game with
/// [`Game::physics`](crate::game::Game::physics); then entities with
/// [`RigidBody`], [`Collider`] and [`d2::Transform`](crate::d2::Transform) are simulated.
pub struct Physics {
    world: rp::PhysicsWorld,
    /// By entity index.
    links: Vec<Option<Link>>,
    gravity: Vec2,
    step: u64,
    /// From `set_paused`: the built-in step does nothing while set.
    paused: bool,
    /// The world's load count when `links` were made (see `forget_bodies`).
    world_loads: u64,
    collisions: mpsc::Receiver<rp::CollisionEvent>,
    collector: rp::ChannelEventCollector,
    _forces: mpsc::Receiver<rp::ContactForceEvent>,
    _tears: mpsc::Receiver<rp::SoftBodyTearEvent>,
}

impl Default for Physics {
    fn default() -> Self {
        Self::new(Vec2::ZERO)
    }
}

fn to_rp(v: Vec2) -> rp::Vector {
    rp::Vector::new(v.x, v.y)
}
fn from_rp(v: rp::Vector) -> Vec2 {
    Vec2::new(v.x, v.y)
}

impl Physics {
    /// Physics pulling everything by `gravity` (pixels/s²; y is down, so
    /// `vec2(0.0, 980.0)` falls).
    #[must_use]
    pub fn new(gravity: Vec2) -> Self {
        let (collision_tx, collisions) = mpsc::channel();
        let (force_tx, forces) = mpsc::channel();
        let (tear_tx, tears) = mpsc::channel();
        let mut world = rp::PhysicsWorld::new();
        // Objects are tens of pixels, not about a meter: tells rapier's
        // tolerances what "small" means.
        world.integration_parameters.length_unit = 50.0;
        Self {
            world,
            links: Vec::new(),
            gravity,
            step: 0,
            paused: false,
            world_loads: 0,
            collisions,
            collector: rp::ChannelEventCollector::new(collision_tx, force_tx, tear_tx),
            _forces: forces,
            _tears: tears,
        }
    }

    /// The pull on every dynamic body.
    pub fn gravity(&self) -> Vec2 {
        self.gravity
    }
    /// Changes the pull on every dynamic body.
    pub fn set_gravity(&mut self, gravity: Vec2) {
        self.gravity = gravity;
    }

    /// Holds the physics step (`true`) or lets it run again (`false`),
    /// while every other simulation system keeps running: a pause screen
    /// whose own sim systems read the "resume" press. Unlike
    /// [`Time::pause`](crate::time::Time::pause), the tick counter goes on.
    ///
    /// While paused, [`step_physics`] does nothing: bodies keep their
    /// [`d2::Transform`](crate::d2::Transform) and [`Velocity`], no [`CollisionStarted`] or
    /// [`CollisionStopped`] events are sent, and ray and overlap queries
    /// see the bodies where the last step left them. Bodies spawned,
    /// changed or despawned meanwhile are picked up by the first step
    /// after the pause. Headless runs honour it like windowed ones. The
    /// flag belongs to this service, not the world: saves don't keep it and
    /// loading one doesn't clear it.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// fn pause_menu(ctx: &mut Ctx) {
    ///     if ctx.input.key_pressed(Key::Escape) {
    ///         let held = ctx.physics.is_paused();
    ///         ctx.physics.set_paused(!held);
    ///     }
    /// }
    ///
    /// let game = Game::new("Pause", 800, 600)
    ///     .physics(vec2(0.0, 980.0))
    ///     .sim_system(pause_menu);
    /// # let _ = game;
    /// ```
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// `true` while [`set_paused`](Self::set_paused) holds the step.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// How many bodies are simulated.
    pub fn body_count(&self) -> usize {
        self.world.bodies.len()
    }

    fn entity_of(&self, collider: rp::ColliderHandle) -> Option<Entity> {
        self.world
            .colliders
            .get(collider)
            .map(|c| Entity::from_bits(c.user_data as u64))
    }

    fn query_filter(&self, filter: QueryFilter) -> rp::QueryFilter<'static> {
        let mut result = rp::QueryFilter::default();
        if !filter.sensors {
            result = result.exclude_sensors();
        }
        if let Some(link) = filter.exclude.and_then(|entity| self.link(entity)) {
            result = result.exclude_rigid_body(link.body);
        }
        result
    }

    /// The first collider a ray from `origin` towards `direction` hits
    /// within `max_distance`, among those `filter` lets through (solid
    /// colliders by default; `QueryFilter::default().with_sensors()` for
    /// sensors too). Up to date as of the last physics step. A zero
    /// direction or a non-finite origin hits nothing, as in 3D.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// fn look_right(ctx: &mut Ctx) {
    ///     let filter = ponko::d2::QueryFilter::default();
    ///     if let Some(hit) = ctx.physics.cast_ray(Vec2::ZERO, Vec2::X, 500.0, filter) {
    ///         ctx.world.insert_resource(hit);
    ///     }
    /// }
    /// let world = Game::new("t", 1, 1).physics(Vec2::ZERO).sim_system(look_right).run_headless(2, |ctx| {
    ///     ctx.world.spawn((ponko::d2::Transform::at(vec2(100.0, 0.0)), ponko::d2::RigidBody::fixed(), ponko::d2::Collider::circle(10.0)));
    /// });
    /// // Queries see bodies once they have been stepped: the second tick hits.
    /// assert_eq!(world.resource::<ponko::d2::Hit>().distance, 90.0);
    /// ```
    pub fn cast_ray(&self, origin: Vec2, direction: Vec2, max_distance: f32, filter: QueryFilter) -> Option<RayHit> {
        let dir = direction.normalize_or_zero();
        if dir == Vec2::ZERO || !origin.is_finite() {
            return None;
        }
        let ray = rp::Ray::new(to_rp(origin), to_rp(dir));
        let (handle, hit) = self
            .world
            .cast_ray_and_get_normal(&ray, max_distance, true, self.query_filter(filter))?;
        Some(RayHit {
            entity: self.entity_of(handle)?,
            point: origin + dir * hit.time_of_impact,
            normal: from_rp(hit.normal),
            distance: hit.time_of_impact,
        })
    }

    /// Every collider overlapping a point; clears `out` first.
    pub fn overlap_point(&self, point: Vec2, filter: QueryFilter, out: &mut Vec<Entity>) {
        out.clear();
        for (handle, _) in self.world.intersect_point(to_rp(point), self.query_filter(filter)) {
            out.extend(self.entity_of(handle));
        }
    }

    /// Every collider overlapping a circle; clears `out` first. A radius
    /// below `f32::EPSILON` (or NaN) is treated as that tiny circle, as in 3D.
    pub fn overlap_circle(&self, center: Vec2, radius: f32, filter: QueryFilter, out: &mut Vec<Entity>) {
        out.clear();
        let ball = rp::Ball::new(radius.max(f32::EPSILON));
        let pose = rp::Pose::from_translation(to_rp(center));
        for (handle, _) in self.world.intersect_shape(pose, &ball, self.query_filter(filter)) {
            out.extend(self.entity_of(handle));
        }
    }

    /// Pushes a dynamic body once (an explosion, a kick), in mass ×
    /// pixels/s. Returns false if `e` isn't a physics body.
    pub fn apply_impulse(&mut self, e: Entity, impulse: Vec2) -> bool {
        let Some(link) = self.link(e) else { return false };
        let handle = link.body;
        match self.world.bodies.get_mut(handle) {
            Some(body) => {
                body.apply_impulse(to_rp(impulse), true);
                true
            }
            None => false,
        }
    }

    /// The mass of a body (from its collider's density and area).
    pub fn mass(&self, e: Entity) -> Option<f32> {
        let link = self.link(e)?;
        self.world.bodies.get(link.body).map(|b| b.mass())
    }

    fn link(&self, e: Entity) -> Option<&Link> {
        self.links.get(e.index() as usize)?.as_ref().filter(|l| l.entity == e)
    }

    fn insert(&mut self, e: Entity, t: &Transform2D, rigid: RigidBody, shape: Collider, velocity: Velocity) {
        let body = self.world.insert_body(body_of(t, rigid, velocity));
        self.world.insert_collider(collider_of(shape, e), Some(body));
        let i = e.index() as usize;
        if self.links.len() <= i {
            self.links.resize(i + 1, None);
        }
        self.links[i] = Some(Link {
            entity: e,
            body,
            rigid,
            shape,
            transform: *t,
            velocity,
            seen: self.step,
        });
    }

    fn remove(&mut self, i: usize) {
        if let Some(link) = self.links[i].take() {
            self.world.remove_body(link.body);
        }
    }

    /// Removes every body, keeping gravity and the step count: the world's
    /// entities were replaced (a scene or save was loaded), so ids may now
    /// name different entities and nothing of the old bodies (speed, sleep,
    /// contacts) may carry over. The next step builds bodies afresh from
    /// the components; until then queries find nothing.
    pub(crate) fn forget_bodies(&mut self) {
        for i in 0..self.links.len() {
            self.remove(i);
        }
    }
}

fn body_of(t: &Transform2D, rigid: RigidBody, velocity: Velocity) -> rp::RigidBody {
    let builder = match rigid.kind {
        BodyKind::Dynamic => rp::RigidBodyBuilder::dynamic(),
        BodyKind::Fixed => rp::RigidBodyBuilder::fixed(),
        BodyKind::Kinematic => rp::RigidBodyBuilder::kinematic_velocity_based(),
    };
    let mut builder = builder
        .pose(rp::Pose::new(to_rp(t.position), t.rotation))
        .linvel(to_rp(velocity.linear))
        .angvel(velocity.angular)
        .gravity_scale(rigid.gravity_scale)
        .linear_damping(rigid.linear_damping)
        .angular_damping(rigid.angular_damping)
        .ccd_enabled(rigid.ccd);
    if rigid.lock_rotation {
        builder = builder.lock_rotations();
    }
    builder.build()
}

fn collider_of(c: Collider, e: Entity) -> rp::Collider {
    let builder = match c.shape {
        Shape::Circle { radius } => rp::ColliderBuilder::ball(radius.max(f32::EPSILON)),
        Shape::Rect { size } => {
            rp::ColliderBuilder::cuboid((size.x * 0.5).max(f32::EPSILON), (size.y * 0.5).max(f32::EPSILON))
        }
        Shape::Capsule { height, radius } => {
            rp::ColliderBuilder::capsule_y((height * 0.5).max(0.0), radius.max(f32::EPSILON))
        }
        Shape::Polygon { .. } => {
            let points: Vec<rp::Vector> = c.shape.points().iter().map(|&p| to_rp(p)).collect();
            // All corners on one line: no area, so a sliver of a ball instead.
            rp::ColliderBuilder::convex_hull(&points).unwrap_or_else(|| rp::ColliderBuilder::ball(f32::EPSILON))
        }
    };
    builder
        .density(c.density)
        .friction(c.friction)
        .restitution(c.restitution)
        .sensor(c.sensor)
        .translation(to_rp(c.offset))
        .active_events(rp::ActiveEvents::COLLISION_EVENTS)
        .user_data(u128::from(e.to_bits()))
        .build()
}

/// Sets up `ctx.physics` for a game that uses physics, and makes its
/// components visible to the inspector, saves and agents.
pub(crate) fn start(ctx: &mut Ctx, gravity: Vec2) {
    use crate::reflect::WorldReflect;
    ctx.physics = Physics::new(gravity);
    ctx.world.reflect::<RigidBody>();
    ctx.world.reflect::<Collider>();
    ctx.world.reflect::<Velocity>();
}

/// Steps the physics world one simulation tick: picks up new, changed and
/// removed bodies, simulates, writes positions and velocities back, and
/// sends [`CollisionStarted`] / [`CollisionStopped`] events. Added by
/// [`Game::physics`](crate::game::Game::physics); remove or reorder it like any
/// other system. Does nothing while
/// [`ctx.physics.set_paused(true)`](Physics::set_paused) holds it.
pub fn step_physics(ctx: &mut Ctx) {
    if ctx.physics.paused {
        return;
    }
    let dt = ctx.time.dt();
    step(&mut ctx.physics, &mut ctx.world, &mut ctx.events, dt);
}

/// What [`step_physics`] uses, as a declaration: it writes the 2D physics
/// service ([`Service::Physics`](crate::access::Service::Physics)), reads
/// the timestep ([`Service::Time`](crate::access::Service::Time)), reads
/// [`RigidBody`], [`Collider`] and the count of world loads (a load drops
/// every body), writes [`d2::Transform`](crate::d2::Transform) and [`Velocity`], and sends
/// [`CollisionStarted`] and [`CollisionStopped`]. Nothing else: no other
/// resources, commands, drawing or input.
///
/// [`Game::physics`](crate::game::Game::physics) registers the step with this
/// declaration, so [`Game::access_report`](crate::game::Game::access_report) lists
/// what the step touches and debug builds check that it stays inside it.
/// Declare it ([`IntoSystem::access`](crate::game::IntoSystem::access)) when you
/// place the step yourself. The step keeps `&mut Ctx` and its place in
/// the order; see [`access`](crate::access).
///
/// ```
/// use ponko::prelude::*;
/// use ponko::{systems, prelude::Systems};
///
/// let game = Game::new("Order", 800, 600)
///     .physics(vec2(0.0, 980.0))
///     .remove(Systems::PHYSICS)
///     .add(Phase::BeforePhysics, systems::step_physics.access(systems::step_physics_access()));
/// # let _ = game;
/// ```
///
/// The [`Service`](crate::access::Service) entries of the declaration are
/// documentation: they appear in the report, but services are not recorded,
/// so nothing checks them at run time. Components, resources and events are
/// checked in debug builds.
#[must_use]
pub fn step_physics_access() -> crate::access::SystemAccess {
    use crate::access::{Service, SystemAccess};
    SystemAccess::new()
        .write_service(Service::Physics)
        .read_service(Service::Time)
        .write::<Transform2D>()
        .read::<RigidBody>()
        .read::<Collider>()
        .write::<Velocity>()
        .read_resource::<crate::reflect::WorldLoads>()
        .send_events::<CollisionStarted>()
        .send_events::<CollisionStopped>()
}

/// The step itself, with only what it touches: the physics state, the world
/// whose [`d2::Transform`](crate::d2::Transform) / [`RigidBody`] / [`Collider`] / [`Velocity`]
/// components it reads and writes, the event queue that gets the collision
/// events, and the timestep. No `Ctx`, window or device is needed, so tests
/// (and a future scheduler) can see and supply exactly these inputs.
fn step(physics: &mut Physics, world: &mut World, events: &mut Events, dt: f32) {
    physics.step += 1;
    sync_into(physics, world);
    physics.world.gravity = to_rp(physics.gravity);
    physics.world.integration_parameters.dt = dt;
    physics.world.step_with_events(&(), &physics.collector);
    write_back(physics, world);
    while let Ok(event) = physics.collisions.try_recv() {
        let (a, b, flags, started) = match event {
            rp::CollisionEvent::Started(a, b, f) => (a, b, f, true),
            rp::CollisionEvent::Stopped(a, b, f) => (a, b, f, false),
        };
        let (Some(a), Some(b)) = (physics.entity_of(a), physics.entity_of(b)) else {
            continue;
        };
        let sensor = flags.contains(rp::CollisionEventFlags::SENSOR);
        if started {
            events.send(CollisionStarted { a, b, sensor });
        } else {
            events.send(CollisionStopped { a, b, sensor });
        }
    }
}

/// New bodies in, your changes applied, gone ones out.
fn sync_into(physics: &mut Physics, world: &mut World) {
    let step = physics.step;
    let loads = crate::reflect::world_loads(world);
    if loads != physics.world_loads {
        physics.world_loads = loads;
        physics.forget_bodies();
    }
    world
        .query::<(Entity, &Transform2D, &RigidBody, &Collider, Option<&Velocity>)>()
        .for_each(|(e, t, rigid, shape, velocity)| {
            let i = e.index() as usize;
            let Some(link) = physics
                .links
                .get_mut(i)
                .and_then(Option::as_mut)
                .filter(|l| l.entity == e)
            else {
                if let Some(Some(old)) = physics.links.get(i).copied() {
                    // A recycled index: the old entity's body goes first.
                    physics.world.remove_body(old.body);
                    physics.links[i] = None;
                }
                physics.insert(e, t, *rigid, *shape, velocity.copied().unwrap_or_default());
                return;
            };
            link.seen = step;
            if link.rigid != *rigid || link.shape != *shape {
                // Rare (a door that becomes dynamic, a resized collider): rebuild.
                let current = link.velocity;
                physics.world.remove_body(link.body);
                physics.insert(e, t, *rigid, *shape, velocity.copied().unwrap_or(current));
                return;
            }
            // `bodies.get_mut` marks a body as changed (rapier then redoes its
            // broad phase and wakes it), so only touch bodies you changed.
            if link.transform != *t
                && let Some(body) = physics.world.bodies.get_mut(link.body)
            {
                body.set_position(rp::Pose::new(to_rp(t.position), t.rotation), true);
                link.transform = *t;
            }
            if let Some(&v) = velocity
                && link.velocity != v
                && let Some(body) = physics.world.bodies.get_mut(link.body)
            {
                body.set_linvel(to_rp(v.linear), true);
                body.set_angvel(v.angular, true);
                link.velocity = v;
            }
        });
    // Despawned entities and ones that lost a physics component, in index
    // order so runs stay deterministic.
    for i in 0..physics.links.len() {
        if physics.links[i].is_some_and(|l| l.seen != step) {
            physics.remove(i);
        }
    }
}

fn write_back(physics: &mut Physics, world: &mut World) {
    world
        .query::<(Entity, &mut Transform2D, Option<&mut Velocity>)>()
        .filter::<With<RigidBody>>()
        .filter::<With<Collider>>()
        .for_each(|(e, t, velocity)| {
            let Some(link) = physics
                .links
                .get_mut(e.index() as usize)
                .and_then(Option::as_mut)
                .filter(|l| l.entity == e)
            else {
                return;
            };
            if link.rigid.kind == BodyKind::Fixed {
                return;
            }
            // A sleeping body hasn't moved since it fell asleep.
            let Some(body) = physics.world.bodies.get(link.body).filter(|b| !b.is_sleeping()) else {
                return;
            };
            t.position = from_rp(body.translation());
            t.rotation = body.rotation().angle();
            link.transform = *t;
            let v = Velocity {
                linear: from_rp(body.linvel()),
                angular: body.angvel(),
            };
            link.velocity = v;
            if let Some(velocity) = velocity {
                *velocity = v;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec2;

    const DT: f32 = 1.0 / 60.0;
    const GRAVITY: Vec2 = vec2(0.0, 980.0);

    /// A floor and a column of balls that fall, roll and touch.
    fn pile(world: &mut World) -> Vec<Entity> {
        let mut entities = vec![world.spawn((
            Transform2D::at(vec2(0.0, 200.0)),
            RigidBody::fixed(),
            Collider::rect(vec2(400.0, 20.0)),
        ))];
        for i in 0..24 {
            let at = vec2((i % 5) as f32 * 9.0 - 20.0, -(i as f32) * 11.0);
            entities.push(world.spawn((Transform2D::at(at), RigidBody::dynamic(), Collider::circle(5.0))));
        }
        entities
    }

    /// Every body's pose and velocity, bit for bit.
    fn fingerprint(world: &World, entities: &[Entity]) -> Vec<[u32; 6]> {
        entities
            .iter()
            .map(|&e| {
                let t = world.get::<Transform2D>(e).copied().unwrap_or_default();
                let v = world.get::<Velocity>(e).copied().unwrap_or_default();
                [
                    t.position.x.to_bits(),
                    t.position.y.to_bits(),
                    t.rotation.to_bits(),
                    v.linear.x.to_bits(),
                    v.linear.y.to_bits(),
                    v.angular.to_bits(),
                ]
            })
            .collect()
    }

    #[test]
    fn the_core_step_needs_only_its_inputs_and_matches_the_ctx_system() {
        // The core, on a bare world: no Ctx, window or device.
        let mut physics = Physics::new(GRAVITY);
        let mut world = World::new();
        let mut events = Events::default();
        let bare = pile(&mut world);
        for _ in 0..120 {
            step(&mut physics, &mut world, &mut events, DT);
        }

        // The public system, through a Ctx.
        let mut ctx = Ctx::new(f64::from(DT), Vec2::ZERO);
        start(&mut ctx, GRAVITY);
        let wrapped = pile(&mut ctx.world);
        for _ in 0..120 {
            ctx.set_in_sim(true);
            step_physics(&mut ctx);
            ctx.set_in_sim(false);
        }

        assert_eq!(physics.step, 120);
        assert_eq!(fingerprint(&world, &bare), fingerprint(&ctx.world, &wrapped));
        let started = events.peek::<CollisionStarted>();
        assert!(!started.is_empty(), "the balls land");
        assert_eq!(started, ctx.events.peek::<CollisionStarted>());
        assert_eq!(events.peek::<CollisionStopped>(), ctx.events.peek::<CollisionStopped>());
    }

    #[test]
    fn the_core_step_reports_contacts_on_the_step_they_begin_and_handles_teleports_and_removals() {
        let mut physics = Physics::new(Vec2::ZERO);
        let mut world = World::new();
        let mut events = Events::default();
        let sensor = world.spawn((
            Transform2D::at(vec2(0.0, 0.0)),
            RigidBody::fixed(),
            Collider::circle(10.0).sensor(),
        ));
        let ball = world.spawn((
            Transform2D::at(vec2(100.0, 0.0)),
            RigidBody::dynamic(),
            Collider::circle(5.0),
        ));

        step(&mut physics, &mut world, &mut events, DT);
        assert!(events.peek::<CollisionStarted>().is_empty(), "apart");
        assert_eq!(physics.body_count(), 2);

        // A teleport into the sensor: picked up, and reported the same step.
        world.get_mut::<Transform2D>(ball).unwrap().position = vec2(2.0, 0.0);
        step(&mut physics, &mut world, &mut events, DT);
        assert_eq!(world.get::<Transform2D>(ball).unwrap().position, vec2(2.0, 0.0));
        let started = events.read::<CollisionStarted>();
        assert_eq!(started.len(), 1);
        let pair = [started[0].a, started[0].b];
        assert!(started[0].sensor && pair.contains(&ball) && pair.contains(&sensor));

        // A removed entity's body goes on the next step.
        world.despawn(ball);
        step(&mut physics, &mut world, &mut events, DT);
        assert_eq!(physics.body_count(), 1);
        assert!(events.read::<CollisionStarted>().is_empty());
    }
}
