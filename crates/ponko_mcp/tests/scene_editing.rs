//! An agent assembles a 3D level through the MCP server's editor tools,
//! saves it and loads it back: the session recorded in
//! `tests/fixtures/scene_editing/session.json`, replayed against a fresh
//! headless game with each reply checked.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ponko::d3::{Attached, Transform};
use ponko::prelude::*;
use ponko_mcp::{Config, Server};
use serde_json::{Value, json};

/// Whether `got` has every key of `want` with the same value (objects
/// recursively; anything else exactly).
fn matches(want: &Value, got: &Value) -> bool {
    match (want, got) {
        (Value::Object(w), Value::Object(g)) => w.iter().all(|(k, v)| g.get(k).is_some_and(|gv| matches(v, gv))),
        _ => want == got,
    }
}

/// A game folder with a mesh and a 3D prefab (a post with a light-sized
/// part attached on top) in `assets/`.
fn project(dir: &Path) {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    std::fs::create_dir_all(dir.join("assets/models")).unwrap();
    std::fs::create_dir_all(dir.join("assets/prefabs")).unwrap();
    std::fs::copy(engine.join("models/house.glb"), dir.join("assets/models/house.glb")).unwrap();
    let prefab = json!({
        "format": "ponko-prefab",
        "version": 2,
        "entities": [
            { "entity": 0, "components": {
                "Transform3D": { "scale": [0.2, 1.0, 0.2] },
                "Model": { "mesh": "models/house.glb" } } },
            { "entity": 1, "components": {
                "Transform3D": { "position": [0.0, 2.0, 0.0] },
                "Attached3D": { "to": 0, "offset": { "position": [0.0, 2.0, 0.0] } } } }
        ]
    });
    std::fs::write(dir.join("assets/prefabs/lamp_post.json"), prefab.to_string()).unwrap();
}

fn start_game() -> (u16, thread::JoinHandle<World>) {
    let (tx, rx) = mpsc::channel();
    let h = thread::spawn(move || {
        Game::new("yard", 800, 600)
            .control_endpoint(true)
            .sim_system(systems::follow_attachments)
            .run_headless(u64::MAX, move |ctx| {
                ctx.time.pause();
                tx.send(ctx.control_addr().unwrap().port()).unwrap();
            })
    });
    (rx.recv_timeout(Duration::from_secs(10)).unwrap(), h)
}

#[test]
fn an_agent_assembles_saves_and_reloads_a_3d_level_through_mcp() {
    let dir = std::env::temp_dir().join(format!("ponko-mcp-scene-editing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    project(&dir);
    // SAFETY: set before any other thread of this test binary starts (it
    // has this one test).
    unsafe { std::env::set_var("PONKO_INSTANCES_DIR", dir.join("instances")) };
    // The game runs in this process and finds assets/ in its working folder.
    std::env::set_current_dir(&dir).unwrap();
    let (_port, game) = start_game();

    let mut server = Server::new(Config {
        instances_dir: dir.join("instances"),
        logs_dir: dir.join("logs"),
        read_only: false,
    });
    let session: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scene_editing/session.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for (i, step) in session["steps"].as_array().unwrap().iter().enumerate() {
        let reply = server
            .handle(&json!({ "jsonrpc": "2.0", "id": i, "method": "tools/call", "params": { "name": step["tool"], "arguments": step["arguments"] } }))
            .unwrap();
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(result["isError"], false, "step {i} ({}): {text}", step["tool"]);
        let got: Value = serde_json::from_str(text).unwrap();
        if let Some(want) = step.get("expect") {
            assert!(
                matches(want, &got),
                "step {i} ({}): wanted {want}, got {got}",
                step["tool"]
            );
        }
    }

    // The tree the editor shows after the load: two houses and the lamp
    // post hanging off the turned one.
    let tree = server
        .handle(&json!({ "jsonrpc": "2.0", "id": 99, "method": "tools/call", "params": { "name": "scene_hierarchy", "arguments": {} } }))
        .unwrap();
    let tree: Value = serde_json::from_str(tree["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let links: Vec<(Value, Value)> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["entity"].clone(), n["link"].clone()))
        .collect();
    assert_eq!(
        links,
        vec![
            (json!(0), json!(null)),
            (json!(1), json!(null)),
            (json!(2), json!("attached3d")),
            (json!(3), json!("attached3d")),
        ],
        "{tree}"
    );
    server
        .handle(&json!({ "jsonrpc": "2.0", "id": 100, "method": "tools/call", "params": { "name": "quit_instance", "arguments": {} } }))
        .unwrap();
    let world = game.join().unwrap();
    let saved = std::fs::read_to_string(dir.join("assets/levels/yard.json"));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(saved.unwrap().contains("Attached3D"));

    let at = |e: u64| *world.get::<Transform>(Entity::from_bits(e)).unwrap();
    assert_eq!(at(0).position, Vec3::new(0.5, 0.0, 0.0), "snapped to the grid");
    let turned = at(1);
    assert!((turned.position - Vec3::new(6.0, 0.0, -4.0)).length() < 1e-4);
    assert!(
        turned
            .rotation
            .angle_between(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
            < 1e-4
    );
    assert_eq!(
        world.get::<Attached>(Entity::from_bits(2)).unwrap().to,
        Entity::from_bits(1)
    );
    // The post was placed at (3, 0, 3), then carried 4 units along -Z with its parent.
    assert!(
        (at(2).position - Vec3::new(3.0, 0.0, -1.0)).length() < 1e-4,
        "{}",
        at(2).position
    );
    assert!(
        (at(3).position - Vec3::new(3.0, 2.0, -1.0)).length() < 1e-4,
        "{}",
        at(3).position
    );
}
