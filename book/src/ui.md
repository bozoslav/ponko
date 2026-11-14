# Retained game UI

Build a tree once in `ctx.ui`, then update its text, values and styles.
The engine handles layout/input before frame systems and draws the tree above
scene sprites. The retained tree is the game's UI: menus, HUDs, shops, dialogs.
The immediate widgets, `ctx.ui.immediate.panel`, `label` and `button`
([Camera and UI](camera-ui.md#ui)), are for debug overlays and quick prototypes.

In C++ the tree is `ctx.ui_retained()`, a `ponko::ui::tree`; layouts start from
a preset (`ui::fill()`, `ui::row(40)`, `ui::fixed(220, 40)`, `ui::centered(360, 180)`)
or `ui::layout()` and are refined by chaining (`.padding(24).gap(12)`), and
paints are chained too (`ui::paint().wrap()`). In C it is the `ponko_ctx_ui_*`
functions with a flat `PonkoUiLayout` (from `ponko_ui_layout_row`, `_column`,
`_fill`, `_fixed`, `_anchored`, `_centered` or `_default`) and `PonkoUiPaint`
(see [C and C++](c-cpp.md) and below).

The tree is owned by `Ui`, independently of ECS entities. Stable `UiId` handles
identify widgets and events. Removing a subtree invalidates its IDs permanently.
Taffy supplies flexbox, grid, percentage sizes, min/max constraints, padding,
margin, gap and absolute positioning behind `UiLayout`, the one layout type. It owns
neither the renderer nor the event loop. Ponko owns interaction and appearance.

## A shop and inventory grid

The runnable Rust example is `cargo run --example ui_shop`. Build it without
opening a window with `cargo build --example ui_shop`.

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/ui_shop.rs:shop_tree}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:shop_tree}}
```

</div>
</div>

In Rust, `ui.label(parent, text)`, `ui.title(...)`, `ui.button(...)`, `ui.panel(...)`,
`ui.scroll_area(...)`, `ui.text_input(...)`, `ui.toggle(...)`, `ui.slider(...)`
and `ui.image(...)` add a node and return a builder; chain its layout, text and
paint, and end with `.id()` when you need the id again. `ui.root()` makes a
detached panel to push as a screen. The builders are shorthand for `add_child`
plus the setters, which keep working.

`UiLayout` names the common shapes, so no screen needs its own layout helpers:
`row(height)` and `column(width)` strips, `fill()`, `fixed(w, h)`,
`anchored(x, y, w, h)` in fractions of the parent (of the window for a screen
root), `centered(w, h)` for dialogs and `grid(columns, row_height)` for
inventories, refined with `gap`, `padding`, `grow`, `horizontal`/`vertical` and
`width`/`height`, and placed in points with `top(y)` and `left(x)` (a
centered dialog near the top: `UiLayout::centered(360.0, 180.0).top(180.0)`).
`hidden()` takes a node and its subtree out of layout, drawing, input and the
accessibility tree; set the layout again without it to show the node.
Children stack top to bottom, except in a row. For a Taffy setting the
presets don't cover (named grid lines, templates), `UiLayout::custom(style)`
wraps a `ui::unstable_taffy::Style` and `style_mut()` edits one. That module
is the Taffy crate itself: it is outside ponko's stability promise and changes
when ponko upgrades Taffy.
Give a scroll view a bounded size; `fill()` shrinks to the space its siblings
leave. `UiWidget::Scroll` clips descendants and supports wheel scrolling,
content dragging and a vertical scrollbar. `set_scroll` uses unscaled layout
units and clamps offsets during layout. Nested scroll views intersect clips.
Navigation scrolls the focused widget into view.

Colors and text defaults come from the tree's theme:
`ui.set_theme(UiTheme { panel, control, accent, text, title, text_size, title_size, .. })`.

<div class="lang-pair">
<div>

```rust
let mut tree = ponko::ui::Ui::default();
let mut theme = tree.theme().clone();
theme.accent = Color::rgb(1.0, 0.6, 0.1);
theme.title_size = 32.0;
tree.set_theme(theme); // nodes keep the paint set on them
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:ui_theme}}
```

</div>
</div>

Every new node takes its `UiPaint` from the theme for its widget, and a title
(`ui.title(parent, text)`) takes the theme's title color and size; paint set on
a node (`.color(..)`, `.size(..)`, `set_paint`) overrides it, and a later
`set_theme` changes only the fields a node didn't set. The screen's theme also
styles the immediate widgets: `set_theme` updates `ctx.ui.immediate.style`
the same way (`UiTheme::style()` is the style it gives them), keeping fields
set there by hand, and so do their font and button border (`UiTheme::font`,
`UiTheme::border_width`). C++: `tree.theme()`, `tree.set_theme(t)` with a `ui::theme`;
C: `ponko_ctx_ui_theme`, `ponko_ctx_ui_set_theme` with a `PonkoUiTheme`.
`ui.paint(id)` (C++: `resolved_paint(id)`, C: `ponko_ctx_ui_paint`) reads what
a node is drawn with.

### Finding nodes by name

A system that didn't build a screen finds its nodes by name, so no resource
of ids is needed: `.name("shop.buy")` on a builder (or `ui.set_name(id, ..)`)
names a node, and `ui.find("shop.buy")` returns its id; `ui.name(id)` reads it
back. A name belongs to one node of the tree: naming another node the same
moves it, an empty name takes it off, and removing the node frees it. Dotted
names are only a convention. For a screen that reacts only to clicks, take
the last click, `ui.events().filter_map(UiEvent::clicked).last()` (C and C++
have `take_click`, which does that). C++: `set_name`, `find`, `name`, `take_click`
on the tree; C:
`ponko_ctx_ui_set_name`, `ponko_ctx_ui_find`, `ponko_ctx_ui_name`,
`ponko_ctx_ui_take_click`.

`UiPaint` supplies background colors for normal/hover/pressed/disabled states,
borders, font, line height, text color, wrapping and horizontal alignment.
Set `texture` for an image or textured panel, and `slice` to the source border
size in texture pixels for nine-slice backgrounds. `UiWidget::Toggle` uses
zero/one values; `Slider` uses normalized 0..1 values. Read them with `value`
after a `Changed` event. Changes made with setters do not emit user events.

## Named grid areas and repeat tracks

Template areas name rectangular regions. A row is a space-separated list of names;
`.` (or multiple dots) leaves a cell unnamed. All rows must have equal widths and
each name must fill a rectangle. C/C++ rejects invalid templates with a status
message. Names are UTF-8 identifiers using letters, digits, `_` and `-`, without
CSS escapes; reserved CSS keywords and names starting with digits are rejected.

<div class="lang-pair">
<div>

```rust
use ponko::ui::{Ui, UiWidget, unstable_taffy::*};
use ponko::ui::unstable_taffy::style_helpers::*;
let mut tree = ponko::ui::Ui::default();
let dashboard = tree.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::custom(Style {
    display: Display::Grid,
    size: Size { width: length(600.0), height: length(400.0) },
    grid_template_columns: vec![repeat(2, vec![fr(1.0)])],
    grid_template_rows: vec![length(40.0), fr(1.0)],
    grid_template_areas: Some(GridTemplateAreas {
        row_count: 2, column_count: 2,
        areas: vec![
            GridTemplateArea { name: "header".into(), row_start: 1, row_end: 2, column_start: 1, column_end: 3 },
            GridTemplateArea { name: "side".into(), row_start: 2, row_end: 3, column_start: 1, column_end: 2 },
            GridTemplateArea { name: "main".into(), row_start: 2, row_end: 3, column_start: 2, column_end: 3 },
        ],
    }),
    ..Style::default()
}));
let placement = GridPlacement::NamedLine("main".into(), 0);
let main = tree.add_child(dashboard, ponko::ui::UiWidget::Label, ponko::ui::UiLayout::custom(Style {
    grid_column: Line { start: placement.clone(), end: placement.clone() },
    grid_row: Line { start: placement.clone(), end: placement },
    ..Style::default()
}));
tree.set_text(main, "Content");
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:ui_grid_areas}}
```

</div>
</div>

C keeps `PonkoUiTrack` and the original flat track arrays unchanged. For repeats,
use `PonkoUiTemplateTrack` arrays in `grid_template_columns`/`grid_template_rows`;
a nonzero component count overrides the corresponding original track array.
`repetition` is 0 for a single `track`, 1 for `count` repetitions of `tracks`,
2 for auto-fill, or 3 for auto-fit. Repeat fragments cannot contain repeats.
C++ accepts `ui::repeat(3, ui::fr(1))`, multi-track fragments such as
`ui::repeat(2, {ui::track_points(40), ui::fr(1)})`, and
`ui::auto_fill(ui::track_points(100))` / `ui::auto_fit(ui::track_points(100))`.
Automatic repeats require fixed point or percentage tracks throughout the axis
and at most one automatic repeat per axis, following taffy's supported subset.

`column_names({"left", "middle split", "right"})` and `row_names(...)` name
line boundaries. With repeats these are component boundaries, rather than every
expanded track; use `ui::repeat(3, ui::fr(1)).names({"start", "end"})` for names inside
a repeat fragment. Supply exactly one more name set than repeated tracks.
`tree.set_grid_named_line(node, 0, false, "middle", 1)` places a column start at
the first matching line (axis 1 selects rows, `true` selects the end). Negative
occurrences count backwards; zero also resolves implicit area start/end names.
Unknown names follow taffy's implicit-line placement rules. Nested repeats are
not supported by taffy.

A named span (CSS `span 2 edge`) reaches the `count`-th line called `name`
from the other edge. It only means something when that other edge is a line
number; against an automatic edge taffy treats it as a span of one track.

<div class="lang-pair">
<div>

```rust
use ponko::ui::unstable_taffy::*;
use ponko::ui::unstable_taffy::style_helpers::*;
let mut tree = ponko::ui::Ui::default();
// Four 100-point columns; lines 3 and 5 are both called "edge".
let strip = tree.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::custom(Style {
    display: Display::Grid,
    size: Size { width: length(400.0), height: length(40.0) },
    grid_template_columns: vec![length(100.0), length(100.0), length(100.0), length(100.0)],
    grid_template_column_names: vec![vec![], vec![], vec!["edge".into()], vec![], vec!["edge".into()]],
    ..Style::default()
}));
// From line 1 to the second "edge" line: all four columns.
let banner = tree.add_child(strip, ponko::ui::UiWidget::Label, ponko::ui::UiLayout::custom(Style {
    grid_column: Line { start: line(1), end: GridPlacement::NamedSpan("edge".into(), 2) },
    ..Style::default()
}));
tree.set_text(banner, "Sale");
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:ui_named_span}}
```

</div>
</div>

C: `ponko_ctx_ui_set_grid_named_span(ctx, target, node, axis, end, name, count)`,
with `count` from 1 to 65535.

C can use the layout descriptor fields or change existing nodes with
`ponko_ctx_ui_set_grid_template_areas`, `ponko_ctx_ui_set_grid_line_names`,
`ponko_ctx_ui_set_grid_area`, `ponko_ctx_ui_set_grid_named_line` and
`ponko_ctx_ui_set_grid_named_span`.
Each takes the same screen or world-panel `target` as other UI calls. Inputs
are copied during the call. Passing zero area/name rows clears those definitions;
an empty area name restores automatic placement. `set_layout` replaces the whole
style, including areas and names. The C layout struct grew, so rebuild native
callers against the regenerated header.

## Screen stack and modal dialogs

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/ui_shop.rs:shop_dialog}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:shop_dialog}}
```

