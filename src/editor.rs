//! The editor: changes to the world with undo and redo, and Play / Stop
//! (run the game, then put the world back as it was).
//!
//! Kept apart from the egui panels (`crate::inspector`) so every edit can
//! be tested without a window, and so the control endpoint's `editor.*`
//! methods make the same edits: the history lives in [`Ctx`], shared by a
//! person in the inspector and an agent over MCP. Edits work on reflected
//! components only: they're what the editor can read, write and put back.
//! Nothing here depends on egui; it costs nothing until something edits.

use std::collections::{HashMap, HashSet};

use ponko_ecs::{Component, Entity};
use serde_json::{Value, json};

use crate::{ctx::Ctx, reflect::WorldReflect};
use glam::Vec2;

/// Undo steps kept; older ones are dropped.
const MAX_UNDO: usize = 200;

/// One change, as what puts it back.
#[derive(Debug)]
enum Undo {
    /// Component `name` of `e` was `before` (`None`: it didn't have it).
    Component {
        e: Entity,
        name: String,
        before: Option<Value>,
    },
    /// These entities were spawned: despawn them, with their trees (prefab
    /// parts and what is attached to them).
    Spawned(Vec<Entity>),
    /// These entities were despawned with these (reflected) components:
    /// spawn them again. Handles never come back, so they're new entities;
    /// [`Editor::remap`] points the other steps at them, and references
    /// between them (attachments, prefab parts) are kept.
    Despawned(Vec<(Entity, Value)>),
    /// Several changes made as one (a drag of several entities).
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    Group(Vec<Undo>),
}

/// Edits with undo and redo, and the world saved by Play for Stop.
#[derive(Default)]
pub(crate) struct Editor {
    undo: Vec<Undo>,
    redo: Vec<Undo>,
    /// Entities that were despawned and brought back by an undo or redo,
    /// to the entity that stands for them now. Older steps keep naming the
    /// first ids; they are looked up here, and component values that name
    /// them are read through it.
    remap: HashMap<Entity, Entity>,
    /// The world as it was when Play was pressed, how many undo steps there
    /// were then (steps taken while playing are dropped by Stop), and the
    /// remap of then.
    played_from: Option<(Value, usize, HashMap<Entity, Entity>)>,
    /// `Ctx::scene_loads` the history belongs to: a load since then
    /// replaced the entities its steps name.
    loads: u64,
}

/// Marks an entity whose components a restore hasn't set yet.
#[derive(Component)]
struct Restoring;

impl Ctx {
    /// Runs `f` with the shared editor (see [`Ctx::editor`]). The history is
    /// dropped first if a scene was loaded since the last edit: its steps
    /// named the old world's entities.
    pub(crate) fn edit<R>(&mut self, f: impl FnOnce(&mut Editor, &mut Ctx) -> R) -> R {
        let mut editor = std::mem::take(&mut self.editor);
        if editor.loads != self.scene_loads {
            editor.forget_history();
            editor.loads = self.scene_loads;
        }
        let out = f(&mut editor, self);
        self.editor = editor;
        out
    }
}

