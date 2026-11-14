# Archetype membership, column lookup and transitions (#144)

Every query asks each archetype whether it has a component (`Archetype::has`,
for `with`/`without` filters) and where that component's column is
(`column_index`, behind `column_ptr`, once per query term). Every structural
change finds its destination through the archetype's transition edges, then
maps the source columns onto the destination's. This file holds the
measurements of those costs and the reasons behind what the engine does.

All groups are headless and live in `src/membership.rs`:

```sh
cargo run --release -p ponko_bench -- membership             # everything below
cargo run --release -p ponko_bench -- membership/query       # query level only
cargo run --release -p ponko_bench -- membership/transition  # structural changes only
```

The group's own filter is a plain substring (no `a|b`).

## Setup and noise

Apple M4 Max, macOS, rustc 1.99.1, `--release`, the workspace's default
profile, measured on 2026-11-12. The machine was busy: other agents were
compiling, with load averages of **60-135** during every run.

- The `has`, `index`, `colmap`, `transition` and `edge-lookup` rows are
  medians over hundreds of rounds in which the candidates of a group run
  interleaved, round by round, so drift hits them alike.
- The `query/*` rows (the real query and the `lookup-*` candidates) are not
  interleaved: each case takes 200 hot samples back to back, then 25 cold
  ones, and the cases run one after the other.
- "Before" binaries are this branch's benchmark code built against
  `crates/ponko_ecs` at `5a7bae0b` (the `main` revision this work started
  from: linear scan, and a heap slot for every destructor remove). Base and
  changed binaries ran alternately, three times each.
- Rows that run identical code in both binaries (the query rows in the final
  pair of binaries) differed by **up to ±30%** between runs, so no claim below
  rests on a difference smaller than that, except where both numbers come
  from the same interleaved rounds.

The earlier slice of this issue (#249, measured on 2026-11-09 under load
~115) has the before/after numbers for the column mapping and the edge
hasher; its tables are repeated here. Today's micro numbers are about 2x
#249's for code that did not change (the linear scan: 19.8 against 9.1 ns
at 32 components), which is the load, not a regression.

## What was already done (#249)

- `move_row` mapped columns with a linear scan per source column (O(n²) in
  the archetype width). Both id lists are sorted; one merge pass does it now.
- The transition-edge maps used SipHash for a `u32` key; they use a
  one-multiply hasher now (`ComponentIdHasher`).
- No extra memory, no allocations, no change to queries or the public API.

Real transitions, ns per insert or remove (one entity, edge cached), #249:

| components | warm, 1 edge | 32 components in turn (64 edges) | cold (fresh world, first add+remove) |
|---:|---:|---:|---:|
| 2 | 17.9 → 14.4 | 19.8 → 14.9 | 125 → 125 |
| 8 | 47.9 → 35.9 | 48.7 → 35.1 | 188 → 167 |
| 16 | 107 → 66 | 108 → 66 | 271 → 229 |
| 32 | 272 → 120 | 273 → 120 | 542 → 396 |
| 64 | 827 → 226 | 829 → 227 | 1313 → 688 |

## Membership and column lookup in isolation

`membership/has/*` and `membership/index/*`: 16,384 shuffled, independent
lookups into one archetype (throughput, hot cache), sparse ids, hits, ns per
lookup. Today's run (load ~110):

| components | `has` (engine, linear) | `has` binary search | `has` bitset | column index (engine, linear) | binary search | 8-wide blocks | bitset + rank | flat table |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2.5 | 1.2 | 0.6 | 3.2 | 1.1 | 3.1 | 1.3 | 0.6 |
| 8 | 5.2 | 2.0 | 0.6 | 8.4 | 1.9 | 1.4 | 1.3 | 0.6 |
| 16 | 6.7 | 2.7 | 0.6 | 12.6 | 2.7 | 3.5 | 1.3 | 0.6 |
| 32 | 7.3 | 3.5 | 0.6 | 19.8 | 3.4 | 7.0 | 1.3 | 0.6 |
| 64 | 16.3 | 4.4 | 0.6 | 30.9 | 4.3 | 15.2 | 1.3 | 0.6 |

