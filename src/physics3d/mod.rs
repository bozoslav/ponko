//! 3D physics with rapier (feature `physics3d`): rigid bodies, colliders
//! (heightfield terrain and triangle meshes too), a character controller
//! and raycast-wheel vehicles as components, stepped inside the simulation
//! clock.
//!
//! [`d3::Transform`](crate::d3::Transform) stays the source of truth: set it and the body teleports;
//! moving bodies write their new pose back after every step, and between
//! steps (in windowed games) a pose blended between the last two steps, so
//! motion looks smooth at any frame rate.

use std::sync::Mutex;

use rapier3d::prelude as rp;
use serde::{Deserialize, Serialize};

use crate::collision::{BodyKind, CollisionStarted, CollisionStopped};
use crate::d3::Transform as Transform3D;
use crate::{ctx::Ctx, events::Events};
use glam::{Quat, Vec3};
use ponko_ecs::{Component, Entity, Without, World};

mod character;
mod contacts;
pub use contacts::{ContactModifier3D, ContactPair3D};
mod dominance;
mod files;
pub use files::CollisionMeshOptions;
mod joints;
pub use joints::{Joint3D, JointAxis3D, JointDrive3D, JointMotion3D};
mod query;
mod ragdoll;
mod streaming;
mod vehicle;
pub use ragdoll::{Ragdoll3D, RagdollBone};
pub(crate) use ragdoll::{blend_back, pose_ragdolls};
pub use streaming::{ColliderFocus, CollisionMeshData, TerrainCollider};

pub use character::Character3D;
pub use query::{Hit3D, QueryFilter3D};
pub use vehicle::{FrictionCurve, Vehicle3D, Wheel3D};

/// The simulation rate [`Game::physics3d`](crate::game::Game::physics3d) sets,
/// unless the game picked its own with
/// [`Game::fixed_hz`](crate::game::Game::fixed_hz): 50 steps per second.
pub(crate) const PHYSICS3D_HZ: u32 = 50;

/// Makes an entity a 3D physics body. It also needs a [`Collider3D`] and a
/// [`d3::Transform`](crate::d3::Transform).
///
/// ```
/// use ponko::prelude::*;
/// let crate_body = ponko::d3::RigidBody::dynamic();
/// let ground = ponko::d3::RigidBody::fixed();
/// assert_eq!(crate_body.gravity_scale, 1.0);
/// assert_eq!(ground.kind, ponko::collision::BodyKind::Fixed);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct RigidBody3D {
    /// Dynamic, fixed or kinematic.
    pub kind: BodyKind,
    /// Higher groups push lower groups without contact reaction; equal groups react normally.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::d3::RigidBody::dynamic().dominance(1).dominance, 1);
    /// ```
    pub dominance: i8,
    /// 1 normal, 0 floats, negative falls up.
    pub gravity_scale: f32,
    /// Slows movement down over time (air drag); 0 none.
    pub linear_damping: f32,
    /// Slows spinning down over time; 0 none.
    pub angular_damping: f32,
    /// Never rotates.
    pub lock_rotation: bool,
    /// Continuous collision detection: fast bodies (bullets) don't pass
    /// through thin walls. Costs a little more.
    pub ccd: bool,
    /// Its mass in kg, instead of its collider's density × volume (its
    /// inertia scales along). `None` (the default): from the density.
    pub mass: Option<f32>,
    /// Its center of mass in its own frame (relative to its
    /// [`d3::Transform`](crate::d3::Transform) position), instead of its collider's center. A car
    /// wants it low, so it doesn't tip over in turns. `None` (the
    /// default): the collider's.
    pub center_of_mass: Option<Vec3>,
}

impl Default for RigidBody3D {
    fn default() -> Self {
        Self::dynamic()
    }
}

impl RigidBody3D {
    /// Sets contact dominance (default zero). Does not change joint reactions.
    /// ```
    /// use ponko::prelude::*;
    /// let body = ponko::d3::RigidBody::dynamic().dominance(10);
    /// assert_eq!(body.dominance, 10);
    /// ```
    #[must_use]
    pub const fn dominance(self, dominance: i8) -> Self {
        Self { dominance, ..self }
    }

    /// A body moved by gravity and collisions.
    #[must_use]
    pub const fn dynamic() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            dominance: 0,
            gravity_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            lock_rotation: false,
            ccd: false,
            mass: None,
            center_of_mass: None,
        }
    }
    /// A body that never moves (ground, walls, terrain).
    #[must_use]
    pub const fn fixed() -> Self {
        Self {
            kind: BodyKind::Fixed,
            ..Self::dynamic()
        }
    }
    /// A body only you move, through its [`Velocity3D`] or [`d3::Transform`](crate::d3::Transform)
    /// (platforms, doors). It pushes dynamic bodies and isn't pushed back.
    #[must_use]
    pub const fn kinematic() -> Self {
        Self {
            kind: BodyKind::Kinematic,
            ..Self::dynamic()
        }
    }
    /// Weighing `kg`, whatever its collider's density.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let chassis = ponko::d3::RigidBody::dynamic().mass(1200.0).center_of_mass(vec3(0.0, -0.4, 0.0));
    /// assert_eq!(chassis.mass, Some(1200.0));
    /// ```
    #[must_use]
    pub const fn mass(self, kg: f32) -> Self {
        Self { mass: Some(kg), ..self }
    }
    /// With its center of mass at `center`, in its own frame.
    #[must_use]
    pub const fn center_of_mass(self, center: Vec3) -> Self {
        Self {
            center_of_mass: Some(center),
            ..self
        }
    }
}

/// A convex hull, triangle mesh or heightfield, built once with
/// [`Physics3D::convex_hull`], [`Physics3D::trimesh`],
/// [`Physics3D::heightfield`] or [`Physics3D::add_mesh`] and shared by any
/// number of [`Collider3D::mesh`] colliders and [`TerrainCollider`]s. A
/// [`Handle`](crate::handle::Handle): copying it doesn't copy the data; check it
/// with [`Physics3D::is_live`] (or `ctx.is_live`) and release it with
/// [`Physics3D::free`] (or `ctx.free`). Handles are only meaningful in the
/// run that made them, so serde never reads or writes one: in a
/// [`Shape3D`] or [`TerrainCollider`] it is saved as `null` and loaded as
/// [`CollisionMesh::NONE`](crate::handle::Handle::NONE). Scenes name the mesh file
/// instead (see [`Shape3D::Mesh`]).
///
/// ```compile_fail,E0277
/// // No handle from a number, through serde either.
/// let mesh: ponko::d3::CollisionMesh = serde_json::from_str(r#"{"id": 0}"#).unwrap();
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CollisionMesh {
    pub(crate) id: u32,
}

