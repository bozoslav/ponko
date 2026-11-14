//! The retained UI's accessibility semantics as AccessKit data: a
//! [`UiAccessTree`] (or only the nodes that changed) becomes a
//! [`TreeUpdate`], and an [`ActionRequest`] from assistive technology becomes
//! a [`UiAccessAction`]. Pure data, the same on every platform; the native
//! adapter (`access_native.rs`) hands it to the system.

use super::access::SLIDER_STEP;
use super::{UiAccessAction, UiAccessChanges, UiAccessNode, UiAccessTree, UiAccessValue, UiId, UiLive, UiRole};
use accesskit::{
    Action, ActionData, ActionRequest, Affine, Live, Node, NodeId, Rect, Role, Toggled, TreeId, TreeInfo, TreeUpdate,
};

/// The root every screen hangs from: the game's window.
pub(super) const WINDOW: NodeId = NodeId(0);

/// A UI node's id on the AccessKit side (0 is the window).
pub(super) fn node_id(id: UiId) -> NodeId {
    NodeId(id.bits() + 1)
}

fn ui_id(node: NodeId) -> Option<UiId> {
    node.0.checked_sub(1).map(UiId::from_bits)
}

fn role(role: UiRole) -> Role {
    match role {
        UiRole::Group => Role::Group,
        UiRole::Dialog => Role::Dialog,
        UiRole::Label => Role::Label,
        UiRole::Button => Role::Button,
        UiRole::Image => Role::Image,
        UiRole::ScrollView => Role::ScrollView,
        UiRole::TextInput => Role::TextInput,
        UiRole::CheckBox => Role::CheckBox,
        UiRole::Slider => Role::Slider,
    }
}

/// The window node: the screens are its children, and its transform turns
/// the UI's logical coordinates into the window's physical pixels.
fn window(tree: &UiAccessTree, title: &str, scale_factor: f64) -> Node {
    let mut node = Node::new(Role::Window);
    if !title.is_empty() {
        node.set_label(title);
    }
    node.set_children(tree.screens().iter().map(|&id| node_id(id)).collect::<Vec<_>>());
    node.set_transform(Affine::scale(scale_factor));
    node
}

fn node(n: &UiAccessNode) -> Node {
    let mut node = Node::new(role(n.role));
    if !n.name.is_empty() {
        if n.role == UiRole::Label {
            // AccessKit reads static text from a label's value: UI
            // Automation and AT-SPI name a `Label` node by it alone (macOS
            // falls back to the label, so it read them either way).
            node.set_value(n.name.as_str());
        } else {
            node.set_label(n.name.as_str());
        }
    }
    if !n.description.is_empty() {
        node.set_description(n.description.as_str());
    }
    match &n.value {
        UiAccessValue::None => {}
        UiAccessValue::Text(text) => node.set_value(text.as_str()),
        UiAccessValue::Checked(on) => node.set_toggled(if *on { Toggled::True } else { Toggled::False }),
        UiAccessValue::Range(value) => {
            node.set_numeric_value(f64::from(*value));
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(1.0);
            node.set_numeric_value_step(f64::from(SLIDER_STEP));
        }
    }
    node.set_bounds(Rect {
        x0: f64::from(n.rect.min.x),
        y0: f64::from(n.rect.min.y),
        x1: f64::from(n.rect.max.x),
        y1: f64::from(n.rect.max.y),
    });
    if n.disabled {
        node.set_disabled();
    }
    if n.modal {
        node.set_modal();
    }
    match n.live {
        UiLive::Off => {}
        UiLive::Polite => node.set_live(Live::Polite),
        UiLive::Assertive => node.set_live(Live::Assertive),
    }
    // Only what keyboard navigation could do right now is offered: a
    // disabled node, or one under another screen, is read but not operated.
    if n.focusable {
        node.add_action(Action::Focus);
        match n.role {
            UiRole::Button | UiRole::CheckBox => node.add_action(Action::Click),
            UiRole::TextInput => {
                node.add_action(Action::Click);
                node.add_action(Action::SetValue);
            }
            UiRole::Slider => {
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
                node.add_action(Action::SetValue);
            }
            _ => {}
        }
    }
    node.set_children(n.children.iter().map(|&id| node_id(id)).collect::<Vec<_>>());
    node
}

