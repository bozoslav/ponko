//! The 3D frame with one sample per pixel, as in a browser's WebGL2 (which has
//! no multisampled textures): it draws without validation errors, and the
//! opaque, sky and transparent passes, with a material reading the scene's
//! depth, give the picture the multisampled frame does. Skips without a GPU.
use super::gpu::GpuTexture;
use super::gpu3d::*;
#[allow(unused_imports)]
use crate::d3::Transform as Transform3D;
use crate::prelude::*;
#[allow(unused_imports)]
use crate::render::Blend;
#[allow(unused_imports)]
use crate::render::{MaterialDesc, Mesh, MeshData, Model, Shader};
use crate::three::{Frame3D, GpuMaterialDesc, Scene3D, SkinFrame, SkyLight};

const SIZE: u32 = 128;

/// An offscreen renderer with a 1x1 white texture, and a `Ctx` to fill.
struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    output: wgpu::Texture,
    target: wgpu::TextureView,
    textures: Vec<GpuTexture>,
    renderer: Renderer3D,
    ctx: Ctx,
    /// Screen effects to run: (effect, material).
    effects: Vec<(u32, u32)>,
}

impl Rig {
    /// `None` without a GPU.
    fn new(samples: u32) -> Option<Self> {
        Self::build(samples, false)
    }

    /// A rig whose renderer reads the scene depth as the GL backend does
    /// (WebGL2): as a float texture, with a copy for transparent surfaces.
    fn gl_depth(samples: u32) -> Option<Self> {
        Self::build(samples, true)
    }

    fn build(samples: u32, gl_depth: bool) -> Option<Self> {
        let gpu = crate::render::render_test_support::gpu_with(|_| Ok(wgpu::DeviceDescriptor::default()))?;
        let (device, queue) = (gpu.device, gpu.queue);
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
        let textures = vec![GpuTexture {
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
        FORCE_FLOAT_DEPTH.set(gl_depth);
        let renderer = Renderer3D::new(&device, &layout, output.format(), 0, &white, samples, true);
        FORCE_FLOAT_DEPTH.set(false);
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(SIZE as f32, SIZE as f32));
        ctx.scene3d.sky.atmosphere = false;
        Some(Self {
            device,
            queue,
            output,
            target,
            textures,
            renderer,
            ctx,
            effects: Vec::new(),
        })
    }

    /// A shader made from `source`, on the CPU and the GPU.
    fn shader(&mut self, source: &str) -> Shader {
        let shader = self.ctx.gfx.create_shader("test", source);
        self.renderer.create_shader(&self.device, "test", source).unwrap();
        shader
    }

    /// A mesh of `data` with `desc`, on the CPU and the GPU.
    fn mesh(&mut self, data: &MeshData, desc: MaterialDesc) -> Mesh {
        let material = self.ctx.gfx.create_material(desc);
        let mesh = self.ctx.gfx.create_mesh(data, material);
        self.renderer
            .create_material(&self.device, &GpuMaterialDesc::new(&desc), &self.textures);
        self.renderer
            .create_mesh(&self.device, data, &[(0..data.indices.len() as u32, material)], true);
        mesh
    }

    /// Draws the world and reads the pixels back (RGBA).
    fn draw(&mut self) -> Vec<u8> {
        let ctx = &mut self.ctx;
        let mut frame = Frame3D::default();
        frame.lod = ctx.scene3d.lod;
        frame.effects.clone_from(&self.effects);
        frame.collect(
            &mut ctx.world,
            &ctx.gfx.assets3d,
            &SkinFrame::default(),
            &ctx.scene3d.camera,
            &ctx.scene3d.lighting,
        );
        let scene = Scene3D {
            frame: &frame,
            lighting: &ctx.scene3d.lighting,
            sky: &ctx.scene3d.sky,
            // Sunlight only in the shadow scene: elsewhere the cubes glow
            // on black.
            light: SkyLight {
                sun: if ctx.scene3d.lighting.shadows {
                    Vec3::splat(ctx.scene3d.lighting.sun_intensity)
                } else {
                    Vec3::ZERO
                },
                sky: Vec3::ZERO,
                ground: Vec3::ZERO,
            },
            camera: ctx.scene3d.camera.position,
            post: &ctx.scene3d.post,
            time: (0.0, 1.0 / 60.0),
            occlusion: false,
        };
        self.renderer.prepare(
            &self.device,
            &self.queue,
            &Prepare3D {
                scene: &scene,
                lut: None,
                size: (SIZE, SIZE),
            },
        );
        let buffers = self
            .renderer
            .draw(&self.device, &self.target, wgpu::Color::BLACK, &self.textures, None);
        self.queue.submit(buffers);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: None,
                },
            },
            self.output.size(),
        );
        self.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        readback.get_mapped_range(..).unwrap().to_vec()
    }
}

