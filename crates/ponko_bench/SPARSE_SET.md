# Sparse-set storage for sparse, toggled components (#166)

The question: archetype storage moves an entity to another table whenever a
component is added or removed, and keeps one table per combination of
components. When many independent optional components (status effects,
selection or visibility markers) are toggled on a fraction of the entities
every tick, the combinations multiply. Would an opt-in sparse-set storage
for such components pay for itself?

**Decision: keep archetype storage only.** Don't add opt-in sparse storage
now. At 1 % and 5 % density, the cases a game is likely to have, a whole
tick is as fast or faster on archetypes; sparse sets win only at an extreme
(16 independent components on 20 % of the entities each, 32,000 toggles per
tick), where packing the flags into one ordinary dense component, which
needs no engine change, does as well (in the layout measured here, with
room left to make it faster). Integrating a second storage would
touch nearly every part of the ECS, the C ABI and the tools, and would give
users two storages with opposite performance profiles to choose between.
What the measurements did show is a cost inside archetype storage that was
worth fixing there: empty archetypes were never reclaimed. That is fixed
since (issues #347 and #348, see
[Empty archetypes](#empty-archetypes-skipped-and-reclaimed-347-348)).

This is about which storage holds sparse data. Issue #144 asks a different
question: how fast an archetype answers "do you have component X, and in
which column?" (`membership/`, `Archetype::has`, `column_index`, the
transition edges). The archetype counts here make those per-archetype costs
matter more, but even free lookups would leave the row moves on every
toggle and the empty tables, which are what this file measures.

## What is measured

All of it is headless and lives in `crates/ponko_bench`:
`src/sparse_set.rs` (the workload and the benchmarks) and
`src/sparse_set/proto.rs` (the prototype). Nothing in `ponko_ecs` changed.

**The workload.** 100,000 entities that all have `Pos` and `Vel`, and 16
independent optional components: 8 zero-sized tags and 8 effects with 8
bytes of data. Each entity has each optional component with probability
1, 5 or 20 %. Every tick, 5 % of each component's holders lose it and as many
other entities gain it, so the density stays the same while the combinations
keep changing (1,626, 8,000 and 32,070 adds and removes per tick). Worlds are
measured after 60 ticks of this, one second at 60 Hz. The changes come from
a seeded simulation, and every storage replays the same sequence.

**The storages**, all doing the same work, which the test
`both_storages_compute_the_same_frames` checks: after every frame, all three
storages have the same `Pos` bits, the same components on every entity and
the same filter checksums.

| Name | Storage |
| --- | --- |
| `arch` | Everything in `ponko_ecs` archetypes, default query index policy (on: these worlds have 500 to 40,000 archetypes) |
| `arch-scan` | The same with the query index off (`QueryCachePolicy::off()`), for scale |
| `sset` | `Pos`/`Vel` in archetypes (one archetype), the 16 optional components in the prototype's sparse sets; joins driven from the sparse set |
| `sset-rows` | The same, joins driven from the archetype rows (one sparse lookup per row) |
| `flags` | No engine change: the 16 optional components are bits and slots of one dense `Status` component (68 bytes), tested row by row |

**The prototype** (`proto.rs`, safe Rust except two pointer casts in
`unsafe fn`s, tested) is the classic layout: per component, a `sparse` array indexed by the
entity's slot index (4 bytes per slot, grown in 4,096-slot steps) pointing
into packed `entities` and `values` arrays. Adding and removing a component
touches only that component's arrays. Joining with archetype components
looks each entity up through `RawWorldRef::get_dynamic` with a cached component
id: the entity table, then the entity's archetype, then a linear search of
that archetype's component ids for the column. It has no change stamps, no
parallel iteration and no C ABI, which an integrated version would have to
pay for; but an integrated join could also cache the archetype's column and
skip the search. So its timings are not a bound either way: the "What the
numbers say" section checks that the decision holds at half its join cost.
`arch` is the engine as it ships.

**The operations**, each timed on its own (`sparse-set/{op}/{density}pct/{storage}`):

| Op | Work |
| --- | --- |
| `churn` | One tick of adds and removes |
| `dense` | `pos += vel * dt` over all 100,000 entities |
| `data` | For each of the 8 effects, `pos.x += effect` over its holders |
| `and` | For 8 pairs of tags, read `Pos` of the entities that have both (`.filter::<(With<A>, With<B>)>()`) |
| `without` | For 4 tags, write `Pos` of every entity without it (`filter::<Without<T>>()`) |
| `frame` | All of the above once, as a game tick would |

`sparse-set/dense-data/*` is the opposite case: every entity has `Pos` and
two effects (all dense). `iter` adds both effects to `Pos`; `add-remove`
adds a tag to all 100,000 entities and removes it again.

Commands:

```sh
cargo build --release -p ponko_bench
B=target/release/ponko_bench
$B sparse-set/          # every timed group plus sparse-set/mem, about 90 s
for d in 1 5 20; do for k in arch sset flags; do $B sparse-set/rss/${d}pct/$k; done; done
rm -rf target/release
```

Each timed case: 5 warm-up runs, 30 measured runs (10 for `add-remove`) on
the same world, median and minimum reported. `sparse-set/rss/...` cases run
one per process: the peak never goes down.

## Machine, revision and noise (2026-11-12)

Apple M4 Max (14 cores, 36 GB), macOS 27.2.1, aarch64-apple-darwin, rustc
1.99.1 (58cf8f9c3 2026-10-13), Cargo release profile, default benchmark
features. Engine `main` at `5a7bae0b`; benchmarks at `39b8ce7b` (runs 1 to
3) and `a740dabd` (runs 4 to 6, which add `flags` and don't change the other
storages' code).

**The machine was very busy**: other agents compiled throughout, with load
averages of 79 to 150 on 14 cores. Every table below gives the median of the
per-run medians over six full runs (three for `flags`), with the lowest
single measurement in parentheses. Between runs, the median of one case
moved by up to 20 % for most sub-millisecond cases, by 50 to 100 % for some,
and by up to 10× for the 20 % `churn` and `frame` rows, which allocate and
move tens of megabytes per tick. Differences below 1.5× are treated as ties
unless the medians and the minimums agree. p95 values are scheduling noise
and are not shown. Memory numbers are exact counts and don't vary between
runs.

## Results

Milliseconds per operation: the median of the per-run medians (six runs,
three for `flags`), and in parentheses the lowest single measurement of
any run.

#### 1 % density

| Operation | `arch` | `arch-scan` | `sset` | `sset-rows` | `flags` |
| --- | --- | --- | --- | --- | --- |
| `churn` | 0.136 (0.121) | — | 0.060 (0.052) | — | 0.034 (0.027) |
| `dense` | 0.040 (0.039) | 0.040 (0.030) | 0.037 (0.036) | — | 0.037 (0.036) |
| `data` | 0.008 (0.008) | 0.018 (0.018) | 0.053 (0.048) | 0.782 (0.686) | 0.949 (0.940) |
| `and` | 0.002 (0.002) | 0.011 (0.010) | 0.021 (0.018) | — | 0.951 (0.944) |
| `without` | 0.155 (0.154) | 0.155 (0.154) | 0.601 (0.595) | — | 0.756 (0.538) |
| `frame` | 0.454 (0.353) | 0.428 (0.382) | 0.843 (0.775) | 1.570 (1.475) | 2.769 (2.537) |

#### 5 % density

| Operation | `arch` | `arch-scan` | `sset` | `sset-rows` | `flags` |
| --- | --- | --- | --- | --- | --- |
| `churn` | 1.027 (0.910) | — | 0.298 (0.239) | — | 0.500 (0.120) |
| `dense` | 0.055 (0.054) | 0.074 (0.054) | 0.037 (0.036) | — | 0.037 (0.037) |
| `data` | 0.058 (0.056) | 0.211 (0.137) | 0.292 (0.282) | 1.224 (1.029) | 1.158 (1.086) |
| `and` | 0.008 (0.008) | 0.087 (0.081) | 0.180 (0.137) | — | 1.133 (0.849) |
| `without` | 0.216 (0.211) | 0.212 (0.209) | 0.827 (0.752) | — | 0.546 (0.539) |
| `frame` | 1.746 (1.453) | 3.302 (1.788) | 1.813 (1.525) | 3.822 (2.526) | 4.439 (3.188) |

#### 20 % density

| Operation | `arch` | `arch-scan` | `sset` | `sset-rows` | `flags` |
| --- | --- | --- | --- | --- | --- |
| `churn` | 33.1 (9.25) | — | 1.594 (1.125) | — | 0.534 (0.462) |
| `dense` | 0.290 (0.239) | 0.291 (0.210) | 0.037 (0.037) | — | 0.037 (0.037) |
| `data` | 1.729 (1.242) | 5.664 (2.622) | 1.208 (1.017) | 2.923 (1.824) | 2.038 (1.876) |
| `and` | 0.377 (0.234) | 3.712 (1.765) | 1.077 (0.712) | — | 2.139 (1.881) |
| `without` | 2.804 (1.667) | 1.966 (1.624) | 1.506 (1.263) | — | 1.004 (0.813) |
| `frame` | 65.8 (15.3) | 64.4 (20.6) | 11.9 (5.29) | 14.2 (7.07) | 8.69 (5.83) |

Heap allocations per tick, counted by the benchmark allocator over the
measured ticks 66 to 95 (after 60 ticks of settling and 5 warm-up ticks):
`arch` 33 (1 %), 278 (5 %) and 3,375 (20 %) in `churn` and `frame`. Mostly
because new combinations keep creating archetypes; growing columns, the
transition edges and the query index following new archetypes allocate
too, and the counter doesn't tell them apart. `sset` and `flags`: none.

#### Dense data (every entity has `Pos` and two effects)

| Operation | `arch` | `sset` | `sset-rows` |
| --- | --- | --- | --- |
| `iter` | 0.067 (0.057) | 0.647 (0.585) | 0.459 (0.414) |
| `add-remove` (100,000 adds, then removes) | 27.5 (9.71) | 6.57 (3.95) | — |

#### Archetypes and memory

`sparse-set/mem`: archetypes in the world (all of them, and those holding
entities) and storage bytes (`World::allocated_bytes`, the prototype's
arrays, and a lower bound for each archetype's own struct, ids and column
headers; the transition-edge maps aren't public and aren't counted).

| Density | `arch` archetypes, tick 0 / 60 / 300 / 600 | not empty, tick 0 / 60 / 300 / 600 | `arch` KiB, tick 0 → 600 | `sset` KiB | `flags` KiB |
| --- | --- | --- | --- | --- | --- |
| 1 % | 184 / 502 / 775 / 864 | 180 / 193 / 180 / 177 | 3,633 → 4,103 | 9,744 | 11,776 |
| 5 % | 1,351 / 3,864 / 6,556 / 8,080 | 1,276 / 1,328 / 1,280 / 1,293 | 6,933 → 12,062 | 11,008 | 11,776 |
| 20 % | 14,796 / 39,818 / 51,979 / 56,057 | 13,064 / 13,099 / 13,105 / 13,115 | 25,385 → 65,909 | 15,616 | 11,776 |

`sset` and `flags` keep 2 archetypes (the empty one and the one with
`Pos`/`Vel`), and their bytes don't change over time. The sparse arrays are a
fixed 4 bytes per entity slot per sparse component (6,400 KiB for 16
components and 102,400 slots), which is why `sset` costs more than `arch` at 1 %.
Paging the sparse array helps only when holders cluster by entity index;
here they are spread evenly.

Peak resident memory after 600 ticks, one process per case, two rounds
(round to round it moved by up to 12 MB, so only the 20 % `arch` case stands
out): 1 %: `arch` 77/89 MB, `sset` 88/97 MB, `flags` 76 MB; 5 %: `arch`
98/95 MB, `sset` 90/86 MB, `flags` 88 MB; 20 %: `arch` 169/156 MB, `sset`
78/78 MB, `flags` 103 MB. That includes the benchmark process's own baseline
and the change simulation, the same for every storage.

## What the numbers say

- **The archetypes multiply, and they never go away.** With 16 independent
  components, the number of live combinations stays about the same from the
  first tick on (within 10 % at every checkpoint: 177 to 193, 1,276 to 1,328
  and 13,064 to 13,115 non-empty archetypes), but every combination that
  ever existed stays as an archetype with its buffers: after 600 ticks there
  are 5×, 6× and 4× more archetypes than live ones, still growing, and at
  20 % the storage has grown from 24.8 to 64.4 MiB (25,385 to 65,909 KiB). A
  world that churns like this allocates on every tick we measured, which the
  project rules don't allow on hot paths in steady state.
- **Toggles cost 2 to 3.5× more on archetypes at 1 to 5 %, and 8 to 20×
  more at 20 %** (`churn`: 0.136 against 0.060 ms, 1.03 against 0.30 ms,
  33 (min 9.3) against 1.6 (min 1.1) ms). An archetype move copies all of
  the entity's other components, and at 20 % it often has to create the
  destination archetype too. `flags` is cheapest: a toggle is a bit write.
- **Selective queries are what archetypes are best at.** `data` and `and`
  are 5 to 22× faster on archetypes at 1 and 5 %: a query visits only the
  matching tables and reads them in order, while the sparse set looks up
  every holder's `Pos` (about 6.5 ns per element, against about 1 ns per
  archetype row). Those 6.5 ns include the prototype's lookup path (entity
  table, archetype, linear column search); an integrated join that caches
  the archetype's column would be faster. Even at half that cost, `data`
  would still be 2.5 to 3.3× faster on archetypes at 1 and 5 % (0.027
  against 0.008 ms, 0.146 against 0.058 ms), and a whole tick would stay
  1.8× faster on archetypes at 1 % and a tie at 5 % (about 1.57 against
  1.75 ms, estimated by halving the `sset` `data` and `and` times). At 20 %, `data` is a tie (1.73
  against 1.21 ms, minimums 1.24 against 1.02) and `and` is still 3× faster
  on archetypes. The query index matters here: with it off, `data` and `and`
  are 2 to 11× slower.
