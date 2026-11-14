//! Scenes and prefabs: worlds and entities kept in files under `assets/`.
//!
//! A scene is a whole world (the [`WorldReflect::save`] format): a level, a
//! menu, a test setup. A prefab is an entity or a tree of them, spawned as
//! many times as you like: a tree, an enemy, a cart with its wheels. Both are JSON, so they
//! diff well, and agents and the editor write them like any other file.

use serde_json::{Value, json};

use std::collections::HashMap;

use crate::{ctx::Ctx, reflect::WorldReflect};
use glam::Vec2;
use ponko_ecs::{Component, Entity};

/// Loads one asset a scene names (`Assets::scene_asset_field`).
type SceneAssetLoader = std::rc::Rc<dyn Fn(&mut Ctx, &str) -> Result<(), crate::error::Error>>;

/// A field of a reflected component that names an asset, and how to load
/// it (`Assets::scene_asset_field`).
pub(crate) struct SceneAssetField {
    component: String,
    field: String,
    load: SceneAssetLoader,
}

pub(crate) const PREFAB_FORMAT: &str = "ponko-prefab";
/// The prefab version of files of one entity.
pub(crate) const PREFAB_VERSION: u64 = 1;
/// The prefab version of files of several entities (a tree).
pub(crate) const PREFAB_TREE_VERSION: u64 = 2;

impl Ctx {
    /// Replaces the world with the scene in `assets/<path>` (written by
    /// [`save_scene`](crate::assets::Assets::save_scene), the editor, or
    /// [`WorldReflect::save`]). Entities keep their ids, so references
    /// between them hold. Components must be [reflected](WorldReflect::reflect).
    /// The scene and the textures it names may also come from an
    /// [asset source](crate::assets::Assets::add_source).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// Game::new("Crabs", 800, 600).run(|ctx| {
    ///     ctx.world.reflect::<ponko::d2::Transform>();
    ///     ctx.load_scene("levels/beach.json").expect("level");
    /// });
    /// ```
    ///
    /// # What a load replaces and what it keeps
    ///
    /// A scene is a world snapshot, not a whole application save: it holds
    /// entities with reflected components and reflected resources. The
    /// game's services live outside the world; a load treats them like this:
    ///
    /// | State | After `load_scene` |
    /// |---|---|
    /// | Entities | Replaced by the scene's, with the scene's ids. An old [`Entity`] handle may now name a different entity of the new scene. |
    /// | Reflected resources | Those in the scene are written; reflected ones it doesn't name stay. |
    /// | Other resources | Kept as they were (scores you didn't reflect, tuning loaded with [`load_resource`](crate::assets::Assets::load_resource), a baked navigation mesh). |
    /// | Components that aren't reflected | Gone with their entities; [`save_scene`](crate::assets::Assets::save_scene) reports them under `"not_saved"`. |
    /// | [`commands`](Self::commands) | Commands queued earlier in the running system are dropped: they were meant for the old entities. Commands queued after the load apply as usual. |
    /// | [`events`](Self::events) | Kept: events sent before the load can still be read for their usual lifetime. Entity ids inside them (such as [`CollisionStarted`](crate::collision::CollisionStarted)) refer to the old world. |
    /// | 2D and 3D physics | Bodies, characters and joints are removed at once (queries find nothing), then built from the new components at the next physics step, from rest unless the scene has a velocity component. Gravity, collision meshes, contact hooks and settings are kept. |
    /// | [`spatial`](Self::spatial) neighbor grid | Refilled at the start of the next simulation tick. |
    /// | 3D navigation (`NavMesh3D`, with the `navmesh` feature) | The baked mesh, a resource, is kept. Crowd agents are components, so they go with their entities; `step_navmesh3d` frees the old agents' crowd slots at its next step, and agents added to the new scene's entities (even with the same ids) start from where they are. |
    /// | [`Attached`](crate::d2::Attached) | Reflected: the scene's attachments, to the scene's entities. [`follow_attachments`](crate::systems::follow_attachments) places them at its next run and despawns those whose entity isn't in the scene. |
    /// | Skeletons and morph weights | Components that aren't saved: gone with their entities unless the game adds them again. The engine's skinning scratch is rebuilt every frame from the world as it is. |
    /// | [`SkinOf`](crate::anim::SkinOf), [`AttachToJoint`](crate::anim::AttachToJoint), 3D joints | Reflected: the scene's, to the scene's entities. Skeletons aren't saved, so skins and joint attachments show once the game gives their entity a skeleton again. |
    /// | [`PrefabParts`] | Reflected: the scene's prefab trees keep their parts, for [`despawn_prefab`](Self::despawn_prefab). |
    /// | Assets | Textures, meshes and collision files the scene names are loaded first and stay cached. Loaded assets are never unloaded by a load. |
    /// | Prefabs | Kept: files [`spawn_prefab`](Self::spawn_prefab) read stay cached, so spawning one after a load doesn't read it again. |
    /// | Positional sounds | A sound playing from an entity stops at the next audio update unless that entity still has the same playing source; other sounds and music keep playing. |
    /// | Retained UI, camera, lighting, input, time, random seed | Kept: they belong to the game, not the scene. |
    /// | 3D UI panels (`UiPanel3D`, with the `3d` feature) | Components: gone with their entities. Which panel had the keyboard is forgotten, so a new panel with the old one's id doesn't get it without a click. |
    /// | Temporal history (TAA, temporal AO, reflections) | Reset, as by `reset_temporal_history` (`3d` feature): the new scene's models have no motion from the old one's. |
    /// | Lockstep networking ([`net`](Self::net)) | Not synchronized: a load changes only this peer. In a session, load from a simulation system (every peer runs it at the same tick) or in setup (every peer runs it before the first tick); [`load_scene_lockstep`](Self::load_scene_lockstep) is the same call, named for that. Loads from frame, late and draw systems, and tools' loads (the editor's Stop, the control endpoint), are refused while a session is connecting or running. After a session is lost, loads are local again. |
    /// | Tools (the inspector) | Its selection is cleared: the id may name another entity now. |
    ///
    /// The physics rule also holds for [`WorldReflect::load`] called
    /// directly on `ctx.world`, except that bodies are removed at the next
    /// physics step instead of at once.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file is
    /// missing or names a component or resource that isn't reflected,
    /// [`Json`](crate::error::ErrorKind::Json) if it isn't a scene, the error of an
    /// asset it names that can't be loaded (its kind and path), and
    /// [`Unsupported`](crate::error::ErrorKind::Unsupported) when a lockstep session
    /// runs and the load would change only this peer (see above). The world,
    /// commands, physics and every other service are unchanged then; assets
    /// the scene named that did load before the failure stay cached.
    pub fn load_scene(&mut self, path: &str) -> Result<(), crate::error::Error> {
        self.check_session_load().map_err(|e| {
            crate::error::Error::new(crate::error::ErrorKind::Unsupported, e)
                .context(path)
                .with_path(path)
        })?;
        self.read_and_replace(path)
    }

