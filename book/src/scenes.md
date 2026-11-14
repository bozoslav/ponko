# Scenes and prefabs

A **scene** is a whole world kept in a file: a level, a menu, a test
setup. A **prefab** is an entity (or a [tree of them](#prefabs-with-children)),
kept in a file and spawned as many times as you like: a tree, an enemy, a
cart with its wheels. Both live in
`assets/`, both are JSON (so they diff well in git), and the editor,
agents and your own code write them like any other file.

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/scenes.rs:scenes}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:scenes}}
```

</div>
</div>

- `load_scene` replaces the world with the scene's entities, which keep
  their ids, so components that refer to other entities still point at the
  right ones. Resources that are [reflected](reflection.md) load too.
- `spawn_prefab` puts the prefab's position at the point you give and keeps
  its saved rotation and scale. A prefab without a `d2::Transform` gets one.
  The file is read once, then kept in memory, so spawning a thousand trees
  reads it once.
- Only [reflected](reflection.md) components go in files: reflect your
  types before loading or saving. `Transform2D`, `Sprite`, `Animation` and
  the physics components are reflected already. A sprite names its image
  (`"texture": "crab.png"`), and loading the scene or prefab loads it; so
  does a [sprite-sheet animation](tiles.md#animation), which also keeps its
  frame.
- Entities with `NotSaved` (`ponko::reflect::NotSaved`) are left out of
  scenes and saves: what the game rebuilds from its own files, such as a
  [level's tiles and colliders](tiles.md#levels-and-saves).
  With 3D on, `d3::Transform` (saved as `"Transform3D"`) and `Model` are reflected too: a model names
  its mesh's glTF file (`"mesh": "tree.glb"`), loaded the same way. Scenes
  can also come from Unity ([Importing from Unity](unity.md)).
- Errors say what's wrong and change nothing: a missing file lists the
  folders searched, and a component that isn't reflected is named.

## Saving

The [inspector's editor](inspector.md#editing-build-levels-in-the-game)
saves and loads scenes and places prefabs with the mouse. From code:

`save_scene(path)` writes the world (its reflected components and
resources) and `save_prefab(entity, path)` writes an entity, without its
position, with its [children](#prefabs-with-children) if it has any. Both
write over the file in `assets/` if it's there, else create it (and its
folders). Web builds can't save files there; for the player's saves, use
save slots in [`ctx.storage`](storage.md#save-slots), which also work in the
browser.

```rust
# let palm = ctx.world.spawn((ponko::d2::Transform::default(),));
ctx.assets.save_scene(&ctx.world, "levels/beach.json")?;
ctx.assets.save_prefab(&ctx.world, palm, "prefabs/palm.json")?;
```

C++: `ctx.save_scene(path)` and `ctx.save_prefab(e, path)`, which throw on
errors like the rest of the wrapper. C: `ponko_ctx_load_scene`,
`ponko_ctx_save_scene`, `ponko_ctx_spawn_prefab` and
`ponko_ctx_save_prefab`.

## Prefabs with children

A prefab can hold a tree of entities: a cart with two wheels, a driver
on the cart and a hat on the driver. `spawn_prefab` spawns all of them and
returns the root; the root gets `PrefabParts`, the list of the others, and
`despawn_prefab` removes the root with its parts.

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/scenes.rs:prefab_tree}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:prefab_tree}}
```

</div>
</div>

- **Transforms are relative to the root.** A part's `Transform2D` (or
  `Transform3D`) in the file is in the root's space: spawning the cart
  turned, scaled or somewhere else puts the parts where they belong on it.
  Parts that are [attached](attachments.md) (`Attached`) then follow the
  root as usual; parts that aren't stay where they were spawned.
- **References follow the copy.** In the file each entity has an id, and
  components name each other by it: an `Attached` part's `to`, a 3D joint's
  bodies, a `SkinOf` or `AttachToJoint` skin target, or your own component
  holding an `Entity`. Every spawn gives the parts new ids, and every value
  typed `Entity` that names an entity of the prefab names the new copy.
  Plain numbers are left alone, and ids that aren't in the file stay as
  written.
- **Saving.** `save_prefab` on a root saves the tree: its `PrefabParts`
  and everything attached to them (`Attached`, `SkinOf`, `AttachToJoint`,
  and 3D joints between two of them), transforms made relative to the
  root. An entity with no children is still saved as a version-1 file.
- **Saved worlds** keep the trees: `PrefabParts` is reflected and a load
  keeps ids, so a scene saved with carts in it loads back with whole carts.
- **Errors change nothing**: a part with a component that isn't reflected,
  or a file that doesn't fit, spawns none of the tree.
