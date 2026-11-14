//! Raycast-wheel vehicles: a dynamic chassis body held up by spring-damper
//! suspension and driven by slip-based tyre friction, the classic
//! wheel-collider model. Steering, gears and the engine are the game's:
//! it sets each wheel's torques and steer angle every tick.

use rapier3d::prelude as rp;
use serde::{Deserialize, Serialize};

use super::{CollisionGroups, LinkKind, Physics3D, from_rp, rot_from_rp, to_rp};
use crate::collision::BodyKind;
use crate::d3::Transform as Transform3D;
use glam::{Quat, Vec3};
use ponko_ecs::{Component, Entity, World};

/// How much grip a tyre has for how much it slips: a curve rising from 0
/// to its peak `(extremum_slip, extremum_value)`, falling to
/// `(asymptote_slip, asymptote_value)` and staying there, both pieces
/// smooth and flat at their ends. The value times `stiffness` times the
/// load on the wheel is the tyre's force.
///
/// Forward slip is how much faster the tyre's surface turns than the road
/// passes under it (0 rolling freely, 1 spinning twice as fast, -1 a
/// locked wheel sliding); sideways slip is the angle in radians between
/// where the wheel points and where it goes.
///
/// ```
/// use ponko::prelude::*;
/// let grip = ponko::d3::FrictionCurve::FORWARD;
/// assert_eq!(grip.evaluate(0.0), 0.0);
/// assert_eq!(grip.evaluate(grip.extremum_slip), 1.0); // the peak
/// assert_eq!(grip.evaluate(5.0), 0.5); // sliding
/// let icy = ponko::d3::FrictionCurve { stiffness: 0.2, ..ponko::d3::FrictionCurve::SIDEWAYS };
/// assert!(icy.evaluate(0.1) < ponko::d3::FrictionCurve::SIDEWAYS.evaluate(0.1));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[repr(C)] // C sees it as PonkoFrictionCurve.
pub struct FrictionCurve {
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

impl Default for FrictionCurve {
    fn default() -> Self {
        Self::FORWARD
    }
}

impl FrictionCurve {
    /// The usual curve for rolling (forward) grip: peak 1 at slip 0.4,
    /// sliding 0.5 from 0.8.
    pub const FORWARD: Self = Self {
        extremum_slip: 0.4,
        extremum_value: 1.0,
        asymptote_slip: 0.8,
        asymptote_value: 0.5,
        stiffness: 1.0,
    };
    /// The usual curve for sideways grip: peak 1 at 0.2 rad, sliding 0.75
    /// from 0.5 rad.
    pub const SIDEWAYS: Self = Self {
        extremum_slip: 0.2,
        extremum_value: 1.0,
        asymptote_slip: 0.5,
        asymptote_value: 0.75,
        stiffness: 1.0,
    };

    /// The grip at `slip` (either sign), stiffness included.
    #[must_use]
    pub fn evaluate(&self, slip: f32) -> f32 {
        self.value_and_slope(slip.abs()).0
    }

