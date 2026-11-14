//! Crab Cove: the game the guide's "Tutorial: a first 2D game" builds.
//!
//!   `cargo run --example tutorial_2d --features physics`
//!
//! Arrow keys or A/D walk, Space, Up or W jumps (a gamepad's left stick and
//! South button too). Collect the coins and touch the flag; Escape opens the
//! title screen. The best score is kept in the player's data folder.
//!
//! The game is in `game.rs` (in a project made with `ponko new`, that is
//! `src/lib.rs`, and this file is `src/main.rs`). `tests/tutorial_2d.rs`
//! plays it headlessly. The web build:
//! `web/build.sh --features physics tutorial_2d`.

mod game;

use game::game; // in your project: `use crab_cove::game;`

// ANCHOR: main
fn main() {
    if std::env::var("HEADLESS").is_ok_and(|v| v != "0") {
        let ticks = std::env::var("TICKS").ok().and_then(|t| t.parse().ok()).unwrap_or(600);
        let world = game().run_headless(ticks);
        println!("ran {ticks} ticks headless: {} entities", world.len());
    } else {
        game().run();
    }
}
// ANCHOR_END: main
