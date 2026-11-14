//! The game playing itself: the packaged-game smoke test, the full
//! playthrough test, and the web build's state report.
//!
//! `shard_grove --smoke headless|window` starts a new game, walks to three
//! shards with [`autopilot`] (paths on the crawlers' navigation mesh, a
//! pulse whenever a crawler comes close), saves at a shrine, quits to the
//! title and continues the saved game, finds the other shards, leaves by
//! the gate and goes back to the title. It writes a JSON [`Report`] of what
//! it saw and exits with 0 only if it got there.

use super::*;
use std::path::PathBuf;

/// What happened, written as JSON when the walk-through ends.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    /// `headless` or `window`.
    pub mode: String,
    /// The screens in the order the game showed them.
    pub screens: Vec<String>,
    pub tally: Tally,
    /// Seconds the winning run took.
    pub time: f32,
    /// The quality preset in use at the start.
    pub quality: String,
    /// The folder `ctx.storage` kept the files in (`None`: none kept, or the web).
    pub storage_dir: Option<String>,
    /// The warden's joints and the corners of the navigation path to the
    /// gate: the packaged game found and read its files.
    pub warden_joints: usize,
    pub path_corners: usize,
    /// Frames drawn (0 headless).
    pub frames: u64,
    pub ticks: u64,
    pub finished: bool,
}

/// The walk-through's progress. A resource.
pub struct Smoke {
    report_path: Option<PathBuf>,
    report: Report,
    step: Step,
    since: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// On the title: start a new game.
    Start,
    /// Find three shards.
    CollectFirst,
    /// Walk to a shrine until it saves.
    Save,
    /// Pause.
    Pause,
    /// Paused: quit to the title (the save stays).
    Leave,
    /// On the title: continue the saved game.
    Continue,
    /// Find the rest, then walk through the gate.
    Finish,
    /// On the end screen: back to the title.
    Done,
    /// Back on the title: report.
    Report,
}

/// Ticks on a screen before acting.
const DWELL: u64 = 20;
/// Shards found before the walk-through saves.
const FIRST_SHARDS: usize = 3;

/// The walk-through gives up after this many ticks (four minutes of play at
/// the 50 Hz of 3D physics), reporting `finished: false`.
pub const MAX_TICKS: u64 = 4 * 60 * 50;

pub fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::Title => "Title",
        Screen::Playing => "Playing",
        Screen::Paused => "Paused",
        Screen::Settings { .. } => "Settings",
        Screen::Won => "Won",
        Screen::Lost => "Lost",
    }
}

/// Startup for the walk-through, after the game's own.
pub fn setup_smoke(ctx: &mut Ctx, report_path: Option<PathBuf>, mode: &str) {
    let warden_joints = ctx.world.resource::<GroveAssets>().warden_skeleton.joints().len();
    let path_corners = ctx
        .world
        .resource::<NavMesh>()
        .find_path(START, GATE)
        .map_or(0, |p| p.len());
    let report = Report {
        mode: mode.to_string(),
        quality: quality_name(ctx).to_string(),
        storage_dir: ctx.storage.dir().map(|d| d.display().to_string()),
        warden_joints,
        path_corners,
        ..Report::default()
    };
    ctx.world.insert_resource(Smoke {
        report_path,
        report,
        step: Step::Start,
        since: 0,
    });
}

impl Smoke {
    pub fn finished(&self) -> bool {
        self.report.finished
    }
    pub fn report(&self) -> &Report {
        &self.report
    }
}

fn menu(ctx: &mut Ctx, choice: Menu) {
    ctx.world.resource_mut::<Intent>().menu = Some(choice);
}

/// Sim system (before the rules): the walk-through's player.
pub fn walkthrough(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    let screen = *ctx.world.resource::<Screen>();
    let (step, since) = {
        let smoke = ctx.world.resource_mut::<Smoke>();
        let name = screen_name(screen).to_string();
        if smoke.report.screens.last() != Some(&name) {
            smoke.report.screens.push(name);
        }
        (smoke.step, smoke.since)
    };
    let waited = tick - since >= DWELL;
    let found = ctx.world.resource::<Run>().shards.len();
    let saves = ctx.world.resource::<Tally>().saves;
    let mut next = step;
    // Lost: try again from the last save, wherever the walk-through was.
    if screen == Screen::Lost {
        if waited {
            menu(ctx, Menu::TryAgain);
            ctx.world.resource_mut::<Smoke>().since = tick;
        }
        return;
    }
    match step {
        Step::Start if screen == Screen::Title && waited => {
            menu(ctx, Menu::NewGame);
            next = Step::CollectFirst;
        }
        Step::CollectFirst if found >= FIRST_SHARDS => next = Step::Save,
        Step::CollectFirst => autopilot(ctx, Goal::Shard),
        Step::Save if saves > 0 => next = Step::Pause,
        Step::Save => autopilot(ctx, Goal::Shrine),
        Step::Pause if waited => {
            ctx.world.resource_mut::<Intent>().pause = true;
            next = Step::Leave;
        }
        Step::Leave if screen == Screen::Paused && waited => {
            menu(ctx, Menu::ToTitle);
            next = Step::Continue;
        }
        Step::Continue if screen == Screen::Title && waited => {
            menu(ctx, Menu::Continue);
            next = Step::Finish;
        }
        Step::Finish if screen == Screen::Won => next = Step::Done,
        Step::Finish if found < SHARD_SPOTS.len() => autopilot(ctx, Goal::Shard),
        Step::Finish => autopilot(ctx, Goal::Gate),
        Step::Done if waited => {
            menu(ctx, Menu::Back);
            next = Step::Report;
        }
        Step::Report if screen == Screen::Title => finish(ctx, tick, true),
        _ => {}
    }
    if tick >= MAX_TICKS {
        finish(ctx, tick, false);
    }
    if next != step {
        let smoke = ctx.world.resource_mut::<Smoke>();
        smoke.step = next;
        smoke.since = tick;
    }
}

