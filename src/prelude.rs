//! The one glob import a game starts with: `use ponko::prelude::*;`.
//!
//! It holds what a typical game touches: the game loop ([`Game`], [`Ctx`],
//! [`Phase`], [`Plugin`], [`Error`]), the ECS ([`World`], [`Entity`],
//! [`Query`], [`Commands`]), math (`Vec2`, `Vec3`, `Vec4`, `Quat`, `Mat4` and their
//! constructors), input ([`Key`], [`MouseButton`]), [`Color`], textures,
//! sounds and fonts, [`Rect`], [`Random`], [`Events`], reflection, and the
//! [`d2`] and `d3` modules, so code for both dimensions can write
//! `d2::Transform` next to `d3::Transform`.
//!
//! Add `use ponko::d2::*;` or `use ponko::d3::*;` for one dimension's names
//! (`Transform`, `Camera`, `RigidBody`, `Physics`, `Hit`, ...), and import
//! service modules (`ponko::net`, `ponko::perf`, `ponko::render`,
//! `ponko::anim`, ...) where a file needs them. Every item has one path, its
//! home: its topic module (`ponko::render::Material`, `ponko::anim::Animator`),
//! or `d2`/`d3` for the types named after a dimension (`d3::Camera`,
//! `d3::PointLight`). The prelude re-exports a few common ones as the same
//! items. Short generic names a game is likely to define itself (`Pose`,
//! `Material`, `Job`, `Motion`, `Shader`, `Joint`, ...) stay out of the
//! prelude.
//!
//! ```
//! use ponko::prelude::*;
//! use ponko::d2::*;
//!
//! struct Pose; // the game's own; no clash with the prelude
//! let mut world = World::new();
//! let e = world.spawn((Transform::at(vec2(1.0, 2.0)),));
//! assert_eq!(world.get::<d2::Transform>(e).unwrap().position, vec2(1.0, 2.0));
//! # let _ = Pose;
//! ```
pub use crate::actions;
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
pub use crate::d2::Sprite;
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

/// Deprecated: write [`d2::Transform`] (the prelude
/// brings `d2`).
#[deprecated(since = "0.2.0", note = "use `d2::Transform`")]
pub type Transform2D = crate::d2::Transform;
/// Deprecated: write [`d2::Camera`].
#[deprecated(since = "0.2.0", note = "use `d2::Camera`")]
pub type Camera2D = crate::d2::Camera;
/// Deprecated: write [`d3::Transform`] (the prelude
/// brings `d3`).
#[cfg(feature = "3d")]
#[deprecated(since = "0.2.0", note = "use `d3::Transform`")]
pub type Transform3D = crate::d3::Transform;
/// Deprecated: write [`d3::Camera`].
#[cfg(feature = "3d")]
#[deprecated(since = "0.2.0", note = "use `d3::Camera`")]
pub type Camera3D = crate::d3::Camera;
#[cfg(feature = "3d")]
pub use crate::render::Model;
pub use crate::ui::{Ui, UiId};
