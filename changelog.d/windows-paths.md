### Fixed
- On Windows, a Tiled or LDtk level loaded by an absolute path (`C:\...\level.tmx`)
  finds the tilesets and images it names next to it, instead of looking for them
  in `assets/`; `..` in such references no longer drops the drive. glTF files
  loaded by such a path find their buffers and images next to them too.
