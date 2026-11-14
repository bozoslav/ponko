//! Tilemaps from level editors (Tiled, LDtk), animated and flipped tiles,
//! parallax, autotiling and solid tiles, checked headless. The fixture
//! levels are in `tests/fixtures/tilemaps`.

use ponko::d2::{Sprite, Transform as Transform2D};
use ponko::prelude::*;
use ponko::tilemap::{
    AutoTile, LayerKind, Level, LevelLayer, LevelObject, LevelOptions, LevelRoot, ObjectShape, TileMap,
};

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/tilemaps/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn load(name: &str, options: &LevelOptions) -> Level {
    let mut level = None;
    let path = fixture(name);
    let options = options.clone();
    Game::new("levels", 1, 1).run_headless(0, |ctx| {
        level = Some(ctx.assets.load_level(&path, &options).unwrap());
    });
    level.unwrap()
}

fn layer<'a>(level: &'a Level, name: &str) -> &'a ponko::tilemap::Layer {
    level
        .layers
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no layer {name}"))
}

fn objects<'a>(level: &'a Level, name: &str) -> &'a [LevelObject] {
    match &layer(level, name).kind {
        LayerKind::Objects(o) => o,
        other => panic!("{name} is {other:?}"),
    }
}

fn tiles<'a>(level: &'a Level, name: &str) -> &'a ponko::tilemap::TileLayer {
    match &layer(level, name).kind {
        LayerKind::Tiles(t) => t,
        other => panic!("{name} is {other:?}"),
    }
}

fn object<'a>(list: &'a [LevelObject], name: &str) -> &'a LevelObject {
    list.iter()
        .find(|o| o.name == name)
        .unwrap_or_else(|| panic!("no object {name}"))
}

// ---- Tiled -----------------------------------------------------------------

#[test]
fn a_tmx_map_loads_its_layers_in_order_with_groups_flattened() {
    let level = load("level.tmx", &LevelOptions::default());
    let names: Vec<&str> = level.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        ["Sky", "Background", "Ground", "Collision", "Objects", "Foreground"]
    );
    assert_eq!(level.size, UVec2::new(8, 6));
    assert_eq!(level.cell_size, vec2(16.0, 16.0));
    assert_eq!(level.parallax_origin, vec2(64.0, 48.0));
    assert!(level.background.is_some());

    let fg = layer(&level, "Foreground");
    assert_eq!(fg.offset, vec2(0.0, -4.0), "the group's offset");
    assert_eq!(fg.opacity, 0.5);
    assert_eq!((fg.tint.r, fg.tint.g), (1.0, 0.0));
    assert!(!layer(&level, "Collision").visible);
    assert_eq!(layer(&level, "Background").parallax, vec2(0.5, 0.5));
    assert_eq!(layer(&level, "Background").properties.bool("far"), Some(true));
    assert_eq!(layer(&level, "Sky").parallax, Vec2::ZERO);
    assert!(matches!(&layer(&level, "Sky").kind, LayerKind::Image(i) if i.ends_with("tilemaps/tiles.png")));
}

#[test]
fn tiled_tilesets_load_embedded_and_external_with_animations_and_shapes() {
    let level = load("level.tmx", &LevelOptions::default());
    assert_eq!(level.tilesets.len(), 2);
    let ground = &level.tilesets[0];
    assert_eq!((ground.name.as_str(), ground.columns, ground.count), ("ground", 4, 8));
    assert!(
        ground.image.ends_with("tilemaps/tiles.png"),
        "resolved against the .tsx"
    );
    assert_eq!(ground.tiles[&1].animation, [(1, 0.2), (2, 0.3)]);
    assert_eq!(ground.tiles[&3].class, "Wall");
    assert_eq!(ground.tiles[&3].properties.bool("solid"), Some(true));
    assert_eq!(ground.tiles[&5].shapes[0].size, vec2(16.0, 8.0));
    assert!(matches!(ground.tiles[&6].shapes[0].shape, ObjectShape::Polygon(ref p) if p.len() == 3));
    let spaced = &level.tilesets[1];
    assert_eq!((spaced.margin, spaced.spacing, spaced.columns), (1, 2, 2));
    assert!(level.files.iter().any(|f| f.ends_with("ground.tsx")));
    assert!(level.files.iter().any(|f| f.ends_with("coin.tx")));
}

