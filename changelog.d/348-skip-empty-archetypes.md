### Changed
- Queries that scan skip empty archetypes cheaply: in a world with many
  empty archetypes (more than 16, and more than one per four with
  entities), they visit a list of the archetypes with entities, rebuilt
  after a structural change, instead of reading every archetype. The
  dense loop of the 20 % sparse-set benchmark takes about two thirds of
  the time it did (`crates/ponko_bench/SPARSE_SET.md`).
