//! Differential tests: seeded random sequences of spawn, despawn, insert
//! (and replace), remove, mutation and clear run against a [`World`] and
//! against a plain reference model built from std collections. After every
//! operation the two are compared: live and dead handles, component values
//! and membership, query results and exactly which component values were
//! dropped.
//!
//! A failing case prints its seed. Rerun just that case with
//! `PONKO_ECS_SEED=<seed> cargo test -p ponko_ecs --test differential`
//! (decimal or `0x` hex). `PONKO_ECS_CASES` and `PONKO_ECS_OPS` change how
//! many seeds run and how long each sequence is. Under Miri the defaults
//! shrink so the run stays affordable.
//!
//! Not covered yet: `spawn_at`, `reset`, commands, dynamic components, and
//! components whose `Drop` panics.

mod common;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, Once};

use common::Rng;
use ponko_ecs::*;

// ---- components ---------------------------------------------------------------

/// Plain `Copy` data.
#[derive(Debug, Clone, Copy, PartialEq, Component)]
struct Num(u32);

/// Not `Copy`, owns heap memory, and records its serial number when dropped,
/// so the test knows exactly which values were dropped and how often.
#[derive(Component)]
struct Tracked {
    serial: u64,
    name: String,
    log: DropLog,
}

type DropLog = Arc<Mutex<Vec<u64>>>;

impl Tracked {
    fn new(serial: u64, log: &DropLog) -> Self {
        Tracked {
            serial,
            name: format!("tracked-{serial}"),
            log: log.clone(),
        }
    }

    fn intact(&self) -> bool {
        self.name == format!("tracked-{}", self.serial)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        lock(&self.log).push(self.serial);
    }
}

/// Zero-sized.
#[derive(Debug, PartialEq, Component)]
struct Marker;

/// Over-aligned: storage must keep every row on a 32-byte boundary.
#[repr(align(32))]
#[derive(Debug, Clone, Copy, PartialEq, Component)]
struct Aligned(u64);

/// Components of short-lived entities outside the model, which only churn
/// the world through many archetypes so the model's empty ones get
/// reclaimed and their slots and buffers reused. Half share a layout with
/// `Num`, half with `Aligned`.
#[allow(dead_code)] // Only their layouts matter.
#[derive(Component)]
struct Junk<const N: usize>(u32);
#[allow(dead_code)]
#[repr(align(32))]
#[derive(Component)]
struct JunkAligned<const N: usize>(u64);

fn lock(log: &DropLog) -> std::sync::MutexGuard<'_, Vec<u64>> {
    log.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- the reference model --------------------------------------------------------

/// What one live entity should hold. `tracked` is the serial of its `Tracked`.
#[derive(Debug, Default, Clone, PartialEq)]
struct Row {
    num: Option<u32>,
    tracked: Option<u64>,
    marker: bool,
    aligned: Option<u64>,
}

impl Row {
    fn component_count(&self) -> usize {
        usize::from(self.num.is_some())
            + usize::from(self.tracked.is_some())
            + usize::from(self.marker)
            + usize::from(self.aligned.is_some())
    }
}

#[derive(Default)]
struct Model {
    live: BTreeMap<Entity, Row>,
    /// Handles that were despawned (or cleared); they must stay dead.
    dead: Vec<Entity>,
    /// Serials of `Tracked` values the last operation should have dropped.
    expected_drops: Vec<u64>,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Num,
    Tracked,
    Marker,
    Aligned,
}

const KINDS: [Kind; 4] = [Kind::Num, Kind::Tracked, Kind::Marker, Kind::Aligned];

/// A world, its model, and the generator driving both.
struct Case {
    world: World,
    model: Model,
    rng: Rng,
    log: DropLog,
    next_serial: u64,
}

impl Case {
    fn new(seed: u64) -> Self {
        Case {
            world: World::new(),
            model: Model::default(),
            rng: Rng::new(seed),
            log: DropLog::default(),
            next_serial: 0,
        }
    }

    fn tracked(&mut self) -> Tracked {
        self.next_serial += 1;
        Tracked::new(self.next_serial, &self.log)
    }

