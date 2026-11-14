//! Differential tests of the archetype index: the same seeded sequence of
//! structural changes runs against a world whose queries use the index and
//! against one where it is off. After every step, random queries must return
//! exactly the same entities, in the same order, with the same values, on
//! every path (`for_each`, `iter`, `count`, `get`, the parallel paths and
//! chunk queries), and both must agree with a plain model of which entity
//! has which components. Change stamps must come out the same too.
//!
//! The changes are the ones an index could go stale on: new archetypes,
//! components registered late, entities moving between archetypes, despawns
//! that empty an archetype, `clear`, and `reset` followed by `spawn_at` (what
//! loading a saved world does).
//!
//! A failing case prints its seed. Rerun it with
//! `PONKO_ECS_SEED=<seed> cargo test -p ponko_ecs --test query_cache_differential`;
//! `PONKO_ECS_CASES` and `PONKO_ECS_OPS` change how much runs. Under Miri
//! the defaults shrink.

mod common;

use std::collections::BTreeMap;

use common::Rng;
use ponko_ecs::*;

// ---- components ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Component)]
struct Value(u64);
/// Eight zero-sized markers: bit `N` of an entity's mask.
#[derive(Component)]
struct M<const N: usize>;
/// Components registered by name while a case runs: bits 8 and up.
const LATE: usize = 4;
const BITS: usize = 8 + LATE;

/// One query result: the entity and its value.
type Rows = Vec<(Entity, u64)>;

// ---- typed filters --------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Path {
    ForEach,
    Iter,
    Count,
    Get,
    ParCollect,
    ParIndexed,
    /// `for_each` over `&mut Value`, adding one: both worlds change alike.
    Mutate,
}

const PATHS: [Path; 7] = [
    Path::ForEach,
    Path::Iter,
    Path::Count,
    Path::Get,
    Path::ParCollect,
    Path::ParIndexed,
    Path::Mutate,
];

fn hinted<'w, Q: QueryData>(query: Query<'w, Q>, hint: QueryCache) -> Query<'w, Q> {
    match hint {
        QueryCache::Always => query.cached(),
        QueryCache::Never => query.uncached(),
        _ => query,
    }
}

/// Runs `(Entity, &Value)` with filter `F` on one path. `Count` returns
/// placeholder rows: only how many there are is compared.
fn run<F: QueryFilter>(w: &mut World, hint: QueryCache, path: Path, live: &[Entity]) -> Rows {
    let mut rows = Rows::new();
    match path {
        Path::ForEach => {
            hinted(w.query::<(Entity, &Value)>().filter::<F>(), hint).for_each(|(e, v)| rows.push((e, v.0)))
        }
        Path::Iter => rows.extend(
            hinted(w.query::<(Entity, &Value)>().filter::<F>(), hint)
                .iter()
                .map(|(e, v)| (e, v.0)),
        ),
        Path::Count => {
            let n = hinted(w.query::<(Entity, &Value)>().filter::<F>(), hint).count();
            rows.extend((0..n).map(|_| (Entity::from_bits(0), 0)));
        }
        Path::Get => {
            let mut q = hinted(w.query::<(Entity, &Value)>().filter::<F>(), hint);
            rows.extend(live.iter().filter_map(|&e| q.get(e).map(|(e, v)| (e, v.0))));
        }
        #[cfg(feature = "parallel")]
        Path::ParCollect => {
            hinted(w.query::<(Entity, &Value)>().filter::<F>(), hint).par_collect_into(&mut rows, |(e, v)| (e, v.0))
        }
        #[cfg(feature = "parallel")]
        Path::ParIndexed => {
            use std::sync::atomic::{AtomicU64, Ordering};
            let mut q = hinted(w.query::<(Entity, &Value)>().filter::<F>(), hint);
            let slots: Vec<(AtomicU64, AtomicU64)> = (0..q.count()).map(|_| Default::default()).collect();
            q.par_for_each_indexed(|i, (e, v)| {
                slots[i].0.store(e.to_bits(), Ordering::Relaxed);
                slots[i].1.store(v.0, Ordering::Relaxed);
            });
            rows.extend(
                slots
                    .iter()
                    .map(|(e, v)| (Entity::from_bits(e.load(Ordering::Relaxed)), v.load(Ordering::Relaxed))),
            );
        }
        #[cfg(not(feature = "parallel"))]
        Path::ParCollect | Path::ParIndexed => return run::<F>(w, hint, Path::ForEach, live),
        Path::Mutate => hinted(w.query::<(Entity, &mut Value)>().filter::<F>(), hint).for_each(|(e, v)| {
            v.0 += 1;
            rows.push((e, v.0));
        }),
    }
    rows
}

