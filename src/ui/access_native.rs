//! The native accessibility adapter: gives the retained UI's semantics to
//! the system's accessibility API, so a screen reader can read and operate
//! `ctx.ui`, and brings the screen reader's requests back to the UI.
//!
//! Three backends, all AccessKit's, fed from the same tree
//! (`access_kit.rs`):
//!
//! - macOS: `NSAccessibility` on the window's view (VoiceOver).
//! - Windows: UI Automation on the window's `HWND` (Narrator, NVDA, JAWS).
//! - Linux and the BSDs: AT-SPI over D-Bus (Orca).
//!
//! On every other system (the web) the adapter is inert: it builds, never
//! activates, and costs nothing.
//!
//! What every backend shares lives in [`Core`]: the tracker, the requests
//! that arrived, and whether the system needs the whole tree. The backends
//! differ in which thread calls back (macOS: the main thread; Windows: the
//! window's thread for activation, any thread for actions; AT-SPI: always
//! its own thread), so the shared state is behind atomics and a mutex.

use super::access_kit;
use super::{Ui, UiAccessTracker};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// What the system's callbacks and the adapter share.
#[derive(Default)]
struct Shared {
    /// Assistive technology is listening: there is someone to tell.
    active: AtomicBool,
    /// Assistive technology (re)started and holds no tree: the next update
    /// the platform takes must be the whole tree. Set by the activation
    /// callback, taken by the update the platform asks for, so a restart
    /// between two frames is never answered with only the changes.
    fresh: AtomicBool,
    /// Requests that arrived since the last update.
    requests: Mutex<Vec<accesskit::ActionRequest>>,
}

/// What every backend shares: see the module's documentation.
struct Core {
    tracker: UiAccessTracker,
    shared: Arc<Shared>,
    title: String,
    /// The scale factor the platform side last heard.
    scale_factor: f64,
    /// The platform side took a tree since it last (re)started; changes
    /// are enough from now on.
    sent: bool,
}

impl Core {
    fn new() -> Self {
        Self {
            tracker: UiAccessTracker::new(),
            shared: Arc::default(),
            title: String::new(),
            scale_factor: 1.0,
            sent: false,
        }
    }

    /// Queues the requests that arrived for the UI's next frame, and says
    /// what to send if the system is listening and something changed.
    fn prepare(&mut self, ui: &mut Ui, scale_factor: f32) -> Option<Outgoing<'_>> {
        for request in self
            .shared
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            if let Some(action) = access_kit::action(&request) {
                ui.push_access_action(action);
            }
        }
        if !self.shared.active.load(Ordering::Acquire) {
            return None;
        }
        let scale_factor = f64::from(scale_factor);
        let window_changed = self.scale_factor != scale_factor;
        let changes = self.tracker.update(ui);
        let restarted = self.shared.fresh.load(Ordering::Acquire);
        if changes.is_none() && !window_changed && self.sent && !restarted {
            return None;
        }
        self.scale_factor = scale_factor;
        Some(Outgoing {
            tree: self.tracker.tree(),
            changes: changes.unwrap_or_default(),
            title: &self.title,
            scale_factor,
            window_changed,
            shared: &self.shared,
            sent: &mut self.sent,
        })
    }
}

/// One frame's news for the platform, built only if it takes it.
struct Outgoing<'a> {
    tree: &'a super::UiAccessTree,
    changes: super::UiAccessChanges,
    title: &'a str,
    scale_factor: f64,
    window_changed: bool,
    shared: &'a Shared,
    sent: &'a mut bool,
}

impl Outgoing<'_> {
    /// The update to hand over: the whole tree if the platform has none
    /// (first time, or assistive technology restarted), else the changes.
    /// Called by the platform, under its own lock, only when it takes one.
    fn build(self) -> accesskit::TreeUpdate {
        let restarted = self.shared.fresh.swap(false, Ordering::AcqRel);
        let full = restarted || !*self.sent;
        *self.sent = true;
        if full {
            access_kit::full_update(self.tree, self.title, self.scale_factor)
        } else {
            access_kit::partial_update(
                self.tree,
                &self.changes,
                self.title,
                self.scale_factor,
                self.window_changed,
            )
        }
    }
}

