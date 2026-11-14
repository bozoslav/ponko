//! Offscreen static instance batches: the GPU-driven path (compute culling
//! and indirect draws) against the CPU path (cell culling and direct draws)
//! on the same scenes, the device limits each path validates, and what the
//! paths allocate per frame. The rig is shared with `static_batch_bench`.
use super::gpu::GpuTexture;
use super::gpu3d::*;
use super::render_test_support::{self as support, TestGpu};
use super::timer::GpuTimer;
use crate::d3::{Camera as Camera3D, PointLight as PointLight3D, SpotLight as SpotLight3D, Transform as Transform3D};
use crate::prelude::*;
use crate::render::traffic;
use crate::render::{
    AntiAliasing, GpuFeature, InstanceBatch, InstanceBatchOptions, InstanceLevel, MaterialDesc, Mesh, MeshData, Model,
};
use crate::three::occlusion::OcclusionGrid;
use crate::three::{Blend, Frame3D, GpuMaterialDesc, Scene3D, SkinFrame, SkyLight, TargetCamera};

/// How a [`BatchRig`]'s device is opened.
#[derive(Clone, Copy)]
pub(super) struct RigSetup {
    /// The target's size in pixels.
    pub size: (u32, u32),
    /// `false`: WebGL2's limits (no compute shaders or storage buffers).
    pub compute: bool,
    /// Ask for GPU timestamp queries where the adapter has them.
    pub timestamps: bool,
    /// Lowers limits below what the tier asks for.
    pub shrink: fn(&mut wgpu::Limits),
}

impl Default for RigSetup {
    fn default() -> Self {
        Self {
            size: (128, 128),
            compute: true,
            timestamps: false,
            shrink: |_| {},
        }
    }
}

/// What one frame cost: CPU times in milliseconds, and what was recorded.
#[derive(Clone, Copy, Default)]
pub(super) struct FrameCost {
    /// `Renderer3D::prepare`: culling on the CPU path, uniforms on both.
    pub prepare_ms: f64,
    /// `Renderer3D::draw`: recording every pass into command buffers.
    pub record_ms: f64,
    /// `Queue::submit`.
    pub submit_ms: f64,
    /// From before `prepare` until the GPU finished the frame.
    pub frame_ms: f64,
    /// State changes and draws of the model passes.
    pub changes: StateChanges,
}

/// An offscreen 3D renderer of any size on a device of a chosen tier, with
/// a `Ctx` to fill with instance batches.
pub(super) struct BatchRig {
    pub gpu: TestGpu,
    output: wgpu::Texture,
    target: wgpu::TextureView,
    textures: Vec<GpuTexture>,
    pub renderer: Renderer3D,
    pub ctx: Ctx,
    frame: Frame3D,
    size: (u32, u32),
    /// GPU pass times, where the device has timestamp queries.
    pub timer: Option<GpuTimer>,
    /// The depth grid the next frames cull against (as a game's readback
    /// would deliver it), if any.
    pub grid: Option<OcclusionGrid>,
    frames: u32,
    /// Hand instances to the engine in the order given on every path (the
    /// benchmark measures what a game gives it). See `ordered`.
    pub keep_order: bool,
    /// Preparing the frame last drawn, as it was drawn: [allocations of
    /// the whole `Renderer3D::prepare`, times the batch preparation ran,
    /// allocations the batch preparation made].
    pub prepared_allocations: [usize; 3],
}

impl BatchRig {
    /// `None` without a GPU (logged as a skip).
    pub fn new(setup: RigSetup) -> Option<Self> {
        let gpu = support::gpu_with(|adapter| {
            let mut limits = if setup.compute {
                wgpu::Limits::default()
            } else {
                wgpu::Limits::downlevel_webgl2_defaults()
            };
            (setup.shrink)(&mut limits);
            let timestamps = setup.timestamps && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
            Ok(wgpu::DeviceDescriptor {
                required_limits: limits,
                required_features: if timestamps {
                    wgpu::Features::TIMESTAMP_QUERY
                } else {
                    wgpu::Features::empty()
                },
                ..Default::default()
            })
        })?;
        let (layout, white, textures) = support::white_textures(&gpu.device, &gpu.queue);
        let output = support::target(&gpu.device, setup.size, wgpu::TextureFormat::Rgba8UnormSrgb);
        let target = output.create_view(&Default::default());
        let renderer = Renderer3D::new(&gpu.device, &layout, output.format(), 0, &white, 4, true);
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(setup.size.0 as f32, setup.size.1 as f32));
        ctx.scene3d.sky.atmosphere = false;
        ctx.scene3d.lighting.shadows = false;
        // What every tier's textures hold (a game's renderer caps it too).
        ctx.scene3d.lighting.light_shadow_atlas = 2048;
        let timer = GpuTimer::new(&gpu.device, &gpu.queue);
        Some(Self {
            gpu,
            output,
            target,
            textures,
            renderer,
            ctx,
            frame: Frame3D::default(),
            size: setup.size,
            timer,
            grid: None,
            frames: 0,
            keep_order: false,
            prepared_allocations: [0; 3],
        })
    }

    /// Whether the device culls batches in a compute pass.
    pub fn compute_culling(&self) -> bool {
        super::capabilities::GpuLimits::of(&self.gpu.device).compute_culling()
    }

    /// A mesh of `data` with `desc`, on the CPU and the GPU.
    pub fn mesh(&mut self, data: &MeshData, desc: MaterialDesc) -> Mesh {
        let material = self.ctx.gfx.create_material(desc);
        let mesh = self.ctx.gfx.create_mesh(data, material);
        self.renderer
            .create_material(&self.gpu.device, &GpuMaterialDesc::new(&desc), &self.textures);
        self.renderer.create_mesh(
            &self.gpu.device,
            data,
            &[(0..data.indices.len() as u32, material)],
            true,
        );
        mesh
    }

    /// A mesh whose first and second half of triangles draw with two materials.
    pub fn mesh_halves(&mut self, data: &MeshData, first: MaterialDesc, second: MaterialDesc) -> Mesh {
        let a = self.ctx.gfx.create_material(first);
        let b = self.ctx.gfx.create_material(second);
        let mesh = self.ctx.gfx.create_mesh(data, a);
        for desc in [first, second] {
            self.renderer
                .create_material(&self.gpu.device, &GpuMaterialDesc::new(&desc), &self.textures);
        }
        let count = data.indices.len() as u32;
        let half = count / 6 * 3;
        self.renderer
            .create_mesh(&self.gpu.device, data, &[(0..half, a), (half..count, b)], true);
        mesh
    }

    /// Hands the batch the game made to the renderer, as `Graphics` does.
    fn upload(&mut self, batch: InstanceBatch) -> InstanceBatch {
        self.renderer.set_instance_batch(
            &self.gpu.device,
            &self.gpu.queue,
            batch.id,
            self.ctx.gfx.assets3d.instance_batches[batch.id as usize].as_ref(),
        );
        batch
    }

    /// `items` in the order this rig hands instances to the engine: as
    /// given, or last to first on a rig forced to the CPU path.
    ///
    /// The two paths draw a batch's instances in different orders (the
    /// compute pass in whatever order the device runs it), so a picture
    /// must not depend on the order. It does where two instances meet at
    /// the same depth: the last one drawn wins the sample. Which device
    /// shows that is luck (one scene's coplanar, overlapping cubes passed
    /// on Metal and failed on a software Direct3D 12 device), so every
    /// comparison of the paths also turns the order around: a scene with
    /// such ties then fails on any device.
    fn ordered<T: Copy>(&self, items: &[T]) -> Vec<T> {
        let mut items = items.to_vec();
        if self.renderer.batching.path == BatchPath::Cpu && !self.keep_order {
            items.reverse();
        }
        items
    }

    /// A batch of `mesh` at `transforms`.
    pub fn batch(&mut self, mesh: Mesh, transforms: &[Mat4], options: InstanceBatchOptions) -> InstanceBatch {
        let transforms = self.ordered(transforms);
        let batch = self.ctx.gfx.create_instance_batch(mesh, &transforms, options);
        self.upload(batch)
    }

    /// A batch of distance `levels` at `transforms`.
    pub fn batch_levels(
        &mut self,
        levels: &[InstanceLevel],
        transforms: &[Mat4],
        colors: &[Color],
        options: InstanceBatchOptions,
    ) -> InstanceBatch {
        let (transforms, colors) = (self.ordered(transforms), self.ordered(colors));
        let batch = self
            .ctx
            .gfx
            .create_instance_batch_levels(levels, &transforms, &colors, options);
        self.upload(batch)
    }

    /// Replaces `batch`'s transforms, as a game's update does.
    pub fn update(&mut self, batch: InstanceBatch, transforms: &[Mat4]) {
        let transforms = self.ordered(transforms);
        assert!(self.ctx.gfx.update_instance_batch(batch, &transforms));
        self.upload(batch);
    }

    /// A render target of the rig's size that `camera` draws into every
    /// frame from now on.
    pub fn target_camera(&mut self, camera: Camera3D) -> wgpu::Texture {
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("batch rig target"),
            size: self.output.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // The test owns the offscreen renderer separately from the context.
        self.ctx.set_headless(true);
        let handle = self.ctx.gfx.create_render_target(self.size.0, self.size.1);
        self.renderer.add_render_target(handle.id, &texture);
        self.ctx
            .scene3d
            .passes
            .target_cameras
            .push(TargetCamera::new(camera, handle));
        texture
    }

    /// Frees `batch` on both sides.
    pub fn free(&mut self, batch: InstanceBatch) {
        self.ctx.gfx.free(batch);
        self.renderer
            .set_instance_batch(&self.gpu.device, &self.gpu.queue, batch.id, None);
    }

    /// Collects and prepares the frame (everything before recording).
    fn prepare(&mut self) -> Frame3D {
        let ctx = &mut self.ctx;
        let mut frame = std::mem::take(&mut self.frame);
        frame.lod = ctx.scene3d.lod;
        frame.occlusion = self.grid.take();
        frame.motion.enabled = ctx.scene3d.post.anti_aliasing == AntiAliasing::Taa;
        frame.collect(
            &mut ctx.world,
            &ctx.gfx.assets3d,
            &SkinFrame::default(),
            &ctx.scene3d.camera,
            &ctx.scene3d.lighting,
        );
        ctx.gfx.frame3d.views = std::mem::take(&mut frame.views);
        ctx.collect_views(&SkinFrame::default());
        frame.views = std::mem::take(&mut ctx.gfx.frame3d.views);
        let time = self.frames as f32 / 60.0;
        self.frames += 1;
        let scene = Scene3D {
            frame: &frame,
            lighting: &ctx.scene3d.lighting,
            sky: &ctx.scene3d.sky,
            light: SkyLight {
                sun: Vec3::splat(ctx.scene3d.lighting.sun_intensity),
                sky: Vec3::ZERO,
                ground: Vec3::ZERO,
            },
            camera: ctx.scene3d.camera.position,
            post: &ctx.scene3d.post,
            time: (time, 1.0 / 60.0),
            occlusion: ctx.scene3d.camera.occlusion_culling,
        };
        let (renderer, gpu, size) = (&mut self.renderer, &self.gpu, self.size);
        self.prepared_allocations = crate::three::lights::tests::count_marked_allocations(|| {
            renderer.prepare(
                &gpu.device,
                &gpu.queue,
                &Prepare3D {
                    scene: &scene,
                    lut: None,
                    size,
                },
            )
        })
        .1;
        frame
    }

    /// Draws one frame, waits for the GPU and says what it cost.
    pub fn frame(&mut self) -> FrameCost {
        let (device, queue) = (self.gpu.device.clone(), self.gpu.queue.clone());
        if let Some(timer) = &mut self.timer {
            timer.collect(&device);
            timer.begin_frame();
        }
        let start = std::time::Instant::now();
        let mut frame = self.prepare();
        let prepared = std::time::Instant::now();
        let mut buffers = self.renderer.draw(
            &device,
            &self.target,
            wgpu::Color::BLACK,
            &self.textures,
            self.timer.as_ref(),
        );
        if let Some(timer) = &mut self.timer {
            let mut encoder = device.create_command_encoder(&Default::default());
            timer.resolve(&mut encoder);
            buffers.push(encoder.finish());
        }
        let recorded = std::time::Instant::now();
        queue.submit(buffers);
        let submitted = std::time::Instant::now();
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let done = std::time::Instant::now();
        // The grid stays for the next frame, like the newest readback.
        self.grid = frame.occlusion.take();
        self.frame = frame;
        let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1e3;
        FrameCost {
            prepare_ms: ms(start, prepared),
            record_ms: ms(prepared, recorded),
            submit_ms: ms(recorded, submitted),
            frame_ms: ms(start, done),
            changes: self.renderer.take_state_changes(),
        }
    }

    /// Prepares the batches of the frame last drawn once more, and returns
    /// how often that allocated.
    pub fn batch_preparation_allocations(&mut self) -> usize {
        self.frame.occlusion = self.grid.take();
        let (camera, occlusion) = (
            self.ctx.scene3d.camera.position,
            self.ctx.scene3d.camera.occlusion_culling,
        );
        let (renderer, gpu, frame) = (&mut self.renderer, &self.gpu, &self.frame);
        let count = crate::three::lights::tests::count_allocations(|| {
            renderer.prepare_batches(&gpu.device, &gpu.queue, frame, camera, occlusion)
        })
        .1;
        self.grid = self.frame.occlusion.take();
        count
    }

    /// Records the frame last prepared once more, on this thread alone:
    /// [allocations, times the batch drawing code ran, allocations the
    /// batch drawing code made itself].
    pub fn recording_allocations(&mut self, pool: &rayon::ThreadPool) -> [usize; 3] {
        pool.install(|| {
            crate::three::lights::tests::count_marked_allocations(|| {
                self.renderer
                    .draw(&self.gpu.device, &self.target, wgpu::Color::BLACK, &self.textures, None)
            })
            .1
        })
    }

    /// Draws one frame and reads it back.
    pub fn image(&mut self) -> support::Image {
        self.frame();
        support::Image::read(&self.gpu.device, &self.gpu.queue, &self.output)
    }
}

