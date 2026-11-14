# Sprite sheets, animation and tilemaps

## Sprite sheets

A sprite sheet is one texture holding many frames of the same size: an
animation strip, a tileset, a set of icons. Frames are numbered left to
right, top to bottom.

<div class="lang-pair">
<div>

```rust
# let pos = vec2(100.0, 100.0);
let tex = ctx.gfx.load_texture("units.png");
let sheet = SpriteSheet::new(tex, 32, 32);   // 32 × 32 frames
ctx.world.spawn((ponko::d2::Transform::at(pos), sheet.sprite(5))); // frame 5
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:sheet}}
```

</div>
</div>

Under the hood a sprite has a `region`, the part of its texture to show in
texture pixels. `sheet.sprite(i)` is `Sprite::new(tex).with_region(sheet.frame(i))`,
and `ctx.gfx.draw_ex` takes the same `region` in `DrawParams`.

For pixel art, turn smoothing off so pixels stay square:

<div class="lang-pair">
<div>

```rust
# let tex = ctx.gfx.load_texture("units.png");
ctx.gfx.set_texture_filter(tex, TextureFilter::Nearest);
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:nearest}}
```

</div>
</div>

## Animation

Add an `Animation` next to the `Sprite`; the built-in `animate_sprites`
system (on by default, all cores) advances it every frame:

<div class="lang-pair">
<div>

```rust
# use ponko::sheet::Animation;
# let pos = vec2(100.0, 100.0);
# let sheet = SpriteSheet::new(ctx.gfx.load_texture("units.png"), 32, 32);
ctx.world.spawn((
    ponko::d2::Transform::at(pos),
    sheet.sprite(0),
    Animation::new(sheet, 0..8, 12.0), // frames 0-7 at 12 fps, looping
));
let boom = Animation::new(sheet, 8..16, 24.0).once(); // stops on the last frame
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:animation}}
```

</div>
</div>

`animation.finished()` tells you when a `once()` animation is done, so you
can despawn the explosion. In C++, `ponko::animation` is a plain struct, so
an `each` can take `ponko::animation&` like `ponko::transform2d&`.

Animations are [reflected](reflection.md) as `"Animation"`, so prefabs and
scenes can give a sprite one, and world saves keep how far each one got
(its frame). In a file it names the sheet's image and frame size like a
sprite names its image; the rest has defaults (the frames from `first` to
the end of the sheet, looping, from the start):

```json
"Sprite": { "texture": "units.png", "region": { "min": [0, 0], "max": [32, 32] } },
"Animation": { "texture": "units.png", "frame_size": [32, 32], "first": 0, "frames": 8, "fps": 12 }
```

A prefab with both spawns animating. Saves also write the sheet's grid
(`columns`, `len`), `looping` and `time`; an animation whose image was
made in code has no file to name and isn't saved, like its sprite.

## Tilemaps

A `TileMap` is a grid of sheet frames on an entity with a `d2::Transform`
(its position is the map's top-left corner). The built-in `draw_tilemaps`
system draws only the tiles the camera can see, so a map can be huge: the
tiles example has a million tiles, and zoomed out to 380 000 visible tiles it
still runs at over 500 FPS.

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/tiles.rs:setup}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:tilemap}}
```

</div>
</div>

Changing tiles is writing into the map. In C++, `world.tilemap(e)` gives the
engine's tiles directly (`set`, `get`, `tiles()` for bulk writes), with no copy:

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/tiles.rs:paint}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:paint}}
```

</div>
</div>

- `TileMap::EMPTY` (or `set(x, y, None)`) leaves a hole.
- `tile_size` defaults to the sheet's frame size; set it to scale tiles.
- Maps are drawn before sprites, so sprites with the same `z` are on top.

`cargo run --release --example tiles`, or `MAP=4000` for 16 million tiles.

### Animated, flipped and parallax tiles

A map can animate a tile: every cell showing it cycles through frames, each
for its own time, with no system of your own. Cells can be mirrored, and a
map can scroll slower than the world, for far-away layers:

<div class="lang-pair">
<div>

```rust
# use ponko::tilemap::TileMap;
# let mut map = TileMap::new(SpriteSheet::new(ctx.gfx.load_texture("levels/meadow.png"), 16, 16), 40, 15);
map.parallax = vec2(0.3, 0.3);                      // far away: scrolls slowly
map.animate(4, &[(4, 0.4), (5, 0.4)]);             // tile 4 shows 4, 5, 4, 5, ...
map.set_flipped(3, 2, Some(7), TileMap::FLIP_X);   // a mirrored tile
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:tile_extras}}
```

</div>
</div>

- `parallax` is how much of the camera's movement the map follows: `(1, 1)`
  moves with the world (the default), `(0, 0)` stays on the screen.
  `parallax_origin` is the camera center at which the map sits at its own
  position.
- Flips are `FLIP_X`, `FLIP_Y` and `FLIP_DIAGONAL` (swap the axes first, as
  Tiled does); combined, they give every rotation by 90°.
- Maps without animations or flips draw exactly as before; the others cost a
  little more per visible tile.

## Levels from Tiled and LDtk

