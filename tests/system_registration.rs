//! `Game::add` and `IntoSystem`: one entry point for `Ctx` systems, view
//! systems and their options (name, interval, declaration). Headless.

use ponko::access::{Service, SystemAccess};
use ponko::prelude::*;
use ponko::system_view::SystemView;

#[derive(Default)]
struct Log(Vec<String>);
#[derive(Component)]
struct Pos;

fn setup(ctx: &mut Ctx) {
    ctx.world.insert_resource(Log::default());
    ctx.world.spawn((Pos,));
}

fn note(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    ctx.world.resource_mut::<Log>().0.push(format!("ctx {tick}"));
}

fn note_view(sys: &mut SystemView) {
    let tick = sys.time.sim_tick();
    sys.world.resource_mut::<Log>().0.push(format!("view {tick}"));
}

fn view_access() -> SystemAccess {
    SystemAccess::new().read_service(Service::Time).write_resource::<Log>()
}

#[test]
fn functions_and_closures_of_both_kinds_go_through_add() {
    let world = Game::new("add", 1, 1)
        .add(Phase::BeforePhysics, note)
        .add(Phase::AfterPhysics, note_view.access(view_access()))
        .add(Phase::SimLast, |ctx: &mut Ctx| {
            ctx.world.resource_mut::<Log>().0.push("closure".into());
        })
        .add(
            Phase::SimLast,
            (|sys: &mut SystemView| sys.world.resource_mut::<Log>().0.push("view closure".into()))
                .access(SystemAccess::new().write_resource::<Log>()),
        )
        .run_headless(1, setup);
    assert_eq!(
        world.resource::<Log>().0,
        ["ctx 0", "view 0", "closure", "view closure"]
    );
}

#[test]
fn the_shortcuts_are_add_in_their_phase() {
    fn sim(_: &mut Ctx) {}
    fn frame(_: &mut Ctx) {}
    // The shortcuts take `Ctx` closures without a parameter type.
    let game = Game::new("shortcuts", 1, 1)
        .sim_system(sim)
        .system(frame)
        .sim_system(|ctx| ctx.world.clear());
    let report = game.access_report();
    let phase = |name: &str| report.systems.iter().find(|s| s.name.ends_with(name)).unwrap().phase;
    assert_eq!(phase("::sim"), Phase::AfterPhysics);
    assert_eq!(phase("::frame"), Phase::Frame);
}

#[test]
fn options_apply_to_ctx_and_view_systems_alike() {
    let game = Game::new("options", 1, 1)
        .add(Phase::AfterPhysics, note.named("slow ctx").every(3))
        .add(
            Phase::AfterPhysics,
            note_view.named("slow view").every(3).access(view_access()),
        );
    let report = game.access_report();
    let listed = |name: &str| report.systems.iter().find(|s| s.name == name).unwrap();
    assert!(!listed("slow ctx").enforced);
    assert!(listed("slow ctx").access.is_none());
    assert!(listed("slow view").enforced);
    assert_eq!(listed("slow view").access, Some(view_access()));
    // Two systems of one interval are spread over its ticks.
    let world = game.run_headless(6, setup);
    assert_eq!(world.resource::<Log>().0, ["ctx 0", "view 1", "ctx 3", "view 4"]);
}

#[test]
fn the_last_option_set_wins() {
    let game = Game::new("options", 1, 1).add(
        Phase::AfterPhysics,
        note.named("first")
            .every(2)
            .access(SystemAccess::new().write::<Pos>())
            .named("second")
            .every(1)
            .access(SystemAccess::new().write_resource::<Log>()),
    );
    let report = game.access_report();
    assert_eq!(report.systems.iter().filter(|s| s.name == "second").count(), 1);
    let listed = report.systems.iter().find(|s| s.name == "second").unwrap();
    assert_eq!(listed.access, Some(SystemAccess::new().write_resource::<Log>()));
    let world = game.run_headless(2, setup);
    assert_eq!(world.resource::<Log>().0, ["ctx 0", "ctx 1"]);
}

