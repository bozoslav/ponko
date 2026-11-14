//! Scene and prefab compatibility: files written by earlier engine revisions
//! must keep loading through the game-facing loaders (`Ctx::load_scene`,
//! `Ctx::spawn_prefab`) with the same meaning.
//!
//! A scene is the saved-world format, so the retained world fixture is loaded
//! here the way a game loads a level. A prefab has its own format
//! (`ponko-prefab`, version 1); `tests/fixtures/saves/prefab_v1.json` is one
//! written by `Assets::save_prefab` at a recorded revision (see
//! `prefab_v1.provenance.json` and the README next to it).
//!
//! These tests only read checked-in bytes; they never write a fixture.
//! Never regenerate a fixture to make a test pass.

mod common;

use common::TempAsset;
#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
use ponko::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// The game-side types of the fixtures. Their names and fields are part of the
// fixtures: don't change them.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Component)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Component)]
struct Team(u8);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Component)]
struct Follows(Entity);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Component)]
struct Label {
    name: String,
    note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Component)]
enum Mood {
    Calm,
    Alert { since: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Score {
    red: u32,
    blue: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Round(u32);

fn reflect_all(w: &mut World) {
    w.reflect::<Transform2D>();
    w.reflect::<ponko::attach::Attached>();
    w.reflect::<Health>();
    w.reflect::<Team>();
    w.reflect::<Follows>();
    w.reflect::<Label>();
    w.reflect::<Mood>();
    w.reflect_resource::<Score>();
    w.reflect_resource::<Round>();
}

fn fixture_text(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/saves")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()))
}

fn fixture(name: &str) -> Value {
    serde_json::from_str(&fixture_text(name)).unwrap_or_else(|e| panic!("{name} is not JSON: {e}"))
}

fn game() -> Game {
    Game::new("prefab compat", 1, 1)
}

fn id(index: u32, generation: u32) -> Entity {
    Entity::from_bits(u64::from(index) | u64::from(generation) << 32)
}

#[test]
fn the_prefab_fixture_records_its_provenance() {
    let p = fixture("prefab_v1.provenance.json");
    let prefab = fixture("prefab_v1.json");
    assert_eq!(p["fixture"], "prefab_v1.json");
    assert_eq!(p["format"], prefab["format"]);
    assert_eq!(p["version"], prefab["version"]);
    assert_eq!(prefab["format"], "ponko-prefab");
    assert_eq!(prefab["version"], 1);
    let revision = p["engine_revision"].as_str().unwrap_or_default();
    assert!(
        revision.len() == 40 && revision.chars().all(|c| c.is_ascii_hexdigit()),
        "engine_revision is a full git commit id, not {revision:?}"
    );
    for key in ["engine_version", "toolchain", "generated_by", "date", "baseline"] {
        assert!(
            p[key].as_str().is_some_and(|s| !s.is_empty()),
            "provenance records {key}"
        );
    }
}

#[test]
fn a_v1_prefab_spawns_where_asked_with_its_saved_components() {
    let file = TempAsset::with_contents("prefab-v1.json", &fixture_text("prefab_v1.json"));
    let mut spawned = Vec::new();
    let world = game().run_headless(0, |ctx| {
        reflect_all(&mut ctx.world);
        for at in [vec2(10.0, 20.0), vec2(-5.0, 7.5)] {
            spawned.push(
                ctx.spawn_prefab(file.name(), at)
                    .expect("a version-1 prefab still loads"),
            );
        }
    });
    assert_eq!(world.len(), 2);
    for (e, at) in spawned.iter().zip([vec2(10.0, 20.0), vec2(-5.0, 7.5)]) {
        // The position is the spawner's; rotation and scale are the prefab's.
        let t = world.get::<Transform2D>(*e).unwrap();
        assert_eq!(t.position, at);
        assert_eq!(t.rotation, 0.75);
        assert_eq!(t.scale, vec2(1.5, 1.5));
        assert_eq!(
            world.get::<Health>(*e),
            Some(&Health {
                current: 8.0,
                max: 12.0
            })
        );
        assert_eq!(world.get::<Team>(*e), Some(&Team(3)));
        assert_eq!(
            world.get::<Label>(*e),
            Some(&Label {
                name: "Palm \"tall\" ✓".into(),
                note: Some("two\nlines".into())
            })
        );
        assert_eq!(world.get::<Mood>(*e), Some(&Mood::Alert { since: 7 }));
    }
}

#[test]
fn unsupported_prefab_files_fail_usefully_and_spawn_nothing() {
    let mut cases: Vec<(Value, &str)> = Vec::new();
    // Version 2 (trees) is read too; 3 isn't.
    for version in [json!(0), json!(3), json!("1"), Value::Null] {
        let mut p = fixture("prefab_v1.json");
        p["version"] = version;
        cases.push((p, "unsupported prefab version"));
    }
    let mut other = fixture("prefab_v1.json");
    other["format"] = json!("some-other-format");
    cases.push((other, "not a prefab"));
    // The pre-release identifier is no longer read.
    let mut removed = fixture("prefab_v1.json");
    removed["format"] = json!("crabby-prefab");
    cases.push((removed, "not a prefab"));
    let mut empty = fixture("prefab_v1.json");
    empty["components"] = json!({});
    cases.push((empty, "non-empty"));
    // A version-2 file lists entities instead of one entity's components.
    let mut tree_without_entities = fixture("prefab_v1.json");
    tree_without_entities["version"] = json!(2);
    cases.push((tree_without_entities, "non-empty \"entities\" list"));

    for (i, (prefab, expected)) in cases.into_iter().enumerate() {
        let file = TempAsset::with_contents(&format!("prefab-bad-{i}.json"), &prefab.to_string());
        let world = game().run_headless(0, |ctx| {
            reflect_all(&mut ctx.world);
            let err = ctx.spawn_prefab(file.name(), vec2(0.0, 0.0)).unwrap_err();
            assert!(err.message().contains(expected), "case {i}: {err}");
            if expected == "unsupported prefab version" {
                assert!(err.message().contains("reads 1 and 2"), "case {i}: {err}");
            }
        });
        assert_eq!(world.len(), 0, "case {i} spawned something");
    }
}

#[test]
fn a_v1_prefab_names_what_the_game_does_not_reflect() {
    let file = TempAsset::with_contents("prefab-v1-unreflected.json", &fixture_text("prefab_v1.json"));
    let world = game().run_headless(0, |ctx| {
        ctx.world.reflect::<Transform2D>();
        let err = ctx.spawn_prefab(file.name(), vec2(0.0, 0.0)).unwrap_err();
        assert!(err.message().contains("reflect"), "{err}");
    });
    assert_eq!(world.len(), 0, "a failed spawn changes nothing");
}

#[test]
fn the_retained_world_save_loads_as_a_scene() {
    // A scene is the saved-world format; a game loads it with `load_scene`.
    let file = TempAsset::with_contents("scene-from-world-v1.json", &fixture_text("world_v1.json"));
    let world = game().run_headless(0, |ctx| {
        reflect_all(&mut ctx.world);
        ctx.world.spawn((Team(99),)); // replaced by the scene
        ctx.load_scene(file.name()).expect("a version-1 scene still loads");
    });
    let (base, turret, scout, marker) = (id(4, 1), id(2, 3), id(1, 3), id(0, 3));
    assert_eq!(world.len(), 4);
    assert_eq!(world.get::<Transform2D>(base).unwrap().position, vec2(120.5, -64.0));
    assert_eq!(world.get::<ponko::attach::Attached>(turret).unwrap().to, base);
    assert_eq!(world.get::<Follows>(scout), Some(&Follows(base)));
    assert_eq!(world.get::<Mood>(base), Some(&Mood::Alert { since: 42 }));
    assert_eq!(world.get::<Label>(marker).map(|l| l.name.as_str()), Some("marker"));
    assert_eq!(*world.resource::<Score>(), Score { red: 3, blue: 7 });
    assert_eq!(world.resource::<Round>().0, 12);
}

#[test]
fn an_unsupported_scene_version_fails_usefully_and_keeps_the_world() {
    let mut save = fixture("world_v1.json");
    save["version"] = json!(2);
    let file = TempAsset::with_contents("scene-v2.json", &save.to_string());
    let world = game().run_headless(0, |ctx| {
        reflect_all(&mut ctx.world);
        ctx.world.spawn((Team(4),));
        let err = ctx.load_scene(file.name()).unwrap_err();
        assert!(
            err.message().contains("unsupported save version") && err.message().contains("reads 1"),
            "{err}"
        );
    });
    assert_eq!(world.len(), 1, "a failed load keeps the world");
}

/// Writes the prefab fixture from `Assets::save_prefab`. Not a regression test:
/// it's how the retained file was made (see tests/fixtures/saves/README.md).
/// It refuses to overwrite a file.
///
/// ```sh
/// PONKO_PREFAB_FIXTURE_OUT=/tmp/prefab_v1.json \
///     cargo test -p ponko --no-default-features --test prefab_compat -- --ignored --exact write_prefab_v1_fixture
/// ```
#[test]
#[ignore = "writes a fixture file; see tests/fixtures/saves/README.md"]
fn write_prefab_v1_fixture() {
    let out = std::env::var("PONKO_PREFAB_FIXTURE_OUT").expect("set PONKO_PREFAB_FIXTURE_OUT to the file to write");
    let out = std::path::Path::new(&out);
    assert!(
        !out.exists(),
        "{} exists; fixtures are never overwritten",
        out.display()
    );
    let tmp = TempAsset::new("prefab-v1-generated.json");
    game().run_headless(0, |ctx| {
        reflect_all(&mut ctx.world);
        let model = ctx.world.spawn((
            Transform2D {
                position: vec2(300.0, 300.0),
                rotation: 0.75,
                scale: vec2(1.5, 1.5),
            },
            Health {
                current: 8.0,
                max: 12.0,
            },
            Team(3),
            Label {
                name: "Palm \"tall\" ✓".into(),
                note: Some("two\nlines".into()),
            },
            Mood::Alert { since: 7 },
        ));
        ctx.assets.save_prefab(&ctx.world, model, tmp.name()).unwrap();
    });
    std::fs::write(out, tmp.read()).unwrap();
}