    /// Grip at `s` ≥ 0 and its slope there, stiffness included.
    fn value_and_slope(&self, s: f32) -> (f32, f32) {
        let (ex_s, ex_v) = (self.extremum_slip, self.extremum_value);
        let (as_s, as_v) = (self.asymptote_slip, self.asymptote_value);
        let k = self.stiffness;
        if !s.is_finite() {
            return (as_v * k, 0.0);
        }
        if s < ex_s {
            // Hermite from (0, 0) to the peak, flat there.
            let t = s / ex_s;
            let v = ex_v * (1.5 * t - 0.5 * t * t * t);
            let slope = ex_v * 1.5 * (1.0 - t * t) / ex_s;
            (v * k, slope * k)
        } else if s < as_s {
            // Smooth step from the peak down to sliding, flat at both ends.
            let span = as_s - ex_s;
            let t = (s - ex_s) / span;
            let v = ex_v + (as_v - ex_v) * t * t * (3.0 - 2.0 * t);
            let slope = (as_v - ex_v) * 6.0 * t * (1.0 - t) / span;
            (v * k, slope * k)
        } else if s <= ex_s {
            (ex_v * k, 0.0)
        } else {
            (as_v * k, 0.0)
        }
    }
}

/// One wheel of a [`Vehicle3D`]: where it's mounted, its suspension and
/// tyres (set these up once), what drives it this tick (yours to set), and
/// what happened to it in the last physics step (read after the step).
///
/// The suspension hangs straight down from [`position`](Self::position)
/// along the chassis's -Y: at full compression the wheel's center is at
/// `position`, fully extended it's [`suspension_distance`](Self::suspension_distance)
/// lower. Under the car's weight it rests at
/// [`target_position`](Self::target_position) of that travel.
///
/// ```
/// use ponko::prelude::*;
/// // Front left wheel of a car whose chassis faces -Z.
/// let mut wheel = ponko::d3::Wheel::new(vec3(-0.8, -0.2, -1.3), 0.35);
/// wheel.spring = 40_000.0;
/// wheel.sideways_friction.stiffness = 1.2;
/// // Each tick:
/// wheel.motor_torque = 300.0;
/// wheel.steer_angle = 0.3; // right
/// assert!(!wheel.grounded);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[repr(C, align(16))] // C sees it as PonkoWheel3D.
pub struct Wheel3D {
    // Setup.
    /// Where the wheel's center is at full compression, in the chassis's
    /// frame (relative to its [`d3::Transform`](crate::d3::Transform) position).
    pub position: Vec3,
    /// The wheel's radius in meters. Default 0.5.
    pub radius: f32,
    /// The wheel's mass in kg, which sets how fast torque spins it up.
    /// Default 20.
    pub mass: f32,
    /// How far the suspension travels, meters. Default 0.3.
    pub suspension_distance: f32,
    /// The suspension's spring rate, N/m. Default 35 000.
    pub spring: f32,
    /// The suspension's damper rate, N·s/m. Default 4 500.
    pub damper: f32,
    /// Where the suspension rests under the car's weight: 0 fully
    /// compressed, 1 fully extended. Default 0.5.
    pub target_position: f32,
    /// Where the suspension and tyre forces act: this far up the
    /// suspension from the bottom of the resting wheel. 0 (the default)
    /// at the tyre's contact; near the center of mass rolls less.
    pub force_app_point_distance: f32,
    /// Slows the wheel's spin by this many N·m per rad/s (rolling
    /// resistance, bearings). Default 0.25.
    pub damping_rate: f32,
    /// Grip along the wheel's rolling direction.
    pub forward_friction: FrictionCurve,
    /// Grip across it.
    pub sideways_friction: FrictionCurve,

    // Inputs: yours to set every tick.
    /// Drive torque, N·m: positive rolls forward (the chassis's -Z).
    pub motor_torque: f32,
    /// Brake torque, N·m (positive): slows the wheel's spin to a stop.
    pub brake_torque: f32,
    /// Steering, radians around the chassis's up axis: positive steers
    /// right (clockwise seen from above).
    pub steer_angle: f32,

