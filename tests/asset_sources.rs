//! Asset sources: named assets read from somewhere other than `assets/`.

use std::cell::Cell;
use std::rc::Rc;

#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
#[allow(unused_imports)]
use ponko::inventory::AssetClass;
use ponko::prelude::*;

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// The start of a prefab file, up to its components.
const PREFAB_HEADER: &str = r#"{ "format": "ponko-prefab", "version": 1, "#;

fn game() -> Game {
    Game::new("asset sources", 1, 1).hot_reload(false)
}

#[test]
fn a_texture_loads_by_name_from_a_memory_source() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("tiles/red.png", png(3, 2)));
        let red = ctx.gfx.load_texture("tiles/red.png");
        assert_eq!((red.width(), red.height()), (3, 2));
        assert_eq!(
            ctx.gfx.load_texture("tiles/red.png"),
            red,
            "loading again returns the same handle"
        );
    });
}

#[test]
fn a_scene_and_the_texture_it_names_load_from_a_memory_source() {
    // A scene saved by the engine, kept only in memory.
    let mut saved = String::new();
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("red.png", png(2, 2)));
        let red = ctx.gfx.load_texture("red.png");
        let mut sprite = Sprite::new(red);
        sprite.z = 2.0;
        ctx.world.spawn((Transform2D::at(vec2(4.0, 5.0)), sprite));
        saved = serde_json::to_string(&ctx.world.save()).unwrap();
    });
    let source = MemorySource::new("mem")
        .with("red.png", png(2, 2))
        .with("levels/one.json", saved);
    let world = game().run_headless(0, |ctx| {
        ctx.assets.add_source(source);
        ctx.load_scene("levels/one.json").unwrap();
        let sprite = *ctx.world.query::<&Sprite>().iter().next().unwrap();
        assert_eq!(sprite.z, 2.0);
        assert_eq!(
            sprite.texture,
            ctx.gfx.load_texture("red.png"),
            "the scene loaded the source's texture"
        );
        assert_eq!((sprite.texture.width(), sprite.texture.height()), (2, 2));
    });
    assert_eq!(world.len(), 1);
}

#[test]
fn prefabs_and_resources_load_from_a_memory_source() {
    #[derive(serde::Deserialize)]
    struct Rules {
        speed: f32,
    }
    let prefab = PREFAB_HEADER.to_string()
        + r#""components": {
        "Transform2D": { "position": [0.0, 0.0], "rotation": 0.0, "scale": [1.0, 1.0] },
        "Sprite": { "texture": "red.png", "z": 0.0 }
    } }"#;
    game().run_headless(0, |ctx| {
        ctx.assets.add_source(
            MemorySource::new("mem")
                .with("red.png", png(1, 1))
                .with("prefabs/crab.json", prefab)
                .with("rules.json", r#"{"speed": 7}"#),
        );
        let e = ctx.spawn_prefab("prefabs/crab.json", vec2(1.0, 2.0)).unwrap();
        assert_eq!(ctx.world.get::<Transform2D>(e).unwrap().position, vec2(1.0, 2.0));
        ctx.assets.load_resource::<Rules>(&mut ctx.world, "rules.json");
        assert_eq!(ctx.world.resource::<Rules>().speed, 7.0);
    });
}

#[test]
fn a_missing_name_reports_the_sources_and_folders_searched() {
    game().run_headless(0, |ctx| {
        ctx.assets.add_source(MemorySource::new("mem"));
        let err = ctx.load_scene("levels/nowhere.json").unwrap_err();
        assert!(
            err.message().contains("\"levels/nowhere.json\" not found in assets/"),
            "{err}"
        );
        assert!(err.message().contains("Asset sources searched first: \"mem\""), "{err}");
    });
}

#[test]
#[should_panic(expected = "\"broken.png\" (from asset source \"mem\") is not a valid image")]
fn corrupt_bytes_name_the_asset_and_its_source() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("broken.png", b"not a png".to_vec()));
        ctx.gfx.load_texture("broken.png");
    });
}

