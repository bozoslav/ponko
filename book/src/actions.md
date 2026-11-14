# Named input actions

Bind physical inputs to names once, then query those names in game systems.
An action is a button, a scalar axis, or a vector axis. Several sources can
control the same action; this makes keyboard and controller support share
the same game logic. Configuration and rebinding may allocate. Event handling,
frame resets and action queries allocate nothing.

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/action_map.rs:action_map}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:action_map}}
```

</div>
</div>

`feed_input` supplies events manually for headless tests and replay. Live
keyboard, mouse and gamepad input updates the same action state automatically.
Frame edges clear after each frame (after each tick in headless runs).

## Input scenarios

To replay input by simulation tick, build an `InputScenario` with
`.at(tick, event)` and install it with `Game::input_scenario` (C++:
`ponko::input_scenario`, `game::input_scenario`). Before each tick's simulation systems run, the scenario feeds that tick's
events through `feed_input`, the same path as live input, so actions see
ordinary pressed, held and released states. The same seed and scenario
therefore give the same headless run. Nothing is injected unless a game calls
`input_scenario` (or a tool is explicitly allowed to, see below).

<div class="lang-pair">
<div>

```rust
# struct Jumps { count: u32 }
let scenario = ponko::scenario::InputScenario::new()
    .named("jump-once")
    .at(10, ponko::actions::InputEvent::Key(Key::Space, true)) // pressed on tick 10, held after
    .expect(10, ponko::scenario::ScenarioCheck::action("jump").pressed(true).down(true))
    .at(12, ponko::actions::InputEvent::Key(Key::Space, false))
    .expect(12, ponko::scenario::ScenarioCheck::action("jump").released(true))
    .expect(12, ponko::scenario::ScenarioCheck::resource("Jumps", "/count", 1)); // a reflected resource, by JSON pointer
let (world, report) = Game::new("test", 1, 1)
    .sim_system(|ctx: &mut Ctx| {})
    .run_headless_scenario(scenario, |ctx| {});
