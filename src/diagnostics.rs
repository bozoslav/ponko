//! ECS diagnostics for tools and agents: archetype layouts, component
//! storage and the system schedule, as stable, machine-readable data.
//!
//! [`Ctx::ecs_diagnostics`] builds a snapshot from the world and the
//! schedule only when asked (by game code, a test, or the control
//! endpoint's `ecs.diagnostics`): the schedule plan, the declared accesses
//! as names and the dependencies between systems are all worked out then.
//! A running game keeps only what it has anyway (system timings, the
//! world's query index counters) plus two counters per batch of the
//! parallel scheduler, so a game that never asks pays nothing measurable.
//!
//! The schedule part answers "why doesn't this system run at the same time
//! as that one?": every system has its [`declared`](SystemDiagnostics::declared)
//! access, why it runs [`alone`](SystemDiagnostics::alone) if it does, and
//! its [`stage`](SystemDiagnostics::stage);
//! [`schedule`](EcsDiagnostics::schedule) lists the stages with the reason
//! each one starts where it does, and which systems must stay in order.
//!
//! ```
//! use ponko::access::SystemAccess;
//! use ponko::prelude::*;
//! use ponko::system_view::SystemView;
//! #[derive(Component)]
//! struct Pos(f32);
//! #[derive(Component)]
//! struct Vel(f32);
//! fn movement(_: &mut SystemView) {}
//! fn friction(_: &mut SystemView) {}
//! fn check(ctx: &mut Ctx) {
//!     let d = ctx.ecs_diagnostics();
//!     // `friction` writes what `movement` reads: it starts a new stage.
//!     let friction = d.system("friction").unwrap();
//!     let stage = &d.schedule.stages[friction.stage.unwrap()];
//!     assert_eq!(stage.boundary, "conflict");
//!     assert_eq!(stage.conflicts[0].kind, "component");
//!     assert!(stage.conflicts[0].name.as_deref().unwrap().ends_with("Vel"));
//!     // `check` itself takes `&mut Ctx` without a declaration.
//!     assert_eq!(d.system("check").unwrap().alone.as_ref().unwrap().reason, "undeclared");
//! }
//! Game::new("t", 1, 1)
//!     .add(Phase::AfterPhysics, movement.access(SystemAccess::new().write::<Pos>().read::<Vel>()))
//!     .add(Phase::AfterPhysics, friction.access(SystemAccess::new().write::<Vel>()))
//!     .add(Phase::SimLast, check)
//!     .run_headless(1, |_| {});
//! ```

use serde::{Deserialize, Serialize};

use crate::access::{AccessConflict, AccessReport, ReportedSystem, SystemAccess};
use crate::ctx::Ctx;
use crate::game::{Phase, SystemId};
use crate::perf::SystemTiming;
use crate::schedule::{Alone, Boundary, PlanInput, SchedulePlan};

/// The schema version of [`EcsDiagnostics`]. Field names are stable; new
/// fields may be added without a bump. Renaming or removing one bumps it.
pub const ECS_DIAGNOSTICS_VERSION: u32 = 1;

/// A snapshot of the ECS: archetypes, component storage and the systems in
/// run order with their last timings. See [`Ctx::ecs_diagnostics`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct EcsDiagnostics {
    /// [`ECS_DIAGNOSTICS_VERSION`] of the producer.
    pub version: u32,
    /// Live entities.
    pub entities: usize,
    /// Bytes allocated for component storage in all archetypes.
    pub allocated_bytes: usize,
    /// Every archetype, empty ones included, in the world's order.
    pub archetypes: Vec<ArchetypeDiagnostics>,
    /// Every registered component type.
    pub components: Vec<ComponentDiagnostics>,
    /// Every system, in the order the schedule runs them.
    pub systems: Vec<SystemDiagnostics>,
    /// The stages the systems run in, and which systems must stay in order.
    pub schedule: ScheduleDiagnostics,
    /// The world's archetype index for queries: what it holds and how often
    /// it answered.
    pub query_cache: QueryCacheDiagnostics,
}

