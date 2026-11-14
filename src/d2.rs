//! Two-dimensional game types. Pair with `ponko::prelude::*`.
//!
//! `use ponko::d2::*;` brings in every 2D name a game uses, under short
//! names (`Camera`, `Transform`, `RigidBody`, ...). As in [`d3`](crate::d3),
//! each type is documented at exactly one path, its home: types of a topic
//! module ([`camera`](crate::camera), [`physics`](crate::physics),
//! `collision` (shared with 3D), [`sheet`](crate::sheet),
//! [`tilemap`](crate::tilemap), ...) are documented
//! there and re-exported here (listed under *Re-exports*); types with no
//! topic module (`Sprite`, `Transform`) live here.
#[doc(no_inline)]
pub use crate::attach::Attached;
#[doc(no_inline)]
pub use crate::camera::Camera2D as Camera;
#[cfg(any(feature = "physics", feature = "physics3d"))]
#[doc(no_inline)]
pub use crate::collision::{BodyKind, CollisionStarted, CollisionStopped};
#[doc(inline)]
pub use crate::ecs::{Sprite, Transform2D as Transform};
#[doc(no_inline)]
pub use crate::flow::{FlowField, NavGrid};
#[doc(no_inline)]
pub use crate::light2d::{DirectionalLight2D, Occluder2D, PointLight2D, SpotLight2D};
#[doc(no_inline)]
pub use crate::particles::{
    ParticleCurve, ParticleEmitter2D as ParticleEmitter, ParticleGradient, ParticleSettings2D as ParticleSettings,
    ParticleShape2D as ParticleShape, ParticleSpace,
};
#[cfg(feature = "physics")]
#[doc(no_inline)]
pub use crate::physics::{Collider, Physics, QueryFilter, RayHit as Hit, RigidBody, Shape, Velocity};
#[doc(no_inline)]
pub use crate::sheet::{Animation, SpriteSheet};
#[doc(no_inline)]
pub use crate::spatial::SpatialGrid;
#[doc(no_inline)]
pub use crate::sprite_material::{BlendMode, SpriteMaterial, SpriteMaterialDesc, SpriteShader};
#[doc(no_inline)]
pub use crate::tilemap::{
    AutoTile, Layer, LayerKind, Level, LevelLayer, LevelObject, LevelOptions, LevelRoot, Neighbours, ObjectShape,
    PlacedTile, Properties, TileAnimation, TileInfo, TileLayer, TileMap, Tileset,
};
