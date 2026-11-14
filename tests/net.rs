//! Lockstep networking: peers fed the same inputs end with the same world.
use std::thread;
use std::time::Duration;

use ponko::access::{Service, SystemAccess};
#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
#[allow(unused_imports)]
use ponko::net::{Desync, Net, NetState};
use ponko::prelude::*;
use ponko::system_view::SystemView;
use serde::{Deserialize, Serialize};

/// What a player does in a tick.
#[derive(Serialize, Deserialize, Clone, Copy)]
struct Steer {
    dir: Vec2,
}

/// A crowd member; `owner` steers it.
#[derive(Serialize, Deserialize, Clone, Copy, Component)]
struct Unit {
    owner: u32,
    vel: Vec2,
}

/// What a run ended with, read in its last tick.
#[derive(Clone)]
struct Outcome {
    desync: Option<Desync>,
    player: usize,
    steered: bool,
}

/// Inputs vary with tick and player, as if someone were playing.
fn play_along(ctx: &mut Ctx) {
    let t = ctx.time.sim_tick() as f32 * 0.05 + ctx.net.player() as f32;
    ctx.net.set_input(&Steer {
        dir: vec2(t.cos(), t.sin()),
    });
}

/// Every player steers its units; units also wander with seeded randomness
/// and push apart through the spatial grid, so any difference in inputs or
/// order spreads through the whole crowd.
fn crowd(ctx: &mut Ctx) {
    let mut steer = [Vec2::ZERO; ponko::net::MAX_PLAYERS];
    for (p, s) in steer.iter_mut().enumerate().take(ctx.net.players()) {
        if let Some(Steer { dir }) = ctx.net.input::<Steer>(p) {
            *s = dir;
        }
    }
    if steer[1] != Vec2::ZERO {
        ctx.world.resource_mut::<Steered>().0 = true;
    }
    let random = ctx.random();
    let dt = ctx.time.dt();
    let grid = &ctx.spatial;
    let mut moves = Vec::new();
    for (e, t, u) in ctx.world.query::<(Entity, &Transform2D, &Unit)>().iter() {
        let mut push = Vec2::ZERO;
        grid.for_each_in_radius(t.position, 16.0, |other| {
            if other.entity != e {
                push += (t.position - other.position).normalize_or_zero();
            }
        });
        let jitter = random.of(e).unit_vec2() * 5.0;
        let vel = u.vel * 0.9 + steer[u.owner as usize] * 20.0 + push * 10.0 + jitter;
        moves.push((e, vel, t.position + vel * dt));
    }
    for (e, vel, at) in moves {
        ctx.world.get_mut::<Unit>(e).unwrap().vel = vel;
        ctx.world.get_mut::<Transform2D>(e).unwrap().position = at;
    }
}

struct Steered(bool);

/// For the view systems: how long a unit has lived, and how tired it is.
#[derive(Serialize, Deserialize, Clone, Copy, Component)]
struct Age(u32);
#[derive(Serialize, Deserialize, Clone, Copy, Component)]
struct Stamina(f32);

/// View systems over different components: with the parallel scheduler on
/// they share a batch.
fn grow_older(sys: &mut SystemView) {
    sys.world.query::<&mut Age>().for_each(|a| a.0 += 1);
}

fn tire(sys: &mut SystemView) {
    let random = sys.random();
    sys.world
        .query::<(Entity, &mut Stamina, &Unit)>()
        .for_each(|(e, s, u)| {
            s.0 = s.0 * 0.99 + u.vel.length() * 0.01 + random.of(e).range(0.0, 0.1);
        });
}

fn spawn_crowd(ctx: &mut Ctx, units: usize, players: usize) {
    ctx.world.reflect::<Unit>();
    ctx.world.insert_resource(Steered(false));
    let mut rng = ctx.random().stream(7);
    for i in 0..units {
        ctx.world.spawn((
            Transform2D::at(vec2(rng.range(0.0, 400.0), rng.range(0.0, 400.0))),
            Unit {
                owner: (i % players) as u32,
                vel: Vec2::ZERO,
            },
        ));
    }
}

