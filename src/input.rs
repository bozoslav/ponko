//! [`Input`], the input service at `ctx.input`: keyboard, mouse, gamepads,
//! named actions, typed text and the cursor lock.
//!
//! **What lives here:** what is held (`key_down`, `mouse_down`,
//! `gamepad_down`), what changed this frame (`*_pressed`, `*_released`),
//! pointer position and movement, gamepad axes, the
//! [`ActionMap`](crate::actions::ActionMap) and its queries (`action_down`,
//! `action_axis`, ...), text typed this frame, and the cursor lock.
//!
//! **When to call it:** in any system. Edges (`pressed`, `released`, wheel,
//! typed text) last one frame and are cleared after the frame's systems; a
//! simulation system that runs several ticks in one frame sees the same
//! edges in each. Install bindings with [`Input::set_actions`] in setup.
//!
//! **What it needs:** a window for live input (feature `window`), and the
//! `gamepad` feature for controllers. Headless it is silent unless you
//! [`feed`](Input::feed) it, which is how tests and replays drive a game.
//!
//! The engine records raw window events here. State lives in fixed-size
//! arrays indexed by enum discriminant, so lookups are O(1) and nothing
//! allocates per frame.

use glam::Vec2;
#[cfg(feature = "window")]
use winit::keyboard::KeyCode;

