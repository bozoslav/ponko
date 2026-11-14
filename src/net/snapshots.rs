//! Snapshot interpolation for remote entities: buffer timestamped states and
//! show the world a little in the past, so a steady picture can be drawn from
//! packets that arrive unevenly, out of order or not at all. The helpers
//! never touch the wire: you decode your own packets and call `push`.

use glam::{Quat, Vec2, Vec3};

/// A state that can be blended between two snapshots and predicted past the
/// last one. Implemented for `f32`, `Vec2`, `Vec3`, `Quat` and [`Pose`];
/// implement it for your own state to use [`SnapshotBuffer`] with it.
///
/// ```
/// use ponko::net::{Interpolate, SnapshotBuffer};
///
/// #[derive(Clone, Copy, Debug, PartialEq)]
/// struct Health(f32);
/// impl ponko::net::Interpolate for Health {
///     fn interpolate(a: &Self, b: &Self, _span: f32, t: f32) -> Self {
///         Health(a.0 + (b.0 - a.0) * t)
///     }
///     fn extrapolate(_prev: &Self, last: &Self, _span: f32, _ahead: f32) -> Self {
///         *last // don't predict
///     }
/// }
/// let mut buffer = ponko::net::SnapshotBuffer::new();
/// buffer.push(0.0, Health(0.0));
/// buffer.push(1.0, Health(10.0));
/// assert_eq!(buffer.sample_at(0.5).unwrap().value, Health(5.0));
/// ```
pub trait Interpolate: Copy {
    /// The state `t` (0..=1) of the way from `a` to `b`, which were `span`
    /// seconds apart.
    fn interpolate(a: &Self, b: &Self, span: f32, t: f32) -> Self;
    /// The state `ahead` seconds after `last`. `prev` came `span` seconds
    /// before `last` (`span` is 0 when there was no earlier snapshot).
    fn extrapolate(prev: &Self, last: &Self, span: f32, ahead: f32) -> Self;
}

impl Interpolate for f32 {
    fn interpolate(a: &Self, b: &Self, _span: f32, t: f32) -> Self {
        a + (b - a) * t
    }
    fn extrapolate(prev: &Self, last: &Self, span: f32, ahead: f32) -> Self {
        if span > 0.0 {
            last + (last - prev) / span * ahead
        } else {
            *last
        }
    }
}
impl Interpolate for Vec2 {
    fn interpolate(a: &Self, b: &Self, _span: f32, t: f32) -> Self {
        a.lerp(*b, t)
    }
    fn extrapolate(prev: &Self, last: &Self, span: f32, ahead: f32) -> Self {
        if span > 0.0 {
            *last + (*last - *prev) / span * ahead
        } else {
            *last
        }
    }
}
impl Interpolate for Vec3 {
    fn interpolate(a: &Self, b: &Self, _span: f32, t: f32) -> Self {
        a.lerp(*b, t)
    }
    fn extrapolate(prev: &Self, last: &Self, span: f32, ahead: f32) -> Self {
        if span > 0.0 {
            *last + (*last - *prev) / span * ahead
        } else {
            *last
        }
    }
}
impl Interpolate for Quat {
    fn interpolate(a: &Self, b: &Self, _span: f32, t: f32) -> Self {
        a.slerp(*b, t).normalize()
    }
    /// Rotations are held, not predicted.
    fn extrapolate(_prev: &Self, last: &Self, _span: f32, _ahead: f32) -> Self {
        *last
    }
}

/// A position and rotation, optionally with the velocity the owner was moving
/// at. Interpolates position linearly, or along a cubic Hermite curve when
/// both neighbouring snapshots carry a velocity (smoother through curves and
/// at low send rates), and rotation with slerp. Extrapolation moves the
/// position on at `velocity`, or at the speed seen between the last two
/// snapshots when there is none; rotation is held.
///
/// ```
/// use ponko::net::{Interpolate, Pose};
/// use ponko::prelude::*;
/// let a = ponko::net::Pose::at(vec3(0.0, 0.0, 0.0));
/// let b = ponko::net::Pose::at(vec3(10.0, 0.0, 0.0));
/// let mid = ponko::net::Pose::interpolate(&a, &b, 1.0, 0.5);
/// assert_eq!(mid.position, vec3(5.0, 0.0, 0.0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Where, in world units.
    pub position: Vec3,
    /// Which way it faces.
    pub rotation: Quat,
    /// Units per second, if the sender knows it.
    pub velocity: Option<Vec3>,
}
impl Default for Pose {
    fn default() -> Self {
        Self::at(Vec3::ZERO)
    }
}
impl Pose {
    /// At `position`, not rotated, without a velocity.
    /// ```
    /// let p = ponko::net::Pose::at(ponko::prelude::vec3(1.0, 2.0, 3.0));
    /// assert!(p.velocity.is_none());
    /// ```
    pub const fn at(position: Vec3) -> Self {
        Self {
            position,
            rotation: Quat::IDENTITY,
            velocity: None,
        }
    }

    /// The same pose facing `rotation`.
    /// ```
    /// use ponko::prelude::*;
    /// let p = ponko::net::Pose::at(Vec3::ZERO).facing(Quat::from_rotation_y(1.0));
    /// assert_ne!(p.rotation, Quat::IDENTITY);
    /// ```
    #[must_use]
    pub const fn facing(self, rotation: Quat) -> Self {
        Self { rotation, ..self }
    }

    /// The same pose moving at `velocity`.
    /// ```
    /// use ponko::prelude::*;
    /// let p = ponko::net::Pose::at(Vec3::ZERO).moving(vec3(0.0, 0.0, 5.0));
    /// assert_eq!(p.velocity, Some(vec3(0.0, 0.0, 5.0)));
    /// ```
    #[must_use]
    pub const fn moving(self, velocity: Vec3) -> Self {
        Self {
            velocity: Some(velocity),
            ..self
        }
    }
}
impl Interpolate for Pose {
    fn interpolate(a: &Self, b: &Self, span: f32, t: f32) -> Self {
        let position = match (a.velocity, b.velocity) {
            (Some(va), Some(vb)) if span > 0.0 => {
                // Cubic Hermite: tangents are velocity × the time between.
                let (t2, t3) = (t * t, t * t * t);
                let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                let h10 = t3 - 2.0 * t2 + t;
                let h01 = -2.0 * t3 + 3.0 * t2;
                let h11 = t3 - t2;
                a.position * h00 + va * (span * h10) + b.position * h01 + vb * (span * h11)
            }
            _ => a.position.lerp(b.position, t),
        };
        Self {
            position,
            rotation: a.rotation.slerp(b.rotation, t).normalize(),
            velocity: match (a.velocity, b.velocity) {
                (Some(va), Some(vb)) => Some(va.lerp(vb, t)),
                _ => None,
            },
        }
    }

