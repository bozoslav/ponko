//! The rendering baseline: seven offscreen 3D scenes drawn with the real
//! renderer, each frame split into collect, prepare, encode, submit and the
//! GPU's passes, with draw calls, uploads and memory. Needs a GPU and the
//! `render` feature; opens no window:
//!
//! ```sh
//! PONKO_BENCH_JSON=render.json PONKO_RENDER_CSV=render.csv \
//!   cargo run --release -p ponko_bench --features render -- render/scene
//! ```
//!
//! Settings come from the environment (see `RENDER_SCENES.md`):
//! `PONKO_RENDER_FRAMES`, `PONKO_RENDER_WARMUP`, `PONKO_RENDER_SIZE`,
//! `PONKO_RENDER_SAMPLES`, `PONKO_RENDER_QUALITY`, `PONKO_RENDER_SCALE`,
//! `PONKO_RENDER_MATCHED`, `PONKO_RENDER_TAA`,
//! and the outputs `PONKO_RENDER_CSV`, `PONKO_RENDER_FRAMES_CSV` and
//! `PONKO_RENDER_PNG`. `PONKO_GPU_TIER` opens the device as a weaker GPU.

use std::time::Duration;

use ponko::render::{GraphicsQuality, SceneBench, SceneFrame, SceneKind, SceneSetup};

use crate::render_report::{Dist, FrameRow, GpuTimes, RenderReport, SceneReport};
use crate::{Bench, alloc, report};

/// What a run is asked to do, from the environment.
struct Settings {
    frames: usize,
    warmup: usize,
    setup: SceneSetup,
    quality: String,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Reads the settings; `Err` names the variable that doesn't parse.
fn settings() -> Result<Settings, String> {
    let number = |name: &str, default: usize| match env(name) {
        None => Ok(default),
        Some(v) => v
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("{name}={v} is not a whole number")),
    };
    let mut setup = SceneSetup::default();
    if let Some(size) = env("PONKO_RENDER_SIZE") {
        let parsed = size
            .trim()
            .split_once(['x', 'X'])
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
        setup.size = parsed.ok_or_else(|| format!("PONKO_RENDER_SIZE={size} is not WIDTHxHEIGHT, as in 1280x720"))?;
    }
    setup.samples = number("PONKO_RENDER_SAMPLES", setup.samples as usize)? as u32;
    if ![1, 2, 4, 8].contains(&setup.samples) {
        return Err(format!("PONKO_RENDER_SAMPLES={} is not 1, 2, 4 or 8", setup.samples));
    }
    if let Some(scale) = env("PONKO_RENDER_SCALE") {
        setup.scale = scale
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|s| s.is_finite() && *s > 0.0)
            .ok_or_else(|| format!("PONKO_RENDER_SCALE={scale} is not a positive number"))?;
    }
    let quality = match env("PONKO_RENDER_QUALITY").as_deref().map(str::trim) {
        None | Some("high") => (GraphicsQuality::High, "high"),
        Some("low") => (GraphicsQuality::Low, "low"),
        Some("medium") => (GraphicsQuality::Medium, "medium"),
        Some("ultra") => (GraphicsQuality::Ultra, "ultra"),
        Some(other) => {
            return Err(format!(
                "PONKO_RENDER_QUALITY={other} is not low, medium, high or ultra"
            ));
        }
    };
    setup.quality = quality.0;
    setup.matched = match env("PONKO_RENDER_MATCHED").as_deref().map(str::trim) {
        None | Some("0") => false,
        Some("1") => true,
        Some(other) => return Err(format!("PONKO_RENDER_MATCHED={other} is not 0 or 1")),
    };
    let frames = number("PONKO_RENDER_FRAMES", 600)?;
    if frames == 0 {
        return Err("PONKO_RENDER_FRAMES=0 measures nothing".to_owned());
    }
    Ok(Settings {
        frames,
        warmup: number("PONKO_RENDER_WARMUP", 120)?,
        setup,
        quality: if setup.matched {
            format!("{} (matched)", quality.1)
        } else {
            quality.1.to_owned()
        },
    })
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn row(f: &SceneFrame) -> FrameRow {
    FrameRow {
        update_ms: ms(f.update),
        collect_ms: ms(f.collect),
        prepare_ms: ms(f.prepare),
        encode_ms: ms(f.encode),
        submit_ms: ms(f.submit),
        frame_ms: ms(f.frame),
        gpu_ms: f.gpu_ms,
        gpu_span_ms: f.gpu_span_ms,
        draw_calls: f.draw_calls as u64,
        recorded_draws: u64::from(f.recorded_draws),
        instances: f.instances as u64,
        triangles: f.triangles as u64,
        shadow_instances: f.shadow_instances as u64,
        shadow_triangles: f.shadow_triangles as u64,
        occluded: f.occluded as u64,
        upload_bytes: f.uploads.bytes(),
        buffers_created: u64::from(f.uploads.buffers_created),
        pipelines_built: f.pipelines_built as u64,
    }
}

