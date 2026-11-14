//! Scenes and prefabs: worlds and entities saved to files and loaded back.

mod common;
#[path = "../examples/prefabs/game.rs"]
mod prefab_example;

use common::TempAsset;
#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
use ponko::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Component)]
struct Palm {
    height: f32,
}

fn game() -> Game {
    Game::new("scenes", 1, 1)
}

#[test]
fn a_saved_scene_loads_back_with_the_same_entities() {
    let file = TempAsset::new("test-scene-roundtrip.json");
    let mut first = None;
    game().run_headless(0, |ctx| {
        ctx.world.reflect::<Palm>();
        first = Some(ctx.world.spawn((Transform2D::at(vec2(1.0, 2.0)), Palm { height: 3.0 })));
        ctx.world.spawn((Transform2D::at(vec2(5.0, 6.0)),));
        ctx.assets.save_scene(&ctx.world, file.name()).unwrap();
    });
    let world = game().run_headless(0, |ctx| {
        ctx.world.reflect::<Palm>();
        ctx.world.spawn((Transform2D::default(),)); // replaced by the scene
        ctx.load_scene(file.name()).unwrap();
    });
    assert_eq!(world.len(), 2);
    let e = first.unwrap();
    assert_eq!(world.get::<Palm>(e), Some(&Palm { height: 3.0 }), "same id, same palm");
    assert_eq!(world.get::<Transform2D>(e).unwrap().position, vec2(1.0, 2.0));
}

#[test]
fn prefabs_spawn_where_asked_with_their_saved_components() {
    let file = TempAsset::new("test-prefab-palm.json");
    let mut world = game().run_headless(0, |ctx| {
        ctx.world.reflect::<Palm>();
        let model = ctx
            .world
            .spawn((Transform2D::at(vec2(70.0, 80.0)).rotated(0.5), Palm { height: 9.0 }));
        ctx.assets.save_prefab(&ctx.world, model, file.name()).unwrap();
        ctx.world.despawn(model);
        for x in 0..3 {
            ctx.spawn_prefab(file.name(), vec2(x as f32 * 10.0, 100.0)).unwrap();
        }
    });
    let mut palms: Vec<(Vec2, f32, f32)> = world
        .query::<(&Transform2D, &Palm)>()
        .iter()
        .map(|(t, p)| (t.position, t.rotation, p.height))
        .collect();
    palms.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    assert_eq!(
        palms,
        vec![
            (vec2(0.0, 100.0), 0.5, 9.0),
            (vec2(10.0, 100.0), 0.5, 9.0),
            (vec2(20.0, 100.0), 0.5, 9.0),
        ]
    );
}

#[test]
fn a_prefab_without_a_transform_gets_one() {
    let file = TempAsset::new("test-prefab-bare.json");
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(file.name()),
        r#"{ "format": "ponko-prefab", "version": 1, "components": { "Palm": { "height": 2 } } }"#,
    )
    .unwrap();
    let mut spawned = None;
    let world = game().run_headless(0, |ctx| {
        ctx.world.reflect::<Palm>();
        spawned = Some(ctx.spawn_prefab(file.name(), vec2(4.0, 5.0)).unwrap());
    });
    assert_eq!(
        world.get::<Transform2D>(spawned.unwrap()).unwrap().position,
        vec2(4.0, 5.0)
    );
}

