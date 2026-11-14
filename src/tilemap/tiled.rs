//! Tiled maps: `.tmj` (JSON) read directly, `.tmx` (XML) turned into the
//! same JSON shape first, so one reader handles both. Tilesets (`.tsx`,
//! `.tsj`) and templates (`.tx`, `.tj`) the same way.

use std::collections::BTreeMap;
use std::io::Read as _;

use glam::{UVec2, Vec2};
use serde_json::{Map, Value, json};

use super::TileMap;
use super::level::{
    Layer, LayerKind, Level, LevelObject, ObjectShape, PlacedTile, Properties, Reader, TileInfo, TileLayer, Tileset,
    decode_error, flag, num, num_or, parse_color, resolve, text,
};
use super::xml::{self, Element};
use crate::color::Color;
use crate::error::{Error, ErrorKind};

const FLIP_H: u32 = 0x8000_0000;
const FLIP_V: u32 = 0x4000_0000;
const FLIP_D: u32 = 0x2000_0000;
const GID_MASK: u32 = 0x0fff_ffff;

// ---- XML to Tiled's JSON shape ---------------------------------------------

fn attrs(el: &Element) -> Map<String, Value> {
    el.attrs
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect()
}

/// `<properties>` as Tiled's JSON writes them: `[{name, type, value}]`.
fn props_json(el: &Element) -> Value {
    let Some(props) = el.child("properties") else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        props
            .all("property")
            .map(|p| {
                let ty = p.attr("type").unwrap_or("string");
                let value = if ty == "class" {
                    // Members are properties themselves: an object of them.
                    let members = props_json(p);
                    Value::Object(read_props(&members).0.into_iter().collect())
                } else {
                    let raw = p.attr("value").map_or_else(|| p.text.clone(), str::to_string);
                    typed(ty, &raw)
                };
                json!({ "name": p.attr("name").unwrap_or(""), "type": ty, "value": value })
            })
            .collect(),
    )
}

/// An XML property value as the type its `type` says.
fn typed(ty: &str, raw: &str) -> Value {
    match ty {
        "int" | "object" => raw.trim().parse::<i64>().map_or(Value::Null, Value::from),
        "float" => raw.trim().parse::<f64>().map_or(Value::Null, Value::from),
        "bool" => Value::Bool(raw.trim() == "true"),
        _ => Value::String(raw.to_string()),
    }
}

fn points_json(s: &str) -> Value {
    Value::Array(
        s.split_whitespace()
            .filter_map(|p| {
                let (x, y) = p.split_once(',')?;
                Some(json!({ "x": x.trim().parse::<f64>().ok()?, "y": y.trim().parse::<f64>().ok()? }))
            })
            .collect(),
    )
}

fn object_json(el: &Element) -> Value {
    let mut o = attrs(el);
    o.insert("properties".into(), props_json(el));
    for c in &el.children {
        match c.name.as_str() {
            "ellipse" => {
                o.insert("ellipse".into(), Value::Bool(true));
            }
            "point" => {
                o.insert("point".into(), Value::Bool(true));
            }
            "polygon" | "polyline" => {
                o.insert(c.name.clone(), points_json(c.attr("points").unwrap_or("")));
            }
            "text" => {
                o.insert("text".into(), json!({ "text": c.text }));
            }
            _ => {}
        }
    }
    Value::Object(o)
}

fn objects_json(el: &Element) -> Value {
    Value::Array(el.all("object").map(object_json).collect())
}

fn tileset_json(el: &Element) -> Value {
    let mut t = attrs(el);
    t.insert("properties".into(), props_json(el));
    if let Some(img) = el.child("image") {
        t.insert("image".into(), img.attr("source").unwrap_or("").into());
        t.insert("imagewidth".into(), img.attr("width").unwrap_or("0").into());
        t.insert("imageheight".into(), img.attr("height").unwrap_or("0").into());
    }
    let tiles: Vec<Value> = el
        .all("tile")
        .map(|tile| {
            let mut o = attrs(tile);
            o.insert("properties".into(), props_json(tile));
            if let Some(a) = tile.child("animation") {
                let frames: Vec<Value> = a.all("frame").map(|f| Value::Object(attrs(f))).collect();
                o.insert("animation".into(), Value::Array(frames));
            }
            if let Some(g) = tile.child("objectgroup") {
                o.insert("objectgroup".into(), json!({ "objects": objects_json(g) }));
            }
            if tile.child("image").is_some() {
                o.insert("image".into(), Value::Bool(true));
            }
            Value::Object(o)
        })
        .collect();
    t.insert("tiles".into(), Value::Array(tiles));
    Value::Object(t)
}

