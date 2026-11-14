//! The performance report: where a game's time and allocations go, the
//! worst frames, warnings with advice, budgets for tests and CI, and
//! comparing two runs.
//!
//! A [`PerfReport`] comes from [`Perf::report`](crate::perf::Perf::report)
//! in a running game and from [`Game::profile_headless`](crate::game::Game::profile_headless)
//! for a headless run; `PONKO_BENCH` prints one, and the control endpoint's
//! `perf.get` returns one as JSON.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::alloc_count::{self, AllocCounts};
use crate::perf::SystemTiming;
use crate::profiler::percentile;
use crate::scope::{self, ScopeState, ScopeTiming};

/// Frames of history a running game keeps (about half a minute at 60 fps).
pub(crate) const HISTORY: usize = 1800;
/// The most a headless run keeps; a longer run reports its last frames.
pub(crate) const HEADLESS_HISTORY: usize = 50_000;
/// How many of the slowest frames a report lists.
const WORST_FRAMES: usize = 3;

// ------------------------------------------------------------------ report

/// Frame times, in milliseconds. In a headless run a frame is one tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameTimes {
    /// Average.
    pub avg: f32,
    /// Median.
    pub p50: f32,
    /// 95th percentile.
    pub p95: f32,
    /// 99th percentile: the stutters.
    pub p99: f32,
    /// The slowest frame.
    pub max: f32,
}

/// One system's cost, from [`PerfReport::systems`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemReport {
    /// The system's function name, e.g. `my_game::movement`.
    pub name: String,
    /// A simulation system (it may run several times per frame).
    pub sim: bool,
    /// Average time per frame, in ms.
    pub avg_ms: f32,
    /// Its slowest frame, in ms.
    pub max_ms: f32,
    /// Average heap allocations per frame (0 unless allocations are counted).
    pub allocs_per_frame: f32,
}

/// One profiling scope's cost, from [`PerfReport::scopes`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScopeReport {
    /// The scope's name.
    pub name: String,
    /// Average time per frame, in ms (all threads added together).
    pub avg_ms: f32,
    /// Its slowest frame, in ms.
    pub max_ms: f32,
    /// How often it ran per frame, on average.
    pub calls_per_frame: f32,
}

/// Heap allocations per frame, from [`PerfReport::allocs`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AllocReport {
    /// Allocations per frame, on average, over the whole process.
    pub per_frame: f32,
    /// Bytes requested per frame, on average.
    pub bytes_per_frame: f64,
    /// The most allocations in one frame.
    pub max_per_frame: u32,
}

/// Something that cost time in a [`WorstFrame`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contributor {
    /// A system or scope name.
    pub name: String,
    /// Its time in that frame, in ms.
    pub ms: f32,
    /// A scope (part of a system) rather than a system.
    pub scope: bool,
}

/// One of the slowest frames, from [`PerfReport::worst_frames`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorstFrame {
    /// The frame (or tick) number.
    pub frame: u64,
    /// How long it took, in ms.
    pub ms: f32,
    /// Allocations in it (0 unless allocations are counted).
    pub allocs: u32,
    /// What took the most time in it, slowest first.
    pub top: Vec<Contributor>,
}

/// One GPU pass, in [`RenderReport::gpu_passes`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PassReport {
    /// The pass's name ("shadows 0", "opaque", "bloom", ...).
    pub name: String,
    /// Its GPU time in ms.
    pub ms: f32,
}

/// What drawing cost, in a windowed game's [`PerfReport`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderReport {
    /// Time in systems per frame, in ms.
    pub systems_ms: f32,
    /// CPU time drawing (preparing, recording, submitting), in ms.
    pub render_cpu_ms: f32,
    /// Time waiting for the display, in ms (idle, not work).
    pub display_wait_ms: f32,
    /// GPU time per frame, in ms, where the GPU can measure it.
    pub gpu_ms: Option<f32>,
    /// GPU draw calls last frame.
    pub draw_calls: usize,
    /// Sprites drawn last frame.
    pub sprites: usize,
    /// 3D models drawn for the camera.
    pub models: usize,
    /// Triangles drawn for the camera.
    pub triangles: usize,
    /// 3D models drawn into shadow maps.
    pub shadow_models: usize,
    /// Triangles drawn into shadow maps.
    pub shadow_triangles: usize,
    /// 3D models skipped because something hid them.
    pub occluded: usize,
    /// GPU time per pass, slowest first.
    pub gpu_passes: Vec<PassReport>,
}

/// What a [`PerfWarning`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WarningKind {
    /// A system allocates on the heap every frame.
    AllocatesEveryFrame,
    /// One system takes most of the CPU time.
    Hotspot,
    /// A few frames are much slower than the rest.
    Spikes,
    /// The GPU takes most of the frame.
    GpuBound,
    /// The CPU takes most of the frame.
    CpuBound,
    /// Shadow maps draw far more triangles than the camera.
    HeavyShadows,
    /// Very many draw calls.
    ManyDrawCalls,
}

/// A problem the report found, and what to do about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfWarning {
    /// What kind of problem.
    pub kind: WarningKind,
    /// What was found, with numbers.
    pub message: String,
    /// What to try.
    pub hint: String,
}