/// Connects one [`Ui`] to the system's accessibility API (feature
/// `accessibility`).
///
/// The engine makes one for the game's window and `ctx.ui` and drives it
/// every frame; nothing needs to be called from game code. A custom host
/// that owns its own window makes one with the constructor for its system
/// (`for_ns_view` on macOS, `for_hwnd` on Windows, `for_atspi` on Linux
/// and the BSDs; each exists only in builds for its system) and calls [`update`](Self::update) once a frame,
/// after [`Ui::frame`].
///
/// Until assistive technology asks for the tree the adapter does nothing:
/// no snapshot is taken and nothing is allocated. Once it has, the first
/// update sends the whole tree and later ones only the nodes that changed
/// (see [`UiAccessTracker`]); if assistive technology goes away and comes
/// back (AT-SPI says so), the whole tree is sent again. Requests (press,
/// focus, set value, increment, decrement) are queued for the UI's next
/// frame as [`UiAccessAction`](super::UiAccessAction)s.
///
/// Supported on macOS, Windows, Linux and the BSDs. Elsewhere (the web)
/// [`SUPPORTED`](Self::SUPPORTED) is false and every method is a no-op.
/// ```
/// let mut adapter = ponko::ui::UiAccessAdapter::inert();
/// let mut ui = ponko::ui::Ui::default();
/// adapter.update(&mut ui, 1.0);
/// assert!(!adapter.is_active());
/// ```
pub struct UiAccessAdapter {
    core: Core,
    platform: Option<platform::Platform>,
}

impl UiAccessAdapter {
    /// Whether this build has a native backend for the system it runs on:
    /// macOS, Windows, Linux and the BSDs.
    /// ```
    /// let desktop = cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux"));
    /// if desktop {
    ///     assert!(ponko::ui::UiAccessAdapter::SUPPORTED);
    /// }
    /// ```
    pub const SUPPORTED: bool = platform::SUPPORTED;

    /// An adapter connected to nothing: it never activates. What the engine
    /// uses where there is no backend.
    /// ```
    /// assert!(!ponko::ui::UiAccessAdapter::inert().is_active());
    /// ```
    pub fn inert() -> Self {
        Self {
            core: Core::new(),
            platform: None,
        }
    }

    /// An inert adapter that already has the window's title.
    fn titled(title: &str) -> Self {
        let mut adapter = Self::inert();
        adapter.core.title.push_str(title);
        adapter
    }

    /// An adapter for an `NSView`: the view answers the system's
    /// accessibility queries from then on. `title` names the window to the
    /// screen reader. Call it on the main thread, before the view is shown
    /// or focused for the first time.
    ///
    /// # Safety
    ///
    /// `view` must be a valid, unreleased pointer to an `NSView`, and no
    /// other adapter may be attached to it.
    ///
    /// # Panics
    ///
    /// Off the main thread, or if the view already has an adapter.
    #[cfg(target_os = "macos")]
    pub unsafe fn for_ns_view(view: *mut std::ffi::c_void, title: &str) -> Self {
        let mut adapter = Self::titled(title);
        let h = Handlers::new(&adapter.core.shared);
        // SAFETY: the caller's contract is the adapter's.
        adapter.platform = Some(unsafe { accesskit_macos::SubclassingAdapter::new(view, h.activation, h.requests) });
        adapter
    }

