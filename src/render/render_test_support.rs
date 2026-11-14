//! Shared helpers and conventions for the offscreen GPU suites
//! (`src/render/*_render_test.rs`).
//!
//! # Conventions
//!
//! - **Adapters.** Get the device with [`gpu`] or [`gpu_with`], never by
//!   calling `request_adapter` directly (GPU tests inside other modules too). When no adapter or device exists the
//!   test returns early and logs `render test SKIPPED [<test>]: <reason>`;
//!   when it runs it logs `render test ran [<test>] on <adapter> (<backend>,
//!   ...)`. A test that needs a capability the adapter lacks calls [`skip`]
//!   with the reason. A skip is not a pass: it proves nothing about the
//!   picture. To see what ran, use `--nocapture` and look for those lines:
//!
//!   ```sh
//!   cargo test -p ponko --lib --features 3d render_test -- --nocapture 2>&1 | grep "render test"
//!   ```
//!
//!   Set `PONKO_REQUIRE_GPU=1` on a machine (or CI runner) that has a GPU to
//!   turn a missing adapter or device into a failure instead of a skip.
//!   Capability skips stay skips: some adapters really lack a feature.
//! - **GPU tiers.** Set `PONKO_GPU_TIER` to `webgl2` or `baseline` to run
//!   every suite on a device opened with that tier's limits and without
//!   timestamp queries ([`forced`]; `desktop` runs as usual), as CI does
//!   one tier at a time:
//!
//!   ```sh
//!   PONKO_GPU_TIER=webgl2 cargo test -p ponko --lib --features 3d render_test
//!   ```
//!
//!   A test that needs a feature the forced tier lacks calls [`skip`] with
//!   the reason ([`needs`] does both); every other test must pass as on the
//!   real device. Only the capability suite opens its own forced devices.
//! - **Assertions.** Prefer semantic checks that say what must hold ("the box
//!   is lit", "the background behind it stays black", "this mode costs more
//!   than that one") with a negative control showing the check can fail.
//!   Compare images with [`diff`] and a [`Tolerance`] (per-channel slack plus
//!   a fraction of pixels allowed beyond it), not byte equality, unless the
//!   same device must produce identical output (a fallback drawing exactly
//!   like the mode it falls back to). Print the measured numbers with
//!   `eprintln!` so a failure on other hardware can be read.
//! - **Reference images.** A whole-image baseline goes through
//!   [`check_reference`]: a small PNG in `tests/fixtures/render/<name>.png`
//!   and its metadata in `<name>.json` (size, format, tolerance, the scene,
//!   the adapter it was made on, the PNG's checksum and why it was last
//!   changed). A missing or mismatching reference fails, and the actual and
//!   difference images are written to `target/render-test-output/` for
//!   inspection. Nothing is ever written to the fixtures unless asked:
//!
//!   ```sh
//!   PONKO_UPDATE_REFERENCES=1 PONKO_REFERENCE_REASON="why the picture changes" \
//!     cargo test -p ponko --lib --features 3d <test>
//!   ```
//!
//!   rewrites the references that no longer match (one that still matches
//!   is left alone), records the reason, and saves `<name>.before.png`,
//!   `<name>.after.png` and `<name>.diff.png` in `target/render-test-output/`
//!   for the pull request. It is refused when `CI` is set and without a
//!   reason. Changing a tolerance or a scene description in code without
//!   regenerating fails too, and so does a PNG swapped by hand (its
//!   checksum), so a different baseline always shows up in review; CI's
//!   reference job holds a pull request that changes one until a maintainer
//!   has looked at the before/after images. Keep them small (128x128 is
//!   plenty). `PONKO_RENDER_TEST_IMAGES=1` keeps every image a test read
//!   back, for looking at a failure.
//! - **Readback.** [`read_texture`] copies any texture back with row padding
//!   handled; [`target`] makes an offscreen colour target to draw into, and
//!   [`white_textures`] the one-white-texture set most rigs draw with.
//!
//! Every suite opens its device through these helpers (a test checks it).
//! The older suites still carry their own readback and texture set-up and
//! move over as they are touched.
use super::gpu::GpuTexture;
use std::path::PathBuf;

/// A device for one test, and what it runs on.
pub(crate) struct TestGpu {
    pub info: wgpu::AdapterInfo,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// The running test's name (libtest names each test's thread after it).
fn test_name() -> String {
    std::thread::current().name().unwrap_or("unnamed test").to_owned()
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0")
}

/// Logs that the running test skips because the adapter can't do something
/// it needs. The caller returns early.
pub(super) fn skip(reason: impl std::fmt::Display) {
    eprintln!("render test SKIPPED [{}]: {reason}", test_name());
}

/// A missing adapter or device: a skip, or a failure under `PONKO_REQUIRE_GPU`.
/// Every suite reports a failed adapter or device request through this.
pub(super) fn no_gpu(reason: impl std::fmt::Display) {
    if env_set("PONKO_REQUIRE_GPU") {
        panic!("{reason}, and PONKO_REQUIRE_GPU is set");
    }
    skip(reason);
}

/// The tier `PONKO_GPU_TIER` forces on the suites, if any.
pub(super) fn forced_tier() -> Option<crate::render::GpuTier> {
    crate::render::GpuTier::forced()
}

/// `descriptor` as the forced tier opens it (unchanged without one): that
/// tier's limits, and no timestamp queries below `desktop`. Every suite
/// opens its device through this.
pub(super) fn forced<'a>(descriptor: &wgpu::DeviceDescriptor<'a>) -> wgpu::DeviceDescriptor<'a> {
    let mut d = descriptor.clone();
    if let Some(tier) = forced_tier() {
        // CI checks for these lines: a run where nothing opened a device on
        // the tier must not pass.
        eprintln!("render test opens a device [{}] as the {tier} tier", test_name());
        d.required_limits = tier.device_limits(d.required_limits);
        d.required_features = tier.device_features(d.required_features);
    }
    d
}

