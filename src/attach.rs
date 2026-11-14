//! Attachments: entities that follow another entity (health bars, turrets,
//! selection rings, carried things), in 2D ([`Attached`]) and 3D
//! (`Attached3D`, with the `3d` feature): the parent and child links of a
//! scene's hierarchy.

use glam::Vec2;
use rayon::prelude::*;

use crate::ctx::Ctx;
use crate::d2::Transform as Transform2D;
use ponko_ecs::{Component, Entity, With, World};

/// Keeps this entity's [`d2::Transform`](crate::d2::Transform) position and rotation at an offset
/// from another entity's, updated by [`follow_attachments`](crate::systems::follow_attachments). Its own scale
/// is left alone. When the entity it's attached to is despawned, this one is
/// despawned too. Attachments can be attached to attachments.
///
/// ```
/// use ponko::prelude::*;
///
/// #[derive(Component)]
/// struct Tank;
///
/// let mut turret = None;
/// let world = Game::new("turrets", 320, 240)
///     .sim_system(|ctx: &mut Ctx| {
///         ctx.world.query::<(&mut ponko::d2::Transform, &Tank)>().for_each(|(t, _)| t.position.x += 1.0);
///     })
///     .sim_system(systems::follow_attachments) // after the systems that move things
///     .run_headless(10, |ctx| {
///         let tank = ctx.world.spawn((ponko::d2::Transform::at(vec2(0.0, 0.0)), Tank));
///         let on_top = ponko::attach::Attached::to(tank).offset(vec2(0.0, -4.0));
///         turret = Some(ctx.world.spawn((ponko::d2::Transform::default(), on_top)));
///     });
/// let at = world.get::<ponko::d2::Transform>(turret.unwrap()).unwrap().position;
/// assert_eq!(at, vec2(10.0, -4.0));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[repr(C)] // shared with C/C++ (`PonkoAttached`)
pub struct Attached {
    /// The entity to follow.
    pub to: Entity,
    /// Where, in the followed entity's space: turned by its rotation and
    /// stretched by its scale (so a flipped sprite mirrors it).
    pub offset: Vec2,
    /// Added to the followed entity's rotation.
    pub rotation: f32,
}

impl Attached {
    /// Attached to `to`, right on top of it.
    #[must_use]
    pub fn to(to: Entity) -> Self {
        Self {
            to,
            offset: Vec2::ZERO,
            rotation: 0.0,
        }
    }

    /// Same attachment, `offset` away.
    #[must_use]
    pub fn offset(self, offset: Vec2) -> Self {
        Self { offset, ..self }
    }

    /// Same attachment, turned by `rotation` radians more.
    #[must_use]
    pub fn rotated(self, rotation: f32) -> Self {
        Self { rotation, ..self }
    }
}

/// Chains deeper than this are treated as broken (a loop, most likely).
const MAX_DEPTH: usize = 64;

/// Scratch kept between ticks so following allocates nothing.
#[derive(Default)]
struct Scratch {
    attached: Vec<(Entity, Attached)>,
    placed: Vec<Option<(Vec2, f32)>>,
}

/// The system behind [`systems::follow_attachments`](crate::systems::follow_attachments); it keeps
/// this path for its name (`ponko::attach::follow_attachments`).
pub(crate) fn follow_attachments(ctx: &mut Ctx) {
    #[cfg(feature = "3d")]
    follow3d(ctx);
    let mut s = ctx.world.remove_resource::<Scratch>().unwrap_or_default();
    // Same entities, in the same order, as the writing query below.
    ctx.world
        .query::<(Entity, &Attached)>()
        .filter::<With<Transform2D>>()
        .par_collect_into(&mut s.attached, |(e, a)| (e, *a));
    if !s.attached.is_empty() {
        let world = &ctx.world;
        s.placed.clear();
        s.attached
            .par_iter()
            .map(|&(_, a)| place(world, a))
            .collect_into_vec(&mut s.placed);
        let placed = &s.placed;
        ctx.world
            .query::<(&mut Transform2D, &Attached)>()
            .par_for_each_indexed(|i, (t, _)| {
                if let Some((position, rotation)) = placed[i] {
                    t.position = position;
                    t.rotation = rotation;
                }
            });
        for (&(e, _), placed) in s.attached.iter().zip(&s.placed) {
            if placed.is_none() {
                ctx.commands.despawn(e);
            }
        }
    }
    ctx.world.insert_resource(s);
}

