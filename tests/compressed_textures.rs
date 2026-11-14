//! Block-compressed textures (feature `compressed-textures`) through the
//! public loaders, headless: KTX2 files load as textures of their size,
//! cost their compressed bytes, stream through their own mip levels, fill
//! texture arrays and hot reload. The files in `tests/fixtures/textures`
//! are made by `ponko textures` from the PNGs next to them.
#![cfg(feature = "compressed-textures")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ponko::inventory::AssetClass;
use ponko::prelude::*;
use ponko::streaming::{AssetState, ResidencyKind};

const CARD_PNG: &[u8] = include_bytes!("fixtures/textures/card.png");
/// BC7 sRGB, 64 x 64, seven mip levels.
const CARD_BC7: &[u8] = include_bytes!("fixtures/textures/card.ktx2");
/// BC5 normal map, 64 x 64, seven mip levels.
const BUMPS_BC5: &[u8] = include_bytes!("fixtures/textures/bumps.ktx2");
/// Basis Universal UASTC, 64 x 64, seven mip levels.
const CARD_UASTC: &[u8] = include_bytes!("fixtures/textures/card_uastc.ktx2");

/// BC7's bytes for a 64 x 64 texture and its mip chain: one 16-byte block
/// per 4 x 4 pixels, a whole block for the levels under 4 x 4.
const BC7_64_WITH_MIPS: u64 = (256 + 64 + 16 + 4 + 1 + 1 + 1) * 16;

fn textures() -> MemorySource {
    MemorySource::new("fixtures")
        .with("card.png", CARD_PNG)
        .with("card.ktx2", CARD_BC7)
        .with("bumps.ktx2", BUMPS_BC5)
        .with("card_uastc.ktx2", CARD_UASTC)
}

#[test]
fn ktx2_textures_load_at_their_size_and_cost_their_compressed_bytes() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets.add_source(textures());
        let png = ctx.gfx.load_texture("card.png");
        let bc7 = ctx.gfx.load_texture("card.ktx2");
        assert_eq!((bc7.width(), bc7.height()), (64, 64));
        assert_eq!((png.width(), png.height()), (64, 64));
        let inventory = ctx.asset_inventory();
        let textures = inventory.get(AssetClass::Texture);
        assert_eq!(textures.live, 2);
        // The PNG: 64 x 64 RGBA8 without mipmaps; the KTX2: its blocks.
        assert_eq!(textures.bytes, 64 * 64 * 4 + BC7_64_WITH_MIPS);
        // Loading the same file again gives the same texture.
        assert_eq!(ctx.gfx.load_texture("card.ktx2"), bc7);
    });
}

#[test]
fn freeing_a_compressed_texture_returns_its_bytes() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets.add_source(textures());
        let normal = ctx.gfx.load_texture("bumps.ktx2");
        assert_eq!(ctx.asset_inventory().get(AssetClass::Texture).bytes, BC7_64_WITH_MIPS);
        ctx.free(normal);
        assert_eq!(ctx.asset_inventory().get(AssetClass::Texture).bytes, 0);
    });
}

#[test]
fn updating_a_compressed_texture_turns_it_into_rgba8() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets.add_source(textures());
        let card = ctx.gfx.load_texture("card.ktx2");
        ctx.gfx.update_texture(card, &vec![255; 64 * 64 * 4]);
        assert_eq!(ctx.asset_inventory().get(AssetClass::Texture).bytes, 64 * 64 * 4);
    });
}

#[test]
fn broken_ktx2_files_are_decode_errors_naming_the_file() {
    let mut cut = CARD_BC7.to_vec();
    cut.truncate(200);
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("broken").with("cut.ktx2", cut.clone()));
        let err = ctx.gfx.try_load_texture("cut.ktx2").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Decode);
        assert!(err.to_string().contains("cut.ktx2"), "{err}");
    });
}

#[test]
fn uastc_textures_need_the_basis_universal_transcoder() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets.add_source(textures());
        let loaded = ctx.gfx.try_load_texture("card_uastc.ktx2");
        if cfg!(feature = "basis-universal") {
            let texture = loaded.unwrap();
            assert_eq!((texture.width(), texture.height()), (64, 64));
            // Headless, nothing is transcoded for a GPU: the UASTC blocks
            // count as the BC7 they would become (also 16 bytes a block).
            assert_eq!(ctx.asset_inventory().get(AssetClass::Texture).bytes, BC7_64_WITH_MIPS);
        } else {
            let err = loaded.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Unsupported);
            assert!(err.message().contains("basis-universal"), "{err}");
        }
    });
}

