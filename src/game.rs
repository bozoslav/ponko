//! [`Game`]: window creation and the main loop.

use std::time::Duration;
use web_time::Instant;

use glam::Vec2;

use ponko_ecs::World;

mod batch;
mod system;
#[cfg(feature = "window")]
mod window;

pub use system::{IntoSystem, SystemConfig};

use crate::control::{self, Control};
use crate::ctx::Ctx;
use crate::perf::SystemTiming;
use crate::systems::{animate_sprites, draw_particles, draw_sprites, draw_tilemaps, rebuild_spatial_grid};
use crate::time::{Clock, DEFAULT_FIXED_HZ, FrameTiming};

/// Trackpads report scrolling in pixels; this many logical pixels count as one
/// wheel "notch" so `mouse_wheel()` means roughly the same on every device.
const PIXELS_PER_SCROLL_LINE: f32 = 20.0;

/// How often to check whether a hidden/minimized window is visible again.
const OCCLUDED_POLL: Duration = Duration::from_millis(100);

/// Execution buckets, ordered independently of builder call order.
///
/// `Input` through `SimLast` run once per fixed tick (also headlessly).
/// `Frame` through `Draw` run once per rendered frame. Systems in the same
/// phase run in registration order, applying deferred commands after each
/// one. With [`Game::parallel_systems`] on, view systems of a phase that
/// don't conflict share a batch and may run at the same time; see
/// [`SchedulePlan`](crate::schedule::SchedulePlan). A
/// phase always ends before the next one starts.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Fixed-tick input preparation, before all other simulation work.
    Input,
    /// Fixed-tick gameplay that feeds the physics step.
    BeforePhysics,
    /// Fixed-tick physics integration.
    Physics,
    /// Fixed-tick gameplay observing the current physics result.
    AfterPhysics,
    /// Final fixed-tick work, before advancing the tick counter.
    SimLast,
    /// Per-frame gameplay, after the due simulation ticks.
    Frame,
    /// Per-frame pose work after frame gameplay. The built-in particle
    /// update ([`Systems::UPDATE_PARTICLES`]) runs at the end of this phase,
    /// after your systems in it; with the `3d` feature, the animators
    /// ([`Systems::UPDATE_ANIMATORS`]) run just before it.
    Animate,
    /// Per-frame work after animation and before drawing.
    Late,
    /// Per-frame drawing after final positions are available. With the `3d`
    /// feature, positional audio ([`Systems::UPDATE_AUDIO3D`]) runs at the
    /// end of this phase, after the drawing systems, so sound follows where
    /// everything ended up this frame.
    Draw,
}

/// A reusable module of systems and startup work.
///
/// `build` runs immediately during registration; `setup` runs once after runtime
/// initialization, interleaved with explicit setup callbacks in registration order.
/// Plugins need no registry, dependency graph, or additional runtime feature.
/// Each system names its [`Phase`], so the order in which plugins are added
/// only matters between systems of the same phase.
///
/// ```
/// use ponko::prelude::{Ctx, Game, Phase, Plugin};
///
/// struct Score;
/// impl Plugin for Score {
///     fn build(&self, game: Game) -> Game {
///         game.add(Phase::AfterPhysics, |ctx: &mut Ctx| *ctx.world.resource_mut::<u32>() += 1)
///     }
///     fn setup(&self, ctx: &mut Ctx) {
///         ctx.world.insert_resource(0u32);
///     }
/// }
///
/// let world = Game::new("score", 1, 1).plugin(Score).setup(|_| {}).run_headless(3);
/// assert_eq!(*world.resource::<u32>(), 3);
/// ```
pub trait Plugin {
    /// Registers systems and options on the existing builder.
    fn build(&self, game: Game) -> Game;
    /// Initializes this module before systems start. Commands apply afterward.
    fn setup(&self, _ctx: &mut Ctx) {}
}

/// An opaque registration handle, from [`Game::add_system`], for
/// [`Game::remove`] and [`Game::order`]. Registration IDs are unique across
/// game builders and never reused. Constants in [`Systems`] identify all
/// registrations of a built-in function. IDs are not persistence identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SystemId(u32);

/// Stable IDs for built-in systems. Feature-gated systems are only installed
/// when enabled/configured; removing an absent ID is a no-op.
pub struct Systems;

impl Systems {
    /// Fixed-tick 2D physics step.
    pub const PHYSICS: SystemId = SystemId(1);
    /// Fixed-tick 3D physics step.
    pub const PHYSICS3D: SystemId = SystemId(2);
    /// Per-frame interpolated 3D physics transforms.
    pub const INTERPOLATE_PHYSICS3D: SystemId = SystemId(3);
    /// Spatial grid rebuild.
    pub const REBUILD_SPATIAL_GRID: SystemId = SystemId(4);
    /// Sprite animation in the drawing bucket.
    pub const ANIMATE_SPRITES: SystemId = SystemId(5);
    /// Tilemap drawing.
    pub const DRAW_TILEMAPS: SystemId = SystemId(6);
    /// Sprite drawing.
    pub const DRAW_SPRITES: SystemId = SystemId(7);
    /// Attachment transforms, when explicitly registered.
    pub const FOLLOW_ATTACHMENTS: SystemId = SystemId(8);
    /// Navigation update, when explicitly registered.
    pub const STEP_NAVMESH3D: SystemId = SystemId(9);
    /// Advances every [`Animator`](crate::anim::Animator) and poses its skeleton
    /// (feature `3d`). Stays at the end of [`Phase::Animate`].
    pub const UPDATE_ANIMATORS: SystemId = SystemId(10);
    /// Advances particle emitters (2D, and with feature `3d` the 3D ones
    /// and trails). Stays at the end of [`Phase::Animate`], after the
    /// animators.
    pub const UPDATE_PARTICLES: SystemId = SystemId(11);
    /// Moves positional sounds to this frame's listener and emitters
    /// (feature `3d`). Stays at the end of [`Phase::Draw`], after the
    /// drawing systems.
    pub const UPDATE_AUDIO3D: SystemId = SystemId(12);
    /// 2D particle drawing, after the sprites.
    pub const DRAW_PARTICLES: SystemId = SystemId(13);
}

impl SystemId {
    /// The id as a plain number, for C/C++ and tools. The [`Systems`]
    /// constants keep their numbers; other ids are only valid in this run.
    #[must_use]
    pub fn to_bits(self) -> u32 {
        self.0
    }

    /// An id from its number, as returned by [`to_bits`](Self::to_bits).
    #[must_use]
    pub fn from_bits(bits: u32) -> Self {
        SystemId(bits)
    }
}

/// The id of built-in system function `name` (a type name), with the name
/// it is listed under.
fn builtin_id(name: &str) -> Option<(SystemId, &'static str)> {
    /// The function in `ponko::systems`, listed once under its own name.
    macro_rules! builtin {
        ($name:ident) => {{
            let path = std::any::type_name_of_val(&crate::systems::$name);
            [path, path]
        }};
        // Public in `systems`, defined in its topic module: listed under the
        // topic module's path, the name it had before `systems` was its
        // only public path, so names in plans and `remove_system` stay.
        ($module:ident :: $name:ident) => {
            [
                std::any::type_name_of_val(&crate::$module::$name),
                std::any::type_name_of_val(&crate::systems::$name),
            ]
        };
    }
    #[cfg(feature = "3d")]
    let one = |name: &'static str| [name, name];
    let entries = [
        (builtin!(sheet::animate_sprites), Systems::ANIMATE_SPRITES),
        (builtin!(tilemap::draw_tilemaps), Systems::DRAW_TILEMAPS),
        (builtin!(draw_sprites), Systems::DRAW_SPRITES),
        (builtin!(draw_particles), Systems::DRAW_PARTICLES),
        (builtin!(update_particles), Systems::UPDATE_PARTICLES),
        (builtin!(spatial::rebuild_spatial_grid), Systems::REBUILD_SPATIAL_GRID),
        (builtin!(attach::follow_attachments), Systems::FOLLOW_ATTACHMENTS),
        #[cfg(feature = "physics")]
        (builtin!(step_physics), Systems::PHYSICS),
        #[cfg(feature = "physics3d")]
        (builtin!(step_physics3d), Systems::PHYSICS3D),
        #[cfg(feature = "physics3d")]
        (builtin!(interpolate_physics3d), Systems::INTERPOLATE_PHYSICS3D),
        #[cfg(feature = "navmesh")]
        (builtin!(step_navmesh3d), Systems::STEP_NAVMESH3D),
        #[cfg(feature = "3d")]
        (
            one(std::any::type_name_of_val(&crate::systems::update_animators)),
            Systems::UPDATE_ANIMATORS,
        ),
        #[cfg(feature = "3d")]
        (
            one(std::any::type_name_of_val(&crate::systems::update_audio3d)),
            Systems::UPDATE_AUDIO3D,
        ),
    ];
    entries
        .into_iter()
        .find_map(|(names, id)| names.contains(&name).then_some((id, names[0])))
}

/// A game: a window plus the loop that drives your code.
///
/// ```no_run
/// use ponko::prelude::*;
///
/// Game::new("My Game", 800, 600).run(|ctx| {
///     // Setup: runs once, after the window exists.
///     let crab = ctx.gfx.load_texture("crab.png");
///     let mut pos = vec2(400.0, 300.0);
///
///     // Returned closure: runs every frame.
///     move |ctx: &mut Ctx| {
///         if ctx.input.key_down(Key::Right) { pos.x += 200.0 * ctx.time.dt(); }
///         if ctx.input.key_pressed(Key::Escape) { ctx.quit(); }
///         ctx.gfx.draw(crab, pos);
///     }
/// });
/// ```
///
/// With the ECS, the game is entities plus systems:
///
/// ```no_run
/// use ponko::prelude::*;
///
/// #[derive(Component)]
/// struct Vel(Vec2);
///
/// fn movement(ctx: &mut Ctx) {
///     let dt = ctx.time.dt();
///     ctx.world.query::<(&mut ponko::d2::Transform, &Vel)>().for_each(|(t, v)| t.position += v.0 * dt);
/// }
///
/// Game::new("Crabs", 800, 600).sim_system(movement).run(|ctx| {
///     let crab = ctx.gfx.load_texture("crab.png");
///     ctx.world.spawn((ponko::d2::Transform::at(vec2(100.0, 100.0)), ponko::d2::Sprite::new(crab), Vel(vec2(50.0, 0.0))));
/// });
/// ```
#[must_use = "a Game does nothing until `.run()` or `.run_headless()`"]
pub struct Game {
    setups: Vec<SetupFn>,
    title: String,
    width: u32,
    height: u32,
    fixed_hz: u32,
    /// `fixed_hz` was called: `physics3d` keeps that rate.
    fixed_hz_set: bool,
    seed: u64,
    vsync: bool,
    fullscreen: bool,
    sim_systems: Vec<System>,
    systems: Vec<System>,
    late_systems: Vec<System>,
    /// Built-in drawing; runs after every other system so it sees final positions.
    draw_systems: Vec<System>,
    /// Cell size from `spatial_grid`, applied to `ctx.spatial` when the game starts.
    spatial_cell: Option<f32>,
    /// Gravity from `physics`, applied to `ctx.physics` when the game starts.
    #[cfg(feature = "physics")]
    gravity: Option<Vec2>,
    /// Gravity from `physics3d`, applied to `ctx.physics3d` when the game starts.
    #[cfg(feature = "physics3d")]
    gravity3d: Option<glam::Vec3>,
    /// The lockstep session from `lockstep`, moved into `ctx.net` at start.
    net: Option<crate::net::Net>,
    /// The message endpoint from `messages`, moved into `ctx.net` at start.
    messages: Option<crate::net::NetMessages>,
    /// Open the control endpoint (also on with `PONKO_CONTROL=1` where the
    /// build honors it, see `control_endpoint`).
    control: bool,
    /// Reload asset and data files when they change (`PONKO_HOT_RELOAD` overrides).
    hot_reload: bool,
    inspector: bool,
    /// Scheduled input from `input_scenario`, fed at the start of sim ticks.
    input_scenario: Option<crate::scenario::InputScenario>,
    /// Whether tools may start input scenarios through the control endpoint.
    input_injection: bool,
    /// From `headless_frames`: headless runs also run frames and the UI.
    headless_frames: bool,
    /// From `parallel_systems`: batches of view systems may use other threads.
    parallel: bool,
    /// From `parallel_threshold`.
    parallel_threshold: Option<Duration>,
    /// `PONKO_PARALLEL_SYSTEMS` may override the two above.
    parallel_env: bool,
    /// Phases where `barrier` was called and no system was added since.
    pending_barriers: Vec<Phase>,
    /// From `order`: the second system must see the first's work.
    order: Vec<(SystemId, SystemId)>,
    /// The stages the systems run in, built when the game starts.
    exec: batch::Exec,
    /// From `app_id`: names the game's folder in the player's data folder.
    app_id: Option<String>,
    /// From `data_dir`: the exact folder for `ctx.storage`.
    data_dir: Option<std::path::PathBuf>,
}

type SetupFn = Box<dyn FnOnce(&mut Ctx)>;

/// A game with stored startup callbacks and closure-free run methods.
///
/// Returned by [`Game::setup`], or by [`Game::configured`] for a game with
/// no setup of its own. Configure systems and window options on `Game`
/// first, then register one or more setup callbacks. A configured game still
/// takes the settings a test or a tool adds without touching the game's
/// code: [`headless_frames`](Self::headless_frames),
/// [`input_scenario`](Self::input_scenario) and
/// [`run_headless_scenario`](Self::run_headless_scenario),
/// [`data_dir`](Self::data_dir), [`seed`](Self::seed), and systems of its
/// own ([`system`](Self::system), [`sim_system`](Self::sim_system)). It is a separate type
/// because its `run()` takes no closure and [`Game::run`] takes one, and
/// Rust has no two methods of one name on a type.
#[must_use = "a configured game does nothing until run or run_headless"]
pub struct ConfiguredGame {
    game: Game,
}

fn run_setups(setups: Vec<SetupFn>, ctx: &mut Ctx) {
    for setup in setups {
        setup(ctx);
        ctx.apply_commands();
        ctx.follow_msaa();
    }
}

