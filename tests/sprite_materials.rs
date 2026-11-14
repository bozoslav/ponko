//! Sprite blend modes and sprite materials, headless: shader errors as data
//! in the shader's own lines, the sprite hook's rules, hot reload of
//! sprite shader files, and the handles' lifecycle. (What entities with a
//! look draw is tested in `src/sprite_material.rs`.)
#![cfg(feature = "window")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ponko::prelude::*;
use ponko::render::{ShaderKind, ShaderOutcome, ShaderStatus};
use ponko::render::{SpriteMaterial, SpriteMaterialDesc, SpriteShader};

const GOOD: &str = "fn fragment(p: SpritePixel) -> vec4<f32> {\n    return p.color * material.params[0];\n}\n";
/// Line 3 returns a number, not a color.
const BROKEN: &str = "// flashes white\nfn fragment(p: SpritePixel) -> vec4<f32> {\n    return 1.0;\n}\n";

#[test]
fn a_broken_sprite_shader_says_where_and_why_in_its_own_lines() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let err = ctx.gfx.try_create_sprite_shader("flash.wgsl", BROKEN).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Shader);
        assert!(err.message().starts_with("flash.wgsl:3:"), "{err}");
        let status = ctx.gfx.shader_status();
        assert_eq!(status.len(), 1);
        let s = &status[0];
        assert_eq!(
            (s.shader.as_str(), s.kind, s.outcome),
            ("flash.wgsl", ShaderKind::Sprite, ShaderOutcome::Failed)
        );
        let d = &s.diagnostics[0];
        assert_eq!(d.line, Some(3), "{d}");
        assert!(!d.in_engine_code, "{d}");
        assert_eq!(d.source_line.as_deref(), Some("    return 1.0;"));

        // The control: correct code compiles, and its status says so.
        ctx.gfx.create_sprite_shader("tint.wgsl", GOOD);
        let s = &ctx.gfx.shader_status()[1];
        assert_eq!((s.outcome, s.kind), (ShaderOutcome::Compiled, ShaderKind::Sprite));
        assert!(s.is_ok() && s.diagnostics.is_empty());
    });
}

fn error_for(source: &str) -> String {
    let mut message = None;
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        message = Some(
            ctx.gfx
                .try_create_sprite_shader("s.wgsl", source)
                .unwrap_err()
                .to_string(),
        );
    });
    message.unwrap()
}

#[test]
fn sprite_shaders_follow_the_hook_contract() {
    assert!(error_for("fn helper() -> f32 { return 1.0; }").contains("needs a `fragment` hook"));
    assert!(error_for("fn fragment(p: SpritePixel) -> vec3<f32> { return p.color.rgb; }").contains("wrong signature"));
    let entry = error_for("@fragment\nfn fragment(p: SpritePixel) -> vec4<f32> { return p.color; }");
    assert!(entry.contains("not a shader-stage entry point"), "{entry}");
    assert!(entry.contains("s.wgsl:1:1: error"), "{entry}");
    let binding = error_for(
        "@group(3) @binding(0) var<uniform> mine: vec4<f32>;\nfn fragment(p: SpritePixel) -> vec4<f32> { return mine; }",
    );
    assert!(binding.contains("may not declare bindings"), "{binding}");
    // A trailing comma and any parameter name are fine.
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.gfx
            .try_create_sprite_shader(
                "ok.wgsl",
                "fn fragment(pixel: SpritePixel,) -> vec4<f32> { return pixel.tint; }",
            )
            .unwrap();
    });
}

#[test]
fn sprite_shaders_are_held_to_what_every_supported_gpu_has() {
    // 64-bit floats exist on some desktop GPUs, not in WebGL2 or WebGPU.
    let e = error_for(
        "fn fragment(p: SpritePixel) -> vec4<f32> {\n    let x: f64 = 1.0lf;\n    return p.color * f32(x);\n}",
    );
    assert!(e.contains("s.wgsl:2:") && e.contains("FLOAT64"), "{e}");
}

#[test]
fn the_engine_helpers_are_there_for_sprite_shaders() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.gfx.create_sprite_shader(
            "helpers.wgsl",
            "fn fragment(p: SpritePixel) -> vec4<f32> {
                let edge = sprite_sample(p.uv + sprite_texel()).a;
                let noise = material_sample0(p.local).r * material_sample1(p.world * 0.01).g;
                if noise < material.params[0].x { discard; }
                return vec4<f32>(p.color.rgb + p.texture.rgb * edge, p.tint.a);
            }",
        );
    });
}

