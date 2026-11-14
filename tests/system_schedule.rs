//! The parallel system scheduler (`Game::parallel_systems`): which systems
//! share a batch, that conflicting systems keep their order, where commands
//! apply and events become visible, and that a simulation comes out the same
//! whether its systems run one at a time, in batches on one thread, or in
//! batches on several threads.
//!
//! The differential test builds random sets of systems from a seed. A
//! failing case prints its seed; rerun just that case with
//! `PONKO_SCHEDULE_SEED=<seed> cargo test --test system_schedule differential`.
//! `PONKO_SCHEDULE_CASES` changes how many seeds run.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ponko::access::{Service, SystemAccess};
use ponko::prelude::*;
use ponko::schedule::{Alone, Boundary};
use ponko::system_view::SystemView;

/// How a game's systems run.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// One at a time: the scheduler is off.
    Off,
    /// In batches, every batch on the game's thread.
    InOrder,
    /// In batches, every batch on the thread pool.
    Threads,
}

const MODES: [Mode; 3] = [Mode::Off, Mode::InOrder, Mode::Threads];

fn game(mode: Mode) -> Game {
    // The modes are compared with each other, so they hold whatever
    // `PONKO_PARALLEL_SYSTEMS` says in this test run.
    let game = Game::new("schedule", 1, 1).parallel_systems_env(false);
    match mode {
        Mode::Off => game.parallel_systems(false),
        Mode::InOrder => game.parallel_systems(true).parallel_threshold(Duration::MAX),
        Mode::Threads => game.parallel_systems(true).parallel_threshold(Duration::ZERO),
    }
}

/// `true` if batches can really use several threads here.
fn has_threads() -> bool {
    rayon::current_num_threads() > 1
}

// ---- the plan, through the public API --------------------------------------------------

#[derive(Component)]
struct Pos(f32);
#[derive(Component)]
struct Vel(f32);
#[derive(Component)]
struct Health(i32);
#[derive(Component)]
struct Score(i64);
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hit(u32);

fn nothing(_: &mut SystemView) {}

fn writes<T: Component>() -> SystemAccess {
    SystemAccess::new().write::<T>()
}

fn reads<T: Component>() -> SystemAccess {
    SystemAccess::new().read::<T>()
}

/// The plan's stages in `AfterPhysics`, as system counts.
fn stage_sizes(game: &Game) -> Vec<usize> {
    let plan = game.schedule_plan();
    plan.stages_in(Phase::AfterPhysics).iter().map(|s| s.len).collect()
}

#[test]
fn systems_that_declare_nothing_stay_alone_and_sequential() {
    let game = Game::new("plan", 1, 1)
        .parallel_systems(true)
        .sim_system(|_| {})
        .sim_system(|_| {})
        .add(Phase::AfterPhysics, (|_: &mut Ctx| {}).access(writes::<Pos>()))
        .add(Phase::AfterPhysics, (|_: &mut Ctx| {}).access(writes::<Vel>()));
    let plan = game.schedule_plan();
    assert_eq!(stage_sizes(&game), [1, 1, 1, 1]);
    assert_eq!(plan.batches().count(), 0);
    let alone: Vec<_> = (plan.systems.iter())
        .filter(|s| s.phase == Phase::AfterPhysics)
        .map(|s| s.alone)
        .collect();
    assert_eq!(
        alone,
        [
            Some(Alone::Undeclared),
            Some(Alone::Undeclared),
            Some(Alone::CtxSystem),
            Some(Alone::CtxSystem)
        ]
    );
}

#[test]
fn the_built_in_systems_run_alone() {
    let plan = Game::new("plan", 1, 1).spatial_grid(16.0).schedule_plan();
    assert!(!plan.systems.is_empty());
    assert!(plan.systems.iter().all(|s| s.alone == Some(Alone::Undeclared)));
    assert!(plan.stages.iter().all(|s| s.len == 1));
}

#[test]
fn a_chain_of_view_systems_runs_one_stage_each() {
    let game = Game::new("plan", 1, 1)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(reads::<Pos>().write::<Vel>()))
        .add(Phase::AfterPhysics, nothing.access(reads::<Vel>().write::<Health>()));
    assert_eq!(stage_sizes(&game), [1, 1, 1]);
}

#[test]
fn readers_fan_out_behind_their_writer() {
    let game = Game::new("plan", 1, 1)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(reads::<Pos>().write::<Vel>()))
        .add(Phase::AfterPhysics, nothing.access(reads::<Pos>().write::<Health>()))
        .add(Phase::AfterPhysics, nothing.access(reads::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>().read::<Vel>()));
    assert_eq!(stage_sizes(&game), [1, 3, 1]);
    let plan = game.schedule_plan();
    let fan = plan.stages_in(Phase::AfterPhysics)[1];
    assert!(fan.is_batch());
    assert!(matches!(fan.boundary, Boundary::Conflict { .. }));
}

#[test]
fn a_later_system_that_interacts_with_nothing_between_joins_an_earlier_batch() {
    // Producers and consumers registered in pairs: each consumer reads what
    // the producer before it writes. Batching only neighbours would give
    // `p0`, `c0 + p1`, `c1`; by dependencies it is two stages.
    let game = Game::new("plan", 1, 1)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(reads::<Pos>().write::<Vel>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Health>()))
        .add(Phase::AfterPhysics, nothing.access(reads::<Health>().write::<Score>()));
    assert_eq!(stage_sizes(&game), [2, 2]);
    let plan = game.schedule_plan();
    let stages = plan.stages_in(Phase::AfterPhysics);
    assert_eq!((stages[0].systems(), stages[1].systems()), (&[0, 2][..], &[1, 3][..]));
    assert!(matches!(plan.systems[3].boundary, Boundary::Conflict { with: 2, .. }));
}

#[test]
fn moved_systems_keep_every_observable_order_in_every_mode() {
    // `late` is registered last but interacts with nothing before it, so it
    // runs in the first stage; the writers of `Score` keep their order, and
    // the senders of `Hit` publish in registration order although the
    // second one's stage is chosen by a conflict of the first.
    #[derive(Default)]
    struct Seen(Vec<u32>);
    let send = |n: u32| move |sys: &mut SystemView| sys.events.send(Hit(n));
    for mode in MODES {
        let game = game(mode)
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| sys.world.query::<&mut Score>().for_each(|s| s.0 += 1))
                    .access(writes::<Score>()),
            )
            .add(
                Phase::AfterPhysics,
                (move |sys: &mut SystemView| {
                    sys.world.query::<&mut Score>().for_each(|s| s.0 *= 3);
                    send(1)(sys);
                })
                .access(writes::<Score>().send_events::<Hit>()),
            )
            .add(
                Phase::AfterPhysics,
                send(2).access(SystemAccess::new().send_events::<Hit>()),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| sys.world.query::<&mut Pos>().for_each(|p| p.0 += 1.0)).access(writes::<Pos>()),
            )
            .add(
                Phase::SimLast,
                (|sys: &mut SystemView| {
                    let hits: Vec<u32> = sys.events.read::<Hit>().iter().map(|h| h.0).collect();
                    sys.world.resource_mut::<Seen>().0.extend(hits);
                })
                .access(SystemAccess::new().read_events::<Hit>().write_resource::<Seen>()),
            );
        let plan = game.schedule_plan();
        let stages = plan.stages_in(Phase::AfterPhysics);
        assert_eq!(stages.len(), 2, "{plan}");
        assert_eq!(
            (stages[0].systems(), stages[1].systems()),
            (&[0, 3][..], &[1, 2][..]),
            "{plan}"
        );
        assert!(
            matches!(plan.systems[2].boundary, Boundary::KeepsOrder { after: 1, .. }),
            "{plan}"
        );
        let mut world = game.run_headless(2, |ctx| {
            ctx.world.insert_resource(Seen::default());
            ctx.world.spawn((Score(1), Pos(0.0)));
        });
        assert_eq!(
            world.query::<&Score>().iter().next().unwrap().0,
            ((1 + 1) * 3 + 1) * 3,
            "{mode:?}"
        );
        assert_eq!(world.resource::<Seen>().0, [1, 2, 1, 2], "{mode:?}");
    }
}