/// One archetype: entities sharing exactly one set of component types.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ArchetypeDiagnostics {
    /// The archetype's index in the world (stable while the world lives).
    pub index: usize,
    /// Full type names of its components, in storage order.
    pub components: Vec<String>,
    /// Entities stored in it now.
    pub entities: usize,
    /// Bytes per entity: every component plus the entity handle.
    pub bytes_per_entity: usize,
    /// Bytes its component columns have allocated (capacity, not length).
    pub allocated_bytes: usize,
}

/// One component type and how much of it is stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ComponentDiagnostics {
    /// Full type name (`my_game::units::Health`), or the registered name.
    pub name: String,
    /// The last path segment (`Health`), as the control endpoint's other
    /// methods name components.
    pub short_name: String,
    /// Size of one value in bytes.
    pub size: usize,
    /// Alignment in bytes.
    pub align: usize,
    /// Archetypes that contain it.
    pub archetypes: usize,
    /// Entities that have it.
    pub entities: usize,
    /// `size * entities`: bytes of live values (excluding spare capacity).
    pub bytes: usize,
}

/// One registered system.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct SystemDiagnostics {
    /// The system's function name (`my_game::movement`).
    pub name: String,
    /// Its [`Phase`], spelled like the variant (`AfterPhysics`).
    pub phase: String,
    /// Position in the schedule: systems run in increasing `order`
    /// (simulation systems once per tick, the rest once per frame).
    pub order: usize,
    /// `true` for fixed-tick simulation systems.
    pub sim: bool,
    /// Simulation systems run on ticks where `tick % every == offset`.
    pub every: u32,
    /// See `every`.
    pub offset: u32,
    /// What the system may touch, in one word:
    /// - `exclusive`: anything. It has no declaration (or declares
    ///   [`SystemAccess::exclusive`]), so nothing can run beside it.
    /// - `declared`: a `&mut Ctx` system with a declaration
    ///   ([`IntoSystem::access`](crate::game::IntoSystem::access), C and C++
    ///   systems). The declaration is only checked in debug builds, so it
    ///   still runs alone.
    /// - `view`: a view system (one that takes `&mut SystemView`; see
    ///   [`Game::add`](crate::game::Game::add)). It can reach nothing beyond its
    ///   declaration and may share a batch.
    pub access: String,
    /// The declaration, by name; `None` for an undeclared system.
    pub declared: Option<AccessDiagnostics>,
    /// Index into [`ScheduleDiagnostics::stages`] of the stage that runs
    /// it. `None` before the game has listed its systems.
    pub stage: Option<usize>,
    /// Why it runs alone, as a stage of its own; `None` for a view system
    /// that can share a batch (whether or not it has company).
    pub alone: Option<AloneDiagnostics>,
    /// Why it isn't in an earlier stage: which system kept it out of the
    /// stage before its own, and on what. `None` before the game has listed
    /// its systems.
    pub blocked: Option<BlockedDiagnostics>,
    /// Time in the last frame (all of its ticks together), in ms. In a
    /// batch this is the system's own run time on whichever thread ran it.
    pub last_ms: f32,
    /// Smoothed average of `last_ms`.
    pub avg_ms: f32,
}

/// What keeps a system (or a stage's first system) out of the stage before
/// its own ([`Boundary`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct BlockedDiagnostics {
    /// - `phase`: it is in the first stage of its phase.
    /// - `alone`: it runs alone, or the system before it does.
    /// - `conflict`: it must see the work of system `after`, which is in
    ///   the stage before, on `conflicts`.
    /// - `barrier`: an explicit [`Game::barrier`](crate::game::Game::barrier).
    /// - `order`: [`Game::order`](crate::game::Game::order) puts it after
    ///   system `after`.
    /// - `keeps_order`: it may not run ahead of system `after`, registered
    ///   earlier and in the same stage, because their order shows on
    ///   `conflicts` (two senders of one event type, say). Only for systems,
    ///   never for a stage.
    pub boundary: String,
    /// `order` of the earlier system, for `conflict`, `order` and
    /// `keeps_order`.
    pub after: Option<usize>,
    /// What the two systems conflict on, for `conflict` and `keeps_order`.
    pub conflicts: Vec<ConflictDiagnostics>,
}

