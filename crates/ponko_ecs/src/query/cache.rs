//! The world's archetype index for repeated queries: which archetypes a
//! query signature matches, so a query in a world with many archetypes visits
//! its candidates instead of testing every archetype.
//!
//! What is kept between queries is archetype indices only, never column
//! pointers or `QueryData::State`: column buffers and the archetype `Vec` may
//! move between queries. Every loop still tests its own filters and resolves
//! its columns for each candidate under the live world borrow, so the index
//! can only narrow which archetypes are looked at, in the same ascending
//! order a scan uses.
//!
//! Archetype slots are never removed, and an entry only has to match the
//! slots created since it last looked. An empty archetype may be reclaimed
//! and its slot reused for another component set (see `slots.rs`): a
//! reclaimed slot can stay in a list, since loops skip empty archetypes,
//! and when the slot is reused, [`MatchCache::reused`] adds it to or removes
//! it from every list before any query runs again. So an indexed entry
//! always lists every matching archetype below its generation.
//!
//! Queries that hold the world exclusively ([`World::query`], chunk
//! queries) build and extend the index. Queries of a
//! [`WorldView`](crate::WorldView) hold it shared, possibly on several
//! threads at once, so they only read it: an entry that is indexed and has
//! seen every archetype is used as it is, without a lock. Anything else is
//! scanned, and the signature is noted in a small list (behind a mutex that
//! is only tried, never waited for) for the next moment the world is held
//! exclusively, when [`MatchCache::settle`] counts it as a use.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{Archetypes, Filters, MAX_OR, MAX_OR_TERMS, MAX_WITH, QueryData};
use crate::storage::{Archetype, ComponentId};
use crate::world::World;

/// Whether one query uses the world's archetype index (see
/// [`QueryCachePolicy`]). Results, their order and change detection are the
/// same either way; only the time spent finding archetypes differs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum QueryCache {
    /// The world's policy decides. The default.
    #[default]
    Auto,
    /// Index this query's signature on first use, however many archetypes
    /// it matches: what [`Query::cached`](crate::Query::cached) asks for.
    Always,
    /// Test every archetype: what [`Query::uncached`](crate::Query::uncached)
    /// asks for.
    Never,
}

/// When a world indexes the archetypes of repeated queries. Set it with
/// [`World::set_query_cache_policy`].
///
/// A query's *signature* is the set of archetypes it can match: its required
/// components and its `With` / `Without` / `Or` filters, in a normal form
/// (so `query::<&A>().filter::<With<B>>()` and `query::<(&A, &B)>()` share one).
/// The default policy indexes a signature when all of these hold:
///
/// - the world has at least [`min_archetypes`](Self::min_archetypes)
///   archetypes: below that, testing every archetype is as fast or faster;
/// - the signature was used [`promote_after`](Self::promote_after) times,
///   so one-off queries build nothing;
/// - it matches at most [`max_match_percent`](Self::max_match_percent) of the
///   archetypes: an index of nearly everything saves nothing;
/// - the index stays within [`max_signatures`](Self::max_signatures) and
///   [`max_index_bytes`](Self::max_index_bytes).
///
/// Everything else is scanned as before. The measurements behind the
/// defaults are in `crates/ponko_bench/QUERY_CACHE.md`.
///
/// ```
/// use ponko_ecs::{QueryCachePolicy, World};
///
/// let mut world = World::new();
/// // Turn the index off for this world:
/// world.set_query_cache_policy(QueryCachePolicy::off());
/// // Or tune it:
/// let mut policy = QueryCachePolicy::new();
/// policy.min_archetypes = 256;
/// world.set_query_cache_policy(policy);
/// assert_eq!(world.query_cache_policy().min_archetypes, 256);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueryCachePolicy {
    /// Fewest archetypes in the world for any query to use the index,
    /// hinted or not. `usize::MAX` turns the index off.
    pub min_archetypes: usize,
    /// Uses of a signature before it is indexed without a hint.
    pub promote_after: u32,
    /// Without a hint, a signature is only indexed while it matches at most
    /// this share of the archetypes, in percent. A world keeps at most 100:
    /// [`World::set_query_cache_policy`] lowers anything above to it.
    pub max_match_percent: u8,
    /// Most signatures remembered. When it is reached, new signatures scan,
    /// and signatures that went unused are dropped to make room. Raise it
    /// if [`QueryCacheStats::evictions`] keeps growing: the world runs more
    /// signatures than fit, and drops ones it still uses now and then.
    pub max_signatures: usize,
    /// Most bytes of archetype indices kept, over all signatures (4 bytes
    /// per matching archetype). A signature that doesn't fit scans.
    pub max_index_bytes: usize,
}

impl QueryCachePolicy {
    /// The default policy.
    #[must_use]
    pub const fn new() -> Self {
        QueryCachePolicy {
            min_archetypes: DEFAULT_MIN_ARCHETYPES,
            promote_after: 2,
            max_match_percent: 50,
            max_signatures: 128,
            max_index_bytes: 1 << 20,
        }
    }

    /// No index at all: every query tests every archetype, including
    /// queries with a [`cached`](crate::Query::cached) hint, and the world
    /// keeps no index memory.
    #[must_use]
    pub const fn off() -> Self {
        QueryCachePolicy {
            min_archetypes: usize::MAX,
            ..Self::new()
        }
    }

    /// Whether this policy turns the index off.
    #[must_use]
    pub const fn is_off(&self) -> bool {
        self.min_archetypes == usize::MAX
    }
}

impl Default for QueryCachePolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// What a world's archetype index holds, from [`World::query_cache_stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueryCacheStats {
    /// Signatures remembered, indexed or not.
    pub signatures: usize,
    /// Signatures that have an index.
    pub indexed: usize,
    /// Archetype indices held, over all signatures.
    pub indices: usize,
    /// Heap bytes the index owns: the signature table and the index lists,
    /// with their spare capacity.
    pub bytes: usize,
    /// Queries answered from an index so far, not counting the queries of
    /// a [`WorldView`](crate::WorldView) (they only read the index).
    pub hits: u64,
    /// Signatures dropped to make room so far.
    pub evictions: u64,
}

/// The measured crossover: see `crates/ponko_bench/QUERY_CACHE.md`.
const DEFAULT_MIN_ARCHETYPES: usize = 64;
/// New signatures refused by a full table before the unused ones are dropped.
const SWEEP_AFTER_MISSES: u32 = 32;
/// Signatures the table makes room for at once, so remembering a new
/// signature doesn't allocate.
const RESERVE_SIGNATURES: usize = 128;

/// Times views run on a full table before they may note new signatures
/// again, for one round.
const REOPEN_AFTER_ROUNDS: u32 = 64;

/// Signatures view queries can ask for between two settles.
const MAX_WANTED: usize = 16;

/// Most components a signature must have: 32 fetched, 8 `with`, and one per
/// `Or` clause that came down to a single component.
const MAX_HAS: usize = 32 + MAX_WITH + MAX_OR;
const MAX_NOT: usize = MAX_WITH + MAX_OR;