#[test]
fn a_ctx_system_between_view_systems_is_a_barrier() {
    let game = Game::new("plan", 1, 1)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Vel>()))
        .sim_system(|_| {})
        .add(Phase::AfterPhysics, nothing.access(writes::<Health>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()));
    assert_eq!(stage_sizes(&game), [2, 1, 2]);
    let plan = game.schedule_plan();
    let stages = plan.stages_in(Phase::AfterPhysics);
    assert_eq!(stages[1].boundary, Boundary::Alone);
    assert_eq!(stages[2].boundary, Boundary::Alone);
}

#[test]
fn an_explicit_barrier_splits_a_batch_where_it_was_placed() {
    let game = Game::new("plan", 1, 1)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Vel>()))
        .barrier(Phase::AfterPhysics)
        .add(Phase::AfterPhysics, nothing.access(writes::<Health>()))
        // A barrier in another phase doesn't touch this one.
        .barrier(Phase::SimLast)
        .add(Phase::AfterPhysics, nothing.access(writes::<Score>()));
    assert_eq!(stage_sizes(&game), [2, 2]);
    let plan = game.schedule_plan();
    assert_eq!(plan.stages_in(Phase::AfterPhysics)[1].boundary, Boundary::Barrier);
}

#[test]
fn a_barrier_with_no_system_after_it_does_nothing() {
    let game = Game::new("plan", 1, 1)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Vel>()))
        .barrier(Phase::AfterPhysics)
        .barrier(Phase::AfterPhysics);
    assert_eq!(stage_sizes(&game), [2]);
}

#[test]
fn explicit_order_keeps_two_systems_in_different_stages() {
    let mut game = Game::new("plan", 1, 1);
    let first = game.add_system(Phase::AfterPhysics, nothing.access(writes::<Pos>()));
    game.add_system(Phase::AfterPhysics, nothing.access(writes::<Vel>()));
    let third = game.add_system(Phase::AfterPhysics, nothing.access(writes::<Health>()));
    assert_eq!(stage_sizes(&game), [3]);
    let game = game.order(first, third);
    assert_eq!(stage_sizes(&game), [2, 1]);
    let plan = game.schedule_plan();
    assert_eq!(
        plan.stages_in(Phase::AfterPhysics)[1].boundary,
        Boundary::Order { after: 0 }
    );
    // A pair whose system was removed is ignored.
    let game = game.remove(first);
    assert_eq!(stage_sizes(&game), [2]);
}

#[test]
#[should_panic(expected = "order can keep two systems apart, not swap them")]
fn explicit_order_against_the_registration_order_panics() {
    let mut game = Game::new("plan", 1, 1);
    let first = game.add_system(Phase::AfterPhysics, nothing.access(writes::<Pos>()));
    let second = game.add_system(Phase::AfterPhysics, nothing.access(writes::<Vel>()));
    let _ = game.order(second, first).schedule_plan();
}

#[test]
#[should_panic(expected = "order can keep two systems apart, not swap them")]
fn an_order_against_the_run_order_panics_at_the_start_with_the_scheduler_off() {
    // Not only once someone turns the scheduler on.
    let mut game = game(Mode::Off);
    let first = game.add_system(Phase::AfterPhysics, nothing.access(writes::<Pos>()));
    let second = game.add_system(Phase::AfterPhysics, nothing.access(writes::<Vel>()));
    game.order(second, first).run_headless(1, |_| {});
}

#[test]
fn an_order_that_agrees_with_the_run_order_runs_in_every_mode() {
    for mode in MODES {
        let mut game = game(mode);
        let first = game.add_system(Phase::BeforePhysics, nothing.access(writes::<Pos>()));
        let second = game.add_system(Phase::AfterPhysics, nothing.access(writes::<Vel>()));
        game.order(first, second).run_headless(2, |_| {});
    }
}

#[test]
fn phases_never_share_a_batch() {
    let game = Game::new("plan", 1, 1)
        .add(Phase::BeforePhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Vel>()))
        .add(Phase::SimLast, nothing.access(writes::<Health>()));
    let plan = game.schedule_plan();
    assert_eq!(plan.batches().count(), 0);
    for phase in [Phase::BeforePhysics, Phase::AfterPhysics, Phase::SimLast] {
        assert_eq!(plan.stages_in(phase).len(), 1);
    }
}

#[test]
fn view_systems_that_hold_a_service_run_alone() {
    let game = Game::new("plan", 1, 1)
        .add(
            Phase::AfterPhysics,
            nothing.access(writes::<Pos>().write_service(Service::Camera)),
        )
        .add(
            Phase::AfterPhysics,
            nothing.access(writes::<Vel>().read_service(Service::Time)),
        )
        .add(
            Phase::AfterPhysics,
            nothing.access(writes::<Health>().read_service(Service::Random)),
        );
    assert_eq!(stage_sizes(&game), [1, 2]);
    let plan = game.schedule_plan();
    let first = plan.systems.iter().find(|s| s.phase == Phase::AfterPhysics).unwrap();
    assert_eq!(first.alone, Some(Alone::Service(Service::Camera)));
}

#[test]
fn the_plan_is_the_same_with_the_scheduler_on_or_off() {
    let build = |on: bool| {
        Game::new("plan", 1, 1)
            .parallel_systems(on)
            .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
            .add(Phase::AfterPhysics, nothing.access(writes::<Vel>()))
            .sim_system(|_| {})
            .schedule_plan()
            .to_string()
    };
    let text = build(true);
    assert_eq!(text, build(false));
    assert!(text.contains("AfterPhysics"), "{text}");
    assert!(text.contains("(batch)"), "{text}");
    assert!(text.contains("alone: undeclared"), "{text}");
}

// ---- execution: order, commands, events ------------------------------------------------

/// What systems observed, in order.
#[derive(Default)]
struct Log(Vec<String>);
#[derive(Component, Default)]
struct Lines(Vec<String>);

fn lines(world: &World) -> Vec<String> {
    world.resource::<Log>().0.clone()
}

#[test]
fn systems_of_a_batch_run_at_the_same_time() {
    if !has_threads() {
        return; // one core: nothing to observe
    }
    // Each system waits until it has seen the other one running. One at a
    // time, neither would ever see the other.
    let inside = Arc::new([AtomicBool::new(false), AtomicBool::new(false)]);
    let met = Arc::new(AtomicU32::new(0));
    let system = |me: usize| {
        let (inside, met) = (inside.clone(), met.clone());
        move |_: &mut SystemView| {
            inside[me].store(true, Ordering::SeqCst);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                if inside[1 - me].load(Ordering::SeqCst) {
                    met.fetch_add(1, Ordering::SeqCst);
                    break;
                }
                std::thread::yield_now();
            }
        }
    };
    let _ = game(Mode::Threads)
        .add(Phase::AfterPhysics, system(0).access(writes::<Pos>()))
        .add(Phase::AfterPhysics, system(1).access(writes::<Vel>()))
        .run_headless(1, |_| {});
    // The flags are never cleared, so the second to arrive always sees the
    // first; only systems that overlap both see each other.
    assert_eq!(met.load(Ordering::SeqCst), 2, "the systems didn't overlap");
}

