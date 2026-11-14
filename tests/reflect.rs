//! Reflection: components read and written by name, as JSON.

#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::{Decal, PointLight, SpotLight, Transform};
use ponko::prelude::*;
#[allow(unused_imports)]
use ponko::reflect::ReflectError;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{LodGroup, LodHysteresis, Model, ModelMaterial, ReceivesDecals};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize, Component)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Component)]
struct Team(u8);

/// Not reflected: must never show up in JSON.
#[derive(Component)]
struct Secret;

/// Shown in the inspector chapter; compiled, not run (it opens a window).
#[allow(dead_code)]
fn playtest_build() {
    // ANCHOR: inspector
    Game::new("Playtest", 1280, 720)
        .inspector(true) // also in release builds: the ` key opens it
        .run(|ctx| {
            ctx.world.reflect::<Health>(); // editable in the inspector
        });
    // ANCHOR_END: inspector
}

fn world() -> World {
    let mut w = World::new();
    w.reflect::<Transform2D>();
    w.reflect::<Health>();
    w.reflect::<Team>();
    w
}

#[test]
fn components_round_trip_through_json() {
    let mut w = world();
    let e = w.spawn((
        Health {
            current: 3.0,
            max: 10.0,
        },
        Team(2),
    ));
    assert_eq!(w.get_json(e, "Health").unwrap(), json!({ "current": 3.0, "max": 10.0 }));
    assert_eq!(w.get_json(e, "Team").unwrap(), json!(2));
    w.set_json(e, "Team", json!(7)).unwrap();
    assert_eq!(w.get::<Team>(e), Some(&Team(7)));
}

#[test]
fn set_json_on_an_object_changes_only_given_fields() {
    let mut w = world();
    let e = w.spawn((Health {
        current: 3.0,
        max: 10.0,
    },));
    w.set_json(e, "Health", json!({ "current": 9.0 })).unwrap();
    assert_eq!(
        w.get::<Health>(e),
        Some(&Health {
            current: 9.0,
            max: 10.0
        })
    );
}

#[test]
fn set_json_adds_a_missing_component() {
    let mut w = world();
    let e = w.spawn((Team(1),));
    w.set_json(e, "Health", json!({ "current": 1.0, "max": 2.0 })).unwrap();
    assert_eq!(w.get::<Health>(e), Some(&Health { current: 1.0, max: 2.0 }));
    // Transform2D has serde defaults, so a partial value is enough.
    w.set_json(e, "Transform2D", json!({ "rotation": 1.5 })).unwrap();
    let t = w.get::<Transform2D>(e).unwrap();
    assert_eq!((t.position, t.rotation, t.scale), (Vec2::ZERO, 1.5, Vec2::ONE));
}

#[test]
fn entity_json_lists_only_reflected_components() {
    let mut w = world();
    let e = w.spawn((Team(4), Secret, Transform2D::at(vec2(1.0, 2.0))));
    let v = w.entity_json(e).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort();
    assert_eq!(keys, ["Team", "Transform2D"]);
}

#[test]
fn spawn_json_reverses_entity_json() {
    let mut w = world();
    let a = w.spawn((
        Team(4),
        Health { current: 1.0, max: 5.0 },
        Transform2D::at(vec2(7.0, 8.0)),
    ));
    let snapshot = w.entity_json(a).unwrap();
    let b = w.spawn_json(snapshot.clone()).unwrap();
    assert_ne!(a, b);
    assert_eq!(w.entity_json(b).unwrap(), snapshot);
}

#[test]
fn failed_spawn_json_leaves_nothing_behind() {
    let mut w = world();
    let before = w.len();
    let err = w
        .spawn_json(json!({ "Team": 1, "Health": "not a health" }))
        .unwrap_err();
    assert!(matches!(err, ReflectError::BadValue { .. }), "{err}");
    assert!(matches!(
        w.spawn_json(json!({ "Team": 1, "Nope": 0 })).unwrap_err(),
        ReflectError::UnknownComponent(n) if n == "Nope"
    ));
    assert!(w.spawn_json(json!({})).is_err());
    assert!(w.spawn_json(json!([1, 2])).is_err());
    assert_eq!(w.len(), before);
}

