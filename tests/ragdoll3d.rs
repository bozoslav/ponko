//! Ragdolls (feature `physics3d`): a skeleton handed over to physics.
#![cfg(feature = "physics3d")]
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::anim::{Joint, Skeleton};
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::RagdollBone;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform;
#[allow(unused_imports)]
#[cfg(feature = "physics3d")]
use ponko::d3::{Collider, Ragdoll, RigidBody};
use ponko::prelude::*;

const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

/// A stick figure: hips 1 m up, a spine 0.6 m above them, a head 0.4 m
/// above that, and an arm sticking out sideways from the spine.
fn figure() -> Skeleton {
    let joint = |name: &str, parent, at: Vec3| Joint {
        name: name.into(),
        parent,
        rest: Transform::at(at),
    };
    Skeleton::new(vec![
        joint("hips", None, vec3(0.0, 1.0, 0.0)),
        joint("spine", Some(0), vec3(0.0, 0.6, 0.0)),
        joint("head", Some(1), vec3(0.0, 0.4, 0.0)),
        joint("arm", Some(1), vec3(0.5, 0.2, 0.0)),
        joint("hand", Some(3), vec3(0.5, 0.0, 0.0)),
    ])
}

fn bones() -> Vec<RagdollBone> {
    vec![
        RagdollBone::new("hips", 0.12),
        RagdollBone::new("spine", 0.1).swing(0.5),
        RagdollBone::new("head", 0.1),
        RagdollBone::new("arm", 0.05).swing(1.2),
    ]
}

fn joint_world(world: &mut World, e: Entity, joint: usize) -> Vec3 {
    let model = world.get::<Transform>(e).unwrap().matrix();
    let skeleton = world.get::<Skeleton>(e).unwrap();
    (model * skeleton.joint_matrix(joint)).transform_point3(Vec3::ZERO)
}

#[test]
fn a_ragdoll_falls_to_the_floor_in_one_piece() {
    let mut figure_entity = None;
    let mut world = Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(250, |ctx| {
        ctx.world.spawn((
            Transform::at(vec3(0.0, -0.5, 0.0)),
            RigidBody::fixed(),
            Collider::cuboid(vec3(20.0, 1.0, 20.0)),
        ));
        let e = ctx.world.spawn((Transform::at(vec3(0.0, 2.0, 0.0)), figure()));
        ctx.start_ragdoll(e, &bones()).unwrap();
        // A shove, so it topples instead of landing on its hips.
        let spine = ctx.world.get::<Ragdoll>(e).unwrap().body(1).unwrap();
        assert!(
            ctx.physics3d.apply_impulse(spine, vec3(4.0, 0.0, 0.0)),
            "linked at once"
        );
        figure_entity = Some(e);
    });
    let e = figure_entity.unwrap();
    let hips = joint_world(&mut world, e, 0);
    let spine = joint_world(&mut world, e, 1);
    let head = joint_world(&mut world, e, 2);
    let arm = joint_world(&mut world, e, 3);
    assert!(hips.y < 0.6, "the hips came down from 3 m: {hips}");
    for p in [hips, spine, head, arm] {
        assert!(p.y > -0.05, "nothing sinks through the floor: {p}");
    }
    assert!(head.y < 1.0, "it toppled over: the head is low ({head})");
    // Bones keep their lengths: the joints hold the figure together.
    assert!((spine.distance(hips) - 0.6).abs() < 0.05, "{}", spine.distance(hips));
    assert!((head.distance(spine) - 0.4).abs() < 0.05, "{}", head.distance(spine));
    assert!((arm.distance(spine) - 0.2f32.hypot(0.5)).abs() < 0.05);
    // The pose follows the bodies, so the animated parts moved with them.
    let skeleton = world.get::<Skeleton>(e).unwrap();
    assert_ne!(skeleton.pose[1].rotation, Quat::IDENTITY);
}

