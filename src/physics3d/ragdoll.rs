//! Ragdolls: a skeleton handed over to physics. Each chosen bone becomes a
//! capsule body, joined to its parent bone's body by a limited spherical
//! joint; every physics step (and every drawn frame) the skeleton's pose
//! follows the bodies. Stopping blends back to the animated pose.
use super::*;
use crate::anim::Skeleton;
use crate::d3::JointAxis as JointAxis3D;
use crate::d3::JointMotion as JointMotion3D;
use glam::Mat4;
use ponko_ecs::Component;

/// One bone of a ragdoll: the skeleton joint it starts at, how thick it
/// is, and how far its joint to the parent bone bends.
/// ```
/// use ponko::prelude::*;
/// let thigh = ponko::d3::RagdollBone::new("thigh_l", 0.08).swing(0.9).twist(0.2);
/// assert_eq!(thigh.radius, 0.08);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct RagdollBone {
    /// The skeleton joint's name. The bone reaches to its first child joint
    /// (a chosen bone's, if one is), or is a ball at the end of a chain.
    pub joint: String,
    /// Capsule radius in meters (world units).
    pub radius: f32,
    /// How far it swings away from its parent bone's direction, in radians
    /// (a cone). Default 0.7.
    pub swing: f32,
    /// How far it twists around its own length either way, in radians.
    /// Default 0.3.
    pub twist: f32,
    /// Collider density (kg/m³-ish, as [`Collider3D::density`]). Default 1000.
    pub density: f32,
}

impl RagdollBone {
    /// A bone at `joint`, `radius` thick, with the default limits.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::d3::RagdollBone::new("spine", 0.12).swing, 0.7);
    /// ```
    #[must_use]
    pub fn new(joint: &str, radius: f32) -> Self {
        Self {
            joint: joint.into(),
            radius,
            swing: 0.7,
            twist: 0.3,
            density: 1000.0,
        }
    }
    /// With a swing cone of `radians`.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::d3::RagdollBone::new("arm", 0.05).swing(1.2).swing, 1.2);
    /// ```
    #[must_use]
    pub fn swing(self, radians: f32) -> Self {
        Self { swing: radians, ..self }
    }
    /// With a twist limit of `radians` either way.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::d3::RagdollBone::new("arm", 0.05).twist(0.1).twist, 0.1);
    /// ```
    #[must_use]
    pub fn twist(self, radians: f32) -> Self {
        Self { twist: radians, ..self }
    }
}

#[derive(Clone, Debug)]
struct Bone {
    joint: usize,
    body: Entity,
    /// The body's rotation in the joint's frame (its local Y runs down the bone).
    align: Quat,
    /// From the joint to the body's center, along the body's Y.
    half: f32,
    /// Whether no other chosen bone is above it: its position follows the body too.
    root: bool,
}

/// A skeleton in physics' hands, on the entity with the [`Skeleton`] and
/// [`d3::Transform`](crate::d3::Transform); made by [`Ctx::start_ragdoll`], ended by
/// [`Ctx::stop_ragdoll`]. While it lasts, the entity's
/// [`Animator`](crate::anim::Animator) is paused and the pose follows the bodies.
/// Push it with [`Physics3D::apply_impulse`] on [`body`](Self::body).
/// Stop it before despawning the entity: its bodies are entities of their own.
///
/// ```
/// use ponko::prelude::*;
/// Game::new("t", 1, 1).physics3d(vec3(0.0, -9.81, 0.0)).run_headless(1, |ctx| {
///     let skeleton = ponko::anim::Skeleton::new(vec![
///         ponko::anim::Joint { name: "hips".into(), parent: None, rest: ponko::d3::Transform::at(vec3(0.0, 1.0, 0.0)) },
///         ponko::anim::Joint { name: "head".into(), parent: Some(0), rest: ponko::d3::Transform::at(vec3(0.0, 0.8, 0.0)) },
///     ]);
///     let body = ctx.world.spawn((ponko::d3::Transform::default(), skeleton));
///     ctx.start_ragdoll(body, &[ponko::d3::RagdollBone::new("hips", 0.15), ponko::d3::RagdollBone::new("head", 0.1)]).unwrap();
///     let ragdoll = ctx.world.get::<ponko::d3::Ragdoll>(body).unwrap();
///     assert_eq!(ragdoll.bodies().count(), 2);
/// });
/// ```
#[derive(Clone, Debug, Component)]
pub struct Ragdoll3D {
    bones: Vec<Bone>,
    joints: Vec<Entity>,
    /// Stopping: the pose when it stopped, seconds since, and how long the blend lasts.
    blend: Option<(Vec<Transform3D>, f32, f32)>,
    /// Scratch: each bone body's pose, and each joint's model-space matrix.
    poses: Vec<(Vec3, Quat)>,
    world: Vec<Mat4>,
}

