//! IME composition in the browser. Browsers send composition events only to
//! an editable element, and winit's web backend reads keys from the canvas,
//! so while IME is allowed the engine focuses a hidden `<textarea>` placed
//! at the caret (where the browser opens its candidate window) and reads
//! the composition, committed text and keys from it:
//!
//! - `compositionupdate`: the preedit, with its caret from the textarea's
//!   selection;
//! - `compositionend`: the commit (an empty one is a cancel);
//! - `input` outside a composition: typed text;
//! - `keydown`/`keyup` outside a composition: keys, as the canvas would
//!   have reported them.
//!
//! The textarea is emptied after every commit, so its value is always the
//! composition alone. Browsers differ in the order of the last `input` and
//! `compositionend`; the `input` of a composition is never taken as typed
//! text.

use crate::input::{ImeEvent, Input, Key};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{CompositionEvent, HtmlCanvasElement, HtmlTextAreaElement, InputEvent, KeyboardEvent};

/// What the textarea's listeners saw, taken at the start of a frame.
enum Queued {
    Ime(ImeEvent),
    Key(Key, bool),
    Text(String),
}

#[derive(Default)]
struct Shared {
    queue: Vec<Queued>,
    /// A composition is going on in the textarea.
    composing: bool,
    /// The composition was dropped from our side: its end commits nothing.
    discard: bool,
    /// A composition just ended: the `input` event some browsers send
    /// after `compositionend` is not typed text.
    just_composed: bool,
}

struct WebIme {
    textarea: HtmlTextAreaElement,
    canvas: HtmlCanvasElement,
    shared: Rc<RefCell<Shared>>,
    /// Kept alive as long as the textarea listens.
    _listeners: Vec<Closure<dyn FnMut(web_sys::Event)>>,
}

thread_local! {
    /// The browser runs the game on one thread; the textarea lives here.
    static WEB_IME: RefCell<Option<WebIme>> = const { RefCell::new(None) };
}

/// Start of a frame: hands what the textarea saw to `input`.
pub(crate) fn drain(input: &mut Input) {
    WEB_IME.with_borrow(|ime| {
        let Some(ime) = ime else { return };
        for queued in ime.shared.borrow_mut().queue.drain(..) {
            match queued {
                Queued::Ime(event) => input.feed_ime(event),
                Queued::Key(key, down) => input.set_key(key, down),
                Queued::Text(text) => input.add_text(&text),
            }
        }
    });
}

/// After a frame: focuses the textarea while IME is allowed, at the caret,
/// and blurs it (giving the keys back to the canvas) when it isn't.
pub(crate) fn apply(window: &winit::window::Window, allowed: bool, reset: bool, area: Option<crate::camera::Rect>) {
    use winit::platform::web::WindowExtWebSys;
    WEB_IME.with_borrow_mut(|slot| {
        if slot.is_none() {
            if !allowed {
                return;
            }
            let Some(canvas) = window.canvas() else { return };
            *slot = WebIme::new(canvas);
        }
        let Some(ime) = slot else { return };
        if !allowed {
            if ime.focused() {
                ime.shared.borrow_mut().discard |= ime.shared.borrow().composing;
                let _ = ime.textarea.blur();
                ime.textarea.set_value("");
                let _ = ime.canvas.focus();
            }
            return;
        }
        if reset {
            // Blurring ends the composition; most browsers commit it then,
            // which is dropped.
            ime.shared.borrow_mut().discard |= ime.shared.borrow().composing;
            let _ = ime.textarea.blur();
            ime.textarea.set_value("");
        }
        if let Some(area) = area {
            ime.place(area);
        }
        // Take the focus back after a click on the canvas; leave it with
        // anything else on the page.
        if !ime.focused() && ime.focus_is_ours() {
            let _ = ime.textarea.focus();
        }
    });
}

impl WebIme {
    fn new(canvas: HtmlCanvasElement) -> Option<Self> {
        let document = web_sys::window()?.document()?;
        let textarea: HtmlTextAreaElement = document.create_element("textarea").ok()?.dyn_into().ok()?;
        textarea.set_attribute("aria-hidden", "true").ok()?;
        textarea.set_attribute("autocomplete", "off").ok()?;
        textarea.set_attribute("autocorrect", "off").ok()?;
        textarea.set_attribute("autocapitalize", "off").ok()?;
        textarea.set_attribute("spellcheck", "false").ok()?;
        textarea.set_tab_index(-1);
        textarea
            .set_attribute(
                "style",
                "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;padding:0;border:0;margin:0;\
                 outline:none;resize:none;overflow:hidden;white-space:pre;background:transparent;\
                 color:transparent;caret-color:transparent;pointer-events:none;z-index:-1",
            )
            .ok()?;
        document.body()?.append_child(&textarea).ok()?;
        let shared = Rc::new(RefCell::new(Shared::default()));
        let mut ime = Self {
            textarea,
            canvas,
            shared,
            _listeners: Vec::new(),
        };
        ime.listen();
        Some(ime)
    }