#[test]
fn a_bad_scene_from_a_source_is_an_error_not_a_panic() {
    game().run_headless(0, |ctx| {
        ctx.assets.add_source(
            MemorySource::new("mem")
                .with("bad.json", "{ not json")
                .with("binary.json", vec![0xff, 0xfe]),
        );
        let err = ctx.load_scene("bad.json").unwrap_err();
        assert!(err.message().starts_with("bad.json:"), "{err}");
        let err = ctx.load_scene("binary.json").unwrap_err();
        assert!(
            err.message().contains("(from asset source \"mem\") is not UTF-8 text"),
            "{err}"
        );
    });
}

/// A source that counts reads, to see what the cache does.
struct Counting {
    id: &'static str,
    reads: Rc<Cell<u32>>,
    bytes: Vec<u8>,
}

impl AssetSource for Counting {
    fn id(&self) -> &str {
        self.id
    }
    fn read(&self, name: &str) -> std::io::Result<Vec<u8>> {
        if name != "shared.png" {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        self.reads.set(self.reads.get() + 1);
        Ok(self.bytes.clone())
    }
}

#[test]
fn repeated_loads_read_a_source_once() {
    let reads = Rc::new(Cell::new(0));
    let counted = reads.clone();
    game().run_headless(0, move |ctx| {
        ctx.assets.add_source(Counting {
            id: "counting",
            reads: counted.clone(),
            bytes: png(1, 1),
        });
        let first = ctx.gfx.load_texture("shared.png");
        for _ in 0..3 {
            assert_eq!(ctx.gfx.load_texture("shared.png"), first);
        }
    });
    assert_eq!(reads.get(), 1);
}

#[test]
fn the_same_name_in_two_sources_is_two_assets_and_the_first_wins() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("first").with("shared.png", png(1, 1)));
        ctx.assets
            .add_source(MemorySource::new("second").with("shared.png", png(2, 2)));
        assert_eq!(
            ctx.gfx.load_texture("shared.png").width(),
            1,
            "the first source added wins"
        );
    });
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("second").with("shared.png", png(2, 2)));
        ctx.assets
            .add_source(MemorySource::new("first").with("shared.png", png(1, 1)));
        assert_eq!(
            ctx.gfx.load_texture("shared.png").width(),
            2,
            "swapped order, the other asset"
        );
    });
}

#[test]
fn a_name_only_in_a_later_source_falls_through_to_it() {
    game().run_headless(0, |ctx| {
        ctx.assets.add_source(MemorySource::new("a"));
        ctx.assets
            .add_source(MemorySource::new("b").with("only-b.png", png(5, 1)));
        assert_eq!(ctx.gfx.load_texture("only-b.png").width(), 5);
    });
}

#[test]
fn sources_are_asked_before_the_assets_folder() {
    // assets/crab.png exists in this repo; a source's own crab.png wins.
    game().run_headless(0, |ctx| {
        let from_folder = ctx.gfx.load_texture("crab.png");
        assert_ne!((from_folder.width(), from_folder.height()), (7, 7));
    });
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("crab.png", png(7, 7)));
        let crab = ctx.gfx.load_texture("crab.png");
        assert_eq!((crab.width(), crab.height()), (7, 7));
    });
}

#[test]
fn a_read_error_other_than_not_found_stops_the_search() {
    struct Broken;
    impl AssetSource for Broken {
        fn id(&self) -> &str {
            "broken"
        }
        fn read(&self, _: &str) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::other("disk on fire"))
        }
    }
    game().run_headless(0, |ctx| {
        ctx.assets.add_source(Broken);
        let err = ctx.load_scene("crab.png").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Io);
        assert_eq!(
            err.message(),
            "asset source \"broken\" could not read \"crab.png\": disk on fire"
        );
    });
}

#[test]
#[should_panic(expected = "an asset source called \"mem\" was already added")]
fn source_ids_must_be_unique() {
    game().run_headless(0, |ctx| {
        ctx.assets.add_source(MemorySource::new("mem"));
        ctx.assets.add_source(MemorySource::new("mem"));
    });
}