    /// An entity to act on: usually live, sometimes despawned, rarely a handle
    /// that was never handed out.
    fn target(&mut self) -> Entity {
        let r = self.rng.below(10);
        if r < 7 && !self.model.live.is_empty() {
            let i = self.rng.below(self.model.live.len());
            *self.model.live.keys().nth(i).unwrap()
        } else if r < 9 && !self.model.dead.is_empty() {
            self.model.dead[self.rng.below(self.model.dead.len())]
        } else {
            Entity::from_bits(u64::from(1_000_000 + self.rng.below(1000) as u32))
        }
    }

    fn spawn(&mut self) -> String {
        let n = self.rng.next_u64() as u32;
        let a = self.rng.next_u64();
        let (e, row) = match self.rng.below(6) {
            0 => {
                // `spawn` needs at least one component; strip it to reach the
                // empty archetype.
                let e = self.world.spawn((Marker,));
                assert!(self.world.remove::<Marker>(e).is_some());
                (e, Row::default())
            }
            1 => (
                self.world.spawn((Num(n),)),
                Row {
                    num: Some(n),
                    ..Row::default()
                },
            ),
            2 => {
                let t = self.tracked();
                let s = t.serial;
                (
                    self.world.spawn((t,)),
                    Row {
                        tracked: Some(s),
                        ..Row::default()
                    },
                )
            }
            3 => {
                let t = self.tracked();
                let s = t.serial;
                (
                    self.world.spawn((Num(n), t)),
                    Row {
                        num: Some(n),
                        tracked: Some(s),
                        ..Row::default()
                    },
                )
            }
            4 => (
                self.world.spawn((Marker, Aligned(a))),
                Row {
                    marker: true,
                    aligned: Some(a),
                    ..Row::default()
                },
            ),
            _ => {
                // Same components as other shapes, in a different order.
                let t = self.tracked();
                let s = t.serial;
                (
                    self.world.spawn((Aligned(a), Num(n), Marker, t)),
                    Row {
                        num: Some(n),
                        tracked: Some(s),
                        marker: true,
                        aligned: Some(a),
                    },
                )
            }
        };
        let op = format!("spawn {e:?} {row:?}");
        assert!(
            self.model.live.insert(e, row).is_none() && !self.model.dead.contains(&e),
            "{op}: handle was handed out before"
        );
        op
    }

    fn despawn(&mut self) -> Result<String, String> {
        let e = self.target();
        let op = format!("despawn {e:?}");
        let expected = self.model.live.remove(&e);
        if let Some(row) = &expected {
            self.model.expected_drops.extend(row.tracked);
            self.model.dead.push(e);
        }
        let got = self.world.despawn(e);
        if got != expected.is_some() {
            return Err(format!("{op}: returned {got}, model expected {}", expected.is_some()));
        }
        Ok(op)
    }

    fn insert(&mut self) -> Result<String, String> {
        let e = self.target();
        let kind = KINDS[self.rng.below(KINDS.len())];
        let v = self.rng.next_u64();
        let op = format!("insert {kind:?} on {e:?}");
        let live = self.model.live.contains_key(&e);
        let got = match kind {
            Kind::Num => self.world.insert(e, Num(v as u32)),
            Kind::Tracked => {
                let t = self.tracked();
                let s = t.serial;
                if !live {
                    self.model.expected_drops.push(s); // dropped by the world
                }
                let got = self.world.insert(e, t);
                if let Some(row) = self.model.live.get_mut(&e) {
                    // Replacing drops the old value.
                    self.model.expected_drops.extend(row.tracked.replace(s));
                }
                got
            }
            Kind::Marker => self.world.insert(e, Marker),
            Kind::Aligned => self.world.insert(e, Aligned(v)),
        };
        if let Some(row) = self.model.live.get_mut(&e) {
            match kind {
                Kind::Num => row.num = Some(v as u32),
                Kind::Tracked => {}
                Kind::Marker => row.marker = true,
                Kind::Aligned => row.aligned = Some(v),
            }
        }
        if got != live {
            return Err(format!("{op}: returned {got}, model expected {live}"));
        }
        Ok(op)
    }

