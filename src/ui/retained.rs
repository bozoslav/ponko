//! Retained, game-styled UI. Taffy owns only layout; this module owns nodes,
//! focus, screens and events, and Ponko draws the results with sprites/text.
//! Taffy's flex/grid algorithms embed without a renderer or event loop.
//! Egui embeds well but is immediate-mode (and remains the inspector); the
//! standard Xilem/Masonry stack couples widgets to Vello and winit. Keeping
//! our tree makes game-owned textures, stable IDs and queued events natural.
//! Layout uses logical window units; the window handles DPI and `Ui::set_scale`
//! adds an accessibility/user scale. Nodes persist until explicitly removed.

use super::build::UiLayout;
use crate::{camera::Rect, color::Color, text::Font, texture::Texture};
use glam::{Vec2, vec2};
use taffy::prelude::*;

/// Stable node identity, never reused within a UI.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let id: ponko::ui::UiId = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
/// assert!(ui.rect(id).is_some());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiId(usize);

impl UiId {
    /// The id as an integer, for hosts that store ids outside the UI (the C
    /// API does).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// assert_eq!(ponko::ui::UiId::from_bits(id.bits()), id);
    /// ```
    pub const fn bits(self) -> u64 {
        self.0 as u64
    }
    /// Rebuild an id from [`UiId::bits`]. Unknown values are stale: every call
    /// ignores or rejects them instead of panicking.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(ponko::ui::Ui::default().rect(ponko::ui::UiId::from_bits(7)).is_none());
    /// ```
    pub const fn from_bits(bits: u64) -> Self {
        // Out-of-range values (32-bit targets) become an id no node has.
        Self(if bits > usize::MAX as u64 {
            usize::MAX
        } else {
            bits as usize
        })
    }
}

/// Built-in behavior; text, images and decoration are configured separately.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let button = ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
/// ui.set_text(button, "Buy");
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum UiWidget {
    /// Non-focusable container.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::Panel;
    /// ```
    Panel,
    /// Non-focusable text.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::Label;
    /// ```
    Label,
    /// Press/release or confirm emits `Clicked`.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::Button;
    /// ```
    Button,
    /// Non-focusable texture.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::Image;
    /// ```
    Image,
    /// Clipped container with two-axis wheel scrolling and vertical drag/bar.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::Scroll;
    /// ```
    Scroll,
    /// Single-line Unicode input, with basic selection (no IME/clipboard).
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::TextInput;
    /// ```
    TextInput,
    /// Checkbox; confirm or clicking toggles its value.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::Toggle;
    /// ```
    Toggle,
    /// Normalized 0..1 value, changed by pointer or left/right.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiWidget::Slider;
    /// ```
    Slider,
}

/// Text alignment within a node's content box.
/// ```
/// use ponko::prelude::*;
/// let paint = ponko::ui::UiPaint { align: ponko::ui::UiTextAlign::Center, ..Default::default() };
/// assert_eq!(paint.align, ponko::ui::UiTextAlign::Center);
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum UiTextAlign {
    /// Start of each line.
    #[default]
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTextAlign::Start;
    /// ```
    Start,
    /// Center each line.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTextAlign::Center;
    /// ```
    Center,
    /// End of each line.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiTextAlign::End;
    /// ```
    End,
}

/// Visual style, independent of Taffy's layout style. A texture with
/// `slice > 0` is drawn as nine patches; `slice` is the source border in pixels.
/// ```
/// use ponko::prelude::*;
/// let paint = ponko::ui::UiPaint { background: Color::BLUE, font_size: 20.0, ..Default::default() };
/// assert_eq!(paint.font_size, 20.0);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct UiPaint {
    /// Normal background.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().background;
    /// ```
    pub background: Color,
    /// Hovered/focused background.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().hover;
    /// ```
    pub hover: Color,
    /// Pointer-held background.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().pressed;
    /// ```
    pub pressed: Color,
    /// Disabled background.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().disabled;
    /// ```
    pub disabled: Color,
    /// Border and focus-ring color.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().border;
    /// ```
    pub border: Color,
    /// Border thickness in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().border_width;
    /// ```
    pub border_width: f32,
    /// Text and image tint.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().color;
    /// ```
    pub color: Color,
    /// Font handle, from `Graphics::load_font`.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().font;
    /// ```
    pub font: Font,
    /// Text line height in logical units.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().font_size;
    /// ```
    pub font_size: f32,
    /// Break text at word boundaries, splitting oversized words.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().wrap;
    /// ```
    pub wrap: bool,
    /// Horizontal text alignment.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().align;
    /// ```
    pub align: UiTextAlign,
    /// Image or background texture; `None` draws only color.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().texture;
    /// ```
    pub texture: Option<Texture>,
    /// Source border size for nine-slice drawing; zero stretches the image.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiPaint::default().slice;
    /// ```
    pub slice: f32,
}
impl Default for UiPaint {
    fn default() -> Self {
        Self {
            background: Color::rgba(0.12, 0.14, 0.18, 1.0),
            hover: Color::rgba(0.25, 0.3, 0.4, 1.0),
            pressed: Color::rgba(0.08, 0.1, 0.15, 1.0),
            disabled: Color::rgba(0.15, 0.15, 0.15, 0.5),
            border: Color::hex(0xf28c28),
            border_width: 0.0,
            color: Color::WHITE,
            font: Font::DEFAULT,
            font_size: 18.0,
            wrap: false,
            align: UiTextAlign::Start,
            texture: None,
            slice: 0.0,
        }
    }
}

/// One semantic event; values are read from the node after `Changed`.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// assert_eq!(ui.events().count(), 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UiEvent {
    /// A button was activated.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
    /// let _ = ponko::ui::UiEvent::Clicked(id);
    /// ```
    Clicked(UiId),
    /// Text, checkbox or slider value changed.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
    /// let _ = ponko::ui::UiEvent::Changed(id);
    /// ```
    Changed(UiId),
    /// Enter/confirm in a text field.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
    /// let _ = ponko::ui::UiEvent::Submitted(id);
    /// ```
    Submitted(UiId),
    /// Cancel popped this screen; the input is consumed for this frame.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
    /// let _ = ponko::ui::UiEvent::ScreenPopped(id);
    /// ```
    ScreenPopped(UiId),
}

impl UiEvent {
    /// The node of a [`Clicked`](Self::Clicked) event, else `None`. For a
    /// screen that reacts only to clicks:
    /// `ui.events().filter_map(UiEvent::clicked).last()`.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
    /// assert_eq!(ponko::ui::UiEvent::Clicked(id).clicked(), Some(id));
    /// assert_eq!(ponko::ui::UiEvent::Changed(id).clicked(), None);
    /// ```
    #[must_use]
    pub fn clicked(self) -> Option<UiId> {
        match self {
            Self::Clicked(id) => Some(id),
            _ => None,
        }
    }
}

