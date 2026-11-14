use ponko::prelude::*;
use ponko::ui::unstable_taffy::prelude::length;
use ponko::ui::unstable_taffy::{Dimension, Display, FlexDirection, LengthPercentage, Rect as Edges, Size, Style};
#[allow(unused_imports)]
use ponko::ui::{Ui, UiEvent, UiId, UiInput, UiLayout, UiPaint, UiStyle, UiTheme, UiWidget};

fn sized(w: f32, h: f32) -> Style {
    Style {
        size: Size {
            width: Dimension::length(w),
            height: Dimension::length(h),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}
fn scene() -> (Ui, UiId) {
    let mut ui = Ui::default();
    let root = ui.add(UiWidget::Panel, ponko::ui::UiLayout::custom(sized(300.0, 200.0)));
    ui.push_screen(root, false);
    (ui, root)
}
#[test]
fn row_column_padding_and_gap_have_exact_positions() {
    let (mut ui, root) = scene();
    let mut style = sized(300.0, 200.0);
    style.flex_direction = FlexDirection::Row;
    style.padding = Edges::length(10.0);
    style.gap = Size::length(8.0);
    ui.set_layout(root, ponko::ui::UiLayout::custom(style));
    let a = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(40.0, 30.0)));
    let b = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(50.0, 30.0)));
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(ui.rect(a).unwrap().min, vec2(10.0, 10.0));
    assert_eq!(ui.rect(b).unwrap().min, vec2(58.0, 10.0));
    let mut style = sized(300.0, 200.0);
    style.flex_direction = FlexDirection::Column;
    ui.set_layout(root, ponko::ui::UiLayout::custom(style));
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(ui.rect(b).unwrap().min.y, 30.0);
}
#[test]
fn grid_places_inventory_cells_and_scroll_clamps() {
    let (mut ui, root) = scene();
    let scroll = ui.add_child(root, UiWidget::Scroll, ponko::ui::UiLayout::custom(sized(150.0, 80.0)));
    let mut grid = sized(140.0, 200.0);
    grid.display = Display::Grid;
    grid.grid_template_columns = vec![length(60.0), length(60.0)];
    grid.gap = Size {
        width: LengthPercentage::length(4.0),
        height: LengthPercentage::length(4.0),
    };
    let grid = ui.add_child(scroll, UiWidget::Panel, ponko::ui::UiLayout::custom(grid));
    let a = ui.add_child(grid, UiWidget::Button, ponko::ui::UiLayout::custom(sized(60.0, 40.0)));
    let b = ui.add_child(grid, UiWidget::Button, ponko::ui::UiLayout::custom(sized(60.0, 40.0)));
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(ui.rect(b).unwrap().min.x - ui.rect(a).unwrap().min.x, 64.0);
    ui.set_scroll(scroll, vec2(0.0, 999.0));
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(ui.scroll(scroll), Some(vec2(0.0, 120.0)));
    assert_ne!(ui.hit_test(vec2(10.0, 100.0)), Some(a));
}
#[test]
fn press_release_events_and_disabled_focus() {
    let (mut ui, root) = scene();
    let a = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(70.0, 30.0)));
    let b = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(70.0, 30.0)));
    ui.set_enabled(a, false);
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(b));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            confirm: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.events().collect::<Vec<_>>(), vec![UiEvent::Clicked(b)]);
    assert_eq!(ui.events().count(), 0);
    let p = ui.rect(b).unwrap().center();
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: p,
            pressed: true,
            held: true,
            ..Default::default()
        },
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: p,
            released: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.events().collect::<Vec<_>>(), vec![UiEvent::Clicked(b)]);
}
#[test]
fn only_top_screen_receives_input_and_cancel_restores_focus() {
    let (mut ui, root) = scene();
    let buy = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(70.0, 30.0)));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            ..Default::default()
        },
    );
    let dialog = ui.add(UiWidget::Panel, ponko::ui::UiLayout::custom(sized(300.0, 200.0)));
    let yes = ui.add_child(dialog, UiWidget::Button, ponko::ui::UiLayout::custom(sized(70.0, 30.0)));
    ui.push_screen(dialog, true);
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            confirm: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(yes));
    assert_eq!(ui.events().collect::<Vec<_>>(), vec![UiEvent::Clicked(yes)]);
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            cancel: true,
            confirm: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(buy));
    assert_eq!(ui.events().collect::<Vec<_>>(), vec![UiEvent::ScreenPopped(dialog)]);
}
#[test]
fn unicode_text_selection_editing_and_submission() {
    let (mut ui, root) = scene();
    let field = ui.add_child(
        root,
        UiWidget::TextInput,
        ponko::ui::UiLayout::custom(sized(160.0, 30.0)),
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            text: "é猫".into(),
            ..Default::default()
        },
    );
    assert_eq!(ui.text(field), Some("é猫"));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            backspace: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.text(field), Some("é"));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            select_all: true,
            text: "ok\n".into(),
            confirm: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.text(field), Some("ok"));
    // Taking one event leaves the rest queued, in order.
    let first = ui.next_event();
    let rest: Vec<_> = ui.events().collect();
    assert_eq!(first, Some(UiEvent::Changed(field)));
    assert_eq!(rest, [UiEvent::Submitted(field)]);
    assert!(ui.wants_keyboard());
}