- **Tools.** The editor's prefab palette places trees and its undo removes
  the parts too. Over the [control endpoint](control.md) and MCP,
  `spawn_prefab` returns the root and its `parts`, and `despawn_prefab`
  removes a tree. C: `ponko_ctx_prefab_parts` and
  `ponko_ctx_despawn_prefab`; C++: `ctx.prefab_parts(root)` and
  `ctx.despawn_prefab(root)`.

The example `cargo run --example prefabs` rolls three carts and scraps one
whole (`HEADLESS=1` runs it without a window).

### Prefabs that include prefabs

An entity of a prefab can be another prefab: give it `"prefab"` with that
file instead of all its components. Its `"components"` then change the
included prefab's root for this use, field by field, like the
inspector's edits: the cart's wheels are the `wheel.json` prefab, each with
its own place.

```json
{{#include ../../assets/prefabs/cart.json}}
```

What is decided, and what isn't (yet):

- Included prefabs nest to any depth up to 16; a prefab that includes
  itself, directly or through others, is refused.
- Changes ("overrides") apply to the included prefab's **root** only. To
  change one of its parts, change it in code after spawning (its id is in
  `PrefabParts`), or make a prefab file for that variant.
- The link is used when spawning only. A spawned tree doesn't remember the
  files it came from, so editing `wheel.json` changes carts spawned later,
  not those already in a world or a saved scene, and `save_prefab` writes an
  included prefab's entities into the file instead of the link.

## What a load replaces and what it keeps

A scene is a snapshot of the world, not a whole save of the game: it holds
entities with reflected components and reflected resources. Everything
else belongs to the game and lives outside the world, so a load treats it
by a fixed rule (the full table is on `Ctx::load_scene` in the API docs):

| State | After `load_scene` |
|---|---|
| Entities | The scene's, with the scene's ids. An old `Entity` may now name a different entity of the new scene. |
| Reflected resources | The scene's values; reflected ones it doesn't name, and every other resource (scores, tuning, a baked navigation mesh), stay. |
| Commands and events | Commands queued earlier in the running system are dropped; events stay readable (their entity ids are the old world's). |
| 2D and 3D physics | Old bodies leave at once; the scene's are built at the next physics step, from rest. Gravity and settings stay. |
| Crowd agents, 3D UI panels, skeletons | Components that aren't saved: gone with their entities. Old crowd slots are freed at the next navigation step, and the keyboard focus of a 3D panel is forgotten. |
| `Attached`, `SkinOf`, `AttachToJoint`, 3D joints, `PrefabParts` | Reflected: the scene's, naming the scene's entities. Skins and joint attachments show once their entity has a skeleton again. |
| Assets and prefabs | Stay cached; a load never unloads anything. |
| Positional sounds | A sound playing from a replaced entity stops; music and other sounds play on. |
| Camera, screen UI, lighting, input, time, random seed | Kept. Temporal effects (TAA) forget their history, and the inspector its selection. |
| Lockstep networking | Not synchronized: in a session, load in setup or a simulation system; other loads are refused (see below). |

A failed load (a missing file, a component that isn't reflected, an image
that can't be loaded) changes none of this; only images and meshes that
did load before the failure stay cached.

### Loading scenes in lockstep

In a [lockstep session](networking.md) every peer runs the same
simulation, so a load must happen on every peer at the same tick. Two
places guarantee that: setup, which every peer runs before the first tick,
and simulation systems, which every peer runs at the same ticks from the
same inputs. Load there with `load_scene`, or with `load_scene_lockstep`,
the same call named so the call site says it is part of the shared
simulation:

```rust
// In a simulation system: every peer reaches this tick with the same world.
if ctx.time.sim_tick() == 600 {
    ctx.load_scene_lockstep("levels/two.json")?;
}
```

Everywhere else a load would change one peer only, so while a session
connects or runs it is refused with an error and changes nothing: loads
from frame, late and draw systems, the inspector's Load button, the
editor's Stop, and the control endpoint's `scene.load` and `world.load`.
Offline, and after a session is lost, every load works as usual. The peers
still compare their worlds, so a load that only some peers make (one
behind an `if` on local state) shows up as a desync. C++:
`ctx.load_scene_lockstep(path)`; C: `ponko_ctx_load_scene_lockstep`, with
the same rules as `ponko_ctx_load_scene`.

## The files

A scene is the [save format](determinism.md) of `world.save()`:

```json
{{#include ../../assets/levels/beach.json}}
```

A prefab of one entity lists its components (format version 1):

```json
{{#include ../../assets/prefabs/palm.json}}
```

Fields left out take their defaults (here the palm's position and scale).
A prefab with children is version 2: an `"entities"` list, root first,
each with an `"entity"` id that the others' components use to name it
(the cart above). This engine reads both versions.
