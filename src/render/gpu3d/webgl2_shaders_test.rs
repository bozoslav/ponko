//! The shaders a WebGL2 device builds, translated to GLSL ES 3.0 the way
//! wgpu's GL backend translates them in the browser (naga's GLSL writer).
//! No GPU needed: a shader WebGL2 can't run fails here as it fails at
//! pipeline creation there. Reading the scene depth with `textureLoad` used
//! to (naga can't write a texel load of a depth texture as GLSL), which
//! stopped every scene-reading material and every screen effect on WebGL2.
use super::*;
use naga::back::glsl;

/// What a WebGL2 device builds: no decals, global illumination or instance
/// storage, one sample per pixel, the scene depth read as a float texture.
fn webgl2_material(user: &str) -> String {
    for_samples(&compose_shader_storage(user, false, false, false), 1, true).into_owned()
}

/// The same shader on a backend that keeps the depth binding.
fn depth_binding_material(user: &str) -> String {
    for_samples(&compose_shader_storage(user, false, false, false), 1, false).into_owned()
}

/// A transparent material that reads the scene behind it (a lake).
const SCENE_READER: &str = "
fn surface(s: Surface) -> Surface {
    var out = s;
    let behind = max(scene_distance(s.screen) - distance(s.world, frame.camera.xyz), 0.0);
    out.emissive = scene_color(s.screen) * exp(-behind) + scene_position(s.screen) * 0.0;
    return out;
}";

/// A game's shader that loads the scene depth itself, as the book allows.
const DIRECT_LOAD: &str = "
fn surface(s: Surface) -> Surface {
    var out = s;
    // textureLoad(scene_depth_texture, unbalanced in a comment (
    let size = vec2<i32>(textureDimensions(scene_depth_texture));
    let d = textureLoad( scene_depth_texture , clamp(vec2<i32>(s.screen), vec2<i32>(0), size - 1), 0 );
    out.alpha = select(1.0, 0.5, d >= 1.0);
    return out;
}";

/// A screen effect that reads every depth-derived field of `Pixel`.
const DEPTH_EFFECT: &str = "
fn effect(p: Pixel) -> vec3<f32> {
    let fade = select(1.0 - exp(-p.distance * 0.05), 1.0, p.sky);
    return mix(p.color, p.world * 0.0, fade);
}";

/// `source` as GLSL `version` for each of its vertex and fragment entry
/// points, as wgpu's GL backend writes it, or the first error.
fn translate(name: &str, source: &str, version: glsl::Version) -> Result<Vec<String>, String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string_with_path(source, name))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| e.emit_to_string_with_path(source, name))?;
    let options = glsl::Options {
        version,
        writer_flags: glsl::WriterFlags::ADJUST_COORDINATE_SPACE | glsl::WriterFlags::FORCE_POINT_SIZE,
        binding_map: Default::default(),
        zero_initialize_workgroup_memory: true,
    };
    // Unchecked image loads, as wgpu asks for on OpenGL ES.
    let policies = naga::proc::BoundsCheckPolicies::default();
    let mut written = Vec::new();
    for entry in &module.entry_points {
        if !matches!(entry.stage, naga::ShaderStage::Vertex | naga::ShaderStage::Fragment) {
            continue;
        }
        let pipeline = glsl::PipelineOptions {
            shader_stage: entry.stage,
            entry_point: entry.name.clone(),
            multiview: None,
        };
        let mut out = String::new();
        glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, policies)
            .and_then(|mut writer| writer.write())
            .map_err(|e| format!("{name}, {}: {e}", entry.name))?;
        written.push(out);
    }
    assert!(!written.is_empty(), "{name} has no vertex or fragment entry points");
    Ok(written)
}

const WEBGL2: glsl::Version = glsl::Version::Embedded {
    version: 300,
    is_webgl: true,
};

/// Every shader a WebGL2 device builds that reads the scene depth, or that
/// the depth and texture array fixes added.
fn webgl2_shaders() -> Vec<(&'static str, String)> {
    vec![
        ("built-in material", webgl2_material("")),
        ("scene-reading material", webgl2_material(SCENE_READER)),
        ("material loading the depth", webgl2_material(DIRECT_LOAD)),
        (
            "effect stand-in",
            for_samples(&compose_effect(crate::three::EFFECT_STAND_IN), 1, true).into_owned(),
        ),
        (
            "depth-reading effect",
            for_samples(&compose_effect(DEPTH_EFFECT), 1, true).into_owned(),
        ),
        (
            "sky",
            for_samples(&format!("{MESH_PRELUDE}\n{SKY}"), 1, true).into_owned(),
        ),
        ("depth copy", depth_copy::shader(1)),
        ("array mipmaps", crate::render::mips::ARRAY_SHADER.to_string()),
        ("mipmaps", crate::render::mips::SHADER.to_string()),
    ]
}

#[test]
fn webgl2_shaders_translate_to_glsl_es_300() {
    for (name, source) in webgl2_shaders() {
        if let Err(e) = translate(name, &source, WEBGL2) {
            panic!("{name} can't run on WebGL2:\n{e}");
        }
    }
}

