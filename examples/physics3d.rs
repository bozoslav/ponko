//! 3D physics: walk around hilly terrain in first person, climb steps,
//! push crates, and knock them over by clicking. The terrain is drawn
//! from the same heights its heightfield collider is made from.
//!
//!   `cargo run --release --example physics3d --features physics3d`
//!
//! Click to lock the mouse (Escape lets it go, Escape again quits). The
//! mouse looks, WASD walks, Shift runs, Space jumps; left click shoves the
//! crate you're looking at.

#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{Material, MaterialDesc, MeshData};
use ponko::terrain::{TerrainMaterialDesc, TextureKind};

const N: usize = 65;
const SIZE: f32 = 128.0;

fn height(x: f32, z: f32) -> f32 {
    (x * 0.05).sin() * 3.0 + (z * 0.07).cos() * 2.0 + (x * 0.013 + z * 0.021).sin() * 6.0
}

/// The walker and where they look.
struct Player {
    id: Entity,
    yaw: f32,
    pitch: f32,
}

// ANCHOR: world
fn build_world(ctx: &mut Ctx) {
    // Ground: a terrain tile and a heightfield from the same heights,
    // both at the tile's corner.
    let corner = vec3(-SIZE / 2.0, 0.0, -SIZE / 2.0);
    let step = SIZE / (N - 1) as f32;
    let heights: Vec<f32> = (0..N * N)
        .map(|i| height(corner.x + (i % N) as f32 * step, corner.z + (i / N) as f32 * step))
        .collect();
    let ground = ctx.physics3d.heightfield(&heights, SIZE);
    let look = grass(ctx);
    let tile = ctx.gfx.create_terrain_tile(&heights, SIZE, look);
    ctx.world
        .spawn((Transform::at(corner), tile, RigidBody::fixed(), Collider::mesh(ground)));

    // Stairs: fixed boxes 0.25 m higher each.
    let stone = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x9A948A)));
    let block = ctx.gfx.create_mesh(&MeshData::cube(1.0), stone);
    for i in 0..8 {
        let size = vec3(2.0, 0.25 * (i + 1) as f32, 0.6);
        let at = vec3(6.0, height(6.0, 3.0 + i as f32 * 0.6), 3.0 + i as f32 * 0.6);
        ctx.world.spawn((
            Transform {
                scale: size,
                ..Transform::at(at + Vec3::Y * size.y / 2.0)
            },
            Model::new(block),
            RigidBody::fixed(),
            Collider::cuboid(size),
        ));
    }

    // Crates: dynamic, light enough to push.
    let wood = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0xB07A43)));
    let crate_mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), wood);
    for i in 0..6 {
        let (x, z) = (-4.0 + i as f32 * 1.5, -6.0);
        ctx.world.spawn((
            Transform::at(vec3(x, height(x, z) + 2.0 + i as f32, z)),
            Model::new(crate_mesh),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::ONE).density(150.0).friction(0.7),
            Velocity::default(),
        ));
    }

    // The walker: a capsule 1.8 m tall, its transform at its feet.
    let mut walker = Character::new(1.8, 0.3);
    walker.step_height = 0.3;
    let id = ctx
        .world
        .spawn((Transform::at(vec3(0.0, height(0.0, 0.0) + 1.0, 0.0)), walker));
    ctx.world.insert_resource(Player {
        id,
        yaw: 0.0,
        pitch: 0.0,
    });
}
// ANCHOR_END: world

/// A one-layer terrain material: grass everywhere.
fn grass(ctx: &mut Ctx) -> Material {
    let green: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            let shade = 200 + (i * 37 % 56) as u8;
            [
                (60 * shade as u32 / 255) as u8,
                (110 * shade as u32 / 255) as u8,
                40,
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

// ANCHOR: walk
/// WASD walks the way the walker faces, Shift runs, Space jumps (only
/// from the ground). A simulation system: it runs before each step.
fn walk(ctx: &mut Ctx) {
    let keys = [Key::W, Key::A, Key::S, Key::D, Key::LeftShift].map(|k| ctx.input.key_down(k));
    let jump = ctx.input.key_down(Key::Space);
    let player = ctx.world.resource::<Player>();
    let (id, yaw) = (player.id, player.yaw);
    let Some(c) = ctx.world.get_mut::<Character>(id) else {
        return;
    };
    let forward = vec3(-yaw.sin(), 0.0, -yaw.cos());
    let right = vec3(yaw.cos(), 0.0, -yaw.sin());
    let axis = |plus: bool, minus: bool| plus as i32 as f32 - minus as i32 as f32;
    let wish = (forward * axis(keys[0], keys[2]) + right * axis(keys[3], keys[1])).normalize_or_zero();
    let speed = if keys[4] { 8.0 } else { 4.0 };
    c.velocity.x = wish.x * speed;
    c.velocity.z = wish.z * speed;
    if c.grounded && jump {
        c.velocity.y = 5.0;
    }
}
// ANCHOR_END: walk

// ANCHOR: queries
/// Left click shoves whatever is in front of the eye, up to 30 m away.
fn shove(ctx: &mut Ctx) {
    let me = ctx.world.resource::<Player>().id;
    let (eye, forward) = (ctx.scene3d.camera.position, ctx.scene3d.camera.rotation * Vec3::NEG_Z);
    let filter = QueryFilter::default().exclude(me);
    if let Some(hit) = ctx.physics3d.cast_ray(eye, forward, 30.0, filter)
        && let Some(mass) = ctx.physics3d.mass(hit.entity)
    {
        ctx.physics3d
            .apply_impulse_at(hit.entity, forward * mass * 6.0, hit.point);
    }
}
// ANCHOR_END: queries

// ANCHOR: look
/// Every frame: the mouse turns the head (raw motion, so it works while the
/// cursor is locked) and the camera sits at the walker's eyes. A click
/// locks the mouse, Escape frees it.
fn eyes(ctx: &mut Ctx) {
    if ctx.input.mouse_pressed(MouseButton::Left) {
        if ctx.input.cursor_locked() {
            shove(ctx);
        } else {
            ctx.input.set_cursor_locked(true);
        }
    } else if ctx.input.key_pressed(Key::Escape) {
        if ctx.input.cursor_locked() {
            ctx.input.set_cursor_locked(false);
        } else {
            ctx.quit();
        }
    }
    let turn = if ctx.input.cursor_locked() {
        ctx.input.mouse_motion() * 0.003
    } else {
        Vec2::ZERO
    };
    let p = ctx.world.resource_mut::<Player>();
    p.yaw -= turn.x;
    p.pitch = (p.pitch - turn.y).clamp(-1.5, 1.5);
    let (id, yaw, pitch) = (p.id, p.yaw, p.pitch);
    if let Some(t) = ctx.world.get::<Transform>(id) {
        ctx.scene3d.camera.position = t.position + Vec3::Y * 1.65;
        ctx.scene3d.camera.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
    }
}
// ANCHOR_END: look

fn main() {
    // ANCHOR: setup
    Game::new("Ponko: 3D physics", 1280, 720)
        .physics3d(vec3(0.0, -9.81, 0.0)) // m/s², +Y up; 50 steps a second
        .sim_system(walk)
        .system(eyes)
        // ANCHOR_END: setup
        .setup(|ctx| {
            build_world(ctx);
        })
        .run();
}
