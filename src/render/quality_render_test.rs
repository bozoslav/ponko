//! Headless GPU tests of graphics quality presets switched at run time:
//! every preset renders without validation errors, a scaled scene still
//! fills the whole window, and shadow maps are made again at the new size.
//! Skips without a GPU.
use super::gpu::GpuTexture;
use super::gpu3d::*;
#[allow(unused_imports)]
use crate::d3::{SpotLight as SpotLight3D, Transform as Transform3D};
use crate::prelude::*;
#[allow(unused_imports)]
use crate::render::{AntiAliasing, GraphicsQuality, MaterialDesc, MeshData, Model};
use crate::three::{Frame3D, GpuMaterialDesc, Scene3D, SkinFrame, SkyLight};

const W: u32 = 64;
const H: u32 = 48;

struct Fixture {
    device: wgpu::Device,
    queue: wgpu::Queue,
    textures: Vec<GpuTexture>,
    output: wgpu::Texture,
    target: wgpu::TextureView,
    renderer: Renderer3D,
    ctx: Ctx,
    /// Kept between frames, as the engine keeps it (far shadow cascades
    /// can be kept from an earlier frame).
    frame: Frame3D,
    timer: Option<super::timer::GpuTimer>,
}

impl Fixture {
    fn new() -> Option<Self> {
        let gpu = crate::render::render_test_support::gpu_with(|adapter| {
            Ok(wgpu::DeviceDescriptor {
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            })
        })?;
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
                width: W,
                height: H,
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

        // A ground, a box on it, a sun, and a shadowed spot light above.
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(W as f32, H as f32));
        let matte = MaterialDesc {
            color: Color::WHITE,
            roughness: 1.0,
            ..Default::default()
        };
        let material = ctx.gfx.create_material(matte);
        let ground = MeshData::plane(20.0);
        let cube = MeshData::cube(1.0);
        let ground_mesh = ctx.gfx.create_mesh(&ground, material);
        let cube_mesh = ctx.gfx.create_mesh(&cube, material);
        ctx.world.spawn((Transform3D::default(), Model::new(ground_mesh)));
        ctx.world
            .spawn((Transform3D::at(vec3(0.0, 0.5, 0.0)), Model::new(cube_mesh)));
        let lamp = Transform3D::at(vec3(1.5, 4.0, 1.0)).looking_at(vec3(0.0, 0.0, 0.0));
        ctx.world.spawn((
            lamp,
            SpotLight3D {
                intensity: 60.0,
                range: 12.0,
                shadows: true,
                ..Default::default()
            },
        ));
        ctx.scene3d.camera.position = vec3(0.0, 5.0, 6.0);
        ctx.scene3d.camera.look_at(Vec3::ZERO);
        ctx.scene3d.sky.atmosphere = false;
        ctx.scene3d.post.bloom = 0.0;

        let mut renderer = Renderer3D::new(&device, &layout, output.format(), 0, &white, 4, true);
        renderer.create_material(&device, &GpuMaterialDesc::new(&matte), &textures);
        renderer.create_mesh(&device, &ground, &[(0..ground.indices.len() as u32, material)], true);
        renderer.create_mesh(&device, &cube, &[(0..cube.indices.len() as u32, material)], true);
        let timer = super::timer::GpuTimer::new(&device, &queue);
        Some(Self {
            timer,
            device,
            queue,
            textures,
            output,
            target,
            renderer,
            ctx,
            frame: Frame3D::default(),
        })
    }

    /// Draws one frame and reads the window back. Fails the test on a
    /// validation error.
    fn draw(&mut self) -> Vec<u8> {
        let (device, queue, ctx) = (&self.device, &self.queue, &mut self.ctx);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        if let Some(timer) = &mut self.timer {
            timer.collect(device);
            timer.begin_frame();
        }
        let frame = &mut self.frame;
        frame.motion.reset |= std::mem::take(&mut ctx.gfx.frame3d.motion.reset);
        frame.size_limit = device.limits().max_texture_dimension_2d;
        frame.collect(
            &mut ctx.world,
            &ctx.gfx.assets3d,
            &SkinFrame::default(),
            &ctx.scene3d.camera,
            &ctx.scene3d.lighting,
        );
        let scene = Scene3D {
            frame,
            lighting: &ctx.scene3d.lighting,
            sky: &ctx.scene3d.sky,
            light: SkyLight {
                sun: Vec3::splat(ctx.scene3d.lighting.sun_intensity),
                sky: Vec3::ONE,
                ground: Vec3::ONE,
            },
            camera: ctx.scene3d.camera.position,
            post: &ctx.scene3d.post,
            time: (0.0, 1.0 / 60.0),
            occlusion: false,
        };
        self.renderer.prepare(
            device,
            queue,
            &Prepare3D {
                scene: &scene,
                lut: None,
                size: (W, H),
            },
        );
        let buffers = self.renderer.draw(
            device,
            &self.target,
            wgpu::Color::BLACK,
            &self.textures,
            self.timer.as_ref(),
        );
        queue.submit(buffers);
        if let Some(timer) = &mut self.timer {
            let mut encoder = device.create_command_encoder(&Default::default());
            timer.resolve(&mut encoder);
            queue.submit([encoder.finish()]);
            timer.submitted();
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(W * H * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: None,
                },
            },
            self.output.size(),
        );
        queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let pixels = readback.get_mapped_range(..).unwrap().to_vec();
        if let Some(timer) = &mut self.timer {
            timer.collect(device);
        }
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("validation error: {error}");
        }
        pixels
    }
}

