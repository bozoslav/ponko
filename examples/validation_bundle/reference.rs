//! The reference interaction: hold a jump button for a long time.
//!
//! A tick-indexed input scenario presses the "jump" action at tick 10 and
//! releases it at tick 110. A correct game jumps once for that press. The
//! deliberately broken variant jumps whenever the button is *held* and the
//! body is on the ground, so it bounces again after landing.

use crate::bundle::Assertion;
#[allow(unused_imports)]
use ponko::actions::InputEvent;
use ponko::prelude::*;
#[allow(unused_imports)]
use ponko::scenario::InputScenario;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

pub const SCENARIO_ID: &str = "hold-jump";
pub const TICKS: u64 = 120;
/// (tick, pressed) steps of the "jump" key.
pub const STEPS: [(u64, bool); 2] = [(10, true), (110, false)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    /// Jumps on the pressed edge of the action.
    Fixed,
    /// Jumps while the action is held: the bug.
    Buggy,
}

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Variant::Fixed => "fixed",
            Variant::Buggy => "buggy",
        }
    }
}

#[derive(Clone, Copy, Component)]
struct Body {
    y: f32,
    vy: f32,
    jumps: u32,
}

/// What was observed on one tick, after the game systems ran.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub tick: u64,
    pub pressed: bool,
    pub down: bool,
    pub y: f32,
    pub vy: f32,
    pub jumps: u32,
}

/// Every tick's [`Row`], kept in the world. Reflected, so a tool that
/// drives a launched copy of the game reads the same trace the in-process
/// run returns.
#[derive(Default, Serialize, Deserialize)]
pub struct Trace {
    pub rows: Vec<Row>,
}

pub struct Run {
    pub trace: Vec<Row>,
    pub log: Vec<String>,
    pub assertions: Vec<Assertion>,
    /// The profiler's report as JSON, when the run was profiled.
    pub perf: Option<String>,
}

pub fn scenario() -> InputScenario {
    STEPS.iter().fold(InputScenario::new(), |s, &(tick, pressed)| {
        s.at(tick, InputEvent::Key(Key::Space, pressed))
    })
}

/// The first line of every run's log.
pub fn banner(variant: Variant, seed: u64) -> String {
    format!(
        "scenario {SCENARIO_ID}: variant={} seed={seed} ticks={TICKS}",
        variant.name()
    )
}

/// The reference game: one body that jumps on the "jump" action (Space) and
/// a [`Trace`] of every tick. `log` gets a line for each jump.
fn game(
    variant: Variant,
    seed: u64,
    scenario: Option<InputScenario>,
    log: impl Fn(String) + 'static,
) -> ponko::prelude::ConfiguredGame {
    let game = Game::new("validation-reference", 320, 240).seed(seed);
    let game = match scenario {
        Some(scenario) => game.input_scenario(scenario),
        None => game,
    };
    game.sim_system(move |ctx: &mut Ctx| {
        let dt = ctx.time.dt();
        let tick = ctx.time.sim_tick();
        let want_jump = match variant {
            Variant::Fixed => ctx.input.action_pressed("jump"),
            Variant::Buggy => ctx.input.action_down("jump"),
        };
        let drift = ctx.random().stream(0).range(-0.01, 0.01);
        ctx.world.query::<&mut Body>().for_each(|b| {
            if want_jump && b.y <= 0.0 {
                b.vy = 5.0;
                b.jumps += 1;
                log(format!("tick {tick}: jump #{} started", b.jumps));
            }
            b.vy -= 9.81 * dt;
            b.y = (b.y + (b.vy + drift) * dt).max(0.0);
        });
    })
    .sim_system(|ctx: &mut Ctx| {
        let tick = ctx.time.sim_tick();
        let (pressed, down) = (ctx.input.action_pressed("jump"), ctx.input.action_down("jump"));
        let body = *ctx.world.query::<&Body>().iter().next().expect("the body exists");
        ctx.world.resource_mut::<Trace>().rows.push(Row {
            tick,
            pressed,
            down,
            y: body.y,
            vy: body.vy,
            jumps: body.jumps,
        });
    })
    // Only a windowed copy draws: the ground and the body, so a
    // screenshot shows where the body is.
    .system(|ctx: &mut Ctx| {
        let size = ctx.window_size();
        let y = ctx.world.query::<&Body>().iter().next().map_or(0.0, |b| b.y);
        let ground = size.y - 40.0;
        ctx.gfx
            .draw_rect(Rect::new(0.0, ground, size.x, 4.0), Color::rgb(0.5, 0.5, 0.5));
        ctx.gfx.draw_rect(
            Rect::new(size.x * 0.5 - 10.0, ground - 20.0 - y * 60.0, 20.0, 20.0),
            Color::rgb(0.9, 0.6, 0.2),
        );
    })
    .setup(|ctx| {
        let mut map = ActionMap::default();
        map.set("jump", Action::Button(vec![ButtonBinding::Key(Key::Space)]));
        ctx.input.set_actions(map);
        ctx.world.reflect_resource::<Trace>();
        ctx.world.insert_resource(Trace::default());
        ctx.world.spawn((Body {
            y: 0.0,
            vy: 0.0,
            jumps: 0,
        },));
    })
}

