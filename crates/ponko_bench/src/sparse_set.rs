//! Sparse, frequently toggled components: archetypes against a sparse-set
//! prototype (issue #166). Group name: `sparse-set/`; run it with
//! `cargo run --release -p ponko_bench -- sparse-set/`. The numbers and the
//! recommendation are in `SPARSE_SET.md`.
//!
//! The workload: [`ENTITIES`] entities that all have `Pos` and `Vel`, and
//! [`SPARSE`] independent optional components (8 zero-sized tags such as
//! "stunned" or "selected", 8 small effects with data such as "burning for
//! n ticks"). Each entity has each optional component with probability
//! `density` (1, 5 or 20 %), independently, so the combinations, and with
//! them the archetypes, multiply. Every tick [`CHURN_PCT`] % of each
//! component's holders lose it and as many other entities gain it: the
//! density stays the same, the combinations keep changing. Worlds are
//! measured after [`SETTLE`] ticks of that (one second at 60 Hz).
//!
//! Three storages do the same work on the same sequence of changes:
//!
//! - `arch`: everything in `ponko_ecs` archetypes, with the default query
//!   index policy (on: these worlds have hundreds to thousands of
//!   archetypes); `arch-scan` is the same with the index off, for scale.
//! - `sset`: `Pos`/`Vel` in archetypes (one archetype), the 16 optional
//!   components in the sparse sets of [`proto`]; joins are driven from the
//!   sparse set (`sset`) or from the archetype rows (`sset-rows`).
//! - `flags`: no engine change at all: the 16 optional components are bits
//!   and slots of one dense `Status` component, tested row by row.
//!
//! Operations, each timed on its own:
//!
//! - `churn`: one tick of adds and removes.
//! - `dense`: `pos += vel * dt` over every entity.
//! - `data`: for each of the 8 effects, `pos.x += effect` over its holders.
//! - `and`: for 8 pairs of tags, read `Pos` of entities that have both.
//! - `without`: for 4 tags, write `Pos` of every entity without it.
//! - `frame`: all of the above once, as a game tick would.
//!
//! `sparse-set/dense-data/*` is the opposite case: every entity has `Pos` and
//! two effects, so archetypes store everything in one table.
//!
//! `sparse-set/mem` prints archetype counts and storage bytes over 600
//! ticks; `sparse-set/rss/{density}/{arch,sset,flags}` (one per process, alone)
//! prints the process's peak resident memory.

mod proto;

use crate::{Bench, DT, Pos, Vel};
use ponko_ecs::{Component, Entity, QueryCachePolicy, With, Without, World};
use proto::Hybrid;

pub const ENTITIES: usize = 100_000;
/// Optional components: 8 tags, then 8 effects.
pub const SPARSE: usize = 16;
const TAGS: usize = 8;
pub const CHURN_PCT: usize = 5;
pub const SETTLE: usize = 60;
const DENSITIES: [u32; 3] = [1, 5, 20];
const RUNS: usize = 30;
/// Ticks precomputed for the timed churn and frame runs (5 warm-up runs).
const TIMED_TICKS: usize = RUNS + 5;

/// Zero-sized marker `K`.
#[derive(Clone, Copy, Default, ponko_ecs::Component)]
struct Tag<const K: usize>;

/// What an effect carries: an amount per tick and the ticks left.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Effect {
    amount: f32,
    ticks: u32,
}

/// Effect `K`.
#[derive(Clone, Copy, ponko_ecs::Component)]
struct Data<const K: usize>(Effect);

/// An optional component of the workload.
trait Sparse: Component + Copy {
    fn make(entity: u32) -> Self;
}
impl<const K: usize> Sparse for Tag<K> {
    fn make(_: u32) -> Self {
        Tag
    }
}
impl<const K: usize> Sparse for Data<K> {
    fn make(entity: u32) -> Self {
        Data(effect(K, entity))
    }
}

/// Effect `k`'s value for an entity.
fn effect(k: usize, entity: u32) -> Effect {
    Effect {
        amount: ((entity % 7) as f32 + k as f32) * 0.25,
        ticks: entity % 100,
    }
}

