# Working with AI agents (MCP)

`ponko-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server. It lets an AI coding agent (Claude Code, Cursor, Codex, ...) start
your game, look inside it while it runs, change it, and step the simulation,
instead of guessing from source code and print statements.

It builds on the [control endpoint](control.md) and [reflection](reflection.md).

## Setup

In this repository it's already configured: `.mcp.json` registers the server
for any MCP client that reads project config (Claude Code asks once to
approve it).

For your own game, install the binary and add it to your agent:

```sh
cargo install ponko_mcp
claude mcp add ponko -- ponko-mcp            # Claude Code
claude mcp add ponko-view -- ponko-mcp --read-only
```

Other clients take the same command (`ponko-mcp`, stdio transport) in their
MCP config.

Then make the components you want agents to see reflected (C++ lists the
fields, so tools also get their schema; C: `ponko_component_reflect`):

<div class="lang-pair">
<div>

```rust
# #[derive(Component, serde::Serialize, serde::Deserialize)] struct Health(f32);
ctx.world.reflect::<Health>();
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:reflect}}
```

</div>
</div>

## What an agent can do

| Tool | Changes the game | What it's for |
|---|---|---|
| `list_instances` | | find running games |
| `launch` | ✓ | build and start a game or example, or start an already built executable (`program`, e.g. a C or C++ game); headless and paused by default. Release builds it makes get the engine's `control-env` feature; a release `program` built without it can't be driven (see [Turning it on](control.md#turning-it-on)) |
| `instance_info` | | tick, sim time, paused, speed, fps, entity count, render CPU/GPU ms |
| `instance_capabilities` | | what this game can do before you try: headless or windowed, compiled features, subsystems and why one is off, which tools work on it (and why not), reflected types with their field schema ([details](control.md#capabilities)) |
| `world_summary` | | archetypes, entity counts, reflected components |
| `list_entities` | | entities with component values; filter by component names and `where` conditions (`Health.current < 5`), paged |
| `get_entity` | | one entity's components |
| `entity_stats` | | min, max, mean and value counts over every matching entity: "how many units per team, how hurt are they?" ([details](control.md#questions-about-many-entities)) |
| `list_resources` / `get_resource` | | game-wide state: scores, rules, the current round |
| `system_timings` | | which systems are slow |
| `ecs_diagnostics` | | archetypes, component storage, every system's phase, order, timings and declared access, and the schedule's stages with why each starts where it does ([details](#ecs-diagnostics)) |
| `perf_report` | | why frames are slow: fps and frame-time percentiles, CPU vs GPU, GPU time per pass, triangles drawn for the camera and the shadows, the heaviest meshes; `render_inspector` with GPU memory estimates, the adapter and (`history: N`) the last N drawn frames ([details](performance.md#the-render-pass-inspector)) |
| `set_graphics_quality` | ✓ | switch a 3D game's quality preset (`low` ... `ultra`, `recommended`) to measure what it costs between two `perf_report` calls ([example](performance.md#measuring-what-a-quality-level-costs)); changes only the picture (and pins render scale and shadow resolution against automatic tuning until the game restarts), but a read-only server doesn't offer it |
| `shader_status` | | did a shader edit take effect? Each custom shader's last compile or hot reload result, and its errors with line and column in your `.wgsl` file ([details](3d.md)) |
| `record_trace` | | the next N frames as a Perfetto trace, to find hitches |
| `perf_hud` | | show the performance overlay, e.g. before a screenshot |
| `list_events` | | event types the game shows tools, and how many are in flight |
| `subscribe_events` / `take_events` | | collect [events](events.md) while ticks run, then see what happened (each with its tick) |
| `read_logs` | | recent stdout/stderr of any running game, also one you started yourself ([logs](control.md#logs)); pass `next` back as `since` for only new lines |
| `screenshot` | | the next frame of a windowed game, as an image the agent can look at |
| `set_camera` | | move or zoom the camera (`look_at`, `zoom`, `position`), e.g. before a screenshot |
| `run_ticks` | ✓ | run exactly N ticks of a paused sim and wait |
| `watch` | ✓ | run N ticks and sample every K: tick, resources, entity counts, time per tick |
| `run_input_scenario` | ✓ | run a tick-indexed [input scenario](actions.md#input-scenarios) (key, mouse and gamepad steps plus checks) on a paused sim; returns the report, with the failing step, tick, expected and actual for a failed check. The keys it holds are released when it ends (`keep_input_when_done` keeps them until `cancel_input_scenario`). Needs a game that allows input injection (`launch` sets it) |
| `cancel_input_scenario` | ✓ | end a scenario and release the input it holds |
| `pause` / `resume` / `set_speed` | ✓ | sim clock |
| `spawn_entity` / `despawn_entity` | ✓ | |
| `set_component` / `remove_component` | ✓ | change values; object fields merge |
| `set_resource` | ✓ | change game-wide state; object fields merge |
| `save_world` | | checkpoint the world to a new file in the game's folder (`overwrite: true` replaces one, not on a read-only server) |
| `load_world` | ✓ | roll back to a checkpoint (same entity ids) |
| `save_scene` / `load_scene` | ✓ | write or read a [scene](scenes.md) in the game's `assets/` |
| `spawn_prefab` / `save_prefab` | ✓ | place a [prefab](scenes.md) at a point (with its children), or save an entity as one |
| `despawn_prefab` | ✓ | remove a placed prefab with its children |
| `pick_entity` | | what is under a window point of a 3D game, as a click in the editor picks it, and where that point's ray meets the ground |
| `scene_hierarchy` | | every entity as a tree: what it hangs off and how (attached, joint, skin, prefab part) |
| `list_assets` | | what the editor's asset browser lists: meshes, prefabs, scenes and materials |
| `place_asset` | ✓ | drop a glTF mesh or a prefab into the scene at a point, or where a window point's ray meets the ground, optionally on the grid |
| `transform_entities` | ✓ | move, turn and scale entities as the gizmo does (world or local axes, snapping), as one undo step |
| `drag_gizmo` | ✓ | drag a gizmo handle from one window point to another, exactly as the mouse does |
| `set_parent` | ✓ | make an entity follow another (or nothing) without moving it |
| `paint_material` | ✓ | paint a model with a material of a loaded mesh file |
| `duplicate_entities` / `delete_entities` | ✓ | copy or remove entities with everything that hangs off them, as one undo step |
| `undo_edit` / `redo_edit` | ✓ | step through the editor's history, shared with the [inspector](inspector.md#editing-in-3d) |
| `save_edited_scene` | ✓ | save the world back to the scene file it came from (or a new one) |
| `set_camera_3d` | | move, turn, fly or frame the 3D camera, e.g. before `pick_entity` or a screenshot |
| `quit_instance` | ✓ | |

`--read-only` hides every tool marked ✓, for agents that should only watch.

## ECS diagnostics

`ecs_diagnostics` (endpoint method `ecs.diagnostics`, Rust
`ctx.ecs_diagnostics()`) answers "why does this tick cost what it does?"
and "why doesn't this system run at the same time as that one?". It is
built only when asked: the schedule plan, the access lists and the
dependencies are worked out for the answer, and a running game keeps
nothing for it beyond its system timings and two counters per batch of the
[parallel scheduler](parallel-systems.md). The answer has a `version` (1);
field names stay stable and new ones may be added.

- `archetypes`: `index`, `components` (full type names), `entities`,
  `bytes_per_entity`, `allocated_bytes`. Empty ones are left out;
  `archetypes_total` counts all of them.
- `components`: `name`, `short_name`, `size`, `align`, and how many
  `archetypes`, `entities` and `bytes` hold it.
- `systems`, in run order: `name`, `phase` (`Input`, `BeforePhysics`,
  `Physics`, `AfterPhysics`, `SimLast`, `Frame`, `Animate`, `Late`, `Draw`),
  `order`, `sim`, `every`/`offset` (runs on ticks where
  `tick % every == offset`), `last_ms`/`avg_ms`, and where it stands in the
  schedule:
  - `access`: `exclusive` (no declaration, or declared exclusive: it may
    touch anything), `declared` (a `&mut Ctx` system with a
    [declaration](system-access.md): checked in debug builds, still alone)
    or `view` (a view system: it may share a batch).
  - `declared`: the declaration by name, or `null`: `components_read`,
    `components_written`, `resources_read`, `resources_written`,
    `events_read`, `events_sent`, `services_read`, `services_written`,
    `commands` and `exclusive`.
  - `stage`: index into `schedule.stages`.
  - `alone`: why it can't share a batch, or `null`. `reason` is
    `undeclared`, `ctx_system`, `service` or `sends_and_reads` (with the
    service or event type in `item`), or `interval`.
  - `blocked`: which system kept it out of the stage before its own, and
    why: `boundary`, `after` and `conflicts` as for a stage (below), plus
    `keeps_order`: it may not run ahead of system `after`, registered
    earlier and in the same stage, because their order shows on
    `conflicts` (two senders of one event type, say).
- `schedule`:
  - `parallel` (the scheduler is on), `threads` (it has other threads to
    use) and `parallel_threshold_us`.
  - `stages`, in run order: `index`, `phase`, `systems` (the `order` of
    each of its systems; they need not be neighbours, since a system that
    interacts with nothing registered between may run ahead), `first` and
    `len` (its first system and how many), `batch` (more than one system),
    and why the stage starts where it does: `boundary` is `phase` (first of its phase),
    `alone` (its system or the one before runs alone), `conflict` (its first
    system conflicts with system `after`, on `conflicts`), `barrier` (an
    explicit `Game::barrier`) or `order` (`Game::order` puts it after system
    `after`). A conflict has a `kind` (`component`, `resource`, `event`,
    `service`, `commands`, `exclusive`, `between`) and usually a `name`.
    `threaded_runs` and `inline_runs` count how a batch was run so far, and
    `usual_work_us` is the work the threshold was last compared with (less
    its slowest system).
  - `dependencies`: pairs of declared systems in one phase that must stay
    in order (`first`, `second`, `conflicts`). The endpoint lists at most
    1000; `dependencies_total` counts all.

  The stages are listed whether or not the scheduler is on; with it off,
  every system runs alone in `order`. Either way **every stage ends with a
  barrier**: its commands apply and its events become visible before the
  next stage starts.
- `query_cache`: the world's [archetype index](ecs.md) for queries:
  `enabled`, `min_archetypes`, `signatures`, `indexed`, `indices`, `bytes`,
  `hits` and `evictions`. `evictions` that keep growing mean the game runs
  more query signatures than the policy keeps.
- With `with: ["Pos", "Vel"]`, `matching` counts the archetypes and entities
  a query for those components visits.

Troubleshooting a slow tick, for example:

1. `system_timings` shows `game::steer` at 6 ms average.
2. `ecs_diagnostics` with `with: ["Transform2D", "Vel"]` gives
   `"matching": {"archetypes": 41, "entities": 20000}`, and `archetypes`
   lists 41 small ones that differ only by a `Stunned` marker component.
3. So the query walks many archetypes. The fix is a `bool` field instead of
   the marker; then `ecs_diagnostics` again: 1 archetype, and `steer`'s
   `avg_ms` drops.

Troubleshooting systems that don't run together: a game turned
`parallel_systems` on, and its tick takes as long as before.

1. `ecs_diagnostics`, then read `schedule`. `parallel` is `true`, so the
   scheduler is on. If `threads` were `false`, there would be nothing to
   run on (the web, one core).
2. Find the slow systems in `systems` and look at each one's `alone`:

   ```json
   { "name": "game::score", "order": 3, "access": "exclusive", "declared": null,
     "stage": 2, "alone": { "reason": "undeclared", "item": null } }
   ```

   `score` takes `&mut Ctx` and has no declaration, so it is a stage of its
   own and ends the batch before it. Rewriting it as a view system with a
   declaration (a function of `&mut SystemView`) lets it join one.
3. For a view system with `"alone": null` that still has no company, read
   its stage:

   ```json
   { "index": 1, "phase": "AfterPhysics", "first": 2, "len": 1, "batch": false,
     "boundary": "conflict", "after": 0,
     "conflicts": [{ "kind": "component", "name": "game::Vel" }] }
   ```

   System 2 (`friction`) writes `Vel`, which system 0 (`movement`) reads, so
   it waits for `movement`'s batch to end. `dependencies` lists every such
   pair. The fix depends on the game: split the component, or accept the
   order. The other systems of a batch each have their own reason in
   their `blocked`:

   ```json
   { "name": "game::wear", "order": 5, "stage": 1,
     "blocked": { "boundary": "conflict", "after": 4,
                  "conflicts": [{ "kind": "component", "name": "game::Grip" }] } }
   ```
4. For a stage with `"batch": true` that still doesn't speed up, read its
   counters:

   ```json
   { "index": 0, "first": 0, "len": 2, "batch": true, "boundary": "phase",
     "threaded_runs": 0, "inline_runs": 3, "usual_work_us": 0.084 }
   ```

   Every run stayed on the game's thread: the batch's work (0.084 µs) is far
   below `parallel_threshold_us` (50), where threads would cost more than
   they save. Nothing to fix; see
   [When it doesn't pay off](parallel-systems.md#when-it-doesnt-pay-off).

The same from Rust, in a test or a debug key handler:

```rust
# use ponko::prelude::*;
fn explain(ctx: &mut Ctx) {
    let d = ctx.ecs_diagnostics();
    for system in &d.systems {
        if let Some(alone) = &system.alone {
            println!("{} runs alone: {}", system.name, alone.reason);
        }
    }
    for system in &d.systems {
        let Some(blocked) = system.blocked.as_ref().filter(|b| b.boundary == "conflict") else {
            continue;
        };
        let earlier = &d.systems[blocked.after.unwrap()];
        println!("{} waits for {}: {:?}", system.name, earlier.name, blocked.conflicts);
    }
}
# Game::new("t", 1, 1).sim_system(explain).run_headless(1, |_| {});
```

From C, `ponko_ctx_ecs_diagnostics` writes the same answer as JSON, the
format `ecs.diagnostics` returns (call it with a null buffer to learn the
length); C++ `ctx.ecs_diagnostics()` returns it as a `std::string`. Take it
from inside a system: the systems list is empty during setup.

## Several agents, several games

Every game with the endpoint on registers itself in `~/.ponko/instances/`,
and every `ponko-mcp` reads that registry. So:

- each agent runs its own `ponko-mcp`, and all of them see all games;
- every tool takes `instance` (the id, pid, port or name from
  `list_instances`), optional when only one game runs;
- the game applies requests between ticks, one at a time, so agents never
  race the simulation or each other.

A typical split: one agent edits gameplay code and relaunches the game, while
a read-only agent watches system timings in a long-running instance.

## A session

An agent asked to "find out why boids clump at the edges" might do:

1. `launch` with `example: "boids"`. The game builds, starts headless and
   paused, and returns `{"id": "6257-57560", "name": "boids", ...}`.
2. `world_summary`: one archetype `[Transform2D, Sprite, Vel]` with 20,000 entities.
3. `run_ticks` with `ticks: 600` (10 simulated seconds), then
   `entity_stats` with `fields: ["Transform2D.position"]` and
   `where: ["Transform2D.position.0 < 20"]` to count how many are near the
   left border (and `list_entities` with the same `where` to look at some).
4. Edit the steering code, `quit_instance`, `launch` again, and compare.
5. `system_timings` to check the fix didn't make the tick slower.

Or, without relaunching: `save_world` at tick 0, `set_component` to try a
different value on some boids, `run_ticks 600`, look, then `load_world` to
roll back and try the next value.

Because the sim only advances on `run_ticks`, every run is repeatable and the
agent can check results at exact ticks.

To see how a run develops, `watch` does it in one call. In the battle
example, with its `Battle` stats reflected:

```json
{"ticks": 3600, "every": 600, "resources": ["Score"], "count": ["Unit"]}
```

returns seven samples (before, then every 10 simulated seconds), each with
the tick, the `Score` resource, how many entities have a `Unit`, and the
wall-clock time per tick, so "who's winning, and does it get slower as
the armies meet?" is one answer.

To check how things look, `launch` with `headless: false` and call
`screenshot`. It works while the window is hidden behind others, and is
scaled down to 1024 pixels by default (`max_size`) to keep it cheap to look
at. A screenshot of the ants example after 40 seconds showed food carriers
looping instead of going home; giving them a sense of where home is raised
deliveries in that time from 328 to 1,747.

## Making your game agent-friendly

- Reflect the components that matter for gameplay, the resources that
  hold game-wide state (`world.reflect_resource::<Score>()`), so an agent
  can ask "who's winning?" without counting entities, and the events
  (`ctx.events.reflect::<Died>()`), so it can see what happened, not only
  the state afterwards.
- Describe the shape of reflected types an agent should write
  (`world.describe_component("Health", ValueSchema::object([...]))`), so
  `instance_capabilities` lists their fields; otherwise they show as opaque.
- Read sizes and seeds from environment variables (like the examples'
  `BOIDS` and `HEADLESS`), so `launch` can set them through `env`.
- Support headless: put game logic in sim systems; it then runs without a
  window or GPU, which is faster for agents and works on CI.


## A reproducible development loop

Start with a small reproduction and record the engine revision, example or
game command, features, seed, input sequence and expected state. Use `launch`
with a controlled environment, inspect the reflected state, then `run_ticks`
to a fixed tick and record `get_entity`/`get_resource` results. Save a
checkpoint when useful; only reflected/serialized game data can be inspected
or restored through these tools.

After editing code, compile, run the relevant tests and relaunch the same
reproduction. Compare the same state and ticks. For a visual change, use a
windowed instance and record camera, resolution, quality and screenshots;
headless success does not verify rendering. Collect a `perf_report` or
`record_trace` when the change affects performance, and preserve comparable
before/after workloads. A screenshot alone does not establish simulation
correctness or a performance improvement.

### Validation bundles

`cargo run --example validation_bundle -- --out DIR --variant buggy|fixed`
writes one self-contained directory for a run: build identity, scenario and
seed, the commands and MCP calls made, each check's outcome with the failing
tick, logs, performance reports, screenshots of a windowed run, and the hash
of every file. It can run the game in-process or launch it and drive it
through the tools above, put a game of your own (Rust, C or C++) through a
scenario file, and compare two bundles. See
[Validation bundles](validation-bundles.md).

These tools exist today, including scheduler inspection (`ecs_diagnostics`),
shader diagnostics (`shader_status`) and GPU reports by pass (`perf_report`).
Their presence does not imply a general gameplay-input recording/replay
service. Reliable autonomous creation of complete games remains
a long-term objective, with human review and reproducible evidence still
required. See [the project vision](https://github.com/ponkoengine/ponko/blob/main/PROJECT_VISION.md).

### Instance schemas and GPU details

The read-only `capabilities` control request (and MCP `instance_capabilities` tool)
reports this instance's mode, compiled features, operation availability, and
reflected component/resource schemas. In a windowed renderer,
`subsystems.renderer.adapter` identifies the adapter/backend and
`subsystems.renderer.gpu` contains the same limits and feature fallback reasons
as `Gfx::gpu_capabilities()`. Before a GPU opens, those values are null and
`adapter_reason` explains why. Headless instances report an inactive renderer.
A tool being present in MCP does not imply that the selected instance supports it.

Built-in transforms, attachments, sprites, models, LOD, lights, decals and
registered physics components carry explicit schemas. Asset references use their
serialized file names; they are not GPU handles. Arbitrary Serde registrations
still work without extra bounds and remain explicitly opaque until described.
Schemas describe JSON shape, not asset existence or all value constraints.

C uses `ponko_world_describe_component(world, name, schema)` and
`ponko_world_describe_resource(world, name, schema)`; C++ uses
`world.describe_component(name, schema)` / `describe_resource(name, schema)`.
Names must already be reflected. The schema string encodes Rust's `ValueSchema`,
for example `{"Object":[["hp","Number"],["label",{"Optional":"String"}]]}`.
Scalar names include `Bool`, `Integer`, `Unsigned`, `Number`, `String`, `Entity`,
`Vec2`, `Vec3`, `Color`, `Null`, and `Opaque`. Containers are `Object` (ordered
name/schema pairs), `StrictObject` (an object that refuses other fields),
`Array`, `Optional`, `Tuple` (an array of schemas), `AnyOf` (alternative
schemas), and `Enum` (string values). Invalid JSON or an unknown
reflected name returns an error without replacing existing metadata. Describing
again replaces metadata only: serialization and field registration are unchanged.
A running game reads its own `capabilities` answer with
`ponko_ctx_capabilities_json(ctx, buffer, capacity, &len)` in C and
`ctx.capabilities_json()` in C++; the native tests check that a description
made from C and from C++ shows up there.

## The agent acceptance suite

The tools are tested call by call; the acceptance suite tests whether a
task can be *finished* with them. It lives in `crates/ponko_mcp/tests/suite/`:
six small game-development tasks, each with a starting project (files under
`assets/`), a goal written for an agent, a checker that scores the result
from the running game and its files, a scripted solution, and negative
controls: plausible wrong fixes that the checker must reject. The games
follow the 2D project template: rules in sim systems, numbers in JSON files
that hot reload, gameplay types reflected.

| Task | Kind | The agent must | Checked by |
|---|---|---|---|
| `tune-speed` | tuning | tune a data file so a run takes between 1.5 and 2 seconds | the file, the game's live value, an input scenario |
| `coin-level` | content | build a level with the tools and save it as a scene | the scene file, loading it, entity counts, a scenario's score |
| `immortal-enemy` | bug | reproduce an enemy that never dies and fix the bad value | the data file, live components, a fresh wave's `Died` events |
| `slow-crowd` | performance | find the system that got slow and cut its work tenfold | work counted per tick, and the same result as an all-pairs count |
| `broken-shader` | shader | fix a WGSL file while the game runs | `shader_status` and the file |
| `hidden-score` | ui | make the HUD show the score | what the UI shows, through `expect_ui` |

The performance task scores the work a tick does (distance checks), not
wall-clock time, so its score is the same on every machine; `system_timings`
is where an agent sees the slowness. The UI task checks what the player sees
through the UI tree in a headless game with frames
(`Game::headless_frames`), so it needs no window or GPU.

### Deterministic mode: in CI, no model

```sh
cargo test -p ponko_mcp --test agent_suite
cargo run -p ponko_mcp --example agent_suite -- replay   # the score table
```

Every task's scripted solution is replayed through `ponko_mcp::Server`
against a fresh copy of its starting project, and must pass every check;
every negative control must fail exactly the checks it names (an empty
"do nothing" control is required, so the starting state always fails). The
scores must equal `tests/suite/baseline.json`; after a deliberate change to
a task or a tool, rewrite it with `PONKO_UPDATE_BASELINE=1` and say why in
the pull request. Nothing leaves the machine; it takes about half a minute.

### The 0.2.0 baseline

The scripted solutions, with the 0.2.0 tools:

| Task | Checks | Steps | Negative controls caught |
|---|---|---|---|
| `tune-speed` | 3/3 | 7 | 4/4 |
| `coin-level` | 4/4 | 11 | 4/4 |
| `immortal-enemy` | 3/3 | 14 | 4/4 |
| `slow-crowd` | 4/4 | 8 | 4/4 |
| `broken-shader` | 2/2 | 5 | 3/3 |
| `hidden-score` | 2/2 | 6 | 3/3 |
| **total** | **18/18, 6/6 tasks** | 51 | 22/22 |

A step is one tool call, one file edit, or one wait for a hot reload. The
steps are a known path, not a bound: a real agent also explores. No run of a
real agent is recorded here yet; the next section is how to make one.

### Trying a real agent

This is opt-in and never runs in CI. It needs no keys and contacts
nothing: the agent is whatever MCP client you start, with its own account.
`agent_suite mcp TASK` is an MCP server over stdio that prepares the task's
folder, starts its game headless and paused in a child process, and offers
every ponko-mcp tool (except `launch` and `quit_instance`, which would
replace the task's game) plus `submit_task`, which runs the checker. Its
`initialize` reply carries the goal and the folder as instructions.

```sh
cargo build -p ponko_mcp --example agent_suite
cargo run -p ponko_mcp --example agent_suite -- list     # the tasks
cargo run -p ponko_mcp --example agent_suite -- goal tune-speed

