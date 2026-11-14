//! `ponko-mcp`: a [Model Context Protocol](https://modelcontextprotocol.io)
//! server that lets AI agents inspect and drive running Ponko games.
//!
//! Agents launch it over stdio. It finds games through the instance registry
//! (`~/.ponko/instances`) that every game with the control endpoint on
//! writes to, and talks to each game's endpoint over localhost TCP. Every
//! tool takes an `instance`, so several agents, each with its own
//! `ponko-mcp`, can work with several games at once.
//!
//! The protocol logic is in [`Server::handle`], which maps one JSON-RPC
//! message to its reply, so it can be tested without stdio.
//!
//! ## Quickstart
//!
//! Install the binary and register it with an agent:
//!
//! ```sh
//! cargo install ponko_mcp
//! claude mcp add ponko -- ponko-mcp            # Claude Code
//! claude mcp add ponko-view -- ponko-mcp --read-only
//! ```
//!
//! The endpoint is off by default: a game turns it on with
//! `Game::control_endpoint(true)`, or a debug build with `PONKO_CONTROL=1`
//! (see the guide's
//! [control endpoint](https://ponko.dev/guide/control.html) chapter). The same
//! server as a library, answering a JSON-RPC message without stdio:
//!
//! ```
//! use ponko_mcp::{Config, Server};
//! use serde_json::json;
//!
//! let mut server = Server::new(Config::from_env());
//! let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
//!                    "params": { "protocolVersion": "2025-06-18" } });
//! let reply = server.handle(&init).unwrap();
//! assert_eq!(reply["result"]["serverInfo"]["name"], "ponko-mcp");
//! ```
//!
//! [`serve_stdio`] runs that loop over stdin and stdout, as the binary does.
//! The guide's [AI agents chapter](https://ponko.dev/guide/agents.html) lists
//! every tool and how agents use them.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

/// MCP protocol versions this server speaks, newest first.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The engine feature that lets a release build honor `PONKO_CONTROL`;
/// `launch` turns it on for the release builds it makes, through the game's
/// own name for the engine dependency (see [`control_env_feature`]).
const CONTROL_ENV: &str = "control-env";
/// The start of what a release build without that feature prints when
/// `PONKO_CONTROL` is set (the engine's `ENV_CONTROL_IGNORED`).
const CONTROL_ENV_IGNORED: &str = "ponko: PONKO_CONTROL is ignored";

/// How long to wait for a TCP connection to an instance.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a single endpoint call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Where games register: `$PONKO_INSTANCES_DIR`, else `~/.ponko/instances`
/// (`%LOCALAPPDATA%\ponko\instances` on Windows). Matches the engine.
///
/// ```
/// let dir = ponko_mcp::default_instances_dir();
/// assert!(dir.is_none_or(|d| d.ends_with("instances") || std::env::var_os("PONKO_INSTANCES_DIR").is_some()));
/// ```
pub fn default_instances_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PONKO_INSTANCES_DIR") {
        return Some(PathBuf::from(dir));
    }
    ponko_home().map(|h| h.join("instances"))
}

/// `~/.ponko` (`%LOCALAPPDATA%\ponko` on Windows).
fn ponko_home() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("ponko"))
    } else {
        std::env::var_os("HOME").map(|d| PathBuf::from(d).join(".ponko"))
    }
}

/// The feature to add to a release `cargo build` or `cargo run` in `cwd`
/// (of `package`, else the package `cwd` is in) so the game honors
/// `PONKO_CONTROL`: `<dependency>/control-env`, where `<dependency>` is the
/// package's name for its `ponko` dependency (`engine` for
/// `engine = { package = "ponko", ... }`). `None` when the package has no
/// direct, normal (not dev- or build-) `ponko` dependency, or `cargo
/// metadata` fails or takes longer than 30 seconds: the build then
/// gets no extra feature, and a release game started from it ignores
/// `PONKO_CONTROL` unless it turns the endpoint on itself.
///
/// ```no_run
/// let feature = ponko_mcp::control_env_feature(std::path::Path::new("."), None);
/// assert_eq!(feature.as_deref(), Some("ponko/control-env")); // in the engine's own folder
/// ```
pub fn control_env_feature(cwd: &Path, package: Option<&str>) -> Option<String> {
    let mut child = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read on a thread, so a large reply can't fill the pipe while we wait.
    // Its result comes over a channel, never by joining it: a process cargo
    // started could keep the pipe open, and the read with it.
    let mut stdout = child.stdout.take()?;
    let (sent, output) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = sent.send(std::io::Read::read_to_end(&mut stdout, &mut out).map(|_| out));
    });
    // cargo gets all but the last second; its output, the rest.
    let started = Instant::now();
    let deadline = started + METADATA_TIMEOUT - Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(Some(_)) => return None,
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    // cargo is done; what it wrote is in the pipe already.
    let left = (started + METADATA_TIMEOUT).saturating_duration_since(Instant::now());
    let out = output.recv_timeout(left).ok()?.ok()?;
    let metadata: Value = serde_json::from_slice(&out).ok()?;
    let cwd = cwd.canonicalize().ok()?;
    control_env_feature_in(&metadata, &cwd, package)
}

/// How long [`control_env_feature`] waits for `cargo metadata` and its
/// output, at most.
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);

/// [`control_env_feature`] from `cargo metadata` output.
fn control_env_feature_in(metadata: &Value, cwd: &Path, package: Option<&str>) -> Option<String> {
    let packages = metadata["packages"].as_array()?;
    let dir = |p: &Value| {
        p["manifest_path"]
            .as_str()
            .and_then(|m| Path::new(m).parent())
            .map(Path::to_path_buf)
    };
    let chosen = match package {
        Some(name) => packages.iter().find(|p| p["name"] == name),
        // The package whose folder `cwd` is in, the innermost one: what
        // `cargo run` without `-p` builds.
        None => packages
            .iter()
            .filter_map(|p| Some((p, dir(p)?)))
            .filter(|(_, d)| cwd.starts_with(d.canonicalize().as_deref().unwrap_or(d)))
            .max_by_key(|(_, d)| d.components().count())
            .map(|(p, _)| p)
            .or_else(|| (packages.len() == 1).then(|| &packages[0])),
    }?;
    if chosen["name"] == "ponko" {
        return Some(format!("ponko/{CONTROL_ENV}"));
    }
    let dep = chosen["dependencies"]
        .as_array()?
        .iter()
        .find(|d| d["name"] == "ponko" && d["kind"].is_null())?;
    let name = dep["rename"].as_str().or_else(|| dep["name"].as_str())?;
    Some(format!("{name}/{CONTROL_ENV}"))
}

/// The `--features` of a `cargo run` that `launch` makes: the caller's, plus
/// `control_env` (from [`control_env_feature`]) for a release build unless
/// the caller already turns on some `control-env`.
fn launch_features<'a>(args: &'a Value, control_env: Option<&'a str>) -> Vec<&'a str> {
    let mut list: Vec<&str> = args["features"]
        .as_array()
        .map(|f| f.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let has_it = list
        .iter()
        .any(|f| *f == CONTROL_ENV || f.ends_with(&format!("/{CONTROL_ENV}")));
    if let Some(feature) = control_env
        && !has_it
    {
        list.push(feature);
    }
    list
}

/// Sets the caller's extra environment for a launched game, over the
/// variables `launch` sets itself.
fn apply_launch_env(cmd: &mut Command, env: &Map<String, Value>) {
    for (key, value) in env {
        let value = value.as_str().map_or_else(|| value.to_string(), str::to_string);
        cmd.env(key, value);
    }
}

/// Server settings.
///
/// ```
/// let mut config = ponko_mcp::Config::from_env();
/// config.read_only = true; // only tools that don't change games
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to look for running instances.
    pub instances_dir: PathBuf,
    /// Where `launch` writes game logs.
    pub logs_dir: PathBuf,
    /// Hide every tool that changes a game (spawn, set, step, launch, ...).
    pub read_only: bool,
}

impl Config {
    /// The default directories, writable tools on.
    pub fn from_env() -> Self {
        let home = ponko_home().unwrap_or_else(|| std::env::temp_dir().join("ponko"));
        Self {
            instances_dir: default_instances_dir().unwrap_or_else(|| home.join("instances")),
            logs_dir: home.join("logs"),
            read_only: false,
        }
    }
}

/// One running game, from its registry file.
#[derive(Debug, Clone)]
struct Instance {
    info: Value,
    file: PathBuf,
}

impl Instance {
    fn name(&self) -> &str {
        self.info["name"].as_str().unwrap_or("")
    }
    fn pid(&self) -> u64 {
        self.info["pid"].as_u64().unwrap_or(0)
    }
    /// `<pid>-<port>`: unique even when one process runs several games.
    fn id(&self) -> String {
        format!("{}-{}", self.pid(), self.port())
    }
    fn port(&self) -> u16 {
        self.info["port"].as_u64().unwrap_or(0) as u16
    }
    fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port()))
    }
    /// The registry entry without the token: what agents get to see.
    fn public(&self) -> Value {
        let mut v = self.info.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("token");
            o.insert("id".into(), json!(self.id()));
        }
        v
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let unreachable = |e: std::io::Error| format!("instance `{}` is not reachable: {e}", self.name());
        let stream = TcpStream::connect_timeout(&self.addr(), CONNECT_TIMEOUT).map_err(unreachable)?;
        stream.set_read_timeout(Some(CALL_TIMEOUT)).map_err(unreachable)?;
        let mut writer = stream.try_clone().map_err(unreachable)?;
        let msg = json!({ "id": 1, "token": self.info["token"], "method": method, "params": params });
        writeln!(writer, "{msg}").map_err(unreachable)?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).map_err(unreachable)?;
        let reply: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply from the game: {e}"))?;
        match reply.get("error") {
            Some(e) => Err(e.as_str().unwrap_or("unknown error").to_string()),
            None => Ok(reply["result"].clone()),
        }
    }
}

/// `Some((pid, port))` if `path` is named like a registry file: `<pid>-<port>.json`.
fn registry_name(path: &Path) -> Option<(u64, u16)> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".json")?;
    let (pid, port) = stem.split_once('-')?;
    Some((pid.parse().ok()?, port.parse().ok()?))
}