#[test]
fn bad_files_are_errors_that_change_nothing() {
    let file = TempAsset::new("test-not-a-prefab.json");
    file.write(r#"{ "format": "ponko-world", "version": 1, "entities": [] }"#);
    game().run_headless(0, |ctx| {
        let missing = ctx.load_scene("test-no-such-scene.json").unwrap_err();
        assert_eq!(missing.kind(), ErrorKind::NotFound);
        assert_eq!(missing.path(), Some("test-no-such-scene.json"));
        assert!(missing.message().contains("not found in assets/"), "{missing}");
        let wrong = ctx.spawn_prefab(file.name(), Vec2::ZERO).unwrap_err();
        assert_eq!(wrong.kind(), ErrorKind::Json);
        assert_eq!(wrong.path(), Some(file.name()));
        assert!(wrong.message().contains("not a prefab"), "{wrong}");
        assert_eq!(ctx.world.len(), 0);
        let dead = ctx.world.spawn((Transform2D::default(),));
        ctx.world.despawn(dead);
        assert!(
            ctx.assets
                .save_prefab(&ctx.world, dead, "test-never-written.json")
                .is_err()
        );
    });
}

#[test]
fn scenes_with_huge_entity_ids_are_errors_that_change_nothing() {
    let file = TempAsset::new("test-huge-entity-id.json");
    game().run_headless(0, |ctx| {
        let kept = ctx.world.spawn((Transform2D::at(vec2(1.0, 2.0)),));
        let (slots, bytes) = (ctx.world.entity_slots(), ctx.world.allocated_bytes());
        for id in [50_000_000u64, 1 << 31] {
            // The issue's 181-byte file, with this id.
            let mut scene = World::new().save();
            scene["entities"] = serde_json::json!([ { "entity": id,
                "components": { "Transform2D": { "position": [0.0, 0.0], "rotation": 0.0, "scale": [1.0, 1.0] } } } ]);
            file.write(&scene.to_string());
            let err = ctx.load_scene(file.name()).unwrap_err();
            assert!(err.message().contains("may only use indices below"), "{err}");
            assert_eq!(ctx.world.len(), 1);
            assert_eq!((ctx.world.entity_slots(), ctx.world.allocated_bytes()), (slots, bytes));
            assert!(ctx.world.is_alive(kept));
        }
    });
}

// ANCHOR: scenes
fn beach(ctx: &mut Ctx) -> Result<(), String> {
    ctx.world.reflect::<Palm>(); // components in files must be reflected
    ctx.load_scene("levels/beach.json")?; // the level: its entities, ids kept
    for i in 0..3 {
        // three more palms, all alike, in a row
        ctx.spawn_prefab("prefabs/palm.json", vec2(400.0 + i as f32 * 64.0, 400.0))?;
    }
    Ok(())
}
// ANCHOR_END: scenes

// ANCHOR: prefab_tree
fn carts(ctx: &mut Ctx) -> Result<(), String> {
    // A cart: its body (the root), two wheels, a driver and the driver's hat.
    let cart = ctx.spawn_prefab("prefabs/cart.json", vec2(200.0, 300.0))?;
    let parts = ctx.world.get::<PrefabParts>(cart).map_or(0, |p| p.0.len()); // the four parts spawned with it
    ctx.despawn_prefab(cart); // the body and its four parts
    assert_eq!((parts, ctx.world.len()), (4, 0));
    Ok(())
}
// ANCHOR_END: prefab_tree

#[test]
fn the_guide_cart_comes_and_goes_whole() {
    let world = game().run_headless(0, |ctx| carts(ctx).unwrap());
    assert_eq!(world.len(), 0);
}

#[test]
fn the_guide_beach_has_five_palms() {
    let mut world = game().run_headless(0, |ctx| beach(ctx).unwrap());
    let mut heights: Vec<f32> = world.query::<&Palm>().iter().map(|p| p.height).collect();
    heights.sort_by(f32::total_cmp);
    assert_eq!(heights, [3.0, 4.0, 4.0, 4.0, 5.0]);
}

#[test]
fn sprites_are_saved_by_their_image_name_and_load_it() {
    let file = TempAsset::new("test-prefab-crab.json");
    game().run_headless(0, |ctx| {
        let crab = ctx.gfx.load_texture("crab.png");
        let mut sprite = Sprite::new(crab);
        sprite.z = 3.0;
        let model = ctx.world.spawn((Transform2D::default(), sprite));
        ctx.assets.save_prefab(&ctx.world, model, file.name()).unwrap();
    });
    let text = file.read();
    assert!(text.contains(r#""texture": "crab.png""#), "{text}");
    // A new game hasn't loaded crab.png: spawning the prefab loads it.
    game().run_headless(0, |ctx| {
        let e = ctx.spawn_prefab(file.name(), vec2(5.0, 5.0)).unwrap();
        let sprite = *ctx.world.get::<Sprite>(e).unwrap();
        assert_eq!(sprite.z, 3.0);
        assert_eq!(sprite.texture, ctx.gfx.load_texture("crab.png"), "the same handle");
    });
}

#[test]
fn sprites_of_textures_made_in_code_are_left_out_of_saves() {
    game().run_headless(0, |ctx| {
        let made = ctx.gfx.create_texture(1, 1, &[255; 4]);
        let e = ctx.world.spawn((Transform2D::default(), Sprite::new(made)));
        let json = ctx.world.entity_json(e).unwrap();
        assert!(
            json.get("Transform2D").is_some() && json.get("Sprite").is_none(),
            "{json}"
        );
    });
}

#[test]
fn a_sprite_needs_an_image_that_exists() {
    game().run_headless(0, |ctx| {
        let e = ctx.world.spawn((Transform2D::default(),));
        let missing = ctx
            .world
            .set_json(e, "Sprite", serde_json::json!({ "z": 1 }))
            .unwrap_err();
        assert!(missing.to_string().contains("needs \"texture\""), "{missing}");
        let unloaded = ctx
            .world
            .set_json(e, "Sprite", serde_json::json!({ "texture": "crab.png" }))
            .unwrap_err();
        assert!(unloaded.to_string().contains("isn't loaded"), "{unloaded}");
    });
}

#[test]
fn both_prefab_spellings_load_and_resave_with_primary_identifier() {
    for format in ["ponko-prefab", "ponko-prefab"] {
        let file = TempAsset::new(&format!("test-naming-{format}.json"));
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(file.name());
        std::fs::write(
            &path,
            serde_json::json!({
                "format": format, "version": 1, "components": {"Palm": {"height": 7.0}}
            })
            .to_string(),
        )
        .unwrap();
        game().run_headless(0, |ctx| {
            ctx.world.reflect::<Palm>();
            let entity = ctx.spawn_prefab(file.name(), Vec2::ZERO).unwrap();
            assert_eq!(ctx.world.get::<Palm>(entity).unwrap().height, 7.0);
            ctx.assets.save_prefab(&ctx.world, entity, file.name()).unwrap();
            let output: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(output["format"], "ponko-prefab");
            assert_eq!(output["version"], 1);
            let again = ctx.spawn_prefab(file.name(), Vec2::ZERO).unwrap();
            assert_eq!(ctx.world.get::<Palm>(again).unwrap().height, 7.0);
        });
    }
}

// Prefabs with children.

/// A game component that names another entity.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Component)]
struct Follows(Entity);

/// A game component holding a plain number, which must never be remapped.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Component)]
struct Count(u64);

fn reflect_tree_types(ctx: &mut Ctx) {
    ctx.world.reflect::<Palm>();
    ctx.world.reflect::<Follows>();
    ctx.world.reflect::<Count>();
}

/// A cart (the root) with a driver attached to it and a hat attached to the
/// driver (two levels), and a flag that isn't attached (a plain part) but
/// follows the hat. Returns the root.
fn build_cart(ctx: &mut Ctx, at: Vec2) -> Entity {
    use ponko::attach::Attached;
    let cart = ctx.world.spawn((Transform2D::at(at), Palm { height: 1.0 }));
    let driver = ctx.world.spawn((
        Transform2D::at(at + vec2(0.0, -10.0)),
        Attached::to(cart).offset(vec2(0.0, -10.0)),
        Palm { height: 2.0 },
    ));
    let hat = ctx.world.spawn((
        Transform2D::at(at + vec2(0.0, -15.0)),
        Attached::to(driver).offset(vec2(0.0, -5.0)),
        Palm { height: 3.0 },
    ));
    ctx.world.insert(cart, Follows(hat));
    let flag = ctx.world.spawn((
        Transform2D::at(at + vec2(20.0, 0.0)),
        Palm { height: 4.0 },
        Follows(hat),
    ));
    ctx.world.insert(cart, PrefabParts(vec![flag]));
    cart
}

/// The parts of a spawned cart, by height: (driver, hat, flag).
fn cart_parts(world: &World, cart: Entity) -> (Entity, Entity, Entity) {
    let parts = &world.get::<PrefabParts>(cart).expect("the root lists its parts").0;
    assert_eq!(parts.len(), 3, "{parts:?}");
    let by_height = |h: f32| {
        *parts
            .iter()
            .find(|&&p| world.get::<Palm>(p).map(|p| p.height) == Some(h))
            .unwrap_or_else(|| panic!("no part of height {h}"))
    };
    (by_height(2.0), by_height(3.0), by_height(4.0))
}

fn assert_cart(world: &World, cart: Entity, at: Vec2) {
    let (driver, hat, flag) = cart_parts(world, cart);
    assert_eq!(world.get::<Transform2D>(cart).unwrap().position, at);
    assert_eq!(world.get::<ponko::attach::Attached>(driver).unwrap().to, cart);
    assert_eq!(world.get::<ponko::attach::Attached>(hat).unwrap().to, driver);
    assert_eq!(
        world.get::<Follows>(cart),
        Some(&Follows(hat)),
        "the root names the new hat"
    );
    assert_eq!(world.get::<Follows>(flag), Some(&Follows(hat)));
    // Placed relative to the root, before any attachment system ran.
    assert_eq!(world.get::<Transform2D>(hat).unwrap().position, at + vec2(0.0, -15.0));
    assert_eq!(world.get::<Transform2D>(flag).unwrap().position, at + vec2(20.0, 0.0));
}

#[test]
fn a_two_level_prefab_spawns_saves_reloads_and_despawns_with_its_children() {
    let prefab = TempAsset::new("test-prefab-cart.json");
    let scene = TempAsset::new("test-scene-carts.json");
    let mut carts = Vec::new();
    game().run_headless(0, |ctx| {
        reflect_tree_types(ctx);
        let model = build_cart(ctx, vec2(300.0, 300.0));
        ctx.assets.save_prefab(&ctx.world, model, prefab.name()).unwrap();
    });
    let saved: serde_json::Value = serde_json::from_str(&prefab.read()).unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(
        saved["entities"].as_array().unwrap().len(),
        4,
        "root, driver, hat, flag"
    );

    game().run_headless(0, |ctx| {
        reflect_tree_types(ctx);
        for at in [vec2(10.0, 20.0), vec2(-50.0, 7.0)] {
            carts.push((ctx.spawn_prefab(prefab.name(), at).unwrap(), at));
        }
        assert_eq!(ctx.world.len(), 8);
        for &(cart, at) in &carts {
            assert_cart(&ctx.world, cart, at);
        }
        assert_ne!(
            cart_parts(&ctx.world, carts[0].0).1,
            cart_parts(&ctx.world, carts[1].0).1
        );
        ctx.assets.save_scene(&ctx.world, scene.name()).unwrap();
    });

    // A saved world keeps the trees: same ids, same parts, same references.
    game().run_headless(0, |ctx| {
        reflect_tree_types(ctx);
        ctx.load_scene(scene.name()).unwrap();
        assert_eq!(ctx.world.len(), 8);
        for &(cart, at) in &carts {
            assert_cart(&ctx.world, cart, at);
        }
        let (driver, hat, flag) = cart_parts(&ctx.world, carts[0].0);
        assert_eq!(ctx.despawn_prefab(carts[0].0), 4);
        for gone in [carts[0].0, driver, hat, flag] {
            assert!(!ctx.world.is_alive(gone));
        }
        assert_eq!(ctx.world.len(), 4, "the other cart stays whole");
        assert_cart(&ctx.world, carts[1].0, carts[1].1);
        // Saving a spawned root saves the whole tree again.
        ctx.assets.save_prefab(&ctx.world, carts[1].0, prefab.name()).unwrap();
        let again = ctx.spawn_prefab(prefab.name(), vec2(1.0, 2.0)).unwrap();
        assert_cart(&ctx.world, again, vec2(1.0, 2.0));
    });
}

#[test]
fn despawning_an_entity_without_parts_despawns_only_it() {
    game().run_headless(0, |ctx| {
        let a = ctx.world.spawn((Transform2D::default(),));
        let b = ctx.world.spawn((Transform2D::default(),));
        assert_eq!(ctx.despawn_prefab(a), 1);
        assert_eq!(ctx.despawn_prefab(a), 0, "already gone");
        assert!(ctx.world.is_alive(b));
    });
}

#[test]
fn only_entity_values_of_the_prefab_are_remapped() {
    // Saved ids 0 and 1; a plain number 1 and a reference to an entity
    // outside the prefab (id 7) are read as written.
    let file = TempAsset::with_contents(
        "test-prefab-numbers.json",
        &serde_json::json!({
            "format": "ponko-prefab", "version": 2,
            "entities": [
                { "entity": 0, "components": { "Count": 1, "Follows": 1 } },
                { "entity": 1, "components": { "Count": 0, "Follows": 7 } }
            ]
        })
        .to_string(),
    );
    game().run_headless(0, |ctx| {
        reflect_tree_types(ctx);
        ctx.world.spawn((Count(99),)); // so new ids differ from the saved ones
        let root = ctx.spawn_prefab(file.name(), Vec2::ZERO).unwrap();
        let part = ctx.world.get::<PrefabParts>(root).unwrap().0[0];
        assert_eq!(ctx.world.get::<Count>(root), Some(&Count(1)));
        assert_eq!(ctx.world.get::<Count>(part), Some(&Count(0)));
        assert_eq!(ctx.world.get::<Follows>(root), Some(&Follows(part)));
        assert_eq!(ctx.world.get::<Follows>(part), Some(&Follows(Entity::from_bits(7))));
    });
}

#[test]
fn parts_without_attachments_follow_the_roots_rotation_and_scale() {
    let file = TempAsset::with_contents(
        "test-prefab-turned.json",
        &serde_json::json!({
            "format": "ponko-prefab", "version": 2,
            "entities": [
                { "entity": 5, "components": { "Transform2D": { "rotation": 0.0, "scale": [1.0, 1.0] } } },
                { "entity": 6, "components": { "Transform2D": { "position": [10.0, 0.0], "rotation": 0.25, "scale": [2.0, 2.0] } } }
            ]
        })
        .to_string(),
    );
    game().run_headless(0, |ctx| {
        let turned = Transform2D::at(vec2(100.0, 100.0))
            .rotated(std::f32::consts::FRAC_PI_2)
            .scaled(vec2(3.0, 3.0));
        let root = ctx.spawn_prefab(file.name(), turned).unwrap();
        assert_eq!(*ctx.world.get::<Transform2D>(root).unwrap(), turned);
        let part = ctx.world.get::<PrefabParts>(root).unwrap().0[0];
        let t = *ctx.world.get::<Transform2D>(part).unwrap();
        // A quarter turn points the offset down (+y), three times as long.
        assert!((t.position - vec2(100.0, 130.0)).length() < 1e-4, "{t:?}");
        assert!((t.rotation - (std::f32::consts::FRAC_PI_2 + 0.25)).abs() < 1e-6);
        assert_eq!(t.scale, vec2(6.0, 6.0));
    });
}

#[test]
fn included_prefabs_spawn_with_their_roots_changed_for_this_use() {
    let wheel = TempAsset::with_contents(
        "test-prefab-wheel.json",
        r#"{ "format": "ponko-prefab", "version": 1,
             "components": { "Transform2D": { "scale": [0.5, 0.5] }, "Palm": { "height": 1.0 }, "Count": 4 } }"#,
    );
    let axle = TempAsset::with_contents(
        "test-prefab-axle.json",
        &serde_json::json!({
            "format": "ponko-prefab", "version": 2,
            "entities": [
                { "entity": 0, "components": { "Transform2D": {}, "Palm": { "height": 10.0 } } },
                { "entity": 1, "prefab": wheel.name(),
                  "components": { "Transform2D": { "position": [-8.0, 0.0] }, "Follows": 0 } },
                { "entity": 2, "prefab": wheel.name(),
                  "components": { "Transform2D": { "position": [8.0, 0.0] }, "Palm": { "height": 2.0 }, "Follows": 1 } }
            ]
        })
        .to_string(),
    );
    let cart = TempAsset::with_contents(
        "test-prefab-axle-cart.json",
        &serde_json::json!({
            "format": "ponko-prefab", "version": 2,
            "entities": [
                { "entity": 0, "components": { "Transform2D": {}, "Palm": { "height": 100.0 } } },
                { "entity": 1, "prefab": axle.name(), "components": { "Transform2D": { "position": [0.0, 20.0] } } }
            ]
        })
        .to_string(),
    );
    game().run_headless(0, |ctx| {
        reflect_tree_types(ctx);
        let root = ctx.spawn_prefab(cart.name(), vec2(1000.0, 0.0)).unwrap();
        let parts = ctx.world.get::<PrefabParts>(root).unwrap().0.clone();
        assert_eq!(parts.len(), 3, "the axle and its two wheels");
        assert_eq!(ctx.world.len(), 4);
        let find = |ctx: &Ctx, h: f32| {
            *parts
                .iter()
                .find(|&&p| ctx.world.get::<Palm>(p).unwrap().height == h)
                .unwrap()
        };
        let axle_root = find(ctx, 10.0);
        let (left, right) = (find(ctx, 1.0), find(ctx, 2.0));
        assert_eq!(
            ctx.world.get::<Transform2D>(axle_root).unwrap().position,
            vec2(1000.0, 20.0)
        );
        let l = ctx.world.get::<Transform2D>(left).unwrap();
        assert_eq!(
            (l.position, l.scale),
            (vec2(992.0, 20.0), vec2(0.5, 0.5)),
            "only the position changed"
        );
        assert_eq!(
            ctx.world.get::<Transform2D>(right).unwrap().position,
            vec2(1008.0, 20.0)
        );
        assert_eq!(
            ctx.world.get::<Count>(right),
            Some(&Count(4)),
            "the rest as the wheel file says"
        );
        assert_eq!(ctx.world.get::<Follows>(left), Some(&Follows(axle_root)));
        assert_eq!(ctx.world.get::<Follows>(right), Some(&Follows(left)));
        assert_eq!(ctx.world.get::<PrefabParts>(axle_root).unwrap().0.len(), 2);
        assert_eq!(ctx.despawn_prefab(root), 4);
        assert_eq!(ctx.world.len(), 0);
    });
}

#[test]
fn prefabs_that_include_each_other_are_refused_and_spawn_nothing() {
    let a = TempAsset::new("test-prefab-loop-a.json");
    let b = TempAsset::new("test-prefab-loop-b.json");
    let include = |other: &str| {
        serde_json::json!({
            "format": "ponko-prefab", "version": 2,
            "entities": [
                { "entity": 0, "components": { "Count": 1 } },
                { "entity": 1, "prefab": other }
            ]
        })
        .to_string()
    };
    a.write(&include(b.name()));
    b.write(&include(a.name()));
    game().run_headless(0, |ctx| {
        reflect_tree_types(ctx);
        let err = ctx.spawn_prefab(a.name(), Vec2::ZERO).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Json);
        assert_eq!(err.path(), Some(a.name()));
        assert!(err.message().contains("loop"), "{err}");
        assert_eq!(ctx.world.len(), 0);
    });
}

#[test]
fn a_tree_with_a_bad_part_spawns_nothing() {
    let file = TempAsset::with_contents(
        "test-prefab-bad-part.json",
        &serde_json::json!({
            "format": "ponko-prefab", "version": 2,
            "entities": [
                { "entity": 0, "components": { "Count": 1 } },
                { "entity": 1, "components": { "Count": 2, "Follows": 0 } },
                { "entity": 2, "components": { "NotReflected": {} } }
            ]
        })
        .to_string(),
    );
    let duplicate = TempAsset::with_contents(
        "test-prefab-duplicate-id.json",
        r#"{ "format": "ponko-prefab", "version": 2, "entities": [
             { "entity": 0, "components": { "Count": 1 } }, { "entity": 0, "components": { "Count": 2 } } ] }"#,
    );
    game().run_headless(0, |ctx| {
        reflect_tree_types(ctx);
        let err = ctx.spawn_prefab(file.name(), Vec2::ZERO).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(err.message().contains("NotReflected"), "{err}");
        let err = ctx.spawn_prefab(duplicate.name(), Vec2::ZERO).unwrap_err();
        assert!(err.message().contains("appears twice"), "{err}");
        assert_eq!(ctx.world.len(), 0);
    });
}