/// Runs `$body` with `$t` naming tag `$k` (0..8).
macro_rules! with_tag {
    ($k:expr, |$t:ident| $body:expr) => {
        with_tag!(@arms $k, $t, $body, 0 1 2 3 4 5 6 7)
    };
    (@arms $k:expr, $t:ident, $body:expr, $($n:literal)*) => {
        match $k {
            $($n => {
                #[allow(dead_code)]
                type $t = Tag<$n>;
                $body
            })*
            _ => unreachable!(),
        }
    };
}

/// Runs `$body` with `$t` naming effect `$k` (0..8).
macro_rules! with_data {
    ($k:expr, |$t:ident| $body:expr) => {
        with_data!(@arms $k, $t, $body, 0 1 2 3 4 5 6 7)
    };
    (@arms $k:expr, $t:ident, $body:expr, $($n:literal)*) => {
        match $k {
            $($n => {
                type $t = Data<$n>;
                $body
            })*
            _ => unreachable!(),
        }
    };
}

/// Runs `$body` with `$t` naming optional component `$c` (0..16).
macro_rules! with_sparse {
    ($c:expr, |$t:ident| $body:expr) => {{
        let c: usize = $c;
        if c < TAGS {
            with_tag!(c, |$t| $body)
        } else {
            with_data!(c - TAGS, |$t| $body)
        }
    }};
}

/// One structural change: entity `entity` gains (`add`) or loses component `comp`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Op {
    entity: u32,
    comp: u8,
    add: bool,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Who has which optional component, and the changes of each tick: a
/// deterministic simulation both storages replay.
struct Churn {
    n: usize,
    /// `has[c * n + i]`: entity `i` has component `c`.
    has: Vec<bool>,
    holders: Vec<Vec<u32>>,
    rng: Rng,
}

impl Churn {
    fn new(n: usize, density_pct: u32) -> Self {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ u64::from(density_pct));
        let mut has = vec![false; SPARSE * n];
        let mut holders = vec![Vec::new(); SPARSE];
        for i in 0..n {
            for c in 0..SPARSE {
                if rng.below(100) < density_pct as usize {
                    has[c * n + i] = true;
                    holders[c].push(i as u32);
                }
            }
        }
        Churn { n, has, holders, rng }
    }

    /// The next tick's changes: per component, [`CHURN_PCT`] % of its
    /// holders lose it, then as many entities without it gain it.
    fn tick(&mut self, out: &mut Vec<Op>) {
        out.clear();
        for c in 0..SPARSE {
            let moves = (self.holders[c].len() * CHURN_PCT).div_ceil(100);
            for _ in 0..moves {
                let at = self.rng.below(self.holders[c].len());
                let i = self.holders[c].swap_remove(at);
                self.has[c * self.n + i as usize] = false;
                out.push(Op {
                    entity: i,
                    comp: c as u8,
                    add: false,
                });
            }
            for _ in 0..moves {
                let i = loop {
                    let i = self.rng.below(self.n);
                    if !self.has[c * self.n + i] {
                        break i;
                    }
                };
                self.has[c * self.n + i] = true;
                self.holders[c].push(i as u32);
                out.push(Op {
                    entity: i as u32,
                    comp: c as u8,
                    add: true,
                });
            }
        }
    }
}

/// The two storages behind one interface, so every operation is written
/// once per storage and both see the same entities and changes.
trait Store {
    fn spawn(&mut self, i: u32) -> Entity;
    fn apply(&mut self, e: Entity, op: Op);
    fn dense(&mut self);
    fn data(&mut self);
    /// Returns an order-independent checksum of the `Pos` values read.
    fn and(&mut self) -> u64;
    fn without(&mut self);
    fn archetypes(&self) -> (usize, usize);
    fn bytes(&self) -> usize;
}

fn start_pos(i: u32) -> (Pos, Vel) {
    (Pos { x: i as f32, y: 0.0 }, Vel { x: 1.0, y: 0.5 })
}

/// The `and` pairs: tags `k` and `k + 1`.
fn pair(k: usize) -> (usize, usize) {
    (k, (k + 1) % TAGS)
}

