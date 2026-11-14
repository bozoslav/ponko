//! Ant colonies: ants leave their nest, wander, find food, and carry it
//! home, laying pheromone trails that other ants follow. Delivered food
//! grows the colony; ants that go too long without delivering starve.
//! Bigger runs get more colonies, so the world grows with the ant count.
//!
//!   `cargo run --release --example ants`
//!   `ANTS=50000 cargo run --release --example ants`
//!   `HEADLESS=1 ANTS=1000000 TICKS=600 cargo run --release --example ants`
//!   `SEED=7 ...` picks a different (but reproducible) world.
//!
//! Mouse: drag with the right button to pan, scroll to zoom. The panel has
//! pause and speed buttons.
//! Keys: Space pause, Period single step, 1-4 speed (1x/4x/16x/64x), Escape quit.
//!
//! Shows: the built-in spatial grid (finding food and nests), a field shared
//! by all ants (the pheromones), seeded randomness inside `par_for_each`,
//! systems at reduced rates (`system.every(n)`), entities taking turns
//! (`ctx.time.turns`), components that refer to other entities (`Ant::home`),
//! and reflection, so agents can inspect and save a running colony through
//! `ponko-mcp`.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::prelude::*;
use ponko::spatial::SpatialGrid;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};

/// Ants per colony at the start; the world has one nest per this many ants.
const ANTS_PER_NEST: usize = 5000;
/// World area per nest, in pixels (the window holds one colony).
const NEST_AREA: Vec2 = vec2(1280.0, 720.0);
/// Pixels per pheromone cell.
const CELL: f32 = 4.0;
const SPEED: f32 = 45.0;
/// How far ahead ants smell, and how wide.
const SENSE_DIST: f32 = 10.0;
const SENSE_ANGLE: f32 = 0.6;
/// Ants see food within this distance, their nest within three times it.
const SIGHT: f32 = 24.0;
const NEST_RADIUS: f32 = 16.0;
/// Trail strength below which food carriers stop following the home trail
/// and head straight for home instead.
const FAINT: f32 = 0.05;
/// Seconds an ant can go without delivering food.
const LIFESPAN: f32 = 90.0;

#[derive(Serialize, Deserialize, Component)]
struct Ant {
    /// The nest this ant belongs to (an entity), and where it is.
    home: Entity,
    home_at: Vec2,
    heading: f32,
    carrying: bool,
    /// Seconds since the ant last touched its nest or food: trails get
    /// weaker the farther the ant has wandered.
    since: f32,
    /// Seconds left before starving; refilled on delivery.
    energy: f32,
    /// What happened this tick, decided in parallel and applied after.
    #[serde(skip)]
    event: Event,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Event {
    #[default]
    None,
    Starved,
    PickedUp(Entity),
    Delivered,
}

#[derive(Serialize, Deserialize, Component)]
struct Nest {
    stored: u32,
    delivered: u64,
    max_ants: u32,
    ants: u32,
}

#[derive(Serialize, Deserialize, Component)]
struct Food {
    amount: u32,
}

/// What new ants look like: a sprite when there's a window, nothing headless.
struct AntLook(Option<Texture>);

/// Two pheromone layers over the world: "food is this way" (laid by ants
/// carrying food home) and "home is this way" (laid by searching ants).
#[derive(Default)]
struct Field {
    w: usize,
    h: usize,
    to_food: Vec<f32>,
    to_home: Vec<f32>,
}

impl Field {
    fn new(size: Vec2) -> Self {
        let (w, h) = ((size.x / CELL) as usize, (size.y / CELL) as usize);
        Self {
            w,
            h,
            to_food: vec![0.0; w * h],
            to_home: vec![0.0; w * h],
        }
    }

