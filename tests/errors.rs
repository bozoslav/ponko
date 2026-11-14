//! `ponko::prelude::Error`: every loader's `try_` twin reports what went wrong as an
//! [`ErrorKind`], and the panicking twin panics with the same message.

use ponko::prelude::*;
#[allow(unused_imports)]
use ponko::reflect::ReflectError;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{Material, MaterialDesc, MeshData};

/// Writes `bytes` to a temporary file and returns its absolute path, which
/// the asset loaders accept as is.
fn temp_file(name: &str, bytes: &[u8]) -> String {
    let path = std::env::temp_dir().join(format!("ponko-errors-{}-{name}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

fn in_tick(f: impl FnOnce(&mut Ctx) + 'static) {
    Game::new("errors", 1, 1).run_headless(0, f);
}

#[test]
fn missing_assets_are_not_found_for_every_loader() {
    in_tick(|ctx| {
        let missing = "no/such/asset.bin";
        assert_eq!(
            ctx.gfx.try_load_texture(missing).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert_eq!(ctx.gfx.try_load_font(missing).unwrap_err().kind(), ErrorKind::NotFound);
        assert_eq!(ctx.audio.try_load(missing).unwrap_err().kind(), ErrorKind::NotFound);
        let err = ctx.gfx.try_load_texture(missing).unwrap_err();
        assert_eq!(err.path(), Some(missing));
    });
}

#[test]
fn files_that_are_not_images_fail_to_decode() {
    let path = temp_file("bad.png", b"not a png");
    in_tick(move |ctx| {
        let err = ctx.gfx.try_load_texture(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Decode);
        assert_eq!(err.path(), Some(path.as_str()));
    });
}

#[test]
fn files_that_are_not_fonts_fail_to_decode() {
    let path = temp_file("bad.ttf", b"not a font");
    in_tick(move |ctx| {
        assert_eq!(ctx.gfx.try_load_font(&path).unwrap_err().kind(), ErrorKind::Decode);
    });
}

#[cfg(feature = "audio")]
#[test]
fn files_that_are_not_sounds_fail_to_decode() {
    let path = temp_file("bad.ogg", b"not a sound");
    in_tick(move |ctx| {
        assert_eq!(ctx.audio.try_load(&path).unwrap_err().kind(), ErrorKind::Decode);
    });
}

#[test]
fn a_failed_load_leaves_later_loads_working() {
    in_tick(|ctx| {
        assert!(ctx.gfx.try_load_texture("nope.png").is_err());
        let a = ctx.gfx.try_load_texture("crab.png").unwrap();
        assert_eq!(ctx.gfx.try_load_texture("crab.png").unwrap(), a);
    });
}

#[test]
fn errors_convert_with_question_mark_into_boxed_errors() {
    fn load(ctx: &mut Ctx) -> Result<Texture, Box<dyn std::error::Error>> {
        Ok(ctx.gfx.try_load_texture("nope.png")?)
    }
    in_tick(|ctx| {
        let err = load(ctx).unwrap_err();
        let err = err.downcast_ref::<Error>().expect("a ponko::prelude::Error");
        assert_eq!(err.kind(), ErrorKind::NotFound);
    });
}

#[test]
#[should_panic(expected = "ponko: \"nope.png\" not found in assets/")]
fn panicking_loaders_panic_with_the_error_message_once_prefixed() {
    in_tick(|ctx| {
        ctx.gfx.load_texture("nope.png");
    });
}

#[test]
fn panics_and_errors_read_the_same() {
    in_tick(|ctx| {
        let err = ctx.gfx.try_load_font("nope.ttf").unwrap_err();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ctx.gfx.load_font("nope.ttf");
        }))
        .unwrap_err();
        let message = panic.downcast_ref::<String>().unwrap();
        assert_eq!(message, &err.to_string());
        assert!(!message.starts_with("ponko: ponko:"));
    });
}

#[test]
fn reflection_errors_convert_into_error() {
    let err: Error = ReflectError::UnknownResource("Nope".into()).into();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn custom_decoders_keep_their_kind_and_get_the_path() {
    let path = temp_file("level.tiles", b"#?");
    in_tick(move |ctx| {
        let decode = |bytes: &[u8]| -> Result<usize, Error> {
            if bytes.contains(&b'?') {
                return Err(Error::new(ErrorKind::Decode, "unknown tile"));
            }
            Ok(bytes.len())
        };
        let err = ctx.assets.load_with(&path, decode).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Decode);
        assert_eq!(err.path(), Some(path.as_str()));
        assert!(err.message().ends_with("could not be decoded: unknown tile"), "{err}");
        assert!(!err.to_string().contains("ponko: ponko:"), "{err}");
        let err = ctx.assets.load_with("no/such.tiles", decode).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    });
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize, Component)]
struct Tuning {
    speed: f32,
}