- **Dense queries suffer from fragmentation, but only at the extreme.**
  `dense` is the same at 1 % (0.040 against 0.037 ms, within the noise),
  1.5× slower at 5 % and 8× slower at 20 %, where the loop visits 40,000
  archetypes, 13,000 of them with entities. `without` is 4× faster on
  archetypes at 1 and 5 % (whole tables are skipped), about 1.3× slower at
  20 % (by the minimums).
- **A whole tick:** at 1 %, archetypes are 1.9× faster than sparse sets
  (0.45 against 0.84 ms) and 6× faster than `flags`; at 5 % archetypes and
  sparse sets tie (1.75 against 1.81 ms, minimums 1.45 against 1.53), and
  `flags` is 2.5× slower. Only at 20 % do archetypes lose: about 3× slower
  than both `sset` and `flags` by the minimums (15.3 against 5.3 and 5.8
  ms), 5 to 8× by the medians (65.8 against 11.9 and 8.7 ms). There, `flags` matches the prototype without any engine
  change. Its layout is the simplest one: the effects sit inline in the
  68-byte `Status`, and `data`, `and` and `without` each make full passes
  over it, so a separate small mask component (2 bytes per entity) next to
  the effect data would likely scan faster; that wasn't measured.
- **The dense case** confirms the other side: when every entity has the
  components, archetypes iterate 10× faster than the sparse sets
  (0.067 against 0.65 ms). Sparse sets still add and remove a component on
  every entity 2.5 to 4× faster.