/// `run` with data that requires marker 2 and fetches marker 3 if present:
/// required components are part of the signature, optional ones are not.
fn run_with_data<F: QueryFilter>(w: &mut World, hint: QueryCache, path: Path, live: &[Entity]) -> Rows {
    type Data<'a> = (Entity, &'a Value, &'a M<2>, Option<&'a M<3>>);
    let mut rows = Rows::new();
    match path {
        Path::Iter => rows.extend(
            hinted(w.query::<Data>().filter::<F>(), hint)
                .iter()
                .map(|(e, v, ..)| (e, v.0)),
        ),
        Path::Count => {
            let n = hinted(w.query::<Data>().filter::<F>(), hint).count();
            rows.extend((0..n).map(|_| (Entity::from_bits(0), 0)));
        }
        Path::Get => {
            let mut q = hinted(w.query::<Data>().filter::<F>(), hint);
            rows.extend(live.iter().filter_map(|&e| q.get(e).map(|(e, v, ..)| (e, v.0))));
        }
        #[cfg(feature = "parallel")]
        Path::ParCollect | Path::ParIndexed => {
            hinted(w.query::<Data>().filter::<F>(), hint).par_collect_into(&mut rows, |(e, v, ..)| (e, v.0))
        }
        _ => hinted(w.query::<Data>().filter::<F>(), hint).for_each(|(e, v, ..)| rows.push((e, v.0))),
    }
    rows
}

type RunFn = fn(&mut World, QueryCache, Path, &[Entity]) -> Rows;

/// A typed filter with what it means for an entity's component mask.
struct Typed {
    name: &'static str,
    run: RunFn,
    with_data: RunFn,
    holds: fn(u32) -> bool,
}

macro_rules! typed {
    ($filter:ty, $holds:expr) => {
        Typed {
            name: stringify!($filter),
            run: run::<$filter>,
            with_data: run_with_data::<$filter>,
            holds: $holds,
        }
    };
}

fn bit(mask: u32, n: u32) -> bool {
    mask >> n & 1 != 0
}

