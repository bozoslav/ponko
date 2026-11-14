//! Reflection: read and write components and resources by name, as JSON.
//!
//! Reflection is opt-in per component type and uses serde, so any type with
//! `#[derive(Serialize, Deserialize)]` can be reflected. It is what tools use
//! to look inside a running game: the agent (MCP) bridge, the inspector, and
//! save/load.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use ponko_ecs::{Component, ComponentId, Entity, World};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

mod animation;
mod builtin_schemas;
mod migrate;
mod particles;
pub(crate) use animation::reflect_animation;
pub(crate) use migrate::migrate as migrate_save;
pub use migrate::{Migration, SaveMigrations};
pub(crate) use particles::reflect_particle_emitter2d;

use crate::schema::{SchemaSource, schema_json};

/// Why a reflection call failed.
///
/// ```
/// # use ponko::prelude::*;
/// let world = World::new();
/// let e = Entity::from_bits(0);
/// let err = world.get_json(e, "Health").unwrap_err();
/// assert_eq!(err, ponko::reflect::ReflectError::UnknownComponent("Health".into()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReflectError {
    /// No reflected component has this name.
    UnknownComponent(String),
    /// The entity was despawned (or never existed).
    NoSuchEntity(Entity),
    /// The entity is alive but doesn't have this component.
    MissingComponent {
        /// The entity.
        entity: Entity,
        /// The component's reflected name.
        component: String,
    },
    /// The JSON doesn't fit the component type, or the value can't be written as JSON.
    BadValue {
        /// The component's reflected name.
        component: String,
        /// What didn't fit.
        message: String,
    },
    /// No reflected resource has this name.
    UnknownResource(String),
    /// The resource is reflected but not in the world right now.
    MissingResource(String),
    /// The save was written by a newer version of the game: its
    /// `"game_version"` is above the game's [`SaveMigrations::version`].
    NewerSave {
        /// The save's game version.
        saved: u32,
        /// The newest game version this build reads.
        current: u32,
    },
    /// A [`Migration`] step failed on the save.
    Migration {
        /// The version the failing step migrates from (to `from + 1`).
        from: u32,
        /// What failed, with the entity or resource.
        message: String,
    },
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownComponent(name) => write!(
                f,
                "no reflected component named `{name}` (register it with `world.reflect::<T>()`)"
            ),
            Self::NoSuchEntity(e) => write!(f, "entity {} does not exist", e.to_bits()),
            Self::MissingComponent { entity, component } => {
                write!(f, "entity {} has no `{component}`", entity.to_bits())
            }
            Self::BadValue { component, message } => write!(f, "bad value for `{component}`: {message}"),
            Self::UnknownResource(name) => write!(
                f,
                "no reflected resource named `{name}` (register it with `world.reflect_resource::<T>()`)"
            ),
            Self::MissingResource(name) => write!(f, "resource `{name}` is not in the world"),
            Self::NewerSave { saved, current } => write!(
                f,
                "the save is from a newer version of the game (game save version {saved}; this build reads up to {current})"
            ),
            Self::Migration { from, message } => write!(
                f,
                "migrating the save from game save version {from} to {}: {message}",
                from + 1
            ),
        }
    }
}

impl std::error::Error for ReflectError {}

type GetFn = Arc<dyn Fn(&World, Entity) -> Option<Result<Value, String>> + Send + Sync>;
type SetFn = Arc<dyn Fn(&mut World, Entity, Value) -> Result<(), String> + Send + Sync>;
type SpawnFn = Arc<dyn Fn(&mut World, Value) -> Result<Entity, String> + Send + Sync>;
type RemoveFn = Arc<dyn Fn(&mut World, Entity) -> bool + Send + Sync>;
type CheckFn = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;
type SpawnAtFn = Arc<dyn Fn(&mut World, Entity, Value) -> Result<bool, String> + Send + Sync>;

/// `format` field of world saves.
const SAVE_FORMAT: &str = "ponko-world";
/// Version of the save layout; bumped when it changes incompatibly.
const SAVE_VERSION: u64 = 1;
/// Entity slots a save may use beyond its own entities, for the gaps that
/// despawns leave: 2^22 slots, about 64 MiB of slot table and free list at worst.
const LOAD_SPARE_SLOTS: usize = 1 << 22;
/// Entity slots a save may use per entity it holds, on top of the spare ones.
const LOAD_SLOTS_PER_ENTITY: usize = 8;

/// How many times [`WorldReflect::load`] has replaced this world's entities.
/// A private resource, never saved: services that link outside state to
/// entity ids (physics bodies) compare it to the count they last saw and
/// drop those links when it moved, because a load may give an old id to a
/// different entity.
pub(crate) struct WorldLoads(pub(crate) u64);

/// The [`WorldLoads`] count of `world` (0 if it was never loaded into).
pub(crate) fn world_loads(world: &World) -> u64 {
    world.get_resource::<WorldLoads>().map_or(0, |l| l.0)
}

/// Leaves its entity out of world saves: [`WorldReflect::save`] skips
/// every entity that has it, and so do the saves built on it
/// ([`Storage::save_world`](crate::storage::Storage::save_world),
/// [`save_scene`](crate::assets::Assets::save_scene), the editor and the
/// control endpoint's `world.save`). Loading a save replaces every entity,
/// these too, so the game spawns them again after a load.
///
/// For what the game rebuilds from its own files rather than from a save:
/// [`Ctx::spawn_level`](crate::ctx::Ctx::spawn_level) puts it on everything
/// it spawns except the objects spawned as prefabs (see
/// [`LevelOptions::without_saved_objects`](crate::d2::LevelOptions::without_saved_objects)).
/// It isn't reflected itself, so it is never in a save, scene or prefab.
///
/// ```
/// use ponko::prelude::*;
/// use ponko::reflect::NotSaved;
///
/// let mut world = World::new();
/// world.reflect::<ponko::d2::Transform>();
/// world.spawn((ponko::d2::Transform::at(vec2(1.0, 2.0)),)); // the player
/// world.spawn((ponko::d2::Transform::default(), NotSaved)); // scenery from a file
/// assert_eq!(world.save()["entities"].as_array().unwrap().len(), 1);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Component)]
pub struct NotSaved;

/// What reflection knows about one component type.
#[derive(Clone)]
struct Reflected {
    name: String,
    /// The Rust type name, or the component name for field-described components.
    type_name: String,
    get: GetFn,
    set: SetFn,
    spawn: SpawnFn,
    spawn_at: SpawnAtFn,
    check: CheckFn,
    remove: RemoveFn,
}

type ResGetFn = Arc<dyn Fn(&World) -> Option<Result<Value, String>> + Send + Sync>;
type ResSetFn = Arc<dyn Fn(&mut World, Value) -> Result<(), String> + Send + Sync>;

/// What reflection knows about one resource type.
#[derive(Clone)]
struct ReflectedResource {
    name: String,
    type_name: String,
    get: ResGetFn,
    set: ResSetFn,
    check: CheckFn,
}

/// The reflected components and resources of a world, stored in it as a resource.
#[derive(Default)]
struct Registry {
    list: Vec<Reflected>,
    by_name: HashMap<String, usize>,
    resources: Vec<ReflectedResource>,
    resource_by_name: HashMap<String, usize>,
    /// Field layouts of components described with `reflect_fields`, by id:
    /// how tools read C events of those types.
    layouts: HashMap<ComponentId, Arc<FieldLayout>>,
    /// Known schemas of reflected components, by name (opt-in; see `crate::schema`).
    schemas: HashMap<String, (ValueSchema, SchemaSource)>,
    /// Known schemas of reflected resources, by name.
    resource_schemas: HashMap<String, (ValueSchema, SchemaSource)>,
}

/// The name of foreign component `id`, if it's described with `reflect_fields`.
pub(crate) fn raw_name(world: &World, id: ComponentId) -> Option<&str> {
    Some(registry(world)?.layouts.get(&id)?.component.as_str())
}

/// The name and JSON of a value of foreign component `id` (one described
/// with `reflect_fields`), or `None` if it isn't described.
///
/// # Safety
/// `p` points to a value of that component (its registered size, readable).
pub(crate) unsafe fn raw_json(world: &World, id: ComponentId, p: *const u8) -> Option<(&str, Value)> {
    let layout = registry(world)?.layouts.get(&id)?;
    // SAFETY: forwarded.
    Some((layout.component.as_str(), unsafe { layout.read(p) }))
}

fn get_resource<R: Send + Sync + 'static + Serialize>(world: &World) -> Option<Result<Value, String>> {
    world
        .get_resource::<R>()
        .map(|v| serde_json::to_value(v).map_err(|err| err.to_string()))
}

fn set_resource<R: Send + Sync + 'static + DeserializeOwned>(world: &mut World, value: Value) -> Result<(), String> {
    let value: R = serde_json::from_value(value).map_err(|err| err.to_string())?;
    world.insert_resource(value);
    Ok(())
}

fn get<T: Component + Serialize>(world: &World, e: Entity) -> Option<Result<Value, String>> {
    world
        .get::<T>(e)
        .map(|v| serde_json::to_value(v).map_err(|err| err.to_string()))
}

fn set<T: Component + DeserializeOwned>(world: &mut World, e: Entity, value: Value) -> Result<(), String> {
    let value: T = serde_json::from_value(value).map_err(|err| err.to_string())?;
    world.insert(e, value);
    Ok(())
}

fn spawn<T: Component + DeserializeOwned>(world: &mut World, value: Value) -> Result<Entity, String> {
    let value: T = serde_json::from_value(value).map_err(|err| err.to_string())?;
    Ok(world.spawn((value,)))
}

fn spawn_at<T: Component + DeserializeOwned>(world: &mut World, e: Entity, value: Value) -> Result<bool, String> {
    let value: T = serde_json::from_value(value).map_err(|err| err.to_string())?;
    Ok(world.spawn_at(e, (value,)))
}

fn check<T: DeserializeOwned>(value: &Value) -> Result<(), String> {
    T::deserialize(value).map(drop).map_err(|err| err.to_string())
}

fn remove<T: Component>(world: &mut World, e: Entity) -> bool {
    world.remove::<T>(e).is_some()
}

/// `crate::game::Score<u32>` → `Score<u32>`: the last path segment, keeping generics.
pub(crate) fn short_name(type_name: &str) -> String {
    let end = type_name.find('<').unwrap_or(type_name.len());
    let start = type_name[..end].rfind("::").map_or(0, |i| i + 2);
    type_name[start..].to_string()
}

/// Fields present in `patch` replace those in `base`; nested objects merge
/// the same way. Anything else (numbers, arrays, ...) is replaced whole.
fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (k, v) in patch {
                match base.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        base.insert(k, v);
                    }
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

