//! Input method editor (IME) composition: the text a player is composing
//! for Chinese, Japanese, Korean and other scripts before it is committed,
//! and the requests that tell the window system where the caret is.
//!
//! The window system reports a composition as [`ImeEvent`]s. While the
//! player composes, [`Input::preedit`] holds the text so far, which a text
//! field shows at its caret, underlined. When they pick a candidate, the
//! committed text arrives in [`Input::typed_text`] like any typed text, and
//! the preedit is cleared. An empty preedit without a commit is a cancel.
//!
//! The window system only composes while IME is allowed
//! ([`Input::set_ime_allowed`]); the retained UI allows it while a text
//! input that takes IME has the focus and places the candidate window at its
//! caret. Games with their own text widgets do the same with
//! [`Input::set_ime_allowed`] and [`Input::set_ime_cursor_area`].

use super::Input;
use crate::camera::Rect;

/// One step of an IME composition, as the window system reports it. Feed
/// them with [`Input::feed_ime`] from a custom host, a test or a replay;
/// input scenarios replay them with
/// [`InputScenario::ime`](crate::scenario::InputScenario::ime).
///
/// A Japanese word, composed and committed:
/// ```
/// use ponko::prelude::*;
/// use ponko::input::ImeEvent;
///
/// Game::new("t", 1, 1).run_headless(1, |ctx| {
///     ctx.input.feed_ime(ImeEvent::Enabled);
///     ctx.input.feed_ime(ImeEvent::preedit("にほん", Some((9, 9))));
///     assert_eq!(ctx.input.preedit(), "にほん");
///     ctx.input.feed_ime(ImeEvent::Commit("日本".into()));
///     assert_eq!(ctx.input.preedit(), "");
///     assert_eq!(ctx.input.typed_text(), "日本");
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ImeEvent {
    /// The window system turned IME on for the window; compositions may follow.
    Enabled,
    /// The composition so far. An empty `text` ends the composition
    /// without committing anything (the player cancelled it).
    Preedit {
        /// The text being composed, shown at the caret.
        text: String,
        /// The composition's own caret, as a byte range of `text`
        /// (`start == end`: a plain caret; otherwise the part being
        /// converted). `None`: hide the caret.
        cursor: Option<(usize, usize)>,
    },
    /// The composition ended with this text, which is added to
    /// [`Input::typed_text`]. The preedit is cleared.
    Commit(String),
    /// The window system turned IME off; a composition in progress is dropped.
    Disabled,
}

impl ImeEvent {
    /// A [`Preedit`](Self::Preedit) event.
    /// ```
    /// let event = ponko::input::ImeEvent::preedit("ni", Some((2, 2)));
    /// ```
    #[must_use]
    pub fn preedit(text: impl Into<String>, cursor: Option<(usize, usize)>) -> Self {
        Self::Preedit {
            text: text.into(),
            cursor,
        }
    }

    /// An empty [`Preedit`](Self::Preedit): the player cancelled the composition.
    /// ```
    /// assert_eq!(ponko::input::ImeEvent::cancel(), ponko::input::ImeEvent::preedit("", None));
    /// ```
    #[must_use]
    pub fn cancel() -> Self {
        Self::preedit("", None)
    }
}

/// IME state kept by [`Input`].
#[derive(Debug, Clone, Default)]
pub(crate) struct ImeState {
    /// The composition so far; empty when nothing is being composed.
    preedit: String,
    preedit_cursor: Option<(usize, usize)>,
    /// A composition was going on at some point this frame (also one that
    /// was committed or cancelled during it).
    composed_this_frame: bool,
    /// The game or UI wants the window system to compose text.
    allowed: bool,
    /// Where the window system should put its candidate window.
    area: Option<Rect>,
    /// `allowed` changed since the window last applied it.
    pub(crate) allowed_changed: bool,
    /// `area` changed since the window last applied it.
    pub(crate) area_changed: bool,
    /// The composition was cancelled from our side: the window should make
    /// the window system drop it too.
    pub(crate) reset: bool,
}