impl Editor {
    /// Whether Play was pressed and Stop wasn't yet.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn playing(&self) -> bool {
        self.played_from.is_some()
    }

    fn forget_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.remap.clear();
        if let Some((_, steps, remap)) = &mut self.played_from {
            *steps = 0;
            remap.clear();
        }
    }

    /// Runs the game, first saving the world for [`stop`](Self::stop).
    /// Pressed again while playing (after a pause), it only resumes.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn play(&mut self, ctx: &mut Ctx) {
        if self.played_from.is_none() {
            self.played_from = Some((ctx.world.save(), self.undo.len(), self.remap.clone()));
        }
        ctx.time.resume();
    }

    /// Pauses and puts the world back as it was when Play was pressed.
    /// Components that aren't reflected can't be saved, so aren't put back.
    /// If the world can't be put back (a lockstep session is running:
    /// [`ErrorKind::Unsupported`](crate::error::ErrorKind::Unsupported)), the Play
    /// snapshot and the undo history are kept and the simulation keeps
    /// running, so Stop can be retried.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn stop(&mut self, ctx: &mut Ctx) -> Result<(), crate::error::Error> {
        let Some((save, _, _)) = &self.played_from else {
            ctx.time.pause();
            return Ok(());
        };
        // Pause only once the world is back: a refused Stop changes nothing.
        ctx.replace_world(save)?;
        ctx.time.pause();
        let Some((_, steps, remap)) = self.played_from.take() else {
            return Ok(());
        };
        self.undo.truncate(steps);
        self.redo.clear();
        self.remap = remap;
        // The editor's own load: the steps from before Play still hold.
        self.loads = ctx.scene_loads;
        Ok(())
    }

    /// Keeps the world as it is now: Stop won't go back.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn keep(&mut self) {
        self.played_from = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// A new change: it can be undone, and what was undone can't be redone.
    fn record(&mut self, step: Undo) {
        if self.undo.len() == MAX_UNDO {
            self.undo.remove(0);
            if let Some((_, steps, _)) = &mut self.played_from {
                *steps = steps.saturating_sub(1);
            }
        }
        self.undo.push(step);
        self.redo.clear();
    }

    /// The entity that stands for `e` now (`e` itself unless it was
    /// despawned and brought back).
    pub fn resolve(&self, e: Entity) -> Entity {
        self.remap.get(&e).copied().unwrap_or(e)
    }

    /// Writes component `name` of `e` (adding it if missing), with undo.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn set(&mut self, ctx: &mut Ctx, e: Entity, name: &str, value: Value) -> Result<(), crate::error::Error> {
        let before = ctx.world.get_json(e, name).ok();
        ctx.load_scene_assets(&json!({ name: &value }))?;
        ctx.world.set_json(e, name, value)?;
        self.record(Undo::Component {
            e,
            name: name.to_string(),
            before,
        });
        settle(&mut ctx.world);
        Ok(())
    }

    /// Adds component `name` to `e` with its default fields (types whose
    /// fields all have defaults; others say which field is missing).
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn add(&mut self, ctx: &mut Ctx, e: Entity, name: &str) -> Result<(), crate::error::Error> {
        self.set(ctx, e, name, json!({}))
    }

    /// Removes component `name` from `e`, with undo.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn remove(&mut self, ctx: &mut Ctx, e: Entity, name: &str) -> Result<(), crate::error::Error> {
        let before = ctx.world.get_json(e, name)?;
        ctx.world.remove_json(e, name)?;
        self.record(Undo::Component {
            e,
            name: name.to_string(),
            before: Some(before),
        });
        Ok(())
    }

    /// Records that `e`'s transform was `before` a drag that moved it
    /// directly (the drag itself changes the world every frame).
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn moved(&mut self, e: Entity, before: Value) {
        self.record(Undo::Component {
            e,
            name: "Transform2D".into(),
            before: Some(before),
        });
    }

    /// Records, as one step, that these components were as given before a
    /// change made directly (a gizmo drag of several entities). Components
    /// that didn't change are left out; nothing is recorded if none did.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub fn changed(&mut self, world: &ponko_ecs::World, before: Before) {
        let steps: Vec<Undo> = before
            .0
            .into_iter()
            .filter(|(e, name, value)| world.get_json(*e, name).ok() != *value)
            .map(|(e, name, before)| Undo::Component { e, name, before })
            .collect();
        if !steps.is_empty() {
            self.record(Undo::Group(steps));
        }
    }

    /// Spawns an entity with only a transform at `at`.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn spawn_empty(&mut self, ctx: &mut Ctx, at: Vec2) -> Entity {
        let e = ctx.world.spawn((crate::d2::Transform::at(at),));
        self.record(Undo::Spawned(vec![e]));
        e
    }

    /// Spawns prefab `path` at `at`, with undo (which despawns its parts too).
    pub fn spawn_prefab(
        &mut self,
        ctx: &mut Ctx,
        path: &str,
        at: impl Into<crate::scene::Placement>,
    ) -> Result<Entity, crate::error::Error> {
        let e = ctx.spawn_prefab(path, at)?;
        self.record(Undo::Spawned(vec![e]));
        settle(&mut ctx.world);
        Ok(e)
    }

    /// Spawns a model of the mesh file `path` (glTF) at `at`, with undo.
    #[cfg(feature = "3d")]
    pub fn spawn_model(
        &mut self,
        ctx: &mut Ctx,
        path: &str,
        at: crate::d3::Transform,
    ) -> Result<Entity, crate::error::Error> {
        let components = json!({
            "Transform3D": serde_json::to_value(at).map_err(crate::error::Error::from)?,
            "Model": { "mesh": path },
        });
        ctx.load_scene_assets(&components).map_err(|e| e.with_path(path))?;
        let e = ctx.world.spawn_json(components)?;
        self.record(Undo::Spawned(vec![e]));
        Ok(e)
    }

    /// A copy of `e`'s reflected components, `offset` away.
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn duplicate(&mut self, ctx: &mut Ctx, e: Entity, offset: Vec2) -> Result<Entity, crate::error::Error> {
        let copies = self.duplicate_many(ctx, &[e], offset, Offset3::ZERO)?;
        Ok(copies[0])
    }

    /// Copies of `roots` with their trees (prefab parts and what is
    /// attached to them; references inside a tree name the copies), moved
    /// by `offset` (2D) or `offset3` (3D). Returns the copies of `roots`.
    pub fn duplicate_many(
        &mut self,
        ctx: &mut Ctx,
        roots: &[Entity],
        offset: Vec2,
        offset3: Offset3,
    ) -> Result<Vec<Entity>, crate::error::Error> {
        let members = tree(&ctx.world, roots);
        let snapshot = snapshot(&ctx.world, &members)?;
        let map = respawn(ctx, &snapshot, HashMap::new())?;
        let copies: Vec<Entity> = roots.iter().map(|r| map[r]).collect();
        for &copy in &copies {
            if let Some(t) = ctx.world.get_mut::<crate::d2::Transform>(copy) {
                t.position += offset;
            }
            #[cfg(feature = "3d")]
            if let Some(&t) = ctx.world.get::<crate::d3::Transform>(copy) {
                let moved = crate::d3::Transform {
                    position: t.position + offset3.0,
                    ..t
                };
                place_world3d(&mut ctx.world, copy, moved);
            }
        }
        let _ = offset3;
        settle(&mut ctx.world);
        self.record(Undo::Spawned(map.values().copied().collect()));
        Ok(copies)
    }

    /// Despawns `e`, with undo (which brings back its reflected components
    /// as a new entity: references to `e` from other components stay dead).
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    pub fn delete(&mut self, ctx: &mut Ctx, e: Entity) -> Result<(), crate::error::Error> {
        self.delete_many(ctx, &[e])
    }

    /// Despawns `roots` with their trees (prefab parts and what is attached
    /// to them), as one step. Undo brings them all back as new entities,
    /// still attached to each other.
    pub fn delete_many(&mut self, ctx: &mut Ctx, roots: &[Entity]) -> Result<(), crate::error::Error> {
        for &e in roots {
            if !ctx.world.is_alive(e) {
                return Err(crate::error::Error::new(
                    crate::error::ErrorKind::NotFound,
                    format!("no entity {}", e.to_bits()),
                ));
            }
        }
        let members = tree(&ctx.world, roots);
        let snapshot = snapshot(&ctx.world, &members)?;
        for &e in &members {
            ctx.world.despawn(e);
        }
        if snapshot
            .iter()
            .any(|(_, c)| c.as_object().is_some_and(|c| !c.is_empty()))
        {
            self.record(Undo::Despawned(snapshot));
        }
        Ok(())
    }

    /// Makes `child` follow `parent` (`None`: follow nothing), without
    /// moving it: its attachment's offset is where it is now, in the
    /// parent's space. 3D entities get an `Attached3D`, 2D ones an
    /// [`Attached`](crate::attach::Attached). With undo.
    ///
    /// # Errors
    ///
    /// `Invalid` when the link would make a loop, the two entities
    /// aren't both 2D or both 3D, or `child` already follows a skeleton or
    /// a skin; `NotFound` when either is gone.
    pub fn set_parent(
        &mut self,
        ctx: &mut Ctx,
        child: Entity,
        parent: Option<Entity>,
    ) -> Result<(), crate::error::Error> {
        use crate::error::{Error, ErrorKind};
        let invalid = |m: String| Err(Error::new(ErrorKind::Invalid, m));
        for e in std::iter::once(child).chain(parent) {
            if !ctx.world.is_alive(e) {
                return Err(Error::new(ErrorKind::NotFound, format!("no entity {}", e.to_bits())));
            }
        }
        let world = &ctx.world;
        let is3d = has3d(world, child);
        let name = if is3d { "Attached3D" } else { "Attached" };
        let before = world.get_json(child, name).ok();
        let Some(parent) = parent else {
            if before.is_some() {
                ctx.world.remove_json(child, name)?;
                self.record(Undo::Component {
                    e: child,
                    name: name.into(),
                    before,
                });
            }
            return Ok(());
        };
        if parent == child || ancestors(world, parent).contains(&child) {
            return invalid(format!(
                "entity {} is above entity {} already: that link would make a loop",
                child.to_bits(),
                parent.to_bits()
            ));
        }
        if let Some((_, link)) = parent_of(world, child, &HashMap::new())
            && matches!(link, Link::Joint | Link::Skin)
        {
            return invalid(format!(
                "entity {} follows a {}: remove that first",
                child.to_bits(),
                link.name()
            ));
        }
        let value = if is3d {
            link3d(world, child, parent)
        } else {
            link2d(world, child, parent)
        };
        let Some(value) = value else {
            return invalid(format!(
                "entities {} and {} need the same kind of transform (both 2D or both 3D) to be linked",
                child.to_bits(),
                parent.to_bits()
            ));
        };
        ctx.world.set_json(child, name, value)?;
        self.record(Undo::Component {
            e: child,
            name: name.into(),
            before,
        });
        Ok(())
    }

    /// Moves, turns or scales entities to these world transforms, as one
    /// step; attached ones keep their parent and get a new offset, and
    /// what is attached to them follows.
    #[cfg(feature = "3d")]
    pub fn set_transforms3d(&mut self, ctx: &mut Ctx, to: &[(Entity, crate::d3::Transform)]) {
        let entities: Vec<Entity> = to.iter().map(|&(e, _)| e).collect();
        let before = Before::of3d(&ctx.world, &entities);
        for &(e, t) in to {
            place_world3d(&mut ctx.world, e, t);
        }
        settle(&mut ctx.world);
        self.changed(&ctx.world, before);
    }

    /// Puts back the last change. `Ok(false)` if there's nothing to undo.
    pub fn undo(&mut self, ctx: &mut Ctx) -> Result<bool, crate::error::Error> {
        let Some(step) = self.undo.pop() else {
            return Ok(false);
        };
        let back = self.apply(ctx, step)?;
        self.redo.push(back);
        settle(&mut ctx.world);
        Ok(true)
    }

    /// Makes the last undone change again. `Ok(false)` if there's nothing
    /// to redo; any new change empties what can be redone.
    pub fn redo(&mut self, ctx: &mut Ctx) -> Result<bool, crate::error::Error> {
        let Some(step) = self.redo.pop() else {
            return Ok(false);
        };
        let back = self.apply(ctx, step)?;
        self.undo.push(back);
        settle(&mut ctx.world);
        Ok(true)
    }

    /// Applies `step` and returns the step that puts that back.
    fn apply(&mut self, ctx: &mut Ctx, step: Undo) -> Result<Undo, crate::error::Error> {
        Ok(match step {
            Undo::Component { e, name, before } => {
                let e = self.resolve(e);
                let now = ctx.world.get_json(e, &name).ok();
                let world = &mut ctx.world;
                Entity::with_remap(&self.remap, || match before {
                    Some(value) => world.set_json(e, &name, value),
                    None => world.remove_json(e, &name).map(drop),
                })?;
                Undo::Component { e, name, before: now }
            }
            Undo::Spawned(entities) => {
                let entities: Vec<Entity> = entities.iter().map(|&e| self.resolve(e)).collect();
                let members = tree(&ctx.world, &entities);
                let snapshot = snapshot(&ctx.world, &members)?;
                for &e in &members {
                    ctx.world.despawn(e);
                }
                Undo::Despawned(snapshot)
            }
            Undo::Despawned(snapshot) => {
                let map = respawn(ctx, &snapshot, self.remap.clone())?;
                let mut back = Vec::with_capacity(snapshot.len());
                for (old, _) in &snapshot {
                    let new = map[old];
                    for to in self.remap.values_mut() {
                        if to == old {
                            *to = new;
                        }
                    }
                    self.remap.insert(*old, new);
                    back.push(new);
                }
                Undo::Spawned(back)
            }
            Undo::Group(steps) => {
                let mut back = Vec::with_capacity(steps.len());
                for step in steps.into_iter().rev() {
                    back.push(self.apply(ctx, step)?);
                }
                Undo::Group(back)
            }
        })
    }
}