/// Serde for a [`CollisionMesh`] field: written as `null`, read back as
/// [`CollisionMesh::NONE`](crate::handle::Handle::NONE), so no saved number
/// becomes a handle.
pub(crate) mod unsaved_mesh {
    use super::CollisionMesh;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(_: &CollisionMesh, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_unit()
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CollisionMesh, D::Error> {
        <()>::deserialize(d).map(|()| <CollisionMesh as crate::handle::Handle>::NONE)
    }
}

/// A collider's shape, in world units (meters), centered on the entity
/// (plus [`Collider3D::offset`]) and turned with it. Scale isn't applied.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Shape3D {
    /// A ball.
    Sphere {
        /// Its radius.
        radius: f32,
    },
    /// A box `size` wide, high and deep.
    Cuboid {
        /// Full size along X, Y and Z.
        size: Vec3,
    },
    /// An upright capsule: `height` between the centers of its round ends.
    Capsule {
        /// Distance between the centers of the round ends.
        height: f32,
        /// Radius of the round ends.
        radius: f32,
    },
    /// A convex hull, triangle mesh or heightfield made by [`Physics3D`].
    /// Serde writes it as `{"Mesh": null}` and reads that back as
    /// [`CollisionMesh::NONE`](crate::handle::Handle::NONE): a handle only means
    /// something in the run that made it. In a scene, name the mesh file
    /// instead (`{"Mesh": {"file": "rock.glb"}}`); reflection leaves out
    /// colliders of meshes made in code.
    Mesh(#[serde(with = "unsaved_mesh")] CollisionMesh),
}

/// Which colliders touch which: a collider is a member of the groups in
/// `memberships` (one bit each, 32 groups) and touches colliders whose
/// memberships share a bit with its `filter`. Both sides must agree.
/// Queries use the same test against [`QueryFilter3D::groups`].
///
/// ```
/// use ponko::prelude::*;
/// const PLAYER: u32 = 1 << 0;
/// const DEBRIS: u32 = 1 << 1;
/// // Debris hits everything except other debris.
/// let debris = ponko::d3::CollisionGroups::new(DEBRIS, !DEBRIS);
/// assert!(!debris.touches(debris));
/// assert!(debris.touches(ponko::d3::CollisionGroups::new(PLAYER, u32::MAX)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct CollisionGroups {
    /// The groups this collider is in.
    pub memberships: u32,
    /// The groups it touches.
    pub filter: u32,
}

impl Default for CollisionGroups {
    fn default() -> Self {
        Self::ALL
    }
}

impl CollisionGroups {
    /// In every group, touching every group (the default).
    pub const ALL: Self = Self::new(u32::MAX, u32::MAX);

    /// Member of `memberships`, touching `filter`.
    #[must_use]
    pub const fn new(memberships: u32, filter: u32) -> Self {
        Self { memberships, filter }
    }

    /// Whether colliders with these groups and `other` touch.
    #[must_use]
    pub const fn touches(self, other: Self) -> bool {
        self.memberships & other.filter != 0 && other.memberships & self.filter != 0
    }

    fn to_rp(self) -> rp::InteractionGroups {
        rp::InteractionGroups::new(
            rp::Group::from_bits_retain(self.memberships),
            rp::Group::from_bits_retain(self.filter),
            rp::InteractionTestMode::And,
        )
    }
}

/// The solid shape of a 3D body, its surface (friction, bounciness) and
/// what it touches.
///
/// ```
/// use ponko::prelude::*;
/// let ball = ponko::d3::Collider::sphere(0.5).restitution(0.8);
/// let trigger = ponko::d3::Collider::cuboid(vec3(4.0, 2.0, 4.0)).sensor();
/// assert!(trigger.sensor);
/// assert_eq!(ball.friction, 0.5);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Component)]
pub struct Collider3D {
    /// Sphere, box, capsule or a mesh.
    pub shape: Shape3D,
    /// Mass per cubic meter (mass = density × volume). Default 1000, water.
    #[serde(default = "water")]
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
    /// Where the shape sits relative to the entity's position, in its frame.
    #[serde(default)]
    pub offset: Vec3,
    /// Which colliders it touches.
    #[serde(default)]
    pub groups: CollisionGroups,
}

// Collider defaults for fields missing from JSON, as in `Collider3D::of`.
fn water() -> f32 {
    1000.0
}
fn half() -> f32 {
    0.5
}