/// Everything the typed filters can say: plain `With`/`Without`, tuples,
/// `Or`, AND groups inside `Or`, nesting, the inline limits, filters that
/// match nothing or everything, and the same condition written differently.
fn typed_filters() -> Vec<Typed> {
    vec![
        typed!((), |_| true),
        typed!(With<M<0>>, |m| bit(m, 0)),
        typed!(Without<M<1>>, |m| !bit(m, 1)),
        typed!((With<M<0>>, With<M<1>>), |m| bit(m, 0) && bit(m, 1)),
        // The same, in another order and nested: one index entry.
        typed!((With<M<1>>, (With<M<0>>,)), |m| bit(m, 0) && bit(m, 1)),
        typed!((With<M<2>>, Without<M<3>>), |m| bit(m, 2) && !bit(m, 3)),
        typed!(Or<(With<M<0>>, With<M<1>>)>, |m| bit(m, 0) || bit(m, 1)),
        typed!(Or<(With<M<1>>, With<M<0>>, With<M<1>>)>, |m| bit(m, 0) || bit(m, 1)),
        typed!(Or<(With<M<0>>, Without<M<1>>)>, |m| bit(m, 0) || !bit(m, 1)),
        // A one-arm `Or` is a `With`.
        typed!(Or<(With<M<0>>,)>, |m| bit(m, 0)),
        // AND groups inside an `Or`.
        typed!(Or<((With<M<0>>, With<M<1>>), Without<M<2>>)>, |m| (bit(m, 0)
            && bit(m, 1))
            || !bit(m, 2)),
        typed!(Or<((With<M<0>>, With<M<1>>), (With<M<2>>, With<M<3>>))>, |m| (bit(
            m, 0
        ) && bit(
            m, 1
        )) || (bit(m, 2)
            && bit(m, 3))),
        // Several `Or`s and a `Without` together.
        typed!(
            (
                Or<(With<M<4>>, With<M<5>>)>,
                Or<(Without<M<6>>, With<M<7>>)>,
                Without<M<0>>
            ),
            |m| (bit(m, 4) || bit(m, 5)) && (!bit(m, 6) || bit(m, 7)) && !bit(m, 0)
        ),
        // Nested tuples and nested `Or`s.
        typed!(((With<M<1>>, (With<M<2>>,)), Without<M<5>>), |m| bit(m, 1)
            && bit(m, 2)
            && !bit(m, 5)),
        typed!(
            Or<(
                Or<(With<M<0>>, With<M<1>>)>,
                (With<M<2>>, Or<(With<M<3>>, Without<M<4>>)>)
            )>,
            |m| bit(m, 0) || bit(m, 1) || (bit(m, 2) && (bit(m, 3) || !bit(m, 4)))
        ),
        // Nothing, everything, and a contradiction.
        typed!(Or<()>, |_| false),
        typed!(Or<((), With<M<0>>)>, |_| true),
        typed!((With<M<0>>, Without<M<0>>), |_| false),
        // A clause the rest already decides, and one it rules out.
        typed!((With<M<0>>, Or<(With<M<0>>, With<M<5>>)>), |m| bit(m, 0)),
        typed!((Without<M<0>>, Or<(With<M<0>>, With<M<5>>)>), |m| !bit(m, 0)
            && bit(m, 5)),
        // The inline limits: 8 `With`, 8 `Without`, 4 clauses of 4 terms.
        typed!(
            (
                With<M<0>>,
                With<M<1>>,
                With<M<2>>,
                With<M<3>>,
                With<M<4>>,
                With<M<5>>,
                With<M<6>>,
                With<M<7>>
            ),
            |m| m & 0xFF == 0xFF
        ),
        typed!(
            (
                Without<M<0>>,
                Without<M<1>>,
                Without<M<2>>,
                Without<M<3>>,
                Without<M<4>>,
                Without<M<5>>,
                Without<M<6>>,
                Without<M<7>>
            ),
            |m| m & 0xFF == 0
        ),
        typed!(
            (
                Or<(With<M<0>>, With<M<1>>, With<M<2>>, With<M<3>>)>,
                Or<(Without<M<0>>, With<M<4>>, With<M<5>>, With<M<6>>)>,
                Or<(With<M<1>>, Without<M<5>>, With<M<7>>, Without<M<2>>)>,
                Or<(Without<M<3>>, Without<M<4>>, With<M<6>>, Without<M<7>>)>
            ),
            |m| (bit(m, 0) || bit(m, 1) || bit(m, 2) || bit(m, 3))
                && (!bit(m, 0) || bit(m, 4) || bit(m, 5) || bit(m, 6))
                && (bit(m, 1) || !bit(m, 5) || bit(m, 7) || !bit(m, 2))
                && (!bit(m, 3) || !bit(m, 4) || bit(m, 6) || !bit(m, 7))
        ),
    ]
}

// ---- runtime filters --------------------------------------------------------------

/// A random filter tree over component slots `0..BITS`.
fn random_tree(rng: &mut Rng, depth: usize) -> Tree {
    match rng.below(if depth >= 3 { 2 } else { 5 }) {
        0 => Tree::With(rng.below(BITS)),
        1 => Tree::Without(rng.below(BITS)),
        2 | 3 => Tree::Or((0..rng.below(4)).map(|_| random_tree(rng, depth + 1)).collect()),
        _ => Tree::All((0..rng.below(4)).map(|_| random_tree(rng, depth + 1)).collect()),
    }
}

#[derive(Debug, Clone)]
enum Tree {
    With(usize),
    Without(usize),
    All(Vec<Tree>),
    Or(Vec<Tree>),
}

