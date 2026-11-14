//! The Windows and Linux accessibility adapters, asked what a screen reader
//! asks, through the system's own accessibility API.
//!
//! The guide's sample menu (`examples/ui_menu.rs`) runs on a thread of its
//! own, the way the engine runs it: the UI's frame, the game's reaction to
//! its events, then the adapter's update. The test is the screen reader: it
//! finds the window through the system, reads roles, names, values and
//! states, and operates the menu (press, set a value, move focus, open and
//! close the quit dialog), then checks what the game did through the same
//! API.
//!
//! - **Windows** (UI Automation, what Narrator and NVDA use): the menu's
//!   thread owns a Win32 window that is never shown, with the adapter on
//!   it, and dispatches its messages; the test is a UI Automation client in
//!   the same process. Runs wherever the tests run on Windows.
//! - **Linux** (AT-SPI, what Orca uses): the adapter registers with the
//!   accessibility bus; the test reads it over D-Bus like any assistive
//!   program, and also turns accessibility off and on again (a screen
//!   reader quitting and starting). It needs a session bus with the AT-SPI
//!   bus launcher (`at-spi2-core`), so it runs only with
//!   `PONKO_ACCESS_ATSPI=1`; CI runs it under `dbus-run-session`.
//!
//! What this does not cover is a screen reader's own speech and cursor:
//! the guide's "Screen reader records" page says what was checked by hand.
//! On macOS `ui_access_macos` does the same through `NSAccessibility`; there
//! this file has no tests.

/// The guide's sample menu: its nodes and what it does with an event,
/// without its `main`.
#[cfg(all(feature = "accessibility", any(target_os = "windows", target_os = "linux")))]
#[allow(dead_code)]
#[path = "../examples/ui_menu.rs"]
mod sample;

/// The menu running on its own thread, as the engine runs it.
#[cfg(all(feature = "accessibility", any(target_os = "windows", target_os = "linux")))]
mod game {
    use super::sample;
    use ponko::prelude::*;
    use ponko::ui::{Ui, UiAccessAdapter, UiInput};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    const VIEW: Vec2 = Vec2::new(520.0, 460.0);

    /// The menu's thread: stopped and joined on drop.
    pub struct Running {
        stop: Arc<AtomicBool>,
        active: Arc<AtomicBool>,
        focus: Arc<Mutex<String>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Running {
        /// Starts the menu. `make` runs on the menu's thread and returns the
        /// adapter with whatever the platform side needs kept there;
        /// `pump` runs there before each frame (Windows: dispatch the
        /// window's messages, which is how UI Automation's questions reach
        /// the adapter).
        pub fn start<P: 'static>(
            make: impl FnOnce() -> (UiAccessAdapter, P) + Send + 'static,
            pump: fn(&mut P),
        ) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let active = Arc::new(AtomicBool::new(false));
            let focus = Arc::new(Mutex::new(String::new()));
            let (stop_menu, active_menu, focus_menu) = (stop.clone(), active.clone(), focus.clone());
            let thread = std::thread::spawn(move || {
                let (mut adapter, mut platform) = make();
                adapter.set_window_focused(true);
                let mut ui = Ui::default();
                let menu = sample::build(&mut ui);
                while !stop_menu.load(Ordering::Acquire) {
                    pump(&mut platform);
                    ui.frame(VIEW, &UiInput::default());
                    while let Some(event) = ui.next_event() {
                        sample::react(&mut ui, &menu, event);
                    }
                    adapter.update(&mut ui, 1.0);
                    active_menu.store(adapter.is_active(), Ordering::Release);
                    let tree = ui.accessibility_tree();
                    let focused = tree.focus().and_then(|id| tree.get(id)).map(|n| n.name.as_str());
                    *focus_menu.lock().unwrap() = focused.unwrap_or_default().to_owned();
                    std::thread::sleep(Duration::from_millis(5));
                }
                drop(adapter);
                drop(platform);
            });
            Self {
                stop,
                active,
                focus,
                thread: Some(thread),
            }
        }