fn focus(tree: &UiAccessTree) -> NodeId {
    tree.focus().map_or(WINDOW, node_id)
}

/// The whole tree, for an adapter's first update.
pub(super) fn full_update(tree: &UiAccessTree, title: &str, scale_factor: f64) -> TreeUpdate {
    let mut nodes = Vec::with_capacity(tree.nodes().len() + 1);
    nodes.push((WINDOW, window(tree, title, scale_factor)));
    nodes.extend(tree.nodes().iter().map(|n| (node_id(n.id), node(n))));
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(WINDOW)),
        tree_id: TreeId::ROOT,
        focus: focus(tree),
    }
}

/// Only what `changes` names. Removed nodes need no entry: a node no parent
/// lists any more is dropped, and every such parent is in `changes`.
/// `window_changed`: the title or the scale factor is new.
pub(super) fn partial_update(
    tree: &UiAccessTree,
    changes: &UiAccessChanges,
    title: &str,
    scale_factor: f64,
    window_changed: bool,
) -> TreeUpdate {
    let mut nodes = Vec::with_capacity(changes.changed.len() + 1);
    if changes.screens_changed || window_changed {
        nodes.push((WINDOW, window(tree, title, scale_factor)));
    }
    nodes.extend(
        changes
            .changed
            .iter()
            .filter_map(|&id| tree.get(id))
            .map(|n| (node_id(n.id), node(n))),
    );
    TreeUpdate {
        nodes,
        tree: None,
        tree_id: TreeId::ROOT,
        focus: focus(tree),
    }
}

