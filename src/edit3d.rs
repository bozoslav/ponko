//! The 3D editor's math: picking with the mouse, the move / rotate / scale
//! gizmo with snapping in world or local space, and the fly camera. No
//! egui and no window: the inspector draws and drives it, the control
//! endpoint's `editor.*` methods use the same functions, and the tests run
//! it headless.

use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use ponko_ecs::{Entity, World};

use crate::d3::{Camera, Transform};

/// A ray from `from` along the unit vector `dir`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ray {
    pub from: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn new(from: Vec3, dir: Vec3) -> Self {
        Self {
            from,
            dir: dir.normalize_or(Vec3::NEG_Z),
        }
    }

    /// The camera's ray through window point `screen` (logical pixels).
    pub fn from_screen(camera: &Camera, screen: Vec2) -> Self {
        let (from, dir) = camera.screen_ray(screen);
        Self::new(from, dir)
    }

    pub fn at(&self, t: f32) -> Vec3 {
        self.from + self.dir * t
    }
}

/// How far around an entity without a model (a light, an empty group) a
/// ray picks it, in world units, times its largest scale.
pub(crate) const EMPTY_RADIUS: f32 = 0.25;

/// What a ray hit: the entity, how far along the ray, and where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hit {
    pub entity: Entity,
    pub distance: f32,
    pub point: Vec3,
}

/// How far along `ray` it enters the sphere; `0` when it starts inside,
/// `None` when it misses or the sphere is behind it.
pub(crate) fn ray_sphere(ray: &Ray, center: Vec3, radius: f32) -> Option<f32> {
    let to = center - ray.from;
    let along = to.dot(ray.dir);
    let d2 = to.length_squared() - along * along;
    let r2 = radius * radius;
    if d2 > r2 {
        return None;
    }
    let half = (r2 - d2).sqrt();
    let (near, far) = (along - half, along + half);
    if far < 0.0 { None } else { Some(near.max(0.0)) }
}

/// `e`'s bounding sphere in the world: its model's mesh bounds moved by its
/// transform, or [`EMPTY_RADIUS`] around it without a (live) mesh.
pub(crate) fn world_sphere(world: &World, assets: &crate::three::Assets3D, e: Entity) -> Option<(Vec3, f32)> {
    let t = world.get::<Transform>(e)?;
    let largest = t.scale.abs().max_element();
    let mesh = world
        .get::<crate::render::Model>(e)
        .and_then(|m| assets.live_bounds(m.mesh.id))
        .filter(|&(_, r)| r > 0.0);
    Some(match mesh {
        Some((center, radius)) => (t.matrix().transform_point3(center), radius * largest),
        None => (t.position, EMPTY_RADIUS * largest.max(1e-3)),
    })
}

/// The entity `ray` hits first: the nearest bounding sphere it enters (of
/// a model's mesh, or a small one around entities without a model). When
/// two are entered at the same distance, the smaller one wins, so a small
/// thing on a big one can be picked. `skip` is left out (the entities being
/// dropped or dragged).
pub(crate) fn pick(world: &World, assets: &crate::three::Assets3D, ray: &Ray, skip: &[Entity]) -> Option<Hit> {
    let mut best: Option<(f32, f32, Entity)> = None;
    for a in world.raw_ref().archetypes() {
        for &e in a.entities() {
            if skip.contains(&e) {
                continue;
            }
            let Some((center, radius)) = world_sphere(world, assets, e) else {
                continue;
            };
            let Some(t) = ray_sphere(ray, center, radius) else {
                continue;
            };
            let better = best.is_none_or(|(bt, br, be)| {
                t < bt - 1e-4 || ((t - bt).abs() <= 1e-4 && (radius < br || (radius == br && e < be)))
            });
            if better {
                best = Some((t, radius, e));
            }
        }
    }
    best.map(|(t, _, entity)| Hit {
        entity,
        distance: t,
        point: ray.at(t),
    })
}

/// Where `ray` meets the level plane `y = height`; when it points away from
/// it (or along it), the point `fallback` units along the ray.
pub(crate) fn ground_point(ray: &Ray, height: f32, fallback: f32) -> Vec3 {
    if ray.dir.y.abs() > 1e-5 {
        let t = (height - ray.from.y) / ray.dir.y;
        if t > 0.0 {
            return ray.at(t);
        }
    }
    ray.at(fallback)
}

