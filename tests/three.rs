//! 3D without a window: meshes, materials and models in headless runs,
//! glTF loading, and transforms as JSON.
#![cfg(feature = "3d")]

mod common;

use common::TempAsset;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::{PointLight, SpotLight, Transform};
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::Blend;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{
    AmbientOcclusion, AntiAliasing, ComputePass, GlobalIllumination, GpuBuffer, GraphicsQuality, InstanceBatchOptions,
    InstanceLevel, LodGroup, LodHysteresis, LodLevel, LodPick, LodRenderer, LodShadow, Material, MaterialDesc, Mesh,
    MeshData, Model, ModelMaterial, Post, ScreenEffect, Sky, TargetCamera, TargetEffect, Tonemap,
};
#[allow(unused_imports)]
use ponko::render::{GpuFeature, GpuTier};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::terrain::{FarTerrain, Heightfield, TerrainMaterialDesc, TerrainTile, TerrainTileData, TextureKind};

// ANCHOR: three_d
/// Turns around its own Y axis, this many radians a second.
#[derive(Component)]
struct Spin(f32);

fn village(ctx: &mut Ctx) {
    let grass = ctx
        .gfx
        .create_material(MaterialDesc::color(Color::hex(0x5E8C4A)).with_roughness(0.9));
    let ground = ctx.gfx.create_mesh(&MeshData::plane(100.0), grass);
    ctx.world.spawn((Transform::default(), Model::new(ground)));

    let house = ctx.gfx.load_mesh("models/house.glb"); // its walls and roof keep their materials
    for i in 0..5 {
        let at = vec3(i as f32 * 4.0 - 8.0, 0.0, -6.0);
        ctx.world.spawn((Transform::at(at), Model::new(house), Spin(0.5)));
    }

    ctx.scene3d.camera.position = vec3(0.0, 6.0, 12.0);
    ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -4.0));
    ctx.scene3d.lighting.sun_direction = vec3(-0.8, -0.6, -0.4); // late afternoon: long shadows
}

fn spin(ctx: &mut Ctx) {
    let dt = ctx.dt();
    ctx.world.query::<(&mut Transform, &Spin)>().for_each(|(t, s)| {
        t.rotation *= Quat::from_rotation_y(s.0 * dt);
    });
}
// ANCHOR_END: three_d

#[test]
fn the_guide_village_has_five_spinning_houses() {
    let mut world = Game::new("village", 640, 480)
        .fixed_hz(60)
        .sim_system(spin)
        .run_headless(60, village);
    let mut houses = 0;
    world.query::<(&Transform, &Spin)>().for_each(|(t, _)| {
        houses += 1;
        // One second at half a radian a second.
        let (axis, angle) = t.rotation.to_axis_angle();
        assert!((angle - 0.5).abs() < 1e-3 && axis.y > 0.99, "{axis} {angle}");
    });
    assert_eq!(houses, 5);
}

// ANCHOR: sky
/// A misty morning: the sun low in the east, fog lying in the valleys.
fn misty_morning(ctx: &mut Ctx) {
    ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(7.5);
    ctx.scene3d.sky.haze = 2.0; // a paler sky, a bigger glow around the sun
    ctx.scene3d.sky.fog_density = 0.03; // thick near the ground
    ctx.scene3d.sky.fog_height = 0.0;
    ctx.scene3d.sky.fog_falloff = 10.0; // and thin ten meters up
}
// ANCHOR_END: sky

// ANCHOR: mesh_shadows
/// A meadow: thousands of grass tufts, too small for their shadows to show.
fn plant_meadow(ctx: &mut Ctx, tuft: Mesh) {
    ctx.gfx.set_mesh_shadows(tuft, false); // no shadow map work for grass
    for i in 0..1000 {
        let at = vec3((i % 40) as f32 * 0.5, 0.0, (i / 40) as f32 * 0.5);
        ctx.world.spawn((Transform::at(at), Model::new(tuft)));
    }
}
// ANCHOR_END: mesh_shadows

#[test]
fn grass_can_cast_no_shadow() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        let material = ctx.gfx.default_material();
        let tuft = ctx.gfx.create_mesh(&MeshData::cube(0.1), material);
        assert!(ctx.gfx.mesh_shadows(tuft), "meshes cast shadows at first");
        plant_meadow(ctx, tuft);
        assert!(!ctx.gfx.mesh_shadows(tuft));
        ctx.gfx.set_mesh_shadows(tuft, true);
        assert!(ctx.gfx.mesh_shadows(tuft));
    });
}

// ANCHOR: shadow_mesh
/// A detailed tree with a cheap shadow, a flat sign with none, and a stand-in
/// shadow for a row of fence pieces that cast none themselves.
fn yard(ctx: &mut Ctx, tree: Mesh, blob: Mesh, sign: Mesh, fence: Mesh, fence_shadow: Mesh) {
    ctx.world.spawn((
        Transform::default(),
        Model::new(tree),
        ponko::render::ShadowMesh(Some(blob)),
    ));
    ctx.world.spawn((
        Transform::at(vec3(4.0, 0.0, 0.0)),
        Model::new(sign),
        ponko::render::ShadowMesh(None),
    ));
    for i in 0..10 {
        let at = vec3(i as f32, 0.0, 6.0);
        ctx.world
            .spawn((Transform::at(at), Model::new(fence), ponko::render::ShadowMesh(None)));
    }
    // Only a shadow: no Model.
    ctx.world.spawn((
        Transform::at(vec3(0.0, 0.0, 6.0)),
        ponko::render::ShadowMesh(Some(fence_shadow)),
    ));
}
// ANCHOR_END: shadow_mesh

#[test]
fn shadow_meshes_round_trip_through_scenes() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        let material = ctx.gfx.default_material();
        let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        yard(ctx, mesh, mesh, mesh, mesh, mesh);
        let only_shadows = ctx
            .world
            .query::<&ponko::render::ShadowMesh>()
            .filter::<Without<Model>>()
            .iter()
            .count();
        assert_eq!(only_shadows, 1);
        let none = ctx
            .world
            .query::<&ponko::render::ShadowMesh>()
            .iter()
            .filter(|s| s.0.is_none())
            .count();
        assert_eq!(none, 11);
        // No shadow, as JSON and back.
        let sign = ctx.world.spawn((Transform::default(), ponko::render::ShadowMesh(None)));
        let json = ctx.world.entity_json(sign).unwrap();
        assert_eq!(json["ShadowMesh"], serde_json::json!({ "mesh": null }), "{json}");
        let other = ctx.world.spawn((Transform::default(),));
        ctx.world
            .set_json(other, "ShadowMesh", json["ShadowMesh"].clone())
            .unwrap();
        assert_eq!(
            ctx.world.get::<ponko::render::ShadowMesh>(other),
            Some(&ponko::render::ShadowMesh(None))
        );
        let unloaded = ctx
            .world
            .set_json(other, "ShadowMesh", serde_json::json!({ "mesh": "nope.glb" }))
            .unwrap_err();
        assert!(unloaded.to_string().contains("isn't loaded"), "{unloaded}");
    });
}

// ANCHOR: distance_fog
/// A big open world fading out far away, glowing toward the sun.
fn far_haze(ctx: &mut Ctx) {
    ctx.scene3d.sky.fog_start = 500.0; // clear up to half a kilometre
    ctx.scene3d.sky.fog_end = 1200.0; // and gone in the haze past 1.2 km
    ctx.scene3d.sky.fog_sun_glow = 0.06; // the haze glows toward the sun
    ctx.scene3d.sky.fog_sun_focus = 0.7; // most of all close around it
}
// ANCHOR_END: distance_fog

// ANCHOR: light_shafts
/// Beams of morning sun through the trees.
fn sunbeams(ctx: &mut Ctx) {
    ctx.scene3d.sky.fog_density = 0.03; // low mist to light up
    ctx.scene3d.sky.fog_sun_glow = 0.1; // glowing in the sun
    ctx.scene3d.sky.light_shafts = 1.0; // but not in the shade
    ctx.scene3d.lighting.shadows = true; // shafts follow the sun's shadows
    ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa; // and smooth out their grain
}
// ANCHOR_END: light_shafts

// ANCHOR: light_shadow_atlas
/// A street at night: a sharp shadow for the lamp beside the player,
/// rough ones for the street lights down the road.
fn street_lights(ctx: &mut Ctx) {
    ctx.scene3d.lighting.light_shadow_atlas = 8192; // room for more shadow views
    ctx.scene3d.lighting.light_shadow_views = 24; // as many as there can be
    let lamp = PointLight {
        color: Color::rgb(1.0, 0.8, 0.6),
        intensity: 40.0,
        range: 12.0,
        shadows: true,
        shadow_resolution: 2048, // always sharp
    };
    ctx.world.spawn((Transform::at(vec3(0.0, 3.0, 0.0)), lamp));
    for i in 0..18 {
        let x = -20.0 + 2.5 * i as f32;
        let at = Transform::at(vec3(x, 5.0, -30.0)).looking_at(vec3(x, 0.0, -29.9));
        // Sized by how big they look: smaller the farther they are.
        let street_light = SpotLight {
            intensity: 60.0,
            range: 8.0,
            shadows: true,
            ..Default::default()
        };
        ctx.world.spawn((at, street_light));
    }
}
// ANCHOR_END: light_shadow_atlas