Memory the index candidates would add per archetype, in a world with 320
registered component types (`membership/memory`): bitset + rank 50 B, flat
`u16` table 640 B (it grows with every registered type, in every archetype).
The id list they would supplement is 4 B per component; an `Archetype` is
128 B and a column 32 B (both asserted by a test in `storage.rs`).

In isolation every candidate beats the scan. The question is whether that
survives inside a query, which looks up the same ids in archetype after
archetype.

## Query level, hot and cache-cold

`membership/query/*`: a `(&mut P, &V)` query over **4096 archetypes** of 4 to
64 components, 4 entities each, so per-archetype work is most of the cost.
`P` and `V` are either the **first** two ids of every archetype (registered
first) or the **last** two (registered after 257 others). Hot: back-to-back
runs. Cold: 128 MiB written before every run, more than the last-level
cache, so the archetype structs, id lists and columns come from memory: the
first frame after the world was out of cache. The flush leaves those lines
dirty, so writing them back to memory may make the cold numbers slightly high. ns per archetype, median of
three runs of the current engine:

| components | first, hot | first, cold | last, hot | last, cold |
|---:|---:|---:|---:|---:|
| 4 | 5.8 | 12.8 | 7.9 | 15.1 |
| 8 | 6.2 | 19.7 | 13.1 | 30.8 |
| 16 | 6.9 | 25.5 | 24.1 | 44.9 |
| 32 | 7.1 | 26.3 | 43.4 | 58.4 |
| 64 | 8.8 | 25.5 | 85.6 | 147 |

Adding `.filter::<Without<Tag>>()` (a `has` miss, which scans the whole list) costs
another 2-20 ns per archetype hot, growing with the width.

- **Membership cost depends on where the id sits, not only on the width.**
  When the queried components are the first ids, the scan stops at once and
  a query costs 6-9 ns per archetype at every width. When they are the last,
  the scan dominates: 43 ns at 32 components and 86 ns at 64.
- **Cold start**: the first frame after the world left the cache costs
  2-4x a hot one (13-26 ns per archetype with early ids). Per archetype a
  query reads the `Archetype` struct, then its id list and its column array
  (two separate allocations), then the rows: dependent misses that a
  different id search doesn't remove.
- **Allocations**: every query row reports 0 heap allocations per run (the
  bench counts them); `tests/archetype_alloc.rs` asserts it for plain
  `for_each`, `iter`, `count`, filters and `for_each_chunk` over
  40-component archetypes.

### Candidates inside the query

`lookup-*` rows do only the two column lookups over the same 4096
archetypes, with each candidate (hot, ns per archetype, one run under load
~111):

| components | linear (engine) first / last | binary search | linear ≤ 8, else binary | 8-wide blocks | rank by counting |
|---:|---:|---:|---:|---:|---:|
| 4 | 2.2 / 4.5 | 3.8 / 3.8 | 2.0 / 4.0 | 2.9 / 5.2 | 5.1 / 5.0 |
| 8 | 2.1 / 8.9 | 5.1 / 5.1 | 2.6 / 7.6 | 3.7 / 3.7 | 5.6 / 7.7 |
| 16 | 2.5 / 21.1 | 6.4 / 10.0 | 6.3 / 9.9 | 4.9 / 4.8 | 13.5 / 10.6 |
| 32 | 2.8 / 38.9 | 8.3 / 7.4 | 8.7 / 7.5 | 4.9 / 8.3 | 18.7 / 21.2 |
| 64 | 2.9 / 74.5 | 14.6 / 9.6 | 14.4 / 9.7 | 3.9 / 19.2 | 34.9 / 34.8 |

Binary search pays its log2(n) dependent loads even for the first id; rank
by counting doesn't vectorize here (about 0.5 ns per id). The 8-wide block
compare (an equality mask per block of 8, and an early exit once a block ends
above the id, since the list is sorted) was the best lookup: within 0.7-2.3 ns
of the scan for early ids, 2.4-4.7x faster for late ones.

### The block compare in the engine: not a win

