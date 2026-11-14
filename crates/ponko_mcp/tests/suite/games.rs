//! The games the tasks start from. They follow the 2D project template
//! (`ponko new --template 2d`): the rules are sim systems, numbers to tune
//! live in JSON files under `assets/` that hot reload, and every gameplay
//! type is reflected so the MCP tools can read and change it.
//!
//! Each game is small on purpose: a task is about finding and fixing one
//! thing with the tools, not about reading a large code base.

use super::tasks::GameAndSetup;
use ponko::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ------------------------------------------------------------------ arena

/// The playing field, in pixels.
pub const ARENA: Vec2 = Vec2::new(800.0, 600.0);
/// Where the player starts.
pub const START: Vec2 = Vec2::new(100.0, 300.0);
/// How close the player has to come to collect a coin.
pub const PICKUP_RADIUS: f32 = 24.0;

/// Numbers to tune without recompiling: `assets/settings.json`.
#[derive(Serialize, Deserialize)]
pub struct Settings {
    /// How fast the player moves, in pixels per second.
    pub speed: f32,
}

/// Where the player wants to go this tick; the keyboard sets it.
#[derive(Default, Serialize, Deserialize)]
pub struct Intent(pub Vec2);

/// The entity the player controls.
#[derive(Component, Serialize, Deserialize)]
pub struct Player;

/// A coin; touching it adds `value` to the score.
#[derive(Component, Serialize, Deserialize)]
pub struct Coin {
    /// Points it is worth.
    pub value: u32,
}

/// The score.
#[derive(Default, Serialize, Deserialize)]
pub struct Score {
    /// Points so far.
    pub points: u32,
}

/// The finish line, and the tick the player first crossed it.
#[derive(Serialize, Deserialize)]
pub struct Flag {
    /// The line's x.
    pub x: f32,
    /// The first tick the player was at or past it; `null` until then.
    pub reached_tick: Option<u64>,
}

/// The arena: a player that the arrow keys move, coins, a score and a flag.
fn arena(game: Game) -> Game {
    game.headless_frames(true)
        .system(read_input)
        .sim_system(movement)
        .sim_system(collect)
        .sim_system(finish_line)
}

fn arena_setup(ctx: &mut Ctx) {
    ctx.assets.load_resource::<Settings>(&mut ctx.world, "settings.json");
    ctx.world.insert_resource(Intent::default());
    ctx.world.insert_resource(Score::default());
    ctx.world.insert_resource(Flag {
        x: 700.0,
        reached_tick: None,
    });
    ctx.world.reflect::<Player>();
    ctx.world.reflect::<Coin>();
    ctx.world.reflect::<d2::Transform>();
    ctx.world.reflect_resource::<Settings>();
    ctx.world.reflect_resource::<Intent>();
    ctx.world.reflect_resource::<Score>();
    ctx.world.reflect_resource::<Flag>();
    ctx.world.spawn((Player, d2::Transform::at(START)));
}

/// Frame system: arrow keys (or WASD) set the intent.
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
}

/// Sim system: moves the player by the intent, inside the arena.
fn movement(ctx: &mut Ctx) {
    let step =
        ctx.world.resource::<Intent>().0.clamp_length_max(1.0) * ctx.world.resource::<Settings>().speed * ctx.time.dt();
    ctx.world
        .query::<(&mut d2::Transform, &Player)>()
        .for_each(|(t, _)| t.position = (t.position + step).clamp(Vec2::ZERO, ARENA));
}

/// Sim system: coins the player touches go, and add to the score.
fn collect(ctx: &mut Ctx) {
    let mut players = Vec::new();
    ctx.world
        .query::<(&d2::Transform, &Player)>()
        .for_each(|(t, _)| players.push(t.position));
    let mut taken = Vec::new();
    ctx.world
        .query::<(Entity, &d2::Transform, &Coin)>()
        .for_each(|(e, t, c)| {
            if players.iter().any(|p| p.distance(t.position) <= PICKUP_RADIUS) {
                taken.push((e, c.value));
            }
        });
    for (e, value) in taken {
        ctx.world.despawn(e);
        ctx.world.resource_mut::<Score>().points += value;
    }
}

/// Sim system: notes the first tick the player reaches the flag.
fn finish_line(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    let x = ctx.world.resource::<Flag>().x;
    let mut past = false;
    ctx.world
        .query::<(&d2::Transform, &Player)>()
        .for_each(|(t, _)| past |= t.position.x >= x);
    let flag = ctx.world.resource_mut::<Flag>();
    if past && flag.reached_tick.is_none() {
        flag.reached_tick = Some(tick);
    }
}

/// `tune-speed` and `coin-level`.
pub fn arena_game() -> GameAndSetup {
    (arena(Game::new("arena", ARENA.x as u32, ARENA.y as u32)), arena_setup)
}