    fn remove(&mut self) -> Result<String, String> {
        let e = self.target();
        let kind = KINDS[self.rng.below(KINDS.len())];
        let op = format!("remove {kind:?} from {e:?}");
        let mut row = self.model.live.get(&e).cloned().unwrap_or_default();
        let ok = match kind {
            Kind::Num => self.world.remove::<Num>(e).map(|n| n.0) == row.num.take(),
            Kind::Tracked => {
                let got = self.world.remove::<Tracked>(e);
                let ok =
                    got.as_ref().is_none_or(Tracked::intact) && got.as_ref().map(|t| t.serial) == row.tracked.take();
                if let Some(t) = got {
                    // The caller owns the value now; dropping it here must be
                    // its one and only drop.
                    self.model.expected_drops.push(t.serial);
                }
                ok
            }
            Kind::Marker => self.world.remove::<Marker>(e).is_some() == std::mem::take(&mut row.marker),
            Kind::Aligned => self.world.remove::<Aligned>(e).map(|a| a.0) == row.aligned.take(),
        };
        if let Some(r) = self.model.live.get_mut(&e) {
            *r = row;
        }
        if !ok {
            return Err(format!("{op}: returned value differs from the model"));
        }
        Ok(op)
    }

    /// Mutates through `get_mut` and through a mutable query.
    fn mutate(&mut self) -> String {
        if self.rng.one_in(2) {
            let e = self.target();
            if let Some(n) = self.world.get_mut::<Num>(e) {
                n.0 = n.0.wrapping_add(1);
            }
            if let Some(n) = self.model.live.get_mut(&e).and_then(|r| r.num.as_mut()) {
                *n = n.wrapping_add(1);
            }
            format!("get_mut Num on {e:?}")
        } else {
            self.world.query::<(&mut Num, &mut Aligned)>().for_each(|(n, a)| {
                n.0 = n.0.wrapping_mul(3);
                a.0 = a.0.rotate_left(1);
            });
            for row in self.model.live.values_mut() {
                if let (Some(n), Some(a)) = (row.num.as_mut(), row.aligned.as_mut()) {
                    *n = n.wrapping_mul(3);
                    *a = a.rotate_left(1);
                }
            }
            "query (&mut Num, &mut Aligned)".to_string()
        }
    }

    fn clear(&mut self) -> String {
        self.world.clear();
        for (e, row) in std::mem::take(&mut self.model.live) {
            self.model.expected_drops.extend(row.tracked);
            self.model.dead.push(e);
        }
        "clear".to_string()
    }

    /// Runs one random operation and returns its description.
    fn step(&mut self) -> Result<String, String> {
        match self.rng.below(100) {
            0..=29 => Ok(self.spawn()),
            30..=44 => self.despawn(),
            45..=69 => self.insert(),
            70..=91 => self.remove(),
            92..=98 => Ok(self.mutate()),
            _ => Ok(self.clear()),
        }
    }
}

/// Spawns and despawns entities with random sets of `Junk` components (and
/// some of the model's own kinds), so that the world passes through more
/// archetypes than it keeps empty ones. Leaves the model as it was.
fn churn(world: &mut World, rng: &mut Rng) {
    macro_rules! with {
        ($mask:expr, $e:expr, $($n:literal)*) => {
            $(if $mask >> $n & 1 != 0 {
                world.insert($e, Junk::<$n>($n));
                world.insert($e, JunkAligned::<$n>($n));
            })*
        };
    }
    let mut junk = Vec::new();
    for _ in 0..6 {
        let mask = rng.next_u64();
        let e = world.spawn((Junk::<0>(0),));
        with!(mask, e, 1 2 3);
        if mask & 1 << 8 != 0 {
            world.insert(e, Num(0));
        }
        if mask & 1 << 9 != 0 {
            world.insert(e, Aligned(0));
        }
        if mask & 1 << 10 != 0 {
            world.insert(e, Marker);
        }
        if mask & 1 << 11 != 0 {
            world.remove::<Junk<0>>(e);
        }
        junk.push(e);
    }
    for e in junk {
        assert!(world.despawn(e));
    }
}

