//! A scene compute pass: eye adaptation. Each frame a compute pass reads
//! the lit frame (after the transparent surfaces, before effects, bloom and
//! tone mapping), measures how bright it is, and blends that into a 1x1
//! render target that remembers the last frames. A screen effect then
//! scales the frame's light by it, in the same frame, so the picture
//! settles to the same brightness as the sun rises and sets.
//!
//! Neither of the older hooks can do this: a compute pass before the scene
//! can't see the frame, and a target effect clears its target, so it can't
//! keep a running average.
//!
//! On a GPU without compute shaders (WebGL2) the pass can't be made: the
//! example says so and draws the frame unadjusted.
//!
//!   `cargo run --release --example scene_compute --features 3d`
//!
//! `SHOT=path.png` saves frame 60 and quits.

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{ComputePass, MaterialDesc, MeshData, ScreenEffect, Sky};

/// The light meter: the average brightness of 16 x 9 points of the frame,
/// blended into what the texel held (`c.value`, the last frames' average)
/// at the rate in `material.custom[0].x` per second.
const METER: &str = "
fn compute(c: Cell) -> vec4<f32> {
    var sum = 0.0;
    for (var y = 0; y < 9; y++) {
        for (var x = 0; x < 16; x++) {
            let at = (vec2<f32>(f32(x), f32(y)) + 0.5) / vec2<f32>(16.0, 9.0) * frame.screen.xy;
            sum += dot(scene_color(at), vec3<f32>(0.2126, 0.7152, 0.0722));
        }
    }
    let now = sum / 144.0;
    // The first frame starts from what it sees, not from 0.
    let rate = select(1.0 - exp(-material.custom[0].x * frame.time.y), 1.0, c.value.a == 0.0);
    return vec4<f32>(mix(c.value.r, now, rate), 0.0, 0.0, 1.0);
}";

/// Scales the frame's light so the measured average (custom_texture0's
/// only texel) becomes `material.custom[0].y`.
const EXPOSE: &str = "
fn effect(p: Pixel) -> vec3<f32> {
    let average = textureLoad(custom_texture0, vec2<i32>(0, 0), 0).r;
    return p.color * material.custom[0].y / max(average, 1e-3);
}";

fn main() {
    Game::new("Ponko: scene compute", 1280, 720)
        .system(|ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            // A day in 40 seconds: the sun rises, crosses and sets.
            let hours = 6.0 + (ctx.time.elapsed() as f32 * 0.3) % 13.0;
            ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(hours);
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

            // The meter's memory: one texel, written only by the compute pass.
            let average = ctx.gfx.create_render_target(1, 1);
            match ctx.gfx.try_create_compute("meter", METER) {
                Ok(meter) => {
                    let adapt = ctx.gfx.create_material(MaterialDesc::default().with_custom([
                        vec4(1.5, 0.0, 0.0, 0.0),
                        Vec4::ZERO,
                        Vec4::ZERO,
                        Vec4::ZERO,
                    ]));
                    ctx.scene3d
                        .passes
                        .scene_compute
                        .push(ComputePass::new(meter, average).with_material(adapt));
                    // The screen effect reads this frame's average.
                    let expose = ctx.gfx.create_effect("expose", EXPOSE);
                    let settings = ctx.gfx.create_material(
                        MaterialDesc::default()
                            .with_custom([vec4(0.0, 0.18, 0.0, 0.0), Vec4::ZERO, Vec4::ZERO, Vec4::ZERO])
                            .with_custom_textures([Some(average), None]),
                    );
                    ctx.scene3d
                        .passes
                        .effects
                        .push(ScreenEffect::new(expose).with_material(settings));
                }
                // No compute shaders here (WebGL2): the frame draws as it is.
                Err(e) => println!("eye adaptation is off on this GPU: {e}"),
            }

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