macro_rules! define_keys {
    ($($(#[$meta:meta])* $name:ident => $code:ident),* $(,)?) => {
        /// A keyboard key, identified by its **physical position** on a US layout.
        ///
        /// Physical keys keep controls in the same place on every keyboard layout:
        /// `Key::W` is the key above `S` on QWERTY, AZERTY and Dvorak alike.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[non_exhaustive]
        #[allow(missing_docs)] // named after the keys
        pub enum Key {
            $($(#[$meta])* $name),*
        }

        impl Key {
            pub(crate) const COUNT: usize = [$(Key::$name),*].len();
            /// Every key, in discriminant order (the index of `key as usize`).
            pub(crate) const ALL: [Key; Key::COUNT] = [$(Key::$name),*];

            #[cfg(feature = "window")]
            pub(crate) fn from_winit(code: KeyCode) -> Option<Key> {
                match code {
                    $(KeyCode::$code => Some(Key::$name),)*
                    _ => None,
                }
            }
        }
    };
}

define_keys! {
    A => KeyA, B => KeyB, C => KeyC, D => KeyD, E => KeyE, F => KeyF, G => KeyG,
    H => KeyH, I => KeyI, J => KeyJ, K => KeyK, L => KeyL, M => KeyM, N => KeyN,
    O => KeyO, P => KeyP, Q => KeyQ, R => KeyR, S => KeyS, T => KeyT, U => KeyU,
    V => KeyV, W => KeyW, X => KeyX, Y => KeyY, Z => KeyZ,

    /// The `0` key in the number row (not the numpad).
    Num0 => Digit0, Num1 => Digit1, Num2 => Digit2, Num3 => Digit3, Num4 => Digit4,
    Num5 => Digit5, Num6 => Digit6, Num7 => Digit7, Num8 => Digit8, Num9 => Digit9,

    F1 => F1, F2 => F2, F3 => F3, F4 => F4, F5 => F5, F6 => F6,
    F7 => F7, F8 => F8, F9 => F9, F10 => F10, F11 => F11, F12 => F12,

    Up => ArrowUp, Down => ArrowDown, Left => ArrowLeft, Right => ArrowRight,

    Space => Space, Enter => Enter, Escape => Escape, Tab => Tab,
    Backspace => Backspace, CapsLock => CapsLock,
    Insert => Insert, Delete => Delete, Home => Home, End => End,
    PageUp => PageUp, PageDown => PageDown,

    LeftShift => ShiftLeft, RightShift => ShiftRight,
    LeftCtrl => ControlLeft, RightCtrl => ControlRight,
    LeftAlt => AltLeft, RightAlt => AltRight,
    /// Cmd on macOS, the Windows key on Windows.
    LeftSuper => SuperLeft,
    /// Cmd on macOS, the Windows key on Windows.
    RightSuper => SuperRight,

    Minus => Minus, Equal => Equal,
    LeftBracket => BracketLeft, RightBracket => BracketRight, Backslash => Backslash,
    Semicolon => Semicolon, Apostrophe => Quote, Backquote => Backquote,
    Comma => Comma, Period => Period, Slash => Slash,

    Numpad0 => Numpad0, Numpad1 => Numpad1, Numpad2 => Numpad2, Numpad3 => Numpad3,
    Numpad4 => Numpad4, Numpad5 => Numpad5, Numpad6 => Numpad6, Numpad7 => Numpad7,
    Numpad8 => Numpad8, Numpad9 => Numpad9,
    NumpadAdd => NumpadAdd, NumpadSubtract => NumpadSubtract,
    NumpadMultiply => NumpadMultiply, NumpadDivide => NumpadDivide,
    NumpadDecimal => NumpadDecimal, NumpadEnter => NumpadEnter,
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum MouseButton {
    /// The left (primary) button.
    Left,
    /// The right (secondary) button.
    Right,
    /// The wheel, pressed.
    Middle,
    /// The "back" side button found on many mice.
    Back,
    /// The "forward" side button found on many mice.
    Forward,
}

impl MouseButton {
    pub(crate) const COUNT: usize = 5;
    /// Every button, in discriminant order.
    pub(crate) const ALL: [MouseButton; Self::COUNT] = [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Back,
        MouseButton::Forward,
    ];

    #[cfg(feature = "window")]
    pub(crate) fn from_winit(button: winit::event::MouseButton) -> Option<MouseButton> {
        use winit::event::MouseButton as W;
        match button {
            W::Left => Some(MouseButton::Left),
            W::Right => Some(MouseButton::Right),
            W::Middle => Some(MouseButton::Middle),
            W::Back => Some(MouseButton::Back),
            W::Forward => Some(MouseButton::Forward),
            W::Other(_) => None,
        }
    }
}

/// A gamepad button, named by its position, so it means the same on every
/// brand of controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum GamepadButton {
    /// The bottom face button: A on Xbox, Cross on PlayStation, B on Nintendo.
    South,
    /// The right face button: B on Xbox, Circle on PlayStation, A on Nintendo.
    East,
    /// The top face button: Y on Xbox, Triangle on PlayStation, X on Nintendo.
    North,
    /// The left face button: X on Xbox, Square on PlayStation, Y on Nintendo.
    West,
    /// The upper shoulder button on the left (LB, L1, L).
    LeftBumper,
    /// The upper shoulder button on the right (RB, R1, R).
    RightBumper,
    /// The lower shoulder trigger on the left (LT, L2, ZL), pulled more than
    /// halfway. For how far, read [`GamepadAxis::LeftTrigger`].
    LeftTrigger,
    /// The lower shoulder trigger on the right (RT, R2, ZR), pulled more than
    /// halfway. For how far, read [`GamepadAxis::RightTrigger`].
    RightTrigger,
    /// The small button left of center (View, Share, Minus).
    Select,
    /// The small button right of center (Menu, Options, Plus).
    Start,
    /// Pressing the left stick in.
    LeftStick,
    /// Pressing the right stick in.
    RightStick,
    /// The d-pad, up.
    DPadUp,
    /// The d-pad, down.
    DPadDown,
    /// The d-pad, left.
    DPadLeft,
    /// The d-pad, right.
    DPadRight,
}

impl GamepadButton {
    pub(crate) const COUNT: usize = 16;
    /// Every button, in discriminant order.
    pub(crate) const ALL: [GamepadButton; Self::COUNT] = [
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::North,
        GamepadButton::West,
        GamepadButton::LeftBumper,
        GamepadButton::RightBumper,
        GamepadButton::LeftTrigger,
        GamepadButton::RightTrigger,
        GamepadButton::Select,
        GamepadButton::Start,
        GamepadButton::LeftStick,
        GamepadButton::RightStick,
        GamepadButton::DPadUp,
        GamepadButton::DPadDown,
        GamepadButton::DPadLeft,
        GamepadButton::DPadRight,
    ];
}

/// One axis of a gamepad: a stick's x or y (`-1.0..=1.0`, y down is
/// positive like screen coordinates) or a trigger (`0.0..=1.0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum GamepadAxis {
    /// Left stick, left (-1) to right (1).
    LeftStickX,
    /// Left stick, up (-1) to down (1).
    LeftStickY,
    /// Right stick, left (-1) to right (1).
    RightStickX,
    /// Right stick, up (-1) to down (1).
    RightStickY,
    /// Left trigger, released (0) to fully pulled (1).
    LeftTrigger,
    /// Right trigger, released (0) to fully pulled (1).
    RightTrigger,
}

impl GamepadAxis {
    pub(crate) const COUNT: usize = 6;
    /// Every axis, in discriminant order.
    pub(crate) const ALL: [GamepadAxis; Self::COUNT] = [
        GamepadAxis::LeftStickX,
        GamepadAxis::LeftStickY,
        GamepadAxis::RightStickX,
        GamepadAxis::RightStickY,
        GamepadAxis::LeftTrigger,
        GamepadAxis::RightTrigger,
    ];
}

/// Which stick, for [`Input::gamepad_stick`](crate::input::Input::gamepad_stick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GamepadStick {
    /// The left stick.
    Left,
    /// The right stick.
    Right,
}

/// A connected gamepad, from [`Input::gamepads`](crate::input::Input::gamepads). It
/// keeps its slot while connected, so `pads[0]` stays player one; a pad that
/// reconnects takes the lowest free slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Gamepad(pub(crate) u8);
impl TryFrom<u8> for Gamepad {
    type Error = &'static str;
    fn try_from(slot: u8) -> Result<Self, Self::Error> {
        Self::from_slot(usize::from(slot)).ok_or("gamepad slot out of range")
    }
}
impl From<Gamepad> for u8 {
    fn from(pad: Gamepad) -> u8 {
        pad.0
    }
}

impl Gamepad {
    /// The pad's slot, `0..MAX_GAMEPADS`: a stable number for "player 1, 2, ...".
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// for pad in ctx.input.gamepads() {
    ///     println!("player {} is connected", pad.slot() + 1);
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn slot(self) -> usize {
        usize::from(self.0)
    }

    /// The pad in `slot`, whether or not one is connected there (queries
    /// on an empty slot read as nothing pressed). `None` past
    /// [`MAX_GAMEPADS`].
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let player_two = ponko::input::Gamepad::from_slot(1).unwrap();
    /// assert_eq!(player_two.slot(), 1);
    /// assert_eq!(ponko::input::Gamepad::from_slot(ponko::input::MAX_GAMEPADS), None);
    /// ```
    #[must_use]
    pub fn from_slot(slot: usize) -> Option<Gamepad> {
        (slot < MAX_GAMEPADS).then_some(Gamepad(slot as u8))
    }
}

/// How many gamepads the engine tracks at once.
pub const MAX_GAMEPADS: usize = 8;

/// One gamepad's buttons and axes.
#[derive(Debug, Clone)]
pub(super) struct PadState {
    pub(super) buttons: ButtonStates<{ GamepadButton::COUNT }>,
    pub(super) axes: [f32; GamepadAxis::COUNT],
    /// The axes when the frame started: what "went past a threshold this
    /// frame" is measured against.
    axes_before: [f32; GamepadAxis::COUNT],
    /// The value of greatest magnitude each axis had this frame, so a stick
    /// flicked out and back within one frame still counts.
    axes_peak: [f32; GamepadAxis::COUNT],
}

impl PadState {
    const fn new() -> Self {
        Self {
            buttons: ButtonStates::new(),
            axes: [0.0; GamepadAxis::COUNT],
            axes_before: [0.0; GamepadAxis::COUNT],
            axes_peak: [0.0; GamepadAxis::COUNT],
        }
    }

    fn end_frame(&mut self) {
        self.buttons.end_frame();
        self.axes_before = self.axes;
        self.axes_peak = self.axes;
    }
}

/// Down / pressed-this-frame / released-this-frame flags for `N` buttons.
#[derive(Debug, Clone)]
pub(super) struct ButtonStates<const N: usize> {
    pub(super) down: [bool; N],
    pressed: [bool; N],
    released: [bool; N],
}

impl<const N: usize> ButtonStates<N> {
    const fn new() -> Self {
        Self {
            down: [false; N],
            pressed: [false; N],
            released: [false; N],
        }
    }

    fn set(&mut self, i: usize, is_down: bool) {
        match (self.down[i], is_down) {
            (false, true) => self.pressed[i] = true,
            (true, false) => self.released[i] = true,
            _ => {}
        }
        self.down[i] = is_down;
    }

    fn release_all(&mut self) {
        for i in 0..N {
            self.set(i, false);
        }
    }

    fn end_frame(&mut self) {
        self.pressed = [false; N];
        self.released = [false; N];
    }
}

/// Keyboard, mouse, gamepads and named actions this frame: `ctx.input`.
///
/// The everyday reads also have shortcuts on [`Ctx`](crate::ctx::Ctx)
/// (`ctx.key_down`, `ctx.key_pressed`, `ctx.key_released`, `ctx.mouse_pos`);
/// everything else is here.
///
/// - **What lives here:** what is held (`key_down`, `mouse_down`,
///   `gamepad_down`), what changed this frame (`*_pressed`, `*_released`),
///   the pointer, gamepad axes, the [`ActionMap`](crate::actions::ActionMap) and its
///   queries, text typed this frame and the cursor lock.
/// - **When to call it:** in any system. Edges last one frame; a simulation
///   system that runs several ticks in a frame sees the same edges in each.
///   Install bindings with [`set_actions`](Self::set_actions) in setup.
/// - **What it needs:** a window for live input (feature `window`) and the
///   `gamepad` feature for controllers. Headless it is silent unless you
///   [`feed`](Self::feed) it, which is how tests and replays drive a game.
///
/// ```
/// use ponko::prelude::*;
///
/// Game::new("t", 1, 1).run_headless(1, |ctx| {
///     ctx.input.feed(ponko::actions::InputEvent::Key(Key::Space, true));
///     assert!(ctx.input.key_pressed(Key::Space));
///     assert!(ctx.input.key_down(Key::Space));
/// });
/// ```
#[derive(Debug, Clone)]
pub struct Input {
    pub(super) keys: ButtonStates<{ Key::COUNT }>,
    pub(super) mouse: ButtonStates<{ MouseButton::COUNT }>,
    mouse_pos: Vec2,
    mouse_pos_at_frame_start: Vec2,
    mouse_wheel: Vec2,
    typed_text: String,
    /// Raw mouse movement this frame (also while the cursor is locked).
    mouse_motion: Vec2,
    pub(super) pads: [PadState; MAX_GAMEPADS],
    /// Connected pads, sorted by slot.
    connected: Vec<Gamepad>,
    pub(crate) actions: crate::actions::ActionMap,
    /// The cursor is hidden and held in the window (`set_cursor_locked`).
    cursor_locked: bool,
    /// `cursor_locked` changed since the window last applied it.
    pub(crate) cursor_lock_changed: bool,
    /// Running input scenario and whether tools may start one.
    pub(crate) scenarios: crate::scenario::Scenarios,
}

impl Input {
    pub(crate) fn new() -> Self {
        Self {
            keys: ButtonStates::new(),
            mouse: ButtonStates::new(),
            mouse_pos: Vec2::ZERO,
            mouse_pos_at_frame_start: Vec2::ZERO,
            mouse_wheel: Vec2::ZERO,
            typed_text: String::new(),
            mouse_motion: Vec2::ZERO,
            pads: [const { PadState::new() }; MAX_GAMEPADS],
            connected: Vec::with_capacity(MAX_GAMEPADS),
            actions: crate::actions::ActionMap::default(),
            cursor_locked: false,
            cursor_lock_changed: false,
            scenarios: Default::default(),
        }
    }

    // --- event recording (engine side) ---

    pub(crate) fn set_key(&mut self, key: Key, is_down: bool) {
        self.keys.set(key as usize, is_down);
        self.refresh_actions();
    }

    pub(crate) fn set_mouse_button(&mut self, button: MouseButton, is_down: bool) {
        self.mouse.set(button as usize, is_down);
        self.refresh_actions();
    }

    pub(crate) fn set_mouse_pos(&mut self, pos: Vec2) {
        self.mouse_pos = pos;
    }

    pub(crate) fn add_text(&mut self, text: &str) {
        self.typed_text.push_str(text);
    }

    /// Committed Unicode typed this frame, also readable when UI has focus.
    /// Cleared with other input after frame systems. Does not include IME
    /// composition/preedit. Physical keys remain separate.
    /// ```
    /// use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// let text = ctx.input.typed_text();
    /// # let _ = text;
    /// # }
    /// ```
    #[must_use]
    pub fn typed_text(&self) -> &str {
        &self.typed_text
    }

    pub(crate) fn add_mouse_wheel(&mut self, delta: Vec2) {
        self.mouse_wheel += delta;
    }

    pub(crate) fn add_mouse_motion(&mut self, delta: Vec2) {
        self.mouse_motion += delta;
    }

    /// A pad was plugged in: it takes the lowest free slot. `None` when all
    /// [`MAX_GAMEPADS`] slots are taken.
    // Only the gilrs backend plugs pads in (and the unit tests).
    #[cfg_attr(not(feature = "gamepad"), allow(dead_code))]
    pub(crate) fn connect_gamepad(&mut self) -> Option<Gamepad> {
        let slot = (0..MAX_GAMEPADS as u8).find(|&s| !self.connected.contains(&Gamepad(s)))?;
        let pad = Gamepad(slot);
        self.pads[usize::from(slot)] = PadState::new();
        let at = self.connected.partition_point(|p| p.0 < slot);
        self.connected.insert(at, pad);
        self.refresh_actions();
        Some(pad)
    }

    /// A pad was unplugged: its held buttons count as released this frame.
    pub(crate) fn disconnect_gamepad(&mut self, pad: Gamepad) {
        self.connected.retain(|&p| p != pad);
        let state = &mut self.pads[pad.slot()];
        state.buttons.release_all();
        state.axes = [0.0; GamepadAxis::COUNT];
        state.axes_peak = [0.0; GamepadAxis::COUNT];
        self.refresh_actions();
    }

    pub(crate) fn set_gamepad_button(&mut self, pad: Gamepad, button: GamepadButton, is_down: bool) {
        self.pads[pad.slot()].buttons.set(button as usize, is_down);
        self.refresh_actions();
    }

    pub(crate) fn set_gamepad_axis(&mut self, pad: Gamepad, axis: GamepadAxis, value: f32) {
        let state = &mut self.pads[pad.slot()];
        state.axes[axis as usize] = value;
        if value.abs() > state.axes_peak[axis as usize].abs() {
            state.axes_peak[axis as usize] = value;
        }
        self.refresh_actions();
    }

    /// Called when the window loses focus: we will never see the matching
    /// key-up events, so treat everything as released to avoid stuck keys.
    pub(crate) fn release_all(&mut self) {
        self.keys.release_all();
        self.mouse.release_all();
        self.refresh_actions();
    }

    /// Clears per-frame state. Called after the game's update for the frame.
    pub(crate) fn end_frame(&mut self) {
        self.actions.end_frame();
        self.keys.end_frame();
        self.mouse.end_frame();
        for pad in &mut self.pads {
            pad.end_frame();
        }
        self.typed_text.clear();
        self.mouse_wheel = Vec2::ZERO;
        self.mouse_motion = Vec2::ZERO;
        self.mouse_pos_at_frame_start = self.mouse_pos;
    }

    pub(crate) fn refresh_actions(&mut self) {
        self.actions.refresh(&self.keys, &self.mouse, &self.pads);
    }

    // --- queries (exposed through Ctx) ---

    /// `true` while `key` is held down.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pos: &mut Vec2) {
    /// if ctx.input.key_down(Key::Right) { pos.x += 200.0 * ctx.time.dt(); }
    /// # }
    /// ```
    #[must_use]
    pub fn key_down(&self, key: Key) -> bool {
        self.keys.down[key as usize]
    }

    /// `true` only on the frame `key` went down. Good for jumps, menus, toggles.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, paused: &mut bool) {
    /// if ctx.input.key_pressed(Key::P) { *paused = !*paused; }
    /// # }
    /// ```
    #[must_use]
    pub fn key_pressed(&self, key: Key) -> bool {
        self.keys.pressed[key as usize]
    }

    /// `true` only on the frame `key` was let go.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, charging: &mut bool) {
    /// if ctx.input.key_released(Key::Space) { *charging = false; }
    /// # }
    /// ```
    #[must_use]
    pub fn key_released(&self, key: Key) -> bool {
        self.keys.released[key as usize]
    }

    /// `true` while `button` is held down.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// let dragging = ctx.input.mouse_down(MouseButton::Left);
    /// # }
    /// ```
    #[must_use]
    pub fn mouse_down(&self, button: MouseButton) -> bool {
        self.mouse.down[button as usize]
    }

    /// `true` only on the frame `button` went down.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// if ctx.input.mouse_pressed(MouseButton::Left) { println!("click at {}", ctx.input.mouse_pos()); }
    /// # }
    /// ```
    #[must_use]
    pub fn mouse_pressed(&self, button: MouseButton) -> bool {
        self.mouse.pressed[button as usize]
    }

    /// `true` only on the frame `button` was let go.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// if ctx.input.mouse_released(MouseButton::Left) { println!("drop"); }
    /// # }
    /// ```
    #[must_use]
    pub fn mouse_released(&self, button: MouseButton) -> bool {
        self.mouse.released[button as usize]
    }

    /// Mouse position in window pixels. `(0, 0)` is the top-left corner,
    /// `y` grows downward.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// let target = ctx.input.mouse_pos();
    /// # }
    /// ```
    #[must_use]
    pub fn mouse_pos(&self) -> Vec2 {
        self.mouse_pos
    }

    /// How far the mouse moved since the previous frame, in window pixels.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, camera: &mut Vec2) {
    /// if ctx.input.mouse_down(MouseButton::Right) { *camera -= ctx.input.mouse_delta(); }
    /// # }
    /// ```
    #[must_use]
    pub fn mouse_delta(&self) -> Vec2 {
        self.mouse_pos - self.mouse_pos_at_frame_start
    }

    /// Scroll wheel movement this frame. `y > 0` means scrolled up / away from
    /// the user. Roughly one unit per wheel notch.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, zoom: &mut f32) {
    /// *zoom *= 1.1_f32.powf(ctx.input.mouse_wheel().y);
    /// # }
    /// ```
    #[must_use]
    pub fn mouse_wheel(&self) -> Vec2 {
        self.mouse_wheel
    }

    /// How far the mouse itself moved this frame, in the system's raw
    /// mouse units (about a pixel each, without pointer acceleration). Unlike
    /// [`mouse_delta`](Self::mouse_delta) it keeps coming while the cursor is
    /// locked and at the window's edge: use it to turn a first-person camera.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn look(ctx: &mut Ctx, yaw: &mut f32, pitch: &mut f32) {
    /// let turn = ctx.input.mouse_motion() * 0.002; // radians per unit
    /// *yaw -= turn.x;
    /// *pitch = (*pitch - turn.y).clamp(-1.5, 1.5);
    /// # }
    /// ```
    #[must_use]
    pub fn mouse_motion(&self) -> Vec2 {
        self.mouse_motion
    }

    /// The connected gamepads, by slot (player one first). Empty when none
    /// are plugged in, in headless runs, and without the `gamepad` feature.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pos: &mut Vec2) {
    /// if let Some(&pad) = ctx.input.gamepads().first() {
    ///     *pos += ctx.input.gamepad_stick(pad, ponko::input::GamepadStick::Left) * 300.0 * ctx.time.dt();
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn gamepads(&self) -> &[Gamepad] {
        &self.connected
    }

    /// `true` while `button` is held on `pad`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pad: ponko::input::Gamepad) {
    /// let running = ctx.input.gamepad_down(pad, ponko::input::GamepadButton::West);
    /// # }
    /// ```
    #[must_use]
    pub fn gamepad_down(&self, pad: Gamepad, button: GamepadButton) -> bool {
        self.pads[pad.slot()].buttons.down[button as usize]
    }

    /// `true` only on the frame `button` went down on `pad`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pad: ponko::input::Gamepad) {
    /// if ctx.input.gamepad_pressed(pad, ponko::input::GamepadButton::South) { println!("jump"); }
    /// # }
    /// ```
    #[must_use]
    pub fn gamepad_pressed(&self, pad: Gamepad, button: GamepadButton) -> bool {
        self.pads[pad.slot()].buttons.pressed[button as usize]
    }

    /// `true` only on the frame `button` was let go on `pad` (or `pad` was
    /// unplugged while it was held).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pad: ponko::input::Gamepad) {
    /// if ctx.input.gamepad_released(pad, ponko::input::GamepadButton::RightTrigger) { println!("fire!"); }
    /// # }
    /// ```
    #[must_use]
    pub fn gamepad_released(&self, pad: Gamepad, button: GamepadButton) -> bool {
        self.pads[pad.slot()].buttons.released[button as usize]
    }

    /// One axis of `pad`: sticks from `-1.0` to `1.0` (y down is positive,
    /// like screen coordinates), triggers from `0.0` to `1.0`. Small
    /// movements around the center read as `0.0`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pad: ponko::input::Gamepad) {
    /// let throttle = ctx.input.gamepad_axis(pad, ponko::input::GamepadAxis::RightTrigger);
    /// # }
    /// ```
    #[must_use]
    pub fn gamepad_axis(&self, pad: Gamepad, axis: GamepadAxis) -> f32 {
        self.pads[pad.slot()].axes[axis as usize]
    }
}

impl Input {
    /// Everything that went down this frame, as the bindings an
    /// [`ActionMap`](crate::actions::ActionMap) takes: keys, mouse buttons,
    /// gamepad buttons, and gamepad axes that moved past
    /// [`PRESS_THRESHOLD`](crate::actions::PRESS_THRESHOLD) this frame. A
    /// rebinding screen asks "what did the player just press?" with this
    /// instead of polling every key it is willing to offer.
    ///
    /// - **Edges only.** A held key does not appear again next frame. A tap
    ///   that goes down and up within one frame still appears, as with
    ///   [`key_pressed`](Self::key_pressed). A stick or trigger counts when it
    ///   was below the threshold when the frame started and reached it during
    ///   the frame.
    /// - **Order.** Keys, then mouse buttons, then each gamepad slot's
    ///   buttons, then axes: the same order every run, so the first item is
    ///   a stable answer. Within a kind it is the order of the enum.
    /// - **Bindable as is.** Items are [`PressedInput::Button`](crate::actions::PressedInput::Button) (put it in an
    ///   `Action::Button`) or [`PressedInput::Axis`](crate::actions::PressedInput::Axis) (an `Action::Axis`, with
    ///   the deadzone [`DEFAULT_DEADZONE`](crate::actions::DEFAULT_DEADZONE)).
    /// - **No allocation.** It is a scan of fixed arrays.
    ///
    /// Like every edge it lasts one frame, and a simulation system that runs
    /// several ticks in a frame sees the same answer in each.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::actions::InputEvent;
    ///
    /// Game::new("rebind", 1, 1).run_headless(1, |ctx| {
    ///     ctx.input.feed(InputEvent::Key(Key::J, true));
    ///     let pressed: Vec<PressedInput> = ctx.input.pressed_this_frame().collect();
    ///     assert_eq!(pressed, [PressedInput::Button(ButtonBinding::Key(Key::J))]);
    ///
    ///     // Bind it directly.
    ///     if let Some(PressedInput::Button(binding)) = pressed.into_iter().next() {
    ///         ctx.input.set_action("jump", Action::Button(vec![binding]));
    ///     }
    ///     assert!(ctx.input.action_down("jump"));
    /// });
    /// ```
    pub fn pressed_this_frame(&self) -> impl Iterator<Item = crate::actions::PressedInput> + '_ {
        self.pressed_this_frame_with_threshold(crate::actions::PRESS_THRESHOLD)
    }

    /// [`pressed_this_frame`](Self::pressed_this_frame) with your own axis
    /// threshold, in `0.0..=1.0` (a value at or below zero reports any axis
    /// that left zero). Buttons and keys do not depend on it.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::actions::InputEvent;
    /// use ponko::input::{Gamepad, GamepadAxis};
    ///
    /// Game::new("trigger", 1, 1).run_headless(1, |ctx| {
    ///     let pad = Gamepad::from_slot(0).unwrap();
    ///     ctx.input.feed(InputEvent::GamepadAxis(pad, GamepadAxis::RightTrigger, 0.3));
    ///     assert_eq!(ctx.input.pressed_this_frame().count(), 0, "under the default 0.5");
    ///     assert_eq!(ctx.input.pressed_this_frame_with_threshold(0.25).count(), 1);
    /// });
    /// ```
    pub fn pressed_this_frame_with_threshold(
        &self,
        axis_threshold: f32,
    ) -> impl Iterator<Item = crate::actions::PressedInput> + '_ {
        use crate::actions::{AxisBinding, ButtonBinding, DEFAULT_DEADZONE, PressedInput};
        let keys = Key::ALL
            .into_iter()
            .filter(|&k| self.keys.pressed[k as usize])
            .map(|k| PressedInput::Button(ButtonBinding::Key(k)));
        let mouse = MouseButton::ALL
            .into_iter()
            .filter(|&b| self.mouse.pressed[b as usize])
            .map(|b| PressedInput::Button(ButtonBinding::Mouse(b)));
        let pad_buttons = self.pads.iter().enumerate().flat_map(|(slot, state)| {
            GamepadButton::ALL
                .into_iter()
                .filter(|&b| state.buttons.pressed[b as usize])
                .map(move |button| {
                    PressedInput::Button(ButtonBinding::Gamepad {
                        pad: Gamepad(slot as u8),
                        button,
                    })
                })
        });
        let pad_axes = self.pads.iter().enumerate().flat_map(move |(slot, state)| {
            GamepadAxis::ALL
                .into_iter()
                .filter(move |&a| {
                    state.axes_peak[a as usize].abs() >= axis_threshold
                        && state.axes_before[a as usize].abs() < axis_threshold
                })
                .map(move |axis| {
                    PressedInput::Axis(AxisBinding::Gamepad {
                        pad: Gamepad(slot as u8),
                        axis,
                        deadzone: DEFAULT_DEADZONE,
                    })
                })
        });
        keys.chain(mouse).chain(pad_buttons).chain(pad_axes)
    }
}