/// A material that glows `color` whatever the light.
fn glow(color: Color) -> MaterialDesc {
    MaterialDesc {
        color: Color::BLACK,
        emissive: color,
        ..Default::default()
    }
}

/// The wall of [`field`]: a thin box across the middle of the field.
const WALL: (Vec3, Vec3) = (Vec3::new(-7.0, 0.0, -10.2), Vec3::new(7.0, 5.0, -9.8));

/// The batches of [`field`], for the tests that look at them.
struct Field {
    cubes: InstanceBatch,
    levels: InstanceBatch,
    halves: InstanceBatch,
}

/// A field that takes every culling decision: red cubes with a maximum
/// distance (one mesh), a batch of three coloured distance levels whose
/// first two bands overlap, a batch of a two-material mesh, and a wall
/// (a model) that hides what is behind it.
fn field(rig: &mut BatchRig) -> Field {
    let cube = rig.mesh(&MeshData::cube(0.4), glow(Color::RED));
    let grid = |step: f32, shift: Vec3| -> Vec<Mat4> {
        let mut out = Vec::new();
        let mut z = 0.0;
        while z > -44.0 {
            let mut x = -22.0;
            while x < 22.0 {
                out.push(Mat4::from_translation(vec3(x, 0.2, z) + shift));
                x += step;
            }
            z -= step;
        }
        out
    };
    let cubes = rig.batch(
        cube,
        &grid(1.0, Vec3::ZERO),
        InstanceBatchOptions {
            cell_size: 4.0,
            max_distance: 26.0,
            ..Default::default()
        },
    );
    let colors = [Color::GREEN, Color::BLUE, Color::YELLOW];
    let bands = [(0.0, 12.0), (10.0, 24.0), (24.0, 38.0)];
    let levels: Vec<InstanceLevel> = colors
        .iter()
        .zip(bands)
        .enumerate()
        .map(|(i, (&color, (min_distance, max_distance)))| InstanceLevel {
            // Each level a little taller, so an overlap shows both.
            mesh: rig.mesh(&MeshData::cube(0.3 + 0.1 * i as f32), glow(color)),
            min_distance,
            max_distance,
            camera: true,
            shadows: true,
        })
        .collect();
    let levels = rig.batch_levels(
        &levels,
        &grid(2.0, vec3(0.5, 0.9, -0.5)),
        &[],
        InstanceBatchOptions {
            cell_size: 6.0,
            ..Default::default()
        },
    );
    let two = rig.mesh_halves(
        &MeshData::sphere(0.35, 8),
        glow(Color::WHITE),
        glow(Color::rgb(1.0, 0.0, 1.0)),
    );
    let halves = rig.batch(
        two,
        &grid(4.0, vec3(0.25, 1.8, -0.25)),
        InstanceBatchOptions {
            cell_size: 8.0,
            ..Default::default()
        },
    );
    let wall = rig.mesh(&MeshData::cube(1.0), glow(Color::GRAY));
    let mut at = Transform3D::at((WALL.0 + WALL.1) * 0.5);
    at.scale = WALL.1 - WALL.0;
    rig.ctx.world.spawn((at, Model::new(wall)));
    rig.ctx.scene3d.camera.far = 200.0;
    Field { cubes, levels, halves }
}

/// The camera of step `k` of a flight over [`field`]: it moves sideways
/// and ahead and turns, so every frame sees other instances than the last.
fn fly(rig: &mut BatchRig, k: usize) {
    let k = k as f32;
    rig.ctx.scene3d.camera.position = vec3(-5.0 + k * 0.9, 2.5 + 0.1 * k, 7.0 - k * 0.6);
    rig.ctx.scene3d.camera.look_at(vec3(-3.0 + k * 0.5, 1.0, -20.0));
}

/// The depth grid a readback of the frame drawn from `view_proj` would
/// give for the wall alone: the wall's depth where it covers a whole grid
/// cell, the far plane elsewhere.
fn wall_grid(view_proj: Mat4, camera: Vec3) -> OcclusionGrid {
    let (w, h) = (32u32, 32u32);
    let mut depth = vec![1.0f32; (w * h) as usize];
    // The face towards the camera (which stays in front of the wall).
    let z = WALL.1.z;
    let corners = [
        vec3(WALL.0.x, WALL.0.y, z),
        vec3(WALL.1.x, WALL.0.y, z),
        vec3(WALL.0.x, WALL.1.y, z),
        vec3(WALL.1.x, WALL.1.y, z),
    ]
    .map(|c| view_proj * c.extend(1.0));
    if corners.iter().all(|c| c.w > 1e-3) {
        let ndc = corners.map(|c| c.truncate() / c.w);
        // The rectangle inside all four edges, and the farthest depth.
        let left = ndc[0].x.max(ndc[2].x);
        let right = ndc[1].x.min(ndc[3].x);
        let bottom = ndc[0].y.max(ndc[1].y);
        let top = ndc[2].y.min(ndc[3].y);
        let far = ndc.iter().map(|c| c.z).fold(0.0, f32::max);
        for y in 0..h {
            for x in 0..w {
                let (x0, x1) = (x as f32 / w as f32 * 2.0 - 1.0, (x + 1) as f32 / w as f32 * 2.0 - 1.0);
                let (y1, y0) = (1.0 - y as f32 / h as f32 * 2.0, 1.0 - (y + 1) as f32 / h as f32 * 2.0);
                if x0 >= left && x1 <= right && y0 >= bottom && y1 <= top {
                    depth[(y * w + x) as usize] = far;
                }
            }
        }
    }
    OcclusionGrid::new(view_proj, camera, w, h, depth)
}

/// What the same-device comparisons allow: the paths draw the same
/// triangles with the same shaders, so only the order of draws differs.
const SAME_DEVICE: support::Tolerance = support::Tolerance {
    channel: 2,
    outliers: 0.0,
    edge: 0,
};

/// Asserts `a` and `b` are the same picture within `tolerance`, and that
/// the picture isn't empty (`lit`: the least fraction of non-black pixels).
fn assert_same(what: &str, a: &support::Image, b: &support::Image, tolerance: support::Tolerance, lit: f32) {
    let d = support::diff(a, b, tolerance.channel);
    eprintln!("{what}: {d}");
    if !d.within(tolerance) {
        // Where and what, for a device this can't be run on at a desk.
        let pixels = a.rgba.as_chunks::<4>().0.iter().zip(b.rgba.as_chunks::<4>().0);
        let off = pixels
            .enumerate()
            .filter(|(_, (p, q))| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > tolerance.channel));
        for (i, (p, q)) in off.take(40) {
            let (x, y) = (i % a.width as usize, i / a.width as usize);
            eprintln!("  {what}: pixel {x},{y}: {p:?} against {q:?}");
        }
    }
    assert!(d.within(tolerance), "{what}: {d}");
    let seen = a.fraction(0..a.width, 0..a.height, |p| p[0] > 40 || p[1] > 40 || p[2] > 40);
    assert!(seen >= lit, "{what}: only {:.1}% of the picture is lit", seen * 100.0);
}

/// A rig on the path `path`, with [`field`] in it. `None` without a GPU
/// or without compute culling when `path` needs it.
fn field_rig(setup: RigSetup, path: BatchPath) -> Option<(BatchRig, Field)> {
    let mut rig = BatchRig::new(setup)?;
    if path == BatchPath::Gpu && !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
        return None;
    }
    rig.renderer.batching.path = path;
    let field = field(&mut rig);
    Some((rig, field))
}

#[test]
fn both_paths_draw_the_same_frames_as_the_camera_moves() {
    let Some((mut gpu, field)) = field_rig(RigSetup::default(), BatchPath::Gpu) else {
        return;
    };
    let Some((mut cpu, cpu_field)) = field_rig(RigSetup::default(), BatchPath::Cpu) else {
        return;
    };
    for id in [field.cubes, field.levels, field.halves] {
        assert_eq!(gpu.renderer.batch_route(id.id), Some(Route::Gpu));
    }
    for id in [cpu_field.cubes, cpu_field.levels, cpu_field.halves] {
        assert_eq!(cpu.renderer.batch_route(id.id), Some(Route::CpuSmall));
    }
    let mut frames = Vec::new();
    for k in 0..14 {
        fly(&mut gpu, k);
        fly(&mut cpu, k);
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("frame {k}"), &a, &b, SAME_DEVICE, 0.05);
        frames.push(a);
    }
    // The flight does change the picture: the comparison isn't of one frame.
    let moved = support::diff(&frames[0], &frames[13], 8);
    assert!(moved.outliers > moved.pixels / 50, "the camera's flight shows: {moved}");
    // And every level is in it somewhere: red cubes, the three level
    // colours, and both materials of the two-material mesh.
    let all = |test: fn([u8; 4]) -> bool| {
        frames
            .iter()
            .any(|f| f.fraction(0..f.width, 0..f.height, test) > 0.0005)
    };
    assert!(all(|p| p[0] > 200 && p[1] < 60 && p[2] < 60), "red cubes");
    assert!(all(|p| p[1] > 200 && p[0] < 60 && p[2] < 60), "the near level");
    assert!(all(|p| p[2] > 200 && p[0] < 60 && p[1] < 60), "the middle level");
    assert!(all(|p| p[0] > 200 && p[1] > 200 && p[2] < 60), "the far level");
    assert!(all(|p| p[0] > 200 && p[1] < 60 && p[2] > 200), "the second material");
    assert!(all(|p| p[0] > 200 && p[1] > 200 && p[2] > 200), "the first material");
}

