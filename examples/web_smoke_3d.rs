//! The 3D browser smoke-test fixture: `web_smoke` for the 3D renderer
//! (`web/smoke/fixtures.json`, built with `--features 3d`).
//!
//! It loads one glTF model (`models/house.glb`, fetched over HTTP on the web
//! and parsed in the browser), draws it with a sun with shadows, a few
//! spheres with different materials and a ground plane, and orbits the camera
//! from the fixed-step simulation. The state it publishes (see
//! `common/web_smoke.rs`) adds the triangles and models drawn last frame, so a
//! scene that loaded but draws nothing is caught:
//!
//! `{"fixture":"web_smoke_3d","ticks":120,"frames":118,"clicks":1,"size":[800,600],"triangles":1234,"models_drawn":8}`
//!
//! The failure cases are `web_smoke`'s. On the desktop it opens a window
//! (`cargo run --example web_smoke_3d --features 3d`).

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{MaterialDesc, MeshData, ModelMaterial};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use serde_json::json;
use std::cell::Cell;
use std::rc::Rc;

#[path = "common/web_smoke.rs"]
mod smoke;

fn main() {
    let ticks = Rc::new(Cell::new(0u64));
    let sim_ticks = ticks.clone();
    Game::new("Ponko: 3D web smoke", 800, 600)
        .fixed_hz(60)
        .sim_system(move |_ctx| sim_ticks.set(sim_ticks.get() + 1))
        .system(frame(ticks))
        .setup(|ctx| {
            let smoke = smoke::Smoke::new("web_smoke_3d");
            ctx.scene3d.lighting.sun_direction = vec3(-0.8, -0.6, -0.4);
            let ground_material = ctx
                .gfx
                .create_material(MaterialDesc::color(Color::hex(0x5E8C4A)).with_roughness(0.95));
            let ground = ctx.gfx.create_mesh(&MeshData::plane(60.0), ground_material);
            ctx.world.spawn((Transform::default(), Model::new(ground)));

            // The fetched asset: a model with its own materials and textures.
            let house = ctx.gfx.load_mesh("models/house.glb");
            ctx.world
                .spawn((Transform::at(vec3(0.0, 0.0, -3.0)), Model::new(house)));

            let white = ctx.gfx.default_material();
            let ball = ctx.gfx.create_mesh(&MeshData::sphere(0.45, 24), white);
            for i in 0..6 {
                let material = ctx.gfx.create_material(
                    MaterialDesc::color(Color::hex(0xD8453B))
                        .with_roughness(0.1 + i as f32 * 0.17)
                        .with_metallic((i % 2) as f32),
                );
                ctx.world.spawn((
                    Transform::at(vec3(i as f32 * 1.1 - 2.75, 0.45, 2.0)),
                    Model::new(ball),
                    ModelMaterial(material),
                ));
            }

            // A sphere with a normal map and an ORM map, made in memory: data
            // textures, which a browser with only WebGL2 must read as stored
            // (not sRGB-decoded) to light the bumps.
            let (bumps, orm) = data_maps();
            let normal_map = ctx.gfx.create_texture(16, 16, &bumps);
            let orm_map = ctx.gfx.create_texture(16, 16, &orm);
            let bumpy = ctx.gfx.create_material(
                MaterialDesc::color(Color::hex(0xD8D8D8))
                    .with_normal_map(normal_map)
                    .with_orm_map(orm_map),
            );
            let bumpy_ball = ctx.gfx.create_mesh(&MeshData::sphere(0.9, 32), bumpy);
            ctx.world
                .spawn((Transform::at(vec3(3.0, 0.9, -1.0)), Model::new(bumpy_ball)));
            ctx.world.insert_resource(Fixture(smoke));
        })
        .run();
}

/// The fixture's counters, made in setup for the frame system.
struct Fixture(smoke::Smoke);

/// The frame system: moves the camera and publishes the state.
fn frame(ticks: Rc<Cell<u64>>) -> impl FnMut(&mut Ctx) {
    move |ctx: &mut Ctx| {
        // The camera orbits on the simulation clock, so a stalled
        // simulation also freezes the picture.
        let angle = ticks.get() as f32 / 240.0;
        ctx.scene3d.camera.position = vec3(angle.sin() * 12.0, 6.0, angle.cos() * 12.0);
        ctx.scene3d.camera.look_at(vec3(0.0, 1.0, 0.0));
        // Taken out while it runs, as `Smoke::frame` needs all of `ctx`.
        let mut fixture = ctx.world.remove_resource::<Fixture>().expect("made in setup");
        fixture.0.frame(
            ctx,
            ticks.get(),
            json!({ "triangles": ctx.perf.render_stats().triangles, "models_drawn": ctx.gfx.models_drawn() }),
        );
        ctx.world.insert_resource(fixture);
    }
}

/// A 16x16 tangent-space normal map of a grid of round bumps, and an ORM map
/// (occlusion, roughness, metallic) that varies across it.
fn data_maps() -> (Vec<u8>, Vec<u8>) {
    let mut normals = Vec::with_capacity(16 * 16 * 4);
    let mut orm = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            // The slope of a bump 8 texels wide, as a unit normal in 0..255.
            let (u, v) = ((x % 8) as f32 - 3.5, (y % 8) as f32 - 3.5);
            let n = vec3(-u * 0.12, -v * 0.12, 1.0).normalize();
            normals.extend([
                (n.x * 127.5 + 127.5) as u8,
                (n.y * 127.5 + 127.5) as u8,
                (n.z * 127.5 + 127.5) as u8,
                255,
            ]);
            orm.extend([255, (60 + x * 12) as u8, if y < 8 { 0 } else { 255 }, 255]);
        }
    }
    (normals, orm)
}