Levels made in [Tiled](https://www.mapeditor.org/) (`.tmx`, `.tmj`, with
embedded or external `.tsx` / `.tsj` tilesets and `.tx` templates) and
[LDtk](https://ldtk.io/) (`.ldtk`, with separate level files too) spawn with
one call:

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/level.rs:spawn}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:level_spawn}}
```

</div>
</div>

`spawn_level` returns a root entity (with a `LevelRoot`) and spawns:

- every tile layer as `TileMap`s, one per tileset, with the tileset's
  texture, animated and flipped tiles, the layer's offset, parallax,
  opacity and tint, and a `LevelLayer` with its name and properties. Layers
  are stacked by `z`, bottom to top; LDtk's auto-layers and IntGrid tiles
  too. Image layers become one-tile maps.
- every object (Tiled) or entity (LDtk) as an entity with a `d2::Transform` at
  its center and a `LevelObject`: its name, class, layer, size, rotation,
  shape (rectangle, ellipse, point, polygon, polyline), tile and custom
  properties. Objects that show a tile get a `Sprite`.
- with the `physics` feature, fixed colliders for the solid tiles, merged
  into few rectangles, and for the collision shapes drawn on tiles.

Objects are plain data until you give them meaning, by their class:

<div class="lang-pair">
<div>

```rust
# use ponko::tilemap::LevelObject;
let mut coins = 0;
for (t, o) in ctx.world.query::<(&ponko::d2::Transform, &LevelObject)>().iter() {
    if o.class == "Coin" {
        coins += o.properties.i64("value").unwrap_or(1);
        let _at = t.position;
    }
}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:level_objects}}
```

</div>
</div>

`LevelOptions` changes what is spawned (`spawn_level_with`):

```rust
# use ponko::tilemap::LevelOptions;
let options = LevelOptions::default()
    .prefab("Chest", "prefabs/chest.json") // objects of class Chest are this prefab
    .solid("Walls")                         // a layer, IntGrid value, tile class or tag that collides
    .level("Level_1")                       // LDtk: which level of the project
    .at_z(2.0);                             // the objects' layer is at z 2
# let _ = options;
```

- Prefab objects are spawned with `spawn_prefab` at the object's place, with
  their `LevelObject` added; an object's `prefab` property names one too.
- Solid by default: layers named `collision` or `collisions`, layers and
  tiles with a boolean `solid` or `collides` property, and tiles with
  collision shapes. `without_colliders()` spawns none.
- `ctx.despawn_level(root)` removes everything a level spawned.
- `without_saved_objects()` spawns none of the prefab objects, for
  [loading a save](#levels-and-saves).
- `ctx.assets.load_level(path, &options)` reads a level without spawning it,
  for tools or your own spawning: its tilesets, layers, objects and
  properties as plain data (`Level`).

### Levels and saves

The level file can rebuild its tiles, colliders and plain objects, so world
saves leave them out: `spawn_level` marks everything it spawns with
`NotSaved` (`ponko::reflect::NotSaved`; give it to your own entities too),
except the objects it spawns as prefabs. Those are the game's state, a
chest opened or a coin collected, and the save holds them. To load, load the
save, then spawn the level around it without the prefab objects:

<div class="lang-pair">
<div>

```rust
# use ponko::tilemap::LevelOptions;
let options = LevelOptions::default().prefab("Chest", "prefabs/chest.json");
ctx.spawn_level_with("levels/meadow.tmx", Vec2::ZERO, options.clone()).expect("level");
// ... play ...
ctx.storage.save_world(&ctx.world, "meadow").expect("saved"); // the chests and the player

// Later, or in the next run:
ctx.load_world("meadow").expect("loaded"); // the saved chests and player, as they were...
let options = options.without_saved_objects(); // ...and the level around them
ctx.spawn_level_with("levels/meadow.tmx", Vec2::ZERO, options).expect("level");
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:level_saves}}
```

</div>
</div>

In C, `ponko_ctx_spawn_level_without_saved_objects` spawns a level that way,
and `ponko_world_set_saved` / `ponko_world_is_saved` (C++ `world.set_saved`,
`world.saved`) mark any entity. The platformer [Ember Hop](ember-hop.md)
saves its runs like this.

With [hot reload](drawing.md#hot-reload) on, saving the level in the editor (or a
tileset or template it uses) spawns it again in place; a file that doesn't
load keeps the old level and prints why. Things your game spawned on top
(the player) stay.

Not supported yet: isometric, staggered and hexagonal maps, and Tiled's
zstd compression. The editors' own painting rules (Tiled's Wang sets,
LDtk's auto-layer rules) don't run in the engine: the tiles they painted,
which the editors save, are spawned.

## Autotiling and colliders for maps built in code

An `AutoTile` rule picks each tile's frame from its filled neighbours, so a
wall drawn cell by cell joins up by itself. `AutoTile::four` takes 16 frames,
one per mask of the four sides (N = 1, E = 2, S = 4, W = 8);
`AutoTile::blob` takes the 47 frames of an 8-neighbour "blob" set, and
`AutoTile::eight` a function from the 8-neighbour mask.

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/level.rs:walls}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:autotile}}
```

</div>
</div>

- `map.autotile(&rule, |x, y| filled)` sets the whole map at once;
  `map.paint_autotile(&rule, x, y, on)` fills or clears one cell and fixes
  its neighbours.
- `map.solid_rects(|tile| solid)` merges the solid cells into few
  rectangles (map pixels, from its top-left); with `physics`,
  `map.colliders(position, |tile| solid)` gives a fixed body per rectangle,
  ready for `spawn_batch`. Spawn them again after changing the map.

`cargo run --release --example level --features physics` shows all of it.