// ------------------------------------------------------------------ enemies

/// One line of `assets/enemies.json`: an enemy the wave spawns.
#[derive(Clone, Serialize, Deserialize)]
pub struct EnemyKind {
    /// Its name.
    pub name: String,
    /// Health it starts with.
    pub health: f32,
    /// How much of the hazard's damage it ignores: 0 none, 0.5 half.
    pub armor: f32,
}

/// The enemies a wave is made of, from `assets/enemies.json`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Roster {
    /// One entry per enemy.
    pub enemies: Vec<EnemyKind>,
}

/// An enemy.
#[derive(Component, Serialize, Deserialize)]
pub struct Enemy {
    /// Its name, from the roster.
    pub name: String,
    /// Damage it ignores (see [`EnemyKind::armor`]).
    pub armor: f32,
}

/// Health.
#[derive(Component, Serialize, Deserialize)]
pub struct Health {
    /// Left.
    pub current: f32,
}

/// Set `wave` to true to replace every enemy with a fresh wave from the
/// roster (the game does it at the start, and between rounds).
#[derive(Serialize, Deserialize)]
pub struct Waves {
    /// A new wave is wanted.
    pub wave: bool,
    /// Waves spawned so far.
    pub spawned: u32,
}

/// Sent when an enemy dies.
#[derive(Serialize)]
pub struct Died {
    /// Who.
    pub name: String,
    /// When.
    pub tick: u64,
}

/// Damage per second that the hazard deals to every enemy.
pub const HAZARD_DPS: f32 = 30.0;

fn enemies_setup(ctx: &mut Ctx) {
    ctx.assets.load_resource::<Roster>(&mut ctx.world, "enemies.json");
    ctx.world.insert_resource(Waves { wave: true, spawned: 0 });
    ctx.world.reflect::<Enemy>();
    ctx.world.reflect::<Health>();
    ctx.world.reflect_resource::<Roster>();
    ctx.world.reflect_resource::<Waves>();
    ctx.events.reflect::<Died>();
    spawn_wave(ctx);
}

/// Sim system: a new wave when one is asked for.
fn waves(ctx: &mut Ctx) {
    if ctx.world.resource::<Waves>().wave {
        spawn_wave(ctx);
    }
}

fn spawn_wave(ctx: &mut Ctx) {
    let mut old = Vec::new();
    ctx.world.query::<(Entity, &Enemy)>().for_each(|(e, _)| old.push(e));
    for e in old {
        ctx.world.despawn(e);
    }
    let roster = ctx.world.resource::<Roster>().clone();
    for (i, kind) in roster.enemies.iter().enumerate() {
        ctx.world.spawn((
            Enemy {
                name: kind.name.clone(),
                armor: kind.armor,
            },
            Health { current: kind.health },
            d2::Transform::at(vec2(100.0 + 100.0 * i as f32, 300.0)),
        ));
    }
    let waves = ctx.world.resource_mut::<Waves>();
    waves.wave = false;
    waves.spawned += 1;
}

/// Sim system: the hazard hurts every enemy, less through armor.
fn hazard(ctx: &mut Ctx) {
    let damage = HAZARD_DPS * ctx.time.dt();
    ctx.world
        .query::<(&mut Health, &Enemy)>()
        .for_each(|(h, e)| h.current -= damage * (1.0 - e.armor));
}

/// Sim system: enemies at zero health die.
fn deaths(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    let mut dead = Vec::new();
    ctx.world
        .query::<(Entity, &Health, &Enemy)>()
        .for_each(|(e, h, enemy)| {
            if h.current <= 0.0 {
                dead.push((e, enemy.name.clone()));
            }
        });
    for (e, name) in dead {
        ctx.world.despawn(e);
        ctx.events.send(Died { name, tick });
    }
}

/// `immortal-enemy`.
pub fn enemies_game() -> GameAndSetup {
    let game = Game::new("waves", ARENA.x as u32, ARENA.y as u32)
        .sim_system(waves)
        .sim_system(hazard)
        .sim_system(deaths);
    (game, enemies_setup)
}

// ------------------------------------------------------------------ crowd

/// How the crowding system searches for neighbours: `assets/tuning.json`.
#[derive(Serialize, Deserialize)]
pub struct Tuning {
    /// Two villagers closer than this are crowded.
    pub radius: f32,
    /// The side of the search grid's cells. The search looks at the
    /// villager's cell and the eight around it.
    pub cell_size: f32,
}

/// A villager.
#[derive(Component, Serialize, Deserialize)]
pub struct Villager;