impl Tree {
    fn holds(&self, mask: u32) -> bool {
        match self {
            Tree::With(slot) => bit(mask, *slot as u32),
            Tree::Without(slot) => !bit(mask, *slot as u32),
            Tree::All(parts) => parts.iter().all(|p| p.holds(mask)),
            Tree::Or(arms) => arms.iter().any(|a| a.holds(mask)),
        }
    }

    /// The filter over a world's component ids; `None` if it names a slot
    /// that isn't registered yet.
    fn to_dyn(&self, ids: &[Option<ComponentId>]) -> Option<DynFilter> {
        Some(match self {
            Tree::With(slot) => DynFilter::With(ids[*slot]?),
            Tree::Without(slot) => DynFilter::Without(ids[*slot]?),
            Tree::All(parts) => DynFilter::All(parts.iter().map(|p| p.to_dyn(ids)).collect::<Option<_>>()?),
            Tree::Or(arms) => DynFilter::Or(arms.iter().map(|a| a.to_dyn(ids)).collect::<Option<_>>()?),
        })
    }
}

/// The rows of a chunk query over `Value` and the components of `also`.
fn chunks(w: &mut World, also: &[ComponentId], filter: Option<&CompiledFilter>, hint: Option<QueryCache>) -> Rows {
    let mut ids = vec![w.register::<Value>()];
    ids.extend_from_slice(also);
    let mut query = ChunkQuery::new(&ids).writes(0).filter(filter);
    if let Some(hint) = hint {
        query = query.cache(hint);
    }
    let mut rows = Rows::new();
    let collect = |chunk: Chunk<'_>| {
        let values = chunk.columns[0].cast::<Value>();
        for (i, &e) in chunk.entities.iter().enumerate() {
            // SAFETY: column 0 holds `entities.len()` `Value`s, only read.
            rows.push((e, unsafe { (*values.add(i)).0 }));
        }
    };
    w.raw().query_chunks(query, collect);
    rows
}

// ---- the two worlds and their model ---------------------------------------------

/// The policies the indexed world runs under, from eager to starved.
fn policy(kind: usize) -> QueryCachePolicy {
    let mut policy = QueryCachePolicy::new();
    policy.min_archetypes = 1;
    match kind {
        // The default decisions, from the first archetype.
        0 => {}
        // Everything indexed at once.
        1 => {
            policy.promote_after = 1;
            policy.max_match_percent = 100;
        }
        // Room for three signatures: constant eviction.
        2 => {
            policy.promote_after = 1;
            policy.max_match_percent = 100;
            policy.max_signatures = 3;
        }
        // Room for eight archetype indices: indices are given up.
        3 => {
            policy.promote_after = 1;
            policy.max_match_percent = 100;
            policy.max_index_bytes = 32;
        }
        // Really the default: only worlds with many archetypes.
        _ => policy = QueryCachePolicy::new(),
    }
    policy
}

struct Case {
    /// Uses the index.
    indexed: World,
    /// The index off: the reference.
    plain: World,
    /// What each live entity has: bit `i` for slot `i`, and its value.
    model: BTreeMap<Entity, (u32, u64)>,
    live: Vec<Entity>,
    late: usize,
    rng: Rng,
    filters: Vec<Typed>,
    serial: u64,
}

macro_rules! each_marker {
    ($mask:expr, $world:expr, $e:expr, $op:ident) => {
        each_marker!(@go $mask, $world, $e, $op, 0 1 2 3 4 5 6 7)
    };
    (@go $mask:expr, $world:expr, $e:expr, insert, $($n:literal)*) => {
        $(if bit($mask, $n) { $world.insert($e, M::<$n>); })*
    };
    (@go $mask:expr, $world:expr, $e:expr, remove, $($n:literal)*) => {
        $(if bit($mask, $n) { $world.remove::<M<$n>>($e); })*
    };
}