#[test]
fn light_shadow_atlas_settings_stay_as_set() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        let l = ctx.scene3d.lighting;
        assert_eq!(
            (l.light_shadow_atlas, l.light_shadow_views, l.light_shadow_resolution),
            (4096, 12, 1024)
        );
        street_lights(ctx);
        assert_eq!(ctx.scene3d.lighting.light_shadow_atlas, 8192);
        let hinted: Vec<u32> = ctx
            .world
            .query::<&PointLight>()
            .iter()
            .map(|p| p.shadow_resolution)
            .collect();
        assert_eq!(hinted, [2048]);
        assert_eq!(ctx.world.query::<&SpotLight>().iter().count(), 18);
    });
}

#[test]
fn sky_settings_stay_as_set() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        assert_eq!(ctx.scene3d.sky, Sky::default());
        assert!(ctx.scene3d.sky.atmosphere);
        misty_morning(ctx);
        assert_eq!(ctx.scene3d.sky.fog_density, 0.03);
        assert_eq!(
            (ctx.scene3d.sky.fog_start, ctx.scene3d.sky.fog_end),
            (0.0, 0.0),
            "no distance fog by default"
        );
        far_haze(ctx);
        assert_eq!(ctx.scene3d.sky.fog_end, 1200.0);
        assert_eq!(ctx.scene3d.sky.fog_density, 0.03, "both kinds of fog at once");
        assert_eq!(ctx.scene3d.sky.light_shafts, 0.0, "no light shafts by default");
        sunbeams(ctx);
        assert_eq!(ctx.scene3d.sky.light_shafts, 1.0);
        let to_sun = -ctx.scene3d.lighting.sun_direction;
        assert!(to_sun.x > 0.0 && to_sun.y > 0.0, "morning: low in the east");
    });
}

// ANCHOR: post
/// A warm, glowing evening look.
fn golden_hour(ctx: &mut Ctx) {
    ctx.scene3d.post.exposure = 0.3; // a third of a stop brighter
    ctx.scene3d.post.bloom = 0.2; // highlights glow
    ctx.scene3d.post.temperature = 0.4; // warmer
    ctx.scene3d.post.saturation = 1.1;
    ctx.scene3d.post.vignette = 0.3; // darker corners
    ctx.scene3d.post.tonemap = Tonemap::AgX;
}
// ANCHOR_END: post

// ANCHOR: lens
/// The rush of speed: colors split at the edges, the middle pinches in.
fn speed_lens(ctx: &mut Ctx, speed: f32) {
    let rush = (speed / 40.0).clamp(0.0, 1.0);
    ctx.scene3d.post.chromatic_aberration = 0.8 * rush;
    ctx.scene3d.post.lens_distortion = -0.26 * rush;
    ctx.scene3d.post.lens_scale = 1.0 + 0.04 * rush; // hide the pulled-in edges
}
// ANCHOR_END: lens

#[test]
fn lens_settings_stay_as_set() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        speed_lens(ctx, 40.0);
        assert_eq!(ctx.scene3d.post.chromatic_aberration, 0.8);
        assert_eq!(ctx.scene3d.post.lens_distortion, -0.26);
        assert_eq!(ctx.scene3d.post.lens_scale, 1.04);
    });
}

#[test]
fn post_settings_stay_as_set() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        assert_eq!(ctx.scene3d.post, Post::default());
        golden_hour(ctx);
        assert_eq!(ctx.scene3d.post.tonemap, Tonemap::AgX);
        let (w, h, pixels) = Post::neutral_lut(16);
        ctx.scene3d.post.lut = Some(ctx.gfx.create_texture(w, h, &pixels));
        assert_eq!(ctx.scene3d.post.lut.map(|t| (t.width(), t.height())), Some((256, 16)));
    });
}

#[test]
#[should_panic(expected = "2 to 64 steps")]
fn lookup_tables_have_two_to_64_steps() {
    let _ = Post::neutral_lut(1);
}

// ANCHOR: shader
/// Grass that sways in the wind: the higher a vertex, the more it moves.
const WIND: &str = "
fn vertex(v: Vertex) -> Vertex {
    var out = v;
    let sway = sin(frame.time.x * 2.0 + v.position.x) * material.custom[0].x;
    out.position.x += sway * v.uv.y * v.uv.y;
    return out;
}";

fn meadow(ctx: &mut Ctx) {
    let wind = ctx.gfx.create_shader("wind", WIND);
    let grass = ctx.gfx.create_material(
        MaterialDesc::color(Color::hex(0x7FA83C))
            .with_shader(wind)
            .with_double_sided(true)
            .with_custom([vec4(0.2, 0.0, 0.0, 0.0), Vec4::ZERO, Vec4::ZERO, Vec4::ZERO]),
    );
    let blade = ctx.gfx.create_mesh(&MeshData::plane(0.5), grass);
    ctx.world.spawn((Transform::default(), Model::new(blade)));
}
// ANCHOR_END: shader

#[test]
fn shaders_and_their_materials_are_made() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        meadow(ctx);
        let water = ctx.gfx.load_shader("shaders/water.wgsl");
        assert_eq!(ctx.gfx.load_shader("shaders/water.wgsl"), water, "loaded once");
        let lake = ctx.gfx.create_material(
            MaterialDesc::default()
                .with_shader(water)
                .with_blend(Blend::Transparent),
        );
        ctx.gfx
            .update_material(lake, MaterialDesc::default().with_roughness(0.1));
    });
}

#[test]
fn transparent_shaders_can_read_the_scene_behind_them() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        let glass = ctx.gfx.create_shader(
            "glass",
            "fn surface(s: Surface) -> Surface {
                var out = s;
                let deep = scene_distance(s.screen) - distance(s.world, frame.camera.xyz);
                out.emissive = scene_color(s.screen + vec2<f32>(4.0, 0.0)) * exp(-deep);
                out.albedo = scene_position(s.screen) * 0.0 + s.albedo;
                return out;
            }",
        );
        ctx.gfx.create_material(
            MaterialDesc::default()
                .with_shader(glass)
                .with_blend(Blend::Transparent),
        );
    });
}

#[test]
#[should_panic(expected = "fix.wgsl")]
fn shader_errors_say_which_shader() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.gfx
            .create_shader("fix.wgsl", "fn surface(s: Surface) -> Surface { return 1.0; }");
    });
}

#[test]
#[should_panic(expected = "wasn't made by this game")]
fn materials_only_take_this_games_shaders() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let mut shader = None;
        Game::new("v", 1, 1).run_headless(0, |c| shader = Some(c.gfx.create_shader("s", "")));
        ctx.gfx.create_material(MaterialDesc::default().with_shader(shader));
    });
}

#[test]
fn loaded_materials_are_found_by_name_and_can_be_changed() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        assert!(ctx.gfx.loaded_materials("walls").is_empty(), "nothing loaded yet");
        let _ = ctx.gfx.load_mesh("models/house.glb");
        let walls = ctx.gfx.loaded_materials("walls");
        assert_eq!(walls.len(), 1);
        let roof = ctx.gfx.loaded_materials("roof");
        assert_eq!(roof.len(), 1);
        assert_ne!(walls[0].0, roof[0].0);
        // Loading the file again makes no new materials.
        let _ = ctx.gfx.load_mesh("models/house.glb");
        assert_eq!(ctx.gfx.loaded_materials("walls").len(), 1);
        // ANCHOR: loaded_materials
        // The house's walls, drawn with a shader of our own: keep what they
        // were made from (textures, colors) and change the shader.
        let weathered = ctx.gfx.create_shader(
            "weathered.wgsl",
            "fn surface(s: Surface) -> Surface { var o = s; o.albedo *= 0.8; o.roughness = 0.95; return o; }",
        );
        for (walls, desc) in ctx.gfx.loaded_materials("walls") {
            ctx.gfx.update_material(walls, desc.with_shader(weathered));
        }
        // ANCHOR_END: loaded_materials
        assert!(ctx.gfx.loaded_materials("no such material").is_empty());
    });
}

#[test]
fn loading_a_model_twice_gives_the_same_mesh() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let a = ctx.gfx.load_mesh("models/house.glb");
        let b = ctx.gfx.load_mesh("models/house.glb");
        assert_eq!(a, b);
        let white = ctx.gfx.default_material();
        let cube = ctx.gfx.create_mesh(&MeshData::cube(1.0), white);
        assert_ne!(a, cube);
    });
}

#[test]
#[should_panic(expected = "models/no-such-model.glb")]
fn a_missing_model_names_the_file() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let _ = ctx.gfx.load_mesh("models/no-such-model.glb");
    });
}

#[test]
#[should_panic(expected = "roughness and metallic go from 0 to 1")]
fn materials_check_their_numbers() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let _ = ctx.gfx.create_material(MaterialDesc::default().with_metallic(1.5));
    });
}

#[test]
#[should_panic(expected = "2 colors for 3 corners")]
fn vertex_colors_come_one_per_corner() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let tri = MeshData {
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            indices: vec![0, 1, 2],
            colors: vec![Color::RED, Color::WHITE],
            ..Default::default()
        };
        let white = ctx.gfx.default_material();
        let _ = ctx.gfx.create_mesh(&tri, white);
    });
}

#[test]
#[should_panic(expected = "3 per triangle")]
fn meshes_check_their_triangles() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let broken = MeshData {
            positions: vec![Vec3::ZERO; 3],
            indices: vec![0, 1],
            ..Default::default()
        };
        let white = ctx.gfx.default_material();
        let _ = ctx.gfx.create_mesh(&broken, white);
    });
}

