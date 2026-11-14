//! The game: rules, enemies, menus, saves and drawing. Shared with
//! `tests/shard_grove.rs`, which plays it without a window.
//!
//! Five layers, each a [`Plugin`], gathered by [`ShardGrove`]:
//!
//! 1. [`Controls`]: `read_input` (frame system) turns named actions from the
//!    keyboard and gamepad into an [`Intent`]; `menus` turns clicks on the
//!    retained UI's buttons into [`Menu`] choices on the same `Intent`.
//! 2. [`Rules`]: `rules` (sim system) is the whole game. It reads only the
//!    `Intent`, the world and the physics results, so a test can write the
//!    `Intent` itself. `step_crawlers` moves the enemies on the navigation
//!    mesh while the game is being played.
//! 3. [`Presentation`]: the camera, the spinning shards, the gate, the
//!    enemies' hum and the pulse's sparks. None of it changes the game.
//! 4. [`Screens`]: the title, pause, settings and end menus and the HUD
//!    (`ctx.ui`), and the panel over the gate (a `UiPanel` in the world).
//! 5. [`Ambience`]: the wind, started once.
//!
//! What the engine does for it: the level is one glTF file
//! (`assets/shard_grove/grove.glb`) that is drawn (`load_mesh`), solid
//! (`load_collision_mesh`) and walkable for the enemies (`load_mesh_data`
//! baked into a `NavMesh`). The warden and the crawlers are skinned glTF
//! characters whose clips (`load_clip`) are states of an `AnimationGraph`,
//! switched by parameters the rules set. The warden is a `Character`
//! controller; the crawlers are `NavAgent`s. Their hum is an `AudioSource`
//! in the world; pickups play where they happen (`play_at`). A sun with
//! shadows and point lights light the grove, at the quality preset the
//! settings menu picks (`GraphicsQuality`). Saves, settings and records are
//! small JSON files in `ctx.storage`.

// The test file includes this module but needs only part of it.
#![allow(dead_code)]

// Next to this file, whichever crate includes it (`main.rs` or `tests/shard_grove.rs`).
#[path = "smoke.rs"]
pub mod smoke;

use ponko::anim::{
    AnimationClip, AnimationCondition, AnimationGraph, AnimationParameter, AnimationState, AnimationTransition,
    Animator, Motion, ParamId, Skeleton, StateId,
};
use ponko::audio::{AudioEffects, AudioRolloff};
use ponko::d3::*;
use ponko::input::{Gamepad, GamepadAxis, GamepadButton, GamepadStick};
use ponko::nav::{NavBakeSettings, NavSource};
use ponko::particles::{ParticleCurve, ParticleGradient};
use ponko::prelude::*;
use ponko::render::{GraphicsQuality, MaterialDesc, Mesh, MeshData};
use ponko::storage::Storage;
use ponko::ui::{UiEvent, UiLayout};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---- tuning ---------------------------------------------------------------------

/// The window title, and the game's name.
pub const TITLE: &str = "Shard Grove";
/// The window, in pixels.
pub const WINDOW: (u32, u32) = (1280, 720);
/// Where the warden starts a new game, at its feet.
pub const START: Vec3 = Vec3::new(0.0, 0.0, 16.0);
/// Walking speed in m/s.
pub const WALK_SPEED: f32 = 4.5;
/// How fast a jump starts, in m/s.
pub const JUMP_SPEED: f32 = 5.0;
/// How fast the view turns, in radians per second.
pub const TURN_SPEED: f32 = 2.2;
/// Hits the warden takes before the run is lost.
pub const MAX_HEALTH: u32 = 3;
/// Seconds after a hit in which nothing hurts.
pub const INVULNERABLE: f32 = 1.5;
/// A hit throws the warden back this fast, for this long.
pub const KNOCKBACK: f32 = 6.0;
pub const KNOCKBACK_TIME: f32 = 0.25;
/// The pulse stuns every crawler this close, for this long, and can be
/// used again after the cooldown.
pub const PULSE_RADIUS: f32 = 4.5;
pub const STUN: f32 = 3.5;
pub const PULSE_COOLDOWN: f32 = 1.2;
/// Crawlers: chasing and patrolling speeds, the distance they notice the
/// warden from, the distance they give up at, and how close hurts.
pub const CHASE_SPEED: f32 = 2.4;
pub const PATROL_SPEED: f32 = 1.6;
pub const AGGRO: f32 = 8.0;
pub const GIVE_UP: f32 = 12.0;
pub const TOUCH: f32 = 0.9;
/// How close the warden has to come to pick a shard up, to wake a shrine
/// and to step through the open gate.
pub const PICKUP: f32 = 1.1;
pub const SHRINE_RADIUS: f32 = 1.6;
pub const GATE_RADIUS: f32 = 2.0;

/// The shards, by id.
pub const SHARD_SPOTS: [Vec3; 6] = [
    Vec3::new(13.0, 1.5, -13.0), // on the platform
    Vec3::new(-18.0, 0.0, -18.0),
    Vec3::new(-18.0, 0.0, 18.0),
    Vec3::new(18.0, 0.0, 18.0),
    Vec3::new(-9.0, 0.0, -4.0), // in the ruins' corner
    Vec3::new(4.0, 0.0, 8.0),   // between two broken walls
];
/// The shrines that save the game, by id.
pub const SHRINE_SPOTS: [Vec3; 2] = [Vec3::new(-16.0, 0.0, 4.0), Vec3::new(17.0, 0.0, 2.0)];
/// Where a load puts the warden: beside its shrine.
pub const SHRINE_STEP: Vec3 = Vec3::new(0.0, 0.0, 2.0);
/// The gate in the north wall: step through once every shard is found.
pub const GATE: Vec3 = Vec3::new(0.0, 0.0, -20.4);
/// Each crawler walks between its two points until it sees the warden.
pub const CRAWLER_ROUTES: [[Vec3; 2]; 4] = [
    [Vec3::new(-10.0, 0.0, -14.0), Vec3::new(-2.0, 0.0, -14.0)],
    [Vec3::new(12.0, 0.0, 10.0), Vec3::new(12.0, 0.0, 18.0)],
    [Vec3::new(-16.0, 0.0, 10.0), Vec3::new(-16.0, 0.0, 18.0)],
    [Vec3::new(4.0, 0.0, -18.0), Vec3::new(4.0, 0.0, -10.0)],
];

// ---- files -------------------------------------------------------------------------

pub const LEVEL: &str = "shard_grove/grove.glb";
pub const WARDEN: &str = "shard_grove/warden.glb";
pub const CRAWLER: &str = "shard_grove/crawler.glb";
/// The name of the game's folder in the player's data folder.
pub const APP_ID: &str = "shard-grove";
/// The run, written at the shrines.
pub const SAVE_FILE: &str = "save.json";
/// The settings menu's choices.
pub const SETTINGS_FILE: &str = "settings.json";
/// Best time and wins.
pub const RECORDS_FILE: &str = "records.json";
/// The save format; a file with another number is set aside.
pub const SAVE_VERSION: u32 = 1;

// ---- state -----------------------------------------------------------------------

/// What is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Title,
    Playing,
    Paused,
    /// The settings menu, and the screen it goes back to.
    Settings {
        from_pause: bool,
    },
    Won,
    Lost,
}

/// A choice on a menu: a button, or Escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    NewGame,
    Continue,
    Settings,
    Quit,
    Resume,
    ToTitle,
    TryAgain,
    CycleQuality,
    CycleVolume,
    Back,
}

/// What the player wants this tick. Presses are latched until the rules
/// use them; the axes are what is held.
#[derive(Debug, Clone, Copy, Default)]
pub struct Intent {
    /// x right, y forward, relative to the view.
    pub movement: Vec2,
    /// Turns the view: positive to the right.
    pub turn: f32,
    pub jump: bool,
    pub pulse: bool,
    pub pause: bool,
    pub menu: Option<Menu>,
}

