//! An offscreen 2D lighting benchmark for `ponko_bench` (`lights2d/...`):
//! a screen of sprites drawn through the window's sprite pass, lit by moving
//! lights with or without shadows and normal maps, or unlit (the baseline).
//! Each frame gathers the lights from a world, uploads and records the
//! passes, then waits for the GPU. Unstable (`bench-internals`).

use std::time::{Duration, Instant};

use glam::Vec2;
use ponko_ecs::World;

use super::batch::{Batch, SpriteInstance};
use super::gpu::{GpuTexture, Lighting, Renderer, SpritePass, sprite_pipeline};
use super::lights2d::Lights2D;
use super::sprite_pipelines::SpritePipelines;
use crate::camera::Rect;
use crate::color::Color;
use crate::ecs::Transform2D;
use crate::light2d::{LightFrame2D, Occluder2D, PointLight2D};
use crate::texture::Texture;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// What a [`Lights2DBench`] draws.
#[derive(Debug, Clone, Copy)]
pub struct Lights2DScene {
    /// The frame's size in pixels.
    pub size: (u32, u32),
    /// Sprites (32 x 32, a grid over the screen).
    pub sprites: usize,
    /// Point lights wandering over the screen; 0 with `lit` is the ambient
    /// light alone.
    pub lights: usize,
    /// The lights cast shadows.
    pub shadows: bool,
    /// Square occluders scattered over the screen.
    pub occluders: usize,
    /// The sprites' texture has a normal map (adds the normal pass).
    pub normal_maps: bool,
    /// Lighting on at all; off is the unlit baseline.
    pub lit: bool,
}

/// One frame's cost.
#[derive(Debug, Clone, Copy)]
pub struct Lights2DFrame {
    /// Gathering the lights and building the shadow volumes.
    pub collect: Duration,
    /// Everything on the CPU: gathering, uploads, recording, submitting.
    pub cpu: Duration,
    /// The whole frame, until the GPU finished it.
    pub total: Duration,
    /// Shadow volume vertices this frame.
    pub shadow_vertices: usize,
}

/// See the module docs.
pub struct Lights2DBench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
    scene: Lights2DScene,
    pipeline: wgpu::RenderPipeline,
    looks: SpritePipelines,
    lights: Lights2D,
    textures: Vec<GpuTexture>,
    camera: wgpu::BindGroup,
    instances: wgpu::Buffer,
    sprites: usize,
    world: World,
    frame: LightFrame2D,
    normal_maps: Vec<Option<Texture>>,
    output: wgpu::TextureView,
    tick: u32,
}

fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    size: u32,
    texel: impl Fn(u32, u32) -> [u8; 4],
) -> GpuTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let texels: Vec<u8> = (0..size * size).flat_map(|i| texel(i % size, i / size)).collect();
    queue.write_texture(
        texture.as_image_copy(),
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * size),
            rows_per_image: None,
        },
        texture.size(),
    );
    let view = texture.create_view(&Default::default());
    let sampler = device.create_sampler(&Default::default());
    GpuTexture {
        bind_group: device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
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
        texture,
        nearest: false,
        #[cfg(feature = "3d")]
        mips: None,
    }
}

