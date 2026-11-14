//! 3D physics from C (the `physics3d` feature, on by default): bodies,
//! colliders and characters as components, collision meshes, a game
//! setting, ray and shape casts, overlaps and impulses. Collision events are
//! read with `ponko_ctx_collisions`, as in 2D. Without the feature every
//! call here fails with an error naming it.
#![cfg_attr(not(feature = "physics3d"), allow(unused_variables))]

use crate::three::PonkoVec3;
use crate::three::{PonkoQuat, PonkoTransform3D};
use crate::types::PonkoEntity;
use crate::{PonkoCtx, PonkoGame, PonkoStatus, PonkoWorld};

/// A 3D collider's shape (`PonkoCollider3D::shape`).
/// ```
/// assert_eq!(ponko_ffi::PonkoShape3DKind::PONKO_SHAPE3D_BOX as u32, 1);
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoShape3DKind {
    /// A ball: uses `radius`.
    PONKO_SHAPE3D_SPHERE,
    /// A box: uses `size` (full width, height, depth).
    PONKO_SHAPE3D_BOX,
    /// An upright capsule: `height` between the centers of its ends, and `radius`.
    PONKO_SHAPE3D_CAPSULE,
    /// A convex hull, triangle mesh or heightfield: uses `mesh`.
    PONKO_SHAPE3D_MESH,
}

/// A convex hull, triangle mesh or heightfield for colliders
/// (`ponko_ctx_physics3d_heightfield` and friends). A handle.
/// ```
/// let m = ponko_ffi::PonkoCollisionMesh { id: 0 };
/// assert_eq!(m.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoCollisionMesh {
    /// Opaque.
    pub id: u64,
}

/// Which colliders touch which: a collider is in the groups of
/// `memberships` (a bit each) and touches colliders whose memberships share
/// a bit with its `filter`; both sides must agree.
/// ```
/// let all = ponko_ffi::ponko_collision_groups_all();
/// assert_eq!(all.filter, u32::MAX);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PonkoCollisionGroups {
    /// The groups it's in.
    pub memberships: u32,
    /// The groups it touches.
    pub filter: u32,
}

/// The built-in `RigidBody3D` component: makes an entity a 3D physics body
/// (it also needs a `PonkoCollider3D` and a `PonkoTransform3D`). Start from
/// `ponko_rigid_body3d(kind)`; `kind` is a `PonkoBodyKind`. Not a query column.
/// ```
/// let b = ponko_ffi::ponko_rigid_body3d(ponko_ffi::PonkoBodyKind::PONKO_BODY_FIXED as u32);
/// assert_eq!(b.kind, 1);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoRigidBody3D {
    /// A `PonkoBodyKind`.
    pub kind: u32,
    /// 1 normal, 0 floats, negative falls up.
    pub gravity_scale: f32,
    /// Slows movement down over time; 0 none.
    pub linear_damping: f32,
    /// Slows spinning down over time; 0 none.
    pub angular_damping: f32,
    /// Never rotates.
    pub lock_rotation: bool,
    /// Continuous collision detection, so fast bodies don't pass through thin walls.
    pub ccd: bool,
    /// Its mass in kg, instead of its collider's density × volume; 0 (the
    /// default): from the density.
    pub mass: f32,
    /// Use `center_of_mass` instead of the collider's center.
    pub has_center_of_mass: bool,
    /// Its center of mass in its own frame, when `has_center_of_mass` (cars
    /// want it low).
    pub center_of_mass: PonkoVec3,
    /// Contact dominance, -128 to 127 (default 0): in a contact, the body
    /// with the higher value pushes the other without being pushed back.
    pub dominance: i8,
}

/// The built-in `Collider3D` component: a body's shape in meters, centered
/// on the entity plus `offset`, its surface and its groups. Start from
/// `ponko_collider3d_sphere`, `_box`, `_capsule` or `_mesh`. Checked on
/// spawn/set: sizes must be positive, density, friction and restitution
/// not negative. Not a query column.
/// ```
/// let c = ponko_ffi::ponko_collider3d_sphere(0.5);
/// assert_eq!((c.radius, c.density, c.friction), (0.5, 1000.0, 0.5));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoCollider3D {
    /// A `PonkoShape3DKind`.
    pub shape: u32,
    /// Sphere and capsule radius.
    pub radius: f32,
    /// Capsule height between the centers of its ends.
    pub height: f32,
    /// Box width, height and depth.
    pub size: PonkoVec3,
    /// The mesh of a mesh collider.
    pub mesh: PonkoCollisionMesh,
    /// Where the shape sits relative to the entity's position.
    pub offset: PonkoVec3,
    /// Mass per cubic meter (default 1000, water).
    pub density: f32,
    /// 0 ice, 1 rubber.
    pub friction: f32,
    /// Bounciness: 0 none, 1 keeps all its speed.
    pub restitution: f32,
    /// Detects overlaps (collision events) but doesn't push.
    pub sensor: bool,
    /// Which colliders it touches.
    pub groups: PonkoCollisionGroups,
}

/// The built-in `Velocity3D` component, meters and radians per second: a
/// plain value (same layout as Rust's), so it works as a query column.
/// ```
/// assert_eq!(std::mem::size_of::<ponko_ffi::PonkoVelocity3D>(), 24);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoVelocity3D {
    /// Meters per second.
    pub linear: PonkoVec3,
    /// Radians per second around X, Y and Z.
    pub angular: PonkoVec3,
}

/// How one axis of a `PonkoJoint3D` may move.
/// ```
/// let _ = ponko_ffi::PonkoJointMotionKind::PONKO_JOINT_LIMITED;
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoJointMotionKind {
    /// Moves freely.
    PONKO_JOINT_FREE = 0,
    /// Doesn't move.
    PONKO_JOINT_LOCKED = 1,
    /// Moves between `min` and `max` (meters, or radians for turning axes).
    PONKO_JOINT_LIMITED = 2,
}

/// One axis of a joint (Rust `JointMotion3D`).
/// ```
/// let m = ponko_ffi::ponko_joint3d_fixed(0, 0).axes[0];
/// assert_eq!(m.kind, ponko_ffi::PonkoJointMotionKind::PONKO_JOINT_LOCKED);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoJointMotion3D {
    /// Free, locked or limited.
    pub kind: PonkoJointMotionKind,
    /// The lower limit, when limited.
    pub min: f32,
    /// The upper limit, when limited.
    pub max: f32,
}

/// A motor on one axis of a joint (Rust `JointDrive3D`): a spring toward
/// `target_position` (`stiffness`), and toward `target_velocity` (`damping`).
/// ```
/// assert!(!ponko_ffi::ponko_joint3d_fixed(0, 0).drives[0].enabled);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoJointDrive3D {
    /// Whether this axis has a motor.
    pub enabled: bool,
    /// Where the spring pulls to (meters or radians).
    pub target_position: f32,
    /// The speed the motor aims for.
    pub target_velocity: f32,
    /// Spring strength toward the target position.
    pub stiffness: f32,
    /// Strength toward the target velocity.
    pub damping: f32,
    /// The most force (or torque) it uses; infinity for no cap.
    pub max_force: f32,
}

