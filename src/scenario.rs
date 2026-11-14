//! Tick-indexed input scenarios: scripted raw input and assertions, run
//! through the real input and action path, for reproducible gameplay tests
//! and for tools that drive a running game.
//!
//! A scenario is a list of steps keyed by simulation tick. Input steps feed
//! an [`InputEvent`] before that tick's simulation systems run; check steps
//! look at the game after they ran. A run reports its outcome in a
//! [`ScenarioReport`] kept in the world, with the failing step and tick when
//! a check fails. Scenarios also read from and write to a small versioned
//! JSON format (see [`InputScenario::from_json`]).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::actions::InputEvent;
use crate::ctx::Ctx;
use crate::input::Gamepad;
use crate::input::GamepadAxis;
use crate::input::GamepadButton;
use crate::input::{Key, MouseButton};
use crate::reflect::WorldReflect;
use crate::time::Time;
use ponko_ecs::World;

/// The scenario file format this engine reads and writes.
pub const SCENARIO_VERSION: u32 = 1;

/// Most steps one scenario holds, so a file can't ask for unbounded memory.
pub const MAX_SCENARIO_STEPS: usize = 100_000;

/// Longest scenario, in simulation ticks (a bound for tools; 77 hours at 60 Hz).
pub const MAX_SCENARIO_TICKS: u64 = 16_000_000;

/// What a check step asserts, after the simulation systems of its tick ran
/// (in a run with [`headless_frames`](crate::game::Game::headless_frames):
/// after the whole frame of that tick).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ScenarioCheck {
    /// A named action's edges and held state, as the systems of that tick
    /// saw them. Only the `Some` fields are compared.
    Action {
        /// The action's name; an unknown name fails the check.
        name: String,
        /// Became held this frame.
        pressed: Option<bool>,
        /// Held now.
        down: Option<bool>,
        /// Released this frame.
        released: Option<bool>,
    },
    /// One value of a [reflected resource](crate::reflect::WorldReflect::reflect_resource),
    /// compared as JSON (numbers compare by value).
    Resource {
        /// The resource's short type name.
        name: String,
        /// A JSON pointer into it (`"/hp"`); empty means the whole resource.
        pointer: String,
        /// The expected value.
        equals: Value,
    },
    /// How many entities have a [reflected component](crate::reflect::WorldReflect::reflect).
    Count {
        /// The component's short type name.
        component: String,
        /// The expected number of entities.
        equals: usize,
    },
    /// A node of the retained UI (`ctx.ui`), found by its
    /// [name](crate::ui::Ui::find). Only the `Some` fields are compared.
    Ui {
        /// The node's name.
        node: String,
        /// A node has that name.
        exists: Option<bool>,
        /// It is [shown](crate::ui::Ui::is_shown): on a screen of the stack, not hidden.
        shown: Option<bool>,
        /// It has the keyboard and gamepad focus.
        focused: Option<bool>,
        /// Its text (a label's, a button's, what a text input holds).
        text: Option<String>,
    },
}

impl ScenarioCheck {
    /// Checks an action's edges and held state; chain with `.pressed(true)`,
    /// `.down(true)` and `.released(false)`.
    /// ```
    /// use ponko::scenario::ScenarioCheck;
    /// let check = ponko::scenario::ScenarioCheck::action("jump").pressed(true).down(true);
    /// ```
    pub fn action(name: impl Into<String>) -> Self {
        Self::Action {
            name: name.into(),
            pressed: None,
            down: None,
            released: None,
        }
    }

    /// Expects an action check's `pressed` edge (no effect on other checks).
    #[must_use]
    pub fn pressed(mut self, expected: bool) -> Self {
        if let Self::Action { pressed, .. } = &mut self {
            *pressed = Some(expected);
        }
        self
    }

    /// Expects an action check's held state (no effect on other checks).
    #[must_use]
    pub fn down(mut self, expected: bool) -> Self {
        if let Self::Action { down, .. } = &mut self {
            *down = Some(expected);
        }
        self
    }

    /// Expects an action check's `released` edge (no effect on other checks).
    #[must_use]
    pub fn released(mut self, expected: bool) -> Self {
        if let Self::Action { released, .. } = &mut self {
            *released = Some(expected);
        }
        self
    }

    /// Checks a field of a reflected resource by JSON pointer.
    /// ```
    /// use ponko::scenario::ScenarioCheck;
    /// let check = ponko::scenario::ScenarioCheck::resource("Score", "/points", 3);
    /// ```
    pub fn resource(name: impl Into<String>, pointer: impl Into<String>, equals: impl Into<Value>) -> Self {
        Self::Resource {
            name: name.into(),
            pointer: pointer.into(),
            equals: equals.into(),
        }
    }

    /// Checks a node of the retained UI by name; chain with `.exists(true)`,
    /// `.shown(true)`, `.focused(true)` and `.text("Back")`. Every field but
    /// `exists` needs the node to exist.
    /// ```
    /// use ponko::scenario::ScenarioCheck;
    /// let check = ScenarioCheck::ui("menu.play").shown(true).focused(true).text("Play");
    /// ```
    pub fn ui(node: impl Into<String>) -> Self {
        Self::Ui {
            node: node.into(),
            exists: None,
            shown: None,
            focused: None,
            text: None,
        }
    }

    /// Expects a UI check's node to exist, or not (no effect on other checks).
    #[must_use]
    pub fn exists(mut self, expected: bool) -> Self {
        if let Self::Ui { exists, .. } = &mut self {
            *exists = Some(expected);
        }
        self
    }

    /// Expects a UI check's node to be shown, or not (no effect on other checks).
    #[must_use]
    pub fn shown(mut self, expected: bool) -> Self {
        if let Self::Ui { shown, .. } = &mut self {
            *shown = Some(expected);
        }
        self
    }

    /// Expects a UI check's node to have the focus, or not (no effect on other checks).
    #[must_use]
    pub fn focused(mut self, expected: bool) -> Self {
        if let Self::Ui { focused, .. } = &mut self {
            *focused = Some(expected);
        }
        self
    }

    /// Expects a UI check's node to have this text (no effect on other checks).
    #[must_use]
    pub fn text(mut self, expected: impl Into<String>) -> Self {
        if let Self::Ui { text, .. } = &mut self {
            *text = Some(expected.into());
        }
        self
    }

