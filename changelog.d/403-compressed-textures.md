### Added
- Block-compressed textures (feature `compressed-textures`): `load_texture`,
  `stream_texture`, `load_texture_array` and hot reload read KTX2 files of
  BC1–BC5, BC7, ETC2/EAC and ASTC 4x4 blocks (Zstandard or zlib
  supercompressed) and DDS files of BC blocks, and upload them as blocks
  with their own mipmaps. Color textures in BC7 take a quarter of RGBA8's
  video memory, normal maps in BC5 a quarter (the 3D shader rebuilds Z)
  and masks in BC4 an eighth; the asset inventory and residency budgets
  count their compressed bytes, and streaming keeps a smaller stored mip
  level resident. A GPU without the format, or a size that isn't a
  multiple of 4, gets the texture decoded to RGBA8 at load. Without the
  feature, a KTX2 file fails with an `Unsupported` error naming it.
- Basis Universal UASTC KTX2 files (feature `basis-universal`), transcoded
  at load to what the GPU samples: BC7/BC5, ASTC, ETC2 or RGBA8. Not in web
  builds, where they are an `Unsupported` error.
- `ponko textures`: converts a folder's PNG and JPEG images into KTX2 files
  with mipmaps (BC7, BC5, BC4, UASTC or RGBA8), with per-texture kinds
  (`color`, `linear`, `normal`, `mask`, `ui`, `skip`) from a
  `textures.json` next to them; `--check` fails when a file is missing or
  stale.
- The C library has the same `compressed-textures` and `basis-universal`
  features; `ponko_ctx_load_texture` then loads `.ktx2` files.