    fn listen(&mut self) {
        let on = |me: &mut Self, name: &str, f: Box<dyn FnMut(web_sys::Event)>| {
            let closure = Closure::wrap(f);
            let _ = me
                .textarea
                .add_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
            me._listeners.push(closure);
        };
        let shared = self.shared.clone();
        on(
            self,
            "compositionstart",
            Box::new(move |_| {
                let mut s = shared.borrow_mut();
                s.composing = true;
                s.discard = false;
                s.queue.push(Queued::Ime(ImeEvent::Enabled));
            }),
        );
        let (shared, area) = (self.shared.clone(), self.textarea.clone());
        on(
            self,
            "compositionupdate",
            Box::new(move |event| {
                let Some(event) = event.dyn_ref::<CompositionEvent>() else {
                    return;
                };
                let text = event.data().unwrap_or_default();
                let cursor = selection_bytes(&area, &text);
                let mut s = shared.borrow_mut();
                if !s.discard {
                    s.queue.push(Queued::Ime(ImeEvent::Preedit { text, cursor }));
                }
            }),
        );
        let (shared, area) = (self.shared.clone(), self.textarea.clone());
        on(
            self,
            "compositionend",
            Box::new(move |event| {
                let text = event
                    .dyn_ref::<CompositionEvent>()
                    .and_then(CompositionEvent::data)
                    .unwrap_or_default();
                let mut s = shared.borrow_mut();
                let event = if std::mem::take(&mut s.discard) || text.is_empty() {
                    ImeEvent::cancel()
                } else {
                    ImeEvent::Commit(text)
                };
                s.queue.push(Queued::Ime(event));
                s.composing = false;
                s.just_composed = true;
                area.set_value("");
            }),
        );
        let (shared, area) = (self.shared.clone(), self.textarea.clone());
        on(
            self,
            "input",
            Box::new(move |event| {
                let Some(event) = event.dyn_ref::<InputEvent>() else {
                    return;
                };
                let mut s = shared.borrow_mut();
                if event.is_composing() || s.composing {
                    return;
                }
                let kind = event.input_type();
                let from_composition = std::mem::take(&mut s.just_composed)
                    || kind == "insertCompositionText"
                    || kind == "insertFromComposition";
                let value = area.value();
                if !from_composition && !value.is_empty() {
                    s.queue.push(Queued::Text(value));
                }
                area.set_value("");
            }),
        );
        for (name, down) in [("keydown", true), ("keyup", false)] {
            let shared = self.shared.clone();
            on(
                self,
                name,
                Box::new(move |event| {
                    let Some(event) = event.dyn_ref::<KeyboardEvent>() else {
                        return;
                    };
                    let mut s = shared.borrow_mut();
                    // 229: the key went to the IME.
                    if event.is_composing() || s.composing || event.key_code() == 229 {
                        return;
                    }
                    if down {
                        s.just_composed = false;
                    }
                    let Some(key) = Key::from_dom_code(&event.code()) else {
                        return;
                    };
                    // Tab would move the page's focus away; Enter and the
                    // arrows would edit the textarea.
                    if matches!(key, Key::Tab | Key::Enter | Key::Up | Key::Down) {
                        event.prevent_default();
                    }
                    if !(down && event.repeat()) {
                        s.queue.push(Queued::Key(key, down));
                    }
                }),
            );
        }
    }

    fn focused(&self) -> bool {
        let active = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.active_element());
        let textarea: &web_sys::Element = self.textarea.as_ref();
        active.is_some_and(|a| &a == textarea)
    }

    /// Whether the page's focus is on the game (the canvas, or nothing).
    fn focus_is_ours(&self) -> bool {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return false;
        };
        match document.active_element() {
            None => true,
            Some(active) => {
                let canvas: &web_sys::Element = self.canvas.as_ref();
                &active == canvas
                    || document
                        .body()
                        .is_some_and(|b| &active == AsRef::<web_sys::Element>::as_ref(&b))
            }
        }
    }

    /// Puts the textarea over the caret (logical units are CSS pixels).
    fn place(&self, area: crate::camera::Rect) {
        let canvas = self.canvas.get_bounding_client_rect();
        let style = self.textarea.style();
        let size = area.size();
        let _ = style.set_property("left", &format!("{}px", canvas.left() + f64::from(area.min.x)));
        let _ = style.set_property("top", &format!("{}px", canvas.top() + f64::from(area.min.y)));
        let _ = style.set_property("height", &format!("{}px", size.y.max(1.0)));
        let _ = style.set_property("font-size", &format!("{}px", size.y.max(1.0)));
    }
}

/// The textarea's selection, which is the composition's caret, as bytes of
/// `text` (the browser counts UTF-16 units).
fn selection_bytes(area: &HtmlTextAreaElement, text: &str) -> Option<(usize, usize)> {
    let start = area.selection_start().ok().flatten()? as usize;
    let end = area.selection_end().ok().flatten()? as usize;
    let byte = |units| crate::input::ime::utf16_to_byte(text, units);
    Some((byte(start.min(end)), byte(end.max(start))))
}
