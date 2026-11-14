//! Two crab armies fight over a map of rocks, lakes and mud. Each base
//! keeps sending units; units march along their side's flow field, fight
//! whatever enemy they meet, and tear down the enemy base when they reach
//! it. You command red: click to set the rally point. Blue attacks.
//!
//!   `cargo run --release --example battle`
//!   `UNITS=200000 cargo run --release --example battle`
//!   `HEADLESS=1 UNITS=1000000 TICKS=600 cargo run --release --example battle`
//!   `SEED=3 ...` picks a different (but reproducible) map.
//!
//! Mouse: left click sets red's rally point; drag with the right button to
//! pan, scroll to zoom. Keys: A attack (rally on blue's base), D defend
//! (rally at home), Space pause, 1-3 speed, R restart, Escape quit.
//!
//! Shows: flow fields shared by a whole army (`NavGrid`, `FlowField`), a
//! spatial grid with data (the team of every unit) for targeting on all
//! cores, decide-in-parallel then apply-in-order combat, deterministic
//! seeded randomness, and reflection, so `ponko-mcp` agents can inspect a
//! running battle (the `Score` resource says who's winning) or change any unit.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::flow::{FlowField, NavGrid};
#[allow(unused_imports)]
use ponko::prelude::*;
use ponko::spatial::SpatialGrid;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};

/// World units per nav cell.
const CELL: f32 = 4.0;
/// Map size (in cells) for `UNITS_PER_MAP` units; bigger armies get a
/// bigger map, so the crowd density stays the same.
const MAP: UVec2 = uvec2(400, 225);
const UNITS_PER_MAP: f32 = 20_000.0;
const MUD: u8 = 5;

const SPEED: f32 = 34.0;
const HP: f32 = 10.0;
const DAMAGE: f32 = 2.0;
/// Seconds between attacks.
const COOLDOWN: f32 = 0.6;
/// Units see enemies this far away, and hit them this close.
const SIGHT: f32 = 18.0;
const REACH: f32 = 5.0;
/// Friends closer than this push each other apart.
const PERSONAL_SPACE: f32 = 3.5;
const BASE_RADIUS: f32 = 22.0;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
enum Team {
    Red,
    Blue,
}

impl Team {
    fn index(self) -> usize {
        self as usize
    }
    fn enemy(self) -> Team {
        match self {
            Team::Red => Team::Blue,
            Team::Blue => Team::Red,
        }
    }
    fn color(self) -> Color {
        match self {
            Team::Red => Color::hex(0xFF6B5B),
            Team::Blue => Color::hex(0x5BB8FF),
        }
    }
}

#[derive(Serialize, Deserialize, Component)]
struct Unit {
    team: Team,
    hp: f32,
    vel: Vec2,
    /// Seconds until the next attack.
    cooldown: f32,
    /// An enemy was in sight at the last look: look again every tick.
    engaged: bool,
    /// Away from crowding friends; refreshed every other tick.
    #[serde(skip)]
    push: Vec2,
    /// What this unit hit this tick, decided in parallel, applied after.
    #[serde(skip)]
    hit: Option<Hit>,
}

#[derive(Clone, Copy)]
enum Hit {
    Unit(Entity),
    Base,
}

#[derive(Serialize, Deserialize, Component)]
struct Base {
    team: Team,
    hp: f32,
    max_hp: f32,
    /// Units this base may still send (fractional, refilled each tick).
    budget: f32,
}

/// The map, one flow field per team, and where each team is headed.
struct Battle {
    nav: NavGrid,
    fields: [FlowField; 2],
    rally: [Vec2; 2],
    bases: [Vec2; 2],
    base_entities: [Entity; 2],
    /// Units per team the bases keep alive.
    cap: usize,
    field_ms: f32,
}

/// Who's winning, reflected so tools can read it (`ponko-mcp`'s
/// `get_resource`, `watch`) and change it. Pairs are `[red, blue]`.
#[derive(Serialize, Deserialize, Default)]
struct Score {
    alive: [usize; 2],
    rounds_won: [u32; 2],
    /// Set when a base falls; the round is over until a restart.
    winner: Option<Team>,
}

