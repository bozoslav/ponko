//! Crab Cove, the game the guide's "Tutorial: a first 2D game" builds step
//! by step. In a project made with `ponko new`, this file is `src/lib.rs`.
//!
//! Each step of the tutorial is one plugin: [`Level`] (sprites and a level
//! from a level editor), [`Controls`] (input actions), [`Pickups`]
//! (collisions and sound), [`Menu`] (a title screen) and [`Saves`] (the best
//! score in the player's storage). The tests in `tests/tutorial_2d.rs` play
//! the finished game headlessly.

// ANCHOR: use_level
use ponko::d2::*;
use ponko::prelude::*;
// ANCHOR_END: use_level
// ANCHOR: use_menu
use ponko::ui::{UiEvent, UiLayout};
// ANCHOR_END: use_menu
// ANCHOR: use_saves
use serde::{Deserialize, Serialize};
// ANCHOR_END: use_saves

// ANCHOR: game
/// The window title.
pub const TITLE: &str = "Crab Cove";

/// The whole game: one plugin per step of the tutorial, ready to `run()`
/// in a window or `run_headless(ticks)`.
pub fn game() -> ConfiguredGame {
    Game::new(TITLE, 960, 540)
        .configured() // the plugins hold all of the setup
        .plugin(Level) // step 1: sprites and a level
        .plugin(Controls) // step 2: input actions
        .plugin(Pickups) // step 3: collisions and sound
        .plugin(Menu) // step 4: a title screen
        .plugin(Saves) // step 5: saving the best score
}
// ANCHOR_END: game

// ANCHOR: level
/// The level file, made in the Tiled map editor.
pub const LEVEL: &str = "tutorial_2d/cove.tmx";
/// The level's size in pixels: 60 x 17 tiles of 16 pixels.
pub const LEVEL_SIZE: Vec2 = Vec2::new(960.0, 272.0);

/// The crab the player controls.
#[derive(Component)]
pub struct Crab;

/// What is on the stage now: the level's root entity and the crab.
#[derive(Clone, Copy)]
pub struct Stage {
    pub level: Entity,
    pub crab: Entity,
}

/// Step 1: the level, the crab, gravity and a camera that follows the crab.
pub struct Level;

impl Plugin for Level {
    fn build(&self, game: Game) -> Game {
        game.physics(vec2(0.0, 900.0)) // gravity: 900 pixels/s², y is down
            .system(follow_crab)
    }

    fn setup(&self, ctx: &mut Ctx) {
        ctx.gfx.clear(Color::hex(0x8ec8f0)); // the sky
        ctx.camera.zoom = 2.0; // 16-pixel tiles, drawn twice as big
        start_level(ctx);
    }
}

/// Spawns the level and puts the crab at its "Spawn" object. If a level is
/// already there, it is removed first, so this also starts over.
pub fn start_level(ctx: &mut Ctx) {
    if let Some(old) = ctx.world.get_resource::<Stage>().copied() {
        ctx.despawn_level(old.level);
        ctx.world.despawn(old.crab);
    }
    // Every tile layer, object and solid tile of the map.
    let level = ctx.spawn_level(LEVEL, Vec2::ZERO).expect("the level loads");
    // Objects keep the class they have in the editor.
    let start = ctx
        .world
        .query::<&LevelObject>()
        .iter()
        .find(|o| o.class == "Spawn")
        .map_or(vec2(40.0, 180.0), |o| o.position);
    let texture = ctx.load_texture("tutorial_2d/crab.png");
    let crab = ctx.world.spawn((
        Transform::at(start),
        Sprite::new(texture),
        // A body that falls and collides, but doesn't tip over.
        RigidBody {
            lock_rotation: true,
            ..RigidBody::dynamic()
        },
        Collider::rect(vec2(12.0, 10.0)).offset(vec2(0.0, 2.0)).friction(0.0),
        Velocity::default(),
        Crab,
    ));
    ctx.world.insert_resource(Stage { level, crab });
}

