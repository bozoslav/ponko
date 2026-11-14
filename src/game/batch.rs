//! Runs the stages of a [`SchedulePlan`]: systems that run alone, and
//! batches of view systems that share the world.
//!
//! A batch runs the same way whether its systems go to other threads or
//! stay on this one: every system gets its view of the shared world, its
//! own command buffer and an outbox for its events, and all of that is
//! delivered when the batch ends, in registration order. So the choice
//! between threads and this thread only changes speed.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::time::Duration;

use ponko_ecs::ViewGroup;
use web_time::Instant;

use super::{ChangesSeen, Run, System, run_one};
use crate::ctx::Ctx;
use crate::events::EventSlot;
use crate::random::Random;
use crate::schedule::{Boundary, DEFAULT_PARALLEL_THRESHOLD, SchedulePlan};
use crate::system_view::{Shared, ViewRunner};

/// Turns the scheduler on or off whatever the game says: `1`, `0`, or
/// `always` (on, and every batch goes to the thread pool).
const ENV_PARALLEL_SYSTEMS: &str = "PONKO_PARALLEL_SYSTEMS";

/// How the systems run, from the builder and the environment.
#[derive(Clone, Copy)]
pub(super) struct Settings {
    /// Batches exist: view systems that don't conflict share the world.
    pub(super) on: bool,
    /// A batch goes to other threads when the usual run time of its
    /// systems, without the slowest, is at least this many microseconds.
    threshold_us: f32,
    /// There are other threads to go to.
    threads: bool,
}

impl Settings {
    /// The threshold in microseconds, for diagnostics.
    pub(super) fn threshold_us(self) -> f32 {
        self.threshold_us
    }

    /// Whether batches have other threads to go to, for diagnostics.
    pub(super) fn threads(self) -> bool {
        self.threads
    }
}

impl Settings {
    /// `env`: whether the environment variable may override the builder.
    pub(super) fn new(on: bool, threshold: Option<Duration>, env: bool) -> Self {
        let value = env.then(|| std::env::var(ENV_PARALLEL_SYSTEMS).ok()).flatten();
        let (on, threshold) = resolve(on, threshold, value.as_deref());
        Settings {
            on,
            threshold_us: threshold.as_secs_f32() * 1e6,
            // Only asked when the scheduler is on: the question starts
            // rayon's global thread pool, which a game that never turns
            // the scheduler on must not get from here. And the web has no
            // threads; asking there would set up a pool of the page's own.
            threads: on && !cfg!(target_arch = "wasm32") && rayon::current_num_threads() > 1,
        }
    }
}

/// The builder's settings after the environment variable's `value`.
pub(super) fn resolve(on: bool, threshold: Option<Duration>, value: Option<&str>) -> (bool, Duration) {
    let threshold = threshold.unwrap_or(DEFAULT_PARALLEL_THRESHOLD);
    match value {
        Some("1") => (true, threshold),
        Some("0") => (false, threshold),
        Some("always") => (true, Duration::ZERO),
        _ => (on, threshold),
    }
}

/// One stage of one system list.
///
/// The plan cuts each list into **segments** (runs of batchable view
/// systems between systems that run alone, phases and barriers) and puts
/// each system of a segment into a stage. Every system writes in the change
/// tick it would have had run one at a time: the segment's first tick plus
/// its position in the segment.
struct Stage {
    /// The systems, as indexes into the list, in registration order.
    members: Vec<usize>,
    /// Index of the first system of the stage's segment.
    segment: usize,
    /// It is the first stage of its segment.
    starts: bool,
    /// One system that runs exactly where it would run one at a time (no
    /// later system of its segment ran before it): it runs alone, through
    /// the usual path. Every other stage has a `group`.
    in_order: bool,
    /// For the other stages: the proof that its view states may share the
    /// world.
    group: Option<ViewGroup>,
}

/// The stages of the game's four system lists, and what running batches
/// reuses from tick to tick.
#[derive(Default)]
pub(super) struct Exec {
    /// `None` until built, and again after the systems or settings change.
    built: Option<Built>,
    /// The view systems of the batch that is running. Boxed as their
    /// systems hold them: a batch moves pointers, not runners.
    #[allow(clippy::vec_box)]
    runners: Vec<Box<ViewRunner>>,
    /// The event slots of the batch that is running. Empty between
    /// batches: only its allocation is kept.
    slots: Vec<EventSlot<'static>>,
}