/// What the asset browser lists: files in `assets/` that can be dragged
/// into the scene, and the materials of the mesh files loaded so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AssetList {
    /// glTF files (`.glb`, `.gltf`).
    pub meshes: Vec<String>,
    /// Prefab files (`"format": "ponko-prefab"`).
    pub prefabs: Vec<String>,
    /// Scene files (`"format": "ponko-world"`).
    pub scenes: Vec<String>,
    /// (mesh file, number in it, name) of each material of a loaded mesh file.
    pub materials: Vec<(String, usize, String)>,
}

/// Folders below `assets/` the browser looks into, at most.
const BROWSE_DEPTH: usize = 8;
/// Files the browser lists, at most.
const BROWSE_FILES: usize = 4096;

/// Lists the assets the editor can place (see [`AssetList`]), from every
/// `assets/` folder found. Web builds have no folders: only materials.
pub(crate) fn browse(ctx: &Ctx) -> AssetList {
    let mut out = AssetList::default();
    #[cfg(not(target_arch = "wasm32"))]
    for root in crate::assets::asset_roots() {
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    if depth < BROWSE_DEPTH {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                let Ok(rel) = path.strip_prefix(&root) else { continue };
                let rel = rel.to_string_lossy().replace('\\', "/");
                match path.extension().and_then(|e| e.to_str()) {
                    Some("glb" | "gltf") => out.meshes.push(rel),
                    Some("json") => match json_format(&path).as_deref() {
                        Some(crate::scene::PREFAB_FORMAT) => out.prefabs.push(rel),
                        Some("ponko-world") => out.scenes.push(rel),
                        _ => {}
                    },
                    _ => {}
                }
                if out.meshes.len() + out.prefabs.len() + out.scenes.len() >= BROWSE_FILES {
                    break;
                }
            }
        }
    }
    for list in [&mut out.meshes, &mut out.prefabs, &mut out.scenes] {
        list.sort();
        list.dedup();
    }
    #[cfg(feature = "3d")]
    if let Some(names) = ctx.world.get_resource::<crate::reflect::MaterialNames>() {
        out.materials = names.list();
    }
    let _ = ctx;
    out
}