// ---- comparing world and model ---------------------------------------------------

fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v
}

/// Compares everything observable about `world` against `model`, and checks
/// that the values dropped since the last check are exactly the expected ones.
fn check(world: &mut World, model: &mut Model, log: &DropLog) -> Result<(), String> {
    let dropped = sorted(std::mem::take(&mut *lock(log)));
    let expected = sorted(std::mem::take(&mut model.expected_drops));
    if dropped != expected {
        return Err(format!("dropped Tracked {dropped:?}, model expected {expected:?}"));
    }

    if world.len() != model.live.len() {
        return Err(format!(
            "world has {} entities, model {}",
            world.len(),
            model.live.len()
        ));
    }
    for (&e, row) in &model.live {
        if !world.is_alive(e) {
            return Err(format!("{e:?} should be alive"));
        }
        let num = world.get::<Num>(e).map(|n| n.0);
        let tracked = world.get::<Tracked>(e);
        if tracked.is_some_and(|t| !t.intact()) {
            return Err(format!("{e:?}: Tracked value is corrupted"));
        }
        let aligned = world.get::<Aligned>(e);
        if aligned.is_some_and(|a| !(a as *const Aligned).addr().is_multiple_of(align_of::<Aligned>())) {
            return Err(format!("{e:?}: Aligned is misaligned"));
        }
        let got = Row {
            num,
            tracked: tracked.map(|t| t.serial),
            marker: world.has::<Marker>(e),
            aligned: aligned.map(|a| a.0),
        };
        if &got != row {
            return Err(format!("{e:?}: world has {got:?}, model {row:?}"));
        }
        if world.has::<Num>(e) != row.num.is_some() || world.has::<Tracked>(e) != row.tracked.is_some() {
            return Err(format!("{e:?}: has::<T> disagrees with get::<T>"));
        }
        let count = world.components_of(e).map_or(usize::MAX, <[_]>::len);
        if count != row.component_count() {
            return Err(format!("{e:?}: {count} components, model {}", row.component_count()));
        }
    }
    for &e in &model.dead {
        if world.is_alive(e) || world.get::<Num>(e).is_some() || world.components_of(e).is_some() {
            return Err(format!("despawned {e:?} is visible"));
        }
    }

    // Queries see the same entities and values as single-entity access.
    let mut nums = Vec::new();
    world.query::<(Entity, &Num)>().for_each(|(e, n)| nums.push((e, n.0)));
    let want: Vec<_> = model.live.iter().filter_map(|(&e, r)| Some((e, r.num?))).collect();
    if sorted(nums) != want {
        return Err("query (Entity, &Num) differs from the model".into());
    }

    let marked: Vec<_> = world
        .query::<(Entity, Option<&Tracked>)>()
        .filter::<With<Marker>>()
        .iter()
        .map(|(e, t)| (e, t.map(|t| t.serial)))
        .collect();
    let want: Vec<_> = model
        .live
        .iter()
        .filter(|(_, r)| r.marker)
        .map(|(&e, r)| (e, r.tracked))
        .collect();
    if sorted(marked) != want {
        return Err("query (Entity, Option<&Tracked>) with Marker differs from the model".into());
    }

    let without = world.query::<Entity>().filter::<Without<Num>>().count();
    let want = model.live.values().filter(|r| r.num.is_none()).count();
    if without != want {
        return Err(format!("query Entity without Num counts {without}, model {want}"));
    }

    for (&e, row) in &model.live {
        let got = world
            .query::<(&Aligned, &Tracked)>()
            .get(e)
            .map(|(a, t)| (a.0, t.serial));
        let want = row.aligned.zip(row.tracked);
        if got != want {
            return Err(format!(
                "{e:?}: query get (&Aligned, &Tracked) is {got:?}, model {want:?}"
            ));
        }
    }
    Ok(())
}

// ---- running seeds -------------------------------------------------------------

/// Runs `ops` random operations from `seed`, checking after each one. If
/// `corrupt_at` is set, the model is deliberately broken before that check,
/// to prove the checker notices.
fn run(seed: u64, ops: usize, corrupt_at: Option<usize>) -> Result<(), String> {
    run_churning(seed, ops, corrupt_at, false)
}

