//! [`Ctx`]: everything game code talks to each frame.

use glam::{Mat4, Vec2};
use ponko_ecs::{Commands, World};

use crate::assets;
use crate::audio::Audio;
use crate::camera::Rect;
use crate::camera2d::Camera2D;
use crate::color::Color;
use crate::ecs::{Sprite, Transform2D};
use crate::events::Events;
use crate::hot_reload::Asset;
use crate::input::{Input, Key};
use crate::render::batch::SpriteInstance;
use crate::render::batch::{FULL_UV, uv_region};
use crate::render::{FrameStatus, Renderer, Screenshot};
use crate::spatial::SpatialGrid;
use crate::text::Font;
use crate::texture::Texture;
use crate::time::{FrameTiming, Time};
use crate::ui::UiState;
use serde_json::Value;
use std::path::Path;

/// Maps window pixels ((0, 0) top-left, y down) to wgpu clip space
/// (x right, y up, both -1..1).
pub(crate) fn screen_projection(size: Vec2) -> Mat4 {
    let size = size.max(Vec2::ONE);
    Mat4::from_cols(
        glam::vec4(2.0 / size.x, 0.0, 0.0, 0.0),
        glam::vec4(0.0, -2.0 / size.y, 0.0, 0.0),
        glam::vec4(0.0, 0.0, 1.0, 0.0),
        glam::vec4(-1.0, 1.0, 0.0, 1.0),
    )
}

