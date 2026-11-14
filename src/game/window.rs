//! The windowed game loop (winit + wgpu), behind the `window` feature.

use std::sync::Arc;
use web_time::Instant;

use glam::Vec2;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Window, WindowId};

use super::*;
use crate::input::{Key, MouseButton};
use crate::render::{FrameStatus, Renderer};

/// `Game::run`: opens the window and runs until it closes.
pub(super) fn run<S, U>(mut game: Game, setup: S)
where
    S: FnOnce(&mut Ctx) -> U,
    U: SetupResult,
{
    #[cfg(feature = "profiling")]
    let _puffin_server = start_profiler();

    #[cfg(target_arch = "wasm32")]
    crate::web::show_panics_in_console();
    connect_hotpatch();
    let event_loop = EventLoop::<Ready>::with_user_event()
        .build()
        .unwrap_or_else(|e| panic!("ponko: could not start the event loop: {e}"));

    let clock = Clock::new(game.fixed_hz);
    let mut ctx = game.build_ctx(false);
    let control = game.start_control(&mut ctx, false);
    ctx.perf.profiler = crate::profiler::Profiler::from_env();
    // A benchmark measures how fast the game can run, not the display.
    if ctx.perf.profiler.benchmarking() {
        game.vsync = false;
    }
    match std::env::var("PONKO_VSYNC").as_deref() {
        Ok("0") => game.vsync = false,
        Ok("1") => game.vsync = true,
        _ => {}
    }
    let mut app = App {
        #[cfg(target_arch = "wasm32")]
        proxy: Some(event_loop.create_proxy()),
        control,
        config: game,
        setup: Some(setup),
        started: false,
        window: None,
        ctx,
        clock,
        last_frame: Instant::now(),
        occluded: false,
        #[cfg(feature = "inspector")]
        inspector: None,
        #[cfg(feature = "accessibility")]
        access: None,
        #[cfg(feature = "accessibility")]
        access_gap_until: None,
        #[cfg(feature = "gamepad")]
        gamepads: None,
    };

    event_loop
        .run_app(&mut app)
        .unwrap_or_else(|e| panic!("ponko: the event loop stopped with an error: {e}"));
}

/// Starts the puffin server; the returned handle keeps it running.
#[cfg(feature = "profiling")]
fn start_profiler() -> Option<puffin_http::Server> {
    const ADDR: &str = "127.0.0.1:8585";
    puffin::set_scopes_on(true);
    match puffin_http::Server::new(ADDR) {
        Ok(server) => {
            eprintln!("ponko: profiler running; view it with `puffin_viewer --url {ADDR}`");
            Some(server)
        }
        Err(e) => {
            eprintln!("ponko: could not start the profiler on {ADDR}: {e}");
            None
        }
    }
}

/// The end of a benchmark report: the meshes that cost the most triangles.
#[cfg(feature = "3d")]
fn print_mesh_costs(ctx: &Ctx) {
    let costs = ctx.gfx.mesh_costs();
    if costs.is_empty() {
        return;
    }
    eprintln!("-- heaviest meshes (camera models / triangles, shadow models / triangles) --");
    for c in costs.iter().take(15) {
        eprintln!(
            "{:>7} {:>7.2}M  {:>7} {:>7.2}M  {}",
            c.models,
            c.triangles as f64 / 1e6,
            c.shadow_models,
            c.shadow_triangles as f64 / 1e6,
            c.name.as_deref().unwrap_or("(made in code)")
        );
    }
    // Every mesh, for sorting and grouping elsewhere.
    if let Some(path) = std::env::var_os("PONKO_BENCH_MESHES") {
        let mut csv = String::from("models,triangles,shadow_models,shadow_triangles,name\n");
        for c in &costs {
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                c.models,
                c.triangles,
                c.shadow_models,
                c.shadow_triangles,
                c.name.as_deref().unwrap_or("")
            ));
        }
        if let Err(e) = std::fs::write(&path, csv) {
            eprintln!("ponko: could not write {}: {e}", path.to_string_lossy());
        }
    }
}

/// The GPU is ready (with the assets, on the web): setup can run.
struct Ready(Renderer);