    /// An adapter for a Win32 window: the window answers UI Automation's
    /// queries (`WM_GETOBJECT`, which Narrator and NVDA send) from then on,
    /// and follows the window's keyboard focus by itself. `title` names the
    /// window to the screen reader. Call it on the thread that owns the
    /// window, before the window is shown for the first time.
    ///
    /// # Safety
    ///
    /// `hwnd` must be a valid window handle owned by the calling thread,
    /// and no other adapter may be attached to it. The adapter may outlive
    /// the window (it notices `WM_NCDESTROY`).
    ///
    /// # Panics
    ///
    /// If the window is already visible.
    #[cfg(target_os = "windows")]
    pub unsafe fn for_hwnd(hwnd: *mut std::ffi::c_void, title: &str) -> Self {
        let mut adapter = Self::titled(title);
        let h = Handlers::new(&adapter.core.shared);
        adapter.platform = Some(accesskit_windows::SubclassingAdapter::new(
            accesskit_windows::HWND(hwnd),
            h.activation,
            h.requests,
        ));
        adapter
    }

    /// An adapter that registers with the AT-SPI accessibility bus, where
    /// Orca and other assistive technology find the application and its
    /// window (named `title`). It answers from AccessKit's own thread, so
    /// it needs no window handle; tell it where the window is with
    /// [`set_window_bounds`](Self::set_window_bounds) and whether it has
    /// focus with [`set_window_focused`](Self::set_window_focused).
    ///
    /// Without a session bus, or while accessibility is off on the desktop
    /// (no screen reader runs), it stays inactive and costs nothing.
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    pub fn for_atspi(title: &str) -> Self {
        let mut adapter = Self::titled(title);
        let h = Handlers::new(&adapter.core.shared);
        adapter.platform = Some(accesskit_unix::Adapter::new(h.activation, h.requests, h.deactivation));
        adapter
    }

    /// The adapter for the engine's window, or an inert one where there is
    /// no backend. The window must not have been shown yet.
    pub(crate) fn for_window(window: &winit::window::Window, title: &str) -> Self {
        let mut adapter = platform::for_window(window, title).unwrap_or_else(|| Self::titled(title));
        adapter.follow_window(window);
        adapter
    }

    /// Assistive technology has asked for the tree, so updates are being
    /// sent. False until a screen reader (or an accessibility inspector)
    /// looks at the window, and again after it has gone (AT-SPI says so).
    /// ```
    /// assert!(!ponko::ui::UiAccessAdapter::inert().is_active());
    /// ```
    pub fn is_active(&self) -> bool {
        self.core.shared.active.load(Ordering::Acquire)
    }

    /// Whether the event loop should pause between frames to answer the
    /// system's questions: assistive technology is reading, and its
    /// questions come through the event loop (macOS, Windows).
    pub(crate) fn answers_on_event_loop(&self) -> bool {
        platform::QUERIES_ON_EVENT_LOOP && self.is_active()
    }

    /// The name the screen reader gives the window. The engine sets it once,
    /// from the game's title: its window keeps the title it was created
    /// with (nothing changes it while the game runs). A host that renames
    /// its own window calls this again.
    /// ```
    /// let mut adapter = ponko::ui::UiAccessAdapter::inert();
    /// adapter.set_title("Settings");
    /// ```
    pub fn set_title(&mut self, title: &str) {
        if self.core.title != title {
            self.core.title.clear();
            self.core.title.push_str(title);
            // The window node carries it: send that node again.
            self.core.scale_factor = f64::NAN;
        }
    }

    /// Tell the system whether the window has keyboard focus; the focused
    /// node is only reported as focused while it does. On Windows the
    /// adapter follows the window's own focus messages, and this does
    /// nothing.
    /// ```
    /// ponko::ui::UiAccessAdapter::inert().set_window_focused(true);
    /// ```
    pub fn set_window_focused(&mut self, focused: bool) {
        if let Some(platform) = self.platform.as_mut() {
            platform::set_window_focused(platform, focused);
        }
    }

