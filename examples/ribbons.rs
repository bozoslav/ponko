//! Ribbon trails, lines and decals: `cargo run --features 3d --example ribbons`.
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::prelude::*;
use ponko::render::{Blend, MaterialDesc, MeshData};

/// Flies in a circle.
#[derive(Component)]
struct Orbiting;

fn fly(ctx: &mut Ctx) {
    let t = ctx.time.elapsed() as f32;
    ctx.world.query::<(&mut Transform, &Orbiting)>().for_each(|(at, _)| {
        at.position = vec3(t.cos() * 3.0, 1.0 + (t * 2.0).sin() * 0.5, t.sin() * 3.0);
    });
}

fn setup(ctx: &mut Ctx) {
    ctx.scene3d.camera.position = vec3(0.0, 4.0, 9.0);
    ctx.scene3d.camera.look_at(vec3(0.0, 0.5, 0.0));
    let floor_material = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x6B7B58)));
    let floor_data = MeshData::plane(12.0);
    let floor = ctx.gfx.create_mesh(&floor_data, floor_material);
    ctx.world.spawn((Transform::default(), Model::new(floor)));

    // ANCHOR: trail
    // A glowing tracer behind a flying ball: a quarter second long, thin at
    // the end and fading out.
    let mut tracer = Trail::new(0.25, 0.12);
    tracer.color = ponko::particles::ParticleGradient::new(vec![
        (0.0, Color::rgb(1.0, 0.8, 0.3)),
        (1.0, Color::rgba(1.0, 0.3, 0.0, 0.0)),
    ]);
    tracer.blend = ParticleBlend::Additive;
    let white = ctx.gfx.default_material();
    let ball = ctx.gfx.create_mesh(&MeshData::sphere(0.15, 16), white);
    ctx.world
        .spawn((Transform::default(), Model::new(ball), tracer, Orbiting));
    // After a teleport: ctx.world.get_mut::<d3::Trail>(entity).unwrap().clear();
    // ANCHOR_END: trail

    // ANCHOR: line
    // A laser beam 5 m long, from the entity forward.
    let beam = Line {
        blend: ParticleBlend::Additive,
        ..Line::new(vec![Vec3::ZERO, vec3(0.0, 0.0, -5.0)], 0.03, Color::RED)
    };
    ctx.world.spawn((Transform::at(vec3(-4.0, 1.5, 3.0)), beam));
    // ANCHOR_END: line

    // ANCHOR: decal
    // A 1 m scorch mark on the floor at (1, 0, -1): the box is 1 x 1 m wide
    // and 0.4 m deep, its +Z turned up, so the image is projected down.
    let up = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let projector = Mat4::from_scale_rotation_translation(vec3(1.0, 1.0, 0.4), up, vec3(1.0, 0.0, -1.0));
    let mark = floor_data.decal(projector, 0.003);
    if !mark.indices.is_empty() {
        let scorch = ctx
            .gfx
            .create_material(MaterialDesc::color(Color::rgba(0.05, 0.04, 0.03, 0.8)).with_blend(Blend::Transparent));
        let mark = ctx.gfx.create_mesh(&mark, scorch);
        // Drawn where the floor is.
        ctx.world.spawn((Transform::default(), Model::new(mark)));
    }
    // A mesh from a glTF file works the same: ctx.gfx.load_mesh_data("models/wall.glb")?.decal(..)
    // ANCHOR_END: decal
}

fn main() {
    Game::new("Ribbons", 960, 640).system(fly).setup(setup).run();
}