    fn extrapolate(prev: &Self, last: &Self, span: f32, ahead: f32) -> Self {
        let velocity = last.velocity.or_else(|| {
            // No velocity from the sender: use the speed between snapshots.
            (span > 0.0).then(|| (last.position - prev.position) / span)
        });
        Self {
            position: last.position + velocity.unwrap_or(Vec3::ZERO) * ahead,
            ..*last
        }
    }
}

/// How a [`SnapshotSample`] was produced.
/// ```
/// assert_ne!(ponko::net::SnapshotState::Interpolated, ponko::net::SnapshotState::Held);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotState {
    /// Between two snapshots: the normal case.
    Interpolated,
    /// Past the newest snapshot (packets are late), predicted from it for at
    /// most [`SnapshotConfig::max_extrapolation`].
    Extrapolated,
    /// Standing still: before the oldest snapshot, or past the extrapolation
    /// limit (at where the prediction ended).
    Held,
}

/// A state read from a [`SnapshotBuffer`] or [`SnapshotStream`].
/// ```
/// use ponko::prelude::*;
/// let mut b = ponko::net::SnapshotBuffer::new();
/// b.push(1.0, 5.0f32);
/// let s = b.sample_at(0.0).unwrap();
/// assert_eq!((s.value, s.state), (5.0, ponko::net::SnapshotState::Held));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotSample<T> {
    /// The state to show.
    pub value: T,
    /// How it was made.
    pub state: SnapshotState,
}

/// Tuning for [`SnapshotBuffer`] and [`SnapshotStream`].
/// ```
/// let c = ponko::net::SnapshotConfig::default();
/// assert_eq!((c.delay, c.max_extrapolation, c.capacity), (0.1, 0.25, 64));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotConfig {
    /// How far in the past remote entities are shown, in seconds. It must
    /// cover the gap between packets plus their jitter, or the buffer runs
    /// dry and entities are extrapolated. Default 0.1.
    pub delay: f64,
    /// How long past the newest snapshot to predict before holding still, in
    /// seconds. Default 0.25.
    pub max_extrapolation: f64,
    /// Snapshots kept (the oldest is dropped first). Make it at least twice
    /// `delay` × the send rate. Default 64.
    pub capacity: usize,
}
impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            delay: 0.1,
            max_extrapolation: 0.25,
            capacity: 64,
        }
    }
}
impl SnapshotConfig {
    /// Checks the settings: `capacity` at least 2, and the times finite and
    /// not negative. The constructors panic with this message.
    /// ```
    /// use ponko::net::SnapshotConfig;
    /// assert!(ponko::net::SnapshotConfig::default().validate().is_ok());
    /// assert!(ponko::net::SnapshotConfig { capacity: 1, ..Default::default() }.validate().is_err());
    /// ```
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.capacity < 2 {
            return Err("SnapshotConfig: capacity must be at least 2");
        }
        if !(self.delay.is_finite() && self.delay >= 0.0) {
            return Err("SnapshotConfig: delay must be a finite number of seconds, 0 or more");
        }
        if !(self.max_extrapolation.is_finite() && self.max_extrapolation >= 0.0) {
            return Err("SnapshotConfig: max_extrapolation must be a finite number of seconds, 0 or more");
        }
        Ok(())
    }
}

/// What happened to a snapshot given to [`SnapshotBuffer::push`].
/// ```
/// assert_ne!(ponko::net::SnapshotPush::Added, ponko::net::SnapshotPush::Duplicate);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotPush {
    /// Stored (in time order, even if it arrived late).
    Added,
    /// A snapshot with this timestamp is already there: ignored.
    Duplicate,
    /// Older than everything kept in a full buffer: ignored.
    TooOld,
    /// The timestamp wasn't a finite number: ignored.
    Invalid,
}

