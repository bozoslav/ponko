//! The runtime inspector: an egui overlay, toggled with the
//! backquote key (`` ` ``, left of 1), that shows the
//! running game. Sim controls (pause, step, speed), frame and system times,
//! every entity with its components, live editing of reflected components
//! and resources, and picking units in the world.
//!
//! Paused, it's an editor ([`crate::editor`]): select, drag, duplicate and
//! delete entities in the world, add and remove components, undo and redo,
//! place prefabs, save and load scenes, and Play / Stop. A hierarchy panel
//! parents entities by drag and drop, and an asset browser drags meshes,
//! prefabs and materials into the world. In 3D, a click picks along the
//! mouse ray and a gizmo moves, turns and scales the selection.
//!
//! Built with the `inspector` feature; turned on per game with
//! [`Game::inspector`](crate::game::Game::inspector) (on in debug builds). While
//! closed, it costs nothing: egui doesn't run and no events are kept.

use std::collections::HashSet;

use egui::{
    Button, CollapsingHeader, Color32, DragValue, Key, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, vec2,
};
use glam::Vec2;
use ponko_ecs::{ComponentId, Entity, World};
use serde_json::Value;

use crate::d2::Sprite;
use crate::d2::Transform as Transform2D;
use crate::reflect::short_name;
use crate::{camera::Rect, ctx::Ctx, reflect::WorldReflect};

#[path = "inspector_scene.rs"]
mod scene;

/// Environment variable that turns the inspector on (`1`) or off (`0`),
/// overriding [`Game::inspector`](crate::game::Game::inspector).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) const ENV_INSPECTOR: &str = "PONKO_INSPECTOR";

/// The key that opens and closes the inspector.
pub(crate) const TOGGLE_CODE: winit::keyboard::KeyCode = winit::keyboard::KeyCode::Backquote;

/// Whether the inspector is on, and whether it starts open:
/// `PONKO_INSPECTOR` (`1`, `0` or `open`) if set, else the game's choice.
/// In web builds, the page's `?inspector` (`=1`, `=0`, `=open`) instead.
pub(crate) fn enabled(game_default: bool) -> (bool, bool) {
    #[cfg(target_arch = "wasm32")]
    let setting = crate::web::query_param("inspector").map(|v| if v.is_empty() { "1".into() } else { v });
    #[cfg(not(target_arch = "wasm32"))]
    let setting = std::env::var(ENV_INSPECTOR).ok();
    match setting.as_deref() {
        Some("1") => (true, false),
        Some("open") => (true, true),
        Some("0") => (false, false),
        _ => (game_default, false),
    }
}

/// Frames kept for the frame-time graph.
const HISTORY: usize = 240;
/// How close (window pixels) a click must be to an entity to pick it.
const PICK_RADIUS: f32 = 24.0;
/// Height of one row in the entity list.
const ROW_HEIGHT: f32 = 18.0;
/// Where Ctrl+D puts the copy, from the original (world units).
const DUPLICATE_OFFSET: Vec2 = Vec2::new(16.0, 16.0);
/// Folder of `assets/` the prefab palette lists.
const PREFAB_FOLDER: &str = "prefabs";
/// Scene file used when none is typed.
const DEFAULT_SCENE: &str = "levels/level.json";

/// What the overlay draws this frame: egui's triangles and texture changes.
pub(crate) struct Overlay {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures: egui::TexturesDelta,
    pub pixels_per_point: f32,
}

impl Overlay {
    /// Adds a frame that wasn't drawn yet: its texture changes still apply.
    pub fn merge_older(&mut self, mut older: Overlay) {
        let mut textures = std::mem::take(&mut older.textures);
        textures.append(std::mem::take(&mut self.textures));
        self.textures = textures;
    }
}

impl Drop for Overlay {
    /// A frame that's never drawn (the game quits first) is fine to drop:
    /// its textures die with the renderer.
    fn drop(&mut self) {
        self.textures.clear();
    }
}

/// The inspector's panels and what's selected in them. Independent of the
/// window, so it can be run (and tested) on any egui context.
#[derive(Default)]
pub(crate) struct Panels {
    pub selected: Option<Entity>,
    /// The rest of a multi-selection (Shift-click), without `selected`.
    others: Vec<Entity>,
    /// Hierarchy, asset browser, and the 3D view's gizmo and camera.
    scene: scene::SceneEdit,
    /// Only list entities with this component.
    filter: Option<ComponentId>,
    /// The next click in the world selects the entity under it.
    pub picking: bool,
    /// A click to pick at (window pixels), from the window events.
    pub pick_at: Option<Vec2>,
    /// The last edit that failed, shown until the next one.
    error: Option<String>,
    frame_ms: Vec<f32>,
    frame_at: usize,
    /// Non-empty archetypes that pass the filter, and the first row of each.
    rows: Vec<(usize, usize)>,
    reflected: HashSet<String>,
    /// Paused with the panels open: clicks and shortcut keys in the world
    /// go to the editor, not the game.
    pub editing: bool,
    /// The entity being dragged in the world.
    drag: Option<Drag>,
    /// A prefab the next click in the world places.
    placing: Option<String>,
    /// Prefab files in `assets/prefabs/`; `None` until listed.
    prefabs: Option<Vec<String>>,
    scene_path: String,
    prefab_path: String,
    /// What the last scene or prefab action did.
    status: Option<String>,
    /// `Ctx::scene_loads` when the panels last ran: a load since then
    /// replaced the entities, so the selection may name another one.
    scene_loads: u64,
}