/// The type of a field in [`WorldReflect::reflect_fields`], and how it looks
/// in JSON.
///
/// ```
/// # use ponko::prelude::*;
/// assert_eq!(ponko::reflect::FieldType::Vec2.size(), 8);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
#[non_exhaustive]
pub enum FieldType {
    /// A JSON number.
    F32,
    /// A JSON number.
    F64,
    /// A JSON integer.
    I8,
    /// A JSON integer.
    I16,
    /// A JSON integer.
    I32,
    /// A JSON integer.
    I64,
    /// A JSON integer, 0 to 255.
    U8,
    /// A JSON integer, 0 to 65535.
    U16,
    /// A JSON integer, 0 or more.
    U32,
    /// A JSON integer, 0 or more.
    U64,
    /// One byte, 0 or 1; `true`/`false` in JSON (other bytes read as `true`).
    Bool,
    /// Two `f32`, `[x, y]` in JSON like [`Vec2`](glam::Vec2).
    Vec2,
    /// Four `f32`, `{ "r", "g", "b", "a" }` in JSON.
    Color,
    /// An entity id as 64 bits, a JSON number like a reflected [`Entity`].
    Entity,
}

impl FieldType {
    /// Bytes the field takes.
    #[must_use]
    pub fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 | Self::Bool => 1,
            Self::I16 | Self::U16 => 2,
            Self::F32 | Self::I32 | Self::U32 => 4,
            Self::F64 | Self::I64 | Self::U64 | Self::Entity | Self::Vec2 => 8,
            Self::Color => 16,
        }
    }
}

/// One field of a component described with [`WorldReflect::reflect_fields`].
///
/// ```
/// # use ponko::prelude::*;
/// let f = ponko::reflect::Field::new("speed", ponko::reflect::FieldType::F32, 8);
/// assert_eq!((f.name.as_str(), f.offset), ("speed", 8));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The JSON key.
    pub name: String,
    /// How the bytes are read and written.
    pub ty: FieldType,
    /// Byte offset in the component.
    pub offset: usize,
}

impl Field {
    /// A field named `name` (the JSON key) of type `ty` at byte `offset`.
    pub fn new(name: impl Into<String>, ty: FieldType, offset: usize) -> Self {
        Self {
            name: name.into(),
            ty,
            offset,
        }
    }
}

/// Checked fields of one component, and its size.
struct FieldLayout {
    component: String,
    size: usize,
    fields: Vec<Field>,
}

macro_rules! num {
    ($p:expr, $t:ty) => {
        // SAFETY: the caller's pointer covers the field (checked in `new`); any bits are a valid $t.
        Value::from(unsafe { $p.cast::<$t>().read_unaligned() })
    };
}

impl FieldLayout {
    #[track_caller]
    fn new(component: &str, size: usize, fields: &[Field]) -> Self {
        let mut names = std::collections::HashSet::new();
        for f in fields {
            assert!(
                f.offset.checked_add(f.ty.size()).is_some_and(|end| end <= size),
                "World::reflect_fields: field `{}` of `{component}` (offset {}, {} bytes) doesn't fit in {size} bytes",
                f.name,
                f.offset,
                f.ty.size()
            );
            assert!(
                names.insert(f.name.as_str()),
                "World::reflect_fields: `{component}` has two fields named `{}`",
                f.name
            );
        }
        Self {
            component: component.to_string(),
            size,
            fields: fields.to_vec(),
        }
    }

    /// The value at `p` as JSON.
    ///
    /// # Safety
    /// `p` points to `size` readable bytes.
    unsafe fn read(&self, p: *const u8) -> Value {
        let mut out = serde_json::Map::with_capacity(self.fields.len());
        for f in &self.fields {
            // SAFETY: inside the value (checked in `new`).
            let p = unsafe { p.add(f.offset) };
            let v = match f.ty {
                FieldType::F32 => num!(p, f32),
                FieldType::F64 => num!(p, f64),
                FieldType::I8 => num!(p, i8),
                FieldType::I16 => num!(p, i16),
                FieldType::I32 => num!(p, i32),
                FieldType::I64 => num!(p, i64),
                FieldType::U8 => num!(p, u8),
                FieldType::U16 => num!(p, u16),
                FieldType::U32 => num!(p, u32),
                FieldType::U64 | FieldType::Entity => num!(p, u64),
                // SAFETY: as in `num!`; read as a byte, so any value is fine.
                FieldType::Bool => Value::from(unsafe { p.read() } != 0),
                FieldType::Vec2 => {
                    // SAFETY: as in `num!`.
                    let [x, y] = [0, 4].map(|o| unsafe { p.add(o).cast::<f32>().read_unaligned() });
                    serde_json::json!([x, y])
                }
                FieldType::Color => {
                    // SAFETY: as in `num!`.
                    let [r, g, b, a] = [0, 4, 8, 12].map(|o| unsafe { p.add(o).cast::<f32>().read_unaligned() });
                    serde_json::json!({ "r": r, "g": g, "b": b, "a": a })
                }
            };
            out.insert(f.name.clone(), v);
        }
        Value::Object(out)
    }

    /// The bytes for `value`: from `current` (or zero) with the given fields written.
    fn bytes(&self, current: Option<*const u8>, value: &Value) -> Result<Vec<u8>, String> {
        let Value::Object(map) = value else {
            return Err(format!("expected an object with the fields of `{}`", self.component));
        };
        let mut bytes = vec![0u8; self.size];
        if let Some(p) = current {
            // SAFETY: `p` is a live value of this component, `size` bytes.
            unsafe { std::ptr::copy_nonoverlapping(p, bytes.as_mut_ptr(), self.size) };
        }
        for (key, v) in map {
            let Some(f) = self.fields.iter().find(|f| f.name == *key) else {
                let known: Vec<&str> = self.fields.iter().map(|f| f.name.as_str()).collect();
                return Err(format!(
                    "`{}` has no field `{key}` (fields: {})",
                    self.component,
                    known.join(", ")
                ));
            };
            let at = &mut bytes[f.offset..f.offset + f.ty.size()];
            write_field(f, v, at).map_err(|e| format!("field `{key}`: {e}"))?;
        }
        Ok(bytes)
    }
}

fn write_field(f: &Field, v: &Value, at: &mut [u8]) -> Result<(), String> {
    fn int<T: TryFrom<i64> + TryFrom<u64>>(v: &Value) -> Result<T, String> {
        let bad = || format!("expected an integer that fits, got {v}");
        match (v.as_i64(), v.as_u64()) {
            (_, Some(u)) => T::try_from(u).map_err(|_| bad()),
            (Some(i), _) => T::try_from(i).map_err(|_| bad()),
            _ => Err(bad()),
        }
    }
    fn float(v: &Value) -> Result<f64, String> {
        v.as_f64().ok_or_else(|| format!("expected a number, got {v}"))
    }
    match f.ty {
        FieldType::F32 => at.copy_from_slice(&(float(v)? as f32).to_ne_bytes()),
        FieldType::F64 => at.copy_from_slice(&float(v)?.to_ne_bytes()),
        FieldType::I8 => at.copy_from_slice(&int::<i8>(v)?.to_ne_bytes()),
        FieldType::I16 => at.copy_from_slice(&int::<i16>(v)?.to_ne_bytes()),
        FieldType::I32 => at.copy_from_slice(&int::<i32>(v)?.to_ne_bytes()),
        FieldType::I64 => at.copy_from_slice(&int::<i64>(v)?.to_ne_bytes()),
        FieldType::U8 => at.copy_from_slice(&int::<u8>(v)?.to_ne_bytes()),
        FieldType::U16 => at.copy_from_slice(&int::<u16>(v)?.to_ne_bytes()),
        FieldType::U32 => at.copy_from_slice(&int::<u32>(v)?.to_ne_bytes()),
        FieldType::U64 => at.copy_from_slice(&int::<u64>(v)?.to_ne_bytes()),
        // Remapped like a Rust `Entity`, so a C prefab part's references follow.
        FieldType::Entity => {
            let e = Entity::from_bits(int::<u64>(v)?).remapped();
            at.copy_from_slice(&e.to_bits().to_ne_bytes());
        }
        FieldType::Bool => at[0] = v.as_bool().ok_or_else(|| format!("expected true or false, got {v}"))? as u8,
        FieldType::Vec2 => {
            let xy = v
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| format!("expected [x, y], got {v}"))?;
            for (i, c) in xy.iter().enumerate() {
                at[i * 4..i * 4 + 4].copy_from_slice(&(float(c)? as f32).to_ne_bytes());
            }
        }
        FieldType::Color => {
            let Value::Object(rgba) = v else {
                return Err(format!("expected {{\"r\", \"g\", \"b\", \"a\"}}, got {v}"));
            };
            for (i, k) in ["r", "g", "b", "a"].iter().enumerate() {
                // Missing channels keep their current value (zero in new values).
                if let Some(c) = rgba.get(*k) {
                    at[i * 4..i * 4 + 4].copy_from_slice(&(float(c)? as f32).to_ne_bytes());
                }
            }
            if let Some(k) = rgba.keys().find(|k| !["r", "g", "b", "a"].contains(&k.as_str())) {
                return Err(format!("colors have r, g, b and a, not `{k}`"));
            }
        }
    }
    Ok(())
}