/// Where a game's time and allocations go, over its last frames. It comes
/// from [`Perf::report`](crate::perf::Perf::report) in a running game and
/// [`Game::profile_headless`](crate::game::Game::profile_headless) in a headless run.
///
/// ```
/// use ponko::prelude::*;
///
/// fn spin(ctx: &mut Ctx) {
///     profile_scope!("spin/work");
///     let mut x = 0.0f32;
///     for i in 0..2000 { x += (i as f32).sin(); }
///     std::hint::black_box(x);
/// }
///
/// let (_world, report) = Game::new("t", 1, 1).sim_system(spin).profile_headless(2, 20, |_| {});
/// assert_eq!(report.frames, 20);
/// assert!(report.system("spin").is_some() && report.scope("spin/work").is_some());
/// println!("{report}");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerfReport {
    /// Frames (or ticks, in a headless run) the numbers cover.
    pub frames: usize,
    /// Entities in the world now.
    pub entities: usize,
    /// Frame times.
    pub frame_ms: FrameTimes,
    /// Systems, slowest first.
    pub systems: Vec<SystemReport>,
    /// Profiling scopes, slowest first. Scopes sit inside systems, so don't
    /// add them to the systems' times.
    pub scopes: Vec<ScopeReport>,
    /// Heap allocations, if they're counted
    /// ([`CountingAllocator`](crate::perf::CountingAllocator) is installed).
    pub allocs: Option<AllocReport>,
    /// The slowest frames and what was in them.
    pub worst_frames: Vec<WorstFrame>,
    /// Drawing (windowed games only).
    pub render: Option<RenderReport>,
    /// Problems found, worst first.
    pub warnings: Vec<PerfWarning>,
}

impl PerfReport {
    /// The system whose name is `name` or ends with `::name`.
    pub fn system(&self, name: &str) -> Option<&SystemReport> {
        self.systems.iter().find(|s| name_matches(&s.name, name))
    }

    /// The scope called `name`.
    pub fn scope(&self, name: &str) -> Option<&ScopeReport> {
        self.scopes.iter().find(|s| s.name == name)
    }

    /// The report as pretty JSON, to keep and [compare](Self::regressions) later.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Reads a report written by [`to_json`](Self::to_json).
    ///
    /// # Errors
    /// [`ErrorKind::Json`](crate::error::ErrorKind::Json) if the text isn't a report.
    pub fn from_json(json: &str) -> Result<Self, crate::error::Error> {
        serde_json::from_str(json).map_err(|e| crate::error::Error::from(e).context("not a performance report"))
    }

    /// Writes [`to_json`](Self::to_json) to `path`.
    ///
    /// # Errors
    /// If the file can't be written.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, self.to_json())
    }

    /// Reads a report saved with [`save`](Self::save).
    ///
    /// # Errors
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if there's no such
    /// file, [`Io`](crate::error::ErrorKind::Io) if it can't be read,
    /// [`Json`](crate::error::ErrorKind::Json) if it isn't a report. The error's
    /// path is `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, crate::error::Error> {
        let path = path.as_ref();
        let shown = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|e| {
            let kind = if e.kind() == std::io::ErrorKind::NotFound {
                crate::error::ErrorKind::NotFound
            } else {
                crate::error::ErrorKind::Io
            };
            crate::error::Error::new(kind, format!("{shown}: {e}")).with_path(shown.clone())
        })?;
        Self::from_json(&text).map_err(|e| e.with_path(shown))
    }

    /// Where `budget` is broken. Empty means every limit holds.
    pub fn check(&self, budget: &Budget) -> Vec<BudgetViolation> {
        let mut out = Vec::new();
        let over = |what: String, limit: Option<f32>, actual: f32| {
            limit.filter(|&l| actual > l).map(|limit| BudgetViolation {
                what,
                limit,
                actual: Some(actual),
            })
        };
        let missing = |what: String, limit: f32| BudgetViolation {
            what,
            limit,
            actual: None,
        };
        out.extend(over(
            "frame time, average (ms)".into(),
            budget.frame_avg_ms,
            self.frame_ms.avg,
        ));
        out.extend(over(
            "frame time, p99 (ms)".into(),
            budget.frame_p99_ms,
            self.frame_ms.p99,
        ));
        out.extend(over(
            "frame time, slowest (ms)".into(),
            budget.frame_max_ms,
            self.frame_ms.max,
        ));
        if let Some(max) = budget.allocs_per_frame {
            match &self.allocs {
                Some(a) => out.extend(over("allocations per frame".into(), Some(max), a.per_frame)),
                None => out.push(missing(
                    "allocations per frame (not counted: install CountingAllocator)".into(),
                    max,
                )),
            }
        }
        for (name, max) in &budget.systems {
            match self.system(name) {
                Some(s) => out.extend(over(format!("system `{}` (ms)", s.name), Some(*max), s.avg_ms)),
                None => out.push(missing(format!("system `{name}` (no such system)"), *max)),
            }
        }
        for (name, max) in &budget.scopes {
            match self.scope(name) {
                Some(s) => out.extend(over(format!("scope `{name}` (ms)"), Some(*max), s.avg_ms)),
                None => out.push(missing(format!("scope `{name}` (never ran)"), *max)),
            }
        }
        out
    }

    /// Panics, listing what's over budget and what the report suggests, if
    /// any limit in `budget` is broken. For tests and CI.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// let (_w, report) = Game::new("t", 1, 1).sim_system(|_: &mut Ctx| {}).profile_headless(0, 10, |_| {});
    /// report.assert_within(&ponko::perf::Budget::new().frame_p99_ms(50.0));
    /// ```
    ///
    /// # Panics
    /// When a limit is broken.
    #[track_caller]
    pub fn assert_within(&self, budget: &Budget) {
        let broken = self.check(budget);
        if broken.is_empty() {
            return;
        }
        let mut msg = String::from("performance budget broken:\n");
        for v in &broken {
            let _ = writeln!(msg, "  - {v}");
        }
        let _ = write!(msg, "\n{self}");
        panic!("{msg}");
    }

    /// What got slower than in `baseline`, a report from an earlier run:
    /// frame times, allocations, and each system and scope by name. A value
    /// counts when it grew by more than `tolerance` (0.2 is 20%) and by more
    /// than 0.05 ms (or one allocation), so noise in tiny numbers is ignored.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// # let (_w, report) = Game::new("t", 1, 1).sim_system(|_: &mut Ctx| {}).profile_headless(0, 5, |_| {});
    /// # let baseline = report.clone();
    /// assert!(report.regressions(&baseline, 0.25).is_empty());
    /// ```
    pub fn regressions(&self, baseline: &PerfReport, tolerance: f32) -> Vec<Regression> {
        let mut out = Vec::new();
        let mut cmp = |what: String, before: f32, after: f32, floor: f32| {
            if after > before * (1.0 + tolerance) && after - before > floor {
                out.push(Regression { what, before, after });
            }
        };
        cmp(
            "frame time, average (ms)".into(),
            baseline.frame_ms.avg,
            self.frame_ms.avg,
            0.05,
        );
        cmp(
            "frame time, p99 (ms)".into(),
            baseline.frame_ms.p99,
            self.frame_ms.p99,
            0.05,
        );
        if let (Some(b), Some(a)) = (&baseline.allocs, &self.allocs) {
            cmp("allocations per frame".into(), b.per_frame, a.per_frame, 1.0);
        }
        for s in &self.systems {
            if let Some(b) = baseline.systems.iter().find(|b| b.name == s.name) {
                cmp(format!("system `{}` (ms)", s.name), b.avg_ms, s.avg_ms, 0.05);
                cmp(
                    format!("system `{}` allocations", s.name),
                    b.allocs_per_frame,
                    s.allocs_per_frame,
                    1.0,
                );
            }
        }
        for s in &self.scopes {
            if let Some(b) = baseline.scopes.iter().find(|b| b.name == s.name) {
                cmp(format!("scope `{}` (ms)", s.name), b.avg_ms, s.avg_ms, 0.05);
            }
        }
        out
    }
}