/// The run: what a save holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// The ids of the shards found.
    pub shards: Vec<u8>,
    pub health: u32,
    /// Seconds played.
    pub time: f32,
    /// The shrine last saved at.
    pub shrine: Option<u8>,
}

impl Default for Run {
    fn default() -> Self {
        Run {
            shards: Vec::new(),
            health: MAX_HEALTH,
            time: 0.0,
            shrine: None,
        }
    }
}

impl Run {
    pub fn complete(&self) -> bool {
        self.shards.len() == SHARD_SPOTS.len()
    }
}

/// The save file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Save {
    pub version: u32,
    pub run: Run,
}

/// The settings menu's choices.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// An index into [`GraphicsQuality::ALL`]; `None` until the player picks
    /// one: the preset recommended for the GPU (on the web, `Low`).
    pub quality: Option<usize>,
    /// 0 to 1, for every sound.
    pub volume: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            quality: None,
            volume: 0.8,
        }
    }
}

/// Best results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Records {
    pub best_time: Option<f32>,
    pub wins: u32,
}

/// How the save file was found when last read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SaveStatus {
    /// None yet (or no storage).
    #[default]
    Fresh,
    Loaded,
    /// It couldn't be read and was set aside as `save.json.bad`.
    Recovered(String),
}

/// What happened, for the HUD's message line, the tests and the smoke report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tally {
    pub pulses: u32,
    pub stuns: u32,
    pub hits: u32,
    pub shards: u32,
    pub saves: u32,
    pub loads: u32,
    pub losses: u32,
    pub wins: u32,
}

/// A line of text shown for a few seconds.
#[derive(Debug, Clone, Default)]
pub struct Notice {
    pub text: String,
    pub seconds: f32,
}

/// The way the camera looks, around the warden (0 looks along -Z).
#[derive(Debug, Clone, Copy, Default)]
pub struct View {
    pub yaw: f32,
}

// ---- components ------------------------------------------------------------------

/// The player's character. Its position is its `Transform`, its motion its
/// `Character`; this is the rest of its state.
#[derive(Component, Debug, Clone, Default)]
pub struct Warden {
    /// The way it faces (0 looks along -Z).
    pub facing: f32,
    /// Seconds of knockback left, and its velocity.
    pub knocked: f32,
    pub knock: Vec3,
    /// Seconds in which nothing hurts.
    pub invulnerable: f32,
    /// Seconds until the next pulse.
    pub cooldown: f32,
}

