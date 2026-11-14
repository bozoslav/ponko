//! Accessibility semantics for the retained UI: a read-only snapshot of roles,
//! names, values, states and focus, built from the existing node hierarchy.
//!
//! This is the platform-neutral half of screen-reader support: tests and
//! tools read [`UiAccessTree`] directly, without a window. A
//! [`UiAccessTracker`] says what changed since it last looked, so an adapter
//! sends only that, and [`UiAccessAction`] is what assistive technology asks
//! the UI to do. The native adapter (`UiAccessAdapter`, feature
//! `accessibility`) is built from these.

use super::retained::Ui;
use super::{UiId, UiWidget};
use crate::camera::Rect;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use taffy::prelude::Display;

/// What a node is to assistive technology.
/// ```
/// use ponko::prelude::*;
/// assert_eq!(ponko::ui::UiRole::of(ponko::ui::UiWidget::Toggle), ponko::ui::UiRole::CheckBox);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiRole {
    /// A container: a panel or a non-modal screen root.
    Group,
    /// The root of a modal screen.
    Dialog,
    /// Static text.
    Label,
    /// Activates on press or confirm.
    Button,
    /// A picture; give it a name unless it is decorative.
    Image,
    /// A scrolling container.
    ScrollView,
    /// Single-line editable text; its text is the value, not the name.
    TextInput,
    /// An on/off toggle.
    CheckBox,
    /// A value from 0 to 1.
    Slider,
}

impl UiRole {
    /// The role a widget kind has outside a modal screen root.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::UiRole::of(ponko::ui::UiWidget::Scroll), ponko::ui::UiRole::ScrollView);
    /// ```
    pub const fn of(widget: UiWidget) -> Self {
        match widget {
            UiWidget::Panel => Self::Group,
            UiWidget::Label => Self::Label,
            UiWidget::Button => Self::Button,
            UiWidget::Image => Self::Image,
            UiWidget::Scroll => Self::ScrollView,
            UiWidget::TextInput => Self::TextInput,
            UiWidget::Toggle => Self::CheckBox,
            UiWidget::Slider => Self::Slider,
        }
    }
    /// Lower-case name used by the snapshot text.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::UiRole::TextInput.as_str(), "text-input");
    /// ```
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Dialog => "dialog",
            Self::Label => "label",
            Self::Button => "button",
            Self::Image => "image",
            Self::ScrollView => "scroll-view",
            Self::TextInput => "text-input",
            Self::CheckBox => "checkbox",
            Self::Slider => "slider",
        }
    }
}

/// A node's current value, by role.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().id();
/// let music = ui.toggle(root, "Music").value(1.0).id();
/// ui.push_screen(root, false);
/// ui.layout(vec2(800.0, 600.0));
/// assert_eq!(ui.accessibility_tree().get(music).unwrap().value, ponko::ui::UiAccessValue::Checked(true));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum UiAccessValue {
    /// No value (groups, labels, buttons, images).
    None,
    /// A text input's text.
    Text(String),
    /// A toggle's state.
    Checked(bool),
    /// A slider's value, from 0 to 1; keyboard and gamepad steps are 0.05.
    Range(f32),
}

/// Whether a node's changes are announced while something else has focus:
/// a score that went up, an error under a form.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().id();
/// let status = ui.label(root, "Saved").id();
/// ui.set_live_region(status, ponko::ui::UiLive::Polite);
/// assert_eq!(ui.live_region(status), Some(ponko::ui::UiLive::Polite));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UiLive {
    /// Not announced on its own (the default).
    #[default]
    Off,
    /// Announced when the screen reader is next idle.
    Polite,
    /// Announced at once, interrupting speech.
    Assertive,
}

impl UiLive {
    /// Lower-case name used by the snapshot text.
    /// ```
    /// assert_eq!(ponko::ui::UiLive::Assertive.as_str(), "assertive");
    /// ```
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Polite => "polite",
            Self::Assertive => "assertive",
        }
    }
}