#[test]
fn errors_say_what_went_wrong() {
    let mut w = world();
    let e = w.spawn((Team(1),));
    assert!(matches!(
        w.get_json(e, "Health"),
        Err(ReflectError::MissingComponent { component, .. }) if component == "Health"
    ));
    assert!(matches!(
        w.get_json(e, "Secret"),
        Err(ReflectError::UnknownComponent(_))
    ));
    let msg = w.set_json(e, "Team", json!("seven")).unwrap_err().to_string();
    assert!(msg.starts_with("bad value for `Team`"), "{msg}");
    w.despawn(e);
    assert_eq!(w.get_json(e, "Team"), Err(ReflectError::NoSuchEntity(e)));
    assert_eq!(w.set_json(e, "Team", json!(1)), Err(ReflectError::NoSuchEntity(e)));
}

#[test]
fn remove_json_removes_and_reports() {
    let mut w = world();
    let e = w.spawn((Team(1), Health { current: 1.0, max: 1.0 }));
    assert_eq!(w.remove_json(e, "Team"), Ok(true));
    assert_eq!(w.remove_json(e, "Team"), Ok(false));
    assert!(w.has::<Health>(e));
}

#[test]
fn reflecting_twice_is_harmless() {
    let mut w = world();
    w.reflect::<Health>();
    assert_eq!(w.reflected_components(), ["Transform2D", "Health", "Team"]);
}

mod other {
    use ponko::prelude::Component;
    #[derive(serde::Serialize, serde::Deserialize, Component)]
    pub struct Health(pub f32);
}

#[test]
#[should_panic(expected = "would both be named `Health`")]
fn two_types_with_one_short_name_panic() {
    let mut w = world();
    w.reflect::<other::Health>();
}

#[test]
fn games_reflect_the_builtin_components_by_default() {
    let world = Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.world.spawn((Transform2D::default(),));
    });
    let mut names = world.reflected_components();
    names.sort();
    let mut expected = vec![
        "Animation",
        "Attached",
        "DirectionalLight2D",
        "Occluder2D",
        "ParticleEmitter2D",
        "PointLight2D",
        "PrefabParts",
        "SpotLight2D",
        "Sprite",
        "Transform2D",
    ];
    if cfg!(feature = "3d") {
        expected.push("Transform3D");
        expected.extend([
            "AttachToJoint",
            "Attached3D",
            "SkinOf",
            "Decal3D",
            "LodGroup",
            "LodHysteresis",
            "Model",
            "ModelMaterial",
            "ModelMaterials",
            "PointLight3D",
            "ReceivesDecals",
            "ShadowMesh",
            "SpotLight3D",
        ]);
        expected.sort();
    }
    assert_eq!(names, expected);
}

// -------------------------------------------------------------- save/load

/// A component that refers to another entity: the reason ids are kept.
#[derive(Debug, PartialEq, Serialize, Deserialize, Component)]
struct Follows(Entity);

fn save_world() -> (World, Entity, Entity) {
    let mut w = world();
    w.reflect::<Follows>();
    // Churn the allocator so ids have gaps and generations.
    for _ in 0..5 {
        let tmp = w.spawn((Team(0),));
        w.despawn(tmp);
    }
    let leader = w.spawn((Team(1), Transform2D::at(vec2(3.0, 4.0))));
    let follower = w.spawn((Team(1), Follows(leader), Health { current: 2.0, max: 9.0 }));
    w.spawn((Secret,)); // not reflected: not saved
    (w, leader, follower)
}

