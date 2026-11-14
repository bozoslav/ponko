//! Custom material shaders: a lake with waves, grass in the wind, and a
//! procedural pattern, with post-processing.
//!
//!   `cargo run --release --example shaders --features 3d`
//!
//! The shaders are in `assets/shaders/`: edit one while the game runs and
//! it reloads. Left and right arrows turn the time of day, F switches fog.
//! `SHOT=path.png` saves the second frame and quits; `HOUR=19` and `FOG=1`
//! set the time and the fog.

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::prelude::*;
use ponko::render::{Blend, MaterialDesc, MeshData, Sky};

/// One blade of grass: a thin quad, texture v from 0 at the root to 1 at the tip.
fn blade() -> MeshData {
    MeshData {
        positions: vec![
            vec3(-0.03, 0.0, 0.0),
            vec3(0.03, 0.0, 0.0),
            vec3(0.01, 0.6, 0.0),
            vec3(-0.01, 0.6, 0.0),
        ],
        normals: vec![Vec3::Z; 4],
        uvs: vec![vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0)],
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Default::default()
    }
}

/// A pattern made in the shader: no texture needed.
const TILES: &str = "
fn surface(s: Surface) -> Surface {
    var out = s;
    let cell = floor(s.uv * vec2<f32>(16.0, 8.0));
    let checker = (cell.x + cell.y) % 2.0;
    out.albedo = mix(vec3<f32>(0.8, 0.3, 0.1), vec3<f32>(0.95, 0.85, 0.6), checker);
    out.roughness = mix(0.25, 0.7, checker);
    return out;
}";

fn main() {
    let env = |name| std::env::var(name).ok().and_then(|v| v.parse::<f32>().ok());
    let mut hour = env("HOUR").unwrap_or(15.5);
    let mut fog = env("FOG").unwrap_or(0.0) > 0.0;
    Game::new("Ponko: shaders", 1280, 720)
        .system(move |ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            // Two hours a second.
            if ctx.input.key_down(Key::Right) {
                hour = (hour + ctx.time.dt() * 2.0) % 24.0;
            }
            if ctx.input.key_down(Key::Left) {
                hour = (hour - ctx.time.dt() * 2.0).rem_euclid(24.0);
            }
            if ctx.input.key_pressed(Key::F) {
                fog = !fog;
            }
            ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(hour);
            ctx.scene3d.sky.fog_density = if fog { 0.02 } else { 0.0 };
            ctx.scene3d.sky.fog_falloff = 8.0;
            if ctx.time.frame() == 2
                && let Ok(path) = std::env::var("SHOT")
            {
                ctx.gfx.save_screenshot(path);
            }
            if ctx.time.frame() == 4 && std::env::var("SHOT").is_ok() {
                ctx.quit();
            }
        })
        .setup(|ctx| {
            ctx.scene3d.camera.position = vec3(0.0, 4.0, 11.0);
            ctx.scene3d.camera.look_at(vec3(0.0, 0.5, 0.0));
            ctx.scene3d.post.bloom = 0.1;

            let ground_look = MaterialDesc::color(Color::hex(0x5E7A3A)).with_roughness(0.95);
            let ground = ctx.gfx.create_material(ground_look);
            let ground = ctx.gfx.create_mesh(&MeshData::plane(600.0), ground);
            ctx.world.spawn((Transform::default(), Model::new(ground)));

            let water = ctx.gfx.load_shader("shaders/water.wgsl");
            let lake = ctx.gfx.create_material(
                MaterialDesc::color(Color::hex(0x1E4D6B))
                    .with_shader(water)
                    .with_blend(Blend::Transparent)
                    .with_custom([vec4(0.06, 1.5, 1.2, 1.2), Vec4::ZERO, Vec4::ZERO, Vec4::ZERO]),
            );
            let mut surface = MeshData::plane(12.0);
            // More vertices, so the waves have something to move.
            surface = subdivide(surface, 5);
            let lake = ctx.gfx.create_mesh(&surface, lake);
            ctx.world
                .spawn((Transform::at(vec3(-3.0, 0.15, -4.0)), Model::new(lake)));

            // The lake bed, under the water: sand sloping down, and stones.
            let sand = ctx
                .gfx
                .create_material(MaterialDesc::color(Color::hex(0xC2A878)).with_roughness(0.9));
            let bed = ctx.gfx.create_mesh(&MeshData::plane(12.0), sand);
            let tilt = Quat::from_rotation_x(0.12);
            ctx.world.spawn((
                Transform {
                    rotation: tilt,
                    ..Transform::at(vec3(-3.0, -0.6, -4.0))
                },
                Model::new(bed),
            ));
            let stone = ctx
                .gfx
                .create_material(MaterialDesc::color(Color::hex(0x8A8A80)).with_roughness(0.7));
            let pebble = ctx.gfx.create_mesh(&MeshData::sphere(0.35, 16), stone);
            for i in 0..12 {
                let at = vec3(
                    -7.0 + i as f32 * 0.8,
                    -0.3 - (i % 3) as f32 * 0.2,
                    -3.0 + (i % 4) as f32 * -1.5,
                );
                ctx.world.spawn((Transform::at(at), Model::new(pebble)));
            }

            let wind = ctx.gfx.load_shader("shaders/wind.wgsl");
            let grass = ctx.gfx.create_material(
                MaterialDesc::color(Color::hex(0x7FA83C))
                    .with_shader(wind)
                    .with_double_sided(true)
                    .with_roughness(0.8)
                    .with_custom([vec4(0.25, 2.0, 0.0, 0.0), Vec4::ZERO, Vec4::ZERO, Vec4::ZERO]),
            );
            let blade = ctx.gfx.create_mesh(&blade(), grass);
            let mut seed = 7u32;
            let mut random = move || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 24) as f32
            };
            for _ in 0..6000 {
                let at = vec3(random() * 14.0 - 2.0, 0.0, random() * 10.0 - 3.0);
                if at.x < 4.0 && at.z < 2.5 {
                    continue; // the lake
                }
                let t = Transform {
                    rotation: Quat::from_rotation_y(random() * 6.3),
                    scale: Vec3::splat(0.6 + random() * 0.8),
                    ..Transform::at(at)
                };
                ctx.world.spawn((t, Model::new(blade)));
            }

            let tiles = ctx.gfx.create_shader("tiles", TILES);
            let ball = ctx.gfx.create_material(MaterialDesc::default().with_shader(tiles));
            let ball = ctx.gfx.create_mesh(&MeshData::sphere(1.0, 48), ball);
            ctx.world.spawn((Transform::at(vec3(3.5, 1.0, 1.0)), Model::new(ball)));
        })
        .run();
}

/// Splits every triangle into four, `times` times.
fn subdivide(mut mesh: MeshData, times: u32) -> MeshData {
    for _ in 0..times {
        let mut indices = Vec::with_capacity(mesh.indices.len() * 4);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let mut mid = |a: u32, b: u32| {
                let (a, b) = (a as usize, b as usize);
                mesh.positions.push((mesh.positions[a] + mesh.positions[b]) / 2.0);
                mesh.normals.push(mesh.normals[a]);
                mesh.uvs.push((mesh.uvs[a] + mesh.uvs[b]) / 2.0);
                mesh.positions.len() as u32 - 1
            };
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
            indices.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
        }
        mesh.indices = indices;
    }
    mesh
}