// ANCHOR: terrain
/// Rolling hills: 3 x 3 tiles of 128 m, each from 65 x 65 heights 2 m
/// apart, sharing their edges, with a pond-sized hole in the middle tile.
/// Grass in the valleys, dirt higher up and rock on the tops.
fn hills(ctx: &mut Ctx) {
    let height = |x: f32, z: f32| (x * 0.02).sin() * 6.0 + (z * 0.03).cos() * 4.0;
    let ground = hill_ground(ctx, height);
    let (n, size) = (65, 128.0);
    for tz in -1..=1 {
        for tx in -1..=1 {
            let corner = vec3(tx as f32 * size, 0.0, tz as f32 * size);
            let heights: Vec<f32> = (0..n * n)
                .map(|i| {
                    let (x, z) = ((i % n) as f32 * 2.0 + corner.x, (i / n) as f32 * 2.0 + corner.z);
                    let hole = tx == 0 && tz == 0 && (vec2(x, z) - vec2(64.0, 64.0)).length() < 6.0;
                    if hole { f32::NAN } else { height(x, z) }
                })
                .collect();
            let tile = ctx.gfx.create_terrain_tile(&heights, size, ground);
            ctx.world.spawn((Transform::at(corner), tile));
        }
    }
    ctx.scene3d.camera.position = vec3(64.0, 30.0, 220.0);
    ctx.scene3d.camera.look_at(vec3(64.0, 0.0, 64.0));
}
// ANCHOR_END: terrain

// ANCHOR: ground_height
/// Trees standing on a 128 m tile: each at the ground's height under it,
/// none over a hole.
fn plant_trees(ctx: &mut Ctx, heights: &[f32], corner: Vec3, tree: Mesh) {
    let ground = Heightfield::new(heights, 128.0);
    for i in 0..20 {
        let (x, z) = ((i * 37 % 128) as f32, (i * 71 % 128) as f32);
        if let Some(y) = ground.height_at(x, z) {
            ctx.world
                .spawn((Transform::at(corner + vec3(x, y, z)), Model::new(tree)));
        }
    }
}
// ANCHOR_END: ground_height

#[test]
fn the_guide_trees_stand_on_the_ground() {
    let mut world = Game::new("t", 1, 1).run_headless(0, |ctx| {
        let material = ctx.gfx.default_material();
        let tree = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        // Flat at 3 m, with the first cell's corner cut out.
        let mut heights = vec![3.0; 65 * 65];
        heights[0] = f32::NAN;
        plant_trees(ctx, &heights, vec3(128.0, 0.0, 0.0), tree);
    });
    let trees: Vec<Vec3> = world.query::<&Transform>().iter().map(|t| t.position).collect();
    assert_eq!(trees.len(), 19, "the tree at the corner is over the hole");
    assert!(trees.iter().all(|p| (p.y - 3.0).abs() < 1e-4 && p.x >= 128.0));
}

// ANCHOR: terrain_material
/// Three ground layers, speckled so they don't look flat (in a game,
/// `ctx.gfx.load_texture_array` from image files), and one weight map over
/// all nine tiles from the heights: red grass, green dirt, blue rock.
fn hill_ground(ctx: &mut Ctx, height: impl Fn(f32, f32) -> f32) -> Material {
    let speckled = |base: [u8; 3]| -> Vec<u8> {
        (0..64 * 64u32)
            .flat_map(|i| {
                let shade = (i.wrapping_mul(2_654_435_761) >> 27) as u8; // 0..31
                [
                    base[0].saturating_add(shade),
                    base[1].saturating_add(shade),
                    base[2].saturating_add(shade),
                    255,
                ]
            })
            .collect()
    };
    let (grass, dirt, rock) = (
        speckled([70, 120, 50]),
        speckled([110, 85, 60]),
        speckled([120, 120, 115]),
    );
    let layers = ctx
        .gfx
        .create_texture_array(64, 64, &[&grass, &dirt, &rock], TextureKind::Color);
    let (cells, area) = (96, 384.0); // 4 m a pixel, from (-128, -128)
    let weights: Vec<u8> = (0..cells * cells)
        .flat_map(|i| {
            let x = (i % cells) as f32 * area / cells as f32 - 128.0;
            let z = (i / cells) as f32 * area / cells as f32 - 128.0;
            let up = ((height(x, z) + 10.0) / 20.0).clamp(0.0, 1.0); // 0 low, 1 high
            let rock = ((up - 0.7) * 4.0).clamp(0.0, 1.0);
            let dirt = (1.0 - (up - 0.5).abs() * 4.0).clamp(0.0, 1.0);
            let grass = 1.0 - rock.max(dirt);
            [grass, dirt, rock, 0.0].map(|w| (w * 255.0) as u8)
        })
        .collect();
    let weights = ctx
        .gfx
        .create_texture_array(cells, cells, &[&weights], TextureKind::Data);
    let desc = TerrainMaterialDesc::new(layers, weights, vec2(-128.0, -128.0), vec2(area, area))
        .with_layer_size([6.0, 4.0, 8.0, 4.0, 4.0, 4.0, 4.0, 4.0]);
    ctx.gfx.create_terrain_material(&desc)
}
// ANCHOR_END: terrain_material

#[test]
fn the_guide_hills_are_nine_tiles_of_four_details() {
    let mut world = Game::new("hills", 640, 480).run_headless(1, hills);
    let mut meshes = std::collections::HashSet::new();
    world.query::<&TerrainTile>().for_each(|tile| {
        assert_eq!(tile.lod_distance(), 128.0);
        meshes.extend(tile.lods());
    });
    assert_eq!(
        meshes.len(),
        9 * ponko::terrain::TerrainTile::LODS,
        "every tile and level has its own mesh"
    );
}

// ANCHOR: effect
/// Under water, light fades to deep blue with distance (and the sky above
/// is gone). `material.custom[0].x` is the water's height.
const UNDERWATER: &str = "
fn effect(p: Pixel) -> vec3<f32> {
    if frame.camera.y > material.custom[0].x {
        return p.color; // above the water: nothing changes
    }
    let deep = vec3<f32>(0.02, 0.12, 0.18);
    let fade = 1.0 - exp(-p.distance * 0.08);
    return mix(p.color * vec3<f32>(0.6, 0.9, 1.0), deep, fade);
}";

fn dive(ctx: &mut Ctx) {
    let underwater = ctx.gfx.create_effect("underwater.wgsl", UNDERWATER);
    let lake = ctx.gfx.create_material(MaterialDesc::default().with_custom([
        vec4(2.0, 0.0, 0.0, 0.0), // water at y = 2
        Vec4::ZERO,
        Vec4::ZERO,
        Vec4::ZERO,
    ]));
    ctx.scene3d
        .passes
        .effects
        .push(ScreenEffect::new(underwater).with_material(lake));
}
// ANCHOR_END: effect

// ANCHOR: mirror
/// A still pond: its color is the world mirrored in it, which a target
/// camera below the surface draws into `custom_texture0`, upside down.
const MIRROR: &str = "
fn surface(s: Surface) -> Surface {
    var out = s;
    let uv = s.screen * frame.screen.zw;
    out.emissive = textureSample(custom_texture0, material_sampler, vec2<f32>(uv.x, 1.0 - uv.y)).rgb * 0.9;
    out.albedo = vec3<f32>(0.0);
    return out;
}";

fn pond(ctx: &mut Ctx) {
    let size = ctx.window_size();
    let reflection = ctx.gfx.create_render_target(size.x as u32 / 2, size.y as u32 / 2);
    let mirror = ctx.gfx.create_shader("mirror", MIRROR);
    let water = ctx.gfx.create_material(
        MaterialDesc::default()
            .with_shader(mirror)
            .with_custom_textures([Some(reflection), None]),
    );
    let surface = ctx.gfx.create_mesh(&MeshData::plane(20.0), water);
    ctx.world.spawn((Transform::default(), Model::new(surface)));
    ctx.scene3d
        .passes
        .target_cameras
        .push(TargetCamera::reflection(ctx.scene3d.camera, reflection, 0.0));
}

/// Every frame: the mirrored camera follows the real one.
fn follow_reflections(ctx: &mut Ctx) {
    for t in &mut ctx.scene3d.passes.target_cameras {
        *t = TargetCamera::reflection(ctx.scene3d.camera, t.target, 0.0);
    }
}
// ANCHOR_END: mirror

// ANCHOR: compute
/// Ripples: each texel moves toward the average of its neighbours,
/// overshooting like water. Red is the height now, green the height a
/// frame ago; `material.custom[0]` drops a stone (uv x, uv y, strength).
const RIPPLES: &str = "
fn compute(c: Cell) -> vec4<f32> {
    let around = cell_at(c.pixel + vec2<i32>(1, 0)).x + cell_at(c.pixel - vec2<i32>(1, 0)).x
        + cell_at(c.pixel + vec2<i32>(0, 1)).x + cell_at(c.pixel - vec2<i32>(0, 1)).x;
    var height = (around * 0.5 - c.value.y) * 0.985;
    let drop = material.custom[0];
    if drop.z > 0.0 && distance(c.uv, drop.xy) * f32(c.size.x) < 3.0 {
        height -= drop.z;
    }
    return vec4<f32>(height, c.value.x, 0.0, 0.0);
}";