/// A query signature in normal form: equal keys match exactly the same
/// archetypes, and filters that say the same thing in the ways below get
/// equal keys.
///
/// - `has`: components an archetype must have (fetched as required, or
///   `With`), sorted, each once.
/// - `not`: components it must not have, sorted, each once.
/// - clauses: `Or` filters as ANDed clauses of literals (component, present
///   or absent); `ends[c]` is where clause `c` ends in `lits`. Literals are
///   sorted and unique inside a clause, clauses are sorted, and a clause is
///   dropped when it always holds (it has a literal `has`/`not` already
///   guarantees, or a component both present and absent) or when another
///   clause implies it. Literals `has`/`not` rule out are removed; a clause
///   left with one literal moves into `has`/`not`.
/// - `never`: nothing can match (an empty clause, or a component both
///   required and excluded). Everything else is then empty.
///
/// `Added`/`Changed` don't appear: they pick rows, not archetypes, beyond
/// requiring their component.
///
/// Unused slots stay zeroed, so comparing whole keys is comparing signatures.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct MatchKey {
    has: [ComponentId; MAX_HAS],
    not: [ComponentId; MAX_NOT],
    lits: [ComponentId; MAX_OR_TERMS],
    /// Bit `i`: literal `i` asks for presence.
    lit_has: u16,
    ends: [u8; MAX_OR],
    n_has: u8,
    n_not: u8,
    n_clauses: u8,
    never: bool,
}

impl Hash for MatchKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let lits = self.n_lits();
        state.write_u32(
            u32::from(self.n_has)
                | u32::from(self.n_not) << 8
                | u32::from(self.n_clauses) << 16
                | u32::from(self.never) << 24,
        );
        for id in &self.has[..self.n_has as usize] {
            state.write_u32(id.0);
        }
        for id in &self.not[..self.n_not as usize] {
            state.write_u32(id.0);
        }
        for id in &self.lits[..lits] {
            state.write_u32(id.0);
        }
        if self.n_clauses != 0 {
            state.write_u32(u32::from(self.lit_has));
            state.write_u32(u32::from_le_bytes(self.ends));
        }
    }
}

/// Sorted insert of `id` into `set[..*n]`, unless it is there.
#[inline]
fn insert_sorted(set: &mut [ComponentId], n: &mut u8, id: ComponentId) {
    let len = *n as usize;
    let at = set[..len].iter().position(|&other| other >= id).unwrap_or(len);
    if at < len && set[at] == id {
        return;
    }
    set.copy_within(at..len, at + 1);
    set[at] = id;
    *n += 1;
}

type Literal = (ComponentId, bool);

/// The signature of the typed query `Q` with `filters`.
#[inline(never)]
pub(crate) fn signature<Q: QueryData>(ids: &Q::Ids, filters: &Filters) -> MatchKey {
    MatchKey::new(
        |require| {
            Q::access(ids, &mut |id, _, required| {
                if required {
                    require(id);
                }
            });
        },
        Some(filters),
    )
}

impl MatchKey {
    /// The key of a query that requires what `required` reports and passes
    /// `filters`. `required` may report a component more than once.
    #[inline]
    pub(crate) fn new(required: impl FnOnce(&mut dyn FnMut(ComponentId)), filters: Option<&Filters>) -> Self {
        let mut key = MatchKey {
            has: [ComponentId(0); MAX_HAS],
            not: [ComponentId(0); MAX_NOT],
            lits: [ComponentId(0); MAX_OR_TERMS],
            lit_has: 0,
            ends: [0; MAX_OR],
            n_has: 0,
            n_not: 0,
            n_clauses: 0,
            never: false,
        };
        required(&mut |id| insert_sorted(&mut key.has, &mut key.n_has, id));
        if let Some(filters) = filters {
            key.add_filters(filters);
        }
        key
    }

    fn n_lits(&self) -> usize {
        match self.n_clauses {
            0 => 0,
            n => self.ends[n as usize - 1] as usize,
        }
    }

    fn has(&self, id: ComponentId) -> bool {
        self.has[..self.n_has as usize].contains(&id)
    }

    fn not(&self, id: ComponentId) -> bool {
        self.not[..self.n_not as usize].contains(&id)
    }

    /// The key that matches nothing.
    fn set_never(&mut self) {
        *self = MatchKey::new(|_| {}, None);
        self.never = true;
    }

    fn add_filters(&mut self, filters: &Filters) {
        for &id in &filters.with[..filters.n_with as usize] {
            insert_sorted(&mut self.has, &mut self.n_has, id);
        }
        for &id in &filters.without[..filters.n_without as usize] {
            insert_sorted(&mut self.not, &mut self.n_not, id);
        }
        if filters.n_any != 0 && !self.add_clauses(filters) {
            self.set_never();
            return;
        }
        if self.not[..self.n_not as usize].iter().any(|&id| self.has(id)) {
            self.set_never();
        }
    }

