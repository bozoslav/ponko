# Drawing

There are two ways to put things on screen. Both end up in the same batched
renderer.

## Sprites as components

Give an entity a `Transform2D` and a `Sprite`, and the built-in
`draw_sprites` system draws it every frame:

<div class="lang-pair">
<div>

```rust
let crab = ctx.load_texture("crab.png");
ctx.world.spawn((
    ponko::d2::Transform::at(vec2(400.0, 300.0)).rotated(0.3).scaled(vec2(2.0, 2.0)),
    ponko::d2::Sprite::new(crab).with_color(Color::RED).with_z(1.0),
));
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:sprites}}
```

</div>
</div>

- `Transform2D`: `position`, `rotation` (radians, clockwise on screen),
  `scale` (negative flips).
- `Sprite`: `texture`, `color` (tint), `z` (higher is on top), `pivot`
  (`(0.5, 0.5)` is the center).

## Immediate drawing

For UI, effects and quick prototypes, draw directly every frame:

<div class="lang-pair">
<div>

```rust
# let crab = ctx.load_texture("crab.png");
ctx.gfx.clear(Color::hex(0x203040));
ctx.draw(crab, vec2(100.0, 100.0));
let params = DrawParams::default()
    .with_rotation(0.5)
    .with_scale(vec2(2.0, 2.0))
    .with_color(Color::RED);
ctx.gfx.draw_ex(crab, vec2(200.0, 100.0), params);
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:immediate}}
```

</div>
</div>

Coordinates are world units, which are window pixels (`(0, 0)` at the
top-left) until you move the camera; see [Camera, text and UI](camera-ui.md).

`DrawParams` (and `TextParams` for text) start from `default()` and change
what they need with `with_*` methods (`with_scale`, `with_rotation`,
`with_color`, `with_z`, `with_pivot`, `with_screen`, `with_region`). They are
`#[non_exhaustive]`, so they can't be written as struct literals.

## Textures