/// What every system gets: the world and the engine's services. Systems are
/// `fn(&mut Ctx)`.
///
/// `Ctx` is a set of public fields, one per area, so two areas can be used
/// at the same time: a sound plays while a `ctx.world` query is alive, UI
/// events are read while entities change.
///
/// | Field | What it is |
/// |---|---|
/// | [`world`](Self::world), [`commands`](Self::commands), [`events`](Self::events) | Entities, components and resources; deferred changes; messages between systems. |
/// | [`time`](Self::time) | [`Time`]: frame and simulation clocks, pause, speed. |
/// | [`input`](Self::input) | [`Input`]: keyboard, mouse, gamepads, named actions, typed text. |
/// | [`gfx`](Self::gfx) | [`Graphics`](crate::gfx::Graphics): drawing, textures, fonts, screenshots and, with `3d`, everything on the GPU. |
/// | [`audio`](Self::audio) | [`Audio`](crate::audio::Audio): sounds, music, buses, the 3D listener. |
/// | [`ui`](Self::ui) | [`Ui`](crate::ui::Ui): the retained widget tree and the immediate debug widgets. |
/// | [`assets`](Self::assets) | [`Assets`](crate::assets::Assets): asset sources, data files, saving scenes and prefabs, hot reload. |
/// | [`perf`](Self::perf) | [`Perf`](crate::perf::Perf): timings, the HUD, traces, profiling reports. |
/// | [`storage`](Self::storage) | [`Storage`](crate::storage::Storage): the player's saves and settings, crash-safe, on the desktop and the web. |
/// | [`camera`](Self::camera), `scene3d` | What the renderer reads when the frame is drawn: the 2D camera, and with `3d` the [`SceneSettings`](crate::render::SceneSettings) (camera, lighting, sky, post-processing, levels of detail, GPU passes, quality). |
/// | [`spatial`](Self::spatial), `physics`, `physics3d`, [`net`](Self::net) | Neighbor queries, the physics worlds, [`Networking`](crate::net::Networking): lockstep, messages, snapshots. |
///
/// # Shortcuts
///
/// These common calls are also on `Ctx`, each one a deliberate shortcut:
/// [`dt`](Self::dt), [`key_down`](Self::key_down),
/// [`key_pressed`](Self::key_pressed), [`key_released`](Self::key_released),
/// [`ctx.input.mouse_pos()`](crate::input::Input::mouse_pos), [`mouse_world`](Self::mouse_world),
/// [`window_size`](Self::window_size), [`quit`](Self::quit),
/// [`draw`](Self::draw), [`draw_text`](Self::draw_text),
/// [`load_texture`](Self::load_texture), [`load_sound`](Self::load_sound),
/// [`play_sound`](Self::play_sound) and [`random`](Self::random). Most just
/// call their service; [`mouse_world`](Self::mouse_world) (the mouse through
/// the camera) and [`random`](Self::random) (the seed and the running system)
/// need more than one area.
///
/// # Calls that span services
///
/// A method that needs several services at once can't live on one of them, so
/// it stays on `Ctx`:
/// - handles: [`free`](Self::free), [`is_live`](Self::is_live);
/// - scenes and prefabs (the world, textures and sounds together):
///   [`load_scene`](Self::load_scene),
///   [`load_scene_lockstep`](Self::load_scene_lockstep),
///   [`spawn_prefab`](Self::spawn_prefab);
/// - streaming and residency (the world, the GPU and the asset sources):
///   `stream_mesh`, `stream_texture`, `resident_mesh`, `resident_texture`,
///   their `try_` forms, `pin_resident_*`, `unpin_resident_*`,
///   `set_residency_budget` and `finish_uploads`;
/// - ragdolls and skeleton debugging (the world with the 3D physics or the
///   drawing): `start_ragdoll`, `stop_ragdoll`, `draw_skeleton`;
/// - reports that look across services: [`live_counts`](Self::live_counts),
///   `asset_inventory`, `ecs_diagnostics`, `capabilities_json` and
///   [`control_addr`](Self::control_addr);
/// - save slots (the world, the player's storage and the assets):
///   [`load_world`](Self::load_world).
///
/// Every other call is on its service: `ctx.input.mouse_down(..)`,
/// `ctx.gfx.create_mesh(..)`, `ctx.audio.play_at(..)`.
///
/// ```
/// use ponko::prelude::*;
/// fn spin(ctx: &mut Ctx) {
///     // `ctx.time` and `ctx.world` are separate fields: read the clock
///     // inside the query.
///     for t in ctx.world.query::<&mut ponko::d2::Transform>().iter() {
///         t.rotation += ctx.time.dt();
///     }
/// }
/// Game::new("Spin", 800, 600).sim_system(spin).run_headless(10, |_| {});
/// ```
pub struct Ctx {
    /// All entities, components and resources.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// #[derive(Component)]
    /// struct Vel(Vec2);
    ///
    /// fn movement(ctx: &mut Ctx) {
    ///     let dt = ctx.time.dt();
    ///     ctx.world.query::<(&mut ponko::d2::Transform, &Vel)>().for_each(|(t, v)| {
    ///         t.position += v.0 * dt;
    ///     });
    /// }
    /// ```
    pub world: World,
    /// Changes to make once the current system finishes, for spawning or
    /// despawning while a query is running. Applied automatically after
    /// every system.
    ///
    /// When they apply (systems run one at a time, each with all of `Ctx`):
    ///
    /// - Right after the recording system returns, before the next system
    ///   starts. Every system is a barrier, within a [`Phase`](crate::game::Phase)
    ///   and across phases; the recording system itself never sees them.
    /// - In recorded order. A [`Commands::add`](ponko_ecs::Commands::add)
    ///   closure sees the commands before it; an `insert` after a `despawn`
    ///   of the same entity does nothing.
    /// - Commands from `setup` apply when it returns, before the first tick.
    /// - Engine steps are systems too: commands from
    ///   [`Phase::BeforePhysics`](crate::game::Phase::BeforePhysics) are in the
    ///   world when the physics step runs, and frame systems see the commands
    ///   of every tick in their frame.
    /// - A system skipped by its interval ([`IntoSystem::every`](crate::game::IntoSystem::every))
    ///   doesn't run, so it records nothing.
    ///
    /// Changes made directly through [`world`](Self::world) are immediate.
    /// C and C++ systems follow the same rules. The guide's "Systems and the
    /// frame" chapter has the full contract with events.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// #[derive(Component)]
    /// struct Health(i32);
    ///
    /// fn deaths(ctx: &mut Ctx) {
    ///     ctx.world.query::<(Entity, &Health)>().for_each(|(e, h)| {
    ///         if h.0 <= 0 {
    ///             ctx.commands.despawn(e);
    ///         }
    ///     });
    /// }
    /// ```
    pub commands: Commands,
    /// Neighbor queries over every entity's [`d2::Transform`](crate::d2::Transform) position; filled
    /// at the start of each simulation tick once enabled with
    /// [`Game::spatial_grid`](crate::game::Game::spatial_grid) (empty otherwise).
    pub spatial: SpatialGrid,
    /// The physics world: ray casts, overlap queries, impulses, gravity.
    /// Simulated once enabled with [`Game::physics`](crate::game::Game::physics)
    /// (feature `physics`).
    #[cfg(feature = "physics")]
    pub physics: crate::d2::Physics,
    /// The 3D physics world: ray and shape casts, overlaps, impulses,
    /// collision meshes, gravity. Simulated once enabled with
    /// [`Game::physics3d`](crate::game::Game::physics3d) (feature `physics3d`).
    #[cfg(feature = "physics3d")]
    pub physics3d: crate::d3::Physics,
    /// Networking: the lockstep session (`ctx.net.lockstep`, which
    /// `ctx.net.set_input(..)` and the other per-tick calls reach directly),
    /// the message endpoint (`ctx.net.messages`), snapshot streams and
    /// interest. Offline and empty unless the game was given a session with
    /// [`Game::lockstep`](crate::game::Game::lockstep) or
    /// [`Game::messages`](crate::game::Game::messages).
    pub net: crate::net::Networking,
    /// Messages between systems: [`Events::send`] and [`Events::read`].
    /// A separate field, so you can read events while changing the world.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// struct Scored(u32);
    /// struct Score(u32);
    ///
    /// fn count_score(ctx: &mut Ctx) {
    ///     for s in ctx.events.read::<Scored>() {
    ///         ctx.world.resource_mut::<Score>().0 += s.0;
    ///     }
    /// }
    /// ```
    pub events: Events,
    /// Seed for `random()`.
    pub(crate) seed: u64,
    /// Index of the running system, so systems get different random streams.
    pub(crate) system_index: usize,
    /// No window or GPU (`Game::run_headless`): textures are size-only handles.
    pub(crate) headless: bool,
    /// Where the control endpoint listens, if it's on.
    pub(crate) control_addr: Option<std::net::SocketAddr>,
    /// `true` while the game's setup (and plugins' setups) run.
    in_setup: bool,
    /// Scene loads so far, so tools holding entity ids (the inspector's
    /// selection) can tell the world was replaced.
    pub(crate) scene_loads: u64,
    /// The scene file the world was last loaded from or saved to (by
    /// `load_scene`, the editor or the control endpoint), where the editor
    /// saves back to.
    pub(crate) scene_file: Option<String>,
    /// The editor's undo and redo history and Play snapshot, shared by the
    /// inspector and the control endpoint's `editor.*` methods so a person
    /// and an agent edit the same world the same way. Empty (no cost) until
    /// something edits.
    pub(crate) editor: crate::editor::Editor,
    /// Keyboard, mouse, gamepads and named actions: see [`Input`].
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// if ctx.input.mouse_pressed(MouseButton::Left) { /* ... */ }
    /// # }
    /// ```
    pub input: Input,
    /// The frame and simulation clocks: see [`Time`].
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let seconds = ctx.time.elapsed();
    /// ctx.time.set_speed(2.0);
    /// # }
    /// ```
    pub time: Time,
    window_size: Vec2,
    quit_requested: bool,
    /// Which part of the world the window shows. Move it, zoom it, or make
    /// it follow something; see [`d2::Camera`](crate::d2::Camera).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.camera.zoom = 2.0;
    /// ctx.camera.look_at(vec2(1000.0, 1000.0));
    /// # }
    /// ```
    pub camera: crate::d2::Camera,
    /// How the 3D world is drawn (feature `3d`): the camera, lighting, sky
    /// and fog, post-processing, levels of detail, the game's own GPU passes,
    /// graphics quality presets and automatic quality. See
    /// [`SceneSettings`](crate::render::SceneSettings).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.scene3d.camera.position = vec3(0.0, 5.0, 10.0);
    /// ctx.scene3d.camera.look_at(Vec3::ZERO);
    /// ctx.scene3d.lighting.sun_intensity = 1.0; // overcast
    /// ctx.scene3d.set_quality(ponko::render::GraphicsQuality::High);
    /// # }
    /// ```
    #[cfg(feature = "3d")]
    pub scene3d: crate::render::SceneSettings,
    /// Engine-side UI scratch: the spare tree, typed text, world panels.
    pub(crate) ui_state: UiState,
    /// The screen's UI: the retained widget tree and its screen stack
    /// (processed before the frame's systems), who has the pointer and the
    /// keyboard, and the immediate widgets (`ctx.ui.immediate`). See
    /// [`Ui`](crate::ui::Ui).
    ///
    /// ```
    /// use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let root = ctx.ui.add(ponko::ui::UiWidget::Panel, ponko::ui::UiLayout::auto());
    /// ctx.ui.push_screen(root, false);
    /// # }
    /// ```
    pub ui: crate::ui::Ui,
    /// Drawing and everything on the GPU: sprites, text, shapes, textures,
    /// fonts, and with the `3d` feature meshes, materials, shaders and the
    /// rest. See [`Graphics`](crate::gfx::Graphics).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, crab: Texture) {
    /// ctx.gfx.draw_circle(vec2(100.0, 100.0), 20.0, Color::WHITE);
    /// ctx.gfx.draw(crab, vec2(400.0, 300.0));
    /// # }
    /// ```
    pub gfx: crate::gfx::Graphics,
    /// Asset files: where they are read from, data files, saving scenes and
    /// prefabs, and what hot reload changed. See [`Assets`](crate::assets::Assets).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # #[derive(serde::Deserialize)]
    /// # struct Rules { speed: f32 }
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.assets.load_resource::<Rules>(&mut ctx.world, "rules.json");
    /// # }
    /// ```
    pub assets: crate::assets::Assets,
    /// Sounds, playing handles, buses and the listener: see
    /// [`Audio`](crate::audio::Audio).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let hit = ctx.audio.load("hit.ogg");
    /// ctx.audio.play(hit);
    /// # }
    /// ```
    pub audio: Audio,
    /// Where the time goes: system timings, frame and render statistics,
    /// GPU passes, the HUD, traces and the report. See [`Perf`](crate::perf::Perf).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.perf.set_hud(true);
    /// let draw_calls = ctx.perf.render_stats().draw_calls;
    /// # }
    /// ```
    pub perf: crate::perf::Perf,
    /// The player's own files (saves, settings) in the per-user data folder,
    /// or the browser's storage on the web, with crash-safe writes. Needs an
    /// app id ([`Game::app_id`](crate::game::Game::app_id)). See
    /// [`Storage`](crate::storage::Storage).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.storage.write("settings.json", br#"{"volume":0.5}"#).expect("saved");
    /// ctx.storage.save_world(&ctx.world, "slot1").expect("saved");
    /// # }
    /// ```
    pub storage: crate::storage::Storage,
}