/// The `"format"` of a JSON file, read whole when it's small, else
/// looked for in its first bytes.
#[cfg(not(target_arch = "wasm32"))]
fn json_format(path: &std::path::Path) -> Option<String> {
    const WHOLE: u64 = 4 << 20;
    let len = std::fs::metadata(path).ok()?.len();
    if len <= WHOLE {
        let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        return v["format"].as_str().map(str::to_string);
    }
    let mut head = vec![0u8; 64 << 10];
    let n = std::io::Read::read(&mut std::fs::File::open(path).ok()?, &mut head).ok()?;
    let head = String::from_utf8_lossy(&head[..n]);
    [crate::scene::PREFAB_FORMAT, "ponko-world"]
        .into_iter()
        .find(|f| head.contains(&format!("\"{f}\"")))
        .map(str::to_string)
}

/// A 3D offset for [`Editor::duplicate_many`] (zero in builds without 3D).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Offset3(#[cfg(feature = "3d")] pub glam::Vec3);

impl Offset3 {
    #[cfg(feature = "3d")]
    pub const ZERO: Self = Self(glam::Vec3::ZERO);
    #[cfg(not(feature = "3d"))]
    pub const ZERO: Self = Self();
}

/// Components as they were before a direct change, for
/// [`Editor::changed`]: (entity, component, value or `None` if absent).
#[cfg_attr(not(feature = "3d"), allow(dead_code))]
#[derive(Default, Debug)]
pub(crate) struct Before(Vec<(Entity, String, Option<Value>)>);

impl Before {
    /// The 3D transforms and attachments of `entities`: what a gizmo drag changes.
    #[cfg(feature = "3d")]
    pub fn of3d(world: &ponko_ecs::World, entities: &[Entity]) -> Self {
        let mut out = Vec::with_capacity(entities.len() * 2);
        for &e in entities {
            for name in ["Transform3D", "Attached3D"] {
                out.push((e, name.to_string(), world.get_json(e, name).ok()));
            }
        }
        Self(out)
    }
}

/// Places every attachment now (the simulation may be paused while
/// editing): 3D ones; 2D ones follow when `follow_attachments` runs.
pub(crate) fn settle(world: &mut ponko_ecs::World) {
    #[cfg(feature = "3d")]
    crate::attach::settle3d(world);
    let _ = world;
}

/// Gives `e` the world transform `t`. If it's attached, its offset changes
/// instead, so the attachment puts it there.
#[cfg(feature = "3d")]
pub(crate) fn place_world3d(world: &mut ponko_ecs::World, e: Entity, t: crate::d3::Transform) {
    use crate::attach::Attached3D;
    if let Some(&a) = world.get::<Attached3D>(e)
        && let Some(parent) = crate::attach::place3d(world, Attached3D::to(a.to))
    {
        let offset = crate::attach::relative3d(&parent, &t);
        if let Some(link) = world.get_mut::<Attached3D>(e) {
            link.offset = offset;
        }
    }
    if let Some(at) = world.get_mut::<crate::d3::Transform>(e) {
        *at = t;
    }
}

fn has3d(world: &ponko_ecs::World, e: Entity) -> bool {
    #[cfg(feature = "3d")]
    if world.get::<crate::d3::Transform>(e).is_some() {
        return true;
    }
    let _ = (world, e);
    false
}

/// `child`'s `Attached3D` to `parent`, where it is now.
#[allow(unused_variables)]
fn link3d(world: &ponko_ecs::World, child: Entity, parent: Entity) -> Option<Value> {
    #[cfg(feature = "3d")]
    {
        let c = *world.get::<crate::d3::Transform>(child)?;
        // The parent's own place, as its attachments put it.
        let p = match world.get::<crate::attach::Attached3D>(parent) {
            Some(&a) => crate::attach::place3d(world, a)?,
            None => *world.get::<crate::d3::Transform>(parent)?,
        };
        let link = crate::attach::Attached3D::to(parent).offset(crate::attach::relative3d(&p, &c));
        return serde_json::to_value(link).ok();
    }
    #[allow(unreachable_code)]
    None
}

/// `child`'s 2D [`Attached`](crate::attach::Attached) to `parent`, where it is now.
fn link2d(world: &ponko_ecs::World, child: Entity, parent: Entity) -> Option<Value> {
    use crate::d2::Transform as T2;
    let c = world.get::<T2>(child)?;
    let p = world.get::<T2>(parent)?;
    let divide = |a: f32, b: f32| if b == 0.0 { a } else { a / b };
    let local = Vec2::from_angle(-p.rotation).rotate(c.position - p.position);
    let offset = Vec2::new(divide(local.x, p.scale.x), divide(local.y, p.scale.y));
    let link = crate::attach::Attached::to(parent)
        .offset(offset)
        .rotated(c.rotation - p.rotation);
    serde_json::to_value(link).ok()
}