impl ImeState {
    pub(crate) fn end_frame(&mut self) {
        self.composed_this_frame = !self.preedit.is_empty();
    }
}

impl Input {
    /// Records an IME event from the window system, a custom host, a test
    /// or a replay. A commit adds its text to [`typed_text`](Self::typed_text).
    /// A preedit cursor that is out of range or not on a character
    /// boundary is dropped (the caret is hidden).
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::input::ImeEvent;
    /// Game::new("t", 1, 1).run_headless(1, |ctx| {
    ///     ctx.input.feed_ime(ImeEvent::preedit("ㅎ", Some((3, 3))));
    ///     ctx.input.feed_ime(ImeEvent::preedit("한", Some((3, 3))));
    ///     ctx.input.feed_ime(ImeEvent::cancel());
    ///     assert!(ctx.input.preedit().is_empty());
    ///     assert!(ctx.input.typed_text().is_empty());
    /// });
    /// ```
    pub fn feed_ime(&mut self, event: ImeEvent) {
        let ime = &mut self.ime;
        match event {
            ImeEvent::Enabled => {}
            ImeEvent::Preedit { text, cursor } => {
                let valid = |i: usize| i <= text.len() && text.is_char_boundary(i);
                ime.preedit_cursor = cursor.filter(|&(a, b)| a <= b && valid(a) && valid(b));
                ime.preedit = text;
                ime.composed_this_frame |= !ime.preedit.is_empty();
            }
            ImeEvent::Commit(text) => {
                ime.preedit.clear();
                ime.preedit_cursor = None;
                ime.composed_this_frame = true;
                self.add_text(&text);
            }
            ImeEvent::Disabled => {
                ime.preedit.clear();
                ime.preedit_cursor = None;
            }
        }
    }

    /// The text being composed with an IME, not yet committed; empty when
    /// nothing is. It stays until the composition changes, so it is not a
    /// per-frame edge like [`typed_text`](Self::typed_text). Show it at the
    /// caret of the focused text widget, underlined.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// let composing = ctx.input.preedit();
    /// # let _ = composing;
    /// # }
    /// ```
    #[must_use]
    pub fn preedit(&self) -> &str {
        &self.ime.preedit
    }

    /// The caret inside [`preedit`](Self::preedit), as a byte range of it:
    /// `start == end` is a plain caret, otherwise the part the IME is
    /// converting. `None` when there is no composition or the IME hides the caret.
    #[must_use]
    pub fn preedit_cursor(&self) -> Option<(usize, usize)> {
        self.ime.preedit_cursor
    }

    /// Whether an IME composition was going on at some point this frame,
    /// including one committed or cancelled during it. While it is, keys such
    /// as Enter, Escape, Backspace and the arrows belong to the IME: a text
    /// widget should not also act on them.
    #[must_use]
    pub fn composing(&self) -> bool {
        self.ime.composed_this_frame || !self.ime.preedit.is_empty()
    }

    /// Lets the window system compose text with an IME (`true`) or not.
    /// Allow it while a text widget has the focus, and not otherwise: with
    /// IME allowed, some keyboard layouts turn keys into compositions instead
    /// of key presses. The retained UI does this for its text inputs.
    /// Applied after the frame; headless runs only remember it. Turning it
    /// off drops a composition in progress.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, chat_open: bool) {
    /// ctx.input.set_ime_allowed(chat_open);
    /// # }
    /// ```
    pub fn set_ime_allowed(&mut self, allowed: bool) {
        if self.ime.allowed != allowed {
            self.ime.allowed = allowed;
            self.ime.allowed_changed = true;
            if !allowed {
                self.ime.preedit.clear();
                self.ime.preedit_cursor = None;
            }
        }
    }

    /// Whether IME is allowed, as set with [`set_ime_allowed`](Self::set_ime_allowed).
    #[must_use]
    pub fn ime_allowed(&self) -> bool {
        self.ime.allowed
    }