    /// Tell AT-SPI where the window is on the screen, as `[x0, y0, x1, y1]`
    /// in physical pixels: `outer` with its decorations, `inner` its
    /// content. Screen readers use it to place what they highlight and to
    /// find the node under the pointer. Only AT-SPI needs it (macOS and
    /// Windows ask the window); elsewhere it does nothing. Under Wayland a
    /// program can't know its window's position: give the origin there.
    /// The engine calls it whenever its window moves or is resized.
    /// ```
    /// let mut adapter = ponko::ui::UiAccessAdapter::inert();
    /// adapter.set_window_bounds([0.0, 0.0, 820.0, 640.0], [10.0, 30.0, 810.0, 630.0]);
    /// ```
    pub fn set_window_bounds(&mut self, outer: [f64; 4], inner: [f64; 4]) {
        if let Some(platform) = self.platform.as_mut() {
            let rect = |[x0, y0, x1, y1]: [f64; 4]| accesskit::Rect { x0, y0, x1, y1 };
            platform::set_window_bounds(platform, rect(outer), rect(inner));
        }
    }

    /// Gives the system window's place on the screen to the adapter (see
    /// [`set_window_bounds`](Self::set_window_bounds)); the engine calls it
    /// when its window is made, moved or resized.
    pub(crate) fn follow_window(&mut self, window: &winit::window::Window) {
        if !platform::NEEDS_WINDOW_BOUNDS || self.platform.is_none() {
            return;
        }
        let (outer, inner) = window_bounds(
            window.outer_position().ok().map(|p| (p.x, p.y)),
            (window.outer_size().width, window.outer_size().height),
            window.inner_position().ok().map(|p| (p.x, p.y)),
            (window.inner_size().width, window.inner_size().height),
        );
        self.set_window_bounds(outer, inner);
    }

    /// Once a frame, after [`Ui::frame`]: queue the requests that arrived
    /// for the UI's next frame, and, if assistive technology is listening,
    /// send what changed. `scale_factor` is the window's (physical pixels
    /// per logical unit); the UI's bounds are logical.
    /// ```
    /// use ponko::prelude::*;
    /// let mut ui = ponko::ui::Ui::default();
    /// let root = ui.root().id();
    /// ui.push_screen(root, false);
    /// ui.frame(vec2(800.0, 600.0), &ponko::ui::UiInput::default());
    /// let mut adapter = ponko::ui::UiAccessAdapter::inert();
    /// adapter.update(&mut ui, 2.0);
    /// ```
    pub fn update(&mut self, ui: &mut Ui, scale_factor: f32) {
        let Some(outgoing) = self.core.prepare(ui, scale_factor) else {
            return;
        };
        if let Some(platform) = self.platform.as_mut() {
            platform::send(platform, || outgoing.build());
        }
    }
}

/// The window's outer and inner rectangles on the screen, as AT-SPI wants
/// them (`[x0, y0, x1, y1]`, physical pixels). A position the system won't
/// say (Wayland) is the origin.
fn window_bounds(
    outer_position: Option<(i32, i32)>,
    outer_size: (u32, u32),
    inner_position: Option<(i32, i32)>,
    inner_size: (u32, u32),
) -> ([f64; 4], [f64; 4]) {
    let rect = |position: Option<(i32, i32)>, (w, h): (u32, u32)| {
        let (x, y) = position.unwrap_or_default();
        let (x, y) = (f64::from(x), f64::from(y));
        [x, y, x + f64::from(w), y + f64::from(h)]
    };
    (rect(outer_position, outer_size), rect(inner_position, inner_size))
}

/// The callbacks a platform adapter is made with, all feeding one
/// [`Shared`].
#[cfg_attr(
    not(any(
        test,
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )),
    allow(dead_code)
)]
struct Handlers {
    activation: Activation,
    #[cfg_attr(
        not(any(
            test,
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )),
        allow(dead_code)
    )]
    deactivation: Deactivation,
    requests: Requests,
}

#[cfg_attr(
    not(any(
        test,
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )),
    allow(dead_code)
)]
impl Handlers {
    fn new(shared: &Arc<Shared>) -> Self {
        Self {
            activation: Activation(shared.clone()),
            deactivation: Deactivation(shared.clone()),
            requests: Requests(shared.clone()),
        }
    }
}

