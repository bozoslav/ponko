//! Sprite sheets (many frames in one texture) and frame animation.

use glam::Vec2;

use crate::camera::Rect;
use crate::ctx::Ctx;
use crate::ecs::Sprite;
use crate::texture::Texture;
use ponko_ecs::Component;

/// A texture cut into equal frames on a grid, numbered left to right, top
/// to bottom: an animation strip, a tileset, a texture atlas of icons.
///
/// ```
/// # use ponko::prelude::*;
/// # fn setup(ctx: &mut Ctx) {
/// let tex = ctx.gfx.load_texture("crab.png"); // say it holds 16 × 16 frames
/// let sheet = SpriteSheet::new(tex, 16, 16);
/// ctx.world.spawn((ponko::d2::Transform::at(vec2(100.0, 100.0)), sheet.sprite(3)));
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct SpriteSheet {
    /// The image holding every frame.
    pub texture: Texture,
    /// Frame size in texture pixels.
    pub frame_size: Vec2,
    /// Frames per row.
    pub columns: u32,
    /// Number of frames.
    pub len: u32,
}

impl SpriteSheet {
    /// Cuts `texture` into `frame_width` × `frame_height` frames. Partial
    /// frames at the right and bottom edges are left out.
    ///
    /// # Panics
    /// If a frame is empty or bigger than the texture.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(tiles: Texture) {
    /// let sheet = SpriteSheet::new(tiles, 32, 32);
    /// println!("{} tiles", sheet.len);
    /// # }
    /// ```
    #[must_use]
    pub fn new(texture: Texture, frame_width: u32, frame_height: u32) -> Self {
        assert!(
            frame_width > 0 && frame_height > 0 && frame_width <= texture.width() && frame_height <= texture.height(),
            "ponko: {frame_width} x {frame_height} frames don't fit a {} x {} texture",
            texture.width(),
            texture.height()
        );
        let columns = texture.width() / frame_width;
        let rows = texture.height() / frame_height;
        Self {
            texture,
            frame_size: Vec2::new(frame_width as f32, frame_height as f32),
            columns,
            len: columns * rows,
        }
    }

    /// Where frame `index` is, in texture pixels. Wraps around past the end.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, sheet: SpriteSheet) {
    /// let params = DrawParams::default().with_region(sheet.frame(2));
    /// ctx.gfx.draw_ex(sheet.texture, vec2(50.0, 50.0), params);
    /// # }
    /// ```
    #[must_use]
    pub fn frame(&self, index: u32) -> Rect {
        let i = index % self.len.max(1);
        let at = Vec2::new((i % self.columns) as f32, (i / self.columns) as f32) * self.frame_size;
        Rect {
            min: at,
            max: at + self.frame_size,
        }
    }

    /// A [`Sprite`] showing frame `index`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, sheet: SpriteSheet) {
    /// ctx.world.spawn((ponko::d2::Transform::at(vec2(10.0, 10.0)), sheet.sprite(0)));
    /// # }
    /// ```
    #[must_use]
    pub fn sprite(&self, index: u32) -> Sprite {
        Sprite::new(self.texture).with_region(self.frame(index))
    }
}

/// Plays frames of a [`SpriteSheet`] on an entity's [`Sprite`]. The built-in
/// [`animate_sprites`](crate::systems::animate_sprites) system advances it every frame.
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::sheet::Animation;
/// # fn setup(ctx: &mut Ctx, sheet: SpriteSheet) {
/// ctx.world.spawn((
///     ponko::d2::Transform::at(vec2(200.0, 200.0)),
///     sheet.sprite(0),
///     Animation::new(sheet, 0..8, 12.0), // frames 0-7 at 12 frames per second, looping
/// ));
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Component)]
#[repr(C)]
pub struct Animation {
    /// The frames come from this sheet.
    pub sheet: SpriteSheet,
    /// First frame and how many frames to play.
    pub first: u32,
    /// How many frames to play, from `first`.
    pub frames: u32,
    /// Frames per second.
    pub fps: f32,
    /// Start over at the end (default), or stop on the last frame.
    pub looping: bool,
    /// Seconds played so far.
    pub time: f32,
}

