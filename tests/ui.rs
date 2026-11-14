//! Camera, shapes, text and UI, checked headless (no window or GPU).

use ponko::prelude::*;

/// Runs `f` inside one headless tick.
fn in_tick(f: impl FnMut(&mut Ctx) + 'static) {
    Game::new("ui", 800, 600).sim_system(f).run_headless(1, |_| {});
}

#[test]
fn the_default_camera_shows_world_units_as_window_pixels() {
    in_tick(|ctx| {
        assert_eq!(ctx.camera.zoom, 1.0);
        assert_eq!(ctx.camera.viewport(), vec2(800.0, 600.0));
        assert_eq!(ctx.camera.screen_to_world(vec2(30.0, 40.0)), vec2(30.0, 40.0));
        assert_eq!(ctx.mouse_world(), ctx.input.mouse_pos());
    });
}

#[test]
fn the_camera_maps_the_mouse_into_the_world() {
    in_tick(|ctx| {
        ctx.camera.zoom = 2.0;
        ctx.camera.look_at(vec2(1000.0, 500.0));
        // The window's middle shows the point the camera looks at.
        assert_eq!(ctx.camera.screen_to_world(vec2(400.0, 300.0)), vec2(1000.0, 500.0));
        let r = ctx.camera.visible_rect();
        assert_eq!(r.size(), vec2(400.0, 300.0));
        assert_eq!(ctx.camera.world_to_screen(r.max), vec2(800.0, 600.0));
    });
}

#[test]
fn shapes_and_screen_sprites_are_queued_like_sprites() {
    in_tick(|ctx| {
        let tex = ctx.gfx.create_texture(1, 1, &[255; 4]);
        ctx.gfx.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::RED);
        ctx.gfx.draw_line(Vec2::ZERO, vec2(5.0, 5.0), 1.0, Color::RED);
        ctx.gfx.draw_circle(Vec2::ZERO, 3.0, Color::RED);
        ctx.gfx
            .draw_ex(tex, Vec2::ZERO, DrawParams::default().with_screen(true));
        assert_eq!(ctx.gfx.sprite_count(), 4);
    });
}

#[test]
fn text_is_measured_headless_but_not_drawn() {
    in_tick(|ctx| {
        let one = ctx.gfx.measure_text("Ants: 5000", 20.0);
        let two = ctx.gfx.measure_text("Ants: 5000\nFood: 12", 20.0);
        assert!(one.x > 50.0 && one.x < 150.0, "{one}");
        assert!((two.y - 2.0 * one.y).abs() < 0.5, "two lines: {one} {two}");
        assert!(ctx.gfx.measure_text("wide text", 40.0).x > ctx.gfx.measure_text("wide text", 20.0).x);
        ctx.gfx.draw_text("hello", Vec2::ZERO, 20.0, Color::WHITE);
        assert_eq!(ctx.gfx.sprite_count(), 0, "no glyph atlas without a GPU");
    });
}

#[test]
fn ui_takes_the_mouse_and_buttons_need_a_click() {
    in_tick(|ctx| {
        // The mouse starts at (0, 0), inside this button.
        let button = Rect::new(0.0, 0.0, 100.0, 30.0);
        assert!(!ctx.ui.wants_pointer());
        assert!(!ctx.immediate_ui().button(button, "Go"), "hovering isn't clicking");
        assert!(ctx.ui.wants_pointer());
        ctx.immediate_ui().panel(Rect::new(500.0, 500.0, 10.0, 10.0));
        assert!(ctx.immediate_ui().label(vec2(500.0, 500.0), "label").y > 0.0);
    });
}

#[test]
fn rect_helpers() {
    let r = Rect::new(10.0, 10.0, 20.0, 10.0);
    assert_eq!(r.center(), vec2(20.0, 15.0));
    assert!(r.contains(r.min) && r.contains(r.max));
    assert!(!r.contains(vec2(9.9, 15.0)));
    assert_eq!(r.expand(2.0).size(), vec2(24.0, 14.0));
}

#[test]
fn loading_a_font_file_twice_gives_the_same_font() {
    let path = std::env::temp_dir().join(format!("ponko-font-{}.ttf", std::process::id()));
    std::fs::write(&path, epaint_default_fonts::UBUNTU_LIGHT).unwrap();
    let name = path.to_str().unwrap().to_string();
    Game::new("fonts", 1, 1).run_headless(0, |ctx| {
        let a = ctx.gfx.load_font(&name);
        assert_eq!(a, ctx.gfx.load_font(&name));
        assert_ne!(a, Font::DEFAULT);
    });
}