/// The mean absolute difference per channel, 0 to 255.
fn difference(a: &[u8], b: &[u8]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(&a, &b)| a.abs_diff(b) as f32).sum::<f32>() / a.len() as f32
}

/// How many pixels of a quarter-size block in each corner of the window are
/// lit (not black).
fn lit_corners(pixels: &[u8]) -> [usize; 4] {
    let lit = |x0: u32, y0: u32| {
        let mut n = 0;
        for y in y0..y0 + H / 4 {
            for x in x0..x0 + W / 4 {
                let i = ((y * W + x) * 4) as usize;
                n += usize::from(pixels[i] > 8 || pixels[i + 1] > 8 || pixels[i + 2] > 8);
            }
        }
        n
    };
    [
        lit(0, 0),
        lit(W - W / 4, 0),
        lit(0, H - H / 4),
        lit(W - W / 4, H - H / 4),
    ]
}

#[test]
fn switching_presets_back_and_forth_renders_clean_full_size_frames() {
    let Some(mut f) = Fixture::new() else { return };
    // The ground fills the lower half of the picture; the first draw is
    // the reference at the engine's defaults.
    let reference = f.draw();
    let lit = lit_corners(&reference);
    assert!(
        lit[2] > 0 && lit[3] > 0,
        "the ground reaches the bottom corners: {lit:?}"
    );
    // `None`: back to the engine's defaults, which are none of the presets.
    let start = (f.ctx.scene3d.lighting, f.ctx.scene3d.post);
    let levels = [
        Some(GraphicsQuality::Low),
        Some(GraphicsQuality::Ultra),
        Some(GraphicsQuality::Medium),
        Some(GraphicsQuality::High),
        Some(GraphicsQuality::Low),
        None,
        Some(GraphicsQuality::Ultra),
        Some(GraphicsQuality::Medium),
    ];
    for (round, quality) in levels.into_iter().enumerate() {
        match quality {
            Some(quality) => f.ctx.scene3d.set_quality(quality),
            None => (f.ctx.scene3d.lighting, f.ctx.scene3d.post) = start,
        }
        let first = f.draw();
        let second = f.draw();
        assert_eq!(
            first.len(),
            (W * H * 4) as usize,
            "round {round}: the output is the window's size"
        );
        if f.ctx.scene3d.post.ao_temporal {
            // Rotating AO slices and history deliberately change between frames.
            let delta = difference(&first, &second);
            assert!(delta < 1.0, "round {round}: temporal AO difference {delta}");
        } else {
            assert_eq!(first, second, "round {round}: stable spatial rendering");
        }
        let lit = lit_corners(&first);
        assert!(
            lit[2] > 0 && lit[3] > 0,
            "round {round} ({quality:?}): a scaled scene still reaches the window's bottom corners: {lit:?}"
        );
        let scene = f.renderer.scene_size().unwrap();
        assert_eq!(
            scene,
            ctx_scene_size(&f.ctx),
            "round {round}: the scene is drawn at the scaled size"
        );
        assert_eq!(f.renderer.scales_output(), f.ctx.scene3d.post.render_scale != 1.0);
    }
    // Back to where it started: the same picture as the first time.
    (f.ctx.scene3d.lighting, f.ctx.scene3d.post) = start;
    let again = f.draw();
    assert_eq!(again, reference, "switching away and back changes nothing");
}