/// A focused text field that typed and confirmed in one frame: `Changed`
/// then `Submitted` are queued.
fn two_queued_events() -> (Ui, UiId) {
    let (mut ui, root) = scene();
    let field = ui.add_child(
        root,
        UiWidget::TextInput,
        ponko::ui::UiLayout::custom(sized(160.0, 30.0)),
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            text: "ok\n".into(),
            confirm: true,
            ..Default::default()
        },
    );
    (ui, field)
}

#[test]
fn dropping_a_half_read_events_iterator_keeps_the_rest_queued() {
    let (mut ui, field) = two_queued_events();
    let mut events = ui.events();
    assert_eq!(events.next(), Some(UiEvent::Changed(field)));
    drop(events);
    assert_eq!(ui.events().collect::<Vec<_>>(), [UiEvent::Submitted(field)]);
    assert_eq!(ui.events().next(), None);
}

#[test]
fn untaken_events_are_gone_after_the_next_frame() {
    let (mut ui, field) = two_queued_events();
    assert_eq!(ui.events().next(), Some(UiEvent::Changed(field)));
    ui.frame(vec2(800.0, 600.0), &UiInput::default());
    assert_eq!(ui.events().count(), 0, "Submitted was never taken");
    assert_eq!(ui.next_event(), None);
}

#[test]
fn pointer_capture_survives_drag_and_scrollbar_matches_scroll_range() {
    let (mut ui, root) = scene();
    let scroll = ui.add_child(root, UiWidget::Scroll, ponko::ui::UiLayout::custom(sized(150.0, 80.0)));
    ui.add_child(
        scroll,
        UiWidget::Panel,
        ponko::ui::UiLayout::custom(sized(140.0, 300.0)),
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: vec2(145.0, 10.0),
            pressed: true,
            held: true,
            ..Default::default()
        },
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: vec2(500.0, 500.0),
            held: true,
            ..Default::default()
        },
    );
    assert!(ui.wants_pointer());
    assert_eq!(ui.scroll(scroll).unwrap().y, 220.0);
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: vec2(500.0, 500.0),
            released: true,
            ..Default::default()
        },
    );
    assert!(!ui.wants_pointer());
}
#[test]
fn navigation_reveals_clipped_buttons_and_skips_disabled_subtrees() {
    let (mut ui, root) = scene();
    let scroll = ui.add_child(root, UiWidget::Scroll, ponko::ui::UiLayout::custom(sized(100.0, 50.0)));
    let mut column = sized(100.0, 150.0);
    column.flex_direction = FlexDirection::Column;
    let column = ui.add_child(scroll, UiWidget::Panel, ponko::ui::UiLayout::custom(column));
    let a = ui.add_child(
        column,
        UiWidget::Button,
        ponko::ui::UiLayout::custom(sized(100.0, 50.0)),
    );
    let b = ui.add_child(
        column,
        UiWidget::Button,
        ponko::ui::UiLayout::custom(sized(100.0, 50.0)),
    );
    let c = ui.add_child(
        column,
        UiWidget::Button,
        ponko::ui::UiLayout::custom(sized(100.0, 50.0)),
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(a));
    ui.set_enabled(b, false);
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            direction: vec2(0.0, 1.0),
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(c));
    assert_eq!(ui.scroll(scroll).unwrap().y, 100.0);
    assert_eq!(ui.hit_test(vec2(30.0, 20.0)), Some(c));
}
#[test]
fn stale_ids_scaling_and_absolute_constraints() {
    let (mut ui, root) = scene();
    let mut style = sized(20.0, 30.0);
    style.position = ponko::ui::unstable_taffy::Position::Absolute;
    style.inset.left = ponko::ui::unstable_taffy::LengthPercentageAuto::length(12.0);
    style.inset.top = ponko::ui::unstable_taffy::LengthPercentageAuto::length(7.0);
    style.min_size.width = ponko::ui::unstable_taffy::LengthPercentageAuto::length(40.0);
    let id = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(style));
    ui.set_scale(2.0);
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(ui.rect(id).unwrap().min, vec2(24.0, 14.0));
    assert_eq!(ui.rect(id).unwrap().size(), vec2(80.0, 60.0));
    ui.remove(id);
    let new = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(20.0, 20.0)));
    assert_ne!(id, new);
    assert!(ui.rect(id).is_none());
}
#[test]
fn toggle_slider_wheel_and_no_release_without_press() {
    let (mut ui, root) = scene();
    let toggle = ui.add_child(root, UiWidget::Toggle, ponko::ui::UiLayout::custom(sized(70.0, 30.0)));
    let slider = ui.add_child(root, UiWidget::Slider, ponko::ui::UiLayout::custom(sized(100.0, 30.0)));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            confirm: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.value(toggle), Some(1.0));
    assert_eq!(ui.events().collect::<Vec<_>>(), vec![UiEvent::Changed(toggle)]);
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            direction: vec2(1.0, 0.0),
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(slider));
    assert_eq!(ui.value(slider), Some(0.05));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: vec2(10.0, 10.0),
            released: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.events().count(), 0);
}

