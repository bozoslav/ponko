//! The packaged-game smoke test, and the web build's state report.
//!
//! `coin_dash --smoke headless|window` plays the game by itself, from the
//! title screen through the settings and a won round and back, and writes a JSON
//! [`Report`] of what it saw. `scripts/package-coin-dash.py smoke` unpacks the
//! package somewhere else, starts it from an unrelated folder and checks the
//! report, the saved files and the notices.
//!
//! The player here is [`autoplay`], a simulation system like the scripted
//! players in `tests/coin_dash.rs`: it writes the player's requests into
//! [`Intent`] and watches the [`Screen`]. In the headless mode the game runs
//! without a window or a GPU; in the window mode a real window opens, so the
//! asset loading, drawing and the retained UI screen are used for real.

use super::*;
use std::path::PathBuf;

/// What happened, written as JSON when the walk-through ends.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    /// `headless` or `window`.
    pub mode: String,
    /// The screens in the order the game showed them.
    pub screens: Vec<String>,
    pub rounds_played: u32,
    pub rounds_won: u32,
    pub best_time: Option<f32>,
    /// How the progress file was found at startup: `Fresh`, `Loaded` or `Recovered`.
    pub progress_at_start: String,
    /// The folder `ctx.storage` kept the files in (`None`: none kept, or the web).
    pub storage_dir: Option<String>,
    pub settings_at_start: String,
    /// The coin picture's file, and its size once loaded (the packaged game found it).
    pub asset: String,
    pub asset_size: [u32; 2],
    /// Frames drawn (0 headless).
    pub frames: u64,
    /// The simulation ticks the walk-through took.
    pub ticks: u64,
    pub finished: bool,
}

/// The walk-through's progress. A resource.
pub struct Smoke {
    report_path: Option<PathBuf>,
    report: Report,
    step: Step,
    /// The tick the current step began on.
    since: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// On the title screen: open the settings.
    OpenSettings,
    /// In the settings: leave them again.
    CloseSettings,
    /// On the title screen again: start a round.
    Start,
    /// Chase coins until the result screen.
    Play,
    /// On the result screen: back to the title.
    Leave,
    Done,
}

/// Ticks to stay on a screen before moving on, so each is shown for a few frames.
const DWELL: u64 = 20;

/// The walk-through gives up after this many ticks (ten rounds' worth of time),
/// reporting `finished: false`, so a stuck game ends instead of hanging a CI job.
pub const MAX_TICKS: u64 = 10 * ROUND_SECONDS as u64 * 60;

fn screen_name(screen: Screen) -> String {
    match screen {
        Screen::Result(outcome) => format!("Result({outcome:?})"),
        other => format!("{other:?}"),
    }
}

/// Startup for the walk-through, after the game's own (the plugins' setups
/// run first): the bookkeeping. The report goes to `report_path` when it ends
/// (`None`: only into the resource).
pub fn setup_smoke(ctx: &mut Ctx, report_path: Option<PathBuf>, mode: &str) {
    let size = {
        let coin = ctx.world.resource::<Art>().coin;
        [coin.width(), coin.height()]
    };
    let report = Report {
        mode: mode.to_string(),
        progress_at_start: format!("{:?}", ctx.world.resource::<SaveFile>().load),
        storage_dir: ctx.storage.dir().map(|d| d.display().to_string()),
        settings_at_start: format!("{:?}", ctx.world.resource::<settings::SettingsFile>().load),
        asset: COIN_ASSET.to_string(),
        asset_size: size,
        ..Report::default()
    };
    ctx.world.insert_resource(Smoke {
        report_path,
        report,
        step: Step::OpenSettings,
        since: 0,
    });
}

impl Smoke {
    /// The walk-through reached its end (rather than giving up at [`MAX_TICKS`]).
    pub fn finished(&self) -> bool {
        self.report.finished
    }

    pub fn report(&self) -> &Report {
        &self.report
    }
}