/// An entity being moved with the mouse.
struct Drag {
    e: Entity,
    /// Its transform before, for undo.
    before: Value,
    /// From the mouse to its position, so it doesn't jump to the cursor.
    grab: Vec2,
    moved: bool,
}

impl Panels {
    /// Draws the inspector into `ui` and applies what was changed in it to `ctx`.
    pub fn show(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        self.editing = ctx.time.is_paused();
        self.record_frame(ctx.time.dt() * 1000.0);
        self.reflected.clear();
        self.reflected
            .extend(ctx.world.reflected_components().into_iter().map(str::to_owned));
        if self.scene_loads != ctx.scene_loads {
            self.scene_loads = ctx.scene_loads;
            self.select(None, false);
            self.drag = None;
            self.scene.forget_drag();
        }
        if self.selected.is_some_and(|e| !ctx.world.is_alive(e)) {
            self.selected = None;
        }
        self.scene_frame(ctx);
        if let Some(at) = self.pick_at.take() {
            self.picking = false;
            if let Some(e) = self.pick_any(ctx, at) {
                self.select(Some(e), false);
            }
        }

        egui::Panel::right("ponko-inspector")
            .default_size(360.0)
            .resizable(true)
            .show(ui, |ui| {
                ScrollArea::vertical().id_salt("inspector-scroll").show(ui, |ui| {
                    self.sim_controls(ui, ctx);
                    self.frame_graph(ui);
                    if let Some(error) = &self.error {
                        ui.colored_label(Color32::LIGHT_RED, error);
                    }
                    CollapsingHeader::new("Edit")
                        .default_open(true)
                        .show(ui, |ui| self.edit_tools(ui, ctx));
                    if let Some(e) = self.selected {
                        ui.separator();
                        self.entity(ui, ctx, e);
                        ui.separator();
                    }
                    CollapsingHeader::new("Hierarchy")
                        .default_open(ctx.world.len() <= scene::HIERARCHY_OPEN_UP_TO)
                        .show(ui, |ui| self.hierarchy_panel(ui, ctx));
                    CollapsingHeader::new("Assets")
                        .default_open(false)
                        .show(ui, |ui| self.asset_browser(ui, ctx));
                    CollapsingHeader::new("Entities")
                        .default_open(true)
                        .show(ui, |ui| self.entity_list(ui, ctx));
                    if !ctx.world.reflected_resources().is_empty() {
                        CollapsingHeader::new("Resources")
                            .default_open(true)
                            .show(ui, |ui| self.resources(ui, ctx));
                    }
                    CollapsingHeader::new("Systems")
                        .default_open(true)
                        .show(ui, |ui| systems(ui, ctx));
                });
            });
        if self.editing {
            if self.scene.active3d {
                self.world_input_3d(ui, ctx);
            } else {
                self.world_input(ui, ctx);
            }
            self.shortcuts(ui, ctx);
        } else {
            if let Some(drag) = self.drag.take() {
                Self::end_drag(ctx, drag);
            }
            self.end_gizmo_drag(ctx);
        }
        if self.scene.active3d {
            self.highlight_3d(ui, ctx);
        } else {
            self.highlight(ui, ctx);
        }
    }

    /// Shows an edit's error, or forgets the last one.
    fn report<T, E: Into<String>>(&mut self, result: Result<T, E>) -> Option<T> {
        match result {
            Ok(v) => {
                self.error = None;
                Some(v)
            }
            Err(e) => {
                self.error = Some(e.into());
                None
            }
        }
    }

    /// Mouse in the world while paused: click selects (or places the armed
    /// prefab), drag moves, right or middle drag pans, the wheel zooms.
    fn world_input(&mut self, ui: &Ui, ctx: &mut Ctx) {
        let (pos, pressed, down, pan, scroll, shift) = ui.input(|i| {
            let p = &i.pointer;
            let pan = if p.secondary_down() || p.middle_down() {
                p.delta()
            } else {
                egui::Vec2::ZERO
            };
            let pos = p.latest_pos().map(|p| Vec2::new(p.x, p.y));
            (
                pos,
                p.primary_pressed(),
                p.primary_down(),
                pan,
                i.smooth_scroll_delta.y,
                i.modifiers.shift,
            )
        });
        let Some(pos) = pos else { return };
        if self.drop_asset(ui, ctx, pos) {
            return;
        }
        if !ui.ctx().is_pointer_over_egui() {
            if pan != egui::Vec2::ZERO {
                ctx.camera.position -= Vec2::new(pan.x, pan.y) / ctx.camera.zoom;
            }
            if scroll != 0.0 {
                ctx.camera.zoom_at(pos, 1.1f32.powf(scroll / 50.0));
            }
            if pressed {
                let add = ui.input(|i| i.modifiers.command);
                self.click(ctx, pos, shift, add);
            }
        }
        if let Some(drag) = &mut self.drag {
            if down && ctx.world.is_alive(drag.e) {
                let to = ctx.camera.screen_to_world(pos) + drag.grab;
                if let Some(t) = ctx.world.get_mut::<Transform2D>(drag.e)
                    && t.position != to
                {
                    t.position = to;
                    drag.moved = true;
                }
            } else if let Some(drag) = self.drag.take() {
                Self::end_drag(ctx, drag);
            }
        }
    }