/// Reads every registry file. Entries whose endpoint refuses connections
/// belong to games that crashed; their files are removed. Only files that
/// are registry entries (named `<pid>-<port>.json`, with a matching pid,
/// port and a token inside) are ever touched; anything else in the
/// directory is left alone.
fn discover(dir: &Path) -> Vec<Instance> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some((pid, port)) = registry_name(&path) else {
            continue;
        };
        let Some(info) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        else {
            continue; // being written right now
        };
        if info["pid"].as_u64() != Some(pid)
            || info["port"].as_u64() != Some(u64::from(port))
            || !info["token"].is_string()
        {
            continue; // not a registry entry after all
        }
        let inst = Instance { info, file: path };
        if TcpStream::connect_timeout(&inst.addr(), Duration::from_millis(300)).is_err() {
            let _ = std::fs::remove_file(&inst.file);
            continue;
        }
        out.push(inst);
    }
    out.sort_by_key(|i| i.info["started"].as_u64().unwrap_or(0));
    out
}

fn random_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    format!("{:016x}", h.finish())
}

// --------------------------------------------------------------------- tools

struct Tool {
    name: &'static str,
    description: &'static str,
    /// Changes the game (hidden with `--read-only`).
    writes: bool,
    /// JSON schema properties, besides `instance`.
    params: fn() -> Value,
    required: &'static [&'static str],
}

const INSTANCE_DOC: &str =
    "Which game: its id, pid, port or name from list_instances. Optional when exactly one game is running.";

