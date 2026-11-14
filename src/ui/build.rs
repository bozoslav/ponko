//! Builders, layout presets and a theme for the retained [`Ui`].
//!
//! Everything here is a thin layer over [`Ui::add_child`] and the setters: a
//! builder adds the node first, each method changes it right away, and
//! [`UiBuilder::id`] hands back its [`UiId`]. Nothing new is stored in the
//! tree except the theme, which gives new nodes their [`UiPaint`].

use super::retained::{Ui, UiId, UiPaint, UiTextAlign, UiWidget};
use crate::ui::UiStyle;
use crate::{color::Color, text::Font, texture::Texture};
use taffy::prelude::*;

/// A node's layout: the one layout type of the retained [`Ui`], with presets
/// for the common shapes of game UI and chained refinements.
///
/// It is backed by Taffy. For a flex or grid setting the presets don't
/// cover, [`UiLayout::custom`], [`UiLayout::style`] and
/// [`UiLayout::style_mut`] reach the Taffy style through
/// [`ui::unstable_taffy`](crate::ui::unstable_taffy), which is outside
/// ponko's stability promise: its types change with Taffy upgrades.
///
/// Lengths are logical window units. Fractions are of the parent's size (of
/// the window for a screen root). Children stack top to bottom, except
/// inside a [`UiLayout::row`], where they go left to right.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().layout(ponko::ui::UiLayout::fill().padding(10.0).gap(4.0)).id();
/// let a = ui.label(root, "A").layout(ponko::ui::UiLayout::row(20.0)).id();
/// let b = ui.label(root, "B").layout(ponko::ui::UiLayout::row(20.0)).id();
/// ui.push_screen(root, false);
/// ui.layout(vec2(200.0, 100.0));
/// assert_eq!(ui.rect(a).unwrap().min, vec2(10.0, 10.0));
/// assert_eq!(ui.rect(b).unwrap().min, vec2(10.0, 34.0));
/// assert_eq!(ui.rect(b).unwrap().size(), vec2(180.0, 20.0));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct UiLayout(Style);

impl Default for UiLayout {
    fn default() -> Self {
        Self::auto()
    }
}

fn column_style() -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        ..Default::default()
    }
}