    fn index(&self, p: Vec2) -> Option<usize> {
        let (x, y) = ((p.x / CELL).floor(), (p.y / CELL).floor());
        (x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h)
            .then(|| y as usize * self.w + x as usize)
    }
}

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

// ------------------------------------------------------------------ systems

/// Every tick, on all cores: smell ahead, turn toward the stronger trail,
/// wobble a little, walk. Seeing food or the nest beats smelling it.
fn steer(bounds: Vec2) -> impl FnMut(&mut Ctx) {
    let mut food_grid = SpatialGrid::new(SIGHT);
    move |ctx: &mut Ctx| {
        let (dt, random) = (ctx.time.dt(), ctx.random());
        food_grid.rebuild_from(
            ctx.world
                .query::<(Entity, &d2::Transform, &Food)>()
                .iter()
                .map(|(e, t, _)| (e, t.position)),
        );
        let field = ctx.world.remove_resource::<Field>().expect("field");
        let (grid, f) = (&food_grid, &field);
        ctx.world
            .query::<(Entity, &mut d2::Transform, &mut Ant)>()
            .par_for_each(|(e, t, ant)| {
                let mut rng = random.of(e);
                let p = t.position;
                let target = if ant.carrying {
                    (p.distance(ant.home_at) < SIGHT * 3.0).then_some(ant.home_at)
                } else {
                    grid.nearest(p, SIGHT, |_| true).map(|food| food.position)
                };
                if let Some(target) = target {
                    ant.heading = (target - p).to_angle();
                } else {
                    let layer = if ant.carrying { &f.to_home } else { &f.to_food };
                    let smell = |angle: f32| {
                        let at = p + Vec2::from_angle(ant.heading + angle) * SENSE_DIST;
                        f.index(at).map_or(-1.0, |i| layer[i])
                    };
                    let (left, ahead, right) = (smell(-SENSE_ANGLE), smell(0.0), smell(SENSE_ANGLE));
                    if ant.carrying && left.max(ahead).max(right) < FAINT {
                        // No trail to follow: real ants keep track of where
                        // home is (path integration) and head that way.
                        let off = (ant.home_at - p).to_angle() - ant.heading;
                        let off = (off + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                        ant.heading += off.clamp(-2.0 * dt, 2.0 * dt);
                    } else if left > ahead && left > right {
                        ant.heading -= 4.0 * dt;
                    } else if right > ahead && right > left {
                        ant.heading += 4.0 * dt;
                    }
                }
                ant.heading += rng.range(-2.5, 2.5) * dt;
                let mut next = p + Vec2::from_angle(ant.heading) * SPEED * dt;
                // Bounce off the edges.
                if next.x < 0.0 || next.x >= bounds.x {
                    ant.heading = std::f32::consts::PI - ant.heading;
                    next.x = next.x.clamp(0.0, bounds.x - 0.01);
                }
                if next.y < 0.0 || next.y >= bounds.y {
                    ant.heading = -ant.heading;
                    next.y = next.y.clamp(0.0, bounds.y - 0.01);
                }
                t.position = next;
                t.rotation = ant.heading;
                ant.since += dt;
                ant.energy -= dt;
            });
        ctx.world.insert_resource(field);
    }
}

/// Every tick: ants mark where they walk. Searching ants lay "home is this
/// way", carrying ants lay "food is this way"; fresher trails are stronger.
fn lay_trails(ctx: &mut Ctx) {
    let mut field = ctx.world.remove_resource::<Field>().expect("field");
    ctx.world.query::<(&d2::Transform, &Ant)>().for_each(|(t, ant)| {
        if let Some(i) = field.index(t.position) {
            let strength = (1.0 - ant.since / 30.0).max(0.0);
            let layer = if ant.carrying {
                &mut field.to_food
            } else {
                &mut field.to_home
            };
            layer[i] = layer[i].max(strength);
        }
    });
    ctx.world.insert_resource(field);
}

/// Every 4th tick: trails fade.
fn evaporate(ctx: &mut Ctx) {
    let keep = 0.985f32.powf(ctx.time.dt() * 60.0);
    let field = ctx.world.resource_mut::<Field>();
    for v in field.to_food.iter_mut().chain(field.to_home.iter_mut()) {
        *v *= keep;
    }
}

/// Ants take turns (each every 2nd tick) picking up food and dropping it at
/// their nest; starving ants die. Each ant decides on all cores and notes
/// what happened in its `event`; one quick pass then applies the events.
fn forage(ctx: &mut Ctx) {
    let turn = ctx.time.turns(2);
    let mut food_grid = SpatialGrid::new(8.0);
    food_grid.rebuild_from(
        ctx.world
            .query::<(Entity, &d2::Transform, &Food)>()
            .iter()
            .map(|(e, t, _)| (e, t.position)),
    );
    let grid = &food_grid;
    ctx.world
        .query::<(Entity, &d2::Transform, &mut Ant)>()
        .par_for_each(|(e, t, ant)| {
            ant.event = Event::None;
            if ant.energy <= 0.0 {
                ant.event = Event::Starved;
                return;
            }
            if !turn.includes(e) {
                return;
            }
            let at_home = t.position.distance(ant.home_at) < NEST_RADIUS;
            if ant.carrying {
                if at_home {
                    ant.carrying = false;
                    ant.since = 0.0;
                    ant.energy = LIFESPAN;
                    ant.heading += std::f32::consts::PI;
                    ant.event = Event::Delivered;
                }
            } else if at_home {
                ant.since = 0.0;
            } else if let Some(f) = grid.nearest(t.position, 4.0, |_| true) {
                ant.carrying = true;
                ant.since = 0.0;
                ant.heading += std::f32::consts::PI;
                ant.event = Event::PickedUp(f.entity);
            }
        });

    let (mut picked, mut delivered) = (Vec::new(), Vec::new());
    let cmds = &mut ctx.commands;
    ctx.world
        .query::<(Entity, &Ant)>()
        .for_each(|(e, ant)| match ant.event {
            Event::None => {}
            Event::Starved => cmds.despawn(e),
            Event::PickedUp(food) => picked.push(food),
            Event::Delivered => delivered.push(ant.home),
        });
    for f in picked {
        if let Some(food) = ctx.world.get_mut::<Food>(f) {
            food.amount = food.amount.saturating_sub(1);
            if food.amount == 0 {
                ctx.world.despawn(f);
            }
        }
    }
    for nest in delivered {
        if let Some(n) = ctx.world.get_mut::<Nest>(nest) {
            n.stored += 1;
            n.delivered += 1;
        }
    }
}

/// Twice a second: nests turn stored food into ants, and new food appears
/// where piles ran out.
fn grow(bounds: Vec2) -> impl FnMut(&mut Ctx) {
    move |ctx: &mut Ctx| {
        let mut rng = ctx.random().stream(0);
        let sprite = ctx.world.resource::<AntLook>().0;
        let mut counts = std::collections::HashMap::new();
        ctx.world
            .query::<&Ant>()
            .for_each(|a| *counts.entry(a.home).or_insert(0u32) += 1);
        let mut nests = Vec::new();
        ctx.world
            .query::<(Entity, &d2::Transform, &mut Nest)>()
            .for_each(|(e, t, n)| {
                n.ants = counts.get(&e).copied().unwrap_or(0);
                let born = n.stored.min(n.max_ants.saturating_sub(n.ants));
                n.stored -= born;
                nests.push((e, t.position, born));
            });
        for &(home, at, born) in &nests {
            for _ in 0..born {
                spawn_ant(&mut ctx.commands, sprite, home, at, rng.angle());
            }
        }
        let piles = ctx.world.query::<&Food>().count();
        let homes: Vec<Vec2> = nests.iter().map(|n| n.1).collect();
        for _ in piles..food_piles(bounds) {
            spawn_food(&mut ctx.commands, &mut rng, bounds, &homes);
        }
    }
}

// -------------------------------------------------------------------- setup

fn food_piles(bounds: Vec2) -> usize {
    ((bounds.x * bounds.y) / 60_000.0).ceil() as usize
}

fn spawn_ant(cmds: &mut Commands, sprite: Option<Texture>, home: Entity, at: Vec2, heading: f32) {
    let ant = Ant {
        home,
        home_at: at,
        heading,
        carrying: false,
        since: 0.0,
        energy: LIFESPAN,
        event: Event::None,
    };
    let t = d2::Transform::at(at).rotated(heading);
    match sprite {
        Some(tex) => cmds.spawn((t, ant, Sprite::new(tex).with_color(Color::hex(0xD4553A)))),
        None => cmds.spawn((t, ant)),
    }
}

fn spawn_food(cmds: &mut Commands, rng: &mut Rng, bounds: Vec2, nests: &[Vec2]) {
    // Not too close to any nest, so trails have to form.
    let p = loop {
        let p = vec2(rng.range(20.0, bounds.x - 20.0), rng.range(20.0, bounds.y - 20.0));
        if nests.iter().all(|n| p.distance(*n) > 180.0) {
            break p;
        }
    };
    cmds.spawn((d2::Transform::at(p), Food { amount: 400 }));
}

/// The simulation, shared by the windowed and headless runs.
fn colonies(game: Game, bounds: Vec2) -> Game {
    game.sim_system(steer(bounds))
        .sim_system(lay_trails)
        .sim_system(forage)
        .add(Phase::AfterPhysics, evaporate.every(4))
        .add(Phase::AfterPhysics, grow(bounds).every(30))
}

/// Nests on a grid, one per `NEST_AREA`; ants and food around them.
fn setup(ctx: &mut Ctx, bounds: Vec2, ants: usize, sprite: Option<Texture>) {
    ctx.world.reflect::<Ant>();
    ctx.world.reflect::<Nest>();
    ctx.world.reflect::<Food>();
    ctx.world.insert_resource(Field::new(bounds));
    ctx.world.insert_resource(AntLook(sprite));
    let mut rng = ctx.random().stream(1);
    let (cols, rows) = ((bounds.x / NEST_AREA.x) as usize, (bounds.y / NEST_AREA.y) as usize);
    let (cols, rows) = (cols.max(1), rows.max(1));
    let per_nest = (ants / (cols * rows)).max(1);
    let mut homes = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let at = (vec2(col as f32, row as f32) + 0.5) * bounds / vec2(cols as f32, rows as f32);
            let nest = Nest {
                stored: 0,
                delivered: 0,
                max_ants: per_nest as u32 * 2,
                ants: per_nest as u32,
            };
            let home = ctx.world.spawn((d2::Transform::at(at), nest));
            homes.push(at);
            for _ in 0..per_nest {
                spawn_ant(&mut ctx.commands, sprite, home, at, rng.angle());
            }
        }
    }
    for _ in 0..food_piles(bounds) {
        spawn_food(&mut ctx.commands, &mut rng, bounds, &homes);
    }
}

