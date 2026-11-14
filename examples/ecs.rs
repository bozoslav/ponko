//! ECS basics: components, simulation systems, a resource and commands.
//!
//!   `cargo run --example ecs`
//!
//! - Crabs wander around and fade out as they age; old crabs are despawned.
//! - Click (or hold) the left mouse button to spawn more.
//! - Space: pause / resume.  Period: one tick while paused.
//! - 1 / 2 / 3 / 4: simulation speed 1x / 4x / 16x / 64x.  Escape: quit.

#[allow(unused_imports)]
#[cfg(feature = "physics")]
use ponko::d2::*;
#[allow(unused_imports)]
use ponko::prelude::*;

/// Velocity in pixels per second.
#[derive(Component)]
struct Vel(Vec2);
/// Seconds left to live.
#[derive(Component)]
struct Life(f32);

/// Resource: shared settings and a tiny random generator.
struct Settings {
    rng: u64,
    crab: Texture,
}

impl Settings {
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }
}

const LIFETIME: f32 = 8.0;

fn spawn_crab(ctx: &mut Ctx, at: Vec2) {
    let s = ctx.world.resource_mut::<Settings>();
    let angle = s.random() * std::f32::consts::TAU;
    let speed = 40.0 + s.random() * 120.0;
    let tint = Color::rgb(0.5 + s.random() * 0.5, 0.5 + s.random() * 0.5, 0.5 + s.random() * 0.5);
    let crab = s.crab;
    ctx.world.spawn((
        d2::Transform::at(at),
        Sprite::new(crab).with_color(tint),
        Vel(Vec2::from_angle(angle) * speed),
        Life(LIFETIME),
    ));
}

/// Simulation: move and bounce off the window edges.
fn movement(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let bounds = ctx.window_size();
    ctx.world.query::<(&mut d2::Transform, &mut Vel)>().for_each(|(t, v)| {
        t.position += v.0 * dt;
        t.rotation = v.0.to_angle();
        if t.position.x < 0.0 || t.position.x > bounds.x {
            v.0.x = -v.0.x;
        }
        if t.position.y < 0.0 || t.position.y > bounds.y {
            v.0.y = -v.0.y;
        }
        t.position = t.position.clamp(Vec2::ZERO, bounds);
    });
}

/// Simulation: age, fade out, and despawn through commands.
fn aging(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let commands = &mut ctx.commands;
    ctx.world
        .query::<(Entity, &mut Life, &mut Sprite)>()
        .for_each(|(e, life, sprite)| {
            life.0 -= dt;
            sprite.color = sprite.color.with_alpha((life.0 / LIFETIME).clamp(0.0, 1.0));
            if life.0 <= 0.0 {
                commands.despawn(e);
            }
        });
}

/// Frame system: input.
fn controls(ctx: &mut Ctx) {
    if ctx.input.key_pressed(Key::Space) {
        if ctx.time.is_paused() {
            ctx.time.resume()
        } else {
            ctx.time.pause()
        }
    }
    if ctx.input.key_pressed(Key::Period) {
        ctx.time.step();
    }
    for (key, speed) in [(Key::Num1, 1.0), (Key::Num2, 4.0), (Key::Num3, 16.0), (Key::Num4, 64.0)] {
        if ctx.input.key_pressed(key) {
            ctx.time.set_speed(speed);
        }
    }
    if ctx.input.mouse_down(MouseButton::Left) {
        let at = ctx.input.mouse_pos();
        for _ in 0..5 {
            spawn_crab(ctx, at);
        }
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

fn main() {
    Game::new("Ponko: ECS", 1280, 720)
        .sim_system(movement)
        .sim_system(aging)
        .system(controls)
        .system(report())
        .setup(|ctx| {
            let crab = ctx.gfx.load_texture("crab.png");
            ctx.world.insert_resource(Settings {
                rng: 0x9E37_79B9_7F4A_7C15,
                crab,
            });
            let center = ctx.window_size() / 2.0;
            for _ in 0..500 {
                spawn_crab(ctx, center);
            }
        })
        .run();
}

/// A frame system with its own state (the time of the next report): prints
/// the numbers once a second.
fn report() -> impl FnMut(&mut Ctx) {
    let mut next_report = 1.0;
    move |ctx: &mut Ctx| {
        if ctx.time.elapsed() >= next_report {
            next_report += 1.0;
            let sim = if ctx.time.is_paused() {
                "paused".to_string()
            } else {
                format!("{}x", ctx.time.speed())
            };
            println!(
                "crabs: {:>6}  fps: {:>5.1}  sim: {sim:>6}  tick: {}",
                ctx.world.len(),
                ctx.time.fps(),
                ctx.time.sim_tick()
            );
        }
    }
}
