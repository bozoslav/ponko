//! What [`Game::add`](super::Game::add) takes: [`IntoSystem`] and
//! [`SystemConfig`].

use super::{SystemFn, SystemId, builtin_id, hot, hot_view};
use crate::access::SystemAccess;
use crate::ctx::Ctx;
use crate::system_view::{SystemView, ViewFn};

/// What a system runs, before it is registered.
pub(super) enum ConfigRun {
    /// A function of the whole `Ctx`.
    Ctx(SystemFn),
    /// A view system: it gets what its declaration lists.
    View(ViewFn),
}

/// A system and how to register it: its name, its interval and what it
/// declares. Made from a system function with the methods of
/// [`IntoSystem`], and taken by [`Game::add`](super::Game::add).
///
/// ```
/// use ponko::prelude::*;
/// fn think(_: &mut Ctx) {}
/// // Every 4th tick, listed as "ai" in timings and tools.
/// let config = think.named("ai").every(4);
/// let game = Game::new("t", 1, 1).add(Phase::AfterPhysics, config);
/// # let _ = game;
/// ```
#[must_use = "a system does nothing until it is added with `Game::add`"]
pub struct SystemConfig {
    pub(super) run: ConfigRun,
    pub(super) name: &'static str,
    pub(super) builtin: Option<SystemId>,
    pub(super) every: u32,
    pub(super) access: Option<SystemAccess>,
}

/// Anything [`Game::add`](super::Game::add) registers as a system.
///
/// Implemented for:
///
/// - **`Ctx` systems**: functions and closures that take `&mut Ctx`. They
///   may use the whole engine and always run alone, in order.
/// - **View systems**: `Send` functions and closures that take
///   `&mut SystemView`. They get exactly what their
///   [`access`](Self::access) declares, and may share a parallel batch
///   ([`Game::parallel_systems`](super::Game::parallel_systems)).
/// - [`SystemConfig`]: either of them with options set.
///
/// The methods set the options, so a system is configured where it is
/// added:
///
/// ```
/// use ponko::access::SystemAccess;
/// use ponko::prelude::*;
/// use ponko::system_view::SystemView;
///
/// #[derive(Component)]
/// struct Pos(f32);
/// #[derive(Component)]
/// struct Vel(f32);
///
/// fn think(_: &mut Ctx) {}
/// fn movement(sys: &mut SystemView) {
///     sys.world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| p.0 += v.0);
/// }
///
/// let game = Game::new("t", 1, 1)
///     .add(Phase::BeforePhysics, think)
///     .add(Phase::AfterPhysics, think.named("ai").every(4))
///     .add(Phase::AfterPhysics, movement.access(SystemAccess::new().write::<Pos>().read::<Vel>()));
/// # let _ = game;
/// ```
///
/// A closure passed to `add` names its parameter type
/// (`|ctx: &mut Ctx| ...`), which is how Rust tells the two kinds apart;
/// [`Game::sim_system`](super::Game::sim_system) and
/// [`Game::system`](super::Game::system) take `Ctx` closures as they are.
///
/// `Takes` only keeps the implementations apart, and Rust infers it: what
/// the function takes ([`Ctx`] or [`SystemView`]), or `()` for a
/// [`SystemConfig`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a system `Game::add` can run",
    label = "not a system",
    note = "a system is a function taking `&mut Ctx`, or a `Send + Sync` function taking `&mut SystemView`",
    note = "a closure names its parameter type: `|ctx: &mut Ctx| ...`"
)]
pub trait IntoSystem<Takes>: Sized {
    /// The system with its options, as [`Game::add`](super::Game::add)
    /// stores it. Game code calls the option methods instead.
    fn into_system(self) -> SystemConfig;

    /// Names the system in timings, plans, reports and tools. Without it a
    /// system is named by its function's path.
    fn named(self, name: &'static str) -> SystemConfig {
        let mut config = self.into_system();
        config.name = name;
        config
    }

    /// Runs the system only every `ticks`-th simulation tick, for work that
    /// doesn't need the full rate: AI that thinks a few times a second,
    /// slow growth, pathfinding, statistics. Inside it, `ctx.time.dt()` is
    /// `ticks` fixed steps, so rates stay right. Only for the fixed-tick
    /// phases (`Input` through `SimLast`).
    ///
    /// Systems added with the same interval are spread over different
    /// ticks, so their cost doesn't land on the same frame.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// #[derive(Component)]
    /// struct Hunger(f32);
    ///
    /// fn get_hungry(ctx: &mut Ctx) {
    ///     let dt = ctx.time.dt(); // 10 ticks' worth of time
    ///     ctx.world.query::<&mut Hunger>().for_each(|h| h.0 += 1.0 * dt);
    /// }
    ///
    /// // 60 ticks at 60 Hz: runs 6 times, 1/6 s each.
    /// let mut world = Game::new("t", 1, 1)
    ///     .add(Phase::AfterPhysics, get_hungry.every(10))
    ///     .run_headless(60, |ctx| { ctx.world.spawn((Hunger(0.0),)); });
    /// let h = world.query::<&Hunger>().iter().next().unwrap().0;
    /// assert!((h - 1.0).abs() < 1e-4);
    /// ```
    ///
    /// # Panics
    /// If `ticks` is 0. [`Game::add`](super::Game::add) panics if the
    /// system then goes into a per-frame phase.
    #[track_caller]
    fn every(self, ticks: u32) -> SystemConfig {
        assert!(ticks > 0, "IntoSystem::every: the interval must be at least 1 tick");
        let mut config = self.into_system();
        config.every = ticks;
        config
    }

    /// Declares what the system reads and writes; see [`SystemAccess`].
    ///
    /// - A **view system** can reach exactly this, in every build: anything
    ///   else panics when it is used. Without a declaration it can reach
    ///   nothing.
    /// - A **`Ctx` system** still gets the whole `Ctx` and still runs alone.
    ///   The declaration is for [`Game::access_report`](super::Game::access_report)
    ///   and tools, and in debug builds using anything beyond it panics
    ///   after the system runs. Without one it is exclusive.
    ///
    /// Calling it again replaces the declaration.
    fn access(self, access: SystemAccess) -> SystemConfig {
        let mut config = self.into_system();
        config.access = Some(access);
        config
    }
}

impl<F: FnMut(&mut Ctx) + 'static> IntoSystem<Ctx> for F {
    fn into_system(self) -> SystemConfig {
        let name = std::any::type_name::<F>();
        let builtin = builtin_id(name);
        SystemConfig {
            run: ConfigRun::Ctx(hot(self)),
            name: builtin.map_or(name, |b| b.1),
            builtin: builtin.map(|b| b.0),
            every: 1,
            access: None,
        }
    }
}

impl<F: FnMut(&mut SystemView<'_>) + Send + 'static> IntoSystem<SystemView<'static>> for F {
    fn into_system(self) -> SystemConfig {
        SystemConfig {
            run: ConfigRun::View(hot_view(self)),
            name: std::any::type_name::<F>(),
            builtin: None,
            every: 1,
            access: None,
        }
    }
}

impl IntoSystem<()> for SystemConfig {
    fn into_system(self) -> SystemConfig {
        self
    }
}
