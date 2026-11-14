//! The scene depth copy of the GL backend (WebGL2 and native OpenGL ES).
//!
//! Transparent surfaces that read the scene behind them (`scene_position`,
//! `scene_distance`) sample the scene depth while the transparent pass tests
//! against it. Elsewhere the pass attaches the depth read-only and samples
//! it at the same time; the GL backend has no read-only depth attachments
//! (wgpu's `DownlevelFlags::READ_ONLY_DEPTH_STENCIL`), so the attachment
//! counts as written and sampling it is rejected. There the transparent
//! pass samples a copy instead, made just before it.
//!
//! The copy is a full-screen pass that writes each pixel's depth with
//! `frag_depth`: wgpu's GL backend copies textures through a color
//! attachment, which a depth texture can't be, and a depth target (unlike
//! a 32-bit float color target) needs no WebGL2 extension. It reads the
//! depth as the scene shaders do on GL (see `shaders::depth_as_float`):
//! sampled through a non-filtering sampler with one sample, loaded when
//! multisampled. It costs one full-screen depth write, only in
//! frames with scene-reading transparent surfaces, only on GL.

use super::DEPTH_FORMAT;

const SAMPLED: &str = "
@group(0) @binding(0) var depth: texture_2d<f32>;
@group(0) @binding(1) var nearest: sampler;

@fragment
fn fs_copy(@builtin(position) at: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureSampleLevel(depth, nearest, at.xy / vec2<f32>(textureDimensions(depth)), 0.0).x;
}
";

const MULTISAMPLED: &str = "
@group(0) @binding(0) var depth: texture_multisampled_2d<f32>;
";

const MULTISAMPLED_COPY: &str = "
@fragment
fn fs_copy(@builtin(position) at: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(depth, vec2<i32>(at.xy), 0).x;
}
";

const VERTEX: &str = "
@vertex
fn vs_copy(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}
";

/// The copy shader for a depth of `samples` per pixel, read as a float
/// texture: sampled at each pixel's center with one sample, or loaded from
/// sample 0 of a multisampled depth into every sample of the copy. Shaders
/// read only sample 0 of the scene depth (the book's shader section says
/// so), and the transparent pass tests against the depth itself, not the
/// copy, so per-sample shading (which not every GL device has) isn't needed.
pub(super) fn shader(samples: u32) -> String {
    if samples > 1 {
        format!("{MULTISAMPLED}{VERTEX}{MULTISAMPLED_COPY}")
    } else {
        format!("{SAMPLED}{VERTEX}")
    }
}

/// The copy pipeline, made once per sample count.
pub(super) struct DepthCopy {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    /// With one sample: the non-filtering sampler the depth is read through.
    sampler: Option<wgpu::Sampler>,
    samples: u32,
}

impl DepthCopy {
    pub(super) fn new(device: &wgpu::Device, samples: u32) -> Self {
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: samples > 1,
            },
            count: None,
        }];
        let sampler = (samples <= 1).then(|| {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            });
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("3d depth copy"),
                ..Default::default()
            })
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d depth copy"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d depth copy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("3d depth copy"),
            source: wgpu::ShaderSource::Wgsl(shader(samples).into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3d depth copy"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_copy"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_copy"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            layout,
            pipeline,
            sampler,
            samples,
        }
    }

    /// The copy of `depth` (the scene depth, of `size`), and what makes it.
    pub(super) fn target(&self, device: &wgpu::Device, depth: &wgpu::TextureView, size: (u32, u32)) -> DepthCopyTarget {
        let view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("3d depth copy"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: self.samples,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(depth),
        }];
        if let Some(sampler) = &self.sampler {
            entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let source = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d depth copy"),
            layout: &self.layout,
            entries: &entries,
        });
        DepthCopyTarget {
            view,
            source,
            pipeline: self.pipeline.clone(),
        }
    }
}

/// A scene-sized depth copy: its texture, and the bind group reading the
/// scene depth it copies.
pub(super) struct DepthCopyTarget {
    /// The copy, which the transparent pass's scene bind group reads.
    pub(super) view: wgpu::TextureView,
    source: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl DepthCopyTarget {
    /// Records the copy.
    pub(super) fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3d depth copy"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.source, &[]);
        pass.draw(0..3, 0..1);
    }
}
