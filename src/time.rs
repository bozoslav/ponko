//! [`Time`], the clock service at `ctx.time`, and the fixed-timestep
//! accumulator behind it.
//!
//! **What lives here:** the frame clock (`frame_dt`, `elapsed`, `frame`,
//! `fps`), the simulation clock (`sim_dt`, `sim_tick`, `sim_time`,
//! `fixed_dt`, `fixed_steps`, `fixed_alpha`) and its controls (`pause`,
//! `resume`, `step`, `set_speed`), plus [`Time::turns`] for spreading work
//! over ticks.
//!
//! **When to call it:** anywhere a system has `ctx`. Reads are valid in
//! setup, simulation systems and frame systems; `dt()` follows the kind of
//! system that is running, `frame_dt()` and `sim_dt()` say which one they
//! mean. The controls take effect from the next frame.
//!
//! **What it needs:** nothing. It works headless, without a window, GPU or
//! audio device, and only borrows `ctx.time`, so it can be read while a
//! query over `ctx.world` is alive.
//!
//! Kept free of any windowing types so it can be unit tested directly.

use ponko_ecs::Entity;

/// Longest frame delta the engine will accept, in seconds.
///
/// A stall (dragging the window, a breakpoint, a slow asset load) would otherwise
/// produce one huge `dt` and a burst of fixed steps trying to "catch up"
/// (the classic spiral of death). Clamping trades a brief slowdown for stability.
pub(crate) const MAX_FRAME_DT: f64 = 0.25;

/// Default fixed update rate.
pub(crate) const DEFAULT_FIXED_HZ: u32 = 60;

/// Most simulation steps run in one frame. At high sim speeds, or when the
/// simulation is too heavy to keep up, the backlog is dropped instead of
/// freezing the window: the simulation just runs slower than asked.
pub(crate) const MAX_STEPS_PER_FRAME: u32 = 64;

/// Timing results for one rendered frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameTiming {
    /// Clamped seconds since the previous frame.
    pub dt: f32,
    /// How many fixed steps the game should run this frame.
    pub fixed_steps: u32,
    /// How far (0..1) we are between the last fixed step and the next one.
    pub alpha: f32,
}

/// Turns variable frame deltas into a whole number of fixed steps per frame.
#[derive(Debug, Clone)]
pub(crate) struct Clock {
    fixed_dt: f64,
    accumulator: f64,
}

impl Clock {
    pub fn new(fixed_hz: u32) -> Self {
        assert!(fixed_hz > 0, "fixed update rate must be at least 1 Hz");
        Self {
            fixed_dt: 1.0 / f64::from(fixed_hz),
            accumulator: 0.0,
        }
    }

    pub fn fixed_dt(&self) -> f64 {
        self.fixed_dt
    }

    /// Advances the clock by one frame that took `frame_dt` seconds, with
    /// simulated time running `speed` times as fast (0 = paused).
    pub fn advance(&mut self, frame_dt: f64, speed: f64) -> FrameTiming {
        let dt = frame_dt.clamp(0.0, MAX_FRAME_DT);
        self.accumulator += dt * speed;

        let mut steps = (self.accumulator / self.fixed_dt).floor();
        if steps > f64::from(MAX_STEPS_PER_FRAME) {
            steps = f64::from(MAX_STEPS_PER_FRAME);
            self.accumulator = 0.0; // drop the backlog
        } else {
            self.accumulator -= steps * self.fixed_dt;
        }

        FrameTiming {
            dt: dt as f32,
            fixed_steps: steps as u32,
            alpha: (self.accumulator / self.fixed_dt) as f32,
        }
    }
}

/// Simulation clock state: speed, pause, single steps, tick count.
#[derive(Debug, Clone)]
struct SimClock {
    speed: f32,
    paused: bool,
    pending_steps: u32,
    tick: u64,
    /// Fixed step in seconds, full precision.
    step: f64,
}