report.assert_passed(); // or look at report.failure
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:input_scenario}}
```

</div>
</div>

- **Ticks.** The first tick of a run is 0. A step at tick `t` is fed before
  tick `t`'s systems run, and several steps on one tick keep the order they were
  added in, so a press and a release on the same tick keep both edges. A scenario
  started mid-run (`ctx.input.start_scenario`, or a tool) counts from the next
  tick. Pausing doesn't move a step, because a paused game doesn't advance the tick.
- **One tick per frame while a scenario runs.** Pressed and released edges last
  until the end of a frame, so in a frame with several ticks a press would be
  `pressed` in every one of them. Headless runs have one tick per frame. In a
  window the engine does the same while a scenario is running: a frame runs
  one tick at most, so the simulation sees the same input in a window as it
  does headless. Ticks a tool asked for wait for the next frames; the clock's
  own extra ticks are dropped, so a window that draws slower than the
  simulation ticks runs the scenario slower than real time (and catches up on
  nothing when it ends). (A game that drives
  `run_systems`-style frames of several ticks itself still gets frame-long
  edges, as with live input.)
- **Checks** run after the tick's systems: `ScenarioCheck::action` compares an
  action's `pressed`, `down` and `released` (an unbound name fails rather than
  passing), `resource` compares a reflected resource's JSON field, and `count`
  counts the entities with a reflected component. The first check that doesn't
  hold ends the run, and `ScenarioReport::failure` says which: its `step`, its
  `tick`, what was `expected` and what was `actual`, as data. In C++,
  `scenario_check::action`, `resource` and `count` build the same checks.
- **The report** is a `ScenarioReport` resource in the world (status `pending`,
  `running`, `passed`, `failed` or `cancelled`, ticks run, checks passed).
  `run_headless_scenario` runs exactly `scenario.ticks()` ticks, which is the
  bound on a run, and hands it back with the world. C++: `run.report` (a
  `ponko::scenario_report`: `status`, `passed()`, `ticks_run`,
  `checks_passed`, `failure_step`, `failure_tick`, ...) and
  `world.scenario_report_json()` for the whole report, failure included; C:
  `ponko_world_scenario_report` and `ponko_world_scenario_report_json`.
- **Releasing input.** `ctx.cancel_scenario()`, a scenario that replaces
  it, a failed check and a tool's disconnect release every key, button and axis
  the scenario still holds. A run in game code that simply finishes leaves its
  input as it was, unless the scenario says `.release_when_done()`; a run a tool
  loads releases it by default (see below). Only what the scenario itself left
  held is released, but the input state is one shared set of held keys with no
  record of which device holds one: a key a player also holds physically reads as
  up until its next key event.
- **Started from a system.** `ctx.input.start_scenario` called from a simulation
  system begins on the next tick; nothing of it (input or checks) happens on the
  tick that started it. C++: `ctx.start_scenario(s)`, `ctx.cancel_scenario()`
  (true if one was running).

### The scenario file

A scenario also has a small versioned JSON form, which tools send and tests can
keep next to their fixtures (`InputScenario::from_json`, `to_json`; C++
`input_scenario::from_json`, `to_json`; C `ponko_input_scenario_from_json`,
`ponko_input_scenario_to_json`):

```json
{
  "version": 1,
  "id": "jump-once",
  "duration": 120,
  "release_when_done": true,
  "steps": [
    { "tick": 10, "type": "key", "key": "Space", "down": true },
    { "tick": 10, "type": "expect_action", "action": "jump", "pressed": true },
    { "tick": 12, "type": "key", "key": "Space", "down": false },
    { "tick": 12, "type": "expect_resource", "resource": "Jumps", "pointer": "/count", "equals": 1 },
    { "tick": 12, "type": "expect_count", "component": "Coin", "equals": 2 }
  ]
}
```

Input steps are `key`, `mouse`, `gamepad_button`, `gamepad_axis`, `focus_lost`
and `gamepad_disconnected`; check steps are `expect_action`, `expect_resource`
and `expect_count`. Steps may come in any order, and a step's number in a
failure report is its position in the file. Unknown fields, a `version` this
engine doesn't read, more than 100,000 steps and steps past `duration` are
errors, so a typo can't quietly weaken a test. The format is for reproducing
gameplay on one build; it isn't a promise of identical replays across engine
versions.

With [headless frames](headless.md#menus-and-the-ui-headless-frames),
scenarios also click and type into the retained UI by node name and check
its tree: `ui_click`, `ui_type` and `expect_ui` steps (`InputScenario::click`,
`type_text`, `ScenarioCheck::ui`).

### From tools

With the [control endpoint](control.md) on, `input.scenario.load`,
`input.scenario.status` and `input.scenario.cancel` run a scenario in a live
game, and the MCP server's `run_input_scenario` steps a paused game through one
and returns the report (see [Working with AI agents](agents.md)). A game that
only opens the endpoint for inspection refuses to load scenarios: allow it with
`Game::input_injection(true)` or `PONKO_INPUT_INJECTION=1` (the MCP `launch`
tool sets it). A scenario loaded with `bind_connection` is cancelled, and its
input released, when the client that loaded it disconnects.

A tool's scenario releases the keys it holds when it ends, however it ends, so a
tool that dies mid-run leaves nothing pressed. With `keep_input_when_done` they
stay held (to look at the result, say), and stay owned by the scenario's owner:
`input.scenario.cancel`, or the owner's disconnect when it was bound, still
releases them, and the report's `still_held` says how many are left. With
`max_ticks` a too-long scenario is refused before the running one is touched, and
with `queue_ticks` a paused game gets the scenario's own ticks queued as steps; a
failed check or a cancel takes back only those that haven't run, never ticks
another client asked for with `sim.step`.

| Action | Rust sources | C++ sources |
| --- | --- | --- |
| `Action::Button` / `action::button` | `ButtonBinding::Key`, `Mouse`, `Gamepad` | `action_binding::key`, `mouse`, `button` |
| `Action::Axis` / `action::axis` | `AxisBinding::Keys`, `Gamepad` | `action_binding::keys`, `axis` |
| `Action::Axis2` / `action::axis2` | `Axis2Binding::Keys`, `Gamepad` | `action_binding::keys2`, `stick` |

For example, add `ButtonBinding::Mouse(MouseButton::Left)` or
`action_binding::mouse(PONKO_MOUSE_LEFT)` to a button action. A scalar key pair
takes the negative key followed by the positive key. Four-key vectors take
left, right, up, down, so WASD is A, D, W, S. Opposing keys cancel and keyboard
diagonals are normalized. Vectors use screen coordinates: positive Y is down.

Buttons combine with OR. `action_pressed` fires when the first source becomes
held; `action_released` fires when the last held source releases. Pressing a
second source or releasing one of two held sources produces neither edge.
A press and release within one frame preserves both edges.

Scalar and vector axes choose the source with the largest absolute value or
vector length; the first binding wins ties. Sources do not add together.
Sticks use a radial deadzone; scalar gamepad axes use a component deadzone.
Inside it the value is zero, and outside it the remaining range is rescaled
to reach one. A deadzone must be finite and in `0..1`; invalid values disable
that source. Non-finite analog input reads zero. Disconnection releases the
pad's buttons and centers its axes. An unconnected slot starts at zero.

Use `action_down`, `action_pressed`, `action_released`, `action_axis` and
`action_axis2` on `ctx.input`. Missing names and queries of the wrong kind
return false or zero. Names are case-sensitive. Queries borrow their names
and allocate nothing.

Resolve each name once and keep the `ActionId` (C++ `ponko::action_id`):
queries by id are an array index instead of a name lookup, and an unknown name
is an error when you resolve it, not a silent `false` every frame.
`ctx.input.action_id("jump")` resolves one, `ctx.input.action_ids(&["jump", "fire"])`
several, and the Rust `actions!` macro (as in the example above) declares the
names in one place. Ids stay valid when you rebind with `set_actions`. The
queries accept an id or a name, so both spellings work.

<div class="lang-pair">
<div>

```rust
struct Controls { jump: ActionId, fire: ActionId }

