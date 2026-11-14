//! Immediate widgets: a panel, a label and a button drawn the frame they are
//! called, for debug overlays and quick tools. Game UI that stays on screen
//! belongs in the retained tree ([`Ui`](super::Ui)).

use glam::Vec2;

use super::UiStyle;
use crate::camera::Rect;
use crate::gfx::Graphics;
use crate::input::{Input, MouseButton};
use crate::text::{Font, TextParams};

/// The state and look of the immediate widgets: `ctx.ui.immediate`. Draw
/// them through [`ctx.immediate_ui()`](crate::ctx::Ctx::immediate_ui).
///
/// Each call draws one widget this frame, on top of the world, in
/// [`style`](Self::style). Positions and sizes are in UI units, the units of
/// the retained tree's layout: window pixels at the default scale, and
/// multiplied by the screen's [`Ui::set_scale`](super::Ui::set_scale) like
/// the retained tree is, so a debug overlay is as large as the menu next to
/// it. The look follows the screen's [`UiTheme`](super::UiTheme)
/// ([`Ui::set_theme`](super::Ui::set_theme)). Nothing is kept between frames but the
/// rectangles drawn, so [`Ui::wants_pointer`](super::Ui::wants_pointer)
/// knows the mouse is over them.
///
/// ```
/// # use ponko::prelude::*;
/// # fn tools(ctx: &mut Ctx) {
/// let panel = Rect::new(10.0, 10.0, 220.0, 120.0);
/// ctx.immediate_ui().panel(panel);
/// let mut at = panel.min + 10.0;
/// at.y += ctx.immediate_ui().label(at, "Simulation").y + 8.0;
/// if ctx.immediate_ui().button(Rect::new(at.x, at.y, 100.0, 32.0), "Pause") {
///     ctx.time.pause();
/// }
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct UiImmediate {
    /// Colors and sizes of the widgets.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// ctx.ui.immediate.style.text_size = 20.0;
    /// # }
    /// ```
    pub style: UiStyle,
    /// The font of labels and buttons. The screen's theme font by default
    /// ([`Ui::set_theme`](super::Ui::set_theme)); setting it here keeps it
    /// through a later theme change.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let font = ctx.gfx.load_font("mono.ttf");
    /// ctx.ui.immediate.font = font;
    /// # }
    /// ```
    pub font: Font,
    /// Thickness of a button's border in UI units, drawn in the accent
    /// color. The screen's theme [`border_width`](super::UiTheme::border_width)
    /// when it sets one; 0 (no border, only the outline of a hovered
    /// button) otherwise.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// ctx.ui.immediate.border_width = 2.0;
    /// # }
    /// ```
    pub border_width: f32,
    /// The screen's UI scale ([`Ui::set_scale`](super::Ui::set_scale)).
    pub(super) scale: f32,
    /// Every rectangle drawn this frame and the previous one, in window
    /// pixels, so `covers` can answer before this frame's widgets are drawn.
    this_frame: Vec<Rect>,
    last_frame: Vec<Rect>,
    /// Where the left mouse button last went down, for button clicks.
    press_at: Vec2,
}

impl Default for UiImmediate {
    fn default() -> Self {
        Self {
            style: UiStyle::default(),
            font: Font::DEFAULT,
            border_width: 0.0,
            scale: 1.0,
            this_frame: Vec::new(),
            last_frame: Vec::new(),
            press_at: Vec2::splat(f32::NAN),
        }
    }
}

impl UiImmediate {
    /// The scale the widgets are drawn at: the screen's
    /// [`Ui::set_scale`](super::Ui::set_scale), 1 by default. Multiply a
    /// size in UI units by it for window pixels.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// assert_eq!(ui.immediate.scale(), 1.0);
    /// ui.set_scale(2.0);
    /// assert_eq!(ui.immediate.scale(), 2.0);
    /// ```
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// `rect` (UI units) in window pixels.
    fn px(&self, rect: Rect) -> Rect {
        Rect {
            min: rect.min * self.scale,
            max: rect.max * self.scale,
        }
    }

