# Ponko architecture

Ponko integrates an archetype ECS, fixed-step simulation, a wgpu renderer and game services behind a code-first Rust API. C and C++ enter through a separate FFI crate; inspection tools use a local control protocol. This describes the source in this checkout, not the target architecture in [ROADMAP.md](ROADMAP.md). Source inspection confirms that mechanisms exist; it does not establish production readiness, performance on your hardware or feature parity across targets.

Package names and source directories use `ponko_*`. Native code uses
`include/ponko.h` and `include/ponko.hpp`; the old native names were removed in
0.1.0 ([naming](book/src/naming.md)).

```mermaid
flowchart TD
    Game[Game builder and loop] --> Ctx[Ctx: world and game services]
    Ctx --> ECS[ponko_ecs World]
    Ctx --> Services[Assets, input, audio, networking, physics]
    ECS --> Collect[3D collection and 2D draw systems]
    Collect --> Renderer[wgpu renderer]
    Renderer --> Output[Window, render targets, capture]
    Headless[Headless runner] --> Ctx
    MCP[ponko_mcp] --> Control[Local control endpoint]
    Control --> Ctx
    Native[C and C++ application] --> FFI[ponko_ffi]
    FFI --> Game
```

## Crate and ownership boundaries

| Boundary | Current responsibility and source |
|---|---|
| `ponko` | Public facade and integrated game services: [src/lib.rs](src/lib.rs), [src/ctx.rs](src/ctx.rs), [src/game.rs](src/game.rs). Feature flags select windowing, audio, inspector, physics, navigation and 3D. |
| `ponko_ecs` | Entities, column storage, queries, commands and resources: [crates/ponko_ecs/src](crates/ponko_ecs/src). It does not own the window or GPU. |
| `ponko_macros` | Derive macros (`#[derive(Component)]`) re-exported by `ponko_ecs` and `ponko`: [crates/ponko_macros](crates/ponko_macros). |
| Rendering | Device/surface and frame coordination in [src/render/gpu.rs](src/render/gpu.rs); 3D coordination in [src/render/gpu3d.rs](src/render/gpu3d.rs); game-facing 3D types in [src/three](src/three). |
| `ponko_mcp` | Stdio MCP server, tool schemas, instance discovery and forwarding to a running game: [crates/ponko_mcp/src/lib.rs](crates/ponko_mcp/src/lib.rs). |
| `ponko_ffi` | Exported native API and handle/callback conversion: [crates/ponko_ffi](crates/ponko_ffi), [include/ponko.h](include/ponko.h), [include/ponko.hpp](include/ponko.hpp). |
| `ponko_unity` | External asset/scene conversion tooling: [crates/ponko_unity](crates/ponko_unity). This is an importer, not a Unity runtime or C# compatibility layer. |
| `ponko_bench` | Development benchmarks: [crates/ponko_bench](crates/ponko_bench). Results need workload, hardware, build and feature context. |

The engine is integrated rather than a collection of independent service crates. `Ctx` gives systems broad access to state, while the 3D collector bridges ECS components and GPU-oriented frame data. These convenient boundaries also create coupling: a scheduler cannot infer a system's access from its closure signature, and renderer ownership changes can affect assets, temporal history and pass sequencing together.

## Game loop and scheduling

[Game](src/game.rs) stores systems as `FnMut(&mut Ctx)` callbacks. `run_systems`, `run_sim_tick` and `run_one` define the current order:

1. Poll the message endpoint if the game has one (`Networking::begin_frame`), update retained UI (`Ctx::frame_begin_services`) and begin frame timings.
2. Run the due fixed simulation ticks. Lockstep can defer a tick until peer inputs are ready while the windowed frame continues.
3. Run `Frame` systems, then `Animate` systems; with 3D, the built-in systems `update_animators` and `update_particles` end `Animate`.
4. Run late systems, then drawing systems; with 3D, the built-in system `update_audio3d` (positional audio) ends `Draw`.
5. The windowed application prepares and submits rendering through [src/game](src/game) and [src/render](src/render).

