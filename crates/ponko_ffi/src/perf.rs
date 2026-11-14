//! Profiling from C: frame-time statistics, the performance overlay and
//! traces (Rust `Perf::frame_stats`, `set_perf_hud`, `record_trace`).

use std::ffi::c_char;

use ponko::perf::Budget;
use ponko::perf::PerfReport;
use ponko::perf::ProfileScope;

use crate::ctx::{ctx_access, out, text};
use crate::{PonkoCtx, PonkoStatus, boundary, invalid, invalid_owned};

/// Frame timing over the last few seconds (Rust `FrameStats`), in ms.
/// ```
/// let s = ponko_ffi::PonkoFrameStats::default();
/// assert_eq!((s.frames, s.gpu_ms), (0, -1.0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoFrameStats {
    /// How many frames these numbers cover (0 before the first frame).
    pub frames: u64,
    /// Frames per second.
    pub fps: f32,
    /// Average frame time.
    pub avg_ms: f32,
    /// Half the frames were faster than this.
    pub p50_ms: f32,
    /// The slowest 5% took at least this.
    pub p95_ms: f32,
    /// The slowest 1% took at least this.
    pub p99_ms: f32,
    /// The slowest frame.
    pub max_ms: f32,
    /// CPU time in systems.
    pub systems_ms: f32,
    /// CPU time drawing, waiting on the display included.
    pub render_cpu_ms: f32,
    /// Of that, waiting for the display (or the GPU) to take a frame.
    pub display_wait_ms: f32,
    /// GPU time per frame; -1 where the GPU can't time itself.
    pub gpu_ms: f32,
}

impl Default for PonkoFrameStats {
    fn default() -> Self {
        Self {
            frames: 0,
            fps: 0.0,
            avg_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            max_ms: 0.0,
            systems_ms: 0.0,
            render_cpu_ms: 0.0,
            display_wait_ms: 0.0,
            gpu_ms: -1.0,
        }
    }
}

/// Writes frame timing over the last few seconds to `*out_stats` (all zero
/// in headless runs).
/// ```
/// let mut s = ponko_ffi::PonkoFrameStats::default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_frame_stats(std::ptr::null(), &mut s) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_stats` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_frame_stats(ctx: *const PonkoCtx, out_stats: *mut PonkoFrameStats) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let s = ctx_access(ctx, |c| Ok(c.perf.frame_stats()))?;
        out(
            out_stats,
            PonkoFrameStats {
                frames: s.frames as u64,
                fps: s.fps,
                avg_ms: s.avg_ms,
                p50_ms: s.p50_ms,
                p95_ms: s.p95_ms,
                p99_ms: s.p99_ms,
                max_ms: s.max_ms,
                systems_ms: s.systems_ms,
                render_cpu_ms: s.render_cpu_ms,
                display_wait_ms: s.display_wait_ms,
                gpu_ms: s.gpu_ms.unwrap_or(-1.0),
            },
        )
    })
}

/// What the renderer sent to the GPU over the last drawn frame (Rust
/// `RenderStats::uploads`, `GpuUploads`): bytes uploaded by kind, buffers
/// made, and the buffers and textures replaced because they were outgrown.
/// All zero in headless runs and before the first frame. In a scene where
/// nothing changes, nothing is uploaded and no buffer is made.
/// ```
/// let u = ponko_ffi::PonkoGpuUploads::default();
/// assert_eq!((u.bytes, u.reallocations), (0, 0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoGpuUploads {
    /// Every byte uploaded, all kinds together.
    pub bytes: u64,
    /// 3D models' per-frame records (transforms, colors, motion).
    pub instances: u64,
    /// 2D sprites' records.
    pub sprites: u64,
    /// Instance batches and their culling data.
    pub static_batches: u64,
    /// Small per-frame uniforms: cameras, post-processing, effect settings.
    pub uniforms: u64,
    /// Lights and decals.
    pub lights: u64,
    /// Material parameters.
    pub materials: u64,
    /// Skinned models: joint matrices and morph weights.
    pub skins: u64,
    /// Mesh vertices and indices, and GPU buffers the game made.
    pub meshes: u64,
    /// Texture pixels.
    pub textures: u64,
    /// Everything else: particles, 3D text, occlusion grids.
    pub other: u64,
    /// The bytes of the buffers made.
    pub buffer_bytes_created: u64,
    /// Buffers made.
    pub buffers_created: u32,
    /// Buffers and textures replaced by bigger ones because they were
    /// outgrown (the new buffers are also in `buffers_created`, the
    /// textures not).
    pub reallocations: u32,
}

