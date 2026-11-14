# The parallel system scheduler (#138)

Run `cargo run --release -p ponko_bench --bin ponko_bench -- schedule`
(`src/schedule.rs`; `-- schedule/` for the scheduler rows only,
`-- schedule-off/` for the before/after rows). Every benchmark is a
headless game. The time is one whole simulation tick, measured inside the
game from before its first system to after its last, so it includes the
scheduler's own work (clocks, event slots, the barrier after each batch).
60 warm-up ticks, then 400 timed ticks (100 or 200 for the biggest worlds).

The systems are view systems (functions of `&mut SystemView`) over `Val<N>` components,
two floats each. *Light* is `v += w * dt` (well under a nanosecond per
entity); *heavy* is a few transcendental calls (40 to 60 ns per entity).
`SxE` means `S` systems, each over its own component of `E` entities.

| Mode | What it is |
|---|---|
| `seq` | scheduler off (the default): every system runs alone, in order |
| `auto` | `parallel_systems(true)`, default threshold (50 µs) |
| `threads` | `parallel_systems(true)`, threshold zero: every batch goes to the thread pool |

Apple M4 Max (10 performance + 4 efficiency cores), macOS 27.0.1, rustc
1.99.1, release profile, default features, 2026-11-11. Medians of one run
of the final code. The machine was in use for other work (load average 7
to 15), so treat the numbers as indicative and rerun before comparing. Two
earlier runs of the same benchmarks agreed on every row up to
`sweep/8x4000/heavy` within 10%; in those, some larger rows were disturbed
by other load (one read 51.6 ms instead of 28.7 ms), which this run did
not show.

## Scheduler off: before and after

The acceptance asks that simple games don't get slower. With the scheduler
off (never mentioned by the game), against `origin/main` at `f3bef8ef`,
which has view systems but no scheduler; `ctx` are `&mut Ctx` systems,
`views` are view systems:

| Benchmark | Before (`f3bef8ef`) | After |
|---|---|---|
| schedule-off/ctx/64x100/light | 8 µs | 8 µs |
| schedule-off/views/64x100/light | 8 µs | 8 µs |
| schedule-off/ctx/8x16000/light | 21 µs | 21 µs |
| schedule-off/views/8x16000/light | 21 µs | 21 µs |
| schedule-off/ctx/8x250000/light | 321 µs | 323 µs |
| schedule-off/views/8x250000/light | 321 µs | 322 µs |

No difference. A caution about these small rows: on this machine a
benchmark process that starts cold (right after a build, or while other
work holds the fast cores) reads the first rows at 11 to 15 µs and 24 to
27 µs instead of 8 and 21 µs. That happened in three of five runs of the
after code, and the same rows measured with the scheduler explicitly off
(`schedule/tiny/64x100/light/seq`) went from 15 µs to 8 µs between two
runs of one binary. The table gives the settled runs (two in a row agreed
to the microsecond); `origin/main` was measured once and was settled. With
the scheduler off the engine also never starts rayon's thread pool
(`tests/system_schedule_off_pool.rs`).

## Overhead: many tiny systems

| Benchmark | `seq` | `auto` | `threads` |
|---|---|---|---|
| tiny/4x100/light | <1 µs | <1 µs | 22 µs |
| tiny/16x100/light | 2 µs | 2 µs | 34 µs |
| tiny/64x100/light | 8 µs | 8 µs | 49 µs |
| sweep/8x100/heavy | 14 µs | 14 µs | 32 µs |
| chain/8x16000/light | 29 µs | 28 µs | 28 µs |

Handing a batch to the thread pool costs about 20 to 45 µs at the median
(9 to 24 µs at best, 50 to 90 µs at the 95th percentile): waking the
workers and waiting for them. For systems this small that is many times
the work itself, which is why `auto` keeps such batches on the game's
thread. There, a batch costs the same as running its systems one at a time
(the `seq` and `auto` columns agree to the microsecond), and a batch
allocates nothing in steady state (`tests/system_schedule_alloc.rs`).

In `chain`, every system reads what the one before wrote, so the plan has
eight stages of one system and nothing ever runs together: the scheduler
costs nothing there in any mode.

## The crossover

Eight independent systems, growing worlds:

| Benchmark | `seq` | `auto` | `threads` | `auto` vs `seq` |
|---|---|---|---|---|
| sweep/8x100/light | 1 µs | 1 µs | 26 µs | same |
| sweep/8x1000/light | 2 µs | 2 µs | 32 µs | same |
| sweep/8x4000/light | 6 µs | 6 µs | 33 µs | same |
| sweep/8x16000/light | 21 µs | 21 µs | 39 µs | same |
| sweep/8x64000/light | 81 µs | 59 µs | 58 µs | 1.4× |
| sweep/8x250000/light | 322 µs | 113 µs | 114 µs | 2.8× |
| sweep/8x1000/heavy | 291 µs | 98 µs | 97 µs | 3.0× |
| sweep/8x4000/heavy | 1.66 ms | 0.31 ms | 0.31 ms | 5.3× |
| sweep/8x16000/heavy | 7.15 ms | 1.04 ms | 1.04 ms | 6.9× |
| sweep/8x64000/heavy | 28.74 ms | 3.82 ms | 3.83 ms | 7.5× |

Threads lose up to 21 µs of total work (8x16000 light: 39 µs instead of
21 µs) and win from 81 µs (8x64000 light). What other threads can save is
the batch's work without its slowest system, which someone has to run
anyway: 18 µs in the first case, 71 µs in the second. The default
threshold sits between them:

