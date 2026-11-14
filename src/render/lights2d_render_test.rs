//! 2D lighting drawn offscreen through the window's own path
//! (`Renderer::sprites_lit`): the world's lights gathered by
//! `LightFrame2D::collect`, the light map with its stencil shadows, normal
//! maps and the composite, then the unlit UI. Each test has a control
//! drawn without the feature under test, which must come out different.
//! Skips without a GPU; `PONKO_GPU_TIER=webgl2` runs it with WebGL2's
//! limits.

use glam::Vec2;
use ponko_ecs::World;

use super::batch::{DrawList, SpriteInstance};
use super::gpu::{GpuTexture, Lighting, Renderer, SpritePass, sprite_pipeline};
use super::lights2d::Lights2D;
use super::render_test_support::{Image, gpu, keep_image, read_texture, target, white_textures};
use super::sprite_pipelines::SpritePipelines;
use crate::camera::Rect;
use crate::color::Color;
use crate::ecs::Transform2D;
use crate::light2d::{DirectionalLight2D, LightFrame2D, Occluder2D, PointLight2D, SpotLight2D};
use crate::texture::Texture;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: u32 = 64;

struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture_layout: wgpu::BindGroupLayout,
    camera_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    looks: SpritePipelines,
    lights: Lights2D,
    textures: Vec<GpuTexture>,
}

/// One sprite: texture id, center, size, rotation.
#[derive(Clone, Copy)]
struct Draw {
    texture: u32,
    at: Vec2,
    size: Vec2,
    rotation: f32,
}

/// A white sprite over the whole target: what the light falls on.
const FLOOR: Draw = Draw {
    texture: 0,
    at: Vec2::splat(SIZE as f32 / 2.0),
    size: Vec2::splat(SIZE as f32),
    rotation: 0.0,
};

impl Rig {
    fn new() -> Option<Self> {
        let gpu = gpu()?;
        let (device, queue) = (gpu.device, gpu.queue);
        let (texture_layout, _, textures) = white_textures(&device, &queue);
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
        Some(Self {
            device,
            queue,
            texture_layout,
            camera_layout,
            pipeline,
            looks,
            lights,
            textures,
        })
    }

    /// Adds a `width` x 1 sRGB texture of `rgba` texels; returns its id.
    fn texture(&mut self, texels: &[[u8; 4]]) -> u32 {
        let width = texels.len() as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            texels.as_flattened(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: None,
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        self.textures.push(GpuTexture {
            bind_group: self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.texture_layout,
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
            nearest: true,
            mips: None,
        });
        (self.textures.len() - 1) as u32
    }

    /// Draws `world` sprites over black, lit by `world_lights` (or unlit
    /// with `None`), and `ui` sprites on top; returns the image.
    fn draw(
        &mut self,
        world: &[Draw],
        ui: &[Draw],
        lights: Option<(&mut World, Option<Color>)>,
        normal_maps: &[Option<Texture>],
    ) -> Image {
        use wgpu::util::DeviceExt;
        let mut world_list = DrawList::default();
        let mut ui_list = DrawList::default();
        for (list, draws) in [(&mut world_list, world), (&mut ui_list, ui)] {
            for d in draws {
                let sprite = SpriteInstance::new(d.at.into(), d.size.into(), [0.5, 0.5], d.rotation, [255; 4]);
                list.push(d.texture, 0.0, sprite);
            }
        }
        let (world_sprites, world_batches) = world_list.finish();
        let (ui_sprites, ui_batches) = ui_list.finish();
        let mut instances = world_sprites.to_vec();
        instances.extend_from_slice(ui_sprites);
        let device = &self.device;
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice::<SpriteInstance, u8>(&instances),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let projection = crate::ctx::screen_projection(Vec2::splat(SIZE as f32));
        let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&projection.to_cols_array()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            }],
        });
        let mut frame = LightFrame2D::default();
        if let Some((world, ambient)) = lights {
            let view = Rect {
                min: Vec2::ZERO,
                max: Vec2::splat(SIZE as f32),
            };
            frame.collect(world, view, ambient);
        }
        if frame.on {
            self.lights.prepare(
                device,
                &self.queue,
                &frame,
                projection,
                [SIZE, SIZE],
                normal_maps,
                &self.textures,
                world_batches,
            );
        }
        let lighting = frame.on.then_some(Lighting {
            lights: &self.lights,
            frame: &frame,
            normal_maps,
        });
        let output = target(device, (SIZE, SIZE), FORMAT);
        let view = output.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        let layers = [
            (&camera, world_batches, 0),
            (&camera, ui_batches, world_sprites.len() as u32),
        ];
        Renderer::sprites_lit(
            &mut encoder,
            &view,
            Some(Color::BLACK),
            None,
            &SpritePass {
                viewport: [SIZE, SIZE],
                scale: 1.0,
                pipeline: &self.pipeline,
                instances: &instance_buffer,
                textures: &self.textures,
                layers: &layers,
            },
            Some((&self.looks, FORMAT)),
            lighting.as_ref(),
        );
        self.queue.submit([encoder.finish()]);
        let image = Image {
            width: SIZE,
            height: SIZE,
            rgba: read_texture(device, &self.queue, &output),
        };
        keep_image(&image);
        image
    }
}

