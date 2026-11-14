//! A playground for the editor in the inspector.
//!
//!   `cargo run --example editor`
//!
//! Crabs walk around and bounce off the edges (in a sim system, so pausing
//! stops them). Press ` (backquote, left of 1) to open the inspector, then
//! Pause: now you're editing.
//!
//! - Click a crab to select it, drag to move it; right-drag pans, wheel zooms.
//! - Delete, Ctrl/Cmd+D duplicate, Ctrl/Cmd+Z undo.
//! - Edit → Prefab: the `crab` button, then click in the world to place one.
//! - Play runs the crabs; Stop puts them back where they were at Play.
//! - Edit → Scene → Save writes `assets/levels/level.json`; the next start
//!   loads it instead of the default crabs.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::prelude::*;
use serde::{Deserialize, Serialize};

/// Walks in a straight line, turning around at the window's edges.
#[derive(Serialize, Deserialize, Clone, Copy, Component)]
#[serde(default)]
struct Walker {
    /// Pixels per second.
    speed: f32,
    /// Direction, in radians.
    heading: f32,
}

impl Default for Walker {
    fn default() -> Self {
        Self {
            speed: 60.0,
            heading: 0.0,
        }
    }
}

const LEVEL: &str = "levels/level.json";

fn walk(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let bounds = ctx.window_size();
    ctx.world
        .query::<(&mut d2::Transform, &mut Walker)>()
        .for_each(|(t, w)| {
            let mut v = Vec2::from_angle(w.heading) * w.speed;
            t.position += v * dt;
            if !(0.0..=bounds.x).contains(&t.position.x) {
                v.x = -v.x;
            }
            if !(0.0..=bounds.y).contains(&t.position.y) {
                v.y = -v.y;
            }
            t.position = t.position.clamp(Vec2::ZERO, bounds);
            w.heading = v.to_angle();
        });
}

fn main() {
    Game::new("Ponko: editor", 1280, 720)
        .inspector(true)
        .sim_system(walk)
        .setup(|ctx| {
            ctx.gfx.clear(Color::hex(0x24323F));
            ctx.world.reflect::<Walker>();
            if ctx.load_scene(LEVEL).is_ok() {
                println!("loaded assets/{LEVEL}");
                return;
            }
            let crab = ctx.gfx.load_texture("crab.png");
            let mut rng = Rng::new(7);
            for _ in 0..12 {
                let at = vec2(rng.range(100.0, 1180.0), rng.range(100.0, 620.0));
                let walker = Walker {
                    speed: rng.range(30.0, 90.0),
                    heading: rng.range(0.0, std::f32::consts::TAU),
                };
                ctx.world.spawn((d2::Transform::at(at), Sprite::new(crab), walker));
            }
        })
        .run();
}