#[test]
fn native_text_storage_is_shared_and_cleared_with_frame_input() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    Game::new("input test", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            let frame = count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(ctx.input.typed_text(), if frame == 0 { "é猫" } else { "" });
        })
        .run_headless(2, |ctx| {
            ctx.input.insert_text("é猫");
        });
    assert_eq!(seen.load(Ordering::SeqCst), 2);
}

#[test]
fn focusing_text_by_pointer_does_not_submit_and_hidden_nodes_do_not_hit() {
    let (mut ui, root) = scene();
    let field = ui.add_child(
        root,
        UiWidget::TextInput,
        ponko::ui::UiLayout::custom(sized(120.0, 30.0)),
    );
    let hidden = ui.add_child(
        root,
        UiWidget::Button,
        ponko::ui::UiLayout::custom(sized(120.0, 30.0)).hidden(),
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: vec2(10.0, 10.0),
            pressed: true,
            held: true,
            ..Default::default()
        },
    );
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: vec2(10.0, 10.0),
            released: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(field));
    assert_eq!(ui.events().count(), 0);
    assert_ne!(ui.hit_test(Vec2::ZERO), Some(hidden));
}
#[test]
fn nested_clips_and_wheel_offsets_do_not_leak_hits() {
    let (mut ui, root) = scene();
    let outer = ui.add_child(root, UiWidget::Scroll, ponko::ui::UiLayout::custom(sized(100.0, 60.0)));
    let mut column = sized(100.0, 200.0);
    column.flex_direction = FlexDirection::Column;
    let column = ui.add_child(outer, UiWidget::Panel, ponko::ui::UiLayout::custom(column));
    let first = ui.add_child(column, UiWidget::Button, ponko::ui::UiLayout::custom(sized(80.0, 40.0)));
    let second = ui.add_child(column, UiWidget::Button, ponko::ui::UiLayout::custom(sized(80.0, 40.0)));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            pointer: vec2(20.0, 20.0),
            wheel: vec2(0.0, 40.0),
            ..Default::default()
        },
    );
    assert_eq!(ui.scroll(outer), Some(vec2(0.0, 40.0)));
    assert_eq!(ui.hit_test(vec2(20.0, 20.0)), Some(second));
    assert_ne!(ui.hit_test(vec2(20.0, 80.0)), Some(first));
}

#[test]
fn last_screen_cancel_owns_input_until_next_frame_and_text_allows_vertical_navigation() {
    let (mut ui, root) = scene();
    let mut style = sized(300.0, 200.0);
    style.flex_direction = FlexDirection::Column;
    ui.set_layout(root, ponko::ui::UiLayout::custom(style));
    let field = ui.add_child(
        root,
        UiWidget::TextInput,
        ponko::ui::UiLayout::custom(sized(100.0, 30.0)),
    );
    let button = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(sized(100.0, 30.0)));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(field));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            direction: vec2(0.0, 1.0),
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), Some(button));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            cancel: true,
            ..Default::default()
        },
    );
    assert!(ui.wants_keyboard());
    assert!(ui.wants_pointer());
    ui.frame(vec2(800.0, 600.0), &UiInput::default());
    assert!(!ui.wants_keyboard());
    assert!(!ui.wants_pointer());
}

// Builders, layout presets and the theme.

#[test]
fn builders_match_add_child_and_setters() {
    let mut ui = Ui::default();
    let root = ui.root().layout(UiLayout::fill()).id();
    let built = ui
        .label(root, "LOBBY")
        .layout(UiLayout::row(20.0))
        .size(13.0)
        .color(Color::YELLOW)
        .id();
    let manual = ui.add_child(root, UiWidget::Label, UiLayout::row(20.0));
    ui.set_text(manual, "LOBBY");
    let mut paint = ui.paint(manual).unwrap();
    paint.font_size = 13.0;
    paint.color = Color::YELLOW;
    ui.set_paint(manual, paint);
    assert_eq!(ui.parent(built), Some(root));
    assert_eq!(ui.text(built), ui.text(manual));
    assert_eq!(ui.node_layout(built), ui.node_layout(manual));
    let (a, b) = (ui.paint(built).unwrap(), ui.paint(manual).unwrap());
    assert_eq!(
        (a.font_size, a.color, a.background),
        (b.font_size, b.color, b.background)
    );

    let buy = ui.button(root, "Buy").enabled(false).id();
    let gift = ui.toggle(root, "Gift").value(1.0).id();
    let volume = ui.slider(root).value(0.25).id();
    let note = ui.text_input(root).text("hi\n").id();
    let list = ui.scroll_area(root).id();
    ui.push_screen(root, false);
    ui.layout(vec2(400.0, 300.0));
    assert_ne!(ui.hit_test(ui.rect(buy).unwrap().center()), Some(buy), "disabled");
    assert_eq!(ui.value(gift), Some(1.0));
    assert_eq!(ui.value(volume), Some(0.25));
    assert_eq!(ui.text(note), Some("hi"), "control characters dropped");
    assert!(ui.rect(list).is_some());
}

