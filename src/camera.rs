//! [`Rect`], and [`camera_controls`] for the 2D camera ([`d2::Camera`](crate::d2::Camera)).

use glam::Vec2;

/// An axis-aligned rectangle: `min` is the top-left corner, `max` the
/// bottom-right (y grows downward, like window pixels).
///
/// ```
/// # use ponko::prelude::*;
/// let r = Rect::new(10.0, 20.0, 100.0, 50.0); // x, y, width, height
/// assert_eq!(r.size(), vec2(100.0, 50.0));
/// assert!(r.contains(vec2(50.0, 40.0)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[repr(C)] // shared with C/C++ (`PonkoRect`)
pub struct Rect {
    /// Top-left corner (smallest x and y).
    pub min: Vec2,
    /// Bottom-right corner (largest x and y).
    pub max: Vec2,
}

impl Rect {
    /// A rectangle from its top-left corner and its size.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let button = Rect::new(20.0, 20.0, 120.0, 32.0);
    /// # let _ = button;
    /// ```
    #[must_use]
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            min: Vec2::new(x, y),
            max: Vec2::new(x + width, y + height),
        }
    }

    /// Width and height.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(Rect::new(0.0, 0.0, 4.0, 3.0).size(), vec2(4.0, 3.0));
    /// ```
    #[must_use]
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// The middle point.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(Rect::new(0.0, 0.0, 4.0, 2.0).center(), vec2(2.0, 1.0));
    /// ```
    #[must_use]
    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// `true` if `point` is inside (edges included).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &Ctx, button: Rect) {
    /// let hovered = button.contains(ctx.input.mouse_pos());
    /// # }
    /// ```
    #[must_use]
    pub fn contains(&self, point: Vec2) -> bool {
        point.cmpge(self.min).all() && point.cmple(self.max).all()
    }

    /// The same rectangle grown by `by` on every side (shrunk if negative).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let inner = Rect::new(0.0, 0.0, 100.0, 100.0).expand(-8.0);
    /// assert_eq!(inner.min, vec2(8.0, 8.0));
    /// ```
    #[must_use]
    pub fn expand(&self, by: f32) -> Self {
        Self {
            min: self.min - by,
            max: self.max + by,
        }
    }
}

/// A ready-made system for looking around big worlds: drag with the right
/// or middle mouse button to pan, scroll to zoom toward the mouse.
///
/// ```no_run
/// use ponko::prelude::*;
///
/// Game::new("Colony", 1280, 720)
///     .system(ponko::camera::camera_controls)
///     .run(|_| {});
/// ```
pub fn camera_controls(ctx: &mut crate::ctx::Ctx) {
    use crate::input::MouseButton;
    if ctx.input.mouse_down(MouseButton::Right) || ctx.input.mouse_down(MouseButton::Middle) {
        let delta = ctx.input.mouse_delta();
        ctx.camera.position -= delta / ctx.camera.zoom;
    }
    let wheel = ctx.input.mouse_wheel().y;
    if wheel != 0.0 && !ctx.ui.wants_pointer() {
        let mouse = ctx.input.mouse_pos();
        ctx.camera.zoom_at(mouse, 1.15f32.powf(wheel));
    }
}