fn no_params() -> Value {
    json!({})
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_instances",
            description: "List running Ponko games that have the control endpoint on (started with PONKO_CONTROL=1 or launched with `launch`). Shows id, name, pid, port, headless, cwd, start time.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "instance_info",
            description: "Live status of a game: simulation tick and time, paused, speed, fps, entity count.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "instance_capabilities",
            description: "What one running game can do, before you call anything that changes it: engine version, headless or windowed, compiled features (window, audio, physics, 3d, ...), subsystems (renderer, physics, audio, networking, control) and why each is off, every endpoint method and MCP tool with available true/false and the reason, and reflected components/resources with their field schema (status `opaque` when the game didn't describe it). Read-only. Older engines without this report say so instead of guessing.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "world_summary",
            description: "Overview of a game's world: entity count, archetypes (which component sets exist and how many entities each has), and which components and resources are reflected (readable/writable as JSON).",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "list_entities",
            description: "List entities with their reflected components as JSON and the names of their other components. Filter with `with` (component names) and `where` (conditions on reflected fields); page with `limit`/`offset`. To summarize many entities, use entity_stats instead.",
            writes: false,
            params: || {
                json!({
                    "with": { "type": "array", "items": { "type": "string" }, "description": "Only entities that have all these components, e.g. [\"Transform2D\", \"Health\"]" },
                    "where": { "type": "array", "items": { "type": "string" }, "description": "Only entities passing all of these, e.g. [\"Health.current < 5\", \"Team == Red\", \"Transform2D.position.0 > 100\"]. Operators: == != < <= > >=" },
                    "limit": { "type": "integer", "minimum": 0, "description": "Max entities to return (default 100, max 10000)" },
                    "offset": { "type": "integer", "minimum": 0, "description": "Skip this many matching entities first" }
                })
            },
            required: &[],
        },
        Tool {
            name: "entity_stats",
            description: "Summarize reflected fields over all matching entities without listing them: count, min, max and mean for numbers (per axis for vectors), counts per value for names and enums (e.g. how many units per team). Works on millions of entities.",
            writes: false,
            params: || {
                json!({
                    "fields": { "type": "array", "items": { "type": "string" }, "description": "Field paths, e.g. [\"Health.current\", \"Transform2D.position\", \"Team\"] (a bare component name means its whole value)" },
                    "with": { "type": "array", "items": { "type": "string" }, "description": "Only entities that have all these components" },
                    "where": { "type": "array", "items": { "type": "string" }, "description": "Only entities passing all of these, e.g. [\"Team == Red\"]" }
                })
            },
            required: &["fields"],
        },
        Tool {
            name: "get_entity",
            description: "All reflected components of one entity, as JSON keyed by component name.",
            writes: false,
            params: || json!({ "entity": { "type": "integer", "description": "Entity id from list_entities" } }),
            required: &["entity"],
        },
        Tool {
            name: "list_resources",
            description: "Every reflected resource (game-wide state such as scores, settings, the current round) with its value as JSON; null when it's not in the world right now. Resources must be reflected in the game (world.reflect_resource::<T>()).",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "get_resource",
            description: "One reflected resource as JSON.",
            writes: false,
            params: || json!({ "resource": { "type": "string", "description": "Resource name from list_resources, e.g. \"Score\"" } }),
            required: &["resource"],
        },
        Tool {
            name: "system_timings",
            description: "How long each system took (last frame and smoothed average, in ms), simulation systems first. Use it to find what's slow.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "shader_status",
            description: "The game's custom WGSL shaders (material shaders, screen effects, compute passes) and how their last compile or hot reload went: outcome (compiled, reloaded, failed, or reload_failed while the last good version stays in use), reload count, frame, and each error's line and column in the game's own .wgsl file, cause, source line and GPU variant. Check it after editing a shader to confirm the edit took effect.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "ecs_diagnostics",
            description: "Why the ECS costs what it does and why systems run in the order they do, built only when asked: archetypes (full component names, entity count, bytes per entity, allocated bytes), each component type's size and how many entities and bytes it has, and every system in run order with its phase (Input, BeforePhysics, Physics, AfterPhysics, SimLast, Frame, Animate, Late, Draw), tick interval, last/average ms, `access` (`exclusive`: undeclared, may touch anything; `declared`: a declared `&mut Ctx` system, still alone; `view`: a view system that may share a batch), `declared` (the components, resources, events and services it reads and writes, by name), `stage` and `alone` (why it can't share a batch). `schedule` has `parallel` (the parallel scheduler is on), the threshold, `stages` (systems `first..first+len`, `batch`, why the stage starts there: `phase`, `alone`, `conflict` with system `after` on `conflicts`, `barrier` or `order`; and how often a batch went to other threads or ran inline) and `dependencies` (pairs of declared systems that must stay in order, with the conflicts; at most 1000, `dependencies_total` counts all). Every stage ends with a barrier: commands apply and events become visible there. `query_cache` has the archetype index's counters. Pass `with` to count the archetypes and entities a query for those components visits. Field names are stable; `version` changes only if one is renamed or removed.",
            writes: false,
            params: || {
                json!({
                    "with": { "type": "array", "items": { "type": "string" }, "description": "Component names (short like \"Health\" or full type names): adds `matching` with the archetypes and entities that have them all" },
                    "limit": { "type": "integer", "minimum": 0, "description": "Most archetypes to list (non-empty ones, world order; default 100)" }
                })
            },
            required: &[],
        },
        Tool {
            name: "perf_report",
            description: "Where a windowed game's frame time goes, over the last few seconds: fps and frame-time percentiles; CPU time in systems, rendering and waiting on the display; GPU time per pass (shadows, opaque, bloom, ...); draw calls and the 3D models and triangles drawn for the camera and the shadows; the slowest systems; optionally the meshes drawing the most triangles. Start here for any performance question: a large display wait means the GPU is the limit. For exact numbers, run the game with PONKO_BENCH=<frames> instead (a fixed-length, vsync-off run that prints a report and quits). Pass profiling=true to also time the game's own profile_scope!/PONKO_PROFILE_SCOPE scopes and count allocations per system; ask again a few seconds later and the answer has `report` with the slowest frames and warnings with advice. The answer always has `render_inspector` (versioned `schema`): the renderer's CPU ms split into extract, prepare, encode, submit and other; GPU ms per frame and per pass; draw/model/triangle/occlusion counts; `counts.state_changes` (model-pass pipeline, bind-group, vertex/index-buffer changes and recorded draws for the last frame); the active graphics quality preset and settings; `quality_autotune` (null when disabled) with active choices, pins, freeze state, reason and the last 16 GPU-driven decisions; `memory_estimate_bytes` (what the renderer's tables hold for the game: its textures, meshes, GPU buffers and instance pools, with the total; not the frame's own render targets, shadow maps or skinning buffers); `adapter` (GPU name, backend, driver, tier); `counts.frustum_culled_models` (plain models outside the camera's view; LOD groups, terrain, skinned models, models with morph targets and GPU-culled batches aren't counted); and with `history`, the last drawn frames one by one. A metric that can't be measured is null, never 0, and `unavailable` says why (for example a GPU without timestamp queries). To see what a setting costs: perf_report, set_graphics_quality, wait a few seconds, perf_report again, and compare `render_inspector.gpu_ms.frame` (the inspector's top-level GPU time, and the answer's `frame_ms`), not the GPU fields of the `render_inspector.history` entries.",
            writes: false,
            params: || {
                json!({
                    "meshes": { "type": "integer", "minimum": 0, "description": "Also list this many meshes drawing the most triangles (3D games; default 0)" },
                    "profiling": { "type": "boolean", "description": "true turns on scope timing and allocation counting (and the deeper `report`), false turns it off" },
                    "history": { "type": "integer", "minimum": 0, "description": "Also list this many of the last drawn frames in `render_inspector.history`, oldest first (frame, cpu_ms, gpu_smoothed_ms: the smoothed GPU time then, gpu_latest_ms and gpu_latest_frames_ago: the newest single frame's GPU time that had come back and how many frames earlier it was drawn, draw_calls, models, triangles, shadow_triangles, upload_bytes); at most 120 are kept; default 0" }
                })
            },
            required: &[],
        },
        Tool {
            name: "record_trace",
            description: "Record the next frames of a windowed game as a Chrome/Perfetto trace JSON (each frame's systems, render, display wait and GPU passes on a timeline), to find hitches: slow frames and what was in them. Returns the path; the file is written once the frames are done (about frames / fps seconds). Each event has name, ts and dur in microseconds.",
            writes: false,
            params: || {
                json!({
                    "frames": { "type": "integer", "minimum": 1, "description": "How many frames (default 120, max 10000)" },
                    "path": { "type": "string", "description": "A new .json file relative to the game's folder, without `..` (default: a temp file)" }
                })
            },
            required: &[],
        },
        Tool {
            name: "perf_hud",
            description: "Show or hide a windowed game's on-screen performance overlay (fps, frame-time percentiles, CPU and GPU ms, the slowest GPU passes), e.g. before a screenshot.",
            writes: false,
            params: || json!({ "on": { "type": "boolean", "description": "true shows it, false hides it" } }),
            required: &["on"],
        },
        Tool {
            name: "set_graphics_quality",
            description: "Switch a 3D game's graphics quality preset while it runs (render scale, shadows, ambient occlusion, reflections, anti-aliasing), fitted to what its GPU can do. Returns the settings before (`previous`) and after (`quality`), and `fitted_to_gpu`: true when the preset had to be lowered for the GPU (always false for `recommended`, which already fits); after fitting, `quality.level` may read `custom`. Use it between two perf_report calls to measure what quality costs. It only changes the picture, never the simulation, and it pins render scale and shadow resolution against the game's automatic quality tuning until the game restarts. Not offered by a read-only server: it changes what the player sees.",
            writes: true,
            params: || {
                json!({
                    "level": { "type": "string", "enum": ["low", "medium", "high", "ultra", "recommended"], "description": "The preset; `recommended` is the engine's choice for this GPU" }
                })
            },
            required: &["level"],
        },
        Tool {
            name: "list_events",
            description: "Event types the game lets tools see (ctx.events.reflect, or C events whose type is a reflected component) and how many of each are in flight now.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "subscribe_events",
            description: "Start collecting a game's events (messages between systems, e.g. Died or Scored) as they happen, so take_events can show what happened while ticks ran. Starts fresh; the newest `capacity` are kept.",
            writes: false,
            params: || {
                json!({
                    "types": { "type": "array", "items": { "type": "string" }, "description": "Event type names from list_events (default: all)" },
                    "capacity": { "type": "integer", "minimum": 0, "description": "Max events kept between takes (default 1000, max 100000)" }
                })
            },
            required: &[],
        },
        Tool {
            name: "take_events",
            description: "Events collected since subscribe_events or the last take, oldest first, each with its tick, type and data; plus how many were dropped for space.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "read_logs",
            description: "The game's recent stdout/stderr (println, printf, panics, engine messages), for any running game, also while a frame is stuck. Pass the returned `next` as `since` to get only newer lines.",
            writes: false,
            params: || {
                json!({
                    "lines": { "type": "integer", "minimum": 1, "description": "How many lines at most (default 100): the newest, or the first ones after `since`" },
                    "since": { "type": "integer", "minimum": 0, "description": "`next` from a previous read_logs: only lines printed after it" },
                })
            },
            required: &[],
        },
        Tool {
            name: "launch",
            description: "Build and start a game or example with `cargo run` (or start an executable that is already built, with `program`), with the control endpoint on, and wait until it is ready. Starts headless and paused by default, so you can inspect it and step it with run_ticks. Returns the new instance.",
            writes: true,
            params: || {
                json!({
                    "cwd": { "type": "string", "description": "Cargo project directory, or the directory `program` starts in (default: this server's working directory)" },
                    "program": { "type": "string", "description": "Start this executable instead of `cargo run` (a packaged game, a C or C++ game, a build made earlier); `example`, `bin`, `package`, `features` and `release` are then unused" },
                    "example": { "type": "string", "description": "Run `--example <name>`" },
                    "bin": { "type": "string", "description": "Run `--bin <name>`" },
                    "features": { "type": "array", "items": { "type": "string" }, "description": "Cargo features to turn on, e.g. [\"physics\"] or [\"3d\"]" },
                    "package": { "type": "string", "description": "Run `-p <package>`" },
                    "release": { "type": "boolean", "description": "Release build (default true; debug is 10-50x slower)" },
                    "headless": { "type": "boolean", "description": "Set HEADLESS=1, which the engine's examples read (default true)" },
                    "paused": { "type": "boolean", "description": "Start with the simulation paused (default true)" },
                    "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Extra environment variables, e.g. {\"BOIDS\": \"100000\"}" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments passed to the game" },
                    "timeout_secs": { "type": "integer", "minimum": 1, "description": "How long to wait for build + start (default 600)" }
                })
            },
            required: &[],
        },
        Tool {
            name: "screenshot",
            description: "Capture the next frame of a windowed game and return it as an image, to check what the game looks like. Headless games have no frames; launch with headless: false.",
            writes: false,
            params: || {
                json!({
                    "max_size": { "type": "integer", "minimum": 16, "description": "Scale down so neither side is bigger than this many pixels (default 1024; 0 keeps full size)" },
                    "path": { "type": "string", "description": "Also keep the PNG here: a new .png file relative to the game's folder, without `..` (default: a temp file)" }
                })
            },
            required: &[],
        },
        Tool {
            name: "set_camera",
            description: "Move a game's camera, e.g. before a screenshot: look_at centers a world point, zoom scales (2 = twice as big), position sets the world point at the window's top-left. With no arguments, just returns the camera.",
            writes: false,
            params: || {
                json!({
                    "look_at": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "World point to center, [x, y]" },
                    "zoom": { "type": "number", "exclusiveMinimum": 0, "description": "Window pixels per world unit" },
                    "position": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "World point at the top-left, [x, y]" }
                })
            },
            required: &[],
        },
        Tool {
            name: "run_ticks",
            description: "Run exactly N simulation ticks (the game should be paused) and wait for them to finish. Returns the new status. Use it to advance a simulation step by step and check results.",
            writes: true,
            params: || {
                json!({
                    "ticks": { "type": "integer", "minimum": 1, "description": "How many ticks" },
                    "timeout_secs": { "type": "integer", "minimum": 1, "description": "How long to wait (default 120)" }
                })
            },
            required: &["ticks"],
        },
        Tool {
            name: "run_input_scenario",
            description: "Run a tick-indexed input scenario against a paused game that allows input injection (launched with `launch`, or started with PONKO_INPUT_INJECTION=1) and wait for it to end. The scenario is a versioned JSON object: input steps (key, mouse, gamepad_button, gamepad_axis, focus_lost, gamepad_disconnected) fed through the game's real input path before the tick's systems run, UI steps (ui_click, ui_type) that click or type into the retained UI's node of that name (in a window, in the frame after the tick), IME steps (ime_enabled, ime_preedit with text and optional cursor [start, end] in bytes, ime_commit with text, ime_disabled) that replay an input-method composition into the focused text input, and checks (expect_action, expect_resource, expect_count, expect_ui) evaluated after them; tick 0 is the next simulation tick. Returns the report: status passed/failed/cancelled, ticks run, and for a failed check its step, tick, expected and actual values. The run stops at the first failed check. Held inputs are released when the run ends, fails or is cancelled (also on a timeout or error here), unless `keep_input_when_done` is set; then cancel_input_scenario releases them. A failed check or a cancel takes back only the ticks this run queued, not ticks someone else asked for.",
            writes: true,
            params: || {
                json!({
                    "scenario": {
                        "type": "object",
                        "description": "{\"version\": 1, \"id\"?, \"duration\"?, \"release_when_done\"?, \"steps\": [{\"tick\": 10, \"type\": \"key\", \"key\": \"Space\", \"down\": true}, {\"tick\": 10, \"type\": \"expect_action\", \"action\": \"jump\", \"pressed\": true}, {\"tick\": 12, \"type\": \"expect_resource\", \"resource\": \"Score\", \"pointer\": \"/points\", \"equals\": 3}, {\"tick\": 12, \"type\": \"expect_count\", \"component\": \"Coin\", \"equals\": 2}]}"
                    },
                    "max_ticks": { "type": "integer", "minimum": 1, "description": "Refuse scenarios longer than this (default 100000), before the running one is touched" },
                    "keep_input_when_done": { "type": "boolean", "description": "Leave the keys the scenario holds pressed when it ends (default: release them). They stay owned by the scenario: cancel_input_scenario releases them" },
                    "timeout_secs": { "type": "integer", "minimum": 1, "description": "How long to wait (default 120)" }
                })
            },
            required: &["scenario"],
        },
        Tool {
            name: "cancel_input_scenario",
            description: "End the running input scenario and release the keys, buttons and axes it holds. Returns whether one was running and its report.",
            writes: true,
            params: || json!({ "reason": { "type": "string", "description": "Why, shown in the report" } }),
            required: &[],
        },
        Tool {
            name: "watch",
            description: "Run N simulation ticks (the game should be paused) and sample it every K ticks: the tick, wall-clock ms per tick (including polling), the given reflected resources, and entity counts for the given components. Use it to see how a simulation develops (who's winning, does the population grow, does it get slower) in one call.",
            writes: true,
            params: || {
                json!({
                    "ticks": { "type": "integer", "minimum": 1, "description": "How many ticks in total" },
                    "every": { "type": "integer", "minimum": 1, "description": "Ticks between samples (default: ticks / 10)" },
                    "resources": { "type": "array", "items": { "type": "string" }, "description": "Reflected resources to read at each sample, e.g. [\"Score\"]" },
                    "count": { "type": "array", "items": { "type": "string" }, "description": "Components to count entities of at each sample, e.g. [\"Unit\"]" },
                    "timeout_secs": { "type": "integer", "minimum": 1, "description": "How long to wait in total (default 300)" }
                })
            },
            required: &["ticks"],
        },
        Tool {
            name: "pause",
            description: "Pause the simulation (rendering and input keep running in windowed games).",
            writes: true,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "resume",
            description: "Resume the simulation.",
            writes: true,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "set_speed",
            description: "Set simulation speed: 1 real time, 4 four times as fast, 0.5 half. Results don't depend on speed.",
            writes: true,
            params: || json!({ "speed": { "type": "number", "minimum": 0 } }),
            required: &["speed"],
        },
        Tool {
            name: "spawn_entity",
            description: "Spawn an entity from reflected components, e.g. {\"Transform2D\": {\"position\": [10, 20]}, \"Health\": {\"current\": 5, \"max\": 5}}. Returns its id.",
            writes: true,
            params: || json!({ "components": { "type": "object", "description": "Component name -> JSON value" } }),
            required: &["components"],
        },
        Tool {
            name: "despawn_entity",
            description: "Despawn an entity.",
            writes: true,
            params: || json!({ "entity": { "type": "integer" } }),
            required: &["entity"],
        },
        Tool {
            name: "set_component",
            description: "Write a reflected component of an entity (adds it if missing). For an existing component, object fields you give are changed and the rest keep their values, e.g. {\"current\": 1}. Returns the new value.",
            writes: true,
            params: || {
                json!({
                    "entity": { "type": "integer" },
                    "component": { "type": "string", "description": "Component name, e.g. \"Transform2D\"" },
                    "value": { "description": "JSON value (or partial object)" }
                })
            },
            required: &["entity", "component", "value"],
        },
        Tool {
            name: "set_resource",
            description: "Write a reflected resource (inserts it if missing). For an existing resource, object fields you give are changed and the rest keep their values, e.g. {\"blue\": 0}. Returns the new value.",
            writes: true,
            params: || {
                json!({
                    "resource": { "type": "string", "description": "Resource name from list_resources" },
                    "value": { "description": "JSON value (or partial object)" }
                })
            },
            required: &["resource", "value"],
        },
        Tool {
            name: "remove_component",
            description: "Remove a reflected component from an entity.",
            writes: true,
            params: || json!({ "entity": { "type": "integer" }, "component": { "type": "string" } }),
            required: &["entity", "component"],
        },
        Tool {
            name: "save_world",
            description: "Save every entity that has reflected components (with its id), and every reflected resource, to a JSON file. Use it as a checkpoint before experiments; restore with load_world. An existing file is refused unless overwrite is true.",
            writes: false,
            params: || {
                json!({
                    "path": { "type": "string", "description": "A .json file relative to the game's folder, without `..`, e.g. saves/before.json" },
                    "overwrite": { "type": "boolean", "description": "Replace the file if it exists (default false: refused). Not on a read-only server." }
                })
            },
            required: &["path"],
        },
        Tool {
            name: "load_world",
            description: "Replace every entity with those in a save file from save_world, with the same ids. The file is checked first: on error nothing changes. Reflected resources in the file are restored too; the tick, seed and other resources don't change.",
            writes: true,
            params: || json!({ "path": { "type": "string", "description": "A file from save_world, relative to the game's folder" } }),
            required: &["path"],
        },
        Tool {
            name: "save_scene",
            description: "Save the world as a scene file in the game's assets/ folder, the format the game loads with ctx.load_scene and the inspector's editor reads. Use it to write a level the game ships with.",
            writes: true,
            params: || json!({ "path": { "type": "string", "description": "Path inside assets/, e.g. levels/beach.json" } }),
            required: &["path"],
        },
        Tool {
            name: "load_scene",
            description: "Replace the world with a scene from the game's assets/ folder (entities keep their ids; images that sprites name are loaded). On error nothing changes.",
            writes: true,
            params: || json!({ "path": { "type": "string", "description": "Path inside assets/, e.g. levels/beach.json" } }),
            required: &["path"],
        },
        Tool {
            name: "spawn_prefab",
            description: "Spawn a prefab (an entity, or a tree of entities with its children, from a file in assets/) with its root's position at a world point. Returns the root entity and its parts.",
            writes: true,
            params: || {
                json!({
                    "path": { "type": "string", "description": "Path inside assets/, e.g. prefabs/palm.json" },
                    "at": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "World position [x, y]" }
                })
            },
            required: &["path", "at"],
        },
        Tool {
            name: "save_prefab",
            description: "Save an entity's reflected components as a prefab file in assets/ (its position isn't kept: each spawn says where), with its prefab parts and what is attached to them, for spawn_prefab, ctx.spawn_prefab and the editor's palette (assets/prefabs/).",
            writes: true,
            params: || {
                json!({
                    "entity": { "type": "integer", "description": "Entity id" },
                    "path": { "type": "string", "description": "Path inside assets/, e.g. prefabs/palm.json" }
                })
            },
            required: &["entity", "path"],
        },
        Tool {
            name: "despawn_prefab",
            description: "Despawn a prefab's root entity and the parts spawn_prefab made with it. Returns how many entities went.",
            writes: true,
            params: || json!({ "entity": { "type": "integer", "description": "The root entity spawn_prefab returned" } }),
            required: &["entity"],
        },
        Tool {
            name: "pick_entity",
            description: "What is under a window point of a 3D game, as a click in the editor picks it: the entity whose mesh bounds the camera's ray through that point enters first, with distance and hit point, and where the ray meets the ground (y = 0). Use set_camera_3d first to look at the level.",
            writes: false,
            params: || json!({ "at": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Window point [x, y] in pixels (0, 0 is the top left)" } }),
            required: &["at"],
        },
        Tool {
            name: "transform_entities",
            description: "Move, turn and scale 3D entities the way the editor's gizmo does, as one undo step: translate by [x, y, z], rotate by degrees about each axis (about the entities' middle), scale by a factor along each entity's own axes, in world or local space, with optional snapping. Attached entities keep their parent; what hangs off them follows.",
            writes: true,
            params: || {
                json!({
                    "entities": { "type": "array", "items": { "type": "integer" }, "minItems": 1, "description": "Entity ids" },
                    "translate": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "World units [x, y, z] (along the first entity's own axes with space: local)" },
                    "rotate": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "Degrees about the X, Y and Z axes, in that order" },
                    "scale": { "description": "A factor, or [x, y, z] factors along each entity's own axes" },
                    "space": { "type": "string", "enum": ["world", "local"], "description": "Whose axes: the world's (default) or the first entity's own" },
                    "snap": { "description": "true to snap to the grid (0.5 units, 15°, 0.1 scale steps), or { step, angle (degrees), scale }" }
                })
            },
            required: &["entities"],
        },
        Tool {
            name: "drag_gizmo",
            description: "Drag a handle of the editor's 3D gizmo from one window point to another, exactly as the mouse does in the inspector, as one undo step: an axis (x, y, z), a plane (xy, xz, yz) or the center (move in the view plane, or scale evenly). Use it to check what a person dragging would get.",
            writes: true,
            params: || {
                json!({
                    "entities": { "type": "array", "items": { "type": "integer" }, "minItems": 1, "description": "Entity ids" },
                    "mode": { "type": "string", "enum": ["translate", "rotate", "scale"] },
                    "handle": { "type": "string", "enum": ["x", "y", "z", "xy", "xz", "yz", "center"] },
                    "from": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Window point where the drag starts" },
                    "to": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Window point where it ends" },
                    "space": { "type": "string", "enum": ["world", "local"] },
                    "snap": { "description": "true to snap to the grid (0.5 units, 15°, 0.1 scale steps), or { step, angle (degrees), scale }" }
                })
            },
            required: &["entities", "mode", "handle", "from", "to"],
        },
        Tool {
            name: "set_parent",
            description: "Make an entity follow another in the scene hierarchy without moving it (Attached3D in 3D, Attached in 2D), or follow nothing (parent: null). Moving the parent then carries it. Refuses loops. One undo step.",
            writes: true,
            params: || {
                json!({
                    "entity": { "type": "integer", "description": "The child" },
                    "parent": { "type": ["integer", "null"], "description": "The new parent, or null to detach" }
                })
            },
            required: &["entity"],
        },
        Tool {
            name: "scene_hierarchy",
            description: "The scene as a tree, as the editor's hierarchy panel shows it: every entity with its parent and how it hangs off it (attached, attached3d, joint, skin, prefab_part), its depth and children, whether it is a prefab instance's root, and its component names.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "list_assets",
            description: "What the editor's asset browser offers: glTF meshes, prefabs and scenes in the game's assets/ folder, and the materials of the mesh files loaded so far (mesh, index, name), for place_asset and paint_material.",
            writes: false,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "place_asset",
            description: "Drop an asset into the scene as the editor's asset browser does, as one undo step: a glTF mesh becomes a model, a prefab is spawned with its parts. At a world point, or where a window point's ray meets the ground, optionally snapped to the grid. Returns the entity and its prefab parts.",
            writes: true,
            params: || {
                json!({
                    "path": { "type": "string", "description": "A .glb, .gltf or prefab .json inside assets/, e.g. models/house.glb" },
                    "at": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": "World point [x, y, z] ([x, y] for a 2D prefab)" },
                    "screen": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Window point to drop at (where its ray meets the ground); default the middle of the view" },
                    "snap": { "description": "true to snap to the grid (0.5 units, 15°, 0.1 scale steps), or { step, angle (degrees), scale }" }
                })
            },
            required: &["path"],
        },
        Tool {
            name: "paint_material",
            description: "Draw a model with a material of a loaded mesh file instead of its own (a ModelMaterial), as dropping a material from the asset browser does. One undo step.",
            writes: true,
            params: || {
                json!({
                    "entity": { "type": "integer" },
                    "mesh": { "type": "string", "description": "The mesh file the material is in, from list_assets" },
                    "material": { "description": "The material's name, or its number in the file" }
                })
            },
            required: &["entity", "mesh", "material"],
        },
        Tool {
            name: "delete_entities",
            description: "Delete entities with everything that hangs off them (prefab parts, attachments), as the editor's Delete key does. One undo step: undo_edit brings them back, still linked.",
            writes: true,
            params: || json!({ "entities": { "type": "array", "items": { "type": "integer" }, "minItems": 1, "description": "Entity ids" } }),
            required: &["entities"],
        },
        Tool {
            name: "duplicate_entities",
            description: "Copy entities with everything that hangs off them (links inside a copy point at the copies), moved by an offset. One undo step. Returns the copies.",
            writes: true,
            params: || {
                json!({
                    "entities": { "type": "array", "items": { "type": "integer" }, "minItems": 1, "description": "Entity ids" },
                    "offset": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": "[x, y, z] in 3D, [x, y] in 2D" }
                })
            },
            required: &["entities"],
        },
        Tool {
            name: "undo_edit",
            description: "Undo the last editor change, made by an agent or by a person in the inspector (they share one history).",
            writes: true,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "redo_edit",
            description: "Redo the last undone editor change.",
            writes: true,
            params: no_params,
            required: &[],
        },
        Tool {
            name: "save_edited_scene",
            description: "Save the world back to the scene file it was loaded from or last saved to, as the editor's Save does (or to path, inside assets/).",
            writes: true,
            params: || json!({ "path": { "type": "string", "description": "Path inside assets/, e.g. levels/yard.json (default: the scene that was loaded)" } }),
            required: &[],
        },
        Tool {
            name: "set_camera_3d",
            description: "Read or move the 3D camera as the editor's fly camera does, e.g. before pick_entity, drag_gizmo or a screenshot: position, look_at, fov (degrees), turn [dx, dy] (mouse pixels), fly [right, up, forward] (world units), focus (an entity to frame). With no arguments, just returns the camera.",
            writes: false,
            params: || {
                json!({
                    "position": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 },
                    "look_at": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 },
                    "fov": { "type": "number" },
                    "turn": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                    "fly": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 },
                    "focus": { "type": "integer", "description": "An entity to frame" }
                })
            },
            required: &[],
        },
        Tool {
            name: "quit_instance",
            description: "Ask a game to exit.",
            writes: true,
            params: no_params,
            required: &[],
        },
    ]
}