/// Window point (logical pixels) of world point `p`; `None` behind the camera.
pub(crate) fn world_to_screen(camera: &Camera, p: Vec3) -> Option<Vec2> {
    let clip = camera.view_proj() * Vec4::new(p.x, p.y, p.z, 1.0);
    if clip.w <= 1e-5 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    let size = camera.viewport();
    Some(Vec2::new((ndc.x + 1.0) * 0.5 * size.x, (1.0 - ndc.y) * 0.5 * size.y))
}

/// World units one window pixel spans at `p`'s depth.
pub(crate) fn pixel_size(camera: &Camera, p: Vec3) -> f32 {
    let depth = (p - camera.position).dot(camera.forward()).max(camera.near);
    2.0 * depth * (camera.fov_y * 0.5).tan() / camera.viewport().y.max(1.0)
}

/// Snapping for gizmo drags and numeric edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Snap {
    pub on: bool,
    /// Grid step for moves, in world units.
    pub step: f32,
    /// Angle step for turns, in radians.
    pub angle: f32,
    /// Step for scale factors (0.1: 1.0, 1.1, 1.2, ...).
    pub scale: f32,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            on: false,
            step: 0.5,
            angle: 15f32.to_radians(),
            scale: 0.1,
        }
    }
}

/// `v` to the nearest multiple of `step` (unchanged for a step of 0 or less).
pub(crate) fn snap(v: f32, step: f32) -> f32 {
    if step > 0.0 { (v / step).round() * step } else { v }
}

/// What the gizmo does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Translate,
    Rotate,
    Scale,
}

/// Which axes the gizmo uses: the world's, or the selected entity's own.
/// Scaling always uses the entity's own axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Space {
    #[default]
    World,
    Local,
}

/// The part of the gizmo being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handle {
    /// Along (or, rotating, about) axis 0, 1 or 2.
    Axis(usize),
    /// In the plane of the two axes other than this one.
    Plane(usize),
    /// The center: move in the view plane, or scale evenly.
    Center,
}

/// The gizmo's three axes for `rotation` (the primary selection's) in `space`.
pub(crate) fn axes(space: Space, rotation: Quat) -> [Vec3; 3] {
    match space {
        Space::World => [Vec3::X, Vec3::Y, Vec3::Z],
        Space::Local => [rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z],
    }
}

/// How far along the line through `origin` in direction `axis` (a unit
/// vector) its point closest to `ray` is; `None` when they're parallel.
pub(crate) fn closest_on_axis(ray: &Ray, origin: Vec3, axis: Vec3) -> Option<f32> {
    let w = origin - ray.from;
    let b = axis.dot(ray.dir);
    let denom = 1.0 - b * b;
    if denom < 1e-6 {
        return None;
    }
    Some((b * w.dot(ray.dir) - w.dot(axis)) / denom)
}

/// Where `ray` meets the plane through `origin` with `normal`.
pub(crate) fn on_plane(ray: &Ray, origin: Vec3, normal: Vec3) -> Option<Vec3> {
    let facing = normal.dot(ray.dir);
    if facing.abs() < 1e-6 {
        return None;
    }
    let t = normal.dot(origin - ray.from) / facing;
    (t >= 0.0).then(|| ray.at(t))
}

/// `starts` moved by `delta`. Snapped in world space, the first entity
/// lands on the grid along the moved axes (`moved`, world axis indices)
/// and the others move with it; in local space, the move along each of
/// `axes` is a whole number of steps.
pub(crate) fn translate(
    starts: &[(Entity, Transform)],
    delta: Vec3,
    space: Space,
    axes: [Vec3; 3],
    moved: [bool; 3],
    snapping: Snap,
) -> Vec<(Entity, Transform)> {
    let mut delta = delta;
    if snapping.on
        && let Some((_, first)) = starts.first()
    {
        delta = match space {
            Space::World => {
                let mut to = first.position + delta;
                for i in 0..3 {
                    if moved[i] {
                        to[i] = snap(to[i], snapping.step);
                    }
                }
                to - first.position
            }
            Space::Local => (0..3).map(|i| axes[i] * snap(delta.dot(axes[i]), snapping.step)).sum(),
        };
    }
    starts
        .iter()
        .map(|&(e, t)| {
            (
                e,
                Transform {
                    position: t.position + delta,
                    ..t
                },
            )
        })
        .collect()
}