    /// Whether a UI check compares anything (an empty one is refused when read from JSON).
    fn ui_is_empty(&self) -> bool {
        matches!(
            self,
            Self::Ui {
                exists: None,
                shown: None,
                focused: None,
                text: None,
                ..
            }
        )
    }

    /// Checks how many entities have a reflected component.
    /// ```
    /// use ponko::scenario::ScenarioCheck;
    /// let check = ponko::scenario::ScenarioCheck::count("Coin", 2);
    /// ```
    pub fn count(component: impl Into<String>, equals: usize) -> Self {
        Self::Count {
            component: component.into(),
            equals,
        }
    }
}

/// A step that acts on the retained UI by node name.
#[derive(Debug, Clone, PartialEq)]
enum UiStep {
    /// Click the node.
    Click(String),
    /// Click the node (a text input) and type `text` at its end.
    Type(String, String),
}

impl UiStep {
    fn node(&self) -> &str {
        match self {
            Self::Click(node) | Self::Type(node, _) => node,
        }
    }
}

/// Raw input and checks scheduled by simulation tick, for reproducible
/// headless tests and tool-driven runs.
///
/// Install it with [`Game::input_scenario`](crate::game::Game::input_scenario)
/// (ticks count from the start of the run) or start it mid-run with
/// [`Input::start_scenario`](crate::input::Input::start_scenario) (ticks
/// count from the tick it starts on). Before the simulation systems of tick
/// `t` run, every input step scheduled at or before `t` goes through
/// [`Input::feed`](crate::input::Input::feed), the same path as live input,
/// so actions see ordinary pressed, held and released states. After they
/// ran, every check scheduled at or before `t` is evaluated. Steps keep the
/// order they were added in within a tick. The first tick of a run is 0.
/// Pausing does not move a step, because paused games don't advance the tick.
///
/// The run lasts [`ticks`](Self::ticks) ticks; a failed check ends it at
/// once. Either way the outcome is the [`ScenarioReport`] resource in the
/// world. A cancelled or failed run releases the inputs it was holding; a
/// finished one does so only after [`release_when_done`](Self::release_when_done).
///
/// Building a scenario allocates; applying input steps does not.
/// ```
/// use ponko::prelude::*;
/// use ponko::actions::InputEvent;
/// use ponko::scenario::InputScenario;
/// // Hold Space on ticks 2, 3 and 4; it is released on tick 5.
/// let jump = ponko::scenario::InputScenario::new()
///     .at(2, ponko::actions::InputEvent::Key(Key::Space, true))
///     .at(5, ponko::actions::InputEvent::Key(Key::Space, false));
/// ```
#[derive(Debug, Clone, Default)]
pub struct InputScenario {
    id: Option<String>,
    /// Sorted by tick; equal ticks keep insertion order. `(tick, step, event)`.
    inputs: Vec<(u64, usize, InputEvent)>,
    /// Sorted like `inputs`.
    checks: Vec<(u64, usize, ScenarioCheck)>,
    /// Steps on the retained UI, sorted like `inputs`.
    ui: Vec<(u64, usize, UiStep)>,
    /// How many steps were added; the next step's number.
    steps: usize,
    duration: Option<u64>,
    release_when_done: bool,
}

impl InputScenario {
    /// An empty scenario.
    /// ```
    /// let scenario = ponko::scenario::InputScenario::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `event` at simulation tick `tick`. Steps may be added in any order.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::actions::InputEvent;
    /// use ponko::scenario::InputScenario;
    /// let scenario = ponko::scenario::InputScenario::new().at(10, ponko::actions::InputEvent::Key(Key::Enter, true));
    /// ```
    pub fn at(mut self, tick: u64, event: InputEvent) -> Self {
        let index = self.inputs.partition_point(|&(t, ..)| t <= tick);
        self.inputs.insert(index, (tick, self.steps, event));
        self.steps += 1;
        self
    }

    /// Adds `check` at tick `tick`; it is evaluated after that tick's
    /// simulation systems ran. A failure is reported with its step number
    /// (the order steps were added in, from 0) and tick.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::actions::InputEvent;
    /// use ponko::scenario::InputScenario;
    /// use ponko::scenario::ScenarioCheck;
    /// let scenario = ponko::scenario::InputScenario::new()
    ///     .at(2, ponko::actions::InputEvent::Key(Key::Space, true))
    ///     .expect(2, ponko::scenario::ScenarioCheck::action("jump").pressed(true));
    /// ```
    pub fn expect(mut self, tick: u64, check: ScenarioCheck) -> Self {
        let index = self.checks.partition_point(|&(t, ..)| t <= tick);
        self.checks.insert(index, (tick, self.steps, check));
        self.steps += 1;
        self
    }

    /// Clicks the retained UI's node named `node` on tick `tick`: the
    /// pointer goes to the node's centre and presses and releases there in
    /// one frame, through the UI's own hit testing, so a button behind a
    /// modal or a disabled one is not clicked.
    ///
    /// The UI handles clicks in frames. In a run with
    /// [`headless_frames`](crate::game::Game::headless_frames) the click is
    /// part of the frame of tick `tick`: its [`UiEvent`](crate::ui::UiEvent)s
    /// are there for that frame's systems. With a window it lands in the
    /// frame after that tick's; a simulation-only headless run has no frames
    /// and never clicks. One UI step is used per frame: a second one on the
    /// same tick waits for the next frame.
    ///
    /// The run fails at this tick when no node has the name, it is not
    /// shown, or something else is under its centre.
    /// ```
    /// use ponko::scenario::{InputScenario, ScenarioCheck};
    /// let scenario = InputScenario::new()
    ///     .click(5, "menu.settings")
    ///     .expect(5, ScenarioCheck::ui("settings").shown(true));
    /// ```
    pub fn click(mut self, tick: u64, node: impl Into<String>) -> Self {
        self.push_ui(tick, UiStep::Click(node.into()));
        self
    }

    /// Types `text` into the retained UI's text input named `node` on tick
    /// `tick`: clicks it (see [`click`](Self::click)), which focuses it, and
    /// types `text` at the end of what it holds, in the same frame. The
    /// field sends [`UiEvent::Changed`](crate::ui::UiEvent::Changed). For
    /// Enter, Backspace or Tab afterwards, add key steps.
    ///
    /// The run fails at this tick for the reasons `click` does, and when the
    /// node is not a text input.
    /// ```
    /// use ponko::scenario::{InputScenario, ScenarioCheck};
    /// let scenario = InputScenario::new()
    ///     .type_text(3, "login.name", "Ada")
    ///     .expect(3, ScenarioCheck::ui("login.name").focused(true).text("Ada"));
    /// ```
    pub fn type_text(mut self, tick: u64, node: impl Into<String>, text: impl Into<String>) -> Self {
        self.push_ui(tick, UiStep::Type(node.into(), text.into()));
        self
    }