#[test]
fn late_occlusion_culling_pops_nothing_on_either_path() {
    // Every frame culls against the depth of the frame before, from where
    // the camera was then. Whatever that hides must be behind the wall
    // now too: the picture is the one drawn with no occlusion culling.
    let Some((mut reference, _)) = field_rig(RigSetup::default(), BatchPath::Cpu) else {
        return;
    };
    for path in [BatchPath::Gpu, BatchPath::Cpu] {
        let Some((mut rig, _)) = field_rig(RigSetup::default(), path) else {
            continue;
        };
        rig.ctx.scene3d.camera.occlusion_culling = true;
        let mut hidden = 0;
        for k in 0..14 {
            if k > 0 {
                fly(&mut rig, k - 1);
                rig.grid = Some(wall_grid(
                    rig.ctx.scene3d.camera.view_proj(),
                    rig.ctx.scene3d.camera.position,
                ));
            }
            fly(&mut rig, k);
            fly(&mut reference, k);
            let (culled, all) = (rig.image(), reference.image());
            assert_same(&format!("{path:?} frame {k}"), &culled, &all, SAME_DEVICE, 0.05);
            // Negative control: the grid does hide instances (on the CPU
            // path, where the draws can be counted).
            if path == BatchPath::Cpu && k > 0 {
                let drawn = |r: &BatchRig| -> u32 {
                    r.renderer
                        .static_batches
                        .iter()
                        .flatten()
                        .map(|b| {
                            b.visible[..b.levels.len()]
                                .iter()
                                .flatten()
                                .map(|run| run.1)
                                .sum::<u32>()
                        })
                        .sum()
                };
                hidden += drawn(&reference) - drawn(&rig);
            }
        }
        if path == BatchPath::Cpu {
            assert!(hidden > 500, "the wall's depth hid {hidden} instances over the flight");
        }
    }
}

#[test]
fn both_paths_cast_the_same_sun_shadows_as_the_camera_moves() {
    let rigs = [BatchPath::Gpu, BatchPath::Cpu].map(|path| {
        let (mut rig, _) = field_rig(RigSetup::default(), path)?;
        // A lit ground under the field, so the batches' shadows show.
        let ground = rig.mesh(
            &MeshData::plane(120.0),
            MaterialDesc {
                color: Color::WHITE,
                ..Default::default()
            },
        );
        rig.ctx
            .world
            .spawn((Transform3D::at(vec3(0.0, 0.0, -20.0)), Model::new(ground)));
        // Slabs floating over it and beside it: their shadows are large
        // and fall on open ground.
        let slab = rig.mesh(&MeshData::cube(1.0), glow(Color::rgb(0.0, 1.0, 1.0)));
        let slabs: Vec<Mat4> = (0..40)
            .map(|i| {
                let at = vec3((i % 8) as f32 * 7.0 - 26.0, 4.0, -((i / 8) as f32) * 8.0 - 2.0);
                Mat4::from_translation(at) * Mat4::from_scale(vec3(3.0, 0.3, 3.0))
            })
            .collect();
        rig.batch(slab, &slabs, Default::default());
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.lighting.shadow_distance = 60.0;
        Some(rig)
    });
    let [Some(mut gpu), Some(mut cpu)] = rigs else { return };
    for k in 0..10 {
        fly(&mut gpu, k);
        fly(&mut cpu, k);
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("shadowed frame {k}"), &a, &b, SAME_DEVICE, 0.3);
    }
    // From above the shadows are in plain sight: the two paths agree, and
    // the same frame without shadows is brighter where they fall.
    for rig in [&mut gpu, &mut cpu] {
        rig.ctx.scene3d.camera.position = vec3(0.0, 45.0, -19.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -20.0));
    }
    let (a, b) = (gpu.image(), cpu.image());
    assert_same("shadows from above", &a, &b, SAME_DEVICE, 0.3);
    gpu.ctx.scene3d.lighting.shadows = false;
    let unshadowed = gpu.image();
    gpu.ctx.scene3d.lighting.shadows = true;
    let pixels = a.rgba.as_chunks::<4>().0.iter().zip(unshadowed.rgba.as_chunks::<4>().0);
    let darker = pixels
        .filter(|(lit, plain)| plain[0] > lit[0].saturating_add(24))
        .count();
    let shadowed = darker as f32 / (a.width * a.height) as f32;
    eprintln!("{:.1}% of the picture is in the batches' shadow", shadowed * 100.0);
    assert!(shadowed > 0.03, "shadows cover only {shadowed} of the picture");
    assert!(
        gpu.renderer.instance_batch_stats().cull_jobs > 3,
        "shadow views were culled too"
    );
}

/// The colours of a target camera's texture (16-bit floats), clamped to
/// 0..=255 like a displayed picture.
fn target_image(rig: &BatchRig, texture: &wgpu::Texture) -> support::Image {
    let bytes = support::read_texture(&rig.gpu.device, &rig.gpu.queue, texture);
    let half = |b: &[u8]| -> f32 {
        let bits = u16::from_le_bytes([b[0], b[1]]);
        let (exponent, mantissa) = (i32::from((bits >> 10) & 31), f32::from(bits & 1023) / 1024.0);
        let value = match exponent {
            0 => mantissa * 2f32.powi(-14),
            31 => f32::INFINITY,
            e => (1.0 + mantissa) * 2f32.powi(e - 15),
        };
        if bits & 0x8000 != 0 { -value } else { value }
    };
    let row = (texture.width() * 8).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let mut rgba = Vec::with_capacity((texture.width() * texture.height() * 4) as usize);
    for y in 0..texture.height() {
        let line = &bytes[(y * row) as usize..][..(texture.width() * 8) as usize];
        rgba.extend(
            line.as_chunks::<2>()
                .0
                .iter()
                .map(|c| (half(c).clamp(0.0, 1.0) * 255.0).round() as u8),
        );
    }
    support::Image {
        width: texture.width(),
        height: texture.height(),
        rgba,
    }
}

/// The two paths' rigs with the same scene made by `scene`, or `None`
/// without a GPU with compute culling.
fn both_paths(setup: RigSetup, mut scene: impl FnMut(&mut BatchRig)) -> Option<[BatchRig; 2]> {
    let mut gpu = BatchRig::new(setup)?;
    if !support::needs(&gpu.gpu.device, GpuFeature::ComputeCulling) {
        return None;
    }
    let mut cpu = BatchRig::new(setup)?;
    gpu.renderer.batching.path = BatchPath::Gpu;
    cpu.renderer.batching.path = BatchPath::Cpu;
    scene(&mut gpu);
    scene(&mut cpu);
    Some([gpu, cpu])
}

#[test]
fn both_paths_draw_the_same_into_a_target_camera() {
    let mut targets = Vec::new();
    let Some([mut gpu, mut cpu]) = both_paths(RigSetup::default(), |rig| {
        field(rig);
        // A second camera looks across the field from the side.
        let mut camera = rig.ctx.scene3d.camera;
        camera.position = vec3(-30.0, 9.0, -18.0);
        camera.look_at(vec3(0.0, 0.0, -20.0));
        camera.far = 200.0;
        targets.push(rig.target_camera(camera));
    }) else {
        return;
    };
    for k in 0..8 {
        fly(&mut gpu, k);
        fly(&mut cpu, k);
        // The target camera moves too.
        for rig in [&mut gpu, &mut cpu] {
            let camera = &mut rig.ctx.scene3d.passes.target_cameras[0].camera;
            camera.position = vec3(-30.0 + k as f32 * 1.5, 9.0, -18.0 - k as f32);
            camera.look_at(vec3(0.0, 0.0, -20.0));
        }
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("main camera, frame {k}"), &a, &b, SAME_DEVICE, 0.05);
        let (a, b) = (target_image(&gpu, &targets[0]), target_image(&cpu, &targets[1]));
        assert_same(&format!("target camera, frame {k}"), &a, &b, SAME_DEVICE, 0.1);
        gpu.renderer.check_batch_order(&gpu.gpu.device, &gpu.gpu.queue);
        for _ in 0..20 {
            gpu.frame();
            assert_eq!(
                target_image(&gpu, &targets[0]),
                a,
                "stationary target must be byte-identical"
            );
        }
        if let Ok(dir) = std::env::var("PONKO_BATCH_IMAGES") {
            std::fs::create_dir_all(&dir).unwrap();
            for (name, image) in [("gpu", &a), ("cpu", &b)] {
                image::save_buffer(
                    std::path::Path::new(&dir).join(format!("target-{k}-{name}.png")),
                    &image.rgba,
                    image.width,
                    image.height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
    assert!(
        gpu.renderer.instance_batch_stats().cull_jobs > 5,
        "the target camera's views were culled too"
    );
}

#[test]
fn both_paths_cast_the_same_local_light_shadows() {
    let mut lamps = Vec::new();
    let Some([mut gpu, mut cpu]) = both_paths(RigSetup::default(), |rig| {
        field(rig);
        ground(rig);
        // Slabs over open ground, a spot light and a point light above them.
        let slab = rig.mesh(&MeshData::cube(1.0), glow(Color::rgb(0.0, 1.0, 1.0)));
        let slabs: Vec<Mat4> = (0..24)
            .map(|i| {
                let at = vec3((i % 6) as f32 * 4.0 - 10.0, 2.5, 6.0 - (i / 6) as f32 * 4.0);
                Mat4::from_translation(at) * Mat4::from_scale(vec3(1.6, 0.2, 1.6))
            })
            .collect();
        rig.batch(slab, &slabs, Default::default());
        rig.ctx.scene3d.lighting.sun_intensity = 0.2;
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.lighting.light_shadow_views = 12;
        lamps.push(shadowed_lamp(rig, vec3(-4.0, 7.0, 2.0), 30.0));
        lamps.push(rig.ctx.world.spawn((
            Transform3D::at(vec3(6.0, 9.0, 0.0)).looking_at(vec3(5.0, 0.0, -2.0)),
            SpotLight3D {
                intensity: 500.0,
                range: 30.0,
                inner_angle: 0.6,
                outer_angle: 0.9,
                shadows: true,
                ..Default::default()
            },
        )));
    }) else {
        return;
    };
    for k in 0..8 {
        for rig in [&mut gpu, &mut cpu] {
            rig.ctx.scene3d.camera.position = vec3(-8.0 + 2.0 * k as f32, 14.0 + k as f32, 16.0 - k as f32);
            rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -2.0));
        }
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("local light shadows, frame {k}"), &a, &b, SAME_DEVICE, 0.2);
    }
    let views = gpu.renderer.static_batches.iter().flatten().map(|b| b.views.len());
    assert!(views.sum::<usize>() > 8, "the lights' views have lists");
    // The slabs do shadow the ground: without the lights' shadows the
    // same frame is brighter under them.
    let lit = gpu.image();
    gpu.ctx.world.get_mut::<PointLight3D>(lamps[0]).unwrap().shadows = false;
    gpu.ctx.world.get_mut::<SpotLight3D>(lamps[1]).unwrap().shadows = false;
    let plain = gpu.image();
    let pixels = lit.rgba.as_chunks::<4>().0.iter().zip(plain.rgba.as_chunks::<4>().0);
    let darker = pixels
        .filter(|(lit, plain)| plain[0] > lit[0].saturating_add(24))
        .count();
    let shadowed = darker as f32 / (lit.width * lit.height) as f32;
    eprintln!("{:.1}% of the picture is in the lights' shadows", shadowed * 100.0);
    assert!(
        shadowed > 0.02,
        "the lights' shadows cover only {shadowed} of the picture"
    );
}