impl From<ponko::render::GpuUploads> for PonkoGpuUploads {
    fn from(u: ponko::render::GpuUploads) -> Self {
        Self {
            bytes: u.bytes(),
            instances: u.instances,
            sprites: u.sprites,
            static_batches: u.static_batches,
            uniforms: u.uniforms,
            lights: u.lights,
            materials: u.materials,
            skins: u.skins,
            meshes: u.meshes,
            textures: u.textures,
            other: u.other,
            buffer_bytes_created: u.buffer_bytes_created,
            buffers_created: u.buffers_created,
            reallocations: u.reallocations,
        }
    }
}

/// Writes what the renderer uploaded over the last drawn frame to
/// `*out_uploads` (see `PonkoGpuUploads`).
/// ```
/// let mut u = ponko_ffi::PonkoGpuUploads::default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_gpu_uploads(std::ptr::null(), &mut u) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_uploads` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_gpu_uploads(ctx: *const PonkoCtx, out_uploads: *mut PonkoGpuUploads) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let u = ctx_access(ctx, |c| Ok(c.perf.render_stats().uploads))?;
        out(out_uploads, u.into())
    })
}

/// Shows or hides the on-screen performance overlay (fps, frame-time
/// percentiles, CPU and GPU time, the slowest GPU passes).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_perf_hud(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_perf_hud(ctx: *mut PonkoCtx, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.perf.set_hud(on);
            Ok(())
        })
    })
}

/// Records the next `frames` frames as a Chrome/Perfetto trace at `path`
/// (open it in <https://ui.perfetto.dev>). Does nothing in headless runs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_record_trace(std::ptr::null_mut(), c"t.json".as_ptr(), 60) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `path` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_record_trace(ctx: *mut PonkoCtx, path: *const c_char, frames: u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.perf.record_trace(text(path)?, frames as usize);
            Ok(())
        })
    })
}

/// Copies `s` into `buffer` (NUL-terminated if it fits) and writes its
/// length to `out_len`.
///
/// # Safety
/// `buffer` holds `capacity` bytes (or is null with capacity 0); `out_len` is writable.
pub(crate) unsafe fn write_text(buffer: *mut c_char, capacity: usize, out_len: *mut usize, s: &str) {
    // SAFETY: forwarded caller contract.
    unsafe {
        out_len.write(s.len());
        if s.len() < capacity {
            std::ptr::copy_nonoverlapping(s.as_ptr(), buffer.cast::<u8>(), s.len());
            buffer.add(s.len()).write(0);
        } else if capacity > 0 {
            buffer.write(0);
        }
    }
}

thread_local! {
    /// The report of this thread's last `ponko_game_profile_headless`.
    static LAST_REPORT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub(crate) fn keep_report(json: String) {
    LAST_REPORT.with(|r| *r.borrow_mut() = json);
}

/// Writes the report of this thread's last `ponko_game_profile_headless`
/// (JSON) to `buffer` (NUL-terminated if it fits in `capacity`) and its
/// length to `*out_len`; call with a null buffer and capacity 0 first to
/// learn the size. The length is 0 if there was none.
/// ```
/// let mut len = 1usize;
/// let s = unsafe { ponko_ffi::ponko_last_perf_report(std::ptr::null_mut(), 0, &mut len) };
/// assert_eq!(s as u32, 0);
/// ```
/// # Safety
/// `buffer` holds `capacity` bytes (or is null with capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_last_perf_report(
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_len.is_null() {
            return Err(invalid("output is null"));
        }
        if buffer.is_null() && capacity > 0 {
            return Err(invalid("buffer is null"));
        }
        LAST_REPORT.with(|r| write_text(buffer, capacity, out_len, &r.borrow()));
        Ok(())
    })
}

