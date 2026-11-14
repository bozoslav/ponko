//! Autotiling: each tile's frame picked from which of its neighbours are
//! filled, so walls, paths and shores join up by themselves.

use super::TileMap;

/// Which neighbours an [`AutoTile`] looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Neighbours {
    /// Up, right, down and left: 16 combinations, one frame each.
    Four,
    /// The four sides and the four corners. A corner only counts when both
    /// sides next to it are filled too, which leaves 47 combinations (the
    /// "blob" tileset).
    Eight,
}

/// An autotile rule: the frame for every combination of filled neighbours.
///
/// A tile's *mask* has a bit for each filled neighbour:
///
/// - [`Neighbours::Four`]: [`N`](Self::N) = 1, [`E`](Self::E) = 2,
///   [`S`](Self::S) = 4, [`W`](Self::W) = 8, so masks are `0..16`.
/// - [`Neighbours::Eight`]: [`N8`](Self::N8) = 1, [`NE`](Self::NE) = 2,
///   [`E8`](Self::E8) = 4, [`SE`](Self::SE) = 8, [`S8`](Self::S8) = 16,
///   [`SW`](Self::SW) = 32, [`W8`](Self::W8) = 64, [`NW`](Self::NW) = 128,
///   with corner bits cleared unless both sides next to them are set: the 47
///   masks of [`BLOB_MASKS`](Self::BLOB_MASKS).
///
/// North is up (smaller `y`). Cells outside the map count as filled (walls
/// run on past the edge); [`outside`](Self::outside) changes that.
///
/// ```
/// use ponko::tilemap::{AutoTile, TileMap};
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx, sheet: SpriteSheet) {
/// // Frame `mask` of the sheet is the wall piece for that mask.
/// let walls = AutoTile::four(std::array::from_fn(|mask| mask as u16));
/// let level = ["#####", "#...#", "#.#.#", "#####"];
/// let mut map = TileMap::new(sheet, 5, 4);
/// map.autotile(&walls, |x, y| level[y as usize].as_bytes()[x as usize] == b'#');
/// // Then while the game runs: knock a wall down; its neighbours follow.
/// map.paint_autotile(&walls, 2, 2, false);
/// # }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct AutoTile {
    neighbours: Neighbours,
    /// Frame by mask: 16 entries for four neighbours, 256 for eight.
    frames: Vec<u16>,
    /// Every frame the rule uses, sorted: a cell holding one is filled.
    owned: Vec<u16>,
    outside: bool,
}

impl AutoTile {
    /// Filled neighbour above ([`Neighbours::Four`]).
    pub const N: u8 = 1;
    /// Filled neighbour to the right ([`Neighbours::Four`]).
    pub const E: u8 = 2;
    /// Filled neighbour below ([`Neighbours::Four`]).
    pub const S: u8 = 4;
    /// Filled neighbour to the left ([`Neighbours::Four`]).
    pub const W: u8 = 8;
    /// Above ([`Neighbours::Eight`]).
    pub const N8: u8 = 1;
    /// Above and to the right ([`Neighbours::Eight`]).
    pub const NE: u8 = 2;
    /// To the right ([`Neighbours::Eight`]).
    pub const E8: u8 = 4;
    /// Below and to the right ([`Neighbours::Eight`]).
    pub const SE: u8 = 8;
    /// Below ([`Neighbours::Eight`]).
    pub const S8: u8 = 16;
    /// Below and to the left ([`Neighbours::Eight`]).
    pub const SW: u8 = 32;
    /// To the left ([`Neighbours::Eight`]).
    pub const W8: u8 = 64;
    /// Above and to the left ([`Neighbours::Eight`]).
    pub const NW: u8 = 128;

    /// The 47 eight-neighbour masks that can happen (corners only next to
    /// both their sides), in increasing order: the order of
    /// [`blob`](Self::blob)'s frames.
    pub const BLOB_MASKS: [u8; 47] = blob_masks();

    /// A four-neighbour rule: `frames[mask]` for each of the 16 masks.
    #[must_use]
    pub fn four(frames: [u16; 16]) -> Self {
        Self::with(Neighbours::Four, frames.to_vec())
    }

    /// An eight-neighbour rule: `frame_for(mask)` for each of the 47
    /// [`BLOB_MASKS`](Self::BLOB_MASKS).
    ///
    /// ```
    /// use ponko::tilemap::AutoTile;
    /// // A tileset laid out in mask order: frame i is BLOB_MASKS[i].
    /// let rule = AutoTile::eight(|mask| AutoTile::BLOB_MASKS.iter().position(|&m| m == mask).unwrap() as u16);
    /// assert_eq!(AutoTile::BLOB_MASKS[..5], [0, 1, 4, 5, 7]);
    /// assert_eq!(rule.frame(AutoTile::N8 | AutoTile::E8 | AutoTile::NE), 4); // mask 7
    /// ```
    #[must_use]
    pub fn eight(frame_for: impl Fn(u8) -> u16) -> Self {
        let frames = (0..=255u8).map(|m| frame_for(reduce(m))).collect();
        Self::with(Neighbours::Eight, frames)
    }

    /// An eight-neighbour rule from a 47-frame blob tileset: `frames[i]` is
    /// the frame for mask [`BLOB_MASKS[i]`](Self::BLOB_MASKS).
    #[must_use]
    pub fn blob(frames: [u16; 47]) -> Self {
        Self::eight(|mask| {
            let i = Self::BLOB_MASKS.iter().position(|&m| m == mask).unwrap_or(0);
            frames[i]
        })
    }

