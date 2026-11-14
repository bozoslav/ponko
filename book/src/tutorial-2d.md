# Tutorial: a first 2D game

This tutorial builds **Crab Cove**, a small platformer, from an empty
project: a crab walks and jumps through a level made in a level editor,
collects coins with a sound, reaches a flag, and the game remembers its best
score. At the end you test it without a window, package it for players and
build it for the browser.

**Rust only.** The tutorial is written in Rust. The engine's C and C++ API
covers the same features ([C and C++](c-cpp.md)), but this walkthrough has
no C++ version yet.

Every piece of code below is pulled from the finished game,
[`examples/tutorial_2d`](https://github.com/ponkoengine/ponko/tree/main/examples/tutorial_2d)
in the engine's repository, which CI compiles and
[`tests/tutorial_2d.rs`](https://github.com/ponkoengine/ponko/blob/main/tests/tutorial_2d.rs)
plays. If something doesn't work for you, compare with those files.

The game is built in five steps, and each step is one
[plugin](systems.md#plugins): a small struct that adds its systems to the
game and sets up what it needs. A step only adds code, so the game builds
and runs after every step.

## Step 0: a new project

Install the `ponko` command and make a 2D project
([Getting started](getting-started.md#a-new-project) has the details):

```sh
cargo install ponko_cli
ponko new crab_cove
cd crab_cove
cargo run
```

A window opens with an orange square you can move with the arrow keys.
This tutorial needs Ponko 0.2; until it's released, make the project with
`ponko new crab_cove --git https://github.com/ponkoengine/ponko`, which
depends on the engine's `main` branch.

The crab falls and collides, so turn on 2D physics in `Cargo.toml`:

```toml
[dependencies]
ponko = { version = "0.2", features = ["physics"] }
```

(With `--git`, keep the `git = ...` the template wrote and add the
`features`.)

**The art.** Copy the folder
[`assets/tutorial_2d`](https://github.com/ponkoengine/ponko/tree/main/assets/tutorial_2d)
from the engine's repository into your project's `assets/`, so you have
`assets/tutorial_2d/cove.tmx` and its neighbours: a level, its tileset
(`cove.tsx` and `tiles.png`), the crab (`crab.png`) and two sounds
(`coin.wav`, `flag.wav`). They were made for this tutorial and are under the
engine's license (MIT or Apache-2.0); use them however you like.

**Start from a clean `src/lib.rs`.** The template's square, its `Settings`
and `assets/settings.json` aren't used here: delete `src/lib.rs`'s contents
and `assets/settings.json`. Put this at the top of the empty `src/lib.rs`:

```rust
{{#include ../../examples/tutorial_2d/game.rs:use_level}}
```

`ponko::d2` holds the 2D names: `Transform`, `Sprite`, `RigidBody`,
`Collider` and the tilemap types.

## Step 1: sprites and a level

The whole game is a function that returns a `Game`. Here it is as it will
look at the end; for now, add it with only the first plugin,
`.plugin(Level)`, and add each other line when you reach its step:

```rust
{{#include ../../examples/tutorial_2d/game.rs:game}}
```

Like the template's, it returns a `ConfiguredGame`, a game ready to
`run()`: `.configured()` says the plugins hold all of its setup. So
`src/main.rs` stays as the template wrote it, calling `game().run()` (or
`run_headless` without a window), and the tests in
[step 6](#step-6-test-it) add their settings to the same `game()`.

Now the level and the crab:

```rust
{{#include ../../examples/tutorial_2d/game.rs:level}}
```

What happens here:

- **`ctx.spawn_level`** reads the Tiled map and spawns everything in it:
  each tile layer becomes a `TileMap` (drawn by the engine), each object an
  entity with a `LevelObject` that keeps its class and properties. The
  tileset marks the grass, dirt and stone tiles with a `solid` property, so
  with physics on they also become fixed colliders, merged into a few
  rectangles. [Levels from Tiled and LDtk](tiles.md#levels-from-tiled-and-ldtk)
  has the whole list.
- **The crab is a sprite** (`Sprite::new(texture)` at a `Transform`), and a
  physics body: a dynamic `RigidBody` that gravity pulls down, a `Collider`
  12 by 10 pixels, and a `Velocity` that the next step sets. `lock_rotation`
  keeps it upright ([Physics](physics.md)).
- **`.physics(vec2(0.0, 900.0))`** turns physics on with gravity in pixels
  per second squared. In 2D, y points down.
- **The camera** shows the world at twice the size (`zoom`) and follows the
  crab in a frame system ([Camera, text and UI](camera-ui.md)).
- `Stage` is a resource that remembers what is on stage, so `start_level`
  can remove it to start over.

`cargo run`: the cove appears and the crab drops onto the grass. To change
the level, open `cove.tmx` in [Tiled](https://www.mapeditor.org/); add
`.hot_reload(true)` to `game()` and saving it in Tiled updates the running
game.

## Step 2: input actions

Instead of asking for keys in game code, name what the player can do and
bind keys and gamepad buttons to the names once
([Named input actions](actions.md)):

```rust
{{#include ../../examples/tutorial_2d/game.rs:bindings}}
```

`walk` is an axis from -1 to 1, fed by the arrow keys, A and D, and a
gamepad's left stick. `jump` and `menu` are buttons. `actions!` makes `Act`,
a struct of ids looked up once, so a typo in a name is reported at startup
instead of reading false forever.

The plugin installs the bindings and moves the crab:

```rust
{{#include ../../examples/tutorial_2d/game.rs:controls}}
```

- `action_axis` is the axis now; `action_pressed` is true only in the frame
  the button went down, so holding Space jumps once.
- `control` is a **frame system** (`.system`), so it sees every press, even
  in a frame with no simulation tick. It only sets the velocity; the physics
  step moves the crab.
- The crab may jump when a short ray straight down (`ctx.physics.cast_ray`,
  leaving out the crab itself) finds ground under its feet.

Add `.plugin(Controls)` to `game()` and `cargo run`: the crab walks, jumps,
lands on the platforms and stops at the stone walls. The `menu` action is
used in step 4.

## Step 3: collisions and sound

Coins and the flag are objects in the level. This step gives each one a
**sensor**, a collider that reports overlaps and doesn't push, and reacts
when the crab touches one:

```rust
{{#include ../../examples/tutorial_2d/game.rs:pickups}}
```

- `make_sensors` runs before the physics step
  (`.add(Phase::BeforePhysics, ...)`) and gives every `Coin` or `Goal`
  object that has no collider yet a fixed body and a sensor. Because it looks
  for objects without one, it also covers a level that was just spawned
  again.
- The physics step sends a **`CollisionStarted` event** when two colliders
  start touching ([Collisions](physics.md#collisions)). `touch` runs after
  the step (`sim_system`), reads the new events, and keeps the ones between
  the crab and a sensor.
- A coin is despawned and counted, and `ctx.play_sound` plays its sound,
  loaded once at startup with `ctx.load_sound` ([Sound](audio.md)).
- The flag sends a `LevelDone` event of our own
  ([Events](events.md)): nothing here needs to know who reacts to it. Steps
  4 and 5 do. Then the level starts over.
- Falling into the water (below the level) starts over too, and `hud` draws
  the count with `ctx.draw_text`, in window pixels on top of the world.

Add `.plugin(Pickups)` and play: coins chime and disappear, the flag plays a
tune and puts you back at the start.

## Step 4: a menu

A title screen with Play and Quit, made with the [retained UI](ui.md).
Add the UI names to the `use` lines at the top:

```rust
{{#include ../../examples/tutorial_2d/game.rs:use_menu}}
```

and the plugin:

```rust
{{#include ../../examples/tutorial_2d/game.rs:menu}}
```

- The setup builds the screen once: a centered root with a title, two
  labels and two buttons, laid out in a column. `push_screen` shows it. The
  nodes get **names** (`menu.play`), which tests and tools use to find and
  click them.
- `.focused()` makes Play the node the screen opens on, every time it is
  pushed: Enter (or a gamepad's South button) starts the game at once
  ([Where the focus starts](ui.md#where-the-focus-starts)).
- `menu` takes the UI's events: a click on Play closes the screen and starts
  the level over, Quit ends the game. Escape (or a gamepad's East button)
  closes the top screen by itself and sends `ScreenPopped`.
- Each `LevelDone` puts the result on the screen and opens it; the `menu`
  action (Escape) opens it while playing.
- **The game waits while the screen is open**: `ctx.time.pause()` stops the
  simulation, physics included, and frame systems (the UI, the HUD) keep
  running ([The simulation clock](sim-clock.md)).

The mouse clicks the buttons; Tab or the arrow keys pick another and Enter
presses it. Add `.plugin(Menu)`.

## Step 5: saving

The best score goes in the player's own storage
([Saves and settings](storage.md)): a folder of the game's own on the
desktop, the browser's storage on the web. Add serde to the `use` lines
(the template's `Cargo.toml` already depends on it):

```rust
{{#include ../../examples/tutorial_2d/game.rs:use_saves}}
```

and the plugin:

```rust
{{#include ../../examples/tutorial_2d/game.rs:saves}}
```

- `app_id` names the game's folder (`crab-cove`); pick it once and keep it.
- `ctx.storage.read_json` reads the file at startup. A missing file is a
  first run, so the default (zero) is right; `unwrap_or_default` treats a
  damaged file the same way. [Saves and settings](storage.md#reading-and-writing)
  shows how to keep a damaged file and tell the player.
- `write_json` writes safely: a crash in the middle never leaves half a file
  ([Safe writes](storage.md#safe-writes)).
- `save_progress` reads the same `LevelDone` events as the menu: every
  system sees each event once.

Add `.plugin(Saves)`. Reach the flag, quit, start the game again: the title
screen shows your best.

## Step 6: test it

The game runs without a window too, as fast as the CPU allows, which makes
it testable in plain `cargo test` ([Headless runs and testing](headless.md)).
`headless_frames(true)` runs frame systems and the UI as well, and an
[input scenario](actions.md#input-scenarios) presses keys and clicks buttons
on given ticks. Both are set on `game()` itself, which stays as it is, and
`run_headless_scenario` runs it with them. Replace `tests/game.rs` with
tests like these; it starts with `use crab_cove::*;` and:

```rust
{{#include ../../tests/tutorial_2d.rs:uses}}
```

Then a helper that plays a scenario, and the first tests:

```rust
{{#include ../../tests/tutorial_2d.rs:play}}
```

Saving and loading, with the storage in a folder of the test's own
(`data_dir`), so tests never touch the player's real saves:

```rust
{{#include ../../tests/tutorial_2d.rs:save_test}}
```

The `play_moved` helper it uses puts the crab at given places on given
ticks; it and the other tests (walls, jumping, coins, the flag, the water,
the menu) are in
[`tests/tutorial_2d.rs`](https://github.com/ponkoengine/ponko/blob/main/tests/tutorial_2d.rs).
`cargo test` runs them all in a fraction of a second.

## Step 7: package it, and put it on the web

`Cargo.toml`'s `[package.metadata.ponko]` table says what players get. Give
the game its name and an id:

```toml
[package.metadata.ponko]
name = "Crab Cove"
id = "com.example.crab-cove"   # a reverse-DNS id you own
assets = ["assets"]
```

Then:

```sh
ponko package
```

builds a release, collects `assets/`, the license files and the notices of
every library linked in, and packs it for this computer (a zip on Windows,
an app in a dmg on macOS, a tar.gz on Linux) in `target/package/`. It also
starts the packaged game once, headless, to check that it runs from
anywhere ([Packaging any game](packaging.md#packaging-any-game-ponko-package)).

For the browser:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock>
ponko package --web
python3 -m http.server -d target/package/<the folder it names> 8000
```

and open <http://localhost:8000>. The same code runs: the level and sounds
are fetched before setup, physics and input work as on the desktop, and the
best score is kept in the browser's storage
([Web builds](web.md)). Sound starts after the first key press or click, as
browsers require. Upload the folder to any static host (itch.io, GitHub
Pages).

## Where next

- More levels: make another map in Tiled and spawn it when the flag is
  reached; `LevelOptions` can turn objects into [prefabs](scenes.md).
- Animation: a [sprite sheet](tiles.md#sprite-sheets) for the crab's walk.
- A settings screen with volume and key rebinding: the reference game
  [Coin Dash](packaging.md#ui-scaling-rebinding-and-navigation) has one.
- A bigger platformer, start to finish: [Ember Hop](ember-hop.md).
- [Tutorial: a first 3D game](tutorial-3d.md) builds a 3D game the same way.