    fn push_ui(&mut self, tick: u64, step: UiStep) {
        let index = self.ui.partition_point(|&(t, ..)| t <= tick);
        self.ui.insert(index, (tick, self.steps, step));
        self.steps += 1;
    }

    /// Names the scenario in its report.
    #[must_use]
    pub fn named(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Lasts `ticks` simulation ticks (default: through the last step's tick).
    /// Steps at or after `ticks` never run.
    #[must_use]
    pub fn duration(mut self, ticks: u64) -> Self {
        self.duration = Some(ticks);
        self
    }

    /// Releases the inputs still held when the run finishes normally
    /// (cancelled and failed runs always release them). A scenario a tool
    /// loads through the control endpoint does this by default; see
    /// `keep_input_when_done` there.
    #[must_use]
    pub fn release_when_done(mut self) -> Self {
        self.release_when_done = true;
        self
    }

    /// For tool-loaded scenarios: release at the end unless `keep` says not to.
    pub(crate) fn release_unless_kept(&mut self, keep: bool) {
        if !keep {
            self.release_when_done = true;
        }
    }

    /// The scenario's name, if it has one.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// How many steps (inputs and checks) it holds.
    pub fn step_count(&self) -> usize {
        self.steps
    }

    /// How many simulation ticks the run lasts: the set duration, or one
    /// past the last step's tick (0 for an empty scenario). Use it as the
    /// tick count of a headless run.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::actions::InputEvent;
    /// use ponko::scenario::InputScenario;
    /// let s = ponko::scenario::InputScenario::new().at(5, ponko::actions::InputEvent::Key(Key::A, true));
    /// assert_eq!(s.ticks(), 6);
    /// assert_eq!(s.duration(10).ticks(), 10);
    /// ```
    pub fn ticks(&self) -> u64 {
        self.duration.unwrap_or_else(|| {
            let last_input = self.inputs.last().map(|s| s.0);
            let last_check = self.checks.last().map(|s| s.0);
            let last_ui = self.ui.last().map(|s| s.0);
            last_input
                .max(last_check)
                .max(last_ui)
                .map_or(0, |t| t.saturating_add(1))
        })
    }

    /// Reads a scenario from its JSON format, version 1:
    ///
    /// ```json
    /// {
    ///   "version": 1,
    ///   "id": "jump-twice",
    ///   "duration": 120,
    ///   "release_when_done": true,
    ///   "steps": [
    ///     { "tick": 10, "type": "key", "key": "Space", "down": true },
    ///     { "tick": 10, "type": "expect_action", "action": "jump", "pressed": true },
    ///     { "tick": 11, "type": "expect_resource", "resource": "Score", "pointer": "/jumps", "equals": 1 },
    ///     { "tick": 12, "type": "expect_count", "component": "Coin", "equals": 2 }
    ///   ]
    /// }
    /// ```
    ///
    /// ponko::input::Input steps are `key` (`key`, `down`), `mouse` (`button`, `down`),
    /// `gamepad_button` (`pad`, `button`, `down`), `gamepad_axis` (`pad`,
    /// `axis`, `value`), `focus_lost` and `gamepad_disconnected` (`pad`).
    /// UI steps are `ui_click` (`node`) and `ui_type` (`node`, `text`), see
    /// [`click`](Self::click) and [`type_text`](Self::type_text).
    /// Check steps are `expect_action` (`action` and any of `pressed`, `down`,
    /// `released`), `expect_resource` (`resource`, `pointer`, `equals`),
    /// `expect_count` (`component`, `equals`) and `expect_ui` (`node` and any
    /// of `exists`, `shown`, `focused`, `text`). Every step has a `tick`.
    /// Steps may come in any order; their step number is their position in
    /// the file, from 0. Unknown fields, a newer `version`, more than
    /// [`crate::scenario::MAX_SCENARIO_STEPS`] steps, a duration over [`crate::scenario::MAX_SCENARIO_TICKS`]
    /// and steps at or after `duration` are errors, so a typo can't silently
    /// weaken a test.
    /// ```
    /// use ponko::scenario::InputScenario;
    /// let scenario = InputScenario::from_json(r#"{
    ///     "version": 1,
    ///     "steps": [{ "tick": 3, "type": "key", "key": "Space", "down": true }]
    /// }"#).unwrap();
    /// assert_eq!(scenario.ticks(), 4);
    /// assert!(InputScenario::from_json(r#"{ "version": 2, "steps": [] }"#).is_err());
    /// ```
    pub fn from_json(text: &str) -> Result<Self, ScenarioError> {
        let file: FileWire = serde_json::from_str(text).map_err(|e| ScenarioError(e.to_string()))?;
        Self::from_file(file)
    }

    /// Like [`from_json`](Self::from_json), from a parsed value.
    pub fn from_value(value: &Value) -> Result<Self, ScenarioError> {
        let file: FileWire = serde_json::from_value(value.clone()).map_err(|e| ScenarioError(e.to_string()))?;
        Self::from_file(file)
    }

    fn from_file(file: FileWire) -> Result<Self, ScenarioError> {
        if file.version != SCENARIO_VERSION {
            return Err(ScenarioError(format!(
                "unsupported scenario version {} (this engine reads version {SCENARIO_VERSION})",
                file.version
            )));
        }
        if file.steps.len() > MAX_SCENARIO_STEPS {
            return Err(ScenarioError(format!(
                "{} steps is too many (at most {MAX_SCENARIO_STEPS})",
                file.steps.len()
            )));
        }
        if file.duration.is_some_and(|d| d > MAX_SCENARIO_TICKS) {
            return Err(ScenarioError(format!(
                "`duration` is too long (at most {MAX_SCENARIO_TICKS} ticks)"
            )));
        }
        let mut scenario = Self::new();
        scenario.id = file.id;
        scenario.release_when_done = file.release_when_done;
        scenario.duration = file.duration;
        for (n, step) in file.steps.into_iter().enumerate() {
            let tick = step.tick();
            if tick > MAX_SCENARIO_TICKS || file.duration.is_some_and(|d| tick >= d) {
                return Err(ScenarioError(format!(
                    "step {n} is at tick {tick}, outside the scenario's {} ticks",
                    file.duration.unwrap_or(MAX_SCENARIO_TICKS)
                )));
            }
            match step.into_step() {
                Step::Input(event) => scenario = scenario.at(tick, event),
                Step::Ui(step) => scenario.push_ui(tick, step),
                Step::Check(check) => {
                    if check.ui_is_empty() {
                        return Err(ScenarioError(format!(
                            "step {n}: `expect_ui` needs at least one of `exists`, `shown`, `focused`, `text`"
                        )));
                    }
                    if let ScenarioCheck::Action {
                        pressed: None,
                        down: None,
                        released: None,
                        ..
                    } = check
                    {
                        return Err(ScenarioError(format!(
                            "step {n}: `expect_action` needs at least one of `pressed`, `down`, `released`"
                        )));
                    }
                    scenario = scenario.expect(tick, check);
                }
            }
        }
        Ok(scenario)
    }

    /// The scenario in its JSON format, steps in step-number order, so
    /// `from_value(&s.to_json())` gives the same scenario back.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::actions::InputEvent;
    /// use ponko::scenario::InputScenario;
    /// let s = ponko::scenario::InputScenario::new().at(2, ponko::actions::InputEvent::Key(Key::Space, true)).named("tap");
    /// let again = ponko::scenario::InputScenario::from_value(&s.to_json()).unwrap();
    /// assert_eq!(again.to_json(), s.to_json());
    /// ```
    pub fn to_json(&self) -> Value {
        let mut steps: Vec<(usize, StepWire)> = Vec::with_capacity(self.steps);
        steps.extend(
            self.inputs
                .iter()
                .map(|&(tick, n, event)| (n, StepWire::from_input(tick, event))),
        );
        steps.extend(
            self.checks
                .iter()
                .map(|(tick, n, check)| (*n, StepWire::from_check(*tick, check))),
        );
        steps.extend(
            self.ui
                .iter()
                .map(|(tick, n, step)| (*n, StepWire::from_ui(*tick, step))),
        );
        steps.sort_by_key(|s| s.0);
        let file = FileWire {
            version: SCENARIO_VERSION,
            id: self.id.clone(),
            duration: self.duration,
            release_when_done: self.release_when_done,
            steps: steps.into_iter().map(|s| s.1).collect(),
        };
        serde_json::to_value(file).unwrap_or(Value::Null)
    }
}

/// Why a scenario file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioError(String);

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad input scenario: {}", self.0)
    }
}

