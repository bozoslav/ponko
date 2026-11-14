//! Save migrations for a game's own components: `SaveMigrations`, the game's
//! save version written into saves, the steps that bring older saves up to
//! date (each kind of change), chains across versions, saves newer than the
//! game, and `Ctx::check_old_saves` over `tests/fixtures/game-saves`.

use ponko::prelude::*;
use ponko::reflect::{Migration, ReflectError, SaveMigrations};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const OLD_SAVES: &str = "tests/fixtures/game-saves";

// ANCHOR: migrations
// The game's components as they are now, at save version 3.
#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Health {
    now: f32,
    max: f32, // hearts; version 1 saved hit points, 4 per heart
    shield: f32,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Regen {
    per_second: f32,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Movement {
    speed: f32,
    jump: f32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Purse {
    coins: u32,
}

/// What changed in each version, so saves of every earlier one still load.
fn save_migrations() -> SaveMigrations {
    SaveMigrations::new(3)
        // 0 -> 1 (saves from before the game had versions).
        .step(
            0,
            Migration::new()
                .rename_component("Hp", "Health")
                .split_component("Stats", &[("Regen", &["hp_regen"]), ("Speed", &["speed"])])
                .rename_field("Regen", "hp_regen", "per_second")
                .remove_component("Legacy")
                .rename_resource("Gold", "Purse"),
        )
        // 1 -> 2.
        .step(
            1,
            Migration::new()
                .convert_field("Health", "max", |points| {
                    let points = points.as_f64().ok_or("expected a number")?;
                    Ok(json!((points / 4.0).ceil()))
                })
                .add_field("Health", "shield", json!(0.0))
                .merge_components(&["Speed", "Jump"], "Movement")
                .convert_resource("Purse", |coins| Ok(json!({ "coins": coins }))),
        )
        // 2 -> 3.
        .step(
            2,
            Migration::new()
                .rename_field("Health", "current", "now")
                .remove_field("Movement", "drag"),
        )
}

fn setup(ctx: &mut Ctx) {
    ctx.world.reflect::<Health>();
    ctx.world.reflect::<Regen>();
    ctx.world.reflect::<Movement>();
    ctx.world.reflect_resource::<Purse>();
    ctx.world.insert_resource(save_migrations()); // before the first load
}
// ANCHOR_END: migrations

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
struct Follows(Entity);

fn headless(test: impl FnOnce(&mut Ctx) + 'static) {
    Game::new("migrations", 64, 64).run_headless(0, move |ctx| {
        setup(ctx);
        ctx.world.reflect::<Follows>();
        test(ctx);
    });
}

fn save_of(version: Option<u32>, components: Value) -> Value {
    let mut save = json!({
        "format": "ponko-world",
        "version": 1,
        "entities": [{ "entity": 0, "components": components }],
    });
    if let Some(v) = version {
        save["game_version"] = v.into();
    }
    save
}

/// `save` migrated by `m`, owned.
fn run(m: &SaveMigrations, save: Value) -> Result<Value, ReflectError> {
    m.migrate(&save).map(std::borrow::Cow::into_owned)
}

/// The world every fixture must come out as.
fn assert_current_world(ctx: &mut Ctx) {
    let player = Entity::from_bits(0);
    let w = &ctx.world;
    assert_eq!(
        w.get::<Health>(player),
        Some(&Health {
            now: 12.0,
            max: 10.0,
            shield: 0.0
        })
    );
    assert_eq!(w.get::<Regen>(player), Some(&Regen { per_second: 0.5 }));
    assert_eq!(w.get::<Movement>(player), Some(&Movement { speed: 3.0, jump: 1.5 }));
    assert_eq!(
        w.get::<ponko::d2::Transform>(player).unwrap().position,
        vec2(10.0, 20.0)
    );
    assert!(
        !w.is_alive(Entity::from_bits(1)),
        "an entity of only a removed component is gone"
    );
    let follower = Entity::from_bits(2);
    assert_eq!(w.get::<Follows>(follower), Some(&Follows(player)));
    assert_eq!(w.get::<Health>(follower).unwrap().max, 2.0);
    assert_eq!(w.resource::<Purse>(), &Purse { coins: 7 });
}

#[test]
fn a_component_rename_plus_a_field_conversion_loads_from_a_save_written_before_both() {
    headless(|ctx| {
        let old = save_of(None, json!({ "Hp": { "current": 3.0, "max": 13.0 } }));
        ctx.world.load(&old).unwrap();
        let hp = ctx.world.get::<Health>(Entity::from_bits(0)).unwrap();
        assert_eq!((hp.now, hp.max, hp.shield), (3.0, 4.0, 0.0));
    });
}

#[test]
fn every_old_save_in_the_fixture_folder_migrates_to_the_same_world() {
    headless(|ctx| {
        for file in ["v0-before-versions.json", "v1.json", "v2.json"] {
            let save: Value =
                serde_json::from_str(&std::fs::read_to_string(format!("{OLD_SAVES}/{file}")).unwrap()).unwrap();
            ctx.world.load(&save).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_current_world(ctx);
        }
        assert_eq!(ctx.check_old_saves(OLD_SAVES).unwrap(), 3);
    });
}

#[test]
fn saves_record_the_game_version_and_load_unchanged_at_that_version() {
    headless(|ctx| {
        let e = ctx.world.spawn((Health {
            now: 1.0,
            max: 2.0,
            shield: 3.0,
        },));
        let save = ctx.world.save();
        assert_eq!(save["game_version"], 3);
        let migrated = save_migrations().migrate(&save).unwrap();
        assert!(
            matches!(migrated, std::borrow::Cow::Borrowed(_)),
            "nothing to do at the current version"
        );
        ctx.world.load(&save).unwrap();
        assert_eq!(ctx.world.get::<Health>(e).unwrap().shield, 3.0);
    });
}

#[test]
fn a_world_without_migrations_writes_no_game_version_and_refuses_versioned_saves() {
    let mut world = World::new();
    world.reflect::<Regen>();
    world.spawn((Regen { per_second: 1.0 },));
    let save = world.save();
    assert!(save.get("game_version").is_none());
    let err = world
        .load(&save_of(Some(1), json!({ "Regen": { "per_second": 2.0 } })))
        .unwrap_err();
    assert_eq!(err, ReflectError::NewerSave { saved: 1, current: 0 });
}

#[test]
fn a_save_from_a_newer_game_is_refused_with_both_versions_and_the_world_is_unchanged() {
    headless(|ctx| {
        let e = ctx.world.spawn((Regen { per_second: 1.0 },));
        let newer = save_of(Some(4), json!({ "Regen": { "per_second": 9.0 } }));
        let err = ctx.world.load(&newer).unwrap_err();
        assert_eq!(err, ReflectError::NewerSave { saved: 4, current: 3 });
        assert!(err.to_string().contains("newer version of the game"), "{err}");
        assert!(
            err.to_string()
                .contains("game save version 4; this build reads up to 3"),
            "{err}"
        );
        assert_eq!(ctx.world.get::<Regen>(e), Some(&Regen { per_second: 1.0 }));
        // The same through the game's own loads.
        let error = Error::from(err);
        assert_eq!(error.kind(), ErrorKind::Unsupported);
    });
}

#[test]
fn a_failing_step_names_its_versions_and_entity_and_leaves_the_world_unchanged() {
    headless(|ctx| {
        let e = ctx.world.spawn((Regen { per_second: 1.0 },));
        let bad = save_of(Some(1), json!({ "Health": { "current": 1.0, "max": "lots" } }));
        let err = ctx.world.load(&bad).unwrap_err();
        let ReflectError::Migration { from, ref message } = err else {
            panic!("expected a migration error, got {err:?}");
        };
        assert_eq!(from, 1);
        assert!(
            message.contains("entity 0") && message.contains("Health.max") && message.contains("expected a number"),
            "{message}"
        );
        assert!(
            err.to_string()
                .starts_with("migrating the save from game save version 1 to 2"),
            "{err}"
        );
        assert_eq!(ctx.world.get::<Regen>(e), Some(&Regen { per_second: 1.0 }));
    });
}

#[test]
fn chained_steps_run_from_the_saves_version_only() {
    // A version-2 save already has hearts: step 1's conversion must not run again.
    headless(|ctx| {
        let v2 = save_of(
            Some(2),
            json!({ "Health": { "current": 5.0, "max": 3.0, "shield": 1.0 } }),
        );
        ctx.world.load(&v2).unwrap();
        assert_eq!(
            ctx.world.get::<Health>(Entity::from_bits(0)),
            Some(&Health {
                now: 5.0,
                max: 3.0,
                shield: 1.0
            })
        );
    });
}

#[test]
fn renamed_and_removed_fields_and_added_fields_keep_saved_values() {
    let m = SaveMigrations::new(1).step(
        0,
        Migration::new()
            .rename_field("A", "old", "new")
            .remove_field("A", "gone")
            .add_field("A", "added", json!(5))
            .add_field("A", "kept", json!(0)),
    );
    let out = run(
        &m,
        save_of(
            None,
            json!({ "A": { "old": 1, "gone": 2, "kept": 7 }, "B": { "old": 3 } }),
        ),
    )
    .unwrap();
    assert_eq!(
        out["entities"][0]["components"],
        json!({ "A": { "new": 1, "added": 5, "kept": 7 }, "B": { "old": 3 } })
    );
    assert_eq!(out["game_version"], 1);
}

#[test]
fn a_retyped_field_or_component_is_converted() {
    let m = SaveMigrations::new(1).step(
        0,
        Migration::new()
            // A number of seconds became `{ "secs": n }`.
            .convert_field("Timer", "left", |v| Ok(json!({ "secs": v })))
            // A tuple struct `Team(u8)` became a named enum.
            .convert_component("Team", |v| Ok(json!(if v == 0 { "Red" } else { "Blue" }))),
    );
    let out = run(&m, save_of(None, json!({ "Timer": { "left": 3 }, "Team": 1 }))).unwrap();
    assert_eq!(
        out["entities"][0]["components"],
        json!({ "Timer": { "left": { "secs": 3 } }, "Team": "Blue" })
    );
}

#[test]
fn a_split_moves_the_listed_fields_and_a_merge_joins_them() {
    let m = SaveMigrations::new(2)
        .step(
            0,
            Migration::new().split_component("Stats", &[("Stats", &["hp"]), ("Speed", &["speed", "missing"])]),
        )
        .step(1, Migration::new().merge_components(&["Speed", "Extra"], "Stats"));
    let one = SaveMigrations::new(1).step(
        0,
        Migration::new().split_component("Stats", &[("Stats", &["hp"]), ("Speed", &["speed", "missing"])]),
    );
    let split = run(
        &one,
        save_of(None, json!({ "Stats": { "hp": 1, "speed": 2, "dropped": 3 } })),
    )
    .unwrap();
    assert_eq!(
        split["entities"][0]["components"],
        json!({ "Stats": { "hp": 1 }, "Speed": { "speed": 2 } })
    );
    let merged = run(
        &m,
        save_of(
            None,
            json!({ "Stats": { "hp": 1, "speed": 2 }, "Extra": { "luck": 4 } }),
        ),
    )
    .unwrap();
    assert_eq!(
        merged["entities"][0]["components"],
        json!({ "Stats": { "hp": 1, "speed": 2, "luck": 4 } })
    );
}

#[test]
fn a_merge_of_two_components_with_the_same_field_fails() {
    let m = SaveMigrations::new(1).step(0, Migration::new().merge_components(&["A", "B"], "C"));
    let err = run(&m, save_of(None, json!({ "A": { "x": 1 }, "B": { "x": 2 } }))).unwrap_err();
    assert!(
        matches!(&err, ReflectError::Migration { from: 0, message } if message.contains("field `x`")),
        "{err}"
    );
}

#[test]
fn a_rename_onto_a_component_the_entity_has_fails() {
    let m = SaveMigrations::new(1).step(0, Migration::new().rename_component("A", "B"));
    let err = run(&m, save_of(None, json!({ "A": {}, "B": {} }))).unwrap_err();
    assert!(err.to_string().contains("both `A` and `B`"), "{err}");
}

#[test]
fn removed_components_and_resources_are_dropped_with_emptied_entities() {
    let m = SaveMigrations::new(1).step(0, Migration::new().remove_component("Old").remove_resource("Gone"));
    let mut save = save_of(None, json!({ "Old": {} }));
    save["entities"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "entity": 1, "components": { "Old": {}, "Kept": {} } }));
    save["resources"] = json!({ "Gone": 1, "Stays": 2 });
    let out = m.migrate(&save).unwrap();
    assert_eq!(out["entities"], json!([{ "entity": 1, "components": { "Kept": {} } }]));
    assert_eq!(out["resources"], json!({ "Stays": 2 }));
}