fn ctx_scene_size(ctx: &Ctx) -> (u32, u32) {
    ctx.scene3d.post.render_size((W, H))
}

#[test]
fn a_scaled_scene_is_the_same_picture_at_the_windows_size() {
    let Some(mut f) = Fixture::new() else { return };
    f.ctx.scene3d.lighting.shadows = false;
    let full = f.draw();
    assert_eq!(f.renderer.scene_size(), Some((W, H)));
    assert!(!f.renderer.scales_output(), "no extra pass at scale 1");
    f.ctx.scene3d.post.render_scale = 0.5;
    let soft = f.draw();
    f.ctx.scene3d.post.sharpen = 1.0;
    let sharp = f.draw();
    assert_ne!(soft, sharp, "sharpening changes a scaled picture");
    f.ctx.scene3d.post.sharpen = 0.0;
    f.ctx.scene3d.post.render_scale = 1.0;
    for (scale, sharpen) in [(0.5, 0.0), (0.5, 1.0), (0.75, 0.5), (1.5, 0.0), (2.0, 0.5)] {
        f.ctx.scene3d.post.render_scale = scale;
        f.ctx.scene3d.post.sharpen = sharpen;
        let scaled = f.draw();
        let expect = ((W as f32 * scale).round() as u32, (H as f32 * scale).round() as u32);
        assert_eq!(f.renderer.scene_size(), Some(expect), "scale {scale}");
        assert!(f.renderer.scales_output());
        let d = difference(&full, &scaled);
        assert!(
            d < 14.0,
            "scale {scale}, sharpen {sharpen}: the same view, not shrunk or shifted ({d})"
        );
        let lit = lit_corners(&scaled);
        assert!(lit[2] > 0 && lit[3] > 0, "scale {scale}: {lit:?}");
    }
    // Antialiasing runs at the scene's size, then scales.
    for mode in [AntiAliasing::Fxaa, AntiAliasing::Smaa] {
        f.ctx.scene3d.post.render_scale = 0.5;
        f.ctx.scene3d.post.anti_aliasing = mode;
        let aa = f.draw();
        assert_eq!(f.renderer.scene_size(), Some((W / 2, H / 2)));
        assert!(difference(&full, &aa) < 14.0, "{mode:?}");
    }
    // And back to full size, with the extra target freed.
    f.ctx.scene3d.post.render_scale = 1.0;
    f.ctx.scene3d.post.anti_aliasing = AntiAliasing::None;
    let back = f.draw();
    assert_eq!(back, full);
    assert_eq!(f.renderer.scene_size(), Some((W, H)));
    assert!(!f.renderer.scales_output());
}