impl std::error::Error for ScenarioError {}

// ------------------------------------------------------------------ report

/// How a scenario run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    /// Installed; its first tick has not run.
    Pending,
    /// Running.
    Running,
    /// Ran its whole duration and every check held.
    Passed,
    /// A check failed; the run ended there.
    Failed,
    /// Ended early by [`crate::input::Input::cancel_scenario`], a tool, a client that
    /// disconnected, or a scenario that replaced it.
    Cancelled,
}

/// The step and tick of a check that did not hold, in machine-readable form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioFailure {
    /// The step's number: its position among the steps as added or as in the file.
    pub step: usize,
    /// The scenario tick the check was scheduled at.
    pub tick: u64,
    /// The simulation tick it ran on (the same unless the scenario started mid-run).
    pub sim_tick: u64,
    /// What was checked, e.g. `action jump`.
    pub check: String,
    /// What the check expected.
    pub expected: Value,
    /// What the game had.
    pub actual: Value,
    /// A sentence for people.
    pub message: String,
}

/// The outcome of the latest scenario run, kept as a resource in the world
/// (`world.get_resource::<ponko::scenario::ScenarioReport>()`), and what the control endpoint's
/// `input.scenario.status` returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioReport {
    /// The report's format version (the scenario format's).
    pub version: u32,
    /// The scenario's name, if it has one.
    pub id: Option<String>,
    /// Where the run stands.
    pub status: ScenarioStatus,
    /// How many ticks the run lasts.
    pub duration: u64,
    /// How many ticks have run.
    pub ticks_run: u64,
    /// The simulation tick the scenario's tick 0 is.
    pub start_tick: u64,
    /// Steps in the scenario.
    pub steps: usize,
    /// Input steps fed so far, UI steps (clicks, typing) included.
    pub inputs_applied: usize,
    /// Checks that held so far.
    pub checks_passed: usize,
    /// The first failed check, if any.
    pub failure: Option<ScenarioFailure>,
    /// How many held inputs the run released when it ended early or failed.
    pub released: usize,
    /// How many inputs a run that finished normally left held (a tool that
    /// loaded it with `keep_input_when_done`); a cancel or the owner's
    /// disconnect still releases them.
    #[serde(default)]
    pub still_held: usize,
    /// Why it was cancelled.
    pub cancel_reason: Option<String>,
}

impl ScenarioReport {
    pub(crate) fn new(scenario: &InputScenario, start_tick: u64) -> Self {
        Self {
            version: SCENARIO_VERSION,
            id: scenario.id.clone(),
            status: ScenarioStatus::Pending,
            duration: scenario.ticks(),
            ticks_run: 0,
            start_tick,
            steps: scenario.steps,
            inputs_applied: 0,
            checks_passed: 0,
            failure: None,
            released: 0,
            still_held: 0,
            cancel_reason: None,
        }
    }

    /// Whether the run ended with every check held.
    pub fn passed(&self) -> bool {
        self.status == ScenarioStatus::Passed
    }

    /// Panics with the failing step and tick unless the run passed. For tests.
    /// ```
    /// use ponko::prelude::*;
    /// let scenario = ponko::scenario::InputScenario::new().at(1, ponko::actions::InputEvent::Key(Key::Space, true));
    /// let (_world, report) = Game::new("t", 1, 1).run_headless_scenario(scenario, |_| {});
    /// report.assert_passed();
    /// ```
    #[track_caller]
    pub fn assert_passed(&self) {
        if let Some(f) = &self.failure {
            panic!(
                "input scenario failed at step {} (tick {}): {}",
                f.step, f.tick, f.message
            );
        }
        assert!(self.passed(), "input scenario did not pass: {:?}", self.status);
    }
}

// --------------------------------------------------------------------- run