    fn with(neighbours: Neighbours, frames: Vec<u16>) -> Self {
        let mut owned = frames.clone();
        owned.sort_unstable();
        owned.dedup();
        Self {
            neighbours,
            frames,
            owned,
            outside: true,
        }
    }

    /// Whether cells outside the map count as filled (default `true`).
    #[must_use]
    pub fn outside(self, filled: bool) -> Self {
        Self {
            outside: filled,
            ..self
        }
    }

    /// Which neighbours this rule looks at.
    #[must_use]
    pub fn neighbours(&self) -> Neighbours {
        self.neighbours
    }

    /// The frame for `mask` (corner bits are cleared as in
    /// [`BLOB_MASKS`](Self::BLOB_MASKS) first).
    #[must_use]
    pub fn frame(&self, mask: u8) -> u16 {
        match self.neighbours {
            Neighbours::Four => self.frames[usize::from(mask & 15)],
            Neighbours::Eight => self.frames[usize::from(reduce(mask))],
        }
    }

    /// Whether the rule uses `frame`: a cell holding it is filled for
    /// [`TileMap::paint_autotile`].
    #[must_use]
    pub fn owns(&self, frame: u16) -> bool {
        self.owned.binary_search(&frame).is_ok()
    }

    /// The mask of cell `(x, y)` when `filled(x, y)` says which cells are
    /// filled. `filled` is only asked about cells inside a `width` × `height`
    /// map.
    #[must_use]
    pub fn mask(&self, x: u32, y: u32, width: u32, height: u32, filled: impl Fn(u32, u32) -> bool) -> u8 {
        let at = |dx: i32, dy: i32| {
            let (nx, ny) = (x as i64 + i64::from(dx), y as i64 + i64::from(dy));
            if nx < 0 || ny < 0 || nx >= i64::from(width) || ny >= i64::from(height) {
                self.outside
            } else {
                filled(nx as u32, ny as u32)
            }
        };
        let bit = |on: bool, b: u8| if on { b } else { 0 };
        match self.neighbours {
            Neighbours::Four => {
                bit(at(0, -1), Self::N) | bit(at(1, 0), Self::E) | bit(at(0, 1), Self::S) | bit(at(-1, 0), Self::W)
            }
            Neighbours::Eight => reduce(
                bit(at(0, -1), Self::N8)
                    | bit(at(1, -1), Self::NE)
                    | bit(at(1, 0), Self::E8)
                    | bit(at(1, 1), Self::SE)
                    | bit(at(0, 1), Self::S8)
                    | bit(at(-1, 1), Self::SW)
                    | bit(at(-1, 0), Self::W8)
                    | bit(at(-1, -1), Self::NW),
            ),
        }
    }
}

/// Clears the corner bits whose two sides aren't both set.
const fn reduce(m: u8) -> u8 {
    let mut out = m & (AutoTile::N8 | AutoTile::E8 | AutoTile::S8 | AutoTile::W8);
    let corners = [
        (AutoTile::NE, AutoTile::N8, AutoTile::E8),
        (AutoTile::SE, AutoTile::S8, AutoTile::E8),
        (AutoTile::SW, AutoTile::S8, AutoTile::W8),
        (AutoTile::NW, AutoTile::N8, AutoTile::W8),
    ];
    let mut i = 0;
    while i < corners.len() {
        let (c, a, b) = corners[i];
        if m & c != 0 && m & a != 0 && m & b != 0 {
            out |= c;
        }
        i += 1;
    }
    out
}

const fn blob_masks() -> [u8; 47] {
    let mut out = [0u8; 47];
    let (mut m, mut n) = (0usize, 0usize);
    while m < 256 {
        if reduce(m as u8) == m as u8 {
            out[n] = m as u8;
            n += 1;
        }
        m += 1;
    }
    out
}

impl TileMap {
    /// Fills the map with `rule`: every cell `filled(x, y)` says is filled
    /// gets the frame for its neighbours; the other cells are left as they
    /// are (set them first for a floor under the walls, or leave them empty).
    pub fn autotile(&mut self, rule: &AutoTile, filled: impl Fn(u32, u32) -> bool) {
        let (w, h) = (self.size.x, self.size.y);
        for y in 0..h {
            for x in 0..w {
                if filled(x, y) {
                    let frame = rule.frame(rule.mask(x, y, w, h, &filled));
                    self.tiles[(y * w + x) as usize] = frame;
                }
            }
        }
    }

    /// Fills (`on`) or empties cell `(x, y)` and redoes it and its eight
    /// neighbours with `rule`, for painting while the game runs. A cell is
    /// filled when it holds one of the rule's frames ([`AutoTile::owns`]).
    /// Out-of-bounds positions are ignored.
    pub fn paint_autotile(&mut self, rule: &AutoTile, x: u32, y: u32, on: bool) {
        let (w, h) = (self.size.x, self.size.y);
        if x >= w || y >= h {
            return;
        }
        let i = (y * w + x) as usize;
        // Any owned frame marks the cell filled; the pass below fixes it.
        self.tiles[i] = if on { rule.frames[0] } else { TileMap::EMPTY };
        for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
            for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                let tiles = &self.tiles;
                let filled = |cx: u32, cy: u32| rule.owns(tiles[(cy * w + cx) as usize]);
                if filled(nx, ny) {
                    let frame = rule.frame(rule.mask(nx, ny, w, h, filled));
                    self.tiles[(ny * w + nx) as usize] = frame;
                }
            }
        }
    }
}