/// Bits of a `Pos`, summed so that the visiting order doesn't matter.
fn sum_bits(p: &Pos) -> u64 {
    u64::from(p.x.to_bits()) ^ (u64::from(p.y.to_bits()) << 32)
}

/// Archetype counts: all of them, and those holding entities.
fn archetypes_of(w: &World) -> (usize, usize) {
    let all = w.raw_ref().archetypes();
    (all.len(), all.iter().filter(|a| !a.is_empty()).count())
}

/// Bytes the world's archetypes take beyond their columns: the 128-byte
/// struct, plus a 4-byte id and a 32-byte column per component. A lower
/// bound: the transition-edge maps aren't public and aren't counted.
fn archetype_overhead(w: &World) -> usize {
    w.raw_ref()
        .archetypes()
        .iter()
        .map(|a| 128 + a.component_ids().len() * (4 + 32))
        .sum()
}

impl Store for World {
    fn spawn(&mut self, i: u32) -> Entity {
        World::spawn(self, start_pos(i))
    }
    fn apply(&mut self, e: Entity, op: Op) {
        with_sparse!(op.comp as usize, |T| if op.add {
            self.insert(e, T::make(op.entity));
        } else {
            self.remove::<T>(e);
        })
    }
    fn dense(&mut self) {
        self.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
            p.x += v.x * DT;
            p.y += v.y * DT;
        });
    }
    fn data(&mut self) {
        for k in 0..TAGS {
            with_data!(k, |T| self
                .query::<(&mut Pos, &T)>()
                .for_each(|(p, d)| p.x += d.0.amount));
        }
    }
    fn and(&mut self) -> u64 {
        let mut sum = 0u64;
        for k in 0..TAGS {
            let (a, b) = pair(k);
            with_tag!(a, |A| with_tag!(b, |B| self
                .query::<&Pos>()
                .filter::<With<A>>()
                .filter::<With<B>>()
                .for_each(|p| sum = sum.wrapping_add(sum_bits(p)))));
        }
        sum
    }
    fn without(&mut self) {
        for k in 0..4 {
            with_tag!(k, |T| self
                .query::<&mut Pos>()
                .filter::<Without<T>>()
                .for_each(|p| p.y += 0.25));
        }
    }
    fn archetypes(&self) -> (usize, usize) {
        archetypes_of(self)
    }
    fn bytes(&self) -> usize {
        self.allocated_bytes() + archetype_overhead(self)
    }
}

/// The hybrid, with joins driven from the sparse set (`rows: false`) or
/// from the archetype rows.
struct Sset {
    h: Hybrid,
    rows: bool,
}

impl Store for Sset {
    fn spawn(&mut self, i: u32) -> Entity {
        self.h.world.spawn(start_pos(i))
    }
    fn apply(&mut self, e: Entity, op: Op) {
        with_sparse!(op.comp as usize, |T| if op.add {
            self.h.insert(e, T::make(op.entity));
        } else {
            self.h.remove::<T>(e);
        })
    }
    fn dense(&mut self) {
        Store::dense(&mut self.h.world);
    }
    fn data(&mut self) {
        for k in 0..TAGS {
            with_data!(k, |T| if self.rows {
                self.h.join_from_rows::<&mut Pos, T>(|p, d| p.x += d.0.amount);
            } else {
                self.h.join_from_set::<Pos, T>(|p, d| p.x += d.0.amount);
            });
        }
    }
    fn and(&mut self) -> u64 {
        // Driven from the smaller set; the other answers `contains`.
        let mut sum = 0u64;
        let pos = proto::DenseId::<Pos>::of(&self.h.world).expect("spawned with `Pos`");
        for k in 0..TAGS {
            let (a, b) = pair(k);
            with_tag!(a, |A| with_tag!(b, |B| {
                let (Some(a), Some(b)) = (self.h.set::<A>(), self.h.set::<B>()) else {
                    continue;
                };
                let (small, big) = if a.len() <= b.len() {
                    (a.entities(), b as &dyn Contains)
                } else {
                    (b.entities(), a as &dyn Contains)
                };
                for &e in small {
                    // SAFETY: `pos` was taken from `self.h.world` above.
                    if big.has(e)
                        && let Some(p) = unsafe { proto::dense_ref(&self.h.world, pos, e) }
                    {
                        sum = sum.wrapping_add(sum_bits(p));
                    }
                }
            }));
        }
        sum
    }
    fn without(&mut self) {
        for k in 0..4 {
            with_tag!(k, |T| self.h.rows_without::<&mut Pos, T>(|p| p.y += 0.25));
        }
    }
    fn archetypes(&self) -> (usize, usize) {
        archetypes_of(&self.h.world)
    }
    fn bytes(&self) -> usize {
        self.h.allocated_bytes() + archetype_overhead(&self.h.world)
    }
}

