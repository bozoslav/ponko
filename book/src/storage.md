# Saves and settings

`ctx.storage` keeps the player's own files: settings, progress, save games.
On the desktop they live in the player's data folder; in a web build, in the
browser's storage. The calls are the same on both, and every write is safe
against crashes.

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/storage.rs:storage}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:storage}}
```

</div>
</div>

## Where the files go

Give the game an app id with `Game::app_id` (C++ `game::app_id`, C
`ponko_game_app_id`). Its files go in a folder of that name:

| System | Folder |
|---|---|
| Windows | `%APPDATA%\<app id>\` |
| macOS | `~/Library/Application Support/<app id>/` |
| Linux and other Unix | `$XDG_DATA_HOME/<app id>/`, else `~/.local/share/<app id>/` |
| Web | The page's IndexedDB database `ponko:<app id>` |

Pick the id once and keep it: a new id is a new folder, and the old saves stay
in the old one. Letters, digits, `-`, `_` and `.` (`"space-crab"`,
`"com.example.crabs"`). A game without an app id keeps no files: every call
fails with `ErrorKind::Unsupported`, and `ctx.storage.is_available()` is
`false`.

`ctx.storage.dir()` is the folder (`None` on the web). Two things move it:

- `PONKO_DATA_DIR=<folder>` replaces the system's data folder; the files go
  in `<folder>/<app id>/`. Handy for a smoke test of a packaged game that must
  not touch the real saves.
- `Game::data_dir(folder)` names the exact folder. Tests use it to give each
  test a folder of its own; a portable install can keep its saves beside the
  game.

## Reading and writing

| Call | What it does |
|---|---|
| `write(name, bytes)`, `write_json(name, &value)` | Writes the file, creating its folders. |
| `read(name)`, `read_json::<T>(name)` | The file's bytes, or the value. `ErrorKind::NotFound` if there is none. |
| `read_backup(name)` | The file as it was before its last write. |
| `exists(name)`, `list()` | Whether a file is there; every file's name, sorted. |
| `delete(name)` | Deletes the file and its backup. A missing file is fine. |

Names are relative, with `/` between folders (`"saves/slot1.json"`). Each part
uses letters, digits, spaces, `-`, `_` and `.`, and doesn't start with a `.`;
anything else (`..`, `\`, `:`, a device name like `NUL`) is
`ErrorKind::Invalid`, so a name can't leave the folder or mean something
different on another system. Names ending in `.tmp` or `.bak` are the engine's
own (see below).

Errors are `ponko::prelude::Error`s with the file's name as their path. A
missing file is a first run, not a failure: start from the defaults. A file
that doesn't parse, or holds values your game can't have produced, may be
damaged: start from the defaults, tell the player, and keep the bytes (the
reference game copies them to `<name>.bad`). A write that fails (a read-only
or full disk) shouldn't stop the game either; say that it couldn't save.

## Safe writes

A crash, a killed process or a power cut in the middle of a write never
leaves a half-written file. `write` does it in steps:

1. the bytes go to `<name>.tmp`, and are flushed to the disk;
2. the current file is kept as `<name>.bak`;
3. a rename puts the new file in place. A rename is atomic: the name is the
   old file or the new one, never a mix.

A crash before step 3 leaves the old file in place, and the next write
replaces the leftover `<name>.tmp`. `list` doesn't show either suffix.
`read_backup` gives the version before the last write, for a game that wants
to offer "the save before" when the newest one is damaged by something else.
The engine's tests simulate a crash between each step and check that the old
save is read back whole.

## Save slots

`ctx.storage.save_world(&ctx.world, slot)` saves the world's reflected
components and resources ([reflection](reflection.md)) to `saves/<slot>.json`,
and `ctx.load_world(slot)` replaces the world with it: the same load as
[`load_scene`](scenes.md), from the player's storage instead of the assets, so
its table of what a load replaces and keeps applies. `world_slots()` lists the
slots, `delete_world(slot)` removes one. C and C++: `ponko_ctx_save_world`,
`ponko_ctx_load_world` and `ctx.save_world`, `ctx.load_world`.

A save holds what the game can't rebuild: entities with `NotSaved` are left
out. A [level spawned from Tiled or LDtk](tiles.md#levels-and-saves) is, all
but its prefab objects; after `load_world`, spawn it again with
`LevelOptions::without_saved_objects`.

A save slot is one player's file, so `load_world` is refused while a
[lockstep session](networking.md) is connecting or running: the other peers
don't have it.

## Save migrations

The engine reads its own older save layouts by itself. Your components are
yours: rename one, or change what a field means, and the saves players
already have stop loading. `#[serde(default)]` covers a new field, but not a
rename or a conversion. `SaveMigrations` (in `ponko::reflect`) covers the
rest. It is the game's save version, plus the changes from each older
version to the next. Put it in the world as a resource before the first load:

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/save_migrations.rs:migrations}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:save_migrations}}
```

</div>
</div>

- **The version.** Every world save records the game's version as
  `"game_version"`. A save without one, from before the game had
  migrations, is version 0. Raise the version whenever a change would break
  older saves, and add a step from the old version that says what changed.
  A version that needs no change needs no step.
- **Steps run in a chain.** Loading a save of version 1 into a game of
  version 3 runs the steps from 1 and from 2, in that order, on the save's
  JSON. Then the load goes on as usual: every component is checked before
  the world changes. A step never runs twice, and each step only has to know
  its own two versions.
- **Every load migrates.** `ctx.load_world`, `load_scene`, `World::load`,
  the editor and the control endpoint's `world.load` all run the steps, so a
  scene saved by an older version of the game loads too.
  `SaveMigrations::migrate(&save)` returns the migrated JSON without loading
  it, if you want to look at it or keep it.
- **A save from a newer game is refused.** If the save's version is above
  the game's, this build can't know what changed, so the load fails with
  `ReflectError::NewerSave { saved, current }` and the world is unchanged.
  Through `Ctx` it is an `Error` of kind `ErrorKind::Unsupported`, with both
  versions in the message: tell the player to update the game. A step that
  fails (a `convert_field` returning `Err`, a rename onto a component the
  entity already has) is `ReflectError::Migration` (kind `Json`). It names
  the versions, the entity and the component.

What a `Migration` can change:

| Change | Call |
|---|---|
| Component renamed | `rename_component(from, to)` |
| Component removed | `remove_component(name)`. An entity left with nothing is dropped. |
| Field renamed, added, removed | `rename_field`, `add_field(component, field, value)`, `remove_field` |
| Field retyped or its meaning changed | `convert_field(component, field, \|old\| Ok(new))` |
| Component retyped (a tuple struct, an enum) | `convert_component(name, \|old\| Ok(new))` |
| Component split | `split_component(from, &[(part, &[fields]), ...])` |
| Components merged | `merge_components(&[from, ...], into)` |
| Reflected resource renamed, removed, converted | `rename_resource`, `remove_resource`, `convert_resource` |
| Anything else | `entities(\|components\| ...)` for each entity, `save(\|save\| ...)` for the whole save |

Names are the names in the save: a component's reflected name (its type name
without the module path, or the name C++ gave it) and its serde field names.
Each change skips entities that don't have the component, so one step suits
every save of its version. Changes of one step run in the order you added
them, so a field of a renamed component goes by its new name.

Prefab and level files are the game's own assets, not players' saves: they
aren't migrated. Save them again after a change.

### Testing old saves

Keep a save from each released version in your tests, for example in
`tests/old-saves/`, and check that they still load:

```rust
# use ponko::prelude::*;
# fn setup(_: &mut Ctx) {}
# let dir = std::env::temp_dir().join(format!("ponko-book-old-saves-{}", std::process::id()));
# std::fs::create_dir_all(&dir).unwrap();
# std::fs::write(dir.join("v1.json"), r#"{"format":"ponko-world","version":1,"entities":[]}"#).unwrap();
# let old_saves = dir.clone();
Game::new("Crabs", 800, 600).run_headless(0, move |ctx| {
    setup(ctx); // the game's reflection and SaveMigrations
    let loaded = ctx.check_old_saves(&old_saves).expect("old saves load");
    assert!(loaded > 0);
});
# let _ = std::fs::remove_dir_all(dir);
```

`ctx.check_old_saves(folder)` loads every `.json` save in the folder the way
`load_world` does, then saves the world and loads that again. The error lists
every file that failed and why. The folder is read from the file system
(`cargo test` runs in the package's folder), not from the assets or the
player's storage. The engine's own test keeps saves of three versions of a
small game in `tests/fixtures/game-saves`. Don't edit an old save to make
the test pass: add a step instead.

### Migrations in C and C++

| C | C++ |
|---|---|
| `ponko_world_set_save_version`, `ponko_world_save_version` | `world.set_save_version(v)`, `world.save_version()` |
| `ponko_save_migration_rename_component(world, from_version, from, to)`, `_remove_component` | `world.migration(from_version).rename_component(from, to)`, `.remove_component(name)` |
| `ponko_save_migration_rename_field`, `_add_field` (value as JSON text), `_remove_field` | `.rename_field`, `.add_field`, `.remove_field` |
| `ponko_save_migration_split_component(world, v, from, parts_json)` (`[["Regen", ["hp_regen"]], ...]`), `_merge_components(world, v, names, count, into)` | `.split_component(from, parts_json)`, `.merge_components({...}, into)` |
| `ponko_save_migration_rename_resource`, `_remove_resource` | `.rename_resource`, `.remove_resource` |
| `ponko_save_migration_convert_field`, `_convert_component`, `_convert_resource`, `_change_entities`, `_change_save` | `.convert_field(component, field, fn)` and the rest, with a `std::function<std::string(const std::string&)>` |
| `ponko_world_migrate_save` | `world.migrate_save(json)` |
| `ponko_ctx_check_old_saves(ctx, folder, &count)` | `ctx.check_old_saves(folder)` |

A C conversion is a `PonkoJsonConvertFn`. It gets the saved value's JSON
text and returns `PONKO_OK`, with `*out_json` pointing to the new value's
JSON, which must stay valid until its next call. Any other status fails the
load, and `*out_json` may then point to the reason. The world keeps the
`user` pointer and calls `free(user)` once when it is dropped, or at once if
the call fails. A C++ conversion that throws fails the load with the
exception's message. A save from a newer game fails with the error kind
`PONKO_ERROR_UNSUPPORTED`.

## In the browser

Browsers only offer their storage asynchronously, so a web build reads every
stored file into memory while the page fetches its assets, before setup runs.
After that the calls work as on the desktop:

- reads, `exists` and `list` answer from memory;
- a write or delete changes memory at once, and the browser stores it in one
  IndexedDB transaction: the file and its backup together, so a tab closed
  halfway keeps both old versions or both new ones;
- `ctx.storage.is_saving()` is `true` until the browser confirmed every
  change. Closing the tab sooner may lose the last write (never half of it).
  On the desktop it is always `false`: `write` returns once the file is on the
  disk.

The same game code works on both; nothing needs to wait or poll. Two
differences remain. Some private windows refuse a site its storage:
`ctx.storage.is_persistent()` is then `false` and files last until the page
closes. And browsers may clear a site's storage when the disk runs low, so
keep a player's work small and expect a first run now and then. The [web
smoke test](web.md#testing-in-a-browser) runs the reference game in a headless
browser, leaves its settings screen (which saves), reloads the page and checks
that the settings come back from the browser's storage.

## C and C++

| C | C++ (`ponko::ctx`) |
|---|---|
| `ponko_game_app_id`, `ponko_game_data_dir` | `game::app_id`, `game::data_dir` |
| `ponko_ctx_storage_write(ctx, name, bytes, len)` | `storage_write(name, bytes, size)`, `storage_write(name, text)` |
| `ponko_ctx_storage_read(ctx, name, buffer, capacity, &len)` | `storage_read(name)` (bytes), `storage_read_text(name)`: nothing if missing |
| `ponko_ctx_storage_read_backup` | `storage_read_backup(name)` |
| `ponko_ctx_storage_exists`, `ponko_ctx_storage_delete` | `storage_exists(name)`, `storage_delete(name)` |
| `ponko_ctx_storage_list` (names, one per line) | `storage_list()` |
| `ponko_ctx_storage_dir` (empty on the web) | `storage_dir()` |
| `ponko_ctx_save_world`, `ponko_ctx_load_world` | `save_world(slot)`, `load_world(slot)` |
| `ponko_ctx_storage_world_slots` (slots, one per line), `ponko_ctx_delete_world` | `storage_world_slots()`, `delete_world(slot)` |
| `ponko_ctx_storage_is_persistent`, `ponko_ctx_storage_is_saving` | `storage_is_persistent()`, `storage_is_saving()` |

`Storage::write_json` and `read_json` are Rust-only (see
[Still Rust-only](c-cpp.md#still-rust-only)): they are generic over serde
types, so C and C++ keep bytes with `storage_write` and `storage_read`, and
parse them themselves.

The C reads take a buffer and always set the length: call with a null buffer
to learn the size, then again with one that big. A missing file is
`PONKO_NOT_FOUND`; storage without an app id fails with the error kind
`PONKO_ERROR_UNSUPPORTED` (`ponko_last_error_kind`).