fn name_matches(full: &str, wanted: &str) -> bool {
    full == wanted || full.strip_suffix(wanted).is_some_and(|rest| rest.ends_with("::"))
}

impl fmt::Display for PerfReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = &self.frame_ms;
        writeln!(
            f,
            "== performance report: {} frames, {} entities ==",
            self.frames, self.entities
        )?;
        writeln!(
            f,
            "frame ms: avg {:.3}  p50 {:.3}  p95 {:.3}  p99 {:.3}  max {:.3}",
            t.avg, t.p50, t.p95, t.p99, t.max
        )?;
        if let Some(a) = &self.allocs {
            writeln!(
                f,
                "allocations per frame: {:.1} ({:.1} KB), at most {}",
                a.per_frame,
                a.bytes_per_frame / 1024.0,
                a.max_per_frame
            )?;
        }
        if let Some(r) = &self.render {
            writeln!(
                f,
                "cpu: systems {:.2}  render {:.2}  display wait {:.2}   gpu {}",
                r.systems_ms,
                r.render_cpu_ms,
                r.display_wait_ms,
                r.gpu_ms.map_or("n/a".into(), |g| format!("{g:.2} ms"))
            )?;
            writeln!(
                f,
                "draw calls {}  sprites {}  models {} ({:.2}M tris)  shadows {} models ({:.2}M tris)  occluded {}",
                r.draw_calls,
                r.sprites,
                r.models,
                r.triangles as f64 / 1e6,
                r.shadow_models,
                r.shadow_triangles as f64 / 1e6,
                r.occluded
            )?;
            if !r.gpu_passes.is_empty() {
                writeln!(f, "-- gpu passes (ms) --")?;
                for p in r.gpu_passes.iter().take(10) {
                    writeln!(f, "{:>8.3}  {}", p.ms, p.name)?;
                }
            }
        }
        if !self.systems.is_empty() {
            writeln!(f, "-- systems (avg ms, max ms, allocs/frame) --")?;
            for s in self.systems.iter().take(15) {
                writeln!(
                    f,
                    "{:>8.3} {:>8.3} {:>8.1}  {}",
                    s.avg_ms, s.max_ms, s.allocs_per_frame, s.name
                )?;
            }
        }
        if !self.scopes.is_empty() {
            writeln!(f, "-- scopes (avg ms, max ms, calls/frame) --")?;
            for s in self.scopes.iter().take(15) {
                writeln!(
                    f,
                    "{:>8.3} {:>8.3} {:>8.1}  {}",
                    s.avg_ms, s.max_ms, s.calls_per_frame, s.name
                )?;
            }
        }
        if !self.worst_frames.is_empty() {
            writeln!(f, "-- slowest frames --")?;
            for w in &self.worst_frames {
                write!(f, "frame {}: {:.2} ms", w.frame, w.ms)?;
                if w.allocs > 0 {
                    write!(f, ", {} allocations", w.allocs)?;
                }
                let top: Vec<_> = w.top.iter().map(|c| format!("{} {:.2}", c.name, c.ms)).collect();
                if !top.is_empty() {
                    write!(f, "  [{}]", top.join(", "))?;
                }
                writeln!(f)?;
            }
        }
        if !self.warnings.is_empty() {
            writeln!(f, "-- warnings --")?;
            for w in &self.warnings {
                writeln!(f, "! {}\n    -> {}", w.message, w.hint)?;
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ budget

/// Limits for a [`PerfReport`], checked with
/// [`PerfReport::assert_within`] in tests and CI. Everything starts unlimited.
///
/// Timings vary between machines; allocation counts don't, so an
/// `allocs_per_frame(0.0)` budget is the one to rely on in CI, with generous
/// time limits next to it. Budgets are also JSON (`{"frame_p99_ms": 2,
/// "systems": {"movement": 0.5}}`), which is how C and C++ pass them.
///
/// ```
/// use ponko::perf::Budget;
/// let budget = ponko::perf::Budget::new()
///     .frame_p99_ms(2.0)
///     .allocs_per_frame(0.0)
///     .system("movement", 0.5)
///     .scope("ai/pathing", 1.0);
/// assert_eq!(budget.systems["movement"], 0.5);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    /// Most average frame time, in ms.
    pub frame_avg_ms: Option<f32>,
    /// Most 99th percentile frame time, in ms.
    pub frame_p99_ms: Option<f32>,
    /// Most time for the slowest frame, in ms.
    pub frame_max_ms: Option<f32>,
    /// Most heap allocations per frame. Needs the counting allocator.
    pub allocs_per_frame: Option<f32>,
    /// Most average ms per frame for systems. Names match the full name or
    /// its last part (`movement` matches `my_game::movement`).
    pub systems: BTreeMap<String, f32>,
    /// Most average ms per frame for scopes.
    pub scopes: BTreeMap<String, f32>,
}

impl Budget {
    /// No limits yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Limits the average frame time.
    pub fn frame_avg_ms(mut self, ms: f32) -> Self {
        self.frame_avg_ms = Some(ms);
        self
    }

    /// Limits the 99th percentile frame time.
    pub fn frame_p99_ms(mut self, ms: f32) -> Self {
        self.frame_p99_ms = Some(ms);
        self
    }

    /// Limits the slowest frame.
    pub fn frame_max_ms(mut self, ms: f32) -> Self {
        self.frame_max_ms = Some(ms);
        self
    }

    /// Limits heap allocations per frame (0.0: none).
    pub fn allocs_per_frame(mut self, n: f32) -> Self {
        self.allocs_per_frame = Some(n);
        self
    }

    /// Limits a system's average time per frame.
    pub fn system(mut self, name: &str, ms: f32) -> Self {
        self.systems.insert(name.into(), ms);
        self
    }

    /// Limits a scope's average time per frame.
    pub fn scope(mut self, name: &str, ms: f32) -> Self {
        self.scopes.insert(name.into(), ms);
        self
    }

    /// Reads a budget from JSON.
    ///
    /// # Errors
    /// [`ErrorKind::Json`](crate::error::ErrorKind::Json) if the text isn't a budget.
    pub fn from_json(json: &str) -> Result<Self, crate::error::Error> {
        serde_json::from_str(json).map_err(|e| crate::error::Error::from(e).context("not a budget"))
    }
}

/// A limit in a [`Budget`] that the report went over.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetViolation {
    /// What was limited.
    pub what: String,
    /// The limit.
    pub limit: f32,
    /// What was measured; `None` if it couldn't be (no such system).
    pub actual: Option<f32>,
}