</div>
</div>

Push a detached root with `push_screen(root, modal)`. All screens render in stack
order; only the top receives input, even when it is not modal. A modal screen
dims earlier screens and owns pointer input outside its rectangle too. Pop with
`pop_screen()`; nodes remain available for reuse. Escape/gamepad East pops the
top screen and emits `ScreenPopped`, consuming the rest of that input snapshot.
The previous screen's focus is restored when possible. Even the last screen can
be popped; game code decides whether to reopen it.

### Where the focus starts

A screen opens with nothing focused, so a keyboard or gamepad player would
press Tab or an arrow key before Enter does anything. Mark the node it
should open on with `.focused()` (or `ui.set_initial_focus(id, true)`): every
`push_screen` of that screen focuses it, so the menu is ready for Enter at
once, and opens on it again the next time it is pushed. `ui.set_focus(id)`
moves the focus from code at any time (it returns `false`, and leaves the
focus alone, for a node that can't take it: not a button, text input, toggle
or slider, disabled, hidden or not on the top screen), and `ui.clear_focus()`
takes it away.

```rust
# use ponko::prelude::*;
# use ponko::ui::UiLayout;
# fn build(ctx: &mut Ctx) {
let menu = ctx.ui.root().layout(UiLayout::centered(300.0, 200.0).gap(12.0)).id();
ctx.ui.button(menu, "Play").layout(UiLayout::row(44.0)).focused();
let quit = ctx.ui.button(menu, "Quit").layout(UiLayout::row(44.0)).id();
ctx.ui.push_screen(menu, false); // Play has the focus
ctx.ui.set_focus(quit); // now Quit has it
# }
```