    /// A click in the world at window point `at`.
    fn click(&mut self, ctx: &mut Ctx, at: Vec2, keep_placing: bool, add: bool) {
        let world_at = ctx.camera.screen_to_world(at);
        if let Some(path) = self.placing.clone() {
            let placed = ctx.edit(|ed, ctx| ed.spawn_prefab(ctx, &path, world_at));
            if let Some(e) = self.report(placed) {
                self.select(Some(e), false);
            }
            if !keep_placing {
                self.placing = None;
            }
            return;
        }
        let picked = pick(ctx, at);
        // Ctrl-click (or Shift-click on an entity) adds to the selection.
        let add = add || (keep_placing && picked.is_some());
        self.select(picked, add);
        if add {
            return;
        }
        if let Some(e) = self.selected
            && let Some(t) = ctx.world.get::<Transform2D>(e)
        {
            let grab = t.position - world_at;
            if let Ok(before) = ctx.world.get_json(e, "Transform2D") {
                self.drag = Some(Drag {
                    e,
                    before,
                    grab,
                    moved: false,
                });
            }
        }
    }

    fn end_drag(ctx: &mut Ctx, drag: Drag) {
        if drag.moved {
            ctx.edit(|ed, _| ed.moved(drag.e, drag.before));
        }
    }

