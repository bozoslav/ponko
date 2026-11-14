//! Raycast-wheel vehicles: drive a box car over hilly terrain. The chassis
//! is a dynamic body on four `Wheel`s; this file is the game's half:
//! throttle, brakes and steering turned into wheel torques and angles.
//!
//!   `cargo run --release --example vehicle --features physics3d`
//!
//! W accelerates, S brakes (and reverses once stopped), A and D steer,
//! Space is the handbrake, R puts the car back on its wheels.
//!
//! `HEADLESS=1` drives it by script instead, without a window, and checks
//! that it speeds up, turns and stops.

#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{Material, MaterialDesc, MeshData};
use ponko::terrain::{TerrainMaterialDesc, TextureKind};

const N: usize = 129;
const SIZE: f32 = 512.0;

fn height(x: f32, z: f32) -> f32 {
    (x * 0.03).sin() * 3.0 + (z * 0.025).cos() * 4.0 + (x * 0.011 + z * 0.007).sin() * 6.0
}

/// What the driver asks for this tick, each -1..1 (steer: positive right).
#[derive(Clone, Copy, Default)]
struct Controls {
    throttle: f32,
    steer: f32,
    handbrake: bool,
}

/// The car being driven, its steering (which turns smoothly) and the
/// entities that draw its wheels.
struct Player {
    car: Entity,
    steer: f32,
    wheels: Vec<Entity>,
}

// ANCHOR: car
/// A 1200 kg box car facing -Z, its center of mass low between the axles,
/// front wheels first.
fn spawn_car(ctx: &mut Ctx, at: Vec3) -> Entity {
    let mut car = Vehicle::new();
    for (x, z) in [(-0.8, -1.3), (0.8, -1.3), (-0.8, 1.3), (0.8, 1.3)] {
        let mut wheel = Wheel::new(vec3(x, -0.3, z), 0.35);
        wheel.sideways_friction.stiffness = 1.3;
        car = car.with_wheel(wheel);
    }
    ctx.world.spawn((
        Transform::at(at),
        RigidBody::dynamic().mass(1200.0).center_of_mass(vec3(0.0, -0.35, 0.0)),
        Collider::cuboid(vec3(1.8, 0.6, 4.2)),
        Velocity::default(),
        car,
    ))
}
// ANCHOR_END: car

// ANCHOR: drive
/// The game's half of driving, before each physics step: rear-wheel
/// drive, brakes on all four, steering that eases in and tightens less at
/// speed.
fn drive(ctx: &mut Ctx) {
    let controls = *ctx.world.resource::<Controls>();
    let dt = ctx.time.dt();
    let car = ctx.world.resource::<Player>().car;
    let forward = forward_speed(&ctx.world, car);
    let wanted = controls.steer * 0.6 / (1.0 + forward.abs() * 0.08);
    let player = ctx.world.resource_mut::<Player>();
    player.steer += (wanted - player.steer).clamp(-2.0 * dt, 2.0 * dt);
    let steer = player.steer;
    let Some(vehicle) = ctx.world.get_mut::<Vehicle>(car) else {
        return;
    };
    // Pressing against the way it rolls brakes; from (almost) still it drives.
    let braking = controls.throttle * forward < 0.0 && forward.abs() > 0.5;
    for (i, wheel) in vehicle.wheels_mut().iter_mut().enumerate() {
        let front = i < 2;
        wheel.steer_angle = if front { steer } else { 0.0 };
        wheel.motor_torque = if front || braking {
            0.0
        } else {
            controls.throttle * 900.0
        };
        wheel.brake_torque = if braking {
            3000.0
        } else if controls.handbrake && !front {
            5000.0
        } else {
            0.0
        };
    }
}
// ANCHOR_END: drive

fn speed_of(world: &World, car: Entity) -> f32 {
    world.get::<Velocity>(car).map_or(0.0, |v| v.linear.length())
}

/// Speed along where the car faces (negative backing up).
fn forward_speed(world: &World, car: Entity) -> f32 {
    match (world.get::<Velocity>(car), world.get::<Transform>(car)) {
        (Some(v), Some(t)) => v.linear.dot(t.forward()),
        _ => 0.0,
    }
}

