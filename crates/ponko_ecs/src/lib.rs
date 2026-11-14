//! # ponko_ecs
//!
//! The entity-component-system behind Ponko, built for simulations
//! with millions of entities.
//!
//! - **Entities** are cheap handles ([`Entity`]).
//! - **Components** are plain Rust types that opt in with
//!   `#[derive(Component)]` (or `impl Component for Type {}`).
//! - **Queries** loop over every entity that has a set of components, as fast
//!   as a hand-written loop over arrays.
//! - **Resources** are global values stored in the world.
//! - **Commands** record changes to apply after a query finishes.
//!
//! ```
//! use ponko_ecs::*;
//!
//! #[derive(Component)]
//! struct Pos(f32, f32);
//! #[derive(Component)]
//! struct Vel(f32, f32);
//! struct Gravity(f32); // a resource: no derive
//!
//! let mut world = World::new();
//! world.insert_resource(Gravity(-9.8));
//! for i in 0..1000 {
//!     world.spawn((Pos(i as f32, 0.0), Vel(0.0, 0.0)));
//! }
//!
//! world
//!     .query::<&mut Vel>()
//!     .with_resources::<&Gravity>()
//!     .for_each(|v, gravity| v.1 += gravity.0);
//! world.query::<(&mut Pos, &Vel)>().for_each(|(p, v)| {
//!     p.0 += v.0;
//!     p.1 += v.1;
//! });
//! ```
//!
//! Under the hood every component, Rust or not, is a runtime-registered column
//! (name, size, alignment, drop). Rust types are registered on first use;
//! C/C++ code uses [`RawWorld::register_component`] and [`RawWorld::for_each_chunk`]
//! (see [`World::raw`]).
//!
//! Most games use this crate through [`ponko`](https://docs.rs/ponko/), whose
//! systems get the world as `ctx.world`; the guide's
//! [ECS chapter](https://ponko.dev/guide/ecs.html) explains it with examples.
//! The main types: [`World`] (entities, components, resources), the query
//! builder from [`World::query`], [`Commands`] for deferred changes, and
//! [`Component`] with its derive.

#![warn(missing_docs)]
// docs.rs: "Available on crate feature X only" on feature-gated items.
#![cfg_attr(docsrs, feature(doc_cfg))]

// `#[derive(Component)]` names the trait `::ponko_ecs::Component`, in this
// crate too (and in its doctests, where `crate` would be the doctest).
extern crate self as ponko_ecs;

mod raw;
pub use raw::{RawWorld, RawWorldRef};

/// Seals the traits this crate implements for its own types only
/// ([`QueryData`], [`ReadOnlyQueryData`], [`Bundle`], [`ResourceSet`],
/// [`ResourceTuple`]): each has a `SEAL` item of this type, and code outside
/// the crate can't write one because it can't name the type. Public but not
/// exported, like the `Filters` that seal the filter traits.
///
/// An impl that has everything but the seal doesn't compile:
///
/// ```compile_fail
/// use ponko_ecs::{ResourceTuple, World};
/// struct Mine;
/// impl ResourceTuple for Mine {
///     type Refs<'w> = ();
///     fn fetch(_: &mut World) -> Self::Refs<'_> {}
/// }
/// ```
///
/// and neither does one that tries to name it:
///
/// ```compile_fail
/// use ponko_ecs::{ResourceTuple, World};
/// struct Mine;
/// impl ResourceTuple for Mine {
///     const SEAL: ponko_ecs::seal::Seal = ponko_ecs::seal::Seal;
///     type Refs<'w> = ();
///     fn fetch(_: &mut World) -> Self::Refs<'_> {}
/// }
/// ```
mod seal {
    #[doc(hidden)]
    pub struct Seal;
}
mod access;
mod borrow;
mod bundle;
mod change;
mod commands;
mod entity;
mod query;
mod slots;
mod storage;
mod view;
mod world;

pub use access::AccessLog;
pub use borrow::{Lookup, QueryLookup, QueryResources, ReadOnlyQueryData, ResourceSet, ResourceTuple};
pub use bundle::{Bundle, Component};
pub use change::{Added, Changed, ChunkQuery, Tick};
pub use commands::Commands;
pub use entity::Entity;
/// `#[derive(Component)]`: `impl Component for Type {}`. See [`Component`](trait@Component).
#[cfg(feature = "derive")]
pub use ponko_macros::Component;
pub use query::{
    CompiledFilter, DynFilter, FilterError, FilterShape, FilterTree, MAX_FILTER_DEPTH, Or, OrTerm, OrTerms, Query,
    QueryCache, QueryCachePolicy, QueryCacheStats, QueryData, QueryFilter, QueryIter, SingleError, With, Without,
    compile_tree,
};
pub use storage::{Archetype, ComponentId, ComponentInfo, DropFn};
pub use view::{ViewGroup, ViewQuery, ViewState, WorldView};
pub use world::{Chunk, World};