/// The endpoint methods an MCP tool needs from the game. `None` for an
/// unknown tool; empty for tools that work without one (or fall back).
fn tool_methods(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "list_instances" | "launch" | "read_logs" => &[],
        "instance_info" => &["info"],
        "instance_capabilities" => &["capabilities"],
        "world_summary" => &["world.summary"],
        "list_entities" => &["entities.list"],
        "entity_stats" => &["entities.stats"],
        "get_entity" => &["entity.get"],
        "list_resources" => &["resources.list"],
        "get_resource" => &["resource.get"],
        "set_resource" => &["resource.set"],
        "system_timings" => &["systems.timings"],
        "ecs_diagnostics" => &["ecs.diagnostics"],
        "perf_report" => &["perf.get"],
        "shader_status" => &["shaders.status"],
        "record_trace" => &["perf.trace"],
        "perf_hud" => &["perf.hud"],
        "set_graphics_quality" => &["quality.set"],
        "list_events" => &["events.list"],
        "subscribe_events" => &["events.subscribe"],
        "take_events" => &["events.take"],
        "screenshot" => &["screenshot"],
        "set_camera" => &["camera.set"],
        "run_ticks" => &["info", "sim.step"],
        "watch" => &["info", "sim.step", "resource.get", "world.summary"],
        "run_input_scenario" => &[
            "info",
            "sim.step",
            "input.scenario.load",
            "input.scenario.status",
            "input.scenario.cancel",
        ],
        "cancel_input_scenario" => &["input.scenario.cancel"],
        "pause" => &["sim.pause"],
        "resume" => &["sim.resume"],
        "set_speed" => &["sim.speed"],
        "spawn_entity" => &["entity.spawn"],
        "despawn_entity" => &["entity.despawn"],
        "set_component" => &["component.set"],
        "remove_component" => &["component.remove"],
        "save_world" => &["world.save"],
        "load_world" => &["world.load"],
        "save_scene" => &["scene.save"],
        "load_scene" => &["scene.load"],
        "spawn_prefab" => &["prefab.spawn"],
        "save_prefab" => &["prefab.save"],
        "despawn_prefab" => &["prefab.despawn"],
        "quit_instance" => &["game.quit"],
        "pick_entity" => &["editor.pick"],
        "transform_entities" => &["editor.transform"],
        "drag_gizmo" => &["editor.drag"],
        "set_parent" => &["editor.parent"],
        "scene_hierarchy" => &["editor.hierarchy"],
        "list_assets" => &["editor.assets"],
        "place_asset" => &["editor.place"],
        "paint_material" => &["editor.material"],
        "delete_entities" => &["editor.delete"],
        "duplicate_entities" => &["editor.duplicate"],
        "undo_edit" => &["editor.undo"],
        "redo_edit" => &["editor.redo"],
        "save_edited_scene" => &["editor.save"],
        "set_camera_3d" => &["editor.camera"],
        _ => return None,
    })
}