impl Battle {
    fn set_rally(&mut self, team: Team, at: Vec2) {
        let start = ponko::perf::Instant::now();
        self.rally[team.index()] = at;
        self.fields[team.index()].compute(&self.nav, &[at]);
        self.field_ms = start.elapsed().as_secs_f32() * 1000.0;
    }
}

/// A unit died: sent by `resolve`, reflected so tools can watch the
/// fighting (`ponko-mcp`'s `subscribe_events` / `take_events`).
#[derive(Serialize)]
struct Fallen {
    team: Team,
    at: Vec2,
}

/// What new units look like: a sprite when there's a window, nothing headless.
struct Look(Option<Texture>);

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

// ---------------------------------------------------------------------- map

/// Rocks and lakes (walls) and mud, in blobs, with a clear spot for each
/// base and a border. The same seed makes the same map.
fn make_map(size: UVec2, seed: u64) -> (NavGrid, [Vec2; 2]) {
    let mut nav = NavGrid::new(size.x, size.y, CELL);
    let mut rng = Rng::new(seed);
    let scale = size.as_vec2() / MAP.as_vec2();
    let area = scale.x * scale.y;
    let mut blob = |nav: &mut NavGrid, cost: u8, radius: f32| {
        let c = vec2(rng.range(0.0, size.x as f32), rng.range(0.0, size.y as f32));
        let r = radius * rng.range(0.6, 1.4);
        for y in (c.y - r).max(0.0) as u32..((c.y + r) as u32).min(size.y) {
            for x in (c.x - r).max(0.0) as u32..((c.x + r) as u32).min(size.x) {
                if vec2(x as f32, y as f32).distance(c) < r {
                    nav.set_cost(x, y, cost);
                }
            }
        }
    };
    for _ in 0..(16.0 * area) as usize {
        blob(&mut nav, MUD, 18.0);
    }
    for _ in 0..(70.0 * area) as usize {
        blob(&mut nav, NavGrid::WALL, 7.0);
    }
    for _ in 0..(6.0 * area) as usize {
        blob(&mut nav, NavGrid::WALL, 16.0);
    }
    let world = size.as_vec2() * CELL;
    let bases = [vec2(0.08, 0.5) * world, vec2(0.92, 0.5) * world];
    for base in bases {
        let c = base / CELL;
        let r = BASE_RADIUS * 3.0 / CELL;
        for y in (c.y - r) as u32..(c.y + r) as u32 {
            for x in (c.x - r) as u32..(c.x + r) as u32 {
                nav.set_cost(x, y, 1);
            }
        }
    }
    for x in 0..size.x {
        nav.set_cost(x, 0, NavGrid::WALL);
        nav.set_cost(x, size.y - 1, NavGrid::WALL);
    }
    for y in 0..size.y {
        nav.set_cost(0, y, NavGrid::WALL);
        nav.set_cost(size.x - 1, y, NavGrid::WALL);
    }
    // If the blobs cut the bases off from each other, dig a road between them.
    let mut flow = FlowField::new();
    flow.compute(&nav, &[bases[1]]);
    if flow.distance_at(bases[0]).is_none() {
        let y = (bases[0].y / CELL) as u32;
        for x in 1..size.x - 1 {
            for dy in 0..3 {
                let c = nav.cost(x, y + dy - 1).unwrap();
                nav.set_cost(x, y + dy - 1, c.max(1));
            }
        }
    }
    (nav, bases)
}

// ------------------------------------------------------------------ systems

