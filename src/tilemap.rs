//! Tilemaps: grids of tiles from a [`SpriteSheet`], drawn only where the
//! camera looks.
//!
//! - [`TileMap`]: one layer of tiles, with [animated tiles](TileMap::animate),
//!   flipped tiles and [parallax](TileMap::parallax).
//! - [`AutoTile`]: picks each tile's frame from its neighbours (4- or
//!   8-neighbour bitmasks), for maps built in code.
//! - [`TileMap::solid_rects`] and (feature `physics`) [`TileMap::colliders`]:
//!   collision from tiles, merged into few rectangles.
//! - [`Level`]: levels made in [Tiled](https://www.mapeditor.org/) (`.tmx`,
//!   `.tmj`) and [LDtk](https://ldtk.io/) (`.ldtk`), spawned with
//!   [`Ctx::spawn_level`](crate::ctx::Ctx::spawn_level): every layer, tileset,
//!   object and custom property, reloaded when the file changes.

mod autotile;
mod colliders;
mod ldtk;
mod level;
mod tiled;
mod xml;

pub use autotile::{AutoTile, Neighbours};
pub use level::{
    Layer, LayerKind, Level, LevelLayer, LevelObject, LevelOptions, LevelRoot, ObjectShape, PlacedTile, Properties,
    TileInfo, TileLayer, Tileset,
};

use std::cell::RefCell;

use glam::{UVec2, Vec2};

use crate::color::Color;
use crate::ctx::Ctx;
use crate::ecs::Transform2D;
use crate::render::batch::{SpriteInstance, uv_region};
use crate::sheet::SpriteSheet;
use ponko_ecs::Component;

/// A grid of tiles, each a frame of `sheet` or empty. Put it on an entity
/// with a [`d2::Transform`](crate::d2::Transform): its position is the map's top-left corner (the
/// rotation and scale are ignored). The built-in [`draw_tilemaps`](crate::systems::draw_tilemaps) system
/// draws the tiles the camera can see, so maps can be huge.
///
/// ```
/// # use ponko::prelude::*;
/// # fn setup(ctx: &mut Ctx, tiles: SpriteSheet) {
/// let mut map = ponko::tilemap::TileMap::new(tiles, 200, 100); // 200 × 100 tiles, all empty
/// for x in 0..200 {
///     map.set(x, 99, Some(0)); // a floor of tile 0
/// }
/// ctx.world.spawn((ponko::d2::Transform::at(vec2(0.0, 0.0)), map));
/// # }
/// ```
///
/// Layers are separate maps: give each its own entity and `z`. A map from a
/// level editor comes from [`Ctx::spawn_level`](crate::ctx::Ctx::spawn_level).
#[derive(Debug, Clone, PartialEq, Component)]
#[non_exhaustive]
pub struct TileMap {
    /// The tiles' images: tile `n` is frame `n` of this sheet.
    pub sheet: SpriteSheet,
    /// Map size in tiles.
    pub size: UVec2,
    /// Size of one tile in world units. Default: the sheet's frame size.
    pub tile_size: Vec2,
    /// Draw order. Default `0`: maps are drawn before sprites, so sprites
    /// with the same `z` are on top. (Keeping every `z` equal also keeps
    /// drawing on its fast path, with no sorting.)
    pub z: f32,
    /// Tint for every tile. Default white.
    pub color: Color,
    /// Frame index per tile, row by row; [`TileMap::EMPTY`] draws nothing.
    ///
    /// `u16` where [`SpriteSheet::frame`] takes a `u32`: on purpose. A map
    /// stores one index per tile, so `u16` halves its memory (a 4096 × 4096
    /// map is 32 MiB instead of 64 MiB), and a tile sheet never needs more
    /// than 65 535 frames. Pass a tile to the sheet with `u32::from(tile)`.
    pub tiles: Vec<u16>,
    /// How far the map moves with the camera: `(1, 1)` (the default) moves
    /// with the world, `(0.5, 0.5)` moves half as far (a far background),
    /// `(0, 0)` stays on the screen. Lines up with the world where the
    /// camera's center is at [`parallax_origin`](Self::parallax_origin).
    pub parallax: Vec2,
    /// Where a parallax map is drawn at its place: when the camera's center
    /// is here. Default `(0, 0)`.
    pub parallax_origin: Vec2,
    /// Animated tiles: wherever a map holds an animation's
    /// [`tile`](TileAnimation::tile), its frames play instead. Set with
    /// [`animate`](Self::animate).
    pub animations: Vec<TileAnimation>,
    /// Empty (no flipped tiles), or one byte per tile, row by row, of
    /// [`FLIP_X`](Self::FLIP_X), [`FLIP_Y`](Self::FLIP_Y) and
    /// [`FLIP_DIAGONAL`](Self::FLIP_DIAGONAL). Set with
    /// [`set_flipped`](Self::set_flipped).
    pub flips: Vec<u8>,
}