/// Runs the scenario headlessly, in this process, and checks it. With
/// `profile`, the run is profiled and its report comes back as JSON.
pub fn run(variant: Variant, seed: u64, profile: bool) -> Run {
    let log = Rc::new(RefCell::new(vec![banner(variant, seed)]));
    let jump_log = log.clone();
    let game = game(variant, seed, Some(scenario()), move |line| {
        jump_log.borrow_mut().push(line)
    });
    let (world, perf) = if profile {
        let (world, report) = game.profile_headless(0, TICKS);
        (world, Some(report.to_json()))
    } else {
        (game.run_headless(TICKS), None)
    };
    let trace = world.resource::<Trace>().rows.clone();
    drop(world);
    let mut log = Rc::try_unwrap(log).expect("systems dropped").into_inner();
    let assertions = check(&trace);
    log.extend(assertion_lines(&assertions));
    Run {
        trace,
        log,
        assertions,
        perf,
    }
}

/// The log's line for each jump, read off a trace: what the game itself
/// logs when it runs in this process.
pub fn jump_lines(trace: &[Row]) -> Vec<String> {
    let mut jumps = 0;
    let mut lines = Vec::new();
    for row in trace {
        while jumps < row.jumps {
            jumps += 1;
            lines.push(format!("tick {}: jump #{jumps} started", row.tick));
        }
    }
    lines
}

/// The log's line for each assertion.
pub fn assertion_lines(assertions: &[Assertion]) -> Vec<String> {
    assertions
        .iter()
        .map(|a| {
            format!(
                "assertion {}: {}{}",
                a.id,
                if a.passed { "passed" } else { "FAILED" },
                a.tick.map(|t| format!(" at tick {t}")).unwrap_or_default()
            )
        })
        .collect()
}

/// Runs the reference game as a process a tool can drive: the engine's
/// control endpoint, input injection and the paused start come from the
/// environment the launcher sets (`PONKO_CONTROL`, `PONKO_INPUT_INJECTION`,
/// `PONKO_START_PAUSED`). Headless when `HEADLESS` is set (the launcher's
/// default), in a window otherwise. Its jump lines go to stdout. Ends when
/// the tool asks it to quit, or after `give_up`, so a tool that died doesn't
/// leave it running.
/// How long a served copy waits to be told to quit before it gives up.
/// Tied to the driver: longer than the driver's own limits add up to (600 s
/// to appear, then 120 s for each of its two long calls), so on a loaded
/// machine the driver's timeout is the one that ends a run and names why,
/// never this one.
#[allow(dead_code)] // Used by the example's `--serve` and by the test binary's served entry.
pub const SERVE_GIVE_UP: std::time::Duration = std::time::Duration::from_secs(900);

#[allow(dead_code)] // The example's `--serve`; a test binary serves through its own entry.
pub fn serve(variant: Variant, seed: u64, give_up: std::time::Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(give_up);
        eprintln!("validation-reference: no tool asked this game to quit; giving up");
        std::process::exit(3);
    });
    println!("{}", banner(variant, seed));
    // The tool that launched this copy feeds the scenario in.
    let game = game(variant, seed, None, |line| println!("{line}"));
    if std::env::var_os("HEADLESS").is_some() {
        game.run_headless(u64::MAX);
    } else {
        game.run();
    }
}

/// The checks every run of the reference interaction is held to, on its trace.
pub fn check(trace: &[Row]) -> Vec<Assertion> {
    let (press_tick, release_tick) = (STEPS[0].0, STEPS[1].0);
    let press_seen = trace.iter().find(|r| r.pressed).map(|r| r.tick);
    let apex = trace.iter().map(|r| r.y).fold(0.0f32, f32::max);
    let extra_jump = trace.iter().find(|r| r.jumps > 1);
    let last = trace.last().expect("the run has ticks");
    vec![
        Assertion {
            id: "scenario-press-applied".into(),
            description: "the scenario's key press reaches the jump action on exactly its tick".into(),
            passed: press_seen == Some(press_tick),
            tick: press_seen.or(Some(press_tick)),
            expected: format!("pressed edge at tick {press_tick}"),
            actual: press_seen.map_or("no pressed edge".into(), |t| format!("pressed edge at tick {t}")),
        },
        Assertion {
            id: "jump-reaches-apex".into(),
            description: "a jump rises at least 1 unit".into(),
            passed: apex >= 1.0,
            tick: None,
            expected: "apex >= 1.0".into(),
            actual: format!("{apex:.3}"),
        },
        Assertion {
            id: "one-jump-per-press".into(),
            description: format!("holding the button from tick {press_tick} to {release_tick} makes one jump"),
            passed: extra_jump.is_none(),
            tick: extra_jump.map(|r| r.tick),
            expected: "1 jump".into(),
            actual: format!("{} jumps", extra_jump.map_or(last.jumps, |r| r.jumps)),
        },
        Assertion {
            id: "lands-by-the-end".into(),
            description: "the body is back on the ground when the run ends".into(),
            passed: last.y == 0.0,
            tick: Some(last.tick),
            expected: "y == 0".into(),
            actual: format!("{:.3}", last.y),
        },
    ]
}