impl Case {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let mut indexed = World::new();
        indexed.set_query_cache_policy(policy(rng.below(5)));
        let mut plain = World::new();
        plain.set_query_cache_policy(QueryCachePolicy::off());
        for w in [&mut indexed, &mut plain] {
            w.track_changes::<Value>();
        }
        Case {
            indexed,
            plain,
            model: BTreeMap::new(),
            live: Vec::new(),
            late: 0,
            rng,
            filters: typed_filters(),
            serial: 0,
        }
    }

    fn worlds(&mut self) -> [&mut World; 2] {
        [&mut self.indexed, &mut self.plain]
    }

    /// The id of slot `slot` in a world, if it is registered there.
    fn ids(w: &World, late: usize) -> Vec<Option<ComponentId>> {
        let mut ids = vec![
            w.component_id::<M<0>>(),
            w.component_id::<M<1>>(),
            w.component_id::<M<2>>(),
            w.component_id::<M<3>>(),
            w.component_id::<M<4>>(),
            w.component_id::<M<5>>(),
            w.component_id::<M<6>>(),
            w.component_id::<M<7>>(),
        ];
        for i in 0..LATE {
            ids.push((i < late).then(|| w.raw_ref().component_by_name(&format!("late-{i}")).unwrap()));
        }
        ids
    }

    fn pick(&mut self) -> Option<Entity> {
        (!self.live.is_empty()).then(|| self.live[self.rng.below(self.live.len())])
    }

    /// A marker mask that favors few markers, so archetypes repeat.
    fn mask(&mut self) -> u32 {
        (self.rng.next_u64() & self.rng.next_u64() & 0xFF) as u32
    }

    fn spawn(&mut self, mask: u32) {
        self.serial += 1;
        let value = self.serial;
        let mut spawned = None;
        for w in self.worlds() {
            let e = w.spawn((Value(value),));
            each_marker!(mask, w, e, insert);
            assert!(
                spawned.is_none_or(|other| other == e),
                "both worlds hand out the same ids"
            );
            spawned = Some(e);
        }
        let e = spawned.unwrap();
        self.model.insert(e, (mask, value));
        self.live.push(e);
    }

    fn despawn(&mut self, e: Entity) {
        for w in self.worlds() {
            assert!(w.despawn(e));
        }
        self.model.remove(&e);
        self.live.retain(|&other| other != e);
    }

    fn step(&mut self) {
        match self.rng.below(100) {
            0..=29 => {
                let mask = self.mask();
                self.spawn(mask);
            }
            30..=39 => {
                if let Some(e) = self.pick() {
                    self.despawn(e);
                }
            }
            // Empty one archetype: despawn everything with one entity's mask.
            40..=44 => {
                if let Some(e) = self.pick() {
                    let mask = self.model[&e].0;
                    let same: Vec<Entity> = self
                        .model
                        .iter()
                        .filter(|(_, m)| m.0 == mask)
                        .map(|(&e, _)| e)
                        .collect();
                    for e in same {
                        self.despawn(e);
                    }
                }
            }
            // Move an entity between archetypes.
            45..=64 => {
                if let Some(e) = self.pick() {
                    let flip = 1u32 << self.rng.below(8);
                    let had = self.model[&e].0 & flip != 0;
                    for w in self.worlds() {
                        if had {
                            each_marker!(flip, w, e, remove);
                        } else {
                            each_marker!(flip, w, e, insert);
                        }
                    }
                    self.model.get_mut(&e).unwrap().0 ^= flip;
                }
            }
            // Register a component late.
            65..=67 => {
                if self.late < LATE {
                    let name = format!("late-{}", self.late);
                    for w in self.worlds() {
                        // SAFETY: a plain 4-byte component without a destructor.
                        unsafe { w.raw().register_component(&name, 4, 4, None) };
                    }
                    self.late += 1;
                }
            }
            // Give an entity a late component, or take it away.
            68..=77 => {
                if let (Some(e), true) = (self.pick(), self.late > 0) {
                    let slot = 8 + self.rng.below(self.late);
                    let had = bit(self.model[&e].0, slot as u32);
                    let late = self.late;
                    for w in self.worlds() {
                        let id = Self::ids(w, late)[slot].unwrap();
                        let value = 7u32;
                        if had {
                            assert!(w.raw().remove_dynamic(e, id));
                        } else {
                            // SAFETY: `value` is a readable 4-byte value of the registered layout.
                            assert!(unsafe { w.raw().insert_dynamic(e, id, (&raw const value).cast()) });
                        }
                    }
                    self.model.get_mut(&e).unwrap().0 ^= 1 << slot;
                }
            }
            // Change a value outside any query.
            78..=84 => {
                if let Some(e) = self.pick() {
                    for w in self.worlds() {
                        w.get_mut::<Value>(e).unwrap().0 += 100;
                    }
                    self.model.get_mut(&e).unwrap().1 += 100;
                }
            }
            85..=86 => {
                for w in self.worlds() {
                    w.clear();
                }
                self.model.clear();
                self.live.clear();
            }
            // What loading a saved world does: forget every id, then put a
            // saved set of entities back under their old ids.
            87..=88 => {
                let saved: Vec<(Entity, (u32, u64))> = self.model.iter().map(|(&e, &row)| (e, row)).collect();
                for w in self.worlds() {
                    w.reset();
                }
                self.model.clear();
                self.live.clear();
                for (e, (mask, value)) in saved.into_iter().filter(|(_, (mask, _))| mask >> 8 == 0).step_by(2) {
                    for w in self.worlds() {
                        assert!(w.spawn_at(e, (Value(value),)));
                        each_marker!(mask, w, e, insert);
                    }
                    self.model.insert(e, (mask, value));
                    self.live.push(e);
                }
            }
            // A new reader tick.
            89..=91 => {
                for w in self.worlds() {
                    let tick = w.advance_change_tick();
                    w.set_changes_since(tick);
                }
            }
            // A new policy: the index starts over.
            92..=93 => {
                let policy = policy(self.rng.below(5));
                self.indexed.set_query_cache_policy(policy);
            }
            _ => {}
        }
    }

    /// The model's answer, as a set: which entities with which values.
    fn expected(&self, holds: impl Fn(u32) -> bool) -> Rows {
        self.model
            .iter()
            .filter(|(_, (mask, _))| holds(*mask))
            .map(|(&e, &(_, value))| (e, value))
            .collect()
    }

    fn check(&mut self) {
        let hints = [QueryCache::Auto, QueryCache::Always, QueryCache::Never];
        // Typed filters.
        for _ in 0..3 {
            let f = self.rng.below(self.filters.len());
            let path = PATHS[self.rng.below(PATHS.len())];
            let hint = hints[self.rng.below(3)];
            let with_data = self.rng.one_in(4) && path != Path::Mutate;
            let filter = &self.filters[f];
            let (run, name, holds) = (
                if with_data { filter.with_data } else { filter.run },
                filter.name,
                filter.holds,
            );
            let reference = run(&mut self.plain, QueryCache::Auto, path, &self.live);
            let got = run(&mut self.indexed, hint, path, &self.live);
            assert_eq!(
                got, reference,
                "filter `{name}`, {path:?}, {hint:?}, with data: {with_data}"
            );
            if path == Path::Mutate {
                for (e, value) in &got {
                    self.model.get_mut(e).unwrap().1 = *value;
                }
            }
            let mut expected = self.expected(|mask| holds(mask) && (!with_data || bit(mask, 2)));
            let mut sorted = reference;
            if path == Path::Count {
                expected.iter_mut().for_each(|row| *row = (Entity::from_bits(0), 0));
            }
            sorted.sort_unstable();
            expected.sort_unstable();
            assert_eq!(sorted, expected, "filter `{name}` against the model, {path:?}");
        }
        // The same query again, every hint: an index that was just built or
        // extended, and one that is reused.
        let f = self.rng.below(self.filters.len());
        let (run, name) = (self.filters[f].run, self.filters[f].name);
        let reference = run(&mut self.plain, QueryCache::Auto, Path::ForEach, &self.live);
        for hint in [
            QueryCache::Auto,
            QueryCache::Auto,
            QueryCache::Always,
            QueryCache::Auto,
            QueryCache::Never,
        ] {
            let got = run(&mut self.indexed, hint, Path::Iter, &self.live);
            assert_eq!(got, reference, "filter `{name}` repeated, {hint:?}");
        }
        // Runtime filters through the chunk path.
        let tree = random_tree(&mut self.rng, 0);
        let late = self.late;
        let (ids, plain_ids) = (Self::ids(&self.indexed, late), Self::ids(&self.plain, late));
        assert_eq!(ids, plain_ids, "both worlds registered the same components");
        let filter = tree.to_dyn(&ids).map(|filter| filter.compile());
        let also: Vec<ComponentId> = (0..self.rng.below(3))
            .filter_map(|_| ids[self.rng.below(BITS)])
            .collect();
        let also_mask = |mask: u32| {
            also.iter()
                .all(|id| bit(mask, ids.iter().position(|slot| *slot == Some(*id)).unwrap() as u32))
        };
        if let Some(Ok(filter)) = filter {
            let reference = chunks(&mut self.plain, &also, Some(&filter), None);
            for hint in [
                None,
                Some(QueryCache::Auto),
                Some(QueryCache::Always),
                Some(QueryCache::Never),
            ] {
                let got = chunks(&mut self.indexed, &also, Some(&filter), hint);
                assert_eq!(got, reference, "runtime filter {tree:?} over {also:?}, {hint:?}");
            }
            let mut expected = self.expected(|mask| tree.holds(mask) && also_mask(mask));
            let mut sorted = reference;
            sorted.sort_unstable();
            expected.sort_unstable();
            assert_eq!(
                sorted, expected,
                "runtime filter {tree:?} over {also:?} against the model"
            );
        }
        // The unfiltered chunk paths.
        let reference = chunks(&mut self.plain, &also, None, None);
        assert_eq!(chunks(&mut self.indexed, &also, None, None), reference);
        assert_eq!(
            chunks(&mut self.indexed, &also, None, Some(QueryCache::Always)),
            reference
        );
        let mut by_raw = Rows::new();
        let mut raw_ids = vec![self.indexed.register::<Value>()];
        raw_ids.extend_from_slice(&also);
        // (`for_each_chunk` hands out writable columns: it stamps `Value` in
        // this world only, so it runs after the stamps are compared below.)
        // Change detection: what counts as changed is the same in both.
        let changed = |w: &mut World| {
            let mut rows = Rows::new();
            w.query::<(Entity, &Value)>()
                .filter::<Changed<Value>>()
                .for_each(|(e, v)| rows.push((e, v.0)));
            (rows, w.change_tick(), w.changes_since())
        };
        assert_eq!(
            changed(&mut self.indexed),
            changed(&mut self.plain),
            "changed rows and ticks"
        );
        let added = |w: &mut World, hint| {
            let mut rows = Rows::new();
            hinted(w.query::<(Entity, &Value)>(), hint)
                .filter::<(Added<Value>, Or<(With<M<0>>, Without<M<1>>)>)>()
                .for_each(|(e, v)| rows.push((e, v.0)));
            rows
        };
        let reference = added(&mut self.plain, QueryCache::Auto);
        assert_eq!(added(&mut self.indexed, QueryCache::Auto), reference);
        assert_eq!(added(&mut self.indexed, QueryCache::Always), reference);
        for w in self.worlds() {
            w.raw().for_each_chunk(&raw_ids, |chunk| {
                let values = chunk.columns[0].cast::<Value>();
                for (i, &e) in chunk.entities.iter().enumerate() {
                    // SAFETY: column 0 holds `entities.len()` `Value`s.
                    by_raw.push((e, unsafe { (*values.add(i)).0 }));
                }
            });
        }
        let (indexed_rows, plain_rows) = by_raw.split_at(by_raw.len() / 2);
        assert_eq!(indexed_rows, plain_rows, "raw chunks");
    }
}