/// Something to time: made once per place in your code with
/// `ponko_profile_scope`. Opaque.
pub struct PonkoProfileScope {
    _private: [u8; 0],
}

/// The profiling scope called `name`, to time with `ponko_profile_begin` and
/// `ponko_profile_end`. Keep the pointer (it lives as long as the process):
/// `PONKO_PROFILE_SCOPE` in `ponko.hpp` keeps it in a `static`. Null if
/// `name` is null or not UTF-8.
/// ```
/// let a = unsafe { ponko_ffi::ponko_profile_scope(c"ai".as_ptr()) };
/// let b = unsafe { ponko_ffi::ponko_profile_scope(c"ai".as_ptr()) };
/// assert!(!a.is_null() && a == b);
/// ```
/// # Safety
/// `name` must be null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_profile_scope(name: *const c_char) -> *const PonkoProfileScope {
    // SAFETY: forwarded caller contract.
    match unsafe { text(name) } {
        Ok(name) => std::ptr::from_ref(ponko::perf::scope_named(name)).cast(),
        Err(_) => std::ptr::null(),
    }
}

/// Starts timing `scope`; hand the result to `ponko_profile_end`. Costs one
/// atomic load (and returns 0) while profiling is off, and is safe to call
/// from any thread.
/// ```
/// let scope = unsafe { ponko_ffi::ponko_profile_scope(c"work".as_ptr()) };
/// let token = unsafe { ponko_ffi::ponko_profile_begin(scope) };
/// unsafe { ponko_ffi::ponko_profile_end(scope, token) };
/// ```
/// # Safety
/// `scope` must be null or from `ponko_profile_scope`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_profile_begin(scope: *const PonkoProfileScope) -> u64 {
    // SAFETY: scopes come from `scope_named`, which leaks them: they are `'static`.
    unsafe { scope.cast::<ProfileScope>().as_ref() }.map_or(0, ProfileScope::begin)
}

/// Stops timing `scope`, started by `ponko_profile_begin` (which returned
/// `token`).
/// # Safety
/// `scope` must be null or from `ponko_profile_scope`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_profile_end(scope: *const PonkoProfileScope, token: u64) {
    // SAFETY: as in `ponko_profile_begin`.
    if let Some(scope) = unsafe { scope.cast::<ProfileScope>().as_ref() } {
        scope.end(token);
    }
}

/// Counts one heap allocation of `bytes`, made by something other than Rust's
/// allocator (`PONKO_COUNT_CPP_ALLOCATIONS` in `ponko.hpp` calls this from
/// `operator new`). Counted only while profiling is on.
/// ```
/// ponko_ffi::ponko_note_allocation(64);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_note_allocation(bytes: usize) {
    ponko::perf::note_allocation(bytes);
}

/// Turns the deeper profiling on or off: scopes are timed and allocations
/// counted (the overlay switches it on by itself).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_profiling(std::ptr::null_mut(), true) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_profiling(ctx: *mut PonkoCtx, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.perf.set_profiling(on);
            Ok(())
        })
    })
}

/// Writes the performance report of the last frames as JSON to `buffer`
/// (NUL-terminated if it fits in `capacity`) and its length to `*out_len`;
/// call with a null buffer and capacity 0 first to learn the size. Needs
/// `ponko_ctx_set_profiling` (or the overlay) to have been on for a while.
/// Print it with `ponko_perf_report_text`.
/// ```
/// let mut len = 0usize;
/// let s = unsafe { ponko_ffi::ponko_ctx_perf_report(std::ptr::null(), std::ptr::null_mut(), 0, &mut len) };
/// assert_eq!(s as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `buffer` holds `capacity` bytes (or is null
/// with capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_perf_report(
    ctx: *const PonkoCtx,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_len.is_null() {
            return Err(invalid("output is null"));
        }
        if buffer.is_null() && capacity > 0 {
            return Err(invalid("buffer is null"));
        }
        let json = ctx_access(ctx, |c| {
            // Count the world as it is now, as this function always has:
            // the service's count is from the last command flush.
            let mut report = c.perf.report();
            report.entities = c.world.len();
            Ok(report.to_json())
        })?;
        write_text(buffer, capacity, out_len, &json);
        Ok(())
    })
}

