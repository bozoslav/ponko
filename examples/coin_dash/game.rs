//! The game: rules, input mapping and drawing. Shared with `tests/coin_dash.rs`.
//!
//! Three layers, each a [`Plugin`] and testable on its own:
//!
//! 1. [`PlayerInput`]: `read_input` (frame system) turns keys and gamepad
//!    into an [`Intent`].
//! 2. [`Rules`]: `rules` (sim system) is the whole game: it reads only the
//!    `Intent` and the world, so a test can replace step 1 with a script and
//!    get the same result on every run.
//! 3. [`Drawing`]: entities with a `Sprite` are drawn by the engine's
//!    built-in `draw_sprites`; `hud` adds the text. Neither changes the game.
//!
//! [`SettingsScreen`] is a fourth plugin beside them, and [`CoinDash`] adds
//! all four.
//!
//! The game moves through five [`Screen`]s: `Title` → `Playing` ⇄ `Paused`,
//! `Playing` → `Result` → `Playing` again (or back to `Title`), and
//! `Title` ⇄ `Settings`. Only `rules` changes the screen. The best time and
//! most coins are kept in a small [`Progress`] file, and the player's choices
//! (UI scale, volume, rebound controls) in a [`settings::Settings`] file; both
//! survive a missing or damaged file.

// The test file includes this module but needs only part of it.
#![allow(dead_code)]

// Next to this file, whichever crate includes it (`main.rs` or `tests/coin_dash.rs`).
#[path = "settings.rs"]
pub mod settings;
#[path = "smoke.rs"]
pub mod smoke;

#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform};
use ponko::prelude::*;
use ponko::storage::Storage;
use serde::{Deserialize, Serialize};
use settings::Settings;

/// Size of the playfield, in world units (the window starts the same size).
pub const ARENA: Vec2 = vec2(800.0, 600.0);
/// Coins on the field at the start of a round.
pub const COINS: usize = 8;
/// Seconds to collect them all.
pub const ROUND_SECONDS: f32 = 20.0;
/// Player speed, units per second.
pub const SPEED: f32 = 240.0;
pub const PLAYER_RADIUS: f32 = 14.0;
pub const COIN_RADIUS: f32 = 10.0;

/// The player. Position is the entity's [`Transform`].
#[derive(Component)]
pub struct Player;
/// A coin to collect.
#[derive(Component)]
pub struct Coin;

/// The pictures, made or loaded once at startup: a procedural disc (the
/// player) and the coin, the one file the game loads from `assets/`.
#[derive(Clone, Copy)]
pub struct Art {
    disc: Texture,
    pub coin: Texture,
}

/// The coin's file, relative to `assets/`. The packaged game finds it next to
/// the executable, wherever it was unpacked.
pub const COIN_ASSET: &str = "coin_dash/coin.png";

/// The sound effects, made from sine tones at startup (no sound files).
#[derive(Clone, Copy)]
pub struct Sounds {
    pickup: Sound,
    win: Sound,
    lose: Sound,
    select: Sound,
}

/// Which screen the game is on. A resource, so tests and tools can read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The menu shown at startup: best results, "press Enter".
    Title,
    /// UI scale, volume and controls, drawn by the retained UI in `settings`.
    Settings,
    /// A round is running.
    Playing,
    /// A round is frozen. Nothing moves and the clock stops.
    Paused,
    /// The round ended; waiting for "play again" or "back to title".
    Result(Outcome),
}

/// How a round ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Won,
    Lost,
}

/// Round bookkeeping. One resource, so tests and tools can read it directly.
#[derive(Debug, Clone)]
pub struct Round {
    pub collected: usize,
    pub time_left: f32,
    /// 0 before the first round, then 1 for the first round, +1 for every new one.
    pub number: u32,
    /// The finished round beat a saved best (set when it ends).
    pub new_best: bool,
}

/// What the player asked for since the last simulation tick. The sim reads
/// this, never the keyboard, so a test can write it directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct Intent {
    /// Movement, each axis in `-1..=1`; y is positive down.
    pub direction: Vec2,
    // The buttons are latched until the sim takes them, so a tap between two
    // ticks (a frame with no tick) is not lost.
    /// Start, resume, play again.
    pub confirm: bool,
    /// Start a new round right now.
    pub restart: bool,
    /// Pause or resume; on the title screen, quit.
    pub pause: bool,
    /// Back to the title screen; on the title screen, quit.
    pub back: bool,
    /// Open the settings from the title screen.
    pub settings: bool,
    /// Leave the settings (its Back button or the Escape key).
    pub close_settings: bool,
}