#[test]
fn has_source_says_which_ids_are_taken() {
    game().run_headless(0, |ctx| {
        assert!(!ctx.assets.has_source("mem"));
        ctx.assets.add_source(MemorySource::new("mem"));
        assert!(ctx.assets.has_source("mem"));
        assert!(!ctx.assets.has_source("other"), "only the ids added");
    });
}

#[test]
fn hot_reload_skips_assets_from_added_sources() {
    // With hot reload on, a source's texture loads and frames run without
    // anything being watched or reloaded (only `assets/` files are watched).
    let mut tex = None;
    Game::new("asset sources", 1, 1)
        .hot_reload(true)
        .sim_system(|ctx: &mut Ctx| assert!(ctx.assets.reloaded_files().is_empty()))
        .run_headless(5, |ctx| {
            ctx.assets
                .add_source(MemorySource::new("mem").with("red.png", png(1, 1)));
            tex = Some(ctx.gfx.load_texture("red.png"));
        });
    assert_eq!(tex.unwrap().width(), 1);
}

// ANCHOR: embedded
#[derive(serde::Deserialize)]
struct Tuning {
    speed: f32,
}

#[test]
fn embedded_files_load_by_name_like_files_in_assets() {
    game().run_headless(0, |ctx| {
        // Compiled into the executable: nothing to ship next to it.
        ctx.assets.add_source(
            MemorySource::new("embedded")
                .with("sprites/crab.png", include_bytes!("../assets/crab.png"))
                .with("tuning.json", r#"{"speed": 30}"#),
        );

        // From here on, loads by name look there first.
        let crab = ctx.gfx.load_texture("sprites/crab.png");
        ctx.assets.load_resource::<Tuning>(&mut ctx.world, "tuning.json");
        // ANCHOR_END: embedded
        assert!(crab.width() > 0);
        assert_eq!(ctx.world.resource::<Tuning>().speed, 30.0);
    });
}

/// A mono 16-bit PCM WAV file of silence, `samples` long.
fn wav(sample_rate: u32, samples: usize) -> Vec<u8> {
    let data_len = (samples * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(out.len() + data_len as usize, 0);
    out
}

#[test]
fn a_sound_loads_by_name_from_a_memory_source() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("sfx/tone.wav", wav(8000, 4000)));
        let tone = ctx.audio.load("sfx/tone.wav");
        // Without the `audio` feature nothing is decoded and sounds last 0 s.
        if cfg!(feature = "audio") {
            assert!((tone.seconds() - 0.5).abs() < 0.01, "{}", tone.seconds());
        }
        assert_eq!(
            ctx.audio.load("sfx/tone.wav"),
            tone,
            "loading again returns the same handle"
        );
        ctx.audio.play(tone);
    });
}

#[test]
fn repeated_sound_loads_read_a_source_once() {
    struct CountingSounds(Rc<Cell<u32>>);
    impl AssetSource for CountingSounds {
        fn id(&self) -> &str {
            "counting"
        }
        fn read(&self, name: &str) -> std::io::Result<Vec<u8>> {
            if name != "tone.wav" {
                return Err(std::io::ErrorKind::NotFound.into());
            }
            self.0.set(self.0.get() + 1);
            Ok(wav(8000, 800))
        }
    }
    let reads = Rc::new(Cell::new(0));
    let counted = reads.clone();
    game().run_headless(0, move |ctx| {
        ctx.assets.add_source(CountingSounds(counted.clone()));
        let first = ctx.audio.load("tone.wav");
        for _ in 0..3 {
            assert_eq!(ctx.audio.load("tone.wav"), first);
        }
    });
    assert_eq!(reads.get(), 1);
}

#[test]
fn the_same_sound_name_in_two_sources_is_two_sounds_and_the_first_wins() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("first").with("tone.wav", wav(8000, 800)));
        ctx.assets
            .add_source(MemorySource::new("second").with("tone.wav", wav(8000, 8000)));
        let tone = ctx.audio.load("tone.wav");
        if cfg!(feature = "audio") {
            assert!((tone.seconds() - 0.1).abs() < 0.01, "the first source wins");
        }
    });
}