impl Collider3D {
    fn of(shape: Shape3D) -> Self {
        Self {
            shape,
            density: water(),
            friction: half(),
            restitution: 0.0,
            sensor: false,
            offset: Vec3::ZERO,
            groups: CollisionGroups::ALL,
        }
    }
    /// A ball.
    #[must_use]
    pub fn sphere(radius: f32) -> Self {
        Self::of(Shape3D::Sphere { radius })
    }
    /// A box `size` wide, high and deep.
    #[must_use]
    pub fn cuboid(size: Vec3) -> Self {
        Self::of(Shape3D::Cuboid { size })
    }
    /// An upright capsule: `height` between the centers of its round ends
    /// (so `height + 2 × radius` tall).
    #[must_use]
    pub fn capsule(height: f32, radius: f32) -> Self {
        Self::of(Shape3D::Capsule { height, radius })
    }
    /// A convex hull, triangle mesh or heightfield made by [`Physics3D`].
    ///
    /// ```
    /// use ponko::prelude::*;
    /// Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(1, |ctx| {
    ///     let ground = ctx.physics3d.heightfield(&[0.0; 3 * 3], 10.0);
    ///     ctx.world.spawn((ponko::d3::Transform::default(), ponko::d3::RigidBody::fixed(), ponko::d3::Collider::mesh(ground)));
    /// });
    /// ```
    #[must_use]
    pub fn mesh(mesh: CollisionMesh) -> Self {
        Self::of(Shape3D::Mesh(mesh))
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
    pub fn offset(self, offset: Vec3) -> Self {
        Self { offset, ..self }
    }
    /// In these collision groups.
    #[must_use]
    pub fn groups(self, groups: CollisionGroups) -> Self {
        Self { groups, ..self }
    }
}

/// A 3D body's speed: meters per second, and radians per second around
/// each axis. Read it after a step; write it to set the speed.
///
/// ```
/// use ponko::prelude::*;
/// let thrown = ponko::d3::Velocity::linear(vec3(0.0, 4.0, -10.0));
/// assert_eq!(thrown.angular, Vec3::ZERO);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Component)]
#[serde(default)]
#[repr(C)] // C sees it as PonkoVelocity3D.
pub struct Velocity3D {
    /// Meters per second.
    pub linear: Vec3,
    /// Radians per second around X, Y and Z.
    pub angular: Vec3,
}

impl Velocity3D {
    /// Moving at `linear`, not spinning.
    #[must_use]
    pub const fn linear(linear: Vec3) -> Self {
        Self {
            linear,
            angular: Vec3::ZERO,
        }
    }
}

/// A position and rotation, for blending between steps.
#[derive(Clone, Copy, PartialEq)]
struct Pose {
    position: Vec3,
    rotation: Quat,
}

impl Pose {
    fn of(t: &Transform3D) -> Self {
        Self {
            position: t.position,
            rotation: t.rotation,
        }
    }
}

/// What the world last knew of one entity, to notice your changes.
#[derive(Clone, Copy)]
struct Link {
    entity: Entity,
    body: rp::RigidBodyHandle,
    /// Its (one) collider.
    collider: rp::ColliderHandle,
    /// Whether the collider is in the physics world (only static ones are
    /// ever left out, by collider streaming).
    active: bool,
    /// Where a static collider lies on the ground, for collider streaming.
    rect: streaming::Rect,
    kind: LinkKind,
    velocity: Velocity3D,
    /// The transform as last written or seen: anything else is your change.
    written: Transform3D,
    /// The pose after the step before the last one, and after the last one.
    prev: Pose,
    curr: Pose,
    seen: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum LinkKind {
    Body(RigidBody3D, Collider3D),
    Character(character::Capsule),
    /// A terrain collider, and the scale of its transform.
    Terrain(TerrainCollider, Vec3),
}

impl LinkKind {
    fn moves(self) -> bool {
        match self {
            LinkKind::Body(rigid, _) => rigid.kind != BodyKind::Fixed,
            LinkKind::Character(_) => true,
            LinkKind::Terrain(..) => false,
        }
    }
}

struct MeshSlot {
    shape: Option<rp::SharedShape>,
    /// From the entity's position to the shape's center (a heightfield is
    /// placed by its corner, rapier's by its center).
    offset: Vec3,
}

/// Collects collision events during a step, into a buffer that's reused.
#[derive(Default)]
struct Collector(Mutex<Vec<rp::CollisionEvent>>);

impl rp::EventHandler for Collector {
    fn handle_collision_event(
        &self,
        _: &rp::RigidBodySet,
        _: &rp::ColliderSet,
        event: rp::CollisionEvent,
        _: Option<&rp::ContactPair>,
    ) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
    fn handle_contact_force_event(
        &self,
        _: f32,
        _: &rp::RigidBodySet,
        _: &rp::ColliderSet,
        _: &rp::ContactPair,
        _: f32,
    ) {
    }
    fn handle_soft_body_tear_event(&self, _: &rp::SoftBodySet, _: &rp::SoftBodyTearEvent) {}
}

/// The 3D physics world: `ctx.physics3d`. Add it to a game with
/// [`Game::physics3d`](crate::game::Game::physics3d); then entities with
/// [`RigidBody3D`], [`Collider3D`] and [`d3::Transform`](crate::d3::Transform) are simulated, and
/// entities with [`Character3D`] and [`d3::Transform`](crate::d3::Transform) walk.
pub struct Physics3D {
    world: rp::PhysicsWorld,
    joints: Vec<joints::Link>,
    /// By entity index.
    links: Vec<Option<Link>>,
    /// The indices in `links` of bodies and characters that move, so
    /// stepping and drawing visit them without looking at every entity.
    /// Rebuilt when a link comes or goes (`moving_dirty`).
    moving: Vec<u32>,
    moving_dirty: bool,
    /// The indices in `links` of fixed bodies and terrain, rebuilt with `moving`.
    statics: Vec<u32>,
    /// Collider streaming: at most this many static colliders come in per step.
    streaming: Option<usize>,
    /// Whether streaming may have left colliders out.
    streamed_out: bool,
    stream_scratch: streaming::Scratch,
    /// Whether some dynamic body has a non-zero dominance group, rebuilt
    /// with `moving`. Without one, no contact can need a dominance
    /// velocity transfer, so the pass over every contact pair is skipped.
    dominance: bool,
    gravity: Vec3,
    step: u64,
    /// From `set_paused`: the built-in step does nothing while set.
    paused: bool,
    /// The world's load count when `links` were made (see `forget_bodies`).
    world_loads: u64,
    collector: Collector,
    /// Events taken from the collector, kept to reuse its memory.
    events: Vec<rp::CollisionEvent>,
    meshes: Vec<MeshSlot>,
    /// The id each mesh slot was last given out as, and the freed ones.
    mesh_ids: crate::handle::SlotIds,
    /// Reused by the character controller.
    character: character::Scratch,
    /// Reused when posing ragdolls.
    pub(crate) ragdolls: Vec<Entity>,
    /// Contact modifiers by entity index, this step's list of them, and the
    /// list from the step before (kept to reuse its memory).
    contact_table: Vec<Option<(Entity, ContactModifier3D)>>,
    contact_hooked: Vec<(Entity, ContactModifier3D)>,
    contact_spare: Vec<(Entity, ContactModifier3D)>,
    contact_hook: Option<contacts::ContactHook>,
}

impl Default for Physics3D {
    fn default() -> Self {
        Self::new(Vec3::ZERO)
    }
}

pub(crate) fn to_rp(v: Vec3) -> rp::Vector {
    rp::Vector::new(v.x, v.y, v.z)
}
pub(crate) fn from_rp(v: rp::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}
fn rot_to_rp(q: Quat) -> rp::Rotation {
    let q = q.normalize();
    if q.is_finite() {
        rp::Rotation::from_xyzw(q.x, q.y, q.z, q.w)
    } else {
        rp::Rotation::IDENTITY
    }
}
fn rot_from_rp(q: rp::Rotation) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w)
}
pub(crate) fn pose_to_rp(position: Vec3, rotation: Quat) -> rp::Pose {
    rp::Pose::from_parts(to_rp(position), rot_to_rp(rotation))
}

impl Physics3D {
    /// The simulation rate [`Game::physics3d`](crate::game::Game::physics3d)
    /// sets, unless the game picked its own with
    /// [`Game::fixed_hz`](crate::game::Game::fixed_hz): 50 steps per second.
    ///
    /// ```
    /// assert_eq!(ponko::d3::Physics::HZ, 50);
    /// ```
    pub const HZ: u32 = PHYSICS3D_HZ;

    /// Physics pulling everything by `gravity` (meters/s²; `vec3(0.0, -9.81, 0.0)`
    /// is Earth's).
    #[must_use]
    pub fn new(gravity: Vec3) -> Self {
        Self {
            world: rp::PhysicsWorld::new(),
            joints: Vec::new(),
            links: Vec::new(),
            moving: Vec::new(),
            moving_dirty: false,
            statics: Vec::new(),
            streaming: None,
            streamed_out: false,
            stream_scratch: streaming::Scratch::default(),
            dominance: false,
            gravity,
            step: 0,
            paused: false,
            world_loads: 0,
            collector: Collector::default(),
            events: Vec::new(),
            meshes: Vec::new(),
            mesh_ids: crate::handle::SlotIds::default(),
            character: character::Scratch::default(),
            ragdolls: Vec::new(),
            contact_table: Vec::new(),
            contact_hooked: Vec::new(),
            contact_spare: Vec::new(),
            contact_hook: None,
        }
    }

    /// Calls `hook` for the contacts of every body whose
    /// [`ContactModifier3D`] has `hook` set, each step, before the solver
    /// uses them: it can drop contacts or points, change friction and
    /// bounciness, or make a surface carry things along. It runs inside the
    /// physics step, so it sees only the [`ContactPair3D`]: capture what
    /// else it needs. Replaces an earlier hook.
    /// ```
    /// use ponko::prelude::*;
    /// let mut physics = ponko::d3::Physics::new(Vec3::ZERO);
    /// // Ice: anything touching a hooked body slides.
    /// physics.set_contact_hook(|pair| pair.set_friction(0.0));
    /// physics.clear_contact_hook();
    /// ```
    pub fn set_contact_hook(&mut self, hook: impl Fn(&mut ContactPair3D) + Send + Sync + 'static) {
        self.contact_hook = Some(Box::new(hook));
    }

    /// Removes the custom contact hook.
    pub fn clear_contact_hook(&mut self) {
        self.contact_hook = None;
    }