#[test]
fn streamed_compressed_textures_climb_their_own_mip_levels() {
    Game::new("t", 1, 1).run_headless(3, |ctx| {
        ctx.assets.add_source(textures());
        ctx.gfx.set_deterministic_uploads(true);
        assert_eq!(ctx.gfx.stream_texture("card.ktx2"), None);
        ctx.finish_uploads();
        let card = ctx.gfx.stream_texture("card.ktx2").expect("streamed in");
        assert_eq!((card.width(), card.height()), (64, 64));
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "card.ktx2"),
            AssetState::Resident
        );
        // Resident at full size: its file's blocks, the whole chain.
        let streamed = ctx.gfx.streamed_assets();
        assert_eq!(streamed[0].size, [64, 64]);
        assert_eq!(
            ctx.gfx.residency(ResidencyKind::Texture).resident_bytes,
            BC7_64_WITH_MIPS
        );
    });
}

#[test]
fn a_residency_budget_holds_a_compressed_texture_at_a_smaller_mip_level() {
    Game::new("t", 1, 1).run_headless(3, |ctx| {
        ctx.assets.add_source(textures());
        ctx.gfx.set_deterministic_uploads(true);
        // Room for the 32 x 32 level and its chain (1376 bytes), not 64 x 64.
        ctx.gfx.set_residency_budget(ResidencyKind::Texture, Some(2048));
        ctx.gfx.stream_texture("card.ktx2");
        ctx.finish_uploads();
        let streamed = ctx.gfx.streamed_assets();
        assert_eq!(streamed[0].size, [32, 32]);
        assert_eq!(
            ctx.gfx.residency(ResidencyKind::Texture).resident_bytes,
            (64 + 16 + 4 + 1 + 1 + 1) * 16
        );
    });
}

#[cfg(feature = "3d")]
#[test]
fn texture_arrays_take_compressed_layers() {
    use ponko::d3::TextureKind;
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets.add_source(textures());
        let array = ctx
            .gfx
            .load_texture_array(&["card.ktx2", "card.ktx2"], TextureKind::Color);
        assert_eq!((array.width(), array.height(), array.layers()), (64, 64, 2));
        // Mixed with an image file, every layer loads as RGBA8.
        let mixed = ctx
            .gfx
            .load_texture_array(&["card.png", "card.ktx2"], TextureKind::Color);
        assert_eq!(mixed.layers(), 2);
        // Different sizes are refused, as for image files.
        ctx.assets
            .add_source(MemorySource::new("small").with("small.png", small_png()));
        let err = ctx
            .gfx
            .try_load_texture_array(&["card.ktx2", "small.png"], TextureKind::Color)
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Invalid);
    });
}

#[cfg(feature = "3d")]
fn small_png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-compressed-hot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs a headless game that loads `path` and writes `edit` over it on
/// tick 2; returns the ticks it was reloaded on and the texture bytes after.
fn reload(path: &Path, edit: Vec<u8>) -> (Vec<u64>, u64) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let bytes = Arc::new(Mutex::new(0));
    let (log, out) = (seen.clone(), bytes.clone());
    let name = path.to_str().unwrap().to_string();
    let file = path.to_path_buf();
    Game::new("hot", 1, 1)
        .hot_reload(true)
        .sim_system(move |ctx: &mut Ctx| {
            if !ctx.assets.reloaded_textures().is_empty() {
                log.lock().unwrap().push(ctx.time.sim_tick());
            }
            if ctx.time.sim_tick() == 2 {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::write(&file, &edit).unwrap();
                std::thread::sleep(Duration::from_millis(400));
            }
            *out.lock().unwrap() = ctx.asset_inventory().get(AssetClass::Texture).bytes;
        })
        .run_headless(5, |ctx| {
            ctx.load_texture(&name);
        });
    let seen = seen.lock().unwrap().clone();
    let bytes = *bytes.lock().unwrap();
    (seen, bytes)
}

#[test]
fn edited_ktx2_files_reload_under_the_same_handle() {
    let path = temp_dir().join("card.ktx2");
    std::fs::write(&path, CARD_BC7).unwrap();
    let (seen, bytes) = reload(&path, BUMPS_BC5.to_vec());
    assert_eq!(seen, vec![3], "reloaded once, on the tick after the edit");
    assert_eq!(bytes, BC7_64_WITH_MIPS);
}

#[test]
fn a_png_replaced_by_its_ktx2_reload_keeps_working() {
    // The same name, now holding a compressed file: still one handle.
    let path = temp_dir().join("swap.png");
    std::fs::write(&path, CARD_PNG).unwrap();
    let (seen, bytes) = reload(&path, CARD_BC7.to_vec());
    assert_eq!(seen, vec![3]);
    assert_eq!(bytes, BC7_64_WITH_MIPS);
}
