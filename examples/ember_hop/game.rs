//! The game: rules, levels, input mapping and drawing. Shared with
//! `tests/ember_hop.rs`, which plays it without a window.
//!
//! Four layers, each a [`Plugin`]:
//!
//! 1. [`Controls`]: `read_input` (frame system) turns named actions from the
//!    keyboard and gamepad into an [`Intent`].
//! 2. [`Rules`]: `rules` (sim system, after the physics step) is the whole
//!    game. It reads only the `Intent`, the world and the physics results, so
//!    a test can write the `Intent` itself and get the same game every run.
//! 3. [`Presentation`]: the camera that follows the hero, the glows, the
//!    fade between levels and the text. None of it changes the game.
//! 4. [`Music`]: the music, started once.
//!
//! What the engine does for it: levels are Tiled maps (`assets/ember_hop/levels`)
//! spawned with `ctx.spawn_level_with`, which makes the tile layers, the
//! colliders of the solid tiles and an entity per object; objects of the
//! classes `Spark`, `Drip`, `Checkpoint` and `Exit` are prefabs
//! (`assets/ember_hop/prefabs`, the way out is a prefab with children). The
//! ground layer holds plain rock and is autotiled at spawn (`AutoTile::four`).
//! The hero is a dynamic physics body; sparks, drips, hazards, braziers and the
//! way out are sensors whose `CollisionStarted` events are the game's rules.
//! The title is a scene file (`ctx.load_scene`). Lighting a brazier saves the
//! world in a save slot (`ctx.storage.save_world`): the hero and the prefab
//! objects with their state and animations, not the level, which the engine
//! leaves out of saves. Falling, spikes, water or a drip load it again
//! (`ctx.load_world`) and spawn the level around it. Progress (best time, most
//! sparks) is a small JSON file in `ctx.storage`.

// The test file includes this module but needs only part of it.
#![allow(dead_code)]

// Next to this file, whichever crate includes it (`main.rs` or `tests/ember_hop.rs`).
#[path = "smoke.rs"]
pub mod smoke;

use ponko::collision::CollisionStarted;
use ponko::d2::{
    Collider, ParticleEmitter, ParticleSettings, ParticleShape, QueryFilter, RigidBody, Transform, Velocity,
};
use ponko::input::{Gamepad, GamepadAxis, GamepadButton};
use ponko::particles::{ParticleCurve, ParticleGradient};
use ponko::prelude::*;
use ponko::render::BlendMode;
use ponko::sheet::{Animation, SpriteSheet};
use ponko::storage::Storage;
use ponko::tilemap::{AutoTile, LevelLayer, LevelObject, LevelOptions, LevelRoot, TileMap};
use serde::{Deserialize, Serialize};

// ---- tuning ---------------------------------------------------------------------

/// The window, in pixels. The camera zooms in [`ZOOM`] times, so 320 x 180
/// world pixels (20 x 11 tiles) are on screen.
pub const WINDOW: Vec2 = vec2(960.0, 540.0);
pub const ZOOM: f32 = 3.0;
/// Pulling down, pixels per second squared.
pub const GRAVITY: f32 = 900.0;
/// Running speed, pixels per second.
pub const RUN_SPEED: f32 = 92.0;
/// Upward speed of a jump: about 3.3 tiles high, 4 tiles far at full run.
pub const JUMP_SPEED: f32 = 310.0;
/// Letting go of jump early keeps this much of the upward speed.
pub const JUMP_CUT: f32 = 0.45;
/// A jump still works this long after running off a ledge, and a press this
/// long before landing jumps on landing.
pub const COYOTE: f32 = 0.1;
pub const JUMP_BUFFER: f32 = 0.12;
/// The flare: drips this close burn up.
pub const FLARE_RADIUS: f32 = 26.0;
pub const FLARE_COOLDOWN: f32 = 0.6;
/// Landing on a drip bounces the hero up this fast.
pub const STOMP_BOUNCE: f32 = 240.0;
/// Seconds between losing a life and starting again at the last brazier.
pub const DEATH_DELAY: f32 = 0.8;
/// Seconds of the fade out (and of the fade in) between levels.
pub const FADE: f32 = 0.45;
/// The hero's collider: 10 x 14 pixels, its bottom 8 pixels below the centre.
pub const HERO_SIZE: Vec2 = vec2(10.0, 14.0);
pub const HERO_OFFSET: Vec2 = vec2(0.0, 1.0);

// ---- files -------------------------------------------------------------------------

/// The levels, in order.
pub const LEVELS: [&str; 2] = ["ember_hop/levels/hollow.tmx", "ember_hop/levels/deep.tmx"];
pub const SPARK_PREFAB: &str = "ember_hop/prefabs/spark.json";
pub const DRIP_PREFAB: &str = "ember_hop/prefabs/drip.json";
pub const CHECKPOINT_PREFAB: &str = "ember_hop/prefabs/checkpoint.json";
pub const EXIT_PREFAB: &str = "ember_hop/prefabs/exit.json";
pub const TITLE_SCENE: &str = "ember_hop/scenes/title.json";
pub const MUSIC: &str = "ember_hop/music/cave.ogg";

/// The name of the game's folder in the player's data folder.
pub const APP_ID: &str = "ember-hop";
/// The save slot the run is kept in: written at the start of each level and
/// at every brazier, read when the hero is lost and by "continue".
pub const RUN_SLOT: &str = "run";
/// Best results, kept beside the save slot.
pub const PROGRESS_FILE: &str = "progress.json";
/// Random seed: the same seed and inputs always give the same game.
pub const SEED: u64 = 374;

// ---- components ------------------------------------------------------------------
// The reflected ones (serde) are in the save slot and in the prefab files, with
// the engine's (sprites, animations, bodies).

/// What the hero shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pose {
    #[default]
    Idle,
    Run,
    Jump,
    Fall,
}

/// The hero, an ember spirit. Its position is its `Transform`, its speed its
/// `Velocity`; this is the rest of its state.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Player {
    /// 1 right, -1 left.
    pub facing: f32,
    /// Standing on something (the last tick's two rays down).
    pub grounded: bool,
    /// Seconds left to jump after leaving the ground.
    pub coyote: f32,
    /// Seconds left of a jump pressed in the air.
    pub buffer: f32,
    /// Seconds until the next flare.
    pub flare: f32,
    pub pose: Pose,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            facing: 1.0,
            grounded: false,
            coyote: 0.0,
            buffer: 0.0,
            flare: 0.0,
            pose: Pose::Idle,
        }
    }
}

/// A spark to collect (prefab `spark.json`).
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Spark {
    pub value: u32,
}

impl Default for Spark {
    fn default() -> Self {
        Self { value: 1 }
    }
}

/// A drip: a slime that crawls along its ledge, turning at walls and edges
/// (prefab `drip.json`).
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Drip {
    /// Pixels per second.
    pub speed: f32,
    /// 1 right, -1 left.
    pub heading: f32,
}

impl Default for Drip {
    fn default() -> Self {
        Self {
            speed: 28.0,
            heading: 1.0,
        }
    }
}

