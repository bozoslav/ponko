//! IME composition in the retained UI's text inputs: which field shows the
//! preedit, where its caret is for the candidate window, and when a focus
//! change interrupts a composition.

use super::retained::{Node, Ui, UiId, UiInput, UiWidget};
use crate::{camera::Rect, text::Font};
use glam::Vec2;

/// What the retained UI keeps about the composition between frames.
#[derive(Debug, Default)]
pub(super) struct UiImeState {
    /// The text input that shows the preedit.
    pub(super) composing: Option<UiId>,
    /// The focus moved away from `composing` during the last frame.
    pub(super) interrupted: bool,
    /// The focused text input's caret, for the candidate window.
    pub(super) area: Option<Rect>,
    /// Reused for the shown text while measuring.
    scratch: String,
}

/// A text input's text as it is shown: the preedit spliced in at the caret.
pub(super) struct Shown {
    /// Bytes of the shown text the preedit takes (empty without one).
    pub(super) preedit: std::ops::Range<usize>,
    /// The caret in the shown text, `None` when the IME hides it.
    pub(super) caret: Option<usize>,
}

impl Node {
    /// Writes the text this input shows into `out` (the preedit at the
    /// caret, replacing nothing yet) and says where the preedit and caret
    /// are in it. Without a preedit, `out` is the text.
    pub(super) fn shown_text(&self, out: &mut String) -> Shown {
        out.clear();
        if self.preedit.is_empty() {
            out.push_str(&self.text);
            return Shown {
                preedit: self.caret..self.caret,
                caret: Some(self.caret),
            };
        }
        out.push_str(&self.text[..self.caret]);
        out.push_str(&self.preedit);
        out.push_str(&self.text[self.caret..]);
        Shown {
            preedit: self.caret..self.caret + self.preedit.len(),
            caret: self.preedit_cursor.map(|(start, _)| self.caret + start),
        }
    }
}

impl Ui {
    /// Lets the text input `id` take IME composition (`true`, the default)
    /// or not (`false`, for a field of digits, a key binding or a password).
    /// While a text input that takes IME has the focus, the engine allows
    /// IME for the window and puts the candidate window at its caret.
    /// ```
    /// use ponko::ui::{Ui, UiLayout, UiWidget};
    /// let mut ui = Ui::default();
    /// let code = ui.add(UiWidget::TextInput, UiLayout::auto());
    /// ui.set_ime_allowed(code, false);
    /// assert!(!ui.ime_allowed(code));
    /// ```
    pub fn set_ime_allowed(&mut self, id: UiId, allowed: bool) {
        if let Some(n) = self.node_mut(id) {
            n.ime = allowed;
            if !allowed {
                n.preedit.clear();
                n.preedit_cursor = None;
            }
        }
    }

    /// Whether `id` takes IME composition (see [`set_ime_allowed`](Self::set_ime_allowed)).
    /// `false` for a removed node.
    #[must_use]
    pub fn ime_allowed(&self, id: UiId) -> bool {
        self.node(id).is_some_and(|n| n.ime)
    }

    /// Whether the focus is on a text input that takes IME, so the window
    /// should let the player compose text.
    /// ```
    /// use ponko::ui::{Ui, UiLayout, UiWidget};
    /// let mut ui = Ui::default();
    /// let root = ui.add(UiWidget::Panel, UiLayout::auto());
    /// let name = ui.add_child(root, UiWidget::TextInput, UiLayout::auto());
    /// ui.push_screen(root, false);
    /// assert!(!ui.wants_ime());
    /// ui.set_focus(name);
    /// assert!(ui.wants_ime());
    /// ```
    #[must_use]
    pub fn wants_ime(&self) -> bool {
        self.ime_target().is_some()
    }

    /// The preedit the text input `id` shows at its caret, when the player
    /// is composing text in it with an IME.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::ui::{Ui, UiInput, UiLayout, UiWidget};
    /// let mut ui = Ui::default();
    /// let root = ui.add(UiWidget::Panel, UiLayout::auto());
    /// let name = ui.add_child(root, UiWidget::TextInput, UiLayout::fixed(200.0, 30.0));
    /// ui.push_screen(root, false);
    /// ui.set_focus(name);
    /// let input = UiInput { preedit: "にほ".into(), composing: true, ..Default::default() };
    /// ui.frame(vec2(800.0, 600.0), &input);
    /// assert_eq!(ui.preedit(name), Some("にほ"));
    /// assert_eq!(ui.text(name), Some(""), "not committed yet");
    /// ```
    #[must_use]
    pub fn preedit(&self, id: UiId) -> Option<&str> {
        self.node(id).map(|n| n.preedit.as_str()).filter(|p| !p.is_empty())
    }