#[test]
fn conflicting_systems_never_overlap_and_keep_their_order() {
    // Three writers of one component; each checks nobody else is inside.
    let busy = Arc::new(AtomicBool::new(false));
    let step = |f: fn(i64) -> i64| {
        let busy = busy.clone();
        move |sys: &mut SystemView| {
            assert!(!busy.swap(true, Ordering::SeqCst), "two writers at once");
            sys.world.query::<&mut Score>().for_each(|s| s.0 = f(s.0));
            std::thread::sleep(Duration::from_micros(200));
            busy.store(false, Ordering::SeqCst);
        }
    };
    for mode in MODES {
        let mut world = game(mode)
            .add(Phase::AfterPhysics, step(|s| s + 1).access(writes::<Score>()))
            .add(Phase::AfterPhysics, step(|s| s * 3).access(writes::<Score>()))
            .add(Phase::AfterPhysics, step(|s| s - 5).access(writes::<Score>()))
            .run_headless(4, |ctx| {
                ctx.world.spawn((Score(0),));
            });
        // ((((0+1)*3-5)+1)*3-5 ...) only in this order.
        let mut expect = 0i64;
        for _ in 0..4 {
            expect = (expect + 1) * 3 - 5;
        }
        assert_eq!(world.query::<&Score>().iter().next().unwrap().0, expect, "{mode:?}");
    }
}

#[test]
fn a_reader_never_runs_while_its_writer_does() {
    let writing = Arc::new(AtomicBool::new(false));
    let (w, r) = (writing.clone(), writing.clone());
    for mode in MODES {
        let (w, r) = (w.clone(), r.clone());
        let _ = game(mode)
            .add(
                Phase::AfterPhysics,
                (move |sys: &mut SystemView| {
                    assert!(!r.load(Ordering::SeqCst), "read during a write");
                    let _ = sys.world.query::<&Pos>().count();
                })
                .access(reads::<Pos>()),
            )
            .add(
                Phase::AfterPhysics,
                (move |sys: &mut SystemView| {
                    w.store(true, Ordering::SeqCst);
                    sys.world.query::<&mut Pos>().for_each(|p| p.0 += 1.0);
                    std::thread::sleep(Duration::from_micros(300));
                    w.store(false, Ordering::SeqCst);
                })
                .access(writes::<Pos>()),
            )
            .run_headless(6, |ctx| {
                ctx.world.spawn((Pos(0.0),));
            });
    }
}

#[test]
fn commands_of_a_batch_apply_when_it_ends() {
    for mode in MODES {
        let world = game(mode)
            // A batch: the counter, then the spawner (a recorder is last).
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let n = sys.world.len();
                    sys.world.resource_mut::<Log>().0.push(format!("batch sees {n}"));
                })
                .access(SystemAccess::new().write_resource::<Log>()),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    sys.commands.spawn((Health(1),));
                    // Not applied inside the recorder.
                    assert_eq!(sys.world.query::<&Health>().count() % 2, 0);
                    sys.commands.spawn((Health(2),));
                })
                .access(writes::<Health>().commands()),
            )
            // The next stage sees both.
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let n = sys.world.query::<&Health>().count();
                    sys.world.resource_mut::<Log>().0.push(format!("next sees {n}"));
                })
                .access(reads::<Health>().write_resource::<Log>()),
            )
            .run_headless(2, |ctx| ctx.world.insert_resource(Log::default()));
        assert_eq!(
            lines(&world),
            ["batch sees 0", "next sees 2", "batch sees 2", "next sees 4"],
            "{mode:?}"
        );
    }
}

#[test]
fn a_command_recorder_is_always_the_last_of_its_batch() {
    let game = Game::new("plan", 1, 1)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Vel>().commands()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Health>()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Score>().commands()))
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>().commands()));
    assert_eq!(stage_sizes(&game), [2, 2, 1]);
}

#[test]
fn events_sent_in_a_batch_arrive_in_registration_order() {
    for mode in MODES {
        let world = game(mode)
            // Two senders of one type share a batch. The first is slow, so
            // on threads the second finishes sending first.
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    std::thread::sleep(Duration::from_millis(2));
                    sys.events.send(Hit(1));
                    sys.events.send(Hit(2));
                })
                .access(writes::<Pos>().send_events::<Hit>()),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    sys.events.send(Hit(3));
                })
                .access(writes::<Vel>().send_events::<Hit>()),
            )
            // A reader after them is in the next stage: it sees all three.
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let hits: Vec<u32> = sys.events.read::<Hit>().iter().map(|h| h.0).collect();
                    sys.world.resource_mut::<Log>().0.push(format!("{hits:?}"));
                })
                .access(SystemAccess::new().read_events::<Hit>().write_resource::<Log>()),
            )
            .run_headless(2, |ctx| ctx.world.insert_resource(Log::default()));
        assert_eq!(lines(&world), ["[1, 2, 3]", "[1, 2, 3]"], "{mode:?}");
    }
}

#[test]
fn a_reader_before_its_sender_shares_the_batch_and_reads_next_tick() {
    let build = |mode| {
        game(mode)
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let kept = sys.events.peek::<Hit>().len();
                    let hits: Vec<u32> = sys.events.read::<Hit>().iter().map(|h| h.0).collect();
                    // A second read in the same run finds nothing new.
                    assert!(sys.events.read::<Hit>().is_empty());
                    sys.world
                        .resource_mut::<Log>()
                        .0
                        .push(format!("read {hits:?} kept {kept}"));
                })
                .access(SystemAccess::new().read_events::<Hit>().write_resource::<Log>()),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let tick = sys.time.sim_tick() as u32;
                    sys.events.send(Hit(tick));
                })
                .access(SystemAccess::new().send_events::<Hit>().read_service(Service::Time)),
            )
    };
    assert_eq!(stage_sizes(&build(Mode::Off)), [2]);
    for mode in MODES {
        let world = build(mode).run_headless(3, |ctx| ctx.world.insert_resource(Log::default()));
        assert_eq!(
            lines(&world),
            ["read [] kept 0", "read [0] kept 1", "read [1] kept 1"],
            "{mode:?}"
        );
    }
}

#[test]
fn a_system_that_sends_and_reads_one_type_still_sees_its_own_events() {
    for mode in MODES {
        let world = game(mode)
            .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    sys.events.send(Hit(7));
                    let n = sys.events.read::<Hit>().len();
                    sys.world.resource_mut::<Log>().0.push(format!("own {n}"));
                })
                .access(
                    SystemAccess::new()
                        .send_events::<Hit>()
                        .read_events::<Hit>()
                        .write_resource::<Log>(),
                ),
            )
            .run_headless(2, |ctx| ctx.world.insert_resource(Log::default()));
        assert_eq!(lines(&world), ["own 1", "own 1"], "{mode:?}");
    }
}

#[test]
fn an_explicit_barrier_makes_earlier_work_visible() {
    // Without a declaration that shows it, only the barrier keeps the
    // second system behind the first.
    let seen = Arc::new(AtomicU32::new(0));
    for mode in MODES {
        let (a, b) = (seen.clone(), seen.clone());
        seen.store(0, Ordering::SeqCst);
        let world = game(mode)
            .add(
                Phase::AfterPhysics,
                (move |_: &mut SystemView| {
                    std::thread::sleep(Duration::from_millis(1));
                    a.store(5, Ordering::SeqCst);
                })
                .access(writes::<Pos>()),
            )
            .barrier(Phase::AfterPhysics)
            .add(
                Phase::AfterPhysics,
                (move |sys: &mut SystemView| {
                    let n = b.load(Ordering::SeqCst);
                    sys.world.resource_mut::<Log>().0.push(n.to_string());
                })
                .access(SystemAccess::new().write_resource::<Log>()),
            )
            .run_headless(1, |ctx| ctx.world.insert_resource(Log::default()));
        assert_eq!(lines(&world), ["5"], "{mode:?}");
    }
}