/// An enemy: walks its route, chases the warden, hurts on touch.
#[derive(Component, Debug, Clone)]
pub struct Crawler {
    pub route: [Vec3; 2],
    /// The point of the route it walks to.
    pub leg: usize,
    /// Seconds of stun left.
    pub stun: f32,
    pub chasing: bool,
    /// Where it was last tick, to face the way it moves.
    pub last: Vec3,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Shard {
    pub id: u8,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Shrine {
    pub id: u8,
}

/// The gate's light, glowing once it is open.
#[derive(Component, Debug, Clone, Copy)]
pub struct GateGlow;

/// Belongs to a run: removed when the run ends.
#[derive(Component, Debug, Clone, Copy)]
pub struct RunEntity;

/// The emitter of the pulse's sparks.
#[derive(Component, Debug, Clone, Copy)]
pub struct PulseSparks;

// ---- assets ----------------------------------------------------------------------

/// The warden's animation parameters and states.
pub mod warden_anim {
    use super::*;
    pub const SPEED: ParamId = ParamId(0);
    pub const PULSE: ParamId = ParamId(1);
    pub const HURT: ParamId = ParamId(2);
    pub const IDLE: StateId = StateId(0);
    pub const WALK: StateId = StateId(1);
    pub const CAST: StateId = StateId(2);
    pub const FLINCH: StateId = StateId(3);
}

/// The crawlers' animation parameters and states.
pub mod crawler_anim {
    use super::*;
    pub const MOVING: ParamId = ParamId(0);
    pub const STUNNED: ParamId = ParamId(1);
    pub const IDLE: StateId = StateId(0);
    pub const CRAWL: StateId = StateId(1);
    pub const DAZED: StateId = StateId(2);
}

/// Everything loaded once.
#[derive(Clone)]
pub struct GroveAssets {
    pub warden: Mesh,
    pub warden_skeleton: Skeleton,
    pub warden_graph: Arc<AnimationGraph>,
    pub crawler: Mesh,
    pub crawler_skeleton: Skeleton,
    pub crawler_graph: Arc<AnimationGraph>,
    pub shard: Mesh,
    pub sounds: Sounds,
}

#[derive(Clone, Copy)]
pub struct Sounds {
    pub shard: Sound,
    pub pulse: Sound,
    pub hurt: Sound,
    pub shrine: Sound,
    pub gate: Sound,
    pub select: Sound,
    pub crawler: Sound,
    pub wind: Sound,
}

/// The mixer buses the volume setting turns.
#[derive(Clone, Copy)]
pub struct Buses {
    pub sfx: AudioBus,
    pub ambience: AudioBus,
}

/// The warden's state machine: idle and walk switched by speed; the pulse
/// and a hit play once from any state, then return to idle.
pub fn warden_graph(skeleton: &Skeleton, clip: &mut dyn FnMut(&str) -> AnimationClip) -> Arc<AnimationGraph> {
    use warden_anim::*;
    let _ = skeleton;
    let once = |c: AnimationClip| AnimationState {
        looping: false,
        ..AnimationState::new(Motion::Clip(c))
    };
    let states = vec![
        AnimationState::new(Motion::Clip(clip("idle"))),
        AnimationState::new(Motion::Clip(clip("walk"))),
        once(clip("pulse")),
        once(clip("hurt")),
    ];
    let transitions = vec![
        AnimationTransition::new(IDLE, WALK)
            .with_condition(AnimationCondition::Greater(SPEED, 0.5))
            .with_duration(0.15),
        AnimationTransition::new(WALK, IDLE)
            .with_condition(AnimationCondition::Less(SPEED, 0.3))
            .with_duration(0.2),
        AnimationTransition::from_any(CAST)
            .with_condition(AnimationCondition::Trigger(PULSE))
            .with_duration(0.05),
        AnimationTransition::from_any(FLINCH)
            .with_condition(AnimationCondition::Trigger(HURT))
            .with_duration(0.05),
        AnimationTransition::new(CAST, IDLE)
            .with_exit_time(Some(1.0))
            .with_duration(0.15),
        AnimationTransition::new(FLINCH, IDLE)
            .with_exit_time(Some(1.0))
            .with_duration(0.15),
    ];
    AnimationGraph::new(
        vec![
            AnimationParameter::Float(0.0),
            AnimationParameter::Trigger(false),
            AnimationParameter::Trigger(false),
        ],
        states,
        transitions,
        IDLE,
    )
    .expect("the warden's graph is valid")
}

/// The crawlers' state machine: idle and crawl switched by moving; dazed
/// while stunned.
pub fn crawler_graph(clip: &mut dyn FnMut(&str) -> AnimationClip) -> Arc<AnimationGraph> {
    use crawler_anim::*;
    let states = vec![
        AnimationState::new(Motion::Clip(clip("idle"))),
        AnimationState::new(Motion::Clip(clip("crawl"))),
        AnimationState::new(Motion::Clip(clip("stunned"))),
    ];
    let when = |from, to, condition| {
        AnimationTransition::new(from, to)
            .with_condition(condition)
            .with_duration(0.15)
    };
    let transitions = vec![
        when(IDLE, CRAWL, AnimationCondition::Bool(MOVING, true)),
        when(CRAWL, IDLE, AnimationCondition::Bool(MOVING, false)),
        when(IDLE, DAZED, AnimationCondition::Bool(STUNNED, true)),
        when(CRAWL, DAZED, AnimationCondition::Bool(STUNNED, true)),
        when(DAZED, IDLE, AnimationCondition::Bool(STUNNED, false)),
    ];
    AnimationGraph::new(
        vec![AnimationParameter::Bool(false), AnimationParameter::Bool(false)],
        states,
        transitions,
        IDLE,
    )
    .expect("the crawlers' graph is valid")
}

fn load_assets(ctx: &mut Ctx) -> GroveAssets {
    let warden_skeleton = ctx.gfx.load_skeleton(WARDEN);
    let warden_graph = warden_graph(&warden_skeleton, &mut |name| ctx.gfx.load_clip(WARDEN, name));
    let crawler_skeleton = ctx.gfx.load_skeleton(CRAWLER);
    let crawler_graph = crawler_graph(&mut |name| ctx.gfx.load_clip(CRAWLER, name));
    let glow = MaterialDesc::color(Color::hex(0x9ff5ff)).with_emissive(Color::hex(0x49d8ff));
    let glow = ctx.gfx.create_material(glow);
    let shard = ctx.gfx.create_mesh(&MeshData::sphere(0.28, 4), glow);
    let mut sound = |name: &str| ctx.audio.load(&format!("shard_grove/sounds/{name}.wav"));
    let sounds = Sounds {
        shard: sound("shard"),
        pulse: sound("pulse"),
        hurt: sound("hurt"),
        shrine: sound("shrine"),
        gate: sound("gate"),
        select: sound("select"),
        crawler: sound("crawler"),
        wind: sound("wind"),
    };
    GroveAssets {
        warden: ctx.gfx.load_mesh(WARDEN),
        warden_skeleton,
        warden_graph,
        crawler: ctx.gfx.load_mesh(CRAWLER),
        crawler_skeleton,
        crawler_graph,
        shard,
        sounds,
    }
}

/// The navigation mesh of the level, for the crawlers (and the smoke
/// test's autopilot): its own triangles, baked for a crawler's size.
pub fn bake_navigation(level: &MeshData) -> NavMesh {
    let source = NavSource::from_mesh(level, Transform::default());
    let settings = NavBakeSettings {
        agent_radius: 0.45,
        agent_height: 1.8,
        max_climb: 0.3,
        ..Default::default()
    };
    NavMesh::bake(&[source], settings).expect("the grove bakes")
}

// ---- the game --------------------------------------------------------------------

/// The whole game: every layer.
pub struct ShardGrove;

impl Plugin for ShardGrove {
    fn build(&self, game: Game) -> Game {
        game.plugin(Controls)
            .plugin(Rules)
            .plugin(Presentation)
            .plugin(Screens)
            .plugin(Ambience)
    }
    fn setup(&self, _ctx: &mut Ctx) {}
}

/// The game, ready to run, with its files in the player's data folder.
pub fn build() -> Game {
    Game::new(TITLE, WINDOW.0, WINDOW.1).app_id(APP_ID).plugin(ShardGrove)
}

// ---- layer 1: controls -------------------------------------------------------------

actions! {
    pub struct Act {
        movement = "move",
        turn = "turn",
        jump = "jump",
        pulse = "pulse",
        pause = "pause",
    }
}

/// Keys and a gamepad, bound to the same actions.
pub fn bindings() -> ActionMap {
    let pad = Gamepad::from_slot(0).expect("slot 0 exists");
    let mut map = ActionMap::default();
    map.set(
        "move",
        Action::Axis2(vec![
            Axis2Binding::Keys {
                left: Key::A,
                right: Key::D,
                up: Key::W,
                down: Key::S,
            },
            Axis2Binding::Keys {
                left: Key::Left,
                right: Key::Right,
                up: Key::Up,
                down: Key::Down,
            },
            Axis2Binding::Gamepad {
                pad,
                stick: GamepadStick::Left,
                deadzone: 0.2,
            },
        ]),
    );
    map.set(
        "turn",
        Action::Axis(vec![
            AxisBinding::Keys {
                negative: Key::Q,
                positive: Key::E,
            },
            AxisBinding::Gamepad {
                pad,
                axis: GamepadAxis::RightStickX,
                deadzone: 0.2,
            },
        ]),
    );
    let button = |key: Key, alt: Key, button: GamepadButton| {
        Action::Button(vec![
            ButtonBinding::Key(key),
            ButtonBinding::Key(alt),
            ButtonBinding::Gamepad { pad, button },
        ])
    };
    map.set("jump", button(Key::Space, Key::Space, GamepadButton::South));
    map.set("pulse", button(Key::F, Key::LeftShift, GamepadButton::West));
    map.set("pause", button(Key::Escape, Key::P, GamepadButton::Start));
    map
}

/// Layer 1: input to [`Intent`].
pub struct Controls;

impl Plugin for Controls {
    fn build(&self, game: Game) -> Game {
        game.system(read_input)
    }
    fn setup(&self, ctx: &mut Ctx) {
        ctx.input.set_actions(bindings());
        let act = Act::resolve(ctx);
        ctx.world.insert_resource(act);
        ctx.world.insert_resource(Intent::default());
    }
}

/// Frame system: the actions into the [`Intent`]. The pause key only
/// pauses: on a menu, Escape belongs to the UI (it goes back).
pub fn read_input(ctx: &mut Ctx) {
    let act = *ctx.world.resource::<Act>();
    let playing = *ctx.world.resource::<Screen>() == Screen::Playing;
    let input = &ctx.input;
    let movement = input.action_axis2(act.movement); // y is down: W is -1
    let turn = input.action_axis(act.turn);
    let jump = input.action_pressed(act.jump);
    let pulse = input.action_pressed(act.pulse);
    let pause = playing && input.action_pressed(act.pause);
    let intent = ctx.world.resource_mut::<Intent>();
    intent.movement = vec2(movement.x, -movement.y);
    intent.turn = turn;
    intent.jump |= jump;
    intent.pulse |= pulse;
    intent.pause |= pause;
}

// ---- layer 2: the rules ------------------------------------------------------------

/// Layer 2: the rules, the run and the enemies.
pub struct Rules;

impl Plugin for Rules {
    fn build(&self, game: Game) -> Game {
        game.physics3d(vec3(0.0, -9.81, 0.0))
            .sim_system(rules)
            .sim_system(step_crawlers)
    }
    fn setup(&self, ctx: &mut Ctx) {
        // The level: drawn, solid, and walkable for the crawlers.
        let look = ctx.gfx.load_mesh(LEVEL);
        let solid = ctx.load_collision_mesh(LEVEL).expect("the grove loads");
        ctx.world.spawn((
            Transform::default(),
            Model::new(look),
            RigidBody::fixed(),
            Collider::mesh(solid),
        ));
        let data = ctx.gfx.load_mesh_data(LEVEL);
        ctx.world.insert_resource(bake_navigation(&data));

        let assets = load_assets(ctx);
        ctx.world.insert_resource(assets);
        spawn_fixtures(ctx);

        let settings = read_settings(&ctx.storage);
        ctx.world.insert_resource(settings);
        let buses = Buses {
            sfx: AudioBus::default(),
            ambience: ctx.audio.bus("ambience"),
        };
        ctx.world.insert_resource(buses);
        apply_settings(ctx);
        let records = ctx.storage.read_json::<Records>(RECORDS_FILE).unwrap_or_default();
        ctx.world.insert_resource(records);
        ctx.world.insert_resource(Run::default());
        ctx.world.insert_resource(SaveStatus::default());
        ctx.world.insert_resource(Tally::default());
        ctx.world.insert_resource(Notice::default());
        ctx.world.insert_resource(View::default());
        ctx.world.insert_resource(Screen::Title);
    }
}

/// The shrines and the gate's glow: part of the level, there on every screen.
fn spawn_fixtures(ctx: &mut Ctx) {
    let stone = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x6f6a62)));
    let base = ctx.gfx.create_mesh(&MeshData::cube(1.0), stone);
    let flame = MaterialDesc::color(Color::hex(0xffd27a)).with_emissive(Color::hex(0xffa53a));
    let flame = ctx.gfx.create_material(flame);
    let flame = ctx.gfx.create_mesh(&MeshData::sphere(0.22, 12), flame);
    for (id, at) in SHRINE_SPOTS.into_iter().enumerate() {
        ctx.world.spawn((
            Transform {
                scale: vec3(0.8, 1.1, 0.8),
                ..Transform::at(at + Vec3::Y * 0.55)
            },
            Model::new(base),
        ));
        ctx.world.spawn((
            Transform::at(at + Vec3::Y * 1.35),
            Model::new(flame),
            PointLight {
                color: Color::rgb(1.0, 0.7, 0.4),
                intensity: 6.0,
                range: 7.0,
                shadows: true,
                ..Default::default()
            },
            Shrine { id: id as u8 },
        ));
    }
    let veil = MaterialDesc::color(Color::rgba(0.5, 0.9, 1.0, 0.0)).with_emissive(Color::hex(0x2ec5ff));
    let veil = ctx.gfx.create_material(veil);
    let mut plane = MeshData::cube(1.0);
    for p in &mut plane.positions {
        *p *= vec3(3.6, 3.6, 0.1);
    }
    let veil = ctx.gfx.create_mesh(&plane, veil);
    ctx.world.spawn((
        Transform {
            scale: Vec3::ZERO, // hidden until the gate opens
            ..Transform::at(vec3(0.0, 1.9, -21.6))
        },
        Model::new(veil),
        PointLight {
            color: Color::rgb(0.4, 0.85, 1.0),
            intensity: 0.0,
            range: 9.0,
            ..Default::default()
        },
        GateGlow,
    ));
}

