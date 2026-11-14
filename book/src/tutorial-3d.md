# Tutorial: a first 3D game

This tutorial builds **Lantern Yard** from an empty project: a walled yard
loaded from a glTF file, a character you walk around it with a camera behind
it, lanterns that light up when you reach them, crates to push, and a HUD.
At the end you test it without a window and package it.

**Rust only.** The tutorial is written in Rust. The engine's C and C++ API
covers the same features ([C and C++](c-cpp.md)), but this walkthrough has
no C++ version yet.

Every piece of code below is pulled from the finished game,
[`examples/tutorial_3d`](https://github.com/ponkoengine/ponko/tree/main/examples/tutorial_3d)
in the engine's repository, which CI compiles and
[`tests/tutorial_3d.rs`](https://github.com/ponkoengine/ponko/blob/main/tests/tutorial_3d.rs)
walks. As in the [2D tutorial](tutorial-2d.md), each step is one
[plugin](systems.md#plugins) and only adds code, so the game builds and runs
after every step. Doing the 2D one first helps, but isn't needed.

## Step 0: a new project

```sh
cargo install ponko_cli
ponko new lantern_yard --3d
cd lantern_yard
cargo run
```

A window opens with a spinning cube. This tutorial needs Ponko 0.2; until
it's released, add `--git https://github.com/ponkoengine/ponko` to `ponko
new`, which depends on the engine's `main` branch.

The game uses 3D physics, which includes 3D. In `Cargo.toml`:

```toml
[dependencies]
ponko = { version = "0.2", features = ["physics3d"] }
```

**The yard.** Copy
[`assets/tutorial_3d/yard.glb`](https://github.com/ponkoengine/ponko/tree/main/assets/tutorial_3d)
from the engine's repository into your project's `assets/tutorial_3d/`. It is
a small glTF scene made for this tutorial (under the engine's license, MIT
or Apache-2.0): ground, walls, a platform with a ramp, stairs up to a ledge
and a pillar, in three materials. Any glTF file works the same way; Blender
and most 3D editors export them.

**Start from a clean `src/lib.rs`.** Delete the template's cube code and
`assets/settings.json`, and start `src/lib.rs` with:

```rust
{{#include ../../examples/tutorial_3d/game.rs:use_yard}}
```

`ponko::d3` holds the 3D names: `Transform`, `Model`, `MeshData`,
`RigidBody`, `Collider`, `Character`, `PointLight`.

## Step 1: a glTF scene

The game, as it will look at the end. Add it with only `.plugin(Yard)` for
now, and each other line at its step:

```rust
{{#include ../../examples/tutorial_3d/game.rs:game}}
```

As in the template, `game()` returns a game ready to `run()` (the plugins
hold all of its setup, which `.configured()` says), so `src/main.rs` stays
as the template wrote it, and the tests in [step 6](#step-6-test-it) add
their settings to the same `game()`.

The yard:

```rust
{{#include ../../examples/tutorial_3d/game.rs:yard}}
```

- **`ctx.gfx.load_mesh`** loads the file's meshes and materials into one
  `Mesh`, drawn wherever an entity with a `Model` of it stands
  ([Meshes and materials](3d.md#meshes-and-materials)).
- **To make it solid**, `ctx.load_collision_mesh` makes a collision mesh of
  the same file's triangles. The entity gets a fixed `RigidBody` and a
  `Collider` of that mesh, so what you walk on is exactly what you see
  ([3D physics](physics3d.md#bodies-colliders-and-terrain)). A triangle mesh
  suits level geometry that never moves.
- **`.physics3d(vec3(0.0, -9.81, 0.0))`** turns 3D physics on, in meters,
  with +Y up. The game then ticks 50 times a second, the physics' rate.
- **The sun**: `sun_direction` is which way the light travels; a low sun
  gives long shadows ([Light and shadows](3d.md#light-and-shadows)). The
  camera looks at the yard from above until step 2 moves it.

`cargo run` shows the yard in the evening sun.

## Step 2: a character controller and a camera

Named actions first, as in 2D ([Named input actions](actions.md)): a 2D
`move` axis for WASD, the arrow keys and the left stick, a `turn` axis for Q,
E and the right stick, and `jump`.

```rust
{{#include ../../examples/tutorial_3d/game.rs:bindings}}
```

Then the walker:

```rust
{{#include ../../examples/tutorial_3d/game.rs:walker}}
```

- **`Character`** is the engine's character controller: an upright capsule
  that walks at its `velocity`, slides along walls, climbs slopes and steps,
  stays on the ground going down, falls by gravity and pushes things. Its
  transform is at its **feet**. After each physics step, `grounded` says
  whether it stands on something
  ([A walking character](physics3d.md#a-walking-character)).
- Its look is a sphere from `MeshData::sphere`, stretched into a capsule
  shape with its bottom at the origin, so it stands where the feet are.
- `control` is a frame system: it turns the facing by the `turn` axis,
  walks the way the walker faces (`Quat::from_rotation_y(yaw)` turns the
  input), and jumps only from the ground. The physics owns the position; the
  rotation is ours to set.
- `follow` puts the camera behind and above the walker and turns it to look
  at its head ([Camera](3d.md#camera)).

Add `.plugin(Walker)`. Walk up the ramp onto the platform and up the stairs;
the walls and the pillar stop you.

## Step 3: lights

Lanterns: an iron post and a glass ball with a **point light** in it,
dark until lit.

```rust
{{#include ../../examples/tutorial_3d/game.rs:lanterns}}
```

- A `PointLight` on an entity with a `Transform` shines in every direction,
  up to its `range` ([Point and spot lights](3d.md#point-and-spot-lights)).
  Here it starts at intensity 0.
- The glass has two meshes from the same `MeshData`, one with a dull
  material and one with an `emissive` color, which shines without any light
  on it. `set_lit` swaps the `Model` and turns the light up or down.
- The post is a 1 m cube scaled into a pole: a `Transform`'s `scale`
  changes how a model is drawn.

Add `.plugin(Lanterns)`. The lanterns stand dark around the yard; the next
step lights them.

## Step 4: physics

Crates that fall and can be pushed, and a **sensor** around each lantern
that lights it when the walker steps in:

```rust
{{#include ../../examples/tutorial_3d/game.rs:bodies}}
```

- A dynamic `RigidBody` with a box `Collider` falls, stacks and tumbles; the
  `Velocity` component lets you read or set its speed. The walker pushes
  crates as it walks into them (light ones: `density` is in kg/m³).
- `.sensor()` makes a collider that reports overlaps and blocks nothing.
  The walker sets off sensors, and the physics step sends a
  `CollisionStarted` event with `sensor` set
  ([Collisions](physics.md#collisions); 3D sends the same events).
- `light_touched_lanterns` runs after the step (`sim_system`) and lights
  each lantern the walker touched.
- This plugin's setup finds the lanterns with a query, so it is added after
  `Lanterns`, whose setup spawns them: plugins set up in the order they
  were added.

Add `.plugin(Bodies)`, walk into a lantern and it lights up; walk into the
crates and they topple.

## Step 5: UI

A counter in the corner and a panel when every lantern is lit, with the
[retained UI](ui.md). Add its names to the `use` lines:

```rust
{{#include ../../examples/tutorial_3d/game.rs:use_hud}}
```

and the plugin:

```rust
{{#include ../../examples/tutorial_3d/game.rs:hud}}
```

- The counter is a label on a **HUD**, shown at setup with `show_hud`: it
  is always drawn, under every screen, takes no input and Escape never
  closes it ([HUDs](ui.md#huds)). The panel is a second root, pushed on top
  as a **modal** screen (it dims what is below and takes all input) when
  the last lantern is lit. It opens with "Play again" focused
  (`.focused()`), so Enter presses it.
- `hud` sets the counter's text every frame from the lanterns themselves,
  so it can't drift from the game.
- "Play again" (or Escape, which closes the top screen and sends
  `ScreenPopped`) puts the lanterns out and the walker back at the start:
  setting a character's transform moves it there.
- Nodes have names (`hud.count`, `hud.again`) that tests and tools find
  them by.

Add `.plugin(Hud)`. Light every lantern to see the panel.

## Step 6: test it

Like the 2D game, this one runs without a window or GPU in `cargo test`
([Headless runs and testing](headless.md)): `headless_frames(true)` runs the
frame systems and the UI, and an [input scenario](actions.md#input-scenarios)
holds keys and clicks buttons on given ticks. In `tests/game.rs`, after
`use lantern_yard::*;`:

```rust
{{#include ../../tests/tutorial_3d.rs:uses}}
```

Walking and colliding:

```rust
{{#include ../../tests/tutorial_3d.rs:walk}}
```

The whole round, with the walker moved from lantern to lantern by a small
system of the test's own, and the panel's button clicked by name:

```rust
{{#include ../../tests/tutorial_3d.rs:all_lit}}
```

[`tests/tutorial_3d.rs`](https://github.com/ponkoengine/ponko/blob/main/tests/tutorial_3d.rs)
also turns, walks up the ramp and lets the crates fall.

## Step 7: package it

Name the game in `Cargo.toml`:

```toml
[package.metadata.ponko]
name = "Lantern Yard"
id = "com.example.lantern-yard"   # a reverse-DNS id you own
assets = ["assets"]
```

```sh
ponko package          # for this computer, in target/package/
ponko package --web    # a folder for any static web host
```

`ponko package` builds a release with the yard and the license notices,
packs it for this computer and starts the packaged copy once, headless, to
check that it runs ([Packaging any game](packaging.md#packaging-any-game-ponko-package)).
`--web` needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli`,
as the [2D tutorial's last step](tutorial-2d.md#step-7-package-it-and-put-it-on-the-web)
shows; browsers draw it with WebGPU or WebGL2 ([Web builds](web.md)).

## Where next

- Shadows from the lanterns: `PointLight` has a `shadows` switch
  ([Shadows from point and spot lights](3d.md#shadows-from-point-and-spot-lights)).
- Sound in 3D: a lantern that hums where it stands
  ([Sound in 3D](audio.md#sound-in-3d)).
- An animated character instead of the capsule:
  [Skinned meshes](3d.md#skinned-meshes) and [animation state machines](3d.md#animation-state-machines).
- Save which lanterns are lit with `ctx.storage`, as the
  [2D tutorial](tutorial-2d.md#step-5-saving) does.
