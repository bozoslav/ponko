//! `Ctx::ecs_diagnostics`: archetypes, component storage and the system
//! schedule as stable, machine-readable data, read headlessly.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use ponko::access::{Service, SystemAccess};

#[allow(unused_imports)]
use ponko::diagnostics::EcsDiagnostics;
#[allow(unused_imports)]
use ponko::input::Input;
use ponko::prelude::*;
use ponko::system_view::SystemView;

#[derive(Component)]
struct Pos(#[allow(dead_code)] [f32; 2]);
#[derive(Component)]
struct Vel(#[allow(dead_code)] [f32; 2]);
#[derive(Component)]
struct Tag;

fn early(_: &mut Ctx) {}
fn moving(_: &mut Ctx) {}
fn rare(_: &mut Ctx) {}

/// Runs `ticks` headless ticks and returns the diagnostics read from a
/// `SimLast` system on the final tick.
fn diagnose(game: Game, ticks: u64, setup: impl FnOnce(&mut Ctx) + 'static) -> EcsDiagnostics {
    let out = Rc::new(RefCell::new(None));
    let sink = out.clone();
    let _world = game
        .add(Phase::SimLast, move |ctx: &mut Ctx| {
            *sink.borrow_mut() = Some(ctx.ecs_diagnostics());
        })
        .run_headless(ticks, setup);
    out.take().expect("the SimLast system ran")
}

#[test]
fn archetypes_list_their_components_entities_and_storage() {
    let d = diagnose(Game::new("t", 1, 1), 1, |ctx| {
        for _ in 0..3 {
            ctx.world.spawn((Pos([0.0; 2]), Vel([1.0; 2])));
        }
        ctx.world.spawn((Pos([0.0; 2]),));
        ctx.world.spawn((Tag,));
    });
    assert_eq!(d.version, ponko::diagnostics::ECS_DIAGNOSTICS_VERSION);
    assert_eq!(d.entities, 5);
    let with = |names: &[&str]| {
        d.archetypes
            .iter()
            .find(|a| {
                a.components.len() == names.len() && names.iter().all(|n| a.components.iter().any(|c| c.ends_with(n)))
            })
            .unwrap_or_else(|| panic!("no archetype {names:?} in {:#?}", d.archetypes))
    };
    let moving = with(&["::Pos", "::Vel"]);
    assert_eq!(moving.entities, 3);
    assert!(moving.bytes_per_entity >= 16 + 8, "two [f32; 2] plus the handle");
    assert!(moving.allocated_bytes >= 3 * 16);
    assert_eq!(with(&["::Pos"]).entities, 1);
    assert_eq!(with(&["::Tag"]).entities, 1);
    for (i, a) in d.archetypes.iter().enumerate() {
        assert_eq!(a.index, i, "indices are the world's archetype indices");
    }
    let total: usize = d.archetypes.iter().map(|a| a.entities).sum();
    assert_eq!(total, d.entities);
    assert!(d.allocated_bytes >= moving.allocated_bytes);

    let pos = d.components.iter().find(|c| c.name.ends_with("::Pos")).unwrap();
    assert_eq!(pos.short_name, "Pos");
    assert_eq!((pos.size, pos.align), (8, 4));
    assert_eq!((pos.archetypes, pos.entities), (2, 4));
    assert_eq!(pos.bytes, 4 * 8);
    let tag = d.components.iter().find(|c| c.short_name == "Tag").unwrap();
    assert_eq!((tag.size, tag.entities, tag.bytes), (0, 1, 0));
}

#[test]
fn the_schedule_lists_systems_in_run_order_with_phases() {
    let game = Game::new("t", 1, 1)
        .sim_system(moving)
        .add(Phase::Input, early)
        .add(Phase::AfterPhysics, rare.every(4))
        .system(|_| {});
    let d = diagnose(game, 3, |_| {});
    let sim: Vec<_> = d.systems.iter().filter(|s| s.sim).collect();
    let phases: Vec<_> = sim.iter().map(|s| s.phase.as_str()).collect();
    assert_eq!(phases, ["Input", "AfterPhysics", "AfterPhysics", "SimLast"]);
    assert!(sim[0].name.ends_with("::early"));
    assert!(sim[1].name.ends_with("::moving"));
    assert!(sim[2].name.ends_with("::rare"));
    let rare = sim.iter().find(|s| s.name.ends_with("::rare")).unwrap();
    assert_eq!(rare.every, 4);
    assert_eq!(sim[0].every, 1);
    for (i, s) in d.systems.iter().enumerate() {
        assert_eq!(s.order, i, "order is the position in the schedule");
        assert_eq!(s.access, "exclusive", "undeclared systems may touch anything");
    }
    // Frame and draw systems are listed (they don't run headlessly).
    let frame = d.systems.iter().find(|s| s.phase == "Frame").unwrap();
    assert!(!frame.sim);
    assert!(
        d.systems
            .iter()
            .any(|s| s.phase == "Draw" && s.name.ends_with("draw_sprites"))
    );
    let draw_at = d.systems.iter().position(|s| s.phase == "Draw").unwrap();
    assert!(d.systems[draw_at..].iter().all(|s| s.phase == "Draw"), "Draw is last");
}

#[test]
fn diagnostics_round_trip_through_json_with_stable_field_names() {
    let d = diagnose(Game::new("t", 1, 1).sim_system(moving), 1, |ctx| {
        ctx.world.spawn((Pos([0.0; 2]),));
    });
    let json: serde_json::Value = serde_json::to_value(&d).unwrap();
    for key in [
        "version",
        "entities",
        "allocated_bytes",
        "archetypes",
        "components",
        "systems",
        "schedule",
        "query_cache",
    ] {
        assert!(json.get(key).is_some(), "missing `{key}` in {json}");
    }
    for key in ["index", "components", "entities", "bytes_per_entity", "allocated_bytes"] {
        assert!(json["archetypes"][0].get(key).is_some(), "archetype `{key}`");
    }
    for key in ["name", "short_name", "size", "align", "archetypes", "entities", "bytes"] {
        assert!(json["components"][0].get(key).is_some(), "component `{key}`");
    }
    for key in [
        "name", "phase", "order", "sim", "every", "offset", "access", "declared", "stage", "alone", "last_ms", "avg_ms",
    ] {
        assert!(json["systems"][0].get(key).is_some(), "system `{key}`");
    }
    for key in ["parallel", "threads", "parallel_threshold_us", "stages", "dependencies"] {
        assert!(json["schedule"].get(key).is_some(), "schedule `{key}`");
    }
    for key in [
        "index",
        "phase",
        "first",
        "len",
        "batch",
        "boundary",
        "after",
        "conflicts",
        "threaded_runs",
        "inline_runs",
        "usual_work_us",
    ] {
        assert!(json["schedule"]["stages"][0].get(key).is_some(), "stage `{key}`");
    }
    for key in [
        "enabled",
        "min_archetypes",
        "signatures",
        "indexed",
        "indices",
        "bytes",
        "hits",
        "evictions",
    ] {
        assert!(json["query_cache"].get(key).is_some(), "query cache `{key}`");
    }
    let back: EcsDiagnostics = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, d);
    // A snapshot written before the schedule fields existed still reads.
    let mut old = json;
    let fields = old.as_object_mut().unwrap();
    fields.remove("schedule");
    fields.remove("query_cache");
    for system in old["systems"].as_array_mut().unwrap() {
        let system = system.as_object_mut().unwrap();
        for key in ["declared", "stage", "alone"] {
            system.remove(key);
        }
    }
    let old: EcsDiagnostics = serde_json::from_value(old).unwrap();
    assert_eq!(old.systems.len(), d.systems.len());
    assert!(old.schedule.stages.is_empty());
}

#[test]
fn matching_counts_archetypes_and_entities_for_component_names() {
    let d = diagnose(Game::new("t", 1, 1), 1, |ctx| {
        ctx.world.spawn((Pos([0.0; 2]), Vel([0.0; 2])));
        ctx.world.spawn((Pos([0.0; 2]), Vel([0.0; 2]), Tag));
        ctx.world.spawn((Pos([0.0; 2]),));
    });
    let m = d.matching(&["Pos", "Vel"]).unwrap();
    assert_eq!((m.archetypes, m.entities), (2, 2));
    assert_eq!(d.matching(&["Pos"]).unwrap().entities, 3);
    assert_eq!(d.matching(&[]).unwrap().entities, 3);
    let err = d.matching(&["Nope"]).unwrap_err();
    assert!(err.message().contains("Nope"), "{err}");
}

#[derive(Component)]
struct Health(#[allow(dead_code)] f32);
struct Gravity(#[allow(dead_code)] f32);
struct Hit;

fn movement(_: &mut SystemView) {}
fn regenerate(_: &mut SystemView) {}
fn stop(_: &mut SystemView) {}
fn spawner(_: &mut SystemView) {}
fn sounds(_: &mut SystemView) {}
fn echo(_: &mut SystemView) {}
fn checked(_: &mut Ctx) {}

/// A game whose environment can't change how its systems run.
fn game() -> Game {
    Game::new("t", 1, 1).parallel_systems_env(false)
}

#[test]
fn systems_list_their_declared_access_by_name() {
    let game = game()
        .add(
            Phase::AfterPhysics,
            movement.access(
                SystemAccess::new()
                    .write::<Pos>()
                    .read::<Vel>()
                    .read_resource::<Gravity>()
                    .read_events::<Hit>()
                    .read_service(Service::Time),
            ),
        )
        .add(
            Phase::AfterPhysics,
            sounds.access(
                SystemAccess::new()
                    .send_events::<Hit>()
                    .commands()
                    .write_service(Service::Audio),
            ),
        )
        .add(
            Phase::AfterPhysics,
            checked.access(SystemAccess::new().write::<Health>()),
        )
        .add(Phase::AfterPhysics, moving.access(SystemAccess::exclusive()))
        .sim_system(early);
    let d = diagnose(game, 1, |_| {});

    let movement = d.system("movement").unwrap();
    assert_eq!(movement.access, "view");
    let a = movement.declared.as_ref().unwrap();
    assert!(!a.exclusive && !a.commands);
    let ends = |names: &[String], end: &str| names.len() == 1 && names[0].ends_with(end);
    assert!(ends(&a.components_written, "::Pos"), "{a:?}");
    assert!(ends(&a.components_read, "::Vel"), "{a:?}");
    assert!(ends(&a.resources_read, "::Gravity"), "{a:?}");
    assert!(ends(&a.events_read, "::Hit"), "{a:?}");
    assert_eq!(a.services_read, ["time"]);
    assert!(a.resources_written.is_empty() && a.events_sent.is_empty() && a.services_written.is_empty());

    let sounds = d.system("sounds").unwrap().declared.as_ref().unwrap();
    assert!(sounds.commands);
    assert!(ends(&sounds.events_sent, "::Hit"), "{sounds:?}");
    assert_eq!(sounds.services_written, ["audio"]);

    // A declared `&mut Ctx` system is listed with its declaration, but it
    // is not a view: it still runs alone.
    let checked = d.system("checked").unwrap();
    assert_eq!(checked.access, "declared");
    assert!(ends(&checked.declared.as_ref().unwrap().components_written, "::Health"));
    assert_eq!(checked.alone.as_ref().unwrap().reason, "ctx_system");

    let exclusive = d.system("moving").unwrap();
    assert_eq!(exclusive.access, "exclusive");
    assert!(exclusive.declared.as_ref().unwrap().exclusive);
    assert_eq!(exclusive.alone.as_ref().unwrap().reason, "undeclared");

    let undeclared = d.system("early").unwrap();
    assert_eq!(undeclared.access, "exclusive");
    assert_eq!(undeclared.declared, None);
    assert_eq!(undeclared.alone.as_ref().unwrap().reason, "undeclared");
}

#[test]
fn stages_say_which_systems_share_a_batch_and_why_each_stage_starts() {
    let build = || {
        game()
            .add(
                Phase::AfterPhysics,
                movement.access(SystemAccess::new().write::<Pos>().read::<Vel>()),
            )
            .add(
                Phase::AfterPhysics,
                regenerate.access(SystemAccess::new().write::<Health>()),
            )
            .add(Phase::AfterPhysics, stop.access(SystemAccess::new().write::<Vel>()))
            .barrier(Phase::AfterPhysics)
            .add(
                Phase::AfterPhysics,
                spawner.access(SystemAccess::new().read::<Health>()),
            )
            .sim_system(early)
    };
    let d = diagnose(build(), 1, |_| {});
    let plan = build().schedule_plan();

    // Every system is in exactly one stage, and the stages are the plan's.
    assert_eq!(
        d.schedule.stages.len(),
        plan.stages.len() + 1,
        "plus the test's SimLast reader"
    );
    let mut covered = 0;
    for (i, stage) in d.schedule.stages.iter().enumerate() {
        assert_eq!(stage.index, i);
        assert_eq!(stage.first, covered, "stages cover the systems in order");
        assert_eq!(stage.batch, stage.len > 1);
        for system in &d.systems[stage.first..stage.first + stage.len] {
            assert_eq!(system.stage, Some(i));
            assert_eq!(system.phase, stage.phase);
        }
        covered += stage.len;
    }
    assert_eq!(covered, d.systems.len());

    let stage_of = |name: &str| &d.schedule.stages[d.system(name).unwrap().stage.unwrap()];
    let batch = stage_of("movement");
    assert_eq!((batch.len, batch.batch), (2, true));
    assert_eq!(batch.boundary, "phase");
    assert_eq!(
        d.system("regenerate").unwrap().stage,
        d.system("movement").unwrap().stage
    );
    assert_eq!(d.system("movement").unwrap().alone, None, "it can share a batch");

    // `stop` writes what `movement` reads.
    let stop = stage_of("stop");
    assert_eq!(stop.boundary, "conflict");
    assert_eq!(stop.after, Some(d.system("movement").unwrap().order));
    assert_eq!(stop.conflicts.len(), 1);
    assert_eq!(stop.conflicts[0].kind, "component");
    assert!(stop.conflicts[0].name.as_deref().unwrap().ends_with("::Vel"));

    // `spawner` conflicts with nothing in `stop`'s stage; only the
    // explicit barrier keeps it out.
    let spawner = stage_of("spawner");
    assert_eq!(spawner.boundary, "barrier");
    assert_eq!((spawner.after, spawner.conflicts.len()), (None, 0));

    // An undeclared system is a stage of its own.
    let early = stage_of("early");
    assert_eq!((early.len, early.boundary.as_str()), (1, "alone"));
}

#[test]
fn a_system_that_moved_ahead_says_which_system_blocked_it_and_why() {
    // `stop` writes what `movement` reads, so it waits; `regenerate`,
    // registered after `stop`, touches neither and joins `movement`'s batch.
    // `spawner` reads what `regenerate` writes.
    let game = game()
        .add(
            Phase::AfterPhysics,
            movement.access(SystemAccess::new().write::<Pos>().read::<Vel>()),
        )
        .add(Phase::AfterPhysics, stop.access(SystemAccess::new().write::<Vel>()))
        .add(
            Phase::AfterPhysics,
            regenerate.access(SystemAccess::new().write::<Health>()),
        )
        .add(
            Phase::AfterPhysics,
            spawner.access(SystemAccess::new().read::<Health>()),
        );
    let d = diagnose(game, 1, |_| {});
    let order = |name: &str| d.system(name).unwrap().order;
    let first = &d.schedule.stages[d.system("movement").unwrap().stage.unwrap()];
    assert_eq!(first.systems, [order("movement"), order("regenerate")]);
    assert_eq!((first.first, first.len), (order("movement"), 2));
    let second = &d.schedule.stages[d.system("stop").unwrap().stage.unwrap()];
    assert_eq!(second.systems, [order("stop"), order("spawner")]);

    // Each system says what kept it out of the stage before its own.
    let blocked = |name: &str| d.system(name).unwrap().blocked.clone().unwrap();
    assert_eq!(blocked("movement").boundary, "phase");
    assert_eq!(blocked("regenerate").boundary, "phase", "nothing held it back");
    let stop = blocked("stop");
    assert_eq!(
        (stop.boundary.as_str(), stop.after),
        ("conflict", Some(order("movement")))
    );
    assert!(stop.conflicts[0].name.as_deref().unwrap().ends_with("::Vel"));
    let spawner = blocked("spawner");
    assert_eq!(
        (spawner.boundary.as_str(), spawner.after),
        ("conflict", Some(order("regenerate")))
    );
    assert!(spawner.conflicts[0].name.as_deref().unwrap().ends_with("::Health"));

    // The field names agents read.
    let json = serde_json::to_value(&d).unwrap();
    let system = &json["systems"][order("spawner")];
    assert_eq!(system["blocked"]["boundary"], "conflict");
    assert!(system["blocked"]["after"].is_u64());
    assert!(json["schedule"]["stages"][0]["systems"].is_array());
}

#[test]
fn ordered_systems_start_a_stage_after_the_one_they_follow() {
    let mut game = game();
    let first = game.add_system(Phase::AfterPhysics, movement.access(SystemAccess::new().write::<Pos>()));
    let second = game.add_system(
        Phase::AfterPhysics,
        regenerate.access(SystemAccess::new().write::<Health>()),
    );
    let game = game.order(first, second);
    let d = diagnose(game, 1, |_| {});
    let stage = &d.schedule.stages[d.system("regenerate").unwrap().stage.unwrap()];
    assert_eq!(stage.boundary, "order");
    assert_eq!(stage.after, Some(d.system("movement").unwrap().order));
}

#[test]
fn view_systems_that_must_run_alone_say_why() {
    let game = game()
        .add(
            Phase::AfterPhysics,
            sounds.access(SystemAccess::new().write_service(Service::Audio)),
        )
        .add(
            Phase::AfterPhysics,
            echo.access(SystemAccess::new().send_events::<Hit>().read_events::<Hit>()),
        );
    let d = diagnose(game, 1, |_| {});
    let why = |name: &str| d.system(name).unwrap().alone.clone().unwrap();
    assert_eq!(
        (why("sounds").reason.as_str(), why("sounds").item.as_deref()),
        ("service", Some("audio"))
    );
    assert_eq!(why("echo").reason, "sends_and_reads");
    assert!(why("echo").item.unwrap().ends_with("::Hit"));
    for name in ["sounds", "echo"] {
        assert_eq!(d.system(name).unwrap().access, "view");
    }
}

#[test]
fn dependencies_list_the_declared_pairs_that_must_stay_in_order() {
    let game = game()
        .add(
            Phase::AfterPhysics,
            movement.access(SystemAccess::new().write::<Pos>().read::<Vel>()),
        )
        .add(
            Phase::AfterPhysics,
            regenerate.access(SystemAccess::new().write::<Health>()),
        )
        .add(Phase::AfterPhysics, stop.access(SystemAccess::new().write::<Vel>()))
        // Another phase: never a pair with the systems above.
        .add(Phase::BeforePhysics, spawner.access(SystemAccess::new().write::<Vel>()));
    let d = diagnose(game, 1, |_| {});
    let order = |name: &str| d.system(name).unwrap().order;
    let pairs: Vec<(usize, usize)> = d.schedule.dependencies.iter().map(|p| (p.first, p.second)).collect();
    assert_eq!(
        pairs,
        [(order("movement"), order("stop"))],
        "{:#?}",
        d.schedule.dependencies
    );
    let conflicts = &d.schedule.dependencies[0].conflicts;
    assert_eq!(conflicts[0].kind, "component");
    assert!(conflicts[0].name.as_deref().unwrap().ends_with("::Vel"));
}

#[test]
fn the_schedule_reports_whether_batches_run_and_how_they_were_dispatched() {
    let systems = |game: Game| {
        game.add(Phase::AfterPhysics, movement.access(SystemAccess::new().write::<Pos>()))
            .add(
                Phase::AfterPhysics,
                regenerate.access(SystemAccess::new().write::<Health>()),
            )
    };
    // Off: the stages are listed, but nothing is dispatched as a batch.
    let off = diagnose(systems(game().parallel_systems(false)), 5, |_| {});
    assert!(!off.schedule.parallel && !off.schedule.threads);
    let batch = &off.schedule.stages[off.system("movement").unwrap().stage.unwrap()];
    assert!(batch.batch);
    assert_eq!((batch.threaded_runs, batch.inline_runs), (0, 0));

    // On with a threshold nothing reaches: every run stays on the game's thread.
    let inline = systems(game().parallel_systems(true).parallel_threshold(Duration::MAX));
    let inline = diagnose(inline, 5, |_| {});
    assert!(inline.schedule.parallel);
    assert!(inline.schedule.parallel_threshold_us > 1e9);
    let batch = &inline.schedule.stages[inline.system("movement").unwrap().stage.unwrap()];
    assert_eq!((batch.threaded_runs, batch.inline_runs), (0, 5));

    // On with no threshold: every run goes to other threads, if there are any.
    let threads = systems(game().parallel_systems(true).parallel_threshold(Duration::ZERO));
    let threads = diagnose(threads, 5, |_| {});
    assert_eq!(threads.schedule.parallel_threshold_us, 0.0);
    let batch = &threads.schedule.stages[threads.system("movement").unwrap().stage.unwrap()];
    let expected = if threads.schedule.threads { (5, 0) } else { (0, 5) };
    assert_eq!((batch.threaded_runs, batch.inline_runs), expected);
    // Single systems are never counted.
    let alone = threads.schedule.stages.last().unwrap();
    assert_eq!((alone.len, alone.threaded_runs, alone.inline_runs), (1, 0, 0));
}

#[test]
fn the_query_index_reports_its_policy_and_counters() {
    let d = diagnose(Game::new("t", 1, 1), 1, |ctx| {
        ctx.world.spawn((Pos([0.0; 2]),));
    });
    assert!(d.query_cache.enabled);
    assert_eq!(
        d.query_cache.min_archetypes,
        ponko::ecs_core::QueryCachePolicy::new().min_archetypes
    );
    assert_eq!(d.query_cache.hits, 0, "a small world scans");

    let off = diagnose(Game::new("t", 1, 1), 1, |ctx| {
        ctx.world
            .set_query_cache_policy(ponko::ecs_core::QueryCachePolicy::off());
    });
    assert!(!off.query_cache.enabled);
}
