//! The stage the regression suites (`regression_*_render_test.rs`) draw on:
//! one offscreen renderer set up as a game's is, a `Ctx` to fill, and the
//! controlled scenes both suites share. Every scene is made of engine
//! primitives (boxes, planes, WGSL written here), so the pictures and the
//! reference images made from them need no asset files.
//!
//! A stage records every GPU validation error its device reports;
//! [`Stage::finish`] fails the test if there was one, so "draws without a
//! GPU error" is part of every test that ends with it.
use std::sync::{Arc, Mutex};

use super::gpu::GpuTexture;
use super::gpu3d::*;
use super::render_test_support::*;
use crate::anim::{Joint, Skeleton};
use crate::d3::Transform as Transform3D;
use crate::prelude::*;
use crate::render::{GpuTier, MaterialDesc, Mesh, MeshData, Model, Shader, Tonemap};
use crate::three::{Blend, GpuMaterialDesc, Scene3D, SkyLight, linear};

/// The side of the square frames the scenes are drawn at.
pub(super) const SIZE: u32 = 128;

/// How a stage's device and target are set up.
#[derive(Clone, Copy)]
pub(super) struct Setup {
    pub size: (u32, u32),
    /// The format of the texture drawn into.
    pub format: wgpu::TextureFormat,
    /// The format of the view drawn through, when it isn't the texture's
    /// (a browser canvas: a linear texture drawn through an sRGB view).
    pub view_format: Option<wgpu::TextureFormat>,
    /// Opens the device with this tier's limits, as `PONKO_GPU_TIER` does
    /// for a game. `None`: the limits a game asks for on desktop. A tier
    /// forced on the whole run by the environment still applies on top.
    pub tier: Option<GpuTier>,
    pub samples: u32,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            size: (SIZE, SIZE),
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            view_format: None,
            tier: None,
            samples: 4,
        }
    }
}