/// Runs each session in its own thread for `ticks`; `tweak(player)` is an
/// extra sim system, to break determinism on purpose.
fn play(nets: Vec<Net>, ticks: u64, units: usize, tweak: fn(&mut Ctx)) -> Vec<(ponko::reflect::json::Value, Outcome)> {
    play_with(nets, ticks, units, tweak, false)
}

/// `play`; with `views`, also the two view systems, and the peers schedule
/// them differently: the host as the environment says (off, unless
/// `PONKO_PARALLEL_SYSTEMS` turns the scheduler on), the others with the
/// scheduler off whatever it says.
fn play_with(
    nets: Vec<Net>,
    ticks: u64,
    units: usize,
    tweak: fn(&mut Ctx),
    views: bool,
) -> Vec<(ponko::reflect::json::Value, Outcome)> {
    let players = nets.len();
    let runs: Vec<_> = nets
        .into_iter()
        .enumerate()
        .map(|(peer, net)| {
            thread::spawn(move || {
                let mut game = Game::new("net", 1, 1).spatial_grid(16.0).lockstep(net);
                if views {
                    let random = || SystemAccess::new().read_service(Service::Random);
                    game = game
                        .add(
                            Phase::BeforePhysics,
                            grow_older.access(SystemAccess::new().write::<Age>()),
                        )
                        .add(
                            Phase::BeforePhysics,
                            tire.access(random().write::<Stamina>().read::<Unit>()),
                        );
                    // The two view systems share a stage, so a peer with the
                    // scheduler on really runs them as a batch.
                    let plan = game.schedule_plan();
                    let batch = plan.batches().next().expect("one batch");
                    assert_eq!((plan.batches().count(), batch.len), (1, 2), "{plan}");
                    if peer > 0 {
                        game = game.parallel_systems(false).parallel_systems_env(false);
                    }
                }
                let mut world = game
                    .sim_system(play_along)
                    .sim_system(crowd)
                    .sim_system(tweak)
                    .sim_system(move |ctx: &mut Ctx| {
                        if ctx.time.sim_tick() + 1 == ticks {
                            let outcome = Outcome {
                                desync: ctx.net.desync().cloned(),
                                player: ctx.net.player(),
                                steered: ctx.world.resource::<Steered>().0,
                            };
                            ctx.world.insert_resource(outcome);
                        }
                    })
                    .run_headless(ticks, |ctx| {
                        spawn_crowd(ctx, units, players);
                        if views {
                            ctx.world.reflect::<Age>();
                            ctx.world.reflect::<Stamina>();
                            let all: Vec<Entity> = ctx.world.query::<Entity>().filter::<With<Unit>>().iter().collect();
                            for e in all {
                                ctx.world.insert(e, Age(0));
                                ctx.world.insert(e, Stamina(1.0));
                            }
                        }
                    });
                let outcome = world.remove_resource::<Outcome>().expect("ran every tick");
                (world.save(), outcome)
            })
        })
        .collect();
    runs.into_iter().map(|r| r.join().unwrap()).collect()
}

fn nothing(_: &mut Ctx) {}

fn assert_identical(results: &[(ponko::reflect::json::Value, Outcome)]) {
    // The host is player 0; guests are numbered in the order their hellos
    // arrive, which packet loss can shuffle.
    let mut players: Vec<usize> = results.iter().map(|(_, o)| o.player).collect();
    assert_eq!(players[0], 0, "the host is player 0");
    players.sort_unstable();
    assert_eq!(
        players,
        (0..results.len()).collect::<Vec<_>>(),
        "every player has its own number"
    );
    for (i, (save, outcome)) in results.iter().enumerate() {
        assert!(outcome.desync.is_none(), "no desync: {:?}", outcome.desync);
        assert!(outcome.steered, "player 1's input arrived");
        assert_eq!(*save, results[0].0, "player {i} has the same world");
    }
}

