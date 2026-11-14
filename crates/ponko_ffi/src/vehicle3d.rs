//! Raycast-wheel vehicles from C (the `physics3d` feature): the
//! `Vehicle3D` component with its wheels and friction curves. Without the
//! feature the component calls fail with an error naming it; the value
//! helpers still work.
#![cfg_attr(not(feature = "physics3d"), allow(unused_variables))]

use crate::three::{PonkoQuat, PonkoTransform3D, PonkoVec3, transform_from_c, transform_to_c};
use crate::{PonkoStatus, PonkoWorld};

/// How much grip a tyre has for how much it slips (Rust `FrictionCurve`):
/// rising from 0 to its peak `(extremum_slip, extremum_value)`, falling to
/// `(asymptote_slip, asymptote_value)` and staying there. The value times
/// `stiffness` times the wheel's load is the tyre's force.
/// ```
/// let c = ponko_ffi::ponko_friction_curve_forward();
/// assert_eq!((c.extremum_slip, c.asymptote_value), (0.4, 0.5));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoFrictionCurve {
    /// The slip where grip peaks.
    pub extremum_slip: f32,
    /// The grip there.
    pub extremum_value: f32,
    /// The slip from which on the tyre slides.
    pub asymptote_slip: f32,
    /// The grip while sliding.
    pub asymptote_value: f32,
    /// Scales the whole curve: 1 normal, lower for wet or icy ground.
    pub stiffness: f32,
}

/// One wheel of a `PonkoVehicle3D` (Rust `Wheel3D`): setup, inputs you set
/// each tick, and outputs as of the last physics step. Start from
/// `ponko_wheel3d(position, radius)`. The suspension hangs down the
/// chassis's -Y from `position` (the wheel's center at full compression).
/// ```
/// let w = ponko_ffi::ponko_wheel3d(Default::default(), 0.35);
/// assert_eq!((w.radius, w.spring, w.target_position), (0.35, 35000.0, 0.5));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoWheel3D {
    /// The wheel's center at full compression, in the chassis's frame.
    pub position: PonkoVec3,
    /// Meters.
    pub radius: f32,
    /// kg: sets how fast torque spins it up.
    pub mass: f32,
    /// How far the suspension travels, meters.
    pub suspension_distance: f32,
    /// Spring rate, N/m.
    pub spring: f32,
    /// Damper rate, N·s/m.
    pub damper: f32,
    /// Where it rests under the car's weight: 0 fully compressed, 1 fully extended.
    pub target_position: f32,
    /// Where the forces act, up the suspension from the resting wheel's bottom.
    pub force_app_point_distance: f32,
    /// Slows the spin by this many N·m per rad/s.
    pub damping_rate: f32,
    /// Grip along the rolling direction.
    pub forward_friction: PonkoFrictionCurve,
    /// Grip across it.
    pub sideways_friction: PonkoFrictionCurve,
    /// Input: drive torque, N·m; positive rolls forward (the chassis's -Z).
    pub motor_torque: f32,
    /// Input: brake torque, N·m.
    pub brake_torque: f32,
    /// Input: steering, radians; positive steers right.
    pub steer_angle: f32,
    /// Output: the wheel's turn relative to the chassis (steer, then spin).
    pub local_rotation: PonkoQuat,
    /// Output: touches the ground.
    pub grounded: bool,
    /// Output: where the tyre touches the ground.
    pub contact_point: PonkoVec3,
    /// Output: the ground's outward direction there.
    pub contact_normal: PonkoVec3,
    /// Output: how far the suspension is pushed in from full extension, meters.
    pub compression: f32,
    /// Output: the suspension's push (the tyre's load), newtons.
    pub suspension_force: f32,
    /// Output: the share of the chassis's mass on this wheel, kg.
    pub sprung_mass: f32,
    /// Output: spin in rad/s; positive rolls forward.
    pub angular_velocity: f32,
    /// Output: forward slip.
    pub forward_slip: f32,
    /// Output: sideways slip, radians.
    pub sideways_slip: f32,
    /// Output: how far it has turned, radians (0..2π).
    pub spin: f32,
    /// Output: the wheel's center in the chassis's frame, suspension travel included.
    pub local_position: PonkoVec3,
    /// Unused; keeps the Rust size.
    pub reserved: [f32; 3],
}