/// A brazier: lighting it saves the run (prefab `checkpoint.json`).
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Checkpoint {
    pub lit: bool,
}

/// The way out of a level (the root of prefab `exit.json`).
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct Exit {}

/// Spikes and water: a sensor made from the level's `Hazard` rectangles.
#[derive(Component, Debug, Clone, Copy)]
pub struct Hazard;

/// A one-shot particle burst, removed when its time is up.
#[derive(Component, Debug, Clone, Copy)]
pub struct Burst {
    pub ttl: f32,
}

// ---- resources -------------------------------------------------------------------

/// The run so far. Reflected, so it is in the save slot with the world.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    /// Index into [`LEVELS`].
    pub level: usize,
    /// Sparks collected in the whole run.
    pub sparks: u32,
    /// Sparks collected in this level, and how many it has.
    pub level_sparks: u32,
    pub level_total: u32,
    /// Times the hero was lost (kept across loads of the save slot).
    pub deaths: u32,
    /// Seconds played (kept across loads of the save slot).
    pub time: f32,
}

/// Which screen the game is on. Only `rules` changes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Screen {
    /// The title scene: "new run", "continue".
    Title,
    Playing,
    /// Nothing moves, the physics step included.
    Paused,
    /// The hero was lost; the run starts again at the last brazier after
    /// [`DEATH_DELAY`].
    Dying {
        t: f32,
    },
    /// Fading out of a level and into the next one (`to`), or to the end
    /// (`None`). The level is swapped halfway.
    Transition {
        to: Option<usize>,
        t: f32,
        swapped: bool,
    },
    /// Every level done: the results, until any key or button.
    Finished,
}

/// What the player asked for since the last simulation tick. The rules read
/// this, never the keyboard, so tests and the smoke run write it directly.
/// The button presses are latched until the rules take them.
#[derive(Debug, Default, Clone, Copy)]
pub struct Intent {
    /// -1 left .. 1 right.
    pub move_x: f32,
    /// The jump button is held (letting go early makes a short hop).
    pub jump_held: bool,
    pub jump: bool,
    pub flare: bool,
    /// Pause or resume; on the title screen, quit.
    pub pause: bool,
    /// Start a new run, resume.
    pub confirm: bool,
    /// Continue the saved run.
    pub resume_run: bool,
    /// Back to the title.
    pub back: bool,
    /// Any key or button at all (the end screen).
    pub any: bool,
}

/// The level being played.
#[derive(Debug, Clone, Default)]
pub struct CurrentLevel {
    pub root: Option<Entity>,
    /// Width and height in pixels.
    pub size: Vec2,
    pub title: String,
}

/// A line shown for a moment ("Saved", "Couldn't save: ...").
#[derive(Debug, Clone, Default)]
pub struct Notice {
    pub text: String,
    pub seconds: f32,
}

/// Counts of what happened, for tests and the smoke report. Not saved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tally {
    pub jumps: u32,
    pub flares: u32,
    pub stomps: u32,
    pub burned: u32,
    pub checkpoints: u32,
    pub saves: u32,
    pub loads: u32,
    pub levels_cleared: u32,
}

/// The pictures and their frames, loaded once.
#[derive(Clone, Copy)]
pub struct Art {
    pub hero: SpriteSheet,
    pub spark: SpriteSheet,
    pub drip: SpriteSheet,
    pub brazier: SpriteSheet,
    pub lantern: SpriteSheet,
    pub ember: Texture,
    pub glow: Texture,
}

/// The sound files, loaded once; the music is streamed.
#[derive(Clone, Copy)]
pub struct Sounds {
    pub jump: Sound,
    pub spark: Sound,
    pub flare: Sound,
    pub stomp: Sound,
    pub hurt: Sound,
    pub checkpoint: Sound,
    pub clear: Sound,
    pub select: Sound,
    pub music: Sound,
}

pub const HERO_PNG: &str = "ember_hop/sprites/hero.png";

// ---- the game --------------------------------------------------------------------

/// The game without a window, ready for `run` or `run_headless`, with its
/// files in the player's data folder (`PONKO_DATA_DIR` moves it).
pub fn build() -> Game {
    Game::new("Ember Hop", WINDOW.x as u32, WINDOW.y as u32)
        .seed(SEED)
        .app_id(APP_ID)
        .plugin(EmberHop)
}

/// The whole game: physics and its layers.
pub struct EmberHop;

impl Plugin for EmberHop {
    fn build(&self, game: Game) -> Game {
        game.plugin(Controls).plugin(Rules).plugin(Presentation).plugin(Music)
    }
}

// ANCHOR: controls
actions! {
    /// The action names, resolved once at startup.
    pub struct Act {
        movement = "move",
        dpad_left = "dpad_left",
        dpad_right = "dpad_right",
        jump = "jump",
        flare = "flare",
        pause = "pause",
        confirm = "confirm",
        resume_run = "continue",
        back = "back",
    }
}

/// Keyboard and gamepad, bound to the same names.
pub fn bindings() -> ActionMap {
    let pad = Gamepad::from_slot(0).expect("slot 0 exists");
    let keys = |list: &[Key]| list.iter().map(|&k| ButtonBinding::Key(k)).collect::<Vec<_>>();
    let with_pad = |mut list: Vec<ButtonBinding>, buttons: &[GamepadButton]| {
        list.extend(buttons.iter().map(|&button| ButtonBinding::Gamepad { pad, button }));
        Action::Button(list)
    };
    let mut map = ActionMap::default();
    map.set(
        "move",
        Action::Axis(vec![
            AxisBinding::Keys {
                negative: Key::A,
                positive: Key::D,
            },
            AxisBinding::Keys {
                negative: Key::Left,
                positive: Key::Right,
            },
            AxisBinding::Gamepad {
                pad,
                axis: GamepadAxis::LeftStickX,
                deadzone: 0.25,
            },
        ]),
    );
    map.set("dpad_left", with_pad(vec![], &[GamepadButton::DPadLeft]));
    map.set("dpad_right", with_pad(vec![], &[GamepadButton::DPadRight]));
    map.set(
        "jump",
        with_pad(
            keys(&[Key::Space, Key::Up, Key::W, Key::Z, Key::K]),
            &[GamepadButton::South],
        ),
    );
    map.set(
        "flare",
        with_pad(
            keys(&[Key::X, Key::J, Key::LeftShift]),
            &[GamepadButton::West, GamepadButton::RightBumper],
        ),
    );
    map.set("pause", with_pad(keys(&[Key::Escape, Key::P]), &[GamepadButton::Start]));
    map.set(
        "confirm",
        with_pad(keys(&[Key::Enter, Key::Space]), &[GamepadButton::South]),
    );
    map.set("continue", with_pad(keys(&[Key::C]), &[GamepadButton::North]));
    map.set(
        "back",
        with_pad(keys(&[Key::Q]), &[GamepadButton::East, GamepadButton::Select]),
    );
    map
}
// ANCHOR_END: controls