    /// Brings the `Or` clauses of `filters` into normal form. `false`: a
    /// clause can't hold, so nothing matches.
    #[inline(never)]
    fn add_clauses(&mut self, filters: &Filters) -> bool {
        let mut clauses = [[(ComponentId(0), false); MAX_OR_TERMS]; MAX_OR];
        let mut lens = [0usize; MAX_OR];
        let mut n = 0;
        let mut start = 0;
        for &end in &filters.any_end[..filters.n_any as usize] {
            let end = end as usize;
            let clause = &mut clauses[n];
            let mut len = 0;
            for i in start..end {
                clause[len] = (filters.any[i], filters.any_has >> i & 1 != 0);
                len += 1;
            }
            clause[..len].sort_unstable();
            // Unique literals.
            let mut unique = 0;
            for i in 0..len {
                if unique == 0 || clause[unique - 1] != clause[i] {
                    clause[unique] = clause[i];
                    unique += 1;
                }
            }
            lens[n] = unique;
            n += 1;
            start = end;
        }
        // Apply what `has`/`not` already say, until nothing changes: a
        // clause that comes down to one literal adds to them.
        let mut live = [true; MAX_OR];
        let mut changed = true;
        while changed {
            changed = false;
            for c in 0..n {
                if !live[c] {
                    continue;
                }
                let clause = &mut clauses[c];
                let len = lens[c];
                let holds = |&(id, has): &Literal| if has { self.has(id) } else { self.not(id) };
                // Sorted, so a component present and absent is adjacent.
                let both = clause[..len].windows(2).any(|pair| pair[0].0 == pair[1].0);
                if both || clause[..len].iter().any(holds) {
                    live[c] = false;
                    continue;
                }
                let mut kept = 0;
                for i in 0..len {
                    let (id, has) = clause[i];
                    let ruled_out = if has { self.not(id) } else { self.has(id) };
                    if !ruled_out {
                        clause[kept] = clause[i];
                        kept += 1;
                    }
                }
                lens[c] = kept;
                match kept {
                    0 => return false,
                    1 => {
                        let (id, has) = clause[0];
                        if has {
                            insert_sorted(&mut self.has, &mut self.n_has, id);
                        } else {
                            insert_sorted(&mut self.not, &mut self.n_not, id);
                        }
                        live[c] = false;
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
        // A clause whose literals all appear in another one implies it: drop
        // the wider one (of two equal clauses, the later).
        for a in 0..n {
            for b in 0..n {
                if a == b || !live[a] || !live[b] {
                    continue;
                }
                let (small, wide) = (&clauses[a][..lens[a]], &clauses[b][..lens[b]]);
                let implied = small.iter().all(|lit| wide.contains(lit));
                if implied && (small.len() < wide.len() || a < b) {
                    live[b] = false;
                }
            }
        }
        // Clauses in a fixed order.
        let mut order = [0usize; MAX_OR];
        let mut count = 0;
        for c in (0..n).filter(|&c| live[c]) {
            order[count] = c;
            count += 1;
        }
        order[..count].sort_unstable_by(|&a, &b| clauses[a][..lens[a]].cmp(&clauses[b][..lens[b]]));
        let mut at = 0;
        for (slot, &c) in order[..count].iter().enumerate() {
            for &(id, has) in &clauses[c][..lens[c]] {
                self.lits[at] = id;
                self.lit_has |= u16::from(has) << at;
                at += 1;
            }
            self.ends[slot] = at as u8;
        }
        self.n_clauses = count as u8;
        true
    }

    /// Whether `arch` is one of the archetypes this signature matches.
    pub(crate) fn matches(&self, arch: &Archetype) -> bool {
        if self.never
            || !self.has[..self.n_has as usize].iter().all(|&id| arch.has(id))
            || self.not[..self.n_not as usize].iter().any(|&id| arch.has(id))
        {
            return false;
        }
        let mut start = 0;
        for &end in &self.ends[..self.n_clauses as usize] {
            let end = end as usize;
            if !(start..end).any(|i| arch.has(self.lits[i]) == (self.lit_has >> i & 1 != 0)) {
                return false;
            }
            start = end;
        }
        true
    }
}

/// A multiply-and-rotate hasher for [`MatchKey`]: the key is hashed on every
/// indexed query, and the table is private to one world, so it needs no
/// protection against chosen keys. Fixed, so nothing depends on a random
/// seed.
#[derive(Default)]
pub(crate) struct KeyHasher(u64);

impl Hasher for KeyHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u32(u32::from(byte));
        }
    }
    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(value)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    #[inline]
    fn finish(&self) -> u64 {
        // The table reads both ends of the hash.
        let h = self.0 ^ (self.0 >> 32);
        h.wrapping_mul(0xBF58_476D_1CE4_E5B9)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Seen, not decided yet.
    Counting,
    /// `indices` lists every matching archetype below `generation`.
    Indexed,
    /// Scanned. Decided again once the world has `recheck` archetypes.
    Scanning { recheck: usize },
}

struct MatchEntry {
    state: State,
    /// Archetype slots are never removed: their count is a non-wrapping
    /// generation. Empty archetypes stay listed: later spawns can populate
    /// them again, and a reused slot is updated by [`MatchCache::reused`].
    generation: usize,
    /// Ascending, so candidates come in scan order.
    indices: Vec<u32>,
    uses: u32,
    /// The index's query count at the last use. Atomic because view
    /// queries, which share the world, record their use too.
    last_used: AtomicU64,
    /// A query asked for this signature with [`QueryCache::Always`].
    pinned: bool,
}

/// The index of one world. See the module docs.
pub(crate) struct MatchCache {
    policy: QueryCachePolicy,
    map: HashMap<MatchKey, MatchEntry, BuildHasherDefault<KeyHasher>>,
    /// The index's clock: it advances with every query that comes here
    /// holding the world exclusively (and every noted view signature that
    /// is settled), and once more with every sweep. Entries are stamped
    /// with it when used.
    clock: u64,
    /// `clock` at the last sweep.
    swept_at: u64,
    /// New signatures refused by a full table since the last sweep.
    misses: u32,
    /// The table refused a new signature and nothing was dropped since:
    /// view queries stop noting signatures it doesn't hold (one relaxed
    /// load each), which could only be refused again.
    full: AtomicBool,
    /// Times views were about to run on a full table. Every
    /// [`REOPEN_AFTER_ROUNDS`]th time they may note again, so a signature
    /// only views ask for still gets in once others go unused.
    full_rounds: u32,
    /// Signatures view queries scanned for and would like indexed, each
    /// with whether the query was hinted. Tried, never waited for.
    wanted: Mutex<Vec<(MatchKey, bool)>>,
    /// Whether `wanted` holds anything.
    pending: AtomicBool,
    /// Indices held over all entries.
    indices: usize,
    hits: u64,
    evictions: u64,
}

impl Default for MatchCache {
    fn default() -> Self {
        MatchCache {
            policy: QueryCachePolicy::new(),
            map: HashMap::default(),
            clock: 0,
            swept_at: 0,
            misses: 0,
            full: AtomicBool::new(false),
            full_rounds: 0,
            wanted: Mutex::new(Vec::new()),
            pending: AtomicBool::new(false),
            indices: 0,
            hits: 0,
            evictions: 0,
        }
    }
}

impl MatchCache {
    /// Fewest archetypes for a query to come here at all: the one test the
    /// query fast path makes.
    #[inline(always)]
    pub(crate) fn min_archetypes(&self) -> usize {
        self.policy.min_archetypes
    }

    /// The archetypes to visit for a query with signature `key`: the indexed
    /// candidates, or `scan` (the world's archetypes with entities, if it
    /// lists them), or all of them.
    #[inline]
    pub(crate) fn archetypes<'a>(
        &'a mut self,
        all: &'a [Archetype],
        key: &MatchKey,
        hint: QueryCache,
        scan: Option<&'a [u32]>,
    ) -> Archetypes<'a> {
        Archetypes {
            all,
            indices: self.candidates(all, key, hint).or(scan).map(<[u32]>::iter),
            next: 0,
        }
    }