/// One animated tile of a [`TileMap`]: wherever the map holds `tile`, the
/// frames play in a loop, each for its time in seconds. The clock is the
/// game's ([`Time::elapsed`](crate::time::Time::elapsed)), so every copy of
/// the tile is in step.
#[derive(Debug, Clone, PartialEq)]
pub struct TileAnimation {
    /// The tile value the map holds.
    pub tile: u16,
    /// The frames to show and how long each stays, in seconds.
    pub frames: Vec<(u16, f32)>,
}

impl TileAnimation {
    /// The frame showing `seconds` into the animation.
    ///
    /// ```
    /// let water = ponko::tilemap::TileAnimation { tile: 4, frames: vec![(4, 0.5), (5, 0.25)] };
    /// assert_eq!(water.frame_at(0.6), 5);
    /// assert_eq!(water.frame_at(0.8), 4); // loops after 0.75 s
    /// ```
    #[must_use]
    pub fn frame_at(&self, seconds: f64) -> u16 {
        let total: f64 = self.frames.iter().map(|&(_, d)| f64::from(d.max(0.0))).sum();
        let Some(&(first, _)) = self.frames.first() else {
            return self.tile;
        };
        if total <= 0.0 || !seconds.is_finite() {
            return first;
        }
        let mut t = seconds.rem_euclid(total);
        for &(frame, d) in &self.frames {
            let d = f64::from(d.max(0.0));
            if t < d {
                return frame;
            }
            t -= d;
        }
        first
    }
}

impl TileMap {
    /// The tile value for "nothing here".
    pub const EMPTY: u16 = u16::MAX;

    /// An empty `width` × `height` map using `sheet`'s frames as tiles.
    #[must_use]
    pub fn new(sheet: SpriteSheet, width: u32, height: u32) -> Self {
        Self {
            sheet,
            size: UVec2::new(width, height),
            tile_size: sheet.frame_size,
            z: 0.0,
            color: Color::WHITE,
            tiles: vec![Self::EMPTY; width as usize * height as usize],
            parallax: Vec2::ONE,
            parallax_origin: Vec2::ZERO,
            animations: Vec::new(),
            flips: Vec::new(),
        }
    }

    /// Mirror the tile left to right ([`flips`](Self::flips)).
    pub const FLIP_X: u8 = 1;
    /// Mirror the tile top to bottom.
    pub const FLIP_Y: u8 = 2;
    /// Swap the tile's x and y axes (Tiled's diagonal flip), before the
    /// other two: with `FLIP_X` it turns the tile a quarter clockwise.
    pub const FLIP_DIAGONAL: u8 = 4;