impl UiLayout {
    /// Sized by its content (text, children); Taffy's default style.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::auto().into_style();
    /// assert_eq!(style, ponko::ui::unstable_taffy::Style::default());
    /// ```
    pub fn auto() -> Self {
        Self(Style::default())
    }
    /// Any Taffy style, for flex and grid settings the presets don't cover.
    /// Unstable: see [`crate::ui::unstable_taffy`].
    /// ```
    /// use ponko::prelude::*;
    /// let layout = ponko::ui::UiLayout::custom(ponko::ui::unstable_taffy::Style { flex_grow: 2.0, ..Default::default() });
    /// assert_eq!(layout.style().flex_grow, 2.0);
    /// ```
    pub fn custom(style: Style) -> Self {
        Self(style)
    }
    /// A horizontal strip: the parent's full width, `height` tall, never
    /// squeezed. Its children go left to right.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::row(30.0).into_style();
    /// assert_eq!(style.flex_direction, ponko::ui::unstable_taffy::FlexDirection::Row);
    /// ```
    pub fn row(height: f32) -> Self {
        Self(Style {
            flex_direction: FlexDirection::Row,
            size: Size {
                width: percent(1.0),
                height: length(height),
            },
            flex_shrink: 0.0,
            ..Default::default()
        })
    }
    /// A vertical strip: `width` wide, the parent's full height, never
    /// squeezed. Its children go top to bottom.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill().horizontal()).id();
    /// let side = ui.panel(root).layout(ponko::ui::UiLayout::column(120.0)).id();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(400.0, 300.0));
    /// assert_eq!(ui.rect(side).unwrap().size(), vec2(120.0, 300.0));
    /// ```
    pub fn column(width: f32) -> Self {
        Self(Style {
            size: Size {
                width: length(width),
                height: percent(1.0),
            },
            flex_shrink: 0.0,
            ..column_style()
        })
    }
    /// The parent's whole content box. Next to siblings in a column it
    /// shrinks to the space they leave (it can shrink to nothing).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
    /// let header = ui.label(root, "Inventory").layout(ponko::ui::UiLayout::row(40.0)).id();
    /// let list = ui.scroll_area(root).layout(ponko::ui::UiLayout::fill()).id();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(400.0, 300.0));
    /// assert_eq!(ui.rect(list).unwrap().size(), vec2(400.0, 260.0));
    /// ```
    pub fn fill() -> Self {
        Self(Style {
            size: Size {
                width: percent(1.0),
                height: percent(1.0),
            },
            min_size: Size::length(0.0),
            flex_shrink: 1.0,
            ..column_style()
        })
    }
    /// Exactly `width` x `height`, never squeezed.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::fixed(200.0, 40.0).into_style();
    /// assert_eq!(style.flex_shrink, 0.0);
    /// ```
    pub fn fixed(width: f32, height: f32) -> Self {
        Self(Style {
            size: Size {
                width: length(width),
                height: length(height),
            },
            flex_shrink: 0.0,
            ..column_style()
        })
    }
    /// Placed by fractions of the parent, out of the flow of its siblings:
    /// top-left corner at (`x`, `y`), size `width` x `height`. For a screen
    /// root these are fractions of the window, so a HUD keeps its place at
    /// any resolution.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let hud = ui.root().layout(ponko::ui::UiLayout::anchored(0.75, 0.0, 0.25, 0.1)).id();
    /// ui.push_screen(hud, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// assert_eq!(ui.rect(hud).unwrap().min, vec2(600.0, 0.0));
    /// assert_eq!(ui.rect(hud).unwrap().size(), vec2(200.0, 60.0));
    /// ```
    pub fn anchored(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self(Style {
            position: Position::Absolute,
            inset: taffy::Rect {
                left: percent(x),
                top: percent(y),
                right: auto(),
                bottom: auto(),
            },
            size: Size {
                width: percent(width),
                height: percent(height),
            },
            ..column_style()
        })
    }
    /// `width` x `height`, centered in the parent and out of the flow of its
    /// siblings: a dialog.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let dialog = ui.root().layout(ponko::ui::UiLayout::centered(200.0, 100.0)).id();
    /// ui.push_screen(dialog, true);
    /// ui.layout(vec2(800.0, 600.0));
    /// assert_eq!(ui.rect(dialog).unwrap().min, vec2(300.0, 250.0));
    /// ```
    pub fn centered(width: f32, height: f32) -> Self {
        Self(Style {
            position: Position::Absolute,
            inset: taffy::Rect {
                left: percent(0.5),
                top: percent(0.5),
                right: auto(),
                bottom: auto(),
            },
            margin: taffy::Rect {
                left: length(-width * 0.5),
                top: length(-height * 0.5),
                right: auto(),
                bottom: auto(),
            },
            size: Size {
                width: length(width),
                height: length(height),
            },
            ..column_style()
        })
    }
    /// A grid of `columns` equal columns as wide as the parent, with rows
    /// `row_height` tall and as many as the children need: an inventory.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fixed(300.0, 300.0)).id();
    /// let grid = ui.panel(root).layout(ponko::ui::UiLayout::grid(3, 50.0)).id();
    /// let cells: Vec<ponko::ui::UiId> = (0..4).map(|_| ui.button(grid, "").id()).collect();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// assert_eq!(ui.rect(cells[1]).unwrap().min, vec2(100.0, 0.0));
    /// assert_eq!(ui.rect(cells[3]).unwrap().min, vec2(0.0, 50.0));
    /// ```
    pub fn grid(columns: usize, row_height: f32) -> Self {
        Self(Style {
            display: Display::Grid,
            size: Size {
                width: percent(1.0),
                height: auto(),
            },
            grid_template_columns: vec![fr(1.0); columns],
            grid_auto_rows: vec![length(row_height)],
            flex_shrink: 0.0,
            ..Default::default()
        })
    }