#[test]
fn save_then_load_restores_entities_with_their_ids() {
    let (w, leader, follower) = save_world();
    let save = w.save();
    assert_eq!(save["entities"].as_array().unwrap().len(), 2);
    assert_eq!(save["not_saved"], json!({ "Secret": 1 }));

    let mut fresh = World::new();
    fresh.reflect::<Transform2D>();
    fresh.reflect::<Health>();
    fresh.reflect::<Team>();
    fresh.reflect::<Follows>();
    fresh.load(&save).unwrap();
    assert_eq!(fresh.len(), 2);
    // The reference still points at the leader.
    let target = fresh.get::<Follows>(follower).unwrap().0;
    assert_eq!(target, leader);
    assert_eq!(fresh.get::<Transform2D>(target).unwrap().position, vec2(3.0, 4.0));
    assert_eq!(fresh.get::<Health>(follower), Some(&Health { current: 2.0, max: 9.0 }));
    // Saving again gives the same entities (the unreflected one was never saved).
    assert_eq!(fresh.save()["entities"], save["entities"]);
    // New entities don't collide with loaded ids.
    let new = fresh.spawn((Team(9),));
    assert!(new != leader && new != follower);
}

#[test]
fn loading_into_the_same_world_rolls_it_back() {
    let (mut w, leader, follower) = save_world();
    let save = w.save();
    w.despawn(leader);
    w.get_mut::<Health>(follower).unwrap().current = 0.0;
    w.spawn((Team(5),));
    w.load(&save).unwrap();
    assert_eq!(w.save()["entities"], save["entities"]);
    // Entities that weren't in the save (nothing reflected) are gone too.
    assert_eq!(w.save()["not_saved"], json!({}));
    assert!(w.is_alive(leader));
    assert_eq!(w.get::<Health>(follower).unwrap().current, 2.0);
}

#[test]
fn a_bad_save_leaves_the_world_untouched() {
    let (mut w, _, _) = save_world();
    let before = w.save();
    let good = before.clone();
    let mut cases = Vec::new();
    let mut bad_value = good.clone();
    bad_value["entities"][1]["components"]["Health"] = json!("lots");
    cases.push(("bad value", bad_value));
    let mut unknown = good.clone();
    unknown["entities"][0]["components"]["Mystery"] = json!(1);
    cases.push(("unknown component", unknown));
    let mut dup = good.clone();
    dup["entities"][1]["entity"] = dup["entities"][0]["entity"].clone();
    cases.push(("duplicate id", dup));
    let mut version = good.clone();
    version["version"] = json!(99);
    cases.push(("version", version));
    cases.push(("not a save", json!({ "hello": 1 })));
    for (what, save) in cases {
        assert!(w.load(&save).is_err(), "{what} should fail");
        assert_eq!(w.save(), before, "{what} changed the world");
    }
}

/// A save of one entity with the given id, as a corrupt or hostile file could hold.
fn one_entity_save(id: u64) -> serde_json::Value {
    let mut save = World::new().save();
    save["entities"] = json!([ { "entity": id,
        "components": { "Transform2D": { "position": [0.0, 0.0], "rotation": 0.0, "scale": [1.0, 1.0] } } } ]);
    save
}

#[test]
fn huge_entity_indices_are_errors_that_allocate_nothing() {
    let (mut w, _, _) = save_world();
    let before = w.save();
    let (len, slots, bytes) = (w.len(), w.entity_slots(), w.allocated_bytes());
    // One past the limit for a one-entity save into a small world, about 2^31,
    // the largest index, and a big index with a generation.
    let limit = (1u64 << 22) + 8;
    for id in [
        limit,
        50_000_000,
        1 << 31,
        u64::from(u32::MAX - 1),
        50_000_000 | 7 << 32,
    ] {
        let err = w.load(&one_entity_save(id)).unwrap_err().to_string();
        assert!(err.contains("may only use indices below 4194312"), "{id}: {err}");
        assert_eq!(
            (w.len(), w.entity_slots(), w.allocated_bytes()),
            (len, slots, bytes),
            "{id}"
        );
        assert_eq!(w.save(), before, "{id} changed the world");
    }
    // Gaps within the limit are fine.
    w.load(&one_entity_save(limit - 1_000_000)).unwrap();
    assert_eq!(w.len(), 1);
}