/// Reflection methods on [`World`]: read and write components by name as JSON.
///
/// Comes in with `use ponko::prelude::*;
pub trait WorldReflect {
    /// Makes component `T` visible to reflection under its type name without
    /// the module path (`my_game::Health` becomes `"Health"`).
    /// Registering the same type again does nothing.
    ///
    /// `Transform2D`, `Sprite`, `Animation` and `Attached` are reflected in
    /// every game by default (a sprite or a sprite-sheet animation with its
    /// texture's file name), and with 3D on,
    /// `Transform3D` and `Model` too (a model with its mesh's file name).
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, Component)]
    /// struct Health { current: f32, max: f32 }
    ///
    /// let mut world = World::new();
    /// world.reflect::<Health>();
    /// let e = world.spawn((Health { current: 3.0, max: 10.0 },));
    /// assert_eq!(world.get_json(e, "Health").unwrap()["current"], 3.0);
    /// ```
    ///
    /// # Panics
    /// If a different type with the same short name is already reflected.
    fn reflect<T: Component + Serialize + DeserializeOwned>(&mut self);

    /// Makes a component that has no Rust type (one registered with
    /// [`RawWorld::register_component`](crate::ecs_core::RawWorld::register_component),
    /// such as a C or C++ component) visible
    /// to reflection, by describing its fields. Its JSON is an object with
    /// one entry per field; bytes no field covers are zero in new values.
    /// Describing the same component again replaces its fields.
    ///
    /// ```
    /// use ponko::prelude::*;
    ///
    /// let mut world = World::new();
    /// // A C struct { float current; uint32_t max; }.
    /// // SAFETY: plain bytes, no destructor.
    /// let id = unsafe { world.raw().register_component("Health", 8, 4, None) };
    /// // SAFETY: any values of these fields make a valid Health.
    /// unsafe {
    ///     world.reflect_fields(id, &[
    ///         ponko::reflect::Field::new("current", ponko::reflect::FieldType::F32, 0),
    ///         ponko::reflect::Field::new("max", ponko::reflect::FieldType::U32, 4),
    ///     ]);
    /// }
    /// let e = world.spawn_json(serde_json::json!({ "Health": { "current": 3.5, "max": 10 } })).unwrap();
    /// assert_eq!(world.get_json(e, "Health").unwrap()["max"], 10);
    /// ```
    ///
    /// # Safety
    /// Any bytes where each field holds a value of its type and every other
    /// byte is zero or unchanged must be a valid value of the component.
    ///
    /// # Panics
    /// If the component has a Rust type or a destructor, a field doesn't fit
    /// in it or two fields share a name, or another type is reflected under
    /// the same name.
    unsafe fn reflect_fields(&mut self, id: ComponentId, fields: &[Field]);

    /// Names of all reflected components, in registration order.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// assert_eq!(world.reflected_components(), ["Transform2D"]);
    /// ```
    fn reflected_components(&self) -> Vec<&str>;

    /// Component `name` of entity `e`, as JSON.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// let e = world.spawn((ponko::d2::Transform::at(vec2(1.0, 2.0)),));
    /// let t = world.get_json(e, "Transform2D").unwrap();
    /// assert_eq!(t["position"], serde_json::json!([1.0, 2.0]));
    /// ```
    ///
    /// # Errors
    ///
    /// If `name` isn't reflected, `e` is dead or lacks it, or the value can't be
    /// written as JSON.
    fn get_json(&self, e: Entity, name: &str) -> Result<Value, ReflectError>;

    /// Writes component `name` of entity `e` from JSON, adding the component
    /// if the entity doesn't have it.
    ///
    /// If the entity has it and `value` is an object, only the fields in
    /// `value` change; the rest keep their values. To add a component,
    /// give every field (or the ones the type has `#[serde(default)]` for).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use serde_json::json;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// let e = world.spawn((ponko::d2::Transform::at(vec2(1.0, 2.0)).rotated(0.5),));
    /// world.set_json(e, "Transform2D", json!({ "position": [5.0, 5.0] })).unwrap();
    /// let t = world.get::<ponko::d2::Transform>(e).unwrap();
    /// assert_eq!((t.position, t.rotation), (vec2(5.0, 5.0), 0.5));
    /// ```
    ///
    /// # Errors
    ///
    /// If `name` isn't reflected, `e` is dead, or `value` doesn't fit the type.
    fn set_json(&mut self, e: Entity, name: &str, value: Value) -> Result<(), ReflectError>;

    /// Removes component `name` from entity `e`. Returns whether it had it.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// let e = world.spawn((ponko::d2::Transform::default(),));
    /// assert_eq!(world.remove_json(e, "Transform2D"), Ok(true));
    /// assert_eq!(world.remove_json(e, "Transform2D"), Ok(false));
    /// ```
    ///
    /// # Errors
    ///
    /// If `name` isn't reflected or `e` is dead.
    fn remove_json(&mut self, e: Entity, name: &str) -> Result<bool, ReflectError>;

    /// Every reflected component of entity `e`, as one JSON object keyed by
    /// component name. Components that aren't reflected are left out.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// let e = world.spawn((ponko::d2::Transform::default(),));
    /// assert!(world.entity_json(e).unwrap().get("Transform2D").is_some());
    /// ```
    ///
    /// # Errors
    ///
    /// If `e` is dead or a component can't be written as JSON.
    fn entity_json(&self, e: Entity) -> Result<Value, ReflectError>;

    /// Spawns an entity from a JSON object keyed by component name, the
    /// format [`entity_json`](Self::entity_json) returns. On error nothing
    /// is left spawned.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use serde_json::json;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// let e = world
    ///     .spawn_json(json!({ "Transform2D": { "position": [3.0, 4.0], "rotation": 0.0, "scale": [1.0, 1.0] } }))
    ///     .unwrap();
    /// assert_eq!(world.get::<ponko::d2::Transform>(e).unwrap().position, vec2(3.0, 4.0));
    /// ```
    ///
    /// # Errors
    ///
    /// If a name isn't reflected or a value doesn't fit its type; nothing is
    /// spawned then.
    fn spawn_json(&mut self, components: Value) -> Result<Entity, ReflectError>;

    /// The whole world as JSON: every entity that has reflected components,
    /// with its id, sorted by id. Components that aren't reflected are not
    /// saved; `"not_saved"` counts them by name, so nothing goes missing
    /// silently. Entities with [`NotSaved`] are left out (and not counted). Reflected resources ([`reflect_resource`](Self::reflect_resource))
    /// are saved under `"resources"`; others are not.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// world.spawn((ponko::d2::Transform::at(vec2(1.0, 2.0)),));
    /// let save = world.save();
    /// assert_eq!(save["entities"].as_array().unwrap().len(), 1);
    /// ```
    fn save(&self) -> Value;

    /// Replaces every entity with those in `save` (from [`save`](Self::save)),
    /// **with the same ids**, so components that refer to entities stay
    /// valid. Reflected resources in the save are written; all other
    /// resources stay. Everything is checked before anything changes: on
    /// error the world is untouched.
    ///
    /// Old handles may match loaded entities afterwards (the save's ids
    /// take over), which is the point.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut world = World::new();
    /// world.reflect::<ponko::d2::Transform>();
    /// let e = world.spawn((ponko::d2::Transform::at(vec2(1.0, 2.0)),));
    /// let save = world.save();
    ///
    /// world.get_mut::<ponko::d2::Transform>(e).unwrap().position = vec2(9.0, 9.0);
    /// world.load(&save).unwrap();
    /// assert_eq!(world.get::<ponko::d2::Transform>(e).unwrap().position, vec2(1.0, 2.0));
    /// ```
    ///
    /// # Errors
    ///
    /// If `save` isn't a world save this version reads, names a component or
    /// resource that isn't reflected, or has a value that doesn't fit its
    /// type. Everything is checked first: on an error the world is unchanged.
    ///
    /// The game's [`SaveMigrations`] run on the save first: a save of an
    /// older game version is brought up to date, one of a newer version is
    /// refused ([`ReflectError::NewerSave`]), and a step that fails is
    /// [`ReflectError::Migration`].
    ///
    /// Also if an entity id's index is out of bounds, so that a corrupt or
    /// hostile file can't make the world allocate a huge entity table. Saved
    /// ids are kept, and the world's slot table grows to the highest index,
    /// so indices must be below the largest of: this world's current
    /// [`entity_slots`](World::entity_slots), or 8 × the number of entities in
    /// the save; plus 4,194,304 (2^22) spare slots for the gaps despawns
    /// leave. A world saved by the engine reuses freed slots, so its highest
    /// index is below the most entities it ever had alive at once: its saves
    /// always load in the same session, and in any session unless it once had
    /// over 4 million entities alive and was then saved with less than an
    /// eighth of them.
    fn load(&mut self, save: &Value) -> Result<(), ReflectError>;

    /// Makes resource `R` visible to reflection under its type name without
    /// the module path, like [`reflect`](Self::reflect) does for components.
    /// Tools (agents through `ponko-mcp`, the control endpoint) can then
    /// read and change it, and world saves include it.
    /// Registering the same type again does nothing.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct Score { red: u32, blue: u32 }
    ///
    /// let mut world = World::new();
    /// world.reflect_resource::<Score>();
    /// world.insert_resource(Score { red: 3, blue: 1 });
    /// assert_eq!(world.resource_json("Score").unwrap()["red"], 3);
    /// ```
    ///
    /// # Panics
    /// If a different type with the same short name is already reflected as a resource.
    fn reflect_resource<R: Send + Sync + 'static + Serialize + DeserializeOwned>(&mut self);

    /// Names of the reflected resources, in registration order (whether or
    /// not they're in the world right now).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # #[derive(serde::Serialize, serde::Deserialize)] struct Score(u32);
    /// let mut world = World::new();
    /// world.reflect_resource::<Score>();
    /// assert_eq!(world.reflected_resources(), ["Score"]);
    /// ```
    fn reflected_resources(&self) -> Vec<&str>;

    /// A reflected resource as JSON.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # #[derive(serde::Serialize, serde::Deserialize)] struct Score(u32);
    /// let mut world = World::new();
    /// world.reflect_resource::<Score>();
    /// assert_eq!(world.resource_json("Score"), Err(ponko::reflect::ReflectError::MissingResource("Score".into())));
    /// world.insert_resource(Score(7));
    /// assert_eq!(world.resource_json("Score").unwrap(), 7);
    /// ```
    ///
    /// # Errors
    ///
    /// If `name` isn't a reflected resource or isn't in the world.
    fn resource_json(&self, name: &str) -> Result<Value, ReflectError>;

    /// Writes a reflected resource from JSON, inserting it if it's missing.
    /// For a resource that exists, an object changes only the fields it
    /// has, like [`set_json`](Self::set_json).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # use serde_json::json;
    /// # #[derive(serde::Serialize, serde::Deserialize)] struct Score { red: u32, blue: u32 }
    /// let mut world = World::new();
    /// world.reflect_resource::<Score>();
    /// world.insert_resource(Score { red: 3, blue: 1 });
    /// world.set_resource_json("Score", json!({"blue": 9})).unwrap();
    /// assert_eq!(world.resource::<Score>().blue, 9);
    /// assert_eq!(world.resource::<Score>().red, 3);
    /// ```
    ///
    /// # Errors
    ///
    /// If `name` isn't a reflected resource, or `value` doesn't fit its type.
    fn set_resource_json(&mut self, name: &str, value: Value) -> Result<(), ReflectError>;

    /// Makes resource `name`, one with no Rust type (stored with
    /// [`RawWorld::insert_resource_dynamic`](ponko_ecs::RawWorld::insert_resource_dynamic), the C and C++ path), visible to
    /// reflection by listing its fields, like
    /// [`reflect_fields`](Self::reflect_fields) does for components.
    /// Describing it again replaces the fields.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// use std::alloc::Layout;
    ///
    /// let mut world = World::new();
    /// // A C struct { uint32_t red, blue; }.
    /// let layout = Layout::from_size_align(8, 4).unwrap();
    /// // SAFETY: any values of these fields make a valid Score.
    /// unsafe {
    ///     world.reflect_resource_fields("Score", layout, &[
    ///         ponko::reflect::Field::new("red", ponko::reflect::FieldType::U32, 0),
    ///         ponko::reflect::Field::new("blue", ponko::reflect::FieldType::U32, 4),
    ///     ]);
    /// }
    /// world.set_resource_json("Score", serde_json::json!({ "red": 3, "blue": 1 })).unwrap();
    /// assert_eq!(world.resource_json("Score").unwrap()["red"], 3);
    /// ```
    ///
    /// # Safety
    /// Any bytes where each field holds a value of its type and every other
    /// byte is zero or unchanged must be a valid value of the resource.
    ///
    /// # Panics
    /// If a field doesn't fit, two fields share a name, or a Rust resource
    /// is reflected under the same name.
    unsafe fn reflect_resource_fields(&mut self, name: &str, layout: std::alloc::Layout, fields: &[Field]);

    /// Tells tools the JSON shape of reflected component `name`, so they can
    /// see its fields before reading or writing it (the control endpoint's
    /// `capabilities`). Optional: without it a component registered with
    /// [`reflect`](Self::reflect) is reported as opaque, because Serde can't
    /// say what a type serializes to. Components described with
    /// [`reflect_fields`](Self::reflect_fields) get theirs from the fields.
    /// Describing again replaces the schema. Nothing checks it against the
    /// type: keep it accurate.
    ///
    /// ```
    /// use ponko::prelude::*;
    /// # #[derive(Component, serde::Serialize, serde::Deserialize)] struct Health { current: f32, max: f32 }
    /// let mut world = World::new();
    /// world.reflect::<Health>();
    /// world
    ///     .describe_component("Health", ponko::reflect::ValueSchema::object([
    ///         ("current", ponko::reflect::ValueSchema::Number),
    ///         ("max", ponko::reflect::ValueSchema::Number),
    ///     ]))
    ///     .unwrap();
    /// ```
    ///
    /// # Errors
    /// If no component named `name` is reflected.
    fn describe_component(&mut self, name: &str, schema: ValueSchema) -> Result<(), ReflectError>;

    /// [`describe_component`](Self::describe_component) for a reflected resource.
    ///
    /// # Errors
    /// If no resource named `name` is reflected.
    fn describe_resource(&mut self, name: &str, schema: ValueSchema) -> Result<(), ReflectError>;
}