#[test]
fn data_files_report_missing_files_bad_json_and_dead_entities() {
    let bad = temp_file("tuning.json", b"{ speed: ");
    in_tick(move |ctx| {
        ctx.world.reflect_resource::<Tuning>();
        ctx.world.reflect::<Tuning>();
        let err = ctx
            .assets
            .load_resource_json::<Tuning>(&mut ctx.world, "no/such.json")
            .unwrap_err();
        assert_eq!((err.kind(), err.path()), (ErrorKind::NotFound, Some("no/such.json")));
        let err = ctx
            .assets
            .load_resource_json::<Tuning>(&mut ctx.world, &bad)
            .unwrap_err();
        assert_eq!((err.kind(), err.path()), (ErrorKind::Json, Some(bad.as_str())));
        let dead = ctx.world.spawn((Tuning::default(),));
        ctx.world.despawn(dead);
        let good = temp_file("tuning-good.json", br#"{"speed": 2}"#);
        let err = ctx
            .assets
            .load_component_json::<Tuning>(&mut ctx.world, &good, dead)
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BadHandle);
        let err = ctx
            .assets
            .save_prefab(&ctx.world, dead, "never-written.json")
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BadHandle);
    });
}

#[test]
fn scenes_and_prefabs_report_their_kind_and_path() {
    let not_json = temp_file("scene.json", b"{ entities: ");
    let not_prefab = temp_file("prefab.json", br#"{"format": "something-else"}"#);
    in_tick(move |ctx| {
        let err = ctx.load_scene("no/such/scene.json").unwrap_err();
        assert_eq!(
            (err.kind(), err.path()),
            (ErrorKind::NotFound, Some("no/such/scene.json"))
        );
        let err = ctx.load_scene(&not_json).unwrap_err();
        assert_eq!((err.kind(), err.path()), (ErrorKind::Json, Some(not_json.as_str())));
        let err = ctx.spawn_prefab(&not_prefab, Vec2::ZERO).unwrap_err();
        assert_eq!((err.kind(), err.path()), (ErrorKind::Json, Some(not_prefab.as_str())));
        assert!(!err.to_string().contains("ponko: ponko:"), "{err}");
        let err = ctx.spawn_prefab("no/such/prefab.json", Vec2::ZERO).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    });
}

#[test]
fn perf_reports_and_diagnostics_report_their_kind() {
    let err = ponko::perf::PerfReport::from_json("{").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Json);
    let err = ponko::perf::PerfReport::load("no/such/report.json").unwrap_err();
    assert_eq!(
        (err.kind(), err.path()),
        (ErrorKind::NotFound, Some("no/such/report.json"))
    );
    assert_eq!(ponko::perf::Budget::from_json("[").unwrap_err().kind(), ErrorKind::Json);
    in_tick(|ctx| {
        let err = ctx.ecs_diagnostics().matching(&["Nope"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    });
}

#[cfg(feature = "3d")]
mod three {
    use super::*;

    #[test]
    fn missing_models_are_not_found() {
        in_tick(|ctx| {
            assert_eq!(
                ctx.gfx.try_load_mesh("no/such.glb").unwrap_err().kind(),
                ErrorKind::NotFound
            );
            assert_eq!(
                ctx.gfx.try_load_skeleton("no/such.glb").unwrap_err().kind(),
                ErrorKind::NotFound
            );
        });
    }

    #[test]
    fn files_that_are_not_models_fail_to_decode() {
        let path = temp_file("bad.glb", b"not a model");
        in_tick(move |ctx| {
            assert_eq!(ctx.gfx.try_load_mesh(&path).unwrap_err().kind(), ErrorKind::Decode);
            assert_eq!(ctx.gfx.try_load_skeleton(&path).unwrap_err().kind(), ErrorKind::Decode);
        });
    }

    #[test]
    fn out_of_range_materials_are_invalid() {
        in_tick(|ctx| {
            let desc = MaterialDesc::default().with_metallic(-1.0);
            assert_eq!(
                ctx.gfx.try_create_material(desc).unwrap_err().kind(),
                ErrorKind::Invalid
            );
            assert!(ctx.gfx.try_create_material(MaterialDesc::default()).is_ok());
        });
    }

    #[test]
    fn meshes_check_their_data_and_material() {
        // A material from another game, which made more materials than this one.
        let other = std::rc::Rc::new(std::cell::Cell::new(None));
        let out = other.clone();
        in_tick(move |ctx| {
            for _ in 0..4 {
                out.set(Some(ctx.gfx.create_material(MaterialDesc::default())));
            }
        });
        let foreign: Material = other.get().unwrap();
        in_tick(move |ctx| {
            let stone = ctx.gfx.create_material(MaterialDesc::default());
            let mut broken = MeshData::cube(1.0);
            broken.indices.pop();
            assert_eq!(
                ctx.gfx.try_create_mesh(&broken, stone).unwrap_err().kind(),
                ErrorKind::Invalid
            );
            let err = ctx.gfx.try_create_mesh(&MeshData::cube(1.0), foreign).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::BadHandle);
            assert!(ctx.gfx.try_create_mesh(&MeshData::cube(1.0), stone).is_ok());
        });
    }

    use ponko::terrain::{TerrainMaterialDesc, TextureKind};

    const BROKEN: &str = "fn surface(s: Surface) -> Surface { return 1.0; }";

    #[test]
    fn missing_shader_files_are_not_found_with_their_path() {
        in_tick(|ctx| {
            let missing = "no/such.wgsl";
            for err in [
                ctx.gfx.try_load_shader(missing).unwrap_err(),
                ctx.gfx.try_load_effect(missing).unwrap_err(),
                ctx.gfx.try_load_compute(missing).unwrap_err(),
            ] {
                assert_eq!((err.kind(), err.path()), (ErrorKind::NotFound, Some(missing)));
            }
        });
    }

    #[test]
    fn shader_files_that_are_not_text_fail_to_decode() {
        let path = temp_file("bad.wgsl", &[0xff, 0xfe, 0xfd]);
        in_tick(move |ctx| {
            for err in [
                ctx.gfx.try_load_shader(&path).unwrap_err(),
                ctx.gfx.try_load_effect(&path).unwrap_err(),
                ctx.gfx.try_load_compute(&path).unwrap_err(),
            ] {
                assert_eq!((err.kind(), err.path()), (ErrorKind::Decode, Some(path.as_str())));
            }
        });
    }

    #[cfg(feature = "window")]
    #[test]
    fn shaders_that_do_not_compile_are_shader_errors() {
        let path = temp_file("broken.wgsl", BROKEN.as_bytes());
        // With hot reload on, a broken file gets a stand-in instead.
        Game::new("errors", 1, 1).hot_reload(false).run_headless(0, move |ctx| {
            let err = ctx.gfx.try_create_shader("broken", BROKEN).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Shader);
            assert!(err.message().starts_with("broken:1:"), "{err}");
            let err = ctx.gfx.try_load_shader(&path).unwrap_err();
            assert_eq!((err.kind(), err.path()), (ErrorKind::Shader, Some(path.as_str())));
            let err = ctx
                .gfx
                .try_create_effect("fx", "fn effect(p: Pixel) -> f32 { return 1; }")
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Shader);
            let err = ctx
                .gfx
                .try_create_compute("c", "fn compute(c: Cell) -> f32 { return 1; }")
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Shader);
        });
    }

    #[cfg(feature = "window")]
    #[test]
    fn a_shader_that_does_not_compile_panics_with_one_prefix() {
        in_tick(|ctx| {
            let err = ctx.gfx.try_create_shader("broken", BROKEN).unwrap_err();
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ctx.gfx.create_shader("broken", BROKEN);
            }))
            .unwrap_err();
            let message = panic.downcast_ref::<String>().unwrap();
            assert_eq!(message, &err.to_string());
            assert!(!message.starts_with("ponko: ponko:"), "{message}");
        });
    }

    #[test]
    fn mesh_data_and_clips_report_missing_and_bad_files() {
        let bad = temp_file("bad-data.glb", b"not a model");
        in_tick(move |ctx| {
            let missing = "no/such.glb";
            let err = ctx.gfx.try_load_mesh_data(missing).unwrap_err();
            assert_eq!((err.kind(), err.path()), (ErrorKind::NotFound, Some(missing)));
            let err = ctx.gfx.try_load_mesh_data(&bad).unwrap_err();
            assert_eq!((err.kind(), err.path()), (ErrorKind::Decode, Some(bad.as_str())));
            assert_eq!(ctx.gfx.try_load_clips(missing).unwrap_err().kind(), ErrorKind::NotFound);
            assert_eq!(ctx.gfx.try_load_clips(&bad).unwrap_err().kind(), ErrorKind::Decode);
            assert_eq!(
                ctx.gfx.try_load_clip(&bad, "walk").unwrap_err().kind(),
                ErrorKind::Decode
            );
        });
    }

    #[test]
    #[should_panic(expected = "ponko: \"no/such.glb\" not found in assets/")]
    fn loading_missing_mesh_data_panics_once_prefixed() {
        in_tick(|ctx| {
            ctx.gfx.load_mesh_data("no/such.glb");
        });
    }

    #[test]
    fn render_targets_check_their_size() {
        in_tick(|ctx| {
            let err = ctx.gfx.try_create_render_target(0, 16).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
            assert!(ctx.gfx.try_create_render_target(16, 16).is_ok());
            let err = ctx.gfx.try_create_ui_panel(16, 16, vec2(-1.0, 1.0)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
            let err = ctx.gfx.try_create_ui_panel(0, 16, vec2(1.0, 1.0)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
        });
    }

    #[test]
    fn texture_arrays_report_missing_bad_and_mismatched_layers() {
        let bad = temp_file("bad-layer.png", b"not a png");
        in_tick(move |ctx| {
            let err = ctx
                .gfx
                .try_create_texture_array(2, 2, &[&[0; 3]], TextureKind::Color)
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
            let err = ctx.gfx.try_load_texture_array(&[], TextureKind::Color).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
            let err = ctx
                .gfx
                .try_load_texture_array(&["no/such.png"], TextureKind::Color)
                .unwrap_err();
            assert_eq!((err.kind(), err.path()), (ErrorKind::NotFound, Some("no/such.png")));
            let err = ctx.gfx.try_load_texture_array(&[&bad], TextureKind::Color).unwrap_err();
            assert_eq!((err.kind(), err.path()), (ErrorKind::Decode, Some(bad.as_str())));
        });
    }

    #[test]
    fn decal_textures_and_terrain_materials_check_their_arrays() {
        let other = std::rc::Rc::new(std::cell::Cell::new(None));
        let out = other.clone();
        in_tick(move |ctx| {
            for _ in 0..3 {
                out.set(Some(ctx.gfx.create_texture_array(1, 1, &[&[0; 4]], TextureKind::Data)));
            }
        });
        let foreign = other.get().unwrap();
        in_tick(move |ctx| {
            let err = ctx.gfx.try_set_decal_textures(Some(foreign)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::BadHandle);
            let color = ctx.gfx.create_texture_array(1, 1, &[&[0; 4]], TextureKind::Color);
            let err = ctx.gfx.try_set_decal_textures(Some(color)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
            let weights = ctx.gfx.create_texture_array(1, 1, &[&[0; 4]], TextureKind::Data);
            let desc = TerrainMaterialDesc::new(color, weights, Vec2::ZERO, Vec2::ZERO);
            let err = ctx.gfx.try_create_terrain_material(&desc).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
            let desc = TerrainMaterialDesc::new(color, foreign, Vec2::ZERO, vec2(10.0, 10.0));
            let err = ctx.gfx.try_create_terrain_material(&desc).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::BadHandle);
        });
    }

    #[test]
    fn reflection_probes_need_a_probe_entity() {
        in_tick(|ctx| {
            let plain = ctx.world.spawn((ponko::d3::Transform::default(),));
            let err = ctx.gfx.bake_reflection_probe(&mut ctx.world, plain).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::NotFound);
        });
    }

    #[test]
    fn mesh_data_checks_are_invalid() {
        let mut broken = MeshData::cube(1.0);
        broken.indices.pop();
        assert_eq!(broken.check().unwrap_err().kind(), ErrorKind::Invalid);
        assert!(MeshData::cube(1.0).check().is_ok());
    }
}