#[test]
fn the_index_limit_grows_with_the_entities_in_the_save() {
    let mut save = one_entity_save(0);
    let entity = save["entities"][0].clone();
    let entities = save["entities"].as_array_mut().unwrap();
    for id in 1..1000u64 {
        let mut e = entity.clone();
        e["entity"] = json!(id);
        entities.push(e);
    }
    // 1000 entities may use indices below 8 * 1000 + 2^22.
    let mut last = entity.clone();
    last["entity"] = json!(8_000 + (1u64 << 22) - 1);
    entities.push(last.clone());
    let mut w = world();
    w.load(&save).unwrap();
    assert_eq!(w.len(), 1001);
    last["entity"] = json!(8_008 + (1u64 << 22));
    *save["entities"].as_array_mut().unwrap().last_mut().unwrap() = last;
    // Into a fresh world: `w` now has the slots, so it would vouch for them.
    let mut fresh = world();
    assert!(fresh.load(&save).is_err());
    assert_eq!(fresh.entity_slots(), 0);
}

#[test]
fn saves_with_heavy_despawn_churn_load_back() {
    let mut w = world();
    let all: Vec<Entity> = (0..100_000)
        .map(|i| w.spawn((Transform2D::at(vec2(i as f32, 0.0)),)))
        .collect();
    for (i, &e) in all.iter().enumerate() {
        if i % 1000 != 999 {
            w.despawn(e);
        }
    }
    // Churn generations too.
    for _ in 0..10 {
        let batch: Vec<Entity> = (0..50_000).map(|_| w.spawn((Team(0),))).collect();
        for e in batch {
            w.despawn(e);
        }
    }
    let save = w.save();
    assert_eq!(save["entities"].as_array().unwrap().len(), 100);
    let mut fresh = world();
    fresh.load(&save).unwrap();
    assert_eq!(fresh.save(), save);
    assert!(fresh.is_alive(all[99_999]));
}

#[test]
fn a_world_always_loads_its_own_save_back() {
    // A world that once used more slots than a fresh one would accept for
    // this save: its own slot table vouches for the ids.
    let mut w = world();
    let far = Entity::from_bits(5_000_000 | 2 << 32);
    assert!(w.spawn_at(far, (Transform2D::default(),)));
    let save = w.save();
    w.load(&save).unwrap();
    assert!(w.is_alive(far));
    assert!(world().load(&save).is_err(), "a fresh world has no such slots");
}