/// Whether `device` has `feature`, as the renderer decides it.
pub(super) fn available(device: &wgpu::Device, feature: crate::render::GpuFeature) -> bool {
    super::capabilities::report(
        Default::default(),
        super::capabilities::GpuLimits::of(device),
        device.features().contains(wgpu::Features::TIMESTAMP_QUERY),
        (4, 4),
        true,
    )
    .available(feature)
}

/// Whether `device` has `feature`; if not, logs a skip naming the forced
/// tier (the caller returns early).
pub(super) fn needs(device: &wgpu::Device, feature: crate::render::GpuFeature) -> bool {
    if available(device, feature) {
        return true;
    }
    let tier = forced_tier().map_or_else(|| "this device".to_owned(), |t| format!("the forced {t} tier"));
    skip(format_args!("needs {feature:?}, which {tier} lacks"));
    false
}

/// A device with default limits and no optional features, or `None` (the
/// test returns early; see the module docs).
pub(crate) fn gpu() -> Option<TestGpu> {
    gpu_with(|_| Ok(wgpu::DeviceDescriptor::default()))
}

/// A device made from the descriptor `describe` picks for the adapter.
/// `describe` returns `Err(reason)` when the adapter can't run the test (a
/// capability skip, logged with that reason).
pub(super) fn gpu_with(
    describe: impl FnOnce(&wgpu::Adapter) -> Result<wgpu::DeviceDescriptor<'static>, String>,
) -> Option<TestGpu> {
    gpu_adjusted(describe, |_| {})
}

/// [`gpu_with`], with `adjust` changing the descriptor after the forced
/// tier was applied to it: for a test that takes a limit below (or sets it
/// apart from) whatever tier runs.
pub(super) fn gpu_adjusted(
    describe: impl FnOnce(&wgpu::Adapter) -> Result<wgpu::DeviceDescriptor<'static>, String>,
    adjust: impl FnOnce(&mut wgpu::DeviceDescriptor<'static>),
) -> Option<TestGpu> {
    // Like the engine's own instance, its flags follow wgpu's environment
    // variables (`WGPU_VALIDATION_INDIRECT_CALL=0` and the like).
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.flags = wgpu::InstanceFlags::from_env_or_default();
    let instance = wgpu::Instance::new(descriptor);
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) {
        Ok(adapter) => adapter,
        Err(e) => {
            no_gpu(format_args!("no GPU adapter ({e})"));
            return None;
        }
    };
    let info = adapter.get_info();
    let descriptor = match describe(&adapter) {
        Ok(descriptor) => descriptor,
        Err(reason) => {
            skip(format_args!("{reason} (on {})", info.name));
            return None;
        }
    };
    let mut descriptor = forced(&descriptor);
    adjust(&mut descriptor);
    let (device, queue) = match pollster::block_on(adapter.request_device(&descriptor)) {
        Ok(pair) => pair,
        Err(e) => {
            no_gpu(format_args!("no GPU device on {} ({e})", info.name));
            return None;
        }
    };
    match forced_tier() {
        Some(tier) => eprintln!(
            "render test ran [{}] on {} as the {tier} tier",
            test_name(),
            describe_adapter(&info)
        ),
        None => eprintln!("render test ran [{}] on {}", test_name(), describe_adapter(&info)),
    }
    Some(TestGpu { info, device, queue })
}

/// An adapter's name, backend, type and driver, for logs and metadata.
pub(super) fn describe_adapter(info: &wgpu::AdapterInfo) -> String {
    format!(
        "{} ({:?}, {:?}, driver {})",
        info.name,
        info.backend,
        info.device_type,
        driver(info)
    )
}

/// The adapter's driver and its version, or "unknown" (Metal reports none).
fn driver(info: &wgpu::AdapterInfo) -> String {
    let driver = format!("{} {}", info.driver, info.driver_info);
    match driver.trim() {
        "" => "unknown".to_owned(),
        d => d.to_owned(),
    }
}

/// The texture set most suites draw with: a bind group layout for one
/// filtered texture, a 1x1 white sRGB texture (also the renderer's default),
/// and that texture as texture 0.
pub(super) fn white_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::BindGroupLayout, wgpu::Texture, Vec<GpuTexture>) {
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty,
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            entry(
                0,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            ),
            entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        ],
    });
    let white = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
    });
    queue.write_texture(
        white.as_image_copy(),
        &[255; 4],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: None,
        },
        white.size(),
    );
    let sampler = device.create_sampler(&Default::default());
    let view = white.create_view(&Default::default());
    let textures = vec![GpuTexture {
        texture: white.clone(),
        nearest: false,
        mips: None,
        bind_group: device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        }),
    }];
    (layout, white, textures)
}

/// An offscreen colour target of `size` that can be drawn into and read back.
pub(super) fn target(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// The first mip and layer of `texture` (made with `COPY_SRC`), row by row
/// from the top with no padding. Waits for the GPU.
pub(super) fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let texel = texture
        .format()
        .block_copy_size(None)
        .expect("a format with one copyable aspect");
    let row = width * texel;
    let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = readback.get_mapped_range(..).unwrap();
    if padded == row {
        return mapped.to_vec();
    }
    mapped
        .chunks_exact(padded as usize)
        .flat_map(|r| &r[..row as usize])
        .copied()
        .collect()
}

/// An RGBA8 image read back from the GPU, row by row from the top.
#[derive(Clone, PartialEq)]
pub(super) struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

impl Image {
    /// Reads back an RGBA8 (or BGRA8, swizzled to RGBA) texture.
    pub fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Self {
        let mut rgba = read_texture(device, queue, texture);
        match texture.format() {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {}
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
                rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
            }
            other => panic!("not an 8-bit RGBA format: {other:?}"),
        }
        Self {
            width: texture.width(),
            height: texture.height(),
            rgba,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().unwrap()
    }

    /// The fraction of the pixels in the rectangle `x` by `y` for which
    /// `test` holds.
    pub fn fraction(&self, x: std::ops::Range<u32>, y: std::ops::Range<u32>, test: impl Fn([u8; 4]) -> bool) -> f32 {
        let (mut hits, mut total) = (0usize, 0usize);
        for py in y {
            for px in x.clone() {
                total += 1;
                hits += usize::from(test(self.pixel(px, py)));
            }
        }
        assert!(total > 0, "an empty region");
        hits as f32 / total as f32
    }
}