#[test]
fn a_missing_sound_reports_the_sources_and_folders_searched() {
    let caught = std::panic::catch_unwind(|| {
        game().run_headless(0, |ctx| {
            ctx.assets.add_source(MemorySource::new("mem"));
            ctx.audio.load("nowhere.wav");
        });
    })
    .unwrap_err();
    let message = caught
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| caught.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap();
    assert!(message.contains("\"nowhere.wav\" not found in assets/"), "{message}");
    assert!(message.contains("Asset sources searched first: \"mem\""), "{message}");
}

#[cfg(feature = "audio")]
#[test]
#[should_panic(expected = "\"broken.wav\" (from asset source \"mem\")")]
fn corrupt_sound_bytes_name_the_asset_and_its_source() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("broken.wav", b"not audio".to_vec()));
        ctx.audio.load("broken.wav");
    });
}

/// A level in a made-up format: one row of tiles per line, `#` is a wall.
struct Tiles(Vec<Vec<bool>>);

fn decode_tiles(bytes: &[u8]) -> Result<Tiles, Error> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::new(ErrorKind::Decode, e.to_string()))?;
    if let Some(bad) = text.chars().find(|c| !matches!(c, '#' | '.' | '\n')) {
        return Err(Error::new(ErrorKind::Decode, format!("unknown tile {bad:?}")));
    }
    Ok(Tiles(
        text.lines()
            .map(|row| row.chars().map(|c| c == '#').collect())
            .collect(),
    ))
}

#[test]
fn a_custom_asset_decodes_from_a_source_with_a_callback() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("levels/one.tiles", "#.\n##\n"));
        let level = ctx.assets.load_with("levels/one.tiles", decode_tiles).unwrap();
        assert_eq!(level.0, vec![vec![true, false], vec![true, true]]);
    });
}

#[test]
fn a_custom_asset_decode_error_names_the_asset_and_its_source() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("levels/bad.tiles", "#?\n"));
        let err = ctx.assets.load_with("levels/bad.tiles", decode_tiles).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Decode);
        assert_eq!(err.path(), Some("levels/bad.tiles"));
        assert_eq!(
            err.message(),
            "\"levels/bad.tiles\" (from asset source \"mem\") could not be decoded: unknown tile '?'"
        );
        let err = ctx.assets.load_with("levels/none.tiles", decode_tiles).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.message().contains("\"levels/none.tiles\" not found in assets/"),
            "{err}"
        );
        assert!(err.message().contains("Asset sources searched first: \"mem\""), "{err}");
    });
}

#[test]
fn a_custom_asset_can_come_from_the_assets_folder_too() {
    // Without a source named for it, `assets/` is searched as for any load.
    game().run_headless(0, |ctx| {
        let bytes = ctx.assets.load_with("crab.png", |bytes| Ok(bytes.len())).unwrap();
        assert!(bytes > 0);
    });
}

#[test]
fn music_streams_by_name_from_a_memory_source() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("music/theme.wav", wav(8000, 8000)));
        let theme = ctx.audio.load_stream("music/theme.wav");
        if cfg!(feature = "audio") {
            assert!((theme.seconds() - 1.0).abs() < 0.01, "{}", theme.seconds());
        }
        assert_eq!(
            ctx.audio.load_stream("music/theme.wav"),
            theme,
            "loading again returns the same handle"
        );
        assert_ne!(
            ctx.audio.load("music/theme.wav"),
            theme,
            "the same name decoded whole is another sound"
        );
        ctx.audio.play_ex(
            theme,
            SoundParams {
                looping: true,
                ..Default::default()
            },
        );
    });
}

