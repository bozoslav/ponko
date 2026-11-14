//! The schedule of a game's systems: which run alone and which share a
//! batch, and why. From [`Game::schedule_plan`](crate::game::Game::schedule_plan).
//!
//! Systems run phase after phase, and inside a phase in registration order.
//! The plan puts them into **stages**: a stage is either one system that
//! runs alone, or a **batch** of view systems (systems that take
//! `&mut SystemView`; see [`Game::add`](crate::game::Game::add)) whose
//! declarations don't conflict
//! ([`SystemAccess::conflicts_before`](crate::access::SystemAccess::conflicts_before)).
//! Stages run one after another; with
//! [`Game::parallel_systems`](crate::game::Game::parallel_systems) on, the systems
//! of a batch may run at the same time.
//!
//! # Batching by dependencies
//!
//! Phases, explicit barriers and systems that run alone cut the order into
//! **segments**, which nothing crosses. Inside a segment, each view system
//! goes to the earliest stage that keeps every rule with the systems
//! registered before it:
//!
//! - it runs in a **later stage** than every earlier system whose work it
//!   must see: one it conflicts with
//!   ([`conflicts_before`](crate::access::SystemAccess::conflicts_before)),
//!   or one [`Game::order`](crate::game::Game::order) puts first;
//! - it runs **no earlier** than any earlier system whose order would show
//!   at all ([`conflicts_with`](crate::access::SystemAccess::conflicts_with):
//!   two senders of one event type, say). It may share that system's
//!   batch, under the barrier rules below, but not move ahead of it.
//!
//! So a system that interacts with nothing registered between may join an
//! earlier stage, ahead of systems registered before it: the order of two
//! systems that don't interact can't be observed. Two systems that
//! conflict keep their registration order. The members of a batch need not
//! be neighbours ([`Stage::members`]), and every system says what kept it
//! out of the stage before its own ([`PlannedSystem::boundary`]).
//!
//! # Barriers
//!
//! The end of every stage is a barrier. There, and only there:
//!
//! - the **commands** recorded in the stage are applied, in registration
//!   order;
//! - the **events** sent in the stage become visible to other systems, in
//!   registration order;
//! - a new change tick starts, so later systems' `Added` and `Changed`
//!   filters see the stage's writes.
//!
//! A system that runs alone is a stage of its own, so this is the contract
//! systems always had: commands apply and events are visible right after
//! each system. For a batch, the conflict rules make sure that no system in
//! it could have seen the difference: a system that sends an event type
//! never shares a batch with a later reader of it, and a system that
//! records commands is always the last of its batch.
//!
//! A system is kept out of an earlier stage ([`Boundary`]):
//!
//! - at every [`Phase`],
//! - before and after every system that runs alone ([`Alone`]): every
//!   `&mut Ctx` system, and view systems that hold a service only one
//!   system may hold, send and read the same event type, or run at an
//!   interval,
//! - by a system it conflicts with,
//! - at an explicit [`Game::barrier`](crate::game::Game::barrier),
//! - where [`Game::order`](crate::game::Game::order) says it must see
//!   another's work,
//! - by an earlier system it must not move ahead of
//!   ([`Boundary::KeepsOrder`]).
//!
//! # Determinism
//!
//! The plan depends only on the registered systems and their declarations.
//! Whether a batch really runs on several threads (it doesn't when its
//! systems are too cheap to be worth it, while profiling, on the web, or on
//! a machine with one core) changes nothing a system can observe: in both
//! cases the batch's systems share the world through their views, get their
//! own command buffer and event outbox, and those are delivered at the
//! barrier in registration order. Each system's random stream depends on
//! the seed, the tick and the system, as always.
//!
//! Change ticks are the same too. Run one at a time, every system has a
//! change tick of its own; the systems of a batch write in exactly those
//! ticks, also when they run ahead of systems registered before them, so
//! `change_tick()` reads the same number in a system, change stamps
//! compare the same against a tick a system kept, and the world's tick
//! after any stage is the same with the scheduler on or off.
//!
//! # Panics
//!
//! If a system of a batch panics, the other systems of the batch still
//! run to their end: on threads they may be running already, so the batch
//! does the same in order. Then the panic is passed on; if several systems
//! panicked, the one registered first. A batch that ends this way has no
//! barrier: the commands its systems recorded and the events they sent are
//! dropped, and the events they read stay unread. What the systems wrote
//! to components and resources stays, as it does when a system that runs
//! alone panics halfway. Nothing is kept for a later batch to deliver, and
//! all of this is the same on threads and in order. (With the scheduler
//! off, systems after the panicking one never start: that difference
//! can't be avoided once systems run at the same time.) A game that
//! catches the panic and goes on runs every system again normally: as
//! after a run alone, a system that returned isn't shown its own writes of
//! that batch as changes, and the batch's change ticks have ended. The
//! system that panicked is shown its own writes once more, as it is when
//! it panics running alone.
//!
//! ```
//! use ponko::access::SystemAccess;
//! use ponko::prelude::*;
//! use ponko::system_view::SystemView;
//!
//! #[derive(Component)]
//! struct Pos(f32);
//! #[derive(Component)]
//! struct Vel(f32);
//! #[derive(Component)]
//! struct Health(i32);
//!
//! fn movement(sys: &mut SystemView) {
//!     sys.world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| p.0 += v.0);
//! }
//! fn regenerate(sys: &mut SystemView) {
//!     sys.world.query::<&mut Health>().for_each(|h| h.0 += 1);
//! }
//! fn stop(sys: &mut SystemView) {
//!     sys.world.query::<&mut Vel>().for_each(|v| v.0 = 0.0);
//! }
//!
//! let game = Game::new("plan", 1, 1)
//!     .parallel_systems(true)
//!     .add(Phase::AfterPhysics, movement.access(SystemAccess::new().write::<Pos>().read::<Vel>()))
//!     .add(Phase::AfterPhysics, regenerate.access(SystemAccess::new().write::<Health>()))
//!     .add(Phase::AfterPhysics, stop.access(SystemAccess::new().write::<Vel>()));
//! let plan = game.schedule_plan();
//! // `movement` and `regenerate` share a batch; `stop` writes what
//! // `movement` reads, so it waits for the batch to end.
//! let stages = plan.stages_in(Phase::AfterPhysics);
//! assert_eq!((stages[0].len, stages[1].len), (2, 1));
//! ```

