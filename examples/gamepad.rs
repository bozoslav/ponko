//! Gamepads: every connected controller gets a crab. Left stick moves it,
//! the bottom face button (A, Cross) spins it, the right trigger grows it.
//! Plug pads in and out while it runs. Without a pad, the arrow keys move
//! a crab of their own.
//!
//!   `cargo run --example gamepad`

#[allow(unused_imports)]
use ponko::d2::*;
#[allow(unused_imports)]
use ponko::input::{GamepadAxis, GamepadButton, GamepadStick};
use ponko::prelude::*;

/// The crab of the pad in `slot`, or of the keyboard (`slot: None`).
#[derive(Component)]
struct Player {
    slot: Option<usize>,
    spin: f32,
}

const SPEED: f32 = 400.0;

/// A tint per player, so pads can tell their crabs apart.
const PLAYER_COLORS: [Color; ponko::input::MAX_GAMEPADS] = [
    Color::rgb(1.0, 0.6, 0.6),
    Color::rgb(0.6, 0.8, 1.0),
    Color::rgb(0.6, 1.0, 0.6),
    Color::rgb(1.0, 1.0, 0.5),
    Color::rgb(1.0, 0.6, 1.0),
    Color::rgb(0.6, 1.0, 1.0),
    Color::rgb(1.0, 0.8, 0.5),
    Color::rgb(0.8, 0.8, 0.8),
];

// ANCHOR: gamepad
fn move_players(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    // Read the pads first: the query below borrows the world.
    let mut sticks = [Vec2::ZERO; ponko::input::MAX_GAMEPADS];
    let mut spins = [false; ponko::input::MAX_GAMEPADS];
    let mut grow = [0.0; ponko::input::MAX_GAMEPADS];
    for &pad in ctx.input.gamepads() {
        sticks[pad.slot()] = ctx.input.gamepad_stick(pad, GamepadStick::Left);
        spins[pad.slot()] = ctx.input.gamepad_pressed(pad, GamepadButton::South);
        grow[pad.slot()] = ctx.input.gamepad_axis(pad, GamepadAxis::RightTrigger);
    }
    for (t, player) in ctx.world.query::<(&mut d2::Transform, &mut Player)>().iter() {
        let Some(slot) = player.slot else { continue };
        t.position += sticks[slot] * SPEED * dt;
        if spins[slot] {
            player.spin = std::f32::consts::TAU;
        }
        t.scale = Vec2::splat(1.0 + 2.0 * grow[slot]);
    }
}
// ANCHOR_END: gamepad

/// Arrow keys move the keyboard's crab.
fn move_keyboard_player(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    let mut dir = Vec2::ZERO;
    for (key, d) in [
        (Key::Left, -Vec2::X),
        (Key::Right, Vec2::X),
        (Key::Up, -Vec2::Y),
        (Key::Down, Vec2::Y),
    ] {
        if ctx.input.key_down(key) {
            dir += d;
        }
    }
    let spin = ctx.input.key_pressed(Key::Space);
    for (t, player) in ctx.world.query::<(&mut d2::Transform, &mut Player)>().iter() {
        if player.slot.is_none() {
            t.position += dir.normalize_or_zero() * SPEED * dt;
            if spin {
                player.spin = std::f32::consts::TAU;
            }
        }
    }
}

/// One crab per connected pad: spawn on plug-in, despawn on unplug.
fn join_and_leave(ctx: &mut Ctx) {
    let mut connected = [false; ponko::input::MAX_GAMEPADS];
    for pad in ctx.input.gamepads() {
        connected[pad.slot()] = true;
    }
    let mut has_crab = [false; ponko::input::MAX_GAMEPADS];
    for (e, player) in ctx.world.query::<(Entity, &Player)>().iter() {
        if let Some(slot) = player.slot {
            if connected[slot] {
                has_crab[slot] = true;
            } else {
                ctx.commands.despawn(e);
            }
        }
    }
    for slot in (0..ponko::input::MAX_GAMEPADS).filter(|&s| connected[s] && !has_crab[s]) {
        let mut sprite = Sprite::new(ctx.gfx.load_texture("crab.png"));
        sprite.color = PLAYER_COLORS[slot];
        let at = vec2(200.0 + slot as f32 * 120.0, 360.0);
        ctx.world.spawn((
            d2::Transform::at(at),
            sprite,
            Player {
                slot: Some(slot),
                spin: 0.0,
            },
        ));
    }
}

/// Spins play out over half a second.
fn spin(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    for (t, player) in ctx.world.query::<(&mut d2::Transform, &mut Player)>().iter() {
        let step = player.spin.min(4.0 * std::f32::consts::TAU * dt);
        t.rotation += step;
        player.spin -= step;
    }
}

fn hud(ctx: &mut Ctx) {
    let pads = ctx.input.gamepads().len();
    let text = match pads {
        0 => "No gamepad: plug one in, or use the arrow keys and Space".to_string(),
        n => format!("{n} gamepad(s): left stick moves, A/Cross spins, right trigger grows"),
    };
    ctx.gfx.draw_text(&text, vec2(20.0, 20.0), 20.0, Color::WHITE);
}

fn main() {
    Game::new("Ponko: gamepads", 1280, 720)
        .system(join_and_leave)
        .system(move_players)
        .system(move_keyboard_player)
        .system(spin)
        .system(hud)
        .setup(|ctx| {
            let crab = ctx.gfx.load_texture("crab.png");
            ctx.world.spawn((
                d2::Transform::at(vec2(640.0, 560.0)),
                Sprite::new(crab),
                Player { slot: None, spin: 0.0 },
            ));
        })
        .run();
}