#[test]
fn repeated_stream_loads_read_a_source_once() {
    struct CountingMusic(Rc<Cell<u32>>);
    impl AssetSource for CountingMusic {
        fn id(&self) -> &str {
            "counting"
        }
        fn read(&self, name: &str) -> std::io::Result<Vec<u8>> {
            if name != "theme.wav" {
                return Err(std::io::ErrorKind::NotFound.into());
            }
            self.0.set(self.0.get() + 1);
            Ok(wav(8000, 800))
        }
    }
    let reads = Rc::new(Cell::new(0));
    let counted = reads.clone();
    game().run_headless(0, move |ctx| {
        ctx.assets.add_source(CountingMusic(counted.clone()));
        let first = ctx.audio.load_stream("theme.wav");
        for _ in 0..3 {
            assert_eq!(ctx.audio.load_stream("theme.wav"), first);
        }
    });
    assert_eq!(reads.get(), 1);
}

#[test]
fn the_same_stream_name_in_two_sources_is_two_sounds_and_the_first_wins() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("first").with("theme.wav", wav(8000, 800)));
        ctx.assets
            .add_source(MemorySource::new("second").with("theme.wav", wav(8000, 8000)));
        let theme = ctx.audio.load_stream("theme.wav");
        if cfg!(feature = "audio") {
            assert!((theme.seconds() - 0.1).abs() < 0.01, "the first source wins");
        }
    });
    // The folder's copy and a source's copy are separate sounds.
    let dir = std::env::temp_dir().join(format!("ponko-stream-sources-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("theme.wav");
    std::fs::write(&file, wav(8000, 800)).unwrap();
    let name = file.to_str().unwrap().to_string();
    game().run_headless(0, |ctx| {
        let from_folder = ctx.audio.load_stream(&name);
        ctx.assets
            .add_source(MemorySource::new("pack").with(name.clone(), wav(8000, 800)));
        let from_source = ctx.audio.load_stream(&name);
        assert_ne!(from_source, from_folder, "same name, other source: another sound");
        assert_eq!(ctx.audio.load_stream(&name), from_source);
    });
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stream_errors_are_values_naming_the_asset_and_its_source() {
    game().run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("mem").with("broken.ogg", b"not audio".to_vec()));
        let err = ctx.audio.try_load_stream("nowhere.ogg").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(err.message().contains("Asset sources searched first: \"mem\""), "{err}");
        if cfg!(feature = "audio") {
            let err = ctx.audio.try_load_stream("broken.ogg").unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Decode);
            assert!(
                err.message().contains("\"broken.ogg\" (from asset source \"mem\")"),
                "{err}"
            );
        }
    });
}

/// A reflected component of a game that names a texture by its asset name.
#[derive(Clone, serde::Serialize, serde::Deserialize, Component)]
struct ShopItem {
    icon: String,
    #[serde(default)]
    variants: Vec<String>,
}

fn load_icon(ctx: &mut Ctx, name: &str) -> Result<(), Error> {
    ctx.gfx.try_load_texture(name).map(|_| ())
}

/// A scene with one shop item, saved by the engine.
fn shop_scene(item: ShopItem) -> String {
    let mut saved = String::new();
    game().run_headless(0, |ctx| {
        ctx.world.reflect::<ShopItem>();
        ctx.world.spawn((item.clone(),));
        saved = serde_json::to_string(&ctx.world.save()).unwrap();
    });
    saved
}

#[test]
fn a_scene_loads_the_assets_custom_component_fields_name() {
    let scene = shop_scene(ShopItem {
        icon: "icons/apple.png".into(),
        variants: vec!["icons/green.png".into(), "icons/red.png".into()],
    });
    let reads = Rc::new(Cell::new(Vec::<String>::new()));
    let seen = reads.clone();
    game().run_headless(0, move |ctx| {
        ctx.world.reflect::<ShopItem>();
        ctx.assets.scene_asset_field::<ShopItem>("icon", load_icon);
        ctx.assets.scene_asset_field::<ShopItem>("variants", move |ctx, name| {
            let mut names = seen.take();
            names.push(name.to_string());
            seen.set(names);
            load_icon(ctx, name)
        });
        ctx.assets.add_source(
            MemorySource::new("shop")
                .with("shop.json", scene)
                .with("icons/apple.png", png(3, 3))
                .with("icons/green.png", png(1, 1))
                .with("icons/red.png", png(2, 2)),
        );
        ctx.load_scene("shop.json").unwrap();
        let item = ctx.world.query::<&ShopItem>().iter().next().unwrap().clone();
        assert_eq!(item.icon, "icons/apple.png", "the component keeps the name");
        // Loaded before the entities: the inventory has all three already.
        assert_eq!(ctx.asset_inventory().get(AssetClass::Texture).live, 3);
        assert_eq!(ctx.gfx.load_texture(&item.icon).width(), 3);
    });
    assert_eq!(
        reads.take(),
        ["icons/green.png", "icons/red.png"],
        "every name in a list"
    );
}

