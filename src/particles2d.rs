//! 2D particles: the emitter types are public in [`d2`](crate::d2), the
//! shared curves in [`particles`](crate::particles).

use std::ops::{Add, AddAssign, Mul};

use glam::Vec2;
use ponko_ecs::Component;

use crate::color::Color;
use crate::ecs::Transform2D;
use crate::random::Rng;
use crate::sprite_material::{BlendMode, SpriteMaterial};
use crate::texture::Texture;

// ---------------------------------------------------------------- curves

/// A piecewise linear size multiplier over normalized lifetime. Keys must be finite,
/// strictly increasing, and in 0..=1; sampling clamps to the endpoints.
/// ```
/// use ponko::particles::ParticleCurve;
/// let curve = ParticleCurve::new(vec![(0.0, 1.0), (1.0, 0.0)]);
/// assert_eq!(curve.sample(0.5), 0.5);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleCurve(Vec<(f32, f32)>);
impl ParticleCurve {
    /// Creates a curve; panics for empty, unordered or nonfinite keys/values.
    /// ```
    /// use ponko::particles::ParticleCurve;
    /// let c = ParticleCurve::new(vec![(0.0, 2.0)]);
    /// assert_eq!(c.sample(1.0), 2.0);
    /// ```
    pub fn new(keys: Vec<(f32, f32)>) -> Self {
        Self::try_new(keys).unwrap_or_else(|e| panic!("{e}"))
    }
    /// [`new`](Self::new) with the panic as an error.
    pub(crate) fn try_new(keys: Vec<(f32, f32)>) -> Result<Self, &'static str> {
        check_keys(&keys)?;
        if !keys.iter().all(|k| k.1.is_finite()) {
            return Err("particle curve values must be finite");
        }
        Ok(Self(keys))
    }
    /// Evaluates the curve; NaN samples the start.
    /// ```
    /// use ponko::particles::ParticleCurve;
    /// assert_eq!(ParticleCurve::default().sample(0.5), 1.0);
    /// ```
    pub fn sample(&self, t: f32) -> f32 {
        let (a, b, f) = segment(&self.0, t);
        a + (b - a) * f
    }
    /// The keys, `(normalized age, value)`, in order.
    /// ```
    /// use ponko::particles::ParticleCurve;
    /// assert_eq!(ParticleCurve::default().keys(), &[(0.0, 1.0)]);
    /// ```
    #[must_use]
    pub fn keys(&self) -> &[(f32, f32)] {
        &self.0
    }
}
impl Default for ParticleCurve {
    fn default() -> Self {
        Self(vec![(0.0, 1.0)])
    }
}

/// A piecewise linear RGBA gradient, interpolated in the supplied color space.
/// ```
/// use ponko::prelude::*;
/// use ponko::particles::ParticleGradient;
/// let g = ParticleGradient::new(vec![(0.0, Color::WHITE), (1.0, Color::BLACK)]);
/// assert_eq!(g.sample(0.5).r, 0.5);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleGradient(Vec<(f32, Color)>);
impl ParticleGradient {
    /// Creates a gradient; panics for empty, unordered or nonfinite keys/colors.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::particles::ParticleGradient;
    /// let g = ParticleGradient::new(vec![(0.0, Color::WHITE)]);
    /// assert_eq!(g.sample(1.0), Color::WHITE);
    /// ```
    pub fn new(keys: Vec<(f32, Color)>) -> Self {
        Self::try_new(keys).unwrap_or_else(|e| panic!("{e}"))
    }
    /// [`new`](Self::new) with the panic as an error.
    pub(crate) fn try_new(keys: Vec<(f32, Color)>) -> Result<Self, &'static str> {
        check_keys(&keys)?;
        if !keys
            .iter()
            .all(|k| [k.1.r, k.1.g, k.1.b, k.1.a].iter().all(|v| v.is_finite()))
        {
            return Err("particle colors must be finite");
        }
        Ok(Self(keys))
    }
    /// Evaluates RGBA at normalized age, clamping beyond the keys.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::particles::ParticleGradient;
    /// assert_eq!(ParticleGradient::default().sample(0.0), Color::WHITE);
    /// ```
    pub fn sample(&self, t: f32) -> Color {
        let (a, b, f) = segment(&self.0, t);
        Color::rgba(
            a.r + (b.r - a.r) * f,
            a.g + (b.g - a.g) * f,
            a.b + (b.b - a.b) * f,
            a.a + (b.a - a.a) * f,
        )
    }
    /// The keys, `(normalized age, color)`, in order.
    /// ```
    /// use ponko::particles::ParticleGradient;
    /// assert_eq!(ParticleGradient::default().keys().len(), 2);
    /// ```
    #[must_use]
    pub fn keys(&self) -> &[(f32, Color)] {
        &self.0
    }
}
impl Default for ParticleGradient {
    fn default() -> Self {
        Self(vec![(0.0, Color::WHITE), (1.0, Color::rgba(1.0, 1.0, 1.0, 0.0))])
    }
}
fn check_keys<T>(keys: &[(f32, T)]) -> Result<(), &'static str> {
    if !keys.is_empty()
        && keys.iter().all(|k| k.0.is_finite() && (0.0..=1.0).contains(&k.0))
        && keys.windows(2).all(|w| w[0].0 < w[1].0)
    {
        Ok(())
    } else {
        Err("particle keys must be nonempty, ordered and in 0..=1")
    }
}
fn segment<T: Copy>(keys: &[(f32, T)], t: f32) -> (T, T, f32) {
    let t = if t.is_nan() { 0.0 } else { t };
    let i = keys.partition_point(|k| k.0 <= t);
    if i == 0 {
        return (keys[0].1, keys[0].1, 0.0);
    }
    if i == keys.len() {
        return (keys[i - 1].1, keys[i - 1].1, 0.0);
    }
    let (a, b) = (keys[i - 1], keys[i]);
    (a.1, b.1, (t - a.0) / (b.0 - a.0))
}

