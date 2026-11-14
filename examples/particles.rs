//! CPU billboards: `cargo run --features 3d --example particles`.
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;

// ANCHOR: particles
fn setup(ctx: &mut Ctx) {
    ctx.scene3d.camera.position = vec3(0.0, 2.0, 7.0);
    ctx.scene3d.camera.look_at(vec3(0.0, 1.0, 0.0));
    ctx.scene3d.sky.atmosphere = false;
    let emitter = ParticleEmitter::new(
        ParticleSettings::default()
            .with_rate(120.0)
            .with_burst(40)
            .with_lifetime(0.7, 1.4)
            .with_speed(2.0, 4.0)
            .with_shape(ParticleShape::Cone { angle: 0.45 })
            .with_gravity(vec3(0.0, -3.0, 0.0))
            .with_drag(0.2)
            .with_size(0.05, 0.13)
            .with_rotation(0.0, std::f32::consts::TAU)
            .with_size_over_life(ponko::particles::ParticleCurve::new(vec![
                (0.0, 0.5),
                (0.2, 1.0),
                (1.0, 0.0),
            ]))
            .with_color_over_life(ponko::particles::ParticleGradient::new(vec![
                (0.0, Color::rgba(1.0, 0.7, 0.2, 1.0)),
                (1.0, Color::rgba(1.0, 0.1, 0.0, 0.0)),
            ]))
            .with_blend(ParticleBlend::Additive)
            .with_max_particles(512)
            .with_seed(42),
    );
    ctx.world.spawn((Transform::default(), emitter));
}
// ANCHOR_END: particles

fn main() {
    Game::new("Particles", 960, 640).setup(setup).run();
}