C++: `tree.set_initial_focus(id)`, `tree.set_focus(id)` and
`tree.clear_focus()`; C: `ponko_ctx_ui_set_initial_focus`,
`ponko_ctx_ui_set_focus` (`PONKO_INVALID_ARGUMENT` for a node that can't take
the focus) and `ponko_ctx_ui_clear_focus`.

### HUDs

A HUD (a score, a health bar, a counter in a corner) is UI that is always
shown and never takes input. Build it as a detached root and show it with
`ui.show_hud(root)` instead of pushing it: it is drawn below every screen,
Escape never closes it, its nodes take no clicks and no focus (clicks go
through to the world), and it doesn't make `wants_pointer()` or
`wants_keyboard()` true, so the game keeps its keys. Screens open and close
above it as usual. `ui.hide_hud(root)` takes it down (its nodes stay) and
`ui.is_hud(root)` tells whether it is up; assistive technology reads it,
inert, under the screens. Anything the player clicks belongs on a screen.

```rust
# use ponko::prelude::*;
# use ponko::ui::UiLayout;
# fn build(ctx: &mut Ctx) {
let corner = ctx.ui.root().layout(UiLayout::fill().padding(16.0)).id();
ctx.ui.label(corner, "Coins: 0").name("hud.coins");
ctx.ui.show_hud(corner);
# }
```

