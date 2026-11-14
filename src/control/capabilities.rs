//! `capabilities`: what this running instance can do, so tools can check
//! before sending a request it would refuse.
//!
//! Everything here is read from the build (`cfg!(feature)`) or the live
//! [`Ctx`]; nothing is guessed. When something can't be known it says so
//! (`null` plus a reason) instead of claiming support.

use serde_json::{Value, json};

use super::METHODS;
use crate::ctx::Ctx;

/// Bumped when a field changes meaning or goes away (additions don't bump it).
pub(super) const VERSION: u64 = 1;

/// Every Cargo feature of the engine and whether this build has it. A test
/// checks this against `Cargo.toml`, so a new feature can't be forgotten.
pub(super) const FEATURES: &[(&str, bool)] = &[
    ("window", cfg!(feature = "window")),
    ("audio", cfg!(feature = "audio")),
    ("inspector", cfg!(feature = "inspector")),
    ("control-env", cfg!(feature = "control-env")),
    ("gamepad", cfg!(feature = "gamepad")),
    ("profiling", cfg!(feature = "profiling")),
    ("hotpatch", cfg!(feature = "hotpatch")),
    ("physics", cfg!(feature = "physics")),
    ("physics3d", cfg!(feature = "physics3d")),
    ("physics-determinism", cfg!(feature = "physics-determinism")),
    ("steam", cfg!(feature = "steam")),
    ("3d", cfg!(feature = "3d")),
    ("navmesh", cfg!(feature = "navmesh")),
    ("accessibility", cfg!(feature = "accessibility")),
    ("bench-internals", cfg!(feature = "bench-internals")),
    ("compressed-textures", cfg!(feature = "compressed-textures")),
    ("basis-universal", cfg!(feature = "basis-universal")),
    ("image-jpeg", cfg!(feature = "image-jpeg")),
    ("image-webp", cfg!(feature = "image-webp")),
    ("image-bmp", cfg!(feature = "image-bmp")),
    ("image-tga", cfg!(feature = "image-tga")),
    ("image-gif", cfg!(feature = "image-gif")),
    ("image-tiff", cfg!(feature = "image-tiff")),
    ("image-ico", cfg!(feature = "image-ico")),
    ("image-qoi", cfg!(feature = "image-qoi")),
    ("image-pnm", cfg!(feature = "image-pnm")),
    ("image-dds", cfg!(feature = "image-dds")),
    ("image-hdr", cfg!(feature = "image-hdr")),
    ("image-exr", cfg!(feature = "image-exr")),
    ("image-formats", cfg!(feature = "image-formats")),
    ("text-shaping", cfg!(feature = "text-shaping")),
    ("system-fonts", cfg!(feature = "system-fonts")),
];

/// Why `method` would be refused by this instance right now, if it would.
/// `handle` checks this first, so the report and the behavior agree.
pub(super) fn unavailable(ctx: &Ctx, method: &str) -> Option<String> {
    match method {
        "screenshot" if ctx.headless => {
            Some("headless games have no window to capture; launch the game with a window".into())
        }
        "perf.trace" if ctx.headless => Some("headless games have no frames to trace".into()),
        "input.scenario.load" if !ctx.input.injection_allowed() => Some(
            "input injection is off: start the game with PONKO_INPUT_INJECTION=1 or call Game::input_injection(true)"
                .into(),
        ),
        #[cfg(not(feature = "3d"))]
        "quality.set" => Some(super::NO_QUALITY.into()),
        // Asks for no lines: errors only when output isn't captured.
        "logs.get" => crate::logs::get(None, 0).err(),
        _ => None,
    }
}

/// Accepted, but with less effect than in a windowed game.
fn limited(ctx: &Ctx, method: &str) -> Option<&'static str> {
    match method {
        "perf.hud" if ctx.headless => Some("accepted, but a headless game draws no overlay"),
        #[cfg(feature = "3d")]
        "quality.set" if ctx.headless => Some(
            "accepted (the settings change and perf.get reports them), but a headless game draws nothing, and `recommended` needs a GPU",
        ),
        "perf.get" if ctx.headless => Some(
            "headless games report only simulation system timings, no frame or GPU numbers (render_inspector marks them unavailable)",
        ),
        _ => None,
    }
}