#[test]
fn layout_presets_place_rows_columns_anchors_and_dialogs() {
    let mut ui = Ui::default();
    let root = ui
        .root()
        .layout(UiLayout::fill().horizontal().padding(10.0).gap(5.0))
        .id();
    let side = ui.panel(root).layout(UiLayout::column(100.0)).id();
    let main = ui
        .panel(root)
        .layout(UiLayout::auto().grow(1.0).vertical().gap(4.0))
        .id();
    let header = ui.label(main, "Title").layout(UiLayout::row(30.0)).id();
    let body = ui.scroll_area(main).layout(UiLayout::fill()).id();
    let footer = ui.panel(main).layout(UiLayout::row(20.0)).id();
    let hud = ui
        .label(root, "100 HP")
        .layout(UiLayout::anchored(0.5, 0.0, 0.5, 0.1))
        .id();
    ui.push_screen(root, false);
    let dialog = ui.root().layout(UiLayout::centered(200.0, 100.0)).id();
    ui.push_screen(dialog, true);
    ui.layout(vec2(800.0, 600.0));

    let rect = |id| {
        let r = ui.rect(id).unwrap();
        (r.min, r.size())
    };
    assert_eq!(rect(side), (vec2(10.0, 10.0), vec2(100.0, 580.0)));
    assert_eq!(rect(main), (vec2(115.0, 10.0), vec2(675.0, 580.0)));
    assert_eq!(rect(header), (vec2(115.0, 10.0), vec2(675.0, 30.0)));
    // The fill shrinks to what the fixed rows leave: 580 - 30 - 20 - 2 gaps.
    assert_eq!(rect(body), (vec2(115.0, 44.0), vec2(675.0, 522.0)));
    assert_eq!(rect(footer), (vec2(115.0, 570.0), vec2(675.0, 20.0)));
    // Anchors are fractions of the parent box, out of the sibling flow.
    assert_eq!(rect(hud), (vec2(400.0, 0.0), vec2(400.0, 60.0)));
    assert_eq!(rect(dialog), (vec2(300.0, 250.0), vec2(200.0, 100.0)));

    // A different window moves anchored and centered nodes with it.
    ui.layout(vec2(1000.0, 400.0));
    assert_eq!(ui.rect(hud).unwrap().min, vec2(500.0, 0.0));
    assert_eq!(ui.rect(dialog).unwrap().min, vec2(400.0, 150.0));
}

#[test]
fn grid_preset_wraps_cells_into_equal_columns() {
    let mut ui = Ui::default();
    let root = ui.root().layout(UiLayout::fixed(320.0, 400.0)).id();
    let grid = ui.panel(root).layout(UiLayout::grid(4, 40.0).gap(0.0)).id();
    let cells: Vec<UiId> = (0..6).map(|i| ui.button(grid, format!("{i}")).id()).collect();
    ui.push_screen(root, false);
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(ui.rect(cells[3]).unwrap().min, vec2(240.0, 0.0));
    assert_eq!(ui.rect(cells[4]).unwrap().min, vec2(0.0, 40.0));
    assert_eq!(ui.rect(cells[5]).unwrap().size(), vec2(80.0, 40.0));
    assert_eq!(ui.rect(grid).unwrap().size(), vec2(320.0, 80.0));
}

#[test]
fn default_theme_keeps_the_old_paint_defaults() {
    let mut ui = Ui::default();
    let root = ui.add(UiWidget::Panel, ponko::ui::UiLayout::auto());
    let button = ui.add_child(root, UiWidget::Button, ponko::ui::UiLayout::auto());
    let label = ui.add_child(root, UiWidget::Label, ponko::ui::UiLayout::auto());
    let old = UiPaint::default();
    for id in [root, button] {
        let p = ui.paint(id).unwrap();
        assert_eq!(
            (p.background, p.hover, p.border, p.color, p.font_size),
            (old.background, old.hover, old.border, old.color, old.font_size)
        );
    }
    assert_eq!(ui.paint(label).unwrap().background.a, 0.0);
}