/// Every tick, on all cores: each unit finds the nearest enemy it can see
/// and keeps a little room from its friends. With an enemy in reach it
/// attacks; in sight, it charges; otherwise it follows its team's field.
///
/// Most units are marching, not fighting, so they look for enemies only
/// on their turn (every 4th tick); units in a fight look every tick.
/// Crowding is refreshed every other tick.
fn fight() -> impl FnMut(&mut Ctx) {
    // Per team: a fine grid for friends (only the closest few matter) and a
    // coarse one for enemies (seen from afar). Marching armies far from the
    // enemy then cost almost nothing to search, however packed they are.
    let mut near = [
        SpatialGrid::new(PERSONAL_SPACE * 2.0),
        SpatialGrid::new(PERSONAL_SPACE * 2.0),
    ];
    let mut far = [SpatialGrid::new(SIGHT), SpatialGrid::new(SIGHT)];
    let mut positions: Vec<(Entity, Vec2, Team)> = Vec::new();
    move |ctx: &mut Ctx| {
        let (dt, random, look) = (ctx.time.dt(), ctx.random(), ctx.time.turns(4));
        let crowding = ctx.time.sim_tick().is_multiple_of(2);
        ctx.world
            .query::<(Entity, &d2::Transform, &Unit)>()
            .par_collect_into(&mut positions, |(e, t, u)| (e, t.position, u.team));
        // The four grids are independent: build them at the same time.
        let [near_red, near_blue] = &mut near;
        let [far_red, far_blue] = &mut far;
        let all = &positions;
        let of = |team: Team| all.iter().filter(move |p| p.2 == team).map(|&(e, at, _)| (e, at));
        rayon::scope(|s| {
            if crowding {
                s.spawn(|_| near_red.rebuild_from(of(Team::Red)));
                s.spawn(|_| near_blue.rebuild_from(of(Team::Blue)));
            }
            s.spawn(|_| far_red.rebuild_from(of(Team::Red)));
            s.spawn(|_| far_blue.rebuild_from(of(Team::Blue)));
        });
        let over = ctx.world.resource::<Score>().winner.is_some();
        let battle = ctx.world.remove_resource::<Battle>().expect("battle");
        let (near, far, b) = (&near, &far, &battle);
        ctx.world
            .query::<(Entity, &mut d2::Transform, &mut Unit)>()
            .par_for_each(|(e, t, unit)| {
                let mut rng = random.of(e);
                let me = t.position;
                unit.hit = None;
                unit.cooldown -= dt;
                if crowding {
                    let mut push = Vec2::ZERO;
                    near[unit.team.index()].for_each_in_radius(me, PERSONAL_SPACE, |other| {
                        if other.entity != e {
                            let away = me - other.position;
                            push += away / (away.length_squared() + 0.01);
                        }
                    });
                    unit.push = push.clamp_length_max(1.0);
                }
                let mut nearest = None;
                if unit.engaged || look.includes(e) {
                    let enemies = &far[unit.team.enemy().index()];
                    nearest = enemies
                        .nearest(me, REACH, |_| true)
                        .or_else(|| enemies.nearest(me, SIGHT, |_| true))
                        .map(|g| (g.position.distance_squared(me), g.entity, g.position));
                    unit.engaged = nearest.is_some();
                }
                let enemy_base = b.bases[unit.team.enemy().index()];
                let mut want = match nearest {
                    Some((d2, target, _)) if d2 < REACH * REACH => {
                        if unit.cooldown <= 0.0 {
                            unit.cooldown = COOLDOWN * rng.range(0.8, 1.2);
                            unit.hit = Some(Hit::Unit(target));
                        }
                        Vec2::ZERO
                    }
                    Some((_, _, at)) => (at - me).normalize_or_zero() * SPEED,
                    None if me.distance(enemy_base) < BASE_RADIUS + REACH => {
                        if unit.cooldown <= 0.0 && !over {
                            unit.cooldown = COOLDOWN;
                            unit.hit = Some(Hit::Base);
                        }
                        Vec2::ZERO
                    }
                    None => b.fields[unit.team.index()].direction_at(me) * SPEED,
                };
                let slow = b.nav.cell_at(me).and_then(|c| b.nav.cost(c.x, c.y)).unwrap_or(1).max(1) as f32;
                want = want / slow + unit.push * SPEED;
                unit.vel = unit.vel.lerp(want, 0.2) + rng.unit_vec2() * 2.0;
                // Slide along walls instead of walking into them.
                let next = me + unit.vel * dt;
                if b.nav.walkable(next) {
                    t.position = next;
                } else if b.nav.walkable(vec2(next.x, me.y)) {
                    t.position.x = next.x;
                } else if b.nav.walkable(vec2(me.x, next.y)) {
                    t.position.y = next.y;
                }
            });
        ctx.world.insert_resource(battle);
    }
}