impl Ctx {
    /// `fixed_dt` in seconds, as f64 so `sim_time()` stays exact over long runs.
    pub(crate) fn new(fixed_dt: f64, window_size: Vec2) -> Self {
        // Every service that loads files shares these two.
        let asset_sources = assets::SharedSources::default();
        let watcher = crate::hot_reload::SharedWatcher::default();
        let mut world = World::new();
        crate::reflect::WorldReflect::reflect::<crate::d2::Transform>(&mut world);
        crate::reflect::describe_transform2d(&mut world);
        crate::reflect::WorldReflect::reflect::<crate::attach::Attached>(&mut world);
        crate::reflect::WorldReflect::reflect::<crate::scene::PrefabParts>(&mut world);
        #[cfg(feature = "3d")]
        crate::reflect::WorldReflect::reflect::<crate::d3::Transform>(&mut world);
        #[cfg(feature = "3d")]
        crate::reflect::reflect_model(&mut world);
        #[cfg(feature = "3d")]
        {
            crate::reflect::WorldReflect::reflect::<crate::d3::PointLight>(&mut world);
            crate::reflect::WorldReflect::reflect::<crate::d3::SpotLight>(&mut world);
            crate::reflect::WorldReflect::reflect::<crate::d3::Decal>(&mut world);
            crate::reflect::WorldReflect::reflect::<crate::render::ReceivesDecals>(&mut world);
            crate::reflect::WorldReflect::reflect::<crate::render::LodHysteresis>(&mut world);
            crate::reflect::WorldReflect::reflect::<crate::anim::SkinOf>(&mut world);
            crate::reflect::WorldReflect::reflect::<crate::anim::AttachToJoint>(&mut world);
            crate::reflect::WorldReflect::reflect::<crate::attach::Attached3D>(&mut world);
        }
        crate::reflect::reflect_sprite(&mut world);
        crate::reflect::WorldReflect::reflect::<crate::light2d::PointLight2D>(&mut world);
        crate::reflect::WorldReflect::reflect::<crate::light2d::SpotLight2D>(&mut world);
        crate::reflect::WorldReflect::reflect::<crate::light2d::DirectionalLight2D>(&mut world);
        crate::reflect::WorldReflect::reflect::<crate::light2d::Occluder2D>(&mut world);
        crate::reflect::reflect_particle_emitter2d(&mut world);
        crate::reflect::reflect_animation(&mut world);
        // The file names behind sprites and models: the world saves by them,
        // the graphics service fills them in as files load.
        let gfx = crate::gfx::Graphics::new(
            asset_sources.clone(),
            watcher.clone(),
            world.resource::<crate::reflect::TextureNames>().clone(),
            #[cfg(feature = "3d")]
            world.resource::<crate::reflect::MeshNames>().clone(),
            #[cfg(feature = "3d")]
            world.resource::<crate::reflect::MaterialNames>().clone(),
        );
        #[allow(unused_mut, clippy::let_and_return)]
        let mut ctx = Self {
            audio: Audio {
                asset_sources: asset_sources.clone(),
                watcher: watcher.clone(),
                ..Default::default()
            },
            gfx,
            assets: crate::assets::Assets {
                asset_sources,
                watcher,
                ..Default::default()
            },
            world,
            commands: Commands::new(),
            spatial: SpatialGrid::default(),
            #[cfg(feature = "physics")]
            physics: crate::d2::Physics::default(),
            #[cfg(feature = "physics3d")]
            physics3d: crate::d3::Physics::default(),
            net: crate::net::Networking::default(),
            events: Events::default(),
            seed: crate::random::DEFAULT_SEED,
            system_index: 0,
            headless: false,
            control_addr: None,
            in_setup: false,
            scene_loads: 0,
            scene_file: None,
            editor: crate::editor::Editor::default(),
            input: Input::new(),
            time: Time::new(fixed_dt),
            window_size,
            quit_requested: false,
            camera: Camera2D::new(window_size),
            #[cfg(feature = "3d")]
            scene3d: crate::render::SceneSettings::new(window_size),
            ui_state: UiState::default(),
            ui: crate::ui::Ui::default(),
            perf: crate::perf::Perf::new(),
            storage: crate::storage::Storage::default(),
        };
        #[cfg(feature = "3d")]
        ctx.sync_audio_camera();
        ctx
    }

    /// Engine side: draw everything queued this frame, then reset the queue.
    pub(crate) fn render_frame(&mut self) -> FrameStatus {
        self.draw_retained_ui();
        let capture = !self.gfx.screenshots.is_empty();
        #[cfg(feature = "3d")]
        if self.scene3d.autotune.policy.is_some() || self.gfx.autotune.is_some() {
            let sample = self.gfx.renderer.as_ref().and_then(Renderer::quality_sample);
            self.update_quality_autotune(sample, capture);
        }
        self.follow_msaa();
        #[cfg(feature = "3d")]
        self.draw_ui_panels();
        self.follow_window_size();
        #[cfg(feature = "3d")]
        self.prepare_3d();
        #[cfg(feature = "3d")]
        let panels = self.ui_state.panels.frame_draws();
        #[cfg(not(feature = "3d"))]
        let panels: &mut [crate::render::batch::PanelDraw] = &mut [];
        if self.gfx.renderer.is_some() {
            let lights = &mut self.gfx.lights2d;
            lights
                .frame
                .collect(&mut self.world, self.camera.visible_rect(), lights.ambient);
        }
        let status = match self.gfx.renderer.as_mut() {
            Some(renderer) => renderer.render(
                &mut self.gfx.draw_list,
                &mut self.gfx.ui_list,
                panels,
                self.gfx.clear_color,
                self.camera.view_proj(),
                screen_projection(self.window_size),
                capture,
                &self.gfx.lights2d,
            ),
            None => FrameStatus::Skipped,
        };
        #[cfg(feature = "3d")]
        // A frame that wasn't drawn hands its probe captures to the next one.
        self.gfx
            .assets3d
            .probes
            .settle(!matches!(status, FrameStatus::Occluded | FrameStatus::Skipped));
        self.perf.last_render_stats = self.gfx.renderer.as_ref().map(Renderer::stats).unwrap_or_default();
        self.perf.last_render_detail = self.gfx.renderer.as_ref().map(Renderer::detail);
        if self.gfx.renderer.is_some() && status.drew() {
            self.perf.push_render_frame(self.time.frame());
        }
        if let Some(renderer) = &self.gfx.renderer {
            self.perf.gpu_passes.clear();
            self.perf.gpu_passes.extend_from_slice(renderer.gpu_passes());
            self.perf.display_wait_ms = renderer.display_wait_ms();
        }
        self.gfx.draw_list.clear();
        self.gfx.ui_list.clear();
        if let Some(shot) = self.gfx.renderer.as_mut().and_then(Renderer::take_screenshot) {
            for req in self.gfx.screenshots.drain(..) {
                let result = match req.claimed {
                    Some(claimed) => save_claimed_png(claimed, &shot, req.max_size),
                    None => save_png(&req.path, &shot, req.max_size),
                };
                match req.reply {
                    Some(reply) => {
                        let _ = reply.send(result);
                    }
                    None => {
                        if let Err(e) = result {
                            eprintln!("ponko: {e}");
                        }
                    }
                }
            }
        }
        status
    }

