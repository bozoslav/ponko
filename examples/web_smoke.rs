//! The 2D browser smoke-test fixture: a tiny game that `web/smoke/` builds with
//! `web/build.sh web_smoke` and drives in a headless browser.
//!
//! It loads one asset file (`crab.png`, fetched over HTTP on the web) and
//! two assets from an in-memory [asset source](Assets::add_source) (an
//! image and streamed music, never fetched), starts the music (a quiet
//! looping hum) during setup, runs a fixed-step simulation, counts left
//! clicks and draws something on every frame. In a browser the music waits
//! for the first click, as browsers allow sound only after one. Each frame
//! it publishes its state for the runner (see `common/web_smoke.rs`):
//!
//! `{"fixture":"web_smoke","ticks":120,"frames":118,"clicks":1,"size":[800,600],"asset":[32,32],"source":[4,2],"stream":true,"audio_unlocked":true}`
//!
//! Failure cases for the runner's self-tests, chosen with the page's query
//! string: `?smoke_case=panic` panics during setup, `?smoke_case=stall`
//! pauses the simulation after a few ticks. `web_smoke_3d` is the same
//! fixture for the 3D renderer.
//!
//! On the desktop it opens a window like any other example
//! (`cargo run --example web_smoke`) and prints its state once a second.

use ponko::prelude::*;

use serde_json::json;
use std::cell::Cell;
use std::rc::Rc;

#[path = "common/web_smoke.rs"]
mod smoke;

fn main() {
    let ticks = Rc::new(Cell::new(0u64));
    let sim_ticks = ticks.clone();
    Game::new("Ponko: web smoke", 800, 600)
        .fixed_hz(60)
        .sim_system(move |_ctx| sim_ticks.set(sim_ticks.get() + 1))
        .system(frame(ticks))
        .setup(|ctx| {
            let smoke = smoke::Smoke::new("web_smoke");
            // The one fetched asset; a missing file panics with its name.
            let crab = ctx.gfx.load_texture("crab.png");
            // Assets from a source the game adds: found by name, never fetched.
            ctx.assets.add_source(
                MemorySource::new("embedded")
                    .with("embedded/dot.png", png(4, 2))
                    .with("embedded/hum.wav", hum_wav(8000)),
            );
            let dot = ctx.gfx.load_texture("embedded/dot.png");
            let music = ctx.audio.try_load_stream("embedded/hum.wav");
            let stream = music.is_ok();
            // Music asked for before any click: in a browser it starts at
            // the first one (`ctx.audio.unlocked()` turns true then).
            if let Ok(music) = music {
                let quiet_loop = SoundParams {
                    volume: 0.05,
                    looping: true,
                    ..Default::default()
                };
                ctx.audio.play_ex(music, quiet_loop);
            }
            ctx.world.insert_resource(Fixture {
                smoke,
                crab,
                dot,
                stream,
            });
        })
        .run();
}

/// What setup made, for the frame system.
struct Fixture {
    smoke: smoke::Smoke,
    crab: Texture,
    dot: Texture,
    stream: bool,
}

/// The frame system: publishes the state and draws.
fn frame(ticks: Rc<Cell<u64>>) -> impl FnMut(&mut Ctx) {
    move |ctx: &mut Ctx| {
        // Taken out while it runs, as `Smoke::frame` needs all of `ctx`.
        let mut fixture = ctx.world.remove_resource::<Fixture>().expect("made in setup");
        let (crab, dot) = (fixture.crab, fixture.dot);
        fixture.smoke.frame(
            ctx,
            ticks.get(),
            json!({
                "asset": [crab.width(), crab.height()],
                "source": [dot.width(), dot.height()],
                "stream": fixture.stream,
                "audio_unlocked": ctx.audio.unlocked(),
            }),
        );
        // The background changes with each click, so a click shows.
        ctx.gfx.clear(if fixture.smoke.clicks().is_multiple_of(2) {
            Color::hex(0x1E2A38)
        } else {
            Color::hex(0x2E4A28)
        });
        ctx.world.insert_resource(fixture);
        let size = ctx.window_size();
        let t = ticks.get() as f32 / 60.0;
        for i in 0..8 {
            let angle = t + i as f32 * std::f32::consts::TAU / 8.0;
            ctx.gfx.draw_ex(
                crab,
                size / 2.0 + Vec2::from_angle(angle) * size.min_element() * 0.3,
                DrawParams::default().with_scale(Vec2::splat(3.0)),
            );
        }
    }
}

/// A `width` x `height` PNG, made in memory.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(width, height, image::Rgba([255, 200, 0, 255]))
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("encoding a PNG in memory");
    bytes
}

/// A mono 16-bit WAV file of `samples` samples of a 200 Hz hum at 8 kHz
/// (whole periods, so it loops without a click).
fn hum_wav(samples: u32) -> Vec<u8> {
    let data = samples * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    for field in [
        16u32.to_le_bytes(),
        [1, 0, 1, 0],
        8000u32.to_le_bytes(),
        16_000u32.to_le_bytes(),
        [2, 0, 16, 0],
    ] {
        out.extend_from_slice(&field);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..samples {
        let phase = i as f32 * 200.0 / 8000.0 * std::f32::consts::TAU;
        out.extend_from_slice(&((phase.sin() * 8000.0) as i16).to_le_bytes());
    }
    out
}
