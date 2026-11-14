### Fixed
- Materials that read the scene behind them (`scene_position`, `scene_distance`),
  screen effects and target effects run on WebGL2 instead of stopping the game
  with a validation error. On OpenGL the scene depth is bound as a float
  texture read through a non-filtering sampler (WebGL2's shading language
  can't load a texel of a depth texture), and a game's own
  `textureLoad(scene_depth_texture, ..)` is rewritten to match. Transparent
  surfaces there read a copy of the depth, since OpenGL has no read-only depth
  attachments. Other backends draw as before.
- Texture arrays on WebGL2: one-layer arrays (such as a terrain's weight map)
  read as arrays, and every array's mipmaps are made right, so terrain
  materials show their layers instead of white.
- The `terrain`, `shaders` and `target_effect` examples run on WebGL2. The web
  gallery no longer marks them as needing WebGPU, and CI runs them on WebGL2.