/// What a system declares it reads and writes ([`SystemAccess`]), with
/// full type names (or the registered names of C and C++ types).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct AccessDiagnostics {
    /// Declared exclusive: it may touch anything, the lists below are empty.
    pub exclusive: bool,
    /// Components it only reads.
    pub components_read: Vec<String>,
    /// Components it writes (and may read).
    pub components_written: Vec<String>,
    /// Resources it only reads.
    pub resources_read: Vec<String>,
    /// Resources it writes (and may read).
    pub resources_written: Vec<String>,
    /// Event types it reads.
    pub events_read: Vec<String>,
    /// Event types it sends.
    pub events_sent: Vec<String>,
    /// Services ([`Service`](crate::access::Service)) it only reads.
    pub services_read: Vec<String>,
    /// Services it writes.
    pub services_written: Vec<String>,
    /// It records deferred commands (spawn, despawn, insert, remove).
    pub commands: bool,
}

/// Why a system runs alone. See [`Alone`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct AloneDiagnostics {
    /// - `undeclared`: no declaration, it may use anything.
    /// - `ctx_system`: declared, but it takes `&mut Ctx`.
    /// - `service`: a view system holding a service (`item`) that only one
    ///   system at a time may hold.
    /// - `sends_and_reads`: a view system that sends and reads one event
    ///   type (`item`).
    /// - `interval`: a view system that doesn't run every tick.
    pub reason: String,
    /// The service or event type, for the reasons that name one.
    pub item: Option<String>,
}

/// One reason two systems can't run at the same time. See [`AccessConflict`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct ConflictDiagnostics {
    /// - `exclusive`: one of them may touch anything.
    /// - `component`, `resource`, `service`: both use `name`, one writes it.
    /// - `event`: one sends event type `name` and the other reads or sends it.
    /// - `commands`: the earlier one records commands, which change what
    ///   the later one may see.
    /// - `between`: system `name` is registered between them and must stay
    ///   there.
    pub kind: String,
    /// The component, resource, event type, service or system.
    pub name: Option<String>,
}

/// How the systems are cut into stages. See [`SchedulePlan`].
///
/// A stage is one system alone or a batch of view systems with no
/// conflict; a batch's systems need not be neighbours (see
/// [`SchedulePlan`]). **Every stage ends with a barrier**: the commands its
/// systems recorded are applied and the events they sent become visible
/// before the next stage starts. The stages are listed whether or not the
/// parallel scheduler is on; with it off, each system runs alone in
/// `order`, with the same barrier after every system.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct ScheduleDiagnostics {
    /// The parallel scheduler is on
    /// ([`Game::parallel_systems`](crate::game::Game::parallel_systems) or the
    /// environment): batches really share the world.
    pub parallel: bool,
    /// There are other threads for a batch to go to. `false` with the
    /// scheduler off, on the web and on one core.
    pub threads: bool,
    /// A batch goes to other threads when the usual run time of its
    /// systems, without the slowest, reaches this many microseconds.
    pub parallel_threshold_us: f32,
    /// The stages, in run order; together they cover every system once.
    pub stages: Vec<StageDiagnostics>,
    /// Pairs of declared systems in one phase that must stay in order,
    /// with what they conflict on. Undeclared systems are left out: they
    /// conflict with everything.
    pub dependencies: Vec<DependencyDiagnostics>,
}