/// Sim system: the whole game.
pub fn rules(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let intent = std::mem::take(ctx.world.resource_mut::<Intent>());
    // The latched presses are used up; what is held stays.
    {
        let held = ctx.world.resource_mut::<Intent>();
        held.movement = intent.movement;
        held.turn = intent.turn;
    }
    {
        let notice = ctx.world.resource_mut::<Notice>();
        notice.seconds = (notice.seconds - dt).max(0.0);
    }
    let screen = *ctx.world.resource::<Screen>();
    if let Some(menu) = intent.menu {
        choose(ctx, screen, menu);
    }
    match *ctx.world.resource::<Screen>() {
        Screen::Playing if screen == Screen::Playing => {
            if intent.pause {
                set_screen(ctx, Screen::Paused);
                hold_still(ctx);
            } else {
                play(ctx, &intent, dt);
            }
        }
        Screen::Playing => {}
        _ => hold_still(ctx),
    }
}

/// A menu choice, on the screen it was made on.
pub fn choose(ctx: &mut Ctx, screen: Screen, menu: Menu) {
    let sound = ctx.world.resource::<GroveAssets>().sounds.select;
    ctx.audio.play(sound);
    match (screen, menu) {
        (Screen::Title, Menu::NewGame) => new_game(ctx),
        (Screen::Title, Menu::Continue) => {
            continue_game(ctx);
        }
        (Screen::Title, Menu::Settings) => set_screen(ctx, Screen::Settings { from_pause: false }),
        (Screen::Title, Menu::Quit) => ctx.quit(),
        (Screen::Paused, Menu::Resume | Menu::Back) => set_screen(ctx, Screen::Playing),
        (Screen::Paused, Menu::Settings) => set_screen(ctx, Screen::Settings { from_pause: true }),
        (Screen::Paused, Menu::ToTitle) => to_title(ctx),
        (Screen::Settings { .. }, Menu::CycleQuality) => {
            let settings = ctx.world.resource_mut::<Settings>();
            let current = settings.quality.unwrap_or(0);
            settings.quality = Some((current + 1) % GraphicsQuality::ALL.len());
            apply_settings(ctx);
            write_settings(ctx);
        }
        (Screen::Settings { .. }, Menu::CycleVolume) => {
            let settings = ctx.world.resource_mut::<Settings>();
            let step = (settings.volume * 5.0).round() as i32;
            settings.volume = ((step + 1) % 6) as f32 / 5.0;
            apply_settings(ctx);
            write_settings(ctx);
        }
        (Screen::Settings { from_pause }, Menu::Back) => {
            set_screen(ctx, if from_pause { Screen::Paused } else { Screen::Title });
        }
        (Screen::Won, _) => to_title(ctx),
        (Screen::Lost, Menu::TryAgain) => {
            if !continue_game(ctx) {
                new_game(ctx);
            }
        }
        (Screen::Lost, Menu::ToTitle | Menu::Back) => to_title(ctx),
        _ => {}
    }
}

pub fn set_screen(ctx: &mut Ctx, screen: Screen) {
    *ctx.world.resource_mut::<Screen>() = screen;
}

fn notice(ctx: &mut Ctx, text: impl Into<String>) {
    *ctx.world.resource_mut::<Notice>() = Notice {
        text: text.into(),
        seconds: 2.5,
    };
}

/// Nothing moves on a menu: the warden stands (3D physics has no pause of
/// its own, so its velocity is held at zero) and the crawlers are not
/// stepped (see [`step_crawlers`]).
fn hold_still(ctx: &mut Ctx) {
    ctx.world
        .query::<&mut Character>()
        .for_each(|c| c.velocity = Vec3::ZERO);
}

/// To the title: the run's entities go, the save stays.
pub fn to_title(ctx: &mut Ctx) {
    clear_run(ctx);
    set_screen(ctx, Screen::Title);
}

fn clear_run(ctx: &mut Ctx) {
    let gone: Vec<Entity> = ctx
        .world
        .query::<(Entity, &RunEntity)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in gone {
        ctx.world.despawn(e);
    }
}

/// A new game from the start; an older save stays until the first shrine.
pub fn new_game(ctx: &mut Ctx) {
    start_run(ctx, Run::default(), START);
}

/// Continues the saved run beside its shrine. `false` (and a notice) if
/// there is none or it can't be read; a damaged file is set aside.
pub fn continue_game(ctx: &mut Ctx) -> bool {
    let (save, status) = read_save(&mut ctx.storage);
    *ctx.world.resource_mut::<SaveStatus>() = status.clone();
    match save {
        Some(run) => {
            let at = run.shrine.map_or(START, |s| SHRINE_SPOTS[usize::from(s)] + SHRINE_STEP);
            start_run(ctx, run, at);
            ctx.world.resource_mut::<Tally>().loads += 1;
            notice(ctx, "Back at the shrine");
            true
        }
        None => {
            let why = match status {
                SaveStatus::Recovered(why) => format!("The save was damaged and set aside ({why})"),
                _ => "No saved game".to_string(),
            };
            notice(ctx, why);
            false
        }
    }
}