    /// Space between children, in both directions.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::fill().gap(8.0).into_style();
    /// assert_eq!(style.gap, ponko::ui::unstable_taffy::Size::length(8.0));
    /// ```
    pub fn gap(mut self, gap: f32) -> Self {
        self.0.gap = Size::length(gap);
        self
    }
    /// Space inside the edges, on all four sides.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::fill().padding(12.0).into_style();
    /// assert_eq!(style.padding, ponko::ui::unstable_taffy::Rect::length(12.0));
    /// ```
    pub fn padding(mut self, padding: f32) -> Self {
        self.0.padding = taffy::Rect::length(padding);
        self
    }
    /// Take a `factor` share of the space the siblings leave free, along the
    /// parent's direction.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fixed(300.0, 100.0)).id();
    /// let top = ui.panel(root).layout(ponko::ui::UiLayout::auto().grow(1.0)).id();
    /// ui.panel(root).layout(ponko::ui::UiLayout::row(40.0));
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// assert_eq!(ui.rect(top).unwrap().size().y, 60.0);
    /// ```
    pub fn grow(mut self, factor: f32) -> Self {
        self.0.flex_grow = factor;
        self
    }
    /// Children go left to right.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::fill().horizontal().into_style();
    /// assert_eq!(style.flex_direction, ponko::ui::unstable_taffy::FlexDirection::Row);
    /// ```
    pub fn horizontal(mut self) -> Self {
        self.0.flex_direction = FlexDirection::Row;
        self
    }
    /// Children go top to bottom.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::row(30.0).vertical().into_style();
    /// assert_eq!(style.flex_direction, ponko::ui::unstable_taffy::FlexDirection::Column);
    /// ```
    pub fn vertical(mut self) -> Self {
        self.0.flex_direction = FlexDirection::Column;
        self
    }
    /// Width in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::row(30.0).width(200.0).into_style();
    /// assert_eq!(style.size.width, ponko::ui::unstable_taffy::Dimension::length(200.0));
    /// ```
    pub fn width(mut self, width: f32) -> Self {
        self.0.size.width = length(width);
        self
    }
    /// Height in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::column(30.0).height(200.0).into_style();
    /// assert_eq!(style.size.height, ponko::ui::unstable_taffy::Dimension::length(200.0));
    /// ```
    pub fn height(mut self, height: f32) -> Self {
        self.0.size.height = length(height);
        self
    }
    /// Width as a fraction of the parent's.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::grid(3, 50.0).width_fraction(0.95).into_style();
    /// assert_eq!(style.size.width, ponko::ui::unstable_taffy::Dimension::percent(0.95));
    /// ```
    pub fn width_fraction(mut self, fraction: f32) -> Self {
        self.0.size.width = percent(fraction);
        self
    }
    /// Height as a fraction of the parent's.
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::row(30.0).height_fraction(0.5).into_style();
    /// assert_eq!(style.size.height, ponko::ui::unstable_taffy::Dimension::percent(0.5));
    /// ```
    pub fn height_fraction(mut self, fraction: f32) -> Self {
        self.0.size.height = percent(fraction);
        self
    }
    /// Puts its top edge `y` pixels below its parent's top, instead of where
    /// the preset put it: a [`centered`](Self::centered) dialog that sits
    /// near the top, an [`anchored`](Self::anchored) panel placed in pixels.
    /// On a node in a row or column it is a margin above it.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let dialog = ui.root().layout(ponko::ui::UiLayout::centered(200.0, 100.0).top(40.0)).id();
    /// ui.push_screen(dialog, true);
    /// ui.layout(vec2(800.0, 600.0));
    /// assert_eq!(ui.rect(dialog).unwrap().min, vec2(300.0, 40.0));
    /// ```
    pub fn top(mut self, y: f32) -> Self {
        if self.0.position == Position::Absolute {
            self.0.inset.top = length(y);
            self.0.margin.top = zero();
        } else {
            self.0.margin.top = length(y);
        }
        self
    }
    /// Puts its left edge `x` pixels right of its parent's left; see
    /// [`top`](Self::top).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
    /// let map = ui.panel(root).layout(ponko::ui::UiLayout::anchored(0.0, 0.0, 0.25, 0.25).left(16.0).top(16.0)).id();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// assert_eq!(ui.rect(map).unwrap().min, vec2(16.0, 16.0));
    /// assert_eq!(ui.rect(map).unwrap().size(), vec2(200.0, 150.0));
    /// ```
    pub fn left(mut self, x: f32) -> Self {
        if self.0.position == Position::Absolute {
            self.0.inset.left = length(x);
            self.0.margin.left = zero();
        } else {
            self.0.margin.left = length(x);
        }
        self
    }
    /// The Taffy style, to read or adjust a field the presets don't cover.
    /// ```
    /// use ponko::prelude::*;
    /// let mut layout = ponko::ui::UiLayout::fill();
    /// layout.style_mut().flex_wrap = ponko::ui::unstable_taffy::FlexWrap::Wrap;
    /// assert_eq!(layout.style().flex_wrap, ponko::ui::unstable_taffy::FlexWrap::Wrap);
    /// ```
    pub fn style(&self) -> &Style {
        &self.0
    }
    /// Mutable access to the Taffy style; see [`UiLayout::style`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut layout = ponko::ui::UiLayout::auto();
    /// layout.style_mut().flex_grow = 1.0;
    /// ```
    pub fn style_mut(&mut self) -> &mut Style {
        &mut self.0
    }
    /// Hidden: the node and its whole subtree take no space, aren't drawn,
    /// don't take the pointer or focus, and are left out of the
    /// accessibility tree. To show it again, set its layout without
    /// `hidden`.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::ui::UiLayout;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(UiLayout::fill()).id();
    /// let a = ui.label(root, "A").layout(UiLayout::row(20.0).hidden()).id();
    /// let b = ui.label(root, "B").layout(UiLayout::row(20.0)).id();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(200.0, 100.0));
    /// assert_eq!(ui.rect(b).unwrap().min, vec2(0.0, 0.0), "A takes no space");
    /// assert!(ui.node_layout(a).unwrap().is_hidden());
    ///
    /// ui.set_layout(a, UiLayout::row(20.0));
    /// ui.layout(vec2(200.0, 100.0));
    /// assert_eq!(ui.rect(b).unwrap().min, vec2(0.0, 20.0));
    /// ```
    pub fn hidden(mut self) -> Self {
        self.0.display = Display::None;
        self
    }
    /// Whether the layout is [`hidden`](Self::hidden).
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.0.display == Display::None
    }
    /// The finished Taffy style. Unstable: see [`crate::ui::unstable_taffy`].
    /// ```
    /// use ponko::prelude::*;
    /// let style = ponko::ui::UiLayout::fill().into_style();
    /// assert_eq!(style.flex_shrink, 1.0);
    /// ```
    pub fn into_style(self) -> Style {
        self.0
    }
}

