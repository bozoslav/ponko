//! Ribbons and lines: camera-facing strips built on the CPU each frame and
//! drawn with the particles (same blending, textures and pass).
//!
//! - [`Trail3D`] leaves a ribbon behind a moving entity: tracers, sword
//!   swings, streaks behind fast things.
//! - [`Line3D`] draws a line through points: beams, cables, ropes, paths.

use std::collections::VecDeque;

use glam::Vec3;

use super::particles::ParticleBlend3D;
use super::{Camera3D, Transform3D};
use crate::particles::{ParticleCurve, ParticleGradient};
use crate::{color::Color, texture::Texture};
use ponko_ecs::Component;

/// A ribbon left behind its entity as it moves: tracers, sword swings,
/// streaks behind cars or projectiles. It faces the camera, is widest where
/// `width` says (over age, 0 at the entity, 1 at the end) and fades by
/// `color`. Points are recorded in the world, so the ribbon stays where the
/// entity went. Like particles, trails are only for show: headless runs
/// don't record them.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 64, 64).run_headless(0, |ctx| {
///     // A tracer: a thin, glowing ribbon a quarter second long.
///     let mut tracer = ponko::d3::Trail::new(0.25, 0.05);
///     tracer.blend = ponko::d3::ParticleBlend::Additive;
///     tracer.color = ponko::particles::ParticleGradient::new(vec![(0.0, Color::rgb(1.0, 0.8, 0.3)), (1.0, Color::rgba(1.0, 0.3, 0.0, 0.0))]);
///     ctx.world.spawn((ponko::d3::Transform::default(), tracer));
/// });
/// ```
#[derive(Debug, Clone, Component)]
pub struct Trail3D {
    /// Seconds a point lasts before the ribbon lets go of it.
    pub lifetime: f32,
    /// Width in meters over age (0 the newest end, 1 the oldest).
    pub width: ParticleCurve,
    /// Color over age (0 the newest end, 1 the oldest).
    pub color: ParticleGradient,
    /// Image along the ribbon: u is each point's age over `lifetime` (0 at
    /// the entity), v across. `None` is plain white.
    pub texture: Option<Texture>,
    /// Alpha (fades over what's behind) or additive (glows).
    pub blend: ParticleBlend3D,
    /// A new point is kept only after the entity moved this far (meters)
    /// from the last one; the ribbon still reaches the entity in between.
    pub min_distance: f32,
    /// At most this many points; the oldest go first.
    pub max_points: usize,
    /// While false, no new points are added; the ribbon fades out.
    pub emitting: bool,
    /// Kept points, oldest first: where, and how old (seconds).
    points: VecDeque<(Vec3, f32)>,
}

impl Trail3D {
    /// A ribbon `width` meters wide (narrowing to 0 at its end) whose points
    /// last `lifetime` seconds, white and alpha blended.
    /// ```
    /// # use ponko::prelude::*;
    /// let t = ponko::d3::Trail::new(0.5, 0.1);
    /// assert_eq!(t.width.sample(0.0), 0.1);
    /// assert_eq!(t.width.sample(1.0), 0.0);
    /// ```
    #[must_use]
    pub fn new(lifetime: f32, width: f32) -> Self {
        Self {
            lifetime,
            width: ParticleCurve::new(vec![(0.0, width), (1.0, 0.0)]),
            color: ParticleGradient::new(vec![(0.0, Color::WHITE), (1.0, Color::rgba(1.0, 1.0, 1.0, 0.0))]),
            texture: None,
            blend: ParticleBlend3D::Alpha,
            min_distance: 0.05,
            max_points: 64,
            emitting: true,
            points: VecDeque::new(),
        }
    }

    /// How many points it keeps now.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::d3::Trail::new(1.0, 0.1).point_count(), 0);
    /// ```
    #[must_use]
    pub fn point_count(&self) -> usize {
        self.points.len()
    }