impl Ragdoll3D {
    /// The body that bone `joint` (a skeleton joint index) became.
    /// ```
    /// # use ponko::prelude::*;
    /// # Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(1, |ctx| {
    /// # let skeleton = ponko::anim::Skeleton::new(vec![ponko::anim::Joint { name: "hips".into(), parent: None, rest: ponko::d3::Transform::default() }]);
    /// # let e = ctx.world.spawn((ponko::d3::Transform::default(), skeleton));
    /// ctx.start_ragdoll(e, &[ponko::d3::RagdollBone::new("hips", 0.15)]).unwrap();
    /// let hips = ctx.world.get::<ponko::d3::Ragdoll>(e).unwrap().body(0).unwrap();
    /// ctx.physics3d.apply_impulse(hips, vec3(0.0, 0.0, -50.0));
    /// # });
    /// ```
    pub fn body(&self, joint: usize) -> Option<Entity> {
        self.bones.iter().find(|b| b.joint == joint).map(|b| b.body)
    }
    /// Every bone's (joint index, body).
    /// ```
    /// # use ponko::prelude::*;
    /// # Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(1, |ctx| {
    /// # let skeleton = ponko::anim::Skeleton::new(vec![ponko::anim::Joint { name: "hips".into(), parent: None, rest: ponko::d3::Transform::default() }]);
    /// # let e = ctx.world.spawn((ponko::d3::Transform::default(), skeleton));
    /// # ctx.start_ragdoll(e, &[ponko::d3::RagdollBone::new("hips", 0.15)]).unwrap();
    /// for (joint, body) in ctx.world.get::<ponko::d3::Ragdoll>(e).unwrap().bodies() {
    ///     println!("joint {joint} is body {body:?}");
    /// }
    /// # });
    /// ```
    pub fn bodies(&self) -> impl Iterator<Item = (usize, Entity)> + '_ {
        self.bones.iter().map(|b| (b.joint, b.body))
    }
    /// Whether it is blending back to the animation after
    /// [`Ctx::stop_ragdoll`] (its bodies are gone then).
    /// ```
    /// # use ponko::prelude::*;
    /// # Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(1, |ctx| {
    /// # let skeleton = ponko::anim::Skeleton::new(vec![ponko::anim::Joint { name: "hips".into(), parent: None, rest: ponko::d3::Transform::default() }]);
    /// # let e = ctx.world.spawn((ponko::d3::Transform::default(), skeleton));
    /// # ctx.start_ragdoll(e, &[ponko::d3::RagdollBone::new("hips", 0.15)]).unwrap();
    /// assert!(!ctx.world.get::<ponko::d3::Ragdoll>(e).unwrap().is_blending());
    /// # });
    /// ```
    pub fn is_blending(&self) -> bool {
        self.blend.is_some()
    }
}

fn decompose(m: Mat4) -> (Vec3, Quat, Vec3) {
    let (scale, rotation, position) = m.to_scale_rotation_translation();
    (position, rotation.normalize(), scale)
}