#[test]
fn each_system_keeps_its_own_random_stream_and_clock() {
    let run = |mode| {
        let draw = |sys: &mut SystemView| {
            let n = sys.random().stream(1).u64();
            let line = format!(
                "{} tick {} dt {:.4} {n}",
                sys.name().len(),
                sys.time.sim_tick(),
                sys.dt()
            );
            sys.world.query::<&mut Lines>().for_each(|v| v.0.push(line.clone()));
        };
        let other = |sys: &mut SystemView| {
            let n = sys.random().stream(1).u64();
            sys.world.resource_mut::<Log>().0.push(n.to_string());
        };
        let random = || {
            SystemAccess::new()
                .read_service(Service::Random)
                .read_service(Service::Time)
        };
        let mut world = game(mode)
            .seed(99)
            .add(Phase::AfterPhysics, draw.access(random().write::<Lines>()))
            .add(Phase::AfterPhysics, other.access(random().write_resource::<Log>()))
            .run_headless(3, |ctx| {
                ctx.world.insert_resource(Log::default());
                ctx.world.spawn((Lines::default(),));
            });
        let first = world.query::<&Lines>().iter().next().unwrap().0.clone();
        (first, lines(&world))
    };
    let off = run(Mode::Off);
    // Two systems, two streams; each differs from tick to tick.
    assert_ne!(off.0[0].rsplit(' ').next(), Some(off.1[0].as_str()));
    assert_ne!(off.1[0], off.1[1]);
    assert_eq!(run(Mode::InOrder), off);
    assert_eq!(run(Mode::Threads), off);
}

/// A raw change tick a system kept for later.
struct Kept(u64);

#[test]
fn raw_change_ticks_are_the_same_in_every_mode() {
    // Every system has a change tick of its own when systems run one at a
    // time. A batch keeps those ticks: what a system reads as the current
    // tick, the stamps its writes get, and the world's tick afterwards.
    let run = |mode| {
        let note = |sys: &mut SystemView, who: &str| {
            let tick = sys.world.change_tick().to_bits();
            sys.world.resource_mut::<Log>().0.push(format!("{who} {tick}"));
        };
        let built = game(mode)
            .add(Phase::BeforePhysics, |ctx: &mut Ctx| {
                let tick = ctx.world.change_tick().to_bits();
                ctx.world.resource_mut::<Log>().0.push(format!("before {tick}"));
            })
            // A batch of three. The first keeps its tick in a resource.
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    sys.world.resource_mut::<Kept>().0 = sys.world.change_tick().to_bits();
                    sys.world.query::<&mut Pos>().for_each(|p| p.0 += 1.0);
                })
                .access(writes::<Pos>().write_resource::<Kept>()),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    sys.world.query::<&mut Vel>().for_each(|v| v.0 += 1.0);
                })
                .access(writes::<Vel>()),
            )
            .add(
                Phase::AfterPhysics,
                (move |sys: &mut SystemView| {
                    note(sys, "third");
                    sys.world.query::<&mut Health>().for_each(|h| h.0 += 1);
                    sys.commands.spawn((Score(0),));
                })
                .access(writes::<Health>().write_resource::<Log>().commands()),
            )
            // Afterwards: what changed after the tick the first one kept?
            // Its own writes didn't; those of the systems after it did.
            .add(Phase::SimLast, |ctx: &mut Ctx| {
                let kept = Tick::from_bits(ctx.world.resource::<Kept>().0);
                let pos = ctx.world.query::<&Pos>().filter::<Changed<Pos>>().since(kept).count();
                let vel = ctx.world.query::<&Vel>().filter::<Changed<Vel>>().since(kept).count();
                let health = (ctx.world.query::<&Health>())
                    .filter::<Changed<Health>>()
                    .since(kept)
                    .count();
                let tick = ctx.world.change_tick().to_bits();
                (ctx.world.resource_mut::<Log>().0).push(format!(
                    "last {tick} kept {} pos {pos} vel {vel} health {health}",
                    kept.to_bits()
                ));
            });
        // The three view systems share a stage: with the scheduler on, the
        // ticks below are those of a real batch.
        let plan = built.schedule_plan();
        let batch = plan.stages_in(Phase::AfterPhysics);
        assert_eq!((batch.len(), batch[0].len), (1, 3), "{plan}");
        let world = built.run_headless(4, |ctx| {
            ctx.world.track_changes::<Pos>();
            ctx.world.track_changes::<Vel>();
            ctx.world.track_changes::<Health>();
            ctx.world.insert_resource(Log::default());
            ctx.world.insert_resource(Kept(0));
            ctx.world.spawn((Pos(0.0), Vel(0.0), Health(0)));
        });
        let mut lines = lines(&world);
        lines.push(format!("end {}", world.change_tick().to_bits()));
        lines
    };
    let off = run(Mode::Off);
    assert!(off.iter().any(|l| l.ends_with("pos 0 vel 1 health 1")), "{off:?}");
    assert_eq!(run(Mode::InOrder), off);
    assert_eq!(run(Mode::Threads), off);
}

#[test]
fn change_filters_see_the_same_changes_in_every_mode() {
    let run = |mode| {
        let world = game(mode)
            // Writes Health of the entities whose Pos changed since it last ran.
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let n = sys.world.query::<Entity>().filter::<Changed<Pos>>().count();
                    let added = sys.world.query::<Entity>().filter::<Added<Pos>>().count();
                    let gone = sys.world.removed::<Pos>().len();
                    sys.world
                        .resource_mut::<Log>()
                        .0
                        .push(format!("changed {n} added {added} gone {gone}"));
                })
                .access(reads::<Pos>().write_resource::<Log>()),
            )
            // Same batch: spawns and despawns through commands.
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let tick = sys.time.sim_tick();
                    if tick.is_multiple_of(2) {
                        sys.commands.spawn((Pos(tick as f32), Health(1)));
                    } else {
                        let first = sys.world.query::<(Entity, &Health)>().iter().next().map(|(e, _)| e);
                        if let Some(e) = first {
                            sys.commands.despawn(e);
                        }
                    }
                })
                .access(writes::<Health>().read_service(Service::Time).commands()),
            )
            // Next stage: moves every third tick.
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    if sys.time.sim_tick().is_multiple_of(3) {
                        sys.world.query::<&mut Pos>().for_each(|p| p.0 += 1.0);
                    }
                })
                .access(writes::<Pos>().read_service(Service::Time)),
            )
            .run_headless(9, |ctx| {
                ctx.world.track_changes::<Pos>();
                ctx.world.insert_resource(Log::default());
                ctx.world.spawn((Pos(0.0), Health(1)));
                ctx.world.spawn((Pos(1.0), Health(1)));
            });
        lines(&world)
    };
    let off = run(Mode::Off);
    assert!(
        off.iter().any(|l| !l.starts_with("changed 0 added 0 gone 0")),
        "{off:?}"
    );
    assert!(off.iter().any(|l| !l.ends_with("gone 0")), "{off:?}");
    assert_eq!(run(Mode::InOrder), off);
    assert_eq!(run(Mode::Threads), off);
}

#[test]
fn interval_systems_between_batches_still_skip_their_ticks() {
    for mode in MODES {
        let world = game(mode)
            .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
            .add(Phase::AfterPhysics, nothing.access(writes::<Vel>()))
            .add(
                Phase::AfterPhysics,
                (|ctx: &mut Ctx| {
                    let line = format!("slow at {} dt {:.3}", ctx.time.sim_tick(), ctx.dt());
                    ctx.world.resource_mut::<Log>().0.push(line);
                })
                .every(3),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    let line = format!("view dt {:.3}", sys.dt());
                    if sys.time.sim_tick() == 0 {
                        sys.world.resource_mut::<Log>().0.push(line);
                    }
                })
                .access(SystemAccess::new().write_resource::<Log>().read_service(Service::Time)),
            )
            .add(Phase::AfterPhysics, nothing.access(writes::<Health>()))
            .run_headless(6, |ctx| ctx.world.insert_resource(Log::default()));
        assert_eq!(
            lines(&world),
            ["slow at 0 dt 0.050", "view dt 0.017", "slow at 3 dt 0.050"],
            "{mode:?}"
        );
    }
}

