# Camera, text and UI

Big worlds need a camera, and games need words on screen. Everything here is
drawn with sprites, so it batches with the rest of the frame.

## World space and screen space

There are two kinds of coordinates:

- **World units**: sprites, `ctx.draw`, and the shapes below. They go
  through the camera.
- **Window pixels**: UI and text, drawn on top of the world. `(0, 0)` is the
  window's top-left. Use `DrawParams::default().with_screen(true)` to draw a sprite
  in window pixels (an icon in a HUD).

The default camera shows world point `(0, 0)` at the top-left at zoom 1, so
until you move it, world units are window pixels.

## The camera

`ctx.camera` is a [`d2::Camera`](https://docs.rs/ponko/latest/ponko/d2/struct.Camera.html) (C++:
`ctx.camera()` / `ctx.set_camera(c)` with the same two fields):
`position` (the world point at the window's top-left) and `zoom` (window
pixels per world unit). Its methods do the usual jobs:

| Want to | Call |
|---|---|
| follow something | `ctx.camera.look_at(hero)` every frame |
| zoom toward the mouse | `ctx.camera.zoom_at(ctx.mouse_pos(), 1.1)` |
| know what the mouse points at | `ctx.mouse_world()` |
| put a label on a unit | `ctx.camera.world_to_screen(unit)` |
| skip work for things off screen | `ctx.camera.visible_rect().contains(p)` |

For looking around a big world, add the ready-made system:

<div class="lang-pair">
<div>

```rust
Game::new("Colony", 1280, 720)
    .system(ponko::camera::camera_controls) // right/middle drag pans, wheel zooms
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:camera_controls}}
```

</div>
</div>

The ants example starts zoomed out to fit every colony:

<div class="lang-pair">
<div>

```rust
# let bounds = vec2(3200.0, 1800.0);
let fit = ctx.window_size() / bounds;
ctx.camera.zoom = fit.x.min(fit.y);
ctx.camera.look_at(bounds / 2.0);
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:camera_fit}}
```

</div>
</div>

## Shapes

In world units, like sprites:

<div class="lang-pair">
<div>

```rust
# let (from, to, tower) = (vec2(0.0, 0.0), vec2(100.0, 50.0), vec2(300.0, 300.0));
ctx.gfx.draw_rect(Rect::new(0.0, 0.0, 200.0, 20.0), Color::hex(0x3a5a40));
ctx.gfx.draw_line(from, to, 2.0, Color::RED);
ctx.gfx.draw_circle(tower, 120.0, Color::BLUE.with_alpha(0.2));
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:shapes}}
```

</div>
</div>

C++ rectangles are `{min, max}`; `ponko::rect_at(min, size)` makes one
from a corner and a size, like Rust's `Rect::new`.

## Text

<div class="lang-pair">
<div>

```rust
ctx.draw_text("Hello", vec2(20.0, 20.0), 24.0, Color::WHITE);
let size = ctx.gfx.measure_text("Paused", 32.0); // for centering
let title = ctx.gfx.load_font("fonts/Title.ttf"); // TrueType or OpenType
ctx.gfx.draw_text_ex("Colony", vec2(20.0, 60.0), TextParams::default().with_font(title).with_size(40.0));
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:text}}
```

</div>
</div>

In C++, `draw_text(text, pos, params)` is `draw_text_ex`, with
`ponko::text_defaults()` to start from.

- `size` is the line height in window pixels; `\n` starts a new line.
- The built-in font is Ubuntu Light (Ubuntu Font Licence).
- Glyphs are rasterized at the screen's real pixel size the first time
  they're drawn and kept in an atlas texture, so text is sharp on Retina
  screens and costs only a lookup per glyph after that.
- `measure_text` works headless too, so layout code can be tested.

### Other languages: fallback fonts, shaping and right-to-left text

A font covers some scripts and not others; the built-in one has Latin,
Greek and Cyrillic. Give a font a **fallback chain** and every character it
lacks comes from the first font in the chain that has it:

<div class="lang-pair">
<div>