impl Ctx {
    /// Hands `entity`'s skeleton to physics, posed as it is now: one capsule
    /// body per bone in `bones`, joined to the nearest chosen bone above it.
    /// Needs [`Game::physics3d`](crate::game::Game::physics3d), and a [`Skeleton`] and
    /// [`d3::Transform`](crate::d3::Transform) on `entity`. Starting again restarts it.
    ///
    /// Remove or disable the entity's own collider or [`Character3D`] first,
    /// or the bones collide with it. Bones joined to each other don't collide.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if `entity` has
    /// no skeleton or transform or a bone names no joint,
    /// [`Invalid`](crate::error::ErrorKind::Invalid) if a joint is chosen twice or a
    /// radius or density isn't positive. A running ragdoll is left alone then.
    pub fn start_ragdoll(&mut self, entity: Entity, bones: &[RagdollBone]) -> Result<(), crate::error::Error> {
        use crate::error::{Error, ErrorKind};
        let model = self
            .world
            .get::<Transform3D>(entity)
            .ok_or_else(|| Error::new(ErrorKind::NotFound, "start_ragdoll: the entity has no Transform3D"))?
            .matrix();
        let skeleton = self
            .world
            .get::<Skeleton>(entity)
            .ok_or_else(|| Error::new(ErrorKind::NotFound, "start_ragdoll: the entity has no Skeleton"))?
            .clone();
        let skeleton = &skeleton;
        let joints = skeleton.joints();
        let mut chosen: Vec<(usize, &RagdollBone)> = Vec::with_capacity(bones.len());
        for b in bones {
            let j = skeleton.find(&b.joint).ok_or_else(|| {
                Error::new(
                    ErrorKind::NotFound,
                    format!("start_ragdoll: no joint named {:?}", b.joint),
                )
            })?;
            if !(b.radius > 0.0 && b.radius.is_finite()) || !b.density.is_finite() || b.density <= 0.0 {
                return Err(Error::new(
                    ErrorKind::Invalid,
                    format!("start_ragdoll: {:?} needs a positive radius and density", b.joint),
                ));
            }
            if chosen.iter().any(|&(c, _)| c == j) {
                return Err(Error::new(
                    ErrorKind::Invalid,
                    format!("start_ragdoll: {:?} is chosen twice", b.joint),
                ));
            }
            chosen.push((j, b));
        }
        // Checked first, so a bad call leaves a running ragdoll alone.
        self.stop_ragdoll(entity, 0.0);
        chosen.sort_by_key(|&(j, _)| j);
        let is_chosen = |j: usize| chosen.binary_search_by_key(&j, |&(c, _)| c).ok();
        let world: Vec<Mat4> = (0..joints.len()).map(|j| model * skeleton.joint_matrix(j)).collect();
        let local = |j: usize| skeleton.pose.get(j).copied().unwrap_or(joints[j].rest);
        // The nearest chosen bone above each chosen bone.
        let parent_bone = |mut j: usize| {
            while let Some(p) = joints[j].parent {
                if let Some(i) = is_chosen(p) {
                    return Some(i);
                }
                j = p;
            }
            None
        };
        let mut planned = Vec::with_capacity(chosen.len());
        for &(j, b) in &chosen {
            let children = || (0..joints.len()).filter(move |&c| joints[c].parent == Some(j));
            let child = children()
                .find(|&c| is_chosen(c).is_some())
                .or_else(|| children().next());
            let (p, q, _) = decompose(world[j]);
            let (dir, length) = match child {
                Some(c) => {
                    let to = decompose(world[c]).0 - p;
                    (local(c).position.try_normalize().unwrap_or(Vec3::Y), to.length())
                }
                None => (Vec3::Y, 2.0 * b.radius),
            };
            let align = Quat::from_rotation_arc(Vec3::Y, dir);
            let rotation = (q * align).normalize();
            let half = length * 0.5;
            let height = (length - 2.0 * b.radius).max(0.0);
            let collider = if height > 0.0 {
                Collider3D::capsule(height, b.radius)
            } else {
                Collider3D::sphere(b.radius)
            };
            planned.push((j, b, p + rotation * Vec3::Y * half, rotation, align, half, collider));
        }
        let mut made = Vec::with_capacity(planned.len());
        for &(j, b, position, rotation, align, half, collider) in &planned {
            let t = Transform3D {
                position,
                rotation,
                ..Default::default()
            };
            let (rigid, collider) = (RigidBody3D::dynamic(), collider.density(b.density));
            let body = self.world.spawn((t, rigid, collider));
            // Linked now, not at the next step, so the caller can push the
            // bodies straight away (a hit that knocks someone over).
            let step = self.physics3d.step;
            let kind = super::LinkKind::Body(rigid, collider);
            super::sync_one(&mut self.physics3d, body, &t, kind, None, step);
            made.push(Bone {
                joint: j,
                body,
                align,
                half,
                root: false,
            });
        }
        let mut links = Vec::new();
        for i in 0..made.len() {
            let Some(parent) = parent_bone(made[i].joint) else {
                made[i].root = true;
                continue;
            };
            let (a, b) = (&planned[parent], &planned[i]);
            let anchor = decompose(world[made[i].joint]).0;
            // The joint's X (its twist axis) runs down the child bone.
            let frame = Quat::from_rotation_arc(Vec3::X, b.3 * Vec3::Y);
            let in_body = |position: Vec3, rotation: Quat| Transform3D {
                position: rotation.inverse() * (anchor - position),
                rotation: (rotation.inverse() * frame).normalize(),
                ..Default::default()
            };
            let mut joint = Joint3D::spherical(made[parent].body, made[i].body)
                .motion(JointAxis3D::AngularX, JointMotion3D::Limited(-b.1.twist, b.1.twist))
                .swing_cone(b.1.swing);
            joint.frame_a = in_body(a.2, a.3);
            joint.frame_b = in_body(b.2, b.3);
            links.push(self.world.spawn((joint,)));
        }
        let count = joints.len();
        self.world.insert(
            entity,
            Ragdoll3D {
                bones: made,
                joints: links,
                blend: None,
                poses: Vec::with_capacity(chosen.len()),
                world: Vec::with_capacity(count),
            },
        );
        Ok(())
    }

