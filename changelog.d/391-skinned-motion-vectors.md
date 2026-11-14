### Added
- `Graphics::reset_motion(entity)` (C `ponko_ctx_reset_motion`, C++
  `ctx.reset_motion`) gives one model no motion on the next drawn frame and
  keeps the rest of the temporal history: for a teleport or an animation
  that snaps to another pose with no blend.
- The rendering baseline takes `PONKO_RENDER_TAA=1` to draw its scenes with
  TAA (and so with motion vectors), which no quality preset turns on.

### Changed
- Motion vectors (TAA, temporal AO) follow skinned and morphing models as
  they bend: the last frame's skin matrices and morph weights are kept per
  model while either is on, a model whose pose changed is posed a second
  time as it was (positions only, on the compute and the CPU skinning
  paths), and the motion pass reads where each corner was from there. A
  limb that swings while the character stands still writes its own
  motion. A held pose costs nothing more; a pose that changes every frame
  costs one more skinning dispatch per model (about 1 ms of GPU time for
  400 moving figures of 16 joints on an Apple M4 Max, see the guide).