/// `starts` turned by `angle` radians about `axis` through `pivot`
/// (snapped to whole angle steps).
pub(crate) fn rotate(
    starts: &[(Entity, Transform)],
    pivot: Vec3,
    axis: Vec3,
    angle: f32,
    snapping: Snap,
) -> Vec<(Entity, Transform)> {
    let angle = if snapping.on {
        snap(angle, snapping.angle)
    } else {
        angle
    };
    let q = Quat::from_axis_angle(axis.normalize_or(Vec3::Y), angle);
    starts
        .iter()
        .map(|&(e, t)| {
            (
                e,
                Transform {
                    position: pivot + q * (t.position - pivot),
                    rotation: (q * t.rotation).normalize(),
                    scale: t.scale,
                },
            )
        })
        .collect()
}

/// `starts` with their own scale multiplied by `factors` (per local axis;
/// each snapped to whole steps from 1). Positions stay.
pub(crate) fn scale(starts: &[(Entity, Transform)], factors: Vec3, snapping: Snap) -> Vec<(Entity, Transform)> {
    let factors = if snapping.on {
        factors.map(|f| 1.0 + snap(f - 1.0, snapping.scale))
    } else {
        factors
    };
    let factors = factors.map(|f| if f.abs() < 1e-3 { 1e-3f32.copysign(f) } else { f });
    starts
        .iter()
        .map(|&(e, t)| {
            (
                e,
                Transform {
                    scale: t.scale * factors,
                    ..t
                },
            )
        })
        .collect()
}

/// Where a drag grabbed the gizmo.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grab {
    /// At this distance along the axis.
    Along(f32),
    /// At this point of the handle's plane.
    Plane(Vec3),
    /// At this direction from the pivot, in the turning plane.
    Turn(Vec3),
}

/// A gizmo drag in progress: the entities and where they started, and
/// where the mouse grabbed which handle.
#[derive(Debug, Clone)]
pub(crate) struct Drag {
    pub mode: Mode,
    pub handle: Handle,
    pub space: Space,
    pub pivot: Vec3,
    pub axes: [Vec3; 3],
    /// The view direction at the start, the plane of the center handle.
    view: Vec3,
    grab: Grab,
    pub starts: Vec<(Entity, Transform)>,
}

impl Drag {
    /// Starts dragging `handle` of the gizmo at the middle of `starts`
    /// (world transforms, the primary selection first) with the mouse on
    /// `ray`. `None` when the ray can't grab it (along an axis seen end on).
    pub fn begin(
        mode: Mode,
        handle: Handle,
        space: Space,
        starts: Vec<(Entity, Transform)>,
        view: Vec3,
        ray: &Ray,
    ) -> Option<Self> {
        let first = starts.first()?.1;
        let pivot = starts.iter().map(|(_, t)| t.position).sum::<Vec3>() / starts.len() as f32;
        let axes = match mode {
            Mode::Scale => axes(Space::Local, first.rotation),
            _ => axes(space, first.rotation),
        };
        let view = view.normalize_or(Vec3::NEG_Z);
        let grab = match (mode, handle) {
            (Mode::Rotate, Handle::Axis(i)) => Grab::Turn(on_plane(ray, pivot, axes[i])? - pivot),
            (Mode::Rotate, _) => Grab::Turn(on_plane(ray, pivot, view)? - pivot),
            (_, Handle::Axis(i)) => Grab::Along(closest_on_axis(ray, pivot, axes[i])?),
            (_, Handle::Plane(n)) => Grab::Plane(on_plane(ray, pivot, axes[n])?),
            (_, Handle::Center) => Grab::Plane(on_plane(ray, pivot, view)?),
        };
        Some(Self {
            mode,
            handle,
            space,
            pivot,
            axes,
            view,
            grab,
            starts,
        })
    }