/// The built-in `Vehicle3D` component: up to 8 raycast wheels holding up
/// the entity's chassis (a dynamic `PonkoRigidBody3D` with a collider and
/// a transform, facing -Z). Only the first `wheel_count` wheels are used.
/// Start from `ponko_vehicle3d()` and `ponko_vehicle3d_add_wheel`. A plain
/// value (same layout as Rust's), so it also works as a query column.
/// ```
/// let v = ponko_ffi::ponko_vehicle3d();
/// assert_eq!((v.wheel_count, v.groups.filter), (0, u32::MAX));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoVehicle3D {
    /// The wheels.
    pub wheels: [PonkoWheel3D; 8],
    /// How many of `wheels` there are.
    pub wheel_count: u32,
    /// Which colliders the wheels roll on.
    pub groups: crate::PonkoCollisionGroups,
    /// Unused; keeps the Rust size.
    pub reserved: u32,
}

/// The usual forward (rolling) grip: peak 1 at slip 0.4, sliding 0.5 from 0.8.
/// ```
/// assert_eq!(ponko_ffi::ponko_friction_curve_forward().stiffness, 1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_friction_curve_forward() -> PonkoFrictionCurve {
    PonkoFrictionCurve {
        extremum_slip: 0.4,
        extremum_value: 1.0,
        asymptote_slip: 0.8,
        asymptote_value: 0.5,
        stiffness: 1.0,
    }
}

/// The usual sideways grip: peak 1 at 0.2 rad, sliding 0.75 from 0.5 rad.
/// ```
/// assert_eq!(ponko_ffi::ponko_friction_curve_sideways().extremum_slip, 0.2);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_friction_curve_sideways() -> PonkoFrictionCurve {
    PonkoFrictionCurve {
        extremum_slip: 0.2,
        extremum_value: 1.0,
        asymptote_slip: 0.5,
        asymptote_value: 0.75,
        stiffness: 1.0,
    }
}

/// The grip of `curve` at `slip` (either sign), stiffness included.
/// ```
/// let c = ponko_ffi::ponko_friction_curve_forward();
/// assert_eq!(ponko_ffi::ponko_friction_curve_evaluate(c, -0.4), 1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_friction_curve_evaluate(curve: PonkoFrictionCurve, slip: f32) -> f32 {
    imp::evaluate(curve, slip)
}

/// A wheel of `radius` at `position` (its center at full compression, in
/// the chassis's frame) with the usual suspension (0.3 m travel, 35 000
/// N/m, 4 500 N·s/m, resting halfway), 20 kg and the usual tyres.
/// ```
/// let w = ponko_ffi::ponko_wheel3d(ponko_ffi::PonkoVec3 { x: 1.0, y: 0.0, z: 0.0 }, 0.4);
/// assert_eq!(w.local_position.y, -0.3);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_wheel3d(position: PonkoVec3, radius: f32) -> PonkoWheel3D {
    PonkoWheel3D {
        position,
        radius,
        mass: 20.0,
        suspension_distance: 0.3,
        spring: 35_000.0,
        damper: 4_500.0,
        target_position: 0.5,
        force_app_point_distance: 0.0,
        damping_rate: 0.25,
        forward_friction: ponko_friction_curve_forward(),
        sideways_friction: ponko_friction_curve_sideways(),
        motor_torque: 0.0,
        brake_torque: 0.0,
        steer_angle: 0.0,
        local_rotation: PonkoQuat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        grounded: false,
        contact_point: PonkoVec3::default(),
        contact_normal: PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 },
        compression: 0.0,
        suspension_force: 0.0,
        sprung_mass: 0.0,
        angular_velocity: 0.0,
        forward_slip: 0.0,
        sideways_slip: 0.0,
        spin: 0.0,
        local_position: PonkoVec3 {
            x: position.x,
            y: position.y - 0.3,
            z: position.z,
        },
        reserved: [0.0; 3],
    }
}

/// A vehicle with no wheels yet, rolling on everything.
/// ```
/// assert_eq!(ponko_ffi::ponko_vehicle3d().wheels[7].radius, 0.5);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_vehicle3d() -> PonkoVehicle3D {
    PonkoVehicle3D {
        wheels: [ponko_wheel3d(PonkoVec3::default(), 0.5); 8],
        wheel_count: 0,
        groups: crate::ponko_collision_groups_all(),
        reserved: 0,
    }
}

