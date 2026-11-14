//! Accessibility semantics of the retained UI: roles, names, values, states
//! and focus, read as a deterministic snapshot without a platform adapter.

#[allow(unused_imports)]
use ponko::input::Gamepad;
use ponko::prelude::*;
#[allow(unused_imports)]
use ponko::ui::{Ui, UiAccessValue, UiId, UiInput, UiLayout, UiRole, UiWidget};

const VIEW: Vec2 = Vec2::new(800.0, 600.0);

fn press(ui: &mut Ui, input: UiInput) {
    ui.frame(VIEW, &input);
}
fn next() -> UiInput {
    UiInput {
        next: true,
        ..Default::default()
    }
}

/// A settings menu with every widget the semantic tree covers.
struct Menu {
    ui: Ui,
    root: UiId,
    title: UiId,
    play: UiId,
    volume: UiId,
    name: UiId,
    music: UiId,
    quit: UiId,
}
fn menu() -> Menu {
    let mut ui = Ui::default();
    let root = ui.root().layout(UiLayout::fill().padding(10.0).gap(4.0)).id();
    let title = ui.label(root, "Settings").layout(UiLayout::row(24.0)).id();
    let play = ui.button(root, "Play").layout(UiLayout::row(30.0)).id();
    let volume = ui
        .slider(root)
        .layout(UiLayout::row(20.0))
        .value(0.5)
        .accessible_name("Volume")
        .id();
    let name = ui
        .text_input(root)
        .layout(UiLayout::row(30.0))
        .text("Ann")
        .accessible_name("Player name")
        .id();
    let music = ui.toggle(root, "Music").layout(UiLayout::row(30.0)).value(1.0).id();
    let quit = ui.button(root, "Quit").layout(UiLayout::row(30.0)).enabled(false).id();
    ui.push_screen(root, false);
    ui.layout(VIEW);
    Menu {
        ui,
        root,
        title,
        play,
        volume,
        name,
        music,
        quit,
    }
}

#[test]
fn widgets_map_to_roles_names_and_values() {
    let m = menu();
    let tree = m.ui.accessibility_tree();
    let ids: Vec<UiId> = tree.nodes().iter().map(|n| n.id).collect();
    assert_eq!(ids, vec![m.root, m.title, m.play, m.volume, m.name, m.music, m.quit]);

    let root = tree.get(m.root).unwrap();
    assert_eq!(root.role, UiRole::Group);
    assert_eq!(root.parent, None);
    assert_eq!(root.depth, 0);
    assert_eq!(root.children, vec![m.title, m.play, m.volume, m.name, m.music, m.quit]);

    let title = tree.get(m.title).unwrap();
    assert_eq!((title.role, title.name.as_str()), (UiRole::Label, "Settings"));
    assert!(!title.focusable);
    assert_eq!(title.parent, Some(m.root));
    assert_eq!(title.depth, 1);

    let play = tree.get(m.play).unwrap();
    assert_eq!((play.role, play.name.as_str()), (UiRole::Button, "Play"));
    assert_eq!(play.value, UiAccessValue::None);
    assert!(play.focusable && !play.disabled && !play.focused);

    let volume = tree.get(m.volume).unwrap();
    assert_eq!((volume.role, volume.name.as_str()), (UiRole::Slider, "Volume"));
    assert_eq!(volume.value, UiAccessValue::Range(0.5));

    let name = tree.get(m.name).unwrap();
    assert_eq!((name.role, name.name.as_str()), (UiRole::TextInput, "Player name"));
    assert_eq!(name.value, UiAccessValue::Text("Ann".into()));

    let music = tree.get(m.music).unwrap();
    assert_eq!((music.role, music.name.as_str()), (UiRole::CheckBox, "Music"));
    assert_eq!(music.value, UiAccessValue::Checked(true));

    let quit = tree.get(m.quit).unwrap();
    assert!(quit.disabled && !quit.focusable);
    // Bounds are the resolved layout rectangles.
    assert_eq!(play.rect, m.ui.rect(m.play).unwrap());
}