    /// Sets tile `(x, y)` like [`set`](Self::set), flipped by `flip`
    /// ([`FLIP_X`](Self::FLIP_X) | [`FLIP_Y`](Self::FLIP_Y) |
    /// [`FLIP_DIAGONAL`](Self::FLIP_DIAGONAL), `0` for none). Out-of-bounds
    /// positions are ignored.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(map: &mut ponko::tilemap::TileMap) {
    /// use ponko::tilemap::TileMap;
    /// map.set_flipped(2, 3, Some(5), TileMap::FLIP_X); // tile 5, mirrored
    /// assert_eq!(map.flip(2, 3), TileMap::FLIP_X);
    /// # }
    /// ```
    pub fn set_flipped(&mut self, x: u32, y: u32, frame: Option<u16>, flip: u8) {
        if x >= self.size.x || y >= self.size.y {
            return;
        }
        let i = (y * self.size.x + x) as usize;
        self.tiles[i] = frame.unwrap_or(Self::EMPTY);
        if flip != 0 && self.flips.len() != self.tiles.len() {
            self.flips = vec![0; self.tiles.len()];
        }
        if let Some(f) = self.flips.get_mut(i) {
            *f = flip & (Self::FLIP_X | Self::FLIP_Y | Self::FLIP_DIAGONAL);
        }
    }

    /// How tile `(x, y)` is flipped (`0` if not, or out of bounds).
    #[must_use]
    pub fn flip(&self, x: u32, y: u32) -> u8 {
        if x >= self.size.x || y >= self.size.y {
            return 0;
        }
        self.flips.get((y * self.size.x + x) as usize).copied().unwrap_or(0)
    }

    /// Animates `tile`: wherever the map holds it, `frames` play in a loop,
    /// each `(frame, seconds)`. Replaces the tile's earlier animation; no
    /// frames removes it. Drawing stays as cheap as for still tiles.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(map: &mut ponko::tilemap::TileMap) {
    /// const WATER: u16 = 8;
    /// map.animate(WATER, &[(8, 0.2), (9, 0.2), (10, 0.2)]); // every water tile ripples
    /// # }
    /// ```
    pub fn animate(&mut self, tile: u16, frames: &[(u16, f32)]) {
        self.animations.retain(|a| a.tile != tile);
        if !frames.is_empty() {
            self.animations.push(TileAnimation {
                tile,
                frames: frames.to_vec(),
            });
        }
    }

    /// The map's top-left corner as drawn when its entity is at `position`
    /// and the camera's center at `camera_center`: `position` itself unless
    /// the map has [parallax](Self::parallax).
    #[must_use]
    pub fn drawn_origin(&self, position: Vec2, camera_center: Vec2) -> Vec2 {
        position + (camera_center - self.parallax_origin) * (Vec2::ONE - self.parallax)
    }

    /// Sets tile `(x, y)` to frame `frame`, or empties it with `None`.
    /// Out-of-bounds positions are ignored.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(map: &mut ponko::tilemap::TileMap) {
    /// map.set(3, 4, Some(7));
    /// map.set(3, 5, None);
    /// # }
    /// ```
    pub fn set(&mut self, x: u32, y: u32, frame: Option<u16>) {
        if x < self.size.x && y < self.size.y {
            self.tiles[(y * self.size.x + x) as usize] = frame.unwrap_or(Self::EMPTY);
        }
    }

    /// The frame at tile `(x, y)`, or `None` if empty or out of bounds.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(map: &ponko::tilemap::TileMap) {
    /// let solid = map.get(10, 2).is_some();
    /// # }
    /// ```
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> Option<u16> {
        if x >= self.size.x || y >= self.size.y {
            return None;
        }
        let t = self.tiles[(y * self.size.x + x) as usize];
        (t != Self::EMPTY).then_some(t)
    }

    /// The tile containing world point `world`, for a map whose top-left is
    /// at `origin`; `None` outside the map.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn paint(ctx: &mut Ctx, map: &mut ponko::tilemap::TileMap) {
    /// if let Some(tile) = map.tile_at(Vec2::ZERO, ctx.mouse_world()) {
    ///     map.set(tile.x, tile.y, Some(1));
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn tile_at(&self, origin: Vec2, world: Vec2) -> Option<UVec2> {
        let t = ((world - origin) / self.tile_size).floor();
        (t.x >= 0.0 && t.y >= 0.0 && t.x < self.size.x as f32 && t.y < self.size.y as f32)
            .then(|| UVec2::new(t.x as u32, t.y as u32))
    }
}

