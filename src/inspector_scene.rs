//! The inspector's scene editing beyond selecting and dragging in 2D: the
//! hierarchy panel (parenting by drag and drop), the asset browser (drag a
//! mesh, prefab or material into the world), multi-selection, and in 3D
//! picking with the mouse ray, the move / rotate / scale gizmo with
//! snapping, and a fly camera.
//!
//! Every change goes through the shared editor (`crate::editor`), so it can
//! be undone and redone, and the control endpoint's `editor.*` methods make
//! the same ones. The math lives in `crate::edit3d`, tested without egui.

use egui::{Color32, Key, RichText, ScrollArea, Sense, Stroke, Ui};
use glam::Vec2;
use ponko_ecs::Entity;

use super::Panels;
use crate::ctx::Ctx;
use crate::editor::{self, AssetList};
use crate::reflect::WorldReflect;

/// Where Ctrl+D puts a 3D copy, from the original (world units).
#[cfg(feature = "3d")]
const DUPLICATE_OFFSET3: glam::Vec3 = glam::Vec3::new(1.0, 0.0, 1.0);
/// Fly camera speed, world units per second (Shift: four times that).
#[cfg(feature = "3d")]
const FLY_SPEED: f32 = 8.0;
/// Height of one hierarchy row (the list scrolls).
const ROW_HEIGHT: f32 = 18.0;
/// Worlds bigger than this open with the hierarchy closed: building the
/// tree walks every entity each frame it's open.
pub(super) const HIERARCHY_OPEN_UP_TO: usize = 2000;

/// Something dragged from the asset browser.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AssetDrag {
    /// A glTF file: dropped, it becomes a model.
    Mesh(String),
    /// A prefab file: dropped, it's spawned.
    Prefab(String),
    /// A material of a loaded mesh file: dropped on a model, it paints it.
    Material { mesh: String, index: usize },
}

/// An entity dragged in the hierarchy, to be dropped on its new parent.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HierarchyDrag(Entity);

/// The scene editing state of the panels.
#[derive(Default)]
pub(crate) struct SceneEdit {
    /// Edit in 3D (`Some(true)`) or 2D; `None` decides from the world:
    /// 3D when any entity has a 3D transform.
    pub view3d: Option<bool>,
    /// The view this frame (`view3d` decided).
    pub active3d: bool,
    /// The right mouse button is held in the world: the 3D camera flies.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub flying: bool,
    #[cfg(feature = "3d")]
    pub gizmo: Gizmo,
    /// The asset browser's list, until refreshed.
    assets: Option<AssetList>,
}

/// The 3D gizmo's settings and the drag in progress.
#[cfg(feature = "3d")]
#[derive(Default)]
pub(crate) struct Gizmo {
    pub mode: crate::edit3d::Mode,
    pub space: crate::edit3d::Space,
    pub snap: crate::edit3d::Snap,
    pub drag: Option<(crate::edit3d::Drag, editor::Before)>,
    /// The handle under the mouse, drawn highlighted.
    hover: Option<crate::edit3d::Handle>,
}

impl SceneEdit {
    /// Drops a gizmo drag in progress without recording it (a scene load
    /// replaced the entities it moves).
    pub fn forget_drag(&mut self) {
        #[cfg(feature = "3d")]
        {
            self.gizmo.drag = None;
        }
    }

    /// Lists the assets again next time the browser is drawn.
    pub fn forget_assets(&mut self) {
        self.assets = None;
    }

    /// Whether `code` (with Ctrl/⌘ held: `command`) is a key the 3D editor
    /// uses, so the game doesn't get it while editing.
    pub fn claims_key(&self, code: winit::keyboard::KeyCode, command: bool) -> bool {
        use winit::keyboard::KeyCode;
        match code {
            KeyCode::KeyY => command,
            KeyCode::KeyW
            | KeyCode::KeyE
            | KeyCode::KeyR
            | KeyCode::KeyF
            | KeyCode::KeyA
            | KeyCode::KeyS
            | KeyCode::KeyD
            | KeyCode::KeyQ
            | KeyCode::ShiftLeft
            | KeyCode::ShiftRight => self.active3d,
            _ => false,
        }
    }
}

/// Whether any entity has a 3D transform.
fn world_is_3d(ctx: &Ctx) -> bool {
    #[cfg(feature = "3d")]
    {
        let world = &ctx.world;
        let Some(id) = world
            .raw_ref()
            .component_by_name(std::any::type_name::<crate::d3::Transform>())
        else {
            return false;
        };
        world.raw_ref().archetypes().iter().any(|a| !a.is_empty() && a.has(id))
    }
    #[cfg(not(feature = "3d"))]
    {
        let _ = ctx;
        false
    }
}