/// Timestamped states of one entity, in time order, read back at any time:
/// interpolated between two snapshots, predicted for a bounded time past the
/// newest, then held. Times are the sender's, in seconds on any timeline you
/// like (for a remote entity use [`SnapshotStream`], which also converts
/// your local clock).
///
/// Snapshots may arrive in any order and more than once.
///
/// ```
/// use ponko::prelude::*;
/// let mut buffer = ponko::net::SnapshotBuffer::new();
/// buffer.push(1.0, vec3(10.0, 0.0, 0.0));
/// buffer.push(0.0, vec3(0.0, 0.0, 0.0)); // arrived late: still placed first
/// assert_eq!(buffer.push(1.0, vec3(99.0, 0.0, 0.0)), ponko::net::SnapshotPush::Duplicate);
///
/// let s = buffer.sample_at(0.25).unwrap();
/// assert_eq!(s.value, vec3(2.5, 0.0, 0.0));
/// assert_eq!(s.state, ponko::net::SnapshotState::Interpolated);
/// // Packets stop: predict for up to 0.25 s, then hold.
/// assert_eq!(buffer.sample_at(1.1).unwrap().state, ponko::net::SnapshotState::Extrapolated);
/// let held = buffer.sample_at(5.0).unwrap();
/// assert_eq!(held.state, ponko::net::SnapshotState::Held);
/// assert_eq!(held.value, vec3(12.5, 0.0, 0.0));
/// ```
#[derive(Clone, Debug)]
pub struct SnapshotBuffer<T: Interpolate> {
    config: SnapshotConfig,
    samples: Vec<(f64, T)>,
}
impl<T: Interpolate> Default for SnapshotBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Interpolate> SnapshotBuffer<T> {
    /// An empty buffer with the default [`SnapshotConfig`].
    /// ```
    /// let b = ponko::net::SnapshotBuffer::<f32>::new();
    /// assert!(b.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::with_config(SnapshotConfig::default())
    }

    /// An empty buffer. Panics if `capacity` is below 2 or a time setting is
    /// negative or not finite.
    /// ```
    /// use ponko::prelude::*;
    /// let b = ponko::net::SnapshotBuffer::<f32>::with_config(ponko::net::SnapshotConfig { delay: 0.2, ..Default::default() });
    /// assert_eq!(b.config().delay, 0.2);
    /// ```
    pub fn with_config(config: SnapshotConfig) -> Self {
        if let Err(message) = config.validate() {
            panic!("{message}");
        }
        Self {
            config,
            samples: Vec::with_capacity(config.capacity),
        }
    }

    /// The settings it was made with.
    /// ```
    /// assert_eq!(ponko::net::SnapshotBuffer::<f32>::new().config().delay, 0.1);
    /// ```
    pub fn config(&self) -> SnapshotConfig {
        self.config
    }

    /// Stores `value`, stamped `time`. Late snapshots go in at their place in
    /// time; one with the same timestamp as a stored snapshot is ignored.
    /// When the buffer is full the oldest snapshot is dropped.
    /// ```
    /// use ponko::prelude::*;
    /// let mut b = ponko::net::SnapshotBuffer::new();
    /// assert_eq!(b.push(1.0, 1.0f32), ponko::net::SnapshotPush::Added);
    /// assert_eq!(b.push(1.0, 1.0f32), ponko::net::SnapshotPush::Duplicate);
    /// assert_eq!(b.push(f64::NAN, 1.0f32), ponko::net::SnapshotPush::Invalid);
    /// ```
    pub fn push(&mut self, time: f64, value: T) -> SnapshotPush {
        if !time.is_finite() {
            return SnapshotPush::Invalid;
        }
        let at = self.samples.partition_point(|s| s.0 < time);
        if self.samples.get(at).is_some_and(|s| s.0 == time) {
            return SnapshotPush::Duplicate;
        }
        if self.samples.len() == self.config.capacity {
            if at == 0 {
                return SnapshotPush::TooOld;
            }
            self.samples.remove(0);
            self.samples.insert(at - 1, (time, value));
        } else {
            self.samples.insert(at, (time, value));
        }
        SnapshotPush::Added
    }

    /// The state at `time` (the sender's timeline): interpolated between the
    /// snapshots around it; past the newest, predicted up to
    /// [`max_extrapolation`](SnapshotConfig::max_extrapolation) and then held
    /// where the prediction ended; before the oldest, the oldest. `None`
    /// while the buffer is empty. Allocation free.
    /// ```
    /// use ponko::prelude::*;
    /// let mut b = ponko::net::SnapshotBuffer::new();
    /// assert!(b.sample_at(0.0).is_none());
    /// b.push(0.0, 0.0f32);
    /// b.push(1.0, 4.0);
    /// assert_eq!(b.sample_at(0.5).unwrap().value, 2.0);
    /// ```
    pub fn sample_at(&self, time: f64) -> Option<SnapshotSample<T>> {
        let first = self.samples.first()?;
        let last = self.samples.last()?;
        if time <= first.0 {
            return Some(SnapshotSample {
                value: first.1,
                state: SnapshotState::Held,
            });
        }
        if time >= last.0 {
            let (prev, span) = match self.samples.len().checked_sub(2).map(|i| &self.samples[i]) {
                Some(p) => (&p.1, (last.0 - p.0) as f32),
                None => (&last.1, 0.0),
            };
            let ahead = time - last.0;
            return Some(if ahead == 0.0 {
                SnapshotSample {
                    value: last.1,
                    state: SnapshotState::Interpolated,
                }
            } else if ahead <= self.config.max_extrapolation {
                SnapshotSample {
                    value: T::extrapolate(prev, &last.1, span, ahead as f32),
                    state: SnapshotState::Extrapolated,
                }
            } else {
                SnapshotSample {
                    value: T::extrapolate(prev, &last.1, span, self.config.max_extrapolation as f32),
                    state: SnapshotState::Held,
                }
            });
        }
        let i = self.samples.partition_point(|s| s.0 <= time) - 1;
        let (a, b) = (&self.samples[i], &self.samples[i + 1]);
        let span = b.0 - a.0;
        Some(SnapshotSample {
            value: T::interpolate(&a.1, &b.1, span as f32, ((time - a.0) / span) as f32),
            state: SnapshotState::Interpolated,
        })
    }

    /// The state to show at `now`, [`delay`](SnapshotConfig::delay) seconds
    /// in the past; for buffers filled with times from the clock `now` is on.
    /// ```
    /// use ponko::prelude::*;
    /// let mut b = ponko::net::SnapshotBuffer::new();
    /// b.push(0.0, 0.0f32);
    /// b.push(1.0, 10.0);
    /// assert_eq!(b.sample(0.6).unwrap().value, 5.0); // 0.6 - 0.1 s delay
    /// ```
    pub fn sample(&self, now: f64) -> Option<SnapshotSample<T>> {
        self.sample_at(now - self.config.delay)
    }

    /// Timestamp of the newest snapshot.
    /// ```
    /// let mut b = ponko::net::SnapshotBuffer::new();
    /// b.push(2.0, 0.0f32);
    /// assert_eq!(b.latest_time(), Some(2.0));
    /// ```
    pub fn latest_time(&self) -> Option<f64> {
        self.samples.last().map(|s| s.0)
    }

    /// Timestamp of the oldest snapshot kept.
    /// ```
    /// let mut b = ponko::net::SnapshotBuffer::new();
    /// b.push(2.0, 0.0f32);
    /// assert_eq!(b.oldest_time(), Some(2.0));
    /// ```
    pub fn oldest_time(&self) -> Option<f64> {
        self.samples.first().map(|s| s.0)
    }

    /// Number of snapshots kept.
    /// ```
    /// assert_eq!(ponko::net::SnapshotBuffer::<f32>::new().len(), 0);
    /// ```
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no snapshot is kept.
    /// ```
    /// assert!(ponko::net::SnapshotBuffer::<f32>::new().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Drops every snapshot. Do this on a teleport, a respawn or a restart
    /// of the sender's clock, so the entity jumps instead of sliding across.
    /// ```
    /// let mut b = ponko::net::SnapshotBuffer::new();
    /// b.push(0.0, 0.0f32);
    /// b.clear();
    /// assert!(b.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Drops snapshots that can no longer be shown at `time` or later: all
    /// but the newest one at or before it.
    /// ```
    /// let mut b = ponko::net::SnapshotBuffer::new();
    /// for i in 0..5 { b.push(i as f64, 0.0f32); }
    /// b.trim_before(3.5);
    /// assert_eq!(b.oldest_time(), Some(3.0));
    /// ```
    pub fn trim_before(&mut self, time: f64) {
        let keep_from = self.samples.partition_point(|s| s.0 <= time).saturating_sub(1);
        self.samples.drain(..keep_from);
    }
}