impl fmt::Display for BudgetViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.actual {
            Some(a) => write!(f, "{}: {a:.3}, limit {:.3}", self.what, self.limit),
            None => write!(f, "{}: nothing measured, limit {:.3}", self.what, self.limit),
        }
    }
}

/// Something that got slower between two reports, from
/// [`PerfReport::regressions`].
#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    /// What got slower.
    pub what: String,
    /// Its value in the baseline.
    pub before: f32,
    /// Its value now.
    pub after: f32,
}

impl fmt::Display for Regression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:.3} -> {:.3}", self.what, self.before, self.after)
    }
}

// --------------------------------------------------------------- collection

/// A system's or scope's per-frame history.
struct Series {
    name: &'static str,
    sim: bool,
    ms: Vec<f32>,
    /// Allocations (systems) or calls (scopes).
    n: Vec<u32>,
}

impl Series {
    fn new(name: &'static str, sim: bool, len: usize, cap: usize) -> Self {
        let mut ms = Vec::with_capacity(cap);
        ms.resize(len, 0.0);
        let mut n = Vec::with_capacity(cap);
        n.resize(len, 0);
        Self { name, sim, ms, n }
    }
}

fn set<T>(v: &mut Vec<T>, i: usize, value: T) {
    if i == v.len() {
        v.push(value);
    } else {
        v[i] = value;
    }
}

/// The last frames' timings, kept in rings so recording a frame allocates
/// nothing once every system and scope has run.
pub(crate) struct Collector {
    cap: usize,
    next: usize,
    frames: Vec<u64>,
    frame_ms: Vec<f32>,
    allocs: Vec<u32>,
    alloc_bytes: Vec<u64>,
    systems: Vec<Series>,
    scopes: Vec<Series>,
}

