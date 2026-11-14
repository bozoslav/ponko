//! Levels from level editors: the editor-neutral model ([`Level`]), loading
//! it from Tiled and LDtk files, and spawning it ([`Ctx::spawn_level`]).

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::{UVec2, Vec2};
use ponko_ecs::{Component, Entity};
use serde_json::Value;

use super::TileMap;
use crate::assets::{Assets, Origin};
use crate::camera::Rect;
use crate::color::Color;
use crate::ctx::Ctx;
use crate::ecs::Transform2D;
use crate::error::{Error, ErrorKind};
use crate::hot_reload::Asset;
use crate::reflect::NotSaved;
use crate::sheet::SpriteSheet;

/// Custom properties from the editor: Tiled's custom properties, LDtk's
/// fields. Values are JSON: numbers, strings, booleans, colors as
/// `"#rrggbb"` / `"#aarrggbb"` strings, Tiled class properties as objects,
/// LDtk arrays, points and entity references as LDtk writes them.
///
/// ```
/// # use ponko::tilemap::LevelObject;
/// # fn f(coin: &LevelObject) {
/// let value = coin.properties.f32("value").unwrap_or(1.0);
/// let hidden = coin.properties.bool("hidden").unwrap_or(false);
/// # }
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Properties(pub BTreeMap<String, Value>);

impl Properties {
    /// The value named `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }
    /// A string (or file, color, enum) property.
    #[must_use]
    pub fn str(&self, name: &str) -> Option<&str> {
        self.get(name)?.as_str()
    }
    /// A number property.
    #[must_use]
    pub fn f32(&self, name: &str) -> Option<f32> {
        self.get(name)?.as_f64().map(|v| v as f32)
    }
    /// A whole-number property.
    #[must_use]
    pub fn i64(&self, name: &str) -> Option<i64> {
        self.get(name)?.as_i64()
    }
    /// A boolean property.
    #[must_use]
    pub fn bool(&self, name: &str) -> Option<bool> {
        self.get(name)?.as_bool()
    }
    /// A color property (`"#rrggbb"` or Tiled's `"#aarrggbb"`).
    #[must_use]
    pub fn color(&self, name: &str) -> Option<Color> {
        parse_color(self.str(name)?)
    }
    /// No properties.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Every property, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// A tileset of a [`Level`]: one image cut into same-size tiles.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Tileset {
    /// Its name in the editor.
    pub name: String,
    /// The image, as an asset name (relative paths resolved against the
    /// level or tileset file).
    pub image: String,
    /// Size of one tile in pixels.
    pub tile_size: UVec2,
    /// Tiles per row.
    pub columns: u32,
    /// Number of tiles.
    pub count: u32,
    /// Pixels around the edge of the image.
    pub margin: u32,
    /// Pixels between tiles.
    pub spacing: u32,
    /// What the editor says about single tiles (only tiles it says
    /// something about), by tile id.
    pub tiles: BTreeMap<u32, TileInfo>,
}

/// What the editor says about one tile of a [`Tileset`].
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TileInfo {
    /// Its class (Tiled) or empty.
    pub class: String,
    /// Tags (LDtk's enum tags) or empty.
    pub tags: Vec<String>,
    /// Custom properties (LDtk's custom data is the string property `data`).
    pub properties: Properties,
    /// An animation: tile ids of the same tileset and seconds each.
    pub animation: Vec<(u32, f32)>,
    /// Collision shapes drawn on the tile (Tiled), in pixels from its
    /// top-left corner.
    pub shapes: Vec<LevelObject>,
}

/// One tile of a [`TileLayer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlacedTile {
    /// Its cell.
    pub cell: UVec2,
    /// Index into [`Level::tilesets`].
    pub tileset: u16,
    /// The tile in its tileset.
    pub id: u32,
    /// [`TileMap::FLIP_X`] | [`TileMap::FLIP_Y`] | [`TileMap::FLIP_DIAGONAL`].
    pub flip: u8,
}

/// The tiles of a tile layer (and LDtk's IntGrid values).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TileLayer {
    /// Size in cells.
    pub size: UVec2,
    /// Size of a cell in pixels.
    pub cell_size: Vec2,
    /// The tiles, in no set order. A cell may hold more than one (LDtk
    /// stacks them); later ones are on top.
    pub tiles: Vec<PlacedTile>,
    /// LDtk IntGrid values, one per cell row by row (0 = empty); empty
    /// for other layers.
    pub int_grid: Vec<u32>,
    /// The names of the IntGrid values.
    pub int_grid_names: BTreeMap<u32, String>,
}

/// The shape of a [`LevelObject`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ObjectShape {
    /// A rectangle of the object's size around its position.
    Rect,
    /// An ellipse of the object's size around its position.
    Ellipse,
    /// Just the position.
    Point,
    /// A closed polygon: corners relative to the position.
    Polygon(Vec<Vec2>),
    /// An open line: points relative to the position.
    Polyline(Vec<Vec2>),
}