/// One stage: the systems `systems` of [`EcsDiagnostics::systems`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct StageDiagnostics {
    /// Its position in [`ScheduleDiagnostics::stages`].
    pub index: usize,
    /// The [`Phase`] of its systems.
    pub phase: String,
    /// `order` of its first system.
    pub first: usize,
    /// How many systems it has.
    pub len: usize,
    /// `order` of each of its systems, in registration order. They need not
    /// be neighbours: a system that doesn't interact with the ones
    /// registered before it may run in an earlier stage than theirs.
    pub systems: Vec<usize>,
    /// More than one system shares it.
    pub batch: bool,
    /// Why it starts here, that is, what keeps its first system out of the
    /// stage before it ([`Boundary`]):
    /// - `phase`: it is the first stage of its phase.
    /// - `alone`: its system runs alone, or the one before it does.
    /// - `conflict`: its first system conflicts with system `after` of the
    ///   batch before it, on `conflicts`.
    /// - `barrier`: an explicit [`Game::barrier`](crate::game::Game::barrier).
    /// - `order`: [`Game::order`](crate::game::Game::order) puts its first system
    ///   after system `after`.
    pub boundary: String,
    /// `order` of the earlier system, for `conflict` and `order`.
    pub after: Option<usize>,
    /// What the two systems conflict on, for `conflict`.
    pub conflicts: Vec<ConflictDiagnostics>,
    /// Times the batch went to other threads so far. Always 0 for a
    /// single system and with the scheduler off.
    pub threaded_runs: u64,
    /// Times the batch ran in order on the game's thread: its work was
    /// under the threshold, there were no other threads, or the game was
    /// profiling.
    pub inline_runs: u64,
    /// The usual run time of the batch's systems together, in
    /// microseconds, when it last ran; what the threshold is compared
    /// with, less the slowest system. 0 until it has run.
    pub usual_work_us: f32,
}

/// Two systems that must stay in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct DependencyDiagnostics {
    /// `order` of the earlier system.
    pub first: usize,
    /// `order` of the later system, which must see the earlier one's work.
    pub second: usize,
    /// Why they can't run at the same time.
    pub conflicts: Vec<ConflictDiagnostics>,
}

/// The world's archetype index for queries
/// ([`World::query_cache_stats`](ponko_ecs::World::query_cache_stats)).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct QueryCacheDiagnostics {
    /// `false` if the world's policy turns the index off.
    pub enabled: bool,
    /// The world needs this many archetypes before any query is indexed.
    pub min_archetypes: usize,
    /// Query signatures remembered, indexed or not.
    pub signatures: usize,
    /// Signatures that have an index.
    pub indexed: usize,
    /// Archetype indices held, over all signatures.
    pub indices: usize,
    /// Heap bytes the index owns.
    pub bytes: usize,
    /// Queries answered from an index so far (view systems' queries are
    /// not counted).
    pub hits: u64,
    /// Signatures dropped to make room so far. If it keeps growing, the
    /// game runs more query signatures than the policy allows.
    pub evictions: u64,
}

/// How many archetypes and entities have a set of components.
/// See [`EcsDiagnostics::matching`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MatchCount {
    /// Non-empty archetypes that have every named component.
    pub archetypes: usize,
    /// Entities in them.
    pub entities: usize,
}

impl EcsDiagnostics {
    /// The first system whose name is `name` or ends with `::name`.
    #[must_use]
    pub fn system(&self, name: &str) -> Option<&SystemDiagnostics> {
        self.systems
            .iter()
            .find(|s| s.name == name || (s.name.strip_suffix(name)).is_some_and(|rest| rest.ends_with("::")))
    }

    /// How many archetypes and entities have all of `components`, named by
    /// full or short name: what a query with those components would visit.
    /// An unknown name is an error that lists the known ones.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// #[derive(Component)]
    /// struct Hp(u32);
    /// fn check(ctx: &mut Ctx) {
    ///     let m = ctx.ecs_diagnostics().matching(&["Hp"]).unwrap();
    ///     assert_eq!((m.archetypes, m.entities), (1, 1));
    /// }
    /// Game::new("t", 1, 1).sim_system(check).run_headless(1, |ctx| {
    ///     ctx.world.spawn((Hp(3),));
    /// });
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) for a name no
    /// component has (the message lists the known ones).
    pub fn matching(&self, components: &[&str]) -> Result<MatchCount, crate::error::Error> {
        let mut full: Vec<&str> = Vec::with_capacity(components.len());
        for &wanted in components {
            let found = self
                .components
                .iter()
                .find(|c| c.name == wanted)
                .or_else(|| self.components.iter().find(|c| c.short_name == wanted));
            match found {
                Some(c) => full.push(&c.name),
                None => {
                    let known: Vec<&str> = self.components.iter().map(|c| c.short_name.as_str()).collect();
                    return Err(crate::error::Error::new(
                        crate::error::ErrorKind::NotFound,
                        format!("unknown component `{wanted}` (known: {})", known.join(", ")),
                    ));
                }
            }
        }
        let mut count = MatchCount::default();
        for a in self.archetypes.iter().filter(|a| a.entities > 0) {
            if full.iter().all(|n| a.components.iter().any(|c| c == n)) {
                count.archetypes += 1;
                count.entities += a.entities;
            }
        }
        Ok(count)
    }
}

