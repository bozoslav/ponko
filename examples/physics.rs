//! Physics: crates, balls and rocks fall, stack, bounce and slide down a
//! ramp. Left click drops a crate, right click a ball, middle click a rock
//! (a polygon), Space blasts everything near the mouse away,
//! and the bell rings (turns red) when something lands in the bucket.
//!
//!   `cargo run --release --example physics --features physics`
//!
//! Keys: R starts over, Escape quits.

#[allow(unused_imports)]
use ponko::d2::*;
#[allow(unused_imports)]
#[cfg(feature = "physics")]
use ponko::prelude::*;

const W: f32 = 1000.0;
const H: f32 = 700.0;

/// Marks the sensor in the bucket.
#[derive(Component)]
struct Bell {
    rang: f32,
}

// ANCHOR: bodies
fn build_level(ctx: &mut Ctx) {
    // Fixed bodies never move: the ground and two walls.
    for (at, size) in [
        (vec2(W / 2.0, H - 10.0), vec2(W, 20.0)),
        (vec2(10.0, H / 2.0), vec2(20.0, H)),
        (vec2(W - 10.0, H / 2.0), vec2(20.0, H)),
    ] {
        ctx.world
            .spawn((d2::Transform::at(at), RigidBody::fixed(), Collider::rect(size)));
    }
    // A bouncy ball and a crate, both dynamic: gravity and collisions move them.
    ctx.world.spawn((
        d2::Transform::at(vec2(300.0, 100.0)),
        RigidBody::dynamic(),
        Collider::circle(16.0).restitution(0.8),
    ));
    ctx.world.spawn((
        d2::Transform::at(vec2(320.0, 0.0)),
        RigidBody::dynamic(),
        Collider::rect(vec2(40.0, 40.0)).friction(0.9),
    ));
    // A ramp: any convex shape of up to 8 corners, around its position.
    ctx.world.spawn((
        d2::Transform::at(vec2(150.0, H - 80.0)),
        RigidBody::fixed(),
        Collider::polygon(&[vec2(-140.0, 60.0), vec2(140.0, 60.0), vec2(-140.0, -60.0)]),
    ));
    // A sensor reports overlaps but doesn't push.
    ctx.world.spawn((
        d2::Transform::at(vec2(800.0, H - 60.0)),
        RigidBody::fixed(),
        Collider::rect(vec2(120.0, 80.0)).sensor(),
        Bell { rang: 0.0 },
    ));
}
// ANCHOR_END: bodies

// ANCHOR: collisions
fn ring_the_bell(ctx: &mut Ctx) {
    let mut rang = false;
    for c in ctx.events.read::<CollisionStarted>() {
        rang |= c.sensor && [c.a, c.b].iter().any(|&e| ctx.world.has::<Bell>(e));
    }
    let dt = ctx.time.dt();
    for bell in ctx.world.query::<&mut Bell>().iter() {
        bell.rang = if rang { 1.0 } else { (bell.rang - dt * 2.0).max(0.0) };
    }
}
// ANCHOR_END: collisions

// ANCHOR: queries
fn blast(ctx: &mut Ctx) {
    if !ctx.input.key_pressed(Key::Space) {
        return;
    }
    let center = ctx.mouse_world();
    let mut near = Vec::new();
    let with_sensors = ponko::d2::QueryFilter::default().with_sensors();
    ctx.physics.overlap_circle(center, 200.0, with_sensors, &mut near);
    for e in near {
        let Some(t) = ctx.world.get::<d2::Transform>(e) else {
            continue;
        };
        let away = (t.position - center).normalize_or(Vec2::NEG_Y);
        if let Some(mass) = ctx.physics.mass(e) {
            ctx.physics.apply_impulse(e, away * mass * 600.0);
        }
    }
}

/// What's under the mouse, straight down: a laser from the cursor.
fn laser(ctx: &mut Ctx) -> Option<Hit> {
    ctx.physics.cast_ray(
        ctx.mouse_world(),
        vec2(0.0, 1.0),
        H,
        ponko::d2::QueryFilter::default().with_sensors(),
    )
}
// ANCHOR_END: queries