/// The clocks of a game: `ctx.time`.
///
/// Two clocks run side by side. The *frame* clock follows the display: one
/// variable-length step per drawn frame. The *simulation* clock advances in
/// fixed steps ([`fixed_dt`](Self::fixed_dt)), zero or more per frame, so a
/// simulation gives the same result on every machine.
///
/// - **What lives here:** the frame clock (`frame_dt`, `elapsed`, `frame`,
///   `fps`), the simulation clock (`sim_dt`, `sim_tick`, `sim_time`,
///   `fixed_dt`, `fixed_steps`, `fixed_alpha`), its controls (`pause`,
///   `resume`, `step`, `set_speed`) and [`turns`](Self::turns).
/// - **When to call it:** in setup, simulation systems and frame systems.
///   `dt()` follows the kind of system that is running; `frame_dt()` and
///   `sim_dt()` say which one they mean. Controls apply from the next frame.
/// - **What it needs:** nothing; it works headless and borrows only
///   `ctx.time`.
///
/// ```
/// use ponko::prelude::*;
///
/// #[derive(Component)]
/// struct Pos(f32);
///
/// fn fall(ctx: &mut Ctx) {
///     // `ctx.time` is its own field, so it can be read inside a query.
///     for p in ctx.world.query::<&mut Pos>().iter() {
///         p.0 += 9.8 * ctx.time.sim_dt();
///     }
/// }
///
/// let mut world = Game::new("t", 1, 1).sim_system(fall).run_headless(60, |ctx| {
///     ctx.world.spawn((Pos(0.0),));
/// });
/// assert!((world.query::<&Pos>().iter().next().unwrap().0 - 9.8).abs() < 1e-3);
/// ```
#[derive(Debug, Clone)]
pub struct Time {
    /// `true` while simulation systems run: `dt()` then returns the fixed step.
    in_sim: bool,
    /// Fixed steps the running sim system covers (`IntoSystem::every`).
    sim_steps: u32,
    sim: SimClock,
    timing: FrameTiming,
    fixed_dt: f32,
    elapsed: f64,
    frame: u64,
    fps: f32,
}

impl Time {
    /// `fixed_dt` in seconds, as f64 so `sim_time()` stays exact over long runs.
    pub(crate) fn new(fixed_dt: f64) -> Self {
        Self {
            in_sim: false,
            sim_steps: 1,
            sim: SimClock {
                speed: 1.0,
                paused: false,
                pending_steps: 0,
                tick: 0,
                step: fixed_dt,
            },
            timing: FrameTiming {
                dt: 0.0,
                fixed_steps: 0,
                alpha: 0.0,
            },
            fixed_dt: fixed_dt as f32,
            elapsed: 0.0,
            frame: 0,
            fps: 0.0,
        }
    }

    /// Engine side: start a new frame with fresh timing.
    pub(crate) fn begin_frame(&mut self, timing: FrameTiming) {
        self.timing = timing;
        self.elapsed += f64::from(timing.dt);
        self.frame += 1;
        if timing.dt > 0.0 {
            let instant = 1.0 / timing.dt;
            // Exponential smoothing so the number is readable, seeded by the first sample.
            self.fps = if self.fps == 0.0 {
                instant
            } else {
                self.fps + (instant - self.fps) * 0.05
            };
        }
    }

    /// Engine side: speed to feed the clock this frame (0 while paused).
    pub(crate) fn effective_sim_speed(&self) -> f64 {
        if self.sim.paused {
            0.0
        } else {
            f64::from(self.sim.speed)
        }
    }

    /// Engine side: uses up one requested single step; `false` if none was pending.
    pub(crate) fn take_one_step(&mut self) -> bool {
        let had = self.sim.pending_steps > 0;
        self.sim.pending_steps = self.sim.pending_steps.saturating_sub(1);
        had
    }

    /// Engine side: single steps requested since the last frame.
    pub(crate) fn take_pending_steps(&mut self) -> u32 {
        std::mem::take(&mut self.sim.pending_steps)
    }

    /// Queues `n` single steps for a paused game.
    pub(crate) fn queue_steps(&mut self, n: u64) {
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        self.sim.pending_steps = self.sim.pending_steps.saturating_add(n);
    }

    /// Takes back up to `n` queued steps.
    pub(crate) fn drop_pending_steps(&mut self, n: u64) {
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        self.sim.pending_steps = self.sim.pending_steps.saturating_sub(n);
    }

    pub(crate) fn advance_tick(&mut self) {
        self.sim.tick += 1;
    }

    pub(crate) fn set_sim_steps(&mut self, steps: u32) {
        self.sim_steps = steps;
    }

    pub(crate) fn set_in_sim(&mut self, in_sim: bool) {
        self.in_sim = in_sim;
    }

    /// Whether a simulation system is running now.
    pub(crate) fn in_sim(&self) -> bool {
        self.in_sim
    }

