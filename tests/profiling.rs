//! Profiling: frame statistics, the performance overlay and traces.

use ponko::prelude::*;

// ANCHOR: profiling
/// F3 shows the performance overlay; F4 records the next 300 frames as a
/// trace to open in Perfetto. Slow stretches are logged for investigation.
fn perf_keys(ctx: &mut Ctx) {
    if ctx.input.key_pressed(Key::F3) {
        let on = !ctx.perf.hud();
        ctx.perf.set_hud(on);
    }
    if ctx.input.key_pressed(Key::F4) {
        ctx.perf.record_trace("trace.json", 300);
    }
    let stats = ctx.perf.frame_stats();
    if stats.frames > 0 && stats.p99_ms > 20.0 && ctx.time.frame().is_multiple_of(600) {
        // Display wait includes pacing; inspect measured CPU/GPU work separately.
        println!("slow: p99 {:.1} ms, gpu {:?} ms", stats.p99_ms, stats.gpu_ms);
        for pass in ctx.perf.gpu_passes() {
            println!("  {}: {:.2} ms", pass.name, pass.ms);
        }
    }
}
// ANCHOR_END: profiling

// ANCHOR: uploads
/// Logs frames that send the GPU far more than a still scene does (which
/// sends next to nothing): many models moved or spawned, or a buffer
/// outgrown and made anew.
fn upload_watch(ctx: &mut Ctx) {
    let u = ctx.perf.render_stats().uploads;
    if u.bytes() > 256 * 1024 || u.reallocations > 0 {
        println!(
            "frame {}: {} KiB uploaded ({} KiB of models), {} buffers outgrown",
            ctx.time.frame(),
            u.bytes() / 1024,
            u.instances / 1024,
            u.reallocations
        );
    }
}
// ANCHOR_END: uploads

#[test]
fn headless_runs_upload_nothing_to_the_gpu() {
    Game::new("t", 1, 1).system(upload_watch).run_headless(10, |ctx| {
        assert_eq!(ctx.perf.render_stats().uploads, ponko::render::GpuUploads::default());
    });
}

#[test]
fn headless_runs_have_no_frame_stats() {
    Game::new("t", 1, 1).system(perf_keys).run_headless(10, |ctx| {
        assert_eq!(ctx.perf.frame_stats().frames, 0);
        assert!(ctx.perf.gpu_passes().is_empty());
        ctx.perf.set_hud(true);
        assert!(ctx.perf.hud());
        ctx.perf.record_trace("never-written.json", 10); // no frames: nothing
    });
    assert!(!std::path::Path::new("never-written.json").exists());
}