/// The system behind [`systems::draw_tilemaps`](crate::systems::draw_tilemaps); it keeps
/// this path for its name (`ponko::tilemap::draw_tilemaps`).
pub(crate) fn draw_tilemaps(ctx: &mut Ctx) {
    let view = ctx.camera.visible_rect();
    let center = (view.min + view.max) * 0.5;
    let time = ctx.time.elapsed();
    let list = &mut ctx.gfx.draw_list;
    ctx.world.query::<(&Transform2D, &TileMap)>().for_each(|(t, map)| {
        let origin = if map.parallax == Vec2::ONE {
            t.position
        } else {
            map.drawn_origin(t.position, center)
        };
        let ts = map.tile_size;
        let lo = ((view.min - origin) / ts).floor().max(Vec2::ZERO);
        let hi = ((view.max - origin) / ts).ceil().min(map.size.as_vec2());
        if lo.x >= hi.x || lo.y >= hi.y {
            return;
        }
        let tex = map.sheet.texture;
        let tex_size = tex.size().max(Vec2::ONE);
        let color = map.color.to_rgba8();
        let uv_size = (map.sheet.frame_size / tex_size).into();
        let (xs, ys) = (lo.x as u32..hi.x as u32, lo.y as u32..hi.y as u32);
        if map.animations.is_empty() && map.flips.is_empty() {
            // The common case, kept exactly as cheap as before animations.
            for y in ys {
                let row = (y * map.size.x) as usize;
                for x in xs.clone() {
                    let tile = map.tiles[row + x as usize];
                    if tile == TileMap::EMPTY {
                        continue;
                    }
                    let frame = map.sheet.frame(u32::from(tile));
                    let pos = origin + Vec2::new(x as f32, y as f32) * ts;
                    let instance = SpriteInstance::new(pos.into(), ts.into(), [0.0, 0.0], 0.0, color)
                        .with_uv(uv_region((frame.min / tex_size).into(), uv_size));
                    list.push(tex.id, map.z, instance);
                }
            }
            return;
        }
        FRAME_OF.with_borrow_mut(|frame_of| {
            animation_frames(map, time, frame_of);
            for y in ys {
                let row = (y * map.size.x) as usize;
                for x in xs.clone() {
                    let i = row + x as usize;
                    let mut tile = map.tiles[i];
                    if tile == TileMap::EMPTY {
                        continue;
                    }
                    if let Some(&shown) = frame_of.get(usize::from(tile)) {
                        tile = shown;
                    }
                    let frame = map.sheet.frame(u32::from(tile));
                    let uv = uv_region((frame.min / tex_size).into(), uv_size);
                    let cell = origin + Vec2::new(x as f32, y as f32) * ts;
                    let flip = map.flips.get(i).copied().unwrap_or(0);
                    let instance = if flip == 0 {
                        SpriteInstance::new(cell.into(), ts.into(), [0.0, 0.0], 0.0, color)
                    } else {
                        let (size, rotation) = flipped(flip, ts);
                        SpriteInstance::new((cell + ts * 0.5).into(), size.into(), [0.5, 0.5], rotation, color)
                    };
                    list.push(tex.id, map.z, instance.with_uv(uv));
                }
            }
        });
    });
}

thread_local! {
    /// Scratch for [`draw_tilemaps`]: the frame each tile value shows now,
    /// reused so drawing animated maps doesn't allocate every frame.
    static FRAME_OF: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
}

/// Fills `frame_of[tile]` with the frame `tile` shows at `time`: itself, or
/// its animation's current frame. Empty when the map has no animations.
fn animation_frames(map: &TileMap, time: f64, frame_of: &mut Vec<u16>) {
    frame_of.clear();
    let Some(highest) = map.animations.iter().map(|a| a.tile).max() else {
        return;
    };
    frame_of.extend(0..=highest);
    for a in &map.animations {
        frame_of[usize::from(a.tile)] = a.frame_at(time);
    }
}

