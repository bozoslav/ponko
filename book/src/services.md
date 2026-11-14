# Ctx and its services

Every system gets one argument, `ctx: &mut Ctx`. `Ctx` is a set of public
fields, one per area of the engine. Each area is a plain struct with its own
methods, so you find a call by asking "which area is this?" and typing
`ctx.<area>.`.

| Field | Type | What lives there |
|---|---|---|
| `ctx.world`, `ctx.commands`, `ctx.events` | `World`, `Commands`, `Events` | Entities, components and resources; changes to make after the system; messages between systems. |
| `ctx.time` | `Time` | The frame clock and the simulation clock, pause, step, speed. |
| `ctx.input` | `Input` | Keyboard, mouse, gamepads, named actions, typed text, the cursor lock. |
| `ctx.gfx` | `Graphics` | Drawing sprites, text and shapes; textures, fonts, screenshots; with the `3d` feature meshes, materials, shaders, effects, compute, instance batches, terrain, render targets, texture arrays, reflection probes and the quality presets. |
| `ctx.audio` | `Audio` | Sounds and music, what is playing, mixer buses, the 3D listener. |
| `ctx.ui` | `Ui` | The retained widget tree and its events, who has the pointer and the keyboard, and the immediate debug widgets (`ctx.ui.immediate`). |
| `ctx.assets` | `Assets` | Asset sources, data files, saving scenes and prefabs, what hot reload changed. |
| `ctx.perf` | `Perf` | System timings, frame and render statistics, the HUD, traces, profiling reports. |
| `ctx.storage` | `Storage` | The player's saves and settings in the per-user data folder (browser storage on the web), crash-safe writes, save slots. See [Saves and settings](storage.md). |
| `ctx.camera`, `ctx.scene3d.camera`, `ctx.scene3d.lighting`, `ctx.scene3d.sky`, `ctx.scene3d.post`, `ctx.scene3d.lod` | plain settings | What the renderer reads when it draws the frame. |
| `ctx.spatial`, `ctx.physics`, `ctx.physics3d`, `ctx.net` | | Neighbor queries, the physics worlds, the lockstep session. |

The API reference of each service type starts with what lives there, when to
call it (setup, simulation systems, frame systems) and what it needs (a
window, the `3d` feature, nothing).

## One spelling per call

Every call is spelled on its service: `ctx.time.dt()`,
`ctx.input.key_pressed(Key::Space)`, `ctx.gfx.draw(texture, pos)`,
`ctx.audio.play(sound)`. Up to 0.1 a few of them were also methods of
`Ctx` (`ctx.dt()`, `ctx.key_down(k)`, `ctx.key_pressed(k)`,
`ctx.key_released(k)`, `ctx.mouse_pos()`, `ctx.draw(..)`,
`ctx.draw_text(..)`, `ctx.load_texture(..)`, `ctx.load_sound(..)`,
`ctx.play_sound(..)`); those are deprecated, and the compiler names the
service call that replaces each. `ctx.quit()` is the context's own.

A few more methods stay on `Ctx` because they use several areas at once:
`ctx.free(handle)`, `ctx.is_live(handle)` and `ctx.live_counts()` (see
[Handles](handles.md)), `ctx.load_scene(name)` and `ctx.spawn_prefab(name, at)`
(the world, textures and sounds together, see [Scenes and prefabs](scenes.md)),
`ctx.random()`, `ctx.mouse_world()`, `ctx.window_size()` and the ragdoll calls.
Asset loading, streaming and residency are all on `ctx.gfx` (see
[Streaming and residency](streaming.md)). Only `ctx.finish_uploads()`, for
loading screens, stays on `Ctx`: it may evict, and eviction looks at what the
world's entities show.

## Two areas at once

The services are separate fields, so Rust lets you use one while another is
borrowed. A method of `Ctx` takes all of `ctx`, so inside a query or a loop
over UI events, use the services:

```rust
{{#include ../../tests/services.rs:time_borrow}}
```

```rust
{{#include ../../tests/services.rs:ui_borrow}}
```

A method of `Ctx` (such as `ctx.mouse_world()`) in the first loop would not
compile: the query holds `ctx.world`, and a method of `Ctx` asks for the
whole of it. Either read the value before the loop, or use the services.

## Which `dt`?

`ctx.time.dt()` follows the system that is
running: the fixed step in a simulation system, the time since the last
frame anywhere else. When the code should say which one it means:

| Call | What it is |
|---|---|
| `ctx.time.sim_dt()` | The simulation step, in every kind of system (times the interval inside a system added with one, `system.every(n)`). |
| `ctx.time.frame_dt()` | Real time since the last frame, in every kind of system. |

## Calls that need another area

A service owns its own state and nothing else. A call that reads or
writes the world takes it as an argument; no service call takes another
service. A call that needs two services is a method of `Ctx`, which holds
them all:

```rust
# use ponko::prelude::*;
# #[derive(serde::Deserialize)]
# struct Rules { speed: f32 }
# fn setup(ctx: &mut Ctx) {
// Data files fill the world.
ctx.assets.load_resource::<Rules>(&mut ctx.world, "rules.json");
// Immediate widgets draw with `gfx` and read the mouse from `input`:
// `ctx.immediate_ui()` lends them both.
let rect = Rect::new(10.0, 10.0, 120.0, 32.0);
if ctx.immediate_ui().button(rect, "Pause") {
    ctx.time.pause();
}
# }
```

