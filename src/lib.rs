//! # Ponko
//!
//! A code-first 2D and 3D game engine with integrated simulation, C++
//! bindings, and tools for AI-assisted development.
//!
//! ```no_run
//! use ponko::prelude::*;
//!
//! fn main() {
//!     Game::new("My Game", 800, 600).run(|ctx| {
//!         let crab = ctx.load_texture("crab.png");
//!         let mut pos = vec2(400.0, 300.0);
//!
//!         move |ctx: &mut Ctx| {
//!             if ctx.key_down(Key::Right) { pos.x += 200.0 * ctx.dt(); }
//!             if ctx.key_down(Key::Left)  { pos.x -= 200.0 * ctx.dt(); }
//!             if ctx.key_pressed(Key::Escape) { ctx.quit(); }
//!             ctx.draw(crab, pos);
//!         }
//!     });
//! }
//! ```
//!
//! Start with `use ponko::prelude::*;`, then add dimension and service modules as needed.
//!
//! The [guide](https://ponko.dev/guide/) explains the engine chapter by
//! chapter, starting with [Getting started](https://ponko.dev/guide/getting-started.html);
//! this reference documents every item.
//!
//! ## Where things are
//!
//! - [`prelude`]: what a typical game needs, in one import.
//! - [`game`]: [`Game`](game::Game), the builder that sets up and runs a
//!   game, its systems and phases; `Ctx`, the per-frame context, is in the prelude.
//! - [`d2`] and `d3` (feature `3d`): 2D and 3D drawing, cameras, scenes and
//!   models; `terrain` (feature `3d`) and [`tilemap`] for worlds.
//! - The ECS: entities, components, queries and resources live on the
//!   game's world (`ctx.world`), with systems added on [`Game`](game::Game);
//!   [`events`] carries messages between systems, [`systems`] holds the
//!   engine's built-in systems and [`schedule`] explains which run together.
//! - [`input`] and [`actions`]: keys, mouse, gamepads and rebindable actions.
//! - [`ui`] and [`text`]: the retained UI, fonts and text layout.
//! - [`audio`]: sound effects and music.
//! - `physics` and `physics3d` (features of the same names): rigid bodies,
//!   colliders and queries; [`nav`] for pathfinding and navigation meshes.
//! - [`assets`], [`stream`] and [`streaming`]: asset files, background jobs
//!   and streaming big worlds, and GPU asset loading with budgets.
//! - [`net`]: networking: lockstep, messages and snapshot replication.
//! - [`perf`], [`diagnostics`] and [`render`]: frame profiling, ECS
//!   diagnostics and renderer statistics.
//! - [`reflect`] and [`scenario`]: components by name as JSON, and scripted
//!   input for gameplay tests, for tools and AI agents.
//!
//! ## Features
//!
//! - `window` (default): a window and GPU rendering (winit + wgpu). Without
//!   it the engine is simulation only: `Game::run_headless` works, drawing
//!   calls do nothing, `Game::run` panics. For simulation servers and CI,
//!   with a much smaller build.
//! - `audio` (default): sound through kira; without it sounds are silent.
//! - `inspector` (default): the in-game inspector overlay (egui), opened
//!   with the backquote key. On in debug builds; see `Game::inspector`.
//! - `gamepad` (default): gamepads through gilrs; without it
//!   `Input::gamepads` is always empty. On Linux it needs libudev headers.
//! - `profiling`: per-system scopes for `puffin_viewer`. (Not needed for
//!   [`profile_scope!`], [`Perf::report`](perf::Perf::report) or [`CountingAllocator`](alloc_count::CountingAllocator), which
//!   are always built in and cost one atomic load while profiling is off.)
//! - `hotpatch`: change system code while the game runs, with
//!   `dx serve --hot-patch --features hotpatch`.
//! - `3d`: meshes, materials, glTF models, a perspective camera, lights and
//!   shadows, drawn under the 2D layer (`ponko::d3`, `ctx.scene3d`).
//! - `navmesh`: navigation meshes for 3D (Recast baking and Detour path and
//!   crowd queries, in pure Rust). Turns on `3d`.
//! - `physics`: 2D physics with rapier (`Game::physics`).
//! - `physics3d`: 3D physics with rapier (`Game::physics3d`): bodies,
//!   colliders (heightfield terrain too), queries, a character controller and
//!   raycast-wheel vehicles. Turns on `3d`.
//! - `physics-determinism`: physics with bit-identical results on every
//!   platform (for lockstep and replays), a little slower. Turns on `physics`.
//! - `control-env`: a release build honors `PONKO_CONTROL=1` and opens the
//!   control endpoint for tools (debug builds always do). Off, a release build
//!   opens it only when the game calls `Game::control_endpoint(true)`.
//! - `accessibility`: screen readers read and operate the retained UI of the
//!   game's window (`ui::UiAccessAdapter`), through AccessKit: macOS
//!   (VoiceOver), Windows (UI Automation: Narrator, NVDA) and Linux (AT-SPI:
//!   Orca). On the web it builds and does nothing. Needs `window`.
//! - `steam`: lockstep and byte messages over Steam's relays and lobbies
//!   (`Net::steam_host`), with the `steamworks` crate re-exported as `ponko::net::steam`.
//! - `bench-internals`: **unstable** benchmark drivers for the engine's own
//!   benchmark crate. Not part of the API; they change in any release.
//!
//! What a 0.x release may change, and what it keeps, is in
//! [STABILITY.md](https://github.com/ponkoengine/ponko/blob/main/STABILITY.md).