```rust
# use ponko::prelude::*;
# fn setup(ctx: &mut Ctx) {
let arabic = ctx.gfx.try_load_font("fonts/NotoSansArabic-Subset.ttf").unwrap_or(Font::DEFAULT);
let japanese = ctx.gfx.try_load_font("fonts/NotoSansJP-Subset.otf").unwrap_or(Font::DEFAULT);
ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[arabic, japanese]);
ctx.gfx.set_text_shaping(TextShaping::Auto); // the default
let size = ctx.gfx.measure_text("Level 3: مرحبا", 24.0);
// Installed fonts need the `system-fonts` feature.
if let Ok(system) = ctx.gfx.try_load_system_font("Noto Sans") {
    ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[arabic, japanese, system]);
}
# let _ = size;
# }
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:localized_text}}
```

</div>
</div>

That is enough for scripts whose letters stand alone: Japanese, Chinese,
Korean. Arabic letters join and change shape with their neighbours,
Devanagari reorders vowel signs and stacks consonants, and Arabic and
Hebrew run right to left. Those need the **`text-shaping`** cargo feature
(on by default in the C library):

```toml
ponko = { version = "0.3", features = ["text-shaping"] }
```

With it, every string that isn't plain ASCII goes through
[harfrust](https://crates.io/crates/harfrust), the HarfBuzz project's
shaper in Rust:

- **Shaping**: the font's own substitutions and positioning (OpenType
  `GSUB` and `GPOS`): Arabic joining forms, Indic conjuncts and reordered
  vowel signs, marks placed on their letters, ligatures and kerning pairs.
- **Bidirectional text**: each line is ordered with the Unicode
  bidirectional algorithm, so "Level 3: مرحبا!" keeps "Level 3:" on the
  left and draws the Arabic word right to left after it. Its direction
  comes from its first strong letter.
- **Fallback per run**: a line is split into runs of one script and one
  font (walking the chain), and each run is shaped with its font.
- **Line breaks**: wrapped UI text breaks where Unicode's line-breaking
  rules allow (UAX #14): at spaces, and between Japanese and Chinese
  characters, which have none.

It reaches every text path: `draw_text` and `measure_text`, the retained
UI's labels and buttons (a paragraph that reads right to left starts at
the right with `UiTextAlign::Start`), the immediate widgets, text fields
(the caret and the selection follow the visual order, so a selection that
crosses a change of direction shows as two pieces; arrow keys move through
the characters in reading order) and 3D text.

**Cost.** ASCII strings keep the per-character path: the feature leaves
their layout cost as it was (the `text/layout-ascii-400` benchmark).
`TextShaping::Always` shapes them too, for ligatures and the font's
OpenType kerning in Latin text. A shaped string is cached in font units,
so drawing it again each frame costs a hash and a lookup per glyph; a new
string is shaped once. The feature adds six crates and about 1.3 MB to a
window-only release build (desktop and web alike); a build with the
default features, whose inspector already links the shaper, grows by
three crates and about 0.5 MB. Each loaded font's file also stays in
memory for the shaper.

**System fonts.** The `system-fonts` feature (desktop only) adds
`load_system_font("Family")` and `set_system_font_fallback(true)`, which
searches the computer's fonts for a character no loaded font has (useful
for text the game can't know, such as player names) and appends the font
it finds to the chain. Without the feature, and on the web, they report
`ErrorKind::Unsupported`. A shipped game should bring its own fonts.

**Not covered yet.** Color emoji: glyphs are rasterized as coverage masks,
so an emoji font's color layers or bitmaps are not drawn (outline emoji
fonts draw in one color). Thai, Lao and Khmer, which need a dictionary to
find word breaks, wrap only at spaces. Vertical text is not laid out.
Fonts' variation axes stay at their defaults.

## UI