const CLOCK_WINDOW: usize = 32;
/// Consecutive samples that disagree with the estimate before the clock is
/// considered to have jumped and the estimate starts over.
const CLOCK_JUMP_CONFIRM: u32 = 8;

/// Maps a sender's clock to ours. Every packet carries the sender's time `s`
/// and arrives at our time `r`; `r - s` is the clock offset plus the trip
/// time. The trip time only ever adds, so the estimate follows the lower end
/// of the recent samples (the fastest packets), which makes it robust against
/// jitter and late packets, then smooths that to avoid steps. Needs no
/// round trips, and works one way.
///
/// The result is a stable mapping, not true time: it includes the shortest
/// trip time, which is what interpolation wants anyway.
///
/// ```
/// use ponko::net::ClockSync;
/// let mut clock = ponko::net::ClockSync::new();
/// // The sender's clock runs 1000 s ahead of ours; packets take 30–80 ms.
/// for i in 0..50 {
///     let sent = 1000.0 + i as f64 * 0.05;
///     let trip = 0.03 + (i % 5) as f64 * 0.0125;
///     clock.observe(sent, sent - 1000.0 + trip);
/// }
/// let offset = clock.offset().unwrap();
/// assert!((offset - -1000.0 - 0.03).abs() < 0.005, "{offset}");
/// assert!((clock.to_local(1010.0).unwrap() - 10.03).abs() < 0.005);
/// ```
#[derive(Clone, Debug)]
pub struct ClockSync {
    window: [f64; CLOCK_WINDOW],
    filled: usize,
    next: usize,
    offset: Option<f64>,
    off_streak: u32,
    rise: f64,
    fall: f64,
    jump: f64,
}
impl Default for ClockSync {
    fn default() -> Self {
        Self::new()
    }
}
impl ClockSync {
    /// A clock with no samples yet.
    /// ```
    /// assert!(ponko::net::ClockSync::new().offset().is_none());
    /// ```
    pub fn new() -> Self {
        Self {
            window: [0.0; CLOCK_WINDOW],
            filled: 0,
            next: 0,
            offset: None,
            off_streak: 0,
            rise: 0.05,
            fall: 0.25,
            jump: 1.0,
        }
    }

    /// Records a packet stamped `remote_time` (sender's clock, seconds) that
    /// arrived at `local_now` (our clock, seconds). Call it for every packet,
    /// including late and duplicate ones. Ignores non-finite numbers.
    /// ```
    /// let mut c = ponko::net::ClockSync::new();
    /// c.observe(5.0, 105.0);
    /// assert_eq!(c.offset(), Some(100.0));
    /// ```
    pub fn observe(&mut self, remote_time: f64, local_now: f64) {
        let sample = local_now - remote_time;
        if !sample.is_finite() {
            return;
        }
        if let Some(estimate) = self.offset {
            if (sample - estimate).abs() > self.jump {
                self.off_streak += 1;
                if self.off_streak >= CLOCK_JUMP_CONFIRM {
                    // The sender's clock (or ours) jumped, e.g. a restart.
                    self.filled = 0;
                    self.next = 0;
                    self.offset = None;
                    self.off_streak = 0;
                }
            } else {
                self.off_streak = 0;
            }
        }
        self.window[self.next] = sample;
        self.next = (self.next + 1) % CLOCK_WINDOW;
        self.filled = (self.filled + 1).min(CLOCK_WINDOW);

        // The 4th smallest of 32 (the smallest while there are few): a few
        // freak low samples can't drag the estimate down.
        let mut sorted = self.window;
        let used = &mut sorted[..self.filled];
        used.sort_unstable_by(f64::total_cmp);
        let target = used[(self.filled - 1) / 8];
        self.offset = Some(match self.offset {
            None => target,
            Some(o) => {
                let rate = if target < o { self.fall } else { self.rise };
                o + (target - o) * rate
            }
        });
    }

    /// Local time minus remote time, once at least one packet was observed.
    /// ```
    /// assert!(ponko::net::ClockSync::new().offset().is_none());
    /// ```
    pub fn offset(&self) -> Option<f64> {
        self.offset
    }

    /// The local time of a sender timestamp.
    /// ```
    /// let mut c = ponko::net::ClockSync::new();
    /// c.observe(0.0, 50.0);
    /// assert_eq!(c.to_local(2.0), Some(52.0));
    /// ```
    pub fn to_local(&self, remote_time: f64) -> Option<f64> {
        self.offset.map(|o| remote_time + o)
    }

    /// The sender's time at a local time.
    /// ```
    /// let mut c = ponko::net::ClockSync::new();
    /// c.observe(0.0, 50.0);
    /// assert_eq!(c.to_remote(52.0), Some(2.0));
    /// ```
    pub fn to_remote(&self, local_time: f64) -> Option<f64> {
        self.offset.map(|o| local_time - o)
    }

    /// Packets in the estimate (at most 32).
    /// ```
    /// assert_eq!(ponko::net::ClockSync::new().samples(), 0);
    /// ```
    pub fn samples(&self) -> usize {
        self.filled
    }

