//! CPU particle pools and lifetime curves. Pools retain their storage between frames.
use super::{Camera3D, Transform3D, linear};
use crate::particles::{ParticleCurve, ParticleGradient, ParticleSpace as ParticleSpace3D};
use crate::particles2d::{Emission, advance, sheet_frame, validate};
use crate::{random::Rng, texture::Texture};
use glam::Vec3;
use ponko_ecs::{Component, World};

/// Emission volume in the entity's coordinates; cone directions point along +Y.
/// ```
/// use ponko::prelude::*;
/// let shape = ponko::d3::ParticleShape::Cone { angle: 0.4 };
/// # let _ = shape;
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub enum ParticleShape3D {
    /// Origin, with velocity along +Y.
    #[default]
    Point,
    /// Uniform volume, with outward velocity. Negative radii become zero.
    Sphere {
        /// Radius in local units.
        radius: f32,
    },
    /// Origin, with directions uniformly distributed inside the cone.
    Cone {
        /// Half angle in radians, clamped to 0..=pi.
        angle: f32,
    },
    /// Uniform box volume, with velocity along +Y.
    Box {
        /// Half extents in local units (absolute values are used).
        half_extents: Vec3,
    },
}
/// Particle compositing, always depth tested with depth writes disabled.
/// ```
/// use ponko::prelude::*;
/// let blend = ponko::d3::ParticleBlend::Additive;
/// # let _ = blend;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ParticleBlend3D {
    /// Ordinary alpha compositing (particles sorted back to front).
    #[default]
    Alpha,
    /// Emitted light adds to the scene, weighted by alpha.
    Additive,
}
/// Settings copied into an emitter. Change settings by creating a new emitter.
/// All numeric inputs must be finite; ranges must be ordered and nonnegative
/// (rotation may be negative). Invalid settings panic in `ParticleEmitter3D::new`.
/// ```
/// use ponko::prelude::*;
/// let settings = ponko::d3::ParticleSettings::default().with_rate(40.0).with_blend(ponko::d3::ParticleBlend::Additive);
/// let emitter = ponko::d3::ParticleEmitter::new(settings);
/// assert_eq!(emitter.live_count(), 0);
/// ```
///
/// New fields may be added in any release: start from [`Default`] and use
/// the `with_*` methods, or set the public fields of a `mut` value.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ParticleSettings3D {
    /// Continuous particles per second. Zero emits only bursts.
    pub rate: f32,
    /// Initial burst, emitted on the first frame (also for non-looping emitters).
    pub burst: usize,
    /// Seconds of continuous emission before stopping or looping.
    pub duration: f32,
    /// Repeat continuous emission after `duration`; initial burst does not repeat.
    pub looping: bool,
    /// Particle lifetime in seconds, minimum strictly positive.
    pub lifetime: (f32, f32),
    /// Initial speed in units per second.
    pub speed: (f32, f32),
    /// Spawn shape in emitter coordinates.
    pub shape: ParticleShape3D,
    /// Acceleration in simulation coordinates.
    pub gravity: Vec3,
    /// Exponential velocity damping per second.
    pub drag: f32,
    /// Initial billboard width and height in world units (before local scale).
    pub size: (f32, f32),
    /// Initial rotation around the billboard normal, in radians.
    pub rotation: (f32, f32),
    /// Size multiplier over normalized lifetime.
    pub size_over_life: ParticleCurve,
    /// RGBA tint over normalized lifetime.
    pub color_over_life: ParticleGradient,
    /// Optional image; absent uses white.
    pub texture: Option<Texture>,
    /// Sprite sheet (columns, rows); frames advance row-major over lifetime.
    pub sheet: (u32, u32),
    /// How billboards combine with the scene.
    pub blend: ParticleBlend3D,
    /// Hard live-particle limit. Storage is allocated when the emitter is made.
    pub max_particles: usize,
    /// World or entity coordinates.
    pub space: ParticleSpace3D,
    /// Independent RNG seed; equal seeds and frame steps give equal simulation.
    pub seed: u64,
}
impl Default for ParticleSettings3D {
    fn default() -> Self {
        Self {
            rate: 10.0,
            burst: 0,
            duration: 1.0,
            looping: true,
            lifetime: (1.0, 2.0),
            speed: (1.0, 2.0),
            shape: ParticleShape3D::Point,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            drag: 0.0,
            size: (0.1, 0.2),
            rotation: (0.0, 0.0),
            size_over_life: ParticleCurve::default(),
            color_over_life: ParticleGradient::default(),
            texture: None,
            sheet: (1, 1),
            blend: ParticleBlend3D::Alpha,
            max_particles: 1024,
            space: ParticleSpace3D::World,
            seed: 1,
        }
    }
}
impl ParticleSettings3D {
    with_fields! {
        /// Sets [`rate`](Self::rate), continuous particles per second.
        rate: f32 => with_rate;
        /// Sets [`burst`](Self::burst), the particles emitted on the first frame.
        burst: usize => with_burst;
        /// Sets [`duration`](Self::duration), seconds of continuous emission.
        duration: f32 => with_duration;
        /// Sets [`looping`](Self::looping).
        looping: bool => with_looping;
        /// Sets [`shape`](Self::shape), where particles spawn.
        shape: ParticleShape3D => with_shape;
        /// Sets [`gravity`](Self::gravity).
        gravity: Vec3 => with_gravity;
        /// Sets [`drag`](Self::drag), velocity damping per second.
        drag: f32 => with_drag;
        /// Sets [`size_over_life`](Self::size_over_life).
        size_over_life: ParticleCurve => with_size_over_life;
        /// Sets [`color_over_life`](Self::color_over_life).
        color_over_life: ParticleGradient => with_color_over_life;
        /// Sets [`texture`](Self::texture).
        texture: Option<Texture> => with_texture;
        /// Sets [`blend`](Self::blend).
        blend: ParticleBlend3D => with_blend;
        /// Sets [`max_particles`](Self::max_particles), the live-particle limit.
        max_particles: usize => with_max_particles;
        /// Sets [`space`](Self::space).
        space: ParticleSpace3D => with_space;
        /// Sets [`seed`](Self::seed).
        seed: u64 => with_seed;
    }

