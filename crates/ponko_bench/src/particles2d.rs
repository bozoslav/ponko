//! 2D particles without a window (`particles2d/...`): emitters spawn,
//! move and expire particles, which are drawn as sprites, then ordered and
//! batched as the renderer does. The time is that CPU side of one frame
//! (simulating, collecting, ordering, batching), after the emitters have
//! reached their steady particle count.
//!
//! - `100k-10`: ten emitters of about 10,000 living particles each, with
//!   two blend modes; emitters advance on all cores.
//! - `100k-10-z`: the same with every other emitter one layer up, so the
//!   frame's sprites are sorted by draw order.
//! - `100k-1`: one emitter of about 100,000 living particles (one thread).
use std::hint::black_box;
use std::time::{Duration, Instant};

use ponko::d2::{ParticleEmitter, ParticleSettings, ParticleShape, Transform};
use ponko::prelude::*;
use ponko::render::BlendMode;

use crate::{Bench, report};

/// Frames until the pools are full (lifetimes are at most 1.5 s).
const WARMUP: usize = 120;
const FRAMES: usize = 200;
const DT: f32 = 1.0 / 60.0;

/// Every timed frame and the particles drawn in the last one.
struct Times(Vec<Duration>, usize);

/// About `live` particles at once: `live` per second living 0.5–1.5 s.
fn settings(live: usize, seed: u64) -> ParticleSettings {
    ParticleSettings::default()
        .with_rate(live as f32)
        .with_lifetime(0.5, 1.5)
        .with_speed(40.0, 160.0)
        .with_shape(ParticleShape::Cone { angle: 0.6 })
        .with_drag(0.3)
        .with_max_particles(live * 3 / 2)
        .with_seed(seed)
}

pub fn run(b: &mut Bench) {
    for (name, emitters, per, layered) in [
        ("particles2d/100k-10", 10, 10_000, false),
        ("particles2d/100k-10-z", 10, 10_000, true),
        ("particles2d/100k-1", 1, 100_000, false),
    ] {
        if b.skips(name) {
            continue;
        }
        let mut world = Game::new("particles bench", 1280, 720).run_headless(0, move |ctx| {
            for i in 0..emitters {
                let blend = if i % 2 == 0 {
                    BlendMode::Alpha
                } else {
                    BlendMode::Additive
                };
                let s =
                    settings(per, i as u64 + 1)
                        .with_blend(blend)
                        .with_z(if layered { (i % 2) as f32 } else { 0.0 });
                let at = vec2(100.0 + 100.0 * i as f32, 600.0);
                ctx.world.spawn((Transform::at(at), ParticleEmitter::new(s)));
            }
            let mut times = Vec::with_capacity(FRAMES);
            let mut drawn = 0;
            for frame in 0..WARMUP + FRAMES {
                let start = Instant::now();
                let (sprites, batches) = ctx.bench_particle2d_frame(DT);
                let took = start.elapsed();
                black_box(batches);
                if frame >= WARMUP {
                    times.push(took);
                    drawn = sprites;
                }
            }
            ctx.world.insert_resource(Times(times, drawn));
        });
        let Times(mut times, drawn) = world.remove_resource::<Times>().expect("the times");
        let live = emitters * per;
        assert!(
            drawn > live * 9 / 10 && drawn < live * 11 / 10,
            "about {live} particles drawn, got {drawn}"
        );
        eprintln!("{name}: {drawn} particles in the last frame");
        b.report(report::Record::new(name, report::Kind::Steady, WARMUP, &mut times));
    }
}