/// Layer 1: keys and gamepad to an [`Intent`], once per frame.
pub struct Controls;

impl Plugin for Controls {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::Frame, read_input)
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.input.set_actions(bindings());
        ctx.world.insert_resource(Act::resolve(ctx));
        ctx.world.insert_resource(Intent::default());
    }
}

// ANCHOR: read_input
/// Frame system: named actions to [`Intent`].
pub fn read_input(ctx: &mut Ctx) {
    let act = *ctx.world.resource::<Act>();
    let input = &ctx.input;
    let dpad =
        f32::from(u8::from(input.action_down(act.dpad_right))) - f32::from(u8::from(input.action_down(act.dpad_left)));
    let move_x = (input.action_axis(act.movement) + dpad).clamp(-1.0, 1.0);
    let jump_held = input.action_down(act.jump);
    let jump = input.action_pressed(act.jump);
    let flare = input.action_pressed(act.flare);
    let pause = input.action_pressed(act.pause);
    let confirm = input.action_pressed(act.confirm);
    let resume_run = input.action_pressed(act.resume_run);
    let back = input.action_pressed(act.back);
    // Whatever was pressed, bound or not: "press any key" on the end screen.
    let any = input.pressed_this_frame().next().is_some();
    let intent = ctx.world.resource_mut::<Intent>();
    intent.move_x = move_x;
    intent.jump_held = jump_held;
    intent.jump |= jump;
    intent.flare |= flare;
    intent.pause |= pause;
    intent.confirm |= confirm;
    intent.resume_run |= resume_run;
    intent.back |= back;
    intent.any |= any;
}
// ANCHOR_END: read_input

/// Layer 2: the rules, the levels and the physics step (held while paused).
pub struct Rules;

impl Plugin for Rules {
    fn build(&self, game: Game) -> Game {
        // The physics step runs only while something should move: not on the
        // title, the pause or the end screen.
        game.physics(vec2(0.0, GRAVITY))
            .add(Phase::BeforePhysics, hold_physics)
            .sim_system(rules)
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.world.reflect::<Player>();
        ctx.world.reflect::<Spark>();
        ctx.world.reflect::<Drip>();
        ctx.world.reflect::<Checkpoint>();
        ctx.world.reflect::<Exit>();
        ctx.world.reflect_resource::<Run>();
        let art = load_art(ctx);
        ctx.world.insert_resource(art);
        let sounds = load_sounds(ctx);
        ctx.world.insert_resource(sounds);
        ctx.world.insert_resource(Run::default());
        ctx.world.insert_resource(CurrentLevel::default());
        ctx.world.insert_resource(Notice::default());
        ctx.world.insert_resource(Tally::default());
        let (progress, status) = Progress::load(&mut ctx.storage);
        ctx.world.insert_resource(progress);
        ctx.world.insert_resource(status);
        ctx.world.insert_resource(Screen::Title);
        to_title(ctx);
    }
}

/// Holds the physics step while the game is paused or between runs; the
/// rules keep running, so they see the press that resumes.
pub fn hold_physics(ctx: &mut Ctx) {
    let moving = matches!(
        *ctx.world.resource::<Screen>(),
        Screen::Playing | Screen::Dying { .. } | Screen::Transition { .. }
    );
    ctx.physics.set_paused(!moving);
}

fn load_art(ctx: &mut Ctx) -> Art {
    let sheet = |ctx: &mut Ctx, path: &str, w, h| {
        let texture = ctx.gfx.load_texture(path);
        ctx.gfx.set_texture_filter(texture, TextureFilter::Nearest);
        SpriteSheet::new(texture, w, h)
    };
    let art = Art {
        hero: sheet(ctx, HERO_PNG, 16, 16),
        spark: sheet(ctx, "ember_hop/sprites/spark.png", 16, 16),
        drip: sheet(ctx, "ember_hop/sprites/drip.png", 16, 16),
        brazier: sheet(ctx, "ember_hop/sprites/brazier.png", 16, 16),
        lantern: sheet(ctx, "ember_hop/sprites/lantern.png", 16, 16),
        ember: ctx.gfx.load_texture("ember_hop/sprites/ember.png"),
        glow: ctx.gfx.load_texture("ember_hop/sprites/glow.png"),
    };
    let post = ctx.gfx.load_texture("ember_hop/sprites/post.png");
    ctx.gfx.set_texture_filter(post, TextureFilter::Nearest);
    art
}

fn load_sounds(ctx: &mut Ctx) -> Sounds {
    let mut sound = |name: &str| ctx.audio.load(&format!("ember_hop/sounds/{name}.wav"));
    Sounds {
        jump: sound("jump"),
        spark: sound("spark"),
        flare: sound("flare"),
        stomp: sound("stomp"),
        hurt: sound("hurt"),
        checkpoint: sound("checkpoint"),
        clear: sound("clear"),
        select: sound("select"),
        // Music is long: streamed from the file rather than decoded whole.
        music: ctx.audio.load_stream(MUSIC),
    }
}

/// Plays a sound effect.
fn sfx(ctx: &mut Ctx, pick: fn(&Sounds) -> Sound) {
    let sound = pick(ctx.world.resource::<Sounds>());
    ctx.audio.play_ex(
        sound,
        SoundParams {
            volume: 0.6,
            ..SoundParams::default()
        },
    );
}

fn notice(ctx: &mut Ctx, text: impl Into<String>) {
    *ctx.world.resource_mut::<Notice>() = Notice {
        text: text.into(),
        seconds: 2.0,
    };
}

// ---- screens -------------------------------------------------------------------

/// To the title: the title scene replaces the world.
pub fn to_title(ctx: &mut Ctx) {
    if let Err(e) = ctx.load_scene(TITLE_SCENE) {
        // The title is decoration: say why it is missing and go on.
        eprintln!("ember_hop: {e}");
        ctx.world.clear();
    }
    ctx.camera.zoom = ZOOM;
    ctx.camera.look_at(vec2(160.0, 90.0));
    *ctx.world.resource_mut::<Screen>() = Screen::Title;
}

/// A saved run to continue.
pub fn has_saved_run(storage: &Storage) -> bool {
    storage
        .world_slots()
        .is_ok_and(|slots| slots.iter().any(|s| s == RUN_SLOT))
}

/// Starts a new run on the first level.
pub fn new_run(ctx: &mut Ctx) {
    *ctx.world.resource_mut::<Run>() = Run::default();
    enter_level(ctx, 0);
}

/// Continues the saved run: the world as it was saved, the level around it.
/// `false` (and a notice) if there is none or it can't be read.
pub fn continue_run(ctx: &mut Ctx) -> bool {
    match ctx.load_world(RUN_SLOT) {
        Ok(()) => {
            ctx.world.resource_mut::<Tally>().loads += 1;
            rehydrate(ctx);
            *ctx.world.resource_mut::<Screen>() = Screen::Playing;
            true
        }
        Err(e) => {
            notice(ctx, format!("No run to continue ({:?})", e.kind()));
            false
        }
    }
}

