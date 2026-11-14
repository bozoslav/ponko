//! The service fields of `Ctx` (`ctx.time`, `ctx.input`, ...): each is its
//! own field, so it borrows apart from the world and from the others, and
//! the flat `Ctx` methods from before them still forward.

mod common;

use common::TempAsset;
#[allow(unused_imports)]
use ponko::actions::InputEvent;
#[allow(unused_imports)]
use ponko::audio::AudioEffects;
#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::{AudioListener, ReflectionProbe, Transform};
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{ComputePass, GraphicsQuality, MaterialDesc, Mesh, MeshData};
#[allow(unused_imports)]
use ponko::ui::{UiEvent, UiId, UiInput, UiWidget};

#[derive(Component)]
struct Pos(f32);

#[test]
fn time_reads_while_a_world_query_is_alive() {
    // ANCHOR: time_borrow
    fn fall(ctx: &mut Ctx) {
        // The query borrows `ctx.world`; the clock is another field.
        for p in ctx.world.query::<&mut Pos>().iter() {
            p.0 += ctx.time.dt();
        }
    }
    // ANCHOR_END: time_borrow
    let mut world = Game::new("t", 1, 1)
        .fixed_hz(10)
        .sim_system(fall)
        .run_headless(10, |ctx| {
            ctx.world.spawn((Pos(0.0),));
        });
    let p = world.query::<&Pos>().iter().next().unwrap().0;
    assert!((p - 1.0).abs() < 1e-5, "ten ticks of 0.1 s, got {p}");
}

#[test]
fn input_reads_while_a_world_query_is_alive() {
    fn jump(ctx: &mut Ctx) {
        for p in ctx.world.query::<&mut Pos>().iter() {
            if ctx.input.key_down(Key::Space) {
                p.0 += 1.0;
            }
        }
    }
    let mut world = Game::new("t", 1, 1).sim_system(jump).run_headless(3, |ctx| {
        ctx.world.spawn((Pos(0.0),));
        ctx.input.feed(InputEvent::Key(Key::Space, true));
    });
    assert_eq!(world.query::<&Pos>().iter().next().unwrap().0, 3.0);
}

#[test]
fn dt_in_a_sim_system_is_the_sim_step_and_frame_dt_stays_the_frames() {
    fn check(ctx: &mut Ctx) {
        assert_eq!(ctx.time.dt(), ctx.time.sim_dt());
        assert_eq!(ctx.time.sim_dt(), ctx.time.fixed_dt());
        assert_eq!(ctx.time.dt(), ctx.time.dt(), "the shortcut is the same number");
        // A headless run has one frame per tick.
        assert_eq!(ctx.time.frame_dt(), ctx.time.fixed_dt());
    }
    Game::new("t", 1, 1)
        .fixed_hz(50)
        .sim_system(check)
        .run_headless(4, |ctx| {
            // Setup is outside the simulation: `dt` is the frame's, and no frame ran yet.
            assert_eq!(ctx.time.dt(), ctx.time.frame_dt());
            assert_eq!(ctx.time.frame_dt(), 0.0);
            assert_eq!(ctx.time.sim_dt(), 1.0 / 50.0);
        });
}

#[test]
fn sim_dt_covers_every_step_of_a_slower_system() {
    fn slow(ctx: &mut Ctx) {
        assert_eq!(ctx.time.sim_dt(), 4.0 * ctx.time.fixed_dt());
        assert_eq!(ctx.time.dt(), ctx.time.sim_dt());
        ctx.world.resource_mut::<Runs>().0 += 1;
    }
    struct Runs(u32);
    let world = Game::new("t", 1, 1)
        .add(Phase::AfterPhysics, slow.every(4))
        .run_headless(8, |ctx| ctx.world.insert_resource(Runs(0)));
    assert_eq!(world.resource::<Runs>().0, 2);
}