    /// [`load_scene`](Self::load_scene), named for a lockstep game: the
    /// same load on every peer at the same tick. It follows the same rules
    /// (a simulation system or setup in a session, anywhere offline); the
    /// name says at the call site that the load is part of the shared
    /// simulation. The peers' worlds are still compared every
    /// [`checksum_every`](crate::net::Net::checksum_every) ticks, so a load that
    /// only some peers made shows up as a [`Desync`](crate::net::Desync).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn next_level(ctx: &mut Ctx) {
    /// // A simulation system: every peer reaches tick 600 with the same world.
    /// if ctx.time.sim_tick() == 600 {
    ///     ctx.load_scene_lockstep("levels/two.json").expect("level");
    /// }
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As `load_scene`.
    pub fn load_scene_lockstep(&mut self, path: &str) -> Result<(), crate::error::Error> {
        self.load_scene(path)
    }

    fn read_and_replace(&mut self, path: &str) -> Result<(), crate::error::Error> {
        let (_, text) = self.assets.read_text(path)?;
        let scene: Value =
            serde_json::from_str(&text).map_err(|e| crate::error::Error::from(e).context(path).with_path(path))?;
        self.replace_world_now(&scene)
            .map_err(|e| e.context(path).with_path(path))
    }

    /// Refuses a game's load that would change only this peer: outside a
    /// simulation system and outside setup while a lockstep session is
    /// connecting or running. Every peer runs its setup before the first
    /// tick and its simulation systems at the same ticks; frame, late and
    /// draw systems run on each peer's own clock.
    fn check_session_load(&self) -> Result<(), String> {
        if !self.session_active() || self.in_sim() || self.in_setup() {
            return Ok(());
        }
        Err(
            "a lockstep session is running and a load outside a simulation system or setup \
             would change only this peer: load from a simulation system, which every peer runs at the same tick"
                .into(),
        )
    }

    fn session_active(&self) -> bool {
        matches!(
            self.net.state(),
            crate::net::NetState::Connecting | crate::net::NetState::Running
        )
    }