/// The workaround a game can use today without engine changes: the optional
/// components as bits and slots of one dense component, so nothing ever
/// changes archetype and every query tests bits row by row.
#[derive(Clone, Copy, ponko_ecs::Component)]
struct Status {
    /// Bit `c`: has optional component `c`.
    bits: u16,
    /// The effects' values, valid where their bit is set.
    effects: [Effect; TAGS],
}

struct Flags(World);

impl Flags {
    fn has(s: &Status, c: usize) -> bool {
        s.bits >> c & 1 != 0
    }
}

impl Store for Flags {
    fn spawn(&mut self, i: u32) -> Entity {
        let none = Effect { amount: 0.0, ticks: 0 };
        let (p, v) = start_pos(i);
        self.0.spawn((
            p,
            v,
            Status {
                bits: 0,
                effects: [none; TAGS],
            },
        ))
    }
    fn apply(&mut self, e: Entity, op: Op) {
        let s = self.0.get_mut::<Status>(e).expect("spawned with `Status`");
        let c = op.comp as usize;
        if op.add {
            s.bits |= 1 << c;
            if c >= TAGS {
                s.effects[c - TAGS] = effect(c - TAGS, op.entity);
            }
        } else {
            s.bits &= !(1 << c);
        }
    }
    fn dense(&mut self) {
        Store::dense(&mut self.0);
    }
    fn data(&mut self) {
        for k in 0..TAGS {
            self.0.query::<(&mut Pos, &Status)>().for_each(|(p, s)| {
                if Flags::has(s, TAGS + k) {
                    p.x += s.effects[k].amount;
                }
            });
        }
    }
    fn and(&mut self) -> u64 {
        let mut sum = 0u64;
        for k in 0..TAGS {
            let (a, b) = pair(k);
            self.0.query::<(&Pos, &Status)>().for_each(|(p, s)| {
                if Flags::has(s, a) && Flags::has(s, b) {
                    sum = sum.wrapping_add(sum_bits(p));
                }
            });
        }
        sum
    }
    fn without(&mut self) {
        for k in 0..4 {
            self.0.query::<(&mut Pos, &Status)>().for_each(|(p, s)| {
                if !Flags::has(s, k) {
                    p.y += 0.25;
                }
            });
        }
    }
    fn archetypes(&self) -> (usize, usize) {
        archetypes_of(&self.0)
    }
    fn bytes(&self) -> usize {
        self.0.allocated_bytes() + archetype_overhead(&self.0)
    }
}

/// Membership test of a sparse set whatever its value type.
trait Contains {
    fn has(&self, e: Entity) -> bool;
}
impl<T> Contains for proto::SparseSet<T> {
    #[inline]
    fn has(&self, e: Entity) -> bool {
        self.contains(e)
    }
}

/// A world of either storage, the change simulation behind it, and the
/// changes of the timed ticks.
struct Workload<S> {
    store: S,
    entities: Vec<Entity>,
    churn: Churn,
    ticks: Vec<Vec<Op>>,
    next: usize,
}