/// 3D trees: relative 3D transforms, skin targets and joint attachments
/// name the new copies.
#[cfg(feature = "3d")]
#[test]
fn a_3d_prefab_keeps_its_skin_targets_and_joint_attachments() {
    use ponko::anim::{AttachToJoint, SkinOf};
    use ponko::d3::Transform as T3;
    let file = TempAsset::new("test-prefab-person.json");
    game().run_headless(0, |ctx| {
        let body = ctx
            .world
            .spawn((T3::at(vec3(5.0, 0.0, 0.0)).rotated(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),));
        let hat = ctx.world.spawn((SkinOf(body),));
        let cup = ctx.world.spawn((
            T3::at(vec3(5.0, 0.0, -2.0)),
            AttachToJoint {
                skeleton: body,
                joint: 3,
                offset: T3::at(vec3(0.0, 0.1, 0.0)),
            },
        ));
        ctx.assets.save_prefab(&ctx.world, body, file.name()).unwrap();
        for e in [body, hat, cup] {
            ctx.world.despawn(e);
        }

        let root = ctx.spawn_prefab(file.name(), vec3(0.0, 0.0, 10.0)).unwrap();
        let parts = ctx.world.get::<PrefabParts>(root).unwrap().0.clone();
        assert_eq!(parts.len(), 2);
        let hat = *parts.iter().find(|&&p| ctx.world.has::<SkinOf>(p)).unwrap();
        let cup = *parts.iter().find(|&&p| ctx.world.has::<AttachToJoint>(p)).unwrap();
        assert_eq!(ctx.world.get::<SkinOf>(hat), Some(&SkinOf(root)));
        let a = ctx.world.get::<AttachToJoint>(cup).unwrap();
        assert_eq!((a.skeleton, a.joint), (root, 3));
        // The cup was 2 m in front of the turned body; it still is.
        let at = ctx.world.get::<T3>(cup).unwrap().position;
        assert!((at - vec3(0.0, 0.0, 8.0)).length() < 1e-4, "{at}");
    });
}