    /// Replaces the world with `save` (a scene or world save) for tools (the
    /// editor's Stop, the control endpoint): refused while a lockstep
    /// session is connecting or running, since a tool acts on this peer only.
    pub(crate) fn replace_world(&mut self, save: &Value) -> Result<(), crate::error::Error> {
        if self.session_active() {
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::Unsupported,
                "a lockstep session is running and a tool's load would change only this peer",
            ));
        }
        self.replace_world_now(save)
    }

    /// Replaces the world with `save`, as [`load_scene`](Self::load_scene)
    /// describes: its assets first, then the entities, then the services
    /// that follow entity ids.
    pub(crate) fn replace_world_now(&mut self, save: &Value) -> Result<(), crate::error::Error> {
        // Migrated first, so the assets are found under the current names.
        let save = crate::reflect::migrate_save(&self.world, save)?;
        self.load_scene_assets(&save)?;
        self.world.load(&save)?;
        self.commands = ponko_ecs::Commands::new();
        self.scene_loads += 1;
        #[cfg(feature = "physics")]
        self.physics.forget_bodies();
        #[cfg(feature = "physics3d")]
        self.physics3d.forget_bodies();
        #[cfg(feature = "3d")]
        {
            // Keyboard focus named an entity of the old world: the same id
            // may be another panel now, which nobody clicked.
            self.ui_state.panels.forget_focus();
            // Motion history is keyed by entity id: a same-id model of the
            // new scene has no past in the old one.
            self.gfx.reset_temporal_history();
            self.scene3d.set_quality_autotune(None);
            self.gfx.autotune = None;
        }
        Ok(())
    }

    /// Spawns the prefab in `assets/<path>` with its position at `at` (its
    /// rotation and scale as saved) and returns its root. The file is read
    /// once, then kept.
    ///
    /// A prefab is one entity or a tree of them (a cart with its wheels, a
    /// character with what it carries): the parts are placed relative to the
    /// root, components that name another entity of the prefab
    /// ([`Attached`](crate::d2::Attached), joints, skin targets, your own
    /// components holding an [`Entity`]) name the new copy, and the root gets
    /// [`PrefabParts`] listing the others, for
    /// [`despawn_prefab`](Self::despawn_prefab). A prefab may include another
    /// one, with its root's components changed for this use (see the guide's
    /// "Prefabs with children").
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// for x in 0..10 {
    ///     ctx.spawn_prefab("prefabs/palm.json", vec2(x as f32 * 64.0, 400.0)).expect("palm");
    /// }
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file (or one
    /// it includes) is missing or a component isn't reflected,
    /// [`Json`](crate::error::ErrorKind::Json) if it isn't a prefab, a component
    /// doesn't fit, or prefabs include each other in a loop, and the error of
    /// an asset it names that can't be loaded. The error's path is `path`;
    /// nothing is spawned then.
    pub fn spawn_prefab(&mut self, path: &str, at: impl Into<Placement>) -> Result<Entity, crate::error::Error> {
        let in_prefab = |e: crate::error::Error| e.context(path).with_path(path);
        let file = self.prefab_file(path).map_err(in_prefab)?;
        if let [single] = &file.entries[..]
            && single.prefab.is_none()
        {
            // One entity: spawned whole, as files from before trees were.
            let components = Value::Object(single.components.clone());
            return spawn_at(&mut self.world, components, at.into()).map_err(in_prefab);
        }
        let mut tree = Tree::default();
        let spawned = self
            .spawn_prefab_entities(path, &file, &mut Vec::new(), &mut tree)
            .and_then(|root| {
                place_root(&mut self.world, root, at.into())?;
                // Parents first: a part's parent is placed before the part.
                for &(part, parent) in tree.relative.iter().rev() {
                    compose(&mut self.world, part, parent);
                }
                Ok(root)
            });
        spawned.map_err(|e| {
            for &e in &tree.spawned {
                self.world.despawn(e);
            }
            in_prefab(e)
        })
    }

    /// Despawns `root` and the [`PrefabParts`] that
    /// [`spawn_prefab`](Self::spawn_prefab) gave it, and returns how many
    /// entities that was. An entity without parts is despawned alone, so this
    /// also suits entities that didn't come from a prefab.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let cart = ctx.spawn_prefab("prefabs/cart.json", vec2(100.0, 200.0)).expect("cart");
    /// ctx.despawn_prefab(cart); // the cart and its wheels
    /// # }
    /// ```
    pub fn despawn_prefab(&mut self, root: Entity) -> usize {
        let parts = self
            .world
            .get::<PrefabParts>(root)
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let mut n = usize::from(self.world.despawn(root));
        for part in parts {
            n += usize::from(self.world.despawn(part));
        }
        n
    }

    /// The prefab file `path`, read, checked and its assets loaded the first
    /// time, then cached.
    fn prefab_file(&mut self, path: &str) -> Result<std::rc::Rc<PrefabFile>, crate::error::Error> {
        if let Some(file) = self.assets.prefabs.get(path) {
            return Ok(file.clone());
        }
        let (_, text) = self.assets.read_text(path)?;
        let json: Value = serde_json::from_str(&text)?;
        let file = PrefabFile::parse(&json).map_err(|e| crate::error::Error::new(crate::error::ErrorKind::Json, e))?;
        for entry in &file.entries {
            self.load_scene_assets(&Value::Object(entry.components.clone()))?;
        }
        let file = std::rc::Rc::new(file);
        self.assets.prefabs.insert(path.to_string(), file.clone());
        Ok(file)
    }

    /// Spawns `file`'s entities (the file at `path`) where the file puts
    /// them, relative to their root, and returns the root. Adds every entity
    /// it spawns to `tree.spawned` (also on error, so the caller can despawn
    /// them) and each part with the entity its transform is relative to to
    /// `tree.relative`, children before parents. `including` is the chain of
    /// prefab files being spawned, to stop loops.
    fn spawn_prefab_entities(
        &mut self,
        path: &str,
        file: &PrefabFile,
        including: &mut Vec<String>,
        tree: &mut Tree,
    ) -> Result<Entity, crate::error::Error> {
        if including.iter().any(|p| p == path) || including.len() >= MAX_PREFAB_DEPTH {
            let chain = including.join(" -> ");
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::Json,
                format!("prefabs include each other in a loop or too deep: {chain} -> {path}"),
            ));
        }
        including.push(path.to_string());
        let first = tree.spawned.len();
        // Every entity exists before any component is read, so a component
        // can name an entity that comes later in the file. Included prefabs
        // first: they can't name this file's entities.
        let mut map = HashMap::with_capacity(file.entries.len());
        let mut entities = Vec::with_capacity(file.entries.len());
        for entry in &file.entries {
            let e = match &entry.prefab {
                Some(inner) => {
                    let nested = self.prefab_file(inner).map_err(|e| e.context(inner.as_str()))?;
                    self.spawn_prefab_entities(inner, &nested, including, tree)
                        .map_err(|e| e.context(inner.as_str()))?
                }
                None => {
                    let e = self.world.spawn((Spawning,));
                    tree.spawned.push(e);
                    e
                }
            };
            if let Some(id) = entry.id {
                map.insert(id, e);
            }
            entities.push(e);
        }
        including.pop();
        Entity::with_remap(&map, || {
            for (entry, &e) in file.entries.iter().zip(&entities) {
                for (name, value) in &entry.components {
                    self.world.set_json(e, name, value.clone())?;
                }
                self.world.remove::<Spawning>(e);
            }
            Ok::<_, crate::error::Error>(())
        })?;
        let root = entities[0];
        for &part in &entities[1..] {
            tree.relative.push((part, root));
        }
        if tree.spawned.len() - first > 1 {
            let parts = tree.spawned[first..].iter().copied().filter(|&e| e != root).collect();
            self.world.insert(root, PrefabParts(parts));
        }
        Ok(root)
    }
}

