//! Offscreen GPU test of block-compressed textures (feature
//! `compressed-textures`): files from `ponko textures`, uploaded as blocks
//! with `blocks::upload`, sample on the GPU like their source PNGs within
//! a stated tolerance, at full size and down the mip chain; the CPU
//! fallback decodes them as the GPU does; and uploading them in the wrong
//! color space (the negative control) breaks the tolerance. Skips without
//! a GPU that samples BC formats.
use super::render_test_support::{Image, TestGpu, Tolerance, diff_within, gpu_with, read_texture, target};
use crate::compressed::{self, Decoded, GpuFormats};

const CARD_PNG: &[u8] = include_bytes!("../../tests/fixtures/textures/card.png");
const CARD_BC7: &[u8] = include_bytes!("../../tests/fixtures/textures/card.ktx2");
const BUMPS_PNG: &[u8] = include_bytes!("../../tests/fixtures/textures/bumps.png");
const BUMPS_BC5: &[u8] = include_bytes!("../../tests/fixtures/textures/bumps.ktx2");

/// BC7 against its source: each channel within 16 of 255, beyond that on
/// at most 1% of the pixels (the sharp edges of the test card). Measured
/// on an Apple M4 Max (Metal): at most 8 apart, mean 0.16; read as linear
/// (the negative control), 84% of the pixels are beyond it.
const BC7: Tolerance = Tolerance {
    channel: 16,
    outliers: 0.01,
    edge: 0,
};

/// A mip level against the source averaged on the CPU: the encoder filters
/// with a wider kernel than a 2 x 2 box, so edges differ more. Measured:
/// 2.9% of the pixels beyond 24, mean 4.3.
const MIP: Tolerance = Tolerance {
    channel: 24,
    outliers: 0.05,
    edge: 0,
};

/// The CPU decode against the GPU's: BC7 decoding is exact, so at most a
/// rounding step apart.
const SAME_DECODE: Tolerance = Tolerance {
    channel: 1,
    outliers: 0.0,
    edge: 0,
};

struct Rig {
    gpu: TestGpu,
    layout: wgpu::BindGroupLayout,
    srgb: wgpu::RenderPipeline,
    linear: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl Rig {
    fn new() -> Option<Self> {
        let gpu = gpu_with(|adapter| {
            if adapter.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
                Ok(wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
                    ..Default::default()
                })
            } else {
                Err("needs BC texture compression".into())
            }
        })?;
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compressed test"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                @group(0) @binding(0) var image: texture_2d<f32>;
                @group(0) @binding(1) var nearest: sampler;
                struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
                @vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
                    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
                    return Out(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
                }
                @fragment fn fs(in: Out) -> @location(0) vec4<f32> {
                    return textureSampleLevel(image, nearest, in.uv, 0.0);
                }
                "#
                .into(),
            ),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let srgb = pipeline(wgpu::TextureFormat::Rgba8UnormSrgb);
        let linear = pipeline(wgpu::TextureFormat::Rgba8Unorm);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        Some(Self {
            layout,
            srgb,
            linear,
            sampler,
            gpu,
        })
    }

    /// Draws mip level `level` of `texture` texel for texel into a target
    /// of that level's size (sRGB, or linear for data).
    fn draw(&self, texture: &wgpu::Texture, level: u32, srgb: bool) -> Image {
        let device = &self.gpu.device;
        let size = ((texture.width() >> level).max(1), (texture.height() >> level).max(1));
        let format = if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let output = target(device, size, format);
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let target_view = output.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(if srgb { &self.srgb } else { &self.linear });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.gpu.queue.submit([encoder.finish()]);
        Image {
            width: size.0,
            height: size.1,
            rgba: read_texture(device, &self.gpu.queue, &output),
        }
    }

    /// An RGBA8 texture of `rgba` (sRGB, or linear for data).
    fn rgba(&self, width: u32, height: u32, rgba: &[u8], srgb: bool) -> wgpu::Texture {
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.gpu.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: None,
            },
            texture.size(),
        );
        texture
    }

    fn blocks(&self, file: &[u8]) -> compressed::CompressedImage {
        match compressed::decode(file, "fixture.ktx2", "the fixture", GpuFormats::ALL) {
            Some(Ok(Decoded::Blocks(image))) => image,
            _ => panic!("the fixture decodes as blocks"),
        }
    }

    fn upload(&self, image: &compressed::CompressedImage) -> wgpu::Texture {
        super::blocks::upload(&self.gpu.device, &self.gpu.queue, "compressed test", &[image])
    }
}

fn png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let image = image::load_from_memory(bytes).unwrap().into_rgba8();
    (image.width(), image.height(), image.into_raw())
}

