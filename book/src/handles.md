# Handles: freeing what you made

Textures, meshes, materials, sounds and the other things the engine keeps
for your game come back as small handles: plain `Copy` values, passed by
value (`ctx.gfx.draw(crab, pos)`, `ctx.audio.play(hit)`). Only the engine makes
them: a number can't be turned into a handle, so a handle always names
something your game made. Every kind works the same way:

| Question | Rust | C++ |
|---|---|---|
| Is it still valid? | `ctx.is_live(h)` | `ctx.is_live(h)` |
| Give the memory back | `ctx.free(h)` | `ctx.free(h)` |
| An empty handle | `Texture::NONE`, `Mesh::NONE`, ... | `ponko::none<ponko::texture>()` |
| How many are live? | `ctx.live_counts()` | `ctx.live_counts()` |

The kinds are `Texture` (render targets included), `Sound`, `Font`, and with
the `3d` feature `Mesh`, `Material`, `Shader`, `Compute`, `Effect`,
`GpuBuffer`, `InstanceBatch`, `TextureArray` and `TerrainTile` (with
`physics3d`, `CollisionMesh` too). They implement the `Handle` trait, so a
helper can take any of them.

The service that holds a handle answers the same questions:
`ctx.gfx.is_live(h)`, `ctx.gfx.free(h)` and `ctx.gfx.live_counts()` for
everything drawn (the `GfxHandle` trait), `ctx.audio.is_live(s)` and
`ctx.audio.free(s)` for sounds, and `ctx.physics3d.is_live(m)` and
`ctx.physics3d.free(m)` for collision meshes. `ctx.is_live` and `ctx.free`
take any kind and forward to them.

A game that streams its world frees what each cell made when the cell goes
out of range:

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/handles.rs:handles}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:handles}}
```

</div>
</div>

## The rules

- **No handle is ever given out twice in a run**, so a freed handle stays
  dead: it never starts naming something made later, and a stale copy kept
  somewhere can't quietly point at the wrong thing. Meshes, GPU buffers and
  collision meshes are made and freed all the time in streamed worlds, so
  the next one made takes a freed one's slot in the engine's tables (which,
  with the per-frame work over every mesh, then don't keep growing), but
  under a new handle that counts how often its slot was given out. After
  4095 times the slot is retired instead of counting from zero again, so a
  stale handle stays dead for good.
- **Freeing twice, or freeing `NONE`, does nothing.** `NONE` is never live.
- **Only what the game made can be freed.** The built-in font, the default
  material's internals and the engine's own textures (shadow maps, the font
  atlas) aren't the game's to free; freeing `Font::DEFAULT` does nothing.
- **Nothing is reference counted.** Freeing is explicit, and the handle you
  hold doesn't keep anything alive.

## Freeing something that is still used

Freeing a resource others still use is allowed, and what they do then is
defined. Nothing panics and nothing reads freed memory:

| Freed | What still uses it does |
|---|---|
| Mesh | A model showing it draws nothing, like `Mesh::NONE`. An instance batch drawing it (or one of its levels) draws nothing from the next frame on, never the mesh made later in the freed one's slot; free the batch first. |
| Texture | Materials and sprites sample white, as if they had no texture. A freed normal, ORM or emissive map is ignored. |
| Material | Meshes draw with the default material. |
| Shader | Materials using it draw with the built-in shader. |
| Texture array | Materials sample a white array; decals using it are turned off. |
| Render target | Cameras, compute passes and 3D panels drawing into it are skipped. |
| Effect, compute pass | They are left out of the frame. |
| GPU buffer | Its compute passes are dropped; meshes drawing from it draw nothing. |
| Sound | Its playing voices stop; playing it again does nothing. |
| Font | Text in it uses the built-in font. |

Updating a freed handle (`update_texture`, `update_material`) does nothing,
and a hot-reloaded file no longer touches it. Loading a freed file again
(`load_mesh`, `load_texture`) loads a fresh copy with a new id.

## Loaded files and their caches

What loads by name is cached by name: loading the same file again returns
the same handle and reads nothing. Freeing the handle also forgets the
cache entry and stops hot reload watching the file, so the next load reads
it afresh. Hot reload replaces an edited texture, sound, font or shader in
place, under the same handle, without making anything new.

A glTF model is more than its mesh. `load_mesh` also makes the file's
materials and the textures they use, and they outlive the mesh:

- **Freeing the mesh** keeps the file's materials and textures, with any
  edits made with `update_material`, for the next `load_mesh` of the same
  file. A level that loads and frees the same models every time stays flat.
- **Freeing one of those materials or textures** and loading the file again
  makes only what was freed again; the rest are reused.
- **`ctx.gfx.unload_mesh(path)`** frees everything loading the file made:
  its mesh, its materials and its textures, except images another loaded
  file shares. Use it in a long-running game that loads many different
  models over time; it returns false when the file wasn't loaded.

To hide something for a while, show `Mesh::NONE` instead of freeing it, and
show the real mesh again later. Free when you won't need it again.

## Counting

`ctx.live_counts()` returns how many handles of each kind are live. Compare
two counts to find a leak: a level that loads and unloads should end where it
started.

```rust
let before = ctx.live_counts();
// ...load and unload a level...
assert_eq!(ctx.live_counts(), before);
```

## Finding a leak

`ctx.asset_inventory()` counts the same handles by class with estimated
bytes (its texture count also includes the engine's own textures), and its
`dangling` list names the sprites and models that still show a freed
texture, mesh or material. The control endpoint's `perf.get` includes it as
`assets`. Take one as a baseline, run the game for a while, and
`growth_since(&baseline)` names the classes that grew, the biggest first.

`ctx.gfx.asset_handles(class)` (for sounds `ctx.audio.asset_handles()`)
then lists every live handle of that class,
oldest first, with its bytes, the frame it was made on, its age in frames
and its owner: game code, the asset file that loaded it (for a glTF model's
materials and textures, the model's file), or the engine. Handles that keep
getting older while the class grows are the ones nothing frees.

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/asset_inventory.rs:asset_leaks}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:asset_leaks}}
```

</div>
</div>

Both read the engine's own handle tables, so they also work in headless
tests; a windowed game's inventory adds `gpu`, what the renderer really
holds, to cross-check them. Leak tests in the engine load, free, replace
and hot-reload assets in loops and expect `growth_since` to stay empty.

## The older names

The per-kind calls (`free_mesh`, `free_instance_batch`, `has_material`,
`has_shader`, `has_compute`, `has_effect`, `has_font`, `is_mesh_live`) were
removed in 0.1.0: use `ctx.gfx.free(h)` and `ctx.gfx.is_live(h)`. `gpu_buffer_len`, `instance_batch_len`,
`has_texture_array` (which also checks the size) and `is_render_target` stay: they answer more than whether a handle is live.

## Handles of several meshes

A `TerrainTile` holds one mesh per detail level. It is a handle too:
`ctx.is_live(tile)` while all its meshes are, and `ctx.free(tile)` frees
them all. `tile.lods()` gives the meshes, to set their shadows or look up
their cost.

## In C

C passes the kind and the id:

```c
{{#include ../../examples/cpp/guide.c:handles}}
```

An unknown kind is an invalid argument. `PONKO_HANDLE_NONE` is the empty id
for every kind. C API calls that take a handle report a freed one as an
invalid argument.

The inventory takes a class (`PonkoAssetClass`, which counts the engine's
textures and lists render targets on their own) instead of a kind:

```c
{{#include ../../examples/cpp/guide.c:asset_inventory}}
```