    /// Keys while paused: Delete, Ctrl+D (duplicate), Ctrl+Z (undo),
    /// Ctrl+Shift+Z or Ctrl+Y (redo), Esc; in 3D also W, E, R and F.
    fn shortcuts(&mut self, ui: &Ui, ctx: &mut Ctx) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if self.selection_keys(ui, ctx) {
            return;
        }
        if self.scene.active3d {
            self.keys_3d(ui, ctx);
        }
        let (delete, duplicate, undo, escape) = ui.input(|i| {
            (
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                i.modifiers.command && i.key_pressed(Key::D),
                i.modifiers.command && !i.modifiers.shift && i.key_pressed(Key::Z),
                i.key_pressed(Key::Escape),
            )
        });
        if escape {
            self.placing = None;
        }
        if undo {
            self.undo(ctx);
        }
        let Some(e) = self.selected else { return };
        if delete {
            let deleted = ctx.edit(|ed, ctx| ed.delete(ctx, e));
            self.report(deleted);
            self.select(None, false);
        } else if duplicate {
            let copy = ctx.edit(|ed, ctx| ed.duplicate(ctx, e, DUPLICATE_OFFSET));
            if let Some(copy) = self.report(copy) {
                self.select(Some(copy), false);
            }
        }
    }

    fn undo(&mut self, ctx: &mut Ctx) {
        let undone = ctx.edit(|ed, ctx| ed.undo(ctx));
        self.report(undone);
    }

    /// Undo, new entity, scenes, and the prefab palette.
    fn edit_tools(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        ui.horizontal(|ui| {
            if ui.add_enabled(ctx.editor.can_undo(), Button::new("Undo")).clicked() {
                self.undo(ctx);
            }
            if ui
                .button("New entity")
                .on_hover_text("An entity with only a transform, mid-screen")
                .clicked()
            {
                let at = ctx.camera.center();
                let e = ctx.edit(|ed, ctx| ed.spawn_empty(ctx, at));
                self.select(Some(e), false);
            }
        });
        self.scene_tools(ui, ctx);
        if !self.editing {
            ui.label(RichText::new("Pause to edit in the world").weak());
        } else {
            ui.label(
                RichText::new(
                    "Click selects (Ctrl adds), drag moves, right-drag pans, wheel zooms. Del, Ctrl+D, Ctrl+Z.",
                )
                .weak(),
            );
        }

        ui.horizontal(|ui| {
            ui.label("Scene");
            // An empty field means the scene loaded last, else the default.
            let current = ctx.scene_file.clone().unwrap_or_else(|| DEFAULT_SCENE.to_owned());
            ui.add(
                TextEdit::singleline(&mut self.scene_path)
                    .hint_text(&current)
                    .desired_width(150.0),
            );
            let path = if self.scene_path.is_empty() {
                current
            } else {
                self.scene_path.clone()
            };
            if ui.button("Save").clicked() {
                self.save_scene(ctx, &path);
            }
            if ui
                .button("Load")
                .on_hover_text("Replaces the world; clears undo")
                .clicked()
            {
                let loaded = ctx.load_scene(&path).map(|()| format!("loaded {path}"));
                if let Some(done) = self.report(loaded) {
                    self.status = Some(done);
                    self.select(None, false);
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Prefab");
            let hint = format!("{PREFAB_FOLDER}/thing.json");
            ui.add(
                TextEdit::singleline(&mut self.prefab_path)
                    .hint_text(hint)
                    .desired_width(150.0),
            );
            let save = ui.add_enabled(
                self.selected.is_some() && !self.prefab_path.is_empty(),
                Button::new("Save"),
            );
            if let Some(e) = self.selected
                && save.on_hover_text("Saves the selected entity as this prefab").clicked()
            {
                let path = self.prefab_path.clone();
                let saved = ctx
                    .assets
                    .save_prefab(&ctx.world, e, &path)
                    .map_err(String::from)
                    .map(|()| format!("saved {path}"));
                self.status = self.report(saved);
                self.prefabs = None;
                self.scene.forget_assets();
            }
        });
        let prefabs = self
            .prefabs
            .get_or_insert_with(|| crate::assets::list(PREFAB_FOLDER, "json"))
            .clone();
        ui.horizontal_wrapped(|ui| {
            for path in &prefabs {
                let name = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".json");
                let armed = self.placing.as_deref() == Some(path.as_str());
                let tip = "Click, then click in the world to place it (hold Shift to place more)";
                if ui.selectable_label(armed, name).on_hover_text(tip).clicked() {
                    self.placing = if armed { None } else { Some(path.clone()) };
                }
            }
            if ui
                .small_button("⟳")
                .on_hover_text(format!("List assets/{PREFAB_FOLDER}/ again"))
                .clicked()
            {
                self.prefabs = None;
            }
        });
        if let Some(status) = &self.status {
            ui.label(RichText::new(status).weak());
        }
    }

    fn record_frame(&mut self, ms: f32) {
        if self.frame_ms.len() < HISTORY {
            self.frame_ms.push(ms);
        } else {
            self.frame_ms[self.frame_at] = ms;
        }
        self.frame_at = (self.frame_at + 1) % HISTORY;
    }

    fn sim_controls(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        ui.horizontal(|ui| {
            if ctx.time.is_paused() {
                let tip = if ctx.editor.playing() {
                    "Resume"
                } else {
                    "Run the game; Stop puts the world back as it is now"
                };
                if ui.button("Play").on_hover_text(tip).clicked() {
                    ctx.edit(|ed, ctx| ed.play(ctx));
                }
            } else if ui.button("Pause").clicked() {
                ctx.time.pause();
            }
            if ctx.editor.playing() {
                let stop = ui
                    .button("Stop")
                    .on_hover_text("Pause and put the world back as it was at Play");
                if stop.clicked() {
                    let stopped = ctx.edit(|ed, ctx| ed.stop(ctx));
                    self.report(stopped);
                    self.drag = None;
                }
                if ui
                    .button("Keep")
                    .on_hover_text("Keep the world as it is: Stop won't go back")
                    .clicked()
                {
                    ctx.edit(|ed, _| ed.keep());
                }
            }
            if ui.button("Step").on_hover_text("Run one tick (pauses)").clicked() {
                ctx.time.pause();
                ctx.time.step();
            }
            let mut speed = ctx.time.speed();
            let drag = DragValue::new(&mut speed).range(0.0..=16.0).speed(0.02).suffix("×");
            if ui.add(drag).on_hover_text("Simulation speed").changed() {
                ctx.time.set_speed(speed);
            }
        });
        ui.label(format!(
            "tick {}   {:.0} fps   {} entities",
            ctx.time.sim_tick(),
            ctx.time.fps(),
            ctx.world.len()
        ));
    }

    /// The last frames' times as bars; the line marks 60 fps.
    fn frame_graph(&self, ui: &mut Ui) {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 2.0, Color32::from_black_alpha(80));
        let target = 1000.0 / 60.0;
        // Scaled to the slowest frame, but at most 4× the target, so one
        // long frame (loading) doesn't flatten the rest.
        let top = self
            .frame_ms
            .iter()
            .copied()
            .fold(target * 2.0, f32::max)
            .min(target * 4.0);
        let bar = rect.width() / HISTORY as f32;
        let n = self.frame_ms.len();
        for i in 0..n {
            // Oldest on the left.
            let ms = if n < HISTORY {
                self.frame_ms[i]
            } else {
                self.frame_ms[(self.frame_at + i) % HISTORY]
            };
            let h = rect.height() * (ms / top).min(1.0);
            let x = rect.left() + i as f32 * bar;
            let color = if ms > target * 1.5 {
                Color32::from_rgb(230, 90, 70)
            } else {
                Color32::from_rgb(90, 180, 120)
            };
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(x, rect.bottom() - h), egui::pos2(x + bar, rect.bottom())),
                0.0,
                color,
            );
        }
        let y = rect.bottom() - rect.height() * target / top;
        painter.hline(rect.x_range(), y, Stroke::new(1.0, Color32::from_white_alpha(60)));
        if n > 0 {
            let last = self.frame_ms[(self.frame_at + HISTORY - 1) % HISTORY];
            response.on_hover_text(format!("frame time; last {last:.2} ms, line at 60 fps"));
        }
    }

    fn resources(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        let names: Vec<String> = ctx.world.reflected_resources().into_iter().map(str::to_owned).collect();
        for name in names {
            let Ok(mut value) = ctx.world.resource_json(&name) else {
                ui.label(RichText::new(format!("{name} (not inserted)")).weak());
                continue;
            };
            CollapsingHeader::new(&name)
                .id_salt(("resource", &name))
                .show(ui, |ui| {
                    if edit_value(ui, &mut value) {
                        self.error = ctx.world.set_resource_json(&name, value).err().map(|e| e.to_string());
                    }
                });
        }
    }

    fn entity_list(&mut self, ui: &mut Ui, ctx: &mut Ctx) {
        let world = &ctx.world;
        ui.horizontal(|ui| {
            ui.label("With");
            let label = self
                .filter
                .map_or("anything".to_owned(), |id| component_name(world, id));
            egui::ComboBox::from_id_salt("inspector-filter")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter, None, "anything");
                    for (i, info) in world.components().iter().enumerate() {
                        let Some(id) = world.component_at(i) else { continue };
                        ui.selectable_value(&mut self.filter, Some(id), short_name(info.name()));
                    }
                });
            let pick = ui
                .selectable_label(self.picking, "Pick")
                .on_hover_text("Click a unit in the world to select it");
            if pick.clicked() {
                self.picking = !self.picking;
            }
        });
        let total = list_rows(world, self.filter, &mut self.rows);
        ui.label(RichText::new(format!("{total} matching")).weak());
        let mut clicked = None;
        ScrollArea::vertical()
            .id_salt("inspector-entities")
            .max_height(220.0)
            .auto_shrink([false, true])
            .show_rows(ui, ROW_HEIGHT, total, |ui, range| {
                for row in range {
                    let Some(e) = entity_at(world, &self.rows, row) else {
                        continue;
                    };
                    let text = format!("{e:?}  {}", components_label(world, e));
                    let selected = self.selected == Some(e) || self.others.contains(&e);
                    if ui.selectable_label(selected, text).clicked() {
                        clicked = Some(e);
                    }
                }
            });
        if clicked.is_some() {
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            self.select(clicked, add);
        }
    }

    fn entity(&mut self, ui: &mut Ui, ctx: &mut Ctx, e: Entity) {
        let position = ctx.world.get::<Transform2D>(e).map(|t| t.position);
        ui.horizontal(|ui| {
            ui.heading(format!("{e:?}"));
            if let Some(at) = position
                && ui.button("Look at").clicked()
            {
                ctx.camera.look_at(at);
            }
            if ui.button("Duplicate").on_hover_text("Ctrl+D").clicked() {
                let copy = ctx.edit(|ed, ctx| ed.duplicate(ctx, e, DUPLICATE_OFFSET));
                if let Some(copy) = self.report(copy) {
                    self.select(Some(copy), false);
                }
            }
            if ui.button("Despawn").on_hover_text("Del").clicked() {
                let deleted = ctx.edit(|ed, ctx| ed.delete(ctx, e));
                self.report(deleted);
                self.select(None, false);
            }
        });
        if self.selected.is_none() {
            return;
        }
        let ids: Vec<ComponentId> = ctx.world.components_of(e).map(<[_]>::to_vec).unwrap_or_default();
        let has: HashSet<String> = ids.iter().map(|&id| component_name(&ctx.world, id)).collect();
        for id in ids {
            let name = component_name(&ctx.world, id);
            if !self.reflected.contains(&name) {
                ui.label(RichText::new(format!("{name} (not reflected)")).weak())
                    .on_hover_text("world.reflect::<T>() makes it readable and editable here");
                continue;
            }
            let Ok(mut value) = ctx.world.get_json(e, &name) else {
                continue;
            };
            let mut remove = false;
            CollapsingHeader::new(&name)
                .id_salt(("component", &name))
                .default_open(true)
                .show(ui, |ui| {
                    if edit_value(ui, &mut value) {
                        let set = ctx.edit(|ed, ctx| ed.set(ctx, e, &name, value));
                        self.report(set);
                    }
                    remove = ui.small_button("Remove").clicked();
                });
            if remove {
                let removed = ctx.edit(|ed, ctx| ed.remove(ctx, e, &name));
                self.report(removed);
            }
        }
        let mut addable: Vec<&String> = self.reflected.iter().filter(|n| !has.contains(*n)).collect();
        addable.sort();
        let mut add = None;
        egui::ComboBox::from_id_salt("inspector-add")
            .selected_text("Add component")
            .show_ui(ui, |ui| {
                for name in addable {
                    if ui.selectable_label(false, name).clicked() {
                        add = Some(name.clone());
                    }
                }
            });
        if let Some(name) = add {
            let added = ctx.edit(|ed, ctx| ed.add(ctx, e, &name));
            self.report(added);
        }
    }

    /// A ring around each selected entity, in the world (the primary one
    /// stronger).
    fn highlight(&self, ui: &Ui, ctx: &Ctx) {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::unique("inspector-highlight"),
        ));
        for (i, e) in self.selection().into_iter().enumerate() {
            let Some(t) = ctx.world.get::<Transform2D>(e) else {
                continue;
            };
            let at = ctx.camera.world_to_screen(t.position);
            let center = egui::pos2(at.x, at.y);
            let width = if i == 0 { 2.0 } else { 1.0 };
            painter.circle_stroke(center, 14.0, Stroke::new(width, Color32::from_rgb(255, 200, 40)));
            painter.circle_stroke(center, 16.0, Stroke::new(1.0, Color32::from_black_alpha(160)));
        }
    }
}