/// Adds `wheel` to `vehicle`. `PONKO_INVALID_ARGUMENT` if it already has 8
/// (or `vehicle` is null).
/// # Safety
/// `vehicle` must be null or writable.
/// ```
/// use ponko_ffi::*;
/// let mut v = ponko_vehicle3d();
/// let w = ponko_wheel3d(Default::default(), 0.3);
/// for _ in 0..8 { assert_eq!(unsafe { ponko_vehicle3d_add_wheel(&mut v, w) }, PonkoStatus::PONKO_OK); }
/// assert_eq!(unsafe { ponko_vehicle3d_add_wheel(&mut v, w) }, PonkoStatus::PONKO_INVALID_ARGUMENT);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_vehicle3d_add_wheel(vehicle: *mut PonkoVehicle3D, wheel: PonkoWheel3D) -> PonkoStatus {
    crate::boundary(|| {
        // SAFETY: caller supplies null or a writable vehicle.
        let Some(v) = (unsafe { vehicle.as_mut() }) else {
            return Err(crate::invalid("vehicle is null"));
        };
        let n = v.wheel_count as usize;
        if n >= v.wheels.len() {
            return Err(crate::invalid("a vehicle has at most 8 wheels"));
        }
        v.wheels[n] = wheel;
        v.wheel_count = n as u32 + 1;
        Ok(())
    })
}

/// Where to draw `wheel` for a chassis at `chassis`: its center, with
/// steer, spin and suspension travel (Rust `Wheel3D::pose`).
/// # Safety
/// `wheel` must be null (gives `chassis` back) or readable.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_wheel3d(PonkoVec3 { x: 1.0, y: 0.0, z: 0.0 }, 0.4);
/// let p = unsafe { ponko_wheel3d_pose(&w, ponko_transform3d_at(PonkoVec3 { x: 0.0, y: 2.0, z: 0.0 })) };
/// assert!((p.position.y - 1.7).abs() < 1e-5);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_wheel3d_pose(wheel: *const PonkoWheel3D, chassis: PonkoTransform3D) -> PonkoTransform3D {
    use ponko::prelude::{Quat, Vec3};
    // SAFETY: caller supplies null or a readable wheel.
    let Some(w) = (unsafe { wheel.as_ref() }) else {
        return chassis;
    };
    let t = transform_from_c(chassis);
    let (r, p) = (w.local_rotation, w.local_position);
    let mut pose = t;
    pose.position = t.position + t.rotation * Vec3::new(p.x, p.y, p.z);
    pose.rotation = t.rotation * Quat::from_xyzw(r.x, r.y, r.z, r.w);
    pose.scale = Vec3::ONE;
    transform_to_c(pose)
}

/// The id of the built-in `Vehicle3D` component (`PonkoVehicle3D`, also a
/// query column), registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_vehicle3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_vehicle3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id) }
}

#[cfg(feature = "physics3d")]
mod imp {
    use std::mem::offset_of;

    use ponko::d3::FrictionCurve;
    use ponko::d3::Vehicle as Vehicle3D;
    use ponko::d3::Wheel as Wheel3D;

    use super::*;
    use crate::builtin;