/// One exposed node of a [`UiAccessTree`].
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().id();
/// let play = ui.button(root, "Play").id();
/// ui.push_screen(root, false);
/// ui.layout(vec2(800.0, 600.0));
/// let tree = ui.accessibility_tree();
/// let node = tree.get(play).unwrap();
/// assert_eq!((node.role, node.name.as_str(), node.parent), (ponko::ui::UiRole::Button, "Play", Some(root)));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct UiAccessNode {
    /// The UI node.
    pub id: UiId,
    /// Exposed parent; `None` for a screen root.
    pub parent: Option<UiId>,
    /// Exposed children, in layout order.
    pub children: Vec<UiId>,
    /// Nesting below the screen root (which is 0).
    pub depth: usize,
    /// What the node is.
    pub role: UiRole,
    /// Accessible name: the override from [`Ui::set_accessible_name`], else
    /// the text (except for text inputs, whose text is the value).
    pub name: String,
    /// Read after the name: what the control does or a hint
    /// ([`Ui::set_accessible_description`]). Empty: none.
    pub description: String,
    /// Current value.
    pub value: UiAccessValue,
    /// A text input's selection as `(anchor, caret)` byte offsets into its
    /// text; equal when nothing is selected. `None` for other widgets.
    pub selection: Option<(usize, usize)>,
    /// Whether changes are announced without focus ([`Ui::set_live_region`]).
    pub live: UiLive,
    /// Bounds in logical window coordinates, including the user scale.
    pub rect: Rect,
    /// Keyboard/gamepad navigation can focus it right now.
    pub focusable: bool,
    /// It has keyboard/gamepad focus.
    pub focused: bool,
    /// It or an ancestor is disabled.
    pub disabled: bool,
    /// It is on a screen below the top one: readable, but not interactive.
    pub inert: bool,
    /// It is the root of a modal screen.
    pub modal: bool,
}

/// A deterministic snapshot of the UI's accessibility semantics.
///
/// Nodes are in depth-first layout order, screens from the bottom up, after
/// the [HUDs](Ui::show_hud). A modal screen hides every screen and HUD below
/// it; non-modal lower screens and HUDs stay readable but
/// [`inert`](UiAccessNode::inert). Nodes with `display: none` (and their
/// subtrees) are left out. Take the snapshot after [`Ui::frame`] or
/// [`Ui::layout`] so bounds and focus are current. Its `Display` form is one
/// line per node, indented by depth, for golden tests and tools.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
/// ui.button(root, "Play");
/// ui.slider(root).layout(ponko::ui::UiLayout::row(20.0)).value(0.25).accessible_name("Volume");
/// ui.push_screen(root, false);
/// ui.layout(vec2(800.0, 600.0));
/// assert_eq!(
///     ui.accessibility_tree().to_string(),
///     "group\n  button \"Play\" focusable\n  slider \"Volume\" value=0.25 focusable\n"
/// );
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiAccessTree {
    nodes: Vec<UiAccessNode>,
    index: HashMap<UiId, usize>,
    screens: Vec<UiId>,
    focus: Option<UiId>,
}

impl UiAccessTree {
    /// All exposed nodes, depth-first, screens from the bottom up.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(ponko::ui::Ui::default().accessibility_tree().nodes().is_empty());
    /// ```
    pub fn nodes(&self) -> &[UiAccessNode] {
        &self.nodes
    }
    /// The exposed node for `id`, or `None` if it is hidden, removed or on a
    /// screen covered by a modal one.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(ponko::ui::Ui::default().accessibility_tree().get(ponko::ui::UiId::from_bits(0)).is_none());
    /// ```
    pub fn get(&self, id: UiId) -> Option<&UiAccessNode> {
        self.index.get(&id).map(|&i| &self.nodes[i])
    }
    /// Exposed screen roots, from the bottom up.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(ponko::ui::Ui::default().accessibility_tree().screens().is_empty());
    /// ```
    pub fn screens(&self) -> &[UiId] {
        &self.screens
    }
    /// The focused node, if it is exposed.
    /// ```
    /// use ponko::prelude::*;
    /// assert_eq!(ponko::ui::Ui::default().accessibility_tree().focus(), None);
    /// ```
    pub fn focus(&self) -> Option<UiId> {
        self.focus
    }
}