    /// Forgets every point: after a teleport, so no ribbon spans the jump.
    /// ```
    /// # use ponko::prelude::*;
    /// let mut t = ponko::d3::Trail::new(1.0, 0.1);
    /// t.clear();
    /// assert_eq!(t.point_count(), 0);
    /// ```
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// Ages the points by `dt` and records the entity at `at`.
    pub(crate) fn update(&mut self, dt: f32, at: Vec3) {
        for p in &mut self.points {
            p.1 += dt;
        }
        let lifetime = self.lifetime.max(1e-4);
        while self.points.front().is_some_and(|p| p.1 >= lifetime) {
            self.points.pop_front();
        }
        if !self.emitting || !at.is_finite() {
            return;
        }
        let far = self
            .points
            .back()
            .is_none_or(|p| p.0.distance_squared(at) >= self.min_distance * self.min_distance);
        if far {
            self.points.push_back((at, 0.0));
            while self.points.len() > self.max_points.max(2) {
                self.points.pop_front();
            }
        }
    }
}

/// A line through `points` (in its entity's space), drawn as a camera-facing
/// strip `width` meters wide: laser beams, cables, ropes, power lines,
/// paths. Change `points` any time; it's rebuilt every frame it's seen.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 64, 64).run_headless(0, |ctx| {
///     // A glowing beam 20 m forward.
///     let beam = ponko::d3::Line::new(vec![Vec3::ZERO, vec3(0.0, 0.0, -20.0)], 0.02, Color::RED);
///     ctx.world.spawn((ponko::d3::Transform::default(), ponko::d3::Line { blend: ponko::d3::ParticleBlend::Additive, ..beam }));
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Component)]
pub struct Line3D {
    /// At least two to draw anything.
    pub points: Vec<Vec3>,
    /// Meters, scaled by the entity's largest scale.
    pub width: f32,
    /// Its color (and opacity) along its whole length.
    pub color: Color,
    /// Image along the line: u along it, v across. `None` is plain white.
    pub texture: Option<Texture>,
    /// Alpha (fades over what's behind) or additive (glows).
    pub blend: ParticleBlend3D,
    /// Meters of line per repeat of `texture`; 0 stretches it once over the
    /// whole line.
    pub texture_length: f32,
}

impl Line3D {
    /// A `color` line `width` meters wide through `points`, alpha blended.
    /// ```
    /// # use ponko::prelude::*;
    /// let l = ponko::d3::Line::new(vec![Vec3::ZERO, Vec3::X], 0.1, Color::WHITE);
    /// assert_eq!(l.texture_length, 0.0);
    /// ```
    #[must_use]
    pub fn new(points: Vec<Vec3>, width: f32, color: Color) -> Self {
        Self {
            points,
            width,
            color,
            texture: None,
            blend: ParticleBlend3D::Alpha,
            texture_length: 0.0,
        }
    }
}

/// One corner of a strip, in the world.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct StripVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// A strip's vertices (a triangle list) in the frame's buffer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StripRun {
    pub first: u32,
    pub count: u32,
    pub texture: Option<u32>,
    pub blend: ParticleBlend3D,
    pub depth: f32,
}

/// This frame's strips, farthest first.
#[derive(Default)]
pub(crate) struct StripFrame {
    pub vertices: Vec<StripVertex>,
    pub runs: Vec<StripRun>,
    /// Scratch: one strip's points, widths, colors and u.
    points: Vec<(Vec3, f32, [f32; 4], f32)>,
}

