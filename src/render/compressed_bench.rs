//! Offscreen benchmark of block-compressed textures (feature
//! `compressed-textures`): GPU memory and load time of a reference set of
//! material textures as PNGs and as the KTX2 files `ponko textures` makes
//! of them. Each texture is read from disk, decoded and uploaded with its
//! whole mip chain, as the renderer does: a PNG's mipmaps made on the GPU
//! (`ensure_mips`), a KTX2's uploaded as stored. Run explicitly, in
//! release, with a folder of the scene's files:
//!
//! ```sh
//! # 1. Writes the PNGs and their textures.json, then stops and names step 2.
//! cargo test --release --lib --features 3d,compressed-textures compressed_bench -- --ignored --nocapture
//! # 2. Makes the KTX2 files next to them.
//! cargo run --release -p ponko_cli -- textures target/compressed-bench
//! # 3. Measures.
//! cargo test --release --lib --features 3d,compressed-textures compressed_bench -- --ignored --nocapture
//! ```
//!
//! `PONKO_TEXTURE_BENCH_DIR` picks another folder (any PNGs with KTX2 files
//! of the same names next to them).
use super::render_test_support::{TestGpu, gpu_with};
use crate::compressed::{self, Decoded};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The reference scene: six materials of a color, a normal and a mask
/// texture each, 1024 x 1024.
const MATERIALS: usize = 6;
const SIZE: u32 = 1024;
const RUNS: usize = 3;

fn folder() -> PathBuf {
    std::env::var_os("PONKO_TEXTURE_BENCH_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("target/compressed-bench"),
        PathBuf::from,
    )
}

/// Smooth value noise in 0..1 at (x, y) on a grid of `cell` pixels.
fn noise(x: u32, y: u32, cell: u32, seed: u32) -> f32 {
    let hash = |x: u32, y: u32| {
        let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        (h & 0xFFFF) as f32 / 65535.0
    };
    let (gx, gy) = (x / cell, y / cell);
    let (fx, fy) = ((x % cell) as f32 / cell as f32, (y % cell) as f32 / cell as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let top = hash(gx, gy) + (hash(gx + 1, gy) - hash(gx, gy)) * sx;
    let bottom = hash(gx, gy + 1) + (hash(gx + 1, gy + 1) - hash(gx, gy + 1)) * sx;
    top + (bottom - top) * sy
}

/// Material `m`'s height: a few octaves of noise.
fn height(x: u32, y: u32, m: u32) -> f32 {
    [(128, 0.5), (32, 0.3), (8, 0.15), (2, 0.05)]
        .iter()
        .map(|&(cell, weight)| noise(x, y, cell, m * 7 + cell) * weight)
        .sum()
}

/// Writes the scene's PNGs and `textures.json` into `dir`.
fn write_scene(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for m in 0..MATERIALS as u32 {
        let mut color = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let mut normal = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let mut mask = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let tint = [noise(m, 0, 1, 1), noise(m, 1, 1, 2), noise(m, 2, 1, 3)];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let h = height(x, y, m);
                let dx = height((x + 1) % SIZE, y, m) - height(x.wrapping_sub(1) % SIZE, y, m);
                let dy = height(x, (y + 1) % SIZE, m) - height(x, y.wrapping_sub(1) % SIZE, m);
                let n = glam::vec3(-dx * 8.0, -dy * 8.0, 1.0).normalize();
                let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                color.extend([
                    byte(tint[0] * h * 1.6),
                    byte(tint[1] * h * 1.6),
                    byte(tint[2] * h * 1.6),
                    255,
                ]);
                normal.extend([byte(n.x * 0.5 + 0.5), byte(n.y * 0.5 + 0.5), byte(n.z * 0.5 + 0.5), 255]);
                // One channel, as blend and roughness masks are.
                let v = byte(noise(x, y, 16, m + 40) * 0.5 + h);
                mask.extend([v, v, v, 255]);
            }
        }
        for (kind, rgba) in [("color", color), ("normal", normal), ("mask", mask)] {
            let path = dir.join(format!("material{m}_{kind}.png"));
            image::save_buffer(&path, &rgba, SIZE, SIZE, image::ColorType::Rgba8).unwrap();
        }
    }
    std::fs::write(
        dir.join("textures.json"),
        r#"{ "textures": { "*_color.png": "color", "*_normal.png": "normal", "*_mask.png": "mask" } }"#,
    )
    .unwrap();
}

/// The scene's PNGs, sorted, each with the KTX2 file of its name.
fn scene_files(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut pngs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "png"))
                .collect()
        })
        .unwrap_or_default();
    pngs.sort();
    pngs.into_iter()
        .map(|png| (png.clone(), png.with_extension("ktx2")))
        .collect()
}

/// Whether a texture holds data (normals, masks), from its file name.
fn is_data(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    name.contains("normal") || name.contains("mask")
}

struct Loaded {
    textures: Vec<wgpu::Texture>,
    seconds: f64,
    /// Textures the GPU couldn't take as blocks, decoded to RGBA8 instead.
    fallbacks: usize,
}