Put into `column_index` and `has`, the block compare made the real query
slower for narrow archetypes, even where it ran the same scan as before. The
compiler stopped inlining `column_ptr` into the query loop (two calls per
archetype instead of none). Variants tried, real `(&mut P, &V)` query, hot,
ns per archetype, each next to the unchanged engine in the same session:

| variant | 4 first | 8 first | 16 first | 32 first | 64 first | 4 last | 16 last | 32 last | 64 last |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| linear scan (engine) | 5.7-5.9 | 6.0-6.2 | 6.6-9.9 | 6.6-11.7 | 7.8-9.7 | 7.9-8.0 | 23.5-24.9 | 41.4-44.0 | 82.5-89.8 |
| blocks, `#[inline]` | 15.5 | 14.3 | 14.4 | 15.8 | 15.7 | 14.5 | 15.6 | 19.4 | 26.7 |
| blocks, everything `#[inline(always)]` | 13.4 | 11.3 | 12.7 | 13.0 | 13.8 | 14.2 | 14.2 | 22.1 | 23.7 |
| first 8 inline, rest out of line | 8.2-9.6 | 9.6 | 8.8-9.0 | 12.1-14.6 | 8.5-11.0 | 10.5-12.7 | 22.0-23.3 | 23.0-25.3 | 30.9-31.4 |
| old scan if ≤ 8 ids, else out-of-line blocks | 7.7-10.1 | 10.3-12.6 | 12.8-13.1 | 12.7-12.9 | 14.2-19.2 | 11.4-12.4 | 14.4-17.9 | 17.3-17.7 | 27.8-34.5 |

To rebuild the "blocks, `#[inline]`" row: copy the bench's `chunked_index`
(`src/membership.rs`, the `lookup-chunked8` rows) into
`crates/ponko_ecs/src/storage.rs` as an `#[inline]` free function `find`, and
make `Archetype::has` return `find(&self.ids, id).is_some()` and
`column_index` return `find(&self.ids, id)`. Nothing else in the engine
changed. The other rows change only that: `#[inline(always)]` on `find`,
`has`, `column_index` and `column_ptr`; the first block of 8 compared inline
and the rest in an `#[inline(never)]` call; or `position` when the list has
at most 8 ids and an `#[inline(never)]` `find` otherwise. Then run
`-- membership/query` against both builds, alternating.

Every variant cut the late-id cost at 32-64 components by 1.7-3.8x but
added 2-10 ns per archetype at 4-16 components (2-6 ns for the last two),
where most archetypes are, even the last one, whose ≤ 8-id path is the old scan: the length check and an
out-of-line call are enough to change how the loop compiles. That is not a
measured win, so **the linear scan stays**; the comment at `column_index`
records why. A bitset or table would change `has`/`column_index` the same
way and cost 50-640 B per archetype besides.