    /// The archetypes to visit for a chunk query: those with every one of
    /// `ids` that pass `filter`, from the index when the policy says so.
    #[inline]
    pub(crate) fn chunk_archetypes<'a>(
        &'a mut self,
        all: &'a [Archetype],
        ids: &[ComponentId],
        filter: Option<&Filters>,
        hint: QueryCache,
        scan: Option<&'a [u32]>,
    ) -> Archetypes<'a> {
        if hint == QueryCache::Never || all.len() < self.min_archetypes() {
            return Archetypes {
                all,
                indices: scan.map(<[u32]>::iter),
                next: 0,
            };
        }
        let key = MatchKey::new(|require| ids.iter().for_each(|&id| require(id)), filter);
        self.archetypes(all, &key, hint, scan)
    }

    /// The indices of the archetypes `key` matches, if the policy indexes
    /// it; `None` to scan. Allocates only when it remembers a signature for
    /// the first time, builds an index, or lists archetypes created since.
    #[inline(never)]
    pub(crate) fn candidates(&mut self, all: &[Archetype], key: &MatchKey, hint: QueryCache) -> Option<&[u32]> {
        self.settle(all);
        self.lookup(all, key, hint)
    }

    fn lookup(&mut self, all: &[Archetype], key: &MatchKey, hint: QueryCache) -> Option<&[u32]> {
        let always = hint == QueryCache::Always;
        self.clock += 1;
        let clock = self.clock;
        let policy = self.policy;
        let budget = policy.max_index_bytes / size_of::<u32>();
        if self.map.capacity() == 0 {
            self.map.reserve(policy.max_signatures.min(RESERVE_SIGNATURES));
        }
        if self.map.len() >= policy.max_signatures && !self.map.contains_key(key) {
            // A new signature and no room: scan, and after enough of these
            // drop the signatures nobody used since the last sweep.
            self.misses += 1;
            *self.full.get_mut() = true;
            if self.misses >= SWEEP_AFTER_MISSES {
                self.sweep();
            }
            return None;
        }
        // One probe for a signature that is known, the usual case.
        let entry = self.map.entry(*key).or_insert_with(|| MatchEntry {
            state: State::Counting,
            generation: 0,
            indices: Vec::new(),
            uses: 0,
            last_used: AtomicU64::new(clock),
            pinned: false,
        });
        *entry.last_used.get_mut() = clock;
        entry.uses = entry.uses.saturating_add(1);
        let newly_pinned = always && !entry.pinned;
        entry.pinned |= always;
        let decide = match entry.state {
            State::Indexed => false,
            State::Counting => entry.pinned || entry.uses >= policy.promote_after,
            State::Scanning { recheck } => newly_pinned || all.len() >= recheck,
        };
        if decide {
            // Indexing pays when it leaves archetypes out; a hint overrides
            // that. Count first, so a signature that stays scanned allocates
            // nothing.
            let matching = all.iter().filter(|arch| key.matches(arch)).count();
            let sparse = matching.saturating_mul(100) <= all.len().saturating_mul(policy.max_match_percent as usize);
            if (entry.pinned || sparse) && self.indices + matching <= budget {
                entry.indices.reserve_exact(matching);
                entry.generation = 0;
                entry.state = State::Indexed;
            } else {
                entry.state = State::Scanning {
                    recheck: all.len().saturating_mul(2),
                };
            }
        }
        if entry.state != State::Indexed {
            return None;
        }
        // Only archetypes created since the last look need matching: the
        // older ones can't change what they hold, and registering a
        // component changes no archetype.
        if entry.generation < all.len() {
            let before = entry.indices.len();
            for (index, arch) in all.iter().enumerate().skip(entry.generation) {
                if key.matches(arch) {
                    entry.indices.push(index as u32);
                }
            }
            entry.generation = all.len();
            self.indices += entry.indices.len() - before;
            if self.indices > budget {
                // Over the memory bound: this signature scans from now on.
                self.indices -= entry.indices.len();
                entry.indices = Vec::new();
                entry.state = State::Scanning {
                    recheck: all.len().saturating_mul(2),
                };
                return None;
            }
        }
        self.hits += 1;
        Some(&entry.indices)
    }

    /// Slot `slot` of `all` holds a new archetype now, in place of a
    /// reclaimed one: every index list that has seen the slot gets it if
    /// the new archetype matches, and loses it if not, keeping its order.
    /// Allocates only when a list grows past its capacity.
    pub(crate) fn reused(&mut self, slot: usize, all: &[Archetype]) {
        let arch = &all[slot];
        let budget = self.policy.max_index_bytes / size_of::<u32>();
        for (key, entry) in &mut self.map {
            if entry.state != State::Indexed || slot >= entry.generation {
                continue;
            }
            match (key.matches(arch), entry.indices.binary_search(&(slot as u32))) {
                (true, Err(at)) => {
                    if self.indices >= budget {
                        // Over the memory bound: this signature scans from now on.
                        self.indices -= entry.indices.len();
                        entry.indices = Vec::new();
                        entry.state = State::Scanning {
                            recheck: all.len().saturating_mul(2),
                        };
                    } else {
                        entry.indices.insert(at, slot as u32);
                        self.indices += 1;
                    }
                }
                (false, Ok(at)) => {
                    entry.indices.remove(at);
                    self.indices -= 1;
                }
                _ => {}
            }
        }
    }

    /// The candidates of `key` for a query that holds the world shared (a
    /// view): only what is already indexed, and only if no archetype was
    /// created since. Reads the index without a lock, so any number of
    /// threads may ask at once. A signature that isn't usable yet, but
    /// could be, is noted for [`settle`](Self::settle).
    #[inline(never)]
    pub(crate) fn shared_candidates(&self, all: &[Archetype], key: &MatchKey, hint: QueryCache) -> Option<&[u32]> {
        let always = hint == QueryCache::Always;
        if let Some(entry) = self.map.get(key) {
            match entry.state {
                State::Indexed if entry.generation == all.len() => {
                    // Keeps the entry from being dropped as unused. A store
                    // only when the count moved, so threads that share an
                    // entry don't keep writing its cache line.
                    if entry.last_used.load(Ordering::Relaxed) != self.clock {
                        entry.last_used.store(self.clock, Ordering::Relaxed);
                    }
                    return Some(&entry.indices);
                }
                // Decided: scanned until the world has grown, or a hint asks.
                State::Scanning { recheck } if all.len() < recheck && !(always && !entry.pinned) => return None,
                _ => {}
            }
        } else if self.full.load(Ordering::Relaxed) {
            // No room for a new signature: asking would only be refused.
            return None;
        }
        self.want(key, always);
        None
    }

    /// Notes that a view query scanned for `key`. Never waits: with the
    /// list busy or full, the next query of this signature asks again.
    #[cold]
    fn want(&self, key: &MatchKey, always: bool) {
        let Ok(mut wanted) = self.wanted.try_lock() else {
            return;
        };
        if wanted.len() < MAX_WANTED && !wanted.contains(&(*key, always)) {
            if wanted.capacity() == 0 {
                wanted.reserve_exact(MAX_WANTED);
            }
            wanted.push((*key, always));
            self.pending.store(true, Ordering::Relaxed);
        }
    }

    /// Counts what view queries asked for since the last call as uses, as
    /// if exclusive queries had run them: signatures get indexed, and
    /// indices learn about new archetypes. Called whenever the world is
    /// held exclusively and the index is about to be used.
    #[inline]
    pub(crate) fn settle(&mut self, all: &[Archetype]) {
        if *self.pending.get_mut() {
            self.settle_wanted(all);
        }
    }

    #[cold]
    #[inline(never)]
    fn settle_wanted(&mut self, all: &[Archetype]) {
        *self.pending.get_mut() = false;
        let mut wanted = std::mem::take(self.wanted.get_mut().unwrap_or_else(|poison| poison.into_inner()));
        if all.len() >= self.min_archetypes() {
            for (key, always) in &wanted {
                let hint = if *always { QueryCache::Always } else { QueryCache::Auto };
                if self.lookup(all, key, hint).is_some() {
                    // Counted as a use, but no query was answered.
                    self.hits -= 1;
                }
            }
        }
        // Keep the list's storage.
        wanted.clear();
        *self.wanted.get_mut().unwrap_or_else(|poison| poison.into_inner()) = wanted;
    }

    /// [`settle`](Self::settle) for views that are about to run. On a
    /// full table views don't note new signatures; every
    /// [`REOPEN_AFTER_ROUNDS`]th call lets them, for one round, so the
    /// misses that lead to a sweep keep coming (slowly) in a world that
    /// only views query.
    #[inline]
    pub(crate) fn settle_for_views(&mut self, all: &[Archetype]) {
        self.settle(all);
        if *self.full.get_mut() {
            self.reopen();
        }
    }

    #[cold]
    fn reopen(&mut self) {
        self.full_rounds += 1;
        if self.full_rounds >= REOPEN_AFTER_ROUNDS {
            self.full_rounds = 0;
            *self.full.get_mut() = false;
        }
    }

    /// Drops the signatures not used since the last sweep.
    fn sweep(&mut self) {
        let swept_at = self.swept_at;
        let (mut dropped, mut freed) = (0, 0);
        self.map.retain(|_, entry| {
            let keep = *entry.last_used.get_mut() > swept_at;
            if !keep {
                dropped += 1;
                freed += entry.indices.len();
            }
            keep
        });
        self.evictions += dropped;
        self.indices -= freed;
        self.swept_at = self.clock;
        // A view stamps an entry with the clock as it is, without advancing
        // it: move on, so a use right after this sweep counts as after it.
        self.clock += 1;
        self.misses = 0;
        if dropped > 0 {
            *self.full.get_mut() = false;
            self.full_rounds = 0;
        }
    }

    fn set_policy(&mut self, policy: QueryCachePolicy) {
        // Start over: whatever the old policy decided may not hold.
        let policy = QueryCachePolicy {
            max_match_percent: policy.max_match_percent.min(100),
            ..policy
        };
        *self = MatchCache {
            policy,
            hits: self.hits,
            evictions: self.evictions,
            ..MatchCache::default()
        };
    }

    fn stats(&self) -> QueryCacheStats {
        let table = self.map.capacity() * (size_of::<MatchKey>() + size_of::<MatchEntry>() + 1);
        let lists: usize = self.map.values().map(|e| e.indices.capacity() * size_of::<u32>()).sum();
        QueryCacheStats {
            signatures: self.map.len(),
            indexed: self.map.values().filter(|e| e.state == State::Indexed).count(),
            indices: self.indices,
            bytes: table + lists,
            hits: self.hits,
            evictions: self.evictions,
        }
    }
}