/// Starts playing `run` with the warden at `at`.
pub fn start_run(ctx: &mut Ctx, run: Run, at: Vec3) {
    clear_run(ctx);
    let assets = ctx.world.resource::<GroveAssets>().clone();
    ctx.world.spawn((
        Transform::at(at),
        Character::new(1.8, 0.35),
        Model::new(assets.warden),
        assets.warden_skeleton.clone(),
        Animator::new(assets.warden_graph.clone()),
        Warden::default(),
        RunEntity,
    ));
    ctx.world
        .spawn((Transform::at(at), pulse_sparks(), PulseSparks, RunEntity));
    for (id, spot) in SHARD_SPOTS.into_iter().enumerate() {
        if run.shards.contains(&(id as u8)) {
            continue;
        }
        ctx.world.spawn((
            Transform::at(spot + Vec3::Y * 1.0),
            Model::new(assets.shard),
            PointLight {
                color: Color::rgb(0.4, 0.85, 1.0),
                intensity: 3.0,
                range: 4.0,
                ..Default::default()
            },
            Shard { id: id as u8 },
            RunEntity,
        ));
    }
    let hum = AudioSettings {
        sound: SoundParams {
            volume: 0.5,
            looping: true,
            ..Default::default()
        },
        min_distance: 1.5,
        max_distance: 14.0,
        rolloff: AudioRolloff::Linear,
        effects: AudioEffects::default(),
        ..Default::default()
    };
    for route in CRAWLER_ROUTES {
        // NavAgent has private fields: set the public ones on a default.
        let mut agent = NavAgent::default();
        agent.speed = PATROL_SPEED;
        agent.radius = 0.4;
        agent.destination = Some(route[1]);
        ctx.world.spawn((
            Transform::at(route[0]),
            agent,
            Model::new(assets.crawler),
            assets.crawler_skeleton.clone(),
            Animator::new(assets.crawler_graph.clone()),
            AudioSource::new(assets.sounds.crawler, hum),
            Crawler {
                route,
                leg: 1,
                stun: 0.0,
                chasing: false,
                last: route[0],
            },
            RunEntity,
        ));
    }
    *ctx.world.resource_mut::<View>() = View::default();
    *ctx.world.resource_mut::<Run>() = run;
    set_screen(ctx, Screen::Playing);
}

fn pulse_sparks() -> ParticleEmitter {
    ParticleEmitter::new(
        ParticleSettings::default()
            .with_rate(0.0)
            .with_lifetime(0.3, 0.6)
            .with_speed(5.0, 8.0)
            .with_shape(ParticleShape::Sphere { radius: 0.3 })
            .with_gravity(Vec3::ZERO)
            .with_drag(2.0)
            .with_size(0.06, 0.14)
            .with_size_over_life(ParticleCurve::new(vec![(0.0, 1.0), (1.0, 0.0)]))
            .with_color_over_life(ParticleGradient::new(vec![
                (0.0, Color::rgba(0.6, 0.95, 1.0, 1.0)),
                (1.0, Color::rgba(0.2, 0.6, 1.0, 0.0)),
            ]))
            .with_blend(ParticleBlend::Additive)
            .with_max_particles(256)
            .with_seed(375),
    )
}

/// The warden: its entity, its feet and its state.
pub fn warden(world: &mut World) -> Option<(Entity, Vec3)> {
    world
        .query::<(Entity, &Transform, &Warden)>()
        .iter()
        .next()
        .map(|(e, t, _)| (e, t.position))
}

/// Distance on the ground plane.
pub fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    vec2(a.x - b.x, a.z - b.z).length()
}

/// The way a direction on the ground faces (0 looks along -Z).
pub fn facing_of(direction: Vec3) -> f32 {
    (-direction.x).atan2(-direction.z)
}

/// One tick of play.
fn play(ctx: &mut Ctx, intent: &Intent, dt: f32) {
    let Some((hero, at)) = warden(&mut ctx.world) else {
        return;
    };
    ctx.world.resource_mut::<Run>().time += dt;
    let yaw = {
        let view = ctx.world.resource_mut::<View>();
        view.yaw -= intent.turn * TURN_SPEED * dt;
        view.yaw
    };

    // Moving, relative to the view.
    let wish = Quat::from_rotation_y(yaw) * vec3(intent.movement.x, 0.0, -intent.movement.y);
    let wish = wish.clamp_length_max(1.0);
    let mut state = ctx.world.get::<Warden>(hero).expect("the warden").clone();
    state.invulnerable = (state.invulnerable - dt).max(0.0);
    state.cooldown = (state.cooldown - dt).max(0.0);
    state.knocked = (state.knocked - dt).max(0.0);
    if wish.length_squared() > 0.01 {
        state.facing = facing_of(wish);
    }
    let mut speed = 0.0;
    if let Some(c) = ctx.world.get_mut::<Character>(hero) {
        let planar = if state.knocked > 0.0 {
            state.knock
        } else {
            wish * WALK_SPEED
        };
        c.velocity.x = planar.x;
        c.velocity.z = planar.z;
        if intent.jump && c.grounded && state.knocked <= 0.0 {
            c.velocity.y = JUMP_SPEED;
        }
        speed = vec2(c.velocity.x, c.velocity.z).length();
    }
    if let Some(t) = ctx.world.get_mut::<Transform>(hero) {
        t.rotation = Quat::from_rotation_y(state.facing);
    }
    if let Some(a) = ctx.world.get_mut::<Animator>(hero) {
        a.set_float(warden_anim::SPEED, speed);
    }

    // The pulse stuns every crawler close by.
    if intent.pulse && state.cooldown <= 0.0 {
        state.cooldown = PULSE_COOLDOWN;
        let mut stunned = 0;
        ctx.world.query::<(&Transform, &mut Crawler)>().for_each(|(t, c)| {
            if flat_distance(t.position, at) <= PULSE_RADIUS {
                c.stun = STUN;
                c.chasing = false;
                stunned += 1;
            }
        });
        let tally = ctx.world.resource_mut::<Tally>();
        tally.pulses += 1;
        tally.stuns += stunned;
        if let Some(a) = ctx.world.get_mut::<Animator>(hero) {
            a.set_trigger(warden_anim::PULSE, true);
        }
        ctx.world
            .query::<(&mut ParticleEmitter, &PulseSparks)>()
            .for_each(|(e, _)| e.burst(120));
        let sound = ctx.world.resource::<GroveAssets>().sounds.pulse;
        ctx.audio.play_at(sound, at, AudioSettings::default());
    }

    // A crawler's touch hurts and throws the warden back.
    let mut hit_from = None;
    ctx.world.query::<(&Transform, &Crawler)>().for_each(|(t, c)| {
        if c.stun <= 0.0 && flat_distance(t.position, at) < TOUCH && (t.position.y - at.y).abs() < 1.2 {
            hit_from = Some(t.position);
        }
    });
    if let Some(from) = hit_from
        && state.invulnerable <= 0.0
    {
        let away = vec3(at.x - from.x, 0.0, at.z - from.z).normalize_or(Vec3::Z);
        state.knock = away * KNOCKBACK;
        state.knocked = KNOCKBACK_TIME;
        state.invulnerable = INVULNERABLE;
        ctx.world.resource_mut::<Tally>().hits += 1;
        let run = ctx.world.resource_mut::<Run>();
        run.health = run.health.saturating_sub(1);
        if let Some(a) = ctx.world.get_mut::<Animator>(hero) {
            a.set_trigger(warden_anim::HURT, true);
        }
        let sound = ctx.world.resource::<GroveAssets>().sounds.hurt;
        ctx.audio.play_at(sound, at, AudioSettings::default());
    }
    *ctx.world.get_mut::<Warden>(hero).expect("the warden") = state;

    // Shards.
    let found: Vec<(Entity, u8)> = ctx
        .world
        .query::<(Entity, &Transform, &Shard)>()
        .iter()
        .filter(|(_, t, _)| flat_distance(t.position, at) < PICKUP && (t.position.y - 1.0 - at.y).abs() < 1.2)
        .map(|(e, _, s)| (e, s.id))
        .collect();
    for (e, id) in found {
        ctx.world.despawn(e);
        let run = ctx.world.resource_mut::<Run>();
        run.shards.push(id);
        let (found, complete) = (run.shards.len(), run.complete());
        ctx.world.resource_mut::<Tally>().shards += 1;
        let sounds = ctx.world.resource::<GroveAssets>().sounds;
        ctx.audio.play_at(sounds.shard, at, AudioSettings::default());
        if complete {
            ctx.audio.play_at(
                sounds.gate,
                GATE,
                AudioSettings {
                    max_distance: 80.0,
                    ..Default::default()
                },
            );
            notice(ctx, "Every shard found: the gate is open");
        } else {
            notice(ctx, format!("Shard {found} of {}", SHARD_SPOTS.len()));
        }
    }

    // Shrines save the run, once per visit.
    let shrine = SHRINE_SPOTS
        .iter()
        .position(|&s| flat_distance(s, at) < SHRINE_RADIUS)
        .map(|i| i as u8);
    let last = ctx.world.resource::<Run>().shrine;
    if let Some(id) = shrine
        && last != Some(id)
    {
        ctx.world.resource_mut::<Run>().shrine = Some(id);
        save_game(ctx);
        let sound = ctx.world.resource::<GroveAssets>().sounds.shrine;
        ctx.audio
            .play_at(sound, SHRINE_SPOTS[usize::from(id)], AudioSettings::default());
    }

    // The end: through the open gate, or out of health.
    let run = ctx.world.resource::<Run>().clone();
    if run.complete() && flat_distance(GATE, at) < GATE_RADIUS {
        win(ctx, &run);
    } else if run.health == 0 {
        ctx.world.resource_mut::<Tally>().losses += 1;
        clear_run(ctx);
        set_screen(ctx, Screen::Lost);
    }
}

