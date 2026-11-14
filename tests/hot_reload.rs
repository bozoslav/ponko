//! Asset hot reload: edited image files come back under the same handle.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ponko::prelude::*;

fn write_png(path: &Path, w: u32, h: u32, rgba: [u8; 4]) {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
    img.save(path).unwrap();
}

fn temp_png(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-hot-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    write_png(&path, 4, 4, [255, 0, 0, 255]);
    path
}

/// Runs a headless game that loads `path`, calls `edit` on tick 2 (then
/// waits past the next check), and returns the texture and what was
/// reloaded on each tick.
fn run(path: &Path, hot_reload: bool, edit: impl Fn() + 'static) -> (Texture, Vec<(u64, Vec<Texture>)>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let mut tex = None;
    let name = path.to_str().unwrap().to_string();
    Game::new("hot", 1, 1)
        .hot_reload(hot_reload)
        .sim_system(move |ctx: &mut Ctx| {
            let reloaded = ctx.assets.reloaded_textures().to_vec();
            if !reloaded.is_empty() {
                log.lock().unwrap().push((ctx.time.sim_tick(), reloaded));
            }
            if ctx.time.sim_tick() == 2 {
                // Windows file times move in ~16 ms steps, and headless ticks
                // are much faster: without this, an edit of the same length can
                // land on the same time as the first write and look unchanged.
                std::thread::sleep(Duration::from_millis(50));
                edit();
                std::thread::sleep(Duration::from_millis(400));
            }
        })
        .run_headless(5, |ctx| tex = Some(ctx.gfx.load_texture(&name)));
    let seen = seen.lock().unwrap().clone();
    (tex.unwrap(), seen)
}

#[test]
fn edited_images_reload_under_the_same_handle() {
    let path = temp_png("crab.png");
    let p = path.clone();
    let (tex, seen) = run(&path, true, move || write_png(&p, 4, 4, [0, 0, 255, 255]));
    assert_eq!(seen, vec![(3, vec![tex])], "reloaded once, on the tick after the edit");
}

#[test]
fn images_that_change_size_are_not_reloaded() {
    let path = temp_png("bigger.png");
    let p = path.clone();
    let (tex, seen) = run(&path, true, move || write_png(&p, 8, 8, [0, 0, 255, 255]));
    assert_eq!((tex.width(), tex.height()), (4, 4));
    assert!(
        seen.is_empty(),
        "sprites keep the old size, so nothing changes: {seen:?}"
    );
}

#[test]
fn nothing_reloads_with_hot_reload_off() {
    let path = temp_png("off.png");
    let p = path.clone();
    let (_, seen) = run(&path, false, move || write_png(&p, 4, 4, [0, 0, 255, 255]));
    assert!(seen.is_empty());
}

#[test]
fn a_file_overridden_by_an_asset_source_is_not_reloaded_but_the_folders_copy_is() {
    // The same name from a source and from the folder: editing the file
    // reloads only the folder's texture, loaded before the source was added.
    let path = temp_png("both.png");
    let name = path.to_str().unwrap().to_string();
    let p = path.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let mut loaded = None;
    Game::new("hot", 1, 1)
        .hot_reload(true)
        .sim_system(move |ctx: &mut Ctx| {
            log.lock()
                .unwrap()
                .extend(ctx.assets.reloaded_textures().iter().copied());
            if ctx.time.sim_tick() == 2 {
                std::thread::sleep(Duration::from_millis(50));
                write_png(&p, 4, 4, [0, 0, 255, 255]);
                std::thread::sleep(Duration::from_millis(400));
            }
        })
        .run_headless(5, |ctx| {
            let from_folder = ctx.gfx.load_texture(&name);
            let mut png = Vec::new();
            image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255]))
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            ctx.assets.add_source(MemorySource::new("pack").with(name.clone(), png));
            let from_source = ctx.gfx.load_texture(&name);
            loaded = Some((from_folder, from_source));
        });
    let (from_folder, from_source) = loaded.unwrap();
    assert_ne!(from_folder, from_source);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![from_folder],
        "only the folder's copy reloads"
    );
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct Rules {
    speed: f32,
    damage: f32,
}