/// How an entity hangs off its parent in the hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Link {
    /// [`Attached`](crate::attach::Attached).
    Attached,
    /// `Attached3D`.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    Attached3D,
    /// `AttachToJoint`: on a joint of the parent's skeleton.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    Joint,
    /// `SkinOf`: posed by the parent's skeleton.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    Skin,
    /// One of the parent's `PrefabParts`, not attached to anything.
    PrefabPart,
}

impl Link {
    pub fn name(self) -> &'static str {
        match self {
            Link::Attached => "attached",
            Link::Attached3D => "attached3d",
            Link::Joint => "joint",
            Link::Skin => "skin",
            Link::PrefabPart => "prefab_part",
        }
    }
}

/// The entity `e` hangs off, and how. `owners` maps prefab parts to their
/// prefab's root (see [`part_owners`]).
fn parent_of(world: &ponko_ecs::World, e: Entity, owners: &HashMap<Entity, Entity>) -> Option<(Entity, Link)> {
    if let Some(a) = world.get::<crate::attach::Attached>(e) {
        return Some((a.to, Link::Attached));
    }
    #[cfg(feature = "3d")]
    {
        if let Some(a) = world.get::<crate::attach::Attached3D>(e) {
            return Some((a.to, Link::Attached3D));
        }
        if let Some(a) = world.get::<crate::anim::AttachToJoint>(e) {
            return Some((a.skeleton, Link::Joint));
        }
        if let Some(s) = world.get::<crate::anim::SkinOf>(e) {
            return Some((s.0, Link::Skin));
        }
    }
    owners.get(&e).map(|&root| (root, Link::PrefabPart))
}

/// Each prefab part, to the root whose `PrefabParts` lists it.
fn part_owners(world: &ponko_ecs::World) -> HashMap<Entity, Entity> {
    let mut owners = HashMap::new();
    for a in world.raw_ref().archetypes() {
        for &root in a.entities() {
            if let Some(parts) = world.get::<crate::scene::PrefabParts>(root) {
                for &part in &parts.0 {
                    owners.entry(part).or_insert(root);
                }
            }
        }
    }
    owners
}

/// The entities above `e`, nearest first (stops at a loop).
pub(crate) fn ancestors(world: &ponko_ecs::World, e: Entity) -> Vec<Entity> {
    let owners = part_owners(world);
    let mut out = Vec::new();
    let mut at = e;
    while let Some((up, _)) = parent_of(world, at, &owners) {
        if up == e || out.contains(&up) || !world.is_alive(up) {
            break;
        }
        out.push(up);
        at = up;
    }
    out
}

/// One entity of the scene's hierarchy.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub entity: Entity,
    /// The entity it hangs off and how; `None` for a root.
    pub parent: Option<(Entity, Link)>,
    /// It's a prefab instance's root (it has `PrefabParts`).
    pub prefab: bool,
    /// The entities that hang off it, in id order.
    pub children: Vec<Entity>,
}

/// Every entity as a tree: roots (in id order) each followed by what hangs
/// off it, depth first. A link to a dead entity, or a loop, makes a root.
pub(crate) fn hierarchy(world: &ponko_ecs::World) -> Vec<Node> {
    let owners = part_owners(world);
    let mut all: Vec<Entity> = world
        .raw_ref()
        .archetypes()
        .iter()
        .flat_map(|a| a.entities().iter().copied())
        .collect();
    all.sort_unstable();
    let mut nodes: HashMap<Entity, Node> = HashMap::with_capacity(all.len());
    for &e in &all {
        let parent = parent_of(world, e, &owners).filter(|(p, _)| *p != e && world.is_alive(*p));
        nodes.insert(
            e,
            Node {
                entity: e,
                parent,
                prefab: world.get::<crate::scene::PrefabParts>(e).is_some(),
                children: Vec::new(),
            },
        );
    }
    for &e in &all {
        if let Some((p, _)) = nodes[&e].parent
            && let Some(parent) = nodes.get_mut(&p)
        {
            parent.children.push(e);
        }
    }
    // Depth first from the roots; entities in a loop have no root above
    // them, so they're added as roots after.
    let mut out = Vec::with_capacity(all.len());
    let mut seen = HashSet::with_capacity(all.len());
    let visit = |root: Entity, nodes: &HashMap<Entity, Node>, out: &mut Vec<Node>, seen: &mut HashSet<Entity>| {
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if !seen.insert(e) {
                continue;
            }
            let node = nodes[&e].clone();
            stack.extend(node.children.iter().rev().copied());
            out.push(node);
        }
    };
    for &e in &all {
        if nodes[&e].parent.is_none() {
            visit(e, &nodes, &mut out, &mut seen);
        }
    }
    for &e in &all {
        if !seen.contains(&e) {
            if let Some(n) = nodes.get_mut(&e) {
                n.parent = None;
            }
            visit(e, &nodes, &mut out, &mut seen);
        }
    }
    out
}

/// `roots` and everything that hangs off them (prefab parts, attachments,
/// skins, joint attachments, all the way down), roots first.
pub(crate) fn tree(world: &ponko_ecs::World, roots: &[Entity]) -> Vec<Entity> {
    let owners = part_owners(world);
    let mut children: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for a in world.raw_ref().archetypes() {
        for &e in a.entities() {
            if let Some((p, _)) = parent_of(world, e, &owners) {
                children.entry(p).or_default().push(e);
            }
        }
    }
    let mut out: Vec<Entity> = Vec::new();
    let mut seen = HashSet::new();
    for &r in roots {
        if world.is_alive(r) && seen.insert(r) {
            out.push(r);
        }
    }
    let mut at = 0;
    while at < out.len() {
        if let Some(kids) = children.get(&out[at]) {
            for &k in kids {
                if seen.insert(k) {
                    out.push(k);
                }
            }
        }
        at += 1;
    }
    out
}

/// The reflected components of `entities`.
fn snapshot(world: &ponko_ecs::World, entities: &[Entity]) -> Result<Vec<(Entity, Value)>, crate::error::Error> {
    entities.iter().map(|&e| Ok((e, world.entity_json(e)?))).collect()
}