    /// The entities' transforms with the mouse now on `ray`; `None` when
    /// the ray doesn't reach the handle's line or plane (keep the last).
    pub fn update(&self, ray: &Ray, snapping: Snap) -> Option<Vec<(Entity, Transform)>> {
        Some(match (self.mode, self.grab) {
            (Mode::Translate, Grab::Along(s0)) => {
                let Handle::Axis(i) = self.handle else { return None };
                let s = closest_on_axis(ray, self.pivot, self.axes[i])?;
                let mut moved = [false; 3];
                moved[i] = true;
                translate(
                    &self.starts,
                    self.axes[i] * (s - s0),
                    self.space,
                    self.axes,
                    moved,
                    snapping,
                )
            }
            (Mode::Translate, Grab::Plane(p0)) => {
                let (normal, moved) = match self.handle {
                    Handle::Plane(n) => {
                        let mut moved = [true; 3];
                        moved[n] = false;
                        (self.axes[n], moved)
                    }
                    _ => (self.view, [true; 3]),
                };
                let p = on_plane(ray, self.pivot, normal)?;
                translate(&self.starts, p - p0, self.space, self.axes, moved, snapping)
            }
            (Mode::Rotate, Grab::Turn(v0)) => {
                let axis = match self.handle {
                    Handle::Axis(i) => self.axes[i],
                    _ => self.view,
                };
                let v = on_plane(ray, self.pivot, axis)? - self.pivot;
                let angle = axis.dot(v0.cross(v)).atan2(v0.dot(v));
                rotate(&self.starts, self.pivot, axis, angle, snapping)
            }
            (Mode::Scale, Grab::Along(s0)) => {
                let Handle::Axis(i) = self.handle else { return None };
                let s = closest_on_axis(ray, self.pivot, self.axes[i])?;
                let mut factors = Vec3::ONE;
                factors[i] = if s0.abs() < 1e-6 { 1.0 } else { s / s0 };
                scale(&self.starts, factors, snapping)
            }
            (Mode::Scale, Grab::Plane(p0)) => {
                let normal = match self.handle {
                    Handle::Plane(n) => self.axes[n],
                    _ => self.view,
                };
                let p = on_plane(ray, self.pivot, normal)?;
                let (d0, d) = ((p0 - self.pivot).length(), (p - self.pivot).length());
                let f = if d0 < 1e-6 { 1.0 } else { d / d0 };
                let factors = match self.handle {
                    Handle::Plane(n) => {
                        let mut v = Vec3::splat(f);
                        v[n] = 1.0;
                        v
                    }
                    _ => Vec3::splat(f),
                };
                scale(&self.starts, factors, snapping)
            }
            _ => return None,
        })
    }
}

/// The gizmo's size on screen, in pixels from its center to an axis tip.
pub(crate) const GIZMO_PIXELS: f32 = 90.0;
/// How close (pixels) the mouse must be to a handle to grab it.
pub(crate) const GRAB_PIXELS: f32 = 8.0;
/// Points per rotation ring.
const RING_POINTS: usize = 48;

/// Where the gizmo's parts are on screen.
#[derive(Debug, Clone, Default)]
pub(crate) struct Layout {
    pub center: Vec2,
    /// Each axis' tip; `None` when behind the camera.
    pub tips: [Option<Vec2>; 3],
    /// The middle of each plane handle (the plane of the other two axes).
    pub planes: [Option<Vec2>; 3],
    /// Each axis' rotation ring, as points (rotating only).
    pub rings: [Vec<Vec2>; 3],
}

/// Lays the gizmo for `mode` out on screen around `pivot` with `axes`;
/// `None` when the pivot is behind the camera.
pub(crate) fn layout(camera: &Camera, mode: Mode, pivot: Vec3, axes: [Vec3; 3]) -> Option<Layout> {
    let center = world_to_screen(camera, pivot)?;
    let length = GIZMO_PIXELS * pixel_size(camera, pivot);
    let mut out = Layout {
        center,
        ..Layout::default()
    };
    for i in 0..3 {
        out.tips[i] = world_to_screen(camera, pivot + axes[i] * length);
        let (a, b) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
        out.planes[i] = world_to_screen(camera, pivot + (a + b) * length * 0.3);
        if mode == Mode::Rotate {
            out.rings[i] = (0..=RING_POINTS)
                .filter_map(|k| {
                    let t = k as f32 / RING_POINTS as f32 * std::f32::consts::TAU;
                    world_to_screen(camera, pivot + (a * t.cos() + b * t.sin()) * length)
                })
                .collect();
        }
    }
    Some(out)
}