/// Reads, decodes and uploads every file with its mip chain, then waits
/// for the GPU.
fn load(gpu: &TestGpu, mips: &super::mips::MipMaker, files: &[&Path]) -> Loaded {
    let formats = super::blocks::gpu_formats(gpu.device.features());
    let start = Instant::now();
    let mut textures = Vec::with_capacity(files.len());
    let mut fallbacks = 0;
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    for path in files {
        let bytes = std::fs::read(path).unwrap();
        let name = path.to_string_lossy();
        let rgba = match compressed::decode(&bytes, &name, "a bench texture", formats) {
            Some(Ok(Decoded::Blocks(image))) => {
                textures.push(super::blocks::upload(&gpu.device, &gpu.queue, "bench", &[&image]));
                continue;
            }
            Some(Ok(Decoded::Rgba(image))) => {
                fallbacks += 1;
                image
            }
            Some(Err(e)) => panic!("{e}"),
            None => crate::assets::decode_image(&bytes, &name, "a bench texture").unwrap(),
        };
        // As `Renderer::create_texture` and `ensure_mips` do.
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench"),
            size: wgpu::Extent3d {
                width: rgba.width,
                height: rgba.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: super::mips::levels(rgba.width, rgba.height),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            &rgba.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * rgba.width),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: rgba.width,
                height: rgba.height,
                depth_or_array_layers: 1,
            },
        );
        mips.generate(&gpu.device, &mut encoder, &texture, is_data(path));
        textures.push(texture);
    }
    gpu.queue.submit([encoder.finish()]);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    Loaded {
        textures,
        seconds: start.elapsed().as_secs_f64(),
        fallbacks,
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

#[test]
#[ignore = "a benchmark: run in release with --ignored --nocapture"]
fn compressed_bench() {
    let dir = folder();
    let mut files = scene_files(&dir);
    if files.is_empty() {
        write_scene(&dir);
        files = scene_files(&dir);
    }
    if let Some((_, missing)) = files.iter().find(|(_, ktx2)| !ktx2.exists()) {
        panic!(
            "{} is missing: run `cargo run --release -p ponko_cli -- textures {}`, then this again",
            missing.display(),
            dir.display()
        );
    }
    let Some(gpu) = gpu_with(|adapter| {
        Ok(wgpu::DeviceDescriptor {
            required_features: adapter.features() & super::blocks::FEATURES,
            ..Default::default()
        })
    }) else {
        return;
    };
    let mips = super::mips::MipMaker::new(&gpu.device);
    let pngs: Vec<&Path> = files.iter().map(|(png, _)| png.as_path()).collect();
    let ktx2s: Vec<&Path> = files.iter().map(|(_, ktx2)| ktx2.as_path()).collect();
    let file_bytes = |paths: &[&Path]| paths.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum::<u64>();
    eprintln!(
        "compressed textures: {} textures from {} on {} ({:?}), formats {:?}",
        files.len(),
        dir.display(),
        gpu.info.name,
        gpu.info.backend,
        super::blocks::gpu_formats(gpu.device.features()),
    );
    // Warm up the file cache and the pipelines once, then alternate.
    drop(load(&gpu, &mips, &pngs));
    drop(load(&gpu, &mips, &ktx2s));
    let mut times = [Vec::new(), Vec::new()];
    let mut report = Vec::new();
    for _ in 0..RUNS {
        for (i, (name, paths)) in [("PNG + GPU mipmaps", &pngs), ("KTX2 (ponko textures)", &ktx2s)]
            .into_iter()
            .enumerate()
        {
            let before = gpu.device.generate_allocator_report().map(|r| r.total_allocated_bytes);
            let loaded = load(&gpu, &mips, paths);
            let after = gpu.device.generate_allocator_report().map(|r| r.total_allocated_bytes);
            times[i].push(loaded.seconds);
            let bytes: u64 = loaded.textures.iter().map(super::gpu::texture_bytes).sum();
            let device = before.zip(after).map(|(b, a)| a.saturating_sub(b));
            report.push((name, bytes, device, file_bytes(paths), loaded.fallbacks));
            drop(loaded);
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    }
    for (i, &(name, bytes, device, on_disk, fallbacks)) in report[..2].iter().enumerate() {
        let mut t = times[i].clone();
        t.sort_by(f64::total_cmp);
        eprintln!(
            "{name}: GPU textures {} (renderer's count), device allocations {}, files {}, \
             load {:.0} ms (median of {RUNS}; min {:.0}, max {:.0}){}",
            mib(bytes),
            device.map_or_else(|| "not reported by this backend".to_owned(), mib),
            mib(on_disk),
            t[RUNS / 2] * 1000.0,
            t[0] * 1000.0,
            t[RUNS - 1] * 1000.0,
            if fallbacks > 0 {
                format!(", {fallbacks} decoded to RGBA8 (the GPU lacks their format)")
            } else {
                String::new()
            },
        );
    }
}