/// [`place`], for the editor's tests.
#[cfg(test)]
pub(crate) fn tests_place(world: &World, a: Attached) -> Option<(Vec2, f32)> {
    place(world, a)
}

/// Where `a` puts its entity: from the root of the chain down. `None` if
/// something up the chain is gone (or the chain loops).
fn place(world: &World, a: Attached) -> Option<(Vec2, f32)> {
    // Walk up to the first entity that isn't attached, keeping the links.
    let mut links = [a; MAX_DEPTH];
    let mut depth = 1;
    let mut up = a.to;
    while let Some(&next) = world.get::<Attached>(up) {
        if depth == MAX_DEPTH {
            return None;
        }
        links[depth] = next;
        depth += 1;
        up = next.to;
    }
    let root = world.get::<Transform2D>(up)?;
    if !world.is_alive(up) {
        return None;
    }
    let (mut position, mut rotation) = (root.position, root.rotation);
    let mut scale = root.scale;
    // Back down: each link is placed in its parent's space.
    for i in (0..depth).rev() {
        let link = links[i];
        position += Vec2::from_angle(rotation).rotate(link.offset * scale);
        rotation += link.rotation;
        if i > 0 {
            // The next link hangs off this one, whose own scale applies.
            let this = links[i - 1].to;
            scale = world.get::<Transform2D>(this).map_or(Vec2::ONE, |t| t.scale);
        }
    }
    Some((position, rotation))
}

/// Keeps this entity's 3D transform at `offset` in another entity's space:
/// the 3D parent link. A lamp on a cart, a door in a house, a level's
/// props grouped under one entity to move them together. Placed by
/// [`follow_attachments`], which also despawns it when the entity it
/// follows is gone; chains of attachments work as in 2D.
///
/// `offset` is a whole transform in the parent's space: its position is
/// turned by the parent's rotation and stretched by its scale, its rotation
/// is added to the parent's, and its scale multiplies the parent's. The
/// editor's hierarchy panel and the control endpoint's `editor.parent`
/// make one from where the entity already is, so parenting doesn't move it.
///
/// ```
/// use ponko::prelude::*;
/// use ponko::d3::{Attached, Transform};
///
/// let mut lamp = None;
/// let world = Game::new("carts", 320, 240)
///     .sim_system(systems::follow_attachments)
///     .run_headless(1, |ctx| {
///         let cart = ctx.world.spawn((Transform::at(vec3(10.0, 0.0, 0.0)).scaled(2.0),));
///         let on_top = Attached::to(cart).offset(Transform::at(vec3(0.0, 1.0, 0.0)));
///         lamp = Some(ctx.world.spawn((Transform::default(), on_top)));
///     });
/// let at = world.get::<Transform>(lamp.unwrap()).unwrap();
/// assert_eq!((at.position, at.scale), (vec3(10.0, 2.0, 0.0), Vec3::splat(2.0)));
/// ```
#[cfg(feature = "3d")]
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[repr(C)] // shared with C/C++ (`PonkoAttached3D`)
pub struct Attached3D {
    /// The entity to follow.
    pub to: Entity,
    /// Where this entity sits in the followed entity's space.
    pub offset: crate::d3::Transform,
}

#[cfg(feature = "3d")]
impl Attached3D {
    /// Attached to `to`, right on top of it (same rotation and scale).
    #[must_use]
    pub fn to(to: Entity) -> Self {
        Self {
            to,
            offset: crate::d3::Transform::default(),
        }
    }

