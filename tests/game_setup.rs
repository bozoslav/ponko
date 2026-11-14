use ponko::prelude::{Component, Game};

#[derive(Component)]
struct Marker;

#[test]
fn builder_setups_run_once_in_order_and_apply_commands_between_callbacks() {
    let world = Game::new("setup", 1, 1)
        .sim_system(|ctx| *ctx.world.resource_mut::<usize>() += 1)
        .setup(|ctx| {
            ctx.world.insert_resource(0usize);
            ctx.commands.spawn((Marker,));
        })
        .setup(|ctx| {
            assert_eq!(ctx.world.query::<&Marker>().count(), 1);
            assert_eq!(*ctx.world.resource::<usize>(), 0);
            *ctx.world.resource_mut::<usize>() = 10;
        })
        .run_headless(3);
    assert_eq!(*world.resource::<usize>(), 13);
}

#[test]
fn runtime_branches_can_choose_different_capturing_setup_closures() {
    for mode in [true, false] {
        let game = Game::new("setup", 1, 1);
        let game = if mode {
            let text = String::from("server");
            game.setup(move |ctx| ctx.world.insert_resource(text))
        } else {
            let number = 7usize;
            game.setup(move |ctx| ctx.world.insert_resource(number.to_string()))
        };
        let world = game.run_headless(0);
        assert_eq!(world.resource::<String>(), if mode { "server" } else { "7" });
    }
}

#[test]
fn stored_setup_is_used_by_headless_profiling() {
    let (world, _) = Game::new("profile", 1, 1)
        .setup(|ctx| ctx.world.insert_resource(42usize))
        .profile_headless(0, 1);
    assert_eq!(*world.resource::<usize>(), 42);
}

#[test]
fn a_game_of_plugins_and_systems_runs_without_a_setup_closure() {
    struct Ticks(u32);
    struct Counting;
    impl ponko::prelude::Plugin for Counting {
        fn build(&self, game: Game) -> Game {
            game.sim_system(|ctx| ctx.world.resource_mut::<Ticks>().0 += 1)
        }
        fn setup(&self, ctx: &mut ponko::prelude::Ctx) {
            ctx.world.insert_resource(Ticks(0));
        }
    }
    let world = Game::new("plugins", 1, 1)
        .plugin(Counting)
        .sim_system(|ctx| ctx.world.resource_mut::<Ticks>().0 += 10)
        .configured()
        .run_headless(2);
    assert_eq!(world.resource::<Ticks>().0, 22);
    // A plain game with nothing to set up runs too, and setups can follow.
    let world = Game::new("empty", 1, 1)
        .configured()
        .setup(|ctx| ctx.world.insert_resource(Ticks(5)))
        .run_headless(1);
    assert_eq!(world.resource::<Ticks>().0, 5);
}

/// A game as `ponko new` makes it: `game()` returns a configured game with a
/// frame system that reads keys, a menu, and a save in `ctx.storage`.
mod template {
    use ponko::prelude::*;

    #[derive(Default)]
    pub struct Steps(pub i32);

    pub fn game() -> ConfiguredGame {
        Game::new("Template", 640, 480)
            .system(|ctx| {
                if ctx.key_pressed(Key::Right) {
                    ctx.world.resource_mut::<Steps>().0 += 1;
                }
            })
            .setup(|ctx| {
                ctx.world.insert_resource(Steps::default());
                let root = ctx.ui.root().layout(ponko::ui::UiLayout::fill()).id();
                ctx.ui.label(root, "").name("menu.saved");
                ctx.ui.show_hud(root);
                let saved = ctx.storage.read_json::<i32>("steps.json").ok();
                let text = saved.map_or("new".to_string(), |s| s.to_string());
                let label = ctx.ui.find("menu.saved").unwrap();
                ctx.ui.set_text(label, text);
            })
    }
}

#[test]
fn a_configured_game_takes_headless_frames_a_scenario_and_a_data_folder() {
    use ponko::actions::InputEvent;
    use ponko::prelude::Key;
    use ponko::scenario::{InputScenario, ScenarioCheck};

    let dir = std::env::temp_dir().join(format!("ponko-configured-game-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    ponko::storage::Storage::in_folder(&dir)
        .write_json("steps.json", &7)
        .unwrap();

    let scenario = InputScenario::new()
        .at(2, InputEvent::Key(Key::Right, true))
        .at(3, InputEvent::Key(Key::Right, false))
        .expect(1, ScenarioCheck::ui("menu.saved").text("7"))
        .duration(5);
    // Frame systems only run with headless frames.
    let (world, report) = template::game()
        .headless_frames(true)
        .data_dir(&dir)
        .seed(3)
        .run_headless_scenario(scenario.clone());
    report.assert_passed();
    assert_eq!(world.resource::<template::Steps>().0, 1);

    // The same, installed with `input_scenario`, and with test systems added.
    let world = template::game()
        .headless_frames(true)
        .data_dir(&dir)
        .input_scenario(scenario)
        .system(|ctx| ctx.world.resource_mut::<template::Steps>().0 += 10)
        .sim_system(|ctx| ctx.world.resource_mut::<template::Steps>().0 += 100)
        .run_headless(5);
    assert_eq!(world.resource::<template::Steps>().0, 1 + 5 * 10 + 5 * 100);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_configured_game_takes_a_seed() {
    let roll = |seed| {
        Game::new("seeded", 1, 1)
            .configured()
            .seed(seed)
            .setup(|ctx| {
                let n = ctx.random().stream(0).u64();
                ctx.world.insert_resource(n);
            })
            .run_headless(0)
            .resource::<u64>()
            .to_owned()
    };
    assert_eq!(roll(1), roll(1));
    assert_ne!(roll(1), roll(2));
}