struct Built {
    settings: Settings,
    /// How many systems each list had when this was built.
    lens: [usize; 4],
    /// Per list; empty when the scheduler is off.
    stages: [Vec<Stage>; 4],
}

impl Exec {
    /// The systems or the settings changed: build again before running.
    pub(super) fn invalidate(&mut self) {
        self.built = None;
    }

    pub(super) fn is_current(&self, lens: [usize; 4]) -> bool {
        self.built.as_ref().is_some_and(|b| b.lens == lens)
    }

    /// Takes the stages from `plan` (`None`: the scheduler is off, every
    /// system runs alone). `lists` are the game's system lists, in the
    /// plan's order.
    pub(super) fn build(&mut self, settings: Settings, plan: Option<&SchedulePlan>, lists: [&[System]; 4]) {
        let lens = lists.map(<[System]>::len);
        let mut stages: [Vec<Stage>; 4] = Default::default();
        if let Some(plan) = plan {
            let mut offset = 0;
            for (list, systems) in lists.into_iter().enumerate() {
                let end = offset + systems.len();
                let mut planned = plan
                    .stages
                    .iter()
                    .filter(|s| (offset..end).contains(&s.first))
                    .peekable();
                while let Some(first) = planned.next() {
                    // One segment: this stage and the ones that continue it.
                    let mut segment = vec![first];
                    while let Some(next) = planned.next_if(|s| continues(s)) {
                        segment.push(next);
                    }
                    build_segment(&segment, offset, systems, &mut stages[list]);
                }
                offset = end;
            }
        }
        self.built = Some(Built { settings, lens, stages });
    }

    /// Runs the systems of list number `list`, whose first system has
    /// timing index `base`. `tick` is the simulation tick for the list of
    /// simulation systems, whose interval systems skip ticks.
    pub(super) fn run_list(
        &mut self,
        systems: &mut [System],
        list: usize,
        ctx: &mut Ctx,
        base: usize,
        tick: Option<u64>,
    ) {
        let alone = |s: &mut System, ctx: &mut Ctx, index: usize| {
            if let Some(tick) = tick {
                if tick % u64::from(s.every) != u64::from(s.phase) {
                    return;
                }
                ctx.set_sim_steps(s.every);
            }
            run_one(s, ctx, index);
        };
        let Exec { built, runners, slots } = self;
        let stages = built
            .as_ref()
            .filter(|b| b.settings.on && b.lens[list] == systems.len());
        let Some(built) = stages else {
            for (i, s) in systems.iter_mut().enumerate() {
                alone(s, ctx, base + i);
            }
            return;
        };
        // The change tick the running segment's first system has.
        let mut segment_tick = 0;
        for stage in &built.stages[list] {
            if stage.starts {
                segment_tick = ctx.world.change_tick().to_bits();
            }
            match (&stage.group, stage.in_order) {
                (Some(group), false) => {
                    if tick.is_some() {
                        ctx.set_sim_steps(1);
                    }
                    let ticks = Ticks {
                        segment: stage.segment,
                        first: segment_tick,
                    };
                    run_batch(systems, stage, group, ctx, base, ticks, built.settings, runners, slots);
                }
                _ => {
                    let i = stage.members[0];
                    alone(&mut systems[i], ctx, base + i);
                }
            }
        }
    }
}

/// `true` if plan stage `stage` continues the segment of the stage before
/// it: it starts there because of a conflict or an order, not because of a
/// phase, a barrier or a system that runs alone.
fn continues(stage: &crate::schedule::Stage) -> bool {
    matches!(stage.boundary, Boundary::Conflict { .. } | Boundary::Order { .. })
}