    /// Ends `entity`'s ragdoll: removes its bodies and joints, and blends
    /// the skeleton from where the ragdoll left it back to the animated
    /// pose over `seconds` (0: at once). The entity stays where it was; move
    /// it to where the ragdoll lies first (its root bone's body) if the
    /// animation should start from there. Does nothing without a ragdoll.
    /// ```
    /// # use ponko::prelude::*;
    /// # Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(1, |ctx| {
    /// # let skeleton = ponko::anim::Skeleton::new(vec![ponko::anim::Joint { name: "hips".into(), parent: None, rest: ponko::d3::Transform::default() }]);
    /// # let e = ctx.world.spawn((ponko::d3::Transform::default(), skeleton));
    /// # ctx.start_ragdoll(e, &[ponko::d3::RagdollBone::new("hips", 0.15)]).unwrap();
    /// ctx.stop_ragdoll(e, 0.5); // half a second back to the animation
    /// # });
    /// ```
    pub fn stop_ragdoll(&mut self, entity: Entity, seconds: f32) {
        let Some(ragdoll) = self.world.get_mut::<Ragdoll3D>(entity) else {
            return;
        };
        let doomed: Vec<Entity> = ragdoll
            .joints
            .drain(..)
            .chain(ragdoll.bones.drain(..).map(|b| b.body))
            .collect();
        for e in doomed {
            self.world.despawn(e);
        }
        let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        let pose = self.world.get::<Skeleton>(entity).map(|s| s.pose.clone());
        match pose {
            Some(pose) if seconds > 0.0 => {
                if let Some(r) = self.world.get_mut::<Ragdoll3D>(entity) {
                    r.blend = Some((pose, 0.0, seconds));
                }
            }
            _ => {
                self.world.remove::<Ragdoll3D>(entity);
            }
        }
    }
}