// ---- running cases ----------------------------------------------------------------

fn env(name: &str) -> Option<u64> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    Some(match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).expect("a hex number"),
        None => raw.parse().expect("a number"),
    })
}

fn run_case(seed: u64, ops: usize) {
    let outcome = std::panic::catch_unwind(|| {
        let mut case = Case::new(seed);
        for _ in 0..ops {
            case.step();
            case.check();
        }
        case.indexed.query_cache_stats()
    });
    match outcome {
        Ok(_) => {}
        Err(panic) => {
            eprintln!(
                "\nfailing seed: rerun with PONKO_ECS_SEED={seed:#x} PONKO_ECS_OPS={ops} cargo test -p ponko_ecs --test query_cache_differential\n"
            );
            std::panic::resume_unwind(panic);
        }
    }
}

#[test]
fn indexed_and_scanned_queries_agree_under_random_structural_changes() {
    let ops = env("PONKO_ECS_OPS").map_or(if cfg!(miri) { 6 } else { 300 }, |n| n as usize);
    if let Some(seed) = env("PONKO_ECS_SEED") {
        return run_case(seed, ops);
    }
    let cases = env("PONKO_ECS_CASES").unwrap_or(if cfg!(miri) { 1 } else { 60 });
    for case in 0..cases {
        run_case(0x139_0000 + case, ops);
    }
}

