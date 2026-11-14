# Ponko

Ponko is a general-purpose 2D/3D game engine in Rust, with C and C++ bindings.
It aims to combine serious game-engine capabilities, measured performance,
clear code-first APIs and practical AI-assisted development. Large simulations
are a strength, alongside ordinary 2D and 3D games.

You do not need a large visual editor. Start with `Game::new()`, setup code and
systems; use external asset tools and the optional runtime inspector when they
help. Modular internals should support an integrated experience: common things
work immediately, advanced things can be customized, and neither requires
understanding the whole engine.

Read the [project vision](https://github.com/ponkoengine/ponko/blob/main/PROJECT_VISION.md),
[architecture](https://github.com/ponkoengine/ponko/blob/main/ARCHITECTURE.md),
and [roadmap](https://github.com/ponkoengine/ponko/blob/main/ROADMAP.md) for direction,
current limitations and planned work. The [Bevy and Unity comparison](comparison.md)
explains the tradeoffs; Ponko does not claim feature parity or overall superiority.

## Start small, scale when needed

A small game can keep state in a closure. This example loads `assets/crab.png`,
moves it with the arrow keys, and quits on Escape:

<div class="lang-pair">
<div>

```rust
use ponko::prelude::*;

fn main() {
    Game::new("My Game", 800, 600).run(|ctx| {
        let crab = ctx.gfx.load_texture("crab.png");
        let mut pos = vec2(400.0, 300.0);
        move |ctx: &mut Ctx| {
            if ctx.input.key_down(Key::Right) { pos.x += 200.0 * ctx.time.dt(); }
            if ctx.input.key_down(Key::Left) { pos.x -= 200.0 * ctx.time.dt(); }
            if ctx.input.key_pressed(Key::Escape) { ctx.quit(); }
            ctx.gfx.draw(crab, pos);
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

The C++ wrapper provides `ponko::` through `ponko.hpp`. Continue with [Getting started](getting-started.md) for installation
and the paired API examples.

Grow into [entities and systems](ecs.md), fixed-step simulation and parallel
queries as needed. A [headless run](headless.md) advances simulation without a
window or GPU. Configure a minimal build with Cargo features when those
dependencies should also be omitted. Parallel query iteration is built in,
and systems that declare what they use can opt into
[running at the same time](parallel-systems.md).

## Current tools and future goals

Ponko includes a wgpu 2D/3D renderer, physics integrations, scenes/prefabs,
audio, UI, networking and native bindings. Feature availability and platform
limits vary; use the relevant chapters and roadmap before relying on a complete
workflow. Renderer restructuring and stronger ECS scheduling are gradual
improvements to existing systems, not replacements already delivered.

[AI-agent tools](agents.md) can launch a game, inspect reflected state, step
exact ticks, read logs and measure performance. For visual checks, launch a
windowed instance and request a screenshot. A useful loop is: edit, launch,
run a fixed scenario, inspect state and timings, correct, and repeat with
evidence. Broader automated acceptance workflows and reliable autonomous
creation of complete games remain goals.

## How this book is laid out

- **Guide:** practical chapters for game code, assets, rendering, simulation and tools.
- **Reference:** benchmarks, public APIs, compatibility and contribution guidance.

Every page is searchable: press `S` or click the magnifying glass. Use
[performance measurements](performance.md) to understand specific workloads;
benchmark results do not establish a general engine ranking.

## Status and compatibility

Formerly Crabby Engine; [Naming and compatibility](naming.md) maps the old
names to the new ones. This guide follows `main`; an installed release may
differ. Ponko is at 0.x:
[STABILITY.md](https://github.com/ponkoengine/ponko/blob/main/STABILITY.md)
says what a release may change and what it keeps (API and ABI, MSRV, saved
worlds, MCP and determinism).

Help improve the engine through
[Contributing](https://github.com/ponkoengine/ponko/blob/main/CONTRIBUTING.md).