fn temp_json(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-data-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// Loads `path` as `Rules`, rewrites it to `edit` on tick 2, and returns
/// the speed and reloaded file names seen on each tick.
fn run_rules(path: &Path, edit: &'static str) -> Vec<(f32, Vec<String>)> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let name = path.to_str().unwrap().to_string();
    let file = path.to_path_buf();
    Game::new("data", 1, 1)
        .hot_reload(true)
        .sim_system(move |ctx: &mut Ctx| {
            let speed = ctx.world.resource::<Rules>().speed;
            log.lock().unwrap().push((speed, ctx.assets.reloaded_files().to_vec()));
            if ctx.time.sim_tick() == 2 {
                std::thread::sleep(Duration::from_millis(50)); // see `run`
                std::fs::write(&file, edit).unwrap();
                std::thread::sleep(Duration::from_millis(400));
            }
        })
        .run_headless(5, |ctx| ctx.assets.load_resource::<Rules>(&mut ctx.world, &name));
    seen.lock().unwrap().clone()
}

#[test]
fn edited_data_files_replace_their_resource() {
    let path = temp_json("rules.json", r#"{"speed": 30, "damage": 2}"#);
    let seen = run_rules(&path, r#"{"speed": 45, "damage": 2}"#);
    let speeds: Vec<f32> = seen.iter().map(|s| s.0).collect();
    assert_eq!(speeds, [30.0, 30.0, 30.0, 45.0, 45.0]);
    let name = path.to_str().unwrap().to_string();
    assert_eq!(seen[3].1, [name], "listed as reloaded on that tick only");
    assert!(seen[4].1.is_empty());
}

#[test]
fn data_files_that_dont_parse_keep_the_old_values() {
    let path = temp_json("broken.json", r#"{"speed": 30, "damage": 2}"#);
    let seen = run_rules(&path, r#"{"speed": 45, "dama"#);
    assert!(seen.iter().all(|s| s.0 == 30.0 && s.1.is_empty()));
}

#[test]
#[should_panic(expected = "line 1 column")]
fn bad_data_files_say_where_at_load() {
    let path = temp_json("bad.json", r#"{"speed": "fast", "damage": 2}"#);
    let name = path.to_str().unwrap().to_string();
    Game::new("data", 1, 1).run_headless(0, |ctx| ctx.assets.load_resource::<Rules>(&mut ctx.world, &name));
}

#[test]
#[should_panic(expected = "not found in assets/")]
fn missing_data_files_say_where_they_were_looked_for() {
    Game::new("data", 1, 1).run_headless(0, |ctx| {
        ctx.assets.load_resource::<Rules>(&mut ctx.world, "no-such-rules.json")
    });
}

// ANCHOR: data_files
#[derive(serde::Deserialize)]
struct Tuning {
    speed: f32,
    damage: f32,
}

fn load_tuning(ctx: &mut Ctx, path: &str) {
    ctx.assets.load_resource::<Tuning>(&mut ctx.world, path); // e.g. "tuning.json": {"speed": 30, "damage": 2}
}

fn step_size(ctx: &Ctx) -> f32 {
    // The file's current values, every tick.
    ctx.world.resource::<Tuning>().speed * ctx.time.dt()
}
// ANCHOR_END: data_files

#[test]
fn the_guide_data_file_example_runs() {
    let path = temp_json("tuning.json", r#"{"speed": 30, "damage": 2}"#);
    let name = path.to_str().unwrap().to_string();
    let world = Game::new("data", 1, 1)
        .fixed_hz(10)
        .sim_system(|ctx| {
            let s = step_size(ctx);
            assert!((s - 3.0).abs() < 1e-5 && ctx.world.resource::<Tuning>().damage == 2.0);
        })
        .run_headless(1, |ctx| load_tuning(ctx, &name));
    assert_eq!(world.resource::<Tuning>().speed, 30.0);
}

/// Loads `name` (written as `before`) with `load`, rewrites it as `after`
/// on tick 1, and returns the ticks on which it was listed as reloaded.
fn reload_ticks(name: &str, before: &[u8], after: Vec<u8>, load: fn(&mut Ctx, &str)) -> Vec<u64> {
    let dir = std::env::temp_dir().join(format!("ponko-media-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, before).unwrap();
    let file = path.to_str().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let watched = file.clone();
    Game::new("media", 1, 1)
        .hot_reload(true)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.assets.reloaded_files().contains(&watched) {
                log.lock().unwrap().push(ctx.time.sim_tick());
            }
            if ctx.time.sim_tick() == 1 {
                std::thread::sleep(Duration::from_millis(50)); // see `run`
                std::fs::write(&path, &after).unwrap();
                std::thread::sleep(Duration::from_millis(400));
            }
        })
        .run_headless(4, move |ctx| load(ctx, &file));
    seen.lock().unwrap().clone()
}

fn wav(samples: usize, level: i16) -> Vec<u8> {
    let data_len = (samples * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    for v in [
        16u32.to_le_bytes(),
        [1, 0, 1, 0],
        8000u32.to_le_bytes(),
        16000u32.to_le_bytes(),
        [2, 0, 16, 0],
    ] {
        out.extend_from_slice(&v);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for _ in 0..samples {
        out.extend_from_slice(&level.to_le_bytes());
    }
    out
}

#[test]
fn edited_fonts_reload_and_broken_ones_are_kept() {
    let font = epaint_default_fonts::UBUNTU_LIGHT;
    let load = |ctx: &mut Ctx, f: &str| {
        ctx.gfx.load_font(f);
    };
    assert_eq!(reload_ticks("good.ttf", font, font[..].to_vec(), load), [2]);
    assert_eq!(
        reload_ticks("broken.ttf", font, b"not a font".to_vec(), load),
        Vec::<u64>::new()
    );
}

#[test]
fn edited_sounds_reload() {
    let load = |ctx: &mut Ctx, f: &str| {
        ctx.audio.load(f);
    };
    assert_eq!(reload_ticks("beep.wav", &wav(800, 1000), wav(1600, 2000), load), [2]);
}

/// Hot reload stress: a texture, a sound and (with 3D) a material shader
/// edited again and again reload in place each time, and the asset
/// inventory never grows: no handle, texture memory or decoded audio is
/// left behind by a reload.
#[test]
fn repeated_edits_reload_in_place_without_growth() {
    const EDITS: u64 = 5;
    let dir = std::env::temp_dir().join(format!("ponko-reload-stress-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("stress.png");
    write_png(&image, 8, 8, [255, 0, 0, 255]);
    let sound = dir.join("stress.wav");
    std::fs::write(&sound, wav(800, 1000)).unwrap();
    #[cfg(feature = "3d")]
    let shader = {
        let shader = dir.join("stress.wgsl");
        std::fs::write(&shader, "fn surface(s: Surface) -> Surface { return s; }").unwrap();
        shader
    };
    #[cfg_attr(not(feature = "3d"), allow(unused_mut))]
    let mut names = vec![image.to_str().unwrap().to_string(), sound.to_str().unwrap().to_string()];
    #[cfg(feature = "3d")]
    names.push(shader.to_str().unwrap().to_string());
    let loads = names.clone();
    let reloads = Arc::new(Mutex::new(vec![0u64; names.len()]));
    let counted = reloads.clone();
    let base: Arc<Mutex<Option<ponko::inventory::AssetInventory>>> = Arc::default();
    Game::new("reload stress", 1, 1)
        .hot_reload(true)
        .sim_system(move |ctx: &mut Ctx| {
            let tick = ctx.time.sim_tick();
            {
                let mut reloads = counted.lock().unwrap();
                for (i, name) in names.iter().enumerate() {
                    reloads[i] += u64::from(ctx.assets.reloaded_files().contains(name));
                }
            }
            let now = ctx.asset_inventory();
            let mut base = base.lock().unwrap();
            match &*base {
                None => *base = Some(now),
                Some(base) => {
                    let growth = now.growth_since(base);
                    assert!(growth.is_empty(), "tick {tick}: {growth:?}");
                    assert_eq!(&now, base, "tick {tick}");
                }
            }
            if tick % 2 == 1 && tick < 2 * EDITS {
                let n = tick / 2;
                std::thread::sleep(Duration::from_millis(50)); // see `run`
                let level = [0, 64, 128, 192, 255][n as usize % 5];
                write_png(&image, 8, 8, [level, 0, 255 - level, 255]);
                std::fs::write(&sound, wav(800, 1000 + 100 * n as i16)).unwrap();
                #[cfg(feature = "3d")]
                std::fs::write(
                    &shader,
                    format!("fn surface(s: Surface) -> Surface {{ var o = s; o.roughness = 0.{n}; return o; }}"),
                )
                .unwrap();
                std::thread::sleep(Duration::from_millis(400));
            }
        })
        .run_headless(2 * EDITS + 2, move |ctx| {
            let _ = ctx.gfx.load_texture(&loads[0]);
            let _ = ctx.audio.load(&loads[1]);
            #[cfg(feature = "3d")]
            let _ = ctx.gfx.load_shader(&loads[2]);
        });
    let reloads = reloads.lock().unwrap().clone();
    assert!(reloads.iter().all(|&n| n == EDITS), "reloads per file: {reloads:?}");
}