struct App<S> {
    /// Delivers [`Ready`] from the task that opens the GPU (web builds).
    #[cfg(target_arch = "wasm32")]
    proxy: Option<winit::event_loop::EventLoopProxy<Ready>>,
    config: Game,
    setup: Option<S>,
    /// Setup has run; frames may run.
    started: bool,
    window: Option<Arc<Window>>,
    ctx: Ctx,
    clock: Clock,
    last_frame: Instant,
    /// Window hidden/minimized: stop drawing, poll slowly instead of spinning.
    occluded: bool,
    /// inspector overlay, when the game has it on.
    #[cfg(feature = "inspector")]
    inspector: Option<crate::inspector::Inspector>,
    /// Gives `ctx.ui` to the system's accessibility API (screen readers).
    #[cfg(feature = "accessibility")]
    access: Option<crate::ui::UiAccessAdapter>,
    /// While assistive technology is reading the window: when the next
    /// frame may start (see [`ACCESS_GAP`]).
    #[cfg(feature = "accessibility")]
    access_gap_until: Option<Instant>,
    /// Connected controllers, read at the start of every frame.
    #[cfg(feature = "gamepad")]
    gamepads: Option<crate::gamepad::Gamepads>,
    /// Kept for the whole run: dropping it closes the endpoint. The last
    /// field, so it is dropped after `ctx`: a reply `ctx` still holds (a
    /// screenshot waiting for its frame) is let go first, its client is told
    /// the game is shutting down, and the endpoint's exit doesn't wait for
    /// an answer that can no longer come.
    control: Option<Control>,
}

impl<S, U> App<S>
where
    S: FnOnce(&mut Ctx) -> U,
    U: SetupResult,
{
    fn logical_window_size(window: &Window) -> Vec2 {
        let size: LogicalSize<f32> = window.inner_size().to_logical(window.scale_factor());
        Vec2::new(size.width, size.height)
    }

    /// Vsync off on a desktop: frames run back to back from `about_to_wait`,
    /// because some systems (macOS) deliver redraw requests only at the
    /// display's refresh rate, which would cap the game there anyway.
    fn unpaced(&self) -> bool {
        !self.config.vsync && cfg!(not(target_arch = "wasm32"))
    }

    fn run_frame(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started {
            return;
        }

        if let Some(control) = &self.control {
            control.poll(&mut self.ctx);
        }
        let now = Instant::now();
        let frame_dt = now.duration_since(self.last_frame).as_secs_f64();
        self.last_frame = now;
        let since_start = |t: Instant| t.duration_since(now).as_secs_f32() * 1000.0;

        #[cfg(feature = "profiling")]
        puffin::GlobalProfiler::lock().new_frame();
        let timing = frame_timing(&mut self.clock, &mut self.ctx, frame_dt);
        self.ctx.begin_frame(timing);
        // In a browser, what the hidden IME text box saw since the last frame.
        #[cfg(target_arch = "wasm32")]
        crate::web::ime::drain(&mut self.ctx.input);
        #[cfg(feature = "gamepad")]
        if let Some(pads) = &mut self.gamepads {
            pads.poll(&mut self.ctx.input);
        }
        self.ctx.frame_prepare_services();
        #[cfg(feature = "3d")]
        self.ctx.gfx.apply_shader_checks();
        let systems_start = Instant::now();
        self.config.run_systems(&mut self.ctx, timing.fixed_steps);
        let systems_ms = systems_start.elapsed().as_secs_f32() * 1000.0;
        // What the systems left in the UI goes to the screen reader, and its
        // requests are queued for the UI's next frame.
        #[cfg(feature = "accessibility")]
        if let (Some(access), Some(window)) = (&mut self.access, &self.window) {
            access.update(&mut self.ctx.ui, window.scale_factor() as f32);
        }
        self.ctx.input.end_frame();
        #[cfg(feature = "inspector")]
        if let (Some(inspector), Some(window)) = (&mut self.inspector, &self.window) {
            inspector.frame(window, &mut self.ctx);
        }

        if self.ctx.quit_requested() {
            event_loop.exit();
            return;
        }
        if self.ctx.input.cursor_lock_changed
            && let Some(window) = &self.window
        {
            self.ctx.input.cursor_lock_changed = false;
            apply_cursor_lock(window, self.ctx.input.cursor_locked());
        }
        if let Some(window) = &self.window {
            self.ctx.input.apply_ime(window);
        }

        // With vsync, presenting blocks until the display is ready, which
        // paces the loop. So we can ask for the next frame right away.
        self.ctx.draw_perf_hud();
        let render_start = Instant::now();
        let status = self.ctx.render_frame();
        let render_ms = render_start.elapsed().as_secs_f32() * 1000.0;
        match status {
            FrameStatus::Presented | FrameStatus::Skipped | FrameStatus::Offscreen => self.occluded = false,
            FrameStatus::Occluded => self.occluded = true,
        }
        let sample = crate::profiler::FrameSample {
            frame_ms: frame_dt as f32 * 1000.0,
            systems_start_ms: since_start(systems_start),
            systems_ms,
            render_start_ms: since_start(render_start),
            render_ms,
            wait_ms: self.ctx.perf.display_wait_ms(),
            gpu_ms: self.ctx.perf.last_render_stats.gpu_ms,
            presented: matches!(status, FrameStatus::Presented | FrameStatus::Offscreen),
        };
        if self.ctx.record_frame(sample) {
            #[cfg(feature = "3d")]
            print_mesh_costs(&self.ctx);
            event_loop.exit();
            return;
        }
        if !self.occluded
            && !self.unpaced()
            && let Some(window) = &self.window
        {
            // A screen reader is asking: leave the event loop idle for a
            // moment before the next frame (`about_to_wait` asks for it).
            #[cfg(feature = "accessibility")]
            if let Some(until) = access_gap(self.access.as_ref().is_some_and(|a| a.is_active()), Instant::now()) {
                self.access_gap_until = Some(until);
                return;
            }
            window.request_redraw();
        }
    }
}

