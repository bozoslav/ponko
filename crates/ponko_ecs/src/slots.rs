//! Bookkeeping of the world's archetype slots that lives next to the
//! archetypes rather than in them: which archetypes hold entities, which
//! are empty and since when, and the storage of reclaimed archetypes kept
//! for reuse.
//!
//! **Skipping empty archetypes (issue #348).** A world whose components
//! come and go keeps many empty archetypes, and a query that scans used to
//! read every archetype's 128-byte struct to find the ones with entities.
//! In such a world (more than one empty archetype per four with entities,
//! and more than a few)
//! a scanning query visits the list of archetypes with entities instead,
//! built from the occupancy bits when it is first needed after a change.
//! The query loop itself is unchanged: the list is handed to it as its
//! candidates, as the archetype index's lists are. Testing a bit in that
//! loop instead, for every archetype, was measured: it slowed scans of
//! worlds without empty archetypes by about 15 % and changed how `Query::iter`
//! compiles (`crates/ponko_bench/SPARSE_SET.md`).
//!
//! **Reclaiming them (issue #347).** Such a world would also keep an
//! archetype, with its buffers, for every combination it ever had. The
//! policy: a world keeps at most [`KEEP_EMPTY`] empty archetypes, or as
//! many as it has archetypes with entities if that is more (pinned ones,
//! below, not counted). Past that, a sweep over the slots reclaims empty
//! archetypes, each the second time it finds it empty (a "second chance"
//! clock): roughly, those empty longest go first. A combination that
//! empties and fills again soon is marked anew each time it empties and
//! keeps its archetype, so it doesn't thrash, and the number of archetypes
//! levels off at about twice the number with entities. Emptying or filling
//! an archetype only flips bits; the sweep runs while the world is over
//! the policy. (A queue of empty archetypes, oldest first, did the same
//! but cost about 3 ns on every move that empties an archetype, measured
//! with `membership/transition`.)
//!
//! A reclaimed archetype's slot stays in `World::archetypes` with no
//! components, rows or edges ("dead"), free for the next new component
//! set. Its id lists, column headers and edge maps wait in a [`Shell`],
//! kept by width for the next archetype as wide, and its column buffers
//! and entity list in the world's [`BufferPool`], which also takes the
//! buffers archetypes outgrow and hands them to archetypes that grow. A
//! full edge map grows straight to the most edges it can hold when that is
//! small (`World::add_edge`). So a world in a steady state of churn builds
//! its new archetypes without allocating once it has needed each width and
//! size it needs; what still allocates is an archetype outgrowing any
//! buffer the world has, as without reclaiming. Archetype indices
//! therefore name an archetype only until the next structural change, and
//! what keeps them across changes is kept right:
//!
//! - Entity locations only name archetypes with entities, never an empty
//!   (reclaimable) one.
//! - A transition edge names the slot it leads to and that slot's
//!   generation, which reclaiming moves on: an edge to a reclaimed
//!   archetype is stale, is skipped like a missing one, and is replaced
//!   when the move is next made. Reclaiming touches no neighbour: removing
//!   the edges from them eagerly, in maps scattered over the world, made
//!   the 20 % churn benchmark measurably slower
//!   (`crates/ponko_bench/SPARSE_SET.md`). A slot whose
//!   generations run out is never reused. A reused archetype starts with
//!   empty edge maps.
//! - The query index keeps lists of slot indices. A reclaimed slot can stay
//!   in a list (it is empty, and loops skip empty archetypes); when the
//!   slot is reused for another component set, the world tells the index,
//!   which adds the slot to or removes it from every list for that set
//!   (`MatchCache::reused`). Slots are never removed, so the slot count the
//!   index uses as its generation still only grows.
//! - The list of archetypes with entities is rebuilt after any change.
//! - Archetypes named by bundle types (the spawn targets the world caches
//!   per Rust bundle type) and archetype 0 (no components) are pinned:
//!   never reclaimed.
//! - Change detection keeps its stamps in the column buffers and its
//!   removal logs by entity, so it names no archetype.

use crate::storage::{BufferPool, Edge, Shell};

/// Empty archetypes a world keeps whatever its size; see the module docs.
pub(crate) const KEEP_EMPTY: usize = 64;
/// Empty archetypes a scan reads (their lengths) rather than keeping a list
/// of the others: for small worlds that change often.
const SCAN_LIST_AFTER: usize = 16;

