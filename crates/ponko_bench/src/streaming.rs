//! Asset streaming without gameplay-thread stalls: a game that needs 16
//! new assets at once (a cell coming into view), loaded on the game thread
//! (`load_texture`, `load_mesh`) or streamed (`stream_texture`,
//! `stream_mesh`: read and decoded on job threads, uploaded within the
//! per-frame budget). The assets are 16 PNGs of 1024 × 1024 noise, or 16
//! glTF binaries of a 256 × 256 vertex grid (65 536 vertices, 130 050
//! triangles, about 3 MiB each).
//!
//! Measures the game thread's CPU time in its loading work each frame: the
//! load or stream calls plus the engine's upload step (for meshes, that is
//! where they are built from the decoded file). CPU time, not wall time,
//! so the OS pausing the thread on a busy machine doesn't count (on
//! systems without a thread CPU clock, Windows, it is wall time).
//! Headless: the GPU's share of uploads isn't included.
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ponko::prelude::{Ctx, Game};

use crate::{Bench, report};

const ASSETS: usize = 16;
const SIDE: u32 = 1024;
/// Vertices along each side of a benchmark mesh.
const GRID: u32 = 256;
const FRAMES: u64 = 600;
/// The frame the game first needs the assets in.
const NEED_AT: u64 = 10;

/// CPU time this thread has used.
#[cfg(unix)]
fn thread_cpu() -> Duration {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `t` is a valid, writable timespec.
    let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    assert_eq!(ok, 0, "thread CPU clock");
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

/// Without a thread CPU clock: wall time since the first call.
#[cfg(not(unix))]
fn thread_cpu() -> Duration {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

/// Writes 16 PNGs of noise (so decoding costs what a real image does) to
/// `dir` and returns their paths.
fn noise_pngs(dir: &Path) -> Vec<String> {
    (0..ASSETS)
        .map(|i| {
            let mut s = (i as u32 + 1).wrapping_mul(0x9E37_79B9) | 1;
            let img = image::RgbaImage::from_fn(SIDE, SIDE, |_, _| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                image::Rgba([s as u8, (s >> 8) as u8, (s >> 16) as u8, 255])
            });
            let path: PathBuf = dir.join(format!("t{i}.png"));
            img.save(&path).expect("write png");
            path.to_str().expect("utf-8 path").to_string()
        })
        .collect()
}

/// A glTF binary of one mesh: a `GRID` × `GRID` vertex height field with
/// normals and 32-bit indices, shaped by `seed`.
fn grid_glb(seed: u32) -> Vec<u8> {
    let n = GRID;
    let height = |x: u32, z: u32| ((x * 7 + z * 13 + seed * 31) % 17) as f32 * 0.05;
    let mut bin = Vec::new();
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for z in 0..n {
        for x in 0..n {
            let p = [x as f32, height(x, z), z as f32];
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
            p.iter().for_each(|v| bin.extend_from_slice(&v.to_le_bytes()));
        }
    }
    let positions = bin.len();
    for _ in 0..n * n {
        [0.0f32, 1.0, 0.0]
            .iter()
            .for_each(|v| bin.extend_from_slice(&v.to_le_bytes()));
    }
    let normals = bin.len() - positions;
    let mut indices = 0;
    for z in 0..n - 1 {
        for x in 0..n - 1 {
            let i = z * n + x;
            for v in [i, i + n, i + 1, i + 1, i + n, i + n + 1] {
                bin.extend_from_slice(&v.to_le_bytes());
                indices += 1;
            }
        }
    }
    let count = n * n;
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1}},"indices":2}}]}}],
"buffers":[{{"byteLength":{len}}}],
"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{positions},"target":34962}},
{{"buffer":0,"byteOffset":{positions},"byteLength":{normals},"target":34962}},
{{"buffer":0,"byteOffset":{inorm},"byteLength":{ibytes},"target":34963}}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":{count},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}},
{{"bufferView":1,"componentType":5126,"count":{count},"type":"VEC3"}},
{{"bufferView":2,"componentType":5125,"count":{indices},"type":"SCALAR"}}]}}"#,
        min[0],
        min[1],
        min[2],
        max[0],
        max[1],
        max[2],
        len = bin.len(),
        inorm = positions + normals,
        ibytes = indices * 4,
    );
    let mut json = json.into_bytes();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&bin);
    glb
}