/// Asserts that `test` holds for at least `at_least` (0..=1) of the pixels
/// in the rectangle `x` by `y`: a semantic check ("the box is lit").
pub(super) fn assert_region(
    image: &Image,
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
    at_least: f32,
    what: &str,
    test: impl Fn([u8; 4]) -> bool,
) {
    let (xs, ys) = (x.clone(), y.clone());
    let got = image.fraction(x, y, test);
    assert!(
        got >= at_least,
        "{what}: holds for {:.1}% of pixels {xs:?} x {ys:?}, wanted at least {:.1}%",
        got * 100.0,
        at_least * 100.0
    );
}

/// How far an image may stray from what it's compared with: each channel by
/// up to `channel`, and beyond that on at most `outliers` (0..=1) of the
/// pixels.
///
/// `edge` is for comparing frames of two adapters. Rasterisers put an edge
/// (a silhouette, a shadow's border) up to a pixel apart and resolve its
/// samples differently, so along edges the same picture differs by the
/// whole contrast of the edge. With `edge` 1 a pixel is compared with the
/// other image's pixels up to one pixel away in each direction: it is fine
/// when every channel lies within `channel` of the range those pixels
/// span, and the same is asked of the other image's pixel against this
/// image's neighbours (so a thin line missing on either side still
/// counts). Flat areas are compared as with `edge` 0; an edge that moved
/// further than `edge` pixels, or a shade that changed, is beyond the
/// tolerance. `outliers` then only has to cover what isn't an edge.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Tolerance {
    pub channel: u8,
    pub outliers: f32,
    /// How many pixels an edge may sit apart (0: pixels are compared in
    /// place).
    #[serde(default)]
    pub edge: u32,
}

/// How two images of the same size differ.
#[derive(Clone, Copy, Debug)]
pub(super) struct Diff {
    /// The largest difference of any channel.
    pub max_channel: u8,
    /// The mean absolute difference over every channel, 0..=255.
    pub mean: f32,
    /// Pixels with a channel differing by more than the tolerance's (after
    /// the tolerance's allowance for edges).
    pub outliers: usize,
    pub pixels: usize,
    /// The largest difference left after the allowance for edges: what
    /// `channel` would have to be for no pixel to be beyond it.
    pub beyond: u8,
    /// Pixels of the second image that lie on an edge: a neighbour within
    /// the tolerance's `edge` differs from them by more than `channel`.
    /// (0 without an allowance for edges.)
    pub edges: usize,
}

impl Diff {
    pub fn within(&self, tolerance: Tolerance) -> bool {
        self.outliers as f32 <= tolerance.outliers * self.pixels as f32
    }
}

impl std::fmt::Display for Diff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let share = |n: usize| n as f32 * 100.0 / self.pixels.max(1) as f32;
        write!(
            f,
            "max channel difference {} ({} off edges), mean {:.3}, {} of {} pixels ({:.2}%) beyond the tolerance, \
             {:.1}% of pixels on edges",
            self.max_channel,
            self.beyond,
            self.mean,
            self.outliers,
            self.pixels,
            share(self.outliers),
            share(self.edges)
        )
    }
}

/// How `a` differs from `b`, counting pixels with a channel off by more than
/// `channel`, each compared in place.
pub(super) fn diff(a: &Image, b: &Image, channel: u8) -> Diff {
    diff_within(
        a,
        b,
        Tolerance {
            channel,
            outliers: 0.0,
            edge: 0,
        },
    )
}

/// How far `pixel` is outside the range that `other`'s pixels up to `reach`
/// away from (x, y) span, as the worst channel; and that range's own width.
fn outside(pixel: &[u8], other: &Image, x: usize, y: usize, reach: usize) -> (u8, u8) {
    let (width, height) = (other.width as usize, other.height as usize);
    let (mut low, mut high) = ([255u8; 4], [0u8; 4]);
    for ny in y.saturating_sub(reach)..=(y + reach).min(height - 1) {
        for nx in x.saturating_sub(reach)..=(x + reach).min(width - 1) {
            let at = (ny * width + nx) * 4;
            for c in 0..4 {
                low[c] = low[c].min(other.rgba[at + c]);
                high[c] = high[c].max(other.rgba[at + c]);
            }
        }
    }
    let excess = (0..4)
        .map(|c| low[c].saturating_sub(pixel[c]).max(pixel[c].saturating_sub(high[c])))
        .max()
        .unwrap();
    (excess, (0..4).map(|c| high[c] - low[c]).max().unwrap())
}

/// What the pixel at (x, y) differs by after the allowance for edges (each
/// image's pixel against the other's neighbours, the worse of the two),
/// and whether `b` has an edge there wider than `channel`.
fn excess(a: &Image, b: &Image, x: usize, y: usize, tolerance: Tolerance) -> (u8, bool) {
    let (at, reach) = ((y * a.width as usize + x) * 4, tolerance.edge as usize);
    let (p, q) = (&a.rgba[at..at + 4], &b.rgba[at..at + 4]);
    let (ours, span) = outside(p, b, x, y, reach);
    (ours.max(outside(q, a, x, y, reach).0), span > tolerance.channel)
}

/// How `a` differs from `b` under `tolerance`: its `channel`, and its
/// allowance for edges (see [`Tolerance`]).
pub(super) fn diff_within(a: &Image, b: &Image, tolerance: Tolerance) -> Diff {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "comparing images of different sizes"
    );
    let (width, height) = (a.width as usize, a.height as usize);
    let (mut max_channel, mut sum, mut outliers, mut beyond, mut edges) = (0u8, 0u64, 0usize, 0u8, 0usize);
    for y in 0..height {
        for x in 0..width {
            let at = (y * width + x) * 4;
            let (p, q) = (&a.rgba[at..at + 4], &b.rgba[at..at + 4]);
            sum += p.iter().zip(q).map(|(x, y)| u64::from(x.abs_diff(*y))).sum::<u64>();
            max_channel = max_channel.max(p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap());
            let (left, edge) = excess(a, b, x, y, tolerance);
            beyond = beyond.max(left);
            outliers += usize::from(left > tolerance.channel);
            edges += usize::from(edge);
        }
    }
    let pixels = a.rgba.len() / 4;
    Diff {
        max_channel,
        mean: sum as f32 / (pixels * 4).max(1) as f32,
        outliers,
        pixels,
        beyond,
        edges,
    }
}