/// What a request from assistive technology asks of the UI; `None` for a
/// request the UI has no answer to (it is then ignored, as AccessKit asks).
pub(super) fn action(request: &ActionRequest) -> Option<UiAccessAction> {
    if request.target_tree != TreeId::ROOT {
        return None;
    }
    let id = ui_id(request.target_node)?;
    match (request.action, &request.data) {
        (Action::Focus, _) => Some(UiAccessAction::Focus(id)),
        (Action::Click, _) => Some(UiAccessAction::Activate(id)),
        (Action::Increment, _) => Some(UiAccessAction::Increment(id)),
        (Action::Decrement, _) => Some(UiAccessAction::Decrement(id)),
        (Action::SetValue, Some(ActionData::NumericValue(value))) => Some(UiAccessAction::SetValue(id, *value as f32)),
        (Action::SetValue, Some(ActionData::Value(text))) => Some(UiAccessAction::SetText(id, text.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Ui, UiAccessTracker, UiInput, UiLayout};
    use glam::Vec2;

    const VIEW: Vec2 = Vec2::new(800.0, 600.0);

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

    fn find(update: &TreeUpdate, id: NodeId) -> &Node {
        &update
            .nodes
            .iter()
            .find(|(n, _)| *n == id)
            .unwrap_or_else(|| panic!("{id:?} is not in the update"))
            .1
    }

    fn ids(update: &TreeUpdate) -> Vec<NodeId> {
        update.nodes.iter().map(|(id, _)| *id).collect()
    }

    #[test]
    fn the_full_update_is_the_window_and_every_node_with_role_name_value_and_actions() {
        let m = menu();
        let update = full_update(&m.ui.accessibility_tree(), "My game", 2.0);
        assert_eq!(update.tree, Some(TreeInfo::new(WINDOW)));
        assert_eq!(update.focus, WINDOW, "nothing focused: the window has it");
        assert_eq!(update.nodes.len(), 8);

        let window = find(&update, WINDOW);
        assert_eq!((window.role(), window.label()), (Role::Window, Some("My game")));
        assert_eq!(window.children(), [node_id(m.root)]);
        assert_eq!(window.transform(), Some(&Affine::scale(2.0)));

        let root = find(&update, node_id(m.root));
        assert_eq!(root.role(), Role::Group);
        assert_eq!(
            root.children(),
            [m.title, m.play, m.volume, m.name, m.music, m.quit].map(node_id)
        );

        let title = find(&update, node_id(m.title));
        assert_eq!(
            (title.role(), title.value(), title.label()),
            (Role::Label, Some("Settings"), None),
            "static text is in the value, where every adapter reads it"
        );
        assert!(!title.supports_action(Action::Focus), "a label is read, not operated");

        let play = find(&update, node_id(m.play));
        assert_eq!((play.role(), play.label()), (Role::Button, Some("Play")));
        assert!(play.supports_action(Action::Focus) && play.supports_action(Action::Click));
        assert!(!play.is_disabled());
        let rect = m.ui.rect(m.play).unwrap();
        assert_eq!(
            play.bounds(),
            Some(Rect {
                x0: f64::from(rect.min.x),
                y0: f64::from(rect.min.y),
                x1: f64::from(rect.max.x),
                y1: f64::from(rect.max.y)
            })
        );

        let volume = find(&update, node_id(m.volume));
        assert_eq!((volume.role(), volume.label()), (Role::Slider, Some("Volume")));
        assert_eq!(
            (
                volume.numeric_value(),
                volume.min_numeric_value(),
                volume.max_numeric_value()
            ),
            (Some(0.5), Some(0.0), Some(1.0))
        );
        assert!((volume.numeric_value_step().unwrap() - 0.05).abs() < 1e-6);
        for action in [Action::Focus, Action::Increment, Action::Decrement, Action::SetValue] {
            assert!(volume.supports_action(action), "{action:?}");
        }

        let name = find(&update, node_id(m.name));
        assert_eq!(
            (name.role(), name.label(), name.value()),
            (Role::TextInput, Some("Player name"), Some("Ann"))
        );
        assert!(name.supports_action(Action::SetValue));

        let music = find(&update, node_id(m.music));
        assert_eq!(
            (music.role(), music.label(), music.toggled()),
            (Role::CheckBox, Some("Music"), Some(Toggled::True))
        );
        assert!(music.supports_action(Action::Click));

        let quit = find(&update, node_id(m.quit));
        assert!(quit.is_disabled());
        assert!(
            !quit.supports_action(Action::Focus) && !quit.supports_action(Action::Click),
            "a disabled button offers nothing"
        );
    }

    #[test]
    fn focus_descriptions_and_live_regions_are_carried() {
        let mut m = menu();
        m.ui.set_accessible_description(m.play, "Starts a new run");
        m.ui.set_live_region(m.title, UiLive::Assertive);
        m.ui.frame(
            VIEW,
            &UiInput {
                next: true,
                ..Default::default()
            },
        );
        let update = full_update(&m.ui.accessibility_tree(), "", 1.0);
        assert_eq!(update.focus, node_id(m.play));
        assert_eq!(find(&update, node_id(m.play)).description(), Some("Starts a new run"));
        assert_eq!(find(&update, node_id(m.title)).live(), Some(Live::Assertive));
        assert_eq!(find(&update, node_id(m.volume)).live(), None);
        assert_eq!(find(&update, WINDOW).label(), None, "no title, no label");
    }

    #[test]
    fn a_partial_update_holds_only_the_changed_nodes_and_always_the_focus() {
        let mut m = menu();
        let mut tracker = UiAccessTracker::new();
        tracker.update(&m.ui);

        m.ui.set_value(m.volume, 0.75);
        let changes = tracker.update(&m.ui).unwrap();
        let update = partial_update(tracker.tree(), &changes, "My game", 1.0, false);
        assert_eq!(ids(&update), [node_id(m.volume)]);
        assert_eq!(update.tree, None, "the tree's root is said once");
        assert_eq!(update.focus, WINDOW);
        assert_eq!(find(&update, node_id(m.volume)).numeric_value(), Some(0.75));

        // Focus moves: the two nodes, and the new focus.
        m.ui.frame(
            VIEW,
            &UiInput {
                next: true,
                ..Default::default()
            },
        );
        let changes = tracker.update(&m.ui).unwrap();
        let update = partial_update(tracker.tree(), &changes, "My game", 1.0, false);
        assert_eq!(ids(&update), [node_id(m.play)]);
        assert_eq!(update.focus, node_id(m.play));

        // A new scale factor resends the window (its transform) even when
        // no node changed.
        let update = partial_update(tracker.tree(), &UiAccessChanges::default(), "My game", 2.0, true);
        assert_eq!(ids(&update), [WINDOW]);
        assert_eq!(find(&update, WINDOW).transform(), Some(&Affine::scale(2.0)));
    }

    #[test]
    fn a_modal_dialog_replaces_the_windows_children_and_is_marked_modal() {
        let mut m = menu();
        let mut tracker = UiAccessTracker::new();
        tracker.update(&m.ui);
        let dialog = m.ui.root().layout(UiLayout::centered(300.0, 120.0)).id();
        let yes = m.ui.button(dialog, "Yes").layout(UiLayout::row(30.0)).id();
        m.ui.push_screen(dialog, true);
        m.ui.frame(VIEW, &UiInput::default());

        let changes = tracker.update(&m.ui).unwrap();
        let update = partial_update(tracker.tree(), &changes, "", 1.0, false);
        assert_eq!(ids(&update), [WINDOW, node_id(dialog), node_id(yes)]);
        assert_eq!(
            find(&update, WINDOW).children(),
            [node_id(dialog)],
            "the covered screen is no longer listed, so it is dropped"
        );
        let node = find(&update, node_id(dialog));
        assert!(node.role() == Role::Dialog && node.is_modal());

        // A non-modal overlay keeps the screen below, without its actions.
        m.ui.pop_screen();
        let toast = m.ui.root().layout(UiLayout::anchored(0.0, 0.0, 200.0, 40.0)).id();
        m.ui.push_screen(toast, false);
        m.ui.frame(VIEW, &UiInput::default());
        let changes = tracker.update(&m.ui).unwrap();
        let update = partial_update(tracker.tree(), &changes, "", 1.0, false);
        assert_eq!(find(&update, WINDOW).children(), [node_id(m.root), node_id(toast)]);
        let play = find(&update, node_id(m.play));
        assert!(!play.supports_action(Action::Click) && !play.is_disabled());
    }

    #[test]
    fn requests_become_ui_actions_and_unknown_ones_are_ignored() {
        let id = UiId::from_bits(4);
        let request = |action, data| ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: node_id(id),
            data,
        };
        assert_eq!(action(&request(Action::Focus, None)), Some(UiAccessAction::Focus(id)));
        assert_eq!(
            action(&request(Action::Click, None)),
            Some(UiAccessAction::Activate(id))
        );
        assert_eq!(
            action(&request(Action::Increment, None)),
            Some(UiAccessAction::Increment(id))
        );
        assert_eq!(
            action(&request(Action::Decrement, None)),
            Some(UiAccessAction::Decrement(id))
        );
        assert_eq!(
            action(&request(Action::SetValue, Some(ActionData::NumericValue(0.25)))),
            Some(UiAccessAction::SetValue(id, 0.25))
        );
        assert_eq!(
            action(&request(Action::SetValue, Some(ActionData::Value("Bea".into())))),
            Some(UiAccessAction::SetText(id, "Bea".into()))
        );
        // No answer: an action the UI doesn't have, a value-less set, the
        // window itself.
        assert_eq!(action(&request(Action::ScrollIntoView, None)), None);
        assert_eq!(action(&request(Action::SetValue, None)), None);
        let mut to_window = request(Action::Focus, None);
        to_window.target_node = WINDOW;
        assert_eq!(action(&to_window), None);
    }
}