impl Lights2DBench {
    /// Opens the GPU as a game does (the high-performance adapter, no
    /// window, `PONKO_GPU_TIER` honored) and builds `scene`. `Err` says why
    /// there is no GPU.
    pub fn new(scene: Lights2DScene) -> Result<Self, String> {
        use wgpu::util::DeviceExt;
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.flags = wgpu::InstanceFlags::from_env_or_default();
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter ({e})"))?;
        let info = adapter.get_info();
        let mut descriptor = wgpu::DeviceDescriptor::default();
        if let Some(tier) = super::GpuTier::forced() {
            descriptor.required_features = tier.device_features(descriptor.required_features);
            descriptor.required_limits = tier.device_limits(descriptor.required_limits);
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))
            .map_err(|e| format!("no GPU device on {} ({e})", info.name))?;
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("sprite.wgsl"));
        let pipeline = sprite_pipeline(&device, &layout, &shader, FORMAT);
        let looks = SpritePipelines::new(&device, &queue, &shader, &layout, &camera_layout, &texture_layout);
        let lights = Lights2D::new(&device, &queue, FORMAT, &camera_layout, &texture_layout);
        // A rounded tile and its normal map (a dome).
        let tile = texture(&device, &queue, &texture_layout, 32, |x, y| {
            let d = Vec2::new(x as f32 - 15.5, y as f32 - 15.5).length();
            if d < 15.0 { [200, 190, 170, 255] } else { [0; 4] }
        });
        let bumps = texture(&device, &queue, &texture_layout, 32, |x, y| {
            let p = Vec2::new(x as f32 - 15.5, 15.5 - y as f32) / 16.0;
            let n = p.extend((1.0 - p.length_squared()).max(0.0).sqrt()).normalize();
            let c = (n * 0.5 + 0.5) * 255.0;
            [c.x as u8, c.y as u8, c.z as u8, 255]
        });
        let textures = vec![tile, bumps];
        let normal_maps = if scene.normal_maps {
            vec![Some(Texture::new(1, 32, 32))]
        } else {
            Vec::new()
        };
        let (w, h) = (scene.size.0 as f32, scene.size.1 as f32);
        let projection = crate::ctx::screen_projection(Vec2::new(w, h));
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&projection.to_cols_array()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        // Sprites in a grid over the screen, overlapping when there are many.
        let columns = (scene.sprites as f32 * w / h).sqrt().ceil().max(1.0) as usize;
        let rows = scene.sprites.div_ceil(columns).max(1);
        let sprites: Vec<SpriteInstance> = (0..scene.sprites)
            .map(|i| {
                let at = [
                    (i % columns) as f32 / columns as f32 * w + 16.0,
                    (i / columns) as f32 / rows as f32 * h + 16.0,
                ];
                SpriteInstance::new(at, [32.0, 32.0], [0.5, 0.5], i as f32 * 0.3, [255; 4])
            })
            .collect();
        let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice::<SpriteInstance, u8>(&sprites),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut world = World::new();
        for i in 0..scene.lights {
            let light = PointLight2D {
                color: Color::rgb(1.0, 0.8 - (i % 3) as f32 * 0.2, 0.5 + (i % 2) as f32 * 0.4),
                radius: 160.0,
                intensity: 0.6,
                shadows: scene.shadows,
                ..Default::default()
            };
            world.spawn((Transform2D::default(), light));
        }
        for i in 0..scene.occluders {
            let at = Vec2::new(
                (i * 7919 % 997) as f32 / 997.0 * w,
                (i * 104_729 % 991) as f32 / 991.0 * h,
            );
            world.spawn((Transform2D::at(at), Occluder2D::rect(Vec2::splat(20.0))));
        }
        let output = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("2d lights bench frame"),
                size: wgpu::Extent3d {
                    width: scene.size.0,
                    height: scene.size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Ok(Self {
            device,
            queue,
            adapter: format!("{} ({:?})", info.name, info.backend),
            scene,
            pipeline,
            looks,
            lights,
            textures,
            camera,
            instances,
            sprites: sprites.len(),
            world,
            frame: LightFrame2D::default(),
            normal_maps,
            output,
            tick: 0,
        })
    }

    /// The GPU it runs on.
    #[must_use]
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Draws one frame, the lights a step further along, and waits for it.
    pub fn frame(&mut self) -> Lights2DFrame {
        let start = Instant::now();
        self.tick += 1;
        let (w, h) = (self.scene.size.0 as f32, self.scene.size.1 as f32);
        let t = self.tick as f32 / 60.0;
        let mut i = 0.0f32;
        self.world
            .query::<(&mut Transform2D, &PointLight2D)>()
            .for_each(|(at, _)| {
                let angle = t * (0.3 + (i * 0.37) % 1.0) + i;
                at.position = Vec2::new(
                    ((i * 0.618).fract() + 0.1 * angle.cos()) * w,
                    ((i * 0.414).fract() + 0.1 * angle.sin()) * h,
                );
                i += 1.0;
            });
        // One batch: the sprites are already on the GPU, all one texture.
        let batches = [Batch {
            clip: None,
            material: 0,
            texture: 0,
            start: 0,
            end: self.sprites as u32,
        }];
        let collect_start = Instant::now();
        let view = Rect {
            min: Vec2::ZERO,
            max: Vec2::new(w, h),
        };
        let ambient = self.scene.lit.then_some(Color::rgb(0.1, 0.1, 0.15));
        // Unlit: what the engine does every frame for a game without
        // lights (finds none and turns nothing on).
        self.frame.collect(&mut self.world, view, ambient);
        let collect = collect_start.elapsed();
        if self.frame.on {
            self.lights.prepare(
                &self.device,
                &self.queue,
                &self.frame,
                crate::ctx::screen_projection(Vec2::new(w, h)),
                [self.scene.size.0, self.scene.size.1],
                &self.normal_maps,
                &self.textures,
                &batches,
            );
        }
        let lighting = self.frame.on.then(|| Lighting {
            lights: &self.lights,
            frame: &self.frame,
            normal_maps: &self.normal_maps,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let layers = [(&self.camera, &batches[..], 0)];
        Renderer::sprites_lit(
            &mut encoder,
            &self.output,
            Some(Color::BLACK),
            None,
            &SpritePass {
                viewport: [self.scene.size.0, self.scene.size.1],
                scale: 1.0,
                pipeline: &self.pipeline,
                instances: &self.instances,
                textures: &self.textures,
                layers: &layers,
            },
            Some((&self.looks, FORMAT)),
            lighting.as_ref(),
        );
        let submitted = self.queue.submit([encoder.finish()]);
        let cpu = start.elapsed();
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted),
            timeout: Some(Duration::from_secs(5)),
        });
        Lights2DFrame {
            collect,
            cpu,
            total: start.elapsed(),
            shadow_vertices: self.frame.vertices.len(),
        }
    }
}

#[cfg(all(test, feature = "3d"))]
mod tests {
    use super::*;

    #[test]
    fn the_benchmark_scene_draws_its_lights_and_shadows() {
        let scene = Lights2DScene {
            size: (256, 256),
            sprites: 100,
            lights: 10,
            shadows: true,
            occluders: 20,
            normal_maps: true,
            lit: true,
        };
        let mut bench = match Lights2DBench::new(scene) {
            Ok(bench) => bench,
            Err(reason) => return crate::render::render_test_support::no_gpu(reason),
        };
        let frame = bench.frame();
        assert!(frame.total >= frame.cpu && frame.cpu >= frame.collect);
        assert!(frame.shadow_vertices > 0, "some occluder is in some light's reach");
        assert_eq!(bench.frame.lights.len(), 10);
    }
}
