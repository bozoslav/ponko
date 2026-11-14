### Added
- C and C++ bindings for features that were Rust-only:
  - Reading an action map back: `ponko_action_map_count`, `ponko_action_map_name`,
    `ponko_action_map_get` (C++ `action_map::size`, `names`, `get`). Changing one
    installed action: `ponko_ctx_set_action` (C++ `ctx::set_action`).
  - Adding a built-in system back in any phase: `ponko_game_add_builtin` (C++
    `game::add_builtin`); physics and navigation steps keep their declared access.
    A built-in's declaration: `ponko_builtin_access` (C++ `access::builtin`).
    Copying a declaration: `ponko_access_clone`.
  - `LodHysteresis`: `ponko_ctx_set_lod_hysteresis`, `ponko_ctx_lod_hysteresis`.
    `LodGroup::instance_levels`: `ponko_lod_instance_levels` (C++
    `ponko::lod_instance_levels`).
  - The scene compute stage and target effects of the custom passes:
    `ponko_ctx_set_scene_compute_passes`, `ponko_ctx_scene_compute_passes`,
    `ponko_ctx_set_target_effects`, `ponko_ctx_target_effects` and
    `PonkoTargetEffect` (C++ `ponko::target_effect_of`).
  - Keyed 3D audio rolloff curves: `ponko_ctx_play_sound_at_keyed`,
    `ponko_ctx_set_audio_source3d_keyed` and `PonkoAudioKey` (C++ overloads of
    `play_sound_at` and `set_audio_source3d`). Whether audio is unlocked:
    `ponko_ctx_audio_unlocked`.
  - ECS diagnostics as JSON: `ponko_ctx_ecs_diagnostics`.
  - Save slots and storage state: `ponko_ctx_storage_world_slots`,
    `ponko_ctx_delete_world`, `ponko_ctx_storage_is_persistent`,
    `ponko_ctx_storage_is_saving`.
  - The immediate UI's own font, border width and scale:
    `ponko_ctx_ui_immediate_font`, `ponko_ctx_set_ui_immediate_font`,
    `ponko_ctx_ui_immediate_border_width`,
    `ponko_ctx_set_ui_immediate_border_width`, `ponko_ctx_ui_immediate_scale`.

### Changed
- Copying a C++ `ponko::access` copies the declaration exactly, including the
  engine types a built-in's declaration names, instead of replaying the names
  it was built from.