/// Whether existing particles follow the emitter's transform.
/// ```
/// use ponko::particles::ParticleSpace;
/// assert_eq!(ParticleSpace::default(), ParticleSpace::World);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleSpace {
    /// Spawn transform is applied once; gravity is in world coordinates.
    #[default]
    World,
    /// Position and gravity stay in entity coordinates; scale affects particle size.
    Local,
}

// ------------------------------------------------- shared simulation core

/// One living particle, in 2D (`Vec2`) or 3D (`Vec3`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Particle<V> {
    pub position: V,
    pub velocity: V,
    pub age: f32,
    pub lifetime: f32,
    pub size: f32,
    pub rotation: f32,
}

/// Ages and moves every particle by `dt`, then drops the expired ones
/// (keeping the others in order and the pool's storage).
pub(crate) fn advance<V>(particles: &mut Vec<Particle<V>>, dt: f32, gravity: V, drag: f32)
where
    V: Copy + Add<Output = V> + Mul<f32, Output = V> + AddAssign,
{
    let damping = (-drag * dt).exp();
    let pull = gravity * dt;
    for p in particles.iter_mut() {
        p.age += dt;
        p.velocity = (p.velocity + pull) * damping;
        p.position += p.velocity * dt;
    }
    particles.retain(|p| p.age < p.lifetime);
}

/// How many particles an emitter spawns: queued bursts plus the rate's
/// share of time, carried between frames.
#[derive(Debug, Clone, Default)]
pub(crate) struct Emission {
    pending: usize,
    elapsed: f64,
    fraction: f64,
}

impl Emission {
    /// The first frame's burst, capped at the pool size.
    pub fn new(burst: usize, max: usize) -> Self {
        Self {
            pending: burst.min(max),
            ..Self::default()
        }
    }

    /// Queues `count` more for the next frame, capped at the pool size.
    pub fn burst(&mut self, count: usize, max: usize) {
        self.pending = self.pending.saturating_add(count).min(max);
    }

    /// The particles to spawn after `dt` seconds, at most `free`.
    pub fn take(&mut self, dt: f32, rate: f32, duration: f32, looping: bool, free: usize) -> usize {
        let active = if looping {
            f64::from(dt)
        } else {
            f64::from(dt).min((f64::from(duration) - self.elapsed).max(0.0))
        };
        self.elapsed += f64::from(dt);
        self.fraction += active * f64::from(rate);
        let rate_count = self.fraction.floor() as usize;
        self.fraction -= self.fraction.floor();
        let count = self.pending.saturating_add(rate_count).min(free);
        self.pending = 0;
        count
    }
}

/// The settings both emitters reject: unordered or nonfinite ranges
/// (lifetime, speed, size, rotation), a lifetime that isn't positive,
/// negative speed or size, bad rate, duration, drag or gravity, and empty
/// sprite sheets. The emitters panic with the message.
pub(crate) fn validate(
    ranges: [(f32, f32); 4],
    rate: f32,
    duration: f32,
    drag: f32,
    gravity_finite: bool,
    sheet: (u32, u32),
) -> Result<(), &'static str> {
    if !ranges.iter().all(|&(a, b)| a.is_finite() && b.is_finite() && a <= b) {
        return Err("particle ranges must be finite and ordered");
    }
    let [lifetime, speed, size, _] = ranges;
    if !(lifetime.0 > 0.0
        && speed.0 >= 0.0
        && size.0 >= 0.0
        && rate.is_finite()
        && rate >= 0.0
        && duration.is_finite()
        && duration > 0.0
        && drag.is_finite()
        && drag >= 0.0
        && gravity_finite)
    {
        return Err("invalid particle settings");
    }
    if !(sheet.0 > 0 && sheet.1 > 0 && sheet.0.checked_mul(sheet.1).is_some()) {
        return Err("invalid particle sheet");
    }
    Ok(())
}

/// The sprite sheet frame (row-major) for normalized age `age`.
#[inline]
pub(crate) fn sheet_frame(age: f32, sheet: (u32, u32)) -> u32 {
    let frames = sheet.0 * sheet.1;
    ((age * frames as f32).floor().max(0.0) as u32).min(frames - 1)
}

// ------------------------------------------------------------------ 2D