impl fmt::Display for UiAccessTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for n in &self.nodes {
            write!(f, "{:indent$}{}", "", n.role.as_str(), indent = n.depth * 2)?;
            if !n.name.is_empty() {
                write!(f, " {:?}", n.name)?;
            }
            match &n.value {
                UiAccessValue::None => {}
                UiAccessValue::Text(text) => write!(f, " value={text:?}")?,
                UiAccessValue::Checked(true) => f.write_str(" checked")?,
                UiAccessValue::Checked(false) => f.write_str(" unchecked")?,
                UiAccessValue::Range(v) => write!(f, " value={v:.2}")?,
            }
            if !n.description.is_empty() {
                write!(f, " description={:?}", n.description)?;
            }
            // The caret matters to a reader only while the field is edited.
            if let (true, Some((anchor, caret))) = (n.focused, n.selection) {
                if anchor == caret {
                    write!(f, " caret={caret}")?;
                } else {
                    write!(f, " selection={}..{}", anchor.min(caret), anchor.max(caret))?;
                }
            }
            if n.live != UiLive::Off {
                write!(f, " live={}", n.live.as_str())?;
            }
            for (on, flag) in [
                (n.modal, "modal"),
                (n.disabled, "disabled"),
                (n.inert, "inert"),
                (n.focusable, "focusable"),
                (n.focused, "focused"),
            ] {
                if on {
                    write!(f, " {flag}")?;
                }
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

impl Ui {
    /// Override the name assistive technology reads for a node. By default
    /// the name is the node's text (none for text inputs, whose text is their
    /// value); sliders, text inputs, images and icon-only buttons need one.
    /// An empty name restores the default. Names never change layout,
    /// drawing, focus or events.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let close = ui.button(root, "X").id();
    /// ui.set_accessible_name(close, "Close");
    /// assert_eq!(ui.accessible_name(close), Some("Close"));
    /// assert_eq!(ui.text(close), Some("X"));
    /// ```
    pub fn set_accessible_name(&mut self, id: UiId, name: impl AsRef<str>) {
        if let Some(n) = self.node_mut(id) {
            n.access_name.clear();
            n.access_name.push_str(name.as_ref());
        }
    }
    /// The name assistive technology reads, or `None` for a stale id.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let play = ui.button(root, "Play").id();
    /// assert_eq!(ui.accessible_name(play), Some("Play"));
    /// ```
    pub fn accessible_name(&self, id: UiId) -> Option<&str> {
        let n = self.node(id)?;
        Some(if !n.access_name.is_empty() {
            &n.access_name
        } else if n.widget == UiWidget::TextInput {
            ""
        } else {
            &n.text
        })
    }
    /// What assistive technology reads after a node's name: what a control
    /// does, or a hint. Empty removes it. Like names, descriptions never
    /// change layout, drawing, focus or events.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let wipe = ui.button(root, "Reset").id();
    /// ui.set_accessible_description(wipe, "Deletes every save");
    /// assert_eq!(ui.accessible_description(wipe), Some("Deletes every save"));
    /// ```
    pub fn set_accessible_description(&mut self, id: UiId, description: impl AsRef<str>) {
        if let Some(n) = self.node_mut(id) {
            n.access_description.clear();
            n.access_description.push_str(description.as_ref());
        }
    }
    /// The description assistive technology reads, or `None` for a stale id.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let play = ui.button(root, "Play").id();
    /// assert_eq!(ui.accessible_description(play), Some(""));
    /// ```
    pub fn accessible_description(&self, id: UiId) -> Option<&str> {
        self.node(id).map(|n| n.access_description.as_str())
    }
    /// Have changes to a node announced while something else has focus; see
    /// [`UiLive`]. Use it sparingly: a status line, not a frame counter.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let status = ui.label(root, "3 lives").id();
    /// ui.set_live_region(status, ponko::ui::UiLive::Assertive);
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// assert_eq!(ui.accessibility_tree().get(status).unwrap().live, ponko::ui::UiLive::Assertive);
    /// ```
    pub fn set_live_region(&mut self, id: UiId, live: UiLive) {
        if let Some(n) = self.node_mut(id) {
            n.access_live = live;
        }
    }
    /// A node's live-region setting, or `None` for a stale id.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// assert_eq!(ui.live_region(root), Some(ponko::ui::UiLive::Off));
    /// ```
    pub fn live_region(&self, id: UiId) -> Option<UiLive> {
        self.node(id).map(|n| n.access_live)
    }
    /// Snapshot the accessibility semantics; see [`UiAccessTree`]. This
    /// allocates, so take it when the UI changes or a tool asks, not as part
    /// of every frame's hot path; a [`UiAccessTracker`] does that for you.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// let play = ui.button(root, "Play").id();
    /// ui.push_screen(root, false);
    /// ui.frame(vec2(800.0, 600.0), &ponko::ui::UiInput { next: true, ..Default::default() });
    /// assert_eq!(ui.accessibility_tree().focus(), Some(play));
    /// ```
    pub fn accessibility_tree(&self) -> UiAccessTree {
        let mut tree = UiAccessTree::default();
        self.build_access(&mut tree);
        tree
    }
    /// Fills an emptied `tree`, keeping its buffers.
    fn build_access(&self, tree: &mut UiAccessTree) {
        tree.nodes.clear();
        tree.index.clear();
        tree.screens.clear();
        self.walk_access(&mut |seen| {
            let at = tree.nodes.len();
            tree.index.insert(seen.id, at);
            match seen.parent {
                Some(parent) => {
                    let parent = tree.index[&parent];
                    tree.nodes[parent].children.push(seen.id);
                }
                None => tree.screens.push(seen.id),
            }
            tree.nodes.push(seen.to_node());
        });
        tree.focus = self.focus().filter(|id| tree.index.contains_key(id));
    }
    /// Calls `visit` for every exposed node: depth-first in layout order,
    /// screens from the bottom up, a parent before its children. The one
    /// place that decides what assistive technology sees; the snapshot and
    /// the change tracker both read it, so they can't disagree.
    fn walk_access(&self, visit: &mut dyn FnMut(&Seen)) {
        let modal = self.screens.iter().rposition(|s| s.modal);
        let top = self.top_screen();
        // HUDs are under every screen: readable, never interactive, and
        // hidden like the screens below a modal one.
        if modal.is_none() {
            for &hud in &self.huds {
                self.walk_access_node(visit, hud, None, 0, true, false, false);
            }
        }
        for screen in self.screens.iter().skip(modal.unwrap_or(0)) {
            self.walk_access_node(
                visit,
                screen.root,
                None,
                0,
                Some(screen.root) != top,
                false,
                screen.modal,
            );
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn walk_access_node(
        &self,
        visit: &mut dyn FnMut(&Seen),
        id: UiId,
        parent: Option<UiId>,
        depth: usize,
        inert: bool,
        disabled: bool,
        modal: bool,
    ) {
        let Some(n) = self.node(id) else { return };
        if self.tree.style(n.layout).is_ok_and(|s| s.display == Display::None) {
            return;
        }
        let disabled = disabled || !n.enabled;
        visit(&Seen {
            id,
            parent,
            depth,
            role: if modal { UiRole::Dialog } else { UiRole::of(n.widget) },
            name: self.accessible_name(id).unwrap_or_default(),
            description: &n.access_description,
            value: match n.widget {
                UiWidget::TextInput => SeenValue::Text(&n.text),
                UiWidget::Toggle => SeenValue::Checked(n.value >= 0.5),
                UiWidget::Slider => SeenValue::Range(n.value),
                _ => SeenValue::None,
            },
            selection: (n.widget == UiWidget::TextInput).then_some((n.anchor, n.caret)),
            live: n.access_live,
            rect: n.rect,
            focusable: self.focusable(id),
            focused: self.focus() == Some(id),
            disabled,
            inert,
            modal,
        });
        for &child in &n.children {
            self.walk_access_node(visit, child, Some(id), depth + 1, inert, disabled, false);
        }
    }

    /// Queue a request from assistive technology (or a test standing in for
    /// it). The next [`Ui::frame`] applies it before that frame's input, so
    /// its [`UiEvent`](super::UiEvent)s are read like those of a key press. A request for a
    /// node that keyboard navigation couldn't reach right now (hidden,
    /// disabled, under a modal screen, removed) does nothing.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::ui::{UiAccessAction, UiEvent, UiInput};
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
    /// let play = ui.button(root, "Play").id();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// ui.push_access_action(UiAccessAction::Activate(play));
    /// ui.frame(vec2(800.0, 600.0), &UiInput::default());
    /// assert_eq!(ui.next_event(), Some(UiEvent::Clicked(play)));
    /// assert_eq!(ui.focus(), Some(play));
    /// ```
    pub fn push_access_action(&mut self, action: UiAccessAction) {
        self.access_actions.push(action);
    }
    /// Applies the queued requests; true if one moved the focus.
    pub(super) fn apply_access_actions(&mut self) -> bool {
        if self.access_actions.is_empty() {
            return false;
        }
        let before = self.focus;
        let mut actions = std::mem::take(&mut self.access_actions);
        for action in actions.drain(..) {
            let id = action.target();
            if !self.focusable(id) {
                continue;
            }
            let widget = self.node(id).unwrap().widget;
            match action {
                UiAccessAction::Focus(_) => self.focus = Some(id),
                UiAccessAction::Activate(_) => {
                    self.focus = Some(id);
                    self.activate(id);
                }
                UiAccessAction::SetValue(_, value) => match widget {
                    UiWidget::Slider if value.is_finite() => self.change_value(id, value.clamp(0.0, 1.0)),
                    UiWidget::Toggle if value.is_finite() => {
                        self.change_value(id, if value >= 0.5 { 1.0 } else { 0.0 })
                    }
                    _ => {}
                },
                UiAccessAction::Increment(_) | UiAccessAction::Decrement(_) => {
                    if widget == UiWidget::Slider {
                        let step = if matches!(action, UiAccessAction::Increment(_)) {
                            SLIDER_STEP
                        } else {
                            -SLIDER_STEP
                        };
                        let value = self.value(id).unwrap() + step;
                        self.change_value(id, value.clamp(0.0, 1.0));
                    }
                }
                UiAccessAction::SetText(_, text) => {
                    if widget == UiWidget::TextInput {
                        let n = self.node_mut(id).unwrap();
                        let changed = n.text != text;
                        n.text = text;
                        n.caret = n.text.len();
                        n.anchor = n.caret;
                        if changed {
                            self.push_changed(id);
                        }
                    }
                }
            }
        }
        // Keep the queue's buffer for the next request.
        self.access_actions = actions;
        self.focus != before
    }
}

/// The step one keyboard, gamepad or assistive-technology nudge moves a slider.
pub(super) const SLIDER_STEP: f32 = 0.05;

/// Something assistive technology asks the UI to do; see
/// [`Ui::push_access_action`]. Each does what the matching keyboard or
/// gamepad input does, and emits the same [`UiEvent`](super::UiEvent)s.
/// ```
/// use ponko::prelude::*;
/// use ponko::ui::{UiAccessAction, UiInput};
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
/// let volume = ui.slider(root).layout(ponko::ui::UiLayout::row(20.0)).value(0.5).id();
/// ui.push_screen(root, false);
/// ui.layout(vec2(800.0, 600.0));
/// ui.push_access_action(UiAccessAction::Increment(volume));
/// ui.frame(vec2(800.0, 600.0), &UiInput::default());
/// assert!((ui.value(volume).unwrap() - 0.55).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum UiAccessAction {
    /// Move keyboard focus to the node.
    Focus(UiId),
    /// Press a button, flip a toggle, submit a text input; focuses it too.
    Activate(UiId),
    /// Set a slider (clamped to 0..1) or a toggle (on from 0.5).
    SetValue(UiId, f32),
    /// Move a slider up one step (0.05), like the right arrow.
    Increment(UiId),
    /// Move a slider down one step (0.05), like the left arrow.
    Decrement(UiId),
    /// Replace a text input's text; the caret goes to the end.
    SetText(UiId, String),
}

impl UiAccessAction {
    /// The node the request is for.
    /// ```
    /// use ponko::ui::{UiAccessAction, UiId};
    /// assert_eq!(UiAccessAction::Focus(UiId::from_bits(3)).target(), UiId::from_bits(3));
    /// ```
    pub fn target(&self) -> UiId {
        match self {
            Self::Focus(id)
            | Self::Activate(id)
            | Self::SetValue(id, _)
            | Self::Increment(id)
            | Self::Decrement(id)
            | Self::SetText(id, _) => *id,
        }
    }
}

/// A node's value as the walk sees it, borrowed.
#[derive(Clone, Copy)]
enum SeenValue<'a> {
    None,
    Text(&'a str),
    Checked(bool),
    Range(f32),
}

/// One exposed node as the walk sees it: everything [`UiAccessNode`] holds
/// except its children, borrowed from the UI.
struct Seen<'a> {
    id: UiId,
    parent: Option<UiId>,
    depth: usize,
    role: UiRole,
    name: &'a str,
    description: &'a str,
    value: SeenValue<'a>,
    selection: Option<(usize, usize)>,
    live: UiLive,
    rect: Rect,
    focusable: bool,
    focused: bool,
    disabled: bool,
    inert: bool,
    modal: bool,
}

impl Seen<'_> {
    fn to_node(&self) -> UiAccessNode {
        UiAccessNode {
            id: self.id,
            parent: self.parent,
            children: Vec::new(),
            depth: self.depth,
            role: self.role,
            name: self.name.to_owned(),
            description: self.description.to_owned(),
            value: match self.value {
                SeenValue::None => UiAccessValue::None,
                SeenValue::Text(text) => UiAccessValue::Text(text.to_owned()),
                SeenValue::Checked(on) => UiAccessValue::Checked(on),
                SeenValue::Range(v) => UiAccessValue::Range(v),
            },
            selection: self.selection,
            live: self.live,
            rect: self.rect,
            focusable: self.focusable,
            focused: self.focused,
            disabled: self.disabled,
            inert: self.inert,
            modal: self.modal,
        }
    }
    /// Everything about the node but its children (their order is the order
    /// of the walk), as one number: equal numbers, same node.
    fn fingerprint(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.id.hash(&mut h);
        self.parent.hash(&mut h);
        self.role.hash(&mut h);
        self.name.hash(&mut h);
        self.description.hash(&mut h);
        match self.value {
            SeenValue::None => 0u8.hash(&mut h),
            SeenValue::Text(text) => (1u8, text).hash(&mut h),
            SeenValue::Checked(on) => (2u8, on).hash(&mut h),
            SeenValue::Range(v) => (3u8, v.to_bits()).hash(&mut h),
        }
        self.selection.hash(&mut h);
        self.live.hash(&mut h);
        for v in [self.rect.min.x, self.rect.min.y, self.rect.max.x, self.rect.max.y] {
            v.to_bits().hash(&mut h);
        }
        (self.focusable, self.focused, self.disabled, self.inert, self.modal).hash(&mut h);
        h.finish()
    }
}

/// What changed in a UI's accessibility semantics since a
/// [`UiAccessTracker`] last looked.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
/// let play = ui.button(root, "Play").id();
/// ui.push_screen(root, false);
/// ui.layout(vec2(800.0, 600.0));
/// let mut tracker = ponko::ui::UiAccessTracker::new();
/// tracker.update(&ui);
/// ui.set_text(play, "Continue");
/// let changes = tracker.update(&ui).unwrap();
/// assert_eq!((changes.changed.as_slice(), changes.removed.len()), (&[play][..], 0));
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiAccessChanges {
    /// Nodes that are new, or whose role, name, description, value,
    /// selection, bounds, state or list of children changed; in tree order
    /// (a parent before its children). Read them from
    /// [`UiAccessTracker::tree`].
    pub changed: Vec<UiId>,
    /// Nodes that are no longer exposed: removed, hidden, or covered by a
    /// modal screen.
    pub removed: Vec<UiId>,
    /// The list of exposed screen roots changed (a screen was pushed or
    /// popped, or a modal one covered or uncovered the ones below).
    pub screens_changed: bool,
    /// The focused node changed.
    pub focus_changed: bool,
}

/// Follows one UI's accessibility semantics and reports only what changed,
/// so an adapter (or a tool) doesn't rebuild the whole tree every frame.
///
/// Call [`update`](Self::update) once a frame, after [`Ui::frame`]. While
/// nothing changed it walks the UI comparing fingerprints and allocates
/// nothing; when something did, it rebuilds its snapshot and says which
/// nodes to resend.
/// ```
/// use ponko::prelude::*;
/// let mut ui = ponko::ui::Ui::default();
/// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
/// ui.button(root, "Play");
/// ui.push_screen(root, false);
/// ui.layout(vec2(800.0, 600.0));
/// let mut tracker = ponko::ui::UiAccessTracker::new();
/// assert_eq!(tracker.update(&ui).unwrap().changed.len(), 2, "the first look reports everything");
/// assert!(tracker.update(&ui).is_none(), "nothing changed since");
/// assert_eq!(tracker.tree().nodes().len(), 2);
/// ```
#[derive(Debug, Default)]
pub struct UiAccessTracker {
    tree: UiAccessTree,
    /// Each exposed node's fingerprint, in walk order, as of `tree`.
    prints: Vec<(UiId, u64)>,
    /// Reused while working out a change.
    next_prints: Vec<(UiId, u64)>,
    old: HashMap<UiId, (u64, Vec<UiId>)>,
    looked: bool,
}

impl UiAccessTracker {
    /// A tracker that has seen nothing: its first update reports every node.
    /// ```
    /// assert!(ponko::ui::UiAccessTracker::new().tree().nodes().is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }
    /// The snapshot as of the last [`update`](Self::update).
    /// ```
    /// assert_eq!(ponko::ui::UiAccessTracker::new().tree().focus(), None);
    /// ```
    pub fn tree(&self) -> &UiAccessTree {
        &self.tree
    }
    /// Forget what was seen: the next update reports every node again. For
    /// an adapter whose platform side started over.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// let mut tracker = ponko::ui::UiAccessTracker::new();
    /// tracker.update(&ui);
    /// tracker.reset();
    /// assert_eq!(tracker.update(&ui).unwrap().changed, [root]);
    /// ```
    pub fn reset(&mut self) {
        self.tree.nodes.clear();
        self.tree.index.clear();
        self.tree.screens.clear();
        self.tree.focus = None;
        self.prints.clear();
        self.looked = false;
    }
    /// Look at `ui` and return what changed since the last look, or `None`
    /// if nothing did. The first look (and the first after
    /// [`reset`](Self::reset)) always returns changes, with every exposed
    /// node in `changed`, even for an empty UI.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().layout(ponko::ui::UiLayout::fill()).id();
    /// let hint = ui.label(root, "Press start").id();
    /// ui.push_screen(root, false);
    /// ui.layout(vec2(800.0, 600.0));
    /// let mut tracker = ponko::ui::UiAccessTracker::new();
    /// tracker.update(&ui);
    /// ui.remove(hint);
    /// ui.layout(vec2(800.0, 600.0));
    /// let changes = tracker.update(&ui).unwrap();
    /// assert_eq!((changes.changed, changes.removed), (vec![root], vec![hint]));
    /// ```
    pub fn update(&mut self, ui: &Ui) -> Option<UiAccessChanges> {
        // The cheap pass: is every node the one seen last time, in order?
        let prints = &self.prints;
        let (mut at, mut same) = (0, self.looked);
        ui.walk_access(&mut |seen| {
            same = same && prints.get(at) == Some(&(seen.id, seen.fingerprint()));
            at += 1;
        });
        if same && at == prints.len() {
            return None;
        }

        // Something changed: keep what is needed of the old snapshot, take a
        // new one, and compare.
        self.old.clear();
        for &(id, print) in &self.prints {
            let children = std::mem::take(&mut self.tree.nodes[self.tree.index[&id]].children);
            self.old.insert(id, (print, children));
        }
        let old_screens = std::mem::take(&mut self.tree.screens);
        let old_focus = self.tree.focus;
        let next = &mut self.next_prints;
        next.clear();
        ui.walk_access(&mut |seen| next.push((seen.id, seen.fingerprint())));
        ui.build_access(&mut self.tree);

        let mut changes = UiAccessChanges {
            screens_changed: !self.looked || old_screens != self.tree.screens,
            focus_changed: old_focus != self.tree.focus,
            ..Default::default()
        };
        for (&(id, print), node) in self.next_prints.iter().zip(&self.tree.nodes) {
            let unchanged = self
                .old
                .get(&id)
                .is_some_and(|(old_print, old_children)| *old_print == print && *old_children == node.children);
            if !unchanged {
                changes.changed.push(id);
            }
        }
        for &(id, _) in &self.prints {
            if !self.tree.index.contains_key(&id) {
                changes.removed.push(id);
            }
        }
        std::mem::swap(&mut self.prints, &mut self.next_prints);
        self.looked = true;
        Some(changes)
    }
}
