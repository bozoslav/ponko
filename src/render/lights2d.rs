//! 2D lighting on the GPU (see `lights2d.wgsl` and `crate::light2d`): a
//! normal pass when a drawn texture has a normal map, the light map with
//! stencil shadow volumes, and the composite onto the frame. Made the first
//! time a frame has lighting on, so unlit games never build any of it.

use glam::Mat4;

use super::batch::{Batch, SpriteInstance};
use super::gpu::GpuTexture;
use super::traffic::{self, Counted, NewBuffer, Upload};
use crate::light2d::{LightFrame2D, LightInstance};
use crate::sprite_material::{BlendMode, split_style};
use crate::texture::Texture;

const SOURCE: &str = include_str!("lights2d.wgsl");
/// The light map: half the light, linear. A float format, as the 3D frame
/// uses on every tier (WebGL2 included), so dim light doesn't band.
pub(super) const LIGHT_MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The world's normals, 0..1 encoded, and whether each pixel has one.
const NORMALS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Stencil for the shadow volumes (its depth is unused; this format exists
/// on every backend).
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
/// Shadowed lights between two stencil clears: each takes the next value.
const STENCIL_VALUES: u32 = 255;

/// One of the three shader modules: the shared part and part `n`.
fn module(device: &wgpu::Device, n: usize) -> wgpu::ShaderModule {
    let marks = ["// ---- 1.", "// ---- 2.", "// ---- 3."];
    let start = SOURCE.find(marks[n - 1]).unwrap_or(0);
    let end = marks.get(n).and_then(|m| SOURCE.find(m)).unwrap_or(SOURCE.len());
    let shared = &SOURCE[..SOURCE.find(marks[0]).unwrap_or(0)];
    let source = format!("{shared}{}", &SOURCE[start..end]);
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("2d lights"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// The window-sized textures, made again when the window's size changes.
struct Targets {
    size: [u32; 2],
    light_map: wgpu::TextureView,
    stencil: wgpu::TextureView,
    /// The light map, for the composite.
    composite: wgpu::BindGroup,
    /// The normals target and the light pass's bind group reading it, made
    /// the first time a frame needs normals.
    normals: Option<(wgpu::TextureView, wgpu::BindGroup)>,
}

pub(crate) struct Lights2D {
    view_buffer: wgpu::Buffer,
    view_group: wgpu::BindGroup,
    sample_layout: wgpu::BindGroupLayout,
    /// The light pass's normals when no pass wrote any: flat, none mapped.
    flat_normals: wgpu::BindGroup,
    /// What sprites without a normal map draw into the normal pass.
    no_normal_map: wgpu::BindGroup,
    normal_pipelines: [wgpu::RenderPipeline; 2],
    shadow_pipeline: wgpu::RenderPipeline,
    free_pipeline: wgpu::RenderPipeline,
    shadowed_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    lights: wgpu::Buffer,
    light_capacity: usize,
    vertices: wgpu::Buffer,
    vertex_capacity: usize,
    targets: Option<Targets>,
    /// This frame needs the normal pass.
    normal_pass: bool,
    /// Lights and shadow vertices this frame draws (what fits).
    drawn: (u32, u32),
    /// Reported once that the lights or shadows didn't fit.
    too_big: bool,
}

fn tiny(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, rgba: [u8; 4]) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("2d lights stand-in"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: None,
        },
        texture.size(),
    );
    texture
}

impl Lights2D {
    /// `camera_layout` and `texture_layout`: the sprite pass's (the normal
    /// pass draws the same sprites); `format`: the frame's.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("2d lights view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let sample_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("2d lights texture"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let view_buffer = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("2d lights view"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("2d lights view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        let flat = tiny(device, queue, NORMALS_FORMAT, [128, 128, 255, 0]);
        let flat_normals = sample_group(device, &sample_layout, &flat.create_view(&Default::default()));
        let none = tiny(device, queue, wgpu::TextureFormat::Rgba8Unorm, [128, 128, 255, 0]);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let no_normal_map = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("2d lights no normal map"),
            layout: texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&none.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let normals = module(device, 1);
        let normal_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("2d normals"),
            bind_group_layouts: &[Some(camera_layout), Some(texture_layout), Some(texture_layout)],
            immediate_size: 0,
        });
        let normal_pipeline = |entry: &str| {
            let attributes = wgpu::vertex_attr_array![
                // As `gpu::sprite_pipeline_with`.
                0 => Float32x2, 1 => Float32x2, 2 => Float16x2, 3 => Float32, 4 => Unorm8x4, 5 => Unorm16x4,
            ];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("2d normals"),
                layout: Some(&normal_layout),
                vertex: wgpu::VertexState {
                    module: &normals,
                    entry_point: Some("vs_normal"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<SpriteInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &normals,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(NORMALS_FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let normal_pipelines = [normal_pipeline("fs_normal_srgb"), normal_pipeline("fs_normal_linear")];

        let lights = module(device, 2);
        let light_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("2d lights"),
            bind_group_layouts: &[Some(&view_layout), Some(&sample_layout)],
            immediate_size: 0,
        });
        let stencil = |compare, pass_op, write_mask| {
            let face = wgpu::StencilFaceState {
                compare,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op,
            };
            Some(wgpu::DepthStencilState {
                format: STENCIL_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState {
                    front: face,
                    back: face,
                    read_mask: 0xff,
                    write_mask,
                },
                bias: Default::default(),
            })
        };
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let light_pipeline = |label, depth_stencil| {
            let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&light_layout),
                vertex: wgpu::VertexState {
                    module: &lights,
                    entry_point: Some("vs_light"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<LightInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &lights,
                    entry_point: Some("fs_light"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: LIGHT_MAP_FORMAT,
                        blend: Some(add),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let free_pipeline = light_pipeline(
            "2d lights",
            stencil(wgpu::CompareFunction::Always, wgpu::StencilOperation::Keep, 0),
        );
        let shadowed_pipeline = light_pipeline(
            "2d shadowed lights",
            stencil(wgpu::CompareFunction::NotEqual, wgpu::StencilOperation::Keep, 0),
        );
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("2d shadow volumes"),
            layout: Some(&light_layout),
            vertex: wgpu::VertexState {
                module: &lights,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: stencil(wgpu::CompareFunction::Always, wgpu::StencilOperation::Replace, 0xff),
            multisample: wgpu::MultisampleState::default(),
            // Stencil only: a fragment stage with no color written (WebGL2
            // wants one for any draw).
            fragment: Some(wgpu::FragmentState {
                module: &lights,
                entry_point: Some("fs_shadow"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: LIGHT_MAP_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let composite = module(device, 3);
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("2d lights composite"),
            bind_group_layouts: &[Some(&sample_layout)],
            immediate_size: 0,
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("2d lights composite"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // frame × light map × 2 (the map holds half): src·dst + dst·src.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Dst,
                            dst_factor: wgpu::BlendFactor::Src,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let light_capacity = 64;
        let vertex_capacity = 1024;
        Self {
            lights: light_buffer(device, light_capacity),
            light_capacity,
            vertices: vertex_buffer(device, vertex_capacity),
            vertex_capacity,
            view_buffer,
            view_group,
            sample_layout,
            flat_normals,
            no_normal_map,
            normal_pipelines,
            shadow_pipeline,
            free_pipeline,
            shadowed_pipeline,
            composite_pipeline,
            targets: None,
            normal_pass: false,
            drawn: (0, 0),
            too_big: false,
        }
    }

    /// Uploads `frame`'s lights and shadows for a frame of `size` pixels
    /// seen through `view_proj`, and finds whether `batches` (the world's)
    /// need the normal pass.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &LightFrame2D,
        view_proj: Mat4,
        size: [u32; 2],
        normal_maps: &[Option<Texture>],
        textures: &[GpuTexture],
        batches: &[Batch],
    ) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(self.make_targets(device, size));
        }
        queue.uploading(Upload::Uniforms).write_buffer(
            &self.view_buffer,
            0,
            bytemuck::cast_slice(&view_proj.to_cols_array()),
        );
        // Lights and shadow vertices: as many as the device's buffers hold.
        let light_size = size_of::<LightInstance>();
        let most = traffic::most_bytes(device, wgpu::BufferUsages::VERTEX);
        if traffic::outgrown(&mut self.light_capacity, frame.lights.len(), most / light_size) {
            self.lights = light_buffer(device, self.light_capacity);
        }
        if traffic::outgrown(&mut self.vertex_capacity, frame.vertices.len(), most / 8) {
            self.vertices = vertex_buffer(device, self.vertex_capacity);
        }
        let fits = frame.lights.len() <= self.light_capacity && frame.vertices.len() <= self.vertex_capacity;
        if !fits && !self.too_big {
            let needed = frame.lights.len() * light_size + frame.vertices.len() * 8;
            traffic::too_big("2D lights and shadows", needed, most, "no 2D lights are drawn");
        }
        self.too_big = !fits;
        self.drawn = if fits {
            (frame.lights.len() as u32, frame.vertices.len() as u32)
        } else {
            (0, 0)
        };
        if fits {
            let uploads = queue.uploading(Upload::Lights);
            if !frame.lights.is_empty() {
                uploads.write_buffer(&self.lights, 0, bytemuck::cast_slice(&frame.lights));
            }
            if !frame.vertices.is_empty() {
                uploads.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&frame.vertices));
            }
        }
        self.normal_pass = batches
            .iter()
            .any(|b| normals_of(b, normal_maps, textures).is_some_and(|n| n.is_some()));
        if self.normal_pass
            && let Some(targets) = &mut self.targets
            && targets.normals.is_none()
        {
            let view = target(device, size, NORMALS_FORMAT, "2d normals").create_view(&Default::default());
            let group = sample_group(device, &self.sample_layout, &view);
            targets.normals = Some((view, group));
        }
    }

    fn make_targets(&self, device: &wgpu::Device, size: [u32; 2]) -> Targets {
        let light_map = target(device, size, LIGHT_MAP_FORMAT, "2d light map").create_view(&Default::default());
        let stencil = target(device, size, STENCIL_FORMAT, "2d light stencil").create_view(&Default::default());
        let composite = sample_group(device, &self.sample_layout, &light_map);
        Targets {
            size,
            light_map,
            stencil,
            composite,
            normals: None,
        }
    }

    /// Records the normal pass (if [`prepare`](Self::prepare) found it
    /// needed), the light map and the composite onto `frame_view`, which
    /// holds the world's sprites. `camera`, `instances`, `batches` and
    /// `base`: the world layer as the sprite pass drew it; `scale` and
    /// `viewport` place clip rectangles.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame_view: &wgpu::TextureView,
        frame: &LightFrame2D,
        world: (&wgpu::BindGroup, &[Batch], u32),
        instances: &wgpu::Buffer,
        textures: &[GpuTexture],
        normal_maps: &[Option<Texture>],
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let Some(targets) = &self.targets else {
            return;
        };
        let normals = match (&targets.normals, self.normal_pass) {
            (Some((view, group)), true) => {
                self.draw_normals(encoder, view, world, instances, textures, normal_maps);
                group
            }
            _ => &self.flat_normals,
        };
        let ambient = frame.ambient;
        let clear = wgpu::Color {
            r: f64::from(ambient[0]),
            g: f64::from(ambient[1]),
            b: f64::from(ambient[2]),
            a: 1.0,
        };
        let mut timestamps = timestamps;
        // The encoder stays locked while a pass is open (`forget_lifetime`),
        // which lets one pass replace the last.
        let begin = |encoder: &mut wgpu::CommandEncoder, load, timestamps| -> wgpu::RenderPass<'static> {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("2d light map"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &targets.light_map,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &targets.stencil,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0),
                            store: wgpu::StoreOp::Discard,
                        }),
                    }),
                    timestamp_writes: timestamps,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            pass.set_bind_group(0, &self.view_group, &[]);
            pass.set_bind_group(1, normals, &[]);
            pass
        };
        {
            let (lights, _) = self.drawn;
            let free = frame.free.min(lights);
            let mut pass = begin(encoder, wgpu::LoadOp::Clear(clear), timestamps.take());
            if free > 0 {
                pass.set_pipeline(&self.free_pipeline);
                pass.set_vertex_buffer(0, self.lights.slice(..));
                pass.draw(0..6, 0..free);
            }
            let mut used = 0;
            for (i, range) in frame.shadows.iter().enumerate() {
                let light = free + i as u32;
                if light >= lights {
                    break;
                }
                if used == STENCIL_VALUES {
                    // Out of stencil values: clear it in a new pass.
                    drop(pass);
                    pass = begin(encoder, wgpu::LoadOp::Load, None);
                    used = 0;
                }
                used += 1;
                pass.set_stencil_reference(used);
                pass.set_pipeline(&self.shadow_pipeline);
                pass.set_vertex_buffer(0, self.vertices.slice(..));
                pass.draw(range.clone(), 0..1);
                pass.set_pipeline(&self.shadowed_pipeline);
                pass.set_vertex_buffer(0, self.lights.slice(..));
                pass.draw(0..6, light..light + 1);
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("2d lights composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &targets.composite, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The world's sprites again, writing their normals.
    fn draw_normals(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        (camera, batches, base): (&wgpu::BindGroup, &[Batch], u32),
        instances: &wgpu::Buffer,
        textures: &[GpuTexture],
        normal_maps: &[Option<Texture>],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("2d normals"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Flat, no normal map.
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.5,
                        g: 0.5,
                        b: 1.0,
                        a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, camera, &[]);
        pass.set_vertex_buffer(0, instances.slice(..));
        let mut current = None;
        for batch in batches {
            let Some(normals) = normals_of(batch, normal_maps, textures) else {
                continue;
            };
            let Some(texture) = textures.get(batch.texture as usize) else {
                continue;
            };
            let (pipeline, group) = match normals {
                Some(n) => (usize::from(!n.texture.format().is_srgb()), &n.bind_group),
                None => (1, &self.no_normal_map),
            };
            if current != Some(pipeline) {
                pass.set_pipeline(&self.normal_pipelines[pipeline]);
                current = Some(pipeline);
            }
            pass.set_bind_group(1, &texture.bind_group, &[]);
            pass.set_bind_group(2, group, &[]);
            pass.draw(0..6, base + batch.start..base + batch.end);
        }
    }
}

/// What `batch` writes into the normal pass: nothing (`None`: glows and
/// darkening drawn additive or multiplied, clipped draws), flat (`Some(None)`),
/// or its texture's normal map.
fn normals_of<'a>(
    batch: &Batch,
    normal_maps: &[Option<Texture>],
    textures: &'a [GpuTexture],
) -> Option<Option<&'a GpuTexture>> {
    let (blend, _) = split_style(batch.material);
    if matches!(blend, BlendMode::Additive | BlendMode::Multiply) || batch.clip.is_some() {
        return None;
    }
    let map = normal_maps.get(batch.texture as usize).copied().flatten();
    Some(map.and_then(|m| textures.get(m.id as usize)))
}

fn sample_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("2d lights texture"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(view),
        }],
    })
}

fn target(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat, label: &str) -> wgpu::Texture {
    let usage = if format.has_stencil_aspect() {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    } else {
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
    };
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn light_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.new_buffer(&wgpu::BufferDescriptor {
        label: Some("2d lights"),
        size: (capacity.max(1) * size_of::<LightInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn vertex_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.new_buffer(&wgpu::BufferDescriptor {
        label: Some("2d shadow volumes"),
        size: (capacity.max(1) * 8) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
