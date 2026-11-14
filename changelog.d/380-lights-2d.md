### Added
- 2D lights, without the `3d` feature: `d2::PointLight2D`, `d2::SpotLight2D`
  (a cone along the transform's rotation) and `d2::DirectionalLight2D`
  components, and an ambient light (`Graphics::set_ambient_light2d`). They
  light sprites, tile maps and particles; UI and text stay unlit. Lights add
  up and multiply the world, up to twice its color.
- 2D shadows: a light with `shadows: true` is blocked by `d2::Occluder2D`
  shapes (`rect`, `circle`, `polygon`, `line`), which turn, scale and mirror
  with their transform; `TileMap::occluder` outlines a map's wall tiles as one
  occluder with merged edges.
- Normal-mapped sprites and tiles: `Graphics::set_normal_map` gives every
  sprite and tile drawn from a texture a tangent-space normal map; normals
  turn and mirror with the sprite.
- Zero cost without lights: with no light and no ambient light nothing new
  is drawn or allocated. On an Apple M4 Max at 1920x1080 with 2,000 sprites,
  100 lights took 0.55 ms a frame and 100 shadowed lights with 200 occluders
  about 1 ms (`ponko_bench lights2d`). Works on WebGL2 (a `lights2d` browser
  smoke fixture).
- The guide's Drawing chapter has a "2D lights and shadows" section, and the
  `lights2d` example shows a lit tile dungeon.
- Lights and occluders are reflected as `"PointLight2D"`, `"SpotLight2D"`,
  `"DirectionalLight2D"` and `"Occluder2D"`.
- C and C++: `PonkoPointLight2D`, `PonkoSpotLight2D`,
  `PonkoDirectionalLight2D` with their `_default` and `_component`
  functions, `ponko_ctx_set_occluder2d_rect` (`_circle`, `_polygon`,
  `_line`), `ponko_ctx_set_tilemap_occluder`, `ponko_ctx_remove_occluder2d`,
  `ponko_ctx_occluder2d_edge_count`, `ponko_ctx_set_ambient_light2d`,
  `ponko_ctx_ambient_light2d`, `ponko_ctx_set_normal_map` and
  `ponko_ctx_normal_map`; C++ `point_light2d`, `spot_light2d`,
  `directional_light2d` and the matching `ctx` methods.
