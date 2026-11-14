//! A dark scene with many colored entity lights (one quarter spotlights).
//! `LIGHTS=1000 cargo run --release --features 3d --example lights`
//! `SHOT=lights.png` saves one frame and exits. For a windowless GPU benchmark:
//! `cargo test --release --features 3d --lib offscreen_light_benchmark -- --ignored --nocapture`
use ponko::prelude::*;

#[path = "common/lights_scene.rs"]
mod scene;
fn main() {
    let count = std::env::var("LIGHTS").ok().and_then(|n| n.parse().ok()).unwrap_or(100);
    let shot = std::env::var("SHOT").ok();
    Game::new("Ponko: point and spot lights", 1280, 720)
        .system(move |ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            if ctx.time.frame() == 5
                && let Some(path) = &shot
            {
                ctx.gfx.save_screenshot(path);
            }
            if ctx.time.frame() == 7 && shot.is_some() {
                ctx.quit();
            }
        })
        .setup(move |ctx| scene::setup(ctx, count))
        .run();
}