/// Applies this tick's hits in order (so the result doesn't depend on
/// thread timing), removes the dead, and checks for a fallen base.
fn resolve() -> impl FnMut(&mut Ctx) {
    let mut hits: Vec<(Team, Hit)> = Vec::new();
    move |ctx: &mut Ctx| {
        hits.clear();
        ctx.world.query::<&Unit>().for_each(|u| {
            if let Some(hit) = u.hit {
                hits.push((u.team, hit));
            }
        });
        let battle = ctx.world.resource::<Battle>();
        let bases = battle.base_entities;
        for &(team, hit) in &hits {
            match hit {
                Hit::Unit(target) => {
                    if let Some(u) = ctx.world.get_mut::<Unit>(target) {
                        u.hp -= DAMAGE;
                    }
                }
                Hit::Base => {
                    if let Some(base) = ctx.world.get_mut::<Base>(bases[team.enemy().index()]) {
                        base.hp = (base.hp - DAMAGE).max(0.0);
                    }
                }
            }
        }
        let mut alive = [0; 2];
        let (cmds, events) = (&mut ctx.commands, &mut ctx.events);
        ctx.world
            .query::<(Entity, &d2::Transform, &Unit)>()
            .for_each(|(e, t, u)| {
                if u.hp <= 0.0 {
                    cmds.despawn(e);
                    events.send(Fallen {
                        team: u.team,
                        at: t.position,
                    });
                } else {
                    alive[u.team.index()] += 1;
                }
            });
        let mut fallen = None;
        ctx.world.query::<&Base>().for_each(|b| {
            if b.hp <= 0.0 {
                fallen = Some(b.team);
            }
        });
        let score = ctx.world.resource_mut::<Score>();
        score.alive = alive;
        if let (Some(team), None) = (fallen, score.winner) {
            score.winner = Some(team.enemy());
            score.rounds_won[team.enemy().index()] += 1;
        }
    }
}

/// Bases send new units (a few per tick, up to the cap) while they stand.
fn reinforce(ctx: &mut Ctx) {
    let mut rng = ctx.random().stream(1);
    let sprite = ctx.world.resource::<Look>().0;
    let cap = ctx.world.resource::<Battle>().cap;
    let (alive, winner) = {
        let s = ctx.world.resource::<Score>();
        (s.alive, s.winner)
    };
    if winner.is_some() {
        return;
    }
    // A full army rebuilds in about 40 seconds.
    let per_tick = cap as f32 / (40.0 * 60.0);
    let mut spawns = Vec::new();
    ctx.world.query::<(&d2::Transform, &mut Base)>().for_each(|(t, base)| {
        let room = cap.saturating_sub(alive[base.team.index()]);
        base.budget = (base.budget + per_tick).min(room as f32);
        let n = base.budget as usize;
        base.budget -= n as f32;
        spawns.push((base.team, t.position, n));
    });
    for (team, at, n) in spawns {
        for _ in 0..n {
            spawn_unit(
                &mut ctx.commands,
                sprite,
                team,
                at + rng.unit_vec2() * rng.range(0.0, BASE_RADIUS),
            );
        }
    }
}

/// Blue's general: attack red's base, or defend when outnumbered 2 to 1.
fn blue_ai(ctx: &mut Ctx) {
    let alive = ctx.world.resource::<Score>().alive;
    let b = ctx.world.resource_mut::<Battle>();
    let goal = if alive[1] * 2 < alive[0] {
        b.bases[1]
    } else {
        b.bases[0]
    };
    if b.rally[1] != goal {
        b.set_rally(Team::Blue, goal);
    }
}

// -------------------------------------------------------------------- setup