C++: `tree.show_hud(root)`, `tree.hide_hud(root)` and `tree.is_hud(root)`;
C: `ponko_ctx_ui_show_hud`, `ponko_ctx_ui_hide_hud` and `ponko_ctx_ui_is_hud`.

## Events, navigation and ownership

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/ui_shop.rs:shop_events}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:shop_events}}
```

</div>
</div>

`events()` takes `Clicked`, `Changed`, `Submitted` and `ScreenPopped` events,
oldest first, and events left untaken (stopping early) stay queued for the
frame; `ui.next_event()` takes one at a time, for a loop that changes the tree
(C++: `tree.events()` or `tree.next_event()`; C: `ponko_ctx_ui_next_event`, one
tagged `PonkoUiEvent` at a time, `PONKO_NOT_FOUND` when none are left).
Unread events expire at the next UI frame. `UiEvent` and `UiWidget` are
`#[non_exhaustive]`: a `match` on them needs a `_` arm. Event IDs avoid callbacks borrowing
the context and game state simultaneously.

Tab/Shift-Tab traverses enabled controls. Arrows/d-pad use spatial navigation;
Enter, Space or gamepad South activates the focused control. Text inputs reserve
left/right, Home/End, Backspace/Delete, Shift-selection and Ctrl/Cmd-A for editing;
Enter submits. Clicking/dragging selects text. Text is UTF-8 with scalar-boundary
editing; grapheme-cluster editing, clipboard and IME composition are not provided.
Native committed Unicode is available to both UI and games as `ctx.input.typed_text()`.
Custom hosts/tests can supply it through `ctx.input.insert_text()`.