/// Prints `line` and appends it to `comparisons.log` in `output`: test
/// runners hide what a passing test prints, and the margins a comparison
/// passed with are what a first run on other hardware is read for.
pub(super) fn record(output: &std::path::Path, line: &str) {
    use std::io::Write;
    eprintln!("{line}");
    let written = std::fs::create_dir_all(output).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(output.join("comparisons.log"))
            .and_then(|mut log| writeln!(log, "[{}] {line}", test_name()))
    });
    if let Err(e) = written {
        eprintln!("(not logged in {}: {e})", output.display());
    }
}

/// A reference image's metadata, stored next to it as `<name>.json`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct ReferenceMeta {
    pub name: String,
    /// What the picture shows and how it's set up, for reviewers.
    pub scene: String,
    pub width: u32,
    pub height: u32,
    /// Always 8-bit RGBA, sRGB-encoded.
    pub format: String,
    pub tolerance: Tolerance,
    /// The adapter the reference was made on.
    pub adapter: String,
    pub backend: String,
    pub driver: String,
    /// FNV-1a (64 bits, hex) of the PNG file: an image replaced without
    /// going through the update fails the check.
    pub checksum: String,
    /// Why the image was last made or changed (`PONKO_REFERENCE_REASON`).
    pub reason: String,
}

/// FNV-1a over `bytes`, as 16 hex digits (`scripts/check-reference-images.py`
/// computes the same).
pub(super) fn checksum(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The checkout's root: where the test runs from when the runner says so
/// (an archived test binary run on another machine), else where it was
/// built.
fn manifest_dir() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.join("tests/fixtures/render").is_dir())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn fixtures() -> PathBuf {
    manifest_dir().join("tests/fixtures/render")
}

/// Where failing comparisons leave their images.
pub(super) fn output_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir().join("target"))
        .join("render-test-output")
}