    /// The pull on every dynamic body and character.
    pub fn gravity(&self) -> Vec3 {
        self.gravity
    }
    /// Changes the pull on every dynamic body and character.
    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
    }

    /// Holds the 3D physics step (`true`) or lets it run again (`false`),
    /// while every other simulation system keeps running: a pause screen
    /// whose own sim systems read the "resume" press. The same call as the
    /// 2D `ctx.physics.set_paused`, on the 3D world. Unlike [`Time::pause`](crate::time::Time::pause), the tick
    /// counter goes on.
    ///
    /// While paused, [`step_physics3d`] does nothing: bodies keep their
    /// [`d3::Transform`](crate::d3::Transform) and [`Velocity3D`],
    /// characters and vehicles don't move (a [`Character3D`]'s velocity is
    /// kept, so a jump goes on where it stopped), joints and ragdolls hold,
    /// no [`CollisionStarted`] or [`CollisionStopped`] events are sent and no
    /// contact hook runs, and ray, shape and overlap queries see the bodies
    /// where the last step left them. Bodies spawned, changed or despawned
    /// meanwhile are picked up by the first step after the pause. In a
    /// window, [`interpolate_physics3d`] draws bodies at their last pose
    /// instead of blending. Headless runs honour it like windowed ones. The
    /// flag belongs to this service, not the world: saves don't keep it and
    /// loading one doesn't clear it.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// fn pause_menu(ctx: &mut Ctx) {
    ///     if ctx.input.key_pressed(Key::Escape) {
    ///         let held = ctx.physics3d.is_paused();
    ///         ctx.physics3d.set_paused(!held);
    ///     }
    /// }
    ///
    /// let game = Game::new("Pause", 800, 600)
    ///     .physics3d(vec3(0.0, -9.81, 0.0))
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

    /// How many bodies are simulated (characters included).
    pub fn body_count(&self) -> usize {
        self.world.bodies.len()
    }

    fn entity_of(&self, collider: rp::ColliderHandle) -> Option<Entity> {
        self.world
            .colliders
            .get(collider)
            .map(|c| Entity::from_bits(c.user_data as u64))
    }

    fn link(&self, e: Entity) -> Option<&Link> {
        self.links.get(e.index() as usize)?.as_ref().filter(|l| l.entity == e)
    }

    /// Pushes a dynamic body once (an explosion, a kick), in kg × m/s at its
    /// center of mass. Returns false if `e` isn't a physics body (yet:
    /// bodies join at the next physics step).
    pub fn apply_impulse(&mut self, e: Entity, impulse: Vec3) -> bool {
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

    /// Pushes a dynamic body once at a point in the world (a bullet's hit),
    /// so it spins too. Returns false if `e` isn't a physics body.
    pub fn apply_impulse_at(&mut self, e: Entity, impulse: Vec3, point: Vec3) -> bool {
        let Some(link) = self.link(e) else { return false };
        let handle = link.body;
        match self.world.bodies.get_mut(handle) {
            Some(body) => {
                body.apply_impulse_at_point(to_rp(impulse), to_rp(point), true);
                true
            }
            None => false,
        }
    }

    /// The mass of a body in kg (from its collider's density and volume,
    /// or [`RigidBody3D::mass`]).
    pub fn mass(&self, e: Entity) -> Option<f32> {
        let link = self.link(e)?;
        self.world.bodies.get(link.body).map(|b| b.mass())
    }

    /// A body's center of mass in the world, as of the last physics step
    /// (see [`RigidBody3D::center_of_mass`]).
    pub fn center_of_mass(&self, e: Entity) -> Option<Vec3> {
        let link = self.link(e)?;
        self.world.bodies.get(link.body).map(|b| from_rp(b.center_of_mass()))
    }

    /// Where a body is as of the last physics step: what [`d3::Transform`](crate::d3::Transform)
    /// holds right after the step (between steps it holds a blend of the
    /// last two, for drawing).
    pub fn position(&self, e: Entity) -> Option<Vec3> {
        self.link(e).map(|l| l.curr.position)
    }

    /// A convex hull around `points` (rocks, crates, vehicle bodies), for
    /// [`Collider3D::mesh`]. `None` if the points are all on one plane or
    /// not finite.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut physics = ponko::d3::Physics::default();
    /// let corners = [vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0)];
    /// assert!(physics.convex_hull(&corners).is_some());
    /// assert!(physics.convex_hull(&corners[..3]).is_none(), "flat");
    /// ```
    pub fn convex_hull(&mut self, points: &[Vec3]) -> Option<CollisionMesh> {
        Some(self.add_mesh(&CollisionMeshData::convex_hull(points)?))
    }

    /// A triangle mesh (buildings, rocks, any static level geometry), for
    /// [`Collider3D::mesh`]: `indices` are three per triangle into
    /// `vertices`. Best on fixed bodies: a dynamic one gets the mass of its
    /// bounding box.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut physics = ponko::d3::Physics::default();
    /// let floor = physics.trimesh(
    ///     &[vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 1.0)],
    ///     &[[0, 2, 1], [1, 2, 3]],
    /// );
    /// assert!(floor.is_some());
    /// ```
    ///
    /// `None` if there are no triangles, an index is out of range or a
    /// vertex isn't finite.
    pub fn trimesh(&mut self, vertices: &[Vec3], indices: &[[u32; 3]]) -> Option<CollisionMesh> {
        Some(self.add_mesh(&CollisionMeshData::trimesh(vertices, indices)?))
    }

    /// Ground for a terrain tile, from the same heights and size as
    /// [`Graphics::create_terrain_tile`](crate::gfx::Graphics::create_terrain_tile) (n × n
    /// heights, row by row along +Z, each row along +X; NaN for a hole), so
    /// the collider matches the drawn ground triangle for triangle. Give it
    /// to a [`TerrainCollider`] on the tile's entity (or a fixed body at the
    /// tile's [`d3::Transform`](crate::d3::Transform): its corner). [`CollisionMeshData::heightfield`]
    /// builds it on another thread.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut physics = ponko::d3::Physics::default();
    /// let heights = [0.0, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0];
    /// let ground = physics.heightfield(&heights, 8.0);
    /// # let _ = ground;
    /// ```
    ///
    /// # Panics
    ///
    /// If `heights` isn't a square grid of at least 2 × 2 or `size` isn't above 0.
    #[track_caller]
    pub fn heightfield(&mut self, heights: &[f32], size: f32) -> CollisionMesh {
        self.add_mesh(&CollisionMeshData::heightfield_named(
            heights,
            size,
            "Physics3D::heightfield",
        ))
    }

    /// Lets go of a mesh made here, as [`Ctx::free`](crate::ctx::Ctx::free)
    /// does. Colliders already using it keep it until they're removed; new
    /// colliders with this handle get no shape. Its slot goes to the next
    /// mesh made, under a new handle, so this one stays dead. Freeing it
    /// again, or freeing [`Handle::NONE`](crate::handle::Handle::NONE), does nothing.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut physics = ponko::d3::Physics::default();
    /// let ground = physics.heightfield(&[0.0; 4], 2.0);
    /// assert!(physics.is_live(ground));
    /// physics.free(ground);
    /// assert!(!physics.is_live(ground));
    /// let next = physics.heightfield(&[0.0; 4], 2.0);
    /// assert!(physics.is_live(next) && next != ground);
    /// ```
    pub fn free(&mut self, mesh: CollisionMesh) {
        if let Some(slot) = self.mesh_ids.slot(mesh.id)
            && let Some(s) = self.meshes.get_mut(slot)
            && s.shape.take().is_some()
        {
            self.mesh_ids.release(mesh.id);
        }
    }

    /// Whether `mesh` is a live mesh of this world, as
    /// [`Ctx::is_live`](crate::ctx::Ctx::is_live) tells. False for
    /// [`Handle::NONE`](crate::handle::Handle::NONE).
    #[must_use]
    pub fn is_live(&self, mesh: CollisionMesh) -> bool {
        self.mesh(mesh).is_some()
    }

    /// How many collision meshes are live.
    pub(crate) fn live_count(&self) -> usize {
        self.meshes.iter().filter(|s| s.shape.is_some()).count()
    }

    fn mesh(&self, mesh: CollisionMesh) -> Option<&MeshSlot> {
        let slot = self.mesh_ids.slot(mesh.id)?;
        self.meshes.get(slot).filter(|s| s.shape.is_some())
    }

    fn add_shape(&mut self, shape: rp::SharedShape, offset: Vec3) -> CollisionMesh {
        let id = self.mesh_ids.make(self.meshes.len(), "collision mesh");
        let slot = (id & crate::handle::INDEX_MASK) as usize;
        let made = MeshSlot {
            shape: Some(shape),
            offset,
        };
        if slot == self.meshes.len() {
            self.meshes.push(made);
        } else {
            self.meshes[slot] = made;
        }
        CollisionMesh { id }
    }

    /// A rapier collider for `c` (a speck touching nothing for a freed mesh).
    fn collider_of(&self, c: &Collider3D, e: Entity) -> rp::Collider {
        self.scaled_collider_of(c, e, Vec3::ONE)
    }

    /// A rapier collider for a terrain collider, scaled with its tile.
    fn terrain_collider_of(&self, t: &TerrainCollider, scale: Vec3, e: Entity) -> rp::Collider {
        let c = Collider3D {
            friction: t.friction,
            restitution: t.restitution,
            groups: t.groups,
            ..Collider3D::mesh(t.mesh)
        };
        self.scaled_collider_of(&c, e, scale)
    }

    /// [`collider_of`](Self::collider_of) with its mesh scaled by `scale`.
    fn scaled_collider_of(&self, c: &Collider3D, e: Entity, scale: Vec3) -> rp::Collider {
        let tiny = f32::EPSILON;
        let (shape, offset) = match c.shape {
            Shape3D::Sphere { radius } => (rp::SharedShape::ball(radius.max(tiny)), Vec3::ZERO),
            Shape3D::Cuboid { size } => {
                let h = (size * 0.5).max(Vec3::splat(tiny));
                (rp::SharedShape::cuboid(h.x, h.y, h.z), Vec3::ZERO)
            }
            Shape3D::Capsule { height, radius } => (
                rp::SharedShape::capsule_y((height * 0.5).max(0.0), radius.max(tiny)),
                Vec3::ZERO,
            ),
            Shape3D::Mesh(m) => match self.mesh(m) {
                Some(slot) if scale == Vec3::ONE => (slot.shape.clone().expect("live slot"), slot.offset),
                Some(slot) => (
                    streaming::scaled(slot.shape.as_ref().expect("live slot"), scale),
                    slot.offset * scale.abs(),
                ),
                // A freed mesh: a speck that touches nothing.
                None => (rp::SharedShape::ball(tiny), Vec3::ZERO),
            },
        };
        let mut types = rp::ActiveCollisionTypes::default();
        if c.sensor {
            // Characters are kinematic: let them set off fixed triggers.
            types |= rp::ActiveCollisionTypes::KINEMATIC_FIXED | rp::ActiveCollisionTypes::KINEMATIC_KINEMATIC;
        }
        let groups = match c.shape {
            Shape3D::Mesh(m) if self.mesh(m).is_none() => CollisionGroups::new(0, 0),
            _ => c.groups,
        };
        rp::ColliderBuilder::new(shape)
            .density(c.density)
            .friction(c.friction)
            .restitution(c.restitution)
            .sensor(c.sensor)
            .translation(to_rp(c.offset + offset))
            .collision_groups(groups.to_rp())
            .active_collision_types(types)
            .active_events(rp::ActiveEvents::COLLISION_EVENTS)
            .user_data(u128::from(e.to_bits()))
            .build()
    }

    /// The indices of links that move, rebuilt if links changed.
    fn moving(&mut self) -> &[u32] {
        if self.moving_dirty {
            self.moving_dirty = false;
            self.moving.clear();
            self.statics.clear();
            let links = &self.links;
            self.moving
                .extend((0..links.len() as u32).filter(|&i| links[i as usize].is_some_and(|l| l.kind.moves())));
            self.statics
                .extend((0..links.len() as u32).filter(|&i| links[i as usize].is_some_and(|l| !l.kind.moves())));
            self.dominance = links.iter().flatten().any(|l| {
                matches!(l.kind, LinkKind::Body(rigid, _) if rigid.kind == BodyKind::Dynamic && rigid.dominance != 0)
            });
        }
        &self.moving
    }

    /// See [`Physics3D::dominance`](Self::dominance).
    fn uses_dominance(&mut self) -> bool {
        self.moving();
        self.dominance
    }

    /// Puts `e` in the physics world. With collider streaming on, a static
    /// collider starts out (the streaming pass brings it in when it's near).
    fn insert_link(&mut self, e: Entity, t: &Transform3D, kind: LinkKind, velocity: Velocity3D) {
        self.moving_dirty = true;
        let active = kind.moves() || self.streaming.is_none();
        let (body, collider) = match kind {
            LinkKind::Body(rigid, shape) => {
                let body = self.world.insert_body(body_of(t, rigid, velocity));
                let mut collider = self.collider_of(&shape, e);
                set_mass(&mut collider, rigid);
                collider.set_enabled(active);
                (body, self.world.insert_collider(collider, Some(body)))
            }
            LinkKind::Character(capsule) => {
                let body = self.world.insert_body(
                    rp::RigidBodyBuilder::kinematic_position_based()
                        .pose(pose_to_rp(t.position, Quat::IDENTITY))
                        .build(),
                );
                (body, self.world.insert_collider(capsule.collider(e), Some(body)))
            }
            LinkKind::Terrain(terrain, scale) => {
                let body = self
                    .world
                    .insert_body(body_of(t, RigidBody3D::fixed(), Velocity3D::default()));
                let mut collider = self.terrain_collider_of(&terrain, scale, e);
                collider.set_enabled(active);
                (body, self.world.insert_collider(collider, Some(body)))
            }
        };
        if !active {
            self.streamed_out = true;
        }
        let rect = if kind.moves() {
            [0.0; 4]
        } else {
            self.rect_of(collider, t)
        };
        let i = e.index() as usize;
        if self.links.len() <= i {
            self.links.resize(i + 1, None);
        }
        let pose = Pose::of(t);
        self.links[i] = Some(Link {
            entity: e,
            body,
            collider,
            active,
            rect,
            kind,
            velocity,
            written: *t,
            prev: pose,
            curr: pose,
            seen: self.step,
        });
    }

    fn remove(&mut self, i: usize) {
        self.moving_dirty = true;
        if let Some(link) = self.links[i].take() {
            self.world.remove_body(link.body);
        }
    }

    /// Removes every body and character (and with them their joints),
    /// keeping gravity, collision meshes, hooks and settings: the world's
    /// entities were replaced (a scene or save was loaded), so ids may now
    /// name different entities and nothing of the old bodies may carry
    /// over. The next step builds them afresh from the components; until
    /// then queries find nothing.
    pub(crate) fn forget_bodies(&mut self) {
        joints::forget(self);
        for i in 0..self.links.len() {
            self.remove(i);
        }
    }
}