impl Collector {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            next: 0,
            frames: Vec::new(),
            frame_ms: Vec::new(),
            allocs: Vec::new(),
            alloc_bytes: Vec::new(),
            systems: Vec::new(),
            scopes: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new(self.cap);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.frame_ms.len()
    }

    /// Records one frame.
    pub fn push(
        &mut self,
        frame: u64,
        frame_ms: f32,
        frame_allocs: AllocCounts,
        systems: &[SystemTiming],
        system_allocs: &[u32],
        scopes: &[ScopeTiming],
    ) {
        let same_systems = self.systems.len() == systems.len()
            && self
                .systems
                .iter()
                .zip(systems)
                .all(|(a, b)| std::ptr::eq(a.name, b.name) || a.name == b.name);
        if !same_systems {
            self.clear();
            self.systems = systems
                .iter()
                .map(|t| Series::new(t.name, t.sim, 0, self.cap))
                .collect();
        }
        let len = self.frame_ms.len();
        while self.scopes.len() < scopes.len() {
            let t = &scopes[self.scopes.len()];
            self.scopes.push(Series::new(t.name, false, len, self.cap));
        }
        let i = if len >= self.cap {
            let i = self.next;
            self.next = (i + 1) % self.cap;
            i
        } else {
            len
        };
        set(&mut self.frames, i, frame);
        set(&mut self.frame_ms, i, frame_ms);
        set(&mut self.allocs, i, frame_allocs.count.min(u64::from(u32::MAX)) as u32);
        set(&mut self.alloc_bytes, i, frame_allocs.bytes);
        for (k, (s, t)) in self.systems.iter_mut().zip(systems).enumerate() {
            set(&mut s.ms, i, t.last_ms);
            set(&mut s.n, i, system_allocs.get(k).copied().unwrap_or(0));
        }
        for (k, s) in self.scopes.iter_mut().enumerate() {
            let t = scopes.get(k);
            set(&mut s.ms, i, t.map_or(0.0, |t| t.last_ms));
            set(&mut s.n, i, t.map_or(0, |t| t.calls));
        }
    }

    /// The report for the frames kept. `entities`, `render` and the warnings
    /// are added by the caller.
    pub fn report(&self) -> PerfReport {
        let n = self.frame_ms.len();
        if n == 0 {
            return PerfReport::default();
        }
        let nf = n as f32;
        let mut sorted = self.frame_ms.clone();
        sorted.sort_unstable_by(f32::total_cmp);
        let frame_ms = FrameTimes {
            avg: self.frame_ms.iter().sum::<f32>() / nf,
            p50: percentile(&sorted, 0.50),
            p95: percentile(&sorted, 0.95),
            p99: percentile(&sorted, 0.99),
            max: sorted[n - 1],
        };
        let max = |v: &[f32]| v.iter().copied().fold(0.0, f32::max);
        let mut systems: Vec<_> = self
            .systems
            .iter()
            .map(|s| SystemReport {
                name: s.name.into(),
                sim: s.sim,
                avg_ms: s.ms.iter().sum::<f32>() / nf,
                max_ms: max(&s.ms),
                allocs_per_frame: s.n.iter().map(|&a| a as f32).sum::<f32>() / nf,
            })
            .collect();
        systems.sort_by(|a, b| b.avg_ms.total_cmp(&a.avg_ms));
        let mut scopes: Vec<_> = self
            .scopes
            .iter()
            .map(|s| ScopeReport {
                name: s.name.into(),
                avg_ms: s.ms.iter().sum::<f32>() / nf,
                max_ms: max(&s.ms),
                calls_per_frame: s.n.iter().map(|&c| c as f32).sum::<f32>() / nf,
            })
            .collect();
        scopes.sort_by(|a, b| b.avg_ms.total_cmp(&a.avg_ms));
        let allocs = alloc_count::counting().then(|| AllocReport {
            per_frame: self.allocs.iter().map(|&a| a as f32).sum::<f32>() / nf,
            bytes_per_frame: self.alloc_bytes.iter().sum::<u64>() as f64 / n as f64,
            max_per_frame: self.allocs.iter().copied().max().unwrap_or(0),
        });
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| self.frame_ms[b].total_cmp(&self.frame_ms[a]));
        let worst_frames = order
            .into_iter()
            .take(WORST_FRAMES)
            .map(|i| {
                let mut top: Vec<Contributor> = self
                    .systems
                    .iter()
                    .map(|s| (s, false))
                    .chain(self.scopes.iter().map(|s| (s, true)))
                    .filter(|(s, _)| s.ms[i] > 0.0)
                    .map(|(s, scope)| Contributor {
                        name: s.name.into(),
                        ms: s.ms[i],
                        scope,
                    })
                    .collect();
                top.sort_by(|a, b| b.ms.total_cmp(&a.ms));
                top.truncate(3);
                WorstFrame {
                    frame: self.frames[i],
                    ms: self.frame_ms[i],
                    allocs: self.allocs[i],
                    top,
                }
            })
            .collect();
        PerfReport {
            frames: n,
            frame_ms,
            systems,
            scopes,
            allocs,
            worst_frames,
            ..Default::default()
        }
    }
}

/// A game's profiling state: scopes, allocation counts and the history
/// behind [`PerfReport`].
pub(crate) struct PerfState {
    /// Profiling is running (scopes and allocation counting count).
    pub on: bool,
    /// `Perf::set_profiling(true)` was called.
    pub explicit: bool,
    pub scopes: ScopeState,
    pub collector: Collector,
    /// Allocations per system in this frame, by system index.
    pub system_allocs: Vec<u32>,
    mark: AllocCounts,
    pub frame_allocs: AllocCounts,
    pub frames: u64,
    /// The overlay's extra lines (scopes, allocations, warnings).
    pub hud_extra: String,
}

