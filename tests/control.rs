//! The control endpoint, end to end: a headless game driven over TCP, the
//! way `ponko-mcp` drives it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

mod common;

use common::TempAsset;
#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
use ponko::prelude::*;

fn registry_file(addr: SocketAddr) -> PathBuf {
    let dir = match std::env::var_os("PONKO_INSTANCES_DIR") {
        Some(d) => PathBuf::from(d),
        None if cfg!(windows) => PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("ponko/instances"),
        None => PathBuf::from(std::env::var_os("HOME").unwrap()).join(".ponko/instances"),
    };
    dir.join(format!("{}-{}.json", std::process::id(), addr.port()))
}

struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    token: String,
    next_id: u64,
}

impl Client {
    fn connect(addr: SocketAddr, token: String) -> Self {
        let writer = TcpStream::connect(addr).unwrap();
        writer.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let reader = BufReader::new(writer.try_clone().unwrap());
        Self {
            reader,
            writer,
            token,
            next_id: 0,
        }
    }

    fn raw(&mut self, msg: ponko::reflect::json::Value) -> ponko::reflect::json::Value {
        writeln!(self.writer, "{msg}").unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn call(
        &mut self,
        method: &str,
        params: ponko::reflect::json::Value,
    ) -> Result<ponko::reflect::json::Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let reply = self.raw(json!({ "id": id, "token": self.token, "method": method, "params": params }));
        assert_eq!(reply["id"], id);
        match reply.get("error") {
            Some(e) => Err(e.as_str().unwrap().to_string()),
            None => Ok(reply["result"].clone()),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Component)]
struct Counter(u32);

fn count(ctx: &mut Ctx) {
    ctx.world.query::<&mut Counter>().for_each(|c| c.0 += 1);
}

#[test]
fn a_paused_headless_game_is_driven_over_tcp() {
    let (addr_tx, addr_rx) = mpsc::channel();
    let game = thread::spawn(move || {
        Game::new("control-test", 1, 1)
            .control_endpoint(true)
            .sim_system(count)
            .run_headless(u64::MAX, move |ctx| {
                ctx.world.reflect::<Counter>();
                ctx.time.pause();
                addr_tx.send(ctx.control_addr().unwrap()).unwrap();
            })
    });
    let addr = addr_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(addr.ip().is_loopback());

    // The registry file says who we are and holds the token.
    let file = registry_file(addr);
    let info: ponko::reflect::json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(info["name"], "control-test");
    assert_eq!(info["port"], addr.port());
    assert_eq!(info["headless"], true);
    let mut c = Client::connect(addr, info["token"].as_str().unwrap().to_string());

    // Wrong token: refused, nothing happens, and the stranger is hung up on.
    let reply = c.raw(json!({ "id": 7, "token": "nope", "method": "game.quit" }));
    assert!(reply["error"].as_str().unwrap().contains("token"), "{reply}");
    let mut rest = String::new();
    assert!(c.reader.read_line(&mut rest).map_or(true, |n| n == 0), "{rest}");
    let mut c = Client::connect(addr, info["token"].as_str().unwrap().to_string());

    let status = c.call("info", json!({})).unwrap();
    assert_eq!(
        (status["paused"].clone(), status["tick"].clone()),
        (json!(true), json!(0))
    );

    let e = c
        .call("entity.spawn", json!({ "components": { "Counter": 0 } }))
        .unwrap()["entity"]
        .as_u64()
        .unwrap();

    // Paused: only the requested ticks run.
    c.call("sim.step", json!({ "ticks": 5 })).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.call("info", json!({})).unwrap()["tick"] != 5 {
        assert!(Instant::now() < deadline, "ticks never ran");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        c.call("entity.get", json!({ "entity": e })).unwrap(),
        json!({ "Counter": 5 })
    );
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        c.call("info", json!({})).unwrap()["tick"],
        5,
        "no ticks beyond the steps"
    );

    let summary = c.call("world.summary", json!({})).unwrap();
    assert_eq!(summary["entities"], 1);
    assert!(
        summary["reflected_components"]
            .as_array()
            .unwrap()
            .contains(&json!("Counter"))
    );
    assert!(summary["allocated_bytes"].as_u64().unwrap() > 0, "memory is reported");
    assert!(
        summary["archetypes"][0]["bytes_per_entity"].as_u64().unwrap() >= 8,
        "at least the entity handle"
    );