    /// Seconds the running system should advance by: the shortcut that reads
    /// right in both kinds of system.
    ///
    /// In a frame system (and in setup) it is [`frame_dt`](Self::frame_dt).
    /// Inside a simulation system it is [`sim_dt`](Self::sim_dt). Use those
    /// two when the code should say which clock it means, for example a
    /// helper called from both kinds of system.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pos: &mut Vec2) {
    /// pos.x += 200.0 * ctx.time.dt(); // 200 pixels per second
    /// # }
    /// ```
    #[must_use]
    pub fn dt(&self) -> f32 {
        if self.in_sim { self.sim_dt() } else { self.timing.dt }
    }

    /// Seconds since the previous frame, in every kind of system. Clamped to
    /// at most 0.25 s so a hiccup never teleports objects. A headless run
    /// has one frame per tick, so there it is the fixed step.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn fade(ctx: &mut Ctx, alpha: &mut f32) {
    /// *alpha = (*alpha - ctx.time.frame_dt()).max(0.0); // real time, even in a sim system
    /// # }
    /// ```
    #[must_use]
    pub fn frame_dt(&self) -> f32 {
        self.timing.dt
    }

    /// Simulated seconds one run of the current simulation system covers:
    /// the fixed step, times the interval of a system added with one
    /// ([`IntoSystem::every`](crate::game::IntoSystem::every)). Outside
    /// simulation systems it is one fixed step, the same as
    /// [`fixed_dt`](Self::fixed_dt).
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// fn slow(ctx: &mut Ctx) {
    ///     assert_eq!(ctx.time.sim_dt(), 4.0 * ctx.time.fixed_dt());
    /// }
    /// Game::new("t", 1, 1).add(Phase::AfterPhysics, slow.every(4)).run_headless(8, |_| {});
    /// ```
    #[must_use]
    pub fn sim_dt(&self) -> f32 {
        self.fixed_dt * self.sim_steps as f32
    }

    /// How many fixed-rate updates to run this frame (often 0, 1 or 2).
    ///
    /// Use this for logic that must behave identically on every machine,
    /// such as physics or deterministic simulations. Each step lasts exactly
    /// [`fixed_dt`](Self::fixed_dt) seconds. The rate is set with
    /// [`Game::fixed_hz`](crate::game::Game::fixed_hz) (default 60).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, vel: &mut Vec2, pos: &mut Vec2) {
    /// for _ in 0..ctx.time.fixed_steps() {
    ///     vel.y += 980.0 * ctx.time.fixed_dt(); // gravity
    ///     *pos += *vel * ctx.time.fixed_dt();
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn fixed_steps(&self) -> u32 {
        self.timing.fixed_steps
    }

    /// Length of one fixed step in seconds (e.g. `1.0 / 60.0`).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// let steps_per_second = 1.0 / ctx.time.fixed_dt();
    /// # }
    /// ```
    #[must_use]
    pub fn fixed_dt(&self) -> f32 {
        self.fixed_dt
    }

    /// How far (0.0 to 1.0) the current frame is between the last fixed step
    /// and the next. Use it to interpolate positions for perfectly smooth
    /// rendering of fixed-step objects.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, prev: Vec2, curr: Vec2) {
    /// let draw_pos = prev.lerp(curr, ctx.time.fixed_alpha());
    /// # }
    /// ```
    #[must_use]
    pub fn fixed_alpha(&self) -> f32 {
        self.timing.alpha
    }

    /// Seconds of game time since the game started (the sum of every
    /// [`frame_dt`](Self::frame_dt)).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// let blink_on = (ctx.time.elapsed() * 2.0) as u64 % 2 == 0;
    /// # }
    /// ```
    #[must_use]
    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    /// Number of frames since the game started. The first frame is `1`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// if ctx.time.frame() % 60 == 0 { println!("fps: {:.0}", ctx.time.fps()); }
    /// # }
    /// ```
    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Smoothed frames per second.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// println!("{:.0} fps", ctx.time.fps());
    /// # }
    /// ```
    #[must_use]
    pub fn fps(&self) -> f32 {
        self.fps
    }

    /// Number of simulation ticks run so far. Use it to do work every Nth
    /// tick, or to timestamp events deterministically.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// fn slow_ai(ctx: &mut Ctx) {
    ///     if ctx.time.sim_tick() % 10 != 0 {
    ///         return; // think 6 times per second instead of 60
    ///     }
    ///     // ...
    /// }
    /// ```
    #[must_use]
    pub fn sim_tick(&self) -> u64 {
        self.sim.tick
    }

    /// Simulated seconds so far: `sim_tick() * fixed_dt()`.
    #[must_use]
    pub fn sim_time(&self) -> f64 {
        self.sim.tick as f64 * self.sim.step
    }