/// Poses every live ragdoll's skeleton from its bodies. Called after each
/// physics step and each frame's interpolation.
pub(crate) fn pose_ragdolls(world: &mut World, scratch: &mut Vec<Entity>) {
    scratch.clear();
    scratch.extend(
        world
            .query::<(Entity, &Ragdoll3D)>()
            .iter()
            .filter(|(_, r)| r.blend.is_none())
            .map(|(e, _)| e),
    );
    for &e in scratch.iter() {
        let Some(mut ragdoll) = world.get_mut::<Ragdoll3D>(e).map(|r| std::mem::replace(r, empty())) else {
            continue;
        };
        ragdoll.poses.clear();
        for b in &ragdoll.bones {
            let t = world.get::<Transform3D>(b.body).copied().unwrap_or_default();
            ragdoll.poses.push((t.position, t.rotation));
        }
        let model = world.get::<Transform3D>(e).map(|t| t.matrix());
        if let (Some(model), Some(skeleton)) = (model, world.get_mut::<Skeleton>(e)) {
            pose_from_bodies(&mut ragdoll, model.inverse(), skeleton);
        }
        if let Some(r) = world.get_mut::<Ragdoll3D>(e) {
            *r = ragdoll;
        }
    }
}

fn empty() -> Ragdoll3D {
    Ragdoll3D {
        bones: Vec::new(),
        joints: Vec::new(),
        blend: None,
        poses: Vec::new(),
        world: Vec::new(),
    }
}

/// Sets each bone joint's local pose so its world pose matches its body:
/// rotation for every bone, position too for bones with none above.
fn pose_from_bodies(ragdoll: &mut Ragdoll3D, to_model: Mat4, skeleton: &mut Skeleton) {
    let count = skeleton.joints().len();
    ragdoll.world.clear();
    let mut next = 0;
    for j in 0..count {
        let joint = &skeleton.joints()[j];
        let parent = joint.parent.map_or(Mat4::IDENTITY, |p| ragdoll.world[p]);
        let mut local = skeleton.pose.get(j).copied().unwrap_or(joint.rest);
        if let Some(b) = ragdoll.bones.get(next).filter(|b| b.joint == j) {
            let (body_position, body_rotation) = ragdoll.poses[next];
            let rotation = body_rotation * b.align.inverse();
            let position = body_position - body_rotation * Vec3::Y * b.half;
            let in_model = to_model * Mat4::from_rotation_translation(rotation, position);
            let (p, q, _) = decompose(parent.inverse() * in_model);
            if q.is_finite() {
                local.rotation = q;
            }
            if b.root && p.is_finite() {
                local.position = p;
            }
            if let Some(slot) = skeleton.pose.get_mut(j) {
                *slot = local;
            }
            next += 1;
        }
        ragdoll.world.push(parent * local.matrix());
    }
}

/// After the entity's animator posed it this frame: blends stopping
/// ragdolls from their last pose to the animation. Returns whether the
/// blend is over (then the caller removes the ragdoll).
pub(crate) fn blend_back(ragdoll: &mut Ragdoll3D, skeleton: &mut Skeleton, dt: f32) -> bool {
    let Some((from, elapsed, duration)) = &mut ragdoll.blend else {
        return false;
    };
    *elapsed += dt;
    let t = (*elapsed / *duration).clamp(0.0, 1.0);
    let w = t * t * (3.0 - 2.0 * t);
    for (to, from) in skeleton.pose.iter_mut().zip(from.iter()) {
        to.position = from.position.lerp(to.position, w);
        to.rotation = from.rotation.slerp(to.rotation, w);
        to.scale = from.scale.lerp(to.scale, w);
    }
    t >= 1.0
}
