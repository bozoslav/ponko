//! GPU block-compressed textures (feature `compressed-textures`).
//!
//! Reading KTX2 and DDS files whose pixels are already in a GPU format
//! (BC1-BC7, ETC2/EAC, ASTC 4x4), so they upload as they are, with their
//! mipmaps, and stay compressed in video memory. Basis Universal UASTC
//! files are transcoded to what the GPU samples (feature
//! `basis-universal`, native builds only). A GPU without the file's format
//! gets level 0 decoded to RGBA8 on the CPU and goes the usual way.
//!
//! The decode is CPU work with no GPU (any thread), like
//! [`decode_image`](crate::assets::decode_image); the renderer turns a
//! [`CompressedImage`] into a texture.

use crate::assets::Image;
use crate::error::{Error, ErrorKind};

/// A block-compressed GPU format. Every one has 4 x 4 pixel blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BlockFormat {
    /// RGB with 1-bit alpha, 8 bytes a block.
    Bc1,
    /// RGBA with explicit 4-bit alpha.
    Bc2,
    /// RGBA with interpolated alpha.
    Bc3,
    /// One channel (masks), 8 bytes a block.
    Bc4,
    /// Two channels (normal maps).
    Bc5,
    /// High-quality RGBA.
    Bc7,
    /// ETC2 RGB, 8 bytes a block.
    Etc2Rgb8,
    /// ETC2 RGBA.
    Etc2Rgba8,
    /// EAC one channel, 8 bytes a block.
    EacR11,
    /// EAC two channels.
    EacRg11,
    /// ASTC with 4 x 4 blocks.
    Astc4x4,
}

/// The pixel size of every [`BlockFormat`]'s block.
pub(crate) const BLOCK: u32 = 4;

impl BlockFormat {
    /// Bytes of one 4 x 4 block.
    pub(crate) fn block_bytes(self) -> u32 {
        match self {
            BlockFormat::Bc1 | BlockFormat::Bc4 | BlockFormat::Etc2Rgb8 | BlockFormat::EacR11 => 8,
            _ => 16,
        }
    }

    /// Whether a GPU with `gpu` samples it.
    pub(crate) fn sampled_by(self, gpu: GpuFormats) -> bool {
        match self {
            BlockFormat::Bc1
            | BlockFormat::Bc2
            | BlockFormat::Bc3
            | BlockFormat::Bc4
            | BlockFormat::Bc5
            | BlockFormat::Bc7 => gpu.bc,
            BlockFormat::Etc2Rgb8 | BlockFormat::Etc2Rgba8 | BlockFormat::EacR11 | BlockFormat::EacRg11 => gpu.etc2,
            BlockFormat::Astc4x4 => gpu.astc,
        }
    }

    /// Whether it holds only red and green (a normal map's X and Y).
    pub(crate) fn two_channel(self) -> bool {
        matches!(self, BlockFormat::Bc5 | BlockFormat::EacRg11)
    }

    /// Whether it has an sRGB variant (the one- and two-channel ones don't).
    pub(crate) fn has_srgb(self) -> bool {
        !matches!(
            self,
            BlockFormat::Bc4 | BlockFormat::Bc5 | BlockFormat::EacR11 | BlockFormat::EacRg11
        )
    }

    /// Its name in messages, such as `"BC7"`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            BlockFormat::Bc1 => "BC1",
            BlockFormat::Bc2 => "BC2",
            BlockFormat::Bc3 => "BC3",
            BlockFormat::Bc4 => "BC4",
            BlockFormat::Bc5 => "BC5",
            BlockFormat::Bc7 => "BC7",
            BlockFormat::Etc2Rgb8 => "ETC2 RGB",
            BlockFormat::Etc2Rgba8 => "ETC2 RGBA",
            BlockFormat::EacR11 => "EAC R11",
            BlockFormat::EacRg11 => "EAC RG11",
            BlockFormat::Astc4x4 => "ASTC 4x4",
        }
    }

    /// Bytes of a `width` x `height` image (blocks at the edges count whole).
    pub(crate) fn level_bytes(self, width: u32, height: u32) -> u64 {
        u64::from(width.div_ceil(BLOCK)) * u64::from(height.div_ceil(BLOCK)) * u64::from(self.block_bytes())
    }
}

/// Which block-compressed format families the GPU samples (its wgpu
/// `TEXTURE_COMPRESSION_*` features).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GpuFormats {
    /// BC1-BC7 (desktop GPUs, Apple silicon).
    pub bc: bool,
    /// ETC2 and EAC (mobile GPUs, most WebGL2 on phones).
    pub etc2: bool,
    /// ASTC (mobile GPUs, Apple silicon).
    pub astc: bool,
}

impl GpuFormats {
    /// Every family: headless runs, where nothing is decoded for a GPU.
    pub(crate) const ALL: GpuFormats = GpuFormats {
        bc: true,
        etc2: true,
        astc: true,
    };
}

/// Where a normal map keeps its X and Y. A two-channel normal map has no Z:
/// the material shader rebuilds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum NormalChannels {
    /// X, Y and Z in red, green and blue (any RGB normal map).
    #[default]
    Rgb,
    /// X in red, Y in green (BC5, EAC RG11).
    Rg,
    /// X in red, Y in alpha (a two-channel UASTC file transcoded to RGBA).
    #[cfg_attr(
        not(all(feature = "basis-universal", not(target_arch = "wasm32"))),
        allow(dead_code, reason = "only UASTC transcoding makes it")
    )]
    Ra,
}