/// The negative control: with the depth binding, as before, the shaders
/// that read the scene depth don't translate, for the reason WebGL2 gave.
#[test]
fn depth_binding_shaders_fail_to_translate_to_glsl() {
    for (name, source) in [
        ("scene-reading material", depth_binding_material(SCENE_READER)),
        ("material loading the depth", depth_binding_material(DIRECT_LOAD)),
        (
            "depth-reading effect",
            for_samples(&compose_effect(DEPTH_EFFECT), 1, false).into_owned(),
        ),
    ] {
        let e = translate(name, &source, WEBGL2).expect_err(name);
        assert!(e.contains("textureLoad` from depth textures"), "{name}: {e}");
    }
}

/// Native OpenGL with multisampling keeps loading each sample (desktop GLSL
/// has multisampled textures and sample indices; WebGL2 never multisamples).
#[test]
fn multisampled_float_depth_translates_to_desktop_glsl() {
    let desktop = glsl::Version::Desktop(430);
    for (name, source) in [
        (
            "scene-reading material",
            for_samples(&compose_shader_storage(SCENE_READER, false, false, false), 4, true).into_owned(),
        ),
        (
            "depth-reading effect",
            for_samples(&compose_effect(DEPTH_EFFECT), 4, true).into_owned(),
        ),
        ("depth copy", depth_copy::shader(4)),
    ] {
        if let Err(e) = translate(name, &source, desktop) {
            panic!("{name} can't run on multisampled OpenGL:\n{e}");
        }
    }
}

/// The float-depth shaders still pass the engine's own WGSL checks, with
/// every capability (what a native device validates them with).
#[test]
fn float_depth_shaders_stay_valid_wgsl() {
    for samples in [1, 4] {
        for (name, user) in [
            ("built-in", ""),
            ("scene reader", SCENE_READER),
            ("direct load", DIRECT_LOAD),
        ] {
            check_shader(name, &for_samples(&compose_shader(user), samples, true)).unwrap();
        }
        check_shader("effect", &for_samples(&compose_effect(DEPTH_EFFECT), samples, true)).unwrap();
        check_shader("depth copy", &depth_copy::shader(samples)).unwrap();
    }
}

#[test]
fn scene_depth_loads_become_samples_with_one_sample() {
    let source = "var scene_depth_texture: texture_depth_multisampled_2d;\n\
        let a = textureLoad(scene_depth_texture, f(vec2<i32>(1, 2), x[0]), 0);\n\
        // textureLoad(scene_depth_texture, at, 0) stays as it is in a comment\n\
        let b = textureLoad(other, at, 0) + mytextureLoad(scene_depth_texture, at, 0);\n";
    let out = depth_as_float(source, 1);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "var scene_depth_texture: texture_2d<f32>;");
    assert_eq!(
        lines[1],
        "let a = scene_depth_texel(vec2<i32>(f(vec2<i32>(1, 2), x[0]))).x;"
    );
    assert_eq!(
        lines[2],
        "// textureLoad(scene_depth_texture, at, 0) stays as it is in a comment"
    );
    // Other textures, and names that only end in textureLoad, are left alone.
    assert_eq!(
        lines[3],
        "let b = textureLoad(other, at, 0) + mytextureLoad(scene_depth_texture, at, 0);"
    );
    assert!(out.ends_with(SAMPLED_DEPTH), "the sampler and the helper come last");
    assert!(
        out.contains("@group(2) @binding(2) var scene_depth_sampler: sampler;"),
        "{out}"
    );
}

#[test]
fn scene_depth_loads_take_the_red_channel_multisampled() {
    let source = "var scene_depth_texture: texture_depth_multisampled_2d;\n\
        let a = textureLoad(scene_depth_texture, at, i) >= 1.0;\n";
    assert_eq!(
        depth_as_float(source, 4),
        "var scene_depth_texture: texture_multisampled_2d<f32>;\n\
         let a = textureLoad(scene_depth_texture, at, i).x >= 1.0;\n"
    );
}

/// Error messages about a game's shader point at its lines on GL too.
#[test]
fn rewriting_the_depth_reads_keeps_every_line_in_place() {
    let source = compose_shader(DIRECT_LOAD);
    let out = depth_as_float(&source, 1);
    let before: Vec<&str> = source.lines().collect();
    let after: Vec<&str> = out.lines().collect();
    assert!(after.len() > before.len(), "the sampler is added after the end");
    for (n, (a, b)) in before.iter().zip(&after).enumerate() {
        if !a.contains("scene_depth_texture") {
            assert_eq!(a, b, "line {} moved or changed", n + 1);
        }
    }
}

#[test]
fn backends_other_than_gl_keep_the_depth_binding() {
    assert_eq!(scene_depth_sample_type(false), wgpu::TextureSampleType::Depth);
    assert_eq!(
        scene_depth_sample_type(true),
        wgpu::TextureSampleType::Float { filterable: false }
    );
    let source = compose_shader(SCENE_READER);
    assert_eq!(
        for_samples(&source, 4, false),
        source.as_str(),
        "multisampled, unchanged"
    );
    assert!(samples_scene_depth(true, 1));
    assert!(!samples_scene_depth(true, 4), "multisampled: loaded per sample");
    assert!(!samples_scene_depth(false, 1));
}