- `ctx.load_texture("path.png")` loads an image file. PNG and JPEG load
  in every default build, with no `3d` feature (see [Image formats](#image-formats)).
- `ctx.gfx.create_texture(w, h, &rgba)` makes one from pixels.

- `ctx.gfx.update_texture(tex, &rgba)` replaces a texture's pixels, for
  images that change while the game runs (a minimap, a heat map, a
  simulated field). The ants example draws its pheromone field this way
  every frame.

`Texture` is a small `Copy` handle; store it in components freely.

```cpp
{{#include ../../examples/cpp/guide.cpp:pixels}}
```

In C++, `Sprite` values are `ponko::sprite` (`sprite_of(texture, tint, z)`).
They can be spawned, read and set, but not used as query columns, because
the engine stores sprites in a Rust layout. `Transform2D` can.

## Blend modes and sprite shaders

A sprite blends with what's behind it by its alpha. Add a `BlendMode` next to
its `Sprite` (or pass one with `DrawParams::with_blend`) to change that:

| `BlendMode` | What it does | For |
|---|---|---|
| `Alpha` (default) | covers what's behind by the pixel's alpha | most sprites |
| `Additive` | adds the color, weighted by alpha; never darkens | glows, fire, sparks, light |
| `Multiply` | multiplies what's behind by the color; white changes nothing | shadows, tinted glass |
| `Premultiplied` | like `Alpha`, for colors already multiplied by their alpha | premultiplied art and shaders |

For a look of your own, write a **sprite shader**: WGSL with one function,
`fn fragment(p: SpritePixel) -> vec4<f32>`, that returns each pixel's color.
A `SpriteMaterial` puts it together with four `vec4` parameters and two extra
textures; add it next to a `Sprite`, or pass it with
`DrawParams::with_material`. This one burns a sprite away with a glowing edge
(the `sprite_effects` example animates it, next to an additive glow and an
outline):

```wgsl
{{#include ../../assets/shaders/sprite_dissolve.wgsl}}
```

<div class="lang-pair">
<div>

```rust
# use ponko::d2::{BlendMode, SpriteMaterialDesc};
let crab = ctx.load_texture("crab.png");
let noise = ctx.load_texture("noise.png"); // grey: where it burns first
// A glow that brightens what's behind it:
ctx.world.spawn((
    ponko::d2::Transform::at(vec2(400.0, 300.0)),
    ponko::d2::Sprite::new(crab),
    BlendMode::Additive,
));
// A sprite with a shader of its own:
let dissolve = ctx.gfx.load_sprite_shader("shaders/sprite_dissolve.wgsl");
let burn = ctx.gfx.create_sprite_material(
    SpriteMaterialDesc::new(dissolve)
        .with_param(1, vec4(1.0, 0.5, 0.1, 1.0)) // the edge's color
        .with_texture(0, noise),
);
ctx.world.spawn((ponko::d2::Transform::at(vec2(200.0, 300.0)), ponko::d2::Sprite::new(crab), burn));
// Immediate draws take both too:
let glow = DrawParams::default().with_blend(BlendMode::Additive).with_material(burn);
ctx.gfx.draw_ex(crab, vec2(100.0, 100.0), glow);
// Each frame: burn a little further.
if let Some(desc) = ctx.gfx.sprite_material_desc(burn) {
    let t = (ctx.time.elapsed() as f32 * 0.25).fract();
    ctx.gfx.update_sprite_material(burn, desc.with_param(0, vec4(t, 0.0, 0.0, 0.0)));
}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:sprite_materials}}
```

</div>
</div>

What a sprite shader can use:

- `p.color`: what the built-in look would draw (texture × tint), and its
  parts `p.texture` and `p.tint`; `p.uv` (in the texture), `p.local` (0 to 1
  across the sprite) and `p.world` (world units).
- `material.params[0]` to `[3]`: the material's four `vec4<f32>`.
- `sprite_sample(uv)` and `sprite_texel()` (the size of one texel in uv, for
  outlines and blurs), and `material_sample0(uv)` and `material_sample1(uv)`
  for the extra textures (white when the material has none).

It returns a color with straight alpha (premultiplied for
`BlendMode::Premultiplied`) and may `discard`. Every binding is the
engine's, so a sprite shader declares no `@group`/`@binding` and no entry
points; it works with every blend mode.

Loaded sprite shaders hot reload like textures. One that doesn't compile is
an error with the line in your file (`dissolve.wgsl:6:12: error: ...`), also
listed by `ctx.gfx.shader_status()` with kind `ShaderKind::Sprite` (see
[Shader errors and reloads](3d.md#shader-errors-and-reloads)). An edit that
doesn't compile keeps the last good version drawing. Shaders are checked against what every supported
GPU runs, WebGL2 included, so an error shows up on your machine rather than
in a player's browser.

Sprites are batched by texture, blend mode and material together, so many
sprites with one material cost one draw call, and sprites without a blend
mode or material draw as before. Each different material or blend mode in a
row is its own draw call.

## Particles

Dust, sparks, smoke and pickup flashes come from one component: a
`d2::ParticleEmitter` next to a `d2::Transform`. It spawns small sprites at a
`rate` per second or in bursts, moves them with `gravity` and `drag`, and
draws them with the sprites. Its settings use the same words as the 3D
emitter's ([Particles](3d.md#particles)), in pixels: a lifetime, speed, size
and rotation range each, a spawn shape, a size curve (`ParticleCurve`) and a
color gradient (`ParticleGradient`) over each particle's life, an optional
image or sprite sheet, and world or local space. This campfire is three
emitters: smoke under a sprite, fire and sparks over it.

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/particles2d.rs:particles2d}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:particles2d}}
```

</div>
</div>

The `particles2d` example (`cargo run --example particles2d`) adds a trail that
follows the mouse and confetti on click.

- **Where they start**: `ParticleShape::Point`, `Circle { radius }` (moving
  outward), `Cone { angle }` (up to `angle` radians from up) or
  `Rect { half_extents }`. "Up" is toward the top of the screen (`-Y`), turned
  by the transform's rotation.
- **Drawing order**: every particle of an emitter is a sprite at the
  emitter's `z`, so emitters sort with sprites and tilemaps by `z`
  (higher on top), and within an emitter the newest particles draw on top.
  The emitter's `blend` takes the sprite [blend modes](#blend-modes-and-sprite-shaders)
  (`Additive` for fire and sparks), and `material` a `SpriteMaterial`; each
  emitter is one batch.
- **Images**: without `texture`, particles are white squares tinted by the
  gradient. `with_sheet(columns, rows)` plays a sprite sheet's frames over
  each particle's life. `size` is the width; the height follows the image's
  (or frame's) shape.
- **World or local**: in `ParticleSpace::World` (the default), a particle
  stays where it was born when the emitter moves; in `Local` it moves,
  turns and scales with the emitter.
- **When it runs**: the engine advances emitters at the end of the
  `Animate` phase (`Systems::UPDATE_PARTICLES`) and draws them after the
  sprites (`Systems::DRAW_PARTICLES`). `burst(n)` queues more at any time
  (C++: `ctx.particle_burst2d(e, n)`); `looping: false` with a `duration`
  stops the rate after that many seconds. Headless runs skip it unless
  `Game::headless_frames` is on; `emitter.update(dt, &transform)` steps one
  by hand. Equal seeds and frame steps give equal particles.
- **Cost**: each emitter's pool is allocated when it is made
  (`max_particles`, default 1,024), and expired particles free their slots
  without allocating. Emitters advance on all cores. On an Apple M4 Max, one
  frame of about 100,000 living particles (spawning, moving, expiring,
  drawn as sprites, ordered and batched; `cargo run --release -p ponko_bench
  -- particles2d`) took 0.84 ms of CPU from ten emitters and 0.83 ms from one;
  with the emitters at two different `z`, sorting the frame's sprites made it
  2.0 ms.

Emitters are reflected as `"ParticleEmitter2D"`, so the inspector, the MCP
tools and scenes see and edit their settings; the image is saved by its file
name like a sprite's. Living particles aren't saved (a loaded emitter starts
empty), and emitters with a `SpriteMaterial` or an image made in code aren't
saved, as for sprites.

## 2D lights and shadows

Lights are components too. A `d2::PointLight2D` shines from its entity's
`d2::Transform`, a `d2::SpotLight2D` shines a cone along the transform's
rotation, and a `d2::DirectionalLight2D` (a sun or moon, no transform needed)
lights the whole view from one direction. An ambient light
(`ctx.gfx.set_ambient_light2d`) is what places no light reaches get. With
`shadows: true`, a light is blocked by `d2::Occluder2D` shapes: a box, a
circle, a polygon, a thin line, or the outline of a tile map's walls.

<div class="lang-pair">
<div>

```rust
# use ponko::d2::*;
# let tiles = ctx.gfx.create_texture(32, 16, &[200; 32 * 16 * 4]);
# let tile_normals = ctx.gfx.create_texture(32, 16, &[128; 32 * 16 * 4]);
# let crate_box = tiles;
# const WALL: u16 = 1;
# let mut map = TileMap::new(SpriteSheet::new(tiles, 16, 16), 20, 15);
# for y in 0..15 { for x in 0..20 {
#     map.set(x, y, Some(if x == 0 || y == 0 || x == 19 || y == 14 { WALL } else { 0 }));
# } }
// Dark blue where no light reaches; this turns 2D lighting on.
ctx.gfx.set_ambient_light2d(Some(Color::rgb(0.1, 0.11, 0.18)));
// A warm torch whose light the walls and the crate block.
ctx.world.spawn((
    Transform::at(vec2(160.0, 120.0)),
    PointLight2D {
        color: Color::rgb(1.0, 0.6, 0.3),
        radius: 250.0,
        shadows: true,
        ..Default::default()
    },
));
// A lantern: a cone along its rotation (here, to the right).
ctx.world.spawn((
    Transform::at(vec2(60.0, 60.0)),
    SpotLight2D { angle: 0.4, ..Default::default() },
));
// The walls (tile 1) of the map cast shadows; so does a crate.
let walls = map.occluder(|tile| tile == WALL);
ctx.world.spawn((Transform::at(Vec2::ZERO), map, walls));
ctx.world.spawn((
    Transform::at(vec2(220.0, 150.0)),
    Sprite::new(crate_box),
    Occluder2D::rect(vec2(16.0, 16.0)),
));
// Bumpy tiles: light shades their bevels.
ctx.gfx.set_normal_map(tiles, Some(tile_normals));
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:lights2d}}
```

</div>
</div>

The `lights2d` example (`cargo run --example lights2d`) is a tile dungeon with
wandering coloured lights, a lantern at the mouse and normal-mapped walls and
crates; L adds a hundred lights and N turns the normal maps off.

- **What it lights**: the world: sprites, tile maps and particles. Lights
  add up; the result multiplies what was drawn, so a light at 1.0 shows
  sprites at their own color, and brighter lights (`intensity` above 1)
  brighten them up to twice. Screen-space UI and text are never lit.
- **Fading**: point and spot lights fade from full at the centre to nothing
  at `radius` pixels; `falloff` shapes the curve (1 straight, 2 the default,
  higher stays bright longer). A spot light covers `angle` radians to each
  side of its rotation, with the last `softness` of that fading out.
- **Shadows**: each light with `shadows: true` is blocked by the edges of the
  occluders within its radius (a directional light: within the view). The
  occluder itself stays lit; only what is behind it goes dark. Occluders
  turn, scale and mirror with their transform, and `TileMap::occluder`
  merges a map's walls into one outline, so a wall of a hundred tiles is a
  few edges. Shadows are hard-edged.
- **Normal maps**: `ctx.gfx.set_normal_map(texture, Some(normals))` gives every
  sprite and tile drawn from `texture` a normal map: an image of the same size
  in tangent space, green pointing up the image (the common "OpenGL" style).
  Light then shades the bumps from its direction, using its `height` above
  the world (and a directional light's `elevation`). Normals turn with the
  sprite and flip when it is mirrored. Sprites without a normal map are lit
  flat. Additive and multiply sprites and clipped (UI-masked) sprites skip
  normal mapping.
- **Cost**: nothing without lights. With no light component and no ambient
  light, a frame draws exactly as before, makes no GPU resources and spends
  only three empty component counts on the CPU. Lights without shadows are
  one instanced draw; each shadowed light adds its shadow volumes and one
  draw. On an Apple M4 Max (Metal, 1920x1080, 2,000 sprites; `cargo run
  --release -p ponko_bench -- lights2d`), a frame took 0.33 ms unlit,
  0.55 ms with 100 lights, about 1 ms with 100 shadowed lights and 200
  occluders (0.3 ms of it CPU, 0.16 ms gathering the shadow edges), and
  1.8 ms with normal maps on top.
- **WebGL2**: the same on WebGL2 as on WebGPU and native; the browser smoke
  test's `lights2d` fixture runs it on Chromium's WebGL2.
- **From C and C++**: the light components are plain structs
  (`ponko::point_light2d` and the others, from `point_light2d_default()`),
  and the context has `set_occluder2d_rect` (and `_circle`, `_polygon`,
  `_line`), `set_tilemap_occluder`, `set_ambient_light2d` and
  `set_normal_map`.

Lights are reflected as `"PointLight2D"`, `"SpotLight2D"`,
`"DirectionalLight2D"` and `"Occluder2D"`, so the inspector, the MCP tools and
scenes see and edit them.

## Hot reload

Save an image in your editor and the running game shows the new version
within a quarter of a second: textures loaded with `load_texture` are
reloaded when their file changes, under the same handle, so every sprite
using them updates by itself. Fonts (`load_font`) and sounds (`load_sound`)
reload the same way, and so do [data files](data-files.md). A file that
can't be read or decoded mid-save keeps the old version. It's on by default
in debug builds and off in release builds; `Game::hot_reload(bool)` or
`PONKO_HOT_RELOAD=1`/`0` changes that.

If you computed something from an image (a level read from its pixels, a
collision mask), redo it when the texture shows up in `reloaded_textures`:

<div class="lang-pair">
<div>

```rust
# struct Level(Texture);
Game::new("Art test", 1280, 720)
    .hot_reload(true) // the default in debug builds
    .system(|ctx: &mut Ctx| {
        let level = ctx.world.resource::<Level>().0;
        if ctx.assets.reloaded_textures().contains(&level) {
            println!("level.png changed: rebuilding the map");
        }
    })
    .setup(|ctx| {
        let level = ctx.load_texture("level.png");
        ctx.world.insert_resource(Level(level));
    })
    .run();
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:hot_reload}}
```

</div>
</div>

An image that changed size isn't reloaded, because sprites keep the size
their texture had; the console says so, and a restart picks it up. A file
caught halfway through saving fails to decode and is tried again on the
next save. Checking costs one file `stat` per loaded texture, four times a
second.

## Screenshots

<div class="lang-pair">
<div>

```rust
if ctx.key_pressed(Key::F12) {
    ctx.gfx.save_screenshot("shot.png");
}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:screenshot}}
```

</div>
</div>

### Image formats

The format of a file is read from its first bytes, so a JPEG saved as
`background.png` still loads. PNG is always built in. The rest are cargo
features, none of them tied to `3d`:

| Feature | Format | On by default |
|---|---|---|
| (always) | PNG | yes |
| `image-jpeg` | JPEG | yes |
| `image-webp` | WebP | |
| `image-bmp` | BMP | |
| `image-tga` | TGA (found from the `.tga` extension) | |
| `image-gif` | GIF (first frame) | |
| `image-tiff` | TIFF | |
| `image-ico` | ICO | |
| `image-qoi` | QOI | |
| `image-pnm` | PNM | |
| `image-dds` | DDS (uncompressed) | |
| `image-hdr`, `image-exr` | Radiance HDR, OpenEXR (read as 8-bit color) | |
| `image-formats` | all of the above | |

```toml
# Cargo.toml: WebP too, and no JPEG decoder in the build
ponko = { version = "0.1", default-features = false, features = ["window", "audio", "image-webp"] }
```

A file in a format the build leaves out fails with an `Unsupported` error
(a panic from `load_texture`) that names the feature to turn on, for
example: `"tile.webp" is a WebP image, but this build of ponko doesn't read
WebP: turn on its `image-webp` cargo feature`. Builds without the default
features, such as a headless server, carry no JPEG decoder. The `3d` feature
turns on `image-jpeg`, `image-hdr` and `image-exr` for glTF textures and
environment maps. The C library builds with the engine's defaults; choosing
image formats from C is not exposed yet.

`save_screenshot` saves the next frame as a PNG once it
has been drawn, even while the window is hidden. Headless runs have no
frames, so it does nothing there.

## Compressed textures

Images in PNG or JPEG are unpacked to 4 bytes a pixel on the GPU, however
small the file. Block-compressed textures stay compressed in video memory:
a color texture in BC7 takes a quarter of that, a normal map in BC5 a
quarter and a one-channel mask in BC4 an eighth, and they load faster, as
their mipmaps are made ahead of time instead of at load. Turn on the
`compressed-textures` feature and load `.ktx2` (or `.dds`) files like any
other texture:

```toml
# Cargo.toml
ponko = { version = "0.1", features = ["3d", "compressed-textures"] }
```

```rust
# use ponko::prelude::*;
# fn f(ctx: &mut Ctx) {
let bricks = ctx.gfx.load_texture("bricks.ktx2");
# let _ = bricks;
# }
```

`ponko textures` makes them from your images, once, at build time. It
converts every PNG and JPEG under a folder (default `assets`) into a
`.ktx2` file next to it, with its mipmaps, and skips those already up to
date:

```sh
ponko textures assets
ponko textures assets --check   # in CI: fail if a .ktx2 is missing or stale
```

A `textures.json` in that folder says what each image is, by path or
pattern (`*` within a folder name, `**` across folders; an exact path wins
over patterns, and a longer pattern over a shorter one):

```json
{
  "default": "color",
  "textures": {
    "**_normal.png": "normal",
    "**_orm.png": "linear",
    "**_mask.png": "mask",
    "ui/**": "ui",
    "hero.png": { "kind": "color", "basis": true }
  }
}
```

| Kind | For | Becomes |
|---|---|---|
| `color` (the default) | colors, read as sRGB | BC7 sRGB |
| `linear` | data that isn't a color: ORM, flow maps | BC7 |
| `normal` | normal maps | BC5: X and Y; the shader rebuilds Z |
| `mask` | one channel, in red | BC4 |
| `ui` | UI and pixel art, which must stay sharp | RGBA8, no mipmaps |
| `skip` | images the game reads as pixels | not converted |

BC formats are what desktop GPUs sample (Windows, macOS, Linux, and
browsers on desktops). A GPU without them, such as most phones, still
loads the files: the engine decodes them to RGBA8 at load, as it does for
textures whose width or height isn't a multiple of 4. They then take as
much memory as a PNG.

For a game that also runs on phones, `"basis": true` keeps a texture as
Basis Universal UASTC instead, and the engine transcodes it at load to
what the GPU samples: BC7 or BC5 on desktops, ASTC or ETC2 on phones,
RGBA8 where there is none. That needs the `basis-universal` feature (the
Basis Universal transcoder, C++ built with your C++ compiler) and costs
time at load: in the measurement below, transcoding made loading slower
than the PNGs. **Web builds have no Basis Universal transcoder**: there a
UASTC file fails with an `Unsupported` error, so give the web BCn files.

Compressed textures work wherever textures do: in materials (a BC5 normal
map is read as one), texture arrays (every layer a `.ktx2` of the same
size and format; mixed with image files, every layer is decoded to RGBA8),
[streaming](streaming.md), where a budget keeps a smaller mip level of the
file resident, and hot reload, where a `.png` may even be replaced by its
`.ktx2`. The asset inventory and residency budgets count their compressed
bytes. `update_texture` on one turns it into an RGBA8 texture. Without the
feature, loading a `.ktx2` fails with an `Unsupported` error naming it.
Cube maps, 3D textures and Basis ETC1S files aren't read yet; nor are the
KTX2 textures of glTF files (`KHR_texture_basisu`). The C library has the
same features (`cargo build -p ponko_ffi --features compressed-textures`);
its released builds leave them out.

Measured on an Apple M4 Max (Metal) with a busy machine, a release build,
reading, decoding and uploading 18 material textures of 1024 x 1024 (six
color, six normal and six mask) with their mip chains, the median of three
runs (`src/render/compressed_bench.rs` says how to repeat it):

| Files | GPU memory | On disk | Load |
|---|---|---|---|
| PNG, mipmaps made on the GPU | 96.0 MiB | 32.2 MiB | 575 ms |
| KTX2, BC7/BC5/BC4 | 20.0 MiB | 18.0 MiB | 69 ms |
| KTX2, UASTC transcoded at load | 24.0 MiB | 22.5 MiB | 1582 ms (PNG in the same run: 1333 ms) |

The web wasn't measured: there is no headless browser GPU to measure it
on.