/// An installed scenario and its cursors.
#[derive(Debug, Clone)]
pub(crate) struct ScenarioRun {
    scenario: InputScenario,
    /// The simulation tick that is the scenario's tick 0.
    start: u64,
    next_input: usize,
    next_check: usize,
    next_ui: usize,
    /// A UI step that could not be done; reported when its tick ends.
    ui_failure: Option<(u64, usize, String, String)>,
    /// The control connection that owns it, if it's bound to one.
    pub(crate) owner: Option<u64>,
    /// Loaded by a tool: input left held at the end stays owned (see [`Leftover`]).
    tool: bool,
    /// Simulation ticks the run queued as steps for itself (a paused game).
    own_steps: u64,
    /// Ticks of the run that have finished.
    ticks_done: u64,
}

impl ScenarioRun {
    /// Queued steps of its own that have not been used yet.
    fn remaining_own(&self) -> u64 {
        self.own_steps.saturating_sub(self.ticks_done)
    }
}

/// How a scenario was started.
#[derive(Default, Clone, Copy)]
pub(crate) struct RunOrigin {
    /// The control connection it is bound to, if any.
    pub(crate) owner: Option<u64>,
    /// Loaded by a tool through the control endpoint.
    pub(crate) tool: bool,
    /// Queue the run's own ticks as steps (needs a paused game).
    pub(crate) queue_ticks: bool,
}

/// ponko::input::Input a finished tool scenario left held on purpose. It stays owned, so a
/// cancel or the owner's disconnect still releases it.
#[derive(Debug, Clone)]
pub(crate) struct Leftover {
    held: Vec<InputEvent>,
    owner: Option<u64>,
}

/// What [`crate::input::Input`] keeps for scenarios.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scenarios {
    pub(crate) run: Option<ScenarioRun>,
    pub(crate) leftover: Option<Leftover>,
    /// Whether tools may load scenarios through the control endpoint.
    pub(crate) injection: bool,
}

impl crate::input::Input {
    /// Deprecated: use [`Ctx::start_scenario`](crate::ctx::Ctx::start_scenario),
    /// which takes the world and the clock itself.
    #[deprecated(since = "0.2.0", note = "use `ctx.start_scenario(scenario)`")]
    pub fn start_scenario(&mut self, world: &mut World, time: &mut Time, scenario: InputScenario) {
        // A system runs inside a tick, so its scenario's tick 0 is the next one.
        let start = if time.in_sim() {
            time.sim_tick() + 1
        } else {
            time.sim_tick()
        };
        self.install_scenario(world, time, scenario, start, RunOrigin::default());
    }

    /// Deprecated: use [`Ctx::cancel_scenario`](crate::ctx::Ctx::cancel_scenario).
    #[deprecated(since = "0.2.0", note = "use `ctx.cancel_scenario()`")]
    pub fn cancel_scenario(&mut self, world: &mut World, time: &mut Time) -> bool {
        self.cancel_scenario_for(world, time, "cancelled", None)
    }

    /// The latest scenario run's report, which is kept in `world`.
    #[must_use]
    pub fn scenario_report<'w>(&self, world: &'w World) -> Option<&'w ScenarioReport> {
        world.get_resource::<ScenarioReport>()
    }

    /// Whether tools may start scenarios through the control endpoint.
    #[must_use]
    pub fn injection_allowed(&self) -> bool {
        self.scenarios.injection
    }

    pub(crate) fn install_scenario(
        &mut self,
        world: &mut World,
        time: &mut Time,
        scenario: InputScenario,
        start: u64,
        origin: RunOrigin,
    ) {
        self.cancel_scenario_for(world, time, "replaced by another scenario", None);
        self.release_leftover(world, "replaced by another scenario", None);
        let ticks = scenario.ticks();
        world.insert_resource(ScenarioReport::new(&scenario, start));
        let own_steps = if origin.queue_ticks { ticks } else { 0 };
        time.queue_steps(own_steps);
        self.scenarios.run = Some(ScenarioRun {
            scenario,
            start,
            next_input: 0,
            next_check: 0,
            next_ui: 0,
            ui_failure: None,
            owner: origin.owner,
            tool: origin.tool,
            own_steps,
            ticks_done: 0,
        });
    }

    /// Cancels the run if `only_owner` is `None` or owns it. Without a run,
    /// releases what a finished tool scenario left held. Returns whether
    /// anything was cancelled or released.
    pub(crate) fn cancel_scenario_for(
        &mut self,
        world: &mut World,
        time: &mut Time,
        reason: &str,
        only_owner: Option<u64>,
    ) -> bool {
        match (&self.scenarios.run, only_owner) {
            (None, _) => return self.release_leftover(world, reason, only_owner),
            (Some(run), Some(owner)) if run.owner != Some(owner) => return false,
            _ => {}
        }
        let Some(run) = self.scenarios.run.take() else {
            return false;
        };
        let released = self.release_held(&run);
        time.drop_pending_steps(run.remaining_own());
        if let Some(report) = world.get_resource_mut::<ScenarioReport>() {
            report.status = ScenarioStatus::Cancelled;
            report.released = released;
            report.cancel_reason = Some(reason.to_string());
        }
        true
    }

    /// Releases the input a finished tool scenario left held, if `only_owner`
    /// is `None` or owns it.
    fn release_leftover(&mut self, world: &mut World, reason: &str, only_owner: Option<u64>) -> bool {
        match (&self.scenarios.leftover, only_owner) {
            (None, _) => return false,
            (Some(left), Some(owner)) if left.owner != Some(owner) => return false,
            _ => {}
        }
        let Some(left) = self.scenarios.leftover.take() else {
            return false;
        };
        for &event in &left.held {
            self.feed(event);
        }
        if let Some(report) = world.get_resource_mut::<ScenarioReport>() {
            report.released = left.held.len();
            report.still_held = 0;
            report.cancel_reason = Some(reason.to_string());
        }
        true
    }

    /// Releases what the inputs fed so far still hold; returns how many.
    fn release_held(&mut self, run: &ScenarioRun) -> usize {
        let held = held_inputs(&run.scenario.inputs[..run.next_input]);
        for &event in &held {
            self.feed(event);
        }
        held.len()
    }
}

