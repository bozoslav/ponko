//! Instanced camera-facing particles, and the ribbons and lines drawn with
//! them (strips built on the CPU). GPU storage grows only at high-water
//! marks, and each frame uploads only the parts that changed: particles
//! that stood still, ribbons and lines that kept their shape and text that
//! stayed the same cost nothing.
use super::gpu::GpuTexture;
use crate::d3::ParticleBlend as ParticleBlend3D;
use crate::render::traffic::{MAX_WRITES, Synced, Upload};
use crate::three::particles::{ParticleFrame, ParticleInstance, ParticleRun};
use crate::three::strips::{StripRun, StripVertex};
use crate::three::text3d::TextVertex;

const SHADER: &str = include_str!("particles.wgsl");
const STRIP_SHADER: &str = include_str!("strips.wgsl");
const TEXT_SHADER: &str = include_str!("text3d.wgsl");
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view_proj: [[f32; 4]; 4],
    right: [f32; 4],
    up: [f32; 4],
}
pub(super) struct Particles {
    alpha: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
    camera: Synced,
    group: wgpu::BindGroup,
    instances: Synced,
    pub runs: Vec<ParticleRun>,
    strip_alpha: wgpu::RenderPipeline,
    strip_additive: wgpu::RenderPipeline,
    strip_vertices: Synced,
    strip_runs: Vec<StripRun>,
    text: wgpu::RenderPipeline,
    text_vertices: Synced,
    /// How many text vertices to draw this frame, and their atlas.
    text_count: u32,
    text_atlas: Option<u32>,
}
impl Particles {
    pub fn new(device: &wgpu::Device, texture_layout: &wgpu::BindGroupLayout, samples: u32) -> Self {
        let camera = Synced::new(
            device,
            "particle camera",
            wgpu::BufferUsages::UNIFORM,
            size_of::<Camera>(),
            Upload::Uniforms,
            (16, 4, MAX_WRITES),
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.buffer().as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles"),
            bind_group_layouts: &[Some(&layout), Some(texture_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let strip_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("strips"),
            source: wgpu::ShaderSource::Wgsl(STRIP_SHADER.into()),
        });
        let text_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("3d text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_SHADER.into()),
        });
        let text_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<TextVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x2
            ],
        };
        let particle_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<ParticleInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
        };
        let strip_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<StripVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4],
        };
        let pipeline = |module: &wgpu::ShaderModule, buffer: &wgpu::VertexBufferLayout, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("particles"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(buffer.clone())],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: super::gpu3d::HDR_FORMAT,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let additive = wgpu::BlendState {
            color: add,
            alpha: wgpu::BlendComponent::OVER,
        };
        let alpha = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        Self {
            alpha: pipeline(&module, &particle_layout, alpha),
            additive: pipeline(&module, &particle_layout, additive),
            camera,
            group,
            instances: vertices::<ParticleInstance>(device, "particle instances", 1),
            runs: Vec::new(),
            strip_alpha: pipeline(&strip_module, &strip_layout, alpha),
            strip_additive: pipeline(&strip_module, &strip_layout, additive),
            strip_vertices: vertices::<StripVertex>(device, "strip vertices", 6),
            strip_runs: Vec::new(),
            text: pipeline(&text_module, &text_layout, alpha),
            text_vertices: vertices::<TextVertex>(device, "3d text vertices", 6),
            text_count: 0,
            text_atlas: None,
        }
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &ParticleFrame,
        view_proj: glam::Mat4,
    ) {
        self.runs.clear();
        self.runs.extend_from_slice(&frame.runs);
        self.strip_runs.clear();
        self.strip_runs.extend_from_slice(&frame.strips.runs);
        let texts = &frame.texts.vertices;
        self.text_count = texts.len() as u32;
        self.text_atlas = frame.texts.atlas.map(|t| t.id);
        if !self.has_draws() {
            return;
        }
        // The buffers are bound when drawn, so a replaced one needs no
        // other rebinding.
        for (buffer, data) in [
            (&mut self.instances, bytemuck::cast_slice(&frame.instances)),
            (&mut self.strip_vertices, bytemuck::cast_slice(&frame.strips.vertices)),
            (&mut self.text_vertices, bytemuck::cast_slice(texts)),
        ] {
            if !data.is_empty() {
                buffer.reserve(device, data.len());
                buffer.write(queue, 0, data);
            }
        }
        // Past the most the device holds (reported), nothing is drawn.
        let fits = |buffer: &Synced, size: usize| (buffer.fits() / size) as u32;
        let instances = fits(&self.instances, size_of::<ParticleInstance>());
        clip(&mut self.runs, instances, |r| (&mut r.first, &mut r.count));
        let strips = fits(&self.strip_vertices, size_of::<StripVertex>());
        clip(&mut self.strip_runs, strips, |r| (&mut r.first, &mut r.count));
        self.text_count = self.text_count.min(fits(&self.text_vertices, size_of::<TextVertex>()));
        let camera = Camera {
            view_proj: view_proj.to_cols_array_2d(),
            right: frame.right,
            up: frame.up,
        };
        self.camera.write(queue, 0, bytemuck::bytes_of(&camera));
    }
    /// See `Renderer3D::check_mirrors`.
    #[cfg(test)]
    pub fn check_mirrors(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for buffer in [&self.camera, &self.instances, &self.strip_vertices, &self.text_vertices] {
            buffer.check(device, queue);
        }
    }

    /// Whether there are particles, ribbons, lines or text to draw.
    pub fn has_draws(&self) -> bool {
        !self.runs.is_empty() || !self.strip_runs.is_empty() || self.text_count > 0
    }

    /// How many draw calls `draw` makes.
    pub fn draw_count(&self) -> usize {
        self.runs.len() + self.strip_runs.len() + usize::from(self.text_count > 0)
    }

    /// Draws the ribbons and lines, then the particles, then the text
    /// (each set farthest first).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, textures: &[GpuTexture], white: u32) {
        if !self.has_draws() {
            return;
        }
        let texture = |id: Option<u32>| {
            textures
                .get(id.unwrap_or(white) as usize)
                .or_else(|| textures.get(white as usize))
        };
        pass.set_bind_group(0, &self.group, &[]);
        if !self.strip_runs.is_empty() {
            pass.set_vertex_buffer(0, self.strip_vertices.buffer().slice(..));
            for run in &self.strip_runs {
                let Some(texture) = texture(run.texture) else {
                    continue;
                };
                pass.set_pipeline(if run.blend == ParticleBlend3D::Alpha {
                    &self.strip_alpha
                } else {
                    &self.strip_additive
                });
                pass.set_bind_group(1, &texture.bind_group, &[]);
                pass.draw(run.first..run.first + run.count, 0..1);
            }
        }
        if !self.runs.is_empty() {
            self.draw_particles(pass, textures, white);
        }
        if self.text_count > 0
            && let Some(atlas) = self.text_atlas.and_then(|id| textures.get(id as usize))
        {
            pass.set_pipeline(&self.text);
            pass.set_bind_group(1, &atlas.bind_group, &[]);
            pass.set_vertex_buffer(0, self.text_vertices.buffer().slice(..));
            pass.draw(0..self.text_count, 0..1);
        }
    }

    fn draw_particles(&self, pass: &mut wgpu::RenderPass<'_>, textures: &[GpuTexture], white: u32) {
        pass.set_vertex_buffer(0, self.instances.buffer().slice(..));
        for run in &self.runs {
            let Some(texture) = textures
                .get(run.texture.unwrap_or(white) as usize)
                .or_else(|| textures.get(white as usize))
            else {
                continue;
            };
            pass.set_pipeline(if run.blend == ParticleBlend3D::Alpha {
                &self.alpha
            } else {
                &self.additive
            });
            pass.set_bind_group(1, &texture.bind_group, &[]);
            pass.draw(0..6, run.first..run.first + run.count);
        }
    }
}
/// Shortens `runs` (each `first` and `count` of items) to the first `fits`
/// items.
fn clip<R>(runs: &mut Vec<R>, fits: u32, range: impl Fn(&mut R) -> (&mut u32, &mut u32)) {
    runs.retain_mut(|r| {
        let (first, count) = range(r);
        *count = (*count).min(fits.saturating_sub(*first));
        *count > 0
    });
}

