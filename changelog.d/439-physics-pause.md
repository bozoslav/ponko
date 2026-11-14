### Added
- `ctx.physics.set_paused(bool)` and `ctx.physics.is_paused()` hold only the
  2D physics step: bodies stay where they are, no collision events arrive,
  and every other simulation system and the tick counter keep running, so a
  pause screen's own sim systems can read the press that resumes. C:
  `ponko_ctx_set_physics_paused` and `ponko_ctx_is_physics_paused`; C++:
  `ctx.set_physics_paused(bool)` and `ctx.is_physics_paused()`. The guide's
  "Pausing only physics" section shows the pattern.

### Changed
- The `Game::run_headless` docs and the guide say plainly that headless
  runs without a control endpoint ignore `ctx.time.pause()`, and point to
  `ctx.physics.set_paused` for holding the world in a test.
- Ember Hop holds the built-in physics step with `ctx.physics.set_paused`
  instead of replacing it with its own system.