/// Where a system sits in the schedule, recorded when the game lists its
/// systems (parallel to [`Perf::system_timings`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScheduleSlot {
    pub phase: Phase,
    pub every: u32,
    pub offset: u32,
}

/// One registered system as the schedule planner takes it, owned.
#[derive(Debug, Clone)]
pub(crate) struct PlanSystem {
    pub name: &'static str,
    pub phase: Phase,
    pub list: u8,
    pub id: SystemId,
    pub access: Option<SystemAccess>,
    pub view: bool,
    pub interval: bool,
    pub barrier: bool,
}

/// What the schedule plan is built from, kept by `Perf` when the game
/// lists its systems (parallel to [`Perf::system_timings`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct PlanSource {
    pub systems: Vec<PlanSystem>,
    /// From `Game::order`.
    pub order: Vec<(SystemId, SystemId)>,
    /// The scheduler's resolved settings.
    pub parallel: bool,
    pub threads: bool,
    pub threshold_us: f32,
}

/// How a batch was dispatched so far. See [`StageDiagnostics`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Dispatch {
    pub threaded_runs: u64,
    pub inline_runs: u64,
    pub usual_work_us: f32,
}

/// `Phase::AfterPhysics` → `"AfterPhysics"`, a stable spelling for tools.
pub(crate) fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Input => "Input",
        Phase::BeforePhysics => "BeforePhysics",
        Phase::Physics => "Physics",
        Phase::AfterPhysics => "AfterPhysics",
        Phase::SimLast => "SimLast",
        Phase::Frame => "Frame",
        Phase::Animate => "Animate",
        Phase::Late => "Late",
        Phase::Draw => "Draw",
    }
}

/// `crate::units::Health` → `Health`.
pub(crate) fn short_name(type_name: &str) -> &str {
    let end = type_name.find('<').unwrap_or(type_name.len());
    type_name[..end].rfind("::").map_or(type_name, |i| &type_name[i + 2..])
}

pub(crate) fn snapshot(ctx: &Ctx) -> EcsDiagnostics {
    let world = &ctx.world;
    let infos = world.components();
    let mut components: Vec<ComponentDiagnostics> = infos
        .iter()
        .map(|info| ComponentDiagnostics {
            name: info.name().to_string(),
            short_name: short_name(info.name()).to_string(),
            size: info.layout().size(),
            align: info.layout().align(),
            ..ComponentDiagnostics::default()
        })
        .collect();
    let archetypes = world
        .raw_ref()
        .archetypes()
        .iter()
        .enumerate()
        .map(|(index, a)| {
            for id in a.component_ids() {
                let c = &mut components[id.index()];
                c.archetypes += 1;
                c.entities += a.len();
            }
            ArchetypeDiagnostics {
                index,
                components: a
                    .component_ids()
                    .iter()
                    .map(|&id| world.component_info(id).name().to_string())
                    .collect(),
                entities: a.len(),
                bytes_per_entity: a.bytes_per_entity(),
                allocated_bytes: a.allocated_bytes(),
            }
        })
        .collect();
    for c in &mut components {
        c.bytes = c.size * c.entities;
    }
    let source = &ctx.perf.plan_source;
    let plan = plan_of(source);
    let (policy, stats) = (world.query_cache_policy(), world.query_cache_stats());
    let query_cache = QueryCacheDiagnostics {
        enabled: !policy.is_off(),
        min_archetypes: policy.min_archetypes,
        signatures: stats.signatures,
        indexed: stats.indexed,
        indices: stats.indices,
        bytes: stats.bytes,
        hits: stats.hits,
        evictions: stats.evictions,
    };
    EcsDiagnostics {
        version: ECS_DIAGNOSTICS_VERSION,
        entities: world.len(),
        allocated_bytes: world.allocated_bytes(),
        archetypes,
        components,
        systems: systems(&ctx.perf.timings, &ctx.perf.schedule, source, plan.as_ref()),
        schedule: schedule(source, plan.as_ref(), &ctx.perf.dispatch),
        query_cache,
    }
}