#[test]
fn snapshot_text_is_deterministic_and_readable() {
    let mut m = menu();
    press(&mut m.ui, next());
    let text = m.ui.accessibility_tree().to_string();
    assert_eq!(
        text,
        "\
group
  label \"Settings\"
  button \"Play\" focusable focused
  slider \"Volume\" value=0.50 focusable
  text-input \"Player name\" value=\"Ann\" focusable
  checkbox \"Music\" checked focusable
  button \"Quit\" disabled
"
    );
    assert_eq!(text, menu_with_focus_on_play().accessibility_tree().to_string());
}
fn menu_with_focus_on_play() -> Ui {
    let mut m = menu();
    press(&mut m.ui, next());
    m.ui
}

#[test]
fn values_and_focus_follow_keyboard_and_gamepad_input() {
    let mut m = menu();
    assert_eq!(m.ui.accessibility_tree().focus(), None);
    // Tab twice: Play, then Volume.
    press(&mut m.ui, next());
    press(&mut m.ui, next());
    let tree = m.ui.accessibility_tree();
    assert_eq!(tree.focus(), Some(m.volume));
    assert!(tree.get(m.volume).unwrap().focused);
    assert!(!tree.get(m.play).unwrap().focused);
    // Gamepad/arrow right moves the slider one step.
    press(
        &mut m.ui,
        UiInput {
            direction: vec2(1.0, 0.0),
            ..Default::default()
        },
    );
    assert_eq!(
        m.ui.accessibility_tree().get(m.volume).unwrap().value,
        UiAccessValue::Range(0.55)
    );
    // Down moves focus to the text field; typing changes its value.
    press(
        &mut m.ui,
        UiInput {
            direction: vec2(0.0, 1.0),
            ..Default::default()
        },
    );
    assert_eq!(m.ui.accessibility_tree().focus(), Some(m.name));
    press(
        &mut m.ui,
        UiInput {
            text: "a".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        m.ui.accessibility_tree().get(m.name).unwrap().value,
        UiAccessValue::Text("Anna".into())
    );
    // Tab to the toggle and confirm unchecks it.
    press(&mut m.ui, next());
    press(
        &mut m.ui,
        UiInput {
            confirm: true,
            ..Default::default()
        },
    );
    let tree = m.ui.accessibility_tree();
    assert_eq!(tree.focus(), Some(m.music));
    assert_eq!(tree.get(m.music).unwrap().value, UiAccessValue::Checked(false));
}

#[test]
fn accessible_names_override_text_and_do_not_change_behavior() {
    let mut plain = menu();
    let mut named = menu();
    named.ui.set_accessible_name(named.play, "Start the game");
    named.ui.set_accessible_name(named.title, "Settings menu");
    assert_eq!(named.ui.accessible_name(named.play), Some("Start the game"));
    assert_eq!(plain.ui.accessible_name(plain.play), Some("Play"));
    // An empty name falls back to the text again.
    named.ui.set_accessible_name(named.title, "");
    assert_eq!(named.ui.accessible_name(named.title), Some("Settings"));
    // A text input's text is its value, never its name.
    let mut ui = Ui::default();
    let root = ui.root().id();
    let field = ui.text_input(root).text("secret").id();
    assert_eq!(ui.accessible_name(field), Some(""));
    assert_eq!(ui.accessible_name(UiId::from_bits(99)), None);

    let inputs = [
        next(),
        next(),
        UiInput {
            direction: vec2(1.0, 0.0),
            ..Default::default()
        },
        next(),
        next(),
        UiInput {
            confirm: true,
            ..Default::default()
        },
        UiInput {
            pointer: plain.ui.rect(plain.play).unwrap().center(),
            pressed: true,
            held: true,
            ..Default::default()
        },
        UiInput {
            pointer: plain.ui.rect(plain.play).unwrap().center(),
            released: true,
            ..Default::default()
        },
    ];
    for input in inputs {
        press(&mut plain.ui, input.clone());
        press(&mut named.ui, input);
        assert_eq!(plain.ui.focus(), named.ui.focus());
        assert_eq!(
            plain.ui.events().collect::<Vec<_>>(),
            named.ui.events().collect::<Vec<_>>()
        );
        assert_eq!(plain.ui.value(plain.volume), named.ui.value(named.volume));
        assert_eq!(plain.ui.value(plain.music), named.ui.value(named.music));
        assert_eq!(plain.ui.rect(plain.quit), named.ui.rect(named.quit));
    }
}

#[test]
fn hidden_subtrees_are_left_out_and_disabled_inherits() {
    let mut m = menu();
    let group = m.ui.panel(m.root).layout(UiLayout::row(40.0)).id();
    let inner = m.ui.button(group, "Inner").layout(UiLayout::fixed(60.0, 30.0)).id();
    m.ui.set_layout(m.play, UiLayout::row(30.0).hidden());
    m.ui.set_enabled(group, false);
    press(&mut m.ui, next());
    let tree = m.ui.accessibility_tree();
    assert!(tree.get(m.play).is_none(), "a hidden layout hides the node");
    assert!(!tree.get(m.root).unwrap().children.contains(&m.play));
    assert_eq!(tree.focus(), Some(m.volume), "focus skips the hidden button");
    let inner = tree.get(inner).unwrap();
    assert!(
        inner.disabled && !inner.focusable,
        "a disabled ancestor disables the subtree"
    );
    assert!(!tree.get(group).unwrap().focusable);

    // Hiding a parent hides its whole subtree.
    m.ui.set_layout(group, UiLayout::auto().hidden());
    m.ui.layout(VIEW);
    let tree = m.ui.accessibility_tree();
    assert!(tree.get(group).is_none() && tree.get(inner.id).is_none());

    // Showing and re-enabling brings the semantics back.
    m.ui.set_layout(m.play, UiLayout::row(30.0));
    m.ui.set_enabled(m.quit, true);
    m.ui.layout(VIEW);
    let tree = m.ui.accessibility_tree();
    assert!(tree.get(m.play).is_some());
    assert!(tree.get(m.quit).unwrap().focusable);
}

#[test]
fn modal_screens_hide_the_screens_below_and_restore_them_on_pop() {
    let mut m = menu();
    press(&mut m.ui, next());
    let dialog = m.ui.root().layout(UiLayout::centered(200.0, 100.0)).id();
    let question = m.ui.label(dialog, "Really quit?").layout(UiLayout::row(20.0)).id();
    let yes = m.ui.button(dialog, "Yes").layout(UiLayout::row(30.0)).id();
    m.ui.set_accessible_name(dialog, "Confirm");
    m.ui.push_screen(dialog, true);
    press(&mut m.ui, next());

    let tree = m.ui.accessibility_tree();
    let ids: Vec<UiId> = tree.nodes().iter().map(|n| n.id).collect();
    assert_eq!(ids, vec![dialog, question, yes], "only the modal dialog is exposed");
    let root = tree.get(dialog).unwrap();
    assert_eq!(
        (root.role, root.name.as_str(), root.modal),
        (UiRole::Dialog, "Confirm", true)
    );
    assert_eq!(tree.focus(), Some(yes));
    assert_eq!(tree.screens(), &[dialog]);

    // Cancel pops the dialog: the menu returns with its saved focus.
    press(
        &mut m.ui,
        UiInput {
            cancel: true,
            ..Default::default()
        },
    );
    let tree = m.ui.accessibility_tree();
    assert!(tree.get(dialog).is_none());
    assert_eq!(tree.focus(), Some(m.play));
    assert!(tree.get(m.play).unwrap().focused);
    assert_eq!(tree.screens(), &[m.root]);
}

#[test]
fn non_modal_overlays_keep_lower_screens_readable_but_inert() {
    let mut m = menu();
    let hud = m.ui.root().layout(UiLayout::anchored(0.0, 0.0, 100.0, 40.0)).id();
    let pause = m.ui.button(hud, "Pause").layout(UiLayout::fill()).id();
    m.ui.push_screen(hud, false);
    m.ui.layout(VIEW);
    let tree = m.ui.accessibility_tree();
    assert_eq!(tree.screens(), &[m.root, hud]);
    let play = tree.get(m.play).unwrap();
    assert!(play.inert && !play.focusable && !play.disabled);
    let pause = tree.get(pause).unwrap();
    assert!(!pause.inert && pause.focusable);
    assert_eq!(tree.get(hud).unwrap().role, UiRole::Group);
    // Bottom screen first, then the overlay, in tree order.
    assert_eq!(tree.nodes().last().unwrap().id, pause.id);
}

#[test]
fn huds_are_read_under_the_screens_inert_and_hidden_by_a_modal() {
    let mut m = menu();
    let hud = m.ui.root().layout(UiLayout::anchored(0.0, 0.0, 200.0, 40.0)).id();
    let count = m.ui.label(hud, "Lanterns lit: 2 / 5").layout(UiLayout::fill()).id();
    m.ui.show_hud(hud);
    m.ui.layout(VIEW);
    let tree = m.ui.accessibility_tree();
    assert_eq!(tree.screens(), &[hud, m.root], "the HUD first, under the menu");
    assert!(tree.get(count).unwrap().inert);
    assert_eq!(tree.get(count).unwrap().name, "Lanterns lit: 2 / 5");
    assert!(!tree.get(m.play).unwrap().inert, "the menu still takes input");

    let dialog = m.ui.root().layout(UiLayout::centered(200.0, 100.0)).id();
    m.ui.push_screen(dialog, true);
    m.ui.layout(VIEW);
    assert_eq!(m.ui.accessibility_tree().screens(), &[dialog]);
}

#[test]
fn removed_nodes_and_empty_uis_have_no_semantics() {
    let empty = Ui::default();
    let tree = empty.accessibility_tree();
    assert!(tree.nodes().is_empty() && tree.focus().is_none() && tree.screens().is_empty());
    assert_eq!(tree.to_string(), "");

    let mut m = menu();
    press(&mut m.ui, next());
    m.ui.remove(m.play);
    m.ui.layout(VIEW);
    let tree = m.ui.accessibility_tree();
    assert!(tree.get(m.play).is_none());
    assert_eq!(tree.focus(), None);
}

#[test]
fn scale_moves_bounds_but_not_semantics() {
    let mut m = menu();
    let before = m.ui.accessibility_tree();
    m.ui.set_scale(2.0);
    m.ui.layout(VIEW);
    let after = m.ui.accessibility_tree();
    assert_eq!(before.to_string(), after.to_string());
    let (a, b) = (before.get(m.play).unwrap().rect, after.get(m.play).unwrap().rect);
    assert_eq!(b.size().y, a.size().y * 2.0);
    assert_eq!(b.min, a.min * 2.0);
}

#[test]
fn images_scrolls_and_panels_have_roles() {
    let mut ui = Ui::default();
    let root = ui.root().layout(UiLayout::fill()).id();
    let list = ui.scroll_area(root).layout(UiLayout::fixed(100.0, 50.0)).id();
    // Images normally come from `ui.image(parent, texture)`; the role is the same.
    let icon = ui.add_child(list, UiWidget::Image, UiLayout::fixed(16.0, 16.0));
    ui.set_accessible_name(icon, "Coin");
    let deco = ui.add_child(list, UiWidget::Image, UiLayout::fixed(16.0, 16.0));
    let box_ = ui.panel(list).id();
    ui.push_screen(root, false);
    ui.layout(VIEW);
    let tree = ui.accessibility_tree();
    assert_eq!(tree.get(list).unwrap().role, UiRole::ScrollView);
    assert_eq!(tree.get(icon).unwrap().role, UiRole::Image);
    assert_eq!(tree.get(icon).unwrap().name, "Coin");
    assert_eq!(tree.get(deco).unwrap().name, "");
    assert_eq!(tree.get(box_).unwrap().role, UiRole::Group);
    assert_eq!(
        tree.to_string(),
        "group\n  scroll-view\n    image \"Coin\"\n    image\n    group\n"
    );
}