/// `Transform2D`'s JSON shape (all fields optional: it has `#[serde(default)]`).
pub(crate) fn describe_transform2d(world: &mut World) {
    let schema = ValueSchema::object([
        ("position", ValueSchema::Vec2),
        ("rotation", ValueSchema::Number),
        ("scale", ValueSchema::Vec2),
    ]);
    let _ = world.describe_component("Transform2D", schema);
}

/// Every reflected component and resource with its schema status, for the
/// control endpoint's `capabilities`.
pub(crate) fn reflected_schemas(world: &World) -> (Vec<Value>, Vec<Value>) {
    let Some(reg) = registry(world) else {
        return (Vec::new(), Vec::new());
    };
    let components = reg
        .list
        .iter()
        .map(|c| json!({ "name": c.name, "schema": schema_json(reg.schemas.get(&c.name)) }))
        .collect();
    let resources = reg
        .resources
        .iter()
        .map(|r| json!({ "name": r.name, "schema": schema_json(reg.resource_schemas.get(&r.name)) }))
        .collect();
    (components, resources)
}

fn find_resource(world: &World, name: &str) -> Result<ReflectedResource, ReflectError> {
    registry(world)
        .and_then(|r| r.resource_by_name.get(name).map(|&i| r.resources[i].clone()))
        .ok_or_else(|| ReflectError::UnknownResource(name.to_string()))
}

fn registry(world: &World) -> Option<&Registry> {
    world.get_resource::<Registry>()
}

fn find(world: &World, name: &str) -> Result<Reflected, ReflectError> {
    registry(world)
        .and_then(|r| r.by_name.get(name).map(|&i| r.list[i].clone()))
        .ok_or_else(|| ReflectError::UnknownComponent(name.to_string()))
}

fn bad_value(component: &str) -> impl FnOnce(String) -> ReflectError + '_ {
    move |message| ReflectError::BadValue {
        component: component.to_string(),
        message,
    }
}

impl WorldReflect for World {
    #[track_caller]
    fn reflect<T: Component + Serialize + DeserializeOwned>(&mut self) {
        let type_name = std::any::type_name::<T>();
        let name = short_name(type_name);
        if self.get_resource::<Registry>().is_none() {
            self.insert_resource(Registry::default());
        }
        let reg = self.resource_mut::<Registry>();
        if let Some(&i) = reg.by_name.get(&name) {
            let existing = &reg.list[i].type_name;
            assert_eq!(
                existing, type_name,
                "World::reflect: `{type_name}` and `{existing}` would both be named `{name}`"
            );
            return;
        }
        if let Some(schema) = builtin_schemas::builtin::<T>() {
            reg.schemas.insert(name.clone(), (schema, SchemaSource::Described));
        }
        reg.by_name.insert(name.clone(), reg.list.len());
        reg.list.push(Reflected {
            name,
            type_name: type_name.to_string(),
            get: Arc::new(get::<T>),
            set: Arc::new(set::<T>),
            spawn: Arc::new(spawn::<T>),
            spawn_at: Arc::new(spawn_at::<T>),
            check: Arc::new(check::<T>),
            remove: Arc::new(remove::<T>),
        });
    }

    #[track_caller]
    unsafe fn reflect_fields(&mut self, id: ComponentId, fields: &[Field]) {
        let info = self.component_info(id);
        let name = info.name().to_string();
        let size = info.layout().size();
        assert!(
            info.type_id().is_none() && !info.needs_drop(),
            "World::reflect_fields: `{name}` is a Rust type or has a destructor; use `reflect::<T>()`"
        );
        let layout = Arc::new(FieldLayout::new(&name, size, fields));
        if self.get_resource::<Registry>().is_none() {
            self.insert_resource(Registry::default());
        }
        let reg = self.resource_mut::<Registry>();
        reg.layouts.insert(id, layout.clone());
        reg.schemas
            .insert(name.clone(), (ValueSchema::from_fields(fields), SchemaSource::Fields));
        let entry = |name: &str| {
            let (l, g, k, st) = (layout.clone(), layout.clone(), layout.clone(), layout.clone());
            let sa = layout.clone();
            Reflected {
                name: name.to_string(),
                type_name: name.to_string(),
                get: Arc::new(move |w: &World, e| {
                    // SAFETY: the pointer is to a live value of `id`, `size` bytes.
                    w.raw_ref().get_dynamic(e, id).map(|p| Ok(unsafe { g.read(p) }))
                }),
                set: Arc::new(move |w: &mut World, e, v| {
                    let bytes = l.bytes(w.raw_ref().get_dynamic(e, id), &v)?;
                    // SAFETY: `reflect_fields`' caller promised these bytes are a valid value.
                    unsafe { w.raw().insert_dynamic(e, id, bytes.as_ptr()) };
                    Ok(())
                }),
                spawn: Arc::new(move |w: &mut World, v| {
                    let bytes = st.bytes(None, &v)?;
                    // SAFETY: as above.
                    Ok(unsafe { w.raw().spawn_dynamic(&[(id, bytes.as_ptr())]) })
                }),
                spawn_at: Arc::new(move |w: &mut World, e, v| {
                    let bytes = sa.bytes(None, &v)?;
                    // SAFETY: as above.
                    Ok(unsafe { w.raw().spawn_at_dynamic(e, &[(id, bytes.as_ptr())]) })
                }),
                check: Arc::new(move |v: &Value| k.bytes(None, v).map(drop)),
                remove: Arc::new(move |w: &mut World, e| w.raw().remove_dynamic(e, id)),
            }
        };
        match reg.by_name.get(&name) {
            // Describing the same component again replaces its fields.
            Some(&i) if reg.list[i].type_name == name => reg.list[i] = entry(&name),
            Some(&i) => panic!(
                "World::reflect_fields: `{name}` is already reflected as `{}`",
                reg.list[i].type_name
            ),
            None => {
                reg.by_name.insert(name.clone(), reg.list.len());
                reg.list.push(entry(&name));
            }
        }
    }

    fn reflected_components(&self) -> Vec<&str> {
        registry(self).map_or_else(Vec::new, |r| r.list.iter().map(|c| c.name.as_str()).collect())
    }

    fn get_json(&self, e: Entity, name: &str) -> Result<Value, ReflectError> {
        let c = find(self, name)?;
        if !self.is_alive(e) {
            return Err(ReflectError::NoSuchEntity(e));
        }
        match (c.get)(self, e) {
            Some(v) => v.map_err(bad_value(name)),
            None => Err(ReflectError::MissingComponent {
                entity: e,
                component: name.to_string(),
            }),
        }
    }

    fn set_json(&mut self, e: Entity, name: &str, value: Value) -> Result<(), ReflectError> {
        let c = find(self, name)?;
        if !self.is_alive(e) {
            return Err(ReflectError::NoSuchEntity(e));
        }
        let value = match (c.get)(self, e) {
            Some(current) if value.is_object() => {
                let mut current = current.map_err(bad_value(name))?;
                merge(&mut current, value);
                current
            }
            _ => value,
        };
        (c.set)(self, e, value).map_err(bad_value(name))
    }

    fn remove_json(&mut self, e: Entity, name: &str) -> Result<bool, ReflectError> {
        let c = find(self, name)?;
        if !self.is_alive(e) {
            return Err(ReflectError::NoSuchEntity(e));
        }
        Ok((c.remove)(self, e))
    }

    fn entity_json(&self, e: Entity) -> Result<Value, ReflectError> {
        if !self.is_alive(e) {
            return Err(ReflectError::NoSuchEntity(e));
        }
        let mut out = serde_json::Map::new();
        for c in registry(self).map_or(&[][..], |r| &r.list) {
            if let Some(v) = (c.get)(self, e) {
                out.insert(c.name.clone(), v.map_err(bad_value(&c.name))?);
            }
        }
        Ok(Value::Object(out))
    }