/// The body's own mass and center of mass, if it has them, on its collider
/// (a collider's mass properties are in its own frame, shifted by its offset).
fn set_mass(collider: &mut rp::Collider, rigid: RigidBody3D) {
    let mass = rigid.mass.filter(|m| *m > 0.0 && m.is_finite());
    let center = rigid.center_of_mass.filter(|c| c.is_finite());
    if mass.is_none() && center.is_none() {
        return;
    }
    let mut props = collider.mass_properties();
    if let Some(mass) = mass {
        if props.mass() > 0.0 {
            props.set_mass(mass, true);
        } else {
            props = rp::MassProperties::new(props.local_com, mass, rp::Vector::splat(mass * 0.1));
        }
    }
    if let Some(center) = center {
        props.local_com = to_rp(center) - collider.position().translation;
    }
    collider.set_mass_properties(props);
}

fn body_of(t: &Transform3D, rigid: RigidBody3D, velocity: Velocity3D) -> rp::RigidBody {
    let builder = match rigid.kind {
        BodyKind::Fixed => rp::RigidBodyBuilder::fixed(),
        BodyKind::Kinematic => rp::RigidBodyBuilder::kinematic_velocity_based(),
        _ => rp::RigidBodyBuilder::dynamic(),
    };
    let mut builder = builder
        .pose(pose_to_rp(t.position, t.rotation))
        .linvel(to_rp(velocity.linear))
        .angvel(to_rp(velocity.angular))
        .dominance_group(rigid.dominance)
        .gravity_scale(rigid.gravity_scale)
        .linear_damping(rigid.linear_damping)
        .angular_damping(rigid.angular_damping)
        .ccd_enabled(rigid.ccd);
    if rigid.lock_rotation {
        builder = builder.lock_rotations();
    }
    builder.build()
}