/// The simulated heights, for a water material to read, and the material
/// that drops stones into them.
fn ripples(ctx: &mut Ctx) -> (Texture, Material) {
    let heights = ctx.gfx.create_render_target(256, 256);
    let ripples = ctx.gfx.create_compute("ripples", RIPPLES);
    let stones = ctx.gfx.create_material(MaterialDesc::default());
    ctx.scene3d
        .passes
        .compute
        .push(ComputePass::new(ripples, heights).with_material(stones));
    (heights, stones)
}

/// Drops a stone at `uv` (0 to 1 across the water) this frame; `strength`
/// 0 drops none.
fn drop_stone(ctx: &mut Ctx, stones: Material, uv: Vec2, strength: f32) {
    let stone = uv.extend(strength).extend(0.0);
    ctx.gfx.update_material(
        stones,
        MaterialDesc::default().with_custom([stone, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO]),
    );
}
// ANCHOR_END: compute

#[test]
fn the_guide_ripples_run_on_a_render_target() {
    Game::new("ripples", 64, 64).run_headless(1, |ctx| {
        let (heights, stones) = ripples(ctx);
        drop_stone(ctx, stones, vec2(0.5, 0.5), 1.0);
        assert!(ctx.gfx.is_render_target(heights));
        assert_eq!(ctx.scene3d.passes.compute.len(), 1);
        assert_eq!(ctx.scene3d.passes.compute[0].material, Some(stones));
    });
}

#[test]
#[should_panic(expected = "ripples.wgsl")]
fn compute_errors_say_which_pass() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.gfx
            .create_compute("ripples.wgsl", "fn compute(c: Cell) -> f32 { return 1.0; }");
    });
}

#[test]
fn the_guide_pond_mirrors_the_camera() {
    Game::new("pond", 64, 64).run_headless(0, |ctx| {
        pond(ctx);
        ctx.scene3d.camera.position = vec3(3.0, 5.0, 1.0);
        follow_reflections(ctx);
        let below = ctx.scene3d.passes.target_cameras[0];
        assert!(ctx.gfx.is_render_target(below.target));
        assert_eq!(below.target.width(), 32);
        assert_eq!(below.camera.position, vec3(3.0, -5.0, 1.0));
        assert_eq!(below.clip_plane, Some(vec4(0.0, 1.0, 0.0, 0.0)));
    });
}

#[test]
#[should_panic(expected = "render target size must not be zero")]
fn render_targets_need_a_size() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.gfx.create_render_target(0, 16);
    });
}

#[test]
#[should_panic(expected = "not updated with pixels")]
fn render_targets_take_no_pixels() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let target = ctx.gfx.create_render_target(1, 1);
        ctx.gfx.update_texture(target, &[0; 4]);
    });
}

#[test]
fn the_guide_effect_is_added_once() {
    let mut effects = Vec::new();
    Game::new("dive", 64, 64).run_headless(2, |ctx| {
        dive(ctx);
        effects = ctx.scene3d.passes.effects.clone();
    });
    assert_eq!(effects.len(), 1);
    assert!(effects[0].material.is_some());
}

#[test]
fn effect_errors_say_which_effect() {
    let err = std::panic::catch_unwind(|| {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let _ = ctx
                .gfx
                .create_effect("broken.wgsl", "fn effect(p: Pixel) -> vec3<f32> { return p; }");
        });
    })
    .unwrap_err();
    let message = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("broken.wgsl"), "{message}");
}

#[test]
fn effects_read_every_pixel_field() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let all = ctx.gfx.create_effect(
            "all.wgsl",
            "fn effect(p: Pixel) -> vec3<f32> {
                let far = select(p.distance, 0.0, p.sky);
                let near = scene_color(p.screen + vec2<f32>(1.0, 0.0));
                return p.color + near * 0.1 + p.world * 0.0 + vec3<f32>(p.uv, far * material.custom[0].x);
            }",
        );
        assert_ne!(
            all,
            ctx.gfx
                .create_effect("other.wgsl", "fn effect(p: Pixel) -> vec3<f32> { return p.color; }")
        );
    });
}

#[test]
fn texture_arrays_hold_one_image_per_layer() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let red = [255, 0, 0, 255].repeat(4);
        let blue = [0, 0, 255, 255].repeat(4);
        let tints = ctx.gfx.create_texture_array(2, 2, &[&red, &blue], TextureKind::Color);
        assert_eq!((tints.width(), tints.height(), tints.layers()), (2, 2, 2));
        let wind = ctx.gfx.create_shader(
            "layers.wgsl",
            "fn surface(s: Surface) -> Surface { var o = s; o.albedo = textureSample(custom_array0, material_sampler, s.uv, 1).rgb; return o; }",
        );
        let mut custom_arrays = [None; 3];
        custom_arrays[0] = Some(tints);
        let _ = ctx.gfx.create_material(MaterialDesc::default().with_shader(wind).with_custom_arrays(custom_arrays));
    });
}

#[test]
#[should_panic(expected = "layer 1 of a 2 x 2 texture array needs 16 bytes")]
fn texture_array_layers_are_all_one_size() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let _ = ctx
            .gfx
            .create_texture_array(2, 2, &[&[0; 16], &[0; 12]], TextureKind::Data);
    });
}

#[test]
fn terrain_materials_check_their_layers() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let image = [128u8; 16];
        let five = ctx.gfx.create_texture_array(2, 2, &[&image[..]; 5], TextureKind::Color);
        let one_weight = ctx.gfx.create_texture_array(2, 2, &[&image], TextureKind::Data);
        let two_weights = ctx.gfx.create_texture_array(2, 2, &[&image[..]; 2], TextureKind::Data);
        let nine = ctx.gfx.create_texture_array(2, 2, &[&image[..]; 9], TextureKind::Color);
        let desc = |layers, weights| TerrainMaterialDesc::new(layers, weights, Vec2::ZERO, vec2(10.0, 10.0));
        let err = |ctx: &mut Ctx, d: TerrainMaterialDesc| {
            let e = ctx.gfx.try_create_terrain_material(&d).unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Invalid);
            e.message().to_string()
        };
        assert!(err(ctx, desc(five, one_weight)).contains("need 2 weight images"));
        assert!(err(ctx, desc(nine, two_weights)).contains("at most 8 layers"));
        let mut no_size = desc(five, two_weights);
        no_size.weights_size = Vec2::ZERO;
        assert!(err(ctx, no_size).contains("size must be above 0"));
        let few_normals = desc(five, two_weights).with_normals(one_weight);
        assert!(err(ctx, few_normals).contains("need 5 normal maps"));
        // Two terrain materials share the one built-in shader.
        let a = ctx.gfx.create_terrain_material(&desc(five, two_weights));
        let b = ctx.gfx.create_terrain_material(&desc(five, two_weights));
        assert_ne!(a, b);
    });
}

#[test]
#[should_panic(expected = "size must be above 0")]
fn terrain_tiles_need_a_size() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let white = ctx.gfx.default_material();
        let _ = ctx.gfx.create_terrain_tile(&[0.0; 4], 0.0, white);
    });
}

#[test]
fn transforms_are_reflected_as_json() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let e = ctx.world.spawn((Transform::at(vec3(1.0, 2.0, 3.0)).scaled(2.0),));
        let json = ctx.world.get_json(e, "Transform3D").unwrap();
        assert_eq!(json["position"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(json["scale"], serde_json::json!([2.0, 2.0, 2.0]));
        ctx.world
            .set_json(e, "Transform3D", serde_json::json!({"position": [0.0, 5.0, 0.0]}))
            .unwrap();
        assert_eq!(ctx.world.get::<Transform>(e).unwrap().position, vec3(0.0, 5.0, 0.0));
    });
}

// ANCHOR: streaming
/// Ground that loads around the camera: tiles 128 m wide within 600 m,
/// built on worker threads, freed past 800 m, and coarse far ground
/// beyond them.
struct Ground {
    cells: ponko::stream::CellStreamer,
    tiles: std::collections::HashMap<(i32, i32), Tile>,
    material: Material,
    far: FarTerrain,
}

enum Tile {
    Building(ponko::stream::Job<TerrainTileData>),
    Live(Entity, TerrainTile),
}

fn hill(x: f32, z: f32) -> f32 {
    (x * 0.01).sin() * (z * 0.013).cos() * 20.0
}

impl Ground {
    fn new(material: Material) -> Self {
        // The far ground: 8 km around the start, a height every 32 m.
        let n = 257;
        let heights: Vec<f32> = (0..n * n)
            .map(|i| hill(-4096.0 + (i % n) as f32 * 32.0, -4096.0 + (i / n) as f32 * 32.0))
            .collect();
        Self {
            cells: ponko::stream::CellStreamer::new(128.0, 600.0, 800.0),
            tiles: Default::default(),
            material,
            far: FarTerrain::new(&heights, n, vec2(-4096.0, -4096.0), 32.0, 128.0, material),
        }
    }

