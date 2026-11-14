//! 2D lighting on the GPU (`lights2d/...`, feature `render`): 2,000 sprites
//! at 1920 x 1080 drawn offscreen through the window's sprite pass, unlit
//! (the baseline) or lit by moving point lights. The time is the whole
//! frame until the GPU finished it; the lit cases minus `unlit` are what
//! lighting costs. Each run also prints the CPU part (gathering the lights,
//! building shadow volumes, uploading and recording) and the gathering alone.
//!
//! - `unlit`: no lights, no ambient: lighting finds nothing to do (its
//!   gathering is the engine's whole per-frame cost for an unlit game).
//! - `ambient`: an ambient light only (the light map and the composite).
//! - `100`: 100 lights without shadows.
//! - `100-shadows`: 100 lights casting shadows from 200 square occluders.
//! - `100-shadows-normals`: the same with a normal map on every sprite.
use std::time::Duration;

use ponko::render::{Lights2DBench, Lights2DScene};

use crate::{Bench, report};

const WARMUP: usize = 30;
const FRAMES: usize = 300;

fn median(times: &mut [Duration]) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1e3
}

pub fn run(b: &mut Bench) {
    let base = Lights2DScene {
        size: (1920, 1080),
        sprites: 2000,
        lights: 0,
        shadows: false,
        occluders: 0,
        normal_maps: false,
        lit: false,
    };
    let cases = [
        ("lights2d/unlit", base),
        ("lights2d/ambient", Lights2DScene { lit: true, ..base }),
        (
            "lights2d/100",
            Lights2DScene {
                lit: true,
                lights: 100,
                ..base
            },
        ),
        (
            "lights2d/100-shadows",
            Lights2DScene {
                lit: true,
                lights: 100,
                shadows: true,
                occluders: 200,
                ..base
            },
        ),
        (
            "lights2d/100-shadows-normals",
            Lights2DScene {
                lit: true,
                lights: 100,
                shadows: true,
                occluders: 200,
                normal_maps: true,
                ..base
            },
        ),
    ];
    for (name, scene) in cases {
        if b.skips(name) {
            continue;
        }
        let mut bench = match Lights2DBench::new(scene) {
            Ok(bench) => bench,
            Err(reason) => {
                eprintln!("{name}: skipped, {reason}");
                continue;
            }
        };
        for _ in 0..WARMUP {
            bench.frame();
        }
        let (mut total, mut cpu, mut collect) = (Vec::new(), Vec::new(), Vec::new());
        let mut vertices = 0;
        for _ in 0..FRAMES {
            let f = bench.frame();
            total.push(f.total);
            cpu.push(f.cpu);
            collect.push(f.collect);
            vertices = f.shadow_vertices;
        }
        eprintln!(
            "{name} on {}: CPU {:.3} ms (gathering {:.3} ms), {vertices} shadow vertices",
            bench.adapter(),
            median(&mut cpu),
            median(&mut collect),
        );
        b.report(report::Record::new(name, report::Kind::Steady, WARMUP, &mut total));
    }
}