/// Pixel `(x, y)`'s red channel, 0..255 (sRGB).
fn red(image: &Image, x: u32, y: u32) -> u8 {
    image.pixel(x, y)[0]
}

fn point(at: Vec2, light: PointLight2D) -> (Transform2D, PointLight2D) {
    (Transform2D::at(at), light)
}

#[test]
fn unlit_frames_draw_exactly_as_the_sprite_pass_without_lighting() {
    let Some(mut rig) = Rig::new() else { return };
    let unlit = rig.draw(&[FLOOR], &[], None, &[]);
    // An empty world turns nothing on either.
    let empty = rig.draw(&[FLOOR], &[], Some((&mut World::new(), None)), &[]);
    assert_eq!(unlit.rgba, empty.rgba);
    assert_eq!(unlit.pixel(10, 10), [255, 255, 255, 255]);
}

#[test]
fn the_ambient_light_dims_the_world_and_leaves_the_ui_alone() {
    let Some(mut rig) = Rig::new() else { return };
    let badge = Draw {
        at: Vec2::new(8.0, 8.0),
        size: Vec2::splat(8.0),
        ..FLOOR
    };
    let ambient = Some(Color::rgb(0.5, 0.5, 0.5));
    let lit = rig.draw(&[FLOOR], &[badge], Some((&mut World::new(), ambient)), &[]);
    // The control: the same frame unlit is white everywhere.
    let unlit = rig.draw(&[FLOOR], &[badge], None, &[]);
    assert_eq!(red(&unlit, 40, 40), 255);
    let world = red(&lit, 40, 40);
    assert!(
        (world as i32 - 128).abs() <= 3,
        "world {world}: white times sRGB 0.5 ambient"
    );
    assert_eq!(red(&lit, 8, 8), 255, "the UI stays unlit");
}

#[test]
fn a_point_light_fades_from_its_center_to_dark_at_its_radius() {
    let Some(mut rig) = Rig::new() else { return };
    let mut world = World::new();
    world.spawn(point(
        Vec2::new(32.0, 32.0),
        PointLight2D {
            radius: 20.0,
            falloff: 1.0,
            ..Default::default()
        },
    ));
    let image = rig.draw(&[FLOOR], &[], Some((&mut world, None)), &[]);
    let (center, near, edge, outside) = (
        red(&image, 32, 32),
        red(&image, 40, 32),
        red(&image, 50, 32),
        red(&image, 60, 32),
    );
    assert!(center >= 250, "center {center}: full light on white");
    assert!(
        center > near && near > edge,
        "{center} > {near} > {edge}: fades outward"
    );
    assert_eq!(outside, 0, "beyond the radius only the black ambient");
    // Twice as bright brightens past the surface's own color nowhere
    // (white can't get whiter) but widens the bright core.
    let mut bright = World::new();
    bright.spawn(point(
        Vec2::new(32.0, 32.0),
        PointLight2D {
            radius: 20.0,
            falloff: 1.0,
            intensity: 2.0,
            ..Default::default()
        },
    ));
    let image2 = rig.draw(&[FLOOR], &[], Some((&mut bright, None)), &[]);
    assert!(red(&image2, 40, 32) > near);
}

