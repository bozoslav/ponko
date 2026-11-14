//! Collision from tiles: solid cells merged into few rectangles.

use glam::Vec2;

use super::TileMap;
use crate::camera::Rect;

/// Covers the cells `solid(x, y)` picks in a `width` × `height` grid with
/// few rectangles, `[x, y, width, height]` in cells: each grows right as
/// far as it can, then down while the whole row below is solid. A room's
/// walls become four rectangles, a solid block one.
pub(crate) fn merge_cells(width: u32, height: u32, solid: impl Fn(u32, u32) -> bool) -> Vec<[u32; 4]> {
    let (w, h) = (width as usize, height as usize);
    let mut taken = vec![false; w * h];
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if taken[y * w + x] || !solid(x as u32, y as u32) {
                continue;
            }
            let free = |cx: usize, cy: usize| !taken[cy * w + cx] && solid(cx as u32, cy as u32);
            let mut rw = 1;
            while x + rw < w && free(x + rw, y) {
                rw += 1;
            }
            let mut rh = 1;
            while y + rh < h && (x..x + rw).all(|cx| free(cx, y + rh)) {
                rh += 1;
            }
            for cy in y..y + rh {
                taken[cy * w + x..cy * w + x + rw].fill(true);
            }
            out.push([x as u32, y as u32, rw as u32, rh as u32]);
        }
    }
    out
}

impl TileMap {
    /// Rectangles covering every tile `solid` picks (by tile value), merged
    /// into as few as practical, relative to the map's top-left corner in
    /// world units. For your own collision, or for physics with
    /// [`colliders`](Self::colliders).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(map: &ponko::tilemap::TileMap) {
    /// const WALL: u16 = 3;
    /// for r in map.solid_rects(|tile| tile == WALL) {
    ///     println!("wall from {} to {}", r.min, r.max);
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn solid_rects(&self, solid: impl Fn(u16) -> bool) -> Vec<Rect> {
        let w = self.size.x;
        let ts = self.tile_size;
        merge_cells(w, self.size.y, |x, y| {
            let t = self.tiles[(y * w + x) as usize];
            t != TileMap::EMPTY && solid(t)
        })
        .into_iter()
        .map(|[x, y, cw, ch]| Rect {
            min: Vec2::new(x as f32, y as f32) * ts,
            max: Vec2::new((x + cw) as f32, (y + ch) as f32) * ts,
        })
        .collect()
    }

    /// Fixed physics bodies for the tiles `solid` picks, merged into few
    /// rectangles ([`solid_rects`](Self::solid_rects)), for a map whose
    /// entity is at `position`. Spawn them; despawn them to take the
    /// collision away. Feature `physics`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, map: &ponko::tilemap::TileMap) {
    /// const WALL: u16 = 3;
    /// ctx.world.spawn_batch(map.colliders(Vec2::ZERO, |tile| tile == WALL));
    /// # }
    /// ```
    #[cfg(feature = "physics")]
    #[must_use]
    pub fn colliders(
        &self,
        position: Vec2,
        solid: impl Fn(u16) -> bool,
    ) -> Vec<(
        crate::d2::Transform,
        crate::physics::RigidBody,
        crate::physics::Collider,
    )> {
        self.solid_rects(solid)
            .into_iter()
            .map(|r| rect_body(position, r))
            .collect()
    }
}

/// A fixed body covering `r` (relative to `position`).
#[cfg(feature = "physics")]
pub(crate) fn rect_body(
    position: Vec2,
    r: Rect,
) -> (
    crate::d2::Transform,
    crate::physics::RigidBody,
    crate::physics::Collider,
) {
    (
        crate::d2::Transform::at(position + (r.min + r.max) * 0.5),
        crate::physics::RigidBody::fixed(),
        crate::physics::Collider::rect(r.max - r.min),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str]) -> (u32, u32, impl Fn(u32, u32) -> bool) {
        let cells: Vec<Vec<bool>> = rows.iter().map(|r| r.bytes().map(|b| b == b'#').collect()).collect();
        (rows[0].len() as u32, rows.len() as u32, move |x: u32, y: u32| {
            cells[y as usize][x as usize]
        })
    }

    #[test]
    fn a_room_becomes_four_rectangles_and_covers_every_solid_cell_once() {
        let (w, h, solid) = grid(&["########", "#......#", "#......#", "########"]);
        let rects = merge_cells(w, h, &solid);
        assert_eq!(rects.len(), 4, "{rects:?}");
        let mut covered = vec![0; (w * h) as usize];
        for [x, y, rw, rh] in rects {
            for cy in y..y + rh {
                for cx in x..x + rw {
                    covered[(cy * w + cx) as usize] += 1;
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                assert_eq!(covered[(y * w + x) as usize], u32::from(solid(x, y)), "cell {x},{y}");
            }
        }
    }

    #[test]
    fn a_solid_block_is_one_rectangle_and_nothing_solid_is_none() {
        let (w, h, solid) = grid(&["###", "###"]);
        assert_eq!(merge_cells(w, h, solid), vec![[0, 0, 3, 2]]);
        let (w, h, solid) = grid(&["...", "..."]);
        assert!(merge_cells(w, h, solid).is_empty());
    }
}
