# Rendering baseline: eight offscreen scenes (#134)

A reproducible measurement of the 3D renderer, to be recorded before and
after rendering work: eight generated scenes drawn by the real renderer into
an offscreen texture, each frame split into its CPU steps and its GPU
passes, with draw calls, uploads, allocations and memory, and with the GPU,
the limits and the quality settings written next to the numbers.

It opens no window and waits for no display. It needs a GPU and the bench's
`render` feature:

```sh
PONKO_BENCH_JSON=render.json PONKO_RENDER_CSV=render.csv \
  cargo run --release -p ponko_bench --features render -- render/scene
```

The scenes live in `src/render/gpu3d/scene_bench.rs` (the hidden
`ponko::render::SceneBench`), the runner in `src/render_scenes.rs`, the
report (JSON, CSV, comparison) in `src/render_report.rs`.

## The scenes

Every scene is generated: meshes from `MeshData`, 64×64 checker textures,
placement from one fixed pseudo-random sequence. Nothing is loaded from disk,
and two runs build the same scene (a test builds every scene twice, draws
both and compares their contents and counts). The camera circles the scene's center, 0.004 radians a frame, so a
run covers many views. Counts are at scale 1.

| Scene | What it holds | What it stresses |
|---|---|---|
| `single-material` | 100 ordinary cube models of one mesh and material | Simple-scene regression control |
| `static-world` | 8 static instance batches of 25,000 boxes and spheres (200,000 instances), a ground | Static batches: culling and drawing instances that never move |
| `dynamic-world` | 20,000 models of 16 meshes, every one moved and turned every frame | Collecting moved models, instance uploads, batching by mesh |
| `multi-material` | 6,000 models of 512 meshes, each mesh with its own material: 32 textures, opaque and cut-out, metallic, emissive, double-sided | Pipeline, bind group and buffer changes between draws |
| `shadow-heavy` | 4,000 shadow casters (half of them moving), a batch of 6,000 pillars, 3 point and 6 spot lights that all cast shadows, four 4096² sun cascades redrawn every frame out to 120 units | Shadow passes: cascades and the lights' shadow atlas |
| `alpha-heavy` | 4,000 moving see-through models and 1,000 cut-out cards over a ground | Sorting and blending, overdraw |
| `animated` | 400 skinned figures (16 joints each) posed every frame | Skinning and the skin upload |
| `game-like` | A small town: terrain, buildings in 4 static batches, 30,000 trees in a batch with two levels of detail, 400 vehicles on the streets, 80 skinned walkers, 48 point and 8 spot lights (6 with shadows), 300 see-through panes, a lake, bloom, occlusion culling | Everything at once, in the proportions of a game |

`game-like` is made of the engine's own generated shapes. It stands in for a
game; it is not one.

All scenes start from a `GraphicsQuality` preset (`high` unless
`PONKO_RENDER_QUALITY` says otherwise) with the atmosphere on; a scene then
changes what it is about (the shadow scene's cascades, the town's bloom).
The settings each scene really drew with are in its report
(`scenes[].scene.quality`).

## What a frame measures

One frame is: move the scene (`update`, not part of the frame time), then

| Step | What it is |
|---|---|
| `collect` | What a game's frame does before the renderer: pose skeletons, gather every model, light and batch the camera and the shadow maps see |
| `prepare` | `Renderer3D::prepare`: culling, sorting, uploads, shadow setup |
| `encode` | `Renderer3D::draw`: recording every pass into command buffers |
| `submit` | `Queue::submit` |
| `cpu` | The four steps above, added |
| `frame` | From the start of `collect` until the GPU has finished the frame (the run waits for it) |

A frame waits for the GPU before the next begins, so `frame` is the cost of
one frame alone, not a frame rate with frames in flight.

GPU times come from timestamp queries around each pass: `gpu_ms.total` adds
the passes of a frame, `gpu_ms.span` is from the first pass's start to the
last one's end, `gpu_ms.passes` lists each pass. Passes that draw nothing in
some frames have fewer samples.