/// [`run`], with [`churn`] before every operation if `churning`.
fn run_churning(seed: u64, ops: usize, corrupt_at: Option<usize>, churning: bool) -> Result<(), String> {
    let mut case = Case::new(seed);
    let mut recent: Vec<String> = Vec::new();
    let fail = |i: usize, recent: &[String], msg: String| {
        let trail = recent.iter().rev().take(12).rev().cloned().collect::<Vec<_>>();
        Err(format!(
            "after op {i}: {msg}\nlast operations:\n  {}",
            trail.join("\n  ")
        ))
    };
    for i in 0..ops {
        if churning {
            churn(&mut case.world, &mut case.rng);
        }
        let step = catch_unwind(AssertUnwindSafe(|| case.step()));
        match step {
            Ok(Ok(op)) => recent.push(op),
            Ok(Err(msg)) => return fail(i, &recent, msg),
            Err(panic) => return fail(i, &recent, format!("panicked: {}", panic_message(&*panic))),
        }
        if corrupt_at == Some(i) {
            case.model.expected_drops.push(u64::MAX);
        }
        if let Err(msg) = check(&mut case.world, &mut case.model, &case.log) {
            return fail(i, &recent, msg);
        }
    }
    // Dropping the world drops every remaining value exactly once.
    for row in case.model.live.values() {
        case.model.expected_drops.extend(row.tracked);
    }
    case.model.live.clear();
    drop(case.world);
    let dropped = sorted(std::mem::take(&mut *lock(&case.log)));
    let expected = sorted(case.model.expected_drops);
    if dropped != expected {
        return Err(format!(
            "dropping the world dropped {dropped:?}, model expected {expected:?}"
        ));
    }
    Ok(())
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic>".into())
}

fn env_number(name: &str) -> Option<u64> {
    let v = std::env::var(name).ok()?;
    let v = v.trim();
    let parsed = match v.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => v.parse(),
    };
    Some(parsed.unwrap_or_else(|_| panic!("{name}={v} is not a number")))
}

fn seed_failure(seed: u64, msg: &str) -> String {
    format!(
        "seed {seed:#x} failed {msg}\n\
         reproduce: PONKO_ECS_SEED={seed:#x} cargo test -p ponko_ecs --test differential"
    )
}

thread_local! {
    /// The seed this thread is running, for the panic hook.
    static RUNNING_SEED: Cell<Option<u64>> = const { Cell::new(None) };
}

/// A bug in unsafe code can panic where unwinding is impossible and abort the
/// process before `run` returns an error. Report the seed from the panic hook
/// too, straight to stderr, which the test harness doesn't capture.
fn report_seed_on_panic() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(seed) = RUNNING_SEED.get() {
                let report = seed_failure(seed, "with a panic (the process may abort)");
                let _ = writeln!(std::io::stderr(), "{report}");
            }
            previous(info);
        }));
    });
}

#[test]
fn world_matches_reference_model() {
    report_seed_on_panic();
    let (cases, ops) = if cfg!(miri) { (3, 60) } else { (200, 300) };
    let ops = env_number("PONKO_ECS_OPS").map_or(ops, |n| n as usize);
    let seeds: Vec<u64> = match env_number("PONKO_ECS_SEED") {
        Some(seed) => vec![seed],
        None => (0..env_number("PONKO_ECS_CASES").unwrap_or(cases)).collect(),
    };
    for seed in seeds {
        RUNNING_SEED.set(Some(seed));
        let result = run(seed, ops, None);
        RUNNING_SEED.set(None);
        if let Err(msg) = result {
            panic!("{}", seed_failure(seed, &msg));
        }
    }
}

