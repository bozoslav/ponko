//! Save migrations: the game's own save version, and the steps that bring
//! an older save's components and resources up to date before it loads.
//!
//! The engine reads its own older save layouts by itself; what the game's
//! components look like is the game's business. [`SaveMigrations`] records
//! the game's save version (written into every world save as
//! `"game_version"`) and, for each older version, a [`Migration`]: the
//! renames, field changes, splits and merges that turn a save of that
//! version into one of the next. [`WorldReflect::load`](super::WorldReflect::load)
//! (and everything built on it: save slots, scenes, the editor, the control
//! endpoint) runs them on the save's JSON before anything is checked or
//! loaded.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use ponko_ecs::World;
use serde_json::{Map, Value};

use super::{ReflectError, SAVE_FORMAT};

/// Key of the game's save version in a world save.
pub(crate) const GAME_VERSION: &str = "game_version";

type ValueFn = Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>;
type EntityFn = Arc<dyn Fn(&mut Map<String, Value>) -> Result<(), String> + Send + Sync>;
type SaveFn = Arc<dyn Fn(&mut Value) -> Result<(), String> + Send + Sync>;

/// One change of a [`Migration`].
#[derive(Clone)]
enum Op {
    RenameComponent {
        from: String,
        to: String,
    },
    RemoveComponent(String),
    ConvertComponent {
        component: String,
        convert: ValueFn,
    },
    RenameField {
        component: String,
        from: String,
        to: String,
    },
    AddField {
        component: String,
        field: String,
        value: Value,
    },
    RemoveField {
        component: String,
        field: String,
    },
    ConvertField {
        component: String,
        field: String,
        convert: ValueFn,
    },
    SplitComponent {
        from: String,
        parts: Vec<(String, Vec<String>)>,
    },
    MergeComponents {
        from: Vec<String>,
        into: String,
    },
    Entities(EntityFn),
    RenameResource {
        from: String,
        to: String,
    },
    RemoveResource(String),
    ConvertResource {
        resource: String,
        convert: ValueFn,
    },
    Save(SaveFn),
}

impl fmt::Debug for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RenameComponent { from, to } => write!(f, "rename_component({from:?}, {to:?})"),
            Self::RemoveComponent(name) => write!(f, "remove_component({name:?})"),
            Self::ConvertComponent { component, .. } => write!(f, "convert_component({component:?}, ..)"),
            Self::RenameField { component, from, to } => {
                write!(f, "rename_field({component:?}, {from:?}, {to:?})")
            }
            Self::AddField {
                component,
                field,
                value,
            } => {
                write!(f, "add_field({component:?}, {field:?}, {value})")
            }
            Self::RemoveField { component, field } => write!(f, "remove_field({component:?}, {field:?})"),
            Self::ConvertField { component, field, .. } => {
                write!(f, "convert_field({component:?}, {field:?}, ..)")
            }
            Self::SplitComponent { from, parts } => write!(f, "split_component({from:?}, {parts:?})"),
            Self::MergeComponents { from, into } => write!(f, "merge_components({from:?}, {into:?})"),
            Self::Entities(_) => write!(f, "entities(..)"),
            Self::RenameResource { from, to } => write!(f, "rename_resource({from:?}, {to:?})"),
            Self::RemoveResource(name) => write!(f, "remove_resource({name:?})"),
            Self::ConvertResource { resource, .. } => write!(f, "convert_resource({resource:?}, ..)"),
            Self::Save(_) => write!(f, "save(..)"),
        }
    }
}

/// The changes that turn a save of one game save version into one of the
/// next, in the order they were added. Register it with
/// [`SaveMigrations::step`].
///
/// Component and field names are the JSON ones: a component's reflected
/// name (its type name without the module path) and its serde field names.
/// Each change skips entities that don't have the component (and field
/// changes skip components that don't have the field), so a migration
/// suits every save of its version.
///
/// ```
/// use ponko::reflect::Migration;
/// use serde_json::json;
///
/// // Version 1 to 2: `Hp` became `Health`, and its `max` went from
/// // hit points to hearts (four points each).
/// let m = Migration::new()
///     .rename_component("Hp", "Health")
///     .convert_field("Health", "max", |v| {
///         let points = v.as_f64().ok_or("expected a number")?;
///         Ok(json!((points / 4.0).ceil()))
///     });
/// # let _ = m;
/// ```
#[derive(Clone, Default, Debug)]
#[must_use]
pub struct Migration {
    ops: Vec<Op>,
}