/// A short name for `e` in the hierarchy: its mesh file, else its
/// components (without the transform and links).
fn label(ctx: &Ctx, e: Entity) -> String {
    if let Ok(model) = ctx.world.get_json(e, "Model")
        && let Some(mesh) = model["mesh"].as_str()
    {
        let file = mesh.rsplit('/').next().unwrap_or(mesh);
        return file.to_string();
    }
    let names: Vec<String> = ctx
        .world
        .components_of(e)
        .map(|ids| {
            ids.iter()
                .map(|&id| crate::reflect::short_name(ctx.world.component_info(id).name()))
                .filter(|n| {
                    !matches!(
                        n.as_str(),
                        "Transform2D" | "Transform3D" | "Attached" | "Attached3D" | "PrefabParts"
                    )
                })
                .take(3)
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        "entity".into()
    } else {
        names.join(", ")
    }
}

impl Panels {
    /// The entity under window point `at`: along the mouse ray in 3D,
    /// else the 2D pick.
    pub(super) fn pick_any(&self, ctx: &mut Ctx, at: Vec2) -> Option<Entity> {
        #[cfg(feature = "3d")]
        if self.scene.active3d {
            let ray = crate::edit3d::Ray::from_screen(&ctx.scene3d.camera, at);
            return crate::edit3d::pick(&ctx.world, &ctx.gfx.assets3d, &ray, &[]).map(|h| h.entity);
        }
        super::pick(ctx, at)
    }

    /// Saves the world to scene file `path`, which becomes the one Save
    /// writes by default.
    pub(super) fn save_scene(&mut self, ctx: &mut Ctx, path: &str) {
        let saved = ctx
            .assets
            .save_scene(&ctx.world, path)
            .map_err(String::from)
            .map(|()| format!("saved {path}"));
        if let Some(done) = self.report(saved) {
            ctx.scene_file = Some(path.to_owned());
            self.status = Some(done);
            self.scene.assets = None;
        }
    }

    /// The selection: the primary entity first, then the others.
    pub(crate) fn selection(&self) -> Vec<Entity> {
        let mut out: Vec<Entity> = self.selected.into_iter().collect();
        for &e in &self.others {
            if !out.contains(&e) {
                out.push(e);
            }
        }
        out
    }

    /// Selects `e` alone, or with `add` toggles it in the selection.
    pub(crate) fn select(&mut self, e: Option<Entity>, add: bool) {
        match (e, add) {
            (Some(e), true) => {
                if self.selected == Some(e) {
                    self.selected = self.others.pop();
                } else if let Some(at) = self.others.iter().position(|&o| o == e) {
                    self.others.remove(at);
                } else {
                    if let Some(old) = self.selected {
                        self.others.push(old);
                    }
                    self.selected = Some(e);
                }
            }
            (e, _) => {
                self.selected = e;
                self.others.clear();
            }
        }
    }

    /// Drops dead entities from the selection and decides the view.
    pub(crate) fn scene_frame(&mut self, ctx: &Ctx) {
        self.others
            .retain(|&e| ctx.world.is_alive(e) && Some(e) != self.selected);
        if self.selected.is_none() && !self.others.is_empty() {
            self.selected = Some(self.others.remove(0));
        }
        self.scene.active3d = self.scene.view3d.unwrap_or_else(|| world_is_3d(ctx));
    }

    /// Undo, redo, the view and, in 3D, the gizmo's settings.
    pub(crate) fn scene_tools(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ctx.editor.can_redo(), egui::Button::new("Redo"))
                .on_hover_text("Ctrl+Shift+Z or Ctrl+Y")
                .clicked()
            {
                let redone = ctx.edit(|ed, ctx| ed.redo(ctx));
                self.report(redone);
            }
            ui.label("View");
            let mut three = self.scene.active3d;
            ui.selectable_value(&mut three, false, "2D");
            ui.add_enabled_ui(cfg!(feature = "3d"), |ui| ui.selectable_value(&mut three, true, "3D"));
            if three != self.scene.active3d {
                self.scene.view3d = Some(three);
                self.scene.active3d = three;
            }
        });
        #[cfg(feature = "3d")]
        if self.scene.active3d {
            use crate::edit3d::{Mode, Space};
            let g = &mut self.scene.gizmo;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut g.mode, Mode::Translate, "Move")
                    .on_hover_text("W");
                ui.selectable_value(&mut g.mode, Mode::Rotate, "Rotate")
                    .on_hover_text("E");
                ui.selectable_value(&mut g.mode, Mode::Scale, "Scale")
                    .on_hover_text("R");
                ui.separator();
                ui.selectable_value(&mut g.space, Space::World, "World");
                ui.selectable_value(&mut g.space, Space::Local, "Local")
                    .on_hover_text("The selected entity's own axes (scaling always uses them)");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut g.snap.on, "Snap");
                ui.add(
                    egui::DragValue::new(&mut g.snap.step)
                        .range(0.01..=100.0)
                        .speed(0.05)
                        .prefix("move "),
                );
                let mut degrees = g.snap.angle.to_degrees();
                if ui
                    .add(
                        egui::DragValue::new(&mut degrees)
                            .range(1.0..=180.0)
                            .speed(1.0)
                            .suffix("°"),
                    )
                    .changed()
                {
                    g.snap.angle = degrees.to_radians();
                }
                ui.add(
                    egui::DragValue::new(&mut g.snap.scale)
                        .range(0.01..=10.0)
                        .speed(0.01)
                        .prefix("scale "),
                );
            });
            ui.label(
                RichText::new(
                    "Click picks (Shift adds), drag a handle, right-drag looks, WASD/QE fly, wheel moves, F frames.",
                )
                .weak(),
            );
        }
    }

    /// The hierarchy: every entity under what it hangs off. Drag a row onto
    /// another to make it follow that one, or onto the top line to detach it.
    pub(crate) fn hierarchy_panel(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        let nodes = editor::hierarchy(&ctx.world);
        let detach = ui
            .add(egui::Label::new(RichText::new("⤒ drop here to detach").weak()).sense(Sense::hover()))
            .on_hover_text("Drag an entity from the list here to make it follow nothing");
        if let Some(dragged) = detach.dnd_release_payload::<HierarchyDrag>() {
            self.reparent(ctx, dragged.0, None);
        }
        let mut depth: std::collections::HashMap<Entity, usize> = std::collections::HashMap::new();
        for n in &nodes {
            let d = n.parent.and_then(|(p, _)| depth.get(&p)).map_or(0, |d| d + 1);
            depth.insert(n.entity, d);
        }
        let mut clicked = None;
        let mut dropped = None;
        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let selection = self.selection();
        ScrollArea::vertical()
            .id_salt("inspector-hierarchy")
            .max_height(260.0)
            .auto_shrink([false, true])
            .show_rows(ui, ROW_HEIGHT, nodes.len(), |ui, range| {
                for n in &nodes[range] {
                    let e = n.entity;
                    let indent = depth[&e] as f32 * 14.0;
                    let mut text = label(ctx, e);
                    if n.prefab {
                        text = format!("▣ {text}");
                    }
                    if let Some((_, link)) = n.parent
                        && link != editor::Link::Attached
                        && link != editor::Link::Attached3D
                    {
                        text = format!("{text}  ({})", link.name().replace('_', " "));
                    }
                    let selected = selection.contains(&e);
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        let row = ui
                            .dnd_drag_source(egui::Id::unique(("hierarchy", e)), HierarchyDrag(e), |ui| {
                                ui.selectable_label(selected, format!("{text}  #{}", e.to_bits()))
                            })
                            .inner;
                        if row.clicked() {
                            clicked = Some(e);
                        }
                        if let Some(dragged) = row.dnd_release_payload::<HierarchyDrag>() {
                            dropped = Some((dragged.0, e));
                        }
                        if row.dnd_hover_payload::<HierarchyDrag>().is_some() {
                            ui.painter().rect_stroke(
                                row.rect,
                                2.0,
                                Stroke::new(1.5, Color32::YELLOW),
                                egui::StrokeKind::Outside,
                            );
                        }
                    });
                }
            });
        if let Some(e) = clicked {
            self.select(Some(e), shift);
        }
        if let Some((child, parent)) = dropped
            && child != parent
        {
            self.reparent(ctx, child, Some(parent));
        }
    }

    /// Makes `child` follow `parent` (or nothing), with undo.
    pub(crate) fn reparent(&mut self, ctx: &mut Ctx, child: Entity, parent: Option<Entity>) {
        let linked = ctx.edit(|ed, ctx| ed.set_parent(ctx, child, parent));
        self.report(linked);
    }

    /// The asset browser: meshes, prefabs and materials to drag into the
    /// world, scenes to open.
    pub(crate) fn asset_browser(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        if ui
            .small_button("⟳ list again")
            .on_hover_text("Read assets/ again, and the materials of the meshes loaded since")
            .clicked()
        {
            self.scene.assets = None;
        }
        let assets = self.scene.assets.get_or_insert_with(|| editor::browse(ctx)).clone();
        let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
        let source = |ui: &mut Ui, id: (&str, &str), payload: AssetDrag, text: String, tip: &str| {
            ui.dnd_drag_source(egui::Id::unique(id), payload, |ui| {
                ui.add(egui::Label::new(text).sense(Sense::click_and_drag()))
            })
            .inner
            .on_hover_text(tip.to_string());
        };
        if cfg!(feature = "3d") {
            ui.label(RichText::new("Meshes").strong());
            ui.horizontal_wrapped(|ui| {
                for path in &assets.meshes {
                    source(ui, ("mesh", path), AssetDrag::Mesh(path.clone()), name(path), path);
                }
            });
        }
        ui.label(RichText::new("Prefabs").strong());
        ui.horizontal_wrapped(|ui| {
            for path in &assets.prefabs {
                source(ui, ("prefab", path), AssetDrag::Prefab(path.clone()), name(path), path);
            }
        });
        if !assets.materials.is_empty() {
            ui.label(RichText::new("Materials").strong())
                .on_hover_text("Of the mesh files loaded so far: drop one on a model to paint it");
            ui.horizontal_wrapped(|ui| {
                for (mesh, index, material) in &assets.materials {
                    let text = if material.is_empty() {
                        format!("{} #{index}", name(mesh))
                    } else {
                        material.clone()
                    };
                    let payload = AssetDrag::Material {
                        mesh: mesh.clone(),
                        index: *index,
                    };
                    let id = format!("{mesh}#{index}");
                    source(ui, ("material", &id), payload, text, mesh);
                }
            });
        }
        ui.label(RichText::new("Scenes").strong());
        let mut open = None;
        ui.horizontal_wrapped(|ui| {
            for path in &assets.scenes {
                if ui
                    .small_button(name(path))
                    .on_hover_text(format!("Open {path}: replaces the world"))
                    .clicked()
                {
                    open = Some(path.clone());
                }
            }
        });
        if let Some(path) = open {
            let loaded = ctx.load_scene(&path).map(|()| format!("loaded {path}"));
            if let Some(done) = self.report(loaded) {
                self.status = Some(done);
                self.scene_path = path;
                self.select(None, false);
            }
        }
    }

    /// An asset dropped over the world at window point `at`, if one was
    /// released there this frame. Returns whether one was.
    pub(crate) fn drop_asset(&mut self, ui: &Ui, ctx: &mut Ctx, at: Vec2) -> bool {
        let released = ui.input(|i| i.pointer.primary_released());
        if !released || ui.ctx().is_pointer_over_egui() {
            return false;
        }
        let Some(asset) = egui::DragAndDrop::take_payload::<AssetDrag>(ui.ctx()) else {
            return false;
        };
        let placed = self.place_asset(ctx, &asset, at);
        if let Some(e) = self.report(placed) {
            self.select(Some(e), false);
        }
        true
    }

    /// Places `asset` where window point `at` points: in 3D on the ground
    /// (y = 0, snapped when snapping is on), in 2D at that world point.
    /// A material paints the model under the point.
    pub(crate) fn place_asset(
        &mut self,
        ctx: &mut Ctx,
        asset: &AssetDrag,
        at: Vec2,
    ) -> Result<Entity, crate::error::Error> {
        #[cfg(feature = "3d")]
        if self.scene.active3d || matches!(asset, AssetDrag::Mesh(_) | AssetDrag::Material { .. }) {
            use crate::edit3d::{Ray, ground_point, pick, snap};
            let ray = Ray::from_screen(&ctx.scene3d.camera, at);
            let mut point = ground_point(&ray, 0.0, 10.0);
            let s = self.scene.gizmo.snap;
            if s.on {
                point = point.map(|v| snap(v, s.step));
            }
            return match asset {
                AssetDrag::Mesh(path) => ctx.edit(|ed, ctx| ed.spawn_model(ctx, path, crate::d3::Transform::at(point))),
                AssetDrag::Prefab(path) => ctx.edit(|ed, ctx| ed.spawn_prefab(ctx, path, point)),
                AssetDrag::Material { mesh, index } => {
                    let target = pick(&ctx.world, &ctx.gfx.assets3d, &ray, &[])
                        .map(|h| h.entity)
                        .filter(|&e| ctx.world.get::<crate::render::Model>(e).is_some())
                        .ok_or_else(|| {
                            crate::error::Error::new(
                                crate::error::ErrorKind::NotFound,
                                "drop a material on a model to paint it",
                            )
                        })?;
                    let value = serde_json::json!({ "mesh": mesh, "material": index });
                    ctx.edit(|ed, ctx| ed.set(ctx, target, "ModelMaterial", value))?;
                    Ok(target)
                }
            };
        }
        match asset {
            AssetDrag::Prefab(path) => {
                let world_at = ctx.camera.screen_to_world(at);
                ctx.edit(|ed, ctx| ed.spawn_prefab(ctx, path, world_at))
            }
            _ => Err(crate::error::Error::new(
                crate::error::ErrorKind::Unsupported,
                "meshes and materials need the `3d` feature",
            )),
        }
    }

    /// Delete and duplicate for the whole selection; redo.
    pub(crate) fn selection_keys(&mut self, ui: &Ui, ctx: &mut Ctx) -> bool {
        let (redo, delete, duplicate) = ui.input(|i| {
            (
                i.modifiers.command && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                i.modifiers.command && i.key_pressed(Key::D),
            )
        });
        if redo {
            let redone = ctx.edit(|ed, ctx| ed.redo(ctx));
            self.report(redone);
            return true;
        }
        let selection = self.selection();
        if selection.len() < 2 && !(self.scene.active3d && duplicate) {
            return false;
        }
        if delete {
            let deleted = ctx.edit(|ed, ctx| ed.delete_many(ctx, &selection));
            self.report(deleted);
            self.select(None, false);
            return true;
        }
        if duplicate {
            #[cfg(feature = "3d")]
            let offset3 = editor::Offset3(DUPLICATE_OFFSET3);
            #[cfg(not(feature = "3d"))]
            let offset3 = editor::Offset3::ZERO;
            let copies = ctx.edit(|ed, ctx| ed.duplicate_many(ctx, &selection, super::DUPLICATE_OFFSET, offset3));
            if let Some(copies) = self.report(copies) {
                self.select(None, false);
                for c in copies {
                    self.select(Some(c), true);
                }
            }
            return true;
        }
        false
    }
}

