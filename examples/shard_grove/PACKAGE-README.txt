Shard Grove
===========

A small third-person 3D game made with the Ponko engine
(https://github.com/ponkoengine/ponko). Guide the warden through a walled
grove: find the six shards while crawlers patrol and chase you, stun them
with a pulse, save at the shrines, and leave by the gate once it opens.

Run it
------
Start `shard_grove` (`shard_grove.exe` on Windows; on macOS the app) from
this folder or from any other: it finds its `assets` folder next to the
program, wherever you unpacked it. Keep the `assets` folder beside it.

Controls
--------
  Move            W A S D, the arrow keys or the left stick
  Turn the view   Q / E or the right stick
  Jump            Space, or South (A / Cross)
  Pulse           F or Left Shift, or West (X / Square)
  Pause           Esc or P, or Start
  Menus           the mouse, or Tab and the arrow keys, and Enter; Esc goes back

Three touches from a crawler and the run is lost: "Try again" starts from the
last shrine. A stunned crawler doesn't hurt.

Settings
--------
The settings menu (on the title and the pause menu) picks the graphics
quality (Low, Medium, High, Ultra; until you pick one, the game uses the one
recommended for your GPU) and the volume.

Your files
----------
The saved game, the settings and your best time are kept in your data folder:

  Windows   %APPDATA%\shard-grove\
  macOS     ~/Library/Application Support/shard-grove/
  Linux     $XDG_DATA_HOME/shard-grove/ (or ~/.local/share/shard-grove/)

`save.json` is the run, `settings.json` the settings, `records.json` the best
time and the wins. Delete the folder to start over. A damaged save is set
aside as `save.json.bad`. `PONKO_DATA_DIR=/some/folder` keeps the files in
`/some/folder/shard-grove/` instead.

Check the package
-----------------
  shard_grove --smoke headless [--report report.json]   plays by itself, no window
  shard_grove --smoke window   [--report report.json]   the same, in a real window

Licenses
--------
Shard Grove and the Ponko engine: MIT or Apache-2.0 (LICENSE-MIT,
LICENSE-APACHE, NOTICE). What the game ships: CONTENT-NOTICES.txt. Every
library linked into the program: THIRD-PARTY-NOTICES.txt.
