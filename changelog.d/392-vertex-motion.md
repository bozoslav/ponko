### Added
- `Graphics::set_gpu_buffer_motion(buffer, keep)` (C `ponko_ctx_set_gpu_buffer_motion`,
  C++ `ctx.set_gpu_buffer_motion`): a GPU buffer keeps a copy of its last frame,
  so temporal antialiasing and temporal AO follow each element's own motion
  (particles moved by a compute pass). It doubles the buffer's memory and copies
  it once a frame on the GPU while motion vectors are drawn.
- Material shaders: `frame.time.zw` is the last drawn frame's time, and
  `drawing_last_frame()` is true while the motion pass runs `vertex` as of the
  last frame, for shaders that animate with their own values.
- Rendering baseline: `PONKO_RENDER_WIND=1` sways the `game-like` scene's trees in
  a `vertex` function and adds a kept GPU buffer; `PONKO_RENDER_VERTEX_MOTION=0`
  is its control.

### Changed
- Motion vectors (temporal antialiasing, temporal AO) follow what a material's
  `vertex` function does: the motion pass runs it as of this frame and as of the
  last, with the last frame's `frame.time` and, for a buffer that keeps its last
  frame, its `items`. Swaying foliage, waves and buffer particles no longer
  smear under TAA. Models whose shader's `vertex` reads `frame.time` or `items`
  are drawn into the motion image every frame (measured: 0.15 ms more GPU time
  for 30,000 swaying trees on an Apple M4 Max); other shaders, and frames with
  TAA and temporal AO off, cost nothing more.
