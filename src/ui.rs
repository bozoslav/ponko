//! [`Ui`]: the screen's UI, as `ctx.ui`.
//!
//! **What lives here.** The retained widget tree with its screen stack,
//! focus and queued events ([`Ui`]); who has the pointer and the keyboard
//! ([`Ui::wants_pointer`], [`Ui::wants_keyboard`]); and the immediate
//! widgets for debug overlays, `ctx.ui.immediate` ([`UiImmediate`]), with
//! their [`UiStyle`]. A [`d3::UiPanel`](crate::d3::UiPanel) carries a tree of its own for a
//! surface in the 3D world.
//!
//! **When to call it.** Build the tree in setup, then change its text,
//! values and screens from any system. The engine lays it out and feeds it
//! input before the frame's systems, so events queued this frame are read
//! with [`Ui::events`] in a frame or simulation system, and it is drawn
//! above the scene after them. Immediate widgets are drawn the frame they
//! are called: call them every frame from a frame system.
//!
//! **What it needs.** Nothing to build and query: the tree lays itself out
//! and takes input without a window, so tests drive it headless. Drawing
//! needs a window. The immediate widgets are drawn through
//! [`ctx.immediate_ui()`](crate::ctx::Ctx::immediate_ui), which lends them the
//! graphics and input services, so `ctx.ui` holds none of them and borrows
//! apart from the world and the other services: read its events while
//! playing sounds or changing entities.
//!
//! ```
//! use ponko::prelude::*;
//!
//! fn menu(ctx: &mut Ctx) {
//!     let click = *ctx.world.resource::<Sound>();
//!     // `ctx.ui` and `ctx.audio` are separate fields.
//!     for event in ctx.ui.events() {
//!         if let ponko::ui::UiEvent::Clicked(_) = event {
//!             ctx.audio.play(click);
//!         }
//!     }
//! }
//!
//! Game::new("ui", 320, 240).sim_system(menu).run_headless(1, |ctx| {
//!     let click = ctx.audio.create(8000, &[0.0; 80]);
//!     ctx.world.insert_resource(click);
//!     let start = ctx.ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
//!     ctx.ui.push_screen(start, false);
//! });
//! ```
//!
//! Layout is delegated to [Taffy](https://docs.rs/taffy/0.15.1/taffy/), which
//! provides flexbox/grid without owning rendering or an event loop. The
//! engine's sprites and text render the widgets.
//! [Egui](https://github.com/emilk/egui) is easy to embed but
//! immediate-mode; it remains the inspector rather than the game UI.
//! [Xilem/Masonry](https://docs.rs/xilem/latest/xilem/) is retained but its
//! standard integration brings Vello rendering and winit window handling.
//! An owned tree plus Taffy keeps game textures, styling and input ownership
//! in one place while using a proven layout implementation.
//!
//! UI is drawn in window pixels on top of everything, built from sprites, so
//! it batches with everything else.

use crate::color::Color;

/// Colors and sizes of the immediate widgets:
/// [`ctx.ui.immediate.style`](UiImmediate::style). The screen's [`UiTheme`]
/// sets them together with the retained tree ([`Ui::set_theme`]), and its
/// [`Ui::set_scale`] scales them.
///
/// ```
/// # use ponko::prelude::*;
/// # fn setup(ctx: &mut Ctx) {
/// ctx.ui.immediate.style.accent = Color::hex(0x2e8b57);
/// ctx.ui.immediate.style.text_size = 18.0;
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiStyle {
    /// A panel's background.
    pub panel: Color,
    /// A button's background.
    pub button: Color,
    /// A button's background while the mouse is over it.
    pub button_hovered: Color,
    /// A button's background while held down.
    pub button_pressed: Color,
    /// Used for highlights, such as a button's outline while hovered.
    pub accent: Color,
    /// Text color.
    pub text: Color,
    /// Text size in UI units: pixels at the default [`Ui::set_scale`], and
    /// scaled with it.
    pub text_size: f32,
    /// Space between a panel's edge and its contents, in UI units (scaled
    /// like the text size).
    pub padding: f32,
}

impl Default for UiStyle {
    fn default() -> Self {
        Self {
            panel: Color::rgba(0.08, 0.09, 0.12, 0.85),
            button: Color::rgba(0.20, 0.22, 0.28, 1.0),
            button_hovered: Color::rgba(0.27, 0.30, 0.38, 1.0),
            button_pressed: Color::rgba(0.15, 0.16, 0.21, 1.0),
            accent: Color::hex(0xf28c28),
            text: Color::rgba(0.93, 0.93, 0.95, 1.0),
            text_size: 16.0,
            padding: 10.0,
        }
    }
}

/// Engine-side UI state that lives across frames.
#[derive(Default)]
pub(crate) struct UiState {
    pub(crate) retained_spare: Option<Ui>,
    pub(crate) retained_input_text: String,
    /// A click (and typed text) an input scenario asked for, applied by the
    /// retained UI's next frame.
    pub(crate) scripted: Option<retained_draw::ScriptedClick>,
    /// World-space panels' input and drawing.
    #[cfg(feature = "3d")]
    pub panels: panel3d::Panels,
}

mod access;
#[cfg(feature = "accessibility")]
mod access_kit;
#[cfg(feature = "accessibility")]
mod access_native;
mod build;
mod immediate;
#[cfg(feature = "3d")]
pub(crate) mod panel3d;
mod retained;
mod retained_draw;
pub use access::{
    UiAccessAction, UiAccessChanges, UiAccessNode, UiAccessTracker, UiAccessTree, UiAccessValue, UiLive, UiRole,
};
#[cfg(feature = "accessibility")]
pub use access_native::UiAccessAdapter;
pub use build::{UiBuilder, UiLayout, UiTheme};
pub use immediate::{UiImmediate, UiImmediateFrame};
pub use retained::{Ui, UiEvent, UiId, UiInput, UiPaint, UiTextAlign, UiWidget};

/// Unstable: the [Taffy](https://docs.rs/taffy/0.15.1/taffy/) layout crate
/// behind [`UiLayout`], for the settings its presets and refinements don't
/// cover ([`UiLayout::custom`], [`UiLayout::style_mut`]). Not covered by
/// ponko's stability promise: its types change when ponko upgrades Taffy.
/// Prefer [`UiLayout`]'s own methods.
pub use taffy as unstable_taffy;
