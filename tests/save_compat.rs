//! Saved-world compatibility: retained world saves written by earlier engine
//! revisions must keep loading with the same meaning.
//!
//! The fixtures live in `tests/fixtures/saves/`. Each one is a file written by
//! `World::save` at a recorded engine revision (see its `.provenance.json` and
//! the README there). These tests only read the checked-in bytes; they never
//! write a fixture. A change to the save format or to a built-in component's
//! saved fields that breaks old saves fails here.
//!
//! To add a fixture for a new save version, see `tests/fixtures/saves/README.md`.
//! Never regenerate an existing fixture to make a test pass.

#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
use ponko::prelude::*;
#[allow(unused_imports)]
use ponko::reflect::ReflectError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// The game-side types of the fixture. Their names and fields are part of the
// fixture: don't change them.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Component)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Component)]
struct Team(u8);

/// Refers to another entity: the reason saves keep entity ids.
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

/// Not reflected: counted under `"not_saved"`, never saved.
#[derive(Component)]
struct Secret;

/// A world that reflects everything the fixtures use.
fn reflected_world() -> World {
    let mut w = World::new();
    w.reflect::<Transform2D>();
    w.reflect::<ponko::attach::Attached>();
    w.reflect::<Health>();
    w.reflect::<Team>();
    w.reflect::<Follows>();
    w.reflect::<Label>();
    w.reflect::<Mood>();
    w.reflect_resource::<Score>();
    w.reflect_resource::<Round>();
    w
}

/// Reads a retained fixture's checked-in bytes.
fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/saves")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} is not JSON: {e}"))
}

fn v1() -> Value {
    fixture("world_v1.json")
}

/// The entity ids recorded in the v1 fixture (index | generation << 32).
fn id(index: u32, generation: u32) -> Entity {
    Entity::from_bits(u64::from(index) | u64::from(generation) << 32)
}

#[test]
fn the_v1_fixture_records_its_provenance() {
    let p = fixture("world_v1.provenance.json");
    let save = v1();
    assert_eq!(p["fixture"], "world_v1.json");
    assert_eq!(p["format"], save["format"], "provenance and fixture agree");
    assert_eq!(p["version"], save["version"], "provenance and fixture agree");
    let revision = p["engine_revision"].as_str().unwrap_or_default();
    assert!(
        revision.len() == 40 && revision.chars().all(|c| c.is_ascii_hexdigit()),
        "engine_revision is a full git commit id, not {revision:?}"
    );
    for key in ["engine_version", "toolchain", "generated_by", "date"] {
        assert!(
            p[key].as_str().is_some_and(|s| !s.is_empty()),
            "provenance records {key}"
        );
    }
}

#[test]
fn the_v1_fixture_has_the_v1_header() {
    let save = v1();
    assert_eq!(save["format"], "ponko-world");
    assert_eq!(save["version"], 1);
    assert_eq!(save["not_saved"], json!({ "Secret": 1 }));
}

#[test]
fn a_v1_save_loads_with_its_entities_ids_and_components() {
    let mut w = reflected_world();
    w.load(&v1()).expect("a version-1 save still loads");

    let (base, turret, scout, marker) = (id(4, 1), id(2, 3), id(1, 3), id(0, 3));
    assert_eq!(w.len(), 4, "every saved entity, and nothing else");

    // Built-in components keep their meaning.
    let t = w.get::<Transform2D>(base).unwrap();
    assert_eq!(t.position, vec2(120.5, -64.0));
    assert_eq!(t.rotation, 0.25);
    assert_eq!(t.scale, vec2(2.0, -1.0));
    let a = w.get::<ponko::attach::Attached>(turret).unwrap();
    assert_eq!(a.to, base, "attachments keep pointing at the saved entity");
    assert_eq!(a.offset, vec2(0.0, -8.0));
    assert_eq!(a.rotation, 1.5);
    assert_eq!(w.get::<Transform2D>(turret), Some(&Transform2D::default()));

    // Game components: structs, tuple structs, entity references, options, enums.
    assert_eq!(w.get::<Team>(base), Some(&Team(1)));
    assert_eq!(
        w.get::<Health>(base),
        Some(&Health {
            current: 37.5,
            max: 100.0
        })
    );
    assert_eq!(
        w.get::<Label>(base),
        Some(&Label {
            name: "Base \"north\" ✓".into(),
            note: Some("line one\nline two".into())
        })
    );
    assert_eq!(w.get::<Mood>(base), Some(&Mood::Alert { since: 42 }));
    assert_eq!(w.get::<Team>(turret), Some(&Team(1)));
    assert_eq!(w.get::<Follows>(scout), Some(&Follows(base)));
    assert_eq!(w.get::<Team>(scout), Some(&Team(2)));
    assert_eq!(w.get::<Mood>(scout), Some(&Mood::Calm));
    assert_eq!(
        w.get::<Label>(marker),
        Some(&Label {
            name: "marker".into(),
            note: None
        })
    );
    assert_eq!(w.get::<Transform2D>(marker).unwrap().position, vec2(-1.0, 0.5));

    // Ids that weren't saved, or saved ids with an older generation, are not alive.
    assert!(!w.is_alive(id(4, 0)), "an older generation of a saved slot stays dead");
    assert!(!w.is_alive(id(3, 0)), "an unsaved slot stays dead");

    // Reflected resources come back.
    assert_eq!(*w.resource::<Score>(), Score { red: 3, blue: 7 });
    assert_eq!(w.resource::<Round>().0, 12);

    // New entities don't take a loaded id.
    let new = w.spawn((Team(9),));
    assert!(![base, turret, scout, marker].contains(&new));
}

