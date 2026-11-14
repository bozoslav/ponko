//! Block-compressed textures on the GPU (feature `compressed-textures`):
//! which formats the device samples, and uploading a
//! [`CompressedImage`](crate::compressed::CompressedImage) with its mipmaps.
use crate::compressed::{BlockFormat, CompressedImage, GpuFormats};
use crate::render::traffic::{Counted, Upload};

/// The device features that sample block formats, asked for where the
/// adapter has them.
pub(crate) const FEATURES: wgpu::Features = wgpu::Features::TEXTURE_COMPRESSION_BC
    .union(wgpu::Features::TEXTURE_COMPRESSION_ETC2)
    .union(wgpu::Features::TEXTURE_COMPRESSION_ASTC);

/// The block format families a device with `features` samples.
pub(crate) fn gpu_formats(features: wgpu::Features) -> GpuFormats {
    GpuFormats {
        bc: features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
        etc2: features.contains(wgpu::Features::TEXTURE_COMPRESSION_ETC2),
        astc: features.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC),
    }
}

/// The wgpu format of `format` blocks, sampled through sRGB decoding when
/// `srgb` (and the format has an sRGB variant).
pub(crate) fn wgpu_format(format: BlockFormat, srgb: bool) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as T;
    match (format, srgb) {
        (BlockFormat::Bc1, false) => T::Bc1RgbaUnorm,
        (BlockFormat::Bc1, true) => T::Bc1RgbaUnormSrgb,
        (BlockFormat::Bc2, false) => T::Bc2RgbaUnorm,
        (BlockFormat::Bc2, true) => T::Bc2RgbaUnormSrgb,
        (BlockFormat::Bc3, false) => T::Bc3RgbaUnorm,
        (BlockFormat::Bc3, true) => T::Bc3RgbaUnormSrgb,
        (BlockFormat::Bc4, _) => T::Bc4RUnorm,
        (BlockFormat::Bc5, _) => T::Bc5RgUnorm,
        (BlockFormat::Bc7, false) => T::Bc7RgbaUnorm,
        (BlockFormat::Bc7, true) => T::Bc7RgbaUnormSrgb,
        (BlockFormat::Etc2Rgb8, false) => T::Etc2Rgb8Unorm,
        (BlockFormat::Etc2Rgb8, true) => T::Etc2Rgb8UnormSrgb,
        (BlockFormat::Etc2Rgba8, false) => T::Etc2Rgba8Unorm,
        (BlockFormat::Etc2Rgba8, true) => T::Etc2Rgba8UnormSrgb,
        (BlockFormat::EacR11, _) => T::EacR11Unorm,
        (BlockFormat::EacRg11, _) => T::EacRg11Unorm,
        (BlockFormat::Astc4x4, srgb) => T::Astc {
            block: wgpu::AstcBlock::B4x4,
            channel: if srgb {
                wgpu::AstcChannel::UnormSrgb
            } else {
                wgpu::AstcChannel::Unorm
            },
        },
    }
}

/// Makes a texture of `layers` (one for a plain texture; an array's
/// layers otherwise, all of one size and format) and uploads every mip
/// level they all have. `array`: a texture array (always viewed as one).
///
/// The caller has checked the device samples the format and the size is
/// whole blocks (`compressed::block_aligned`).
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    layers: &[&CompressedImage],
) -> wgpu::Texture {
    let first = layers[0];
    let levels = layers.iter().map(|l| l.levels.len()).min().unwrap_or(1).max(1);
    let format = wgpu_format(first.format, first.srgb);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: first.width,
            height: first.height,
            depth_or_array_layers: layers.len() as u32,
        },
        mip_level_count: levels as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let block_bytes = first.format.block_bytes();
    for level in 0..levels {
        let (w, h) = first.level_size(level);
        // A level smaller than a block still takes a whole block.
        let (blocks_x, blocks_y) = (
            w.div_ceil(crate::compressed::BLOCK),
            h.div_ceil(crate::compressed::BLOCK),
        );
        for (z, layer) in layers.iter().enumerate() {
            queue.uploading(Upload::Textures).write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: z as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &layer.levels[level],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(blocks_x * block_bytes),
                    rows_per_image: Some(blocks_y),
                },
                wgpu::Extent3d {
                    width: blocks_x * crate::compressed::BLOCK,
                    height: blocks_y * crate::compressed::BLOCK,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture
}