    let diag = c.call("ecs.diagnostics", json!({ "with": ["Counter"] })).unwrap();
    assert_eq!(diag["matching"]["entities"], 1);
    let systems = diag["systems"].as_array().unwrap();
    let counter = systems
        .iter()
        .find(|s| s["name"].as_str().unwrap().ends_with("::count"))
        .unwrap();
    assert_eq!(
        (
            counter["phase"].clone(),
            counter["sim"].clone(),
            counter["access"].clone()
        ),
        (json!("AfterPhysics"), json!(true), json!("exclusive"))
    );
    // The schedule: `count` has no declaration, so it is a stage of its own.
    assert_eq!(counter["alone"]["reason"], "undeclared");
    assert!(counter["declared"].is_null());
    let stage = &diag["schedule"]["stages"][counter["stage"].as_u64().unwrap() as usize];
    assert_eq!(
        (stage["first"].clone(), stage["len"].clone()),
        (counter["order"].clone(), json!(1))
    );
    assert_eq!(diag["schedule"]["dependencies_total"], 0);
    assert!(diag["query_cache"]["enabled"].as_bool().unwrap());

    let info = c.call("info", json!({})).unwrap();
    assert!(info["render"].is_null(), "headless games draw nothing");
    let cam = c
        .call("camera.set", json!({ "zoom": 2.0, "look_at": [100.0, 50.0] }))
        .unwrap();
    assert_eq!(cam["zoom"], 2.0);
    assert_eq!(cam["center"], json!([100.0, 50.0]));
    assert_eq!(c.call("camera.get", json!({})).unwrap(), cam);
    assert!(
        c.call("camera.set", json!({ "zoom": -1 }))
            .unwrap_err()
            .contains("positive")
    );

    let err = c.call("screenshot", json!({})).unwrap_err();
    assert!(err.contains("no window"), "{err}");

    let err = c.call("entity.get", json!({ "entity": 999_999 })).unwrap_err();
    assert!(err.contains("does not exist"), "{err}");
    assert!(
        c.call("no.such.method", json!({}))
            .unwrap_err()
            .contains("unknown method")
    );

    c.call("game.quit", json!(null)).unwrap();
    let mut world = game.join().unwrap();
    assert_eq!(world.query::<&Counter>().iter().next().unwrap().0, 5);
    assert!(!file.exists(), "registry file is removed on exit");
}

#[test]
fn two_instances_get_their_own_endpoints() {
    let start = || {
        let (tx, rx) = mpsc::channel();
        let h = thread::spawn(move || {
            Game::new("twin", 1, 1)
                .control_endpoint(true)
                .run_headless(u64::MAX, move |ctx| {
                    ctx.time.pause();
                    tx.send(ctx.control_addr().unwrap()).unwrap();
                })
        });
        (rx.recv_timeout(Duration::from_secs(10)).unwrap(), h)
    };
    let (a, ha) = start();
    let (b, hb) = start();
    assert_ne!(a.port(), b.port());
    for (addr, h) in [(a, ha), (b, hb)] {
        let info: ponko::reflect::json::Value =
            serde_json::from_str(&std::fs::read_to_string(registry_file(addr)).unwrap()).unwrap();
        let mut c = Client::connect(addr, info["token"].as_str().unwrap().into());
        c.call("game.quit", json!({})).unwrap();
        h.join().unwrap();
    }
}