/// Starts level `index` from its file and saves the run.
pub fn enter_level(ctx: &mut Ctx, index: usize) {
    ctx.world.clear();
    {
        let run = ctx.world.resource_mut::<Run>();
        run.level = index;
        run.level_sparks = 0;
    }
    if let Err(e) = spawn_level(ctx, index, false) {
        // A level that doesn't load is a broken install: say which file.
        panic!("ember_hop: level {} doesn't load: {e}", LEVELS[index]);
    }
    let total = ctx.world.query::<&Spark>().iter().map(|s| s.value).sum::<u32>();
    ctx.world.resource_mut::<Run>().level_total = total;
    let start = ctx
        .world
        .query::<&LevelObject>()
        .iter()
        .find(|o| o.class == "Spawn")
        .map_or(vec2(40.0, 40.0), |o| o.position);
    spawn_hero(ctx, start);
    snap_camera(ctx);
    save_run(ctx);
    *ctx.world.resource_mut::<Screen>() = Screen::Playing;
}

/// The hero at `at`.
pub fn spawn_hero(ctx: &mut Ctx, at: Vec2) -> Entity {
    let art = *ctx.world.resource::<Art>();
    ctx.world.spawn((
        Transform::at(at),
        art.hero.sprite(0).with_z(0.5),
        Animation::new(art.hero, 0..2, 3.0),
        RigidBody {
            lock_rotation: true,
            ..RigidBody::dynamic()
        },
        Collider::rect(HERO_SIZE).offset(HERO_OFFSET).friction(0.0),
        Velocity::default(),
        Player::default(),
        ParticleEmitter::new(trail(art.ember)),
    ))
}

/// Spawns level `index` with its prefabs, autotiles its ground and makes
/// its hazards sensors. After loading a save (`loaded`), without the prefab
/// objects: the save has them, as they were.
pub fn spawn_level(ctx: &mut Ctx, index: usize, loaded: bool) -> Result<Entity, Error> {
    // ANCHOR: spawn_level
    let mut options = LevelOptions::default()
        .prefab("Spark", SPARK_PREFAB)
        .prefab("Drip", DRIP_PREFAB)
        .prefab("Checkpoint", CHECKPOINT_PREFAB)
        .prefab("Exit", EXIT_PREFAB);
    if loaded {
        options = options.without_saved_objects();
    }
    let root = ctx.spawn_level_with(LEVELS[index], Vec2::ZERO, options)?;

    // The ground layer is painted with one rock tile; the edges and the moss
    // on top are picked here from each cell's four neighbours.
    let rule = AutoTile::four(std::array::from_fn(|mask| mask as u16)).outside(true);
    ctx.world
        .query::<(&mut TileMap, &LevelLayer)>()
        .for_each(|(map, layer)| {
            if layer.name == "Ground" {
                let filled: Vec<bool> = map.tiles.iter().map(|&t| t != TileMap::EMPTY).collect();
                let width = map.size.x;
                map.autotile(&rule, |x, y| filled[(y * width + x) as usize]);
            }
        });
    // ANCHOR_END: spawn_level

    // Spikes and water: the level's Hazard rectangles become sensors.
    let hazards: Vec<(Entity, Vec2)> = ctx
        .world
        .query::<(Entity, &LevelObject)>()
        .iter()
        .filter(|(_, o)| o.class == "Hazard")
        .map(|(e, o)| (e, o.size))
        .collect();
    for (e, size) in hazards {
        ctx.world.insert(e, RigidBody::fixed());
        ctx.world.insert(e, Collider::rect(size).sensor());
        ctx.world.insert(e, Hazard);
    }

    // Pixel art: no smoothing on the tileset either.
    let sheets: Vec<Texture> = ctx.world.query::<&TileMap>().iter().map(|m| m.sheet.texture).collect();
    for texture in sheets {
        ctx.gfx.set_texture_filter(texture, TextureFilter::Nearest);
    }

    let (size, title) = {
        let level = ctx.world.get::<LevelRoot>(root).expect("a level root");
        let title = level.properties.str("title").unwrap_or("").to_string();
        let size = ctx
            .world
            .query::<(&TileMap, &LevelLayer)>()
            .iter()
            .find(|(_, l)| l.name == "Ground")
            .map_or(Vec2::ZERO, |(m, _)| m.size.as_vec2() * m.tile_size);
        (size, title)
    };
    *ctx.world.resource_mut::<CurrentLevel>() = CurrentLevel {
        root: Some(root),
        size,
        title,
    };
    Ok(root)
}

/// After `load_world`: the save holds the hero and the level's prefab objects
/// (sparks left, drips, braziers, the way out) with their animations; the
/// level around them comes from its file.
pub fn rehydrate(ctx: &mut Ctx) {
    let level = ctx.world.resource::<Run>().level.min(LEVELS.len() - 1);
    if let Err(e) = spawn_level(ctx, level, true) {
        panic!("ember_hop: level {} doesn't load: {e}", LEVELS[level]);
    }
    // A rule, not a repair: the hero starts again from rest.
    let heroes: Vec<Entity> = ctx.world.query::<(Entity, &Player)>().iter().map(|(e, _)| e).collect();
    for e in heroes {
        ctx.world.insert(e, Velocity::default());
    }
    snap_camera(ctx);
}

/// Saves the run in its slot. A failed write is shown, and play goes on.
pub fn save_run(ctx: &mut Ctx) {
    match ctx.storage.save_world(&ctx.world, RUN_SLOT) {
        Ok(()) => ctx.world.resource_mut::<Tally>().saves += 1,
        Err(e) if e.kind() == ErrorKind::Unsupported => {} // no storage: a demo without saves
        Err(e) => notice(ctx, format!("Couldn't save: {e}")),
    }
}

/// The hero is lost: a puff of embers, then the last save.
fn lose_hero(ctx: &mut Ctx, hero: Entity) {
    let at = ctx.world.get::<Transform>(hero).map_or(Vec2::ZERO, |t| t.position);
    ctx.world.despawn(hero);
    let art = *ctx.world.resource::<Art>();
    burst(ctx, at, puff(art.ember, Color::hex(0xff8a2a), 40, 120.0), 1.2);
    sfx(ctx, |s| s.hurt);
    *ctx.world.resource_mut::<Screen>() = Screen::Dying { t: 0.0 };
}

/// Back at the last save (the level's start or its last brazier); the
/// deaths and the clock keep counting.
pub fn respawn(ctx: &mut Ctx) {
    let (deaths, time) = {
        let run = ctx.world.resource::<Run>();
        (run.deaths + 1, run.time)
    };
    let loaded = ctx.load_world(RUN_SLOT).is_ok();
    if loaded {
        ctx.world.resource_mut::<Tally>().loads += 1;
        rehydrate(ctx);
    } else {
        let level = ctx.world.resource::<Run>().level;
        enter_level(ctx, level);
    }
    let run = ctx.world.resource_mut::<Run>();
    run.deaths = deaths;
    run.time = time;
    *ctx.world.resource_mut::<Screen>() = Screen::Playing;
}