#[test]
fn unsupported_save_versions_fail_usefully_and_change_nothing() {
    let mut w = reflected_world();
    let keep = w.spawn((Team(4),));
    let before = w.save();

    for version in [json!(0), json!(2), json!("1"), Value::Null] {
        let mut save = v1();
        save["version"] = version.clone();
        let err = w.load(&save).expect_err("only version 1 is read");
        let message = err.to_string();
        assert!(
            message.contains("unsupported save version") && message.contains("reads 1"),
            "version {version}: {message}"
        );
        assert_eq!(w.save(), before, "version {version} changed the world");
    }

    let mut save = v1();
    save["format"] = json!("some-other-format");
    let message = w.load(&save).unwrap_err().to_string();
    assert!(message.contains("not a world save"), "{message}");
    assert_eq!(w.get::<Team>(keep), Some(&Team(4)));
}

#[test]
fn a_v1_save_names_what_this_world_does_not_reflect() {
    let mut w = World::new();
    w.reflect::<Transform2D>();
    let err = w.load(&v1()).unwrap_err();
    assert!(matches!(err, ReflectError::UnknownComponent(_)), "{err}");
    assert!(err.to_string().contains("reflect"), "{err}");
    assert_eq!(w.len(), 0, "a failed load changes nothing");
}

/// Writes the v1 fixture. Not a regression test: it's how
/// `tests/fixtures/saves/world_v1.json` was made. Run it only to create a
/// fixture for a new save version (copy it, change the version and file
/// name), never to overwrite a retained one:
///
/// ```sh
/// PONKO_SAVE_FIXTURE_OUT=/tmp/world_v1.json \
///     cargo test -p ponko --no-default-features --test save_compat -- --ignored --exact write_v1_fixture
/// ```
#[test]
#[ignore = "writes a fixture file; see tests/fixtures/saves/README.md"]
fn write_v1_fixture() {
    let out = std::env::var("PONKO_SAVE_FIXTURE_OUT").expect("set PONKO_SAVE_FIXTURE_OUT to the file to write");
    let out = std::path::Path::new(&out);
    assert!(
        !out.exists(),
        "{} exists; fixtures are never overwritten",
        out.display()
    );

    let mut w = reflected_world();
    // Churn the allocator so saved ids have gaps and generations.
    for round in 0..3 {
        let tmp: Vec<Entity> = (0..3 + round).map(|_| w.spawn((Team(0),))).collect();
        for e in tmp {
            w.despawn(e);
        }
    }
    let base = w.spawn((
        Team(1),
        Transform2D {
            position: vec2(120.5, -64.0),
            rotation: 0.25,
            scale: vec2(2.0, -1.0),
        },
        Health {
            current: 37.5,
            max: 100.0,
        },
        Label {
            name: "Base \"north\" ✓".into(),
            note: Some("line one\nline two".into()),
        },
        Mood::Alert { since: 42 },
    ));
    w.spawn((
        Team(1),
        Transform2D::default(),
        ponko::attach::Attached::to(base).offset(vec2(0.0, -8.0)).rotated(1.5),
    ));
    w.spawn((Team(2), Follows(base), Mood::Calm));
    w.spawn((
        Label {
            name: "marker".into(),
            note: None,
        },
        Transform2D::at(vec2(-1.0, 0.5)),
    ));
    w.spawn((Secret,));
    w.insert_resource(Score { red: 3, blue: 7 });
    w.insert_resource(Round(12));

    let text = serde_json::to_string_pretty(&w.save()).unwrap() + "\n";
    std::fs::write(out, text).unwrap();
}

#[test]
fn a_v1_save_resaves_with_the_same_identifier_and_entities() {
    let input = v1();
    let mut world = reflected_world();
    world.load(&input).unwrap();
    let output = world.save();
    assert_eq!(output["format"], "ponko-world");
    assert_eq!(output["version"], 1);
    assert_eq!(output["entities"], input["entities"]);
    let mut again = reflected_world();
    again.load(&output).unwrap();
    assert_eq!(again.save(), output);
}

#[test]
fn the_removed_pre_release_identifier_is_not_a_world_save() {
    let mut input = v1();
    input["format"] = json!("crabby-world");
    assert!(reflected_world().load(&input).is_err());
}
