//! Heightfield terrain: 5 x 5 tiles of 256 m (129 x 129 heights, 2 m
//! apart), drawn coarser with distance, with grass, dirt and rock blended
//! by a weight map, a lake, and an underwater screen effect.
//!
//!   `cargo run --release --example terrain --features 3d`
//!
//! WASD moves, Q and E go down and up, the arrows turn, U switches the
//! underwater effect. `SHOT=path.png` saves the third frame and quits;
//! `VIEW=high|low|under|cliff` picks where the camera starts, `HOUR=18.5` the
//! time of day. `BENCH=1` prints the frame's cost after 300 frames and quits.

#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{Blend, Material, MaterialDesc, MeshData, ScreenEffect, Sky, TargetCamera};
use ponko::terrain::{TerrainMaterialDesc, TextureKind};

const TILES: i32 = 5;
const N: usize = 129;
const TILE: f32 = 256.0;
const WATER: f32 = 2.0;

/// Smooth value noise, 0 to 1.
fn noise(x: f32, z: f32) -> f32 {
    tiling_noise(x, z, i32::MAX)
}

/// Value noise that repeats every `period` cells, for textures that tile.
fn tiling_noise(x: f32, z: f32, period: i32) -> f32 {
    let hash = |i: i32, j: i32| {
        let (i, j) = (i.rem_euclid(period), j.rem_euclid(period));
        let h = (i.wrapping_mul(374_761_393) ^ j.wrapping_mul(668_265_263)) as u32;
        let h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        (h >> 8) as f32 / (1u32 << 24) as f32
    };
    let (i, j) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (x - x.floor(), z - z.floor());
    let (sx, sz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let top = hash(i, j) + (hash(i + 1, j) - hash(i, j)) * sx;
    let bottom = hash(i, j + 1) + (hash(i + 1, j + 1) - hash(i, j + 1)) * sx;
    top + (bottom - top) * sz
}

/// Hills: a few octaves of noise, flattened in the valleys.
fn height(x: f32, z: f32) -> f32 {
    let mut h = 0.0;
    let (mut scale, mut amp) = (1.0 / 300.0, 40.0);
    for _ in 0..5 {
        h += (noise(x * scale, z * scale) - 0.5) * amp;
        scale *= 2.1;
        amp *= 0.45;
    }
    h + 6.0
}

/// A ground layer: `base` color with two scales of noise in it.
fn layer(base: [f32; 3], grain: f32) -> Vec<u8> {
    let size = 256;
    (0..size * size)
        .flat_map(|i| {
            let (x, y) = ((i % size) as f32, (i / size) as f32);
            // Periods that divide the 256 pixels, so the layer tiles.
            let n = tiling_noise(x / 16.0, y / 16.0, 16) * 0.6 + tiling_noise(x / 4.0, y / 4.0, 64) * 0.4;
            let shade = 1.0 + (n - 0.5) * grain;
            let c = base.map(|v| ((v * shade).clamp(0.0, 1.0) * 255.0) as u8);
            [c[0], c[1], c[2], 255]
        })
        .collect()
}

fn ground(ctx: &mut Ctx) -> Material {
    let layers = [
        layer([0.24, 0.42, 0.16], 0.6),
        layer([0.40, 0.31, 0.22], 0.7),
        layer([0.46, 0.45, 0.42], 0.9),
        layer([0.70, 0.62, 0.45], 0.4),
    ];
    let refs: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
    let layers = ctx.gfx.create_texture_array(256, 256, &refs, TextureKind::Color);
    // One weight map over the whole world, 2 m a pixel.
    let cells = (TILES as usize * (N - 1)) / 2;
    let area = TILES as f32 * TILE;
    let origin = -area / 2.0;
    let weights: Vec<u8> = (0..cells * cells)
        .flat_map(|i| {
            let x = origin + (i % cells) as f32 * area / cells as f32;
            let z = origin + (i / cells) as f32 * area / cells as f32;
            let h = height(x, z);
            let slope = (height(x + 2.0, z) - height(x - 2.0, z)).hypot(height(x, z + 2.0) - height(x, z - 2.0)) / 4.0;
            let sand = (1.0 - (h - WATER).abs() / 1.5).clamp(0.0, 1.0);
            let rock = ((slope - 0.45) * 4.0).clamp(0.0, 1.0);
            let dirt = ((h - 18.0) / 8.0).clamp(0.0, 1.0) * (1.0 - rock);
            let grass = (1.0 - sand - rock - dirt).max(0.0);
            [grass, dirt, rock, sand].map(|w| (w * 255.0) as u8)
        })
        .collect();
    let weights = ctx
        .gfx
        .create_texture_array(cells as u32, cells as u32, &[&weights], TextureKind::Data);
    let desc = TerrainMaterialDesc::new(layers, weights, vec2(origin, origin), vec2(area, area))
        .with_layer_size([5.0, 4.0, 7.0, 3.0, 4.0, 4.0, 4.0, 4.0]);
    ctx.gfx.create_terrain_material(&desc)
}

/// The lake: waves, the bottom seen through it, and the hills mirrored in
/// it (custom_texture0, drawn by a target camera below the water).
/// custom[0]: wave height, wave length, speed, clarity.
const LAKE: &str = "
fn vertex(v: Vertex) -> Vertex {
    var out = v;
    let p = v.position.xz / material.custom[0].y;
    let t = frame.time.x * material.custom[0].z;
    out.position.y += (sin(p.x + t) + sin(p.y * 1.3 + t * 0.8)) * material.custom[0].x;
    return out;
}

fn surface(s: Surface) -> Surface {
    var out = s;
    let p = s.world.xz / material.custom[0].y;
    let t = frame.time.x * material.custom[0].z;
    let slope = vec2<f32>(cos(p.x + t), cos(p.y * 1.3 + t * 0.8) * 1.3) * material.custom[0].x / material.custom[0].y;
    let fine = vec2<f32>(noise2(s.world.xz * 3.0 + t), noise2(s.world.zx * 3.0 - t)) - 0.5;
    out.normal = normalize(vec3<f32>(-slope.x - fine.x * 0.15, 1.0, -slope.y - fine.y * 0.15));

    let bent = s.screen + out.normal.xz * 25.0;
    let depth = max(scene_distance(bent) - distance(s.world, frame.camera.xyz), 0.0);
    let seen = exp(-depth * material.custom[0].w);
    let fresnel = mix(0.02, 1.0, pow(1.0 - max(dot(out.normal, s.view), 0.0), 5.0));

    // The mirror image, upside down, nudged by the ripples.
    let uv = s.screen * frame.screen.zw;
    let mirrored = vec2<f32>(uv.x, 1.0 - uv.y) + out.normal.xz * 0.008;
    let reflected = textureSample(custom_texture0, material_sampler, mirrored).rgb;

    out.emissive = scene_color(bent) * seen * (1.0 - fresnel) + reflected * fresnel;
    out.albedo = s.albedo * (1.0 - seen) * (1.0 - fresnel);
    out.alpha = 1.0;
    out.roughness = 0.05;
    return out;
}";

const UNDERWATER: &str = "
fn effect(p: Pixel) -> vec3<f32> {
    if frame.camera.y > material.custom[0].x {
        return p.color;
    }
    let deep = vec3<f32>(0.02, 0.12, 0.18);
    let fade = 1.0 - exp(-p.distance * 0.05);
    return mix(p.color * vec3<f32>(0.6, 0.9, 1.0), deep, fade);
}";

fn main() {
    let view = std::env::var("VIEW").unwrap_or_default();
    let hour = std::env::var("HOUR").ok().and_then(|h| h.parse().ok()).unwrap_or(16.0);
    // The first spot nearest the middle where the ground fits.
    let spot = |fits: &dyn Fn(f32) -> bool| {
        (0..200 * 200)
            .map(|i| vec2((i % 200) as f32 * 4.0 - 400.0, (i / 200) as f32 * 4.0 - 400.0))
            .filter(|p| fits(height(p.x, p.y)))
            .min_by(|a, b| a.length().total_cmp(&b.length()))
            .unwrap_or(Vec2::ZERO)
    };
    let (start, (mut yaw, mut pitch)) = match view.as_str() {
        "low" => {
            let p = spot(&|h| h > WATER + 4.0);
            (vec3(p.x, height(p.x, p.y) + 1.8, p.y), (0.6f32, -0.08f32))
        }
        "under" => {
            let p = spot(&|h| h < WATER - 2.0 && h > WATER - 5.0);
            (vec3(p.x, WATER - 1.0, p.y), (0.6, -0.6))
        }
        "cliff" => {
            // The steepest ground near the middle, seen from 30 m off its foot.
            let slope = |p: Vec2| {
                (height(p.x + 2.0, p.y) - height(p.x - 2.0, p.y)).hypot(height(p.x, p.y + 2.0) - height(p.x, p.y - 2.0))
            };
            let steep = (0..200 * 200)
                .map(|i| vec2((i % 200) as f32 * 4.0 - 400.0, (i / 200) as f32 * 4.0 - 400.0))
                .filter(|p| height(p.x, p.y) > WATER + 2.0)
                .max_by(|a, b| slope(*a).total_cmp(&slope(*b)))
                .unwrap_or(Vec2::ZERO);
            // Downhill, away from the face.
            let down = -vec2(
                height(steep.x + 2.0, steep.y) - height(steep.x - 2.0, steep.y),
                height(steep.x, steep.y + 2.0) - height(steep.x, steep.y - 2.0),
            )
            .normalize_or_zero();
            let eye = steep + down * 30.0;
            let at = vec3(eye.x, height(eye.x, eye.y).max(WATER) + 6.0, eye.y);
            let to = vec3(steep.x, height(steep.x, steep.y), steep.y) - at;
            (at, (f32::atan2(-to.x, -to.z), (to.y / to.length()).asin()))
        }
        _ => (vec3(-300.0, 90.0, 300.0), (-0.8, -0.25)),
    };
    let mut parked: Vec<ScreenEffect> = Vec::new();
    let bench = std::env::var("BENCH").is_ok();
    Game::new("Ponko: terrain", 1280, 720)
        .vsync(!bench)
        .fullscreen(bench)
        .system(move |ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            let dt = ctx.time.dt();
            if ctx.input.key_down(Key::Left) {
                yaw += dt;
            }
            if ctx.input.key_down(Key::Right) {
                yaw -= dt;
            }
            if ctx.input.key_down(Key::Up) {
                pitch = (pitch + dt).min(1.4);
            }
            if ctx.input.key_down(Key::Down) {
                pitch = (pitch - dt).max(-1.4);
            }
            let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
            let forward = rotation * Vec3::NEG_Z;
            let right = rotation * Vec3::X;
            let speed = 60.0 * dt;
            let mut go = Vec3::ZERO;
            for (key, dir) in [
                (Key::W, forward),
                (Key::S, -forward),
                (Key::D, right),
                (Key::A, -right),
                (Key::E, Vec3::Y),
                (Key::Q, Vec3::NEG_Y),
            ] {
                if ctx.input.key_down(key) {
                    go += dir;
                }
            }
            ctx.scene3d.camera.position += go * speed;
            ctx.scene3d.camera.rotation = rotation;
            // The lake's mirror follows the camera.
            if let Some(mirror) = ctx.scene3d.passes.target_cameras.first().map(|c| c.target) {
                ctx.scene3d.passes.target_cameras[0] = TargetCamera::reflection(ctx.scene3d.camera, mirror, WATER);
            }
            if ctx.input.key_pressed(Key::U) {
                std::mem::swap(&mut ctx.scene3d.passes.effects, &mut parked);
            }
            if ctx.time.frame() == 3
                && let Ok(path) = std::env::var("SHOT")
            {
                ctx.gfx.save_screenshot(path);
            }
            if ctx.time.frame() == 5 && std::env::var("SHOT").is_ok() {
                ctx.quit();
            }
            if ctx.time.frame() == 300 {
                let stats = ctx.perf.render_stats();
                println!("{stats:?} models drawn: {}", ctx.gfx.models_drawn());
                if bench {
                    ctx.quit();
                }
            }
        })
        .setup(move |ctx| {
            ctx.scene3d.post.bloom = 0.1;
            ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(hour);
            ctx.scene3d.lighting.shadow_distance = 400.0;
            ctx.scene3d.camera.far = 3000.0;
            ctx.scene3d.sky.fog_density = 0.0015;
            ctx.scene3d.sky.fog_height = 0.0;
            ctx.scene3d.sky.fog_falloff = 60.0;

            let ground = ground(ctx);
            let half = TILES / 2;
            for tz in -half..=half {
                for tx in -half..=half {
                    let corner = vec3(tx as f32 * TILE - TILE / 2.0, 0.0, tz as f32 * TILE - TILE / 2.0);
                    let heights: Vec<f32> = (0..N * N)
                        .map(|i| height(corner.x + (i % N) as f32 * 2.0, corner.z + (i / N) as f32 * 2.0))
                        .collect();
                    let tile = ctx.gfx.create_terrain_tile(&heights, TILE, ground);
                    ctx.world.spawn((Transform::at(corner), tile));
                }
            }

            // Half the window's size is plenty for a rippled mirror.
            let size = ctx.window_size();
            let mirror = ctx
                .gfx
                .create_render_target((size.x as u32 / 2).max(1), (size.y as u32 / 2).max(1));
            ctx.scene3d
                .passes
                .target_cameras
                .push(TargetCamera::reflection(ctx.scene3d.camera, mirror, WATER));
            let water = ctx.gfx.create_shader("lake", LAKE);
            let lake = ctx.gfx.create_material(
                MaterialDesc::color(Color::hex(0x1E4D6B))
                    .with_shader(water)
                    .with_blend(Blend::Transparent)
                    .with_custom_textures([Some(mirror), None])
                    .with_custom([vec4(0.06, 1.5, 1.2, 1.2), Vec4::ZERO, Vec4::ZERO, Vec4::ZERO]),
            );
            let surface = ctx.gfx.create_mesh(&MeshData::plane(TILES as f32 * TILE), lake);
            ctx.world
                .spawn((Transform::at(vec3(0.0, WATER, 0.0)), Model::new(surface)));

            let underwater = ctx.gfx.create_effect("underwater", UNDERWATER);
            let level = ctx.gfx.create_material(MaterialDesc::default().with_custom([
                vec4(WATER, 0.0, 0.0, 0.0),
                Vec4::ZERO,
                Vec4::ZERO,
                Vec4::ZERO,
            ]));
            ctx.scene3d
                .passes
                .effects
                .push(ScreenEffect::new(underwater).with_material(level));

            ctx.scene3d.camera.position = start;
        })
        .run();
}