// ANCHOR: controls
actions! {
    /// The action names from [`bindings`], resolved once at startup. Queries by
    /// `ActionId` are an array index, and a name that `bindings` doesn't define
    /// is logged once when resolved instead of silently reading false forever.
    pub struct Act {
        movement = "move",
        confirm = "confirm",
        restart = "restart",
        pause = "pause",
        back = "back",
        settings = "settings",
    }
}
// ANCHOR_END: controls

/// Random seed for coin layouts: the same seed and the same inputs always give
/// the same game.
pub const SEED: u64 = 2024;

/// The name of the game's folder in the player's data folder, where
/// `ctx.storage` keeps [`PROGRESS_FILE`] and the settings.
pub const APP_ID: &str = "coin-dash";

/// The game without a window, ready for `run` or `run_headless`: the
/// [`CoinDash`] plugin with progress and settings in the player's data
/// folder (`PONKO_DATA_DIR` moves it, handy for tests and smoke runs).
pub fn build() -> Game {
    Game::new("Coin Dash", ARENA.x as u32, ARENA.y as u32)
        .seed(SEED)
        .app_id(APP_ID)
        .plugin(CoinDash)
}

// ANCHOR: plugins
/// The whole game: its layers, each a plugin of its own. Startup is the
/// plugins' `setup`, in this order. Progress and settings are kept in
/// `ctx.storage`; a game without an app id (or a data folder) just doesn't
/// keep them.
pub struct CoinDash;

impl Plugin for CoinDash {
    fn build(&self, game: Game) -> Game {
        game.plugin(PlayerInput)
            .plugin(SettingsScreen)
            .plugin(Rules)
            .plugin(Drawing)
    }
}

/// Layer 1: keys and gamepad to an [`Intent`], once per rendered frame.
pub struct PlayerInput;

impl Plugin for PlayerInput {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::Frame, read_input)
    }
    fn setup(&self, ctx: &mut Ctx) {
        // The default controls; the ids stay valid when the settings (or a
        // rebinding) replace them.
        ctx.input.set_actions(bindings());
        ctx.world.insert_resource(Act::resolve(ctx));
        ctx.world.insert_resource(Intent::default());
    }
}

/// The player's choices (UI scale, volume, controls): loaded at startup,
/// changed on the settings screen.
pub struct SettingsScreen;

impl Plugin for SettingsScreen {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::Frame, settings::settings_ui)
    }
    fn setup(&self, ctx: &mut Ctx) {
        let (loaded, load) = Settings::load(&mut ctx.storage);
        ctx.input.set_actions(settings::bindings_for(&loaded.controls));
        ctx.ui.set_scale(loaded.ui_scale);
        ctx.world.insert_resource(loaded);
        ctx.world.insert_resource(settings::SettingsFile {
            load,
            save_failed: false,
        });
    }
}

/// Layer 2: the rules, on the fixed tick, and the resources they own. The
/// game starts on the title screen with the progress loaded from `ctx.storage`.
pub struct Rules;

impl Plugin for Rules {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::AfterPhysics, rules)
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.world.insert_resource(Round {
            collected: 0,
            time_left: ROUND_SECONDS,
            number: 0,
            new_best: false,
        });
        let (progress, load) = Progress::load(&mut ctx.storage);
        ctx.world.insert_resource(progress);
        ctx.world.insert_resource(SaveFile {
            load,
            save_failed: false,
        });
        ctx.world.insert_resource(Screen::Title);
    }
}

/// Layer 3: the pictures, the text and the sound effects. The sprites
/// themselves are drawn by the engine's built-in sprite system.
pub struct Drawing;

impl Plugin for Drawing {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::Frame, hud)
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.gfx.clear(Color::hex(0x14181f));
        let disc = disc(ctx);
        let coin = ctx.gfx.load_texture(COIN_ASSET);
        ctx.world.insert_resource(Art { disc, coin });
        let sounds = Sounds {
            pickup: tone(ctx, 880.0, 1320.0, 0.10),
            win: tone(ctx, 523.0, 1047.0, 0.45),
            lose: tone(ctx, 330.0, 140.0, 0.45),
            select: tone(ctx, 660.0, 660.0, 0.05),
        };
        ctx.world.insert_resource(sounds);
    }
}
// ANCHOR_END: plugins

