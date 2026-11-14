//! Text layout: 400 HUD strings laid out per headless frame with
//! `measure_text_ex` (the layout `draw_text_ex` does before it looks the
//! glyphs up in the atlas; headless runs draw no text), timed inside the
//! frame.
//!
//! - `text/layout-ascii-400`: ASCII strings, the per-character fast path. Run
//!   it with and without the `text-shaping` feature: the feature must not
//!   change its cost (`cargo run --release -p ponko_bench -- text/` and
//!   `cargo run --release -p ponko_bench --features text-shaping -- text/`).
//! - `text/layout-ascii-shaped-400` (`text-shaping`): the same strings with
//!   `TextShaping::Always`, through the shaper's cache.
//! - `text/layout-mixed-400` (`text-shaping`): Latin and Arabic in one line
//!   through a fallback font: bidirectional runs, from the shaper's cache.
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ponko::game::Phase;
use ponko::prelude::{Ctx, Font, Game};

use crate::{Bench, report};

const STRINGS: usize = 400;
const FRAMES: u64 = 120;
const WARMUP: usize = 10;

fn strings(mixed: bool) -> Vec<String> {
    (0..STRINGS)
        .map(|i| {
            if mixed {
                format!("Wave {}: مرحبا بالعالم {}", i % 20, i * 37 % 1000)
            } else {
                format!("Wave {}: score {} / gold {}", i % 20, i * 37 % 1000, i * 11 % 500)
            }
        })
        .collect()
}

pub fn run(b: &mut Bench) {
    let cases: &[(&str, bool, bool)] = &[
        ("text/layout-ascii-400", false, false),
        #[cfg(feature = "text-shaping")]
        ("text/layout-ascii-shaped-400", false, true),
        #[cfg(feature = "text-shaping")]
        ("text/layout-mixed-400", true, false),
    ];
    for &(name, mixed, always) in cases {
        if b.skips(name) {
            continue;
        }
        let times = Rc::new(RefCell::new(Vec::<Duration>::new()));
        let log = times.clone();
        let lines = strings(mixed);
        Game::new("bench", 1280, 720)
            .headless_frames(true)
            .add(Phase::Draw, move |ctx: &mut Ctx| {
                let t = Instant::now();
                let mut width = 0.0;
                for line in &lines {
                    width += ctx.gfx.measure_text_ex(line, Font::DEFAULT, 16.0).x;
                }
                log.borrow_mut().push(t.elapsed());
                std::hint::black_box(width);
            })
            .run_headless(FRAMES + WARMUP as u64, move |ctx| {
                if mixed {
                    let arabic = ctx.gfx.load_font("fonts/NotoSansArabic-Subset.ttf");
                    ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[arabic]);
                }
                #[cfg(feature = "text-shaping")]
                if always {
                    ctx.gfx.set_text_shaping(ponko::prelude::TextShaping::Always);
                }
                #[cfg(not(feature = "text-shaping"))]
                let _ = always;
            });
        let mut times = times.borrow_mut().split_off(WARMUP);
        b.report(report::Record::new(name, report::Kind::Steady, WARMUP, &mut times));
    }
}