fn save_png(path: &std::path::Path, image: &Image) {
    image::save_buffer(path, &image.rgba, image.width, image.height, image::ColorType::Rgba8)
        .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

/// With `PONKO_RENDER_TEST_IMAGES=1`, saves `image` as
/// `target/render-test-output/<test>.<n>.png` (`n` counts the test's images
/// in the order it read them): what a test saw, for looking at a failure
/// or showing a change. Does nothing otherwise.
pub(super) fn keep_image(image: &Image) {
    use std::cell::Cell;
    thread_local!(static COUNT: Cell<u32> = const { Cell::new(0) });
    if !env_set("PONKO_RENDER_TEST_IMAGES") {
        return;
    }
    let n = COUNT.with(|c| c.replace(c.get() + 1));
    let name = test_name().replace("::", ".");
    let out = output_dir();
    std::fs::create_dir_all(&out).unwrap();
    save_png(&out.join(format!("{name}.{n:02}.png")), image);
}

/// Whether this run may rewrite reference images, and why it does:
/// `Ok(None)` for a normal run, `Ok(Some(reason))` for an update, `Err` for
/// an update that is not allowed (in CI, or without a reason).
pub(super) fn update_request(update: bool, reason: Option<&str>, ci: bool) -> Result<Option<String>, String> {
    if !update {
        return Ok(None);
    }
    if ci {
        return Err(
            "PONKO_UPDATE_REFERENCES is refused in CI: regenerate references locally and commit them for review"
                .to_owned(),
        );
    }
    match reason.map(str::trim) {
        Some(reason) if !reason.is_empty() => Ok(Some(reason.to_owned())),
        _ => Err(
            "PONKO_UPDATE_REFERENCES needs PONKO_REFERENCE_REASON=\"why the picture changes\": it is recorded \
             in the reference's metadata for the reviewer"
                .to_owned(),
        ),
    }
}

/// Who drew an image: the adapter's name, backend and driver.
#[derive(Clone, Debug)]
pub(super) struct Drawn {
    pub adapter: String,
    pub backend: String,
    pub driver: String,
}

impl Drawn {
    pub fn by(gpu: &wgpu::AdapterInfo) -> Self {
        Self {
            adapter: gpu.name.clone(),
            backend: format!("{:?}", gpu.backend),
            driver: driver(gpu),
        }
    }
}

/// Where the references are and where evidence and failure images go.
pub(super) struct ReferenceDirs {
    pub fixtures: PathBuf,
    pub output: PathBuf,
}

/// What a reference check did.
#[derive(Debug, PartialEq)]
pub(super) enum Checked {
    /// The image is within the tolerance of its reference.
    Matches,
    /// An update run: the image already matched, nothing was written.
    LeftAlone,
    /// An update run: the reference was made or replaced.
    Written,
}

/// `image` with the pixels beyond `tolerance` of `reference` in red and the
/// rest as dim grey.
fn difference_image(image: &Image, reference: &Image, tolerance: Tolerance) -> Image {
    let mut shown = image.clone();
    let width = image.width as usize;
    for y in 0..image.height as usize {
        for x in 0..width {
            let beyond = excess(image, reference, x, y, tolerance).0 > tolerance.channel;
            let p = &mut shown.rgba[(y * width + x) * 4..][..4];
            let grey = ((u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 12) as u8;
            p.copy_from_slice(&if beyond {
                [255, 0, 0, 255]
            } else {
                [grey, grey, grey, 255]
            });
        }
    }
    shown
}

/// A negative control for a reference: `image`, a frame of the same scene
/// with what the reference is there to protect taken away (`what`: the
/// effect off, the object moved), must be beyond `tolerance` of the stored
/// reference, by at least twice its budget of outliers. A tolerance wide
/// enough to let it through compares nothing.
pub(super) fn reference_rejects(name: &str, image: &Image, tolerance: Tolerance, what: &str) {
    let bytes = std::fs::read(fixtures().join(format!("{name}.png")))
        .unwrap_or_else(|e| panic!("no reference image {name}.png ({e})"));
    let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
    let reference = Image {
        width: decoded.width(),
        height: decoded.height(),
        rgba: decoded.into_raw(),
    };
    let d = diff_within(image, &reference, tolerance);
    record(
        &output_dir(),
        &format!("reference {name} against {what} (must differ): {d}; allowed {tolerance:?}"),
    );
    // Twice the budget: a control that only just fails says the budget is
    // as wide as the regression it is there to catch.
    assert!(
        d.outliers as f32 >= 2.0 * tolerance.outliers * d.pixels as f32 && !d.within(tolerance),
        "{name}: {what} is not at least twice beyond the tolerance of the reference ({d}, allowed {tolerance:?}): \
         the comparison would let it through or nearly so"
    );
}

/// Compares `image` with the reference `tests/fixtures/render/<name>.png`
/// within `tolerance` (see the module docs). `scene` describes the setup for
/// the metadata; `gpu` is the adapter that drew `image`.
pub(super) fn check_reference(name: &str, image: &Image, tolerance: Tolerance, scene: &str, gpu: &wgpu::AdapterInfo) {
    let reason = std::env::var("PONKO_REFERENCE_REASON").ok();
    let update = update_request(env_set("PONKO_UPDATE_REFERENCES"), reason.as_deref(), env_set("CI"))
        .unwrap_or_else(|e| panic!("{e}"));
    let dirs = ReferenceDirs {
        fixtures: fixtures(),
        output: output_dir(),
    };
    if let Err(e) = check_reference_in(&dirs, name, image, tolerance, scene, &Drawn::by(gpu), update.as_deref()) {
        panic!("{e}");
    }
}

/// [`check_reference`] in `dirs`, as an update with `update`'s reason or
/// as a check; `Err` is the failure's message.
pub(super) fn check_reference_in(
    dirs: &ReferenceDirs,
    name: &str,
    image: &Image,
    tolerance: Tolerance,
    scene: &str,
    drawn: &Drawn,
    update: Option<&str>,
) -> Result<Checked, String> {
    let (png, json) = (
        dirs.fixtures.join(format!("{name}.png")),
        dirs.fixtures.join(format!("{name}.json")),
    );
    let how = "regenerate locally with PONKO_UPDATE_REFERENCES=1 PONKO_REFERENCE_REASON=\"...\" and review the \
               new image (see the book's \"GPU regression tests\")";
    // What is there now, if anything.
    let existing = || -> Result<(ReferenceMeta, Image), String> {
        let text =
            std::fs::read_to_string(&json).map_err(|e| format!("no reference metadata {} ({e})", json.display()))?;
        let meta: ReferenceMeta = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", json.display()))?;
        let bytes = std::fs::read(&png).map_err(|e| format!("no reference image {} ({e})", png.display()))?;
        if checksum(&bytes) != meta.checksum {
            return Err(format!(
                "{} is not the image its metadata was made with (checksum {}, metadata says {})",
                png.display(),
                checksum(&bytes),
                meta.checksum
            ));
        }
        let decoded = image::load_from_memory(&bytes)
            .map_err(|e| format!("{}: {e}", png.display()))?
            .into_rgba8();
        let reference = Image {
            width: decoded.width(),
            height: decoded.height(),
            rgba: decoded.into_raw(),
        };
        if (meta.width, meta.height) != (reference.width, reference.height) {
            return Err(format!(
                "{}: the metadata's size doesn't match the image",
                json.display()
            ));
        }
        Ok((meta, reference))
    };
    // Whether `image` is what the reference holds, as this code describes it.
    let compare = |meta: &ReferenceMeta, reference: &Image| -> Result<Diff, String> {
        if meta.tolerance != tolerance {
            return Err(format!(
                "{name}: the tolerance in code ({tolerance:?}) differs from the reference's metadata ({:?})",
                meta.tolerance
            ));
        }
        if meta.scene != scene {
            return Err(format!(
                "{name}: the scene described in code differs from the reference's metadata"
            ));
        }
        if (image.width, image.height) != (reference.width, reference.height) {
            return Err(format!(
                "{name}: drawn at {}x{}, its reference is {}x{}",
                image.width, image.height, reference.width, reference.height
            ));
        }
        Ok(diff_within(image, reference, tolerance))
    };

    if let Some(reason) = update {
        let before = existing();
        if let Ok((meta, reference)) = &before
            && compare(meta, reference).is_ok_and(|d| d.within(tolerance))
        {
            eprintln!("reference {name}: still matches, left alone");
            return Ok(Checked::LeftAlone);
        }
        std::fs::create_dir_all(&dirs.fixtures).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dirs.output).map_err(|e| e.to_string())?;
        // The evidence a reviewer needs: what it was, what it is, where
        // they differ.
        let after = dirs.output.join(format!("{name}.after.png"));
        save_png(&after, image);
        let mut evidence = format!("after {}", after.display());
        if let Ok((meta, reference)) = &before {
            let old = dirs.output.join(format!("{name}.before.png"));
            save_png(&old, reference);
            evidence = format!("before {}, {evidence}", old.display());
            if (image.width, image.height) == (reference.width, reference.height) {
                let changed = dirs.output.join(format!("{name}.diff.png"));
                save_png(&changed, &difference_image(image, reference, tolerance));
                evidence = format!(
                    "{evidence}, difference {} ({}; made on {}, {})",
                    changed.display(),
                    diff_within(image, reference, tolerance),
                    meta.adapter,
                    meta.backend
                );
            }
        }
        // A reference whose picture still matches but whose description or
        // tolerance changed keeps its image: only the metadata is rewritten.
        let same_picture = before.as_ref().is_ok_and(|(_, reference)| {
            (image.width, image.height) == (reference.width, reference.height)
                && diff_within(image, reference, tolerance).within(tolerance)
        });
        if same_picture {
            evidence = format!("the image is unchanged, only its metadata; {evidence}");
        } else {
            save_png(&png, image);
        }
        let bytes = std::fs::read(&png).map_err(|e| e.to_string())?;
        let meta = ReferenceMeta {
            name: name.to_owned(),
            scene: scene.to_owned(),
            width: image.width,
            height: image.height,
            format: "rgba8-srgb".to_owned(),
            tolerance,
            adapter: drawn.adapter.clone(),
            backend: drawn.backend.clone(),
            driver: drawn.driver.clone(),
            checksum: checksum(&bytes),
            reason: reason.to_owned(),
        };
        std::fs::write(&json, serde_json::to_string_pretty(&meta).unwrap() + "\n").map_err(|e| e.to_string())?;
        eprintln!(
            "updated reference {}: look at it, and put the images in the pull request ({evidence})",
            png.display()
        );
        return Ok(Checked::Written);
    }

    let (meta, reference) = existing().map_err(|e| format!("{e}: {how}"))?;
    let d = compare(&meta, &reference).map_err(|e| format!("{e}; {how}"))?;
    record(
        &dirs.output,
        &format!(
            "reference {name}: {d}; allowed {tolerance:?} (made on {}, {}; drawn on {}, {})",
            meta.adapter, meta.backend, drawn.adapter, drawn.backend
        ),
    );
    if d.within(tolerance) {
        return Ok(Checked::Matches);
    }
    std::fs::create_dir_all(&dirs.output).map_err(|e| e.to_string())?;
    let actual = dirs.output.join(format!("{name}.actual.png"));
    let difference = dirs.output.join(format!("{name}.diff.png"));
    save_png(&actual, image);
    save_png(&difference, &difference_image(image, &reference, tolerance));
    Err(format!(
        "{name} differs from its reference: {d}, allowed {tolerance:?}. Reference made on {} ({}, driver {}), \
         drawn on {} ({}, driver {}). See {} and {}; if the change is intended, {how}",
        meta.adapter,
        meta.backend,
        meta.driver,
        drawn.adapter,
        drawn.backend,
        drawn.driver,
        actual.display(),
        difference.display()
    ))
}

#[test]
fn diff_counts_pixels_beyond_the_channel_tolerance() {
    let a = Image {
        width: 2,
        height: 2,
        rgba: vec![0, 0, 0, 255, 10, 10, 10, 255, 100, 100, 100, 255, 255, 255, 255, 255],
    };
    let mut b = a.clone();
    b.rgba[4] = 13; // within 3
    b.rgba[8] = 120; // beyond
    let d = diff(&a, &b, 3);
    assert_eq!((d.max_channel, d.outliers, d.pixels), (20, 1, 4));
    assert!(d.within(Tolerance {
        channel: 3,
        outliers: 0.25,
        edge: 0,
    }));
    assert!(!d.within(Tolerance {
        channel: 3,
        outliers: 0.2,
        edge: 0,
    }));
    assert!((d.mean - 23.0 / 16.0).abs() < 1e-6);
}

/// The allowance for edges: an edge a pixel apart is the same picture, one
/// two pixels apart is not, and neither a changed shade nor a thin line
/// missing from either image hides in it.
#[test]
fn an_edge_a_pixel_apart_is_within_the_tolerance_and_a_changed_shade_is_not() {
    // 16x16, black left of column `edge` and white from it on.
    let split = |edge: u32| Image {
        width: 16,
        height: 16,
        rgba: (0..16 * 16)
            .flat_map(|i| if i % 16 < edge { [0, 0, 0, 255] } else { [255; 4] })
            .collect(),
    };
    let (in_place, near) = (
        Tolerance {
            channel: 8,
            outliers: 0.0,
            edge: 0,
        },
        Tolerance {
            channel: 8,
            outliers: 0.0,
            edge: 1,
        },
    );
    let (reference, moved_one, moved_two) = (split(8), split(9), split(10));
    assert_eq!(diff_within(&moved_one, &reference, in_place).outliers, 16);
    let d = diff_within(&moved_one, &reference, near);
    assert_eq!((d.outliers, d.beyond, d.max_channel), (0, 0, 255), "{d}");
    // Two columns of the reference touch its edge.
    assert_eq!(d.edges, 32, "{d}");
    assert!(d.within(near));
    // Two pixels apart: the column that is neither image's neighbour.
    let d = diff_within(&moved_two, &reference, near);
    assert_eq!(d.outliers, 32, "{d}");
    assert!(!d.within(near));
    // An antialiased edge: a shade between the two sides is within.
    let mut soft = split(8);
    for y in 0..16 {
        soft.rgba[(y * 16 + 8) * 4..][..3].fill(120);
    }
    assert_eq!(diff_within(&soft, &reference, near).outliers, 0);
    // A shade changed away from any edge is beyond it as before.
    let mut shaded = split(8);
    for y in 0..16 {
        shaded.rgba[(y * 16 + 2) * 4..][..3].fill(40);
    }
    let d = diff_within(&shaded, &reference, near);
    assert_eq!((d.outliers, d.beyond), (16, 40), "{d}");
    // A one-pixel line the reference has and the frame lacks, and the
    // other way round, both count.
    let mut line = split(8);
    for x in 0..5 {
        line.rgba[(12 * 16 + x) * 4..][..3].fill(200);
    }
    assert_eq!(diff_within(&reference, &line, near).outliers, 5);
    assert_eq!(diff_within(&line, &reference, near).outliers, 5);
    // The difference image marks the same pixels.
    let marked = difference_image(&line, &reference, near);
    let red = marked
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| **p == [255, 0, 0, 255])
        .count();
    assert_eq!(red, 5);
}