/// Spawns `snapshot`'s entities as new ones and returns old id to new.
/// Components that name an entity of the snapshot name its new one, and
/// those in `remap` (entities brought back before) their new one too.
fn respawn(
    ctx: &mut Ctx,
    snapshot: &[(Entity, Value)],
    mut remap: HashMap<Entity, Entity>,
) -> Result<HashMap<Entity, Entity>, crate::error::Error> {
    for (_, components) in snapshot {
        ctx.load_scene_assets(components)?;
    }
    let mut fresh = Vec::with_capacity(snapshot.len());
    for (old, _) in snapshot {
        let e = ctx.world.spawn((Restoring,));
        remap.insert(*old, e);
        fresh.push(e);
    }
    let world = &mut ctx.world;
    let set = Entity::with_remap(&remap, || {
        for ((_, components), &e) in snapshot.iter().zip(&fresh) {
            if let Value::Object(components) = components {
                for (name, value) in components {
                    world.set_json(e, name, value.clone())?;
                }
            }
            world.remove::<Restoring>(e);
        }
        Ok::<_, crate::reflect::ReflectError>(())
    });
    if let Err(e) = set {
        for e in fresh {
            ctx.world.despawn(e);
        }
        return Err(e.into());
    }
    Ok(snapshot.iter().map(|(old, _)| (*old, remap[old])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d2::Transform as Transform2D;
    use glam::vec2;
    use ponko_ecs::Component;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug, Component)]
    #[serde(default)]
    struct Hp(f32);

    fn ctx() -> Ctx {
        let mut ctx = Ctx::new(1.0 / 60.0, vec2(800.0, 600.0));
        ctx.world.reflect::<Hp>();
        ctx
    }
    fn at(ctx: &Ctx, e: Entity) -> Vec2 {
        ctx.world.get::<Transform2D>(e).unwrap().position
    }

    #[test]
    fn every_edit_is_undone_in_reverse_order() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let a = ed.spawn_empty(&mut ctx, vec2(1.0, 1.0));
        ed.set(&mut ctx, a, "Hp", json!(5.0)).unwrap();
        let b = ed.duplicate(&mut ctx, a, vec2(10.0, 0.0)).unwrap();
        assert_eq!((at(&ctx, b), ctx.world.get::<Hp>(b)), (vec2(11.0, 1.0), Some(&Hp(5.0))));
        ed.remove(&mut ctx, a, "Hp").unwrap();
        ed.delete(&mut ctx, b).unwrap();
        assert_eq!(ctx.world.len(), 1);

        assert!(ed.undo(&mut ctx).unwrap()); // delete: back as a new entity
        assert!(!ctx.world.is_alive(b), "old handles stay dead");
        let back = ctx
            .world
            .query::<(Entity, &Hp)>()
            .iter()
            .map(|(e, _)| e)
            .next()
            .unwrap();
        assert_eq!(at(&ctx, back), vec2(11.0, 1.0));
        assert!(ed.undo(&mut ctx).unwrap()); // remove
        assert_eq!(ctx.world.get::<Hp>(a), Some(&Hp(5.0)));
        assert!(ed.undo(&mut ctx).unwrap()); // duplicate: despawns the one brought back
        assert!(!ctx.world.is_alive(back));
        assert!(ed.undo(&mut ctx).unwrap()); // set (added)
        assert_eq!(ctx.world.get::<Hp>(a), None);
        assert!(ed.undo(&mut ctx).unwrap()); // spawn
        assert_eq!(ctx.world.len(), 0);
        assert!(!ed.undo(&mut ctx).unwrap());
    }

    #[test]
    fn undoing_a_placed_prefab_removes_its_parts() {
        let path = format!("ponko-test-editor-tree-{}.json", std::process::id());
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(&path);
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let root = ctx.world.spawn((Transform2D::default(), Hp(1.0)));
        let part = ctx
            .world
            .spawn((Transform2D::default(), crate::attach::Attached::to(root)));
        let saved = ctx.assets.save_prefab(&ctx.world, root, &path);
        ctx.world.despawn(root);
        ctx.world.despawn(part);
        let placed = saved.and_then(|()| ed.spawn_prefab(&mut ctx, &path, vec2(5.0, 5.0)));
        let _ = std::fs::remove_file(&file);
        placed.unwrap();
        assert_eq!(ctx.world.len(), 2);
        assert!(ed.undo(&mut ctx).unwrap());
        assert_eq!(ctx.world.len(), 0, "the part went with the root");
    }

    #[test]
    fn a_drag_undoes_in_one_step() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let e = ctx.world.spawn((Transform2D::at(vec2(1.0, 2.0)),));
        let before = ctx.world.get_json(e, "Transform2D").unwrap();
        for x in 0..10 {
            ctx.world.get_mut::<Transform2D>(e).unwrap().position.x = x as f32; // the drag
        }
        ed.moved(e, before);
        ed.undo(&mut ctx).unwrap();
        assert_eq!(at(&ctx, e), vec2(1.0, 2.0));
    }

    #[test]
    fn stop_puts_the_world_back_as_it_was_at_play() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let e = ed.spawn_empty(&mut ctx, vec2(1.0, 2.0));
        ed.play(&mut ctx);
        assert!(ed.playing() && !ctx.time.is_paused());
        // The game runs: things move, spawn and die; the editor edits too.
        ctx.world.get_mut::<Transform2D>(e).unwrap().position = vec2(50.0, 50.0);
        ctx.world.spawn((Transform2D::default(),));
        ed.set(&mut ctx, e, "Hp", json!(1.0)).unwrap();
        ctx.time.pause();
        ed.play(&mut ctx); // resume: still from the first Play
        ed.stop(&mut ctx).unwrap();
        assert!(ctx.time.is_paused() && !ed.playing());
        assert_eq!(ctx.world.len(), 1);
        assert_eq!((at(&ctx, e), ctx.world.get::<Hp>(e)), (vec2(1.0, 2.0), None));
        // Undo steps from before Play still apply; those from playing are gone.
        assert!(ed.undo(&mut ctx).unwrap());
        assert_eq!(ctx.world.len(), 0);
        assert!(!ed.can_undo());
    }

    #[test]
    fn keep_makes_stop_do_nothing() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        ed.play(&mut ctx);
        ctx.world.spawn((Transform2D::default(),));
        ed.keep();
        ed.stop(&mut ctx).unwrap();
        assert_eq!(ctx.world.len(), 1);
    }

    #[test]
    fn a_failed_edit_changes_nothing_and_is_not_recorded() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let e = ctx.world.spawn((Transform2D::default(),));
        assert!(ed.set(&mut ctx, e, "Hp", json!("not a number")).is_err());
        assert!(ed.add(&mut ctx, e, "NoSuchThing").is_err());
        assert!(!ed.can_undo());
    }

    #[test]
    fn a_refused_stop_keeps_the_play_snapshot_and_undo_for_a_retry() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let a = ed.spawn_empty(&mut ctx, vec2(1.0, 1.0));
        ed.set(&mut ctx, a, "Hp", json!(5.0)).unwrap();
        ed.play(&mut ctx);
        ed.set(&mut ctx, a, "Hp", json!(9.0)).unwrap(); // while playing
        ctx.world.spawn((Hp(1.0),));
        // A lockstep session: Stop would put the world back on this peer only.
        let session = std::mem::replace(&mut ctx.net.lockstep, crate::net::Net::loopback(2).remove(0));
        drop(session);
        let err = ed.stop(&mut ctx).unwrap_err();
        assert!(err.message().contains("lockstep"), "{err}");
        assert!(ed.playing(), "the Play snapshot is kept");
        assert!(!ctx.time.is_paused(), "a refused Stop doesn't pause the simulation");
        assert_eq!(ed.undo.len(), 3, "and the undo history");
        assert_eq!(ctx.world.len(), 2, "the world is unchanged");
        assert_eq!(ctx.world.get::<Hp>(a), Some(&Hp(9.0)));
        // The session ended: Stop works now.
        ctx.net.lockstep = crate::net::Net::default();
        ed.stop(&mut ctx).unwrap();
        assert!(!ed.playing() && ctx.time.is_paused());
        assert_eq!(ed.undo.len(), 2, "edits made while playing are dropped");
        assert_eq!((ctx.world.len(), ctx.world.get::<Hp>(a)), (1, Some(&Hp(5.0))));
    }

    #[test]
    fn redo_makes_undone_edits_again_and_a_new_edit_drops_it() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let a = ed.spawn_empty(&mut ctx, vec2(1.0, 1.0));
        ed.set(&mut ctx, a, "Hp", json!(5.0)).unwrap();
        ed.delete(&mut ctx, a).unwrap();
        assert_eq!(ctx.world.len(), 0);
        assert!(ed.undo(&mut ctx).unwrap() && ed.undo(&mut ctx).unwrap());
        let back = ed.resolve(a);
        assert_eq!((ctx.world.get::<Hp>(back), ed.can_redo()), (None, true));
        assert!(ed.redo(&mut ctx).unwrap()); // set
        assert_eq!(ctx.world.get::<Hp>(back), Some(&Hp(5.0)));
        assert!(ed.redo(&mut ctx).unwrap()); // delete
        assert_eq!(ctx.world.len(), 0);
        assert!(!ed.redo(&mut ctx).unwrap());
        assert!(ed.undo(&mut ctx).unwrap() && ed.undo(&mut ctx).unwrap());
        ed.spawn_empty(&mut ctx, Vec2::ZERO);
        assert!(!ed.can_redo(), "a new edit drops what was undone");
    }

    #[test]
    fn undo_and_redo_of_a_spawn_and_delete_cycle_keep_finding_the_entity() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let a = ed.spawn_empty(&mut ctx, vec2(3.0, 4.0));
        ed.set(&mut ctx, a, "Hp", json!(2.0)).unwrap();
        for _ in 0..3 {
            ed.undo(&mut ctx).unwrap(); // Hp
            ed.undo(&mut ctx).unwrap(); // spawn: despawned
            assert_eq!(ctx.world.len(), 0);
            ed.redo(&mut ctx).unwrap(); // spawned again, a new entity
            ed.redo(&mut ctx).unwrap(); // Hp on the new one
            let now = ed.resolve(a);
            assert!(ctx.world.is_alive(now));
            assert_eq!(
                (at(&ctx, now), ctx.world.get::<Hp>(now)),
                (vec2(3.0, 4.0), Some(&Hp(2.0)))
            );
        }
    }

    #[test]
    fn deleting_a_parent_takes_its_children_and_undo_brings_them_back_attached() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let parent = ctx.world.spawn((Transform2D::at(vec2(10.0, 0.0)),));
        let child = ctx
            .world
            .spawn((Transform2D::default(), crate::attach::Attached::to(parent), Hp(1.0)));
        let other = ctx.world.spawn((Transform2D::default(),));
        ed.delete(&mut ctx, parent).unwrap();
        assert!(!ctx.world.is_alive(child), "the tree goes as one");
        assert!(ctx.world.is_alive(other));
        ed.undo(&mut ctx).unwrap();
        let (p, c) = (ed.resolve(parent), ed.resolve(child));
        assert!(ctx.world.is_alive(p) && ctx.world.is_alive(c) && p != parent);
        assert_eq!(
            ctx.world.get::<crate::attach::Attached>(c).unwrap().to,
            p,
            "still attached"
        );
        assert_eq!(ctx.world.get::<Hp>(c), Some(&Hp(1.0)));
    }

    #[test]
    fn parenting_keeps_the_child_where_it_is_and_refuses_loops() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let parent = ctx.world.spawn((Transform2D {
            position: vec2(10.0, 0.0),
            rotation: std::f32::consts::FRAC_PI_2,
            scale: vec2(2.0, 2.0),
        },));
        let child = ctx.world.spawn((Transform2D::at(vec2(10.0, 4.0)),));
        ed.set_parent(&mut ctx, child, Some(parent)).unwrap();
        let link = *ctx.world.get::<crate::attach::Attached>(child).unwrap();
        let (p, r) = crate::attach::tests_place(&ctx.world, link).unwrap();
        assert!((p - vec2(10.0, 4.0)).length() < 1e-4, "{p}: it didn't move");
        assert!((r - 0.0).abs() < 1e-5);
        let err = ed.set_parent(&mut ctx, parent, Some(child)).unwrap_err();
        assert!(err.message().contains("loop"), "{err}");
        ed.set_parent(&mut ctx, child, None).unwrap();
        assert!(ctx.world.get::<crate::attach::Attached>(child).is_none());
        ed.undo(&mut ctx).unwrap();
        assert_eq!(ctx.world.get::<crate::attach::Attached>(child), Some(&link));
        ed.undo(&mut ctx).unwrap();
        assert!(ctx.world.get::<crate::attach::Attached>(child).is_none());
    }

    #[test]
    fn the_hierarchy_lists_roots_then_what_hangs_off_them() {
        let mut ctx = ctx();
        let root = ctx.world.spawn((Transform2D::default(),));
        let part = ctx.world.spawn((Transform2D::default(),));
        ctx.world.insert(root, crate::scene::PrefabParts(vec![part]));
        let attached = ctx
            .world
            .spawn((Transform2D::default(), crate::attach::Attached::to(part)));
        let loose = ctx.world.spawn((Transform2D::default(),));
        let nodes = hierarchy(&ctx.world);
        let order: Vec<Entity> = nodes.iter().map(|n| n.entity).collect();
        assert_eq!(order, vec![root, part, attached, loose]);
        assert!(nodes[0].prefab && nodes[0].parent.is_none());
        assert_eq!(nodes[1].parent, Some((root, Link::PrefabPart)));
        assert_eq!(nodes[2].parent, Some((part, Link::Attached)));
        assert_eq!(nodes[0].children, vec![part]);
        assert_eq!(tree(&ctx.world, &[root]), vec![root, part, attached]);
    }

    #[test]
    fn duplicating_a_tree_copies_its_links_to_the_copies() {
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let parent = ctx.world.spawn((Transform2D::default(),));
        let child = ctx
            .world
            .spawn((Transform2D::default(), crate::attach::Attached::to(parent)));
        let copy = ed.duplicate(&mut ctx, parent, vec2(5.0, 0.0)).unwrap();
        assert_eq!(ctx.world.len(), 4);
        let copied_child = hierarchy(&ctx.world)
            .into_iter()
            .find(|n| n.parent.map(|p| p.0) == Some(copy))
            .unwrap()
            .entity;
        assert_ne!(copied_child, child);
        ed.undo(&mut ctx).unwrap();
        assert_eq!(ctx.world.len(), 2, "undo removes the copied tree");
    }

    #[test]
    fn a_scene_load_drops_the_history_but_stop_keeps_it() {
        let mut ctx = ctx();
        ctx.edit(|ed, ctx| ed.spawn_empty(ctx, Vec2::ZERO));
        ctx.edit(|ed, ctx| ed.play(ctx));
        ctx.edit(|ed, ctx| ed.stop(ctx)).unwrap();
        assert!(ctx.editor.can_undo(), "Stop is the editor's own load");
        let save = ctx.world.save();
        ctx.replace_world(&save).unwrap();
        assert!(
            !ctx.edit(|ed, ctx| ed.undo(ctx)).unwrap(),
            "the steps named the old world"
        );
    }

    #[cfg(feature = "3d")]
    #[test]
    fn moving_a_3d_parent_carries_its_children_and_moving_a_child_changes_its_offset() {
        use crate::attach::Attached3D;
        use crate::d3::Transform as T3;
        use glam::vec3;
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let parent = ctx.world.spawn((T3::at(vec3(1.0, 0.0, 0.0)),));
        let child = ctx.world.spawn((T3::at(vec3(1.0, 2.0, 0.0)),));
        ed.set_parent(&mut ctx, child, Some(parent)).unwrap();
        let offset = ctx.world.get::<Attached3D>(child).unwrap().offset;
        assert!((offset.position - vec3(0.0, 2.0, 0.0)).length() < 1e-5);
        ed.set_transforms3d(&mut ctx, &[(parent, T3::at(vec3(5.0, 0.0, 0.0)))]);
        let c = ctx.world.get::<T3>(child).unwrap().position;
        assert!(
            (c - vec3(5.0, 2.0, 0.0)).length() < 1e-5,
            "{c}: the child follows at once"
        );
        ed.set_transforms3d(&mut ctx, &[(child, T3::at(vec3(5.0, 3.0, 1.0)))]);
        let offset = ctx.world.get::<Attached3D>(child).unwrap().offset;
        assert!(
            (offset.position - vec3(0.0, 3.0, 1.0)).length() < 1e-5,
            "the offset moved, not the link"
        );
        ed.undo(&mut ctx).unwrap();
        ed.undo(&mut ctx).unwrap();
        let c = ctx.world.get::<T3>(child).unwrap().position;
        assert!((c - vec3(1.0, 2.0, 0.0)).length() < 1e-5, "{c}: both moves undone");
        ed.redo(&mut ctx).unwrap();
        let c = ctx.world.get::<T3>(child).unwrap().position;
        assert!((c - vec3(5.0, 2.0, 0.0)).length() < 1e-5, "{c}: redone with the parent");
    }

    #[cfg(feature = "3d")]
    #[test]
    fn a_multi_entity_move_is_one_undo_step() {
        use crate::d3::Transform as T3;
        use glam::vec3;
        let mut ctx = ctx();
        let mut ed = Editor::default();
        let a = ctx.world.spawn((T3::default(),));
        let b = ctx.world.spawn((T3::at(vec3(2.0, 0.0, 0.0)),));
        ed.set_transforms3d(
            &mut ctx,
            &[(a, T3::at(vec3(0.0, 1.0, 0.0))), (b, T3::at(vec3(2.0, 1.0, 0.0)))],
        );
        assert!(ed.undo(&mut ctx).unwrap());
        assert_eq!(ctx.world.get::<T3>(a).unwrap().position, glam::Vec3::ZERO);
        assert_eq!(ctx.world.get::<T3>(b).unwrap().position, vec3(2.0, 0.0, 0.0));
        assert!(!ed.can_undo());
    }
}
