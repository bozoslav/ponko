### Fixed
- `World::despawn` no longer runs extra instructions on every call since
  empty archetypes are reclaimed: whether the archetype emptied is decided
  from the removed row, off the common path. Under cachegrind on x86-64 the
  despawn benchmark is back within 1 % of its count before reclaiming.