impl Ctx {
    /// Starts `scenario` on the next simulation tick, which is its tick 0,
    /// replacing (and cancelling) one that is running. Use it from systems and
    /// tests; [`Game::input_scenario`](crate::game::Game::input_scenario) starts one
    /// with the game. The run's [`ScenarioReport`] is kept in `ctx.world`
    /// and its ticks are counted on `ctx.time`.
    /// ```
    /// use ponko::prelude::*;
    /// struct PressedOn(Option<u64>);
    ///
    /// let game = Game::new("t", 1, 1)
    ///     .sim_system(|ctx: &mut Ctx| {
    ///         if ctx.time.sim_tick() == 3 {
    ///             // Started during tick 3, so the scenario's tick 0 is tick 4.
    ///             let scenario = ponko::scenario::InputScenario::new().at(0, ponko::actions::InputEvent::Key(Key::Space, true));
    ///             ctx.start_scenario(scenario);
    ///         }
    ///     })
    ///     .sim_system(|ctx: &mut Ctx| {
    ///         if ctx.input.key_pressed(Key::Space) {
    ///             let tick = ctx.time.sim_tick();
    ///             ctx.world.resource_mut::<PressedOn>().0 = Some(tick);
    ///         }
    ///     });
    /// let world = game.run_headless(6, |ctx| ctx.world.insert_resource(PressedOn(None)));
    /// assert_eq!(world.resource::<ponko::scenario::ScenarioReport>().start_tick, 4);
    /// assert_eq!(world.resource::<PressedOn>().0, Some(4), "fed before tick 4's systems");
    /// ```
    #[allow(deprecated)] // the one implementation, until the old call goes
    pub fn start_scenario(&mut self, scenario: InputScenario) {
        self.input.start_scenario(&mut self.world, &mut self.time, scenario);
    }

    /// Ends the running scenario, releasing the keys, buttons and axes it
    /// still holds. Returns whether one was running. The report (in
    /// `ctx.world`) says `cancelled`.
    ///
    /// The input state is one shared set of held keys, with no record of
    /// which device holds one, so a key the scenario pressed is released even
    /// if a player is physically holding it too: it reads as up until its
    /// next key event. Only what the scenario itself left held is released.
    /// ```
    /// use ponko::prelude::*;
    /// let mut game = Game::new("t", 1, 1).sim_system(|ctx: &mut Ctx| {
    ///     if ctx.time.sim_tick() == 2 {
    ///         assert!(ctx.cancel_scenario());
    ///         assert!(!ctx.input.key_down(Key::Space)); // released
    ///     }
    /// });
    /// let scenario = ponko::scenario::InputScenario::new().at(0, ponko::actions::InputEvent::Key(Key::Space, true)).duration(10);
    /// let (world, report) = game.run_headless_scenario(scenario, |_| {});
    /// assert_eq!(report.status, ponko::scenario::ScenarioStatus::Cancelled);
    /// ```
    #[allow(deprecated)] // the one implementation, until the old call goes
    pub fn cancel_scenario(&mut self) -> bool {
        self.input.cancel_scenario(&mut self.world, &mut self.time)
    }
}

impl Ctx {
    /// Before a tick's simulation systems: feeds the input steps due.
    pub(crate) fn scenario_begin_tick(&mut self) {
        let Some(mut run) = self.input.scenarios.run.take() else {
            return;
        };
        // Started by a system during an earlier tick: tick 0 is still to come.
        if self.time.sim_tick() < run.start {
            self.input.scenarios.run = Some(run);
            return;
        }
        let rel = self.time.sim_tick() - run.start;
        let mut fed = 0;
        while let Some(&(tick, _, event)) = run.scenario.inputs.get(run.next_input) {
            if tick > rel {
                break;
            }
            self.input.feed(event);
            run.next_input += 1;
            fed += 1;
        }
        // One UI step a frame: the UI takes one click per frame.
        if let Some((tick, step, ui)) = run.scenario.ui.get(run.next_ui)
            && *tick <= rel
            && run.ui_failure.is_none()
        {
            let text = match ui {
                UiStep::Click(_) => None,
                UiStep::Type(_, text) => Some(text.as_str()),
            };
            match self.script_ui_click(ui.node(), text) {
                Ok(()) => fed += 1,
                Err(message) => {
                    let what = match ui {
                        UiStep::Click(node) => format!("click {node}"),
                        UiStep::Type(node, _) => format!("type into {node}"),
                    };
                    run.ui_failure = Some((*tick, *step, what, message));
                }
            }
            run.next_ui += 1;
        }
        if let Some(report) = self.world.get_resource_mut::<ScenarioReport>() {
            report.status = ScenarioStatus::Running;
            report.inputs_applied += fed;
        }
        self.input.scenarios.run = Some(run);
    }

    /// After the simulation systems of tick `sim_tick` (with headless
    /// frames, after its whole frame): runs the checks due, and ends the run
    /// on a failure or when its duration is over.
    pub(crate) fn scenario_end_tick(&mut self, sim_tick: u64) {
        let Some(mut run) = self.input.scenarios.run.take() else {
            return;
        };
        if sim_tick < run.start {
            self.input.scenarios.run = Some(run);
            return;
        }
        let rel = sim_tick - run.start;
        run.ticks_done += 1;
        let mut passed = 0;
        let mut failure = run
            .ui_failure
            .take()
            .map(|(tick, step, check, message)| ScenarioFailure {
                step,
                tick,
                sim_tick,
                check,
                expected: Value::Null,
                actual: Value::Null,
                message,
            });
        while let Some((tick, step, check)) = run.scenario.checks.get(run.next_check)
            && failure.is_none()
        {
            if *tick > rel {
                break;
            }
            let outcome = evaluate(self, check);
            run.next_check += 1;
            match outcome {
                Ok(()) => passed += 1,
                Err(mut f) => {
                    f.step = *step;
                    f.tick = *tick;
                    f.sim_tick = sim_tick;
                    failure = Some(*f);
                    break;
                }
            }
        }
        let done = rel + 1 >= run.scenario.ticks();
        let release = failure.is_some() || (done && run.scenario.release_when_done);
        let released = if release { self.input.release_held(&run) } else { 0 };
        // A tool's scenario that finished and kept its input: stays owned.
        let left = if done && !release && run.tool {
            held_inputs(&run.scenario.inputs[..run.next_input])
        } else {
            Vec::new()
        };
        let still_held = left.len();
        if still_held > 0 {
            self.input.scenarios.leftover = Some(Leftover {
                held: left,
                owner: run.owner,
            });
        }
        if let Some(report) = self.world.get_resource_mut::<ScenarioReport>() {
            report.ticks_run = rel + 1;
            report.checks_passed += passed;
            report.released = released;
            report.still_held = still_held;
            if failure.is_some() {
                report.status = ScenarioStatus::Failed;
                report.failure = failure.take();
                // Only the steps this run queued for itself are dropped, not
                // ticks another client asked for.
                self.time.drop_pending_steps(run.remaining_own());
            } else if done {
                report.status = ScenarioStatus::Passed;
            }
        }
        // `failure` was moved into the report when there is one.
        let ended = done || release;
        if !ended {
            self.input.scenarios.run = Some(run);
        }
    }
}