/// A vertex buffer for `capacity` `T`s, written one changed `T` at a time
/// (with up to four unchanged ones between two changed ones along, and in
/// at most [`MAX_WRITES`] writes a frame).
fn vertices<T>(device: &wgpu::Device, label: &'static str, capacity: usize) -> Synced {
    let size = size_of::<T>();
    let usage = wgpu::BufferUsages::VERTEX;
    Synced::new(
        device,
        label,
        usage,
        capacity * size,
        Upload::Other,
        (size, 4, MAX_WRITES),
    )
}

#[cfg(test)]
mod tests {
    use super::super::gpu::GpuTexture;
    use super::super::gpu3d::*;
    #[allow(unused_imports)]
    use crate::d3::{
        Line as Line3D, ParticleBlend as ParticleBlend3D, ParticleEmitter as ParticleEmitter3D,
        ParticleSettings as ParticleSettings3D, Transform as Transform3D,
    };
    use crate::prelude::*;
    #[allow(unused_imports)]
    use crate::render::Blend;
    #[allow(unused_imports)]
    use crate::render::{MaterialDesc, MeshData, Model};
    use crate::three::{Frame3D, GpuMaterialDesc, Scene3D, SkinFrame, SkyLight};
    const SIZE: u32 = 64;

    #[test]
    fn particles_shader_is_valid() {
        super::super::gpu3d::check_shader("particles", super::SHADER).unwrap();
        super::super::gpu3d::check_shader("strips", super::STRIP_SHADER).unwrap();
        super::super::gpu3d::check_shader("3d text", super::TEXT_SHADER).unwrap();
    }