        /// The adapter's [`UiAccessAdapter::is_active`], as of its last frame.
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        pub fn active(&self) -> bool {
            self.active.load(Ordering::Acquire)
        }
    }

    impl Running {
        /// The name of the node the UI itself has focused, as of its last
        /// frame: what a request to move focus must change.
        pub fn focus(&self) -> String {
            self.focus.lock().unwrap().clone()
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let result = thread.join();
                if !std::thread::panicking() {
                    result.expect("the menu's thread panicked");
                }
            }
        }
    }
}

/// How long the system may take to show a change.
#[cfg(all(feature = "accessibility", any(target_os = "windows", target_os = "linux")))]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(all(feature = "accessibility", target_os = "linux"))]
mod atspi {
    //! AT-SPI over D-Bus, as Orca reads it.
    use super::{TIMEOUT, game::Running};
    use atspi_proxies::accessible::AccessibleProxy;
    use atspi_proxies::action::ActionProxy;
    use atspi_proxies::bus::{BusProxy, StatusProxy};
    use atspi_proxies::common::{Role, State, StateSet};
    use atspi_proxies::component::ComponentProxy;
    use atspi_proxies::value::ValueProxy;
    use ponko::ui::UiAccessAdapter;
    use std::time::{Duration, Instant};
    use zbus::Connection;
    use zbus::proxy::{Builder, CacheProperties, Defaults};

    const POLL: Duration = Duration::from_millis(20);

    /// An object on the accessibility bus.
    #[derive(Clone, Debug)]
    struct Obj {
        bus: String,
        path: String,
    }

    /// One node as a screen reader reads it.
    #[derive(Clone, Debug)]
    struct Read {
        obj: Obj,
        role: Role,
        name: String,
        states: StateSet,
    }