/// Short name of component `id`, as reflection and the control endpoint name it.
fn component_name(world: &World, id: ComponentId) -> String {
    short_name(world.component_info(id).name())
}

/// The components of `e`, comma separated.
fn components_label(world: &World, e: Entity) -> String {
    let Some(ids) = world.components_of(e) else {
        return String::new();
    };
    let mut out = String::new();
    for (i, &id) in ids.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&component_name(world, id));
    }
    out
}

/// Fills `rows` with each non-empty archetype that has `filter` and the row
/// its entities start at. Returns the number of rows.
fn list_rows(world: &World, filter: Option<ComponentId>, rows: &mut Vec<(usize, usize)>) -> usize {
    rows.clear();
    let mut total = 0;
    for (i, arch) in world.raw_ref().archetypes().iter().enumerate() {
        if arch.is_empty() || filter.is_some_and(|id| !arch.has(id)) {
            continue;
        }
        rows.push((i, total));
        total += arch.len();
    }
    total
}

/// The entity on `row` of a list made by [`list_rows`].
fn entity_at(world: &World, rows: &[(usize, usize)], row: usize) -> Option<Entity> {
    let k = rows.partition_point(|&(_, start)| start <= row).checked_sub(1)?;
    let (arch, start) = rows[k];
    world.raw_ref().archetypes()[arch].entities().get(row - start).copied()
}

/// The entity under window point `at`: the top sprite there (by `z`), else
/// the entity with a `Transform2D` closest to it, within [`PICK_RADIUS`]
/// pixels. Sprites are hit by their unrotated rectangle.
fn pick(ctx: &mut Ctx, at: Vec2) -> Option<Entity> {
    let camera = ctx.camera;
    let world_at = camera.screen_to_world(at);
    let mut top: Option<(f32, Entity)> = None;
    for (e, t, s) in ctx.world.query::<(Entity, &Transform2D, &Sprite)>().iter() {
        let size = s.region.map_or(s.texture.size(), |r| r.size()) * t.scale.abs();
        let min = t.position - size * s.pivot;
        let rect = Rect { min, max: min + size };
        if rect.contains(world_at) && top.is_none_or(|(z, _)| s.z >= z) {
            top = Some((s.z, e));
        }
    }
    if let Some((_, e)) = top {
        return Some(e);
    }
    let mut best: Option<(f32, Entity)> = None;
    for (e, t) in ctx.world.query::<(Entity, &Transform2D)>().iter() {
        let d = camera.world_to_screen(t.position).distance_squared(at);
        if d <= PICK_RADIUS * PICK_RADIUS && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, e));
        }
    }
    best.map(|(_, e)| e)
}