fn spawn_unit(cmds: &mut Commands, sprite: Option<Texture>, team: Team, at: Vec2) {
    let unit = Unit {
        team,
        hp: HP,
        vel: Vec2::ZERO,
        cooldown: 0.0,
        engaged: false,
        push: Vec2::ZERO,
        hit: None,
    };
    match sprite {
        Some(tex) => cmds.spawn((d2::Transform::at(at), Sprite::new(tex).with_color(team.color()), unit)),
        None => cmds.spawn((d2::Transform::at(at), unit)),
    }
}

fn setup(ctx: &mut Ctx, units: usize, seed: u64, sprite: Option<Texture>) {
    let scale = (units as f32 / UNITS_PER_MAP).max(1.0).sqrt();
    let size = (MAP.as_vec2() * scale).as_uvec2();
    let (nav, bases) = make_map(size, seed);
    ctx.world.reflect::<Unit>();
    ctx.world.reflect::<Base>();
    let max_hp = 400.0 * (units as f32 / 1000.0).sqrt();
    let base_entities = [Team::Red, Team::Blue].map(|team| {
        ctx.world.spawn((
            d2::Transform::at(bases[team.index()]),
            Base {
                team,
                hp: max_hp,
                max_hp,
                budget: 0.0,
            },
        ))
    });
    let mut battle = Battle {
        nav,
        fields: [FlowField::new(), FlowField::new()],
        rally: bases,
        bases,
        base_entities,
        cap: units / 2,
        field_ms: 0.0,
    };
    battle.set_rally(Team::Red, bases[1]);
    battle.set_rally(Team::Blue, bases[0]);
    ctx.world.insert_resource(battle);
    ctx.world.reflect_resource::<Score>();
    ctx.events.reflect::<Fallen>();
    ctx.world.insert_resource(Score::default());
    ctx.world.insert_resource(Look(sprite));
    start_round(ctx);
}

/// Both armies at full strength around their bases, bases repaired.
fn start_round(ctx: &mut Ctx) {
    let mut rng = ctx.random().stream(ctx.time.sim_tick());
    let cmds = &mut ctx.commands;
    ctx.world.query::<(Entity, &Unit)>().for_each(|(e, _)| cmds.despawn(e));
    ctx.world.query::<&mut Base>().for_each(|b| b.hp = b.max_hp);
    let sprite = ctx.world.resource::<Look>().0;
    ctx.world.resource_mut::<Score>().winner = None;
    let battle = ctx.world.resource_mut::<Battle>();
    let (cap, bases) = (battle.cap, battle.bases);
    let nav = &battle.nav;
    // Spread each army over the walkable ground around its base.
    let mut spots = Vec::with_capacity(cap * 2);
    for team in [Team::Red, Team::Blue] {
        let spread = (cap as f32).sqrt() * 2.2;
        while spots.len() < cap * (team.index() + 1) {
            let p = bases[team.index()] + rng.unit_vec2() * rng.range(0.0, 1.0).sqrt() * spread;
            if nav.walkable(p) {
                spots.push((team, p));
            }
        }
    }
    for (team, at) in spots {
        spawn_unit(&mut ctx.commands, sprite, team, at);
    }
}

/// The systems both the window and headless runs use.
fn battle_systems(game: Game) -> Game {
    game.add(Phase::AfterPhysics, fight().named("fight"))
        .add(Phase::AfterPhysics, resolve().named("resolve"))
        .sim_system(reinforce)
        .add(Phase::AfterPhysics, blue_ai.every(30))
        .add(Phase::AfterPhysics, keep_neighbors_close.every(60))
}

/// Once a second, units are reordered in memory by where they stand, so
/// neighbors on the map are neighbors in memory: grid rebuilds, flow field
/// lookups and searches then mostly hit the cache.
fn keep_neighbors_close(ctx: &mut Ctx) {
    ctx.world
        .sort_by::<d2::Transform>(|t| ponko::spatial::spatial_key(t.position, 64.0));
}

// ------------------------------------------------------------------ playing

