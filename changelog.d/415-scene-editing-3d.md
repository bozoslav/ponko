### Added
- The inspector edits 3D scenes: a click picks the model under the mouse
  (along the camera's ray, by mesh bounds), and a move / rotate / scale
  gizmo works in world or local space with snapping to a grid, angle and
  scale steps. Right-drag with W A S D Q E flies the camera, F frames the
  selection, W / E / R switch the gizmo. Shift- or Ctrl-click builds a
  multi-selection that moves, duplicates and deletes as one undo step.
- A hierarchy panel in the inspector shows what each entity hangs off
  (attachments, joints, skins, prefab parts); dragging one row onto another
  parents it without moving it, and loops are refused.
- An asset browser in the inspector lists glTF meshes, prefabs, scenes and
  loaded materials from `assets/`: drag a mesh or prefab into the world to
  place it, a material onto a model to paint it, or open a scene.
- Redo in the editor (Ctrl+Shift+Z or Ctrl+Y, and a button); undo and redo
  cover moves, gizmo drags, component edits, spawns, despawns, parenting
  and placed assets, and survive Play / Stop. **Save** writes back to the
  scene loaded or saved last.
- `attach::Attached3D`: keeps an entity at a transform in another
  one's space, placed by `systems::follow_attachments` with the 2D ones,
  saved in scenes and prefabs. C: `PonkoAttached3D` and
  `ponko_attached3d_component`; C++: `ponko::attached3d` and
  `ponko::attach3d_to`.
- Control endpoint `editor.*` methods and MCP tools for every editor action
  (`pick_entity`, `transform_entities`, `drag_gizmo`, `set_parent`,
  `scene_hierarchy`, `list_assets`, `place_asset`, `paint_material`,
  `delete_entities`, `duplicate_entities`, `undo_edit`, `redo_edit`,
  `save_edited_scene`, `set_camera_3d`), sharing the inspector's undo
  history, so an agent can assemble and save a level.

### Changed
- In the inspector's editor, Ctrl+Shift+Z redoes instead of undoing, and a
  Ctrl-click (or a Shift-click on an entity when no prefab is being placed)
  adds to the selection instead of replacing it.