/// macOS: AccessKit's `NSAccessibility` adapter on the window's view.
#[cfg(target_os = "macos")]
mod platform {
    pub(super) type Platform = accesskit_macos::SubclassingAdapter;
    pub(super) const SUPPORTED: bool = true;
    /// The system's questions arrive through the event loop.
    pub(super) const QUERIES_ON_EVENT_LOOP: bool = true;
    pub(super) const NEEDS_WINDOW_BOUNDS: bool = false;

    pub(super) fn for_window(window: &winit::window::Window, title: &str) -> Option<super::UiAccessAdapter> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(RawWindowHandle::AppKit(handle)) = window.window_handle().map(|h| h.as_raw()) else {
            return None;
        };
        // SAFETY: winit hands out its own live view, on the main thread
        // (windows are only made there), and nothing else attaches an
        // adapter to it.
        Some(unsafe { super::UiAccessAdapter::for_ns_view(handle.ns_view.as_ptr(), title) })
    }

    pub(super) fn send(platform: &mut Platform, update: impl FnOnce() -> accesskit::TreeUpdate) {
        if let Some(events) = platform.update_if_active(update) {
            events.raise();
        }
    }

    pub(super) fn set_window_focused(platform: &mut Platform, focused: bool) {
        if let Some(events) = platform.update_view_focus_state(focused) {
            events.raise();
        }
    }

    pub(super) fn set_window_bounds(_: &mut Platform, _: accesskit::Rect, _: accesskit::Rect) {}
}

/// Windows: AccessKit's UI Automation adapter, which subclasses the
/// window to answer `WM_GETOBJECT` and follows its focus messages.
#[cfg(target_os = "windows")]
mod platform {
    pub(super) type Platform = accesskit_windows::SubclassingAdapter;
    pub(super) const SUPPORTED: bool = true;
    /// `WM_GETOBJECT` arrives through the event loop.
    pub(super) const QUERIES_ON_EVENT_LOOP: bool = true;
    pub(super) const NEEDS_WINDOW_BOUNDS: bool = false;

    pub(super) fn for_window(window: &winit::window::Window, title: &str) -> Option<super::UiAccessAdapter> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(RawWindowHandle::Win32(handle)) = window.window_handle().map(|h| h.as_raw()) else {
            return None;
        };
        // SAFETY: winit's own live window, made on this thread (the event
        // loop's), and nothing else attaches an adapter to it.
        Some(unsafe { super::UiAccessAdapter::for_hwnd(handle.hwnd.get() as *mut std::ffi::c_void, title) })
    }

    pub(super) fn send(platform: &mut Platform, update: impl FnOnce() -> accesskit::TreeUpdate) {
        if let Some(events) = platform.update_if_active(update) {
            events.raise();
        }
    }

    /// The adapter follows `WM_SETFOCUS` and `WM_KILLFOCUS` itself.
    pub(super) fn set_window_focused(_: &mut Platform, _: bool) {}

    pub(super) fn set_window_bounds(_: &mut Platform, _: accesskit::Rect, _: accesskit::Rect) {}
}

/// Linux and the BSDs: AccessKit's AT-SPI adapter, which talks D-Bus on its
/// own thread from its own copy of the tree.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod platform {
    pub(super) type Platform = accesskit_unix::Adapter;
    pub(super) const SUPPORTED: bool = true;
    /// Answered on AccessKit's thread: the event loop needs no pause.
    pub(super) const QUERIES_ON_EVENT_LOOP: bool = false;
    /// AT-SPI has no handle to the system window: it is told where it is.
    pub(super) const NEEDS_WINDOW_BOUNDS: bool = true;

    pub(super) fn for_window(_: &winit::window::Window, title: &str) -> Option<super::UiAccessAdapter> {
        Some(super::UiAccessAdapter::for_atspi(title))
    }

    pub(super) fn send(platform: &mut Platform, update: impl FnOnce() -> accesskit::TreeUpdate) {
        platform.update_if_active(update);
    }

    pub(super) fn set_window_focused(platform: &mut Platform, focused: bool) {
        platform.update_window_focus_state(focused);
    }

    pub(super) fn set_window_bounds(platform: &mut Platform, outer: accesskit::Rect, inner: accesskit::Rect) {
        platform.set_root_window_bounds(outer, inner);
    }
}