#![warn(missing_docs)]
// docs.rs: "Available on crate feature X only" on feature-gated items.
#![cfg_attr(docsrs, feature(doc_cfg))]
// Without a window, the code that only the window loop calls is unused.
#![cfg_attr(not(feature = "window"), allow(dead_code))]

// `#[derive(Component)]` names the trait `::ponko::prelude::Component`, in
// this crate too; tests also include example files as they are.
extern crate self as ponko;

pub mod anim;
pub mod d2;
#[cfg(feature = "3d")]
pub mod d3;
pub mod nav;
pub mod prelude;
#[cfg(feature = "3d")]
pub mod terrain;

pub mod access;
pub mod actions;
pub mod alloc_count;
mod asset_cache;
pub mod assets;
pub mod attach;
pub mod audio;
pub mod camera;
#[cfg(any(feature = "physics", feature = "physics3d"))]
pub mod collision;
pub mod color;
#[cfg(feature = "compressed-textures")]
pub(crate) mod compressed;
mod control;
mod ctx;
pub mod diagnostics;
mod ecs;
#[cfg(feature = "3d")]
mod edit3d;
mod editor;
pub mod error;
pub mod events;
pub mod flow;
pub mod game;
#[cfg(feature = "gamepad")]
mod gamepad;
pub mod gfx;
pub mod handle;

/// Raw handle ids, for the engine's own language bindings (`ponko_ffi`),
/// whose C handles are plain ids. **Not part of the API**: hidden, unstable,
/// and changed or removed in any release without notice. Games never need
/// it: handles come from the engine, and a made-up id names nothing, or
/// something else.
#[doc(hidden)]
pub mod __ffi {
    use crate::handle::raw::RawHandle;

    /// The id inside a handle of one id. Unstable (see the [module](self)).
    #[must_use]
    pub fn id<H: RawHandle>(handle: H) -> u32 {
        handle.raw_id()
    }

    /// A handle from an id from [`id`]. Sizes and lengths read 0; check it
    /// with `Ctx::is_live` before trusting it. Unstable (see the
    /// [module](self)).
    #[must_use]
    pub fn from_id<H: RawHandle>(id: u32) -> H {
        H::from_raw_id(id)
    }

    /// The first system named `name` (in phase order), for removal by
    /// name. Unstable (see the [module](self)).
    #[must_use]
    pub fn system_named(game: &crate::game::Game, name: &str) -> Option<crate::game::SystemId> {
        game.system_named(name)
    }

    /// The slot bits of a mesh's, GPU buffer's or collision mesh's id: the
    /// bits above count how often the slot was given out. Unstable.
    #[cfg(feature = "3d")]
    pub const SLOT_MASK: u32 = crate::handle::INDEX_MASK;
}
mod hot_reload;
pub mod input;
pub mod inventory;

#[cfg(feature = "inspector")]
mod inspector;
pub mod light2d;
mod logs;
#[cfg(feature = "navmesh")]
mod nav3d;
pub mod net;
pub mod particles;

pub mod perf;
mod perf_report;
#[cfg(feature = "physics")]
pub mod physics;
#[cfg(feature = "physics3d")]
pub mod physics3d;
mod profiler;
#[cfg(any(all(target_arch = "wasm32", feature = "window"), test))]
mod query;
pub mod random;
pub mod reflect;
pub mod render;
mod residency;
pub mod scenario;
mod scene;
pub mod schedule;
mod schema;
pub mod scope;
pub mod sheet;
pub mod spatial;
mod sprite_material;
pub mod storage;
pub mod stream;
pub mod streaming;
pub mod system_view;
pub mod systems;
pub mod text;
pub mod texture;
#[cfg(feature = "3d")]
mod three;
pub mod tilemap;
mod time;
pub mod ui;
#[cfg(all(target_arch = "wasm32", feature = "window"))]
mod web;

/// The whole ECS crate ([`ponko_ecs`]), for advanced use: component ids,
/// read-only world views, the dynamic and C path ([`World::raw`](ponko_ecs::World::raw)), chunks
/// and archetypes. Games normally need only the ECS types in the prelude.
pub mod ecs_core {
    #[doc(no_inline)]
    pub use ponko_ecs::*;
}