A few immediate-mode widgets, in the same units as the retained tree's layout
(window pixels at the default scale), for debug overlays and
quick prototypes; a game's menus and HUD belong in the [retained UI](ui.md).
Call them every frame:

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/ui.rs:hud}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:hud}}
```

</div>
</div>

The widgets are on `ctx.ui.immediate` and say what they use: all of them
draw through `ctx.gfx`, and a button also reads the mouse from `ctx.input`
(C++: `ctx.ui_panel(rect)`, `ctx.ui_label(pos, text)`, `ctx.ui_button(rect, label)`).

- `panel(&mut ctx.gfx, rect)`: a background.
- `label(&mut ctx.gfx, pos, text)`: text in the style's size and color;
  returns the space it took, so you can stack lines.
- `button(&mut ctx.gfx, &ctx.input, rect, label)`: `true` on the frame it's
  clicked.
- `ctx.ui.immediate.style` sets colors, text size and padding (C++: `ui_style()`,
  `set_ui_style(s)`). It follows the screen UI's theme: `ctx.ui.set_theme(..)`
  styles the retained tree and these widgets together, and keeps the fields
  set here by hand ([Retained game UI](ui.md)).
- One scale and one theme for both styles: `ctx.ui.set_scale(2.0)` and
  `ctx.ui.set_theme(..)` set the look of the retained tree and of these
  widgets together (see below).
- `ctx.ui.wants_pointer()` says whether the mouse is over UI, so a click on a
  button doesn't also act on the world behind it. `camera_controls` checks
  it before zooming.

### One look for both styles

The immediate widgets read the same scale and theme as the screen's retained
tree, so a debug overlay next to a menu looks like part of it, on a
high-density display too. Positions and sizes you pass are in UI units, the
units of the tree's layout, and the screen's scale multiplies them (text,
the padding in the style, a button's border): `label` returns the size it
took in UI units, so stacking lines needs no change. The mouse and
`covers` stay in window pixels.

```rust
# use ponko::ui::UiTheme;
# fn setup(ctx: &mut Ctx) {
ctx.ui.set_scale(2.0); // the tree and the immediate widgets, together
ctx.ui.set_theme(UiTheme {
    accent: Color::hex(0x2e8b57),
    text_size: 20.0,
    ..Default::default()
});
// An immediate button now has the theme's accent and text size, twice as
// large as the 1x layout says: the tree's buttons look the same.
# }
```

| Retained theme / tree | Immediate widgets |
|---|---|
| `Ui::set_scale` | geometry, text and hit testing, at the same scale |
| `UiTheme` colors (`panel`, `control`, `hover`, `pressed`, `accent`, `text`) | `style` (`panel`, `button`, `button_hovered`, `button_pressed`, `accent`, `text`) |
| `UiTheme::text_size` | `style.text_size` |
| `UiTheme::font` | `ctx.ui.immediate.font` |
| `UiTheme::border_width` | `ctx.ui.immediate.border_width` (a button's border) |

A theme field left at its default keeps the immediate widgets' own default
(their panels are see-through, for overlays; their text is 16 units, not
18), so a game that sets no theme sees no change. A field set by hand, such
as `ctx.ui.immediate.style.text_size = 14.0`, stays through a later
`set_theme`. The retained theme has no spacing besides `border_width`: the
immediate `style.padding` is its own, and scales with the screen. From C,
`ponko_ctx_ui_set_scale` and `ponko_ctx_ui_set_theme` do the same; C++:
`ctx.ui_retained().set_scale(..)` and `.set_theme(..)`. The snippet is
written in Rust; the C and C++ calls above do the same. The immediate
widgets' own font, border and scale read back and change on their own too:
`ponko_ctx_ui_immediate_font`, `ponko_ctx_set_ui_immediate_font`,
`ponko_ctx_ui_immediate_border_width`,
`ponko_ctx_set_ui_immediate_border_width` and
`ponko_ctx_ui_immediate_scale` in C; `ui_immediate_font()`,
`set_ui_immediate_font(f)`, `ui_immediate_border_width()`,
`set_ui_immediate_border_width(w)` and `ui_immediate_scale()` on the C++
`ctx`. The scale is read-only: it follows `set_scale`.

The whole scene from `cargo run --release --example ui`:

```rust
{{#include ../../examples/ui.rs:scene}}
```

## For agents

The control endpoint has `camera.get` and `camera.set`, and `ponko-mcp`
has a `set_camera` tool. Combined with `screenshot`, an agent can look at
any part of a running world.