/// Keys to controls (windowed), in the same tick as `drive`.
fn keys(ctx: &mut Ctx) {
    let axis = |plus: bool, minus: bool| plus as i32 as f32 - minus as i32 as f32;
    let controls = Controls {
        throttle: axis(ctx.input.key_down(Key::W), ctx.input.key_down(Key::S)),
        steer: axis(ctx.input.key_down(Key::D), ctx.input.key_down(Key::A)),
        handbrake: ctx.input.key_down(Key::Space),
    };
    *ctx.world.resource_mut::<Controls>() = controls;
    if ctx.input.key_pressed(Key::R) {
        let car = ctx.world.resource::<Player>().car;
        if let Some(t) = ctx.world.get_mut::<Transform>(car) {
            let p = t.position;
            *t = Transform::at(vec3(p.x, height(p.x, p.z) + 1.5, p.z));
        }
    }
}

// ANCHOR: wheels
/// Every frame: each wheel drawn where its car's last step put it (steer,
/// spin and suspension travel), following the chassis's smoothed pose.
fn place_wheels(ctx: &mut Ctx) {
    let player = ctx.world.resource::<Player>();
    let car = player.car;
    let (Some(&vehicle), Some(&chassis)) = (ctx.world.get::<Vehicle>(car), ctx.world.get::<Transform>(car)) else {
        return;
    };
    for i in 0..player.wheels.len() {
        let e = ctx.world.resource::<Player>().wheels[i];
        let pose = vehicle.wheels()[i].pose(&chassis);
        if let Some(t) = ctx.world.get_mut::<Transform>(e) {
            *t = Transform { scale: t.scale, ..pose };
        }
    }
}
// ANCHOR_END: wheels

/// Every frame: the camera trails the car.
fn chase(ctx: &mut Ctx) {
    let car = ctx.world.resource::<Player>().car;
    let Some(t) = ctx.world.get::<Transform>(car).copied() else {
        return;
    };
    let back = -t.forward().with_y(0.0).normalize_or(Vec3::Z);
    let eye = t.position + back * 9.0 + Vec3::Y * 3.5;
    let k = (ctx.time.dt() * 4.0).min(1.0);
    ctx.scene3d.camera.position = ctx.scene3d.camera.position.lerp(eye, k);
    let look = Transform::at(ctx.scene3d.camera.position).looking_at(t.position + Vec3::Y);
    ctx.scene3d.camera.rotation = look.rotation;
}

fn build_world(ctx: &mut Ctx, draw: bool) {
    let corner = vec3(-SIZE / 2.0, 0.0, -SIZE / 2.0);
    let step = SIZE / (N - 1) as f32;
    let heights: Vec<f32> = (0..N * N)
        .map(|i| height(corner.x + (i % N) as f32 * step, corner.z + (i / N) as f32 * step))
        .collect();
    let ground = ctx.physics3d.heightfield(&heights, SIZE);
    let terrain = ctx
        .world
        .spawn((Transform::at(corner), RigidBody::fixed(), Collider::mesh(ground)));
    let car = spawn_car(ctx, vec3(0.0, height(0.0, 0.0) + 1.5, 0.0));
    ctx.world.insert_resource(Player {
        car,
        steer: 0.0,
        wheels: Vec::new(),
    });
    ctx.world.insert_resource(Controls::default());
    if !draw {
        return;
    }
    let look = grass(ctx);
    let tile = ctx.gfx.create_terrain_tile(&heights, SIZE, look);
    ctx.world.insert(terrain, tile);
    let paint = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0xC8402E)));
    let body = ctx.gfx.create_mesh(&MeshData::cube(1.0), paint);
    if let Some(t) = ctx.world.get_mut::<Transform>(car) {
        t.scale = vec3(1.8, 0.6, 4.2);
    }
    ctx.world.insert(car, Model::new(body));
    let rubber = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x202020)));
    let tyre = ctx.gfx.create_mesh(&MeshData::cube(1.0), rubber);
    for _ in 0..4 {
        let wheel = ctx.world.spawn((
            Transform {
                scale: vec3(0.3, 0.7, 0.7),
                ..Transform::default()
            },
            Model::new(tyre),
        ));
        ctx.world.resource_mut::<Player>().wheels.push(wheel);
    }
}