/// Builds the stages of one segment, given its plan stages, whose indexes
/// are `offset` more than the list's `systems`.
fn build_segment(segment: &[&crate::schedule::Stage], offset: usize, systems: &[System], out: &mut Vec<Stage>) {
    let first = segment[0].first - offset;
    let len: usize = segment.iter().map(|s| s.len).sum();
    let mut built = Vec::with_capacity(segment.len());
    // The next system that would run one at a time.
    let mut next = first;
    for (k, stage) in segment.iter().enumerate() {
        let members: Vec<usize> = stage.members.iter().map(|&m| m - offset).collect();
        let in_order = members.len() == 1 && members[0] == next;
        next = next.max(members[members.len() - 1] + 1);
        let group = (!in_order)
            .then(|| ViewGroup::new(members.iter().filter_map(|&m| runner(&systems[m])).map(|r| &r.state)).ok())
            .flatten()
            .filter(|g| g.len() == members.len());
        if !in_order && group.is_none() {
            // The states can't be shown to be disjoint (the plan says they
            // are): the segment's systems simply run alone, in order.
            out.extend((first..first + len).map(|m| Stage {
                members: vec![m],
                segment: first,
                starts: m == first,
                in_order: true,
                group: None,
            }));
            return;
        }
        built.push(Stage {
            members,
            segment: first,
            starts: k == 0,
            in_order,
            group,
        });
    }
    out.extend(built);
}

/// The change ticks of a segment's systems: the system at list index `i`
/// writes in tick `first + (i - segment)`, as it would run one at a time.
#[derive(Clone, Copy)]
struct Ticks {
    /// Index of the segment's first system.
    segment: usize,
    /// The change tick of that system.
    first: u64,
}

impl Ticks {
    fn of(self, i: usize) -> u64 {
        self.first + (i - self.segment) as u64
    }
}

fn runner(s: &System) -> Option<&ViewRunner> {
    match &s.run {
        Run::View(Some(runner)) => Some(runner),
        _ => None,
    }
}

