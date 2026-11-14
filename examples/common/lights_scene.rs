use super::*;

pub fn geometry() -> [ponko::render::MeshData; 2] {
    [
        ponko::render::MeshData::plane(160.0),
        ponko::render::MeshData::cube(2.0),
    ]
}

pub fn setup(ctx: &mut Ctx, count: usize) {
    ctx.scene3d.camera.position = vec3(0.0, 34.0, 62.0);
    ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -25.0));
    ctx.scene3d.camera.far = 250.0;
    ctx.scene3d.lighting.shadows = false;
    ctx.scene3d.lighting.sun_intensity = 0.0;
    ctx.scene3d.lighting.ambient_intensity = 0.015;
    ctx.scene3d.sky.atmosphere = false;
    ctx.scene3d.post.bloom = 0.0;
    let stone = ctx
        .gfx
        .create_material(ponko::render::MaterialDesc::default().with_roughness(0.5));
    let [floor, cube] = geometry().map(|data| ctx.gfx.create_mesh(&data, stone));
    ctx.world
        .spawn((d3::Transform::at(vec3(0.0, -1.0, -25.0)), Model::new(floor)));
    for z in 0..10 {
        for x in 0..15 {
            ctx.world.spawn((
                d3::Transform::at(vec3((x as f32 - 7.0) * 6.0, 0.0, 10.0 - z as f32 * 7.0)),
                Model::new(cube),
            ));
        }
    }
    for i in 0..count {
        let x = ((i * 37) % 101) as f32 - 50.0;
        let z = 15.0 - ((i * 53) % 101) as f32;
        let color = [
            Color::rgb(1.0, 0.12, 0.03),
            Color::rgb(0.06, 0.3, 1.0),
            Color::rgb(0.05, 1.0, 0.25),
        ][i % 3];
        let t = d3::Transform::at(vec3(x, 3.0, z));
        if i % 4 == 0 {
            ctx.world.spawn((
                t.looking_at(vec3(x + 2.0, -1.0, z - 3.0)),
                ponko::d3::SpotLight {
                    color,
                    intensity: 80.0,
                    range: 12.0,
                    inner_angle: 0.3,
                    outer_angle: 0.7,
                    ..Default::default()
                },
            ));
        } else {
            ctx.world.spawn((
                t,
                ponko::d3::PointLight {
                    color,
                    intensity: 30.0,
                    range: 9.0,
                    ..Default::default()
                },
            ));
        }
    }
}