/// The built-in `Joint3D` component: put it on its own entity (or either
/// body's) to join two bodies. Axes and drives are in the order linear X,
/// Y, Z, then angular X, Y, Z of the joint frame; local X is the hinge,
/// slider and twist axis. Start from `ponko_joint3d_fixed`, `_spherical`,
/// `_revolute`, `_prismatic` or `_generic`. Despawning it or either body
/// removes the joint. Not a query column.
/// ```
/// let j = ponko_ffi::ponko_joint3d_revolute(1, 2);
/// assert_eq!(j.axes[3].kind, ponko_ffi::PonkoJointMotionKind::PONKO_JOINT_FREE);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoJoint3D {
    /// The first body's entity.
    pub body_a: u64,
    /// The second body's entity.
    pub body_b: u64,
    /// The joint's frame in body A's space.
    pub frame_a: PonkoTransform3D,
    /// The joint's frame in body B's space.
    pub frame_b: PonkoTransform3D,
    /// How each axis may move.
    pub axes: [PonkoJointMotion3D; 6],
    /// Motors, per axis.
    pub drives: [PonkoJointDrive3D; 6],
    /// Whether the two joined bodies still collide with each other. Default false.
    pub contacts_enabled: bool,
    /// A cone limit on swinging (angular Y and Z together), in radians;
    /// negative for none (the default).
    pub swing_cone: f32,
}

fn joint3d(a: u64, b: u64, locked: [bool; 6]) -> PonkoJoint3D {
    let identity = PonkoTransform3D {
        rotation: PonkoQuat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        position: PonkoVec3::default(),
        scale: PonkoVec3 { x: 1.0, y: 1.0, z: 1.0 },
        reserved: [0.0; 2],
    };
    PonkoJoint3D {
        body_a: a,
        body_b: b,
        frame_a: identity,
        frame_b: identity,
        axes: locked.map(|l| PonkoJointMotion3D {
            kind: if l {
                PonkoJointMotionKind::PONKO_JOINT_LOCKED
            } else {
                PonkoJointMotionKind::PONKO_JOINT_FREE
            },
            min: 0.0,
            max: 0.0,
        }),
        drives: [PonkoJointDrive3D {
            enabled: false,
            target_position: 0.0,
            target_velocity: 0.0,
            stiffness: 0.0,
            damping: 0.0,
            max_force: f32::INFINITY,
        }; 6],
        contacts_enabled: false,
        swing_cone: -1.0,
    }
}

/// A joint that leaves every axis free: set the axes yourself.
/// ```
/// assert_eq!(ponko_ffi::ponko_joint3d_generic(1, 2).swing_cone, -1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_joint3d_generic(body_a: u64, body_b: u64) -> PonkoJoint3D {
    joint3d(body_a, body_b, [false; 6])
}

/// A joint that welds two bodies together.
/// ```
/// let j = ponko_ffi::ponko_joint3d_fixed(1, 2);
/// assert_eq!(j.body_b, 2);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_joint3d_fixed(body_a: u64, body_b: u64) -> PonkoJoint3D {
    joint3d(body_a, body_b, [true; 6])
}

/// A ball joint: the frames stay together, turning freely (shoulders, ragdolls).
/// ```
/// let j = ponko_ffi::ponko_joint3d_spherical(1, 2);
/// assert_eq!(j.axes[5].kind, ponko_ffi::PonkoJointMotionKind::PONKO_JOINT_FREE);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_joint3d_spherical(body_a: u64, body_b: u64) -> PonkoJoint3D {
    joint3d(body_a, body_b, [true, true, true, false, false, false])
}

/// A hinge turning about the frames' X axis (doors, wheels, elbows).
/// ```
/// let j = ponko_ffi::ponko_joint3d_revolute(1, 2);
/// assert_eq!(j.axes[4].kind, ponko_ffi::PonkoJointMotionKind::PONKO_JOINT_LOCKED);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_joint3d_revolute(body_a: u64, body_b: u64) -> PonkoJoint3D {
    joint3d(body_a, body_b, [true, true, true, false, true, true])
}

/// A slider along the frames' X axis (pistons, drawers).
/// ```
/// let j = ponko_ffi::ponko_joint3d_prismatic(1, 2);
/// assert_eq!(j.axes[0].kind, ponko_ffi::PonkoJointMotionKind::PONKO_JOINT_FREE);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_joint3d_prismatic(body_a: u64, body_b: u64) -> PonkoJoint3D {
    joint3d(body_a, body_b, [false, true, true, true, true, true])
}

/// The built-in `Character3D` component: a walking capsule whose
/// `PonkoTransform3D` position is at its feet. Set `velocity` (x and z to
/// walk, y to jump); read `grounded` after the step. Start from
/// `ponko_character3d(height, radius)`. Not a query column.
/// ```
/// let c = ponko_ffi::ponko_character3d(1.8, 0.3);
/// assert_eq!(c.step_height, 0.35);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoCharacter3D {
    /// Feet to head, meters.
    pub height: f32,
    /// The capsule's radius.
    pub radius: f32,
    /// The steepest ground it walks up, radians.
    pub max_slope: f32,
    /// The highest step it climbs; 0 none.
    pub step_height: f32,
    /// How far down it follows the ground; 0 never.
    pub snap_distance: f32,
    /// How hard it pushes dynamic bodies, as a mass in kg; 0 not at all.
    pub push_mass: f32,
    /// 1 falls by gravity, 0 floats.
    pub gravity_scale: f32,
    /// Which colliders it touches.
    pub groups: PonkoCollisionGroups,
    /// Meters per second: yours to set; the step adds gravity.
    pub velocity: PonkoVec3,
    /// Stands on something, as of the last step.
    pub grounded: bool,
}

/// What a 3D ray or shape cast hit.
/// ```
/// assert_eq!(ponko_ffi::PonkoHit3D::default().distance, 0.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoHit3D {
    /// Whose collider.
    pub entity: PonkoEntity,
    /// Where (a shape cast: the first point touched).
    pub point: PonkoVec3,
    /// The surface's outward direction there.
    pub normal: PonkoVec3,
    /// How far the ray or shape went.
    pub distance: f32,
}

/// Which colliders a 3D query sees. `ponko_query_filter3d()` sees every
/// solid collider and no sensors.
/// ```
/// let f = ponko_ffi::ponko_query_filter3d();
/// assert!(!f.exclude && !f.sensors);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PonkoQueryFilter3D {
    /// Look through `exclude_entity`.
    pub exclude: bool,
    /// The entity to look through, when `exclude`.
    pub exclude_entity: u64,
    /// The query touches colliders as a collider with these groups would.
    pub groups: PonkoCollisionGroups,
    /// Sees sensors as well.
    pub sensors: bool,
}

/// In every group, touching every group (the default).
/// ```
/// assert_eq!(ponko_ffi::ponko_collision_groups_all().memberships, u32::MAX);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_collision_groups_all() -> PonkoCollisionGroups {
    PonkoCollisionGroups {
        memberships: u32::MAX,
        filter: u32::MAX,
    }
}

/// The default query filter: every solid collider, no sensors.
/// ```
/// assert_eq!(ponko_ffi::ponko_query_filter3d().groups.filter, u32::MAX);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_query_filter3d() -> PonkoQueryFilter3D {
    PonkoQueryFilter3D {
        exclude: false,
        exclude_entity: 0,
        groups: ponko_collision_groups_all(),
        sensors: false,
    }
}

