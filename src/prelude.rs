//! The one glob import a game starts with: `use ponko::prelude::*;`.
//!
//! It holds what a typical game touches: the game loop ([`Game`], [`Ctx`],
//! [`Phase`], [`Plugin`], [`Error`]), the ECS ([`World`], [`Entity`],
//! [`Query`], [`Commands`]), math (`Vec2`, `Vec3`, `Vec4`, `Quat`, `Mat4` and their
//! constructors), input ([`Key`], [`MouseButton`]), [`Color`], textures,
//! sounds and fonts, [`Rect`], [`Random`], [`Events`], reflection, and the
//! common transforms and cameras under their descriptive names
//! (`Transform2D`, `Camera2D`, `Transform3D`, `Camera3D`).
//!
//! Add `use ponko::d2::*;` or `use ponko::d3::*;` for one dimension's names
//! (`Transform`, `RigidBody`, `Physics`, `Hit`, ...), and import service
//! modules (`ponko::net`, `ponko::perf`, `ponko::render`, `ponko::anim`, ...)
//! where a file needs them. Every type is documented at one path, its home:
//! its topic module (`ponko::render::Material`, `ponko::anim::Animator`), or
//! `d2`/`d3` for the types that have none (`d3::Camera`, `d3::PointLight`).
//! `d2`/`d3` re-export the topic modules' types, so the glob and the topic
//! path name the same type; the prelude re-exports a few of each. Short
//! generic names a game is likely to define itself (`Pose`, `Material`, `Job`, `Motion`, `Shader`, `Joint`, ...) stay
//! out of the prelude.
//!
//! ```
//! use ponko::prelude::*;
//! use ponko::d2::*;
//!
//! struct Pose; // the game's own; no clash with the prelude
//! let mut world = World::new();
//! let e = world.spawn((Transform::at(vec2(1.0, 2.0)),));
//! assert_eq!(world.get::<Transform2D>(e).unwrap().position, vec2(1.0, 2.0));
//! # let _ = Pose;
//! ```
pub use crate::actions::{
    Action, ActionId, ActionMap, ActionRef, Axis2Binding, AxisBinding, ButtonBinding, PressedInput,
};
pub use crate::assets::{AssetSource, Assets, MemorySource};
pub use crate::audio::{AudioBus, Playing, Sound, SoundParams};
pub use crate::camera::Rect;
#[cfg(any(feature = "physics", feature = "physics3d"))]
#[doc(no_inline)]
pub use crate::collision::{BodyKind, CollisionStarted, CollisionStopped};
pub use crate::color::Color;
pub use crate::ctx::Ctx;
#[cfg(feature = "3d")]
pub use crate::d3;
pub use crate::error::{Error, ErrorKind};
pub use crate::events::Events;
pub use crate::game::{ConfiguredGame, Game, IntoSystem, Phase, Plugin, SetupResult, SystemId, Systems};
pub use crate::handle::Handle;
pub use crate::input::{Key, MouseButton};
pub use crate::profile_scope;
pub use crate::random::{Random, Rng};
pub use crate::reflect::WorldReflect;
pub use crate::scene::{Placement, PrefabParts};
pub use crate::sheet::SpriteSheet;
pub use crate::text::{Font, TextParams, TextShaping};
pub use crate::texture::{DrawParams, Texture, TextureFilter};
pub use crate::time::{Time, Turns};
pub use crate::{d2, systems};
#[cfg(feature = "3d")]
pub use glam::Mat4;
pub use glam::{Quat, UVec2, Vec2, Vec3, Vec4, uvec2, vec2, vec3, vec4};
pub use ponko_ecs::{Added, Changed, Commands, Component, Entity, Or, Query, Tick, With, Without, World};
pub use serde_json::json;
// Common dimension types retain descriptive spellings in mixed-dimensional code.
pub use crate::actions;
pub use crate::d2::{Camera as Camera2D, Sprite, Transform as Transform2D};
#[cfg(feature = "3d")]
pub use crate::d3::{Camera as Camera3D, Model, Transform as Transform3D};
pub use crate::ui::{Ui, UiId};