/// Every other system (the web): no backend, so no adapter can exist.
#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
mod platform {
    pub(super) enum Platform {}
    pub(super) const SUPPORTED: bool = false;
    pub(super) const QUERIES_ON_EVENT_LOOP: bool = false;
    pub(super) const NEEDS_WINDOW_BOUNDS: bool = false;

    pub(super) fn for_window(_: &winit::window::Window, _: &str) -> Option<super::UiAccessAdapter> {
        None
    }

    pub(super) fn send(platform: &mut Platform, _: impl FnOnce() -> accesskit::TreeUpdate) {
        match *platform {}
    }

    pub(super) fn set_window_focused(platform: &mut Platform, _: bool) {
        match *platform {}
    }

    pub(super) fn set_window_bounds(platform: &mut Platform, _: accesskit::Rect, _: accesskit::Rect) {
        match *platform {}
    }
}

impl std::fmt::Debug for UiAccessAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiAccessAdapter")
            .field("supported", &Self::SUPPORTED)
            .field("active", &self.is_active())
            .field("sent", &self.core.sent)
            .finish_non_exhaustive()
    }
}

/// The system asks for the tree the first time assistive technology looks
/// (and on AT-SPI again each time it comes back). There is no `Ui` to read
/// here, so the answer is "not yet": the next [`UiAccessAdapter::update`]
/// sends the whole tree.
struct Activation(Arc<Shared>);

impl accesskit::ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        self.0.fresh.store(true, Ordering::Release);
        self.0.active.store(true, Ordering::Release);
        None
    }
}

/// Assistive technology went away (AT-SPI only): stop sending until it
/// comes back, which asks for the whole tree again.
struct Deactivation(Arc<Shared>);

impl accesskit::DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {
        self.0.active.store(false, Ordering::Release);
    }
}

/// Requests are queued; the next update hands them to the UI.
struct Requests(Arc<Shared>);

impl accesskit::ActionHandler for Requests {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        self.0
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
    }
}

#[cfg(test)]
mod tests {
    //! The adapter's own logic, without a system: a stand-in platform keeps
    //! the state machine every AccessKit adapter has (inactive, waiting for
    //! its first tree, active), checks AccessKit's contract on what it is
    //! given, and holds the tree the way assistive technology sees it.
    use super::*;
    use crate::ui::{UiId, UiInput, UiLayout};
    use accesskit::{
        Action, ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, NodeId, Role, TreeId, TreeInfo,
        TreeUpdate,
    };
    use glam::Vec2;
    use std::collections::HashMap;

    const VIEW: Vec2 = Vec2::new(800.0, 600.0);

    #[derive(Debug, PartialEq)]
    enum State {
        Inactive,
        Waiting,
        Active,
    }

    struct FakePlatform {
        state: State,
        handlers: Handlers,
        nodes: HashMap<NodeId, accesskit::Node>,
        focus: NodeId,
        updates: Vec<TreeUpdate>,
    }

    impl FakePlatform {
        fn new(shared: &Arc<Shared>) -> Self {
            Self {
                state: State::Inactive,
                handlers: Handlers::new(shared),
                nodes: HashMap::new(),
                focus: access_kit::WINDOW,
                updates: Vec::new(),
            }
        }

        /// A screen reader starts and asks for the tree.
        fn screen_reader_starts(&mut self) {
            assert_eq!(self.state, State::Inactive);
            assert!(self.handlers.activation.request_initial_tree().is_none());
            self.state = State::Waiting;
        }

        /// The screen reader quits (AT-SPI tells the adapter).
        fn screen_reader_quits(&mut self) {
            self.state = State::Inactive;
            self.nodes.clear();
            self.handlers.deactivation.deactivate_accessibility();
        }

