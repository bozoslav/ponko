//! Motion vectors of what a material's own `vertex` function does, drawn
//! offscreen under TAA: a shader that moves vertices with `frame.time`
//! writes that motion while the model holds still (as an ordinary model and
//! in a static batch, on the GPU-culled and the CPU path), adds it to the
//! model's own motion when it moves, and a GPU buffer that keeps its last
//! frame (`Graphics::set_gpu_buffer_motion`) gives each element its own
//! motion. The first frame, a history reset and TAA turned back on have no
//! motion; materials without such a `vertex` draw nothing more. The control
//! frames run as before this was added (`NO_VERTEX_PAST`: no rewrite, no
//! still motion draws) and must fail the same checks. Skips without a GPU
//! or without TAA (WebGL2 has none).
use super::vertex_motion::NO_VERTEX_PAST;
use super::*;
use crate::d3::Transform as Transform3D;
use crate::prelude::*;
use crate::render::render_test_support::{self as support, Image};
use crate::render::{
    AntiAliasing, BatchCulling, GpuBuffer, GpuFeature, InstanceBatchOptions, MaterialDesc, Mesh, MeshData, Model,
    Tonemap,
};
use crate::three::{Frame3D, GpuMaterialDesc, Scene3D, SkinFrame, SkyLight};

const SIZE: u32 = 128;

/// Slides the whole model along x by the time: one unit a second.
const SLIDE: &str = "fn vertex(v: Vertex) -> Vertex { var o = v; o.position.x += frame.time.x; return o; }";

/// Bends the model in the wind: its top sways along x, its base holds.
const SWAY: &str = "fn sway(height: f32) -> f32 { return sin(frame.time.x * 5.0) * 0.6 * height; }
fn vertex(v: Vertex) -> Vertex { var o = v; o.position.x += sway(v.position.y - v.origin.y); return o; }";

/// Shifts each buffer element's model by the element's `xyz`.
const OFFSETS: &str =
    "fn vertex(v: Vertex) -> Vertex { var o = v; o.position += items[v.instance_index].xyz; return o; }";

/// An offscreen renderer drawing glowing models under TAA, with the frame
/// kept between frames as the game keeps it.
struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer3D,
    textures: Vec<crate::render::gpu::GpuTexture>,
    output: wgpu::Texture,
    ctx: Ctx,
    frame: Frame3D,
    /// `frame.time.x` of the next frame.
    time: f32,
}

impl Rig {
    /// `control`: shaders made by this rig read this frame's values only and
    /// nothing is drawn for still models (the behaviour before).
    fn new(control: bool) -> Option<Self> {
        let gpu = support::gpu_with(|_| Ok(wgpu::DeviceDescriptor::default()))?;
        let (device, queue) = (gpu.device, gpu.queue);
        if !support::needs(&device, GpuFeature::TemporalAntialiasing) {
            return None;
        }
        NO_VERTEX_PAST.set(control);
        let (layout, white, textures) = support::white_textures(&device, &queue);
        let output = support::target(&device, (SIZE, SIZE), wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::splat(SIZE as f32));
        let renderer = Renderer3D::new(&device, &layout, output.format(), 0, &white, 4, true);
        ctx.scene3d.camera.position = vec3(0.0, 1.0, 6.0);
        ctx.scene3d.camera.look_at(vec3(0.0, 1.0, 0.0));
        ctx.scene3d.lighting.shadows = false;
        ctx.scene3d.lighting.sun_intensity = 0.0;
        ctx.scene3d.lighting.ambient_intensity = 0.0;
        ctx.scene3d.sky.atmosphere = false;
        ctx.scene3d.post.bloom = 0.0;
        ctx.scene3d.post.tonemap = Tonemap::None;
        ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa;
        Some(Self {
            device,
            queue,
            renderer,
            textures,
            output,
            ctx,
            frame: Frame3D::default(),
            time: 0.0,
        })
    }

    /// A box 0.4 wide and 2 tall standing on the origin, glowing white,
    /// drawn with `shader` (`None`: the default one).
    fn mesh(&mut self, shader: Option<&str>) -> Mesh {
        let glow = MaterialDesc {
            color: Color::BLACK,
            roughness: 1.0,
            emissive: Color::WHITE,
            ..Default::default()
        };
        // Numbered alike on both sides: the context has no default material.
        let material = self.ctx.gfx.create_material(glow);
        // The renderer's shader id (the context here has no built-in one).
        let shader = shader.map(|code| crate::three::Shader {
            id: self.renderer.create_shader(&self.device, "test", code).unwrap(),
        });
        self.renderer.create_material(
            &self.device,
            &GpuMaterialDesc::new(&MaterialDesc { shader, ..glow }),
            &self.textures,
        );
        let cube = MeshData::cube(1.0);
        let mut data = cube.clone();
        for p in &mut data.positions {
            *p = vec3(p.x * 0.4, (p.y + 0.5) * 2.0, p.z * 0.4);
        }
        let mesh = self.ctx.gfx.create_mesh(&data, material);
        self.renderer
            .create_mesh(&self.device, &data, &[(0..data.indices.len() as u32, material)], true);
        mesh
    }