    /// Sets [`lifetime`](Self::lifetime): seconds, from `min` to `max`.
    #[must_use]
    pub fn with_lifetime(mut self, min: f32, max: f32) -> Self {
        self.lifetime = (min, max);
        self
    }

    /// Sets [`speed`](Self::speed): units per second, from `min` to `max`.
    #[must_use]
    pub fn with_speed(mut self, min: f32, max: f32) -> Self {
        self.speed = (min, max);
        self
    }

    /// Sets [`size`](Self::size): billboard size, from `min` to `max`.
    #[must_use]
    pub fn with_size(mut self, min: f32, max: f32) -> Self {
        self.size = (min, max);
        self
    }

    /// Sets [`rotation`](Self::rotation): radians, from `min` to `max`.
    #[must_use]
    pub fn with_rotation(mut self, min: f32, max: f32) -> Self {
        self.rotation = (min, max);
        self
    }

    /// Sets [`sheet`](Self::sheet): a sprite sheet of `columns` by `rows`.
    #[must_use]
    pub fn with_sheet(mut self, columns: u32, rows: u32) -> Self {
        self.sheet = (columns, rows);
        self
    }
}

type Particle = crate::particles2d::Particle<Vec3>;

/// A CPU particle pool component, paired with `Transform3D`. Simulation runs after
/// frame systems; no meshes are required. `run_headless` skips this visual frame phase.
/// Cloning copies the live particles and RNG state. Capped emission is discarded.
/// ```
/// use ponko::prelude::*;
/// let mut world = World::new();
/// world.spawn((ponko::d3::Transform::default(), ponko::d3::ParticleEmitter::default()));
/// ```
#[derive(Debug, Component)]
pub struct ParticleEmitter3D {
    settings: ParticleSettings3D,
    particles: Vec<Particle>,
    rng: Rng,
    emission: Emission,
}
impl Clone for ParticleEmitter3D {
    fn clone(&self) -> Self {
        let mut particles = Vec::with_capacity(self.settings.max_particles);
        particles.extend_from_slice(&self.particles);
        Self {
            settings: self.settings.clone(),
            particles,
            rng: self.rng.clone(),
            emission: self.emission.clone(),
        }
    }
}
impl Default for ParticleEmitter3D {
    fn default() -> Self {
        Self::new(ParticleSettings3D::default())
    }
}
impl ParticleEmitter3D {
    /// Allocates the entire particle pool. Panics for invalid settings.
    /// ```
    /// use ponko::prelude::*;
    /// let e = ponko::d3::ParticleEmitter::new(ponko::d3::ParticleSettings::default().with_max_particles(64));
    /// assert_eq!(e.live_count(), 0);
    /// ```
    pub fn new(settings: ParticleSettings3D) -> Self {
        let s = &settings;
        validate(
            [s.lifetime, s.speed, s.size, s.rotation],
            s.rate,
            s.duration,
            s.drag,
            s.gravity.is_finite(),
            s.sheet,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        match s.shape {
            ParticleShape3D::Sphere { radius } => assert!(radius.is_finite(), "invalid particle sphere"),
            ParticleShape3D::Cone { angle } => assert!(angle.is_finite(), "invalid particle cone"),
            ParticleShape3D::Box { half_extents } => assert!(half_extents.is_finite(), "invalid particle box"),
            _ => {}
        }
        Self {
            particles: Vec::with_capacity(s.max_particles),
            rng: Rng::new(s.seed),
            emission: Emission::new(s.burst, s.max_particles),
            settings,
        }
    }
    /// Queues particles for the next frame, capped at the pool size; works after a one-shot ends.
    /// ```
    /// use ponko::prelude::*;
    /// let mut e = ponko::d3::ParticleEmitter::default();
    /// e.burst(20);
    /// ```
    pub fn burst(&mut self, count: usize) {
        self.emission.burst(count, self.settings.max_particles);
    }
    /// Number of currently living particles.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::d3::ParticleEmitter::default().live_count(), 0);
    /// ```
    pub fn live_count(&self) -> usize {
        self.particles.len()
    }

    pub(crate) fn update(&mut self, dt: f32, transform: &Transform3D) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        let s = &self.settings;
        advance(&mut self.particles, dt, s.gravity, s.drag);
        let free = s.max_particles - self.particles.len();
        let count = self.emission.take(dt, s.rate, s.duration, s.looping, free);
        for _ in 0..count {
            let (position, direction) = spawn_shape(s.shape, &mut self.rng);
            let velocity = direction * self.rng.range(s.speed.0, s.speed.1);
            let (position, velocity) = if s.space == ParticleSpace3D::World {
                (
                    transform.matrix().transform_point3(position),
                    transform.rotation * velocity,
                )
            } else {
                (position, velocity)
            };
            self.particles.push(Particle {
                position,
                velocity,
                age: 0.0,
                lifetime: self.rng.range(s.lifetime.0, s.lifetime.1),
                size: self.rng.range(s.size.0, s.size.1),
                rotation: self.rng.range(s.rotation.0, s.rotation.1),
            });
        }
    }
}
fn spawn_shape(shape: ParticleShape3D, rng: &mut Rng) -> (Vec3, Vec3) {
    let direction = |rng: &mut Rng, y: f32| {
        let angle = rng.angle();
        let r = (1.0 - y * y).max(0.0).sqrt();
        Vec3::new(angle.cos() * r, y, angle.sin() * r)
    };
    match shape {
        ParticleShape3D::Point => (Vec3::ZERO, Vec3::Y),
        ParticleShape3D::Sphere { radius } => {
            let y = rng.range(-1.0, 1.0);
            let d = direction(rng, y);
            (d * radius.max(0.0) * rng.f32().cbrt(), d)
        }
        ParticleShape3D::Cone { angle } => {
            let y = rng.range(angle.clamp(0.0, std::f32::consts::PI).cos(), 1.0);
            (Vec3::ZERO, direction(rng, y))
        }
        ParticleShape3D::Box { half_extents } => (
            Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)) * half_extents.abs(),
            Vec3::Y,
        ),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ParticleInstance {
    pub position_size: [f32; 4],
    pub color: [f32; 4],
    pub rotation_sheet: [f32; 4],
}
#[derive(Clone, Copy)]
pub(crate) struct ParticleRun {
    pub first: u32,
    pub count: u32,
    pub texture: Option<u32>,
    pub blend: ParticleBlend3D,
    depth: f32,
}
#[derive(Default)]
pub(crate) struct ParticleFrame {
    pub instances: Vec<ParticleInstance>,
    pub runs: Vec<ParticleRun>,
    pub right: [f32; 4],
    pub up: [f32; 4],
    /// Ribbons and lines, drawn with the particles.
    pub strips: super::strips::StripFrame,
    /// 3D text, drawn after them (collected by `Ctx::collect_text3d`).
    pub texts: super::text3d::TextFrame,
}
impl ParticleFrame {
    pub fn collect(&mut self, world: &mut World, camera: &Camera3D) {
        self.instances.clear();
        self.runs.clear();
        self.strips.collect(world, camera);
        self.right = (camera.rotation * Vec3::X).extend(0.0).to_array();
        self.up = (camera.rotation * Vec3::Y).extend(0.0).to_array();
        world.query::<(&ParticleEmitter3D, &Transform3D)>().for_each(|(e, t)| {
            if e.particles.is_empty() {
                return;
            }
            let first = self.instances.len();
            let s = &e.settings;
            let matrix = t.matrix();
            let mut center = Vec3::ZERO;
            for p in &e.particles {
                let age = p.age / p.lifetime;
                let local = s.space == ParticleSpace3D::Local;
                let position = if local {
                    matrix.transform_point3(p.position)
                } else {
                    p.position
                };
                center += position;
                let size =
                    p.size * s.size_over_life.sample(age) * if local { t.scale.abs().max_element() } else { 1.0 };
                self.instances.push(ParticleInstance {
                    position_size: position.extend(size.max(0.0)).to_array(),
                    color: linear(s.color_over_life.sample(age)),
                    rotation_sheet: [
                        p.rotation,
                        sheet_frame(age, s.sheet) as f32,
                        s.sheet.0 as f32,
                        s.sheet.1 as f32,
                    ],
                });
            }
            if s.blend == ParticleBlend3D::Alpha {
                self.instances[first..].sort_unstable_by(|a, b| {
                    let depth = |p: &ParticleInstance| {
                        (Vec3::new(p.position_size[0], p.position_size[1], p.position_size[2]) - camera.position)
                            .dot(camera.forward())
                    };
                    depth(b).total_cmp(&depth(a))
                });
            }
            center /= e.particles.len() as f32;
            self.runs.push(ParticleRun {
                first: first as u32,
                count: e.particles.len() as u32,
                texture: s.texture.map(|t| t.id),
                blend: s.blend,
                depth: (center - camera.position).dot(camera.forward()),
            });
        });
        self.runs.sort_unstable_by(|a, b| b.depth.total_cmp(&a.depth));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    fn emitter() -> ParticleEmitter3D {
        ParticleEmitter3D::new(ParticleSettings3D {
            rate: 10.0,
            lifetime: (1.0, 1.0),
            ..Default::default()
        })
    }

    #[test]
    fn particles_rate_bursts_and_cap() {
        let mut e = emitter();
        let t = Transform3D::default();
        for _ in 0..5 {
            e.update(0.1, &t);
        }
        assert_eq!(e.live_count(), 5);
        e.burst(3);
        e.update(0.0, &t);
        assert_eq!(e.live_count(), 8);
        e.burst(usize::MAX);
        e.update(0.0, &t);
        assert_eq!(e.live_count(), e.settings.max_particles);
    }

    #[test]
    fn particles_expire_and_reuse_the_pool() {
        let mut e = emitter();
        e.settings.rate = 0.0;
        let capacity = e.particles.capacity();
        e.burst(4);
        e.update(0.0, &Transform3D::default());
        e.update(1.0, &Transform3D::default());
        assert_eq!(e.live_count(), 0);
        e.burst(4);
        e.update(0.0, &Transform3D::default());
        assert_eq!(e.live_count(), 4);
        assert_eq!(e.particles.capacity(), capacity);
    }

    #[test]
    fn particles_curves_interpolate_and_clamp() {
        let c = ParticleCurve::new(vec![(0.0, 0.0), (0.5, 2.0), (1.0, 0.0)]);
        assert_eq!(c.sample(0.25), 1.0);
        assert_eq!(c.sample(-1.0), 0.0);
        assert_eq!(c.sample(2.0), 0.0);
        let g = ParticleGradient::new(vec![(0.0, Color::BLACK), (1.0, Color::WHITE)]);
        assert_eq!(g.sample(0.5).r, 0.5);
    }

    #[test]
    fn particles_seed_repeats() {
        let mut a = emitter();
        let mut b = emitter();
        for _ in 0..9 {
            a.update(0.1, &Transform3D::default());
            b.update(0.1, &Transform3D::default());
        }
        assert_eq!(a.particles, b.particles);
        let mut c = ParticleEmitter3D::new(ParticleSettings3D {
            seed: 91,
            ..a.settings.clone()
        });
        c.update(0.9, &Transform3D::default());
        assert_ne!(a.particles[0].velocity, c.particles[0].velocity);
    }
    #[test]
    fn particles_one_shot_stops_rate_but_accepts_new_bursts() {
        let mut e = ParticleEmitter3D::new(ParticleSettings3D {
            looping: false,
            duration: 0.25,
            rate: 20.0,
            burst: 2,
            lifetime: (10.0, 10.0),
            ..Default::default()
        });
        e.update(0.5, &Transform3D::default());
        assert_eq!(e.live_count(), 7);
        e.update(0.5, &Transform3D::default());
        assert_eq!(e.live_count(), 7);
        e.burst(3);
        e.update(0.0, &Transform3D::default());
        assert_eq!(e.live_count(), 10);
        assert!(e.clone().particles.capacity() >= e.settings.max_particles);
    }

    #[test]
    fn particles_local_follows_transform_world_stays_put_and_runs_sort() {
        let mut world = World::new();
        let mut local = ParticleEmitter3D::new(ParticleSettings3D {
            space: ParticleSpace3D::Local,
            rate: 0.0,
            burst: 1,
            size: (1.0, 1.0),
            ..Default::default()
        });
        let mut global = ParticleEmitter3D::new(ParticleSettings3D {
            rate: 0.0,
            burst: 1,
            ..Default::default()
        });
        let at = Transform3D::at(Vec3::new(0.0, 0.0, -2.0));
        local.update(0.0, &at);
        global.update(0.0, &at);
        world.spawn((
            Transform3D {
                position: Vec3::new(3.0, 0.0, -8.0),
                scale: Vec3::splat(2.0),
                ..Default::default()
            },
            local,
        ));
        world.spawn((Transform3D::at(Vec3::X * 100.0), global));
        let mut camera = Camera3D::new(glam::Vec2::splat(64.0));
        camera.position = Vec3::ZERO;
        let mut frame = ParticleFrame::default();
        frame.collect(&mut world, &camera);
        assert_eq!(frame.runs.len(), 2);
        let far = frame.instances[frame.runs[0].first as usize];
        let near = frame.instances[frame.runs[1].first as usize];
        assert_eq!(far.position_size, [3.0, 0.0, -8.0, 2.0]);
        assert_eq!(&near.position_size[..3], &[0.0, 0.0, -2.0]);
    }

    #[test]
    fn particles_shapes_obey_volume_and_direction() {
        let mut rng = Rng::new(8);
        for _ in 0..100 {
            let (p, d) = spawn_shape(ParticleShape3D::Sphere { radius: 2.0 }, &mut rng);
            assert!(p.length() <= 2.0 && (d.length() - 1.0).abs() < 1e-5);
            let (p, d) = spawn_shape(ParticleShape3D::Cone { angle: 0.3 }, &mut rng);
            assert_eq!(p, Vec3::ZERO);
            assert!(d.y >= 0.3f32.cos() - 1e-6);
            let (p, d) = spawn_shape(
                ParticleShape3D::Box {
                    half_extents: Vec3::new(1.0, 2.0, 3.0),
                },
                &mut rng,
            );
            assert!(p.abs().cmple(Vec3::new(1.0, 2.0, 3.0)).all());
            assert_eq!(d, Vec3::Y);
        }
    }

    #[test]
    fn particles_headless_skips_visual_frame_simulation() {
        let mut world = crate::game::Game::new("particles test", 1, 1).run_headless(2, |ctx| {
            ctx.world.spawn((
                Transform3D::default(),
                ParticleEmitter3D::new(ParticleSettings3D {
                    rate: 0.0,
                    burst: 3,
                    ..Default::default()
                }),
            ));
        });
        let mut count = 0;
        world
            .query::<&ParticleEmitter3D>()
            .for_each(|e| count += e.live_count());
        assert_eq!(count, 0);
    }

    #[test]
    #[should_panic(expected = "particle keys")]
    fn particles_reject_unordered_curve_keys() {
        ParticleCurve::new(vec![(1.0, 1.0), (0.0, 0.0)]);
    }

    #[test]
    fn particles_zero_capacity_and_invalid_dt_are_harmless() {
        let mut e = ParticleEmitter3D::new(ParticleSettings3D {
            max_particles: 0,
            burst: usize::MAX,
            ..Default::default()
        });
        for dt in [0.0, 1.0, -1.0, f32::NAN, f32::INFINITY] {
            e.update(dt, &Transform3D::default());
        }
        assert_eq!(e.live_count(), 0);
    }
    #[test]
    fn particles_sort_by_view_depth_even_off_axis() {
        let mut camera = Camera3D::new(glam::Vec2::splat(64.0));
        camera.position = Vec3::ZERO;
        camera.look_at(Vec3::NEG_Z);
        let mut world = World::new();
        let mut e = ParticleEmitter3D::new(ParticleSettings3D {
            rate: 0.0,
            burst: 2,
            ..Default::default()
        });
        e.update(0.0, &Transform3D::default());
        e.particles[0].position = Vec3::new(4.0, 0.0, -4.0);
        e.particles[1].position = Vec3::new(0.0, 0.0, -5.0);
        let mut first = e.clone();
        first.particles.truncate(1);
        let mut second = e.clone();
        second.particles.remove(0);
        let mut frame = ParticleFrame::default();
        let id = world.spawn((Transform3D::default(), e));
        frame.collect(&mut world, &camera);
        assert_eq!(frame.instances[0].position_size[2], -5.0);
        world.despawn(id);
        world.spawn((Transform3D::default(), first));
        world.spawn((Transform3D::default(), second));
        frame.collect(&mut world, &camera);
        assert_eq!(frame.instances[frame.runs[0].first as usize].position_size[2], -5.0);
    }
}