Step 1, and the asset reload and streaming step before it (`Ctx::frame_prepare_services`, called by both loops), are the engine's own service steps: named methods of `Ctx` in [src/ctx.rs](src/ctx.rs), called only from the loops, so each one's place in the frame is one line. Built-in animation, particles and positional audio are ordinary systems ([src/systems.rs](src/systems.rs), ids in `Systems`): listed in timings, removable with `Game::remove` and re-addable elsewhere, and pinned at the end of their phase (`System::last`) so a game's own systems in that phase run before them whenever they were added. [Service ownership and lifecycle](#service-ownership-and-lifecycle) lists every service.

`Game::add(Phase, system)` is the one registration entry point; it selects an
explicit sequential bucket. Each fixed tick runs `Input`, `BeforePhysics`,
`Physics`, `AfterPhysics`, then `SimLast`. The `sim_system` shortcut uses
`AfterPhysics`; installing physics always places the step in `Physics`,
independent of builder order. The `system` shortcut uses `Frame`; pose fixes go
in `Late`; built-in drawing uses `Draw`. Within a
phase registration order is preserved, except that the pinned built-ins above
stay at the end of `Animate` and `Draw`.

`run_one` applies deferred commands after each system. An interval (`system.every(n)`, `IntoSystem`) schedules a system at a tick interval and phase; it is simulation frequency control, not parallel scheduling. Events have reader/lifetime machinery in [src/events.rs](src/events.rs). Changes to barriers must preserve the currently observable command and event behavior.

