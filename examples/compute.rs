//! A compute pass: rain on a pond. A wave simulation runs on the GPU each
//! frame in a render target (height now in red, the frame before in
//! green), and the water's shader reads it to bend its normals.
//!
//!   `cargo run --release --example compute --features 3d`
//!
//! Click the water to drop a stone in. `SHOT=path.png` saves frame 120
//! and quits.

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{ComputePass, MaterialDesc, MeshData, Sky};

/// The wave equation on a grid: each texel moves toward the average of
/// its neighbours, overshooting like water does, and calms slowly.
/// material.custom[0]: a drop (uv x, uv y, strength; 0 for none).
const RIPPLES: &str = "
fn compute(c: Cell) -> vec4<f32> {
    let around = cell_at(c.pixel + vec2<i32>(1, 0)).x + cell_at(c.pixel - vec2<i32>(1, 0)).x
        + cell_at(c.pixel + vec2<i32>(0, 1)).x + cell_at(c.pixel - vec2<i32>(0, 1)).x;
    var height = (around * 0.5 - c.value.y) * 0.985;
    let drop = material.custom[0];
    let r = distance(c.uv, drop.xy) * f32(c.size.x);
    if drop.z > 0.0 && r < 3.0 {
        height -= drop.z * (1.0 - r / 3.0);
    }
    return vec4<f32>(height, c.value.x, 0.0, 0.0);
}";

/// The water: its normals follow the simulated heights (custom_texture0).
const WATER: &str = "
fn surface(s: Surface) -> Surface {
    var out = s;
    let texel = 1.0 / vec2<f32>(textureDimensions(custom_texture0));
    let x = textureSample(custom_texture0, material_sampler, s.uv + vec2<f32>(texel.x, 0.0)).x
        - textureSample(custom_texture0, material_sampler, s.uv - vec2<f32>(texel.x, 0.0)).x;
    let z = textureSample(custom_texture0, material_sampler, s.uv + vec2<f32>(0.0, texel.y)).x
        - textureSample(custom_texture0, material_sampler, s.uv - vec2<f32>(0.0, texel.y)).x;
    out.normal = normalize(vec3<f32>(-x * 8.0, 1.0, -z * 8.0));
    out.roughness = 0.06;
    return out;
}";

const SIZE: f32 = 12.0;

fn main() {
    let mut seed = 11u32;
    let mut random = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1 << 24) as f32
    };
    let mut next_drop = 0.0;
    Game::new("Ponko: compute", 1280, 720)
        .system(move |ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            let Some(pass) = ctx.scene3d.passes.compute.first().copied() else {
                return;
            };
            let Some(drops) = pass.material else { return };
            // Rain every so often, or a stone where the mouse clicks.
            let mut drop = Vec4::ZERO;
            next_drop -= ctx.time.dt();
            if next_drop <= 0.0 {
                next_drop = 0.05 + random() * 0.2;
                drop = vec4(random(), random(), 0.3 + random() * 0.4, 0.0);
            }
            if ctx.input.mouse_pressed(MouseButton::Left) {
                let (from, dir) = ctx.scene3d.camera.screen_ray(ctx.input.mouse_pos());
                if dir.y < 0.0 {
                    let at = from + dir * (-from.y / dir.y);
                    drop = vec4(at.x / SIZE + 0.5, at.z / SIZE + 0.5, 3.0, 0.0);
                }
            }
            ctx.gfx.update_material(
                drops,
                MaterialDesc::default().with_custom([drop, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO]),
            );
            if ctx.time.frame() == 120
                && let Ok(path) = std::env::var("SHOT")
            {
                ctx.gfx.save_screenshot(path);
            }
            if ctx.time.frame() == 122 && std::env::var("SHOT").is_ok() {
                ctx.quit();
            }
        })
        .setup(|ctx| {
            ctx.scene3d.camera.position = vec3(0.0, 7.0, 10.0);
            ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -1.0));
            ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(15.0);
            ctx.scene3d.post.bloom = 0.15;

            let heights = ctx.gfx.create_render_target(512, 512);
            let ripples = ctx.gfx.create_compute("ripples", RIPPLES);
            let drops = ctx.gfx.create_material(MaterialDesc::default());
            ctx.scene3d
                .passes
                .compute
                .push(ComputePass::new(ripples, heights).with_material(drops));

            let shader = ctx.gfx.create_shader("water", WATER);
            let water = ctx.gfx.create_material(
                MaterialDesc::color(Color::hex(0x2A5A75))
                    .with_shader(shader)
                    .with_custom_textures([Some(heights), None]),
            );
            let surface = ctx.gfx.create_mesh(&MeshData::plane(SIZE), water);
            ctx.world.spawn((Transform::default(), Model::new(surface)));

            let stone = ctx
                .gfx
                .create_material(MaterialDesc::color(Color::hex(0x8A8578)).with_roughness(0.8));
            let rock = ctx.gfx.create_mesh(&MeshData::sphere(0.6, 16), stone);
            for i in 0..40 {
                let a = i as f32 / 40.0 * std::f32::consts::TAU;
                let edge = vec3(a.cos(), 0.0, a.sin()) * SIZE * 0.55;
                let t = Transform {
                    scale: Vec3::splat(0.8 + (i % 3) as f32 * 0.3),
                    ..Transform::at(edge)
                };
                ctx.world.spawn((t, Model::new(rock)));
            }
            let grass = ctx
                .gfx
                .create_material(MaterialDesc::color(Color::hex(0x4E6B33)).with_roughness(0.95));
            let ground = ctx.gfx.create_mesh(&MeshData::plane(200.0), grass);
            ctx.world
                .spawn((Transform::at(vec3(0.0, -0.3, 0.0)), Model::new(ground)));
        })
        .run();
}