#[test]
fn the_clock_counts_ticks_and_controls_the_simulation() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        assert_eq!(ctx.time.sim_tick(), 0);
        assert_eq!(ctx.time.speed(), 1.0);
        ctx.time.set_speed(4.0);
        assert_eq!(ctx.time.speed(), 4.0);
        assert!(!ctx.time.is_paused());
        ctx.time.pause();
        assert!(ctx.time.is_paused());
        ctx.time.resume();
        assert!(!ctx.time.is_paused());
    });
    fn count(ctx: &mut Ctx) {
        let tick = ctx.time.sim_tick();
        assert_eq!(ctx.time.sim_time(), tick as f64 * f64::from(ctx.time.fixed_dt()));
    }
    Game::new("t", 1, 1)
        .fixed_hz(64)
        .sim_system(count)
        .run_headless(5, |_| {});
}

#[test]
#[should_panic(expected = "Time::set_speed: speed must be a finite number >= 0")]
fn a_negative_sim_speed_panics_with_the_service_name() {
    Game::new("t", 1, 1).run_headless(0, |ctx| ctx.time.set_speed(-1.0));
}

#[test]
fn input_edges_and_the_cursor_lock_live_on_the_input_service() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.input.feed(InputEvent::Mouse(MouseButton::Left, true));
        assert!(ctx.input.mouse_pressed(MouseButton::Left));
        assert!(ctx.input.mouse_down(MouseButton::Left));
        assert!(!ctx.input.cursor_locked());
        ctx.input.set_cursor_locked(true);
        assert!(ctx.input.cursor_locked());
        ctx.input.insert_text("hi");
        assert_eq!(ctx.input.typed_text(), "hi");
        // The shortcuts read the same state.
        ctx.input.feed(InputEvent::Key(Key::A, true));
        assert!(ctx.input.key_down(Key::A) && ctx.input.key_pressed(Key::A));
        assert_eq!(ctx.input.mouse_pos(), ctx.input.mouse_pos());
    });
}

#[test]
fn perf_reads_while_a_world_query_is_alive_and_scopes_borrow_nothing() {
    fn work(ctx: &mut Ctx) {
        // The guard holds no borrow of `ctx`, so the timed code may use all of it.
        let _timing = ctx.perf.scope("services/work");
        for p in ctx.world.query::<&mut Pos>().iter() {
            p.0 += ctx.perf.system_timings().len() as f32;
        }
        ctx.commands.spawn((Pos(0.0),));
    }
    let mut world = Game::new("t", 1, 1).sim_system(work).run_headless(2, |ctx| {
        ctx.perf.set_profiling(true);
        ctx.world.spawn((Pos(0.0),));
    });
    assert_eq!(world.query::<&Pos>().count(), 3);
}

#[test]
fn the_report_counts_the_entities_left_by_the_last_system() {
    fn grow(ctx: &mut Ctx) {
        ctx.commands.spawn((Pos(0.0),));
    }
    struct Seen(Vec<usize>);
    fn look(ctx: &mut Ctx) {
        let entities = ctx.perf.report().entities;
        ctx.world.resource_mut::<Seen>().0.push(entities);
    }
    let world = Game::new("t", 1, 1)
        .sim_system(grow)
        .sim_system(look)
        .run_headless(3, |ctx| ctx.world.insert_resource(Seen(Vec::new())));
    assert_eq!(world.resource::<Seen>().0, [1, 2, 3]);
}