    // ------------------------------------------------- engine steps of a frame
    //
    // The windowed loop (`window.rs`, then `Game::run_systems`) runs these
    // before the game's systems, in this order; the headless loop runs only
    // `frame_prepare_services`, because a headless run has ticks and no frames
    // to draw or hear. The other engine steps of a frame (animators,
    // particles, positional audio) are systems in `crate::systems`, pinned at
    // the end of their phase. ARCHITECTURE.md lists who owns each service.

    /// Engine side, start of a frame, after `begin_frame` (and, windowed, the
    /// gamepad poll; headless runs have none) and before any system:
    /// reloads changed asset files and runs the streaming and residency
    /// step. Windowed and headless loops both run it.
    pub(crate) fn frame_prepare_services(&mut self) {
        self.follow_window_size();
        self.reload_changed_assets();
        self.pump_streaming();
    }

    /// The 3D frame takes the sample count in `scene3d.post.msaa`, when the
    /// GPU can (see `Renderer::follow_msaa`). Runs after each setup callback,
    /// so a count set there applies before a later one builds 3D state and
    /// before the first frame, and at every frame.
    pub(crate) fn follow_msaa(&mut self) {
        #[cfg(all(feature = "3d", feature = "window"))]
        if let Some(renderer) = self.gfx.renderer.as_mut() {
            renderer.follow_msaa(self.scene3d.post.msaa);
        }
    }

    /// The 3D camera's view takes the window's size again, also after the
    /// game replaced `scene3d` (its `Default` has a 1×1 view).
    fn follow_window_size(&mut self) {
        #[cfg(feature = "3d")]
        self.scene3d.camera.set_viewport(self.window_size);
    }

    /// Engine side, before the frame's first simulation tick or frame
    /// system: the retained UI processes this frame's input, so systems see
    /// its focus, events and screen changes. Frames only: headless runs
    /// never update the UI.
    #[inline]
    pub(crate) fn frame_begin_services(&mut self) {
        self.update_retained_ui();
    }

    /// Engine side: start a new frame with fresh timing.
    pub(crate) fn begin_frame(&mut self, timing: FrameTiming) {
        self.time.begin_frame(timing);
        self.events.begin_frame(self.time.frame(), self.time.sim_tick());
        self.gfx.frame = self.time.frame();
        self.gfx.asset_ledger.now = self.time.frame();
        self.audio.ledger.now = self.time.frame();
        self.gfx.text.begin_frame();
        self.ui.immediate.begin_frame();
        #[cfg(feature = "3d")]
        self.unlink_freed_gpu_buffers();
    }

    pub(crate) fn set_scale_factor(&mut self, scale: f32) {
        self.gfx.scale_factor = scale.max(0.25);
    }

    pub(crate) fn set_window_size(&mut self, size: Vec2) {
        self.window_size = size;
        self.camera.set_viewport(size);
        #[cfg(feature = "3d")]
        self.scene3d.camera.set_viewport(size);
    }

    pub(crate) fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// A snapshot of the ECS for tools and tests: every archetype with its
    /// components, entity count and storage bytes; every component type
    /// with its size and how many entities have it; and every system in run
    /// order with its [`Phase`](crate::game::Phase), interval, last timings and
    /// declared access. Its `schedule` explains the order: the stages the
    /// systems run in, why each stage starts where it does, why a system
    /// runs alone, and which declared systems must stay in order (see
    /// [`diagnostics`](crate::diagnostics)).
    ///
    /// Built only when called, so it costs nothing in a game that never
    /// asks (the game itself only counts how each batch of the parallel
    /// scheduler was run). It allocates and works out the schedule plan,
    /// so don't call it every frame. The control endpoint serves it as
    /// `ecs.diagnostics`.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// #[derive(Component)]
    /// struct Pos(f32);
    /// fn report(ctx: &mut Ctx) {
    ///     let d = ctx.ecs_diagnostics();
    ///     assert_eq!(d.entities, 2);
    ///     assert_eq!(d.systems[0].phase, "AfterPhysics");
    ///     println!("{}", serde_json::to_string_pretty(&d).unwrap());
    /// }
    /// Game::new("t", 1, 1).sim_system(report).run_headless(1, |ctx| {
    ///     ctx.world.spawn_batch([(Pos(0.0),), (Pos(1.0),)]);
    /// });
    /// ```
    pub fn ecs_diagnostics(&self) -> crate::diagnostics::EcsDiagnostics {
        crate::diagnostics::snapshot(self)
    }

    /// The address of this game's control endpoint, or `None` if it's off.
    /// Tools find it in the instance registry; this is for tests and logs.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut addr = None;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| addr = ctx.control_addr());
    /// assert_eq!(addr, None); // off unless enabled
    /// ```
    pub fn control_addr(&self) -> Option<std::net::SocketAddr> {
        self.control_addr
    }

    /// What this running game can do, as JSON text: the control endpoint's
    /// `capabilities` answer (engine version, headless or windowed, compiled
    /// features, subsystems with why one is off, which control methods it
    /// accepts with why not, and the reflected components and resources with
    /// their schemas). Reading it changes nothing. Tools ask the endpoint;
    /// this is for tests, logs and native code.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let report: serde_json::Value = serde_json::from_str(&ctx.capabilities_json()).unwrap();
    ///     assert_eq!(report["mode"], "headless");
    /// });
    /// ```
    pub fn capabilities_json(&self) -> String {
        crate::control::capabilities_json(self)
    }

    /// Engine side: speed to feed the clock this frame (0 while paused).
    pub(crate) fn effective_sim_speed(&self) -> f64 {
        self.time.effective_sim_speed()
    }

    /// Engine side: uses up one requested single step; `false` if none was pending.
    pub(crate) fn take_one_step(&mut self) -> bool {
        self.time.take_one_step()
    }

    pub(crate) fn take_pending_steps(&mut self) -> u32 {
        self.time.take_pending_steps()
    }

    pub(crate) fn advance_tick(&mut self) {
        self.time.advance_tick();
        self.events.set_tick(self.time.sim_tick());
        // Residency tells ticks apart by this (it is also set before each
        // system), so loads between ticks see the new one.
        self.gfx.sim_tick = self.time.sim_tick();
    }

    pub(crate) fn set_sim_steps(&mut self, steps: u32) {
        self.time.set_sim_steps(steps);
    }

    pub(crate) fn set_in_sim(&mut self, in_sim: bool) {
        self.time.set_in_sim(in_sim);
    }

    /// Random numbers for this system and tick: `ctx.random().of(entity)`
    /// for an entity's own stream (safe in `par_for_each`), or
    /// `.stream(n)` for anything else. A simulation run twice with the same
    /// [seed](crate::game::Game::seed) gives the same results.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// #[derive(Component)]
    /// struct Pos(f32);
    ///
    /// fn jitter(ctx: &mut Ctx) {
    ///     let random = ctx.random();
    ///     ctx.world.query::<(Entity, &mut Pos)>().par_for_each(|(e, p)| p.0 += random.of(e).range(-1.0, 1.0));
    /// }
    ///
    /// let run = || {
    ///     let mut w = Game::new("t", 1, 1).sim_system(jitter).run_headless(100, |ctx| {
    ///         ctx.world.spawn_batch((0..1000).map(|_| (Pos(0.0),)));
    ///     });
    ///     w.query::<&Pos>().iter().map(|p| p.0).sum::<f32>()
    /// };
    /// assert_eq!(run(), run()); // same seed, same result
    /// ```
    pub fn random(&self) -> crate::random::Random {
        crate::random::Random::new(self.seed, self.time.sim_tick(), self.system_index)
    }