    fn spawn_json(&mut self, components: Value) -> Result<Entity, ReflectError> {
        let Value::Object(map) = components else {
            return Err(ReflectError::BadValue {
                component: "<entity>".into(),
                message: "expected an object keyed by component name".into(),
            });
        };
        // Look every name up first, so an unknown name spawns nothing.
        let parts = map
            .into_iter()
            .map(|(name, v)| find(self, &name).map(|c| (c, v)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut parts = parts.into_iter();
        let Some((first, value)) = parts.next() else {
            return Err(ReflectError::BadValue {
                component: "<entity>".into(),
                message: "an entity needs at least one component".into(),
            });
        };
        let e = (first.spawn)(self, value).map_err(bad_value(&first.name))?;
        for (c, value) in parts {
            if let Err(message) = (c.set)(self, e, value) {
                self.despawn(e);
                return Err(ReflectError::BadValue {
                    component: c.name,
                    message,
                });
            }
        }
        Ok(e)
    }

    #[track_caller]
    fn reflect_resource<R: Send + Sync + 'static + Serialize + DeserializeOwned>(&mut self) {
        let type_name = std::any::type_name::<R>();
        let name = short_name(type_name);
        if self.get_resource::<Registry>().is_none() {
            self.insert_resource(Registry::default());
        }
        let reg = self.resource_mut::<Registry>();
        if let Some(&i) = reg.resource_by_name.get(&name) {
            let existing = &reg.resources[i].type_name;
            assert_eq!(
                existing, type_name,
                "World::reflect_resource: `{type_name}` and `{existing}` would both be named `{name}`"
            );
            return;
        }
        reg.resource_by_name.insert(name.clone(), reg.resources.len());
        reg.resources.push(ReflectedResource {
            name,
            type_name: type_name.to_string(),
            get: Arc::new(get_resource::<R>),
            set: Arc::new(set_resource::<R>),
            check: Arc::new(check::<R>),
        });
    }

    fn reflected_resources(&self) -> Vec<&str> {
        registry(self).map_or_else(Vec::new, |r| r.resources.iter().map(|c| c.name.as_str()).collect())
    }

    fn resource_json(&self, name: &str) -> Result<Value, ReflectError> {
        let r = find_resource(self, name)?;
        match (r.get)(self) {
            Some(v) => v.map_err(bad_value(name)),
            None => Err(ReflectError::MissingResource(name.to_string())),
        }
    }

    fn set_resource_json(&mut self, name: &str, value: Value) -> Result<(), ReflectError> {
        let r = find_resource(self, name)?;
        let value = match (r.get)(self) {
            Some(current) if value.is_object() => {
                let mut current = current.map_err(bad_value(name))?;
                merge(&mut current, value);
                current
            }
            _ => value,
        };
        (r.set)(self, value).map_err(bad_value(name))
    }

    #[track_caller]
    unsafe fn reflect_resource_fields(&mut self, name: &str, layout: std::alloc::Layout, fields: &[Field]) {
        let schema = ValueSchema::from_fields(fields);
        let fields = Arc::new(FieldLayout::new(name, layout.size(), fields));
        if self.get_resource::<Registry>().is_none() {
            self.insert_resource(Registry::default());
        }
        let (read, write, check) = (fields.clone(), fields.clone(), fields);
        let (get_name, set_name) = (name.to_string(), name.to_string());
        let entry = ReflectedResource {
            name: name.to_string(),
            type_name: name.to_string(),
            get: Arc::new(move |w: &World| {
                // SAFETY: the pointer is to the live resource, `layout.size()` bytes.
                w.raw_ref()
                    .get_resource_dynamic(&get_name)
                    .map(|p| Ok(unsafe { read.read(p) }))
            }),
            set: Arc::new(move |w: &mut World, v| {
                let bytes = write.bytes(w.raw_ref().get_resource_dynamic(&set_name), &v)?;
                // SAFETY: `bytes` holds `layout.size()` bytes, a valid value as
                // `reflect_resource_fields`' caller promised.
                unsafe { w.raw().insert_resource_dynamic(&set_name, layout, bytes.as_ptr()) };
                Ok(())
            }),
            check: Arc::new(move |v: &Value| check.bytes(None, v).map(drop)),
        };
        let reg = self.resource_mut::<Registry>();
        match reg.resource_by_name.get(name) {
            Some(&i) if reg.resources[i].type_name == name => reg.resources[i] = entry,
            Some(&i) => panic!(
                "World::reflect_resource_fields: `{name}` is already reflected as `{}`",
                reg.resources[i].type_name
            ),
            None => {
                reg.resource_by_name.insert(name.to_string(), reg.resources.len());
                reg.resources.push(entry);
            }
        }
        reg.resource_schemas
            .insert(name.to_string(), (schema, SchemaSource::Fields));
    }

    fn describe_component(&mut self, name: &str, schema: ValueSchema) -> Result<(), ReflectError> {
        let reg = self
            .get_resource_mut::<Registry>()
            .filter(|r| r.by_name.contains_key(name))
            .ok_or_else(|| ReflectError::UnknownComponent(name.to_string()))?;
        reg.schemas.insert(name.to_string(), (schema, SchemaSource::Described));
        Ok(())
    }

    fn describe_resource(&mut self, name: &str, schema: ValueSchema) -> Result<(), ReflectError> {
        let reg = self
            .get_resource_mut::<Registry>()
            .filter(|r| r.resource_by_name.contains_key(name))
            .ok_or_else(|| ReflectError::UnknownResource(name.to_string()))?;
        reg.resource_schemas
            .insert(name.to_string(), (schema, SchemaSource::Described));
        Ok(())
    }

    fn save(&self) -> Value {
        let reflected = self.reflected_components();
        let mut entities = Vec::new();
        let mut not_saved: std::collections::BTreeMap<String, usize> = Default::default();
        let left_out = self.component_id::<NotSaved>();
        for a in self
            .raw_ref()
            .archetypes()
            .iter()
            .filter(|a| !a.is_empty() && left_out.is_none_or(|id| !a.component_ids().contains(&id)))
        {
            for &id in a.component_ids() {
                // Rust components are reflected under their short name, C ones
                // under the name they were registered with ("game::Health").
                let full = self.component_info(id).name();
                let name = short_name(full);
                if !reflected.contains(&name.as_str()) && !reflected.contains(&full) {
                    *not_saved.entry(name).or_default() += a.len();
                }
            }
            for &e in a.entities() {
                if let Ok(Value::Object(components)) = self.entity_json(e)
                    && !components.is_empty()
                {
                    entities.push((e.to_bits(), components));
                }
            }
        }
        entities.sort_unstable_by_key(|(id, _)| *id);
        let entities: Vec<Value> = entities
            .into_iter()
            .map(|(id, components)| serde_json::json!({ "entity": id, "components": components }))
            .collect();
        let mut resources = serde_json::Map::new();
        for r in registry(self).map_or(&[][..], |r| &r.resources) {
            if let Some(Ok(v)) = (r.get)(self) {
                resources.insert(r.name.clone(), v);
            }
        }
        let mut save = serde_json::json!({
            "format": SAVE_FORMAT,
            "version": SAVE_VERSION,
            "entities": entities,
            "resources": resources,
            "not_saved": not_saved,
        });
        if let Some(version) = migrate::game_version(self) {
            save[migrate::GAME_VERSION] = version.into();
        }
        save
    }

    fn load(&mut self, save: &Value) -> Result<(), ReflectError> {
        let bad = |message: String| ReflectError::BadValue {
            component: "<save>".into(),
            message,
        };
        if save["format"] != SAVE_FORMAT {
            return Err(bad(format!(
                "not a world save (expected \"format\": \"{SAVE_FORMAT}\")"
            )));
        }
        if save["version"].as_u64() != Some(SAVE_VERSION) {
            return Err(bad(format!(
                "unsupported save version {} (this engine reads {SAVE_VERSION})",
                save["version"]
            )));
        }
        // The game's own changes: older saves are brought up to date first.
        let save = migrate::migrate(self, save)?;
        let list = save["entities"]
            .as_array()
            .ok_or_else(|| bad("missing \"entities\" list".into()))?;

        // Check everything first, so a bad save leaves the world as it was.
        let mut plan = Vec::with_capacity(list.len());
        let mut seen = std::collections::HashSet::with_capacity(list.len());
        let mut highest: Option<u64> = None;
        for item in list {
            let id = item["entity"]
                .as_u64()
                .ok_or_else(|| bad(format!("entity id missing or not a number: {}", item["entity"])))?;
            if !seen.insert(id) || Entity::from_bits(id).index() == u32::MAX {
                return Err(bad(format!("entity id {id} appears twice or is invalid")));
            }
            let Some(Value::Object(components)) = item.get("components") else {
                return Err(bad(format!("entity {id} has no \"components\" object")));
            };
            if components.is_empty() {
                return Err(bad(format!("entity {id} has no components")));
            }
            let mut parts = Vec::with_capacity(components.len());
            for (name, value) in components {
                let c = find(self, name)?;
                (c.check)(value).map_err(bad_value(name))?;
                parts.push((c, value.clone()));
            }
            plan.push((Entity::from_bits(id), parts));
            if highest.is_none_or(|h| Entity::from_bits(h).index() < Entity::from_bits(id).index()) {
                highest = Some(id);
            }
        }
        // Bound the slot table the ids would grow (see the docs of `load`).
        let allowed = self
            .entity_slots()
            .max(list.len().saturating_mul(LOAD_SLOTS_PER_ENTITY))
            .saturating_add(LOAD_SPARE_SLOTS);
        if let Some(id) = highest
            && Entity::from_bits(id).index() as usize >= allowed
        {
            return Err(bad(format!(
                "entity id {id} has index {}, but a save of {} entities may only use indices below {allowed}; \
                 the file is corrupt or not a world save",
                Entity::from_bits(id).index(),
                list.len()
            )));
        }
        // Saves from before resources were saved have none.
        let mut resource_plan = Vec::new();
        if let Some(saved) = save.get("resources") {
            let saved = saved
                .as_object()
                .ok_or_else(|| bad("\"resources\" is not an object".into()))?;
            for (name, value) in saved {
                let r = find_resource(self, name)?;
                (r.check)(value).map_err(bad_value(name))?;
                resource_plan.push((r, value.clone()));
            }
        }

        self.reset();
        // Counted as soon as the old entities are gone, so linked services
        // never keep them.
        let loads = world_loads(self) + 1;
        self.insert_resource(WorldLoads(loads));
        for (e, parts) in plan {
            let mut parts = parts.into_iter();
            let (first, value) = parts.next().expect("checked: at least one component");
            let placed = (first.spawn_at)(self, e, value).map_err(bad_value(&first.name))?;
            debug_assert!(placed, "ids are unique and the world was reset");
            for (c, value) in parts {
                (c.set)(self, e, value).map_err(bad_value(&c.name))?;
            }
        }
        for (r, value) in resource_plan {
            (r.set)(self, value).map_err(bad_value(&r.name))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn short_names_drop_the_path_but_keep_generics() {
        assert_eq!(short_name("my_game::units::Health"), "Health");
        assert_eq!(short_name("Health"), "Health");
        assert_eq!(short_name("a::Wrapper<b::Inner>"), "Wrapper<b::Inner>");
    }

    #[test]
    fn merge_replaces_only_given_fields() {
        let mut base = json!({ "a": 1, "b": { "x": 1, "y": 2 }, "c": [1, 2] });
        merge(&mut base, json!({ "b": { "y": 9 }, "c": [3] }));
        assert_eq!(base, json!({ "a": 1, "b": { "x": 1, "y": 9 }, "c": [3] }));
    }
}

// ---- Sprites ------------------------------------------------------------------------

/// File names of loaded assets by handle id, and the handles by name, so
/// components are saved by file name (`"texture": "crab.png"`).
///
/// One table is shared by the world (a resource: saving and loading read
/// it) and the graphics service (which fills it in as files load, without
/// borrowing the world).
pub(crate) struct Names<H>(Arc<std::sync::Mutex<NameTable<H>>>);

struct NameTable<H> {
    by_id: HashMap<u32, String>,
    by_name: HashMap<String, H>,
}

impl<H> Default for Names<H> {
    fn default() -> Self {
        Self(Arc::new(std::sync::Mutex::new(NameTable {
            by_id: HashMap::new(),
            by_name: HashMap::new(),
        })))
    }
}

impl<H> Clone for Names<H> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<H: Copy> Names<H> {
    fn table(&self) -> std::sync::MutexGuard<'_, NameTable<H>> {
        // A panic while holding the lock leaves the maps as they were.
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The file name handle `id` was loaded from.
    pub(crate) fn name(&self, id: u32) -> Option<String> {
        self.table().by_id.get(&id).cloned()
    }

    /// The handle loaded from file `name`.
    pub(crate) fn get(&self, name: &str) -> Option<H> {
        self.table().by_name.get(name).copied()
    }

    /// Names handle `id` after file `name`, unless either is taken.
    pub(crate) fn add(&self, id: u32, name: &str, handle: H) {
        let mut table = self.table();
        table.by_id.entry(id).or_insert_with(|| name.to_string());
        table.by_name.entry(name.to_string()).or_insert(handle);
    }

    /// Names handle `id` after file `name`, over what was there.
    #[cfg(feature = "3d")]
    pub(crate) fn set(&self, id: u32, name: &str, handle: H) {
        let mut table = self.table();
        table.by_id.insert(id, name.to_string());
        table.by_name.insert(name.to_string(), handle);
    }

    /// Forgets handle `id` (it was freed).
    pub(crate) fn remove(&self, id: u32) {
        let mut table = self.table();
        if let Some(name) = table.by_id.remove(&id) {
            table.by_name.remove(&name);
        }
    }
}

/// The asset names of textures loaded from files (`Graphics::load_texture`), so
/// sprites are saved as `"texture": "crab.png"`. A resource of the game's world.
pub(crate) type TextureNames = Names<crate::texture::Texture>;

/// A [`Sprite`](crate::d2::Sprite) as JSON: the texture by its file name.
#[derive(Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SpriteJson {
    texture: Option<String>,
    color: crate::color::Color,
    z: f32,
    pivot: glam::Vec2,
    region: Option<crate::camera::Rect>,
}

impl Default for SpriteJson {
    fn default() -> Self {
        Self {
            texture: None,
            color: crate::color::Color::WHITE,
            z: 0.0,
            pivot: glam::Vec2::splat(0.5),
            region: None,
        }
    }
}

/// `None` for a texture made in code: it has no file to name, so the sprite
/// isn't saved (the rest of the entity is).
fn sprite_json(world: &World, s: &crate::d2::Sprite) -> Option<Result<Value, String>> {
    let name = world.get_resource::<TextureNames>()?.name(s.texture.id)?;
    let json = SpriteJson {
        texture: Some(name),
        color: s.color,
        z: s.z,
        pivot: s.pivot,
        region: s.region,
    };
    Some(serde_json::to_value(json).map_err(|err| err.to_string()))
}

fn sprite_from_json(world: &World, value: Value) -> Result<crate::d2::Sprite, String> {
    let json: SpriteJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let name = json
        .texture
        .ok_or("a sprite needs \"texture\": an image in assets/, like \"crab.png\"")?;
    let texture = world
        .get_resource::<TextureNames>()
        .and_then(|t| t.get(&name))
        .ok_or_else(|| format!("texture \"{name}\" isn't loaded: ctx.load_texture(\"{name}\") first"))?;
    Ok(crate::d2::Sprite {
        texture,
        color: json.color,
        z: json.z,
        pivot: json.pivot,
        region: json.region,
    })
}

/// Reflects [`Sprite`](crate::d2::Sprite) as `"Sprite"`, its texture saved by
/// file name (see [`TextureNames`]).
pub(crate) fn reflect_sprite(world: &mut World) {
    if world.get_resource::<TextureNames>().is_none() {
        world.insert_resource(TextureNames::default());
    }
    if world.get_resource::<Registry>().is_none() {
        world.insert_resource(Registry::default());
    }
    let reg = world.resource_mut::<Registry>();
    if reg.by_name.contains_key("Sprite") {
        return;
    }
    reg.schemas
        .insert("Sprite".into(), (builtin_schemas::sprite(), SchemaSource::Described));
    reg.by_name.insert("Sprite".into(), reg.list.len());
    reg.list.push(Reflected {
        name: "Sprite".into(),
        type_name: std::any::type_name::<crate::d2::Sprite>().into(),
        get: Arc::new(|w: &World, e| w.get::<crate::d2::Sprite>(e).and_then(|s| sprite_json(w, s))),
        set: Arc::new(|w: &mut World, e, v| {
            let sprite = sprite_from_json(w, v)?;
            w.insert(e, sprite);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let sprite = sprite_from_json(w, v)?;
            Ok(w.spawn((sprite,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let sprite = sprite_from_json(w, v)?;
            Ok(w.spawn_at(e, (sprite,)))
        }),
        check: Arc::new(check::<SpriteJson>),
        remove: Arc::new(remove::<crate::d2::Sprite>),
    });
}

// ---- Models ------------------------------------------------------------------------

/// The asset names of meshes loaded from files (`Graphics::load_mesh`), so
/// models are saved as `"mesh": "tree.glb"`. A resource of the game's world.
#[cfg(feature = "3d")]
pub(crate) type MeshNames = Names<crate::render::Mesh>;

/// A [`Model`](crate::render::Model) as JSON: the mesh by its file name.
#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ModelJson {
    mesh: Option<String>,
    color: crate::color::Color,
}

#[cfg(feature = "3d")]
impl Default for ModelJson {
    fn default() -> Self {
        Self {
            mesh: None,
            color: crate::color::Color::WHITE,
        }
    }
}

/// `None` for a mesh made in code: it has no file to name, so the model
/// isn't saved (the rest of the entity is).
#[cfg(feature = "3d")]
fn model_json(world: &World, m: &crate::render::Model) -> Option<Result<Value, String>> {
    let name = world.get_resource::<MeshNames>()?.name(m.mesh.id)?;
    let json = ModelJson {
        mesh: Some(name),
        color: m.color,
    };
    Some(serde_json::to_value(json).map_err(|err| err.to_string()))
}

#[cfg(feature = "3d")]
fn model_from_json(world: &World, value: Value) -> Result<crate::render::Model, String> {
    let json: ModelJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let name = json
        .mesh
        .ok_or("a model needs \"mesh\": a glTF file in assets/, like \"tree.glb\"")?;
    let mesh = world
        .get_resource::<MeshNames>()
        .and_then(|t| t.get(&name))
        .ok_or_else(|| format!("mesh \"{name}\" isn't loaded: ctx.gfx.load_mesh(\"{name}\") first"))?;
    Ok(crate::render::Model {
        mesh,
        color: json.color,
    })
}

#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ShadowMeshJson {
    mesh: Option<String>,
}

#[cfg(feature = "3d")]
fn shadow_mesh_json(world: &World, s: &crate::render::ShadowMesh) -> Option<Result<Value, String>> {
    let mesh = match s.0 {
        Some(mesh) => Some(world.get_resource::<MeshNames>()?.name(mesh.id)?),
        None => None,
    };
    Some(serde_json::to_value(ShadowMeshJson { mesh }).map_err(|err| err.to_string()))
}

#[cfg(feature = "3d")]
fn shadow_mesh_from_json(world: &World, value: Value) -> Result<crate::render::ShadowMesh, String> {
    let json: ShadowMeshJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let Some(name) = json.mesh else {
        return Ok(crate::render::ShadowMesh(None));
    };
    let mesh = world
        .get_resource::<MeshNames>()
        .and_then(|t| t.get(&name))
        .ok_or_else(|| format!("mesh \"{name}\" isn't loaded: ctx.gfx.load_mesh(\"{name}\") first"))?;
    Ok(crate::render::ShadowMesh(Some(mesh)))
}

/// The materials of meshes loaded from files (`Graphics::load_mesh`), so a
/// [`ModelMaterial`](crate::render::ModelMaterial) is saved as the file and the
/// material's name in it. One table shared by the world (a resource) and
/// the graphics service, like [`Names`].
#[cfg(feature = "3d")]
#[derive(Default, Clone)]
pub(crate) struct MaterialNames(Arc<std::sync::Mutex<MaterialTable>>);

#[cfg(feature = "3d")]
#[derive(Default)]
struct MaterialTable {
    /// A material's file and its number in that file's list.
    by_id: HashMap<u32, (String, usize)>,
    /// Each file's materials, in the file's order, with their names
    /// (empty: unnamed).
    by_file: HashMap<String, Vec<(crate::render::Material, String)>>,
}

#[cfg(feature = "3d")]
impl MaterialNames {
    fn table(&self) -> std::sync::MutexGuard<'_, MaterialTable> {
        // A panic while holding the lock leaves the maps as they were.
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records the materials of mesh file `file`.
    pub(crate) fn add(&self, file: &str, materials: impl Iterator<Item = (crate::render::Material, String)>) {
        let list: Vec<_> = materials.collect();
        let mut table = self.table();
        for (i, (material, _)) in list.iter().enumerate() {
            // A material shared by two files keeps its first name.
            table.by_id.entry(material.id).or_insert_with(|| (file.to_string(), i));
        }
        table.by_file.insert(file.to_string(), list);
    }

    /// Forgets mesh file `file` (it was unloaded), so a scene naming its
    /// materials says it isn't loaded.
    pub(crate) fn forget_file(&self, file: &str) {
        let mut table = self.table();
        if let Some(list) = table.by_file.remove(file) {
            for (material, _) in list {
                table.by_id.remove(&material.id);
            }
        }
    }

    /// Forgets freed material `id`.
    pub(crate) fn remove(&self, id: u32) {
        let mut table = self.table();
        table.by_id.remove(&id);
        for list in table.by_file.values_mut() {
            list.retain(|(m, _)| m.id != id);
        }
    }

    /// How `material` is written in a scene file, if it came from a file:
    /// by its name when the file has that name once, else by its number.
    fn named(&self, material: crate::render::Material) -> Option<MaterialJson> {
        let table = self.table();
        let (file, _) = table.by_id.get(&material.id)?;
        let list = table.by_file.get(file)?;
        let at = list.iter().position(|(m, _)| *m == material)?;
        let name = &list[at].1;
        let unique = !name.is_empty() && list.iter().filter(|(_, n)| n == name).count() == 1;
        Some(MaterialJson {
            mesh: file.clone(),
            material: if unique {
                MaterialRef::Name(name.clone())
            } else {
                MaterialRef::Index(at)
            },
        })
    }

    /// The material a scene file names, or an error saying what's missing.
    fn find(&self, json: &MaterialJson) -> Result<crate::render::Material, String> {
        let file = &json.mesh;
        let table = self.table();
        let list = table
            .by_file
            .get(file)
            .ok_or_else(|| format!("mesh \"{file}\" isn't loaded: ctx.gfx.load_mesh(\"{file}\") first"))?;
        let found = match &json.material {
            MaterialRef::Name(name) => list.iter().find(|(_, n)| n == name),
            MaterialRef::Index(i) => list.get(*i),
        };
        found.map(|(m, _)| *m).ok_or_else(|| {
            let names: Vec<_> = list.iter().map(|(_, n)| format!("\"{n}\"")).collect();
            let wanted = match &json.material {
                MaterialRef::Name(name) => format!("\"{name}\""),
                MaterialRef::Index(i) => i.to_string(),
            };
            format!(
                "mesh \"{file}\" has no material {wanted}: it has {} ({})",
                list.len(),
                names.join(", ")
            )
        })
    }
}

/// A material of a loaded mesh file: `{ "mesh": "rocks.glb", "material":
/// "Wet" }`, or its number in the file instead of the name.
#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialJson {
    mesh: String,
    material: MaterialRef,
}

#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize)]
#[serde(untagged)]
enum MaterialRef {
    Name(String),
    Index(usize),
}

/// `None` for a material made in code: it has no file to name, so the
/// override isn't saved (the rest of the entity is).
#[cfg(feature = "3d")]
fn model_material_json(world: &World, m: &crate::render::ModelMaterial) -> Option<Result<Value, String>> {
    let json = world.get_resource::<MaterialNames>()?.named(m.0)?;
    Some(serde_json::to_value(json).map_err(|err| err.to_string()))
}

#[cfg(feature = "3d")]
fn model_material_from_json(world: &World, value: Value) -> Result<crate::render::ModelMaterial, String> {
    let json: MaterialJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let names = world.get_resource::<MaterialNames>();
    let material = names.map_or_else(|| MaterialNames::default().find(&json), |n| n.find(&json))?;
    Ok(crate::render::ModelMaterial(material))
}

/// [`ModelMaterials`](crate::render::ModelMaterials) as JSON: `{ "parts": [{ "part":
/// 1, "mesh": "door.glb", "material": "Glass" }] }`.
#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ModelMaterialsJson {
    parts: Vec<PartMaterialJson>,
}

#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PartMaterialJson {
    part: usize,
    mesh: String,
    material: MaterialRef,
}