    /// `mesh` drawn once per element of a GPU buffer holding `offsets`, the
    /// buffer kept for motion (`keep`). `None` without vertex storage.
    fn buffered(&mut self, mesh: Mesh, offsets: &[Vec4], keep: bool) -> Option<GpuBuffer> {
        if !support::available(&self.device, GpuFeature::StorageBuffers) {
            return None;
        }
        let len = offsets.len() as u32;
        let b = self.ctx.gfx.create_gpu_buffer(len, offsets);
        self.renderer.create_gpu_buffer(&self.device, b.slot(), len, offsets);
        self.ctx.gfx.set_mesh_gpu_buffer(mesh, b, len);
        self.renderer.set_mesh_gpu_buffer(mesh.slot(), b.slot(), len);
        self.ctx.gfx.set_mesh_gpu_bounds(mesh, Vec3::ZERO, 4.0);
        if keep {
            self.ctx.gfx.set_gpu_buffer_motion(b, true);
            self.renderer
                .set_gpu_buffer_motion(&self.device, &self.queue, b.slot(), true);
        }
        Some(b)
    }

    /// Writes `values` from element 0 of `b`, as `Graphics::write_gpu_buffer` does.
    fn write(&mut self, b: GpuBuffer, values: &[Vec4]) {
        self.ctx.gfx.write_gpu_buffer(b, 0, values);
        self.renderer.write_gpu_buffer(&self.queue, b.slot(), 0, values);
    }

    /// A static batch of `mesh` at `places`, culled on the GPU or the CPU.
    fn batch(&mut self, mesh: Mesh, places: &[Vec3], gpu: bool) {
        let transforms: Vec<Mat4> = places.iter().map(|&p| Mat4::from_translation(p)).collect();
        let options = InstanceBatchOptions {
            culling: if gpu {
                BatchCulling::PreferGpu
            } else {
                BatchCulling::PreferCpu
            },
            ..Default::default()
        };
        let batch = self.ctx.gfx.create_instance_batch(mesh, &transforms, options);
        self.renderer.set_instance_batch(
            &self.device,
            &self.queue,
            batch.id,
            self.ctx.gfx.assets3d.instance_batches[batch.id as usize].as_ref(),
        );
    }