/// Every level done: the results are recorded and the run's slot deleted.
fn finish_run(ctx: &mut Ctx) {
    let run = ctx.world.resource::<Run>().clone();
    let new_best = ctx.world.resource_mut::<Progress>().record(&run);
    ctx.world.resource_mut::<Progress>().last_new_best = new_best;
    let progress = ctx.world.resource::<Progress>().clone();
    if let Err(e) = progress.save(&mut ctx.storage)
        && e.kind() != ErrorKind::Unsupported
    {
        notice(ctx, format!("Couldn't save: {e}"));
    }
    let _ = ctx.storage.delete_world(RUN_SLOT);
    if ctx.load_scene(TITLE_SCENE).is_err() {
        ctx.world.clear();
    }
    ctx.camera.look_at(vec2(160.0, 90.0));
}

// ---- the rules -------------------------------------------------------------------

// ANCHOR: rules
/// Sim system, after the physics step: the whole game.
pub fn rules(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let intent = std::mem::take(ctx.world.resource_mut::<Intent>());
    // The latched presses are used up; what is held stays.
    {
        let held = ctx.world.resource_mut::<Intent>();
        held.move_x = intent.move_x;
        held.jump_held = intent.jump_held;
    }
    // This tick's collisions. Read every tick, so none are left over for a
    // later tick (after a load they would name the old world's entities).
    let hits: Vec<CollisionStarted> = ctx.events.read::<CollisionStarted>().to_vec();
    {
        let notice = ctx.world.resource_mut::<Notice>();
        notice.seconds = (notice.seconds - dt).max(0.0);
    }
    tick_bursts(ctx, dt);

    match *ctx.world.resource::<Screen>() {
        Screen::Title => {
            if intent.confirm {
                sfx(ctx, |s| s.select);
                new_run(ctx);
            } else if intent.resume_run {
                sfx(ctx, |s| s.select);
                continue_run(ctx);
            } else if intent.pause || intent.back {
                ctx.quit();
            }
        }
        Screen::Playing => {
            if intent.pause {
                *ctx.world.resource_mut::<Screen>() = Screen::Paused;
                return;
            }
            ctx.world.resource_mut::<Run>().time += dt;
            play(ctx, &intent, &hits);
        }
        Screen::Paused => {
            if intent.pause || intent.confirm {
                *ctx.world.resource_mut::<Screen>() = Screen::Playing;
            } else if intent.back {
                // The run stays in its slot: "continue" picks it up.
                to_title(ctx);
            }
        }
        Screen::Dying { t } => {
            let t = t + dt;
            if t >= DEATH_DELAY {
                respawn(ctx);
            } else {
                *ctx.world.resource_mut::<Screen>() = Screen::Dying { t };
            }
        }
        Screen::Transition { to, t, swapped } => {
            let t = t + dt;
            let mut swapped = swapped;
            if !swapped && t >= FADE {
                swapped = true;
                match to {
                    Some(level) => enter_level(ctx, level),
                    None => finish_run(ctx),
                }
            }
            *ctx.world.resource_mut::<Screen>() = if t >= 2.0 * FADE {
                if to.is_some() {
                    Screen::Playing
                } else {
                    Screen::Finished
                }
            } else {
                Screen::Transition { to, t, swapped }
            };
        }
        Screen::Finished => {
            if intent.any || intent.confirm || intent.back {
                to_title(ctx);
            }
        }
    }
}
// ANCHOR_END: rules

/// One tick of play: the hero, the drips, then what touched what.
fn play(ctx: &mut Ctx, intent: &Intent, hits: &[CollisionStarted]) {
    let dt = ctx.time.dt();
    let Some((hero, mut at)) = ctx
        .world
        .query::<(Entity, &Transform, &Player)>()
        .iter()
        .next()
        .map(|(e, t, _)| (e, t.position))
    else {
        return;
    };
    move_hero(ctx, hero, intent, dt);
    if intent.flare {
        flare(ctx, hero, at);
    }
    crawl_drips(ctx, dt);

    let art = *ctx.world.resource::<Art>();
    for hit in hits {
        let other = if hit.a == hero {
            hit.b
        } else if hit.b == hero {
            hit.a
        } else {
            continue;
        };
        if !ctx.world.is_alive(other) || !ctx.world.is_alive(hero) {
            continue;
        }
        at = ctx.world.get::<Transform>(hero).map_or(at, |t| t.position);
        if let Some(value) = ctx.world.get::<Spark>(other).map(|s| s.value) {
            let spot = ctx.world.get::<Transform>(other).map_or(at, |t| t.position);
            ctx.world.despawn(other);
            let run = ctx.world.resource_mut::<Run>();
            run.sparks += value;
            run.level_sparks += value;
            burst(ctx, spot, puff(art.ember, Color::hex(0xffd35a), 14, 60.0), 0.6);
            sfx(ctx, |s| s.spark);
        } else if ctx.world.get::<Checkpoint>(other).is_some_and(|c| !c.lit) {
            ctx.world.insert(other, Checkpoint { lit: true });
            ctx.world.insert(other, Animation::new(art.brazier, 1..3, 6.0));
            ctx.world.insert(other, ParticleEmitter::new(flames(art.ember)));
            ctx.world.resource_mut::<Tally>().checkpoints += 1;
            sfx(ctx, |s| s.checkpoint);
            save_run(ctx);
            notice(ctx, "The brazier keeps your progress");
        } else if ctx.world.has::<Drip>(other) {
            let drip_at = ctx.world.get::<Transform>(other).map_or(at, |t| t.position);
            let falling = ctx.world.get::<Velocity>(hero).is_some_and(|v| v.linear.y > 20.0);
            if falling && at.y < drip_at.y - 3.0 {
                burn_drip(ctx, other, art);
                if let Some(v) = ctx.world.get_mut::<Velocity>(hero) {
                    v.linear.y = -STOMP_BOUNCE;
                }
                ctx.world.resource_mut::<Tally>().stomps += 1;
                sfx(ctx, |s| s.stomp);
            } else {
                lose_hero(ctx, hero);
                return;
            }
        } else if ctx.world.has::<Hazard>(other) {
            lose_hero(ctx, hero);
            return;
        } else if ctx.world.has::<Exit>(other) {
            level_cleared(ctx);
            return;
        }
    }
    // Fell out of the level.
    let bottom = ctx.world.resource::<CurrentLevel>().size.y;
    if bottom > 0.0 && at.y > bottom + 32.0 {
        lose_hero(ctx, hero);
        return;
    }
    pose_hero(ctx, hero, art);
}

