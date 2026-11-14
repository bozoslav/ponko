//! Motion vectors of skinned and morphing models drawn offscreen under
//! TAA: a limb that bends (or a morph target that moves part of a mesh)
//! writes the motion of its own corners, on the compute and the CPU
//! skinning paths, while the part that holds still writes none. Teleports
//! reset with `Graphics::reset_motion` and models that come back into view
//! start with no motion. The control frames drop the last frame's poses
//! (the old behaviour: the model moved only by its matrix) and must fail
//! the same checks. Skips without a GPU or without TAA.
use super::*;
use crate::anim::{Joint, MorphTarget, MorphWeights, Skeleton};
use crate::d3::Transform as Transform3D;
use crate::prelude::*;
use crate::render::render_test_support::{self as support, Image};
use crate::render::{AntiAliasing, GpuFeature, MaterialDesc, MeshData, Model, Tonemap};
use crate::three::{Frame3D, GpuMaterialDesc, Scene3D, SkyLight};

const SIZE: u32 = 128;

/// Where the joint of the upper box turns (and the two boxes meet).
const PIVOT: Vec3 = Vec3::new(0.0, 1.0, 0.0);

/// A renderer drawing a glowing bar of two boxes (the lower on joint 0,
/// the upper on joint 1; or, `morph`, the upper moved by a morph target)
/// under TAA, with the frame and the skin poses kept between frames as
/// the game keeps them.
struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer3D,
    textures: Vec<crate::render::gpu::GpuTexture>,
    output: wgpu::Texture,
    ctx: Ctx,
    frame: Frame3D,
    bar: ponko_ecs::Entity,
    morph: bool,
    /// Whether the last frame's poses are kept: off for the control.
    keep_poses: bool,
}

/// Two boxes 0.3 wide stacked into a bar 2 tall standing on the origin.
fn two_boxes(morph: bool) -> MeshData {
    let cube = MeshData::cube(1.0);
    let mut bar = MeshData::default();
    for (joint, y) in [(0u16, 0.0f32), (1, 1.0)] {
        let base = bar.positions.len() as u32;
        bar.positions
            .extend(cube.positions.iter().map(|p| vec3(p.x * 0.3, p.y + 0.5 + y, p.z * 0.3)));
        bar.normals.extend_from_slice(&cube.normals);
        bar.uvs.extend_from_slice(&cube.uvs);
        bar.indices.extend(cube.indices.iter().map(|i| i + base));
        if !morph {
            bar.joints
                .extend(std::iter::repeat_n([joint, 0, 0, 0], cube.positions.len()));
            bar.weights
                .extend(std::iter::repeat_n([1.0, 0.0, 0.0, 0.0], cube.positions.len()));
        }
    }
    if morph {
        // "shift": the upper box (the corners after the lower box's) goes a
        // unit left.
        let n = cube.positions.len();
        bar.morph_targets.push(MorphTarget {
            name: "shift".into(),
            positions: (0..2 * n)
                .map(|i| if i >= n { vec3(-1.0, 0.0, 0.0) } else { Vec3::ZERO })
                .collect(),
            normals: Vec::new(),
        });
    }
    bar
}