use std::fmt;

use crate::access::{AccessConflict, Service, SystemAccess, Unbatchable};
use crate::game::Phase;
use crate::game::SystemId;

/// How much work a batch needs before it goes to other threads, unless
/// [`Game::parallel_threshold`](crate::game::Game::parallel_threshold) says
/// otherwise: the usual run time of its systems, without the slowest one.
/// From the `schedule/` benchmarks (see the guide's "Parallel systems").
pub const DEFAULT_PARALLEL_THRESHOLD: std::time::Duration = std::time::Duration::from_micros(50);

/// Why a system runs alone, as a stage of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Alone {
    /// It has no declaration: it may use anything.
    Undeclared,
    /// It is declared but takes `&mut Ctx`
    /// ([`IntoSystem::access`](crate::game::IntoSystem::access), C and C++
    /// systems): its declaration is only checked in debug builds, so it
    /// isn't trusted to share the world.
    CtxSystem,
    /// A view system that declares this service, which only one system at a
    /// time may hold (sound, the cameras, the physics worlds, networking).
    Service(Service),
    /// A view system that sends and reads this event type: alone, it sees
    /// its own events at once.
    SendsAndReads(&'static str),
    /// A view system that runs at an interval (not every tick): a batch
    /// runs as a whole, so a system that skips ticks stays out of it.
    Interval,
}

impl fmt::Display for Alone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Alone::Undeclared => f.write_str("undeclared"),
            Alone::CtxSystem => f.write_str("takes `&mut Ctx`"),
            Alone::Service(s) => write!(f, "holds service `{s}`"),
            Alone::SendsAndReads(n) => write!(f, "sends and reads events `{n}`"),
            Alone::Interval => f.write_str("runs at an interval"),
        }
    }
}

/// A [`Game::order`](crate::game::Game::order) pair that contradicts the run
/// order: from [`Game::check_order`](crate::game::Game::check_order). `order` can
/// keep two systems apart, not swap them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OrderError {
    /// The name of the system the pair puts first.
    pub first: &'static str,
    /// The name of the system the pair puts second, which already runs
    /// before `first`: in an earlier phase, or registered first in the same
    /// phase.
    pub second: &'static str,
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let OrderError { first, second } = self;
        write!(
            f,
            "Game::order: `{second}` already runs before `{first}` (an earlier phase, or registered first in \
             the same phase); order can keep two systems apart, not swap them"
        )
    }
}

impl std::error::Error for OrderError {}

/// One system of a [`SchedulePlan`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PlannedSystem {
    /// The system's name (its function path).
    pub name: &'static str,
    /// The phase it runs in.
    pub phase: Phase,
    /// Why it runs alone; `None` for a view system that can share a batch.
    pub alone: Option<Alone>,
    /// Why it isn't in an earlier stage: which system keeps it out of the
    /// stage before its own, and on what. For the first system of a stage
    /// this is the stage's [`Stage::boundary`]; the other systems of a
    /// batch each have their own reason.
    pub boundary: Boundary,
}

/// Why a stage starts where it does: what keeps its first system out of
/// the stage before it. Also, per system ([`PlannedSystem::boundary`]),
/// what keeps that system out of the stage before its own.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Boundary {
    /// It is the first stage of its phase.
    Phase,
    /// Its system runs alone, or the system before it does.
    Alone,
    /// Its first system conflicts with system `with` (an index into
    /// [`SchedulePlan::systems`]) of the batch before it.
    Conflict {
        /// The earlier system.
        with: usize,
        /// What they conflict on.
        conflicts: Vec<AccessConflict>,
    },
    /// An explicit [`Game::barrier`](crate::game::Game::barrier).
    Barrier,
    /// [`Game::order`](crate::game::Game::order) puts its first system after
    /// system `after` of the batch before it.
    Order {
        /// The earlier system.
        after: usize,
    },
    /// Only per system, never for a stage: the system must not run before
    /// system `after`, registered earlier and in this same stage, because
    /// their order would show (two senders of one event type, a sender and
    /// a reader, ...). It may share `after`'s batch, as neighbours always
    /// could, but not move ahead of it.
    KeepsOrder {
        /// The earlier system.
        after: usize,
        /// What makes their order visible
        /// ([`SystemAccess::conflicts_with`]).
        conflicts: Vec<AccessConflict>,
    },
}

/// One stage of a [`SchedulePlan`]: the systems [`members`](Self::members)
/// of [`SchedulePlan::systems`]. More than one system is a batch.
///
/// The members are in registration order, but need not be neighbours: a
/// system that doesn't interact with the systems registered before it may
/// join an earlier stage than theirs.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Stage {
    /// The phase of its systems.
    pub phase: Phase,
    /// Index of its first system (the first registered).
    pub first: usize,
    /// How many systems it has.
    pub len: usize,
    /// The indexes of its systems in [`SchedulePlan::systems`], in
    /// registration order.
    pub members: Vec<usize>,
    /// Why it starts here: what keeps its first system out of the stage
    /// before it. Never [`Boundary::KeepsOrder`].
    pub boundary: Boundary,
}