#[test]
#[should_panic(expected = "doesn't declare that it may write component")]
fn an_undeclared_write_in_a_batch_panics_with_the_usual_message() {
    let _ = game(Mode::Threads)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(
            Phase::AfterPhysics,
            (|sys: &mut SystemView| {
                sys.world.query::<&mut Vel>().for_each(|v| v.0 += 1.0);
            })
            .access(reads::<Vel>()),
        )
        .run_headless(1, |ctx| {
            ctx.world.spawn((Pos(0.0), Vel(0.0)));
        });
}

#[test]
#[should_panic(expected = "doesn't declare that it may send events")]
fn an_undeclared_event_in_a_batch_panics_with_the_usual_message() {
    let _ = game(Mode::InOrder)
        .add(Phase::AfterPhysics, nothing.access(writes::<Pos>()))
        .add(
            Phase::AfterPhysics,
            (|sys: &mut SystemView| {
                sys.events.send(Hit(1));
            })
            .access(writes::<Vel>()),
        )
        .run_headless(1, |_| {});
}

#[test]
fn resources_written_in_a_batch_are_back_in_the_world_afterwards() {
    for mode in MODES {
        let world = game(mode)
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| sys.world.resource_mut::<Score>().0 += 1)
                    .access(SystemAccess::new().write_resource::<Score>()),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    // Created by the system: it wasn't there before.
                    if sys.world.get_resource::<Log>().is_none() {
                        sys.world.insert_resource(Log(vec!["made".into()]));
                    }
                })
                .access(SystemAccess::new().write_resource::<Log>()),
            )
            .run_headless(5, |ctx| ctx.world.insert_resource(Score(0)));
        assert_eq!(world.resource::<Score>().0, 5, "{mode:?}");
        assert_eq!(lines(&world), ["made"], "{mode:?}");
    }
}

// ---- profiling ---------------------------------------------------------------------------

#[test]
fn a_profile_lists_the_same_systems_in_every_mode() {
    let run = |mode| {
        let (mut world, report) = game(mode)
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    profile_scope!("schedule/pos");
                    sys.world.query::<&mut Pos>().for_each(|p| p.0 += 1.0);
                })
                .access(writes::<Pos>()),
            )
            .add(
                Phase::AfterPhysics,
                (|sys: &mut SystemView| {
                    sys.world.query::<&mut Vel>().for_each(|v| v.0 += 2.0);
                })
                .access(writes::<Vel>()),
            )
            .sim_system(|ctx: &mut Ctx| {
                ctx.world.query::<&mut Health>().for_each(|h| h.0 += 1);
            })
            .profile_headless(2, 10, |ctx| {
                ctx.world.spawn((Pos(0.0), Vel(0.0), Health(0)));
            });
        let names: Vec<String> = report.systems.iter().map(|s| s.name.to_string()).collect();
        let values = (world.query::<(&Pos, &Vel, &Health)>().iter())
            .map(|(p, v, h)| (p.0, v.0, h.0))
            .next()
            .unwrap();
        let scope = report.scope("schedule/pos").map(|s| s.calls_per_frame);
        (names, report.frames, values, scope)
    };
    let off = run(Mode::Off);
    assert_eq!(off.1, 10);
    // The three systems, then the built-in drawing systems (listed, though
    // headless runs have no frames to run them in).
    assert!(off.0.len() >= 3, "{:?}", off.0);
    assert!(off.0[..3].iter().all(|n| n.contains("a_profile_lists")), "{:?}", off.0);
    assert_eq!(off.2, (12.0, 24.0, 12));
    assert_eq!(run(Mode::InOrder), off);
    assert_eq!(run(Mode::Threads), off);
}

// ---- lockstep ----------------------------------------------------------------------------

mod lockstep {
    use std::thread;

    use ponko::d2::Transform as Transform2D;
    use ponko::net::{Desync, Net};
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Serialize, Deserialize, Clone, Copy)]
    struct Steer {
        dir: Vec2,
    }

    #[derive(Serialize, Deserialize, Clone, Copy, Component)]
    struct Unit {
        owner: u32,
        vel: Vec2,
        fuel: f32,
    }

    #[derive(Serialize, Deserialize, Clone, Copy, Component)]
    struct Heat(f32);

    /// Every player's input for this tick, copied out of the session by a
    /// `Ctx` system so view systems can read it.
    struct Steering([Vec2; ponko::net::MAX_PLAYERS]);

    struct Burned(#[allow(dead_code)] u32);

    fn play_along(ctx: &mut Ctx) {
        let t = ctx.time.sim_tick() as f32 * 0.05 + ctx.net.player() as f32;
        ctx.net.set_input(&Steer {
            dir: vec2(t.cos(), t.sin()),
        });
        let mut steer = [Vec2::ZERO; ponko::net::MAX_PLAYERS];
        for (p, s) in steer.iter_mut().enumerate().take(ctx.net.players()) {
            if let Some(Steer { dir }) = ctx.net.input::<Steer>(p) {
                *s = dir;
            }
        }
        ctx.world.insert_resource(Steering(steer));
    }

    fn accelerate(sys: &mut SystemView) {
        let steer = sys.world.resource::<Steering>().0;
        let random = sys.random();
        sys.world.query::<(Entity, &mut Unit)>().for_each(|(e, u)| {
            let jitter = random.of(e).unit_vec2() * 5.0;
            u.vel = u.vel * 0.9 + steer[u.owner as usize] * 20.0 + jitter;
            u.fuel -= u.vel.length() * 0.001;
        });
    }

    fn cool(sys: &mut SystemView) {
        let random = sys.random();
        sys.world.query::<(Entity, &mut Heat)>().for_each(|(e, h)| {
            h.0 = h.0 * 0.95 + random.of(e).f32();
        });
    }

    fn movement(sys: &mut SystemView) {
        let dt = sys.dt();
        sys.world
            .query::<(&mut Transform2D, &Unit)>()
            .for_each(|(t, u)| t.position += u.vel * dt);
    }

    fn burn_out(sys: &mut SystemView) {
        let mut out = Vec::new();
        sys.world.query::<(Entity, &Unit, &Heat)>().for_each(|(e, u, h)| {
            if u.fuel < 0.0 && h.0 > 12.0 {
                out.push(e);
            }
        });
        for e in out {
            sys.events.send(Burned(e.index()));
            sys.commands.despawn(e);
        }
    }

    fn count_burned(sys: &mut SystemView) {
        let n = sys.events.read::<Burned>().len() as i64;
        sys.world.resource_mut::<Score>().0 += n;
    }

    fn peer(net: Net, mode: Mode, ticks: u64) -> (ponko::reflect::json::Value, Option<Desync>, i64, usize) {
        let time_and_random = || {
            SystemAccess::new()
                .read_service(Service::Time)
                .read_service(Service::Random)
        };
        let world = game(mode)
            .lockstep(net)
            .add(Phase::BeforePhysics, play_along)
            .add(
                Phase::AfterPhysics,
                accelerate.access(time_and_random().write::<Unit>().read_resource::<Steering>()),
            )
            .add(Phase::AfterPhysics, cool.access(time_and_random().write::<Heat>()))
            .add(
                Phase::AfterPhysics,
                movement.access(time_and_random().write::<Transform2D>().read::<Unit>()),
            )
            .add(
                Phase::AfterPhysics,
                burn_out.access(
                    SystemAccess::new()
                        .read::<Unit>()
                        .read::<Heat>()
                        .send_events::<Burned>()
                        .commands(),
                ),
            )
            .add(
                Phase::SimLast,
                count_burned.access(SystemAccess::new().read_events::<Burned>().write_resource::<Score>()),
            )
            .sim_system(move |ctx: &mut Ctx| {
                if ctx.time.sim_tick() + 1 == ticks {
                    let desync = ctx.net.desync().cloned();
                    ctx.world.insert_resource(desync);
                }
            })
            .run_headless(ticks, |ctx| {
                ctx.world.reflect::<Unit>();
                ctx.world.reflect::<Heat>();
                ctx.world.insert_resource(Score(0));
                let mut rng = ctx.random().stream(7);
                for i in 0..300 {
                    ctx.world.spawn((
                        Transform2D::at(vec2(rng.range(0.0, 400.0), rng.range(0.0, 400.0))),
                        Unit {
                            owner: (i % 2) as u32,
                            vel: Vec2::ZERO,
                            fuel: rng.range(0.5, 3.0),
                        },
                        Heat(rng.range(0.0, 10.0)),
                    ));
                }
            });
        let desync = world.resource::<Option<Desync>>().clone();
        let burned = world.resource::<Score>().0;
        (world.save(), desync, burned, world.len())
    }

    /// Runs two peers over loopback, each with its own mode.
    fn play(modes: [Mode; 2], ticks: u64) -> Vec<(ponko::reflect::json::Value, Option<Desync>, i64, usize)> {
        let runs: Vec<_> = (Net::loopback(2).into_iter().zip(modes))
            .map(|(net, mode)| thread::spawn(move || peer(net, mode, ticks)))
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    }

    #[test]
    fn peers_with_and_without_parallel_systems_stay_in_sync() {
        for modes in [
            [Mode::Off, Mode::Threads],
            [Mode::Threads, Mode::InOrder],
            [Mode::Threads, Mode::Threads],
        ] {
            let results = play(modes, 400);
            for (save, desync, burned, len) in &results {
                assert!(desync.is_none(), "{modes:?}: {desync:?}");
                assert_eq!(*save, results[0].0, "{modes:?}: the worlds differ");
                assert_eq!((*burned, *len), (results[0].2, results[0].3), "{modes:?}");
            }
            // The run exercised commands and events, not just movement.
            assert!(results[0].2 > 0, "{modes:?}: nothing burned out");
            assert!(results[0].3 < 300, "{modes:?}");
        }
    }
}