#[test]
fn a_custom_asset_field_that_fails_to_load_fails_the_scene_with_its_name() {
    let scene = shop_scene(ShopItem {
        icon: "icons/missing.png".into(),
        variants: Vec::new(),
    });
    game().run_headless(0, |ctx| {
        ctx.world.reflect::<ShopItem>();
        ctx.assets.scene_asset_field::<ShopItem>("icon", load_icon);
        ctx.assets
            .add_source(MemorySource::new("shop").with("shop.json", scene));
        let before = ctx.world.spawn((Transform2D::default(),));
        let err = ctx.load_scene("shop.json").unwrap_err();
        assert!(
            err.message()
                .starts_with("shop.json: ShopItem.icon \"icons/missing.png\": "),
            "{err}"
        );
        assert!(err.message().contains("not found in assets/"), "{err}");
        assert!(ctx.world.is_alive(before), "the world is unchanged");
    });
}

// ANCHOR: portal
/// Levels of a custom format, decoded by the game and kept in a resource.
#[derive(Default)]
struct Levels(std::collections::HashMap<String, Tiles>);

#[derive(Clone, serde::Serialize, serde::Deserialize, Component)]
struct Portal {
    level: String,
}
// ANCHOR_END: portal

#[test]
fn a_custom_asset_field_can_load_a_custom_decoded_asset() {
    let mut scene = String::new();
    game().run_headless(0, |ctx| {
        ctx.world.reflect::<Portal>();
        ctx.world.spawn((Portal {
            level: "levels/two.tiles".into(),
        },));
        scene = serde_json::to_string(&ctx.world.save()).unwrap();
    });
    game().run_headless(0, |ctx| {
        // ANCHOR: scene_asset_field
        ctx.world.reflect::<Portal>();
        ctx.world.insert_resource(Levels::default());
        ctx.assets.scene_asset_field::<Portal>("level", |ctx, name| {
            let tiles = ctx.assets.load_with(name, decode_tiles)?;
            ctx.world.resource_mut::<Levels>().0.insert(name.to_string(), tiles);
            Ok(())
        });
        // ANCHOR_END: scene_asset_field
        ctx.assets.add_source(
            MemorySource::new("mem")
                .with("world.json", scene.clone())
                .with("levels/two.tiles", "##\n"),
        );
        ctx.load_scene("world.json").unwrap();
        assert_eq!(
            ctx.world.resource::<Levels>().0["levels/two.tiles"].0,
            vec![vec![true, true]]
        );
    });
}

#[test]
fn prefabs_load_the_assets_custom_component_fields_name() {
    let prefab = PREFAB_HEADER.to_string() + r#""components": { "ShopItem": { "icon": "icons/pear.png" } } }"#;
    game().run_headless(0, |ctx| {
        ctx.world.reflect::<ShopItem>();
        ctx.assets.scene_asset_field::<ShopItem>("icon", load_icon);
        ctx.assets.add_source(
            MemorySource::new("mem")
                .with("prefabs/pear.json", prefab)
                .with("icons/pear.png", png(2, 1)),
        );
        ctx.spawn_prefab("prefabs/pear.json", Vec2::ZERO).unwrap();
        assert_eq!(ctx.asset_inventory().get(AssetClass::Texture).live, 1);
    });
}