#[test]
fn entity_and_whole_save_changes_cover_the_rest() {
    let m = SaveMigrations::new(1).step(
        0,
        Migration::new()
            // A flag moved to a component of its own.
            .entities(|c| {
                if let Some(true) = c
                    .get_mut("Enemy")
                    .and_then(|e| e.as_object_mut()?.remove("boss"))
                    .and_then(|b| b.as_bool())
                {
                    c.insert("Boss".into(), json!({}));
                }
                Ok(())
            })
            // Older saves lack the weather entity.
            .save(|save| {
                save["entities"]
                    .as_array_mut()
                    .ok_or("no entities")?
                    .push(json!({ "entity": 9, "components": { "Weather": {} } }));
                Ok(())
            }),
    );
    let out = run(&m, save_of(None, json!({ "Enemy": { "boss": true } }))).unwrap();
    assert_eq!(out["entities"][0]["components"], json!({ "Enemy": {}, "Boss": {} }));
    assert_eq!(out["entities"][1]["entity"], 9);
}

#[test]
fn several_steps_from_one_version_run_in_order() {
    let mut m = SaveMigrations::new(1);
    m.add_step(0, Migration::new().rename_component("A", "B"));
    m.add_step(0, Migration::new().rename_component("B", "C"));
    let out = run(&m, save_of(None, json!({ "A": {} }))).unwrap();
    assert_eq!(out["entities"][0]["components"], json!({ "C": {} }));
}