#[test]
fn an_occluder_shadows_the_far_side_from_the_light_only() {
    let Some(mut rig) = Rig::new() else { return };
    let scene = |shadows: bool| {
        let mut world = World::new();
        world.spawn(point(
            Vec2::new(12.0, 32.0),
            PointLight2D {
                radius: 64.0,
                falloff: 0.5,
                shadows,
                ..Default::default()
            },
        ));
        world.spawn((
            Transform2D::at(Vec2::new(28.0, 32.0)),
            Occluder2D::rect(Vec2::new(4.0, 12.0)),
        ));
        world
    };
    let shadowed = rig.draw(&[FLOOR], &[], Some((&mut scene(true), None)), &[]);
    let control = rig.draw(&[FLOOR], &[], Some((&mut scene(false), None)), &[]);
    assert_eq!(red(&shadowed, 50, 32), 0, "behind the occluder");
    assert!(red(&control, 50, 32) > 100, "the control lights it");
    // In front of, beside and on the occluder: as lit as the control.
    for (x, y) in [(20, 32), (50, 10), (28, 32)] {
        let (a, b) = (red(&shadowed, x, y), red(&control, x, y));
        assert!(a.abs_diff(b) <= 2 && a > 50, "({x}, {y}): {a} vs {b}");
    }
}

#[test]
fn hundreds_of_shadowed_lights_each_keep_their_shadow() {
    let Some(mut rig) = Rig::new() else { return };
    // More shadowed lights than stencil values: the light pass starts over
    // with a cleared stencil partway through.
    let mut many = World::new();
    for _ in 0..300 {
        many.spawn(point(
            Vec2::new(12.0, 32.0),
            PointLight2D {
                radius: 64.0,
                falloff: 0.5,
                intensity: 1.0 / 300.0,
                shadows: true,
                ..Default::default()
            },
        ));
    }
    many.spawn((
        Transform2D::at(Vec2::new(28.0, 32.0)),
        Occluder2D::rect(Vec2::new(4.0, 12.0)),
    ));
    let image = rig.draw(&[FLOOR], &[], Some((&mut many, None)), &[]);
    assert_eq!(red(&image, 50, 32), 0, "every light's shadow");
    let mut one = World::new();
    one.spawn(point(
        Vec2::new(12.0, 32.0),
        PointLight2D {
            radius: 64.0,
            falloff: 0.5,
            shadows: true,
            ..Default::default()
        },
    ));
    let single = rig.draw(&[FLOOR], &[], Some((&mut one, None)), &[]);
    let (a, b) = (red(&image, 20, 32), red(&single, 20, 32));
    // The light map adds in half floats: 300 small sums round a little.
    assert!(a.abs_diff(b) <= 8, "300 lights of 1/300 add up to one: {a} vs {b}");
}

#[test]
fn a_spot_light_lights_only_inside_its_cone() {
    let Some(mut rig) = Rig::new() else { return };
    let mut world = World::new();
    let mut t = Transform2D::at(Vec2::new(4.0, 32.0));
    t.rotation = 0.0; // pointing right
    world.spawn((
        t,
        SpotLight2D {
            radius: 64.0,
            angle: 0.3,
            softness: 0.0,
            falloff: 0.5,
            ..Default::default()
        },
    ));
    let image = rig.draw(&[FLOOR], &[], Some((&mut world, None)), &[]);
    assert!(red(&image, 40, 32) > 100, "on the center line");
    assert_eq!(red(&image, 40, 4), 0, "well outside the cone");
    assert_eq!(red(&image, 4, 50), 0, "beside the light");
}