/// Input snapshot in logical window coordinates. `Ctx` supplies this each
/// frame; tests and custom hosts can call `Ui::frame` directly.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// ui.frame(vec2(800.0,600.0), &ponko::ui::UiInput::default());
/// ```
#[derive(Debug, Default, Clone)]
pub struct UiInput {
    /// Pointer position in logical window units (before user scaling).
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().pointer;
    /// ```
    pub pointer: Vec2,
    /// Primary pointer pressed this frame.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().pressed;
    /// ```
    pub pressed: bool,
    /// Primary pointer held.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().held;
    /// ```
    pub held: bool,
    /// Primary pointer released this frame.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().released;
    /// ```
    pub released: bool,
    /// Wheel displacement in logical units (positive scrolls down/right).
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().wheel;
    /// ```
    pub wheel: Vec2,
    /// Tab/next focus.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().next;
    /// ```
    pub next: bool,
    /// Shift-tab/previous focus.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().previous;
    /// ```
    pub previous: bool,
    /// Directional focus, caret or slider movement (-1..1 per axis).
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().direction;
    /// ```
    pub direction: Vec2,
    /// Enter/space/gamepad south.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().confirm;
    /// ```
    pub confirm: bool,
    /// Escape/gamepad east, pops a screen.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().cancel;
    /// ```
    pub cancel: bool,
    /// Shift extends text selection.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().shift;
    /// ```
    pub shift: bool,
    /// Select all text.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().select_all;
    /// ```
    pub select_all: bool,
    /// Delete before caret.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().backspace;
    /// ```
    pub backspace: bool,
    /// Delete after caret.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().delete;
    /// ```
    pub delete: bool,
    /// Move caret to start.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().home;
    /// ```
    pub home: bool,
    /// Move caret to end.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().end;
    /// ```
    pub end: bool,
    /// Committed text; control characters are ignored.
    /// ```
    /// use ponko::prelude::*;
    /// let _ = ponko::ui::UiInput::default().text;
    /// ```
    pub text: String,
}

pub(super) struct Node {
    pub widget: UiWidget,
    pub layout: NodeId,
    pub parent: Option<UiId>,
    pub children: Vec<UiId>,
    pub text: String,
    pub paint: UiPaint,
    pub enabled: bool,
    pub rect: Rect,
    pub clip: Rect,
    pub scroll: Vec2,
    pub max_scroll: Vec2,
    pub value: f32,
    pub caret: usize,
    pub anchor: usize,
    /// Accessible name override; empty means "derive it from the text".
    pub access_name: String,
    /// What assistive technology reads after the name; empty: nothing.
    pub access_description: String,
    /// Whether changes to it are announced without it having focus.
    pub access_live: super::UiLive,
    /// The name it is found by ([`Ui::find`]); empty: none.
    pub name: String,
    /// A title ([`Ui::title`]): painted from the theme's title color and size.
    pub title: bool,
    /// Its screen opens with it focused ([`Ui::set_initial_focus`]).
    pub initial_focus: bool,
}
pub(super) struct Screen {
    pub root: UiId,
    pub modal: bool,
    saved_focus: Option<UiId>,
}
#[derive(Clone, Copy)]
enum Drag {
    Scroll(UiId, Vec2, Vec2),
    Bar(UiId, f32, f32),
    Text(UiId),
}

/// The screen's UI: `ctx.ui`. An owned widget tree and screen stack: build
/// once, then mutate node properties. Draining events avoids callbacks
/// borrowing the game and UI together.
///
/// - **What lives here:** the retained widget tree with its screen stack,
///   focus and queued events; who has the pointer and the keyboard
///   ([`wants_pointer`](Self::wants_pointer),
///   [`wants_keyboard`](Self::wants_keyboard)); and the immediate widgets
///   for debug overlays, [`immediate`](Self::immediate).
/// - **When to call it:** build the tree in setup, then change text, values
///   and screens from any system. The engine lays it out and feeds it input
///   before the frame's systems, so [`events`](Self::events) queued this
///   frame are read in a frame or simulation system. Immediate widgets are
///   drawn the frame they are called.
/// - **What it needs:** nothing to build and query (tests drive it with
///   [`frame`](Self::frame)); drawing needs a window. It borrows apart from
///   the world and the other services: read its events while playing
///   sounds or changing entities.
///
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
/// ui.push_screen(root, false);
/// ui.frame(vec2(800.0,600.0), &ponko::ui::UiInput::default());
/// ```
pub struct Ui {
    pub(super) tree: TaffyTree<usize>,
    pub(super) nodes: Vec<Option<Node>>,
    pub(super) screens: Vec<Screen>,
    /// The HUD roots ([`Ui::show_hud`]), drawn below every screen, bottom first.
    pub(super) huds: Vec<UiId>,
    pub(super) visible: Vec<UiId>,
    events: std::collections::VecDeque<UiEvent>,
    pub(super) focus: Option<UiId>,
    pub(super) hovered: Option<UiId>,
    pub(super) pressed: Option<UiId>,
    drag: Option<Drag>,
    scale: f32,
    viewport: Vec2,
    pointer: Vec2,
    measure: crate::text::Text,
    external_fonts: bool,
    consumed: bool,
    /// Requests from assistive technology, applied by the next [`Ui::frame`].
    pub(super) access_actions: Vec<super::UiAccessAction>,
    pub(super) theme: super::UiTheme,
    /// The named nodes ([`Ui::find`]).
    pub(super) names: std::collections::HashMap<String, UiId>,
    /// The immediate widgets (a panel, a label, a button drawn the frame
    /// they are called) and their style. Only the screen's UI (`ctx.ui`)
    /// shows them; a [`UiPanel3D`](crate::d3::UiPanel)'s tree leaves it unused.
    pub immediate: super::UiImmediate,
    /// The mouse is over the inspector overlay.
    pub(crate) overlay_has_pointer: bool,
    /// The ray the mouse uses on world panels instead of the camera's through it.
    #[cfg(feature = "3d")]
    pub(crate) pointer_ray: Option<(glam::Vec3, glam::Vec3)>,
}
// SAFETY: the only part that isn't `Send`/`Sync` is taffy's `CompactLength`,
// which may hold an opaque `calc()` handle as a raw pointer. The engine
// never turns on taffy's `calc` feature or makes such values, so every
// length is plain numbers; the rest of `Ui` is owned data with no interior
// mutability. That lets a `Ui` live in a component (`UiPanel3D`).
unsafe impl Send for Ui {}
unsafe impl Sync for Ui {}
impl Default for Ui {
    fn default() -> Self {
        Self {
            tree: TaffyTree::new(),
            nodes: Vec::new(),
            screens: Vec::new(),
            huds: Vec::new(),
            visible: Vec::new(),
            events: std::collections::VecDeque::new(),
            focus: None,
            hovered: None,
            pressed: None,
            drag: None,
            scale: 1.0,
            viewport: Vec2::ZERO,
            pointer: Vec2::ZERO,
            measure: crate::text::Text::new(),
            external_fonts: false,
            consumed: false,
            access_actions: Vec::new(),
            theme: super::UiTheme::default(),
            names: std::collections::HashMap::new(),
            immediate: super::UiImmediate::default(),
            overlay_has_pointer: false,
            #[cfg(feature = "3d")]
            pointer_ray: None,
        }
    }
}
impl Ui {
    pub(super) fn node(&self, id: UiId) -> Option<&Node> {
        self.nodes.get(id.0)?.as_ref()
    }
    pub(super) fn node_mut(&mut self, id: UiId) -> Option<&mut Node> {
        self.nodes.get_mut(id.0)?.as_mut()
    }
    pub(super) fn paint_mut(&mut self, id: UiId) -> Option<&mut UiPaint> {
        Some(&mut self.node_mut(id)?.paint)
    }