/// Colors and text defaults shared by the whole tree. Every new node takes
/// its [`UiPaint`] from the theme for its widget; paint set on a node
/// (through a builder or [`Ui::set_paint`]) overrides it.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// ui.set_theme(ponko::ui::UiTheme { text: Color::YELLOW, text_size: 14.0, ..Default::default() });
/// let root = ui.root().id();
/// let label = ui.label(root, "Gold").id();
/// assert_eq!(ui.paint(label).unwrap().color, Color::YELLOW);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiTheme {
    /// Background of panels and scroll areas.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().panel;
    /// ```
    pub panel: Color,
    /// Background of buttons, text inputs, toggles and sliders.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().control;
    /// ```
    pub control: Color,
    /// Hovered or focused background.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().hover;
    /// ```
    pub hover: Color,
    /// Pointer-held background.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().pressed;
    /// ```
    pub pressed: Color,
    /// Disabled background.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().disabled;
    /// ```
    pub disabled: Color,
    /// Borders, the focus ring and slider fill.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().accent;
    /// ```
    pub accent: Color,
    /// Text and image tint.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().text;
    /// ```
    pub text: Color,
    /// Text of titles ([`Ui::title`]).
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().title;
    /// ```
    pub title: Color,
    /// Font handle, from `Graphics::load_font`.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().font;
    /// ```
    pub font: Font,
    /// Text line height in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().text_size;
    /// ```
    pub text_size: f32,
    /// Line height of titles ([`Ui::title`]) in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(ponko::ui::UiTheme::default().title_size > ponko::ui::UiTheme::default().text_size);
    /// ```
    pub title_size: f32,
    /// Border thickness of controls in logical units; panels and text have none.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTheme::default().border_width;
    /// ```
    pub border_width: f32,
}

impl Default for UiTheme {
    /// The engine's look: dark blue-grey controls, white text, orange
    /// accent, titles in the text color and half again as tall.
    fn default() -> Self {
        let paint = UiPaint::default();
        Self {
            panel: paint.background,
            control: paint.background,
            hover: paint.hover,
            pressed: paint.pressed,
            disabled: paint.disabled,
            accent: paint.border,
            text: paint.color,
            title: paint.color,
            font: paint.font,
            text_size: paint.font_size,
            title_size: paint.font_size * 1.5,
            border_width: paint.border_width,
        }
    }
}

impl UiTheme {
    /// The style the [immediate widgets](Ui::immediate) get from this theme:
    /// one theme for the retained tree and the debug widgets. A field this
    /// theme leaves at its default keeps the immediate widgets' own default
    /// (their panels are see-through, for overlays), so the default theme
    /// gives `UiStyle::default()`; a field it changes is taken as it is.
    /// `padding` is the immediate widgets' own.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::UiTheme::default().style(), ponko::ui::UiStyle::default());
    /// let theme = ponko::ui::UiTheme { accent: Color::GREEN, panel: Color::BLACK, ..Default::default() };
    /// let style = theme.style();
    /// assert_eq!((style.accent, style.panel), (Color::GREEN, Color::BLACK));
    /// assert_eq!(style.button, ponko::ui::UiStyle::default().button);
    /// ```
    pub fn style(&self) -> UiStyle {
        let (plain, mut style) = (UiTheme::default(), UiStyle::default());
        macro_rules! take {
            ($($theme:ident => $style:ident),*) => {$(
                if self.$theme != plain.$theme {
                    style.$style = self.$theme;
                }
            )*};
        }
        take!(panel => panel, control => button, hover => button_hovered, pressed => button_pressed,
              accent => accent, text => text, text_size => text_size);
        style
    }

    /// The paint a new node of this widget gets. Labels and images have a
    /// clear background.
    /// ```
    /// use ponko::prelude::*;
    /// let theme = ponko::ui::UiTheme::default();
    /// assert_eq!(theme.paint(ponko::ui::UiWidget::Label).background.a, 0.0);
    /// assert_eq!(theme.paint(ponko::ui::UiWidget::Button).background, theme.control);
    /// ```
    pub fn paint(&self, widget: UiWidget) -> UiPaint {
        let (background, border_width) = match widget {
            UiWidget::Label | UiWidget::Image => (Color::rgba(0.0, 0.0, 0.0, 0.0), 0.0),
            UiWidget::Panel | UiWidget::Scroll => (self.panel, 0.0),
            UiWidget::Button | UiWidget::TextInput | UiWidget::Toggle | UiWidget::Slider => {
                (self.control, self.border_width)
            }
        };
        UiPaint {
            background,
            hover: self.hover,
            pressed: self.pressed,
            disabled: self.disabled,
            border: self.accent,
            border_width,
            color: self.text,
            font: self.font,
            font_size: self.text_size,
            ..UiPaint::default()
        }
    }