    /// Forgets everything, for a new connection.
    /// ```
    /// let mut c = ponko::net::ClockSync::new();
    /// c.observe(0.0, 1.0);
    /// c.reset();
    /// assert!(c.offset().is_none());
    /// ```
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// A [`SnapshotBuffer`] plus a [`ClockSync`]: what you need per remote
/// entity. Push each decoded snapshot with the sender's timestamp and our
/// clock; sample with our clock, once a frame.
///
/// The clock is yours (`Time::elapsed`, an `Instant`, ...) but must be the same
/// for `push` and `sample`. Entities of one sender share its clock, so they
/// all see the same offset estimate; each stream still keeps its own, which
/// is cheap.
///
/// ```
/// use ponko::prelude::*;
/// let mut stream = ponko::net::SnapshotStream::new();
/// // The sender stamps 20 snapshots per second with its own clock (which is
/// // 500 s ahead); each takes 40 ms to arrive.
/// for i in 0..10 {
///     let sent = 500.0 + i as f64 * 0.05;
///     let arrived = i as f64 * 0.05 + 0.04;
///     stream.push(sent, arrived, ponko::net::Pose::at(vec3(i as f32, 0.0, 0.0)));
/// }
/// // Shown 100 ms in the past: x is 6.0 at local time 0.34 + 0.1 - 0.04.
/// let s = stream.sample(0.4 + 0.04).unwrap();
/// assert_eq!(s.state, ponko::net::SnapshotState::Interpolated);
/// assert!((s.value.position.x - 6.0).abs() < 0.01);
/// ```
#[derive(Clone, Debug)]
pub struct SnapshotStream<T: Interpolate> {
    clock: ClockSync,
    buffer: SnapshotBuffer<T>,
}
impl<T: Interpolate> Default for SnapshotStream<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Interpolate> SnapshotStream<T> {
    /// A stream with the default [`SnapshotConfig`].
    /// ```
    /// assert!(ponko::net::SnapshotStream::<f32>::new().sample(0.0).is_none());
    /// ```
    pub fn new() -> Self {
        Self::with_config(SnapshotConfig::default())
    }

    /// A stream with these settings (panics like [`SnapshotBuffer::with_config`]).
    /// ```
    /// use ponko::prelude::*;
    /// let s = ponko::net::SnapshotStream::<f32>::with_config(ponko::net::SnapshotConfig { delay: 0.05, ..Default::default() });
    /// assert_eq!(s.buffer().config().delay, 0.05);
    /// ```
    pub fn with_config(config: SnapshotConfig) -> Self {
        Self {
            clock: ClockSync::new(),
            buffer: SnapshotBuffer::with_config(config),
        }
    }

    /// Stores a snapshot stamped `remote_time` by the sender that arrived at
    /// our time `local_now`, and feeds the pair to the clock estimate.
    /// ```
    /// use ponko::prelude::*;
    /// let mut s = ponko::net::SnapshotStream::new();
    /// assert_eq!(s.push(10.0, 0.5, 1.0f32), ponko::net::SnapshotPush::Added);
    /// assert_eq!(s.push(10.0, 0.6, 1.0f32), ponko::net::SnapshotPush::Duplicate);
    /// ```
    pub fn push(&mut self, remote_time: f64, local_now: f64, value: T) -> SnapshotPush {
        let result = self.buffer.push(remote_time, value);
        if matches!(result, SnapshotPush::Added | SnapshotPush::TooOld) {
            self.clock.observe(remote_time, local_now);
        }
        result
    }

    /// The state to show at our time `now`: the sender's time at
    /// `now - delay`, read from the buffer. `None` until the first snapshot.
    /// ```
    /// use ponko::prelude::*;
    /// let mut s = ponko::net::SnapshotStream::new();
    /// s.push(0.0, 0.0, 0.0f32);
    /// s.push(1.0, 1.0, 10.0);
    /// assert_eq!(s.sample(0.6).unwrap().value, 5.0);
    /// ```
    pub fn sample(&self, now: f64) -> Option<SnapshotSample<T>> {
        let remote = self.clock.to_remote(now - self.buffer.config.delay)?;
        self.buffer.sample_at(remote)
    }

    /// Sets how far in the past to show the entity; for example when
    /// the link gets worse. Takes effect at once (the picture jumps).
    /// Panics if negative or not finite.
    /// ```
    /// let mut s = ponko::net::SnapshotStream::<f32>::new();
    /// s.set_delay(0.2);
    /// assert_eq!(s.buffer().config().delay, 0.2);
    /// ```
    pub fn set_delay(&mut self, delay: f64) {
        assert!(
            delay.is_finite() && delay >= 0.0,
            "SnapshotStream::set_delay: delay must be a finite number of seconds, 0 or more"
        );
        self.buffer.config.delay = delay;
    }

    /// The clock estimate.
    /// ```
    /// assert!(ponko::net::SnapshotStream::<f32>::new().clock().offset().is_none());
    /// ```
    pub fn clock(&self) -> &ClockSync {
        &self.clock
    }

    /// The stored snapshots, in the sender's time.
    /// ```
    /// assert!(ponko::net::SnapshotStream::<f32>::new().buffer().is_empty());
    /// ```
    pub fn buffer(&self) -> &SnapshotBuffer<T> {
        &self.buffer
    }

    /// Our time of the newest snapshot, if any.
    /// ```
    /// let mut s = ponko::net::SnapshotStream::new();
    /// s.push(10.0, 0.5, 1.0f32);
    /// assert_eq!(s.latest_local_time(), Some(0.5));
    /// ```
    pub fn latest_local_time(&self) -> Option<f64> {
        self.clock.to_local(self.buffer.latest_time()?)
    }

    /// Forgets all snapshots, keeping the clock estimate: a teleport or respawn.
    /// ```
    /// let mut s = ponko::net::SnapshotStream::new();
    /// s.push(10.0, 0.5, 1.0f32);
    /// s.clear();
    /// assert!(s.sample(1.0).is_none());
    /// ```
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Forgets snapshots and the clock estimate: a new connection.
    /// ```
    /// let mut s = ponko::net::SnapshotStream::new();
    /// s.push(10.0, 0.5, 1.0f32);
    /// s.reset();
    /// assert!(s.clock().offset().is_none());
    /// ```
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.clock.reset();
    }
}

