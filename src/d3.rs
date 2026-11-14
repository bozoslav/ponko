//! Three-dimensional game types. Pair with `ponko::prelude::*`.
//!
//! `use ponko::d3::*;` brings in every 3D name a game uses, under short
//! names (`Camera`, `Transform`, `Physics`, `Material`, ...). Each type is
//! documented at exactly one path, its home:
//!
//! - A topic module is the home of its types: [`render`](crate::render)
//!   (meshes, materials, models, shaders, level of detail, render
//!   settings), [`anim`](crate::anim) (skeletons, clips, animators, IK),
//!   [`terrain`](crate::terrain), [`physics3d`](crate::physics3d),
//!   `collision` (body kinds and collision events, shared with 2D),
//!   [`audio`](crate::audio), [`nav`](crate::nav) and [`ui`](crate::ui).
//!   This module re-exports them (listed under *Re-exports*) so one glob is
//!   enough; both paths name the same type.
//! - Types with no topic module (cameras, transforms, lights, decals, 3D
//!   text, lines, trails, particle emitters, reflection probes) live here.
//!
//! ```
//! use ponko::prelude::*;
//! use ponko::d3::*;
//!
//! let desc = MaterialDesc::color(Color::WHITE);
//! assert_eq!(desc, ponko::render::MaterialDesc::color(Color::WHITE));
//! let light = PointLight { intensity: 20.0, ..Default::default() };
//! # let _ = (light, Transform::default());
//! ```

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

// Types documented in their topic module.
#[doc(no_inline)]
pub use crate::anim::{
    AnimationClip, AnimationCondition, AnimationEvent, AnimationGraph, AnimationGraphError, AnimationLayer,
    AnimationParameter, AnimationState, AnimationTrack, AnimationTransition, Animator, AttachToJoint, Interpolation,
    JointAnimation, JointMask, LayerBlend, LayerId, MorphAnimation, MorphFrame, MorphTarget, MorphWeights, Motion,
    MotionChild, ParamId, RootMotion, Skeleton, SkinOf, StateId,
};
#[doc(no_inline)]
pub use crate::audio::{
    AudioListener3D as AudioListener, AudioSettings3D as AudioSettings, AudioSource3D as AudioSource,
};
#[cfg(feature = "navmesh")]
#[doc(no_inline)]
pub use crate::nav::{NavAgent, NavMesh, NavObstacle};
#[doc(no_inline)]
pub use crate::particles::{ParticleCurve, ParticleGradient, ParticleSpace};
#[cfg(feature = "physics3d")]
#[doc(no_inline)]
pub use crate::physics3d::{
    Character3D as Character, Collider3D as Collider, ColliderFocus, CollisionGroups, CollisionMesh, CollisionMeshData,
    CollisionMeshOptions, ContactModifier3D as ContactModifier, ContactPair3D as ContactPair, FrictionCurve,
    Hit3D as Hit, Joint3D as Joint, JointAxis3D as JointAxis, JointDrive3D as JointDrive, JointMotion3D as JointMotion,
    Physics3D as Physics, QueryFilter3D as QueryFilter, Ragdoll3D as Ragdoll, RagdollBone, RigidBody3D as RigidBody,
    Shape3D as Shape, TerrainCollider, Vehicle3D as Vehicle, Velocity3D as Velocity, Wheel3D as Wheel,
};
#[doc(no_inline)]
pub use crate::render::{
    AmbientOcclusion, AntiAliasing, BatchCulling, Blend, Compute, ComputePass, Effect, GlobalIllumination, GpuBuffer,
    GraphicsQuality, InstanceBatch, InstanceBatchOptions, InstanceLevel, Lighting, LodGroup, LodHysteresis, LodLevel,
    LodPick, LodRenderer, LodSettings, LodShadow, Material, MaterialDesc, Mesh, MeshCost, MeshData, Model,
    ModelMaterial, ModelMaterials, Post, ReceivesDecals, RenderPasses, SceneSettings, ScreenEffect, Shader,
    ShaderDiagnostic, ShaderKind, ShaderOutcome, ShaderStatus, ShadowFilter, ShadowMesh, ShadowOnly, Sky, StreamedLod,
    TargetCamera, TargetEffect, Tonemap,
};
#[doc(no_inline)]
pub use crate::terrain::{
    FarTerrain, Heightfield, TerrainMaterialDesc, TerrainTile, TerrainTileData, TextureArray, TextureKind,
};
#[doc(no_inline)]
pub use crate::ui::UiPanel3D as UiPanel;

#[cfg(any(feature = "physics", feature = "physics3d"))]
#[doc(no_inline)]
pub use crate::collision::{BodyKind, CollisionStarted, CollisionStopped};