fn delivered(world: &mut World) -> u64 {
    world.query::<&Nest>().iter().map(|n| n.delivered).sum()
}

// ------------------------------------------------------------------ drawing

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
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

/// Colony stats and sim controls, in a panel on top of the world.
fn hud(ctx: &mut Ctx) {
    let ants = ctx.world.query::<&Ant>().count();
    let nests = ctx.world.query::<&Nest>().count();
    let food = delivered(&mut ctx.world);
    let panel = Rect::new(12.0, 12.0, 260.0, 156.0);
    ctx.immediate_ui().panel(panel);
    let mut at = panel.min + ctx.ui.immediate.style.padding;
    let speed = if ctx.time.is_paused() {
        "paused".to_string()
    } else {
        format!("{}x", ctx.time.speed())
    };
    let stats = format!(
        "{ants} ants in {nests} colonies\n{food} food delivered\nspeed {speed}, {:.0} FPS",
        ctx.time.fps()
    );
    at.y += ctx.immediate_ui().label(at, &stats).y + 8.0;
    let pause = if ctx.time.is_paused() { "Run" } else { "Pause" };
    if ctx.immediate_ui().button(Rect::new(at.x, at.y, 72.0, 30.0), pause) {
        if ctx.time.is_paused() {
            ctx.time.resume()
        } else {
            ctx.time.pause()
        }
    }
    for (i, speed) in [1.0f32, 4.0, 16.0].into_iter().enumerate() {
        let r = Rect::new(at.x + 80.0 + i as f32 * 54.0, at.y, 48.0, 30.0);
        if ctx.immediate_ui().button(r, &format!("{speed}x")) {
            ctx.time.set_speed(speed);
            ctx.time.resume();
        }
    }
}