#[test]
fn sprite_material_handles_live_until_freed() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let before = ctx.live_counts();
        let shader = ctx.gfx.create_sprite_shader("tint.wgsl", GOOD);
        let pixel = ctx.gfx.create_texture(1, 1, &[255; 4]);
        let desc = SpriteMaterialDesc::new(shader)
            .with_param(0, vec4(1.0, 0.0, 0.0, 1.0))
            .with_texture(1, pixel);
        let material = ctx.gfx.create_sprite_material(desc);
        assert!(ctx.is_live(shader) && ctx.is_live(material));
        assert_eq!(ctx.live_counts().sprite_shaders, before.sprite_shaders + 1);
        assert_eq!(ctx.live_counts().sprite_materials, before.sprite_materials + 1);
        assert_eq!(ctx.gfx.sprite_material_desc(material), Some(desc));

        let brighter = desc.with_param(0, vec4(2.0, 2.0, 2.0, 1.0));
        ctx.gfx.update_sprite_material(material, brighter);
        assert_eq!(ctx.gfx.sprite_material_desc(material), Some(brighter));

        ctx.free(material);
        ctx.free(shader);
        assert!(!ctx.is_live(material) && !ctx.is_live(shader));
        assert_eq!(ctx.gfx.sprite_material_desc(material), None);
        ctx.gfx.update_sprite_material(material, desc); // does nothing
        assert_eq!(ctx.gfx.sprite_material_desc(material), None);
        ctx.free(pixel);
        assert_eq!(ctx.live_counts(), before);
        assert!(!ctx.is_live(SpriteMaterial::NONE) && !ctx.is_live(SpriteShader::NONE));
    });
}

#[test]
#[should_panic(expected = "a sprite material has params 0 to 3, not 4")]
fn a_fifth_param_is_refused() {
    let _ = SpriteMaterialDesc::default().with_param(4, Vec4::ONE);
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-sprite-shader-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Loads `file` with `first`, writes each of `edits` on its own tick, and
/// returns the shader's status after each edit.
fn reload(file: &str, first: &str, edits: &'static [&'static str], hot_reload: bool) -> Vec<ShaderStatus> {
    let path = temp_dir().join(file);
    std::fs::write(&path, first).unwrap();
    let name = path.to_str().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let edit_path = path.clone();
    Game::new("hot", 1, 1)
        .hot_reload(hot_reload)
        .sim_system(move |ctx: &mut Ctx| {
            let tick = ctx.time.sim_tick() as usize;
            if tick >= 2 && tick.is_multiple_of(2) && tick / 2 <= edits.len() {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::write(&edit_path, edits[tick / 2 - 1]).unwrap();
                std::thread::sleep(Duration::from_millis(400));
            }
            if tick >= 3 && !tick.is_multiple_of(2) {
                log.lock().unwrap().push(ctx.gfx.shader_status()[0].clone());
            }
        })
        .run_headless(2 * edits.len() as u64 + 2, |ctx| {
            ctx.gfx.load_sprite_shader(&name);
        });
    seen.lock().unwrap().clone()
}

#[test]
fn edited_sprite_shader_files_reload_and_broken_edits_keep_the_last_good_one() {
    let seen = reload("glow.wgsl", GOOD, &[BROKEN, GOOD], true);
    let outcomes: Vec<_> = seen.iter().map(|s| (s.outcome, s.reloads)).collect();
    assert_eq!(
        outcomes,
        [(ShaderOutcome::ReloadFailed, 0), (ShaderOutcome::Reloaded, 1)]
    );
    assert_eq!(seen[0].diagnostics[0].line, Some(3), "{}", seen[0].diagnostics[0]);
    // The control: with hot reload off, nothing changes.
    let off = reload("glow_off.wgsl", GOOD, &[BROKEN], false);
    assert_eq!(off[0].outcome, ShaderOutcome::Compiled);
}

#[test]
fn a_sprite_shader_file_that_never_compiled_gets_a_stand_in_until_fixed() {
    let seen = reload("fix.wgsl", BROKEN, &[GOOD], true);
    assert_eq!((seen[0].outcome, seen[0].reloads), (ShaderOutcome::Reloaded, 1));
}

#[test]
fn the_examples_sprite_shaders_compile() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        for file in ["shaders/sprite_outline.wgsl", "shaders/sprite_dissolve.wgsl"] {
            ctx.gfx.try_load_sprite_shader(file).unwrap();
        }
        assert!(ctx.gfx.shader_status().iter().all(ShaderStatus::is_ok));
    });
}