/// The same model, while the world reclaims empty archetypes (the model's
/// among them) and builds new ones in their slots with their buffers.
#[test]
fn world_matches_reference_model_while_archetypes_are_reclaimed() {
    report_seed_on_panic();
    let (cases, ops) = if cfg!(miri) { (1, 40) } else { (40, 300) };
    for seed in 0..cases {
        let seed = 0x347_0000 + seed;
        RUNNING_SEED.set(Some(seed));
        let result = run_churning(seed, ops, None, true);
        RUNNING_SEED.set(None);
        if let Err(msg) = result {
            panic!("{}", seed_failure(seed, &msg));
        }
    }
    // The churn really reclaims: far fewer slots than component sets.
    let mut world = World::new();
    let mut rng = Rng::new(7);
    for _ in 0..if cfg!(miri) { 20 } else { 200 } {
        churn(&mut world, &mut rng);
    }
    let slots = world.raw_ref().archetypes().len();
    assert!(slots <= 2 * 64 + 16, "{slots} archetype slots");
}

// ---- the checker catches inconsistencies ------------------------------------

/// A small world and matching model, built by hand.
fn known_case() -> (Case, Entity, Entity) {
    let mut case = Case::new(0);
    let t = case.tracked();
    let a = case.world.spawn((Num(1), t, Marker));
    let b = case.world.spawn((Aligned(7),));
    case.model.live.insert(
        a,
        Row {
            num: Some(1),
            tracked: Some(1),
            marker: true,
            aligned: None,
        },
    );
    case.model.live.insert(
        b,
        Row {
            aligned: Some(7),
            ..Row::default()
        },
    );
    (case, a, b)
}

fn check_case(case: &mut Case) -> Result<(), String> {
    check(&mut case.world, &mut case.model, &case.log)
}

#[test]
fn checker_accepts_a_matching_model() {
    let (mut case, a, _) = known_case();
    check_case(&mut case).unwrap();
    case.world.despawn(a);
    case.model.live.remove(&a);
    case.model.dead.push(a);
    case.model.expected_drops.push(1);
    check_case(&mut case).unwrap();
}

#[test]
fn checker_detects_a_wrong_value() {
    let (mut case, a, _) = known_case();
    case.model.live.get_mut(&a).unwrap().num = Some(2);
    let err = check_case(&mut case).unwrap_err();
    assert!(err.contains("world has"), "{err}");
}

#[test]
fn checker_detects_a_missing_component() {
    let (mut case, _, b) = known_case();
    case.model.live.get_mut(&b).unwrap().marker = true;
    let err = check_case(&mut case).unwrap_err();
    assert!(err.contains("world has"), "{err}");
}

#[test]
fn checker_detects_an_untracked_entity() {
    let (mut case, _, b) = known_case();
    case.model.live.remove(&b);
    let err = check_case(&mut case).unwrap_err();
    assert!(err.contains("world has 2 entities, model 1"), "{err}");
}

#[test]
fn checker_detects_a_live_handle_the_model_thinks_is_dead() {
    let (mut case, _, b) = known_case();
    case.model.dead.push(b);
    let err = check_case(&mut case).unwrap_err();
    assert!(err.contains("is visible"), "{err}");
}

#[test]
fn checker_detects_a_missing_drop() {
    let (mut case, _, _) = known_case();
    case.model.expected_drops.push(1);
    let err = check_case(&mut case).unwrap_err();
    assert!(err.contains("dropped Tracked [], model expected [1]"), "{err}");
}

#[test]
fn checker_detects_an_unexpected_drop() {
    let (mut case, a, _) = known_case();
    drop(case.world.remove::<Tracked>(a));
    case.model.live.get_mut(&a).unwrap().tracked = None;
    let err = check_case(&mut case).unwrap_err();
    assert!(err.contains("dropped Tracked [1], model expected []"), "{err}");
}

#[test]
fn a_failing_seed_is_reported_and_reproducible() {
    let ops = if cfg!(miri) { 20 } else { 100 };
    let first = run(42, ops, Some(ops / 2)).unwrap_err();
    let again = run(42, ops, Some(ops / 2)).unwrap_err();
    assert_eq!(first, again, "the same seed must fail the same way");
    assert!(first.starts_with(&format!("after op {}:", ops / 2)), "{first}");
    assert!(first.contains("last operations:"), "{first}");
    let report = seed_failure(42, &first);
    assert!(report.contains("PONKO_ECS_SEED=0x2a"), "{report}");
    run(42, ops, None).unwrap();
}