/// A 3D body of this kind (a `PonkoBodyKind`) with default settings.
/// Unknown kinds give a dynamic body.
/// ```
/// assert_eq!(ponko_ffi::ponko_rigid_body3d(9).kind, 0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_rigid_body3d(kind: u32) -> PonkoRigidBody3D {
    PonkoRigidBody3D {
        kind: if kind <= 2 { kind } else { 0 },
        gravity_scale: 1.0,
        linear_damping: 0.0,
        angular_damping: 0.0,
        lock_rotation: false,
        ccd: false,
        mass: 0.0,
        has_center_of_mass: false,
        center_of_mass: PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 },
        dominance: 0,
    }
}

fn collider3d(shape: PonkoShape3DKind) -> PonkoCollider3D {
    PonkoCollider3D {
        shape: shape as u32,
        radius: 0.0,
        height: 0.0,
        size: PonkoVec3::default(),
        mesh: PonkoCollisionMesh::default(),
        offset: PonkoVec3::default(),
        density: 1000.0,
        friction: 0.5,
        restitution: 0.0,
        sensor: false,
        groups: ponko_collision_groups_all(),
    }
}

/// A ball collider (density 1000, friction 0.5, no bounce).
/// ```
/// assert_eq!(ponko_ffi::ponko_collider3d_sphere(1.0).shape, 0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_collider3d_sphere(radius: f32) -> PonkoCollider3D {
    PonkoCollider3D {
        radius,
        ..collider3d(PonkoShape3DKind::PONKO_SHAPE3D_SPHERE)
    }
}

/// A box collider `size` wide, high and deep.
/// ```
/// let c = ponko_ffi::ponko_collider3d_box(ponko_ffi::PonkoVec3 { x: 1.0, y: 2.0, z: 3.0 });
/// assert_eq!(c.size.z, 3.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_collider3d_box(size: PonkoVec3) -> PonkoCollider3D {
    PonkoCollider3D {
        size,
        ..collider3d(PonkoShape3DKind::PONKO_SHAPE3D_BOX)
    }
}

/// An upright capsule collider: `height` between the centers of its ends.
/// ```
/// assert_eq!(ponko_ffi::ponko_collider3d_capsule(1.2, 0.3).height, 1.2);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_collider3d_capsule(height: f32, radius: f32) -> PonkoCollider3D {
    PonkoCollider3D {
        height,
        radius,
        ..collider3d(PonkoShape3DKind::PONKO_SHAPE3D_CAPSULE)
    }
}

/// A collider of a convex hull, triangle mesh or heightfield.
/// ```
/// let c = ponko_ffi::ponko_collider3d_mesh(ponko_ffi::PonkoCollisionMesh { id: 7 });
/// assert_eq!(c.mesh.id, 7);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_collider3d_mesh(mesh: PonkoCollisionMesh) -> PonkoCollider3D {
    PonkoCollider3D {
        mesh,
        ..collider3d(PonkoShape3DKind::PONKO_SHAPE3D_MESH)
    }
}

/// A character `height` tall (feet to head) and `radius` wide, with the
/// default limits: 45° slopes, 0.35 m steps, 0.3 m ground snapping, pushes
/// like 80 kg.
/// ```
/// let c = ponko_ffi::ponko_character3d(1.8, 0.3);
/// assert!((c.max_slope - 45f32.to_radians()).abs() < 1e-6);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_character3d(height: f32, radius: f32) -> PonkoCharacter3D {
    PonkoCharacter3D {
        height,
        radius,
        max_slope: 45f32.to_radians(),
        step_height: 0.35,
        snap_distance: 0.3,
        push_mass: 80.0,
        gravity_scale: 1.0,
        groups: ponko_collision_groups_all(),
        velocity: PonkoVec3::default(),
        grounded: false,
    }
}

/// The id of the built-in `RigidBody3D` component (`PonkoRigidBody3D`),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_rigid_body3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_rigid_body3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id, 0) }
}

/// The id of the built-in `Collider3D` component (`PonkoCollider3D`),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_collider3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_collider3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id, 1) }
}

/// The id of the built-in `Velocity3D` component (`PonkoVelocity3D`, also a
/// query column), registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_velocity3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_velocity3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id, 2) }
}

/// The id of the built-in `Character3D` component (`PonkoCharacter3D`),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_character3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_character3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id, 3) }
}

/// The id of the built-in `Joint3D` component (`PonkoJoint3D`),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_joint3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_joint3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id, 4) }
}

/// How many joints the 3D physics world holds after its last step.
/// # Safety
/// `ctx` null or live; `out_count` writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_joint_count(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_joint_count(ctx: *mut PonkoCtx, out_count: *mut usize) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::joint_count(ctx, out_count) }
}

/// Turns on 3D physics, pulling dynamic bodies and characters by `gravity`
/// (m/s², +Y up). Adds the physics step as a simulation system at this
/// point in the order, and sets the simulation to 50 steps per second
/// unless `ponko_game_fixed_hz` chose a rate.
/// # Safety
/// `game` must be null or a live game.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_game_physics3d(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_game_physics3d(game: *mut PonkoGame, gravity: PonkoVec3) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::game_physics3d(game, gravity) }
}

/// Changes 3D gravity.
/// # Safety
/// Live ctx, outside query callbacks.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_gravity3d(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_gravity3d(ctx: *mut crate::PonkoCtx, gravity: PonkoVec3) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::set_gravity(ctx, gravity) }
}

/// A heightfield collision mesh from the same `count` (n × n) heights and
/// `size` as `ponko_ctx_create_terrain_tile` (NaN for holes): give it to a
/// fixed body at the tile's transform (its corner).
/// # Safety
/// Live ctx; `heights` readable for `count` floats; writable output.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_heightfield(std::ptr::null_mut(), std::ptr::null(), 0, 1.0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_heightfield(
    ctx: *mut crate::PonkoCtx,
    heights: *const f32,
    count: usize,
    size: f32,
    out_mesh: *mut PonkoCollisionMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::heightfield(ctx, heights, count, size, out_mesh) }
}

/// A convex hull collision mesh around `count` points.
/// `PONKO_INVALID_ARGUMENT` if they're flat or not finite.
/// # Safety
/// Live ctx; `points` readable for `count` values; writable output.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_convex_hull(std::ptr::null_mut(), std::ptr::null(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_convex_hull(
    ctx: *mut crate::PonkoCtx,
    points: *const PonkoVec3,
    count: usize,
    out_mesh: *mut PonkoCollisionMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::convex_hull(ctx, points, count, out_mesh) }
}

/// A triangle collision mesh: `index_count` indices (three per triangle)
/// into `vertex_count` vertices. `PONKO_INVALID_ARGUMENT` for no
/// triangles, an index out of range or a vertex that isn't finite.
/// # Safety
/// Live ctx; readable arrays of the given lengths; writable output.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_trimesh(std::ptr::null_mut(), std::ptr::null(), 0, std::ptr::null(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_trimesh(
    ctx: *mut crate::PonkoCtx,
    vertices: *const PonkoVec3,
    vertex_count: usize,
    indices: *const u32,
    index_count: usize,
    out_mesh: *mut PonkoCollisionMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::trimesh(ctx, vertices, vertex_count, indices, index_count, out_mesh) }
}