Counts per frame: draw calls, recorded draws, instances and triangles for
the camera and for the shadow maps, models skipped by occlusion culling,
bytes uploaded (split by kind), buffers created, pipelines built, pipeline /
bind group / vertex buffer / index buffer changes, command buffers.
Triangles and instances count what the CPU hands the renderer: the instances
of static batches are drawn from their own buffers and are reported under
`instance_batches` instead.

Allocations per frame are the whole process's during the measured frames
(the bench's counting allocator), wgpu's included.

### What is unavailable, and how the report says so

Nothing is left out silently, and nothing unavailable is written as 0:

- **GPU pass times** need timestamp queries. Without them (a software
  adapter, WebGL2, `PONKO_GPU_TIER=webgl2`), `gpu_ms.total` and `span` are
  `null`, `passes` is empty, `timed_frames` is 0 and `gpu_ms.unavailable`
  says why. The CSV writes the rows with empty numbers and the reason in the
  `note` column.
- **Features** the device lacks are listed in `gpu.capabilities.features`
  with `available: false`, the `reason` and the `fallback` the renderer uses.
- **Memory** comes in three kinds, kept apart:
  - *Counted* from the renderer's own tables (`memory.counted_bytes`): mesh
    buffers, the game's GPU buffers, the GPU-driven batches' pools and the
    scene's textures.
  - *Computed from sizes* (`memory.computed_bytes`, with `memory.computed`
    saying so): `render_target_bytes` (the scene's multisampled color and
    depth, the HDR and scene-copy targets, the output), `shadow_map_bytes`
    (the sun's cascades) and `light_shadow_atlas_bytes`, each width × height
    × layers × samples × the format's bytes. These are what the textures
    hold, not what the device reports having allocated.
  - *Uncounted*, named in `memory.uncounted`: the effects' own targets
    (ambient occlusion, reflections, antialiasing, motion vectors, bloom and
    the post chain), history textures, the instance buffers of static
    batches culled on the CPU and the per-frame instance, uniform and light
    buffers.

  The device's allocator totals are there where wgpu reports them; on Metal
  it does not, and `memory.device_unavailable` says so.
- **Peak resident memory** of the process is read with `getrusage` on Unix;
  elsewhere `process.peak_rss_unavailable` says it is not read. It is one
  number for the whole run, not per scene.
- **The driver** is whatever wgpu reports; Metal reports none (`""`).

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `PONKO_RENDER_FRAMES` | 600 | Measured frames per scene |
| `PONKO_RENDER_WARMUP` | 120 | Frames drawn before measuring (pipelines, first uploads) |
| `PONKO_RENDER_SIZE` | `1280x720` | The offscreen target |
| `PONKO_RENDER_SAMPLES` | 4 | MSAA samples asked for: 1, 2, 4 or 8. Where the adapter can't multisample the 3D targets that much, 4 is used; the report has both (`gpu.target.samples_requested`, `samples`) |
| `PONKO_RENDER_QUALITY` | `high` | `low`, `medium`, `high` or `ultra` |
| `PONKO_RENDER_SCALE` | 1 | Multiplies every scene's object counts |
| `PONKO_RENDER_MATCHED` | 0 | 1: draw only what the Bevy copies draw too (see below) |
| `PONKO_RENDER_TAA` | 0 | 1: temporal antialiasing instead of the preset's, so motion vectors are drawn (no preset turns TAA on) |
| `PONKO_RENDER_WIND` | 0 | 1: `game-like`'s tree crowns sway in a material `vertex` function, and 4,096 cubes are placed from a GPU buffer of 262,144 elements that keeps its last frame (what motion vectors of `vertex` functions and buffers cost) |
| `PONKO_RENDER_VERTEX_MOTION` | 1 | 0: the motion pass ignores what `vertex` functions do (the control for `PONKO_RENDER_WIND` with TAA) |
| `PONKO_GPU_TIER` | unset | Opens the device as a weaker GPU (`webgl2`, `baseline`), as the engine's tests do |

A value that doesn't parse stops the run and names the variable.

The filter picks scenes: `-- 'render/scene shadow-heavy'`, or several with
`|`.

## Output

| Variable | File |
|---|---|
| `PONKO_BENCH_JSON=file.json` | The bench's usual document (`schema_version`, `env`, `results`) with a `render` section |
| `PONKO_RENDER_CSV=file.csv` | One row per scene and metric: `scene,group,metric,unit,samples,min,p50,p95,p99,max,mean,note` |
| `PONKO_RENDER_FRAMES_CSV=file.csv` | One row per measured frame: every step, GPU total and span, the counts |
| `PONKO_RENDER_PNG=dir` | The last frame of each scene as `dir/<scene>.png`, to look at what was measured |

The JSON document:

- `env`: Rust version, git revision and whether the tree was dirty, OS, CPU,
  cores, features, and the machine's load averages when the run started.
- `results`: one record per scene and step (`render/scene <scene> collect`,
  `prepare`, `encode`, `submit`, `cpu`, `frame`), the same record as every
  other benchmark, with `p99_ms` added.
- `render.settings`: resolution, samples, quality, scale, frames, warm-up,
  and that frames are offscreen.
- `render.gpu`: adapter (name, backend, device type, driver), vendor and
  device ids, the limits the renderer depends on, every optional feature
  with its availability, the forced tier if any, the target's format and
  sample count, whether timestamp queries exist.
- `render.process`: peak resident memory.
- `render.scenes[]`: `scene` (content counts and the quality settings
  drawn with), `timings_ms` (a distribution per step: samples, min, p50,
  p95, p99, max, mean), `gpu_ms`, `counts`, `upload_bytes_per_frame`,
  `instance_batches`, `state_changes`, `command_buffers`,
  `allocations_per_frame`, `allocation_bytes_per_frame`, `memory`.

`render.schema_version` is 1. The section reuses the engine's own types and
counters (`GpuCapabilities`, `GpuTimer` spans, `GpuUploads`,
`InstanceBatchStats`, `GraphicsQuality`); it is not a second schema for them.

## Comparing two runs

```sh
cargo run -p ponko_bench --bin render_compare -- before.json after.json
```

prints the medians and 95th percentiles side by side with the change, per
scene and step. It first checks that the runs can be compared: the same
adapter, backend and driver, resolution, samples asked for and used,
quality, scale, measured and warm-up frame counts, GPU tier, OS and CPU,
the same scenes in both, and the same content and settings in each. If
anything differs it prints every difference, prints no table, and exits
with 1.

The table does not say which differences are noise. Read the 95th and 99th
percentiles and both runs' `env.load_average`.

## The committed baseline

`baselines/render.json` and `baselines/render.csv`, recorded 2026-11-11 at
`e63e8db1` on an Apple M4 Max (14 cores, 36 GB), macOS 27.0.1, Metal, wgpu
30.0.1, Rust 1.99.1, release profile, features `physics`, `physics3d`,
`navmesh`, `window`; 1280×720, 4× MSAA, `high`, scale 1, 120 warm-up and 600
measured frames. Peak resident memory of the run: 224 MB.

The files predate two later changes, neither of which changes what was
measured: the `game-like` scene's panes are now turned by exactly a quarter
turn instead of 1.5708 radians (a difference of 0.000004 radians), and the
memory report gained the sizes computed for render targets and shadow maps
(`render_target_bytes`, `shadow_map_bytes`, `light_shadow_atlas_bytes`,
`computed_bytes`), which the committed files therefore do not have yet.

**This baseline was recorded on a machine under heavy load** (load averages
25.41 23.83 20.80 on 14 cores when it started: other builds were running).
The medians are usable as a first reference; the 95th and 99th percentiles
of the first five scenes are mostly the other processes. An earlier run of
the same scenes minutes before (commit `b97e0f13`) had, for example,
`multi-material` at a 4.4 ms median frame against 8.9 ms here. Record it
again on an idle machine before using it to judge a change:

```sh
PONKO_BENCH_JSON=crates/ponko_bench/baselines/render.json \
PONKO_RENDER_CSV=crates/ponko_bench/baselines/render.csv \
  cargo run --release -p ponko_bench --features render -- render/scene
```

Milliseconds, medians unless a percentile is named:

| Scene | CPU | Frame | Frame p95 | Frame p99 | GPU passes | Draw calls | Triangles (camera / shadows) |
|---|---:|---:|---:|---:|---:|---:|---:|
| `static-world` | 1.050 | 4.930 | 26.975 | 37.460 | 3.462 | 26 | 2 / 6 |
| `dynamic-world` | 3.188 | 8.344 | 17.713 | 43.799 | 4.006 | 90 | 721,530 / 38,698 |
| `multi-material` | 1.997 | 8.893 | 9.684 | 10.386 | 6.006 | 1,566 | 219,480 / 146,678 |
| `shadow-heavy` | 2.450 | 9.252 | 35.797 | 67.389 | 5.767 | 259 | 133,686 / 614,002 |
| `alpha-heavy` | 1.841 | 9.543 | 21.351 | 30.621 | 6.881 | 77 | 142,866 / 196,420 |
| `animated` | 1.041 | 4.238 | 5.710 | 7.991 | 2.771 | 1,378 | 145,922 / 227,334 |
| `game-like` | 0.881 | 2.858 | 3.213 | 3.713 | 1.516 | 91 | 38,280 / 284,574 |

`static-world` shows 2 triangles because its 200,000 instances are in static
batches (see the counts note above).

| Scene | Collect | Prepare | Encode | Submit | Allocations a frame | Counted GPU bytes |
|---|---:|---:|---:|---:|---:|---:|
| `static-world` | 0.032 | 0.181 | 0.654 | 0.135 | 698.4 | 20,460 |
| `dynamic-world` | 1.467 | 1.166 | 0.442 | 0.106 | 770.1 | 40,716 |
| `multi-material` | 0.183 | 0.362 | 1.244 | 0.172 | 850.7 | 1,869,336 |
| `shadow-heavy` | 0.504 | 0.753 | 0.940 | 0.223 | 1,777.0 | 21,660 |
| `alpha-heavy` | 0.523 | 0.756 | 0.466 | 0.116 | 781.3 | 37,336 |
| `animated` | 0.120 | 0.184 | 0.594 | 0.135 | 822.3 | 40,956 |
| `game-like` | 0.106 | 0.240 | 0.412 | 0.106 | 1,151.5 | 1,248,844 |

The counted bytes are small because what dominates GPU memory here is not
in the renderer's tables: render targets and shadow maps (computed from
their sizes in runs recorded from now on) and the CPU-culled batches'
instances (uncounted), and Metal gives no allocator totals. The per-pass GPU times, upload
split and state changes are in the files.

`cargo test -p ponko_bench` checks that the committed files hold all seven
scenes with every timing, GPU times or the reason, and memory counted or
named as uncounted.

## Side by side with Bevy (optional)

`bench/bevy_scenes` builds three of the scenes again with Bevy 0.20.1:
`static-world`, `dynamic-world` and `multi-material`. It is a crate of its
own with its own workspace and lock file: Bevy is not a dependency of the
engine, the bench or any default build, and the crate is not published.

```sh
# Ponko, drawing only what the Bevy copies draw too:
PONKO_RENDER_MATCHED=1 PONKO_BENCH_JSON=ponko.json PONKO_RENDER_CSV=ponko.csv \
  cargo run --release -p ponko_bench --features render -- \
  'render/scene static-world|render/scene dynamic-world|render/scene multi-material'

# Bevy (reads the same PONKO_RENDER_FRAMES, WARMUP, SIZE, SCALE and PNG):
cd bench/bevy_scenes
PONKO_BENCH_JSON=bevy.json PONKO_RENDER_CSV=bevy.csv cargo run --release
```

Both write the same CSV columns, so the rows can be put in one sheet.

**Matched** (the same code or the same numbers on both sides):

- Meshes: the vertices and triangles of `MeshData::cube`, `sphere` and
  `plane`, ported line for line, so both draw the same triangles.
- Placement, sizes, rotations and motion: the same sequence and formulas.
- Materials: base color, roughness, metallic, emissive, cut-out at 0.5,
  double-sided; the same generated textures with a full mip chain, linear
  filtering, repeat.
- Camera: 60° tall, near 0.1, the same far plane, the same circle, the same
  frame index.
- Sun: the same direction, color and strength, shadows from 4 cascades of
  2048² out to 80 units, all redrawn every frame.
- Target: 1280×720 `Rgba8UnormSrgb`, 4× MSAA, FXAA, ACES tone mapping, no
  bloom, no ambient occlusion, no reflections, black behind the scene.
- Loop: one thread simulates and renders (Bevy's pipelined rendering is
  off), pipelines are compiled before use, each frame waits for the GPU.
  `cpu` is everything from the scene's world to the submit, `frame` adds
  the wait for the GPU.

**Not matched** (read the numbers with these in mind):

- **Static instances.** Ponko's `static-world` uses its static instance
  batch API: 8 batches. Bevy has no such API; its copy is 200,000 entities
  that Bevy batches itself each frame. This is the largest difference and
  it is a difference in what each engine offers, not in the scene.
- **GPU times.** The Bevy program reports none, with the reason: Bevy's
  render diagnostics record GPU timestamps on Vulkan and DX12 only, and the
  baseline machine runs Metal. Only `cpu` and `frame` can be set side by side.
- **Ambient light.** Ponko blends a ground color and a sky color by the
  surface's direction; Bevy's ambient light is one color (the middle of the
  two is used). Bevy's shaded sides come out somewhat darker.
- **Tone curve.** Ponko uses Narkowicz's ACES fit, Bevy the fitted ACES
  transform; the Bevy camera's exposure is set to 1/0.6 to bring the
  brightness together. Saturated colors still differ.
- **Shadow filtering and bias**, cascade split distances and the shading
  model's details (specular terms, Fresnel) are each engine's defaults.
- **Culling and batching** are each engine's own: frustum culling on both,
  no occlusion culling on either.
- **Texture mips** are made by each side's own box filter.
- The Bevy copy counts no draw calls, uploads or memory.

The pictures (`PONKO_RENDER_PNG`) of both were compared by eye at the same
frame: objects, camera and shadows line up; brightness and color differ as
described above. That is the extent of the image match: the two renderers'
images are close, not equal.

Recorded together with the baseline, minutes apart, on the same loaded
machine (`baselines/render-matched.json`, `render-matched.csv`,
`render-bevy.json`, `render-bevy.csv`; load averages 18.28 22.35 20.40 and
24.86 23.97 21.19 when each started; the matched file's `git_dirty` is true
only because the run before it had just rewritten the baseline files).
Milliseconds:

| Scene | CPU p50 Ponko | CPU p50 Bevy | Frame p50 Ponko | Frame p50 Bevy | Frame p95 Ponko | Frame p95 Bevy | Frame p99 Ponko | Frame p99 Bevy |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `static-world` | 0.368 | 7.255 | 2.714 | 10.727 | 2.948 | 14.130 | 3.037 | 18.408 |
| `dynamic-world` | 1.057 | 4.502 | 2.188 | 6.874 | 2.892 | 8.485 | 3.372 | 9.327 |
| `multi-material` | 1.928 | 1.851 | 3.495 | 3.483 | 3.944 | 4.781 | 4.144 | 5.870 |

What these numbers support, and what they do not:

- They are three generated scenes on one Apple GPU, one run each, on a busy
  machine. They are not a claim that either engine is faster in general.
- `static-world` mostly measures that Ponko has an API for instances that
  never move and this Bevy scene does not use an equivalent.
- `multi-material` is level within the noise of this run.
- Each engine is used in its plain, documented way. Neither side is tuned;
  a Bevy user could restructure these scenes, and so could a Ponko user.
- Other GPUs, backends, resolutions and scenes may order them differently.
  Record both on the machine you care about.

## Known artifact

With the atmosphere on (every scene of the plain run), the pictures show a
thin dark streak in the sky toward −X. It is an engine defect outside this
issue: `sky_radiance` in `src/render/mesh.wgsl` samples the sky atlas with a
linear, repeating sampler, and at the seam of the sky's azimuth it filters
across neighboring regions of the atlas. It does not change what the
baseline measures.

## Limits

- Offscreen only: no swapchain, no present, no display wait. A windowed
  game adds those; `PONKO_BENCH` (see the book's "Benchmark runs") measures
  a real game's frames.
- The scenes go through `Renderer3D` directly, mirroring what a game's
  frame does before it. The 2D renderer, UI and the window's final blit are
  not part of it.
- One machine, one backend. So far the scenes and their tests have run
  only on Metal (an Apple M4 Max), as the device is and with
  `PONKO_GPU_TIER=webgl2` and `baseline` forcing the weaker tiers on it.
  The tests open their device through the engine's shared render test
  helpers (a missing adapter is logged, or fails under `PONKO_REQUIRE_GPU`).
  They are written to pass on software adapters and other backends (small
  targets and shadow maps, no timing assertions, missing or unusable GPU
  timestamps accepted, nothing that depends on when occlusion results come
  back), but that has not been observed yet: the first runs
  on Vulkan, DX12 and software adapters will be CI's after the merge. The
  tests check that the scenes draw and report, never how fast.
- The harness adds no timing code to the renderer's frame path. It reads
  counters the renderer already keeps; the one change to the renderer is
  that `Renderer3D::take_state_changes` is no longer test-only (the counters
  it reads were already kept in release builds).

## Shadow-cache workload controls

`PONKO_SHADOW_WORKLOAD` specializes the same generated scenes for shadow
measurements, always with `shadow_refresh = 1`:

- `static`: fixed camera and fixed actors.
- `slow-camera`: static actors, camera orbit step 0.00001 radians per frame.
- `actors`: fixed camera, the scene's normal actor animation.
- `thrash`: normal camera and actor motion, plus a moving sun every frame.

Unset preserves the original baseline. The selected workload is recorded in
`scenes[].scene.shadow_workload`. Use `static-world` for the first two,
`dynamic-world` for moving actors and full invalidation; the latter moves all
models every frame. For example:

```sh
PONKO_SHADOW_WORKLOAD=slow-camera PONKO_RENDER_MATCHED=1 \
PONKO_RENDER_FRAMES=300 PONKO_RENDER_WARMUP=60 PONKO_RENDER_SCALE=0.1 \
PONKO_BENCH_JSON=shadows.json PONKO_RENDER_FRAMES_CSV=shadows.csv \
  cargo run --release -p ponko_bench --features render -- 'render/scene static-world'
```

Compare average shadow triangles and shadow GPU passes over all measured
frames, rather than the last frame alone (snapping deliberately creates
occasional refreshes). The frame CSV includes the work counts; JSON includes
CPU/GPU distributions, allocation counts, memory and adapter metadata.


For shared mesh arenas, global transparent order, and the cost of retaining
instancing versus sorting by depth, see [the draw-order measurements](DRAW_ORDER_156.md).