/// Per-slot state of a world's archetypes. Indexed like `World::archetypes`;
/// the bitsets hold slot `a` in word `a / 64`, bit `a % 64`.
#[derive(Default)]
pub(crate) struct Slots {
    /// Slots in the world.
    len: usize,
    /// Set exactly when archetype `a` holds at least one entity. Every
    /// change of an archetype's length between zero and not zero goes
    /// through [`filled`](Self::filled) or [`emptied`](Self::emptied).
    occupied: Vec<u64>,
    /// Set when archetype `a` became empty (or was created) since the
    /// reclaiming sweep last passed it: its second chance.
    recent: Vec<u64>,
    /// Never reclaimed.
    pinned: Vec<u64>,
    /// Reclaimed: no components, free for reuse (or, out of generations,
    /// never used again).
    dead: Vec<u64>,
    /// Each slot's generation: how often an archetype in it was reclaimed.
    /// Edges carry the generation of the slot they lead to.
    generations: Vec<u32>,
    /// Archetypes with entities.
    live: usize,
    pinned_count: usize,
    dead_count: usize,
    /// The word the reclaiming sweep goes on from.
    hand: usize,
    /// The archetypes with entities, ascending, while `listed`.
    list: Vec<u32>,
    listed: bool,
    /// Dead slots free for reuse.
    free: Vec<u32>,
    /// Storage of reclaimed archetypes, by width (component count): at most
    /// as many more than there are dead slots as the world may keep empty
    /// archetypes. A new archetype takes one as wide as it, so the widths
    /// kept follow the widths the world builds, and a churning world stops
    /// allocating once it has built its widest mix.
    shells: Vec<Vec<Shell>>,
    shell_count: usize,
    /// Column buffers and entity lists of reclaimed archetypes, and the ones
    /// archetypes outgrew.
    pub(crate) buffers: BufferPool,
}

impl Slots {
    /// Archetype `a` was just created, empty: in a new slot (the next
    /// index), or in a dead one.
    pub(crate) fn created(&mut self, a: usize) {
        if a == self.len {
            self.len += 1;
            self.generations.push(0);
            if a / 64 >= self.occupied.len() {
                for bits in [&mut self.occupied, &mut self.recent, &mut self.pinned, &mut self.dead] {
                    bits.push(0);
                }
            }
        } else if take_bit(&mut self.dead, a) {
            self.dead_count -= 1;
        }
        set_bit(&mut self.recent, a);
    }

    /// Archetype `a` is never reclaimed.
    pub(crate) fn pin(&mut self, a: usize) {
        if !is_set(&self.pinned, a) {
            set_bit(&mut self.pinned, a);
            self.pinned_count += 1;
        }
    }

    /// Archetype `a` went from no entities to one.
    #[inline]
    pub(crate) fn filled(&mut self, a: usize) {
        self.occupied[a / 64] |= 1 << (a % 64);
        self.live += 1;
        self.listed = false;
    }

    /// Archetype `a` lost its last entity. The world then reclaims empty
    /// archetypes while [`over`](Self::over) says so.
    #[inline]
    pub(crate) fn emptied(&mut self, a: usize) {
        self.occupied[a / 64] &= !(1 << (a % 64));
        self.recent[a / 64] |= 1 << (a % 64);
        self.live -= 1;
        self.listed = false;
    }

    /// Whether the world keeps more empty archetypes than the policy allows.
    /// Pinned archetypes count as if they all had entities, so this is
    /// `true` only while there are empty archetypes the sweep may reclaim.
    #[inline]
    pub(crate) fn over(&self) -> bool {
        let empty = self.len - self.live - self.dead_count;
        empty.saturating_sub(self.pinned_count) > KEEP_EMPTY.max(self.live)
    }

    /// An empty archetype to reclaim: the sweep goes on over the slots,
    /// taking the first empty, unpinned, live archetype that didn't become
    /// empty since the sweep last passed it, and clearing that mark on the
    /// ones it passes. So an archetype is reclaimed the second time the
    /// sweep finds it empty: roughly, those empty longest go first. Call
    /// only while [`over`](Self::over).
    pub(crate) fn next_empty(&mut self) -> Option<usize> {
        let words = self.occupied.len();
        // Two rounds clear every mark, so a third finds a candidate if any.
        for _ in 0..=2 * words {
            let w = self.hand;
            let mut candidates = !(self.occupied[w] | self.pinned[w] | self.dead[w]);
            if w == words - 1 && !self.len.is_multiple_of(64) {
                candidates &= (1 << (self.len % 64)) - 1;
            }
            let ready = candidates & !self.recent[w];
            if ready != 0 {
                return Some(w * 64 + ready.trailing_zeros() as usize);
            }
            self.recent[w] &= !candidates;
            self.hand = (w + 1) % words;
        }
        None
    }

