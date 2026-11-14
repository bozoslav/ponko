//! The game's rules, shared by the game (`src/main.rs`) and its tests
//! (`tests/game.rs`).
//!
//! Game logic lives in sim systems, so a headless test (or an agent driving
//! the game through `ponko-mcp`) can run it without a window or GPU.

use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{MaterialDesc, MeshData};
use serde::{Deserialize, Serialize};

/// Numbers to tune without recompiling, loaded from `assets/settings.json`.
/// In debug builds, saving the file changes them in the running game.
#[derive(Serialize, Deserialize)]
pub struct Settings {
    /// How fast the cube turns, in radians per second.
    pub spin_speed: f32,
}

/// Turns its entity around the Y axis at `Settings::spin_speed`.
#[derive(Component, Serialize, Deserialize)]
pub struct Spin;

/// The window title (and the name agents see).
pub const TITLE: &str = "{{project-name}}";

/// The whole game, ready to `run()` in a window or `run_headless(ticks)`.
pub fn game() -> ConfiguredGame {
    Game::new(TITLE, 1280, 720)
        .sim_system(spin)
        .system(controls)
        .setup(setup)
}

fn setup(ctx: &mut Ctx) {
    ctx.assets
        .load_resource::<Settings>(&mut ctx.world, "settings.json");
    // Reflected types are what agents can read and change through ponko-mcp.
    ctx.world.reflect::<Spin>();
    ctx.world.reflect::<Transform>();
    ctx.world.reflect_resource::<Settings>();

    ctx.scene3d.camera.position = vec3(3.0, 2.5, 4.0);
    ctx.scene3d.camera.look_at(Vec3::ZERO);
    ctx.scene3d.lighting.sun_direction = vec3(-0.6, -1.0, -0.4);

    let grass = ctx
        .gfx
        .create_material(MaterialDesc::color(Color::rgb(0.37, 0.55, 0.29)));
    let ground = ctx.gfx.create_mesh(&MeshData::plane(20.0), grass);
    ctx.world
        .spawn((Transform::at(vec3(0.0, -0.5, 0.0)), Model::new(ground)));

    let orange = ctx
        .gfx
        .create_material(MaterialDesc::color(Color::rgb(0.95, 0.55, 0.2)));
    let cube = ctx.gfx.create_mesh(&MeshData::cube(1.0), orange);
    ctx.world
        .spawn((Transform::default(), Model::new(cube), Spin));
}

/// Sim system: turns every `Spin` entity.
pub fn spin(ctx: &mut Ctx) {
    let turn = Quat::from_rotation_y(ctx.world.resource::<Settings>().spin_speed * ctx.time.dt());
    ctx.world
        .query::<(&mut Transform, &Spin)>()
        .for_each(|(t, _)| t.rotation = turn * t.rotation);
}

/// Frame system: Space pauses the simulation, Escape quits.
fn controls(ctx: &mut Ctx) {
    if ctx.input.key_pressed(Key::Space) {
        if ctx.time.is_paused() {
            ctx.time.resume();
        } else {
            ctx.time.pause();
        }
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}