impl Stage {
    /// The indexes of its systems in [`SchedulePlan::systems`], in
    /// registration order (the same as [`members`](Self::members)).
    #[must_use]
    pub fn systems(&self) -> &[usize] {
        &self.members
    }

    /// `true` if more than one system shares the stage.
    #[must_use]
    pub fn is_batch(&self) -> bool {
        self.len > 1
    }
}

/// Which systems run alone and which share a batch: from
/// [`Game::schedule_plan`](crate::game::Game::schedule_plan). See the
/// [module documentation](self) for the rules.
///
/// The plan is the same whether or not
/// [`Game::parallel_systems`](crate::game::Game::parallel_systems) is on: it shows
/// what turning it on would run together. Printing it (`{plan}`) lists every
/// stage with the reason it starts there.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SchedulePlan {
    /// Every registered system, in run order.
    pub systems: Vec<PlannedSystem>,
    /// The stages, in run order; together they cover every system once.
    pub stages: Vec<Stage>,
}

impl SchedulePlan {
    /// The stages of `phase`, in run order.
    #[must_use]
    pub fn stages_in(&self, phase: Phase) -> Vec<&Stage> {
        self.stages.iter().filter(|s| s.phase == phase).collect()
    }

    /// The stages with more than one system.
    pub fn batches(&self) -> impl Iterator<Item = &Stage> + '_ {
        self.stages.iter().filter(|s| s.is_batch())
    }

    /// The names of `stage`'s systems, in registration order.
    #[must_use]
    pub fn names(&self, stage: &Stage) -> Vec<&'static str> {
        stage.members.iter().map(|&i| self.systems[i].name).collect()
    }

    /// The index of the stage that runs the system named `name` (the first
    /// one, if several systems share the name).
    #[must_use]
    pub fn stage_of(&self, name: &str) -> Option<usize> {
        let system = self.systems.iter().position(|s| s.name == name)?;
        self.stages.iter().position(|s| s.systems().contains(&system))
    }
}

impl fmt::Display for SchedulePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut phase = None;
        for stage in &self.stages {
            if phase != Some(stage.phase) {
                phase = Some(stage.phase);
                writeln!(f, "{:?}", stage.phase)?;
            }
            let names = self.names(stage).join(" + ");
            let first = &self.systems[stage.first];
            let kind = match (first.alone, stage.is_batch()) {
                (Some(why), _) => format!("alone: {why}"),
                (None, true) => "batch".into(),
                (None, false) => "batch of one".into(),
            };
            writeln!(f, "  {names} ({kind}{})", self.reason(&stage.boundary))?;
            // The other members, where something else placed them.
            for &m in &stage.members[1..] {
                let why = &self.systems[m].boundary;
                if *why != stage.boundary && !matches!(why, Boundary::Phase | Boundary::Alone | Boundary::Barrier) {
                    writeln!(f, "    {}{}", self.systems[m].name, self.reason(why))?;
                }
            }
        }
        Ok(())
    }
}

impl SchedulePlan {
    /// `boundary` as printed after a stage or system: empty, or `; ` and why.
    fn reason(&self, boundary: &Boundary) -> String {
        let name = |i: usize| self.systems[i].name;
        let list = |conflicts: &[AccessConflict]| {
            let why: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
            why.join(", ")
        };
        match boundary {
            Boundary::Phase | Boundary::Alone => String::new(),
            Boundary::Conflict { with, conflicts } => {
                format!("; after `{}`: conflict on {}", name(*with), list(conflicts))
            }
            Boundary::Barrier => "; after a barrier".into(),
            Boundary::Order { after } => format!("; ordered after `{}`", name(*after)),
            Boundary::KeepsOrder { after, conflicts } => {
                format!("; not ahead of `{}`: order shows on {}", name(*after), list(conflicts))
            }
        }
    }
}

/// One registered system, as the planner sees it.
pub(crate) struct PlanInput<'a> {
    pub(crate) name: &'static str,
    pub(crate) phase: Phase,
    /// Which of the game's system lists it is in: stages don't cross lists.
    pub(crate) list: u8,
    pub(crate) id: SystemId,
    pub(crate) access: Option<&'a SystemAccess>,
    /// A view system: it can't reach anything undeclared.
    pub(crate) view: bool,
    /// It runs at an interval, not every tick.
    pub(crate) interval: bool,
    /// An explicit barrier was placed before it.
    pub(crate) barrier: bool,
}

fn why_alone(s: &PlanInput<'_>) -> Option<Alone> {
    let Some(access) = s.access else {
        return Some(Alone::Undeclared);
    };
    if access.is_exclusive() {
        return Some(Alone::Undeclared);
    }
    if !s.view {
        return Some(Alone::CtxSystem);
    }
    if s.interval {
        return Some(Alone::Interval);
    }
    access.batch_problem().map(|p| match p {
        Unbatchable::Service(s) => Alone::Service(s),
        Unbatchable::SendsAndReads(n) => Alone::SendsAndReads(n),
    })
}