/// Where 2D particles start, in the entity's coordinates (y down, so "up"
/// is toward the top of the screen, `-Y`).
/// ```
/// use ponko::d2::ParticleShape;
/// let shape = ParticleShape::Cone { angle: 0.4 };
/// # let _ = shape;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum ParticleShape2D {
    /// The origin, moving up.
    #[default]
    Point,
    /// Anywhere in a disc, moving outward. A negative radius becomes zero.
    Circle {
        /// Radius in local pixels.
        radius: f32,
    },
    /// The origin, moving in directions up to `angle` away from up.
    Cone {
        /// Half angle in radians, clamped to 0..=pi (pi: every direction).
        angle: f32,
    },
    /// Anywhere in a rectangle, moving up.
    Rect {
        /// Half width and height in local pixels (absolute values are used).
        half_extents: Vec2,
    },
}

/// Settings copied into a [`ParticleEmitter2D`]; change them by making a
/// new emitter. The same vocabulary as the 3D emitter's, in pixels.
/// All numeric inputs must be finite; ranges must be ordered and
/// nonnegative (rotation may be negative). Invalid settings panic in
/// [`ParticleEmitter2D::new`].
/// ```
/// use ponko::d2::{ParticleEmitter, ParticleSettings};
/// use ponko::render::BlendMode;
/// let settings = ParticleSettings::default().with_rate(40.0).with_blend(BlendMode::Additive);
/// let emitter = ParticleEmitter::new(settings);
/// assert_eq!(emitter.live_count(), 0);
/// ```
///
/// New fields may be added in any release: start from [`Default`] and use
/// the `with_*` methods, or set the public fields of a `mut` value.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ParticleSettings2D {
    /// Continuous particles per second. Zero emits only bursts.
    pub rate: f32,
    /// Initial burst, emitted on the first frame (also for non-looping emitters).
    pub burst: usize,
    /// Seconds of continuous emission before stopping or looping.
    pub duration: f32,
    /// Repeat continuous emission after `duration`; the initial burst does not repeat.
    pub looping: bool,
    /// Particle lifetime in seconds, minimum strictly positive.
    pub lifetime: (f32, f32),
    /// Initial speed in pixels per second.
    pub speed: (f32, f32),
    /// Spawn shape in emitter coordinates.
    pub shape: ParticleShape2D,
    /// Acceleration in pixels per second squared, in simulation
    /// coordinates (y down: positive `y` falls). Default `(0, 200)`.
    pub gravity: Vec2,
    /// Exponential velocity damping per second.
    pub drag: f32,
    /// Initial particle width in pixels (before local scale). The height
    /// follows the image's (or sheet frame's) aspect; square without one.
    pub size: (f32, f32),
    /// Initial rotation in radians, clockwise on screen.
    pub rotation: (f32, f32),
    /// Size multiplier over normalized lifetime.
    pub size_over_life: ParticleCurve,
    /// RGBA tint over normalized lifetime.
    pub color_over_life: ParticleGradient,
    /// Optional image; absent draws white squares.
    pub texture: Option<Texture>,
    /// Sprite sheet (columns, rows); frames advance row-major over lifetime.
    pub sheet: (u32, u32),
    /// How particles combine with what's behind them, as for sprites.
    pub blend: BlendMode,
    /// A sprite material (custom shader) for the particles; `None` draws
    /// the built-in look, texture × tint.
    pub material: Option<SpriteMaterial>,
    /// Draw order among sprites, tilemaps and other emitters: higher is on
    /// top, as [`Sprite::z`](crate::d2::Sprite::z).
    pub z: f32,
    /// Hard live-particle limit. Storage is allocated when the emitter is made.
    pub max_particles: usize,
    /// World or entity coordinates.
    pub space: ParticleSpace,
    /// Independent RNG seed; equal seeds and frame steps give equal simulation.
    pub seed: u64,
}

impl Default for ParticleSettings2D {
    fn default() -> Self {
        Self {
            rate: 10.0,
            burst: 0,
            duration: 1.0,
            looping: true,
            lifetime: (1.0, 2.0),
            speed: (50.0, 100.0),
            shape: ParticleShape2D::Point,
            gravity: Vec2::new(0.0, 200.0),
            drag: 0.0,
            size: (4.0, 8.0),
            rotation: (0.0, 0.0),
            size_over_life: ParticleCurve::default(),
            color_over_life: ParticleGradient::default(),
            texture: None,
            sheet: (1, 1),
            blend: BlendMode::Alpha,
            material: None,
            z: 0.0,
            max_particles: 1024,
            space: ParticleSpace::World,
            seed: 1,
        }
    }
}