/// Distance from `p` to the segment `a`–`b`.
pub(crate) fn to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() < 1e-6 {
        0.0
    } else {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    };
    p.distance(a + ab * t)
}

/// The handle under window point `mouse`, if any: the center first, then
/// the plane handles, then the nearest axis (or ring).
pub(crate) fn hit(layout: &Layout, mode: Mode, mouse: Vec2) -> Option<Handle> {
    if mode == Mode::Rotate {
        let mut best: Option<(f32, usize)> = None;
        for (i, ring) in layout.rings.iter().enumerate() {
            for pair in ring.windows(2) {
                let d = to_segment(mouse, pair[0], pair[1]);
                if d <= GRAB_PIXELS && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, i));
                }
            }
        }
        return best.map(|(_, i)| Handle::Axis(i));
    }
    if mouse.distance(layout.center) <= GRAB_PIXELS {
        return Some(Handle::Center);
    }
    for (n, at) in layout.planes.iter().enumerate() {
        if at.is_some_and(|at| mouse.distance(at) <= GRAB_PIXELS) {
            return Some(Handle::Plane(n));
        }
    }
    let mut best: Option<(f32, usize)> = None;
    for (i, tip) in layout.tips.iter().enumerate() {
        let Some(tip) = *tip else { continue };
        let d = to_segment(mouse, layout.center, tip);
        if d <= GRAB_PIXELS && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| Handle::Axis(i))
}

/// Radians the fly camera turns per pixel the mouse moves.
pub(crate) const LOOK_SPEED: f32 = 0.004;

/// Flies `camera`: turns it by `look` (mouse pixels: right turns right,
/// down looks down; it never rolls or turns past straight up or down),
/// then moves it by `step` along its own right, up and forward.
pub(crate) fn fly(camera: &mut Camera, look: Vec2, step: Vec3) {
    let (yaw, pitch, _) = camera.rotation.to_euler(EulerRot::YXZ);
    let yaw = yaw - look.x * LOOK_SPEED;
    let limit = 89f32.to_radians();
    let pitch = (pitch - look.y * LOOK_SPEED).clamp(-limit, limit);
    camera.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    camera.position += camera.rotation * Vec3::new(step.x, step.y, -step.z);
}