impl Animation {
    /// Plays `frames` of `sheet` at `fps`, looping.
    #[must_use]
    pub fn new(sheet: SpriteSheet, frames: std::ops::Range<u32>, fps: f32) -> Self {
        Self {
            sheet,
            first: frames.start,
            frames: frames.end.saturating_sub(frames.start).max(1),
            fps,
            looping: true,
            time: 0.0,
        }
    }

    /// Same animation, played once and then held on its last frame.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use ponko::sheet::Animation;
    /// # fn f(sheet: SpriteSheet) {
    /// let explode = Animation::new(sheet, 8..16, 24.0).once();
    /// # }
    /// ```
    #[must_use]
    pub fn once(self) -> Self {
        Self { looping: false, ..self }
    }

    /// The frame showing now (an index into the sheet).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use ponko::sheet::Animation;
    /// # fn f(sheet: SpriteSheet) {
    /// let walk = Animation::new(sheet, 4..8, 10.0);
    /// assert_eq!(walk.current_frame(), 4);
    /// # }
    /// ```
    #[must_use]
    pub fn current_frame(&self) -> u32 {
        let n = (self.time * self.fps).max(0.0) as u32;
        let step = if self.looping {
            n % self.frames
        } else {
            n.min(self.frames - 1)
        };
        self.first + step
    }

    /// `true` once an animation made with [`once`](Self::once) reached its
    /// last frame (never for looping ones).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use ponko::sheet::Animation;
    /// # fn cleanup(ctx: &mut Ctx) {
    /// let done: Vec<Entity> = ctx.world.query::<(Entity, &Animation)>().iter()
    ///     .filter(|(_, a)| a.finished())
    ///     .map(|(e, _)| e)
    ///     .collect();
    /// for e in done { ctx.world.despawn(e); }
    /// # }
    /// ```
    #[must_use]
    pub fn finished(&self) -> bool {
        !self.looping && self.time * self.fps >= self.frames as f32
    }
}

/// The system behind [`systems::animate_sprites`](crate::systems::animate_sprites); it keeps
/// this path for its name (`ponko::sheet::animate_sprites`).
pub(crate) fn animate_sprites(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    ctx.world
        .query::<(&mut Animation, &mut Sprite)>()
        .par_for_each(|(a, s)| {
            a.time += dt;
            s.texture = a.sheet.texture;
            s.region = Some(a.sheet.frame(a.current_frame()));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> SpriteSheet {
        // 64 × 40 texture, 16 × 16 frames: 4 columns, 2 full rows.
        SpriteSheet::new(Texture::new(0, 64, 40), 16, 16)
    }

    #[test]
    fn frames_are_numbered_row_by_row() {
        let s = sheet();
        assert_eq!((s.columns, s.len), (4, 8));
        assert_eq!(s.frame(0), Rect::new(0.0, 0.0, 16.0, 16.0));
        assert_eq!(s.frame(5), Rect::new(16.0, 16.0, 16.0, 16.0));
        assert_eq!(s.frame(8), s.frame(0), "wraps");
    }

    #[test]
    fn animations_loop_or_hold() {
        let mut a = Animation::new(sheet(), 2..5, 10.0);
        a.time = 0.25; // 2.5 frames in
        assert_eq!(a.current_frame(), 4);
        a.time = 0.35;
        assert_eq!(a.current_frame(), 2, "looped back");
        assert!(!a.finished());
        let mut once = Animation::new(sheet(), 2..5, 10.0).once();
        once.time = 10.0;
        assert_eq!(once.current_frame(), 4);
        assert!(once.finished());
    }

    #[test]
    #[should_panic(expected = "don't fit")]
    fn frames_bigger_than_the_texture_panic() {
        let _ = SpriteSheet::new(Texture::new(0, 8, 8), 16, 16);
    }
}