    /// The caret of the focused text input that takes IME, in logical
    /// window units, as of the last [`frame`](Self::frame): where the
    /// window system should put its candidate window. With a preedit it is
    /// the preedit's own caret.
    #[must_use]
    pub fn ime_area(&self) -> Option<Rect> {
        self.ime.area
    }

    /// Whether the last [`frame`](Self::frame) took the focus away from a
    /// text input in the middle of a composition (a click or Tab elsewhere,
    /// or [`set_focus`](Self::set_focus) between frames). The composition is
    /// dropped, not committed into another field; the host then tells the
    /// window system to drop it too (the engine calls
    /// [`Input::cancel_composition`](crate::input::Input::cancel_composition)).
    #[must_use]
    pub fn composition_interrupted(&self) -> bool {
        self.ime.interrupted
    }

    /// The focused text input that takes IME.
    fn ime_target(&self) -> Option<UiId> {
        self.focus.filter(|&id| {
            self.node(id)
                .is_some_and(|n| n.widget == UiWidget::TextInput && n.ime && n.enabled)
        })
    }

    /// Whether the IME owns this frame's editing keys: a composition is
    /// going on in the focused text input.
    pub(super) fn ime_owns_keys(&self, input: &UiInput) -> bool {
        input.composing && self.ime_target().is_some() && self.ime.composing.is_none_or(|c| Some(c) == self.focus)
    }

    /// End of a frame: the focused text input that takes IME shows
    /// `input.preedit`; the field that showed it before stops. Called after
    /// focus changes were applied.
    pub(super) fn apply_composition(&mut self, input: &UiInput) {
        let before = self.ime.composing.take();
        let target = self.ime_target();
        self.ime.interrupted = before.is_some() && before != target;
        if let Some(old) = before.filter(|&old| Some(old) != target)
            && let Some(n) = self.node_mut(old)
        {
            n.preedit.clear();
            n.preedit_cursor = None;
        }
        if let Some(id) = target
            && !self.ime.interrupted
        {
            let n = self.node_mut(id).unwrap();
            n.preedit.clone_from(&input.preedit);
            n.preedit_cursor = input.preedit_cursor.filter(|&(a, b)| {
                a <= b && b <= n.preedit.len() && n.preedit.is_char_boundary(a) && n.preedit.is_char_boundary(b)
            });
            if !n.preedit.is_empty() {
                self.ime.composing = Some(id);
            }
        }
        self.ime.area = target.map(|id| self.caret_area(id));
    }

    /// The caret of the text input `id` as it is drawn, in window units.
    fn caret_area(&mut self, id: UiId) -> Rect {
        let n = self.nodes[id.0].as_ref().unwrap();
        let layout = self.tree.layout(n.layout).unwrap();
        let font = if self.external_fonts {
            n.paint.font
        } else {
            Font::DEFAULT
        };
        let size = n.paint.font_size;
        let mut shown = std::mem::take(&mut self.ime.scratch);
        let at = n.shown_text(&mut shown);
        let caret = at.caret.unwrap_or(at.preedit.end);
        let inset = Vec2::new(
            layout.padding.left + layout.border.left,
            layout.padding.top + layout.border.top,
        );
        let available = n.rect.size().x / self.scale - inset.x - layout.padding.right - layout.border.right;
        let rect_min = n.rect.min;
        // Scrolled as drawn: the caret stays inside the field.
        let caret_x = self.measure.caret_x(&shown, font, size, caret);
        let x = caret_x - (caret_x - available + 3.0).max(0.0);
        self.ime.scratch = shown;
        let scale = self.scale;
        Rect::new(
            rect_min.x + (inset.x + x) * scale,
            rect_min.y + inset.y * scale,
            scale,
            size * scale,
        )
    }
}