impl ConfiguredGame {
    /// Appends a startup callback. Callbacks run once in registration order,
    /// after runtime initialization and before any systems. Deferred commands
    /// are applied after each callback, including when running zero ticks.
    pub fn setup(mut self, setup: impl FnOnce(&mut Ctx) + 'static) -> Self {
        self.game.setups.push(Box::new(setup));
        self
    }

    /// Registers a plugin after earlier startup callbacks. Its systems are
    /// configured immediately and its setup joins the same ordered queue.
    pub fn plugin(mut self, plugin: impl Plugin + 'static) -> Self {
        self.game = self.game.plugin(plugin);
        self
    }

    /// Opens the window and runs with the stored setup callbacks.
    /// Has the same platform requirements and panics as [`Game::run`].
    pub fn run(self) {
        self.game.run(|_| {});
    }

    /// Runs fixed ticks without a window or GPU, returning the world.
    /// Frame systems do not run; see [`Game::run_headless`].
    pub fn run_headless(self, ticks: u64) -> World {
        self.game.run_headless(ticks, |_| {})
    }

    /// Profiles fixed ticks using the stored setup callbacks.
    /// See [`Game::profile_headless`] for warmup and report semantics.
    pub fn profile_headless(self, warmup: u64, ticks: u64) -> (World, crate::perf::PerfReport) {
        self.game.profile_headless(warmup, ticks, |_| {})
    }

    /// Runs `scenario` headlessly with the stored setup callbacks and returns
    /// the world with the scenario's report: [`Game::run_headless_scenario`]
    /// for a game that is already configured, such as the `game()` of a
    /// project made with `ponko new`.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::scenario::{InputScenario, ScenarioCheck};
    ///
    /// // In a project: `src/lib.rs`.
    /// fn game() -> ConfiguredGame {
    ///     Game::new("Menu", 640, 480).setup(|ctx| {
    ///         let root = ctx.ui.root().layout(ponko::ui::UiLayout::fill()).id();
    ///         ctx.ui.button(root, "Play").name("menu.play").focused();
    ///         ctx.ui.push_screen(root, false);
    ///     })
    /// }
    ///
    /// // In its tests: frames, a scenario and a storage folder of the test's own.
    /// let scenario = InputScenario::new().expect(0, ScenarioCheck::ui("menu.play").shown(true));
    /// let (_world, report) = game()
    ///     .headless_frames(true)
    ///     .data_dir(std::env::temp_dir().join("menu-test-saves"))
    ///     .run_headless_scenario(scenario);
    /// report.assert_passed();
    /// ```
    pub fn run_headless_scenario(
        self,
        scenario: crate::scenario::InputScenario,
    ) -> (World, crate::scenario::ScenarioReport) {
        self.game.run_headless_scenario(scenario, |_| {})
    }

    /// Runs whole frames in headless runs, UI and frame systems included:
    /// [`Game::headless_frames`].
    pub fn headless_frames(mut self, on: bool) -> Self {
        self.game = self.game.headless_frames(on);
        self
    }

    /// Feeds scheduled input into the game by tick: [`Game::input_scenario`].
    pub fn input_scenario(mut self, scenario: crate::scenario::InputScenario) -> Self {
        self.game = self.game.input_scenario(scenario);
        self
    }

    /// Keeps `ctx.storage`'s files in `dir`, for tests that must not touch the
    /// player's saves: [`Game::data_dir`].
    pub fn data_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.game = self.game.data_dir(dir);
        self
    }

    /// Sets the seed for [`Ctx::random`]: [`Game::seed`].
    pub fn seed(mut self, seed: u64) -> Self {
        self.game = self.game.seed(seed);
        self
    }

    /// Adds a frame system after the game's own: [`Game::system`]. For a
    /// test or a tool that watches or drives a finished game.
    pub fn system<F: FnMut(&mut Ctx) + 'static>(mut self, system: F) -> Self {
        self.game = self.game.system(system);
        self
    }

    /// Adds a simulation system after the game's own: [`Game::sim_system`].
    /// For a test or a tool that watches or drives a finished game.
    pub fn sim_system<F: FnMut(&mut Ctx) + 'static>(mut self, system: F) -> Self {
        self.game = self.game.sim_system(system);
        self
    }
}

type SystemFn = Box<dyn FnMut(&mut Ctx)>;

/// What a system runs.
enum Run {
    /// A function of the whole `Ctx`: always alone.
    Ctx(SystemFn),
    /// A view system. `None` only while a batch has it.
    View(Option<Box<crate::system_view::ViewRunner>>),
}

/// A system: a named function that runs over the world.
struct System {
    key: SystemId,
    builtin: Option<SystemId>,
    name: &'static str,
    stage: Phase,
    run: Run,
    /// Simulation systems only: run on ticks where `tick % every == phase`.
    every: u32,
    phase: u32,
    /// Unique per system: its read position for events.
    id: u32,
    /// Declared access (`IntoSystem::access`); `None` is exclusive.
    access: Option<crate::access::SystemAccess>,
    /// Stays at the end of its phase: systems added later go before it.
    /// Set for the built-ins that must see what the phase's other systems did.
    last: bool,
    /// Where this system last finished: its `Added` and `Changed` filters
    /// and `removed` report what happened after it.
    changes_seen: ChangesSeen,
    /// A view system: it can't reach anything undeclared.
    enforced: bool,
    /// `Game::barrier` was called before it was added: it starts a stage.
    barrier: bool,
}

/// A system's place in the world's change history: the change tick at which
/// it last finished, and the world that tick belongs to.
#[derive(Clone, Copy, Default)]
struct ChangesSeen {
    /// [`World::id`] of the world the system last ran in; 0 before its first run.
    world: u64,
    tick: ponko_ecs::Tick,
}

impl ChangesSeen {
    /// The tick to compare `world`'s changes against. A system that last
    /// ran in another world (the game replaced `ctx.world`) has seen
    /// nothing of this one: the new world's ticks start over, so the old
    /// tick would hide whatever was stamped before the new world reached it.
    fn in_world(self, world: &ponko_ecs::World) -> ponko_ecs::Tick {
        if self.world == world.id() {
            self.tick
        } else {
            ponko_ecs::Tick::default()
        }
    }
}

/// Ids for [`System::id`]; 0 means "outside any system".
fn next_system_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1024);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    assert!(id >= 1024, "system ID space exhausted");
    id
}

/// This frame's timing: the clock at the current sim speed (0 while
/// paused), plus any single steps requested with `ctx.time.step()`.
///
/// While an input scenario runs, a frame runs one tick at most. Pressed and
/// released edges last for a frame, so in a frame with several ticks a
/// scenario's key press would be "just pressed" in every one of them; a
/// headless run has one tick per frame, and a windowed run of the same
/// scenario has to see the same input.
///
/// - Steps that were asked for (a tool stepping a paused game) are never
///   lost: the ones that don't fit this frame stay queued for the next.
/// - The clock's own extra ticks are dropped, as the clock drops what it
///   can't catch up on: a window that draws slower than the simulation
///   ticks runs the scenario slower than real time, with no backlog during
///   it and no burst after it.
fn frame_timing(clock: &mut Clock, ctx: &mut Ctx, frame_dt: f64) -> FrameTiming {
    let mut timing = clock.advance(frame_dt, ctx.effective_sim_speed());
    let asked = ctx.take_pending_steps();
    if ctx.input.scenarios.run.is_none() {
        timing.fixed_steps += asked;
        return timing;
    }
    if timing.fixed_steps > 0 {
        // The clock's tick is this frame's; everything asked for waits.
        timing.fixed_steps = 1;
        ctx.time.queue_steps(u64::from(asked));
    } else if asked > 0 {
        timing.fixed_steps = 1;
        ctx.time.queue_steps(u64::from(asked - 1));
    }
    timing
}

fn end_timings(ctx: &mut Ctx) {
    for t in &mut ctx.perf.timings {
        t.avg_ms += (t.last_ms - t.avg_ms) * 0.05;
    }
}

/// Drops the removals of tracked components that every one of `systems` has
/// already been shown (those logged before the oldest of their last runs),
/// so the logs don't grow. A system that hasn't run in this world yet keeps
/// everything: while one never runs (the simulation is paused, say), the
/// removal logs of tracked components grow with every removal.
fn forget_seen_removals<'a>(ctx: &mut Ctx, systems: impl Iterator<Item = &'a System>) {
    if let Some(oldest) = systems.map(|s| s.changes_seen.in_world(&ctx.world)).min() {
        ctx.world.forget_removed(oldest);
    }
}

/// Runs one system, applies its commands, and records how long both took.
#[inline]
fn run_one(s: &mut System, ctx: &mut Ctx, timing_index: usize) {
    #[cfg(feature = "profiling")]
    puffin::profile_scope!("system", s.name);
    let allocs_before = ctx.perf.state.on.then(crate::perf::alloc_counts);
    let start = Instant::now();
    ctx.system_index = timing_index;
    ctx.events.reader = s.id;
    // Change filters in this system report what happened since it last ran.
    ctx.world.set_changes_since(s.changes_seen.in_world(&ctx.world));
    ctx.sync_services();
    // View systems can't reach anything undeclared; only `Ctx` systems
    // with a declaration need the recorder.
    #[cfg(debug_assertions)]
    let check = s.access.as_ref().filter(|_| !s.enforced);
    #[cfg(debug_assertions)]
    if check.is_some() {
        crate::access::begin_check(ctx);
    }
    match &mut s.run {
        Run::Ctx(f) => f(ctx),
        Run::View(runner) => runner.as_mut().expect("a batch gives its systems back").run_alone(ctx),
    }
    #[cfg(debug_assertions)]
    if let Some(access) = check {
        crate::access::end_check(ctx, s.name, access);
    }
    ctx.events.reader = 0;
    ctx.apply_commands();
    // The system's own changes (and its commands, just applied) belong to
    // the tick that ends here, so it won't be shown them next time; every
    // other system will. Code outside systems sees everything.
    // If the system replaced the world, this is the new world's tick.
    s.changes_seen = ChangesSeen {
        world: ctx.world.id(),
        tick: ctx.world.advance_change_tick(),
    };
    ctx.world.set_changes_since(ponko_ecs::Tick::default());
    ctx.perf.timings[timing_index].last_ms += start.elapsed().as_secs_f32() * 1000.0;
    if let Some(before) = allocs_before {
        let made = crate::perf::alloc_counts().count.saturating_sub(before.count);
        ctx.perf.state.add_system_allocs(timing_index, made);
    }
}

impl System {
    /// A `Ctx` system with no options set.
    fn new<F: FnMut(&mut Ctx) + 'static>(f: F) -> Self {
        Self::from_config(f.into_system())
    }

    /// The system `config` describes, not yet in a phase.
    ///
    /// # Panics
    /// If it is a view system whose declaration a view can't have.
    #[track_caller]
    fn from_config(config: SystemConfig) -> Self {
        let SystemConfig {
            run,
            name,
            builtin,
            every,
            access,
        } = config;
        let id = next_system_id();
        let (run, access, enforced) = match run {
            system::ConfigRun::Ctx(f) => (Run::Ctx(f), access, false),
            system::ConfigRun::View(f) => {
                // A view system that declares nothing can reach nothing.
                let access = access.unwrap_or_default();
                if let Some(problem) = access.view_problem() {
                    panic!("Game::add: view system `{name}` {problem}");
                }
                let runner = crate::system_view::ViewRunner::new(name, access.clone(), f);
                (Run::View(Some(Box::new(runner))), Some(access), true)
            }
        };
        System {
            key: SystemId(id),
            builtin,
            name,
            stage: Phase::AfterPhysics,
            run,
            every,
            phase: 0,
            id,
            access,
            last: false,
            changes_seen: ChangesSeen::default(),
            enforced,
            barrier: false,
        }
    }

    /// A built-in that stays at the end of its phase.
    fn last<F: FnMut(&mut Ctx) + 'static>(f: F) -> Self {
        let mut s = System::new(f);
        s.last = true;
        s
    }
}

/// Boxes system `f`. With the `hotpatch` feature, each call goes through
/// subsecond's jump table, so a patched version of the system (and of what
/// it calls) runs from the next call on.
#[cfg(not(feature = "hotpatch"))]
fn hot<F: FnMut(&mut Ctx) + 'static>(f: F) -> SystemFn {
    Box::new(f)
}

#[cfg(feature = "hotpatch")]
fn hot<F: FnMut(&mut Ctx) + 'static>(mut f: F) -> SystemFn {
    /// Calls the system. A zero-sized function item, instantiated in the
    /// game's crate, so subsecond finds its patched version in the jump
    /// table (a pointer-sized `&mut F` would be taken for a fn pointer).
    fn call<F: FnMut(&mut Ctx)>(f: &mut F, ctx: &mut Ctx) {
        f(ctx);
    }
    Box::new(move |ctx: &mut Ctx| {
        // A patch that changed code "above" this system (in its caller)
        // can't run in place; skip this call, the next one is current.
        let _ = subsecond::HotFn::current(call::<F>).try_call((&mut f, ctx));
    })
}

/// Boxes view system `f`, like [`hot`] does for `Ctx` systems.
#[cfg(not(feature = "hotpatch"))]
fn hot_view<F>(f: F) -> crate::system_view::ViewFn
where
    F: FnMut(&mut crate::system_view::SystemView<'_>) + Send + 'static,
{
    Box::new(f)
}

#[cfg(feature = "hotpatch")]
fn hot_view<F>(mut f: F) -> crate::system_view::ViewFn
where
    F: FnMut(&mut crate::system_view::SystemView<'_>) + Send + 'static,
{
    /// As in `hot`: a zero-sized function item for subsecond's jump table.
    fn call<F: FnMut(&mut crate::system_view::SystemView<'_>)>(
        f: &mut F,
        view: &mut crate::system_view::SystemView<'_>,
    ) {
        f(view);
    }
    Box::new(move |view: &mut crate::system_view::SystemView<'_>| {
        let _ = subsecond::HotFn::current(call::<F>).try_call((&mut f, view));
    })
}

/// With the `hotpatch` feature: connects to `dx serve --hotpatch` (when the
/// game was started by it) so patches are applied as they arrive.
fn connect_hotpatch() {
    #[cfg(feature = "hotpatch")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // stdout: `dx` shows stderr lines as errors.
            subsecond::register_handler(std::sync::Arc::new(|| println!("ponko: code patched")));
            dioxus_devtools::connect_subsecond();
        });
    }
}