#[test]
fn semantic_regions_report_the_fraction_that_holds() {
    let image = Image {
        width: 2,
        height: 1,
        rgba: vec![255, 0, 0, 255, 0, 0, 0, 255],
    };
    assert_eq!(image.fraction(0..2, 0..1, |p| p[0] > 128), 0.5);
    assert_region(&image, 0..1, 0..1, 1.0, "the left pixel is red", |p| p[0] > 128);
}

/// A 4x4 grey image, and what a workflow test checks references with.
#[cfg(test)]
mod reference_workflow {
    use super::*;

    const TOLERANCE: Tolerance = Tolerance {
        channel: 4,
        outliers: 0.0,
        edge: 0,
    };

    fn grey(level: u8) -> Image {
        Image {
            width: 4,
            height: 4,
            rgba: [level, level, level, 255].repeat(16),
        }
    }

    fn drawn() -> Drawn {
        Drawn {
            adapter: "Test adapter".to_owned(),
            backend: "None".to_owned(),
            driver: "unknown".to_owned(),
        }
    }

    /// Empty fixture and output folders of this test's own.
    fn dirs() -> ReferenceDirs {
        let root = output_dir().join("workflow-tests").join(test_name().replace("::", "."));
        let _ = std::fs::remove_dir_all(&root);
        ReferenceDirs {
            fixtures: root.join("fixtures"),
            output: root.join("output"),
        }
    }