    /// Every frame: start up to 4 tiles, drop the far ones, and put up to
    /// 2 finished ones in the world.
    fn update(&mut self, ctx: &mut Ctx) {
        self.cells.update(ctx.scene3d.camera.position, 4);
        for &cell in self.cells.to_load() {
            let corner = self.cells.corner(cell);
            let job = ponko::stream::Job::spawn(move || {
                // 65 x 65 heights, 2 m apart.
                let heights: Vec<f32> = (0..65 * 65)
                    .map(|i| hill(corner.x + (i % 65) as f32 * 2.0, corner.z + (i / 65) as f32 * 2.0))
                    .collect();
                TerrainTileData::new(&heights, 128.0)
            });
            self.tiles.insert(cell, Tile::Building(job));
        }
        for cell in self.cells.to_drop() {
            if let Some(Tile::Live(entity, tile)) = self.tiles.remove(cell) {
                ctx.world.despawn(entity);
                ctx.free(tile);
                self.far.uncut(*cell);
            }
        }
        let mut uploads = 0;
        for (&cell, tile) in &mut self.tiles {
            if let Tile::Building(job) = tile
                && uploads < 2
                && let Some(data) = job.take()
            {
                let made = ctx.gfx.upload_terrain_tile(&data, self.material);
                let entity = ctx.world.spawn((Transform::at(self.cells.corner(cell)), made));
                *tile = Tile::Live(entity, made);
                // Shown now: cut the far ground away under it.
                self.far.cut(cell);
                uploads += 1;
            }
        }
        self.far.update(ctx);
    }
}
// ANCHOR_END: streaming

#[test]
fn the_guide_ground_streams_in_and_out() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let material = ctx.gfx.default_material();
        let mut ground = Ground::new(material);
        let live = |g: &Ground| g.tiles.values().filter(|t| matches!(t, Tile::Live(..))).count();
        // Until everything around the start is in.
        for _ in 0..10_000 {
            ground.update(ctx);
            if ground.tiles.len() == ground.cells.loaded_count()
                && live(&ground) == ground.tiles.len()
                && ground.cells.to_load().is_empty()
            {
                break;
            }
            std::thread::yield_now();
        }
        let first = live(&ground);
        assert!(first > 20, "{first} tiles");
        assert!(
            ground.cells.loaded().all(|c| ground.far.is_cut(c)),
            "far ground cut under every tile"
        );
        assert_eq!(ground.far.mesh_count(), 16);
        // Far away: the old tiles are freed.
        ctx.scene3d.camera.position = vec3(10_000.0, 0.0, 0.0);
        ground.update(ctx);
        assert_eq!(ground.cells.to_drop().len(), first);
        assert!(ground.tiles.keys().all(|&(x, _)| x > 50));
        assert!(!ground.far.is_cut((0, 0)), "filled in again behind");
    });
}

#[test]
fn models_of_loaded_meshes_save_and_load_by_file_name() {
    let temp = TempAsset::new("three-scene.json");
    let file = temp.name();
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let house = ctx.gfx.load_mesh("models/house.glb");
        let red = Color::rgb(1.0, 0.0, 0.0);
        let e = ctx
            .world
            .spawn((Transform::at(vec3(1.0, 2.0, 3.0)), Model::new(house).with_color(red)));
        assert_eq!(ctx.world.get_json(e, "Model").unwrap()["mesh"], "models/house.glb");
        let white = ctx.gfx.default_material();
        let cube = ctx.gfx.create_mesh(&MeshData::cube(1.0), white);
        ctx.world.spawn((Transform::default(), Model::new(cube)));
        ctx.assets.save_scene(&ctx.world, file).unwrap();
    });
    let result = std::panic::catch_unwind(|| {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            ctx.load_scene(file).unwrap();
            let house = ctx.gfx.load_mesh("models/house.glb");
            let models: Vec<(Transform, Model)> = ctx
                .world
                .query::<(&Transform, &Model)>()
                .iter()
                .map(|(t, m)| (*t, *m))
                .collect();
            assert_eq!(models.len(), 1, "the made-in-code cube has no file to name");
            assert_eq!(models[0].1.mesh, house, "loaded with the scene");
            assert_eq!(models[0].1.color, Color::rgb(1.0, 0.0, 0.0));
            assert_eq!(models[0].0.position, vec3(1.0, 2.0, 3.0));
        });
    });
    drop(temp);
    result.unwrap();
}

// ANCHOR: model_material
/// One uploaded ball in three looks, and a house with only its roof changed.
fn recolored(ctx: &mut Ctx, house: Mesh, gold: Material, rust: Material) {
    let ball = ctx.gfx.create_plain_mesh(&MeshData::sphere(0.5, 32));
    let at = |x: f32| Transform::at(vec3(x, 0.0, 0.0));
    ctx.world.spawn((at(0.0), Model::new(ball))); // plain white
    ctx.world.spawn((at(1.0), Model::new(ball), ModelMaterial(gold)));
    ctx.world.spawn((at(2.0), Model::new(ball), ModelMaterial(rust)));
    // Part 1 only; the other parts keep the materials from the file.
    ctx.world.spawn((
        at(6.0),
        Model::new(house),
        ponko::render::ModelMaterials::new(&[(1, rust)]),
    ));
}
// ANCHOR_END: model_material

#[test]
fn one_plain_mesh_is_drawn_in_each_models_material() {
    Game::new("t", 1, 1).run_headless(2, |ctx| {
        let gold = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0xFFC83D)));
        let rust = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x8A4B2A)));
        let house = ctx.gfx.load_mesh("models/house.glb");
        ctx.gfx.default_material();
        let (before, materials) = (ctx.live_counts().meshes, ctx.live_counts().materials);
        recolored(ctx, house, gold, rust);
        // Three balls and three looks from one uploaded mesh, which has the
        // default material: no new one is made for it.
        assert_eq!(ctx.live_counts().meshes, before + 1);
        assert_eq!(ctx.live_counts().materials, materials);
        assert_eq!(ctx.world.query::<(&Model, &ModelMaterial)>().iter().count(), 2);
        let listed: Vec<_> = ctx
            .world
            .query::<&ponko::render::ModelMaterials>()
            .iter()
            .flat_map(|m| m.parts().collect::<Vec<_>>())
            .collect();
        assert_eq!(listed, vec![(1, rust)]);
        // Bad data is an error before anything is made.
        let mut broken = MeshData::cube(1.0);
        broken.indices.pop();
        let e = ctx.gfx.try_create_plain_mesh(&broken).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Invalid);
        assert!(e.to_string().contains("create_plain_mesh: 35 indices"), "{e}");
        assert_eq!(ctx.live_counts().meshes, before + 1);
    });
}

#[test]
fn model_material_overrides_save_and_load_by_file_and_material_name() {
    let temp = TempAsset::new("material-scene.json");
    let file = temp.name();
    let house_file = "models/house.glb"; // part 0 "walls", part 1 "roof"
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let house = ctx.gfx.load_mesh(house_file);
        let (walls, roof) = (
            ctx.gfx.loaded_materials("walls")[0].0,
            ctx.gfx.loaded_materials("roof")[0].0,
        );
        let at = |x: f32| Transform::at(vec3(x, 0.0, 0.0));
        let all_roof = ctx.world.spawn((at(1.0), Model::new(house), ModelMaterial(roof)));
        let swapped = ctx.world.spawn((
            at(2.0),
            Model::new(house),
            ponko::render::ModelMaterials::new(&[(1, walls), (0, roof)]),
        ));
        // A material made in code has no file to name: not saved.
        let paint = ctx.gfx.create_material(MaterialDesc::color(Color::rgb(1.0, 0.0, 0.0)));
        let painted = ctx.world.spawn((
            at(3.0),
            Model::new(house),
            ModelMaterial(paint),
            ponko::render::ModelMaterials::new(&[(0, roof), (1, paint)]),
        ));
        assert_eq!(
            ctx.world.get_json(all_roof, "ModelMaterial").unwrap(),
            serde_json::json!({ "mesh": house_file, "material": "roof" })
        );
        assert_eq!(
            ctx.world.get_json(swapped, "ModelMaterials").unwrap(),
            serde_json::json!({ "parts": [
                { "part": 0, "mesh": house_file, "material": "roof" },
                { "part": 1, "mesh": house_file, "material": "walls" },
            ] })
        );
        assert!(ctx.world.get_json(painted, "ModelMaterial").is_err());
        assert!(ctx.world.get_json(painted, "ModelMaterials").is_err());
        ctx.assets.save_scene(&ctx.world, file).unwrap();
    });
    let result = std::panic::catch_unwind(|| {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            // The scene names the file, so loading it loads the materials.
            ctx.load_scene(file).unwrap();
            let (walls, roof) = (
                ctx.gfx.loaded_materials("walls")[0].0,
                ctx.gfx.loaded_materials("roof")[0].0,
            );
            let mut got: Vec<(f32, Option<ModelMaterial>, Option<ponko::render::ModelMaterials>)> = ctx
                .world
                .query::<(
                    &Transform,
                    &Model,
                    Option<&ModelMaterial>,
                    Option<&ponko::render::ModelMaterials>,
                )>()
                .iter()
                .map(|(t, _, m, parts)| (t.position.x, m.copied(), parts.cloned()))
                .collect();
            got.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert_eq!(
                got,
                vec![
                    (1.0, Some(ModelMaterial(roof)), None),
                    (
                        2.0,
                        None,
                        Some(ponko::render::ModelMaterials::new(&[(0, roof), (1, walls)]))
                    ),
                    (3.0, None, None),
                ]
            );

            // By number in the file, and the errors name what's wrong.
            let e = ctx.world.spawn((Transform::default(),));
            let by_number = serde_json::json!({ "mesh": house_file, "material": 0 });
            ctx.world.set_json(e, "ModelMaterial", by_number).unwrap();
            assert_eq!(ctx.world.get::<ModelMaterial>(e), Some(&ModelMaterial(walls)));
            let unknown = serde_json::json!({ "mesh": house_file, "material": "door" });
            let err = ctx.world.set_json(e, "ModelMaterial", unknown).unwrap_err().to_string();
            assert!(
                err.contains("has no material \"door\"") && err.contains("\"walls\", \"roof\""),
                "{err}"
            );
            let unloaded = serde_json::json!({ "parts": [{ "part": 0, "mesh": "nope.glb", "material": 0 }] });
            let err = ctx
                .world
                .set_json(e, "ModelMaterials", unloaded)
                .unwrap_err()
                .to_string();
            assert!(err.contains("ctx.gfx.load_mesh(\"nope.glb\")"), "{err}");

            // A freed material is no longer named.
            ctx.free(roof);
            let again = serde_json::json!({ "mesh": house_file, "material": "roof" });
            assert!(ctx.world.set_json(e, "ModelMaterial", again).is_err());
        });
    });
    drop(temp);
    result.unwrap();
}