/// A texture in a GPU block format, as the file had it (or as UASTC was
/// transcoded to).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompressedImage {
    pub width: u32,
    pub height: u32,
    pub format: BlockFormat,
    /// Colors (sampled through sRGB decoding) or data (as stored).
    pub srgb: bool,
    /// Mip levels, largest first: each one's blocks, row by row.
    pub levels: Vec<Vec<u8>>,
    /// Where a normal map's channels are.
    pub normal: NormalChannels,
}

impl CompressedImage {
    /// The size of mip level `level`.
    pub(crate) fn level_size(&self, level: usize) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// Bytes on the GPU: every level.
    pub(crate) fn bytes(&self) -> u64 {
        self.levels.iter().map(|l| l.len() as u64).sum()
    }

    /// The same texture from mip level `first` down: a smaller resolution
    /// for streaming, with its own mipmaps.
    pub(crate) fn starting_at_level(&self, first: usize) -> CompressedImage {
        let (width, height) = self.level_size(first);
        CompressedImage {
            width,
            height,
            format: self.format,
            srgb: self.srgb,
            levels: self.levels[first..].to_vec(),
            normal: self.normal,
        }
    }
}

/// Whether a GPU texture can start at a `width` x `height` level of a block
/// format: whole blocks only.
pub(crate) fn block_aligned(width: u32, height: u32) -> bool {
    width.is_multiple_of(BLOCK) && height.is_multiple_of(BLOCK)
}

/// A decoded texture file: compressed blocks to upload as they are, or
/// RGBA8 (an image file, or a compressed one this GPU can't sample).
pub(crate) enum Decoded {
    Rgba(Image),
    Blocks(CompressedImage),
}

const KTX2_MAGIC: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
];

/// Whether `bytes` start like a KTX2 file.
pub(crate) fn is_ktx2(bytes: &[u8]) -> bool {
    bytes.starts_with(&KTX2_MAGIC)
}

/// Decodes a KTX2 file, or a DDS file holding BCn blocks, for a GPU that
/// samples `gpu`. `None`: not one of those (an image file, or a DDS file
/// of plain pixels for the image decoder). `path` and `what` are for
/// messages, as in [`decode_image`](crate::assets::decode_image).
pub(crate) fn decode(bytes: &[u8], path: &str, what: &str, gpu: GpuFormats) -> Option<Result<Decoded, Error>> {
    let file = if is_ktx2(bytes) {
        read_ktx2(bytes, path, what)
    } else if bytes.starts_with(b"DDS ") {
        read_dds(bytes, path, what)?
    } else {
        return None;
    };
    Some(file.and_then(|file| for_gpu(file, path, what, gpu)))
}

/// [`decode`] to RGBA8 on the CPU, for loaders that need pixels rather
/// than a texture.
pub(crate) fn decode_rgba(bytes: &[u8], path: &str, what: &str) -> Option<Result<Image, Error>> {
    // No block format is sampled, so every file comes back as RGBA8.
    Some(
        decode(bytes, path, what, GpuFormats::default())?.map(|decoded| match decoded {
            Decoded::Rgba(image) => image,
            Decoded::Blocks(image) => {
                unreachable!("a {} image chosen for a GPU without block formats", image.format.name())
            }
        }),
    )
}

/// What a file holds, before choosing for a GPU.
enum Pixels {
    Blocks(BlockFormat),
    Rgba8,
    /// Basis Universal UASTC, and the channels it holds.
    Uastc(UastcChannels),
}

/// A UASTC file's channels (its data format descriptor says).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UastcChannels {
    Rgb,
    Rgba,
    /// One channel copied into red, green and blue.
    Rrr,
    /// X in red, green and blue, Y in alpha: a two-channel normal map.
    Rrrg,
    /// X in red, Y in green.
    Rg,
}

/// A texture file read into its levels.
struct File {
    width: u32,
    height: u32,
    pixels: Pixels,
    srgb: bool,
    /// Every mip level, largest first, unpacked from any supercompression.
    levels: Vec<Vec<u8>>,
}

fn unsupported(path: &str, message: String) -> Error {
    Error::new(ErrorKind::Unsupported, message).with_path(path)
}

fn invalid(path: &str, what: &str, why: impl std::fmt::Display) -> Error {
    Error::decode(path, format!("{what} is not a valid texture file: {why}"))
}

