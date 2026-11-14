# Screen reader records

What was read from, and heard about, the sample menu
(`examples/ui_menu.rs`, built with the `accessibility` feature) on macOS.
The [accessibility section](ui.md#accessibility-semantics) of the UI chapter
says what the engine exposes and where its limits are; this page is the
evidence. C and C++ games get the same: build `ponko_ffi` with its
`accessibility` feature. The names, descriptions, live regions and the
`ponko::ui::access` tracker shown in the UI chapter's C++ example are the same
from C++. All three records were made on macOS 27 on Apple silicon, with the
game in its own window.

On Windows and Linux the menu is read and operated through UI Automation
and AT-SPI by the automatic tests (see the UI chapter); no record with
NVDA, Narrator or Orca has been made yet, so what those screen readers say
is not established here. [Windows and Linux](#windows-and-linux) below says
how to hear it.

| Record | Read by | What it shows |
|---|---|---|
| [From another process](#from-another-process) | a separate program, through the system accessibility API (`AXUIElement`) | what any assistive program receives, and that it can operate the game |
| [VoiceOver](#voiceover) | VoiceOver, driven by AppleScript | what VoiceOver says for each control |
| [Inside the game](#inside-the-game) | a system of the game, through the `NSAccessibility` methods of its own window | the engine's window path, repeatable as a test |

## From another process

A small program outside the game opened the game's window through
`AXUIElementCreateApplication(pid)`, printed every element's role, title,
value, help and state, and then operated the menu: it moved keyboard focus,
stepped the slider, replaced the text, pressed buttons and the checkbox, and
opened and closed the quit dialog. Each `->` line is one request; the tree
after it shows what the game did with it (the status line under "Settings"
is the game's own reaction). The last `AXStaticText "Settings"` of each tree
is the window's title bar, which belongs to the system.

```text
{{#include ui-accessibility-outside-transcript.txt}}
```

## VoiceOver

VoiceOver was started, the game's window brought to the front, and the menu
walked twice: with the Tab key (the engine's own focus, which the VoiceOver
cursor follows) and with VoiceOver's "move right" and "move left" commands.
After each step the script asked VoiceOver for `content of last phrase`,
which is the last thing it said. VoiceOver was turned off again afterwards.

```text
{{#include ui-accessibility-voiceover-transcript.txt}}
```

How to read it:

- VoiceOver speaks a control's name, then after a pause its help and a hint.
  Where the last phrase was the hint, the line shows the hint ("Starts a new
  run. You are currently on a button...") instead of the name; "Starts a new
  run" is the Play button's description.
- Return on Play starts a run and changes the status line, a live region.
  Its announcement is not in the record: the last phrase asked for afterwards
  was the button's hint again. Whether VoiceOver speaks the live region was
  not established.
- An earlier pass of the same script, before the pause between frames
  described in the UI chapter existed, had VoiceOver say that the game "is
  not responding" twice. That is how the slow answers were found; this
  record was made after the fix.

## Inside the game

`PONKO_ACCESS_WINDOW=1 cargo test --features accessibility --test
ui_access_macos engine_window` runs the game with `Game::run`; a system of
the game asks the window's content view what a screen reader asks and
operates the menu through it. A focused element is marked.

```text
{{#include ui-accessibility-window-transcript.txt}}
```

## Hearing it yourself

1. `cargo run --example ui_menu --features accessibility`, then turn
   VoiceOver on (Cmd-F5).
2. Tab moves the engine's focus and the VoiceOver cursor follows it;
   VO-Right (Control-Option-Right) moves the VoiceOver cursor alone, also
   onto the texts.
3. On Play, press VO-Space: the status line changes to "Started a run for
   Ann".
4. On Volume, interact (VO-Shift-Down) and press VO-Right: the value goes up
   one step.
5. On Quit, press VO-Space: only the dialog "Quit the game?" with Yes and No
   can be reached. Escape or No brings the menu back.

## Windows and Linux

The same menu, the same steps, with the screen reader of each system:

- **Windows**: `cargo run --example ui_menu --features accessibility`, then
  start Narrator (Ctrl-Win-Enter) or NVDA. Tab moves the engine's focus and
  the screen reader should announce each control; the screen reader's
  default action on Play should change the status line to "Started a run
  for Ann". Narrator's scan mode or NVDA's object navigation also reach the
  texts.
  Accessibility Insights for Windows or `inspect.exe` from the Windows SDK
  show the tree the adapter gives.
- **Linux**: start Orca (Super-Alt-S on GNOME, or `orca`), then the example.
  Orca follows the engine's focus as Tab moves it; Accerciser shows the
  tree on the accessibility bus. The adapter starts only while the
  desktop's accessibility is on, which Orca does.

What to note for a record, as on macOS: what is said for each control, for
the Play button's description, for the status line after Play (a polite live
region), and for the Quit dialog, and whether the reading cursor can leave
the dialog while it is open.
