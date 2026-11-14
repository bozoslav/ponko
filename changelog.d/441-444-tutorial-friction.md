### Added
- `Ui::set_focus(id)` moves the retained UI's keyboard and gamepad focus from
  code (it returns `false` for a node that can't take it), and
  `Ui::clear_focus()` takes it away. `Ui::set_initial_focus(id, on)`, or the
  builder's `.focused()`, marks the node a screen opens on: every
  `push_screen` of the screen focuses it, so a menu is ready for Enter at
  once. C: `ponko_ctx_ui_set_focus`, `ponko_ctx_ui_clear_focus`,
  `ponko_ctx_ui_set_initial_focus`; C++: `tree.set_focus`, `clear_focus`,
  `set_initial_focus`.
- HUDs: `Ui::show_hud(root)` shows display-only UI (a score, a counter)
  below every screen. Escape never closes it, it takes no pointer or focus,
  and it doesn't make `wants_pointer` or `wants_keyboard` true;
  `Ui::hide_hud` and `Ui::is_hud` go with it. C: `ponko_ctx_ui_show_hud`,
  `ponko_ctx_ui_hide_hud`, `ponko_ctx_ui_is_hud`; C++: `tree.show_hud`,
  `hide_hud`, `is_hud`.
- `ConfiguredGame` takes the settings a test adds to a finished game:
  `headless_frames`, `input_scenario`, `run_headless_scenario(scenario)`,
  `data_dir`, `seed`, and test systems with `system` and `sim_system`. The
  tests of a `ponko new` project can drive it with frames, a scenario and a
  storage folder of their own without changing `src/lib.rs`.
- `Ctx::load_collision_mesh(path)` makes a glTF file solid in one call (a
  triangle mesh of its meshes, for a mesh collider), and
  `load_collision_mesh_with(path, CollisionMeshOptions { scale, convex })`
  scales it or makes a convex hull. It shares the scene loader's code and
  cache, and a collider of it is saved by its file. C:
  `ponko_ctx_load_collision_mesh`; C++: `ctx.load_collision_mesh`.
- `MeshData::triangles()`: the indices three at a time.
- The `ponko new` 2D and 3D templates have a test that drives the game with
  headless frames, an input scenario and a data folder of its own.

### Changed
- The tutorials' games return a `ConfiguredGame`, as the templates do, so
  `src/main.rs` stays as `ponko new` wrote it. The 2D title screen opens on
  Play, the 3D counter is a HUD, and the 3D yard is made solid with
  `load_collision_mesh`.
- A scene's mesh collider that writes `scale` and `convex` out as their
  defaults now shares its mesh with one that leaves them out.
- `ponko_ctx_ui_push_screen` fails with `PONKO_INVALID_ARGUMENT` for a root
  shown as a HUD.