    fn check(dirs: &ReferenceDirs, image: &Image, update: Option<&str>) -> Result<Checked, String> {
        check_reference_in(dirs, "sample", image, TOLERANCE, "A grey square.", &drawn(), update)
    }

    #[test]
    fn a_missing_reference_fails_and_says_how_to_make_one() {
        let dirs = dirs();
        let e = check(&dirs, &grey(100), None).unwrap_err();
        assert!(
            e.contains("no reference metadata") && e.contains("PONKO_UPDATE_REFERENCES=1"),
            "{e}"
        );
        assert!(!dirs.fixtures.exists(), "a check writes nothing");
    }

    #[test]
    fn an_update_is_refused_in_ci_and_without_a_reason() {
        assert_eq!(update_request(false, None, true), Ok(None));
        assert_eq!(update_request(false, Some("ignored"), false), Ok(None));
        assert_eq!(
            update_request(true, Some(" new shadow filter "), false),
            Ok(Some("new shadow filter".to_owned()))
        );
        let in_ci = update_request(true, Some("new shadow filter"), true).unwrap_err();
        assert!(in_ci.contains("refused in CI"), "{in_ci}");
        for reason in [None, Some(""), Some("  ")] {
            let e = update_request(true, reason, false).unwrap_err();
            assert!(e.contains("PONKO_REFERENCE_REASON"), "{e}");
        }
    }

    #[test]
    fn an_update_records_its_reason_and_leaves_before_after_and_difference_images() {
        let dirs = dirs();
        assert_eq!(check(&dirs, &grey(100), Some("first picture")), Ok(Checked::Written));
        assert_eq!(check(&dirs, &grey(100), None), Ok(Checked::Matches));
        // A first reference has nothing before it.
        assert!(dirs.output.join("sample.after.png").exists());
        assert!(!dirs.output.join("sample.before.png").exists());

        assert_eq!(check(&dirs, &grey(180), Some("brighter light")), Ok(Checked::Written));
        for evidence in ["sample.before.png", "sample.after.png", "sample.diff.png"] {
            assert!(dirs.output.join(evidence).exists(), "{evidence} is saved");
        }
        let before = image::open(dirs.output.join("sample.before.png")).unwrap().into_rgba8();
        assert_eq!(before.get_pixel(0, 0).0, [100, 100, 100, 255], "the old picture");
        let meta: ReferenceMeta =
            serde_json::from_str(&std::fs::read_to_string(dirs.fixtures.join("sample.json")).unwrap()).unwrap();
        assert_eq!(meta.reason, "brighter light");
        assert_eq!((meta.adapter.as_str(), meta.width, meta.height), ("Test adapter", 4, 4));
        assert_eq!(
            meta.checksum,
            checksum(&std::fs::read(dirs.fixtures.join("sample.png")).unwrap())
        );
        assert_eq!(check(&dirs, &grey(180), None), Ok(Checked::Matches));
    }

    #[test]
    fn an_update_leaves_a_reference_that_still_matches_alone() {
        let dirs = dirs();
        check(&dirs, &grey(100), Some("first picture")).unwrap();
        let written = std::fs::read(dirs.fixtures.join("sample.json")).unwrap();
        // Within the tolerance: no churn, and the reason on record stays.
        assert_eq!(
            check(&dirs, &grey(102), Some("an unrelated update run")),
            Ok(Checked::LeftAlone)
        );
        assert_eq!(std::fs::read(dirs.fixtures.join("sample.json")).unwrap(), written);
    }

    #[test]
    fn a_frame_beyond_the_tolerance_fails_naming_both_adapters_and_leaves_the_images() {
        let dirs = dirs();
        check(&dirs, &grey(100), Some("first picture")).unwrap();
        assert_eq!(
            check(&dirs, &grey(104), None),
            Ok(Checked::Matches),
            "within the tolerance"
        );
        let e = check(&dirs, &grey(105), None).unwrap_err();
        assert!(
            e.contains("differs from its reference") && e.contains("max channel difference 5"),
            "{e}"
        );
        assert!(
            e.contains("Reference made on Test adapter") && e.contains("drawn on Test adapter"),
            "{e}"
        );
        assert!(dirs.output.join("sample.actual.png").exists() && dirs.output.join("sample.diff.png").exists());
        // The reference itself is untouched by a failing check.
        assert_eq!(check(&dirs, &grey(100), None), Ok(Checked::Matches));
    }

    #[test]
    fn a_changed_tolerance_scene_or_size_fails_until_regenerated() {
        let dirs = dirs();
        check(&dirs, &grey(100), Some("first picture")).unwrap();
        let looser = Tolerance {
            channel: 40,
            outliers: 0.5,
            edge: 0,
        };
        let e = check_reference_in(&dirs, "sample", &grey(100), looser, "A grey square.", &drawn(), None).unwrap_err();
        assert!(e.contains("tolerance in code"), "{e}");
        let e =
            check_reference_in(&dirs, "sample", &grey(100), TOLERANCE, "Another scene.", &drawn(), None).unwrap_err();
        assert!(e.contains("scene described in code"), "{e}");
        let wide = Image {
            width: 8,
            height: 2,
            rgba: [100, 100, 100, 255].repeat(16),
        };
        let e = check(&dirs, &wide, None).unwrap_err();
        assert!(e.contains("drawn at 8x2"), "{e}");
        // Regenerating with the looser tolerance records it.
        assert_eq!(
            check_reference_in(
                &dirs,
                "sample",
                &grey(100),
                looser,
                "A grey square.",
                &drawn(),
                Some("looser")
            ),
            Ok(Checked::Written)
        );
    }