fn read_ktx2(bytes: &[u8], path: &str, what: &str) -> Result<File, Error> {
    let reader = ktx2::Reader::new(bytes).map_err(|e| invalid(path, what, format_args!("KTX2: {e}")))?;
    let header = reader.header();
    if header.face_count != 1 {
        return Err(unsupported(
            path,
            format!("{what} is a KTX2 cube map; ponko loads 2D KTX2 textures only"),
        ));
    }
    if header.pixel_depth > 1 || header.pixel_height == 0 {
        return Err(unsupported(
            path,
            format!("{what} is a 1D or 3D KTX2 texture; ponko loads 2D KTX2 textures only"),
        ));
    }
    if header.layer_count > 1 {
        return Err(unsupported(
            path,
            format!(
                "{what} is a KTX2 array of {} layers; load each layer from its own file with load_texture_array",
                header.layer_count
            ),
        ));
    }
    let srgb = reader.transfer_function() == Some(ktx2::TransferFunction::SRGB);
    let pixels = match header.format {
        Some(format) => match block_format(format) {
            Some(block) => Pixels::Blocks(block),
            None if format == ktx2::Format::R8G8B8A8_UNORM || format == ktx2::Format::R8G8B8A8_SRGB => Pixels::Rgba8,
            None => {
                return Err(unsupported(
                    path,
                    format!(
                        "{what} is a KTX2 texture in {format:?}, which ponko doesn't load: use BC1-BC7, \
                         ETC2/EAC, ASTC 4x4, RGBA8 or Basis Universal UASTC (`ponko textures` makes them)"
                    ),
                ));
            }
        },
        None if reader.color_model() == Some(ktx2::ColorModel::UASTC) => {
            // KHR_DF_CHANNEL_UASTC_*: which channels the blocks hold.
            let channel = reader
                .basic_dfd()
                .and_then(|dfd| dfd.sample_information.first())
                .map_or(0, |s| s.channel_type);
            Pixels::Uastc(match channel {
                3 => UastcChannels::Rgba,
                4 => UastcChannels::Rrr,
                5 => UastcChannels::Rrrg,
                6 => UastcChannels::Rg,
                _ => UastcChannels::Rgb,
            })
        }
        None if reader.color_model() == Some(ktx2::ColorModel::ETC1S) => {
            return Err(unsupported(
                path,
                format!(
                    "{what} is Basis Universal ETC1S (BasisLZ), which ponko doesn't transcode: \
                     encode it as UASTC (`ponko textures` with \"basis\": true) or as BC7"
                ),
            ));
        }
        None => {
            return Err(unsupported(
                path,
                format!("{what} is a KTX2 texture of an unknown format, which ponko doesn't load"),
            ));
        }
    };
    let mut levels = Vec::with_capacity(header.level_count.max(1) as usize);
    for level in reader.levels() {
        levels.push(match header.supercompression_scheme {
            None => level.data.to_vec(),
            Some(ktx2::SupercompressionScheme::Zstandard) => {
                let mut out = Vec::with_capacity(level.uncompressed_byte_length as usize);
                let mut decoder = ruzstd::decoding::StreamingDecoder::new(level.data)
                    .map_err(|e| invalid(path, what, format_args!("Zstandard: {e}")))?;
                std::io::Read::read_to_end(&mut decoder, &mut out)
                    .map_err(|e| invalid(path, what, format_args!("Zstandard: {e}")))?;
                out
            }
            Some(ktx2::SupercompressionScheme::ZLIB) => {
                let mut out = Vec::with_capacity(level.uncompressed_byte_length as usize);
                std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(level.data), &mut out)
                    .map_err(|e| invalid(path, what, format_args!("zlib: {e}")))?;
                out
            }
            Some(scheme) => {
                return Err(unsupported(
                    path,
                    format!("{what} uses {scheme:?} supercompression, which ponko doesn't unpack"),
                ));
            }
        });
    }
    let file = File {
        width: header.pixel_width,
        height: header.pixel_height,
        pixels,
        srgb,
        levels,
    };
    check_levels(&file, path, what)?;
    Ok(file)
}

/// The block format of a Vulkan format, if it is one ponko reads.
fn block_format(format: ktx2::Format) -> Option<BlockFormat> {
    use ktx2::Format as F;
    Some(match format {
        F::BC1_RGB_UNORM_BLOCK | F::BC1_RGB_SRGB_BLOCK | F::BC1_RGBA_UNORM_BLOCK | F::BC1_RGBA_SRGB_BLOCK => {
            BlockFormat::Bc1
        }
        F::BC2_UNORM_BLOCK | F::BC2_SRGB_BLOCK => BlockFormat::Bc2,
        F::BC3_UNORM_BLOCK | F::BC3_SRGB_BLOCK => BlockFormat::Bc3,
        F::BC4_UNORM_BLOCK => BlockFormat::Bc4,
        F::BC5_UNORM_BLOCK => BlockFormat::Bc5,
        F::BC7_UNORM_BLOCK | F::BC7_SRGB_BLOCK => BlockFormat::Bc7,
        F::ETC2_R8G8B8_UNORM_BLOCK | F::ETC2_R8G8B8_SRGB_BLOCK => BlockFormat::Etc2Rgb8,
        F::ETC2_R8G8B8A8_UNORM_BLOCK | F::ETC2_R8G8B8A8_SRGB_BLOCK => BlockFormat::Etc2Rgba8,
        F::EAC_R11_UNORM_BLOCK => BlockFormat::EacR11,
        F::EAC_R11G11_UNORM_BLOCK => BlockFormat::EacRg11,
        F::ASTC_4x4_UNORM_BLOCK | F::ASTC_4x4_SRGB_BLOCK => BlockFormat::Astc4x4,
        _ => return None,
    })
}