        fn update_if_active(&mut self, factory: impl FnOnce() -> TreeUpdate) {
            match self.state {
                State::Inactive => {}
                State::Waiting => {
                    let update = factory();
                    assert_eq!(
                        update.tree,
                        Some(TreeInfo::new(access_kit::WINDOW)),
                        "the first update after an activation must be the whole tree"
                    );
                    self.apply(update);
                    self.state = State::Active;
                }
                State::Active => {
                    let update = factory();
                    assert_eq!(update.tree, None, "the tree's root is said once");
                    self.apply(update);
                }
            }
        }

        fn apply(&mut self, update: TreeUpdate) {
            for (id, node) in &update.nodes {
                self.nodes.insert(*id, node.clone());
            }
            self.focus = update.focus;
            self.updates.push(update);
        }

        /// Every node reachable from the window, as `role name` lines in
        /// tree order: what a screen reader can walk. The name is what UI
        /// Automation and AT-SPI give: a label's value, any other node's
        /// label.
        fn walk(&self) -> Vec<String> {
            fn visit(nodes: &HashMap<NodeId, accesskit::Node>, id: NodeId, out: &mut Vec<String>) {
                let node = &nodes[&id];
                let name = if node.role() == Role::Label {
                    node.value()
                } else {
                    node.label()
                };
                out.push(format!("{:?} {}", node.role(), name.unwrap_or_default()));
                for &child in node.children() {
                    visit(nodes, child, out);
                }
            }
            let mut out = Vec::new();
            visit(&self.nodes, access_kit::WINDOW, &mut out);
            out
        }
    }

    struct Menu {
        ui: Ui,
        adapter: UiAccessAdapter,
        platform: FakePlatform,
        play: UiId,
        volume: UiId,
    }

    impl Menu {
        fn new() -> Self {
            let mut ui = Ui::default();
            let root = ui.root().layout(UiLayout::fill().padding(10.0).gap(4.0)).id();
            ui.label(root, "Settings").layout(UiLayout::row(24.0));
            let play = ui.button(root, "Play").layout(UiLayout::row(30.0)).id();
            let volume = ui
                .slider(root)
                .layout(UiLayout::row(20.0))
                .value(0.5)
                .accessible_name("Volume")
                .id();
            ui.push_screen(root, false);
            ui.frame(VIEW, &UiInput::default());
            let mut adapter = UiAccessAdapter::inert();
            adapter.set_title("Sample menu");
            let platform = FakePlatform::new(&adapter.core.shared);
            Self {
                ui,
                adapter,
                platform,
                play,
                volume,
            }
        }

        /// One engine frame: the UI, then the adapter's update, with the
        /// stand-in where the system's adapter would be.
        fn frame(&mut self) {
            self.ui.frame(VIEW, &UiInput::default());
            if let Some(outgoing) = self.adapter.core.prepare(&mut self.ui, 1.0) {
                self.platform.update_if_active(|| outgoing.build());
            }
        }

        fn request(&self, action: Action, id: UiId) -> ActionRequest {
            ActionRequest {
                action,
                target_tree: TreeId::ROOT,
                target_node: access_kit::node_id(id),
                data: None,
            }
        }
    }

    const MENU: [&str; 5] = [
        "Window Sample menu",
        "Group ",
        "Label Settings",
        "Button Play",
        "Slider Volume",
    ];

    #[test]
    fn nothing_is_sent_until_a_screen_reader_starts_and_then_the_whole_tree() {
        let mut m = Menu::new();
        for _ in 0..3 {
            m.frame();
        }
        assert!(!m.adapter.is_active());
        assert!(m.platform.updates.is_empty());

        m.platform.screen_reader_starts();
        assert!(m.adapter.is_active());
        m.frame();
        assert_eq!(m.platform.walk(), MENU);
        assert_eq!(m.platform.updates.len(), 1);

        // Nothing changed: nothing is sent.
        m.frame();
        assert_eq!(m.platform.updates.len(), 1);
    }