/// The process's largest resident set so far, or why it can't be read.
fn peak_rss() -> Result<u64, String> {
    #[cfg(unix)]
    {
        // SAFETY: `getrusage` fills the zeroed struct it is given.
        let usage = unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
                return Err("getrusage failed".to_owned());
            }
            usage
        };
        // Bytes on macOS, kilobytes elsewhere.
        let unit = if cfg!(target_os = "macos") { 1 } else { 1024 };
        Ok(usage.ru_maxrss as u64 * unit)
    }
    #[cfg(not(unix))]
    {
        Err("the peak resident set is not read on this platform".to_owned())
    }
}

/// Draws `kind` and gathers its report. `Err` without a GPU.
fn measure(kind: SceneKind, settings: &Settings) -> Result<(SceneReport, SceneBench), String> {
    let mut bench = SceneBench::new(kind, settings.setup)?;
    for _ in 0..settings.warmup {
        bench.frame();
    }
    let mut frames = Vec::with_capacity(settings.frames);
    let mut passes: Vec<(&'static str, Vec<f64>)> = Vec::new();
    let mut uploads = [0u64; 10];
    let mut reallocations = 0u64;
    let mut last = SceneFrame::default();
    let before = alloc::Counts::now();
    for _ in 0..settings.frames {
        let frame = bench.frame();
        if frame.gpu_timed {
            for &(name, pass_ms) in bench.gpu_passes() {
                match passes.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, samples)) => samples.push(pass_ms),
                    None => {
                        let mut samples = Vec::with_capacity(settings.frames);
                        samples.push(pass_ms);
                        passes.push((name, samples));
                    }
                }
            }
        }
        let u = &frame.uploads;
        let kinds = [
            u.instances,
            u.sprites,
            u.static_batches,
            u.uniforms,
            u.lights,
            u.materials,
            u.skins,
            u.meshes,
            u.textures,
            u.other,
        ];
        for (total, bytes) in uploads.iter_mut().zip(kinds) {
            *total += bytes;
        }
        reallocations += u64::from(u.reallocations);
        frames.push(row(&frame));
        last = frame;
    }
    // The rows above are reserved up front, so this counts the frames' own.
    let allocs = alloc::Counts::now().since(before);
    let count = settings.frames as f64;
    let timed: Vec<&FrameRow> = frames.iter().filter(|f| f.gpu_ms.is_some()).collect();
    let gpu = GpuTimes {
        timed_frames: timed.len(),
        total: Dist::of(&mut timed.iter().filter_map(|f| f.gpu_ms).collect::<Vec<_>>()),
        span: Dist::of(&mut timed.iter().filter_map(|f| f.gpu_span_ms).collect::<Vec<_>>()),
        passes: passes
            .iter_mut()
            .filter_map(|(name, samples)| Some(((*name).to_owned(), Dist::of(samples)?)))
            .collect(),
        unavailable: bench.gpu_timing_unavailable().or_else(|| {
            timed
                .is_empty()
                .then(|| "the device has timestamp queries, but no frame's results came back".to_owned())
        }),
    };
    let names = [
        "instances",
        "sprites",
        "static_batches",
        "uniforms",
        "lights",
        "materials",
        "skins",
        "meshes",
        "textures",
        "other",
    ];
    let b = bench.instance_batches();
    let scene = SceneReport {
        name: kind.name().to_owned(),
        scene_json: bench.scene_json(),
        memory_json: bench.memory_json(),
        warmup: settings.warmup,
        frames,
        gpu,
        uploads_per_frame: names
            .into_iter()
            .zip(uploads.map(|bytes| bytes as f64 / count))
            .collect(),
        reallocations,
        instance_batches: vec![
            ("batches", u64::from(b.batches)),
            ("instances", b.instances),
            ("gpu_driven", u64::from(b.gpu_driven)),
            ("gpu_instances", b.gpu_instances),
            ("cpu_small", u64::from(b.cpu_small)),
            ("cpu_no_compute", u64::from(b.cpu_no_compute)),
            ("cpu_over_limit", u64::from(b.cpu_over_limit)),
            ("dropped", u64::from(b.dropped)),
            ("cull_dispatches", u64::from(b.cull_dispatches)),
            ("cull_jobs", u64::from(b.cull_jobs)),
            ("views_skipped", u64::from(b.views_skipped)),
        ],
        state_changes: last.state_changes.map(u64::from),
        command_buffers: last.command_buffers as u64,
        allocs_per_frame: allocs.count as f64 / count,
        alloc_bytes_per_frame: allocs.bytes as f64 / count,
    };
    Ok((scene, bench))
}

/// Writes the frame last drawn as `<dir>/<scene>.png`.
fn save_png(dir: &std::path::Path, kind: SceneKind, bench: &SceneBench, size: (u32, u32)) {
    let path = dir.join(format!("{}.png", kind.name()));
    let saved = std::fs::create_dir_all(dir).map_err(|e| e.to_string()).and_then(|()| {
        image::save_buffer(&path, &bench.pixels(), size.0, size.1, image::ColorType::Rgba8).map_err(|e| e.to_string())
    });
    match saved {
        Ok(()) => println!("    picture written to {}", path.display()),
        Err(e) => eprintln!("could not write {}: {e}", path.display()),
    }
}

