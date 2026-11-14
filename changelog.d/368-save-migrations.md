### Added
- Save migrations for a game's own components: `ponko::reflect::SaveMigrations`
  is the game's save version, written into every world save as
  `"game_version"`, plus a `Migration` step from each older version. A step
  renames, removes, converts, splits or merges components; renames, adds,
  removes or converts fields; renames, removes or converts reflected
  resources; or changes each entity or the whole save. Every load
  (`load_world`, `load_scene`, `World::load`, the editor, the control
  endpoint) runs the steps from the save's version up, in a chain, before it
  checks anything. A save from a newer game version is refused with
  `ReflectError::NewerSave` (`ErrorKind::Unsupported`), and a failing step
  is `ReflectError::Migration`, naming the versions and the entity.
  `SaveMigrations::migrate` returns a migrated save without loading it.
- `Ctx::check_old_saves(folder)` loads every old save in a folder through
  the migrations and loads it again after saving, for a game's tests.
- C and C++: `ponko_world_set_save_version`, `ponko_save_migration_*` (C
  conversions take a `PonkoJsonConvertFn` with a user pointer and free
  function), `ponko_world_migrate_save` and `ponko_ctx_check_old_saves`;
  C++ `world::set_save_version`, `world::migration(from)` (a
  `save_migration` builder) and `ctx::check_old_saves`. `ponko_world_load`
  reports a save from a newer game with the error kind
  `PONKO_ERROR_UNSUPPORTED`.
- Guide: "Save migrations" in the saves chapter.
