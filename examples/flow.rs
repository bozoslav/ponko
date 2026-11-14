//! Crowds finding their way: every agent follows one shared flow field
//! around walls and through (slow) mud to the goal.
//!
//!   `cargo run --release --example flow`
//!   `AGENTS=200000 cargo run --release --example flow`
//!   `HEADLESS=1 AGENTS=1000000 TICKS=600 cargo run --release --example flow`
//!
//! Mouse: left click moves the goal; drag with the right button to pan,
//! scroll to zoom. Keys: H shows the distance field, Space pause,
//! Escape quit.
//!
//! Shows: `NavGrid` and `FlowField` (one search shared by every agent),
//! looked up from `par_for_each`, recomputed when the goal moves.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::flow::{FlowField, NavGrid};
#[allow(unused_imports)]
use ponko::prelude::*;
use serde::{Deserialize, Serialize};

/// World units per grid cell.
// ANCHOR: constants
const CELL: f32 = 4.0;
const MAP: UVec2 = uvec2(320, 180); // 1280 x 720 world units
const SPEED: f32 = 40.0;
const MUD: u8 = 6;
// ANCHOR_END: constants

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Serialize, Deserialize, Clone, Copy, Component)]
struct Agent {
    vel: Vec2,
}

/// The map, the field, and where everyone is going.
struct Paths {
    nav: NavGrid,
    flow: FlowField,
    goal: Vec2,
    /// Milliseconds the last `compute` took.
    compute_ms: f32,
}

impl Paths {
    fn set_goal(&mut self, goal: Vec2) {
        let start = ponko::perf::Instant::now();
        self.goal = goal;
        self.flow.compute(&self.nav, &[goal]);
        self.compute_ms = start.elapsed().as_secs_f32() * 1000.0;
    }
}

/// Walls in blobs, a few mud patches, and a border, from `seed`.
fn make_map(seed: u64) -> NavGrid {
    let mut nav = NavGrid::new(MAP.x, MAP.y, CELL);
    let mut rng = Rng::new(seed);
    let mut blob = |nav: &mut NavGrid, cost: u8, r: f32| {
        let c = vec2(rng.range(0.0, MAP.x as f32), rng.range(0.0, MAP.y as f32));
        for y in (c.y - r).max(0.0) as u32..((c.y + r) as u32).min(MAP.y) {
            for x in (c.x - r).max(0.0) as u32..((c.x + r) as u32).min(MAP.x) {
                if vec2(x as f32, y as f32).distance(c) < r {
                    nav.set_cost(x, y, cost);
                }
            }
        }
    };
    for _ in 0..14 {
        blob(&mut nav, MUD, 14.0);
    }
    for _ in 0..90 {
        blob(&mut nav, NavGrid::WALL, 6.0);
    }
    for x in 0..MAP.x {
        nav.set_cost(x, 0, NavGrid::WALL);
        nav.set_cost(x, MAP.y - 1, NavGrid::WALL);
    }
    for y in 0..MAP.y {
        nav.set_cost(0, y, NavGrid::WALL);
        nav.set_cost(MAP.x - 1, y, NavGrid::WALL);
    }
    nav
}

/// A random walkable point.
fn walkable_point(nav: &NavGrid, rng: &mut Rng) -> Vec2 {
    loop {
        let p = vec2(rng.range(0.0, MAP.x as f32), rng.range(0.0, MAP.y as f32)) * CELL;
        if nav.walkable(p) {
            return p;
        }
    }
}