/// Lets go of a collision mesh; colliders using it keep it until removed.
/// # Safety
/// Live ctx.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_free_mesh(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_free_mesh(
    ctx: *mut crate::PonkoCtx,
    mesh: PonkoCollisionMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::free_mesh(ctx, mesh) }
}

/// The closest collider a ray hits within `max_distance`, as of the last
/// physics step. `*out_found` says whether anything was hit; `*out_hit` is
/// written only then.
/// # Safety
/// Live ctx, outside query callbacks; writable outputs.
/// ```
/// use ponko_ffi::*;
/// let (mut found, mut hit) = (false, PonkoHit3D::default());
/// let s = unsafe { ponko_ctx_cast_ray3d(std::ptr::null_mut(), Default::default(), Default::default(), 1.0, ponko_query_filter3d(), &mut found, &mut hit) };
/// assert_eq!(s, PonkoStatus::PONKO_INVALID_ARGUMENT);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_cast_ray3d(
    ctx: *mut crate::PonkoCtx,
    origin: PonkoVec3,
    direction: PonkoVec3,
    max_distance: f32,
    filter: PonkoQueryFilter3D,
    out_found: *mut bool,
    out_hit: *mut PonkoHit3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        imp::cast(
            ctx,
            imp::Cast::Ray,
            origin,
            direction,
            max_distance,
            filter,
            out_found,
            out_hit,
        )
    }
}

/// Every collider a ray hits within `max_distance`, nearest first. Writes
/// up to `capacity` hits and sets `*out_count` to how many there are, so a
/// too-small buffer can be grown and the call repeated.
/// # Safety
/// Live ctx, outside query callbacks; `out_hits` writable for `capacity`
/// (may be null if 0); writable count.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_cast_ray3d_all(std::ptr::null_mut(), Default::default(), Default::default(), 1.0, ponko_ffi::ponko_query_filter3d(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_cast_ray3d_all(
    ctx: *mut crate::PonkoCtx,
    origin: PonkoVec3,
    direction: PonkoVec3,
    max_distance: f32,
    filter: PonkoQueryFilter3D,
    out_hits: *mut PonkoHit3D,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        imp::cast_all(
            ctx,
            origin,
            direction,
            max_distance,
            filter,
            out_hits,
            capacity,
            out_count,
        )
    }
}

/// Sweeps a ball from `origin` towards `direction`: the first collider it
/// touches within `max_distance`. Outputs as `ponko_ctx_cast_ray3d`.
/// # Safety
/// As `ponko_ctx_cast_ray3d`.
/// ```
/// use ponko_ffi::*;
/// let (mut found, mut hit) = (false, PonkoHit3D::default());
/// let s = unsafe { ponko_ctx_cast_sphere3d(std::ptr::null_mut(), Default::default(), 0.5, Default::default(), 1.0, ponko_query_filter3d(), &mut found, &mut hit) };
/// assert_eq!(s, PonkoStatus::PONKO_INVALID_ARGUMENT);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_cast_sphere3d(
    ctx: *mut crate::PonkoCtx,
    origin: PonkoVec3,
    radius: f32,
    direction: PonkoVec3,
    max_distance: f32,
    filter: PonkoQueryFilter3D,
    out_found: *mut bool,
    out_hit: *mut PonkoHit3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        imp::cast(
            ctx,
            imp::Cast::Sphere(radius),
            origin,
            direction,
            max_distance,
            filter,
            out_found,
            out_hit,
        )
    }
}

/// Sweeps an upright capsule centered on `origin` (`height` between the
/// centers of its ends). Outputs as `ponko_ctx_cast_ray3d`.
/// # Safety
/// As `ponko_ctx_cast_ray3d`.
/// ```
/// use ponko_ffi::*;
/// let (mut found, mut hit) = (false, PonkoHit3D::default());
/// let s = unsafe { ponko_ctx_cast_capsule3d(std::ptr::null_mut(), Default::default(), 1.0, 0.3, Default::default(), 1.0, ponko_query_filter3d(), &mut found, &mut hit) };
/// assert_eq!(s, PonkoStatus::PONKO_INVALID_ARGUMENT);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_cast_capsule3d(
    ctx: *mut crate::PonkoCtx,
    origin: PonkoVec3,
    height: f32,
    radius: f32,
    direction: PonkoVec3,
    max_distance: f32,
    filter: PonkoQueryFilter3D,
    out_found: *mut bool,
    out_hit: *mut PonkoHit3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        imp::cast(
            ctx,
            imp::Cast::Capsule(height, radius),
            origin,
            direction,
            max_distance,
            filter,
            out_found,
            out_hit,
        )
    }
}

/// Entities whose collider overlaps the ball. Writes up to `capacity` and
/// sets `*out_count` to how many there are in total.
/// # Safety
/// Live ctx, outside query callbacks; `out_entities` writable for
/// `capacity` (may be null if 0); writable count.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_overlap_sphere3d(std::ptr::null_mut(), Default::default(), 1.0, ponko_ffi::ponko_query_filter3d(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_overlap_sphere3d(
    ctx: *mut crate::PonkoCtx,
    center: PonkoVec3,
    radius: f32,
    filter: PonkoQueryFilter3D,
    out_entities: *mut PonkoEntity,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        imp::overlap(
            ctx,
            imp::Overlap::Sphere(radius),
            center,
            filter,
            out_entities,
            capacity,
            out_count,
        )
    }
}

/// Like `ponko_ctx_overlap_sphere3d`, for a box of `size` turned by `rotation`.
/// # Safety
/// As `ponko_ctx_overlap_sphere3d`.
/// ```
/// let q = ponko_ffi::ponko_quat_identity();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_overlap_box3d(std::ptr::null_mut(), Default::default(), Default::default(), q, ponko_ffi::ponko_query_filter3d(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_overlap_box3d(
    ctx: *mut crate::PonkoCtx,
    center: PonkoVec3,
    size: PonkoVec3,
    rotation: PonkoQuat,
    filter: PonkoQueryFilter3D,
    out_entities: *mut PonkoEntity,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        imp::overlap(
            ctx,
            imp::Overlap::Box(size, rotation),
            center,
            filter,
            out_entities,
            capacity,
            out_count,
        )
    }
}

/// Like `ponko_ctx_overlap_sphere3d`, for an upright capsule.
/// # Safety
/// As `ponko_ctx_overlap_sphere3d`.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_overlap_capsule3d(std::ptr::null_mut(), Default::default(), 1.0, 0.3, ponko_ffi::ponko_query_filter3d(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_overlap_capsule3d(
    ctx: *mut crate::PonkoCtx,
    center: PonkoVec3,
    height: f32,
    radius: f32,
    filter: PonkoQueryFilter3D,
    out_entities: *mut PonkoEntity,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        imp::overlap(
            ctx,
            imp::Overlap::Capsule(height, radius),
            center,
            filter,
            out_entities,
            capacity,
            out_count,
        )
    }
}