/// What the crowding system found in its last tick.
#[derive(Default, Serialize, Deserialize)]
pub struct Crowding {
    /// Pairs of villagers closer than `Tuning.radius`.
    pub pairs: u64,
    /// Distance checks it made to find them: the work a tick costs.
    pub checks: u64,
}

/// Villagers in the crowd.
pub const VILLAGERS: usize = 1500;
/// The field they stand in, in pixels.
pub const FIELD: f32 = 2000.0;

/// The villagers' positions: fixed, from a small generator, so every run
/// (and the checker's own count) sees the same crowd.
pub fn villager_positions() -> Vec<Vec2> {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    (0..VILLAGERS).map(|_| vec2(next() * FIELD, next() * FIELD)).collect()
}

fn crowd_setup(ctx: &mut Ctx) {
    ctx.assets.load_resource::<Tuning>(&mut ctx.world, "tuning.json");
    ctx.world.insert_resource(Crowding::default());
    ctx.world.reflect::<Villager>();
    ctx.world.reflect::<d2::Transform>();
    ctx.world.reflect_resource::<Tuning>();
    ctx.world.reflect_resource::<Crowding>();
    for p in villager_positions() {
        ctx.world.spawn((Villager, d2::Transform::at(p)));
    }
}

/// Sim system: counts crowded pairs with a grid of `Tuning.cell_size`.
fn crowding(ctx: &mut Ctx) {
    let (radius, cell) = {
        let t = ctx.world.resource::<Tuning>();
        (t.radius, t.cell_size.max(1.0))
    };
    let mut at = Vec::new();
    ctx.world
        .query::<(&d2::Transform, &Villager)>()
        .for_each(|(t, _)| at.push(t.position));
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in at.iter().enumerate() {
        grid.entry(key(*p)).or_default().push(i);
    }
    let (mut pairs, mut checks) = (0, 0);
    for (i, p) in at.iter().enumerate() {
        let (cx, cy) = key(*p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in grid.get(&(cx + dx, cy + dy)).map_or(&[][..], Vec::as_slice) {
                    if j > i {
                        checks += 1;
                        if p.distance(at[j]) < radius {
                            pairs += 1;
                        }
                    }
                }
            }
        }
    }
    *ctx.world.resource_mut::<Crowding>() = Crowding { pairs, checks };
}

/// `slow-crowd`.
pub fn crowd_game() -> GameAndSetup {
    let game = Game::new("crowd", ARENA.x as u32, ARENA.y as u32).sim_system(crowding);
    (game, crowd_setup)
}

// ------------------------------------------------------------------ glow

/// The shader that makes the player glow.
pub const GLOW_SHADER: &str = "shaders/glow.wgsl";

fn glow_setup(ctx: &mut Ctx) {
    arena_setup(ctx);
    // With hot reload on, a shader that doesn't compile gets a stand-in and
    // stays watched until a saved fix compiles.
    let _glow = ctx.gfx.load_sprite_shader(GLOW_SHADER);
}

/// `broken-shader`.
pub fn glow_game() -> GameAndSetup {
    (arena(Game::new("glow", ARENA.x as u32, ARENA.y as u32)), glow_setup)
}

// ------------------------------------------------------------------ hud

/// How the HUD shows the score: `assets/hud.json`.
#[derive(Serialize, Deserialize)]
pub struct HudStyle {
    /// The score label's text; `{points}` becomes the score.
    pub score_text: String,
    /// Whether the score label is shown.
    pub show_score: bool,
}

/// The HUD's nodes.
#[derive(Clone, Copy)]
struct Hud {
    score: ponko::ui::UiId,
}

fn hud_setup(ctx: &mut Ctx) {
    arena_setup(ctx);
    ctx.assets.load_resource::<HudStyle>(&mut ctx.world, "hud.json");
    ctx.world.reflect_resource::<HudStyle>();
    let root = ctx.ui.root().name("hud").id();
    let score = ctx.ui.label(root, "").name("hud.score").id();
    ctx.ui.push_screen(root, false);
    ctx.world.insert_resource(Hud { score });
}

/// Frame system: the score label follows the score and the HUD style.
fn hud(ctx: &mut Ctx) {
    let Hud { score } = *ctx.world.resource::<Hud>();
    let (text, show) = {
        let style = ctx.world.resource::<HudStyle>();
        let points = ctx.world.resource::<Score>().points;
        (
            style.score_text.replace("{points}", &points.to_string()),
            style.show_score,
        )
    };
    ctx.ui.set_text(score, text);
    let layout = ponko::ui::UiLayout::auto();
    ctx.ui.set_layout(score, if show { layout } else { layout.hidden() });
}

/// `hidden-score`.
pub fn hud_game() -> GameAndSetup {
    let game = arena(Game::new("hud", ARENA.x as u32, ARENA.y as u32)).system(hud);
    (game, hud_setup)
}