/// Runs, jumps (with a little forgiveness both ways) and faces where it goes.
fn move_hero(ctx: &mut Ctx, hero: Entity, intent: &Intent, dt: f32) {
    let at = ctx.world.get::<Transform>(hero).map_or(Vec2::ZERO, |t| t.position);
    let velocity = ctx.world.get::<Velocity>(hero).map_or(Vec2::ZERO, |v| v.linear);
    // Two short rays down from inside the feet: standing on something solid.
    let feet = at.y + HERO_OFFSET.y + HERO_SIZE.y / 2.0;
    let filter = QueryFilter::default().exclude(hero);
    let grounded = velocity.y >= -1.0
        && [-HERO_SIZE.x / 2.0 + 1.0, HERO_SIZE.x / 2.0 - 1.0].iter().any(|dx| {
            ctx.physics
                .cast_ray(vec2(at.x + dx, feet - 2.0), vec2(0.0, 1.0), 4.0, filter)
                .is_some()
        });
    let mut jumped = false;
    let mut v = velocity;
    {
        let Some(player) = ctx.world.get_mut::<Player>(hero) else {
            return;
        };
        player.grounded = grounded;
        player.coyote = if grounded {
            COYOTE
        } else {
            (player.coyote - dt).max(0.0)
        };
        player.buffer = if intent.jump {
            JUMP_BUFFER
        } else {
            (player.buffer - dt).max(0.0)
        };
        player.flare = (player.flare - dt).max(0.0);
        let target = intent.move_x * RUN_SPEED;
        let grip = if grounded { 20.0 } else { 9.0 };
        v.x += (target - v.x) * (grip * dt).min(1.0);
        if intent.move_x.abs() > 0.1 {
            player.facing = intent.move_x.signum();
        }
        if player.buffer > 0.0 && player.coyote > 0.0 {
            v.y = -JUMP_SPEED;
            player.buffer = 0.0;
            player.coyote = 0.0;
            jumped = true;
        } else if !intent.jump_held && v.y < 0.0 && !grounded {
            // Let go early: a short hop.
            v.y *= JUMP_CUT.powf(dt * 60.0 / 4.0);
        }
    }
    if let Some(vel) = ctx.world.get_mut::<Velocity>(hero) {
        vel.linear = v;
    }
    let facing = ctx.world.get::<Player>(hero).map_or(1.0, |p| p.facing);
    if let Some(t) = ctx.world.get_mut::<Transform>(hero) {
        t.scale.x = facing;
    }
    if jumped {
        ctx.world.resource_mut::<Tally>().jumps += 1;
        let art = *ctx.world.resource::<Art>();
        burst(
            ctx,
            vec2(at.x, feet),
            puff(art.ember, Color::hex(0x9a8fb0), 6, 30.0),
            0.5,
        );
        sfx(ctx, |s| s.jump);
    }
}

/// The flare: every drip within [`FLARE_RADIUS`] burns up.
fn flare(ctx: &mut Ctx, hero: Entity, at: Vec2) {
    let ready = ctx.world.get::<Player>(hero).is_some_and(|p| p.flare <= 0.0);
    if !ready {
        return;
    }
    if let Some(p) = ctx.world.get_mut::<Player>(hero) {
        p.flare = FLARE_COOLDOWN;
    }
    let mut near = Vec::new();
    ctx.physics.overlap_circle(
        at,
        FLARE_RADIUS,
        QueryFilter::default().with_sensors().exclude(hero),
        &mut near,
    );
    let art = *ctx.world.resource::<Art>();
    for e in near {
        if ctx.world.has::<Drip>(e) {
            burn_drip(ctx, e, art);
        }
    }
    ctx.world.resource_mut::<Tally>().flares += 1;
    burst(ctx, at, ring(art.ember), 0.6);
    sfx(ctx, |s| s.flare);
}

fn burn_drip(ctx: &mut Ctx, drip: Entity, art: Art) {
    let at = ctx.world.get::<Transform>(drip).map_or(Vec2::ZERO, |t| t.position);
    ctx.world.despawn(drip);
    ctx.world.resource_mut::<Tally>().burned += 1;
    burst(ctx, at, puff(art.ember, Color::hex(0x3fb59a), 18, 70.0), 0.8);
}

/// Drips crawl along their ledge and turn at walls and edges.
fn crawl_drips(ctx: &mut Ctx, dt: f32) {
    let drips: Vec<(Entity, Vec2, Drip)> = ctx
        .world
        .query::<(Entity, &Transform, &Drip)>()
        .iter()
        .map(|(e, t, d)| (e, t.position, d.clone()))
        .collect();
    for (e, at, mut drip) in drips {
        let filter = QueryFilter::default();
        let ahead = vec2(drip.heading, 0.0);
        let wall = ctx.physics.cast_ray(at + vec2(0.0, 4.0), ahead, 8.0, filter).is_some();
        let floor = ctx
            .physics
            .cast_ray(at + vec2(drip.heading * 7.0, 4.0), vec2(0.0, 1.0), 8.0, filter)
            .is_some();
        if wall || !floor {
            drip.heading = -drip.heading;
        }
        let step = drip.heading * drip.speed * dt;
        if let Some(t) = ctx.world.get_mut::<Transform>(e) {
            t.position.x += step;
            t.scale.x = -drip.heading;
        }
        ctx.world.insert(e, drip);
    }
}

/// The hero's animation follows what it does.
fn pose_hero(ctx: &mut Ctx, hero: Entity, art: Art) {
    let Some(v) = ctx.world.get::<Velocity>(hero).map(|v| v.linear) else {
        return;
    };
    let Some(player) = ctx.world.get::<Player>(hero).cloned() else {
        return;
    };
    let pose = if !player.grounded {
        if v.y < 0.0 { Pose::Jump } else { Pose::Fall }
    } else if v.x.abs() > 12.0 {
        Pose::Run
    } else {
        Pose::Idle
    };
    if pose != player.pose {
        let animation = match pose {
            Pose::Idle => Animation::new(art.hero, 0..2, 3.0),
            Pose::Run => Animation::new(art.hero, 2..6, 10.0),
            Pose::Jump => Animation::new(art.hero, 6..7, 1.0),
            Pose::Fall => Animation::new(art.hero, 7..8, 1.0),
        };
        ctx.world.insert(hero, animation);
        if let Some(p) = ctx.world.get_mut::<Player>(hero) {
            p.pose = pose;
        }
    }
}

/// The way out was reached: fade to the next level, or to the end.
fn level_cleared(ctx: &mut Ctx) {
    let level = ctx.world.resource::<Run>().level;
    let next = (level + 1 < LEVELS.len()).then_some(level + 1);
    ctx.world.resource_mut::<Tally>().levels_cleared += 1;
    sfx(ctx, |s| s.clear);
    *ctx.world.resource_mut::<Screen>() = Screen::Transition {
        to: next,
        t: 0.0,
        swapped: false,
    };
}

// ---- particles -------------------------------------------------------------------

fn ember_colors(color: Color) -> ParticleGradient {
    ParticleGradient::new(vec![
        (0.0, color.with_alpha(1.0)),
        (0.6, color.with_alpha(0.7)),
        (1.0, color.with_alpha(0.0)),
    ])
}