# Claude Code, for example; any MCP client starts the same command.
claude mcp add ponko-suite -- "$PWD/target/debug/examples/agent_suite" \
    mcp tune-speed --dir /tmp/suite/tune-speed --agent claude-code
```

Start the agent in an empty folder, not in this repository, so it can't read
the checkers and solutions, and ask it to do the task in the `ponko-suite`
server's instructions. It edits files in the task's folder with its own
tools. When it calls `submit_task`, or when its client disconnects, the
result goes to `--out` (default `<dir>.result.json`): every check with what
was seen, the last submission's score, the number of submissions, and every
tool call with its arguments and error. The calls are the steps; the errors
and the failed checks show where it got stuck. Run each task a few times:
one run of a model says little.

### Turning failures into fixes

A real agent that fails a task points at a tool, an error message or this
guide. File an issue labelled `area: tooling` with the result file, fix
what misled the agent, and when a checker let a wrong fix through, add it
as a negative control. Building the suite turned up these, now in this
guide:

- **A paused game reads edited files only when it ticks.** Hot reload looks
  at files a few times a second, at the start of a tick: after saving a file,
  `run_ticks` with `ticks: 1` and read the value back (`get_resource`), and
  repeat until it changes. A file that doesn't parse is reported in
  `read_logs` ("could not reload ... keeping the old one"), and the game
  keeps the old values.
- **`shader_status` counts every reload, failed ones too.** Wait for
  `reloads` to change, then read `ok`, `outcome` and the diagnostics' line
  and column.
- **Input scenarios count ticks from their start.** To compare with a tick
  the game stores (such as a "reached at" tick), read `instance_info`'s
  `tick` before running the scenario.
- **What the player sees, headless:** a game with
  `Game::headless_frames(true)` runs its UI, and `expect_ui` steps check a
  node's `shown` and `text`, with no window.

### Limits

- The tasks change data files, scenes and shaders. None asks for Rust code
  changes: rebuilding a game per attempt takes minutes, too slow for CI;
  code tasks for real agents are future work.
- Screenshots need a window and a GPU; the suite checks what is shown
  through the UI tree instead, so it runs in CI.
- A checker is only as strict as its checks: the negative controls are
  what keeps them honest.
- The suite drives games through MCP, so it is the same for games written in
  C or C++; its own tasks are Rust games.
