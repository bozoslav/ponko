//! Performance reports: scopes, per-system timings, budgets and comparing runs.

#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
#[allow(unused_imports)]
use ponko::perf::{Budget, PerfReport};
use ponko::prelude::*;

// ANCHOR: profile_scopes
/// Time the parts of your own code you suspect.
fn steering(ctx: &mut Ctx) {
    profile_scope!("steering");
    {
        profile_scope!("steering/neighbors");
        std::hint::black_box((0..2000).map(|i| (i as f32).sqrt()).sum::<f32>());
    }
    {
        let _timing = ctx.perf.scope("steering/avoid");
        ctx.world.query::<&mut Transform2D>().for_each(|t| t.position.x += 1.0);
    }
}
// ANCHOR_END: profile_scopes

// ANCHOR: profile_headless
/// Run it headless for 200 ticks (after 20 warm-up ticks), then read the
/// report and fail the test if it's too slow. Keep time limits generous:
/// machines differ.
#[test]
fn steering_stays_within_budget() {
    let (_world, report) = Game::new("bench", 1, 1)
        .sim_system(steering)
        .profile_headless(20, 200, |ctx| {
            for _ in 0..100 {
                ctx.world.spawn((Transform2D::default(),));
            }
        });
    println!("{report}");
    report.assert_within(
        &Budget::new()
            .frame_p99_ms(50.0)
            .system("steering", 10.0)
            .scope("steering/neighbors", 10.0),
    );
}
// ANCHOR_END: profile_headless

/// Scopes are shared by the whole process, so tests that count calls use a
/// system no other test runs.
fn nested(ctx: &mut Ctx) {
    profile_scope!("nested");
    {
        profile_scope!("nested/neighbors");
        std::hint::black_box((0..2000).map(|i| (i as f32).sqrt()).sum::<f32>());
    }
    ctx.world.query::<&mut Transform2D>().for_each(|t| t.position.x += 1.0);
}

// ANCHOR: profile_live
/// F5 starts timing your scopes and counting allocations; F6 prints where
/// the last half minute went.
fn perf_keys(ctx: &mut Ctx) {
    if ctx.input.key_pressed(Key::F5) {
        let on = !ctx.perf.profiling();
        ctx.perf.set_profiling(on);
    }
    if ctx.input.key_pressed(Key::F6) {
        println!("{}", ctx.perf.report());
    }
}
// ANCHOR_END: profile_live

#[test]
fn live_profiling_keys_run_headless() {
    Game::new("t", 1, 1).sim_system(perf_keys).run_headless(2, |_| {});
}

#[test]
fn a_headless_report_has_systems_scopes_and_the_worst_frames() {
    let (_world, report) = Game::new("t", 1, 1).sim_system(nested).profile_headless(5, 50, |ctx| {
        ctx.world.spawn((Transform2D::default(),));
    });
    assert_eq!((report.frames, report.entities), (50, 1));
    assert!(report.render.is_none(), "no drawing in a headless run");
    let system = report.system("nested").expect("the system is listed");
    assert!(system.avg_ms > 0.0);
    let outer = report.scope("nested").unwrap();
    let inner = report.scope("nested/neighbors").unwrap();
    assert_eq!(outer.calls_per_frame, 1.0);
    assert!(
        inner.avg_ms > 0.0 && inner.avg_ms <= outer.avg_ms,
        "scopes nest: {report}"
    );
    assert_eq!(report.worst_frames.len(), 3);
    assert!(report.worst_frames[0].ms >= report.frame_ms.p50);
    // Without a counting allocator the report says nothing about allocations.
    assert!(report.allocs.is_none());
    let tight = report.check(&Budget::new().system("nested", 0.0));
    assert_eq!(tight.len(), 1, "{tight:?}");
}

#[test]
fn reports_survive_json_and_show_regressions() {
    let (_w, before) = Game::new("t", 1, 1)
        .sim_system(steering)
        .profile_headless(2, 20, |_| {});
    let loaded = PerfReport::from_json(&before.to_json()).unwrap();
    assert_eq!(loaded, before);
    let mut after = before.clone();
    after.systems[0].avg_ms = before.systems[0].avg_ms * 3.0 + 1.0;
    let slower = after.regressions(&before, 0.2);
    assert!(slower.iter().any(|r| r.what.contains("steering")), "{slower:?}");
    assert!(before.regressions(&before, 0.2).is_empty());
}