#[test]
fn models_name_meshes_that_must_be_files() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let e = ctx.world.spawn((Transform::default(),));
        let err = ctx
            .world
            .set_json(e, "Model", serde_json::json!({ "color": Color::WHITE }));
        assert!(err.unwrap_err().to_string().contains("\"mesh\""));
        let err = ctx
            .world
            .set_json(e, "Model", serde_json::json!({ "mesh": "nope.glb" }));
        assert!(err.unwrap_err().to_string().contains("ctx.gfx.load_mesh(\"nope.glb\")"));
    });
}

#[test]
fn occlusion_culling_is_off_until_turned_on() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        assert!(!ctx.scene3d.camera.occlusion_culling);
        // ANCHOR: occlusion
        // A town or a forest: skip what houses, hills and trunks hide.
        ctx.scene3d.camera.occlusion_culling = true;
        // ANCHOR_END: occlusion
        // Headless, there's no depth to cull with: everything in view is drawn.
        let white = ctx.gfx.default_material();
        let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), white);
        ctx.world.spawn((Transform::default(), Model::new(mesh)));
        ctx.scene3d.camera.position = vec3(0.0, 0.0, 5.0);
        ctx.scene3d.camera.look_at(Vec3::ZERO);
        assert_eq!(ctx.perf.render_stats().occluded, 0);
    });
}

#[test]
fn a_three_color_ambient_is_set_on_the_flat_sky() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        assert_eq!(ctx.scene3d.sky.equator_color, None);
        // ANCHOR: trilight
        // A flat sky (no atmosphere): a blue sky, a green-grey horizon and brown ground.
        ctx.scene3d.sky.atmosphere = false;
        ctx.scene3d.sky.sky_color = Color::hex(0x8FB3D9);
        ctx.scene3d.sky.equator_color = Some(Color::hex(0x8C9A86));
        ctx.scene3d.sky.ground_color = Color::hex(0x4D4135);
        // ANCHOR_END: trilight
        assert!(ctx.scene3d.sky.equator_color.is_some());
    });
}

#[test]
fn lod_groups_pick_levels_by_screen_height() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let white = ctx.gfx.default_material();
        let high = ctx.gfx.create_mesh(&MeshData::sphere(1.0, 32), white);
        let low = ctx.gfx.create_mesh(&MeshData::cube(2.0), white);
        // ANCHOR: lod
        // A 2 m object: the sphere until it's 25% of the screen's height,
        // the cube down to 3%, then nothing. The sphere crossfades into the
        // cube over the top quarter of its range.
        let group = LodGroup::new(
            2.0,
            vec![
                LodLevel::new(0.25, vec![LodRenderer::new(high)]).with_fade(0.25),
                LodLevel::new(0.03, vec![LodRenderer::new(low)]),
            ],
        );
        let e = ctx.world.spawn((Transform::default(), group));
        // Lower quality: drop detail at 70% of the distance.
        ctx.scene3d.lod.bias = 0.7;
        // ANCHOR_END: lod
        let group = ctx.world.get::<LodGroup>(e).unwrap().clone();
        let at = |ctx: &mut Ctx, d: f32| {
            ctx.scene3d.camera.position = vec3(0.0, 0.0, d);
            ctx.scene3d.camera.look_at(Vec3::ZERO);
            group.pick_for(&Transform::default(), &ctx.scene3d.camera, &ctx.scene3d.lod)
        };
        ctx.scene3d.lod.bias = 1.0;
        assert_eq!(at(ctx, 2.0), LodPick::Level(0));
        assert_eq!(at(ctx, 10.0), LodPick::Level(1));
        assert_eq!(at(ctx, 100.0), LodPick::Culled);
        assert!(matches!(
            at(ctx, 6.4),
            LodPick::Fade {
                from: 0,
                to: Some(1),
                ..
            }
        ));
        // A distance to the screen height and back.
        let fov = ctx.scene3d.camera.fov_y;
        let d = LodGroup::distance_at(2.0, 0.25, fov);
        assert!((LodGroup::screen_height_at(2.0, d, fov) - 0.25).abs() < 1e-5);
        ctx.scene3d.lod.bias = 0.7;
        assert_eq!(at(ctx, 5.0), LodPick::Level(1), "the bias drops detail sooner");
    });
}

#[test]
fn lod_groups_of_loaded_meshes_save_and_load_by_file_name() {
    let temp = TempAsset::new("lod-scene.json");
    let file = temp.name();
    let group = |house: Mesh| {
        LodGroup::new(
            8.0,
            vec![
                LodLevel::new(0.3, vec![LodRenderer::new(house).with_color(Color::rgb(1.0, 0.0, 0.0))]).with_fade(0.2),
                LodLevel::new(
                    0.05,
                    vec![LodRenderer::new(house).with_offset(Transform::at(vec3(0.0, 1.0, 0.0)))],
                )
                .with_shadow(LodShadow::Casters(vec![])),
            ],
        )
        .with_center(vec3(0.0, 4.0, 0.0))
    };
    Game::new("t", 1, 1).run_headless(0, move |ctx| {
        let house = ctx.gfx.load_mesh("models/house.glb");
        ctx.world.spawn((Transform::at(vec3(1.0, 2.0, 3.0)), group(house)));
        // A group with a mesh made in code has no file to name: not saved.
        let white = ctx.gfx.default_material();
        let cube = ctx.gfx.create_mesh(&MeshData::cube(1.0), white);
        ctx.world.spawn((
            Transform::default(),
            LodGroup::new(1.0, vec![LodLevel::new(0.1, vec![LodRenderer::new(cube)])]),
        ));
        ctx.assets.save_scene(&ctx.world, file).unwrap();
    });
    let result = std::panic::catch_unwind(move || {
        Game::new("t", 1, 1).run_headless(0, move |ctx| {
            ctx.load_scene(file).unwrap();
            let house = ctx.gfx.load_mesh("models/house.glb");
            let groups: Vec<LodGroup> = ctx.world.query::<&LodGroup>().iter().cloned().collect();
            assert_eq!(groups, vec![group(house)], "loaded with the scene, its mesh with it");
        });
    });
    drop(temp);
    result.unwrap();
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let e = ctx.world.spawn((Transform::default(),));
        let level = serde_json::json!({ "levels": [{ "screen_height": 0.1, "renderers": [{ "mesh": "nope.glb" }] }] });
        let err = ctx.world.set_json(e, "LodGroup", level);
        assert!(err.unwrap_err().to_string().contains("ctx.gfx.load_mesh(\"nope.glb\")"));
    });
}

#[test]
fn temporal_antialiasing_is_a_setting_and_can_forget_its_history() {
    Game::new("taa settings", 64, 64).run_headless(1, |ctx| {
        assert_eq!(ctx.scene3d.post.taa_sharpness, 0.25);
        // ANCHOR: taa
        ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa;
        ctx.scene3d.post.taa_sharpness = 0.3; // it softens the image a little
        // After teleporting the camera (or an object), forget the old view:
        ctx.scene3d.camera.position = vec3(400.0, 3.0, 0.0);
        ctx.gfx.reset_temporal_history();
        // ANCHOR_END: taa
        assert_eq!(ctx.scene3d.post.anti_aliasing, AntiAliasing::Taa);
    });
}

#[test]
fn one_teleported_model_drops_only_its_own_motion() {
    Game::new("reset motion", 64, 64).run_headless(1, |ctx| {
        ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa;
        let hero = ctx.world.spawn((Transform::default(),));
        // ANCHOR: reset_motion
        // Teleport one object: only its motion is dropped, the rest of the
        // picture keeps its temporal history.
        ctx.world.get_mut::<Transform>(hero).unwrap().position = vec3(100.0, 0.0, 0.0);
        ctx.gfx.reset_motion(hero);
        // ANCHOR_END: reset_motion
        // Many in one frame are fine: past a limit the whole history resets.
        for _ in 0..10_000 {
            ctx.gfx.reset_motion(hero);
        }
    });
}

