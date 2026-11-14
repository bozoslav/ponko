# Shard Grove: a 3D game, start to finish

[`examples/shard_grove`](https://github.com/ponkoengine/ponko/tree/main/examples/shard_grove)
is the engine's 3D reference game. The other 3D examples each show one
system; Shard Grove puts them together in one small third-person game: a
glTF level that is drawn, solid and walkable for the enemies, skinned glTF
characters whose clips are states of an animation state machine, a character
controller, enemies on a navigation mesh, positional sound, a sun with
shadows and point lights, menus and a HUD, a UI panel in the world, quality
presets, and saves. It packages with `ponko package` and builds for the web.

You guide a warden through a walled grove. Six shards are hidden in its
corners, on a platform and behind broken walls; crawlers patrol the grove and
chase you when you come close. Their touch costs a heart (three and the run
is lost); a pulse stuns every crawler around you for a few seconds. The two
shrines save the game when you walk up to them. Once every shard is found,
the gate in the north wall opens: walk through it to win.

```sh
cargo run --release --example shard_grove --features navmesh,physics3d
cargo test --test shard_grove --features navmesh,physics3d   # 19 headless tests
```

| Action | Keyboard | Gamepad |
|---|---|---|
| Move (relative to the view) | WASD, arrow keys | left stick |
| Turn the view | Q / E | right stick |
| Jump | Space | South |
| Pulse | F, Left Shift | West |
| Pause | Esc, P | Start |
| Menus | mouse, or Tab / arrows and Enter; Esc goes back | |

## How the code is laid out

`main.rs` only opens the window. Everything else is in `game.rs`, as five
plugins gathered by `ShardGrove`, so the tests build the same game without a
window:

1. `Controls`: a frame system turns actions into an `Intent` resource, and
   another turns clicks on the menus' buttons into a `Menu` choice on it.
2. `Rules`: one sim system is the whole game (moving the warden, the pulse,
   hits, shards, shrines, the gate, the menus' effects). A second moves the
   crawlers on the navigation mesh while the game is played. They read only
   the `Intent` and the world, so a test writes the `Intent` itself.
3. `Presentation`: the camera behind the warden, the shards' spin, the
   gate's glow, the crawlers' hum. None of it changes the game.
4. `Screens`: the title, pause, settings and end menus and the HUD, built
   once with the [retained UI](ui.md) and shown as the game's `Screen`
   resource says, and the sign over the gate.
5. `Ambience`: the wind, looping on its own mixer bus.

This is the layout of [Ember Hop](ember-hop.md), the 2D reference game; the
3D parts are below.

## One glTF file: drawn, solid, walkable

The level is one glTF file (`assets/shard_grove/grove.glb`). It is drawn
with `load_mesh`, made solid with `load_collision_mesh` (the warden walks on
exactly what you see), and its triangles, read on the CPU with
`load_mesh_data`, are baked into the crawlers' navigation mesh
([3D navigation](navigation3d.md)):

```rust
{{#include ../../examples/shard_grove/game.rs:level}}
```

The bake uses the crawlers' size (0.45 m radius, 1.8 m tall); the platform's
ramp is gentle enough to walk up, the walls and pillars are holes in the
mesh. The tests ask the mesh for a path from the start to every shard, both
shrines and the gate, so a level edit that walls something off fails a test.

## Characters and their state machines

The warden and the crawler are skinned glTF characters
([Skinned meshes](3d.md#skinned-meshes)): `load_mesh` draws them, `load_skeleton`
gives each a skeleton to pose, and `load_clip` reads their clips by name.
The clips are the states of an `AnimationGraph`
([Animation state machines](3d.md#animation-state-machines)); the rules only set its
parameters, and the transitions pick the clip:

```rust
{{#include ../../examples/shard_grove/game.rs:warden_graph}}
```

Each warden and crawler gets its own `Animator` (sharing the graph through
an `Arc`) and a clone of the skeleton; the engine advances the animators in
the frame's animation step. A crawler's graph has idle, crawl and stunned,
switched by two booleans. A glTF file's skeleton holds every node of the
file, so the mesh's own node is the first of the warden's nine joints.

## The warden and the crawlers

The warden is a [`Character`](physics3d.md) controller: the rules set its
velocity from the stick, turned by the view's yaw so that "forward" is
where the camera looks, and the physics step moves it along the walls and up
the ramp. The camera follows from behind and above.

The crawlers are `NavAgent`s. Each walks between two points of its route
until the warden comes within 8 m, then chases (re-planning only when the
warden has moved half a metre), and gives up beyond 12 m. A stunned crawler
stops. The navigation step runs only while the game is played, so the
crawlers hold still on the pause menu:

```rust
{{#include ../../examples/shard_grove/game.rs:crawlers}}
```

## Sound and light

Each crawler carries an `AudioSource`, a looping hum that is loud close by
and silent 14 m away ([Audio](audio.md)); the listener follows the camera.
Shards, the pulse, hits, shrines and the gate play where they happen with
`play_at`. The wind loops on an `ambience` bus; the volume setting turns
both buses.

A low sun casts shadows over the grove; each shard and shrine has a point
light (the shrines' cast shadows where the preset allows), and the gate
lights up when it opens.

## Menus, the HUD and a sign in the world

The menus are retained UI screens, one per `Screen`; a late system keeps the
screen stack, the HUD and the texts in step with the game, so the rules
never touch the UI. Escape on a menu pops it, which the game reads as
"back" (or "resume"). The sign over the gate is a `UiPanel`: retained UI
drawn into a texture on a quad in the world
([UI on surfaces in the world](ui.md#ui-on-surfaces-in-the-world)),
counting the shards and saying when the gate is open.

## Quality presets and settings

The settings menu cycles through the four `GraphicsQuality` presets
([Graphics quality](3d.md#graphics-quality)) and the volume, and keeps both in
`settings.json`. Until the player picks a preset, the game asks for the one
recommended for the GPU, `Low` on the web:

```rust
{{#include ../../examples/shard_grove/game.rs:settings}}
```

## Saves

Walking up to a shrine writes the run (the shards found, the hearts left,
the time and the shrine) to `save.json` in `ctx.storage`
([Saves and settings](storage.md)). "Continue" reads it and starts the run
beside that shrine. A file that can't be read, or holds a run that can't be
(a seventh shard, a shrine that doesn't exist), is set aside as
`save.json.bad` and the title says so. Winning deletes the save and records
the best time and the wins in `records.json`.

## Tests without a window

`tests/shard_grove.rs` includes the game's module and plays it with a script
that writes the `Intent` before the rules run:

```rust
{{#include ../../tests/shard_grove.rs:script}}
```

The tests check the level's paths, the characters' joints and clips, walking
relative to the view, the state machines (with headless frames, so the
animators run), crawlers that chase and hurt, the pulse and its cooldown,
losing and trying again, the gate, saving and continuing, damaged saves, the
settings, pausing, the menus by keyboard, the sign, and the crawlers' hum.
The last one lets an autopilot play the whole game:

```rust
{{#include ../../examples/shard_grove/smoke.rs:autopilot}}
```

It follows paths on the crawlers' own navigation mesh, which the warden can
walk too, and pulses when a crawler comes close. A whole game takes it about
2,000 ticks (40 seconds of play).

## Packaging and the smoke test

The root `Cargo.toml` describes the package in
`[package.metadata.ponko.examples.shard_grove]`: its name, id, the
`assets/shard_grove` folder, the licenses, `CONTENT-NOTICES.txt` and a readme
for players. `ponko package --example shard_grove --features
navmesh,physics3d` builds it, runs its own smoke run and archives it.
`shard_grove --smoke headless|window [--report report.json]` plays the
walk-through: a new game, three shards, a shrine, a pause and back to the
title, "continue", the other shards, the gate, and back to the title. It
writes what it saw as JSON and exits with 0 only if it got to the end.

## The web build

```sh
web/build.sh --features navmesh,physics3d shard_grove && python3 -m http.server -d target/web/shard_grove 8000
```

builds it for the browser at the `Low` preset. The game publishes its state
on `<html data-smoke>` (ticks, frames, the warden's joints, the screen, the
shards, the quality, the storage), in the format of the
`web/smoke` fixtures, and the [web gallery](web.md#the-gallery-of-examples)
lists it.

## Gap list

What the game found missing or awkward, and what is not covered yet:

| Gap | Status |
|---|---|
| 3D physics has no pause like the 2D `ctx.physics.set_paused`; the game holds the warden's velocity at zero on menus instead. | Recorded on [#375](https://github.com/ponkoengine/ponko/issues/375) |
| `NavAgent` has private fields, so it can't be built with `..Default::default()`; the game sets fields on a default. | Recorded on [#375](https://github.com/ponkoengine/ponko/issues/375) |
| A frame-rate performance baseline per GPU tier (`PONKO_BENCH`) needs runs on real GPUs with a window. | Open |
| CI doesn't package and play it on every OS yet, as it does Ember Hop; its headless tests run in the test job on Linux, macOS and Windows. | Open |
| One level: level streaming isn't part of the game. | Open |
| Rust only; no C or C++ counterpart. | No native port planned yet |

## Assets and licenses

The level, both characters with their clips and every sound were made for
this game and are under the repository's MIT or Apache-2.0 license, like the
code. `examples/shard_grove/CONTENT-NOTICES.txt` lists each file, and a test
checks that the list matches what the package ships.
