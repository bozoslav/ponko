//! Executable guide examples and public shader contract regressions.
#![cfg(feature = "3d")]
use ponko::anim::{Joint, Skeleton};
use ponko::prelude::*;
use ponko::render::{Blend, MaterialDesc, MeshData, Model};

// ANCHOR: material_features
const UNLIT: &str = include_str!("fixtures/shaders/material-unlit.wgsl");

fn material_features(ctx: &mut Ctx) {
    // Lit is the default; this hook explicitly opts into unlit shading.
    let unlit = ctx.gfx.create_shader("unlit", UNLIT);
    for blend in [Blend::Opaque, Blend::Cutout, Blend::Transparent] {
        for double_sided in [false, true] {
            for shader in [None, Some(unlit)] {
                let desc = MaterialDesc::default()
                    .with_color(Color::rgba(
                        1.0,
                        1.0,
                        1.0,
                        if blend == Blend::Transparent { 0.3 } else { 1.0 },
                    ))
                    .with_blend(blend)
                    .with_double_sided(double_sided)
                    .with_shader(shader);
                let material = ctx.gfx.create_material(desc);
                let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
                ctx.world.spawn((d3::Transform::default(), Model::new(mesh)));
            }
        }
    }
}
// ANCHOR_END: material_features

// ANCHOR: material_skin
fn skinned_material(ctx: &mut Ctx) {
    let skeleton = Skeleton::new(vec![Joint {
        name: "root".into(),
        parent: None,
        rest: d3::Transform::default(),
    }]);
    let mut data = MeshData::cube(1.0);
    data.joints = vec![[0; 4]; data.positions.len()];
    data.weights = vec![[1.0, 0.0, 0.0, 0.0]; data.positions.len()];
    let material = ctx.gfx.default_material();
    let mesh = ctx.gfx.create_skinned_mesh(&data, material, &skeleton);
    ctx.world.spawn((d3::Transform::default(), Model::new(mesh), skeleton));
}
// ANCHOR_END: material_skin

// ANCHOR: material_instances
fn instanced_material(ctx: &mut Ctx) {
    let material = ctx.gfx.default_material();
    let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
    ctx.gfx.create_instance_batch(
        mesh,
        &[Mat4::IDENTITY, Mat4::from_translation(Vec3::X * 2.0)],
        Default::default(),
    );
}
// ANCHOR_END: material_instances

// ANCHOR: material_displacement
fn displaced_material(ctx: &mut Ctx) {
    let shader = ctx.gfx.create_shader(
        "displaced",
        "
fn vertex(v: Vertex) -> Vertex {
    var out = v;
    out.position.y += sin(frame.time.x) * 2.0;
    return out;
}",
    );
    let material = ctx.gfx.create_material(MaterialDesc::default().with_shader(shader));
    let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
    // The cube's radius plus the maximum two-unit world displacement.
    // This bound assumes unit scale; adapt it when changing the transform.
    ctx.gfx.set_mesh_gpu_bounds(mesh, Vec3::ZERO, 3.0);
    ctx.world.spawn((d3::Transform::default(), Model::new(mesh)));
}
// ANCHOR_END: material_displacement

#[test]
fn every_guide_feature_sample_runs() {
    Game::new("material features", 64, 64).run_headless(0, |ctx| {
        material_features(ctx);
        skinned_material(ctx);
        instanced_material(ctx);
        displaced_material(ctx);
    });
}

#[cfg(feature = "window")]
#[test]
fn contract_errors_name_the_hook_rule_and_fix_at_the_user_line() {
    Game::new("contract", 1, 1).run_headless(0, |ctx| {
        for (source, expected) in [
            (
                "@fragment fn surface(s: Surface) -> Surface { return s; }",
                "remove the @vertex/@fragment/@compute attribute",
            ),
            (
                "fn vertex() -> f32 { return 1.0; }",
                "fn vertex(value: Vertex) -> Vertex",
            ),
            (
                "fn surface(s: Surface, x: f32) -> Surface { return s; }",
                "fn surface(value: Surface) -> Surface",
            ),
            (
                "fn surface(s: Surface) -> f32 { return 1.0; }",
                "fn surface(value: Surface) -> Surface",
            ),
            (
                "@group(7) @binding(0) var<uniform> mine: vec4<f32>;",
                "every @group/@binding is engine-owned",
            ),
            (
                "@group(0) @binding(0) var<uniform> frame: vec4<f32>;",
                "remove the declaration",
            ),
        ] {
            let err = ctx.gfx.try_create_shader("contract.wgsl", source).unwrap_err();
            assert!(err.message().contains(expected), "{err}");
            let d = &ctx.gfx.shader_status().last().unwrap().diagnostics[0];
            assert_eq!(d.line, Some(1));
            assert!(!d.in_engine_code);
        }
        // Comments cannot declare hooks or engine-owned bindings.
        ctx.gfx
            .try_create_shader("comments", "/* fn vertex() {} /* @group(0) */ */ // fn surface() {}\n")
            .unwrap();
        ctx.gfx
            .try_create_shader(
                "trailing comma",
                "fn surface(value: Surface,) -> Surface { return value; }",
            )
            .unwrap();
    });
}

#[test]
#[should_panic(expected = "skinned meshes cannot use persistent instance batches")]
fn skinned_static_batches_explain_how_to_draw_a_pose() {
    Game::new("contract", 1, 1).run_headless(0, |ctx| {
        let skeleton = Skeleton::new(vec![Joint {
            name: "root".into(),
            parent: None,
            rest: d3::Transform::default(),
        }]);
        let mut data = MeshData::cube(1.0);
        data.joints = vec![[0; 4]; data.positions.len()];
        data.weights = vec![[1.0, 0.0, 0.0, 0.0]; data.positions.len()];
        let material = ctx.gfx.default_material();
        let mesh = ctx.gfx.create_skinned_mesh(&data, material, &skeleton);
        ctx.gfx
            .create_instance_batch(mesh, &[Mat4::IDENTITY], Default::default());
    });
}

#[test]
#[should_panic(expected = "skinned meshes cannot use GPU-buffer instances")]
fn skinned_gpu_buffers_explain_how_to_draw_a_pose() {
    Game::new("contract", 1, 1).run_headless(0, |ctx| {
        let skeleton = Skeleton::new(vec![Joint {
            name: "root".into(),
            parent: None,
            rest: d3::Transform::default(),
        }]);
        let mut data = MeshData::cube(1.0);
        data.joints = vec![[0; 4]; data.positions.len()];
        data.weights = vec![[1.0, 0.0, 0.0, 0.0]; data.positions.len()];
        let material = ctx.gfx.default_material();
        let mesh = ctx.gfx.create_skinned_mesh(&data, material, &skeleton);
        let buffer = ctx.gfx.create_gpu_buffer(1, &[Vec4::ZERO]);
        ctx.gfx.set_mesh_gpu_buffer(mesh, buffer, 1);
    });
}