impl Rig {
    /// `cpu`: pose on the CPU (the fallback without GPU skinning), with
    /// TAA still on.
    fn new(morph: bool, cpu: bool) -> Option<Self> {
        let gpu = support::gpu_with(|_| Ok(wgpu::DeviceDescriptor::default()))?;
        let (device, queue) = (gpu.device, gpu.queue);
        if !support::needs(&device, GpuFeature::TemporalAntialiasing) {
            return None;
        }
        let (layout, white, textures) = support::white_textures(&device, &queue);
        let output = support::target(&device, (SIZE, SIZE), wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::splat(SIZE as f32));
        let glow = MaterialDesc {
            color: Color::BLACK,
            roughness: 1.0,
            emissive: Color::WHITE,
            ..Default::default()
        };
        let material = ctx.gfx.create_material(glow);
        let data = two_boxes(morph);
        let skeleton = Skeleton::new(vec![
            Joint {
                name: "base".into(),
                parent: None,
                rest: Transform3D::default(),
            },
            Joint {
                name: "top".into(),
                parent: Some(0),
                rest: Transform3D::at(PIVOT),
            },
        ]);
        let bar = if morph {
            let mesh = ctx.gfx.create_mesh(&data, material);
            ctx.world
                .spawn((Transform3D::default(), Model::new(mesh), MorphWeights::default()))
        } else {
            let mesh = ctx.gfx.create_skinned_mesh(&data, material, &skeleton);
            ctx.world.spawn((Transform3D::default(), Model::new(mesh), skeleton))
        };
        let mut renderer = Renderer3D::new(&device, &layout, output.format(), 0, &white, 4, true);
        if cpu {
            renderer.skinning = super::skinning::Skinning::new(&device, false);
        }
        renderer.create_material(&device, &GpuMaterialDesc::new(&glow), &textures);
        let mut weighted = data.clone();
        for w in &mut weighted.weights {
            *w = crate::three::normalized_weights(*w);
        }
        renderer.create_mesh(&device, &weighted, &[(0..data.indices.len() as u32, material)], true);
        ctx.scene3d.camera.position = vec3(0.0, 1.0, 6.0);
        ctx.scene3d.camera.look_at(PIVOT);
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
            bar,
            morph,
            keep_poses: true,
        })
    }

    /// Bends the upper box: turns joint 1 by `angle` about z, or (`morph`)
    /// sets the morph weight to `angle`.
    fn bend(&mut self, angle: f32) {
        if self.morph {
            self.ctx.world.get_mut::<MorphWeights>(self.bar).unwrap().set(0, angle);
        } else {
            self.ctx.world.get_mut::<Skeleton>(self.bar).unwrap().pose[1].rotation = Quat::from_rotation_z(angle);
        }
    }

    /// Where the upper box's point that is at `rest` with no bend is with
    /// the bend `angle`.
    fn bent(&self, rest: Vec3, angle: f32) -> Vec3 {
        if self.morph {
            rest + vec3(-angle, 0.0, 0.0)
        } else {
            PIVOT + Quat::from_rotation_z(angle) * (rest - PIVOT)
        }
    }

    /// Collects, prepares and draws a frame as `Ctx::render` does.
    fn draw(&mut self) -> Image {
        let ctx = &mut self.ctx;
        let mut skins = std::mem::take(&mut self.frame.skins);
        skins.motion = self.keep_poses;
        skins.collect(&mut ctx.world, &ctx.gfx.assets3d);
        self.frame.motion.enabled = true;
        // What `Graphics::reset_motion` asked for since the last frame.
        self.frame.motion.forget.append(&mut ctx.gfx.frame3d.motion.forget);
        self.frame.collect(
            &mut ctx.world,
            &ctx.gfx.assets3d,
            &skins,
            &ctx.scene3d.camera,
            &ctx.scene3d.lighting,
        );
        self.frame.skins = skins;
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
        self.frame.motion.reset = false;
        let target = self.output.create_view(&Default::default());
        let buffers = self
            .renderer
            .draw(&self.device, &target, wgpu::Color::BLACK, &self.textures, None);
        self.queue.submit(buffers);
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

    /// The motion vector a point that moved from `before` to `now` (world)
    /// has: where it was, from where it is, in texture coordinates.
    fn expected(&self, before: Vec3, now: Vec3) -> Vec2 {
        let view_proj = self.ctx.scene3d.camera.view_proj();
        let (b, a) = (view_proj.project_point3(before), view_proj.project_point3(now));
        vec2((b.x - a.x) * 0.5, (a.y - b.y) * 0.5)
    }
}

/// A point on the front face of the upper box, and one on the lower box's.
const UPPER: Vec3 = Vec3::new(0.0, 1.6, 0.15);
const LOWER: Vec3 = Vec3::new(0.0, 0.5, 0.15);

