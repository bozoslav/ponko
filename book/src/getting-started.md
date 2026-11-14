# Getting started

You need a recent stable Rust (`rustup update stable`). On Linux, also the
headers for sound and gamepads: `sudo apt-get install libasound2-dev libudev-dev`
(Debian, Ubuntu).

## Run the examples

Clone the repository and run any example:

```sh
git clone https://github.com/ponkoengine/ponko
cd ponko
cargo run --release --example boids
```

| Example | What it shows |
|---|---|
| `window` | The smallest game: a window, input, one sprite |
| `sprites` | Drawing many sprites with tint, rotation and scale |
| `ecs` | Entities, components and systems, with sim controls |
| `bunnymark` / `bunnymark_ecs` | Sprite throughput, immediate mode vs ECS |
| `boids` | Flocking with a spatial grid on all cores |
| `ants` | Ant colonies: pheromone trails, foraging, growth; up to millions of ants |
| `headless` | A million agents with no window or GPU |
| `coin_dash` | A small complete game (title, settings with UI scale and rebinding, rounds, pause, saved progress, sound), tested headlessly, packaged for the desktop and built for the web; see [A complete game, packaged](packaging.md) and [Headless runs and testing](headless.md#a-complete-game-tested-headlessly) |
| `examples/cpp/boids.cpp` | The boids example in C++ (build with CMake, see [C and C++](c-cpp.md)) |

Always use `--release` for anything with many entities; debug builds are
10–50× slower.

## A new project

The `ponko` command makes a project that builds, tests and can be driven by
an AI agent as it is:

```sh
cargo install ponko_cli
ponko new my_game        # a 2D game; --3d for a 3D game, --headless for a simulation
cd my_game
cargo test               # headless tests: no window, no GPU
cargo run                # the game, in a window
ponko package            # the download for players: release build, assets, licenses, archive
ponko textures           # GPU-compressed .ktx2 textures from your images (see Drawing)
```

`ponko_cli` is new in 0.2; until that release, install it from the
repository: `cargo install ponko_cli --git https://github.com/ponkoengine/ponko`.

| Template | Engine features | What's in it |
|---|---|---|
| `--2d` (the default) | the defaults: window, sound, gamepads, inspector | a square moved with the arrow keys, kept inside the window |
| `--3d` | the defaults plus `3d` | a spinning cube on a ground plane, a camera and a sun |
| `--headless` | none (`default-features = false`): no window, GPU or sound | a thousand walkers bouncing around a square, and a summary |

Each project has:

- `Cargo.toml` with the engine, a fast debug profile (below) and the
  `[package.metadata.ponko]` table that tells `ponko package` what goes in the
  game's download ([Packaging any game](packaging.md#packaging-any-game-ponko-package));
- `src/lib.rs` with the rules as sim systems, `src/main.rs` that starts the
  game (`HEADLESS=1 cargo run` runs it without a window), and headless tests
  in `tests/` ([Headless runs and testing](headless.md));
- `assets/` with a `settings.json` of numbers to tune, which a debug build
  reloads when you save it ([Data files](data-files.md));
- `.mcp.json`, which registers [`ponko-mcp`](agents.md) for your coding agent
  (`cargo install ponko_mcp` once), and `AGENTS.md` (`CLAUDE.md` points at it)
  with the commands, the layout and a pointer to [the agents chapter](agents.md).

To build a whole game from here, step by step, follow [Tutorial: a first 2D
game](tutorial-2d.md) (a platformer with a level from a level editor, a menu
and saves) or [Tutorial: a first 3D game](tutorial-3d.md) (a glTF scene, a
character controller, lights and UI).

The project depends on the released engine from crates.io. `--git <url>` or
`--path <checkout>` points it at an engine repository or a local checkout
instead (for features that are only on `main` so far, see below).

The same templates work with
[cargo-generate](https://github.com/cargo-generate/cargo-generate), if you
have it: `cargo generate --git https://github.com/ponkoengine/ponko
crates/ponko_cli/templates/2d` (or `3d`, `headless`).

### The fast debug profile

The templates' `Cargo.toml` builds debug builds like this:

```toml
[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3
```

Your own code gets light optimization, and the engine and every other
dependency full optimization, which they get once and keep: games and tests
run many times faster than in a plain debug build, rebuilding after an edit
stays quick, and debug info and debug checks stay on. Add it to a project you
made by hand too.

## Your first game

To start by hand instead, add the engine to a new project from crates.io:

```sh
cargo new my_game && cd my_game
cargo add ponko
```

In `Cargo.toml` that is:

```toml
[dependencies]
ponko = "0.1"
```

Its API reference is [on docs.rs](https://docs.rs/ponko/0.1/ponko/). This
guide follows the `main` branch, so after a release a page can describe an
API that is only on `main` so far (the note under each page's title says
so). To use those, depend on `main` instead:
`ponko = { git = "https://github.com/ponkoengine/ponko" }` (or
`cargo add ponko --git https://github.com/ponkoengine/ponko`). The
[changelog](https://github.com/ponkoengine/ponko/blob/main/CHANGELOG.md) lists
what each release added.

Then in `src/main.rs` (Rust) or `main.cpp` (C++, see [C and C++](c-cpp.md)
for building):

<div class="lang-pair">
<div>

```rust
use ponko::prelude::*;

fn main() {
    Game::new("My Game", 800, 600).run(|ctx| {
        let crab = ctx.load_texture("crab.png");
        let mut pos = vec2(400.0, 300.0);

        move |ctx: &mut Ctx| {
            if ctx.key_down(Key::Right) { pos.x += 200.0 * ctx.dt(); }
            if ctx.key_down(Key::Left)  { pos.x -= 200.0 * ctx.dt(); }
            if ctx.key_pressed(Key::Escape) { ctx.quit(); }
            ctx.draw(crab, pos);
        }
    });
}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:first_game_full}}
```

</div>
</div>

`setup` runs once and returns a closure that runs every frame. In C++, the
per-frame code is a frame `system` and the state lives in variables it
captures. Variables you
create in `setup` and move into the closure are your game state. For small
games that's all you need; when things grow, move to
[entities and systems](ecs.md).

Everything here comes in with `use ponko::prelude::*;`: `Game`, `Ctx`,
`Vec2`, `Color`, `Key`, the ECS types and the common components. Add
`use ponko::d2::*;` or `use ponko::d3::*;` for one dimension's names
([Naming and compatibility](naming.md)).

## Setup on the builder

Once a game has systems, its startup work goes on the builder too:
`Game::setup` takes the startup code, and `run()` takes nothing.

<div class="lang-pair">
<div>

```rust
# fn movement(_: &mut Ctx) {} fn spawn_enemies(_: &mut Ctx) {}
# let with_enemies = true;
let game = Game::new("My Game", 800, 600)
    .sim_system(movement)
    .setup(|ctx| {
        let crab = ctx.load_texture("crab.png");
        ctx.world.spawn((ponko::d2::Transform::at(vec2(400.0, 300.0)), ponko::d2::Sprite::new(crab)));
    });
// Startup chosen at run time: each call adds one more callback.
let game = if with_enemies { game.setup(spawn_enemies) } else { game };
game.run(); // or run_headless(ticks)
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:builder_setup}}
```

</div>
</div>

- Setup callbacks run once, after the window and the renderer exist and
  before any system, in the order they were added. A
  [plugin](systems.md#plugins)'s `setup` is in the same queue, at the place
  the plugin was added.
- Commands recorded in a callback are applied when it returns, so the next
  callback and the first system see what it spawned.
- Add systems and options first: after the first `setup`, the builder
  takes more `setup` and `plugin` calls, the `run` methods, and what a test
  adds to a finished game without changing it: `headless_frames`,
  `input_scenario` and `run_headless_scenario(scenario)`, `data_dir`,
  `seed`, and systems of its own (`system`, `sim_system`). So the tests of a
  `ponko new` project can drive its menus with frames and keep its saves in
  a folder of their own while `game()` stays as it is.
- A game whose startup is all in its [plugins](systems.md#plugins) and
  systems has no `setup` to add: `.configured()` gives it the same
  `run()` and `run_headless(ticks)` (C++ already has `run()` and
  `run_headless(ticks)` on any game; in C, pass a null setup).
- Nothing is returned, so two branches can pick different startup code
  (above). The state a frame closure would have captured goes in a
  resource, or in a [system that owns its state](systems.md#systems-with-state).

`run(|ctx| ...)` and `run_headless(ticks, |ctx| ...)` stay for the small
cases: the one-closure program at the top of this page, and a headless run
whose setup borrows a local variable (a timer, a counter in a test), which a
stored `setup` callback can't do.

## When loading fails

`load_texture`, `load_mesh`, `load_sound`, `load_font` and the other
`load_*` / `create_*` calls are for setup code, where a missing file is a
bug: they panic with a message that says what's wrong and where the engine
looked. Each has a `try_` twin that returns `Result<_, ponko::error::Error>`
instead, for assets that may legitimately be missing (a mod, an optional
skin, a user's file):

```rust
# use ponko::prelude::*;
# fn setup(ctx: &mut Ctx) {
match ctx.gfx.try_load_texture("skins/gold.png") {
    Ok(skin) => { /* use it */ let _ = skin; }
    Err(e) if e.kind() == ErrorKind::NotFound => { /* no skin installed */ }
    Err(e) => eprintln!("{e}"), // "ponko: ..." with the reason
}
# }
```

Match on `e.kind()` (`NotFound`, `Io`, `Decode`, `Shader`, `Invalid`,
`BadHandle`, `Limit`, ...), not on the message text, which may change.
`ponko::error::Error` implements `std::error::Error`, so `?` passes it on into a
`Box<dyn Error>` or your own error type.

Calls that can fail in normal play return `Result<_, ponko::error::Error>` under
their plain name and have no panicking twin: loading a scene and spawning a
prefab (`ctx.load_scene`, `ctx.spawn_prefab`), saving them
(`ctx.assets.save_scene`, `save_prefab`), loading JSON into a component or
resource (`ctx.assets.load_component_json`, `load_resource_json`), starting a ragdoll,
baking a reflection probe, and your own decoders (`ctx.assets.load_with`).
In C and C++, `ponko_last_error_kind()` and `ponko::error::kind()` give the
same kinds ([C and C++](c-cpp.md)).
