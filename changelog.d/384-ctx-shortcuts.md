### Added
- `Ctx::immediate_ui()` returns a `UiImmediateFrame` that draws immediate
  widgets with the context's graphics and input:
  `ctx.immediate_ui().button(rect, "Play")`. No widget call takes another
  service as an argument any more.
- `Ctx::start_scenario`, `Ctx::cancel_scenario` and
  `Ctx::quality_autotune_status` (3d): the calls that need several services
  now live on `Ctx`, the one place that holds them all.

### Deprecated
- The `Ctx` shortcuts that only forwarded to one service, each with a
  warning naming its one spelling: `ctx.dt()` (`ctx.time.dt()`),
  `ctx.key_down/key_pressed/key_released` and `ctx.mouse_pos()`
  (`ctx.input.*`), `ctx.draw` and `ctx.draw_text` (`ctx.gfx.*`),
  `ctx.load_texture` (`ctx.gfx.load_texture`), `ctx.load_sound` and
  `ctx.play_sound` (`ctx.audio.load` and `ctx.audio.play`). They go in 0.3.
  `Ctx` keeps only the calls that span services (`free`, `is_live`,
  `live_counts`, `mouse_world`, `window_size`, `quit`, the three above and
  `immediate_ui`). The C and C++ API already has one flat spelling per call
  and is unchanged.
- `UiImmediate::panel/label/button(gfx, input, ...)`: use
  `ctx.immediate_ui().panel/label/button(...)`.
- `Input::start_scenario(world, time, ..)` and
  `Input::cancel_scenario(world, time)`: use `ctx.start_scenario(..)` and
  `ctx.cancel_scenario()`.
- `SceneSettings::quality_autotune_status(gfx)`: use
  `ctx.quality_autotune_status()`.