/// The hero's trail of embers, rising behind it.
fn trail(ember: Texture) -> ParticleSettings {
    ParticleSettings::default()
        .with_rate(14.0)
        .with_lifetime(0.4, 0.8)
        .with_speed(4.0, 12.0)
        .with_shape(ParticleShape::Circle { radius: 3.0 })
        .with_gravity(vec2(0.0, -30.0))
        .with_size(1.5, 3.0)
        .with_size_over_life(ParticleCurve::new(vec![(0.0, 1.0), (1.0, 0.3)]))
        .with_color_over_life(ember_colors(Color::hex(0xffa040)))
        .with_texture(ember)
        .with_blend(BlendMode::Additive)
        .with_z(0.4)
        .with_seed(1)
}

/// A lit brazier's flames.
fn flames(ember: Texture) -> ParticleSettings {
    ParticleSettings::default()
        .with_rate(20.0)
        .with_lifetime(0.3, 0.7)
        .with_speed(10.0, 25.0)
        .with_shape(ParticleShape::Rect {
            half_extents: vec2(3.0, 1.0),
        })
        .with_gravity(vec2(0.0, -40.0))
        .with_size(2.0, 4.0)
        .with_color_over_life(ember_colors(Color::hex(0xff7a1f)))
        .with_texture(ember)
        .with_blend(BlendMode::Additive)
        .with_z(0.6)
        .with_seed(2)
}

/// A one-shot puff of `count` particles.
fn puff(ember: Texture, color: Color, count: usize, speed: f32) -> ParticleSettings {
    ParticleSettings::default()
        .with_rate(0.0)
        .with_burst(count)
        .with_looping(false)
        .with_lifetime(0.3, 0.7)
        .with_speed(speed * 0.4, speed)
        .with_shape(ParticleShape::Circle { radius: 3.0 })
        .with_gravity(vec2(0.0, 120.0))
        .with_drag(2.0)
        .with_size(1.5, 3.5)
        .with_color_over_life(ember_colors(color))
        .with_texture(ember)
        .with_blend(BlendMode::Additive)
        .with_z(0.8)
}

/// The flare's ring of fire.
fn ring(ember: Texture) -> ParticleSettings {
    ParticleSettings::default()
        .with_rate(0.0)
        .with_burst(48)
        .with_looping(false)
        .with_lifetime(0.25, 0.4)
        .with_speed(70.0, 90.0)
        .with_shape(ParticleShape::Circle { radius: 4.0 })
        .with_gravity(Vec2::ZERO)
        .with_drag(1.0)
        .with_size(2.0, 4.0)
        .with_color_over_life(ember_colors(Color::hex(0xffc04a)))
        .with_texture(ember)
        .with_blend(BlendMode::Additive)
        .with_z(0.8)
}

/// A one-shot emitter at `at`, removed after `ttl` seconds.
fn burst(ctx: &mut Ctx, at: Vec2, settings: ParticleSettings, ttl: f32) {
    let seed = ctx.time.sim_tick();
    ctx.world.spawn((
        Transform::at(at),
        ParticleEmitter::new(settings.with_seed(seed)),
        Burst { ttl },
    ));
}

fn tick_bursts(ctx: &mut Ctx, dt: f32) {
    let mut done = Vec::new();
    ctx.world.query::<(Entity, &mut Burst)>().for_each(|(e, b)| {
        b.ttl -= dt;
        if b.ttl <= 0.0 {
            done.push(e);
        }
    });
    for e in done {
        ctx.world.despawn(e);
    }
}

// ---- layer 3: presentation ----------------------------------------------------------

/// The camera, the glows, the fade and the text.
pub struct Presentation;

impl Plugin for Presentation {
    fn build(&self, game: Game) -> Game {
        game.add(Phase::Late, follow_camera).add(Phase::Draw, draw_overlay)
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.gfx.clear(Color::hex(0x0d0b12));
    }
}

/// Where the camera wants to be: on the hero, a little ahead of it, inside
/// the level.
pub fn camera_target(ctx: &mut Ctx) -> Option<Vec2> {
    let (at, facing) = ctx
        .world
        .query::<(&Transform, &Player)>()
        .iter()
        .next()
        .map(|(t, p)| (t.position, p.facing))?;
    let size = ctx.world.resource::<CurrentLevel>().size;
    let half = ctx.camera.visible_rect().size() / 2.0;
    let want = at + vec2(facing * 24.0, -12.0);
    let clamp = |v: f32, lo: f32, hi: f32| if hi < lo { (lo + hi) / 2.0 } else { v.clamp(lo, hi) };
    Some(vec2(
        clamp(want.x, half.x, size.x - half.x),
        clamp(want.y, half.y, size.y - half.y),
    ))
}

fn snap_camera(ctx: &mut Ctx) {
    ctx.camera.zoom = ZOOM;
    if let Some(target) = camera_target(ctx) {
        ctx.camera.look_at(target);
    }
}

// ANCHOR: camera
/// Frame system: the camera eases toward the hero.
pub fn follow_camera(ctx: &mut Ctx) {
    if let Some(target) = camera_target(ctx) {
        let center = ctx.camera.center();
        let ease = 1.0 - (-8.0 * ctx.time.frame_dt()).exp();
        ctx.camera.look_at(center + (target - center) * ease);
    }
}
// ANCHOR_END: camera

/// Frame system: additive glows, the fade between levels and the text.
pub fn draw_overlay(ctx: &mut Ctx) {
    let art = *ctx.world.resource::<Art>();
    // Light around the hero, the sparks, lit braziers and the lantern.
    let mut glows: Vec<(Vec2, f32, Color)> = Vec::new();
    for (t, _) in ctx.world.query::<(&Transform, &Player)>().iter() {
        glows.push((t.position, 3.0, Color::rgba(1.0, 0.55, 0.2, 0.5)));
    }
    for (t, _) in ctx.world.query::<(&Transform, &Spark)>().iter() {
        glows.push((t.position, 1.2, Color::rgba(1.0, 0.8, 0.3, 0.35)));
    }
    for (t, c) in ctx.world.query::<(&Transform, &Checkpoint)>().iter() {
        if c.lit {
            glows.push((t.position, 2.5, Color::rgba(1.0, 0.5, 0.15, 0.45)));
        }
    }
    for (t, s) in ctx.world.query::<(&Transform, &Sprite)>().iter() {
        if s.texture == art.lantern.texture {
            glows.push((t.position, 3.0, Color::rgba(1.0, 0.85, 0.4, 0.5)));
        }
    }
    for (at, scale, color) in glows {
        ctx.gfx.draw_ex(
            art.glow,
            at,
            DrawParams::default()
                .with_blend(BlendMode::Additive)
                .with_scale(Vec2::splat(scale))
                .with_color(color)
                .with_z(0.9),
        );
    }
    // The fade between levels.
    let fade = match *ctx.world.resource::<Screen>() {
        Screen::Transition { t, .. } => 1.0 - ((t - FADE).abs() / FADE).min(1.0),
        Screen::Dying { t } => (t / DEATH_DELAY).min(1.0) * 0.6,
        _ => 0.0,
    };
    if fade > 0.0 {
        let view = ctx.camera.visible_rect();
        ctx.gfx.draw_rect(view, Color::rgba(0.03, 0.02, 0.05, fade));
    }
    hud(ctx);
}