    // Outputs: as of the last step.
    /// The wheel's turn relative to the chassis (steer, then spin): for
    /// drawing it, with [`local_position`](Self::local_position) or
    /// [`pose`](Self::pose).
    pub local_rotation: Quat,
    /// Touches the ground.
    pub grounded: bool,
    /// Where the tyre touches the ground (when `grounded`).
    pub contact_point: Vec3,
    /// The ground's outward direction there (when `grounded`).
    pub contact_normal: Vec3,
    /// How far the suspension is pushed in from full extension, meters
    /// (0 in the air, `suspension_distance` bottomed out).
    pub compression: f32,
    /// The suspension's push, newtons: the load on the tyre.
    pub suspension_force: f32,
    /// The share of the chassis's mass this wheel carries, kg.
    pub sprung_mass: f32,
    /// How fast the wheel spins, rad/s: positive rolls forward. See [`rpm`](Self::rpm).
    pub angular_velocity: f32,
    /// Forward slip (see [`FrictionCurve`]).
    pub forward_slip: f32,
    /// Sideways slip, radians: positive slides to the wheel's right.
    pub sideways_slip: f32,
    /// How far the wheel has turned, radians (0..2π), for drawing.
    pub spin: f32,
    /// The wheel's center in the chassis's frame, suspension travel
    /// included: for drawing it.
    pub local_position: Vec3,
}

impl Default for Wheel3D {
    fn default() -> Self {
        Self::new(Vec3::ZERO, 0.5)
    }
}

impl Wheel3D {
    /// A wheel of `radius` mounted at `position` (its center at full
    /// compression, in the chassis's frame), with the usual suspension and
    /// tyres.
    #[must_use]
    pub fn new(position: Vec3, radius: f32) -> Self {
        let suspension_distance = 0.3;
        Self {
            position,
            radius,
            mass: 20.0,
            suspension_distance,
            spring: 35_000.0,
            damper: 4_500.0,
            target_position: 0.5,
            force_app_point_distance: 0.0,
            damping_rate: 0.25,
            forward_friction: FrictionCurve::FORWARD,
            sideways_friction: FrictionCurve::SIDEWAYS,
            motor_torque: 0.0,
            brake_torque: 0.0,
            steer_angle: 0.0,
            local_rotation: Quat::IDENTITY,
            grounded: false,
            contact_point: Vec3::ZERO,
            contact_normal: Vec3::Y,
            compression: 0.0,
            suspension_force: 0.0,
            sprung_mass: 0.0,
            angular_velocity: 0.0,
            forward_slip: 0.0,
            sideways_slip: 0.0,
            spin: 0.0,
            local_position: position - Vec3::Y * suspension_distance,
        }
    }

    /// How fast the wheel spins, in revolutions per minute.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut wheel = ponko::d3::Wheel::new(Vec3::ZERO, 0.3);
    /// wheel.angular_velocity = std::f32::consts::TAU; // one turn a second
    /// assert!((wheel.rpm() - 60.0).abs() < 1e-3);
    /// ```
    #[must_use]
    pub fn rpm(&self) -> f32 {
        self.angular_velocity * 60.0 / std::f32::consts::TAU
    }