#[test]
fn a_bad_game_version_is_refused() {
    let mut save = save_of(None, json!({ "A": {} }));
    save["game_version"] = json!("two");
    let err = SaveMigrations::new(1).migrate(&save).unwrap_err();
    assert!(err.to_string().contains("not a version number"), "{err}");
}

#[test]
#[should_panic(expected = "a step from version 3 can't run in a game whose save version is 3")]
fn a_step_from_the_current_version_panics() {
    let _ = SaveMigrations::new(3).step(3, Migration::new());
}

#[test]
#[should_panic(expected = "a step from version 1 is registered")]
fn lowering_the_version_below_a_step_panics() {
    let mut m = SaveMigrations::new(3).step(1, Migration::new());
    m.set_version(1);
}

#[test]
fn save_slots_written_by_an_older_version_load_through_load_world() {
    let dir = std::env::temp_dir().join(format!("ponko-migrations-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let slot = dir.join("saves");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::copy(format!("{OLD_SAVES}/v0-before-versions.json"), slot.join("old.json")).unwrap();
    Game::new("migrations", 64, 64).data_dir(&dir).run_headless(0, |ctx| {
        setup(ctx);
        ctx.world.reflect::<Follows>();
        ctx.load_world("old").unwrap();
        assert_current_world(ctx);
    });
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn check_old_saves_names_every_file_that_fails_and_refuses_an_empty_folder() {
    let dir = std::env::temp_dir().join(format!("ponko-old-saves-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.clone();
    let check = dir.clone();
    headless(move |ctx| {
        assert_eq!(ctx.check_old_saves(&empty).unwrap_err().kind(), ErrorKind::NotFound);
        std::fs::copy(format!("{OLD_SAVES}/v1.json"), check.join("a-good.json")).unwrap();
        std::fs::write(
            check.join("b-newer.json"),
            save_of(Some(7), json!({ "Regen": { "per_second": 1 } })).to_string(),
        )
        .unwrap();
        std::fs::write(
            check.join("c-unknown.json"),
            save_of(Some(3), json!({ "Nope": {} })).to_string(),
        )
        .unwrap();
        std::fs::write(check.join("notes.txt"), "not a save").unwrap();
        let err = ctx.check_old_saves(&check).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("2 of 3 old saves"), "{text}");
        assert!(
            text.contains("b-newer.json: the save is from a newer version"),
            "{text}"
        );
        assert!(
            text.contains("c-unknown.json: no reflected component named `Nope`"),
            "{text}"
        );
        assert!(!text.contains("a-good.json"), "{text}");
    });
    let _ = std::fs::remove_dir_all(dir);
}
