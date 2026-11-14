//! Boundary sampling checks and the original sky shader as a negative control.
use super::*;
use crate::render::render_test_support::{self, Image};

/// Recreate the original atlas lookup in the real atmosphere pass. Only the
/// sampling expression changes; the LUT, frame, sun and post remain identical.
pub(in crate::render) fn use_unbounded_sky(renderer: &mut Renderer3D, device: &wgpu::Device) {
    let sky = SKY.replace(
        "sky_radiance(dir)",
        "textureSampleLevel(sky_texture, sky_sampler, sky_uv(dir) * vec2<f32>(0.25, 128.0 / 1536.0), 0.0).rgb",
    );
    assert_ne!(sky, SKY, "negative control must replace the production lookup");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("unbounded sky negative control"),
        source: wgpu::ShaderSource::Wgsl(
            for_samples(
                &format!("{MESH_PRELUDE}\n{sky}"),
                renderer.samples,
                renderer.float_depth,
            )
            .into_owned()
            .into(),
        ),
    });
    renderer.sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("unbounded sky negative control"),
        layout: Some(&renderer.sky_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_far"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: renderer.samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_sky"),
            compilation_options: Default::default(),
            targets: &[Some(HDR_FORMAT.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
}

#[test]
fn sky_filter_wraps_longitude_and_keeps_both_poles_inside_its_region() {
    let Some(gpu) = render_test_support::gpu() else { return };
    let (device, queue) = (&gpu.device, &gpu.queue);
    // Deliberately different seam columns make a clamp-only fix fail: the
    // exact seam must blend them equally, with quarter weights on either side.
    let cases = [
        ([0.0, 0.5], [128, 0, 128]),
        ([1.0, 0.5], [128, 0, 128]),
        ([0.25 / 256.0, 0.5], [191, 0, 64]),
        ([255.75 / 256.0, 0.5], [64, 0, 191]),
        ([0.5 / 256.0, 0.5], [255, 0, 0]),
        ([255.5 / 256.0, 0.5], [0, 0, 255]),
        ([0.5, 0.0], [64, 128, 192]),
        ([0.5, 1.0], [64, 128, 192]),
        ([0.5, 0.5], [64, 128, 192]),
        ([0.0, 0.0], [128, 0, 128]),
        ([1.0, 1.0], [128, 0, 128]),
    ];
    let uv_list = cases
        .iter()
        .map(|(uv, _)| format!("vec2<f32>({:?}, {:?})", uv[0], uv[1]))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        "{}\n@group(0) @binding(0) var image: texture_2d<f32>;
        @group(0) @binding(1) var linear_sampler: sampler;
        @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{
            let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
            return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
        }}
        @fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
            let uv = array<vec2<f32>, {}>({uv_list});
            return vec4<f32>(sample_sky(image, linear_sampler, uv[min(u32(p.x), {}u)]), 1.0);
        }}",
        include_str!("../sky_sampling.wgsl"),
        cases.len(),
        cases.len() - 1,
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky boundary sampling"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky boundary sampling"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    for (width, height, address_mode_u) in [
        (256, 128, wgpu::AddressMode::ClampToEdge),
        (1024, 1536, wgpu::AddressMode::Repeat),
    ] {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky with hostile neighbours"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let color = if x >= 256 || y >= 128 {
                    [0, 255, 0, 255]
                } else if x == 0 {
                    [255, 0, 0, 255]
                } else if x == 255 {
                    [0, 0, 255, 255]
                } else {
                    [64, 128, 192, 255]
                };
                let i = ((y * width + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&color);
            }
        }
        queue.write_texture(
            texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
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
        });
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky boundary result"),
            size: wgpu::Extent3d {
                width: 128,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        let result = Image::read(device, queue, &output);
        let mut worst = 0;
        for (i, (uv, expected)) in cases.iter().enumerate() {
            let got = &result.rgba[i * 4..i * 4 + 3];
            for (got, expected) in got.iter().zip(expected) {
                worst = worst.max(got.abs_diff(*expected));
                assert!(
                    got.abs_diff(*expected) <= 1,
                    "{width}x{height}, uv {uv:?}: got {got}, expected {expected}"
                );
            }
        }
        eprintln!(
            "sky boundaries {width}x{height}: {} samples, maximum channel error {worst}",
            cases.len()
        );
    }
}