// ---- resources --------------------------------------------------------------------

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Score {
    red: u32,
    blue: u32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Round(u32);

/// Not reflected: saves and loads leave it alone.
struct Cache(Vec<u8>);

fn resource_world() -> World {
    let mut w = world();
    w.reflect_resource::<Score>();
    w.reflect_resource::<Round>();
    w.insert_resource(Score { red: 2, blue: 5 });
    w.insert_resource(Round(3));
    w.insert_resource(Cache(vec![1, 2, 3]));
    w
}

#[test]
fn resources_round_trip_through_json() {
    let mut w = resource_world();
    assert_eq!(w.reflected_resources(), ["Score", "Round"]);
    assert_eq!(w.resource_json("Score").unwrap(), json!({ "red": 2, "blue": 5 }));
    w.set_resource_json("Score", json!({ "red": 9 })).unwrap();
    assert_eq!(*w.resource::<Score>(), Score { red: 9, blue: 5 }, "only red changed");
    w.set_resource_json("Round", json!(4)).unwrap();
    assert_eq!(w.resource::<Round>().0, 4);
}

#[test]
fn setting_a_missing_resource_inserts_it() {
    let mut w = World::new();
    w.reflect_resource::<Score>();
    assert_eq!(
        w.resource_json("Score"),
        Err(ReflectError::MissingResource("Score".into()))
    );
    assert!(
        w.set_resource_json("Score", json!({ "red": 1 })).is_err(),
        "half a Score isn't one"
    );
    w.set_resource_json("Score", json!({ "red": 1, "blue": 0 })).unwrap();
    assert_eq!(*w.resource::<Score>(), Score { red: 1, blue: 0 });
}

#[test]
fn resource_errors_say_what_went_wrong() {
    let mut w = resource_world();
    let err = w.resource_json("Gold").unwrap_err();
    assert_eq!(err, ReflectError::UnknownResource("Gold".into()));
    assert!(err.to_string().contains("reflect_resource"), "{err}");
    let err = w.set_resource_json("Score", json!({ "red": "lots" })).unwrap_err();
    assert!(
        matches!(err, ReflectError::BadValue { ref component, .. } if component == "Score"),
        "{err}"
    );
    assert_eq!(*w.resource::<Score>(), Score { red: 2, blue: 5 }, "unchanged");
}

#[test]
fn saves_carry_reflected_resources() {
    let mut w = resource_world();
    let e = w.spawn((Health { current: 1.0, max: 2.0 },));
    let save = w.save();
    assert_eq!(
        save["resources"],
        json!({ "Score": { "red": 2, "blue": 5 }, "Round": 3 })
    );

    w.insert_resource(Score { red: 0, blue: 0 });
    w.insert_resource(Round(99));
    w.resource_mut::<Cache>().0.clear();
    w.load(&save).unwrap();
    assert_eq!(*w.resource::<Score>(), Score { red: 2, blue: 5 });
    assert_eq!(w.resource::<Round>().0, 3);
    assert!(
        w.resource::<Cache>().0.is_empty(),
        "resources that aren't reflected stay as they are"
    );
    assert_eq!(w.get::<Health>(e).unwrap().current, 1.0);
}

#[test]
fn saves_without_resources_still_load() {
    let mut w = resource_world();
    let mut save = w.save();
    save.as_object_mut().unwrap().remove("resources");
    w.insert_resource(Round(8));
    w.load(&save).unwrap();
    assert_eq!(w.resource::<Round>().0, 8, "nothing to restore");
}

#[test]
fn a_bad_resource_in_a_save_leaves_the_world_untouched() {
    let mut w = resource_world();
    w.spawn((Health { current: 1.0, max: 2.0 },));
    let before = w.save();
    for (what, resources) in [
        ("bad value", json!({ "Score": "lots" })),
        ("unknown resource", json!({ "Gold": 1 })),
        ("not an object", json!([1, 2])),
    ] {
        let mut save = before.clone();
        save["resources"] = resources;
        assert!(w.load(&save).is_err(), "{what} should fail");
        assert_eq!(w.save(), before, "{what} changed the world");
    }
}

#[test]
#[should_panic(expected = "would both be named `Score`")]
fn two_resource_types_with_one_short_name_panic() {
    #[derive(Serialize, Deserialize)]
    struct Score;
    let mut w = resource_world();
    w.reflect_resource::<Score>();
}

// ---- components described by fields (C and C++ components) -----------------------

/// A C-style struct: `{ float hp; uint8_t level; bool alive; Vec2 at; Color tint; uint64_t target; int16_t mood; }`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CUnit {
    hp: f32,
    level: u8,
    alive: bool,
    spare: u16, // not described: must survive edits, and be zero in new values
    at: [f32; 2],
    tint: [f32; 4],
    target: u64,
    mood: i16,
}

fn c_world() -> (World, ponko::ecs_core::ComponentId) {
    let mut world = World::new();
    // SAFETY: CUnit is plain bytes; bool is only written as 0/1 by reflection.
    let id = unsafe {
        world.raw().register_component(
            "CUnit",
            std::mem::size_of::<CUnit>(),
            std::mem::align_of::<CUnit>(),
            None,
        )
    };
    let fields = [
        ponko::reflect::Field::new("hp", ponko::reflect::FieldType::F32, std::mem::offset_of!(CUnit, hp)),
        ponko::reflect::Field::new(
            "level",
            ponko::reflect::FieldType::U8,
            std::mem::offset_of!(CUnit, level),
        ),
        ponko::reflect::Field::new(
            "alive",
            ponko::reflect::FieldType::Bool,
            std::mem::offset_of!(CUnit, alive),
        ),
        ponko::reflect::Field::new("at", ponko::reflect::FieldType::Vec2, std::mem::offset_of!(CUnit, at)),
        ponko::reflect::Field::new(
            "tint",
            ponko::reflect::FieldType::Color,
            std::mem::offset_of!(CUnit, tint),
        ),
        ponko::reflect::Field::new(
            "target",
            ponko::reflect::FieldType::Entity,
            std::mem::offset_of!(CUnit, target),
        ),
        ponko::reflect::Field::new(
            "mood",
            ponko::reflect::FieldType::I16,
            std::mem::offset_of!(CUnit, mood),
        ),
    ];
    // SAFETY: every field value makes a valid CUnit.
    unsafe { world.reflect_fields(id, &fields) };
    world.reflect::<Transform2D>();
    (world, id)
}

