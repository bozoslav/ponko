### Breaking changes
- `schedule::Stage::systems` returns `&[usize]` instead of a `Range<usize>`.
  A batch's systems need not be neighbours any more (see Changed), so a
  stage lists them in the new `Stage::members` field, in registration
  order; `first` and `len` stay, as its first system and how many it has.
  Instead: iterate the slice (`.iter().copied()`) or read `stage.members`;
  `stage.systems().contains(&i)` works as before.
  Before:
  ```rust
  for i in stage.systems() {
      println!("{}", plan.systems[i].name);
  }
  ```
  After:
  ```rust
  for &i in stage.systems() {
      println!("{}", plan.systems[i].name);
  }
  ```

### Changed
- The parallel scheduler (`Game::parallel_systems`) batches view systems by
  their dependencies instead of only when they are next to each other.
  Between phases, explicit barriers and systems that run alone, each view
  system goes to the earliest stage after every earlier system whose work
  it must see (a conflict or `Game::order`), and never ahead of an earlier
  system whose order would show (`SystemAccess::conflicts_with`, such as two
  senders of one event type). A system that interacts with nothing
  registered between may so run ahead of systems registered before it.
  Results, change ticks and the order of commands and events are the same
  as with the scheduler off, whatever the number of threads. Producers and
  consumers registered in pairs now take two stages instead of five
  (`schedule/interleaved/4x16000/heavy`: see `crates/ponko_bench/SCHEDULE.md`).
  `&mut Ctx` systems still split batches, and the scheduler stays off by
  default. Systems that talk through something undeclared need
  `Game::order` to keep their order, as they already needed it to stay out
  of one batch.

### Added
- Diagnostics that say which system blocked a batch and why:
  `PlannedSystem::boundary` (per system, what kept it out of the stage
  before its own), `Boundary::KeepsOrder`, `SystemDiagnostics::blocked`
  (`BlockedDiagnostics`) and `StageDiagnostics::systems` in
  `ctx.ecs_diagnostics()` and the `ecs_diagnostics` tool. The printed plan
  names each batch member's own reason when it differs from the stage's.