    /// Where the caret of the focused text widget is, in logical window
    /// units, so the window system puts its candidate window next to it
    /// (below it, usually). Applied after the frame when it changed.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// ctx.input.set_ime_cursor_area(Rect::new(120.0, 40.0, 2.0, 18.0));
    /// assert_eq!(ctx.input.ime_cursor_area(), Some(Rect::new(120.0, 40.0, 2.0, 18.0)));
    /// # }
    /// ```
    pub fn set_ime_cursor_area(&mut self, area: Rect) {
        if self.ime.area != Some(area) {
            self.ime.area = Some(area);
            self.ime.area_changed = true;
        }
    }

    /// The caret area set with [`set_ime_cursor_area`](Self::set_ime_cursor_area).
    #[must_use]
    pub fn ime_cursor_area(&self) -> Option<Rect> {
        self.ime.area
    }

    /// Drops the composition in progress: the preedit is cleared now and the
    /// window system is told to forget it after the frame, so nothing of it
    /// is committed later. For a text widget that loses the focus while the
    /// player composes (the retained UI does this itself).
    /// ```
    /// use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(1, |ctx| {
    ///     ctx.input.feed_ime(ponko::input::ImeEvent::preedit("zhong", None));
    ///     ctx.input.cancel_composition();
    ///     assert_eq!(ctx.input.preedit(), "");
    /// });
    /// ```
    pub fn cancel_composition(&mut self) {
        if !self.ime.preedit.is_empty() {
            self.ime.preedit.clear();
            self.ime.preedit_cursor = None;
            self.ime.reset = true;
        }
    }

    /// Whether [`cancel_composition`](Self::cancel_composition) asked the
    /// window system to drop a composition this frame. Engine side, and for
    /// tests: the window clears it after acting on it.
    #[cfg(test)]
    pub(crate) fn ime_reset_requested(&self) -> bool {
        self.ime.reset
    }
}

/// The byte offset in `text` of a position counted in UTF-16 units (how
/// browsers count), rounded up to the next character; past the end, the end.
#[cfg_attr(not(all(target_arch = "wasm32", feature = "window")), allow(dead_code))]
pub(crate) fn utf16_to_byte(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, ch) in text.char_indices() {
        if seen >= units {
            return i;
        }
        seen += ch.len_utf16();
    }
    text.len()
}

#[cfg(feature = "window")]
impl ImeEvent {
    pub(crate) fn from_winit(event: winit::event::Ime) -> Self {
        use winit::event::Ime;
        match event {
            Ime::Enabled => Self::Enabled,
            Ime::Preedit(text, cursor) => Self::Preedit { text, cursor },
            Ime::Commit(text) => Self::Commit(text),
            Ime::Disabled => Self::Disabled,
        }
    }
}