    /// Engine side: a new frame; last frame's rectangles still count.
    pub(crate) fn begin_frame(&mut self) {
        std::mem::swap(&mut self.this_frame, &mut self.last_frame);
        self.this_frame.clear();
    }

    /// Whether `point` (window pixels, such as the mouse) is on a widget
    /// drawn this frame or the previous one.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 320, 240).run_headless(0, |ctx| {
    ///     let panel = Rect::new(10.0, 10.0, 100.0, 50.0);
    ///     assert!(!ctx.ui.immediate.covers(vec2(20.0, 20.0)));
    ///     ctx.immediate_ui().panel(panel);
    ///     assert!(ctx.ui.immediate.covers(vec2(20.0, 20.0)));
    /// });
    /// ```
    #[must_use]
    pub fn covers(&self, point: Vec2) -> bool {
        self.this_frame
            .iter()
            .chain(&self.last_frame)
            .any(|r| r.contains(point))
    }

    /// The implementation behind [`UiImmediateFrame::panel`].
    pub(crate) fn panel_on(&mut self, gfx: &mut Graphics, rect: Rect) {
        let rect = self.px(rect);
        gfx.ui_fill(rect, self.style.panel, -1.0);
        self.this_frame.push(rect);
    }

    /// The implementation behind [`UiImmediateFrame::label`].
    pub(crate) fn label_on(&self, gfx: &mut Graphics, pos: Vec2, text: &str) -> Vec2 {
        let (style, scale) = (self.style, self.scale);
        let size = style.text_size * scale;
        gfx.draw_text_ex(
            text,
            pos * scale,
            TextParams {
                font: self.font,
                size,
                color: style.text,
                z: 1.0,
            },
        );
        gfx.measure_text_ex(text, self.font, size) / scale
    }

    /// The implementation behind [`UiImmediateFrame::button`].
    pub(crate) fn button_on(&mut self, gfx: &mut Graphics, input: &Input, rect: Rect, label: &str) -> bool {
        let (style, scale) = (self.style, self.scale);
        let rect = self.px(rect);
        let mouse = input.mouse_pos();
        if input.mouse_pressed(MouseButton::Left) {
            self.press_at = mouse;
        }
        let hovered = rect.contains(mouse);
        let held = hovered && input.mouse_down(MouseButton::Left);
        let clicked = hovered && input.mouse_released(MouseButton::Left) && rect.contains(self.press_at);
        let fill = if held {
            style.button_pressed
        } else if hovered {
            style.button_hovered
        } else {
            style.button
        };
        let border = self.border_width.max(0.0) * scale;
        if hovered {
            gfx.ui_fill(rect.expand(scale), style.accent, -0.5);
        }
        if border > 0.0 {
            gfx.ui_fill(rect, style.accent, -0.25);
        }
        gfx.ui_fill(rect.expand(-border), fill, 0.0);
        let text_size = style.text_size * scale;
        let size = gfx.measure_text_ex(label, self.font, text_size);
        let at = (rect.center() - size / 2.0).round();
        gfx.draw_text_ex(
            label,
            at,
            TextParams {
                font: self.font,
                size: text_size,
                color: style.text,
                z: 1.0,
            },
        );
        self.this_frame.push(rect);
        clicked
    }

    /// Deprecated: use [`ctx.immediate_ui()`](crate::ctx::Ctx::immediate_ui)`.panel(rect)`,
    /// which takes the graphics service itself.
    #[deprecated(since = "0.2.0", note = "use `ctx.immediate_ui().panel(rect)`")]
    pub fn panel(&mut self, gfx: &mut Graphics, rect: Rect) {
        self.panel_on(gfx, rect);
    }