#[cfg(feature = "3d")]
impl Panels {
    /// The selected entities the gizmo moves: those with a 3D transform,
    /// without any whose parent is selected too (it carries them), with
    /// their world transforms, the primary one first.
    fn gizmo_targets(&self, ctx: &Ctx) -> Vec<(Entity, crate::d3::Transform)> {
        let selection = self.selection();
        selection
            .iter()
            .filter(|&&e| !editor::ancestors(&ctx.world, e).iter().any(|a| selection.contains(a)))
            .filter_map(|&e| ctx.world.get::<crate::d3::Transform>(e).map(|t| (e, *t)))
            .collect()
    }

    /// The gizmo's pivot and axes for the selection.
    fn gizmo_frame(&self, ctx: &Ctx) -> Option<(glam::Vec3, [glam::Vec3; 3])> {
        use crate::edit3d::{Mode, Space, axes};
        let targets = self.gizmo_targets(ctx);
        let first = targets.first()?.1;
        let pivot = targets.iter().map(|(_, t)| t.position).sum::<glam::Vec3>() / targets.len() as f32;
        let g = &self.scene.gizmo;
        let space = if g.mode == Mode::Scale { Space::Local } else { g.space };
        Some((pivot, axes(space, first.rotation)))
    }

    /// The mouse and keys in the world while editing in 3D: picking, gizmo
    /// drags, the fly camera, dropped assets, the gizmo and focus keys.
    pub(super) fn world_input_3d(&mut self, ui: &Ui, ctx: &mut Ctx) {
        use crate::edit3d::{Drag, Ray, fly, hit, layout};
        let input = ui.input(|i| {
            let p = &i.pointer;
            let key = |k| if i.key_down(k) { 1.0 } else { 0.0 };
            let step = glam::Vec3::new(
                key(Key::D) - key(Key::A),
                key(Key::E) - key(Key::Q),
                key(Key::W) - key(Key::S),
            );
            (
                p.latest_pos().map(|p| Vec2::new(p.x, p.y)),
                p.primary_pressed(),
                p.primary_down(),
                p.secondary_down(),
                p.middle_down(),
                Vec2::new(p.delta().x, p.delta().y),
                i.smooth_scroll_delta.y,
                (i.modifiers.shift, i.modifiers.command),
                step,
                i.stable_dt.min(0.1),
            )
        });
        let (pos, pressed, down, look, pan, delta, scroll, (shift, command), step, dt) = input;
        let Some(pos) = pos else { return };
        if self.drop_asset(ui, ctx, pos) {
            return;
        }
        let over_ui = ui.ctx().is_pointer_over_egui();
        let camera = &mut ctx.scene3d.camera;

        // The camera: right-drag looks and WASD/QE fly, middle-drag pans,
        // the wheel moves forward.
        self.scene.flying = look && (self.scene.flying || !over_ui);
        if self.scene.flying {
            let speed = if shift { FLY_SPEED * 4.0 } else { FLY_SPEED };
            fly(camera, delta, step * speed * dt);
        } else if pan && !over_ui {
            let px = crate::edit3d::pixel_size(camera, camera.position + camera.forward() * 10.0);
            fly(camera, Vec2::ZERO, glam::Vec3::new(-delta.x * px, delta.y * px, 0.0));
        }
        if scroll != 0.0 && !over_ui {
            fly(camera, Vec2::ZERO, glam::Vec3::new(0.0, 0.0, scroll * 0.02));
        }
        let camera = ctx.scene3d.camera;
        let ray = Ray::from_screen(&camera, pos);

        // A drag in progress follows the mouse until the button goes up.
        if let Some((drag, _)) = &self.scene.gizmo.drag {
            if down {
                if let Some(moved) = drag.update(&ray, self.scene.gizmo.snap) {
                    for (e, t) in moved {
                        editor::place_world3d(&mut ctx.world, e, t);
                    }
                    editor::settle(&mut ctx.world);
                }
                return;
            }
            self.end_gizmo_drag(ctx);
        }

        // The handle under the mouse.
        let g = &self.scene.gizmo;
        let frame = self.gizmo_frame(ctx);
        let hovered = frame
            .and_then(|(pivot, axes)| layout(&camera, g.mode, pivot, axes))
            .and_then(|l| hit(&l, g.mode, pos));
        self.scene.gizmo.hover = if over_ui { None } else { hovered };
        if !pressed || over_ui || self.scene.flying {
            return;
        }
        if let Some(handle) = self.scene.gizmo.hover {
            let targets = self.gizmo_targets(ctx);
            let entities: Vec<Entity> = targets.iter().map(|&(e, _)| e).collect();
            let g = &self.scene.gizmo;
            if let Some(drag) = Drag::begin(g.mode, handle, g.space, targets, camera.forward(), &ray) {
                let before = editor::Before::of3d(&ctx.world, &entities);
                self.scene.gizmo.drag = Some((drag, before));
                return;
            }
        }
        if let Some(path) = self.placing.clone() {
            let placed = self.place_asset(ctx, &AssetDrag::Prefab(path), pos);
            if let Some(e) = self.report(placed) {
                self.select(Some(e), false);
            }
            if !shift {
                self.placing = None;
            }
            return;
        }
        let picked = crate::edit3d::pick(&ctx.world, &ctx.gfx.assets3d, &ray, &[]).map(|h| h.entity);
        self.select(picked, shift || command);
    }