    /// Where to draw the wheel for a chassis at `chassis`: its center,
    /// with steer, spin and suspension travel. Pass the chassis's
    /// [`d3::Transform`](crate::d3::Transform) in a frame system and the wheel moves as smoothly as
    /// the chassis does.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let wheel = ponko::d3::Wheel::new(vec3(1.0, 0.0, 0.0), 0.4);
    /// let pose = wheel.pose(&ponko::d3::Transform::at(vec3(0.0, 2.0, 0.0)));
    /// // Hanging fully extended: 0.3 m below its mount.
    /// assert!((pose.position - vec3(1.0, 1.7, 0.0)).length() < 1e-5);
    /// ```
    #[must_use]
    pub fn pose(&self, chassis: &Transform3D) -> Transform3D {
        Transform3D {
            position: chassis.position + chassis.rotation * self.local_position,
            rotation: chassis.rotation * self.local_rotation,
            scale: Vec3::ONE,
        }
    }
}

/// A car, truck or any wheeled vehicle: up to [`MAX_WHEELS`](Self::MAX_WHEELS)
/// raycast [`Wheel3D`]s holding up the entity's chassis, which needs a
/// dynamic [`RigidBody3D`](super::RigidBody3D), a [`Collider3D`](super::Collider3D)
/// and a [`d3::Transform`](crate::d3::Transform). The chassis faces its -Z.
///
/// Each physics step casts a ray down from every wheel's mount, pushes the
/// chassis up with the suspension, spins the wheels by their torques and
/// the road, and pushes the chassis along by the tyres' grip. Set the
/// wheels' `motor_torque`, `brake_torque` and `steer_angle` in a simulation
/// system before the step; gears, differentials and steering assists are
/// the game's.
///
/// ```
/// use ponko::prelude::*;
/// let mut car = ponko::d3::Vehicle::new();
/// for (x, z) in [(-0.8, -1.3), (0.8, -1.3), (-0.8, 1.3), (0.8, 1.3)] {
///     car = car.with_wheel(ponko::d3::Wheel::new(vec3(x, -0.2, z), 0.35));
/// }
/// assert_eq!(car.wheels().len(), 4);
/// // Rear-wheel drive, front-wheel steering:
/// for (i, wheel) in car.wheels_mut().iter_mut().enumerate() {
///     if i < 2 {
///         wheel.steer_angle = 0.2;
///     } else {
///         wheel.motor_torque = 400.0;
///     }
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Component)]
#[serde(default)]
#[repr(C, align(16))] // C sees it as PonkoVehicle3D.
pub struct Vehicle3D {
    /// The wheels; only the first [`wheel_count`](Self::wheel_count) are
    /// used (see [`wheels`](Self::wheels)).
    pub wheels: [Wheel3D; Vehicle3D::MAX_WHEELS],
    /// How many of `wheels` there are.
    pub wheel_count: u32,
    /// Which colliders the wheels roll on.
    pub groups: CollisionGroups,
}

impl Default for Vehicle3D {
    fn default() -> Self {
        Self::new()
    }
}

impl Vehicle3D {
    /// The most wheels a vehicle has.
    pub const MAX_WHEELS: usize = 8;

    /// A vehicle with no wheels yet, rolling on everything.
    #[must_use]
    pub fn new() -> Self {
        Self {
            wheels: [Wheel3D::default(); Self::MAX_WHEELS],
            wheel_count: 0,
            groups: CollisionGroups::ALL,
        }
    }

    /// With one more wheel.
    ///
    /// # Panics
    ///
    /// If it already has [`MAX_WHEELS`](Self::MAX_WHEELS).
    #[must_use]
    #[track_caller]
    pub fn with_wheel(mut self, wheel: Wheel3D) -> Self {
        let n = self.wheels().len();
        assert!(
            n < Self::MAX_WHEELS,
            "Vehicle3D::with_wheel: a vehicle has at most {} wheels",
            Self::MAX_WHEELS
        );
        self.wheels[n] = wheel;
        self.wheel_count = n as u32 + 1;
        self
    }

    /// The wheels.
    #[must_use]
    pub fn wheels(&self) -> &[Wheel3D] {
        &self.wheels[..(self.wheel_count as usize).min(Self::MAX_WHEELS)]
    }

    /// The wheels, to set their inputs.
    pub fn wheels_mut(&mut self) -> &mut [Wheel3D] {
        let n = (self.wheel_count as usize).min(Self::MAX_WHEELS);
        &mut self.wheels[..n]
    }
}

/// Below this speed (m/s) slip is measured against it instead, so a car
/// at rest has finite slip.
const MIN_SLIP_SPEED: f32 = 1.0;
/// Wheel-spin substeps per physics step: the wheel is light and the tyre
/// stiff, so its spin is integrated in smaller steps.
const SUBSTEPS: u32 = 5;

pub(super) fn step_vehicles(physics: &mut Physics3D, world: &mut World, dt: f32) {
    if dt <= 0.0 {
        return;
    }
    let gravity = physics.gravity;
    world.query::<(Entity, &mut Vehicle3D)>().for_each(|(e, vehicle)| {
        let Some(link) = physics.link(e) else { return };
        let LinkKind::Body(rigid, _) = link.kind else { return };
        if rigid.kind != BodyKind::Dynamic {
            return;
        }
        let body = link.body;
        step_one(&mut physics.world, body, vehicle, gravity * rigid.gravity_scale, dt);
    });
}