#[test]
fn both_paths_draw_the_same_transparent_cutout_and_coloured_batches() {
    let Some([mut gpu, mut cpu]) = both_paths(RigSetup::default(), |rig| {
        ground(rig);
        let row = |z: f32, y: f32| -> Vec<Mat4> {
            (0..40)
                .map(|i| Mat4::from_translation(vec3(i as f32 * 1.2 - 24.0, y, z)))
                .collect()
        };
        // See-through tiles lying flat above the ground, so none is in
        // front of another: batches don't sort their instances, and tiles
        // one behind another would blend in whatever order a path draws.
        let pane = rig.mesh(
            &MeshData::cube(0.8),
            MaterialDesc {
                color: Color::rgba(1.0, 0.2, 0.2, 0.5),
                blend: Blend::Transparent,
                ..Default::default()
            },
        );
        let tiles: Vec<Mat4> = row(-6.0, 0.3)
            .into_iter()
            .map(|at| at * Mat4::from_scale(vec3(1.0, 0.05, 1.0)))
            .collect();
        rig.batch(pane, &tiles, Default::default());
        // Cutout cubes: the cutout pipelines and their shadows.
        let fence = rig.mesh(
            &MeshData::cube(0.8),
            MaterialDesc {
                color: Color::rgb(0.2, 1.0, 0.2),
                blend: Blend::Cutout,
                ..Default::default()
            },
        );
        rig.batch(fence, &row(-10.0, 2.0), Default::default());
        // A colour per instance, at two levels.
        let white = MaterialDesc {
            color: Color::WHITE,
            ..Default::default()
        };
        let levels = [(0.0, 22.0, 0.8), (22.0, 300.0, 0.5)].map(|(min_distance, max_distance, size)| InstanceLevel {
            mesh: rig.mesh(&MeshData::cube(size), white),
            min_distance,
            max_distance,
            camera: true,
            shadows: true,
        });
        let transforms: Vec<Mat4> = (0..120)
            .map(|i| Mat4::from_translation(vec3((i % 40) as f32 * 1.2 - 24.0, 1.0, -14.0 - (i / 40) as f32 * 2.0)))
            .collect();
        let colors: Vec<Color> = (0..120)
            .map(|i| [Color::RED, Color::BLUE, Color::YELLOW, Color::rgb(1.0, 0.0, 1.0)][i % 4])
            .collect();
        rig.batch_levels(&levels, &transforms, &colors, Default::default());
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.lighting.shadow_distance = 80.0;
        rig.ctx.scene3d.camera.far = 200.0;
    }) else {
        return;
    };
    for k in 0..8 {
        for rig in [&mut gpu, &mut cpu] {
            rig.ctx.scene3d.camera.position = vec3(-10.0 + 3.0 * k as f32, 5.0 + 0.5 * k as f32, 8.0 - 0.5 * k as f32);
            rig.ctx.scene3d.camera.look_at(vec3(-4.0 + k as f32, 1.0, -12.0));
        }
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("blend modes and colours, frame {k}"), &a, &b, SAME_DEVICE, 0.2);
        // Each kind is in the picture: blended red over the white ground,
        // green cutouts, and instances of more than one colour.
        let has = |what: fn(&[u8; 4]) -> bool| a.fraction(0..a.width, 0..a.height, |p| what(&[p[0], p[1], p[2], p[3]]));
        let panes = has(|p| p[0] > 150 && p[1] > 60 && p[1] < 200 && p[2] > 60 && p[2] < 200);
        let fences = has(|p| p[1] > 120 && p[0] < 100 && p[2] < 100);
        let blue = has(|p| p[2] > 120 && p[0] < 80 && p[1] < 80);
        let yellow = has(|p| p[0] > 120 && p[1] > 120 && p[2] < 80);
        assert!(
            panes > 0.002 && fences > 0.002 && blue > 0.0002 && yellow > 0.0002,
            "frame {k}: panes {panes}, cutouts {fences}, blue {blue}, yellow {yellow}"
        );
    }
}

#[test]
fn a_camera_cut_with_occlusion_culling_pops_nothing_on_either_path() {
    // The camera jumps to the other end of the field while the depth grid
    // is still the one seen from before the cut. Nothing that is in sight
    // after the cut may be culled by it: the picture is the one drawn
    // with no occlusion culling.
    let Some((mut reference, _)) = field_rig(RigSetup::default(), BatchPath::Cpu) else {
        return;
    };
    let cuts = [
        (vec3(18.0, 6.0, -34.0), vec3(-6.0, 1.0, -4.0)),
        (vec3(-3.0, 3.0, -16.0), vec3(-3.0, 1.0, -40.0)),
        (vec3(-20.0, 30.0, -20.0), vec3(0.0, 0.0, -20.0)),
    ];
    for path in [BatchPath::Gpu, BatchPath::Cpu] {
        let Some((mut rig, _)) = field_rig(RigSetup::default(), path) else {
            continue;
        };
        rig.ctx.scene3d.camera.occlusion_culling = true;
        for (k, (position, target)) in cuts.into_iter().enumerate() {
            // A few frames in front of the wall, each with the grid of
            // the frame before.
            for step in 0..3 {
                rig.grid = Some(wall_grid(
                    rig.ctx.scene3d.camera.view_proj(),
                    rig.ctx.scene3d.camera.position,
                ));
                fly(&mut rig, 3 + step);
                rig.frame();
            }
            // The cut: the grid is the last one from before it.
            rig.grid = Some(wall_grid(
                rig.ctx.scene3d.camera.view_proj(),
                rig.ctx.scene3d.camera.position,
            ));
            for r in [&mut rig, &mut reference] {
                r.ctx.scene3d.camera.position = position;
                r.ctx.scene3d.camera.look_at(target);
            }
            let (culled, all) = (rig.image(), reference.image());
            assert_same(&format!("{path:?} cut {k}"), &culled, &all, SAME_DEVICE, 0.05);
            // And the frame after it, with the same stale grid.
            let (culled, all) = (rig.image(), reference.image());
            assert_same(&format!("{path:?} after cut {k}"), &culled, &all, SAME_DEVICE, 0.05);
        }
    }
}

#[test]
fn a_view_at_rest_keeps_its_cpu_lists_and_a_move_or_an_update_builds_them_again() {
    let transforms = |shift: f32| -> Vec<Mat4> {
        (0..4096)
            .map(|i| Mat4::from_translation(vec3((i % 64) as f32 - 32.0 + shift, 0.2, -((i / 64) as f32))))
            .collect()
    };
    let options = InstanceBatchOptions {
        cell_size: 8.0,
        max_distance: 40.0,
        ..Default::default()
    };
    // A rig on the CPU path looking from `x`, with the batch at `shift`.
    let rig_at = |x: f32, shift: f32| -> Option<(BatchRig, InstanceBatch)> {
        let mut rig = BatchRig::new(RigSetup::default())?;
        rig.renderer.batching.path = BatchPath::Cpu;
        let cube = rig.mesh(&MeshData::cube(0.4), glow(Color::RED));
        let batch = rig.batch(cube, &transforms(shift), options);
        rig.ctx.scene3d.camera.occlusion_culling = false;
        rig.ctx.scene3d.camera.position = vec3(x, 6.0, 8.0);
        rig.ctx.scene3d.camera.look_at(vec3(x, 0.0, -20.0));
        Some((rig, batch))
    };
    let Some((mut rig, batch)) = rig_at(0.0, 0.0) else {
        return;
    };
    let first = rig.image();
    let built = rig.renderer.batch_lists_built();
    assert!(built > 0, "the first frame builds the camera's list");
    let second = rig.image();
    assert_eq!(
        rig.renderer.batch_lists_built(),
        built,
        "nothing moved, so no list is built again"
    );
    assert_same("at rest", &first, &second, SAME_DEVICE, 0.02);

    // The camera moves: the list is built again, and is the one a renderer
    // that never saw the old view builds.
    rig.ctx.scene3d.camera.position = vec3(14.3, 6.0, 8.0);
    rig.ctx.scene3d.camera.look_at(vec3(14.3, 0.0, -20.0));
    let moved = rig.image();
    let after_move = rig.renderer.batch_lists_built();
    assert!(after_move > built, "a moved view builds its list again");
    let (mut fresh, _) = rig_at(14.3, 0.0).unwrap();
    assert_same("after the move", &moved, &fresh.image(), SAME_DEVICE, 0.02);
    assert!(
        support::diff(&first, &moved, 2).outliers > 100,
        "the two views show different pictures"
    );

    // The batch changes under a camera at rest: its lists are built again.
    rig.update(batch, &transforms(0.5));
    let updated = rig.image();
    assert!(
        rig.renderer.batch_lists_built() > after_move,
        "an updated batch builds its list again"
    );
    let (mut fresh, _) = rig_at(14.3, 0.5).unwrap();
    assert_same("after the update", &updated, &fresh.image(), SAME_DEVICE, 0.02);
}

#[test]
fn the_webgl2_tier_draws_the_same_frames_as_the_gpu_driven_path() {
    // The device without compute shaders takes the CPU path by itself.
    let webgl2 = RigSetup {
        compute: false,
        ..Default::default()
    };
    let Some((mut low, field)) = field_rig(webgl2, BatchPath::Auto) else {
        return;
    };
    assert_eq!(low.renderer.batch_route(field.cubes.id), Some(Route::CpuNoCompute));
    assert_eq!(low.renderer.instance_batch_stats().cpu_no_compute, 3);
    assert_eq!(
        low.renderer.instance_batch_stats().pools,
        0,
        "no compute resources were made"
    );
    let Some((mut gpu, _)) = field_rig(RigSetup::default(), BatchPath::Gpu) else {
        return;
    };
    for k in 0..14 {
        fly(&mut low, k);
        fly(&mut gpu, k);
        let (a, b) = (low.image(), gpu.image());
        // Two devices of one adapter: the same rasteriser, other limits.
        assert_same(&format!("webgl2 limits, frame {k}"), &a, &b, SAME_DEVICE, 0.05);
    }
}

#[test]
fn the_path_follows_the_measured_crossover() {
    let Some(mut rig) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    let mesh = rig.mesh(&MeshData::cube(0.1), glow(Color::RED));
    let min = super::capabilities::GPU_BATCH_MIN as usize;
    let at = |n: usize| -> Vec<Mat4> {
        (0..n)
            .map(|i| Mat4::from_translation(vec3((i % 512) as f32 * 0.2 - 50.0, 0.0, -((i / 512) as f32) * 0.2)))
            .collect()
    };
    let small = rig.batch(mesh, &at(min - 1), Default::default());
    let large = rig.batch(mesh, &at(min), Default::default());
    let compute = rig.compute_culling();
    let expected = |gpu| match (compute, gpu) {
        (false, _) => Route::CpuNoCompute,
        (true, true) => Route::Gpu,
        (true, false) => Route::CpuSmall,
    };
    assert_eq!(rig.renderer.batch_route(small.id), Some(expected(false)));
    assert_eq!(rig.renderer.batch_route(large.id), Some(expected(true)));
    rig.ctx.scene3d.camera.position = vec3(0.0, 20.0, 20.0);
    rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -20.0));
    let image = rig.image();
    assert!(
        image.fraction(0..image.width, 0..image.height, |p| p[0] > 150) > 0.02,
        "both batches are drawn"
    );
    let stats = rig.renderer.instance_batch_stats();
    assert_eq!(stats.batches, 2);
    assert_eq!(stats.instances, 2 * min as u64 - 1);
    if compute {
        assert_eq!((stats.gpu_driven, stats.cpu_small), (1, 1));
        assert_eq!(stats.gpu_instances, min as u64);
        assert_eq!((stats.pools, stats.cull_dispatches, stats.cull_jobs), (1, 1, 1));
        assert!(stats.pool_bytes >= min as u64 * 96);
    } else {
        assert_eq!(stats.cpu_no_compute, 2);
    }
    // Freed batches leave the counts and give their pool ranges back.
    rig.free(large);
    rig.free(small);
    rig.frame();
    let stats = rig.renderer.instance_batch_stats();
    assert_eq!((stats.batches, stats.gpu_driven, stats.cull_jobs), (0, 0, 0));
    if compute {
        let (_, _, used) = rig.renderer.batch_pool_use();
        assert!(used < 64 << 10, "only the table is left in the pool: {used} bytes");
    }
}