**A batch goes to other threads when the usual run time of its systems,
not counting the slowest, is at least 50 µs** (`DEFAULT_PARALLEL_THRESHOLD`,
changed with `Game::parallel_threshold`). "Usual" is a moving average of
each system's measured run time, so a batch moves to threads, or back, as
its systems get slower or faster. Below the threshold it runs on the
game's thread in registration order. Either way the result is the same;
see the guide's "Parallel systems" chapter.

In every row `auto` matches the better of `seq` and `threads`.

## Representative schedules

| Benchmark | `seq` | `auto` | `threads` | `auto` vs `seq` |
|---|---|---|---|---|
| pair/2x64000/heavy | 7.16 ms | 3.65 ms | 3.66 ms | 2.0× |
| mixed/50000 | 8.45 ms | 5.71 ms | 5.75 ms | 1.5× |
| uneven/1x64000+7x100 | 3.63 ms | 3.62 ms | 3.62 ms | same |

- `pair`: two big systems is the smallest batch there is, and it halves.
- `mixed`: one heavy system alone in its phase, then two medium systems
  that read what it wrote, two heavy ones, a `&mut Ctx` system and two
  cheap ones. Only the batch of four before the `Ctx` system gains; the
  heavy system in its own phase and the `Ctx` system bound the speedup
  (1.5×, not 4×).
- `uneven`: one big system next to seven tiny ones has nothing to gain,
  since the big one sets the time. `auto` sees that (the work besides the
  slowest is far under 50 µs) and stays on the game's thread. Forcing it
  to threads gains nothing either; in one earlier, disturbed run it was
  slower (5.9 ms median, 14.5 ms at the 95th percentile).

## Batching by dependencies (#388)

Batches used to form only from neighbours in the registration order. Now
each view system goes to the earliest stage after every earlier system
whose work it must see, so a system that interacts with nothing
registered between joins an earlier batch (the guide's "Parallel systems",
"Stages and batches"). `schedule/interleaved/4x16000/heavy` registers four
producers and four consumers in pairs, `p0 c0 p1 c1 p2 c2 p3 c3`, where
`ck` reads what `pk` writes: neighbours give five stages (`p0`, `c0 p1`,
`c1 p2`, `c2 p3`, `c3`), dependencies give two (every producer, then every
consumer).

Before is this branch just before the planner change (`0976d3bc`, the
benchmark added to the 0.2 batch 4 code), after is the new planner; one
release build each, the same machine and toolchain as above (Apple M4 Max,
macOS 27.2.1, rustc 1.99.1), 2026-11-14. The machine was very busy (other
agents compiling, load average 38 to 60, peaks over 100), so the two
binaries ran alternately, five times each, on the rows below; the table
gives the median of each run's median, and the range of the three best
runs in brackets.

| Benchmark | Before | After |
|---|---|---|
| interleaved/4x16000/heavy/seq | 3.69 ms | 3.71 ms |
| interleaved/4x16000/heavy/auto | 3.71 ms (3.66 to 3.71) | 1.00 ms (0.98 to 1.00) |
| interleaved/4x16000/heavy/threads | 5.42 ms (3.72 to 5.42) | 1.04 ms (0.99 to 1.04) |
| chain/8x16000/light/auto | 30 µs | 29 µs |
| tiny/64x100/light/auto | 10 µs | 10 µs |
| sweep/8x16000/heavy/auto | 1.90 ms (1.29 to 1.90) | 4.36 ms (1.32 to 4.36) |
| mixed/50000/auto | 12.9 ms (5.8 to 12.9) | 16.6 ms (5.8 to 16.6) |

- `interleaved` gains 3.7× over the old planner: before, each batch was a
  heavy producer next to a light consumer, under the threshold, so `auto`
  ran everything in order and matched `seq`; now the four producers share
  one batch. Every run of the new code whose machine was quiet enough read
  0.98 to 1.04 ms; the slow ones (5.5 to 8.9 ms) came with load spikes,
  as did the old code's (up to 14.9 ms with threads).
- `chain`, `tiny`, `sweep` and `mixed` have the same plan with either
  planner (a chain, one batch, one batch, and a `&mut Ctx` system that cuts
  the phase). Their best runs agree within 3%; the medians of `sweep` and
  `mixed` swing by several times between runs of the same binary under
  this load, so they show noise, not a change. The planner runs once when
  the game starts, and the executor does the same work per stage as before
  (`tests/system_schedule_alloc.rs` still finds no allocation per tick).
- With the scheduler off nothing changed: `seq` rows agree, and the plan
  is not even built.

**The reference games and the simulation examples gain nothing**, with
either planner: `examples/coin_dash`, `examples/ember_hop`, `ants`, `boids`
and `battle` have only `&mut Ctx` systems, which run alone, so their plans
have no batch at all. **Decision: the scheduler stays off by default.** It
only helps view systems with real work, the games we ship have none,
turning it on starts rayon's thread pool, and `&mut Ctx` systems still cut
segments (their declarations are only checked in debug builds, so no
system may move across one).

## When it doesn't pay off

- Systems that take microseconds: nothing to gain, and `auto` doesn't try.
- Schedules that are chains (each system reads the previous one's output),
  or where `&mut Ctx` systems sit between the declared ones: there are no
  batches. (View systems that only sit apart in the registration order
  don't matter any more: see "Batching by dependencies".) `Game::schedule_plan()` shows the stages and why each starts.
- One dominant system: split its work with `par_for_each` inside the
  system instead (see [PAR_ARCHETYPES.md](PAR_ARCHETYPES.md)).
- Light, memory-bound systems gain less than compute-bound ones (2.8×
  against 7.5× with eight systems on ten performance cores).
- While profiling (`Game::profile_headless`, `Perf::set_profiling(true)`),
  batches stay on the game's thread so each system can be charged its own
  allocations; and on the web there are no threads to use.
