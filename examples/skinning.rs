//! Procedural tentacles: `cargo run --example skinning --features 3d`.
//! Escape quits. All visible geometry is built in code.
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::anim::Joint;
use ponko::anim::{
    AnimationCondition, AnimationGraph, AnimationLayer, AnimationParameter, AnimationState, AnimationTrack,
    AnimationTransition, Animator, AttachToJoint, Interpolation, JointAnimation, Motion, ParamId, Skeleton, SkinOf,
    StateId,
};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{MaterialDesc, MeshData};

const JOINTS: usize = 8;
const STEP: f32 = 0.5;
#[derive(Component)]
struct Sway(f32);
#[derive(Component)]
struct Bend(ponko::anim::AnimationClip);

// ANCHOR: skin_chain
fn chain() -> Skeleton {
    Skeleton::new(
        (0..JOINTS)
            .map(|i| Joint {
                name: format!("joint_{i}"),
                parent: i.checked_sub(1),
                rest: Transform::at(Vec3::Y * if i == 0 { 0.0 } else { STEP }),
            })
            .collect(),
    )
}
// ANCHOR_END: skin_chain

// A cylinder (including caps), subdivided along +Y. Also used for the band.
// ANCHOR: skin_geometry
fn tube(bottom: f32, top: f32, radius: f32) -> MeshData {
    const SLICES: usize = 12;
    const ROWS: usize = 28;
    let mut data = MeshData::default();
    for row in 0..=ROWS {
        let y = bottom + (top - bottom) * row as f32 / ROWS as f32;
        let along = (y / STEP).clamp(0.0, (JOINTS - 1) as f32);
        let lower = (along.floor() as usize).min(JOINTS - 2);
        let blend = along - lower as f32;
        for slice in 0..SLICES {
            let angle = std::f32::consts::TAU * slice as f32 / SLICES as f32;
            data.positions.push(vec3(radius * angle.cos(), y, radius * angle.sin()));
            data.joints.push([lower as u16, (lower + 1) as u16, 0, 0]);
            data.weights.push([1.0 - blend, blend, 0.0, 0.0]);
        }
    }
    for row in 0..ROWS {
        for slice in 0..SLICES {
            let a = (row * SLICES + slice) as u32;
            let b = (row * SLICES + (slice + 1) % SLICES) as u32;
            let c = a + SLICES as u32;
            let d = b + SLICES as u32;
            data.indices.extend([a, c, b, b, c, d]);
        }
    }
    for slice in 1..SLICES - 1 {
        let last = (ROWS * SLICES) as u32;
        data.indices.extend([0, slice as u32, (slice + 1) as u32]);
        data.indices
            .extend([last, last + (slice + 1) as u32, last + slice as u32]);
    }
    data
}
// ANCHOR_END: skin_geometry

// ANCHOR: skin_pose
fn sway(ctx: &mut Ctx) {
    let time = ctx.time.elapsed() as f32;
    ctx.world.query::<(&mut Skeleton, &Sway)>().for_each(|(s, phase)| {
        for (i, pose) in s.pose.iter_mut().enumerate() {
            pose.rotation = Quat::from_rotation_z((time * 1.5 + phase.0 - i as f32 * 0.45).sin() * 0.16);
        }
    });
    ctx.world.query::<(&mut Skeleton, &Bend)>().for_each(|(s, bend)| {
        bend.0.sample_blended(time.rem_euclid(bend.0.duration()), 0.5, s);
    });
    ctx.world.query::<&mut Animator>().for_each(|a| {
        a.set_bool(ParamId(0), ((time / 3.0) as u32).is_multiple_of(2));
    });
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}
// ANCHOR_END: skin_pose