/// An object of an object layer (Tiled) or an entity (LDtk). Spawned
/// levels put it on the object's entity, so systems can find objects by
/// class or name and read their properties.
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::tilemap::LevelObject;
/// fn coins(ctx: &mut Ctx) {
///     for (t, obj) in ctx.world.query::<(&ponko::d2::Transform, &LevelObject)>().iter() {
///         if obj.class == "Coin" {
///             let value = obj.properties.i64("value").unwrap_or(1);
///             // ...
///         }
///     }
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Component)]
#[non_exhaustive]
pub struct LevelObject {
    /// Tiled's object id, or LDtk's `iid` in [`properties`](Self::properties)
    /// (`"iid"`) with this the entity's index in its layer.
    pub id: u64,
    /// Its name (Tiled); LDtk entities have none.
    pub name: String,
    /// Its class or type (Tiled), or its entity identifier (LDtk).
    pub class: String,
    /// The name of its layer.
    pub layer: String,
    /// Where it is, in level pixels: the center of rectangles, ellipses and
    /// tiles, the point of points, the origin of polygons and lines. A
    /// spawned object's `Transform2D` is here (plus where the level is).
    pub position: Vec2,
    /// Width and height in pixels (zero for points and lines).
    pub size: Vec2,
    /// Radians, clockwise.
    pub rotation: f32,
    /// Its shape.
    pub shape: ObjectShape,
    /// The tile it shows, if any: `(tileset, tile id, flips)`.
    pub tile: Option<(u16, u32, u8)>,
    /// Shown in the editor.
    pub visible: bool,
    /// Custom properties (LDtk fields), plus `"iid"` and `"tags"` for LDtk.
    pub properties: Properties,
}

/// What a layer holds.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum LayerKind {
    /// Tiles (and LDtk IntGrid values).
    Tiles(TileLayer),
    /// Objects (Tiled) or entities (LDtk).
    Objects(Vec<LevelObject>),
    /// One image (Tiled's image layer), as an asset name.
    Image(String),
}

/// One layer of a [`Level`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Layer {
    /// Its name.
    pub name: String,
    /// Its class (Tiled) or empty.
    pub class: String,
    /// Where it is drawn from the level's top-left, in pixels.
    pub offset: Vec2,
    /// As [`TileMap::parallax`]: `(1, 1)` moves with the world.
    pub parallax: Vec2,
    /// 0 transparent to 1 opaque.
    pub opacity: f32,
    /// Multiplied into its tiles' colors.
    pub tint: Color,
    /// Shown in the editor. Hidden layers are not drawn, but still give
    /// collision (collision layers are often hidden).
    pub visible: bool,
    /// Custom properties.
    pub properties: Properties,
    /// What it holds.
    pub kind: LayerKind,
}

/// A level from a level editor, read but not spawned: its tilesets, layers
/// (bottom to top), objects and custom properties. Load one with
/// [`Assets::load_level`]; spawn one with [`Ctx::spawn_level`].
///
/// Read from:
///
/// - **Tiled** maps, `.tmx` (XML) and `.tmj` / `.json`: orthogonal maps with
///   tile, object, image and group layers; embedded and external tilesets
///   (`.tsx`, `.tsj`); CSV, base64, zlib and gzip tile data; infinite maps;
///   flipped and animated tiles; tile collision shapes; object templates
///   (`.tx`); custom properties everywhere.
/// - **LDtk** projects, `.ldtk` (and their external `.ldtkl` level files):
///   IntGrid, auto-layer, tile and entity layers; tilesets; flipped tiles;
///   entity fields; tile custom data and enum tags; parallax.
///
/// Not read: Tiled's isometric and hexagonal maps, image-collection
/// tilesets, zstd-compressed tile data and text objects; LDtk's level
/// background images.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Level {
    /// The level's name (LDtk) or the file name (Tiled).
    pub name: String,
    /// Size in cells of [`cell_size`](Self::cell_size).
    pub size: UVec2,
    /// The grid's cell size in pixels.
    pub cell_size: Vec2,
    /// The background color, if the editor sets one.
    pub background: Option<Color>,
    /// Where parallax layers line up, in level pixels.
    pub parallax_origin: Vec2,
    /// Custom properties of the map (Tiled) or level fields (LDtk).
    pub properties: Properties,
    /// The tilesets the layers use.
    pub tilesets: Vec<Tileset>,
    /// The layers, bottom to top (Tiled's groups flattened into them).
    pub layers: Vec<Layer>,
    /// Every file it was read from (the level and its tilesets,
    /// templates and level files), as asset names.
    pub files: Vec<String>,
}