/// Runs `f` on the report in `json`, writes its text to `buffer`.
///
/// # Safety
/// As the public functions below.
unsafe fn report_text(
    json: *const c_char,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
    f: impl FnOnce(PerfReport) -> Result<String, String>,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if out_len.is_null() {
            return Err(invalid("output is null"));
        }
        if buffer.is_null() && capacity > 0 {
            return Err(invalid("buffer is null"));
        }
        let report = PerfReport::from_json(text(json)?).map_err(invalid_owned)?;
        let out = f(report).map_err(invalid_owned)?;
        write_text(buffer, capacity, out_len, &out);
        Ok(())
    })
}

/// Formats a report from `ponko_ctx_perf_report` or
/// `ponko_game_profile_headless` for people to read: frame times, systems,
/// scopes, allocations, the slowest frames and warnings with advice.
/// Writes into `buffer` as `ponko_ctx_perf_report` does.
/// ```
/// let mut len = 0usize;
/// let s = unsafe { ponko_ffi::ponko_perf_report_text(c"nope".as_ptr(), std::ptr::null_mut(), 0, &mut len) };
/// assert_ne!(s as u32, 0, "not a report");
/// ```
/// # Safety
/// `report_json` NUL-terminated; `buffer` holds `capacity` bytes (or is null
/// with capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_perf_report_text(
    report_json: *const c_char,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { report_text(report_json, buffer, capacity, out_len, |r| Ok(r.to_string())) }
}

/// Checks a report against a budget, for tests and CI. `budget_json` is
/// like `{"frame_p99_ms": 2, "allocs_per_frame": 0, "systems": {"movement":
/// 0.5}, "scopes": {"ai/pathing": 1}}` (every key is optional). Writes what
/// is over budget, one line each, to `buffer`: an empty string (length 0)
/// means every limit holds. `PONKO_INVALID_ARGUMENT` if either is not JSON
/// of that shape.
/// ```
/// let mut len = 1usize;
/// let s = unsafe { ponko_ffi::ponko_perf_check(c"{}".as_ptr(), c"{}".as_ptr(), std::ptr::null_mut(), 0, &mut len) };
/// assert_ne!(s as u32, 0, "an empty object is not a report");
/// ```
/// # Safety
/// Both strings NUL-terminated; `buffer` holds `capacity` bytes (or is null
/// with capacity 0); `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_perf_check(
    report_json: *const c_char,
    budget_json: *const c_char,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        let budget = text(budget_json)
            .map_err(|_| ())
            .and_then(|j| Budget::from_json(j).map_err(|_| ()));
        report_text(report_json, buffer, capacity, out_len, |r| {
            let budget = budget.map_err(|()| "the budget is not valid JSON of the budget's shape".to_string())?;
            Ok(r.check(&budget).iter().map(|v| format!("{v}\n")).collect())
        })
    }
}

/// What got slower between two reports, one line each (empty: nothing did):
/// `report_json` is today's, `baseline_json` an earlier one you saved.
/// Values count when they grew by more than `tolerance` (0.2 is 20%). Writes
/// into `buffer` as `ponko_perf_check` does.
/// ```
/// let mut len = 1usize;
/// let s = unsafe { ponko_ffi::ponko_perf_compare(c"x".as_ptr(), c"y".as_ptr(), 0.2, std::ptr::null_mut(), 0, &mut len) };
/// assert_ne!(s as u32, 0);
/// ```
/// # Safety
/// As `ponko_perf_check`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_perf_compare(
    report_json: *const c_char,
    baseline_json: *const c_char,
    tolerance: f32,
    buffer: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        let baseline = text(baseline_json)
            .map_err(|_| ())
            .and_then(|j| PerfReport::from_json(j).map_err(|_| ()));
        report_text(report_json, buffer, capacity, out_len, |r| {
            let baseline = baseline.map_err(|()| "the baseline is not a performance report".to_string())?;
            Ok(r.regressions(&baseline, tolerance)
                .iter()
                .map(|v| format!("{v}\n"))
                .collect())
        })
    }
}