// ---- differential ------------------------------------------------------------------------

mod differential {
    use super::*;

    /// A small seeded generator (SplitMix64).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[derive(Debug, PartialEq, Clone, Copy, Component)]
    struct C<const N: usize>(i64);
    struct R<const N: usize>(i64);
    #[derive(Clone, Copy)]
    struct E<const N: usize>(i64);

    impl<const N: usize> E<N> {
        fn of(v: i64) -> Self {
            E(v)
        }
    }

    const COMPONENTS: u64 = 4;
    const RESOURCES: u64 = 2;
    const EVENTS: u64 = 2;
    const MAX_ENTITIES: usize = 160;

    /// One thing a generated system does. Every result feeds the system's
    /// running value, and every write uses it, so anything a system sees
    /// differently spreads into the world.
    #[derive(Clone, Copy, Debug)]
    enum Op {
        Read(u64),
        Write(u64),
        Changed(u64),
        ReadResource(u64),
        WriteResource(u64),
        Send(u64),
        ReadEvents(u64),
        Spawn,
        Random,
        Tick,
        /// The change tick the system writes in.
        ChangeTick,
    }

    /// Runs `$body` with `$t` naming the type number `$i` of `$family`.
    macro_rules! with_type {
        ($family:ident, $i:expr, $t:ident => $body:expr) => {
            match $i {
                0 => {
                    type $t = $family<0>;
                    $body
                }
                1 => {
                    type $t = $family<1>;
                    $body
                }
                2 => {
                    type $t = $family<2>;
                    $body
                }
                _ => {
                    type $t = $family<3>;
                    $body
                }
            }
        };
    }

    fn mix(acc: i64, v: i64) -> i64 {
        acc.wrapping_mul(31).wrapping_add(v)
    }

    fn declare(ops: &[Op]) -> SystemAccess {
        let mut a = SystemAccess::new();
        for &op in ops {
            a = match op {
                Op::Read(i) | Op::Changed(i) => with_type!(C, i, T => a.read::<T>()),
                Op::Write(i) => with_type!(C, i, T => a.write::<T>()),
                Op::ReadResource(i) => with_type!(R, i, T => a.read_resource::<T>()),
                Op::WriteResource(i) => with_type!(R, i, T => a.write_resource::<T>()),
                Op::Send(i) => with_type!(E, i, T => a.send_events::<T>()),
                Op::ReadEvents(i) => with_type!(E, i, T => a.read_events::<T>()),
                Op::Spawn => a.commands(),
                Op::Random => a.read_service(Service::Random),
                Op::Tick => a.read_service(Service::Time),
                Op::ChangeTick => a,
            };
        }
        a
    }

    /// The generated system as a view system.
    fn run_view(ops: &[Op], salt: i64, sys: &mut SystemView) {
        let mut acc = salt;
        for &op in ops {
            match op {
                Op::Read(i) => with_type!(C, i, T => {
                    sys.world.query::<&T>().for_each(|c| acc = mix(acc, c.0));
                }),
                Op::Write(i) => with_type!(C, i, T => {
                    let mut k = acc;
                    sys.world.query::<&mut T>().for_each(|c| {
                        k = mix(k, c.0);
                        c.0 = k;
                    });
                    acc = k;
                }),
                Op::Changed(i) => with_type!(C, i, T => {
                    acc = mix(acc, sys.world.query::<Entity>().filter::<Changed<T>>().count() as i64);
                }),
                Op::ReadResource(i) => with_type!(R, i, T => acc = mix(acc, sys.world.resource::<T>().0)),
                Op::WriteResource(i) => with_type!(R, i, T => {
                    let r = sys.world.resource_mut::<T>();
                    r.0 = mix(r.0, acc);
                    acc = r.0;
                }),
                Op::Send(i) => with_type!(E, i, T => {
                    sys.events.send(T::of(acc));
                    sys.events.send(T::of(acc ^ 1));
                }),
                Op::ReadEvents(i) => with_type!(E, i, T => {
                    for e in sys.events.read::<T>() {
                        acc = mix(acc, e.0);
                    }
                }),
                Op::Spawn => {
                    if sys.world.len() < MAX_ENTITIES {
                        sys.commands.spawn((C::<0>(acc), C::<2>(acc ^ 5)));
                    }
                }
                Op::Random => acc = mix(acc, sys.random().stream(3).u64() as i64),
                Op::Tick => acc = mix(acc, sys.time.sim_tick() as i64),
                Op::ChangeTick => acc = mix(acc, sys.world.change_tick().to_bits() as i64),
            }
        }
    }

    /// The same system with the whole `Ctx`: it runs alone.
    fn run_ctx(ops: &[Op], salt: i64, ctx: &mut Ctx) {
        let mut acc = salt;
        for &op in ops {
            match op {
                Op::Read(i) => with_type!(C, i, T => {
                    ctx.world.query::<&T>().for_each(|c| acc = mix(acc, c.0));
                }),
                Op::Write(i) => with_type!(C, i, T => {
                    let mut k = acc;
                    ctx.world.query::<&mut T>().for_each(|c| {
                        k = mix(k, c.0);
                        c.0 = k;
                    });
                    acc = k;
                }),
                Op::Changed(i) => with_type!(C, i, T => {
                    acc = mix(acc, ctx.world.query::<Entity>().filter::<Changed<T>>().count() as i64);
                }),
                Op::ReadResource(i) => with_type!(R, i, T => acc = mix(acc, ctx.world.resource::<T>().0)),
                Op::WriteResource(i) => with_type!(R, i, T => {
                    let r = ctx.world.resource_mut::<T>();
                    r.0 = mix(r.0, acc);
                    acc = r.0;
                }),
                Op::Send(i) => with_type!(E, i, T => {
                    ctx.events.send(T::of(acc));
                    ctx.events.send(T::of(acc ^ 1));
                }),
                Op::ReadEvents(i) => with_type!(E, i, T => {
                    for e in ctx.events.read::<T>() {
                        acc = mix(acc, e.0);
                    }
                }),
                Op::Spawn => {
                    if ctx.world.len() < MAX_ENTITIES {
                        ctx.commands.spawn((C::<0>(acc), C::<2>(acc ^ 5)));
                    }
                }
                Op::Random => acc = mix(acc, ctx.random().stream(3).u64() as i64),
                Op::Tick => acc = mix(acc, ctx.time.sim_tick() as i64),
                Op::ChangeTick => acc = mix(acc, ctx.world.change_tick().to_bits() as i64),
            }
        }
    }