fn win(ctx: &mut Ctx, run: &Run) {
    ctx.world.resource_mut::<Tally>().wins += 1;
    let records = ctx.world.resource_mut::<Records>();
    records.wins += 1;
    if records.best_time.is_none_or(|best| run.time < best) {
        records.best_time = Some(run.time);
    }
    let records = records.clone();
    if let Err(e) = ctx.storage.write_json(RECORDS_FILE, &records)
        && e.kind() != ErrorKind::Unsupported
    {
        notice(ctx, format!("Couldn't save the records: {e}"));
    }
    let _ = ctx.storage.delete(SAVE_FILE);
    clear_run(ctx);
    set_screen(ctx, Screen::Won);
}

/// Sim system: the crawlers decide where to go, then the navigation step
/// moves them, only while the game is being played.
pub fn step_crawlers(ctx: &mut Ctx) {
    if *ctx.world.resource::<Screen>() != Screen::Playing {
        return;
    }
    let dt = ctx.time.dt();
    let target = warden(&mut ctx.world).map(|(_, at)| at);
    ctx.world
        .query::<(&Transform, &mut NavAgent, &mut Crawler, &mut Animator)>()
        .for_each(|(t, agent, c, animator)| {
            let at = t.position;
            if c.stun > 0.0 {
                c.stun = (c.stun - dt).max(0.0);
                agent.destination = None;
            } else {
                let near = target.map(|w| (w, flat_distance(w, at)));
                match near {
                    Some((_, d)) if c.chasing && d > GIVE_UP => c.chasing = false,
                    Some((_, d)) if !c.chasing && d < AGGRO => c.chasing = true,
                    None => c.chasing = false,
                    _ => {}
                }
                if let (true, Some((w, _))) = (c.chasing, near) {
                    agent.speed = CHASE_SPEED;
                    if agent.destination.is_none_or(|d| d.distance(w) > 0.5) {
                        agent.destination = Some(w);
                    }
                } else {
                    agent.speed = PATROL_SPEED;
                    if flat_distance(at, c.route[c.leg]) < 0.6 {
                        c.leg ^= 1;
                    }
                    agent.destination = Some(c.route[c.leg]);
                }
            }
            animator.set_bool(crawler_anim::STUNNED, c.stun > 0.0);
        });
    ponko::systems::step_navmesh3d(ctx);
    // Face the way they moved.
    ctx.world
        .query::<(&mut Transform, &mut Crawler, &mut Animator)>()
        .for_each(|(t, c, animator)| {
            let moved = t.position - c.last;
            let moving = vec2(moved.x, moved.z).length() > 0.2 * dt;
            if moving && c.stun <= 0.0 {
                t.rotation = Quat::from_rotation_y(facing_of(moved));
            }
            animator.set_bool(crawler_anim::MOVING, moving);
            c.last = t.position;
        });
}

// ---- saves and settings ------------------------------------------------------------

/// Writes the run to its file. A failed write is shown, and play goes on.
pub fn save_game(ctx: &mut Ctx) {
    let save = Save {
        version: SAVE_VERSION,
        run: ctx.world.resource::<Run>().clone(),
    };
    match ctx.storage.write_json(SAVE_FILE, &save) {
        Ok(()) => {
            ctx.world.resource_mut::<Tally>().saves += 1;
            notice(ctx, "Saved at the shrine");
        }
        Err(e) if e.kind() == ErrorKind::Unsupported => notice(ctx, "The shrine glows (no saves here)"),
        Err(e) => notice(ctx, format!("Couldn't save: {e}")),
    }
}

/// A saved run to continue.
pub fn has_save(storage: &Storage) -> bool {
    storage.exists(SAVE_FILE)
}

/// The saved run, and how the file was found. A file that can't be read,
/// or holds what no run can be, is set aside as `save.json.bad`.
pub fn read_save(storage: &mut Storage) -> (Option<Run>, SaveStatus) {
    let damaged = |storage: &mut Storage, why: String| {
        if let Ok(bytes) = storage.read(SAVE_FILE) {
            let _ = storage.write(&format!("{SAVE_FILE}.bad"), &bytes);
        }
        let _ = storage.delete(SAVE_FILE);
        (None, SaveStatus::Recovered(why))
    };
    match storage.read_json::<Save>(SAVE_FILE) {
        Ok(save) if save.version != SAVE_VERSION => damaged(storage, format!("version {}", save.version)),
        Ok(save)
            if save.run.health == 0
                || save.run.health > MAX_HEALTH
                || save.run.shards.iter().any(|&s| usize::from(s) >= SHARD_SPOTS.len())
                || save.run.shrine.is_some_and(|s| usize::from(s) >= SHRINE_SPOTS.len()) =>
        {
            damaged(storage, "unexpected values".into())
        }
        Ok(save) => (Some(save.run), SaveStatus::Loaded),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::Unsupported) => (None, SaveStatus::Fresh),
        Err(e) => damaged(storage, format!("{:?}", e.kind())),
    }
}

/// The settings file, or the defaults.
pub fn read_settings(storage: &Storage) -> Settings {
    let mut settings = storage.read_json::<Settings>(SETTINGS_FILE).unwrap_or_default();
    settings.quality = settings.quality.filter(|&q| q < GraphicsQuality::ALL.len());
    settings.volume = if settings.volume.is_finite() {
        settings.volume.clamp(0.0, 1.0)
    } else {
        0.8
    };
    settings
}

fn write_settings(ctx: &mut Ctx) {
    let settings = *ctx.world.resource::<Settings>();
    if let Err(e) = ctx.storage.write_json(SETTINGS_FILE, &settings)
        && e.kind() != ErrorKind::Unsupported
    {
        notice(ctx, format!("Couldn't save the settings: {e}"));
    }
}

/// The settings to the renderer and the mixer: the chosen preset, or the
/// one recommended for this GPU (`Low` on the web, `Medium` without a GPU).
pub fn apply_settings(ctx: &mut Ctx) {
    let settings = *ctx.world.resource::<Settings>();
    match settings.quality {
        Some(q) => ctx.scene3d.set_quality(GraphicsQuality::ALL[q]),
        None if cfg!(target_arch = "wasm32") => ctx.scene3d.set_quality(GraphicsQuality::Low),
        None => match ctx.gfx.gpu_capabilities() {
            Some(caps) => ctx.scene3d.set_recommended_quality(caps),
            None => ctx.scene3d.set_quality(GraphicsQuality::Medium),
        },
    }
    let buses = *ctx.world.resource::<Buses>();
    ctx.audio.set_bus_volume(buses.sfx, settings.volume);
    ctx.audio.set_bus_volume(buses.ambience, settings.volume * 0.6);
}