/// Whether simulation system `name` (a `type_name`) is scheduled. `None`
/// before the game loop has listed its systems (no frame or tick yet).
#[cfg(any(feature = "physics", feature = "physics3d"))]
fn scheduled(ctx: &Ctx, name: &str) -> Option<bool> {
    let t = ctx.perf.system_timings();
    (!t.is_empty()).then(|| t.iter().any(|s| s.sim && s.name == name))
}

/// A subsystem behind a feature with a scheduled step system.
#[cfg(any(feature = "physics", feature = "physics3d"))]
fn stepped(ctx: &Ctx, feature: &str, name: &str) -> Value {
    match scheduled(ctx, name) {
        Some(on) => json!({ "feature": feature, "compiled": true, "active": on }),
        None => json!({
            "feature": feature,
            "compiled": true,
            "active": null,
            "reason": "the game loop hasn't started yet",
        }),
    }
}

fn compiled_out(feature: &str) -> Value {
    json!({
        "feature": feature,
        "compiled": false,
        "active": false,
        "reason": format!("built without the `{feature}` feature"),
    })
}

fn subsystems(ctx: &Ctx, gpu: Option<&crate::render::GpuCapabilities>) -> Value {
    let windowed = !ctx.headless;
    let renderer = if !cfg!(feature = "window") {
        compiled_out("window")
    } else if !windowed {
        json!({ "feature": "window", "compiled": true, "active": false, "reason": "running headless" })
    } else {
        json!({
            "feature": "window",
            "compiled": true,
            "active": true,
            "3d": cfg!(feature = "3d"),
            "adapter": gpu.map(|g| &g.adapter),
            "gpu": gpu,
            "adapter_reason": if gpu.is_none() { Some("the renderer has not opened a GPU device") } else { None },
        })
    };
    let windowed_only = |feature: &str, compiled: bool| {
        if !compiled {
            compiled_out(feature)
        } else if !windowed {
            json!({ "feature": feature, "compiled": true, "active": false, "reason": "running headless" })
        } else {
            json!({ "feature": feature, "compiled": true, "active": true })
        }
    };
    #[cfg(feature = "physics")]
    let physics = stepped(
        ctx,
        "physics",
        std::any::type_name_of_val(&crate::systems::step_physics),
    );
    #[cfg(not(feature = "physics"))]
    let physics = compiled_out("physics");
    #[cfg(feature = "physics3d")]
    let physics3d = stepped(
        ctx,
        "physics3d",
        std::any::type_name_of_val(&crate::systems::step_physics3d),
    );
    #[cfg(not(feature = "physics3d"))]
    let physics3d = compiled_out("physics3d");
    let audio = if cfg!(feature = "audio") {
        json!({
            "feature": "audio",
            "compiled": true,
            // Opens on the first sound; headless games may never open it.
            "device": ctx.audio.backend.device_state(),
        })
    } else {
        compiled_out("audio")
    };
    let net = &ctx.net;
    json!({
        "renderer": renderer,
        "inspector": windowed_only("inspector", cfg!(feature = "inspector")),
        "gamepad": windowed_only("gamepad", cfg!(feature = "gamepad")),
        "physics": physics,
        "physics3d": physics3d,
        "audio": audio,
        "networking": {
            "state": format!("{:?}", net.state()).to_lowercase(),
            "players": net.players(),
            "transports": { "udp": true, "steam": cfg!(feature = "steam") },
        },
        "profiling": {
            "scope_timing": ctx.perf.profiling(),
            "puffin_feature": cfg!(feature = "profiling"),
        },
        // Whoever reads this reached it through the endpoint.
        "control": { "active": true },
    })
}