    // The component is stored as Rust's Vehicle3D and seen by C as
    // PonkoVehicle3D: every field at the same place (C's padding is spelled
    // out as `reserved`).
    const _: () = {
        assert!(size_of::<PonkoFrictionCurve>() == size_of::<FrictionCurve>());
        assert!(offset_of!(PonkoFrictionCurve, extremum_slip) == offset_of!(FrictionCurve, extremum_slip));
        assert!(offset_of!(PonkoFrictionCurve, extremum_value) == offset_of!(FrictionCurve, extremum_value));
        assert!(offset_of!(PonkoFrictionCurve, asymptote_slip) == offset_of!(FrictionCurve, asymptote_slip));
        assert!(offset_of!(PonkoFrictionCurve, asymptote_value) == offset_of!(FrictionCurve, asymptote_value));
        assert!(offset_of!(PonkoFrictionCurve, stiffness) == offset_of!(FrictionCurve, stiffness));

        assert!(size_of::<PonkoWheel3D>() == size_of::<Wheel3D>());
        assert!(offset_of!(PonkoWheel3D, position) == offset_of!(Wheel3D, position));
        assert!(offset_of!(PonkoWheel3D, radius) == offset_of!(Wheel3D, radius));
        assert!(offset_of!(PonkoWheel3D, mass) == offset_of!(Wheel3D, mass));
        assert!(offset_of!(PonkoWheel3D, suspension_distance) == offset_of!(Wheel3D, suspension_distance));
        assert!(offset_of!(PonkoWheel3D, spring) == offset_of!(Wheel3D, spring));
        assert!(offset_of!(PonkoWheel3D, damper) == offset_of!(Wheel3D, damper));
        assert!(offset_of!(PonkoWheel3D, target_position) == offset_of!(Wheel3D, target_position));
        assert!(offset_of!(PonkoWheel3D, force_app_point_distance) == offset_of!(Wheel3D, force_app_point_distance));
        assert!(offset_of!(PonkoWheel3D, damping_rate) == offset_of!(Wheel3D, damping_rate));
        assert!(offset_of!(PonkoWheel3D, forward_friction) == offset_of!(Wheel3D, forward_friction));
        assert!(offset_of!(PonkoWheel3D, sideways_friction) == offset_of!(Wheel3D, sideways_friction));
        assert!(offset_of!(PonkoWheel3D, motor_torque) == offset_of!(Wheel3D, motor_torque));
        assert!(offset_of!(PonkoWheel3D, brake_torque) == offset_of!(Wheel3D, brake_torque));
        assert!(offset_of!(PonkoWheel3D, steer_angle) == offset_of!(Wheel3D, steer_angle));
        assert!(offset_of!(PonkoWheel3D, local_rotation) == offset_of!(Wheel3D, local_rotation));
        assert!(offset_of!(PonkoWheel3D, grounded) == offset_of!(Wheel3D, grounded));
        assert!(offset_of!(PonkoWheel3D, contact_point) == offset_of!(Wheel3D, contact_point));
        assert!(offset_of!(PonkoWheel3D, contact_normal) == offset_of!(Wheel3D, contact_normal));
        assert!(offset_of!(PonkoWheel3D, compression) == offset_of!(Wheel3D, compression));
        assert!(offset_of!(PonkoWheel3D, suspension_force) == offset_of!(Wheel3D, suspension_force));
        assert!(offset_of!(PonkoWheel3D, sprung_mass) == offset_of!(Wheel3D, sprung_mass));
        assert!(offset_of!(PonkoWheel3D, angular_velocity) == offset_of!(Wheel3D, angular_velocity));
        assert!(offset_of!(PonkoWheel3D, forward_slip) == offset_of!(Wheel3D, forward_slip));
        assert!(offset_of!(PonkoWheel3D, sideways_slip) == offset_of!(Wheel3D, sideways_slip));
        assert!(offset_of!(PonkoWheel3D, spin) == offset_of!(Wheel3D, spin));
        assert!(offset_of!(PonkoWheel3D, local_position) == offset_of!(Wheel3D, local_position));

        assert!(size_of::<PonkoVehicle3D>() == size_of::<Vehicle3D>());
        assert!(offset_of!(PonkoVehicle3D, wheels) == offset_of!(Vehicle3D, wheels));
        assert!(offset_of!(PonkoVehicle3D, wheel_count) == offset_of!(Vehicle3D, wheel_count));
        assert!(offset_of!(PonkoVehicle3D, groups) == offset_of!(Vehicle3D, groups));
    };

    fn curve(c: PonkoFrictionCurve) -> FrictionCurve {
        FrictionCurve {
            extremum_slip: c.extremum_slip,
            extremum_value: c.extremum_value,
            asymptote_slip: c.asymptote_slip,
            asymptote_value: c.asymptote_value,
            stiffness: c.stiffness,
        }
    }

    pub(super) fn evaluate(c: PonkoFrictionCurve, slip: f32) -> f32 {
        curve(c).evaluate(slip)
    }

    pub(super) unsafe fn component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        unsafe { builtin::<Vehicle3D>(world, out_id) }
    }
}

#[cfg(not(feature = "physics3d"))]
mod imp {
    //! Without the feature: the component can't be used, saying why.
    use super::*;
    use crate::{boundary, invalid};

    pub(super) fn evaluate(c: PonkoFrictionCurve, slip: f32) -> f32 {
        // The same curve as the engine's, for builds without it.
        let s = slip.abs();
        let k = c.stiffness;
        if s < c.extremum_slip {
            let t = s / c.extremum_slip;
            c.extremum_value * (1.5 * t - 0.5 * t * t * t) * k
        } else if s < c.asymptote_slip {
            let t = (s - c.extremum_slip) / (c.asymptote_slip - c.extremum_slip);
            (c.extremum_value + (c.asymptote_value - c.extremum_value) * t * t * (3.0 - 2.0 * t)) * k
        } else if s <= c.extremum_slip {
            c.extremum_value * k
        } else {
            c.asymptote_value * k
        }
    }
    pub(super) unsafe fn component(_: *mut PonkoWorld, _: *mut u32) -> PonkoStatus {
        boundary(|| Err(invalid("ponko_ffi was built without the `physics3d` feature")))
    }
}