/// `<data>`'s tiles: a CSV or XML list as numbers, base64 as its string.
fn data_json(data: &Element) -> Value {
    match data.attr("encoding") {
        Some("csv") => Value::Array(
            data.text
                .split(',')
                .filter_map(|n| n.trim().parse::<u32>().ok())
                .map(Value::from)
                .collect(),
        ),
        Some(_) => Value::String(data.text.trim().to_string()),
        None => Value::Array(
            data.all("tile")
                .map(|t| Value::from(t.attr("gid").unwrap_or("0").parse::<u32>().unwrap_or(0)))
                .collect(),
        ),
    }
}

fn layers_json(el: &Element) -> Value {
    let mut out = Vec::new();
    for c in &el.children {
        let mut o = attrs(c);
        o.insert("properties".into(), props_json(c));
        match c.name.as_str() {
            "layer" => {
                o.insert("type".into(), "tilelayer".into());
                if let Some(d) = c.child("data") {
                    for key in ["encoding", "compression"] {
                        if let Some(v) = d.attr(key) {
                            o.insert(key.into(), v.into());
                        }
                    }
                    let chunks: Vec<Value> = d
                        .all("chunk")
                        .map(|ch| {
                            let mut co = attrs(ch);
                            let mut as_data = ch.clone();
                            as_data.attrs = d.attrs.clone();
                            co.insert("data".into(), data_json(&as_data));
                            Value::Object(co)
                        })
                        .collect();
                    if chunks.is_empty() {
                        o.insert("data".into(), data_json(d));
                    } else {
                        o.insert("chunks".into(), Value::Array(chunks));
                    }
                }
            }
            "objectgroup" => {
                o.insert("type".into(), "objectgroup".into());
                o.insert("objects".into(), objects_json(c));
            }
            "imagelayer" => {
                o.insert("type".into(), "imagelayer".into());
                if let Some(img) = c.child("image") {
                    o.insert("image".into(), img.attr("source").unwrap_or("").into());
                }
            }
            "group" => {
                o.insert("type".into(), "group".into());
                o.insert("layers".into(), layers_json(c));
            }
            _ => continue,
        }
        out.push(Value::Object(o));
    }
    Value::Array(out)
}

fn map_json(el: &Element) -> Value {
    let mut m = attrs(el);
    m.insert("properties".into(), props_json(el));
    m.insert(
        "tilesets".into(),
        Value::Array(el.all("tileset").map(tileset_json).collect()),
    );
    m.insert("layers".into(), layers_json(el));
    Value::Object(m)
}

fn parse_xml(text: &str, path: &str, root: &str) -> Result<Element, Error> {
    let el = xml::parse(text).map_err(|e| decode_error(format!("{path}: {e}")))?;
    if el.name != root {
        return Err(decode_error(format!(
            "{path}: expected a <{root}>, found <{}>",
            el.name
        )));
    }
    Ok(el)
}

// ---- reading -----------------------------------------------------------------

/// Tiled's `[{name, type, value}]` properties (an object of values is
/// accepted too).
pub(super) fn read_props(v: &Value) -> Properties {
    let mut out = BTreeMap::new();
    match v {
        Value::Array(list) => {
            for p in list {
                let name = text(p, "name").to_string();
                let value = p.get("value").cloned().unwrap_or(Value::Null);
                out.insert(name, value);
            }
        }
        Value::Object(map) => out.extend(map.iter().map(|(k, v)| (k.clone(), v.clone()))),
        _ => {}
    }
    Properties(out)
}

fn props_of(v: &Value) -> Properties {
    v.get("properties").map(read_props).unwrap_or_default()
}

/// The map's tilesets in gid order: first gid and the tileset.
struct Sets {
    first: Vec<u32>,
    sources: Vec<Option<String>>,
}

impl Sets {
    /// `(tileset index, tile id, flips)` of a gid; `None` for an empty one.
    fn lookup(&self, raw: u32) -> Option<(u16, u32, u8)> {
        let gid = raw & GID_MASK;
        if gid == 0 {
            return None;
        }
        let i = self.first.iter().rposition(|&f| f <= gid)?;
        let mut flip = 0;
        if raw & FLIP_H != 0 {
            flip |= TileMap::FLIP_X;
        }
        if raw & FLIP_V != 0 {
            flip |= TileMap::FLIP_Y;
        }
        if raw & FLIP_D != 0 {
            flip |= TileMap::FLIP_DIAGONAL;
        }
        Some((i as u16, gid - self.first[i], flip))
    }
}