/// Prefabs included in prefabs deeper than this are refused (a loop, most likely).
const MAX_PREFAB_DEPTH: usize = 16;

/// Marks an entity of a prefab whose components aren't set yet.
#[derive(Component)]
struct Spawning;

/// What spawning a prefab tree made so far.
#[derive(Default)]
struct Tree {
    spawned: Vec<Entity>,
    /// (part, the entity its transform is relative to), children first.
    relative: Vec<(Entity, Entity)>,
}

/// The other entities [`Ctx::spawn_prefab`] spawned with a prefab's root: its
/// parts, and the entities of the prefabs it includes. Reflected, so saved
/// worlds and scenes keep it; [`Ctx::despawn_prefab`] removes them with the
/// root, and [`save_prefab`](crate::assets::Assets::save_prefab) on the root
/// saves them with it. A prefab of one entity has none.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// let cart = ctx.spawn_prefab("prefabs/cart.json", vec2(0.0, 0.0)).expect("cart");
/// let parts = ctx.world.get::<PrefabParts>(cart).map_or(0, |p| p.0.len());
/// println!("a cart and {parts} parts");
/// # }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, Component)]
pub struct PrefabParts(pub Vec<Entity>);

/// A prefab file, checked: its entities, root first.
#[derive(Debug, Clone)]
pub(crate) struct PrefabFile {
    entries: Vec<PrefabEntry>,
}

/// One entity of a prefab file.
#[derive(Debug, Clone)]
struct PrefabEntry {
    /// Its id in the file, which components of the prefab use to name it.
    /// Files of one entity have none.
    id: Option<Entity>,
    /// An included prefab, spawned here; `components` then change its root.
    prefab: Option<String>,
    components: serde_json::Map<String, Value>,
}

impl PrefabFile {
    /// Reads a version-1 file (one entity) or a version-2 file (a tree).
    fn parse(prefab: &Value) -> Result<Self, String> {
        if prefab["format"] != PREFAB_FORMAT {
            return Err(format!("not a prefab (expected \"format\": \"{PREFAB_FORMAT}\")"));
        }
        match prefab["version"].as_u64() {
            Some(PREFAB_VERSION) => match &prefab["components"] {
                Value::Object(c) if !c.is_empty() => Ok(Self {
                    entries: vec![PrefabEntry {
                        id: None,
                        prefab: None,
                        components: c.clone(),
                    }],
                }),
                _ => Err("a prefab needs a non-empty \"components\" object".into()),
            },
            Some(PREFAB_TREE_VERSION) => {
                let list = match &prefab["entities"] {
                    Value::Array(list) if !list.is_empty() => list,
                    _ => return Err("a version-2 prefab needs a non-empty \"entities\" list".into()),
                };
                let mut seen = std::collections::HashSet::new();
                let entries = list
                    .iter()
                    .map(|item| {
                        let id = item["entity"]
                            .as_u64()
                            .ok_or_else(|| format!("entity id missing or not a number: {}", item["entity"]))?;
                        if !seen.insert(id) {
                            return Err(format!("entity id {id} appears twice"));
                        }
                        let prefab = match &item["prefab"] {
                            Value::Null => None,
                            Value::String(p) => Some(p.clone()),
                            other => return Err(format!("entity {id}: \"prefab\" is a file name, not {other}")),
                        };
                        let components = match &item["components"] {
                            Value::Object(c) if !c.is_empty() || prefab.is_some() => c.clone(),
                            Value::Null if prefab.is_some() => serde_json::Map::new(),
                            _ => return Err(format!("entity {id} needs a non-empty \"components\" object")),
                        };
                        Ok(PrefabEntry {
                            id: Some(Entity::from_bits(id)),
                            prefab,
                            components,
                        })
                    })
                    .collect::<Result<_, String>>()?;
                Ok(Self { entries })
            }
            _ => Err(format!(
                "unsupported prefab version {} (this engine reads {PREFAB_VERSION} and {PREFAB_TREE_VERSION})",
                prefab["version"]
            )),
        }
    }