- In the sparse workload, driving joins from the archetype rows
  (`sset-rows`) is never better than driving them from the sparse set, even
  at 20 %. In the dense case, where every entity has the components, it is
  (0.459 against 0.647 ms).

## Integration cost of a hybrid

Read from the code at `5a7bae0b`. Each item is something an opt-in sparse
storage would have to change; nothing here is implemented.

- **Storage and world** (`storage.rs`, `world.rs`). A type-erased sparse
  set like `Column` (layout, drop function, change stamps), stored per
  `ComponentId` and not in a type map, so that views can borrow different
  sets at once (below). Every structural path dispatches on the component's
  storage: `spawn`, `spawn_batch` and `spawn_at` (a bundle's `BundleInfo` is
  split into its archetype part and its sparse part), `insert`, `remove`,
  `insert_dynamic`, `remove_dynamic`, `spawn_dynamic`, `clear`, `reset`.
  `despawn` has to remove the entity from every sparse set it is in: either
  visit them all (one lookup per sparse type per despawn) or keep a list per
  entity. Every read path dispatches too: `get`, `get_mut`, `has`,
  `get_many_mut`, `locate`, `get_dynamic`, `components_of`,
  `allocated_bytes`. `sort_by` only permutes archetype rows and is
  unaffected.
- **Queries** (`query.rs`, `query/cache.rs`, `query/dynamic.rs`).
  `QueryData::state` receives only an `&Archetype` and `fetch` only a row,
  so `&T` for a sparse `T` can't be expressed: both need the world's set
  and the row's entity, which changes the (doc-hidden) methods of the
  public `unsafe trait QueryData`. `Filters::matches` decides per archetype;
  `with`/`without`/`Or` terms on sparse components become per-row tests like
  `Added`/`Changed` (`RowFilter` in `change.rs`), so `count()` can no longer
  add up archetype lengths, and the fixed-size `Filters` needs room for
  them. The query index's `MatchKey` must leave sparse terms out. A query
  whose most selective term is sparse should be driven from that set, which
  is a second query loop next to the archetype loop.