#[test]
fn a_paused_game_reports_its_capabilities_before_any_tick() {
    let (tx, rx) = mpsc::channel();
    let game = thread::spawn(move || {
        let game = Game::new("caps", 1, 1).control_endpoint(true).sim_system(count);
        #[cfg(feature = "physics")]
        let game = game.physics(vec2(0.0, 0.0));
        game.run_headless(u64::MAX, move |ctx| {
            ctx.world.reflect::<Counter>();
            ctx.time.pause();
            tx.send(ctx.control_addr().unwrap()).unwrap();
        })
    });
    let addr = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let info: ponko::reflect::json::Value =
        serde_json::from_str(&std::fs::read_to_string(registry_file(addr)).unwrap()).unwrap();
    let mut c = Client::connect(addr, info["token"].as_str().unwrap().into());

    let caps = c.call("capabilities", json!({})).unwrap();
    assert_eq!(caps["mode"], "headless");
    assert_eq!(caps["engine_version"], info["engine_version"]);
    let physics = &caps["subsystems"]["physics"];
    if cfg!(feature = "physics") {
        assert_eq!(physics["active"], true, "the step is scheduled: {physics}");
    } else {
        assert_eq!(
            (physics["compiled"].clone(), physics["active"].clone()),
            (json!(false), json!(false))
        );
    }
    let methods = caps["methods"].as_array().unwrap();
    let names: Vec<_> = methods.iter().map(|m| m["name"].clone()).collect();
    assert_eq!(
        names,
        c.call("methods", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].clone())
            .collect::<Vec<_>>()
    );
    let counter = caps["reflection"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Counter")
        .unwrap();
    assert_eq!(counter["schema"]["status"], "opaque");
    assert_eq!(c.call("info", json!({})).unwrap()["tick"], 0, "asking ran nothing");
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn the_endpoint_is_off_by_default() {
    let mut addr = Some("127.0.0.1:1".parse().unwrap());
    Game::new("t", 1, 1).run_headless(1, |ctx| addr = ctx.control_addr());
    assert_eq!(addr, None);
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Score {
    red: u32,
    blue: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Weather(String);

#[test]
fn reflected_resources_are_read_and_written_over_tcp() {
    let (addr_tx, addr_rx) = mpsc::channel();
    let game = thread::spawn(move || {
        Game::new("resources-test", 1, 1)
            .control_endpoint(true)
            .sim_system(|ctx: &mut Ctx| ctx.world.resource_mut::<Score>().red += 1)
            .run_headless(u64::MAX, move |ctx| {
                ctx.world.reflect_resource::<Score>();
                ctx.world.reflect_resource::<Weather>();
                ctx.world.insert_resource(Score { red: 0, blue: 0 });
                ctx.time.pause();
                addr_tx.send(ctx.control_addr().unwrap()).unwrap();
            })
    });
    let addr = addr_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let info: ponko::reflect::json::Value =
        serde_json::from_str(&std::fs::read_to_string(registry_file(addr)).unwrap()).unwrap();
    let mut c = Client::connect(addr, info["token"].as_str().unwrap().to_string());

    assert_eq!(
        c.call("resources.list", json!({})).unwrap(),
        json!({ "Score": { "red": 0, "blue": 0 }, "Weather": null }),
        "missing resources are listed as null"
    );
    let summary = c.call("world.summary", json!({})).unwrap();
    assert_eq!(summary["reflected_resources"], json!(["Score", "Weather"]));

    // Partial objects merge; the game sees the change on its next tick.
    let score = c
        .call("resource.set", json!({ "resource": "Score", "value": { "blue": 7 } }))
        .unwrap();
    assert_eq!(score, json!({ "red": 0, "blue": 7 }));
    c.call("sim.step", json!({ "ticks": 3 })).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.call("info", json!({})).unwrap()["tick"] != 3 {
        assert!(Instant::now() < deadline, "ticks never ran");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        c.call("resource.get", json!({ "resource": "Score" })).unwrap(),
        json!({ "red": 3, "blue": 7 })
    );

    // A missing resource is inserted.
    c.call("resource.set", json!({ "resource": "Weather", "value": "rain" }))
        .unwrap();
    assert_eq!(
        c.call("resource.get", json!({ "resource": "Weather" })).unwrap(),
        "rain"
    );

    let err = c.call("resource.get", json!({ "resource": "Gold" })).unwrap_err();
    assert!(err.contains("no reflected resource named `Gold`"), "{err}");
    let err = c
        .call(
            "resource.set",
            json!({ "resource": "Score", "value": { "red": "lots" } }),
        )
        .unwrap_err();
    assert!(err.contains("bad value for `Score`"), "{err}");

    c.call("game.quit", json!(null)).unwrap();
    let world = game.join().unwrap();
    assert_eq!(world.resource::<Score>().blue, 7);
    assert_eq!(world.resource::<Weather>().0, "rain");
}

#[derive(serde::Serialize)]
struct Ping {
    tick: u64,
}

fn wait_for_tick(c: &mut Client, tick: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.call("info", json!({})).unwrap()["tick"] != tick {
        assert!(Instant::now() < deadline, "tick {tick} never came");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn subscribers_get_reflected_events_with_their_ticks() {
    let (addr_tx, addr_rx) = mpsc::channel();
    let game = thread::spawn(move || {
        Game::new("events-test", 1, 1)
            .control_endpoint(true)
            .sim_system(|ctx| {
                let tick = ctx.time.sim_tick();
                ctx.events.send(Ping { tick });
            })
            .run_headless(u64::MAX, move |ctx| {
                ctx.events.reflect::<Ping>();
                ctx.time.pause();
                addr_tx.send(ctx.control_addr().unwrap()).unwrap();
            })
    });
    let addr = addr_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let info: ponko::reflect::json::Value =
        serde_json::from_str(&std::fs::read_to_string(registry_file(addr)).unwrap()).unwrap();
    let mut c = Client::connect(addr, info["token"].as_str().unwrap().to_string());

    assert!(c.call("events.take", json!({})).unwrap_err().contains("subscribe"));
    let list = c.call("events.list", json!({})).unwrap();
    assert_eq!(list, json!([{ "name": "Ping", "kept": 0 }]));

    c.call("events.subscribe", json!({ "types": ["Ping"] })).unwrap();
    c.call("sim.step", json!({ "ticks": 5 })).unwrap();
    wait_for_tick(&mut c, 5);
    let got = c.call("events.take", json!({})).unwrap();
    let ticks: Vec<u64> = got["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tick"].as_u64().unwrap())
        .collect();
    assert_eq!(ticks, [0, 1, 2, 3, 4]);
    assert_eq!(
        got["events"][2],
        json!({ "tick": 2, "type": "Ping", "data": { "tick": 2 } })
    );
    assert_eq!(got["dropped"], 0);
    assert_eq!(c.call("events.take", json!({})).unwrap()["events"], json!([]));

    // A small buffer keeps the newest and counts the rest.
    c.call("events.subscribe", json!({ "capacity": 2 })).unwrap();
    c.call("sim.step", json!({ "ticks": 5 })).unwrap();
    wait_for_tick(&mut c, 10);
    let got = c.call("events.take", json!({})).unwrap();
    let ticks: Vec<u64> = got["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tick"].as_u64().unwrap())
        .collect();
    assert_eq!((ticks, got["dropped"].clone()), (vec![8, 9], json!(3)));

    assert_eq!(c.call("events.unsubscribe", json!({})).unwrap(), json!(true));
    assert!(c.call("events.take", json!({})).is_err());
    let summary = c.call("world.summary", json!({})).unwrap();
    assert_eq!(summary["reflected_events"][0]["name"], "Ping");
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

/// Lines this test printed. Other tests share the process, and the test
/// harness's progress dots (no newline) can end up in front of ours.
fn ours(logs: &ponko::reflect::json::Value) -> Vec<String> {
    let lines = logs["lines"].as_array().unwrap().iter().filter_map(|l| l.as_str());
    lines
        .filter_map(|l| l.find("logs-test").map(|i| l[i..].to_string()))
        .collect()
}

#[test]
fn the_game_output_is_kept_for_tools() {
    let (addr_tx, addr_rx) = mpsc::channel();
    let game = thread::spawn(move || {
        Game::new("logs-test", 1, 1)
            .control_endpoint(true)
            .sim_system(|ctx: &mut Ctx| {
                // Straight to the file descriptors, past the test harness's capture.
                writeln!(std::io::stdout(), "logs-test tick {}", ctx.time.sim_tick()).unwrap();
            })
            .run_headless(u64::MAX, move |ctx| {
                ctx.time.pause();
                writeln!(std::io::stderr(), "logs-test setup on stderr").unwrap();
                addr_tx.send(ctx.control_addr().unwrap()).unwrap();
            })
    });
    let addr = addr_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let info: ponko::reflect::json::Value =
        serde_json::from_str(&std::fs::read_to_string(registry_file(addr)).unwrap()).unwrap();
    let mut c = Client::connect(addr, info["token"].as_str().unwrap().to_string());

    let first = c.call("logs.get", json!({ "lines": 5000 })).unwrap();
    assert_eq!(ours(&first), ["logs-test setup on stderr"], "{first}");

    // Only what's new since the last read.
    c.call("sim.step", json!({ "ticks": 3 })).unwrap();
    let mut since = first["next"].as_u64().unwrap();
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.len() < 3 {
        assert!(Instant::now() < deadline, "saw only {seen:?}");
        let new = c.call("logs.get", json!({ "since": since })).unwrap();
        since = new["next"].as_u64().unwrap();
        seen.extend(ours(&new));
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(seen, ["logs-test tick 0", "logs-test tick 1", "logs-test tick 2"]);
    assert!(c.call("methods", json!({})).unwrap().to_string().contains("logs.get"));
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

/// With `CHILD_EXITS=1` this test is the child: a game with the endpoint on
/// prints more than a pipe holds and calls `std::process::exit` mid-tick,
/// skipping every destructor; the engine's exit hook passes the rest on.
#[test]
fn output_printed_right_before_exit_still_reaches_the_terminal() {
    if std::env::var_os("CHILD_EXITS").is_some() {
        Game::new("exit-test", 1, 1)
            .control_endpoint(true)
            .sim_system(|_: &mut Ctx| {
                for i in 0..20_000 {
                    writeln!(std::io::stdout(), "exit-test line {i}").unwrap();
                }
                std::process::exit(0);
            })
            .run_headless(1, |_| {});
        return;
    }
    // The parent stops reading near the end, so the last lines are still in
    // the engine's pipe (and its copy thread) when the child exits.
    let total: usize = (0..20_000).map(|i| format!("exit-test line {i}\n").len()).sum();
    let me = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(me)
        .args([
            "--exact",
            "output_printed_right_before_exit_still_reaches_the_terminal",
            "--nocapture",
        ])
        .env("CHILD_EXITS", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let mut paused = false;
    loop {
        let n = stdout.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if !paused && data.len() + 6_000 >= total {
            // Less than the pipes in between hold, so the child gets to its
            // exit while this stands still (but under the engine's wait).
            paused = true;
            thread::sleep(Duration::from_millis(250));
        }
    }
    let status = child.wait().unwrap();
    let text = String::from_utf8_lossy(&data);
    assert!(status.success(), "{text}");
    assert!(text.contains("exit-test line 19999"), "lost the last lines: {text}");
}

#[test]
fn scenes_and_prefabs_are_saved_and_loaded_over_tcp() {
    let prefab = TempAsset::new("control-prefab.json");
    let scene = TempAsset::new("control-scene.json");
    let huge_scene = TempAsset::new("control-huge-id.json");
    let names = [
        prefab.name().to_string(),
        scene.name().to_string(),
        huge_scene.name().to_string(),
    ];
    let (addr_tx, addr_rx) = mpsc::channel();
    let game = thread::spawn(move || {
        Game::new("scenes-test", 1, 1)
            .control_endpoint(true)
            .run_headless(u64::MAX, move |ctx| {
                ctx.time.pause();
                addr_tx.send(ctx.control_addr().unwrap()).unwrap();
            })
    });
    let addr = addr_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let info: ponko::reflect::json::Value =
        serde_json::from_str(&std::fs::read_to_string(registry_file(addr)).unwrap()).unwrap();
    let mut c = Client::connect(addr, info["token"].as_str().unwrap().to_string());

    // An agent builds a level: a crab sprite saved as a prefab, placed twice.
    let crab = json!({ "Transform2D": { "rotation": 0.5 }, "Sprite": { "texture": "crab.png", "z": 2 } });
    let model = c.call("entity.spawn", json!({ "components": crab })).unwrap()["entity"].clone();
    c.call("prefab.save", json!({ "entity": model, "path": names[0] }))
        .unwrap();
    c.call("entity.despawn", json!({ "entity": model })).unwrap();
    let a = c
        .call("prefab.spawn", json!({ "path": names[0], "at": [10, 20] }))
        .unwrap()["entity"]
        .clone();
    c.call("prefab.spawn", json!({ "path": names[0], "at": [30, 20] }))
        .unwrap();
    let placed = c.call("entity.get", json!({ "entity": a })).unwrap();
    assert_eq!(placed["Transform2D"]["position"], json!([10.0, 20.0]));
    assert_eq!(placed["Sprite"]["texture"], "crab.png");

    c.call("scene.save", json!({ "path": names[1] })).unwrap();
    c.call("entity.despawn", json!({ "entity": a })).unwrap();
    let loaded = c.call("scene.load", json!({ "path": names[1] })).unwrap();
    assert_eq!(loaded["entities"], 2);
    assert!(c.call("entity.get", json!({ "entity": a })).is_ok(), "same id back");

    // A scene with a huge entity id is refused and changes nothing.
    let mut saved: ponko::reflect::json::Value =
        serde_json::from_str(&std::fs::read_to_string(scene.path()).unwrap()).unwrap();
    saved["entities"][1]["entity"] = json!(50_000_000u64);
    std::fs::write(huge_scene.path(), saved.to_string()).unwrap();
    let huge = c.call("scene.load", json!({ "path": names[2] })).unwrap_err();
    assert!(huge.contains("may only use indices below"), "{huge}");
    let summary = c.call("world.summary", json!({})).unwrap();
    assert_eq!(summary["entities"], 2);
    assert!(c.call("entity.get", json!({ "entity": a })).is_ok());
    let missing = c.call("prefab.spawn", json!({ "path": "control-missing.json", "at": [0, 0] }));
    assert!(missing.unwrap_err().contains("not found"));

    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

// ------------------------------------------------------- input scenarios

/// Whether Space is held, as the last tick's sim system saw it.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SpaceHeld(bool);

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Jumps(u32);

fn watch_space(ctx: &mut Ctx) {
    let held = ctx.input.key_down(Key::Space);
    ctx.world.resource_mut::<SpaceHeld>().0 = held;
    if ctx.input.action_pressed("jump") {
        ctx.world.resource_mut::<Jumps>().0 += 1;
    }
}

/// A paused headless game with a "jump" action on Space; returns a client.
fn scenario_game(injection: bool) -> (Client, thread::JoinHandle<World>) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let game = thread::spawn(move || {
        Game::new("scenario-control", 1, 1)
            .control_endpoint(true)
            .input_injection(injection)
            .sim_system(watch_space)
            .run_headless(u64::MAX, move |ctx| {
                ctx.world.reflect_resource::<SpaceHeld>();
                ctx.world.reflect_resource::<Jumps>();
                ctx.world.insert_resource(SpaceHeld::default());
                ctx.world.insert_resource(Jumps::default());
                let mut map = ActionMap::default();
                map.set("jump", Action::Button(vec![ButtonBinding::Key(Key::Space)]));
                ctx.input.set_actions(map);
                ctx.time.pause();
                addr_tx.send(ctx.control_addr().unwrap()).unwrap();
            })
    });
    let addr = addr_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    (connect(addr), game)
}

fn connect(addr: SocketAddr) -> Client {
    let info: ponko::reflect::json::Value =
        serde_json::from_str(&std::fs::read_to_string(registry_file(addr)).unwrap()).unwrap();
    Client::connect(addr, info["token"].as_str().unwrap().to_string())
}

/// Steps the paused game `ticks` ticks and waits for them.
fn step(c: &mut Client, ticks: u64) {
    let target = c.call("info", json!({})).unwrap()["tick"].as_u64().unwrap() + ticks;
    c.call("sim.step", json!({ "ticks": ticks })).unwrap();
    wait_for_tick(c, target);
}

fn scenario_status(c: &mut Client) -> ponko::reflect::json::Value {
    c.call("input.scenario.status", json!({})).unwrap()
}

#[test]
fn tools_cannot_inject_input_unless_the_game_allows_it() {
    let (mut c, game) = scenario_game(false);
    let scenario = json!({ "version": 1, "steps": [{ "tick": 0, "type": "key", "key": "Space", "down": true }] });
    let refused = c
        .call("input.scenario.load", json!({ "scenario": scenario }))
        .unwrap_err();
    assert!(refused.contains("input injection is off"), "{refused}");
    // The refusal is reported before a tool tries, and nothing was installed.
    let caps = c.call("capabilities", json!({})).unwrap();
    let method = caps["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "input.scenario.load")
        .unwrap();
    assert_eq!(method["available"], false, "{method}");
    assert_eq!(scenario_status(&mut c), ponko::reflect::json::Value::Null);
    step(&mut c, 2);
    assert_eq!(
        c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap(),
        json!(false)
    );
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn a_scenario_started_by_a_tool_drives_the_game_and_reports_a_failed_check() {
    let (mut c, game) = scenario_game(true);
    let scenario = json!({
        "version": 1,
        "id": "tool-run",
        "steps": [
            { "tick": 1, "type": "key", "key": "Space", "down": true },
            { "tick": 1, "type": "expect_resource", "resource": "SpaceHeld", "equals": true },
            { "tick": 2, "type": "key", "key": "Space", "down": false },
            { "tick": 3, "type": "expect_resource", "resource": "Jumps", "pointer": "", "equals": 5 }
        ]
    });
    let loaded = c.call("input.scenario.load", json!({ "scenario": scenario })).unwrap();
    assert_eq!(loaded["report"]["status"], "pending");
    assert_eq!(loaded["bound_to_connection"], false);
    step(&mut c, 2);
    let status = scenario_status(&mut c);
    assert_eq!(
        (status["status"].clone(), status["ticks_run"].clone()),
        (json!("running"), json!(2))
    );
    assert_eq!(
        c.call("resource.get", json!({ "resource": "Jumps" })).unwrap(),
        json!(1)
    );
    step(&mut c, 2);
    let status = scenario_status(&mut c);
    assert_eq!(status["status"], "failed", "{status}");
    let failure = &status["failure"];
    assert_eq!((failure["step"].clone(), failure["tick"].clone()), (json!(3), json!(3)));
    assert_eq!(failure["expected"], 5);
    assert_eq!(failure["actual"], 1);
    assert_eq!(status["ticks_run"], 4, "the run ended on the failing tick");

    // A scenario loaded mid-run counts its ticks from the next tick.
    let again = json!({ "version": 1, "steps": [{ "tick": 0, "type": "key", "key": "Space", "down": true }] });
    let loaded = c.call("input.scenario.load", json!({ "scenario": again })).unwrap();
    assert_eq!(loaded["report"]["start_tick"], 4);
    step(&mut c, 1);
    assert_eq!(
        c.call("resource.get", json!({ "resource": "Jumps" })).unwrap(),
        json!(2)
    );

    // Malformed files are refused with the reason, and leave the run alone.
    let bad = json!({ "version": 1, "steps": [{ "tick": 0, "type": "key", "key": "Spacee", "down": true }] });
    let error = c.call("input.scenario.load", json!({ "scenario": bad })).unwrap_err();
    assert!(error.contains("Spacee"), "{error}");
    assert_eq!(scenario_status(&mut c)["status"], "passed");
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn cancelling_a_tool_started_scenario_releases_its_input() {
    let (mut c, game) = scenario_game(true);
    let scenario = json!({
        "version": 1,
        "duration": 100,
        "steps": [{ "tick": 0, "type": "key", "key": "Space", "down": true }]
    });
    c.call("input.scenario.load", json!({ "scenario": scenario })).unwrap();
    step(&mut c, 2);
    assert_eq!(
        c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap(),
        json!(true)
    );
    let cancelled = c
        .call("input.scenario.cancel", json!({ "reason": "test is done" }))
        .unwrap();
    assert_eq!(cancelled["cancelled"], true);
    assert_eq!(cancelled["report"]["status"], "cancelled");
    assert_eq!(cancelled["report"]["cancel_reason"], "test is done");
    assert_eq!(cancelled["report"]["released"], 1);
    step(&mut c, 1);
    assert_eq!(
        c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap(),
        json!(false)
    );
    let again = c.call("input.scenario.cancel", json!({})).unwrap();
    assert_eq!(again["cancelled"], false, "nothing left to cancel");
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn a_scenario_bound_to_its_connection_is_cancelled_when_the_client_disconnects() {
    let (mut c, game) = scenario_game(true);
    let addr = c.writer.peer_addr().unwrap();
    let scenario = json!({
        "version": 1,
        "duration": 100,
        "steps": [{ "tick": 0, "type": "key", "key": "Space", "down": true }]
    });
    // A scenario not bound to a connection survives its client.
    {
        let mut short_lived = connect(addr);
        short_lived
            .call("input.scenario.load", json!({ "scenario": scenario }))
            .unwrap();
    }
    step(&mut c, 1);
    assert_eq!(scenario_status(&mut c)["status"], "running");

    // A bound one does not.
    {
        let mut owner = connect(addr);
        let loaded = owner
            .call(
                "input.scenario.load",
                json!({ "scenario": scenario, "bind_connection": true }),
            )
            .unwrap();
        assert_eq!(loaded["bound_to_connection"], true);
        step(&mut c, 1);
        assert_eq!(
            c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap(),
            json!(true)
        );
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while scenario_status(&mut c)["status"] != "cancelled" {
        assert!(Instant::now() < deadline, "the disconnect never cancelled the scenario");
        thread::sleep(Duration::from_millis(5));
    }
    let status = scenario_status(&mut c);
    assert_eq!(status["released"], 1);
    assert!(
        status["cancel_reason"].as_str().unwrap().contains("disconnected"),
        "{status}"
    );
    step(&mut c, 1);
    assert_eq!(
        c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap(),
        json!(false)
    );
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn a_paused_game_does_not_advance_a_scenario_and_resuming_continues_it() {
    let (mut c, game) = scenario_game(true);
    let scenario = json!({
        "version": 1,
        "duration": 8,
        "release_when_done": true,
        "steps": [
            { "tick": 1, "type": "key", "key": "Space", "down": true },
            { "tick": 6, "type": "expect_resource", "resource": "SpaceHeld", "equals": true }
        ]
    });
    c.call("input.scenario.load", json!({ "scenario": scenario })).unwrap();
    step(&mut c, 3);
    // Paused for a while: the scenario stays where it was.
    thread::sleep(Duration::from_millis(100));
    let status = scenario_status(&mut c);
    assert_eq!(
        (status["status"].clone(), status["ticks_run"].clone()),
        (json!("running"), json!(3))
    );
    assert_eq!(
        c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap(),
        json!(true)
    );
    // Resumed: it runs to its end by itself and releases its input there.
    c.call("sim.resume", json!({})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while scenario_status(&mut c)["status"] == "running" {
        assert!(Instant::now() < deadline, "the scenario never finished");
        thread::sleep(Duration::from_millis(5));
    }
    let status = scenario_status(&mut c);
    assert_eq!(
        (
            status["status"].clone(),
            status["ticks_run"].clone(),
            status["released"].clone()
        ),
        (json!("passed"), json!(8), json!(1))
    );
    c.call("sim.pause", json!({})).unwrap();
    step(&mut c, 1);
    assert_eq!(
        c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap(),
        json!(false)
    );
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

fn space_held(c: &mut Client) -> bool {
    c.call("resource.get", json!({ "resource": "SpaceHeld" })).unwrap() == json!(true)
}

fn hold_space(extra: ponko::reflect::json::Value) -> ponko::reflect::json::Value {
    let mut params = json!({
        "scenario": {
            "version": 1,
            "duration": 3,
            "steps": [{ "tick": 0, "type": "key", "key": "Space", "down": true }]
        }
    });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    params
}

#[test]
fn a_finished_tool_scenario_leaves_nothing_held() {
    let (mut c, game) = scenario_game(true);
    c.call("input.scenario.load", hold_space(json!({}))).unwrap();
    step(&mut c, 3);
    let status = scenario_status(&mut c);
    assert_eq!(
        (
            status["status"].clone(),
            status["released"].clone(),
            status["still_held"].clone()
        ),
        (json!("passed"), json!(1), json!(0))
    );
    step(&mut c, 1);
    assert!(!space_held(&mut c), "released when the run ended");
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn input_kept_by_a_finished_tool_scenario_is_released_by_a_cancel() {
    let (mut c, game) = scenario_game(true);
    c.call(
        "input.scenario.load",
        hold_space(json!({ "keep_input_when_done": true })),
    )
    .unwrap();
    step(&mut c, 4);
    let status = scenario_status(&mut c);
    assert_eq!(
        (status["status"].clone(), status["still_held"].clone()),
        (json!("passed"), json!(1))
    );
    assert!(space_held(&mut c), "kept on purpose");
    let cancelled = c
        .call("input.scenario.cancel", json!({ "reason": "done looking" }))
        .unwrap();
    assert_eq!(cancelled["cancelled"], true);
    assert_eq!(
        (
            cancelled["report"]["released"].clone(),
            cancelled["report"]["still_held"].clone()
        ),
        (json!(1), json!(0))
    );
    step(&mut c, 1);
    assert!(!space_held(&mut c));
    assert_eq!(
        c.call("input.scenario.cancel", json!({})).unwrap()["cancelled"],
        false,
        "nothing left"
    );
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn input_kept_by_a_finished_bound_scenario_is_released_when_its_client_disconnects() {
    let (mut c, game) = scenario_game(true);
    let addr = c.writer.peer_addr().unwrap();
    {
        let mut owner = connect(addr);
        owner
            .call(
                "input.scenario.load",
                hold_space(json!({ "keep_input_when_done": true, "bind_connection": true })),
            )
            .unwrap();
        step(&mut c, 4);
        assert!(space_held(&mut c));
        assert_eq!(scenario_status(&mut c)["still_held"], 1);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while scenario_status(&mut c)["still_held"] != 0 {
        assert!(
            Instant::now() < deadline,
            "the disconnect never released the kept input"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        scenario_status(&mut c)["cancel_reason"]
            .as_str()
            .unwrap()
            .contains("disconnected")
    );
    step(&mut c, 1);
    assert!(!space_held(&mut c));
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

#[test]
fn a_refused_scenario_leaves_the_running_one_alone() {
    let (mut c, game) = scenario_game(true);
    c.call(
        "input.scenario.load",
        hold_space(json!({ "keep_input_when_done": true })),
    )
    .unwrap();
    step(&mut c, 1);
    let too_long = c
        .call("input.scenario.load", hold_space(json!({ "max_ticks": 2 })))
        .unwrap_err();
    assert!(too_long.contains("more than `max_ticks` (2)"), "{too_long}");
    let empty = c
        .call(
            "input.scenario.load",
            json!({ "scenario": { "version": 1, "steps": [] } }),
        )
        .unwrap_err();
    assert!(empty.contains("no steps"), "{empty}");
    let status = scenario_status(&mut c);
    assert_eq!(
        (status["status"].clone(), status["ticks_run"].clone()),
        (json!("running"), json!(1))
    );
    assert!(space_held(&mut c), "its input is still held");
    c.call("sim.resume", json!({})).unwrap();
    let not_paused = c
        .call("input.scenario.load", hold_space(json!({ "queue_ticks": true })))
        .unwrap_err();
    assert!(not_paused.contains("paused"), "{not_paused}");
    assert_ne!(scenario_status(&mut c)["status"], "cancelled");
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}

/// `quality.set` over the socket on a headless game: what `capabilities`
/// says about it matches what the endpoint does, and with the `3d` feature
/// the new settings show in `perf.get`'s render inspector.
#[test]
fn quality_set_over_tcp_matches_what_capabilities_reports() {
    let (tx, rx) = mpsc::channel();
    let game = thread::spawn(move || {
        Game::new("quality", 1, 1)
            .control_endpoint(true)
            .run_headless(u64::MAX, move |ctx| {
                ctx.time.pause();
                tx.send(ctx.control_addr().unwrap()).unwrap();
            })
    });
    let addr = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut c = connect(addr);
    let caps = c.call("capabilities", json!({})).unwrap();
    let entry = caps["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "quality.set")
        .cloned()
        .expect("quality.set is listed");
    let set = c.call("quality.set", json!({ "level": "low" }));
    if cfg!(feature = "3d") {
        assert_eq!(entry["available"], true, "{entry}");
        assert!(entry["limited"].as_str().unwrap().contains("headless"), "{entry}");
        let set = set.unwrap();
        assert_eq!(
            (set["requested"].clone(), set["quality"]["level"].clone()),
            (json!("low"), json!("low"))
        );
        assert!(set["previous"]["level"].is_string(), "{set}");
        let perf = c.call("perf.get", json!({ "history": 5 })).unwrap();
        let inspector = &perf["render_inspector"];
        assert_eq!(inspector["quality"], set["quality"], "perf.get shows the new settings");
        assert_eq!(inspector["history"], json!([]), "a headless game draws no frames");
        assert!(inspector["adapter"].is_null() && inspector["unavailable"]["adapter"].is_string());
        // No GPU to recommend for: refused, and nothing changes.
        let err = c.call("quality.set", json!({ "level": "recommended" })).unwrap_err();
        assert!(err.contains("GPU"), "{err}");
        let perf = c.call("perf.get", json!({})).unwrap();
        assert_eq!(perf["render_inspector"]["quality"]["level"], "low");
    } else {
        assert_eq!(entry["available"], false, "{entry}");
        let reason = entry["reason"].as_str().unwrap();
        assert!(reason.contains("`3d` feature"), "{entry}");
        assert_eq!(set.unwrap_err(), reason, "the refusal is the reported reason");
    }
    c.call("game.quit", json!({})).unwrap();
    game.join().unwrap();
}