/// Bends the upper box from `from` to `to` in one frame (after two still
/// frames at `from`) and checks the motion vectors: the upper box's point
/// moves as it does, the lower box's not at all. `Err` says what did not
/// hold.
fn check_bend(rig: &mut Rig, from: f32, to: f32) -> Result<(), String> {
    rig.bend(from);
    rig.draw();
    rig.draw();
    rig.bend(to);
    rig.draw();
    let (before, now) = (rig.bent(UPPER, from), rig.bent(UPPER, to));
    let expected = rig.expected(before, now);
    let got = rig.motion_at(now);
    eprintln!("upper box: motion {got:?}, expected {expected:?}");
    let Some(got) = got else {
        return Err(format!("the bending limb wrote no motion (expected {expected:?})"));
    };
    // Half a pixel off where the point is, the motion differs by the
    // rotation over half a pixel: far less than this.
    if (got - expected).abs().max_element() > 0.002 {
        return Err(format!("the limb's motion {got:?} is not its movement {expected:?}"));
    }
    if let Some(lower) = rig.motion_at(LOWER) {
        return Err(format!("the part that holds still wrote motion {lower:?}"));
    }
    Ok(())
}

fn bending_limb(morph: bool, cpu: bool) {
    let Some(mut rig) = Rig::new(morph, cpu) else {
        return;
    };
    assert_eq!(
        rig.renderer.skinning.uses_compute(),
        !cpu && support::available(&rig.device, GpuFeature::GpuSkinning)
    );
    check_bend(&mut rig, 0.0, 0.25).unwrap();
    // A second bend, from a bent pose: the history follows.
    check_bend(&mut rig, 0.25, 0.1).unwrap();
    // Control: without the last frame's poses, the bar moved only by its
    // matrix (not at all), so the limb writes no motion: the check fails.
    let mut control = Rig::new(morph, cpu).unwrap();
    control.keep_poses = false;
    let failed = check_bend(&mut control, 0.0, 0.25);
    assert!(
        failed.is_err(),
        "the control must fail: without pose history the limb has no motion"
    );
    rig.renderer.check_mirrors(&rig.device, &rig.queue);
}

#[test]
fn a_bending_limb_writes_the_motion_of_its_corners() {
    bending_limb(false, false);
}

#[test]
fn a_moving_morph_target_writes_the_motion_of_its_corners() {
    bending_limb(true, false);
}

#[test]
fn the_cpu_skinning_fallback_writes_the_same_motion() {
    bending_limb(false, true);
    bending_limb(true, true);
}

#[test]
fn a_held_pose_writes_no_motion() {
    let Some(mut rig) = Rig::new(false, false) else {
        return;
    };
    rig.bend(0.3);
    for _ in 0..3 {
        rig.draw();
    }
    assert_eq!(rig.motion_at(rig.bent(UPPER, 0.3)), None, "a bent pose held still");
    assert_eq!(rig.motion_at(LOWER), None);
}

#[test]
fn a_teleport_with_reset_motion_writes_no_motion_for_that_model() {
    let Some(mut rig) = Rig::new(false, false) else {
        return;
    };
    let step = |rig: &mut Rig, reset: bool| {
        rig.bend(0.0);
        rig.ctx.world.get_mut::<Transform3D>(rig.bar).unwrap().position = Vec3::ZERO;
        rig.draw();
        rig.draw();
        // A jump to the side and a snap to another pose in one frame.
        rig.ctx.world.get_mut::<Transform3D>(rig.bar).unwrap().position = vec3(0.5, 0.0, 0.0);
        rig.bend(0.4);
        if reset {
            rig.ctx.gfx.reset_motion(rig.bar);
        }
        rig.draw();
        let shift = vec3(0.5, 0.0, 0.0);
        (
            rig.motion_at(rig.bent(UPPER, 0.4) + shift),
            rig.motion_at(LOWER + shift),
        )
    };
    // Negative control: without the reset, the jump reads as fast motion.
    let (upper, lower) = step(&mut rig, false);
    assert!(
        upper.is_some() && lower.is_some(),
        "a jump is motion: {upper:?} {lower:?}"
    );
    let (upper, lower) = step(&mut rig, true);
    assert_eq!((upper, lower), (None, None), "a teleport with a reset has no motion");
    // The frame after, the motion is measured from where the reset frame
    // drew it.
    rig.bend(0.2);
    rig.draw();
    let shift = vec3(0.5, 0.0, 0.0);
    let expected = rig.expected(rig.bent(UPPER, 0.4) + shift, rig.bent(UPPER, 0.2) + shift);
    let got = rig.motion_at(rig.bent(UPPER, 0.2) + shift).expect("motion resumes");
    assert!((got - expected).abs().max_element() < 0.002, "{got:?} vs {expected:?}");
}