**Systems execute sequentially by default.** Parallel ECS iteration and native parallel GPU command recording are separate mechanisms. Systems can declare their access (`SystemAccess`, [#137](https://github.com/ponkoengine/ponko/issues/137)); view systems (functions of `&mut SystemView`, registered through the same `Game::add`) get a restricted `SystemView` that enforces the declaration. The optional scheduler ([#138](https://github.com/ponkoengine/ponko/issues/138), `Game::parallel_systems`) puts each phase's systems into stages ([src/schedule.rs](src/schedule.rs), a pure plan tested apart from execution): a system alone, or a batch of view systems with no conflict. Phases, explicit barriers and systems that run alone cut the registration order into segments that nothing crosses; inside one, each view system goes to the earliest stage after every earlier system whose work it must see (`conflicts_before`, `Game::order`) and not ahead of any whose order would show (`conflicts_with`), so batches follow the dependency graph rather than adjacency ([#388](https://github.com/ponkoengine/ponko/issues/388)). Every system of a segment still writes in the change tick it would have had run one at a time. `&mut Ctx` systems stay segment cuts: their declarations are only checked in debug builds. [src/game/batch.rs](src/game/batch.rs) runs a batch through `ponko_ecs::ViewGroup`, the only place where views share a world; each system gets its own command buffer and event outbox, delivered at the batch's end in registration order, and a batch runs through the same code on several threads or on one, so results don't depend on threads. The barrier contract is in the guide's "Parallel systems" chapter. Arbitrary `&mut Ctx` callbacks remain exclusive; annotations alone cannot make concurrent mutable access sound, so only view systems are ever batched.

Fixed steps and seeded random streams support repeatability, but deterministic gameplay also depends on user code, math and physics configuration. Follow [STABILITY.md](STABILITY.md), including its same-build lockstep and physics restrictions. A future scheduler must define ordering and reductions before claiming equivalent determinism.

## Service ownership and lifecycle

`Ctx` is one struct that holds the services as public fields; it is built once per run and lives until the loop ends. The loops (`Game::run_systems`, `run_sim_tick`, [window.rs](src/game/window.rs) and `run_headless_inner` in [game.rs](src/game.rs)) are the only callers of the engine steps below. Nothing here is a public registry: games reach services through the fields.

### Lifecycle, in order

Rows follow `window.rs`'s frame (`run_frame`) and `run_headless_inner`'s tick.

| Stage | Windowed run (`Game::run`) | Headless run (`Game::run_headless`) |
|---|---|---|
| Construct | `Game::build_ctx(false)`: `Ctx::new`, then seed, hot reload, spatial grid, physics worlds, lockstep session and message endpoint (`Game::start_networking`, `Net::begin`) | `Game::build_ctx(true)`: the same, with the headless flag set (size-only textures, silent audio, dropped draws) |
| Control endpoint | `Game::start_control` (the loop owns the `Control`, not `Ctx`) | The same, and, with the endpoint on, a paused game waits for tools to step it |
| Renderer | `App::start` attaches `ctx.gfx.renderer` once the window and GPU exist; setup waits for it | None: `ctx.gfx.renderer` stays `None` |
| Setup | `run_setups` (plugins and builder setups in order), then the `run` closure; `Ctx::set_in_setup`; commands applied when setup returns | The same; the closure's returned frame function is dropped (there are no frames) |
| Fixed tick | `Game::run_sim_tick`, which ends with `Ctx::advance_tick` | The same, exactly one per loop iteration; sim speed ignored |
| Frame start | `Ctx::begin_frame`, gamepad poll, `Ctx::frame_prepare_services` (asset reload, streaming), then `Graphics::apply_shader_checks` (3D) | `Ctx::begin_frame` and `frame_prepare_services` each tick; no gamepad poll, no shader checks |
| Frame | `Game::run_systems`: UI, ticks, frame and animate systems (ending with animation and particles), late, draw (ending with positional audio) | Not run: no UI update, no frame, animate, late or draw systems, so no animation, particles or positional audio |
| End of systems | `Input::end_frame`, then the inspector overlay; a quit request or cursor-lock change is handled here | `end_timings`, `Graphics::discard_draws`, `Input::end_frame` |
| Render | `Ctx::draw_perf_hud`, `Ctx::render_frame`, `Ctx::record_frame` | None: no renderer, nothing to draw or time |
| Teardown | The event loop returns; `App` and its `Ctx` drop | `run_headless` returns the world (`std::mem::take(&mut ctx.world)`); the rest of the `Ctx` drops |

### Who owns each service

Every service is constructed by `Ctx::new` unless the row says otherwise. "Scope" is the state's lifetime: **world** state is replaced when a scene or world is loaded (`Ctx::replace_world_now` in [scene.rs](src/scene.rs): the world and its commands, the physics body mapping, 3D panel focus and temporal history); **game** state lives for the whole run; **device** state belongs to the window, GPU or sound output and is absent headless.

| `Ctx` field | Scope | Tick entry | Frame entry | Teardown owner |
|---|---|---|---|---|
| `world`, `commands` | world | Systems through `run_one`; `Ctx::apply_commands` after each system | The same for frame, late and draw systems; `draw_world_sprites` queues sprites | Headless: returned by `run_headless`. Windowed: dropped with `Ctx` |
| `events` | game | `Ctx::advance_tick` sets the tick | `Ctx::begin_frame` (`Events::begin_frame`) | Dropped with `Ctx` |
| `time` | game | `Ctx::advance_tick`; `set_in_sim` and `set_sim_steps` around systems | `frame_timing`, then `Ctx::begin_frame` | Dropped with `Ctx` |
| `input` | game (gamepads: device) | Read by systems; scenario hooks `scenario_begin_tick` / `scenario_end_tick` in `run_sim_tick` | Window events and the gamepad poll feed it; `Input::end_frame` clears the frame's edges | Dropped with `Ctx` |
| `spatial` | game | `rebuild_spatial_grid`, an `Input`-phase system added by `Game::spatial_grid`; cell size set in `build_ctx` | None | Dropped with `Ctx` |
| `physics`, `physics3d` | game; body mapping is world | The step system in `Phase::Physics`; `physics::start` / `physics3d::start` in `build_ctx` | None | Dropped with `Ctx` |
| `net` (lockstep session, message endpoint, snapshot streams, interest) | game | `Net::ready` gates each tick, `Net::end_tick` after it; `Net::begin` in `build_ctx` | `Networking::begin_frame` polls the message endpoint before any system, in both loops (headless: every pass, also while paused or while a tick waits). The frame runs on while a tick waits | `Drop for Net` (keeps resending the last inputs for a short time); a dropped message endpoint is noticed by its peers' timeout |
| Render state: `camera`; `scene3d` (`SceneSettings`: 3D camera, lighting, sky, post, LOD, extra passes, automatic quality) | game | None | Read by `Ctx::render_frame` and `prepare_3d` | Dropped with `Ctx` |
| `ui` (retained tree, immediate widgets) | game | None | `frame_begin_services` (`update_retained_ui`) before any system; the immediate widgets reset in `begin_frame`; drawn in `render_frame` (`draw_retained_ui`) | Dropped with `Ctx` |
| `gfx` | device (renderer); game (caches, draw lists) | None; headless discards draws | `begin_frame` (text and frame number); `render_frame` submits and clears the lists | Dropped with `Ctx`; the renderer drops with it. `Ctx::free` releases one texture or font |
| `audio` | device (backend); game (loaded sounds) | None | Positional update by the `update_audio3d` system at the end of `Draw` (3D only); the backend opens its device on first load or play | Dropped with `Ctx`; headless and the no-`audio` build never open a device |
| `assets` (sources, hot reload) | game | `frame_prepare_services` each tick, headless included | The same, each frame | Dropped with `Ctx` |
| `perf` | game | `begin_timings`, `end_timings` | The same, plus `record_frame` and `end_perf_frame` | `Drop for PerfState` stops scope collection |

`gfx` and `audio` keep what they have loaded in one private owner, `AssetCache` ([asset_cache.rs](src/asset_cache.rs)): handles by where a name was found (`Origin`) and which loader ran (`AssetKind`), so loading a name again returns the same handle and never reads an asset source again. The services pass in the shared asset sources and the hot-reload watch list; the cache owns neither. Freed handles are forgotten through `AssetCache::release`.

Services share state through values the constructor hands them (the asset sources and the watch list are shared handles; sprite and mesh file names are shared with the world) and through `Ctx::sync_services`, which runs before each system and hands services what they answer from: the camera for the 3D listener, the mouse for the UI pointer, the tick for residency. A service never borrows `Ctx`, so game code can hold several fields at once.

Out of scope here: the loops' own state (`Clock`, `Control`, the window and the inspector overlay live in the loop, not in `Ctx`), and the scheduler's batches, which hand view systems separate borrows of `Ctx` fields ([#137](https://github.com/ponkoengine/ponko/issues/137), [#138](https://github.com/ponkoengine/ponko/issues/138)).

## ECS storage and queries

[World](crates/ponko_ecs/src/world.rs) manages generational entity locations and archetypes. [Storage](crates/ponko_ecs/src/storage.rs) holds type-erased component columns; structural changes move entities between layouts, following cached transition edges between archetypes. A world keeps at most 64 empty archetypes, or as many as it has archetypes with entities if that is more; past that, the one empty longest is reclaimed ([#347](https://github.com/ponkoengine/ponko/issues/347)). Its slot stays in the archetype list with no components and is reused for the next new component set, with the reclaimed storage (id lists, column headers, edge maps, and pooled column buffers and entity lists), so a world that keeps visiting new component sets stops allocating once it settles. Archetype indices therefore name an archetype only until the next structural change: entity locations only name archetypes with entities, edges carry the generation of the slot they lead to, which reclaiming moves on, so edges to a reclaimed archetype are stale and get replaced, the matching index is told about every reused slot, bundle archetypes and the empty archetype are never reclaimed, and change detection names no archetype. Query order follows slot order, so it can change as slots are reused, the same way for the same operations. Queries that scan skip empty archetypes through a list of the ones with entities while many are empty ([#348](https://github.com/ponkoengine/ponko/issues/348)). The design and its invariants are documented in [slots.rs](crates/ponko_ecs/src/slots.rs); measurements in [SPARSE_SET.md](crates/ponko_bench/SPARSE_SET.md). This custom storage remains the baseline, rather than a candidate for replacement without measurements.

[Typed queries](crates/ponko_ecs/src/query.rs) resolve component IDs, validate aliasing, match archetypes and fetch row references. They support shared/mutable references, optional components, entity IDs, tuples up to eight elements and bounded `with`/`without` filtering. Matching scans archetypes in small worlds. From 64 archetypes on, the world keeps a [matching index](crates/ponko_ecs/src/query/cache.rs): per canonical query signature (required components and normalized `with`/`without`/`Or` filters, typed or runtime, without change filters), the ascending list of candidate archetype indices, extended incrementally as archetypes are appended and updated when a reclaimed slot is reused. A signature is indexed after two uses when it matches at most half of the archetypes; the table is bounded in signatures and memory and drops unused entries. `Query::cached()`/`uncached()` and `World::set_query_cache_policy` override the policy per query and per world. Loops still test their own filters, so the index never changes results or order. World views only read the index (lock-free); what they scanned for is noted and indexed when the world is next held exclusively. Measurements: [QUERY_CACHE.md](crates/ponko_bench/QUERY_CACHE.md). Column pointers are resolved under each live query borrow. [Change detection](crates/ponko_ecs/src/change.rs) is opt-in per component: a tracked column keeps two tick stamps per row behind its values, `Added`/`Changed` filters pick rows inside matching archetypes, and removals go to a per-component log. A world that tracks nothing keeps no stamps and runs the same loops as before.

Rayon-backed query methods partition disjoint rows: the calling thread lines up the matching rows of every archetype in query order (a reused, world-owned work plan), then one parallel pass runs chunks of it, so many small archetypes share chunks and big ones split. Indexed parallel operations preserve their documented output ordering, but callers must make per-entity work safe and avoid nondeterministic external effects. If the mapper of `par_collect_into` panics, the results already written are dropped and the output is left empty with its allocation kept ([#142](https://github.com/ponkoengine/ponko/issues/142)). For queries over small archetypes only, the choice between the calling thread and the pool depends on the measured time of the first rows, so the threads and order that run per-entity side effects are not reproducible; indexed positions and collected order are. See [queries tests](crates/ponko_ecs/tests/queries.rs), [parallel tests](crates/ponko_ecs/tests/parallel.rs), and [#133](https://github.com/ponkoengine/ponko/issues/133) for the broader proposed differential/Miri suite.

## Scene preparation and renderer

[Collection](src/three/collect.rs) turns models, transforms, lights, terrain, skinning and camera state into a reusable `Frame3D`. It sorts instances into mesh runs for camera and shadow views, selects LOD and maintains frame-related caches. Reused vectors reduce repeated allocation; that is distinct from general incremental extraction of unchanged ECS objects.

[Renderer3D](src/render/gpu3d.rs) coordinates the frame; the work and the state belong to subsystems under [src/render/gpu3d](src/render/gpu3d), each owning its pipelines, GPU objects and state. The renderer hands a subsystem what it needs, never itself.

| Responsibility | Owner | Lives |
|---|---|---|
| Meshes, materials, texture arrays, GPU buffers | `Resources` ([resources.rs](src/render/gpu3d/resources.rs)) | Until the game frees them |
| Material shaders: composing, variants, pipelines, warming | `Shaders` ([shaders.rs](src/render/gpu3d/shaders.rs), [variants.rs](src/render/gpu3d/variants.rs), [pipeline_cache.rs](src/render/gpu3d/pipeline_cache.rs)) | Until the shader is freed or replaced |
| The game's render targets, the scene's images, frame bind groups | `RenderTargets`, `Targets` ([targets.rs](src/render/gpu3d/targets.rs)) | Until freed; the scene's until its size or passes change |
| Shadow maps and what each view keeps | `ShadowMaps`, `ShadowCache` ([shadows.rs](src/render/gpu3d/shadows.rs), [shadow_cache.rs](src/render/gpu3d/shadow_cache.rs)) | Until resized; the cache from frame to frame |
| Static and instanced batches, GPU culling | `StaticBatch`, `Batching` ([instances.rs](src/render/gpu3d/instances.rs)) | Until the batch is freed |
| Pass preparation: named steps, in order | `Renderer3D::prepare` ([prepare.rs](src/render/gpu3d/prepare.rs)) | One frame |
| This frame's draw lists | `FrameLists` ([lists.rs](src/render/gpu3d/lists.rs)) | One frame; capacity is kept |
| The frame's description: each pass's steps, the resources they use, their formats, and the order | `FRAME_PASSES` ([passes.rs](src/render/gpu3d/passes.rs)) | Constant |
| Holding a drawn frame to that description, in debug builds | `FrameChecks` ([frame_checks.rs](src/render/gpu3d/frame_checks.rs)) | One frame |
| Pass encoding | `Renderer3D::draw` and `record` ([record.rs](src/render/gpu3d/record.rs)) | One frame |
| Temporal history: jitter index, last frame's matrices, camera cuts | `History` ([history.rs](src/render/gpu3d/history.rs)) | Frame to frame, until a reset |
| Game screen and target effects | `Effects` ([effects.rs](src/render/gpu3d/effects.rs)) | Pipelines until freed; lists one frame |
| The post chain: bloom, tone mapping, antialiasing, scaling | `PostChain`, `PostTargets` ([post.rs](src/render/gpu3d/post.rs)) | Pipelines for good; images with the scene's |
| Screen-space passes (temporal antialiasing, ambient occlusion, reflections) and which run this frame | `ScreenSpace` ([screen_space.rs](src/render/gpu3d/screen_space.rs)) | Pipelines once first used; flags one frame |

`prepare` (`&mut self`) uploads and decides everything for the frame; `draw` (`&self`) only records. State that outlives a frame is in `History` and the temporal passes' own histories, and one decision a frame (`History::decide`) drops all of it together. Adding a post effect touches `post.rs`, and its declaration in the frame's description (`passes.rs`), alone: its module documentation walks through one. The chain's choices and images are private to it; the renderer asks it only whether its images are stale and how many draws it makes, and a [render test](src/render/post_links_render_test.rs) holds both answers for every link switched on and off. The screen-space passes are less separate: `ScreenSpace` holds their state, but pass preparation decides their flags and `targets.rs` makes their images, so a new one still touches those. Global illumination, the sky, particles, skinning and occlusion culling remain fields of the renderer, each a type of its own.

The `Part` enum expresses recording stages: prelude/compute and atmosphere, shadows, target-camera views, GI, depth prepass, opaque, optional scene-copy/transparency and post-processing. Exact work depends on enabled features. Temporal antialiasing and AO use motion/history data; SSR, GI and post effects have dedicated modules. Sprites and UI composite afterward.

Native builds can record parts using Rayon; browser GPU handles stay on one thread. [Static instances](src/render/gpu3d/instances.rs) already have compute culling and indirect drawing on supported devices. This does not mean every model uses a fully GPU-driven scene pipeline. CPU/downlevel paths and device limits remain part of the contract.

The renderer already has effect modules and limited extension APIs for surface shaders, screen effects and compute: [three/api.rs](src/three/api.rs), [effect.rs](src/three/effect.rs), [compute.rs](src/three/compute.rs). Games add their own GPU work at named stages of the 3D frame (compute, target cameras, scene compute, target effects and screen effects; see the guide's "Custom passes"), with declared reads and writes checked every frame; there is no arbitrary pass insertion. `Part` is not a general validated render graph: [passes.rs](src/render/gpu3d/passes.rs) describes the parts for the renderer's own checks (each pass's steps, the frames they are recorded in, the resources they use and how, each resource's format, sample count and lifetime), and debug builds check every drawn frame and what each part records against it; it schedules and allocates nothing.

The resource, pipeline, preparation, encoding, frame-lifetime and post-processing responsibilities are separate ([#136](https://github.com/ponkoengine/ponko/issues/136)); the intended evolution continues with lightweight explicit internal dependencies between passes. It retains wgpu and the existing public workflow. [#134](https://github.com/ponkoengine/ponko/issues/134), [#135](https://github.com/ponkoengine/ponko/issues/135), [#136](https://github.com/ponkoengine/ponko/issues/136), [#147](https://github.com/ponkoengine/ponko/issues/147) and [#148](https://github.com/ponkoengine/ponko/issues/148) describe the foundations and next steps; no giant public graph or mandatory plugin framework is assumed.

## Assets, scenes and lifetime

[assets.rs](src/assets.rs) resolves native files relative to asset roots; [asset_cache.rs](src/asset_cache.rs) remembers what each name loaded as; web startup prefetches an asset index into memory. [Textures](src/texture.rs), [3D assets](src/three/api.rs), [glTF](src/three/gltf.rs), and [hot reload](src/hot_reload.rs) connect decoded data and handles to runtime resources. [Scenes and prefabs](src/scene.rs) use reflected JSON components; [reflection](src/reflect.rs) is also shared by inspection and world serialization. Saving an ECS world is not a promise to serialize every external service or GPU allocation automatically.

[Job and CellStreamer](src/stream.rs) support background CPU work and cell load/drop decisions. GPU creation belongs on the game's thread after preparation. Browser jobs execute synchronously in the current implementation. These helpers are useful building blocks, but do not constitute automatic texture mip/mesh residency management with a GPU memory budget; that remains [#154](https://github.com/ponkoengine/ponko/issues/154). Ownership/leak diagnostics remain an audit area in [#161](https://github.com/ponkoengine/ponko/issues/161).

## Physics, networking and other services

[2D physics](src/physics.rs) and [3D physics](src/physics3d) integrate Rapier behind features. `Game::physics` and `physics3d` install fixed-step systems; ECS components and physics state synchronize at that boundary. Characters, joints and vehicles extend the 3D integration. [Navigation](src/nav3d) uses optional Recast/Detour functionality. These services are not implemented by the ECS scheduler itself.

`ctx.net` is the [Networking](src/net/service.rs) service: it owns the lockstep session ([Net](src/net.rs)), the optional [message](src/net/messages.rs) endpoint (polled by the engine once per frame), [snapshot](src/net/snapshots.rs) streams and [interest](src/net/interest.rs); optional Steam transports sit below lockstep and messages. The two transport seams are internal and stay separate; their contracts, and what they share, are written down next to their scripted conformance tests ([lockstep](src/net/lockstep_conformance.rs), [messages](src/net/message_conformance.rs)). Peer coordination, authority and replication choices remain game-level responsibilities. Loopback tests do not prove internet, relay or live Steam-client behavior.

[Audio](src/audio.rs) uses Kira when enabled and a silent API otherwise. [Input](src/input.rs), [actions](src/actions.rs), [gamepad](src/gamepad.rs), [retained UI](src/ui) and [text](src/text.rs) form integrated game-facing services. Platform support for each backend must be checked separately; their presence does not establish accessibility completeness.

## Headless execution and AI observation

`Game::run_headless` creates a context, runs setup and fixed simulation ticks, and returns the world. Frame, late and drawing systems do not run, and neither do the retained-UI update or positional audio, which belong to frames. Changed asset files and streaming still run each tick. The lifecycle table above compares both loops stage by stage. With default features disabled, [render/none.rs](src/render/none.rs) supplies the no-GPU path; the audio API can remain silent. Headless behavior tests therefore complement GPU tests rather than substitute for them.

The opt-in [control endpoint](src/control.rs) exposes runtime operations at engine-controlled boundaries. [MCP tools](crates/ponko_mcp/src/lib.rs) discover instances, launch games, inspect/edit reflected state, operate scenes, pause/step, capture screenshots, retrieve logs and performance information. Reflection registration determines what component data tools can understand. [Profiling](src/perf_report.rs), [scope instrumentation](src/scope.rs) and [GPU timers](src/render/timer.rs) provide existing evidence sources.

The complete autonomous edit/build/observe/repair loop is a development direction. Custom shader errors and reload results are available as data through `Graphics::shader_status` ([shader_status.rs](src/three/shader_status.rs)) and the `shader_status` MCP tool. Scheduler explanations and a richer render-pass inspector remain backlog work. Screenshots require a rendering-capable instance; a headless state assertion cannot establish visual correctness. See the [agent guide](book/src/agents.md) and [roadmap](ROADMAP.md).

## Rust, C and C++ compatibility

Rust applications use `ponko`; native applications call the FFI exports directly or through the header-only C++ wrapper. C++ callbacks and dynamic component columns bridge into the same world and loop. The headers retain existing ABI names. Rust-only contributions may land independently. Maintainers coordinate native coverage and record follow-ups; existing binding compatibility must remain correct. Do not infer universal parity from the existence of the wrapper.

[STABILITY.md](STABILITY.md) defines the versioning, saved-world, MSRV, determinism and MCP compatibility policy. Package renaming does not authorize changing ABI symbols, persisted format tags or established tool contracts. Architectural improvements must preserve these boundaries or undergo an explicit compatibility decision.