#[test]
fn tiled_tile_data_keeps_cells_tilesets_and_flips() {
    let level = load("level.tmx", &LevelOptions::default());
    let ground = tiles(&level, "Ground"); // base64 + zlib
    assert_eq!(ground.tiles.len(), 14);
    let at = |x: u32, y: u32| *ground.tiles.iter().find(|t| t.cell == UVec2::new(x, y)).unwrap();
    assert_eq!((at(0, 5).tileset, at(0, 5).id, at(0, 5).flip), (0, 3, 0));
    assert_eq!(at(3, 4).flip, TileMap::FLIP_X);
    assert_eq!(at(7, 4).flip, TileMap::FLIP_DIAGONAL | TileMap::FLIP_X);
    assert_eq!((at(6, 4).tileset, at(6, 4).id), (1, 1), "second tileset, its own ids");
    assert_eq!(tiles(&level, "Background").tiles.len(), 48); // CSV
}

#[test]
fn tiled_objects_have_centers_shapes_classes_and_typed_properties() {
    let level = load("level.tmx", &LevelOptions::default());
    let objs = objects(&level, "Objects");
    assert_eq!(objs.len(), 5);

    let door = object(objs, "Door");
    assert_eq!(
        (door.class.as_str(), door.position, door.size),
        ("Door", vec2(104.0, 64.0), vec2(16.0, 32.0))
    );
    assert_eq!(door.properties.str("target"), Some("level2.tmj"));
    assert_eq!(door.properties.bool("locked"), Some(true));
    assert_eq!(door.properties.color("glow").map(|c| (c.g, c.r)), Some((1.0, 0.0)));

    let spawn = object(objs, "Spawn");
    assert_eq!((spawn.position, &spawn.shape), (vec2(24.0, 72.0), &ObjectShape::Point));
    assert!(matches!(object(objs, "Hill").shape, ObjectShape::Polygon(ref p) if p[2] == vec2(8.0, -8.0)));

    // Tile objects hang from their bottom-left corner.
    let coin = objs.iter().find(|o| o.id == 5).unwrap();
    assert_eq!(
        (coin.class.as_str(), coin.position, coin.tile),
        ("Coin", vec2(80.0, 72.0), Some((0, 1, 0)))
    );

    // From a template, with the instance's property over the template's.
    let templated = objs.iter().find(|o| o.id == 1).unwrap();
    assert_eq!((templated.name.as_str(), templated.class.as_str()), ("Coin", "Coin"));
    assert_eq!(templated.position, vec2(48.0, 40.0));
    assert_eq!(templated.tile, Some((0, 4, 0)));
    assert_eq!(templated.properties.i64("value"), Some(10));
    assert_eq!(templated.properties.str("sound"), Some("coin.wav"));

    assert_eq!(level.properties.str("music"), Some("theme.ogg"));
    assert_eq!(level.properties.f32("gravity"), Some(980.0));
    assert_eq!(
        level.properties.get("rules").unwrap()["lives"],
        3,
        "class properties are objects"
    );
}