#[test]
fn shadow_maps_are_made_again_at_the_new_resolution_and_count() {
    let Some(mut f) = Fixture::new() else { return };
    f.draw();
    // The default 4096² atlas, or the largest texture the device allows.
    let atlas = Some(4096.min(f.device.limits().max_texture_dimension_2d));
    assert_eq!(f.renderer.shadow_map(), (2048, 4), "the engine's defaults");
    assert_eq!(f.renderer.light_shadow_map(), atlas, "one 4096² atlas");

    f.ctx.scene3d.lighting.shadow_resolution = 512;
    f.draw();
    assert_eq!(f.renderer.shadow_map(), (512, 4));
    assert_eq!(
        f.renderer.light_shadow_map(),
        atlas,
        "the light shadow map is left alone"
    );

    f.ctx.scene3d.lighting.shadow_cascades = 2;
    f.draw();
    assert_eq!(f.renderer.shadow_map(), (512, 2));

    // Fewer or smaller views share the same atlas; a new atlas size makes it again.
    f.ctx.scene3d.lighting.light_shadow_resolution = 256;
    f.ctx.scene3d.lighting.light_shadow_views = 4;
    f.draw();
    assert_eq!(f.renderer.light_shadow_map(), atlas);
    f.ctx.scene3d.lighting.light_shadow_atlas = 2048;
    f.draw();
    assert_eq!(f.renderer.light_shadow_map(), Some(2048));
    assert_eq!(f.renderer.shadow_map(), (512, 2));

    // Point and spot light shadows off: the map is freed.
    f.ctx.scene3d.lighting.light_shadow_views = 0;
    f.draw();
    assert_eq!(f.renderer.light_shadow_map(), None);
    // ... and made again when they come back.
    f.ctx.scene3d.lighting.light_shadow_views = 1;
    f.draw();
    assert_eq!(f.renderer.light_shadow_map(), Some(2048));
    // Not a power of two: rounded down.
    f.ctx.scene3d.lighting.light_shadow_atlas = 3000;
    f.draw();
    assert_eq!(f.renderer.light_shadow_map(), Some(2048));
    f.ctx.scene3d.lighting.light_shadow_atlas = 1;
    f.draw();
    assert_eq!(f.renderer.light_shadow_map(), Some(1024));

    // A resolution beyond the allowed range is clamped.
    f.ctx.scene3d.lighting.shadow_resolution = 1 << 20;
    f.draw();
    assert_eq!(
        f.renderer.shadow_map().0,
        4096.min(f.device.limits().max_texture_dimension_2d)
    );
    f.ctx.scene3d.lighting.shadow_resolution = 1;
    f.draw();
    assert_eq!(f.renderer.shadow_map().0, 256);

    // All shadows off: stand-ins of 1x1.
    f.ctx.scene3d.lighting.shadows = false;
    f.draw();
    assert_eq!(f.renderer.shadow_map(), (1, 1));
    assert_eq!(f.renderer.light_shadow_map(), None);
    f.ctx.scene3d.lighting.shadows = true;
    f.draw();
    assert_eq!(f.renderer.shadow_map(), (256, 2));
}

#[test]
fn a_bigger_shadow_map_changes_the_picture_less_than_it_changes_the_map() {
    // Presets differ in shadow detail but the lit ground and the box's
    // shadow stay: the picture is about the same.
    let Some(mut f) = Fixture::new() else { return };
    f.ctx.scene3d.set_quality(GraphicsQuality::Medium);
    f.ctx.scene3d.post.anti_aliasing = AntiAliasing::None;
    f.ctx.scene3d.post.ambient_occlusion.enabled = false;
    let medium = f.draw();
    f.ctx.scene3d.lighting.shadow_resolution = 4096;
    let sharp = f.draw();
    assert!(difference(&medium, &sharp) < 6.0, "{}", difference(&medium, &sharp));
}

#[test]
fn a_kept_far_cascade_still_shadows_the_frames_it_is_not_drawn_in() {
    let Some(mut f) = Fixture::new() else { return };
    // Two cascades, and the box far enough away to be in the far one.
    f.ctx.scene3d.lighting.shadow_cascades = 2;
    f.ctx.scene3d.lighting.light_shadow_views = 0;
    f.ctx.scene3d.lighting.sun_intensity = 1.0;
    f.ctx.scene3d.lighting.sun_direction = vec3(-1.0, -0.6, 0.0); // a long shadow
    f.ctx.scene3d.lighting.ambient_intensity = 0.1;
    f.ctx.scene3d.post.exposure = -1.0;
    f.ctx.world.query::<&mut Transform3D>().for_each(|t| {
        if t.position.y > 0.0 {
            t.scale = Vec3::splat(4.0); // the box: big enough to see
        }
    });
    f.ctx.scene3d.camera.position = vec3(0.0, 24.0, 30.0);
    f.ctx.scene3d.camera.look_at(Vec3::ZERO);
    f.ctx.scene3d.camera.fov_y = 20f32.to_radians(); // zoomed in on the box
    f.ctx.scene3d.post.anti_aliasing = AntiAliasing::None;
    f.ctx.scene3d.post.ambient_occlusion.enabled = false;
    f.ctx.scene3d.lighting.shadow_refresh = 1;
    let every_frame = f.draw();
    f.ctx.scene3d.lighting.shadows = false;
    let unshadowed = f.draw();
    let shadow = difference(&every_frame, &unshadowed);
    assert!(shadow > 0.5, "the box casts a visible shadow: {shadow}");

    f.ctx.scene3d.lighting.shadows = true;
    f.ctx.scene3d.lighting.shadow_refresh = 2;
    let mut kept = 0;
    for _ in 0..4 {
        let picture = f.draw();
        if f.frame.redrawn & 0b10 == 0 {
            kept += 1;
        }
        let off = difference(&picture, &every_frame);
        assert!(off < shadow * 0.25, "{off} against a shadow of {shadow}");
    }
    assert!(kept >= 1, "the far cascade was kept on some frames");
}