/// What `setup` may return: nothing, or a closure to run every frame.
///
/// Implemented for `()` and for every `FnMut(&mut Ctx)`.
#[doc(hidden)]
pub trait SetupResult {
    #[doc(hidden)]
    fn into_frame_fn(self) -> Option<SystemFn>;
}

impl SetupResult for () {
    fn into_frame_fn(self) -> Option<SystemFn> {
        None
    }
}

impl<F: FnMut(&mut Ctx) + 'static> SetupResult for F {
    fn into_frame_fn(self) -> Option<SystemFn> {
        Some(Box::new(self))
    }
}

impl Game {
    /// Describes a game window with a title and size in (logical) pixels.
    ///
    /// Nothing opens until you call [`run`](Self::run).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Space Crab", 1280, 720).run(|_| |_: &mut Ctx| {});
    /// ```
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        let mut game = Self {
            setups: Vec::new(),
            title: title.into(),
            width,
            height,
            fixed_hz: DEFAULT_FIXED_HZ,
            fixed_hz_set: false,
            seed: crate::random::DEFAULT_SEED,
            vsync: true,
            fullscreen: false,
            sim_systems: Vec::new(),
            systems: Vec::new(),
            late_systems: Vec::new(),
            draw_systems: vec![
                System::new(animate_sprites),
                System::new(draw_tilemaps),
                System::new(draw_sprites),
                System::new(draw_particles),
            ],
            spatial_cell: None,
            #[cfg(feature = "physics")]
            gravity: None,
            #[cfg(feature = "physics3d")]
            gravity3d: None,
            net: None,
            messages: None,
            control: control::env_control(),
            hot_reload: cfg!(debug_assertions),
            inspector: cfg!(debug_assertions),
            input_scenario: None,
            input_injection: false,
            headless_frames: false,
            parallel: false,
            parallel_threshold: None,
            parallel_env: true,
            pending_barriers: Vec::new(),
            order: Vec::new(),
            exec: batch::Exec::default(),
            app_id: None,
            data_dir: None,
        };
        #[cfg(feature = "3d")]
        game.insert_system(Phase::Animate, System::last(crate::systems::update_animators));
        game.insert_system(Phase::Animate, System::last(crate::systems::update_particles));
        #[cfg(feature = "3d")]
        game.insert_system(Phase::Draw, System::last(crate::systems::update_audio3d));
        game
    }

    /// Adds a system to `phase` and returns the game, to chain more builder
    /// calls; [`add_system`](Self::add_system) returns the system's id
    /// instead. The general form of [`sim_system`](Self::sim_system)
    /// (`Phase::AfterPhysics`) and [`system`](Self::system) (`Phase::Frame`).
    ///
    /// `system` is anything that is [`IntoSystem`]:
    ///
    /// - a function or closure that takes `&mut Ctx`. It may use the whole
    ///   engine and always runs alone, in order;
    /// - a **view system**: a `Send` function or closure that takes
    ///   `&mut SystemView` and gets exactly what its declaration lists, so
    ///   the declaration is enforced by the view, not trusted. View systems
    ///   run one at a time, in order, unless
    ///   [`parallel_systems`](Self::parallel_systems) is on: then the ones
    ///   of a phase that don't conflict share a batch and may run at the
    ///   same time;
    /// - either of them with options: [`named`](IntoSystem::named),
    ///   [`every`](IntoSystem::every) (an interval in ticks) and
    ///   [`access`](IntoSystem::access) (what it reads and writes).
    ///
    /// A closure names its parameter type (`|ctx: &mut Ctx| ...`): that is
    /// how Rust tells the two kinds of system apart.
    ///
    /// Built-in physics always uses `Phase::Physics`; use `BeforePhysics` for
    /// logic that must affect the current physics step. See
    /// [`SystemAccess`](crate::access::SystemAccess), the
    /// [`schedule`](crate::schedule) module and the guide's "Systems",
    /// "Declared system access" and "Parallel systems" chapters.
    ///
    /// ```
    /// use ponko::access::{Service, SystemAccess};
    /// use ponko::prelude::*;
    /// use ponko::system_view::SystemView;
    ///
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// #[derive(Component)]
    /// struct Vel(f32);
    ///
    /// fn steer(ctx: &mut Ctx) {
    ///     ctx.world.query::<&mut Vel>().for_each(|v| v.0 = 1.0);
    /// }
    /// fn movement(sys: &mut SystemView) {
    ///     let dt = sys.dt();
    ///     sys.world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| p.0 += v.0 * dt);
    /// }
    ///
    /// let mut world = Game::new("add", 1, 1)
    ///     .add(Phase::BeforePhysics, steer)
    ///     .add(
    ///         Phase::AfterPhysics,
    ///         movement.access(SystemAccess::new().write::<Pos>().read::<Vel>().read_service(Service::Time)),
    ///     )
    ///     .add(Phase::SimLast, (|_: &mut Ctx| {}).named("bookkeeping").every(30))
    ///     .run_headless(60, |ctx| {
    ///         ctx.world.spawn((Pos(0.0), Vel(0.0)));
    ///     });
    /// let x = world.query::<&Pos>().iter().next().unwrap().0;
    /// assert!((x - 1.0).abs() < 1e-4);
    /// ```
    ///
    /// # Panics
    /// - If the system has an interval ([`every`](IntoSystem::every)) above 1
    ///   and `phase` is a per-frame phase (`Frame` through `Draw`).
    /// - If it is a view system and its declaration can't be a view's:
    ///   exclusive, items without a Rust type (the C/C++ path), or a service
    ///   the view doesn't offer (`gfx`, `ui`, `assets`, `perf`, or writing
    ///   `input`, `time` or `spatial`). Those systems take `&mut Ctx`.
    #[track_caller]
    pub fn add<T>(mut self, phase: Phase, system: impl IntoSystem<T>) -> Self {
        self.add_config(phase, system.into_system());
        self
    }

    /// Adds a system to `phase` and returns its id, for
    /// [`remove`](Self::remove) and [`order`](Self::order), instead of the
    /// game: [`add`](Self::add) for when the id is needed. It is the one
    /// builder method that borrows the game instead of taking and
    /// returning it, so it doesn't chain.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut game = Game::new("ids", 1, 1);
    /// let id = game.add_system(Phase::AfterPhysics, |_: &mut Ctx| {});
    /// assert_eq!(game.system_count(id), 1);
    /// assert_eq!(game.remove(id).system_count(id), 0);
    /// ```
    ///
    /// # Panics
    /// As [`add`](Self::add).
    #[track_caller]
    pub fn add_system<T>(&mut self, phase: Phase, system: impl IntoSystem<T>) -> SystemId {
        self.add_config(phase, system.into_system())
    }

    #[track_caller]
    fn add_config(&mut self, phase: Phase, config: SystemConfig) -> SystemId {
        assert!(
            config.every == 1 || phase <= Phase::SimLast,
            "Game::add: system `{}` has an interval, and only fixed-tick phases take one",
            config.name
        );
        let mut s = System::from_config(config);
        if s.every > 1 {
            // Spread systems of one interval over its ticks.
            let same = self.sim_systems.iter().filter(|x| x.every == s.every).count() as u32;
            s.phase = same % s.every;
        }
        self.insert_system(phase, s)
    }

    /// Lets view systems (see [`add`](Self::add)) that don't conflict
    /// run at the same time, on several threads. Off by default: then every
    /// system runs alone, in order, as always.
    ///
    /// On, the systems of each phase are put into stages
    /// ([`schedule_plan`](Self::schedule_plan) shows them): view systems
    /// whose declarations don't conflict share a **batch**; every other
    /// system (all `&mut Ctx` systems, and view systems that hold sound, a
    /// camera, a physics world or networking) is a stage of its own, which
    /// no system moves across. Between those, a view system goes to the
    /// earliest stage after every system whose work it must see, so it may
    /// run ahead of systems registered before it that it doesn't interact
    /// with. Systems that conflict keep their registration order. Commands recorded in a
    /// batch are applied, and events sent in it become visible, when the
    /// batch ends, in registration order; the conflict rules make sure no
    /// system could have seen them earlier anyway.
    ///
    /// The simulation comes out the same as with it off, whatever the
    /// threads do, down to the world's change ticks: a game in lockstep can
    /// turn it on for some players only. Off, the scheduler never touches
    /// the thread pool (rayon's global pool): a game that doesn't turn it
    /// on gets no threads from here and can still configure the pool.
    ///
    /// If a system of a batch panics, the batch's other systems still run
    /// (as they may already be running on other threads), then the panic
    /// is passed on. That batch has no barrier: the commands its systems
    /// recorded and the events they sent are dropped.
    /// A batch whose systems are too cheap to be worth another thread runs
    /// in order on the game's thread ([`parallel_threshold`](Self::parallel_threshold)),
    /// and so do all batches while the profiler is on, on the web, and on a
    /// machine with one core. Many tiny systems don't get faster: see the
    /// guide's "Parallel systems" chapter for measurements.
    ///
    /// The environment variable `PONKO_PARALLEL_SYSTEMS` overrides this:
    /// `1` on, `0` off, `always` on with a threshold of zero (every batch
    /// goes to the thread pool, for tests).
    ///
    /// ```
    /// use ponko::access::SystemAccess;
    /// use ponko::prelude::*;
    /// use ponko::system_view::SystemView;
    ///
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// #[derive(Component)]
    /// struct Health(i32);
    ///
    /// fn drift(sys: &mut SystemView) {
    ///     sys.world.query::<&mut Pos>().for_each(|p| p.0 += 1.0);
    /// }
    /// fn decay(sys: &mut SystemView) {
    ///     sys.world.query::<&mut Health>().for_each(|h| h.0 -= 1);
    /// }
    ///
    /// let mut world = Game::new("parallel", 1, 1)
    ///     .parallel_systems(true)
    ///     .add(Phase::AfterPhysics, drift.access(SystemAccess::new().write::<Pos>()))
    ///     .add(Phase::AfterPhysics, decay.access(SystemAccess::new().write::<Health>()))
    ///     .run_headless(10, |ctx| {
    ///         ctx.world.spawn((Pos(0.0), Health(100)));
    ///     });
    /// let mut rows = world.query::<(&Pos, &Health)>();
    /// let (p, h) = rows.iter().next().unwrap();
    /// assert_eq!((p.0, h.0), (10.0, 90));
    /// ```
    pub fn parallel_systems(mut self, on: bool) -> Self {
        self.parallel = on;
        self.exec.invalidate();
        self
    }

    /// How much work a batch must have before it goes to other threads,
    /// with [`parallel_systems`](Self::parallel_systems) on: the usual run
    /// time of its systems, without the slowest one (the batch takes at
    /// least that long on any number of threads). Below it, the batch runs in order on the game's
    /// thread, where waking other threads would cost more than it saves.
    /// The default is
    /// [`DEFAULT_PARALLEL_THRESHOLD`](crate::schedule::DEFAULT_PARALLEL_THRESHOLD),
    /// from the benchmarks in the guide; `Duration::ZERO` sends every batch
    /// to the pool. It only changes speed, never results.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use std::time::Duration;
    /// let game = Game::new("t", 1, 1).parallel_systems(true).parallel_threshold(Duration::from_micros(200));
    /// # let _ = game;
    /// ```
    pub fn parallel_threshold(mut self, work: Duration) -> Self {
        self.parallel_threshold = Some(work);
        self.exec.invalidate();
        self
    }

    /// Whether the environment variable `PONKO_PARALLEL_SYSTEMS` may
    /// override [`parallel_systems`](Self::parallel_systems) and
    /// [`parallel_threshold`](Self::parallel_threshold) (it may, by
    /// default). For tests that compare a game with the scheduler off, on
    /// in order, and on threads: their settings must hold whatever the
    /// test run's environment says.
    #[doc(hidden)]
    pub fn parallel_systems_env(mut self, obeyed: bool) -> Self {
        self.parallel_env = obeyed;
        self.exec.invalidate();
        self
    }

    /// Puts a barrier into `phase`, after the systems added to it so far:
    /// the next system added to the phase starts a new stage, so it and
    /// everything after it see the applied commands and the events of
    /// everything before. Only matters with
    /// [`parallel_systems`](Self::parallel_systems) on, where it splits
    /// what would be one batch; without it, every system is already a
    /// stage. A barrier with no system after it does nothing.
    ///
    /// ```
    /// use ponko::access::SystemAccess;
    /// use ponko::prelude::*;
    /// use ponko::system_view::SystemView;
    /// #[derive(Component)]
    /// struct A;
    /// #[derive(Component)]
    /// struct B;
    /// let plan = Game::new("barrier", 1, 1)
    ///     .add(Phase::AfterPhysics, (|_: &mut SystemView| {}).access(SystemAccess::new().write::<A>()))
    ///     .barrier(Phase::AfterPhysics)
    ///     .add(Phase::AfterPhysics, (|_: &mut SystemView| {}).access(SystemAccess::new().write::<B>()))
    ///     .schedule_plan();
    /// assert_eq!(plan.stages_in(Phase::AfterPhysics).len(), 2);
    /// ```
    pub fn barrier(mut self, phase: Phase) -> Self {
        if !self.pending_barriers.contains(&phase) {
            self.pending_barriers.push(phase);
        }
        self
    }

    /// Says that `second` must see `first`'s work: `second` runs in a later
    /// stage than `first`, never in the same batch or ahead of it, even if
    /// their declarations don't conflict (for an order that the
    /// declarations can't show, such as two systems that talk through a
    /// file or a channel). With the scheduler on, a view system that
    /// doesn't interact with the systems registered before it may run
    /// ahead of them; this is how to stop that for a pair. `first` must already run before `second`: an
    /// earlier phase, or registered first in the same phase. Ids come from
    /// [`add_system`](Self::add_system); a removed system's pair is ignored.
    ///
    /// # Panics
    /// When the game starts (or its [`schedule_plan`](Self::schedule_plan)
    /// is made), if `second` runs before `first`. That is checked with the
    /// scheduler on or off, so turning it on can't be what makes a game
    /// panic.
    ///
    /// ```
    /// use ponko::access::SystemAccess;
    /// use ponko::prelude::*;
    /// use ponko::system_view::SystemView;
    /// #[derive(Component)]
    /// struct A;
    /// #[derive(Component)]
    /// struct B;
    /// let mut game = Game::new("order", 1, 1);
    /// let first = game.add_system(Phase::AfterPhysics, (|_: &mut SystemView| {}).access(SystemAccess::new().write::<A>()));
    /// let second = game.add_system(Phase::AfterPhysics, (|_: &mut SystemView| {}).access(SystemAccess::new().write::<B>()));
    /// assert_eq!(game.schedule_plan().stages_in(Phase::AfterPhysics).len(), 1);
    /// let game = game.order(first, second);
    /// assert_eq!(game.schedule_plan().stages_in(Phase::AfterPhysics).len(), 2);
    /// ```
    pub fn order(mut self, first: SystemId, second: SystemId) -> Self {
        self.order.push((first, second));
        self.exec.invalidate();
        self
    }

    /// The stages the systems run in with
    /// [`parallel_systems`](Self::parallel_systems) on: which systems share
    /// a batch, which run alone, and why each stage starts where it does.
    /// The plan doesn't depend on whether it is on. Print it to see why two
    /// systems don't run together.
    ///
    /// # Panics
    /// If an [`order`](Self::order) pair contradicts the run order.
    #[must_use]
    #[track_caller]
    pub fn schedule_plan(&self) -> crate::schedule::SchedulePlan {
        crate::schedule::plan(&self.plan_inputs(), &self.order)
    }

    /// Every system in run order, as the planner takes them.
    fn plan_inputs(&self) -> Vec<crate::schedule::PlanInput<'_>> {
        let lists = [&self.sim_systems, &self.systems, &self.late_systems, &self.draw_systems];
        (lists.into_iter().enumerate())
            .flat_map(|(list, systems)| {
                systems.iter().map(move |s| crate::schedule::PlanInput {
                    name: s.name,
                    // Late and draw systems report their list's phase
                    // (built-ins are added without setting `stage`).
                    phase: match list {
                        2 => Phase::Late,
                        3 => Phase::Draw,
                        _ => s.stage,
                    },
                    list: list as u8,
                    id: s.key,
                    access: s.access.as_ref(),
                    view: s.enforced,
                    // A batch runs every tick: an interval system stays alone.
                    interval: s.every > 1,
                    barrier: s.barrier,
                })
            })
            .collect()
    }

    /// Every system in run order with its declared access, and which
    /// declared systems in the same phase could run together.
    #[must_use]
    pub fn access_report(&self) -> crate::access::AccessReport {
        let systems = (self.sim_systems.iter())
            .chain(&self.systems)
            .chain(&self.late_systems)
            .chain(&self.draw_systems)
            .map(|s| crate::access::ReportedSystem {
                id: s.key,
                name: s.name,
                phase: s.stage,
                access: s.access.clone(),
                enforced: s.enforced,
            })
            .collect();
        crate::access::AccessReport::new(systems)
    }

    fn insert_system(&mut self, phase: Phase, mut s: System) -> SystemId {
        let id = s.key;
        s.stage = phase;
        // A pending barrier belongs to the next system the game adds, not
        // to a built-in that stays at the end of the phase.
        if !s.last
            && let Some(i) = self.pending_barriers.iter().position(|&p| p == phase)
        {
            self.pending_barriers.swap_remove(i);
            s.barrier = true;
        }
        self.exec.invalidate();
        let systems = match phase {
            Phase::Input | Phase::BeforePhysics | Phase::Physics | Phase::AfterPhysics | Phase::SimLast => {
                &mut self.sim_systems
            }
            Phase::Frame | Phase::Animate => &mut self.systems,
            Phase::Late => &mut self.late_systems,
            Phase::Draw => &mut self.draw_systems,
        };
        // At the end of the phase; before the built-ins that stay last in it.
        let last = s.last;
        let index = systems.partition_point(|x| x.stage < phase || (x.stage == phase && (last || !x.last)));
        systems.insert(index, s);
        id
    }

    /// Stores startup work on the builder. Configure systems and options first;
    /// the returned [`ConfiguredGame`] has plain `.run()` and
    /// `.run_headless(ticks)` methods, and accepts additional `.setup(...)` calls.
    /// Callbacks run once in order, after initialization and before systems.
    ///
    /// Runtime branches can choose different capturing closures without needing
    /// a common returned frame-closure type:
    ///
    /// ```
    /// use ponko::prelude::Game;
    /// let headless = true;
    /// let game = Game::new("Example", 800, 600);
    /// let game = if headless {
    ///     let label = String::from("server");
    ///     game.setup(move |ctx| ctx.world.insert_resource(label))
    /// } else {
    ///     let size = 42usize;
    ///     game.setup(move |ctx| ctx.world.insert_resource(size.to_string()))
    /// };
    /// let world = game.run_headless(0);
    /// assert_eq!(world.resource::<String>(), "server");
    /// ```
    pub fn setup(mut self, setup: impl FnOnce(&mut Ctx) + 'static) -> ConfiguredGame {
        self.setups.push(Box::new(setup));
        ConfiguredGame { game: self }
    }

    /// The game as it is, with the closure-free `.run()` and
    /// `.run_headless(ticks)` of [`ConfiguredGame`], for a game whose startup
    /// is all in its plugins and systems (or that needs none). It adds no
    /// setup; [`setup`](Self::setup) does the same with one.
    ///
    /// ```
    /// use ponko::prelude::{Ctx, Game, Plugin};
    /// struct Score(u32);
    /// struct Scoring;
    /// impl Plugin for Scoring {
    ///     fn build(&self, game: Game) -> Game {
    ///         game.sim_system(|ctx| ctx.world.resource_mut::<Score>().0 += 1)
    ///     }
    ///     fn setup(&self, ctx: &mut Ctx) {
    ///         ctx.world.insert_resource(Score(0));
    ///     }
    /// }
    /// let world = Game::new("Plugins only", 1, 1).plugin(Scoring).configured().run_headless(3);
    /// assert_eq!(world.resource::<Score>().0, 3);
    /// ```
    pub fn configured(self) -> ConfiguredGame {
        ConfiguredGame { game: self }
    }

    /// Composes an optional module. Its build runs now; its setup runs in
    /// registration order with the [`Game::setup`] callbacks, before the
    /// closure passed to `run`.
    pub fn plugin(self, plugin: impl Plugin + 'static) -> Self {
        let mut game = plugin.build(self);
        game.setups.push(Box::new(move |ctx| plugin.setup(ctx)));
        game
    }

    /// Removes all registrations with this ID, including built-ins. An absent
    /// or already removed ID is a no-op. Runtime event readers remain distinct
    /// even when multiple registrations share a built-in ID.
    pub fn remove(mut self, id: SystemId) -> Self {
        self.remove_id(id);
        self
    }

    /// The first system named `name`, in phase order, without building an
    /// [`AccessReport`](crate::access::AccessReport): for the C API, which
    /// removes systems by name (through `__ffi`).
    pub(crate) fn system_named(&self, name: &str) -> Option<SystemId> {
        (self.sim_systems.iter())
            .chain(&self.systems)
            .chain(&self.late_systems)
            .chain(&self.draw_systems)
            .find(|s| s.name == name)
            .map(|s| s.key)
    }

    fn remove_id(&mut self, id: SystemId) {
        self.exec.invalidate();
        self.sim_systems.retain(|s| s.key != id && s.builtin != Some(id));
        self.systems.retain(|s| s.key != id && s.builtin != Some(id));
        self.late_systems.retain(|s| s.key != id && s.builtin != Some(id));
        self.draw_systems.retain(|s| s.key != id && s.builtin != Some(id));
    }

    /// Plays over the network in deterministic lockstep: `net` from
    /// [`Net::host`](crate::net::Net::host), [`Net::join`](crate::net::Net::join) or
    /// [`Net::loopback`](crate::net::Net::loopback). Ticks then run only once
    /// every player's input for them is in, and the setup and simulation
    /// systems must be deterministic (same seed, no wall clock, no frame
    /// systems changing the world). Without it, `ctx.net.lockstep` is offline.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use std::thread;
    ///
    /// // Two players in one process; over the network it's ponko::net::Net::host / ponko::net::Net::join.
    /// let games: Vec<_> = ponko::net::Net::loopback(2)
    ///     .into_iter()
    ///     .map(|net| thread::spawn(move || {
    ///         let world = Game::new("t", 1, 1).lockstep(net).run_headless(30, |_| {});
    ///         world.len()
    ///     }))
    ///     .collect();
    /// for g in games {
    ///     assert_eq!(g.join().unwrap(), 0);
    /// }
    /// ```
    pub fn lockstep(mut self, net: crate::net::Net) -> Self {
        self.net = Some(net);
        self
    }

    /// Gives the game a byte-message endpoint: `net` from
    /// [`NetMessages::host`](crate::net::NetMessages::host),
    /// [`NetMessages::join`](crate::net::NetMessages::join) or
    /// [`NetMessages::loopback`](crate::net::NetMessages::loopback). It becomes
    /// [`ctx.net.messages`](crate::net::Networking::messages), and the engine
    /// polls it once per frame before any system runs (once per tick in
    /// headless runs), also while the simulation is paused. Systems send
    /// and read its events; they never call `poll`. See
    /// [`Networking`](crate::net::Networking) for a complete session.
    ///
    /// Independent of [`lockstep`](Self::lockstep): messages don't hold
    /// ticks back and the worlds aren't compared.
    ///
    /// ```
    /// use ponko::net::NetMessages;
    /// use ponko::prelude::*;
    ///
    /// let host = NetMessages::loopback(2).remove(0);
    /// let world = Game::new("t", 1, 1)
    ///     .messages(host)
    ///     .sim_system(|ctx: &mut Ctx| {
    ///         let messages = ctx.net.messages.as_mut().expect("the endpoint");
    ///         for event in messages.events() {
    ///             println!("{event:?}");
    ///         }
    ///     })
    ///     .run_headless(3, |_| {});
    /// # let _ = world;
    /// ```
    pub fn messages(mut self, net: crate::net::NetMessages) -> Self {
        self.messages = Some(net);
        self
    }

    /// Moves the sessions from the builder into `ctx.net`, before setup.
    fn start_networking(&mut self, ctx: &mut Ctx) {
        if let Some(net) = self.net.take() {
            ctx.net.lockstep = net;
        }
        if let Some(messages) = self.messages.take() {
            ctx.net.messages = Some(messages);
        }
        ctx.net.lockstep.begin(ctx.seed);
    }

    /// Adds a simulation system: it runs at the fixed rate
    /// ([`fixed_hz`](Self::fixed_hz), default 60 per second), zero or more
    /// times per frame, and `ctx.time.dt()` inside it is the fixed step.
    /// Put game logic here so it behaves the same at any frame rate.
    ///
    /// Systems run in `Phase::AfterPhysics`, in registration order: this is
    /// [`add`](Self::add)`(Phase::AfterPhysics, system)`. Use `add` for
    /// another phase, or to give the system a name, an interval or a
    /// declaration.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// fn gravity(ctx: &mut Ctx) { /* ... */ }
    /// fn movement(ctx: &mut Ctx) { /* ... */ }
    ///
    /// Game::new("Sim", 800, 600).sim_system(gravity).sim_system(movement).run(|_| {});
    /// ```
    pub fn sim_system<F: FnMut(&mut Ctx) + 'static>(self, system: F) -> Self {
        self.add(Phase::AfterPhysics, system)
    }

    /// How many registrations `id` names: 1 for an id from
    /// [`add_system`](Self::add_system) that is still registered, the number
    /// of instances for a [`Systems`] constant, 0 if there is none.
    ///
    /// ```
    /// use ponko::prelude::{Game, Systems};
    /// let game = Game::new("t", 1, 1);
    /// assert_eq!(game.system_count(Systems::DRAW_SPRITES), 1);
    /// assert_eq!(game.remove(Systems::DRAW_SPRITES).system_count(Systems::DRAW_SPRITES), 0);
    /// ```
    #[must_use]
    pub fn system_count(&self, id: SystemId) -> usize {
        (self.sim_systems.iter())
            .chain(&self.systems)
            .chain(&self.late_systems)
            .chain(&self.draw_systems)
            .filter(|s| s.key == id || s.builtin == Some(id))
            .count()
    }

    /// Turns on 2D physics (feature `physics`): every entity with a
    /// [`RigidBody`](crate::d2::RigidBody), a [`Collider`](crate::d2::Collider) and a
    /// [`d2::Transform`](crate::d2::Transform) is simulated each tick, pulled by
    /// `gravity` (pixels/s²; y is down). Collisions arrive as
    /// [`CollisionStarted`](crate::collision::CollisionStarted) events; `ctx.physics`
    /// casts rays and finds overlaps.
    ///
    /// The step is the built-in [`step_physics`](crate::systems::step_physics) sim
    /// system in [`Phase::Physics`], independent of builder call order.
    /// Use [`Phase::BeforePhysics`] for work that must run before the step.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut world = Game::new("Drop", 800, 600)
    ///     .physics(vec2(0.0, 980.0))
    ///     .run_headless(60, |ctx| {
    ///         ctx.world.spawn((ponko::d2::Transform::at(vec2(0.0, 0.0)), ponko::d2::RigidBody::dynamic(), ponko::d2::Collider::circle(8.0)));
    ///     });
    /// // One second of free fall: about 490 pixels down.
    /// let y = world.query::<&ponko::d2::Transform>().iter().next().unwrap().position.y;
    /// assert!((y - 490.0).abs() < 10.0, "{y}");
    /// ```
    #[cfg(feature = "physics")]
    pub fn physics(mut self, gravity: Vec2) -> Self {
        self.gravity = Some(gravity);
        if !self.sim_systems.iter().any(|s| s.builtin == Some(Systems::PHYSICS)) {
            self = self.add(
                Phase::Physics,
                crate::systems::step_physics.access(crate::systems::step_physics_access()),
            );
        }
        self
    }

    /// Turns on 3D physics (feature `physics3d`): every entity with a
    /// [`RigidBody3D`](crate::d3::RigidBody), a [`Collider3D`](crate::d3::Collider)
    /// and a [`d3::Transform`](crate::d3::Transform) is simulated each tick, and
    /// every [`Character3D`](crate::d3::Character) walks, pulled by `gravity`
    /// (m/s²; +Y is up, so `vec3(0.0, -9.81, 0.0)`). Collisions arrive as
    /// [`CollisionStarted`](crate::collision::CollisionStarted) events; `ctx.physics3d`
    /// casts rays and shapes and finds overlaps.
    ///
    /// The simulation runs at [`Physics::HZ`](crate::d3::Physics::HZ) (50)
    /// ticks per second unless [`fixed_hz`](Self::fixed_hz) chose a rate;
    /// in a window, moving bodies are drawn blended between ticks
    /// ([`interpolate_physics3d`](crate::systems::interpolate_physics3d), a
    /// `Frame` system). The step is the built-in
    /// [`step_physics3d`](crate::systems::step_physics3d) sim system, removable and
    /// registered in [`Phase::Physics`], independent of builder call order.
    /// Use [`Phase::BeforePhysics`] for work that must run before the step.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut world = Game::new("Drop", 800, 600)
    ///     .physics3d(vec3(0.0, -9.81, 0.0))
    ///     .run_headless(50, |ctx| {
    ///         ctx.world.spawn((ponko::d3::Transform::at(vec3(0.0, 10.0, 0.0)), ponko::d3::RigidBody::dynamic(), ponko::d3::Collider::sphere(0.5)));
    ///     });
    /// // One second of free fall: about 4.9 m down.
    /// let y = world.query::<&ponko::d3::Transform>().iter().next().unwrap().position.y;
    /// assert!((y - 5.1).abs() < 0.2, "{y}");
    /// ```
    #[cfg(feature = "physics3d")]
    pub fn physics3d(mut self, gravity: glam::Vec3) -> Self {
        self.gravity3d = Some(gravity);
        if !self.fixed_hz_set {
            self.fixed_hz = crate::physics3d::PHYSICS3D_HZ;
        }
        if !self.sim_systems.iter().any(|s| s.builtin == Some(Systems::PHYSICS3D)) {
            self = self.add(
                Phase::Physics,
                crate::systems::step_physics3d.access(crate::systems::step_physics3d_access()),
            );
        }
        if !self
            .systems
            .iter()
            .any(|s| s.builtin == Some(Systems::INTERPOLATE_PHYSICS3D))
        {
            self = self.add(
                Phase::Frame,
                crate::systems::interpolate_physics3d.access(crate::systems::interpolate_physics3d_access()),
            );
        }
        self
    }

    /// Turns on the built-in [`SpatialGrid`](crate::spatial::SpatialGrid) with square
    /// cells of `cell_size` world units: at the start of every simulation
    /// tick, all entities with a [`d2::Transform`](crate::d2::Transform) are
    /// indexed into `ctx.spatial` for radius, rectangle and nearest queries.
    ///
    /// Choose a cell size near your usual query radius. The rebuild is the
    /// built-in [`rebuild_spatial_grid`] system, removable like any other.
    ///
    /// # Panics
    /// If `cell_size` isn't a positive, finite number.
    #[track_caller]
    pub fn spatial_grid(mut self, cell_size: f32) -> Self {
        self.spatial_cell = Some(crate::spatial::SpatialGrid::new(cell_size).cell_size());
        if !self
            .sim_systems
            .iter()
            .any(|s| s.builtin == Some(Systems::REBUILD_SPATIAL_GRID))
        {
            self = self.add(Phase::Input, rebuild_spatial_grid);
        }
        self
    }

    /// Adds a frame system: it runs once per rendered frame, after the
    /// simulation systems. Use it for input, camera and drawing. This is
    /// [`add`](Self::add)`(Phase::Frame, system)`; frame systems do not run
    /// in `run_headless`.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// fn quit_on_escape(ctx: &mut Ctx) {
    ///     if ctx.input.key_pressed(Key::Escape) { ctx.quit(); }
    /// }
    ///
    /// Game::new("Sim", 800, 600).system(quit_on_escape).run(|_| {});
    /// ```
    pub fn system<F: FnMut(&mut Ctx) + 'static>(self, system: F) -> Self {
        self.add(Phase::Frame, system)
    }

    /// Feeds scheduled input into the game by simulation tick, for
    /// reproducible tests (off unless called). Before the simulation systems
    /// of each tick run, the scenario's steps for that tick go through
    /// [`Input::feed`](crate::input::Input::feed), so [named actions](crate::actions::ActionMap) and raw input
    /// queries see them exactly like live input. Calling it again replaces
    /// the previous scenario.
    ///
    /// Check steps run after the tick's systems, and the outcome is the
    /// [`ScenarioReport`](crate::scenario::ScenarioReport) resource; see
    /// [`run_headless_scenario`](Self::run_headless_scenario) for a runner.
    ///
    /// Edges last until the end of the frame. Headless runs have one tick per
    /// frame, so a press is visible to exactly one tick. A windowed frame that
    /// runs several ticks shows it to the rest of that frame's ticks, as with
    /// live input. Live input is still read alongside the scenario.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// #[derive(Component)]
    /// struct Jumps(u32);
    ///
    /// fn jump(ctx: &mut Ctx) {
    ///     if ctx.input.action_pressed("jump") {
    ///         ctx.world.query::<&mut Jumps>().for_each(|j| j.0 += 1);
    ///     }
    /// }
    ///
    /// let scenario = ponko::scenario::InputScenario::new()
    ///     .at(3, ponko::actions::InputEvent::Key(Key::Space, true))
    ///     .at(6, ponko::actions::InputEvent::Key(Key::Space, false));
    /// let mut world = Game::new("test", 1, 1)
    ///     .sim_system(jump)
    ///     .input_scenario(scenario)
    ///     .run_headless(10, |ctx| {
    ///         let mut actions = ActionMap::default();
    ///         actions.set("jump", Action::Button(vec![ButtonBinding::Key(Key::Space)]));
    ///         ctx.input.set_actions(actions);
    ///         ctx.world.spawn((Jumps(0),));
    ///     });
    /// assert_eq!(world.query::<&Jumps>().iter().next().unwrap().0, 1);
    /// ```
    pub fn input_scenario(mut self, scenario: crate::scenario::InputScenario) -> Self {
        self.input_scenario = Some(scenario);
        self
    }

    /// Makes headless runs ([`run_headless`](Self::run_headless),
    /// [`run_headless_scenario`](Self::run_headless_scenario),
    /// [`profile_headless`](Self::profile_headless) and
    /// [`ConfiguredGame`]'s) run whole frames instead of simulation ticks
    /// alone (off by default). Each frame is one tick, in the windowed
    /// order: the retained UI takes the frame's input, the simulation tick
    /// runs, then the frame, `Animate`, `Late` and `Draw` systems and the
    /// closure `setup` returned. The window is virtual: `ctx.window_size()`
    /// is the size given to [`Game::new`], and nothing is drawn (draw calls
    /// are dropped) or played. No GPU or window is opened.
    ///
    /// With it, a test can drive a game's menus: an
    /// [`InputScenario`](crate::scenario::InputScenario) feeds each tick's
    /// input before the UI takes it, clicks and types into the UI by node
    /// name ([`click`](crate::scenario::InputScenario::click),
    /// [`type_text`](crate::scenario::InputScenario::type_text)), and checks
    /// the UI tree ([`ScenarioCheck::ui`](crate::scenario::ScenarioCheck::ui))
    /// after each frame's systems ran. Off, a headless run costs what it did
    /// before: no UI, no frame systems.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::scenario::{InputScenario, ScenarioCheck};
    ///
    /// struct Menu { play: ponko::ui::UiId }
    /// #[derive(Default)]
    /// struct Started(bool);
    ///
    /// fn menu(ctx: &mut Ctx) {
    ///     let play = ctx.world.resource::<Menu>().play;
    ///     if ctx.ui.events().any(|e| e.clicked() == Some(play)) {
    ///         ctx.world.resource_mut::<Started>().0 = true;
    ///         ctx.ui.pop_screen();
    ///     }
    /// }
    ///
    /// let scenario = InputScenario::new()
    ///     .expect(0, ScenarioCheck::ui("menu.play").shown(true).text("Play"))
    ///     .click(2, "menu.play")
    ///     .expect(2, ScenarioCheck::ui("menu.play").shown(false));
    /// let (world, report) = Game::new("menu", 640, 480)
    ///     .headless_frames(true)
    ///     .system(menu)
    ///     .run_headless_scenario(scenario, |ctx| {
    ///         let root = ctx.ui.root().layout(ponko::ui::UiLayout::fill()).id();
    ///         let play = ctx.ui.button(root, "Play").name("menu.play").id();
    ///         ctx.ui.push_screen(root, false);
    ///         ctx.world.insert_resource(Menu { play });
    ///         ctx.world.insert_resource(Started::default());
    ///     });
    /// report.assert_passed();
    /// assert!(world.resource::<Started>().0);
    /// ```
    pub fn headless_frames(mut self, on: bool) -> Self {
        self.headless_frames = on;
        self
    }

    /// Runs `scenario` headlessly for its [`ticks`](crate::scenario::InputScenario::ticks)
    /// and returns the world with the scenario's report: the bounded runner
    /// for tests and CI. A failed check ends the run at that tick and the
    /// report names its step and tick; [`ScenarioReport::assert_passed`](crate::scenario::ScenarioReport::assert_passed)
    /// turns that into a panic.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// let scenario = ponko::scenario::InputScenario::new()
    ///     .at(2, ponko::actions::InputEvent::Key(Key::Space, true))
    ///     .expect(2, ponko::scenario::ScenarioCheck::action("jump").pressed(true))
    ///     .expect(3, ponko::scenario::ScenarioCheck::action("jump").pressed(true)); // deliberately wrong
    /// let (_world, report) = Game::new("test", 1, 1).run_headless_scenario(scenario, |ctx| {
    ///     let mut actions = ActionMap::default();
    ///     actions.set("jump", Action::Button(vec![ButtonBinding::Key(Key::Space)]));
    ///     ctx.input.set_actions(actions);
    /// });
    /// let failure = report.failure.unwrap();
    /// assert_eq!((failure.step, failure.tick), (2, 3));
    /// ```
    pub fn run_headless_scenario<S, U>(
        self,
        scenario: crate::scenario::InputScenario,
        setup: S,
    ) -> (World, crate::scenario::ScenarioReport)
    where
        S: FnOnce(&mut Ctx) -> U,
        U: SetupResult,
    {
        let ticks = scenario.ticks();
        let pending = crate::scenario::ScenarioReport::new(&scenario, 0);
        let world = self.input_scenario(scenario).run_headless(ticks, setup);
        let report = world.get_resource::<crate::scenario::ScenarioReport>().cloned();
        (world, report.unwrap_or(pending))
    }

    /// Lets tools start input scenarios through the [control endpoint](Self::control_endpoint)
    /// (the `input.scenario.*` methods and MCP's `run_input_scenario`). Off
    /// by default, so a game with the endpoint open for inspection still
    /// refuses injected input; setting `PONKO_INPUT_INJECTION=1` has the same
    /// effect without code changes (the MCP `launch` tool sets it). Scenarios
    /// installed with [`input_scenario`](Self::input_scenario) don't need it.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Dev build", 800, 600).control_endpoint(true).input_injection(true).run(|_| |_: &mut Ctx| {});
    /// ```
    pub fn input_injection(mut self, on: bool) -> Self {
        self.input_injection = on;
        self
    }

    /// Opens the control endpoint, so tools such as the `ponko-mcp` agent
    /// bridge can inspect and drive this game while it runs: list and edit
    /// entities (through [reflection](crate::reflect::WorldReflect)), pause, step,
    /// change speed, read system timings.
    ///
    /// It listens on `127.0.0.1` only, and every request must carry a random
    /// token written to the instance registry (`~/.ponko/instances`),
    /// which only your user can read. Off by default; setting the
    /// environment variable `PONKO_CONTROL=1` turns it on without code
    /// changes in a debug build, or in a release build with the
    /// `control-env` feature (a shipped release build without it ignores
    /// the variable). `PONKO_START_PAUSED=1` starts the simulation paused.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Colony", 1280, 720).control_endpoint(true).run(|_| {});
    /// ```
    pub fn control_endpoint(mut self, on: bool) -> Self {
        self.control = on;
        self
    }

    /// Reload textures, fonts, sounds and data files ([`Assets::load_resource`](crate::assets::Assets::load_resource))
    /// from `assets/` when their files change, under the same handles: save an
    /// image or a JSON file in your editor and the running game shows it
    /// within a quarter of a second. On by default in debug builds, off in
    /// release builds; `PONKO_HOT_RELOAD=1` or `0` overrides both.
    ///
    /// A file is checked a few times a second (one `stat` per loaded
    /// texture). An image that changed size isn't reloaded (sprites store
    /// their texture's size): restart the game for that.
    /// [`Assets::reloaded_textures`](crate::assets::Assets::reloaded_textures) lists what was reloaded this frame.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Art test", 1280, 720).hot_reload(true).run(|ctx| {
    ///     let crab = ctx.gfx.load_texture("crab.png"); // edit crab.png while it runs
    ///     ctx.world.spawn((ponko::d2::Transform::at(vec2(640.0, 360.0)), ponko::d2::Sprite::new(crab)));
    /// });
    /// ```
    pub fn hot_reload(mut self, on: bool) -> Self {
        self.hot_reload = on;
        self
    }

    /// Turns the runtime inspector on or off: an overlay that the backquote
    /// key (`` ` ``, left of 1) opens and closes,
    /// showing every entity and its components (reflected ones editable),
    /// reflected resources, system times, and pause / step / speed controls.
    /// Its "Pick" button selects the unit you click next.
    ///
    /// On by default in debug builds, off in release builds. The
    /// environment variable `PONKO_INSPECTOR=1` (or `0`) overrides this, and
    /// `PONKO_INSPECTOR=open` also starts the game with it open.
    /// Needs the `inspector` cargo feature (on by default); without it, this
    /// does nothing. While closed it costs nothing.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// // Ship a release build with the inspector, for playtesters.
    /// Game::new("Playtest", 1280, 720).inspector(true).run(|ctx| {
    ///     let _ = ctx;
    /// });
    /// ```
    pub fn inspector(mut self, on: bool) -> Self {
        self.inspector = on;
        self
    }

    /// Constructs the context both loops run: the services from the game's
    /// configuration (seed, hot reload, spatial grid, physics worlds, the
    /// lockstep session and message endpoint), before setup and before the control endpoint
    /// starts. `headless` is a run without a window or GPU.
    ///
    /// Window and headless runs differ only in what `ctx.gfx` and
    /// `ctx.audio` do with a missing device; every other service is built
    /// the same way in both.
    fn build_ctx(&mut self, headless: bool) -> Ctx {
        let fixed_dt = Clock::new(self.fixed_hz).fixed_dt();
        let mut ctx = Ctx::new(fixed_dt, Vec2::new(self.width as f32, self.height as f32));
        ctx.seed = self.seed;
        if headless {
            ctx.set_headless(true);
        }
        ctx.assets.watcher.set_on(crate::hot_reload::enabled(self.hot_reload));
        if let Some(cell) = self.spatial_cell {
            ctx.spatial = crate::spatial::SpatialGrid::new(cell);
        }
        #[cfg(feature = "physics")]
        if let Some(gravity) = self.gravity {
            crate::physics::start(&mut ctx, gravity);
        }
        #[cfg(feature = "physics3d")]
        if let Some(gravity) = self.gravity3d {
            crate::physics3d::start(&mut ctx, gravity);
        }
        self.start_networking(&mut ctx);
        ctx.storage = crate::storage::Storage::for_game(self.app_id.as_deref(), self.data_dir.clone());
        ctx
    }

    /// Starts the control endpoint if it's enabled, and applies `PONKO_START_PAUSED`.
    fn start_control(&self, ctx: &mut Ctx, headless: bool) -> Option<Control> {
        if control::env_flag(control::ENV_START_PAUSED) {
            ctx.time.pause();
        }
        ctx.input.scenarios.injection = self.input_injection || control::env_flag(control::ENV_INPUT_INJECTION);
        if !self.control {
            return None;
        }
        if cfg!(target_arch = "wasm32") {
            eprintln!("ponko: the control endpoint isn't available in web builds");
            return None;
        }
        match Control::start(&self.title, headless) {
            Ok(c) => {
                ctx.control_addr = Some(c.addr());
                Some(c)
            }
            Err(e) => {
                eprintln!("ponko: could not open the control endpoint: {e}");
                None
            }
        }
    }

    /// Names the game for [`ctx.storage`](crate::ctx::Ctx::storage): its files
    /// go in the folder `app_id` inside the player's data folder (Windows
    /// `%APPDATA%`, macOS `~/Library/Application Support`, Linux
    /// `$XDG_DATA_HOME` or `~/.local/share`), or in the browser database
    /// `ponko:<app_id>` on the web. Without one, the game keeps no files.
    ///
    /// Pick it once and keep it: a new id is a new folder, and the player's
    /// old saves stay in the old one. A reverse domain (`"com.example.crabs"`)
    /// or a plain name (`"space-crab"`) both work.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let game = Game::new("Space Crab", 1280, 720).app_id("space-crab");
    /// ```
    ///
    /// # Panics
    ///
    /// If `app_id` is empty, starts with a `.`, or has characters other
    /// than letters, digits, `-`, `_` and `.`.
    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        let app_id = app_id.into();
        assert!(
            crate::storage::is_app_id(&app_id),
            "ponko: app id {app_id:?} must be 1 to 128 letters, digits, '-', '_' or '.', not starting with '.'"
        );
        self.app_id = Some(app_id);
        self
    }

    /// Keeps [`ctx.storage`](crate::ctx::Ctx::storage)'s files in `dir`
    /// itself instead of the player's data folder: for tests, and for
    /// portable installs that keep saves beside the game. Wins over
    /// [`app_id`](Self::app_id)'s folder and `PONKO_DATA_DIR`; the web build
    /// has no folders and ignores it.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let dir = std::env::temp_dir().join("crab-test-saves");
    /// Game::new("Space Crab", 1280, 720).data_dir(&dir).run_headless(0, |ctx| {
    ///     assert_eq!(ctx.storage.dir(), Some(dir.as_path()));
    /// });
    /// ```
    pub fn data_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }

    /// Sets the seed for [`Ctx::random`]. Same seed, same simulation: runs
    /// are reproducible by default ([`DEFAULT_SEED`](crate::random::DEFAULT_SEED)).
    /// For a different world each run, seed from the clock.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    /// Game::new("Roguelike", 800, 600).seed(seed).run(|_| {});
    /// ```
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Sets how many fixed updates run per second (default 60).
    /// See [`Time::fixed_steps`](crate::time::Time::fixed_steps).
    ///
    /// # Panics
    /// If `hz` is 0.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Physics", 800, 600).fixed_hz(120).run(|_| |_: &mut Ctx| {});
    /// ```
    pub fn fixed_hz(mut self, hz: u32) -> Self {
        assert!(hz > 0, "Game::fixed_hz: the rate must be at least 1 Hz");
        self.fixed_hz = hz;
        self.fixed_hz_set = true;
        self
    }

    /// Turns vsync on (default) or off. With vsync the game runs at the
    /// display's refresh rate with no tearing; turn it off to measure how fast
    /// the game *could* run.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Benchmark", 800, 600).vsync(false).run(|_| |_: &mut Ctx| {});
    /// ```
    pub fn vsync(mut self, on: bool) -> Self {
        self.vsync = on;
        self
    }

    /// Fills the screen with the game instead of opening a window (off by
    /// default). On macOS, windowed games are shown at most once per display
    /// refresh even with vsync off; fullscreen ones aren't. Web builds
    /// ignore it.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Benchmark", 1280, 720).fullscreen(true).vsync(false).run(|_| |_: &mut Ctx| {});
    /// ```
    pub fn fullscreen(mut self, on: bool) -> Self {
        self.fullscreen = on;
        self
    }

    /// One frame of game logic: simulation systems `fixed_steps` times, then
    /// frame, late and draw systems. The retained UI updates first
    /// (`Ctx::frame_begin_services`); every other engine step of a frame is
    /// a system: animators and particles end `Animate`, positional audio ends
    /// `Draw`. Commands are applied after every system (after every batch
    /// with `parallel_systems`).
    fn run_systems(&mut self, ctx: &mut Ctx, fixed_steps: u32) {
        self.ensure_schedule();
        ctx.net.begin_frame();
        ctx.frame_begin_services();
        self.begin_timings(ctx);
        ctx.set_in_sim(true);
        for _ in 0..fixed_steps {
            // Lockstep: a tick waits for everyone's input; the frame goes on.
            if !ctx.net.lockstep.ready(ctx.time.sim_tick()) {
                break;
            }
            self.run_sim_tick(ctx);
        }
        ctx.set_in_sim(false);
        let n_sim = self.sim_systems.len();
        let n_frame = self.systems.len();
        let n_late = self.late_systems.len();
        self.exec.run_list(&mut self.systems, 1, ctx, n_sim, None);
        self.exec
            .run_list(&mut self.late_systems, 2, ctx, n_sim + n_frame, None);
        self.exec
            .run_list(&mut self.draw_systems, 3, ctx, n_sim + n_frame + n_late, None);
        end_timings(ctx);
        let frame = self.systems.iter().chain(&self.late_systems).chain(&self.draw_systems);
        forget_seen_removals(ctx, self.sim_systems.iter().chain(frame));
    }

    /// One simulation tick: every sim system once, then the tick counter.
    fn run_sim_tick(&mut self, ctx: &mut Ctx) {
        self.ensure_schedule();
        let tick = ctx.time.sim_tick();
        self.install_input_scenario(ctx);
        ctx.scenario_begin_tick();
        self.exec.run_list(&mut self.sim_systems, 0, ctx, 0, Some(tick));
        ctx.set_sim_steps(1);
        ctx.scenario_end_tick(tick);
        ctx.net.lockstep.end_tick(&ctx.world, tick);
        ctx.advance_tick();
    }

    /// The scenario from [`input_scenario`](Self::input_scenario), started
    /// on the first tick of the run.
    fn install_input_scenario(&mut self, ctx: &mut Ctx) {
        if let Some(scenario) = self.input_scenario.take() {
            ctx.input.install_scenario(
                &mut ctx.world,
                &mut ctx.time,
                scenario,
                0,
                crate::scenario::RunOrigin::default(),
            );
        }
    }

    /// One frame of a [`headless_frames`](Self::headless_frames) run: the
    /// windowed frame of `run_systems` with one tick. The scenario's input
    /// for the tick comes first, as a window's input arrives before its
    /// frame, so the UI takes it too; its checks run after the whole frame.
    fn run_headless_frame(&mut self, ctx: &mut Ctx) {
        self.ensure_schedule();
        let tick = ctx.time.sim_tick();
        self.install_input_scenario(ctx);
        ctx.scenario_begin_tick();
        ctx.frame_begin_services();
        self.begin_timings(ctx);
        ctx.set_in_sim(true);
        self.exec.run_list(&mut self.sim_systems, 0, ctx, 0, Some(tick));
        ctx.set_sim_steps(1);
        ctx.net.lockstep.end_tick(&ctx.world, tick);
        ctx.advance_tick();
        ctx.set_in_sim(false);
        let n_sim = self.sim_systems.len();
        let n_frame = self.systems.len();
        let n_late = self.late_systems.len();
        self.exec.run_list(&mut self.systems, 1, ctx, n_sim, None);
        self.exec
            .run_list(&mut self.late_systems, 2, ctx, n_sim + n_frame, None);
        self.exec
            .run_list(&mut self.draw_systems, 3, ctx, n_sim + n_frame + n_late, None);
        end_timings(ctx);
        let frame = self.systems.iter().chain(&self.late_systems).chain(&self.draw_systems);
        forget_seen_removals(ctx, self.sim_systems.iter().chain(frame));
        ctx.scenario_end_tick(tick);
    }

    /// Adds the closure a `setup` returned as the last frame system.
    fn add_setup_closure(&mut self, f: SystemFn) {
        let index = self.systems.partition_point(|s| s.stage <= Phase::Frame);
        let id = next_system_id();
        self.systems.insert(
            index,
            System {
                key: SystemId(id),
                builtin: None,
                name: "setup closure",
                stage: Phase::Frame,
                run: Run::Ctx(f),
                every: 1,
                phase: 0,
                id,
                access: None,
                last: false,
                changes_seen: Default::default(),
                enforced: false,
                barrier: false,
            },
        );
    }

    /// Checks the [`order`](Self::order) pairs against the run order: the
    /// error names the first pair whose second system already runs before
    /// its first. The game panics with that error when it starts (and
    /// [`schedule_plan`](Self::schedule_plan) when it is made); call this to
    /// refuse earlier, as language bindings do.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut game = Game::new("order", 1, 1);
    /// let first = game.add_system(Phase::AfterPhysics, (|_: &mut Ctx| {}).named("first"));
    /// let second = game.add_system(Phase::AfterPhysics, (|_: &mut Ctx| {}).named("second"));
    /// let game = game.order(second, first);
    /// let error = game.check_order().unwrap_err();
    /// assert_eq!((error.first, error.second), ("second", "first"));
    /// ```
    ///
    /// # Errors
    /// An [`OrderError`](crate::schedule::OrderError) for the first such pair.
    pub fn check_order(&self) -> Result<(), crate::schedule::OrderError> {
        let lists = [&self.sim_systems, &self.systems, &self.late_systems, &self.draw_systems];
        let systems = lists.into_iter().flat_map(|l| l.iter().map(|s| (s.key, s.name)));
        crate::schedule::check_order(systems, &self.order)
    }

    /// Builds the stages the systems run in, if the systems or the settings
    /// changed since they were built.
    fn ensure_schedule(&mut self) {
        let lens = [
            self.sim_systems.len(),
            self.systems.len(),
            self.late_systems.len(),
            self.draw_systems.len(),
        ];
        if self.exec.is_current(lens) {
            return;
        }
        let settings = batch::Settings::new(self.parallel, self.parallel_threshold, self.parallel_env);
        // Checked on or off: a wrong `order` must not wait for the day the
        // scheduler is turned on (which the environment can do).
        if let Err(problem) = self.check_order() {
            panic!("{problem}");
        }
        let plan = settings.on.then(|| self.schedule_plan());
        let lists = [
            &self.sim_systems[..],
            &self.systems[..],
            &self.late_systems[..],
            &self.draw_systems[..],
        ];
        self.exec.build(settings, plan.as_ref(), lists);
    }

    /// Each system's phase and interval, in timing order, for diagnostics.
    /// Late and draw systems report their list's phase (bindings add them
    /// without setting `stage`).
    fn schedule_slots(&self) -> Vec<crate::diagnostics::ScheduleSlot> {
        let slot = |s: &System, phase| crate::diagnostics::ScheduleSlot {
            phase,
            every: s.every,
            offset: s.phase,
        };
        (self.sim_systems.iter().map(|s| slot(s, s.stage)))
            .chain(self.systems.iter().map(|s| slot(s, s.stage)))
            .chain(self.late_systems.iter().map(|s| slot(s, Phase::Late)))
            .chain(self.draw_systems.iter().map(|s| slot(s, Phase::Draw)))
            .collect()
    }

    /// What `Ctx::ecs_diagnostics` builds the schedule plan from: the
    /// planner's inputs, owned. Made when the systems are listed, not per
    /// frame; the plan itself is only built when diagnostics are asked for.
    fn plan_source(&self) -> crate::diagnostics::PlanSource {
        let settings = batch::Settings::new(self.parallel, self.parallel_threshold, self.parallel_env);
        crate::diagnostics::PlanSource {
            systems: (self.plan_inputs().into_iter())
                .map(|s| crate::diagnostics::PlanSystem {
                    name: s.name,
                    phase: s.phase,
                    list: s.list,
                    id: s.id,
                    access: s.access.cloned(),
                    view: s.view,
                    interval: s.interval,
                    barrier: s.barrier,
                })
                .collect(),
            order: self.order.clone(),
            parallel: settings.on,
            threads: settings.threads(),
            threshold_us: settings.threshold_us(),
        }
    }

    /// Makes sure there's one timing slot per system, and zeroes this frame's times.
    fn begin_timings(&self, ctx: &mut Ctx) {
        if ctx.perf.timings.len()
            != self.sim_systems.len() + self.systems.len() + self.late_systems.len() + self.draw_systems.len()
        {
            ctx.perf.timings = (self.sim_systems.iter().map(|s| (s, true)))
                .chain(
                    self.systems
                        .iter()
                        .chain(&self.late_systems)
                        .chain(&self.draw_systems)
                        .map(|s| (s, false)),
                )
                .map(|(s, sim)| SystemTiming {
                    name: s.name,
                    sim,
                    last_ms: 0.0,
                    avg_ms: 0.0,
                })
                .collect();
            ctx.perf.schedule = self.schedule_slots();
            ctx.perf.plan_source = self.plan_source();
            ctx.perf.dispatch.clear();
            ctx.perf
                .dispatch
                .resize(ctx.perf.timings.len(), crate::diagnostics::Dispatch::default());
        }
        for t in &mut ctx.perf.timings {
            t.last_ms = 0.0;
        }
        ctx.sync_profiling();
        ctx.perf.state.begin_frame(ctx.perf.timings.len());
        ctx.events.keep_ticks = self.sim_systems.iter().map(|s| s.every).max().unwrap_or(1);
    }

    /// Runs the simulation without a window or GPU, as fast as possible, and
    /// returns the world when done. For dedicated servers, batch runs,
    /// benchmarks, and testing game logic in plain `cargo test`.
    ///
    /// `setup` runs once, then `ticks` simulation ticks run (fewer if a
    /// system calls `ctx.quit()`). Only **simulation systems** run: frame
    /// systems, the closure returned by `setup`, and drawing need frames, and
    /// there are none. Sim speed is ignored, and so is
    /// [`ctx.time.pause()`](crate::time::Time::pause) unless the
    /// [control endpoint](Self::control_endpoint) is on: then a paused game
    /// waits for tools to step or resume it. Without it every tick runs, so
    /// a game that pauses behind a menu still sees the scenario input that
    /// closes it. To hold only physics (a pause screen whose sim systems keep
    /// running, in a window and headless alike), use
    /// [`Physics::set_paused`](crate::d2::Physics::set_paused). Textures
    /// still load (for their sizes), but nothing is uploaded to a GPU.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// #[derive(Component)]
    /// struct Pos(f32);
    ///
    /// fn fall(ctx: &mut Ctx) {
    ///     let dt = ctx.time.dt();
    ///     ctx.world.query::<&mut Pos>().for_each(|p| p.0 -= 10.0 * dt);
    /// }
    ///
    /// let mut world = Game::new("test", 800, 600)
    ///     .sim_system(fall)
    ///     .run_headless(60, |ctx| {
    ///         ctx.world.spawn((Pos(100.0),));
    ///     });
    ///
    /// // One simulated second at 10 units/s.
    /// let y = world.query::<&Pos>().iter().next().unwrap().0;
    /// assert!((y - 90.0).abs() < 1e-3);
    /// ```
    ///
    /// For a game with systems, put the startup code on the builder with
    /// [`setup`](Self::setup) and call [`ConfiguredGame::run_headless`],
    /// which takes no closure. This form remains for short tests, and for a
    /// setup that borrows a local variable, which a stored callback can't.
    pub fn run_headless<S, U>(self, ticks: u64, setup: S) -> World
    where
        S: FnOnce(&mut Ctx) -> U,
        U: SetupResult,
    {
        self.run_headless_inner(ticks, None, setup).0
    }

    /// Like [`run_headless`](Self::run_headless), but times everything and
    /// returns a [`PerfReport`](crate::perf::PerfReport) next to the world: where
    /// the time goes (systems, [`profile_scope!`](crate::profile_scope)
    /// scopes), allocations, the slowest ticks, and warnings with advice.
    /// The first `warmup` ticks run but aren't measured (caches fill, buffers
    /// grow); the report covers the next `ticks` ticks (at most the last
    /// 50,000). Call it from a test or a CI job; see [`PerfReport::assert_within`](crate::perf::PerfReport::assert_within).
    ///
    /// Counting allocations needs [`CountingAllocator`](crate::perf::CountingAllocator)
    /// as the global allocator.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// fn movement(ctx: &mut Ctx) {
    ///     profile_scope!("movement/integrate");
    ///     ctx.world.query::<&mut ponko::d2::Transform>().for_each(|t| t.position.x += 1.0);
    /// }
    ///
    /// let (_world, report) = Game::new("bench", 1, 1)
    ///     .sim_system(movement)
    ///     .profile_headless(10, 100, |ctx| {
    ///         for _ in 0..1000 {
    ///             ctx.world.spawn((ponko::d2::Transform::default(),));
    ///         }
    ///     });
    /// assert_eq!((report.frames, report.entities), (100, 1000));
    /// // Timings vary between machines: keep limits generous.
    /// report.assert_within(&ponko::perf::Budget::new().frame_p99_ms(100.0));
    /// ```
    pub fn profile_headless<S, U>(self, warmup: u64, ticks: u64, setup: S) -> (World, crate::perf::PerfReport)
    where
        S: FnOnce(&mut Ctx) -> U,
        U: SetupResult,
    {
        let (world, report) = self.run_headless_inner(ticks, Some(warmup), setup);
        (world, report.unwrap_or_default())
    }

    fn run_headless_inner<S, U>(
        mut self,
        ticks: u64,
        warmup: Option<u64>,
        setup: S,
    ) -> (World, Option<crate::perf::PerfReport>)
    where
        S: FnOnce(&mut Ctx) -> U,
        U: SetupResult,
    {
        connect_hotpatch();
        let fixed_dt = Clock::new(self.fixed_hz).fixed_dt();
        let mut ctx = self.build_ctx(true);
        let control = self.start_control(&mut ctx, true);
        ctx.set_in_setup(true);
        run_setups(std::mem::take(&mut self.setups), &mut ctx);
        let frame_fn = setup(&mut ctx).into_frame_fn();
        match frame_fn {
            Some(f) if self.headless_frames => self.add_setup_closure(f),
            other => drop(other), // no frames, so no frame closure
        }
        ctx.set_in_setup(false);
        ctx.apply_commands();
        ctx.gfx.discard_draws();

        let timing = FrameTiming {
            dt: fixed_dt as f32,
            fixed_steps: 1,
            alpha: 0.0,
        };
        if warmup.is_some() {
            ctx.perf.state.collector = crate::perf_report::Collector::new(crate::perf_report::HEADLESS_HISTORY);
            ctx.perf.set_profiling(true);
        }
        let total = ticks + warmup.unwrap_or(0);
        let mut ran = 0;
        // Lists the systems before the first tick, for tools asking a paused game.
        self.begin_timings(&mut ctx);
        while ran < total && !ctx.quit_requested() {
            // Messages are polled every pass: while paused, and while
            // lockstep waits for inputs.
            ctx.net.begin_frame();
            if let Some(control) = &control {
                control.poll(&mut ctx);
                if ctx.quit_requested() {
                    break;
                }
                // A paused game runs only the ticks tools ask for.
                if ctx.time.is_paused() && !ctx.take_one_step() {
                    control.wait(&mut ctx, Duration::from_millis(50));
                    continue;
                }
            }
            if !ctx.net.lockstep.ready(ctx.time.sim_tick()) {
                if !ctx.net.lockstep.wait() {
                    break;
                }
                continue;
            }
            ran += 1;
            let tick_start = Instant::now();
            ctx.begin_frame(timing);
            ctx.frame_prepare_services();
            if self.headless_frames {
                self.run_headless_frame(&mut ctx);
            } else {
                self.begin_timings(&mut ctx);
                ctx.set_in_sim(true);
                self.run_sim_tick(&mut ctx);
                ctx.set_in_sim(false);
                end_timings(&mut ctx);
                // Only simulation systems run here, so only they hold removals back.
                forget_seen_removals(&mut ctx, self.sim_systems.iter());
            }
            ctx.gfx.discard_draws();
            ctx.input.end_frame();
            if ctx.perf.state.on {
                ctx.end_perf_frame(tick_start.elapsed().as_secs_f32() * 1000.0);
                if warmup == Some(ran) {
                    ctx.perf.state.collector.clear();
                }
            }
        }
        let report = warmup.map(|_| ctx.perf.report());
        (std::mem::take(&mut ctx.world), report)
    }

    /// Opens the window and runs the game until it is closed.
    ///
    /// `setup` runs once, after the window exists. It can spawn entities into
    /// `ctx.world`, and it can return a closure that runs every frame (after
    /// the systems). Variables created in `setup` and moved into that closure
    /// are your game state; no structs or traits required.
    ///
    /// Each frame runs: simulation systems (`ctx.time.fixed_steps()` times), then
    /// frame systems, then the closure from `setup`, then the
    /// [`Phase::Animate`], [`Phase::Late`] and [`Phase::Draw`] systems (the
    /// built-in [`draw_sprites`] is one of the last), then rendering.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Counter", 800, 600).run(|_ctx| {
    ///     let mut clicks = 0;
    ///     move |ctx: &mut Ctx| {
    ///         if ctx.input.mouse_pressed(MouseButton::Left) {
    ///             clicks += 1;
    ///             println!("clicks: {clicks}");
    ///         }
    ///     }
    /// });
    /// ```
    ///
    /// This form is for small programs. For a game with systems, put the
    /// startup code on the builder with [`setup`](Self::setup) and call
    /// [`ConfiguredGame::run`], which takes no closure: setup callbacks can
    /// be chosen at run time and are ordered with [`Plugin`] setups.
    ///
    /// # Smoke runs
    /// With the environment variable `PONKO_SMOKE=<ticks>` (a number above
    /// zero), `run` opens no window: it runs the setup and `ticks` ticks
    /// headless, frame systems included (as with
    /// [`headless_frames`](Self::headless_frames)), prints
    /// `ponko: smoke run passed: <ticks> ticks` to stderr and returns. This
    /// is how `ponko package` checks a packaged game starts, finds its
    /// assets and runs, on machines without a display or GPU. Not read in
    /// browsers.
    ///
    /// # Panics
    /// If the operating system refuses to create the event loop or window,
    /// or no usable GPU is found, and always when the `window` feature is
    /// off (except in a smoke run).
    pub fn run<S, U>(self, setup: S)
    where
        S: FnOnce(&mut Ctx) -> U,
        U: SetupResult,
    {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ticks) = smoke_ticks(std::env::var(ENV_SMOKE).ok().as_deref()) {
            let world = self.headless_frames(true).run_headless(ticks, setup);
            eprintln!("ponko: smoke run passed: {ticks} ticks, {} entities", world.len());
            return;
        }
        #[cfg(feature = "window")]
        window::run(self, setup);
        #[cfg(not(feature = "window"))]
        {
            drop((self, setup));
            panic!(
                "ponko: Game::run needs a window, and this build has none \
                 (the `window` feature is off); use Game::run_headless"
            );
        }
    }
}

