# Ponko vs Bevy vs Unity

**Last reviewed: 2026-11-14.** Ponko was renamed before 0.1.0 (the
[naming chapter](naming.md) maps the old names). This comparison describes
the checked-in implementation and the linked official documentation; it does
not claim feature parity or general performance superiority. Product versions
and tool availability change.

Ponko aims to make a capable 2D/3D engine accessible through a small,
code-first public API and integrated runtime tools. Its differentiator is
that combination of choices. Code-first development, ECS, headless runs,
modularity and remote inspection are also available in other engines.

## The programming model

Ponko uses Rust functions or closures taking `&mut Ctx`; a system that
[declares what it uses](system-access.md) can take a restricted view instead.
The same engine has a C ABI and C++17 wrapper. Ordinary games start with
`Game::new`, optional builder settings, and `run`; no plugin framework or
large visual editor is required, and `ponko new` makes a project from a
template. See [getting started](getting-started.md) for a complete game and
[C and C++](c-cpp.md) for the equivalent native API.

Bevy is also a code-first Rust engine. Its `App`, systems and plugins form
its application model. `DefaultPlugins` already supplies ordinary engine
facilities; developers do not have to assemble those individually. It also
supports selecting or replacing plugins. See the official
[plugin guide](https://bevy.org/learn/quick-start/getting-started/plugins/).

Unity's usual workflow combines C# scripts, components, assets, scenes and
its visual Editor. This is often useful for teams that author scenes and
content visually. Unity also provides an Entities package for data-oriented
work, so its GameObject workflow is not its only programming model. The
[Unity manual](https://docs.unity.com/en-us/engine/6000.0/manual)
and [Entities concepts](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/concepts-intro.html)
describe those alternatives.

## ECS and parallelism

Ponko stores component columns in archetypes and gives entities generational
IDs. Typed queries, optional components, `with`/`without` filtering, runtime
component registration and parallel query iteration exist today in
`crates/ponko_ecs`. A small fixed-step simulation can be checked without a GPU:

<div class="lang-pair">
<div>

```rust
use ponko::prelude::*;

#[derive(Component)]
struct Pos(f32);

fn fall(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    ctx.world.query::<&mut Pos>().for_each(|p| p.0 -= 10.0 * dt);
}

#[test]
fn things_fall_ten_units_per_second() {
    let mut world = Game::new("test", 800, 600)
        .sim_system(fall)
        .setup(|ctx| {
            ctx.world.spawn((Pos(100.0),));
        })
        .run_headless(60);
    let y = world.query::<&Pos>().iter().next().unwrap().0;
    assert!((y - 90.0).abs() < 1e-3);
}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:headless}}
```

</div>
</div>

This uses the existing Rust and C++ headless APIs. Parallelizing entities
*within* a query is different from running independent systems concurrently.
By default Ponko runs systems one at a time, in registration order inside each
phase. An optional scheduler can run some of them at the same time, as
described below; arbitrary `&mut Ctx` callbacks remain exclusive.

Bevy's ECS uses typed system parameters to express access, supports table
and sparse-set component storage, and schedules compatible systems in
parallel. Explicit ordering still matters. See
[Bevy ECS](https://docs.rs/bevy_ecs/0.20.1/bevy_ecs/) and its
[system scheduling tutorial](https://bevy.org/learn/quick-start/getting-started/ecs/).
Ponko has declared system access and restricted system views (see
[Declared system access](system-access.md)) and an optional scheduler
that runs non-conflicting view systems at the same time
(`Game::parallel_systems`, see [Parallel systems](parallel-systems.md)). It
is off by default, batches only neighbouring systems in registration order
rather than building a dependency graph, and leaves `&mut Ctx` systems
exclusive: unrestricted mutable contexts are never shared between threads.
Systems written in C and C++ take the whole `ctx`, so they always run alone,
though they can declare their access by name.

Unity Entities supplies ECS systems and archetype/chunk storage; Unity's
[Job System](https://docs.unity.com/en-us/engine/6000.0/manual/scripting/programming-distribute-work-threads/job-system)
and [Burst](https://docs.unity3d.com/Packages/com.unity.burst@1.8/manual/index.html)
provide parallel and optimized code paths. Comparing Ponko's ECS against
Mono GameObject callbacks does not measure Unity Entities/Jobs/Burst.

## Rendering and customization

Ponko's custom wgpu renderer already draws 2D sprites and 3D meshes with
materials, lighting, shadows and post-processing. WGSL surface shaders,
full-screen effects and compute workloads have existing APIs. Large static
instance batches use GPU culling on supported devices, with device-dependent
fallbacks. See [3D](3d.md), [profiling](profiling.md) and the source modules
listed in [ARCHITECTURE.md](https://github.com/ponkoengine/ponko/blob/main/ARCHITECTURE.md).

Those features do not establish graphics parity. The 3D renderer is split
into subsystems that each own their pipelines, GPU objects and state, with
`Renderer3D` coordinating the frame (see
[ARCHITECTURE.md](https://github.com/ponkoengine/ponko/blob/main/ARCHITECTURE.md)).
Pipelines are cached per shader, variant, target format and device features,
and GPU capability tiers have documented fallbacks (see
[GPU tiers](3d.md#gpu-tiers)). A game can add its own GPU work at named
stages of a 3D frame: compute passes, target cameras, scene compute passes,
target effects and screen effects (see [Custom passes](3d.md#custom-passes)).
Those stages are not a render graph, and the renderer's own passes are not
replaceable through them; shader hooks are not full renderer replaceability.

Bevy also uses wgpu. Its renderer exposes extraction, preparation and
rendering infrastructure plus render-graph and material extension points;
see [bevy_render](https://docs.rs/bevy_render/0.20.1/bevy_render/).
Unity provides multiple render pipelines and documented customization,
including [URP](https://docs.unity.com/en-us/engine/6000.0/manual/render-pipelines/universal-render-pipeline/urp-introduction)
and [HDRP](https://docs.unity3d.com/Packages/com.unity.render-pipelines.high-definition@17.0/manual/index.html).
Choose from actual visual requirements, target hardware, tooling and
maintenance cost. A shared graphics API or one favorable scene is not an
overall renderer comparison.

## AI development, inspection and evidence

Ponko's `ponko-mcp` server exposes tools for launching games and examples,
inspecting reflected entities/resources, modifying scenes, stepping paused
simulations, reading logs, capturing windowed screenshots and requesting
performance reports. The [agent guide](agents.md) documents their real names
and inputs. The optional inspector and control endpoint complement ordinary
debugging. A headless run (`Game::run_headless`) has no GPU, so it cannot
produce a rendered screenshot.

For example: launch a seeded reproduction paused, capture a checkpoint,
run a fixed tick count, inspect entity state, change code, rebuild and rerun.
For a visual change, also capture a windowed frame at the same camera and
quality settings. State inspection, successful compilation and screenshots
answer different questions; none substitutes for all the others.

Bevy's official [Remote Protocol](https://docs.rs/bevy_remote/0.20.1/bevy_remote/)
allows inspection and modification of ECS data through JSON-RPC, including
queries and schema discovery. This is real runtime tooling; Ponko does not
have an exclusive claim to remote ECS inspection. BRP and MCP are different
protocols, so client integration must be checked rather than assumed.

Unity provides an official AI Assistant and MCP integration. Its current
[CLI migration guidance](https://docs.unity.com/en-us/unity-cli/replace-mcp-server-unity-cli)
says the MCP server of the in-editor AI assistant package is deprecated and
replaced by the CLI's `unity mcp` command. This is a tool migration, not
removal of Unity's agent support. Follow the current Unity setup and access
requirements.

Ponko's tools include structured shader diagnostics (`shader_status`, with
the line and column in your WGSL), schedule and query inspection
(`ecs_diagnostics`) and GPU reports by pass (`perf_report`). Reliable
autonomous completion of arbitrary games is not an established capability.

## Multiplayer, determinism and servers

Ponko includes UDP lockstep, snapshots/replication and message-oriented
network APIs, with optional Steam integration. These are building blocks;
a shipped game's authority, prediction, recovery and hosting behavior still
need game-specific validation. Headless `Game::run_headless` runs fixed
simulation ticks without rendering. Disabling default features removes the
window/GPU path; enabling only what a server needs is supported today.

Read [networking](networking.md), [headless execution](headless.md) and
[determinism](determinism.md) before choosing a synchronization model.
Reproducibility assumes matching builds, seeds and inputs. Cross-platform
physics determinism requires the appropriate feature; floating-point
transcendentals can differ between platforms. These are not blanket
bit-identical-world guarantees.

Bevy lets an application choose which plugins it adds (see the plugin guide
above), so check the current release for what a headless or server
application needs, and evaluate networking against the library and release
you choose. Unity has both
[desktop headless and Dedicated Server workflows](https://docs.unity.com/en-us/engine/6000.0/manual/platform-specific/dedicated-server/desktop-headless-mode)
and an official [multiplayer stack](https://docs.unity.com/en-us/multiplayer)
covering netcode and player connections. Headless operation and multiplayer
are not unique to Ponko.

## Assets, portability and ecosystem

Ponko has direct asset loading, scenes and prefabs (prefabs can hold
children), glTF model loading, Tiled and LDtk levels that spawn as tilemaps,
and a Unity text-scene importer with FBX-related processing. Textures load as
PNG and JPEG by default, and as other formats through `image-*` cargo
features. External tools such as Blender remain appropriate for content
creation. Importing supported scene data does not run arbitrary Unity
scripts, packages or shaders. Texture and mesh residency budgets,
asynchronous decoding with a per-frame upload budget, texture resolution
streaming and streamed LOD levels are described in
[Streaming](streaming.md); live handle counts and leak checks for assets are
in [Handles](handles.md). Saves and settings go in a per-user data folder
with crash-safe writes, kept in the browser's storage on the web (see
[Saves and settings](storage.md)). Do not infer a complete production asset
pipeline from a successful import of one asset.

Ponko's tests run in CI on Linux, macOS and Windows. The browser target
(WebGPU with a WebGL2 fallback) is built on every change, and small web
builds are run in headless Chromium and WebKit in the merge queue. That does
not mean that every desktop feature works on every browser. Mobile and
consoles are not currently validated release targets.
[STABILITY.md](https://github.com/ponkoengine/ponko/blob/main/STABILITY.md)
is the compatibility contract: ordinary 0.x minor releases may break the API
and C ABI, with each break listed under Breaking changes in the CHANGELOG.

Bevy has a broader public contributor/plugin ecosystem; Unity has a much
longer production history, commercial content/tooling ecosystem and broader
platform workflows. These maturity assessments are practical judgments,
not benchmark results. Check the maintained
[Bevy ecosystem directory](https://bevy.org/assets/),
[Unity platform documentation](https://docs.unity.com/en-us/engine/6000.0/manual/platform-specific)
and required third-party integrations for your actual project.

## Choosing an engine

Choose Ponko when its compact Rust/C++ interface, integrated simulation and
runtime-control workflow fit your project, and you are willing to validate
missing production requirements and contribute to an early engine.

Choose Bevy when you want a Rust ECS-centered ecosystem with automatic
compatible-system scheduling and more existing plugin integrations. Both
engines support code-first development and useful defaults.

Choose Unity when visual content authoring, established production tools,
team experience, commercial integrations or supported deployment targets
are decisive. Its modular packages and high-performance data-oriented path
should be evaluated alongside its familiar GameObject workflow.

Use [the performance chapter](performance.md) as a record of specific
reproducible workloads, not a league table of engines. Match workloads,
resolution, quality, build configuration and target hardware before drawing
conclusions. Consult [the roadmap](https://github.com/ponkoengine/ponko/blob/main/ROADMAP.md)
for unresolved foundations; it is directional and has no promised dates.