// ANCHOR: loopback
#[test]
fn two_peers_run_ten_thousand_ticks_bit_identical() {
    // `play` runs each session in its own thread and returns every world.
    assert_identical(&play(Net::loopback(2), 10_000, 200, nothing));
}
// ANCHOR_END: loopback

#[test]
fn peers_that_schedule_their_view_systems_differently_stay_bit_identical() {
    // The same crowd plus two view systems. CI runs this file once more
    // with `PONKO_PARALLEL_SYSTEMS=always`: then the host runs them as a
    // batch on the thread pool while the guest runs them one at a time.
    let results = play_with(Net::loopback(2), 600, 100, nothing, true);
    assert_identical(&results);
    let units = results[0].0["entities"].as_array().unwrap();
    assert!(
        units.iter().any(|e| e.to_string().contains("Stamina")),
        "the view systems' data is in the save"
    );
}

#[test]
fn lost_packets_are_recovered() {
    // One packet in three never arrives.
    assert_identical(&play(Net::loopback_lossy(3, 3), 1000, 150, nothing));
}

#[test]
fn peers_over_udp_stay_in_sync() {
    let host = Net::host("127.0.0.1:0", 2).unwrap().input_delay(2);
    let addr = host.local_addr().unwrap();
    let guest = Net::join(addr).unwrap();
    assert_identical(&play(vec![host, guest], 600, 100, nothing));
}

/// Player 1 alone nudges one unit: a bug that only shows on one machine.
fn diverge(ctx: &mut Ctx) {
    if ctx.net.player() == 1 && ctx.time.sim_tick() == 100 {
        let e = ctx
            .world
            .query::<Entity>()
            .filter::<With<Unit>>()
            .iter()
            .next()
            .unwrap();
        ctx.world.get_mut::<Unit>(e).unwrap().vel.x += 0.001;
    }
}

#[test]
fn a_desync_is_detected_and_named() {
    let results = play(Net::loopback(2), 400, 50, diverge);
    for (_, outcome) in &results {
        let d = outcome.desync.as_ref().expect("both sides notice");
        assert_eq!(d.tick, 119, "the first check after the change (every 60 ticks)");
        assert!(d.components.contains(&"Unit".to_string()), "{d}");
    }
    assert_eq!(results[0].1.desync.as_ref().unwrap().player, 1);
    assert_eq!(results[1].1.desync.as_ref().unwrap().player, 0);
}

#[test]
fn a_different_seed_is_refused() {
    let mut nets = Net::loopback(2);
    let guest = nets.pop().unwrap().timeout(Duration::from_secs(2));
    let host = nets.pop().unwrap().timeout(Duration::from_secs(2));
    let host = thread::spawn(move || {
        let world = Game::new("h", 1, 1).lockstep(host).run_headless(10, |_| {});
        world.len()
    });
    let mut state = None;
    Game::new("g", 1, 1).seed(99).lockstep(guest).run_headless(10, |ctx| {
        state = Some(ctx.net.state());
    });
    assert_eq!(state, Some(NetState::Connecting));
    assert_eq!(host.join().unwrap(), 0, "the host gave up waiting too");
}

#[test]
fn offline_input_applies_on_the_next_tick() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = seen.clone();
    Game::new("t", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            log.borrow_mut().push(ctx.net.input::<u64>(0));
            ctx.net.set_input(&ctx.time.sim_tick());
        })
        .run_headless(3, |ctx| {
            assert_eq!((ctx.net.player(), ctx.net.players()), (0, 1));
            assert_eq!(ctx.net.state(), NetState::Offline);
        });
    assert_eq!(*seen.borrow(), [None, Some(0), Some(1)]);
}