/// How [`Ctx::spawn_level_with`] spawns a level. `LevelOptions::default()`
/// spawns every layer and object, and colliders (feature `physics`) for
/// the layers named `collision` or `collisions` (any case), layers with
/// the boolean property `solid` or `collides`, and tiles with that property
/// or with collision shapes.
///
/// ```
/// # use ponko::prelude::*;
/// use ponko::tilemap::LevelOptions;
/// let options = LevelOptions::default()
///     .prefab("Coin", "prefabs/coin.json") // objects of class Coin are this prefab
///     .solid("Walls") // and the Walls layer (or IntGrid value, or tile tag) is solid
///     .level("Level_1"); // LDtk: this level of the project
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LevelOptions {
    /// Prefabs by object class (or name): `(class, prefab path)`.
    pub prefabs: Vec<(String, String)>,
    /// More solid things by name: layers, LDtk IntGrid values, tile classes
    /// and tags.
    pub solid: Vec<String>,
    /// The LDtk level to spawn (identifier or iid); the first one if `None`.
    pub level: Option<String>,
    /// No colliders at all.
    pub no_colliders: bool,
    /// None of the objects a world save holds: those spawned as prefabs
    /// (see [`without_saved_objects`](Self::without_saved_objects)).
    pub no_saved_objects: bool,
    /// The `z` of the layer the objects are on; layers below get one less
    /// each, layers above one more. Default `0`.
    pub z: f32,
}

impl LevelOptions {
    /// Objects of class (or name) `class` are spawned as the prefab at
    /// `path` ([`Ctx::spawn_prefab`]), at the object's place, with their
    /// [`LevelObject`] added. An object's string property `prefab` names a
    /// prefab too.
    #[must_use]
    pub fn prefab(mut self, class: impl Into<String>, path: impl Into<String>) -> Self {
        self.prefabs.push((class.into(), path.into()));
        self
    }
    /// Layers, LDtk IntGrid values, tile classes or tile tags named `name`
    /// (any case) are solid.
    #[must_use]
    pub fn solid(mut self, name: impl Into<String>) -> Self {
        self.solid.push(name.into());
        self
    }
    /// For an LDtk project: spawn the level with this identifier or iid.
    #[must_use]
    pub fn level(mut self, name: impl Into<String>) -> Self {
        self.level = Some(name.into());
        self
    }
    /// Spawn no colliders, even with the `physics` feature.
    #[must_use]
    pub fn without_colliders(mut self) -> Self {
        self.no_colliders = true;
        self
    }
    /// Spawn none of the objects a world save holds: those of a class
    /// given a [`prefab`](Self::prefab) (or with a `prefab` property).
    /// Everything else a level spawns carries [`NotSaved`], so world saves
    /// hold only the prefab objects, with their state (a coin collected is
    /// gone, a door opened stays open). After loading such a save, spawn
    /// the level again with this option: the tiles, colliders and plain
    /// objects come from the file, the prefab objects from the save.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// use ponko::tilemap::LevelOptions;
    /// # fn f(ctx: &mut Ctx) -> Result<(), Error> {
    /// let options = LevelOptions::default().prefab("Coin", "prefabs/coin.json");
    /// ctx.spawn_level_with("levels/one.tmx", Vec2::ZERO, options.clone())?;
    /// ctx.storage.save_world(&ctx.world, "slot1")?; // the coins, not the level
    ///
    /// // Later: the saved coins, and the level around them.
    /// ctx.load_world("slot1")?;
    /// ctx.spawn_level_with("levels/one.tmx", Vec2::ZERO, options.without_saved_objects())?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn without_saved_objects(mut self) -> Self {
        self.no_saved_objects = true;
        self
    }
    /// The `z` of the objects' layer (see [`z`](Self::z)).
    #[must_use]
    pub fn at_z(mut self, z: f32) -> Self {
        self.z = z;
        self
    }

    fn names_solid(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case("collision")
            || name.eq_ignore_ascii_case("collisions")
            || self.solid.iter().any(|s| s.eq_ignore_ascii_case(name))
    }
}

/// On the root entity of a spawned level: where it came from and what was
/// spawned for it. Despawn the level with
/// [`Ctx::despawn_level`]; editing the file (with hot reload on) spawns it
/// again in place.
#[derive(Clone, Debug, PartialEq, Component)]
#[non_exhaustive]
pub struct LevelRoot {
    /// The level file, as passed to [`Ctx::spawn_level`].
    pub path: String,
    /// How it was spawned.
    pub options: LevelOptions,
    /// The level's own custom properties.
    pub properties: Properties,
    /// Every entity spawned for it: layers, objects and colliders.
    pub entities: Vec<Entity>,
}

/// On each layer entity of a spawned level: the layer's name and custom
/// properties. A tile layer may be several entities (one per tileset), all
/// with the same `LevelLayer`.
#[derive(Clone, Debug, PartialEq, Component)]
#[non_exhaustive]
pub struct LevelLayer {
    /// The layer's name.
    pub name: String,
    /// Its class (Tiled) or empty.
    pub class: String,
    /// Its index in [`Level::layers`] (bottom to top).
    pub index: usize,
    /// Its custom properties.
    pub properties: Properties,
}

// ---- JSON helpers shared by the readers ----------------------------------

/// A number, also from a string (XML attributes arrive as strings).
pub(super) fn num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// [`num`] or `default`.
pub(super) fn num_or(v: &Value, key: &str, default: f64) -> f64 {
    num(v, key).unwrap_or(default)
}