    /// Ends a gizmo drag: one undo step for everything it moved.
    pub(super) fn end_gizmo_drag(&mut self, ctx: &mut Ctx) {
        if let Some((_, before)) = self.scene.gizmo.drag.take() {
            ctx.edit(|ed, ctx| ed.changed(&ctx.world, before));
        }
    }

    /// W, E, R pick the gizmo; F frames the selection.
    pub(super) fn keys_3d(&mut self, ui: &Ui, ctx: &mut Ctx) {
        use crate::edit3d::Mode;
        if self.scene.flying || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (w, e, r, f, command) = ui.input(|i| {
            (
                i.key_pressed(Key::W),
                i.key_pressed(Key::E),
                i.key_pressed(Key::R),
                i.key_pressed(Key::F),
                i.modifiers.command,
            )
        });
        if command {
            return;
        }
        let g = &mut self.scene.gizmo;
        if w {
            g.mode = Mode::Translate;
        } else if e {
            g.mode = Mode::Rotate;
        } else if r {
            g.mode = Mode::Scale;
        }
        if f {
            self.focus(ctx);
        }
    }

    /// Moves the camera to frame the selection.
    pub(crate) fn focus(&self, ctx: &mut Ctx) {
        let spheres: Vec<(glam::Vec3, f32)> = self
            .selection()
            .iter()
            .filter_map(|&e| crate::edit3d::world_sphere(&ctx.world, &ctx.gfx.assets3d, e))
            .collect();
        let Some(&(first, _)) = spheres.first() else { return };
        let (lo, hi) = spheres.iter().fold((first, first), |(lo, hi), &(c, r)| {
            (lo.min(c - glam::Vec3::splat(r)), hi.max(c + glam::Vec3::splat(r)))
        });
        crate::edit3d::focus(&mut ctx.scene3d.camera, (lo + hi) * 0.5, (hi - lo).length() * 0.5);
    }

