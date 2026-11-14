//! 2D particles: a campfire (smoke under fire, sparks on top), a crab in
//! between, a glowing trail that follows the mouse, and a burst on click.
//!
//! Run with: `cargo run --example particles2d`
//!
//! - Smoke blends by alpha below the crab; fire and sparks add light above it
//!   (each emitter's `z` places it among the sprites)
//! - The mouse trail lives in the emitter's own space (`ParticleSpace::Local`)
//! - Left click: a one-shot burst of confetti where you click
//! - Escape: quit

use ponko::d2::{ParticleEmitter, ParticleSettings, ParticleShape, Sprite, Transform};
use ponko::particles::ParticleSpace;
use ponko::particles::{ParticleCurve, ParticleGradient};
use ponko::prelude::*;
use ponko::render::BlendMode;

/// A white disc fading out to its edge: one soft particle.
fn soft_circle(ctx: &mut Ctx, size: u32) -> Texture {
    let center = Vec2::splat(size as f32 / 2.0);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = vec2(x as f32 + 0.5, y as f32 + 0.5).distance(center) / center.x;
            let a = (1.0 - d).clamp(0.0, 1.0).powi(2);
            pixels.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
        }
    }
    ctx.gfx.create_texture(size, size, &pixels)
}

/// The mouse trail.
#[derive(Component)]
struct FollowMouse;

/// A one-shot burst, despawned once its particles are gone.
#[derive(Component)]
struct Confetti;

// ANCHOR: particles2d
fn campfire(ctx: &mut Ctx, at: Vec2, soft: Texture) {
    // Smoke: big, slow, alpha blended, under the crab (z -1).
    let smoke = ParticleSettings::default()
        .with_rate(18.0)
        .with_lifetime(2.0, 3.5)
        .with_speed(20.0, 45.0)
        .with_shape(ParticleShape::Cone { angle: 0.35 })
        .with_gravity(vec2(8.0, -10.0)) // drifts up and to the right
        .with_size(24.0, 40.0)
        .with_rotation(0.0, std::f32::consts::TAU)
        .with_size_over_life(ParticleCurve::new(vec![(0.0, 0.4), (1.0, 2.0)]))
        .with_color_over_life(ParticleGradient::new(vec![
            (0.0, Color::rgba(0.3, 0.3, 0.32, 0.0)),
            (0.2, Color::rgba(0.3, 0.3, 0.32, 0.5)),
            (1.0, Color::rgba(0.2, 0.2, 0.22, 0.0)),
        ]))
        .with_texture(soft)
        .with_z(-1.0)
        .with_seed(1);
    // Fire: additive, above the crab (z 1).
    let fire = ParticleSettings::default()
        .with_rate(90.0)
        .with_lifetime(0.4, 0.9)
        .with_speed(40.0, 90.0)
        .with_shape(ParticleShape::Rect {
            half_extents: vec2(14.0, 2.0),
        })
        .with_gravity(vec2(0.0, -60.0))
        .with_size(14.0, 22.0)
        .with_size_over_life(ParticleCurve::new(vec![(0.0, 1.0), (1.0, 0.2)]))
        .with_color_over_life(ParticleGradient::new(vec![
            (0.0, Color::rgba(1.0, 0.85, 0.4, 0.9)),
            (0.5, Color::rgba(1.0, 0.4, 0.1, 0.6)),
            (1.0, Color::rgba(0.6, 0.1, 0.0, 0.0)),
        ]))
        .with_texture(soft)
        .with_blend(BlendMode::Additive)
        .with_z(1.0)
        .with_seed(2);
    // Sparks: small, fast, falling back down, on top of everything (z 2).
    let sparks = ParticleSettings::default()
        .with_rate(25.0)
        .with_lifetime(0.8, 1.6)
        .with_speed(120.0, 220.0)
        .with_shape(ParticleShape::Cone { angle: 0.5 })
        .with_gravity(vec2(0.0, 260.0))
        .with_drag(0.6)
        .with_size(2.0, 4.0)
        .with_color_over_life(ParticleGradient::new(vec![
            (0.0, Color::rgb(1.0, 0.95, 0.6)),
            (1.0, Color::rgba(1.0, 0.4, 0.0, 0.0)),
        ]))
        .with_blend(BlendMode::Additive)
        .with_z(2.0)
        .with_seed(3);
    for settings in [smoke, fire, sparks] {
        ctx.world.spawn((Transform::at(at), ParticleEmitter::new(settings)));
    }
}
// ANCHOR_END: particles2d