    /// A file of one entity's `components`, as `save_prefab` writes it.
    pub(crate) fn single(components: serde_json::Map<String, Value>) -> Self {
        Self {
            entries: vec![PrefabEntry {
                id: None,
                prefab: None,
                components,
            }],
        }
    }
}

/// Puts a prefab tree's root at `at`, as [`spawn_at`] does for one entity.
fn place_root(world: &mut ponko_ecs::World, root: Entity, at: Placement) -> Result<(), crate::error::Error> {
    use crate::d2::Transform as T2;
    match at {
        Placement::Position2(at) => match world.get_mut::<T2>(root) {
            Some(t) => t.position = at,
            None => drop(world.insert(root, T2::at(at))),
        },
        Placement::D2(at) => drop(world.insert(root, at)),
        #[cfg(feature = "3d")]
        Placement::Position3(at) => match world.get_mut::<crate::d3::Transform>(root) {
            Some(t) => t.position = at,
            None => drop(world.insert(root, crate::d3::Transform::at(at))),
        },
        #[cfg(feature = "3d")]
        Placement::D3(at) => drop(world.insert(root, at)),
    }
    Ok(())
}

/// Turns `part`'s transforms, relative to `parent`'s, into world ones.
fn compose(world: &mut ponko_ecs::World, part: Entity, parent: Entity) {
    use crate::d2::Transform as T2;
    if let Some(&p) = world.get::<T2>(parent)
        && let Some(t) = world.get_mut::<T2>(part)
    {
        *t = T2 {
            position: p.position + Vec2::from_angle(p.rotation).rotate(t.position * p.scale),
            rotation: p.rotation + t.rotation,
            scale: p.scale * t.scale,
        };
    }
    #[cfg(feature = "3d")]
    if let Some(&p) = world.get::<crate::d3::Transform>(parent)
        && let Some(t) = world.get_mut::<crate::d3::Transform>(part)
    {
        t.position = p.position + p.rotation * (p.scale * t.position);
        t.rotation = p.rotation * t.rotation;
        t.scale *= p.scale;
    }
}

/// `part`'s transforms relative to `root`'s (the inverse of [`compose`];
/// exact unless a scale is zero, or non-uniform and rotated), written into
/// its components `json`.
pub(crate) fn relative_to(world: &ponko_ecs::World, part: Entity, root: Entity, json: &mut Value) {
    use crate::d2::Transform as T2;
    let divide = |a: f32, b: f32| if b == 0.0 { a } else { a / b };
    if let (Some(&r), Some(&t)) = (world.get::<T2>(root), world.get::<T2>(part)) {
        let local = T2 {
            position: Vec2::from_angle(-r.rotation).rotate(t.position - r.position),
            rotation: t.rotation - r.rotation,
            scale: t.scale,
        };
        let local = T2 {
            position: Vec2::new(divide(local.position.x, r.scale.x), divide(local.position.y, r.scale.y)),
            scale: Vec2::new(divide(local.scale.x, r.scale.x), divide(local.scale.y, r.scale.y)),
            ..local
        };
        if let Ok(v) = serde_json::to_value(local) {
            json["Transform2D"] = v;
        }
    }
    #[cfg(feature = "3d")]
    if let (Some(&r), Some(&t)) = (
        world.get::<crate::d3::Transform>(root),
        world.get::<crate::d3::Transform>(part),
    ) {
        let d = |a: glam::Vec3| glam::Vec3::new(divide(a.x, r.scale.x), divide(a.y, r.scale.y), divide(a.z, r.scale.z));
        let local = crate::d3::Transform {
            position: d(r.rotation.inverse() * (t.position - r.position)),
            rotation: r.rotation.inverse() * t.rotation,
            scale: d(t.scale),
        };
        if let Ok(v) = serde_json::to_value(local) {
            json["Transform3D"] = v;
        }
    }
}