The cost that does grow (late ids in wide archetypes) is paid because a
query recomputes each matched archetype's column pointers on every run.
The way to remove it without touching the scan is to keep column indices
next to the matched archetypes in the query index (#139's `MatchCache`), so
the lookup happens when an archetype is first matched, not every frame. That
is a change to the query index, left as a follow-up.

## Transition-edge cache

Steady state is a 100% hit rate. The bench visits every edge once, times
the cases, then makes one more pass over every edge it used and asserts that
the pass made no heap allocation: a miss copies the id list (`to_vec` or
`collect` in `world.rs`), which allocates for any archetype of more than one
component, even when the archetype it finds already exists.
`tests/archetype_alloc.rs` asserts the same over 100 warm toggles on a
40-component entity. (The archetype count can't show a miss, because a miss
finds the existing archetype.) Today:

| | 1 edge | 4 | 16 | 64 |
|---|---:|---:|---:|---:|
| edge lookup, SipHash (before #249), ns | 10.2 | 10.3 | 10.3 | 11.0 |
| edge lookup, one-multiply hasher (engine), ns | 1.6 | 1.8 | 1.8 | 1.8 |
| short linear list, ns | 1.1 | 2.9 | 4.9 | 13.2 |

The engine's lookup is flat from 1 to 64 edges; a list only wins below
about 4. A miss costs the "cold" column of the transition table (copy and
sort the id list, look the set up, and for a new archetype allocate it):
333-1750 ns today for 2-64 components, 4-11x a hit. Edges are never evicted;
an archetype holds at most one per component added or removed there.

Current warm transitions, ns per insert or remove (load ~100, so about 2x
#249's quieter numbers): 31 at 2 components, 76 at 8, 140 at 16, 253 at 32,
477 at 64, the same with 64 edges in use (`cycle32`).

## Removing a component that has a destructor (changed)

`RawWorld::remove_dynamic`, which C/C++ removes and deferred dynamic removes
go through, moved the value of a component with a drop function into a
**heap** slot before dropping it (so that a panicking drop leaves the world
consistent): one allocation and one free per remove. Values up to 64 bytes
at up to 16-byte alignment now use a stack slot; larger ones keep the heap.
The typed `World::remove::<T>` never allocated and is unchanged.

`membership/transition/*/warm-1-edge-with-drop-fn` toggles an 8-byte
component with a drop function. Its extra cost over the plain warm toggle,
from the same interleaved rounds, ns per transition, three runs each:

| components | before | after |
|---:|---:|---:|
| 2 | 11.8-18.5 | 3.3-5.6 |
| 8 | 15.2-17.0 | 2.0-3.5 |
| 16 | 12.0-18.8 | 2.5-4.3 |
| 32 | 14.7-17.6 | 3.2-4.3 |
| 64 | 13.9-14.4 | 2.2-2.8 |

Allocations: 100 toggles of such a component made 100 heap allocations
before and make 0 now (`tests/archetype_alloc.rs`, which fails against the
old code). Memory: no heap; the remove uses a 64-byte stack slot.

## Reclaiming empty archetypes (#347): no regression

Reclaiming empty archetypes (`crates/ponko_ecs/src/slots.rs`) adds a bit
operation to every move that fills or empties an archetype and checks a
generation when following an edge. Three interleaved full runs of
`membership` against engine `main` at `3b8b1c17`, on 2026-11-14, with the
machine busy (load averages around 20 to 35): over all 372 cases, the
median ratio after/before is 1.00 (10th percentile 0.96, 90th 1.02).
Transitions, ns:

| case | before | after |
|---|---:|---:|
| n2 `warm-1-edge` | 15.3 (14.2) | 16.4 (15.3) |
| n2 `cycle32-64-edges` | 15.9 (14.8) | 16.4 (15.2) |
| n8 `warm-1-edge` | 35.6 (33.2) | 36.0 (34.7) |
| n16 `warm-1-edge` | 65.4 (56.3) | 65.7 (59.5) |
| n64 `warm-1-edge` | 221.3 (211.3) | 221.6 (209.2) |

Medians of the three runs, lowest in parentheses. The two-component toggle
is about 1 ns (4 to 7 %) slower, at the edge of the 6 to 8 % its runs moved
by; the larger ones tie. A first version that queued empty archetypes,
oldest first, cost about 3 ns there (19 ns) and was replaced by a sweep
over bits. `edge-lookup` doesn't use the engine's edge type and ties.

## Despawning after reclaiming (#448)

The benchmark tracking on the pull request that brought reclaiming counted
`ecs/despawn/10k` at 1,609,416 instructions per operation before and
1,721,823 after (+7.0 %, cachegrind on GitHub's Ubuntu runner). Where the
extra instructions went:

- **Not setup or teardown.** `fresh(...)` builds the same world in the
  measured and the unmeasured run, and leaks it with `mem::forget` in both,
  so neither the spawning nor dropping the world (with its `Slots` and
  `BufferPool`) is in the difference. The workload measures only the
  despawns and stays as it is; its description (`bench_track list`) now
  says that spawning and dropping the world aren't measured.
- **Per-entity work in `World::despawn`.** It checked
  `archetypes[a].is_empty()` after every removal. On x86-64 that kept the
  archetype pointer and index live across the column loop: three more
  spills and reloads, and two more jumps where the moved entity's row is
  written, on every despawn. On arm64 the same code compiles without them,
  which is why the arm64 counts below never moved.

The fix decides "the archetype just emptied" from what `swap_remove_drop`
returns: no entity moved into the freed row, and that row was 0. That
branch is marked cold (`std::hint::cold_path`) and calls a cold function
for `Slots::emptied` and the reclaiming check, so the path where an entity
moves in compiles to the same instructions as before reclaiming. What
remains is the test of the row on the branch where nothing moved (2
instructions; the workload despawns in spawn order, so that is half of its
despawns). Reclaiming needs it: it is how the world learns an archetype
emptied. `despawning_the_last_entity_in_any_row_order_empties_the_archetype`
(`crates/ponko_ecs/tests/queries.rs`) empties archetypes through every row
order and fails if an emptied archetype is missed (it then counts 129
archetype slots instead of fewer than 128).

**Instruction counts, cachegrind.** valgrind doesn't run on macOS, so these
come from an x86-64 Debian 12 virtual machine (qemu, `-cpu max`, glibc 2.36,
valgrind 3.19, rustc 1.100.0), on a copy of the `ecs/despawn/10k` workload
built against `ponko_ecs` alone (the same `moving_world`, `fresh` and loop).
Counts are deterministic: repeated runs gave the same numbers.

| revision | per operation | change |
|---|---:|---:|
| before reclaiming (`81d54c08`) | 1,625,178 | |
| with reclaiming (`a14ba7da`) | 1,698,678 | +4.5 % |
| emptiness from `swap_remove_drop`'s result | 1,643,678 | +1.1 % |
| and the no-move branch cold (this change) | 1,638,685 | +0.8 % |

By function, per operation, before → with reclaiming → this change:
`World::despawn` 645,091 → 715,123 → 655,095 (the 2 instructions on the
no-move branch, 10,004); `memcpy` 179,899 → 182,543 (about +2,600, most
likely the entity free list's reallocations, which depend on how the heap
is laid out now that outgrown buffers wait in the pool); `malloc` about
+550. `swap_remove_drop`, `Entities` and the workload's own loop are equal.
The VM shows a smaller rise than the runner (+4.5 % against +7.0 %): a
different compiler and glibc; the next tracking run on the runner gives
the number that counts.

**Wall clock**, release, Apple M4 Max, `ponko_bench despawn`, the three
builds interleaved, 5 rounds each, with the machine busy (load averages 25
to 45 from other builds). Median of the 5 medians, ms (lowest single run in
parentheses):

| benchmark | before reclaiming (`81d54c08`) | with reclaiming (`f3844aa3`) | this change |
|---|---:|---:|---:|
| `despawn/ponko` (100k, one by one) | 8.92 (8.44) | 8.68 (7.96) | 8.60 (7.86) |
| `ecs/spawn-despawn-100k/ponko` | 0.221 (0.208) | 0.212 (0.204) | 0.215 (0.203) |
| `ecs/spawn-despawn-1m/ponko` | 2.18 (2.14) | 2.13 (2.05) | 2.17 (2.13) |
| `commands/spawn-despawn` | 2.68 (2.40) | 2.65 (2.33) | 2.60 (2.32) |
| `changes/ponko-despawn-tracked` | 13.7 (13.0) | 14.1 (12.9) | 13.6 (12.4) |

All three are within the runs' noise (one build's medians moved by up to
20 % between rounds): the extra instructions were never visible in time
on this machine, and the change claims no speed-up. arm64 instruction
counts of `bench_track run ecs/despawn/10k` (from `proc_pid_rusage`, which
includes some kernel work, so ±5 % under this load): 1.54 M before, 1.52 M
with reclaiming, 1.51 M with this change.

## Not measured

- Only Apple M4 Max under heavy load; other platforms untested, and a quiet
  machine would give lower absolute numbers.
- End-to-end game frames: `archetypes-1024` and `ecs/game` were not re-run,
  because the engine's lookup is unchanged.
- C/C++: the destructor-remove change is inside the engine and needs no
  binding change; no native benchmark was run.