impl PerfState {
    pub fn new() -> Self {
        Self {
            on: false,
            explicit: false,
            scopes: ScopeState::default(),
            collector: Collector::new(HISTORY),
            system_allocs: Vec::new(),
            mark: AllocCounts::default(),
            frame_allocs: AllocCounts::default(),
            frames: 0,
            hud_extra: String::new(),
        }
    }

    /// Turns profiling on or off as wanted.
    pub fn sync(&mut self, wanted: bool) {
        if wanted == self.on {
            return;
        }
        self.on = wanted;
        if wanted {
            scope::enable();
            self.scopes.rebase();
            self.mark = alloc_count::alloc_counts();
        } else {
            scope::disable();
            self.hud_extra.clear();
        }
    }

    /// Starts a frame with `n` systems.
    pub fn begin_frame(&mut self, n: usize) {
        self.system_allocs.clear();
        self.system_allocs.resize(n, 0);
        if self.on {
            self.mark = alloc_count::alloc_counts();
        }
    }

    /// Charges `n` allocations to system `i`.
    pub fn add_system_allocs(&mut self, i: usize, n: u64) {
        if let Some(slot) = self.system_allocs.get_mut(i) {
            *slot = slot.saturating_add(n.min(u64::from(u32::MAX)) as u32);
        }
    }

    /// Ends a frame that took `frame_ms`.
    pub fn end_frame(&mut self, frame_ms: f32, timings: &[SystemTiming]) {
        if !self.on {
            return;
        }
        self.frames += 1;
        self.scopes.end_frame();
        let now = alloc_count::alloc_counts();
        self.frame_allocs = AllocCounts {
            count: now.count.saturating_sub(self.mark.count),
            bytes: now.bytes.saturating_sub(self.mark.bytes),
        };
        self.collector.push(
            self.frames,
            frame_ms,
            self.frame_allocs,
            timings,
            &self.system_allocs,
            &self.scopes.timings,
        );
    }

    /// `true` about twice a second while the overlay is up: time to
    /// rebuild its extra lines.
    pub fn hud_due(&self) -> bool {
        self.hud_extra.is_empty() || self.frames.is_multiple_of(30)
    }

    /// Leaves out of this frame's count what was allocated since `before`.
    pub fn forgive_allocs(&mut self, before: AllocCounts) {
        let now = alloc_count::alloc_counts();
        self.mark.count += now.count.saturating_sub(before.count);
        self.mark.bytes += now.bytes.saturating_sub(before.bytes);
    }
}

impl Drop for PerfState {
    fn drop(&mut self) {
        if self.on {
            scope::disable();
        }
    }
}

// ------------------------------------------------------------------ advice

