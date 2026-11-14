# Streaming: memory budgets and loading without waiting

`load_texture` and `load_mesh` read and decode a file on the game thread
and keep it until you [free it](handles.md). That is the simple way, and
right for what a game needs all the time. A large world needs more than
fits in GPU memory at once, and can't stop for a frame while a file
decodes. For that, ask for assets **by name, every frame you need them**:

All of them are on `ctx.gfx`, the one service that owns GPU assets, their
upload budget and their residency budgets. Which loader do I use?

| | Waits for the file | Under a memory budget | Use it for |
|---|---|---|---|
| `load_texture`, `load_mesh` | yes | no: kept until freed | what the game needs all the time |
| `resident_texture`, `resident_mesh` | yes, the first time | yes | what must be there this frame (cell props, a level's set) |
| `stream_texture`, `stream_mesh` | never | yes | what can pop in a few frames later (far cells, LOD levels) |

None of these calls evicts anything: the engine checks the budgets at the
start of every frame, before your systems, with the world your entities are
in (see [Budgets](#budgets-staying-in-memory)). The one exception is
`ctx.finish_uploads()`, for loading screens, which uploads everything asked
for and evicts as that check would. The two budgets
are separate: `set_upload_budget` limits the bytes sent to the GPU per frame,
`set_residency_budget` the bytes kept on it. The same module
also holds tools that hold no assets: [`Job`](https://docs.rs/ponko/latest/ponko/streaming/struct.Job.html) runs your own work on a
worker thread and `CellStreamer` says which cells of a grid to load, for
games that build their own content.

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/streaming.rs:streaming}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:streaming}}
```

</div>
</div>

## Streaming: never waiting

The first `stream_texture` or `stream_mesh` call starts reading and decoding
the file on a [job](https://docs.rs/ponko/latest/ponko/streaming/struct.Job.html)
thread and returns nothing. At the start of a later frame, before your
systems run, the engine's **upload step** puts decoded assets on the GPU,
in the order they were asked for, until the frame's **upload budget**
(`ctx.gfx.set_upload_budget`, 16 MiB by default) is spent. One upload bigger than
the budget still goes, alone in its frame, so nothing waits forever.

A texture comes in as a **stand-in** at most 32 pixels a side, and from then
on `stream_texture` returns the same handle every time. Each later upload
doubles its resolution until it is full size; the handle always reports the
full size, so sprites and materials don't change. A mesh comes in whole.

Every asset has an `AssetState`, and `ctx.gfx.streamed_assets()` lists them all:

| State | Meaning |
|---|---|
| `Loading` | decoding on a worker |
| `Uploading` | decoded, waiting for room in the upload budget |
| `Streaming` | a texture below full resolution, getting sharper |
| `Resident` | fully loaded |
| `Evicted` | dropped to fit its budget (see below) |
| `Failed` | the file is missing or broken; `ctx.gfx.asset_error` says why |

A failed asset isn't retried by asking again, so a missing file costs
nothing each frame; `resident_texture` or `resident_mesh` loads it again.

States change only in the upload step. Normally that step never waits for a
decode, so which frame an asset arrives in (and the order new textures get
their ids in) depends on how fast the workers are. With
`ctx.gfx.set_deterministic_uploads(true)` the step waits for each decode instead,
so every state changes in a frame that depends only on the order of
requests and the budget: replays, lockstep games and tests see the same
frames every run. States are fixed per frame, not per simulation tick: a
frame can run several ticks or none, so replays need the same frames too. `finish_uploads(&mut ctx.world)` waits for everything asked for so far,
for loading screens and tests.

## Budgets: staying in memory

`ctx.gfx.set_residency_budget(kind, bytes)` caps the bytes of meshes or textures
loaded by name (estimates of their GPU memory). When resident assets go
over it, the **least recently used** ones are evicted until they fit. The
budget is checked at the start of every frame (and while uploading streamed
assets), never inside the call that loads: a load, a lower budget or an
unpin can leave usage over the budget for **at most one frame**, until the
next frame's check evicts what may go. What entities stop showing goes
then too, without waiting for the next load. Never evicted:

- assets asked for in this or the previous frame or tick: a handle you just
  got stays good while you use it, and what you ask for every frame (or
  every fixed-step tick, with frames between) is never evicted between two
  asks,
- meshes a `Model`, `ShadowMesh` or `LodGroup` shows, and textures a
  `Sprite` shows,
- pinned ones (`pin_resident_texture`, `pin_resident_mesh` until unpinned),
- meshes the game also loaded with `load_mesh`: it holds those handles.

Those are the documented exceptions: they can keep usage over the budget,
and `ctx.gfx.residency(kind).over_budget_bytes()` says by how much. Assets loaded
with `load_*` or made with `create_*` are never counted.

Staying over a budget has a cost each frame: while a kind is over, the
frame's check looks at every entity that shows an asset of that kind (each
`Sprite` for textures; each `Model`, `ShadowMesh` and `LodGroup` for
meshes) to see what is still in use. It allocates nothing, but it grows
with the scene, so set a budget above what the scene shows at once. Under
the budget the check is a comparison.

A texture streaming up stops at the largest resolution that fits the
budget once nothing else can be evicted, and goes on when room appears.

## Evicted handles stay safe

Eviction never leaves a handle pointing at something else:

- An **evicted texture** drops to its stand-in (which the budget doesn't
  count) and stays live: whatever shows it draws a blurry version until it
  is asked for again and streams back up under the same handle.
- An **evicted mesh** is freed, but its id is kept for its file: a handle
  you kept in a resource reads as not live (`is_live` is false), a model
  showing it draws nothing, and no other mesh takes that id. Asking for
  the file again brings the mesh back under the same handle. Freeing that
  handle (`ctx.free`) releases the id; the file then loads as a new mesh.

Materials keep sampling an evicted texture's stand-in, so ask for textures
that only materials use each frame they are on screen, or pin them.

## LOD levels that stream in

`StreamedLod` streams the levels of a `LodGroup` from files, one per level.
Each frame the engine asks for the level the camera shows and the coarsest
one. Until a level is in, the group shows the closest coarser level that is,
so models never pop out while they load; levels the camera left stop being
shown, and the mesh budget can evict them.

```rust
# use ponko::prelude::*;
# fn f(ctx: &mut Ctx) {
let levels = vec![
    ponko::render::LodLevel::new(0.3, vec![ponko::render::LodRenderer::new(ponko::render::Mesh::NONE)]),
    ponko::render::LodLevel::new(0.05, vec![ponko::render::LodRenderer::new(ponko::render::Mesh::NONE)]),
];
ctx.world.spawn((
    ponko::d3::Transform::default(),
    ponko::render::LodGroup::new(10.0, levels),
    ponko::render::StreamedLod::new(&["models/tower_lod0.glb", "models/tower_lod1.glb"]),
));
# }
```

From C, `ponko_ctx_set_streamed_lod`; from C++, `ctx.set_streamed_lod(e, files)`.

## Hot reload and freeing

With [hot reload](data-files.md) on, editing a streamed texture's file
replaces it at full resolution under the same handle.
[Compressed textures](drawing.md#compressed-textures) (`.ktx2`) stream
through the mip levels stored in the file, and count their compressed
bytes against the budget. Freeing a streamed
texture or mesh with `ctx.free` works as for any handle; asking for it by
name again loads it afresh, as a new handle.

## Where to look

`perf.get` on the [control endpoint](control.md) reports `residency` (the
budget and counters of each kind) and `streaming` (the upload queue's
counters and every asset's state); `ctx.gfx.upload_stats()` gives the same
numbers in code, including how long the upload step took.

## Measured

`cargo run --release -p ponko_bench -- streaming/` runs a headless game that
needs sixteen new assets at once, written to disk first: 1024 × 1024 noise
PNGs, or glTF binaries of a 256 × 256 vertex grid (65 536 vertices and
130 050 triangles, about 3 MiB each). It measures the game thread's CPU
time spent loading in each frame: the load or stream calls plus the upload
step, which for meshes includes building them from the decoded file. CPU
time rather than wall time, so a busy machine pausing the thread doesn't
count (on Windows, which has no such clock here, it is wall time). On an
Apple M4 Max (macOS, release build, the default 16 MiB upload budget, one
set of three runs with the machine under heavy other load, load average
about 60):

| | Worst frame's loading work | Median | Usable after |
|---|---|---|---|
| `load_texture` | 10.1 to 11.5 ms | 0.18 ms | the same frame |
| `stream_texture` | 0.42 to 0.73 ms | 0.001 ms | 3 to 11 frames |
| `load_mesh` | 44.0 to 48.4 ms | 0.19 ms | the same frame |
| `stream_mesh` | 0.52 to 0.65 ms | 0.002 to 0.005 ms | 10 to 11 frames |

The loading calls decode all sixteen in the frame that first needs them;
streaming moves decoding to workers and spreads the uploads over frames.
Read the numbers with these caveats:

- "Usable after" for textures counts the 32-pixel stand-ins; full
  resolution comes a few uploads later. Meshes arrive whole.
- Headless runs don't include the GPU's share of uploads (copying buffers
  and textures to the GPU); the upload budget is what bounds that.
- Decode cost depends on the files: noise PNGs stand in for real art and
  may decode more cheaply than it, and the grid meshes have no materials
  or textures.
- One set of runs on a loaded machine: compare the two ways within a run,
  not against other machines.

## Browsers and asset sources

Browsers have no worker threads here, so on the web a stream's decode runs
when it is asked for. [Asset sources](asset-sources.md) are read on the
game thread (their trait runs there). Images from them are still decoded
on workers; meshes, whose files can refer to other files only those sources
can read, are decoded on the game thread while any source is added.

## C and C++

Every call above has a C function (`ponko_ctx_stream_texture`,
`ponko_ctx_asset_state`, `ponko_ctx_set_residency_budget`, ...) and a
`ponko::ctx` method of the same name; kinds are `PONKO_RESIDENCY_MESH` and
`PONKO_RESIDENCY_TEXTURE` (`ponko::residency_kind`), states
`PONKO_ASSET_*` (`ponko::asset_state`).