/// Halves an sRGB image by averaging 2 x 2 blocks in linear light.
fn halve_srgb(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let to_linear = |v: u8| {
        let c = f32::from(v) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |l: f32| {
        let c = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    };
    let (w, h) = (width / 2, height / 2);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let at = |dx, dy| rgba[(((2 * y + dy) * width + 2 * x + dx) * 4 + c) as usize];
                let samples = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
                out.push(if c == 3 {
                    (samples.iter().map(|&v| u32::from(v)).sum::<u32>() / 4) as u8
                } else {
                    to_srgb(samples.iter().map(|&v| to_linear(v)).sum::<f32>() / 4.0)
                });
            }
        }
    }
    out
}

#[test]
fn bc7_textures_sample_like_their_source_and_the_wrong_color_space_does_not() {
    let Some(rig) = Rig::new() else {
        return;
    };
    let (w, h, source) = png(CARD_PNG);
    let reference = rig.draw(&rig.rgba(w, h, &source, true), 0, true);
    let image = rig.blocks(CARD_BC7);
    assert_eq!(image.format, compressed::BlockFormat::Bc7);
    let bc7 = rig.upload(&image);
    assert_eq!(bc7.format(), wgpu::TextureFormat::Bc7RgbaUnormSrgb);
    assert_eq!(bc7.mip_level_count(), 7);
    let drawn = rig.draw(&bc7, 0, true);
    let d = diff_within(&drawn, &reference, BC7);
    eprintln!("BC7 against its source: {d}");
    assert!(d.within(BC7), "BC7 against its source: {d}");

    // Negative control: the same blocks read as linear data, a color-space
    // mix-up, are far outside the tolerance.
    let mut wrong = image.clone();
    wrong.srgb = false;
    let wrong = rig.draw(&rig.upload(&wrong), 0, true);
    let d = diff_within(&wrong, &reference, BC7);
    eprintln!("BC7 read as linear (negative control): {d}");
    assert!(!d.within(BC7), "the tolerance must catch a color-space mix-up: {d}");
}

#[test]
fn bc7_mip_levels_sample_like_the_source_at_half_size() {
    let Some(rig) = Rig::new() else {
        return;
    };
    let (w, h, source) = png(CARD_PNG);
    let half = halve_srgb(w, h, &source);
    let reference = rig.draw(&rig.rgba(w / 2, h / 2, &half, true), 0, true);
    let bc7 = rig.upload(&rig.blocks(CARD_BC7));
    let drawn = rig.draw(&bc7, 1, true);
    let d = diff_within(&drawn, &reference, MIP);
    eprintln!("BC7 mip level 1 against the source halved: {d}");
    assert!(d.within(MIP), "mip level 1: {d}");
    // Negative control: level 0's top-left quarter is not level 1.
    let level0 = rig.draw(&bc7, 0, true);
    let quarter = Image {
        width: w / 2,
        height: h / 2,
        rgba: (0..h / 2)
            .flat_map(|y| level0.rgba[(y * w * 4) as usize..((y * w + w / 2) * 4) as usize].to_vec())
            .collect(),
    };
    assert!(!diff_within(&quarter, &reference, MIP).within(MIP));
}

#[test]
fn the_cpu_fallback_decodes_bc7_as_the_gpu_does() {
    let Some(rig) = Rig::new() else {
        return;
    };
    let gpu = rig.draw(&rig.upload(&rig.blocks(CARD_BC7)), 0, true);
    let Some(Ok(cpu)) = compressed::decode_rgba(CARD_BC7, "card.ktx2", "card") else {
        panic!("decodes to RGBA");
    };
    let cpu = rig.draw(&rig.rgba(cpu.width, cpu.height, &cpu.rgba, true), 0, true);
    let d = diff_within(&cpu, &gpu, SAME_DECODE);
    eprintln!("CPU against GPU BC7 decode: {d}");
    assert!(d.within(SAME_DECODE), "{d}");
}

#[test]
fn bc5_normal_maps_keep_x_and_y() {
    let Some(rig) = Rig::new() else {
        return;
    };
    let (w, h, source) = png(BUMPS_PNG);
    let image = rig.blocks(BUMPS_BC5);
    assert_eq!(rig.upload(&image).format(), wgpu::TextureFormat::Bc5RgUnorm);
    let drawn = rig.draw(&rig.upload(&image), 0, false);
    // BC5 has only red and green: compare those.
    let rg = |rgba: &[u8]| rgba.chunks(4).flat_map(|p| [p[0], p[1], 0, 255]).collect::<Vec<u8>>();
    let reference = Image {
        width: w,
        height: h,
        rgba: rg(&source),
    };
    let drawn = Image {
        width: w,
        height: h,
        rgba: rg(&drawn.rgba),
    };
    let d = diff_within(&drawn, &reference, BC7);
    eprintln!("BC5 X and Y against the source: {d}");
    assert!(d.within(BC7), "{d}");
    // Negative control: X and Y swapped are far outside the tolerance.
    let swapped = Image {
        width: w,
        height: h,
        rgba: drawn.rgba.chunks(4).flat_map(|p| [p[1], p[0], 0, 255]).collect(),
    };
    let d = diff_within(&swapped, &reference, BC7);
    eprintln!("BC5 with X and Y swapped (negative control): {d}");
    assert!(!d.within(BC7), "{d}");
}