/// Edits a JSON value in place with the widget that fits it. Returns whether it changed.
fn edit_value(ui: &mut Ui, value: &mut Value) -> bool {
    match value {
        Value::Null => {
            ui.label(RichText::new("none").weak());
            false
        }
        Value::Bool(b) => ui.checkbox(b, "").changed(),
        Value::Number(n) => edit_number(ui, n),
        Value::String(s) => {
            // Applied when editing ends, not per key: a half-typed enum
            // variant ("Blu") would be refused and overwritten.
            let id = ui.next_auto_id();
            let mut draft = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| s.clone());
            let response = ui.text_edit_singleline(&mut draft);
            if response.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, draft));
                false
            } else {
                ui.data_mut(|d| d.remove::<String>(id));
                let changed = response.lost_focus() && draft != *s;
                if changed {
                    *s = draft;
                }
                changed
            }
        }
        Value::Array(items) if items.len() <= 4 && items.iter().all(Value::is_number) => {
            // Vectors and colors: one row.
            let mut changed = false;
            ui.horizontal(|ui| {
                for item in items.iter_mut() {
                    if let Value::Number(n) = item {
                        changed |= edit_number(ui, n);
                    }
                }
            });
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            CollapsingHeader::new(format!("[{}]", items.len()))
                .id_salt(ui.next_auto_id())
                .show(ui, |ui| {
                    for (i, item) in items.iter_mut().enumerate() {
                        ui.push_id(i, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(i.to_string());
                                changed |= edit_value(ui, item);
                            });
                        });
                    }
                });
            changed
        }
        Value::Object(fields) => {
            let mut changed = false;
            egui::Grid::new(ui.next_auto_id())
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (key, field) in fields.iter_mut() {
                        ui.label(key);
                        ui.push_id(key, |ui| changed |= edit_value(ui, field));
                        ui.end_row();
                    }
                });
            changed
        }
    }
}

/// Keeps the number's kind: unsigned stays unsigned, integers stay integers.
fn edit_number(ui: &mut Ui, n: &mut serde_json::Number) -> bool {
    if let Some(mut v) = n.as_u64() {
        let changed = ui.add(DragValue::new(&mut v)).changed();
        *n = v.into();
        changed
    } else if let Some(mut v) = n.as_i64() {
        let changed = ui.add(DragValue::new(&mut v)).changed();
        *n = v.into();
        changed
    } else {
        let mut v = n.as_f64().unwrap_or(0.0);
        let speed = (v.abs() * 0.01).clamp(0.001, 10.0);
        let changed = ui.add(DragValue::new(&mut v).speed(speed)).changed();
        if let Some(f) = serde_json::Number::from_f64(v) {
            *n = f;
        }
        changed
    }
}

/// Each system's time this frame, slowest first.
fn systems(ui: &mut Ui, ctx: &Ctx) {
    let timings = ctx.perf.system_timings();
    if timings.is_empty() {
        ui.label(RichText::new("no systems").weak());
        return;
    }
    let top = timings.iter().map(|t| t.avg_ms).fold(0.001, f32::max);
    egui::Grid::new("inspector-systems").num_columns(3).show(ui, |ui| {
        for t in timings {
            let name = t.name.rsplit("::").next().unwrap_or(t.name);
            ui.label(if t.sim {
                format!("{name} (sim)")
            } else {
                name.to_owned()
            })
            .on_hover_text(t.name);
            ui.label(format!("{:.3} ms", t.avg_ms));
            let (rect, _) = ui.allocate_exact_size(vec2(60.0, 8.0), Sense::hover());
            let mut bar = rect;
            bar.set_width(rect.width() * t.avg_ms / top);
            ui.painter().rect_filled(bar, 1.0, Color32::from_rgb(242, 140, 40));
            ui.end_row();
        }
    });
}

/// The overlay as part of the windowed loop: egui's context and window
/// input, around [`Panels`].
pub(crate) struct Inspector {
    pub open: bool,
    egui: egui::Context,
    input: egui_winit::State,
    pub panels: Panels,
}

impl Inspector {
    pub fn new(window: &winit::window::Window, max_texture_side: usize) -> Self {
        let egui = egui::Context::default();
        let input = egui_winit::State::new(
            egui.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(max_texture_side),
        );
        Self {
            open: false,
            egui,
            input,
            panels: Panels::default(),
        }
    }