/// Sim system (runs before `rules`): the player of the walk-through.
pub fn autoplay(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    let screen = *ctx.world.resource::<Screen>();
    let (step, since) = {
        let smoke = ctx.world.resource_mut::<Smoke>();
        let name = screen_name(screen);
        if smoke.report.screens.last() != Some(&name) {
            smoke.report.screens.push(name);
        }
        (smoke.step, smoke.since)
    };
    let waited = tick - since >= DWELL;
    let mut next = step;
    match step {
        Step::OpenSettings if screen == Screen::Title && waited => {
            ctx.world.resource_mut::<Intent>().settings = true;
            next = Step::CloseSettings;
        }
        Step::CloseSettings if screen == Screen::Settings && waited => {
            ctx.world.resource_mut::<Intent>().close_settings = true;
            next = Step::Start;
        }
        Step::Start if screen == Screen::Title && waited => {
            ctx.world.resource_mut::<Intent>().confirm = true;
            next = Step::Play;
        }
        Step::Play => match screen {
            Screen::Playing => chase(ctx),
            Screen::Result(_) => next = Step::Leave,
            _ => {}
        },
        Step::Leave if waited => {
            ctx.world.resource_mut::<Intent>().back = true;
            next = Step::Done;
        }
        Step::Done if screen == Screen::Title => finish(ctx, tick, true),
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

/// Walks straight to the nearest coin.
fn chase(ctx: &mut Ctx) {
    let mut player = Vec2::ZERO;
    for (t, _) in ctx.world.query::<(&d2::Transform, &Player)>().iter() {
        player = t.position;
    }
    let nearest = ctx
        .world
        .query::<(&d2::Transform, &Coin)>()
        .iter()
        .map(|(t, _)| t.position)
        .min_by(|a, b| a.distance(player).total_cmp(&b.distance(player)));
    ctx.world.resource_mut::<Intent>().direction = nearest.map_or(Vec2::ZERO, |c| (c - player).normalize_or_zero());
}

/// Frame system: counts the frames drawn, for the report.
pub fn count_frames(ctx: &mut Ctx) {
    ctx.world.resource_mut::<Smoke>().report.frames += 1;
}

fn finish(ctx: &mut Ctx, tick: u64, completed: bool) {
    let progress = ctx.world.resource::<Progress>().clone();
    let smoke = ctx.world.resource_mut::<Smoke>();
    let report = &mut smoke.report;
    report.rounds_played = progress.rounds_played;
    report.rounds_won = progress.rounds_won;
    report.best_time = progress.best_time;
    report.ticks = tick;
    report.finished = completed;
    let json = serde_json::to_string_pretty(&*report).expect("a report is plain data");
    match &smoke.report_path {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &json) {
                eprintln!("coin_dash: could not write the report {}: {e}", path.display());
            }
        }
        None => println!("{json}"),
    }
    ctx.quit();
}

/// The game with the walk-through installed: [`autoplay`] (before the
/// rules), the [`CoinDash`] plugin keeping its files in `ctx.storage`, and
/// [`count_frames`]. `game` names where the files go: [`build_smoke`] for the
/// player's data folder. Start it with [`setup_smoke`].
pub fn smoke_game(game: Game) -> Game {
    game.seed(SEED)
        .sim_system(autoplay)
        .plugin(CoinDash)
        .system(count_frames)
}

/// [`smoke_game`] with the files in the player's data folder, like the real game.
pub fn build_smoke() -> Game {
    smoke_game(Game::new("Coin Dash", ARENA.x as u32, ARENA.y as u32).app_id(APP_ID))
}

/// Frame system for the web build: publishes the state on `<html data-smoke>`
/// for `web/smoke/smoke.mjs`, in the format of `examples/web_smoke.rs` (plus
/// the screen), so the browser runner can check the game the same way.
#[cfg(target_arch = "wasm32")]
pub fn publish_web_state(ctx: &mut Ctx) {
    #[derive(Default)]
    struct Counts {
        frames: u64,
        clicks: u32,
    }
    let mut counts = ctx.world.remove_resource::<Counts>().unwrap_or_default();
    counts.frames += 1;
    if ctx.input.mouse_pressed(MouseButton::Left) {
        counts.clicks += 1;
    }
    let coin = ctx.world.resource::<Art>().coin;
    let size = ctx.window_size();
    let state = format!(
        r#"{{"ticks":{},"frames":{},"asset":[{},{}],"clicks":{},"size":[{},{}],"screen":"{}","settings_at_start":"{:?}","storage":"{}","saving":{}}}"#,
        ctx.time.sim_tick(),
        counts.frames,
        coin.width(),
        coin.height(),
        counts.clicks,
        size.x,
        size.y,
        screen_name(*ctx.world.resource::<Screen>()),
        ctx.world.resource::<settings::SettingsFile>().load,
        if ctx.storage.is_persistent() {
            "persistent"
        } else {
            "memory"
        },
        ctx.storage.is_saving(),
    );
    ctx.world.insert_resource(counts);
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute("data-smoke", &state);
        // What the engine says about the GPU it opened, once it is known.
        if !root.has_attribute("data-smoke-gpu")
            && let Some(gpu) = ctx
                .gfx
                .gpu_capabilities()
                .and_then(|gpu| serde_json::to_string(gpu).ok())
        {
            let _ = root.set_attribute("data-smoke-gpu", &gpu);
        }
    }
}