/// Draws the pheromone field (one texture, updated every frame), the food
/// piles and the nests. Ants are sprites, drawn by the engine.
fn draw_world(bounds: Vec2) -> impl FnMut(&mut Ctx) {
    let mut pixels = Vec::new();
    let mut dots: Vec<(Vec2, f32, Color)> = Vec::new();
    let mut field_texture = None;
    move |ctx: &mut Ctx| {
        let (w, h) = {
            let f = ctx.world.resource::<Field>();
            (f.w, f.h)
        };
        let field_tex =
            *field_texture.get_or_insert_with(|| ctx.gfx.create_texture(w as u32, h as u32, &vec![0; w * h * 4]));
        let field = ctx.world.resource::<Field>();
        pixels.clear();
        pixels.extend(field.to_food.iter().zip(&field.to_home).flat_map(|(&food, &home)| {
            let g = (food.sqrt() * 200.0) as u8;
            let b = (home.sqrt() * 120.0) as u8;
            [g / 3, g, b.max(g / 4), 255]
        }));
        ctx.gfx.update_texture(field_tex, &pixels);
        ctx.gfx.clear(Color::BLACK);
        let scale = bounds / vec2(w as f32, h as f32);
        ctx.gfx.draw_ex(
            field_tex,
            bounds * 0.5,
            DrawParams::default().with_scale(scale).with_z(-10.0),
        );

        dots.clear();
        ctx.world.query::<(&d2::Transform, &Food)>().for_each(|(t, f)| {
            dots.push((t.position, 2.0 + (f.amount as f32).sqrt() / 2.0, Color::hex(0x7BD389)));
        });
        ctx.world.query::<(&d2::Transform, &Nest)>().for_each(|(t, _)| {
            dots.push((t.position, NEST_RADIUS, Color::hex(0x8E5A3C)));
        });
        for &(p, radius, color) in &dots {
            ctx.gfx.draw_circle(p, radius, color);
        }
    }
}