/// The preset's name for the settings menu.
pub fn quality_name(ctx: &Ctx) -> &'static str {
    const NAMES: [&str; 4] = ["Low", "Medium", "High", "Ultra"];
    match ctx.scene3d.quality() {
        Some(q) => GraphicsQuality::ALL
            .iter()
            .position(|&p| p == q)
            .map_or("Custom", |i| NAMES[i]),
        None => "Recommended",
    }
}

// ---- layer 3: presentation ---------------------------------------------------------

/// Layer 3: the camera, the shards' spin, the gate's glow, the hum and the
/// light. It reads the game and changes none of it.
pub struct Presentation;

impl Plugin for Presentation {
    fn build(&self, game: Game) -> Game {
        game.system(present)
    }
    fn setup(&self, ctx: &mut Ctx) {
        // A late-afternoon sun with shadows over a hazy sky.
        let lighting = &mut ctx.scene3d.lighting;
        lighting.sun_direction = vec3(-0.45, -0.7, -0.35);
        lighting.sun_color = Color::rgb(1.0, 0.88, 0.72);
        lighting.ambient_intensity = 0.35;
        ctx.scene3d.camera.position = vec3(0.0, 20.0, 34.0);
        ctx.scene3d.camera.look_at(Vec3::ZERO);
    }
}

/// Frame system: everything that only shows the game.
pub fn present(ctx: &mut Ctx) {
    let screen = *ctx.world.resource::<Screen>();
    let time = ctx.time.elapsed() as f32;
    let dt = ctx.time.dt();
    let view = *ctx.world.resource::<View>();

    // The camera: behind the warden, or circling the grove on the title.
    match warden(&mut ctx.world) {
        Some((_, at)) => {
            let behind = Quat::from_rotation_y(view.yaw) * vec3(0.0, 3.4, 7.0);
            let eye = at + behind;
            let cam = &mut ctx.scene3d.camera;
            let blend = 1.0 - (-10.0 * dt).exp();
            cam.position = cam.position.lerp(eye, blend);
            cam.look_at(at + Vec3::Y * 1.4);
        }
        None => {
            let angle = time * 0.05;
            let cam = &mut ctx.scene3d.camera;
            cam.position = vec3(angle.sin() * 30.0, 18.0, angle.cos() * 30.0);
            cam.look_at(Vec3::ZERO);
        }
    }
    // Shards spin and bob.
    ctx.world.query::<(&mut Transform, &Shard)>().for_each(|(t, s)| {
        let base = SHARD_SPOTS[usize::from(s.id)];
        t.position = base + Vec3::Y * (1.0 + 0.15 * (time * 2.0 + f32::from(s.id)).sin());
        t.rotation = Quat::from_rotation_y(time * 1.5);
    });
    // The pulse's sparks stay with the warden.
    if let Some((_, at)) = warden(&mut ctx.world) {
        ctx.world.query::<(&mut Transform, &PulseSparks)>().for_each(|(t, _)| {
            t.position = at + Vec3::Y;
        });
    }
    // The gate glows once open.
    let open = ctx.world.resource::<Run>().complete() && screen != Screen::Title;
    ctx.world
        .query::<(&mut Transform, &mut PointLight, &GateGlow)>()
        .for_each(|(t, light, _)| {
            t.scale = if open { Vec3::ONE } else { Vec3::ZERO };
            light.intensity = if open { 10.0 + 2.0 * (time * 3.0).sin() } else { 0.0 };
        });
    // The crawlers hum only during play.
    let volume = if screen == Screen::Playing { 0.5 } else { 0.0 };
    ctx.world
        .query::<&mut AudioSource>()
        .for_each(|s| s.settings.sound.volume = volume);
}

// ---- layer 4: screens --------------------------------------------------------------

/// The menus' and the HUD's nodes.
#[derive(Clone, Copy)]
pub struct ScreenUi {
    pub title: UiId,
    pub new_game: UiId,
    pub continue_game: UiId,
    pub title_settings: UiId,
    pub quit: UiId,
    pub records: UiId,
    pub title_notice: UiId,
    pub pause: UiId,
    pub resume: UiId,
    pub pause_settings: UiId,
    pub to_title: UiId,
    pub settings: UiId,
    pub quality: UiId,
    pub volume: UiId,
    pub settings_back: UiId,
    pub won: UiId,
    pub won_text: UiId,
    pub won_ok: UiId,
    pub lost: UiId,
    pub try_again: UiId,
    pub lost_title: UiId,
    pub hud: UiId,
    pub health: UiId,
    pub shards: UiId,
    pub pulse: UiId,
    pub message: UiId,
}

/// The panel over the gate, in the world.
#[derive(Component, Debug, Clone, Copy)]
pub struct GateSign {
    pub text: UiId,
}

/// Layer 4: menus, HUD and the sign over the gate.
pub struct Screens;

impl Plugin for Screens {
    fn build(&self, game: Game) -> Game {
        game.system(menus).add(Phase::Late, update_screens)
    }
    fn setup(&self, ctx: &mut Ctx) {
        let ids = build_screens(ctx);
        ctx.world.insert_resource(ids);
        // The sign: retained UI drawn on a panel above the gate.
        let (mut panel, model) = ctx.gfx.create_ui_panel(480, 120, vec2(3.6, 0.9));
        let ui = &mut panel.ui;
        let root = ui.root().layout(UiLayout::fill().padding(12.0)).id();
        let text = ui.title(root, "").layout(UiLayout::fill()).name("gate.sign").id();
        ui.push_screen(root, false);
        ctx.world
            .spawn((Transform::at(vec3(0.0, 5.2, -21.1)), model, panel, GateSign { text }));
    }
}