/// Frame system: the camera follows the crab, without showing past the
/// level's ends.
fn follow_crab(ctx: &mut Ctx) {
    let crab = ctx.world.resource::<Stage>().crab;
    if let Some(t) = ctx.world.get::<Transform>(crab) {
        let half_view = ctx.camera.visible_rect().size() / 2.0;
        let x = t.position.x.clamp(half_view.x, LEVEL_SIZE.x - half_view.x);
        ctx.camera.look_at(vec2(x, LEVEL_SIZE.y - half_view.y));
    }
}
// ANCHOR_END: level

// ANCHOR: bindings
// The game's actions, looked up by name once.
actions! {
    pub struct Act {
        walk = "walk",
        jump = "jump",
        menu = "menu",
    }
}

/// How fast the crab walks, in pixels per second.
pub const WALK_SPEED: f32 = 110.0;
/// How fast a jump starts, in pixels per second.
pub const JUMP_SPEED: f32 = 330.0;

/// Keys and a gamepad, bound to the same three actions.
pub fn bindings() -> ActionMap {
    let pad = ponko::input::Gamepad::from_slot(0).expect("slot 0 exists");
    let mut map = ActionMap::default();
    map.set(
        "walk",
        Action::Axis(vec![
            AxisBinding::Keys {
                negative: Key::Left,
                positive: Key::Right,
            },
            AxisBinding::Keys {
                negative: Key::A,
                positive: Key::D,
            },
            AxisBinding::Gamepad {
                pad,
                axis: ponko::input::GamepadAxis::LeftStickX,
                deadzone: 0.2,
            },
        ]),
    );
    map.set(
        "jump",
        Action::Button(vec![
            ButtonBinding::Key(Key::Space),
            ButtonBinding::Key(Key::Up),
            ButtonBinding::Key(Key::W),
            ButtonBinding::Gamepad {
                pad,
                button: ponko::input::GamepadButton::South,
            },
        ]),
    );
    // Opens the title screen (step 4).
    map.set(
        "menu",
        Action::Button(vec![
            ButtonBinding::Key(Key::Escape),
            ButtonBinding::Gamepad {
                pad,
                button: ponko::input::GamepadButton::Start,
            },
        ]),
    );
    map
}

// ANCHOR_END: bindings

// ANCHOR: controls
/// Step 2: the actions move the crab.
pub struct Controls;

impl Plugin for Controls {
    fn build(&self, game: Game) -> Game {
        game.system(control)
    }

    fn setup(&self, ctx: &mut Ctx) {
        ctx.input.set_actions(bindings());
        let act = Act::resolve(ctx);
        ctx.world.insert_resource(act);
    }
}

/// Frame system: walk while the action is held, jump on the frame it is
/// pressed, but only when standing on something.
fn control(ctx: &mut Ctx) {
    let act = *ctx.world.resource::<Act>();
    let walk = ctx.input.action_axis(act.walk);
    let jump = ctx.input.action_pressed(act.jump);
    let crab = ctx.world.resource::<Stage>().crab;
    let Some(at) = ctx.world.get::<Transform>(crab).map(|t| t.position) else {
        return;
    };
    // Standing: a short ray from the crab's middle hits ground below its feet.
    let ground = QueryFilter::default().exclude(crab);
    let standing = ctx.physics.cast_ray(at, Vec2::Y, 9.0, ground).is_some();
    if let Some(v) = ctx.world.get_mut::<Velocity>(crab) {
        v.linear.x = walk * WALK_SPEED;
        if jump && standing {
            v.linear.y = -JUMP_SPEED; // y is down: up is negative
        }
    }
}
// ANCHOR_END: controls

// ANCHOR: pickups
/// The score of the run in progress.
#[derive(Default)]
pub struct Score {
    pub coins: u32,
}

/// Event: the crab reached the flag.
#[derive(Clone, Copy, Debug)]
pub struct LevelDone {
    pub coins: u32,
    pub total: u32,
}

/// The sound effects.
#[derive(Clone, Copy)]
pub struct Sounds {
    coin: Sound,
    flag: Sound,
}

/// Step 3: coins and the flag become sensors, touching them counts.
pub struct Pickups;

impl Plugin for Pickups {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::BeforePhysics, make_sensors)
            .sim_system(touch)
            .sim_system(fall_off)
            .system(hud)
    }

    fn setup(&self, ctx: &mut Ctx) {
        let sounds = Sounds {
            coin: ctx.load_sound("tutorial_2d/coin.wav"),
            flag: ctx.load_sound("tutorial_2d/flag.wav"),
        };
        ctx.world.insert_resource(sounds);
        ctx.world.insert_resource(Score::default());
    }
}