/// Each MCP tool's availability for one game, from the game's method
/// report (`[{name, available, reason?}]`) and this server's mode.
fn tool_availability(methods: &[Value], read_only: bool) -> Vec<Value> {
    tools()
        .iter()
        .map(|t| {
            let refused = if read_only && t.writes {
                Some("changes the game and this server is read-only".to_string())
            } else {
                tool_methods(t.name).unwrap_or(&[]).iter().find_map(|m| {
                    match methods.iter().find(|x| x["name"] == *m) {
                        None => Some(format!("this game's engine has no `{m}` method")),
                        Some(x) if x["available"] == false => {
                            Some(x["reason"].as_str().unwrap_or("refused by the game").to_string())
                        }
                        Some(_) => None,
                    }
                })
            };
            match refused {
                Some(reason) => json!({ "name": t.name, "available": false, "reason": reason }),
                None => json!({ "name": t.name, "available": true }),
            }
        })
        .collect()
}

fn tool_schema(t: &Tool) -> Value {
    let mut props = (t.params)();
    if t.name != "list_instances" && t.name != "launch" {
        props.as_object_mut().unwrap().insert(
            "instance".into(),
            json!({ "type": ["string", "integer"], "description": INSTANCE_DOC }),
        );
    }
    json!({
        "name": t.name,
        "description": t.description,
        "inputSchema": { "type": "object", "properties": props, "required": t.required },
        "annotations": { "readOnlyHint": !t.writes, "destructiveHint": destructive(t.name) },
    })
}

/// Tools whose change can't simply be undone: they delete, replace the
/// whole world, write or overwrite files, or start or stop a process. MCP
/// clients use the hint to ask the user before calling them.
fn destructive(tool: &str) -> bool {
    matches!(
        tool,
        "despawn_entity"
            | "remove_component"
            | "quit_instance"
            | "launch"
            | "load_world"
            | "load_scene"
            | "save_scene"
            | "save_prefab"
            | "delete_entities"
            | "save_edited_scene"
    )
}

// -------------------------------------------------------------------- server

/// The MCP server state. One per agent connection.
///
/// ```
/// use ponko_mcp::{Config, Server};
/// use serde_json::json;
///
/// let mut server = Server::new(Config::from_env());
/// let reply = server.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
/// assert!(reply["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "list_instances"));
/// ```
pub struct Server {
    config: Config,
    /// Log files of games this server launched, by launch id.
    launched: HashMap<String, PathBuf>,
}