fn drop_things(ctx: &mut Ctx) {
    let at = ctx.mouse_world();
    if ctx.input.mouse_pressed(MouseButton::Left) {
        ctx.world.spawn((
            d2::Transform::at(at),
            RigidBody::dynamic(),
            Collider::rect(vec2(30.0, 30.0)).friction(0.8),
        ));
    }
    if ctx.input.mouse_pressed(MouseButton::Right) {
        ctx.world.spawn((
            d2::Transform::at(at),
            RigidBody::dynamic(),
            Collider::circle(12.0).restitution(0.6),
        ));
    }
    if ctx.input.mouse_pressed(MouseButton::Middle) {
        let rock: Vec<Vec2> = (0..5)
            .map(|i| Vec2::from_angle(i as f32 * std::f32::consts::TAU / 5.0) * 18.0)
            .collect();
        ctx.world.spawn((
            d2::Transform::at(at),
            RigidBody::dynamic(),
            Collider::polygon(&rock).friction(0.7),
        ));
    }
    if ctx.input.key_pressed(Key::R) {
        let all: Vec<Entity> = ctx.world.query::<Entity>().filter::<With<RigidBody>>().iter().collect();
        for e in all {
            ctx.world.despawn(e);
        }
        build_level(ctx);
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

fn draw(ctx: &mut Ctx) {
    ctx.gfx.clear(Color::hex(0x1d2330));
    let shapes: Vec<(d2::Transform, Collider, bool, f32)> = ctx
        .world
        .query::<(&d2::Transform, &Collider, &RigidBody, Option<&Bell>)>()
        .iter()
        .map(|(t, c, b, bell)| (*t, *c, b.kind == BodyKind::Fixed, bell.map_or(0.0, |b| b.rang)))
        .collect();
    for (t, c, fixed, rang) in shapes {
        let color = if c.sensor {
            Color::rgba(1.0, 0.3 + 0.7 * (1.0 - rang), 0.3, 0.35)
        } else if fixed {
            Color::hex(0x4a5568)
        } else {
            Color::hex(0xf6ad55)
        };
        match c.shape {
            Shape::Circle { radius } => {
                ctx.gfx.draw_circle(t.position, radius, color);
                // A spoke shows the spin.
                let spoke = t.position + Vec2::from_angle(t.rotation) * radius;
                ctx.gfx.draw_line(t.position, spoke, 2.0, Color::hex(0x1d2330));
            }
            Shape::Rect { size } => {
                // Four corners turned with the body, drawn as edges.
                let (sin, cos) = t.rotation.sin_cos();
                let turn = |v: Vec2| t.position + vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos);
                let h = size / 2.0;
                let corners = [vec2(-h.x, -h.y), vec2(h.x, -h.y), vec2(h.x, h.y), vec2(-h.x, h.y)].map(turn);
                if fixed || c.sensor {
                    ctx.gfx
                        .draw_rect(Rect::new(t.position.x - h.x, t.position.y - h.y, size.x, size.y), color);
                } else {
                    for i in 0..4 {
                        ctx.gfx.draw_line(corners[i], corners[(i + 1) % 4], 3.0, color);
                    }
                }
            }
            Shape::Capsule { height, radius } => {
                let up = Vec2::from_angle(t.rotation + std::f32::consts::FRAC_PI_2) * height / 2.0;
                ctx.gfx.draw_line(t.position - up, t.position + up, radius * 2.0, color);
            }
            Shape::Polygon { .. } => {
                let (sin, cos) = t.rotation.sin_cos();
                let turn = |v: Vec2| t.position + vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos);
                let corners: Vec<Vec2> = c.shape.points().iter().map(|&p| turn(p + c.offset)).collect();
                for i in 0..corners.len() {
                    ctx.gfx
                        .draw_line(corners[i], corners[(i + 1) % corners.len()], 3.0, color);
                }
            }
            _ => {} // shapes added in later versions
        }
    }
    if let Some(hit) = laser(ctx) {
        ctx.gfx
            .draw_line(ctx.mouse_world(), hit.point, 1.0, Color::rgba(1.0, 0.2, 0.2, 0.8));
        ctx.gfx.draw_circle(hit.point, 4.0, Color::rgb(1.0, 0.2, 0.2));
    }
    let bodies = ctx.physics.body_count();
    ctx.gfx.draw_text(
        &format!(
            "{bodies} bodies  |  left click: crate  right click: ball  middle click: rock  space: blast  R: reset"
        ),
        vec2(30.0, 30.0),
        18.0,
        Color::WHITE,
    );
}

fn main() {
    // ANCHOR: setup
    Game::new("Physics", W as u32, H as u32)
        .physics(vec2(0.0, 980.0)) // pixels/s², y is down
        .sim_system(ring_the_bell)
        // ANCHOR_END: setup
        .system(drop_things)
        .system(blast)
        .system(draw)
        .setup(build_level)
        .run();
}