/// Writes 16 grid meshes to `dir` and returns their paths.
fn grid_meshes(dir: &Path) -> Vec<String> {
    (0..ASSETS)
        .map(|i| {
            let path = dir.join(format!("m{i}.glb"));
            std::fs::write(&path, grid_glb(i as u32)).expect("write glb");
            path.to_str().expect("utf-8 path").to_string()
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Load {
    Texture,
    StreamTexture,
    Mesh,
    StreamMesh,
}

impl Load {
    fn streamed(self) -> bool {
        matches!(self, Load::StreamTexture | Load::StreamMesh)
    }

    /// Loads or streams `path`; whether it is usable now.
    fn ask(self, ctx: &mut Ctx, path: &str) -> bool {
        match self {
            Load::Texture => {
                ctx.gfx.load_texture(path);
                true
            }
            Load::Mesh => {
                ctx.gfx.load_mesh(path);
                true
            }
            Load::StreamTexture => ctx.gfx.stream_texture(path).is_some(),
            Load::StreamMesh => ctx.gfx.stream_mesh(path).is_some(),
        }
    }
}

/// Each frame's loading work on the game thread, the frame (after the
/// game first needed them) all assets were usable in, and the longest
/// time between frames.
fn frames(paths: &[String], load: Load) -> (Vec<Duration>, Option<u64>, Duration) {
    let work = Rc::new(RefCell::new(Vec::new()));
    let ready_at = Rc::new(RefCell::new(None));
    let wall = Rc::new(RefCell::new(Duration::ZERO));
    let (log, ready, worst) = (work.clone(), ready_at.clone(), wall.clone());
    let mut last: Option<Instant> = None;
    let mut since_system = Duration::ZERO;
    let paths = paths.to_vec();
    Game::new("bench", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            let now = Instant::now();
            if let Some(last) = last {
                let mut w = worst.borrow_mut();
                *w = (*w).max(now - last);
            }
            last = Some(now);
            let tick = ctx.time.sim_tick();
            // The engine's upload step ran between the end of this system
            // last frame and now: count its CPU time (with a little engine
            // bookkeeping), then the loading calls'.
            let start = thread_cpu();
            let step = if load.streamed() && tick > 0 {
                start.saturating_sub(since_system)
            } else {
                Duration::ZERO
            };
            if tick >= NEED_AT {
                let usable = paths.iter().filter(|p| load.ask(ctx, p)).count();
                if usable == ASSETS && ready.borrow().is_none() {
                    *ready.borrow_mut() = Some(tick - NEED_AT);
                }
            }
            log.borrow_mut().push(step + (thread_cpu() - start));
            // Gameplay work that must not wait: 1 ms of it each frame.
            while now.elapsed() < Duration::from_millis(1) {
                std::hint::black_box(0u64);
            }
            since_system = thread_cpu();
        })
        .run_headless(FRAMES, |_| {});
    let w = work.borrow().clone();
    let r = *ready_at.borrow();
    let worst = *wall.borrow();
    (w, r, worst)
}

pub fn run(b: &mut Bench) {
    if b.skips("streaming/") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("ponko-bench-streaming-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut pngs = None;
    let mut glbs = None;
    let cases = [
        ("streaming/sync-load-16x1k", Load::Texture),
        ("streaming/stream-16x1k", Load::StreamTexture),
        ("streaming/sync-load-16-meshes", Load::Mesh),
        ("streaming/stream-16-meshes", Load::StreamMesh),
    ];
    for (name, load) in cases {
        if b.skips(name) {
            continue;
        }
        let paths = match load {
            Load::Texture | Load::StreamTexture => pngs.get_or_insert_with(|| noise_pngs(&dir)),
            Load::Mesh | Load::StreamMesh => glbs.get_or_insert_with(|| grid_meshes(&dir)),
        };
        let (mut times, ready, wall) = frames(paths, load);
        let record = report::Record::new(name, report::Kind::Steady, 0, &mut times);
        let ready = ready.unwrap_or_else(|| panic!("{name}: the assets never became usable"));
        println!(
            "{name:<32} worst frame's loading work {:>8.3} ms   usable after {ready} frames   longest frame {:>8.3} ms",
            crate::ms(record.max),
            crate::ms(wall)
        );
        b.report(record);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thread_clock_moves_forward_with_work() {
        let before = thread_cpu();
        let mut sum = 0u64;
        while thread_cpu() == before {
            sum = std::hint::black_box(sum.wrapping_add(1));
        }
        assert!(thread_cpu() > before);
    }

    #[test]
    fn the_benchmark_meshes_load() {
        let dir = std::env::temp_dir().join(format!("ponko-bench-glb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("grid.glb");
        std::fs::write(&path, grid_glb(3)).unwrap();
        let path = path.to_str().unwrap().to_string();
        Game::new("t", 1, 1).run_headless(0, move |ctx| {
            let mesh = ctx.gfx.load_mesh(&path);
            assert!(ctx.gfx.is_live(mesh));
        });
        let _ = std::fs::remove_dir_all(dir);
    }
}