/// Checks every level holds exactly the bytes its size needs, so nothing
/// short reaches the GPU or the decoders.
fn check_levels(file: &File, path: &str, what: &str) -> Result<(), Error> {
    if file.levels.is_empty() {
        return Err(invalid(path, what, "it has no mip levels"));
    }
    if file.levels.len() > 32 {
        return Err(invalid(path, what, "it has more than 32 mip levels"));
    }
    for (i, level) in file.levels.iter().enumerate() {
        let (w, h) = ((file.width >> i).max(1), (file.height >> i).max(1));
        let expected = match file.pixels {
            Pixels::Blocks(format) => format.level_bytes(w, h),
            Pixels::Rgba8 => u64::from(w) * u64::from(h) * 4,
            // UASTC blocks are 16 bytes each.
            Pixels::Uastc(_) => BlockFormat::Bc7.level_bytes(w, h),
        };
        if level.len() as u64 != expected {
            return Err(invalid(
                path,
                what,
                format_args!("mip level {i} ({w} x {h}) has {} bytes, not {expected}", level.len()),
            ));
        }
    }
    Ok(())
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Reads a DDS file of BCn blocks; `None` if it holds plain pixels (for
/// the image decoder, feature `image-dds`).
fn read_dds(bytes: &[u8], path: &str, what: &str) -> Option<Result<File, Error>> {
    const HEADER: usize = 128;
    const DX10: usize = 20;
    const MIPMAP_COUNT: u32 = 0x2_0000;
    const FOURCC: u32 = 0x4;
    const CUBEMAP: u32 = 0x200;
    const VOLUME: u32 = 0x20_0000;
    let short = || Some(Err(invalid(path, what, "the DDS header is cut short")));
    let (Some(height), Some(width), Some(flags), Some(mips), Some(pf_flags), Some(caps2)) = (
        read_u32(bytes, 12),
        read_u32(bytes, 16),
        read_u32(bytes, 8),
        read_u32(bytes, 28),
        read_u32(bytes, 80),
        read_u32(bytes, 112),
    ) else {
        return short();
    };
    if pf_flags & FOURCC == 0 {
        return None;
    }
    let fourcc = bytes.get(84..88)?;
    let (format, srgb, data) = match fourcc {
        b"DXT1" => (BlockFormat::Bc1, true, HEADER),
        b"DXT2" | b"DXT3" => (BlockFormat::Bc2, true, HEADER),
        b"DXT4" | b"DXT5" => (BlockFormat::Bc3, true, HEADER),
        b"ATI1" | b"BC4U" => (BlockFormat::Bc4, false, HEADER),
        b"ATI2" | b"BC5U" => (BlockFormat::Bc5, false, HEADER),
        b"DX10" => {
            let (Some(dxgi), Some(array)) = (read_u32(bytes, HEADER), read_u32(bytes, HEADER + 12)) else {
                return short();
            };
            if array > 1 {
                return Some(Err(unsupported(
                    path,
                    format!(
                        "{what} is a DDS array of {array} layers; load each layer from its own file with load_texture_array"
                    ),
                )));
            }
            let (format, srgb) = match dxgi {
                71 => (BlockFormat::Bc1, false),
                72 => (BlockFormat::Bc1, true),
                74 => (BlockFormat::Bc2, false),
                75 => (BlockFormat::Bc2, true),
                77 => (BlockFormat::Bc3, false),
                78 => (BlockFormat::Bc3, true),
                80 => (BlockFormat::Bc4, false),
                83 => (BlockFormat::Bc5, false),
                98 => (BlockFormat::Bc7, false),
                99 => (BlockFormat::Bc7, true),
                // Plain pixels (or a format ponko doesn't read): the image decoder's.
                _ => return None,
            };
            (format, srgb, HEADER + DX10)
        }
        _ => return None,
    };
    if caps2 & (CUBEMAP | VOLUME) != 0 {
        return Some(Err(unsupported(
            path,
            format!("{what} is a DDS cube map or volume; ponko loads 2D DDS textures only"),
        )));
    }
    if width == 0 || height == 0 {
        return Some(Err(invalid(path, what, "its size is zero")));
    }
    let count = if flags & MIPMAP_COUNT != 0 { mips.max(1) } else { 1 }.min(32);
    let mut levels = Vec::with_capacity(count as usize);
    let mut at = data;
    for i in 0..count {
        let (w, h) = ((width >> i).max(1), (height >> i).max(1));
        let len = format.level_bytes(w, h) as usize;
        let Some(level) = at.checked_add(len).and_then(|end| bytes.get(at..end)) else {
            return Some(Err(invalid(
                path,
                what,
                format_args!("mip level {i} ({w} x {h}) is cut short"),
            )));
        };
        levels.push(level.to_vec());
        at += len;
    }
    Some(Ok(File {
        width,
        height,
        pixels: Pixels::Blocks(format),
        srgb,
        levels,
    }))
}

/// Chooses what to upload for a GPU that samples `gpu`: the file's blocks,
/// UASTC transcoded, or RGBA8 decoded on the CPU.
fn for_gpu(file: File, path: &str, what: &str, gpu: GpuFormats) -> Result<Decoded, Error> {
    match file.pixels {
        Pixels::Rgba8 => {
            let mut levels = file.levels;
            Ok(Decoded::Rgba(Image {
                width: file.width,
                height: file.height,
                rgba: levels.swap_remove(0),
            }))
        }
        Pixels::Blocks(format) => {
            if format.sampled_by(gpu) && block_aligned(file.width, file.height) {
                Ok(Decoded::Blocks(CompressedImage {
                    width: file.width,
                    height: file.height,
                    format,
                    srgb: file.srgb && format.has_srgb(),
                    normal: if format.two_channel() {
                        NormalChannels::Rg
                    } else {
                        NormalChannels::Rgb
                    },
                    levels: file.levels,
                }))
            } else {
                decode_blocks(format, file.width, file.height, &file.levels[0])
                    .map(Decoded::Rgba)
                    .map_err(|e| invalid(path, what, e))
            }
        }
        Pixels::Uastc(channels) => transcode_uastc(file, channels, path, what, gpu),
    }
}

/// Decodes one level of `format` blocks to RGBA8 on the CPU. A two-channel
/// format gets Z rebuilt into blue, so it reads as an ordinary normal map.
pub(crate) fn decode_blocks(format: BlockFormat, width: u32, height: u32, blocks: &[u8]) -> Result<Image, String> {
    use texture2ddecoder as t;
    let (w, h) = (width as usize, height as usize);
    let mut pixels = vec![0u32; w * h];
    let decoded = match format {
        BlockFormat::Bc1 => t::decode_bc1(blocks, w, h, &mut pixels),
        BlockFormat::Bc2 => t::decode_bc2(blocks, w, h, &mut pixels),
        BlockFormat::Bc3 => t::decode_bc3(blocks, w, h, &mut pixels),
        BlockFormat::Bc4 => t::decode_bc4(blocks, w, h, &mut pixels),
        BlockFormat::Bc5 => t::decode_bc5(blocks, w, h, &mut pixels),
        BlockFormat::Bc7 => t::decode_bc7(blocks, w, h, &mut pixels),
        BlockFormat::Etc2Rgb8 => t::decode_etc2_rgb(blocks, w, h, &mut pixels),
        BlockFormat::Etc2Rgba8 => t::decode_etc2_rgba8(blocks, w, h, &mut pixels),
        BlockFormat::EacR11 => t::decode_eacr(blocks, w, h, &mut pixels),
        BlockFormat::EacRg11 => t::decode_eacrg(blocks, w, h, &mut pixels),
        BlockFormat::Astc4x4 => t::decode_astc_4_4(blocks, w, h, &mut pixels),
    };
    decoded.map_err(|e| format!("its {} blocks don't decode: {e}", format.name()))?;
    // The decoder writes each pixel as little-endian B, G, R, A.
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in pixels {
        let [b, g, r, a] = p.to_le_bytes();
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    if format.two_channel() {
        rebuild_normal_z(&mut rgba);
    }
    Ok(Image { width, height, rgba })
}

/// Fills blue with a two-channel normal map's Z (from X in red and Y in
/// green), and makes alpha opaque.
fn rebuild_normal_z(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        let x = f32::from(p[0]) / 127.5 - 1.0;
        let y = f32::from(p[1]) / 127.5 - 1.0;
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        p[2] = ((z * 0.5 + 0.5) * 255.0).round() as u8;
        p[3] = 255;
    }
}

#[cfg(all(feature = "basis-universal", not(target_arch = "wasm32")))]
fn transcode_uastc(
    file: File,
    channels: UastcChannels,
    path: &str,
    what: &str,
    gpu: GpuFormats,
) -> Result<Decoded, Error> {
    use basis_universal::{DecodeFlags, LowLevelUastcTranscoder, SliceParametersUastc, TranscoderBlockFormat as T};
    // Two-channel files: X and Y (red and alpha) into BC5 or EAC RG11's two
    // channels, else into an RGBA format that keeps them in red and alpha.
    let rrrg = channels == UastcChannels::Rrrg;
    let two = |rg| if rrrg { rg } else { NormalChannels::Rgb };
    let target = if gpu.bc {
        Some(if rrrg {
            (T::BC5, BlockFormat::Bc5, NormalChannels::Rg)
        } else {
            (T::BC7, BlockFormat::Bc7, two(NormalChannels::Ra))
        })
    } else if gpu.astc {
        Some((T::ASTC_4x4, BlockFormat::Astc4x4, two(NormalChannels::Ra)))
    } else if gpu.etc2 {
        Some(if rrrg {
            (T::ETC2_EAC_RG11, BlockFormat::EacRg11, NormalChannels::Rg)
        } else {
            (T::ETC2_RGBA, BlockFormat::Etc2Rgba8, NormalChannels::Rgb)
        })
    } else {
        None
    };
    let target = target.filter(|_| block_aligned(file.width, file.height));
    let has_alpha = matches!(channels, UastcChannels::Rgba | UastcChannels::Rrrg);
    let transcoder = LowLevelUastcTranscoder::new();
    let slice = |level: &[u8], w: u32, h: u32, to: T| {
        transcoder
            .transcode_slice(
                level,
                SliceParametersUastc {
                    num_blocks_x: w.div_ceil(BLOCK),
                    num_blocks_y: h.div_ceil(BLOCK),
                    has_alpha,
                    original_width: w,
                    original_height: h,
                },
                DecodeFlags::HIGH_QUALITY,
                to,
            )
            .map_err(|e| invalid(path, what, format_args!("its UASTC blocks don't transcode: {e:?}")))
    };
    let Some((to, format, normal)) = target else {
        // No block format to use: RGBA8 of the full size, through BC7 (BC5
        // for two channels) decoded on the CPU. Not the transcoder's own
        // RGBA32 output: the `basis-universal` crate sizes its buffer for
        // that with a row pitch a quarter of the width, which the C++
        // transcoder then writes past.
        let (to, format) = if rrrg {
            (T::BC5, BlockFormat::Bc5)
        } else {
            (T::BC7, BlockFormat::Bc7)
        };
        let blocks = slice(&file.levels[0], file.width, file.height, to)?;
        return decode_blocks(format, file.width, file.height, &blocks)
            .map(Decoded::Rgba)
            .map_err(|e| invalid(path, what, e));
    };
    let mut levels = Vec::with_capacity(file.levels.len());
    for (i, level) in file.levels.iter().enumerate() {
        let (w, h) = ((file.width >> i).max(1), (file.height >> i).max(1));
        levels.push(slice(level, w, h, to)?);
    }
    Ok(Decoded::Blocks(CompressedImage {
        width: file.width,
        height: file.height,
        format,
        srgb: file.srgb && format.has_srgb(),
        levels,
        normal,
    }))
}

#[cfg(not(all(feature = "basis-universal", not(target_arch = "wasm32"))))]
fn transcode_uastc(
    _file: File,
    _channels: UastcChannels,
    path: &str,
    what: &str,
    _gpu: GpuFormats,
) -> Result<Decoded, Error> {
    let why = if cfg!(target_arch = "wasm32") {
        "web builds have no Basis Universal transcoder"
    } else {
        "this build of ponko has no Basis Universal transcoder: turn on its `basis-universal` cargo feature"
    };
    Err(unsupported(
        path,
        format!("{what} is Basis Universal UASTC, and {why}. BC7 files from `ponko textures` load everywhere"),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal KTX2 file of `format` (Vulkan value, 0 for UASTC) and
    /// `levels`, for tests.
    pub(crate) fn ktx2_file(
        format: Option<ktx2::Format>,
        width: u32,
        height: u32,
        levels: &[Vec<u8>],
        zstd: bool,
    ) -> Vec<u8> {
        let dfd = match format {
            Some(f) => ktx2::dfd::Block::Basic(ktx2::dfd::Basic::from_format(f).expect("a DFD").0),
            None => ktx2::dfd::Block::Basic(uastc_dfd()),
        }
        .to_vec();
        let dfd_len = 4 + dfd.len();
        let index_len = 24 * levels.len();
        let dfd_at = 80 + index_len;
        let packed: Vec<Vec<u8>> = levels
            .iter()
            .map(|l| {
                if zstd {
                    ruzstd::encoding::compress_to_vec(l.as_slice(), ruzstd::encoding::CompressionLevel::Fastest)
                } else {
                    l.clone()
                }
            })
            .collect();
        let mut data_at = (dfd_at + dfd_len).next_multiple_of(16);
        let mut offsets = Vec::new();
        for l in &packed {
            offsets.push(data_at);
            data_at = (data_at + l.len()).next_multiple_of(16);
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
            supercompression_scheme: zstd.then_some(ktx2::SupercompressionScheme::Zstandard),
            index: ktx2::Index {
                dfd_byte_offset: dfd_at as u32,
                dfd_byte_length: dfd_len as u32,
                kvd_byte_offset: 0,
                kvd_byte_length: 0,
                sgd_byte_offset: 0,
                sgd_byte_length: 0,
            },
        };
        let mut out = header.as_bytes().to_vec();
        for ((l, p), at) in levels.iter().zip(&packed).zip(&offsets) {
            out.extend_from_slice(
                &ktx2::LevelIndex {
                    byte_offset: *at as u64,
                    byte_length: p.len() as u64,
                    uncompressed_byte_length: l.len() as u64,
                }
                .as_bytes(),
            );
        }
        out.extend_from_slice(&(dfd_len as u32).to_le_bytes());
        out.extend_from_slice(&dfd);
        for (p, at) in packed.iter().zip(&offsets) {
            out.resize(*at, 0);
            out.extend_from_slice(p);
        }
        out
    }

    fn uastc_dfd() -> ktx2::dfd::Basic {
        let mut dfd = ktx2::dfd::Basic::from_format(ktx2::Format::BC7_SRGB_BLOCK)
            .expect("a DFD")
            .0;
        dfd.color_model = Some(ktx2::ColorModel::UASTC);
        dfd
    }

    /// One BC1 block of a single color (565): both endpoints `color`,
    /// every index 0.
    fn bc1_block(color: u16) -> [u8; 8] {
        let [a, b] = color.to_le_bytes();
        [a, b, a, b, 0, 0, 0, 0]
    }

    fn solid_bc1(width: u32, height: u32, color: u16) -> Vec<u8> {
        let blocks = (width.div_ceil(4) * height.div_ceil(4)) as usize;
        (0..blocks).flat_map(|_| bc1_block(color)).collect()
    }

    const RED_565: u16 = 0xF800;

    #[test]
    fn ktx2_blocks_upload_as_they_are_when_the_gpu_samples_them() {
        let levels = vec![
            solid_bc1(8, 8, RED_565),
            solid_bc1(4, 4, RED_565),
            solid_bc1(2, 2, RED_565),
        ];
        let file = ktx2_file(Some(ktx2::Format::BC1_RGBA_SRGB_BLOCK), 8, 8, &levels, false);
        let Some(Ok(Decoded::Blocks(image))) = decode(&file, "a.ktx2", "\"a.ktx2\"", GpuFormats::ALL) else {
            panic!("decoded as blocks");
        };
        assert_eq!((image.width, image.height, image.format), (8, 8, BlockFormat::Bc1));
        assert!(image.srgb);
        assert_eq!(image.levels, levels);
        assert_eq!(image.bytes(), 32 + 8 + 8);
    }

    #[test]
    fn zstd_supercompressed_levels_unpack() {
        let levels = vec![solid_bc1(16, 16, RED_565), solid_bc1(8, 8, RED_565)];
        let file = ktx2_file(Some(ktx2::Format::BC1_RGBA_UNORM_BLOCK), 16, 16, &levels, true);
        let Some(Ok(Decoded::Blocks(image))) = decode(&file, "a.ktx2", "a", GpuFormats::ALL) else {
            panic!("decoded as blocks");
        };
        assert!(!image.srgb);
        assert_eq!(image.levels, levels);
    }

    #[test]
    fn a_gpu_without_the_format_gets_rgba_decoded_on_the_cpu() {
        let file = ktx2_file(
            Some(ktx2::Format::BC1_RGBA_SRGB_BLOCK),
            8,
            4,
            &[solid_bc1(8, 4, RED_565)],
            false,
        );
        let Some(Ok(Decoded::Rgba(image))) = decode(&file, "a.ktx2", "a", GpuFormats::default()) else {
            panic!("decoded to RGBA");
        };
        assert_eq!((image.width, image.height), (8, 4));
        assert!(
            image.rgba.chunks(4).all(|p| p == [255, 0, 0, 255]),
            "{:?}",
            &image.rgba[..8]
        );
    }

    #[test]
    fn sizes_that_are_not_whole_blocks_decode_on_the_cpu() {
        // A GPU texture of a block format must be whole blocks.
        let file = ktx2_file(
            Some(ktx2::Format::BC1_RGBA_SRGB_BLOCK),
            6,
            5,
            &[solid_bc1(6, 5, RED_565)],
            false,
        );
        let Some(Ok(Decoded::Rgba(image))) = decode(&file, "a.ktx2", "a", GpuFormats::ALL) else {
            panic!("decoded to RGBA");
        };
        assert_eq!((image.width, image.height, image.rgba.len()), (6, 5, 6 * 5 * 4));
    }

    #[test]
    fn two_channel_normal_maps_are_marked_and_their_cpu_decode_gets_z() {
        // BC5: red 255 (X = 1), green 128 (Y = 0) in every pixel.
        let channel = |v: u8| [v, v, 0, 0, 0, 0, 0, 0];
        let block: Vec<u8> = [channel(128), channel(128)].concat();
        let file = ktx2_file(Some(ktx2::Format::BC5_UNORM_BLOCK), 4, 4, &[block], false);
        let Some(Ok(Decoded::Blocks(image))) = decode(&file, "n.ktx2", "n", GpuFormats::ALL) else {
            panic!("blocks");
        };
        assert_eq!(image.normal, NormalChannels::Rg);
        assert!(!image.srgb, "BC5 has no sRGB variant");
        let Some(Ok(Decoded::Rgba(flat))) = decode(&file, "n.ktx2", "n", GpuFormats::default()) else {
            panic!("rgba");
        };
        // X = Y = 0: Z is 1, so blue is full.
        assert!(
            flat.rgba.chunks(4).all(|p| p[2] >= 254 && p[3] == 255),
            "{:?}",
            &flat.rgba[..4]
        );
    }

    #[test]
    fn short_levels_are_decode_errors_not_gpu_crashes() {
        let file = ktx2_file(Some(ktx2::Format::BC7_SRGB_BLOCK), 8, 8, &[vec![0; 16 * 3]], false);
        let err = decode(&file, "a.ktx2", "\"a.ktx2\"", GpuFormats::ALL)
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(err.kind(), ErrorKind::Decode);
        assert!(err.message().contains("mip level 0"), "{}", err.message());
    }

    #[test]
    fn cube_maps_and_unknown_formats_are_unsupported() {
        let mut file = ktx2_file(Some(ktx2::Format::R16_SFLOAT), 4, 4, &[vec![0; 32]], false);
        let err = decode(&file, "a.ktx2", "a", GpuFormats::ALL).unwrap().err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Unsupported);
        file = ktx2_file(Some(ktx2::Format::BC7_SRGB_BLOCK), 4, 4, &[vec![0; 16]], false);
        file[36..40].copy_from_slice(&6u32.to_le_bytes());
        let err = decode(&file, "a.ktx2", "a", GpuFormats::ALL).unwrap().err().unwrap();
        assert!(err.message().contains("cube map"), "{}", err.message());
    }

    #[test]
    fn uastc_without_a_transcoder_names_the_way_out() {
        let file = ktx2_file(None, 4, 4, &[vec![0; 16]], false);
        let result = decode(&file, "a.ktx2", "\"a.ktx2\"", GpuFormats::ALL).unwrap();
        if cfg!(all(feature = "basis-universal", not(target_arch = "wasm32"))) {
            // All-zero blocks are a valid (if dull) UASTC block mode.
            let _ = result;
        } else {
            let err = result.err().unwrap();
            assert_eq!(err.kind(), ErrorKind::Unsupported);
            assert!(err.message().contains("basis-universal"), "{}", err.message());
        }
    }

    fn dds(fourcc: &[u8; 4], width: u32, height: u32, levels: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0u8; 128];
        out[..4].copy_from_slice(b"DDS ");
        out[4..8].copy_from_slice(&124u32.to_le_bytes());
        out[8..12].copy_from_slice(&(0x1007u32 | 0x2_0000).to_le_bytes());
        out[12..16].copy_from_slice(&height.to_le_bytes());
        out[16..20].copy_from_slice(&width.to_le_bytes());
        out[28..32].copy_from_slice(&(levels.len() as u32).to_le_bytes());
        out[76..80].copy_from_slice(&32u32.to_le_bytes());
        out[80..84].copy_from_slice(&4u32.to_le_bytes());
        out[84..88].copy_from_slice(fourcc);
        for l in levels {
            out.extend_from_slice(l);
        }
        out
    }

    #[test]
    fn dds_bc_files_load_with_their_mipmaps() {
        let levels = vec![
            solid_bc1(8, 8, RED_565),
            solid_bc1(4, 4, RED_565),
            solid_bc1(2, 2, RED_565),
            solid_bc1(1, 1, RED_565),
        ];
        let file = dds(b"DXT1", 8, 8, &levels);
        let Some(Ok(Decoded::Blocks(image))) = decode(&file, "a.dds", "a", GpuFormats::ALL) else {
            panic!("blocks");
        };
        assert_eq!(image.format, BlockFormat::Bc1);
        assert_eq!(image.levels.len(), 4);
        let cut = &file[..file.len() - 4];
        let err = decode(cut, "a.dds", "a", GpuFormats::ALL).unwrap().err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Decode);
    }

    #[test]
    fn dds_files_of_plain_pixels_are_left_to_the_image_decoder() {
        let mut file = dds(b"DXT1", 4, 4, &[]);
        file[80..84].copy_from_slice(&0x41u32.to_le_bytes()); // RGB | alpha, no FourCC
        assert!(decode(&file, "a.dds", "a", GpuFormats::ALL).is_none());
        assert!(decode(b"\x89PNG", "a.png", "a", GpuFormats::ALL).is_none());
    }

    /// The fixture card (`tests/fixtures/textures`), and its pixels.
    fn card_png() -> Vec<u8> {
        image::load_from_memory(include_bytes!("../tests/fixtures/textures/card.png"))
            .unwrap()
            .into_rgba8()
            .into_raw()
    }

    fn mean_error(a: &[u8], b: &[u8]) -> f32 {
        let sum: f32 = a
            .chunks(4)
            .zip(b.chunks(4))
            .map(|(p, q)| (0..3).map(|c| (f32::from(p[c]) - f32::from(q[c])).abs()).sum::<f32>())
            .sum();
        sum / (a.len() / 4 * 3) as f32
    }

    #[test]
    fn bc7_from_ponko_textures_decodes_close_to_its_source() {
        let file = include_bytes!("../tests/fixtures/textures/card.ktx2");
        let Some(Ok(Decoded::Rgba(image))) = decode(file, "card.ktx2", "card", GpuFormats::default()) else {
            panic!("rgba");
        };
        let source = card_png();
        let error = mean_error(&image.rgba, &source);
        assert!(error < 2.0, "mean error {error}");
        // Negative control: another image is far from it.
        let inverted: Vec<u8> = source.iter().map(|v| 255 - v).collect();
        assert!(mean_error(&image.rgba, &inverted) > 40.0);
    }

    #[cfg(all(feature = "basis-universal", not(target_arch = "wasm32")))]
    #[test]
    fn uastc_transcodes_to_what_each_gpu_samples() {
        let file = include_bytes!("../tests/fixtures/textures/card_uastc.ktx2");
        let source = card_png();
        let cases = [
            (
                GpuFormats {
                    bc: true,
                    etc2: true,
                    astc: true,
                },
                Some(BlockFormat::Bc7),
            ),
            (
                GpuFormats {
                    astc: true,
                    etc2: true,
                    ..Default::default()
                },
                Some(BlockFormat::Astc4x4),
            ),
            (
                GpuFormats {
                    etc2: true,
                    ..Default::default()
                },
                Some(BlockFormat::Etc2Rgba8),
            ),
            (GpuFormats::default(), None),
        ];
        for (gpu, expected) in cases {
            let decoded = decode(file, "card_uastc.ktx2", "card", gpu).unwrap().unwrap();
            let rgba = match (decoded, expected) {
                (Decoded::Blocks(image), Some(format)) => {
                    assert_eq!(image.format, format);
                    assert!(image.srgb);
                    assert_eq!(image.levels.len(), 7);
                    decode_blocks(image.format, 64, 64, &image.levels[0]).unwrap().rgba
                }
                (Decoded::Rgba(image), None) => image.rgba,
                _ => panic!("{gpu:?}: not {expected:?}"),
            };
            let error = mean_error(&rgba, &source);
            assert!(error < 4.0, "{gpu:?}: mean error {error}");
        }
    }

    #[cfg(all(feature = "basis-universal", not(target_arch = "wasm32")))]
    #[test]
    fn two_channel_uastc_goes_to_bc5_or_keeps_y_in_alpha() {
        // A normal map from `ponko textures` with "basis": true (RRRG).
        let file = include_bytes!("../tests/fixtures/textures/bumps_uastc.ktx2");
        let source = image::load_from_memory(include_bytes!("../tests/fixtures/textures/bumps.png"))
            .unwrap()
            .into_rgba8()
            .into_raw();
        let xy_error = |rgba: &[u8], y: usize| {
            let sum: f32 = rgba
                .chunks(4)
                .zip(source.chunks(4))
                .map(|(p, q)| (f32::from(p[0]) - f32::from(q[0])).abs() + (f32::from(p[y]) - f32::from(q[1])).abs())
                .sum();
            sum / (rgba.len() / 4 * 2) as f32
        };
        let bc = GpuFormats {
            bc: true,
            ..Default::default()
        };
        let Decoded::Blocks(bc5) = decode(file, "b", "b", bc).unwrap().unwrap() else {
            panic!("blocks")
        };
        assert_eq!(
            (bc5.format, bc5.normal, bc5.srgb),
            (BlockFormat::Bc5, NormalChannels::Rg, false)
        );
        let error = xy_error(&decode_blocks(bc5.format, 64, 64, &bc5.levels[0]).unwrap().rgba, 1);
        assert!(error < 3.0, "BC5: {error}");
        let astc = GpuFormats {
            astc: true,
            ..Default::default()
        };
        let Decoded::Blocks(rgba) = decode(file, "b", "b", astc).unwrap().unwrap() else {
            panic!("blocks")
        };
        assert_eq!((rgba.format, rgba.normal), (BlockFormat::Astc4x4, NormalChannels::Ra));
        let error = xy_error(&decode_blocks(rgba.format, 64, 64, &rgba.levels[0]).unwrap().rgba, 3);
        assert!(error < 3.0, "ASTC, Y in alpha: {error}");
        // No block format: RGBA8 with X, Y and the rebuilt Z, like the source.
        let Decoded::Rgba(flat) = decode(file, "b", "b", GpuFormats::default()).unwrap().unwrap() else {
            panic!("rgba")
        };
        assert!(xy_error(&flat.rgba, 1) < 3.0);
        let z: f32 = flat
            .rgba
            .chunks(4)
            .zip(source.chunks(4))
            .map(|(p, q)| (f32::from(p[2]) - f32::from(q[2])).abs())
            .sum();
        assert!(z / 4096.0 < 4.0, "Z: {}", z / 4096.0);
    }

    #[test]
    fn streaming_levels_start_further_down_the_chain() {
        let levels = vec![
            solid_bc1(16, 8, RED_565),
            solid_bc1(8, 4, RED_565),
            solid_bc1(4, 2, RED_565),
        ];
        let image = CompressedImage {
            width: 16,
            height: 8,
            format: BlockFormat::Bc1,
            srgb: true,
            levels: levels.clone(),
            normal: NormalChannels::Rgb,
        };
        let half = image.starting_at_level(1);
        assert_eq!((half.width, half.height), (8, 4));
        assert_eq!(half.levels, levels[1..]);
    }
}