/// A lit emissive cube in front of the camera, a depth-fading transparent
/// plane in front of it (reads the scene depth, `scene_distance`), and the
/// sky behind: every pass of the lit frame.
fn scene(rig: &mut Rig) {
    rig.ctx.scene3d.sky.atmosphere = true;
    let cube = rig.mesh(
        &MeshData::cube(2.0),
        MaterialDesc {
            color: Color::BLACK,
            emissive: Color::rgb(1.0, 0.2, 0.1),
            ..Default::default()
        },
    );
    rig.ctx.world.spawn((Transform3D::default(), Model::new(cube)));
    let shader = rig.shader(
        "fn surface(s: Surface) -> Surface { var o = s; o.alpha = clamp((scene_distance(s.screen) - distance(s.world, frame.camera.xyz)) * 0.5, 0.0, 0.6); return o; }",
    );
    let glass = rig.mesh(
        &MeshData::plane(3.0),
        MaterialDesc {
            color: Color::BLACK,
            emissive: Color::rgb(0.1, 0.3, 1.0),
            blend: Blend::Transparent,
            shader: Some(shader),
            double_sided: true,
            ..Default::default()
        },
    );
    rig.ctx.world.spawn((
        Transform3D {
            position: vec3(0.0, 0.0, 1.4),
            rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ..Default::default()
        },
        Model::new(glass),
    ));
    rig.ctx.scene3d.camera.position = vec3(0.0, 0.0, 6.0);
    rig.ctx.scene3d.camera.look_at(Vec3::ZERO);
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2]]
}

#[test]
fn a_single_sample_frame_draws_every_pass_like_the_multisampled_one() {
    let (Some(mut single), Some(mut multi)) = (Rig::new(1), Rig::new(4)) else {
        return;
    };
    scene(&mut single);
    scene(&mut multi);
    let one = single.draw();
    let four = multi.draw();
    // The middle of the cube, seen through the glass: lit, not the sky or black.
    let c = pixel(&one, SIZE / 2, SIZE / 2);
    assert!(c[0] > 60, "the cube is drawn: {c:?}");
    assert!(c[0] > c[2], "the cube's red wins over the glass's blue: {c:?}");
    // The sky shows beside it.
    let corner = pixel(&one, 4, 4);
    assert_ne!(corner, [0, 0, 0], "the sky is drawn");
    // Away from edges the two agree.
    for (x, y) in [(SIZE / 2, SIZE / 2), (4, 4), (SIZE - 5, 4), (SIZE / 2, SIZE / 2 + 12)] {
        let (a, b) = (pixel(&one, x, y), pixel(&four, x, y));
        for k in 0..3 {
            assert!(
                a[k].abs_diff(b[k]) <= 10,
                "pixel ({x}, {y}): one sample {a:?}, four {b:?}"
            );
        }
    }
}

/// A screen effect that paints what is nearer than 5.5 units green (the
/// cube, seen through the glass) and leaves the rest: it reads the depth.
const NEAR_GREEN: &str = "
fn effect(p: Pixel) -> vec3<f32> {
    return select(p.color, vec3<f32>(0.0, 1.0, 0.0), !p.sky && p.distance < 5.5);
}";

/// The GL backend's depth reads (WebGL2: the scene depth as a float texture
/// sampled through a non-filtering sampler, and a copy of it for the
/// transparent pass) draw the frame the depth binding does, here on this
/// machine's GPU: the depth-fading glass and a depth-reading screen effect.
#[test]
fn gl_depth_reads_draw_the_frame_the_depth_binding_does() {
    for samples in [1, 4] {
        let (Some(mut gl), Some(mut native)) = (Rig::gl_depth(samples), Rig::new(samples)) else {
            return;
        };
        for rig in [&mut gl, &mut native] {
            scene(rig);
            let effect = rig.renderer.create_effect(&rig.device, "near", NEAR_GREEN).unwrap();
            rig.effects.push((effect, 0));
        }
        let scope = gl.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let ours = gl.draw();
        assert!(
            pollster::block_on(scope.pop()).is_none(),
            "{samples} samples: no validation error"
        );
        assert_eq!(
            gl.renderer.copies_scene_depth(),
            (true, true),
            "{samples} samples: the GL path, with the transparent pass's copy"
        );
        assert_eq!(native.renderer.copies_scene_depth(), (false, false));
        let theirs = native.draw();
        // The effect saw the depth: green on the cube, the sky beside it.
        let c = pixel(&ours, SIZE / 2, SIZE / 2);
        assert!(c[1] > 200 && c[0] < 40, "{samples} samples: the cube is near: {c:?}");
        assert_ne!(pixel(&ours, 4, 4)[1], 255, "{samples} samples: the sky is far");
        let differing = ours.iter().zip(&theirs).filter(|(a, b)| a.abs_diff(**b) > 1).count();
        assert_eq!(
            differing, 0,
            "{samples} samples: {differing} bytes differ from the depth binding's frame"
        );
    }
}

/// The negative control of the test above: a depth that reads as 0 (what
/// WebGL2 returned for a depth texture loaded without a `NEAREST` filter)
/// changes the picture, so the comparison would catch it.
#[test]
fn a_depth_read_as_zero_changes_the_frame() {
    let (Some(mut broken), Some(mut native)) = (Rig::new(1), Rig::new(1)) else {
        return;
    };
    for rig in [&mut broken, &mut native] {
        scene(rig);
    }
    // Depth 0 is the near plane everywhere: nothing is sky, everything near.
    let zero = NEAR_GREEN.replace("!p.sky && p.distance < 5.5", "true");
    assert_ne!(zero, NEAR_GREEN);
    let effect = broken.renderer.create_effect(&broken.device, "zero", &zero).unwrap();
    broken.effects.push((effect, 0));
    let effect = native
        .renderer
        .create_effect(&native.device, "near", NEAR_GREEN)
        .unwrap();
    native.effects.push((effect, 0));
    let (a, b) = (broken.draw(), native.draw());
    let differing = a.iter().zip(&b).filter(|(x, y)| x.abs_diff(**y) > 1).count();
    assert!(differing > 0, "a wrong depth must show");
}