/// Pushes a dynamic 3D body once (kg × m/s). `PONKO_NOT_FOUND` if
/// `entity` isn't a 3D body (yet: bodies join at the next physics step).
/// # Safety
/// Live ctx, outside query callbacks.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_apply_impulse3d(std::ptr::null_mut(), 0, Default::default()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_apply_impulse3d(
    ctx: *mut crate::PonkoCtx,
    entity: u64,
    impulse: PonkoVec3,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::apply_impulse(ctx, entity, impulse) }
}

/// The mass of a 3D body in kg. `PONKO_NOT_FOUND` if `entity` isn't one.
/// # Safety
/// Live ctx, outside query callbacks; writable output.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_body_mass3d(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_body_mass3d(
    ctx: *mut crate::PonkoCtx,
    entity: u64,
    out_mass: *mut f32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::mass(ctx, entity, out_mass) }
}

/// A 3D body's center of mass in the world, as of the last physics step.
/// `PONKO_NOT_FOUND` if `entity` isn't a 3D body.
/// # Safety
/// Live ctx, outside query callbacks; writable output.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_body_center_of_mass3d(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_body_center_of_mass3d(
    ctx: *mut crate::PonkoCtx,
    entity: u64,
    out_center: *mut PonkoVec3,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::center_of_mass(ctx, entity, out_center) }
}

#[cfg(feature = "physics3d")]
mod imp {
    use std::cell::RefCell;

    use ponko::__ffi;
    use ponko::d3::Character as Character3D;
    use ponko::d3::Collider as Collider3D;
    use ponko::d3::CollisionGroups;
    use ponko::d3::CollisionMesh;
    use ponko::d3::Hit as Hit3D;
    use ponko::d3::Joint as Joint3D;
    use ponko::d3::JointDrive as JointDrive3D;
    use ponko::d3::JointMotion as JointMotion3D;
    use ponko::d3::QueryFilter as QueryFilter3D;
    use ponko::d3::RigidBody as RigidBody3D;
    use ponko::d3::Shape as Shape3D;
    use ponko::d3::Velocity as Velocity3D;
    use ponko::prelude::{BodyKind, Handle, Quat, Vec3};
    use ponko_ecs::Entity;

    use super::*;
    use crate::ctx::{collect, ctx_access, out};
    use crate::game::{build, ok};
    use crate::types::to_c;
    use crate::{CMirror, Failure, Kind, Mirror, PonkoCtx, boundary, builtin, error, invalid, mirror};

    const _: () = {
        assert!(size_of::<PonkoVelocity3D>() == size_of::<Velocity3D>());
        assert!(align_of::<PonkoVelocity3D>() == align_of::<Velocity3D>());
        assert!(std::mem::offset_of!(PonkoVelocity3D, linear) == std::mem::offset_of!(Velocity3D, linear));
        assert!(std::mem::offset_of!(PonkoVelocity3D, angular) == std::mem::offset_of!(Velocity3D, angular));
    };

    fn non_negative(x: f32, what: &'static str) -> Result<f32, Failure> {
        if x >= 0.0 && x.is_finite() {
            Ok(x)
        } else {
            Err(invalid(what))
        }
    }
    fn positive(x: f32, what: &'static str) -> Result<f32, Failure> {
        if x > 0.0 && x.is_finite() {
            Ok(x)
        } else {
            Err(invalid(what))
        }
    }
    fn finite(v: PonkoVec3, what: &'static str) -> Result<Vec3, Failure> {
        let v: Vec3 = to_c(v);
        if v.is_finite() { Ok(v) } else { Err(invalid(what)) }
    }
    fn groups(g: PonkoCollisionGroups) -> CollisionGroups {
        CollisionGroups::new(g.memberships, g.filter)
    }
    fn groups_to_c(g: CollisionGroups) -> PonkoCollisionGroups {
        PonkoCollisionGroups {
            memberships: g.memberships,
            filter: g.filter,
        }
    }
    fn kind_from_c(kind: u32) -> Result<BodyKind, Failure> {
        Ok(match kind {
            0 => BodyKind::Dynamic,
            1 => BodyKind::Fixed,
            2 => BodyKind::Kinematic,
            _ => return Err(invalid("unknown rigid body kind")),
        })
    }

    impl CMirror for RigidBody3D {
        type C = PonkoRigidBody3D;
        fn from_c(c: &PonkoRigidBody3D) -> Result<Self, Failure> {
            if !c.gravity_scale.is_finite() {
                return Err(invalid("gravity scale must be finite"));
            }
            let mut b = RigidBody3D::dynamic();
            b.kind = kind_from_c(c.kind)?;
            b.gravity_scale = c.gravity_scale;
            b.linear_damping = non_negative(c.linear_damping, "damping must not be negative")?;
            b.angular_damping = non_negative(c.angular_damping, "damping must not be negative")?;
            b.lock_rotation = c.lock_rotation;
            b.ccd = c.ccd;
            let mass = non_negative(c.mass, "mass must not be negative")?;
            b.mass = (mass > 0.0).then_some(mass);
            if c.has_center_of_mass {
                b.center_of_mass = Some(finite(c.center_of_mass, "center of mass must be finite")?);
            }
            b.dominance = c.dominance;
            Ok(b)
        }
        fn to_c(&self) -> PonkoRigidBody3D {
            PonkoRigidBody3D {
                kind: match self.kind {
                    BodyKind::Dynamic => 0,
                    BodyKind::Fixed => 1,
                    BodyKind::Kinematic => 2,
                    _ => unreachable!("a new BodyKind needs a PonkoBodyKind"),
                },
                gravity_scale: self.gravity_scale,
                linear_damping: self.linear_damping,
                angular_damping: self.angular_damping,
                lock_rotation: self.lock_rotation,
                ccd: self.ccd,
                mass: self.mass.unwrap_or(0.0),
                has_center_of_mass: self.center_of_mass.is_some(),
                center_of_mass: to_c(self.center_of_mass.unwrap_or(Vec3::ZERO)),
                dominance: self.dominance,
            }
        }
    }

    impl CMirror for Collider3D {
        type C = PonkoCollider3D;
        fn from_c(c: &PonkoCollider3D) -> Result<Self, Failure> {
            const SIZE: &str = "collider sizes must be positive";
            let base = match c.shape {
                0 => Collider3D::sphere(positive(c.radius, SIZE)?),
                1 => {
                    let size = finite(c.size, SIZE)?;
                    if !(size.cmpgt(Vec3::ZERO).all()) {
                        return Err(invalid(SIZE));
                    }
                    Collider3D::cuboid(size)
                }
                2 => Collider3D::capsule(non_negative(c.height, SIZE)?, positive(c.radius, SIZE)?),
                3 => Collider3D::mesh(mesh_from_c(c.mesh)),
                _ => return Err(invalid("unknown collider shape")),
            };
            const AMOUNT: &str = "density, friction and restitution must not be negative";
            Ok(base
                .density(non_negative(c.density, AMOUNT)?)
                .friction(non_negative(c.friction, AMOUNT)?)
                .restitution(non_negative(c.restitution, AMOUNT)?)
                .offset(finite(c.offset, "collider offset must be finite")?)
                .groups(groups(c.groups)))
            .map(|b| if c.sensor { b.sensor() } else { b })
        }
        fn to_c(&self) -> PonkoCollider3D {
            let base = match self.shape {
                Shape3D::Sphere { radius } => ponko_collider3d_sphere(radius),
                Shape3D::Cuboid { size } => ponko_collider3d_box(to_c(size)),
                Shape3D::Capsule { height, radius } => ponko_collider3d_capsule(height, radius),
                Shape3D::Mesh(m) => ponko_collider3d_mesh(mesh_to_c(m)),
                _ => unreachable!("a new Shape3D needs a PonkoShape3DKind"),
            };
            PonkoCollider3D {
                density: self.density,
                friction: self.friction,
                restitution: self.restitution,
                sensor: self.sensor,
                offset: to_c(self.offset),
                groups: groups_to_c(self.groups),
                ..base
            }
        }
    }