#[cfg(feature = "physics3d")]
#[test]
fn a_3d_prefabs_joints_join_its_new_bodies() {
    use ponko::d3::{Joint, RigidBody, Transform as T3};
    let file = TempAsset::new("test-prefab-hinge.json");
    Game::new("scenes", 1, 1).physics3d(Vec3::ZERO).run_headless(0, |ctx| {
        let door = ctx.world.spawn((T3::default(), RigidBody::dynamic()));
        let frame = ctx.world.spawn((T3::at(vec3(1.0, 0.0, 0.0)), RigidBody::fixed()));
        let hinge = ctx.world.spawn((Joint::spherical(frame, door),));
        ctx.world.insert(door, PrefabParts(vec![frame]));
        ctx.assets.save_prefab(&ctx.world, door, file.name()).unwrap();
        for e in [door, frame, hinge] {
            ctx.world.despawn(e);
        }
        let root = ctx.spawn_prefab(file.name(), vec3(0.0, 2.0, 0.0)).unwrap();
        let parts = ctx.world.get::<PrefabParts>(root).unwrap().0.clone();
        let joint = parts.iter().find_map(|&p| ctx.world.get::<Joint>(p)).unwrap();
        assert_eq!(joint.body_b, root);
        assert!(parts.contains(&joint.body_a) && joint.body_a != root);
        assert_eq!(ctx.world.get::<T3>(joint.body_a).unwrap().position, vec3(1.0, 2.0, 0.0));
    });
}