/// The random cases stay small; this one has enough archetypes for the
/// default policy, and checks that the index is really in use while the
/// answers stay equal.
#[test]
fn the_default_policy_indexes_a_big_world_and_changes_no_answer() {
    let mut case = Case::new(4);
    case.indexed.set_query_cache_policy(QueryCachePolicy::new());
    // Miri interprets every instruction: just enough archetypes for the
    // default policy there, and a few steps.
    let masks = if cfg!(miri) { 64 } else { 256 };
    for mask in 0..masks {
        case.spawn(mask);
    }
    assert!(case.indexed.raw_ref().archetypes().len() >= masks as usize);
    let ops = if cfg!(miri) { 2 } else { 400 };
    for _ in 0..ops {
        case.step();
        // Keep this world big and on the default policy.
        if case.live.len() < masks as usize / 2 {
            for mask in 0..masks {
                case.spawn(mask);
            }
        }
        case.indexed.set_query_cache_policy_if_changed();
        case.check();
    }
    let stats = case.indexed.query_cache_stats();
    assert!(stats.indexed > 0 && stats.hits > 0, "{stats:?}");
    assert_eq!(case.plain.query_cache_stats().signatures, 0);
}

trait KeepDefault {
    fn set_query_cache_policy_if_changed(&mut self);
}
impl KeepDefault for World {
    /// A step may have set another policy: go back to the default one,
    /// without forgetting the index when nothing changed.
    fn set_query_cache_policy_if_changed(&mut self) {
        if self.query_cache_policy() != QueryCachePolicy::new() {
            self.set_query_cache_policy(QueryCachePolicy::new());
        }
    }
}