// ANCHOR: steer
/// Every agent steers along the field; mud slows it down, walls stop it.
fn steer(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let random = ctx.random();
    // Taken out for the query, which borrows the world, then put back.
    let paths = ctx.world.remove_resource::<Paths>().expect("paths");
    ctx.world
        .query::<(Entity, &mut d2::Transform, &mut Agent)>()
        .par_for_each(|(e, t, a)| {
            let mut rng = random.of(e);
            let cell = paths.nav.cell_at(t.position);
            let slow = cell.and_then(|c| paths.nav.cost(c.x, c.y)).unwrap_or(1).max(1) as f32;
            let want = paths.flow.direction_at(t.position) * (SPEED / slow);
            a.vel = a.vel.lerp(want, 0.15) + rng.unit_vec2() * 3.0;
            // Slide along walls instead of walking into them.
            let next = t.position + a.vel * dt;
            if paths.nav.walkable(next) {
                t.position = next;
            } else if paths.nav.walkable(vec2(next.x, t.position.y)) {
                t.position.x = next.x;
            } else if paths.nav.walkable(vec2(t.position.x, next.y)) {
                t.position.y = next.y;
            }
        });
    ctx.world.insert_resource(paths);
}
// ANCHOR_END: steer

fn setup(ctx: &mut Ctx, agents: usize, texture: Option<Texture>) {
    let seed = 7;
    let nav = make_map(seed);
    let mut rng = Rng::new(seed ^ 1);
    let goal = walkable_point(&nav, &mut rng);
    let mut paths = Paths {
        nav,
        flow: FlowField::new(),
        goal,
        compute_ms: 0.0,
    };
    paths.set_goal(goal);
    ctx.world.reflect::<Agent>();
    let starts: Vec<Vec2> = (0..agents).map(|_| walkable_point(&paths.nav, &mut rng)).collect();
    ctx.world.insert_resource(paths);
    match texture {
        Some(tex) => ctx.world.spawn_batch(starts.into_iter().map(|p| {
            (
                d2::Transform::at(p),
                Sprite::new(tex).with_color(Color::rgb(1.0, 0.85, 0.4)),
                Agent { vel: Vec2::ZERO },
            )
        })),
        None => ctx.world.spawn_batch(
            starts
                .into_iter()
                .map(|p| (d2::Transform::at(p), Agent { vel: Vec2::ZERO })),
        ),
    };
}

/// The map picture and what it shows now. Made once at startup.
struct MapView {
    tex: Texture,
    pixels: Vec<u8>,
    /// Colour the cells by their distance to the goal (H toggles it).
    heat: bool,
    /// The goal and mode the texture was last painted for.
    drawn: Option<(Vec2, bool)>,
}

/// Startup: the texture the map is painted into.
fn make_map_view(ctx: &mut Ctx) {
    let (w, h) = (MAP.x as usize, MAP.y as usize);
    let tex = ctx.gfx.create_texture(MAP.x, MAP.y, &vec![0; w * h * 4]);
    ctx.gfx.set_texture_filter(tex, TextureFilter::Nearest);
    ctx.world.insert_resource(MapView {
        tex,
        pixels: vec![0u8; w * h * 4],
        heat: false,
        drawn: None,
    });
}

/// Frame system: repaints the map when the goal or the mode changed, then
/// draws it with the goal on top.
fn draw_map(ctx: &mut Ctx) {
    let (w, h) = (MAP.x as usize, MAP.y as usize);
    let toggle = ctx.input.key_pressed(Key::H);
    let (paths, view) = ctx.world.resources_mut::<(Paths, MapView)>();
    view.heat ^= toggle;
    let (goal, heat, tex) = (paths.goal, view.heat, view.tex);
    if view.drawn != Some((goal, heat)) {
        view.drawn = Some((goal, heat));
        for y in 0..h {
            for x in 0..w {
                let center = paths.nav.center_of(uvec2(x as u32, y as u32));
                let px = match paths.nav.cost(x as u32, y as u32).unwrap() {
                    NavGrid::WALL => [40, 44, 52, 255],
                    MUD => [70, 52, 30, 255],
                    _ => [24, 40, 30, 255],
                };
                let px = match paths.flow.distance_at(center) {
                    Some(d) if heat && px[0] != 40 => {
                        let t = (d / 1500.0).min(1.0);
                        [
                            (255.0 * (1.0 - t)) as u8 / 2 + px[0] / 2,
                            px[1],
                            (255.0 * t) as u8 / 2 + px[2] / 2,
                            255,
                        ]
                    }
                    _ => px,
                };
                view.pixels[(y * w + x) * 4..][..4].copy_from_slice(&px);
            }
        }
        // The upload needs all of `ctx`, so the pixels leave the resource for it.
        let pixels = std::mem::take(&mut view.pixels);
        ctx.gfx.update_texture(tex, &pixels);
        ctx.world.resource_mut::<MapView>().pixels = pixels;
    }
    ctx.gfx.clear(Color::BLACK);
    let size = MAP.as_vec2() * CELL;
    ctx.gfx.draw_ex(
        tex,
        size / 2.0,
        DrawParams::default().with_scale(Vec2::splat(CELL)).with_z(-10.0),
    );
    ctx.gfx.draw_circle(goal, 8.0, Color::rgba(1.0, 0.3, 0.3, 0.8));
}