#[test]
fn heavy_meshes_and_the_batch_options_pick_the_path() {
    let Some(mut rig) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    if !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    let at = |n: usize| -> Vec<Mat4> {
        (0..n)
            .map(|i| Mat4::from_translation(vec3((i % 64) as f32 - 32.0, 0.0, -((i / 64) as f32))))
            .collect()
    };
    let with = |culling| InstanceBatchOptions {
        culling,
        ..Default::default()
    };
    // A sphere of 2,024 triangles: 16.8 million instance-triangles, the
    // measured crossover, are 8,290 instances of it.
    let heavy = rig.mesh(&MeshData::sphere(0.3, 46), glow(Color::RED));
    let cube = rig.mesh(&MeshData::cube(0.3), glow(Color::BLUE));
    let triangles = super::capabilities::GPU_BATCH_TRIANGLES as usize;
    let enough = triangles.div_ceil(2024);
    let route = |rig: &mut BatchRig, mesh, n, culling| {
        let batch = rig.batch(mesh, &at(n), with(culling));
        rig.renderer.batch_route(batch.id).unwrap()
    };
    use crate::render::BatchCulling::{Auto, PreferCpu, PreferGpu};
    assert_eq!(route(&mut rig, heavy, enough, Auto), Route::Gpu, "heavy enough");
    assert_eq!(route(&mut rig, heavy, enough - 1, Auto), Route::CpuSmall, "just short");
    assert_eq!(route(&mut rig, cube, enough, Auto), Route::CpuSmall, "light meshes");
    // The game's wish overrules the crossover, both ways.
    assert_eq!(route(&mut rig, cube, 100, PreferGpu), Route::Gpu);
    assert_eq!(route(&mut rig, heavy, enough, PreferCpu), Route::CpuSmall);
    // A see-through batch keeps its instances' order: the CPU path, even
    // when large. Asking for the GPU path still gets it.
    let glass = rig.mesh(
        &MeshData::sphere(0.3, 46),
        MaterialDesc {
            color: Color::rgba(1.0, 1.0, 1.0, 0.5),
            blend: Blend::Transparent,
            ..Default::default()
        },
    );
    assert_eq!(route(&mut rig, glass, enough, Auto), Route::CpuSmall, "see-through");
    assert_eq!(route(&mut rig, glass, enough, PreferGpu), Route::Gpu);
    // Every one of them is drawn.
    rig.ctx.scene3d.camera.position = vec3(0.0, 40.0, 30.0);
    rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -60.0));
    rig.ctx.scene3d.camera.far = 400.0;
    let image = rig.image();
    assert!(image.fraction(0..image.width, 0..image.height, |p| p[0] > 100 || p[2] > 100) > 0.1);
    let stats = rig.renderer.instance_batch_stats();
    assert_eq!((stats.gpu_driven, stats.cpu_small), (3, 4), "{stats:?}");
    // Where the device has no compute culling the wish changes nothing.
    let webgl2 = RigSetup {
        compute: false,
        ..Default::default()
    };
    let Some(mut low) = BatchRig::new(webgl2) else { return };
    let cube = low.mesh(&MeshData::cube(0.3), glow(Color::BLUE));
    assert_eq!(route(&mut low, cube, 100, PreferGpu), Route::CpuNoCompute);
}

#[test]
fn neighbouring_visible_cells_are_one_draw_on_the_cpu_path() {
    let Some(mut rig) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    rig.renderer.batching.path = BatchPath::Cpu;
    let mesh = rig.mesh(&MeshData::cube(0.2), glow(Color::RED));
    // 32 x 32 instances in 64 cells, all in view and in range.
    let transforms: Vec<_> = (0..1024)
        .map(|i| Mat4::from_translation(vec3((i % 32) as f32 - 16.0, 0.0, -10.0 - (i / 32) as f32)))
        .collect();
    let batch = rig.batch(
        mesh,
        &transforms,
        InstanceBatchOptions {
            cell_size: 4.0,
            ..Default::default()
        },
    );
    rig.ctx.scene3d.camera.position = vec3(0.0, 30.0, 10.0);
    rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -25.0));
    rig.frame();
    let cost = rig.frame();
    let runs = &rig.renderer.static_batches[batch.id as usize].as_ref().unwrap().visible[0];
    assert_eq!(runs, &[(0, 1024)], "every cell is visible: one run");
    // The depth prepass and the opaque pass: one draw each.
    assert_eq!(cost.changes.draws, 2, "{:?}", cost.changes);
    // Half the field out of range: the instances kept are still few runs,
    // decided per instance where a cell straddles the edge.
    rig.free(batch);
    let batch = rig.batch(
        mesh,
        &transforms,
        InstanceBatchOptions {
            cell_size: 4.0,
            max_distance: 45.0,
            ..Default::default()
        },
    );
    rig.frame();
    let b = rig.renderer.static_batches[batch.id as usize].as_ref().unwrap();
    let eye = rig.ctx.scene3d.camera.position;
    let expected = transforms
        .iter()
        .filter(|m| m.w_axis.truncate().distance(eye) - 0.2 * 3f32.sqrt() / 2.0 * 1.0001 <= 45.0)
        .count() as u32;
    let drawn: u32 = b.visible[0].iter().map(|r| r.1).sum();
    assert!(
        drawn.abs_diff(expected) <= 2 && drawn < 1000 && drawn > 100,
        "{drawn} instances drawn, {expected} within the distance"
    );
}

#[test]
fn batches_of_one_mesh_share_their_draw_state() {
    for path in [BatchPath::Gpu, BatchPath::Cpu] {
        let Some(mut rig) = BatchRig::new(RigSetup::default()) else {
            return;
        };
        if path == BatchPath::Gpu && !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
            continue;
        }
        rig.renderer.batching.path = path;
        // Two meshes, their batches made in turn: sorted by mesh to draw.
        let meshes = [Color::RED, Color::GREEN].map(|c| rig.mesh(&MeshData::cube(0.3), glow(c)));
        for i in 0..40 {
            let transforms: Vec<_> = (0..64)
                .map(|k| Mat4::from_translation(vec3(i as f32 - 20.0, 0.0, -5.0 - k as f32 * 0.5)))
                .collect();
            rig.batch(meshes[i % 2], &transforms, Default::default());
        }
        rig.ctx.scene3d.camera.position = vec3(0.0, 25.0, 5.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -20.0));
        rig.frame();
        let c = rig.frame().changes;
        eprintln!("{path:?}: {c:?}");
        // Two passes draw them (depth prepass, opaque); each binds each
        // mesh's material once, and the shared geometry once, not once a batch.
        assert_eq!(c.pipelines, 2, "{path:?}: one pipeline a pass: {c:?}");
        assert_eq!(c.index_buffers, 2, "{path:?}: shared indices once a pass: {c:?}");
        assert_eq!(c.draws, 80, "{path:?}: one draw per batch and pass: {c:?}");
        match path {
            // Per batch: its bind group. Per pass: the marker, two materials.
            BatchPath::Gpu => assert!(c.bind_groups <= 80 + 12 && c.vertex_buffers <= 12, "{c:?}"),
            // Per batch: its instances. Per pass: the dummy group, two materials.
            _ => assert!(c.vertex_buffers <= 80 + 12 && c.bind_groups <= 12, "{c:?}"),
        }
    }
}

#[test]
fn freed_ranges_are_reused_and_growth_keeps_the_batches() {
    let Some(mut gpu) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    if !support::needs(&gpu.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    let Some(mut cpu) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    gpu.renderer.batching.path = BatchPath::Gpu;
    cpu.renderer.batching.path = BatchPath::Cpu;
    let meshes = [&mut gpu, &mut cpu].map(|rig| {
        rig.ctx.scene3d.camera.position = vec3(0.0, 30.0, 12.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -20.0));
        rig.mesh(&MeshData::cube(0.4), glow(Color::GREEN))
    });
    let row = |z: usize, n: usize| -> Vec<Mat4> {
        (0..n)
            .map(|i| Mat4::from_translation(vec3((i % 64) as f32 * 0.7 - 22.0, 0.0, -(z as f32) - (i / 64) as f32)))
            .collect()
    };
    // Streaming: batches come and go, and later ones are far larger, so
    // the pool's buffers are replaced by bigger ones while batches live.
    let mut live = std::collections::VecDeque::new();
    let mut high = 0;
    for step in 0..24 {
        let n = if step < 12 { 200 } else { 6000 };
        let transforms = row(step % 30, n);
        live.push_back((
            gpu.batch(meshes[0], &transforms, Default::default()),
            cpu.batch(meshes[1], &transforms, Default::default()),
        ));
        if live.len() > 3 {
            let (a, b) = live.pop_front().unwrap();
            gpu.free(a);
            cpu.free(b);
        }
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("step {step}"), &a, &b, SAME_DEVICE, 0.01);
        gpu.renderer.check_mirrors(&gpu.gpu.device, &gpu.gpu.queue);
        let (pools, _, used) = gpu.renderer.batch_pool_use();
        assert_eq!(pools, 1);
        if step == 11 {
            high = used;
        }
        if (6..12).contains(&step) {
            assert!(used <= high.max(used), "{used}");
        }
    }
    // Three small batches in turn never took more than a few ranges:
    // freed ones were handed out again.
    assert!(
        high < 3 * (200 * 96 + 4096) * 3,
        "the pool held {high} bytes for three small batches"
    );
    assert!(
        gpu.renderer.instance_batch_stats().pool_bytes > 3 * 6000 * 96,
        "the pool grew"
    );
}

#[test]
fn freed_batches_give_their_pages_and_pools_back() {
    let Some((mut rig, _)) = field_rig(RigSetup::default(), BatchPath::Gpu) else {
        return;
    };
    rig.ctx.scene3d.lighting.shadows = true;
    rig.frame();
    let small = rig.renderer.instance_batch_stats();
    assert_eq!(small.pools, 1);
    // Far larger batches arrive: the pool's buffers grow.
    let cube = rig.mesh(&MeshData::cube(0.2), glow(Color::GREEN));
    let many = |shift: f32| -> Vec<Mat4> {
        (0..40_000)
            .map(|i| {
                Mat4::from_translation(vec3(
                    (i % 200) as f32 * 0.2 - 20.0 + shift,
                    1.0,
                    -((i / 200) as f32) * 0.2,
                ))
            })
            .collect()
    };
    let big: Vec<InstanceBatch> = (0..3)
        .map(|i| rig.batch(cube, &many(i as f32 * 0.05), Default::default()))
        .collect();
    rig.frame();
    let grown = rig.renderer.instance_batch_stats();
    assert!(
        grown.pool_bytes > small.pool_bytes + 3 * 40_000 * 96,
        "{} bytes, then {}",
        small.pool_bytes,
        grown.pool_bytes
    );
    // They go, the small ones stay: an updated batch keeps its page.
    for batch in big {
        rig.free(batch);
    }
    let kept = rig.image();
    let stats = rig.renderer.instance_batch_stats();
    assert_eq!((stats.pools, stats.pages), (small.pools, small.pages));
    rig.renderer.check_mirrors(&rig.gpu.device, &rig.gpu.queue);
    // Every batch goes: the pool and its pages give their memory back.
    let ids: Vec<u32> = (0..rig.ctx.gfx.assets3d.instance_batches.len() as u32).collect();
    for id in ids {
        if rig.ctx.gfx.assets3d.instance_batches[id as usize].is_some() {
            rig.renderer
                .set_instance_batch(&rig.gpu.device, &rig.gpu.queue, id, None);
        }
    }
    rig.frame();
    // Buffers survive short gaps but are released after eight empty frames.
    assert_eq!(rig.renderer.instance_batch_stats().pool_bytes, stats.pool_bytes);
    for _ in 1..8 {
        rig.frame();
    }
    let empty = rig.renderer.instance_batch_stats();
    assert_eq!((empty.batches, empty.pools, empty.pages), (0, 0, 0));
    assert!(
        empty.pool_bytes < small.pool_bytes.max(1 << 20) && empty.pool_bytes < grown.pool_bytes / 8,
        "{} bytes held with no batch left ({} at the peak)",
        empty.pool_bytes,
        grown.pool_bytes
    );
    // And the emptied pool takes batches again.
    let again = rig.batch(cube, &many(0.0), Default::default());
    assert_eq!(rig.renderer.batch_route(again.id), Some(Route::Gpu));
    let image = rig.image();
    assert!(
        image.fraction(0..image.width, 0..image.height, |p| p[1] > 150) > 0.01,
        "the new batch is drawn"
    );
    assert!(
        kept.fraction(0..kept.width, 0..kept.height, |p| p[0] > 40 || p[1] > 40 || p[2] > 40) > 0.05,
        "the batches that stayed were drawn"
    );
    assert_eq!(rig.renderer.instance_batch_stats().pools, 1);
    rig.renderer.check_mirrors(&rig.gpu.device, &rig.gpu.queue);
}