/// The plan for `source`, unless there is nothing to plan or a
/// `Game::order` pair is wrong (the game panics on that when it starts).
fn plan_of(source: &PlanSource) -> Option<SchedulePlan> {
    let ids = source.systems.iter().map(|s| (s.id, s.name));
    if source.systems.is_empty() || crate::schedule::check_order(ids, &source.order).is_err() {
        return None;
    }
    let inputs: Vec<PlanInput<'_>> = (source.systems.iter())
        .map(|s| PlanInput {
            name: s.name,
            phase: s.phase,
            list: s.list,
            id: s.id,
            access: s.access.as_ref(),
            view: s.view,
            interval: s.interval,
            barrier: s.barrier,
        })
        .collect();
    Some(crate::schedule::plan(&inputs, &source.order))
}

fn conflict(c: &AccessConflict) -> ConflictDiagnostics {
    let (kind, name) = match c {
        AccessConflict::Exclusive => ("exclusive", None),
        AccessConflict::Component(n) => ("component", Some((*n).to_string())),
        AccessConflict::Resource(n) => ("resource", Some((*n).to_string())),
        AccessConflict::Event(n) => ("event", Some((*n).to_string())),
        AccessConflict::Commands => ("commands", None),
        AccessConflict::Service(s) => ("service", Some(s.name().to_string())),
        AccessConflict::Between(n) => ("between", Some((*n).to_string())),
    };
    ConflictDiagnostics {
        kind: kind.to_string(),
        name,
    }
}

fn alone(why: Alone) -> AloneDiagnostics {
    let (reason, item) = match why {
        Alone::Undeclared => ("undeclared", None),
        Alone::CtxSystem => ("ctx_system", None),
        Alone::Service(s) => ("service", Some(s.name().to_string())),
        Alone::SendsAndReads(n) => ("sends_and_reads", Some(n.to_string())),
        Alone::Interval => ("interval", None),
    };
    AloneDiagnostics {
        reason: reason.to_string(),
        item,
    }
}

/// `boundary` as diagnostics.
fn blocked(boundary: &Boundary) -> BlockedDiagnostics {
    let (kind, after, conflicts) = match boundary {
        Boundary::Phase => ("phase", None, Vec::new()),
        Boundary::Alone => ("alone", None, Vec::new()),
        Boundary::Conflict { with, conflicts } => ("conflict", Some(*with), conflicts.iter().map(conflict).collect()),
        Boundary::Barrier => ("barrier", None, Vec::new()),
        Boundary::Order { after } => ("order", Some(*after), Vec::new()),
        Boundary::KeepsOrder { after, conflicts } => {
            ("keeps_order", Some(*after), conflicts.iter().map(conflict).collect())
        }
    };
    BlockedDiagnostics {
        boundary: kind.to_string(),
        after,
        conflicts,
    }
}