fn c_unit(world: &World, e: Entity, id: ponko::ecs_core::ComponentId) -> CUnit {
    // SAFETY: `id` is CUnit's id.
    unsafe { *world.raw_ref().get_dynamic(e, id).unwrap().cast::<CUnit>() }
}

fn spawn_c_unit(world: &mut World, id: ponko::ecs_core::ComponentId, unit: CUnit) -> Entity {
    // SAFETY: a valid CUnit.
    unsafe { world.raw().spawn_dynamic(&[(id, (&unit as *const CUnit).cast())]) }
}

#[test]
fn described_components_read_as_json_objects() {
    let (mut world, id) = c_world();
    let unit = CUnit {
        hp: 2.5,
        level: 3,
        alive: true,
        spare: 0xBEEF,
        at: [1.0, -2.0],
        tint: [1.0, 0.5, 0.25, 1.0],
        target: 7 | 1 << 32,
        mood: -4,
    };
    let e = spawn_c_unit(&mut world, id, unit);
    assert_eq!(
        world.get_json(e, "CUnit").unwrap(),
        serde_json::json!({
            "hp": 2.5, "level": 3, "alive": true, "at": [1.0, -2.0],
            "tint": { "r": 1.0, "g": 0.5, "b": 0.25, "a": 1.0 },
            "target": 7u64 | 1 << 32, "mood": -4
        })
    );
    assert!(world.reflected_components().contains(&"CUnit"));
}

#[test]
fn described_components_change_only_given_fields() {
    let (mut world, id) = c_world();
    let e = spawn_c_unit(
        &mut world,
        id,
        CUnit {
            hp: 1.0,
            spare: 0xBEEF,
            tint: [1.0, 1.0, 1.0, 1.0],
            ..Default::default()
        },
    );
    world
        .set_json(
            e,
            "CUnit",
            serde_json::json!({ "hp": 9.0, "alive": true, "tint": { "g": 0.0 } }),
        )
        .unwrap();
    let u = c_unit(&world, e, id);
    assert_eq!((u.hp, u.alive, u.tint), (9.0, true, [1.0, 0.0, 1.0, 1.0]));
    assert_eq!(u.spare, 0xBEEF, "bytes no field describes are left alone");
}

#[test]
fn described_components_spawn_and_add_with_missing_fields_zeroed() {
    let (mut world, id) = c_world();
    let e = world
        .spawn_json(
            serde_json::json!({ "CUnit": { "hp": 4.0, "at": [3.0, 4.0] }, "Transform2D": Transform2D::default() }),
        )
        .unwrap();
    assert_eq!(
        c_unit(&world, e, id),
        CUnit {
            hp: 4.0,
            at: [3.0, 4.0],
            ..Default::default()
        }
    );
    assert!(world.get::<Transform2D>(e).is_some());

    let bare = world.spawn((Transform2D::default(),));
    world
        .set_json(bare, "CUnit", serde_json::json!({ "level": 2 }))
        .unwrap();
    assert_eq!(c_unit(&world, bare, id).level, 2);
    assert_eq!(world.remove_json(bare, "CUnit"), Ok(true));
    assert!(world.raw_ref().get_dynamic(bare, id).is_none());
}