/// A short sine tone gliding from `from` to `to` Hz, fading out: a sound
/// effect made in code, so the game needs no sound files.
fn tone(ctx: &mut Ctx, from: f32, to: f32, seconds: f32) -> Sound {
    const RATE: u32 = 44_100;
    let count = (RATE as f32 * seconds) as usize;
    let mut phase = 0.0f32;
    let samples: Vec<f32> = (0..count)
        .map(|i| {
            let t = i as f32 / count as f32;
            phase += (from + (to - from) * t) * std::f32::consts::TAU / RATE as f32;
            phase.sin() * (1.0 - t) * 0.25
        })
        .collect();
    ctx.audio.create(RATE, &samples)
}

/// Plays one of the sound effects at the saved volume. Silent without audio.
fn sfx(ctx: &mut Ctx, pick: fn(&Sounds) -> Sound) {
    let sound = pick(ctx.world.resource::<Sounds>());
    let volume = ctx.world.resource::<Settings>().volume;
    if volume > 0.0 {
        ctx.audio.play_ex(
            sound,
            SoundParams {
                volume,
                ..Default::default()
            },
        );
    }
}

/// A white 32x32 disc, generated here so the game needs no image files. Sprites
/// tint and scale it into the player and the coins.
fn disc(ctx: &mut Ctx) -> Texture {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = vec2(x as f32 + 0.5, y as f32 + 0.5).distance(Vec2::splat(SIZE as f32 / 2.0));
            let alpha = (SIZE as f32 / 2.0 - d).clamp(0.0, 1.0); // soft 1 px edge
            rgba.extend([255, 255, 255, (alpha * 255.0) as u8]);
        }
    }
    ctx.gfx.create_texture(SIZE, SIZE, &rgba)
}

/// A disc sprite with a radius in world units.
fn ball(art: Art, at: Vec2, radius: f32, color: Color) -> (Transform, Sprite) {
    let scale = Vec2::splat(radius / 16.0); // the disc is 16 units in radius
    (Transform::at(at).scaled(scale), Sprite::new(art.disc).with_color(color))
}

/// Keyboard and gamepad, bound to the same names: the default controls, see
/// [`settings::bindings_for`] for the ones a player rebound.
pub fn bindings() -> ActionMap {
    settings::bindings_for(&settings::Controls::default())
}

// ANCHOR: read_input
/// Frame system: named actions to [`Intent`]. Runs once per rendered frame.
pub fn read_input(ctx: &mut Ctx) {
    let act = *ctx.world.resource::<Act>();
    let direction = ctx.input.action_axis2(act.movement);
    let confirm = ctx.input.action_pressed(act.confirm);
    let restart = ctx.input.action_pressed(act.restart);
    let pause = ctx.input.action_pressed(act.pause);
    let back = ctx.input.action_pressed(act.back);
    let open_settings = ctx.input.action_pressed(act.settings);
    let intent = ctx.world.resource_mut::<Intent>();
    intent.direction = direction;
    intent.confirm |= confirm;
    intent.restart |= restart;
    intent.pause |= pause;
    intent.back |= back;
    intent.settings |= open_settings;
}
// ANCHOR_END: read_input

/// Starts the next round: clears the field, then places the player in the
/// middle and the coins at random spots. The spots come from the context's
/// seeded random numbers, so a given seed and round number always give one layout.
pub fn new_round(ctx: &mut Ctx, number: u32) {
    ctx.world.clear();
    let art = *ctx.world.resource::<Art>();
    let mut rng = ctx.random().stream(u64::from(number));
    let (transform, sprite) = ball(art, ARENA / 2.0, PLAYER_RADIUS, Color::hex(0x3a86ff));
    ctx.world.spawn((transform, sprite, Player));
    for _ in 0..COINS {
        let at = vec2(rng.range(40.0, ARENA.x - 40.0), rng.range(40.0, ARENA.y - 40.0));
        let scale = Vec2::splat(COIN_RADIUS / 16.0); // the coin picture is 16 units in radius
        ctx.world
            .spawn((Transform::at(at).scaled(scale), Sprite::new(art.coin), Coin));
    }
    ctx.world.insert_resource(Round {
        collected: 0,
        time_left: ROUND_SECONDS,
        number,
        new_best: false,
    });
    ctx.world.insert_resource(Screen::Playing);
}

/// Back to the title screen: the field is emptied, the saved progress stays.
fn to_title(ctx: &mut Ctx) {
    ctx.world.clear();
    ctx.world.insert_resource(Screen::Title);
}