    #[test]
    fn an_image_swapped_without_its_metadata_fails() {
        let dirs = dirs();
        check(&dirs, &grey(100), Some("first picture")).unwrap();
        save_png(&dirs.fixtures.join("sample.png"), &grey(101));
        let e = check(&dirs, &grey(101), None).unwrap_err();
        assert!(e.contains("not the image its metadata was made with"), "{e}");
    }

    #[test]
    fn the_checksum_is_fnv1a_64() {
        // The published test vectors, which the CI script's copy shares.
        assert_eq!(checksum(b""), "cbf29ce484222325");
        assert_eq!(checksum(b"a"), "af63dc4c8601ec8c");
        assert_eq!(checksum(b"foobar"), "85944171f73967e8");
    }

    /// Every `*_render_test.rs` here, with its text.
    fn suites() -> Vec<(String, String)> {
        let dir = manifest_dir().join("src/render");
        let mut suites: Vec<(String, String)> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                let name = path.file_name()?.to_str()?.to_owned();
                name.ends_with("_render_test.rs")
                    .then(|| (name, std::fs::read_to_string(&path).unwrap()))
            })
            .collect();
        suites.sort();
        suites
    }

    /// Every Rust file under `folder` (relative to the package), with its text.
    fn sources(folder: &str) -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, root: &std::path::Path, found: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let name = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                    found.push((name, std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let root = manifest_dir();
        let mut found = Vec::new();
        walk(&root.join(folder), &root, &mut found);
        found.sort();
        found
    }

    /// The files that may open a GPU device themselves, and why.
    const OWN_DEVICE: [(&str, &str); 8] = [
        ("src/render/gpu.rs", "the engine's own device"),
        ("src/render/render_test_support.rs", "the shared helpers"),
        (
            "src/render/capabilities_render_test.rs",
            "forces its own tiers on the adapter, and reports a missing one through no_gpu",
        ),
        ("src/render/lights_bench.rs", "an ignored benchmark, run by hand"),
        ("src/render/gpu3d/record_bench.rs", "benchmark support, not a test"),
        (
            "src/render/gpu3d/scene_bench.rs",
            "benchmark support: `SceneBench::new` opens the high-performance adapter the benchmark reports; \
             its tests open theirs with gpu_with",
        ),
        (
            "src/render/gpu3d/lod_bench.rs",
            "benchmark support: `LodBench::new` opens the high-performance adapter the benchmark reports \
             and applies PONKO_GPU_TIER; its test reports a missing one through no_gpu",
        ),
        (
            "src/render/lights2d_bench.rs",
            "benchmark support: `Lights2DBench::new` opens the high-performance adapter the benchmark \
             reports and applies PONKO_GPU_TIER; its test reports a missing one through no_gpu",
        ),
    ];

    /// Not only the `*_render_test.rs` suites: a GPU test inside any module
    /// (a `mod tests` next to the code it checks) follows the same rule.
    #[test]
    fn every_suite_opens_its_device_through_the_shared_helpers() {
        let suites = suites();
        assert!(suites.len() >= 30, "the suites are found: {}", suites.len());
        let mut files = sources("src");
        files.extend(sources("tests"));
        assert!(files.len() > suites.len(), "the sources are found: {}", files.len());
        for (name, why) in OWN_DEVICE {
            let (_, text) = files
                .iter()
                .find(|(file, _)| file == name)
                .unwrap_or_else(|| panic!("{name} ({why}) is gone: take it off the list"));
            assert!(
                text.contains("request_adapter"),
                "{name} ({why}) opens no device any more: take it off the list"
            );
        }
        let capabilities = &files.iter().find(|(file, _)| file == OWN_DEVICE[2].0).unwrap().1;
        assert!(
            capabilities.contains("render_test_support::no_gpu"),
            "the capability suite reports a missing adapter"
        );
        for (name, text) in &files {
            if OWN_DEVICE.iter().any(|(file, _)| file == name) {
                continue;
            }
            for direct in ["request_adapter", "request_device", "wgpu::Instance::new"] {
                assert!(
                    !text.contains(direct),
                    "{name} calls {direct} itself: open the device with render_test_support::gpu or gpu_with, \
                     so a missing adapter is logged, PONKO_REQUIRE_GPU fails it and PONKO_GPU_TIER applies"
                );
            }
            assert!(
                !text.contains(": skipped\""),
                "{name} prints its own skip: call render_test_support::skip with the reason"
            );
        }
    }

    #[test]
    fn every_reference_image_is_checked_by_a_test_and_described() {
        let all: String = suites().into_iter().map(|(_, text)| text).collect();
        let mut names = Vec::new();
        for entry in std::fs::read_dir(fixtures()).unwrap() {
            let path = entry.unwrap().path();
            let stem = path.file_stem().unwrap().to_str().unwrap().to_owned();
            match path.extension().and_then(|e| e.to_str()) {
                Some("json") => {
                    let meta: ReferenceMeta = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
                        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    assert_eq!(meta.name, stem, "{}", path.display());
                    assert!(meta.scene.len() > 40, "{stem}: the scene is described for reviewers");
                    assert!(!meta.reason.trim().is_empty(), "{stem}: why it was made is on record");
                    assert!(meta.width <= 256 && meta.height <= 256, "{stem}: references stay small");
                    assert!(path.with_extension("png").exists(), "{stem}.json has its image");
                    names.push(stem);
                }
                Some("png") => assert!(path.with_extension("json").exists(), "{stem}.png has its metadata"),
                _ => panic!(
                    "{}: only reference images and their metadata belong here",
                    path.display()
                ),
            }
        }
        assert!(names.len() >= 10, "the references are found: {names:?}");
        for name in names {
            assert!(
                all.contains(&format!("\"{name}\"")),
                "{name} is checked by no test: remove it, or check it with check_reference"
            );
        }
    }
}