/// Moves `camera` back along its view so the sphere (`center`, `radius`)
/// fills about the middle of the view, keeping where it looks.
pub(crate) fn focus(camera: &mut Camera, center: Vec3, radius: f32) {
    let radius = radius.max(0.25);
    let distance = radius / (camera.fov_y * 0.5).sin() * 1.2;
    camera.position = center - camera.forward() * distance;
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    fn e(n: u64) -> Entity {
        Entity::from_bits(n)
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    /// A camera at (0, 0, 10) looking down -Z, 800×600.
    fn camera() -> Camera {
        let mut c = crate::render::SceneSettings::new(Vec2::new(800.0, 600.0)).camera;
        c.position = vec3(0.0, 0.0, 10.0);
        c.rotation = Quat::IDENTITY;
        c
    }

    #[test]
    fn a_ray_enters_the_nearest_side_of_a_sphere() {
        let ray = Ray::new(vec3(0.0, 0.0, 10.0), Vec3::NEG_Z);
        assert!((ray_sphere(&ray, Vec3::ZERO, 1.0).unwrap() - 9.0).abs() < 1e-5);
        assert_eq!(ray_sphere(&ray, Vec3::ZERO, 20.0), Some(0.0), "inside");
        assert_eq!(ray_sphere(&ray, vec3(3.0, 0.0, 0.0), 1.0), None, "beside");
        assert_eq!(ray_sphere(&ray, vec3(0.0, 0.0, 20.0), 1.0), None, "behind");
    }

    #[test]
    fn picking_takes_the_nearest_model_and_skips_what_is_dragged() {
        let mut world = World::new();
        let near = world.spawn((Transform::at(vec3(0.0, 0.0, 2.0)),));
        let far = world.spawn((Transform::at(vec3(0.0, 0.0, -2.0)).scaled(4.0),));
        let aside = world.spawn((Transform::at(vec3(5.0, 0.0, 0.0)),));
        let assets = crate::three::Assets3D::default();
        let ray = Ray::new(vec3(0.0, 0.0, 10.0), Vec3::NEG_Z);
        let hit = pick(&world, &assets, &ray, &[]).unwrap();
        assert_eq!(hit.entity, near);
        assert!(close(hit.point, vec3(0.0, 0.0, 2.0 + EMPTY_RADIUS)));
        assert_eq!(pick(&world, &assets, &ray, &[near]).unwrap().entity, far);
        let miss = Ray::new(vec3(0.0, 5.0, 10.0), Vec3::NEG_Z);
        assert_eq!(pick(&world, &assets, &miss, &[]), None);
        let _ = aside;
    }

    #[test]
    fn screen_points_and_rays_agree() {
        let camera = camera();
        let p = vec3(1.5, -0.5, 2.0);
        let at = world_to_screen(&camera, p).unwrap();
        let ray = Ray::from_screen(&camera, at);
        let s = closest_on_axis(&ray, Vec3::ZERO, p.normalize()).unwrap();
        assert!(close(p.normalize() * s, p), "the ray through p's pixel passes p");
        assert_eq!(world_to_screen(&camera, vec3(0.0, 0.0, 20.0)), None, "behind");
        assert!(close(
            world_to_screen(&camera, Vec3::ZERO).unwrap().extend(0.0),
            vec3(400.0, 300.0, 0.0)
        ));
    }

    #[test]
    fn the_ground_point_is_on_the_plane_or_ahead() {
        let down = Ray::new(vec3(0.0, 10.0, 0.0), vec3(1.0, -1.0, 0.0));
        assert!(close(ground_point(&down, 0.0, 5.0), vec3(10.0, 0.0, 0.0)));
        let up = Ray::new(Vec3::ZERO + Vec3::Y, Vec3::Y);
        assert!(close(ground_point(&up, 0.0, 5.0), vec3(0.0, 6.0, 0.0)));
    }

    #[test]
    fn dragging_an_axis_moves_along_it_only() {
        let start = Transform::at(vec3(1.0, 2.0, 3.0));
        let grab = Ray::new(vec3(1.0, 10.0, 3.0), Vec3::NEG_Y); // straight down at the pivot
        let drag = Drag::begin(
            Mode::Translate,
            Handle::Axis(0),
            Space::World,
            vec![(e(1), start)],
            Vec3::NEG_Y,
            &grab,
        )
        .unwrap();
        let to = Ray::new(vec3(3.7, 10.0, 9.0), Vec3::NEG_Y); // 2.7 along x, 6 along z
        let moved = drag.update(&to, Snap::default()).unwrap();
        assert!(
            close(moved[0].1.position, vec3(3.7, 2.0, 3.0)),
            "{}",
            moved[0].1.position
        );
        let snapped = drag
            .update(
                &to,
                Snap {
                    on: true,
                    step: 0.5,
                    ..Snap::default()
                },
            )
            .unwrap();
        assert!(
            close(snapped[0].1.position, vec3(3.5, 2.0, 3.0)),
            "the grid, in world space"
        );
    }

    #[test]
    fn local_space_axes_follow_the_entity_and_snap_the_move() {
        let turned = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2); // local +X is world -Z
        let start = Transform::default().rotated(turned);
        let grab = Ray::new(vec3(0.0, 10.0, 0.0), Vec3::NEG_Y);
        let drag = Drag::begin(
            Mode::Translate,
            Handle::Axis(0),
            Space::Local,
            vec![(e(1), start)],
            Vec3::NEG_Y,
            &grab,
        )
        .unwrap();
        let to = Ray::new(vec3(4.0, 10.0, -1.3), Vec3::NEG_Y);
        let moved = drag.update(&to, Snap::default()).unwrap();
        assert!(close(moved[0].1.position, vec3(0.0, 0.0, -1.3)));
        let snapped = drag
            .update(
                &to,
                Snap {
                    on: true,
                    step: 1.0,
                    ..Snap::default()
                },
            )
            .unwrap();
        assert!(
            close(snapped[0].1.position, vec3(0.0, 0.0, -1.0)),
            "whole steps along local +X"
        );
    }

    #[test]
    fn a_plane_handle_moves_in_its_plane_and_keeps_the_group_together() {
        let a = Transform::at(vec3(0.0, 0.0, 0.0));
        let b = Transform::at(vec3(2.0, 0.0, 0.0));
        let grab = Ray::new(vec3(1.0, 10.0, 0.0), Vec3::NEG_Y);
        let drag = Drag::begin(
            Mode::Translate,
            Handle::Plane(1),
            Space::World,
            vec![(e(1), a), (e(2), b)],
            Vec3::NEG_Y,
            &grab,
        )
        .unwrap();
        assert!(close(drag.pivot, vec3(1.0, 0.0, 0.0)));
        let moved = drag
            .update(&Ray::new(vec3(2.2, 10.0, 3.1), Vec3::NEG_Y), Snap::default())
            .unwrap();
        assert!(close(moved[0].1.position, vec3(1.2, 0.0, 3.1)));
        assert!(close(moved[1].1.position, vec3(3.2, 0.0, 3.1)));
        let snapped = drag
            .update(
                &Ray::new(vec3(2.2, 10.0, 3.1), Vec3::NEG_Y),
                Snap {
                    on: true,
                    step: 1.0,
                    ..Snap::default()
                },
            )
            .unwrap();
        assert!(
            close(snapped[0].1.position, vec3(1.0, 0.0, 3.0)),
            "the first one lands on the grid"
        );
        assert!(
            close(snapped[1].1.position, vec3(3.0, 0.0, 3.0)),
            "the rest keep their distance"
        );
    }

    #[test]
    fn rotating_turns_about_the_axis_through_the_pivot_in_snapped_steps() {
        let a = Transform::at(vec3(1.0, 0.0, 0.0));
        let b = Transform::at(vec3(3.0, 0.0, 0.0));
        let starts = vec![(e(1), a), (e(2), b)];
        // Grab the Y ring at +X from the pivot (2, 0, 0), from above.
        let grab = Ray::new(vec3(3.0, 10.0, 0.0), Vec3::NEG_Y);
        let drag = Drag::begin(Mode::Rotate, Handle::Axis(1), Space::World, starts, Vec3::NEG_Y, &grab).unwrap();
        // Drag to -Z from the pivot: a quarter turn about +Y.
        let to = Ray::new(vec3(2.0, 10.0, -1.0), Vec3::NEG_Y);
        let turned = drag.update(&to, Snap::default()).unwrap();
        let quarter = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        assert!(turned[0].1.rotation.angle_between(quarter) < 1e-4);
        assert!(close(turned[0].1.position, vec3(2.0, 0.0, 1.0)));
        assert!(close(turned[1].1.position, vec3(2.0, 0.0, -1.0)));
        // 50° snaps to 45° with 15° steps.
        let fifty = 50f32.to_radians();
        let to = Ray::new(vec3(2.0 + fifty.cos(), 10.0, -fifty.sin()), Vec3::NEG_Y);
        let snapped = drag
            .update(
                &to,
                Snap {
                    on: true,
                    ..Snap::default()
                },
            )
            .unwrap();
        let (yaw, _, _) = snapped[0].1.rotation.to_euler(EulerRot::YXZ);
        assert!((yaw.to_degrees() - 45.0).abs() < 1e-3, "{}", yaw.to_degrees());
    }

    #[test]
    fn scaling_an_axis_scales_the_entity_along_its_own_axis() {
        let turned = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let start = Transform::default().rotated(turned);
        // Local +X is world -Z: grab it 1 unit out, drag to 2.5.
        let grab = Ray::new(vec3(0.0, 10.0, -1.0), Vec3::NEG_Y);
        let drag = Drag::begin(
            Mode::Scale,
            Handle::Axis(0),
            Space::World,
            vec![(e(1), start)],
            Vec3::NEG_Y,
            &grab,
        )
        .unwrap();
        let to = Ray::new(vec3(0.0, 10.0, -2.5), Vec3::NEG_Y);
        let scaled = drag.update(&to, Snap::default()).unwrap();
        assert!(close(scaled[0].1.scale, vec3(2.5, 1.0, 1.0)));
        assert!(close(scaled[0].1.position, Vec3::ZERO), "positions stay");
        let to = Ray::new(vec3(0.0, 10.0, -2.43), Vec3::NEG_Y);
        let snapped = drag
            .update(
                &to,
                Snap {
                    on: true,
                    ..Snap::default()
                },
            )
            .unwrap();
        assert!(close(snapped[0].1.scale, vec3(2.4, 1.0, 1.0)));
        // The center scales evenly.
        let grab = Ray::new(vec3(1.0, 10.0, 0.0), Vec3::NEG_Y);
        let even = Drag::begin(
            Mode::Scale,
            Handle::Center,
            Space::World,
            vec![(e(1), start)],
            Vec3::NEG_Y,
            &grab,
        )
        .unwrap();
        let out = even
            .update(&Ray::new(vec3(0.0, 10.0, 3.0), Vec3::NEG_Y), Snap::default())
            .unwrap();
        assert!(close(out[0].1.scale, Vec3::splat(3.0)));
    }

    #[test]
    fn an_axis_seen_end_on_cannot_be_grabbed() {
        let grab = Ray::new(vec3(0.0, 10.0, 0.0), Vec3::NEG_Y);
        let starts = vec![(e(1), Transform::default())];
        assert!(
            Drag::begin(
                Mode::Translate,
                Handle::Axis(1),
                Space::World,
                starts,
                Vec3::NEG_Y,
                &grab
            )
            .is_none()
        );
    }

    #[test]
    fn the_handle_under_the_mouse_is_found_on_screen() {
        let camera = camera();
        let pivot = Vec3::ZERO;
        let l = layout(&camera, Mode::Translate, pivot, axes(Space::World, Quat::IDENTITY)).unwrap();
        assert_eq!(hit(&l, Mode::Translate, l.center), Some(Handle::Center));
        let x_tip = l.tips[0].unwrap();
        assert!(
            (x_tip.x - l.center.x - GIZMO_PIXELS).abs() < 1.0,
            "a fixed size on screen"
        );
        let on_x = l.center.lerp(x_tip, 0.8);
        assert_eq!(hit(&l, Mode::Translate, on_x), Some(Handle::Axis(0)));
        let on_y = l.center.lerp(l.tips[1].unwrap(), 0.8);
        assert_eq!(hit(&l, Mode::Translate, on_y), Some(Handle::Axis(1)));
        assert_eq!(hit(&l, Mode::Translate, l.planes[2].unwrap()), Some(Handle::Plane(2)));
        assert_eq!(hit(&l, Mode::Translate, l.center + Vec2::new(-60.0, 60.0)), None);

        let r = layout(&camera, Mode::Rotate, pivot, axes(Space::World, Quat::IDENTITY)).unwrap();
        // The Z ring faces the camera: a circle of GIZMO_PIXELS around the center.
        let on_ring = r.center + Vec2::new(GIZMO_PIXELS, 0.0).rotate(Vec2::from_angle(0.7));
        assert_eq!(hit(&r, Mode::Rotate, on_ring), Some(Handle::Axis(2)));
        assert_eq!(hit(&r, Mode::Rotate, r.center + Vec2::new(30.0, 30.0)), None);
    }

    #[test]
    fn the_fly_camera_turns_without_rolling_and_moves_along_its_view() {
        let mut c = camera();
        fly(&mut c, Vec2::new(0.0, 0.0), vec3(0.0, 0.0, 2.0));
        assert!(close(c.position, vec3(0.0, 0.0, 8.0)), "forward is -Z");
        let quarter = std::f32::consts::FRAC_PI_2 / LOOK_SPEED;
        fly(&mut c, Vec2::new(quarter, 0.0), vec3(0.0, 0.0, 1.0));
        assert!(close(c.forward(), Vec3::X), "a quarter turn right: {}", c.forward());
        assert!(close(c.position, vec3(1.0, 0.0, 8.0)));
        fly(&mut c, Vec2::new(0.0, 10_000.0), Vec3::ZERO);
        assert!(
            c.forward().y > -1.0 && c.forward().y < -0.99,
            "stops short of straight down"
        );
        let right = c.rotation * Vec3::X;
        assert!(right.y.abs() < 1e-4, "no roll");
    }

    #[test]
    fn focus_frames_the_sphere_in_front_of_the_camera() {
        let mut c = camera();
        focus(&mut c, vec3(5.0, 1.0, -3.0), 2.0);
        let to = vec3(5.0, 1.0, -3.0) - c.position;
        assert!(close(to.normalize(), c.forward()));
        let half = (to.length() * (c.fov_y * 0.5).sin()).abs();
        assert!(half > 2.0, "the whole sphere is in view");
    }
}
