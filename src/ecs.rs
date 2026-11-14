//! Built-in components and systems. They are ordinary ECS data and code: you
//! can query them, remove the systems, or replace them with your own.

use glam::Vec2;

use crate::camera::Rect;
use crate::color::Color;
use crate::ctx::Ctx;
use crate::texture::Texture;
use ponko_ecs::Component;

/// Where an entity is: position, rotation and scale.
///
/// Positions are in window pixels, `(0, 0)` top-left, y down (same as
/// [`Ctx::draw`]).
///
/// ```
/// # use ponko::prelude::*;
/// let t = ponko::d2::Transform::at(vec2(100.0, 50.0)).rotated(0.5);
/// assert_eq!(t.position, vec2(100.0, 50.0));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[serde(default)]
#[repr(C)] // shared with C/C++ (`PonkoTransform2D`)
pub struct Transform2D {
    /// Where the entity is, in world pixels.
    pub position: Vec2,
    /// Radians, clockwise on screen.
    pub rotation: f32,
    /// Size multiplier; negative flips. Default `(1, 1)`.
    pub scale: Vec2,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            rotation: 0.0,
            scale: Vec2::ONE,
        }
    }
}

impl Transform2D {
    /// A transform at `position` with no rotation and scale 1.
    #[must_use]
    pub fn at(position: Vec2) -> Self {
        Self {
            position,
            ..Default::default()
        }
    }

    /// Same transform, rotated to `rotation` radians.
    #[must_use]
    pub fn rotated(self, rotation: f32) -> Self {
        Self { rotation, ..self }
    }

    /// Same transform, scaled by `scale`.
    #[must_use]
    pub fn scaled(self, scale: Vec2) -> Self {
        Self { scale, ..self }
    }
}

/// Makes an entity visible: drawn at its [`d2::Transform`](crate::d2::Transform) by the built-in
/// [`draw_sprites`] system.
///
/// ```
/// # use ponko::prelude::*;
/// # fn setup(ctx: &mut Ctx) {
/// let crab = ctx.gfx.load_texture("crab.png");
/// ctx.world.spawn((
///     ponko::d2::Transform::at(vec2(400.0, 300.0)),
///     ponko::d2::Sprite::new(crab).with_color(Color::RED).with_z(1.0),
/// ));
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Component)]
pub struct Sprite {
    /// The image to draw.
    pub texture: Texture,
    /// Tint multiplied with the texture. Default white (no tint).
    pub color: Color,
    /// Draw order: higher is on top. Default `0`.
    pub z: f32,
    /// Point of the sprite placed at the position: `(0.5, 0.5)` = center (default).
    pub pivot: Vec2,
    /// The part of the texture to show, in texture pixels (a frame of a
    /// [`SpriteSheet`](crate::sheet::SpriteSheet)). `None` (default) shows all of it.
    pub region: Option<Rect>,
}

impl Sprite {
    /// A sprite showing `texture`, untinted, centered on the position.
    #[must_use]
    pub fn new(texture: Texture) -> Self {
        Self {
            texture,
            color: Color::WHITE,
            z: 0.0,
            pivot: Vec2::splat(0.5),
            region: None,
        }
    }

    /// Same sprite showing only `region` of its texture (texture pixels).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(tiles: Texture) {
    /// let grass = ponko::d2::Sprite::new(tiles).with_region(Rect::new(16.0, 0.0, 16.0, 16.0));
    /// # }
    /// ```
    #[must_use]
    pub fn with_region(self, region: Rect) -> Self {
        Self {
            region: Some(region),
            ..self
        }
    }

    /// Same sprite with tint `color`.
    #[must_use]
    pub fn with_color(self, color: Color) -> Self {
        Self { color, ..self }
    }

    /// Same sprite with draw order `z` (higher is on top).
    #[must_use]
    pub fn with_z(self, z: f32) -> Self {
        Self { z, ..self }
    }

    /// Same sprite with a different pivot (`(0, 0)` = top-left corner).
    #[must_use]
    pub fn with_pivot(self, pivot: Vec2) -> Self {
        Self { pivot, ..self }
    }
}

/// Built-in system: draws every entity that has a [`d2::Transform`](crate::d2::Transform) and a
/// [`Sprite`]. [`Game`](crate::game::Game) runs it every frame by default.
///
/// Replace it with your own renderer like this:
///
/// ```no_run
/// # use ponko::prelude::*;
/// fn my_draw(ctx: &mut Ctx) { /* ... */ }
///
/// Game::new("Custom", 800, 600)
///     .remove(Systems::DRAW_SPRITES)
///     .system(my_draw)
///     .run(|_| {});
/// ```
pub fn draw_sprites(ctx: &mut Ctx) {
    ctx.draw_world_sprites();
}