    impl CMirror for Character3D {
        type C = PonkoCharacter3D;
        fn from_c(c: &PonkoCharacter3D) -> Result<Self, Failure> {
            const SIZE: &str = "character height and radius must be positive";
            const LIMIT: &str = "character limits must not be negative";
            let mut ch = Character3D::new(positive(c.height, SIZE)?, positive(c.radius, SIZE)?);
            ch.max_slope = non_negative(c.max_slope, LIMIT)?;
            ch.step_height = non_negative(c.step_height, LIMIT)?;
            ch.snap_distance = non_negative(c.snap_distance, LIMIT)?;
            ch.push_mass = non_negative(c.push_mass, LIMIT)?;
            if !c.gravity_scale.is_finite() {
                return Err(invalid("gravity scale must be finite"));
            }
            ch.gravity_scale = c.gravity_scale;
            ch.groups = groups(c.groups);
            ch.velocity = finite(c.velocity, "character velocity must be finite")?;
            ch.grounded = c.grounded;
            Ok(ch)
        }
        fn to_c(&self) -> PonkoCharacter3D {
            PonkoCharacter3D {
                height: self.height,
                radius: self.radius,
                max_slope: self.max_slope,
                step_height: self.step_height,
                snap_distance: self.snap_distance,
                push_mass: self.push_mass,
                gravity_scale: self.gravity_scale,
                groups: groups_to_c(self.groups),
                velocity: to_c(self.velocity),
                grounded: self.grounded,
            }
        }
    }

    impl CMirror for Joint3D {
        type C = PonkoJoint3D;
        fn from_c(c: &PonkoJoint3D) -> Result<Self, Failure> {
            let mut j = Joint3D::generic(Entity::from_bits(c.body_a), Entity::from_bits(c.body_b));
            j.frame_a = crate::three::transform_from_c(c.frame_a);
            j.frame_b = crate::three::transform_from_c(c.frame_b);
            for i in 0..6 {
                let m = c.axes[i];
                j.axes[i] = match m.kind {
                    PonkoJointMotionKind::PONKO_JOINT_FREE => JointMotion3D::Free,
                    PonkoJointMotionKind::PONKO_JOINT_LOCKED => JointMotion3D::Locked,
                    PonkoJointMotionKind::PONKO_JOINT_LIMITED => {
                        if !(m.min.is_finite() && m.max.is_finite() && m.min <= m.max) {
                            return Err(invalid("joint limits must be finite, min <= max"));
                        }
                        JointMotion3D::Limited(m.min, m.max)
                    }
                };
                let d = c.drives[i];
                j.drives[i] = if d.enabled {
                    const DRIVE: &str = "joint drive strengths must not be negative";
                    Some(JointDrive3D {
                        target_position: d.target_position,
                        target_velocity: d.target_velocity,
                        stiffness: non_negative(d.stiffness, DRIVE)?,
                        damping: non_negative(d.damping, DRIVE)?,
                        max_force: if d.max_force >= 0.0 {
                            d.max_force
                        } else {
                            return Err(invalid(DRIVE));
                        },
                    })
                } else {
                    None
                };
            }
            j.contacts_enabled = c.contacts_enabled;
            j.swing_cone = (c.swing_cone >= 0.0).then_some(c.swing_cone);
            Ok(j)
        }
        fn to_c(&self) -> PonkoJoint3D {
            let mut c = ponko_joint3d_generic(self.body_a.to_bits(), self.body_b.to_bits());
            c.frame_a = crate::three::transform_to_c(self.frame_a);
            c.frame_b = crate::three::transform_to_c(self.frame_b);
            for i in 0..6 {
                c.axes[i] = match self.axes[i] {
                    JointMotion3D::Free => PonkoJointMotion3D {
                        kind: PonkoJointMotionKind::PONKO_JOINT_FREE,
                        min: 0.0,
                        max: 0.0,
                    },
                    JointMotion3D::Locked => PonkoJointMotion3D {
                        kind: PonkoJointMotionKind::PONKO_JOINT_LOCKED,
                        min: 0.0,
                        max: 0.0,
                    },
                    JointMotion3D::Limited(min, max) => PonkoJointMotion3D {
                        kind: PonkoJointMotionKind::PONKO_JOINT_LIMITED,
                        min,
                        max,
                    },
                };
                if let Some(d) = self.drives[i] {
                    c.drives[i] = PonkoJointDrive3D {
                        enabled: true,
                        target_position: d.target_position,
                        target_velocity: d.target_velocity,
                        stiffness: d.stiffness,
                        damping: d.damping,
                        max_force: d.max_force,
                    };
                }
            }
            c.contacts_enabled = self.contacts_enabled;
            c.swing_cone = self.swing_cone.unwrap_or(-1.0);
            c
        }
    }

    static RIGID_BODY: Mirror = mirror::<RigidBody3D>();
    static COLLIDER: Mirror = mirror::<Collider3D>();
    static CHARACTER: Mirror = mirror::<Character3D>();
    static JOINT: Mirror = mirror::<Joint3D>();

    pub(crate) fn kind(t: std::any::TypeId) -> Option<Kind> {
        use std::any::TypeId;
        if t == TypeId::of::<RigidBody3D>() {
            Some(Kind::Mirror(&RIGID_BODY))
        } else if t == TypeId::of::<Collider3D>() {
            Some(Kind::Mirror(&COLLIDER))
        } else if t == TypeId::of::<Character3D>() {
            Some(Kind::Mirror(&CHARACTER))
        } else if t == TypeId::of::<Joint3D>() {
            Some(Kind::Mirror(&JOINT))
        } else if t == TypeId::of::<Velocity3D>() {
            // repr(C), layout checked against PonkoVelocity3D; any bits are valid.
            Some(Kind::Plain)
        } else if t == TypeId::of::<ponko::d3::Vehicle>() {
            // repr(C), layout checked against PonkoVehicle3D; floats, counts
            // (clamped to 8) and bools a C `bool` keeps 0 or 1.
            Some(Kind::Plain)
        } else {
            None
        }
    }