fn write(path: &std::ffi::OsStr, what: &str, text: String) {
    match std::fs::write(path, text) {
        Ok(()) => println!("{what} written to {}", path.to_string_lossy()),
        Err(e) => {
            eprintln!("could not write {}: {e}", path.to_string_lossy());
            std::process::exit(1);
        }
    }
}

/// Runs the scenes the filter keeps. Returns the `render` section for the
/// JSON document, if any scene ran.
pub fn run(b: &mut Bench) -> Option<String> {
    let kinds: Vec<SceneKind> = SceneKind::ALL
        .into_iter()
        .filter(|k| !b.skips(&format!("render/scene {}", k.name())))
        .collect();
    if kinds.is_empty() {
        return None;
    }
    let settings = match settings() {
        Ok(settings) => settings,
        Err(problem) => {
            eprintln!("render/scene: {problem}");
            std::process::exit(2);
        }
    };
    let mut report = RenderReport {
        frames: settings.frames,
        warmup: settings.warmup,
        size: settings.setup.size,
        samples: settings.setup.samples,
        quality: settings.quality.clone(),
        scale: settings.setup.scale,
        ..Default::default()
    };
    let png = std::env::var_os("PONKO_RENDER_PNG").map(std::path::PathBuf::from);
    for kind in kinds {
        let (scene, bench) = match measure(kind, &settings) {
            Ok(measured) => measured,
            Err(reason) => {
                println!("{:<32} skipped: {reason}", format!("render/scene {}", kind.name()));
                continue;
            }
        };
        if report.gpu_json.is_empty() {
            report.gpu_json = bench.gpu_json();
            println!(
                "render/scene: {} x {}, {}x MSAA asked for, {} quality, scale {}, {} warm-up + {} measured frames, on {}",
                settings.setup.size.0,
                settings.setup.size.1,
                settings.setup.samples,
                settings.quality,
                settings.setup.scale,
                settings.warmup,
                settings.frames,
                bench.adapter()
            );
            if let Some(reason) = &scene.gpu.unavailable {
                println!("    GPU pass times unavailable: {reason}");
            }
        }
        // The same record format as every other benchmark, one per step.
        for step in ["collect", "prepare", "encode", "submit", "cpu", "frame"] {
            let name = format!("render/scene {} {step}", kind.name());
            let mut times: Vec<Duration> = scene
                .frames
                .iter()
                .map(|f| {
                    Duration::from_secs_f64(
                        match step {
                            "collect" => f.collect_ms,
                            "prepare" => f.prepare_ms,
                            "encode" => f.encode_ms,
                            "submit" => f.submit_ms,
                            "cpu" => f.cpu_ms(),
                            _ => f.frame_ms,
                        } / 1e3,
                    )
                })
                .collect();
            let mut record = report::Record::new(&name, report::Kind::Steady, settings.warmup, &mut times);
            if step == "frame" {
                record.allocs_per_run = Some(scene.allocs_per_frame);
                record.alloc_bytes_per_run = Some(scene.alloc_bytes_per_frame);
            }
            b.report(record);
        }
        let last = scene.frames.last().copied().unwrap_or_default();
        match &scene.gpu.total {
            Some(gpu) => println!(
                "    GPU passes: median {:.3} ms, p95 {:.3} ms over {} timed frames",
                gpu.p50, gpu.p95, gpu.samples
            ),
            None => println!("    GPU passes: not measured"),
        }
        let batch_instances = scene
            .instance_batches
            .iter()
            .find_map(|&(key, n)| (key == "instances").then_some(n))
            .unwrap_or(0);
        println!(
            "    {} draw calls, {} models and {} triangles in view (+{} triangles in shadow maps), {} instances in static batches, {:.0} bytes uploaded a frame",
            last.draw_calls,
            last.instances,
            last.triangles,
            last.shadow_triangles,
            batch_instances,
            scene.uploads_per_frame.iter().map(|(_, bytes)| bytes).sum::<f64>()
        );
        if let Some(dir) = &png {
            save_png(dir, kind, &bench, settings.setup.size);
        }
        report.scenes.push(scene);
    }
    if report.scenes.is_empty() {
        return None;
    }
    match peak_rss() {
        Ok(bytes) => report.peak_rss_bytes = Some(bytes),
        Err(reason) => report.peak_rss_unavailable = Some(reason),
    }
    if let Some(path) = std::env::var_os("PONKO_RENDER_CSV") {
        write(&path, "render summary (CSV)", report.summary_csv());
    }
    if let Some(path) = std::env::var_os("PONKO_RENDER_FRAMES_CSV") {
        write(&path, "render frames (CSV)", report.frames_csv());
    }
    Some(report.to_json())
}
