# The inspector: look inside a running game

Press the backquote key (`` ` ``, left of 1) in a debug build and a panel
opens on the right of the window. It shows the game as it runs:

- **Sim controls**: play, pause, step one tick, and the simulation speed
  (drag the `1.00×` box). The tick, frame rate and entity count are below,
  and a graph of the last 240 frame times (the line marks 60 fps; red bars
  are slow frames).
- **The selected entity**: each component it has. Reflected components are
  editable in place: drag numbers, tick booleans, type text (applied when
  you press Enter or click away). **Remove** takes a component off,
  **Duplicate** copies the entity, **Despawn** removes it, **Look at**
  moves the camera to it. A
  yellow ring marks it in the world.
- **Entities**: every entity, with its components. Pick a component under
  **With** to list only entities that have it. Click a row to select it, or
  press **Pick** and then click a unit in the world.
- **Resources**: reflected resources, editable the same way.
- **Systems**: each system's average time per frame, slowest bars longest.
  Sim systems are marked `(sim)`.

Only [reflected](reflection.md) components and resources can be read and
edited; others show up by name, marked *not reflected*. The same reflection
is what agents and save files use, so one `reflect` call covers all three.

## Editing: build levels in the game

To try it: `cargo run --example editor`, press `` ` ``, then **Pause**.

Pause, and the inspector is an editor. Clicks and the editor's keys in the
world go to it, not to the game:

| Do | To |
|---|---|
| Click | Select the sprite under the mouse (the top one), or the nearest entity |
| Ctrl+click (⌘-click) | Add it to the selection, or take it out |
| Drag | Move the selected entity |
| Right or middle drag, wheel | Pan and zoom the camera |
| Delete (or Backspace) | Despawn the selection, with what hangs off it |
| Ctrl+D (⌘D) | Duplicate it, a little down and to the right |
| Ctrl+Z (⌘Z) | Undo: moves, edits, added and removed components, spawns, despawns, parenting |
| Ctrl+Shift+Z or Ctrl+Y | Redo what was undone (a new change drops it) |
| Esc | Stop placing a prefab |

The **Edit** section has **Undo**, **New entity** (a bare transform in the
middle of the screen), **Redo**, the **View** (2D or 3D, see
[below](#editing-in-3d)), and two rows for files in `assets/`:

- **Scene**: save the world to a [scene](scenes.md) file, or load one
  (loading clears undo). Left empty, **Save** writes the scene loaded or
  saved last, so loading a level, changing it and pressing **Save** puts
  it back where it came from.
- **Prefab**: type a name and **Save** the selected entity as a prefab
  (with its [children](scenes.md#prefabs-with-children), if it has any).
  Prefabs in `assets/prefabs/` are listed below: click one, then click in
  the world to place it (hold Shift to place several).

Below a selected entity's components, **Add component** lists the
reflected components it doesn't have. A component is added with its
default fields, so types whose fields all have `#[serde(default)]` add
cleanly; others say which field they need.

**Play** runs the game and remembers the world; **Stop** pauses and puts
the world back as it was at Play, so you can try a level and return to it.
**Keep** forgets the remembered world. Only reflected components are put
back: anything that isn't reflected isn't in the snapshot either.

Undoing a despawn brings the entity back as a new entity: handles never
come back to life. Undo and redo keep finding it under its new id, and the
entities despawned with it (its attachments and prefab parts) point at the
new one, but other components that pointed at it still point at nothing.

### The hierarchy and the asset browser

**Hierarchy** lists every entity under what it hangs off: the entity it is
[attached](attachments.md) to, the prefab it is a part of (▣ marks a
prefab instance), or the skeleton it is on (a joint or a skin). Click a row
to select it (Shift or Ctrl adds to the selection). Drag a row onto another
to make it follow that one, or onto the *drop here to detach* line to make
it follow nothing; it stays where it is either way, and loops are refused.
In 3D the link is an `Attached3D`, in 2D an `Attached`.

**Assets** lists what is in `assets/`: glTF meshes (`.glb`, `.gltf`),
prefabs, scenes, and the materials of the mesh files loaded so far. Drag a
mesh or a prefab into the world to place it where you let go (on the
ground, y = 0, in 3D); drag a material onto a model to paint it with it
(a `ModelMaterial`). A scene's button opens it. **⟳ list again** reads the
folder again.

### Editing in 3D

When the world has 3D transforms (or **View** is set to **3D**), the
editor works in the 3D view:

| Do | To |
|---|---|
| Click | Select what's under the mouse: the nearest mesh whose bounds the mouse ray enters (entities without a mesh count as a small ball) |
| Shift+click or Ctrl+click | Add it to the selection, or take it out |
| Drag a gizmo handle | Move, turn or scale the selection, as one undo step |
| W, E, R | The move, rotate and scale gizmo |
| Right drag, with W A S D Q E | Look around and fly (Shift: faster) |
| Middle drag, wheel | Pan, and move forward or back |
| F | Frame the selection |
| Ctrl+D (⌘D) | Duplicate the selection one unit over |

The gizmo sits in the middle of the selection. The move gizmo has an arrow
per axis, a square per plane and a dot in the middle (moves in the view's
plane); the rotate gizmo a ring per axis; the scale gizmo a box per axis
and a dot (scales evenly). **World** and **Local** pick the world's axes or
the selected entity's own (scaling always uses the entity's own). With
**Snap** on, moves land on a grid (**move**, in world units), turns go in
steps (degrees) and scale factors in steps (**scale**).

Moving a parent carries what is attached to it; moving an attached entity
changes its offset from the parent. When a parent and its child are both
selected, only the parent is moved, and the child follows it.

Selected entities get a yellow ring; the gizmo is drawn over the world.
Drag in an asset, place a few, parent them in the hierarchy, and **Save**:
the level loads back the same, attachments and all.

Agents can do the same edits through the
[control endpoint](control.md#the-editor-from-an-agent): every action in
the editor is a method there and an [MCP tool](agents.md#what-an-agent-can-do),
and both share one undo history with the inspector.

## Turning it on and off

The inspector is on by default in debug builds and off in release builds.
To ship it in a release build for playtesters:

<div class="lang-pair">
<div>

```rust
{{#include ../../tests/reflect.rs:inspector}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:inspector}}
```

</div>
</div>

The environment variable `PONKO_INSPECTOR` overrides the game's choice:
`1` turns it on, `0` turns it off, and `open` turns it on and opens it at
start.

While the inspector is closed it costs nothing: egui doesn't run and no
input goes to it. While it's open, clicks, keys and the mouse wheel over
the panel stay in the panel, and `ctx.ui.wants_pointer()` is `true` there,
so a game that checks it doesn't also act on clicks in the panel. The
panel isn't in screenshots taken with `ctx.gfx.save_screenshot` or by agents.

The overlay is built with [egui](https://github.com/emilk/egui), behind
the `inspector` cargo feature (on by default). Games that don't want egui
in their build turn default features off:

```toml
ponko = { version = "0.1", default-features = false, features = ["window", "audio"] }
```

C and C++ games get the inspector from `ponko_ffi`'s `inspector` feature,
also on by default.