/// Component: a remote entity's incoming [`Pose`] snapshots, applied to its
/// [`d3::Transform`](crate::d3::Transform) by [`apply_net_poses`]. Push each
/// decoded update from your message handler with `ctx.time.elapsed()` as the
/// arrival time.
///
/// ```
/// use ponko::prelude::*;
/// let world = Game::new("remote", 1, 1)
///     .system(systems::apply_net_poses)
///     .run_headless(60, |ctx| {
///         let mut pose = ponko::net::NetPose::default();
///         pose.stream.push(0.0, 0.0, ponko::net::Pose::at(vec3(0.0, 0.0, 0.0)));
///         pose.stream.push(1.0, 1.0, ponko::net::Pose::at(vec3(100.0, 0.0, 0.0)));
///         ctx.world.spawn((ponko::d3::Transform::default(), pose));
///     });
/// # let _ = world;
/// ```
#[cfg(feature = "3d")]
#[derive(Clone, Debug, Default, ponko_ecs::Component)]
pub struct NetPose {
    /// The buffered snapshots and clock estimate.
    pub stream: SnapshotStream<Pose>,
}

/// System: sets the [`d3::Transform`](crate::d3::Transform) position and rotation
/// of every entity with a [`NetPose`] from its stream, sampled at
/// `ctx.time.elapsed()` (scale is left alone). Entities without snapshots stay
/// where they are. Add it with [`Game::system`](crate::game::Game::system), so entities
/// move every frame whatever the simulation does; the stream's timestamps are matched against
/// `ctx.time.elapsed()`, so push with that clock.
///
/// ```
/// use ponko::prelude::*;
/// let _ = Game::new("remote", 1, 1).system(systems::apply_net_poses);
/// ```
#[cfg(feature = "3d")]
pub fn apply_net_poses(ctx: &mut crate::ctx::Ctx) {
    let now = ctx.time.elapsed();
    ctx.world
        .query::<(&NetPose, &mut crate::d3::Transform)>()
        .for_each(|(pose, transform)| {
            if let Some(sample) = pose.stream.sample(now) {
                transform.position = sample.value.position;
                transform.rotation = sample.value.rotation;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    fn x(sample: Option<SnapshotSample<Vec3>>) -> f32 {
        sample.unwrap().value.x
    }

    /// A sender walking along x at 10 units/s, one snapshot every 50 ms.
    fn walk(t: f64) -> Vec3 {
        vec3(t as f32 * 10.0, 0.0, 0.0)
    }

    #[test]
    fn interpolates_linearly_between_snapshots() {
        let mut b = SnapshotBuffer::new();
        b.push(0.0, walk(0.0));
        b.push(0.1, walk(0.1));
        let s = b.sample_at(0.025).unwrap();
        assert!((s.value.x - 0.25).abs() < 1e-5);
        assert_eq!(s.state, SnapshotState::Interpolated);
    }

    #[test]
    fn hermite_follows_a_curve_better_than_a_straight_line() {
        // Quarter circle of radius 10 in two snapshots: the middle of the
        // curve is at (7.07, 7.07); a straight line gives (5, 5).
        let a = Pose::at(vec3(10.0, 0.0, 0.0)).moving(vec3(0.0, 10.0, 0.0));
        let b = Pose::at(vec3(0.0, 10.0, 0.0)).moving(vec3(-10.0, 0.0, 0.0));
        let span = std::f32::consts::FRAC_PI_2; // speed 10 over an arc of 5π
        let hermite = Pose::interpolate(&a, &b, span * 1.0, 0.5).position;
        let linear = Pose::interpolate(&Pose::at(a.position), &Pose::at(b.position), span, 0.5).position;
        let truth = vec3(7.071, 7.071, 0.0);
        assert!((hermite - truth).length() < (linear - truth).length() * 0.5);
        // Endpoints are exact.
        assert!((Pose::interpolate(&a, &b, span, 0.0).position - a.position).length() < 1e-5);
        assert!((Pose::interpolate(&a, &b, span, 1.0).position - b.position).length() < 1e-5);
    }

    #[test]
    fn hermite_needs_both_velocities() {
        let a = Pose::at(vec3(0.0, 0.0, 0.0)).moving(vec3(100.0, 0.0, 0.0));
        let b = Pose::at(vec3(10.0, 0.0, 0.0));
        assert_eq!(Pose::interpolate(&a, &b, 1.0, 0.5).position, vec3(5.0, 0.0, 0.0));
        assert_eq!(Pose::interpolate(&a, &b, 1.0, 0.5).velocity, None);
    }

    #[test]
    fn rotation_takes_the_short_way_round() {
        let a = Pose::at(Vec3::ZERO).facing(Quat::from_rotation_y(0.1));
        let b = Pose::at(Vec3::ZERO).facing(Quat::from_rotation_y(std::f32::consts::TAU - 0.1));
        let mid = Pose::interpolate(&a, &b, 1.0, 0.5).rotation;
        // Halfway between +0.1 and -0.1 rad is 0, not π.
        assert!(mid.angle_between(Quat::IDENTITY) < 1e-3);
        assert!((mid.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn out_of_order_and_duplicate_snapshots_end_up_sorted() {
        let mut b = SnapshotBuffer::new();
        let order = [3, 1, 4, 0, 2, 3, 1];
        let results: Vec<_> = order
            .iter()
            .map(|&i| b.push(i as f64 * 0.1, walk(i as f64 * 0.1)))
            .collect();
        assert_eq!(
            results,
            [
                SnapshotPush::Added,
                SnapshotPush::Added,
                SnapshotPush::Added,
                SnapshotPush::Added,
                SnapshotPush::Added,
                SnapshotPush::Duplicate,
                SnapshotPush::Duplicate,
            ]
        );
        assert_eq!(b.len(), 5);
        for i in 0..40 {
            let t = i as f64 * 0.01;
            assert!((x(b.sample_at(t)) - walk(t).x).abs() < 1e-4, "t={t}");
        }
    }

    #[test]
    fn a_duplicate_does_not_overwrite_the_first_copy() {
        let mut b = SnapshotBuffer::new();
        b.push(1.0, 1.0f32);
        b.push(1.0, 99.0);
        assert_eq!(b.sample_at(1.0).unwrap().value, 1.0);
    }

    #[test]
    fn extrapolates_for_a_bounded_time_then_holds_where_it_ended() {
        let mut b = SnapshotBuffer::new();
        b.push(0.0, walk(0.0));
        b.push(0.1, walk(0.1));
        let s = b.sample_at(0.2).unwrap(); // 0.1 s past the last snapshot
        assert_eq!(s.state, SnapshotState::Extrapolated);
        assert!((s.value.x - 2.0).abs() < 1e-4, "{}", s.value.x);
        // Just inside the cap, still extrapolating.
        let cap = b.sample_at(0.349).unwrap();
        assert_eq!(cap.state, SnapshotState::Extrapolated);
        assert!((cap.value.x - 3.49).abs() < 1e-4);
        // Beyond it: held at the end of the prediction, never flying off.
        for t in [0.36, 1.0, 100.0] {
            let h = b.sample_at(t).unwrap();
            assert_eq!(h.state, SnapshotState::Held, "t={t}");
            assert!((h.value.x - 3.5).abs() < 1e-4, "t={t}");
        }
    }

    #[test]
    fn velocity_drives_extrapolation_when_given() {
        let mut b = SnapshotBuffer::new();
        b.push(0.0, Pose::at(vec3(1.0, 0.0, 0.0)).moving(vec3(0.0, 4.0, 0.0)));
        let s = b.sample_at(0.1).unwrap();
        assert_eq!(s.state, SnapshotState::Extrapolated);
        assert!((s.value.position - vec3(1.0, 0.4, 0.0)).length() < 1e-5);
        // A single snapshot without velocity stays put.
        let mut b = SnapshotBuffer::new();
        b.push(0.0, Pose::at(vec3(1.0, 0.0, 0.0)));
        assert_eq!(b.sample_at(0.2).unwrap().value.position, vec3(1.0, 0.0, 0.0));
    }

    #[test]
    fn zero_extrapolation_holds_at_the_last_snapshot() {
        let mut b = SnapshotBuffer::with_config(SnapshotConfig {
            max_extrapolation: 0.0,
            ..Default::default()
        });
        b.push(0.0, walk(0.0));
        b.push(0.1, walk(0.1));
        let s = b.sample_at(0.15).unwrap();
        assert_eq!(s.state, SnapshotState::Held);
        assert!((s.value.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn before_the_first_snapshot_the_first_is_held() {
        let mut b = SnapshotBuffer::new();
        b.push(5.0, walk(5.0));
        b.push(5.1, walk(5.1));
        let s = b.sample_at(1.0).unwrap();
        assert_eq!((s.value, s.state), (walk(5.0), SnapshotState::Held));
    }

    #[test]
    fn full_buffer_drops_the_oldest_and_refuses_older_ones() {
        let mut b = SnapshotBuffer::with_config(SnapshotConfig {
            capacity: 4,
            ..Default::default()
        });
        for i in 1..=4 {
            b.push(i as f64, i as f32);
        }
        assert_eq!(b.push(0.5, 0.0), SnapshotPush::TooOld);
        assert_eq!(b.push(5.0, 5.0), SnapshotPush::Added);
        assert_eq!(b.oldest_time(), Some(2.0));
        // A late one that fits in the middle pushes the oldest out.
        assert_eq!(b.push(2.5, 2.5), SnapshotPush::Added);
        assert_eq!((b.oldest_time(), b.len()), (Some(2.5), 4));
        assert_eq!(b.sample_at(3.5).unwrap().value, 3.5);
    }

    #[test]
    fn rejects_non_finite_times() {
        let mut b = SnapshotBuffer::new();
        assert_eq!(b.push(f64::INFINITY, 0.0f32), SnapshotPush::Invalid);
        assert_eq!(b.push(f64::NAN, 0.0f32), SnapshotPush::Invalid);
        assert!(b.is_empty());
    }

    #[test]
    fn trim_keeps_one_snapshot_before_the_time() {
        let mut b = SnapshotBuffer::new();
        for i in 0..10 {
            b.push(i as f64, i as f32);
        }
        b.trim_before(4.5);
        assert_eq!(b.oldest_time(), Some(4.0));
        assert_eq!(b.sample_at(4.5).unwrap().value, 4.5);
        b.trim_before(-3.0);
        assert_eq!(b.len(), 6);
        b.trim_before(100.0);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn clock_sync_ignores_jitter_and_late_packets() {
        let mut clock = ClockSync::new();
        // Deterministic jitter: trip times 20–120 ms, with an occasional
        // packet delayed by a whole second.
        let mut worst: f64 = 0.0;
        for i in 0..400u32 {
            let sent = 7.0 + i as f64 * 0.02;
            let mut trip = 0.02 + ((i * 37) % 11) as f64 * 0.01;
            if i % 53 == 0 {
                trip += 1.0;
            }
            clock.observe(sent, sent + 100.0 + trip);
            if i > 60 {
                worst = worst.max((clock.offset().unwrap() - 100.02).abs());
            }
        }
        assert!(worst < 0.025, "estimate strayed {worst} s from the fastest trip");
    }

    #[test]
    fn clock_sync_is_not_dragged_by_one_freakishly_fast_packet() {
        let mut clock = ClockSync::new();
        for i in 0..40 {
            clock.observe(i as f64, i as f64 + 0.05);
        }
        clock.observe(40.0, 40.0 + 0.005); // 45 ms "faster" than ever
        assert!((clock.offset().unwrap() - 0.05).abs() < 1e-9);
    }

    #[test]
    fn clock_sync_follows_slow_drift() {
        let mut clock = ClockSync::new();
        // The sender's clock runs 100 ppm fast: the offset shrinks 0.1 ms/s.
        for i in 0..6000 {
            let sent = i as f64 * 0.05;
            let local = sent * (1.0 - 1e-4) + 0.04 + (i % 4) as f64 * 0.01;
            clock.observe(sent, local);
        }
        let truth = 0.04 - 299.95e-4; // at the last sample
        assert!((clock.offset().unwrap() - truth).abs() < 0.004, "{:?}", clock.offset());
    }

    #[test]
    fn clock_sync_restarts_after_a_clock_jump() {
        let mut clock = ClockSync::new();
        for i in 0..50 {
            clock.observe(i as f64, i as f64 + 0.05);
        }
        // The sender restarts: its clock now starts from zero again, 5000 s lower.
        for i in 0..40 {
            clock.observe(i as f64, 5000.0 + i as f64 + 0.05);
        }
        assert!((clock.offset().unwrap() - 5000.05).abs() < 0.01, "{:?}", clock.offset());
        // A single late packet isn't a jump.
        let before = clock.offset().unwrap();
        clock.observe(41.0, 5041.0 + 20.0);
        assert!((clock.offset().unwrap() - before).abs() < 1e-6);
    }

    #[test]
    fn clock_sync_converts_both_ways() {
        let mut clock = ClockSync::new();
        assert_eq!(clock.to_local(1.0), None);
        clock.observe(10.0, 3.0);
        assert_eq!(clock.to_local(12.0), Some(5.0));
        assert_eq!(clock.to_remote(5.0), Some(12.0));
        clock.observe(f64::NAN, 1.0);
        assert_eq!(clock.samples(), 1);
    }

    /// Feeds a stream with a walking sender through a lossy, jittery link
    /// and checks how far the picture is from the truth 100 ms ago.
    fn run_link(loss_every: u32, jitter_ms: u32, reorder: bool) -> (f32, usize, usize) {
        let mut stream = SnapshotStream::new();
        let mut inflight: Vec<(f64, f64, Vec3)> = Vec::new(); // (arrival, sent, value)
        let (mut worst, mut extrapolated, mut held) = (0.0f32, 0, 0);
        for frame in 0..600u32 {
            let now = frame as f64 / 60.0;
            if frame % 3 == 0 {
                // 20 snapshots a second, sender clock 900 s ahead.
                let sent = 900.0 + now;
                let n = frame / 3;
                if loss_every == 0 || n % loss_every != 0 {
                    let jitter = ((n * 7919) % (jitter_ms + 1)) as f64 / 1000.0;
                    inflight.push((now + 0.03 + jitter, sent, walk(now)));
                    if reorder && n % 5 == 0 {
                        inflight.push((now + 0.03 + jitter + 0.02, sent, walk(now))); // duplicate
                    }
                }
            }
            inflight.sort_by(|a, b| a.0.total_cmp(&b.0));
            while inflight.first().is_some_and(|p| p.0 <= now) {
                let (_, sent, value) = inflight.remove(0);
                stream.push(sent, now, value);
            }
            if now > 1.0 {
                let s = stream.sample(now).unwrap();
                // Truth: where the sender was `delay` + the minimum trip ago.
                let truth = walk(now - 0.1 - 0.03).x;
                worst = worst.max((s.value.x - truth).abs());
                match s.state {
                    SnapshotState::Extrapolated => extrapolated += 1,
                    SnapshotState::Held => held += 1,
                    SnapshotState::Interpolated => {}
                }
            }
        }
        (worst, extrapolated, held)
    }

    #[test]
    fn stream_is_smooth_on_a_clean_link() {
        let (worst, extrapolated, held) = run_link(0, 0, false);
        // Packets are only seen at frame boundaries (1/60 s, 0.17 units of
        // walking), which the clock estimate takes as trip time.
        assert!(worst < 0.1, "{worst}");
        assert_eq!((extrapolated, held), (0, 0));
    }

    #[test]
    fn stream_hides_jitter_within_the_delay() {
        // Up to 60 ms jitter at 50 ms spacing: the 100 ms delay covers it,
        // although the packets arrive out of order and doubled.
        let (worst, extrapolated, held) = run_link(0, 60, true);
        assert!(worst < 0.5, "{worst}");
        assert_eq!(held, 0);
        assert!(extrapolated < 60, "{extrapolated}");
    }

    #[test]
    fn stream_rides_out_packet_loss_on_a_straight_walk() {
        // Every fourth snapshot lost: interpolation bridges single gaps.
        let (worst, extrapolated, held) = run_link(4, 0, false);
        assert!(worst < 0.05, "{worst}");
        assert_eq!((extrapolated, held), (0, 0));
    }

    #[test]
    fn stream_extrapolates_then_holds_through_a_blackout() {
        let mut stream = SnapshotStream::new();
        for i in 0..20 {
            let t = i as f64 * 0.05;
            stream.push(t, t, walk(t));
        }
        // The last snapshot was at 0.95; nothing arrives after.
        let states: Vec<_> = [1.0, 1.1, 1.29, 1.5, 3.0]
            .iter()
            .map(|&now| stream.sample(now).unwrap())
            .collect();
        assert_eq!(states[0].state, SnapshotState::Interpolated); // now - 0.1 = 0.9
        assert_eq!(states[1].state, SnapshotState::Extrapolated); // 1.0, 0.05 past
        assert_eq!(states[2].state, SnapshotState::Extrapolated); // 1.2, 0.25 past
        assert_eq!(states[3].state, SnapshotState::Held);
        assert_eq!(states[4].state, SnapshotState::Held);
        assert!((states[4].value.x - walk(0.95 + 0.25).x).abs() < 1e-3);
        assert!(states[1].value.x > states[0].value.x);
    }

    #[test]
    fn stream_is_empty_until_the_first_snapshot_and_after_clear() {
        let mut s = SnapshotStream::<Vec3>::new();
        assert!(s.sample(1.0).is_none());
        s.push(0.0, 0.0, Vec3::ONE);
        assert!(s.sample(1.0).is_some());
        s.clear();
        assert!(s.sample(1.0).is_none());
        s.push(0.0, 0.0, Vec3::ONE);
        s.reset();
        assert_eq!(s.clock().samples(), 0);
    }

    #[test]
    fn late_duplicate_does_not_disturb_the_clock() {
        let mut s = SnapshotStream::new();
        s.push(10.0, 0.05, 0.0f32);
        let before = s.clock().offset();
        assert_eq!(s.push(10.0, 3.0, 0.0f32), SnapshotPush::Duplicate);
        assert_eq!(s.clock().offset(), before);
    }

    #[test]
    fn delay_can_be_changed_at_runtime() {
        let mut s = SnapshotStream::new();
        s.push(0.0, 0.0, 0.0f32);
        s.push(1.0, 1.0, 10.0);
        assert_eq!(s.sample(0.6).unwrap().value, 5.0);
        s.set_delay(0.0);
        assert_eq!(s.sample(0.6).unwrap().value, 6.0);
    }

    #[test]
    #[should_panic(expected = "capacity must be at least 2")]
    fn tiny_capacity_is_refused() {
        let _ = SnapshotBuffer::<f32>::with_config(SnapshotConfig {
            capacity: 1,
            ..Default::default()
        });
    }

    #[test]
    #[should_panic(expected = "delay must be a finite number")]
    fn negative_delay_is_refused() {
        SnapshotStream::<f32>::new().set_delay(-1.0);
    }
}