    /// Whether a simulation system is running now.
    pub(crate) fn in_sim(&self) -> bool {
        self.time.in_sim()
    }

    pub(crate) fn set_in_setup(&mut self, in_setup: bool) {
        self.in_setup = in_setup;
    }

    /// Whether the game's setup is running now (before the first tick).
    pub(crate) fn in_setup(&self) -> bool {
        self.in_setup
    }

    /// Engine side: apply commands recorded by the system that just ran.
    pub(crate) fn apply_commands(&mut self) {
        if !self.commands.is_empty() {
            self.world.apply(&mut self.commands);
        }
        self.perf.entities = self.world.len();
    }

    /// Queues every `(Transform2D, Sprite)` entity for drawing.
    pub(crate) fn draw_world_sprites(&mut self) {
        use crate::sprite_material::{BlendMode, SpriteMaterial};
        use ponko_ecs::Without;
        let draw_list = &mut self.gfx.draw_list;
        // Plain sprites first, on the same path as before blend modes and
        // materials existed; then the (usually few) sprites with either.
        let mut query = self
            .world
            .query::<(&Transform2D, &Sprite)>()
            .filter::<(Without<BlendMode>, Without<SpriteMaterial>)>();
        let n = query.count();
        if n < SPRITES_PARALLEL_MIN {
            query.for_each(|(t, s)| draw_list.push(s.texture.id, s.z, sprite_instance(t, s)));
        } else {
            // SAFETY: `par_for_each_indexed` visits each of the query's `n`
            // entities exactly once, with indices `0..n`.
            unsafe {
                draw_list.extend(n, |slots| {
                    query.par_for_each_indexed(|i, (t, s)| slots.write(i, s.texture.id, s.z, sprite_instance(t, s)));
                });
            }
        }
        self.draw_styled_sprites();
    }

    /// The sprites with a [`BlendMode`](crate::render::BlendMode) or a
    /// [`SpriteMaterial`](crate::render::SpriteMaterial); most frames have
    /// none, so it stays out of line.
    #[inline(never)]
    fn draw_styled_sprites(&mut self) {
        use crate::sprite_material::{BlendMode, SpriteMaterial, style_key};
        use ponko_ecs::{Or, With};
        let draw_list = &mut self.gfx.draw_list;
        let mut styled = self
            .world
            .query::<(&Transform2D, &Sprite, Option<&BlendMode>, Option<&SpriteMaterial>)>()
            .filter::<Or<(With<BlendMode>, With<SpriteMaterial>)>>();
        let n = styled.count();
        if n == 0 {
            // Visiting an `Or` query checks every sprite even when none
            // match; counting checks whole archetypes.
            return;
        }
        let style =
            |b: Option<&BlendMode>, m: Option<&SpriteMaterial>| style_key(b.copied().unwrap_or_default(), m.copied());
        if n < SPRITES_PARALLEL_MIN {
            styled
                .for_each(|(t, s, b, m)| draw_list.push_styled(s.texture.id, style(b, m), s.z, sprite_instance(t, s)));
        } else {
            // SAFETY: as in `draw_world_sprites`.
            unsafe {
                draw_list.extend(n, |slots| {
                    styled.par_for_each_indexed(|i, (t, s, b, m)| {
                        slots.write_styled(i, s.texture.id, style(b, m), s.z, sprite_instance(t, s));
                    });
                });
            }
        }
    }

    /// One frame of the built-in sprite drawing without a GPU, for
    /// `ponko_bench`: collects every `Sprite` entity as `draw_sprites` does,
    /// orders and batches them as the renderer does, and forgets them.
    /// Returns the sprites and batches. Unstable (`bench-internals`).
    #[doc(hidden)]
    #[cfg(feature = "bench-internals")]
    pub fn bench_sprite_frame(&mut self) -> (usize, usize) {
        self.draw_world_sprites();
        let (sprites, batches) = self.gfx.draw_list.finish();
        let counts = (sprites.len(), batches.len());
        self.gfx.draw_list.clear();
        counts
    }

    // ---------------------------------------------------------------- drawing

    /// Loads the textures of sprites and meshes of models that `json` (a
    /// scene, a prefab, or components) names, so they can be spawned.
    pub(crate) fn load_scene_assets(&mut self, json: &serde_json::Value) -> Result<(), crate::error::Error> {
        let mut names = Vec::new();
        crate::reflect::asset_names(json, "Sprite", "texture", &mut names);
        crate::reflect::asset_names(json, "ParticleEmitter2D", "texture", &mut names);
        crate::reflect::asset_names(json, "Animation", "texture", &mut names);
        for name in names.drain(..) {
            self.gfx.try_load_texture(&name)?;
        }
        #[cfg(feature = "3d")]
        {
            crate::reflect::asset_names(json, "Model", "mesh", &mut names);
            crate::reflect::asset_names(json, "ShadowMesh", "mesh", &mut names);
            crate::reflect::lod_mesh_names(json, &mut names);
            names.sort_unstable();
            names.dedup();
            for name in names {
                self.gfx.try_load_mesh(&name)?;
            }
        }
        #[cfg(feature = "physics3d")]
        self.load_collider_files(json)?;
        self.load_scene_asset_fields(json)
    }