    pub(super) unsafe fn component(world: *mut PonkoWorld, out_id: *mut u32, which: u8) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        unsafe {
            match which {
                0 => builtin::<RigidBody3D>(world, out_id),
                1 => builtin::<Collider3D>(world, out_id),
                2 => builtin::<Velocity3D>(world, out_id),
                3 => builtin::<Character3D>(world, out_id),
                _ => builtin::<Joint3D>(world, out_id),
            }
        }
    }

    pub(super) unsafe fn joint_count(ctx: *mut PonkoCtx, out_count: *mut usize) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        boundary(|| unsafe { ctx_access(ctx, |c| out(out_count, c.physics3d.joint_count())) })
    }

    pub(super) unsafe fn game_physics3d(game: *mut PonkoGame, gravity: PonkoVec3) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        unsafe {
            build(game, |g| match finite(gravity, "gravity must be finite") {
                Ok(gravity) => ok(g.physics3d(gravity)),
                Err(f) => (g, Err(f)),
            })
        }
    }

    pub(super) unsafe fn set_gravity(ctx: *mut PonkoCtx, gravity: PonkoVec3) -> PonkoStatus {
        boundary(|| {
            let gravity = finite(gravity, "gravity must be finite")?;
            // SAFETY: forwarded caller contract.
            unsafe {
                ctx_access(ctx, |c| {
                    c.physics3d.set_gravity(gravity);
                    Ok(())
                })
            }
        })
    }

    /// `count` values at `ptr`, which may be null only for 0.
    unsafe fn slice<'a, T>(ptr: *const T, count: usize) -> Result<&'a [T], Failure> {
        if count == 0 {
            return Ok(&[]);
        }
        if ptr.is_null() {
            return Err(invalid("array is null"));
        }
        // SAFETY: caller supplies `count` readable values.
        Ok(unsafe { std::slice::from_raw_parts(ptr, count) })
    }

    pub(crate) fn mesh_to_c(m: CollisionMesh) -> PonkoCollisionMesh {
        PonkoCollisionMesh {
            id: u64::from(__ffi::id(m)),
        }
    }

    /// The handle a C collision mesh names. C keeps 64 bits for it; the
    /// engine's ids are 32, so a wider value names no mesh.
    pub(crate) fn mesh_from_c(m: PonkoCollisionMesh) -> CollisionMesh {
        u32::try_from(m.id).map_or(CollisionMesh::NONE, __ffi::from_id::<CollisionMesh>)
    }

    pub(super) unsafe fn heightfield(
        ctx: *mut PonkoCtx,
        heights: *const f32,
        count: usize,
        size: f32,
        out_mesh: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        boundary(|| {
            if out_mesh.is_null() {
                return Err(invalid("output is null"));
            }
            // SAFETY: forwarded caller contract.
            let heights = unsafe { slice(heights, count)? };
            let n = (count as f64).sqrt() as usize;
            if n < 2 || n * n != count {
                return Err(invalid("heights must be a square grid of at least 2 x 2"));
            }
            let size = positive(size, "size must be above 0")?;
            // SAFETY: forwarded caller contract.
            let mesh = unsafe { ctx_access(ctx, |c| Ok(c.physics3d.heightfield(heights, size)))? };
            // SAFETY: checked non-null.
            unsafe { out(out_mesh, mesh_to_c(mesh)) }
        })
    }

    pub(super) unsafe fn convex_hull(
        ctx: *mut PonkoCtx,
        points: *const PonkoVec3,
        count: usize,
        out_mesh: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        boundary(|| {
            if out_mesh.is_null() {
                return Err(invalid("output is null"));
            }
            // SAFETY: forwarded caller contract.
            let points: Vec<Vec3> = unsafe { slice(points, count)? }.iter().map(|&p| to_c(p)).collect();
            // SAFETY: forwarded caller contract.
            let mesh = unsafe { ctx_access(ctx, |c| Ok(c.physics3d.convex_hull(&points)))? }
                .ok_or_else(|| invalid("a convex hull needs points that aren't flat, all finite"))?;
            // SAFETY: checked non-null.
            unsafe { out(out_mesh, mesh_to_c(mesh)) }
        })
    }

    pub(super) unsafe fn trimesh(
        ctx: *mut PonkoCtx,
        vertices: *const PonkoVec3,
        vertex_count: usize,
        indices: *const u32,
        index_count: usize,
        out_mesh: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        boundary(|| {
            if out_mesh.is_null() {
                return Err(invalid("output is null"));
            }
            if !index_count.is_multiple_of(3) {
                return Err(invalid("three indices per triangle"));
            }
            // SAFETY: forwarded caller contract.
            let (vertices, indices) = unsafe { (slice(vertices, vertex_count)?, slice(indices, index_count)?) };
            let vertices: Vec<Vec3> = vertices.iter().map(|&p| to_c(p)).collect();
            let triangles: Vec<[u32; 3]> = indices.as_chunks::<3>().0.to_vec();
            // SAFETY: forwarded caller contract.
            let mesh = unsafe { ctx_access(ctx, |c| Ok(c.physics3d.trimesh(&vertices, &triangles)))? }
                .ok_or_else(|| invalid("a triangle mesh needs triangles, indices in range and finite vertices"))?;
            // SAFETY: checked non-null.
            unsafe { out(out_mesh, mesh_to_c(mesh)) }
        })
    }

    pub(super) unsafe fn free_mesh(ctx: *mut PonkoCtx, mesh: PonkoCollisionMesh) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        boundary(|| unsafe {
            ctx_access(ctx, |c| {
                c.physics3d.free(mesh_from_c(mesh));
                Ok(())
            })
        })
    }

    fn filter_from_c(f: PonkoQueryFilter3D) -> QueryFilter3D {
        let mut q = QueryFilter3D::default().groups(groups(f.groups));
        if f.exclude {
            q = q.exclude(Entity::from_bits(f.exclude_entity));
        }
        if f.sensors {
            q = q.with_sensors();
        }
        q
    }

    fn hit_to_c(h: Hit3D) -> PonkoHit3D {
        PonkoHit3D {
            entity: to_c(h.entity),
            point: to_c(h.point),
            normal: to_c(h.normal),
            distance: h.distance,
        }
    }

    pub(super) enum Cast {
        Ray,
        Sphere(f32),
        Capsule(f32, f32),
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn cast(
        ctx: *mut PonkoCtx,
        shape: Cast,
        origin: PonkoVec3,
        direction: PonkoVec3,
        max_distance: f32,
        filter: PonkoQueryFilter3D,
        out_found: *mut bool,
        out_hit: *mut PonkoHit3D,
    ) -> PonkoStatus {
        boundary(|| {
            if out_found.is_null() || out_hit.is_null() {
                return Err(invalid("output is null"));
            }
            let (origin, direction) = (to_c(origin), to_c(direction));
            let filter = filter_from_c(filter);
            // SAFETY: forwarded caller contract.
            let hit = unsafe {
                ctx_access(ctx, |c| {
                    let p = &c.physics3d;
                    Ok(match shape {
                        Cast::Ray => p.cast_ray(origin, direction, max_distance, filter),
                        Cast::Sphere(r) => p.cast_sphere(origin, r, direction, max_distance, filter),
                        Cast::Capsule(h, r) => p.cast_capsule(origin, h, r, direction, max_distance, filter),
                    })
                })?
            };
            // SAFETY: checked non-null; the caller supplies writable outputs.
            unsafe {
                if let Some(h) = hit {
                    out_hit.write(hit_to_c(h));
                }
                out(out_found, hit.is_some())
            }
        })
    }

    thread_local! {
        /// Reused between queries, so they don't allocate each time.
        static HITS: RefCell<Vec<Hit3D>> = const { RefCell::new(Vec::new()) };
        static FOUND: RefCell<Vec<Entity>> = const { RefCell::new(Vec::new()) };
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn cast_all(
        ctx: *mut PonkoCtx,
        origin: PonkoVec3,
        direction: PonkoVec3,
        max_distance: f32,
        filter: PonkoQueryFilter3D,
        out_hits: *mut PonkoHit3D,
        capacity: usize,
        out_count: *mut usize,
    ) -> PonkoStatus {
        boundary(|| {
            HITS.with_borrow_mut(|hits| {
                let filter = filter_from_c(filter);
                // SAFETY: forwarded caller contract.
                unsafe {
                    ctx_access(ctx, |c| {
                        c.physics3d
                            .cast_ray_all(to_c(origin), to_c(direction), max_distance, filter, hits);
                        Ok(())
                    })?;
                    let mut n = 0;
                    collect(out_hits, capacity, out_count, &mut n, |sink| {
                        for &h in hits.iter() {
                            sink.push(hit_to_c(h));
                        }
                    })
                }
            })
        })
    }

    pub(super) enum Overlap {
        Sphere(f32),
        Box(PonkoVec3, PonkoQuat),
        Capsule(f32, f32),
    }

    pub(super) unsafe fn overlap(
        ctx: *mut PonkoCtx,
        shape: Overlap,
        center: PonkoVec3,
        filter: PonkoQueryFilter3D,
        out_entities: *mut PonkoEntity,
        capacity: usize,
        out_count: *mut usize,
    ) -> PonkoStatus {
        boundary(|| {
            FOUND.with_borrow_mut(|found| {
                let (center, filter) = (to_c(center), filter_from_c(filter));
                // SAFETY: forwarded caller contract.
                unsafe {
                    ctx_access(ctx, |c| {
                        let p = &c.physics3d;
                        match shape {
                            Overlap::Sphere(r) => p.overlap_sphere(center, r, filter, found),
                            Overlap::Box(size, q) => {
                                let q = Quat::from_xyzw(q.x, q.y, q.z, q.w);
                                p.overlap_box(center, to_c(size), q, filter, found)
                            }
                            Overlap::Capsule(h, r) => p.overlap_capsule(center, h, r, filter, found),
                        }
                        Ok(())
                    })?;
                    let mut n = 0;
                    collect(out_entities, capacity, out_count, &mut n, |sink| {
                        for &e in found.iter() {
                            sink.push(to_c(e));
                        }
                    })
                }
            })
        })
    }

    pub(super) unsafe fn apply_impulse(ctx: *mut PonkoCtx, entity: u64, impulse: PonkoVec3) -> PonkoStatus {
        boundary(|| {
            let impulse = finite(impulse, "impulse must be finite")?;
            // SAFETY: forwarded caller contract.
            let applied = unsafe {
                ctx_access(ctx, |c| {
                    Ok(c.physics3d.apply_impulse(Entity::from_bits(entity), impulse))
                })?
            };
            if applied {
                Ok(())
            } else {
                Err(error(PonkoStatus::PONKO_NOT_FOUND, "entity is not a 3D physics body"))
            }
        })
    }

    pub(super) unsafe fn center_of_mass(ctx: *mut PonkoCtx, entity: u64, out_center: *mut PonkoVec3) -> PonkoStatus {
        boundary(|| {
            // SAFETY: forwarded caller contract.
            let c = unsafe { ctx_access(ctx, |c| Ok(c.physics3d.center_of_mass(Entity::from_bits(entity))))? }
                .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "entity is not a 3D physics body"))?;
            // SAFETY: forwarded caller contract.
            unsafe { out(out_center, to_c(c)) }
        })
    }

    pub(super) unsafe fn mass(ctx: *mut PonkoCtx, entity: u64, out_mass: *mut f32) -> PonkoStatus {
        boundary(|| {
            // SAFETY: forwarded caller contract.
            let mass = unsafe { ctx_access(ctx, |c| Ok(c.physics3d.mass(Entity::from_bits(entity))))? }
                .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "entity is not a 3D physics body"))?;
            // SAFETY: forwarded caller contract.
            unsafe { out(out_mass, mass) }
        })
    }
}