#[test]
fn theme_paints_new_nodes_and_overrides_survive_a_retheme() {
    let dark = UiTheme {
        panel: Color::BLACK,
        control: Color::rgb(0.2, 0.2, 0.2),
        accent: Color::GREEN,
        text: Color::WHITE,
        text_size: 14.0,
        border_width: 1.0,
        ..Default::default()
    };
    let mut ui = Ui::default();
    ui.set_theme(dark);
    let root = ui.root().id();
    let ok = ui.button(root, "OK").id();
    let title = ui.label(root, "Title").size(28.0).id();
    let p = ui.paint(ok).unwrap();
    assert_eq!(
        (p.background, p.border, p.border_width, p.font_size),
        (dark.control, Color::GREEN, 1.0, 14.0)
    );
    assert_eq!(ui.paint(root).unwrap().background, Color::BLACK);
    assert_eq!(ui.paint(root).unwrap().border_width, 0.0, "panels have no border");

    let light = UiTheme {
        panel: Color::WHITE,
        text: Color::BLACK,
        text_size: 16.0,
        ..dark
    };
    ui.set_theme(light);
    assert_eq!(ui.paint(root).unwrap().background, Color::WHITE);
    assert_eq!(ui.paint(ok).unwrap().color, Color::BLACK);
    let t = ui.paint(title).unwrap();
    assert_eq!(
        (t.font_size, t.color),
        (28.0, Color::BLACK),
        "own size kept, color follows"
    );
    assert_eq!(ui.theme(), &light);
}

#[test]
fn a_title_takes_the_themes_title_paint_and_follows_a_retheme() {
    let mut ui = Ui::default();
    let root = ui.root().id();
    let plain = ui.title(root, "LOBBY").id();
    let sized = ui.title(root, "Players").size(40.0).id();
    let label = ui.label(root, "4 online").id();
    let theme = *ui.theme();
    let p = ui.paint(plain).unwrap();
    assert_eq!((p.color, p.font_size), (theme.title, theme.title_size));
    assert_eq!(p.background.a, 0.0, "a title is a label: no background");
    assert!(theme.title_size > theme.text_size);

    ui.set_theme(UiTheme {
        title: Color::YELLOW,
        title_size: 32.0,
        text: Color::GREEN,
        ..theme
    });
    let p = ui.paint(plain).unwrap();
    assert_eq!((p.color, p.font_size), (Color::YELLOW, 32.0));
    let p = ui.paint(sized).unwrap();
    assert_eq!((p.color, p.font_size), (Color::YELLOW, 40.0), "own size kept");
    let p = ui.paint(label).unwrap();
    assert_eq!((p.color, p.font_size), (Color::GREEN, theme.text_size));
}

#[test]
fn one_theme_styles_the_tree_and_the_immediate_widgets() {
    // Through the `ui` service, as a game sets it.
    Game::new("theme", 320, 240).run_headless(0, |ctx| {
        assert_eq!(ctx.ui.immediate.style, UiStyle::default());
        assert_eq!(UiTheme::default().style(), UiStyle::default());
        ctx.ui.immediate.style.padding = 3.0;
        ctx.ui.immediate.style.text_size = 21.0; // set by hand
        let theme = UiTheme {
            panel: Color::BLACK,
            control: Color::BLUE,
            hover: Color::GREEN,
            pressed: Color::RED,
            accent: Color::YELLOW,
            text: Color::rgb(0.5, 0.5, 0.5),
            text_size: 14.0,
            ..Default::default()
        };
        ctx.ui.set_theme(theme);
        let style = ctx.ui.immediate.style;
        assert_eq!(
            (style.panel, style.button, style.button_hovered, style.button_pressed),
            (Color::BLACK, Color::BLUE, Color::GREEN, Color::RED)
        );
        assert_eq!((style.accent, style.text), (Color::YELLOW, theme.text));
        assert_eq!((style.text_size, style.padding), (21.0, 3.0), "fields set by hand stay");
        // The tree's nodes are painted from the same theme.
        let root = ctx.ui.root().id();
        let ok = ctx.ui.button(root, "OK").id();
        assert_eq!(ctx.ui.paint(ok).unwrap().background, Color::BLUE);
        assert_eq!(ctx.ui.paint(root).unwrap().background, Color::BLACK);

        // Back to the default theme: the immediate widgets get their own
        // defaults back (see-through panels), not the tree's opaque ones.
        ctx.ui.set_theme(UiTheme::default());
        let style = ctx.ui.immediate.style;
        let plain = UiStyle::default();
        assert_eq!(
            (style.panel, style.button, style.accent),
            (plain.panel, plain.button, plain.accent)
        );
        assert_eq!(style.text_size, 21.0);
    });
}