`ctx.ui.wants_pointer()` includes immediate and retained UI, including captured
drags. `ctx.ui.wants_keyboard()` reports retained keyboard/gamepad ownership
while any screen is open (a HUD doesn't count). These queries do not erase raw game input: game code
uses them to suppress world actions. UI screen changes cancel pointer capture,
so releasing a dialog button cannot activate an underlying screen.

## Scale and headless tests

Layout and pointer coordinates use logical window units. DPI already belongs
to the window/text renderer; `set_scale(1.25)` adds a user preference on top,
scaling geometry, text and hit testing together; the
[immediate widgets](camera-ui.md#one-look-for-both-styles) are drawn at that
scale too. Percentage dimensions resolve
against the available logical viewport, so roots can adapt to resolution.

`Ui::frame(viewport, &UiInput)` runs without a GPU and allows deterministic tests
of layout, events and editing. `Ui::layout` resolves geometry without processing
input. Standalone `Ui` measurements use the built-in font; context-owned UI also
measures loaded custom fonts. The engine keeps existing sprite batching and adds
per-batch scissor rectangles; world sprites and immediate UI remain unclipped.

## Accessibility semantics

`ui.accessibility_tree()` returns a deterministic snapshot of what assistive
technology reads: each shown node's role (group, dialog, label, button,
image, scroll view, text input, checkbox, slider), name, description, value
(text, checked state or 0..1 range), bounds and states (focusable, focused,
disabled, inert, modal), plus the focused node. Names come from the node's
text; give sliders, text inputs, images and icon-only buttons one with
`.accessible_name("Volume")` or `ui.set_accessible_name(id, "Volume")`. Text
inputs expose their text as the value, never as the name, and their caret and
selection as byte offsets. Names never change layout, focus or events.

Two more things a screen reader uses:

- A **description** (`.accessible_description("Starts a new run")`) is read
  after the name: what a control does, or a hint.
- A **live region** (`.live_region(UiLive::Polite)`) is announced when it
  changes while focus is elsewhere: a status line, a score, an error.
  `Assertive` interrupts what is being read; keep it for what can't wait.

Nodes with `display: none` are left out with their subtrees. A modal screen
hides every screen below it; lower screens under a non-modal one stay readable
but inert. The snapshot prints one indented line per node, which suits golden
tests:

```rust
let mut ui = ponko::ui::Ui::default();
let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
ui.button(root, "Play");
ui.slider(root).layout(ponko::ui::UiLayout::row(20.0)).value(0.25).accessible_name("Volume");
ui.push_screen(root, false);
ui.layout(vec2(800.0, 600.0));
assert_eq!(
    ui.accessibility_tree().to_string(),
    "group\n  button \"Play\" focusable\n  slider \"Volume\" value=0.25 focusable\n"
);
```

### Requests and changes

Assistive technology also operates the UI. `ui.push_access_action(..)` queues
a request (`Focus`, `Activate`, `Increment`, `Decrement`, `SetValue`,
`SetText`); the next `Ui::frame` applies it by doing what the matching key
does, so the game gets the same `Clicked`, `Changed` and `FocusChanged` events
and needs no second code path. A request for a node that can't take focus
right now (disabled, hidden, removed, under a modal screen) is dropped.
`Increment` and `Decrement` move a slider by 0.05, as the arrow keys do.

`UiAccessTracker::update(&ui)` reports what changed since the last call (the
changed and removed nodes, and whether the screens or the focus changed), so
a backend resends only those. While nothing changed it allocates nothing.

<div class="lang-pair">
<div>

```rust
# use ponko::ui::{Ui, UiAccessAction, UiAccessTracker, UiLayout, UiLive};
# let mut ui = Ui::default();
# let root = ui.root().layout(UiLayout::fill()).id();
# let buy = ui.button(root, "Buy").layout(UiLayout::fixed(220.0, 40.0)).id();
// A slider has no text of its own to be named by.
let volume = ui
    .slider(root)
    .layout(UiLayout::fixed(240.0, 24.0))
    .value(0.5)
    .accessible_name("Volume")
    .id();
ui.set_accessible_description(buy, "Buys the highlighted item");
// A change to a live region is announced while focus is elsewhere.
ui.label(root, "Ready")
    .layout(UiLayout::fixed(400.0, 24.0))
    .live_region(UiLive::Polite);
// A request from assistive technology; the next frame applies it.
ui.push_access_action(UiAccessAction::Increment(volume));
// What changed since the last look, for a backend of your own.
let mut tracker = UiAccessTracker::new();
if let Some(changes) = tracker.update(&ui) {
    for id in &changes.changed {
        let _name = tracker.tree().get(*id).map(|node| node.name.as_str());
    }
}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:ui_access}}
```

</div>
</div>

In C the same calls are `ponko_ctx_ui_set_accessible_name`,
`ponko_ctx_ui_set_accessible_description`, `ponko_ctx_ui_set_live_region`,
`ponko_ctx_ui_access_action`, `ponko_ctx_ui_access_text` (the snapshot as
text) and the `ponko_ui_access_*` functions for the tracker.

### The screen reader adapter

With the cargo feature `accessibility` (off by default) the engine gives
`ctx.ui` to the operating system's accessibility API and brings the screen
reader's requests back as the requests above. Nothing is called from game
code:

```toml
ponko = { version = "0.1", features = ["accessibility"] }
```

Until assistive technology asks for the window's content the adapter does
nothing: no snapshot is taken and nothing is allocated. After that it sends
the whole tree once and then only the nodes that changed. A host that owns
its own window instead of the engine's makes a `UiAccessAdapter` itself
(`UiAccessAdapter::for_ns_view` on macOS, `for_hwnd` on Windows,
`for_atspi` on Linux) and calls `update` once a frame after `Ui::frame`.

What is supported:

| System | Adapter |
|---|---|
| macOS | Yes: `NSAccessibility` on the window's view, through AccessKit (VoiceOver and other `NSAccessibility` clients). |
| Windows | Yes: UI Automation on the window, through AccessKit (Narrator, NVDA, JAWS and other UI Automation clients). |
| Linux | Yes: AT-SPI over D-Bus, through AccessKit (Orca and other AT-SPI clients), on X11 and Wayland. The BSDs use the same adapter. |
| Web | None: the canvas has no accessible content, and AccessKit has no adapter for one. The feature builds and does nothing. |

All three desktop adapters get the same tree, focus and requests: roles,
names, descriptions, values, checked and disabled states, modal dialogs and
live regions; press, focus, set value, increment and decrement.
`UiAccessAdapter::SUPPORTED` says at compile time whether the build has an
adapter. The limits:

- The window is named after the game's title, once: the engine has no call
  that renames its window while the game runs.
- On macOS and Windows a screen reader's questions arrive through the event
  loop. While one is reading the window, the engine leaves the event loop
  idle for 4 ms between two vsync-paced frames, so that it can answer: a
  frame that waits for the display would answer one question per frame
  (measured on macOS: a read of the whole window took 1.7 to 1.9 s, and
  VoiceOver called the game "not responding"; with the pause, 15 ms). A
  game whose frame needs more than the rest of the refresh interval loses
  a refresh while it is being read. The pause stays on once a screen reader
  has asked. On Linux AT-SPI is answered on AccessKit's own thread from its
  copy of the tree, so there is no pause.
- On Linux the adapter starts only while the desktop's accessibility is on
  (a screen reader such as Orca turns it on). If the screen reader quits
  and starts again, the whole tree is sent again. Under Wayland a program
  can't know where its window is, so the positions a screen reader gets are
  relative to the window, not the screen.
- Only `ctx.ui` is exposed. UI on surfaces in the world (`d3::UiPanel`), the
  immediate-mode calls (`draw_text`, `ui_label`) and the inspector are not.
- A text input's text is read and can be replaced, but its caret and
  selection are not sent to the system yet (they are in the snapshot), so a
  screen reader does not follow the caret character by character.
- A scroll view is exposed as a scroll area and its content is read, but a
  screen reader's own scroll requests are not handled.
- C and C++ games get the adapter the same way (it belongs to the engine's
  window) when `ponko_ffi` is built with its `accessibility` feature; there
  is no C call to make an adapter for a window of your own.

`examples/ui_menu.rs` is the sample menu for trying it: a title, a status
line that is a live region, a described Play button, a named slider and text
input, a toggle, and a Quit button that opens a modal dialog.

How it is checked:

- **The adapter's logic** (every run, every system): the tree, the changes
  and the requests as AccessKit data (`ui::access_kit` unit tests), and the
  adapter against a stand-in platform that keeps AccessKit's rules (nothing
  until a screen reader asks, the whole tree first and after a restart,
  requests from other threads), in `ui::access_native`.