/// A line of text on the screen.
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

/// The centred text of the current screen (empty while playing). A function
/// of the world only, so tests can read the screens.
pub fn screen_lines(world: &World, storage: &Storage) -> Vec<Line> {
    let title = Color::hex(0xffd35a);
    let text = Color::hex(0xe8dcc8);
    let dim = Color::hex(0x9a8fb0);
    let progress = world.resource::<Progress>();
    match *world.resource::<Screen>() {
        Screen::Title => {
            let mut lines = vec![
                line("Ember Hop", 64.0, title),
                line("Carry the light through the caves.", 24.0, text),
                line("", 16.0, text),
                line("Enter / Space / A: new run", 22.0, text),
            ];
            if has_saved_run(storage) {
                lines.push(line("C / Y: continue the saved run", 22.0, text));
            }
            lines.push(line("Esc: quit", 18.0, dim));
            if let Some(best) = progress.best_time {
                lines.push(line(
                    format!("Best run {:.1} s, most sparks {}", best, progress.most_sparks),
                    18.0,
                    dim,
                ));
            }
            if let Some(why) = &world.resource::<LoadStatus>().message() {
                lines.push(line(why.clone(), 16.0, Color::hex(0xff8a6a)));
            }
            lines
        }
        Screen::Paused => vec![
            line("Paused", 48.0, title),
            line(
                "Esc / Start: resume    Q / B: to the title (the run is saved)",
                20.0,
                text,
            ),
        ],
        Screen::Finished => {
            let run = world.resource::<Run>();
            let mut lines = vec![
                line("The light made it out", 48.0, title),
                line(
                    format!(
                        "{} sparks   {:.1} s   lost {} time{}",
                        run.sparks,
                        run.time,
                        run.deaths,
                        if run.deaths == 1 { "" } else { "s" }
                    ),
                    24.0,
                    text,
                ),
            ];
            if progress.last_new_best {
                lines.push(line("A new best!", 24.0, title));
            }
            lines.push(line("Press any key or button", 18.0, dim));
            lines
        }
        _ => Vec::new(),
    }
}

fn hud(ctx: &mut Ctx) {
    let screen = *ctx.world.resource::<Screen>();
    if matches!(
        screen,
        Screen::Playing | Screen::Paused | Screen::Dying { .. } | Screen::Transition { .. }
    ) {
        let run = ctx.world.resource::<Run>().clone();
        let level = ctx.world.resource::<CurrentLevel>().title.clone();
        let status = format!(
            "{}   Sparks {}/{}   {:.0} s",
            level, run.level_sparks, run.level_total, run.time
        );
        ctx.gfx.draw_text(&status, vec2(14.0, 10.0), 22.0, Color::hex(0xe8dcc8));
        ctx.gfx.draw_text(
            "Move: arrows / A D / stick   Jump: Space / A   Flare: X / Shift / X button   Pause: Esc",
            vec2(14.0, WINDOW.y - 26.0),
            14.0,
            Color::hex(0x9a8fb0),
        );
    }
    let notice = ctx.world.resource::<Notice>().clone();
    if notice.seconds > 0.0 {
        let width = ctx.gfx.measure_text(&notice.text, 18.0).x;
        ctx.gfx.draw_text(
            &notice.text,
            vec2((WINDOW.x - width) / 2.0, 44.0),
            18.0,
            Color::hex(0xffd35a),
        );
    }
    let lines = screen_lines(&ctx.world, &ctx.storage);
    let height: f32 = lines.iter().map(|l| l.size * 1.4).sum();
    let mut y = (WINDOW.y - height) / 2.0 - 40.0;
    for l in lines {
        let width = ctx.gfx.measure_text(&l.text, l.size).x;
        ctx.gfx
            .draw_text(&l.text, vec2((WINDOW.x - width) / 2.0, y), l.size, l.color);
        y += l.size * 1.4;
    }
}

// ---- layer 4: music -------------------------------------------------------------------

/// The music: one streamed loop for the whole game.
pub struct Music;

impl Plugin for Music {
    fn build(&self, game: Game) -> Game {
        game
    }
    fn setup(&self, ctx: &mut Ctx) {
        let music = ctx.world.resource::<Sounds>().music;
        ctx.audio.play_ex(
            music,
            SoundParams {
                volume: 0.35,
                looping: true,
                ..SoundParams::default()
            },
        );
    }
}

// ---- progress --------------------------------------------------------------------------

/// The best results, kept in [`PROGRESS_FILE`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub version: u32,
    pub runs_finished: u32,
    /// Fastest finished run, seconds.
    pub best_time: Option<f32>,
    pub most_sparks: u32,
    /// The last finished run beat a best (not saved).
    #[serde(skip)]
    pub last_new_best: bool,
}

/// How the progress file was found at startup.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadStatus {
    /// No file yet (or no storage).
    Fresh,
    Loaded,
    /// It was damaged: kept as `progress.json.bad`, started over.
    Recovered(String),
}

impl LoadStatus {
    pub fn message(&self) -> Option<String> {
        match self {
            LoadStatus::Recovered(why) => Some(format!("The progress file was damaged and set aside ({why}).")),
            _ => None,
        }
    }
}

impl Progress {
    pub const VERSION: u32 = 1;

    /// Reads [`PROGRESS_FILE`]; a missing one is a first run, a damaged one
    /// is copied to `.bad` and replaced by the defaults.
    pub fn load(storage: &mut Storage) -> (Progress, LoadStatus) {
        let fresh = Progress {
            version: Self::VERSION,
            ..Progress::default()
        };
        match storage.read_json::<Progress>(PROGRESS_FILE) {
            Ok(p) if p.version == Self::VERSION && p.best_time.is_none_or(|t| t.is_finite() && t > 0.0) => {
                (p, LoadStatus::Loaded)
            }
            Ok(_) => {
                Self::set_aside(storage);
                (fresh, LoadStatus::Recovered("unexpected values".into()))
            }
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::Unsupported) => (fresh, LoadStatus::Fresh),
            Err(e) => {
                Self::set_aside(storage);
                (fresh, LoadStatus::Recovered(format!("{:?}", e.kind())))
            }
        }
    }

    fn set_aside(storage: &mut Storage) {
        if let Ok(bytes) = storage.read(PROGRESS_FILE) {
            let _ = storage.write(&format!("{PROGRESS_FILE}.bad"), &bytes);
        }
    }

    pub fn save(&self, storage: &mut Storage) -> Result<(), Error> {
        storage.write_json(PROGRESS_FILE, self)
    }

    /// Adds a finished run; `true` if it beat a best.
    pub fn record(&mut self, run: &Run) -> bool {
        self.version = Self::VERSION;
        self.runs_finished += 1;
        let faster = self.best_time.is_none_or(|t| run.time < t);
        let more = run.sparks > self.most_sparks;
        if faster {
            self.best_time = Some(run.time);
        }
        if more {
            self.most_sparks = run.sparks;
        }
        faster || more
    }
}