    /// Reloads textures, fonts, sounds and data files that changed (checked a few times a second).
    pub(crate) fn reload_changed_assets(&mut self) {
        self.assets.reloaded_textures.clear();
        self.assets.reloaded_files.clear();
        if !self.assets.watcher.on() {
            return;
        }
        let mut changed = std::mem::take(&mut self.assets.reload_pending);
        self.assets.watcher.changed(web_time::Instant::now(), &mut changed);
        for (asset, file, name) in changed.drain(..) {
            let texture = match asset {
                Asset::Texture(t) => t,
                Asset::Font(id) => {
                    let applied = assets::read(&file)
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| Ok(self.gfx.text.replace_font(Font(id), &bytes)?));
                    self.assets.report_reload(name, applied);
                    continue;
                }
                Asset::Sound(id) => {
                    let applied = assets::read(&file)
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| self.audio.backend.replace(id, bytes));
                    self.assets.report_reload(name, applied);
                    continue;
                }
                Asset::Data(i) => {
                    let applied = assets::read_to_string(&file)
                        .and_then(|text| (self.assets.data_loaders[i])(&mut self.world, &text).map_err(String::from));
                    self.assets.report_reload(name, applied);
                    continue;
                }
                Asset::SpriteShader(id) => {
                    let applied =
                        assets::read_to_string(&file).and_then(|text| self.gfx.reload_sprite_shader(id, &name, &text));
                    self.assets.report_reload(name, applied);
                    continue;
                }
                Asset::Level => {
                    let applied = self.reload_level(&name);
                    self.assets.report_reload(name, applied);
                    continue;
                }
                #[cfg(feature = "3d")]
                Asset::Shader(id) => {
                    let applied =
                        assets::read_to_string(&file).and_then(|text| self.gfx.reload_shader(id, &name, &text));
                    self.assets.report_reload(name, applied);
                    continue;
                }
                #[cfg(feature = "3d")]
                Asset::Effect(id) => {
                    let applied =
                        assets::read_to_string(&file).and_then(|text| self.gfx.reload_effect(id, &name, &text));
                    self.assets.report_reload(name, applied);
                    continue;
                }
                #[cfg(feature = "3d")]
                Asset::Compute(id) => {
                    let applied =
                        assets::read_to_string(&file).and_then(|text| self.gfx.reload_compute(id, &name, &text));
                    self.assets.report_reload(name, applied);
                    continue;
                }
            };
            #[cfg(feature = "compressed-textures")]
            match self.gfx.reload_compressed_texture(texture, &file, &name) {
                Some(Ok(())) => {
                    self.assets.reloaded_textures.push(texture);
                    self.assets.reloaded_files.push(name.clone());
                    eprintln!("ponko: reloaded {name}");
                    continue;
                }
                // Often a file caught halfway through saving; the next save reloads it.
                Some(Err(e)) => {
                    eprintln!("ponko: {e}");
                    continue;
                }
                None => {}
            }
            match assets::load_image_at(&file, &name) {
                Ok(image) if (image.width, image.height) == (texture.width(), texture.height()) => {
                    self.gfx.update_texture(texture, &image.rgba);
                    self.assets.reloaded_textures.push(texture);
                    self.assets.reloaded_files.push(name.clone());
                    eprintln!("ponko: reloaded {name}");
                }
                Ok(image) => eprintln!(
                    "ponko: {name} is now {} x {} (was {} x {}); restart the game to use the new size",
                    image.width,
                    image.height,
                    texture.width(),
                    texture.height()
                ),
                // Often a file caught halfway through saving; the next save reloads it.
                Err(e) => eprintln!("ponko: could not reload {name}: {e}"),
            }
        }
        self.assets.reload_pending = changed;
    }

    /// Hands one frame's timings to the profiler. `true` when a benchmark
    /// run is done and the game should quit.
    pub(crate) fn record_frame(&mut self, sample: crate::profiler::FrameSample) -> bool {
        let gpu_frame = self.gfx.renderer.as_ref().and_then(Renderer::gpu_frame);
        self.perf.entities = self.world.len();
        self.perf.record_frame(sample, gpu_frame)
    }

    /// Starts or stops the deeper profiling to match what's wanted.
    pub(crate) fn sync_profiling(&mut self) {
        self.perf.sync_profiling();
    }

    /// Ends a frame (or headless tick) that took `ms` for the report.
    pub(crate) fn end_perf_frame(&mut self, ms: f32) {
        self.perf.entities = self.world.len();
        self.perf.end_frame(ms);
    }

    /// Engine side: hands the 3D camera to the audio service, whose default
    /// listener it is. Runs before every system and before the frame's
    /// positional sound update.
    /// Engine side, before each system: gives the services the state of
    /// their neighbours they answer from (the camera for the 3D listener,
    /// the mouse for the UI's pointer), so they need no borrow of `Ctx`.
    #[inline]
    pub(crate) fn sync_services(&mut self) {
        #[cfg(feature = "3d")]
        self.sync_audio_camera();
        self.ui.set_pointer(self.input.mouse_pos());
        self.gfx.sim_tick = self.time.sim_tick();
    }

    #[cfg(feature = "3d")]
    #[inline]
    pub(crate) fn sync_audio_camera(&mut self) {
        self.audio.backend.spatial.camera = crate::d3::AudioListener {
            position: self.scene3d.camera.position,
            rotation: self.scene3d.camera.rotation,
        };
    }

    /// Draws `texture` centered at `pos`, in world units (window pixels until
    /// you move or zoom [`camera`](Self::camera)). A shortcut for
    /// [`ctx.gfx.draw`](crate::gfx::Graphics::draw), which has the details;
    /// [`draw_ex`](crate::gfx::Graphics::draw_ex) scales, rotates and tints.
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.gfx.draw`.
    #[deprecated(since = "0.2.0", note = "use `ctx.gfx.draw`")]
    pub fn draw(&mut self, texture: Texture, pos: Vec2) {
        self.gfx.draw(texture, pos);
    }

    /// Draws `text` with its top-left corner at `pos`, in window pixels (on
    /// top of the world, ignoring the camera). `size` is the line height in
    /// pixels. A shortcut for [`ctx.gfx.draw_text`](crate::gfx::Graphics::draw_text).
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.gfx.draw_text`.
    #[deprecated(since = "0.2.0", note = "use `ctx.gfx.draw_text`")]
    pub fn draw_text(&mut self, text: &str, pos: Vec2, size: f32, color: Color) {
        self.gfx.draw_text(text, pos, size, color);
    }

    /// Loads an image from an [added asset source](crate::assets::Assets::add_source)
    /// or the `assets/` folder. Do this once in setup and keep the
    /// [`Texture`]; it's a cheap handle. A shortcut for
    /// [`ctx.gfx.load_texture`](crate::gfx::Graphics::load_texture), which has
    /// the details.
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.gfx.load_texture`.
    #[deprecated(since = "0.2.0", note = "use `ctx.gfx.load_texture`")]
    pub fn load_texture(&mut self, path: &str) -> Texture {
        self.gfx.load_texture(path)
    }

    /// Loads a sound from an [added asset source](crate::assets::Assets::add_source)
    /// or the `assets/` folder. A shortcut for
    /// [`ctx.audio.load`](crate::audio::Audio::load), which has the details.
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.audio.load`.
    #[deprecated(since = "0.2.0", note = "use `ctx.audio.load`")]
    pub fn load_sound(&mut self, path: &str) -> crate::audio::Sound {
        self.audio.load(path)
    }

    /// Plays a sound once at full volume. A shortcut for
    /// [`ctx.audio.play`](crate::audio::Audio::play); the other ways to play
    /// (volume, pitch, looping, buses, positions) are on
    /// [`ctx.audio`](Self::audio).
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.audio.play`.
    #[deprecated(since = "0.2.0", note = "use `ctx.audio.play`")]
    pub fn play_sound(&mut self, sound: crate::audio::Sound) -> crate::audio::Playing {
        self.audio.play(sound)
    }

    /// Engine side: this context has no window or GPU.
    pub(crate) fn set_headless(&mut self, headless: bool) {
        self.headless = headless;
        self.perf.headless = headless;
        self.gfx.headless = headless;
        self.audio.headless = headless;
    }

    /// Queues the HUD's drawing for this frame (the game loop calls it
    /// before rendering).
    pub(crate) fn draw_perf_hud(&mut self) {
        if !self.perf.profiler.hud || self.perf.profiler.hud_text().is_empty() {
            return;
        }
        if self.perf.state.on && self.perf.state.hud_due() {
            let before = crate::perf::alloc_counts();
            self.perf.entities = self.world.len();
            let report = self.perf.report();
            crate::perf_report::hud_lines(&mut self.perf.state.hud_extra, &report);
            // Building the overlay isn't the game's allocation.
            self.perf.state.forgive_allocs(before);
        }
        let mut text = std::mem::take(self.perf.profiler.hud_text_mut());
        let base = text.len();
        text.push_str(&self.perf.state.hud_extra);
        let lines = text.lines().count() as f32;
        let (size, pad) = (14.0, 8.0);
        let panel = crate::camera::Rect::new(4.0, 4.0, 420.0, lines * size * 1.25 + pad * 2.0);
        self.gfx.ui_fill(panel, Color::rgba(0.0, 0.0, 0.0, 0.6), 100.0);
        self.gfx.draw_text_ex(
            &text,
            panel.min + pad,
            crate::text::TextParams {
                size,
                color: Color::WHITE,
                z: 101.0,
                ..Default::default()
            },
        );
        text.truncate(base);
        *self.perf.profiler.hud_text_mut() = text;
    }

    // ---------------------------------------------------------------- timing

    /// Seconds since the previous frame. Multiply speeds by this for smooth,
    /// frame-rate independent movement. A shortcut for
    /// [`ctx.time.dt()`](crate::time::Time::dt).
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.time.dt()`.
    #[deprecated(since = "0.2.0", note = "use `ctx.time.dt()`")]
    pub fn dt(&self) -> f32 {
        self.time.dt()
    }

    // ---------------------------------------------------------------- keyboard

    /// `true` while `key` is held down.
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.input.key_down`.
    #[deprecated(since = "0.2.0", note = "use `ctx.input.key_down`")]
    pub fn key_down(&self, key: Key) -> bool {
        self.input.key_down(key)
    }

    /// `true` only on the frame `key` went down. Good for jumps, menus, toggles.
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.input.key_pressed`.
    #[deprecated(since = "0.2.0", note = "use `ctx.input.key_pressed`")]
    pub fn key_pressed(&self, key: Key) -> bool {
        self.input.key_pressed(key)
    }

    /// `true` only on the frame `key` was let go.
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.input.key_released`.
    #[deprecated(since = "0.2.0", note = "use `ctx.input.key_released`")]
    pub fn key_released(&self, key: Key) -> bool {
        self.input.key_released(key)
    }

    // ---------------------------------------------------------------- mouse

    /// Mouse position in window pixels. `(0, 0)` is the top-left corner,
    /// `y` grows downward.
    ///
    /// Deprecated: [`Ctx`] holds the services, and each call has one
    /// spelling, on its service: `ctx.input.mouse_pos()`.
    #[deprecated(since = "0.2.0", note = "use `ctx.input.mouse_pos()`")]
    pub fn mouse_pos(&self) -> Vec2 {
        self.input.mouse_pos()
    }

    /// The world point under the mouse: [`ctx.input.mouse_pos()`](crate::input::Input::mouse_pos)
    /// through the [`camera`](Self::camera).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// if ctx.input.mouse_pressed(MouseButton::Left) {
    ///     println!("clicked the world at {}", ctx.mouse_world());
    /// }
    /// # }
    /// ```
    pub fn mouse_world(&self) -> Vec2 {
        self.camera.screen_to_world(self.input.mouse_pos())
    }

    // ---------------------------------------------------------------- gamepads

    // ---------------------------------------------------------------- window

    /// Window size in pixels (logical pixels, so it matches [`ctx.input.mouse_pos()`](crate::input::Input::mouse_pos)
    /// on high-DPI screens).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// let center = ctx.window_size() / 2.0;
    /// # }
    /// ```
    pub fn window_size(&self) -> Vec2 {
        self.window_size
    }

    /// Closes the window and ends the game after this frame.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn update(ctx: &mut Ctx) {
    /// if ctx.input.key_pressed(Key::Escape) { ctx.quit(); }
    /// # }
    /// ```
    pub fn quit(&mut self) {
        self.quit_requested = true;
    }
}

