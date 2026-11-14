# Asset sources: assets that don't live in `assets/`

By default a named asset (`"sprites/crab.png"`, `"rules.json"`) is a file in
the `assets/` folder (on the web, a file listed in `assets/index.json`). An
**asset source** is another place to look first: bytes compiled into the
executable, a pack file, or files made up in a test. A game adds one in
setup; after that, the same loads by name find the asset there, and the rest of
the code doesn't change.

C and C++ have asset sources and `load_asset_with` too;
`scene_asset_field` is Rust-only for now (see
[Still Rust-only](c-cpp.md#still-rust-only)).

## Embedding files in the executable

`MemorySource` holds named files in memory. Fill it with `include_bytes!` and
the game needs nothing next to the executable:

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/asset_sources.rs:embedded}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:asset_sources}}
```

</div>
</div>

`ctx.assets.add_source(source)` takes any type that implements `AssetSource`,
and `MemorySource` is the ready-made one. Your own is two methods: `id` (a
short name, unique among the sources you add) and `read(name)` (the bytes, or
an error of kind `NotFound` when the source doesn't have that name). Use it
for a pack file or a download cache.

From C++, `ctx.add_asset_source(ponko::memory_source(id).with(name, bytes))`
adds a copy of a memory source, and `ctx.add_asset_source(id, read)` takes a
function from a name to `std::optional<std::vector<std::uint8_t>>` (`nullopt`
when the source doesn't have it; an exception is reported as an I/O error).
From C, `ponko_memory_source_new`, `ponko_memory_source_insert` and
`ponko_ctx_add_memory_source`, or `ponko_ctx_add_asset_source` with a read
callback: it returns `PONKO_OK` with the bytes (which stay valid until the
next call to it), `PONKO_NOT_FOUND` to let the next source answer, or another
status for an I/O error. It runs inside the load that needs it and must not
call back into the engine. The engine owns `user` once the call succeeds and
hands it to the optional free callback when the game ends; on a failure it is
never freed.

## The rules

- **Lookup order.** Added sources are asked in the order you added them, then
  the `assets/` folders. The first one that has the name wins, so a source can
  override a file in `assets/`. A source that lacks a name returns a
  `NotFound` error and the next one is asked; any other error (a disk read that
  failed, say) stops the search and is reported.
- **Names.** Names are the same logical names the default source uses:
  `/`-separated, relative, `"sprites/crab.png"`. Scenes and prefabs refer to
  assets by these names too, so a scene stored in a source finds the textures
  stored in it.
- **Caching.** A loaded asset is cached by its source's `id` and its name.
  Loading a name again returns the first handle without reading the source
  again. The same name in two sources is two assets.
- **Add sources in setup**, before loading what they hold: an asset loaded
  earlier stays cached where it was found.
- **No hot reload.** Only files found in `assets/` on a desktop build are
  [watched](drawing.md#hot-reload). Assets from added sources load once and
  never change while the game runs. If a source overrides a file that was
  loaded from `assets/` before the source was added, the folder's copy is
  still watched and reloads; the source's copy does not.
- **In a browser** sources work the same: they are asked first, and what they
  have is never fetched. Only names they lack are looked up in the files
  fetched from `assets/index.json`. The browser smoke test (`web/smoke/`)
  checks this with an image and streamed music from an in-memory source.
- **Errors name the source.** A missing name lists the sources and folders
  searched; a corrupt file says which source it came from.

## Which loads use sources

Every load by name on `Ctx`: `load_texture`, `load_font`, `load_resource`,
`load_scene`, `spawn_prefab` (and the assets scenes and prefabs name),
`load_sound`, `ctx.audio.load_stream`, `ctx.assets.load_with`, and with the `3d` feature
meshes, skeletons and animation clips (and the buffers and images a glTF file
refers to), texture arrays, reflection probe images, shaders, screen effects
and compute passes. The `try_` forms (`ctx.gfx.try_load_texture`, `ctx.audio.try_load`,
`ctx.audio.try_load_stream`, ...) return an error instead of panicking.

A few details:

- **Streamed music** (`load_stream`) from a source is kept in memory still
  encoded, and decoded as it plays; from `assets/` it is read from the file
  as it plays. In a browser it is kept encoded too, and decoded a packet at
  a time as it plays.
- **glTF images** that are files of their own are shared between models
  loaded from the same source: two models naming `"tex.png"` show one
  texture, read once. The same name found in two sources is two textures.
  A source can say it has a name without handing over its bytes by
  overriding `AssetSource::contains` (by default it reads the name).
- **Texture arrays, probe images and `load_asset_with`** read their source
  on each call; keep what they return.

## Your own asset types

`ctx.assets.load_with(name, decode)` reads a named asset from the sources
(then `assets/`) and hands its bytes to your decode function: no
registration. `decode` returns your value or a `ponko::error::Error` (usually
`ErrorKind::Decode`), which comes back with the asset's name and source in
front of its message. Nothing is cached, so keep the result, for example in
a resource.

C++: `ctx.load_asset_with(name, decode)`, where `decode(data, len)` returns
your value or throws; the example above counts the walls of a level file. C:
`ponko_ctx_load_asset_with(ctx, name, decode, user)`, where `decode` returns
`PONKO_OK` or a status that becomes a `decode` error naming the asset. The
decoder runs outside the engine's lock, so it may call other `ctx` functions.
The bytes are valid only during the call.

## Assets named in your own components

A scene saves the textures of `Sprite`s and meshes of `Model`s by name, and
loads them before it spawns anything. Your own reflected components can do
the same: keep the asset's **name** in the component (a `String`, or a list
of them) and tell the engine how to load it with
`ctx.assets.scene_asset_field::<T>(field, load)`, where `load` returns
`Result<(), ponko::error::Error>` (an engine `try_` loader can be returned as it is):

```rust
{{#include ../../tests/asset_sources.rs:portal}}
```

```rust
{{#include ../../tests/asset_sources.rs:scene_asset_field}}
```

`load_scene`, `spawn_prefab`, the editor and the control endpoint then call
`load` with each name in that field before spawning, so an asset that can't
be loaded fails the load with the component, field and name, before any
entity changes. A system gets the handle by loading the name again
(`ctx.gfx.load_texture(&item.icon)`), which returns the cached one.

The engine deliberately doesn't turn names into handles inside your
components: the component keeps exactly what the file says, saves
unchanged, and `load` decides what a name means, including asset types of
your own.

A source also makes headless tests self-contained: a test that adds a
`MemorySource` with a generated image or a JSON file doesn't depend on the
repository's `assets/` folder (see [Headless runs and
testing](headless.md)). The engine's own tests do this in
[`tests/asset_sources.rs`](https://github.com/ponkoengine/ponko/blob/main/tests/asset_sources.rs).