/// The `prefabs` example, headless: three carts of five parts each (a body,
/// two included wheels, a driver and a hat on the driver); the wheels roll
/// with their cart, and scrapping a cart takes all five away.
#[test]
fn the_prefab_example_rolls_carts_and_scraps_one_whole() {
    use ponko::attach::Attached;
    let mut carts = Vec::new();
    let world = prefab_example::game().run_headless(prefab_example::SCRAP_AT - 1, |ctx| {
        carts = prefab_example::setup(ctx);
        assert_eq!(ctx.world.len(), 15);
    });
    let rolled = (prefab_example::SCRAP_AT - 1) as f32 * prefab_example::SPEED;
    for (i, &cart) in carts.iter().enumerate() {
        let at = vec2(120.0 + i as f32 * 200.0 + rolled, 300.0);
        assert_eq!(world.get::<Transform2D>(cart).unwrap().position, at);
        let parts = world.get::<PrefabParts>(cart).unwrap().0.clone();
        assert_eq!(parts.len(), 4);
        let wheels: Vec<Vec2> = parts
            .iter()
            .filter(|&&p| {
                world
                    .get::<Attached>(p)
                    .is_some_and(|a| a.to == cart && a.offset.y > 0.0)
            })
            .map(|&p| world.get::<Transform2D>(p).unwrap().position)
            .collect();
        assert_eq!(wheels.len(), 2);
        assert!(wheels.iter().all(|w| (w.y - (at.y + 20.0)).abs() < 1e-3), "{wheels:?}");
        let driver = parts
            .iter()
            .copied()
            .find(|&p| {
                world
                    .get::<Attached>(p)
                    .is_some_and(|a| a.to == cart && a.offset.y < 0.0)
            })
            .unwrap();
        let hat = parts
            .iter()
            .find(|&&p| world.get::<Attached>(p).is_some_and(|a| a.to == driver))
            .unwrap();
        let hat_at = world.get::<Transform2D>(*hat).unwrap().position;
        // On the driver, whose scale (0.75) stretches the hat's offset.
        assert!(
            (hat_at - (at + vec2(0.0, -28.0 - 24.0 * 0.75))).length() < 1e-3,
            "{hat_at}"
        );
    }

    let world = prefab_example::game().run_headless(prefab_example::SCRAP_AT + 1, |ctx| {
        prefab_example::setup(ctx);
    });
    assert_eq!(world.len(), 10, "one cart and its four parts went");
}