/// A boolean, also from `0`/`1` and `"true"`/`"false"`.
pub(super) fn flag(v: &Value, key: &str, default: bool) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !matches!(s.trim(), "0" | "false" | ""),
        _ => default,
    }
}

/// A string, or `""`.
pub(super) fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `#rrggbb` or `#aarrggbb` (Tiled puts alpha first).
pub(super) fn parse_color(s: &str) -> Option<Color> {
    let hex = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(hex, 16).ok()?;
    let byte = |shift: u32| ((v >> shift) & 0xff) as f32 / 255.0;
    match hex.len() {
        6 => Some(Color::rgba(byte(16), byte(8), byte(0), 1.0)),
        8 => Some(Color::rgba(byte(16), byte(8), byte(0), byte(24))),
        _ => None,
    }
}

/// Resolves `rel` (a path in file `base`) to an asset name: relative to
/// `base`'s folder, with `.` and `..` worked out.
pub(super) fn resolve(base: &str, rel: &str) -> String {
    let rel = rel.replace('\\', "/");
    let joined = if rel.starts_with('/') || rel.contains(':') {
        rel
    } else {
        match base.rfind('/') {
            Some(i) => format!("{}/{rel}", &base[..i]),
            None => rel,
        }
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "." => {}
            "" if !parts.is_empty() => {}
            ".." if parts.last().is_some_and(|p| *p != ".." && !p.is_empty()) => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

pub(super) fn decode_error(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Decode, message)
}

/// Reads level files through the asset sources, remembering each one.
pub(super) struct Reader<'a> {
    assets: &'a Assets,
    pub files: Vec<ReadFile>,
}

impl Reader<'_> {
    pub fn bytes(&mut self, name: &str) -> Result<Vec<u8>, Error> {
        let found = self.assets.asset_sources.find(name)?;
        let file = match &found.origin {
            Origin::File(path) => Some(path.clone()),
            Origin::Source { .. } => None,
        };
        let bytes = found.into_bytes().map_err(|e| e.with_path(name))?;
        if !self.files.iter().any(|(n, _)| n == name) {
            self.files.push((name.to_string(), file));
        }
        Ok(bytes)
    }

    pub fn text(&mut self, name: &str) -> Result<String, Error> {
        String::from_utf8(self.bytes(name)?).map_err(|e| decode_error(format!("{name} is not UTF-8 text: {e}")))
    }
}

/// A file a level was read from: its asset name and, when it is a file on
/// disk, its path (for hot reload).
type ReadFile = (String, Option<PathBuf>);

impl Level {
    /// Reads `path`, telling the format by its extension: `.tmx` and `.tmj`
    /// (or other `.json`) are Tiled maps, `.ldtk` LDtk projects. Also
    /// returns the files read and where they are on disk.
    fn read(assets: &Assets, path: &str, options: &LevelOptions) -> Result<(Level, Vec<ReadFile>), Error> {
        let mut reader = Reader {
            assets,
            files: Vec::new(),
        };
        let text = reader.text(path)?;
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let level = match ext.as_str() {
            "tmx" => super::tiled::read_tmx(&text, path, &mut reader),
            "tmj" | "json" => super::tiled::read_tmj(&text, path, &mut reader),
            "ldtk" => super::ldtk::read(&text, path, options.level.as_deref(), &mut reader),
            _ => Err(Error::new(
                ErrorKind::Unsupported,
                format!("{path}: not a level file Ponko reads (.tmx, .tmj, .json or .ldtk)"),
            )),
        };
        let mut level = level.map_err(|e| e.context(path).with_path(path))?;
        level.files = reader.files.iter().map(|(n, _)| n.clone()).collect();
        Ok((level, reader.files))
    }

    /// The layers below the objects' layer count down from `options.z`,
    /// those above up: the `z` of layer `index`.
    fn layer_z(&self, index: usize, options: &LevelOptions) -> f32 {
        let objects = self
            .layers
            .iter()
            .position(|l| matches!(l.kind, LayerKind::Objects(_)))
            .unwrap_or(self.layers.len().saturating_sub(1));
        options.z + index as f32 - objects as f32
    }

    fn tile_info(&self, tileset: u16, id: u32) -> Option<&TileInfo> {
        self.tilesets.get(usize::from(tileset))?.tiles.get(&id)
    }

    /// Whether a tile fills its cell for collision: a solid layer, the
    /// boolean property `solid` or `collides`, a class or tag `options`
    /// names, or a collision shape covering the whole tile.
    fn tile_is_solid(&self, layer_solid: bool, t: &PlacedTile, options: &LevelOptions) -> bool {
        if layer_solid {
            return true;
        }
        let Some(info) = self.tile_info(t.tileset, t.id) else {
            return false;
        };
        let size = self.tilesets[usize::from(t.tileset)].tile_size.as_vec2();
        info.properties.bool("solid") == Some(true)
            || info.properties.bool("collides") == Some(true)
            || (!info.class.is_empty() && options.solid.iter().any(|s| s.eq_ignore_ascii_case(&info.class)))
            || info
                .tags
                .iter()
                .any(|tag| options.solid.iter().any(|s| s.eq_ignore_ascii_case(tag)))
            || info.shapes.iter().any(|s| covers(s, size))
    }

