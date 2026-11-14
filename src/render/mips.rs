//! Mipmaps for 3D material textures, made on the GPU: each level is the
//! one above it, halved with a bilinear sample in the middle of every 2x2
//! block. Color maps are averaged as light (sRGB decoded), data maps
//! (normals, ORM) as stored.

pub(super) const SHADER: &str = r"
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.clip = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_filter: sampler;

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return textureSampleLevel(source, linear_filter, in.uv, 0.0);
}

// The bytes an sRGB view decoded, back: the first level of a data texture
// made from an sRGB one on a device that can't view it as linear.
fn encode(c: f32) -> f32 {
    return select(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, c <= 0.0031308);
}

@fragment
fn fs_encode(in: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(source, linear_filter, in.uv, 0.0);
    return vec4<f32>(encode(c.r), encode(c.g), encode(c.b), c.a);
}
";

/// The levels of a texture array, one layer at a time, where a layer can't
/// be sampled through a 2D view of it (the GL backend: see
/// [`MipMaker::generate`]): the whole array is bound and the layer is picked
/// here. The same sample as [`SHADER`]'s `fs`.
pub(super) const ARRAY_SHADER: &str = r"
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.clip = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var source: texture_2d_array<f32>;
@group(0) @binding(1) var linear_filter: sampler;
@group(0) @binding(2) var<uniform> layer: vec4<u32>;

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return textureSampleLevel(source, linear_filter, in.uv, layer.x, 0.0);
}
";