#[test]
fn described_components_reject_bad_json_and_keep_their_value() {
    let (mut world, id) = c_world();
    let e = spawn_c_unit(
        &mut world,
        id,
        CUnit {
            level: 5,
            ..Default::default()
        },
    );
    let cases = [
        (
            serde_json::json!({ "speed": 1.0 }),
            "has no field `speed` (fields: hp, level",
        ),
        (
            serde_json::json!({ "level": 300 }),
            "field `level`: expected an integer that fits",
        ),
        (
            serde_json::json!({ "level": 1.5 }),
            "field `level`: expected an integer",
        ),
        (serde_json::json!({ "mood": -40000 }), "field `mood`"),
        (serde_json::json!({ "alive": 1 }), "expected true or false"),
        (serde_json::json!({ "at": [1.0] }), "expected [x, y]"),
        (serde_json::json!({ "tint": { "q": 1.0 } }), "not `q`"),
        (serde_json::json!({ "hp": "lots" }), "expected a number"),
    ];
    for (value, message) in cases {
        let err = world.set_json(e, "CUnit", value.clone()).unwrap_err().to_string();
        assert!(err.contains(message), "{value}: {err}");
    }
    let err = world
        .spawn_json(serde_json::json!({ "CUnit": 5 }))
        .unwrap_err()
        .to_string();
    assert!(err.contains("expected an object with the fields of `CUnit`"), "{err}");
    assert_eq!(c_unit(&world, e, id).level, 5);
    assert_eq!(world.len(), 1);
}

#[test]
fn described_components_save_and_load_with_their_ids() {
    let (mut world, id) = c_world();
    let a = spawn_c_unit(
        &mut world,
        id,
        CUnit {
            hp: 1.0,
            ..Default::default()
        },
    );
    let b = spawn_c_unit(
        &mut world,
        id,
        CUnit {
            hp: 2.0,
            target: a.to_bits(),
            ..Default::default()
        },
    );
    world.despawn(a);
    let a2 = spawn_c_unit(
        &mut world,
        id,
        CUnit {
            hp: 3.0,
            ..Default::default()
        },
    );
    let save = world.save();
    assert_eq!(save["not_saved"], serde_json::json!({}));

    let (mut loaded, id2) = c_world();
    loaded.load(&save).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(c_unit(&loaded, b, id2).hp, 2.0);
    assert_eq!(c_unit(&loaded, b, id2).target, a.to_bits());
    assert_eq!(c_unit(&loaded, a2, id2).hp, 3.0);
    assert!(!loaded.is_alive(a), "the reused slot has the new generation");
}

#[test]
fn describing_again_replaces_the_fields() {
    let (mut world, id) = c_world();
    // SAFETY: as in c_world.
    unsafe {
        world.reflect_fields(
            id,
            &[ponko::reflect::Field::new("health", ponko::reflect::FieldType::F32, 0)],
        )
    };
    let e = spawn_c_unit(
        &mut world,
        id,
        CUnit {
            hp: 6.0,
            ..Default::default()
        },
    );
    assert_eq!(
        world.get_json(e, "CUnit").unwrap(),
        serde_json::json!({ "health": 6.0 })
    );
}

#[test]
#[should_panic(expected = "field `wide` of `Small` (offset 2, 4 bytes) doesn't fit in 4 bytes")]
fn fields_must_fit_in_the_component() {
    let mut world = World::new();
    // SAFETY: plain bytes.
    let id = unsafe { world.raw().register_component("Small", 4, 4, None) };
    // SAFETY: never reached: the call panics.
    unsafe {
        world.reflect_fields(
            id,
            &[ponko::reflect::Field::new("wide", ponko::reflect::FieldType::F32, 2)],
        )
    };
}

#[test]
#[should_panic(expected = "`Twice` has two fields named `x`")]
fn field_names_must_be_unique() {
    let mut world = World::new();
    // SAFETY: plain bytes.
    let id = unsafe { world.raw().register_component("Twice", 8, 4, None) };
    // SAFETY: never reached: the call panics.
    unsafe {
        world.reflect_fields(
            id,
            &[
                ponko::reflect::Field::new("x", ponko::reflect::FieldType::F32, 0),
                ponko::reflect::Field::new("x", ponko::reflect::FieldType::F32, 4),
            ],
        )
    };
}