/// Sim system: every coin and flag the level spawned gets a sensor, a
/// collider that reports overlaps instead of pushing back.
fn make_sensors(ctx: &mut Ctx) {
    let new: Vec<Entity> = ctx
        .world
        .query::<(Entity, &LevelObject)>()
        .filter::<Without<Collider>>()
        .iter()
        .filter(|(_, o)| o.class == "Coin" || o.class == "Goal")
        .map(|(e, _)| e)
        .collect();
    for e in new {
        ctx.world.insert(e, RigidBody::fixed());
        ctx.world.insert(e, Collider::circle(6.0).sensor());
    }
}

/// Sim system, after the physics step: what the crab touched.
fn touch(ctx: &mut Ctx) {
    let crab = ctx.world.resource::<Stage>().crab;
    let touched: Vec<Entity> = ctx
        .events
        .read::<CollisionStarted>()
        .iter()
        .filter(|c| c.sensor && (c.a == crab || c.b == crab))
        .map(|c| if c.a == crab { c.b } else { c.a })
        .collect();
    let sounds = *ctx.world.resource::<Sounds>();
    for thing in touched {
        let class = ctx.world.get::<LevelObject>(thing).map(|o| o.class.clone());
        match class.as_deref() {
            Some("Coin") => {
                ctx.world.despawn(thing);
                ctx.world.resource_mut::<Score>().coins += 1;
                ctx.play_sound(sounds.coin);
            }
            Some("Goal") => {
                let coins = std::mem::take(&mut ctx.world.resource_mut::<Score>().coins);
                let left = coins_left(&mut ctx.world);
                ctx.events.send(LevelDone {
                    coins,
                    total: coins + left,
                });
                ctx.play_sound(sounds.flag);
                start_level(ctx);
                return;
            }
            _ => {}
        }
    }
}

/// The coins still in the level.
pub fn coins_left(world: &mut World) -> u32 {
    let coins = world
        .query::<&LevelObject>()
        .iter()
        .filter(|o| o.class == "Coin")
        .count();
    coins as u32
}

/// Sim system: falling into the water starts the level over.
fn fall_off(ctx: &mut Ctx) {
    let crab = ctx.world.resource::<Stage>().crab;
    let fell = ctx
        .world
        .get::<Transform>(crab)
        .is_some_and(|t| t.position.y > LEVEL_SIZE.y);
    if fell {
        ctx.world.resource_mut::<Score>().coins = 0;
        start_level(ctx);
    }
}

/// Frame system: the coins collected, in the window's corner.
fn hud(ctx: &mut Ctx) {
    let coins = ctx.world.resource::<Score>().coins;
    let total = coins + coins_left(&mut ctx.world);
    let text = format!("Coins: {coins} / {total}");
    ctx.draw_text(&text, vec2(16.0, 12.0), 28.0, Color::WHITE);
}
// ANCHOR_END: pickups

// ANCHOR: menu
/// The title screen's nodes.
#[derive(Clone, Copy)]
pub struct TitleScreen {
    pub screen: UiId,
    pub message: UiId,
    pub play: UiId,
    pub quit: UiId,
}

/// Step 4: a title screen with Play and Quit. The game waits while it is open.
pub struct Menu;

impl Plugin for Menu {
    fn build(&self, game: Game) -> Game {
        game.system(menu)
    }

    fn setup(&self, ctx: &mut Ctx) {
        let ui = &mut ctx.ui;
        let screen = ui
            .root()
            .layout(UiLayout::centered(360.0, 300.0).padding(24.0).gap(12.0))
            .id();
        ui.title(screen, TITLE).layout(UiLayout::row(44.0));
        let message = ui
            .label(screen, "Collect the coins, then touch the flag.")
            .layout(UiLayout::row(24.0))
            .name("menu.message")
            .id();
        // Filled in by step 5.
        ui.label(screen, "").layout(UiLayout::row(24.0)).name("menu.best");
        let play = ui
            .button(screen, "Play")
            .layout(UiLayout::row(44.0))
            .name("menu.play")
            .focused() // the screen opens on Play: Enter starts the game
            .id();
        let quit = ui
            .button(screen, "Quit")
            .layout(UiLayout::row(44.0))
            .name("menu.quit")
            .id();
        ui.push_screen(screen, false);
        ctx.world.insert_resource(TitleScreen {
            screen,
            message,
            play,
            quit,
        });
    }
}