- **Change detection** (`change.rs`). Sparse values need their own `added`
  and `changed` stamps, moved by every swap-remove, aged by `age_stamps`,
  written by `mark_changed` and by mutable fetches. The removal logs work by
  component id and need no change.
- **Commands** (`commands.rs`). They call `World::insert` and `remove`, so
  they work once those dispatch; their order and barriers don't change.
- **Views and the parallel scheduler** (`view.rs`, `access.rs`, the
  engine's scheduler). Declared access is by component type and doesn't
  change. `WorldView::get`, `get_mut`, `has` and `query` need the sparse
  paths. `ViewGroup` runs views with disjoint access in parallel, so two
  views writing two different sparse components must be able to borrow two
  sets at once: per-id storage with the same disjointness proof the
  archetype columns have today. Parallel queries (`ParPlan`) plan row ranges
  per archetype; a sparse row filter makes the chunks uneven, and writes to
  sparse values from worker threads need their own proof that rows map to
  distinct slots. Sparse adds and removes could in principle be
  non-structural (only one set changes), which would be a scheduler
  feature of its own, with a new kind of access.
- **C ABI** (`crates/ponko_ffi`, `include/ponko.h`, the C++ header).
  `ponko_component_register` has no storage option: a new function or flag
  (additive). `ponko_query`, `ponko_query_filtered` and `ponko_query_cached`
  hand out `PonkoChunk`s whose columns are contiguous arrays aligned with
  the chunk's entities; a sparse component has no such column, so these
  must reject sparse ids with an error, gather values into temporary arrays
  (writes would be lost), or gain a new per-row API. `ponko_get` works
  unchanged through `get_dynamic`. C and C++ tests in `examples/cpp` for all
  of it.
- **Saved worlds** (`src/reflect.rs`). The format is per entity, by
  component name, independent of storage, so the save version and the
  compatibility fixtures stay. `entity_json` reads through the registry's
  getters (`World::get`), which would work once `get` dispatches; `save`
  counts the components it can't save by walking `raw_ref().archetypes()` and
  would miss sparse ones.
- **Determinism.** A sparse set's packed order depends only on the order
  of adds and removes, so the same build still gives the same world from
  the same inputs, with or without parallel systems. Queries driven from a
  sparse set iterate in a different order than archetype queries; that
  order is not part of the API (STABILITY.md), so that is allowed, but it
  is a visible change for code that depends on it.
- **Tools.** The inspector (`src/inspector.rs`), the control endpoint
  (`src/control.rs`), diagnostics, the inventory and scenario code walk
  `raw_ref().archetypes()` and `components_of` to list an entity's components
  and count them; all would show sparse components as missing.

**Maintenance cost.** The prototype is about 250 lines (plus tests) without stamps,
parallel iteration, views, the C ABI or tools. An integrated version would
change nearly every file of `ponko_ecs` (storage, world, query, cache,
dynamic filters, change, view, borrow, raw), four FFI modules, the C and
C++ headers, and five engine tools; by estimate several thousand lines
with tests, new unsafe code under Miri, and from then on two implementations
to keep equivalent for every query feature, filter and C entry point.

## Public query API implications

What would change for users if sparse storage were opt-in per component:

- **Choosing the storage.** `Component` is implemented for every
  `Send + Sync + 'static` type, so a per-type constant would break the
  blanket implementation. The choice would be a runtime call before first
  use, like `World::track_changes` (for example `World::store_sparse::<T>()`),
  plus a C registration option. Users then have to choose per component
  between two storages whose costs are opposite (above), which works
  against "one obvious way to do each thing".
- **Same query syntax, different costs.** `query::<(&mut Pos, &Burning)>()`
  would look the same, but `With<Stunned>` on a sparse tag would become
  a per-row test: no longer free for non-matching archetypes, and `count()`
  would walk rows.
- **Chunk-level APIs can't include sparse components**: `Chunk`,
  `raw().query_chunks`, `raw().for_each_chunk`, `Archetype::column_ptr`
  and the C `PonkoChunk` queries expose contiguous columns per archetype.
  They would reject sparse ids with an error.
- **Archetype-level introspection becomes partial**: `components_of(e)`
  returns the archetype's id slice and couldn't list sparse components
  without a new API (an iterator, or an owned list), and
  `raw_ref().archetypes()` / `Archetype::has` would describe only the dense
  part of a world.
- **Iteration order** of queries driven from a sparse set would differ
  from archetype order (allowed, as above).

Archetype storage keeps all of these as they are today.

## Recommendation

Keep archetype storage only. Reasons, from the measurements:

1. At realistic densities (1 to 5 % of entities per optional component),
   a whole tick is faster on archetypes or ties with sparse sets, and the
   selective queries such components exist for are 5 to 22× faster on
   archetypes thanks to whole-table skipping and the query index.
2. Where archetypes lose (20 % density, 32,000 toggles per tick), keeping
   the flags as fields of one dense component does as well as the sparse
   sets, today, with no engine change. That is the advice for games with
   this pattern: put components that toggle every few frames into a dense
   component's fields (bits, or an `Option` per effect), and keep marker
   components for state that changes rarely. The measured layout keeps the
   effects inline in a 68-byte component that every filtered pass reads in
   full; keeping the bits in their own small component would likely make
   those passes cheaper (not measured).
3. A hybrid's integration cost is large, reaches the C ABI and the tools,
   and adds a per-component choice with opposite performance profiles.

**What would change the answer:**

- A profiled game whose frame time is dominated by structural changes of
  optional components (thousands of live combinations and tens of
  thousands of toggles per tick), where packing the flags into a dense
  component isn't possible: for example components owned by separate
  plugins that can't share one component, or large per-component data.
- The archetype-side fixes below were tried first and weren't enough.
- If sparse storage is added later, the narrowest version: Rust only,
  opt-in per component by a runtime call, no change stamps at first, sparse
  ids rejected by chunk and C queries with a clear error, `with`/`without`
  on sparse components as row filters, and the C gap recorded.

## Empty archetypes: skipped and reclaimed (#347, #348)

Two follow-ups from this investigation, done on 2026-11-14 and described in
`crates/ponko_ecs/src/slots.rs`:

- **#348, skipping empty archetypes in queries.** While a world has many
  empty archetypes (more than 16, and more than one per four with
  entities), a scanning query visits a list of the archetypes with
  entities, rebuilt from occupancy bits when it is first needed after a
  structural change, and handed to the query loop the way the archetype
  index hands it its lists. Testing a bit in that loop instead was measured
  first and rejected: scans of 1,024 archetypes with 1,000 entities each
  (`query-cache/1024/1000/*/scan`) got 12 to 23 % slower, and
  `iter/ponko-iter` went from 0.81 to 1.97 ms because `Query::iter`
  compiled differently.
- **#347, reclaiming empty archetypes.** A world keeps at most 64 empty
  archetypes, or as many as it has archetypes with entities if that is more.
  Past that, a second-chance sweep over the slots reclaims those empty
  longest; the slot is reused for the next new component set, with the
  reclaimed storage (id lists, column headers, edge maps, and pooled column
  buffers and entity lists). Edges to a reclaimed archetype go stale with
  its slot's generation.

**Setup.** Same machine and toolchain as above (rustc 1.99.1), engine `main`
at `3b8b1c17` against this branch, benchmark code of this branch in both
binaries (the `before` binary is it built against `main`'s
`crates/ponko_ecs`). The machine was busy, load averages 19 to 36 during the
final runs and up to 83 during earlier ones. Timings: three full runs per
binary, alternating, median of the per-run medians with the lowest single
measurement in parentheses. Between runs, the same case moved by up to 10 %
for most query rows, by up to 2.4× for the 20 % `churn` and `frame` rows,
and one run of `dense/20pct/arch` and `data/5pct/arch-scan` each was 3 to
5× slower than the other two (a busy core; the median drops it). Counts (archetypes, bytes, allocations) are exact and the same in
every run.

**Archetypes, memory and allocations** (`sparse-set/mem`, which now also
prints heap allocations per tick since the previous checkpoint, and runs the
`arch` world to tick 9,600):

| Density | Archetypes at tick 600 / 2,400 / 9,600, before | after | KiB at tick 600 / 9,600, before | after | Allocations per tick, ticks 300–600 / 4,800–9,600, before | after |
| --- | --- | --- | --- | --- | --- | --- |
| 1 % | 864 / 1,332 / 2,304 | 407 / 418 / 418 | 4,103 / 5,050 | 3,816 / 3,834 | 5.1 / 1.8 | 0.83 / 0.00 |
| 5 % | 8,080 / 11,506 / 16,408 | 2,750 / 2,750 / 2,750 | 12,062 / 18,915 | 8,132 / 8,363 | 105 / 11.6 | 2.2 / 0.10 |
| 20 % | 56,057 / 61,600 / 64,213 | 26,430 / 26,524 / 26,527 | 65,909 / 75,083 | 37,522 / 37,897 | 679 / 20.1 | 18.6 / 0.02 |

The archetype count levels off at about twice the archetypes with entities
(177, 1,293 and 13,115 at tick 600), and the bytes with it. Before, both
kept growing. Allocations fall to nothing or nearly: what is left is an
archetype outgrowing every buffer the world has kept of that size, which
`main` does too, since buffers never shrink. In the timed `churn` and
`frame` cases (ticks 66 to 95, right after settling), the allocations per
tick went from 33, 278 and 3,375 to 1.4, 9.9 and 97 (1, 5 and 20 %).

A world that walks through more component sets than it keeps empty
archetypes for allocates nothing once settled
(`walking_through_more_component_sets_than_are_kept_stops_allocating` in
`crates/ponko_ecs/tests/archetype_alloc.rs`: 6 entities walking through all
256 sets of 8 optional components, beside 100 that stay put, in fewer than
256 archetype slots).

**Timings, milliseconds** (`arch` and `arch-scan`; `sset` and `flags` don't
change):

| Case | Before | After |
| --- | --- | --- |
| `dense/20pct/arch` | 0.137 (0.128) | 0.113 (0.100) |
| `dense/20pct/arch-scan` | 0.144 (0.131) | 0.100 (0.087) |
| `data/20pct/arch-scan` | 1.493 (1.418) | 1.222 (1.064) |
| `and/20pct/arch-scan` | 1.459 (1.354) | 1.073 (0.958) |
| `without/20pct/arch` | 0.840 (0.764) | 0.699 (0.556) |
| `without/20pct/arch-scan` | 0.873 (0.730) | 0.632 (0.581) |
| `data/5pct/arch-scan` | 0.065 (0.061) | 0.055 (0.052) |
| `churn/20pct/arch` | 6.32 (4.65) | 7.97 (4.18) |
| `frame/20pct/arch` | 7.81 (7.28) | 9.95 (7.92) |
| `frame/20pct/arch-scan` | 11.50 (9.29) | 9.99 (7.96) |
| `churn/5pct/arch` | 0.523 (0.413) | 0.494 (0.416) |
| `frame/5pct/arch` | 0.719 (0.666) | 0.733 (0.658) |

Scans of worlds with many empty archetypes are faster: the dense loop at
20 % takes about 0.7 to 0.8 of the time (minimums 0.100 against 0.128 and
0.087 against 0.131 ms), and the `arch-scan` filters about 0.7 to 0.8.
Queries that use the archetype index (`data`, `and` with `arch`) don't
change beyond noise, nor does anything at 1 %. `churn` and `frame` at 20 %
are ties within their noise: their medians are 1.26× and 1.27× but their
minimums 0.90× and 1.09×, and `frame` with `arch-scan` is faster; a world
that reclaims rebuilds about 670 archetypes per tick there (the ones its
entities come back to after they were reclaimed), where `main` mostly
follows old edges, and allocates instead of rebuilding when a set is new.
`iter/` ties everywhere (ratios 0.98 to 1.00, and `iter/ponko-iter` 0.823
→ 0.765 ms, which the earlier runs of this work did not repeat), and so does `membership/` (`crates/ponko_bench/MEMBERSHIP.md`).

Two designs were measured and dropped on the way. Removing the edges to a
reclaimed archetype from its neighbours, eagerly, touched the edge maps of
about 20 archetypes scattered over the world per reclaim: `churn/20pct/arch`
went from 3.9–4.9 ms (minimums, before) to 4.9–8.0 ms. And a queue of empty
archetypes, oldest first, cost about 3 ns on every move that empties an
archetype (`membership/transition/n2-sparse/warm-1-edge`, 15.3 → 19 ns).

## Follow-ups inside archetype storage

Not done in this investigation (no default storage changes here):

- **Reclaim empty archetypes**, or at least their column buffers, so a
  churning world doesn't keep every combination it ever had (864, 8,080 and
  56,057 archetypes after 600 ticks for 177, 1,293 and 13,115 live ones) or
  allocate on every tick. Done in #347 (above).
- **Skip empty archetypes cheaply in queries**: at 20 %, the dense loop
  reads about 40,000 archetype structs (128 bytes each) to find 13,000 with
  entities. Done in #348 (above).
- **Batch structural changes** that move many entities between the same
  two archetypes (`add-remove` above: 100,000 single moves).
