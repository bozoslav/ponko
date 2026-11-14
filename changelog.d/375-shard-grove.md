### Added
- Shard Grove (`examples/shard_grove`, features `navmesh` and `physics3d`),
  the 3D reference game: a small third-person game whose glTF level is drawn,
  solid and baked into a navigation mesh; skinned glTF characters whose clips
  are states of animation state machines; a character controller; enemies
  that patrol and chase on the navigation mesh; positional sound; a sun with
  shadows and point lights; title, pause, settings and end menus, a HUD and
  a UI panel in the world; graphics quality presets and volume in a settings
  file; and saves at shrines, with damaged saves set aside.
  `tests/shard_grove.rs` plays it headlessly, and its autopilot plays the
  whole game (`shard_grove --smoke headless|window`). It packages with
  `ponko package --example shard_grove --features navmesh,physics3d`, builds
  for the web (`web/build.sh --features navmesh,physics3d shard_grove`), and
  the web gallery lists it. The guide's chapter "Shard Grove: a 3D game,
  start to finish" walks through it and lists the gaps it found.