impl Input {
    /// Current binding configuration.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let value = ctx.input.actions();
    /// # }
    /// ```
    pub fn actions(&self) -> &crate::actions::ActionMap {
        &self.actions
    }

    /// Resolves an action name once into a `Copy` [`ActionId`](crate::actions::ActionId)
    /// for the queries below, which then skip the string lookup. An unknown
    /// name logs an error (naming the known actions) and gives
    /// [`ActionId::NONE`](crate::actions::ActionId::NONE), which reads false/zero.
    /// Resolve in setup, after installing the bindings with
    /// [`set_actions`](Self::set_actions); later rebinding keeps the id valid.
    ///
    /// The queries take an id or a `&str` name. They are meant to move to the
    /// `ctx.input` service of the API-friction work (#195).
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let jump = ctx.input.action_id("jump");
    /// if ctx.input.action_pressed(jump) { /* ... */ }
    /// # }
    /// ```
    pub fn action_id(&self, name: &str) -> crate::actions::ActionId {
        match self.actions.resolve(name) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("ponko: {e}");
                crate::actions::ActionId::NONE
            }
        }
    }

    /// Like [`action_id`](Self::action_id), but returns the error instead of
    /// logging it.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let jump = ctx.input.try_action_id("jump");
    /// # }
    /// ```
    pub fn try_action_id(&self, name: &str) -> Result<crate::actions::ActionId, crate::actions::UnknownAction> {
        self.actions.resolve(name)
    }

    /// Resolves several actions at once; see also [`actions!`](crate::actions!),
    /// which keeps the names in one declaration.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let [jump, fire] = ctx.input.action_ids(&["jump", "fire"]);
    /// # }
    /// ```
    pub fn action_ids<const N: usize>(&self, names: &[&str; N]) -> [crate::actions::ActionId; N] {
        names.map(|n| self.action_id(n))
    }

    /// Whether a named button is held. Takes a name or an
    /// [`ActionId`](crate::actions::ActionId) from [`action_id`](Self::action_id).
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let value = ctx.input.action_down("move");
    /// # }
    /// ```
    pub fn action_down(&self, action: impl crate::actions::ActionRef) -> bool {
        self.actions.down(action)
    }

    /// Whether a named button became held this frame.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let value = ctx.input.action_pressed("move");
    /// # }
    /// ```
    pub fn action_pressed(&self, action: impl crate::actions::ActionRef) -> bool {
        self.actions.pressed(action)
    }

    /// Whether all sources of a named button were released this frame.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let value = ctx.input.action_released("move");
    /// # }
    /// ```
    pub fn action_released(&self, action: impl crate::actions::ActionRef) -> bool {
        self.actions.released(action)
    }

    /// A named scalar axis, or zero.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let value = ctx.input.action_axis("move");
    /// # }
    /// ```
    pub fn action_axis(&self, action: impl crate::actions::ActionRef) -> f32 {
        self.actions.axis(action)
    }

    /// A named vector axis, or zero.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &Ctx) {
    /// let value = ctx.input.action_axis2("move");
    /// # }
    /// ```
    pub fn action_axis2(&self, action: impl crate::actions::ActionRef) -> Vec2 {
        self.actions.axis2(action)
    }

    /// Replace bindings at runtime. Held sources establish a baseline;
    /// rebinding itself generates no pressed/released edges.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &mut Ctx) {
    /// ctx.input.set_actions(ActionMap::default());
    /// # }
    /// ```
    pub fn set_actions(&mut self, mut actions: crate::actions::ActionMap) {
        actions.prepare_after(&self.actions);
        self.actions = actions;
        self.refresh_actions();
        self.actions.end_frame();
    }

    /// Adds or replaces one installed action, leaving the others alone: the
    /// name keeps its slot (ids resolved earlier stay valid), the other
    /// actions keep their state and edges, and, as with
    /// [`set_actions`](Self::set_actions), sources already held are the
    /// baseline, so the change fires no pressed/released edge. Read the
    /// current bindings with [`actions`](Self::actions).
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &mut Ctx) {
    /// let mut jump = ctx.input.actions().get("jump").cloned().unwrap_or(Action::Button(vec![]));
    /// if let Action::Button(sources) = &mut jump {
    ///     sources.push(ButtonBinding::Key(Key::K));
    /// }
    /// ctx.input.set_action("jump", jump);
    /// # }
    /// ```
    pub fn set_action(&mut self, name: impl Into<String>, action: crate::actions::Action) {
        let name = name.into();
        self.actions.set(name.clone(), action);
        self.refresh_actions();
        self.actions.clear_edges(&name);
    }

    /// Feed input by hand for tests/replay; live input uses the same state.
    /// Edges clear after each headless or windowed frame.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn example(ctx: &mut Ctx) {
    /// ctx.input.feed(ponko::actions::InputEvent::Key(Key::Space, true));
    /// # }
    /// ```
    pub fn feed(&mut self, event: crate::actions::InputEvent) {
        use crate::actions::InputEvent;
        match event {
            InputEvent::Key(k, down) => self.set_key(k, down),
            InputEvent::Mouse(b, down) => self.set_mouse_button(b, down),
            InputEvent::GamepadButton(p, b, down) => self.set_gamepad_button(p, b, down),
            InputEvent::GamepadAxis(p, a, value) => self.set_gamepad_axis(p, a, value),
            InputEvent::FocusLost => self.release_all(),
            InputEvent::GamepadDisconnected(p) => self.disconnect_gamepad(p),
        }
    }

    /// A stick of `pad` as a vector, each part from `-1.0` to `1.0`, y down:
    /// add it to a position to move that way.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx, pad: ponko::input::Gamepad, aim: &mut Vec2) {
    /// let stick = ctx.input.gamepad_stick(pad, ponko::input::GamepadStick::Right);
    /// if stick.length() > 0.5 { *aim = stick.normalize(); }
    /// # }
    /// ```
    pub fn gamepad_stick(&self, pad: Gamepad, stick: GamepadStick) -> Vec2 {
        let (x, y) = match stick {
            GamepadStick::Left => (GamepadAxis::LeftStickX, GamepadAxis::LeftStickY),
            GamepadStick::Right => (GamepadAxis::RightStickX, GamepadAxis::RightStickY),
        };
        Vec2::new(self.gamepad_axis(pad, x), self.gamepad_axis(pad, y))
    }

    /// Hides the cursor and holds it in the window (`true`), for mouse-look,
    /// or shows and frees it again (`false`). Applied after the frame; read
    /// the movement with [`mouse_motion`](Self::mouse_motion). The system
    /// frees the cursor while the window isn't focused, and it's held again
    /// when focus comes back. Headless runs only remember the setting.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(1, |ctx| {
    ///     ctx.input.set_cursor_locked(true);
    ///     assert!(ctx.input.cursor_locked());
    /// });
    /// ```
    pub fn set_cursor_locked(&mut self, locked: bool) {
        if self.cursor_locked != locked {
            self.cursor_locked = locked;
            self.cursor_lock_changed = true;
        }
    }

    /// Whether the cursor is locked, as set with
    /// [`set_cursor_locked`](Self::set_cursor_locked).
    pub fn cursor_locked(&self) -> bool {
        self.cursor_locked
    }

    /// Feed committed Unicode text from a custom host or headless test. Native
    /// hosts should forward character events, rather than mapping physical keys.
    /// ```
    /// use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// ctx.input.insert_text("Hello");
    /// # }
    /// ```
    pub fn insert_text(&mut self, text: &str) {
        self.add_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_hold_release_cycle() {
        let mut input = Input::new();

        input.set_key(Key::Space, true);
        assert!(input.key_down(Key::Space));
        assert!(input.key_pressed(Key::Space));
        assert!(!input.key_released(Key::Space));
        input.end_frame();

        // Held: still down, no longer "pressed".
        assert!(input.key_down(Key::Space));
        assert!(!input.key_pressed(Key::Space));
        input.end_frame();

        input.set_key(Key::Space, false);
        assert!(!input.key_down(Key::Space));
        assert!(input.key_released(Key::Space));
        input.end_frame();
        assert!(!input.key_released(Key::Space));
    }

    #[test]
    fn tap_within_one_frame_is_not_lost() {
        let mut input = Input::new();
        input.set_key(Key::A, true);
        input.set_key(Key::A, false);
        assert!(input.key_pressed(Key::A));
        assert!(input.key_released(Key::A));
        assert!(!input.key_down(Key::A));
    }

    #[test]
    fn repeated_down_events_do_not_retrigger_pressed() {
        let mut input = Input::new();
        input.set_key(Key::W, true);
        input.end_frame();
        input.set_key(Key::W, true);
        assert!(!input.key_pressed(Key::W));
    }

    #[test]
    fn release_all_clears_held_keys_and_buttons() {
        let mut input = Input::new();
        input.set_key(Key::Left, true);
        input.set_mouse_button(MouseButton::Right, true);
        input.end_frame();

        input.release_all();
        assert!(!input.key_down(Key::Left));
        assert!(input.key_released(Key::Left));
        assert!(!input.mouse_down(MouseButton::Right));
        assert!(input.mouse_released(MouseButton::Right));
        // Keys that were never down are not reported as released.
        assert!(!input.key_released(Key::Right));
    }

    #[test]
    fn mouse_delta_and_wheel_reset_each_frame() {
        let mut input = Input::new();
        input.set_mouse_pos(Vec2::new(10.0, 20.0));
        input.end_frame();

        input.set_mouse_pos(Vec2::new(15.0, 18.0));
        input.add_mouse_wheel(Vec2::new(0.0, 1.0));
        input.add_mouse_wheel(Vec2::new(0.0, 2.0));
        assert_eq!(input.mouse_delta(), Vec2::new(5.0, -2.0));
        assert_eq!(input.mouse_wheel(), Vec2::new(0.0, 3.0));

        input.end_frame();
        assert_eq!(input.mouse_delta(), Vec2::ZERO);
        assert_eq!(input.mouse_wheel(), Vec2::ZERO);
        assert_eq!(input.mouse_pos(), Vec2::new(15.0, 18.0));
    }

    #[test]
    fn raw_mouse_motion_adds_up_and_resets_each_frame() {
        let mut input = Input::new();
        input.add_mouse_motion(Vec2::new(3.0, -1.0));
        input.add_mouse_motion(Vec2::new(2.0, 4.0));
        // The cursor didn't move (it's locked), but the mouse did.
        assert_eq!(input.mouse_delta(), Vec2::ZERO);
        assert_eq!(input.mouse_motion(), Vec2::new(5.0, 3.0));
        input.end_frame();
        assert_eq!(input.mouse_motion(), Vec2::ZERO);
    }

    #[test]
    fn gamepads_take_the_lowest_free_slot() {
        let mut input = Input::new();
        let a = input.connect_gamepad().unwrap();
        let b = input.connect_gamepad().unwrap();
        assert_eq!((a.slot(), b.slot()), (0, 1));
        input.disconnect_gamepad(a);
        assert_eq!(input.gamepads(), [b]);
        let c = input.connect_gamepad().unwrap();
        assert_eq!(c.slot(), 0, "a new pad fills the gap");
        assert_eq!(input.gamepads(), [c, b], "sorted by slot");
        for _ in 2..MAX_GAMEPADS {
            input.connect_gamepad().unwrap();
        }
        assert_eq!(input.connect_gamepad(), None, "all slots taken");
    }

    #[test]
    fn gamepad_buttons_and_axes() {
        let mut input = Input::new();
        let pad = input.connect_gamepad().unwrap();
        input.set_gamepad_button(pad, GamepadButton::South, true);
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, -0.5);
        assert!(input.gamepad_pressed(pad, GamepadButton::South));
        assert!(input.gamepad_down(pad, GamepadButton::South));
        assert_eq!(input.gamepad_axis(pad, GamepadAxis::LeftStickX), -0.5);
        input.end_frame();
        assert!(!input.gamepad_pressed(pad, GamepadButton::South));
        assert!(input.gamepad_down(pad, GamepadButton::South));
        assert_eq!(input.gamepad_axis(pad, GamepadAxis::LeftStickX), -0.5, "axes hold");
    }

    #[test]
    fn unplugging_releases_buttons_and_centers_sticks() {
        let mut input = Input::new();
        let pad = input.connect_gamepad().unwrap();
        input.set_gamepad_button(pad, GamepadButton::Start, true);
        input.set_gamepad_axis(pad, GamepadAxis::RightTrigger, 1.0);
        input.end_frame();
        input.disconnect_gamepad(pad);
        assert!(input.gamepad_released(pad, GamepadButton::Start));
        assert!(!input.gamepad_down(pad, GamepadButton::Start));
        assert_eq!(input.gamepad_axis(pad, GamepadAxis::RightTrigger), 0.0);
        assert!(input.gamepads().is_empty());
    }

    use crate::actions::{AxisBinding, ButtonBinding, DEFAULT_DEADZONE, PressedInput};

    fn pressed(input: &Input) -> Vec<PressedInput> {
        input.pressed_this_frame().collect()
    }

    #[test]
    fn every_list_of_inputs_is_in_discriminant_order() {
        for (i, k) in Key::ALL.into_iter().enumerate() {
            assert_eq!(k as usize, i);
        }
        for (i, b) in MouseButton::ALL.into_iter().enumerate() {
            assert_eq!(b as usize, i);
        }
        for (i, b) in GamepadButton::ALL.into_iter().enumerate() {
            assert_eq!(b as usize, i);
        }
        for (i, a) in GamepadAxis::ALL.into_iter().enumerate() {
            assert_eq!(a as usize, i);
        }
    }

    #[test]
    fn pressed_this_frame_lists_new_presses_in_a_stable_order_and_clears() {
        let mut input = Input::new();
        let pad = input.connect_gamepad().unwrap();
        // Fed in the opposite order of the report.
        input.set_gamepad_button(pad, GamepadButton::South, true);
        input.set_mouse_button(MouseButton::Right, true);
        input.set_key(Key::Z, true);
        input.set_key(Key::A, true);
        assert_eq!(
            pressed(&input),
            [
                PressedInput::Button(ButtonBinding::Key(Key::A)),
                PressedInput::Button(ButtonBinding::Key(Key::Z)),
                PressedInput::Button(ButtonBinding::Mouse(MouseButton::Right)),
                PressedInput::Button(ButtonBinding::Gamepad {
                    pad,
                    button: GamepadButton::South
                }),
            ]
        );
        input.end_frame();
        assert!(pressed(&input).is_empty(), "held inputs are not new");
    }

    #[test]
    fn a_tap_inside_one_frame_is_still_reported() {
        let mut input = Input::new();
        input.set_key(Key::Q, true);
        input.set_key(Key::Q, false);
        assert_eq!(pressed(&input), [PressedInput::Button(ButtonBinding::Key(Key::Q))]);
    }

    #[test]
    fn a_release_is_not_a_press() {
        let mut input = Input::new();
        input.set_key(Key::Q, true);
        input.end_frame();
        input.set_key(Key::Q, false);
        assert!(pressed(&input).is_empty());
    }

    #[test]
    fn axes_report_when_they_cross_the_threshold_not_while_held() {
        let mut input = Input::new();
        let pad = input.connect_gamepad().unwrap();
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.4);
        assert!(pressed(&input).is_empty(), "under the threshold");
        input.end_frame();
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, -0.9);
        assert_eq!(
            pressed(&input),
            [PressedInput::Axis(AxisBinding::Gamepad {
                pad,
                axis: GamepadAxis::LeftStickX,
                deadzone: DEFAULT_DEADZONE
            })]
        );
        input.end_frame();
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, -1.0);
        assert!(pressed(&input).is_empty(), "still held past the threshold");
        input.end_frame();
        // Back to center, then out again: a new press.
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.0);
        input.end_frame();
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.6);
        assert_eq!(pressed(&input).len(), 1);
    }

    #[test]
    fn a_flick_out_and_back_in_one_frame_is_reported() {
        let mut input = Input::new();
        let pad = input.connect_gamepad().unwrap();
        input.set_gamepad_axis(pad, GamepadAxis::RightTrigger, 1.0);
        input.set_gamepad_axis(pad, GamepadAxis::RightTrigger, 0.0);
        assert_eq!(pressed(&input).len(), 1);
    }

    #[test]
    fn the_axis_threshold_can_be_chosen_and_unplugging_reports_nothing() {
        let mut input = Input::new();
        let pad = input.connect_gamepad().unwrap();
        input.set_gamepad_axis(pad, GamepadAxis::RightTrigger, 0.3);
        assert_eq!(input.pressed_this_frame().count(), 0);
        assert_eq!(input.pressed_this_frame_with_threshold(0.25).count(), 1);
        assert_eq!(input.pressed_this_frame_with_threshold(0.5).count(), 0);
        input.set_gamepad_axis(pad, GamepadAxis::RightTrigger, 1.0);
        input.disconnect_gamepad(pad);
        assert_eq!(
            input.pressed_this_frame().count(),
            0,
            "an unplugged stick is not a press"
        );
    }

    #[test]
    fn nan_axis_values_are_never_reported() {
        let mut input = Input::new();
        let pad = input.connect_gamepad().unwrap();
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickY, f32::NAN);
        assert_eq!(input.pressed_this_frame().count(), 0);
    }

    #[test]
    #[cfg(feature = "window")]
    fn winit_mapping() {
        assert_eq!(Key::from_winit(KeyCode::KeyW), Some(Key::W));
        assert_eq!(Key::from_winit(KeyCode::ArrowLeft), Some(Key::Left));
        assert_eq!(Key::from_winit(KeyCode::Digit5), Some(Key::Num5));
        assert_eq!(Key::from_winit(KeyCode::MediaPlayPause), None);
        assert_eq!(
            MouseButton::from_winit(winit::event::MouseButton::Middle),
            Some(MouseButton::Middle)
        );
    }
}