/// Looks at `report` for known problems and adds the warnings.
pub(crate) fn analyze(report: &mut PerfReport) {
    let mut w = Vec::new();
    let avg = report.frame_ms.avg;
    let cpu_ms: f32 = report.systems.iter().map(|s| s.avg_ms).sum();

    if report.allocs.is_some() {
        for s in report.systems.iter().filter(|s| s.allocs_per_frame >= 1.0) {
            w.push(PerfWarning {
                kind: WarningKind::AllocatesEveryFrame,
                message: format!("`{}` allocates {:.0} times per frame", s.name, s.allocs_per_frame),
                hint: "Keep the buffer between frames instead: a Vec in a resource or a captured variable, cleared \
                       and refilled. Avoid format!, to_string, collect and Box in per-frame code. Wrap suspect lines \
                       in profile_scope! and watch the allocation count to find the one."
                    .into(),
            });
        }
    }
    if let Some(s) = report.systems.first()
        && s.avg_ms >= 1.0
        && cpu_ms > 0.0
        && s.avg_ms / cpu_ms > 0.5
    {
        w.push(PerfWarning {
            kind: WarningKind::Hotspot,
            message: format!(
                "`{}` takes {:.0}% of the time spent in systems ({:.2} of {:.2} ms)",
                s.name,
                100.0 * s.avg_ms / cpu_ms,
                s.avg_ms,
                cpu_ms
            ),
            hint: "Wrap parts of it in profile_scope! to see where the time goes. Then: iterate with for_each / \
                   par_for_each, spread per-entity work over frames with ctx.time.turns(n), or run it less often with \
                   an interval: system.every(n)."
                .into(),
        });
    }
    let t = &report.frame_ms;
    if report.frames >= 30 && t.p50 > 0.0 && t.max >= 4.0 * t.p50 && t.max - t.p50 >= 2.0 {
        let worst = report.worst_frames.first();
        let culprit = worst
            .and_then(|f| f.top.first())
            .map(|c| format!("; its biggest cost was `{}` ({:.2} ms)", c.name, c.ms))
            .unwrap_or_default();
        w.push(PerfWarning {
            kind: WarningKind::Spikes,
            message: format!(
                "frame spikes: the slowest frame ({}) took {:.2} ms, {:.1}x the median ({:.2} ms){culprit}",
                worst.map_or(0, |f| f.frame),
                t.max,
                t.max / t.p50,
                t.p50
            ),
            hint: "Spikes come from allocation bursts, loading assets, spawning many entities at once, or a system \
                   that sometimes does a lot (path searches, rebuilds). Spread such work over frames with \
                   ctx.time.turns(n), or do it ahead of time. ctx.perf.record_trace shows a slow frame on a timeline."
                .into(),
        });
    }
    if let Some(r) = &report.render {
        if let Some(gpu) = r.gpu_ms
            && avg > 0.0
            && gpu >= 0.8 * avg
        {
            let (pass, hint) = match r.gpu_passes.first() {
                Some(p) if p.name.contains("shadow") => (
                    format!("; the slowest pass is `{}` ({:.2} ms)", p.name, p.ms),
                    "Fewer or smaller shadow casters, a ShadowMesh for detailed models, set_mesh_shadows(false) for \
                     small things, or fewer cascades.",
                ),
                Some(p) => (
                    format!("; the slowest pass is `{}` ({:.2} ms)", p.name, p.ms),
                    "Draw fewer or smaller things, lower ctx.scene3d.post.msaa, or use cheaper shaders. gpu_passes says which \
                     pass; render_stats says whether it's triangles or pixels.",
                ),
                None => (
                    String::new(),
                    "Draw fewer or smaller things, or lower ctx.scene3d.post.msaa. Compare render_stats triangles with pixels.",
                ),
            };
            w.push(PerfWarning {
                kind: WarningKind::GpuBound,
                message: format!("GPU-bound: the GPU needs {gpu:.2} ms of the {avg:.2} ms frame{pass}"),
                hint: hint.into(),
            });
        } else if avg > 0.0 && r.systems_ms + r.render_cpu_ms >= 0.8 * avg {
            w.push(PerfWarning {
                kind: WarningKind::CpuBound,
                message: format!(
                    "CPU-bound: systems {:.2} ms + drawing {:.2} ms of the {avg:.2} ms frame",
                    r.systems_ms, r.render_cpu_ms
                ),
                hint: "Look at the slowest systems and scopes. If drawing dominates, check draw calls and models \
                       drawn (render_stats), and cull or merge what you can."
                    .into(),
            });
        }
        if r.shadow_triangles >= 200_000 && r.shadow_triangles >= 2 * r.triangles.max(1) {
            w.push(PerfWarning {
                kind: WarningKind::HeavyShadows,
                message: format!(
                    "shadow maps draw {:.1}M triangles, {:.1}x what the camera draws",
                    r.shadow_triangles as f64 / 1e6,
                    r.shadow_triangles as f64 / r.triangles.max(1) as f64
                ),
                hint: "Every caster is drawn again for each cascade it falls in. Give detailed models a simpler \
                       ShadowMesh, turn shadows off for small things (set_mesh_shadows), or shorten the shadow distance."
                    .into(),
            });
        }
        if r.draw_calls >= 1000 {
            w.push(PerfWarning {
                kind: WarningKind::ManyDrawCalls,
                message: format!("{} draw calls per frame", r.draw_calls),
                hint: "Each run of sprites sharing a texture, and each mesh part with its material, is one call. \
                       Pack sprites into an atlas, share meshes and materials between objects, and merge static \
                       geometry. ctx.gfx.mesh_costs lists the meshes."
                    .into(),
            });
        }
    }
    report.warnings = w;
}