/// Sets up `ctx.physics3d` for a game that uses 3D physics, and makes its
/// components visible to the inspector, saves and agents.
pub(crate) fn start(ctx: &mut Ctx, gravity: Vec3) {
    use crate::reflect::WorldReflect;
    ctx.physics3d = Physics3D::new(gravity);
    ctx.world.reflect::<RigidBody3D>();
    crate::reflect::reflect_collider(&mut ctx.world);
    ctx.world.reflect::<Velocity3D>();
    ctx.world.reflect::<Character3D>();
    ctx.world.reflect::<Vehicle3D>();
    ctx.world.reflect::<ColliderFocus>();
    ctx.world.reflect::<Joint3D>();
}

/// Steps the 3D physics world one simulation tick: picks up new, changed
/// and removed bodies, moves characters, simulates, writes poses and
/// velocities back, and sends [`CollisionStarted`] / [`CollisionStopped`]
/// events. Added by [`Game::physics3d`](crate::game::Game::physics3d); remove or
/// reorder it like any other system.
///
/// Does nothing while [`ctx.physics3d.set_paused(true)`](Physics3D::set_paused)
/// holds it.
pub fn step_physics3d(ctx: &mut Ctx) {
    if ctx.physics3d.paused {
        return;
    }
    let dt = ctx.time.dt();
    step(&mut ctx.physics3d, &mut ctx.world, &mut ctx.events, dt);
}

/// What [`step_physics3d`] uses, as a declaration: it writes the 3D physics
/// service ([`Service::Physics3D`](crate::access::Service::Physics3D)), reads
/// the timestep ([`Service::Time`](crate::access::Service::Time)), reads the
/// body, collider, joint, contact-modifier, terrain and streaming-focus
/// components and the count of world loads (a load drops every body), writes
/// [`d3::Transform`](crate::d3::Transform), [`Velocity3D`], [`Character3D`], [`Vehicle3D`],
/// [`Ragdoll3D`] and the ragdolls' skeletons, and sends [`CollisionStarted`]
/// and [`CollisionStopped`]. Nothing else: no other resources, commands,
/// drawing or input.
///
/// [`Game::physics3d`](crate::game::Game::physics3d) registers the step with this
/// declaration, so [`Game::access_report`](crate::game::Game::access_report) lists
/// what the step touches and debug builds check that it stays inside it.
/// Declare it ([`IntoSystem::access`](crate::game::IntoSystem::access)) when you
/// place the step yourself. The step keeps `&mut Ctx` and its place in
/// the order; see [`access`](crate::access).
///
/// The [`Service`](crate::access::Service) entries of the declaration are
/// documentation: they appear in the report, but services are not recorded,
/// so nothing checks them at run time. Components, resources and events are
/// checked in debug builds.
#[must_use]
pub fn step_physics3d_access() -> crate::access::SystemAccess {
    use crate::access::{Service, SystemAccess};
    SystemAccess::new()
        .write_service(Service::Physics3D)
        .read_service(Service::Time)
        .write::<Transform3D>()
        .read::<RigidBody3D>()
        .read::<Collider3D>()
        .write::<Velocity3D>()
        .write::<Character3D>()
        .write::<Vehicle3D>()
        .read::<Joint3D>()
        .read::<ContactModifier3D>()
        .read::<TerrainCollider>()
        .read::<ColliderFocus>()
        .read_resource::<crate::reflect::WorldLoads>()
        .write::<Ragdoll3D>()
        .write::<crate::anim::Skeleton>()
        .send_events::<CollisionStarted>()
        .send_events::<CollisionStopped>()
}

/// What [`interpolate_physics3d`] uses, as a declaration: it writes the 3D
/// physics service (the poses it last wrote), reads the frame's place
/// between two steps ([`Service::Time`](crate::access::Service::Time)) and
/// writes [`d3::Transform`](crate::d3::Transform). [`Game::physics3d`](crate::game::Game::physics3d)
/// registers the blend with it.
///
/// The [`Service`](crate::access::Service) entries of the declaration are
/// documentation: they appear in the report, but services are not recorded,
/// so nothing checks them at run time. Components, resources and events are
/// checked in debug builds.
#[must_use]
pub fn interpolate_physics3d_access() -> crate::access::SystemAccess {
    use crate::access::{Service, SystemAccess};
    SystemAccess::new()
        .write_service(Service::Physics3D)
        .read_service(Service::Time)
        .write::<Transform3D>()
}