    /// Stops simulation systems from running (frame systems and drawing go on).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// fn controls(ctx: &mut Ctx) {
    ///     if ctx.input.key_pressed(Key::Space) {
    ///         if ctx.time.is_paused() { ctx.time.resume() } else { ctx.time.pause() }
    ///     }
    ///     if ctx.input.key_pressed(Key::Period) {
    ///         ctx.time.step(); // advance one tick while paused
    ///     }
    /// }
    /// ```
    pub fn pause(&mut self) {
        self.sim.paused = true;
    }

    /// Lets simulation systems run again after [`pause`](Self::pause).
    pub fn resume(&mut self) {
        self.sim.paused = false;
    }

    /// `true` while the simulation is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.sim.paused
    }

    /// Runs exactly one extra simulation tick next frame. Meant for stepping
    /// through a paused simulation; each call adds one tick.
    pub fn step(&mut self) {
        self.sim.pending_steps = self.sim.pending_steps.saturating_add(1);
    }

    /// Sets how fast simulated time runs: `1.0` real time, `4.0` four times
    /// as fast, `0.5` half speed. Ticks stay the same length (the fixed step),
    /// so results don't depend on speed; only how many ticks run per frame
    /// changes. If the simulation can't keep up, it runs as fast as it can.
    ///
    /// # Panics
    /// If `speed` is negative, NaN or infinite.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.time.set_speed(16.0); // fast-forward
    /// # }
    /// ```
    #[track_caller]
    pub fn set_speed(&mut self, speed: f32) {
        assert!(
            speed.is_finite() && speed >= 0.0,
            "Time::set_speed: speed must be a finite number >= 0 (got {speed})"
        );
        self.sim.speed = speed;
    }

    /// Current simulation speed (see [`set_speed`](Self::set_speed)).
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.sim.speed
    }

    /// Splits per-entity work over `every` ticks: entities take turns, so
    /// each tick updates 1/`every` of them. For costly logic that's fine at
    /// a lower rate per entity (path planning, AI decisions). Take it before
    /// the query; it is `Copy`, so it works in `par_for_each` too.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// #[derive(Component)]
    /// struct Brain { plans: u32 }
    ///
    /// fn plan(ctx: &mut Ctx) {
    ///     let turn = ctx.time.turns(4);
    ///     ctx.world.query::<(Entity, &mut Brain)>().par_for_each(|(e, b)| {
    ///         if turn.includes(e) {
    ///             b.plans += 1; // each entity plans every 4th tick
    ///         }
    ///     });
    /// }
    ///
    /// let mut world = Game::new("t", 1, 1).sim_system(plan).run_headless(40, |ctx| {
    ///     for _ in 0..10 { ctx.world.spawn((Brain { plans: 0 },)); }
    /// });
    /// assert!(world.query::<&Brain>().iter().all(|b| b.plans == 10));
    /// ```
    ///
    /// # Panics
    /// If `every` is 0.
    #[track_caller]
    pub fn turns(&self, every: u32) -> Turns {
        assert!(every > 0, "Time::turns: `every` must be at least 1");
        Turns {
            every,
            now: (self.sim.tick % u64::from(every)) as u32,
            dt: self.dt() * every as f32,
        }
    }
}

/// Which entities update this tick, from [`Time::turns`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Turns {
    every: u32,
    now: u32,
    dt: f32,
}

impl Turns {
    /// `true` if `entity`'s turn is this tick.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, e: Entity) {
    /// if ctx.time.turns(8).includes(e) { /* expensive update */ }
    /// # }
    /// ```
    #[inline]
    #[must_use]
    pub fn includes(self, entity: Entity) -> bool {
        entity.index() % self.every == self.now
    }