fn build_screens(ctx: &mut Ctx) -> ScreenUi {
    let ui = &mut ctx.ui;
    let menu = |ui: &mut ponko::ui::Ui, name: &str, title: &str, height: f32| {
        let root = ui
            .root()
            .layout(UiLayout::centered(440.0, height).padding(24.0).gap(10.0))
            .name(name)
            .id();
        ui.title(root, title).layout(UiLayout::row(44.0));
        root
    };
    let title = menu(ui, "title", TITLE, 420.0);
    ui.label(title, "Find the six shards, wake the shrines, and leave by the gate.")
        .layout(UiLayout::row(44.0))
        .wrap();
    let new_game = ui
        .button(title, "New game")
        .layout(UiLayout::row(44.0))
        .name("title.new")
        .focused()
        .id();
    let continue_game = ui
        .button(title, "Continue")
        .layout(UiLayout::row(44.0))
        .name("title.continue")
        .id();
    let title_settings = ui
        .button(title, "Settings")
        .layout(UiLayout::row(44.0))
        .name("title.settings")
        .id();
    let quit = ui
        .button(title, "Quit")
        .layout(UiLayout::row(44.0))
        .name("title.quit")
        .id();
    if cfg!(target_arch = "wasm32") {
        ui.set_layout(quit, UiLayout::row(44.0).hidden()); // a page can't close itself
    }
    let records = ui
        .label(title, "")
        .layout(UiLayout::row(24.0))
        .name("title.records")
        .id();
    let title_notice = ui
        .label(title, "")
        .layout(UiLayout::row(24.0))
        .name("title.notice")
        .id();

    let pause = menu(ui, "pause", "Paused", 280.0);
    let resume = ui
        .button(pause, "Resume")
        .layout(UiLayout::row(44.0))
        .name("pause.resume")
        .focused()
        .id();
    let pause_settings = ui
        .button(pause, "Settings")
        .layout(UiLayout::row(44.0))
        .name("pause.settings")
        .id();
    let to_title = ui
        .button(pause, "Quit to title")
        .layout(UiLayout::row(44.0))
        .name("pause.title")
        .id();

    let settings = menu(ui, "settings", "Settings", 280.0);
    let quality = ui
        .button(settings, "")
        .layout(UiLayout::row(44.0))
        .name("settings.quality")
        .focused()
        .id();
    let volume = ui
        .button(settings, "")
        .layout(UiLayout::row(44.0))
        .name("settings.volume")
        .id();
    let settings_back = ui
        .button(settings, "Back")
        .layout(UiLayout::row(44.0))
        .name("settings.back")
        .id();

    let won = menu(ui, "won", "Out of the grove!", 220.0);
    let won_text = ui.label(won, "").layout(UiLayout::row(28.0)).name("won.text").id();
    let won_ok = ui
        .button(won, "Back to the title")
        .layout(UiLayout::row(44.0))
        .name("won.ok")
        .focused()
        .id();

    let lost = menu(ui, "lost", "The crawlers got you", 220.0);
    let try_again = ui
        .button(lost, "Try again")
        .layout(UiLayout::row(44.0))
        .name("lost.again")
        .focused()
        .id();
    let lost_title = ui
        .button(lost, "Quit to title")
        .layout(UiLayout::row(44.0))
        .name("lost.title")
        .id();

    let hud = ui
        .root()
        .layout(UiLayout::fill().padding(16.0).gap(4.0))
        .name("hud")
        .id();
    let health = ui
        .label(hud, "")
        .layout(UiLayout::fixed(360.0, 28.0))
        .name("hud.health")
        .id();
    let shards = ui
        .label(hud, "")
        .layout(UiLayout::fixed(360.0, 28.0))
        .name("hud.shards")
        .id();
    let pulse = ui
        .label(hud, "")
        .layout(UiLayout::fixed(360.0, 28.0))
        .name("hud.pulse")
        .id();
    let message = ui
        .label(hud, "")
        .layout(UiLayout::fixed(600.0, 28.0))
        .name("hud.message")
        .id();
    ScreenUi {
        title,
        new_game,
        continue_game,
        title_settings,
        quit,
        records,
        title_notice,
        pause,
        resume,
        pause_settings,
        to_title,
        settings,
        quality,
        volume,
        settings_back,
        won,
        won_text,
        won_ok,
        lost,
        try_again,
        lost_title,
        hud,
        health,
        shards,
        pulse,
        message,
    }
}

/// Frame system: clicks (and Escape) on the menus to [`Menu`] choices.
pub fn menus(ctx: &mut Ctx) {
    let ids = *ctx.world.resource::<ScreenUi>();
    let mut choice = None;
    for event in ctx.ui.events() {
        choice = match event {
            UiEvent::Clicked(id) if id == ids.new_game => Some(Menu::NewGame),
            UiEvent::Clicked(id) if id == ids.continue_game => Some(Menu::Continue),
            UiEvent::Clicked(id) if id == ids.title_settings || id == ids.pause_settings => Some(Menu::Settings),
            UiEvent::Clicked(id) if id == ids.quit => Some(Menu::Quit),
            UiEvent::Clicked(id) if id == ids.resume => Some(Menu::Resume),
            UiEvent::Clicked(id) if id == ids.to_title || id == ids.lost_title => Some(Menu::ToTitle),
            UiEvent::Clicked(id) if id == ids.quality => Some(Menu::CycleQuality),
            UiEvent::Clicked(id) if id == ids.volume => Some(Menu::CycleVolume),
            UiEvent::Clicked(id) if id == ids.settings_back || id == ids.won_ok => Some(Menu::Back),
            UiEvent::Clicked(id) if id == ids.try_again => Some(Menu::TryAgain),
            // Escape closes a menu: back, or resume.
            UiEvent::ScreenPopped(_) => Some(Menu::Back),
            _ => choice,
        };
    }
    if let Some(choice) = choice {
        ctx.world.resource_mut::<Intent>().menu = Some(choice);
    }
}

/// Late system: the screen stack, the texts and the HUD as the game is now.
pub fn update_screens(ctx: &mut Ctx) {
    let ids = *ctx.world.resource::<ScreenUi>();
    let screen = *ctx.world.resource::<Screen>();
    let want = match screen {
        Screen::Title => Some(ids.title),
        Screen::Playing => None,
        Screen::Paused => Some(ids.pause),
        Screen::Settings { .. } => Some(ids.settings),
        Screen::Won => Some(ids.won),
        Screen::Lost => Some(ids.lost),
    };
    if ctx.ui.top_screen() != want {
        while ctx.ui.top_screen().is_some() {
            ctx.ui.pop_screen();
        }
        if let Some(root) = want {
            ctx.ui.push_screen(root, true);
        }
    }
    let in_run = matches!(screen, Screen::Playing | Screen::Paused);
    if in_run && !ctx.ui.is_hud(ids.hud) {
        ctx.ui.show_hud(ids.hud);
    } else if !in_run && ctx.ui.is_hud(ids.hud) {
        ctx.ui.hide_hud(ids.hud);
    }

    let run = ctx.world.resource::<Run>().clone();
    let notice = ctx.world.resource::<Notice>().clone();
    let message = if notice.seconds > 0.0 {
        notice.text.clone()
    } else {
        String::new()
    };
    let cooldown = warden(&mut ctx.world)
        .and_then(|(e, _)| ctx.world.get::<Warden>(e).map(|w| w.cooldown))
        .unwrap_or(0.0);
    let has_save = has_save(&ctx.storage);
    let records = ctx.world.resource::<Records>().clone();
    let settings = *ctx.world.resource::<Settings>();
    let quality = quality_name(ctx);
    let ui = &mut ctx.ui;
    ui.set_text(ids.health, format!("Health {} / {MAX_HEALTH}", run.health));
    ui.set_text(
        ids.shards,
        format!("Shards {} / {}", run.shards.len(), SHARD_SPOTS.len()),
    );
    ui.set_text(
        ids.pulse,
        if cooldown > 0.0 {
            format!("Pulse in {cooldown:.1} s")
        } else {
            "Pulse ready (F)".to_string()
        },
    );
    ui.set_text(ids.message, &message);
    ui.set_enabled(ids.continue_game, has_save);
    ui.set_text(
        ids.records,
        match records.best_time {
            Some(best) => format!("Best time {best:.1} s, {} wins", records.wins),
            None => String::new(),
        },
    );
    ui.set_text(ids.title_notice, &message);
    ui.set_text(ids.quality, format!("Quality: {quality}"));
    ui.set_text(ids.volume, format!("Volume: {:.0}%", settings.volume * 100.0));
    ui.set_text(ids.won_text, format!("{:.1} seconds", run.time));

    let sign = if run.complete() {
        "The gate is open".to_string()
    } else {
        format!("Shards {} / {}", run.shards.len(), SHARD_SPOTS.len())
    };
    ctx.world.query::<(&mut UiPanel, &GateSign)>().for_each(|(panel, s)| {
        if panel.ui.text(s.text) != Some(sign.as_str()) {
            panel.ui.set_text(s.text, &sign);
        }
    });
}

// ---- layer 5: ambience -------------------------------------------------------------

/// Layer 5: the wind, looping on its own bus from the start.
pub struct Ambience;

impl Plugin for Ambience {
    fn build(&self, game: Game) -> Game {
        game
    }
    fn setup(&self, ctx: &mut Ctx) {
        let wind = ctx.world.resource::<GroveAssets>().sounds.wind;
        let bus = ctx.world.resource::<Buses>().ambience;
        ctx.audio.play_on(
            wind,
            SoundParams {
                looping: true,
                volume: 0.5,
                ..Default::default()
            },
            bus,
            AudioEffects::default(),
        );
    }
}