/// The overlay's extra lines: scopes, allocations and the worst warnings.
pub(crate) fn hud_lines(out: &mut String, report: &PerfReport) {
    out.clear();
    if let Some(a) = &report.allocs {
        let _ = writeln!(
            out,
            "allocs/frame {:.1} ({:.1} KB)",
            a.per_frame,
            a.bytes_per_frame / 1024.0
        );
    }
    if !report.scopes.is_empty() {
        out.push_str("-- scopes\n");
        for s in report.scopes.iter().take(6) {
            let _ = writeln!(out, "{:>6.2}  {} (x{:.0})", s.avg_ms, s.name, s.calls_per_frame);
        }
    }
    for w in report.warnings.iter().take(3) {
        let mut msg: String = w.message.chars().take(60).collect();
        if msg.len() < w.message.len() {
            msg.push('…');
        }
        let _ = writeln!(out, "! {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(name: &'static str, ms: f32) -> SystemTiming {
        SystemTiming {
            name,
            sim: true,
            last_ms: ms,
            avg_ms: ms,
        }
    }

    fn collector_with(frames: &[(f32, f32, f32)]) -> Collector {
        let mut c = Collector::new(100);
        for (i, &(total, a, b)) in frames.iter().enumerate() {
            c.push(
                i as u64 + 1,
                total,
                AllocCounts::default(),
                &[timing("g::a", a), timing("g::b", b)],
                &[0, 0],
                &[],
            );
        }
        c
    }

    #[test]
    fn reports_sort_systems_and_find_the_worst_frame() {
        let mut frames = vec![(2.0, 1.0, 0.5); 99];
        frames.push((30.0, 1.0, 25.0));
        let r = collector_with(&frames).report();
        assert_eq!(r.frames, 100);
        assert_eq!(r.systems[0].name, "g::a".to_string().as_str().to_owned().as_str());
        assert!((r.frame_ms.avg - 2.28).abs() < 1e-3);
        assert_eq!((r.frame_ms.p50, r.frame_ms.max), (2.0, 30.0));
        let worst = &r.worst_frames[0];
        assert_eq!((worst.frame, worst.ms), (100, 30.0));
        assert_eq!(worst.top[0].name, "g::b");
        assert!(r.system("b").is_some() && r.system("g::b").is_some() && r.system("::b").is_none());
    }

    #[test]
    fn the_ring_keeps_the_latest_frames() {
        let mut c = Collector::new(10);
        for i in 0..25u64 {
            c.push(
                i + 1,
                i as f32,
                AllocCounts::default(),
                &[timing("s", i as f32)],
                &[0],
                &[],
            );
        }
        let r = c.report();
        assert_eq!(r.frames, 10);
        assert_eq!(r.frame_ms.max, 24.0);
        assert!((r.systems[0].avg_ms - 19.5).abs() < 1e-4);
        assert_eq!(r.worst_frames[0].frame, 25);
    }

    #[test]
    fn scopes_that_appear_later_are_zero_before() {
        let mut c = Collector::new(10);
        let sys = [timing("s", 1.0)];
        c.push(1, 1.0, AllocCounts::default(), &sys, &[0], &[]);
        let scope = [ScopeTiming {
            name: "late",
            last_ms: 2.0,
            avg_ms: 2.0,
            calls: 4,
        }];
        c.push(2, 1.0, AllocCounts::default(), &sys, &[0], &scope);
        let r = c.report();
        assert_eq!(r.scope("late").map(|s| (s.avg_ms, s.calls_per_frame)), Some((1.0, 2.0)));
    }

    #[test]
    fn changing_the_systems_starts_the_history_over() {
        let mut c = collector_with(&[(1.0, 1.0, 1.0); 5]);
        c.push(6, 1.0, AllocCounts::default(), &[timing("only", 1.0)], &[0], &[]);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn budgets_name_what_broke() {
        let r = collector_with(&[(2.0, 1.0, 0.5); 40]).report();
        assert!(r.check(&Budget::new().frame_p99_ms(3.0).system("a", 1.5)).is_empty());
        let broken = r.check(
            &Budget::new()
                .frame_p99_ms(1.0)
                .system("b", 0.1)
                .system("nope", 1.0)
                .scope("x", 1.0),
        );
        let text: Vec<_> = broken.iter().map(ToString::to_string).collect();
        assert_eq!(broken.len(), 4, "{text:?}");
        assert!(text.iter().any(|t| t.contains("`g::b`")));
        assert!(text.iter().any(|t| t.contains("no such system")));
        assert!(text.iter().any(|t| t.contains("never ran")));
    }

    #[test]
    #[should_panic(expected = "performance budget broken")]
    fn assert_within_panics_with_the_report() {
        collector_with(&[(2.0, 1.0, 0.5); 10])
            .report()
            .assert_within(&Budget::new().frame_avg_ms(1.0));
    }

    #[test]
    fn budgets_come_from_json() {
        let b = Budget::from_json(r#"{"frame_p99_ms": 2.5, "systems": {"movement": 0.5}}"#).unwrap();
        assert_eq!(b, Budget::new().frame_p99_ms(2.5).system("movement", 0.5));
        assert!(Budget::from_json("3").is_err());
    }

    #[test]
    fn reports_round_trip_and_compare() {
        let before = collector_with(&[(2.0, 1.0, 0.5); 40]).report();
        let same = PerfReport::from_json(&before.to_json()).unwrap();
        assert_eq!(same, before);
        assert!(same.regressions(&before, 0.1).is_empty());
        let after = collector_with(&[(3.0, 2.0, 0.5); 40]).report();
        let slower = after.regressions(&before, 0.1);
        let text: Vec<_> = slower.iter().map(ToString::to_string).collect();
        assert!(text.iter().any(|t| t.contains("system `g::a`")), "{text:?}");
        assert!(!text.iter().any(|t| t.contains("g::b")), "{text:?}");
        assert!(PerfReport::from_json("nope").is_err());
    }

    #[test]
    fn warnings_explain_spikes_hotspots_and_allocations() {
        let mut frames = vec![(2.0, 1.5, 0.1); 99];
        frames.push((40.0, 1.5, 30.0));
        let mut r = collector_with(&frames).report();
        r.allocs = Some(AllocReport::default());
        r.systems[0].allocs_per_frame = 3.0;
        analyze(&mut r);
        let kinds: Vec<_> = r.warnings.iter().map(|w| w.kind).collect();
        assert!(kinds.contains(&WarningKind::AllocatesEveryFrame), "{r}");
        assert!(kinds.contains(&WarningKind::Hotspot), "{r}");
        assert!(kinds.contains(&WarningKind::Spikes), "{r}");
        let spikes = r.warnings.iter().find(|w| w.kind == WarningKind::Spikes).unwrap();
        assert!(
            spikes.message.contains("`g::b`"),
            "names the culprit: {}",
            spikes.message
        );
        assert!(r.to_string().contains("-> "), "the text report carries the hints");
    }

    #[test]
    fn render_warnings_name_the_slow_pass() {
        let mut r = collector_with(&[(16.0, 1.0, 1.0); 40]).report();
        r.render = Some(RenderReport {
            gpu_ms: Some(15.0),
            gpu_passes: vec![PassReport {
                name: "shadows 0".into(),
                ms: 9.0,
            }],
            shadow_triangles: 5_000_000,
            triangles: 1_000_000,
            draw_calls: 4000,
            ..Default::default()
        });
        analyze(&mut r);
        let kinds: Vec<_> = r.warnings.iter().map(|w| w.kind).collect();
        assert_eq!(
            kinds,
            [
                WarningKind::GpuBound,
                WarningKind::HeavyShadows,
                WarningKind::ManyDrawCalls
            ]
        );
        assert!(r.warnings[0].message.contains("shadows 0"));
    }

    #[test]
    fn quiet_games_get_no_warnings() {
        let mut r = collector_with(&[(2.0, 0.5, 0.5); 60]).report();
        analyze(&mut r);
        assert!(r.warnings.is_empty(), "{r}");
    }
}