impl Migration {
    /// A migration that changes nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn with(mut self, op: Op) -> Self {
        self.ops.push(op);
        self
    }

    /// Component `from` is called `to` now. An entity that already has
    /// `to` as well fails the load: rename or remove one of them first.
    pub fn rename_component(self, from: &str, to: &str) -> Self {
        self.with(Op::RenameComponent {
            from: from.into(),
            to: to.into(),
        })
    }

    /// Component `name` is gone: it is dropped from every entity. An entity
    /// left with no components is dropped from the save.
    pub fn remove_component(self, name: &str) -> Self {
        self.with(Op::RemoveComponent(name.into()))
    }

    /// Replaces each value of component `component` with what `convert`
    /// makes of it: for a component whose JSON is not an object (a tuple
    /// struct, an enum), or changes that touch several fields at once.
    /// An `Err` fails the load with its message.
    pub fn convert_component(
        self,
        component: &str,
        convert: impl Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Self {
        self.with(Op::ConvertComponent {
            component: component.into(),
            convert: Arc::new(convert),
        })
    }

    /// Field `from` of component `component` is called `to` now.
    pub fn rename_field(self, component: &str, from: &str, to: &str) -> Self {
        self.with(Op::RenameField {
            component: component.into(),
            from: from.into(),
            to: to.into(),
        })
    }

    /// Component `component` has a new field `field`: older saves get
    /// `value` for it (components that have it keep theirs). The same as a
    /// `#[serde(default = ...)]`, but kept with the version that added it.
    pub fn add_field(self, component: &str, field: &str, value: Value) -> Self {
        self.with(Op::AddField {
            component: component.into(),
            field: field.into(),
            value,
        })
    }

    /// Component `component` lost its field `field`: it is dropped from
    /// older saves (serde would refuse it with `deny_unknown_fields`, and
    /// otherwise ignore it silently).
    pub fn remove_field(self, component: &str, field: &str) -> Self {
        self.with(Op::RemoveField {
            component: component.into(),
            field: field.into(),
        })
    }

    /// Field `field` of component `component` changed its type or meaning:
    /// `convert` turns each saved value into the new one. An `Err` fails
    /// the load with its message.
    pub fn convert_field(
        self,
        component: &str,
        field: &str,
        convert: impl Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Self {
        self.with(Op::ConvertField {
            component: component.into(),
            field: field.into(),
            convert: Arc::new(convert),
        })
    }

    /// Component `from` became several: each `(name, fields)` part gets the
    /// listed fields of `from` (those it has), and `from` is removed; fields
    /// no part lists are dropped. A part may keep the name `from`. Rename
    /// fields after the split if they changed too. An entity that already
    /// has a part's component fails the load.
    ///
    /// ```
    /// # use ponko::reflect::Migration;
    /// let m = Migration::new()
    ///     .split_component("Stats", &[("Health", &["hp", "max_hp"]), ("Speed", &["speed"])]);
    /// # let _ = m;
    /// ```
    pub fn split_component(self, from: &str, parts: &[(&str, &[&str])]) -> Self {
        let parts = parts
            .iter()
            .map(|(name, fields)| (name.to_string(), fields.iter().map(|f| f.to_string()).collect()))
            .collect();
        self.with(Op::SplitComponent {
            from: from.into(),
            parts,
        })
    }

    /// Components `from` became one, `into`, with the fields of all of
    /// them (and of `into`, if the entity has it already). The sources are
    /// removed. Each must be an object, and two of them having the same
    /// field fails the load: rename it first.
    pub fn merge_components(self, from: &[&str], into: &str) -> Self {
        self.with(Op::MergeComponents {
            from: from.iter().map(|s| s.to_string()).collect(),
            into: into.into(),
        })
    }

    /// Runs `change` on every saved entity's components (an object keyed
    /// by component name), for what the other changes don't cover, such as
    /// a field that moves between components depending on another's value.
    /// An entity left with no components is dropped from the save.
    pub fn entities(
        self,
        change: impl Fn(&mut Map<String, Value>) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.with(Op::Entities(Arc::new(change)))
    }

    /// Reflected resource `from` is called `to` now.
    pub fn rename_resource(self, from: &str, to: &str) -> Self {
        self.with(Op::RenameResource {
            from: from.into(),
            to: to.into(),
        })
    }

    /// Reflected resource `name` is gone from the game.
    pub fn remove_resource(self, name: &str) -> Self {
        self.with(Op::RemoveResource(name.into()))
    }

    /// Replaces the saved value of resource `resource` with what `convert`
    /// makes of it.
    pub fn convert_resource(
        self,
        resource: &str,
        convert: impl Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Self {
        self.with(Op::ConvertResource {
            resource: resource.into(),
            convert: Arc::new(convert),
        })
    }

    /// Runs `change` on the whole save (`"entities"`, `"resources"`, the
    /// format of [`WorldReflect::save`](super::WorldReflect::save)): the
    /// last resort, for example to spawn an entity that older saves lack.
    pub fn save(self, change: impl Fn(&mut Value) -> Result<(), String> + Send + Sync + 'static) -> Self {
        self.with(Op::Save(Arc::new(change)))
    }

    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    fn apply(&self, save: &mut Value) -> Result<(), String> {
        for op in &self.ops {
            apply(op, save)?;
        }
        if let Some(Value::Array(entities)) = save.get_mut("entities") {
            entities.retain(|e| {
                e.get("components")
                    .and_then(Value::as_object)
                    .is_none_or(|c| !c.is_empty())
            });
        }
        Ok(())
    }
}

/// The game's save version and the [`Migration`]s from each older one: put
/// it in the world as a resource, before the first load.
///
/// World saves ([`WorldReflect::save`](super::WorldReflect::save), save
/// slots, scenes) record the version as `"game_version"`; saves without
/// one, from before the game had migrations, are version 0. Loading a save
/// of an older version runs the steps from its version up, one version at
/// a time, on the save's JSON; then the load goes on as usual (checking
/// every component before changing the world). Versions without a step
/// need no change. A save of a newer version than the game's is refused
/// with [`ReflectError::NewerSave`], since this build can't know what
/// changed.
///
/// ```
/// use ponko::prelude::*;
/// use ponko::reflect::{Migration, SaveMigrations};
/// use serde::{Deserialize, Serialize};
/// use serde_json::json;
///
/// #[derive(Component, Serialize, Deserialize)]
/// struct Health { now: f32, max: f32 }
///
/// let mut world = World::new();
/// world.reflect::<Health>();
/// world.insert_resource(
///     SaveMigrations::new(2)
///         // 0 -> 1: the component was called `Hp`.
///         .step(0, Migration::new().rename_component("Hp", "Health"))
///         // 1 -> 2: `current` became `now`, and `max` was in tenths.
///         .step(1, Migration::new()
///             .rename_field("Health", "current", "now")
///             .convert_field("Health", "max", |v| Ok(json!(v.as_f64().ok_or("not a number")? / 10.0)))),
/// );
///
/// // A save from before the game had versions.
/// let old = json!({
///     "format": "ponko-world", "version": 1,
///     "entities": [{ "entity": 0, "components": { "Hp": { "current": 3.0, "max": 100.0 } } }],
/// });
/// world.load(&old).unwrap();
/// let hp = world.get::<Health>(Entity::from_bits(0)).unwrap();
/// assert_eq!((hp.now, hp.max), (3.0, 10.0));
/// assert_eq!(world.save()["game_version"], 2);
/// ```
#[derive(Clone, Default, Debug)]
pub struct SaveMigrations {
    version: u32,
    steps: BTreeMap<u32, Migration>,
}

impl SaveMigrations {
    /// The game's save version is `version`, with no migrations yet. Raise
    /// it, and add a [`step`](Self::step) from the old one, whenever a
    /// change to the game's components would break older saves.
    pub fn new(version: u32) -> Self {
        Self {
            version,
            steps: BTreeMap::new(),
        }
    }

    /// The game's save version: what saves record, and the newest version
    /// it loads.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The version the newest registered step migrates from, if any: the
    /// game's version can't go down to it or below.
    pub fn newest_step(&self) -> Option<u32> {
        self.steps.keys().next_back().copied()
    }

    /// Sets the game's save version (for code that registers steps one at
    /// a time, as the C API does).
    ///
    /// # Panics
    /// If a step from `version` or later is registered: it could never run.
    #[track_caller]
    pub fn set_version(&mut self, version: u32) {
        if let Some((&from, _)) = self.steps.range(version..).next() {
            panic!(
                "SaveMigrations::set_version: a step from version {from} is registered, so the version must be above {from}"
            );
        }
        self.version = version;
    }

    /// Adds `migration` as the step from save version `from` to `from + 1`.
    /// A second step from the same version runs after the first.
    ///
    /// # Panics
    /// If `from` is not below the game's [`version`](Self::version).
    #[track_caller]
    pub fn step(mut self, from: u32, migration: Migration) -> Self {
        self.add_step(from, migration);
        self
    }

    /// [`step`](Self::step) on a `&mut`, for a `SaveMigrations` already in
    /// the world.
    ///
    /// # Panics
    /// As `step`.
    #[track_caller]
    pub fn add_step(&mut self, from: u32, migration: Migration) {
        assert!(
            from < self.version,
            "SaveMigrations::step: a step from version {from} can't run in a game whose save version is {}",
            self.version
        );
        self.steps.entry(from).or_default().ops.extend(migration.ops);
    }

    /// `save` as the game's version would have written it: the steps from
    /// its `"game_version"` up applied, and `"game_version"` set. Borrowed
    /// if nothing changes. Anything that isn't a world save comes back as
    /// it is (the load reports it).
    /// [`WorldReflect::load`](super::WorldReflect::load) calls it; call it
    /// yourself to look at or keep a migrated save.
    ///
    /// # Errors
    /// [`ReflectError::NewerSave`] for a save of a newer version than the
    /// game's, [`ReflectError::Migration`] if a step fails, and
    /// [`ReflectError::BadValue`] if `"game_version"` isn't a version.
    pub fn migrate<'a>(&self, save: &'a Value) -> Result<Cow<'a, Value>, ReflectError> {
        if save.get("format").and_then(Value::as_str) != Some(SAVE_FORMAT) {
            return Ok(Cow::Borrowed(save));
        }
        let saved = saved_version(save)?;
        if saved > self.version {
            return Err(ReflectError::NewerSave {
                saved,
                current: self.version,
            });
        }
        if saved == self.version {
            return Ok(Cow::Borrowed(save));
        }
        let mut out = save.clone();
        for (&from, step) in self.steps.range(saved..self.version) {
            step.apply(&mut out)
                .map_err(|message| ReflectError::Migration { from, message })?;
        }
        out[GAME_VERSION] = self.version.into();
        Ok(Cow::Owned(out))
    }
}