    /// Rings around the selected entities and the gizmo, drawn over the world.
    pub(super) fn highlight_3d(&self, ui: &Ui, ctx: &Ctx) {
        use crate::edit3d::{Handle, Mode, layout, pixel_size, world_sphere, world_to_screen};
        let camera = &ctx.scene3d.camera;
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::unique("inspector-gizmo"),
        ));
        let pos = |p: Vec2| egui::pos2(p.x, p.y);
        for (i, e) in self.selection().into_iter().enumerate() {
            let Some((center, radius)) = world_sphere(&ctx.world, &ctx.gfx.assets3d, e) else {
                continue;
            };
            let Some(at) = world_to_screen(camera, center) else {
                continue;
            };
            let r = (radius / pixel_size(camera, center)).clamp(6.0, 2000.0);
            let color = if i == 0 {
                Color32::from_rgb(255, 200, 40)
            } else {
                Color32::from_rgb(255, 230, 140)
            };
            painter.circle_stroke(pos(at), r, Stroke::new(1.5, color));
        }
        let Some((pivot, axes)) = self.gizmo_frame(ctx) else {
            return;
        };
        let g = &self.scene.gizmo;
        let Some(l) = layout(camera, g.mode, pivot, axes) else {
            return;
        };
        let active = g.drag.as_ref().map(|(d, _)| d.handle).or(g.hover);
        let colors = [
            Color32::from_rgb(230, 70, 70),
            Color32::from_rgb(90, 200, 90),
            Color32::from_rgb(80, 130, 240),
        ];
        let tint = |handle: Handle, base: Color32| {
            if active == Some(handle) { Color32::YELLOW } else { base }
        };
        let center = pos(l.center);
        match g.mode {
            Mode::Rotate => {
                for (i, ring) in l.rings.iter().enumerate() {
                    let points: Vec<egui::Pos2> = ring.iter().map(|&p| pos(p)).collect();
                    painter.add(egui::Shape::line(
                        points,
                        Stroke::new(2.0, tint(Handle::Axis(i), colors[i])),
                    ));
                }
            }
            Mode::Translate | Mode::Scale => {
                for (i, &base) in colors.iter().enumerate() {
                    let Some(tip) = l.tips[i] else { continue };
                    let color = tint(Handle::Axis(i), base);
                    painter.line_segment([center, pos(tip)], Stroke::new(2.5, color));
                    if g.mode == Mode::Translate {
                        painter.circle_filled(pos(tip), 5.0, color);
                    } else {
                        painter.rect_filled(egui::Rect::from_center_size(pos(tip), egui::vec2(9.0, 9.0)), 0.0, color);
                    }
                    if let Some(p) = l.planes[i] {
                        let c = tint(Handle::Plane(i), base.gamma_multiply(0.7));
                        painter.rect_filled(egui::Rect::from_center_size(pos(p), egui::vec2(10.0, 10.0)), 1.0, c);
                    }
                }
                painter.circle_filled(center, 6.0, tint(Handle::Center, Color32::WHITE));
            }
        }
    }
}