    /// Feeds a window event to egui. Returns `true` if the game shouldn't
    /// see it: egui uses it, or it's the click that picks an entity.
    pub fn on_event(&mut self, window: &winit::window::Window, event: &winit::event::WindowEvent) -> bool {
        use winit::event::{ElementState, MouseButton, WindowEvent};
        if !self.open {
            return false;
        }
        let consumed = self.input.on_window_event(window, event).consumed;
        if self.panels.picking
            && !self.egui.is_pointer_over_egui()
            && let WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } = event
        {
            let at = self.egui.input(|i| i.pointer.latest_pos());
            self.panels.pick_at = at.map(|p| Vec2::new(p.x, p.y));
            return true;
        }
        let input = matches!(
            event,
            WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
        );
        if self.panels.editing && input {
            // Paused: the editor has the mouse in the world, and its keys.
            let mouse = !matches!(event, WindowEvent::KeyboardInput { .. });
            return consumed || mouse || self.is_editor_key(event);
        }
        consumed && input
    }

    /// Whether `event` is one of the editor's keys (see `Panels::shortcuts`).
    fn is_editor_key(&self, event: &winit::event::WindowEvent) -> bool {
        use winit::keyboard::{KeyCode, PhysicalKey};
        let winit::event::WindowEvent::KeyboardInput { event: key, .. } = event else {
            return false;
        };
        let PhysicalKey::Code(code) = key.physical_key else {
            return false;
        };
        let command = self.egui.input(|i| i.modifiers.command);
        match code {
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Escape => true,
            KeyCode::KeyD | KeyCode::KeyZ => command,
            _ => self.panels.scene.claims_key(code, command),
        }
    }

    /// Runs the panels for this frame and hands their drawing to the renderer.
    pub fn frame(&mut self, window: &winit::window::Window, ctx: &mut Ctx) {
        ctx.ui.overlay_has_pointer = false;
        if !self.open {
            return;
        }
        let raw = self.input.take_egui_input(window);
        let panels = &mut self.panels;
        let output = self.egui.run_ui(raw, |ui| panels.show(ui, ctx));
        ctx.ui.overlay_has_pointer = self.egui.is_pointer_over_egui();
        self.input.handle_platform_output(window, output.platform_output);
        let primitives = self.egui.tessellate(output.shapes, output.pixels_per_point);
        if let Some(renderer) = ctx.gfx.renderer.as_mut() {
            renderer.set_overlay(Overlay {
                primitives,
                textures: output.textures_delta,
                pixels_per_point: output.pixels_per_point,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ponko_ecs::Component;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Serialize, Deserialize, Component)]
    struct Health {
        current: f32,
        max: u32,
    }
    #[derive(Component)]
    struct Secret;

    fn run(panels: &mut Panels, ctx: &mut Ctx) -> egui::FullOutput {
        let egui = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let mut out = egui.run_ui(input, |ui| panels.show(ui, ctx));
        out.textures_delta.clear();
        out
    }

    fn game() -> (Ctx, Entity) {
        let mut ctx = Ctx::new(1.0 / 60.0, Vec2::new(1280.0, 720.0));
        ctx.world.reflect::<Health>();
        ctx.world.reflect::<Transform2D>();
        let e = ctx.world.spawn((
            Transform2D::at(Vec2::new(10.0, 0.0)),
            Health { current: 3.0, max: 10 },
            Secret,
        ));
        for i in 0..100 {
            ctx.world.spawn((Transform2D::at(Vec2::new(i as f32 * 100.0, 500.0)),));
        }
        (ctx, e)
    }

    /// Runs one frame of `panels` on a lasting egui context (pointer state
    /// carries over between frames) with these input events.
    fn frame(egui: &egui::Context, panels: &mut Panels, ctx: &mut Ctx, mut events: Vec<egui::Event>) {
        // egui keeps held modifiers from ModifiersChanged, not from key events.
        let command = events
            .iter()
            .any(|e| matches!(e, egui::Event::Key { modifiers, .. } if modifiers.command));
        let held = if command {
            egui::Modifiers::COMMAND
        } else {
            Default::default()
        };
        events.insert(0, egui::Event::ModifiersChanged(held));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 720.0))),
            events,
            ..Default::default()
        };
        let mut out = egui.run_ui(input, |ui| panels.show(ui, ctx));
        out.textures_delta.clear();
    }
    fn button(x: f32, y: f32, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(x, y),
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
    fn edit_scene() -> (egui::Context, Panels, Ctx, Entity) {
        let (mut ctx, _) = game();
        ctx.time.pause();
        let e = ctx.world.spawn((
            Transform2D::at(Vec2::new(300.0, 300.0)),
            Health { current: 1.0, max: 1 },
        ));
        let egui = egui::Context::default();
        let mut panels = Panels::default();
        frame(&egui, &mut panels, &mut ctx, vec![]); // lay the panels out once
        (egui, panels, ctx, e)
    }

    #[test]
    fn paused_a_click_selects_a_drag_moves_and_undo_puts_it_back() {
        let (egui, mut panels, mut ctx, e) = edit_scene();
        let at = |ctx: &Ctx| ctx.world.get::<Transform2D>(e).unwrap().position;
        frame(
            &egui,
            &mut panels,
            &mut ctx,
            vec![
                egui::Event::PointerMoved(egui::pos2(305.0, 302.0)),
                button(305.0, 302.0, true),
            ],
        );
        assert_eq!(panels.selected, Some(e));
        frame(
            &egui,
            &mut panels,
            &mut ctx,
            vec![egui::Event::PointerMoved(egui::pos2(355.0, 322.0))],
        );
        assert_eq!(
            at(&ctx),
            Vec2::new(350.0, 320.0),
            "moved by the drag, not snapped to the cursor"
        );
        frame(&egui, &mut panels, &mut ctx, vec![button(355.0, 322.0, false)]);
        assert!(ctx.editor.can_undo());
        frame(&egui, &mut panels, &mut ctx, vec![key(Key::Z, true)]);
        assert_eq!(at(&ctx), Vec2::new(300.0, 300.0));
    }

    #[test]
    fn paused_keys_duplicate_and_delete_the_selection() {
        let (egui, mut panels, mut ctx, e) = edit_scene();
        panels.selected = Some(e);
        let before = ctx.world.len();
        frame(&egui, &mut panels, &mut ctx, vec![key(Key::D, true)]);
        let copy = panels.selected.unwrap();
        assert_ne!(copy, e);
        assert_eq!(
            ctx.world.get::<Transform2D>(copy).unwrap().position,
            Vec2::new(316.0, 316.0)
        );
        frame(&egui, &mut panels, &mut ctx, vec![key(Key::Delete, false)]);
        assert!(!ctx.world.is_alive(copy) && panels.selected.is_none());
        assert_eq!(ctx.world.len(), before);
    }

    #[test]
    fn running_the_world_clicks_go_to_the_game() {
        let (egui, mut panels, mut ctx, e) = edit_scene();
        ctx.time.resume();
        frame(
            &egui,
            &mut panels,
            &mut ctx,
            vec![
                egui::Event::PointerMoved(egui::pos2(300.0, 300.0)),
                button(300.0, 300.0, true),
            ],
        );
        assert_eq!(panels.selected, None);
        assert!(!panels.editing);
        let _ = e;
    }

    #[test]
    fn a_scene_load_clears_the_selection() {
        let (egui, mut panels, mut ctx, e) = edit_scene();
        panels.selected = Some(e);
        frame(&egui, &mut panels, &mut ctx, vec![]);
        assert_eq!(panels.selected, Some(e));
        // The same ids again, from a load: `e` may be another entity now.
        let save = WorldReflect::save(&ctx.world);
        ctx.replace_world(&save).unwrap();
        assert!(ctx.world.is_alive(e));
        frame(&egui, &mut panels, &mut ctx, vec![]);
        assert_eq!(panels.selected, None, "the selection named the old world's entity");
        panels.selected = Some(e);
        frame(&egui, &mut panels, &mut ctx, vec![]);
        assert_eq!(panels.selected, Some(e), "a new selection stays");
    }

    #[test]
    fn play_then_stop_from_the_panels_restores_the_world() {
        let (egui, mut panels, mut ctx, e) = edit_scene();
        ctx.edit(|ed, ctx| ed.play(ctx));
        ctx.world.despawn(e);
        frame(&egui, &mut panels, &mut ctx, vec![]);
        ctx.edit(|ed, ctx| ed.stop(ctx)).unwrap();
        assert!(
            ctx.world.get::<Health>(e).is_some(),
            "back, with the same id (the world was reloaded)"
        );
    }

    #[test]
    fn panels_draw_a_selected_entity_with_reflected_and_hidden_components() {
        let (mut ctx, e) = game();
        let mut panels = Panels {
            selected: Some(e),
            ..Default::default()
        };
        let out = run(&mut panels, &mut ctx);
        assert!(!out.shapes.is_empty());
        assert_eq!(panels.selected, Some(e));
        assert!(panels.error.is_none());
    }

    #[test]
    fn a_despawned_selection_is_dropped() {
        let (mut ctx, e) = game();
        let mut panels = Panels {
            selected: Some(e),
            ..Default::default()
        };
        ctx.world.despawn(e);
        let _ = run(&mut panels, &mut ctx);
        assert_eq!(panels.selected, None);
    }

    #[test]
    fn rows_cover_every_matching_entity_once() {
        let (ctx, e) = game();
        let mut rows = Vec::new();
        let total = list_rows(&ctx.world, None, &mut rows);
        assert_eq!(total, 101);
        let listed: HashSet<Entity> = (0..total).filter_map(|r| entity_at(&ctx.world, &rows, r)).collect();
        assert_eq!(listed.len(), 101);
        assert!(listed.contains(&e));
        assert_eq!(entity_at(&ctx.world, &rows, total), None);

        let health = ctx
            .world
            .raw_ref()
            .component_by_name(std::any::type_name::<Health>())
            .unwrap();
        assert_eq!(list_rows(&ctx.world, Some(health), &mut rows), 1);
        assert_eq!(entity_at(&ctx.world, &rows, 0), Some(e));
    }

    #[test]
    fn a_pick_selects_the_nearest_entity_under_the_click() {
        let (mut ctx, e) = game();
        let at = ctx.camera.world_to_screen(Vec2::new(12.0, 3.0));
        let mut panels = Panels {
            picking: true,
            pick_at: Some(at),
            ..Default::default()
        };
        let _ = run(&mut panels, &mut ctx);
        assert_eq!(panels.selected, Some(e));
        assert!(!panels.picking, "picking ends after one click");

        let far = ctx.camera.world_to_screen(Vec2::new(55.0, -300.0));
        assert_eq!(pick(&mut ctx, far), None, "nothing within reach");
    }

    #[test]
    fn number_edits_keep_their_kind() {
        let egui = egui::Context::default();
        let mut value = json!({ "current": 2.5, "max": 7, "offset": -3, "name": "crab", "on": true });
        egui.run_ui(egui::RawInput::default(), |ui| {
            assert!(!edit_value(ui, &mut value), "nothing changes without input");
        })
        .drop_without_applying_deltas();
        assert_eq!(
            value,
            json!({ "current": 2.5, "max": 7, "offset": -3, "name": "crab", "on": true })
        );
        assert!(value["max"].is_u64() && value["offset"].is_i64() && value["current"].is_f64());
    }

    #[test]
    fn frame_history_wraps_around() {
        let mut panels = Panels::default();
        for i in 0..HISTORY + 10 {
            panels.record_frame(i as f32);
        }
        assert_eq!(panels.frame_ms.len(), HISTORY);
        assert_eq!(panels.frame_at, 10);
        assert_eq!(panels.frame_ms[9], (HISTORY + 9) as f32);
    }
}