/// What one wheel's ray found.
#[derive(Clone, Copy)]
struct Contact {
    collider: rp::ColliderHandle,
    distance: f32,
    normal: Vec3,
}

fn step_one(w: &mut rp::PhysicsWorld, handle: rp::RigidBodyHandle, vehicle: &mut Vehicle3D, gravity: Vec3, dt: f32) {
    let n = (vehicle.wheel_count as usize).min(Vehicle3D::MAX_WHEELS);
    let wheels = &mut vehicle.wheels[..n];
    let Some(body) = w.bodies.get(handle) else { return };
    let mass = body.mass();
    if n == 0 || !(mass > 0.0 && mass.is_finite()) {
        return;
    }
    if body.is_sleeping() {
        if wheels.iter().all(|w| w.motor_torque == 0.0) {
            // Parked: nothing changes, but the wheels still steer.
            for wheel in wheels.iter_mut() {
                wheel.local_rotation = wheel_rotation(wheel);
            }
            return;
        }
        if let Some(b) = w.bodies.get_mut(handle) {
            b.wake_up(true);
        }
    }
    // The wheels push with forces, spread over the step like gravity (an
    // impulse up front would leave a car at rest reading a small fall
    // speed). The engine has no other forces on bodies, and rapier keeps
    // forces until they're reset: start from none.
    if let Some(b) = w.bodies.get_mut(handle) {
        b.reset_forces(false);
        b.reset_torques(false);
    }
    let Some(body) = w.bodies.get(handle) else { return };
    let pose = *body.position();
    let (origin, rotation) = (from_rp(pose.translation), rot_from_rp(pose.rotation));
    let up = rotation * Vec3::Y;
    let forward = rotation * Vec3::NEG_Z;
    let sprung = sprung_masses(wheels, from_rp(body.local_center_of_mass()), mass);
    // The weight the suspension holds, per kg.
    let weight = (-gravity.dot(up)).max(0.0);

    // Cast every wheel's ray first, against the world without the chassis.
    let mut contacts: [Option<Contact>; Vehicle3D::MAX_WHEELS] = [None; Vehicle3D::MAX_WHEELS];
    {
        let filter = rp::QueryFilter::default()
            .exclude_rigid_body(handle)
            .exclude_sensors()
            .groups(vehicle.groups.to_rp());
        let queries = w.query_pipeline_with_filter(filter);
        for (i, wheel) in wheels.iter().enumerate() {
            let radius = wheel.radius.max(0.01);
            let travel = wheel.suspension_distance.max(0.0);
            let mount = origin + rotation * wheel.position;
            let ray = rp::Ray::new(to_rp(mount), to_rp(-up));
            contacts[i] = queries
                .cast_ray_and_get_normal(&ray, travel + radius, true)
                .map(|(collider, hit)| Contact {
                    collider,
                    distance: hit.time_of_impact,
                    normal: from_rp(hit.normal),
                });
        }
    }
    let grounded = contacts[..n].iter().filter(|c| c.is_some()).count().max(1);
    // The mass each grounded wheel moves, for the low-speed limits.
    let share = mass / grounded as f32;

    for (i, wheel) in wheels.iter_mut().enumerate() {
        let radius = wheel.radius.max(0.01);
        let travel = wheel.suspension_distance.max(0.0);
        let rest = travel * wheel.target_position.clamp(0.0, 1.0);
        let inertia = (0.5 * wheel.mass.max(0.01) * radius * radius).max(1e-4);
        let damping = wheel.damping_rate.max(0.0);
        let motor = if wheel.motor_torque.is_finite() {
            wheel.motor_torque
        } else {
            0.0
        };
        let brake = if wheel.brake_torque.is_finite() {
            wheel.brake_torque.abs()
        } else {
            0.0
        };
        let steer = if wheel.steer_angle.is_finite() {
            wheel.steer_angle
        } else {
            0.0
        };
        let mount = origin + rotation * wheel.position;
        let was_grounded = wheel.grounded;
        let previous = wheel.compression;
        wheel.sprung_mass = sprung[i];
        let h = dt / SUBSTEPS as f32;

        let Some(contact) = contacts[i] else {
            // In the air: the wheel hangs and spins freely.
            let mut omega = finite_or_zero(wheel.angular_velocity);
            for _ in 0..SUBSTEPS {
                omega = spin(omega, h * (motor - damping * omega) / inertia, h * brake / inertia);
            }
            *wheel = Wheel3D {
                grounded: false,
                compression: 0.0,
                suspension_force: 0.0,
                forward_slip: 0.0,
                sideways_slip: 0.0,
                angular_velocity: omega,
                spin: (wheel.spin + omega * dt).rem_euclid(std::f32::consts::TAU),
                local_position: wheel.position - Vec3::Y * travel,
                ..*wheel
            };
            wheel.local_rotation = wheel_rotation(wheel);
            continue;
        };

        // Suspension: how far it's pushed in, from full extension.
        let raw = travel + radius - contact.distance;
        let compression = raw.clamp(0.0, travel);
        let normal = if contact.normal.length_squared() > 0.5 {
            contact.normal
        } else {
            up
        };
        let point = mount - up * contact.distance;
        let ground = w
            .colliders
            .get(contact.collider)
            .and_then(|c| c.parent())
            .filter(|&b| w.bodies.get(b).is_some_and(|b| b.is_dynamic() || b.is_kinematic()));
        let ground_velocity = ground
            .and_then(|b| w.bodies.get(b))
            .map_or(Vec3::ZERO, |b| from_rp(b.velocity_at_point(to_rp(point))));
        let Some(body) = w.bodies.get(handle) else { return };
        let velocity = from_rp(body.velocity_at_point(to_rp(point))) - ground_velocity;
        let compression_speed = if was_grounded {
            (compression - previous) / dt
        } else {
            velocity.dot(-up)
        };
        // Holds the wheel's share of the weight at the resting point, plus
        // the spring past it and the damper.
        let past_rest = compression - (travel - rest);
        let load = (sprung[i] * weight + wheel.spring * past_rest + wheel.damper * compression_speed).max(0.0);

        // The wheel's own axes on the ground.
        let (sin, cos) = (-steer).sin_cos();
        let pointing = forward * cos + up.cross(forward) * sin;
        let along = (pointing - normal * pointing.dot(normal)).normalize_or(forward);
        let across = along.cross(normal);
        let v_forward = velocity.dot(along);
        let v_side = velocity.dot(across);

        // Spin and rolling grip, in substeps, implicitly in the spin (the
        // tyre is stiff, the wheel light). The road speed seen by the tyre
        // follows the grip's push on this wheel's share of the chassis.
        let curve = wheel.forward_friction;
        let mut omega = finite_or_zero(wheel.angular_velocity);
        let mut road = v_forward;
        let mut push = 0.0;
        for _ in 0..SUBSTEPS {
            let denominator = road.abs().max(MIN_SLIP_SPEED);
            let slip = (omega * radius - road) / denominator;
            let (grip, slope) = curve.value_and_slope(slip.abs());
            let force = grip * load * slip.signum();
            let stiffness = slope.max(0.0) * load * radius / denominator;
            let resist = inertia + h * (radius * stiffness + damping);
            omega = spin(
                omega,
                h * (motor - damping * omega - radius * force) / resist,
                h * brake / resist,
            );
            let slip = (omega * radius - road) / denominator;
            let mut force = curve.evaluate(slip) * load * slip.signum();
            // Never push the road past the tyre's speed in one substep.
            let gap = omega * radius - road;
            let most = share * gap.abs() / h;
            force = force.clamp(-most, most);
            road += force * h / share;
            push += force;
        }
        let forward_force = push / SUBSTEPS as f32;
        let sideways_slip = v_side.atan2(v_forward.abs().max(MIN_SLIP_SPEED));
        let most = share * v_side.abs() / dt;
        let side_force = (-wheel.sideways_friction.evaluate(sideways_slip) * load * v_side.signum()).clamp(-most, most);

        // Where the forces act: up the suspension from the resting wheel's bottom.
        let resting_bottom = mount - up * (rest + radius);
        let at = resting_bottom + up * wheel.force_app_point_distance;
        let mut force = up * load + along * forward_force + across * side_force;
        if raw > travel {
            // Bottomed out: a hard stop, so the chassis doesn't sink into the ground.
            let approach = velocity.dot(-up).max(0.0);
            force += up * share * (approach + 0.2 * (raw - travel) / dt) / dt;
        }
        if let Some(b) = w.bodies.get_mut(handle) {
            b.add_force_at_point(to_rp(force), to_rp(at), false);
        }
        // The road is pushed back (a car on a raft or a ramp that moves). An
        // impulse: forces on other bodies would stay until reset.
        if let Some(g) = ground
            && let Some(b) = w.bodies.get_mut(g)
            && b.is_dynamic()
        {
            b.apply_impulse_at_point(to_rp(-force * dt), to_rp(point), true);
        }

        *wheel = Wheel3D {
            grounded: true,
            contact_point: point,
            contact_normal: normal,
            compression,
            suspension_force: load,
            angular_velocity: omega,
            forward_slip: (omega * radius - v_forward) / v_forward.abs().max(MIN_SLIP_SPEED),
            sideways_slip,
            spin: (wheel.spin + omega * dt).rem_euclid(std::f32::consts::TAU),
            local_position: wheel.position - Vec3::Y * (travel - compression),
            ..*wheel
        };
        wheel.local_rotation = wheel_rotation(wheel);
    }
}

fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// `omega` sped up by `change` and slowed by up to `brake` (both rad/s),
/// stopping at 0 rather than turning back.
fn spin(omega: f32, change: f32, brake: f32) -> f32 {
    let free = omega + change;
    if free.abs() <= brake {
        0.0
    } else {
        free - brake * free.signum()
    }
}

/// Steer around the chassis's up, then spin around the axle (rolling
/// forward, towards -Z, turns the wheel around -X).
fn wheel_rotation(wheel: &Wheel3D) -> Quat {
    let steer = if wheel.steer_angle.is_finite() {
        wheel.steer_angle
    } else {
        0.0
    };
    Quat::from_rotation_y(-steer) * Quat::from_rotation_x(-wheel.spin)
}

/// How the chassis's mass splits over the wheels so that it balances on
/// them: the smallest masses (in the least-squares sense) that add up to
/// `mass` and whose center is the center of mass, seen from above.
fn sprung_masses(wheels: &[Wheel3D], center: Vec3, mass: f32) -> [f32; Vehicle3D::MAX_WHEELS] {
    let mut out = [0.0; Vehicle3D::MAX_WHEELS];
    let n = wheels.len();
    if n == 0 {
        return out;
    }
    // Rows of A: 1, dx, dz per wheel. Solve (A Aᵀ) y = (mass, 0, 0), m = Aᵀ y.
    let mut m = [[0.0f64; 3]; 3];
    for wheel in wheels {
        let row = [
            1.0,
            f64::from(wheel.position.x - center.x),
            f64::from(wheel.position.z - center.z),
        ];
        for a in 0..3 {
            for b in 0..3 {
                m[a][b] += row[a] * row[b];
            }
        }
    }
    // Wheels in a line (a bike) can't balance across it: don't try.
    m[1][1] += 1e-6;
    m[2][2] += 1e-6;
    let y = solve3(m, [f64::from(mass), 0.0, 0.0]);
    let mut total = 0.0;
    for (i, wheel) in wheels.iter().enumerate() {
        let row = [
            1.0,
            f64::from(wheel.position.x - center.x),
            f64::from(wheel.position.z - center.z),
        ];
        let share = (row[0] * y[0] + row[1] * y[1] + row[2] * y[2]).max(0.0);
        out[i] = share as f32;
        total += share;
    }
    if total > 0.0 {
        let scale = f64::from(mass) / total;
        for share in &mut out[..n] {
            *share = (f64::from(*share) * scale) as f32;
        }
    } else {
        out[..n].fill(mass / n as f32);
    }
    out
}