impl World {
    /// Sets when this world indexes the archetypes of repeated queries, and
    /// forgets what was indexed so far. [`QueryCachePolicy::off`] turns the
    /// index off for the whole world.
    ///
    /// The policy never changes what a query returns, in what order, or what
    /// counts as changed.
    ///
    /// ```
    /// use ponko_ecs::{QueryCachePolicy, World};
    ///
    /// let mut world = World::new();
    /// world.set_query_cache_policy(QueryCachePolicy::off());
    /// assert!(world.query_cache_policy().is_off());
    /// assert_eq!(world.query_cache_stats().bytes, 0);
    /// ```
    pub fn set_query_cache_policy(&mut self, policy: QueryCachePolicy) {
        self.query_matches.set_policy(policy);
    }

    /// The policy set with
    /// [`set_query_cache_policy`](Self::set_query_cache_policy), or the
    /// default one.
    #[must_use]
    pub fn query_cache_policy(&self) -> QueryCachePolicy {
        self.query_matches.policy
    }

    /// What the archetype index holds now: how many signatures, how many
    /// bytes, and how often it was used.
    ///
    /// ```
    /// use ponko_ecs::World;
    ///
    /// let world = World::new();
    /// let stats = world.query_cache_stats();
    /// assert_eq!((stats.signatures, stats.bytes, stats.hits), (0, 0, 0));
    /// ```
    #[must_use]
    pub fn query_cache_stats(&self) -> QueryCacheStats {
        self.query_matches.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;

    #[derive(Component)]
    struct Marker<const N: usize>;

    /// Archetypes for every subset of component ids `0..bits`, as `World`
    /// stores them: archetype `i` has component `c` when bit `c` of `i` is set.
    fn subsets(bits: u32) -> Vec<Archetype> {
        let mut world = World::new();
        let ids = [
            world.register::<Marker<0>>(),
            world.register::<Marker<1>>(),
            world.register::<Marker<2>>(),
            world.register::<Marker<3>>(),
            world.register::<Marker<4>>(),
            world.register::<Marker<5>>(),
        ];
        assert!(ids.iter().enumerate().all(|(i, id)| id.index() == i));
        (0..1u32 << bits)
            .map(|set| {
                let ids: Vec<ComponentId> = (0..bits).filter(|i| set >> i & 1 != 0).map(ComponentId).collect();
                Archetype::new(ids.into(), &world.infos)
            })
            .collect()
    }

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u32) -> u32 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) % u64::from(n)) as u32
        }
    }

    /// Random filters over component ids `0..bits`: `with`, `without`, and
    /// `Or` clauses with repeated, contradicting and redundant literals.
    fn random_filters(rng: &mut Rng, bits: u32) -> Filters {
        let mut filters = Filters::default();
        for _ in 0..rng.below(3) {
            filters.add_with(ComponentId(rng.below(bits)));
        }
        for _ in 0..rng.below(3) {
            filters.add_without(ComponentId(rng.below(bits)));
        }
        for _ in 0..rng.below(MAX_OR as u32 + 1) {
            for _ in 0..rng.below(5) {
                filters.add_or_term(ComponentId(rng.below(bits)), rng.below(2) == 0);
            }
            filters.close_or();
        }
        filters
    }

    #[test]
    fn a_key_matches_exactly_what_its_query_matches() {
        const BITS: u32 = 6;
        let archetypes = subsets(BITS);
        let mut rng = Rng(139);
        let cases = if cfg!(miri) { 40 } else { 20_000 };
        for case in 0..cases {
            let filters = random_filters(&mut rng, BITS);
            let required: Vec<ComponentId> = (0..rng.below(3)).map(|_| ComponentId(rng.below(BITS))).collect();
            let key = MatchKey::new(|f| required.iter().for_each(|&id| f(id)), Some(&filters));
            for arch in &archetypes {
                let direct = filters.matches(arch) && required.iter().all(|&id| arch.has(id));
                assert_eq!(
                    key.matches(arch),
                    direct,
                    "case {case}, archetype {:?}",
                    arch.component_ids()
                );
            }
        }
    }

    #[test]
    fn keys_that_match_the_same_archetypes_are_usually_equal_and_equal_keys_always_match_the_same() {
        const BITS: u32 = 4;
        let archetypes = subsets(BITS);
        let mut rng = Rng(7);
        let mut by_key: HashMap<MatchKey, Vec<bool>, BuildHasherDefault<KeyHasher>> = HashMap::default();
        let cases = if cfg!(miri) { 60 } else { 30_000 };
        for _ in 0..cases {
            let filters = random_filters(&mut rng, BITS);
            let key = MatchKey::new(|_| {}, Some(&filters));
            let set: Vec<bool> = archetypes.iter().map(|arch| filters.matches(arch)).collect();
            // Soundness: one key, one set of archetypes.
            assert_eq!(by_key.entry(key).or_insert_with(|| set.clone()), &set);
        }
        // Sharing: far fewer keys than filters were tried. (Not one key per
        // set: the normal form is cheap, not minimal.) Miri's few cases
        // are too few to repeat keys.
        if !cfg!(miri) {
            assert!(by_key.len() * 4 < cases, "{} keys", by_key.len());
        }
    }

    fn key(with: &[u32], without: &[u32], clauses: &[&[(u32, bool)]], required: &[u32]) -> MatchKey {
        let mut filters = Filters::default();
        with.iter().for_each(|&id| filters.add_with(ComponentId(id)));
        without.iter().for_each(|&id| filters.add_without(ComponentId(id)));
        for clause in clauses {
            clause
                .iter()
                .for_each(|&(id, has)| filters.add_or_term(ComponentId(id), has));
            filters.close_or();
        }
        MatchKey::new(|f| required.iter().for_each(|&id| f(ComponentId(id))), Some(&filters))
    }

    #[test]
    fn equivalent_filters_share_a_key() {
        // Order of `with`/`without`, and fetched against filtered.
        assert!(key(&[1, 2], &[4, 3], &[], &[0]) == key(&[2], &[3, 4], &[], &[1, 0]));
        assert!(key(&[1], &[], &[], &[1]) == key(&[], &[], &[], &[1, 1]));
        // Literal and clause order, repeated literals and clauses.
        let a = key(&[], &[], &[&[(1, true), (2, false)], &[(3, true), (4, true)]], &[]);
        let b = key(
            &[],
            &[],
            &[
                &[(4, true), (3, true), (3, true)],
                &[(2, false), (1, true)],
                &[(1, true), (2, false)],
            ],
            &[],
        );
        assert!(a == b);
        // A one-arm `Or` is a `with` / `without`.
        assert!(key(&[], &[], &[&[(1, true)], &[(2, false)]], &[]) == key(&[1], &[2], &[], &[]));
        // A clause that always holds says nothing.
        assert!(key(&[], &[], &[&[(1, true), (1, false)]], &[]) == key(&[], &[], &[], &[]));
        assert!(key(&[1], &[], &[&[(1, true), (2, true)]], &[]) == key(&[1], &[], &[], &[]));
        assert!(key(&[], &[], &[&[(1, true), (2, true)]], &[1]) == key(&[], &[], &[], &[1]));
        // A literal that can't hold drops out; what is left may be a `with`.
        assert!(key(&[], &[1], &[&[(1, true), (2, true)]], &[]) == key(&[2], &[1], &[], &[]));
        // A clause implied by a narrower one drops out.
        let narrow = key(&[], &[], &[&[(1, true), (2, true)]], &[]);
        assert!(
            key(
                &[],
                &[],
                &[&[(1, true), (2, true), (3, false)], &[(2, true), (1, true)]],
                &[]
            ) == narrow
        );
        // Different polarity, different key.
        assert!(key(&[], &[], &[&[(1, true), (2, false)]], &[]) != narrow);
    }

    #[test]
    fn filters_that_match_nothing_share_the_never_key() {
        let never = key(&[], &[], &[&[]], &[]);
        assert!(never.never);
        assert!(key(&[1], &[1], &[], &[]) == never);
        assert!(key(&[], &[1], &[], &[1]) == never);
        assert!(key(&[2], &[1], &[&[(1, true), (2, false)]], &[]) == never);
        assert!(subsets(3).iter().all(|arch| !never.matches(arch)));
    }

    fn policy(min_archetypes: usize) -> QueryCachePolicy {
        QueryCachePolicy {
            min_archetypes,
            ..QueryCachePolicy::new()
        }
    }

    fn with(id: u32) -> MatchKey {
        key(&[id], &[], &[], &[])
    }

    #[test]
    fn a_signature_is_indexed_on_its_second_use_and_only_if_it_leaves_archetypes_out() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(policy(1));
        // `with 0 and 1`: 4 of 16 archetypes.
        let sparse = key(&[0, 1], &[], &[], &[]);
        assert!(cache.candidates(&archetypes, &sparse, QueryCache::Auto).is_none());
        let indexed = cache
            .candidates(&archetypes, &sparse, QueryCache::Auto)
            .unwrap()
            .to_vec();
        assert_eq!(indexed, [3, 7, 11, 15]);
        // `without 0 and 1` matches 12 of 16: scanned, whatever its uses.
        let dense = key(&[], &[], &[&[(0, false), (1, false)]], &[]);
        for _ in 0..5 {
            assert!(cache.candidates(&archetypes, &dense, QueryCache::Auto).is_none());
        }
        // A hint indexes it at once, and it stays indexed for everyone.
        assert_eq!(
            cache.candidates(&archetypes, &dense, QueryCache::Always).unwrap().len(),
            12
        );
        assert_eq!(
            cache.candidates(&archetypes, &dense, QueryCache::Auto).unwrap().len(),
            12
        );
        let stats = cache.stats();
        assert_eq!((stats.signatures, stats.indexed, stats.indices), (2, 2, 16));
        assert_eq!(stats.hits, 3);
        assert!(stats.bytes >= 16 * 4);
    }

    #[test]
    fn a_dense_signature_is_decided_again_when_the_archetypes_double() {
        let archetypes = subsets(5);
        let mut cache = MatchCache::default();
        cache.set_policy(policy(1));
        // Among the first 2 archetypes ({} and {0}), `without 4` matches all.
        let k = key(&[], &[4], &[], &[]);
        assert!(cache.candidates(&archetypes[..2], &k, QueryCache::Auto).is_none());
        assert!(cache.candidates(&archetypes[..2], &k, QueryCache::Auto).is_none());
        assert!(cache.candidates(&archetypes[..3], &k, QueryCache::Auto).is_none());
        // Of all 32 it matches 16: half, so it is indexed now.
        assert_eq!(cache.candidates(&archetypes, &k, QueryCache::Auto).unwrap().len(), 16);
    }

    #[test]
    fn an_index_follows_new_archetypes_in_order() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(policy(1));
        let k = with(3);
        assert_eq!(cache.candidates(&archetypes[..9], &k, QueryCache::Always).unwrap(), [8]);
        assert_eq!(
            cache.candidates(&archetypes[..12], &k, QueryCache::Auto).unwrap(),
            [8, 9, 10, 11]
        );
        assert_eq!(
            cache
                .shared_candidates(&archetypes[..12], &k, QueryCache::Auto)
                .unwrap(),
            [8, 9, 10, 11]
        );
        // A shared lookup never uses a list that hasn't seen every archetype.
        assert!(cache.shared_candidates(&archetypes, &k, QueryCache::Auto).is_none());
        assert!(
            cache
                .shared_candidates(&archetypes, &with(2), QueryCache::Auto)
                .is_none()
        );
        assert_eq!(cache.candidates(&archetypes, &k, QueryCache::Auto).unwrap().len(), 8);
        assert_eq!(cache.stats().indices, 8);
    }

    #[test]
    fn a_full_table_scans_new_signatures_then_drops_the_unused_ones() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(QueryCachePolicy {
            max_signatures: 4,
            ..policy(1)
        });
        for id in 0..4 {
            assert!(cache.candidates(&archetypes, &with(id), QueryCache::Always).is_some());
        }
        // Full: a fifth signature scans, hinted or not, and nothing is dropped
        // while the others are in use.
        for _ in 0..3 * SWEEP_AFTER_MISSES {
            assert!(cache.candidates(&archetypes, &with(9), QueryCache::Always).is_none());
            for id in 0..4 {
                assert!(cache.candidates(&archetypes, &with(id), QueryCache::Auto).is_some());
            }
        }
        assert_eq!((cache.stats().signatures, cache.stats().evictions), (4, 0));
        // Signatures 2 and 3 go unused: after two sweeps they are gone and
        // the new one has room.
        let mut got_in = false;
        for _ in 0..3 * SWEEP_AFTER_MISSES {
            got_in = cache.candidates(&archetypes, &with(9), QueryCache::Always).is_some();
            for id in 0..2 {
                assert!(cache.candidates(&archetypes, &with(id), QueryCache::Auto).is_some());
            }
        }
        assert!(got_in);
        let stats = cache.stats();
        assert_eq!((stats.signatures, stats.evictions), (3, 2));
        assert_eq!(stats.indices, 8 + 8);
        assert!(
            cache
                .shared_candidates(&archetypes, &with(3), QueryCache::Auto)
                .is_none()
        );
    }

    #[test]
    fn the_memory_bound_makes_signatures_scan() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        // Room for 10 indices.
        cache.set_policy(QueryCachePolicy {
            max_index_bytes: 40,
            ..policy(1)
        });
        assert_eq!(
            cache
                .candidates(&archetypes, &with(0), QueryCache::Always)
                .unwrap()
                .len(),
            8
        );
        // 8 more don't fit: scanned, even with a hint, and again next time.
        assert!(cache.candidates(&archetypes, &with(1), QueryCache::Always).is_none());
        assert!(cache.candidates(&archetypes, &with(1), QueryCache::Always).is_none());
        // 2 fit.
        assert_eq!(
            cache
                .candidates(&archetypes, &key(&[1, 2, 3], &[], &[], &[]), QueryCache::Always)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(cache.stats().indices, 10);
        // An index that outgrows the bound through new archetypes is given up.
        let mut cache = MatchCache::default();
        cache.set_policy(QueryCachePolicy {
            max_index_bytes: 16,
            ..policy(1)
        });
        assert_eq!(
            cache
                .candidates(&archetypes[..8], &with(0), QueryCache::Always)
                .unwrap()
                .len(),
            4
        );
        assert!(cache.candidates(&archetypes, &with(0), QueryCache::Always).is_none());
        assert_eq!((cache.stats().indices, cache.stats().indexed), (0, 0));
    }

    #[test]
    fn a_shared_lookup_reads_the_index_and_its_demand_is_settled_later() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(policy(1));
        let k = key(&[0, 1], &[], &[], &[]);
        // Unknown: scanned, and asked for (once, however often it runs).
        for _ in 0..3 {
            assert!(cache.shared_candidates(&archetypes, &k, QueryCache::Auto).is_none());
        }
        assert_eq!(cache.stats().signatures, 0, "a shared lookup changes nothing");
        cache.settle(&archetypes);
        assert_eq!((cache.stats().signatures, cache.stats().indexed), (1, 0));
        // The second use indexes it, as for exclusive queries.
        assert!(cache.shared_candidates(&archetypes, &k, QueryCache::Auto).is_none());
        cache.settle(&archetypes);
        assert_eq!(
            cache.shared_candidates(&archetypes, &k, QueryCache::Auto).unwrap(),
            [3, 7, 11, 15]
        );
        assert_eq!((cache.stats().indexed, cache.stats().hits), (1, 0));
        // Nothing left to settle: a usable entry asks for nothing.
        assert!(!*cache.pending.get_mut());
        // A dense signature is decided once, then scanned without asking.
        let dense = key(&[], &[], &[&[(0, false), (1, false)]], &[]);
        for _ in 0..2 {
            assert!(cache.shared_candidates(&archetypes, &dense, QueryCache::Auto).is_none());
            cache.settle(&archetypes);
        }
        assert!(cache.shared_candidates(&archetypes, &dense, QueryCache::Auto).is_none());
        assert!(!*cache.pending.get_mut());
        // A hint asks again, once, and gets it indexed.
        assert!(
            cache
                .shared_candidates(&archetypes, &dense, QueryCache::Always)
                .is_none()
        );
        cache.settle(&archetypes);
        assert_eq!(
            cache
                .shared_candidates(&archetypes, &dense, QueryCache::Auto)
                .unwrap()
                .len(),
            12
        );
        // New archetypes: the list is stale for shared lookups until settled.
        let mut cache = MatchCache::default();
        cache.set_policy(policy(1));
        assert!(cache.candidates(&archetypes[..8], &k, QueryCache::Always).is_some());
        assert_eq!(
            cache.shared_candidates(&archetypes[..8], &k, QueryCache::Auto).unwrap(),
            [3, 7]
        );
        assert!(cache.shared_candidates(&archetypes, &k, QueryCache::Auto).is_none());
        cache.settle(&archetypes);
        assert_eq!(
            cache.shared_candidates(&archetypes, &k, QueryCache::Auto).unwrap(),
            [3, 7, 11, 15]
        );
        assert_eq!(cache.stats().hits, 1, "only the exclusive query counted");
    }

    #[test]
    fn shared_use_keeps_a_signature_from_being_dropped() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(QueryCachePolicy {
            max_signatures: 2,
            ..policy(1)
        });
        assert!(cache.candidates(&archetypes, &with(0), QueryCache::Always).is_some());
        assert!(cache.candidates(&archetypes, &with(1), QueryCache::Always).is_some());
        // `with(0)` is only used by shared lookups from here on; `with(1)`
        // not at all. Exclusive queries of new signatures keep missing.
        for _ in 0..4 * SWEEP_AFTER_MISSES {
            assert!(
                cache
                    .shared_candidates(&archetypes, &with(0), QueryCache::Auto)
                    .is_some()
            );
            cache.candidates(&archetypes, &with(2), QueryCache::Auto);
        }
        assert!(
            cache
                .shared_candidates(&archetypes, &with(0), QueryCache::Auto)
                .is_some()
        );
        assert!(
            cache
                .shared_candidates(&archetypes, &with(1), QueryCache::Auto)
                .is_none()
        );
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn a_signature_only_views_use_survives_every_sweep() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(QueryCachePolicy {
            max_signatures: 2,
            ..policy(1)
        });
        assert!(cache.candidates(&archetypes, &with(0), QueryCache::Always).is_some());
        assert!(cache.candidates(&archetypes, &with(1), QueryCache::Always).is_some());
        // A frame: views use `with(0)`, then exclusive queries use
        // `with(1)` and miss with a third signature just often enough to
        // sweep. The sweep is the last thing before the views run again,
        // so their stamp is the first use after it.
        for frame in 0..6 {
            cache.settle_for_views(&archetypes);
            for _ in 0..3 {
                let seen = cache.shared_candidates(&archetypes, &with(0), QueryCache::Auto);
                assert_eq!(seen.map(<[u32]>::len), Some(8), "frame {frame}");
            }
            assert!(cache.candidates(&archetypes, &with(1), QueryCache::Auto).is_some());
            for _ in 0..SWEEP_AFTER_MISSES {
                assert!(cache.candidates(&archetypes, &with(2), QueryCache::Auto).is_none());
            }
            assert_eq!(cache.misses, 0, "frame {frame}: the frame ended with a sweep");
        }
        let stats = cache.stats();
        assert_eq!((stats.signatures, stats.indexed, stats.evictions), (2, 2, 0));
    }

    #[test]
    fn views_stop_asking_a_full_table_until_it_has_room() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(QueryCachePolicy {
            max_signatures: 1,
            ..policy(1)
        });
        assert!(cache.candidates(&archetypes, &with(0), QueryCache::Always).is_some());
        // A view asks for a second signature once; the table refuses it.
        assert!(
            cache
                .shared_candidates(&archetypes, &with(1), QueryCache::Auto)
                .is_none()
        );
        assert!(*cache.pending.get_mut());
        cache.settle_for_views(&archetypes);
        assert_eq!((cache.misses, cache.stats().signatures), (1, 1));
        // From here on views don't ask: nothing is noted and no miss is
        // counted, however many queries and rounds.
        for _ in 0..REOPEN_AFTER_ROUNDS - 2 {
            for _ in 0..5 {
                assert!(
                    cache
                        .shared_candidates(&archetypes, &with(1), QueryCache::Always)
                        .is_none()
                );
            }
            assert!(!*cache.pending.get_mut());
            assert!(cache.wanted.get_mut().unwrap().is_empty());
            cache.settle_for_views(&archetypes);
            assert_eq!(cache.misses, 1);
        }
        // A signature the table holds is still served, and still asked for
        // when it falls behind.
        assert!(
            cache
                .shared_candidates(&archetypes, &with(0), QueryCache::Auto)
                .is_some()
        );
        // Miri: the thousands of rounds below add no code path.
        if cfg!(miri) {
            return;
        }
        // Every so often one round may ask again, so the misses that lead
        // to a sweep keep coming: `with(0)` goes unused, is dropped, and
        // the signature the views want gets in.
        let mut rounds = 0;
        while cache
            .shared_candidates(&archetypes, &with(1), QueryCache::Auto)
            .is_none()
        {
            cache.settle_for_views(&archetypes);
            rounds += 1;
            // Two sweeps: the first one still sees the use above.
            assert!(
                rounds <= REOPEN_AFTER_ROUNDS * (2 * SWEEP_AFTER_MISSES + 4),
                "never got in"
            );
        }
        let stats = cache.stats();
        assert_eq!((stats.signatures, stats.indexed, stats.evictions), (1, 1, 1));
        assert!(cache.misses <= 1, "asked at most once a round: {}", cache.misses);
    }

    #[test]
    fn a_policy_matches_at_most_every_archetype() {
        let mut cache = MatchCache::default();
        cache.set_policy(QueryCachePolicy {
            max_match_percent: 250,
            ..policy(1)
        });
        assert_eq!(cache.policy.max_match_percent, 100);
        // 100 percent: a signature that matches everything is indexed.
        let archetypes = subsets(3);
        let everything = key(&[], &[], &[], &[]);
        assert!(cache.candidates(&archetypes, &everything, QueryCache::Auto).is_none());
        assert_eq!(
            cache
                .candidates(&archetypes, &everything, QueryCache::Auto)
                .map(<[u32]>::len),
            Some(8)
        );
    }

    #[test]
    fn many_threads_read_the_index_at_once() {
        let archetypes = subsets(5);
        let mut cache = MatchCache::default();
        cache.set_policy(policy(1));
        for id in 0..3 {
            assert!(cache.candidates(&archetypes, &with(id), QueryCache::Always).is_some());
        }
        let rounds = if cfg!(miri) { 20 } else { 2000 };
        for _ in 0..2 {
            let cache = &cache;
            let archetypes = &archetypes;
            std::thread::scope(|scope| {
                for thread in 0..4u32 {
                    scope.spawn(move || {
                        for round in 0..rounds {
                            let id = (thread + round) % 5;
                            let k = with(id);
                            let found = cache.shared_candidates(archetypes, &k, QueryCache::Auto);
                            if let Some(indices) = found {
                                // Exactly the archetypes with the component, in order.
                                let expected: Vec<u32> = (0..32).filter(|a| a >> id & 1 != 0).collect();
                                assert_eq!(indices, expected);
                            } else {
                                assert!(id >= 3, "an indexed signature is always usable");
                            }
                        }
                    });
                }
            });
        }
        // What the threads asked for is indexed at the next exclusive moment.
        cache.settle(&archetypes);
        cache.shared_candidates(&archetypes, &with(3), QueryCache::Auto);
        cache.shared_candidates(&archetypes, &with(4), QueryCache::Auto);
        cache.settle(&archetypes);
        assert_eq!(cache.stats().indexed, 5);
    }

    #[test]
    fn changing_the_policy_forgets_the_index() {
        let archetypes = subsets(4);
        let mut cache = MatchCache::default();
        cache.set_policy(policy(1));
        assert!(cache.candidates(&archetypes, &with(0), QueryCache::Always).is_some());
        cache.set_policy(QueryCachePolicy::off());
        let stats = cache.stats();
        assert_eq!((stats.signatures, stats.indices, stats.bytes), (0, 0, 0));
        assert_eq!(cache.min_archetypes(), usize::MAX);
    }
}
