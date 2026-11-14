//! Looks around a 3D scene file (a `ponko-world` save of `d3::Transform` and
//! `Model` entities, like the Unity importer writes):
//!
//!   SCENE=town/Town.json cargo run --release --features 3d --example scene_viewer
//!
//! W, A, S and D fly, Q and E go down and up, the arrow keys turn, Shift
//! flies faster. It starts above the middle of the scene.
//! `CAMERA=x,y,z,yaw,pitch` (degrees) starts there instead.
//! `SHOT=path.png` saves frame 300 and quits. `HEADLESS=1` only loads the
//! scene, without a window, and says what's in it and how long it took.

use std::time::Instant;

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::Sky;

/// The middle and size of where the scene's models are.
fn extent(world: &mut World) -> (Vec3, f32) {
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    world.query::<(&Transform, &Model)>().for_each(|(t, _)| {
        min = min.min(t.position);
        max = max.max(t.position);
    });
    if min.x > max.x {
        return (Vec3::ZERO, 10.0);
    }
    ((min + max) * 0.5, (max - min).max_element().max(10.0))
}

fn camera_from_env() -> Option<(Vec3, f32, f32)> {
    let v: Vec<f32> = std::env::var("CAMERA")
        .ok()?
        .split(',')
        .map(|n| n.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, z, yaw, pitch] = v[..] else { return None };
    Some((vec3(x, y, z), yaw.to_radians(), pitch.to_radians()))
}

fn main() {
    let scene = std::env::var("SCENE").unwrap_or_else(|_| {
        eprintln!("say which scene: SCENE=town/Town.json (a file under assets/)");
        std::process::exit(2);
    });
    if std::env::var("HEADLESS").is_ok() {
        Game::new("scene viewer", 1, 1)
            .setup(move |ctx| {
                let start = Instant::now();
                ctx.load_scene(&scene).unwrap_or_else(|e| panic!("{e}"));
                let models = ctx.world.query::<&Model>().iter().count();
                let (middle, size) = extent(&mut ctx.world);
                println!(
                    "{scene}: {models} models in {:.2} s, around {middle} across {size:.0} m",
                    start.elapsed().as_secs_f32()
                );
            })
            .run_headless(0);
        return;
    }
    let shot = std::env::var("SHOT").ok();
    let (mut yaw, mut pitch) = (0.0f32, -0.3f32);
    Game::new("Ponko: scene viewer", 1280, 720)
        .system(move |ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            let dt = ctx.time.dt();
            if ctx.time.frame() == 1 {
                let (y, x, _) = ctx.scene3d.camera.rotation.to_euler(glam::EulerRot::YXZ);
                (yaw, pitch) = (y, x);
            }
            let turn = 1.2 * dt;
            if ctx.input.key_down(Key::Left) {
                yaw += turn;
            }
            if ctx.input.key_down(Key::Right) {
                yaw -= turn;
            }
            if ctx.input.key_down(Key::Up) {
                pitch = (pitch + turn).min(1.5);
            }
            if ctx.input.key_down(Key::Down) {
                pitch = (pitch - turn).max(-1.5);
            }
            let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
            let speed = if ctx.input.key_down(Key::LeftShift) {
                120.0
            } else {
                20.0
            } * dt;
            let mut go = Vec3::ZERO;
            for (key, way) in [
                (Key::W, Vec3::NEG_Z),
                (Key::S, Vec3::Z),
                (Key::A, Vec3::NEG_X),
                (Key::D, Vec3::X),
            ] {
                if ctx.input.key_down(key) {
                    go += rotation * way;
                }
            }
            if ctx.input.key_down(Key::E) {
                go += Vec3::Y;
            }
            if ctx.input.key_down(Key::Q) {
                go -= Vec3::Y;
            }
            ctx.scene3d.camera.position += go * speed;
            ctx.scene3d.camera.rotation = rotation;
            if ctx.time.frame() == 300
                && let Some(path) = &shot
            {
                ctx.gfx.save_screenshot(path);
            }
            if ctx.time.frame() == 302 && shot.is_some() {
                ctx.quit();
            }
        })
        .setup(move |ctx| {
            ctx.load_scene(&scene).unwrap_or_else(|e| panic!("{e}"));
            ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(15.0);
            ctx.scene3d.lighting.shadow_distance = 400.0;
            ctx.scene3d.camera.far = 3000.0;
            ctx.scene3d.post.bloom = 0.05;
            let (middle, size) = extent(&mut ctx.world);
            let (at, yaw, pitch) =
                camera_from_env().unwrap_or((middle + vec3(0.0, size * 0.25, size * 0.4), 0.0, -0.5));
            ctx.scene3d.camera.position = at;
            ctx.scene3d.camera.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
        })
        .run();
}