#[test]
fn joint_limits_hold_a_bone_inside_its_cone() {
    let mut figure_entity = None;
    let mut world = Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(100, |ctx| {
        let e = ctx.world.spawn((Transform::default(), figure()));
        ctx.start_ragdoll(
            e,
            &[
                RagdollBone::new("hips", 0.12),
                RagdollBone::new("spine", 0.1).swing(0.3).twist(0.1),
            ],
        )
        .unwrap();
        // Hold the hips still and kick the top of the spine sideways, hard.
        let r = ctx.world.get::<Ragdoll>(e).unwrap();
        let (hips, spine) = (r.body(0).unwrap(), r.body(1).unwrap());
        ctx.world.get_mut::<RigidBody>(hips).unwrap().kind = BodyKind::Fixed;
        assert!(
            ctx.physics3d
                .apply_impulse_at(spine, vec3(30.0, 0.0, 0.0), vec3(0.0, 2.0, 0.0))
        );
        figure_entity = Some(e);
    });
    let e = figure_entity.unwrap();
    let up = (joint_world(&mut world, e, 2) - joint_world(&mut world, e, 1)).normalize();
    let bend = up.angle_between(Vec3::Y);
    assert!(bend > 0.15, "the kick bent it: {bend}");
    assert!(bend < 0.3 + 0.08, "but no further than its 0.3 rad cone: {bend}");
}

#[test]
fn stopping_removes_the_bodies_and_starting_checks_its_bones() {
    Game::new("t", 1, 1).physics3d(GRAVITY).run_headless(3, |ctx| {
        let e = ctx.world.spawn((Transform::default(), figure()));
        let entities = ctx.world.len();
        ctx.start_ragdoll(e, &bones()).unwrap();
        assert_eq!(ctx.world.len(), entities + 4 + 3, "four bodies and three joints");
        // Starting again replaces it.
        ctx.start_ragdoll(e, &bones()).unwrap();
        assert_eq!(ctx.world.len(), entities + 7);
        ctx.stop_ragdoll(e, 0.0);
        assert_eq!(ctx.world.len(), entities);
        assert!(ctx.world.get::<Ragdoll>(e).is_none());
        // Blending back keeps the component (without bodies) until it ends.
        ctx.start_ragdoll(e, &bones()).unwrap();
        ctx.stop_ragdoll(e, 0.5);
        assert_eq!(ctx.world.len(), entities);
        assert!(ctx.world.get::<Ragdoll>(e).unwrap().is_blending());

        let errors = [
            vec![RagdollBone::new("tail", 0.1)],
            vec![RagdollBone::new("hips", 0.1), RagdollBone::new("hips", 0.1)],
            vec![RagdollBone::new("hips", 0.0)],
        ];
        ctx.start_ragdoll(e, &bones()).unwrap();
        for bad in errors {
            assert!(ctx.start_ragdoll(e, &bad).is_err());
        }
        // A bad call leaves the running ragdoll alone.
        assert_eq!(ctx.world.len(), entities + 7);
        assert!(ctx.world.get::<Ragdoll>(e).unwrap().body(0).is_some());
        let bare = ctx.world.spawn((Transform::default(),));
        assert!(ctx.start_ragdoll(bare, &bones()).is_err());
    });
}

#[test]
fn guide_ragdoll_snippet_compiles_and_steps() {
    Game::new("ragdoll", 1, 1).physics3d(GRAVITY).run_headless(5, |ctx| {
        ctx.world.spawn((
            Transform::at(vec3(0.0, -0.5, 0.0)),
            RigidBody::fixed(),
            Collider::cuboid(vec3(20.0, 1.0, 20.0)),
        ));
        let enemy = ctx.world.spawn((Transform::default(), figure()));
        let hit = Vec3::X;
        // ANCHOR: ragdoll
        // A hit knocks `enemy` over: physics takes over its skeleton.
        ctx.start_ragdoll(
            enemy,
            &[
                RagdollBone::new("hips", 0.12),
                RagdollBone::new("spine", 0.1).swing(0.5),
                RagdollBone::new("head", 0.1),
                RagdollBone::new("arm", 0.05).swing(1.2),
            ],
        )
        .expect("the skeleton has these joints");
        let spine = ctx.world.get::<Skeleton>(enemy).unwrap().find("spine").unwrap();
        let body = ctx.world.get::<Ragdoll>(enemy).unwrap().body(spine).unwrap();
        ctx.physics3d.apply_impulse(body, hit * 40.0);
        // When it gets up: bodies gone, half a second back to the animation.
        ctx.stop_ragdoll(enemy, 0.5);
        // ANCHOR_END: ragdoll
    });
}
