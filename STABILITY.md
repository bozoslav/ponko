# Stability

What a Ponko release may change, and what it keeps. Ponko is at **0.x**: the
API is still settling, and the rules below say how breaking changes happen
while it does.

Ponko was formerly Crabby Engine. 0.1.0 removed every old name; the
[naming chapter](book/src/naming.md) maps them to the new ones.

## Versions

The crates follow [semver as Cargo applies it to 0.x](https://doc.rust-lang.org/cargo/reference/semver.html):

- **Patch releases (0.1.x)** are compatible with each other: bug fixes and
  additions only. `ponko = "0.1"` in Cargo.toml picks up the newest one, and
  your code keeps compiling and behaving as documented.
- **Minor releases (0.2.0, 0.3.0, ...)** may break things: rename, move or
  remove items, change signatures, behavior or defaults. Every break is
  listed in the CHANGELOG under **Breaking changes** with what to use
  instead, so the CHANGELOG is the migration guide.
- Removing something does not wait for a deprecation period. Where it is
  cheap, a patch release marks the item `#[deprecated]` (Rust) or adds a note
  in the header (C) first, pointing at its replacement.

1.0 will come when the API has stopped moving; from then on, only major
releases break. [Releases](#releases) says when each kind of release happens.

What the versions cover:

| Crate | What is covered |
|---|---|
| `ponko` | Every public item that is not `#[doc(hidden)]`, and the cargo features, except the [unstable surfaces](#unstable) |
| `ponko_ecs` | Every public item that is not `#[doc(hidden)]` |
| `ponko_macros` | The derives (`#[derive(Component)]`) and the attributes they accept, used through `ponko` or `ponko_ecs`. The code they generate is not covered. |
| `ponko_ffi` | The C ABI in `include/ponko.h` and the C++ wrapper `include/ponko.hpp` (see [C ABI](#c-abi)) |
| `ponko_mcp` | The MCP tool names and their parameters |
| `ponko_unity` | Its Rust API. The files it writes (the imported scenes, meshes and materials) are not a stable format: re-import after upgrading. |

Enums and structs that will grow are `#[non_exhaustive]` (`ErrorKind`, `Key`,
`MouseButton`, `GamepadButton`, `NetState`, `BodyKind`, the render
descriptors and settings, ...): matches need a `_` arm and such structs are
built through their constructors or `Default`, so new variants and fields
can arrive in a patch release.

The image-format features (`image-jpeg`, `image-webp`, `image-bmp`, `image-tga`,
`image-gif`, `image-tiff`, `image-ico`, `image-qoi`, `image-pnm`, `image-dds`,
`image-hdr`, `image-exr` and `image-formats` for all of them) are covered like
every cargo feature: they stay, and keep the formats they name. `image-jpeg`
is a default feature, and `3d` turns on `image-jpeg`, `image-hdr` and
`image-exr`. PNG is always built in. A new format arrives as a new feature,
never as a change to what the existing ones turn on.

## Releases

| Release | When | Contains |
|---|---|---|
| Patch (0.1.1, 0.1.2, ...) | As needed: within days for a regression or a security fix, otherwise when there are fixes to ship | Bug fixes and additions. Nothing that breaks code written for the same minor. |
| Minor (0.2.0, 0.3.0, ...) | When the P1 scope of its [milestone](https://github.com/ponkoengine/ponko/milestones) is done, roughly every 8 to 12 weeks | New features and breaking changes, each break with its migration entry. |
| 1.0 | When the [readiness checklist](https://github.com/ponkoengine/ponko/issues/432) is complete | The last 0.x release with the compatibility promise attached. It breaks nothing. |

- **A release is scope, not a date.** P1 issues define a minor; what is not
  ready moves to the next one instead of delaying or rushing the release.
- **Release candidates.** From 0.2.0 on, a minor is published as
  `0.y.0-rc.1` at least a week before the release. Cargo never selects a
  release candidate unless the version is written out
  (`ponko = "0.2.0-rc.1"`). A regression found in one is fixed and a new
  candidate follows; the release is the last candidate, unchanged. Patch
  releases have no candidates.
- **Support.** The newest minor gets patch releases. The minor before it
  gets fixes for security problems, crashes and data loss for four weeks
  after a new minor, so there is time to migrate. Older ones get none.
- **Patch releases come from a release branch** (`release-0.1`, ...), so
  `main` can take breaking changes for the next minor right after a
  release. Fixes land on `main` first and are carried over (the `backport`
  label marks them; CONTRIBUTING.md, "Releasing", has the procedure).
- **Deprecation.** Where it is cheap, a replaced item stays as
  `#[deprecated]`, naming its replacement, until the next minor removes it.
  Where it is not, it goes at once, with its migration entry.
- **Yanking.** A release is yanked only if it loses data, fails to build or
  has a security problem, and a fixed patch release goes out with it.
- **After 1.0.** Minor releases only add, major releases break, and majors
  are at most yearly; the exact support periods are written here before 1.0
  is released.

## Unstable

Not covered by the versions, and allowed to change in any release:

- `#[doc(hidden)]` items, among them `ponko::__ffi` (the engine internals that
  `ponko_ffi` builds on), test helpers such as `Net::loopback_lossy`, and
  macro internals.
- The `bench-internals` cargo feature and what it turns on (benchmark
  drivers for `crates/ponko_bench`, which is not published).
- `ponko::ui::unstable_taffy`, a re-export of the `taffy` layout crate for
  advanced UI layout.
- **Re-exported third-party crates and types.** Their major versions are
  Ponko's choice, and a Ponko minor release can move to a new one, which is a
  breaking change for code that uses them directly (it is listed under
  Breaking changes): `glam` (`Vec2`, `Vec3`, `Quat`, `Mat4`, ... in the crate
  root and the prelude), `serde_json` (`ponko::json!` and reflection values)
  and `steamworks` (`ponko::net::steam`, feature `steam`).
- Error message text, log output, the inspector's look, and the exact
  frames rendered: tests should check behavior, not strings or pixels.
- Performance. Regressions are bugs, and benchmark numbers go in PRs, but
  timings are not part of the API. CI tracks the instruction counts of a few
  workloads per commit and comments on pull requests that move them
  (`crates/ponko_bench/README.md`, "Tracking results over time").

## Errors and panics

Fallible calls follow one convention, built on `ponko::error::Error`:

- `load_x` / `create_x` are for setup code, where a missing file is a bug.
  They panic with the `Error`'s message (which starts with `ponko:`).
- `try_load_x` / `try_create_x` return `Result<T, ponko::error::Error>`. Match on
  `Error::kind()` (a `#[non_exhaustive]` `ErrorKind`), never on the message.
- Operations that fail in normal play (scene loads, prefab spawns, network
  joins) return a `Result` under their plain name and have no panicking twin.

Every loader and creator on the `ctx.gfx` and `ctx.assets` services follows
it: textures, meshes, mesh data, skeletons, clips, sounds, fonts, materials,
shaders, screen effects, compute passes, render targets, texture arrays,
terrain materials, UI panels and reflection probes, plus the data-file
loaders and scene and prefab saving. Callbacks a game hands these services
(`Assets::load_with`'s decoder, `Assets::scene_asset_field`'s loader) return
`ponko::error::Error` too, and so do the play-time calls on `Ctx`
(`load_scene`, `load_scene_lockstep`, `spawn_prefab`, `start_ragdoll`),
performance reports and budgets (`PerfReport::from_json`, `load`,
`Budget::from_json`), and checks (`MeshData::check`,
`EcsDiagnostics::matching`). `String: From<ponko::error::Error>` lets `?` turn
an `Error` into a `String` (its message, without the prefix) in code that uses
strings for errors.

## The crate root and globs

The crate root holds only modules: every item has one path, in its module,
and the prelude re-exports the common ones as the same items. Games start from `use ponko::prelude::*;`
and import modules as needed; `use ponko::*;` brings in the modules, not the
prelude.
`ponko::ecs_core` is the one way into the ECS crate beyond the prelude, and
`d2`/`d3` are the one path of the types named after a dimension.
`scripts/check-naming.py --paths` fails CI on an item with a second public
path; a `#[deprecated]` alias of a moved item stays for one minor.

The root glob also brings in the root modules (`audio`, `input`, `net`,
`render`, `ui`, ...). For a game that writes it and depends on a crate of the
same name, a path in code then means the Ponko module, and a
`use name::...;` declaration is ambiguous (E0659); the way out is to spell
the crate `::name::...` or to import `ponko::prelude::*` instead. A new root
module is therefore a source change for such games, so the CHANGELOG names
every root module a release adds.

## Minimum Rust version (MSRV)

Ponko builds with **Rust 1.95** and newer (`rust-version` in Cargo.toml;
CI checks it). The MSRV may rise in a minor release (0.y.0), never in a
patch release, and the CHANGELOG says so. It follows the dependencies
(wgpu, egui, rapier) and stays at least a few releases behind stable.

## C ABI

The C ABI has its own version, `ponko_abi_version()` and `PONKO_ABI_VERSION`
in `ponko.h` (currently **2**). It changes whenever an existing function,
struct layout or enum value changes incompatibly, which only happens in a
minor release (0.y.0), never in a patch release; the CHANGELOG says so.
Within an ABI version, releases only add functions and enum values, and
existing struct layouts never change. Programs that load the library
dynamically check the version at startup:

```c
if (ponko_abi_version() != PONKO_ABI_VERSION) { /* built against another version */ }
```

A failing call returns its `PonkoStatus`, and `ponko_last_error()` has its
message. `ponko_last_error_kind()` says what kind of failure it was, as a
`PonkoErrorKind` that mirrors Rust's `ponko::error::ErrorKind` (`PONKO_ERROR_NOT_FOUND`,
`_IO`, `_DECODE`, `_SHADER`, `_INVALID`, ..., `_OTHER`); new kinds can arrive
in any release, so a `switch` needs a `default:`. Statuses don't change
with it: a call that returned `PONKO_INVALID_ARGUMENT` for a missing file
still does, and says `PONKO_ERROR_NOT_FOUND` as its kind. The C++ wrapper
throws `ponko::error`, a `std::runtime_error` with `kind()`.

The C and C++ names stay flat (`ponko_ctx_cast_ray_filtered`,
`ctx.graphics_quality()`, ...) even where the Rust API groups things into
modules and services.

## Saved worlds

`World::save` writes JSON tagged `"format": "ponko-world"` and a save version
(currently 1). A release reads every save version that earlier 0.x releases
wrote; a release that changes the format says so in the CHANGELOG and still
loads the older versions. Your own components are yours to migrate: give a
component `#[serde(default)]` and saves written before you added a field
still load.

`World::load` refuses entity ids whose index is far beyond what the save
needs (the exact bound is in its docs), so a corrupt or hostile file can't
make the world allocate a huge entity table. Saves the engine writes stay
within it unless the world once had over 4 million entities alive and was
saved with under an eighth of them; such a save still loads back into the
world that wrote it.

## Compatibility checks

The promises above are tested against retained baselines, not only against
what the current revision generates. A baseline is a small fixture recorded at
a named engine revision with its provenance next to it; it is never edited to
make a test pass.

| Contract | Baseline | Check |
|---|---|---|
| Saved worlds, scenes and prefabs | `tests/fixtures/saves/` | `tests/save_compat.rs`, `tests/prefab_compat.rs` |
| MCP tools and parameters | `crates/ponko_mcp/tests/fixtures/` | `crates/ponko_mcp/tests/contract.rs` |
| C ABI header and layouts | `crates/ponko_ffi/tests/fixtures/abi/` | `crates/ponko_ffi/tests/abi_compat.rs`, `examples/cpp/abi_baseline.c` |

Additions pass: new MCP tools and optional parameters, new C functions and enum
constants. A removed or changed tool, parameter, function or struct layout, a
new required MCP parameter, or a parameter that accepts fewer values fails with
a message naming it. Each fixture folder's README says how to retain a new
baseline after an intentional breaking change.

## Determinism

What gives the same world from the same seed and inputs:

| | Same machine, same build | Other OS or CPU, same build | Other engine version |
|---|---|---|---|
| ECS, scheduling, `ctx.random` | yes | yes, see below | not promised |
| Your sim code with `+ - * /` and `sqrt` on `f32`/`f64` | yes | yes | yes |
| `sin`, `cos`, `powf`, `exp`, ... | yes | **may differ** | — |
| Physics (`physics`) | yes | **no** | not promised |
| Physics (`physics-determinism`) | yes | yes | not promised |

- IEEE-754 makes basic arithmetic and `sqrt` bit-identical everywhere, but
  the standard library's `sin`, `cos`, `powf` and friends call the platform's
  math library, which may round differently. For lockstep between
  different platforms, keep them out of the simulation or use a pure-Rust
  implementation such as the `libm` crate.
- Lockstep peers, and replays, must use the same game build and engine
  version. Lockstep packets carry a protocol version, and a peer with
  another version is ignored and times out.
- Iteration order of queries is deterministic but not part of the API: it
  can change between versions.
- `Game::parallel_systems` doesn't change any of this: a simulation gives
  the same world with it on or off and on any number of threads, so
  lockstep peers may differ in that setting.

## Platforms

Tested in CI on every change: Linux x86-64, macOS (Apple Silicon) and
Windows x86-64. Built in CI on every change: the web (wasm32, WebGPU with a
WebGL2 fallback); small web builds (2D, 3D and the complete-game reference)
are also run in headless Chromium and WebKit with WebGL2 in the merge
queue (`web/smoke/`; Firefox is tried there too, but the runner's headless
Firefox has no WebGL2, so it is reported, not verified). WebGPU is tried
there too and reported, but a runner without a GPU usually can't show it, so
WebGPU is verified on real GPUs by hand, not gated in CI. Other targets that
wgpu and winit support usually work but aren't tested.