/// The `"game_version"` of `save`: 0 if it has none.
fn saved_version(save: &Value) -> Result<u32, ReflectError> {
    match save.get(GAME_VERSION) {
        None | Some(Value::Null) => Ok(0),
        Some(v) => v
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| ReflectError::BadValue {
                component: "<save>".into(),
                message: format!("\"{GAME_VERSION}\" is {v}, not a version number"),
            }),
    }
}

/// `save` migrated to the game's save version of `world` (version 0 if it
/// has no [`SaveMigrations`]).
pub(crate) fn migrate<'a>(world: &World, save: &'a Value) -> Result<Cow<'a, Value>, ReflectError> {
    match world.get_resource::<SaveMigrations>() {
        Some(m) => m.migrate(save),
        None => SaveMigrations::default().migrate(save),
    }
}

/// The game's save version, to write into a save (none without migrations).
pub(crate) fn game_version(world: &World) -> Option<u32> {
    world.get_resource::<SaveMigrations>().map(SaveMigrations::version)
}

fn apply(op: &Op, save: &mut Value) -> Result<(), String> {
    match op {
        Op::Save(change) => change(save),
        Op::RenameResource { from, to } => {
            if let Some(resources) = resources(save)?
                && let Some(value) = resources.remove(from)
            {
                if resources.contains_key(to) {
                    return Err(format!("the save has both resources `{from}` and `{to}`"));
                }
                resources.insert(to.clone(), value);
            }
            Ok(())
        }
        Op::RemoveResource(name) => {
            if let Some(resources) = resources(save)? {
                resources.remove(name);
            }
            Ok(())
        }
        Op::ConvertResource { resource, convert } => {
            if let Some(resources) = resources(save)?
                && let Some(value) = resources.get_mut(resource)
            {
                *value = convert(value.take()).map_err(|e| format!("resource `{resource}`: {e}"))?;
            }
            Ok(())
        }
        _ => {
            let Some(entities) = save.get_mut("entities").and_then(Value::as_array_mut) else {
                return Ok(());
            };
            for item in entities {
                let id = item["entity"].clone();
                if let Some(components) = item.get_mut("components").and_then(Value::as_object_mut) {
                    entity_op(op, components).map_err(|e| format!("entity {id}: {e}"))?;
                }
            }
            Ok(())
        }
    }
}