    /// Add a detached node. Push it as a screen or use `add_child` instead.
    /// Its paint comes from the theme ([`Ui::set_theme`]). [`Ui::root`] and
    /// the other builders do the same in one expression.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// ```
    pub fn add(&mut self, widget: UiWidget, layout: UiLayout) -> UiId {
        let id = UiId(self.nodes.len());
        let layout = self
            .tree
            .new_leaf_with_context(layout.into_style(), id.0)
            .expect("valid UI layout");
        let paint = self.theme.paint(widget);
        self.nodes.push(Some(Node {
            widget,
            layout,
            parent: None,
            children: Vec::new(),
            text: String::new(),
            paint,
            enabled: true,
            rect: Rect::default(),
            clip: Rect::default(),
            scroll: Vec2::ZERO,
            max_scroll: Vec2::ZERO,
            value: 0.0,
            caret: 0,
            anchor: 0,
            access_name: String::new(),
            access_description: String::new(),
            access_live: super::UiLive::Off,
            name: String::new(),
            title: false,
            initial_focus: false,
        }));
        id
    }
    /// Add a child. Panics if the parent was removed or belongs to another UI.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// let child = ui.add_child(root, ponko::ui::UiWidget::Label, ponko::ui::UiLayout::auto());
    /// ```
    pub fn add_child(&mut self, parent: UiId, widget: UiWidget, layout: UiLayout) -> UiId {
        let p = self.node(parent).expect("UI parent must exist").layout;
        let id = self.add(widget, layout);
        let node = self.node_mut(id).unwrap();
        node.parent = Some(parent);
        let layout = node.layout;
        self.tree.add_child(p, layout).expect("valid UI child");
        self.node_mut(parent).unwrap().children.push(id);
        id
    }
    /// Remove a subtree. Stale IDs stay invalid; removing a screen drops it.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// ui.remove(id);
    /// assert!(ui.rect(id).is_none());
    /// ```
    pub fn remove(&mut self, id: UiId) {
        while let Some(child) = self.node(id).and_then(|n| n.children.last().copied()) {
            self.remove(child);
        }
        if let Some(n) = self.nodes.get_mut(id.0).and_then(Option::take) {
            if let Some(parent) = n.parent.and_then(|p| self.node_mut(p)) {
                parent.children.retain(|&c| c != id);
            }
            let _ = self.tree.remove(n.layout);
            if self.names.get(&n.name) == Some(&id) {
                self.names.remove(&n.name);
            }
        }
        self.screens.retain(|s| s.root != id);
        self.huds.retain(|&h| h != id);
        self.visible.retain(|&n| n != id);
        if self.drag.is_some_and(|d| match d {
            Drag::Scroll(n, _, _) | Drag::Bar(n, _, _) | Drag::Text(n) => n == id,
        }) {
            self.drag = None;
        }
        if self.focus == Some(id) {
            self.focus = None;
        }
        if self.pressed == Some(id) {
            self.pressed = None;
            self.drag = None;
        }
        self.events.retain(|e| match e {
            UiEvent::Clicked(n) | UiEvent::Changed(n) | UiEvent::Submitted(n) | UiEvent::ScreenPopped(n) => *n != id,
        });
    }
    /// Replace the node's layout (flex, grid, absolute positioning and
    /// constraints). [`Ui::node_layout`] reads it back.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// ui.set_layout(id, ponko::ui::UiLayout::fixed(100.0, 40.0));
    /// ```
    pub fn set_layout(&mut self, id: UiId, layout: UiLayout) {
        if let Some(n) = self.node(id) {
            let _ = self.tree.set_style(n.layout, layout.into_style());
        }
    }
    /// The node's current layout, or `None` for a removed node.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let id = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::fill());
    /// assert_eq!(ui.node_layout(id), Some(ponko::ui::UiLayout::fill()));
    /// ```
    pub fn node_layout(&self, id: UiId) -> Option<UiLayout> {
        let style = self.tree.style(self.node(id)?.layout).ok()?;
        Some(UiLayout::custom(style.clone()))
    }
    /// Replace visual style.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// ui.set_paint(id, ponko::ui::UiPaint::default());
    /// ```
    pub fn set_paint(&mut self, id: UiId, paint: UiPaint) {
        if let Some(n) = self.node_mut(id) {
            n.paint = paint;
        }
    }
    /// Set label or input text. Input control characters are discarded.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Label, ponko::ui::UiLayout::auto());
    /// ui.set_text(id, "Shop"); assert_eq!(ui.text(id), Some("Shop"));
    /// ```
    pub fn set_text(&mut self, id: UiId, text: impl AsRef<str>) {
        if let Some(n) = self.node_mut(id) {
            n.text.clear();
            if n.widget == UiWidget::TextInput {
                n.text.extend(text.as_ref().chars().filter(|c| !c.is_control()));
            } else {
                n.text.push_str(text.as_ref());
            }
            n.caret = n.text.len();
            n.anchor = n.caret;
        }
    }
    /// Read text, or `None` for a stale ID.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Label, ponko::ui::UiLayout::auto());
    /// assert_eq!(ui.text(id), Some(""));
    /// ```
    pub fn text(&self, id: UiId) -> Option<&str> {
        Some(&self.node(id)?.text)
    }
    /// Enable/disable a node and all descendant interaction.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
    /// ui.set_enabled(id, false);
    /// ```
    pub fn set_enabled(&mut self, id: UiId, enabled: bool) {
        if let Some(n) = self.node_mut(id) {
            n.enabled = enabled;
        }
    }
    /// Slider value (0..1), or toggle value (zero/off, one/on).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Slider, ponko::ui::UiLayout::auto());
    /// ui.set_value(id, 0.5); assert_eq!(ui.value(id), Some(0.5));
    /// ```
    pub fn set_value(&mut self, id: UiId, value: f32) {
        if let Some(n) = self.node_mut(id) {
            n.value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        }
    }
    /// Read the current slider/toggle value.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Toggle, ponko::ui::UiLayout::auto());
    /// assert_eq!(ui.value(id), Some(0.0));
    /// ```
    pub fn value(&self, id: UiId) -> Option<f32> {
        Some(self.node(id)?.value)
    }
    /// Set scroll offset in unscaled layout units; clamped during layout.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Scroll, ponko::ui::UiLayout::auto());
    /// ui.set_scroll(id, vec2(0.0,40.0));
    /// ```
    pub fn set_scroll(&mut self, id: UiId, offset: Vec2) {
        if let Some(n) = self.node_mut(id) {
            n.scroll = offset.max(Vec2::ZERO);
        }
    }
    /// Current clamped offset, after layout.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Scroll, ponko::ui::UiLayout::auto());
    /// assert_eq!(ui.scroll(id), Some(Vec2::ZERO));
    /// ```
    pub fn scroll(&self, id: UiId) -> Option<Vec2> {
        Some(self.node(id)?.scroll)
    }
    /// Push a detached root; only the top screen is interactive. `modal`
    /// draws a dim overlay below this screen. Duplicate/parented roots panic,
    /// and so does a root shown as a [HUD](Self::show_hud).
    ///
    /// The screen opens with its [initial focus](Self::set_initial_focus)
    /// (the builder's [`focused`](super::UiBuilder::focused)) focused, every
    /// time it is pushed; without one, nothing is focused until Tab, an
    /// arrow key or [`set_focus`](Self::set_focus).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let root = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// ui.push_screen(root, true);
    /// ```
    pub fn push_screen(&mut self, root: UiId, modal: bool) {
        assert!(
            self.node(root).is_some_and(|n| n.parent.is_none()),
            "UI screen must be a detached root"
        );
        assert!(
            !self.screens.iter().any(|s| s.root == root),
            "UI screen already on stack"
        );
        assert!(!self.huds.contains(&root), "UI screen is shown as a HUD");
        self.screens.push(Screen {
            root,
            modal,
            saved_focus: self.focus.take(),
        });
        self.pressed = None;
        self.drag = None;
        self.hovered = None;
        self.focus = self.initial_focus_in(root);
    }
    /// The first node under `root`, depth-first, marked as its screen's
    /// initial focus that can take the focus now.
    fn initial_focus_in(&self, root: UiId) -> Option<UiId> {
        let n = self.node(root)?;
        if n.initial_focus && self.can_focus(root) {
            return Some(root);
        }
        n.children.iter().find_map(|&child| self.initial_focus_in(child))
    }
    /// Moves the keyboard and gamepad focus to `id`, as Tab or a click
    /// would: Enter (or a gamepad's South button) then activates it. Returns
    /// whether it took. Only a button, text input, toggle or slider can
    /// have the focus, and only while it is enabled, [shown](Self::is_shown)
    /// and on the top screen; anything else leaves the focus as it is and
    /// returns `false`. A node with no size after the next layout loses it
    /// again.
    ///
    /// For the node a screen should open on every time it is pushed, mark it
    /// instead ([`set_initial_focus`](Self::set_initial_focus), or the
    /// builder's [`focused`](super::UiBuilder::focused)).
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::ui::{UiInput, UiLayout};
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(UiLayout::fill()).id();
    /// let play = ui.button(root, "Play").layout(UiLayout::row(40.0)).id();
    /// let quit = ui.button(root, "Quit").layout(UiLayout::row(40.0)).id();
    /// ui.push_screen(root, false);
    /// assert!(ui.set_focus(quit));
    /// ui.frame(vec2(800.0, 600.0), &UiInput { confirm: true, ..Default::default() });
    /// assert_eq!(ui.events().filter_map(ponko::ui::UiEvent::clicked).last(), Some(quit));
    /// assert!(!ui.set_focus(root), "a panel can't have the focus");
    /// # let _ = play;
    /// ```
    pub fn set_focus(&mut self, id: UiId) -> bool {
        let ok = self.can_focus(id);
        if ok {
            self.focus = Some(id);
        }
        ok
    }
    /// Takes the focus away: no node has it until Tab, an arrow key, a click
    /// or [`set_focus`](Self::set_focus).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let ok = ui.button(root, "OK").focused().id();
    /// ui.push_screen(root, false);
    /// assert_eq!(ui.focus(), Some(ok));
    /// ui.clear_focus();
    /// assert_eq!(ui.focus(), None);
    /// ```
    pub fn clear_focus(&mut self) {
        self.focus = None;
    }
    /// Marks `id` as the node its screen opens on (or unmarks it): every
    /// [`push_screen`](Self::push_screen) of the screen focuses it, so a
    /// keyboard or gamepad player can press Enter at once, and a menu that
    /// is opened again starts on it again. If several nodes of a screen are
    /// marked, the first one (depth-first) that can take the focus wins. The
    /// builder's [`focused`](super::UiBuilder::focused) does the same.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let play = ui.button(root, "Play").id();
    /// ui.set_initial_focus(play, true);
    /// assert!(ui.is_initial_focus(play));
    /// ui.push_screen(root, false);
    /// assert_eq!(ui.focus(), Some(play));
    /// ```
    pub fn set_initial_focus(&mut self, id: UiId, on: bool) {
        if let Some(n) = self.node_mut(id) {
            n.initial_focus = on;
        }
    }
    /// Whether `id` is marked as the node its screen opens on
    /// ([`set_initial_focus`](Self::set_initial_focus)).
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let play = ui.button(root, "Play").focused().id();
    /// assert!(ui.is_initial_focus(play));
    /// assert!(!ui.is_initial_focus(root));
    /// ```
    pub fn is_initial_focus(&self, id: UiId) -> bool {
        self.node(id).is_some_and(|n| n.initial_focus)
    }
    /// Whether `id` may take the focus now, before layout gives it a size.
    fn can_focus(&self, id: UiId) -> bool {
        self.interactive(id)
            && self.is_shown(id)
            && self.node(id).is_some_and(|n| {
                matches!(
                    n.widget,
                    UiWidget::Button | UiWidget::TextInput | UiWidget::Toggle | UiWidget::Slider
                )
            })
    }
    /// Shows the tree under a detached `root` as a HUD: display-only UI,
    /// such as a score or a health bar, drawn below every screen and kept
    /// up while screens open and close above it. Escape (or a gamepad's
    /// East button) never closes it, its nodes never take the pointer or the
    /// focus, and it doesn't make [`wants_pointer`](Self::wants_pointer) or
    /// [`wants_keyboard`](Self::wants_keyboard) true, so the game keeps its
    /// keys. Anything the player clicks belongs on a screen
    /// ([`push_screen`](Self::push_screen)). Several HUDs are drawn in the
    /// order they were shown; showing one again changes nothing. Panics like
    /// `push_screen` for a node with a parent, or one on the screen stack.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::ui::{UiInput, UiLayout};
    /// let mut ui = ponko::ui::Ui::default();
    /// let hud = ui.root().layout(UiLayout::fill().padding(16.0)).id();
    /// let coins = ui.label(hud, "Coins: 0").id();
    /// ui.show_hud(hud);
    /// assert!(ui.is_shown(coins) && !ui.wants_keyboard());
    /// ui.frame(vec2(800.0, 600.0), &UiInput { cancel: true, ..Default::default() });
    /// assert!(ui.is_shown(coins), "Escape leaves a HUD up");
    /// ```
    pub fn show_hud(&mut self, root: UiId) {
        assert!(
            self.node(root).is_some_and(|n| n.parent.is_none()),
            "UI HUD must be a detached root"
        );
        assert!(!self.is_screen(root), "UI HUD is already a screen");
        if !self.huds.contains(&root) {
            self.huds.push(root);
        }
    }
    /// Takes a HUD ([`show_hud`](Self::show_hud)) off the screen. Its nodes
    /// stay, so it can be shown again. Does nothing for a node that isn't a
    /// HUD.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let hud = ui.root().id();
    /// ui.show_hud(hud);
    /// ui.hide_hud(hud);
    /// assert!(!ui.is_hud(hud) && !ui.is_shown(hud));
    /// ```
    pub fn hide_hud(&mut self, root: UiId) {
        self.huds.retain(|&h| h != root);
    }
    /// Whether `id` is a HUD's root ([`show_hud`](Self::show_hud)) right now.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let hud = ui.root().id();
    /// assert!(!ui.is_hud(hud));
    /// ui.show_hud(hud);
    /// assert!(ui.is_hud(hud));
    /// ```
    pub fn is_hud(&self, id: UiId) -> bool {
        self.huds.contains(&id)
    }
    /// Pop a screen, restoring the previous focus. Nodes remain reusable.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let root = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// ui.push_screen(root, true); assert_eq!(ui.pop_screen(), Some(root));
    /// ```
    pub fn pop_screen(&mut self) -> Option<UiId> {
        let screen = self.screens.pop()?;
        self.consumed = true;
        self.focus = screen.saved_focus.filter(|&id| self.interactive(id));
        self.pressed = None;
        self.drag = None;
        self.hovered = None;
        Some(screen.root)
    }
    /// The node's parent, or `None` for a detached root and for stale IDs.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// let child = ui.add_child(root, ponko::ui::UiWidget::Label, ponko::ui::UiLayout::auto());
    /// assert_eq!(ui.parent(child), Some(root));
    /// assert_eq!(ui.parent(root), None);
    /// ```
    pub fn parent(&self, id: UiId) -> Option<UiId> {
        self.node(id)?.parent
    }
    /// Whether `id` is on the screen stack right now.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// assert!(!ui.is_screen(root));
    /// ui.push_screen(root, false);
    /// assert!(ui.is_screen(root));
    /// ```
    pub fn is_screen(&self, id: UiId) -> bool {
        self.screens.iter().any(|s| s.root == id)
    }
    /// Whether the node is shown: it belongs to a screen on the stack (a
    /// modal above may cover it) or a [HUD](Self::show_hud), and neither it nor a node above it is
    /// [hidden](super::UiLayout::hidden). A tree built but not pushed, or a
    /// popped screen, is not shown. Stale ids are not shown.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let ok = ui.button(root, "OK").id();
    /// assert!(!ui.is_shown(ok), "built, not pushed");
    /// ui.push_screen(root, false);
    /// assert!(ui.is_shown(ok));
    /// ui.set_layout(ok, ponko::ui::UiLayout::auto().hidden());
    /// assert!(!ui.is_shown(ok));
    /// ```
    pub fn is_shown(&self, id: UiId) -> bool {
        let mut at = id;
        loop {
            let Some(n) = self.node(at) else { return false };
            if self
                .tree
                .style(n.layout)
                .is_ok_and(|s| s.display == taffy::Display::None)
            {
                return false;
            }
            match n.parent {
                Some(parent) => at = parent,
                None => return self.is_screen(at) || self.is_hud(at),
            }
        }
    }
    /// Top screen root.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::Ui::default().top_screen(), None);
    /// ```
    pub fn top_screen(&self) -> Option<UiId> {
        self.screens.last().map(|s| s.root)
    }
    /// User scale, multiplied with the window's existing DPI handling. The
    /// [immediate widgets](Ui::immediate) are drawn at it too: their sizes
    /// are in the same units as the tree's layout.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); ui.set_scale(1.5);
    /// assert_eq!(ui.immediate.scale(), 1.5);
    /// ```
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = if scale.is_finite() { scale.clamp(0.25, 8.0) } else { 1.0 };
        self.immediate.scale = self.scale;
    }
    /// The node's rectangle in logical window coordinates (including user
    /// scale) as of the last layout: [`Ui::frame`] (run by the engine before
    /// the frame's systems) or [`Ui::layout`]. It is a zero rectangle until the
    /// first layout, and changes made since then show only after the next
    /// one. `None` for an id not in the tree.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); let id = ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// assert_eq!(ui.rect(id), Some(Rect::default()), "no layout yet");
    /// ```
    pub fn rect(&self, id: UiId) -> Option<Rect> {
        Some(self.node(id)?.rect)
    }
    /// Current keyboard/gamepad focus.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::Ui::default().focus(), None);
    /// ```
    pub fn focus(&self) -> Option<UiId> {
        self.focus
    }
    /// Takes the queued semantic events, oldest first. Events left untaken
    /// (stopping the loop early) stay queued until the next frame clears
    /// them. To change the tree while reading, take them one at a time with
    /// [`next_event`](Self::next_event). For a screen that reacts only to
    /// clicks:
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::ui::UiEvent;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
    /// let ok = ui.button(root, "OK").layout(ponko::ui::UiLayout::fixed(100.0, 40.0)).id();
    /// ui.push_screen(root, false);
    /// let at = |pressed, held, released| ponko::ui::UiInput {
    ///     pointer: vec2(50.0, 20.0),
    ///     pressed,
    ///     held,
    ///     released,
    ///     ..Default::default()
    /// };
    /// ui.frame(vec2(800.0, 600.0), &at(true, true, false));
    /// ui.frame(vec2(800.0, 600.0), &at(false, false, true));
    /// let clicked = ui.events().filter_map(UiEvent::clicked).last();
    /// assert_eq!(clicked, Some(ok));
    /// assert_eq!(ui.events().count(), 0);
    /// ```
    pub fn events(&mut self) -> impl Iterator<Item = UiEvent> + '_ {
        std::iter::from_fn(move || self.events.pop_front())
    }
    /// Takes the oldest queued event, or `None` when none are left: what
    /// [`events`](Self::events) gives one at a time, for a loop whose body
    /// changes the tree (`while let Some(e) = ui.next_event() { ... }`).
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::Ui::default().next_event(), None);
    /// ```
    pub fn next_event(&mut self) -> Option<UiEvent> {
        self.events.pop_front()
    }
    /// The pointer belongs to UI: it is over a widget of the tree or an
    /// [immediate](Self::immediate) one drawn this frame or the previous
    /// one, a drag is captured, or a modal screen is open (it captures
    /// outside too). Check it before reacting to clicks in the world, so
    /// clicking a button doesn't also select a unit behind it.
    ///
    /// The pointer is where the mouse was when the running system started.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(!ponko::ui::Ui::default().wants_pointer());
    /// # fn select(ctx: &mut Ctx) {
    /// if ctx.input.mouse_pressed(MouseButton::Left) && !ctx.ui.wants_pointer() {
    ///     let at = ctx.mouse_world();
    ///     # let _ = at;
    /// }
    /// # }
    /// ```
    pub fn wants_pointer(&self) -> bool {
        self.consumed
            || self.pressed.is_some()
            || self.drag.is_some()
            || self.hit_test(self.pointer).is_some()
            || self.screens.last().is_some_and(|s| s.modal)
            || self.overlay_has_pointer
            || self.immediate.covers(self.pointer)
    }
    /// Engine side: where the mouse is now, between frames of the tree.
    pub(crate) fn set_pointer(&mut self, at: Vec2) {
        self.pointer = at;
    }
    /// Sets the ray the mouse uses on [`UiPanel3D`](crate::d3::UiPanel)s this
    /// frame and later (origin and direction, in the world), instead of the
    /// camera's ray through the mouse: for a locked cursor, use the camera's
    /// forward ray; for VR, a controller's. `None` goes back to the mouse.
    /// Only the screen's UI (`ctx.ui`) reads it.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// // Aim with the screen's center while the cursor is locked.
    /// let (origin, forward) = (ctx.scene3d.camera.position, ctx.scene3d.camera.forward());
    /// ctx.ui.set_pointer_ray(Some((origin, forward)));
    /// # }
    /// ```
    #[cfg(feature = "3d")]
    pub fn set_pointer_ray(&mut self, ray: Option<(glam::Vec3, glam::Vec3)>) {
        self.pointer_ray = ray;
    }
    /// Keyboard/gamepad belongs to UI while a screen is open (including text
    /// focus); a [HUD](Self::show_hud) doesn't count. Check it before the game's own movement and actions, just as
    /// [`wants_pointer`](Self::wants_pointer) guards the mouse.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(!ponko::ui::Ui::default().wants_keyboard());
    /// # fn update(ctx: &mut Ctx) {
    /// if !ctx.ui.wants_keyboard() && ctx.key_pressed(Key::Space) { /* jump */ }
    /// # }
    /// ```
    pub fn wants_keyboard(&self) -> bool {
        self.consumed || !self.screens.is_empty()
    }
    fn interactive(&self, id: UiId) -> bool {
        let mut at = Some(id);
        while let Some(id) = at {
            let Some(n) = self.node(id) else { return false };
            if !n.enabled {
                return false;
            }
            if Some(id) == self.top_screen() {
                return true;
            }
            at = n.parent;
        }
        false
    }
    pub(super) fn focusable(&self, id: UiId) -> bool {
        self.interactive(id)
            && self.node(id).is_some_and(|n| {
                matches!(
                    n.widget,
                    UiWidget::Button | UiWidget::TextInput | UiWidget::Toggle | UiWidget::Slider
                ) && n.rect.size().cmpgt(Vec2::ZERO).all()
            })
    }
    /// Hit test the top screen, respecting ancestor clips and disabled nodes.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::Ui::default().hit_test(Vec2::ZERO), None);
    /// ```
    pub fn hit_test(&self, point: Vec2) -> Option<UiId> {
        self.visible.iter().rev().copied().find(|&id| {
            self.interactive(id)
                && self.node(id).is_some_and(|n| {
                    n.rect.size().cmpgt(Vec2::ZERO).all()
                        && n.rect.contains(point)
                        && n.clip.contains(point)
                        && n.clip.size().cmpgt(Vec2::ZERO).all()
                })
        })
    }
    /// Resolve all screens against the logical window size. Percent sizes,
    /// min/max, flex/grid and absolute offsets are Taffy styles.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); ui.layout(vec2(800.0,600.0));
    /// ```
    pub fn layout(&mut self, viewport: Vec2) {
        self.viewport = viewport.max(Vec2::ZERO);
        self.visible.clear();
        // Intrinsic text measurement uses the engine's font layout, headlessly.
        for n in self.nodes.iter().flatten() {
            let _ = self.tree.mark_dirty(n.layout);
        }
        let nodes = &self.nodes;
        let text = &mut self.measure;
        let external_fonts = self.external_fonts;
        let roots = self.huds.iter().chain(self.screens.iter().map(|s| &s.root));
        for root in roots {
            let layout = nodes[root.0].as_ref().unwrap().layout;
            let _ = self.tree.compute_layout_with_measure(
                layout,
                Size {
                    width: AvailableSpace::Definite(viewport.x / self.scale),
                    height: AvailableSpace::Definite(viewport.y / self.scale),
                },
                |inputs, _, context, style| {
                    let Some(n) = context.and_then(|index| nodes[*index].as_ref()) else {
                        return taffy::tree::LayoutOutput::HIDDEN;
                    };
                    taffy::compute_leaf_layout(
                        inputs,
                        style,
                        |_, _| 0.0,
                        |known, available| {
                            let size = text.layout(
                                &n.text,
                                if external_fonts { n.paint.font } else { Font::DEFAULT },
                                n.paint.font_size,
                                false,
                            );
                            let font = if external_fonts { n.paint.font } else { Font::DEFAULT };
                            let width = known.width.unwrap_or_else(|| match available.width {
                                AvailableSpace::Definite(w) if n.paint.wrap => size.x.min(w.max(0.0)),
                                AvailableSpace::MinContent if n.paint.wrap => {
                                    text.min_content_width(&n.text, font, n.paint.font_size)
                                }
                                _ => size.x,
                            });
                            let mut height = 0.0;
                            for paragraph in n.text.split('\n') {
                                let mut rest = paragraph;
                                loop {
                                    let end = if n.paint.wrap {
                                        wrap_end(text, rest, font, n.paint.font_size, width)
                                    } else {
                                        rest.len()
                                    };
                                    height += text.layout(&rest[..end], font, n.paint.font_size, false).y;
                                    if end == rest.len() {
                                        break;
                                    }
                                    rest = rest[end..].trim_start();
                                }
                            }
                            if n.text.is_empty()
                                && !matches!(n.widget, UiWidget::TextInput | UiWidget::Button | UiWidget::Toggle)
                            {
                                height = 0.0;
                            }
                            Size {
                                width,
                                height: known.height.unwrap_or(height),
                            }
                        },
                    )
                },
            );
        }
        // HUDs first: `visible` is drawing order, so they are under the screens.
        for i in 0..self.huds.len() + self.screens.len() {
            let root = match i.checked_sub(self.huds.len()) {
                None => self.huds[i],
                Some(s) => self.screens[s].root,
            };
            self.place(
                root,
                self.root_offset(root),
                Rect::new(0.0, 0.0, viewport.x, viewport.y),
            );
        }
        if self.focus.is_some_and(|id| !self.focusable(id)) {
            self.focus = None;
        }
    }
    /// Where a screen root's `auto` margins move it: they take the window's
    /// free space, as they do for a child of a flex container (both sides
    /// `auto` centers it, one side pushes it to the other edge). Taffy gives
    /// a root's auto margins nothing, because a root has no container. Roots
    /// out of the flow are placed by their insets instead.
    fn root_offset(&self, root: UiId) -> Vec2 {
        let Some(node) = self.node(root) else { return Vec2::ZERO };
        let (Ok(style), Ok(layout)) = (self.tree.style(node.layout), self.tree.layout(node.layout)) else {
            return Vec2::ZERO;
        };
        if matches!(style.position, Position::Absolute | Position::Fixed) {
            return Vec2::ZERO;
        }
        let (margin, own) = (&style.margin, layout.margin);
        let share = |start: bool, end: bool, free: f32| match (start, end) {
            (true, true) => free * 0.5,
            (true, false) => free,
            _ => 0.0,
        };
        let free = self.viewport / self.scale
            - vec2(
                layout.size.width + own.left + own.right,
                layout.size.height + own.top + own.bottom,
            );
        let free = free.max(Vec2::ZERO);
        vec2(
            share(margin.left.is_auto(), margin.right.is_auto(), free.x),
            share(margin.top.is_auto(), margin.bottom.is_auto(), free.y),
        ) * self.scale
    }
    fn place(&mut self, id: UiId, origin: Vec2, clip: Rect) {
        let layout = *self.tree.layout(self.node(id).unwrap().layout).unwrap();
        let scale = self.scale;
        let pos = origin + vec2(layout.location.x, layout.location.y) * scale;
        let size = vec2(layout.size.width, layout.size.height) * scale;
        let rect = Rect {
            min: pos,
            max: pos + size,
        };
        let n = self.node_mut(id).unwrap();
        n.rect = rect;
        n.clip = clip;
        let scrollable = n.widget == UiWidget::Scroll;
        n.max_scroll = if scrollable {
            vec2(layout.scroll_width(), layout.scroll_height())
        } else {
            Vec2::ZERO
        };
        n.scroll = n.scroll.clamp(Vec2::ZERO, n.max_scroll);
        let scroll = n.scroll * scale;
        self.visible.push(id);
        let child_clip = if scrollable {
            let mut inner = rect;
            if self.node(id).unwrap().max_scroll.y > 0.0 {
                inner.max.x = (inner.max.x - 12.0 * scale).max(inner.min.x);
            }
            intersect(clip, inner)
        } else {
            clip
        };
        let count = self.node(id).unwrap().children.len();
        for i in 0..count {
            let child = self.node(id).unwrap().children[i];
            self.place(child, pos - scroll, child_clip);
        }
    }
    /// Layout and process one input snapshot. Call once per frame, before game
    /// logic reads events. Cancel consumes the frame, preventing click-through.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default(); ui.frame(vec2(800.0,600.0), &ponko::ui::UiInput::default());
    /// ```
    pub fn frame(&mut self, viewport: Vec2, input: &UiInput) {
        self.events.clear();
        self.consumed = false;
        self.pointer = input.pointer;
        self.layout(viewport);
        let access_moved_focus = self.apply_access_actions();
        if input.cancel {
            if let Some(root) = self.pop_screen() {
                self.events.push_back(UiEvent::ScreenPopped(root));
            }
            self.layout(viewport);
            return;
        }
        self.hovered = self.hit_test(input.pointer);
        if input.next || input.previous {
            self.tab(input.previous);
        }
        if self.focus.is_some_and(|id| !self.focusable(id)) {
            self.focus = None;
        }
        if input.pressed {
            self.pressed = self.hovered;
            self.drag = None;
            if let Some(id) = self.hovered {
                self.focus = self.focusable(id).then_some(id);
                let n = self.node(id).unwrap();
                if n.widget == UiWidget::TextInput {
                    let caret = self.caret_at(id, input.pointer);
                    let n = self.node_mut(id).unwrap();
                    n.caret = caret;
                    if !input.shift {
                        n.anchor = caret;
                    }
                    self.drag = Some(Drag::Text(id));
                } else if n.widget != UiWidget::Slider {
                    let mut at = Some(id);
                    while let Some(id) = at {
                        let n = self.node(id).unwrap();
                        if n.widget == UiWidget::Scroll {
                            self.drag = Some(if input.pointer.x >= n.rect.max.x - 12.0 * self.scale {
                                Drag::Bar(id, input.pointer.y, n.scroll.y)
                            } else {
                                Drag::Scroll(id, input.pointer, n.scroll)
                            });
                            break;
                        }
                        at = n.parent;
                    }
                }
            } else {
                self.focus = None;
            }
        }
        let mut at = self.hovered;
        while let Some(id) = at {
            let n = self.node(id).unwrap();
            if n.widget == UiWidget::Scroll {
                let next = n.scroll + input.wheel / self.scale;
                self.node_mut(id).unwrap().scroll = next;
                break;
            }
            at = n.parent;
        }
        if input.held {
            if let Some(drag) = self.drag {
                match drag {
                    Drag::Scroll(id, start, offset) => {
                        let scale = self.scale;
                        if let Some(n) = self.node_mut(id) {
                            n.scroll = offset - (input.pointer - start) / scale;
                        }
                        if (input.pointer - start).length() > 4.0 {
                            self.pressed = None;
                        }
                    }
                    Drag::Bar(id, start, offset) => {
                        let n = self.node(id).unwrap();
                        let h = n.rect.size().y;
                        let thumb = (h * h / (h + n.max_scroll.y * self.scale))
                            .max(20.0 * self.scale)
                            .min(h);
                        let track = (h - thumb).max(1.0);
                        let value = offset + (input.pointer.y - start) / track * n.max_scroll.y;
                        self.node_mut(id).unwrap().scroll.y = value;
                        self.pressed = None;
                    }
                    Drag::Text(id) => {
                        let caret = self.caret_at(id, input.pointer);
                        self.node_mut(id).unwrap().caret = caret;
                    }
                }
            }
            if let Some(id) = self
                .pressed
                .filter(|&id| self.node(id).is_some_and(|n| n.widget == UiWidget::Slider))
            {
                let n = self.node(id).unwrap();
                let value = ((input.pointer.x - n.rect.min.x) / n.rect.size().x.max(1.0)).clamp(0.0, 1.0);
                self.change_value(id, value);
            }
        }
        if input.released {
            if let Some(id) = self.pressed.take().filter(|id| Some(*id) == self.hovered)
                && self
                    .node(id)
                    .is_some_and(|n| matches!(n.widget, UiWidget::Button | UiWidget::Toggle))
            {
                self.activate(id);
            }
            self.drag = None;
        }
        let editing = self
            .focus
            .is_some_and(|id| self.node(id).unwrap().widget == UiWidget::TextInput);
        if editing && input.direction.y == 0.0 {
            self.edit(input);
        } else if let Some(id) = self
            .focus
            .filter(|&id| self.node(id).unwrap().widget == UiWidget::Slider)
            && input.direction.x != 0.0
        {
            let value = self.value(id).unwrap() + input.direction.x * super::access::SLIDER_STEP;
            self.change_value(id, value.clamp(0.0, 1.0));
        } else if input.direction != Vec2::ZERO {
            self.navigate(input.direction);
        }
        if input.confirm
            && let Some(id) = self.focus
        {
            self.activate(id);
        }
        self.layout(viewport);
        if input.next || input.previous || input.direction != Vec2::ZERO || access_moved_focus {
            self.reveal_focus();
            self.layout(viewport);
        }
    }
    fn reveal_focus(&mut self) {
        let Some(id) = self.focus else { return };
        let mut rect = self.node(id).unwrap().rect;
        let mut at = self.node(id).unwrap().parent;
        let scale = self.scale;
        while let Some(id) = at {
            let n = self.node_mut(id).unwrap();
            if n.widget == UiWidget::Scroll {
                let delta = (rect.max - n.rect.max).max(Vec2::ZERO) + (rect.min - n.rect.min).min(Vec2::ZERO);
                let old = n.scroll;
                n.scroll = (n.scroll + delta / scale).clamp(Vec2::ZERO, n.max_scroll);
                let moved = (n.scroll - old) * scale;
                rect.min -= moved;
                rect.max -= moved;
            }
            at = n.parent;
        }
    }
    fn tab(&mut self, back: bool) {
        let current = self.focus.and_then(|id| self.visible.iter().position(|&n| n == id));
        let len = self.visible.len();
        for step in 1..=len {
            let index = if back {
                (current.unwrap_or(0) + len - step % len) % len
            } else {
                current.map_or(step - 1, |i| (i + step) % len)
            };
            let id = self.visible[index];
            if self.focusable(id) {
                self.focus = Some(id);
                break;
            }
        }
    }
    fn navigate(&mut self, direction: Vec2) {
        let Some(from) = self.focus.and_then(|id| self.rect(id)) else {
            self.tab(false);
            return;
        };
        let mut best = None;
        let mut score = f32::INFINITY;
        for &id in &self.visible {
            if !self.focusable(id) || Some(id) == self.focus {
                continue;
            }
            let delta = self.rect(id).unwrap().center() - from.center();
            let along = delta.dot(direction);
            if along <= 0.0 {
                continue;
            }
            let cross = delta.perp_dot(direction).abs();
            let cost = along + cross * 4.0;
            if cost < score {
                best = Some(id);
                score = cost;
            }
        }
        if best.is_some() {
            self.focus = best;
        }
    }
    pub(super) fn activate(&mut self, id: UiId) {
        if !self.interactive(id) {
            return;
        }
        match self.node(id).unwrap().widget {
            UiWidget::Button => self.events.push_back(UiEvent::Clicked(id)),
            UiWidget::Toggle => {
                let v = 1.0 - self.value(id).unwrap();
                self.change_value(id, v);
            }
            UiWidget::TextInput => self.events.push_back(UiEvent::Submitted(id)),
            _ => {}
        }
    }
    pub(super) fn push_changed(&mut self, id: UiId) {
        self.events.push_back(UiEvent::Changed(id));
    }
    pub(super) fn change_value(&mut self, id: UiId, value: f32) {
        if self.value(id) != Some(value) {
            self.set_value(id, value);
            self.events.push_back(UiEvent::Changed(id));
        }
    }
    fn caret_at(&mut self, id: UiId, p: Vec2) -> usize {
        let n = self.nodes[id.0].as_ref().unwrap();
        let layout = self.tree.layout(n.layout).unwrap();
        let font = if self.external_fonts {
            n.paint.font
        } else {
            Font::DEFAULT
        };
        let x = (p.x - n.rect.min.x) / self.scale - layout.padding.left - layout.border.left;
        let caret_x = self.measure.caret_x(&n.text, font, n.paint.font_size, n.caret);
        let available = n.rect.size().x / self.scale - layout.padding.left - layout.padding.right;
        let target = x + (caret_x - available + 3.0).max(0.0);
        self.measure.hit(&n.text, font, n.paint.font_size, target)
    }
    fn edit(&mut self, input: &UiInput) {
        let id = self.focus.unwrap();
        let n = self.node_mut(id).unwrap();
        let mut changed = false;
        if input.select_all {
            n.anchor = 0;
            n.caret = n.text.len();
        }
        if input.home {
            n.caret = 0;
            if !input.shift {
                n.anchor = n.caret;
            }
        }
        if input.end {
            n.caret = n.text.len();
            if !input.shift {
                n.anchor = n.caret;
            }
        }
        if input.direction.x != 0.0 {
            n.caret = if input.direction.x < 0.0 {
                n.text[..n.caret].char_indices().next_back().map_or(0, |(i, _)| i)
            } else {
                n.text[n.caret..]
                    .chars()
                    .next()
                    .map_or(n.caret, |c| n.caret + c.len_utf8())
            };
            if !input.shift {
                n.anchor = n.caret;
            }
        }
        if input.backspace || input.delete || input.text.chars().any(|c| !c.is_control()) {
            let lo = n.caret.min(n.anchor);
            let hi = n.caret.max(n.anchor);
            if lo != hi {
                n.text.replace_range(lo..hi, "");
                n.caret = lo;
                changed = true;
            } else if input.backspace && n.caret > 0 {
                let prev = n.text[..n.caret].char_indices().next_back().unwrap().0;
                n.text.replace_range(prev..n.caret, "");
                n.caret = prev;
                changed = true;
            } else if input.delete && n.caret < n.text.len() {
                let end = n.caret + n.text[n.caret..].chars().next().unwrap().len_utf8();
                n.text.replace_range(n.caret..end, "");
                changed = true;
            }
            for ch in input.text.chars().filter(|c| !c.is_control()) {
                n.text.insert(n.caret, ch);
                n.caret += ch.len_utf8();
                changed = true;
            }
            n.anchor = n.caret;
        }
        if changed {
            self.events.push_back(UiEvent::Changed(id));
        }
    }
    pub(super) fn layout_with_text(&mut self, viewport: Vec2, text: &mut crate::text::Text) {
        std::mem::swap(&mut self.measure, text);
        self.external_fonts = true;
        self.layout(viewport);
        self.external_fonts = false;
        std::mem::swap(&mut self.measure, text);
    }
    pub(super) fn frame_with_text(&mut self, viewport: Vec2, input: &UiInput, text: &mut crate::text::Text) {
        std::mem::swap(&mut self.measure, text);
        self.external_fonts = true;
        self.frame(viewport, input);
        self.external_fonts = false;
        std::mem::swap(&mut self.measure, text);
    }
    pub(super) fn scale(&self) -> f32 {
        self.scale
    }
    pub(super) fn modal_root(&self, id: UiId) -> bool {
        self.screens.iter().any(|s| s.root == id && s.modal)
    }
}
pub(super) fn intersect(a: Rect, b: Rect) -> Rect {
    let min = a.min.max(b.min);
    Rect {
        min,
        max: a.max.min(b.max).max(min),
    }
}

pub(super) fn wrap_end(text: &mut crate::text::Text, line: &str, font: Font, size: f32, width: f32) -> usize {
    if let Some(end) = text.wrap_end_shaped_or_none(line, font, size, width) {
        return end;
    }
    let mut space = None;
    for (i, ch) in line.char_indices() {
        if ch.is_whitespace() {
            space = Some(i);
        }
        let next = i + ch.len_utf8();
        if text.layout(&line[..next], font, size, false).x > width {
            return space.filter(|&s| s > 0).unwrap_or(if i == 0 { next } else { i });
        }
    }
    line.len()
}