#[test]
fn ao_and_aa_settings_preserve_existing_defaults() {
    Game::new("post settings", 64, 64).run_headless(0, |ctx| {
        assert_eq!(ctx.scene3d.post.anti_aliasing, AntiAliasing::None);
        assert!(!ctx.scene3d.post.ambient_occlusion.enabled);
        assert!(!ctx.scene3d.post.ao_half_resolution && !ctx.scene3d.post.ao_temporal);
        // ANCHOR: ao_aa
        ctx.scene3d.post.ao_half_resolution = true;
        ctx.scene3d.post.ao_temporal = true;
        ctx.scene3d.post.ambient_occlusion = AmbientOcclusion {
            enabled: true,
            radius: 1.5,
            intensity: 1.0,
            slices: 3,
            steps: 6,
        };
        ctx.scene3d.post.contact_shadows = 0.3; // small, sharp sun shadows under feet and props
        ctx.scene3d.post.anti_aliasing = AntiAliasing::Smaa;
        // ANCHOR_END: ao_aa
        assert_eq!(ctx.scene3d.post.ambient_occlusion.radius, 1.5);
        assert_eq!(ctx.scene3d.post.contact_shadows, 0.3);
    });
}

#[test]
fn screen_space_reflections_are_off_until_turned_on() {
    Game::new("ssr settings", 64, 64).run_headless(0, |ctx| {
        assert_eq!((ctx.scene3d.post.ssr, ctx.scene3d.post.ssr_max_distance), (0.0, 30.0));
        // ANCHOR: ssr
        // Wet streets and polished floors reflect what's on screen.
        ctx.scene3d.post.ssr = 1.0;
        ctx.scene3d.post.ssr_max_distance = 40.0; // world units along the reflected ray
        ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa; // smooths their grain
        // Glossy surfaces blur it: full strength up to roughness 0.4, gone by 0.7.
        let wet = MaterialDesc::color(Color::rgb(0.2, 0.2, 0.22)).with_roughness(0.1);
        // ANCHOR_END: ssr
        assert_eq!(
            (ctx.scene3d.post.ssr, ctx.scene3d.post.ssr_max_distance, wet.roughness),
            (1.0, 40.0, 0.1)
        );
        ctx.scene3d.set_quality(GraphicsQuality::Medium);
        assert_eq!(ctx.scene3d.post.ssr, 0.0, "medium turns them off");
        ctx.scene3d.set_quality(GraphicsQuality::High);
        assert_eq!(ctx.scene3d.post.ssr, 1.0, "high turns them on");
    });
}

#[test]
fn global_illumination_is_off_until_turned_on() {
    Game::new("gi settings", 64, 64).run_headless(0, |ctx| {
        assert!(!ctx.scene3d.lighting.gi.enabled);
        // ANCHOR: gi
        // Sunlight bounces off walls and floors onto other surfaces.
        ctx.scene3d.lighting.gi = GlobalIllumination {
            enabled: true,
            extent: vec3(48.0, 16.0, 48.0), // the probe grid around the camera
            spacing: 2.0,                   // a probe every 2 world units
            probes_per_frame: 4,            // how many look around each frame
            intensity: 1.0,
        };
        // ANCHOR_END: gi
        assert_eq!(ctx.scene3d.lighting.gi.probe_counts(), [25, 9, 25]);
        ctx.scene3d.set_quality(GraphicsQuality::High);
        assert!(!ctx.scene3d.lighting.gi.enabled, "high turns it off");
        ctx.scene3d.set_quality(GraphicsQuality::Ultra);
        assert!(ctx.scene3d.lighting.gi.enabled, "ultra turns it on");
        assert_eq!(
            ctx.scene3d.lighting.gi.extent,
            vec3(48.0, 16.0, 48.0),
            "and keeps the rest"
        );
    });
}

#[test]
fn quality_presets_switch_at_run_time() {
    Game::new("quality", 64, 64).run_headless(0, |ctx| {
        // ANCHOR: quality
        // An options menu: pick a preset ...
        ctx.scene3d.set_quality(GraphicsQuality::Medium);
        // ... then change single settings on top of it: they are the
        // ordinary ones in ctx.scene3d.post and ctx.scene3d.lighting.
        ctx.scene3d.post.render_scale = 0.8; // draw the 3D scene at 80% and scale it up
        ctx.scene3d.post.sharpen = 0.3;
        ctx.scene3d.lighting.shadow_resolution = 4096;
        ctx.scene3d.post.ambient_occlusion.radius = 2.0;
        let preset = ctx.scene3d.quality(); // None: custom
        // ANCHOR_END: quality
        assert_eq!(preset, None);
        assert_eq!(ctx.scene3d.post.render_scale, 0.8);
        assert_eq!(ctx.scene3d.lighting.shadow_resolution, 4096);
        assert_eq!(ctx.scene3d.post.render_size((1000, 500)), (800, 400));
    });
}

/// The guide's GPU tier example: the preset for the GPU, and what a tier
/// lacks without such a GPU.
#[test]
fn quality_follows_the_gpu_tier() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        // ANCHOR: gpu_tiers
        // At startup: the preset for this GPU's tier, fitted to what it has.
        match ctx.gfx.gpu_capabilities() {
            Some(caps) => ctx.scene3d.set_recommended_quality(caps),
            None => ctx.scene3d.set_recommended_quality(&GpuTier::Desktop.report()), // no GPU opened (headless)
        }
        // What a weaker GPU lacks, with no such GPU at hand: the report of
        // the tier's floor, each missing feature with its fallback and reason.
        let baseline = GpuTier::Baseline.report();
        for f in baseline.features.iter().filter(|f| !f.available) {
            println!(
                "{:?}: {} ({})",
                f.feature,
                f.fallback.as_deref().unwrap_or("-"),
                f.reason.as_deref().unwrap_or("-")
            );
        }
        // ANCHOR_END: gpu_tiers
        assert_eq!(ctx.scene3d.quality(), Some(GraphicsQuality::High));
        assert!(!baseline.available(GpuFeature::Decals));
        assert_eq!(
            GpuTier::Baseline.unavailable_features(),
            [
                GpuFeature::Timestamps,
                GpuFeature::GpuSkinning,
                GpuFeature::GlobalIllumination,
                GpuFeature::Decals
            ]
        );
    });
}

/// The scene format the Unity importer writes for a LOD group loads.
#[test]
fn lod_groups_in_the_importers_json_shape_load() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.gfx.load_mesh("models/house.glb");
        let part = |y: f32| {
            serde_json::json!({
                "mesh": "models/house.glb",
                "offset": { "position": [0.0, y, -1.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
            })
        };
        let json = serde_json::json!({
            "size": 4.0,
            "center": [0.0, 1.0, -2.0],
            "levels": [
                { "screen_height": 0.6, "fade_width": 0.25, "renderers": [part(1.0)], "shadow": "Same" },
                { "screen_height": 0.3, "fade_width": 0.0, "renderers": [part(0.0)], "shadow": { "Casters": [part(2.0)] } },
                { "screen_height": 0.1, "fade_width": 0.5, "renderers": [], "shadow": { "Casters": [] } },
            ],
        });
        let e = ctx.world.spawn((Transform::default(),));
        ctx.world.set_json(e, "LodGroup", json).unwrap();
        let group = ctx.world.get::<LodGroup>(e).unwrap();
        assert_eq!(group.levels.len(), 3);
        assert_eq!(group.levels[0].renderers[0].offset.position, vec3(0.0, 1.0, -1.0));
        assert!(matches!(&group.levels[1].shadow, LodShadow::Casters(c) if c.len() == 1));
        assert_eq!(group.center, vec3(0.0, 1.0, -2.0));
    });
}

#[test]
fn lod_hysteresis_is_a_scene_component() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let e = ctx.world.spawn((Transform::default(),));
        ctx.world.set_json(e, "LodHysteresis", serde_json::json!(0.1)).unwrap();
        assert_eq!(ctx.world.get::<LodHysteresis>(e), Some(&LodHysteresis(0.1)));
    });
}