macro_rules! with_field {
    ($($(#[$doc:meta])* $field:ident: $ty:ty => $name:ident;)*) => {$(
        $(#[$doc])*
        #[must_use]
        pub fn $name(mut self, $field: $ty) -> Self {
            self.$field = $field;
            self
        }
    )*};
}

impl ParticleSettings2D {
    with_field! {
        /// Sets [`rate`](Self::rate), continuous particles per second.
        rate: f32 => with_rate;
        /// Sets [`burst`](Self::burst), the particles emitted on the first frame.
        burst: usize => with_burst;
        /// Sets [`duration`](Self::duration), seconds of continuous emission.
        duration: f32 => with_duration;
        /// Sets [`looping`](Self::looping).
        looping: bool => with_looping;
        /// Sets [`shape`](Self::shape), where particles spawn.
        shape: ParticleShape2D => with_shape;
        /// Sets [`gravity`](Self::gravity).
        gravity: Vec2 => with_gravity;
        /// Sets [`drag`](Self::drag), velocity damping per second.
        drag: f32 => with_drag;
        /// Sets [`size_over_life`](Self::size_over_life).
        size_over_life: ParticleCurve => with_size_over_life;
        /// Sets [`color_over_life`](Self::color_over_life).
        color_over_life: ParticleGradient => with_color_over_life;
        /// Sets [`blend`](Self::blend).
        blend: BlendMode => with_blend;
        /// Sets [`z`](Self::z), the draw order.
        z: f32 => with_z;
        /// Sets [`max_particles`](Self::max_particles), the live-particle limit.
        max_particles: usize => with_max_particles;
        /// Sets [`space`](Self::space).
        space: ParticleSpace => with_space;
        /// Sets [`seed`](Self::seed).
        seed: u64 => with_seed;
    }

    /// Sets [`texture`](Self::texture).
    #[must_use]
    pub fn with_texture(mut self, texture: impl Into<Option<Texture>>) -> Self {
        self.texture = texture.into();
        self
    }

    /// Sets [`material`](Self::material).
    #[must_use]
    pub fn with_material(mut self, material: impl Into<Option<SpriteMaterial>>) -> Self {
        self.material = material.into();
        self
    }

    /// Sets [`lifetime`](Self::lifetime): seconds, from `min` to `max`.
    #[must_use]
    pub fn with_lifetime(mut self, min: f32, max: f32) -> Self {
        self.lifetime = (min, max);
        self
    }

    /// Sets [`speed`](Self::speed): pixels per second, from `min` to `max`.
    #[must_use]
    pub fn with_speed(mut self, min: f32, max: f32) -> Self {
        self.speed = (min, max);
        self
    }

    /// Sets [`size`](Self::size): pixels, from `min` to `max`.
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

/// A 2D particle pool component, paired with a [`d2::Transform`](crate::d2::Transform). The
/// engine advances it after the frame systems (the end of
/// [`Phase::Animate`](crate::game::Phase::Animate),
/// [`Systems::UPDATE_PARTICLES`](crate::game::Systems::UPDATE_PARTICLES))
/// and draws it with the sprites
/// ([`Systems::DRAW_PARTICLES`](crate::game::Systems::DRAW_PARTICLES)):
/// one sprite per particle at the emitter's `z`, so particles sort with
/// sprites and tilemaps by `z`, and an emitter's particles keep their
/// spawn order. `run_headless` skips this visual frame phase (unless
/// [`Game::headless_frames`](crate::game::Game::headless_frames) is on).
/// Cloning copies the live particles and RNG state. Capped emission is
/// discarded.
/// ```
/// use ponko::prelude::*;
/// let mut world = World::new();
/// world.spawn((ponko::d2::Transform::default(), ponko::d2::ParticleEmitter::default()));
/// ```
#[derive(Debug, Component)]
pub struct ParticleEmitter2D {
    pub(crate) settings: ParticleSettings2D,
    pub(crate) particles: Vec<Particle<Vec2>>,
    rng: Rng,
    emission: Emission,
}

impl Clone for ParticleEmitter2D {
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

impl Default for ParticleEmitter2D {
    fn default() -> Self {
        Self::new(ParticleSettings2D::default())
    }
}

impl ParticleEmitter2D {
    /// Allocates the entire particle pool. Panics for invalid settings.
    /// ```
    /// use ponko::d2::{ParticleEmitter, ParticleSettings};
    /// let e = ParticleEmitter::new(ParticleSettings::default().with_max_particles(64));
    /// assert_eq!(e.live_count(), 0);
    /// ```
    pub fn new(settings: ParticleSettings2D) -> Self {
        Self::check(&settings).unwrap_or_else(|e| panic!("{e}"));
        let s = &settings;
        Self {
            particles: Vec::with_capacity(s.max_particles),
            rng: Rng::new(s.seed),
            emission: Emission::new(s.burst, s.max_particles),
            settings,
        }
    }

    /// Why [`new`](Self::new) would panic for `s`, if it would.
    pub(crate) fn check(s: &ParticleSettings2D) -> Result<(), &'static str> {
        validate(
            [s.lifetime, s.speed, s.size, s.rotation],
            s.rate,
            s.duration,
            s.drag,
            s.gravity.is_finite(),
            s.sheet,
        )?;
        let shape_ok = match s.shape {
            ParticleShape2D::Circle { radius } => radius.is_finite(),
            ParticleShape2D::Cone { angle } => angle.is_finite(),
            ParticleShape2D::Rect { half_extents } => half_extents.is_finite(),
            ParticleShape2D::Point => true,
        };
        if !shape_ok {
            return Err("invalid particle shape");
        }
        if !s.z.is_finite() {
            return Err("particle z must be finite");
        }
        Ok(())
    }

    /// Queues particles for the next frame, capped at the pool size; works after a one-shot ends.
    /// ```
    /// let mut e = ponko::d2::ParticleEmitter::default();
    /// e.burst(20);
    /// ```
    pub fn burst(&mut self, count: usize) {
        self.emission.burst(count, self.settings.max_particles);
    }

    /// Number of currently living particles.
    /// ```
    /// assert_eq!(ponko::d2::ParticleEmitter::default().live_count(), 0);
    /// ```
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.particles.len()
    }

    /// The settings the emitter was made with.
    /// ```
    /// let e = ponko::d2::ParticleEmitter::default();
    /// assert_eq!(e.settings().max_particles, 1024);
    /// ```
    #[must_use]
    pub fn settings(&self) -> &ParticleSettings2D {
        &self.settings
    }

    /// Advances the pool by `dt` seconds: existing particles age and move,
    /// expired ones go, then this frame's rate and bursts spawn at
    /// `transform`. The engine calls it every frame; call it yourself to
    /// step an emitter by hand (tests, previews, a removed
    /// `UPDATE_PARTICLES`). A negative or nonfinite `dt` does nothing.
    /// ```
    /// use ponko::d2::{ParticleEmitter, ParticleSettings, Transform};
    /// let mut e = ParticleEmitter::new(ParticleSettings::default().with_rate(0.0).with_burst(5));
    /// e.update(0.0, &Transform::default());
    /// assert_eq!(e.live_count(), 5);
    /// ```
    pub fn update(&mut self, dt: f32, transform: &Transform2D) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        let s = &self.settings;
        advance(&mut self.particles, dt, s.gravity, s.drag);
        let free = s.max_particles - self.particles.len();
        let count = self.emission.take(dt, s.rate, s.duration, s.looping, free);
        let turn = Vec2::from_angle(transform.rotation);
        for _ in 0..count {
            let (position, direction) = spawn_shape2d(s.shape, &mut self.rng);
            let velocity = direction * self.rng.range(s.speed.0, s.speed.1);
            let (position, velocity) = match s.space {
                ParticleSpace::World => (
                    transform.position + turn.rotate(position * transform.scale),
                    turn.rotate(velocity),
                ),
                ParticleSpace::Local => (position, velocity),
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

    /// The texture drawn and the batch style key (`sprite_material`).
    pub(crate) fn look(&self, white: Texture) -> (Texture, u32) {
        let s = &self.settings;
        (
            s.texture.unwrap_or(white),
            crate::sprite_material::style_key(s.blend, s.material),
        )
    }

    /// Calls `out` with each living particle as a sprite, in spawn order.
    pub(crate) fn for_each_sprite(
        &self,
        transform: &Transform2D,
        mut out: impl FnMut(crate::render::batch::SpriteInstance),
    ) {
        use crate::render::batch::{SpriteInstance, uv_region};
        let s = &self.settings;
        let (columns, rows) = s.sheet;
        let frame_uv = [1.0 / columns as f32, 1.0 / rows as f32];
        // Height over width of one frame; square without an image.
        let aspect = match s.texture {
            Some(t) => {
                let frame = t.size() * Vec2::new(frame_uv[0], frame_uv[1]);
                if frame.x > 0.0 { frame.y / frame.x } else { 1.0 }
            }
            None => 1.0,
        };
        let local = s.space == ParticleSpace::Local;
        let turn = Vec2::from_angle(transform.rotation);
        let scale = if local {
            transform.scale.abs().max_element()
        } else {
            1.0
        };
        let spin = if local { transform.rotation } else { 0.0 };
        let animated = columns * rows > 1;
        for p in &self.particles {
            let age = p.age / p.lifetime;
            let position = if local {
                transform.position + turn.rotate(p.position * transform.scale)
            } else {
                p.position
            };
            let width = (p.size * s.size_over_life.sample(age) * scale).max(0.0);
            let mut sprite = SpriteInstance::new(
                position.into(),
                [width, width * aspect],
                [0.5, 0.5],
                p.rotation + spin,
                s.color_over_life.sample(age).to_rgba8(),
            );
            if animated {
                let frame = sheet_frame(age, s.sheet);
                let min = [
                    (frame % columns) as f32 * frame_uv[0],
                    (frame / columns) as f32 * frame_uv[1],
                ];
                sprite = sprite.with_uv(uv_region(min, frame_uv));
            }
            out(sprite);
        }
    }
}

/// A start position and unit direction for `shape`.
fn spawn_shape2d(shape: ParticleShape2D, rng: &mut Rng) -> (Vec2, Vec2) {
    const UP: Vec2 = Vec2::NEG_Y;
    match shape {
        ParticleShape2D::Point => (Vec2::ZERO, UP),
        ParticleShape2D::Circle { radius } => {
            let d = Vec2::from_angle(rng.angle());
            (d * radius.max(0.0) * rng.f32().sqrt(), d)
        }
        ParticleShape2D::Cone { angle } => {
            let a = angle.clamp(0.0, std::f32::consts::PI);
            (Vec2::ZERO, Vec2::from_angle(rng.range(-a, a)).rotate(UP))
        }
        ParticleShape2D::Rect { half_extents } => (
            Vec2::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)) * half_extents.abs(),
            UP,
        ),
    }
}

impl crate::ctx::Ctx {
    /// Advances every 2D emitter by `dt` (on all cores when there are many).
    pub(crate) fn update_particles2d(&mut self, dt: f32) {
        self.world
            .query::<(&mut ParticleEmitter2D, &Transform2D)>()
            .par_for_each(|(emitter, transform)| emitter.update(dt, transform));
    }

    /// Adds every 2D emitter's particles to this frame's sprites.
    pub(crate) fn draw_particles2d(&mut self) {
        let mut emitters = self.world.query::<(&ParticleEmitter2D, &Transform2D)>();
        if emitters.count() == 0 {
            return;
        }
        let white = self.gfx.white_texture();
        let draw_list = &mut self.gfx.draw_list;
        emitters.for_each(|(emitter, transform)| {
            if emitter.particles.is_empty() {
                return;
            }
            let (texture, style) = emitter.look(white);
            let z = emitter.settings.z;
            emitter.for_each_sprite(transform, |sprite| {
                draw_list.push_styled(texture.id, style, z, sprite);
            });
        });
    }

    /// One frame of 2D particles without a GPU, for `ponko_bench`: advances
    /// every emitter by `dt`, draws them as `draw_particles` does, orders
    /// and batches the sprites as the renderer does, and forgets them.
    /// Returns the sprites and batches. Unstable (`bench-internals`).
    #[doc(hidden)]
    #[cfg(feature = "bench-internals")]
    pub fn bench_particle2d_frame(&mut self, dt: f32) -> (usize, usize) {
        self.update_particles2d(dt);
        self.draw_particles2d();
        let (sprites, batches) = self.gfx.draw_list.finish();
        let counts = (sprites.len(), batches.len());
        self.gfx.draw_list.clear();
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitter() -> ParticleEmitter2D {
        ParticleEmitter2D::new(ParticleSettings2D {
            rate: 10.0,
            lifetime: (1.0, 1.0),
            ..Default::default()
        })
    }

    #[test]
    fn emission_counts_rate_bursts_and_one_shot_end() {
        let mut e = Emission::new(2, 100);
        assert_eq!(e.take(0.5, 20.0, 0.25, false, 100), 7);
        assert_eq!(e.take(0.5, 20.0, 0.25, false, 100), 0);
        e.burst(3, 100);
        assert_eq!(e.take(0.0, 20.0, 0.25, false, 100), 3);
        e.burst(usize::MAX, 10);
        assert_eq!(e.take(0.0, 0.0, 1.0, true, 4), 4);
    }

    #[test]
    fn sheet_frames_advance_row_major_and_clamp() {
        assert_eq!(sheet_frame(0.0, (2, 2)), 0);
        assert_eq!(sheet_frame(0.5, (2, 2)), 2);
        assert_eq!(sheet_frame(1.0, (2, 2)), 3);
        assert_eq!(sheet_frame(-1.0, (2, 2)), 0);
    }

    #[test]
    fn rate_bursts_and_cap() {
        let mut e = emitter();
        let t = Transform2D::default();
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
    fn expired_particles_free_the_pool_without_reallocating() {
        let mut e = emitter();
        e.settings.rate = 0.0;
        let capacity = e.particles.capacity();
        e.burst(4);
        e.update(0.0, &Transform2D::default());
        e.update(1.0, &Transform2D::default());
        assert_eq!(e.live_count(), 0);
        e.burst(4);
        e.update(0.0, &Transform2D::default());
        assert_eq!(e.live_count(), 4);
        assert_eq!(e.particles.capacity(), capacity);
    }

    #[test]
    fn equal_seeds_repeat_and_other_seeds_differ() {
        let mut a = emitter();
        let mut b = emitter();
        for _ in 0..9 {
            a.update(0.1, &Transform2D::default());
            b.update(0.1, &Transform2D::default());
        }
        assert_eq!(a.particles, b.particles);
        let mut c = ParticleEmitter2D::new(ParticleSettings2D {
            seed: 91,
            shape: ParticleShape2D::Cone { angle: 1.0 },
            ..a.settings.clone()
        });
        let mut d = ParticleEmitter2D::new(ParticleSettings2D {
            shape: ParticleShape2D::Cone { angle: 1.0 },
            ..a.settings.clone()
        });
        c.update(0.9, &Transform2D::default());
        d.update(0.9, &Transform2D::default());
        assert_ne!(c.particles[0].velocity, d.particles[0].velocity);
    }

    #[test]
    fn gravity_and_drag_move_particles_down_and_slow_them() {
        let mut e = ParticleEmitter2D::new(ParticleSettings2D {
            rate: 0.0,
            burst: 1,
            speed: (0.0, 0.0),
            lifetime: (10.0, 10.0),
            gravity: Vec2::new(0.0, 100.0),
            ..Default::default()
        });
        e.update(0.0, &Transform2D::default());
        e.update(0.5, &Transform2D::default());
        let p = e.particles[0];
        assert_eq!(p.velocity, Vec2::new(0.0, 50.0));
        assert!(p.position.y > 0.0, "positive y falls (y down)");
        let mut dragged = ParticleEmitter2D::new(ParticleSettings2D {
            drag: 2.0,
            ..e.settings.clone()
        });
        dragged.update(0.0, &Transform2D::default());
        dragged.update(0.5, &Transform2D::default());
        assert!(dragged.particles[0].velocity.y < p.velocity.y);
    }

    #[test]
    fn shapes_obey_area_and_direction() {
        let mut rng = Rng::new(8);
        for _ in 0..200 {
            let (p, d) = spawn_shape2d(ParticleShape2D::Circle { radius: 2.0 }, &mut rng);
            assert!(p.length() <= 2.0 + 1e-5 && (d.length() - 1.0).abs() < 1e-5);
            let (p, d) = spawn_shape2d(ParticleShape2D::Cone { angle: 0.3 }, &mut rng);
            assert_eq!(p, Vec2::ZERO);
            assert!(d.dot(Vec2::NEG_Y) >= 0.3f32.cos() - 1e-5, "{d}");
            let (p, d) = spawn_shape2d(
                ParticleShape2D::Rect {
                    half_extents: Vec2::new(1.0, -3.0),
                },
                &mut rng,
            );
            assert!(p.abs().cmple(Vec2::new(1.0, 3.0)).all());
            assert_eq!(d, Vec2::NEG_Y);
        }
        assert_eq!(
            spawn_shape2d(ParticleShape2D::Point, &mut rng),
            (Vec2::ZERO, Vec2::NEG_Y)
        );
    }

    #[test]
    fn world_particles_stay_put_and_local_ones_follow_the_emitter() {
        let settings = ParticleSettings2D {
            rate: 0.0,
            burst: 1,
            speed: (0.0, 0.0),
            gravity: Vec2::ZERO,
            size: (2.0, 2.0),
            ..Default::default()
        };
        let mut world = ParticleEmitter2D::new(settings.clone());
        let mut local = ParticleEmitter2D::new(settings.with_space(ParticleSpace::Local));
        let at = Transform2D::at(Vec2::new(10.0, 20.0));
        world.update(0.0, &at);
        local.update(0.0, &at);
        let moved = Transform2D::at(Vec2::new(100.0, 50.0)).scaled(Vec2::splat(3.0));
        let mut sprites = Vec::new();
        world.for_each_sprite(&moved, |s| sprites.push(s));
        local.for_each_sprite(&moved, |s| sprites.push(s));
        assert_eq!(sprites[0].pos, [10.0, 20.0]);
        assert_eq!(sprites[0].size, [2.0, 2.0]);
        assert_eq!(sprites[1].pos, [100.0, 50.0]);
        assert_eq!(sprites[1].size, [6.0, 6.0], "local particles scale with the emitter");
    }

    #[test]
    fn world_spawns_turn_with_the_emitter() {
        let mut e = ParticleEmitter2D::new(ParticleSettings2D {
            rate: 0.0,
            burst: 1,
            speed: (10.0, 10.0),
            ..Default::default()
        });
        // A quarter turn clockwise: up becomes right.
        e.update(0.0, &Transform2D::default().rotated(std::f32::consts::FRAC_PI_2));
        let v = e.particles[0].velocity;
        assert!((v - Vec2::new(10.0, 0.0)).length() < 1e-4, "{v}");
    }

    #[test]
    fn curves_gradients_and_sheets_shape_the_sprites() {
        let sheet = Texture::new(3, 64, 32);
        let mut e = ParticleEmitter2D::new(ParticleSettings2D {
            rate: 0.0,
            burst: 1,
            lifetime: (1.0, 1.0),
            size: (10.0, 10.0),
            size_over_life: ParticleCurve::new(vec![(0.0, 1.0), (1.0, 0.0)]),
            color_over_life: ParticleGradient::new(vec![(0.0, Color::WHITE), (1.0, Color::BLACK)]),
            texture: Some(sheet),
            sheet: (2, 1),
            ..Default::default()
        });
        e.update(0.0, &Transform2D::default());
        e.update(0.5, &Transform2D::default());
        let mut sprites = Vec::new();
        e.for_each_sprite(&Transform2D::default(), |s| sprites.push(s));
        let s = sprites[0];
        assert_eq!(s.size, [5.0, 5.0], "half size at half life; frames are 32x32");
        assert_eq!(s.color[0], Color::rgb(0.5, 0.5, 0.5).to_rgba8()[0]);
        assert_eq!(
            s.uv,
            crate::render::batch::uv_region([0.5, 0.0], [0.5, 1.0]),
            "second frame"
        );
    }

    #[test]
    fn looks_follow_texture_blend_and_material() {
        let white = Texture::new(0, 1, 1);
        let plain = ParticleEmitter2D::default();
        assert_eq!(plain.look(white), (white, 0));
        let image = Texture::new(5, 8, 8);
        let glow = ParticleEmitter2D::new(
            ParticleSettings2D::default()
                .with_texture(image)
                .with_blend(BlendMode::Additive),
        );
        assert_eq!(glow.look(white), (image, BlendMode::Additive as u32));
    }

    #[test]
    fn zero_capacity_and_invalid_dt_are_harmless() {
        let mut e = ParticleEmitter2D::new(ParticleSettings2D {
            max_particles: 0,
            burst: usize::MAX,
            ..Default::default()
        });
        for dt in [0.0, 1.0, -1.0, f32::NAN, f32::INFINITY] {
            e.update(dt, &Transform2D::default());
        }
        assert_eq!(e.live_count(), 0);
    }

    #[test]
    fn clones_keep_particles_and_capacity() {
        let mut e = emitter();
        e.burst(3);
        e.update(0.0, &Transform2D::default());
        let c = e.clone();
        assert_eq!(c.particles, e.particles);
        assert!(c.particles.capacity() >= e.settings.max_particles);
    }

    #[test]
    fn drawn_particles_sort_with_sprites_by_z_and_batch_by_blend() {
        let mut ctx = crate::ctx::Ctx::new(1.0 / 60.0, Vec2::ZERO);
        ctx.set_headless(true);
        let crab = ctx.gfx.create_texture(2, 2, &[255; 16]);
        ctx.world
            .spawn((Transform2D::default(), crate::d2::Sprite::new(crab).with_z(1.0)));
        let sparks = |z: f32, blend: BlendMode| {
            let mut e = ParticleEmitter2D::new(
                ParticleSettings2D::default()
                    .with_rate(0.0)
                    .with_burst(3)
                    .with_z(z)
                    .with_blend(blend),
            );
            e.update(0.0, &Transform2D::default());
            e
        };
        ctx.world
            .spawn((Transform2D::default(), sparks(2.0, BlendMode::Additive)));
        ctx.world.spawn((Transform2D::default(), sparks(0.0, BlendMode::Alpha)));
        ctx.draw_world_sprites();
        ctx.draw_particles2d();
        let white = ctx.gfx.white_texture();
        let (sprites, batches) = ctx.gfx.draw_list.finish();
        assert_eq!(sprites.len(), 7, "one sprite and six particles");
        let looks: Vec<_> = batches
            .iter()
            .map(|b| (b.texture, b.material, b.end - b.start))
            .collect();
        assert_eq!(
            looks,
            [
                (white.id, 0, 3),
                (crab.id, 0, 1),
                (white.id, BlendMode::Additive as u32, 3)
            ],
            "below the sprite, the sprite, then the additive sparks on top"
        );
    }

    #[test]
    fn headless_frames_advance_and_draw_particles() {
        let mut world = crate::game::Game::new("particles 2d", 64, 64)
            .headless_frames(true)
            .run_headless(3, |ctx| {
                ctx.world.spawn((
                    Transform2D::default(),
                    ParticleEmitter2D::new(ParticleSettings2D::default().with_rate(0.0).with_burst(5)),
                ));
            });
        let mut live = 0;
        world.query::<&ParticleEmitter2D>().for_each(|e| live += e.live_count());
        assert_eq!(live, 5);
    }

    #[test]
    fn removing_update_particles_freezes_emitters() {
        let mut world = crate::game::Game::new("particles 2d", 64, 64)
            .headless_frames(true)
            .remove(crate::game::Systems::UPDATE_PARTICLES)
            .run_headless(3, |ctx| {
                ctx.world.spawn((
                    Transform2D::default(),
                    ParticleEmitter2D::new(ParticleSettings2D::default().with_burst(5)),
                ));
            });
        let mut live = 0;
        world.query::<&ParticleEmitter2D>().for_each(|e| live += e.live_count());
        assert_eq!(live, 0);
    }

    /// Adding a settings field breaks this on purpose: give it to
    /// `PonkoParticleSettings2D` (crates/ponko_ffi/src/particles2d.rs), the
    /// C++ `particle_settings2d` and the reflected JSON too.
    #[test]
    fn the_c_api_and_json_mirror_every_settings_field() {
        let ParticleSettings2D {
            rate: _,
            burst: _,
            duration: _,
            looping: _,
            lifetime: _,
            speed: _,
            shape: _,
            gravity: _,
            drag: _,
            size: _,
            rotation: _,
            size_over_life: _,
            color_over_life: _,
            texture: _,
            sheet: _,
            blend: _,
            material: _,
            z: _,
            max_particles: _,
            space: _,
            seed: _,
        } = ParticleSettings2D::default();
    }

    #[test]
    #[should_panic(expected = "particle ranges must be finite and ordered")]
    fn unordered_ranges_are_rejected() {
        ParticleEmitter2D::new(ParticleSettings2D::default().with_lifetime(2.0, 1.0));
    }

    #[test]
    #[should_panic(expected = "invalid particle sheet")]
    fn empty_sheets_are_rejected() {
        ParticleEmitter2D::new(ParticleSettings2D::default().with_sheet(0, 1));
    }

    #[test]
    #[should_panic(expected = "particle keys")]
    fn unordered_curve_keys_are_rejected() {
        ParticleCurve::new(vec![(1.0, 1.0), (0.0, 0.0)]);
    }
}