    /// One generated system.
    struct Spec {
        ops: Vec<Op>,
        /// A view system (else it takes `&mut Ctx`).
        view: bool,
        /// Only for `Ctx` systems: run every this many ticks.
        every: u32,
        barrier: bool,
        phase: Phase,
    }

    fn specs(seed: u64) -> Vec<Spec> {
        let mut rng = Rng(seed);
        let n = 4 + rng.below(10);
        (0..n)
            .map(|_| {
                let ops = (0..1 + rng.below(4))
                    .map(|_| match rng.below(15) {
                        0..=2 => Op::Read(rng.below(COMPONENTS)),
                        3..=5 => Op::Write(rng.below(COMPONENTS)),
                        6 => Op::Changed(rng.below(COMPONENTS)),
                        7 => Op::ReadResource(rng.below(RESOURCES)),
                        8 => Op::WriteResource(rng.below(RESOURCES)),
                        9 => Op::Send(rng.below(EVENTS)),
                        10 => Op::ReadEvents(rng.below(EVENTS)),
                        11 => Op::Spawn,
                        12 => Op::Random,
                        13 => Op::ChangeTick,
                        _ => Op::Tick,
                    })
                    .collect();
                let view = rng.below(5) != 0;
                Spec {
                    ops,
                    view,
                    every: if !view && rng.below(3) == 0 { 3 } else { 1 },
                    barrier: rng.below(9) == 0,
                    phase: if rng.below(4) == 0 {
                        Phase::SimLast
                    } else {
                        Phase::AfterPhysics
                    },
                }
            })
            .collect()
    }

    /// Everything the run left behind.
    type Outcome = (Vec<(u64, [Option<i64>; 4])>, [i64; 2], usize);

    /// How the plan of a generated game came out.
    #[derive(Default)]
    struct Planned {
        batches: usize,
        /// Systems in an earlier stage than a system registered before them.
        moved: usize,
    }

    /// Only view systems, every tick, no barriers, few commands: schedules
    /// where systems can move ahead of each other.
    fn view_specs(seed: u64) -> Vec<Spec> {
        let mut specs = specs(seed);
        for (i, spec) in specs.iter_mut().enumerate() {
            spec.view = true;
            spec.every = 1;
            spec.barrier = false;
            if i % 4 != 0 {
                spec.ops.retain(|op| !matches!(op, Op::Spawn));
                if spec.ops.is_empty() {
                    spec.ops.push(Op::Read(i as u64 % COMPONENTS));
                }
            }
        }
        specs
    }

    fn run(seed: u64, mode: Mode, ticks: u64) -> (Outcome, Planned) {
        run_specs(seed, specs(seed), mode, ticks)
    }

    fn run_specs(seed: u64, specs: Vec<Spec>, mode: Mode, ticks: u64) -> (Outcome, Planned) {
        let mut game = game(mode).seed(seed);
        for (salt, spec) in specs.into_iter().enumerate() {
            let salt = salt as i64 + 1;
            if spec.barrier {
                game = game.barrier(spec.phase);
            }
            let ops = spec.ops;
            game = if spec.view {
                let access = declare(&ops);
                game.add(
                    spec.phase,
                    (move |sys: &mut SystemView| {
                        run_view(&ops, salt, sys);
                    })
                    .access(access),
                )
            } else if spec.every > 1 {
                game.add(
                    Phase::AfterPhysics,
                    (move |ctx: &mut Ctx| run_ctx(&ops, salt, ctx)).every(spec.every),
                )
            } else {
                game.add(spec.phase, move |ctx: &mut Ctx| run_ctx(&ops, salt, ctx))
            };
        }
        let plan = game.schedule_plan();
        let stage_of: Vec<usize> = (0..plan.systems.len())
            .map(|i| plan.stages.iter().position(|s| s.members.contains(&i)).unwrap())
            .collect();
        let planned = Planned {
            batches: plan.batches().count(),
            moved: (0..stage_of.len())
                .filter(|&s| (0..s).any(|t| stage_of[t] > stage_of[s]))
                .count(),
        };
        let mut world = game.run_headless(ticks, |ctx| {
            ctx.world.track_changes::<C<0>>();
            ctx.world.track_changes::<C<1>>();
            ctx.world.track_changes::<C<2>>();
            ctx.world.track_changes::<C<3>>();
            ctx.world.insert_resource(R::<0>(1));
            ctx.world.insert_resource(R::<1>(2));
            for i in 0..48i64 {
                let e = ctx.world.spawn((C::<0>(i),));
                if i % 2 == 0 {
                    ctx.world.insert(e, C::<1>(i * 3));
                }
                if i % 3 == 0 {
                    ctx.world.insert(e, C::<2>(i * 5));
                }
                if i % 5 == 0 {
                    ctx.world.insert(e, C::<3>(i * 7));
                }
            }
        });
        let entities: Vec<Entity> = world.query::<Entity>().iter().collect();
        let mut rows: Vec<(u64, [Option<i64>; 4])> = entities
            .into_iter()
            .map(|e| {
                (
                    e.to_bits(),
                    [
                        world.get::<C<0>>(e).map(|c| c.0),
                        world.get::<C<1>>(e).map(|c| c.0),
                        world.get::<C<2>>(e).map(|c| c.0),
                        world.get::<C<3>>(e).map(|c| c.0),
                    ],
                )
            })
            .collect();
        rows.sort_unstable();
        let resources = [world.resource::<R<0>>().0, world.resource::<R<1>>().0];
        ((rows, resources, world.len()), planned)
    }

