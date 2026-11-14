//! Three-dimensional game types. Pair with `ponko::prelude::*`.
//!
//! `use ponko::d3::*;` brings in the types of a 3D world that have a 2D
//! twin or would otherwise carry a `3D` in their name: the transform, the
//! camera, lights, decals, text, lines, trails, particle emitters,
//! reflection probes, positional audio, navigation agents, world-space UI
//! panels and the physics (feature `physics3d`). This module is their one
//! path, under these names, so it is the name rustdoc and the guide use.
//!
//! The rest of 3D lives in its topic module: [`render`](crate::render)
//! (meshes, materials, models, shaders, level of detail, render settings),
//! [`anim`](crate::anim) (skeletons, clips, animators, IK),
//! [`terrain`](crate::terrain), [`particles`](crate::particles) (curves and
//! gradients shared with 2D), [`collision`](crate::collision) (body kinds
//! and collision events, shared with 2D), [`attach`](crate::attach)
//! (3D attachments, `Attached3D`, with the 2D ones) and [`nav`](crate::nav)
//! (navigation mesh baking).
//!
//! ```
//! use ponko::prelude::*;
//! use ponko::d3::*;
//! use ponko::render::MaterialDesc;
//!
//! let desc = MaterialDesc::color(Color::WHITE);
//! let light = PointLight { intensity: 20.0, ..Default::default() };
//! # let _ = (desc, light, Transform::default());
//! ```
//!
//! ## Physics
//!
//! 3D physics with rapier (feature `physics3d`): rigid bodies, colliders
//! (heightfield terrain and triangle meshes too), a character controller
//! and raycast-wheel vehicles as components, stepped inside the simulation
//! clock. [`Transform`] stays the source of truth: set it and the body
//! teleports; moving bodies write their new pose back after every step,
//! and between steps (in windowed games) a pose blended between the last
//! two steps, so motion looks smooth at any frame rate. Turn it on with
//! [`Game::physics3d`](crate::game::Game::physics3d).

// Types whose home is this module.
#[doc(inline)]
pub use crate::three::Camera3D as Camera;
#[doc(inline)]
pub use crate::three::Decal3D as Decal;
#[doc(inline)]
pub use crate::three::Line3D as Line;
#[doc(inline)]
pub use crate::three::ParticleBlend3D as ParticleBlend;
#[doc(inline)]
pub use crate::three::ParticleEmitter3D as ParticleEmitter;
#[doc(inline)]
pub use crate::three::ParticleSettings3D as ParticleSettings;
#[doc(inline)]
pub use crate::three::ParticleShape3D as ParticleShape;
#[doc(inline)]
pub use crate::three::PointLight3D as PointLight;
#[doc(inline)]
pub use crate::three::ReflectionProbe3D as ReflectionProbe;
#[doc(inline)]
pub use crate::three::SpotLight3D as SpotLight;
#[doc(inline)]
pub use crate::three::Text3D as Text;
#[doc(inline)]
pub use crate::three::TextAlign3D as TextAlign;
#[doc(inline)]
pub use crate::three::Trail3D as Trail;
#[doc(inline)]
pub use crate::three::Transform3D as Transform;

#[doc(inline)]
pub use crate::audio::spatial::{
    AudioListener3D as AudioListener, AudioSettings3D as AudioSettings, AudioSource3D as AudioSource,
};
#[cfg(feature = "navmesh")]
#[doc(inline)]
pub use crate::nav3d::{NavAgent3D as NavAgent, NavMesh3D as NavMesh, NavObstacle3D as NavObstacle};
#[cfg(feature = "physics3d")]
#[doc(inline)]
pub use crate::physics3d::{
    Character3D as Character, Collider3D as Collider, ColliderFocus, CollisionGroups, CollisionMesh, CollisionMeshData,
    CollisionMeshOptions, ContactModifier3D as ContactModifier, ContactPair3D as ContactPair, FrictionCurve,
    Hit3D as Hit, Joint3D as Joint, JointAxis3D as JointAxis, JointDrive3D as JointDrive, JointMotion3D as JointMotion,
    Physics3D as Physics, QueryFilter3D as QueryFilter, Ragdoll3D as Ragdoll, RagdollBone, RigidBody3D as RigidBody,
    Shape3D as Shape, TerrainCollider, Vehicle3D as Vehicle, Velocity3D as Velocity, Wheel3D as Wheel,
};
#[doc(inline)]
pub use crate::ui::panel3d::UiPanel3D as UiPanel;