impl<S: Store> Workload<S> {
    /// Spawns `n` entities, gives them their optional components, runs
    /// `settle` ticks and precomputes `timed` more.
    fn new(mut store: S, n: usize, density_pct: u32, settle: usize, timed: usize) -> Self {
        let churn = Churn::new(n, density_pct);
        let entities: Vec<Entity> = (0..n as u32).map(|i| store.spawn(i)).collect();
        for (c, holders) in churn.holders.iter().enumerate() {
            for &i in holders {
                let op = Op {
                    entity: i,
                    comp: c as u8,
                    add: true,
                };
                store.apply(entities[i as usize], op);
            }
        }
        let mut w = Workload {
            store,
            entities,
            churn,
            ticks: Vec::new(),
            next: 0,
        };
        let mut ops = Vec::new();
        for _ in 0..settle {
            w.churn.tick(&mut ops);
            w.replay(&ops);
        }
        w.ticks = (0..timed)
            .map(|_| {
                w.churn.tick(&mut ops);
                ops.clone()
            })
            .collect();
        w
    }

    fn replay(&mut self, ops: &[Op]) {
        for &op in ops {
            self.store.apply(self.entities[op.entity as usize], op);
        }
    }

    /// Applies the next precomputed tick.
    fn churn_tick(&mut self) {
        let ops = std::mem::take(&mut self.ticks[self.next]);
        self.replay(&ops);
        self.ticks[self.next] = ops;
        self.next += 1;
    }

    /// One game tick: the changes, then every query once.
    fn frame(&mut self) -> u64 {
        self.churn_tick();
        self.store.dense();
        self.store.data();
        let sum = self.store.and();
        self.store.without();
        sum
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Arch,
    ArchScan,
    Sset,
    SsetRows,
    Flags,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Arch => "arch",
            Kind::ArchScan => "arch-scan",
            Kind::Sset => "sset",
            Kind::SsetRows => "sset-rows",
            Kind::Flags => "flags",
        }
    }
}

/// Every storage as one type, so a benchmark is written once.
enum Either {
    Arch(Box<Workload<World>>),
    Sset(Box<Workload<Sset>>),
    Flags(Box<Workload<Flags>>),
}

macro_rules! either {
    ($e:expr, |$w:ident| $body:expr) => {
        match $e {
            Either::Arch($w) => $body,
            Either::Sset($w) => $body,
            Either::Flags($w) => $body,
        }
    };
}

fn build(kind: Kind, n: usize, density_pct: u32, settle: usize, timed: usize) -> Either {
    match kind {
        Kind::Arch | Kind::ArchScan => {
            let mut w = World::new();
            if kind == Kind::ArchScan {
                w.set_query_cache_policy(QueryCachePolicy::off());
            }
            Either::Arch(Box::new(Workload::new(w, n, density_pct, settle, timed)))
        }
        Kind::Sset | Kind::SsetRows => {
            let s = Sset {
                h: Hybrid::default(),
                rows: kind == Kind::SsetRows,
            };
            Either::Sset(Box::new(Workload::new(s, n, density_pct, settle, timed)))
        }
        Kind::Flags => Either::Flags(Box::new(Workload::new(
            Flags(World::new()),
            n,
            density_pct,
            settle,
            timed,
        ))),
    }
}

pub fn run(b: &mut Bench) {
    use Kind::*;
    type OpFn = fn(&mut Either);
    let groups: [(&str, &[Kind], OpFn); 6] = [
        ("churn", &[Arch, Sset, Flags], |w| either!(w, |w| w.churn_tick())),
        ("dense", &[Arch, ArchScan, Sset, Flags], |w| {
            either!(w, |w| w.store.dense())
        }),
        ("data", &[Arch, ArchScan, Sset, SsetRows, Flags], |w| {
            either!(w, |w| w.store.data())
        }),
        ("and", &[Arch, ArchScan, Sset, Flags], |w| {
            std::hint::black_box(either!(w, |w| w.store.and()));
        }),
        ("without", &[Arch, ArchScan, Sset, Flags], |w| {
            either!(w, |w| w.store.without())
        }),
        ("frame", &[Arch, ArchScan, Sset, SsetRows, Flags], |w| {
            std::hint::black_box(either!(w, |w| w.frame()));
        }),
    ];
    for density in DENSITIES {
        for (group, kinds, op) in groups {
            for &kind in kinds {
                let name = format!("sparse-set/{group}/{density}pct/{}", kind.name());
                // Only the groups that change the world use timed ticks.
                let timed = if matches!(group, "churn" | "frame") {
                    TIMED_TICKS
                } else {
                    0
                };
                b.steady_with_bytes(
                    &name,
                    RUNS,
                    || build(kind, ENTITIES, density, SETTLE, timed),
                    op,
                    |w| Some(either!(w, |w| w.store.bytes())),
                );
            }
        }
    }
    dense_data(b);
    memory(b);
    rss(b);
}

