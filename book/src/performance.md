# Performance and benchmarks

Numbers from a MacBook Pro (M4 Max), release builds. Reproduce them yourself;
every number here has a command next to it.

These are historical measurements made under the Ponko name, retained
without rerunning them for the Ponko documentation update. They are specific
workload results, not current-version guarantees or proof of overall engine
superiority. See [the engine comparison](comparison.md) and baseline issues
[#132](https://github.com/ponkoengine/ponko/issues/132) and
[#134](https://github.com/ponkoengine/ponko/issues/134) for measurement scope.

## ECS iteration

`cargo run --release -p ponko_bench`

| Benchmark (1M entities) | Ponko | bevy_ecs | hecs |
|---|---|---|---|
| iterate `(&mut Pos, &Vel)` | 0.15 ms | 0.36 ms | 1.77 ms |

`par_for_each` on a heavy per-entity workload, 10M entities: 8.1 ms on all
cores vs 57.6 ms on one.

The full ECS matrix (1k/100k/1M entities: iteration, parallel scaling,
spawning, spawn/despawn and add/remove churn, optional and filtered queries,
and a game-like tick, each against bevy_ecs and hecs, with allocation counts)
records machine-readable results with one command:

```sh
PONKO_BENCH_JSON=crates/ponko_bench/baselines/ecs.json \
  cargo run --release -p ponko_bench -- 'ecs/|archetypes-1024|query-build'
```

`crates/ponko_bench/ECS.md` has the methodology, the limits of each
comparison and the committed baselines. `cargo test -p ponko_bench` checks
that every workload does the same work in all three libraries; timings are
never checked in CI.

## Headless simulation

`AGENTS=1000000 cargo run --release --example headless`

| Agents | ms per tick |
|---|---|
| 1M | 1.0 |
| 10M | 8.7 |

## Boids (neighbor search)

`HEADLESS=1 BOIDS=<n> cargo run --release --example boids`

| Boids | ms per tick |
|---|---|
| 20k | 0.9 |
| 100k | 3.7 |
| 1M | ~33 |

## Ant colonies (whole game loop)

`HEADLESS=1 ANTS=<n> cargo run --release --example ants`: steering with
pheromone sensing, trail laying, foraging, colony growth.

| Ants | ms per tick |
|---|---|
| 5k | 0.22 |
| 50k | 1.2 |
| 1M | 15 |

## Flow fields (pathfinding)

`cargo run --release -p ponko_bench -- flow` for the field itself;
`HEADLESS=1 AGENTS=<n> cargo run --release --example flow` for units
following it around walls and mud, with a new goal every 2 seconds.

| What | Time |
|---|---|
| `compute`, 256 × 256 cells | 1.25 ms |
| `compute`, 1024 × 1024 cells | 13.0 ms |
| 1M `direction_at` lookups, parallel | 0.29 ms |
| 20k units, per tick | 0.14 ms |
| 1M units, per tick | 3.2 ms |

## Battle (the flagship demo)

`HEADLESS=1 UNITS=<n> TICKS=1200 cargo run --release --example battle`:
two armies march along flow fields, keep their spacing, find enemies with
per-team spatial grids, fight, and rebuild from their bases. The map grows
with the army, so the crowd density stays the same.

| Units | Map (cells) | ms per tick |
|---|---|---|
| 20k | 400 × 225 | 0.57 |
| 200k | 1264 × 711 | 4.0 |
| 1M | 2828 × 1590 | 18.9 |

With a window, 20k units run at the display's 120 FPS with 1.2 ms of
simulation per frame.

How it stays fast, in case your game needs the same tricks:
- Marching units look for enemies only on their turn (`ctx.time.turns(4)`),
  units in a fight every tick.
- Separation uses a fine grid of friends, every other tick; enemy search
  a coarse grid of enemies, at melee reach first and full sight only if
  nothing is in reach.
- The four grids are rebuilt at the same time (`rayon::scope`).
- Once a second, units are sorted by `spatial_key`, so neighbors on the
  map are neighbors in memory.

## Events and attachments

`cargo run --release -p ponko_bench -- events` and `-- attach`:

| What | Per tick |
|---|---|
| 100k [events](events.md) sent, read by another system and dropped | 0.42 ms (4.2 ns per event) |
| 100k [attachments](attachments.md) following 100k moving entities | 0.66 ms |

Both allocate nothing once running: events reuse their buffers, and
attachments are placed in parallel into scratch kept between ticks.

## Rendering a million sprites

`CRABS=1000000 SCALE=<s> cargo run --release --example bunnymark_ecs`:
1M bouncing crabs, drawn by the built-in `draw_sprites`. The example prints
the render stats described below.

| Crab size on screen | FPS | render CPU | GPU |
|---|---|---|---|
| 1.6 px (`SCALE=0.05`) | 120 (display cap) | 3.3 ms | 2.4 ms |
| 8 px (`SCALE=0.25`) | 99 | 3.3 ms | 8.6 ms |
| 32 px (`SCALE=1`) | 15 | 2.8 ms | 71 ms |

The 32 px row is limited by the GPU filling pixels: on a Retina screen a
million 32 px crabs are about 4 billion blended pixels per frame. Fewer or
smaller sprites on screen is the only fix for that; the cost per sprite is
small (about 1.6 ms of GPU time per million).

What keeps the CPU side small:
- `draw_sprites` builds the instance data on all cores (1M sprites: 1.1 ms,
  was 3.4 ms on one core).
- Each sprite is 36 bytes on the GPU (was 52): the per-frame upload for 1M
  sprites is 36 MB. (A 28-byte version without a texture region measured
  the same render CPU time; the region is needed for text and atlases.)

### Is it the CPU or the GPU?

`ctx.perf.render_stats()` tells you what drawing the last frame cost:

```rust
# use ponko::prelude::*;
fn report(ctx: &mut Ctx) {
    let r = ctx.perf.render_stats();
    println!(
        "{} sprites in {} draw calls: {:.2} ms CPU, GPU {:?} ms",
        r.sprites, r.draw_calls, r.cpu_ms, r.gpu_ms
    );
}
```

`gpu_ms` is measured on the GPU with timestamp queries, so the frame rate
the window shows (which the compositor can cap or smooth) can't fool it.
If `gpu_ms` is close to the frame time, the GPU is the limit: draw fewer
or smaller sprites. If `cpu_ms` plus your systems' time
(`ctx.perf.system_timings()`) is close to it, the CPU is. Agents
get the same numbers from the control endpoint's `info` method.

## Profiling a frame

(The [profiling chapter](profiling.md) has the rest: your own timing scopes, counting allocations, headless performance tests with budgets, and what to do about what you find.)

A frame takes about as long as the slower of the CPU (systems, then
recording draws) and the GPU. The profiler shows which, and where the time
goes, in a running game:

- **The overlay** (`ctx.perf.set_hud(true)`, or `PONKO_HUD=1`): fps,
  frame-time percentiles, CPU and GPU time, and the slowest GPU passes.
- **`ctx.perf.frame_stats()`**: the same numbers over the last few seconds:
  fps, average, p50, p95, p99 and worst frame time, CPU time in systems,
  in rendering and waiting on the display, and GPU time. Display wait
  includes pacing; use measured GPU/CPU work to identify the bottleneck.
- **`ctx.perf.gpu_passes()`**: GPU time per pass (each shadow cascade, depth
  prepass, opaque, transparent, screen effects, bloom, post), measured
  with timestamp queries. On GPUs that draw in tiles, passes overlap, so
  each is counted from the end of the one before.
- **`ctx.perf.render_stats()`**: draw calls; 3D models and triangles drawn for
  the camera and, separately, for the shadow maps; and the render CPU
  time split into culling (`collect_ms`), recording (`record_ms`, all
  cores) and submitting. `uploads` counts the bytes sent to the GPU that
  frame, by kind (model transforms, uniforms, lights, ...), the buffers
  created, and the `reallocations` (buffers and textures replaced because
  they were outgrown; the new buffers also count as created): in a scene where nothing changes it should stay at the
  clock's few bytes and no buffers. Models that stand still cost no
  uploads, also while the camera moves; only the models that changed, or
  entered or left a view, are sent (see
  [Models between frames](#models-between-frames) and
  [What a frame sends the GPU](#what-a-frame-sends-the-gpu)).
- **`ctx.gfx.mesh_costs()`**: per mesh, how many models and triangles it drew
  for the camera and the shadows last frame, heaviest first.
- **Traces**: `ctx.perf.record_trace(path, frames)` (or `PONKO_TRACE=path`)
  writes the next frames as a Chrome trace: every system, rendering,
  display waits and the GPU passes on a timeline. Open it in
  [Perfetto](https://ui.perfetto.dev) to see what's in a hitch.

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/profiling.rs:profiling}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:profiling}}
```

</div>
</div>

### Benchmark runs

`PONKO_BENCH=<frames>` turns any windowed game into a benchmark: after
`PONKO_BENCH_WARMUP` frames (240), it measures that many with vsync off,
prints a report and quits. The report has the frame-time percentiles, the
CPU split, the GPU passes, the slowest systems and the meshes drawing the
most triangles; `PONKO_BENCH_OUT=file.json` also writes it as JSON, and
`PONKO_BENCH_MESHES=file.csv` writes every mesh's counts. Frames are
drawn offscreen, so the display's refresh rate neither caps nor stalls
them (`PONKO_BENCH_PRESENT=1` shows them instead).

```sh
PONKO_BENCH=600 PONKO_BENCH_OUT=before.json cargo run --release
```

Run it at a fixed camera before and after a change, and compare.

### Tracking benchmarks over time

A slow drift between releases is easy to miss in single pull requests, so CI
keeps a history. `bench_track`, a tool in `crates/ponko_bench`, counts the
instructions of seven small workloads under cachegrind: ECS iteration, a
filtered query, lookups, spawning, despawning, a headless game tick and
collecting a 3D frame. Instruction counts hardly move with the load of a
shared CI machine, where times jump around; they don't show memory or
parallel effects, which the wall-clock benchmarks still measure.

- **Pull requests** that touch the engine are counted against their base
  (three runs each, with the same workloads), and a comment names every
  benchmark that moved beyond noise.
- **Every commit on `main`** is recorded on the `bench-history` branch, one
  file per commit and machine; its README shows each benchmark over time,
  marking the commits that moved one.
- **Releases** are recorded there too, and each release adds a wall-clock
  baseline from one named machine, so two releases can be compared from
  files.

A change counts when the medians differ by more than the threshold: the
larger of a floor (1% for instruction counts, 5% for times) and twice the
larger spread (`(max - min) / median`) of the two sides' runs. Every report
prints both spreads and the threshold. To compare your own runs:

```sh
for i in 1 2 3; do PONKO_BENCH_JSON=before/$i.json cargo run --release -p ponko_bench -- ecs/; done
# ... make the change, then the same into after/ ...
cargo run --release -p ponko_bench --bin bench_track -- compare before/ after/
```

The `ponko_bench` README, "Tracking results over time", has the details:
the workloads, the file formats and the CI workflow.

### The rendering baseline

For work on the renderer itself there is a fixed set of scenes that needs
no game: seven generated 3D scenes (a static world, a dynamic one, many
materials, heavy shadows, see-through models, skinned figures and a small
town with all of it), drawn offscreen by the real renderer.

```sh
PONKO_BENCH_JSON=render.json PONKO_RENDER_CSV=render.csv \
  cargo run --release -p ponko_bench --features render -- render/scene
```

Each scene reports the CPU steps of a frame (collect, prepare, encode,
submit), the GPU time of every pass, draw calls, triangles, uploads,
allocations, the memory the renderer can count, and the frame-time
distribution (median, 95th and 99th percentile). The JSON also holds the
adapter, backend, driver, the limits and features the device gave, the
resolution, sample count and every quality setting, so two files say
whether they can be compared at all:

```sh
cargo run -p ponko_bench --bin render_compare -- before.json after.json
```

What a device can't measure is written as missing with the reason (GPU
pass times without timestamp queries, allocator totals on Metal), never as
0. It opens no window. `crates/ponko_bench/RENDER_SCENES.md` describes the
scenes, the settings, every field, the committed baseline and the three
scenes built again with Bevy, with what is and is not matched between the
two.

### From an agent

The [control endpoint](control.md) has the same numbers (`perf.get`,
`perf.trace`, `perf.hud`), and the [MCP server](agents.md) offers them as
`perf_report`, `record_trace` and `perf_hud`: an agent can ask a running
game why it's slow, change something, and ask again.

### The render-pass inspector

`perf.get` (and `perf_report`) always includes `render_inspector`, the
last frame's rendering cost in a fixed shape for tools. `schema` is its
version (1); a field is only removed or changes meaning with a new version.
A field's suffix is its unit: `_ms` milliseconds, `_bytes` bytes; counts
are per frame.

| Field | What |
|---|---|
| `gpu_timing` | `available`: the GPU has timestamp queries; `frames_timed`: frames whose GPU times came back |
| `cpu_ms` | the renderer's CPU time: `total`, split into `extract` (culling, collecting models), `prepare` (uploading 3D data), `encode` (recording command buffers), `submit` and `other` (sprites, overlays) |
| `gpu_ms` | `frame` and `passes` (`[{name, ms}]` in the order they run), smoothed |
| `counts` | `draw_calls`, `sprites`, `models`, `triangles`, `shadow_models`, `shadow_triangles`, `occluded_models`, `frustum_culled_models` (see below), `frustum_culled_other_models` (always `null`, see below), `pipelines_built`, and `instance_batches`: how the [static instance batches](3d.md#which-path-a-batch-takes) were culled (`batches`, `instances`, `gpu_driven`, `gpu_instances`, `cpu_small`, `cpu_no_compute`, `cpu_over_limit`, `dropped`, `pools`, `pages`, `pool_bytes`, `cull_dispatches`, `cull_jobs`, `views_skipped`) |
| `upload_bytes` | bytes the renderer uploaded to the GPU in the last drawn frame (`render.uploads` in `perf.get` splits them by kind) |
| `memory_estimate_bytes` | what the renderer's tables hold for the game: its `textures` (render-target textures included), `meshes`, its `gpu_buffers`, the `instance_pools` and their `total`. Not counted: the frame's own targets (scene color and depth, post-processing, ambient occlusion and temporal history), shadow maps, the light shadow atlas, skinning buffers and per-frame instance buffers. An estimate from what the engine allocated, not the driver's total |
| `adapter` | the GPU the renderer opened: `name`, `backend` (`vulkan`, `metal`, `dx12`, `gl`, `webgpu`), `device_type`, `driver`, `driver_info`, its [capability](3d.md) `tier` (`webgl2`, `baseline`, `desktop`) and `forced` (`true` when `PONKO_GPU_TIER` set the tier) |
| `history` | with `history: N` in the request, the last N drawn frames one by one, oldest first: `frame`, `cpu_ms`, `gpu_smoothed_ms` (the smoothed GPU time as it stood after that frame), `gpu_latest_ms` and `gpu_latest_frames_ago` (the newest single frame's own GPU time that had come back by then, and how many drawn frames earlier that frame was: timestamps come back a few frames late, so a frame's own GPU time isn't known yet when it is kept), `draw_calls`, `models`, `triangles`, `shadow_triangles` and `upload_bytes`. The GPU fields are `null` when GPU time wasn't measured. Up to 120 frames are kept, in a ring of 88-byte entries allocated once at the first drawn frame; without the parameter it is `[]` |
| `quality` | the active [graphics quality](3d.md#graphics-quality) `level` (`low` ... `ultra`, or `custom`) and every setting (3D builds) |
| `unavailable` | a reason for every metric that is `null` |

`counts.frustum_culled_models` counts plain models (entities with a
`Model`) whose bounds were outside the camera's view in the last frame. It
is worked out when `perf.get` asks, from the bounds the renderer keeps per
model, with the same test the culler uses, so a frame pays nothing for it
and a still camera gives the same number every frame. Models hidden by
occlusion culling are inside the view and counted in `occluded_models`
instead (it counts plain models and each LOD renderer once, a crossfade
between two hidden levels counting both, but not shadow-only casters).
Models whose mesh isn't loaded yet aren't counted in `frustum_culled_models`. While most
models move every frame the renderer stops keeping per-model results, and
the field is `null` with that reason until they settle.
`counts.frustum_culled_other_models` is always `null`: LOD groups, terrain
tiles, skinned models, models with morph targets and shadow-only casters
are culled anew every frame
without a result kept to count from, and static instance batches are
culled on the GPU (`counts.instance_batches` says how).

A metric the engine can't measure is `null`, never `0`, and `unavailable`
says why under its path, for example `"gpu_ms.frame": "the GPU adapter
doesn't support timestamp queries, so GPU time can't be measured"`.
Headless runs and games that haven't drawn yet report `cpu_ms`, `gpu_ms`
and `counts` as `null` the same way. The inspector is built only when
`perf.get` is called, from numbers the renderer keeps anyway, so it
costs a frame nothing; the history is a fixed ring of 120 entries (about
10 KB) filled in place after each drawn frame.

#### Measuring what a quality level costs

`quality.set` (the MCP tool `set_graphics_quality`) switches the
[graphics quality](3d.md#graphics-quality) preset of a running 3D game: `low`, `medium`,
`high`, `ultra`, or `recommended` (the engine's choice for the GPU).
Nothing the GPU lacks is turned on: the preset is fitted to the adapter,
and `fitted_to_gpu` says when it had to be (always `false` for
`recommended`, which already fits); a fitted preset's `quality.level` can
read `custom`. The answer has the settings before (`previous`) and after
(`quality`). It changes only the picture, never the simulation, and pins
render scale and shadow resolution against
[automatic quality tuning](3d.md#optional-scene-quality-autotuning) so the
comparison isn't undone; the pins stay until the game restarts. A headless
game accepts it (the settings change and `perf.get` shows them) but draws
nothing, and has no GPU to recommend for; neither has a windowed game
before its first frame.

An agent compares two levels in four steps: ask for the report, change the
level, wait a few seconds for the smoothed GPU times to settle, ask again.
The excerpts below are illustrative: the field names are the engine's, the
numbers made up for the example (shapes as the engine's tests build them).

1. `perf_report` with `history: 3`:

   ```json
   {
     "fps": 58.9,
     "render_inspector": {
       "schema": 1,
       "adapter": { "name": "Example GPU", "backend": "metal", "device_type": "integrated_gpu", "driver": "", "driver_info": "", "tier": "desktop", "forced": false },
       "gpu_timing": { "available": true, "frames_timed": 412 },
       "gpu_ms": { "frame": 14.8, "passes": [{ "name": "shadows 0", "ms": 1.3 }, { "name": "shadows 1", "ms": 1.0 }, { "name": "shadows 2", "ms": 0.9 }, { "name": "shadows 3", "ms": 0.7 }, { "name": "depth prepass", "ms": 1.2 }, { "name": "GTAO horizons", "ms": 0.6 }, { "name": "GTAO bilateral", "ms": 0.3 }, { "name": "opaque", "ms": 6.2 }, { "name": "post", "ms": 2.6 }] },
       "counts": { "draw_calls": 412, "models": 3810, "triangles": 2954112, "shadow_triangles": 4101870, "occluded_models": 0, "frustum_culled_models": 1270, "frustum_culled_other_models": null },
       "memory_estimate_bytes": { "total": 611319808, "textures": 402653184, "meshes": 150994944, "gpu_buffers": 41943040, "instance_pools": 15728640 },
       "history": [
         { "frame": 1840, "cpu_ms": 4.1, "gpu_smoothed_ms": 14.9, "gpu_latest_ms": 15.1, "gpu_latest_frames_ago": 2, "draw_calls": 412, "models": 3810, "triangles": 2954112, "shadow_triangles": 4101870, "upload_bytes": 65536 },
         { "frame": 1841, "cpu_ms": 4.0, "gpu_smoothed_ms": 14.8, "gpu_latest_ms": 14.6, "gpu_latest_frames_ago": 2, "draw_calls": 412, "models": 3810, "triangles": 2954112, "shadow_triangles": 4101870, "upload_bytes": 65536 },
         { "frame": 1842, "cpu_ms": 4.2, "gpu_smoothed_ms": 14.8, "gpu_latest_ms": 14.7, "gpu_latest_frames_ago": 2, "draw_calls": 412, "models": 3810, "triangles": 2954112, "shadow_triangles": 4101870, "upload_bytes": 65536 }
       ],
       "quality": { "level": "ultra", "render_scale": 1.0, "shadow_cascades": 4, "shadow_resolution": 4096, "ambient_occlusion": true, "anti_aliasing": "smaa" },
       "quality_autotune": null,
       "unavailable": { "counts.frustum_culled_other_models": "not counted: ...", "quality_autotune": "automatic quality tuning is off ..." }
     }
   }
   ```

2. `set_graphics_quality` with `level: "medium"`:

   ```json
   {
     "requested": "medium",
     "fitted_to_gpu": false,
     "previous": { "level": "ultra", "render_scale": 1.0, "shadow_cascades": 4, "shadow_resolution": 4096, "ambient_occlusion": true, "anti_aliasing": "smaa" },
     "quality": { "level": "medium", "render_scale": 1.0, "shadow_cascades": 3, "shadow_resolution": 2048, "ambient_occlusion": false, "anti_aliasing": "fxaa" }
   }
   ```

3. Wait a few seconds: GPU times are smoothed over recent frames, and the
   first frames after the switch build new shadow maps and targets.

4. `perf_report` again, and compare:

   ```json
   {
     "fps": 60.0,
     "render_inspector": {
       "gpu_ms": { "frame": 8.1, "passes": [{ "name": "shadows 0", "ms": 0.6 }, { "name": "shadows 1", "ms": 0.5 }, { "name": "shadows 2", "ms": 0.4 }, { "name": "depth prepass", "ms": 1.0 }, { "name": "opaque", "ms": 5.0 }, { "name": "post", "ms": 0.6 }] },
       "counts": { "draw_calls": 377, "models": 3810, "triangles": 2954112, "shadow_triangles": 3011544, "occluded_models": 0, "frustum_culled_models": 1270, "frustum_culled_other_models": null },
       "memory_estimate_bytes": { "total": 476053504, "textures": 268435456, "meshes": 150994944, "gpu_buffers": 41943040, "instance_pools": 14680064 },
       "quality": { "level": "medium", "render_scale": 1.0, "shadow_cascades": 3, "shadow_resolution": 2048, "ambient_occlusion": false, "anti_aliasing": "fxaa" }
     }
   }
   ```

   Here the GPU frame went from 14.8 to 8.1 ms: the shadow passes
   (`shadows 0` ... `shadows 3`) from 3.9 to 1.5 ms with one cascade fewer
   and smaller maps, the ambient occlusion passes (`GTAO ...`) are gone,
   and `post` is cheaper with FXAA instead of SMAA. The camera's
   triangles didn't change, and
   `fps` barely moved because the display's refresh rate capped it. Under
   vsync, compare `gpu_ms`, not `fps`. If `gpu_timing.available` is
   `false`, `gpu_ms` is `null` on both sides: compare the `cpu_ms` and the
   `counts` instead, or benchmark with `PONKO_BENCH`.

C and C++ games get the control method and the MCP tool through the same
endpoint, and can read and change the quality level themselves with
`ponko_ctx_graphics_quality` and `ponko_ctx_set_graphics_quality`.

### What usually helps in 3D

- **Shadows**: every caster is drawn again for each shadow cascade it
  falls in, also behind the camera. Compare the camera's triangles with
  the shadows' in `render_stats`. Turn shadows off for small things
  (`set_mesh_shadows`), and give detailed models a simpler
  [`ShadowMesh`](3d.md), especially far away.
- **Distant detail**: swap far models for simpler meshes or billboards.
  `mesh_costs` shows which meshes cost the most.
- **Pixels**: on a large Retina screen, `ctx.scene3d.post.msaa = 2` costs about a
  third less GPU time for the 3D passes than the default 4x.

### Models between frames

The 3D frame remembers the models it collected: each model's place in the
list its mesh is drawn from, in the camera's view and in every shadow map,
and what it was collected from. A frame then pays for what changed:

- **Nothing changed**: one compare per model. Nothing is culled, written
  or uploaded.
- **The camera or the sun moved**: every model is culled again, from the
  bounding sphere kept for it. Models that stay in view keep their place,
  so only the ones that entered or left a view are written and uploaded.
- **A model changed** (its `d3::Transform`, mesh, tint, `ShadowMesh`,
  `ModelMaterial` or `ReceivesDecals`): that model is worked out again
  and its instances rewritten. Changes are found by value, whichever way a
  component is written: a system that sets the same transform every frame
  changes nothing.
- **Models spawned or despawned, or got or lost a component**: the new
  ones are worked out, the gone ones leave their lists.
- **A mesh changed** (new data with other bounds, `set_mesh_shadows`): the
  models of that mesh are worked out again.

`LodGroup` renderers, terrain tiles, skinned models and shadow-only casters
are still worked out every frame (what they draw depends on the camera or
a pose), but keep their places in the lists as well.

While more than half the models change every frame, comparing them costs
more than it saves. The frame then works every model out from its
components alone, as it did before any were kept, and only looks up the
ones a view draws; it compares one model in 64 to notice when they settle.

Collecting 100,000 models of 64 meshes, a tenth of them in view, with sun
shadows (`cargo run --release -p ponko_bench -- extract-100k`: the median
of 120 frames after 30 warm-up frames, then the median of 8 runs on all 14
cores and of 5 runs on one core with `RAYON_NUM_THREADS=1`; M4 Max, while
other builds were running on the machine, so the 14-core times are a
little high on both sides):

| Each frame | 14 cores: before | now | One core: before | now | Instances uploaded: before | now |
|---|---|---|---|---|---|---|
| nothing changes | 0.45 ms | 0.20 ms | 1.21 ms | 0.41 ms | 11 KiB | 0 |
| the camera flies | 0.44 ms | 0.29 ms | 1.20 ms | 0.85 ms | 789 KiB | 25 KiB |
| it flies, 1% of the models move | 0.44 ms | 0.30 ms | 1.20 ms | 0.87 ms | 791 KiB | 73 KiB |
| it flies, every model moves | 0.45 ms | 0.45 ms | 1.20 ms | 1.31 ms | 831 KiB | 906 KiB |
| it flies, 100 models despawn and 100 spawn | 0.48 ms | 0.47 ms | 1.40 ms | 1.13 ms | 795 KiB | 27 KiB |
| the first frame (nothing remembered) | 0.49 ms | 1.33 ms | 1.25 ms | 2.66 ms | all | all |

What it costs:

- **Every model moving, on one core** (the web): about 9% slower than
  before (1.31 ms against 1.20 ms above). The models are culled as before;
  each drawn one is then looked up and written to its place in every view
  that draws it, which costs more than appending it to a list that is
  sorted afterwards. On 14 cores it costs the same as before.
- **The first frame**, and a frame that spawns many models at once, builds
  what later frames keep: two to three times a frame of before for those
  models, once.
- **Memory**: for every model in the world, in view or not, about 150
  bytes on one core and 210 on several (14 and 21 MiB for the 100,000
  above, with their instances; a frame's scratch lists took 3.5 MiB
  before). Every view that collects models keeps its own: a target camera
  with a 3D view, and global illumination's capture, each cost that again
  for every model in the world.
- **Order**: models of one mesh are kept in no particular order, and it
  changes as models enter and leave the view. `Blend::Transparent` models
  of one mesh are drawn far to near by their origins every frame, whatever
  that order is. Opaque models drawn in another order can differ in a few
  edge pixels, by a little.
- **`ctx.gfx.models_drawn()`** counts each model once, including models
  in a shadow cascade whose map is kept this frame.

`ctx.perf.render_stats().uploads.instances` shows what a frame uploaded: with a
moving camera it should be a small part of the instances drawn.

### What a frame sends the GPU

The renderer keeps a copy of what each per-frame buffer holds on the GPU
and uploads only the parts that changed since the last frame: the
models' records, their motion records (temporal antialiasing), the frame
settings of the camera and of every shadow view, the post-processing and
effect settings, skeletons' joints and morph weights, instance batches'
view settings, particles, ribbons, lines and 3D text, and 2D sprites. A
model that slides without turning changes 32 of its 144 motion bytes,
and only those go. A buffer that outgrows its contents is made anew at
the next power of two, so a world that keeps growing replaces it a
logarithmic number of times, and one that stays the same size replaces
none; the new buffer is written whole, and everything that referred to
the old one (bind groups, the copy of its contents) is made again with
it. Buffers never grow past what the device allows: a frame that needs
more is reported once on stderr (`ponko: ... more than this GPU's
buffers hold`), and what doesn't fit is not drawn (particles, ribbons
and text past the limit; with too many models or sprites, the frame's 3D
scene or its sprites) instead of failing.

Each write costs more than its bytes: `wgpu` makes and maps a staging
buffer for every one, about 2 µs of CPU time on an M4 Max, as much as
copying about 30 KB more. So a buffer whose changes can be scattered
(one record per model, sprite, particle or joint) is written in at most
16 writes a frame: when more parts changed, the narrowest gaps between
them are uploaded along. That sends more bytes than the changes alone,
never more than the whole buffer, and keeps the CPU time bounded however
the changes fall.

The copies cost CPU memory: as much as their buffers hold at most, which
for most games is small. Each model's record is 80 bytes in every list
it is drawn in (the camera, each shadow cascade, each target view): 1,024
models in a camera and four cascades keep about 400 KB. Particles keep
48 bytes each, a skeleton 64 bytes a joint, and every view's frame
settings about 3 KB. The copies grow with their buffers and, like them,
never shrink.

`RenderStats::uploads` (`ctx.gpu_uploads()` in C++) counts what was sent,
the buffers made and the buffers and textures replaced:

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/profiling.rs:uploads}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:uploads}}
```

</div>
</div>

Measured offscreen on an M4 Max (Metal), release build, 128×128 target,
sun shadows with four cascades, 1,024 cubes (256 for the particles), the
clock running, averages over 120 frames after 10 warm-up ones; before is
the renderer before this change, which uploaded every per-frame buffer
whole:

| Scene | Bytes uploaded per frame, before | After | `prepare` CPU p50, before | After |
|---|---:|---:|---:|---:|
| Nothing moves | 15,616 | 80 | 0.085 ms | 0.053 ms |
| 1% of the models move, temporal antialiasing | 20,640 | 99,019 | 0.157 ms | 0.155 ms |
| The camera pans | 17,498 | 4,994 | 0.079 ms | 0.098 ms |
| 5% of the models despawned and spawned again a frame | 39,715 | 24,243 | 0.098 ms | 0.097 ms |
| 2,000 still particles and 8 lines | 113,440 | 80 | 0.065 ms | 0.049 ms |

The 80 bytes left in a still scene are the clock: one vec4 in the frame
settings of the camera and of each of the four cascades. No scene made a
buffer in its measured frames. With temporal antialiasing, the 11 moving
models' records are spread over the lists of the camera and the
cascades, in more runs than a list's 16 writes (below): without that
limit the frame took 67 writes and 4,331 bytes, with it 48 writes and
99,019 bytes, and no more CPU time (0.155 ms, against 0.160 to 0.212 ms
in runs without the limit). The
byte counts are exact and the same on every run. The CPU times are from
runs on a machine busy with other builds (load 10 to 80), and the same
build moved by up to 0.02 ms between runs: the panning camera took 0.080
and 0.098 ms in two runs of one build, so it shows no clear change.

Changes scattered all over the buffers (every 17th of the 1,024 models
moving and every other of 200 particle emitters), by the most writes a
buffer takes per frame:

| Writes per buffer | Writes per frame | Bytes per frame | `prepare` CPU p50 |
|---|---:|---:|---:|
| No limit | 241 | 98,080 | 0.56 ms |
| 64 | 133 | 207,520 | 0.37 to 0.41 ms |
| 32 | 69 | 263,840 | 0.27 ms |
| 16 (the renderer's) | 37 | 292,000 | 0.17 ms |

Run it with
`cargo test --release --lib --features 3d upload_bench -- --ignored --nocapture`;
it also prints what writes cost on the machine it runs on.

## 3D: Ponko vs Unity

The same 3D scene in Ponko and in Unity 6, built the way a normal Unity
game is: one GameObject per cube with a `MeshRenderer` and a script whose
`Update()` turns it. Both scenes have:

- N spinning, tinted cubes;
- 12 physically based spheres and a ground;
- a sun with soft shadows (one 2048² shadow map, 60 units);
- sky and ground light and 4x MSAA.

Both run fullscreen at the display's native resolution with vsync off, on
the M4 Max. Each number is the median of three 10-second runs, after a
3-second warmup.

![The 100,000-cube scene: Ponko on the left, Unity 6 (URP) on the right](images/3d-vs-unity.jpg)

*Ponko (left) and Unity 6 (right), 100,000 cubes. Unity's axes are
left-handed, so it sees the field mirrored.*

| Spinning cubes | Unity 6 (URP) | Ponko | Ponko's lead |
|---|---|---|---|
| 1,000 | 728 FPS | **750 FPS** | about even |
| 10,000 | 398 FPS | **724 FPS** | 1.8x |
| 100,000 | 54 FPS | **402 FPS** | **7.5x** |

CPU time per frame:

| Spinning cubes | Unity main thread | Ponko (everything but waiting for the display) |
|---|---|---|
| 1,000 | 0.65 ms | **0.17 ms** |
| 10,000 | 2.51 ms | **0.43 ms** |
| 100,000 | 18.7 ms | **2.48 ms** |

Unity's time goes to the per-object work every GameObject brings: 100k
`Update()` calls, transform syncing, and culling and batching 100k
renderers. Ponko's goes to one query that turns every cube and one pass
that culls them, groups them by mesh and uploads them (28 draw calls).

To be fair to Unity:

- SSAO, post-processing and HDR were disabled in this historical
  comparison. The current engine has additional rendering features.
- The cubes share 64 tinted materials. Giving each its own color, as
  Ponko does for free, would break Unity's batching.
- Ponko's fullscreen frame is 3% smaller: macOS keeps the strip by the
  camera notch, so it's 3456 x 2168 against Unity's 3456 x 2234.
- The Unity player uses Mono, Unity's default script backend. IL2CPP
  would make the `Update()` calls somewhat faster.
- Each run started 10 seconds after the last one ended, so neither engine
  inherited the other's fullscreen exit.
- Unity Entities/DOTS, Jobs and Burst were not measured. The result applies
  to this GameObject/Mono workload; its performance gap cannot be generalized
  to Unity's optimized data-oriented workflows.

Run it yourself:

- Ponko: `CUBES=100000 BENCH_SECONDS=10 cargo run --release --features 3d --example 3d`
  (fullscreen; `BENCH_WINDOWED=1` keeps the window).
- Unity: the project and the steps are in
  [`bench/unity`](https://github.com/ponkoengine/ponko/tree/main/bench/unity).

On macOS, a game in a window is shown at most once per display refresh
even with vsync off. To measure past 120 FPS, run fullscreen
(`Game::fullscreen(true)`).

## Memory

Speed is half of scale; the other half is what each entity costs. The
headless example prints it: 1M agents with `d2::Transform`, `Sprite` and a
small `Agent` component take **76 bytes per entity** allocated, spare
capacity included.

From code, `world.allocated_bytes()` is the total for all component data,
and `Archetype::bytes_per_entity()` is what one more entity of that exact
component set costs. Agents get both from the control endpoint's
`world.summary` (the MCP `world_summary` tool).

## Rules the engine follows

- No heap allocation per frame in steady state on hot paths.
- Components live in tightly packed columns per archetype, so `for_each`
  compiles to plain loops the compiler can vectorize.
- Performance claims in pull requests come with numbers from release builds.

## Tips

- Use `--release`. Always.
- Prefer `for_each`/`par_for_each` over `iter()` for big queries.
- Spawn with `spawn_batch`.
- Run expensive logic at a reduced rate with `.add(phase, system.every(n))`,
  or spread per-entity work over ticks with `ctx.time.turns(n)` (see
  [The simulation clock](sim-clock.md)).

## C++

The [battle demo in C++](c-cpp.md#speed) runs at the same speed as the Rust
one for 20k units (0.57 ms/tick) and 6% slower for a million (19.97 against
18.85 ms/tick).
[C++ boids](c-cpp.md#speed) run 1M boids at 34.5 ms/tick headless, against
31.3 ms/tick for the Rust version; plain column updates are the same speed in
both (about 0.15 ms per million).