fn save_png(path: &Path, shot: &Screenshot, max_size: u32) -> Result<Value, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let image = fitted(shot, max_size)?;
    image
        .save(path)
        .map_err(|e| format!("could not save the screenshot to {}: {e}", path.display()))?;
    Ok(serde_json::json!({ "path": path, "width": image.width(), "height": image.height() }))
}

/// [`save_png`] into a file the control endpoint claimed, through its
/// handle; the file is removed if it can't be written.
fn save_claimed_png(
    mut claimed: crate::control::ClaimedFile,
    shot: &Screenshot,
    max_size: u32,
) -> Result<Value, String> {
    let path = claimed.path.clone();
    let written = fitted(shot, max_size).and_then(|image| {
        let mut out = std::io::BufWriter::new(claimed.file());
        image
            .write_to(&mut out, image::ImageFormat::Png)
            .and_then(|()| std::io::Write::flush(&mut out).map_err(image::ImageError::IoError))
            .map_err(|e| format!("could not save the screenshot to {}: {e}", path.display()))?;
        Ok((image.width(), image.height()))
    });
    match written {
        Ok((width, height)) => {
            let reply = serde_json::json!({ "path": claimed.path, "width": width, "height": height });
            claimed.keep();
            Ok(reply)
        }
        // The drop removes it.
        Err(e) => Err(e),
    }
}