/// The dense case: every entity has `Pos` and two effects, all in one
/// archetype table.
fn dense_arch(n: usize) -> (World, Vec<Entity>) {
    let mut w = World::new();
    let es = (0..n as u32)
        .map(|i| w.spawn((Pos { x: i as f32, y: 0.0 }, Data::<0>::make(i), Data::<1>::make(i))))
        .collect();
    (w, es)
}

/// The dense case with the two effects in sparse sets.
fn dense_sset(n: usize) -> (Hybrid, Vec<Entity>) {
    let mut h = Hybrid::default();
    let es: Vec<Entity> = (0..n as u32)
        .map(|i| h.world.spawn((Pos { x: i as f32, y: 0.0 },)))
        .collect();
    for (i, &e) in es.iter().enumerate() {
        h.insert(e, Data::<0>::make(i as u32));
        h.insert(e, Data::<1>::make(i as u32));
    }
    (h, es)
}

/// Adds both effects to `Pos`.
fn dense_arch_iter(w: &mut World) {
    w.query::<(&mut Pos, &Data<0>, &Data<1>)>().for_each(|(p, a, c)| {
        p.x += a.0.amount;
        p.y += c.0.amount;
    })
}

/// The same, joined from the archetype rows (`rows`) or from the first set.
fn dense_sset_iter(h: &mut Hybrid, rows: bool) {
    // Out of the store while the world is borrowed (no allocation).
    let d0 = std::mem::take(h.set_mut::<Data<0>>());
    let d1 = std::mem::take(h.set_mut::<Data<1>>());
    let pos = proto::DenseId::<Pos>::of(&h.world).expect("spawned with `Pos`");
    if rows {
        h.world.query::<(Entity, &mut Pos)>().for_each(|(e, p)| {
            if let (Some(a), Some(c)) = (d0.get(e), d1.get(e)) {
                p.x += a.0.amount;
                p.y += c.0.amount;
            }
        });
    } else {
        for (e, a) in d0.iter() {
            // SAFETY: `pos` was taken from `h.world` above.
            if let (Some(c), Some(p)) = (d1.get(e), unsafe { proto::dense_mut(&mut h.world, pos, e) }) {
                p.x += a.0.amount;
                p.y += c.0.amount;
            }
        }
    }
    *h.set_mut::<Data<0>>() = d0;
    *h.set_mut::<Data<1>>() = d1;
}

/// Adds a tag to every entity, then takes it off again.
fn dense_arch_add_remove(w: &mut World, es: &[Entity]) {
    for &e in es {
        w.insert(e, Tag::<0>);
    }
    for &e in es {
        w.remove::<Tag<0>>(e);
    }
}

/// [`dense_arch_add_remove`] on the sparse sets.
fn dense_sset_add_remove(h: &mut Hybrid, es: &[Entity]) {
    for &e in es {
        h.insert(e, Tag::<0>);
    }
    for &e in es {
        h.remove::<Tag<0>>(e);
    }
}

fn dense_data(b: &mut Bench) {
    b.steady_with_bytes(
        "sparse-set/dense-data/iter/arch",
        RUNS,
        || dense_arch(ENTITIES),
        |(w, _)| dense_arch_iter(w),
        |(w, _)| Some(w.allocated_bytes() + archetype_overhead(w)),
    );
    for (name, rows) in [("sset", false), ("sset-rows", true)] {
        b.steady_with_bytes(
            &format!("sparse-set/dense-data/iter/{name}"),
            RUNS,
            || dense_sset(ENTITIES),
            |(h, _)| dense_sset_iter(h, rows),
            |(h, _)| Some(h.allocated_bytes() + archetype_overhead(&h.world)),
        );
    }
    b.steady(
        "sparse-set/dense-data/add-remove/arch",
        10,
        || dense_arch(ENTITIES),
        |(w, es)| dense_arch_add_remove(w, es),
    );
    b.steady(
        "sparse-set/dense-data/add-remove/sset",
        10,
        || dense_sset(ENTITIES),
        |(h, es)| dense_sset_add_remove(h, es),
    );
}