impl Server {
    /// A server with `config`, before any request.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            launched: HashMap::new(),
        }
    }

    /// Handles one JSON-RPC message. Returns the reply, or `None` for
    /// notifications (which get no reply).
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.visible_tools().iter().map(tool_schema).collect::<Vec<_>>() })),
            "tools/call" => Ok(self.call_tool(&params)),
            m if m.starts_with("notifications/") => return None,
            m => Err((-32601, format!("method not found: {m}"))),
        };
        let id = id?; // a request without id is a notification: no reply
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let asked = params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSIONS[0]);
        let version = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == asked)
            .unwrap_or(&PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "ponko-mcp", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Tools for Ponko games. Start with list_instances; if none is running, use launch (headless and paused by default). Then world_summary and list_entities to look around, run_ticks to advance the simulation, and get_entity / set_component to check and change state. Every tool except list_instances and launch takes `instance`, optional when only one game runs. Components must be reflected in the game (world.reflect::<T>()) to be read or written as JSON."
        })
    }

    fn visible_tools(&self) -> Vec<Tool> {
        tools()
            .into_iter()
            .filter(|t| !(self.config.read_only && t.writes))
            .collect()
    }

    fn call_tool(&mut self, params: &Value) -> Value {
        let name = params["name"].as_str().unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let result = match self.visible_tools().iter().find(|t| t.name == name) {
            None if tools().iter().any(|t| t.name == name) => {
                Err(format!("`{name}` changes the game and this server is read-only"))
            }
            None => Err(format!("unknown tool `{name}`")),
            Some(_) if name == "screenshot" => {
                return match self.screenshot(&args) {
                    Ok(content) => json!({ "content": content, "isError": false }),
                    Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                };
            }
            Some(_) => self.run_tool(name, &args),
        };
        match result {
            Ok(v) => json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap() }],
                "isError": false,
            }),
            Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
        }
    }

    /// Finds the instance an agent means.
    fn instance(&self, args: &Value) -> Result<Instance, String> {
        let all = discover(&self.config.instances_dir);
        let wanted = match args.get("instance") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(v) => Some(v.to_string()),
        };
        let Some(wanted) = wanted else {
            return match all.len() {
                1 => Ok(all.into_iter().next().unwrap()),
                0 => Err("no game is running with the control endpoint on; start one with `launch`, or run it with PONKO_CONTROL=1".into()),
                n => Err(format!(
                    "{n} games are running; pass `instance` (one of: {})",
                    all.iter().map(|i| format!("{} \"{}\"", i.id(), i.name())).collect::<Vec<_>>().join(", ")
                )),
            };
        };
        let mut matches: Vec<Instance> = all
            .iter()
            .filter(|i| i.id() == wanted || i.pid().to_string() == wanted || i.port().to_string() == wanted)
            .cloned()
            .collect();
        if matches.is_empty() {
            matches = all.iter().filter(|i| i.name() == wanted).cloned().collect();
        }
        match matches.len() {
            1 => Ok(matches.pop().unwrap()),
            0 => Err(format!("no running instance `{wanted}`; see list_instances")),
            _ => Err(format!(
                "several instances match `{wanted}`; pass the id from list_instances"
            )),
        }
    }

    fn run_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let pass = |keys: &[&str]| {
            let mut m = Map::new();
            for k in keys {
                if let Some(v) = args.get(*k) {
                    m.insert((*k).into(), v.clone());
                }
            }
            Value::Object(m)
        };
        match name {
            "list_instances" => Ok(Value::Array(
                discover(&self.config.instances_dir)
                    .iter()
                    .map(Instance::public)
                    .collect(),
            )),
            "instance_info" => {
                let i = self.instance(args)?;
                let mut info = i.call("info", json!({}))?;
                info["name"] = json!(i.name());
                Ok(info)
            }
            "instance_capabilities" => self.capabilities(args),
            "world_summary" => self.instance(args)?.call("world.summary", json!({})),
            "list_entities" => self
                .instance(args)?
                .call("entities.list", pass(&["with", "where", "limit", "offset"])),
            "entity_stats" => self
                .instance(args)?
                .call("entities.stats", pass(&["fields", "with", "where"])),
            "get_entity" => self.instance(args)?.call("entity.get", pass(&["entity"])),
            "system_timings" => self.instance(args)?.call("systems.timings", json!({})),
            "ecs_diagnostics" => self.instance(args)?.call("ecs.diagnostics", pass(&["with", "limit"])),
            "perf_report" => self
                .instance(args)?
                .call("perf.get", pass(&["meshes", "profiling", "history"])),
            "shader_status" => self.instance(args)?.call("shaders.status", json!({})),
            "record_trace" => self.instance(args)?.call("perf.trace", pass(&["frames", "path"])),
            "perf_hud" => self.instance(args)?.call("perf.hud", pass(&["on"])),
            "set_graphics_quality" => self.instance(args)?.call("quality.set", pass(&["level"])),
            "list_resources" => self.instance(args)?.call("resources.list", json!({})),
            "get_resource" => self.instance(args)?.call("resource.get", pass(&["resource"])),
            "set_resource" => self.instance(args)?.call("resource.set", pass(&["resource", "value"])),
            "read_logs" => self.read_logs(args),
            "list_events" => self.instance(args)?.call("events.list", json!({})),
            "subscribe_events" => self
                .instance(args)?
                .call("events.subscribe", pass(&["types", "capacity"])),
            "take_events" => self.instance(args)?.call("events.take", json!({})),
            "set_camera" => self
                .instance(args)?
                .call("camera.set", pass(&["position", "zoom", "look_at"])),
            "launch" => self.launch(args),
            "run_ticks" => self.run_ticks(args),
            "watch" => self.watch(args),
            "run_input_scenario" => self.run_input_scenario(args),
            "cancel_input_scenario" => self.instance(args)?.call("input.scenario.cancel", pass(&["reason"])),
            "pause" => self.instance(args)?.call("sim.pause", json!({})),
            "resume" => self.instance(args)?.call("sim.resume", json!({})),
            "set_speed" => self.instance(args)?.call("sim.speed", pass(&["speed"])),
            "spawn_entity" => self.instance(args)?.call("entity.spawn", pass(&["components"])),
            "despawn_entity" => self.instance(args)?.call("entity.despawn", pass(&["entity"])),
            "set_component" => self
                .instance(args)?
                .call("component.set", pass(&["entity", "component", "value"])),
            "remove_component" => self
                .instance(args)?
                .call("component.remove", pass(&["entity", "component"])),
            "save_world" => {
                // Replacing a file changes more than the game; a read-only
                // server may only make new ones.
                if self.config.read_only && args["overwrite"] == true {
                    return Err("this server is read-only: save_world may only write a new file (no overwrite)".into());
                }
                self.instance(args)?.call("world.save", pass(&["path", "overwrite"]))
            }
            "load_world" => self.instance(args)?.call("world.load", pass(&["path"])),
            "save_scene" => self.instance(args)?.call("scene.save", pass(&["path"])),
            "load_scene" => self.instance(args)?.call("scene.load", pass(&["path"])),
            "spawn_prefab" => self.instance(args)?.call("prefab.spawn", pass(&["path", "at"])),
            "save_prefab" => self.instance(args)?.call("prefab.save", pass(&["entity", "path"])),
            "despawn_prefab" => self.instance(args)?.call("prefab.despawn", pass(&["entity"])),
            "quit_instance" => self.instance(args)?.call("game.quit", json!({})),
            "pick_entity" => self.instance(args)?.call("editor.pick", pass(&["at"])),
            "transform_entities" => self.instance(args)?.call(
                "editor.transform",
                pass(&["entities", "translate", "rotate", "scale", "space", "snap"]),
            ),
            "drag_gizmo" => self.instance(args)?.call(
                "editor.drag",
                pass(&["entities", "mode", "handle", "from", "to", "space", "snap"]),
            ),
            "set_parent" => self.instance(args)?.call("editor.parent", pass(&["entity", "parent"])),
            "scene_hierarchy" => self.instance(args)?.call("editor.hierarchy", json!({})),
            "list_assets" => self.instance(args)?.call("editor.assets", json!({})),
            "place_asset" => self
                .instance(args)?
                .call("editor.place", pass(&["path", "at", "screen", "snap"])),
            "paint_material" => self
                .instance(args)?
                .call("editor.material", pass(&["entity", "mesh", "material"])),
            "delete_entities" => self.instance(args)?.call("editor.delete", pass(&["entities"])),
            "duplicate_entities" => self
                .instance(args)?
                .call("editor.duplicate", pass(&["entities", "offset"])),
            "undo_edit" => self.instance(args)?.call("editor.undo", json!({})),
            "redo_edit" => self.instance(args)?.call("editor.redo", json!({})),
            "save_edited_scene" => self.instance(args)?.call("editor.save", pass(&["path"])),
            "set_camera_3d" => self.instance(args)?.call(
                "editor.camera",
                pass(&["position", "look_at", "fov", "turn", "fly", "focus"]),
            ),
            _ => Err(format!("unknown tool `{name}`")),
        }
    }

    /// The `instance_capabilities` tool: the game's own report plus which
    /// MCP tools it supports here. Engines older than the `capabilities`
    /// method get a report that says what can't be known.
    fn capabilities(&self, args: &Value) -> Result<Value, String> {
        let i = self.instance(args)?;
        let mut report = match i.call("capabilities", json!({})) {
            Ok(r) => r,
            Err(e) if e.contains("unknown method") => {
                // Only names: an older engine can't say what it would refuse.
                let methods: Vec<Value> = i
                    .call("methods", json!({}))?
                    .as_array()
                    .map(|m| {
                        m.iter()
                            .map(|m| json!({ "name": m["name"], "about": m["about"], "available": null }))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({
                    "format": "ponko-capabilities",
                    "version": null,
                    "supported": false,
                    "reason": "this game's engine predates the `capabilities` method: features, subsystems, method availability and schemas are unknown",
                    "engine_version": i.info.get("engine_version").cloned().unwrap_or(Value::Null),
                    "mode": match i.info.get("headless").and_then(Value::as_bool) {
                        Some(true) => json!("headless"),
                        Some(false) => json!("windowed"),
                        None => Value::Null,
                    },
                    "methods": methods,
                })
            }
            Err(e) => return Err(e),
        };
        let methods = report["methods"].as_array().cloned().unwrap_or_default();
        let mut tools = tool_availability(&methods, self.config.read_only);
        if report.get("supported") == Some(&json!(false)) {
            // Present methods may still be refused at run time: unknown, not available.
            for t in &mut tools {
                if t["available"] == true && !tool_methods(t["name"].as_str().unwrap_or("")).unwrap_or(&[]).is_empty() {
                    t["available"] = Value::Null;
                    t["reason"] = json!("unknown: the engine can't report availability");
                }
            }
        }
        report["name"] = json!(i.name());
        report["mcp"] = json!({ "read_only": self.config.read_only, "tools": tools });
        Ok(report)
    }

    /// The `screenshot` tool: the image plus a line saying where it is.
    fn screenshot(&self, args: &Value) -> Result<Value, String> {
        let max_size = match args.get("max_size") {
            None | Some(Value::Null) => 1024,
            Some(v) => v.as_u64().ok_or("`max_size` must be a whole number")?,
        };
        let mut params = json!({ "max_size": max_size });
        if let Some(path) = args.get("path").filter(|p| !p.is_null()) {
            params["path"] = path.clone();
        }
        let shot = self.instance(args)?.call("screenshot", params)?;
        let path = shot["path"]
            .as_str()
            .ok_or("the game didn't say where it saved the screenshot")?;
        let png = std::fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
        Ok(json!([
            { "type": "image", "data": base64(&png), "mimeType": "image/png" },
            { "type": "text", "text": format!("{}x{} screenshot, saved at {path}", shot["width"], shot["height"]) },
        ]))
    }

    fn run_ticks(&self, args: &Value) -> Result<Value, String> {
        let ticks = args["ticks"]
            .as_u64()
            .filter(|t| *t > 0)
            .ok_or("`ticks` must be a positive integer")?;
        let deadline = deadline_after(args["timeout_secs"].as_u64().unwrap_or(120));
        let i = self.instance(args)?;
        let start = paused_tick(&i)?;
        let begun = Instant::now();
        let info = step_and_wait(&i, start, ticks, deadline)?;
        Ok(json!({
            "ticks_run": info["tick"].as_u64().unwrap_or(0) - start,
            "seconds": begun.elapsed().as_secs_f64(),
            "status": info,
        }))
    }

    fn run_input_scenario(&self, args: &Value) -> Result<Value, String> {
        let max_ticks = args["max_ticks"].as_u64().unwrap_or(MAX_SCENARIO_TICKS);
        let deadline = deadline_after(args["timeout_secs"].as_u64().unwrap_or(120));
        let scenario = args
            .get("scenario")
            .filter(|s| s.is_object())
            .ok_or("`scenario` must be an object")?;
        let keep = args["keep_input_when_done"].as_bool().unwrap_or(false);
        let i = self.instance(args)?;
        paused_tick(&i)?; // refuses a running game, so the ticks are exactly the run's
        // The game checks the length and the steps before it touches a running
        // scenario, queues the run's own ticks, takes back the ones a failed
        // check leaves unrun, and releases the input when the run ends.
        let loaded = i.call(
            "input.scenario.load",
            json!({
                "scenario": scenario,
                "max_ticks": max_ticks,
                "queue_ticks": true,
                "keep_input_when_done": keep,
            }),
        )?;
        let duration = loaded["report"]["duration"].as_u64().unwrap_or(0);
        let outcome = (|| loop {
            let status = i.call("input.scenario.status", json!({}))?;
            if !matches!(status["status"].as_str(), Some("pending" | "running")) {
                return Ok(status);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "the scenario ran {} of {duration} ticks within the timeout",
                    status["ticks_run"]
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        })();
        match outcome {
            Ok(report) => Ok(json!({ "passed": report["status"] == "passed", "report": report })),
            Err(e) => {
                // Don't leave the game holding keys because this call gave up.
                let _ = i.call(
                    "input.scenario.cancel",
                    json!({ "reason": "the run_input_scenario call failed" }),
                );
                Err(e)
            }
        }
    }

    fn watch(&self, args: &Value) -> Result<Value, String> {
        let ticks = args["ticks"]
            .as_u64()
            .filter(|t| *t > 0)
            .ok_or("`ticks` must be a positive integer")?;
        let every = match args.get("every") {
            None | Some(Value::Null) => (ticks / 10).max(1),
            Some(v) => v
                .as_u64()
                .filter(|t| *t > 0)
                .ok_or("`every` must be a positive integer")?,
        };
        if ticks / every > MAX_SAMPLES {
            return Err(format!(
                "{} samples is too many (at most {MAX_SAMPLES}); raise `every`",
                ticks / every
            ));
        }
        let names = |key: &str| -> Result<Vec<String>, String> {
            match args.get(key) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(v) => serde_json::from_value(v.clone()).map_err(|_| format!("`{key}` must be a list of names")),
            }
        };
        let (resources, count) = (names("resources")?, names("count")?);
        let deadline = deadline_after(args["timeout_secs"].as_u64().unwrap_or(300));
        let i = self.instance(args)?;
        let mut tick = paused_tick(&i)?;
        let end = tick + ticks;
        let sample = |i: &Instance, info: &Value, ms_per_tick: Value| -> Result<Value, String> {
            let mut out =
                json!({ "tick": info["tick"], "entities": info["entities"], "wall_ms_per_tick": ms_per_tick });
            if !resources.is_empty() {
                let mut values = serde_json::Map::new();
                for r in &resources {
                    values.insert(r.clone(), i.call("resource.get", json!({ "resource": r }))?);
                }
                out["resources"] = Value::Object(values);
            }
            if !count.is_empty() {
                let summary = i.call("world.summary", json!({}))?;
                let mut counts = serde_json::Map::new();
                for c in &count {
                    let n: u64 = summary["archetypes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|a| a["components"].as_array().is_some_and(|cs| cs.iter().any(|n| n == c)))
                        .filter_map(|a| a["entities"].as_u64())
                        .sum();
                    counts.insert(c.clone(), json!(n));
                }
                out["counts"] = Value::Object(counts);
            }
            Ok(out)
        };
        let mut samples = vec![sample(&i, &i.call("info", json!({}))?, Value::Null)?];
        while tick < end {
            let step = every.min(end - tick);
            let begun = Instant::now();
            let info = step_and_wait(&i, tick, step, deadline)?;
            // Wall time includes the polling round trips: an upper bound.
            let ms = begun.elapsed().as_secs_f64() * 1000.0 / step as f64;
            tick = info["tick"].as_u64().unwrap_or(end);
            samples.push(sample(&i, &info, json!((ms * 1000.0).round() / 1000.0))?);
        }
        Ok(json!({ "ticks_run": ticks, "every": every, "samples": samples }))
    }

    fn launch(&mut self, args: &Value) -> Result<Value, String> {
        let flag = |k: &str| args[k].as_bool().unwrap_or(true);
        let cwd = args["cwd"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let program = args["program"].as_str();
        let mut cmd = match program {
            // An executable that is already built: started as it is.
            Some(program) => Command::new(program),
            None => Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())),
        };
        cmd.current_dir(&cwd);
        if program.is_none() {
            cmd.arg("run");
            if flag("release") {
                cmd.arg("--release");
            }
            for (key, opt) in [("package", "-p"), ("example", "--example"), ("bin", "--bin")] {
                if let Some(v) = args[key].as_str() {
                    cmd.args([opt, v]);
                }
            }
            // Release builds honor PONKO_CONTROL only with the engine's
            // `control-env` feature, so a shipped game never opens a port.
            let control_env = if flag("release") {
                control_env_feature(&cwd, args["package"].as_str())
            } else {
                None
            };
            let list = launch_features(args, control_env.as_deref());
            if !list.is_empty() {
                cmd.args(["--features", &list.join(",")]);
            }
        }
        if let Some(extra) = args["args"].as_array() {
            if program.is_none() {
                cmd.arg("--");
            }
            cmd.args(extra.iter().filter_map(Value::as_str));
        }
        let launch_id = random_id();
        cmd.env("PONKO_CONTROL", "1")
            .env("PONKO_LAUNCH_ID", &launch_id)
            .env("PONKO_INSTANCES_DIR", &self.config.instances_dir)
            .env("PONKO_START_PAUSED", if flag("paused") { "1" } else { "0" })
            .env("PONKO_INPUT_INJECTION", "1");
        if flag("headless") {
            cmd.env("HEADLESS", "1");
        }
        if let Some(env) = args["env"].as_object() {
            apply_launch_env(&mut cmd, env);
        }
        std::fs::create_dir_all(&self.config.logs_dir).map_err(|e| format!("can't create the logs dir: {e}"))?;
        let log_path = self.config.logs_dir.join(format!("{launch_id}.log"));
        let log = std::fs::File::create(&log_path).map_err(|e| format!("can't create the log file: {e}"))?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log2)
            .spawn()
            .map_err(|e| format!("could not run {}: {e}", program.unwrap_or("cargo")))?;
        self.launched.insert(launch_id.clone(), log_path.clone());

        let deadline = deadline_after(args["timeout_secs"].as_u64().unwrap_or(600));
        loop {
            if let Some(i) = discover(&self.config.instances_dir)
                .into_iter()
                .find(|i| i.info["launch_id"] == json!(launch_id))
            {
                // Reap the cargo process in the background when it exits.
                std::thread::spawn(move || child.wait());
                let mut v = i.public();
                v["log_file"] = json!(log_path);
                return Ok(v);
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "the game exited ({status}) before its control endpoint opened. Last log lines:\n{}",
                    tail(&log_path, 40)
                ));
            }
            if std::fs::read_to_string(&log_path).is_ok_and(|log| log.contains(CONTROL_ENV_IGNORED)) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the game is a release build that ignores PONKO_CONTROL, so its control endpoint never opens. \
                     Build it with the engine's `control-env` feature (`launch` adds it to its own release builds \
                     when the package depends on `ponko` directly; else pass `features: [\"<dependency>/control-env\"]`), \
                     as a debug build, or have it call `control_endpoint(true)`. Last log lines:\n{}",
                    tail(&log_path, 40)
                ));
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                // Reaped, so a timed-out launch leaves no zombie behind.
                let _ = child.wait();
                return Err(format!(
                    "timed out waiting for the game to start. Last log lines:\n{}",
                    tail(&log_path, 40)
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn read_logs(&self, args: &Value) -> Result<Value, String> {
        let lines = args["lines"].as_u64().unwrap_or(100) as usize;
        let i = self.instance(args)?;
        // The game keeps its own recent output: works for any game, also
        // mid-frame. Older engines don't have `logs.get`; then the log file
        // of a game started with `launch`.
        let mut params = json!({ "lines": lines });
        if let Some(since) = args.get("since").and_then(Value::as_u64) {
            params["since"] = json!(since);
        }
        let kept = i.call("logs.get", params);
        if let Ok(logs) = &kept {
            let text: Vec<&str> = logs["lines"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            return Ok(json!({ "text": text.join("\n"), "next": logs["next"], "missed": logs["missed"] }));
        }
        let Some(id) = i.info["launch_id"].as_str() else {
            return Err(kept.unwrap_err());
        };
        let path = self
            .launched
            .get(id)
            .cloned()
            .unwrap_or_else(|| self.config.logs_dir.join(format!("{id}.log")));
        Ok(json!({ "log_file": path, "text": tail(&path, lines) }))
    }
}

fn tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Runs the server on stdin/stdout until stdin closes.
///
/// ```no_run
/// ponko_mcp::serve_stdio(ponko_mcp::Config::from_env()).unwrap();
/// ```
pub fn serve_stdio(config: Config) -> std::io::Result<()> {
    let mut server = Server::new(config);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } }),
            ),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Standard base64 with padding (what MCP image content uses).
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Most samples one `watch` call returns.
const MAX_SAMPLES: u64 = 1000;
/// The longest a tool call may wait, whatever `timeout_secs` asks for (a year).
const MAX_WAIT_SECS: u64 = 365 * 24 * 3600;