// Once, after set_actions: an unknown name is logged here, not every frame.
let [jump, fire] = ctx.input.action_ids(&["jump", "fire"]);
let controls = Controls { jump, fire };

let shoots = ctx.input.action_pressed(controls.fire) && !ctx.input.action_down(controls.jump);
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:action_ids}}
```

</div>
</div>

Rust logs an unknown name and returns an id that reads false and zero; C++
`action_id` and `action_ids` throw `ponko::error` (kind `not found`, the
message lists the known names). C: `ponko_ctx_action_id` and
`ponko_ctx_action_ids` give `PONKO_ACTION_NONE` for an unknown name and return
`PONKO_NOT_FOUND`; `ponko_ctx_action_down_id`, `_pressed_id`, `_released_id`,
`_axis_id` and `_axis2_id` query by id.

To rebind, copy `ctx.input.actions()`, call `set` or `remove`, then install it with
`ctx.input.set_actions`. Rust uses `.clone()` to copy the map; C++ returns an owned
map. Installation resets action edges and uses currently held inputs as the
baseline. Rebinding a held key therefore does not generate a new press.

A map reads back: `get("jump")` gives one action's bindings, `iter()` every
action in name order (the order the JSON file stores them in), and `len()`
counts them. An `Action`'s `buttons()`, `axes()` and `axes2()` are its
sources as slices (empty for the other kinds). A settings screen can show
what `ctx.input.actions()` binds instead of keeping its own copy. To change
one action without rebuilding the map, `ctx.input.set_action("jump", action)`
replaces it in place. The name keeps its `ActionId`, the other actions keep
their state and edges, and held sources become the baseline as above:

<div class="lang-pair">
<div>

```rust
# use ponko::prelude::*;
# use ponko::actions::{Action, ButtonBinding};
# fn example(ctx: &mut Ctx) {
let mut jump = ctx.input.actions().get("jump").cloned().unwrap_or(Action::Button(vec![]));
if let Action::Button(sources) = &mut jump {
    sources.retain(|s| !matches!(s, ButtonBinding::Key(_)));
    sources.insert(0, ButtonBinding::Key(Key::K)); // the player's new key
}
ctx.input.set_action("jump", jump);
# }
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:set_action}}
```

</div>
</div>

In C, `ponko_action_map_count`, `ponko_action_map_name` and
`ponko_action_map_get` read a map back (an unknown name or index is
`PONKO_NOT_FOUND`), and `ponko_ctx_set_action` changes one installed action.
C++ has `action_map::size`, `names` and `get` (a `std::optional<action>`),
and `ctx::set_action`.

`ActionMap::to_json` / `action_map::to_json` save bindings only, with the same
serde JSON format in both languages. Write that string to your settings file
using normal file I/O. Read it back with `ActionMap::from_json` /
`action_map::from_json`, then install the result. Invalid JSON fails before
changing active bindings. Rust returns `Result`; C++ throws the usual engine
exception. Button state is never saved.

The C API uses an owned `PonkoActionMap`: create/free, set/remove,
load/save JSON, then `ponko_ctx_set_actions`. `ponko_action_map_set` takes
a kind (0 button, 1 scalar, 2 vector) and an array of `PonkoActionBinding`.
`PonkoActionSource` documents each source's fields; unused fields are zero.
Save follows the standard two-call buffer convention: query the length,
allocate length + 1 bytes, then save again. Context queries are named
`ponko_ctx_action_down`, `pressed`, `released`, `axis` and `axis2`.

This map has no chords, action contexts, smoothing, interactive binding UI,
or device-specific profiles. Each gamepad binding names a stable controller
slot, the same slot used by the raw gamepad API.

## Rebinding screens: what was just pressed

A rebinding screen asks "which input did the player just press?". The input
service answers it: `ctx.input.pressed_this_frame()` yields every key, mouse
button, gamepad button and gamepad axis that went down this frame, as a
`PressedInput` that wraps the binding an `ActionMap` takes. The screen lists no
keys itself, so a key the game never thought of works too. Nothing is polled.

<div class="lang-pair">
<div>

```rust
# use ponko::prelude::*;
# fn example(ctx: &mut Ctx, waiting_for_jump: &mut bool) {
if *waiting_for_jump {
    // The first key, mouse button or gamepad button that went down this frame.
    let pressed = ctx.input.pressed_this_frame().find_map(|p| p.button().cloned());
    if let Some(binding) = pressed {
        ctx.input.set_action("jump", Action::Button(vec![binding]));
        *waiting_for_jump = false;
    }
}
# }
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:pressed_this_frame}}
```

</div>
</div>

- **Edges.** An input appears on the frame it goes down, not while it is held,
  and a tap that goes down and up inside one frame still appears (like
  `key_pressed`). A stick or trigger counts when it was under the threshold
  at the start of the frame and reached it during the frame; flicking it out
  and back within one frame still counts. A simulation system that runs
  several ticks in a frame sees the same answer in each.
- **What comes back.** `PressedInput::Button` holds a `ButtonBinding` (key,
  mouse button or gamepad slot and button) for an `Action::Button`;
  `PressedInput::Axis` holds an `AxisBinding::Gamepad` for an `Action::Axis`,
  with a deadzone of `DEFAULT_DEADZONE` (0.15). `p.button()` and `p.axis()`
  pick one kind. To bind a stick to a vector action, wrap the axis's stick in
  `Axis2Binding::Gamepad` yourself.
- **Axis threshold.** `PRESS_THRESHOLD` is 0.5; `pressed_this_frame_with_threshold(t)`
  takes another.
- **Order.** Keys, then mouse buttons, then each gamepad slot's buttons, then
  its axes, each in the order of its enum: the same every run, so "the first
  press" is a stable answer, and a scenario can assert the whole list.
- **No allocation.** It scans fixed arrays. In Rust, collect the first
  answer before you call `set_action` (the iterator borrows `ctx.input`), as
  the example does.
- **Held when the prompt opens.** The press that opened the prompt (Enter on
  a focused button) is still "pressed this frame" in the frame the prompt
  opens: skip that frame, as the reference game's prompt does.

Native code: C's `ponko_ctx_pressed_this_frame(ctx, axis_threshold, out,
capacity, &count)` fills `PonkoActionBinding` descriptions (kinds
`PONKO_ACTION_KEY`, `_MOUSE`, `_GAMEPAD_BUTTON` and `_GAMEPAD_AXIS`) that go
straight into `ponko_action_map_set`; `count` is the full number, so a call
with a null buffer and capacity 0 sizes it. A threshold of 0 means
`PONKO_PRESS_THRESHOLD`. C++ returns `std::vector<ponko::pressed_input>`,
each with `key()`, `mouse()`, `button()`, `axis()`, `pad()` and `binding()`.

The reference game's settings screen is built on it (see
[Packaging a game](packaging.md)), and `tests/pressed_this_frame.rs` drives the
query with an input scenario, tick by tick.