pub(super) struct Stage {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The adapter it draws on.
    pub info: wgpu::AdapterInfo,
    pub renderer: Renderer3D,
    pub ctx: Ctx,
    pub output: wgpu::Texture,
    pub size: (u32, u32),
    textures: Vec<GpuTexture>,
    view_format: Option<wgpu::TextureFormat>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl Stage {
    /// A stage with the default setup; `None` without a GPU (logged).
    pub fn new() -> Option<Self> {
        Self::with(Setup::default())
    }

    pub fn with(setup: Setup) -> Option<Self> {
        let TestGpu { device, queue, info } = gpu_with(|adapter| {
            // The limits a game asks for (`gpu.rs`): the decal and global
            // illumination images need more texture slots than the default.
            let engine = wgpu::Limits {
                max_sampled_textures_per_shader_stage: adapter
                    .limits()
                    .max_sampled_textures_per_shader_stage
                    .clamp(16, 24),
                ..wgpu::Limits::default()
            };
            Ok(wgpu::DeviceDescriptor {
                required_limits: setup.tier.map_or(engine.clone(), |tier| tier.device_limits(engine)),
                ..Default::default()
            })
        })?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = errors.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| sink.lock().unwrap().push(e.to_string())));
        let (layout, white, textures) = white_textures(&device, &queue);
        let output = Self::target(&device, setup.size, setup.format, setup.view_format);
        let drawn_format = setup.view_format.unwrap_or(setup.format);
        let renderer = Renderer3D::new(&device, &layout, drawn_format, 0, &white, setup.samples, true);
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(setup.size.0 as f32, setup.size.1 as f32));
        ctx.gfx.assets3d.decals_supported = supports_decals(&device);
        ctx.gfx.assets3d.gi_supported = supports_gi(&device);
        // A black background, and the picture as lit: no curve, no glow.
        // Scenes about those turn them on.
        ctx.scene3d.sky.atmosphere = false;
        ctx.scene3d.post.tonemap = Tonemap::None;
        ctx.scene3d.post.bloom = 0.0;
        // White light from one side and a little from everywhere.
        let lighting = &mut ctx.scene3d.lighting;
        lighting.sun_direction = vec3(-0.5, -1.0, -0.35);
        lighting.sun_color = Color::WHITE;
        lighting.sun_intensity = 1.0;
        ctx.scene3d.sky.sky_color = Color::WHITE;
        ctx.scene3d.sky.ground_color = Color::WHITE;
        lighting.ambient_intensity = 0.05;
        lighting.shadows = false;
        lighting.shadow_distance = 30.0;
        ctx.scene3d.camera.position = vec3(0.0, 2.4, 5.0);
        ctx.scene3d.camera.look_at(vec3(0.0, 0.6, 0.0));
        Some(Self {
            device,
            queue,
            info,
            renderer,
            ctx,
            output,
            size: setup.size,
            textures,
            view_format: setup.view_format,
            errors,
        })
    }

    fn target(
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        view_format: Option<wgpu::TextureFormat>,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: view_format.as_slice(),
        })
    }

    /// Draws later frames at `size` (a window resize).
    pub fn resize(&mut self, size: (u32, u32)) {
        self.output = Self::target(&self.device, size, self.output.format(), self.view_format);
        self.size = size;
        self.ctx
            .scene3d
            .camera
            .set_viewport(Vec2::new(size.0 as f32, size.1 as f32));
    }

    /// A material shader made from `source`, on the CPU and the GPU, and
    /// its index in the renderer (for [`reload`](Self::reload)).
    pub fn shader(&mut self, name: &str, source: &str) -> (Shader, u32) {
        let shader = self.ctx.gfx.create_shader(name, source);
        let id = self
            .renderer
            .create_shader(&self.device, name, source)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        (shader, id)
    }

    /// The frame `frames` frames after the camera stepped `by` to the side,
    /// with the camera put back afterwards: the whole picture shifted, for
    /// a reference's negative control.
    pub fn shifted(&mut self, by: Vec3, frames: usize) -> Image {
        self.ctx.scene3d.camera.position += by;
        let image = self.settled(frames);
        self.ctx.scene3d.camera.position -= by;
        image
    }

    /// Hot-reloads shader `id` with `source`.
    pub fn reload(&mut self, id: u32, name: &str, source: &str) -> Result<(), String> {
        self.renderer
            .replace_shader(&self.device, id, name, source)
            .map_err(|e| e.to_string())
    }

    /// A mesh of `data` with `desc`, on the CPU and the GPU.
    pub fn mesh(&mut self, data: &MeshData, desc: MaterialDesc) -> Mesh {
        let material = self.ctx.gfx.create_material(desc);
        let mesh = self.ctx.gfx.create_mesh(data, material);
        self.renderer
            .create_material(&self.device, &GpuMaterialDesc::new(&desc), &self.textures);
        self.renderer
            .create_mesh(&self.device, data, &[(0..data.indices.len() as u32, material)], true);
        mesh
    }

    /// A unit cube of `color`.
    pub fn cube(&mut self, color: Color) -> Mesh {
        self.mesh(&MeshData::cube(1.0), MaterialDesc::color(color))
    }

    /// A box from `lo` to `hi`.
    pub fn block(&mut self, mesh: Mesh, lo: Vec3, hi: Vec3) -> Entity {
        self.ctx.world.spawn((
            Transform3D {
                position: (lo + hi) * 0.5,
                scale: hi - lo,
                ..Default::default()
            },
            Model::new(mesh),
        ))
    }

    /// A grey floor 20 across, its top at height 0.
    pub fn floor(&mut self) -> Entity {
        let grey = self.cube(Color::rgb(0.6, 0.6, 0.6));
        self.block(grey, vec3(-10.0, -1.0, -10.0), vec3(10.0, 0.0, 10.0))
    }

    /// Collects and draws one frame into `output`.
    pub fn render(&mut self) {
        let ctx = &mut self.ctx;
        let post = &ctx.scene3d.post;
        ctx.gfx.frame3d.motion.enabled = post.anti_aliasing == crate::render::AntiAliasing::Taa
            || (post.ao_temporal && post.ambient_occlusion.enabled);
        ctx.gfx.frame3d.lod = ctx.scene3d.lod;
        let mut skins = std::mem::take(&mut ctx.gfx.frame3d.skins);
        skins.motion = ctx.gfx.frame3d.motion.enabled;
        skins.collect(&mut ctx.world, &ctx.gfx.assets3d);
        ctx.gfx.frame3d.collect(
            &mut ctx.world,
            &ctx.gfx.assets3d,
            &skins,
            &ctx.scene3d.camera,
            &ctx.scene3d.lighting,
        );
        ctx.gfx.frame3d.skins = skins;
        let rgb = |c| {
            let [r, g, b, _] = linear(c);
            Vec3::new(r, g, b)
        };
        // The light as a game without an atmosphere gets it.
        let scene = Scene3D {
            frame: &ctx.gfx.frame3d,
            lighting: &ctx.scene3d.lighting,
            sky: &ctx.scene3d.sky,
            light: SkyLight {
                sun: rgb(ctx.scene3d.lighting.sun_color) * ctx.scene3d.lighting.sun_intensity,
                sky: rgb(ctx.scene3d.sky.sky_color),
                ground: rgb(ctx.scene3d.sky.ground_color),
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
                size: self.size,
            },
        );
        ctx.gfx.frame3d.motion.reset = false;
        let view = self.output.create_view(&wgpu::TextureViewDescriptor {
            format: self.view_format,
            ..Default::default()
        });
        let buffers = self
            .renderer
            .draw(&self.device, &view, wgpu::Color::BLACK, &self.textures, None);
        self.queue.submit(buffers);
    }

    /// Draws a frame and reads it back (RGBA8, as the target stores it).
    pub fn image(&mut self) -> Image {
        self.render();
        let image = Image::read(&self.device, &self.queue, &self.output);
        keep_image(&image);
        image
    }

    /// Draws `frames` frames and reads the last one back.
    pub fn settled(&mut self, frames: usize) -> Image {
        for _ in 1..frames {
            self.render();
        }
        self.image()
    }

    /// The pixel the camera sees the world point `p` at.
    pub fn pixel(&self, p: Vec3) -> (u32, u32) {
        let ndc = self.ctx.scene3d.camera.view_proj().project_point3(p);
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let x = ((ndc.x * 0.5 + 0.5) * w).clamp(0.0, w - 1.0);
        let y = ((0.5 - ndc.y * 0.5) * h).clamp(0.0, h - 1.0);
        (x as u32, y as u32)
    }

    /// The mean colour of the 3x3 pixels around the world point `p`.
    pub fn color_at(&self, image: &Image, p: Vec3) -> [f32; 3] {
        let (x, y) = self.pixel(p);
        mean_color(image, x, y, 1)
    }

    /// The GPU errors reported so far, taken.
    pub fn take_errors(&self) -> Vec<String> {
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        std::mem::take(&mut self.errors.lock().unwrap())
    }

    /// Ends the test: no GPU error was reported while it ran.
    pub fn finish(self) {
        let errors = self.take_errors();
        assert!(
            errors.is_empty(),
            "the GPU reported errors on {}: {errors:#?}",
            describe_adapter(&self.info)
        );
    }
}