/// Without the `3d` feature there is no 3D view: these never run.
#[cfg(not(feature = "3d"))]
impl Panels {
    pub(super) fn world_input_3d(&mut self, _: &Ui, _: &mut Ctx) {}
    pub(super) fn keys_3d(&mut self, _: &Ui, _: &mut Ctx) {}
    pub(super) fn highlight_3d(&self, _: &Ui, _: &Ctx) {}
    pub(super) fn end_gizmo_drag(&mut self, _: &mut Ctx) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one frame of `p` on a lasting egui context with these events.
    /// Held Ctrl/Shift come from the key events, as egui wants them.
    fn frame(egui: &egui::Context, p: &mut Panels, ctx: &mut Ctx, mut events: Vec<egui::Event>) {
        let held = events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .fold(egui::Modifiers::NONE, |a, b| a | b);
        events.insert(0, egui::Event::ModifiersChanged(held));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0))),
            events,
            ..Default::default()
        };
        let mut out = egui.run_ui(input, |ui| p.show(ui, ctx));
        out.textures_delta.clear();
    }
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    fn pointer(at: egui::Pos2) -> egui::Event {
        egui::Event::PointerMoved(at)
    }
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    fn button(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }
    fn key(key: Key, command: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if command {
                egui::Modifiers::COMMAND
            } else {
                Default::default()
            },
        }
    }

    #[test]
    fn shift_clicks_build_a_selection_and_a_plain_click_replaces_it() {
        let mut p = Panels::default();
        let (a, b, c) = (Entity::from_bits(1), Entity::from_bits(2), Entity::from_bits(3));
        p.select(Some(a), false);
        p.select(Some(b), true);
        p.select(Some(c), true);
        assert_eq!(p.selection(), vec![c, a, b], "the last one added is the primary");
        p.select(Some(a), true);
        assert_eq!(p.selection(), vec![c, b], "adding a selected one takes it out");
        p.select(Some(b), false);
        assert_eq!(p.selection(), vec![b]);
        p.select(None, false);
        assert!(p.selection().is_empty());
    }

    #[test]
    fn several_selected_entities_are_deleted_and_undone_together() {
        let egui = egui::Context::default();
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(1280.0, 720.0));
        ctx.time.pause();
        let a = ctx.world.spawn((crate::d2::Transform::default(),));
        let b = ctx.world.spawn((crate::d2::Transform::at(Vec2::new(50.0, 0.0)),));
        let mut p = Panels::default();
        p.select(Some(a), false);
        p.select(Some(b), true);
        frame(&egui, &mut p, &mut ctx, vec![key(Key::Delete, false)]);
        assert_eq!(ctx.world.len(), 0);
        frame(&egui, &mut p, &mut ctx, vec![key(Key::Z, true)]);
        assert_eq!(ctx.world.len(), 2, "one undo brings both back");
        let shift_z = egui::Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        };
        frame(&egui, &mut p, &mut ctx, vec![shift_z]);
        assert_eq!(ctx.world.len(), 0, "Ctrl+Shift+Z redoes");
    }

    #[test]
    fn reparenting_from_the_hierarchy_refuses_loops_and_detaching_makes_a_root() {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(1280.0, 720.0));
        let parent = ctx.world.spawn((crate::d2::Transform::default(),));
        let child = ctx.world.spawn((crate::d2::Transform::at(Vec2::new(4.0, 0.0)),));
        let mut p = Panels::default();
        p.reparent(&mut ctx, child, Some(parent));
        let nodes = editor::hierarchy(&ctx.world);
        assert_eq!(nodes[1].parent.map(|(e, _)| e), Some(parent));
        p.reparent(&mut ctx, parent, Some(child));
        assert!(p.error.as_deref().is_some_and(|e| e.contains("loop")), "{:?}", p.error);
        p.reparent(&mut ctx, child, None);
        assert!(editor::hierarchy(&ctx.world)[1].parent.is_none());
    }

    #[cfg(feature = "3d")]
    mod three {
        use super::*;
        use crate::d3::Transform as T3;
        use glam::{Vec3, vec3};

        /// A paused 3D world seen from (0, 6, 10) looking at the origin.
        fn scene() -> (egui::Context, Panels, Ctx) {
            let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(1280.0, 720.0));
            ctx.time.pause();
            ctx.scene3d.camera.position = vec3(0.0, 6.0, 10.0);
            ctx.scene3d.camera.look_at(Vec3::ZERO);
            let egui = egui::Context::default();
            let mut panels = Panels::default();
            panels.scene.view3d = Some(true);
            frame(&egui, &mut panels, &mut ctx, vec![]);
            (egui, panels, ctx)
        }

        fn screen(ctx: &Ctx, p: Vec3) -> egui::Pos2 {
            let s = crate::edit3d::world_to_screen(&ctx.scene3d.camera, p).unwrap();
            egui::pos2(s.x, s.y)
        }

        /// Presses at `from`, moves to `to` and releases, a frame each.
        fn drag(egui: &egui::Context, p: &mut Panels, ctx: &mut Ctx, from: egui::Pos2, to: egui::Pos2) {
            frame(egui, p, ctx, vec![pointer(from)]);
            frame(egui, p, ctx, vec![button(from, true)]);
            frame(egui, p, ctx, vec![pointer(to)]);
            frame(egui, p, ctx, vec![button(to, false)]);
        }

        #[test]
        fn a_level_is_assembled_saved_and_reloaded_with_the_editor_alone() {
            let (egui, mut p, mut ctx) = scene();
            let path = format!("ponko-test-inspector-level-{}.json", std::process::id());
            let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(&path);

            // Drag a house from the asset browser onto the ground in the middle.
            let middle = screen(&ctx, Vec3::ZERO);
            egui::DragAndDrop::set_payload(&egui, AssetDrag::Mesh("models/house.glb".into()));
            frame(&egui, &mut p, &mut ctx, vec![pointer(middle), button(middle, true)]);
            egui::DragAndDrop::set_payload(&egui, AssetDrag::Mesh("models/house.glb".into()));
            frame(&egui, &mut p, &mut ctx, vec![button(middle, false)]);
            let house = p.selected.expect("the dropped house is selected");
            assert!(ctx.world.get::<crate::render::Model>(house).is_some());
            assert!(
                ctx.world.get::<T3>(house).unwrap().position.length() < 1e-3,
                "on the ground under the mouse"
            );

            // Drag its X arrow two units along, with snapping on.
            p.scene.gizmo.snap.on = true;
            p.scene.gizmo.snap.step = 1.0;
            frame(&egui, &mut p, &mut ctx, vec![]);
            let on_arrow = {
                let l = crate::edit3d::layout(
                    &ctx.scene3d.camera,
                    crate::edit3d::Mode::Translate,
                    Vec3::ZERO,
                    crate::edit3d::axes(crate::edit3d::Space::World, glam::Quat::IDENTITY),
                )
                .unwrap();
                let at = l.center.lerp(l.tips[0].unwrap(), 0.7);
                egui::pos2(at.x, at.y)
            };
            let along = crate::edit3d::closest_on_axis(
                &crate::edit3d::Ray::from_screen(&ctx.scene3d.camera, Vec2::new(on_arrow.x, on_arrow.y)),
                Vec3::ZERO,
                Vec3::X,
            )
            .unwrap();
            let target = screen(&ctx, vec3(along + 2.2, 0.0, 0.0));
            drag(&egui, &mut p, &mut ctx, on_arrow, target);
            assert_eq!(p.selected, Some(house), "a gizmo drag doesn't change the selection");
            let moved = ctx.world.get::<T3>(house).unwrap().position;
            assert!(
                (moved - vec3(2.0, 0.0, 0.0)).length() < 1e-3,
                "{moved}: snapped to the grid"
            );

            // A second house, made a child of the first in the hierarchy, then
            // the first turned a quarter with the rotate gizmo's keys and numbers.
            let second = p
                .place_asset(
                    &mut ctx,
                    &AssetDrag::Mesh("models/house.glb".into()),
                    Vec2::new(middle.x, middle.y),
                )
                .unwrap();
            p.reparent(&mut ctx, second, Some(house));
            ctx.edit(|ed, ctx| ed.set_transforms3d(ctx, &[(house, T3::at(vec3(2.0, 0.0, -3.0)))]));
            assert!((ctx.world.get::<T3>(second).unwrap().position - vec3(0.0, 0.0, -3.0)).length() < 1e-3);

            // Undo the last move, redo it, and save to a scene file.
            frame(&egui, &mut p, &mut ctx, vec![key(Key::Z, true)]);
            assert!(
                (ctx.world.get::<T3>(second).unwrap().position).length() < 1e-3,
                "undo moved both back"
            );
            frame(&egui, &mut p, &mut ctx, vec![key(Key::Y, true)]);
            p.save_scene(&mut ctx, &path);
            let before = ctx.world.save();
            let reloaded = ctx.load_scene(&path);
            let _ = std::fs::remove_file(&file);
            assert_eq!(p.error, None);
            assert_eq!(
                ctx.scene_file.as_deref(),
                Some(path.as_str()),
                "Save now writes there by default"
            );
            reloaded.unwrap();
            assert_eq!(before["entities"], ctx.world.save()["entities"]);
            frame(&egui, &mut p, &mut ctx, vec![]);
            assert_eq!(p.selected, None, "the load cleared the selection");
        }

        #[test]
        fn clicking_picks_the_model_under_the_mouse_and_f_frames_it() {
            let (egui, mut p, mut ctx) = scene();
            let near = ctx.world.spawn((T3::at(vec3(-2.0, 0.0, 0.0)),));
            let far = ctx.world.spawn((T3::at(vec3(3.0, 0.0, -4.0)),));
            let at = screen(&ctx, vec3(3.0, 0.0, -4.0));
            frame(&egui, &mut p, &mut ctx, vec![pointer(at), button(at, true)]);
            frame(&egui, &mut p, &mut ctx, vec![button(at, false)]);
            assert_eq!(p.selected, Some(far));
            let shift_click = |at| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::SHIFT,
            };
            let at = screen(&ctx, vec3(-2.0, 0.0, 0.0));
            frame(
                &egui,
                &mut p,
                &mut ctx,
                vec![
                    egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
                    pointer(at),
                    shift_click(at),
                ],
            );
            frame(&egui, &mut p, &mut ctx, vec![button(at, false)]);
            assert_eq!(p.selection(), vec![near, far]);
            frame(&egui, &mut p, &mut ctx, vec![key(Key::F, false)]);
            let mid = screen(&ctx, vec3(0.5, 0.0, -2.0));
            assert!(
                (mid.x - 640.0).abs() < 1.0 && (mid.y - 360.0).abs() < 1.0,
                "framed: {mid:?}"
            );
            frame(&egui, &mut p, &mut ctx, vec![key(Key::E, false)]);
            assert_eq!(p.scene.gizmo.mode, crate::edit3d::Mode::Rotate);
        }

        #[test]
        fn right_drag_and_keys_fly_the_camera_and_the_game_never_sees_them() {
            let (egui, mut p, mut ctx) = scene();
            let start = ctx.scene3d.camera;
            let at = egui::pos2(300.0, 300.0);
            let right = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Default::default(),
            };
            frame(&egui, &mut p, &mut ctx, vec![pointer(at), right(true)]);
            frame(&egui, &mut p, &mut ctx, vec![pointer(egui::pos2(400.0, 300.0))]);
            assert!(p.scene.flying);
            assert!(
                ctx.scene3d.camera.forward().angle_between(start.forward()) > 0.2,
                "it turned"
            );
            frame(&egui, &mut p, &mut ctx, vec![right(false)]);
            assert!(!p.scene.flying);
            assert!(p.scene.claims_key(winit::keyboard::KeyCode::KeyW, false));
            assert!(!p.scene.claims_key(winit::keyboard::KeyCode::KeyP, false));
        }
    }
}
