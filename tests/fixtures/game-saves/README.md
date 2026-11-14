# Old saves of a game that changed its components

`tests/save_migrations.rs` plays a small game whose components changed over
three save versions, and keeps a save of each older version here, as a game
keeps the saves its released versions wrote. `Ctx::check_old_saves` loads the
folder through the game's `SaveMigrations`; every file must come out as the
same world.

| File | `game_version` | What the game had then |
|---|---|---|
| `v0-before-versions.json` | none (0) | `Hp` (`max` in hit points), `Stats` (regen, speed, armor), `Jump`, `Legacy`, resource `Gold` (a number) |
| `v1.json` | 1 | `Hp` renamed `Health`; `Stats` split into `Regen` and `Speed`, armor gone; `Legacy` gone; `Gold` renamed `Purse` |
| `v2.json` | 2 | `Health.max` in hearts and a new `shield`; `Speed` and `Jump` merged into `Movement` (with a `drag` field); `Purse` an object |

Version 3, the test's current one, renames `Health.current` to `now` and
drops `Movement.drag`.

These are written by hand in the `World::save` format, the way the game's
versions would have written them. Don't edit one to make a test pass: add a
migration step instead.