#[test]
fn a_small_storage_binding_holds_the_frames_buffer_and_the_grid_gives_way() {
    // 256 KiB a storage binding: less than the occlusion grid's room on a
    // device with large textures, so the frame's buffer would not bind.
    let setup = RigSetup {
        shrink: |limits| limits.max_storage_buffer_binding_size = 256 << 10,
        ..Default::default()
    };
    let (Some((mut gpu, _)), Some((mut cpu, _))) = (field_rig(setup, BatchPath::Gpu), field_rig(setup, BatchPath::Cpu))
    else {
        return;
    };
    for rig in [&mut gpu, &mut cpu] {
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.camera.occlusion_culling = true;
    }
    traffic::take_too_big();
    for k in 0..4 {
        for rig in [&mut gpu, &mut cpu] {
            fly(rig, k);
            // A grid that still fits what is left for it.
            rig.grid = Some(wall_grid(
                rig.ctx.scene3d.camera.view_proj(),
                rig.ctx.scene3d.camera.position,
            ));
        }
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("frame {k}"), &a, &b, SAME_DEVICE, 0.05);
    }
    let stats = gpu.renderer.instance_batch_stats();
    assert!(stats.gpu_driven > 0 && stats.views_skipped == 0, "{stats:?}");
    assert_eq!(traffic::take_too_big(), 0, "nothing was left out");
}

#[test]
fn views_past_the_frames_buffer_are_not_drawn_and_reported() {
    let (Some((mut gpu, _)), Some((mut cpu, _))) = (
        field_rig(RigSetup::default(), BatchPath::Gpu),
        field_rig(RigSetup::default(), BatchPath::Cpu),
    ) else {
        return;
    };
    for rig in [&mut gpu, &mut cpu] {
        rig.ctx.scene3d.lighting.shadows = true;
        fly(rig, 2);
    }
    // Room for the camera and two sun cascades only.
    gpu.renderer.limit_batch_views(3);
    traffic::take_too_big();
    for _ in 0..3 {
        let image = gpu.image();
        cpu.frame();
        assert!(
            image.fraction(0..image.width, 0..image.height, |p| p[0] > 40 || p[1] > 40 || p[2] > 40) > 0.05,
            "the camera's view is drawn"
        );
        assert!(gpu.renderer.instance_batch_stats().views_skipped > 0);
    }
    assert_eq!(traffic::take_too_big(), 1, "three frames, one report");
    // With room again every view is culled, and the picture is the CPU path's.
    gpu.renderer.limit_batch_views(usize::MAX);
    let (a, b) = (gpu.image(), cpu.image());
    assert_same("with room for every view", &a, &b, SAME_DEVICE, 0.05);
    assert_eq!(gpu.renderer.instance_batch_stats().views_skipped, 0);
    assert_eq!(traffic::take_too_big(), 0);
}

/// A device with small buffers: one megabyte a storage binding.
fn small_bindings(limits: &mut wgpu::Limits) {
    limits.max_storage_buffer_binding_size = 1 << 20;
}

#[test]
fn a_batch_past_one_storage_binding_is_culled_on_the_cpu() {
    let setup = RigSetup {
        shrink: small_bindings,
        ..Default::default()
    };
    let Some(mut rig) = BatchRig::new(setup) else { return };
    if !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    rig.renderer.batching.path = BatchPath::Gpu;
    let mesh = rig.mesh(&MeshData::cube(0.2), glow(Color::RED));
    let at = |n: usize| -> Vec<Mat4> {
        (0..n)
            .map(|i| Mat4::from_translation(vec3((i % 128) as f32 * 0.3 - 19.0, 0.0, -((i / 128) as f32) * 0.3)))
            .collect()
    };
    traffic::take_too_big();
    // 12,000 instances need 1.15 MB of records: more than a binding.
    let big = rig.batch(mesh, &at(12_000), Default::default());
    assert_eq!(rig.renderer.batch_route(big.id), Some(Route::CpuOverLimit));
    assert_eq!(traffic::take_too_big(), 1, "said once, when the batch was made");
    // 8,000 fit.
    let fits = rig.batch(mesh, &at(8_000), Default::default());
    assert_eq!(rig.renderer.batch_route(fits.id), Some(Route::Gpu));
    rig.ctx.scene3d.camera.position = vec3(0.0, 25.0, 10.0);
    rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -15.0));
    for _ in 0..3 {
        let image = rig.image();
        assert!(
            image.fraction(0..image.width, 0..image.height, |p| p[0] > 150) > 0.05,
            "the batch over the limit is drawn all the same"
        );
    }
    assert_eq!(traffic::take_too_big(), 0, "and not again every frame");
    let stats = rig.renderer.instance_batch_stats();
    assert_eq!((stats.cpu_over_limit, stats.gpu_driven, stats.dropped), (1, 1, 0));
}

#[test]
fn batches_past_one_pool_get_another_and_draw_the_same() {
    let setup = RigSetup {
        shrink: small_bindings,
        ..Default::default()
    };
    let Some(mut gpu) = BatchRig::new(setup) else { return };
    if !support::needs(&gpu.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    let Some(mut cpu) = BatchRig::new(setup) else { return };
    gpu.renderer.batching.path = BatchPath::Gpu;
    cpu.renderer.batching.path = BatchPath::Cpu;
    traffic::take_too_big();
    for rig in [&mut gpu, &mut cpu] {
        let mesh = rig.mesh(&MeshData::cube(0.3), glow(Color::BLUE));
        // Five batches of 4,000: 384 KB of records each, two to a pool.
        for b in 0..5 {
            let transforms: Vec<_> = (0..4000)
                .map(|i| {
                    Mat4::from_translation(vec3(
                        (i % 100) as f32 * 0.4 - 20.0,
                        0.0,
                        -(b as f32) * 18.0 - (i / 100) as f32 * 0.4,
                    ))
                })
                .collect();
            rig.batch(mesh, &transforms, Default::default());
        }
        rig.ctx.scene3d.camera.far = 300.0;
    }
    let stats = gpu.renderer.instance_batch_stats();
    assert_eq!(stats.gpu_driven, 5, "{stats:?}");
    assert_eq!(stats.pools, 3, "{stats:?}");
    for k in 0..8 {
        for rig in [&mut gpu, &mut cpu] {
            rig.ctx.scene3d.camera.position = vec3(-10.0 + 3.0 * k as f32, 12.0, 12.0 - 6.0 * k as f32);
            rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -40.0 - 6.0 * k as f32));
        }
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("three pools, frame {k}"), &a, &b, SAME_DEVICE, 0.02);
    }
    let stats = gpu.renderer.instance_batch_stats();
    assert!(
        stats.cull_dispatches >= 2 && stats.cull_dispatches <= 3,
        "one dispatch a pool in sight: {stats:?}"
    );
    assert_eq!(stats.views_skipped, 0);
    assert_eq!(traffic::take_too_big(), 0, "nothing was over a limit");
}

/// A lit ground under the origin's surroundings, so shadows show.
fn ground(rig: &mut BatchRig) {
    let ground = rig.mesh(
        &MeshData::plane(160.0),
        MaterialDesc {
            color: Color::WHITE,
            ..Default::default()
        },
    );
    rig.ctx
        .world
        .spawn((Transform3D::at(vec3(0.0, 0.0, -20.0)), Model::new(ground)));
}

/// Three levels that all draw at every distance (their bands overlap), in
/// the camera and in shadow views: three views of a batch per frame view.
fn stacked_levels(rig: &mut BatchRig) -> Vec<InstanceLevel> {
    [Color::RED, Color::GREEN, Color::BLUE]
        .iter()
        .enumerate()
        .map(|(i, &color)| InstanceLevel {
            mesh: rig.mesh(&MeshData::cube(0.5 - 0.12 * i as f32), glow(color)),
            min_distance: 0.0,
            max_distance: 500.0,
            camera: true,
            shadows: true,
        })
        .collect()
}

/// A shadowed point light: six shadow views that reach `range`.
fn shadowed_lamp(rig: &mut BatchRig, at: Vec3, range: f32) -> Entity {
    rig.ctx.world.spawn((
        Transform3D::at(at),
        PointLight3D {
            intensity: 300.0,
            range,
            shadows: true,
            ..Default::default()
        },
    ))
}

#[test]
fn a_big_batch_keeps_every_view_past_one_page() {
    // One batch of 8,000 instances at three shadow-casting levels, seen by
    // the camera, four sun cascades and four shadowed point lights: up to
    // 29 views of three lists of 32 KB each, 2.8 MB, where one buffer
    // holds 1 MB. Every view still has a list: the pool gets more pages.
    let setup = RigSetup {
        shrink: small_bindings,
        ..Default::default()
    };
    let Some(mut gpu) = BatchRig::new(setup) else { return };
    if !support::needs(&gpu.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    let Some(mut cpu) = BatchRig::new(setup) else { return };
    gpu.renderer.batching.path = BatchPath::Gpu;
    cpu.renderer.batching.path = BatchPath::Cpu;
    traffic::take_too_big();
    for rig in [&mut gpu, &mut cpu] {
        ground(rig);
        let levels = stacked_levels(rig);
        let transforms: Vec<_> = (0..8000)
            .map(|i| Mat4::from_translation(vec3((i % 100) as f32 * 0.7 - 35.0, 1.5, -((i / 100) as f32) * 0.7)))
            .collect();
        let options = InstanceBatchOptions {
            cell_size: 8.0,
            ..Default::default()
        };
        rig.batch_levels(&levels, &transforms, &[], options);
        for (x, z) in [(-12.0, -8.0), (12.0, -8.0), (-12.0, -30.0), (12.0, -30.0)] {
            let _ = shadowed_lamp(rig, vec3(x, 6.0, z), 40.0);
        }
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.lighting.shadow_distance = 80.0;
        rig.ctx.scene3d.camera.far = 300.0;
    }
    assert_eq!(gpu.renderer.batch_route(0), Some(Route::Gpu));
    for k in 0..6 {
        for rig in [&mut gpu, &mut cpu] {
            rig.ctx.scene3d.camera.position = vec3(-14.0 + 5.0 * k as f32, 9.0 + k as f32, 16.0 - 4.0 * k as f32);
            rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -20.0));
        }
        let (a, b) = (gpu.image(), cpu.image());
        assert_same(&format!("views over three pages, frame {k}"), &a, &b, SAME_DEVICE, 0.2);
        let stats = gpu.renderer.instance_batch_stats();
        assert_eq!(stats.views_skipped, 0, "frame {k}: {stats:?}");
    }
    let stats = gpu.renderer.instance_batch_stats();
    let views = gpu.renderer.static_batches[0].as_ref().unwrap().views.len();
    eprintln!("{views} views in {} pages: {stats:?}", stats.pages);
    assert!(views > 32, "more views than one megabyte of lists holds: {views}");
    assert!(stats.pages >= 2 && stats.pools == 1, "{stats:?}");
    assert_eq!(traffic::take_too_big(), 0, "no limit was hit");
}

