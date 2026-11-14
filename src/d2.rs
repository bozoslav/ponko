//! Two-dimensional game types. Pair with `ponko::prelude::*`.
//!
//! `use ponko::d2::*;` brings in the types whose 3D twin has the same name
//! in `d3` or that would otherwise carry a `2D` in their name: the
//! transform, the camera, the physics (feature `physics`), the particle
//! emitter, the lights and occluders, and the sprite. This module is their
//! one path, under these names, so it is the name rustdoc and the guide
//! use. The rest of 2D lives in its topic module: [`tilemap`](crate::tilemap),
//! [`sheet`](crate::sheet) (sprite-sheet animation), [`attach`](crate::attach),
//! [`spatial`](crate::spatial), [`flow`](crate::flow),
//! [`particles`](crate::particles) (curves and gradients shared with 3D),
//! `collision` (shared with 3D) and
//! [`render`](crate::render) (sprite materials).
//!
//! ## Physics
//!
//! 2D physics with rapier (feature `physics`): rigid bodies and colliders
//! as components, stepped inside the simulation clock. `Transform` stays
//! the source of truth: set it and the body teleports; dynamic bodies write
//! their new position back after every step. Physics is opt-in per entity,
//! so the million-entity paths (spatial grid, flow fields) stay as fast as
//! before. Turn it on with `Game::physics`;
//! the step is `systems::step_physics`.
//!
//! ## Lights
//!
//! [`PointLight`], [`SpotLight`] (a cone along the transform's rotation),
//! [`DirectionalLight`], an ambient light
//! ([`Graphics::set_ambient_light2d`](crate::gfx::Graphics::set_ambient_light2d)),
//! shadows cast by [`Occluder`] shapes and tile maps
//! ([`TileMap::occluder`](crate::tilemap::TileMap::occluder)), and normal maps
//! for sprites and tiles
//! ([`Graphics::set_normal_map`](crate::gfx::Graphics::set_normal_map)).
//!
//! Lighting is off until the game spawns a 2D light or sets an ambient
//! light: a game without either draws exactly as before, with no extra
//! passes and no extra GPU memory. Once on, the world's sprites, tiles and
//! particles are drawn as usual, then multiplied by a light map: the
//! ambient light plus every light in view, each cut by its shadows. The
//! screen-space UI is drawn after, unlit.
//!
//! ```
//! use ponko::prelude::*;
//! use ponko::d2::{Occluder, PointLight};
//!
//! Game::new("Cave", 320, 240).run_headless(1, |ctx| {
//!     ctx.gfx.set_ambient_light2d(Some(Color::rgb(0.1, 0.1, 0.15)));
//!     ctx.world.spawn((
//!         ponko::d2::Transform::at(vec2(160.0, 120.0)),
//!         PointLight { color: Color::rgb(1.0, 0.8, 0.5), radius: 150.0, shadows: true, ..Default::default() },
//!     ));
//!     ctx.world.spawn((ponko::d2::Transform::at(vec2(200.0, 120.0)), Occluder::rect(vec2(16.0, 48.0))));
//! });
//! ```
//!
//! Every GPU tier draws it, WebGL2 included: the light map is a plain
//! render target, shadows are stencil volumes, and nothing needs compute
//! or storage buffers.
#[doc(inline)]
pub use crate::camera2d::Camera2D as Camera;
#[doc(inline)]
pub use crate::ecs::{Sprite, Transform2D as Transform};
#[doc(inline)]
pub use crate::light2d::{
    DirectionalLight2D as DirectionalLight, Occluder2D as Occluder, PointLight2D as PointLight,
    SpotLight2D as SpotLight,
};
#[doc(inline)]
pub use crate::particles2d::{
    ParticleEmitter2D as ParticleEmitter, ParticleSettings2D as ParticleSettings, ParticleShape2D as ParticleShape,
};
#[cfg(feature = "physics")]
#[doc(inline)]
pub use crate::physics::{Collider, Physics, QueryFilter, RayHit as Hit, RigidBody, Shape, Velocity};