/// How long the event loop is left idle between two vsync-paced frames
/// while assistive technology is reading the window.
///
/// A screen reader's questions arrive through the event loop, one per pass.
/// A paced frame spends nearly all of its time waiting for the display
/// inside the frame, so without a pause the game answers one question per
/// frame: a screen reader that asks hundreds waits seconds and reports the
/// game as not responding. During the pause every waiting question is
/// answered. The frame still ends on the display's next refresh, as long as
/// its own work fits in what is left of the refresh interval.
#[cfg(feature = "accessibility")]
const ACCESS_GAP: std::time::Duration = std::time::Duration::from_millis(4);

/// When the next paced frame may start: `None` for right away, or the end
/// of the pause that lets assistive technology be answered.
#[cfg(feature = "accessibility")]
fn access_gap(read_by_assistive_technology: bool, now: Instant) -> Option<Instant> {
    read_by_assistive_technology.then(|| now + ACCESS_GAP)
}

#[cfg(all(test, feature = "accessibility"))]
mod access_gap_tests {
    use super::*;

    #[test]
    fn a_window_nobody_reads_starts_its_next_frame_right_away() {
        assert_eq!(access_gap(false, Instant::now()), None);
    }

    #[test]
    fn a_window_a_screen_reader_reads_leaves_the_event_loop_idle_before_the_next_frame() {
        let now = Instant::now();
        let until = access_gap(true, now).expect("a pause");
        assert_eq!(until - now, ACCESS_GAP);
        // Short enough to keep a 60 Hz frame on its refresh, long enough to
        // answer many questions (each takes well under a millisecond).
        assert!(ACCESS_GAP >= std::time::Duration::from_millis(2));
        assert!(ACCESS_GAP <= std::time::Duration::from_millis(8));
    }
}

impl<S, U> App<S>
where
    S: FnOnce(&mut Ctx) -> U,
    U: SetupResult,
{
    /// The GPU is open: run setup and start the frames.
    fn start(&mut self, renderer: Renderer) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let mut renderer = renderer;
        // Benchmarks draw offscreen: a window is shown at most once per display
        // refresh, so above that rate presenting stalls now and then.
        // PONKO_BENCH_PRESENT=1 draws to the window when it can take a frame.
        let present = std::env::var("PONKO_BENCH_PRESENT").is_ok_and(|v| v == "1");
        let benchmarking = self.ctx.perf.profiler.benchmarking();
        renderer.set_draw_hidden(benchmarking, benchmarking && !present);
        self.ctx.gfx.renderer = Some(renderer);
        self.ctx.set_window_size(Self::logical_window_size(&window));
        self.ctx.set_scale_factor(window.scale_factor() as f32);
        #[cfg(feature = "inspector")]
        if let (true, open) = crate::inspector::enabled(self.config.inspector) {
            let max_side = self.ctx.gfx.renderer.as_ref().map_or(2048, Renderer::max_texture_size);
            let mut inspector = crate::inspector::Inspector::new(&window, max_side as usize);
            inspector.open = open;
            self.inspector = Some(inspector);
        }

        #[cfg(feature = "gamepad")]
        {
            self.gamepads = Some(crate::gamepad::Gamepads::new(&mut self.ctx.input));
        }

        if let Some(setup) = self.setup.take() {
            self.ctx.set_in_setup(true);
            run_setups(std::mem::take(&mut self.config.setups), &mut self.ctx);
            let frame_fn = setup(&mut self.ctx).into_frame_fn();
            self.ctx.set_in_setup(false);
            self.ctx.apply_commands();
            // A sample count set in setup, before the first frame is drawn.
            self.ctx.follow_msaa();
            if let Some(f) = frame_fn {
                self.config.add_setup_closure(f);
            }
            self.started = true;
        }

        // Don't count setup time (e.g. asset loading) as the first frame's dt.
        self.last_frame = Instant::now();
        window.request_redraw();
    }
}