fn main() {
    let ants = env_number("ANTS", ANTS_PER_NEST as f64) as usize;
    let seed = env_number("SEED", ponko::random::DEFAULT_SEED as f64) as u64;
    // One colony's worth of space per ANTS_PER_NEST ants.
    let nests = (ants as f32 / ANTS_PER_NEST as f32).max(1.0);
    let side = nests.sqrt().ceil();
    let bounds = NEST_AREA * vec2(side, (nests / side).ceil());

    if env_number("HEADLESS", 0.0) != 0.0 {
        let ticks = env_number("TICKS", 600.0) as u64;
        let mut start = ponko::perf::Instant::now();
        // The two-argument form may borrow a local, here the timer.
        let mut world = colonies(Game::new("ants", 1, 1).seed(seed), bounds).run_headless(ticks, |ctx| {
            setup(ctx, bounds, ants, None);
            start = ponko::perf::Instant::now();
        });
        let secs = start.elapsed().as_secs_f64();
        println!(
            "{ticks} ticks: {:.2} ms/tick ({:.0} ticks/s). {} ants, {} food delivered",
            secs * 1e3 / ticks as f64,
            ticks as f64 / secs,
            world.query::<&Ant>().count(),
            delivered(&mut world)
        );
        return;
    }

    colonies(Game::new("Ponko: ants", 1280, 720).seed(seed), bounds)
        .system(controls)
        .system(ponko::camera::camera_controls)
        .system(draw_world(bounds))
        .system(hud)
        .system(report())
        .setup(move |ctx| {
            let sprite = ctx.gfx.create_texture(3, 2, &[255; 3 * 2 * 4]);
            setup(ctx, bounds, ants, Some(sprite));
            // Start with the whole world in view.
            let fit = ctx.window_size() / bounds;
            ctx.camera.zoom = fit.x.min(fit.y);
            ctx.camera.look_at(bounds / 2.0);
        })
        .run();
}

/// Frame system with its own state: prints the numbers once a second.
fn report() -> impl FnMut(&mut Ctx) {
    let mut next = 1.0;
    move |ctx: &mut Ctx| {
        if ctx.time.elapsed() >= next {
            next += 1.0;
            let sim_ms: f32 = ctx
                .perf
                .system_timings()
                .iter()
                .filter(|t| t.sim)
                .map(|t| t.avg_ms)
                .sum();
            println!(
                "ants: {}  delivered: {}  fps: {:.0}  sim: {sim_ms:.2} ms/frame  tick: {}",
                ctx.world.query::<&Ant>().count(),
                delivered(&mut ctx.world),
                ctx.time.fps(),
                ctx.time.sim_tick()
            );
        }
    }
}