/// When a call that may wait `secs` seconds must give up. Huge values are
/// clamped: adding them to `Instant::now()` would overflow and panic.
fn deadline_after(secs: u64) -> Instant {
    let now = Instant::now();
    now.checked_add(Duration::from_secs(secs.min(MAX_WAIT_SECS)))
        .unwrap_or(now)
}
/// The longest scenario `run_input_scenario` accepts unless told otherwise.
const MAX_SCENARIO_TICKS: u64 = 100_000;

/// The current tick of a paused game; an error if it's running, since
/// "exactly N ticks" can't hold then.
fn paused_tick(i: &Instance) -> Result<u64, String> {
    let info = i.call("info", json!({}))?;
    if info["paused"] != json!(true) {
        return Err("the simulation is running; call `pause` first so exactly N ticks run".into());
    }
    Ok(info["tick"].as_u64().unwrap_or(0))
}

/// Steps a paused game `ticks` ticks from `start` and waits for them; returns its `info`.
fn step_and_wait(i: &Instance, start: u64, ticks: u64, deadline: Instant) -> Result<Value, String> {
    i.call("sim.step", json!({ "ticks": ticks }))?;
    loop {
        let info = i.call("info", json!({}))?;
        let now = info["tick"].as_u64().unwrap_or(0);
        if now >= start + ticks {
            return Ok(info);
        }
        if Instant::now() > deadline {
            return Err(format!("only {} of {ticks} ticks ran within the timeout", now - start));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_timeout_is_clamped_instead_of_overflowing_the_clock() {
        let now = Instant::now();
        for secs in [0, 1, MAX_WAIT_SECS, u64::MAX] {
            let deadline = deadline_after(secs);
            assert!(deadline >= now);
            assert!(deadline <= now + Duration::from_secs(MAX_WAIT_SECS + 5));
        }
    }

    #[test]
    fn base64_matches_the_rfc_examples() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), want);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn discovery_removes_dead_entries_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("ponko-mcp-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Port 1 is never listening: a crashed game's entry.
        let dead = dir.join("4242-1.json");
        std::fs::write(&dead, r#"{"pid": 4242, "port": 1, "token": "t", "name": "gone"}"#).unwrap();
        let mismatch = dir.join("4242-2.json"); // name and content disagree: not ours
        std::fs::write(&mismatch, r#"{"pid": 1, "port": 1, "token": "t"}"#).unwrap();
        let save = dir.join("checkpoint.json");
        std::fs::write(&save, r#"{"format": "crabby-world"}"#).unwrap();
        let note = dir.join("notes.txt");
        std::fs::write(&note, "hi").unwrap();

        assert!(discover(&dir).is_empty());
        assert!(!dead.exists(), "dead registry entries are cleaned up");
        assert!(
            mismatch.exists() && save.exists() && note.exists(),
            "other files are left alone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tools_that_replace_state_or_write_files_are_destructive() {
        let tools = tools();
        for name in [
            "launch",
            "load_world",
            "load_scene",
            "save_scene",
            "save_prefab",
            "despawn_entity",
            "remove_component",
            "quit_instance",
            "delete_entities",
            "save_edited_scene",
        ] {
            let t = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("no tool {name}"));
            let schema = tool_schema(t);
            assert_eq!(schema["annotations"]["destructiveHint"], true, "{name}");
            assert_eq!(schema["annotations"]["readOnlyHint"], false, "{name}");
        }
        // Read-only tools leave the game as it was (save_world and
        // screenshot only add files), and MCP ignores the hint on them.
        for t in &tools {
            if !t.writes {
                assert!(!destructive(t.name), "{} is read-only but marked destructive", t.name);
            }
        }
        assert_eq!(
            tool_schema(tools.iter().find(|t| t.name == "spawn_entity").unwrap())["annotations"]["destructiveHint"],
            false
        );
    }

    #[test]
    fn release_launches_add_control_env_once() {
        let args = json!({ "features": ["physics"] });
        assert_eq!(
            launch_features(&args, Some("engine/control-env")),
            ["physics", "engine/control-env"]
        );
        assert_eq!(launch_features(&args, None), ["physics"]);
        assert_eq!(
            launch_features(&json!({}), Some("ponko/control-env")),
            ["ponko/control-env"]
        );
        // Any spelling the caller already chose is kept, not doubled.
        for own in ["engine/control-env", "ponko/control-env", "control-env"] {
            let args = json!({ "features": [own] });
            assert_eq!(launch_features(&args, Some("ponko/control-env")), [own]);
        }
        assert!(crate::CONTROL_ENV_IGNORED.starts_with("ponko: PONKO_CONTROL"));
    }

    #[test]
    fn the_control_env_feature_uses_the_games_name_for_the_engine() {
        let metadata = json!({ "packages": [
            { "name": "ponko", "manifest_path": "/w/Cargo.toml", "dependencies": [] },
            { "name": "renamed", "manifest_path": "/w/games/renamed/Cargo.toml", "dependencies": [
                { "name": "serde", "rename": null, "kind": null },
                { "name": "ponko", "rename": "engine", "kind": null },
            ] },
            { "name": "plain", "manifest_path": "/w/games/plain/Cargo.toml", "dependencies": [
                { "name": "ponko", "rename": null, "kind": "dev" },
            ] },
            { "name": "both", "manifest_path": "/w/games/both/Cargo.toml", "dependencies": [
                { "name": "ponko", "rename": "dev-engine", "kind": "dev" },
                { "name": "ponko", "rename": "engine", "kind": null },
            ] },
            { "name": "indirect", "manifest_path": "/w/games/indirect/Cargo.toml", "dependencies": [
                { "name": "my-framework", "rename": null, "kind": null },
                { "name": "ponko", "rename": null, "kind": "build" },
            ] },
        ] });
        let feature = |cwd: &str, package: Option<&str>| control_env_feature_in(&metadata, Path::new(cwd), package);
        assert_eq!(feature("/w", None).as_deref(), Some("ponko/control-env"));
        assert_eq!(
            feature("/w/games/renamed/src", None).as_deref(),
            Some("engine/control-env")
        );
        assert_eq!(feature("/w", Some("renamed")).as_deref(), Some("engine/control-env"));
        // A dev-dependency alone doesn't count; a normal one wins over it.
        assert_eq!(feature("/w/games/plain", None), None);
        assert_eq!(feature("/w/games/both", None).as_deref(), Some("engine/control-env"));
        // No direct dependency (only through another crate, or a build
        // script's): no feature.
        assert_eq!(feature("/w/games/indirect", None), None);
        assert_eq!(feature("/w", Some("missing")), None);
        assert_eq!(feature("/elsewhere", None), None);
    }

    #[test]
    fn a_renamed_engine_dependency_is_found_with_cargo_metadata() {
        let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let game = std::env::temp_dir().join(format!("ponko-mcp-renamed-{}", random_id()));
        std::fs::create_dir_all(game.join("src")).unwrap();
        std::fs::write(
            game.join("Cargo.toml"),
            format!(
                "[package]\nname = \"my-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
                 [dependencies]\nengine = {{ package = \"ponko\", path = {:?} }}\n\n[workspace]\n",
                // Cargo doesn't take Windows' verbatim `\\?\` prefix,
                // which canonicalize adds.
                engine.to_str().unwrap().trim_start_matches(r"\\?\")
            ),
        )
        .unwrap();
        std::fs::write(game.join("src/main.rs"), "fn main() {}\n").unwrap();
        let feature = control_env_feature(&game, None);
        let by_name = control_env_feature(&game, Some("my-game"));
        std::fs::remove_dir_all(&game).unwrap();
        assert_eq!(feature.as_deref(), Some("engine/control-env"));
        assert_eq!(by_name.as_deref(), Some("engine/control-env"));
        // The engine's own folder.
        assert_eq!(control_env_feature(&engine, None).as_deref(), Some("ponko/control-env"));
    }

    #[test]
    fn launch_overrides_replace_the_launch_defaults_and_nothing_else() {
        let mut cmd = Command::new("game");
        cmd.env("PONKO_START_PAUSED", "1");
        apply_launch_env(
            &mut cmd,
            json!({"PONKO_START_PAUSED": "0", "UNITS": 400}).as_object().unwrap(),
        );
        let env: Vec<_> = cmd.get_envs().collect();
        let get = |key: &str| env.iter().find(|(name, _)| *name == key).and_then(|(_, v)| *v);
        assert_eq!(get("PONKO_START_PAUSED"), Some(std::ffi::OsStr::new("0")));
        assert_eq!(get("UNITS"), Some(std::ffi::OsStr::new("400")));
        assert_eq!(env.len(), 2, "no other spellings are added: {env:?}");
    }

    #[test]
    fn every_tool_names_the_methods_it_needs() {
        for t in tools() {
            assert!(tool_methods(t.name).is_some(), "add `{}` to tool_methods", t.name);
        }
        assert!(tool_methods("nope").is_none());
    }

    #[test]
    fn tool_availability_follows_the_game_and_the_server_mode() {
        let methods = vec![
            json!({ "name": "info", "available": true }),
            json!({ "name": "screenshot", "available": false, "reason": "headless games have no window" }),
            json!({ "name": "entity.spawn", "available": true }),
        ];
        let find = |list: &[Value], name: &str| list.iter().find(|t| t["name"] == name).unwrap().clone();
        let all = tool_availability(&methods, false);
        assert_eq!(all.len(), tools().len());
        assert_eq!(find(&all, "instance_info")["available"], true);
        assert_eq!(find(&all, "list_instances")["available"], true, "needs no game method");
        let shot = find(&all, "screenshot");
        assert_eq!(
            (shot["available"].clone(), shot["reason"].clone()),
            (json!(false), json!("headless games have no window"))
        );
        let missing = find(&all, "save_world");
        assert_eq!(missing["available"], false);
        assert!(
            missing["reason"].as_str().unwrap().contains("no `world.save`"),
            "{missing}"
        );
        assert_eq!(find(&all, "spawn_entity")["available"], true);
        let read_only = tool_availability(&methods, true);
        let spawn = find(&read_only, "spawn_entity");
        assert_eq!(spawn["available"], false);
        assert!(spawn["reason"].as_str().unwrap().contains("read-only"));
        assert_eq!(find(&read_only, "instance_info")["available"], true);
    }

    /// An engine from before `capabilities`: knows `methods` only.
    #[test]
    fn older_engines_report_capabilities_as_unknown() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fake = std::thread::spawn(move || {
            let mut answered = 0;
            while answered < 2 {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                // Discovery connects just to see the game is alive.
                if BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap_or(0)
                    == 0
                {
                    continue;
                }
                answered += 1;
                let req: Value = serde_json::from_str(&line).unwrap();
                let reply = match req["method"].as_str().unwrap() {
                    "methods" => json!({ "id": 1, "result": [
                        { "name": "info", "about": "status" },
                        { "name": "screenshot", "about": "a frame" },
                    ] }),
                    m => json!({ "id": 1, "error": format!("unknown method `{m}` (call `methods` for the list)") }),
                };
                writeln!(&stream, "{reply}").unwrap();
            }
        });
        let dir = std::env::temp_dir().join(format!("ponko-mcp-old-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pid = std::process::id();
        let entry = json!({ "name": "old", "pid": pid, "port": port, "token": "t", "headless": true, "engine_version": "0.0.0" });
        std::fs::write(dir.join(format!("{pid}-{port}.json")), entry.to_string()).unwrap();
        let server = Server::new(Config {
            instances_dir: dir.clone(),
            logs_dir: dir.join("logs"),
            read_only: false,
        });
        let caps = server.capabilities(&json!({})).unwrap();
        fake.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            (caps["supported"].clone(), caps["version"].clone()),
            (json!(false), Value::Null)
        );
        assert!(caps["reason"].as_str().unwrap().contains("predates"));
        assert_eq!(
            (caps["mode"].clone(), caps["engine_version"].clone()),
            (json!("headless"), json!("0.0.0"))
        );
        assert!(
            caps["methods"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["available"].is_null())
        );
        let tools = caps["mcp"]["tools"].as_array().unwrap();
        let find = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap();
        assert!(find("screenshot")["available"].is_null(), "present but unknown");
        assert_eq!(find("save_world")["available"], false, "absent method");
        assert_eq!(find("list_instances")["available"], true);
    }

    #[test]
    fn registry_names_parse() {
        assert_eq!(registry_name(Path::new("/x/123-4567.json")), Some((123, 4567)));
        assert_eq!(registry_name(Path::new("/x/checkpoint.json")), None);
        assert_eq!(registry_name(Path::new("/x/1-99999.json")), None); // not a port
        assert_eq!(registry_name(Path::new("/x/1-2.txt")), None);
    }
}