#[test]
fn a_model_that_comes_back_into_view_starts_with_no_motion() {
    let Some(mut rig) = Rig::new(false, false) else {
        return;
    };
    rig.draw();
    rig.draw();
    // Out of view for a frame (its model taken away), bent meanwhile.
    let model = rig.ctx.world.remove::<Model>(rig.bar).unwrap();
    rig.bend(0.2);
    rig.draw();
    rig.bend(0.4);
    rig.draw();
    rig.ctx.world.insert(rig.bar, model);
    rig.bend(0.5);
    rig.draw();
    assert_eq!(rig.motion_at(rig.bent(UPPER, 0.5)), None, "no past: no motion");
    // From then on it has a past.
    rig.bend(0.3);
    rig.draw();
    let expected = rig.expected(rig.bent(UPPER, 0.5), rig.bent(UPPER, 0.3));
    let got = rig.motion_at(rig.bent(UPPER, 0.3)).expect("motion the frame after");
    assert!((got - expected).abs().max_element() < 0.002, "{got:?} vs {expected:?}");
}

/// Puts the camera close to the limb, for the swing.
fn close_up(rig: &mut Rig) {
    rig.ctx.scene3d.camera.position = vec3(0.0, 1.4, 3.0);
    rig.ctx.scene3d.camera.look_at(vec3(0.0, 1.4, 0.0));
}

/// How far the last frame of a running swing (the limb turning 0.2 a
/// frame) is from the same pose held until TAA settled: the summed and the
/// largest difference of a pixel (0..1, red). A limb whose motion TAA
/// misses smears along its edges. With `PONKO_RENDER_TEST_IMAGES=1` both
/// frames are kept as images (the swing's, then the settled one).
fn swing_error(keep_poses: bool) -> Option<(f32, f32)> {
    let mut rig = Rig::new(false, false)?;
    rig.keep_poses = keep_poses;
    close_up(&mut rig);
    // Still for a while, so the history holds the bar standing up.
    for _ in 0..8 {
        rig.draw();
    }
    let (step, frames) = (0.2, 6);
    let mut last = None;
    for k in 1..=frames {
        rig.bend(step * k as f32);
        last = Some(rig.draw());
    }
    let last = last.unwrap();
    support::keep_image(&last);
    let mut settled = Rig::new(false, false)?;
    close_up(&mut settled);
    settled.bend(step * frames as f32);
    for _ in 0..24 {
        settled.draw();
    }
    let settled = settled.draw();
    support::keep_image(&settled);
    let (mut sum, mut worst) = (0.0f32, 0.0f32);
    for (a, b) in last.rgba.as_chunks::<4>().0.iter().zip(settled.rgba.as_chunks::<4>().0) {
        let d = (f32::from(a[0]) - f32::from(b[0])).abs() / 255.0;
        sum += d;
        worst = worst.max(d);
    }
    eprintln!("pose history {keep_poses}: summed difference {sum:.1}, largest {worst:.3}");
    Some((sum, worst))
}

#[test]
fn a_swinging_limb_stays_closer_to_its_settled_image_with_pose_history() {
    let (Some(before), Some(after)) = (swing_error(false), swing_error(true)) else {
        return;
    };
    // On this adapter: about 89 and 0.85 without, 44 and 0.48 with. The
    // control is the old behaviour, which must be clearly worse.
    assert!(
        after.0 < before.0 * 0.75 && after.1 < before.1,
        "with pose history {after:?}, without {before:?}"
    );
}