    /// Deprecated: use [`ctx.immediate_ui()`](crate::ctx::Ctx::immediate_ui)`.label(pos, text)`.
    #[deprecated(since = "0.2.0", note = "use `ctx.immediate_ui().label(pos, text)`")]
    pub fn label(&self, gfx: &mut Graphics, pos: Vec2, text: &str) -> Vec2 {
        self.label_on(gfx, pos, text)
    }

    /// Deprecated: use [`ctx.immediate_ui()`](crate::ctx::Ctx::immediate_ui)`.button(rect, label)`,
    /// which takes the graphics and input services itself.
    #[deprecated(since = "0.2.0", note = "use `ctx.immediate_ui().button(rect, label)`")]
    pub fn button(&mut self, gfx: &mut Graphics, input: &Input, rect: Rect, label: &str) -> bool {
        self.button_on(gfx, input, rect, label)
    }
}

/// The immediate widgets for one call site: from
/// [`ctx.immediate_ui()`](crate::ctx::Ctx::immediate_ui). It borrows the
/// widgets' state ([`ctx.ui.immediate`](UiImmediate)), the graphics service
/// to draw and the input service to read the mouse, so its calls take only
/// what to draw. Drop it (end the statement or the block) before using those
/// services again.
///
/// ```
/// # use ponko::prelude::*;
/// # fn tools(ctx: &mut Ctx) {
/// let panel = Rect::new(10.0, 10.0, 220.0, 120.0);
/// let mut ui = ctx.immediate_ui();
/// ui.panel(panel);
/// let mut at = panel.min + 10.0;
/// at.y += ui.label(at, "Simulation").y + 8.0;
/// if ui.button(Rect::new(at.x, at.y, 100.0, 32.0), "Pause") {
///     ctx.time.pause();
/// }
/// # }
/// ```
pub struct UiImmediateFrame<'a> {
    widgets: &'a mut UiImmediate,
    gfx: &'a mut Graphics,
    input: &'a Input,
}

impl std::fmt::Debug for UiImmediateFrame<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiImmediateFrame")
            .field("widgets", &self.widgets)
            .finish_non_exhaustive()
    }
}

impl UiImmediateFrame<'_> {
    /// A background panel at `rect` (UI units), in the style's panel color.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn sidebar(ctx: &mut Ctx) {
    /// let panel = Rect::new(10.0, 10.0, 220.0, 300.0);
    /// ctx.immediate_ui().panel(panel);
    /// ctx.immediate_ui().label(panel.min + 10.0, "Colony");
    /// # }
    /// ```
    pub fn panel(&mut self, rect: Rect) {
        self.widgets.panel_on(self.gfx, rect);
    }

    /// Text at `pos` (UI units) in the style's text size and color and the
    /// immediate font. Returns the space it took, in UI units.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn stats(ctx: &mut Ctx, ants: usize) {
    /// let mut at = vec2(20.0, 20.0);
    /// at.y += ctx.immediate_ui().label(at, "Ants").y;
    /// ctx.immediate_ui().label(at, &ants.to_string());
    /// # }
    /// ```
    pub fn label(&mut self, pos: Vec2, text: &str) -> Vec2 {
        self.widgets.label_on(self.gfx, pos, text)
    }

    /// A button at `rect` (UI units) with `label` centered on it.
    /// Returns `true` on the frame it's clicked (mouse released over it
    /// after being pressed over it), reading the mouse (window pixels) from
    /// the input service.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn controls(ctx: &mut Ctx) {
    /// if ctx.immediate_ui().button(Rect::new(20.0, 20.0, 100.0, 32.0), "Pause") {
    ///     ctx.time.pause();
    /// }
    /// # }
    /// ```
    pub fn button(&mut self, rect: Rect, label: &str) -> bool {
        self.widgets.button_on(self.gfx, self.input, rect, label)
    }

    /// Whether `point` (window pixels) is on a widget drawn this frame or
    /// the previous one ([`UiImmediate::covers`]).
    #[must_use]
    pub fn covers(&self, point: Vec2) -> bool {
        self.widgets.covers(point)
    }
}