#[test]
fn a_name_finds_its_node_moves_to_the_last_one_named_and_dies_with_it() {
    let mut ui = Ui::default();
    let root = ui.root().name("chat").id();
    let history = ui.scroll_area(root).name("chat.history").id();
    let line = ui.label(history, "hello").id();
    let input = ui.text_input(root).name("chat.input").id();
    assert_eq!(ui.find("chat"), Some(root));
    assert_eq!(ui.find("chat.history"), Some(history));
    assert_eq!(ui.find("chat.input"), Some(input));
    assert_eq!(ui.find("chat.send"), None);
    assert_eq!((ui.name(history), ui.name(line)), (Some("chat.history"), None));

    // Renaming frees the old name; the same name twice moves it.
    ui.set_name(input, "chat.box");
    assert_eq!((ui.find("chat.input"), ui.find("chat.box")), (None, Some(input)));
    ui.set_name(line, "chat.box");
    assert_eq!((ui.find("chat.box"), ui.name(input)), (Some(line), None));
    // Naming a node what it is already named changes nothing.
    ui.set_name(line, "chat.box");
    assert_eq!((ui.find("chat.box"), ui.name(line)), (Some(line), Some("chat.box")));

    // Removing a subtree frees its names; the ids stay dead.
    ui.remove(history);
    assert_eq!((ui.find("chat.history"), ui.find("chat.box")), (None, None));
    assert_eq!(ui.name(history), None);
    ui.set_name(history, "ghost");
    assert_eq!(ui.find("ghost"), None, "a removed node can't be named");
    // The name is free for a new node.
    let again = ui.scroll_area(root).name("chat.history").id();
    assert_eq!(ui.find("chat.history"), Some(again));
    // Each tree has its own names.
    assert_eq!(Ui::default().find("chat"), None);
}

#[test]
fn events_are_taken_once_and_untaken_ones_stay_queued() {
    let mut ui = Ui::default();
    let root = ui.root().layout(UiLayout::fill()).id();
    let ok = ui
        .button(root, "OK")
        .layout(UiLayout::fixed(100.0, 40.0))
        .name("ok")
        .id();
    let music = ui.toggle(root, "Music").layout(UiLayout::fixed(100.0, 40.0)).id();
    ui.push_screen(root, false);
    let viewport = vec2(400.0, 300.0);
    let click = |ui: &mut Ui, at: Vec2| {
        for (pressed, released) in [(true, false), (false, true)] {
            ui.frame(
                viewport,
                &UiInput {
                    pointer: at,
                    pressed,
                    held: pressed,
                    released,
                    ..Default::default()
                },
            );
        }
    };
    let last_click = |ui: &mut Ui| ui.events().filter_map(UiEvent::clicked).last();
    ui.frame(viewport, &UiInput::default());
    assert_eq!(last_click(&mut ui), None);
    click(&mut ui, vec2(50.0, 20.0));
    let clicked = last_click(&mut ui);
    assert_eq!(clicked, Some(ok));
    assert_eq!(clicked.and_then(|id| ui.name(id)), Some("ok"));
    assert_eq!(last_click(&mut ui), None, "taken once");
    // A toggle changes, it isn't clicked.
    click(&mut ui, vec2(50.0, 60.0));
    assert_eq!(ui.value(music), Some(1.0));
    assert_eq!(ui.next_event(), Some(UiEvent::Changed(music)));
    assert_eq!(ui.next_event(), None);
}

#[test]
fn top_and_left_place_a_preset_in_pixels() {
    let mut ui = Ui::default();
    let root = ui.root().layout(UiLayout::fill()).id();
    let map = ui
        .panel(root)
        .layout(UiLayout::anchored(0.5, 0.5, 0.25, 0.25).left(16.0).top(8.0))
        .id();
    let row = ui.panel(root).layout(UiLayout::row(20.0).top(30.0).left(10.0)).id();
    let dialog = ui.root().layout(UiLayout::centered(200.0, 100.0).top(40.0)).id();
    ui.push_screen(root, false);
    ui.push_screen(dialog, true);
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(ui.rect(map).unwrap().min, vec2(16.0, 8.0));
    assert_eq!(ui.rect(map).unwrap().size(), vec2(200.0, 150.0));
    assert_eq!(ui.rect(row).unwrap().min, vec2(10.0, 30.0), "in the flow: margins");
    assert_eq!(ui.rect(dialog).unwrap().min, vec2(300.0, 40.0), "still centered across");
}