    /// Collects, prepares and draws a frame at `self.time` as `Ctx::render`
    /// does, then moves the time on by `step`.
    fn draw(&mut self, step: f32) -> Image {
        let ctx = &mut self.ctx;
        let taa = ctx.scene3d.post.anti_aliasing == AntiAliasing::Taa;
        self.frame.motion.enabled = taa;
        self.frame.motion.reset |= std::mem::take(&mut ctx.gfx.frame3d.motion.reset);
        self.frame.collect(
            &mut ctx.world,
            &ctx.gfx.assets3d,
            &SkinFrame::default(),
            &ctx.scene3d.camera,
            &ctx.scene3d.lighting,
        );
        let scene = Scene3D {
            frame: &self.frame,
            lighting: &ctx.scene3d.lighting,
            sky: &ctx.scene3d.sky,
            light: SkyLight {
                sun: Vec3::ZERO,
                sky: Vec3::ZERO,
                ground: Vec3::ZERO,
            },
            camera: ctx.scene3d.camera.position,
            post: &ctx.scene3d.post,
            time: (self.time, 1.0 / 60.0),
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
        self.frame.motion.reset = false;
        let target = self.output.create_view(&Default::default());
        let buffers = self
            .renderer
            .draw(&self.device, &target, wgpu::Color::BLACK, &self.textures, None);
        self.queue.submit(buffers);
        self.time += step;
        Image::read(&self.device, &self.queue, &self.output)
    }

    /// The pixel the camera sees `at` in.
    fn pixel(&self, at: Vec3) -> (u32, u32) {
        let p = self.ctx.scene3d.camera.view_proj().project_point3(at);
        (
            ((p.x * 0.5 + 0.5) * SIZE as f32) as u32,
            ((0.5 - p.y * 0.5) * SIZE as f32) as u32,
        )
    }

    /// The motion vector the last frame wrote where the camera sees `at`,
    /// `None` where it wrote none.
    fn motion_at(&self, at: Vec3) -> Option<Vec2> {
        let texture = self.renderer.velocity_texture()?;
        let bytes = support::read_texture(&self.device, &self.queue, texture);
        let (x, y) = self.pixel(at);
        let i = ((y * SIZE + x) * 4) as usize;
        let half = |k: usize| half::f16::from_le_bytes([bytes[i + k * 2], bytes[i + k * 2 + 1]]).to_f32();
        let v = vec2(half(0), half(1));
        (v != Vec2::ZERO).then_some(v)
    }

    /// The motion vector of a point that moved from `before` to `now`.
    fn expected(&self, before: Vec3, now: Vec3) -> Vec2 {
        let view_proj = self.ctx.scene3d.camera.view_proj();
        let (b, a) = (view_proj.project_point3(before), view_proj.project_point3(now));
        vec2((b.x - a.x) * 0.5, (a.y - b.y) * 0.5)
    }

    /// Whether the motion at `now` is that of a point coming from `before`.
    fn moved(&self, before: Vec3, now: Vec3) -> Result<(), String> {
        let expected = self.expected(before, now);
        match self.motion_at(now) {
            None => Err(format!("no motion at {now} (expected {expected:?})")),
            Some(got) if (got - expected).abs().max_element() > 0.002 => {
                Err(format!("motion {got:?} at {now} is not {expected:?}"))
            }
            Some(_) => Ok(()),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        NO_VERTEX_PAST.set(false);
    }
}

/// A point on the front face of the box at `x` (0.4 wide, 2 tall), at
/// height `y`.
fn front(x: f32, y: f32) -> Vec3 {
    vec3(x, y, 0.2)
}

/// A sliding box (time 0, 0, then 0.25) and a plain one beside it: the
/// slide's motion is written, the plain box writes none.
fn check_slide(control: bool) -> Option<Result<(), String>> {
    let mut rig = Rig::new(control)?;
    let slide = rig.mesh(Some(SLIDE));
    let plain = rig.mesh(None);
    rig.ctx
        .world
        .spawn((Transform3D::at(vec3(-1.0, 0.0, 0.0)), Model::new(slide)));
    rig.ctx
        .world
        .spawn((Transform3D::at(vec3(1.0, 0.0, 0.0)), Model::new(plain)));
    rig.draw(0.0);
    rig.draw(0.25);
    rig.draw(0.0);
    if let Some(m) = rig.motion_at(front(1.0, 1.0)) {
        return Some(Err(format!("a plain still box wrote motion {m:?}")));
    }
    Some(rig.moved(front(-1.0, 1.0), front(-0.75, 1.0)))
}

#[test]
fn a_vertex_function_moving_with_time_writes_its_motion() {
    let (Some(with), Some(control)) = (check_slide(false), check_slide(true)) else {
        return;
    };
    with.unwrap();
    assert!(
        control.is_err(),
        "the control must fail: `vertex` saw this frame's time twice"
    );
}

/// The sliding box also moves up 0.2 by its matrix in the same frame: the
/// motion is both.
fn check_slide_and_move(control: bool) -> Option<Result<(), String>> {
    let mut rig = Rig::new(control)?;
    let slide = rig.mesh(Some(SLIDE));
    let e = rig.ctx.world.spawn((Transform3D::default(), Model::new(slide)));
    rig.draw(0.0);
    rig.draw(0.25);
    rig.ctx.world.get_mut::<Transform3D>(e).unwrap().position.y = 0.2;
    rig.draw(0.0);
    Some(rig.moved(front(0.0, 1.0), front(0.25, 1.2)))
}

#[test]
fn a_moving_model_adds_its_matrix_motion_to_its_vertex_motion() {
    let (Some(with), Some(control)) = (check_slide_and_move(false), check_slide_and_move(true)) else {
        return;
    };
    with.unwrap();
    assert!(control.is_err(), "the control writes the matrix motion only");
}

/// Two sliding boxes in a static batch, on the GPU-culled or the CPU path.
fn check_batch(gpu: bool, control: bool) -> Option<Result<(), String>> {
    let mut rig = Rig::new(control)?;
    let slide = rig.mesh(Some(SLIDE));
    rig.batch(slide, &[vec3(-1.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0)], gpu);
    rig.draw(0.0);
    rig.draw(-0.25);
    rig.draw(0.0);
    let left = rig.moved(front(-1.0, 1.0), front(-1.25, 1.0));
    Some(left.and_then(|_| rig.moved(front(1.0, 1.0), front(0.75, 1.0))))
}

#[test]
fn static_batches_write_the_motion_of_their_vertex_function() {
    for gpu in [true, false] {
        let (Some(with), Some(control)) = (check_batch(gpu, false), check_batch(gpu, true)) else {
            return;
        };
        with.unwrap_or_else(|e| panic!("culled on the {}: {e}", if gpu { "GPU" } else { "CPU" }));
        assert!(control.is_err(), "the control writes no motion for batches");
    }
}

/// Two boxes from a GPU buffer: the first element moves 0.3 right in a
/// frame (written from the CPU), the second holds still. `keep`: the
/// buffer keeps its last frame.
fn check_buffer(keep: bool) -> Option<Result<(), String>> {
    let mut rig = Rig::new(false)?;
    let mesh = rig.mesh(Some(OFFSETS));
    let at = [vec4(-1.0, 0.0, 0.0, 0.0), vec4(1.0, 0.0, 0.0, 0.0)];
    let b = rig.buffered(mesh, &at, keep)?;
    rig.ctx.world.spawn((Transform3D::default(), Model::new(mesh)));
    rig.draw(1.0 / 60.0);
    rig.draw(1.0 / 60.0);
    rig.write(b, &[vec4(-0.7, 0.0, 0.0, 0.0)]);
    rig.draw(1.0 / 60.0);
    if let Some(m) = rig.motion_at(front(1.0, 1.0)) {
        return Some(Err(format!("the still element wrote motion {m:?}")));
    }
    let first = rig.moved(front(-1.0, 1.0), front(-0.7, 1.0));
    // The frame after, it holds still at its new place.
    rig.draw(1.0 / 60.0);
    if let Some(m) = rig.motion_at(front(-0.7, 1.0)) {
        return Some(Err(format!("the element that stopped wrote motion {m:?}")));
    }
    Some(first)
}

#[test]
fn a_buffer_that_keeps_its_last_frame_gives_each_element_its_motion() {
    let (Some(with), Some(without)) = (check_buffer(true), check_buffer(false)) else {
        return;
    };
    with.unwrap();
    assert!(
        without.is_err(),
        "a buffer without its last frame has no element motion"
    );
}

#[test]
fn turning_buffer_motion_on_and_off_keeps_the_elements() {
    let Some(mut rig) = Rig::new(false) else {
        return;
    };
    let mesh = rig.mesh(Some(OFFSETS));
    let Some(b) = rig.buffered(mesh, &[vec4(-1.0, 0.0, 0.0, 0.0)], false) else {
        return;
    };
    rig.ctx.world.spawn((Transform3D::default(), Model::new(mesh)));
    rig.draw(0.0);
    rig.draw(0.0);
    let size = rig.renderer.residency().3;
    // On: twice the memory, and no motion from turning it on.
    rig.renderer
        .set_gpu_buffer_motion(&rig.device, &rig.queue, b.slot(), true);
    assert_eq!(rig.renderer.residency().3, size * 2);
    rig.draw(0.0);
    assert_eq!(rig.motion_at(front(-1.0, 1.0)), None);
    assert!(
        rig.draw(0.0)
            .pixel(rig.pixel(front(-1.0, 1.0)).0, rig.pixel(front(-1.0, 1.0)).1)[0]
            > 200
    );
    // Off again: the memory back, the element where it was.
    rig.renderer
        .set_gpu_buffer_motion(&rig.device, &rig.queue, b.slot(), false);
    assert_eq!(rig.renderer.residency().3, size);
    let image = rig.draw(0.0);
    let (x, y) = rig.pixel(front(-1.0, 1.0));
    assert!(image.pixel(x, y)[0] > 200, "the element is still drawn where it was");
}

/// A sliding box drawn for its first frame, after a history reset, and
/// after TAA was off for a frame: none of these has a past, so none has
/// motion; the frame after each does.
#[test]
fn the_first_frame_a_reset_and_taa_turned_on_have_no_vertex_motion() {
    let Some(mut rig) = Rig::new(false) else {
        return;
    };
    let slide = rig.mesh(Some(SLIDE));
    rig.ctx.world.spawn((Transform3D::default(), Model::new(slide)));
    // Each frame is drawn at `rig.time`, which then moves on.
    rig.draw(0.25);
    assert_eq!(rig.motion_at(front(0.0, 1.0)), None, "the first frame");
    rig.draw(0.25);
    rig.moved(front(0.0, 1.0), front(0.25, 1.0)).unwrap();
    rig.ctx.gfx.reset_temporal_history();
    rig.draw(0.25);
    assert_eq!(rig.motion_at(front(0.5, 1.0)), None, "a reset");
    rig.draw(0.25);
    rig.moved(front(0.5, 1.0), front(0.75, 1.0)).unwrap();
    rig.ctx.scene3d.post.anti_aliasing = AntiAliasing::None;
    rig.draw(0.25);
    assert!(
        !rig.renderer.screen.still_motion,
        "nothing is drawn for motion with TAA off"
    );
    rig.ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa;
    rig.draw(-1.0);
    assert_eq!(rig.motion_at(front(1.25, 1.0)), None, "TAA turned back on");
    rig.draw(0.0);
    rig.moved(front(1.25, 1.0), front(0.25, 1.0)).unwrap();
}

#[test]
fn materials_without_a_moving_vertex_function_draw_nothing_more() {
    let Some(mut rig) = Rig::new(false) else {
        return;
    };
    // Its `vertex` moves vertices, but not over time; `surface` animates.
    let fixed = "fn vertex(v: Vertex) -> Vertex { var o = v; o.position.y += 0.1; return o; }
        fn surface(s: Surface) -> Surface { var o = s; o.emissive *= 0.5 + 0.5 * sin(frame.time.x); return o; }";
    let mesh = rig.mesh(Some(fixed));
    let plain = rig.mesh(None);
    rig.ctx
        .world
        .spawn((Transform3D::at(vec3(-1.0, 0.0, 0.0)), Model::new(mesh)));
    rig.ctx
        .world
        .spawn((Transform3D::at(vec3(1.0, 0.0, 0.0)), Model::new(plain)));
    for _ in 0..3 {
        rig.draw(0.25);
    }
    assert!(
        rig.renderer
            .shaders
            .sets
            .iter()
            .all(|s| s.variant(ShaderVariant::ALL[0]).velocity_still.is_none()),
        "no still motion pipeline is built"
    );
    assert!(
        !rig.renderer.screen.still_motion,
        "the motion pass draws no still models"
    );
    assert_eq!(rig.motion_at(front(-1.0, 1.1)), None);
    assert_eq!(rig.motion_at(front(1.0, 1.0)), None);
}

/// Puts the camera close to the box's top.
fn close_up(rig: &mut Rig) {
    rig.ctx.scene3d.camera.position = vec3(0.0, 1.6, 3.0);
    rig.ctx.scene3d.camera.look_at(vec3(0.0, 1.6, 0.0));
}

/// How far the last frame of a box swaying in the wind is from the same
/// pose held until TAA settled: the summed and the largest difference of
/// a pixel (0..1, red). Foliage whose motion TAA misses smears along its
/// edges. With `PONKO_RENDER_TEST_IMAGES=1` both frames are kept as images
/// (the sway's, then the settled one).
fn sway_error(control: bool) -> Option<(f32, f32)> {
    let step = 1.0 / 30.0;
    let mut rig = Rig::new(control)?;
    let mesh = rig.mesh(Some(SWAY));
    rig.ctx.world.spawn((Transform3D::default(), Model::new(mesh)));
    close_up(&mut rig);
    let mut last = None;
    for _ in 0..16 {
        last = Some(rig.draw(step));
    }
    let last = last.unwrap();
    support::keep_image(&last);
    let at = rig.time - step;
    drop(rig);
    let mut settled = Rig::new(false)?;
    let mesh = settled.mesh(Some(SWAY));
    settled.ctx.world.spawn((Transform3D::default(), Model::new(mesh)));
    close_up(&mut settled);
    settled.time = at;
    for _ in 0..24 {
        settled.draw(0.0);
    }
    let settled = settled.draw(0.0);
    support::keep_image(&settled);
    let (mut sum, mut worst) = (0.0f32, 0.0f32);
    for (a, b) in last.rgba.as_chunks::<4>().0.iter().zip(settled.rgba.as_chunks::<4>().0) {
        let d = (f32::from(a[0]) - f32::from(b[0])).abs() / 255.0;
        sum += d;
        worst = worst.max(d);
    }
    eprintln!(
        "vertex motion {}: summed difference {sum:.1}, largest {worst:.3}",
        !control
    );
    Some((sum, worst))
}

#[test]
fn swaying_foliage_stays_closer_to_its_settled_image_with_vertex_motion() {
    let (Some(before), Some(after)) = (sway_error(true), sway_error(false)) else {
        return;
    };
    // On an Apple M4 Max: about 114 and 0.89 without, 55 and 0.77 with.
    // The control is the old behaviour, which must be clearly worse.
    assert!(
        after.0 < before.0 * 0.75 && after.1 <= before.1,
        "with vertex motion {after:?}, without {before:?}"
    );
}