    /// Same attachment, at `offset` in the followed entity's space.
    #[must_use]
    pub fn offset(self, offset: crate::d3::Transform) -> Self {
        Self { offset, ..self }
    }
}

/// `local`, given in `parent`'s space, as a world transform.
#[cfg(feature = "3d")]
pub(crate) fn compose3d(parent: &crate::d3::Transform, local: &crate::d3::Transform) -> crate::d3::Transform {
    crate::d3::Transform {
        position: parent.position + parent.rotation * (parent.scale * local.position),
        rotation: (parent.rotation * local.rotation).normalize(),
        scale: parent.scale * local.scale,
    }
}

/// `world` in `parent`'s space: the inverse of [`compose3d`] (exact unless
/// a scale is zero, or non-uniform and rotated).
#[cfg(feature = "3d")]
pub(crate) fn relative3d(parent: &crate::d3::Transform, world: &crate::d3::Transform) -> crate::d3::Transform {
    let divide = |a: glam::Vec3, b: glam::Vec3| {
        glam::Vec3::new(
            if b.x == 0.0 { a.x } else { a.x / b.x },
            if b.y == 0.0 { a.y } else { a.y / b.y },
            if b.z == 0.0 { a.z } else { a.z / b.z },
        )
    };
    let inverse = parent.rotation.inverse();
    crate::d3::Transform {
        position: divide(inverse * (world.position - parent.position), parent.scale),
        rotation: (inverse * world.rotation).normalize(),
        scale: divide(world.scale, parent.scale),
    }
}

/// Scratch kept between ticks so following in 3D allocates nothing.
#[cfg(feature = "3d")]
#[derive(Default)]
struct Scratch3D {
    attached: Vec<(Entity, Attached3D)>,
    placed: Vec<Option<crate::d3::Transform>>,
}

/// [`follow_attachments`] for [`Attached3D`].
#[cfg(feature = "3d")]
fn follow3d(ctx: &mut Ctx) {
    use crate::d3::Transform as T3;
    let mut s = ctx.world.remove_resource::<Scratch3D>().unwrap_or_default();
    ctx.world
        .query::<(Entity, &Attached3D)>()
        .filter::<With<T3>>()
        .par_collect_into(&mut s.attached, |(e, a)| (e, *a));
    if !s.attached.is_empty() {
        let world = &ctx.world;
        s.placed.clear();
        s.attached
            .par_iter()
            .map(|&(_, a)| place3d(world, a))
            .collect_into_vec(&mut s.placed);
        let placed = &s.placed;
        ctx.world
            .query::<(&mut T3, &Attached3D)>()
            .par_for_each_indexed(|i, (t, _)| {
                if let Some(at) = placed[i] {
                    *t = at;
                }
            });
        for (&(e, _), placed) in s.attached.iter().zip(&s.placed) {
            if placed.is_none() {
                ctx.commands.despawn(e);
            }
        }
    }
    ctx.world.insert_resource(s);
}

/// Where `a` puts its entity: from the root of the chain down. `None` if
/// something up the chain is gone (or the chain loops).
#[cfg(feature = "3d")]
pub(crate) fn place3d(world: &World, a: Attached3D) -> Option<crate::d3::Transform> {
    let mut links = [a; MAX_DEPTH];
    let mut depth = 1;
    let mut up = a.to;
    while let Some(&next) = world.get::<Attached3D>(up) {
        if depth == MAX_DEPTH {
            return None;
        }
        links[depth] = next;
        depth += 1;
        up = next.to;
    }
    let mut at = *world.get::<crate::d3::Transform>(up)?;
    for link in links[..depth].iter().rev() {
        at = compose3d(&at, &link.offset);
    }
    Some(at)
}