    /// A proxy for one interface of `obj`. Properties are read fresh each
    /// time: the checks wait for them to change.
    async fn at<'a, P>(conn: &Connection, obj: &Obj) -> zbus::Result<P>
    where
        P: Defaults + From<zbus::Proxy<'a>>,
    {
        Builder::<P>::new(conn)
            .destination(obj.bus.clone())?
            .path(obj.path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    async fn children(conn: &Connection, obj: &Obj) -> zbus::Result<Vec<Obj>> {
        let accessible: AccessibleProxy<'_> = at(conn, obj).await?;
        Ok(accessible
            .get_children()
            .await?
            .iter()
            .filter(|child| !child.is_null())
            .map(|child| Obj {
                bus: child.name_as_str().unwrap_or(obj.bus.as_str()).to_owned(),
                path: child.path_as_str().to_owned(),
            })
            .collect())
    }

    async fn read(conn: &Connection, obj: &Obj) -> zbus::Result<Read> {
        let accessible: AccessibleProxy<'_> = at(conn, obj).await?;
        Ok(Read {
            obj: obj.clone(),
            role: accessible.get_role().await?,
            name: accessible.name().await?,
            states: accessible.get_state().await?,
        })
    }

    /// The game's window: the frame named after it, under an application
    /// the registry lists.
    async fn find_window(conn: &Connection) -> zbus::Result<Option<Obj>> {
        let registry = Obj {
            bus: "org.a11y.atspi.Registry".into(),
            path: "/org/a11y/atspi/accessible/root".into(),
        };
        for app in children(conn, &registry).await? {
            let Ok(windows) = children(conn, &app).await else {
                continue;
            };
            for window in windows {
                if let Ok(node) = read(conn, &window).await
                    && node.role == Role::Frame
                    && node.name == "Sample menu"
                {
                    return Ok(Some(window));
                }
            }
        }
        Ok(None)
    }

    /// Every node under `window`, depth-first in tree order.
    async fn walk(conn: &Connection, window: &Obj) -> zbus::Result<Vec<Read>> {
        let mut out = Vec::new();
        let mut stack = vec![window.clone()];
        while let Some(obj) = stack.pop() {
            out.push(read(conn, &obj).await?);
            let mut kids = children(conn, &obj).await?;
            kids.reverse();
            stack.extend(kids);
        }
        Ok(out)
    }

    /// The node with this role and name, if the window shows one.
    async fn find(conn: &Connection, window: &Obj, role: Role, name: &str) -> zbus::Result<Option<Read>> {
        let all = walk(conn, window).await?;
        Ok(all.into_iter().find(|n| n.role == role && n.name == name))
    }

    /// Waits until `$body` (an `Option`, may `.await`) is `Some`.
    macro_rules! eventually {
        ($what:expr, $body:expr) => {{
            let start = Instant::now();
            loop {
                if let Some(value) = $body {
                    break value;
                }
                assert!(start.elapsed() < TIMEOUT, "timed out waiting for {}", $what);
                async_io::Timer::after(POLL).await;
            }
        }};
    }

    /// Waits for a node with this role and name.
    macro_rules! node {
        ($conn:expr, $window:expr, $role:expr, $name:expr) => {
            eventually!(
                format!("{:?} {:?}", $role, $name),
                find($conn, $window, $role, $name).await.ok().flatten()
            )
        };
    }

    #[test]
    fn the_sample_menu_is_read_and_operated_through_at_spi() {
        if std::env::var_os("PONKO_ACCESS_ATSPI").is_none() {
            eprintln!("skipped: needs an AT-SPI bus; run with PONKO_ACCESS_ATSPI=1 under dbus-run-session");
            return;
        }
        async_io::block_on(async {
            // A screen reader starting is accessibility turning on.
            let session = Connection::session().await.expect("a session bus");
            let status = StatusProxy::new(&session).await.unwrap();
            status.set_is_enabled(true).await.unwrap();
            let address = BusProxy::new(&session).await.unwrap().get_address().await.unwrap();
            let conn = zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .build()
                .await
                .expect("the accessibility bus");

            let menu = Running::start(|| (UiAccessAdapter::for_atspi("Sample menu"), ()), |_| {});
            let window = eventually!("the window on the bus", find_window(&conn).await.ok().flatten());
            assert!(menu.active());

            // Read: roles, names, values, states.
            let all = walk(&conn, &window).await.unwrap();
            let names: Vec<(Role, &str)> = all.iter().map(|n| (n.role, n.name.as_str())).collect();
            for want in [
                (Role::Label, "Ready"),
                (Role::Button, "Play"),
                (Role::Slider, "Volume"),
                (Role::Entry, "Player name"),
                (Role::CheckBox, "Music"),
                (Role::Button, "Quit"),
            ] {
                assert!(names.contains(&want), "{want:?} is not in {names:#?}");
            }
            let play = node!(&conn, &window, Role::Button, "Play");
            let accessible: AccessibleProxy<'_> = at(&conn, &play.obj).await.unwrap();
            assert_eq!(accessible.description().await.unwrap(), "Starts a new run");
            assert!(play.states.contains(State::Enabled) && play.states.contains(State::Focusable));
            let music = node!(&conn, &window, Role::CheckBox, "Music");
            assert!(music.states.contains(State::Checked));
            let volume = node!(&conn, &window, Role::Slider, "Volume");
            let value: ValueProxy<'_> = at(&conn, &volume.obj).await.unwrap();
            assert!((value.current_value().await.unwrap() - 0.5).abs() < 1e-6);
            assert!((value.maximum_value().await.unwrap() - 1.0).abs() < 1e-6);

            // Press Play: the game reacts, and its status line says so.
            let action: ActionProxy<'_> = at(&conn, &play.obj).await.unwrap();
            assert!(action.do_action(0).await.unwrap());
            node!(&conn, &window, Role::Label, "Started a run for Ann");

            // Set the slider: the game hears a change.
            value.set_current_value(0.8).await.unwrap();
            node!(&conn, &window, Role::Label, "Volume 80%");

            // Move focus: the UI follows, and the system reports it.
            let component: ComponentProxy<'_> = at(&conn, &music.obj).await.unwrap();
            assert!(component.grab_focus().await.unwrap());
            eventually!(
                "Music to have focus",
                find(&conn, &window, Role::CheckBox, "Music")
                    .await
                    .ok()
                    .flatten()
                    .filter(|n| n.states.contains(State::Focused))
            );
            assert_eq!(menu.focus(), "Music");

            // Quit opens a modal dialog: only the dialog can be reached.
            let quit = node!(&conn, &window, Role::Button, "Quit");
            let action: ActionProxy<'_> = at(&conn, &quit.obj).await.unwrap();
            assert!(action.do_action(0).await.unwrap());
            let no = node!(&conn, &window, Role::Button, "No");
            eventually!(
                "the menu to be covered",
                find(&conn, &window, Role::Button, "Play")
                    .await
                    .ok()
                    .and_then(|play| play.is_none().then_some(()))
            );
            let action: ActionProxy<'_> = at(&conn, &no.obj).await.unwrap();
            assert!(action.do_action(0).await.unwrap());
            node!(&conn, &window, Role::Button, "Play");

            // The screen reader quits: the adapter stops sending. It starts
            // again: the whole tree is back.
            status.set_is_enabled(false).await.unwrap();
            eventually!("the adapter to go quiet", (!menu.active()).then_some(()));
            status.set_is_enabled(true).await.unwrap();
            let window = eventually!("the window again", find_window(&conn).await.ok().flatten());
            node!(&conn, &window, Role::Button, "Play");
            node!(&conn, &window, Role::Label, "Volume 80%");
            drop(menu);
        });
    }
}

#[cfg(all(feature = "accessibility", target_os = "windows"))]
mod uia {
    //! UI Automation, as Narrator and NVDA read it.
    use super::{TIMEOUT, game::Running};
    use ponko::ui::UiAccessAdapter;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
        RegisterClassW, SendMessageW, TranslateMessage, WM_SETFOCUS, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::{BSTR, Interface, w};

    const POLL: Duration = Duration::from_millis(20);

    extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: the arguments are the system's, passed on unchanged.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// A window that is never shown, on the calling thread.
    struct HiddenWindow(HWND);

    impl HiddenWindow {
        fn new() -> Self {
            // SAFETY: plain Win32 calls with valid arguments; the class is
            // registered once per process (a second registration fails,
            // which is fine: the first one stands).
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class = w!("PonkoAccessTest");
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.into(),
                    lpszClassName: class,
                    ..Default::default()
                });
                let window = CreateWindowExW(
                    Default::default(),
                    class,
                    w!("Sample menu"),
                    WS_OVERLAPPEDWINDOW,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    520,
                    460,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .expect("a window");
                Self(window)
            }
        }

        /// Tells the window it has keyboard focus, as the system does when
        /// it is activated (a window that is never shown can't really get
        /// it). The adapter follows the window's focus messages, so the
        /// focused node is reported as focused from then on.
        fn say_focused(&self) {
            // SAFETY: our own window, on the thread that made it.
            unsafe { SendMessageW(self.0, WM_SETFOCUS, None, None) };
        }

        /// Dispatches every waiting message: UI Automation's questions
        /// (`WM_GETOBJECT`) are answered here.
        fn pump(&mut self) {
            let mut message = MSG::default();
            // SAFETY: messages of this thread, dispatched on it.
            unsafe {
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            // SAFETY: our own window, on the thread that made it.
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    /// The window's handle, to be used by the test's own thread.
    #[derive(Clone, Copy)]
    struct Handle(isize);

    /// One node as a screen reader reads it.
    struct Read {
        element: IUIAutomationElement,
        control_type: UIA_CONTROLTYPE_ID,
        name: String,
    }

    struct Client {
        uia: IUIAutomation,
        window: HWND,
    }

    impl Client {
        fn new(window: Handle) -> Self {
            // SAFETY: COM is set up for this thread (the multithreaded
            // apartment, which UI Automation clients may use) before the
            // client object is made.
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
                let uia: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).unwrap();
                Self {
                    uia,
                    window: HWND(window.0 as *mut std::ffi::c_void),
                }
            }
        }

        /// Every element under the window, depth-first, asked afresh.
        fn walk(&self) -> windows::core::Result<Vec<Read>> {
            // SAFETY: COM calls on interfaces this client owns.
            unsafe {
                let root = self.uia.ElementFromHandle(self.window)?;
                let walker = self.uia.RawViewWalker()?;
                let mut out = Vec::new();
                let mut stack = vec![root];
                while let Some(element) = stack.pop() {
                    let mut kids = Vec::new();
                    let mut child = walker.GetFirstChildElement(&element).ok();
                    while let Some(c) = child {
                        child = walker.GetNextSiblingElement(&c).ok();
                        kids.push(c);
                    }
                    kids.reverse();
                    stack.extend(kids);
                    out.push(Read {
                        control_type: element.CurrentControlType()?,
                        name: element.CurrentName()?.to_string(),
                        element,
                    });
                }
                Ok(out)
            }
        }

        fn find(&self, control_type: UIA_CONTROLTYPE_ID, name: &str) -> Option<Read> {
            self.walk()
                .ok()?
                .into_iter()
                .find(|n| n.control_type == control_type && n.name == name)
        }

        /// Waits until `f` gives a value.
        fn eventually<T>(&self, what: &str, mut f: impl FnMut(&Self) -> Option<T>) -> T {
            let start = Instant::now();
            loop {
                if let Some(value) = f(self) {
                    return value;
                }
                assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
                std::thread::sleep(POLL);
            }
        }

        fn node(&self, control_type: UIA_CONTROLTYPE_ID, name: &str) -> IUIAutomationElement {
            self.eventually(&format!("{} {name:?}", control_type.0), |c| c.find(control_type, name))
                .element
        }

        fn names(&self) -> Vec<(i32, String)> {
            self.walk()
                .unwrap_or_default()
                .into_iter()
                .map(|n| (n.control_type.0, n.name))
                .collect()
        }
    }

    #[test]
    fn the_sample_menu_is_read_and_operated_through_ui_automation() {
        let menu = Running::start(
            || {
                let window = HiddenWindow::new();
                // SAFETY: a live window of this thread, never shown, with no
                // other adapter; the adapter is dropped first, on this
                // thread.
                let adapter = unsafe { UiAccessAdapter::for_hwnd(window.0.0, "Sample menu") };
                window.say_focused();
                let handle = Handle(window.0.0 as isize);
                *WINDOW.lock().unwrap() = Some(handle);
                (adapter, window)
            },
            HiddenWindow::pump,
        );
        let handle = wait_for_handle();
        let client = Client::new(handle);

        // Read: the first question wakes the adapter, the next frame sends
        // the tree.
        client.eventually("the menu", |c| c.find(UIA_ButtonControlTypeId, "Play"));
        let names = client.names();
        for (control_type, name) in [
            (UIA_TextControlTypeId, "Ready"),
            (UIA_ButtonControlTypeId, "Play"),
            (UIA_SliderControlTypeId, "Volume"),
            (UIA_EditControlTypeId, "Player name"),
            (UIA_CheckBoxControlTypeId, "Music"),
            (UIA_ButtonControlTypeId, "Quit"),
        ] {
            assert!(
                names.contains(&(control_type.0, name.to_owned())),
                "{} {name:?} is not in {names:#?}",
                control_type.0
            );
        }

        // SAFETY (every block below): COM calls on elements and patterns
        // this client got from UI Automation.
        let play = client.node(UIA_ButtonControlTypeId, "Play");
        unsafe {
            let play6: IUIAutomationElement6 = play.cast().unwrap();
            assert_eq!(play6.CurrentFullDescription().unwrap().to_string(), "Starts a new run");
            assert!(play.CurrentIsEnabled().unwrap().as_bool());
            assert!(play.CurrentIsKeyboardFocusable().unwrap().as_bool());
        }
        let music = client.node(UIA_CheckBoxControlTypeId, "Music");
        unsafe {
            let toggle: IUIAutomationTogglePattern = music.GetCurrentPatternAs(UIA_TogglePatternId).unwrap();
            assert_eq!(toggle.CurrentToggleState().unwrap(), ToggleState_On);
        }
        let volume = client.node(UIA_SliderControlTypeId, "Volume");
        let range: IUIAutomationRangeValuePattern =
            unsafe { volume.GetCurrentPatternAs(UIA_RangeValuePatternId).unwrap() };
        unsafe {
            assert!((range.CurrentValue().unwrap() - 0.5).abs() < 1e-6);
            assert!((range.CurrentMaximum().unwrap() - 1.0).abs() < 1e-6);
        }
        let name = client.node(UIA_EditControlTypeId, "Player name");
        unsafe {
            let value: IUIAutomationValuePattern = name.GetCurrentPatternAs(UIA_ValuePatternId).unwrap();
            assert_eq!(value.CurrentValue().unwrap().to_string(), "Ann");
        }

        // Press Play: the game reacts, and its status line says so.
        unsafe {
            let invoke: IUIAutomationInvokePattern = play.GetCurrentPatternAs(UIA_InvokePatternId).unwrap();
            invoke.Invoke().unwrap();
        }
        client.node(UIA_TextControlTypeId, "Started a run for Ann");

        // Set the slider, and replace the name: the game hears both.
        unsafe { range.SetValue(0.8).unwrap() };
        client.node(UIA_TextControlTypeId, "Volume 80%");
        unsafe {
            let value: IUIAutomationValuePattern = name.GetCurrentPatternAs(UIA_ValuePatternId).unwrap();
            value.SetValue(&BSTR::from("Bea")).unwrap();
        }
        let renamed = client.eventually("the new name", |c| {
            let name = c.find(UIA_EditControlTypeId, "Player name")?;
            // SAFETY: as above.
            let value: IUIAutomationValuePattern =
                unsafe { name.element.GetCurrentPatternAs(UIA_ValuePatternId).ok()? };
            let text = unsafe { value.CurrentValue().ok()? }.to_string();
            (text == "Bea").then_some(text)
        });
        assert_eq!(renamed, "Bea");

        // Move focus: the UI follows, and the system reports it (the
        // window had keyboard focus: see `HiddenWindow::new`).
        unsafe { music.SetFocus().unwrap() };
        client.eventually("the UI to focus Music", |_| (menu.focus() == "Music").then_some(()));
        client.eventually("Music to have focus", |c| {
            let music = c.find(UIA_CheckBoxControlTypeId, "Music")?;
            // SAFETY: as above.
            unsafe { music.element.CurrentHasKeyboardFocus().ok()? }
                .as_bool()
                .then_some(())
        });

        // Toggle Music off.
        unsafe {
            let toggle: IUIAutomationTogglePattern = music.GetCurrentPatternAs(UIA_TogglePatternId).unwrap();
            toggle.Toggle().unwrap();
        }
        client.node(UIA_TextControlTypeId, "Music off");

        // Quit opens a modal dialog: only the dialog can be reached.
        let quit = client.node(UIA_ButtonControlTypeId, "Quit");
        unsafe {
            let invoke: IUIAutomationInvokePattern = quit.GetCurrentPatternAs(UIA_InvokePatternId).unwrap();
            invoke.Invoke().unwrap();
        }
        let no = client.node(UIA_ButtonControlTypeId, "No");
        client.eventually("the menu to be covered", |c| {
            c.find(UIA_ButtonControlTypeId, "Play").is_none().then_some(())
        });
        unsafe {
            let invoke: IUIAutomationInvokePattern = no.GetCurrentPatternAs(UIA_InvokePatternId).unwrap();
            invoke.Invoke().unwrap();
        }
        client.node(UIA_ButtonControlTypeId, "Play");
        drop(menu);
    }

    /// The menu's thread made its window: its handle, for the client.
    fn wait_for_handle() -> Handle {
        let start = Instant::now();
        loop {
            if let Some(handle) = *WINDOW.lock().unwrap() {
                return handle;
            }
            assert!(start.elapsed() < TIMEOUT, "the menu's window was not made");
            std::thread::sleep(POLL);
        }
    }

    /// The menu's window, made on its thread.
    static WINDOW: std::sync::Mutex<Option<Handle>> = std::sync::Mutex::new(None);
}