    /// The paint a title gets ([`Ui::title`]): a label's, in the title
    /// color and size.
    /// ```
    /// use ponko::prelude::*;
    /// let theme = ponko::ui::UiTheme { title: Color::YELLOW, title_size: 30.0, ..Default::default() };
    /// let paint = theme.title_paint();
    /// assert_eq!((paint.color, paint.font_size), (Color::YELLOW, 30.0));
    /// assert_eq!(paint.background.a, 0.0);
    /// ```
    pub fn title_paint(&self) -> UiPaint {
        UiPaint {
            color: self.title,
            font_size: self.title_size,
            ..self.paint(UiWidget::Label)
        }
    }
}

/// A node being built: each method changes it right away, and [`UiBuilder::id`]
/// returns its id. Dropping the builder keeps the node, so a label nothing
/// refers to again needs no `.id()`.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
/// let title = ui.label(root, "LOBBY").layout(ponko::ui::UiLayout::row(20.0)).size(13.0).color(Color::YELLOW).id();
/// assert_eq!(ui.text(title), Some("LOBBY"));
/// assert_eq!(ui.paint(title).unwrap().font_size, 13.0);
/// ```
pub struct UiBuilder<'a> {
    ui: &'a mut Ui,
    id: UiId,
}

impl UiBuilder<'_> {
    fn paint_with(self, f: impl FnOnce(&mut UiPaint)) -> Self {
        if let Some(paint) = self.ui.paint_mut(self.id) {
            f(paint);
        }
        self
    }
    /// The node's id.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// assert!(ui.rect(root).is_some());
    /// ```
    pub fn id(self) -> UiId {
        self.id
    }
    /// Layout; see [`Ui::set_layout`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fixed(100.0, 50.0)).id();
    /// assert_eq!(ui.node_layout(root), Some(ponko::ui::UiLayout::fixed(100.0, 50.0)));
    /// ```
    pub fn layout(self, layout: UiLayout) -> Self {
        self.ui.set_layout(self.id, layout);
        self
    }
    /// Label, button or input text; see [`Ui::set_text`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let name = ui.text_input(root).text("Player").id();
    /// assert_eq!(ui.text(name), Some("Player"));
    /// ```
    pub fn text(self, text: impl AsRef<str>) -> Self {
        self.ui.set_text(self.id, text);
        self
    }
    /// Text line height in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let id = ui.label(root, "Hi").size(30.0).id();
    /// assert_eq!(ui.paint(id).unwrap().font_size, 30.0);
    /// ```
    pub fn size(self, font_size: f32) -> Self {
        self.paint_with(|p| p.font_size = font_size)
    }
    /// Text and image tint.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let id = ui.label(root, "Hi").color(Color::RED).id();
    /// assert_eq!(ui.paint(id).unwrap().color, Color::RED);
    /// ```
    pub fn color(self, color: Color) -> Self {
        self.paint_with(|p| p.color = color)
    }
    /// Normal background.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().background(Color::BLACK).id();
    /// assert_eq!(ui.paint(root).unwrap().background, Color::BLACK);
    /// ```
    pub fn background(self, color: Color) -> Self {
        self.paint_with(|p| p.background = color)
    }
    /// Border color and thickness in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().border(Color::WHITE, 2.0).id();
    /// assert_eq!(ui.paint(root).unwrap().border_width, 2.0);
    /// ```
    pub fn border(self, color: Color, width: f32) -> Self {
        self.paint_with(|p| {
            p.border = color;
            p.border_width = width;
        })
    }
    /// Border thickness in logical units, in the theme's accent color.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let slot = ui.button(root, "").border_width(1.0).id();
    /// assert_eq!(ui.paint(slot).unwrap().border, ponko::ui::UiTheme::default().accent);
    /// ```
    pub fn border_width(self, width: f32) -> Self {
        self.paint_with(|p| p.border_width = width)
    }
    /// Font handle, from `Graphics::load_font`.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let id = ui.label(root, "Hi").font(Font::DEFAULT).id();
    /// ```
    pub fn font(self, font: Font) -> Self {
        self.paint_with(|p| p.font = font)
    }
    /// Break text at word boundaries.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let id = ui.label(root, "A long story").wrap().id();
    /// assert!(ui.paint(id).unwrap().wrap);
    /// ```
    pub fn wrap(self) -> Self {
        self.paint_with(|p| p.wrap = true)
    }
    /// Horizontal text alignment.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let id = ui.label(root, "Hi").align(ponko::ui::UiTextAlign::Center).id();
    /// assert_eq!(ui.paint(id).unwrap().align, ponko::ui::UiTextAlign::Center);
    /// ```
    pub fn align(self, align: UiTextAlign) -> Self {
        self.paint_with(|p| p.align = align)
    }
    /// Image or background texture; `slice > 0` draws it as nine patches
    /// with that source border in pixels.
    /// ```
    /// use ponko::prelude::*;
    /// # fn build(ui: &mut ponko::ui::Ui, root: ponko::ui::UiId, frame: Texture) {
    /// let id = ui.panel(root).texture(frame, 4.0).id();
    /// assert_eq!(ui.paint(id).unwrap().slice, 4.0);
    /// # }
    /// ```
    pub fn texture(self, texture: Texture, slice: f32) -> Self {
        self.paint_with(|p| {
            p.texture = Some(texture);
            p.slice = slice;
        })
    }
    /// Replace the whole paint; see [`Ui::set_paint`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().paint(ponko::ui::UiPaint { wrap: true, ..Default::default() }).id();
    /// assert!(ui.paint(root).unwrap().wrap);
    /// ```
    pub fn paint(self, paint: UiPaint) -> Self {
        self.ui.set_paint(self.id, paint);
        self
    }
    /// Enable or disable interaction; see [`Ui::set_enabled`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let buy = ui.button(root, "Buy").enabled(false).id();
    /// ```
    pub fn enabled(self, enabled: bool) -> Self {
        self.ui.set_enabled(self.id, enabled);
        self
    }
    /// The node its screen opens on: every [`push_screen`](Ui::push_screen)
    /// of the screen focuses it, so Enter (or a gamepad's South button)
    /// presses it at once; see [`Ui::set_initial_focus`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let play = ui.button(root, "Play").focused().id();
    /// ui.button(root, "Quit");
    /// ui.push_screen(root, false);
    /// assert_eq!(ui.focus(), Some(play));
    /// ```
    pub fn focused(self) -> Self {
        self.ui.set_initial_focus(self.id, true);
        self
    }
    /// Slider (0..1) or toggle (0 off, 1 on) value; see [`Ui::set_value`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let volume = ui.slider(root).value(0.8).id();
    /// assert_eq!(ui.value(volume), Some(0.8));
    /// ```
    pub fn value(self, value: f32) -> Self {
        self.ui.set_value(self.id, value);
        self
    }
    /// Name read by assistive technology; see [`Ui::set_accessible_name`].
    /// Give one to sliders, text inputs, images and icon-only buttons.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let volume = ui.slider(root).accessible_name("Volume").id();
    /// assert_eq!(ui.accessible_name(volume), Some("Volume"));
    /// ```
    pub fn accessible_name(self, name: impl AsRef<str>) -> Self {
        self.ui.set_accessible_name(self.id, name);
        self
    }
    /// Read by assistive technology after the name; see
    /// [`Ui::set_accessible_description`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let wipe = ui.button(root, "Reset").accessible_description("Deletes every save").id();
    /// assert_eq!(ui.accessible_description(wipe), Some("Deletes every save"));
    /// ```
    pub fn accessible_description(self, description: impl AsRef<str>) -> Self {
        self.ui.set_accessible_description(self.id, description);
        self
    }
    /// Announce this node's changes without focus; see [`Ui::set_live_region`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let status = ui.label(root, "Saved").live_region(ponko::ui::UiLive::Polite).id();
    /// assert_eq!(ui.live_region(status), Some(ponko::ui::UiLive::Polite));
    /// ```
    pub fn live_region(self, live: crate::ui::UiLive) -> Self {
        self.ui.set_live_region(self.id, live);
        self
    }
    /// A name to find the node by later ([`Ui::find`]), instead of keeping
    /// its id.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// ui.label(root, "0 coins").name("hud.coins");
    /// let coins = ui.find("hud.coins").unwrap();
    /// assert_eq!(ui.text(coins), Some("0 coins"));
    /// ```
    pub fn name(self, name: impl AsRef<str>) -> Self {
        self.ui.set_name(self.id, name);
        self
    }
}