#[test]
fn a_directional_light_lights_everything_but_its_shadows() {
    let Some(mut rig) = Rig::new() else { return };
    let mut world = World::new();
    world.spawn((DirectionalLight2D {
        direction: Vec2::X,
        intensity: 0.5,
        shadows: true,
        ..Default::default()
    },));
    world.spawn((
        Transform2D::at(Vec2::new(16.0, 32.0)),
        Occluder2D::rect(Vec2::new(4.0, 8.0)),
    ));
    let image = rig.draw(&[FLOOR], &[], Some((&mut world, None)), &[]);
    assert_eq!(red(&image, 60, 32), 0, "the endless strip behind the box");
    let lit = red(&image, 60, 10);
    assert!((lit as i32 - 188).abs() <= 4, "{lit}: half light on white, sRGB");
    assert_eq!(red(&image, 8, 32), lit, "before the box");
}

#[test]
fn normal_maps_light_the_side_facing_the_light_and_turn_with_the_sprite() {
    let Some(mut rig) = Rig::new() else { return };
    // Two squares left and right of a light; their normal map faces right
    // (+x): toward the light on the left, away from it on the right.
    let bump = rig.texture(&[[255, 255, 255, 255]]);
    // sRGB-stored like any image: (1, 0.5, 0.5) encoded is "facing right".
    let normals = rig.texture(&[[255, 128, 128, 255]]);
    let maps = {
        let mut maps = vec![None; bump as usize + 1];
        maps[bump as usize] = Some(Texture::new(normals, 1, 1));
        maps
    };
    let square = |x: f32, rotation: f32| Draw {
        texture: bump,
        at: Vec2::new(x, 32.0),
        size: Vec2::splat(12.0),
        rotation,
    };
    let scene = || {
        let mut world = World::new();
        world.spawn(point(
            Vec2::new(32.0, 32.0),
            PointLight2D {
                radius: 64.0,
                falloff: 0.0,
                height: 8.0,
                ..Default::default()
            },
        ));
        world
    };
    let draws = [FLOOR, square(12.0, 0.0), square(52.0, 0.0)];
    let mapped = rig.draw(&draws, &[], Some((&mut scene(), None)), &maps);
    let flat = rig.draw(&draws, &[], Some((&mut scene(), None)), &[]);
    let (left, right) = (red(&mapped, 12, 32), red(&mapped, 52, 32));
    assert!(left > 200, "the left square faces the light: {left}");
    assert!(right < 60, "the right square faces away from it: {right}");
    // The control: without the normal map both are lit flat, equally.
    let (fl, fr) = (red(&flat, 12, 32), red(&flat, 52, 32));
    assert!(fl > 240 && fr > 240, "{fl} {fr}");
    // Turned half a circle, the squares' normals face left.
    let turned = [
        FLOOR,
        square(12.0, std::f32::consts::PI),
        square(52.0, std::f32::consts::PI),
    ];
    let image = rig.draw(&turned, &[], Some((&mut scene(), None)), &maps);
    assert!(red(&image, 12, 32) < 60 && red(&image, 52, 32) > 200);
    // Mirrored by a negative width, the same.
    let mut mirrored = [FLOOR, square(12.0, 0.0), square(52.0, 0.0)];
    for d in &mut mirrored[1..] {
        d.size.x = -d.size.x;
    }
    let image = rig.draw(&mirrored, &[], Some((&mut scene(), None)), &maps);
    assert!(red(&image, 12, 32) < 60 && red(&image, 52, 32) > 200);
}