    /// Archetype `a` was reclaimed, leaving `shell`.
    pub(crate) fn reclaimed(&mut self, a: usize, shell: Shell) {
        set_bit(&mut self.dead, a);
        self.dead_count += 1;
        let width = shell.width();
        // A new generation: edges to the archetype that was here are stale.
        // A slot whose generations ran out stays dead, so that an edge from
        // generations ago can never pass for a current one.
        self.generations[a] += 1;
        if self.generations[a] != u32::MAX {
            self.free.push(a as u32);
        }
        if self.shell_count >= self.free.len() + KEEP_EMPTY.max(self.live) {
            // Enough kept already: let this one go.
            return;
        }
        if width >= self.shells.len() {
            self.shells.resize_with(width + 1, Vec::new);
        }
        self.shells[width].push(shell);
        self.shell_count += 1;
    }

    /// An edge to archetype `to` as it is now.
    #[inline]
    pub(crate) fn edge(&self, to: usize) -> Edge {
        Edge {
            to: to as u32,
            generation: self.generations[to],
        }
    }

    /// Where `edge` leads, unless the archetype it was made for was
    /// reclaimed since.
    #[inline]
    pub(crate) fn leads(&self, edge: Edge) -> Option<usize> {
        (self.generations[edge.to as usize] == edge.generation).then_some(edge.to as usize)
    }

    /// A dead slot to build the next new archetype in, if there is one.
    pub(crate) fn free_slot(&mut self) -> Option<usize> {
        self.free.pop().map(|a| a as usize)
    }

    /// The storage of a reclaimed archetype as wide as `width` components,
    /// if one is kept.
    pub(crate) fn shell(&mut self, width: usize) -> Option<Shell> {
        let shell = self.shells.get_mut(width)?.pop()?;
        self.shell_count -= 1;
        Some(shell)
    }

    /// Bytes of entity storage kept for reuse: pooled column buffers and
    /// entity lists.
    pub(crate) fn kept_bytes(&self) -> usize {
        self.buffers.bytes()
    }

    /// Whether more than one archetype in five is empty (dead slots count),
    /// and more than [`SCAN_LIST_AFTER`]: then scanning queries visit
    /// [`scan`](Self::scan)'s list instead of every archetype.
    #[inline]
    fn many_empty(&self) -> bool {
        let empty = self.len - self.live;
        empty > SCAN_LIST_AFTER && empty * 4 > self.live
    }

    /// What a query that scans visits: the archetypes with entities, in a
    /// world with many empty ones (listed now if they changed since the
    /// last time), or `None` for all of them. Allocates only when the list
    /// grows past every earlier length.
    #[inline]
    pub(crate) fn scan(&mut self) -> Option<&[u32]> {
        if !self.many_empty() {
            return None;
        }
        if !self.listed {
            self.relist();
        }
        Some(&self.list)
    }

    /// [`scan`](Self::scan) for queries that hold the world shared: the list
    /// only if it is current. Views list it before they run.
    #[inline]
    pub(crate) fn shared_scan(&self) -> Option<&[u32]> {
        (self.listed && self.many_empty()).then_some(&self.list[..])
    }

    #[cold]
    #[inline(never)]
    fn relist(&mut self) {
        self.list.clear();
        for (word, &bits) in self.occupied.iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                self.list.push((word * 64) as u32 + bits.trailing_zeros());
                bits &= bits - 1;
            }
        }
        self.listed = true;
    }
}

fn is_set(bits: &[u64], a: usize) -> bool {
    bits[a / 64] >> (a % 64) & 1 != 0
}

fn set_bit(bits: &mut [u64], a: usize) {
    bits[a / 64] |= 1 << (a % 64);
}

/// Clears bit `a`; whether it was set.
fn take_bit(bits: &mut [u64], a: usize) -> bool {
    let was = is_set(bits, a);
    bits[a / 64] &= !(1 << (a % 64));
    was
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_list_the_filled_slots_once_most_are_empty() {
        let mut slots = Slots::default();
        for a in 0..130 {
            slots.created(a);
        }
        assert_eq!(slots.occupied.len(), 3);
        for a in 0..130 {
            slots.filled(a);
        }
        // None empty: scans visit everything.
        assert_eq!(slots.scan(), None);
        for a in (0..130).filter(|a| ![0, 63, 64, 129].contains(a)) {
            slots.emptied(a);
        }
        assert_eq!(slots.scan(), Some(&[0, 63, 64, 129][..]));
        assert_eq!(slots.shared_scan(), Some(&[0, 63, 64, 129][..]));
        slots.emptied(63);
        // Stale until listed again by a query that holds the world exclusively.
        assert_eq!(slots.shared_scan(), None);
        assert_eq!(slots.scan(), Some(&[0, 64, 129][..]));
        slots.filled(1);
        assert_eq!(slots.scan(), Some(&[0, 1, 64, 129][..]));
    }
}