#[test]
fn the_hud_switch_turns_profiling_on_and_scopes_get_timed() {
    fn timed(ctx: &mut Ctx) {
        let _timing = ctx.perf.scope("services/timed");
        std::hint::black_box((0..1000).sum::<u64>());
    }
    struct Names(Vec<&'static str>);
    fn read(ctx: &mut Ctx) {
        let names: Vec<_> = ctx.perf.scope_timings().iter().map(|s| s.name).collect();
        ctx.world.resource_mut::<Names>().0 = names;
    }
    let world = Game::new("t", 1, 1)
        .sim_system(timed)
        .sim_system(read)
        .run_headless(3, |ctx| {
            ctx.world.insert_resource(Names(Vec::new()));
            assert!(!ctx.perf.profiling());
            ctx.perf.set_hud(true);
            assert!(ctx.perf.hud());
            assert!(ctx.perf.profiling(), "the overlay needs the deeper numbers");
        });
    assert!(world.resource::<Names>().0.contains(&"services/timed"));
}

struct Beep(Sound);

#[test]
fn sounds_play_while_a_world_query_is_alive() {
    fn beep_each(ctx: &mut Ctx) {
        let beep = ctx.world.resource::<Beep>().0;
        for p in ctx.world.query::<&mut Pos>().iter() {
            // `ctx.audio` is its own field: no need to collect first.
            let playing = ctx.audio.play(beep);
            ctx.audio.stop(playing);
            p.0 += 1.0;
        }
    }
    let mut world = Game::new("t", 1, 1).sim_system(beep_each).run_headless(2, |ctx| {
        let beep = ctx.audio.create(8000, &[0.0; 80]);
        ctx.world.insert_resource(Beep(beep));
        ctx.world.spawn_batch([(Pos(0.0),), (Pos(0.0),)]);
    });
    assert!(world.query::<&Pos>().iter().all(|p| p.0 == 2.0));
}

#[test]
fn the_audio_service_owns_sounds_buses_and_the_shortcuts_reach_it() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let beep = ctx.audio.create(8000, &[0.0; 8000]);
        assert_eq!(beep.seconds(), 1.0);
        assert!(ctx.is_live(beep));
        // The shortcut plays through the same service.
        let playing = ctx.audio.play(beep);
        ctx.audio.stop(playing);
        let music = ctx.audio.bus("music");
        assert_eq!(ctx.audio.bus("music"), music, "a name gives the same bus");
        ctx.audio.set_bus_volume(music, 0.25);
        ctx.audio.mute_bus(music, true);
        let on_bus = ctx
            .audio
            .play_on(beep, SoundParams::default(), music, AudioEffects::default());
        ctx.audio.set_effects(on_bus, AudioEffects::default());
        ctx.audio.stop(on_bus);
        assert!(ctx.audio.try_load("missing.ogg").is_err());
        ctx.free(beep);
        assert!(!ctx.is_live(beep));
    });
}

#[cfg(feature = "3d")]
#[test]
fn the_default_listener_is_the_camera_as_the_running_system_found_it() {
    fn listen(ctx: &mut Ctx) {
        // The camera the system started with is where the ears are...
        assert_eq!(ctx.audio.listener().position, ctx.scene3d.camera.position);
        // ...and a listener of the game's own replaces it at once.
        let ears = AudioListener {
            position: vec3(1.0, 2.0, 3.0),
            rotation: Quat::IDENTITY,
        };
        ctx.audio.set_listener(Some(ears));
        assert_eq!(ctx.audio.listener().position, ears.position);
        ctx.audio.set_listener(None);
        ctx.scene3d.camera.position.x += 1.0;
    }
    Game::new("t", 1, 1).sim_system(listen).run_headless(3, |ctx| {
        ctx.scene3d.camera.position = vec3(5.0, 0.0, 0.0);
    });
}

#[derive(Component)]
struct Dot(Texture);

#[test]
fn drawing_queues_while_a_world_query_is_alive() {
    struct Queued(usize);
    fn draw_each(ctx: &mut Ctx) {
        // `ctx.gfx` is its own field: draw from inside the query.
        for (dot, at) in ctx.world.query::<(&Dot, &Transform2D)>().iter() {
            ctx.gfx.draw(dot.0, at.position);
            ctx.gfx
                .draw_rect(Rect::new(at.position.x, at.position.y, 1.0, 1.0), Color::WHITE);
        }
        let queued = ctx.gfx.sprite_count();
        ctx.world.resource_mut::<Queued>().0 = queued;
    }
    let world = Game::new("t", 8, 8).sim_system(draw_each).run_headless(1, |ctx| {
        let pixel = ctx.gfx.create_texture(1, 1, &[255; 4]);
        ctx.world.insert_resource(Queued(0));
        ctx.world
            .spawn_batch((0..3).map(|i| (Dot(pixel), Transform2D::at(vec2(i as f32, 0.0)))));
    });
    assert_eq!(world.resource::<Queued>().0, 6, "a sprite and a rectangle per entity");
}

