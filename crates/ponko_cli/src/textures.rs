//! `ponko textures`: source images (PNG, JPEG) to GPU-compressed KTX2
//! files, with per-texture settings from a `textures.json` next to them.
//!
//! Every texture is encoded once with Basis Universal's UASTC encoder (with
//! its mipmaps), then written either as BC7, BC5 or BC4 blocks, which
//! desktop GPUs and the web sample as they are, or kept as UASTC (`"basis":
//! true`) for the engine to transcode at load to whatever the GPU has. UI
//! images stay uncompressed RGBA8. Levels are Zstandard-supercompressed.

use basis_universal::{
    BasisTextureFormat, ColorSpace, Compressor, CompressorParams, DecodeFlags, LowLevelUastcTranscoder,
    SliceParametersUastc, TranscoderBlockFormat, UASTC_QUALITY_DEFAULT,
};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
ponko textures: compress a folder's images into KTX2 textures

Usage:
  ponko textures [<folder>] [--out <folder>] [--force] [--check]

Converts every PNG and JPEG in <folder> (default: assets) and its
subfolders into a .ktx2 file of the same name, in the same place or under
--out. Load them like any texture: ctx.gfx.load_texture(\"bricks.ktx2\")
(the engine's `compressed-textures` feature).

  --out <folder>  Write the .ktx2 files here instead, in the same subfolders
  --force         Convert every image, even those whose .ktx2 is up to date
  --check         Convert nothing; fail if any .ktx2 is missing or older than
                  its image or the settings (for CI)

Settings: <folder>/textures.json (optional) gives each image a kind:
  {
    \"default\": \"color\",
    \"textures\": {
      \"*_normal.png\": \"normal\",
      \"*_orm.png\": \"linear\",
      \"ui/*\": \"ui\",
      \"hero.png\": { \"kind\": \"color\", \"basis\": true }
    }
  }
Kinds:
  color   Colors (sRGB): BC7, a quarter of RGBA8's video memory
  linear  Data that isn't a color (ORM, roughness, flow): BC7, unencoded
  normal  Normal maps: X and Y in BC5 (the shader rebuilds Z)
  mask    One channel (red): BC4, an eighth of RGBA8
  ui      UI and pixel art: RGBA8, uncompressed, no mipmaps
  skip    Not converted
A key is a path relative to <folder>, where * matches within a folder
name and ** across folders; an exact path wins over patterns, and a
longer pattern over a shorter one. Options of the { } form:
  \"basis\": true      Basis Universal UASTC instead of BCn: transcoded at load
                     to BC7, ASTC or ETC2 (needs the engine's
                     `basis-universal` feature; not in web builds)
  \"mipmaps\": false   Only the full-size level

Width and height should be multiples of 4: GPUs sample other sizes only
after the engine decodes them to RGBA8.";

/// The parsed arguments of `ponko textures`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub folder: PathBuf,
    pub out: Option<PathBuf>,
    pub force: bool,
    pub check: bool,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Options, String> {
        let mut options = Options {
            folder: PathBuf::from("assets"),
            out: None,
            force: false,
            check: false,
        };
        let mut folder = None;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--out" => {
                    let out = args.next().ok_or("--out needs a folder")?;
                    options.out = Some(PathBuf::from(out));
                }
                "--force" => options.force = true,
                "--check" => options.check = true,
                flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`\n\n{USAGE}")),
                path if folder.is_none() => folder = Some(PathBuf::from(path)),
                extra => return Err(format!("unexpected argument `{extra}`\n\n{USAGE}")),
            }
        }
        if let Some(folder) = folder {
            options.folder = folder;
        }
        Ok(options)
    }
}

/// What a texture holds, which decides its GPU format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Color,
    Linear,
    Normal,
    Mask,
    Ui,
    Skip,
}

impl Kind {
    fn parse(name: &str) -> Result<Kind, String> {
        Ok(match name {
            "color" => Kind::Color,
            "linear" => Kind::Linear,
            "normal" => Kind::Normal,
            "mask" => Kind::Mask,
            "ui" => Kind::Ui,
            "skip" => Kind::Skip,
            other => {
                return Err(format!(
                    "unknown kind \"{other}\" (use color, linear, normal, mask, ui or skip)"
                ));
            }
        })
    }
}

/// One texture's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: Kind,
    /// Basis Universal UASTC instead of BC blocks.
    pub basis: bool,
    pub mipmaps: bool,
}

impl Rule {
    const fn of(kind: Kind) -> Rule {
        Rule {
            kind,
            basis: false,
            mipmaps: !matches!(kind, Kind::Ui),
        }
    }
}

/// `textures.json`: the default rule and the rules by path or pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub default: Rule,
    pub rules: Vec<(String, Rule)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default: Rule::of(Kind::Color),
            rules: Vec::new(),
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, String> {
        let json: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("textures.json: {e}"))?;
        let object = json.as_object().ok_or("textures.json: expected an object")?;
        let mut settings = Settings::default();
        for (key, value) in object {
            match key.as_str() {
                "default" => settings.default = rule(value).map_err(|e| format!("textures.json: default: {e}"))?,
                "textures" => {
                    let rules = value
                        .as_object()
                        .ok_or("textures.json: \"textures\" must be an object")?;
                    for (path, value) in rules {
                        let rule = rule(value).map_err(|e| format!("textures.json: \"{path}\": {e}"))?;
                        settings.rules.push((path.clone(), rule));
                    }
                }
                other => return Err(format!("textures.json: unknown key \"{other}\"")),
            }
        }
        Ok(settings)
    }

    /// The rule for `path` (relative, `/`-separated): an exact path, else
    /// the longest matching pattern, else the default.
    pub fn rule_for(&self, path: &str) -> Rule {
        if let Some((_, rule)) = self.rules.iter().find(|(p, _)| p == path) {
            return *rule;
        }
        self.rules
            .iter()
            .filter(|(pattern, _)| pattern.contains('*') && glob(pattern.as_bytes(), path.as_bytes()))
            .max_by_key(|(pattern, _)| pattern.len())
            .map_or(self.default, |(_, rule)| *rule)
    }
}

fn rule(value: &serde_json::Value) -> Result<Rule, String> {
    if let Some(kind) = value.as_str() {
        return Ok(Rule::of(Kind::parse(kind)?));
    }
    let object = value.as_object().ok_or("expected a kind or an object")?;
    let kind = object.get("kind").and_then(|k| k.as_str()).ok_or("expected \"kind\"")?;
    let mut rule = Rule::of(Kind::parse(kind)?);
    for (key, value) in object {
        match key.as_str() {
            "kind" => {}
            "basis" => rule.basis = value.as_bool().ok_or("\"basis\" is true or false")?,
            "mipmaps" => rule.mipmaps = value.as_bool().ok_or("\"mipmaps\" is true or false")?,
            other => return Err(format!("unknown option \"{other}\"")),
        }
    }
    if rule.kind == Kind::Ui && rule.basis {
        return Err("ui textures stay uncompressed: \"basis\" doesn't apply".into());
    }
    Ok(rule)
}

/// Whether `path` matches `pattern`: `*` matches within a folder name,
/// `**` across folders.
fn glob(pattern: &[u8], path: &[u8]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((b'*', rest)) => {
            let (across, rest) = match rest.split_first() {
                Some((b'*', rest)) => (true, rest),
                _ => (false, rest),
            };
            (0..=path.len())
                .take_while(|&i| across || i == 0 || path[i - 1] != b'/')
                .any(|i| glob(rest, &path[i..]))
        }
        Some((c, rest)) => path.first() == Some(c) && glob(rest, &path[1..]),
    }
}

/// Runs `ponko textures` with `options`, relative to `dir`.
pub fn textures_command(options: &Options, dir: &Path) -> Result<String, String> {
    let folder = dir.join(&options.folder);
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let out = options.out.as_ref().map_or_else(|| folder.clone(), |o| dir.join(o));
    let settings_file = folder.join("textures.json");
    let (settings, settings_time) = if settings_file.is_file() {
        let text = std::fs::read_to_string(&settings_file)
            .map_err(|e| format!("can't read {}: {e}", settings_file.display()))?;
        (Settings::parse(&text)?, modified(&settings_file))
    } else {
        (Settings::default(), None)
    };
    let mut sources = Vec::new();
    find_images(&folder, &folder, &out, &mut sources)?;
    sources.sort();
    let mut report = Vec::new();
    let mut stale = Vec::new();
    let (mut before, mut after) = (0u64, 0u64);
    for relative in &sources {
        let name = relative.to_string_lossy().replace('\\', "/");
        let rule = settings.rule_for(&name);
        if rule.kind == Kind::Skip {
            continue;
        }
        let source = folder.join(relative);
        let target = out.join(relative).with_extension("ktx2");
        let up_to_date = match modified(&target) {
            Some(made) => modified(&source).is_some_and(|s| s <= made) && settings_time.is_none_or(|s| s <= made),
            None => false,
        };
        if options.check {
            if !up_to_date {
                stale.push(name);
            }
            continue;
        }
        if up_to_date && !options.force {
            continue;
        }
        let image = image::open(&source)
            .map_err(|e| format!("can't read {}: {e}", source.display()))?
            .into_rgba8();
        let (width, height) = image.dimensions();
        let encoded = encode(image.as_raw(), width, height, rule).map_err(|e| format!("{name}: {e}"))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("can't make {}: {e}", parent.display()))?;
        }
        std::fs::write(&target, &encoded.file).map_err(|e| format!("can't write {}: {e}", target.display()))?;
        let rgba = rgba8_with_mips(width, height, rule.mipmaps);
        before += rgba;
        after += encoded.gpu_bytes;
        let mut line = format!(
            "{name} -> {}: {}, {width} x {height}, {} level{}, {} on the GPU (RGBA8: {}), {} on disk",
            target.strip_prefix(dir).unwrap_or(&target).display(),
            encoded.format,
            encoded.levels,
            if encoded.levels == 1 { "" } else { "s" },
            size(encoded.gpu_bytes),
            size(rgba),
            size(encoded.file.len() as u64),
        );
        if rule.kind != Kind::Ui && !(width.is_multiple_of(4) && height.is_multiple_of(4)) {
            line.push_str(
                "\n  warning: not a multiple of 4 pixels, so GPUs can't sample it compressed; it loads as RGBA8",
            );
        }
        report.push(line);
    }
    if options.check {
        return if stale.is_empty() {
            Ok(format!("{} textures are up to date", sources.len()))
        } else {
            Err(format!("out of date (run `ponko textures`): {}", stale.join(", ")))
        };
    }
    if report.is_empty() {
        return Ok("textures are up to date".into());
    }
    report.push(format!(
        "{} converted: {} on the GPU instead of {}",
        report.len(),
        size(after),
        size(before)
    ));
    Ok(report.join("\n"))
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn size(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MiB", bytes as f64 / f64::from(1 << 20))
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

/// The images under `dir`, relative to `root`, leaving out `out`'s own
/// subfolder (when it is inside) and hidden folders.
fn find_images(root: &Path, dir: &Path, out: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("can't read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("can't read {}: {e}", dir.display()))?.path();
        if path.is_dir() {
            let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if !hidden && (path != out || out == root) {
                find_images(root, &path, out, found)?;
            }
            continue;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "png" | "jpg" | "jpeg")
            && let Ok(relative) = path.strip_prefix(root)
        {
            found.push(relative.to_path_buf());
        }
    }
    Ok(())
}

/// RGBA8's GPU bytes for a `width` x `height` image, with a full mip chain.
fn rgba8_with_mips(width: u32, height: u32, mipmaps: bool) -> u64 {
    let (mut w, mut h, mut total) = (u64::from(width), u64::from(height), 0);
    loop {
        total += w * h * 4;
        if !mipmaps || (w == 1 && h == 1) {
            return total;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
}

/// An encoded texture: the KTX2 file, and what it costs on a GPU that
/// samples its format.
pub struct Encoded {
    pub file: Vec<u8>,
    pub format: &'static str,
    pub levels: usize,
    pub gpu_bytes: u64,
}

/// Encodes `width` x `height` RGBA8 pixels per `rule`.
pub fn encode(rgba: &[u8], width: u32, height: u32, rule: Rule) -> Result<Encoded, String> {
    if rule.kind == Kind::Ui {
        // RGBA8, sampled through sRGB like every image the engine loads.
        let mut levels = vec![rgba.to_vec()];
        if rule.mipmaps {
            while let Some(next) = halve(
                levels.last().expect("a level"),
                width >> (levels.len() - 1),
                height >> (levels.len() - 1),
            ) {
                levels.push(next);
            }
        }
        let gpu_bytes = levels.iter().map(|l| l.len() as u64).sum();
        let dfd = ktx2::dfd::Basic::from_format(ktx2::Format::R8G8B8A8_SRGB)
            .map_err(|e| format!("{e:?}"))?
            .0;
        return Ok(Encoded {
            levels: levels.len(),
            file: write_ktx2(Some(ktx2::Format::R8G8B8A8_SRGB), dfd, width, height, &levels),
            format: "RGBA8",
            gpu_bytes,
        });
    }
    let srgb = rule.kind == Kind::Color;
    let normal = rule.kind == Kind::Normal;
    // A normal map's X goes to red, green and blue, its Y to alpha:
    // Basis Universal's layout for two-channel textures (BC5 takes red and alpha).
    let pixels: std::borrow::Cow<[u8]> = if normal {
        rgba.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect::<Vec<u8>>()
            .into()
    } else {
        rgba.into()
    };
    let (levels, has_alpha) = uastc(&pixels, width, height, srgb, rule.mipmaps)?;
    let level_sizes: Vec<(u32, u32)> = (0..levels.len())
        .map(|i| ((width >> i).max(1), (height >> i).max(1)))
        .collect();
    if rule.basis {
        // KHR_DF_CHANNEL_UASTC_*: RGB 0, RGBA 3, RRRG 5.
        let channel = if normal {
            5
        } else if has_alpha {
            3
        } else {
            0
        };
        let gpu_bytes = levels.iter().map(|l| l.len() as u64).sum();
        return Ok(Encoded {
            levels: levels.len(),
            file: write_ktx2(None, uastc_dfd(srgb, channel), width, height, &levels),
            format: if srgb { "UASTC sRGB" } else { "UASTC" },
            gpu_bytes,
        });
    }
    let (format, to, name) = match rule.kind {
        Kind::Normal => (ktx2::Format::BC5_UNORM_BLOCK, TranscoderBlockFormat::BC5, "BC5"),
        Kind::Mask => (ktx2::Format::BC4_UNORM_BLOCK, TranscoderBlockFormat::BC4, "BC4"),
        Kind::Color => (ktx2::Format::BC7_SRGB_BLOCK, TranscoderBlockFormat::BC7, "BC7 sRGB"),
        _ => (ktx2::Format::BC7_UNORM_BLOCK, TranscoderBlockFormat::BC7, "BC7"),
    };
    let transcoder = LowLevelUastcTranscoder::new();
    let mut blocks = Vec::with_capacity(levels.len());
    for (level, &(w, h)) in levels.iter().zip(&level_sizes) {
        let out = transcoder
            .transcode_slice(
                level,
                SliceParametersUastc {
                    num_blocks_x: w.div_ceil(4),
                    num_blocks_y: h.div_ceil(4),
                    has_alpha,
                    original_width: w,
                    original_height: h,
                },
                DecodeFlags::HIGH_QUALITY,
                to,
            )
            .map_err(|e| format!("transcoding to {name} failed: {e:?}"))?;
        blocks.push(out);
    }
    let dfd = ktx2::dfd::Basic::from_format(format).map_err(|e| format!("{e:?}"))?.0;
    let gpu_bytes = blocks.iter().map(|l| l.len() as u64).sum();
    Ok(Encoded {
        levels: blocks.len(),
        file: write_ktx2(Some(format), dfd, width, height, &blocks),
        format: name,
        gpu_bytes,
    })
}

/// Halves an RGBA8 level by averaging 2 x 2 blocks (in stored values; UI
/// images rarely use their mipmaps); `None` at 1 x 1.
fn halve(level: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let (w, h) = (width.max(1), height.max(1));
    if w == 1 && h == 1 {
        return None;
    }
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut sum = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((2 * x + dx).min(w - 1), (2 * y + dy).min(h - 1));
                    sum += u32::from(level[((sy * w + sx) * 4 + c) as usize]);
                }
                out[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
            }
        }
    }
    Some(out)
}

/// Encodes RGBA8 pixels as UASTC with Basis Universal: each mip level's
/// 16-byte blocks, largest first, and whether any pixel isn't opaque.
fn uastc(rgba: &[u8], width: u32, height: u32, srgb: bool, mipmaps: bool) -> Result<(Vec<Vec<u8>>, bool), String> {
    let mut params = CompressorParams::new();
    params.set_basis_format(BasisTextureFormat::UASTC4x4);
    params.set_uastc_quality_level(UASTC_QUALITY_DEFAULT);
    params.set_color_space(if srgb { ColorSpace::Srgb } else { ColorSpace::Linear });
    params.set_generate_mipmaps(mipmaps);
    params.set_print_status_to_stdout(false);
    params.source_image_mut(0).init(rgba, width, height, 4);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    let mut compressor = Compressor::new(threads);
    // SAFETY: `params` outlives the compressor's use of it (both are dropped
    // at the end of this function, after `process`), and holds one valid
    // source image of `width` x `height` x 4 bytes copied in by `init`.
    let ready = unsafe { compressor.init(&params) };
    if !ready {
        return Err("the Basis Universal encoder refused the settings".into());
    }
    // SAFETY: as above; `init` succeeded.
    unsafe { compressor.process() }.map_err(|e| format!("Basis Universal encoding failed: {e:?}"))?;
    let levels = basis_uastc_levels(compressor.basis_file())?;
    Ok((levels, compressor.any_source_image_has_alpha()))
}

/// The UASTC blocks of each mip level of image 0 in a `.basis` file (its
/// slice descriptions say where), so they can go into a KTX2 file.
fn basis_uastc_levels(file: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    // Little-endian packed fields of `basis_file_header` and `basis_slice_desc`.
    let uint = |at: usize, len: usize| -> Result<usize, String> {
        let bytes = file.get(at..at + len).ok_or("the .basis file is cut short")?;
        Ok(bytes.iter().rev().fold(0usize, |v, &b| (v << 8) | usize::from(b)))
    };
    const HEADER: usize = 77;
    const SLICE: usize = 23;
    if uint(0, 2)? != 0x4273 || uint(4, 2)? != HEADER {
        return Err("the encoder's output isn't a .basis file this tool reads".into());
    }
    // m_tex_format: 1 is UASTC 4x4.
    if uint(20, 1)? != 1 {
        return Err("the encoder's output isn't UASTC".into());
    }
    let (slices, descs) = (uint(14, 3)?, uint(65, 4)?);
    let mut levels = Vec::new();
    for i in 0..slices {
        let at = descs + i * SLICE;
        let (image, level) = (uint(at, 3)?, uint(at + 3, 1)?);
        if image != 0 {
            continue;
        }
        if level != levels.len() {
            return Err("the .basis file's mip levels are out of order".into());
        }
        let (offset, len) = (uint(at + 13, 4)?, uint(at + 17, 4)?);
        let blocks = file.get(offset..offset + len).ok_or("a .basis slice is cut short")?;
        levels.push(blocks.to_vec());
    }
    if levels.is_empty() {
        return Err("the .basis file has no image".into());
    }
    Ok(levels)
}

/// The data format descriptor of a UASTC texture with channels `channel`.
fn uastc_dfd(srgb: bool, channel: u8) -> ktx2::dfd::Basic {
    let mut dfd = ktx2::dfd::Basic::from_format(ktx2::Format::BC7_UNORM_BLOCK)
        .expect("BC7 has a descriptor")
        .0;
    dfd.color_model = Some(ktx2::ColorModel::UASTC);
    dfd.transfer_function = Some(if srgb {
        ktx2::TransferFunction::SRGB
    } else {
        ktx2::TransferFunction::Linear
    });
    if let Some(sample) = dfd.sample_information.first_mut() {
        sample.channel_type = channel;
    }
    dfd
}

/// A KTX2 file of one 2D image: `format` (`None`: UASTC) and its levels,
/// largest first, each Zstandard-supercompressed.
fn write_ktx2(
    format: Option<ktx2::Format>,
    dfd: ktx2::dfd::Basic,
    width: u32,
    height: u32,
    levels: &[Vec<u8>],
) -> Vec<u8> {
    let dfd = ktx2::dfd::Block::Basic(dfd).to_vec();
    let dfd_len = 4 + dfd.len();
    let kvd = key_value(
        b"KTXwriter",
        concat!("ponko textures ", env!("CARGO_PKG_VERSION")).as_bytes(),
    );
    let dfd_at = 80 + 24 * levels.len();
    let kvd_at = dfd_at + dfd_len;
    let packed: Vec<Vec<u8>> = levels
        .iter()
        .map(|l| ruzstd::encoding::compress_to_vec(l.as_slice(), ruzstd::encoding::CompressionLevel::Fastest))
        .collect();
    // Smallest level first in the file, as the specification recommends.
    let mut offsets = vec![0usize; levels.len()];
    let mut at = kvd_at + kvd.len();
    for i in (0..levels.len()).rev() {
        offsets[i] = at;
        at += packed[i].len();
    }
    let header = ktx2::Header {
        format,
        type_size: 1,
        pixel_width: width,
        pixel_height: height,
        pixel_depth: 0,
        layer_count: 0,
        face_count: 1,
        level_count: levels.len() as u32,
        supercompression_scheme: Some(ktx2::SupercompressionScheme::Zstandard),
        index: ktx2::Index {
            dfd_byte_offset: dfd_at as u32,
            dfd_byte_length: dfd_len as u32,
            kvd_byte_offset: kvd_at as u32,
            kvd_byte_length: kvd.len() as u32,
            sgd_byte_offset: 0,
            sgd_byte_length: 0,
        },
    };
    let mut out = header.as_bytes().to_vec();
    for ((level, packed), offset) in levels.iter().zip(&packed).zip(&offsets) {
        out.extend_from_slice(
            &ktx2::LevelIndex {
                byte_offset: *offset as u64,
                byte_length: packed.len() as u64,
                uncompressed_byte_length: level.len() as u64,
            }
            .as_bytes(),
        );
    }
    out.extend_from_slice(&(dfd_len as u32).to_le_bytes());
    out.extend_from_slice(&dfd);
    out.extend_from_slice(&kvd);
    for packed in packed.iter().rev() {
        out.extend_from_slice(packed);
    }
    out
}

/// One key/value entry: its length, the key and value each ended by a
/// zero byte, padded to 4 bytes.
fn key_value(key: &[u8], value: &[u8]) -> Vec<u8> {
    let len = key.len() + 1 + value.len() + 1;
    let mut out = (len as u32).to_le_bytes().to_vec();
    out.extend_from_slice(key);
    out.push(0);
    out.extend_from_slice(value);
    out.push(0);
    out.resize(4 + len.next_multiple_of(4), 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn options_default_to_the_assets_folder() {
        let o = Options::parse(&[]).unwrap();
        assert_eq!(o.folder, PathBuf::from("assets"));
        let o = Options::parse(&args(&["art", "--out", "assets/textures", "--force"])).unwrap();
        assert_eq!(
            (o.folder, o.out, o.force),
            (PathBuf::from("art"), Some(PathBuf::from("assets/textures")), true)
        );
        assert!(Options::parse(&args(&["--bogus"])).is_err());
    }

    #[test]
    fn exact_paths_beat_patterns_and_longer_patterns_beat_shorter() {
        let s = Settings::parse(
            r#"{"default": "linear", "textures": {
                "*.png": "color", "*_normal.png": "normal", "ui/**": "ui",
                "rock_normal.png": {"kind": "color", "basis": true}}}"#,
        )
        .unwrap();
        assert_eq!(s.rule_for("grass.png").kind, Kind::Color);
        assert_eq!(s.rule_for("wall_normal.png").kind, Kind::Normal);
        assert_eq!(
            s.rule_for("rock_normal.png"),
            Rule {
                kind: Kind::Color,
                basis: true,
                mipmaps: true
            }
        );
        assert_eq!(s.rule_for("ui/menu/button.png").kind, Kind::Ui);
        assert!(!s.rule_for("ui/menu/button.png").mipmaps);
        // `*` stays inside one folder name.
        assert_eq!(s.rule_for("props/barrel.png").kind, Kind::Linear);
        assert_eq!(s.rule_for("photo.jpg").kind, Kind::Linear);
    }

    #[test]
    fn the_guides_patterns_match_at_any_depth() {
        let s = Settings::parse(
            r#"{"default": "color", "textures": {
                "**_normal.png": "normal", "**_mask.png": "mask", "ui/**": "ui"}}"#,
        )
        .unwrap();
        assert_eq!(s.rule_for("wall_normal.png").kind, Kind::Normal);
        assert_eq!(s.rule_for("props/barrel/wood_normal.png").kind, Kind::Normal);
        assert_eq!(s.rule_for("terrain/grass_mask.png").kind, Kind::Mask);
        assert_eq!(s.rule_for("ui/menu/button_normal.png").kind, Kind::Normal);
        assert_eq!(s.rule_for("ui/menu/button.png").kind, Kind::Ui);
        assert_eq!(s.rule_for("props/barrel/wood.png").kind, Kind::Color);
    }

    #[test]
    fn bad_settings_name_the_problem() {
        let err = Settings::parse(r#"{"textures": {"a.png": "shiny"}}"#).unwrap_err();
        assert!(err.contains("\"a.png\"") && err.contains("unknown kind"), "{err}");
        assert!(
            Settings::parse(r#"{"defaults": "color"}"#)
                .unwrap_err()
                .contains("unknown key")
        );
        assert!(Settings::parse(r#"{"textures": {"a.png": {"kind": "ui", "basis": true}}}"#).is_err());
    }

    /// A 32 x 32 test card: a color gradient with a soft disc.
    fn card(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let disc = (x as f32 - 16.0).hypot(y as f32 - 16.0) < 10.0;
                rgba.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, if disc { 220 } else { 40 }, 255]);
            }
        }
        rgba
    }

    fn read(file: &[u8]) -> (ktx2::Header, Vec<Vec<u8>>) {
        let reader = ktx2::Reader::new(file).expect("a valid KTX2 file");
        let levels = reader
            .levels()
            .map(|l| {
                let mut out = Vec::new();
                let mut d = ruzstd::decoding::StreamingDecoder::new(l.data).unwrap();
                std::io::Read::read_to_end(&mut d, &mut out).unwrap();
                out
            })
            .collect();
        (reader.header(), levels)
    }

    fn mean_error(a: &[u8], b: &[u8], channels: &[usize]) -> f32 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for (p, q) in a.chunks(4).zip(b.chunks(4)) {
            for &c in channels {
                sum += (f32::from(p[c]) - f32::from(q[c])).abs();
                n += 1.0;
            }
        }
        sum / n
    }

    fn decode(format: ktx2::Format, blocks: &[u8], w: u32, h: u32) -> Vec<u8> {
        let mut pixels = vec![0u32; (w * h) as usize];
        let (w, h) = (w as usize, h as usize);
        match format {
            ktx2::Format::BC7_SRGB_BLOCK | ktx2::Format::BC7_UNORM_BLOCK => {
                texture2ddecoder::decode_bc7(blocks, w, h, &mut pixels)
            }
            ktx2::Format::BC5_UNORM_BLOCK => texture2ddecoder::decode_bc5(blocks, w, h, &mut pixels),
            ktx2::Format::BC4_UNORM_BLOCK => texture2ddecoder::decode_bc4(blocks, w, h, &mut pixels),
            other => panic!("{other:?}"),
        }
        .unwrap();
        pixels
            .iter()
            .flat_map(|p| {
                let [b, g, r, a] = p.to_le_bytes();
                [r, g, b, a]
            })
            .collect()
    }

    #[test]
    fn color_textures_become_bc7_with_a_full_mip_chain_close_to_the_source() {
        let rgba = card(32, 32);
        let encoded = encode(&rgba, 32, 32, Rule::of(Kind::Color)).unwrap();
        let (header, levels) = read(&encoded.file);
        assert_eq!(header.format, Some(ktx2::Format::BC7_SRGB_BLOCK));
        assert_eq!((header.pixel_width, header.pixel_height, levels.len()), (32, 32, 6));
        assert_eq!(levels[0].len(), 8 * 8 * 16);
        assert_eq!(encoded.gpu_bytes, (64 + 16 + 4 + 1 + 1 + 1) * 16);
        let back = decode(ktx2::Format::BC7_SRGB_BLOCK, &levels[0], 32, 32);
        let error = mean_error(&rgba, &back, &[0, 1, 2]);
        assert!(error < 3.0, "mean error {error}");
        // Negative control: the check notices a different image.
        let other: Vec<u8> = rgba.iter().map(|v| 255 - v).collect();
        assert!(mean_error(&other, &back, &[0, 1, 2]) > 30.0);
    }

    #[test]
    fn normal_maps_become_bc5_with_x_and_y_in_red_and_green() {
        let rgba = card(32, 32);
        let encoded = encode(&rgba, 32, 32, Rule::of(Kind::Normal)).unwrap();
        let (header, levels) = read(&encoded.file);
        assert_eq!(header.format, Some(ktx2::Format::BC5_UNORM_BLOCK));
        let back = decode(ktx2::Format::BC5_UNORM_BLOCK, &levels[0], 32, 32);
        let error = mean_error(&rgba, &back, &[0, 1]);
        assert!(error < 2.0, "mean error {error}");
    }

    #[test]
    fn masks_become_bc4_and_ui_stays_rgba8() {
        let rgba = card(32, 32);
        let mask = encode(&rgba, 32, 32, Rule::of(Kind::Mask)).unwrap();
        let (header, levels) = read(&mask.file);
        assert_eq!(header.format, Some(ktx2::Format::BC4_UNORM_BLOCK));
        assert!(mean_error(&rgba, &decode(ktx2::Format::BC4_UNORM_BLOCK, &levels[0], 32, 32), &[0]) < 2.0);
        let ui = encode(&rgba, 32, 32, Rule::of(Kind::Ui)).unwrap();
        let (header, levels) = read(&ui.file);
        assert_eq!(header.format, Some(ktx2::Format::R8G8B8A8_SRGB));
        assert_eq!(levels, [rgba]);
    }

    #[test]
    fn basis_textures_keep_uastc_blocks_and_say_their_channels() {
        let rgba = card(32, 32);
        let rule = Rule {
            basis: true,
            ..Rule::of(Kind::Normal)
        };
        let encoded = encode(&rgba, 32, 32, rule).unwrap();
        let reader = ktx2::Reader::new(&encoded.file[..]).unwrap();
        assert_eq!(reader.header().format, None);
        assert_eq!(reader.color_model(), Some(ktx2::ColorModel::UASTC));
        assert_eq!(reader.basic_dfd().unwrap().sample_information[0].channel_type, 5);
        assert_eq!(reader.levels().len(), 6);
        assert_eq!(reader.transfer_function(), Some(ktx2::TransferFunction::Linear));
    }

    #[test]
    fn the_command_converts_stale_images_only_and_check_reports_them() {
        let dir = std::env::temp_dir().join(format!("ponko-textures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets/ui")).unwrap();
        let png = |path: &Path| {
            image::RgbaImage::from_raw(32, 32, card(32, 32))
                .unwrap()
                .save(path)
                .unwrap();
        };
        png(&dir.join("assets/wall.png"));
        png(&dir.join("assets/ui/button.png"));
        std::fs::write(dir.join("assets/textures.json"), r#"{"textures": {"ui/*": "ui"}}"#).unwrap();
        let options = Options::parse(&[]).unwrap();
        let check = Options {
            check: true,
            ..options.clone()
        };
        assert!(textures_command(&check, &dir).unwrap_err().contains("wall.png"));
        let out = textures_command(&options, &dir).unwrap();
        assert!(out.contains("wall.png -> ") && out.contains("BC7 sRGB"), "{out}");
        assert!(
            out.contains("ui/button.png -> ") && out.contains("RGBA8, 32 x 32, 1 level,"),
            "{out}"
        );
        assert!(dir.join("assets/wall.ktx2").is_file());
        assert_eq!(textures_command(&options, &dir).unwrap(), "textures are up to date");
        assert!(textures_command(&check, &dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