/// The releases that undo `applied`: what is still held after those inputs.
fn held_inputs(applied: &[(u64, usize, InputEvent)]) -> Vec<InputEvent> {
    // One entry per control, holding the event that releases it.
    let mut held: Vec<InputEvent> = Vec::new();
    let same_control = |a: &InputEvent, b: &InputEvent| match (a, b) {
        (InputEvent::Key(x, _), InputEvent::Key(y, _)) => x == y,
        (InputEvent::Mouse(x, _), InputEvent::Mouse(y, _)) => x == y,
        (InputEvent::GamepadButton(p, x, _), InputEvent::GamepadButton(q, y, _)) => p == q && x == y,
        (InputEvent::GamepadAxis(p, x, _), InputEvent::GamepadAxis(q, y, _)) => p == q && x == y,
        _ => false,
    };
    for &(_, _, event) in applied {
        match event {
            InputEvent::FocusLost => {
                held.retain(|h| !matches!(h, InputEvent::Key(..) | InputEvent::Mouse(..)));
            }
            InputEvent::GamepadDisconnected(pad) => held.retain(|h| match h {
                InputEvent::GamepadButton(p, ..) | InputEvent::GamepadAxis(p, ..) => *p != pad,
                _ => true,
            }),
            _ => {
                held.retain(|h| !same_control(h, &event));
                match event {
                    InputEvent::Key(k, true) => held.push(InputEvent::Key(k, false)),
                    InputEvent::Mouse(b, true) => held.push(InputEvent::Mouse(b, false)),
                    InputEvent::GamepadButton(p, b, true) => held.push(InputEvent::GamepadButton(p, b, false)),
                    InputEvent::GamepadAxis(p, a, v) if v != 0.0 => held.push(InputEvent::GamepadAxis(p, a, 0.0)),
                    _ => {}
                }
            }
        }
    }
    held
}

fn failure(check: impl Into<String>, expected: Value, actual: Value, message: String) -> Box<ScenarioFailure> {
    Box::new(ScenarioFailure {
        step: 0,
        tick: 0,
        sim_tick: 0,
        check: check.into(),
        expected,
        actual,
        message,
    })
}

/// Compares JSON, numbers by value (`3` equals `3.0`).
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| json_eq(x, y)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

/// `Ok` if `check` holds now. The failure's step and ticks are filled in by the caller.
fn evaluate(ctx: &Ctx, check: &ScenarioCheck) -> Result<(), Box<ScenarioFailure>> {
    match check {
        ScenarioCheck::Action {
            name,
            pressed,
            down,
            released,
        } => {
            let label = format!("action {name}");
            let id = ctx.input.actions.resolve(name).map_err(|e| {
                failure(
                    &label,
                    Value::Null,
                    Value::Null,
                    format!("no action `{name}` is bound ({e})"),
                )
            })?;
            let actual = [
                ("pressed", ctx.input.action_pressed(id)),
                ("down", ctx.input.action_down(id)),
                ("released", ctx.input.action_released(id)),
            ];
            let wanted = [("pressed", pressed), ("down", down), ("released", released)];
            let mut expected = serde_json::Map::new();
            let mut seen = serde_json::Map::new();
            let mut wrong = Vec::new();
            for ((key, want), (_, have)) in wanted.into_iter().zip(actual) {
                if let Some(want) = want {
                    expected.insert(key.into(), json!(want));
                    seen.insert(key.into(), json!(have));
                    if *want != have {
                        wrong.push(format!("{key}: expected {want}, saw {have}"));
                    }
                }
            }
            if wrong.is_empty() {
                return Ok(());
            }
            Err(failure(
                &label,
                Value::Object(expected),
                Value::Object(seen),
                format!("action `{name}` {}", wrong.join("; ")),
            ))
        }
        ScenarioCheck::Resource { name, pointer, equals } => {
            let label = format!("resource {name}{pointer}");
            let whole = ctx
                .world
                .resource_json(name)
                .map_err(|e| failure(&label, equals.clone(), Value::Null, e.to_string()))?;
            let Some(actual) = whole.pointer(pointer) else {
                return Err(failure(
                    &label,
                    equals.clone(),
                    Value::Null,
                    format!("resource `{name}` has nothing at `{pointer}`"),
                ));
            };
            if json_eq(actual, equals) {
                return Ok(());
            }
            Err(failure(
                &label,
                equals.clone(),
                actual.clone(),
                format!("{label}: expected {equals}, saw {actual}"),
            ))
        }
        ScenarioCheck::Count { component, equals } => {
            let label = format!("count {component}");
            if !ctx.world.reflected_components().contains(&component.as_str()) {
                return Err(failure(
                    &label,
                    json!(equals),
                    Value::Null,
                    format!("`{component}` is not a reflected component"),
                ));
            }
            let world = &ctx.world;
            let actual: usize = world
                .raw_ref()
                .archetypes()
                .iter()
                .filter(|a| {
                    a.component_ids()
                        .iter()
                        .any(|&id| crate::diagnostics::short_name(world.component_info(id).name()) == component)
                })
                .map(|a| a.entities().len())
                .sum();
            if actual == *equals {
                return Ok(());
            }
            Err(failure(
                &label,
                json!(equals),
                json!(actual),
                format!("expected {equals} entities with `{component}`, found {actual}"),
            ))
        }
        ScenarioCheck::Ui {
            node,
            exists,
            shown,
            focused,
            text,
        } => evaluate_ui(&ctx.ui, node, *exists, *shown, *focused, text.as_deref()),
    }
}

