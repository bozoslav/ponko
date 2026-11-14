### Added
- `Animation` (sprite-sheet animation) is reflected as `"Animation"`: its
  sheet's image by file name, frame size and grid, frames, fps, looping and
  time. Prefabs and scenes can give a sprite its animation (a file needs
  only `texture`, `frame_size` and `fps`), and world saves keep each
  animation's frame. Loading a scene or prefab loads its image first.
- `ponko::reflect::NotSaved`: entities with it are left out of
  `WorldReflect::save` and everything built on it (save slots, scenes, the
  editor, the control endpoint's `world.save`).
- `LevelOptions::without_saved_objects` spawns a level without its prefab
  objects, for spawning it again around a loaded save. C:
  `ponko_ctx_spawn_level_without_saved_objects`, `ponko_world_set_saved`,
  `ponko_world_is_saved`; C++: `level_options::without_saved_objects`,
  `world.set_saved`, `world.saved`.

### Changed
- `Ctx::spawn_level` marks everything it spawns `NotSaved` except the
  objects it spawns as prefabs, so world saves hold a level's prefab objects
  (the game's state) but not its tiles, colliders, plain objects or root,
  which come back from the level file. Saves written before still load as
  before.
- Ember Hop's prefabs and title scene hold their animations; its
  `decorate` workaround is gone and `rehydrate` only spawns the level again.