impl<S, U> ApplicationHandler<Ready> for App<S>
where
    S: FnOnce(&mut Ctx) -> U,
    U: SetupResult,
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            if let Some(renderer) = &mut self.ctx.gfx.renderer {
                renderer.surface_event(crate::render::surface_lifecycle::SurfaceEvent::Suspended(false));
                let size = window.inner_size();
                renderer.resize(size.width, size.height);
            }
            window.request_redraw();
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(self.config.width, self.config.height));
        #[cfg(target_arch = "wasm32")]
        let attributes = crate::web::canvas_attributes(attributes);
        #[cfg(not(target_arch = "wasm32"))]
        let attributes = if self.config.fullscreen {
            attributes.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)))
        } else {
            attributes
        };
        // The accessibility adapter has to be on the view before it is
        // first shown, so the window starts hidden and is shown right after.
        #[cfg(all(feature = "accessibility", target_os = "macos"))]
        let attributes = attributes.with_visible(false);
        let window = event_loop
            .create_window(attributes)
            .unwrap_or_else(|e| panic!("ponko: could not create the window: {e}"));
        #[cfg(feature = "accessibility")]
        {
            self.access = Some(crate::ui::UiAccessAdapter::for_window(&window, &self.config.title));
            #[cfg(target_os = "macos")]
            window.set_visible(true);
        }
        let window = Arc::new(window);
        self.window = Some(window.clone());
        let display = event_loop.owned_display_handle();
        let vsync = self.config.vsync;
        // The 3D frame's samples per pixel so far (setup hasn't run yet: the
        // default). A count set by a setup callback applies right after it
        // (`Ctx::follow_msaa`), before the first frame.
        #[cfg(feature = "3d")]
        let msaa = self.ctx.scene3d.post.msaa;
        #[cfg(not(feature = "3d"))]
        let msaa = 4;

        // Desktop: open the GPU and start right away.
        #[cfg(not(target_arch = "wasm32"))]
        self.start(Renderer::new(window, display, vsync, msaa));

        // Web: the browser can't block, so fetch the assets, read the
        // player's stored files (`ctx.storage`) and open the GPU
        // in a task, which sends `Ready` when done.
        #[cfg(target_arch = "wasm32")]
        if let Some(proxy) = self.proxy.take() {
            wasm_bindgen_futures::spawn_local(async move {
                crate::web::fetch_assets().await;
                crate::storage::preload_web().await;
                let renderer = Renderer::new_async(window, display, vsync, msaa).await;
                let _ = proxy.send_event(Ready(renderer));
            });
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(renderer) = &mut self.ctx.gfx.renderer {
            renderer.surface_event(crate::render::surface_lifecycle::SurfaceEvent::Suspended(true));
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, Ready(renderer): Ready) {
        self.start(renderer);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };

        // Browsers allow sound only from the first click, tap or key press
        // on (the audio output opens then); on the desktop it does nothing.
        if is_user_gesture(&event) {
            self.ctx.audio.backend.user_gesture();
        }

        #[cfg(feature = "inspector")]
        if let Some(inspector) = &mut self.inspector {
            if let WindowEvent::KeyboardInput { event: key, .. } = &event
                && key.state == ElementState::Pressed
                && !key.repeat
                && key.physical_key == PhysicalKey::Code(crate::inspector::TOGGLE_CODE)
            {
                inspector.open = !inspector.open;
                // Its text fields may have changed the window's IME.
                self.ctx.input.reapply_ime();
                window.request_redraw();
                return;
            }
            if inspector.on_event(&window, &event) {
                return;
            }
        }

        #[cfg(feature = "accessibility")]
        if let (WindowEvent::Focused(focused), Some(access)) = (&event, &mut self.access) {
            access.set_window_focused(*focused);
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::RedrawRequested => self.run_frame(event_loop),

            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let size = window.inner_size();
                if let Some(renderer) = self.ctx.gfx.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                if crate::render::surface_lifecycle::SurfaceLifecycle::nonzero_size(size.width, size.height).is_some() {
                    self.ctx.set_window_size(Self::logical_window_size(&window));
                }
                self.ctx.set_scale_factor(window.scale_factor() as f32);
                window.request_redraw();
            }

            WindowEvent::Occluded(occluded) => {
                if let Some(renderer) = &mut self.ctx.gfx.renderer {
                    renderer.surface_event(crate::render::surface_lifecycle::SurfaceEvent::Occluded(occluded));
                }
                // A benchmark draws offscreen while hidden and keeps going.
                self.occluded = occluded && !self.ctx.perf.profiler.benchmarking();
                if !occluded {
                    window.request_redraw();
                }
            }

            WindowEvent::Focused(false) => self.ctx.input.release_all(),

            // The system lets go of the cursor while the window is away.
            WindowEvent::Focused(true) if self.ctx.input.cursor_locked() => apply_cursor_lock(&window, true),

            WindowEvent::Ime(ime) => self.ctx.input.feed_ime(crate::input::ImeEvent::from_winit(ime)),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && let Some(text) = &event.text
                {
                    self.ctx.input.add_text(text);
                }
                if event.repeat {
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(key) = Key::from_winit(code)
                {
                    self.ctx.input.set_key(key, event.state == ElementState::Pressed);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = MouseButton::from_winit(button) {
                    self.ctx.input.set_mouse_button(button, state == ElementState::Pressed);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                let pos = position.to_logical::<f32>(window.scale_factor());
                self.ctx.input.set_mouse_pos(Vec2::new(pos.x, pos.y));
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(x, y),
                    MouseScrollDelta::PixelDelta(p) => {
                        let p: PhysicalPosition<f64> = p;
                        let logical = p.to_logical::<f32>(window.scale_factor());
                        Vec2::new(logical.x, logical.y) / PIXELS_PER_SCROLL_LINE
                    }
                };
                self.ctx.input.add_mouse_wheel(lines);
            }

            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (x, y) } = event
            && self.window.as_ref().is_some_and(|w| w.has_focus())
        {
            self.ctx.input.add_mouse_motion(Vec2::new(x as f32, y as f32));
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Between two frames while a screen reader is asking: stay idle
        // (answering it) until the pause is over, then ask for the frame.
        #[cfg(feature = "accessibility")]
        if let Some(until) = self.access_gap_until {
            if Instant::now() < until {
                event_loop.set_control_flow(ControlFlow::WaitUntil(until));
                return;
            }
            self.access_gap_until = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if self.occluded {
            // Hidden: check back now and then instead of spinning a core.
            // The simulation pauses while hidden (dt is clamped on return).
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + OCCLUDED_POLL));
        } else if self.unpaced() && self.started {
            event_loop.set_control_flow(ControlFlow::Poll);
            self.run_frame(event_loop);
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

/// Hides the cursor and holds it in place (or in the window, where the
/// system can't hold it still), or frees it.
fn apply_cursor_lock(window: &Window, locked: bool) {
    if locked {
        if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        }
    } else {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
    }
    window.set_cursor_visible(!locked);
}

/// Whether `event` lets a browser page start sound (a "user activation"):
/// a mouse button or key going down (not Escape) or a touch lifting.
fn is_user_gesture(event: &WindowEvent) -> bool {
    match event {
        WindowEvent::MouseInput { state, .. } => *state == ElementState::Pressed,
        WindowEvent::KeyboardInput { event, .. } => {
            event.state == ElementState::Pressed
                && event.physical_key != PhysicalKey::Code(winit::keyboard::KeyCode::Escape)
        }
        WindowEvent::Touch(touch) => touch.phase == winit::event::TouchPhase::Ended,
        _ => false,
    }
}