/// `slots` emptied, with its allocation kept for a batch that borrows
/// something else. Collecting an emptied vector of the same layout reuses
/// its buffer.
fn recycle(mut slots: Vec<EventSlot<'_>>) -> Vec<EventSlot<'static>> {
    slots.clear();
    slots
        .into_iter()
        .map(|_| -> EventSlot<'static> { unreachable!("the vector is empty") })
        .collect()
}

/// Runs `stage` of the list `systems` (all view systems, checked by
/// `group`); the list's first system has timing index `base`.
#[allow(clippy::vec_box)] // see `Exec::runners`
#[allow(clippy::too_many_arguments)]
fn run_batch(
    systems: &mut [System],
    stage: &Stage,
    group: &ViewGroup,
    ctx: &mut Ctx,
    base: usize,
    ticks: Ticks,
    settings: Settings,
    runners: &mut Vec<Box<ViewRunner>>,
    slot_store: &mut Vec<EventSlot<'static>>,
) {
    ctx.sync_services();
    let tick = ctx.time.sim_tick();
    let profiling = ctx.perf.state.on;
    // Run one at a time, every system has a change tick of its own. The
    // batch keeps that: its systems write in the ticks they would have had,
    // so change stamps and the world's tick are the same as with the
    // scheduler off. The world moves on to the last of them, unless a
    // later system of the segment already took it there; then this stage
    // only has systems registered before that one, none of which records
    // commands (a recorder is never followed by an earlier stage).
    let last = ticks.of(stage.members[stage.members.len() - 1]);
    let moves_on = ctx.world.change_tick().to_bits() <= last;
    while ctx.world.change_tick().to_bits() < last {
        ctx.world.advance_change_tick();
    }
    let members = &stage.members;
    runners.clear();
    let mut slots: Vec<EventSlot<'_>> = std::mem::take(slot_store);
    // What the batch could save by using other threads: everything but its
    // slowest system, which this thread would run anyway.
    let (mut work_us, mut slowest_us) = (0.0f32, 0.0f32);
    for &i in members {
        let s = &mut systems[i];
        let Run::View(taken) = &mut s.run else {
            unreachable!("a batch only has view systems");
        };
        let mut runner = taken.take().expect("a batch gives its systems back");
        // The stream `Ctx::random` gives this system.
        runner.task.random = Random::new(ctx.seed, tick, base + i);
        // Each view compares changes against its own system's last run.
        runner
            .state
            .override_changes_since(Some(s.changes_seen.in_world(&ctx.world)));
        runner
            .state
            .override_change_tick(Some(ponko_ecs::Tick::from_bits(ticks.of(i))));
        // The events it may read: what is kept now. Nothing reaches the
        // store before the batch ends.
        let first = slots.len();
        for (id, items) in runner.access().event_reads() {
            if let Some(items) = items(&ctx.events) {
                slots.push(EventSlot {
                    id,
                    items,
                    unread: ctx.events.unread(id, s.id),
                });
            }
        }
        runner.task.slots = first..slots.len();
        work_us += runner.task.usual_us;
        slowest_us = slowest_us.max(runner.task.usual_us);
        runners.push(runner);
    }
    // Other threads only when it pays, and never while profiling: the
    // allocation counter is the whole process's, so only systems that run
    // one at a time can be charged what they allocate.
    let parallel = settings.threads && !profiling && work_us - slowest_us >= settings.threshold_us;
    // Two counters per batch, for `Ctx::ecs_diagnostics`.
    if let Some(d) = ctx.perf.dispatch.get_mut(base + members[0]) {
        if parallel {
            d.threaded_runs += 1;
        } else {
            d.inline_runs += 1;
        }
        d.usual_work_us = work_us;
    }
    let shared = Shared {
        slots: &slots,
        input: &ctx.input,
        time: &ctx.time,
        spatial: &ctx.spatial,
        count_allocs: profiling,
    };
    let world = &mut ctx.world;
    let ran = catch_unwind(AssertUnwindSafe(|| {
        group.run(
            world,
            runners,
            parallel,
            |runner| {
                let runner: &mut ViewRunner = runner;
                (&mut runner.state, &mut runner.task)
            },
            |task, view| task.run_shared(view, &shared),
        );
    }));
    *slot_store = recycle(slots);
    if let Err(panic) = ran {
        // A system panicked (every other system of the batch still ran:
        // see `ViewGroup::run`). The batch gets no barrier: what its
        // systems recorded and sent is dropped and their read positions
        // stay, on threads and in order alike, so nothing is left over for
        // a later batch to deliver. As after a system that ran alone, a
        // system that returned won't be shown its own writes again, and
        // the last system's tick ends. Then give the systems back and pass
        // the panic on; the game is whole if someone catches it.
        let world_id = ctx.world.id();
        for (&i, mut runner) in members.iter().zip(runners.drain(..)) {
            let s = &mut systems[i];
            runner.task.commands.clear();
            runner.task.outbox.clear();
            runner.task.read.clear();
            if runner.task.finished {
                s.changes_seen = ChangesSeen {
                    world: world_id,
                    tick: ponko_ecs::Tick::from_bits(ticks.of(i)),
                };
            }
            s.run = Run::View(Some(runner));
        }
        if moves_on {
            ctx.world.advance_change_tick();
        }
        resume_unwind(panic);
    }

    // The barrier. Everything below happens in registration order.
    // First every system's read positions: the store still holds exactly
    // the events the batch saw.
    for (&i, runner) in members.iter().zip(runners.iter()) {
        for &id in &runner.task.read {
            ctx.events.mark_read(id, systems[i].id);
        }
    }
    let world_id = ctx.world.id();
    for (&i, mut runner) in members.iter().zip(runners.drain(..)) {
        let s = &mut systems[i];
        let task = &mut runner.task;
        task.outbox.flush(&mut ctx.events);
        let mut ms = task.last_ms;
        // Its writes belong to the tick it wrote in: it won't be shown
        // them again, every later system will.
        s.changes_seen = ChangesSeen {
            world: world_id,
            tick: ponko_ecs::Tick::from_bits(ticks.of(i)),
        };
        if !task.commands.is_empty() {
            // Only the last system of a batch records commands, and the
            // world is in that system's tick: its commands are its own
            // changes, as when it runs alone; the batch's other systems
            // see them on their next run.
            let start = Instant::now();
            ctx.world.apply(&mut task.commands);
            ms += start.elapsed().as_secs_f32() * 1000.0;
        }
        let us = task.last_ms * 1000.0;
        task.usual_us = if task.usual_us == 0.0 {
            us
        } else {
            task.usual_us + (us - task.usual_us) * 0.1
        };
        ctx.perf.timings[base + i].last_ms += ms;
        if profiling {
            ctx.perf.state.add_system_allocs(base + i, task.allocs);
        }
        s.run = Run::View(Some(runner));
    }
    // The last system's tick ends here, as after a system that ran alone.
    if moves_on {
        ctx.world.advance_change_tick();
    }
    ctx.perf.entities = ctx.world.len();
}