/// Sim system: the whole game, one fixed step at a time. What a button does
/// depends on the screen:
///
/// | Screen | confirm | restart | pause | back | settings |
/// |---|---|---|---|---|---|
/// | Title | play | | quit | quit | open settings |
/// | Playing | | new round | pause | | |
/// | Paused | resume | new round | resume | title | |
/// | Result | play again | play again | title | title | |
/// | Settings | | | | | |
///
/// The settings screen is the retained UI's: only `close_settings` (its Back
/// button or Escape) leaves it, see `settings::settings_ui`.
pub fn rules(ctx: &mut Ctx) {
    let Intent {
        direction,
        confirm,
        restart,
        pause,
        back,
        settings,
        close_settings,
    } = std::mem::take(ctx.world.resource_mut::<Intent>());
    // `take` cleared the latches; keep the held direction for the next tick.
    ctx.world.resource_mut::<Intent>().direction = direction;

    let next_round = ctx.world.resource::<Round>().number + 1;
    match *ctx.world.resource::<Screen>() {
        Screen::Title => {
            if confirm {
                sfx(ctx, |s| s.select);
                new_round(ctx, next_round);
            } else if settings {
                sfx(ctx, |s| s.select);
                ctx.world.insert_resource(Screen::Settings);
            } else if pause || back {
                ctx.quit();
            }
        }
        Screen::Settings => {
            if close_settings {
                settings::leave_settings(ctx);
            }
        }
        Screen::Playing => {
            if pause {
                ctx.world.insert_resource(Screen::Paused);
            } else if restart {
                new_round(ctx, next_round);
            } else {
                play(ctx, direction);
            }
        }
        Screen::Paused => {
            if back {
                to_title(ctx);
            } else if restart {
                new_round(ctx, next_round);
            } else if pause || confirm {
                ctx.world.insert_resource(Screen::Playing);
            }
        }
        Screen::Result(_) => {
            if confirm || restart {
                new_round(ctx, next_round);
            } else if pause || back {
                to_title(ctx);
            }
        }
    }
}

/// One tick of a running round: move, pick up coins, run the clock, end the
/// round when it is won or the time is up.
fn play(ctx: &mut Ctx, direction: Vec2) {
    // Move, clamped to the arena. A diagonal is not faster than a straight line.
    let step = direction.clamp_length_max(1.0) * SPEED * ctx.time.dt();
    let mut player = Vec2::ZERO;
    for (t, _) in ctx.world.query::<(&mut Transform, &Player)>().iter() {
        t.position = (t.position + step).clamp(Vec2::ZERO, ARENA);
        player = t.position;
    }

    // Pick up the coins the player touches.
    let reach = PLAYER_RADIUS + COIN_RADIUS;
    let mut touched = Vec::new(); // empty Vecs don't allocate
    for (e, t, _) in ctx.world.query::<(Entity, &Transform, &Coin)>().iter() {
        if t.position.distance(player) <= reach {
            touched.push(e);
        }
    }
    for e in &touched {
        ctx.world.despawn(*e);
    }
    if !touched.is_empty() {
        sfx(ctx, |s| s.pickup);
    }

    let dt = ctx.time.dt();
    let round = ctx.world.resource_mut::<Round>();
    round.collected += touched.len();
    round.time_left = (round.time_left - dt).max(0.0);
    if round.collected >= COINS {
        finish_round(ctx, Outcome::Won);
    } else if round.time_left <= 0.0 {
        finish_round(ctx, Outcome::Lost);
    }
}

/// Ends the round: records the result in the saved progress (and the file)
/// and shows the result screen.
fn finish_round(ctx: &mut Ctx, outcome: Outcome) {
    let (collected, elapsed) = {
        let round = ctx.world.resource::<Round>();
        (round.collected, ROUND_SECONDS - round.time_left)
    };
    let new_best = ctx.world.resource_mut::<Progress>().record(outcome, collected, elapsed);
    let progress = ctx.world.resource::<Progress>().clone();
    let failed = ctx.storage.is_available() && progress.save(&mut ctx.storage).is_err();
    ctx.world.resource_mut::<SaveFile>().save_failed = failed;
    ctx.world.resource_mut::<Round>().new_best = new_best;
    match outcome {
        Outcome::Won => sfx(ctx, |s| s.win),
        Outcome::Lost => sfx(ctx, |s| s.lose),
    }
    ctx.world.insert_resource(Screen::Result(outcome));
}