/// The mean colour of the pixels within `radius` of (x, y), clipped to the
/// image.
pub(super) fn mean_color(image: &Image, x: u32, y: u32, radius: u32) -> [f32; 3] {
    let (mut sum, mut n) = ([0.0f32; 3], 0.0);
    for py in y.saturating_sub(radius)..=(y + radius).min(image.height - 1) {
        for px in x.saturating_sub(radius)..=(x + radius).min(image.width - 1) {
            let p = image.pixel(px, py);
            for k in 0..3 {
                sum[k] += f32::from(p[k]);
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

/// The mean of a colour's three channels.
pub(super) fn luma(c: [f32; 3]) -> f32 {
    (c[0] + c[1] + c[2]) / 3.0
}

/// Compares two images drawn by different devices or setups: within
/// `tolerance`, or the test fails with the numbers and `what`.
pub(super) fn assert_similar(a: &Image, b: &Image, tolerance: Tolerance, what: &str) {
    let d = diff_within(a, b, tolerance);
    record(&output_dir(), &format!("{what}: {d}; allowed {tolerance:?}"));
    assert!(d.within(tolerance), "{what}: {d}, allowed {tolerance:?}");
}

/// The same, the other way: the images differ by more than `tolerance`
/// (a negative control, or a change that must show).
pub(super) fn assert_differs(a: &Image, b: &Image, tolerance: Tolerance, what: &str) {
    let d = diff_within(a, b, tolerance);
    record(
        &output_dir(),
        &format!("{what} (must differ): {d}; allowed {tolerance:?}"),
    );
    assert!(!d.within(tolerance), "{what}: only {d}, within {tolerance:?}");
}

/// What two frames of one scene may differ by across devices of one
/// adapter (another tier, another target format): rounding and edge
/// samples.
pub(super) const SAME_PICTURE: Tolerance = Tolerance {
    channel: 6,
    outliers: 0.01,
    edge: 0,
};

/// What a frame may differ by from its reference image, made on another
/// adapter. Edges (silhouettes, shadow borders) may sit a pixel apart and
/// resolve differently (`edge`), shades may be 12 levels off, and what is
/// left may be wrong on 0.75% of the frame: 123 of these scenes' pixels,
/// which is an edge two pixels out of place along half the frame's width.
/// The budget is half of what the mildest negative control produces
/// (`reference_rejects` checks that margin), so the shifted, unshadowed or
/// unposed frame of each scene is still far beyond it.
pub(super) const REFERENCE: Tolerance = Tolerance {
    channel: 12,
    outliers: 0.0075,
    edge: 1,
};

/// The same for pictures of screen-space effects, global illumination and
/// the post chain: their shades come out of longer chains of filtering
/// (16 levels), and their noise may put more single pixels off (1.5%).
/// Still less than half of what the effect switched off changes.
pub(super) const EFFECT_REFERENCE: Tolerance = Tolerance {
    channel: 16,
    outliers: 0.015,
    edge: 1,
};

// ---- The scenes ----

/// Opaque: a red box in front of a blue one on the grey floor, lit by the
/// sun from the upper left front. Returns points on the red box's front
/// face, the blue box's top, and the red box's face where it hides the
/// blue box behind it.
pub(super) fn opaque(stage: &mut Stage) -> (Vec3, Vec3, Vec3) {
    stage.floor();
    let red = stage.cube(Color::rgb(0.9, 0.1, 0.1));
    let blue = stage.cube(Color::rgb(0.1, 0.1, 0.9));
    stage.block(red, vec3(-1.0, 0.0, 0.5), vec3(0.2, 1.2, 1.7));
    stage.block(blue, vec3(-0.4, 0.0, -1.5), vec3(1.6, 1.8, -0.3));
    (vec3(-0.4, 0.6, 1.7), vec3(0.9, 1.8, -0.9), vec3(0.0, 1.0, 1.7))
}

/// The three panes of [`panes`], near to far: where each stands (x, y, z)
/// and its colour. They overlap like a Venn diagram, so every combination
/// of them is somewhere on screen.
pub(super) const PANES: [(Vec3, [f32; 3]); 3] = [
    (vec3(-0.6, 0.35, 0.1), [1.0, 0.1, 0.1]),
    (vec3(0.6, 0.35, 0.0), [0.1, 1.0, 0.1]),
    (vec3(0.0, -0.7, -0.1), [0.1, 0.1, 1.0]),
];
/// How much of what is behind a pane it hides.
pub(super) const PANE_ALPHA: f32 = 0.5;
/// Where (x, y) each set of panes shows with no other: the near one alone,
/// the middle one alone, the far one alone, near and middle, near and far,
/// middle and far, and all three.
pub(super) const PANE_SPOTS: [(&[usize], (f32, f32)); 7] = [
    (&[0], (-1.3, 0.95)),
    (&[1], (1.3, 0.95)),
    (&[2], (0.0, -1.35)),
    (&[0, 1], (0.0, 0.85)),
    (&[0, 2], (-0.7, -0.15)),
    (&[1, 2], (0.7, -0.15)),
    (&[0, 1, 2], (0.0, -0.15)),
];

/// Transparency: three see-through panes of one mesh (2 x 2, facing the
/// camera), tinted red (near), green and blue (far) and lit evenly from
/// everywhere on black, spawned in `order` (indices into [`PANES`]).
pub(super) fn panes(stage: &mut Stage, order: [usize; 3]) {
    let glass = MaterialDesc {
        color: Color::rgba(1.0, 1.0, 1.0, PANE_ALPHA),
        roughness: 1.0,
        blend: Blend::Transparent,
        // One face each way: a double-sided box would blend twice.
        double_sided: false,
        ..Default::default()
    };
    let pane = stage.mesh(&MeshData::cube(1.0), glass);
    stage.ctx.scene3d.lighting.sun_intensity = 0.0;
    stage.ctx.scene3d.lighting.ambient_intensity = 1.0;
    stage.ctx.scene3d.camera.position = vec3(0.0, -0.2, 5.5);
    stage.ctx.scene3d.camera.look_at(vec3(0.0, -0.2, 0.0));
    for i in order {
        let (position, [r, g, b]) = PANES[i];
        stage.ctx.world.spawn((
            Transform3D {
                position,
                scale: vec3(2.0, 2.0, 0.02),
                ..Default::default()
            },
            Model::new(pane).with_color(Color::rgb(r, g, b)),
        ));
    }
}

/// A cutout shader: round holes in a grid, `HOLES` across the plane.
pub(super) const HOLES_SHADER: &str = "fn surface(s: Surface) -> Surface {
    var o = s;
    let cell = fract(s.uv * 3.0) - vec2<f32>(0.5);
    o.alpha = select(1.0, 0.0, length(cell) < 0.3);
    return o;
}";

/// The same sheet with stripes cut out instead: what a hot reload of
/// [`HOLES_SHADER`] changes it to.
pub(super) const STRIPES_SHADER: &str = "fn surface(s: Surface) -> Surface {
    var o = s;
    o.alpha = select(1.0, 0.0, fract(s.uv.x * 3.0) < 0.5);
    return o;
}";

/// A point on [`cutout`]'s sheet that [`HOLES_SHADER`] leaves solid and
/// [`STRIPES_SHADER`] cuts away.
pub(super) const STRIPE_NOT_HOLE: Vec3 = vec3(-1.75, 1.0, 0.5);

/// Alpha cutout: a yellow sheet with round holes hanging level over the
/// grey floor, the sun straight above casting its shadow, seen from above
/// at an angle. Returns the shader's index (for a reload) and four world
/// points: the middle of a hole and the solid sheet beside it (both on the
/// sheet), and the floor under each (the hole's sunlit spot, the sheet's
/// shadow).
pub(super) fn cutout(stage: &mut Stage, source: &str) -> (u32, [Vec3; 4]) {
    stage.floor();
    let (shader, id) = stage.shader("holes.wgsl", source);
    let sheet = stage.mesh(
        &MeshData::plane(1.0),
        MaterialDesc {
            color: Color::rgb(0.9, 0.8, 0.1),
            blend: Blend::Cutout,
            shader: Some(shader),
            double_sided: true,
            ..Default::default()
        },
    );
    // 3 x 3 on the ground plane, 1 up: its uv cells are 1 x 1.
    stage.ctx.world.spawn((
        Transform3D {
            position: vec3(-1.5, 1.0, 0.0),
            scale: Vec3::splat(3.0),
            ..Default::default()
        },
        Model::new(sheet),
    ));
    let lighting = &mut stage.ctx.scene3d.lighting;
    lighting.sun_direction = vec3(1.0, -1.0, 0.0);
    lighting.shadows = true;
    stage.ctx.scene3d.camera.position = vec3(-0.5, 5.5, 3.5);
    stage.ctx.scene3d.camera.look_at(vec3(-0.9, 0.3, 0.0));
    // The middle cell's hole is at the sheet's centre; between two holes
    // the sheet is solid. The sun comes down at 45 degrees towards +x: the
    // shadow lies 1 further along x than the sheet, so the holes of the
    // sheet's last column (x -0.5) light the floor at x 0.5, clear of it.
    let (hole, solid) = (vec3(-1.5, 1.0, 0.0), vec3(-1.0, 1.0, 0.5));
    (id, [hole, solid, vec3(0.5, 0.0, 0.0), vec3(0.5, 0.0, 0.5)])
}

/// Shadows: a white box on the grey floor with the sun low from the left,
/// so its shadow falls on the floor to its right. Returns the box's entity
/// and two floor points: in its shadow, and in the sun.
pub(super) fn shadowed(stage: &mut Stage) -> (Entity, Vec3, Vec3) {
    stage.floor();
    let white = stage.cube(Color::WHITE);
    let entity = stage.block(white, vec3(-1.5, 0.0, -0.5), vec3(-0.5, 1.5, 0.5));
    let lighting = &mut stage.ctx.scene3d.lighting;
    lighting.sun_direction = vec3(1.0, -1.0, 0.0);
    lighting.shadows = true;
    (entity, vec3(0.3, 0.0, 0.0), vec3(0.3, 0.0, 2.0))
}

/// Skinned meshes: an orange bar 2 tall standing on the floor, its upper
/// half on a second joint, lit and casting a shadow. Returns its entity;
/// [`bend`] poses it.
pub(super) fn skinned(stage: &mut Stage) -> Entity {
    stage.floor();
    let skeleton = Skeleton::new(vec![
        Joint {
            name: "base".into(),
            parent: None,
            rest: Transform3D::default(),
        },
        Joint {
            name: "top".into(),
            parent: Some(0),
            rest: Transform3D::at(vec3(0.0, 1.0, 0.0)),
        },
    ]);
    // Two boxes on top of each other, each wholly on its joint, so the
    // bar bends at the joint and not along its whole length.
    let half = MeshData::cube(1.0);
    let mut bar = MeshData::default();
    for joint in 0..2u16 {
        let first = bar.positions.len() as u32;
        let lift = f32::from(joint);
        bar.positions.extend(
            half.positions
                .iter()
                .map(|p| vec3(p.x * 0.4, p.y + 0.5 + lift, p.z * 0.4)),
        );
        bar.normals.extend(&half.normals);
        bar.uvs.extend(&half.uvs);
        bar.indices.extend(half.indices.iter().map(|i| i + first));
        bar.joints.extend(half.positions.iter().map(|_| [joint, 0, 0, 0]));
    }
    bar.weights = vec![[1.0, 0.0, 0.0, 0.0]; bar.positions.len()];
    let desc = MaterialDesc::color(Color::rgb(0.9, 0.35, 0.1));
    let material = stage.ctx.gfx.create_material(desc);
    let mesh = stage.ctx.gfx.create_skinned_mesh(&bar, material, &skeleton);
    stage
        .renderer
        .create_material(&stage.device, &GpuMaterialDesc::new(&desc), &stage.textures);
    stage
        .renderer
        .create_mesh(&stage.device, &bar, &[(0..bar.indices.len() as u32, material)], true);
    let lighting = &mut stage.ctx.scene3d.lighting;
    lighting.sun_direction = vec3(1.0, -1.0, 0.0);
    lighting.shadows = true;
    stage
        .ctx
        .world
        .spawn((Transform3D::at(vec3(-0.5, 0.0, 0.0)), Model::new(mesh), skeleton))
}

/// Turns the bar's upper half `angle` about the view axis (positive: to
/// the left).
pub(super) fn bend(stage: &mut Stage, bar: Entity, angle: f32) {
    stage.ctx.world.get_mut::<Skeleton>(bar).unwrap().pose[1].rotation = Quat::from_rotation_z(angle);
}

/// Screen-space reflections: a mirror floor with a glowing red box over
/// it, nothing else lit. Returns the floor points that reflect the box,
/// and one to the side that reflects only the dark.
pub(super) fn mirror(stage: &mut Stage) -> (Vec3, Vec3) {
    let floor = stage.mesh(
        &MeshData::cube(1.0),
        MaterialDesc {
            color: Color::WHITE,
            roughness: 0.0,
            metallic: 1.0,
            ..Default::default()
        },
    );
    let glow = stage.mesh(
        &MeshData::cube(1.0),
        MaterialDesc {
            color: Color::BLACK,
            emissive: Color::rgb(4.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    stage.block(floor, vec3(-10.0, -1.0, -10.0), vec3(10.0, 0.0, 10.0));
    stage.block(glow, vec3(-0.5, 0.5, -0.5), vec3(0.5, 1.5, 0.5));
    stage.ctx.scene3d.lighting.sun_intensity = 0.0;
    stage.ctx.scene3d.lighting.ambient_intensity = 0.0;
    stage.ctx.scene3d.camera.position = vec3(0.0, 2.0, 6.0);
    stage.ctx.scene3d.camera.look_at(vec3(0.0, 0.5, 0.0));
    // The box's centre mirrored is (0, -1, 0): seen through the floor at
    // (0, 0, 2).
    (vec3(0.0, 0.0, 2.0), vec3(2.5, 0.0, 2.0))
}

/// Ambient occlusion: a white floor and a white wall meeting in a crease,
/// lit only by the ambient light, so everything is one flat shade without
/// it. Returns a floor point in the crease and one out in the open.
pub(super) fn crease(stage: &mut Stage) -> (Vec3, Vec3) {
    let white = stage.cube(Color::WHITE);
    stage.block(white, vec3(-10.0, -1.0, -10.0), vec3(10.0, 0.0, 10.0));
    stage.block(white, vec3(-10.0, 0.0, -3.0), vec3(10.0, 6.0, -1.0));
    let lighting = &mut stage.ctx.scene3d.lighting;
    lighting.sun_intensity = 0.0;
    lighting.ambient_intensity = 0.6;
    stage.ctx.scene3d.camera.position = vec3(0.0, 2.5, 6.0);
    stage.ctx.scene3d.camera.look_at(vec3(0.0, 0.5, -1.0));
    (vec3(0.0, 0.0, -0.85), vec3(0.0, 0.0, 2.5))
}

/// Global illumination: a closed white room whose west wall is red, the
/// sun shining in through an opening in the roof onto that wall (not the
/// floor), the camera inside looking west. Returns floor points by the red
/// wall and further from it, and a point on the sunlit wall.
pub(super) fn red_room(stage: &mut Stage, gi: bool) -> (Vec3, Vec3, Vec3) {
    let white = stage.cube(Color::WHITE);
    let red = stage.cube(Color::rgb(0.9, 0.04, 0.04));
    let (inner, outer) = (3.5, 3.9);
    stage.block(white, vec3(-outer, -0.9, -outer), vec3(outer, -0.5, outer)); // floor
    stage.block(red, vec3(-outer, -0.5, -outer), vec3(-inner, 3.5, outer)); // west
    stage.block(white, vec3(inner, -0.5, -outer), vec3(outer, 3.5, outer)); // east
    stage.block(white, vec3(-inner, -0.5, -outer), vec3(inner, 3.5, -inner)); // north
    stage.block(white, vec3(-inner, -0.5, inner), vec3(inner, 3.5, outer)); // south
    // The roof, with an opening from x -2 to 0 and z -1.5 to 1.5.
    stage.block(white, vec3(-outer, 3.5, -outer), vec3(-2.0, 3.9, outer));
    stage.block(white, vec3(0.0, 3.5, -outer), vec3(outer, 3.9, outer));
    stage.block(white, vec3(-2.0, 3.5, -outer), vec3(0.0, 3.9, -1.5));
    stage.block(white, vec3(-2.0, 3.5, 1.5), vec3(0.0, 3.9, outer));
    let lighting = &mut stage.ctx.scene3d.lighting;
    lighting.sun_direction = vec3(-1.0, -1.0, 0.0);
    lighting.sun_intensity = 4.0;
    stage.ctx.scene3d.sky.sky_color = Color::rgb(0.5, 0.5, 0.5);
    stage.ctx.scene3d.sky.ground_color = Color::rgb(0.5, 0.5, 0.5);
    lighting.ambient_intensity = 0.3;
    lighting.shadows = true;
    lighting.gi = crate::render::GlobalIllumination {
        enabled: gi,
        extent: vec3(12.0, 4.0, 8.0),
        spacing: 1.0,
        probes_per_frame: crate::render::GlobalIllumination::MAX_PROBES_PER_FRAME,
        intensity: 1.0,
    };
    stage.ctx.scene3d.camera.position = vec3(2.0, 1.3, 0.3);
    stage.ctx.scene3d.camera.look_at(vec3(-3.5, -0.2, 0.0));
    (vec3(-3.0, -0.5, -0.8), vec3(-0.5, -0.5, -0.8), vec3(-3.5, 1.0, 0.0))
}

/// Post-processing: three glowing bars in front of a black wall, left to
/// right dim (0.25), white (1.0) and far brighter than white (6.0) and
/// orange, with nothing lit. (The wall is there because the post chain
/// leaves the background's clear colour alone: a glow shows on models.)
/// Returns a point on each bar, and one on the wall between the last two.
pub(super) fn bars(stage: &mut Stage) -> ([Vec3; 3], Vec3) {
    stage.ctx.scene3d.lighting.sun_intensity = 0.0;
    stage.ctx.scene3d.lighting.ambient_intensity = 0.0;
    stage.ctx.scene3d.camera.position = vec3(0.0, 0.0, 8.0);
    stage.ctx.scene3d.camera.look_at(Vec3::ZERO);
    let glows = [Color::rgb(0.25, 0.25, 0.25), Color::WHITE, Color::rgb(6.0, 3.0, 0.6)];
    let wall = stage.mesh(
        &MeshData::cube(1.0),
        MaterialDesc {
            color: Color::BLACK,
            roughness: 1.0,
            ..Default::default()
        },
    );
    stage.block(wall, vec3(-8.0, -8.0, -1.2), vec3(8.0, 8.0, -1.0));
    let mut at = [Vec3::ZERO; 3];
    for (i, glow) in glows.into_iter().enumerate() {
        let mesh = stage.mesh(
            &MeshData::cube(1.0),
            MaterialDesc {
                color: Color::BLACK,
                emissive: glow,
                roughness: 1.0,
                ..Default::default()
            },
        );
        let x = i as f32 * 2.0 - 2.0;
        stage.block(mesh, vec3(x - 0.5, -1.5, -0.1), vec3(x + 0.5, 1.5, 0.1));
        at[i] = vec3(x, 0.0, 0.1);
    }
    (at, vec3(1.0, 0.0, -1.0))
}

/// Temporal antialiasing: a glowing white slab turned 0.37 rad about the
/// view axis on black (one long diagonal edge), and its twin a quarter
/// turn to the camera's left, placed so that a camera turned to face it
/// with [`face_twin`] sees exactly the same picture.
pub(super) fn edge(stage: &mut Stage) {
    let glow = stage.mesh(
        &MeshData::cube(1.0),
        MaterialDesc {
            color: Color::BLACK,
            emissive: Color::WHITE,
            roughness: 1.0,
            ..Default::default()
        },
    );
    stage.ctx.scene3d.lighting.sun_intensity = 0.0;
    stage.ctx.scene3d.lighting.ambient_intensity = 0.0;
    stage.ctx.scene3d.camera.position = vec3(0.0, 0.0, 5.0);
    stage.ctx.scene3d.camera.look_at(Vec3::ZERO);
    let quarter = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    for turn in [Quat::IDENTITY, quarter] {
        stage.ctx.world.spawn((
            Transform3D {
                // 5 in front of the camera, whichever way it faces.
                position: vec3(0.0, 0.0, 5.0) + turn * vec3(0.0, 0.0, -5.0),
                rotation: turn * Quat::from_rotation_z(0.37),
                scale: vec3(4.0, 1.5, 0.05),
            },
            Model::new(glow),
        ));
    }
}

/// Turns the camera of [`edge`] a quarter turn left, to face the twin.
pub(super) fn face_twin(stage: &mut Stage) {
    stage.ctx.scene3d.camera.look_at(vec3(-5.0, 0.0, 5.0));
}