// Compiled book example; the procedural demo never loads asset files.
#[allow(dead_code)]
fn load_character(ctx: &mut Ctx) {
    // ANCHOR: skin_load
    let mesh = ctx.gfx.load_mesh("models/character.glb");
    let skeleton = ctx.gfx.load_skeleton("models/character.glb");
    let hand = skeleton.find("hand_r").expect("model needs hand_r");
    let character = ctx.world.spawn((Transform::default(), Model::new(mesh), skeleton));
    let skeleton = ctx.world.get_mut::<Skeleton>(character).unwrap();
    skeleton.pose[hand].rotation = Quat::from_rotation_y(0.3);
    let local_hand = skeleton.joint_matrix(hand);
    let transform = ctx.world.get::<Transform>(character).unwrap();
    let world_hand = (transform.matrix() * local_hand).transform_point3(Vec3::ZERO);
    println!("Hand at {world_hand}");
    // ANCHOR_END: skin_load
}

// ANCHOR: clip_code
fn bend_clip(skeleton: &Skeleton) -> ponko::anim::AnimationClip {
    let rotation = AnimationTrack::new(
        vec![0.0, 1.0, 2.0],
        [0.0, 0.3, 0.0]
            .map(|angle| Vec4::from_array(Quat::from_rotation_z(angle).to_array()))
            .to_vec(),
        Interpolation::Linear,
    );
    ponko::anim::AnimationClip::new(
        "bend",
        skeleton,
        vec![JointAnimation {
            joint: 1,
            name: "joint_1".into(),
            translation: None,
            rotation: Some(rotation),
            scale: None,
        }],
    )
}
// ANCHOR_END: clip_code

#[allow(dead_code)]
fn play_clips(ctx: &mut Ctx) {
    // ANCHOR: clip_play
    let mut skeleton = ctx.gfx.load_skeleton("models/character.glb");
    let walk = ctx.gfx.load_clip("models/character.glb", "walk");
    let run = ctx.gfx.load_clip("models/character.glb", "run");
    let time = ctx.time.elapsed() as f32;
    let loop_time = |clip: &ponko::anim::AnimationClip| {
        if clip.duration() > 0.0 {
            time.rem_euclid(clip.duration())
        } else {
            0.0
        }
    };
    skeleton.reset_pose();
    walk.sample(loop_time(&walk), &mut skeleton);
    run.sample_blended(loop_time(&run), 0.5, &mut skeleton);
    // Keep clips and the skeleton as components/resources for subsequent frames.
    // ANCHOR_END: clip_play
}

// ANCHOR: animator_graph
fn bend_animator(skeleton: &Skeleton) -> Animator {
    let idle = ponko::anim::AnimationClip::new("idle", skeleton, vec![]);
    let bend = bend_clip(skeleton);
    let states = vec![
        AnimationState::new(Motion::Clip(idle)),
        AnimationState::new(Motion::Clip(bend)),
    ];
    let transition = |from, to, active| {
        AnimationTransition::new(from, to)
            .with_condition(AnimationCondition::Bool(ParamId(0), active))
            .with_duration(0.12)
    };
    let graph = AnimationGraph::new(
        vec![AnimationParameter::Bool(false)],
        states,
        vec![
            transition(StateId(0), StateId(1), true),
            transition(StateId(1), StateId(0), false),
        ],
        StateId(0),
    )
    .expect("a valid graph");
    Animator::new(graph)
}
// ANCHOR_END: animator_graph

#[allow(dead_code)]
fn animate_now() {
    // ANCHOR: animator_update
    let mut skeleton = chain();
    let mut animator = bend_animator(&skeleton);
    animator.manual = true;
    animator.set_bool(ParamId(0), true);
    animator.update(0.016, &mut skeleton);
    // Or spawn (Animator, Skeleton) together for automatic updates.
    animator.crossfade(StateId(0), 0.12, 0.0);
    animator.update(0.016, &mut skeleton);
    for name in animator.drain_events() {
        println!("Animation event: {name}");
    }
    // ANCHOR_END: animator_update
}