#[cfg(not(feature = "physics3d"))]
mod imp {
    //! Without the feature: every call fails, saying why.
    use super::*;
    use crate::{Failure, Kind, PonkoCtx, boundary, invalid};

    fn off() -> Result<(), Failure> {
        Err(invalid("ponko_ffi was built without the `physics3d` feature"))
    }
    pub(crate) fn kind(_: std::any::TypeId) -> Option<Kind> {
        None
    }
    #[allow(dead_code)]
    pub(super) enum Cast {
        Ray,
        Sphere(f32),
        Capsule(f32, f32),
    }
    #[allow(dead_code)]
    pub(super) enum Overlap {
        Sphere(f32),
        Box(PonkoVec3, PonkoQuat),
        Capsule(f32, f32),
    }
    pub(super) unsafe fn component(_: *mut PonkoWorld, _: *mut u32, _: u8) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn joint_count(_: *mut PonkoCtx, _: *mut usize) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn game_physics3d(_: *mut PonkoGame, _: PonkoVec3) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn set_gravity(_: *mut PonkoCtx, _: PonkoVec3) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn heightfield(
        _: *mut PonkoCtx,
        _: *const f32,
        _: usize,
        _: f32,
        _: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn convex_hull(
        _: *mut PonkoCtx,
        _: *const PonkoVec3,
        _: usize,
        _: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn trimesh(
        _: *mut PonkoCtx,
        _: *const PonkoVec3,
        _: usize,
        _: *const u32,
        _: usize,
        _: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn free_mesh(_: *mut PonkoCtx, _: PonkoCollisionMesh) -> PonkoStatus {
        boundary(off)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn cast(
        _: *mut PonkoCtx,
        _: Cast,
        _: PonkoVec3,
        _: PonkoVec3,
        _: f32,
        _: PonkoQueryFilter3D,
        _: *mut bool,
        _: *mut PonkoHit3D,
    ) -> PonkoStatus {
        boundary(off)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn cast_all(
        _: *mut PonkoCtx,
        _: PonkoVec3,
        _: PonkoVec3,
        _: f32,
        _: PonkoQueryFilter3D,
        _: *mut PonkoHit3D,
        _: usize,
        _: *mut usize,
    ) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn overlap(
        _: *mut PonkoCtx,
        _: Overlap,
        _: PonkoVec3,
        _: PonkoQueryFilter3D,
        _: *mut PonkoEntity,
        _: usize,
        _: *mut usize,
    ) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn apply_impulse(_: *mut PonkoCtx, _: u64, _: PonkoVec3) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn mass(_: *mut PonkoCtx, _: u64, _: *mut f32) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn center_of_mass(_: *mut PonkoCtx, _: u64, _: *mut PonkoVec3) -> PonkoStatus {
        boundary(off)
    }
}

pub(crate) use imp::kind as kind3d;
#[cfg(feature = "physics3d")]
pub(crate) use imp::{mesh_from_c, mesh_to_c};