/// `root` and what [`save_prefab`](crate::assets::Assets::save_prefab) saves
/// with it, root first: its [`PrefabParts`], then whatever is attached to
/// those (by [`Attached`](crate::d2::Attached), a skin or joint attachment,
/// or a 3D joint between two of them), until nothing more is.
pub(crate) fn prefab_members(world: &ponko_ecs::World, root: Entity) -> Vec<Entity> {
    let mut members = vec![root];
    if let Some(parts) = world.get::<PrefabParts>(root) {
        members.extend(parts.0.iter().copied().filter(|&p| p != root));
    }
    members.retain(|&e| world.is_alive(e));
    let mut set: std::collections::HashSet<Entity> = members.iter().copied().collect();
    let everyone: Vec<Entity> = world
        .raw_ref()
        .archetypes()
        .iter()
        .flat_map(|a| a.entities().iter().copied())
        .collect();
    // Whether `e` hangs off an entity of `set`.
    let attached = |e: Entity, set: &std::collections::HashSet<Entity>| {
        let mut to = Vec::new();
        to.extend(world.get::<crate::d2::Attached>(e).map(|a| a.to));
        #[cfg(feature = "3d")]
        {
            to.extend(world.get::<crate::anim::SkinOf>(e).map(|s| s.0));
            to.extend(world.get::<crate::anim::AttachToJoint>(e).map(|a| a.skeleton));
        }
        #[cfg(feature = "physics3d")]
        if let Some(j) = world.get::<crate::d3::Joint>(e)
            && set.contains(&j.body_a)
            && set.contains(&j.body_b)
        {
            return true;
        }
        to.iter().any(|t| set.contains(t))
    };
    loop {
        let found: Vec<Entity> = everyone
            .iter()
            .copied()
            .filter(|e| !set.contains(e) && attached(*e, &set))
            .collect();
        if found.is_empty() {
            break;
        }
        set.extend(found.iter().copied());
        members.extend(found);
    }
    members[1..].sort_unstable();
    members
}

impl crate::assets::Assets {
    /// Says that field `field` of reflected component `T` names an asset,
    /// and how to load it: before a scene or prefab is spawned
    /// ([`load_scene`](Ctx::load_scene), [`spawn_prefab`](Ctx::spawn_prefab),
    /// the editor and the control endpoint), `load` is called with every
    /// name found there, the way the textures of `Sprite`s and meshes of
    /// `Model`s are loaded first.
    ///
    /// The component keeps the asset's name (a `String`, or a list of
    /// them), not a handle: scenes save it as written, and a system gets
    /// the handle by loading the name again, which returns the cached one.
    /// `load` decides what the name means, so it covers your own asset
    /// types too ([`load_with`](Self::load_with) into a
    /// resource). Names are found through the [asset
    /// sources](Self::add_source) like any other load.
    ///
    /// `load` returns a [`crate::error::Error`], so the engine's `try_` loaders
    /// can be returned as they are; [`Error::new`](crate::error::Error::new) makes
    /// one for your own asset types.
    ///
    /// The field may hold a name or a list of names; other values are
    /// skipped (the component's own deserialization reports them). A
    /// failing `load` fails the scene load before any entity changes, with
    /// the component, field and name in the message. Calling this again
    /// for the same component and field replaces its loader.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// /// A shop item: its icon is an image, saved by name.
    /// #[derive(Clone, serde::Serialize, serde::Deserialize, Component)]
    /// struct ShopItem {
    ///     icon: String,
    /// }
    ///
    /// # let mut red = Vec::new();
    /// # image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
    /// #     .write_to(&mut std::io::Cursor::new(&mut red), image::ImageFormat::Png)
    /// #     .unwrap();
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     ctx.world.reflect::<ShopItem>();
    ///     ctx.assets.scene_asset_field::<ShopItem>("icon", |ctx, name| ctx.gfx.try_load_texture(name).map(|_| ()));
    ///     // A scene with one shop item, kept in memory with its icon.
    ///     ctx.world.spawn((ShopItem { icon: "icons/apple.png".into() },));
    ///     let shop = serde_json::to_string(&ctx.world.save()).unwrap();
    ///     ctx.assets.add_source(MemorySource::new("shop").with("shop.json", shop).with("icons/apple.png", red));
    ///
    ///     ctx.load_scene("shop.json").unwrap(); // loads "icons/apple.png" first
    ///     let item = ctx.world.query::<&ShopItem>().iter().next().unwrap().clone();
    ///     let icon = ctx.load_texture(&item.icon); // the cached texture
    ///     assert_eq!(icon.width(), 1);
    /// });
    /// ```
    pub fn scene_asset_field<T: 'static>(
        &mut self,
        field: &str,
        load: impl Fn(&mut Ctx, &str) -> Result<(), crate::error::Error> + 'static,
    ) {
        let component = crate::reflect::short_name(std::any::type_name::<T>());
        let entry = SceneAssetField {
            component,
            field: field.to_string(),
            load: std::rc::Rc::new(load),
        };
        match self
            .scene_asset_fields
            .iter_mut()
            .find(|f| f.component == entry.component && f.field == entry.field)
        {
            Some(existing) => *existing = entry,
            None => self.scene_asset_fields.push(entry),
        }
    }
}

impl Ctx {
    /// Loads the assets that fields registered with
    /// [`scene_asset_field`](crate::assets::Assets::scene_asset_field) name anywhere in `json`.
    pub(crate) fn load_scene_asset_fields(&mut self, json: &Value) -> Result<(), crate::error::Error> {
        for i in 0..self.assets.scene_asset_fields.len() {
            let f = &self.assets.scene_asset_fields[i];
            let (component, field, load) = (f.component.clone(), f.field.clone(), f.load.clone());
            let mut names = Vec::new();
            field_names(json, &component, &field, &mut names);
            names.sort_unstable();
            names.dedup();
            for name in names {
                load(self, &name).map_err(|e| e.context(format!("{component}.{field} \"{name}\"")))?;
            }
        }
        Ok(())
    }
}