    #[test]
    fn particles_additive_brightens_pixels_offscreen() {
        let Some(crate::render::render_test_support::TestGpu { device, queue, .. }) =
            crate::render::render_test_support::gpu()
        else {
            return;
        };
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
        let textures = [GpuTexture {
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
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target = output.create_view(&Default::default());

        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(SIZE as f32, SIZE as f32));
        ctx.scene3d.camera.position = vec3(0.0, 0.0, 4.0);
        ctx.scene3d.camera.look_at(Vec3::ZERO);
        ctx.scene3d.lighting.shadows = false;
        ctx.scene3d.sky.atmosphere = false;
        ctx.scene3d.post.bloom = 0.0;
        let mut renderer = Renderer3D::new(&device, &layout, output.format(), 0, &white, 4, true);
        let draw = |ctx: &mut Ctx, renderer: &mut Renderer3D| -> Vec<u8> {
            let mut skins = SkinFrame::default();
            skins.collect(&mut ctx.world, &ctx.gfx.assets3d);
            let mut frame = Frame3D::default();
            frame.collect(
                &mut ctx.world,
                &ctx.gfx.assets3d,
                &skins,
                &ctx.scene3d.camera,
                &ctx.scene3d.lighting,
            );
            frame.skins = skins;
            let scene = Scene3D {
                frame: &frame,
                lighting: &ctx.scene3d.lighting,
                sky: &ctx.scene3d.sky,
                light: SkyLight {
                    sun: Vec3::ZERO,
                    sky: Vec3::ZERO,
                    ground: Vec3::ZERO,
                },
                camera: ctx.scene3d.camera.position,
                post: &ctx.scene3d.post,
                time: (0.0, 1.0 / 60.0),
                occlusion: false,
            };
            renderer.prepare(
                &device,
                &queue,
                &Prepare3D {
                    scene: &scene,
                    lut: None,
                    size: (SIZE, SIZE),
                },
            );
            let buffers = renderer.draw(&device, &target, wgpu::Color::BLACK, &textures, None);
            queue.submit(buffers);
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(SIZE * SIZE * 4),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIZE * 4),
                        rows_per_image: None,
                    },
                },
                output.size(),
            );
            queue.submit([encoder.finish()]);
            readback.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            readback.get_mapped_range(..).unwrap().to_vec()
        };

        let before = draw(&mut ctx, &mut renderer);
        let center = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;

        // A glowing line across the middle lights it; the corner stays dark.
        let mut line = Line3D::new(
            vec![vec3(-2.0, 0.0, 0.0), vec3(2.0, 0.0, 0.0)],
            0.3,
            Color::rgb(1.0, 0.2, 0.0),
        );
        line.blend = ParticleBlend3D::Additive;
        let beam = ctx.world.spawn((Transform3D::default(), line));
        let lit = draw(&mut ctx, &mut renderer);
        assert!(lit[center] > before[center] + 40, "the line must brighten its pixels");
        assert_eq!(&lit[0..3], &before[0..3]);
        ctx.world.despawn(beam);
        assert_eq!(draw(&mut ctx, &mut renderer)[center], before[center]);
        let e = ctx.world.spawn((
            Transform3D::default(),
            ParticleEmitter3D::new(ParticleSettings3D {
                rate: 0.0,
                burst: 1,
                speed: (0.0, 0.0),
                gravity: Vec3::ZERO,
                size: (1.0, 1.0),
                blend: ParticleBlend3D::Additive,
                color_over_life: crate::particles::ParticleGradient::new(vec![(0.0, Color::rgba(1.0, 0.2, 0.0, 0.5))]),
                ..Default::default()
            }),
        ));
        ctx.update_particles();
        let after = draw(&mut ctx, &mut renderer);
        assert!(
            after[center] > before[center] + 40,
            "additive particles must brighten their location"
        );
        assert_eq!(
            &after[0..3],
            &before[0..3],
            "background outside particles stays unchanged"
        );

        // A second overlapping particle contributes light: particles do not write depth.
        ctx.world.get_mut::<ParticleEmitter3D>(e).unwrap().burst(1);
        ctx.update_particles();
        let twice = draw(&mut ctx, &mut renderer);
        assert!(twice[center] > after[center] + 10);

        // Removing the last emitter clears cached GPU draws; a later emitter can reuse storage.
        let saved = ctx.world.get::<ParticleEmitter3D>(e).unwrap().clone();
        ctx.world.despawn(e);
        assert_eq!(draw(&mut ctx, &mut renderer)[center], before[center]);
        assert!(!renderer.has_draws());
        let e = ctx.world.spawn((Transform3D::default(), saved));

        // A scene-reading transparent model exercises the separate transparent pass.
        let source = "fn surface(s: Surface) -> Surface { var o = s; o.emissive = scene_color(s.screen); o.alpha = 0.0; return o; }";
        let shader = ctx.gfx.create_shader("particle test glass", source);
        renderer.create_shader(&device, "particle test glass", source).unwrap();
        let desc = MaterialDesc {
            shader: Some(shader),
            blend: Blend::Transparent,
            ..Default::default()
        };
        let mat = ctx.gfx.create_material(desc);
        renderer.create_material(&device, &GpuMaterialDesc::new(&desc), &textures);
        let cube = MeshData::cube(0.5);
        let mesh = ctx.gfx.create_mesh(&cube, mat);
        renderer.create_mesh(&device, &cube, &[(0..cube.indices.len() as u32, mat)], false);
        ctx.world
            .spawn((Transform3D::at(vec3(1.2, 0.0, 0.0)), Model::new(mesh)));
        assert_eq!(draw(&mut ctx, &mut renderer)[center], twice[center]);

        // Moving the emitter does not move existing world-space particles.
        ctx.world.get_mut::<Transform3D>(e).unwrap().position = vec3(0.0, 0.0, 6.0);
        // World-space particles stay at the original position.
        assert_eq!(draw(&mut ctx, &mut renderer)[center], twice[center]);

        // Opaque geometry in front occludes the particles in the split pass too.
        let desc = MaterialDesc {
            color: Color::BLACK,
            ..Default::default()
        };
        let mat = ctx.gfx.create_material(desc);
        renderer.create_material(&device, &GpuMaterialDesc::new(&desc), &textures);
        let cube = MeshData::cube(1.0);
        let mesh = ctx.gfx.create_mesh(&cube, mat);
        renderer.create_mesh(&device, &cube, &[(0..cube.indices.len() as u32, mat)], false);
        ctx.world
            .spawn((Transform3D::at(vec3(0.0, 0.0, 1.0)), Model::new(mesh)));
        assert!(draw(&mut ctx, &mut renderer)[center] < twice[center] - 40);
    }
}