fn resources(save: &mut Value) -> Result<Option<&mut Map<String, Value>>, String> {
    match save.get_mut("resources") {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(_) => Err("\"resources\" is not an object".into()),
    }
}

/// The object of component `name`, if the entity has it.
fn fields<'a>(
    components: &'a mut Map<String, Value>,
    name: &str,
) -> Result<Option<&'a mut Map<String, Value>>, String> {
    match components.get_mut(name) {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(other) => Err(format!(
            "`{name}` is {other}, not an object with fields (use convert_component for it)"
        )),
    }
}

fn entity_op(op: &Op, c: &mut Map<String, Value>) -> Result<(), String> {
    match op {
        Op::RenameComponent { from, to } => {
            if let Some(value) = c.remove(from) {
                if c.contains_key(to) {
                    return Err(format!("it has both `{from}` and `{to}`"));
                }
                c.insert(to.clone(), value);
            }
        }
        Op::RemoveComponent(name) => {
            c.remove(name);
        }
        Op::ConvertComponent { component, convert } => {
            if let Some(value) = c.get_mut(component) {
                *value = convert(value.take()).map_err(|e| format!("`{component}`: {e}"))?;
            }
        }
        Op::RenameField { component, from, to } => {
            if let Some(f) = fields(c, component)?
                && let Some(value) = f.remove(from)
            {
                if f.contains_key(to) {
                    return Err(format!("`{component}` has both fields `{from}` and `{to}`"));
                }
                f.insert(to.clone(), value);
            }
        }
        Op::AddField {
            component,
            field,
            value,
        } => {
            if let Some(f) = fields(c, component)? {
                f.entry(field.clone()).or_insert_with(|| value.clone());
            }
        }
        Op::RemoveField { component, field } => {
            if let Some(f) = fields(c, component)? {
                f.remove(field);
            }
        }
        Op::ConvertField {
            component,
            field,
            convert,
        } => {
            if let Some(f) = fields(c, component)?
                && let Some(value) = f.get_mut(field)
            {
                *value = convert(value.take()).map_err(|e| format!("`{component}.{field}`: {e}"))?;
            }
        }
        Op::SplitComponent { from, parts } => {
            let Some(mut source) = fields(c, from)?.map(std::mem::take) else {
                return Ok(());
            };
            c.remove(from);
            for (name, list) in parts {
                if c.contains_key(name) {
                    return Err(format!("splitting `{from}`: it already has `{name}`"));
                }
                let part: Map<String, Value> = list
                    .iter()
                    .filter_map(|f| source.remove(f).map(|v| (f.clone(), v)))
                    .collect();
                c.insert(name.clone(), Value::Object(part));
            }
        }
        Op::MergeComponents { from, into } => {
            if !from.iter().any(|name| c.contains_key(name)) {
                return Ok(());
            }
            let mut merged = fields(c, into)?.map(std::mem::take).unwrap_or_default();
            for name in from.iter().filter(|n| *n != into) {
                let Some(source) = fields(c, name)?.map(std::mem::take) else {
                    continue;
                };
                c.remove(name);
                for (field, value) in source {
                    if merged.contains_key(&field) {
                        return Err(format!(
                            "merging into `{into}`: field `{field}` is in more than one component"
                        ));
                    }
                    merged.insert(field, value);
                }
            }
            c.insert(into.clone(), Value::Object(merged));
        }
        Op::Entities(change) => change(c)?,
        Op::RenameResource { .. } | Op::RemoveResource(_) | Op::ConvertResource { .. } | Op::Save(_) => {
            unreachable!("handled per save")
        }
    }
    Ok(())
}