impl StripFrame {
    pub fn collect(&mut self, world: &mut ponko_ecs::World, camera: &Camera3D) {
        self.vertices.clear();
        self.runs.clear();
        let (vertices, runs, points) = (&mut self.vertices, &mut self.runs, &mut self.points);
        world.query::<(&Trail3D, &Transform3D)>().for_each(|(t, at)| {
            points.clear();
            let n = t.points.len();
            if n == 0 {
                return;
            }
            let lifetime = t.lifetime.max(1e-4);
            // The newest end follows the entity even between kept points.
            let head = at.position;
            let mut add = |p: Vec3, age: f32| {
                let a = (age / lifetime).clamp(0.0, 1.0);
                points.push((p, t.width.sample(a), super::linear(t.color.sample(a)), a));
            };
            if t.emitting && t.points.back().is_some_and(|p| p.0 != head) {
                add(head, 0.0);
            }
            for &(p, age) in t.points.iter().rev() {
                add(p, age);
            }
            build(vertices, runs, points, camera, t.texture, t.blend);
        });
        world.query::<(&Line3D, &Transform3D)>().for_each(|(l, at)| {
            points.clear();
            if l.points.len() < 2 {
                return;
            }
            let matrix = at.matrix();
            let width = l.width * at.scale.abs().max_element();
            let color = super::linear(l.color);
            let total: f32 = l
                .points
                .windows(2)
                .map(|w| matrix.transform_vector3(w[1] - w[0]).length())
                .sum();
            let mut along = 0.0;
            let mut last = None;
            for &p in &l.points {
                let p = matrix.transform_point3(p);
                if let Some(q) = last {
                    along += p.distance(q);
                }
                last = Some(p);
                let u = if l.texture_length > 0.0 {
                    along / l.texture_length
                } else {
                    along / total.max(1e-6)
                };
                points.push((p, width, color, u));
            }
            build(vertices, runs, points, camera, l.texture, l.blend);
        });
        runs.sort_unstable_by(|a, b| b.depth.total_cmp(&a.depth));
    }
}