    fn layer_is_solid(layer: &Layer, options: &LevelOptions) -> bool {
        options.names_solid(&layer.name)
            || layer.properties.bool("solid") == Some(true)
            || layer.properties.bool("collides") == Some(true)
    }

    /// The solid parts of the level as rectangles in level pixels, merged
    /// into few per layer (see [`LevelOptions`] for what is solid). For
    /// your own collision; with the `physics` feature
    /// [`Ctx::spawn_level`] makes colliders of them.
    ///
    /// Tiles with collision shapes that don't cover them are left out
    /// here; spawned levels give each such shape its own collider.
    #[must_use]
    pub fn solid_rects(&self, options: &LevelOptions) -> Vec<Rect> {
        let mut out = Vec::new();
        for layer in &self.layers {
            let LayerKind::Tiles(tiles) = &layer.kind else {
                continue;
            };
            let layer_solid = Self::layer_is_solid(layer, options);
            let (w, h) = (tiles.size.x, tiles.size.y);
            let mut solid = vec![false; (w * h) as usize];
            for t in &tiles.tiles {
                if t.cell.x < w && t.cell.y < h && self.tile_is_solid(layer_solid, t, options) {
                    solid[(t.cell.y * w + t.cell.x) as usize] = true;
                }
            }
            for (i, &v) in tiles.int_grid.iter().enumerate() {
                let named = tiles.int_grid_names.get(&v).is_some_and(|n| options.names_solid(n));
                if v != 0 && (layer_solid || named) && i < solid.len() {
                    solid[i] = true;
                }
            }
            for [x, y, cw, ch] in super::colliders::merge_cells(w, h, |x, y| solid[(y * w + x) as usize]) {
                let min = layer.offset + Vec2::new(x as f32, y as f32) * tiles.cell_size;
                out.push(Rect {
                    min,
                    max: min + Vec2::new(cw as f32, ch as f32) * tiles.cell_size,
                });
            }
        }
        out
    }
}

/// Whether collision shape `s` covers a whole `size` tile.
fn covers(s: &LevelObject, size: Vec2) -> bool {
    s.shape == ObjectShape::Rect
        && s.rotation == 0.0
        && (s.position - s.size * 0.5).abs().max_element() < 0.5
        && (s.size - size).abs().max_element() < 0.5
}