/// Frame system: the buttons, Escape, and the result of a finished level.
fn menu(ctx: &mut Ctx) {
    let title = *ctx.world.resource::<TitleScreen>();
    let (mut play, mut quit, mut closed) = (false, false, false);
    for event in ctx.ui.events() {
        match event {
            UiEvent::Clicked(id) if id == title.play => play = true,
            UiEvent::Clicked(id) if id == title.quit => quit = true,
            // Escape (or a gamepad's East) closes the screen.
            UiEvent::ScreenPopped(_) => closed = true,
            _ => {}
        }
    }
    if play {
        ctx.ui.pop_screen();
        ctx.world.resource_mut::<Score>().coins = 0;
        start_level(ctx);
    }
    if quit {
        ctx.quit();
    }
    let done: Vec<LevelDone> = ctx.events.read::<LevelDone>().to_vec();
    if let Some(last) = done.last() {
        let text = format!("You found {} of {} coins!", last.coins, last.total);
        ctx.ui.set_text(title.message, text);
        open(ctx, title);
    }
    let act = *ctx.world.resource::<Act>();
    if ctx.input.action_pressed(act.menu) && !closed && !ctx.ui.is_shown(title.screen) {
        ctx.ui.set_text(title.message, "Paused");
        open(ctx, title);
    }
    // The world waits while the title screen is open.
    if ctx.ui.is_shown(title.screen) {
        ctx.time.pause();
    } else {
        ctx.time.resume();
    }
}

/// Opens the title screen, unless it is open.
fn open(ctx: &mut Ctx, title: TitleScreen) {
    if !ctx.ui.is_shown(title.screen) {
        ctx.ui.push_screen(title.screen, false);
    }
}
// ANCHOR_END: menu

// ANCHOR: saves
/// The game's folder in the player's data folder.
pub const APP_ID: &str = "crab-cove";
/// The file the progress is kept in.
pub const PROGRESS_FILE: &str = "progress.json";

/// What the game remembers between runs.
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// The most coins collected in one run that reached the flag.
    pub best: u32,
    /// How many times the flag was reached.
    pub wins: u32,
}

/// Step 5: the best score, kept in `ctx.storage`.
pub struct Saves;

impl Plugin for Saves {
    fn build(&self, game: Game) -> Game {
        game.app_id(APP_ID).system(save_progress)
    }

    fn setup(&self, ctx: &mut Ctx) {
        // No file yet is a first run: start from zero. (So is a damaged
        // file here; the "Saves and settings" chapter shows how to keep it.)
        let progress = ctx.storage.read_json::<Progress>(PROGRESS_FILE).unwrap_or_default();
        show_best(ctx, &progress);
        ctx.world.insert_resource(progress);
    }
}

/// Frame system: a finished level updates the progress and saves it.
fn save_progress(ctx: &mut Ctx) {
    let done: Vec<LevelDone> = ctx.events.read::<LevelDone>().to_vec();
    if done.is_empty() {
        return;
    }
    let mut progress = ctx.world.resource::<Progress>().clone();
    for d in done {
        progress.wins += 1;
        progress.best = progress.best.max(d.coins);
    }
    // A write that fails (a full or read-only disk) must not stop the game.
    if let Err(e) = ctx.storage.write_json(PROGRESS_FILE, &progress) {
        eprintln!("couldn't save the progress: {e}");
    }
    show_best(ctx, &progress);
    ctx.world.insert_resource(progress);
}

/// Shows the best score on the title screen.
fn show_best(ctx: &mut Ctx, progress: &Progress) {
    if let Some(label) = ctx.ui.find("menu.best") {
        let text = match progress.wins {
            0 => String::new(),
            _ => format!("Best: {} coins", progress.best),
        };
        ctx.ui.set_text(label, text);
    }
}
// ANCHOR_END: saves