/// Archetype counts, storage bytes and heap allocations per tick (since the
/// previous checkpoint) as the changes go on: whether a world that keeps
/// visiting new component combinations levels off.
fn memory(b: &Bench) {
    if b.skips("sparse-set/mem") {
        return;
    }
    // The archetype world runs on until its rare remaining allocations
    // (archetypes growing past any size they had) are visible.
    const CHECKPOINTS: [usize; 4] = [0, 60, 300, 600];
    const ARCH_CHECKPOINTS: [usize; 8] = [0, 60, 300, 600, 1200, 2400, 4800, 9600];
    for density in DENSITIES {
        for kind in [Kind::Arch, Kind::Sset, Kind::Flags] {
            let mut w = build(kind, ENTITIES, density, 0, 0);
            let mut ops = Vec::new();
            let mut done = 0;
            let checkpoints: &[usize] = if kind == Kind::Arch {
                &ARCH_CHECKPOINTS
            } else {
                &CHECKPOINTS
            };
            for &at in checkpoints {
                either!(&mut w, |w| {
                    let before = crate::alloc::Counts::now();
                    for _ in done..at {
                        w.churn.tick(&mut ops);
                        w.replay(&ops);
                    }
                    let allocs = crate::alloc::Counts::now().since(before).count as f64 / (at - done).max(1) as f64;
                    let (all, live) = w.store.archetypes();
                    println!(
                        "sparse-set/mem/{density}pct/{:<4} tick {at:>4}: {all:>6} archetypes ({live:>6} not empty), {:>9.1} KiB, {} changes/tick, {allocs:.2} allocs/tick since the last",
                        kind.name(),
                        w.store.bytes() as f64 / 1024.0,
                        ops.len(),
                    );
                });
                done = at;
            }
        }
    }
}

/// Peak resident memory of one case, after 600 ticks. Only meaningful when
/// it is the only thing the process ran: the filter must name it exactly.
fn rss(b: &Bench) {
    let Some(filter) = b.filter.as_deref() else { return };
    for density in DENSITIES {
        for kind in [Kind::Arch, Kind::Sset, Kind::Flags] {
            let name = format!("sparse-set/rss/{density}pct/{}", kind.name());
            if filter != name {
                continue;
            }
            let mut w = build(kind, ENTITIES, density, 600, 0);
            let bytes = either!(&mut w, |w| w.store.bytes());
            println!(
                "{name}: peak RSS {}, storage {} KiB",
                peak_rss().map_or("unavailable".into(), |b| format!("{} KiB", b / 1024)),
                bytes / 1024
            );
        }
    }
}

#[cfg(unix)]
fn peak_rss() -> Option<usize> {
    // SAFETY: `getrusage` fills the struct it is given.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        (libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0).then_some(usage)?
    };
    // Bytes on macOS, KiB on Linux.
    let unit = if cfg!(target_os = "macos") { 1 } else { 1024 };
    Some(usage.ru_maxrss as usize * unit)
}