impl crate::ctx::Ctx {
    /// The immediate widgets (panel, label, button) for debug overlays and
    /// quick tools, drawn the frame they are called. Their look is
    /// [`ctx.ui.immediate`](crate::ui::UiImmediate) (style, font, border);
    /// game UI that stays on screen belongs in the retained tree
    /// ([`ctx.ui`](crate::ui::Ui)).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn controls(ctx: &mut Ctx) {
    /// if ctx.immediate_ui().button(Rect::new(20.0, 20.0, 100.0, 32.0), "Pause") {
    ///     ctx.time.pause();
    /// }
    /// # }
    /// ```
    pub fn immediate_ui(&mut self) -> UiImmediateFrame<'_> {
        UiImmediateFrame {
            widgets: &mut self.ui.immediate,
            gfx: &mut self.gfx,
            input: &self.input,
        }
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::input::Input;
    use crate::prelude::*;
    #[allow(unused_imports)]
    use crate::ui::{Ui, UiStyle};

    #[test]
    fn the_pointer_follows_the_mouse_between_systems() {
        fn first(ctx: &mut Ctx) {
            ctx.immediate_ui().panel(Rect::new(100.0, 100.0, 50.0, 50.0));
            assert!(!ctx.ui.wants_pointer(), "the mouse is at (0, 0)");
            ctx.input.set_mouse_pos(vec2(120.0, 120.0));
            assert!(
                !ctx.ui.wants_pointer(),
                "the pointer is the mouse as the system found it"
            );
        }
        fn second(ctx: &mut Ctx) {
            assert!(ctx.ui.wants_pointer(), "the next system sees where the mouse went");
        }
        Game::new("t", 800, 600)
            .sim_system(first)
            .sim_system(second)
            .run_headless(1, |_| {});
    }

    use crate::ui::UiTheme;

    /// A frame of the mouse at `at` (window pixels), then a click on it.
    fn click(ctx: &mut Ctx, at: Vec2, rect: Rect) -> bool {
        ctx.input.set_mouse_pos(at);
        ctx.input.set_mouse_button(MouseButton::Left, true);
        ctx.immediate_ui().button(rect, "Go");
        ctx.input.end_frame();
        ctx.input.set_mouse_button(MouseButton::Left, false);
        let clicked = ctx.immediate_ui().button(rect, "Go");
        ctx.input.end_frame();
        clicked
    }

    #[test]
    fn the_screens_scale_sizes_the_widgets_like_the_retained_tree() {
        let mut ctx = Ctx::new(1.0 / 60.0, vec2(800.0, 600.0));
        ctx.set_headless(true);
        let one = ctx.immediate_ui().label(vec2(0.0, 0.0), "Simulation");
        ctx.ui.set_scale(2.0);
        assert_eq!(ctx.ui.immediate.scale(), 2.0);
        let two = ctx.immediate_ui().label(vec2(0.0, 0.0), "Simulation");
        // The size is in UI units, so it doesn't depend on the scale.
        assert!((one - two).abs().max_element() < 1.0, "{one} at 1x, {two} at 2x");
        // The panel at (10, 10) in UI units is at (20, 20) in window pixels.
        ctx.immediate_ui().panel(Rect::new(10.0, 10.0, 100.0, 50.0));
        assert!(!ctx.ui.immediate.covers(vec2(15.0, 15.0)));
        assert!(ctx.ui.immediate.covers(vec2(25.0, 25.0)));
        assert!(ctx.ui.immediate.covers(vec2(219.0, 119.0)));
        assert!(!ctx.ui.immediate.covers(vec2(221.0, 121.0)));
    }