/// A world that churns through more component sets than it keeps empty
/// archetypes for reclaims them and builds new ones in their slots. Both
/// worlds make the same changes, so they reclaim the same slots; every
/// answer must still match the model, also from indices built before a
/// slot was reused for another set.
#[test]
fn reclaimed_and_reused_archetype_slots_change_no_answer() {
    let (cases, rounds) = if cfg!(miri) { (1, 12) } else { (10, 150) };
    for seed in 0..cases {
        let outcome = std::panic::catch_unwind(|| {
            let mut case = Case::new(0x347_0000 + seed);
            case.indexed.set_query_cache_policy(policy(seed as usize % 5));
            let mut sets = std::collections::BTreeSet::new();
            for _ in 0..rounds {
                // A few short-lived entities with any of the 256 marker
                // sets: each insert passes through intermediate sets too,
                // which empty again at once.
                let mut spawned = Vec::new();
                for _ in 0..4 {
                    let mask = (case.rng.next_u64() & 0xFF) as u32;
                    sets.insert(mask);
                    case.spawn(mask);
                    spawned.push(*case.live.last().unwrap());
                }
                case.step();
                case.check();
                for e in spawned {
                    if case.model.contains_key(&e) {
                        case.despawn(e);
                    }
                }
                case.check();
            }
            let slots = case.indexed.raw_ref().archetypes().len();
            assert_eq!(slots, case.plain.raw_ref().archetypes().len());
            if !cfg!(miri) {
                assert!(
                    slots < sets.len(),
                    "{} marker sets lived in {slots} archetype slots: none reused",
                    sets.len()
                );
            }
        });
        if let Err(panic) = outcome {
            eprintln!("\nfailing case: seed {:#x}\n", 0x347_0000 + seed);
            std::panic::resume_unwind(panic);
        }
    }
}