/// Where the autopilot walks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    /// The closest shard not yet found.
    Shard,
    /// The closest shrine.
    Shrine,
    /// The gate.
    Gate,
}

// ANCHOR: autopilot
/// A simple player: follows a path on the navigation mesh to its goal,
/// turning it into the stick's direction relative to the view, and pulses
/// whenever a crawler that isn't stunned comes close.
pub fn autopilot(ctx: &mut Ctx, goal: Goal) {
    if *ctx.world.resource::<Screen>() != Screen::Playing {
        return;
    }
    let Some((_, at)) = warden(&mut ctx.world) else {
        return;
    };
    let target = match goal {
        Goal::Shard => ctx
            .world
            .query::<(&Transform, &Shard)>()
            .iter()
            .map(|(_, s)| SHARD_SPOTS[usize::from(s.id)])
            .min_by(|a, b| flat_distance(*a, at).total_cmp(&flat_distance(*b, at))),
        Goal::Shrine => SHRINE_SPOTS
            .into_iter()
            .min_by(|a, b| flat_distance(*a, at).total_cmp(&flat_distance(*b, at))),
        Goal::Gate => Some(GATE),
    };
    let Some(target) = target else {
        return;
    };
    let nav = ctx.world.resource::<NavMesh>();
    let from = nav.sample_position(at, 2.0).unwrap_or(at);
    let corner = nav
        .find_path(from, target)
        .and_then(|path| path.into_iter().find(|c| flat_distance(*c, at) > 0.3))
        .unwrap_or(target);
    let toward = vec3(corner.x - at.x, 0.0, corner.z - at.z).normalize_or_zero();
    // The stick is relative to the view: turn the world direction into it.
    let yaw = ctx.world.resource::<View>().yaw;
    let local = Quat::from_rotation_y(-yaw) * toward;
    let danger = ctx
        .world
        .query::<(&Transform, &Crawler)>()
        .iter()
        .any(|(t, c)| c.stun <= 0.0 && flat_distance(t.position, at) < 3.5);
    let intent = ctx.world.resource_mut::<Intent>();
    intent.movement = vec2(local.x, -local.z);
    intent.pulse |= danger;
}
// ANCHOR_END: autopilot

/// Frame system: counts the frames drawn, for the report.
pub fn count_frames(ctx: &mut Ctx) {
    if let Some(smoke) = ctx.world.get_resource_mut::<Smoke>() {
        smoke.report.frames += 1;
    }
}

fn finish(ctx: &mut Ctx, tick: u64, completed: bool) {
    let tally = ctx.world.resource::<Tally>().clone();
    let time = ctx.world.resource::<Records>().best_time.unwrap_or(0.0);
    let smoke = ctx.world.resource_mut::<Smoke>();
    let report = &mut smoke.report;
    report.tally = tally;
    report.time = time;
    report.ticks = tick;
    report.finished = completed;
    let json = serde_json::to_string_pretty(&*report).expect("a report is plain data");
    match &smoke.report_path {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &json) {
                eprintln!("shard_grove: could not write the report {}: {e}", path.display());
            }
        }
        None => println!("{json}"),
    }
    ctx.quit();
}

/// The game with the walk-through installed, its files where `game` keeps them.
pub fn smoke_game(game: Game) -> Game {
    game.sim_system(walkthrough)
        .plugin(ShardGrove)
        .add(Phase::Frame, count_frames)
}

/// [`smoke_game`] with the files in the player's data folder, like the real game.
pub fn build_smoke() -> Game {
    smoke_game(Game::new(TITLE, WINDOW.0, WINDOW.1).app_id(APP_ID))
}

/// Frame system for the web build: the state on `<html data-smoke>`, in the
/// format of `examples/web_smoke.rs` plus the screen, for the browser runner
/// of `web/smoke` (and the web gallery).
#[cfg(target_arch = "wasm32")]
pub fn publish_web_state(ctx: &mut Ctx) {
    #[derive(Default)]
    struct Frames(u64);
    let mut frames = ctx.world.remove_resource::<Frames>().unwrap_or_default();
    frames.0 += 1;
    let joints = ctx.world.resource::<GroveAssets>().warden_skeleton.joints().len();
    let size = ctx.window_size();
    let run = ctx.world.resource::<Run>();
    let state = format!(
        r#"{{"ticks":{},"frames":{},"joints":{},"size":[{},{}],"screen":"{}","shards":{},"quality":"{}","storage":"{}"}}"#,
        ctx.time.sim_tick(),
        frames.0,
        joints,
        size.x,
        size.y,
        screen_name(*ctx.world.resource::<Screen>()),
        run.shards.len(),
        quality_name(ctx),
        if ctx.storage.is_persistent() {
            "persistent"
        } else {
            "memory"
        },
    );
    ctx.world.insert_resource(frames);
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute("data-smoke", &state);
    }
}