/// What is wrong with `order`, given the systems' ids and names in run
/// order: the first pair that names two registered systems the wrong way
/// round. Checked whether the scheduler is on or not, so that turning it
/// on can't be what makes a game panic. Allocates nothing unless there is
/// a problem.
pub(crate) fn check_order(
    systems: impl Iterator<Item = (SystemId, &'static str)> + Clone,
    order: &[(SystemId, SystemId)],
) -> Result<(), OrderError> {
    let wrong = order.iter().find_map(|&(first, second)| {
        let find = |id: SystemId| systems.clone().enumerate().find(|(_, s)| s.0 == id);
        let ((a, (_, first)), (b, (_, second))) = (find(first)?, find(second)?);
        (a >= b).then_some(OrderError { first, second })
    });
    wrong.map_or(Ok(()), Err)
}

/// Cuts `systems` (in registration order) into stages. `order` lists
/// pairs of systems where the second must see the first's work.
///
/// Systems that run alone, phases, lists and explicit barriers cut the
/// order into **segments**: runs of batchable view systems. Inside a
/// segment, each system goes to the earliest stage that keeps every rule
/// with the systems registered before it (see [`place`]); outside, nothing
/// moves.
///
/// # Panics
/// If a pair in `order` names two registered systems the wrong way round.
#[track_caller]
pub(crate) fn plan(systems: &[PlanInput<'_>], order: &[(SystemId, SystemId)]) -> SchedulePlan {
    if let Err(problem) = check_order(systems.iter().map(|s| (s.id, s.name)), order) {
        panic!("{problem}");
    }
    let mut out = SchedulePlan::default();
    let mut i = 0;
    while i < systems.len() {
        let s = &systems[i];
        let alone = why_alone(s);
        let new_phase = i == 0 || systems[i - 1].phase != s.phase || systems[i - 1].list != s.list;
        let cut = if new_phase {
            Boundary::Phase
        } else if s.barrier {
            Boundary::Barrier
        } else {
            Boundary::Alone
        };
        if alone.is_some() {
            out.systems.push(PlannedSystem {
                name: s.name,
                phase: s.phase,
                alone,
                boundary: cut.clone(),
            });
            out.stages.push(Stage {
                phase: s.phase,
                first: i,
                len: 1,
                members: vec![i],
                boundary: cut,
            });
            i += 1;
            continue;
        }
        // The segment: `s` and the batchable view systems after it, up to
        // the next phase, list, barrier or system that runs alone.
        let end = (i + 1..systems.len())
            .find(|&j| {
                let (a, b) = (&systems[j - 1], &systems[j]);
                a.phase != b.phase || a.list != b.list || b.barrier || why_alone(b).is_some()
            })
            .unwrap_or(systems.len());
        let segment = &systems[i..end];
        let placed = place(segment, order);
        let stages = placed.iter().map(|p| p.stage).max().map_or(0, |m| m + 1);
        let first_stage = out.stages.len();
        for level in 0..stages {
            let members: Vec<usize> = (placed.iter().enumerate())
                .filter(|(_, p)| p.stage == level)
                .map(|(k, _)| i + k)
                .collect();
            let first = members[0];
            out.stages.push(Stage {
                phase: s.phase,
                first,
                len: members.len(),
                members,
                boundary: Boundary::Phase,
            });
        }
        for (k, (input, p)) in segment.iter().zip(placed).enumerate() {
            let boundary = match p.why {
                None => cut.clone(),
                Some(why) => why.offset(i),
            };
            if out.stages[first_stage + p.stage].first == i + k {
                out.stages[first_stage + p.stage].boundary = boundary.clone();
            }
            out.systems.push(PlannedSystem {
                name: input.name,
                phase: input.phase,
                alone: None,
                boundary,
            });
        }
        i = end;
    }
    out
}

/// Where one system of a segment goes: its stage, counted from the
/// segment's first, and why it can't go to the one before.
struct Placed {
    stage: usize,
    /// `None` in the segment's first stage. Indexes are into the segment.
    why: Option<Boundary>,
}

impl Boundary {
    /// The same reason with its system index moved by `by`.
    fn offset(self, by: usize) -> Boundary {
        match self {
            Boundary::Conflict { with, conflicts } => Boundary::Conflict {
                with: with + by,
                conflicts,
            },
            Boundary::Order { after } => Boundary::Order { after: after + by },
            Boundary::KeepsOrder { after, conflicts } => Boundary::KeepsOrder {
                after: after + by,
                conflicts,
            },
            other => other,
        }
    }
}

/// Places the systems of one segment (batchable view systems, in
/// registration order) into stages. A system `s` goes to the earliest stage
/// that respects, for every system `t` registered before it:
///
/// - **after** `t`'s stage if `s` must see `t`'s work: `t` conflicts with a
///   later `s` ([`SystemAccess::conflicts_before`]) or `order` says so;
/// - **not before** `t`'s stage if their order is visible at all
///   ([`SystemAccess::conflicts_with`]: moving `s` ahead of `t` would show,
///   say two senders of one event type), so `s` may share `t`'s batch, as
///   it could before, but never runs ahead of it.
///
/// Systems that don't interact at all may run in any order, so a later
/// system may go to an earlier stage than one registered before it. The
/// result is the fewest stages these rules allow (each system as early as
/// it can be), and it depends only on the declarations.
fn place(segment: &[PlanInput<'_>], order: &[(SystemId, SystemId)]) -> Vec<Placed> {
    let mut placed: Vec<Placed> = Vec::with_capacity(segment.len());
    for (k, s) in segment.iter().enumerate() {
        let access = s.access.expect("a batchable system is declared");
        let mut at = Placed { stage: 0, why: None };
        for (j, t) in segment[..k].iter().enumerate() {
            let earlier = t.access.expect("a batchable system is declared");
            let stage = placed[j].stage;
            // Only a constraint that would move `s` later matters.
            if order.contains(&(t.id, s.id)) {
                if stage + 1 > at.stage || at.why.is_none() {
                    at = Placed {
                        stage: stage + 1,
                        why: Some(Boundary::Order { after: j }),
                    };
                }
                continue;
            }
            if stage + 1 > at.stage || at.why.is_none() {
                let conflicts = earlier.conflicts_before(access);
                if !conflicts.is_empty() {
                    at = Placed {
                        stage: stage + 1,
                        why: Some(Boundary::Conflict { with: j, conflicts }),
                    };
                    continue;
                }
            }
            if stage > at.stage {
                let conflicts = earlier.conflicts_with(access);
                if !conflicts.is_empty() {
                    at = Placed {
                        stage,
                        why: Some(Boundary::KeepsOrder { after: j, conflicts }),
                    };
                }
            }
        }
        placed.push(at);
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use ponko_ecs::Component;

    #[derive(Component)]
    struct A;
    #[derive(Component)]
    struct B;
    #[derive(Component)]
    struct C;
    struct Hit;

    /// A view system in `AfterPhysics`.
    fn view<'a>(name: &'static str, access: &'a SystemAccess) -> PlanInput<'a> {
        PlanInput {
            name,
            phase: Phase::AfterPhysics,
            list: 0,
            id: SystemId::from_bits(name.as_bytes()[0].into()),
            access: Some(access),
            view: true,
            interval: false,
            barrier: false,
        }
    }

    fn stages(plan: &SchedulePlan) -> Vec<String> {
        plan.stages.iter().map(|s| plan.names(s).join("+")).collect()
    }

    #[test]
    fn independent_view_systems_share_one_batch() {
        let (a, b, c) = (
            SystemAccess::new().write::<A>(),
            SystemAccess::new().write::<B>(),
            SystemAccess::new().write::<C>(),
        );
        let plan = plan(&[view("a", &a), view("b", &b), view("c", &c)], &[]);
        assert_eq!(stages(&plan), ["a+b+c"]);
        assert_eq!(plan.stages[0].boundary, Boundary::Phase);
        assert_eq!(plan.batches().count(), 1);
    }

    #[test]
    fn a_chain_of_conflicts_is_one_stage_per_system() {
        // Each reads what the one before writes.
        let (a, b, c) = (
            SystemAccess::new().write::<A>(),
            SystemAccess::new().read::<A>().write::<B>(),
            SystemAccess::new().read::<B>().write::<C>(),
        );
        let plan = plan(&[view("a", &a), view("b", &b), view("c", &c)], &[]);
        assert_eq!(stages(&plan), ["a", "b", "c"]);
        assert_eq!(
            plan.stages[1].boundary,
            Boundary::Conflict {
                with: 0,
                conflicts: vec![AccessConflict::Component(std::any::type_name::<A>())]
            }
        );
        assert_eq!(plan.batches().count(), 0);
    }

    #[test]
    fn readers_fan_out_after_their_writer_and_join_again_before_the_next() {
        let w = SystemAccess::new().write::<A>();
        let r = SystemAccess::new().read::<A>();
        let rb = SystemAccess::new().read::<A>().write::<B>();
        let end = SystemAccess::new().write::<A>().read::<B>();
        let systems = [
            view("w", &w),
            view("x", &r),
            view("y", &rb),
            view("z", &r),
            view("e", &end),
        ];
        let plan = plan(&systems, &[]);
        assert_eq!(stages(&plan), ["w", "x+y+z", "e"]);
        // `e` is kept out by the first batch member it conflicts with.
        assert!(matches!(plan.stages[2].boundary, Boundary::Conflict { with: 1, .. }));
    }

    #[test]
    fn a_system_is_checked_against_every_member_of_the_batch() {
        // `c` is fine next to `b` but not next to `a`.
        let (a, b, c) = (
            SystemAccess::new().write::<A>(),
            SystemAccess::new().write::<B>(),
            SystemAccess::new().read::<A>(),
        );
        let plan = plan(&[view("a", &a), view("b", &b), view("c", &c)], &[]);
        assert_eq!(stages(&plan), ["a+b", "c"]);
    }

    #[test]
    fn systems_that_run_alone_cut_the_batches_around_them() {
        let (a, b) = (SystemAccess::new().write::<A>(), SystemAccess::new().write::<B>());
        let ctx_declared = SystemAccess::new().write::<C>();
        let mut undeclared = view("u", &a);
        undeclared.access = None;
        let mut ctx_system = view("d", &ctx_declared);
        ctx_system.view = false;
        let systems = [view("a", &a), undeclared, view("b", &b), ctx_system, view("c", &a)];
        let plan = plan(&systems, &[]);
        assert_eq!(stages(&plan), ["a", "u", "b", "d", "c"]);
        let alone: Vec<_> = plan.systems.iter().map(|s| s.alone).collect();
        assert_eq!(
            alone,
            [None, Some(Alone::Undeclared), None, Some(Alone::CtxSystem), None]
        );
        assert!(plan.stages[1..].iter().all(|s| s.boundary == Boundary::Alone));
    }

    #[test]
    fn exclusive_services_and_self_read_events_run_alone() {
        let audio = SystemAccess::new().write_service(Service::Audio);
        let camera = SystemAccess::new().read_service(Service::Camera);
        let echo = SystemAccess::new().send_events::<Hit>().read_events::<Hit>();
        let shared = SystemAccess::new()
            .read_service(Service::Input)
            .read_service(Service::Time)
            .read_service(Service::Random)
            .read_service(Service::Spatial);
        let systems = [
            view("a", &audio),
            view("c", &camera),
            view("e", &echo),
            view("s", &shared),
            view("t", &shared),
        ];
        let plan = plan(&systems, &[]);
        assert_eq!(stages(&plan), ["a", "c", "e", "s+t"]);
        assert_eq!(plan.systems[0].alone, Some(Alone::Service(Service::Audio)));
        assert_eq!(plan.systems[1].alone, Some(Alone::Service(Service::Camera)));
        assert_eq!(
            plan.systems[2].alone,
            Some(Alone::SendsAndReads(std::any::type_name::<Hit>()))
        );
    }

    #[test]
    fn events_and_commands_follow_the_ordered_rules() {
        let send = SystemAccess::new().send_events::<Hit>();
        let read = SystemAccess::new().read_events::<Hit>();
        let record = SystemAccess::new().write::<A>().commands();
        let other = SystemAccess::new().write::<B>();
        // Reader first, two senders: one batch. A later reader waits.
        let systems = [view("r", &read), view("s", &send), view("t", &send), view("u", &read)];
        assert_eq!(stages(&plan(&systems, &[])), ["r+s+t", "u"]);
        // A recorder is the last of its batch.
        let third = SystemAccess::new().write::<C>();
        let systems = [view("o", &other), view("c", &record), view("p", &third)];
        let p = plan(&systems, &[]);
        assert_eq!(stages(&p), ["o+c", "p"]);
        assert!(matches!(
            &p.stages[1].boundary,
            Boundary::Conflict { with: 1, conflicts } if conflicts.contains(&AccessConflict::Commands)
        ));
    }

    #[test]
    fn an_explicit_barrier_starts_a_new_stage() {
        let (a, b, c) = (
            SystemAccess::new().write::<A>(),
            SystemAccess::new().write::<B>(),
            SystemAccess::new().write::<C>(),
        );
        let mut after_barrier = view("b", &b);
        after_barrier.barrier = true;
        let plan = plan(&[view("a", &a), after_barrier, view("c", &c)], &[]);
        assert_eq!(stages(&plan), ["a", "b+c"]);
        assert_eq!(plan.stages[1].boundary, Boundary::Barrier);
    }

    #[test]
    fn explicit_order_keeps_two_compatible_systems_apart() {
        let (a, b, c) = (
            SystemAccess::new().write::<A>(),
            SystemAccess::new().write::<B>(),
            SystemAccess::new().write::<C>(),
        );
        let systems = [view("a", &a), view("b", &b), view("c", &c)];
        let order = [(systems[0].id, systems[2].id)];
        let plan = plan(&systems, &order);
        assert_eq!(stages(&plan), ["a+b", "c"]);
        assert_eq!(plan.stages[1].boundary, Boundary::Order { after: 0 });
    }

    #[test]
    #[should_panic(expected = "`a` already runs before `c`")]
    fn explicit_order_against_the_run_order_panics() {
        let (a, c) = (SystemAccess::new().write::<A>(), SystemAccess::new().write::<C>());
        let systems = [view("a", &a), view("c", &c)];
        let _ = plan(&systems, &[(systems[1].id, systems[0].id)]);
    }

    #[test]
    fn phases_and_lists_are_barriers() {
        let (a, b) = (SystemAccess::new().write::<A>(), SystemAccess::new().write::<B>());
        let mut later = view("b", &b);
        later.phase = Phase::SimLast;
        let mut other_list = view("c", &a);
        other_list.phase = Phase::SimLast;
        other_list.list = 1;
        let plan = plan(&[view("a", &a), later, other_list], &[]);
        assert_eq!(stages(&plan), ["a", "b", "c"]);
        assert!(plan.stages.iter().all(|s| s.boundary == Boundary::Phase));
        assert_eq!(plan.stages_in(Phase::SimLast).len(), 2);
    }

    #[test]
    fn a_view_system_with_an_interval_runs_alone_and_says_why() {
        let (a, b, c) = (
            SystemAccess::new().write::<A>(),
            SystemAccess::new().write::<B>(),
            SystemAccess::new().write::<C>(),
        );
        let every_other_tick = PlanInput {
            interval: true,
            ..view("b", &b)
        };
        let plan = plan(&[view("a", &a), every_other_tick, view("c", &c)], &[]);
        assert_eq!(stages(&plan), ["a", "b", "c"]);
        assert_eq!(plan.systems[1].alone, Some(Alone::Interval));
        // Not "takes `&mut Ctx`": it is a view system.
        assert!(plan.to_string().contains("b (alone: runs at an interval)"), "{plan}");
    }

    #[test]
    fn a_wrong_order_is_found_without_a_plan() {
        let ids = |names: &[&'static str]| -> Vec<(SystemId, &'static str)> {
            names
                .iter()
                .map(|&n| (SystemId::from_bits(n.as_bytes()[0].into()), n))
                .collect()
        };
        let systems = ids(&["a", "b", "c"]);
        let id = |i: usize| systems[i].0;
        let check = |order: &[(SystemId, SystemId)]| check_order(systems.iter().copied(), order);
        assert_eq!(check(&[]), Ok(()));
        assert_eq!(check(&[(id(0), id(1)), (id(1), id(2))]), Ok(()));
        let problem = check(&[(id(0), id(1)), (id(2), id(0))]).unwrap_err();
        assert_eq!((problem.first, problem.second), ("c", "a"));
        assert!(problem.to_string().contains("`a` already runs before `c`"), "{problem}");
        // A system can't be ordered after itself either.
        assert!(check(&[(id(1), id(1))]).is_err());
        // A pair that names a removed system is ignored.
        assert_eq!(check(&[(id(2), SystemId::from_bits(9999))]), Ok(()));
    }

    #[test]
    fn a_later_system_joins_an_earlier_stage_when_nothing_between_interacts_with_it() {
        // Producers and consumers registered in pairs: `c0` reads what
        // `p0` writes, `c1` what `p1` writes. Neighbours alone would give
        // `p0`, `c0+p1`, `c1`.
        #[derive(Component)]
        struct D;
        let p0 = SystemAccess::new().write::<A>();
        let c0 = SystemAccess::new().read::<A>().write::<B>();
        let p1 = SystemAccess::new().write::<C>();
        let c1 = SystemAccess::new().read::<C>().write::<D>();
        let plan = plan(&[view("p", &p0), view("c", &c0), view("q", &p1), view("d", &c1)], &[]);
        assert_eq!(stages(&plan), ["p+q", "c+d"]);
        assert_eq!(plan.stages[0].members, [0, 2]);
        assert_eq!((plan.stages[1].first, plan.stages[1].len), (1, 2));
        // Each system says what kept it out of the stage before.
        assert_eq!(plan.systems[2].boundary, Boundary::Phase);
        assert!(matches!(plan.systems[1].boundary, Boundary::Conflict { with: 0, .. }));
        assert!(matches!(plan.systems[3].boundary, Boundary::Conflict { with: 2, .. }));
        // The stage's reason is its first system's.
        assert_eq!(plan.stages[1].boundary, plan.systems[1].boundary);
        // The second consumer was kept out by another system than the
        // first: the printed plan names it.
        let text = plan.to_string();
        assert!(
            text.contains("  c + d (batch; after `p`: conflict on component"),
            "{text}"
        );
        assert!(text.contains("\n    d; after `q`: conflict on component"), "{text}");
    }

    #[test]
    fn a_system_never_moves_ahead_of_one_whose_order_would_show() {
        // `t` waits for `w`; `s` sends what `t` sends, so it may share
        // `t`'s batch but not run ahead of it: the events would come out in
        // another order.
        let w = SystemAccess::new().write::<A>();
        let t = SystemAccess::new().read::<A>().send_events::<Hit>();
        let s = SystemAccess::new().write::<B>().send_events::<Hit>();
        let plan = plan(&[view("w", &w), view("t", &t), view("s", &s)], &[]);
        assert_eq!(stages(&plan), ["w", "t+s"]);
        assert!(matches!(
            &plan.systems[2].boundary,
            Boundary::KeepsOrder { after: 1, conflicts } if conflicts == &[AccessConflict::Event(std::any::type_name::<Hit>())]
        ));
        assert!(
            plan.to_string()
                .contains("    s; not ahead of `t`: order shows on events"),
            "{plan}"
        );

        // A sender after a reader of its events: the reader must not see
        // them early, so the sender doesn't move ahead of it either.
        let r = SystemAccess::new().read::<A>().read_events::<Hit>();
        let send = SystemAccess::new().send_events::<Hit>();
        let plan = super::plan(&[view("w", &w), view("r", &r), view("s", &send)], &[]);
        assert_eq!(stages(&plan), ["w", "r+s"]);
    }

    #[test]
    fn nothing_moves_ahead_of_a_command_recorder_or_behind_its_batch() {
        let w = SystemAccess::new().write::<A>();
        let rec = SystemAccess::new().read::<A>().commands();
        let free = SystemAccess::new().write::<B>();
        // `free` interacts with nobody, but sees the world's structure: it
        // must come after the recorder's commands.
        let plan = plan(&[view("w", &w), view("r", &rec), view("f", &free)], &[]);
        assert_eq!(stages(&plan), ["w", "r", "f"]);
        assert!(matches!(
            &plan.systems[2].boundary,
            Boundary::Conflict { with: 1, conflicts } if conflicts == &[AccessConflict::Commands]
        ));
        // A recorder doesn't move ahead of earlier systems either.
        let plan = super::plan(&[view("w", &w), view("x", &rec.clone()), view("b", &free)], &[]);
        assert_eq!(plan.stages[0].members, [0]);
    }

    #[test]
    fn explicit_order_also_keeps_a_later_system_from_moving_ahead() {
        let (a, b) = (SystemAccess::new().write::<A>(), SystemAccess::new().read::<A>());
        let c = SystemAccess::new().write::<C>();
        let systems = [view("a", &a), view("b", &b), view("c", &c)];
        assert_eq!(stages(&plan(&systems, &[])), ["a+c", "b"]);
        // `c` must see `b`'s work.
        let p = plan(&systems, &[(systems[1].id, systems[2].id)]);
        assert_eq!(stages(&p), ["a", "b", "c"]);
        assert_eq!(p.systems[2].boundary, Boundary::Order { after: 1 });
    }

    #[test]
    fn systems_never_move_across_a_system_that_runs_alone_or_a_barrier() {
        let (a, b) = (SystemAccess::new().write::<A>(), SystemAccess::new().read::<A>());
        let c = SystemAccess::new().write::<C>();
        let mut ctx_system = view("x", &c);
        ctx_system.view = false;
        let plan = plan(&[view("a", &a), view("b", &b), ctx_system, view("c", &c)], &[]);
        assert_eq!(stages(&plan), ["a", "b", "x", "c"]);
        let mut after_barrier = view("c", &c);
        after_barrier.barrier = true;
        let plan = super::plan(&[view("a", &a), view("b", &b), after_barrier], &[]);
        assert_eq!(stages(&plan), ["a", "b", "c"]);
    }

    /// A seeded generator for the randomized plan test (SplitMix64).
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) % n
        }
    }

    fn random_access(rng: &mut Rng) -> SystemAccess {
        struct E0;
        struct E1;
        struct R0;
        let mut a = SystemAccess::new();
        for _ in 0..1 + rng.below(3) {
            a = match rng.below(12) {
                0 => a.read::<A>(),
                1 => a.write::<A>(),
                2 => a.read::<B>(),
                3 => a.write::<B>(),
                4 => a.read::<C>(),
                5 => a.write::<C>(),
                6 => a.read_resource::<R0>(),
                7 => a.write_resource::<R0>(),
                8 => a.send_events::<E0>(),
                9 => a.read_events::<E1>(),
                10 => a.send_events::<E1>(),
                _ => {
                    if rng.below(3) == 0 {
                        a.commands()
                    } else {
                        a.read_events::<E0>()
                    }
                }
            };
        }
        a
    }

    /// Randomly generated schedules keep every rule: the plan is checked
    /// against the rules themselves, pair by pair, not against another
    /// planner.
    #[test]
    fn random_schedules_never_batch_or_reorder_conflicting_systems() {
        let mut moved = 0;
        for seed in 0..400u64 {
            let mut rng = Rng(seed);
            let n = 2 + rng.below(12) as usize;
            let accesses: Vec<SystemAccess> = (0..n).map(|_| random_access(&mut rng)).collect();
            let systems: Vec<PlanInput<'_>> = (accesses.iter().enumerate())
                .map(|(i, a)| PlanInput {
                    name: "s",
                    phase: if rng.below(6) == 0 {
                        Phase::SimLast
                    } else {
                        Phase::AfterPhysics
                    },
                    list: 0,
                    id: SystemId::from_bits(i as u32 + 1),
                    access: (rng.below(10) != 0).then_some(a),
                    view: rng.below(8) != 0,
                    interval: false,
                    barrier: rng.below(10) == 0,
                })
                .collect();
            let mut systems = systems;
            // Phases in run order, as the game lists them.
            systems.sort_by_key(|s| s.phase);
            let mut order = Vec::new();
            for _ in 0..rng.below(3) {
                let (a, b) = (rng.below(n as u64) as usize, rng.below(n as u64) as usize);
                if a < b && systems[a].phase == systems[b].phase {
                    order.push((systems[a].id, systems[b].id));
                }
            }
            let plan = plan(&systems, &order);
            let ctx = |what: &str| format!("seed {seed}: {what}\n{plan}");

            // Every system is in exactly one stage, members in registration
            // order, stages in phase order.
            let mut stage_of = vec![usize::MAX; n];
            for (k, stage) in plan.stages.iter().enumerate() {
                assert!(
                    stage.members.windows(2).all(|w| w[0] < w[1]),
                    "{}",
                    ctx("members out of order")
                );
                assert_eq!((stage.first, stage.len), (stage.members[0], stage.members.len()));
                for &m in &stage.members {
                    assert_eq!(stage_of[m], usize::MAX, "{}", ctx("a system in two stages"));
                    assert_eq!(systems[m].phase, stage.phase);
                    stage_of[m] = k;
                }
                assert!(
                    !matches!(stage.boundary, Boundary::KeepsOrder { .. }),
                    "{}",
                    ctx("stage kept")
                );
            }
            assert!(
                stage_of.iter().all(|&k| k != usize::MAX),
                "{}",
                ctx("a system in no stage")
            );

            // Segments: a phase, a barrier or a system that runs alone cuts.
            let batchable = |i: usize| why_alone(&systems[i]).is_none();
            let mut segment = vec![0usize; n];
            for i in 1..n {
                let cut = systems[i].phase != systems[i - 1].phase
                    || systems[i].barrier
                    || !batchable(i)
                    || !batchable(i - 1);
                segment[i] = segment[i - 1] + usize::from(cut);
            }
            for s in 0..n {
                let alone = why_alone(&systems[s]).is_some();
                if alone {
                    assert_eq!(
                        plan.stages[stage_of[s]].len,
                        1,
                        "{}",
                        ctx("a system that runs alone in a batch")
                    );
                }
                for t in 0..s {
                    let (st, ss) = (stage_of[t], stage_of[s]);
                    // Anything between them that cuts the order keeps them in
                    // order: a phase, a barrier, a system that runs alone.
                    if segment[t] != segment[s] {
                        assert!(st < ss, "{}", ctx(&format!("{s} moved across a cut before {t}")));
                        continue;
                    }
                    let (a, b) = (systems[t].access.unwrap(), systems[s].access.unwrap());
                    let ordered = order.contains(&(systems[t].id, systems[s].id));
                    if ordered || !a.conflicts_before(b).is_empty() {
                        assert!(st < ss, "{}", ctx(&format!("{s} must see {t}'s work")));
                    } else if !a.conflicts_with(b).is_empty() {
                        assert!(st <= ss, "{}", ctx(&format!("{s} moved ahead of {t}")));
                    }
                    if ss < st {
                        moved += 1;
                    }
                }
                // As early as allowed: the system's reason names a system
                // that really keeps it out of the stage before.
                let k = stage_of[s];
                match &plan.systems[s].boundary {
                    Boundary::Conflict { with, conflicts } => {
                        assert_eq!(stage_of[*with] + 1, k, "{}", ctx("conflict not in the stage before"));
                        let (a, b) = (systems[*with].access.unwrap(), systems[s].access.unwrap());
                        assert_eq!(&a.conflicts_before(b), conflicts);
                    }
                    Boundary::Order { after } => {
                        assert_eq!(stage_of[*after] + 1, k);
                        assert!(order.contains(&(systems[*after].id, systems[s].id)));
                    }
                    Boundary::KeepsOrder { after, conflicts } => {
                        assert_eq!(stage_of[*after], k);
                        assert!(*after < s);
                        let (a, b) = (systems[*after].access.unwrap(), systems[s].access.unwrap());
                        assert_eq!(&a.conflicts_with(b), conflicts);
                    }
                    // The first stage of its segment.
                    Boundary::Phase | Boundary::Alone | Boundary::Barrier => {
                        let first = (0..n).filter(|&j| segment[j] == segment[s]).map(|j| stage_of[j]).min();
                        assert_eq!(first, Some(k), "{}", ctx(&format!("{s} could have gone earlier")));
                    }
                }
            }
        }
        // The generator must make systems move, or nothing was tested.
        assert!(moved > 50, "only {moved} systems moved ahead");
    }

    #[test]
    fn the_plan_prints_every_stage_with_its_reason() {
        let (a, b, c) = (
            SystemAccess::new().write::<A>(),
            SystemAccess::new().write::<B>(),
            SystemAccess::new().read::<A>(),
        );
        let mut alone = view("u", &a);
        alone.access = None;
        let text = plan(&[view("a", &a), view("b", &b), view("c", &c), alone], &[]).to_string();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "AfterPhysics");
        assert_eq!(lines[1], "  a + b (batch)");
        assert!(lines[2].starts_with("  c (batch of one; after `a`: conflict on component"));
        assert_eq!(lines[3], "  u (alone: undeclared)");
    }
}
