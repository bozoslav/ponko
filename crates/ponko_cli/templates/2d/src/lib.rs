//! The game's rules, shared by the game (`src/main.rs`) and its tests
//! (`tests/game.rs`).
//!
//! The rules live in sim systems that read an [`Intent`] instead of the
//! keyboard, so a headless test (or an agent driving the game through
//! `ponko-mcp`) can play the game without a window.

use ponko::prelude::*;
use serde::{Deserialize, Serialize};

/// The size of the playing field, in pixels.
pub const ARENA: Vec2 = Vec2::new(800.0, 600.0);
/// The size of the player's square, in pixels.
pub const PLAYER_SIZE: f32 = 40.0;

/// Numbers to tune without recompiling, loaded from `assets/settings.json`.
/// In debug builds, saving the file changes them in the running game.
#[derive(Serialize, Deserialize)]
pub struct Settings {
    /// How fast the player moves, in pixels per second.
    pub speed: f32,
}

/// Where the player wants to go this tick, -1 to 1 on each axis. The
/// keyboard sets it in a window; tests and agents set it directly.
#[derive(Default, Serialize, Deserialize)]
pub struct Intent(pub Vec2);

/// The entity the player controls.
#[derive(Component, Serialize, Deserialize)]
pub struct Player;

/// The window title (and the name agents see).
pub const TITLE: &str = "{{project-name}}";

/// The whole game, ready to `run()` in a window or `run_headless(ticks)`.
pub fn game() -> ConfiguredGame {
    Game::new(TITLE, ARENA.x as u32, ARENA.y as u32)
        .sim_system(movement)
        .system(read_input)
        .system(draw)
        .setup(setup)
}

fn setup(ctx: &mut Ctx) {
    ctx.assets
        .load_resource::<Settings>(&mut ctx.world, "settings.json");
    ctx.world.insert_resource(Intent::default());
    // Reflected types are what agents can read and change through ponko-mcp.
    ctx.world.reflect::<Player>();
    ctx.world.reflect::<ponko::d2::Transform>();
    ctx.world.reflect_resource::<Settings>();
    ctx.world.reflect_resource::<Intent>();
    ctx.world
        .spawn((Player, ponko::d2::Transform::at(ARENA / 2.0)));
}

/// Sim system: moves the player by the intent, inside the arena.
pub fn movement(ctx: &mut Ctx) {
    let step = ctx.world.resource::<Intent>().0.clamp_length_max(1.0)
        * ctx.world.resource::<Settings>().speed
        * ctx.time.dt();
    let half = Vec2::splat(PLAYER_SIZE / 2.0);
    ctx.world
        .query::<(&mut ponko::d2::Transform, &Player)>()
        .for_each(|(t, _)| {
            t.position = (t.position + step).clamp(half, ARENA - half);
        });
}

/// Frame system: the arrow keys (or WASD) set the intent. Escape quits.
fn read_input(ctx: &mut Ctx) {
    let held = |ctx: &Ctx, a: Key, b: Key| ctx.input.key_down(a) || ctx.input.key_down(b);
    let mut dir = Vec2::ZERO;
    if held(ctx, Key::Left, Key::A) {
        dir.x -= 1.0;
    }
    if held(ctx, Key::Right, Key::D) {
        dir.x += 1.0;
    }
    if held(ctx, Key::Up, Key::W) {
        dir.y -= 1.0;
    }
    if held(ctx, Key::Down, Key::S) {
        dir.y += 1.0;
    }
    ctx.world.resource_mut::<Intent>().0 = dir;
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

/// Frame system: draws the player and a hint.
fn draw(ctx: &mut Ctx) {
    let mut at = Vec::new();
    ctx.world
        .query::<(&ponko::d2::Transform, &Player)>()
        .for_each(|(t, _)| at.push(t.position));
    for p in at {
        let corner = p - Vec2::splat(PLAYER_SIZE / 2.0);
        let rect = Rect::new(corner.x, corner.y, PLAYER_SIZE, PLAYER_SIZE);
        ctx.gfx.draw_rect(rect, Color::rgb(0.95, 0.55, 0.2));
    }
    ctx.gfx.draw_text(
        "Arrow keys or WASD to move, Escape to quit",
        vec2(16.0, 16.0),
        20.0,
        Color::WHITE,
    );
}