/// The names (a string, or the strings of a list) under `component`
/// objects' `field` anywhere in `json`.
fn field_names(json: &Value, component: &str, field: &str, out: &mut Vec<String>) {
    match json {
        Value::Object(map) => {
            for (k, value) in map {
                if k == component && value.is_object() {
                    match value.get(field) {
                        Some(Value::String(name)) => out.push(name.clone()),
                        Some(Value::Array(list)) => out.extend(list.iter().filter_map(Value::as_str).map(String::from)),
                        _ => {}
                    }
                } else {
                    field_names(value, component, field, out);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|v| field_names(v, component, field, out)),
        _ => {}
    }
}

/// Spawns `components` with the position set to `at` (adding a transform if
/// there's none).
fn spawn_at(world: &mut ponko_ecs::World, mut components: Value, at: Placement) -> Result<Entity, crate::error::Error> {
    match at {
        Placement::Position2(at) => match components.get_mut("Transform2D") {
            Some(t) => t["position"] = json!([at.x, at.y]),
            None => components["Transform2D"] = serde_json::to_value(crate::d2::Transform::at(at))?,
        },
        Placement::D2(at) => {
            components["Transform2D"] = serde_json::to_value(at)?;
        }
        #[cfg(feature = "3d")]
        Placement::Position3(at) => match components.get_mut("Transform3D") {
            Some(t) => t["position"] = json!([at.x, at.y, at.z]),
            None => components["Transform3D"] = serde_json::to_value(crate::d3::Transform::at(at))?,
        },
        #[cfg(feature = "3d")]
        Placement::D3(at) => {
            components["Transform3D"] = serde_json::to_value(at)?;
        }
    }
    Ok(world.spawn_json(components)?)
}

/// Where [`Ctx::spawn_prefab`](crate::ctx::Ctx::spawn_prefab) puts a prefab: a
/// `Vec2` or `Vec3` moves it and keeps its saved rotation and scale; a
/// `d2::Transform` or `d3::Transform` sets all three. Serialized component
/// names stay stable. Variants depend on features (`3d`), so the enum is
/// `#[non_exhaustive]`: build one with `.into()` or `Placement::from`.
///
/// `spawn_prefab` takes `impl Into<Placement>`, so an argument whose type the
/// compiler used to infer as `Vec2` (`Default::default()`, `(x, y).into()`)
/// now needs its type written out: `Vec2::ZERO`, `vec2(x, y)`.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Placement {
    /// Repositions a 2D prefab, preserving saved rotation and scale.
    Position2(glam::Vec2),
    /// Repositions a 3D prefab, preserving saved rotation and scale.
    #[cfg(feature = "3d")]
    Position3(glam::Vec3),
    /// A 2D position/transform.
    D2(crate::d2::Transform),
    /// A 3D position/transform.
    #[cfg(feature = "3d")]
    D3(crate::d3::Transform),
}
impl From<Vec2> for Placement {
    fn from(at: Vec2) -> Self {
        Self::Position2(at)
    }
}
impl From<crate::d2::Transform> for Placement {
    fn from(at: crate::d2::Transform) -> Self {
        Self::D2(at)
    }
}
#[cfg(feature = "3d")]
impl From<glam::Vec3> for Placement {
    fn from(at: glam::Vec3) -> Self {
        Self::Position3(at)
    }
}
#[cfg(feature = "3d")]
impl From<crate::d3::Transform> for Placement {
    fn from(at: crate::d3::Transform) -> Self {
        Self::D3(at)
    }
}

/// Loads during a lockstep session: refused where only one peer would load,
/// taken where every peer does (setup, simulation systems).
#[cfg(test)]
mod lockstep_tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use crate::net::Desync;
    use crate::net::Net;
    use crate::net::NetState;
    use crate::{ctx::Ctx, game::Game, reflect::WorldReflect};
    use ponko_ecs::{Component, World};

    #[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Component)]
    struct Tag(u32);

    /// A scene in `assets/` (for `load_scene`) and the same world in a plain
    /// file in the working folder (for the control endpoint's `world.load`,
    /// which takes paths relative to it), deleted afterwards.
    struct Files {
        asset: &'static str,
        plain: String,
    }
    impl Files {
        fn new(asset: &'static str) -> Self {
            let mut world = World::new();
            world.reflect::<Tag>();
            world.spawn((Tag(7),));
            let text = serde_json::to_string(&world.save()).unwrap();
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            std::fs::write(root.join("assets").join(asset), &text).unwrap();
            let plain = format!("ponko-test-{}-{asset}", std::process::id());
            std::fs::write(&plain, &text).unwrap();
            Self { asset, plain }
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            let _ = std::fs::remove_file(root.join("assets").join(self.asset));
            let _ = std::fs::remove_file(&self.plain);
        }
    }

    fn tags(ctx: &mut Ctx) -> Vec<u32> {
        let mut t: Vec<u32> = ctx.world.query::<&Tag>().iter().map(|t| t.0).collect();
        t.sort_unstable();
        t
    }

    /// Every way to load that only this peer would make: each must fail and
    /// leave the world as it was.
    fn assert_every_local_load_is_refused(ctx: &mut Ctx, files: &Files) {
        let before = WorldReflect::save(&ctx.world);
        let plain = files.plain.as_str();
        let errors: [String; 4] = [
            ctx.load_scene(files.asset).unwrap_err().into(),
            ctx.load_scene_lockstep(files.asset).unwrap_err().into(),
            crate::control::handle(ctx, "scene.load", &json!({ "path": files.asset })).unwrap_err(),
            crate::control::handle(ctx, "world.load", &json!({ "path": plain })).unwrap_err(),
        ];
        for e in &errors {
            assert!(e.contains("lockstep session"), "{e}");
        }
        // The editor's Stop (with the inspector).
        #[cfg(feature = "inspector")]
        {
            let mut editor = crate::editor::Editor::default();
            editor.play(ctx);
            let extra = ctx.world.spawn((Tag(100),));
            let err = editor.stop(ctx).unwrap_err();
            assert_eq!(err.kind(), crate::error::ErrorKind::Unsupported);
            assert!(err.message().contains("lockstep session"), "{err}");
            assert!(editor.playing(), "Stop can be retried");
            assert!(!ctx.time.is_paused(), "a refused Stop doesn't pause the simulation");
            ctx.world.despawn(extra);
        }
        assert_eq!(WorldReflect::save(&ctx.world), before, "nothing changed");
    }

    /// Runs two loopback peers for `ticks`; returns each one's tags and desync.
    fn two_peers(
        ticks: u64,
        setup: fn(&mut Ctx, &Files),
        tick: fn(&mut Ctx, &Files),
        files: &std::sync::Arc<Files>,
    ) -> Vec<(Vec<u32>, Option<Desync>)> {
        let runs: Vec<_> = Net::loopback(2)
            .into_iter()
            .map(|net| {
                let (f1, f2) = (files.clone(), files.clone());
                std::thread::spawn(move || {
                    let mut world = Game::new("t", 1, 1)
                        .lockstep(net.checksum_every(5))
                        .sim_system(move |ctx: &mut Ctx| {
                            ctx.net.set_input(&(ctx.time.sim_tick() as u32));
                            tick(ctx, &f1);
                            if ctx.time.sim_tick() + 1 == ticks {
                                let desync = ctx.net.desync().cloned();
                                ctx.world.insert_resource(Some(desync));
                            }
                        })
                        .run_headless(ticks, move |ctx| {
                            ctx.world.reflect::<Tag>();
                            ctx.world.spawn((Tag(1),));
                            setup(ctx, &f2);
                        });
                    let desync = world
                        .remove_resource::<Option<Option<Desync>>>()
                        .expect("every tick ran");
                    let mut t: Vec<u32> = world.query::<&Tag>().iter().map(|t| t.0).collect();
                    t.sort_unstable();
                    (t, desync.flatten())
                })
            })
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    }

    #[test]
    fn loads_outside_simulation_systems_are_refused_while_a_session_runs() {
        let files = std::sync::Arc::new(Files::new("test-scene-lockstep-running.json"));
        let runs = two_peers(
            20,
            |_, _| {},
            |ctx, files| {
                if ctx.time.sim_tick() == 8 {
                    assert_eq!(ctx.net.state(), NetState::Running);
                    // As in a frame, late or draw system, or a tool between ticks.
                    ctx.set_in_sim(false);
                    assert_every_local_load_is_refused(ctx, files);
                    ctx.set_in_sim(true);
                    assert_eq!(tags(ctx), [1]);
                    ctx.load_scene(files.asset).unwrap();
                }
            },
            &files,
        );
        for (tags, desync) in runs {
            assert_eq!(desync, None);
            assert_eq!(tags, [7], "the scene, on both peers");
        }
    }

    #[test]
    fn a_frame_systems_load_while_connecting_is_refused() {
        let files = std::sync::Arc::new(Files::new("test-scene-lockstep-connecting.json"));
        let runs = two_peers(
            10,
            |ctx, files| {
                assert_eq!(ctx.net.state(), NetState::Connecting);
                // As in a frame system while the peers are still connecting.
                ctx.set_in_setup(false);
                assert_every_local_load_is_refused(ctx, files);
                ctx.set_in_setup(true);
            },
            |_, _| {},
            &files,
        );
        for (tags, desync) in runs {
            assert_eq!(desync, None);
            assert_eq!(tags, [1], "nothing loaded");
        }
    }

    #[test]
    fn setup_and_simulation_loads_still_work_on_every_peer() {
        let files = std::sync::Arc::new(Files::new("test-scene-lockstep-works.json"));
        let runs = two_peers(
            20,
            |ctx, files| {
                assert_eq!(ctx.net.state(), NetState::Connecting);
                ctx.load_scene(files.asset).unwrap();
                assert_eq!(tags(ctx), [7]);
                ctx.world.query::<&mut Tag>().for_each(|t| t.0 = 2);
            },
            |ctx, files| {
                if ctx.time.sim_tick() == 8 {
                    assert_eq!(tags(ctx), [2]);
                    ctx.load_scene(files.asset).unwrap();
                }
            },
            &files,
        );
        for (tags, desync) in runs {
            assert_eq!(desync, None, "both peers loaded at the same tick");
            assert_eq!(tags, [7]);
        }
    }
}