#[test]
fn a_tmj_infinite_map_reads_chunks_left_of_the_origin_and_json_tilesets() {
    let level = load("level.tmj", &LevelOptions::default());
    let ground = layer(&level, "Ground");
    assert_eq!(ground.offset, vec2(-64.0, 0.0), "the first chunk starts 4 tiles left");
    let t = tiles(&level, "Ground");
    assert_eq!(t.size, UVec2::new(8, 4));
    assert_eq!(t.tiles.len(), 6); // gzip
    let torch = t.tiles.iter().find(|t| t.tileset == 1).unwrap();
    assert_eq!(
        (torch.cell, torch.id, torch.flip),
        (UVec2::new(7, 0), 0, TileMap::FLIP_Y)
    );
    assert_eq!(level.tilesets[1].tiles[&0].class, "Torch");
    assert_eq!(level.tilesets[1].tiles[&0].animation.len(), 2);

    let lamp = object(objects(&level, "Things"), "Lamp");
    assert_eq!(lamp.class, "Light");
    assert!((lamp.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    // Turned a quarter clockwise around its top-left corner.
    assert!((lamp.position - vec2(-40.0, 24.0)).length() < 1e-4, "{}", lamp.position);
    assert_eq!(lamp.properties.f32("radius"), Some(48.5));
    assert!(matches!(
        object(objects(&level, "Things"), "Path").shape,
        ObjectShape::Polyline(_)
    ));
    assert_eq!(level.properties.str("title"), Some("Caves"));
}

// ---- LDtk --------------------------------------------------------------------

#[test]
fn an_ldtk_level_loads_its_layers_bottom_to_top_with_int_grids_and_entities() {
    let level = load("world.ldtk", &LevelOptions::default());
    assert_eq!(level.name, "Level_0");
    let names: Vec<&str> = level.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Terrain", "Decor", "Entities"]);
    assert_eq!(level.tilesets.len(), 1, "the built-in icons have no image");
    assert_eq!(level.tilesets[0].tiles[&3].tags, ["Solid"]);
    assert_eq!(level.tilesets[0].tiles[&1].properties.str("data"), Some("water"));

    let terrain = tiles(&level, "Terrain");
    assert_eq!(terrain.tiles.len(), 9, "auto-layer tiles");
    assert_eq!(terrain.int_grid[5 * 8], 1);
    assert_eq!(terrain.int_grid_names[&2], "water");
    let decor = tiles(&level, "Decor");
    assert_eq!(
        decor.tiles.iter().find(|t| t.cell == UVec2::new(1, 1)).unwrap().flip,
        TileMap::FLIP_X
    );
    assert_eq!(layer(&level, "Decor").parallax, vec2(0.5, 0.75));

    let ents = objects(&level, "Entities");
    let player = ents.iter().find(|o| o.class == "Player").unwrap();
    assert_eq!(player.position, vec2(24.0, 72.0), "pivot at the feet");
    assert_eq!(player.properties.f32("speed"), Some(1.5));
    assert_eq!(player.properties.i64("hp"), Some(3));
    assert_eq!(player.properties.str("iid"), Some("player-1"));
    let coin = ents.iter().find(|o| o.class == "Coin").unwrap();
    assert_eq!((coin.position, coin.tile), (vec2(44.0, 44.0), Some((0, 6, 0))));
    assert_eq!(coin.properties.get("tags").unwrap()[0], "pickup");
    assert_eq!(level.properties.str("music"), Some("cave.ogg"));
}

#[test]
fn ldtk_levels_are_picked_by_name_and_read_from_their_own_files() {
    let second = load("world.ldtk", &LevelOptions::default().level("Level_1"));
    assert_eq!((second.name.as_str(), second.size), ("Level_1", UVec2::new(4, 4)));
    let external = load("split.ldtk", &LevelOptions::default());
    assert_eq!(external.name, "Level_1");
    assert!(external.files.iter().any(|f| f.ends_with("split/Level_1.ldtkl")));

    let path = fixture("world.ldtk");
    Game::new("levels", 1, 1).run_headless(0, |ctx| {
        let err = ctx
            .assets
            .load_level(&path, &LevelOptions::default().level("Nope"))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(err.to_string().contains("Level_0, Level_1"), "{err}");
    });
}

#[test]
fn files_that_are_not_levels_are_refused_with_the_reason() {
    Game::new("levels", 1, 1).run_headless(0, |ctx| {
        ctx.assets.add_source(
            MemorySource::new("bad")
                .with(
                    "iso.tmx",
                    r#"<map orientation="isometric" tilewidth="16" tileheight="16"/>"#,
                )
                .with("level.txt", "x")
                .with("broken.tmx", "<map>"),
        );
        let o = LevelOptions::default();
        assert_eq!(
            ctx.assets.load_level("iso.tmx", &o).unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        assert_eq!(
            ctx.assets.load_level("level.txt", &o).unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        assert_eq!(
            ctx.assets.load_level("broken.tmx", &o).unwrap_err().kind(),
            ErrorKind::Decode
        );
        assert_eq!(
            ctx.assets.load_level("missing.tmx", &o).unwrap_err().kind(),
            ErrorKind::NotFound
        );
    });
}

// ---- spawning ------------------------------------------------------------------

fn spawned(name: &str, options: LevelOptions, at: Vec2) -> World {
    let path = fixture(name);
    Game::new("levels", 1, 1).run_headless(0, move |ctx| {
        ctx.spawn_level_with(&path, at, options).unwrap();
    })
}

#[test]
fn spawned_tmx_layers_become_tile_maps_stacked_by_z() {
    let mut world = spawned("level.tmx", LevelOptions::default(), vec2(100.0, 0.0));
    let mut maps: Vec<(String, f32, Vec2, Vec2)> = world
        .query::<(&Transform2D, &TileMap, &LevelLayer)>()
        .iter()
        .map(|(t, m, l)| (l.name.clone(), m.z, t.position, m.parallax))
        .collect();
    maps.sort_by(|a, b| a.1.total_cmp(&b.1));
    let names: Vec<&str> = maps.iter().map(|m| m.0.as_str()).collect();
    // Ground uses two tilesets: two maps. The hidden Collision layer isn't drawn.
    assert_eq!(names, ["Sky", "Background", "Ground", "Ground", "Foreground"]);
    let z = |name: &str| maps.iter().find(|m| m.0 == name).unwrap().1;
    assert!(z("Sky") < z("Background") && z("Background") < z("Ground"));
    assert!(
        z("Ground") < 0.0 && z("Foreground") > 0.0,
        "objects (and sprites) are at z 0"
    );
    let fg = maps.iter().find(|m| m.0 == "Foreground").unwrap();
    assert_eq!(fg.2, vec2(100.0, -4.0));
    assert_eq!(maps.iter().find(|m| m.0 == "Background").unwrap().3, vec2(0.5, 0.5));

    let animated = world
        .query::<(&TileMap, &LevelLayer)>()
        .iter()
        .find(|(_, l)| l.name == "Background")
        .map(|(m, _)| m.animations.clone())
        .unwrap();
    assert_eq!(animated[0].tile, 1);
    assert_eq!(animated[0].frames, [(1, 0.2), (2, 0.3)]);

    // Margin and spacing: the tiles are packed into a texture of their own.
    let spaced = world
        .query::<&TileMap>()
        .iter()
        .find(|m| m.sheet.texture.width() == 32)
        .cloned()
        .expect("the spaced tileset, repacked to 2 x 16 px");
    assert_eq!(spaced.get(6, 4), Some(1));
}

#[test]
fn spawned_objects_carry_their_level_object_and_tile_objects_get_sprites() {
    let mut world = spawned("level.tmx", LevelOptions::default(), vec2(100.0, 0.0));
    let objs: Vec<(Vec2, LevelObject)> = world
        .query::<(&Transform2D, &LevelObject)>()
        .iter()
        .map(|(t, o)| (t.position, o.clone()))
        .collect();
    assert_eq!(objs.len(), 5);
    let door = objs.iter().find(|o| o.1.name == "Door").unwrap();
    assert_eq!(door.0, vec2(204.0, 64.0), "at the level's place");
    let sprites = world.query::<(&LevelObject, &Sprite)>().iter().count();
    assert_eq!(sprites, 2, "the two coins show their tiles");

    let root = world
        .query::<(Entity, &LevelRoot)>()
        .iter()
        .next()
        .map(|(e, r)| (e, r.clone()))
        .unwrap();
    assert_eq!(root.1.properties.str("music"), Some("theme.ogg"));
    assert!(root.1.entities.iter().all(|&e| world.is_alive(e)));
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize, Component)]
struct Pickup {
    value: i32,
}

#[test]
fn objects_of_a_class_spawn_as_its_prefab_with_their_level_object() {
    let path = fixture("level.tmx");
    let mut world = Game::new("levels", 1, 1).run_headless(0, move |ctx| {
        ctx.world.reflect::<Pickup>();
        let prefab = r#"{"format": "ponko-prefab", "version": 1, "components": {"Pickup": {"value": 1}}}"#;
        ctx.assets
            .add_source(MemorySource::new("prefabs").with("prefabs/coin.json", prefab));
        let options = LevelOptions::default().prefab("Coin", "prefabs/coin.json");
        ctx.spawn_level_with(&path, Vec2::ZERO, options).unwrap();
    });
    let coins: Vec<(Vec2, i64)> = world
        .query::<(&Transform2D, &Pickup, &LevelObject)>()
        .iter()
        .map(|(t, _, o)| (t.position, o.properties.i64("value").unwrap_or(0)))
        .collect();
    assert_eq!(coins.len(), 2);
    assert!(coins.contains(&(vec2(48.0, 40.0), 10)));
}

#[test]
fn spawned_ldtk_levels_split_stacked_tiles_into_maps_and_despawn_cleanly() {
    let path = fixture("world.ldtk");
    let mut world = Game::new("levels", 1, 1).run_headless(0, move |ctx| {
        let root = ctx.spawn_level(&path, Vec2::ZERO).unwrap();
        let decor = ctx
            .world
            .query::<&LevelLayer>()
            .iter()
            .filter(|l| l.name == "Decor")
            .count();
        assert_eq!(decor, 2, "two tiles in one cell: two maps");
        assert_eq!(ctx.world.query::<(&LevelObject, &Sprite)>().iter().count(), 1);
        assert!(ctx.despawn_level(root));
        assert!(!ctx.despawn_level(root));
    });
    assert_eq!(world.query::<&TileMap>().iter().count(), 0);
    assert_eq!(world.query::<&LevelObject>().iter().count(), 0);
}

// ---- solid tiles -------------------------------------------------------------

fn area(rects: &[Rect]) -> f32 {
    rects.iter().map(|r| (r.max - r.min).x * (r.max - r.min).y).sum()
}

#[test]
fn tiled_solid_tiles_come_from_tile_properties_full_shapes_and_collision_layers() {
    let level = load("level.tmx", &LevelOptions::default());
    let rects = level.solid_rects(&LevelOptions::default());
    // Row 5 (8 walls), (7, 4) wall, (5, 4) full shape, (0, 0) of the hidden Collision layer.
    assert_eq!(area(&rects), 11.0 * 256.0, "{rects:?}");
    assert!(rects.len() <= 5, "merged: {rects:?}");
    let covered = |x: f32, y: f32| rects.iter().any(|r| r.contains(vec2(x, y)));
    assert!(covered(8.0, 8.0) && covered(120.0, 72.0) && covered(88.0, 72.0));
    assert!(!covered(72.0, 72.0), "the half block has its own shape collider");
}

#[test]
fn ldtk_int_grid_values_and_tile_tags_are_solid_by_name() {
    let level = load("world.ldtk", &LevelOptions::default());
    assert!(level.solid_rects(&LevelOptions::default()).is_empty());
    let walls = level.solid_rects(&LevelOptions::default().solid("wall"));
    assert_eq!(walls, [Rect::new(0.0, 80.0, 128.0, 16.0)]);
    let water = level.solid_rects(&LevelOptions::default().solid("water"));
    assert_eq!(water, [Rect::new(32.0, 48.0, 16.0, 16.0)]);
    let tagged = level.solid_rects(&LevelOptions::default().solid("Solid"));
    assert_eq!(tagged, walls, "the auto-layer's wall tiles carry the Solid tag");
}

#[test]
fn tile_maps_merge_solid_tiles_into_few_rectangles() {
    Game::new("solid", 1, 1).run_headless(0, |ctx| {
        let tex = ctx.gfx.create_texture(32, 16, &[255; 32 * 16 * 4]);
        let mut map = TileMap::new(SpriteSheet::new(tex, 16, 16), 10, 10);
        for i in 0..10 {
            for (x, y) in [(i, 0), (i, 9), (0, i), (9, i)] {
                map.set(x, y, Some(1));
            }
        }
        map.set(5, 5, Some(0)); // floor, not solid
        let rects = map.solid_rects(|t| t == 1);
        assert_eq!(rects.len(), 4);
        assert_eq!(area(&rects), 36.0 * 256.0);
    });
}

// ---- autotiling ----------------------------------------------------------------

fn sheet(ctx: &mut Ctx) -> SpriteSheet {
    let tex = ctx.gfx.create_texture(256, 256, &vec![255; 256 * 256 * 4]);
    SpriteSheet::new(tex, 16, 16)
}

#[test]
fn four_neighbour_autotiles_pick_frames_by_their_neighbours() {
    Game::new("auto", 1, 1).run_headless(0, |ctx| {
        let rows = ["....", ".##.", ".#..", "...."];
        let filled = |x: u32, y: u32| rows[y as usize].as_bytes()[x as usize] == b'#';
        let rule = AutoTile::four(std::array::from_fn(|m| 100 + m as u16)).outside(false);
        let mut map = TileMap::new(sheet(ctx), 4, 4);
        map.autotile(&rule, filled);
        assert_eq!(map.get(1, 1), Some(100 + (AutoTile::E | AutoTile::S) as u16));
        assert_eq!(map.get(2, 1), Some(100 + AutoTile::W as u16));
        assert_eq!(map.get(1, 2), Some(100 + AutoTile::N as u16));
        assert_eq!(map.get(0, 0), None, "unfilled cells are left alone");

        // Outside the map counts as filled by default.
        let mut edge = TileMap::new(sheet(ctx), 1, 1);
        edge.autotile(&AutoTile::four(std::array::from_fn(|m| m as u16)), |_, _| true);
        assert_eq!(edge.get(0, 0), Some(15));
    });
}

#[test]
fn eight_neighbour_autotiles_ignore_corners_without_both_sides() {
    assert_eq!(AutoTile::BLOB_MASKS.len(), 47);
    let rule = AutoTile::eight(u16::from);
    // A corner alone doesn't count; with both its sides it does.
    assert_eq!(rule.frame(AutoTile::NE), 0);
    assert_eq!(rule.frame(AutoTile::NE | AutoTile::N8), u16::from(AutoTile::N8));
    let full = AutoTile::N8 | AutoTile::NE | AutoTile::E8;
    assert_eq!(rule.frame(full), u16::from(full));
    let mask = rule.mask(1, 1, 3, 3, |x, y| (x, y) != (2, 2));
    assert_eq!(mask, 255 - AutoTile::SE);
    let blob = AutoTile::blob(std::array::from_fn(|i| i as u16));
    assert_eq!(blob.frame(255), 46);
}

#[test]
fn painting_an_autotile_redoes_its_neighbours() {
    Game::new("auto", 1, 1).run_headless(0, |ctx| {
        let rule = AutoTile::four(std::array::from_fn(|m| 100 + m as u16)).outside(false);
        let mut map = TileMap::new(sheet(ctx), 3, 1);
        map.paint_autotile(&rule, 0, 0, true);
        assert_eq!(map.get(0, 0), Some(100));
        map.paint_autotile(&rule, 1, 0, true);
        assert_eq!(map.get(0, 0), Some(100 + AutoTile::E as u16));
        assert_eq!(map.get(1, 0), Some(100 + AutoTile::W as u16));
        map.paint_autotile(&rule, 0, 0, false);
        assert_eq!(map.get(0, 0), None);
        assert_eq!(map.get(1, 0), Some(100));
        map.paint_autotile(&rule, 7, 7, true); // out of bounds: ignored
    });
}

// ---- drawing -------------------------------------------------------------------

#[test]
fn parallax_maps_follow_the_camera_by_their_factor() {
    Game::new("parallax", 800, 600).run_headless(1, |ctx| {
        let s = sheet(ctx);
        let mut sky = TileMap::new(s, 10, 10);
        sky.tiles.fill(0);
        sky.parallax = Vec2::ZERO; // stays on the screen
        ctx.world.spawn((Transform2D::default(), sky));
        ctx.camera.position = vec2(50_000.0, 50_000.0);
        systems::draw_tilemaps(ctx);
        assert_eq!(ctx.gfx.sprite_count(), 100, "still in view, the camera far away");
        let map = ctx.world.query::<&TileMap>().iter().next().unwrap().clone();
        let center = ctx.camera.position + vec2(400.0, 300.0);
        assert_eq!(map.drawn_origin(Vec2::ZERO, center), center);
    });
}

#[test]
fn animated_and_flipped_tiles_draw_one_sprite_each() {
    Game::new("anim", 800, 600).run_headless(1, |ctx| {
        let mut map = TileMap::new(sheet(ctx), 4, 4);
        map.tiles.fill(3);
        map.animate(3, &[(3, 0.1), (4, 0.1)]);
        map.set_flipped(1, 1, Some(2), TileMap::FLIP_X | TileMap::FLIP_DIAGONAL);
        assert_eq!(map.flip(1, 1), TileMap::FLIP_X | TileMap::FLIP_DIAGONAL);
        assert_eq!(map.flip(0, 0), 0);
        ctx.world.spawn((Transform2D::default(), map));
        systems::draw_tilemaps(ctx);
        assert_eq!(ctx.gfx.sprite_count(), 16);
    });
}

// ---- hot reload ----------------------------------------------------------------

/// The fixtures copied to a temporary folder, to edit.
fn temp_fixtures(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-levels-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["level.tmx", "ground.tsx", "coin.tx", "tiles.png", "spaced.png"] {
        std::fs::copy(fixture(f), dir.join(f)).unwrap();
    }
    dir
}

/// Spawns `level.tmx` from `dir` with hot reload, runs `edit` on tick 2
/// and returns the ticks that reloaded it and the objects at the end.
fn edit_and_run(dir: &std::path::Path, edit: impl Fn() + 'static) -> (Vec<u64>, usize, usize) {
    use std::sync::{Arc, Mutex};
    let path = dir.join("level.tmx").to_str().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let name = path.clone();
    let mut world = Game::new("hot", 1, 1)
        .hot_reload(true)
        .sim_system(move |ctx: &mut Ctx| {
            if ctx.assets.reloaded_files().contains(&name) {
                log.lock().unwrap().push(ctx.time.sim_tick());
            }
            if ctx.time.sim_tick() == 2 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                edit();
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        })
        .run_headless(5, move |ctx| {
            ctx.spawn_level(&path, Vec2::ZERO).unwrap();
        });
    let roots: Vec<LevelRoot> = world.query::<&LevelRoot>().iter().cloned().collect();
    assert_eq!(roots.len(), 1, "the root stays");
    assert!(roots[0].entities.iter().all(|&e| world.is_alive(e)));
    let objects = world.query::<&LevelObject>().iter().count();
    let walls = world
        .query::<&LevelObject>()
        .iter()
        .filter(|o| o.class == "Door")
        .count();
    let seen = seen.lock().unwrap().clone();
    (seen, objects, walls)
}

#[test]
fn editing_a_level_file_respawns_the_level_in_place() {
    let dir = temp_fixtures("level");
    let file = dir.join("level.tmx");
    let (seen, objects, doors) = edit_and_run(&dir, move || {
        let text = std::fs::read_to_string(&file).unwrap();
        let start = text.find("  <object id=\"3\"").unwrap();
        let end = start + text[start..].find("</object>").unwrap() + "</object>\n".len();
        std::fs::write(&file, format!("{}{}", &text[..start], &text[end..])).unwrap();
    });
    assert_eq!(seen, [3], "reloaded once, on the tick after the edit");
    assert_eq!((objects, doors), (4, 0), "the door is gone, the rest spawned again");
}

#[test]
fn editing_a_tileset_respawns_the_levels_using_it() {
    let dir = temp_fixtures("tileset");
    let file = dir.join("ground.tsx");
    let (seen, objects, _) = edit_and_run(&dir, move || {
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text.replace("type=\"Wall\"", "type=\"Stone\"")).unwrap();
    });
    assert_eq!(seen, [3]);
    assert_eq!(objects, 5);
}

#[test]
fn a_broken_edit_keeps_the_old_level() {
    let dir = temp_fixtures("broken");
    let file = dir.join("level.tmx");
    let (seen, objects, _) = edit_and_run(&dir, move || std::fs::write(&file, "<map").unwrap());
    assert!(seen.is_empty());
    assert_eq!(objects, 5);
}

#[test]
fn the_example_level_loads_with_its_coins_and_animated_water() {
    let mut world = Game::new("meadow", 1, 1).run_headless(0, |ctx| {
        ctx.spawn_level("levels/meadow.tmx", Vec2::ZERO).unwrap();
    });
    let coins = world
        .query::<&LevelObject>()
        .iter()
        .filter(|o| o.class == "Coin")
        .count();
    assert_eq!(coins, 6);
    let water = world
        .query::<(&TileMap, &LevelLayer)>()
        .iter()
        .find(|(_, l)| l.name == "Water")
        .map(|(m, _)| m.animations.len());
    assert_eq!(water, Some(1));
}

// ---- saves -------------------------------------------------------------------

#[derive(Clone, Default, serde::Serialize, serde::Deserialize, Component)]
struct Hero {
    lives: u32,
}

/// The coin prefab and the game's components, reflected.
fn coins(ctx: &mut Ctx) {
    ctx.world.reflect::<Pickup>();
    ctx.world.reflect::<Hero>();
    let prefab = r#"{"format": "ponko-prefab", "version": 1, "components": {"Pickup": {"value": 1}}}"#;
    ctx.assets
        .add_source(MemorySource::new("prefabs").with("prefabs/coin.json", prefab));
}

fn coin_options() -> LevelOptions {
    LevelOptions::default().prefab("Coin", "prefabs/coin.json")
}

#[test]
fn a_world_save_holds_the_gameplay_entities_not_the_level() {
    let path = fixture("level.tmx");
    let mut ids = Vec::new();
    let mut save = serde_json::Value::Null;
    Game::new("levels", 1, 1).run_headless(0, |ctx| {
        coins(ctx);
        ctx.spawn_level_with(&path, Vec2::ZERO, coin_options()).unwrap();
        assert!(ctx.world.query::<&TileMap>().iter().count() > 0);
        ids.push(ctx.world.spawn((Transform2D::at(vec2(8.0, 8.0)), Hero { lives: 3 })));
        ids.push(ctx.world.spawn((Transform2D::at(vec2(30.0, 8.0)),)));
        ids.extend(ctx.world.query::<(Entity, &Pickup)>().iter().map(|(e, _)| e));
        save = ctx.world.save();
    });
    // The hero, the plain entity and the two coins the level spawned as
    // prefabs: no layers, no plain objects, no root.
    let mut saved: Vec<u64> = save["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["entity"].as_u64().unwrap())
        .collect();
    let mut expected: Vec<u64> = ids.iter().map(|e| e.to_bits()).collect();
    saved.sort_unstable();
    expected.sort_unstable();
    assert_eq!(saved, expected, "{save:#}");
    assert_eq!(ids.len(), 4);
    // Only the coins' `LevelObject`s (not reflected) are counted as not
    // saved: no tile maps, layers or level root.
    assert_eq!(save["not_saved"], serde_json::json!({ "LevelObject": 2 }));
}

#[test]
fn a_loaded_save_and_the_level_respawned_without_its_saved_objects_make_the_level_again() {
    let path = fixture("level.tmx");
    let mut save = serde_json::Value::Null;
    let mut kept = None;
    Game::new("levels", 1, 1).run_headless(0, |ctx| {
        coins(ctx);
        ctx.spawn_level_with(&path, Vec2::ZERO, coin_options()).unwrap();
        // One coin collected before the save.
        let coins: Vec<Entity> = ctx.world.query::<(Entity, &Pickup)>().iter().map(|(e, _)| e).collect();
        ctx.world.despawn(coins[0]);
        kept = Some(coins[1]);
        save = ctx.world.save();
    });
    let reference_count = {
        let path = path.clone();
        let mut world = Game::new("levels", 1, 1).run_headless(0, move |ctx| {
            coins(ctx);
            ctx.spawn_level_with(&path, Vec2::ZERO, coin_options()).unwrap();
        });
        world.query::<&TileMap>().iter().count()
    };
    let mut world = Game::new("levels", 1, 1).run_headless(0, |ctx| {
        coins(ctx);
        ctx.world.load(&save).unwrap();
        assert_eq!(ctx.world.query::<&TileMap>().iter().count(), 0, "the save has no level");
        let options = coin_options().without_saved_objects();
        let root = ctx.spawn_level_with(&path, Vec2::ZERO, options).unwrap();
        assert!(ctx.world.has::<ponko::reflect::NotSaved>(root));
    });
    let coins: Vec<Entity> = world.query::<(Entity, &Pickup)>().iter().map(|(e, _)| e).collect();
    assert_eq!(coins, [kept.unwrap()], "the saved coin, not the level's two again");
    assert_eq!(
        world.query::<&TileMap>().iter().count(),
        reference_count,
        "every layer is back"
    );
    // The plain objects come from the file; the coins' objects don't.
    assert_eq!(world.query::<&LevelObject>().iter().count(), 3);
    // Saving again still holds only the coin.
    assert_eq!(world.save()["entities"].as_array().unwrap().len(), 1);
}
