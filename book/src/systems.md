# Systems and the frame

A **system** is a function `fn(&mut Ctx)` (or a closure); in C++, anything
callable as `void(ponko::ctx&)`, with an optional name for timings and tools.
A game is built from two things:

- **[Phases](#phases)** say *when* a system runs. Every system is in one
  phase; the phases run in a fixed order, and systems in the same phase run
  in the order you added them.
- **[Plugins](#plugins)** say *which* systems belong together. A plugin is
  one area of the game (the player, the HUD, the shop): its systems and its
  startup work, added with one call.

The engine's own work (physics, animation, drawing) is made of
[ordinary systems](#built-in-systems) in the same phases, so you can see,
time, remove and replace every step of the frame.

<div class="lang-pair">
<div>

```rust
# fn steer(_: &mut Ctx) {} fn movement(_: &mut Ctx) {} fn camera(_: &mut Ctx) {}
Game::new("Sim", 1280, 720)
    .sim_system(steer)      // fixed rate: game logic
    .sim_system(movement)
    .system(camera)         // once per frame: input, camera, UI
    .setup(|ctx| { /* spawn things */ })
    .run();
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:systems}}
```

</div>
</div>

## Two kinds of systems

| | `sim_system` | `system` |
|---|---|---|
| Runs | at the fixed rate, 0 or more times per frame | once per rendered frame |
| `ctx.time.dt()` | the fixed step (1/60 s by default) | real time since last frame |
| Use for | gameplay, physics, AI: anything that must behave the same at any FPS | input, camera, debug UI |
| In headless runs | runs | doesn't run |

Change the rate with `.fixed_hz(120)`. In C++, `sim_system(f, name, every)`
also takes a name and an [interval](#a-name-an-interval-a-declaration).

These two are short for the general form, `Game::add`, which the rest of
this chapter uses: `sim_system(f)` is `add(Phase::AfterPhysics, f)` and
`system(f)` is `add(Phase::Frame, f)`.

## Phases

Every system belongs to a `Phase`. Phases always run in this order, whatever
order the builder calls come in; within a phase, systems run in the order
you added them. `Game::add(phase, f)` (C++: `game::add(ponko::phase::..., f)`,
C: `ponko_game_add_system`) puts a system in any phase and returns the
game, so builder calls chain; `sim_system` and `system` are short for the
two most common ones. When you need the system's id later (to remove or
order it), add it with `Game::add_system` instead, which returns the id
(see [Replacing a built-in](#replacing-a-built-in)).

| Phase | Runs | Your systems | The engine's systems |
|---|---|---|---|
| `Input` | each fixed tick, first | | the spatial grid's rebuild |
| `BeforePhysics` | each fixed tick | what feeds the physics step: forces, velocities, wheel inputs | |
| `Physics` | each fixed tick | | the `physics` / `physics3d` step |
| `AfterPhysics` | each fixed tick | `sim_system` | |
| `SimLast` | each fixed tick, last | bookkeeping that must see the whole tick | |
| `Frame` | each rendered frame | `system`, the closure returned from `run`'s setup | interpolated physics transforms |
| `Animate` | each rendered frame | pose work, before the engine's | animators (`3d`), then particles |
| `Late` | each rendered frame | cameras and anything that follows a final pose (C++: `late_system`) | |
| `Draw` | each rendered frame | your own drawing | sprite animation, tilemaps, sprites, 2D particles; positional audio last (`3d`) |

Headless runs (`run_headless`) run the fixed-tick phases only.

The physics step always runs in `Physics`, so a `sim_system` runs *after*
it even when you add it before calling `.physics()` or `.physics3d()`.
Logic that must affect the current tick's step goes in `BeforePhysics`:

```rust
# fn steer(_: &mut Ctx) {} fn score(_: &mut Ctx) {}
Game::new("Drive", 1280, 720)
    .add(Phase::BeforePhysics, steer) // the step below sees this tick's input
    .physics3d(vec3(0.0, -9.81, 0.0))
    .sim_system(score)                // sees the result of the step
    .setup(|ctx| { /* spawn things */ })
    .run();
```

Code written when the step ran where `.physics()` was called, with
simulation logic added before it, should move that logic to
`.add(Phase::BeforePhysics, logic)`; otherwise it acts one tick late.

The animators, the particles and positional audio stay at the **end** of
their phase: a system you add to `Animate` runs before the engine poses
the skeletons, and one you add to `Late` or `Draw` runs before sounds are
placed, no matter when you add it. To run *after* the engine's pose, use the next
phase (`Late` for a camera that follows a bone).

### A name, an interval, a declaration

`Game::add` takes a system function as it is, or with options set on it.
The option methods come from the `IntoSystem` trait, which the prelude
imports:

```rust
# use ponko::access::SystemAccess;
# #[derive(Component)]
# struct Hunger(f32);
# fn think(_: &mut Ctx) {} fn get_hungry(_: &mut Ctx) {} fn follow(_: &mut Ctx) {}
let game = Game::new("Options", 1280, 720)
    // Listed as "ai" in timings and tools, instead of by the function's path.
    .add(Phase::AfterPhysics, think.named("ai"))
    // Every 10th tick; inside it `ctx.time.dt()` is 10 fixed steps.
    .add(Phase::AfterPhysics, get_hungry.every(10))
    // What it reads and writes: see "Declared system access".
    .add(Phase::AfterPhysics, get_hungry.named("hunger").access(SystemAccess::new().write::<Hunger>()))
    // A closure names its parameter type.
    .add(Phase::Late, |ctx: &mut Ctx| follow(ctx));
# let _ = game;
```

- `named(name)` names the system in timings, plans, reports and tools.
- `every(ticks)` runs it only every `ticks`-th tick, in the fixed-tick
  phases; [The simulation clock](sim-clock.md) has the details.
- `access(declaration)` says what it reads and writes
  ([Declared system access](system-access.md)).

A closure passed to `add` writes out its parameter type
(`|ctx: &mut Ctx| ...`), because `add` also takes
[view systems](system-access.md), whose parameter is a `SystemView`.
`sim_system` and `system` only take `Ctx` systems, so their closures need
no type. In C++ the same options are arguments of `game::add`.

## One frame, in order

1. The fixed-tick phases, `ctx.time.fixed_steps()` times: each tick runs
   `Input`, `BeforePhysics`, `Physics`, `AfterPhysics`, `SimLast`.
2. `Frame`.
3. `Animate`.
4. `Late`.
5. `Draw`.
6. The renderer draws what the frame queued.

There is nothing else: every step between the ticks and the renderer is a
system in one of these phases, and [`ctx.perf.system_timings()`](#timings) lists
all of them.

## Physics, navigation, characters and interpolation: who runs when

These four all move transforms, so their order decides what each one (and
your own code) sees in a tick. Nothing here is a separate movement
framework: they are the ordinary systems above, in the phases above.

| Step | System | Where it runs | What it does to the tick |
|---|---|---|---|
| Physics | `step_physics`, `step_physics3d` | `Physics`, added by `Game::physics` / `physics3d` | picks up new, changed and removed bodies, simulates, writes poses and velocities back, sends `CollisionStarted` / `CollisionStopped` |
| Character motion | inside `step_physics3d` | `Physics` | every `d3::Character` moves by its `velocity` first, then the bodies are simulated |
| Navigation | `step_navmesh3d` | where you add it | moves every `d3::NavAgent`'s transform along its path |
| Interpolation | `interpolate_physics3d` | first in `Frame`, added by `Game::physics3d` | between two steps, sets moving bodies' `d3::Transform` to a blend of their last two poses |

What follows from that:

- **Input for the step goes before it.** Forces, velocities, a
  character's `velocity`, a transform you move (a teleport) and commands
  that spawn or despawn bodies reach *this* tick's step from `Input` and
  `BeforePhysics`. From `AfterPhysics`, `SimLast` and the frame phases they
  reach the next tick's step. Commands are applied after the system that
  recorded them, so a despawn from `BeforePhysics` is gone before the step
  and its body leaves with that step.
- **Results are read after it.** Poses, velocities, `grounded` and physics
  queries are this tick's from `AfterPhysics` on.
- **Collision events follow the [event rules](events.md).** The step sends
  them while it runs: a reader in `AfterPhysics` or `SimLast` sees a contact
  in the tick it began, a reader before the step sees it one tick later,
  and a reader with an interval (`.every(n)`) sees each one once, on its first
  run after the step. Events are kept at least as long as the slowest
  interval.
- **A step that runs every `n` ticks covers `n` ticks.** Take the step out
  (`Game::remove(Systems::PHYSICS)`) and add
  `systems::step_physics.every(n).access(systems::step_physics_access())`
  to `Phase::Physics`: its `ctx.time.dt()` is `n` fixed steps, and its events are
  still read once. Without the declaration it runs alone, like any
  undeclared system.
- **Navigation moves transforms; it doesn't push.** An agent has no
  collision response, so don't give it a dynamic body or a character
  controller: both would write the same transform, and an agent that finds
  its transform changed by anything else plans again from where it is. A
  kinematic body or a sensor that only *follows* an agent is fine: add the
  navigation step in `BeforePhysics`, and the physics step moves the body to
  the agent's new place in the same tick (as a teleport). Added after the
  step (`sim_system`), the body, and every physics query, is one tick
  behind the agent. To walk a `d3::Character` along a path, ask the mesh
  for the path (`NavMesh::find_path`) and set the character's `velocity`
  in `BeforePhysics`; leave `NavAgent` off that entity.
- **Destinations and obstacles go before the navigation step**, in the
  same tick, wherever you put it.
- **The blend is for drawing.** `interpolate_physics3d` runs before your
  frame systems, so cameras, animation and drawing see the smooth pose. It
  only changes `d3::Transform`; `ctx.physics3d.position(e)` stays the
  stepped pose. Simulation systems *after* the step always read the stepped
  pose. One *before* the step reads whatever the last frame drew, which in
  a window is the blend, a little behind the stepped pose (headless runs
  never blend). Gameplay that needs the exact pose before the step reads
  it from `ctx.physics3d`. A transform you change between steps is left
  alone by the blend and teleports the body at the next step; an entity
  you removed is skipped, and its body leaves at the next step.

`tests/step_inputs.rs` holds each of these as a test.

### What the steps touch

The physics and navigation steps [declare their access](system-access.md):
`systems::step_physics_access()`, `step_physics3d_access()`,
`interpolate_physics3d_access()` and `step_navmesh3d_access()` list the
service, components, resource and events each one uses, and nothing else.
`Game::physics` and `Game::physics3d` register their systems with these
declarations, so `Game::access_report()` shows them and debug builds check
that the steps stay inside them. They still take `&mut Ctx` and run alone,
in order. When you place a step yourself, pass the declaration along:

<div class="lang-pair">
<div>

```rust
# use ponko::systems;
let game = Game::new("Agents", 1280, 720)
    .add(Phase::BeforePhysics, systems::step_navmesh3d.access(systems::step_navmesh3d_access()))
    .physics3d(vec3(0.0, -9.81, 0.0));
# let _ = game;
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:builtin_access}}
```

</div>
</div>

In C, `ponko_game_add_builtin` adds a built-in by its id with its
declaration, and `ponko_builtin_access` returns a copy of a built-in's
declaration (free it with `ponko_access_free`). The built-ins without a
declaration of their own return an exclusive one.

## Built-in systems

The functions are in `ponko::systems`; the ids in `Systems` name them for
`Game::remove` and `Game::system_count`.

| Function in `ponko::systems` | Id | Added by | Phase |
|---|---|---|---|
| `rebuild_spatial_grid` | `Systems::REBUILD_SPATIAL_GRID` | `Game::spatial_grid` | `Input` |
| `step_physics` | `Systems::PHYSICS` | `Game::physics` | `Physics` |
| `step_physics3d` | `Systems::PHYSICS3D` | `Game::physics3d` | `Physics` |
| `interpolate_physics3d` | `Systems::INTERPOLATE_PHYSICS3D` | `Game::physics3d` | `Frame` |
| `update_animators` | `Systems::UPDATE_ANIMATORS` | always, with `3d` | end of `Animate` |
| `update_particles` | `Systems::UPDATE_PARTICLES` | always | end of `Animate` |
| `update_audio3d` | `Systems::UPDATE_AUDIO3D` | always, with `3d` | end of `Draw` |
| `animate_sprites` | `Systems::ANIMATE_SPRITES` | always | `Draw` |
| `draw_tilemaps` | `Systems::DRAW_TILEMAPS` | always | `Draw` |
| `draw_sprites` | `Systems::DRAW_SPRITES` | always | `Draw` |
| `draw_particles` | `Systems::DRAW_PARTICLES` | always | `Draw` |
| `follow_attachments` | `Systems::FOLLOW_ATTACHMENTS` | you | where you add it |
| `step_navmesh3d` | `Systems::STEP_NAVMESH3D` | you | where you add it |
| `apply_net_poses` | | you | where you add it |

In C the ids are `PONKO_SYSTEM_DRAW_SPRITES` and so on; in C++,
`ponko::systems::draw_sprites`.

Before these functions had their own module they were at the crate root;
those spellings are gone, so import them from `ponko::systems`
([Naming and compatibility](naming.md#the-root-glob)).

### Replacing a built-in

`Game::remove` takes a built-in out by its id. Add your own system, or the
same function in another place (C++ `game::add_builtin`, C
`ponko_game_add_builtin`, with the built-in's id):

<div class="lang-pair">
<div>

```rust
# fn my_draw(_: &mut Ctx) {}
Game::new("t", 800, 600)
    .remove(Systems::DRAW_SPRITES)   // draw your own way
    .add(Phase::Draw, my_draw);

// Tilemaps over the sprites instead of under them.
Game::new("t", 800, 600)
    .remove(Systems::DRAW_TILEMAPS)
    .add(Phase::Draw, systems::draw_tilemaps);
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:remove_system}}
```

</div>
</div>

Removing an id that isn't installed does nothing, so a plugin can remove a
built-in without knowing whether the game turned that feature on.

Your own systems have ids too: `Game::add_system` returns one (C++:
`game::add_system`; C: `ponko_game_add_system_id`). Two systems made from
the same function get different ids, so each can be removed by itself:

```rust
# fn debug_overlay(_: &mut Ctx) {}
let mut game = Game::new("t", 800, 600);
let overlay = game.add_system(Phase::Draw, debug_overlay);
// later, while still building the game:
let game = game.remove(overlay);
# let _ = game;
```

`add_system` is `add` that returns the id. It is the one builder call that
borrows the game instead of taking and returning it, so it doesn't chain.
Removal is by id only: a function or a name can't tell two registrations
apart.

## Plugins

A `Plugin` is one module of a game: `build` adds its systems (each in the
phase it needs) and `setup` does its startup work once the game is
running. `Game::plugin` adds it:

<div class="lang-pair">
<div>

```rust
# #[derive(Default)] struct Score(u32);
struct Player { speed: f32 }

impl Plugin for Player {
    fn build(&self, game: Game) -> Game {
        let speed = self.speed;
        game.add(Phase::BeforePhysics, move |ctx: &mut Ctx| {
            let dt = ctx.time.dt();
            ctx.world.query::<&mut ponko::d2::Transform>().for_each(|t| t.position.x += speed * dt);
        })
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.world.spawn((ponko::d2::Transform::default(),));
    }
}

struct Hud;

impl Plugin for Hud {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::Draw, |ctx: &mut Ctx| {
            let score = ctx.world.resource::<Score>().0;
            ctx.gfx.draw_text(&format!("{score}"), vec2(16.0, 16.0), 24.0, Color::WHITE);
        })
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.world.insert_resource(Score::default());
    }
}

Game::new("Modules", 1280, 720)
    .plugin(Player { speed: 4.0 })
    .plugin(Hud)
    .configured() // startup is in the plugins: no setup of its own
    .run();
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:plugin}}

{{#include ../../examples/cpp/guide.cpp:plugin_use}}
```

</div>
</div>

In C++ a plugin is any object with `build(ponko::game&)`; `setup(ponko::ctx&)`
is optional. In C, `ponko_game_plugin(game, build, setup, user)` takes the
two callbacks.

What to rely on:

- **`build` runs at once**, inside the `plugin` call. Everything it adds
  names its phase, so the order of the `plugin` calls only matters between
  two systems in the same phase.
- **`setup` runs once**, when the game starts and the window and the
  renderer exist (headless: before the first tick). Plugin setups and
  [`Game::setup`](getting-started.md#setup-on-the-builder) callbacks run in the order they were
  added; the setup passed to `run` comes last.
- **Commands recorded in any setup** are applied before the first system
  runs, so one plugin's systems can count on what another's setup spawned.
- A plugin is plain data: there is no registry, no dependency list and
  nothing to enable. A plugin that needs another one says so in its docs,
  or adds it itself in `build` with `game.plugin(Other)`.

A game built from several files usually ends up as one plugin per file
and a `main` that lists them; `examples/coin_dash` is built this way.

## When changes become visible

Systems run one at a time, each with all of `ctx`. Between two systems
the engine applies the first one's commands, so **every system is a
barrier**: within a phase, across phases, and between ticks. (The one
exception is opt-in: with [parallel systems](parallel-systems.md) on, view
systems that share a batch have their barrier at the end of the batch;
see the last rows.)

| What | Becomes visible |
|---|---|
| A change through `ctx.world` | immediately |
| A command in `ctx.commands` | when the recording system returns, before the next system; never to the recording system itself |
| Commands recorded in `setup` | before the first tick |
| Commands recorded in a `Game::setup` callback | when that callback returns, before the next callback: every setup callback is a barrier too |
| Commands recorded before a [`load_scene`](scenes.md#what-a-load-replaces-and-what-it-keeps) in the same system | never: the load drops them, they were meant for the old entities. Commands recorded after it apply as usual |
| Events sent before a `load_scene` | as before the load, for their usual lifetime; entity ids in them name the old world's entities |
| A sent event | immediately, to every later `read` (also the sender's own) |
| An event, to a system earlier in the order | on that system's next run (the next tick for simulation systems) |
| Events from simulation ticks, to frame systems | in the same frame, all ticks of it |
| Events from frame systems, to simulation systems | on the next tick that runs |
| A change to a tracked component, to `Changed<T>`, `Added<T>` and `removed::<T>()` | to every other system, the next time it runs; never to the system that made it (see [Change detection](ecs.md#change-detection)) |
| A command recorded in a parallel batch | when the batch ends, before the next stage; the batch's commands apply in registration order |
| An event sent in a parallel batch | when the batch ends, in registration order; a later reader of that type is never in the same batch |

Commands apply in the order they were recorded: a closure from
`commands.add` sees the ones before it, and an `insert` after a `despawn` of
the same entity does nothing. Engine steps such as the physics step are
ordinary systems, so commands recorded in `Phase::BeforePhysics` are in the
world when physics runs. A system skipped by its interval (`.every(n)`) doesn't run
and records nothing.

C and C++ systems follow the same rules, with the scheduler off or on (they
always run alone). A query callback may record commands (`*_later`), but it
can't read or change the world it is walking: world calls return
`PONKO_BUSY` (C++ throws) until the query ends, so a callback never sees a
half-applied change. Parallel queries refuse commands.

<div class="lang-pair">
<div>

```rust
# use ponko::prelude::*;
#[derive(Component)]
struct Shot(i32);

// Records a spawn. The world doesn't change until this system returns.
fn fire(ctx: &mut Ctx) {
    let before = ctx.world.len();
    ctx.commands.spawn((Shot(3),));
    assert_eq!(ctx.world.len(), before); // still pending
}

// The next system finds the shot in the world, in the same tick.
fn count_shots(ctx: &mut Ctx) {
    let shots = ctx.world.query::<&Shot>().count() as u64;
    assert_eq!(shots, ctx.time.sim_tick() + 1);
}
# Game::new("t", 1, 1).sim_system(fire).sim_system(count_shots).run_headless(3, |_| {});
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:command_barrier}}
```

</div>
</div>

So the next system sees an event together with the commands its sender
recorded: a `Died(e)` sent with `commands.despawn(e)` arrives when `e` is
already gone.

Events are dropped at the start of a frame (each tick in headless runs) once
they are at least one whole frame old *and* older than the slowest
system interval (`.every(n)`). Headless, with every system running every
tick, an event from tick `t` is kept through tick `t + 1`. While the
simulation is paused no ticks pass, so events sent by frame systems (a pause
menu's "restart") wait for the simulation readers.

`tests/schedule_barriers.rs` pins these rules, with the parallel scheduler
off and on; `tests/scene_lifecycle.rs` pins the scene rows, and
`examples/cpp/tests.c` and `tests.cpp` the C and C++ ones.

### Writing systems that keep working

The flush after every system is the contract for `&mut Ctx` systems, and it
stays that way: turning [parallel systems](parallel-systems.md) on changes
nothing for them. Only a system you rewrite as a *view system* (one that
takes `&mut SystemView`) gets the coarser barrier at the end of its batch.
Systems written like this work under both:

- Depend on another system's commands or events through explicit order: put
  the reader in a later phase, rather than relying on registration order
  inside one phase.
- Read results of your own commands on your next run, not in the same call.
- Keep work that needs the world exactly as it is in one system, through
  `ctx.world` (immediate) or one `commands.add` closure.

### Moving a system to a view system

A `&mut Ctx` system can lean on the flush after every system without saying
so. When you turn it into a view system, check these, in this order:

| The `&mut Ctx` system relied on | As a view system |
|---|---|
| Seeing the commands of the system registered just before it | Still true, and nothing to do, if that system *declares* `commands()`: a recorder is always the last of its batch, so the next system starts a new stage. An undeclared command is a panic naming the system, not a silent change. |
| Seeing the events of the system registered just before it | Still true if both declare the event type (`send_events` and `read_events`): a sender never shares a batch with a later reader. |
| Seeing another system's *direct* world changes | Still true if both declare the component or resource: a reader and a writer never share a batch. |
| Changing the world at once through `ctx.world` (spawn, despawn, insert, remove) | Not possible in a view: structural changes go through `sys.commands` and apply when the batch ends. Read the result on the next run, or keep the system a `&mut Ctx` system. |
| Sending an event and reading it back in the same call | Works: a view system that sends and reads one type runs alone and sees its own events at once. |
| Order that comes from nothing the declarations show (a file both write, a global, a callback) | Not kept. Say it: `Game::order(first, second)` or `Game::barrier(phase)`. |

So the declarations carry the order the flush used to give for free. Two
ways to check a conversion: `Game::schedule_plan()` (or
[`ecs_diagnostics`](agents.md#ecs-diagnostics) on a running game) shows
which systems share a batch and why each stage starts where it does, and
running the game's tests with `PONKO_PARALLEL_SYSTEMS=0`, `1` and `always`
must give the same results, since the scheduler never changes what a
correctly declared simulation computes.

Nothing has to move: a game can keep every system a `&mut Ctx` system, or
convert only the heavy ones. A `&mut Ctx` system between two view systems
is a barrier of its own, exactly as before.

## Parallel queries and parallel systems

Systems execute sequentially by default. A system can use the ECS's parallel
query methods to divide its entity work across threads; this is not
parallel execution of different systems. Systems may declare what
they read and write, and view systems (which take `&mut SystemView`) get only what they
declared; see [Declared system access](system-access.md). With
`Game::parallel_systems(true)`, view systems that don't conflict run at the
same time; see [Parallel systems](parallel-systems.md) for the rules and
for when it pays off. Existing unannotated `sim_system`
callbacks remain usable and exclusive.
See [the architecture](https://github.com/ponkoengine/ponko/blob/main/ARCHITECTURE.md)
for current subsystem boundaries.

## Systems with state

A system can be a closure that owns data. Build it with a function that
returns the closure:

<div class="lang-pair">
<div>

```rust
fn spawner(every: u64) -> impl FnMut(&mut Ctx) {
    let mut spawned = 0;
    move |ctx| {
        if ctx.time.sim_tick() % every == 0 {
            ctx.commands.spawn((ponko::d2::Transform::default(),));
            spawned += 1;
        }
    }
}

Game::new("t", 800, 600).sim_system(spawner(30));
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:stateful}}
```

</div>
</div>

## Timings

`ctx.perf.system_timings()` lists every system with its last and average time in
milliseconds, so a game can show its own profiler overlay. The engine's
systems are in the list under their names (`ponko::systems::update_animators`,
`ponko::ecs::draw_sprites`), next to yours, in the order they run. For a full
timeline, build with `--features profiling` and open
[puffin_viewer](https://github.com/EmbarkStudios/puffin) on `127.0.0.1:8585`.