/// The instance size and rotation that draw a tile flipped by `flip`
/// (pivot at the tile's center). A diagonal flip is a quarter turn with one
/// axis mirrored: the shader turns `(x, y)` into `(-y, x)` at +90°.
fn flipped(flip: u8, ts: Vec2) -> (Vec2, f32) {
    let sign = |on: bool| if on { -1.0 } else { 1.0 };
    let (fx, fy) = (flip & TileMap::FLIP_X != 0, flip & TileMap::FLIP_Y != 0);
    if flip & TileMap::FLIP_DIAGONAL == 0 {
        (Vec2::new(ts.x * sign(fx), ts.y * sign(fy)), 0.0)
    } else {
        // Turned a quarter: the x extent on screen comes from the size's y.
        (
            Vec2::new(ts.y * sign(fy), -ts.x * sign(fx)),
            std::f32::consts::FRAC_PI_2,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where texture corner `corner` (0..1 each way) lands, as the sprite
    /// shader places it, relative to the tile's center.
    fn lands(flip: u8, corner: Vec2) -> Vec2 {
        let (size, rot) = flipped(flip, Vec2::splat(16.0));
        let local = (corner - Vec2::splat(0.5)) * size;
        let (s, c) = rot.sin_cos();
        let p = Vec2::new(local.x * c - local.y * s, local.x * s + local.y * c);
        Vec2::new(p.x.round(), p.y.round())
    }

    #[test]
    fn flips_put_the_texture_corners_where_tiled_puts_them() {
        let (tl, tr) = (Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0));
        let (bl, half) = (Vec2::new(0.0, 1.0), 8.0);
        // No flip: the texture's top-left is at the tile's top-left.
        assert_eq!(lands(0, tl), Vec2::new(-half, -half));
        assert_eq!(lands(TileMap::FLIP_X, tl), Vec2::new(half, -half));
        assert_eq!(lands(TileMap::FLIP_Y, tl), Vec2::new(-half, half));
        // Diagonal: x and y swap, so the top-right goes to the bottom-left.
        assert_eq!(lands(TileMap::FLIP_DIAGONAL, tl), Vec2::new(-half, -half));
        assert_eq!(lands(TileMap::FLIP_DIAGONAL, tr), Vec2::new(-half, half));
        assert_eq!(lands(TileMap::FLIP_DIAGONAL, bl), Vec2::new(half, -half));
        // Diagonal then x: a quarter turn clockwise (top-left to top-right).
        let cw = TileMap::FLIP_DIAGONAL | TileMap::FLIP_X;
        assert_eq!(lands(cw, tl), Vec2::new(half, -half));
        assert_eq!(lands(cw, tr), Vec2::new(half, half));
        // Diagonal then y: a quarter turn counterclockwise.
        let ccw = TileMap::FLIP_DIAGONAL | TileMap::FLIP_Y;
        assert_eq!(lands(ccw, tl), Vec2::new(-half, half));
    }

    #[test]
    fn animation_frames_follow_the_clock_and_leave_other_tiles_alone() {
        let tex = crate::texture::Texture::new(1, 64, 16);
        let mut map = TileMap::new(SpriteSheet::new(tex, 16, 16), 2, 2);
        let mut frames = Vec::new();
        animation_frames(&map, 0.0, &mut frames);
        assert!(frames.is_empty());
        map.animate(2, &[(2, 0.5), (3, 0.5)]);
        animation_frames(&map, 0.75, &mut frames);
        assert_eq!(frames, [0, 1, 3]);
        animation_frames(&map, 1.25, &mut frames);
        assert_eq!(frames, [0, 1, 2]);
        map.animate(2, &[]);
        assert!(map.animations.is_empty());
    }
}