#[test]
fn the_graphics_service_owns_textures_and_the_shortcuts_reach_it() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let before = ctx.live_counts().textures;
        let pixel = ctx.gfx.create_texture(2, 2, &[255; 16]);
        assert_eq!((pixel.width(), pixel.height()), (2, 2));
        assert!(ctx.is_live(pixel));
        assert_eq!(ctx.live_counts().textures, before + 1);
        ctx.gfx.update_texture(pixel, &[0; 16]);
        ctx.gfx.set_texture_filter(pixel, TextureFilter::Nearest);
        // The shortcuts queue through the same service.
        ctx.gfx.draw(pixel, Vec2::ZERO);
        ctx.gfx.draw_ex(pixel, Vec2::ZERO, DrawParams::default());
        assert_eq!(ctx.gfx.sprite_count(), 2);
        ctx.gfx
            .draw_text("headless text is measured, not drawn", Vec2::ZERO, 16.0, Color::WHITE);
        assert!(ctx.gfx.measure_text("wide", 16.0).x > 0.0);
        assert!(ctx.gfx.is_live(Font::DEFAULT));
        assert!(ctx.gfx.try_load_texture("missing.png").is_err());
        assert!(ctx.gfx.try_load_font("missing.ttf").is_err());
        assert!(ctx.gfx.gpu_capabilities().is_none(), "no GPU in a headless run");
        ctx.free(pixel);
        assert!(!ctx.is_live(pixel));
    });
}

#[test]
#[should_panic(expected = "a 2 x 2 texture needs 16 bytes of RGBA, got 4")]
fn a_texture_with_the_wrong_byte_count_panics_from_the_service() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.gfx.create_texture(2, 2, &[255; 4]);
    });
}

#[test]
fn a_texture_loaded_by_the_service_is_saved_by_file_name() {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let world = Game::new("t", 1, 1).hot_reload(false).run_headless(0, |ctx| {
        ctx.assets.add_source(MemorySource::new("mem").with("dot.png", png));
        // The service needs no world to load; the world still learns the name.
        let dot = ctx.gfx.load_texture("dot.png");
        assert_eq!(ctx.gfx.load_texture("dot.png"), dot, "a file is loaded once");
        ctx.world.spawn((Transform2D::default(), Sprite::new(dot)));
        // A freed texture's name goes with it.
        let gone = ctx.gfx.create_texture(1, 1, &[0; 4]);
        ctx.world.spawn((Transform2D::default(), Sprite::new(gone)));
        ctx.free(gone);
    });
    let save = world.save().to_string();
    assert_eq!(save.matches("\"texture\":\"dot.png\"").count(), 1, "{save}");
}