/// The step itself, with only what it touches: the physics state (bodies,
/// joints, contact rules, ragdolls), the world whose body, character,
/// vehicle, joint and terrain components it reads and writes, the event
/// queue that gets the collision events, and the timestep. No `Ctx`, window
/// or device is needed, so tests (and a future scheduler) can see and
/// supply exactly these inputs.
fn step(physics: &mut Physics3D, world: &mut World, events: &mut Events, dt: f32) {
    physics.step += 1;
    sync_into(physics, world);
    streaming::stream(physics, world);
    joints::sync(physics, world);
    contacts::sync(physics, world);
    physics.world.gravity = to_rp(physics.gravity);
    physics.world.integration_parameters.dt = dt;
    character::move_characters(physics, world, dt);
    vehicle::step_vehicles(physics, world, dt);
    let hooks = contacts::Hooks {
        table: &physics.contact_table,
        custom: physics.contact_hook.as_deref(),
    };
    physics.world.step_with_events(&hooks, &physics.collector);
    if physics.uses_dominance() {
        dominance::transfer_velocity(physics, dt);
    }
    write_back(physics, world);
    pose_ragdolls(world, &mut physics.ragdolls);
    std::mem::swap(
        &mut physics.events,
        &mut *physics
            .collector
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for i in 0..physics.events.len() {
        let (a, b, flags, started) = match physics.events[i] {
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
    physics.events.clear();
}

/// Draws moving bodies smoothly: between two physics steps, sets each
/// moving body's [`d3::Transform`](crate::d3::Transform) to a blend of its last two poses, by how
/// far the frame is between them ([`Time::fixed_alpha`](crate::time::Time::fixed_alpha)). Added as the first
/// frame system by [`Game::physics3d`](crate::game::Game::physics3d), so camera
/// code sees the blended pose too. Does nothing in headless runs, where
/// frames and ticks line up. Transforms you changed since the last step are
/// left alone (they teleport the body at the next step). While
/// [`set_paused`](Physics3D::set_paused) holds the step, bodies are drawn
/// where the last step left them, so they don't sway between two poses.
pub fn interpolate_physics3d(ctx: &mut Ctx) {
    if ctx.headless {
        return;
    }
    let alpha = if ctx.physics3d.paused {
        1.0
    } else {
        ctx.time.fixed_alpha().clamp(0.0, 1.0)
    };
    let physics = &mut ctx.physics3d;
    if physics.links.is_empty() {
        return;
    }
    let n = physics.moving().len();
    for k in 0..n {
        let i = physics.moving[k] as usize;
        let Some(link) = physics.links[i].as_mut() else {
            continue;
        };
        let Some(t) = ctx.world.get_mut::<Transform3D>(link.entity) else {
            continue;
        };
        if !written_matches(link, t) {
            continue;
        }
        t.position = link.prev.position.lerp(link.curr.position, alpha);
        if let LinkKind::Body(..) = link.kind {
            t.rotation = link.prev.rotation.slerp(link.curr.rotation, alpha);
        }
        link.written = *t;
    }
}

/// Whether `t` is what physics last wrote (or saw): characters only own
/// their position, so turning one isn't a teleport.
fn written_matches(link: &Link, t: &Transform3D) -> bool {
    match link.kind {
        LinkKind::Character(_) => link.written.position == t.position,
        LinkKind::Body(..) | LinkKind::Terrain(..) => link.written == *t,
    }
}

/// New bodies in, your changes applied, gone ones out.
fn sync_into(physics: &mut Physics3D, world: &mut World) {
    let step = physics.step;
    let loads = crate::reflect::world_loads(world);
    if loads != physics.world_loads {
        physics.world_loads = loads;
        physics.forget_bodies();
    }
    world
        .query::<(Entity, &Transform3D, &RigidBody3D, &Collider3D, Option<&Velocity3D>)>()
        .filter::<Without<Character3D>>()
        .for_each(|(e, t, rigid, shape, velocity)| {
            let kind = LinkKind::Body(*rigid, *shape);
            sync_one(physics, e, t, kind, velocity.copied(), step);
        });
    world
        .query::<(Entity, &Transform3D, &Character3D)>()
        .for_each(|(e, t, c)| {
            let kind = LinkKind::Character(character::Capsule::of(c));
            sync_one(physics, e, t, kind, None, step);
        });
    world
        .query::<(Entity, &Transform3D, &TerrainCollider)>()
        .filter::<Without<RigidBody3D>>()
        .filter::<Without<Character3D>>()
        .for_each(|(e, t, terrain)| {
            let kind = LinkKind::Terrain(*terrain, t.scale);
            sync_one(physics, e, t, kind, None, step);
        });
    // Despawned entities and ones that lost a physics component, in index
    // order so runs stay deterministic.
    for i in 0..physics.links.len() {
        if physics.links[i].is_some_and(|l| l.seen != step) {
            physics.remove(i);
        }
    }
}

fn sync_one(
    physics: &mut Physics3D,
    e: Entity,
    t: &Transform3D,
    kind: LinkKind,
    velocity: Option<Velocity3D>,
    step: u64,
) {
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
            physics.moving_dirty = true;
        }
        physics.insert_link(e, t, kind, velocity.unwrap_or_default());
        return;
    };
    link.seen = step;
    if link.kind != kind {
        // Rare (a door that becomes dynamic, a resized collider, edited
        // terrain): rebuild. A static collider that was in stays in, so
        // nothing standing on it falls through.
        let (current, was_active) = (link.velocity, link.active);
        physics.world.remove_body(link.body);
        physics.insert_link(e, t, kind, velocity.unwrap_or(current));
        if was_active && physics.links[i].is_some_and(|l| !l.active) {
            physics.set_active(i, true);
        }
        return;
    }
    // `bodies.get_mut` marks a body as changed (rapier then redoes its
    // broad phase and wakes it), so only touch bodies you changed.
    let mut moved_static = None;
    if !written_matches(link, t)
        && let Some(body) = physics.world.bodies.get_mut(link.body)
    {
        let rotation = match kind {
            LinkKind::Body(..) | LinkKind::Terrain(..) => t.rotation,
            LinkKind::Character(_) => Quat::IDENTITY,
        };
        body.set_position(pose_to_rp(t.position, rotation), true);
        link.written = *t;
        // A teleport isn't blended.
        link.curr = Pose::of(t);
        link.prev = link.curr;
        if !kind.moves() {
            moved_static = Some(link.collider);
        }
    }
    if let Some(v) = velocity
        && link.velocity != v
        && let Some(body) = physics.world.bodies.get_mut(link.body)
    {
        body.set_linvel(to_rp(v.linear), true);
        body.set_angvel(to_rp(v.angular), true);
        link.velocity = v;
    }
    if let Some(collider) = moved_static {
        // Moved ground: where it lies, for collider streaming.
        let rect = physics.rect_of(collider, t);
        if let Some(link) = physics.links[i].as_mut() {
            link.rect = rect;
        }
    }
}

fn write_back(physics: &mut Physics3D, world: &mut World) {
    let n = physics.moving().len();
    for k in 0..n {
        let i = physics.moving[k] as usize;
        let Some(link) = physics.links[i].as_mut() else {
            continue;
        };
        let e = link.entity;
        let Some(body) = physics.world.bodies.get(link.body) else {
            continue;
        };
        // A sleeping body hasn't moved since it fell asleep: it only
        // needs its speed cleared, once. Checked before looking the
        // entity up, since most bodies in a big scene are asleep.
        if body.is_sleeping() {
            link.prev = link.curr;
            if let LinkKind::Body(..) = link.kind
                && link.velocity != Velocity3D::default()
                && world.get::<Transform3D>(e).is_some()
            {
                link.velocity = Velocity3D::default();
                if let Some(velocity) = world.get_mut::<Velocity3D>(e) {
                    *velocity = link.velocity;
                }
            }
            continue;
        }
        // Only entities that still have a transform (as the old query).
        if world.get::<Transform3D>(e).is_none() {
            continue;
        }
        link.prev = link.curr;
        link.curr.position = from_rp(body.translation());
        let moved_rotation = matches!(link.kind, LinkKind::Body(..)).then(|| rot_from_rp(*body.rotation()));
        if let Some(rotation) = moved_rotation {
            link.curr.rotation = rotation;
            let v = Velocity3D {
                linear: from_rp(body.linvel()),
                angular: from_rp(body.angvel()),
            };
            link.velocity = v;
            if let Some(velocity) = world.get_mut::<Velocity3D>(e) {
                *velocity = v;
            }
        }
        let Some(t) = world.get_mut::<Transform3D>(e) else {
            continue;
        };
        t.position = link.curr.position;
        if let Some(rotation) = moved_rotation {
            t.rotation = rotation;
        }
        link.written = *t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::FrameTiming;
    use glam::{Vec2, vec3};

    fn tick(ctx: &mut Ctx) {
        ctx.set_in_sim(true);
        step_physics3d(ctx);
        ctx.set_in_sim(false);
    }

    fn frame(ctx: &mut Ctx, alpha: f32) {
        ctx.begin_frame(FrameTiming {
            dt: 0.01,
            fixed_steps: 0,
            alpha,
        });
        interpolate_physics3d(ctx);
    }

    #[test]
    fn frames_between_steps_draw_a_blend_and_steps_continue_from_the_physics_pose() {
        let mut ctx = Ctx::new(0.02, Vec2::ZERO);
        start(&mut ctx, Vec3::ZERO);
        let ball = ctx.world.spawn((
            Transform3D::default(),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.5),
            Velocity3D::linear(vec3(10.0, 0.0, 0.0)),
        ));
        tick(&mut ctx);
        tick(&mut ctx);
        let x = |ctx: &mut Ctx| ctx.world.get::<Transform3D>(ball).unwrap().position.x;
        assert!((x(&mut ctx) - 0.4).abs() < 1e-4, "two steps of 0.2 m");
        frame(&mut ctx, 0.5);
        assert!(
            (x(&mut ctx) - 0.3).abs() < 1e-4,
            "halfway between the last two: {}",
            x(&mut ctx)
        );
        frame(&mut ctx, 0.75);
        assert!((x(&mut ctx) - 0.35).abs() < 1e-4);
        // The blend isn't a teleport: the next step goes on from 0.4.
        tick(&mut ctx);
        assert!((x(&mut ctx) - 0.6).abs() < 1e-4, "{}", x(&mut ctx));
        // But your own change between steps is.
        ctx.world.get_mut::<Transform3D>(ball).unwrap().position = vec3(-5.0, 0.0, 0.0);
        frame(&mut ctx, 0.5);
        assert_eq!(x(&mut ctx), -5.0, "left alone by the blend");
        tick(&mut ctx);
        assert!((x(&mut ctx) + 4.8).abs() < 1e-4, "{}", x(&mut ctx));
    }

    #[test]
    fn code_before_the_next_step_reads_the_drawn_blend_and_the_service_keeps_the_stepped_pose() {
        let mut ctx = Ctx::new(0.02, Vec2::ZERO);
        start(&mut ctx, Vec3::ZERO);
        let ball = ctx.world.spawn((
            Transform3D::default(),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.5),
            Velocity3D::linear(vec3(10.0, 0.0, 0.0)),
        ));
        tick(&mut ctx);
        tick(&mut ctx);
        frame(&mut ctx, 0.25);
        // What a system before the next step finds in the component: the
        // pose that was drawn. The physics service has the stepped one.
        let drawn = ctx.world.get::<Transform3D>(ball).unwrap().position.x;
        assert!((drawn - 0.25).abs() < 1e-4, "{drawn}");
        let stepped = ctx.physics3d.position(ball).unwrap().x;
        assert!((stepped - 0.4).abs() < 1e-4, "{stepped}");
        // After the step the component is the stepped pose again.
        tick(&mut ctx);
        let after = ctx.world.get::<Transform3D>(ball).unwrap().position.x;
        assert!((after - 0.6).abs() < 1e-4, "{after}");
        assert_eq!(after, ctx.physics3d.position(ball).unwrap().x);
    }

    #[test]
    fn a_blend_skips_entities_removed_since_the_step_and_the_next_step_drops_their_bodies() {
        let mut ctx = Ctx::new(0.02, Vec2::ZERO);
        start(&mut ctx, Vec3::ZERO);
        let spawn = |ctx: &mut Ctx, z: f32| {
            ctx.world.spawn((
                Transform3D::at(vec3(0.0, 0.0, z)),
                RigidBody3D::dynamic(),
                Collider3D::sphere(0.5),
                Velocity3D::linear(vec3(10.0, 0.0, 0.0)),
            ))
        };
        let (gone, kept) = (spawn(&mut ctx, 0.0), spawn(&mut ctx, 5.0));
        tick(&mut ctx);
        tick(&mut ctx);
        ctx.world.despawn(gone);
        frame(&mut ctx, 0.5);
        let x = ctx.world.get::<Transform3D>(kept).unwrap().position.x;
        assert!((x - 0.3).abs() < 1e-4, "the other body is still blended: {x}");
        assert_eq!(ctx.physics3d.body_count(), 2, "bodies leave on a step, not on a frame");
        tick(&mut ctx);
        assert_eq!(ctx.physics3d.body_count(), 1);
        frame(&mut ctx, 0.5);
        let x = ctx.world.get::<Transform3D>(kept).unwrap().position.x;
        assert!((x - 0.5).abs() < 1e-4, "{x}");
    }

    /// A floor, a stack of boxes and a walking character.
    fn pile(world: &mut World) -> Vec<Entity> {
        let mut entities = vec![world.spawn((
            Transform3D::at(vec3(0.0, -0.5, 0.0)),
            RigidBody3D::fixed(),
            Collider3D::cuboid(vec3(20.0, 1.0, 20.0)),
        ))];
        for i in 0..27 {
            let at = vec3(
                (i % 3) as f32 * 0.6,
                0.5 + (i / 9) as f32 * 0.6,
                (i / 3 % 3) as f32 * 0.6,
            );
            entities.push(world.spawn((
                Transform3D::at(at),
                RigidBody3D::dynamic(),
                Collider3D::cuboid(Vec3::splat(0.5)),
            )));
        }
        entities.push(world.spawn((Transform3D::at(vec3(4.0, 1.0, 0.0)), Character3D::default())));
        entities
    }

    /// Every pose and velocity, bit for bit.
    fn fingerprint(world: &World, entities: &[Entity]) -> Vec<[u32; 10]> {
        entities
            .iter()
            .map(|&e| {
                let t = world.get::<Transform3D>(e).copied().unwrap_or_default();
                let v = world.get::<Velocity3D>(e).copied().unwrap_or_default();
                let (p, r, l) = (t.position, t.rotation, v.linear);
                [p.x, p.y, p.z, r.x, r.y, r.z, r.w, l.x, l.y, l.z].map(f32::to_bits)
            })
            .collect()
    }

    #[test]
    fn the_core_step_needs_only_its_inputs_and_matches_the_ctx_system() {
        const DT: f32 = 0.02;
        let gravity = vec3(0.0, -9.81, 0.0);
        // The core, on a bare world: no Ctx, window or device.
        let mut physics = Physics3D::new(gravity);
        let mut world = World::new();
        let mut events = Events::default();
        let bare = pile(&mut world);
        for _ in 0..100 {
            step(&mut physics, &mut world, &mut events, DT);
        }

        // The public system, through a Ctx.
        let mut ctx = Ctx::new(f64::from(DT), Vec2::ZERO);
        ctx.set_headless(true);
        start(&mut ctx, gravity);
        let wrapped = pile(&mut ctx.world);
        for _ in 0..100 {
            tick(&mut ctx);
        }

        assert_eq!(physics.step, 100);
        assert_eq!(fingerprint(&world, &bare), fingerprint(&ctx.world, &wrapped));
        let started = events.peek::<CollisionStarted>();
        assert!(!started.is_empty(), "the boxes land");
        assert_eq!(started, ctx.events.peek::<CollisionStarted>());
        assert_eq!(events.peek::<CollisionStopped>(), ctx.events.peek::<CollisionStopped>());
    }

    #[test]
    fn headless_runs_draw_the_stepped_pose() {
        let mut ctx = Ctx::new(0.02, Vec2::ZERO);
        ctx.set_headless(true);
        start(&mut ctx, Vec3::ZERO);
        let ball = ctx.world.spawn((
            Transform3D::default(),
            RigidBody3D::dynamic(),
            Collider3D::sphere(0.5),
            Velocity3D::linear(vec3(10.0, 0.0, 0.0)),
        ));
        tick(&mut ctx);
        frame(&mut ctx, 0.5);
        assert!((ctx.world.get::<Transform3D>(ball).unwrap().position.x - 0.2).abs() < 1e-4);
    }
}