/// `None` when one of its materials was made in code: a list saved in part
/// would draw something else when loaded.
#[cfg(feature = "3d")]
fn model_materials_json(world: &World, m: &crate::render::ModelMaterials) -> Option<Result<Value, String>> {
    let names = world.get_resource::<MaterialNames>()?;
    let parts = m
        .parts()
        .map(|(part, material)| {
            let MaterialJson { mesh, material } = names.named(material)?;
            Some(PartMaterialJson { part, mesh, material })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(serde_json::to_value(ModelMaterialsJson { parts }).map_err(|err| err.to_string()))
}

#[cfg(feature = "3d")]
fn model_materials_from_json(world: &World, value: Value) -> Result<crate::render::ModelMaterials, String> {
    let json: ModelMaterialsJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let none = MaterialNames::default();
    let names = world.get_resource::<MaterialNames>().unwrap_or(&none);
    let parts = json
        .parts
        .into_iter()
        .map(|p| {
            let named = MaterialJson {
                mesh: p.mesh,
                material: p.material,
            };
            Ok((p.part, names.find(&named)?))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(crate::render::ModelMaterials::new(&parts))
}

/// A mesh's file name, or an error naming what to load first.
#[cfg(feature = "3d")]
fn mesh_by_name(world: &World, name: &str) -> Result<crate::render::Mesh, String> {
    world
        .get_resource::<MeshNames>()
        .and_then(|t| t.get(name))
        .ok_or_else(|| format!("mesh \"{name}\" isn't loaded: ctx.gfx.load_mesh(\"{name}\") first"))
}

/// A [`LodRenderer`](crate::render::LodRenderer) as JSON: the mesh by its file name.
#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LodRendererJson {
    mesh: Option<String>,
    offset: crate::d3::Transform,
    color: crate::color::Color,
}

#[cfg(feature = "3d")]
impl Default for LodRendererJson {
    fn default() -> Self {
        Self {
            mesh: None,
            offset: crate::d3::Transform::default(),
            color: crate::color::Color::WHITE,
        }
    }
}

#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
enum LodShadowJson {
    #[default]
    Same,
    Casters(Vec<LodRendererJson>),
}

#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct LodLevelJson {
    screen_height: f32,
    fade_width: f32,
    renderers: Vec<LodRendererJson>,
    shadow: LodShadowJson,
}

/// A [`LodGroup`](crate::render::LodGroup) as JSON, its meshes by file name.
#[cfg(feature = "3d")]
#[derive(Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LodGroupJson {
    size: f32,
    center: glam::Vec3,
    levels: Vec<LodLevelJson>,
}

#[cfg(feature = "3d")]
impl Default for LodGroupJson {
    fn default() -> Self {
        Self {
            size: 1.0,
            center: glam::Vec3::ZERO,
            levels: Vec::new(),
        }
    }
}

/// `None` if a mesh was made in code: it has no file to name, so the group
/// isn't saved (the rest of the entity is).
#[cfg(feature = "3d")]
fn lod_group_json(world: &World, g: &crate::render::LodGroup) -> Option<Result<Value, String>> {
    let names = world.get_resource::<MeshNames>()?;
    let renderers = |list: &[crate::render::LodRenderer]| -> Option<Vec<LodRendererJson>> {
        list.iter()
            .map(|r| {
                Some(LodRendererJson {
                    mesh: Some(names.name(r.mesh.id)?),
                    offset: r.offset,
                    color: r.color,
                })
            })
            .collect()
    };
    let mut levels = Vec::with_capacity(g.levels.len());
    for level in &g.levels {
        levels.push(LodLevelJson {
            screen_height: level.screen_height,
            fade_width: level.fade_width,
            renderers: renderers(&level.renderers)?,
            shadow: match &level.shadow {
                crate::render::LodShadow::Same => LodShadowJson::Same,
                crate::render::LodShadow::Casters(list) => LodShadowJson::Casters(renderers(list)?),
            },
        });
    }
    let json = LodGroupJson {
        size: g.size,
        center: g.center,
        levels,
    };
    Some(serde_json::to_value(json).map_err(|err| err.to_string()))
}

#[cfg(feature = "3d")]
fn lod_group_from_json(world: &World, value: Value) -> Result<crate::render::LodGroup, String> {
    let json: LodGroupJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let renderers = |list: Vec<LodRendererJson>| -> Result<Vec<crate::render::LodRenderer>, String> {
        list.into_iter()
            .map(|r| {
                let name = r
                    .mesh
                    .ok_or("a LOD renderer needs \"mesh\": a glTF file in assets/, like \"tree.glb\"")?;
                Ok(crate::render::LodRenderer::new(mesh_by_name(world, &name)?)
                    .with_offset(r.offset)
                    .with_color(r.color))
            })
            .collect()
    };
    let mut levels = Vec::with_capacity(json.levels.len());
    for level in json.levels {
        levels.push(
            crate::render::LodLevel::new(level.screen_height, renderers(level.renderers)?)
                .with_fade(level.fade_width)
                .with_shadow(match level.shadow {
                    LodShadowJson::Same => crate::render::LodShadow::Same,
                    LodShadowJson::Casters(list) => crate::render::LodShadow::Casters(renderers(list)?),
                }),
        );
    }
    Ok(crate::render::LodGroup::new(json.size, levels).with_center(json.center))
}

/// Every `"mesh"` named inside `"LodGroup"`, `"ModelMaterial"` and
/// `"ModelMaterials"` objects anywhere in `json`, for loading them first.
#[cfg(feature = "3d")]
pub(crate) fn lod_mesh_names(json: &Value, out: &mut Vec<String>) {
    fn meshes(json: &Value, out: &mut Vec<String>) {
        match json {
            Value::Object(map) => {
                for (k, value) in map {
                    match value {
                        Value::String(name) if k == "mesh" => out.push(name.clone()),
                        _ => meshes(value, out),
                    }
                }
            }
            Value::Array(list) => list.iter().for_each(|v| meshes(v, out)),
            _ => {}
        }
    }
    match json {
        Value::Object(map) => {
            for (k, value) in map {
                if matches!(k.as_str(), "LodGroup" | "ModelMaterial" | "ModelMaterials") {
                    meshes(value, out);
                } else {
                    lod_mesh_names(value, out);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|v| lod_mesh_names(v, out)),
        _ => {}
    }
}

/// Reflects [`Model`](crate::render::Model) as `"Model"`,
/// [`ShadowMesh`](crate::render::ShadowMesh) as `"ShadowMesh"` and
/// [`LodGroup`](crate::render::LodGroup) as `"LodGroup"`, their meshes saved by file
/// name (see [`MeshNames`]), and [`ModelMaterial`](crate::render::ModelMaterial) and
/// [`ModelMaterials`](crate::render::ModelMaterials), their materials saved by file
/// and name (see [`MaterialNames`]).
#[cfg(feature = "3d")]
pub(crate) fn reflect_model(world: &mut World) {
    if world.get_resource::<MeshNames>().is_none() {
        world.insert_resource(MeshNames::default());
    }
    if world.get_resource::<MaterialNames>().is_none() {
        world.insert_resource(MaterialNames::default());
    }
    if world.get_resource::<Registry>().is_none() {
        world.insert_resource(Registry::default());
    }
    let reg = world.resource_mut::<Registry>();
    if reg.by_name.contains_key("Model") {
        return;
    }
    reg.schemas
        .insert("Model".into(), (builtin_schemas::model(), SchemaSource::Described));
    reg.by_name.insert("Model".into(), reg.list.len());
    reg.list.push(Reflected {
        name: "Model".into(),
        type_name: std::any::type_name::<crate::render::Model>().into(),
        get: Arc::new(|w: &World, e| w.get::<crate::render::Model>(e).and_then(|m| model_json(w, m))),
        set: Arc::new(|w: &mut World, e, v| {
            let model = model_from_json(w, v)?;
            w.insert(e, model);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let model = model_from_json(w, v)?;
            Ok(w.spawn((model,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let model = model_from_json(w, v)?;
            Ok(w.spawn_at(e, (model,)))
        }),
        check: Arc::new(check::<ModelJson>),
        remove: Arc::new(remove::<crate::render::Model>),
    });
    reg.schemas.insert(
        "ShadowMesh".into(),
        (builtin_schemas::shadow(), SchemaSource::Described),
    );
    reg.by_name.insert("ShadowMesh".into(), reg.list.len());
    reg.list.push(Reflected {
        name: "ShadowMesh".into(),
        type_name: std::any::type_name::<crate::render::ShadowMesh>().into(),
        get: Arc::new(|w: &World, e| {
            w.get::<crate::render::ShadowMesh>(e)
                .and_then(|s| shadow_mesh_json(w, s))
        }),
        set: Arc::new(|w: &mut World, e, v| {
            let shadow = shadow_mesh_from_json(w, v)?;
            w.insert(e, shadow);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let shadow = shadow_mesh_from_json(w, v)?;
            Ok(w.spawn((shadow,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let shadow = shadow_mesh_from_json(w, v)?;
            Ok(w.spawn_at(e, (shadow,)))
        }),
        check: Arc::new(check::<ShadowMeshJson>),
        remove: Arc::new(remove::<crate::render::ShadowMesh>),
    });
    reg.schemas.insert(
        "ModelMaterial".into(),
        (builtin_schemas::model_material(), SchemaSource::Described),
    );
    reg.by_name.insert("ModelMaterial".into(), reg.list.len());
    reg.list.push(Reflected {
        name: "ModelMaterial".into(),
        type_name: std::any::type_name::<crate::render::ModelMaterial>().into(),
        get: Arc::new(|w: &World, e| {
            w.get::<crate::render::ModelMaterial>(e)
                .and_then(|m| model_material_json(w, m))
        }),
        set: Arc::new(|w: &mut World, e, v| {
            let material = model_material_from_json(w, v)?;
            w.insert(e, material);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let material = model_material_from_json(w, v)?;
            Ok(w.spawn((material,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let material = model_material_from_json(w, v)?;
            Ok(w.spawn_at(e, (material,)))
        }),
        check: Arc::new(check::<MaterialJson>),
        remove: Arc::new(remove::<crate::render::ModelMaterial>),
    });
    reg.schemas.insert(
        "ModelMaterials".into(),
        (builtin_schemas::model_materials(), SchemaSource::Described),
    );
    reg.by_name.insert("ModelMaterials".into(), reg.list.len());
    reg.list.push(Reflected {
        name: "ModelMaterials".into(),
        type_name: std::any::type_name::<crate::render::ModelMaterials>().into(),
        get: Arc::new(|w: &World, e| {
            w.get::<crate::render::ModelMaterials>(e)
                .and_then(|m| model_materials_json(w, m))
        }),
        set: Arc::new(|w: &mut World, e, v| {
            let materials = model_materials_from_json(w, v)?;
            w.insert(e, materials);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let materials = model_materials_from_json(w, v)?;
            Ok(w.spawn((materials,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let materials = model_materials_from_json(w, v)?;
            Ok(w.spawn_at(e, (materials,)))
        }),
        check: Arc::new(check::<ModelMaterialsJson>),
        remove: Arc::new(remove::<crate::render::ModelMaterials>),
    });
    reg.schemas
        .insert("LodGroup".into(), (builtin_schemas::lod(), SchemaSource::Described));
    reg.by_name.insert("LodGroup".into(), reg.list.len());
    reg.list.push(Reflected {
        name: "LodGroup".into(),
        type_name: std::any::type_name::<crate::render::LodGroup>().into(),
        get: Arc::new(|w: &World, e| w.get::<crate::render::LodGroup>(e).and_then(|g| lod_group_json(w, g))),
        set: Arc::new(|w: &mut World, e, v| {
            let group = lod_group_from_json(w, v)?;
            w.insert(e, group);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let group = lod_group_from_json(w, v)?;
            Ok(w.spawn((group,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let group = lod_group_from_json(w, v)?;
            Ok(w.spawn_at(e, (group,)))
        }),
        check: Arc::new(check::<LodGroupJson>),
        remove: Arc::new(remove::<crate::render::LodGroup>),
    });
}

// ---- Colliders ---------------------------------------------------------------

/// Collision meshes made from mesh files by loading scenes, with the
/// description they were made from (`{"file": "rock.glb", "scale": [1, 1,
/// 1], "convex": false}`), so their colliders are saved the same way. A
/// resource of the game's world.
#[cfg(feature = "physics3d")]
#[derive(Default)]
pub(crate) struct ColliderFiles {
    pub(crate) by_mesh: HashMap<crate::physics3d::CollisionMesh, Value>,
    pub(crate) by_file: HashMap<String, crate::physics3d::CollisionMesh>,
}

/// A collider's mesh description as a lookup key: the same for every way
/// to write it (`scale` and `convex` left out or given as their defaults),
/// so a scene and [`Ctx::load_collision_mesh`](crate::ctx::Ctx::load_collision_mesh)
/// share one mesh.
#[cfg(feature = "physics3d")]
pub(crate) fn collider_file_key(file: &Value) -> String {
    let (path, scale, convex) = collider_file_parts(file);
    format!("{path:?} {:?} {convex}", scale.to_array())
}

/// A mesh description's file, scale (one when left out) and convex flag
/// (false when left out). The file is `None` when it isn't a string.
#[cfg(feature = "physics3d")]
pub(crate) fn collider_file_parts(file: &Value) -> (Option<&str>, glam::Vec3, bool) {
    let scale = match &file["scale"] {
        Value::Array(s) if s.len() == 3 => {
            glam::Vec3::from_array(std::array::from_fn(|i| s[i].as_f64().unwrap_or(1.0) as f32))
        }
        _ => glam::Vec3::ONE,
    };
    (file["file"].as_str(), scale, file["convex"].as_bool().unwrap_or(false))
}

/// A collider described by a mesh file, as JSON: its mesh's description.
#[cfg(feature = "physics3d")]
pub(crate) fn collider_file(value: &Value) -> Option<&Value> {
    value.pointer("/shape/Mesh").filter(|m| m.get("file").is_some())
}

/// `None` for a collider of a mesh made in code: it has no file to name, so
/// the collider isn't saved (the rest of the entity is), as with a model.
#[cfg(feature = "physics3d")]
fn collider_json(world: &World, c: &crate::d3::Collider) -> Option<Result<Value, String>> {
    let file = match c.shape {
        crate::d3::Shape::Mesh(mesh) => Some(world.get_resource::<ColliderFiles>()?.by_mesh.get(&mesh)?),
        _ => None,
    };
    let mut value = match serde_json::to_value(c) {
        Ok(v) => v,
        Err(err) => return Some(Err(err.to_string())),
    };
    if let Some(file) = file {
        value["shape"] = serde_json::json!({ "Mesh": file });
    }
    Some(Ok(value))
}

/// A mesh collider names its mesh file; only loaded files give a mesh.
#[cfg(feature = "physics3d")]
fn collider_mesh_json(world: &World, value: &Value) -> Result<Option<crate::physics3d::CollisionMesh>, String> {
    let Some(mesh) = value.pointer("/shape/Mesh") else {
        return Ok(None);
    };
    let Some(file) = collider_file(value) else {
        return Err(format!(
            "a mesh collider names its mesh file, like {{\"Mesh\": {{\"file\": \"rock.glb\"}}}}, not {mesh}"
        ));
    };
    world
        .get_resource::<ColliderFiles>()
        .and_then(|f| f.by_file.get(&collider_file_key(file)))
        .copied()
        .map(Some)
        .ok_or_else(|| format!("collision mesh {file} isn't loaded: load the scene with ctx.load_scene"))
}

/// The JSON with a named mesh replaced by the `null` that serde reads.
#[cfg(feature = "physics3d")]
fn without_mesh_file(mut value: Value) -> Value {
    if collider_file(&value).is_some() {
        value["shape"] = serde_json::json!({ "Mesh": null });
    }
    value
}

#[cfg(feature = "physics3d")]
fn collider_from_json(world: &World, value: Value) -> Result<crate::d3::Collider, String> {
    let mesh = collider_mesh_json(world, &value)?;
    let mut collider: crate::d3::Collider =
        serde_json::from_value(without_mesh_file(value)).map_err(|err| err.to_string())?;
    if let Some(mesh) = mesh {
        collider.shape = crate::d3::Shape::Mesh(mesh);
    }
    Ok(collider)
}

/// Reflects [`Collider3D`](crate::d3::Collider) as `"Collider3D"`; a mesh
/// made from a file (by loading a scene) is saved by that file.
#[cfg(feature = "physics3d")]
pub(crate) fn reflect_collider(world: &mut World) {
    if world.get_resource::<ColliderFiles>().is_none() {
        world.insert_resource(ColliderFiles::default());
    }
    if world.get_resource::<Registry>().is_none() {
        world.insert_resource(Registry::default());
    }
    let reg = world.resource_mut::<Registry>();
    if reg.by_name.contains_key("Collider3D") {
        return;
    }
    reg.schemas.insert(
        "Collider3D".into(),
        (
            builtin_schemas::builtin::<crate::d3::Collider>().unwrap(),
            SchemaSource::Described,
        ),
    );
    reg.by_name.insert("Collider3D".into(), reg.list.len());
    reg.list.push(Reflected {
        name: "Collider3D".into(),
        type_name: std::any::type_name::<crate::d3::Collider>().into(),
        get: Arc::new(|w: &World, e| w.get::<crate::d3::Collider>(e).and_then(|c| collider_json(w, c))),
        set: Arc::new(|w: &mut World, e, v| {
            let c = collider_from_json(w, v)?;
            w.insert(e, c);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let c = collider_from_json(w, v)?;
            Ok(w.spawn((c,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let c = collider_from_json(w, v)?;
            Ok(w.spawn_at(e, (c,)))
        }),
        check: Arc::new(|v: &Value| {
            if v.pointer("/shape/Mesh").is_some() && collider_file(v).is_none() {
                return Err("a mesh collider names its mesh file, like {\"Mesh\": {\"file\": \"rock.glb\"}}".into());
            }
            check::<crate::d3::Collider>(&without_mesh_file(v.clone()))
        }),
        remove: Arc::new(remove::<crate::d3::Collider>),
    });
}

/// The asset names under `key` objects' `field` anywhere in `json` (a
/// scene, a prefab, or an entity's components): the textures of `"Sprite"`s
/// and meshes of `"Model"`s, for loading them first.
pub(crate) fn asset_names(json: &Value, key: &str, field: &str, out: &mut Vec<String>) {
    match json {
        Value::Object(map) => {
            for (k, value) in map {
                if k == key
                    && let Some(name) = value.get(field).and_then(Value::as_str)
                {
                    out.push(name.to_string());
                } else {
                    asset_names(value, key, field, out);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|v| asset_names(v, key, field, out)),
        _ => {}
    }
}

pub use crate::schema::ValueSchema;
/// JSON representation used by reflection.
pub use serde_json as json;