#[allow(dead_code)]
fn layers_and_root_motion(skeleton: &Skeleton, wave: ponko::anim::AnimationClip) -> Animator {
    // ANCHOR: animator_layers
    let mut animator = bend_animator(skeleton);
    let waving = AnimationGraph::new(
        vec![],
        vec![AnimationState::new(Motion::Clip(wave))],
        vec![],
        StateId(0),
    )
    .expect("a valid graph");
    let mut layer = AnimationLayer::new(waving);
    layer.mask = ponko::anim::JointMask::descendants(skeleton, "joint_3"); // that joint and its children
    layer.weight = 0.8;
    let wave = animator.add_layer(layer);
    animator.layer_mut(wave).weight = 0.5; // fade it any time; its graph has its own parameters
    // The clip's own steps move the entity, instead of sliding in place.
    animator.root_joint = 0;
    animator.apply_root_motion = true;
    // ANCHOR_END: animator_layers
    animator
}

fn main() {
    Game::new("Ponko: skinned meshes", 1000, 700)
        .system(sway)
        .setup(|ctx| {
            ctx.scene3d.camera.position = vec3(6.0, 4.0, 9.0);
            ctx.scene3d.camera.look_at(vec3(0.0, 1.7, 0.0));
            let green = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x398F79)));
            let gold = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0xF2BD50)));
            let white = ctx.gfx.default_material();
            let ground = ctx.gfx.create_mesh(&MeshData::plane(20.0), white);
            ctx.world.spawn((Transform::default(), Model::new(ground)));
            // ANCHOR: skin_create
            let skeleton = chain();
            let mesh = ctx.gfx.create_skinned_mesh(&tube(0.0, 3.5, 0.22), green, &skeleton);
            // A separate skeleton with matching names demonstrates name binding.
            let band = ctx.gfx.create_skinned_mesh(&tube(2.2, 2.5, 0.25), gold, &chain());
            // ANCHOR_END: skin_create
            let ball = ctx.gfx.create_mesh(&MeshData::sphere(0.18, 16), gold);
            let bend = bend_clip(&skeleton);
            for i in 0..3 {
                let tentacle = ctx.world.spawn((
                    Transform::at(vec3((i as f32 - 1.0) * 2.5, 0.0, 0.0)),
                    Model::new(mesh),
                    skeleton.clone(),
                    Sway(i as f32 * 1.8),
                    Bend(bend.clone()),
                ));
                if i == 2 {
                    ctx.world.insert(tentacle, bend_animator(&skeleton));
                }
                if i == 0 {
                    // ANCHOR: skin_parts
                    ctx.world.spawn((Model::new(band), SkinOf(tentacle)));
                    let tip = skeleton.find("joint_7").unwrap();
                    ctx.world.spawn((
                        Transform::default(),
                        Model::new(ball),
                        AttachToJoint {
                            skeleton: tentacle,
                            joint: tip,
                            offset: Transform::at(vec3(0.0, 0.12, 0.0)),
                        },
                    ));
                    // ANCHOR_END: skin_parts
                }
            }
        })
        .run();
}

// ANCHOR: skin_ik
// Register this with Game::new(...).add(Phase::Late, procedural_pose).
#[allow(dead_code)]
fn procedural_pose(ctx: &mut Ctx) {
    let target = vec3(0.7, 0.6, 0.2);
    ctx.world
        .query::<(&Transform, &mut Skeleton)>()
        .for_each(|(world, skeleton)| {
            ponko::anim::two_bone_ik(skeleton, world, [0, 1, 2], target, Vec3::Z, 1.0);
            // Apply parent first when distributing aim across spine/neck/head.
            ponko::anim::look_at_joint(skeleton, world, 5, Vec3::Y, target, 0.3, 0.2);
            ponko::anim::look_at_joint(skeleton, world, 6, Vec3::Y, target, 0.5, 0.5);
            ponko::anim::look_at_joint(skeleton, world, 7, Vec3::Y, target, 0.8, 1.0);
        });
}

#[allow(dead_code)]
fn debug_bones(ctx: &mut Ctx, character: Entity) {
    ctx.draw_skeleton(character, Color::RED);
}
// ANCHOR_END: skin_ik
