# Sample assets

The files the examples, tests and guide load (`ctx.gfx.load_texture("crab.png")`
reads `assets/crab.png`). The examples and these assets ship in the `ponko`
crate; `branding/` does not.

| File | What it is | Where it comes from |
|---|---|---|
| `crab.png` | The sprite of the examples and the guide | Added with the first sprite renderer (#3); no separate provenance note, see git history |
| `models/house.glb` | A small house, walls and a roof with two materials | Added as the sample model of the first 3D support (#51); no separate provenance note, see git history |
| `coin_dash/coin.png` | The coin of the `coin_dash` example | Drawn by a script for that example; see `examples/coin_dash/CONTENT-NOTICES.txt` |
| `ember_hop/sprites/*.png`, `ember_hop/levels/cave.png` | The sprite sheets and the tileset of the `ember_hop` platformer | Drawn by a script for that example (MIT or Apache-2.0); see `examples/ember_hop/CONTENT-NOTICES.txt` |
| `ember_hop/levels/*.tmx`, `cave.tsx` | Its two levels and the tileset, in the Tiled map editor's format | Made for that example (MIT or Apache-2.0) |
| `ember_hop/prefabs/*.json`, `ember_hop/scenes/title.json` | Its prefabs and its title scene | Written for that example (MIT or Apache-2.0) |
| `ember_hop/sounds/*.wav`, `ember_hop/music/cave.ogg` | Its sound effects and its music loop | Synthesized by a script for that example (MIT or Apache-2.0) |
| `shard_grove/grove.glb` | The glTF level of the `shard_grove` 3D reference game: ground, walls, broken walls, pillars, a platform with a ramp, the gate's arch | Made for that example (MIT or Apache-2.0); see `examples/shard_grove/CONTENT-NOTICES.txt` |
| `shard_grove/warden.glb`, `shard_grove/crawler.glb` | Its two skinned characters with their animation clips | Made for that example (MIT or Apache-2.0) |
| `shard_grove/sounds/*.wav` | Its sound effects, the crawlers' hum and the wind loop | Synthesized for that example (MIT or Apache-2.0) |
| `levels/beach.json`, `prefabs/*.json` | A scene and prefabs for the scenes chapter | Written for the engine (#50) |
| `levels/meadow.tmx`, `levels/meadow.tsx`, `levels/meadow.png` | A Tiled map, its tileset and the tileset's image, for the `level` example and the tilemaps chapter | Written for the engine (#381); the image drawn by a script |
| `tutorial_2d/cove.tmx`, `tutorial_2d/cove.tsx`, `tutorial_2d/tiles.png`, `tutorial_2d/crab.png` | The level, tileset and crab of the 2D tutorial (`tutorial_2d` example) | Made for the engine (#372); the images drawn and the level laid out by a script |
| `tutorial_2d/coin.wav`, `tutorial_2d/flag.wav` | The 2D tutorial's two sound effects | Made for the engine (#372): square and sine tones synthesized by a script |
| `tutorial_3d/yard.glb` | The glTF scene of the 3D tutorial (`tutorial_3d` example): ground, walls, a platform with a ramp, stairs and a pillar | Made for the engine (#372): boxes and a wedge written by a script |
| `shaders/*.wgsl` | Custom material shaders for the 3D examples | Written for the engine (#54) |
| `fonts/NotoSansArabic-Subset.ttf`, `fonts/NotoSansDevanagari-Subset.ttf`, `fonts/NotoSansJP-Subset.otf` | Arabic, Devanagari and Japanese fonts for the text-shaping tests and reference images, cut down with fontTools to the characters those tests draw (a few KB each) | Subsets of Noto Sans Arabic 2.013, Noto Sans Devanagari 2.007 and Noto Sans JP 2.004 by the Noto Project Authors and Adobe, under the SIL Open Font License 1.1 (not the repository's license); copyright lines and the license text in `fonts/OFL.txt` (#409) |

Files written for the engine are under the repository's license (MIT or
Apache-2.0, at your option; see `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE`
at the root).

The fonts in `fonts/` are the exception: they stay under the SIL Open Font
License 1.1 (`fonts/OFL.txt`), which lets them be used, subset and
redistributed, bundled with software too, but not sold on their own.

The logo in `branding/` has its own note in `branding/README.md`.