impl Assets {
    /// Reads the level at `path` (Tiled `.tmx` / `.tmj`, LDtk `.ldtk`) and
    /// the files it uses, without spawning anything: for tools, or to read
    /// a level's data your own way. See [`Level`] for what is read.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) -> Result<(), Error> {
    /// let level = ctx.assets.load_level("levels/one.tmx", &ponko::tilemap::LevelOptions::default())?;
    /// for layer in &level.layers {
    ///     println!("{}", layer.name);
    /// }
    /// # Ok(()) }
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotFound`] if a file is missing,
    /// [`Unsupported`](ErrorKind::Unsupported) for a format or feature
    /// Ponko doesn't read, [`Decode`](ErrorKind::Decode) for a broken file.
    /// The error's path is `path`.
    pub fn load_level(&self, path: &str, options: &LevelOptions) -> Result<Level, Error> {
        Level::read(self, path, options).map(|(level, _)| level)
    }
}

/// A tileset's frames as a sheet: the image as it is, or, for images with
/// a margin or spacing, its tiles copied next to each other.
fn sheet_of(ctx: &mut Ctx, set: &Tileset) -> Result<SpriteSheet, Error> {
    let (tw, th) = (set.tile_size.x, set.tile_size.y);
    if tw == 0 || th == 0 {
        return Err(decode_error(format!("tileset {} has empty tiles", set.name)));
    }
    let texture = if set.margin == 0 && set.spacing == 0 {
        ctx.gfx.try_load_texture(&set.image)?
    } else {
        let what = format!("tileset {} image \"{}\"", set.name, set.image);
        let image = ctx.assets.load_with(&set.image, |bytes| {
            crate::assets::decode_image(bytes, &set.image, &what)
        })?;
        let columns = set.columns.max(1);
        let rows = set.count.div_ceil(columns).max(1);
        let (w, h) = (columns * tw, rows * th);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for i in 0..set.count {
            let (cx, cy) = (i % columns, i / columns);
            let (sx, sy) = (
                set.margin + cx * (tw + set.spacing),
                set.margin + cy * (th + set.spacing),
            );
            for row in 0..th {
                let (src_y, dst_y) = (sy + row, cy * th + row);
                if src_y >= image.height || sx + tw > image.width {
                    continue;
                }
                let src = ((src_y * image.width + sx) * 4) as usize;
                let dst = ((dst_y * w + cx * tw) * 4) as usize;
                let len = (tw * 4) as usize;
                rgba[dst..dst + len].copy_from_slice(&image.rgba[src..src + len]);
            }
        }
        ctx.gfx.create_texture(w, h, &rgba)
    };
    if tw > texture.width() || th > texture.height() {
        return Err(decode_error(format!(
            "tileset {}: {tw} x {th} tiles don't fit its {} x {} image {}",
            set.name,
            texture.width(),
            texture.height(),
            set.image
        )));
    }
    Ok(SpriteSheet::new(texture, tw, th))
}

impl Ctx {
    /// Spawns the level at `path` (a Tiled `.tmx` / `.tmj` map or an LDtk
    /// `.ldtk` project) with its top-left corner at `at`, as
    /// [`spawn_level_with`](Self::spawn_level_with) with
    /// `LevelOptions::default()`.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Platformer", 800, 600).run(|ctx| {
    ///     ctx.spawn_level("levels/one.tmx", Vec2::ZERO).expect("level");
    ///     |_ctx: &mut Ctx| {}
    /// });
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Assets::load_level`], and the error of a texture or prefab
    /// that can't be loaded. Nothing is spawned then.
    pub fn spawn_level(&mut self, path: &str, at: Vec2) -> Result<Entity, Error> {
        self.spawn_level_with(path, at, LevelOptions::default())
    }

    /// Spawns the level at `path` with its top-left corner at `at`:
    ///
    /// - a root entity with a [`LevelRoot`] and a `Transform2D` at `at`;
    /// - each tile layer as [`TileMap`]s (one per tileset it uses), with
    ///   its tilesets' textures, animated and flipped tiles, offset,
    ///   parallax, opacity and tint, and a [`LevelLayer`]; layers are
    ///   stacked by `z` (see [`LevelOptions::z`]);
    /// - each object or entity as an entity with a `Transform2D` and its
    ///   [`LevelObject`]: the prefab [`LevelOptions::prefab`] names for its
    ///   class, or else a `Sprite` if it shows a tile;
    /// - with the `physics` feature, fixed colliders for the solid tiles
    ///   (see [`LevelOptions`]), merged into few rectangles.
    ///
    /// Everything but the prefab objects carries [`NotSaved`]: world saves
    /// hold the prefab objects (the game's state), not what the file
    /// rebuilds. To load such a save, load it, then spawn the level again
    /// with [`LevelOptions::without_saved_objects`].
    ///
    /// With [hot reload](crate::game::Game::hot_reload) on, editing the
    /// level (or a tileset or template it uses) despawns what it spawned
    /// and spawns it again, in place; a file that doesn't load keeps the
    /// old level and prints why. [`Assets::reloaded_files`] lists `path`
    /// that frame.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// use ponko::tilemap::LevelOptions;
    /// # fn setup(ctx: &mut Ctx) {
    /// let options = LevelOptions::default().prefab("Coin", "prefabs/coin.json").level("Level_0");
    /// ctx.spawn_level_with("world.ldtk", Vec2::ZERO, options).expect("level");
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`spawn_level`](Self::spawn_level).
    pub fn spawn_level_with(&mut self, path: &str, at: Vec2, options: LevelOptions) -> Result<Entity, Error> {
        let (level, files) = Level::read(&self.assets, path, &options)?;
        let entities = self
            .spawn_level_parts(&level, at, &options)
            .map_err(|e| e.with_path(path))?;
        let root = self.world.spawn((
            Transform2D::at(at),
            LevelRoot {
                path: path.to_string(),
                options,
                properties: level.properties.clone(),
                entities,
            },
            NotSaved,
        ));
        for file in files.into_iter().filter_map(|(_, file)| file) {
            watch_once(&self.assets, path, file);
        }
        Ok(root)
    }

    /// Despawns a level spawned with [`spawn_level`](Self::spawn_level):
    /// its root and everything spawned for it. `false` if `root` isn't a
    /// level's root.
    pub fn despawn_level(&mut self, root: Entity) -> bool {
        let Some(level) = self.world.get::<LevelRoot>(root) else {
            return false;
        };
        for e in level.entities.clone() {
            self.world.despawn(e);
        }
        self.world.despawn(root)
    }

    /// Spawns `level`'s layers, objects and colliders at `at`; returns what
    /// it spawned. Despawns it all again on an error.
    fn spawn_level_parts(&mut self, level: &Level, at: Vec2, options: &LevelOptions) -> Result<Vec<Entity>, Error> {
        let mut spawned = Vec::new();
        let result = self.spawn_level_into(level, at, options, &mut spawned);
        if result.is_err() {
            for &e in &spawned {
                self.world.despawn(e);
            }
        }
        result.map(|()| spawned)
    }

    fn spawn_level_into(
        &mut self,
        level: &Level,
        at: Vec2,
        options: &LevelOptions,
        spawned: &mut Vec<Entity>,
    ) -> Result<(), Error> {
        // Sheets for the tilesets the level uses.
        let mut sheets: Vec<Option<SpriteSheet>> = vec![None; level.tilesets.len()];
        let mut sheet = |ctx: &mut Ctx, i: u16| -> Result<SpriteSheet, Error> {
            let slot = &mut sheets[usize::from(i)];
            if let Some(s) = slot {
                return Ok(*s);
            }
            let s = sheet_of(ctx, &level.tilesets[usize::from(i)])?;
            *slot = Some(s);
            Ok(s)
        };
        let parallax_origin = at + level.parallax_origin;
        for (index, layer) in level.layers.iter().enumerate() {
            let z = level.layer_z(index, options);
            let info = LevelLayer {
                name: layer.name.clone(),
                class: layer.class.clone(),
                index,
                properties: layer.properties.clone(),
            };
            let color = Color {
                a: layer.tint.a * layer.opacity,
                ..layer.tint
            };
            let origin = at + layer.offset;
            match &layer.kind {
                LayerKind::Tiles(tiles) if layer.visible => {
                    // One map per tileset, and another where tiles stack.
                    let mut maps: Vec<(u16, TileMap)> = Vec::new();
                    for t in &tiles.tiles {
                        if t.cell.x >= tiles.size.x || t.cell.y >= tiles.size.y {
                            continue;
                        }
                        let free = maps
                            .iter()
                            .position(|(set, m)| *set == t.tileset && m.get(t.cell.x, t.cell.y).is_none());
                        let i = match free {
                            Some(i) => i,
                            None => {
                                let s = sheet(self, t.tileset)?;
                                let mut map = TileMap::new(s, tiles.size.x, tiles.size.y);
                                map.tile_size = tiles.cell_size;
                                map.parallax = layer.parallax;
                                map.parallax_origin = parallax_origin;
                                map.color = color;
                                // Stacked maps go just above their layer.
                                map.z = z + maps.len() as f32 * 1e-3;
                                let set = &level.tilesets[usize::from(t.tileset)];
                                for (&id, tile) in &set.tiles {
                                    if !tile.animation.is_empty() && id < u32::from(TileMap::EMPTY) {
                                        let frames: Vec<(u16, f32)> =
                                            tile.animation.iter().map(|&(f, d)| (f as u16, d)).collect();
                                        map.animate(id as u16, &frames);
                                    }
                                }
                                maps.push((t.tileset, map));
                                maps.len() - 1
                            }
                        };
                        let frame = u16::try_from(t.id).ok().filter(|&f| f != TileMap::EMPTY);
                        maps[i].1.set_flipped(t.cell.x, t.cell.y, frame, t.flip);
                    }
                    for (_, map) in maps {
                        spawned.push(self.world.spawn((Transform2D::at(origin), map, info.clone(), NotSaved)));
                    }
                }
                LayerKind::Image(image) if layer.visible => {
                    let tex = self.gfx.try_load_texture(image)?;
                    let mut map = TileMap::new(SpriteSheet::new(tex, tex.width(), tex.height()), 1, 1);
                    map.set(0, 0, Some(0));
                    map.parallax = layer.parallax;
                    map.parallax_origin = parallax_origin;
                    map.color = color;
                    map.z = z;
                    spawned.push(self.world.spawn((Transform2D::at(origin), map, info.clone(), NotSaved)));
                }
                LayerKind::Objects(objects) => {
                    for obj in objects {
                        let e = self.spawn_level_object(level, obj, origin, z, color, options, &mut sheet)?;
                        spawned.extend(e);
                    }
                }
                _ => {}
            }
        }
        #[cfg(feature = "physics")]
        if !options.no_colliders {
            self.spawn_level_colliders(level, at, options, spawned);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_level_object(
        &mut self,
        level: &Level,
        obj: &LevelObject,
        origin: Vec2,
        z: f32,
        color: Color,
        options: &LevelOptions,
        sheet: &mut impl FnMut(&mut Ctx, u16) -> Result<SpriteSheet, Error>,
    ) -> Result<Option<Entity>, Error> {
        let position = origin + obj.position;
        let prefab = obj.properties.str("prefab").map(str::to_string).or_else(|| {
            options
                .prefabs
                .iter()
                .find(|(class, _)| *class == obj.class || (!obj.name.is_empty() && *class == obj.name))
                .map(|(_, path)| path.clone())
        });
        if let Some(path) = prefab {
            // A world save holds it (it isn't `NotSaved`).
            if options.no_saved_objects {
                return Ok(None);
            }
            let e = if obj.rotation == 0.0 {
                self.spawn_prefab(&path, position)?
            } else {
                let t = Transform2D {
                    rotation: obj.rotation,
                    ..Transform2D::at(position)
                };
                self.spawn_prefab(&path, t)?
            };
            self.world.insert(e, obj.clone());
            return Ok(Some(e));
        }
        let transform = Transform2D {
            rotation: obj.rotation,
            ..Transform2D::at(position)
        };
        let e = self.world.spawn((transform, obj.clone(), NotSaved));
        if let Some((set, id, flip)) = obj.tile
            && obj.visible
            && usize::from(set) < level.tilesets.len()
        {
            let s = sheet(self, set)?;
            let mut sprite = s.sprite(id);
            sprite.z = z;
            sprite.color = color;
            let mut scale = obj.size / s.frame_size.max(Vec2::ONE);
            if flip & TileMap::FLIP_X != 0 {
                scale.x = -scale.x;
            }
            if flip & TileMap::FLIP_Y != 0 {
                scale.y = -scale.y;
            }
            self.world.insert(e, Transform2D { scale, ..transform });
            self.world.insert(e, sprite);
        }
        Ok(Some(e))
    }

    /// Fixed colliders: merged rectangles for the solid cells, and the
    /// collision shapes of tiles whose shapes don't cover them.
    #[cfg(feature = "physics")]
    fn spawn_level_colliders(&mut self, level: &Level, at: Vec2, options: &LevelOptions, spawned: &mut Vec<Entity>) {
        use crate::physics::{Collider, RigidBody};
        for r in level.solid_rects(options) {
            let (transform, body, collider) = super::colliders::rect_body(at, r);
            spawned.push(self.world.spawn((transform, body, collider, NotSaved)));
        }
        for layer in &level.layers {
            let LayerKind::Tiles(tiles) = &layer.kind else {
                continue;
            };
            let layer_solid = Level::layer_is_solid(layer, options);
            for t in &tiles.tiles {
                if level.tile_is_solid(layer_solid, t, options) {
                    continue;
                }
                let Some(info) = level.tile_info(t.tileset, t.id) else {
                    continue;
                };
                let set = &level.tilesets[usize::from(t.tileset)];
                // Shapes are in the tileset's pixels; the cell may be bigger.
                let scale = tiles.cell_size / set.tile_size.as_vec2().max(Vec2::ONE);
                let cell = at + layer.offset + t.cell.as_vec2() * tiles.cell_size;
                for s in &info.shapes {
                    let collider = match &s.shape {
                        ObjectShape::Rect => Collider::rect(s.size * scale),
                        ObjectShape::Ellipse => Collider::circle((s.size * scale).min_element() * 0.5),
                        ObjectShape::Polygon(points)
                            if (3..=crate::physics::MAX_POLYGON_POINTS).contains(&points.len()) =>
                        {
                            let pts: Vec<Vec2> = points.iter().map(|p| *p * scale).collect();
                            Collider::polygon(&pts)
                        }
                        _ => continue,
                    };
                    let transform = Transform2D {
                        rotation: s.rotation,
                        ..Transform2D::at(cell + s.position * scale)
                    };
                    spawned.push(self.world.spawn((transform, RigidBody::fixed(), collider, NotSaved)));
                }
            }
        }
    }

    /// Spawns every level loaded from `path` again (hot reload). On an
    /// error the old ones stay.
    pub(crate) fn reload_level(&mut self, path: &str) -> Result<(), String> {
        let roots: Vec<(Entity, Vec2, LevelOptions)> = self
            .world
            .query::<(Entity, &Transform2D, &LevelRoot)>()
            .iter()
            .filter(|(_, _, r)| r.path == path)
            .map(|(e, t, r)| (e, t.position, r.options.clone()))
            .collect();
        for (root, at, options) in roots {
            let (level, _) = Level::read(&self.assets, path, &options).map_err(|e| e.to_string())?;
            let old = self
                .world
                .get::<LevelRoot>(root)
                .map(|r| r.entities.clone())
                .unwrap_or_default();
            for e in old {
                self.world.despawn(e);
            }
            let entities = self
                .spawn_level_parts(&level, at, &options)
                .map_err(|e| e.to_string())?;
            if let Some(r) = self.world.get_mut::<LevelRoot>(root) {
                r.entities = entities;
                r.properties = level.properties;
            }
        }
        Ok(())
    }
}

/// Watches `file` for the level `path`, once.
fn watch_once(assets: &Assets, path: &str, file: PathBuf) {
    if !assets.watcher.watching(&file, |a| matches!(a, Asset::Level)) {
        assets.watcher.watch(path, file, Asset::Level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_resolve_against_the_file_that_names_them() {
        assert_eq!(resolve("levels/one.tmx", "../tiles/ground.png"), "tiles/ground.png");
        assert_eq!(resolve("levels/one.tmx", "./ground.tsx"), "levels/ground.tsx");
        assert_eq!(resolve("one.tmx", "ground.png"), "ground.png");
        assert_eq!(resolve("/tmp/x/one.tmx", "../t.png"), "/tmp/t.png");
        assert_eq!(resolve("a/b.tmx", "../../t.png"), "../t.png");
    }

    #[test]
    fn colors_read_with_alpha_first_as_tiled_writes_them() {
        let c = parse_color("#80ff0000").unwrap();
        assert!((c.a - 128.0 / 255.0).abs() < 1e-6 && c.r == 1.0 && c.g == 0.0);
        assert_eq!(parse_color("#00ff00"), Some(Color::rgba(0.0, 1.0, 0.0, 1.0)));
        assert_eq!(parse_color("nope"), None);
    }
}