    /// Time since an entity's previous turn: `every` times `ctx.time.dt()`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let turn = ctx.time.turns(4);
    /// assert_eq!(turn.dt(), ctx.time.dt() * 4.0);
    /// # }
    /// ```
    #[must_use]
    pub fn dt(self) -> f32 {
        self.dt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_step_per_frame_when_rates_match() {
        let mut clock = Clock::new(60);
        let total: u32 = (0..600).map(|_| clock.advance(1.0 / 60.0, 1.0).fixed_steps).sum();
        // Float rounding may defer one step to the next frame, never more.
        assert!((599..=600).contains(&total), "got {total}");
    }

    #[test]
    fn fast_frames_accumulate_into_steps() {
        let mut clock = Clock::new(60);
        let total: u32 = (0..240).map(|_| clock.advance(1.0 / 240.0, 1.0).fixed_steps).sum();
        assert!((59..=60).contains(&total), "got {total}");
    }

    #[test]
    fn slow_frame_runs_multiple_steps() {
        let mut clock = Clock::new(60);
        let t = clock.advance(3.5 / 60.0, 1.0);
        assert_eq!(t.fixed_steps, 3);
        assert!((t.alpha - 0.5).abs() < 1e-4);
    }

    #[test]
    fn huge_frame_is_clamped() {
        let mut clock = Clock::new(60);
        let t = clock.advance(10.0, 1.0);
        assert_eq!(t.dt, MAX_FRAME_DT as f32);
        assert_eq!(t.fixed_steps, 15);
    }

    #[test]
    fn negative_dt_is_treated_as_zero() {
        let mut clock = Clock::new(60);
        let t = clock.advance(-1.0, 1.0);
        assert_eq!(t.dt, 0.0);
        assert_eq!(t.fixed_steps, 0);
    }

    #[test]
    fn speed_scales_steps() {
        let mut clock = Clock::new(60);
        let total: u32 = (0..60).map(|_| clock.advance(1.0 / 60.0, 4.0).fixed_steps).sum();
        assert!((239..=240).contains(&total), "got {total}");
    }

    #[test]
    fn zero_speed_pauses() {
        let mut clock = Clock::new(60);
        let total: u32 = (0..60).map(|_| clock.advance(1.0 / 60.0, 0.0).fixed_steps).sum();
        assert_eq!(total, 0);
    }

    #[test]
    fn steps_per_frame_are_capped_and_backlog_dropped() {
        let mut clock = Clock::new(60);
        let t = clock.advance(0.25, 100.0); // would be 1500 steps
        assert_eq!(t.fixed_steps, MAX_STEPS_PER_FRAME);
        assert_eq!(t.alpha, 0.0);
        let t = clock.advance(1.0 / 60.0, 1.0);
        assert_eq!(t.fixed_steps, 1, "no catch-up burst after the cap");
    }

    #[test]
    #[should_panic(expected = "at least 1 Hz")]
    fn zero_hz_panics() {
        Clock::new(0);
    }

    fn frame(dt: f32) -> FrameTiming {
        FrameTiming {
            dt,
            fixed_steps: 1,
            alpha: 0.5,
        }
    }

    #[test]
    fn dt_follows_the_running_system_and_the_explicit_pair_does_not() {
        let mut time = Time::new(1.0 / 50.0);
        time.begin_frame(frame(0.004));
        // A frame system: `dt` is the frame's.
        assert_eq!(time.dt(), 0.004);
        assert_eq!(time.frame_dt(), 0.004);
        assert_eq!(time.sim_dt(), 1.0 / 50.0);
        // A simulation system: `dt` is the step; the explicit pair is unchanged.
        time.set_in_sim(true);
        assert_eq!(time.dt(), 1.0 / 50.0);
        assert_eq!(time.frame_dt(), 0.004);
        assert_eq!(time.sim_dt(), 1.0 / 50.0);
        // `system.every(3)`: one run covers three steps.
        time.set_sim_steps(3);
        assert_eq!(time.dt(), 3.0 / 50.0);
        assert_eq!(time.sim_dt(), 3.0 / 50.0);
        assert_eq!(time.fixed_dt(), 1.0 / 50.0);
    }

    #[test]
    fn frames_add_up_and_ticks_count_simulated_time() {
        let mut time = Time::new(0.25);
        time.begin_frame(frame(0.5));
        time.begin_frame(frame(0.25));
        assert_eq!(time.frame(), 2);
        assert!((time.elapsed() - 0.75).abs() < 1e-9);
        assert_eq!(time.fixed_alpha(), 0.5);
        time.advance_tick();
        time.advance_tick();
        assert_eq!(time.sim_tick(), 2);
        assert_eq!(time.sim_time(), 0.5);
    }

    #[test]
    fn single_steps_are_used_up_one_at_a_time() {
        let mut time = Time::new(1.0 / 60.0);
        time.pause();
        assert_eq!(time.effective_sim_speed(), 0.0);
        time.step();
        time.step();
        assert!(time.take_one_step());
        assert_eq!(time.take_pending_steps(), 1);
        assert!(!time.take_one_step());
        time.resume();
        assert_eq!(time.effective_sim_speed(), 1.0);
    }
}