/// Real timestamp queries drive the same controller/application path as a
/// windowed game. Forced tiers also exercise its no-timestamps path.
#[test]
fn quality_autotune_uses_live_gpu_samples_and_resets_scaled_history() {
    use crate::render::{QualityAutotune, QualityAutotuneReason};
    const MAX_FRAMES: usize = 3000;
    let Some(mut f) = Fixture::new() else { return };
    f.ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa;
    f.ctx.scene3d.set_quality_autotune(Some(QualityAutotune {
        target_gpu_ms: 1000.0,
        min_render_scale: 1.0,
        max_render_scale: 1.05,
        ..Default::default()
    }));
    let before = f.draw();
    // A decision needs 180 consecutive fresh GPU samples, and one late
    // sample (a busy machine) restarts the count: draw until it lands, with
    // a generous cap, rather than for a fixed number of frames.
    for frame in 0..MAX_FRAMES {
        let sample = f.timer.as_ref().and_then(super::timer::GpuTimer::quality_sample);
        f.ctx.update_quality_autotune(sample, false);
        f.draw();
        if frame >= 185 && (f.timer.is_none() || f.ctx.scene3d.post.render_scale != 1.0) {
            break;
        }
    }
    let status = f.ctx.quality_autotune_status().unwrap();
    if f.timer.is_none() {
        assert_eq!(status.changes, 0);
        assert_eq!(status.reason, QualityAutotuneReason::GpuUnavailable);
        assert_eq!(f.ctx.scene3d.post.render_scale, 1.0);
        return;
    }
    assert_eq!(status.changes, 1, "{status:?}");
    assert_eq!(f.ctx.scene3d.post.render_scale, 1.05);
    assert!(status.history[0].gpu_ms > 0.0);
    // Prove an automatic scale decision uses the central reset and cannot
    // leave stale TAA/SSR/AO resources. The explicit reset is consumed by
    // the same prepare path as the normal targets invalidation.
    f.ctx.scene3d.post.render_scale = 1.0;
    f.ctx.scene3d.set_quality_autotune(Some(QualityAutotune {
        target_gpu_ms: 1000.0,
        min_render_scale: 1.0,
        max_render_scale: 1.05,
        ..Default::default()
    }));
    for _ in 0..MAX_FRAMES {
        f.draw();
        f.ctx
            .update_quality_autotune(f.timer.as_ref().and_then(super::timer::GpuTimer::quality_sample), false);
        if f.ctx.scene3d.post.render_scale != 1.0 {
            break;
        }
    }
    assert_eq!(f.ctx.scene3d.post.render_scale, 1.05);
    assert!(f.ctx.gfx.frame3d.motion.reset);
    let after = f.draw();
    assert!(f.renderer.history_reset().requested);
    assert!(f.renderer.history_reset().targets);
    assert_eq!(after.len(), before.len());
    assert!(lit_corners(&after)[2] > 0);
    f.ctx.scene3d.set_quality_autotune(Some(QualityAutotune {
        target_gpu_ms: 1000.0,
        max_shadow_resolution: 4096,
        ..Default::default()
    }));
    for _ in 0..MAX_FRAMES {
        f.draw();
        f.ctx
            .update_quality_autotune(f.timer.as_ref().and_then(super::timer::GpuTimer::quality_sample), false);
        if f.ctx.scene3d.lighting.shadow_resolution == 4096 {
            break;
        }
    }
    f.draw();
    assert_eq!(f.ctx.scene3d.lighting.shadow_resolution, 4096);
    assert_eq!(
        f.renderer.shadow_map().0,
        4096.min(f.device.limits().max_texture_dimension_2d)
    );
    // Optional local inspection artifacts, never reference regeneration.
    if std::env::var_os("PONKO_AUTOTUNE_IMAGES").is_some() {
        for (name, pixels) in [("before", before), ("after", after)] {
            image::save_buffer(format!(".autotune-{name}.png"), &pixels, W, H, image::ColorType::Rgba8).unwrap();
        }
    }
}