#[test]
fn new_lights_views_of_many_batches_spill_into_another_page() {
    // Ten batches of 1,000 instances at three levels share a pool. With the
    // camera and the sun's cascades their lists fit one page; shadowed
    // point lights add six views of each and they no longer do. Nothing
    // vanishes: the new views go to a second page.
    let setup = RigSetup {
        shrink: small_bindings,
        ..Default::default()
    };
    let Some(mut gpu) = BatchRig::new(setup) else { return };
    if !support::needs(&gpu.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    let Some(mut cpu) = BatchRig::new(setup) else { return };
    // The control: the CPU path with every batch's instances in the other
    // order than the CPU rig's. The paths draw a batch's instances in different orders (the
    // compute pass in whatever order the device runs it), so the scene
    // must not depend on the order: no two instances may meet at the same
    // depth. They did once: cubes 0.5 m wide, 0.2 m apart, had coplanar
    // overlapping tops, and the last one drawn won each tied sample (34
    // pixels off by up to 5 of 255 on a software Direct3D 12 device, and 5
    // pixels on any device between the CPU path and this control). Here
    // the cubes are 0.15 m wide, with air between them.
    let Some(mut flipped) = BatchRig::new(setup) else {
        return;
    };
    gpu.renderer.batching.path = BatchPath::Gpu;
    cpu.renderer.batching.path = BatchPath::Cpu;
    flipped.renderer.batching.path = BatchPath::Cpu;
    traffic::take_too_big();
    for (r, rig) in [&mut gpu, &mut cpu, &mut flipped].into_iter().enumerate() {
        ground(rig);
        let levels = stacked_levels(rig);
        for b in 0..10 {
            let origin = vec3((b % 5) as f32 * 9.0 - 22.0, 1.5, -((b / 5) as f32) * 12.0 - 6.0);
            let mut transforms: Vec<_> = (0..1000)
                .map(|i| {
                    let at = origin + vec3((i % 40) as f32 * 0.2, 0.0, -((i / 40) as f32) * 0.4);
                    Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(0.3))
                })
                .collect();
            if r == 2 {
                // (The rig turns a CPU rig's order around once already.)
                transforms.reverse();
            }
            rig.batch_levels(&levels, &transforms, &[], Default::default());
        }
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.lighting.shadow_distance = 80.0;
        rig.ctx.scene3d.camera.far = 300.0;
        rig.ctx.scene3d.camera.position = vec3(0.0, 22.0, 14.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -14.0));
    }
    for k in 0..2 {
        let (a, b, c) = (gpu.image(), cpu.image(), flipped.image());
        assert_same(
            &format!("before the light, frame {k}, instance order"),
            &b,
            &c,
            SAME_DEVICE,
            0.2,
        );
        assert_same(&format!("before the light, frame {k}"), &a, &b, SAME_DEVICE, 0.2);
    }
    let before = gpu.renderer.instance_batch_stats();
    assert_eq!(
        (before.pools, before.pages, before.gpu_driven),
        (1, 1, 10),
        "{before:?}"
    );
    for rig in [&mut gpu, &mut cpu, &mut flipped] {
        rig.ctx.scene3d.lighting.light_shadow_views = 24;
        for x in [-14.0, 0.0, 14.0] {
            let _ = shadowed_lamp(rig, vec3(x, 3.0, -12.0), 60.0);
        }
    }
    for k in 0..4 {
        let (a, b, c) = (gpu.image(), cpu.image(), flipped.image());
        assert_same(
            &format!("with the light, frame {k}, instance order"),
            &b,
            &c,
            SAME_DEVICE,
            0.2,
        );
        assert_same(&format!("with the light, frame {k}"), &a, &b, SAME_DEVICE, 0.2);
        let stats = gpu.renderer.instance_batch_stats();
        assert_eq!(stats.views_skipped, 0, "frame {k}: {stats:?}");
    }
    let after = gpu.renderer.instance_batch_stats();
    let views: usize = gpu
        .renderer
        .static_batches
        .iter()
        .flatten()
        .map(|b| b.views.len())
        .sum();
    eprintln!("{views} views: {after:?}");
    assert!(views > 10 * 3 * 5, "the light's views have lists: {views}");
    assert_eq!((after.pools, after.pages), (1, 3), "{after:?}");
    assert_eq!(traffic::take_too_big(), 0, "no limit was hit");
}

#[test]
fn a_full_pool_keeps_drawing_when_more_views_arrive() {
    // A batch that fills its pool's records to the last kilobytes. Views
    // that arrive later (shadows turned on, a shadowed light, a target
    // camera) need a bigger culling table: it has its place outside the
    // records, so the pool is never dropped for want of room.
    let setup = RigSetup {
        shrink: small_bindings,
        ..Default::default()
    };
    let Some(mut gpu) = BatchRig::new(setup) else { return };
    if !support::needs(&gpu.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    let Some(mut cpu) = BatchRig::new(setup) else { return };
    gpu.renderer.batching.path = BatchPath::Gpu;
    cpu.renderer.batching.path = BatchPath::Cpu;
    // What the capability report promises always fits does.
    let promised = super::capabilities::GpuLimits::of(&gpu.gpu.device)
        .instance_batches()
        .max_gpu_instances as usize;
    assert_eq!(promised, ((1 << 20) - (64 << 10)) / 128);
    traffic::take_too_big();
    let at = |n: usize| -> Vec<Mat4> {
        (0..n)
            .map(|i| Mat4::from_translation(vec3((i % 128) as f32 * 0.3 - 19.0, 1.0, -((i / 128) as f32) * 0.3)))
            .collect()
    };
    for rig in [&mut gpu, &mut cpu] {
        ground(rig);
        let mesh = rig.mesh(&MeshData::cube(0.2), glow(Color::RED));
        let fits = rig.batch(mesh, &at(promised), Default::default());
        rig.free(fits);
        // 10,880 instances: 1,044,496 bytes of records and their cells,
        // of the 1,048,576 a binding holds.
        rig.batch(mesh, &at(10_880), Default::default());
        rig.ctx.scene3d.camera.far = 300.0;
        rig.ctx.scene3d.camera.position = vec3(0.0, 20.0, 12.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -14.0));
    }
    assert_eq!(
        gpu.renderer.batch_route(0),
        None,
        "the promised size was GPU-driven, then freed"
    );
    assert_eq!(gpu.renderer.batch_route(1), Some(Route::Gpu));
    let (pools, _, used) = gpu.renderer.batch_pool_use();
    assert_eq!(pools, 1);
    let mut views = 0;
    let mut same = |gpu: &mut BatchRig, cpu: &mut BatchRig, what: &str| {
        for k in 0..2 {
            let (a, b) = (gpu.image(), cpu.image());
            assert_same(&format!("{what}, frame {k}"), &a, &b, SAME_DEVICE, 0.05);
            let stats = gpu.renderer.instance_batch_stats();
            assert_eq!((stats.views_skipped, stats.gpu_driven), (0, 1), "{what}: {stats:?}");
        }
        let now = gpu.renderer.static_batches[1].as_ref().unwrap().views.len();
        assert!(
            now > views,
            "{what}: the batch has more views than before ({now}, {views})"
        );
        views = now;
    };
    same(&mut gpu, &mut cpu, "camera only");
    assert!(used > (1 << 20) - (8 << 10), "the pool's records are full: {used}");
    for rig in [&mut gpu, &mut cpu] {
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.lighting.shadow_distance = 80.0;
    }
    same(&mut gpu, &mut cpu, "sun shadows on");
    for rig in [&mut gpu, &mut cpu] {
        let _ = shadowed_lamp(rig, vec3(0.0, 8.0, -12.0), 60.0);
    }
    same(&mut gpu, &mut cpu, "a shadowed light");
    for rig in [&mut gpu, &mut cpu] {
        let mut camera = rig.ctx.scene3d.camera;
        camera.position = vec3(20.0, 10.0, -14.0);
        camera.look_at(vec3(0.0, 0.0, -14.0));
        rig.target_camera(camera);
    }
    same(&mut gpu, &mut cpu, "a target camera");
    assert_eq!(traffic::take_too_big(), 0, "nothing was reported, on any frame");
}

#[test]
fn an_updated_batch_over_a_limit_is_reported_once() {
    let setup = RigSetup {
        shrink: small_bindings,
        ..Default::default()
    };
    let Some(mut rig) = BatchRig::new(setup) else { return };
    if !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    rig.renderer.batching.path = BatchPath::Gpu;
    let mesh = rig.mesh(&MeshData::cube(0.2), glow(Color::RED));
    let at = |n: usize, shift: f32| -> Vec<Mat4> {
        (0..n)
            .map(|i| {
                Mat4::from_translation(vec3(
                    (i % 128) as f32 * 0.3 - 19.0 + shift,
                    0.0,
                    -((i / 128) as f32) * 0.3,
                ))
            })
            .collect()
    };
    traffic::take_too_big();
    let batch = rig.batch(mesh, &at(12_000, 0.0), Default::default());
    assert_eq!(traffic::take_too_big(), 1, "said when the batch was made");
    // A game that moves its instances every so often is not told again.
    for step in 1..4 {
        rig.update(batch, &at(12_000, step as f32));
        rig.frame();
        assert_eq!(rig.renderer.batch_route(batch.id), Some(Route::CpuOverLimit));
    }
    assert_eq!(traffic::take_too_big(), 0, "three updates, no more reports");
    // Once it fits it is GPU-driven; growing past the limit again is news.
    rig.update(batch, &at(8_000, 0.0));
    assert_eq!(rig.renderer.batch_route(batch.id), Some(Route::Gpu));
    rig.update(batch, &at(12_000, 0.0));
    assert_eq!(traffic::take_too_big(), 1, "over the limit again: said again");
}

#[test]
fn a_batch_past_one_buffer_is_dropped_and_reported() {
    for compute in [true, false] {
        let setup = RigSetup {
            compute,
            // A megabyte a buffer: 13,107 instances of 80 bytes.
            shrink: |limits| {
                limits.max_buffer_size = 1 << 20;
                limits.max_storage_buffer_binding_size = limits.max_storage_buffer_binding_size.min(1 << 20);
                limits.max_uniform_buffer_binding_size = limits.max_uniform_buffer_binding_size.min(1 << 20);
            },
            ..Default::default()
        };
        let Some(mut rig) = BatchRig::new(setup) else { return };
        rig.renderer.batching.path = BatchPath::Cpu;
        let mesh = rig.mesh(&MeshData::cube(0.2), glow(Color::RED));
        let at = |n: usize| -> Vec<Mat4> {
            (0..n)
                .map(|i| Mat4::from_translation(vec3((i % 128) as f32 * 0.3 - 19.0, 0.0, -((i / 128) as f32) * 0.3)))
                .collect()
        };
        traffic::take_too_big();
        let big = rig.batch(mesh, &at(14_000), Default::default());
        assert_eq!(rig.renderer.batch_route(big.id), Some(Route::Dropped));
        assert_eq!(traffic::take_too_big(), 1);
        rig.ctx.scene3d.camera.position = vec3(0.0, 25.0, 10.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -15.0));
        let image = rig.image();
        assert_eq!(
            image.fraction(0..image.width, 0..image.height, |p| p[0] > 150),
            0.0,
            "the dropped batch draws nothing"
        );
        let fits = rig.batch(mesh, &at(13_000), Default::default());
        assert_ne!(rig.renderer.batch_route(fits.id), Some(Route::Dropped));
        let image = rig.image();
        assert!(image.fraction(0..image.width, 0..image.height, |p| p[0] > 150) > 0.05);
        let stats = rig.renderer.instance_batch_stats();
        assert_eq!(stats.dropped, 1, "{stats:?}");
        assert_eq!(traffic::take_too_big(), 0, "not again every frame");
    }
}

