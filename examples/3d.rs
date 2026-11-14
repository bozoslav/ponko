//! 3D: meshes, materials, a sun, and thousands of instanced models.
//!
//!   `cargo run --release --example 3d --features 3d`
//!
//! - Drag with the left mouse button to orbit, wheel to zoom.
//! - Space: pause the spinning.  Escape: quit.
//! - `CUBES=100000` (environment) spins that many cubes in a field.
//! - `BENCH_SECONDS=10` goes fullscreen (`BENCH_WINDOWED=1`: not) with vsync off, measures that many seconds after a
//!   3 second warmup, prints the averages as JSON and quits (the same
//!   measurement as the Unity version of this scene).

#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{MaterialDesc, MeshData, ModelMaterial};

/// Turns around its own Y axis.
#[derive(Component)]
struct Spin(f32);

/// The orbiting camera: angles around the target and distance.
struct Orbit {
    yaw: f32,
    pitch: f32,
    distance: f32,
}

fn spin(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    ctx.world.query::<(&mut Transform, &Spin)>().for_each(|(t, s)| {
        t.rotation *= Quat::from_rotation_y(s.0 * dt);
    });
}

fn orbit(ctx: &mut Ctx) {
    if ctx.input.key_pressed(Key::Space) {
        if ctx.time.is_paused() {
            ctx.time.resume()
        } else {
            ctx.time.pause()
        }
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
    let drag = if ctx.input.mouse_down(MouseButton::Left) {
        ctx.input.mouse_delta()
    } else {
        Vec2::ZERO
    };
    let wheel = ctx.input.mouse_wheel().y;
    let o = ctx.world.resource_mut::<Orbit>();
    o.yaw -= drag.x * 0.005;
    o.pitch = (o.pitch + drag.y * 0.005).clamp(0.05, 1.5);
    o.distance = (o.distance * 0.9f32.powf(wheel)).clamp(3.0, 400.0);
    let (yaw, pitch, distance) = (o.yaw, o.pitch, o.distance);
    let offset = vec3(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()) * distance;
    ctx.scene3d.camera.position = offset;
    ctx.scene3d.camera.look_at(Vec3::ZERO);
}

/// Frame times summed after the warmup.
struct Bench {
    seconds: f64,
    start: Option<std::time::Instant>,
    last: Option<std::time::Instant>,
    frames: u64,
    frame_ms: f64,
    render_cpu_ms: f64,
    /// Frame time minus waiting for the display: the work.
    busy_ms: f64,
    /// GPU time of the frame's passes.
    gpu_ms: f64,
}

fn bench(ctx: &mut Ctx) {
    let now = std::time::Instant::now();
    let cpu = ctx.perf.render_stats().cpu_ms as f64;
    let gpu = ctx.perf.render_stats().gpu_ms.unwrap_or(0.0) as f64;
    let wait = ctx.perf.display_wait_ms() as f64;
    let b = ctx.world.resource_mut::<Bench>();
    let start = *b.start.get_or_insert(now);
    let elapsed = now.duration_since(start).as_secs_f64();
    let last = b.last.replace(now);
    let measuring = last.is_some() && elapsed > 3.0;
    let first = measuring && b.frames == 0;
    if let (true, Some(last)) = (measuring, last) {
        b.frames += 1;
        b.frame_ms += now.duration_since(last).as_secs_f64() * 1000.0;
        b.render_cpu_ms += cpu;
        b.gpu_ms += gpu;
        b.busy_ms += now.duration_since(last).as_secs_f64() * 1000.0 - wait;
    }
    let done = elapsed > 3.0 + b.seconds;
    let (frames, frame_ms, render_cpu_ms, busy_ms, gpu_ms) =
        (b.frames, b.frame_ms, b.render_cpu_ms, b.busy_ms, b.gpu_ms);
    if first && let Ok(path) = std::env::var("BENCH_SHOT") {
        ctx.gfx.save_screenshot(path);
    }
    if done {
        let n = frames as f64;
        println!(
            "{{\"engine\":\"ponko\",\"frames\":{frames},\"fps\":{:.3},\"frame_ms\":{:.3},\"render_cpu_ms\":{:.3},\"busy_ms\":{:.3},\"gpu_ms\":{:.3}}}",
            n / (frame_ms / 1000.0),
            frame_ms / n,
            render_cpu_ms / n,
            busy_ms / n,
            gpu_ms / n
        );
        ctx.quit();
    }
}

fn main() {
    let bench_seconds: Option<f64> = std::env::var("BENCH_SECONDS").ok().and_then(|s| s.parse().ok());
    let cubes: usize = std::env::var("CUBES").ok().and_then(|n| n.parse().ok()).unwrap_or(400);
    let mut game = Game::new("Ponko: 3D", 1280, 720).system(orbit);
    game = if bench_seconds.is_some() {
        // Spin every frame, as Unity's Update does, so both do the same work.
        let fullscreen = std::env::var_os("BENCH_WINDOWED").is_none();
        game.vsync(false).fullscreen(fullscreen).system(spin).system(bench)
    } else {
        game.sim_system(spin)
    };
    game.setup(move |ctx| {
        ctx.scene3d.camera.far = 2000.0;
        ctx.scene3d.lighting.sun_direction = vec3(-0.8, -0.6, -0.4); // late afternoon: long shadows
        if let Some(seconds) = bench_seconds {
            ctx.world.insert_resource(Bench {
                seconds,
                start: None,
                last: None,
                frames: 0,
                frame_ms: 0.0,
                render_cpu_ms: 0.0,
                busy_ms: 0.0,
                gpu_ms: 0.0,
            });
        }
        ctx.world.insert_resource(Orbit {
            yaw: 0.6,
            pitch: 0.45,
            distance: 18.0,
        });

        let grass = ctx
            .gfx
            .create_material(MaterialDesc::color(Color::hex(0x5E8C4A)).with_roughness(0.95));
        let ground = ctx.gfx.create_mesh(&MeshData::plane(400.0), grass);
        ctx.world.spawn((Transform::default(), Model::new(ground)));

        // Spheres: rough to smooth left to right, plastic in front, metal behind.
        // One mesh, uploaded once; each sphere draws it with its own material.
        let ball = ctx.gfx.create_plain_mesh(&MeshData::sphere(0.45, 32));
        for row in 0..2 {
            for col in 0..6 {
                let material = ctx.gfx.create_material(
                    MaterialDesc::color(if row == 0 {
                        Color::hex(0xD8453B)
                    } else {
                        Color::hex(0xE8C070)
                    })
                    .with_roughness(0.05 + col as f32 * 0.19)
                    .with_metallic(row as f32),
                );
                let at = vec3(col as f32 * 1.1 - 2.75, 0.45, row as f32 * -1.2);
                ctx.world
                    .spawn((Transform::at(at), Model::new(ball), ModelMaterial(material)));
            }
        }

        // A field of spinning cubes, one mesh, tinted per cube.
        let cube = ctx.gfx.create_plain_mesh(&MeshData::cube(0.6));
        let side = (cubes as f32).sqrt().ceil() as usize;
        let mut rng = Rng::new(3);
        for i in 0..cubes {
            let (x, z) = ((i % side) as f32, (i / side) as f32);
            let at = vec3((x - side as f32 / 2.0) * 1.6, 0.3, -6.0 - z * 1.6);
            let tint = Color::rgb(rng.range(0.3, 1.0), rng.range(0.3, 1.0), rng.range(0.3, 1.0));
            ctx.world.spawn((
                Transform::at(at).rotated(Quat::from_rotation_y(rng.range(0.0, 6.3))),
                Model::new(cube).with_color(tint),
                Spin(rng.range(-2.0, 2.0)),
            ));
        }
    })
    .run();
}