- **macOS without a window** (`tests/ui_access_macos.rs`, every run, and
  CI's macOS leg): the adapter is attached to a view that has no window and
  asked what VoiceOver asks (children, role, title, value, help, enabled,
  focused, modal; press, increment, decrement, set value, set focus),
  through this menu from Play to the Quit dialog.
- **Windows through UI Automation** (`tests/ui_access_platforms.rs`, every
  run on Windows, and CI's Windows leg): the menu runs with the adapter on a
  window that is never shown, and a UI Automation client in the same
  process reads names, control types, descriptions, values and toggle
  states, then invokes Play, sets the slider and the name, moves focus,
  toggles Music and opens and closes the Quit dialog, checking each result
  through UI Automation.
- **Linux through AT-SPI** (the same file, with `PONKO_ACCESS_ATSPI=1`
  under `dbus-run-session` with `at-spi2-core` installed; CI's Linux leg
  runs it): the adapter registers with a private accessibility bus, and a
  client finds the window through the AT-SPI registry as Orca does, reads
  roles, names, descriptions, values and states, operates the menu through
  the Action, Value and Component interfaces, and turns accessibility off
  and on again to check that the tree comes back whole.
- **In the engine's own window on macOS** (by hand, `PONKO_ACCESS_WINDOW=1
  cargo test --features accessibility --test ui_access_macos
  engine_window`): the game runs with `Game::run`, which attaches the
  adapter to the window's content view; a system of the game asks that view
  the same questions and operates the menu through it. It opens a window
  for about a second, so CI doesn't run it.
- **From another process and with VoiceOver** (by hand, on a machine where
  the terminal has the Accessibility permission and VoiceOver may be
  scripted): the menu was read and operated through the system accessibility
  API from outside the game, and VoiceOver was run over it with its speech
  recorded.

What the automatic checks can't show is what a screen reader makes of the
tree: what NVDA, Narrator or Orca say, how their cursors move, whether they
announce the live region, and whether the engine's own window (made by
winit) is found and focused as expected. Those need the program with a
screen reader on each system, by hand; until such records exist for Windows
and Linux, the claim there is "the system's accessibility API reads and
operates the menu", not "tested with NVDA and Orca".

What each of these read and heard is in
[Screen reader records](ui-accessibility.md).

## UI on surfaces in the world

A computer screen, a shop terminal or a sign with buttons is a `d3::UiPanel`: a
component with its own `Ui` that the engine draws every frame into a render
target, which a glowing, alpha-blended quad shows. Build `panel.ui` just like the
screen's tree; its widgets are laid out in the target's pixels, so a 600 x 400
panel has a 600 x 400 viewport. `ctx.gfx.create_ui_panel(width, height, size)` makes
the render target, the material and the quad (`size` is in meters, in the
entity's XY plane, facing +Z) and returns the panel and the `Model` to spawn
with it, next to a `d3::Transform`. The quad is an ordinary model: move and
rotate the entity as usual.

In C++ the entity comes first: `ctx.gfx.create_ui_panel(entity, width, height, size)`
gives it the panel and the model, and `ctx.panel_ui(entity)` is the panel's
`ui::tree`, with every call of the screen's tree.

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/ui_panel.rs:panel_create}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:panel_create}}
```

</div>
</div>

The mouse uses the nearest panel the camera ray through it hits, at the pixel
it hits, unless the screen's own UI wants the pointer. Clicking a panel also
gives it the keyboard (typing, Tab, Enter) until another is clicked; the others
only see the pointer. `panel.pointer()` is where the aim is on it this frame, in
its pixels (`None` when it is off the panel), and `panel.interactive = false`
turns its input off. Events, focus and the screen stack belong to each panel's
own `Ui`: read them from it.

When the cursor is locked (a first-person game) or the aim isn't the mouse (a
controller in VR), give panels a ray yourself with `ctx.ui.set_pointer_ray`,
for example from the camera along its forward direction. `None` goes back to the
mouse.

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/ui_panel.rs:panel_events}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:panel_events}}
```

</div>
</div>

Panels are drawn in a pass of their own before the 3D frame that shows them, so
the quad is never a frame behind its contents. Headless runs build and lay out
panels (and `Ui::frame` drives them by hand, like the screen's tree) but draw
nothing and give them no mouse.

## From C

The C API mirrors the Rust one function by function. Nodes are `uint64_t` ids
(`PONKO_UI_NONE` for "no node"); a removed node's id fails with
`PONKO_NOT_FOUND` instead of panicking, and a call that fails changes nothing.

```c
{{#include ../../examples/cpp/guide.c:ui_c}}
```

Retained tree operations take a `target` after the context: `PONKO_UI_SCREEN`
for the screen's own UI, or the entity of a world-space panel to build and read
that panel's UI with the same calls. Node ids are local to the UI that made them;
the same numeric id can exist in several trees. `ponko_ctx_ui_insert_text` and
`ponko_ctx_ui_wants_keyboard` operate on context-wide input and have no target.
Panels are made with `ponko_ctx_create_ui_panel(ctx, entity, width, height, size, &texture)`,
which gives the entity (it needs a `PonkoTransform3D`) the panel and its model;
`ponko_ctx_ui_panel_pointer`, `ponko_ctx_set_ui_panel_interactive`,
`ponko_ctx_remove_ui_panel`, `ponko_ctx_set_ui_pointer_ray` and
`ponko_ctx_clear_ui_pointer_ray` do the rest.

```c
{{#include ../../examples/cpp/guide.c:ui_panel_c}}
```

`PonkoUiLayout` is the Taffy `Style` flattened: display, position, flex
direction, wrap, grow, shrink and basis, the alignment properties, width, height,
min/max sizes, margin, padding, inset, gap, aspect ratio, grid tracks (arrays with
a count, read only during the call) and grid item placement. Lengths are a kind
(automatic, points or a fraction of the parent) and a value. Enum-valued fields
are `uint32_t` holding the constants of the matching `PonkoUi*` enum, and an
unknown value is an error. `PonkoUiPaint` is `UiPaint`; fonts come from
`ponko_ctx_load_font`. Text is read back into a buffer you size with
`ponko_ctx_ui_text` (call once with capacity 0 for the length).

`ponko_ctx_ui_frame` runs a frame by hand with a `PonkoUiInput`, like
`Ui::frame`, which is how the C and C++ tests click buttons without a window.

This chapter's Rust and C++ examples build the same shop: the C++ one is
`examples/cpp/guide.cpp`, compiled and run headless in CI.