#[cfg(not(unix))]
fn peak_rss() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entity's `Pos` and optional components, in spawn order.
    fn state<S: Store>(
        w: &Workload<S>,
        has: impl Fn(&S, Entity, usize) -> bool,
        pos: impl Fn(&S, Entity) -> Pos,
    ) -> Vec<(u32, u32, u16)> {
        w.entities
            .iter()
            .map(|&e| {
                let p = pos(&w.store, e);
                let bits = (0..SPARSE).fold(0u16, |m, c| m | (u16::from(has(&w.store, e, c)) << c));
                (p.x.to_bits(), p.y.to_bits(), bits)
            })
            .collect()
    }

    fn arch_state(w: &Workload<World>) -> Vec<(u32, u32, u16)> {
        state(
            w,
            |s, e, c| with_sparse!(c, |T| s.has::<T>(e)),
            |s, e| *s.get::<Pos>(e).unwrap(),
        )
    }

    fn sset_state(w: &Workload<Sset>) -> Vec<(u32, u32, u16)> {
        state(
            w,
            |s, e, c| with_sparse!(c, |T| s.h.has::<T>(e)),
            |s, e| *s.h.world.get::<Pos>(e).unwrap(),
        )
    }

    fn flags_state(w: &Workload<Flags>) -> Vec<(u32, u32, u16)> {
        state(
            w,
            |s, e, c| Flags::has(s.0.get::<Status>(e).unwrap(), c),
            |s, e| *s.0.get::<Pos>(e).unwrap(),
        )
    }

    #[test]
    fn churn_keeps_every_components_density() {
        let mut churn = Churn::new(4000, 5);
        let before: Vec<usize> = churn.holders.iter().map(Vec::len).collect();
        let mut ops = Vec::new();
        for _ in 0..20 {
            churn.tick(&mut ops);
            assert!(!ops.is_empty());
        }
        let after: Vec<usize> = churn.holders.iter().map(Vec::len).collect();
        assert_eq!(before, after);
        assert!(
            before.iter().all(|&n| n > 100 && n < 300),
            "about 5 % of 4000: {before:?}"
        );
    }

    #[test]
    fn both_storages_compute_the_same_frames() {
        for density in [5, 20] {
            for rows in [false, true] {
                let mut arch = Workload::new(World::new(), 3000, density, 10, 4);
                let sset = Sset {
                    h: Hybrid::default(),
                    rows,
                };
                let mut sset = Workload::new(sset, 3000, density, 10, 4);
                assert_eq!(arch_state(&arch), sset_state(&sset), "after settling");
                assert!(arch.store.archetypes().1 > 50, "the combinations multiply");
                assert_eq!(sset.store.archetypes().1, 1, "dense data in one archetype");
                let mut flags = Workload::new(Flags(World::new()), 3000, density, 10, 4);
                assert_eq!(arch_state(&arch), flags_state(&flags), "after settling");
                for _ in 0..4 {
                    let (a, s, f) = (arch.frame(), sset.frame(), flags.frame());
                    assert_ne!(a, 0, "the `and` queries found entities");
                    assert_eq!(a, s);
                    assert_eq!(a, f);
                    assert_eq!(arch_state(&arch), sset_state(&sset));
                    assert_eq!(arch_state(&arch), flags_state(&flags));
                }
            }
        }
    }

    #[test]
    fn the_dense_case_computes_the_same_on_every_storage() {
        let n = 2000;
        let (mut w, es) = dense_arch(n);
        let (mut set_driven, _) = dense_sset(n);
        let (mut row_driven, _) = dense_sset(n);
        for _ in 0..3 {
            dense_arch_iter(&mut w);
            dense_sset_iter(&mut set_driven, false);
            dense_sset_iter(&mut row_driven, true);
        }
        dense_arch_add_remove(&mut w, &es);
        dense_sset_add_remove(&mut set_driven, &es);
        let bits = |p: &Pos| (p.x.to_bits(), p.y.to_bits());
        for &e in &es {
            let expected = bits(w.get::<Pos>(e).unwrap());
            let start = Pos {
                x: e.index() as f32,
                y: 0.0,
            };
            assert_ne!(expected, bits(&start), "the loops ran");
            assert_eq!(bits(set_driven.world.get::<Pos>(e).unwrap()), expected);
            assert_eq!(bits(row_driven.world.get::<Pos>(e).unwrap()), expected);
            // Add-remove leaves every entity as it was, effects included.
            assert!(!w.has::<Tag<0>>(e) && !set_driven.has::<Tag<0>>(e));
            assert!(w.has::<Data<0>>(e) && w.has::<Data<1>>(e));
            assert!(set_driven.has::<Data<0>>(e) && set_driven.has::<Data<1>>(e));
        }
        let live = w.raw_ref().archetypes().iter().filter(|a| !a.is_empty()).count();
        assert_eq!(live, 1, "every entity is back in its table");
    }
}