    fn seeds() -> Vec<u64> {
        let parse = |name: &str| {
            let v = std::env::var(name).ok()?;
            match v.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None => v.parse().ok(),
            }
        };
        if let Some(seed) = parse("PONKO_SCHEDULE_SEED") {
            return vec![seed];
        }
        let cases = parse("PONKO_SCHEDULE_CASES").unwrap_or(60);
        (0..cases).map(|i| 0x5EED_0000 + i * 7919).collect()
    }

    #[test]
    fn random_systems_come_out_the_same_alone_in_order_and_on_threads() {
        let (mut batches, mut moved, mut cases) = (0, 0, 0);
        for seed in seeds() {
            let (reference, planned) = run(seed, Mode::Off, 24);
            for mode in [Mode::InOrder, Mode::Threads] {
                let (outcome, _) = run(seed, mode, 24);
                assert!(
                    outcome == reference,
                    "seed {seed:#x}: {mode:?} differs from one system at a time.\n\
                     Rerun with PONKO_SCHEDULE_SEED={seed:#x}\nsystems: {:?}",
                    specs(seed)
                        .iter()
                        .map(|s| (s.view, s.every, s.barrier, s.phase, &s.ops))
                        .collect::<Vec<_>>()
                );
            }
            batches += planned.batches;
            moved += planned.moved;
            cases += 1;
        }
        // The generator must actually produce batches, and systems that run
        // ahead of ones registered before them, or nothing was tested.
        assert!(cases == 1 || batches >= cases / 2, "{batches} batches in {cases} cases");
        assert!(
            cases == 1 || moved >= cases / 10,
            "{moved} moved systems in {cases} cases"
        );
    }

    /// The same for schedules of view systems only, where many systems run
    /// ahead of ones registered before them.
    #[test]
    fn random_view_systems_that_move_ahead_come_out_the_same() {
        let (mut moved, mut cases) = (0, 0);
        for seed in seeds() {
            let (reference, planned) = run_specs(seed, view_specs(seed), Mode::Off, 24);
            for mode in [Mode::InOrder, Mode::Threads] {
                let (outcome, _) = run_specs(seed, view_specs(seed), mode, 24);
                assert!(
                    outcome == reference,
                    "seed {seed:#x}: {mode:?} differs from one system at a time.\n\
                     Rerun with PONKO_SCHEDULE_SEED={seed:#x}\nsystems: {:?}",
                    view_specs(seed).iter().map(|s| (s.phase, &s.ops)).collect::<Vec<_>>()
                );
            }
            moved += planned.moved;
            cases += 1;
        }
        assert!(
            cases == 1 || moved >= cases / 2,
            "{moved} moved systems in {cases} cases"
        );
    }

    /// The same, on thread pools of different sizes: the number of threads
    /// never shows in the result.
    #[test]
    fn random_systems_come_out_the_same_with_any_thread_count() {
        for seed in seeds().into_iter().take(16) {
            let (reference, _) = run(seed, Mode::Off, 16);
            for threads in [1, 2, 3, 8] {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
                let (outcome, _) = pool.install(|| run(seed, Mode::Threads, 16));
                assert!(
                    outcome == reference,
                    "seed {seed:#x}: {threads} threads differ from one system at a time. \
                     Rerun with PONKO_SCHEDULE_SEED={seed:#x}"
                );
            }
        }
    }

    /// Generated view systems, each with a declaration, record when they
    /// start and end. Run on threads, no two systems that conflict may ever
    /// overlap, and a system that must see another's work starts after it
    /// ended.
    #[test]
    fn random_systems_that_conflict_never_overlap() {
        use std::sync::Mutex;
        #[derive(Clone, Copy)]
        struct Span {
            start: u64,
            end: u64,
        }
        let mut overlaps = 0;
        for seed in seeds().into_iter().take(24) {
            let specs: Vec<Vec<Op>> = view_specs(seed).into_iter().map(|s| s.ops).collect();
            let accesses: Vec<SystemAccess> = specs.iter().map(|ops| declare(ops)).collect();
            let clock = Arc::new(AtomicU32::new(0));
            let running = Arc::new(Mutex::new(Vec::<usize>::new()));
            let spans = Arc::new(Mutex::new(vec![Vec::<Span>::new(); specs.len()]));
            let conflict = |t: usize, s: usize| {
                let (a, b) = if t < s { (t, s) } else { (s, t) };
                !accesses[a].conflicts_before(&accesses[b]).is_empty()
            };
            let conflicts: Arc<Vec<Vec<bool>>> = Arc::new(
                (0..specs.len())
                    .map(|i| (0..specs.len()).map(|j| i != j && conflict(i, j)).collect())
                    .collect(),
            );
            let mut game = game(Mode::Threads).seed(seed);
            for (me, ops) in specs.iter().enumerate() {
                let (ops, clock, running, spans, conflicts) = (
                    ops.clone(),
                    clock.clone(),
                    running.clone(),
                    spans.clone(),
                    conflicts.clone(),
                );
                game = game.add(
                    Phase::AfterPhysics,
                    (move |sys: &mut SystemView| {
                        {
                            let mut inside = running.lock().unwrap();
                            if let Some(&other) = inside.iter().find(|&&o| conflicts[me][o]) {
                                panic!("seed {seed:#x}: systems {other} and {me} conflict but overlap");
                            }
                            inside.push(me);
                        }
                        let start = u64::from(clock.fetch_add(1, Ordering::SeqCst));
                        run_view(&ops, me as i64, sys);
                        std::thread::sleep(Duration::from_micros(300));
                        let end = u64::from(clock.fetch_add(1, Ordering::SeqCst));
                        running.lock().unwrap().retain(|&o| o != me);
                        spans.lock().unwrap()[me].push(Span { start, end });
                    })
                    .access(accesses[me].clone()),
                );
            }
            let _ = game.run_headless(3, |ctx| {
                ctx.world.track_changes::<C<0>>();
                ctx.world.track_changes::<C<1>>();
                ctx.world.track_changes::<C<2>>();
                ctx.world.track_changes::<C<3>>();
                ctx.world.insert_resource(R::<0>(1));
                ctx.world.insert_resource(R::<1>(2));
                for i in 0..8i64 {
                    ctx.world.spawn((C::<0>(i), C::<1>(i), C::<2>(i), C::<3>(i)));
                }
            });
            let spans = spans.lock().unwrap();
            for tick in 0..3 {
                for s in 0..specs.len() {
                    for t in 0..s {
                        let (a, b) = (spans[t][tick], spans[s][tick]);
                        // (Systems whose order only shows may share a batch,
                        // where either may finish first: the plan's tests
                        // and the differential ones check those.)
                        if !accesses[t].conflicts_before(&accesses[s]).is_empty() {
                            assert!(a.end < b.start, "seed {seed:#x}: {s} started before {t} ended");
                        }
                        if a.start < b.end && b.start < a.end {
                            overlaps += 1;
                        }
                    }
                }
            }
        }
        // On one core nothing overlaps; elsewhere something must, or the
        // test checked nothing.
        assert!(!has_threads() || overlaps > 0, "no two systems ever overlapped");
    }

    #[test]
    fn a_threaded_run_repeats_exactly() {
        for seed in seeds().into_iter().take(8) {
            assert_eq!(
                run(seed, Mode::Threads, 40).0,
                run(seed, Mode::Threads, 40).0,
                "seed {seed:#x}"
            );
        }
    }
}

// ---- the guide's example ---------------------------------------------------------------

mod guide {
    use ponko::access::{Service, SystemAccess};
    use ponko::prelude::*;
    use ponko::system_view::SystemView;

    // ANCHOR: parallel_systems
    #[derive(Component)]
    struct Pos(f32);
    #[derive(Component)]
    struct Vel(f32);
    #[derive(Component)]
    struct Health(f32);
    struct Gravity(f32);

    fn fall(sys: &mut SystemView) {
        let g = sys.world.resource::<Gravity>().0;
        let dt = sys.dt();
        sys.world.query::<&mut Vel>().for_each(|v| v.0 += g * dt);
    }

    fn regenerate(sys: &mut SystemView) {
        let dt = sys.dt();
        sys.world
            .query::<&mut Health>()
            .for_each(|h| h.0 = (h.0 + dt).min(100.0));
    }

    fn movement(sys: &mut SystemView) {
        let dt = sys.dt();
        sys.world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| p.0 += v.0 * dt);
    }

    fn game() -> Game {
        let time = || SystemAccess::new().read_service(Service::Time);
        Game::new("falling", 320, 240)
            // Off by default. On, view systems that don't conflict share a batch.
            .parallel_systems(true)
            .add(
                Phase::AfterPhysics,
                fall.access(time().write::<Vel>().read_resource::<Gravity>()),
            )
            .add(Phase::AfterPhysics, regenerate.access(time().write::<Health>()))
            // Reads what `fall` writes: it waits for `fall`'s batch to end.
            .add(
                Phase::AfterPhysics,
                movement.access(time().write::<Pos>().read::<Vel>()),
            )
    }
    // ANCHOR_END: parallel_systems

    #[test]
    fn the_guide_example_plans_two_stages_and_runs() {
        // ANCHOR: plan
        let plan = game().schedule_plan();
        let stages = plan.stages_in(Phase::AfterPhysics);
        // `fall` and `regenerate` together, then `movement`.
        assert_eq!(stages.len(), 2);
        assert_eq!(stages[0].len, 2);
        assert!(plan.to_string().contains("conflict on component"));
        // ANCHOR_END: plan
        let mut world = game().run_headless(60, |ctx| {
            ctx.world.insert_resource(Gravity(-10.0));
            ctx.world.spawn((Pos(0.0), Vel(0.0), Health(1.0)));
        });
        let mut rows = world.query::<(&Pos, &Vel, &Health)>();
        let (p, v, h) = rows.iter().map(|(p, v, h)| (p.0, v.0, h.0)).next().unwrap();
        assert!((v + 10.0).abs() < 1e-3, "{v}");
        assert!(p < -4.9 && p > -5.2, "{p}");
        assert!((h - 2.0).abs() < 1e-3, "{h}");
    }
}