/// How many mip levels a `width` x `height` texture has, down to 1x1.
pub(super) fn levels(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

pub(super) struct MipMaker {
    layout: wgpu::BindGroupLayout,
    /// For sRGB (color) and linear (data) views.
    srgb: wgpu::RenderPipeline,
    linear: wgpu::RenderPipeline,
    /// Writes a linear (`Rgba8Unorm`) level the bytes of an sRGB one, see `data_texture`.
    encode: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// On the GL backend: the levels of texture arrays (see `generate`).
    arrays: Option<ArrayMips>,
}

/// What makes the levels of texture arrays on the GL backend.
struct ArrayMips {
    layout: wgpu::BindGroupLayout,
    srgb: wgpu::RenderPipeline,
    linear: wgpu::RenderPipeline,
}

impl ArrayMips {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("array mipmaps"),
            source: wgpu::ShaderSource::Wgsl(ARRAY_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("array mipmaps"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("array mipmaps"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("array mipmaps"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
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
        Self {
            srgb: pipeline(wgpu::TextureFormat::Rgba8UnormSrgb),
            linear: pipeline(wgpu::TextureFormat::Rgba8Unorm),
            layout,
        }
    }
}

/// Whether `device` can't sample one layer of a texture array through a 2D
/// view: wgpu's GL backend (WebGL2) has no texture views, and binds the
/// whole array, which a shader reading a 2D texture can't read.
fn needs_array_views(device: &wgpu::Device) -> bool {
    #[cfg(test)]
    if FORCE_ARRAY_MIPS.get() {
        return true;
    }
    device.adapter_info().backend == wgpu::Backend::Gl
}

#[cfg(test)]
thread_local! {
    /// Tests: mip makers made on this thread take the GL backend's path for
    /// texture arrays on any backend.
    static FORCE_ARRAY_MIPS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl MipMaker {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mipmaps"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mipmaps"),
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
            label: Some("mipmaps"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |format: wgpu::TextureFormat, entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mipmaps"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
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
        Self {
            srgb: pipeline(wgpu::TextureFormat::Rgba8UnormSrgb, "fs"),
            linear: pipeline(wgpu::TextureFormat::Rgba8Unorm, "fs"),
            encode: pipeline(wgpu::TextureFormat::Rgba8Unorm, "fs_encode"),
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mipmaps"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            arrays: needs_array_views(device).then(|| ArrayMips::new(device)),
        }
    }

    /// A data texture (normals, ORM) with `levels` mip levels from the
    /// single-level sRGB texture `source`, on a device that can't read an
    /// sRGB texture through a linear view (WebGL2): the texture is made
    /// `Rgba8Unorm` with the same bytes, so it is sampled and averaged as
    /// stored, not decoded as light. (The bytes are written by a draw that
    /// undoes the sRGB decoding, not copied: a copy between the two formats
    /// isn't the same bytes on every API.)
    pub fn data_texture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        levels: u32,
    ) -> wgpu::Texture {
        let size = source.size();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material data texture"),
            size,
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mipmaps"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.create_view(&wgpu::TextureViewDescriptor {
                        mip_level_count: Some(1),
                        ..Default::default()
                    })),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let target = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("data texture level 0"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("data texture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.encode);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.generate(device, encoder, &texture, true);
        texture
    }

    /// Fills every level of `texture` below the first from the one above,
    /// in each of its layers.
    pub fn generate(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        linear: bool,
    ) {
        let format = if linear {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };
        let view = |level, layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("mip level"),
                format: Some(format),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: level,
                mip_level_count: Some(1),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        let layers = texture.depth_or_array_layers();
        if layers > 1
            && let Some(arrays) = &self.arrays
        {
            self.generate_layers(arrays, device, encoder, texture, linear, view);
            return;
        }
        for (layer, level) in (0..layers).flat_map(|l| (1..texture.mip_level_count()).map(move |m| (l, m))) {
            let source = view(level - 1, layer);
            let target = view(level, layer);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mipmaps"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mipmaps"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(if linear { &self.linear } else { &self.srgb });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// [`generate`](Self::generate) for a texture array on the GL backend:
    /// each level of each layer is drawn (into a 2D view of that layer and
    /// level, as elsewhere) from the level above, read through a view of the
    /// whole array with the layer as a uniform. `target(level, layer)` is
    /// that 2D view.
    fn generate_layers(
        &self,
        arrays: &ArrayMips,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        linear: bool,
        target: impl Fn(u32, u32) -> wgpu::TextureView,
    ) {
        let format = if linear {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };
        let layers = texture.depth_or_array_layers();
        // Each layer's index, a uniform offset alignment apart.
        let stride = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(16);
        let mut indices = vec![0u8; (stride * u64::from(layers)) as usize];
        for layer in 0..layers {
            let at = (stride * u64::from(layer)) as usize;
            indices[at..at + 4].copy_from_slice(&layer.to_le_bytes());
        }
        let buffer = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("array mipmap layers"),
                contents: &indices,
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        for level in 1..texture.mip_level_count() {
            let source = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("mip level, every layer"),
                format: Some(format),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_mip_level: level - 1,
                mip_level_count: Some(1),
                ..Default::default()
            });
            for layer in 0..layers {
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("array mipmaps"),
                    layout: &arrays.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &buffer,
                                offset: stride * u64::from(layer),
                                size: wgpu::BufferSize::new(16),
                            }),
                        },
                    ],
                });
                let view = target(level, layer);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("array mipmaps"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(if linear { &arrays.linear } else { &arrays.srgb });
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An offscreen device, or `None` without a GPU.
    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        let gpu = crate::render::render_test_support::gpu()?;
        Some((gpu.device, gpu.queue))
    }

    /// Mip level `level` of an RGBA8 `texture`, rows unpadded.
    fn read_level(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, level: u32) -> Vec<u8> {
        read_layer(device, queue, texture, level, 0)
    }

    /// Mip level `level` of layer `layer` of an RGBA8 `texture`, rows unpadded.
    fn read_layer(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        level: u32,
        layer: u32,
    ) -> Vec<u8> {
        let (width, height) = ((texture.width() >> level).max(1), (texture.height() >> level).max(1));
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level,
                origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
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
        buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mapped = buffer.get_mapped_range(..).unwrap();
        mapped
            .chunks(row as usize)
            .take(height as usize)
            .flat_map(|r| r[..(width * 4) as usize].iter().copied())
            .collect()
    }

    /// The device without view formats: an sRGB texture holding data (a
    /// normal map's flat blue and a hard black and white edge) becomes a
    /// linear one with the same bytes, and its mip levels average the stored
    /// values, as on a device that can view it as linear.
    #[test]
    fn data_textures_keep_their_bytes_without_view_formats() {
        let Some((device, queue)) = gpu() else {
            return;
        };
        // 4x4: red alternates 0 and 200 by column; green and blue are a normal map's flat (128, 255).
        let pixels: Vec<u8> = (0..16)
            .flat_map(|i| [if i % 2 == 0 { 0 } else { 200 }, 128, 255, 255])
            .collect();
        let source = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            source.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(16),
                rows_per_image: Some(4),
            },
            source.size(),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        let texture = MipMaker::new(&device).data_texture(&device, &mut encoder, &source, levels(4, 4));
        queue.submit([encoder.finish()]);

        assert_eq!(
            texture.format(),
            wgpu::TextureFormat::Rgba8Unorm,
            "read without decoding"
        );
        let base = read_level(&device, &queue, &texture, 0);
        for (i, (got, want)) in base.iter().zip(&pixels).enumerate() {
            assert!(got.abs_diff(*want) <= 1, "byte {i}: {got}, stored {want}");
        }
        // Averaged as stored: (0 + 200) / 2 = 100. As light it would be about 146.
        let level1 = read_level(&device, &queue, &texture, 1);
        for texel in level1.chunks(4) {
            assert!(texel[0].abs_diff(100) <= 2, "red {texel:?}");
            assert!(texel[1].abs_diff(128) <= 1 && texel[2].abs_diff(255) <= 1, "{texel:?}");
        }
    }

    /// The GL backend's path for texture arrays (the whole array bound, the
    /// layer picked by a uniform; WebGL2 has no texture views) makes the
    /// levels the 2D views of single layers do, layer by layer.
    #[test]
    fn array_levels_match_without_texture_views() {
        let Some((device, queue)) = gpu() else {
            return;
        };
        let (size, layers) = (8u32, 3u32);
        for linear in [false, true] {
            let format = if linear {
                wgpu::TextureFormat::Rgba8Unorm
            } else {
                wgpu::TextureFormat::Rgba8UnormSrgb
            };
            let array = || {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: layers,
                    },
                    mip_level_count: levels(size, size),
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                // Each layer its own pattern: stripes of a layer's colour.
                for layer in 0..layers {
                    let pixels: Vec<u8> = (0..size * size)
                        .flat_map(|i| {
                            let on = (i % size + layer) % 2 == 0;
                            [
                                if on { 255 } else { 0 },
                                (layer * 100) as u8,
                                if on { 0 } else { 200 },
                                255,
                            ]
                        })
                        .collect();
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &pixels,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(size * 4),
                            rows_per_image: Some(size),
                        },
                        wgpu::Extent3d {
                            width: size,
                            height: size,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                texture
            };
            let views = MipMaker::new(&device);
            FORCE_ARRAY_MIPS.set(true);
            let whole = MipMaker::new(&device);
            FORCE_ARRAY_MIPS.set(false);
            assert!(views.arrays.is_none() && whole.arrays.is_some());
            let (a, b) = (array(), array());
            let mut encoder = device.create_command_encoder(&Default::default());
            views.generate(&device, &mut encoder, &a, linear);
            whole.generate(&device, &mut encoder, &b, linear);
            queue.submit([encoder.finish()]);
            for level in 1..levels(size, size) {
                let mut seen = Vec::new();
                for layer in 0..layers {
                    let (want, got) = (
                        read_layer(&device, &queue, &a, level, layer),
                        read_layer(&device, &queue, &b, level, layer),
                    );
                    for (i, (w, g)) in want.iter().zip(&got).enumerate() {
                        assert!(
                            w.abs_diff(*g) <= 1,
                            "linear {linear}, level {level}, layer {layer}, byte {i}: {g}, want {w}"
                        );
                    }
                    seen.push(got);
                }
                // Each layer's level came from that layer.
                assert!(seen[0] != seen[1] && seen[1] != seen[2], "level {level}: layers differ");
            }
        }
    }

    #[test]
    fn levels_go_down_to_one_texel() {
        assert_eq!(levels(1, 1), 1);
        assert_eq!(levels(2, 2), 2);
        assert_eq!(levels(256, 256), 9);
        assert_eq!(levels(300, 7), 9);
        assert_eq!(levels(1024, 2048), 12);
    }
}
