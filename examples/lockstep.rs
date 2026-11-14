//! Lockstep multiplayer: two players each move a crab with the arrow keys,
//! and both windows show the same game, because both run the same
//! simulation from the same inputs. Run it twice:
//!
//!   `cargo run --example lockstep -- host 7777`
//!   `cargo run --example lockstep -- join 127.0.0.1:7777`
//!
//! (or on two machines on one network). Space shoves the other crab away.
//!
//! Over Steam (feature `steam`; Steam running; Valve's test app 480):
//!
//!   `cargo run --example lockstep --features steam -- steam`       host, press I to invite
//!   `cargo run --example lockstep --features steam -- steam-join`  then accept the invite

#[allow(unused_imports)]
use ponko::d2::*;
#[allow(unused_imports)]
use ponko::net::{Net, NetState};
use ponko::prelude::*;
use serde::{Deserialize, Serialize};

// ANCHOR: input
/// What one player does in one tick: all that crosses the network.
#[derive(Serialize, Deserialize, Default)]
struct Pad {
    dir: Vec2,
    shove: bool,
}

/// Reads this player's keys every frame; the session sends them.
fn read_keys(ctx: &mut Ctx) {
    let mut pad = Pad::default();
    for (key, d) in [
        (Key::Left, -Vec2::X),
        (Key::Right, Vec2::X),
        (Key::Up, -Vec2::Y),
        (Key::Down, Vec2::Y),
    ] {
        if ctx.input.key_down(key) {
            pad.dir += d;
        }
    }
    pad.shove = ctx.input.key_down(Key::Space);
    ctx.net.set_input(&pad);
}
// ANCHOR_END: input

/// A crab, and whose it is.
#[derive(Serialize, Deserialize, Clone, Copy, Component)]
struct Crab {
    player: u32,
    push: Vec2,
}

// ANCHOR: simulate
/// The simulation: every peer runs it on every player's input, in the
/// same order, so every peer ends up with the same world.
fn move_crabs(ctx: &mut Ctx) {
    let pads: Vec<Pad> = (0..ctx.net.players())
        .map(|p| ctx.net.input::<Pad>(p).unwrap_or_default())
        .collect();
    let dt = ctx.time.dt();
    let crabs: Vec<(Entity, Vec2, u32)> = ctx
        .world
        .query::<(Entity, &d2::Transform, &Crab)>()
        .iter()
        .map(|(e, t, c)| (e, t.position, c.player))
        .collect();
    for &(e, at, player) in &crabs {
        let pad = &pads[player as usize];
        if pad.shove {
            for &(other, other_at, _) in &crabs {
                if other != e && at.distance(other_at) < 120.0 {
                    ctx.world.get_mut::<Crab>(other).unwrap().push += (other_at - at).normalize_or_zero() * 900.0;
                }
            }
        }
        let crab = *ctx.world.get::<Crab>(e).unwrap();
        let t = ctx.world.get_mut::<d2::Transform>(e).unwrap();
        t.position += (pad.dir.normalize_or_zero() * 250.0 + crab.push) * dt;
        t.position = t.position.clamp(vec2(40.0, 40.0), vec2(760.0, 560.0));
        ctx.world.get_mut::<Crab>(e).unwrap().push = crab.push * 0.9;
    }
}
// ANCHOR_END: simulate

fn draw(ctx: &mut Ctx) {
    ctx.gfx.clear(Color::hex(0x20304a));
    let crabs: Vec<(Vec2, u32)> = ctx
        .world
        .query::<(&d2::Transform, &Crab)>()
        .iter()
        .map(|(t, c)| (t.position, c.player))
        .collect();
    for (at, player) in crabs {
        let color = [Color::hex(0xf56565), Color::hex(0x63b3ed)][player as usize % 2];
        ctx.gfx.draw_circle(at, 30.0, color);
    }
    let net = &ctx.net;
    let status = match net.state() {
        NetState::Connecting => "waiting for the other player...".to_string(),
        NetState::Running => format!(
            "you are player {} ({}), tick {}{}",
            net.player() + 1,
            ["red", "blue"][net.player() % 2],
            ctx.time.sim_tick(),
            net.desync().map_or(String::new(), |d| format!("  DESYNC: {d}"))
        ),
        NetState::Lost => format!("connection lost: {}", net.error().unwrap_or("?")),
        NetState::Offline => "offline".into(),
        _ => String::new(),
    };
    ctx.gfx.draw_text(&status, vec2(20.0, 20.0), 20.0, Color::WHITE);
    #[cfg(feature = "steam")]
    if ctx.net.state() == NetState::Connecting
        && let Some(lobby) = ctx.net.steam_lobby()
    {
        let text = format!("Steam lobby {}: press I to invite a friend", lobby.raw());
        ctx.gfx.draw_text(&text, vec2(20.0, 50.0), 20.0, Color::WHITE);
        if ctx.input.key_pressed(Key::I) {
            ctx.net.steam_invite();
        }
    }
    if ctx.input.key_pressed(Key::Escape) {
        ctx.quit();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(feature = "steam")]
    if let Some(net) = steam_session(&args) {
        return play(net);
    }
    // ANCHOR: session
    let net = match args.first().map(String::as_str) {
        Some("host") => Net::host(format!("0.0.0.0:{}", args.get(1).map_or("7777", |p| p)), 2),
        Some("join") => Net::join(args.get(1).map_or("127.0.0.1:7777", |a| a)),
        _ => {
            eprintln!("usage: lockstep host [port] | join <host:port> | steam | steam-join");
            return;
        }
    }
    .expect("could not open a socket");
    play(net);
}

fn play(net: Net) {
    Game::new("Lockstep", 800, 600)
        .lockstep(net)
        .system(read_keys) // frame system: input out
        .sim_system(move_crabs) // sim system: everyone's input in
        .system(draw)
        .setup(|ctx| {
            ctx.world.reflect::<Crab>(); // compared between peers
            for player in 0..2 {
                ctx.world.spawn((
                    d2::Transform::at(vec2(250.0 + 300.0 * player as f32, 300.0)),
                    Crab {
                        player,
                        push: Vec2::ZERO,
                    },
                ));
            }
        })
        .run();
    // ANCHOR_END: session
}

// ANCHOR: steam
/// Steam: the host creates a friends-only lobby and invites from the
/// overlay; the friend accepts in Steam. No addresses, no port forwarding.
#[cfg(feature = "steam")]
fn steam_session(args: &[String]) -> Option<Net> {
    let mode = args.first().map(String::as_str);
    let invited = ponko::net::steam_lobby_from_args(); // launched by accepting an invite
    if !matches!(mode, Some("steam" | "steam-join")) && invited.is_none() {
        return None;
    }
    // 480 is Valve's test app; a real game uses its own app id.
    let client = steamworks::Client::init_app(480).expect("Steam must be running and logged in");
    if let Some(lobby) = invited {
        return Some(Net::steam_join(&client, lobby));
    }
    if mode == Some("steam") {
        return Some(Net::steam_host(&client, 2));
    }
    println!("waiting for you to accept a Steam invite...");
    let lobby = ponko::net::steam_wait_for_invite(&client, std::time::Duration::from_secs(300))?;
    Some(Net::steam_join(&client, lobby))
}
// ANCHOR_END: steam