#[test]
fn auto_margins_share_the_windows_free_space_around_a_screen_root() {
    use ponko::ui::unstable_taffy::prelude::{auto, length};
    let margins = |left, right, top, bottom| Style {
        margin: Edges {
            left,
            right,
            top,
            bottom,
        },
        ..sized(200.0, 100.0)
    };
    let mut ui = Ui::default();
    let centered = ui.add(
        UiWidget::Panel,
        ponko::ui::UiLayout::custom(margins(auto(), auto(), auto(), auto())),
    );
    let across = ui.add(
        UiWidget::Panel,
        ponko::ui::UiLayout::custom(margins(auto(), auto(), length(40.0), auto())),
    );
    let right = ui.add(
        UiWidget::Panel,
        ponko::ui::UiLayout::custom(margins(auto(), length(10.0), length(0.0), length(0.0))),
    );
    let plain = ui.add(
        UiWidget::Panel,
        ponko::ui::UiLayout::custom(margins(length(5.0), length(0.0), length(7.0), length(0.0))),
    );
    let child = ui.add_child(plain, UiWidget::Button, ponko::ui::UiLayout::custom(sized(20.0, 20.0)));
    for root in [centered, across, right, plain] {
        ui.push_screen(root, false);
    }
    ui.layout(vec2(800.0, 600.0));
    let at = |ui: &Ui, id| ui.rect(id).unwrap().min;
    assert_eq!(at(&ui, centered), vec2(300.0, 250.0));
    assert_eq!(at(&ui, across), vec2(300.0, 40.0));
    assert_eq!(at(&ui, right), vec2(590.0, 0.0));
    assert_eq!(at(&ui, plain), vec2(5.0, 7.0));
    assert_eq!(at(&ui, child), vec2(5.0, 7.0), "children move with their root");
    assert_eq!(ui.rect(centered).unwrap().size(), vec2(200.0, 100.0));
    // A click lands where the root was moved to.
    assert_eq!(ui.hit_test(vec2(10.0, 10.0)), Some(child));

    // In a smaller window there is no free space to share.
    ui.layout(vec2(150.0, 80.0));
    assert_eq!(at(&ui, centered), vec2(0.0, 0.0));
    // The UI scale counts: 400 x 300 logical units here.
    ui.set_scale(2.0);
    ui.layout(vec2(800.0, 600.0));
    assert_eq!(at(&ui, centered), vec2(200.0, 200.0));
    assert_eq!(ui.rect(centered).unwrap().size(), vec2(400.0, 200.0));
}

#[test]
fn the_ui_examples_are_at_most_half_as_long_as_before_the_builders() {
    // 156 and 72 lines before the builders, layout presets and names.
    for (source, before) in [
        (include_str!("../examples/ui_shop.rs"), 156),
        (include_str!("../examples/ui_panel.rs"), 72),
    ] {
        let lines = source.lines().count();
        assert!(lines * 2 <= before, "{lines} lines, {before} before");
        // No layout helper of its own, no raw styles, no id kept in a resource.
        for helper in [
            "ui_layout::",
            "Style {",
            "fn fixed",
            "fn row",
            "insert_resource",
            "add_child",
        ] {
            assert!(!source.contains(helper), "the example still has `{helper}`");
        }
    }
}

/// A menu: Play and Quit in a column, filling an 800 x 600 window.
fn menu(ui: &mut Ui) -> (UiId, UiId, UiId) {
    let root = ui.root().layout(UiLayout::fill()).id();
    let play = ui.button(root, "Play").layout(UiLayout::row(40.0)).id();
    let quit = ui.button(root, "Quit").layout(UiLayout::row(40.0)).id();
    (root, play, quit)
}

fn confirm() -> UiInput {
    UiInput {
        confirm: true,
        ..Default::default()
    }
}

#[test]
fn set_focus_then_enter_clicks_that_button() {
    let mut ui = Ui::default();
    let (root, _, quit) = menu(&mut ui);
    ui.push_screen(root, false);
    assert_eq!(ui.focus(), None, "nothing is focused without asking");
    assert!(ui.set_focus(quit));
    assert_eq!(ui.focus(), Some(quit));
    ui.frame(vec2(800.0, 600.0), &confirm());
    assert_eq!(ui.events().filter_map(UiEvent::clicked).last(), Some(quit));
}

#[test]
fn set_focus_refuses_what_cant_be_focused() {
    let mut ui = Ui::default();
    let (root, play, quit) = menu(&mut ui);
    let label = ui.label(root, "Hello").id();
    assert!(!ui.set_focus(play), "not on a screen yet");
    ui.push_screen(root, false);
    assert!(ui.set_focus(play));
    assert!(!ui.set_focus(label), "a label");
    ui.set_enabled(quit, false);
    assert!(!ui.set_focus(quit), "disabled");
    ui.set_layout(quit, UiLayout::row(40.0).hidden());
    ui.set_enabled(quit, true);
    assert!(!ui.set_focus(quit), "hidden");
    assert_eq!(ui.focus(), Some(play), "a refused call leaves the focus alone");

    // Only the top screen takes the focus.
    let dialog = ui.root().id();
    let ok = ui.button(dialog, "OK").id();
    ui.push_screen(dialog, true);
    assert!(!ui.set_focus(play), "under another screen");
    assert!(ui.set_focus(ok));
    ui.clear_focus();
    assert_eq!(ui.focus(), None);
}