/// Places every 3D attachment now, without despawning broken ones: for the
/// editor, whose edits must show while the simulation is paused.
#[cfg(feature = "3d")]
pub(crate) fn settle3d(world: &mut World) {
    let attached: Vec<(Entity, Attached3D)> = world
        .query::<(Entity, &Attached3D)>()
        .iter()
        .map(|(e, a)| (e, *a))
        .collect();
    for (e, a) in attached {
        if let Some(at) = place3d(world, a)
            && let Some(t) = world.get_mut::<crate::d3::Transform>(e)
        {
            *t = at;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec2;

    #[test]
    fn chains_compose_from_the_root_down() {
        let mut w = World::new();
        let root = w.spawn((Transform2D::at(vec2(10.0, 0.0)).rotated(std::f32::consts::FRAC_PI_2),));
        let arm = w.spawn((Transform2D::default(), Attached::to(root).offset(vec2(1.0, 0.0))));
        let hand = Attached::to(arm).offset(vec2(1.0, 0.0));
        let (p, r) = place(&w, hand).unwrap();
        // Root turned a quarter: both offsets point down (+y on screen).
        assert!((p - vec2(10.0, 2.0)).length() < 1e-5, "{p}");
        assert!((r - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn loops_and_missing_roots_place_nothing() {
        let mut w = World::new();
        let a = w.spawn((Transform2D::default(),));
        let b = w.spawn((Transform2D::default(), Attached::to(a)));
        w.insert(a, Attached::to(b)); // a <-> b
        assert_eq!(place(&w, Attached::to(a)), None);
        let gone = w.spawn((Transform2D::default(),));
        w.despawn(gone);
        assert_eq!(place(&w, Attached::to(gone)), None);
    }

    #[cfg(feature = "3d")]
    #[test]
    fn chains_in_3d_compose_rotation_scale_and_offset_from_the_root_down() {
        use crate::d3::Transform as T3;
        use glam::{Quat, vec3};
        let mut w = World::new();
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let root = w.spawn((T3::at(vec3(10.0, 0.0, 0.0)).rotated(turn).scaled(2.0),));
        let arm = w.spawn((T3::default(), Attached3D::to(root).offset(T3::at(vec3(1.0, 0.0, 0.0)))));
        let hand = Attached3D::to(arm).offset(T3::at(vec3(1.0, 0.0, 0.0)));
        let at = place3d(&w, hand).unwrap();
        // Turned a quarter about +Y, +X points to -Z; scale 2 doubles both offsets.
        assert!((at.position - vec3(10.0, 0.0, -4.0)).length() < 1e-5, "{}", at.position);
        assert!(at.rotation.angle_between(turn) < 1e-5);
        assert_eq!(at.scale, glam::Vec3::splat(2.0));
    }

    #[cfg(feature = "3d")]
    #[test]
    fn relative3d_undoes_compose3d() {
        use crate::d3::Transform as T3;
        use glam::{Quat, vec3};
        let parent = T3::at(vec3(1.0, 2.0, 3.0))
            .rotated(Quat::from_euler(glam::EulerRot::YXZ, 0.4, -0.3, 0.2))
            .scaled(1.5);
        let world = T3::at(vec3(-4.0, 0.5, 2.0)).rotated(Quat::from_rotation_x(1.0));
        let back = compose3d(&parent, &relative3d(&parent, &world));
        assert!((back.position - world.position).length() < 1e-4);
        assert!(back.rotation.angle_between(world.rotation) < 1e-4);
        assert!((back.scale - world.scale).length() < 1e-5);
    }

    #[cfg(feature = "3d")]
    #[test]
    fn broken_3d_chains_place_nothing() {
        use crate::d3::Transform as T3;
        let mut w = World::new();
        let a = w.spawn((T3::default(),));
        let b = w.spawn((T3::default(), Attached3D::to(a)));
        w.insert(a, Attached3D::to(b));
        assert_eq!(place3d(&w, Attached3D::to(a)), None);
    }
}
