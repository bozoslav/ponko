//! A target effect: a cheap depth of field. Each frame a target effect
//! copies the lit frame into a quarter-size render target (after the
//! transparent surfaces, before screen effects), and a screen effect then
//! blends distant pixels toward that soft copy, in the same frame.
//!
//!   `cargo run --release --example target_effect --features 3d`
//!
//! `SHOT=path.png` saves frame 60 and quits.

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{MaterialDesc, MeshData, ScreenEffect, Sky, TargetEffect};

/// Writes the frame's light into its render target; a smaller target than
/// the frame is a coarse copy, which the screen effect samples smoothly.
const COPY: &str = "
fn effect(p: Pixel) -> vec3<f32> {
    return p.color;
}";

/// Blends toward the soft copy (custom_texture0) with distance from the
/// focus (material.custom[0].x), over material.custom[0].y world units.
const FOCUS: &str = "
fn effect(p: Pixel) -> vec3<f32> {
    let soft = textureSampleLevel(custom_texture0, material_sampler, p.uv, 0.0).rgb;
    let focus = material.custom[0];
    let blur = select(clamp(abs(p.distance - focus.x) / focus.y, 0.0, 1.0), 1.0, p.sky);
    return mix(p.color, soft, blur);
}";

fn main() {
    Game::new("Ponko: target effect", 1280, 720)
        .system(|ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            if ctx.time.frame() == 60
                && let Ok(path) = std::env::var("SHOT")
            {
                ctx.gfx.save_screenshot(path);
            }
            if ctx.time.frame() == 62 && std::env::var("SHOT").is_ok() {
                ctx.quit();
            }
        })
        .setup(|ctx| {
            ctx.scene3d.camera.position = vec3(0.0, 2.0, 6.0);
            ctx.scene3d.camera.look_at(vec3(0.0, 0.5, 0.0));
            ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(15.0);

            // The quarter-size copy, written by the target effect each frame.
            let soft = ctx.gfx.create_render_target(320, 180);
            let copy = ctx.gfx.create_effect("copy", COPY);
            ctx.scene3d.passes.target_effects.push(TargetEffect::new(copy, soft));

            // The screen effect reads it in the same frame.
            let focus = ctx.gfx.create_effect("focus", FOCUS);
            let settings = ctx.gfx.create_material(
                MaterialDesc::default()
                    .with_custom([vec4(6.0, 8.0, 0.0, 0.0), Vec4::ZERO, Vec4::ZERO, Vec4::ZERO])
                    .with_custom_textures([Some(soft), None]),
            );
            ctx.scene3d
                .passes
                .effects
                .push(ScreenEffect::new(focus).with_material(settings));

            let stone = ctx
                .gfx
                .create_material(MaterialDesc::color(Color::hex(0xC0A080)).with_roughness(0.7));
            let block = ctx.gfx.create_mesh(&MeshData::cube(1.0), stone);
            for i in 0..12 {
                let z = -(i as f32) * 2.5;
                for x in [-1.5, 1.5] {
                    ctx.world.spawn((Transform::at(vec3(x, 0.5, z)), Model::new(block)));
                }
            }
            let grass = ctx
                .gfx
                .create_material(MaterialDesc::color(Color::hex(0x4E6B33)).with_roughness(0.95));
            let ground = ctx.gfx.create_mesh(&MeshData::plane(200.0), grass);
            ctx.world.spawn((Transform::default(), Model::new(ground)));
        })
        .run();
}