// ---- Saved progress ---------------------------------------------------------

/// Layout version of the save file. A file with another version is treated as
/// unreadable (and kept as a `.bad` copy), never half-understood.
pub const SAVE_VERSION: u32 = 1;

/// The progress file's name in `ctx.storage`.
pub const PROGRESS_FILE: &str = "progress.json";

/// What survives between runs: the best results over all rounds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub version: u32,
    /// Fastest win, in seconds into the round.
    pub best_time: Option<f32>,
    /// Most coins in one round, won or not (the high score).
    pub best_coins: usize,
    pub rounds_played: u32,
    pub rounds_won: u32,
}

/// How the saved progress was found at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    /// There was no file (first run), or saving is off (no app id).
    Fresh,
    /// The file was read.
    Loaded,
    /// The file was damaged or from another version: the game started from
    /// zero and kept the old file next to it as `<name>.bad`.
    Recovered,
}

/// What happened to the progress file. A resource.
#[derive(Debug, Clone)]
pub struct SaveFile {
    pub load: LoadStatus,
    /// The last attempt to write failed (a read-only disk, say). The game goes on.
    pub save_failed: bool,
}

/// Reads the JSON file `name` from `storage` into a `T`. A missing file (or no
/// storage) is a first run (`Fresh`, the default value); a file that can't be
/// read, doesn't parse or fails `valid` is recovered from (`Recovered`, the
/// default value), and its bytes are copied to `<name>.bad` for inspection.
/// Shared by the progress and the settings files.
fn load_json<T: Default + serde::de::DeserializeOwned>(
    storage: &mut Storage,
    name: &str,
    valid: fn(&T) -> bool,
) -> (T, LoadStatus) {
    if !storage.is_available() {
        return (T::default(), LoadStatus::Fresh);
    }
    let bytes = match storage.read(name) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return (T::default(), LoadStatus::Fresh),
        Err(_) => return (T::default(), LoadStatus::Recovered),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) if valid(&value) => (value, LoadStatus::Loaded),
        _ => {
            let _ = storage.write(&format!("{name}.bad"), &bytes); // best effort
            (T::default(), LoadStatus::Recovered)
        }
    }
}

impl Progress {
    /// Reads [`PROGRESS_FILE`]. A missing file is a first run; a file that
    /// can't be read, doesn't parse or holds impossible numbers is recovered
    /// from: zeroed progress, and the old bytes copied to `progress.json.bad`
    /// for inspection.
    pub fn load(storage: &mut Storage) -> (Progress, LoadStatus) {
        load_json(storage, PROGRESS_FILE, Progress::is_valid)
    }

    /// Writes [`PROGRESS_FILE`]. `ctx.storage` writes safely: a crash in the
    /// middle leaves the old file, not half a new one.
    pub fn save(&self, storage: &mut Storage) -> Result<(), Error> {
        storage.write_json(
            PROGRESS_FILE,
            &Progress {
                version: SAVE_VERSION,
                ..self.clone()
            },
        )
    }

    /// Whether the numbers could have come from this game.
    fn is_valid(&self) -> bool {
        self.version == SAVE_VERSION
            && self.best_coins <= COINS
            && self.rounds_won <= self.rounds_played
            && self
                .best_time
                .is_none_or(|t| t.is_finite() && t > 0.0 && t <= ROUND_SECONDS)
    }

    /// Counts a finished round; true when it set a new best time or high score.
    pub fn record(&mut self, outcome: Outcome, collected: usize, elapsed: f32) -> bool {
        self.version = SAVE_VERSION;
        self.rounds_played += 1;
        let mut new_best = collected > self.best_coins;
        self.best_coins = self.best_coins.max(collected);
        if outcome == Outcome::Won {
            self.rounds_won += 1;
            // A win can't take longer than the round or no time at all.
            let elapsed = elapsed.clamp(f32::EPSILON, ROUND_SECONDS);
            if self.best_time.is_none_or(|best| elapsed < best) {
                self.best_time = Some(elapsed);
                new_best = true;
            }
        }
        new_best
    }
}

// ---- Drawing ----------------------------------------------------------------

/// One line of text on a screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub size: f32,
    pub color: Color,
}

fn line(text: impl Into<String>, size: f32, color: Color) -> Line {
    Line {
        text: text.into(),
        size,
        color,
    }
}