fn sized(w: f32, h: f32) -> ponko::ui::unstable_taffy::Style {
    ponko::ui::unstable_taffy::Style {
        size: ponko::ui::unstable_taffy::Size {
            width: ponko::ui::unstable_taffy::Dimension::length(w),
            height: ponko::ui::unstable_taffy::Dimension::length(h),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

#[test]
fn ui_events_are_read_while_sounds_play() {
    struct Clicks(Vec<UiId>);
    // ANCHOR: ui_borrow
    fn menu(ctx: &mut Ctx) {
        let click = ctx.world.resource::<Beep>().0;
        // `ctx.ui.events()` borrows the UI; `ctx.audio` and `ctx.world` are other fields.
        for event in ctx.ui.events() {
            if let UiEvent::Clicked(id) = event {
                let playing = ctx.audio.play(click);
                ctx.audio.stop(playing);
                ctx.world.resource_mut::<Clicks>().0.push(id);
            }
        }
    }
    // ANCHOR_END: ui_borrow
    let mut button = None;
    let world = Game::new("t", 800, 600).sim_system(menu).run_headless(2, |ctx| {
        let beep = ctx.audio.create(8000, &[0.0; 80]);
        ctx.world.insert_resource(Beep(beep));
        ctx.world.insert_resource(Clicks(Vec::new()));
        let root = ctx
            .ui
            .add(UiWidget::Panel, ponko::ui::UiLayout::custom(sized(300.0, 200.0)));
        button = Some(
            ctx.ui
                .add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(70.0, 30.0))),
        );
        ctx.ui.push_screen(root, false);
        assert!(ctx.ui.wants_keyboard(), "a screen is open");
        // A headless run has no window to feed the tree, so give it a frame
        // of input by hand: focus the button and confirm.
        let input = UiInput {
            next: true,
            confirm: true,
            ..Default::default()
        };
        ctx.ui.frame(ctx.window_size(), &input);
    });
    assert_eq!(world.resource::<Clicks>().0, [button.unwrap()]);
}

#[test]
fn immediate_widgets_take_the_services_they_use() {
    Game::new("t", 800, 600).run_headless(0, |ctx| {
        ctx.ui.immediate.style.text_size = 20.0;
        let panel = Rect::new(100.0, 100.0, 200.0, 100.0);
        assert!(!ctx.ui.immediate.covers(vec2(150.0, 150.0)));
        ctx.immediate_ui().panel(panel);
        assert!(ctx.ui.immediate.covers(vec2(150.0, 150.0)));
        assert_eq!(ctx.gfx.sprite_count(), 1, "the panel is one sprite");
        let size = ctx.immediate_ui().label(panel.min, "label");
        assert_eq!(size, ctx.gfx.measure_text("label", 20.0));

        // A click is a press and a release over the button.
        let button = Rect::new(0.0, 0.0, 100.0, 30.0);
        // The mouse starts at (0, 0), inside it.
        ctx.input.feed(InputEvent::Mouse(MouseButton::Left, true));
        assert!(!ctx.immediate_ui().button(button, "Go"));
        ctx.input.feed(InputEvent::Mouse(MouseButton::Left, false));
        assert!(ctx.immediate_ui().button(button, "Go"));
    });
}

#[test]
fn the_ui_has_the_pointer_over_immediate_widgets_drawn_this_frame_or_the_last() {
    struct Seen(Vec<bool>);
    fn overlay(ctx: &mut Ctx) {
        let over = ctx.ui.wants_pointer();
        ctx.world.resource_mut::<Seen>().0.push(over);
        if ctx.time.sim_tick() == 0 {
            // The mouse starts at (0, 0), inside this panel.
            ctx.immediate_ui().panel(Rect::new(0.0, 0.0, 50.0, 50.0));
            assert!(ctx.ui.wants_pointer(), "drawn this frame");
        }
    }
    let world = Game::new("t", 800, 600).sim_system(overlay).run_headless(3, |ctx| {
        ctx.world.insert_resource(Seen(Vec::new()));
    });
    // Not drawn yet; drawn the frame before; not drawn for a whole frame.
    assert_eq!(world.resource::<Seen>().0, [false, true, false]);
}

#[derive(serde::Deserialize)]
struct Rules {
    speed: f32,
}

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize, Component)]
struct Stats {
    hp: f32,
}

