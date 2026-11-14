# Ember Hop: a platformer, start to finish

[`examples/ember_hop`](https://github.com/ponkoengine/ponko/tree/main/examples/ember_hop)
is the engine's second reference game. [Coin Dash](packaging.md) draws
everything in code; Ember Hop is built from the engine's content features:
levels made in the [Tiled](https://www.mapeditor.org/) map editor, prefabs, a
scene file, 2D physics, animated sprite sheets, particles, sound files and
streamed music, a camera that follows the hero, saves in save slots, and the
keyboard and a gamepad through named actions. It packages with `ponko
package` and builds for the web, and CI plays it from its package.

You carry an ember spirit through two caves: collect sparks, burn drips with
a flare or stomp them, light braziers (each one saves the run) and reach the
lantern at the end of each cave. Falling, spikes, water or a drip take you
back to the last brazier.

```sh
cargo run --release --example ember_hop --features physics
cargo test --test ember_hop --features physics       # 22 headless tests
```

| Action | Keyboard | Gamepad |
|---|---|---|
| Move | arrow keys, A / D | left stick, d-pad |
| Jump (hold for higher) | Space, Up, W, Z, K | South |
| Flare | X, J, Left Shift | West, right bumper |
| Pause | Esc, P | Start |
| Title: new run / continue | Enter / C | South / North |
| Leave (paused) | Q | East, Select |

## How the code is laid out

`main.rs` only opens the window. Everything else is in `game.rs`, as four
plugins, so the tests can build the same game without a window:

1. `Controls`: a frame system turns actions into an `Intent` resource (what
   the player wants this tick).
2. `Rules`: one sim system, after the physics step, is the whole game. It
   reads only the `Intent`, the world and the physics results, so a test can
   write the `Intent` itself and get the same game on every run.
3. `Presentation`: the camera, the glows, the fade between levels, the text.
   None of it changes the game.
4. `Music`: the music loop, started once.

The input side names actions once and binds keys and buttons to them (see
[Actions](actions.md)):

```rust
{{#include ../../examples/ember_hop/game.rs:controls}}
```

Presses are latched into the `Intent` by the frame system, and the rules use
them up, so a press between two ticks is never lost:

```rust
{{#include ../../examples/ember_hop/game.rs:read_input}}
```

The rules are a small state machine over the screens (title, playing, paused,
dying, the fade between levels, the end):

```rust
{{#include ../../examples/ember_hop/game.rs:rules}}
```

## Levels from Tiled

Each cave is a Tiled map (`assets/ember_hop/levels/*.tmx`) with one tileset
(`cave.tsx`, whose solid tiles have a collider and whose water tiles are
animated). The layers are a tinted backdrop, the ground, decoration, spikes,
water, and an object layer, `Things`, with points for the hero's start, the
sparks, drips, braziers and the way out, and rectangles for the hazards. The
map's properties hold the cave's title.
[`ctx.spawn_level_with`](tiles.md#levels-from-tiled-and-ldtk) spawns all of it: a `TileMap` per tile
layer, the colliders of the solid tiles merged into few rectangles, and an
entity per object; objects whose class has a prefab become that prefab. The
ground layer is painted with one rock tile, and autotiling picks each cell's
edge and moss from its four neighbours when the level spawns:

```rust
{{#include ../../examples/ember_hop/game.rs:spawn_level}}
```

The hazard rectangles get a fixed sensor collider in code. LDtk levels spawn
through the same call; this game uses Tiled only.

## Prefabs

Sparks, drips, braziers and the way out are prefab files
(`assets/ember_hop/prefabs`). The way out is a prefab with children: a post
with its sensor, a lantern attached to it, and an ember emitter attached to
the lantern:

```json
{{#include ../../assets/ember_hop/prefabs/exit.json}}
```

The game's own components (`Spark`, `Drip`, `Checkpoint`, `Exit`, `Player`)
are reflected, so prefab files can hold them and saves keep them.

## Physics

The hero is a dynamic body with its rotation locked; the rules set its
horizontal speed, and gravity (900 pixels per second squared) does the rest.
Two short rays down from its feet say whether it stands on the ground. A jump
works a moment after running off a ledge (coyote time), a press just before
landing is kept (a jump buffer), and letting go early cuts the jump short.
Sparks, drips, hazards, braziers and the way out are sensors; their
`CollisionStarted` events are the game's rules. The flare is
`ctx.physics.overlap_circle` around the hero; landing on a drip from above
stomps it and bounces. Drips are kinematic bodies that turn at walls and
ledges, found with rays.

The pause screen holds only the physics step: a system before the step calls
`ctx.physics.set_paused` unless the hero is playing, dying or between levels
([Pausing only physics](sim-clock.md#pausing-only-physics)).
`ctx.time.pause()` would also stop the rules that read the unpause.

## Saves, the title scene and progress

- **The title** is a scene file, `assets/ember_hop/scenes/title.json`, loaded
  with `ctx.load_scene`: some ground, the hero, a lit brazier, sparks and the
  lantern post.
- **The run** is saved in the save slot `run` with
  `ctx.storage.save_world` when a level starts and when a brazier is lit. A
  loss, and "continue" on the title, load it with `ctx.load_world`. The game
  keeps the run's numbers (`Run`, a reflected resource), its own components
  and the engine's (sprites with their animations, bodies, particle emitters)
  in the save. The level isn't in it: `ctx.spawn_level_with` marks its tiles,
  colliders and plain objects `NotSaved`, so the save holds only the hero and
  the prefab objects (sparks still to collect, drips, braziers, the way out)
  as they were. After a load, `rehydrate` spawns the level again with
  `LevelOptions::without_saved_objects`, which leaves out the objects the save
  brought back, and starts the hero from rest.
- **Animations** are in the files: the spark, drip and way-out prefabs and
  the title scene give their sprites an `"Animation"` next to the `"Sprite"`,
  so they spawn animating and saves keep each one's frame.
- **Progress** (runs finished, best time, most sparks) is a small versioned
  JSON file written with `ctx.storage.write_json`. A damaged file is set
  aside and the game starts fresh, with a note on the title.

## Camera, particles and sound

The camera zooms in three times and eases toward a point ahead of the hero,
clamped so it never shows outside the level:

```rust
{{#include ../../examples/ember_hop/game.rs:camera}}
```

The hero leaves a trail of embers, lit braziers burn, and sparks, flares,
stomps and losses throw short bursts (`ParticleEmitter` with additive
blending). Sprites animate from sprite sheets (`Animation`); the hero's sheet
has idle, run, jump and fall frames, and it faces left with a negative
x scale ([#440](https://github.com/ponkoengine/ponko/issues/440)). Sound
effects are WAV files loaded with `ctx.load_sound`; the music is an Ogg
Vorbis loop streamed with `ctx.audio.load_stream`.

## Tests without a window

`tests/ember_hop.rs` includes `game.rs` and plays it headless. Most tests
write the `Intent` from a sim system that runs before the rules:

```rust
{{#include ../../tests/ember_hop.rs:script}}
```

They cover running and facing, a full and a short jump, collecting a spark,
the flare and the stomp, losing to a drip, spikes, water and a fall,
braziers saving and a loss loading, the way out to the next level, the end
of the run (progress recorded, its save deleted), continuing from the title,
pausing, the camera, and a damaged progress file. One test drives the game
with real key and gamepad events through headless frames
([Headless runs and testing](headless.md)):

```rust
{{#include ../../tests/ember_hop.rs:input_test}}
```

And one plays the whole game with an autopilot: it runs right, flares at
drips close ahead, and jumps at walls and gaps it finds with rays. The two
caves are built so that this simple player gets through them:

```rust
{{#include ../../examples/ember_hop/smoke.rs:autopilot}}
```

## Packaging and the smoke test

The root `Cargo.toml` describes the package in
`[package.metadata.ponko.examples.ember_hop]`: its name, id, the
`assets/ember_hop` folder, the licenses, `CONTENT-NOTICES.txt` and a readme
for players. `ponko package --example ember_hop --features physics` builds
it, runs its own smoke run, and archives it. `ember_hop --smoke
headless|window [--report report.json]` plays the walk-through: a new run,
the first cave with the autopilot, a pause and back to the title in the
second cave, "continue", the rest of the run, and the end screen. It writes
what it saw as JSON and exits with 0 only if it got to the end.

CI's `package-smoke` job does this on Linux, macOS and Windows: it packages
the game, unpacks it in a folder elsewhere, and runs the walk-through from
the package without a window and in one (under `xvfb-run` on Linux),
checking the report (both caves cleared, the saved run loaded, one run
finished) and that the progress file was written. The `test` job runs the
headless tests on every OS with the physics feature (`physics-determinism`).

## The web build

```sh
web/build.sh --features physics ember_hop && python3 -m http.server -d target/web/ember_hop 8000
```

builds it for the browser, and CI builds it in the `web-smoke` job. Like
Coin Dash, the game publishes its state on `<html data-smoke>` (ticks,
frames, the hero picture's size, the screen, the cave, the sparks, the
storage), so the browser runner of `web/smoke` can drive and check it, and
the [web gallery](web.md#the-gallery-of-examples) lists it with the other examples. In a browser the
music starts
at the player's first click or key press ([Audio](audio.md#in-a-browser)).

## Gap list

| Gap | Where it is tracked |
|---|---|
| Sprites have no flip; the hero uses a negative scale. | [#440](https://github.com/ponkoengine/ponko/issues/440) |
| Rust only; no C or C++ counterpart. | No native port planned yet |

## Assets and licenses

Every picture, level, sound and the music were made for this game and are
under the repository's MIT or Apache-2.0 license, like the code.
`examples/ember_hop/CONTENT-NOTICES.txt` lists each file, and a test checks
that the list matches what the package ships.