#[cfg(feature = "physics3d")]
#[test]
fn a_joint_drive_without_a_force_limit_reads_back_unlimited() {
    use ponko::d3::{Joint, JointAxis, JointDrive};
    let joint = Joint::spherical(Entity::from_bits(1), Entity::from_bits(2))
        .drive(JointAxis::AngularX, JointDrive::velocity(2.0, 10.0));
    let json = serde_json::to_value(joint).unwrap();
    assert_eq!(json["drives"][3]["max_force"], serde_json::Value::Null);
    let back: Joint = serde_json::from_value(json).unwrap();
    assert_eq!(back, joint);
}

// ---- sprite-sheet animations ----------------------------------------------------

#[test]
fn a_prefab_with_an_animated_sprite_spawns_animating() {
    // crab.png is 32 x 32: four 16 x 16 frames.
    let prefab = r#"{"format": "ponko-prefab", "version": 1, "components": {
        "Sprite": { "texture": "crab.png", "region": { "min": [0, 0], "max": [16, 16] } },
        "Animation": { "texture": "crab.png", "frame_size": [16, 16], "fps": 10 }
    }}"#;
    let mut crab = None;
    let world = game().headless_frames(true).run_headless(20, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("prefabs").with("prefabs/crab.json", prefab));
        crab = Some(ctx.spawn_prefab("prefabs/crab.json", vec2(5.0, 5.0)).unwrap());
    });
    let crab = crab.unwrap();
    let animation = world
        .get::<ponko::sheet::Animation>(crab)
        .expect("the prefab's animation");
    assert_eq!((animation.first, animation.frames, animation.sheet.len), (0, 4, 4));
    assert!(animation.time > 0.3, "a third of a second: {}", animation.time);
    let sprite = world.get::<Sprite>(crab).unwrap();
    assert_eq!(sprite.region, Some(animation.sheet.frame(animation.current_frame())));
    assert_ne!(animation.current_frame(), 0, "past the first frame");
}

#[test]
fn a_scene_keeps_each_animations_frame() {
    let file = TempAsset::new("test-scene-animated.json");
    let mut crab = None;
    let saved = game().run_headless(0, |ctx| {
        let sheet = SpriteSheet::new(ctx.gfx.load_texture("crab.png"), 16, 16);
        let mut walk = ponko::sheet::Animation::new(sheet, 1..4, 6.0).once();
        walk.time = 0.4;
        crab = Some(ctx.world.spawn((Transform2D::default(), sheet.sprite(1), walk)));
        ctx.assets.save_scene(&ctx.world, file.name()).unwrap();
    });
    let crab = crab.unwrap();
    let before = saved.get::<ponko::sheet::Animation>(crab).unwrap().clone();
    let world = game().run_headless(0, |ctx| ctx.load_scene(file.name()).unwrap());
    let after = world
        .get::<ponko::sheet::Animation>(crab)
        .expect("loaded with the scene");
    assert_eq!(after, &before);
    assert_eq!(after.current_frame(), 3);
}