#[cfg(feature = "window")]
impl Input {
    /// After the frame: hands what changed in the IME requests to the window.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn apply_ime(&mut self, window: &winit::window::Window) {
        use winit::dpi::{LogicalPosition, LogicalSize};
        let ime = &mut self.ime;
        if std::mem::take(&mut ime.allowed_changed) {
            window.set_ime_allowed(ime.allowed);
            ime.area_changed |= ime.allowed;
        }
        // Turning IME off and on again makes the window system drop the
        // composition (on macOS it discards the marked text, on Windows and
        // Linux it resets the input context).
        if std::mem::take(&mut ime.reset) && ime.allowed {
            window.set_ime_allowed(false);
            window.set_ime_allowed(true);
            ime.area_changed = true;
        }
        if ime.allowed
            && std::mem::take(&mut ime.area_changed)
            && let Some(area) = ime.area
        {
            let size = area.size();
            window.set_ime_cursor_area(
                LogicalPosition::new(area.min.x, area.min.y),
                LogicalSize::new(size.x.max(1.0), size.y.max(1.0)),
            );
        }
    }

    /// After the frame, in a browser: winit's web backend has no IME, so the
    /// engine composes in a hidden text box of its own at the caret.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn apply_ime(&mut self, window: &winit::window::Window) {
        let ime = &mut self.ime;
        ime.allowed_changed = false;
        let area = ime.area.filter(|_| std::mem::take(&mut ime.area_changed));
        crate::web::ime::apply(window, ime.allowed, std::mem::take(&mut ime.reset), area);
    }

    /// Something else (the inspector's own text fields) may have changed the
    /// window's IME: hand the whole state to it again after the frame.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub(crate) fn reapply_ime(&mut self) {
        self.ime.allowed_changed = true;
        self.ime.area_changed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_selection_offsets_map_to_byte_offsets() {
        // Kana take one UTF-16 unit and three bytes; the emoji two and four.
        assert_eq!(utf16_to_byte("にほん", 0), 0);
        assert_eq!(utf16_to_byte("にほん", 2), 6);
        assert_eq!(utf16_to_byte("にほん", 3), 9);
        assert_eq!(utf16_to_byte("a😀b", 3), 5);
        assert_eq!(utf16_to_byte("a😀b", 2), 5, "inside a pair: the next character");
        assert_eq!(utf16_to_byte("ab", 10), 2);
    }

    #[test]
    fn commit_clears_preedit_and_types_the_text() {
        let mut input = Input::new();
        input.feed_ime(ImeEvent::preedit("にほ", Some((6, 6))));
        assert_eq!(input.preedit(), "にほ");
        assert_eq!(input.preedit_cursor(), Some((6, 6)));
        input.feed_ime(ImeEvent::Commit("日本".into()));
        assert_eq!(input.preedit(), "");
        assert_eq!(input.preedit_cursor(), None);
        assert_eq!(input.typed_text(), "日本");
        assert!(input.composing(), "the frame of the commit still counts as composing");
        input.end_frame();
        assert!(!input.composing());
        assert_eq!(input.typed_text(), "");
    }

    #[test]
    fn preedit_lasts_across_frames_until_it_changes() {
        let mut input = Input::new();
        input.feed_ime(ImeEvent::preedit("zhong", None));
        input.end_frame();
        input.end_frame();
        assert_eq!(input.preedit(), "zhong");
        assert!(input.composing());
    }

    #[test]
    fn preedit_cursor_off_a_char_boundary_is_dropped() {
        let mut input = Input::new();
        input.feed_ime(ImeEvent::preedit("한", Some((1, 1))));
        assert_eq!(input.preedit_cursor(), None);
        input.feed_ime(ImeEvent::preedit("한", Some((0, 9))));
        assert_eq!(input.preedit_cursor(), None);
        input.feed_ime(ImeEvent::preedit("한", Some((0, 3))));
        assert_eq!(input.preedit_cursor(), Some((0, 3)));
    }

    #[test]
    fn disabling_ime_drops_the_composition() {
        let mut input = Input::new();
        input.set_ime_allowed(true);
        input.feed_ime(ImeEvent::preedit("ni", None));
        input.set_ime_allowed(false);
        assert_eq!(input.preedit(), "");
        input.set_ime_allowed(true);
        input.feed_ime(ImeEvent::preedit("ni", None));
        input.feed_ime(ImeEvent::Disabled);
        assert_eq!(input.preedit(), "");
        assert_eq!(input.typed_text(), "");
    }

    #[test]
    fn requests_are_flagged_only_when_they_change() {
        let mut input = Input::new();
        input.set_ime_allowed(false);
        assert!(!input.ime.allowed_changed);
        input.set_ime_allowed(true);
        assert!(input.ime.allowed_changed);
        let area = Rect::new(1.0, 2.0, 3.0, 4.0);
        input.set_ime_cursor_area(area);
        assert!(input.ime.area_changed);
        input.ime.area_changed = false;
        input.set_ime_cursor_area(area);
        assert!(!input.ime.area_changed);
        input.cancel_composition();
        assert!(!input.ime_reset_requested(), "nothing to cancel");
        input.feed_ime(ImeEvent::preedit("a", None));
        input.cancel_composition();
        assert!(input.ime_reset_requested());
    }
}