    #[test]
    fn after_the_first_tree_only_changes_are_sent() {
        let mut m = Menu::new();
        m.platform.screen_reader_starts();
        m.frame();
        m.ui.set_value(m.volume, 0.75);
        m.frame();
        let last = m.platform.updates.last().unwrap();
        assert_eq!(last.nodes.len(), 1, "only the slider");
        assert_eq!(m.platform.walk(), MENU);
    }

    #[test]
    fn a_screen_reader_that_comes_back_gets_the_whole_tree_again() {
        let mut m = Menu::new();
        m.platform.screen_reader_starts();
        m.frame();

        m.platform.screen_reader_quits();
        assert!(!m.adapter.is_active());
        m.ui.set_value(m.volume, 0.25);
        m.frame();
        assert_eq!(m.platform.updates.len(), 1, "no one to tell");

        // Back, with nothing changed in the UI since the last frame: the
        // stand-in fails unless the update is the whole tree.
        m.platform.screen_reader_starts();
        m.frame();
        assert_eq!(m.platform.updates.len(), 2);
        assert_eq!(m.platform.walk(), MENU);
        let volume = &m.platform.nodes[&access_kit::node_id(m.volume)];
        assert_eq!(volume.numeric_value(), Some(0.25));
    }

    #[test]
    fn a_restart_between_two_frames_is_answered_with_the_whole_tree() {
        // The screen reader quits and comes back on AccessKit's thread
        // while the game is between frames: the adapter never sees it
        // inactive, and nothing changed in the UI, but the platform waits
        // for a whole tree.
        let mut m = Menu::new();
        m.platform.screen_reader_starts();
        m.frame();
        m.platform.screen_reader_quits();
        m.platform.screen_reader_starts();
        m.frame();
        assert_eq!(m.platform.updates.len(), 2);
        assert_eq!(m.platform.walk(), MENU);
    }

    #[test]
    fn requests_from_another_thread_reach_the_ui_on_its_next_frame() {
        let mut m = Menu::new();
        m.platform.screen_reader_starts();
        m.frame();
        assert_eq!(m.platform.focus, access_kit::WINDOW);
        // AT-SPI calls the handler on its own thread; UI Automation may.
        let mut requests = Requests(m.adapter.core.shared.clone());
        let focus_play = m.request(Action::Focus, m.play);
        std::thread::spawn(move || requests.do_action(focus_play))
            .join()
            .unwrap();
        // Queued for the UI by this frame's update, applied by the next.
        m.frame();
        m.frame();
        assert_eq!(m.platform.focus, access_kit::node_id(m.play));
    }

    #[test]
    fn a_new_title_resends_only_the_window_node() {
        let mut m = Menu::new();
        m.platform.screen_reader_starts();
        m.frame();
        m.adapter.set_title("Paused");
        m.frame();
        let last = m.platform.updates.last().unwrap();
        assert_eq!(last.nodes.len(), 1);
        assert_eq!(last.nodes[0].1.role(), Role::Window);
        assert_eq!(last.nodes[0].1.label(), Some("Paused"));
    }

    #[test]
    fn window_bounds_fall_back_to_the_origin_where_the_position_is_unknown() {
        assert_eq!(
            window_bounds(Some((100, 50)), (820, 640), Some((110, 80)), (800, 600)),
            ([100.0, 50.0, 920.0, 690.0], [110.0, 80.0, 910.0, 680.0])
        );
        assert_eq!(
            window_bounds(None, (800, 600), None, (800, 600)),
            ([0.0, 0.0, 800.0, 600.0], [0.0, 0.0, 800.0, 600.0])
        );
    }

    #[test]
    fn only_systems_that_ask_through_the_event_loop_pause_it() {
        let adapter = UiAccessAdapter::inert();
        assert!(!adapter.answers_on_event_loop());
        adapter.core.shared.active.store(true, Ordering::Release);
        assert_eq!(
            adapter.answers_on_event_loop(),
            cfg!(any(target_os = "macos", target_os = "windows"))
        );
    }
}