pub(super) fn read_tmx(text: &str, path: &str, reader: &mut Reader) -> Result<Level, Error> {
    let map = map_json(&parse_xml(text, path, "map")?);
    read_map(&map, path, reader)
}

pub(super) fn read_tmj(text: &str, path: &str, reader: &mut Reader) -> Result<Level, Error> {
    let map: Value = serde_json::from_str(text)?;
    if map.get("type").and_then(Value::as_str).is_some_and(|t| t != "map") {
        return Err(decode_error(format!("{path} is not a Tiled map")));
    }
    read_map(&map, path, reader)
}

/// A tileset file (`.tsx` or `.tsj`) as JSON.
fn tileset_file(reader: &mut Reader, name: &str) -> Result<Value, Error> {
    let text = reader.text(name)?;
    if name.to_ascii_lowercase().ends_with(".tsx") {
        Ok(tileset_json(&parse_xml(&text, name, "tileset")?))
    } else {
        Ok(serde_json::from_str(&text).map_err(|e| Error::from(e).context(name))?)
    }
}

fn read_tileset(t: &Value, base: &str, reader: &mut Reader) -> Result<Tileset, Error> {
    let tw = num_or(t, "tilewidth", 0.0) as u32;
    let th = num_or(t, "tileheight", 0.0) as u32;
    let margin = num_or(t, "margin", 0.0) as u32;
    let spacing = num_or(t, "spacing", 0.0) as u32;
    let image = text(t, "image");
    let iw = num_or(t, "imagewidth", 0.0) as u32;
    let ih = num_or(t, "imageheight", 0.0) as u32;
    let mut columns = num_or(t, "columns", 0.0) as u32;
    if columns == 0 && tw > 0 {
        columns = (iw + spacing).saturating_sub(2 * margin) / (tw + spacing);
    }
    let mut count = num_or(t, "tilecount", 0.0) as u32;
    if count == 0 && th > 0 && columns > 0 {
        count = columns * ((ih + spacing).saturating_sub(2 * margin) / (th + spacing));
    }
    let mut tiles = BTreeMap::new();
    if let Some(Value::Array(list)) = t.get("tiles") {
        for tile in list {
            let Some(id) = num(tile, "id") else { continue };
            let class = match text(tile, "class") {
                "" => text(tile, "type"),
                c => c,
            };
            let animation = match tile.get("animation") {
                Some(Value::Array(frames)) => frames
                    .iter()
                    .map(|f| {
                        (
                            num_or(f, "tileid", 0.0) as u32,
                            num_or(f, "duration", 100.0) as f32 / 1000.0,
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let shapes = match tile.get("objectgroup").and_then(|g| g.get("objects")) {
                Some(Value::Array(objects)) => objects
                    .iter()
                    .map(|o| read_object(o, "", base, None, reader))
                    .collect::<Result<_, _>>()?,
                _ => Vec::new(),
            };
            let info = TileInfo {
                class: class.to_string(),
                properties: props_of(tile),
                animation,
                shapes,
                ..TileInfo::default()
            };
            tiles.insert(id as u32, info);
        }
    }
    Ok(Tileset {
        name: text(t, "name").to_string(),
        image: if image.is_empty() {
            String::new()
        } else {
            resolve(base, image)
        },
        tile_size: UVec2::new(tw, th),
        columns,
        count,
        margin,
        spacing,
        tiles,
    })
}

/// Base64 to bytes (whitespace skipped).
fn base64(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return Err(format!("not base64: {:?}", c as char)),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// A layer's or chunk's gids from `data` (numbers, or base64 with the
/// layer's compression).
fn gids(layer: &Value, data: &Value, path: &str) -> Result<Vec<u32>, Error> {
    match data {
        Value::Array(list) => Ok(list.iter().map(|v| v.as_u64().unwrap_or(0) as u32).collect()),
        Value::String(s) => {
            let raw = base64(s).map_err(|e| decode_error(format!("{path}: layer data: {e}")))?;
            let mut bytes = Vec::new();
            match text(layer, "compression") {
                "" => bytes = raw,
                "zlib" => {
                    flate2::read::ZlibDecoder::new(&raw[..])
                        .read_to_end(&mut bytes)
                        .map_err(|e| decode_error(format!("{path}: zlib layer data: {e}")))?;
                }
                "gzip" => {
                    flate2::read::GzDecoder::new(&raw[..])
                        .read_to_end(&mut bytes)
                        .map_err(|e| decode_error(format!("{path}: gzip layer data: {e}")))?;
                }
                other => {
                    return Err(Error::new(
                        ErrorKind::Unsupported,
                        format!("{path}: {other} compressed layers aren't read; save the map with zlib, gzip or CSV"),
                    ));
                }
            }
            Ok(bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect())
        }
        _ => Ok(Vec::new()),
    }
}

/// Inherited from groups: offset, parallax, opacity, tint, visibility.
#[derive(Clone, Copy)]
struct Inherited {
    offset: Vec2,
    parallax: Vec2,
    opacity: f32,
    tint: Color,
    visible: bool,
}

struct MapReader<'r, 'a> {
    path: String,
    tile: Vec2,
    sets: Sets,
    reader: &'r mut Reader<'a>,
    layers: Vec<Layer>,
}

impl MapReader<'_, '_> {
    fn layers(&mut self, list: &[Value], parent: Inherited) -> Result<(), Error> {
        for l in list {
            let tint = l
                .get("tintcolor")
                .and_then(Value::as_str)
                .and_then(parse_color)
                .unwrap_or(Color::WHITE);
            let here = Inherited {
                offset: parent.offset + Vec2::new(num_or(l, "offsetx", 0.0) as f32, num_or(l, "offsety", 0.0) as f32),
                parallax: parent.parallax
                    * Vec2::new(num_or(l, "parallaxx", 1.0) as f32, num_or(l, "parallaxy", 1.0) as f32),
                opacity: parent.opacity * num_or(l, "opacity", 1.0) as f32,
                tint: Color::rgba(
                    parent.tint.r * tint.r,
                    parent.tint.g * tint.g,
                    parent.tint.b * tint.b,
                    parent.tint.a * tint.a,
                ),
                visible: parent.visible && flag(l, "visible", true),
            };
            let name = text(l, "name").to_string();
            let mut offset = here.offset;
            let kind = match text(l, "type") {
                "group" => {
                    let sub = l.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
                    self.layers(&sub, here)?;
                    continue;
                }
                "tilelayer" => {
                    let (tiles, origin) = self.tile_layer(l)?;
                    offset += origin;
                    LayerKind::Tiles(tiles)
                }
                "objectgroup" => {
                    let mut objects = Vec::new();
                    for o in l.get("objects").and_then(Value::as_array).into_iter().flatten() {
                        if o.get("text").is_some() {
                            continue;
                        }
                        let path = self.path.clone();
                        objects.push(read_object(o, &name, &path, Some(&self.sets), self.reader)?);
                    }
                    LayerKind::Objects(objects)
                }
                "imagelayer" => match text(l, "image") {
                    "" => continue,
                    image => LayerKind::Image(resolve(&self.path, image)),
                },
                _ => continue,
            };
            self.layers.push(Layer {
                name,
                class: text(l, "class").to_string(),
                offset,
                parallax: here.parallax,
                opacity: here.opacity,
                tint: here.tint,
                visible: here.visible,
                properties: props_of(l),
                kind,
            });
        }
        Ok(())
    }

    /// A tile layer, and where its first cell is (infinite maps' chunks
    /// may start left of or above the origin).
    fn tile_layer(&mut self, l: &Value) -> Result<(TileLayer, Vec2), Error> {
        // (cell x, cell y, width, gids) per block of tiles.
        let mut blocks: Vec<(i64, i64, u32, Vec<u32>)> = Vec::new();
        if let Some(Value::Array(chunks)) = l.get("chunks") {
            for c in chunks {
                let data = c.get("data").cloned().unwrap_or(Value::Null);
                let g = gids(l, &data, &self.path)?;
                blocks.push((
                    num_or(c, "x", 0.0) as i64,
                    num_or(c, "y", 0.0) as i64,
                    num_or(c, "width", 0.0) as u32,
                    g,
                ));
            }
        } else {
            let data = l.get("data").cloned().unwrap_or(Value::Null);
            let g = gids(l, &data, &self.path)?;
            blocks.push((
                num_or(l, "x", 0.0) as i64,
                num_or(l, "y", 0.0) as i64,
                num_or(l, "width", 0.0) as u32,
                g,
            ));
        }
        let (mut min, mut max) = ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
        for (x, y, w, g) in &blocks {
            let h = if *w == 0 { 0 } else { g.len() as i64 / i64::from(*w) };
            min = (min.0.min(*x), min.1.min(*y));
            max = (max.0.max(x + i64::from(*w)), max.1.max(y + h));
        }
        if blocks.is_empty() || max.0 <= min.0 || max.1 <= min.1 {
            return Ok((
                TileLayer {
                    cell_size: self.tile,
                    ..TileLayer::default()
                },
                Vec2::ZERO,
            ));
        }
        let size = UVec2::new((max.0 - min.0) as u32, (max.1 - min.1) as u32);
        let mut tiles = Vec::new();
        for (bx, by, w, g) in &blocks {
            for (i, &raw) in g.iter().enumerate() {
                let Some((tileset, id, flip)) = self.sets.lookup(raw) else {
                    continue;
                };
                let (x, y) = (bx + (i as u32 % w) as i64 - min.0, by + (i as u32 / w) as i64 - min.1);
                tiles.push(PlacedTile {
                    cell: UVec2::new(x as u32, y as u32),
                    tileset,
                    id,
                    flip,
                });
            }
        }
        let origin = Vec2::new(min.0 as f32, min.1 as f32) * self.tile;
        Ok((
            TileLayer {
                size,
                cell_size: self.tile,
                tiles,
                ..TileLayer::default()
            },
            origin,
        ))
    }
}

/// Reads one object; `sets` maps its `gid` (none for a tile's collision
/// shapes). A `template` is read and the object's own values put over it.
fn read_object(
    o: &Value,
    layer: &str,
    base: &str,
    sets: Option<&Sets>,
    reader: &mut Reader,
) -> Result<LevelObject, Error> {
    let mut merged;
    let mut o = o;
    let mut gid_sets: Option<(Vec<u32>, Vec<String>)> = None;
    if let Some(template) = o.get("template").and_then(Value::as_str) {
        let name = resolve(base, template);
        let body = reader.text(&name)?;
        let t: Value = if name.to_ascii_lowercase().ends_with(".tx") {
            let el = parse_xml(&body, &name, "template")?;
            let mut t = Map::new();
            if let Some(ts) = el.child("tileset") {
                t.insert("tileset".into(), Value::Object(attrs(ts)));
            }
            if let Some(obj) = el.child("object") {
                t.insert("object".into(), object_json(obj));
            }
            Value::Object(t)
        } else {
            serde_json::from_str(&body).map_err(|e| Error::from(e).context(&name))?
        };
        merged = t.get("object").cloned().unwrap_or_else(|| json!({}));
        if let Some(ts) = t.get("tileset") {
            let first = num_or(ts, "firstgid", 1.0) as u32;
            gid_sets = Some((vec![first], vec![resolve(&name, text(ts, "source"))]));
        }
        // The instance's own values win; properties merge by name.
        let mut props = props_of(&merged).0;
        props.extend(props_of(o).0);
        if let (Value::Object(m), Value::Object(own)) = (&mut merged, o) {
            for (k, v) in own {
                if k != "template" && k != "properties" {
                    m.insert(k.clone(), v.clone());
                }
            }
            m.insert("properties".into(), Value::Object(props.into_iter().collect()));
        }
        o = &merged;
    }
    let pos = Vec2::new(num_or(o, "x", 0.0) as f32, num_or(o, "y", 0.0) as f32);
    let size = Vec2::new(num_or(o, "width", 0.0) as f32, num_or(o, "height", 0.0) as f32);
    let rotation = (num_or(o, "rotation", 0.0) as f32).to_radians();
    let turn = |v: Vec2| {
        let (s, c) = rotation.sin_cos();
        Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
    };
    let points = |key: &str| -> Option<Vec<Vec2>> {
        let list = o.get(key)?.as_array()?;
        Some(
            list.iter()
                .map(|p| Vec2::new(num_or(p, "x", 0.0) as f32, num_or(p, "y", 0.0) as f32))
                .collect(),
        )
    };
    let raw_gid = num(o, "gid").map(|g| g as u32);
    let tile = match (raw_gid, sets, &gid_sets) {
        (Some(raw), Some(sets), Some((tfirst, tsource))) => {
            // A template's gid counts in the template's own tileset: find
            // the map's copy of that tileset.
            let local = Sets {
                first: tfirst.clone(),
                sources: vec![Some(tsource[0].clone())],
            };
            local.lookup(raw).and_then(|(_, id, flip)| {
                let i = sets
                    .sources
                    .iter()
                    .position(|s| s.as_deref() == Some(tsource[0].as_str()))?;
                Some((i as u16, id, flip))
            })
        }
        (Some(raw), Some(sets), None) => sets.lookup(raw),
        _ => None,
    };
    let (shape, position) = if tile.is_some() {
        // Tile objects hang from their bottom-left corner.
        (ObjectShape::Rect, pos + turn(Vec2::new(size.x, -size.y) * 0.5))
    } else if flag(o, "point", false) {
        (ObjectShape::Point, pos)
    } else if let Some(p) = points("polygon") {
        (ObjectShape::Polygon(p), pos)
    } else if let Some(p) = points("polyline") {
        (ObjectShape::Polyline(p), pos)
    } else if flag(o, "ellipse", false) {
        (ObjectShape::Ellipse, pos + turn(size * 0.5))
    } else {
        (ObjectShape::Rect, pos + turn(size * 0.5))
    };
    let class = match text(o, "class") {
        "" => text(o, "type"),
        c => c,
    };
    Ok(LevelObject {
        id: num_or(o, "id", 0.0) as u64,
        name: text(o, "name").to_string(),
        class: class.to_string(),
        layer: layer.to_string(),
        position,
        size,
        rotation,
        shape,
        tile,
        visible: flag(o, "visible", true),
        properties: props_of(o),
    })
}

fn read_map(map: &Value, path: &str, reader: &mut Reader) -> Result<Level, Error> {
    match text(map, "orientation") {
        "" | "orthogonal" => {}
        other => {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!("{path}: {other} maps aren't read, only orthogonal ones"),
            ));
        }
    }
    let tile = Vec2::new(
        num_or(map, "tilewidth", 0.0) as f32,
        num_or(map, "tileheight", 0.0) as f32,
    );
    if tile.min_element() <= 0.0 {
        return Err(decode_error(format!("{path}: the map has no tile size")));
    }
    let mut tilesets = Vec::new();
    let mut sets = Sets {
        first: Vec::new(),
        sources: Vec::new(),
    };
    for t in map.get("tilesets").and_then(Value::as_array).into_iter().flatten() {
        let first = num_or(t, "firstgid", 1.0) as u32;
        let (set, source) = match t.get("source").and_then(Value::as_str) {
            Some(source) => {
                let name = resolve(path, source);
                let json = tileset_file(reader, &name)?;
                (read_tileset(&json, &name, reader)?, Some(name))
            }
            None => (read_tileset(t, path, reader)?, None),
        };
        tilesets.push(set);
        sets.first.push(first);
        sets.sources.push(source);
    }
    let mut r = MapReader {
        path: path.to_string(),
        tile,
        sets,
        reader,
        layers: Vec::new(),
    };
    let list = map.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
    r.layers(
        &list,
        Inherited {
            offset: Vec2::ZERO,
            parallax: Vec2::ONE,
            opacity: 1.0,
            tint: Color::WHITE,
            visible: true,
        },
    )?;
    let layers = r.layers;
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    Ok(Level {
        name: name.rsplit_once('.').map_or(name, |(n, _)| n).to_string(),
        size: UVec2::new(num_or(map, "width", 0.0) as u32, num_or(map, "height", 0.0) as u32),
        cell_size: tile,
        background: map.get("backgroundcolor").and_then(Value::as_str).and_then(parse_color),
        parallax_origin: Vec2::new(
            num_or(map, "parallaxoriginx", 0.0) as f32,
            num_or(map, "parallaxoriginy", 0.0) as f32,
        ),
        properties: props_of(map),
        tilesets,
        layers,
        files: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_with_padding_and_line_breaks() {
        assert_eq!(base64("AQAAAAIAAAA=").unwrap(), [1, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(base64("aGVs\n  bG8=").unwrap(), b"hello");
        assert!(base64("a*b").is_err());
    }

    #[test]
    fn gids_carry_their_flips_and_find_their_tileset() {
        let sets = Sets {
            first: vec![1, 11],
            sources: vec![None, None],
        };
        assert_eq!(sets.lookup(0), None);
        assert_eq!(sets.lookup(3), Some((0, 2, 0)));
        assert_eq!(
            sets.lookup(12 | FLIP_H | FLIP_D),
            Some((1, 1, TileMap::FLIP_X | TileMap::FLIP_DIAGONAL))
        );
    }
}