fn schedule(source: &PlanSource, plan: Option<&SchedulePlan>, dispatch: &[Dispatch]) -> ScheduleDiagnostics {
    let stages = plan.map_or_else(Vec::new, |plan| {
        (plan.stages.iter().enumerate())
            .map(|(index, stage)| {
                let BlockedDiagnostics {
                    boundary,
                    after,
                    conflicts,
                } = blocked(&stage.boundary);
                let ran = dispatch.get(stage.first).copied().unwrap_or_default();
                StageDiagnostics {
                    index,
                    phase: phase_name(stage.phase).to_string(),
                    first: stage.first,
                    len: stage.len,
                    systems: stage.members.clone(),
                    batch: stage.is_batch(),
                    boundary,
                    after,
                    conflicts,
                    threaded_runs: ran.threaded_runs,
                    inline_runs: ran.inline_runs,
                    usual_work_us: ran.usual_work_us,
                }
            })
            .collect()
    });
    let report = AccessReport::new(
        (source.systems.iter())
            .map(|s| ReportedSystem {
                id: s.id,
                name: s.name,
                phase: s.phase,
                access: s.access.clone(),
                enforced: s.view,
            })
            .collect(),
    );
    let dependencies = (report.pairs.iter())
        .filter(|p| !p.conflicts.is_empty())
        .map(|p| DependencyDiagnostics {
            first: p.first,
            second: p.second,
            conflicts: p.conflicts.iter().map(conflict).collect(),
        })
        .collect();
    ScheduleDiagnostics {
        parallel: source.parallel,
        threads: source.threads,
        parallel_threshold_us: source.threshold_us,
        stages,
        dependencies,
    }
}

fn systems(
    timings: &[SystemTiming],
    schedule: &[ScheduleSlot],
    source: &PlanSource,
    plan: Option<&SchedulePlan>,
) -> Vec<SystemDiagnostics> {
    timings
        .iter()
        .enumerate()
        .map(|(order, t)| {
            // A game that hasn't listed its systems yet (or a bare `Ctx`)
            // has timings without slots; report the compatibility default.
            let slot = schedule.get(order).copied().unwrap_or(ScheduleSlot {
                phase: if t.sim { Phase::AfterPhysics } else { Phase::Frame },
                every: 1,
                offset: 0,
            });
            let planned = source.systems.get(order);
            let declared = planned.and_then(|s| s.access.as_ref());
            let access = match (declared, planned.is_some_and(|s| s.view)) {
                (Some(a), _) if a.is_exclusive() => "exclusive",
                (Some(_), true) => "view",
                (Some(_), false) => "declared",
                (None, _) => "exclusive",
            };
            SystemDiagnostics {
                name: t.name.to_string(),
                phase: phase_name(slot.phase).to_string(),
                order,
                sim: t.sim,
                every: slot.every,
                offset: slot.offset,
                access: access.to_string(),
                declared: declared.map(SystemAccess::describe),
                stage: plan.and_then(|p| p.stages.iter().position(|s| s.systems().contains(&order))),
                alone: match plan {
                    Some(p) => p.systems.get(order).and_then(|s| s.alone).map(alone),
                    // Not planned yet: every system runs alone, undeclared
                    // as far as anyone knows.
                    None => Some(alone(Alone::Undeclared)),
                },
                blocked: plan.and_then(|p| p.systems.get(order)).map(|s| blocked(&s.boundary)),
                last_ms: t.last_ms,
                avg_ms: t.avg_ms,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ctx_without_a_game_reports_systems_alone_and_no_stages() {
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::ZERO);
        ctx.perf.timings = vec![SystemTiming {
            name: "game::tick",
            sim: true,
            last_ms: 0.0,
            avg_ms: 0.0,
        }];
        let d = ctx.ecs_diagnostics();
        assert_eq!(d.systems.len(), 1);
        assert_eq!(d.systems[0].access, "exclusive");
        assert_eq!((d.systems[0].stage, d.systems[0].declared.as_ref()), (None, None));
        assert_eq!(d.systems[0].alone.as_ref().unwrap().reason, "undeclared");
        assert!(d.schedule.stages.is_empty() && d.schedule.dependencies.is_empty());
        assert!(!d.schedule.parallel);
    }

    #[test]
    fn systems_are_found_by_full_name_or_last_segment() {
        let named = |name: &str| SystemDiagnostics {
            name: name.to_string(),
            ..SystemDiagnostics::default()
        };
        let d = EcsDiagnostics {
            systems: vec![named("game::unmovement"), named("game::movement")],
            ..EcsDiagnostics::default()
        };
        assert_eq!(d.system("movement").unwrap().name, "game::movement");
        assert_eq!(d.system("game::unmovement").unwrap().name, "game::unmovement");
        assert!(d.system("ovement").is_none());
    }
}