fn controls(ctx: &mut Ctx) {
    let home = ctx.world.resource::<Battle>().bases;
    let rally = if ctx.input.mouse_pressed(MouseButton::Left) && !ctx.ui.wants_pointer() {
        Some(ctx.mouse_world())
    } else if ctx.input.key_pressed(Key::A) {
        Some(home[1])
    } else if ctx.input.key_pressed(Key::D) {
        Some(home[0])
    } else {
        None
    };
    if let Some(at) = rally {
        let b = ctx.world.resource_mut::<Battle>();
        if b.nav.walkable(at) {
            b.set_rally(Team::Red, at);
        }
    }
    if ctx.input.key_pressed(Key::R) {
        start_round(ctx);
    }
    if ctx.input.key_pressed(Key::Space) {
        if ctx.time.is_paused() {
            ctx.time.resume()
        } else {
            ctx.time.pause()
        }
    }
    for (key, speed) in [(Key::Num1, 1.0), (Key::Num2, 2.0), (Key::Num3, 4.0)] {
        if ctx.input.key_pressed(key) {
            ctx.time.set_speed(speed);
        }
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

/// The map as one texture, a pixel per cell. Made once at startup.
#[derive(Clone, Copy)]
struct MapArt {
    tex: Texture,
    size: UVec2,
}

/// Startup: paints the map texture from the navigation grid.
fn paint_map(ctx: &mut Ctx) {
    let size = ctx.world.resource::<Battle>().nav.size();
    let mut pixels = vec![0u8; (size.x * size.y * 4) as usize];
    {
        let nav = &ctx.world.resource::<Battle>().nav;
        for y in 0..size.y {
            for x in 0..size.x {
                // A little per-cell noise so the ground doesn't look flat.
                let n = ((x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 9) as u8;
                let px = match nav.cost(x, y).unwrap() {
                    NavGrid::WALL => [52 + n, 56 + n, 66 + n, 255],
                    MUD => [74 + n, 56 + n, 34, 255],
                    _ => [30 + n, 46 + n, 32, 255],
                };
                pixels[((y * size.x + x) * 4) as usize..][..4].copy_from_slice(&px);
            }
        }
    }
    let tex = ctx.gfx.create_texture(size.x, size.y, &pixels);
    ctx.gfx.set_texture_filter(tex, TextureFilter::Nearest);
    ctx.world.insert_resource(MapArt { tex, size });
}

/// Frame system: the map, the bases with their health, and red's rally
/// point. Units are sprites, drawn by the engine.
fn draw_map(ctx: &mut Ctx) {
    let MapArt { tex, size } = *ctx.world.resource::<MapArt>();
    ctx.gfx.clear(Color::BLACK);
    let world = size.as_vec2() * CELL;
    ctx.gfx.draw_ex(
        tex,
        world / 2.0,
        DrawParams::default().with_scale(Vec2::splat(CELL)).with_z(-10.0),
    );
    let rally = ctx.world.resource::<Battle>().rally[0];
    let mut bases = [(Vec2::ZERO, 0.0, Team::Red); 2];
    ctx.world.query::<(&d2::Transform, &Base)>().for_each(|(t, b)| {
        bases[b.team.index()] = (t.position, b.hp / b.max_hp, b.team);
    });
    for (at, health, team) in bases {
        ctx.gfx.draw_circle(at, BASE_RADIUS, team.color().with_alpha(0.35));
        ctx.gfx.draw_circle(at, BASE_RADIUS * 0.4, team.color());
        let bar = Rect::new(at.x - BASE_RADIUS, at.y - BASE_RADIUS - 8.0, BASE_RADIUS * 2.0, 3.0);
        ctx.gfx.draw_rect(bar, Color::rgba(0.0, 0.0, 0.0, 0.6));
        ctx.gfx.draw_rect(
            Rect::new(bar.min.x, bar.min.y, bar.size().x * health, 3.0),
            team.color(),
        );
    }
    ctx.gfx.draw_circle(rally, 5.0, Color::rgba(1.0, 0.9, 0.3, 0.9));
}

fn hud(ctx: &mut Ctx) {
    let field_ms = ctx.world.resource::<Battle>().field_ms;
    let s = ctx.world.resource::<Score>();
    let (alive, won, winner) = (s.alive, s.rounds_won, s.winner);
    let sim_ms: f32 = ctx
        .perf
        .system_timings()
        .iter()
        .filter(|t| t.sim)
        .map(|t| t.avg_ms)
        .sum();
    let speed = if ctx.time.is_paused() {
        "paused".to_string()
    } else {
        format!("{}x", ctx.time.speed())
    };
    let panel = Rect::new(12.0, 12.0, 300.0, 150.0);
    ctx.immediate_ui().panel(panel);
    let at = panel.min + ctx.ui.immediate.style.padding;
    let text = format!(
        "red {} vs blue {}   (rounds {}-{})\nsim {sim_ms:.2} ms/tick, {:.0} FPS, {speed}\nfield {field_ms:.2} ms\nclick: rally  A: attack  D: defend\nR: restart  1-3: speed",
        alive[0],
        alive[1],
        won[0],
        won[1],
        ctx.time.fps()
    );
    ctx.immediate_ui().label(at, &text);
    if let Some(team) = winner {
        let msg = if team == Team::Red {
            "Red wins! R for another round"
        } else {
            "Blue wins. R to try again"
        };
        let center = ctx.window_size() / 2.0;
        let size = ctx.gfx.measure_text(msg, 32.0);
        ctx.gfx.draw_text(msg, center - size / 2.0, 32.0, team.color());
    }
}

fn main() {
    let units = env_number("UNITS", 20_000.0) as usize;
    let seed = env_number("SEED", 1.0) as u64;
    if env_number("HEADLESS", 0.0) != 0.0 {
        let ticks = env_number("TICKS", 600.0) as u64;
        let mut start = ponko::perf::Instant::now();
        let world = battle_systems(Game::new("battle", 1, 1).seed(seed))
            .sim_system(|ctx: &mut Ctx| {
                // Headless rounds restart on their own.
                if ctx.world.resource::<Score>().winner.is_some() {
                    start_round(ctx);
                }
                if ctx.time.sim_tick().is_multiple_of(120) {
                    let b = ctx.world.resource::<Score>();
                    let timings: Vec<String> = ctx
                        .perf
                        .system_timings()
                        .iter()
                        .filter(|t| t.sim && t.avg_ms >= 0.01)
                        .map(|t| format!("{} {:.2} ms", t.name.rsplit("::").next().unwrap_or(t.name), t.avg_ms))
                        .collect();
                    println!(
                        "tick {}: red {} blue {}, rounds {}-{}  [{}]",
                        ctx.time.sim_tick(),
                        b.alive[0],
                        b.alive[1],
                        b.rounds_won[0],
                        b.rounds_won[1],
                        timings.join(", ")
                    );
                }
            })
            // The two-argument form may borrow a local, here the timer.
            .run_headless(ticks, |ctx| {
                setup(ctx, units, seed, None);
                start = ponko::perf::Instant::now();
            });
        let secs = start.elapsed().as_secs_f64();
        let b = world.resource::<Battle>();
        println!(
            "{units} units, {}x{} cells, {ticks} ticks: {:.2} ms/tick ({:.0} ticks/s)",
            b.nav.size().x,
            b.nav.size().y,
            secs * 1e3 / ticks as f64,
            ticks as f64 / secs
        );
        return;
    }

    battle_systems(Game::new("Ponko: battle", 1280, 720).seed(seed))
        .system(controls)
        .system(ponko::camera::camera_controls)
        .system(hud)
        .system(draw_map)
        .setup(move |ctx| {
            let dot = ctx.gfx.create_texture(2, 2, &[255; 2 * 2 * 4]);
            setup(ctx, units, seed, Some(dot));
            let world = ctx.world.resource::<Battle>().nav.size().as_vec2() * CELL;
            ctx.camera.zoom = (ctx.camera.viewport() / world).min_element();
            ctx.camera.look_at(world / 2.0);
            paint_map(ctx);
        })
        .run();
}