/// `PONKO_SMOKE=<ticks>`: [`Game::run`] runs that many ticks headless and returns.
#[cfg(not(target_arch = "wasm32"))]
const ENV_SMOKE: &str = "PONKO_SMOKE";

/// The ticks of a smoke run from the value of `PONKO_SMOKE`: a number above
/// zero, else no smoke run (unset, empty, `0`, or not a number, which is
/// also reported, so a typo doesn't open a window unnoticed).
#[cfg(not(target_arch = "wasm32"))]
fn smoke_ticks(value: Option<&str>) -> Option<u64> {
    let value = value?.trim();
    match value.parse::<u64>() {
        Ok(0) => None,
        Ok(ticks) => Some(ticks),
        Err(_) if value.is_empty() => None,
        Err(_) => {
            eprintln!("ponko: ignoring {ENV_SMOKE}={value:?}: it takes a number of ticks");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ponko_ecs::Component;

    #[test]
    fn smoke_ticks_take_a_positive_number_only() {
        assert_eq!(smoke_ticks(Some("120")), Some(120));
        assert_eq!(smoke_ticks(Some(" 5 ")), Some(5));
        for off in [None, Some(""), Some("0"), Some("yes"), Some("-3")] {
            assert_eq!(smoke_ticks(off), None, "{off:?}");
        }
    }

    struct Log(Vec<String>);
    #[derive(Component)]
    struct Marker;

    fn ctx() -> Ctx {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(800.0, 600.0));
        ctx.world.insert_resource(Log(Vec::new()));
        ctx
    }

    fn log(ctx: &mut Ctx, s: &str) {
        let line = format!("{s} dt={:.4}", ctx.time.dt());
        ctx.world.resource_mut::<Log>().0.push(line);
    }

    fn sim_a(ctx: &mut Ctx) {
        log(ctx, "sim_a");
    }

    /// A game without the systems `Game::new` adds by itself.
    fn bare(game: Game) -> Game {
        game.remove(Systems::ANIMATE_SPRITES)
            .remove(Systems::DRAW_TILEMAPS)
            .remove(Systems::DRAW_SPRITES)
            .remove(Systems::DRAW_PARTICLES)
            .remove(Systems::UPDATE_ANIMATORS)
            .remove(Systems::UPDATE_PARTICLES)
            .remove(Systems::UPDATE_AUDIO3D)
    }
    fn frame_b(ctx: &mut Ctx) {
        log(ctx, "frame_b");
    }

    #[cfg(feature = "3d")]
    #[test]
    fn late_system_sees_automatic_animation_pose() {
        use crate::anim::AnimationClip;
        use crate::anim::AnimationGraph;
        use crate::anim::AnimationState;
        use crate::anim::Animator;
        use crate::anim::Joint;
        use crate::anim::Motion;
        use crate::anim::Skeleton;
        use crate::d3::Transform as Transform3D;
        let skeleton = Skeleton::new(vec![Joint {
            name: "root".into(),
            parent: None,
            rest: Transform3D::default(),
        }]);
        let clip = AnimationClip::new("rest", &skeleton, vec![]);
        let animator = Animator::new(
            AnimationGraph::new(
                vec![],
                vec![AnimationState::new(Motion::Clip(clip))],
                vec![],
                crate::anim::StateId(0),
            )
            .expect("a valid graph"),
        );
        let mut ctx = ctx();
        let entity = ctx.world.spawn((skeleton, animator));
        // Keeps the built-in animator system; it has nothing to draw.
        let mut game = Game::new("t", 1, 1)
            .system(move |ctx| ctx.world.get_mut::<Skeleton>(entity).unwrap().pose[0].position.x = 7.0)
            .add(Phase::Animate, move |ctx: &mut Ctx| {
                // Before the built-in animators, although it was added after them.
                assert_eq!(ctx.world.get::<Skeleton>(entity).unwrap().pose[0].position.x, 7.0);
            })
            .add(Phase::Late, move |ctx: &mut Ctx| {
                let s = ctx.world.get_mut::<Skeleton>(entity).unwrap();
                assert_eq!(s.pose[0].position.x, 0.0);
                s.pose[0].position.x = 2.0;
            });
        game.run_systems(&mut ctx, 0);
        assert_eq!(ctx.world.get::<Skeleton>(entity).unwrap().pose[0].position.x, 2.0);
    }

    #[test]
    fn late_system_runs_after_frame_before_draw_and_is_timed() {
        fn late(ctx: &mut Ctx) {
            log(ctx, "late");
        }
        fn draw(ctx: &mut Ctx) {
            log(ctx, "draw");
        }
        let mut game = bare(Game::new("t", 1, 1)).system(frame_b);
        let late = game.add_system(Phase::Late, late);
        game.draw_systems.push(System::new(draw));
        let mut ctx = ctx();
        game.run_systems(&mut ctx, 0);
        assert_eq!(
            ctx.world.resource::<Log>().0,
            ["frame_b dt=0.0000", "late dt=0.0000", "draw dt=0.0000"]
        );
        assert_eq!(ctx.perf.system_timings().len(), 3);
        assert!(ctx.perf.system_timings()[1].name.ends_with("::late"));
        let game = game.remove(late);
        assert!(game.late_systems.is_empty());
    }

    #[test]
    fn ids_remove_from_every_phase_and_builtin_aliases_remove_all_instances() {
        let mut game = Game::new("ids", 1, 1);
        for phase in [
            Phase::Input,
            Phase::BeforePhysics,
            Phase::Physics,
            Phase::AfterPhysics,
            Phase::SimLast,
            Phase::Frame,
            Phase::Animate,
            Phase::Late,
            Phase::Draw,
        ] {
            let id = game.add_system(phase, |_: &mut Ctx| panic!("removed system ran"));
            game = game.remove(id);
            assert!(
                !game
                    .sim_systems
                    .iter()
                    .chain(&game.systems)
                    .chain(&game.late_systems)
                    .chain(&game.draw_systems)
                    .any(|s| s.key == id)
            );
        }
        game.add_system(Phase::Draw, draw_sprites);
        assert_eq!(game.system_count(Systems::DRAW_SPRITES), 2);
        game = game.remove(Systems::DRAW_SPRITES);
        assert_eq!(game.system_count(Systems::DRAW_SPRITES), 0);
        assert_eq!(game.system_count(Systems::ANIMATE_SPRITES), 1);
        assert_eq!(game.system_count(Systems::DRAW_TILEMAPS), 1);
        let game = game
            .spatial_grid(32.0)
            .remove(Systems::REBUILD_SPATIAL_GRID)
            .spatial_grid(64.0);
        assert_eq!(game.sim_systems.len(), 1);
    }

    #[test]
    fn explicit_phases_sort_without_changing_order_within_a_phase() {
        let mut game = bare(Game::new("t", 1, 1))
            .add(Phase::Draw, |ctx: &mut Ctx| log(ctx, "draw"))
            .add(Phase::Late, |ctx: &mut Ctx| log(ctx, "late"))
            .add(Phase::Animate, |ctx: &mut Ctx| log(ctx, "animate"))
            .add(Phase::Frame, |ctx: &mut Ctx| log(ctx, "frame1"))
            .system(|ctx| log(ctx, "frame2"))
            .add(Phase::SimLast, |ctx: &mut Ctx| log(ctx, "sim_last"))
            .add(Phase::AfterPhysics, |ctx: &mut Ctx| log(ctx, "after"))
            .add(Phase::Physics, |ctx: &mut Ctx| log(ctx, "physics"))
            .add(Phase::BeforePhysics, |ctx: &mut Ctx| log(ctx, "before"))
            .add(Phase::Input, |ctx: &mut Ctx| log(ctx, "input"));
        let mut ctx = ctx();
        game.run_systems(&mut ctx, 1);
        let names: Vec<_> = ctx
            .world
            .resource::<Log>()
            .0
            .iter()
            .map(|s| s.split(' ').next().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "input", "before", "physics", "after", "sim_last", "frame1", "frame2", "animate", "late", "draw"
            ]
        );
    }

    #[test]
    fn order_and_dt() {
        let mut game = Game::new("t", 1, 1)
            .sim_system(sim_a)
            .system(frame_b)
            .remove(Systems::DRAW_SPRITES);
        let mut ctx = ctx();
        game.run_systems(&mut ctx, 2);
        assert_eq!(
            ctx.world.resource::<Log>().0,
            ["sim_a dt=0.0167", "sim_a dt=0.0167", "frame_b dt=0.0000"]
        );
    }

    #[test]
    fn commands_apply_between_systems() {
        fn spawner(ctx: &mut Ctx) {
            ctx.commands.spawn((Marker,));
        }
        fn counter(ctx: &mut Ctx) {
            let n = ctx.world.query::<&Marker>().count();
            ctx.world.resource_mut::<Log>().0.push(n.to_string());
        }
        let mut game = Game::new("t", 1, 1).system(spawner).system(counter);
        let mut ctx = ctx();
        game.run_systems(&mut ctx, 0);
        game.run_systems(&mut ctx, 0);
        assert_eq!(ctx.world.resource::<Log>().0, ["1", "2"]);
    }

    #[test]
    fn remove_takes_user_ids_and_builtin_constants() {
        let mut game = Game::new("t", 1, 1);
        let user = game.add_system(Phase::AfterPhysics, sim_a);
        let game = game
            .remove(user)
            .remove(Systems::ANIMATE_SPRITES)
            .remove(Systems::DRAW_TILEMAPS)
            .remove(Systems::DRAW_SPRITES)
            .remove(Systems::DRAW_PARTICLES);
        assert!(game.sim_systems.is_empty());
        // Only positional audio (feature `3d`) is left in `Draw`.
        assert!(
            game.draw_systems
                .iter()
                .all(|s| s.builtin == Some(Systems::UPDATE_AUDIO3D))
        );
    }

    #[test]
    fn removing_an_absent_system_does_nothing() {
        let mut game = Game::new("t", 1, 1);
        let before = game.sim_systems.len();
        let id = game.add_system(Phase::AfterPhysics, sim_a);
        let game = game.remove(id).remove(id);
        assert_eq!(game.sim_systems.len(), before);
        assert_eq!(game.system_count(id), 0);
    }

    #[test]
    #[allow(deprecated)]
    fn a_builtin_added_by_its_old_root_name_is_the_same_system() {
        let game = Game::new("t", 1, 1)
            .remove(Systems::DRAW_SPRITES)
            .add(Phase::Draw, crate::systems::draw_sprites);
        assert_eq!(game.system_count(Systems::DRAW_SPRITES), 1);
        let listed = game
            .draw_systems
            .iter()
            .find(|s| s.builtin == Some(Systems::DRAW_SPRITES));
        assert_eq!(
            listed.unwrap().name,
            std::any::type_name_of_val(&crate::systems::draw_sprites)
        );
    }

    /// The built-in frame steps are systems: listed, timed and removable.
    #[cfg(feature = "3d")]
    #[test]
    fn animators_particles_and_positional_audio_are_listed_as_systems() {
        fn user_draw(ctx: &mut Ctx) {
            log(ctx, "draw");
        }
        fn late(ctx: &mut Ctx) {
            log(ctx, "late");
        }
        // Added after the built-ins: they still end their phase.
        let mut game = Game::new("t", 1, 1)
            .add(Phase::Late, late)
            .add(Phase::Animate, |ctx: &mut Ctx| log(ctx, "animate"))
            .add(Phase::Draw, user_draw);
        let mut ctx = ctx();
        game.run_systems(&mut ctx, 0);
        let listed: Vec<&str> = ctx
            .perf
            .system_timings()
            .iter()
            .map(|t| t.name.rsplit("::").next().unwrap())
            .collect();
        // User systems of a phase run before the built-ins that end it.
        let at = |name: &str| {
            listed
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("{name}: {listed:?}"))
        };
        let user_animate = listed.iter().position(|n| n.contains("closure")).unwrap();
        assert!(user_animate < at("update_animators"));
        assert!(at("update_animators") < at("update_particles"));
        assert!(at("update_particles") < at("late"), "{listed:?}");
        // Positional audio follows the late and the drawing systems.
        assert!(at("late") < at("update_audio3d"), "{listed:?}");
        assert!(at("draw_sprites") < at("update_audio3d"), "{listed:?}");
        assert!(at("user_draw") < at("update_audio3d"), "{listed:?}");
        assert_eq!(listed.last(), Some(&"update_audio3d"), "{listed:?}");
        let phases: Vec<_> = (game.systems.iter().chain(&game.late_systems))
            .map(|s| (s.stage, s.last))
            .collect();
        assert_eq!(
            phases,
            [
                (Phase::Animate, false),
                (Phase::Animate, true),
                (Phase::Animate, true),
                (Phase::Late, false),
            ]
        );
        let last_draw = game.draw_systems.last().unwrap();
        assert_eq!((last_draw.stage, last_draw.last), (Phase::Draw, true));
        assert_eq!(last_draw.builtin, Some(Systems::UPDATE_AUDIO3D));
        assert_eq!(
            ctx.world.resource::<Log>().0,
            ["animate dt=0.0000", "late dt=0.0000", "draw dt=0.0000"]
        );

        // Removed like any other system, and placed again where the game wants it.
        let game = game
            .remove(Systems::UPDATE_PARTICLES)
            .remove(Systems::UPDATE_AUDIO3D)
            .add(Phase::Late, crate::systems::update_audio3d);
        assert_eq!(game.system_count(Systems::UPDATE_PARTICLES), 0);
        assert_eq!(game.system_count(Systems::UPDATE_ANIMATORS), 1);
        assert_eq!(game.system_count(Systems::UPDATE_AUDIO3D), 1);
        assert_eq!(game.late_systems.last().unwrap().builtin, Some(Systems::UPDATE_AUDIO3D));
        assert_ne!(game.draw_systems.last().unwrap().builtin, Some(Systems::UPDATE_AUDIO3D));
    }

    #[test]
    fn pause_step_and_speed_shape_frame_timing() {
        let mut clock = Clock::new(60);
        let mut ctx = ctx();
        let frame = 1.0 / 60.0;
        ctx.time.pause();
        assert_eq!(frame_timing(&mut clock, &mut ctx, frame).fixed_steps, 0);
        ctx.time.step();
        ctx.time.step();
        assert_eq!(
            frame_timing(&mut clock, &mut ctx, frame).fixed_steps,
            2,
            "two single steps while paused"
        );
        assert_eq!(
            frame_timing(&mut clock, &mut ctx, frame).fixed_steps,
            0,
            "steps are used up"
        );
        ctx.time.resume();
        ctx.time.set_speed(4.0);
        let steps: u32 = (0..60)
            .map(|_| frame_timing(&mut clock, &mut ctx, frame).fixed_steps)
            .sum();
        assert!((239..=240).contains(&steps), "4x speed for one second, got {steps}");
    }

    #[test]
    fn records_system_timings() {
        fn slow(_: &mut Ctx) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut game = bare(Game::new("t", 1, 1).sim_system(slow)).system(frame_b);
        let mut ctx = ctx();
        game.run_systems(&mut ctx, 3);
        let t = ctx.perf.system_timings();
        assert_eq!(t.len(), 2, "{t:?}");
        assert!(t[0].sim && !t[1].sim);
        assert!(t[0].name.ends_with("slow"), "{}", t[0].name);
        assert!(t[0].last_ms >= 6.0, "3 steps of 2 ms, got {}", t[0].last_ms);
        assert!(t[0].avg_ms > 0.0 && t[0].avg_ms < t[0].last_ms);
    }

    #[test]
    fn drawing_sees_positions_from_frame_systems() {
        fn move_right(ctx: &mut Ctx) {
            ctx.world
                .query::<&mut crate::d2::Transform>()
                .for_each(|t| t.position.x += 1.0);
        }
        let mut game = Game::new("t", 1, 1).system(move_right);
        let mut ctx = ctx();
        game.run_systems(&mut ctx, 0);
        let order: Vec<_> = ctx.perf.system_timings().iter().map(|t| t.name).collect();
        let at = |name: &str| order.iter().position(|n| n.ends_with(name)).unwrap();
        assert!(order[0].ends_with("move_right"), "{order:?}");
        assert!(at("move_right") < at("draw_sprites"), "{order:?}");
    }

    #[test]
    fn draw_sprites_queues_world_sprites() {
        let mut game = Game::new("t", 1, 1);
        let mut ctx = ctx();
        let tex = crate::texture::Texture::new(0, 16, 16);
        for i in 0..10 {
            ctx.world.spawn((
                crate::d2::Transform::at(Vec2::splat(i as f32)),
                crate::d2::Sprite::new(tex),
            ));
        }
        ctx.world.spawn((crate::d2::Transform::default(),)); // no sprite: not drawn
        game.run_systems(&mut ctx, 0);
        assert_eq!(ctx.gfx.sprite_count(), 10);
    }
}

#[cfg(test)]
mod barrier_tests;
#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod phase_order_tests;
#[cfg(test)]
mod scenario_tests;