/// Adds the strip through `points` (position, width, linear color, u),
/// facing the camera, as two triangles per segment.
fn build(
    vertices: &mut Vec<StripVertex>,
    runs: &mut Vec<StripRun>,
    points: &[(Vec3, f32, [f32; 4], f32)],
    camera: &Camera3D,
    texture: Option<Texture>,
    blend: ParticleBlend3D,
) {
    if points.len() < 2 || points.iter().any(|p| !p.0.is_finite()) {
        return;
    }
    let first = vertices.len();
    let side = |i: usize| {
        let p = points[i].0;
        let tangent = points[(i + 1).min(points.len() - 1)].0 - points[i.saturating_sub(1)].0;
        let to_eye = camera.position - p;
        let s = tangent.cross(to_eye).normalize_or_zero();
        // Looking straight along the line: any side will do.
        let s = if s == Vec3::ZERO { camera.rotation * Vec3::X } else { s };
        s * (points[i].1.max(0.0) * 0.5)
    };
    let corner = |i: usize, s: Vec3, v: f32| StripVertex {
        position: (points[i].0 + s * (1.0 - 2.0 * v)).to_array(),
        uv: [points[i].3, v],
        color: points[i].2,
    };
    let mut previous = side(0);
    // Each segment uses the points at both of its ends.
    #[allow(clippy::needless_range_loop)]
    for i in 1..points.len() {
        let next = side(i);
        let (a0, a1) = (corner(i - 1, previous, 0.0), corner(i - 1, previous, 1.0));
        let (b0, b1) = (corner(i, next, 0.0), corner(i, next, 1.0));
        vertices.extend([a0, a1, b0, b0, a1, b1]);
        previous = next;
    }
    let center = points[1..].iter().map(|p| p.0).sum::<Vec3>() / (points.len() - 1) as f32;
    runs.push(StripRun {
        first: first as u32,
        count: (vertices.len() - first) as u32,
        texture: texture.map(|t| t.id),
        blend,
        depth: (center - camera.position).dot(camera.forward()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera3D {
        let mut c = Camera3D::new(glam::Vec2::new(100.0, 100.0));
        c.position = Vec3::new(0.0, 0.0, 10.0);
        c.look_at(Vec3::ZERO);
        c
    }

    #[test]
    fn trails_keep_spaced_points_and_drop_old_ones() {
        let mut t = Trail3D::new(1.0, 0.1);
        t.min_distance = 0.5;
        t.update(0.1, Vec3::ZERO);
        t.update(0.1, Vec3::new(0.1, 0.0, 0.0)); // too close: not kept
        t.update(0.1, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(t.point_count(), 2);
        t.update(0.85, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(t.point_count(), 1, "the first point is 1.15 s old");
        t.emitting = false;
        t.update(1.0, Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(t.point_count(), 0, "not emitting: it fades out");
        t.emitting = true;
        t.max_points = 3;
        for x in 0..10 {
            t.update(0.0, Vec3::new(x as f32, 0.0, 0.0));
        }
        assert_eq!(t.point_count(), 3);
        assert_eq!(t.points.back().unwrap().0.x, 9.0);
    }

    #[test]
    fn strips_face_the_camera_with_their_width() {
        let mut world = ponko_ecs::World::new();
        world.spawn((
            Transform3D::default(),
            Line3D::new(
                vec![Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
                0.5,
                Color::WHITE,
            ),
        ));
        let mut frame = StripFrame::default();
        frame.collect(&mut world, &camera());
        assert_eq!(frame.runs.len(), 1);
        assert_eq!(frame.vertices.len(), 6, "one segment, two triangles");
        // Seen from +Z, a line along X is widened along Y: 0.25 each side.
        for v in &frame.vertices {
            assert!((v.position[1].abs() - 0.25).abs() < 1e-5, "{v:?}");
            assert_eq!(v.position[2], 0.0);
        }
        // Triangles face the camera (counter-clockwise from +Z) or are
        // drawn double-sided; either way none is degenerate.
        for tri in frame.vertices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(tri[k].position));
            assert!((b - a).cross(c - a).length() > 1e-4);
        }
    }

    #[test]
    fn lines_follow_their_entity_and_repeat_textures_by_length() {
        let mut world = ponko_ecs::World::new();
        let mut line = Line3D::new(vec![Vec3::ZERO, Vec3::X, Vec3::new(3.0, 0.0, 0.0)], 0.1, Color::WHITE);
        line.texture_length = 0.5;
        world.spawn((Transform3D::at(Vec3::new(0.0, 2.0, 0.0)), line));
        let mut frame = StripFrame::default();
        frame.collect(&mut world, &camera());
        assert_eq!(frame.vertices.len(), 12);
        let us: Vec<f32> = frame.vertices.iter().map(|v| v.uv[0]).collect();
        assert_eq!(us.iter().copied().fold(0.0, f32::max), 6.0, "3 m at 0.5 m per repeat");
        assert!(frame.vertices.iter().all(|v| (v.position[1] - 2.0).abs() < 0.06));
    }

    #[test]
    fn trails_reach_their_entity_and_fade_along_their_length() {
        let mut world = ponko_ecs::World::new();
        let mut t = Trail3D::new(1.0, 0.2);
        t.min_distance = 10.0;
        t.update(0.5, Vec3::ZERO);
        let e = world.spawn((Transform3D::at(Vec3::new(2.0, 0.0, 0.0)), t));
        world
            .get_mut::<Trail3D>(e)
            .unwrap()
            .update(0.25, Vec3::new(2.0, 0.0, 0.0));
        let mut frame = StripFrame::default();
        frame.collect(&mut world, &camera());
        assert_eq!(frame.vertices.len(), 6, "the kept point and the entity");
        let head = frame.vertices.iter().find(|v| v.uv[0] == 0.0).unwrap();
        assert!((head.position[0] - 2.0).abs() < 1e-5);
        let tail = frame
            .vertices
            .iter()
            .max_by(|a, b| a.uv[0].total_cmp(&b.uv[0]))
            .unwrap();
        assert_eq!(tail.uv[0], 0.25, "kept 0.25 s ago: a quarter of its life");
        assert!(tail.color[3] < head.color[3]);
        // Nothing to draw with fewer than two points.
        world.get_mut::<Trail3D>(e).unwrap().clear();
        frame.collect(&mut world, &camera());
        assert!(frame.runs.is_empty() && frame.vertices.is_empty());
    }
}
