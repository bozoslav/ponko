# Tools and agents: the control endpoint

A running game, windowed or headless, can open a **control endpoint** that
tools use to look inside it and drive it: AI agents (through
[`ponko-mcp`](agents.md)), the inspector, test scripts. Several instances can run
at once; each gets its own endpoint.

## Turning it on

It's off by default, so shipped games never open a port. The game can turn
it on in code (C and C++ games too: `game.control_endpoint(true)`):

<div class="lang-pair">
<div>

```rust
Game::new("Colony", 1280, 720).control_endpoint(true)
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:control_on}}
```

</div>
</div>

Or, without code changes, the `PONKO_CONTROL=1` environment variable turns
it on in a debug build, or in a release build made with the engine's
`control-env` feature:

```sh
PONKO_CONTROL=1 cargo run --example boids
PONKO_CONTROL=1 PONKO_START_PAUSED=1 HEADLESS=1 cargo run --release --features control-env --example boids
```

A release build without `control-env` ignores the variable (and says so on
stderr), so a player can't open a port into a shipped game that didn't ask
for one. [`ponko-mcp`'s `launch`](agents.md) adds the feature to the
release builds it makes. Don't ship a game with it: Cargo turns a feature
on for the whole build when any crate in it asks for it, so check
`cargo tree -e features -i ponko` if you're unsure.
`PONKO_START_PAUSED=1` starts the simulation paused, so a tool can set
things up and step it tick by tick.

## Finding instances

Each instance writes a small JSON file to `~/.ponko/instances/`
(`%LOCALAPPDATA%\ponko\instances` on Windows, or `$PONKO_INSTANCES_DIR`)
and removes it on exit:

```json
{
  "name": "Ponko: boids",
  "pid": 41213,
  "port": 50113,
  "token": "9f1c…",
  "headless": false,
  "cwd": "/home/me/my_game",
  "exe": "/home/me/my_game/target/release/my_game",
  "started": 1793642400,
  "engine_version": "0.1.0"
}
```

A file whose `pid` is no longer running is stale (the game crashed); tools
skip it.

## Security

- The endpoint listens on `127.0.0.1` only.
- Every request must carry the instance's `token`, 128 bits from the
  operating system's secure random source. The registry file is readable
  only by your user (mode `0600` on Unix).
- A request line may be at most 4 MiB, and at most 32 connections are
  served at once; past either limit the client gets an error and is
  disconnected. A new connection's first request must carry the right
  token, within 10 seconds; otherwise it gets one error reply and is
  closed. After that it may stay idle.
- A release build honors `PONKO_CONTROL` only with the `control-env`
  feature (see [Turning it on](#turning-it-on)).

### File paths

Methods that read or write files take paths relative to a folder of the
game, and only of their own file type. Absolute paths, `..`, `.`, `\`,
`:`, names starting with `.` and Windows device names (`CON`, `NUL`,
`COM1`, `CONIN$`, ... with any extension) are refused, and so is a path
that a link inside the folder leads out of it. An `assets/` folder that is
itself a link (to assets shared between games, say) is trusted:

- `world.save` and `world.load`: a `.json` file below the game's working
  folder (the `cwd` in its registry file). `world.save` makes missing
  folders and refuses an existing file unless `overwrite` is `true`; even
  then it replaces only a plain file (not a link, a pipe or a device;
  on unix also not one with other names, hard links) and not one a
  `screenshot` or `perf.trace` will still write. On unix such a file is
  recognized by its device and inode; on Windows only by its full path,
  so another name for it (a hard link) isn't caught there.
- `scene.*`, `prefab.*` and `editor.save`: a `.json` file below
  `assets/`. `scene.save`, `prefab.save` and `editor.save` refuse a file a
  `screenshot` or `perf.trace` will still write, too. `editor.place` takes a
  prefab `.json` or a glTF mesh (`.glb`, `.gltf`) below `assets/`.
- `screenshot` (`.png`) and `perf.trace` (`.json`): a new file below the
  working folder (missing folders are made), created empty when the
  request arrives and written through that handle, so they never replace a
  file. If it can't be written, a newer `perf.trace` replaces the trace,
  or the game shuts down first, the file is removed. Leave `path` out for a
  new temp file with a random name. A headless game refuses `perf.trace`
  (it draws no frames to trace) before it makes any file or folder.

## Protocol

TCP, one JSON object per line in each direction.

```json
{"id": 1, "token": "9f1c…", "method": "entities.list", "params": {"with": ["Transform2D"], "limit": 2}}
{"id": 1, "result": {"total": 20000, "offset": 0, "entities": [{"entity": 4294967296, "components": {"Transform2D": {"position": [10.5, 3.0], "rotation": 0.0, "scale": [0.5, 0.5]}}, "other_components": ["Sprite", "Vel"]}, …]}}
```

Errors come back as `{"id": 1, "error": "entity 999 does not exist"}`.

Requests are applied by the game loop between frames (between ticks when
headless), so they never race the simulation or each other. A paused
headless game waits for requests and runs only the ticks you step.

| Method | Params | Returns |
|---|---|---|
| `methods` | | every method with a description |
| `capabilities` | | what this instance can do ([details](#capabilities)) |
| `info` | | pid, headless, tick, sim time, paused, speed, fps, entity count, render stats (sprites, draw calls, CPU and GPU ms) |
| `world.summary` | | entity count, reflected components and resources, archetypes and sizes |
| `entities.list` | `with?`, `where?`, `limit?` (100), `offset?` | entities with reflected components as JSON, names of the others |
| `entities.stats` | `fields`, `with?`, `where?` | `{entities, fields: {path: {count, min, max, mean} or {count, values}}}` (see below) |
| `entity.get` | `entity` | `{component: value}` |
| `entity.spawn` | `components` | `{entity}` |
| `entity.despawn` | `entity` | `true` if it existed |
| `component.set` | `entity`, `component`, `value` | the new value (object fields merge) |
| `component.remove` | `entity`, `component` | `true` if it had it |
| `resources.list` | | `{resource: value}` for every reflected resource (`null` if it's not in the world) |
| `resource.get` | `resource` | its value |
| `resource.set` | `resource`, `value` | the new value (object fields merge; inserted if missing) |
| `world.save` | `path`, `overwrite?` (false) | writes a [save](determinism.md#saving-and-loading-the-world) (see [file paths](#file-paths)); entity and resource counts, bytes, tick |
| `world.load` | `path` | replaces every entity from a save (same ids; checked first) |
| `scene.save` / `scene.load` | `path` | a [scene](scenes.md) in `assets/` (loading also loads sprite images) |
| `prefab.spawn` | `path`, `at` | spawns a [prefab](scenes.md) at `[x, y]`; returns `entity` (the root) and `parts` (its [children](scenes.md#prefabs-with-children)) |
| `prefab.save` | `entity`, `path` | saves an entity, with its parts and attachments, as a prefab in `assets/` |
| `prefab.despawn` | `entity` | despawns a prefab's root and its parts; how many entities went |
| `editor.*` | see [below](#the-editor-from-an-agent) | the inspector's scene editing: picking, the gizmo, parenting, placing assets, undo and redo, saving |
| `sim.pause` / `sim.resume` | | |
| `sim.step` | `ticks?` (1) | ticks queued |
| `sim.speed` | `speed` | |
| `input.scenario.load` | `scenario`, `bind_connection?`, `keep_input_when_done?`, `max_ticks?`, `queue_ticks?` | starts a tick-indexed [input scenario](actions.md#input-scenarios) on the next tick, replacing a running one; refused unless the game allows input injection (`Game::input_injection(true)` or `PONKO_INPUT_INJECTION=1`). The keys it holds are released when it ends unless `keep_input_when_done`. With `bind_connection`, it is cancelled when this connection closes. `max_ticks` and an empty scenario are refused before anything running is touched; `queue_ticks` (paused games) queues the scenario's own ticks as steps. Returns `{report, bound_to_connection}` |
| `input.scenario.status` | | the latest scenario's report (`status`, `ticks_run`, `checks_passed`, `released`, `still_held`, and for a failed check its `step`, `tick`, `expected` and `actual`), or `null` |
| `input.scenario.cancel` | `reason?` | ends the running scenario, or releases what a finished one kept held, and releases the input; returns `{cancelled, report}`. A failed check or a cancel takes back only the steps the scenario queued itself (`queue_ticks`), not ticks queued with `sim.step` |
| `systems.timings` | | each system's last and average ms |
| `ecs.diagnostics` | `with?`, `limit?` (100) | archetypes, component storage, every system's declared access, the schedule's stages and dependencies, and the query index's counters (see [ECS diagnostics](agents.md#ecs-diagnostics)); with `with`, also `matching` |
| `events.list` | | reflected [event](events.md) types and how many are kept now |
| `events.subscribe` | `types?` (all), `capacity?` (1000) | starts collecting new events between frames; the newest `capacity` are kept |
| `events.take` | | `{events: [{tick, type, data}], dropped}` since the last take |
| `events.unsubscribe` | | `true` if there was a subscription |
| `logs.get` | `lines?` (100), `since?` | `{lines, next, missed}`: the game's recent stdout and stderr (see below) |
| `perf.get` | `meshes?` (0), `profiling?`, `history?` (0) | where frame time goes, over the last few seconds: fps; avg, p50, p95, p99 and max ms; CPU in systems, render and display wait; GPU ms in total and per pass; draw calls and 3D models and triangles (camera and shadows); the slowest systems; with `meshes`, the meshes drawing the most triangles; `render_inspector`, the last frame's rendering cost in a versioned schema, with the GPU memory estimate, the adapter and, with `history`, the last N drawn frames (at most 120) (see [Profiling a frame](performance.md#the-render-pass-inspector)) |
| `quality.set` | `level` | switches the 3D [graphics quality](3d.md#graphics-quality) preset (`low`, `medium`, `high`, `ultra` or `recommended`), fitted to the GPU; returns `{requested, fitted_to_gpu, previous, quality}` (`fitted_to_gpu` is always `false` for `recommended`; a fitted preset's `quality.level` can read `custom`). Pins render scale and shadow resolution against automatic quality tuning until the game restarts. Needs the `3d` feature (`capabilities` says so otherwise); headless games accept it but draw nothing, and `recommended` is refused until a GPU is open (headless, or before the first frame). See [measuring what a quality level costs](performance.md#measuring-what-a-quality-level-costs) |
| `perf.trace` | `path?` (a new file; a temp file if left out), `frames?` (120) | records the next frames as a Chrome/Perfetto trace; returns the path, written once the frames are done. Refused by a headless game |
| `perf.hud` | `on` | shows or hides the performance overlay |
| `shaders.status` | | each custom shader's last compile or hot reload result (`compiled`, `reloaded`, `failed`, `reload_failed`), reload count, frame, and errors with line and column in the game's own WGSL, cause and GPU variant |
| `camera.get` | | position, zoom, center, visible rect |
| `camera.set` | `position`?, `zoom`?, `look_at`? | move the camera; returns `camera.get` |
| `screenshot` | `path`?, `max_size`? | save the next frame as a PNG (windowed games only); returns `path`, `width`, `height` |
| `game.quit` | | |

Component values use [reflection](reflection.md): call
`world.reflect::<T>()` for a component, `world.reflect_resource::<R>()` for
a resource, to make it readable and writable.
Entity ids are the numbers from `Entity::to_bits`.

## The editor from an agent

The `editor.*` methods make the same changes as the
[inspector's editor](inspector.md#editing-in-3d), through the same undo
history: an agent can build a level, a person can undo its last step in
the inspector, and the other way round. Each change is one undo step and
returns `can_undo` and `can_redo`. Most of them work in 3D (they need the
`3d` feature; `capabilities` says so otherwise); `editor.parent`,
`editor.hierarchy`, `editor.assets`, `editor.place` (of a prefab),
`editor.delete`, `editor.duplicate`, `editor.undo`, `editor.redo` and
`editor.save` work in 2D games too.

| Method | Params | Does |
|---|---|---|
| `editor.pick` | `at` [x, y] | the entity under a window point (the nearest mesh bounds the mouse ray enters; `null` if none), its distance and hit point, and where the ray meets the ground |
| `editor.transform` | `entities`, `translate?` [x, y, z], `rotate?` [x, y, z] (degrees), `scale?` (a factor or [x, y, z]), `space?` (`world` or `local`), `snap?` (`true` or `{step, angle, scale}`) | moves, turns (about the entities' middle) and scales; attached entities keep their parent, and what hangs off them follows |
| `editor.drag` | `entities`, `mode` (`translate`, `rotate`, `scale`), `handle` (`x`, `y`, `z`, `xy`, `xz`, `yz`, `center`), `from`, `to` (window points), `space?`, `snap?` | drags a gizmo handle exactly as the mouse does |
| `editor.parent` | `entity`, `parent` (an id or `null`) | makes it follow the parent (`Attached3D` in 3D, `Attached` in 2D) without moving it, or follow nothing; refuses loops |
| `editor.hierarchy` | | every entity as a tree: parent, link (`attached`, `attached3d`, `joint`, `skin`, `prefab_part`), depth, children, prefab root or not, component names |
| `editor.assets` | | glTF meshes, prefabs and scenes in `assets/`, and the materials of the mesh files loaded so far |
| `editor.place` | `path`, `at?` [x, y, z] (or [x, y] for a 2D prefab), `screen?` [x, y], `snap?` | spawns a mesh as a model, or a prefab, at a point or where a window point's ray meets the ground (default: the middle of the view); returns the entity and its prefab parts |
| `editor.material` | `entity`, `mesh`, `material` (a name or a number) | paints a model with a material of a loaded mesh file |
| `editor.delete` | `entities` | despawns them with everything that hangs off them |
| `editor.duplicate` | `entities`, `offset?` | copies them with everything that hangs off them; returns the copies |
| `editor.undo` / `editor.redo` | | steps back or forward through the history; a new change drops what was undone |
| `editor.save` | `path?` | saves the world as a scene, by default to the file it was loaded from or last saved to |
| `editor.camera` | `position?`, `look_at?`, `fov?`, `turn?` [dx, dy], `fly?` [right, up, forward], `focus?` (an entity) | moves the 3D camera as the editor's fly camera does; returns where it is |

Loading a scene or a save (by any means) forgets the history: its entity
ids no longer mean what they did. The inspector's **Stop** puts the world
back and keeps the history.

## Capabilities

`capabilities` answers, without changing anything, what this running build
supports, so a tool can check before sending a request that would fail:

- `mode`: `headless` or `windowed`; `engine_version`; `features`: every
  Cargo feature and whether this build has it.
- `subsystems`: `renderer`, `inspector`, `gamepad`, `physics`, `physics3d`,
  `audio`, `networking`, `profiling`, `control`. Each says whether it's
  compiled in and active, with a `reason` when it isn't. What isn't known
  is `null` with a reason (the GPU adapter before a GPU opens; physics
  before the game loop starts), never a guess.
- `methods`: every method with `available` and, when it would be refused
  here (`screenshot` in a headless game), the same `reason` the refusal
  gives; `limited` notes calls that are accepted but do less.
- `reflection`: reflected components and resources, each with a `schema`.
  Serde can't say what a type serializes to, so a type registered with
  `reflect::<T>()` is `"status": "opaque"` until the game describes it;
  types described with `reflect_fields` get theirs from the fields:

<div class="lang-pair">
<div>

```rust
# #[derive(Component, serde::Serialize, serde::Deserialize)] struct Health(f32);
ctx.world.reflect::<Health>();
ctx.world.describe_component("Health", ponko::reflect::ValueSchema::object([
    ("current", ponko::reflect::ValueSchema::Number),
    ("max", ponko::reflect::ValueSchema::Number),
])).unwrap();
// capabilities → { "name": "Health", "schema": { "status": "described",
//   "json_schema": { "type": "object", "properties": { "current": { "type": "number" }, ... },
//                    "required": ["current", "max"] } } }
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:describe_component}}
```

</div>
</div>

`ValueSchema` has a constructor for each kind of shape: `object` (fields
not marked `optional` are `required`), `strict_object` (an object that
refuses other fields: `"additionalProperties": false`), `enum_of` (one of
some strings), `any_of` (one of some shapes, like an enum's variants that
carry data), `array`, `optional`, and scalars such as `Number`, `Vec2` and
`Entity`. The game itself can read the same answer with
`ctx.capabilities_json()`.

The MCP tool `instance_capabilities` returns this plus `mcp.tools`: each
tool's availability for that game and server (a read-only server marks the
tools that change games unavailable). Against an engine from before this
method it reports `"supported": false` instead of guessing.

Components reflected from C and C++ with fields get schemas from those
fields. `ponko_world_describe_component` / `world.describe_component`
describe any other shape, and `ponko_ctx_capabilities_json` /
`ctx.capabilities_json()` return the answer (see [Agents](agents.md)).

## Questions about many entities

A simulation with a million units is too big to list. `entities.stats`
answers questions about all of them at once, from reflected fields:

```json
{"method": "entities.stats", "params": {
  "fields": ["Unit.hp", "Transform2D.position", "Unit.team"],
  "where": ["Unit.hp < 5"]}}
```

gives, for the matching entities, `count`, `min`, `max` and `mean` of
numbers (per axis for vectors like positions) and counts per value for
names and enums (`"values": {"Red": 120, "Blue": 98}`). A field path is a
component name, then fields, array elements by number:
`Transform2D.position.0` is x. A bare component name (`Team`) is its whole
value.

`where` takes conditions on field paths with `==`, `!=`, `<`, `<=`, `>`,
`>=` and a JSON value (a bare word is a string: `Unit.team == Red`). It
filters `entities.list` too. On the battle demo, stats over 1M units take
about 0.3 s (release); the game is paused for that long.

## Logs

While the endpoint is open, the engine keeps the game's last 5,000 lines of
stdout and stderr: `println!`, C and C++ `printf` and `std::cout`, panic
messages and the engine's own notes. The terminal still shows everything.
So a tool can read the output of a game you started yourself, from a
terminal or an IDE. `logs.get` is answered right away, not between frames,
so it works while a frame is stuck too.

Without `since` you get the newest `lines`. Pass the `next` of one answer as
`since` in the next request to get only what's new; `missed` counts lines
that were dropped before you read them. This works on macOS, Linux and
Windows. A Windows game built as a GUI program (no console) has no output to
keep; `logs.get` then says it isn't captured.

## Trying it by hand

```sh
PONKO_CONTROL=1 cargo run --example boids &
f=$(ls ~/.ponko/instances/*.json | head -1)
port=$(jq .port "$f"); token=$(jq -r .token "$f")
echo "{\"id\":1,\"token\":\"$token\",\"method\":\"info\"}" | nc -q1 127.0.0.1 "$port"
```