    #[test]
    fn the_default_scale_leaves_the_widgets_where_they_were() {
        let mut ctx = Ctx::new(1.0 / 60.0, vec2(800.0, 600.0));
        ctx.set_headless(true);
        assert_eq!(ctx.ui.immediate.scale(), 1.0);
        ctx.immediate_ui().panel(Rect::new(10.0, 10.0, 100.0, 50.0));
        assert!(ctx.ui.immediate.covers(vec2(10.5, 10.5)));
        assert!(!ctx.ui.immediate.covers(vec2(9.5, 9.5)));
        assert!(ctx.ui.immediate.covers(vec2(109.5, 59.5)));
        assert!(!ctx.ui.immediate.covers(vec2(110.5, 60.5)));
        assert_eq!(ctx.ui.immediate.font, Font::DEFAULT);
        assert_eq!(ctx.ui.immediate.border_width, 0.0);
    }

    #[test]
    fn a_button_is_clicked_where_it_is_drawn_at_any_scale() {
        for scale in [1.0, 2.0] {
            let mut ctx = Ctx::new(1.0 / 60.0, vec2(800.0, 600.0));
            ctx.set_headless(true);
            ctx.ui.set_scale(scale);
            let rect = Rect::new(20.0, 20.0, 100.0, 32.0);
            // Its middle, in window pixels.
            let middle = rect.center() * scale;
            assert!(click(&mut ctx, middle, rect), "the middle at {scale}x");
            // The place the button would be at 1x, when it isn't there.
            if scale > 1.0 {
                let mut ctx = Ctx::new(1.0 / 60.0, vec2(800.0, 600.0));
                ctx.set_headless(true);
                ctx.ui.set_scale(scale);
                let outside = vec2(30.0, 30.0);
                assert!(!click(&mut ctx, outside, rect), "unscaled place at {scale}x");
            }
        }
    }

    #[test]
    fn the_theme_font_and_border_reach_the_immediate_widgets() {
        let mut ctx = Ctx::new(1.0 / 60.0, vec2(800.0, 600.0));
        ctx.set_headless(true);
        let mono = ctx
            .gfx
            .text
            .add_font(epaint_default_fonts::HACK_REGULAR)
            .expect("a font");
        let plain = ctx.immediate_ui().label(vec2(0.0, 0.0), "Simulation");
        ctx.ui.set_theme(UiTheme {
            font: mono,
            border_width: 2.0,
            ..Default::default()
        });
        assert_eq!(ctx.ui.immediate.font, mono);
        assert_eq!(ctx.ui.immediate.border_width, 2.0);
        let themed = ctx.immediate_ui().label(vec2(0.0, 0.0), "Simulation");
        assert_eq!(themed, ctx.gfx.measure_text_ex("Simulation", mono, 16.0));
        assert_ne!(plain.x, themed.x, "the monospace font is wider");
        // Back to the default theme: the font and the border follow it.
        ctx.ui.set_theme(UiTheme::default());
        assert_eq!(ctx.ui.immediate.font, Font::DEFAULT);
        assert_eq!(ctx.ui.immediate.border_width, 0.0);
    }

    #[test]
    fn a_font_set_by_hand_stays_through_a_theme_change() {
        let mut ctx = Ctx::new(1.0 / 60.0, vec2(800.0, 600.0));
        let mine = ctx
            .gfx
            .text
            .add_font(epaint_default_fonts::HACK_REGULAR)
            .expect("a font");
        let theirs = ctx
            .gfx
            .text
            .add_font(epaint_default_fonts::HACK_REGULAR)
            .expect("a font");
        ctx.ui.immediate.font = mine;
        ctx.ui.immediate.border_width = 3.0;
        ctx.ui.set_theme(UiTheme {
            font: theirs,
            border_width: 1.0,
            ..Default::default()
        });
        assert_eq!(ctx.ui.immediate.font, mine);
        assert_eq!(ctx.ui.immediate.border_width, 3.0);
        assert_eq!(ctx.ui.theme().font, theirs);
    }
}