#[test]
fn batch_views_past_one_dispatch_are_not_drawn_and_reported() {
    let setup = RigSetup {
        // 4 x 4 x 4 workgroups a dispatch: 64 cells.
        shrink: |limits| limits.max_compute_workgroups_per_dimension = 4,
        ..Default::default()
    };
    let Some(mut rig) = BatchRig::new(setup) else { return };
    if !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    rig.renderer.batching.path = BatchPath::Gpu;
    let Some(mut cpu) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    cpu.renderer.batching.path = BatchPath::Cpu;
    let row = |z: f32, n: usize| -> Vec<Mat4> {
        (0..n)
            .map(|i| Mat4::from_translation(vec3(i as f32 * 0.5 - 16.0, 0.0, z)))
            .collect()
    };
    let options = InstanceBatchOptions {
        cell_size: 4.0,
        ..Default::default()
    };
    traffic::take_too_big();
    // Twelve batches of 8 cells each: 96 cells, of which 64 fit. The
    // reference has the eight batches that fit, and nothing else.
    for (rig, batches) in [(&mut rig, 12), (&mut cpu, 8)] {
        let mesh = rig.mesh(&MeshData::cube(0.3), glow(Color::RED));
        for b in 0..batches {
            rig.batch(mesh, &row(-4.0 - b as f32, 64), options);
        }
        rig.ctx.scene3d.camera.position = vec3(0.0, 25.0, 5.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -12.0));
    }
    // Every frame the same: a view left out of the dispatch is not drawn
    // with the counts an earlier frame left in its draw arguments.
    for k in 0..4 {
        if k == 2 {
            assert_eq!(traffic::take_too_big(), 1, "two frames, one report");
            // The camera turns away and back: the jobs change in between,
            // and the limit bites anew (so it is said anew).
            rig.ctx.scene3d.camera.look_at(vec3(0.0, 50.0, 5.0));
            rig.frame();
            rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -12.0));
        }
        let (a, b) = (rig.image(), cpu.image());
        assert_same(
            &format!("eight of twelve batches, frame {k}"),
            &a,
            &b,
            SAME_DEVICE,
            0.01,
        );
        let stats = rig.renderer.instance_batch_stats();
        assert_eq!((stats.views_skipped, stats.cull_jobs), (4, 8), "frame {k}: {stats:?}");
    }
    assert_eq!(
        traffic::take_too_big(),
        1,
        "said once more when it bit again, not every frame"
    );
    // A scene that fits is whole, and nothing is reported.
    let Some(mut rig) = BatchRig::new(setup) else { return };
    rig.renderer.batching.path = BatchPath::Gpu;
    let Some(mut cpu) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    cpu.renderer.batching.path = BatchPath::Cpu;
    for rig in [&mut rig, &mut cpu] {
        let mesh = rig.mesh(&MeshData::cube(0.3), glow(Color::RED));
        // Six batches of 8 cells: 48, laid out past one row and one layer.
        for b in 0..6 {
            rig.batch(mesh, &row(-4.0 - b as f32, 64), options);
        }
        rig.ctx.scene3d.camera.position = vec3(0.0, 25.0, 5.0);
        rig.ctx.scene3d.camera.look_at(vec3(0.0, 0.0, -12.0));
    }
    let (a, b) = (rig.image(), cpu.image());
    assert_same("48 cells in a 4x4x4 dispatch", &a, &b, SAME_DEVICE, 0.01);
    assert_eq!(rig.renderer.instance_batch_stats().views_skipped, 0);
    assert_eq!(traffic::take_too_big(), 0);
}

#[test]
fn a_depth_grid_past_the_culling_buffer_is_left_out_and_reported() {
    let Some((mut rig, _)) = field_rig(RigSetup::default(), BatchPath::Gpu) else {
        return;
    };
    let Some((mut reference, _)) = field_rig(RigSetup::default(), BatchPath::Cpu) else {
        return;
    };
    fly(&mut rig, 3);
    fly(&mut reference, 3);
    rig.ctx.scene3d.camera.occlusion_culling = true;
    traffic::take_too_big();
    let view_proj = rig.ctx.scene3d.camera.view_proj();
    let all = reference.image();
    for _ in 0..3 {
        // A grid that hides everything, of 1024 x 1024 depths: 4 MB, more
        // than the culling pass's buffer for it holds.
        rig.grid = Some(OcclusionGrid::new(
            view_proj,
            rig.ctx.scene3d.camera.position,
            1024,
            1024,
            vec![0.0; 1024 * 1024],
        ));
        let image = rig.image();
        assert_same("without the oversized grid", &image, &all, SAME_DEVICE, 0.05);
    }
    assert_eq!(traffic::take_too_big(), 1, "three frames, one report");
    // A grid that fits is used: the same depths hide the batches behind
    // it (all but those reaching past the grid's view or the camera).
    rig.grid = Some(OcclusionGrid::new(
        view_proj,
        rig.ctx.scene3d.camera.position,
        16,
        16,
        vec![0.0; 256],
    ));
    let hidden = rig.image();
    let red = |i: &support::Image| i.fraction(0..i.width, 0..i.height, |p| p[0] > 200 && p[1] < 60 && p[2] < 60);
    assert!(
        red(&all) > 0.005 && red(&hidden) < red(&all) * 0.6,
        "{} of the picture red, then {}",
        red(&all),
        red(&hidden)
    );
}

#[test]
fn steady_frames_allocate_nothing_preparing_batches() {
    for path in [BatchPath::Gpu, BatchPath::Cpu] {
        let Some((mut rig, _)) = field_rig(RigSetup::default(), path) else {
            continue;
        };
        rig.ctx.scene3d.lighting.shadows = true;
        rig.ctx.scene3d.camera.occlusion_culling = true;
        // The whole flight once, so every list has its capacity and every
        // view its ranges and bind groups.
        let mut built = rig.renderer.batch_lists_built();
        for round in 0..2 {
            for k in 0..14 {
                rig.grid = Some(wall_grid(
                    rig.ctx.scene3d.camera.view_proj(),
                    rig.ctx.scene3d.camera.position,
                ));
                fly(&mut rig, k);
                rig.frame();
                // The frame itself: the camera moved, so every view's
                // lists were built again.
                let [_, ran, moving] = rig.prepared_allocations;
                let lists = rig.renderer.batch_lists_built();
                // And the same frame once more: nothing moved.
                let at_rest = rig.batch_preparation_allocations();
                if round == 1 {
                    assert!(ran >= 2, "{path:?}: the frame prepared its batches");
                    assert_eq!(moving, 0, "{path:?}: frame {k} allocated preparing its batches");
                    assert_eq!(at_rest, 0, "{path:?}: frame {k} allocated preparing its batches again");
                    if path == BatchPath::Cpu {
                        assert!(
                            lists > built + 4,
                            "the moving frame built the camera's and the shadows' lists"
                        );
                    }
                }
                built = rig.renderer.batch_lists_built();
            }
        }
    }
}

#[test]
fn recording_batches_allocates_only_inside_wgpu() {
    // Recording does allocate: wgpu grows its command lists, and its
    // backends allocate as bind groups and buffers are set. The batch code
    // around those calls must not. Test builds mark the batch drawing code
    // and unmark its calls into wgpu, so the counting allocator can tell
    // the two apart on any platform and without debug info.
    use crate::three::lights::tests::{count_marked_allocations, marked, unmarked};
    let (_, counted) = count_marked_allocations(|| {
        let _code = marked();
        let ours = std::hint::black_box(vec![1u8; 32]);
        let _library = unmarked();
        (ours, std::hint::black_box(vec![2u8; 32]))
    });
    assert_eq!(
        counted,
        [2, 1, 1],
        "the count tells marked code's allocations from a library's"
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .use_current_thread()
        .build()
        .unwrap();
    for path in [BatchPath::Gpu, BatchPath::Cpu] {
        let Some((mut rig, _)) = field_rig(RigSetup::default(), path) else {
            continue;
        };
        rig.ctx.scene3d.lighting.shadows = true;
        for k in 0..3 {
            fly(&mut rig, k);
            rig.frame();
        }
        let [all, entered, ours] = rig.recording_allocations(&pool);
        eprintln!("{path:?}: {all} allocations recording the frame, {ours} by the batch code in {entered} passes");
        assert!(entered > 0, "{path:?}: the batch drawing code ran while counting");
        assert_eq!(ours, 0, "{path:?}: the batch code allocated while recording");
    }
}

#[test]
fn short_lived_batches_reuse_buffers_then_expire_with_new_generations() {
    let Some(mut rig) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    if !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    rig.renderer.batching.path = BatchPath::Gpu;
    let mesh = rig.mesh(&MeshData::cube(1.0), glow(Color::BLUE));
    let transforms = vec![Mat4::IDENTITY; 513]; // crosses cell/workgroup boundaries
    let mut batch = rig.batch(mesh, &transforms, Default::default());
    let reference = rig.image();
    let generations = rig.renderer.batch_generations();
    assert_eq!(generations.len(), 3);
    for _ in 0..12 {
        rig.renderer.check_batch_order(&rig.gpu.device, &rig.gpu.queue);
        rig.free(batch);
        for _ in 0..3 {
            rig.frame();
        }
        batch = rig.batch(mesh, &transforms, Default::default());
        assert_eq!(rig.image(), reference);
        assert_eq!(
            rig.renderer.batch_generations(),
            generations,
            "short gaps must keep the same buffers"
        );
    }
    rig.free(batch);
    for _ in 0..8 {
        rig.frame();
    }
    let replaced = rig.renderer.batch_generations();
    assert_eq!(replaced.len(), 1, "the expired pool drops its pages");
    assert!(replaced[0] > *generations.iter().max().unwrap());
    rig.batch(mesh, &transforms, Default::default());
    assert_eq!(rig.image(), reference);
    let renewed = rig.renderer.batch_generations();
    assert!(renewed.iter().all(|g| *g > *generations.iter().max().unwrap()));
}

#[test]
fn material_blend_changes_reroute_auto_batches_without_transform_updates() {
    use crate::render::BatchCulling;
    let Some(mut rig) = BatchRig::new(RigSetup::default()) else {
        return;
    };
    if !support::needs(&rig.gpu.device, GpuFeature::ComputeCulling) {
        return;
    }
    let desc = glow(Color::WHITE);
    let mesh = rig.mesh(&MeshData::cube(1.0), desc);
    let data = vec![Mat4::IDENTITY; super::capabilities::GPU_BATCH_MIN as usize];
    let automatic = rig.batch(mesh, &data, Default::default());
    let forced = rig.batch(
        mesh,
        &data,
        InstanceBatchOptions {
            culling: BatchCulling::PreferGpu,
            ..Default::default()
        },
    );
    assert_eq!(rig.renderer.batch_route(automatic.id), Some(Route::Gpu));
    // BatchRig mirrors the production Renderer wrapper's material update path.
    for (blend, expected) in [(Blend::Transparent, Route::CpuSmall), (Blend::Opaque, Route::Gpu)] {
        let gpu = GpuMaterialDesc::new(&MaterialDesc { blend, ..desc });
        assert!(
            rig.renderer
                .update_material(&rig.gpu.device, &rig.gpu.queue, 0, &gpu, &rig.textures)
        );
        rig.renderer.refresh_batch_material(
            &rig.gpu.device,
            &rig.gpu.queue,
            0,
            &rig.ctx.gfx.assets3d.instance_batches,
        );
        assert_eq!(rig.renderer.batch_route(automatic.id), Some(expected));
        assert_eq!(rig.renderer.batch_route(forced.id), Some(Route::Gpu));
        assert!(
            !rig.renderer
                .update_material(&rig.gpu.device, &rig.gpu.queue, 0, &gpu, &rig.textures),
            "same blend needs no batch refresh"
        );
    }
    for (preference, route) in [
        (BatchCulling::PreferCpu, Route::CpuSmall),
        (BatchCulling::PreferGpu, Route::Gpu),
    ] {
        assert!(rig.ctx.gfx.set_instance_batch_culling(automatic, preference));
        rig.upload(automatic);
        assert_eq!(rig.renderer.batch_route(automatic.id), Some(route));
    }
}