/// The captured frame, scaled down so neither side exceeds `max_size`
/// (0: full size).
fn fitted(shot: &Screenshot, max_size: u32) -> Result<image::RgbaImage, String> {
    let image = image::RgbaImage::from_raw(shot.width, shot.height, shot.rgba.clone())
        .ok_or("the captured frame has the wrong size")?;
    let longest = shot.width.max(shot.height);
    let image = if max_size > 0 && longest > max_size {
        let (w, h) = (
            (u64::from(shot.width) * u64::from(max_size) / u64::from(longest)).max(1) as u32,
            (u64::from(shot.height) * u64::from(max_size) / u64::from(longest)).max(1) as u32,
        );
        image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    Ok(image)
}

/// Below this many sprites, one thread is faster than many.
const SPRITES_PARALLEL_MIN: usize = 20_000;

/// What the renderer draws for a sprite entity.
#[inline]
fn sprite_instance(t: &Transform2D, s: &Sprite) -> SpriteInstance {
    let (size, uv) = region(s.texture, s.region);
    SpriteInstance::new(
        t.position.into(),
        (size * t.scale).into(),
        s.pivot.into(),
        t.rotation,
        s.color.to_rgba8(),
    )
    .with_uv(uv)
}

/// The drawn size (texture pixels) and UV of `region` of `texture`.
#[inline]
pub(crate) fn region(texture: Texture, region: Option<Rect>) -> (Vec2, [u16; 4]) {
    match region {
        None => (texture.size(), FULL_UV),
        Some(r) => {
            let t = texture.size().max(Vec2::ONE);
            (r.size(), uv_region((r.min / t).into(), (r.size() / t).into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_cache::AssetKind;
    use crate::texture::DrawParams;

    fn timing(dt: f32) -> FrameTiming {
        FrameTiming {
            dt,
            fixed_steps: 1,
            alpha: 0.25,
        }
    }

    fn png(size: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(size, size, image::Rgba([0, 0, 255, 255]))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn source_assets_are_cached_by_source_and_never_watched() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::ZERO);
        ctx.set_headless(true);
        ctx.assets.watcher.set_on(true);
        // An absolute name resolves as itself in the default source.
        let dir = std::env::temp_dir().join(format!("ponko-source-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("crab.png");
        std::fs::write(&file, png(2)).unwrap();
        let name = file.to_str().unwrap().to_string();
        let from_folder = ctx.gfx.load_texture(&name);
        assert_eq!(ctx.assets.watcher.watched(), 1, "default-source files are watched");
        ctx.assets
            .add_source(crate::assets::MemorySource::new("mem").with(name.clone(), png(3)));
        let from_source = ctx.gfx.load_texture(&name);
        assert_ne!(from_source, from_folder, "same name, other source: another asset");
        assert_eq!(from_source.width(), 3);
        assert_eq!(ctx.assets.watcher.watched(), 1, "source assets are not watched");
        let keys = ctx.gfx.loaded_textures.origins();
        assert!(keys.contains(&assets::Origin::Source {
            source: "mem".into(),
            name: name.clone()
        }));
        assert!(keys.iter().any(|o| matches!(o, assets::Origin::File(_))));
        assert_eq!(
            ctx.gfx.load_texture(&name),
            from_source,
            "the source's copy stays cached"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A WAV file of silence, so sounds load with and without the `audio` feature.
    fn wav() -> Vec<u8> {
        let samples = [0i16; 800];
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&8000u32.to_le_bytes());
        out.extend_from_slice(&16_000u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.resize(out.len() + data_len as usize, 0);
        out
    }

    #[test]
    fn sounds_from_sources_are_cached_by_source_and_never_watched() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::ZERO);
        ctx.set_headless(true);
        ctx.assets.watcher.set_on(true);
        let dir = std::env::temp_dir().join(format!("ponko-source-sound-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tone.wav");
        std::fs::write(&file, wav()).unwrap();
        let name = file.to_str().unwrap().to_string();
        let from_folder = ctx.audio.load(&name);
        assert_eq!(ctx.assets.watcher.watched(), 1, "default-source sounds are watched");
        ctx.assets
            .add_source(crate::assets::MemorySource::new("mem").with(name.clone(), wav()));
        let from_source = ctx.audio.load(&name);
        assert_ne!(from_source, from_folder, "same name, other source: another sound");
        assert_eq!(ctx.assets.watcher.watched(), 1, "source sounds are not watched");
        assert_eq!(ctx.audio.load(&name), from_source, "the source's copy stays cached");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn streams_are_never_watched_and_cached_apart_from_sounds() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::ZERO);
        ctx.set_headless(true);
        ctx.assets.watcher.set_on(true);
        ctx.assets
            .add_source(crate::assets::MemorySource::new("mem").with("theme.wav", wav()));
        let stream = ctx.audio.load_stream("theme.wav");
        assert_eq!(ctx.assets.watcher.watched(), 0, "streams are not watched");
        let sound = ctx.audio.load("theme.wav");
        assert_ne!(stream, sound);
        let origin = assets::Origin::Source {
            source: "mem".into(),
            name: "theme.wav".into(),
        };
        assert!(ctx.audio.loaded.contains(&origin, AssetKind::Stream));
        assert!(ctx.audio.loaded.contains(&origin, AssetKind::Sound));
        // Freeing the stream forgets it: loading again makes a new one.
        ctx.free(stream);
        assert_ne!(ctx.audio.load_stream("theme.wav"), stream);
        assert_eq!(ctx.audio.load("theme.wav"), sound);
    }

    #[cfg(feature = "3d")]
    #[test]
    fn shaders_from_sources_are_cached_by_source_and_never_watched() {
        let wgsl = "fn surface(s: Surface) -> Surface { return s; }";
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::ZERO);
        ctx.set_headless(true);
        ctx.assets.watcher.set_on(true);
        let dir = std::env::temp_dir().join(format!("ponko-source-shader-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plain.wgsl");
        std::fs::write(&file, wgsl).unwrap();
        let name = file.to_str().unwrap().to_string();
        let from_folder = ctx.gfx.load_shader(&name);
        assert_eq!(ctx.assets.watcher.watched(), 1, "default-source shaders are watched");
        ctx.assets
            .add_source(crate::assets::MemorySource::new("mem").with(name.clone(), wgsl));
        let from_source = ctx.gfx.load_shader(&name);
        assert_ne!(from_source, from_folder, "same name, other source: another shader");
        assert_eq!(ctx.assets.watcher.watched(), 1, "source shaders are not watched");
        assert_eq!(
            ctx.gfx.load_shader(&name),
            from_source,
            "the source's copy stays cached"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn begin_frame_updates_timing() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(800.0, 600.0));
        ctx.begin_frame(timing(0.5));
        ctx.begin_frame(timing(0.25));
        assert_eq!(ctx.time.frame(), 2);
        assert_eq!(ctx.time.dt(), 0.25);
        assert!((ctx.time.elapsed() - 0.75).abs() < 1e-9);
        assert_eq!(ctx.time.fixed_steps(), 1);
        assert_eq!(ctx.time.fixed_alpha(), 0.25);
    }

    #[test]
    fn fps_is_seeded_then_smoothed() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::ZERO);
        ctx.begin_frame(timing(1.0 / 60.0));
        assert!((ctx.time.fps() - 60.0).abs() < 0.01);
        ctx.begin_frame(timing(1.0 / 30.0));
        assert!(ctx.time.fps() > 30.0 && ctx.time.fps() < 60.0);
    }

    #[test]
    fn screen_projection_maps_corners() {
        let p = screen_projection(Vec2::new(800.0, 600.0));
        let clip = |x: f32, y: f32| p.project_point3(glam::vec3(x, y, 0.0)).truncate();
        assert!(clip(0.0, 0.0).abs_diff_eq(Vec2::new(-1.0, 1.0), 1e-6));
        assert!(clip(800.0, 600.0).abs_diff_eq(Vec2::new(1.0, -1.0), 1e-6));
        assert!(clip(400.0, 300.0).abs_diff_eq(Vec2::ZERO, 1e-6));
    }

    #[test]
    fn draw_without_gpu_queues_sprites() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(800.0, 600.0));
        let tex = Texture::new(0, 16, 8);
        ctx.gfx.draw(tex, Vec2::new(10.0, 20.0));
        ctx.gfx.draw_ex(
            tex,
            Vec2::ZERO,
            DrawParams {
                scale: Vec2::splat(2.0),
                ..Default::default()
            },
        );
        assert_eq!(ctx.gfx.sprite_count(), 2);
        assert_eq!(ctx.render_frame(), FrameStatus::Skipped);
        assert_eq!(ctx.gfx.sprite_count(), 0, "queue is cleared after each frame");
    }

    /// What `render_frame` keeps for the render inspector's history: only
    /// frames the renderer drew. The real coverage is the
    /// `FrameStatus::drew()` mapping that `render_frame` decides with: a
    /// drawn frame needs a renderer, and the renderer needs a window, which
    /// tests don't open (the offscreen GPU tests drive the 3D renderer
    /// directly, without a `Ctx`). The `render_frame()` half below has no
    /// renderer, so its push is skipped before `drew()` is even asked.
    #[test]
    fn only_drawn_frames_enter_the_render_history() {
        for (status, drew) in [
            (FrameStatus::Presented, true),
            (FrameStatus::Offscreen, true),
            (FrameStatus::Occluded, false),
            (FrameStatus::Skipped, false),
        ] {
            assert_eq!(status.drew(), drew, "{status:?}");
        }
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(800.0, 600.0));
        for _ in 0..3 {
            assert_eq!(ctx.render_frame(), FrameStatus::Skipped);
        }
        assert!(ctx.perf.render_history.is_empty(), "no renderer: nothing was drawn");
    }

    #[test]
    fn quit_sets_flag() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::ZERO);
        assert!(!ctx.quit_requested());
        ctx.quit();
        assert!(ctx.quit_requested());
    }
}