/// The `capabilities` answer.
pub(super) fn report(ctx: &Ctx) -> Value {
    report_with_gpu(ctx, ctx.gfx.gpu_capabilities())
}

fn report_with_gpu(ctx: &Ctx, gpu: Option<&crate::render::GpuCapabilities>) -> Value {
    let methods: Vec<Value> = METHODS
        .iter()
        .map(|(name, about)| {
            let mut m = json!({ "name": name, "about": about, "available": true });
            if let Some(reason) = unavailable(ctx, name) {
                m["available"] = json!(false);
                m["reason"] = json!(reason);
            } else if let Some(note) = limited(ctx, name) {
                m["limited"] = json!(note);
            }
            m
        })
        .collect();
    let (components, resources) = crate::reflect::reflected_schemas(&ctx.world);
    json!({
        "format": "ponko-capabilities",
        "version": VERSION,
        "engine_version": env!("CARGO_PKG_VERSION"),
        "mode": if ctx.headless { "headless" } else { "windowed" },
        "features": FEATURES.iter().map(|&(f, on)| (f.to_string(), json!(on))).collect::<serde_json::Map<_, _>>(),
        "subsystems": subsystems(ctx, gpu),
        "methods": methods,
        "reflection": {
            "components": components,
            "resources": resources,
            "events": ctx.events.reflected_json(&ctx.world),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::WorldReflect;
    use ponko_ecs::Component;

    fn ctx(headless: bool) -> Ctx {
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::ZERO);
        ctx.set_headless(headless);
        ctx
    }

    fn method<'a>(report: &'a Value, name: &str) -> &'a Value {
        report["methods"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing"))
    }

    #[test]
    fn headless_and_windowed_instances_report_differently_without_mutating() {
        let mut headless = ctx(true);
        let mut windowed = ctx(false);
        let h = super::super::handle(&mut headless, "capabilities", &json!({})).unwrap();
        let w = super::super::handle(&mut windowed, "capabilities", &json!({})).unwrap();
        assert_eq!(
            (h["format"].clone(), h["version"].clone()),
            (json!("ponko-capabilities"), json!(VERSION))
        );
        assert_eq!(
            (h["mode"].clone(), w["mode"].clone()),
            (json!("headless"), json!("windowed"))
        );

        assert_eq!(h["subsystems"]["renderer"]["active"], false);
        assert!(h["subsystems"]["renderer"]["reason"].is_string());
        if cfg!(feature = "window") {
            assert_eq!(w["subsystems"]["renderer"]["active"], true);
            assert_eq!(w["subsystems"]["renderer"]["3d"], cfg!(feature = "3d"));
            // Unknown, and said so, rather than guessed.
            assert!(w["subsystems"]["renderer"]["adapter"].is_null());
            assert!(w["subsystems"]["renderer"]["adapter_reason"].is_string());
        }
        for name in ["screenshot", "perf.trace"] {
            assert_eq!(method(&h, name)["available"], false, "{name}");
            assert!(method(&h, name)["reason"].as_str().unwrap().contains("headless"));
            assert_eq!(method(&w, name)["available"], true, "{name}");
        }
        assert!(method(&h, "perf.hud")["limited"].is_string());
        assert!(method(&w, "perf.hud").get("limited").is_none());
        assert_eq!(method(&w, "capabilities")["available"], true);
        // Reading capabilities changes nothing.
        assert_eq!(
            (
                headless.world.len(),
                headless.time.sim_tick(),
                headless.time.is_paused()
            ),
            (0, 0, false)
        );
    }

    #[test]
    fn every_method_is_reported_and_refusals_match_the_endpoint() {
        for headless in [true, false] {
            let mut ctx = ctx(headless);
            let report = report(&ctx);
            let listed = report["methods"].as_array().unwrap();
            assert_eq!(listed.len(), METHODS.len());
            for m in listed {
                let name = m["name"].as_str().unwrap();
                if m["available"] == false {
                    let refused = super::super::handle(&mut ctx, name, &json!({})).unwrap_err();
                    assert_eq!(json!(refused), m["reason"], "{name}");
                } else {
                    assert!(m.get("reason").is_none(), "{name}");
                }
            }
        }
    }

    /// The method names `handle` matches on, read from its source.
    fn handled_methods() -> Vec<&'static str> {
        // Line by line, so a checkout with CRLF endings (Windows) reads the
        // same: from the `handle` line to the first line that is just `}`.
        let methods: Vec<_> = include_str!("../control.rs")
            .lines()
            .skip_while(|line| !line.starts_with("pub(crate) fn handle("))
            .take_while(|line| line.trim_end() != "}")
            .filter_map(|line| {
                let (name, rest) = line.trim_start().strip_prefix('"')?.split_once('"')?;
                rest.trim_start().starts_with("=>").then_some(name)
            })
            .collect();
        assert!(!methods.is_empty(), "no methods found in control.rs's handle");
        methods
    }

    #[test]
    fn every_handled_method_is_listed_and_every_listed_one_handled() {
        let handled = handled_methods();
        assert!(handled.len() > 30, "the source scan found too few arms: {handled:?}");
        for name in &handled {
            assert!(
                METHODS.iter().any(|(m, _)| m == name),
                "`{name}` is handled but not in METHODS, so `methods` and `capabilities` don't list it"
            );
        }
        for (name, _) in METHODS {
            assert!(handled.contains(name), "`{name}` is listed but has no arm in handle");
        }
    }

    #[test]
    fn every_method_reported_available_answers_in_both_modes() {
        for headless in [true, false] {
            let mut ctx = ctx(headless);
            let report = report(&ctx);
            // In METHODS order: events.subscribe comes before events.take.
            for m in report["methods"].as_array().unwrap() {
                let name = m["name"].as_str().unwrap();
                if m["available"] == false {
                    continue;
                }
                let Err(e) = super::super::handle(&mut ctx, name, &json!({})) else {
                    continue;
                };
                // A windowed screenshot is answered after the next frame.
                let deferred = name == "screenshot" && !headless;
                assert!(
                    e.starts_with("missing param") || deferred,
                    "{name} is reported available (headless: {headless}) but refused an empty call: {e}"
                );
            }
        }
    }

    #[test]
    fn feature_list_matches_cargo_toml() {
        let toml = include_str!("../../Cargo.toml");
        let section = &toml[toml.find("[features]").unwrap() + "[features]".len()..];
        let section = &section[..section.find("\n[").unwrap()];
        let mut declared: Vec<&str> = section
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim()))
            .filter(|k| !k.is_empty() && *k != "default")
            .collect();
        declared.sort_unstable();
        let mut reported: Vec<&str> = FEATURES.iter().map(|(f, _)| *f).collect();
        reported.sort_unstable();
        assert_eq!(declared, reported, "update capabilities::FEATURES");
        let r = report(&ctx(true));
        assert_eq!(r["features"]["window"], cfg!(feature = "window"));
        assert_eq!(r["features"]["steam"], cfg!(feature = "steam"));
    }

    #[test]
    fn off_subsystems_say_why() {
        let r = report(&ctx(true));
        let s = &r["subsystems"];
        for name in ["renderer", "inspector", "gamepad", "physics", "physics3d"] {
            if s[name]["active"] == false {
                assert!(s[name]["reason"].is_string(), "{name}: {}", s[name]);
            }
        }
        if !cfg!(feature = "physics") {
            assert_eq!(s["physics"]["compiled"], false);
            assert!(s["physics"]["reason"].as_str().unwrap().contains("`physics`"));
        }
        if cfg!(feature = "audio") {
            assert_eq!(
                s["audio"]["device"], "not_opened",
                "reading capabilities opens no device"
            );
        } else {
            assert_eq!(s["audio"]["compiled"], false);
        }
        assert_eq!(s["networking"]["state"], "offline");
        assert_eq!(s["control"]["active"], true);
    }

    #[cfg(feature = "physics")]
    #[test]
    fn physics_is_active_only_when_its_step_is_scheduled() {
        let mut ctx = ctx(true);
        assert!(
            report(&ctx)["subsystems"]["physics"]["active"].is_null(),
            "unknown before the loop runs"
        );
        let timing = |name| crate::perf::SystemTiming {
            name,
            sim: true,
            last_ms: 0.0,
            avg_ms: 0.0,
        };
        ctx.perf.timings = vec![timing("game::tick")];
        assert_eq!(report(&ctx)["subsystems"]["physics"]["active"], false);
        ctx.perf
            .timings
            .push(timing(std::any::type_name_of_val(&crate::systems::step_physics)));
        assert_eq!(report(&ctx)["subsystems"]["physics"]["active"], true);
    }

    #[derive(serde::Serialize, serde::Deserialize, Component)]
    struct Health {
        current: f32,
        max: f32,
    }

    /// Serialized as a plain string, whatever its fields are.
    #[derive(Component)]
    struct Tag {
        id: u32,
    }
    impl serde::Serialize for Tag {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(&format!("tag-{}", self.id))
        }
    }
    impl<'de> serde::Deserialize<'de> for Tag {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            let id = s.trim_start_matches("tag-").parse().map_err(serde::de::Error::custom)?;
            Ok(Self { id })
        }
    }

    #[test]
    fn reflected_types_report_described_field_and_opaque_schemas() {
        let mut ctx = ctx(true);
        ctx.world.reflect::<Health>();
        ctx.world.reflect::<Tag>();
        ctx.world.spawn((Tag { id: 7 },));
        ctx.world
            .describe_component(
                "Health",
                crate::reflect::ValueSchema::object([
                    ("current", crate::reflect::ValueSchema::Number),
                    ("max", crate::reflect::ValueSchema::Number),
                ]),
            )
            .unwrap();
        // SAFETY: plain bytes; any f32 is a valid value.
        let id = unsafe { ctx.world.raw().register_component("CHealth", 4, 4, None) };
        // SAFETY: as above.
        unsafe {
            ctx.world.reflect_fields(
                id,
                &[crate::reflect::Field::new("hp", crate::reflect::FieldType::F32, 0)],
            );
        }
        let r = crate::control::handle(&mut ctx, "capabilities", &json!({})).unwrap();
        let comp = |name: &str| {
            r["reflection"]["components"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == name)
                .unwrap()["schema"]
                .clone()
        };
        let health = comp("Health");
        assert_eq!(
            (health["status"].clone(), health["source"].clone()),
            (json!("described"), json!("described"))
        );
        assert_eq!(health["json_schema"]["properties"]["max"]["type"], "number");
        // A custom serializer: the shape isn't guessed from a live value.
        let tag = comp("Tag");
        assert_eq!(tag["status"], "opaque");
        assert!(tag["reason"].is_string() && tag.get("json_schema").is_none());
        let c = comp("CHealth");
        assert_eq!(
            (c["status"].clone(), c["source"].clone()),
            (json!("described"), json!("fields"))
        );
        assert_eq!(c["json_schema"]["properties"]["hp"]["type"], "number");
        let t = comp("Transform2D");
        assert_eq!(t["json_schema"]["properties"]["position"]["maxItems"], 2);
    }

    #[test]
    fn describing_an_unreflected_type_is_an_error() {
        let mut ctx = ctx(true);
        let e = ctx
            .world
            .describe_component("Nope", crate::reflect::ValueSchema::Number);
        assert!(matches!(e, Err(crate::reflect::ReflectError::UnknownComponent(_))));
        let e = ctx.world.describe_resource("Nope", crate::reflect::ValueSchema::Number);
        assert!(matches!(e, Err(crate::reflect::ReflectError::UnknownResource(_))));
    }
}

#[cfg(test)]
#[path = "capabilities_completion_tests.rs"]
mod completion_tests;