/// Solves a 3 × 3 system by Cramer's rule (it's symmetric positive
/// definite here, and tiny).
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> [f64; 3] {
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-12 {
        return [b[0] / m[0][0].max(1e-12), 0.0, 0.0];
    }
    let mut out = [0.0; 3];
    for (col, slot) in out.iter_mut().enumerate() {
        let mut mc = m;
        for row in 0..3 {
            mc[row][col] = b[row];
        }
        *slot = det(mc) / d;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    #[test]
    fn friction_curves_peak_then_settle_smoothly() {
        let c = FrictionCurve::FORWARD;
        let mut last = 0.0;
        for i in 1..=40 {
            let s = i as f32 * 0.01;
            let v = c.evaluate(s);
            assert!(v > last, "rises to the peak at {s}");
            last = v;
        }
        assert!((c.evaluate(0.4) - 1.0).abs() < 1e-6);
        assert!((c.evaluate(0.6) - 0.75).abs() < 1e-6, "halfway down");
        assert!((c.evaluate(0.8) - 0.5).abs() < 1e-6);
        assert_eq!(c.evaluate(-3.0), 0.5);
        // Flat at the peak: the slope just below is tiny.
        assert!(c.value_and_slope(0.399).1 < 0.05);
    }

    #[test]
    fn sprung_masses_balance_the_center_of_mass() {
        let wheels =
            [(-1.0, -1.5), (1.0, -1.5), (-1.0, 1.5), (1.0, 1.5)].map(|(x, z)| Wheel3D::new(vec3(x, 0.0, z), 0.3));
        let even = sprung_masses(&wheels, Vec3::ZERO, 1000.0);
        for m in &even[..4] {
            assert!((m - 250.0).abs() < 0.01, "{even:?}");
        }
        // Weight forward: the front wheels carry more.
        let front = sprung_masses(&wheels, vec3(0.0, -0.3, -0.5), 1000.0);
        assert!(front[0] > front[2] && (front[0] - front[1]).abs() < 0.01, "{front:?}");
        assert!((front[..4].iter().sum::<f32>() - 1000.0).abs() < 0.01);
        let moment: f32 = (0..4).map(|i| front[i] * (wheels[i].position.z + 0.5)).sum();
        assert!(moment.abs() < 0.1, "balances: {moment}");
        // A line of wheels still shares the mass.
        let bike = [
            Wheel3D::new(vec3(0.0, 0.0, -1.0), 0.3),
            Wheel3D::new(vec3(0.0, 0.0, 1.0), 0.3),
        ];
        let shares = sprung_masses(&bike, Vec3::ZERO, 200.0);
        assert!((shares[0] - 100.0).abs() < 0.01, "{shares:?}");
    }

    #[test]
    fn brakes_stop_a_wheel_without_turning_it_back() {
        assert_eq!(spin(1.0, 0.0, 2.0), 0.0);
        assert_eq!(spin(-1.0, 0.0, 2.0), 0.0);
        assert_eq!(spin(5.0, 1.0, 2.0), 4.0);
        assert_eq!(spin(-5.0, 0.0, 2.0), -3.0);
    }
}