/// A one-layer terrain material: grass everywhere.
fn grass(ctx: &mut Ctx) -> Material {
    let green: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            let shade = 200 + (i * 37 % 56) as u8;
            [
                (70 * shade as u32 / 255) as u8,
                (120 * shade as u32 / 255) as u8,
                45,
                255,
            ]
        })
        .collect();
    let layers = ctx.gfx.create_texture_array(16, 16, &[&green], TextureKind::Color);
    let weights = ctx
        .gfx
        .create_texture_array(1, 1, &[&[255, 0, 0, 0]], TextureKind::Data);
    let corner = vec2(-SIZE / 2.0, -SIZE / 2.0);
    ctx.gfx
        .create_terrain_material(&TerrainMaterialDesc::new(layers, weights, corner, vec2(SIZE, SIZE)))
}

/// The script for `HEADLESS=1`: full throttle for 4 s, then steer right
/// for 2 s, then brake to a stop and hold it with the handbrake.
fn autopilot(ctx: &mut Ctx) {
    let t = ctx.time.sim_tick() as f32 * ctx.time.dt();
    let car = ctx.world.resource::<Player>().car;
    let rolling = forward_speed(&ctx.world, car) > 0.3;
    *ctx.world.resource_mut::<Controls>() = Controls {
        throttle: if t < 6.0 {
            1.0
        } else if rolling {
            -1.0
        } else {
            0.0
        },
        steer: if (4.0..6.0).contains(&t) { 1.0 } else { 0.0 },
        handbrake: t >= 6.0 && !rolling,
    };
}

/// What the car did, second by second.
#[derive(Default)]
struct Log(Vec<(f32, Vec3)>);

fn record(ctx: &mut Ctx) {
    if ctx.time.sim_tick().is_multiple_of(50) {
        let car = ctx.world.resource::<Player>().car;
        let speed = speed_of(&ctx.world, car);
        let facing = ctx.world.get::<Transform>(car).map_or(Vec3::NEG_Z, |t| t.forward());
        ctx.world.resource_mut::<Log>().0.push((speed, facing));
    }
}

/// Runs the script and checks what the car did.
fn headless() {
    let world = Game::new("vehicle", 1, 1)
        .add(Phase::BeforePhysics, autopilot)
        .add(Phase::BeforePhysics, drive)
        .physics3d(vec3(0.0, -9.81, 0.0))
        .sim_system(record)
        .setup(|ctx| {
            build_world(ctx, false);
            ctx.world.insert_resource(Log::default());
        })
        .run_headless(500);
    let log = &world.resource::<Log>().0;
    for (second, (speed, facing)) in log.iter().enumerate() {
        println!("{:2} s: {speed:5.2} m/s, facing {facing:.2}", second + 1);
    }
    let (accelerated, _) = log[3];
    let (_, turned) = log[5];
    let (stopped, _) = log[log.len() - 1];
    assert!(accelerated > 8.0, "accelerates: {accelerated} m/s after 4 s");
    assert!(turned.x > 0.3, "steers right: facing {turned}");
    assert!(stopped < 0.5, "stops: {stopped} m/s");
    println!("accelerates, steers and stops");
}

fn main() {
    if std::env::var("HEADLESS").is_ok() {
        headless();
        return;
    }
    // ANCHOR: setup
    Game::new("Ponko: vehicle", 1280, 720)
        .add(Phase::BeforePhysics, keys)
        .add(Phase::BeforePhysics, drive) // sets the wheels' inputs for this tick's step
        .physics3d(vec3(0.0, -9.81, 0.0))
        .system(place_wheels)
        .system(chase)
        // ANCHOR_END: setup
        .setup(|ctx| {
            build_world(ctx, true);
        })
        .run();
}