The other calls of that kind are `ctx.start_scenario(..)` and
`ctx.cancel_scenario()` (input and the clock), `ctx.quality_autotune_status()`
(`3d`: render settings and the GPU's measurements), `ctx.mouse_world()`
(input and the camera) and the handle calls `ctx.free(..)` and
`ctx.is_live(..)`, which reach whichever service holds the handle.

With the `3d` feature the same holds for
`ctx.scene3d.set_quality(quality)`,
`ctx.gfx.write_animated_morphs(&mut ctx.world)` and the reflection probe
calls.

## In C++

The C++ `ponko::ctx` has no service fields: every call is a method of `ctx`
(`ctx.dt()`, `ctx.draw(..)`, `ctx.pause()`, `ctx.set_perf_hud(...)`), and C's
are `ponko_ctx_*` functions. That is the one spelling in C and C++, as the
service call is in Rust: the flat names stay (STABILITY.md, "C ABI"), and
the C++ wrapper has no second spelling for any of them. The borrowing
rules above are Rust's; C++ instead refuses a world or context call while a
query runs, except the ones listed in [Queries and threads](c-cpp.md#queries-and-threads).

```cpp
{{#include ../../examples/cpp/guide.cpp:services_flat}}
```

## Coming from the flat spelling

Before the services, every call was a method of `Ctx`. Those flat methods
were removed in 0.1.0; call the service. Most keep their name and only gain the
field: `ctx.set_cursor_locked(true)` is `ctx.input.set_cursor_locked(true)`,
`ctx.create_mesh(...)` is `ctx.gfx.create_mesh(...)`. The ones whose name
changed, because the field now says the area:

| Before | Now |
|---|---|
| `ctx.time()` | `ctx.time.elapsed()` |
| `ctx.set_sim_speed(x)`, `ctx.sim_speed()` | `ctx.time.set_speed(x)`, `ctx.time.speed()` |
| `ctx.feed_input(event)` | `ctx.input.feed(event)` |
| `ctx.ui_insert_text(text)` | `ctx.input.insert_text(text)` |
| `ctx.create_sound`, `ctx.try_load_sound` | `ctx.audio.create`, `ctx.audio.try_load` |
| `ctx.play_sound_ex`, `ctx.play_sound_on`, `ctx.play_sound_at` | `ctx.audio.play_ex`, `ctx.audio.play_on`, `ctx.audio.play_at` |
| `ctx.stop_sound`, `ctx.set_sound_effects` | `ctx.audio.stop`, `ctx.audio.set_effects` |
| `ctx.audio_bus`, `ctx.set_audio_bus_volume`, `ctx.mute_audio_bus` | `ctx.audio.bus`, `ctx.audio.set_bus_volume`, `ctx.audio.mute_bus` |
| `ctx.set_audio_listener`, `ctx.audio_listener` | `ctx.audio.set_listener`, `ctx.audio.listener` |
| `ctx.graphics_quality()`, `ctx.set_graphics_quality(q)` | `ctx.scene3d.quality()`, `ctx.scene3d.set_quality(q)` |
| `ctx.free_gpu_buffer(buffer)` | `ctx.free(buffer)` |
| `ctx.ui_retained` (field) | `ctx.ui` |
| `ctx.ui_style` (field) | `ctx.ui.immediate.style` |
| `ctx.ui_panel`, `ctx.ui_label`, `ctx.ui_button` | `ctx.immediate_ui().panel(...)`, `.label(...)`, `.button(...)` |
| `ctx.mouse_over_ui()` | `ctx.ui.wants_pointer()` |
| `ctx.ui_wants_keyboard()`, `ctx.set_ui_pointer_ray(ray)` | `ctx.ui.wants_keyboard()`, `ctx.ui.set_pointer_ray(ray)` |
| `ctx.add_asset_source`, `ctx.load_asset_with` | `ctx.assets.add_source`, `ctx.assets.load_with` |
| `ctx.load_resource::<R>(path)` and the `load_*_json` calls | `ctx.assets.load_resource::<R>(&mut ctx.world, path)`, ... |
| `ctx.save_scene(path)`, `ctx.save_prefab(entity, path)` | `ctx.assets.save_scene(&ctx.world, path)`, `ctx.assets.save_prefab(&ctx.world, entity, path)` |
| `ctx.set_perf_hud`, `ctx.perf_hud`, `ctx.perf_report` | `ctx.perf.set_hud`, `ctx.perf.hud`, `ctx.perf.report` |
| `ctx.profile(name, \|ctx\| ...)` | `let _timing = ctx.perf.scope(name);` |

`Ctx` keeps a short list of shortcuts on purpose: `dt`, `key_down`,
`key_pressed`, `key_released`, `mouse_pos`, `mouse_world`, `window_size`,
`quit`, `draw`, `draw_text`, `load_texture`, `load_sound`, `play_sound` and
`random`. A method that needs several services at once stays on `Ctx` as well:
`free` and `is_live`, loading a scene or spawning a prefab, streaming and
residency (`stream_mesh`, `resident_texture`, ...), ragdolls and the reports
across services (`live_counts`, `asset_inventory`). Every other call is on its
service.