#[test]
fn add_system_returns_the_id_of_either_kind() {
    let mut game = Game::new("ids", 1, 1);
    let ctx_id = game.add_system(Phase::AfterPhysics, note);
    let view_id = game.add_system(Phase::AfterPhysics, note_view.access(view_access()));
    assert_ne!(ctx_id, view_id);
    let report = game.access_report();
    let ids: Vec<SystemId> = (report.systems.iter())
        .filter(|s| s.name.ends_with("::note") || s.name.ends_with("::note_view"))
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, [ctx_id, view_id], "the report names registrations by id");
    let world = game.remove(ctx_id).run_headless(1, setup);
    assert_eq!(world.resource::<Log>().0, ["view 0"]);
}

#[test]
#[should_panic(expected = "doesn't declare that it may write resource")]
fn a_view_system_without_a_declaration_reaches_nothing() {
    let _ = Game::new("bare", 1, 1)
        .add(Phase::AfterPhysics, |sys: &mut SystemView| {
            sys.world.resource_mut::<Log>().0.clear();
        })
        .run_headless(1, setup);
}

#[test]
#[should_panic(expected = "system `hud` has an interval, and only fixed-tick phases take one")]
fn an_interval_in_a_frame_phase_panics() {
    let _ = Game::new("frame", 1, 1).add(Phase::Frame, note.named("hud").every(2));
}

#[test]
#[should_panic(expected = "view system `aim` declares service `gfx`, which a view doesn't offer")]
fn a_view_declaration_a_view_cannot_have_names_the_system() {
    let _ = Game::new("views", 1, 1).add(
        Phase::AfterPhysics,
        note_view
            .named("aim")
            .access(SystemAccess::new().read_service(Service::Gfx)),
    );
}

#[test]
fn a_wrong_order_is_a_typed_error() {
    let mut game = Game::new("order", 1, 1);
    let first = game.add_system(Phase::AfterPhysics, note.named("first"));
    let second = game.add_system(Phase::AfterPhysics, note.named("second"));
    assert_eq!(game.check_order(), Ok(()));
    let game = game.order(first, second);
    assert_eq!(game.check_order(), Ok(()));
    let game = game.order(second, first);
    let error = game.check_order().unwrap_err();
    assert_eq!((error.first, error.second), ("second", "first"));
    assert!(error.to_string().contains("`first` already runs before `second`"));
}

#[test]
fn built_in_systems_keep_their_names_when_added_from_systems() {
    // `systems` is the one public path of these functions; the names they
    // are listed under (plans, diagnostics, `remove_system` in C) stay the
    // ones from before.
    let game = Game::new("names", 1, 1)
        .remove(Systems::FOLLOW_ATTACHMENTS)
        .remove(Systems::REBUILD_SPATIAL_GRID)
        .add(Phase::SimLast, ponko::systems::follow_attachments)
        .add(Phase::SimLast, ponko::systems::rebuild_spatial_grid);
    let names = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = names.clone();
    game.add(Phase::SimLast, move |ctx: &mut Ctx| {
        *seen.lock().unwrap() = ctx
            .ecs_diagnostics()
            .systems
            .into_iter()
            .map(|s| s.name)
            .collect::<Vec<_>>();
    })
    .run_headless(1, |_| {});
    let names = names.lock().unwrap();
    for name in [
        "ponko::sheet::animate_sprites",
        "ponko::tilemap::draw_tilemaps",
        "ponko::attach::follow_attachments",
        "ponko::spatial::rebuild_spatial_grid",
    ] {
        assert!(names.iter().any(|n| n == name), "{name} missing from {names:?}");
    }
    for moved in [
        "ponko::systems::follow_attachments",
        "ponko::systems::rebuild_spatial_grid",
    ] {
        assert!(!names.iter().any(|n| n == moved), "{names:?}");
    }
}