/// A [`ScenarioCheck::Ui`] against `ui`.
fn evaluate_ui(
    ui: &crate::ui::Ui,
    node: &str,
    exists: Option<bool>,
    shown: Option<bool>,
    focused: Option<bool>,
    text: Option<&str>,
) -> Result<(), Box<ScenarioFailure>> {
    let label = format!("ui {node}");
    let id = ui.find(node);
    let mut expected = serde_json::Map::new();
    let mut seen = serde_json::Map::new();
    let mut wrong = Vec::new();
    let mut compare = |key: &str, want: Value, have: Value| {
        if want != have {
            wrong.push(format!("{key}: expected {want}, saw {have}"));
        }
        expected.insert(key.into(), want);
        seen.insert(key.into(), have);
    };
    if let Some(want) = exists {
        compare("exists", json!(want), json!(id.is_some()));
    }
    let others = shown.is_some() || focused.is_some() || text.is_some();
    match id {
        None if others => {
            return Err(failure(
                &label,
                Value::Null,
                Value::Null,
                format!("no UI node is named `{node}`"),
            ));
        }
        None => {}
        Some(id) => {
            if let Some(want) = shown {
                compare("shown", json!(want), json!(ui.is_shown(id)));
            }
            if let Some(want) = focused {
                compare("focused", json!(want), json!(ui.focus() == Some(id)));
            }
            if let Some(want) = text {
                compare("text", json!(want), json!(ui.text(id).unwrap_or_default()));
            }
        }
    }
    if wrong.is_empty() {
        return Ok(());
    }
    Err(failure(
        &label,
        Value::Object(expected),
        Value::Object(seen),
        format!("UI node `{node}` {}", wrong.join("; ")),
    ))
}

// -------------------------------------------------------------------- wire

enum Step {
    Input(InputEvent),
    Ui(UiStep),
    Check(ScenarioCheck),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileWire {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration: Option<u64>,
    #[serde(default)]
    release_when_done: bool,
    steps: Vec<StepWire>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum StepWire {
    Key {
        tick: u64,
        key: Key,
        down: bool,
    },
    Mouse {
        tick: u64,
        button: MouseButton,
        down: bool,
    },
    GamepadButton {
        tick: u64,
        pad: Gamepad,
        button: GamepadButton,
        down: bool,
    },
    GamepadAxis {
        tick: u64,
        pad: Gamepad,
        axis: GamepadAxis,
        value: f32,
    },
    FocusLost {
        tick: u64,
    },
    GamepadDisconnected {
        tick: u64,
        pad: Gamepad,
    },
    ExpectAction {
        tick: u64,
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pressed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        down: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        released: Option<bool>,
    },
    ExpectResource {
        tick: u64,
        resource: String,
        #[serde(default)]
        pointer: String,
        equals: Value,
    },
    ExpectCount {
        tick: u64,
        component: String,
        equals: usize,
    },
    UiClick {
        tick: u64,
        node: String,
    },
    UiType {
        tick: u64,
        node: String,
        text: String,
    },
    ExpectUi {
        tick: u64,
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exists: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shown: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focused: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

impl StepWire {
    fn tick(&self) -> u64 {
        match self {
            Self::Key { tick, .. }
            | Self::Mouse { tick, .. }
            | Self::GamepadButton { tick, .. }
            | Self::GamepadAxis { tick, .. }
            | Self::FocusLost { tick }
            | Self::GamepadDisconnected { tick, .. }
            | Self::ExpectAction { tick, .. }
            | Self::ExpectResource { tick, .. }
            | Self::ExpectCount { tick, .. }
            | Self::UiClick { tick, .. }
            | Self::UiType { tick, .. }
            | Self::ExpectUi { tick, .. } => *tick,
        }
    }

    fn into_step(self) -> Step {
        match self {
            Self::Key { key, down, .. } => Step::Input(InputEvent::Key(key, down)),
            Self::Mouse { button, down, .. } => Step::Input(InputEvent::Mouse(button, down)),
            Self::GamepadButton { pad, button, down, .. } => Step::Input(InputEvent::GamepadButton(pad, button, down)),
            Self::GamepadAxis { pad, axis, value, .. } => Step::Input(InputEvent::GamepadAxis(pad, axis, value)),
            Self::FocusLost { .. } => Step::Input(InputEvent::FocusLost),
            Self::GamepadDisconnected { pad, .. } => Step::Input(InputEvent::GamepadDisconnected(pad)),
            Self::ExpectAction {
                action,
                pressed,
                down,
                released,
                ..
            } => Step::Check(ScenarioCheck::Action {
                name: action,
                pressed,
                down,
                released,
            }),
            Self::ExpectResource {
                resource,
                pointer,
                equals,
                ..
            } => Step::Check(ScenarioCheck::Resource {
                name: resource,
                pointer,
                equals,
            }),
            Self::ExpectCount { component, equals, .. } => Step::Check(ScenarioCheck::Count { component, equals }),
            Self::UiClick { node, .. } => Step::Ui(UiStep::Click(node)),
            Self::UiType { node, text, .. } => Step::Ui(UiStep::Type(node, text)),
            Self::ExpectUi {
                node,
                exists,
                shown,
                focused,
                text,
                ..
            } => Step::Check(ScenarioCheck::Ui {
                node,
                exists,
                shown,
                focused,
                text,
            }),
        }
    }

    fn from_input(tick: u64, event: InputEvent) -> Self {
        match event {
            InputEvent::Key(key, down) => Self::Key { tick, key, down },
            InputEvent::Mouse(button, down) => Self::Mouse { tick, button, down },
            InputEvent::GamepadButton(pad, button, down) => Self::GamepadButton {
                tick,
                pad,
                button,
                down,
            },
            InputEvent::GamepadAxis(pad, axis, value) => Self::GamepadAxis { tick, pad, axis, value },
            InputEvent::FocusLost => Self::FocusLost { tick },
            InputEvent::GamepadDisconnected(pad) => Self::GamepadDisconnected { tick, pad },
        }
    }

    fn from_check(tick: u64, check: &ScenarioCheck) -> Self {
        match check.clone() {
            ScenarioCheck::Action {
                name,
                pressed,
                down,
                released,
            } => Self::ExpectAction {
                tick,
                action: name,
                pressed,
                down,
                released,
            },
            ScenarioCheck::Resource { name, pointer, equals } => Self::ExpectResource {
                tick,
                resource: name,
                pointer,
                equals,
            },
            ScenarioCheck::Count { component, equals } => Self::ExpectCount {
                tick,
                component,
                equals,
            },
            ScenarioCheck::Ui {
                node,
                exists,
                shown,
                focused,
                text,
            } => Self::ExpectUi {
                tick,
                node,
                exists,
                shown,
                focused,
                text,
            },
        }
    }

    fn from_ui(tick: u64, step: &UiStep) -> Self {
        match step.clone() {
            UiStep::Click(node) => Self::UiClick { tick, node },
            UiStep::Type(node, text) => Self::UiType { tick, node, text },
        }
    }
}