/// The text of the current screen, top to bottom. Pure, so a test can check
/// what the player would read without a window.
pub fn screen_lines(world: &World) -> Vec<Line> {
    let gold = Color::hex(0xffbe0b);
    let green = Color::hex(0x8ac926);
    let red = Color::hex(0xff595e);
    let progress = world.resource::<Progress>();
    let save = world.resource::<SaveFile>();
    let settings = world.resource::<Settings>();
    let key = |control| settings.controls.key_name(control);
    use settings::Control as C;
    let best = if progress.rounds_played == 0 {
        "No rounds played yet".to_string()
    } else {
        let time = progress.best_time.map_or("none".to_string(), |t| format!("{t:.1} s"));
        format!(
            "Best time {time}   Most coins {}/{COINS}   Won {} of {}",
            progress.best_coins, progress.rounds_won, progress.rounds_played
        )
    };
    let mut lines = match *world.resource::<Screen>() {
        // The settings screen is the retained UI's; a round shows only the HUD.
        Screen::Playing | Screen::Settings => return Vec::new(),
        Screen::Title => {
            let mut lines = vec![
                line("COIN DASH", 56.0, gold),
                line(
                    format!("Collect {COINS} coins in {ROUND_SECONDS:.0} seconds"),
                    20.0,
                    Color::WHITE,
                ),
                line(best, 20.0, Color::WHITE),
            ];
            if save.load == LoadStatus::Recovered {
                lines.push(line("Saved progress was unreadable and was reset", 20.0, red));
            }
            if world.resource::<settings::SettingsFile>().load == LoadStatus::Recovered {
                lines.push(line("Saved settings were unreadable and were reset", 20.0, red));
            }
            // A browser tab can't be closed by the game, so the web build has no quit.
            let quit = if cfg!(target_arch = "wasm32") {
                String::new()
            } else {
                format!("    {}: quit", key(C::Pause))
            };
            lines.push(line(
                format!("{}: play    {}: settings{quit}", key(C::Confirm), key(C::Settings)),
                20.0,
                Color::WHITE,
            ));
            lines
        }
        Screen::Paused => vec![
            line("Paused", 48.0, Color::WHITE),
            line(
                format!(
                    "{}: resume    {}: restart    {}: title",
                    key(C::Pause),
                    key(C::Restart),
                    key(C::Back)
                ),
                20.0,
                Color::WHITE,
            ),
        ],
        Screen::Result(outcome) => {
            let round = world.resource::<Round>();
            let mut lines = match outcome {
                Outcome::Won => vec![
                    line("You win!", 48.0, green),
                    line(
                        format!("Time {:.1} s", ROUND_SECONDS - round.time_left),
                        24.0,
                        Color::WHITE,
                    ),
                ],
                Outcome::Lost => vec![
                    line("Time's up", 48.0, red),
                    line(format!("Coins {}/{COINS}", round.collected), 24.0, Color::WHITE),
                ],
            };
            if round.new_best {
                lines.push(line("New best!", 24.0, gold));
            }
            lines.push(line(best, 20.0, Color::WHITE));
            lines.push(line(
                format!("{}: play again    {}: title", key(C::Confirm), key(C::Back)),
                20.0,
                Color::WHITE,
            ));
            lines
        }
    };
    if save.save_failed {
        lines.push(line("Progress could not be saved", 20.0, red));
    }
    lines
}

/// Frame system: the text on top of the sprites, at the player's UI scale.
pub fn hud(ctx: &mut Ctx) {
    let scale = ctx.world.resource::<Settings>().ui_scale;
    if matches!(*ctx.world.resource::<Screen>(), Screen::Playing | Screen::Paused) {
        let round = ctx.world.resource::<Round>().clone();
        let status = format!(
            "Round {}   Coins {}/{}   Time {:.1}",
            round.number, round.collected, COINS, round.time_left
        );
        ctx.gfx
            .draw_text(&status, vec2(12.0, 8.0) * scale, 20.0 * scale, Color::WHITE);
    }
    // The block of lines, centred on the arena.
    let lines = screen_lines(&ctx.world);
    let height: f32 = lines.iter().map(|l| l.size * scale * 1.5).sum();
    let mut y = (ARENA.y - height) / 2.0;
    for l in lines {
        let size = l.size * scale;
        let width = ctx.gfx.measure_text(&l.text, size).x;
        ctx.gfx
            .draw_text(&l.text, vec2((ARENA.x - width) / 2.0, y), size, l.color);
        y += size * 1.5;
    }
}