fn confetti(at: Vec2, seed: u64) -> (Transform, ParticleEmitter, Confetti) {
    let settings = ParticleSettings::default()
        .with_rate(0.0) // bursts only
        .with_burst(120)
        .with_looping(false)
        .with_lifetime(0.8, 1.4)
        .with_speed(80.0, 260.0)
        .with_shape(ParticleShape::Circle { radius: 6.0 })
        .with_gravity(vec2(0.0, 300.0))
        .with_drag(1.5)
        .with_size(3.0, 6.0)
        .with_rotation(0.0, std::f32::consts::TAU)
        .with_color_over_life(ParticleGradient::new(vec![
            (0.0, Color::rgb(0.4, 0.9, 1.0)),
            (0.6, Color::rgb(1.0, 0.5, 0.9)),
            (1.0, Color::rgba(1.0, 1.0, 0.4, 0.0)),
        ]))
        .with_z(3.0)
        .with_seed(seed);
    (Transform::at(at), ParticleEmitter::new(settings), Confetti)
}

fn main() {
    Game::new("Ponko: 2D particles", 800, 600).run(|ctx| {
        ctx.gfx.clear(Color::hex(0x10141C));
        let soft = soft_circle(ctx, 32);
        let crab = ctx.gfx.load_texture("crab.png");
        ctx.gfx.set_texture_filter(crab, TextureFilter::Nearest);

        let fire_at = vec2(400.0, 420.0);
        campfire(ctx, fire_at, soft);
        // The crab sits between the smoke (z -1) and the fire (z 1).
        ctx.world.spawn((
            Transform::at(fire_at + vec2(0.0, -10.0)).scaled(Vec2::splat(3.0)),
            Sprite::new(crab),
        ));

        // A glowing trail around the mouse: local space, so living particles
        // move with it.
        let trail = ParticleSettings::default()
            .with_rate(60.0)
            .with_lifetime(0.5, 0.8)
            .with_speed(10.0, 30.0)
            .with_shape(ParticleShape::Circle { radius: 12.0 })
            .with_gravity(Vec2::ZERO)
            .with_size(10.0, 16.0)
            .with_size_over_life(ParticleCurve::new(vec![(0.0, 1.0), (1.0, 0.0)]))
            .with_color_over_life(ParticleGradient::new(vec![
                (0.0, Color::rgba(0.3, 0.6, 1.0, 0.8)),
                (1.0, Color::rgba(0.1, 0.2, 1.0, 0.0)),
            ]))
            .with_texture(soft)
            .with_blend(BlendMode::Additive)
            .with_space(ParticleSpace::Local)
            .with_z(4.0);
        ctx.world
            .spawn((Transform::default(), ParticleEmitter::new(trail), FollowMouse));

        let mut bursts = 0u64;
        move |ctx: &mut Ctx| {
            let mouse = ctx.input.mouse_pos();
            ctx.world
                .query::<(&mut Transform, &FollowMouse)>()
                .for_each(|(t, _)| t.position = mouse);
            // Bursts emit in the frame they're spawned, so an empty one is done.
            let mut done = Vec::new();
            ctx.world
                .query::<(Entity, &ParticleEmitter, &Confetti)>()
                .for_each(|(e, p, _)| {
                    if p.live_count() == 0 {
                        done.push(e);
                    }
                });
            for e in done {
                ctx.world.despawn(e);
            }
            if ctx.input.mouse_pressed(MouseButton::Left) {
                bursts += 1;
                ctx.world.spawn(confetti(mouse, bursts));
            }
            let mut live = 0;
            ctx.world
                .query::<&ParticleEmitter>()
                .for_each(|e| live += e.live_count());
            ctx.gfx.draw_text(
                &format!("{live} particles - click for confetti"),
                vec2(16.0, 16.0),
                18.0,
                Color::WHITE,
            );
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
        }
    });
}