// ANCHOR: gpu_buffers
fn gpu_particles(ctx: &mut Ctx) -> GpuBuffer {
    let items = ctx.gfx.create_gpu_buffer(128, &[]);
    let update = ctx
        .gfx
        .create_compute("particles", "fn update(i: u32) { items[i].y += frame.time.y; }");
    ctx.scene3d.passes.compute.push(ComputePass::for_buffer(update, items));
    let shader = ctx.gfx.create_shader(
        "particles",
        "fn vertex(v: Vertex) -> Vertex { var o = v; o.position += items[v.instance_index].xyz; return o; }",
    );
    let material = ctx.gfx.create_material(MaterialDesc::default().with_shader(shader));
    let mesh = ctx.gfx.create_mesh(&MeshData::cube(0.05), material);
    ctx.gfx.set_mesh_gpu_buffer(mesh, items, 128);
    // The emitter's extent around its entity, for culling.
    ctx.gfx.set_mesh_gpu_bounds(mesh, Vec3::ZERO, 20.0);
    ctx.world.spawn((Transform::default(), Model::new(mesh)));
    items
}
// ANCHOR_END: gpu_buffers
#[test]
fn gpu_buffers_support_headless_setup_and_writes() {
    Game::new("buffers", 1, 1).run_headless(1, |ctx| {
        let b = gpu_particles(ctx);
        assert_eq!(ctx.gfx.gpu_buffer_len(b), Some(128));
        ctx.gfx.write_gpu_buffer(b, 127, &[Vec4::ONE]);
        ctx.gfx.write_gpu_buffer(b, 128, &[]);
        assert_eq!(ctx.scene3d.passes.compute[0].buffer, Some(b));
    });
}
#[test]
fn freed_gpu_buffers_reuse_slots_under_new_ids_and_drop_their_passes() {
    Game::new("buffers", 1, 1).run_headless(0, |ctx| {
        let b = ctx.gfx.create_gpu_buffer(4, &[]);
        let other = ctx.gfx.create_gpu_buffer(2, &[]);
        let update = ctx.gfx.create_compute("up", "fn update(i: u32) { items[i].x += 1.0; }");
        ctx.scene3d.passes.compute.push(ComputePass::for_buffer(update, b));
        ctx.scene3d.passes.compute.push(ComputePass::for_buffer(update, other));
        ctx.free(b);
        assert_eq!(ctx.gfx.gpu_buffer_len(b), None);
        assert_eq!(
            ctx.scene3d.passes.compute.len(),
            1,
            "only the other buffer's pass is left"
        );
        assert_eq!(ctx.scene3d.passes.compute[0].buffer, Some(other));
        ctx.free(b); // again: nothing
        let again = ctx.gfx.create_gpu_buffer(8, &[]);
        assert_ne!(again, b, "the freed slot comes back under a new id");
        assert_eq!(ctx.gfx.gpu_buffer_len(again), Some(8));
        assert_eq!(ctx.gfx.gpu_buffer_len(b), None, "so the old handle stays dead");
        assert!(!ctx.is_live(b));
        ctx.free(b); // a stale free leaves the new buffer alone
        assert_eq!(ctx.gfx.gpu_buffer_len(again), Some(8));
        let next = ctx.gfx.create_gpu_buffer(1, &[]);
        assert_eq!(ponko::__ffi::id(next), 2, "then a new slot");
    });
}
#[test]
#[should_panic(expected = "invalid GPU buffer")]
fn a_freed_gpu_buffer_rejects_writes() {
    Game::new("buffers", 1, 1).run_headless(0, |ctx| {
        let b = ctx.gfx.create_gpu_buffer(1, &[]);
        ctx.free(b);
        ctx.gfx.write_gpu_buffer(b, 0, &[Vec4::ONE]);
    });
}
#[test]
#[should_panic(expected = "out of bounds")]
fn gpu_buffer_writes_validate_ranges() {
    Game::new("buffers", 1, 1).run_headless(0, |ctx| {
        let b = ctx.gfx.create_gpu_buffer(1, &[]);
        ctx.gfx.write_gpu_buffer(b, 1, &[Vec4::ONE]);
    });
}

// ANCHOR: gpu_instances
fn persistent_instances(ctx: &mut Ctx, mesh: Mesh) {
    let transforms = (0..100_000)
        .map(|i| Mat4::from_translation(vec3((i % 1000) as f32, 0.0, (i / 1000) as f32)))
        .collect::<Vec<_>>();
    let batch = ctx.gfx.create_instance_batch(
        mesh,
        &transforms,
        InstanceBatchOptions {
            max_distance: 500.0,
            ..Default::default()
        },
    );
    assert_eq!(ctx.gfx.instance_batch_len(batch), Some(100_000));
    assert!(ctx.gfx.update_instance_batch(batch, &[Mat4::IDENTITY]));
    ctx.gfx.free(batch);
    assert_eq!(ctx.gfx.instance_batch_len(batch), None);
    assert!(!ctx.gfx.update_instance_batch(batch, &[]));
    ctx.gfx.free(batch);
}
// ANCHOR_END: gpu_instances
#[test]
fn hundred_thousand_static_instances_need_no_entities() {
    Game::new("static instances", 1, 1).run_headless(0, |ctx| {
        let material = ctx.gfx.create_material(MaterialDesc::default());
        let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        persistent_instances(ctx, mesh);
        assert_eq!(ctx.world.len(), 0);
    });
}

#[test]
fn target_effects_run_headless_and_keep_their_settings() {
    let mut kept = Vec::new();
    Game::new("t", 64, 64).run_headless(2, |ctx| {
        if ctx.scene3d.passes.target_effects.is_empty() {
            let copy = ctx
                .gfx
                .create_effect("copy", "fn effect(p: Pixel) -> vec3<f32> { return p.color; }");
            let target = ctx.gfx.create_render_target(16, 16);
            let image = ctx.gfx.create_texture(1, 1, &[255; 4]);
            ctx.scene3d.passes.target_effects.push(TargetEffect::new(copy, target));
            // Not a render target: skipped, not an error.
            ctx.scene3d.passes.target_effects.push(TargetEffect::new(copy, image));
        }
        kept = ctx.scene3d.passes.target_effects.clone();
    });
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|e| e.material.is_none()));
}

// ANCHOR: instance_levels
fn leveled_instances(ctx: &mut Ctx, near: Mesh, far: Mesh) {
    let levels = [
        InstanceLevel {
            mesh: near,
            min_distance: 0.0,
            max_distance: 25.0,
            camera: true,
            shadows: false,
        },
        InstanceLevel {
            mesh: far,
            min_distance: 25.0,
            max_distance: 100.0,
            camera: true,
            shadows: true,
        },
        InstanceLevel {
            mesh: far,
            min_distance: 0.0,
            max_distance: 25.0,
            camera: false,
            shadows: true,
        },
    ];
    let batch = ctx
        .gfx
        .create_instance_batch_levels(&levels, &[Mat4::IDENTITY], &[Color::RED], Default::default());
    assert_eq!(ctx.gfx.instance_batch_len(batch), Some(1));
    assert!(ctx.is_live(batch));
    assert!(
        ctx.gfx
            .update_instance_batch_colors(batch, &[Mat4::IDENTITY; 2], &[Color::BLUE; 2])
    );
    assert_eq!(ctx.gfx.instance_batch_len(batch), Some(2));
    assert!(ctx.gfx.update_instance_batch(batch, &[Mat4::IDENTITY]));
    ctx.gfx.free(batch);
    assert!(!ctx.is_live(batch));
    assert!(!ctx.gfx.update_instance_batch_colors(batch, &[], &[]));
    assert_eq!(ctx.gfx.instance_batch_len(batch), None);
    ctx.gfx.free(batch);
}
// ANCHOR_END: instance_levels

#[test]
fn multilevel_batches_keep_handle_lifecycle_without_entities() {
    Game::new("levels", 1, 1).run_headless(0, |ctx| {
        let material = ctx.gfx.create_material(MaterialDesc::default());
        let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        leveled_instances(ctx, mesh, mesh);
        assert_eq!(ctx.world.len(), 0);
    });
}

#[test]
#[should_panic(expected = "instance colors must be empty or match transform count")]
fn instance_colors_reject_mismatched_lengths() {
    Game::new("levels", 1, 1).run_headless(0, |ctx| {
        let material = ctx.gfx.create_material(MaterialDesc::default());
        let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        ctx.gfx.create_instance_batch_levels(
            &[InstanceLevel {
                mesh,
                min_distance: 0.0,
                max_distance: 10.0,
                camera: true,
                shadows: true,
            }],
            &[Mat4::IDENTITY],
            &[Color::WHITE; 2],
            Default::default(),
        );
    });
}

#[test]
fn batch_culling_setter_accepts_live_and_empty_batches_and_rejects_freed_handles() {
    use ponko::render::{BatchCulling, InstanceBatch};
    Game::new("batch culling", 1, 1).run_headless(0, |ctx| {
        let material = ctx.gfx.create_material(Default::default());
        let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        let batch = ctx.gfx.create_instance_batch(mesh, &[], Default::default());
        for mode in [BatchCulling::Auto, BatchCulling::PreferGpu, BatchCulling::PreferCpu] {
            assert!(ctx.gfx.set_instance_batch_culling(batch, mode));
        }
        ctx.gfx.free(batch);
        assert!(!ctx.gfx.set_instance_batch_culling(batch, BatchCulling::Auto));
        assert!(
            !ctx.gfx
                .set_instance_batch_culling(InstanceBatch::NONE, BatchCulling::Auto)
        );
    });
}

#[test]
fn quality_autotune_is_scene_opt_in_and_headless_stays_fixed() {
    // ANCHOR: quality-autotune
    use ponko::render::QualityAutotune;
    Game::new("Quality policy", 640, 360).run_headless(2, |ctx| {
        assert!(ctx.scene3d.quality_autotune_status(&ctx.gfx).is_none()); // default: off
        ctx.scene3d.set_quality_autotune(Some(QualityAutotune {
            target_gpu_ms: 1000.0 / 60.0,
            min_render_scale: 0.6,
            max_render_scale: 1.0,
            min_shadow_resolution: 512,
            max_shadow_resolution: 2048,
        }));
        // An options-menu choice always wins, including same-value writes.
        ctx.scene3d.pin_shadow_resolution(2048);
        // Freeze before a deterministic screenshot sequence and its warm-up.
        ctx.scene3d.freeze_quality_autotune(true);
        let status = ctx.scene3d.quality_autotune_status(&ctx.gfx).unwrap();
        assert!(status.shadow_resolution_pinned);
        assert_eq!(status.changes, 0); // headless never tunes
        ctx.scene3d.freeze_quality_autotune(false);
        ctx.scene3d.set_quality_autotune(None); // retain choices, stop tuning
    });
    // ANCHOR_END: quality-autotune
}
