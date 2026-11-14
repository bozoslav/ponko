# Benchmark history

Written by `bench_track history` (crates/ponko_bench, "Tracking results over time"). One file per commit on `main` and machine: `<machine>/<commit>.json`; releases in `releases/<tag>/`.

## github-ubuntu-24.04-x64

Median instructions per operation. ▲ slower, ▼ faster than the commit below it, beyond the larger of 1.0% and 2 times the spread.

| commit | date | `ecs/iter/10k` | `ecs/query-filtered/10k` | `ecs/get/10k` | `ecs/spawn/10k` | `ecs/despawn/10k` | `sim/tick/10k` | `render/collect/10k` |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `b03d10c82b` | 2026-11-14 | 33,015 | 20,641 | 880,015 | 2,902,129 | 1,721,823 | 119,859 | 4,616,858 |
| `eb884faf97` | 2026-11-14 | 33,015 | 20,641 | 880,015 | 2,902,129 | 1,721,823 | 119,858 | 4,616,856 |
| `a14ba7da21` | 2026-11-14 | 33,015 | 20,641 | 880,015 | 2,902,127 | 1,721,823 ▲+7.0% | 119,891 | 4,626,698 |
| `81d54c080c` | 2026-11-14 | 32,992 | 20,623 | 880,015 | 2,881,635 | 1,609,416 | 119,815 | 4,626,155 |

# Releases

## github-ubuntu-24.04-x64

Median instructions per operation. ▲ slower, ▼ faster than the release below it, beyond the larger of 1.0% and 2 times the spread.

| release | date | `ecs/iter/10k` | `ecs/query-filtered/10k` | `ecs/get/10k` | `ecs/spawn/10k` | `ecs/despawn/10k` | `sim/tick/10k` | `render/collect/10k` |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `v0.1.0` | 2026-11-13 | 32,992 | 20,624 | 880,015 | 2,881,635 | 1,609,416 | 119,703 | 4,706,259 |