#[test]
fn data_files_load_while_a_world_query_is_alive() {
    fn give_stats(ctx: &mut Ctx) {
        // Read data files for the entities the query finds: the files load
        // into another world here, since the query has the game's.
        let mut scratch = World::new();
        for p in ctx.world.query::<&mut Pos>().iter() {
            let found = ctx
                .assets
                .load_with("rules.json", |bytes| Ok(serde_json::from_slice::<Rules>(bytes)?));
            p.0 = found.unwrap().speed;
            ctx.assets.load_resource::<Rules>(&mut scratch, "rules.json");
        }
        assert_eq!(scratch.resource::<Rules>().speed, 30.0);
    }
    let mut world = Game::new("t", 1, 1)
        .hot_reload(false)
        .sim_system(give_stats)
        .run_headless(1, |ctx| {
            ctx.assets
                .add_source(MemorySource::new("mem").with("rules.json", r#"{"speed": 30}"#));
            ctx.world.spawn((Pos(0.0),));
        });
    assert_eq!(world.query::<&Pos>().iter().next().unwrap().0, 30.0);
}

#[test]
fn the_assets_service_fills_resources_and_components_of_the_world_it_is_given() {
    Game::new("t", 1, 1).hot_reload(false).run_headless(0, |ctx| {
        assert!(!ctx.assets.hot_reload());
        ctx.assets.add_source(
            MemorySource::new("mem")
                .with("rules.json", r#"{"speed": 30}"#)
                .with("stats.json", r#"{"hp": 7}"#),
        );
        ctx.assets.load_resource::<Rules>(&mut ctx.world, "rules.json");
        assert_eq!(ctx.world.resource::<Rules>().speed, 30.0);

        ctx.world.reflect::<Stats>();
        let e = ctx.world.spawn((Stats::default(),));
        ctx.assets
            .load_component_json::<Stats>(&mut ctx.world, "stats.json", e)
            .unwrap();
        assert_eq!(ctx.world.get::<Stats>(e).unwrap().hp, 7.0);

        ctx.world.reflect_resource::<Stats>();
        ctx.assets
            .load_resource_json::<Stats>(&mut ctx.world, "stats.json")
            .unwrap();
        assert_eq!(ctx.world.resource::<Stats>().hp, 7.0);

        let missing = ctx
            .assets
            .load_component_json::<Stats>(&mut ctx.world, "missing.json", e);
        assert!(missing.is_err());
        assert!(ctx.assets.reloaded_files().is_empty() && ctx.assets.reloaded_textures().is_empty());
    });
}

#[test]
#[should_panic(expected = "rules.json")]
fn a_missing_data_file_panics_with_its_name() {
    Game::new("t", 1, 1).hot_reload(false).run_headless(0, |ctx| {
        ctx.assets
            .load_resource::<Rules>(&mut ctx.world, "no-such-dir/rules.json");
    });
}

#[test]
fn scenes_and_prefabs_are_saved_from_the_world_they_are_given() {
    let scene = TempAsset::new("services-scene.json");
    let prefab = TempAsset::new("services-prefab.json");
    let (scene, prefab) = (scene.name(), prefab.name());
    Game::new("t", 1, 1).hot_reload(false).run_headless(0, |ctx| {
        ctx.world.reflect::<Stats>();
        let e = ctx.world.spawn((Transform2D::at(vec2(3.0, 4.0)), Stats { hp: 5.0 }));
        ctx.assets.save_scene(&ctx.world, scene).unwrap();
        ctx.assets.save_prefab(&ctx.world, e, prefab).unwrap();

        // The prefab is cached by the service that saved it; the scene comes back from its file.
        let copy = ctx.spawn_prefab(prefab, vec2(9.0, 9.0)).unwrap();
        assert_eq!(ctx.world.get::<Stats>(copy).unwrap().hp, 5.0);
        assert_eq!(ctx.world.get::<Transform2D>(copy).unwrap().position, vec2(9.0, 9.0));
        ctx.load_scene(scene).unwrap();
        assert_eq!(ctx.world.query::<&Stats>().count(), 1);

        let dead = ctx.world.spawn((Stats::default(),));
        ctx.world.despawn(dead);
        assert!(ctx.assets.save_prefab(&ctx.world, dead, prefab).is_err());
    });
}

#[cfg(feature = "3d")]
mod three {
    #[allow(unused_imports)]
    use ponko::actions::InputEvent;
    #[allow(unused_imports)]
    use ponko::audio::AudioEffects;
    #[allow(unused_imports)]
    use ponko::d2::Sprite;
    #[allow(unused_imports)]
    use ponko::d2::Transform as Transform2D;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::d3::AudioListener;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::d3::ReflectionProbe;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::d3::Transform;
    use ponko::prelude::*;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::render::ComputePass;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::render::GraphicsQuality;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::render::MaterialDesc;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::render::Mesh;
    #[allow(unused_imports)]
    #[cfg(feature = "3d")]
    use ponko::render::MeshData;
    #[allow(unused_imports)]
    use ponko::ui::UiEvent;
    #[allow(unused_imports)]
    use ponko::ui::UiId;
    #[allow(unused_imports)]
    use ponko::ui::UiInput;
    #[allow(unused_imports)]
    use ponko::ui::UiWidget;

    #[derive(Component)]
    struct Box3(Mesh);

    #[test]
    fn meshes_are_made_while_a_world_query_is_alive() {
        fn grow(ctx: &mut Ctx) {
            let material = ctx.gfx.default_material();
            for b in ctx.world.query::<&mut Box3>().iter() {
                // The world is borrowed by the query; `ctx.gfx` is free.
                let bigger = ctx.gfx.create_mesh(&MeshData::cube(2.0), material);
                ctx.gfx.free(b.0);
                b.0 = bigger;
            }
        }
        let mut world = Game::new("t", 1, 1).sim_system(grow).run_headless(2, |ctx| {
            let material = ctx.gfx.default_material();
            let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            ctx.world.spawn((Box3(mesh),));
        });
        assert_eq!(world.query::<&Box3>().count(), 1);
    }

    #[test]
    fn the_calls_that_touch_entities_take_the_world() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let probe = ctx.world.spawn((Transform::default(), ReflectionProbe::default()));
            ctx.gfx.bake_reflection_probe(&mut ctx.world, probe).unwrap();
            assert_ne!(ctx.world.get::<ReflectionProbe>(probe).unwrap().content, 0);
            let bare = ctx.world.spawn((Transform::default(),));
            assert!(ctx.gfx.bake_reflection_probe(&mut ctx.world, bare).is_err());
            ctx.gfx.write_animated_morphs(&mut ctx.world);
        });
    }

    #[test]
    fn freeing_a_gpu_buffer_drops_its_compute_passes() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let buffer = ctx.gfx.create_gpu_buffer(4, &[]);
            let update = ctx.gfx.create_compute("up", "fn update(i: u32) { items[i].x += 1.0; }");
            ctx.scene3d.passes.compute.push(ComputePass::for_buffer(update, buffer));
            assert!(ctx.is_live(buffer));
            ctx.free(buffer);
            assert!(ctx.scene3d.passes.compute.is_empty());
            assert_eq!(ctx.gfx.gpu_buffer_len(buffer), None);
            // The slot is reused under a new id, and starts clean.
            let again = ctx.gfx.create_gpu_buffer(2, &[]);
            use ponko::__ffi::{self, SLOT_MASK}; // raw ids, to see the slot
            assert_eq!(__ffi::id(again) & SLOT_MASK, __ffi::id(buffer) & SLOT_MASK);
            assert_ne!(again, buffer);
            assert_eq!(ctx.gfx.gpu_buffer_len(again), Some(2));
        });
    }

    #[test]
    fn quality_presets_write_the_post_and_lighting_settings() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            ctx.scene3d.set_quality(GraphicsQuality::Low);
            assert_eq!(ctx.scene3d.quality(), Some(GraphicsQuality::Low));
            assert_eq!(ctx.scene3d.post.render_scale, 0.67);
            // A tweak on top stays, and the preset is gone.
            ctx.scene3d.lighting.shadow_distance += 1.0;
            assert_eq!(ctx.scene3d.quality(), None);
        });
    }

    #[test]
    fn the_pointer_ray_for_world_panels_is_set_on_the_ui() {
        Game::new("t", 800, 600).run_headless(1, |ctx| {
            let (panel, model) = ctx.gfx.create_ui_panel(64, 64, vec2(1.0, 1.0));
            ctx.world.spawn((Transform::at(vec3(0.0, 0.0, -2.0)), model, panel));
            ctx.ui.set_pointer_ray(Some((Vec3::ZERO, -Vec3::Z)));
            ctx.ui.set_pointer_ray(None);
        });
    }

    #[test]
    fn handles_from_the_service_are_live_until_freed() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let material = ctx.gfx.create_material(MaterialDesc::default());
            let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            assert!(ctx.is_live(material) && ctx.is_live(mesh));
            assert!(ctx.gfx.is_live(material) && ctx.gfx.is_live(mesh));
            ctx.free(mesh);
            ctx.free(material);
            assert!(!ctx.is_live(material) && !ctx.is_live(mesh));
            assert!(!ctx.gfx.is_live(material));
        });
    }
}