impl crate::ctx::Ctx {
    /// Loads every world save (`*.json`) in `folder` the way
    /// [`load_world`](Self::load_world) and [`load_scene`](Self::load_scene)
    /// do, migrations included, then saves the world and loads that again:
    /// the test that a game's [`SaveMigrations`] still bring its old saves up
    /// to date. Keep a save of each released version in a folder of the
    /// game's tests and run it in a headless test after the game's setup
    /// (which registers its components and migrations). Returns how many
    /// saves loaded; the world holds the last one afterwards.
    ///
    /// The folder is read from the file system (relative paths from the
    /// working directory, which `cargo test` sets to the package's folder),
    /// not from the assets or the player's storage. Files are sorted by
    /// name; subfolders are not read.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// # let dir = std::env::temp_dir().join(format!("ponko-doc-old-saves-{}", std::process::id()));
    /// # std::fs::create_dir_all(&dir).unwrap();
    /// # std::fs::write(dir.join("v0.json"), r#"{"format":"ponko-world","version":1,"entities":[]}"#).unwrap();
    /// Game::new("Crabs", 800, 600).run_headless(0, |ctx| {
    ///     // my_game::setup(ctx): reflection and SaveMigrations.
    ///     assert_eq!(ctx.check_old_saves(&dir).unwrap(), 1);
    /// });
    /// # let _ = std::fs::remove_dir_all(dir);
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the
    /// folder has no `.json` files (or can't be read), otherwise the first
    /// failure's kind, with every file that failed and why in the message.
    pub fn check_old_saves(&mut self, folder: impl AsRef<std::path::Path>) -> Result<usize, crate::error::Error> {
        use crate::error::{Error, ErrorKind};
        let folder = folder.as_ref();
        let shown = folder.display().to_string();
        let mut files: Vec<_> = std::fs::read_dir(folder)
            .map_err(|e| Error::new(ErrorKind::NotFound, format!("can't read {shown}: {e}")).with_path(&shown))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(Error::new(ErrorKind::NotFound, format!("no .json saves in {shown}")).with_path(&shown));
        }
        let mut failures = Vec::new();
        let mut first_kind = None;
        for path in &files {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let checked = std::fs::read(path)
                .map_err(|e| Error::new(ErrorKind::Io, e.to_string()))
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(Error::from))
                .and_then(|save| self.replace_world_now(&save))
                .and_then(|()| {
                    let again = super::WorldReflect::save(&self.world);
                    self.replace_world_now(&again)
                        .map_err(|e| e.context("loading it again after saving it"))
                });
            if let Err(e) = checked {
                first_kind.get_or_insert(e.kind());
                failures.push(format!("{name}: {}", String::from(e)));
            }
        }
        match first_kind {
            None => Ok(files.len()),
            Some(kind) => Err(Error::new(
                kind,
                format!(
                    "{} of {} old saves in {shown} don't load:\n{}",
                    failures.len(),
                    files.len(),
                    failures.join("\n")
                ),
            )
            .with_path(&shown)),
        }
    }
}