#[test]
#[should_panic(expected = "is a Rust type or has a destructor; use `reflect::<T>()`")]
fn rust_types_use_reflect_instead() {
    let mut world = World::new();
    let id = world.register::<Transform2D>();
    // SAFETY: never reached: the call panics.
    unsafe {
        world.reflect_fields(
            id,
            &[ponko::reflect::Field::new(
                "rotation",
                ponko::reflect::FieldType::F32,
                8,
            )],
        )
    };
}

// ---- resources described by fields (C and C++ resources) ---------------------------

/// A C-style struct `{ uint32_t red, blue; float time; }`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CScore {
    red: u32,
    blue: u32,
    time: f32,
}

fn c_score_world() -> World {
    let mut w = World::new();
    // SAFETY: any field values make a valid CScore.
    unsafe {
        w.reflect_resource_fields(
            "game::Score",
            std::alloc::Layout::new::<CScore>(),
            &[
                ponko::reflect::Field::new("red", ponko::reflect::FieldType::U32, std::mem::offset_of!(CScore, red)),
                ponko::reflect::Field::new(
                    "blue",
                    ponko::reflect::FieldType::U32,
                    std::mem::offset_of!(CScore, blue),
                ),
                ponko::reflect::Field::new(
                    "time",
                    ponko::reflect::FieldType::F32,
                    std::mem::offset_of!(CScore, time),
                ),
            ],
        );
    }
    w
}

fn c_score(w: &World) -> CScore {
    // SAFETY: the resource holds a CScore.
    unsafe {
        *w.raw_ref()
            .get_resource_dynamic("game::Score")
            .unwrap()
            .cast::<CScore>()
    }
}

#[test]
fn field_described_resources_read_write_and_save() {
    let mut w = c_score_world();
    assert_eq!(w.reflected_resources(), ["game::Score"]);
    assert_eq!(
        w.resource_json("game::Score"),
        Err(ReflectError::MissingResource("game::Score".into()))
    );
    let value = CScore {
        red: 2,
        blue: 5,
        time: 1.5,
    };
    // SAFETY: `value` is a CScore.
    unsafe {
        w.raw().insert_resource_dynamic(
            "game::Score",
            std::alloc::Layout::new::<CScore>(),
            (&value as *const CScore).cast(),
        )
    };
    assert_eq!(
        w.resource_json("game::Score").unwrap(),
        json!({ "red": 2, "blue": 5, "time": 1.5 })
    );
    w.set_resource_json("game::Score", json!({ "blue": 9 })).unwrap();
    assert_eq!(
        c_score(&w),
        CScore {
            red: 2,
            blue: 9,
            time: 1.5
        },
        "only blue changed"
    );
    assert!(w.set_resource_json("game::Score", json!({ "red": "lots" })).is_err());

    let save = w.save();
    assert_eq!(save["resources"]["game::Score"]["blue"], 9);
    w.set_resource_json("game::Score", json!({ "red": 0, "blue": 0 }))
        .unwrap();
    w.load(&save).unwrap();
    assert_eq!(
        c_score(&w),
        CScore {
            red: 2,
            blue: 9,
            time: 1.5
        }
    );
}

#[test]
fn setting_a_missing_field_described_resource_inserts_it() {
    let mut w = c_score_world();
    w.set_resource_json("game::Score", json!({ "red": 4 })).unwrap();
    assert_eq!(
        c_score(&w),
        CScore {
            red: 4,
            blue: 0,
            time: 0.0
        },
        "missing fields are zero"
    );
}

#[test]
#[should_panic(expected = "is already reflected as")]
fn a_field_described_resource_cannot_take_a_rust_resources_name() {
    let mut w = World::new();
    w.reflect_resource::<Score>();
    // SAFETY: never read; the call panics.
    unsafe { w.reflect_resource_fields("Score", std::alloc::Layout::new::<u32>(), &[]) };
}
