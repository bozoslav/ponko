### Breaking changes
- An index into `RawWorldRef::archetypes` is valid only until the next change.
  It names an archetype only until the next structural change: a world now
  reclaims archetypes that stayed empty and builds new component sets in
  their slots; a reclaimed slot shows as an empty archetype without
  components. Query order, which follows the archetype slots, can differ from
  before (it is still the same for the same operations).
  Instead: don't keep an index across spawns, despawns, inserts or removes;
  find the archetype again by its components (`Archetype::has`,
  `Archetype::component_ids`).
  Before:
  ```rust
  let index = world.raw_ref().archetypes().iter().position(|a| a.has(id)).unwrap();
  world.spawn((Position::default(),));
  let count = world.raw_ref().archetypes()[index].len();
  ```
  After:
  ```rust
  world.spawn((Position::default(),));
  let count = world.raw_ref().archetypes().iter().find(|a| a.has(id)).map_or(0, |a| a.len());
  ```

### Changed
- A world reclaims empty archetypes: it keeps at most 64 of them, or as
  many as it has archetypes with entities if that is more, and reclaims
  the one empty longest past that. The reclaimed slot and storage are
  reused for the next new component set, so a world whose entities keep
  visiting new component combinations levels off in archetypes and memory
  and stops allocating once it settles (`crates/ponko_bench/SPARSE_SET.md`).
