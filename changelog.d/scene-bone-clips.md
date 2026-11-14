### Fixed

- `ponko_unity`: when a prefab is saved in a posed state, the clips of bones it
  keeps under their original parent no longer get that pose added on top, so a
  rig saved at frame 0 of a clip no longer plays every bone rotated twice.