impl Ui {
    fn build(&mut self, parent: UiId, widget: UiWidget) -> UiBuilder<'_> {
        let id = self.add_child(parent, widget, UiLayout::auto());
        UiBuilder { ui: self, id }
    }
    /// A detached panel, to fill with children and push as a screen.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
    /// ui.push_screen(root, false);
    /// ```
    pub fn root(&mut self) -> UiBuilder<'_> {
        let id = self.add(UiWidget::Panel, UiLayout::auto());
        UiBuilder { ui: self, id }
    }
    /// A child container. Panics, like [`Ui::add_child`], if `parent` was removed.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let bar = ui.panel(root).layout(ponko::ui::UiLayout::row(40.0)).id();
    /// ```
    pub fn panel(&mut self, parent: UiId) -> UiBuilder<'_> {
        self.build(parent, UiWidget::Panel)
    }
    /// A text label.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let score = ui.label(root, "Score: 0").id();
    /// ```
    pub fn label(&mut self, parent: UiId, text: impl AsRef<str>) -> UiBuilder<'_> {
        self.build(parent, UiWidget::Label).text(text)
    }
    /// A title: a label in the theme's title color and size
    /// ([`UiTheme::title`], [`UiTheme::title_size`]), which it follows when
    /// the theme changes.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// ui.set_theme(ponko::ui::UiTheme { title: Color::YELLOW, title_size: 30.0, ..Default::default() });
    /// let root = ui.root().id();
    /// let lobby = ui.title(root, "LOBBY").layout(ponko::ui::UiLayout::row(36.0)).id();
    /// let paint = ui.paint(lobby).unwrap();
    /// assert_eq!((paint.color, paint.font_size), (Color::YELLOW, 30.0));
    /// ```
    pub fn title(&mut self, parent: UiId, text: impl AsRef<str>) -> UiBuilder<'_> {
        let paint = self.theme.title_paint();
        let builder = self.build(parent, UiWidget::Label).text(text).paint(paint);
        if let Some(n) = builder.ui.node_mut(builder.id) {
            n.title = true;
        }
        builder
    }
    /// A button; activating it queues [`crate::ui::UiEvent::Clicked`].
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let play = ui.button(root, "Play").layout(ponko::ui::UiLayout::fixed(200.0, 40.0)).id();
    /// ```
    pub fn button(&mut self, parent: UiId, text: impl AsRef<str>) -> UiBuilder<'_> {
        self.build(parent, UiWidget::Button).text(text)
    }
    /// An image, stretched over the node.
    /// ```
    /// use ponko::prelude::*;
    /// # fn build(ui: &mut ponko::ui::Ui, root: ponko::ui::UiId, coin: Texture) {
    /// let icon = ui.image(root, coin).layout(ponko::ui::UiLayout::fixed(32.0, 32.0)).id();
    /// # }
    /// ```
    pub fn image(&mut self, parent: UiId, texture: Texture) -> UiBuilder<'_> {
        self.build(parent, UiWidget::Image).texture(texture, 0.0)
    }
    /// A clipped, scrolling container. Not called `scroll`, because
    /// [`Ui::scroll`] already reads a node's scroll offset.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
    /// let list = ui.scroll_area(root).layout(ponko::ui::UiLayout::fill()).id();
    /// ```
    pub fn scroll_area(&mut self, parent: UiId) -> UiBuilder<'_> {
        self.build(parent, UiWidget::Scroll)
    }
    /// A single-line text field.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let chat = ui.text_input(root).layout(ponko::ui::UiLayout::row(30.0)).id();
    /// ```
    pub fn text_input(&mut self, parent: UiId) -> UiBuilder<'_> {
        self.build(parent, UiWidget::TextInput)
    }
    /// A checkbox with a caption.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let music = ui.toggle(root, "Music").value(1.0).id();
    /// ```
    pub fn toggle(&mut self, parent: UiId, text: impl AsRef<str>) -> UiBuilder<'_> {
        self.build(parent, UiWidget::Toggle).text(text)
    }
    /// A 0..1 slider.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let volume = ui.slider(root).layout(ponko::ui::UiLayout::fixed(200.0, 24.0)).id();
    /// ```
    pub fn slider(&mut self, parent: UiId) -> UiBuilder<'_> {
        self.build(parent, UiWidget::Slider)
    }

    /// The current theme.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(*ponko::ui::Ui::default().theme(), ponko::ui::UiTheme::default());
    /// ```
    pub fn theme(&self) -> &UiTheme {
        &self.theme
    }
    /// Replace the theme. New nodes take their paint from it; existing nodes
    /// follow it in every paint field still equal to the old theme's value,
    /// and keep the fields set on them.
    ///
    /// The [immediate widgets](Ui::immediate) follow the theme by the same
    /// rule: every field of their [`UiStyle`] still equal to what the old
    /// theme gave them (see [`UiTheme::style`]) takes the new theme's, and a
    /// field set by hand stays. So do their font
    /// ([`UiImmediate::font`](crate::ui::UiImmediate::font), the theme's
    /// [`font`](UiTheme::font)) and button border
    /// ([`UiImmediate::border_width`](crate::ui::UiImmediate::border_width),
    /// the theme's [`border_width`](UiTheme::border_width)).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// ui.immediate.style.text_size = 20.0; // set by hand: kept
    /// ui.set_theme(ponko::ui::UiTheme { accent: Color::GREEN, text_size: 12.0, ..Default::default() });
    /// assert_eq!(ui.immediate.style.accent, Color::GREEN);
    /// assert_eq!(ui.immediate.style.text_size, 20.0);
    /// ```
    ///
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let plain = ui.label(root, "a").id();
    /// let red = ui.label(root, "b").color(Color::RED).id();
    /// ui.set_theme(ponko::ui::UiTheme { text: Color::YELLOW, ..Default::default() });
    /// assert_eq!(ui.paint(plain).unwrap().color, Color::YELLOW);
    /// assert_eq!(ui.paint(red).unwrap().color, Color::RED);
    /// ```
    pub fn set_theme(&mut self, theme: UiTheme) {
        let old = std::mem::replace(&mut self.theme, theme);
        // The immediate widgets follow the same way.
        let (from, to) = (old.style(), theme.style());
        let style = &mut self.immediate.style;
        macro_rules! follow_style {
            ($($field:ident),*) => {$(
                if style.$field == from.$field {
                    style.$field = to.$field;
                }
            )*};
        }
        follow_style!(panel, button, button_hovered, button_pressed, accent, text, text_size);
        // The font and the border are the theme's as they are: their
        // defaults are the immediate widgets' own.
        let immediate = &mut self.immediate;
        if immediate.font == old.font {
            immediate.font = theme.font;
        }
        if immediate.border_width == old.border_width {
            immediate.border_width = theme.border_width;
        }
        for n in self.nodes.iter_mut().flatten() {
            let (from, to) = if n.title {
                (old.title_paint(), theme.title_paint())
            } else {
                (old.paint(n.widget), theme.paint(n.widget))
            };
            let p = &mut n.paint;
            macro_rules! follow {
                ($($field:ident),*) => {$(
                    if p.$field == from.$field {
                        p.$field = to.$field;
                    }
                )*};
            }
            follow!(
                background,
                hover,
                pressed,
                disabled,
                border,
                border_width,
                color,
                font,
                font_size,
                wrap,
                align,
                texture,
                slice
            );
        }
    }
    /// Names a node, so code that didn't build it finds it with
    /// [`find`](Self::find): an update system needs no resource full of ids.
    /// A name belongs to one node of this tree: naming another node the same
    /// moves the name to it. An empty name takes the node's name off. Dotted
    /// names (`"chat.history"`) are a convention, not a hierarchy. Stale ids
    /// are ignored.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let a = ui.label(root, "a").id();
    /// let b = ui.label(root, "b").id();
    /// ui.set_name(a, "status");
    /// assert_eq!((ui.find("status"), ui.name(a)), (Some(a), Some("status")));
    /// ui.set_name(b, "status"); // moves
    /// assert_eq!((ui.find("status"), ui.name(a), ui.name(b)), (Some(b), None, Some("status")));
    /// ui.set_name(b, "");
    /// assert_eq!(ui.find("status"), None);
    /// ```
    pub fn set_name(&mut self, id: UiId, name: impl AsRef<str>) {
        let name = name.as_ref();
        let Some(old) = self
            .node_mut(id)
            .map(|n| std::mem::replace(&mut n.name, name.to_string()))
        else {
            return;
        };
        if self.names.get(&old) == Some(&id) {
            self.names.remove(&old);
        }
        if name.is_empty() {
            return;
        }
        if let Some(before) = self.names.insert(name.to_string(), id).filter(|&b| b != id)
            && let Some(n) = self.node_mut(before)
        {
            n.name.clear();
        }
    }
    /// The node named `name` ([`set_name`](Self::set_name), or `.name(..)`
    /// on a builder), if there is one. A removed node's name is free again.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().name("shop").id();
    /// let buy = ui.button(root, "Buy").name("shop.buy").id();
    /// assert_eq!(ui.find("shop.buy"), Some(buy));
    /// assert_eq!(ui.find("shop.sell"), None);
    /// ui.remove(root); // with its children
    /// assert_eq!((ui.find("shop"), ui.find("shop.buy")), (None, None));
    /// ```
    pub fn find(&self, name: &str) -> Option<UiId> {
        self.names.get(name).copied()
    }
    /// A node's name, or `None` without one (or for a stale id).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().name("menu").id();
    /// assert_eq!(ui.name(root), Some("menu"));
    /// ```
    pub fn name(&self, id: UiId) -> Option<&str> {
        self.node(id).map(|n| n.name.as_str()).filter(|n| !n.is_empty())
    }
    /// A node's resolved paint (theme plus its own settings), or `None` for a
    /// stale id.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// assert_eq!(ui.paint(root).unwrap().background, ponko::ui::UiTheme::default().panel);
    /// ```
    pub fn paint(&self, id: UiId) -> Option<UiPaint> {
        Some(self.node(id)?.paint)
    }
}