#[test]
fn a_screen_opens_on_its_focused_node_every_time_it_is_pushed() {
    let mut ui = Ui::default();
    let root = ui.root().layout(UiLayout::fill()).id();
    ui.button(root, "Settings").layout(UiLayout::row(40.0));
    let play = ui.button(root, "Play").layout(UiLayout::row(40.0)).focused().id();
    ui.push_screen(root, false);
    assert_eq!(ui.focus(), Some(play), "before the first layout");
    ui.frame(vec2(800.0, 600.0), &confirm());
    assert_eq!(ui.events().filter_map(UiEvent::clicked).last(), Some(play));

    // Tab moves on; closing and opening the menu again starts on Play again.
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            ..Default::default()
        },
    );
    assert_ne!(ui.focus(), Some(play));
    ui.pop_screen();
    ui.push_screen(root, false);
    assert_eq!(ui.focus(), Some(play));

    // A screen above it, popped, gives back the focus the menu had.
    let dialog = ui.root().id();
    let ok = ui.button(dialog, "OK").focused().id();
    ui.push_screen(dialog, true);
    assert_eq!(ui.focus(), Some(ok));
    ui.pop_screen();
    assert_eq!(ui.focus(), Some(play));

    // A disabled focus mark is skipped; unmarked, nothing is focused.
    ui.pop_screen();
    ui.set_enabled(play, false);
    ui.push_screen(root, false);
    assert_eq!(ui.focus(), None);
    ui.pop_screen();
    ui.set_enabled(play, true);
    ui.set_initial_focus(play, false);
    ui.push_screen(root, false);
    assert_eq!(ui.focus(), None);
}

#[test]
fn a_hud_stays_up_when_escape_closes_the_screen_above_it() {
    let mut ui = Ui::default();
    let hud = ui.root().layout(UiLayout::fill().padding(16.0)).id();
    let count = ui.label(hud, "0 / 5").layout(UiLayout::fixed(200.0, 30.0)).id();
    ui.show_hud(hud);
    assert!(ui.is_hud(hud) && !ui.is_screen(hud));
    assert!(!ui.wants_keyboard(), "the game keeps its keys");
    let escape = UiInput {
        cancel: true,
        ..Default::default()
    };
    ui.frame(vec2(800.0, 600.0), &escape);
    assert!(ui.is_shown(count), "Escape with no screen open");
    assert_eq!(ui.events().count(), 0, "nothing popped");

    let (menu, play, _) = menu(&mut ui);
    ui.push_screen(menu, true);
    ui.frame(vec2(800.0, 600.0), &UiInput::default());
    assert!(ui.is_shown(count) && ui.is_shown(play));
    ui.frame(vec2(800.0, 600.0), &escape);
    assert_eq!(ui.events().collect::<Vec<_>>(), vec![UiEvent::ScreenPopped(menu)]);
    assert!(ui.is_shown(count), "the HUD is still up");
    assert_eq!(ui.top_screen(), None);

    ui.hide_hud(hud);
    assert!(!ui.is_shown(count));
    ui.show_hud(hud);
    ui.show_hud(hud);
    assert!(ui.is_shown(count), "showing twice is showing once");
    ui.remove(hud);
    assert!(!ui.is_hud(hud), "removed with its nodes");
}

#[test]
fn a_hud_takes_no_pointer_and_no_focus() {
    let mut ui = Ui::default();
    let hud = ui.root().layout(UiLayout::fill()).id();
    let pause = ui.button(hud, "Pause").layout(UiLayout::fixed(100.0, 40.0)).id();
    ui.show_hud(hud);
    let at = |pressed, released| UiInput {
        pointer: vec2(50.0, 20.0),
        pressed,
        held: pressed,
        released,
        ..Default::default()
    };
    ui.frame(vec2(800.0, 600.0), &at(true, false));
    assert!(!ui.wants_pointer(), "clicks go through to the world");
    ui.frame(vec2(800.0, 600.0), &at(false, true));
    assert_eq!(ui.events().count(), 0);
    assert_eq!(ui.hit_test(vec2(50.0, 20.0)), None);
    assert!(!ui.set_focus(pause));
    ui.frame(
        vec2(800.0, 600.0),
        &UiInput {
            next: true,
            ..Default::default()
        },
    );
    assert_eq!(ui.focus(), None, "Tab skips it");

    // Drawn below the screens: a screen's button over it takes the click.
    let (menu, play, _) = menu(&mut ui);
    ui.push_screen(menu, false);
    ui.frame(vec2(800.0, 600.0), &at(true, false));
    ui.frame(vec2(800.0, 600.0), &at(false, true));
    assert_eq!(ui.events().filter_map(UiEvent::clicked).last(), Some(play));
}

#[test]
#[should_panic(expected = "UI HUD is already a screen")]
fn a_screen_cant_also_be_a_hud() {
    let mut ui = Ui::default();
    let root = ui.root().id();
    ui.push_screen(root, false);
    ui.show_hud(root);
}

#[test]
#[should_panic(expected = "UI screen is shown as a HUD")]
fn a_hud_cant_also_be_a_screen() {
    let mut ui = Ui::default();
    let root = ui.root().id();
    ui.show_hud(root);
    ui.push_screen(root, false);
}