/// The public methods of `Ctx` that rustdoc shows: every `pub fn` in an
/// `impl Ctx` block of the engine's source that isn't `#[doc(hidden)]`,
/// with whether it is `#[deprecated]`.
fn visible_ctx_methods() -> Vec<(String, bool)> {
    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    rust_files(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut methods = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let mut in_ctx = false;
        // Attributes and doc comments since the last item or blank line.
        let mut hidden = false;
        let mut deprecated = false;
        for line in text.lines() {
            if !in_ctx {
                in_ctx = line == "impl Ctx {" || line == "impl crate::ctx::Ctx {";
                continue;
            }
            if line == "}" {
                in_ctx = false;
            } else if line.trim() == "#[doc(hidden)]" {
                hidden = true;
            } else if line.trim().starts_with("#[deprecated") {
                deprecated = true;
            } else if let Some(rest) = line.strip_prefix("    pub fn ") {
                if !hidden {
                    let name = rest.split(['(', '<']).next().unwrap();
                    methods.push((name.to_string(), deprecated));
                }
                hidden = false;
                deprecated = false;
            } else if line.trim().is_empty() || line == "    }" {
                hidden = false;
                deprecated = false;
            }
        }
    }
    methods.sort();
    methods
}

#[test]
fn ctx_keeps_fewer_than_forty_documented_methods() {
    let all = visible_ctx_methods();
    let methods: Vec<&str> = all.iter().filter(|m| !m.1).map(|m| m.0.as_str()).collect();
    assert!(
        methods.len() < 40,
        "`Ctx` has {} documented methods; a new call belongs on a service (`ctx.gfx`, `ctx.audio`, ...): {methods:?}",
        methods.len()
    );
    // One spelling per call: the 0.1 shortcuts that only forwarded to one
    // service are deprecated, and only calls that span services stay.
    for shortcut in [
        "dt",
        "key_down",
        "key_pressed",
        "key_released",
        "mouse_pos",
        "draw",
        "draw_text",
        "load_texture",
        "load_sound",
        "play_sound",
    ] {
        assert!(
            all.iter().any(|m| m.0 == shortcut && m.1),
            "`Ctx::{shortcut}` duplicates a service call, so it stays deprecated"
        );
    }
    for kept in ["quit", "free", "is_live", "mouse_world", "immediate_ui"] {
        assert!(methods.contains(&kept), "`Ctx::{kept}` is the one spelling");
    }
}

#[test]
#[allow(deprecated)]
fn deprecated_ctx_shortcuts_still_forward_to_their_services() {
    // Kept for one minor so 0.1 code still builds (with a warning naming the
    // service call); each must answer as its service does.
    Game::new("t", 800, 600).run_headless(1, |ctx| {
        assert_eq!(ctx.mouse_pos(), ctx.input.mouse_pos());
        assert_eq!(ctx.dt(), ctx.time.dt());
        assert_eq!(ctx.key_down(Key::Space), ctx.input.key_down(Key::Space));
        assert_eq!(ctx.key_pressed(Key::Space), ctx.input.key_pressed(Key::Space));
        assert_eq!(ctx.key_released(Key::Space), ctx.input.key_released(Key::Space));
        let beep = ctx.audio.create(8000, &[0.0; 80]);
        let playing = ctx.play_sound(beep);
        ctx.audio.stop(playing);
        assert!(ctx.immediate_ui().label(vec2(0.0, 0.0), "x").y > 0.0);
        assert!(ctx.ui.immediate.label(&mut ctx.gfx, vec2(0.0, 0.0), "x").y > 0.0);
    });
}