fn click_to_move_goal(ctx: &mut Ctx) {
    if ctx.input.mouse_pressed(MouseButton::Left) && !ctx.ui.wants_pointer() {
        let at = ctx.mouse_world();
        let paths = ctx.world.resource_mut::<Paths>();
        if paths.nav.walkable(at) {
            paths.set_goal(at);
        }
    }
    if ctx.input.key_pressed(Key::Space) {
        if ctx.time.is_paused() {
            ctx.time.resume()
        } else {
            ctx.time.pause()
        }
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

fn hud(ctx: &mut Ctx) {
    let panel = Rect::new(12.0, 12.0, 260.0, 104.0);
    ctx.immediate_ui().panel(panel);
    let at = panel.min + ctx.ui.immediate.style.padding;
    let agents = ctx.world.query::<&Agent>().count();
    let ms = ctx.world.resource::<Paths>().compute_ms;
    let text = format!(
        "{agents} agents, {:.0} FPS\nfield: {}x{} cells, {ms:.2} ms\nclick: new goal   H: distances",
        ctx.time.fps(),
        MAP.x,
        MAP.y
    );
    ctx.immediate_ui().label(at, &text);
}

fn main() {
    let agents = env_number("AGENTS", 20_000.0) as usize;
    if env_number("HEADLESS", 0.0) != 0.0 {
        let ticks = env_number("TICKS", 600.0) as u64;
        let mut start = ponko::perf::Instant::now();
        let mut rng = Rng::new(99);
        let mut field_ms = Vec::new();
        Game::new("flow", 1, 1)
            .sim_system(steer)
            .sim_system(move |ctx: &mut Ctx| {
                // A new goal every 2 seconds, like a player clicking around.
                let tick = ctx.time.sim_tick();
                if tick % 120 == 119 {
                    let paths = ctx.world.resource_mut::<Paths>();
                    let goal = walkable_point(&paths.nav, &mut rng);
                    paths.set_goal(goal);
                    field_ms.push(paths.compute_ms);
                    let avg = field_ms.iter().sum::<f32>() / field_ms.len() as f32;
                    println!(
                        "new goal at tick {tick}: field {:.2} ms (avg {avg:.2})",
                        paths.compute_ms
                    );
                }
            })
            // The two-argument form may borrow a local, here the timer.
            .run_headless(ticks, |ctx| {
                setup(ctx, agents, None);
                start = ponko::perf::Instant::now();
            });
        let secs = start.elapsed().as_secs_f64();
        println!(
            "{agents} agents, {ticks} ticks: {:.2} ms/tick ({:.0} ticks/s)",
            secs * 1e3 / ticks as f64,
            ticks as f64 / secs
        );
        return;
    }

    Game::new("Ponko: flow fields", 1280, 720)
        .sim_system(steer)
        .system(click_to_move_goal)
        .system(ponko::camera::camera_controls)
        .system(hud)
        .system(draw_map)
        .setup(move |ctx| {
            let dot = ctx.gfx.create_texture(2, 2, &[255; 2 * 2 * 4]);
            setup(ctx, agents, Some(dot));
            make_map_view(ctx);
        })
        .run();
}
