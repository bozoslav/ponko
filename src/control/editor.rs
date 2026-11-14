//! The editor's methods (`editor.*`): every edit a person makes in the
//! inspector, for agents, with the same undo and redo history (see
//! `crate::editor`). The 3D ones (picking, the gizmo, the camera, materials)
//! need the `3d` feature; `capabilities` reports them unavailable without it.

use serde_json::{Value, json};

use super::{entity_param, param, str_param};
use crate::ctx::Ctx;
#[cfg(feature = "3d")]
use crate::reflect::WorldReflect;
use ponko_ecs::Entity;

/// Why the 3D methods are refused in builds without 3D.
#[cfg_attr(feature = "3d", allow(dead_code))]
pub(super) const NO_3D: &str = "3D scene editing needs the `3d` feature";

/// The `entities` param: a list of entity ids, all alive.
fn entities_param(ctx: &Ctx, params: &Value) -> Result<Vec<Entity>, String> {
    let list = param(params, "entities")?
        .as_array()
        .ok_or("`entities` must be a list of entity ids")?;
    if list.is_empty() {
        return Err("`entities` is empty".into());
    }
    list.iter()
        .map(|v| {
            let e = v
                .as_u64()
                .map(Entity::from_bits)
                .ok_or("`entities` must be a list of entity ids (non-negative integers)")?;
            if ctx.world.is_alive(e) {
                Ok(e)
            } else {
                Err(format!("no entity {}", e.to_bits()))
            }
        })
        .collect()
}

/// An optional list of numbers of length `n`.
fn floats<const N: usize>(params: &Value, name: &str) -> Result<Option<[f32; N]>, String> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let bad = || format!("`{name}` must be a list of {N} numbers");
            let list = v.as_array().filter(|a| a.len() == N).ok_or_else(bad)?;
            let mut out = [0.0; N];
            for (o, x) in out.iter_mut().zip(list) {
                *o = x.as_f64().ok_or_else(bad)? as f32;
            }
            Ok(Some(out))
        }
    }
}

/// What the history can do now, for every editing reply.
fn history(ctx: &Ctx) -> Value {
    json!({ "can_undo": ctx.editor.can_undo(), "can_redo": ctx.editor.can_redo() })
}

pub(super) fn undo(ctx: &mut Ctx, redo: bool) -> Result<Value, String> {
    let done = ctx.edit(|ed, ctx| if redo { ed.redo(ctx) } else { ed.undo(ctx) })?;
    let mut out = history(ctx);
    out["done"] = json!(done);
    Ok(out)
}

pub(super) fn parent(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    let child = entity_param(params)?;
    let parent = match params.get("parent") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .map(Entity::from_bits)
                .ok_or("`parent` must be an entity id or null")?,
        ),
    };
    ctx.edit(|ed, ctx| ed.set_parent(ctx, child, parent))?;
    let mut out = history(ctx);
    out["entity"] = json!(child.to_bits());
    out["parent"] = json!(parent.map(Entity::to_bits));
    Ok(out)
}

pub(super) fn delete(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    let roots = entities_param(ctx, params)?;
    let before = ctx.world.len();
    ctx.edit(|ed, ctx| ed.delete_many(ctx, &roots))?;
    let mut out = history(ctx);
    out["despawned"] = json!(before - ctx.world.len());
    Ok(out)
}

pub(super) fn duplicate(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    let roots = entities_param(ctx, params)?;
    let (offset2, offset3) = match params.get("offset") {
        None | Some(Value::Null) => (glam::Vec2::ZERO, crate::editor::Offset3::ZERO),
        Some(Value::Array(a)) if a.len() == 2 => {
            let [x, y] = floats::<2>(params, "offset")?.unwrap_or_default();
            (glam::Vec2::new(x, y), crate::editor::Offset3::ZERO)
        }
        Some(_) => {
            let o = floats::<3>(params, "offset")?.unwrap_or_default();
            #[cfg(feature = "3d")]
            let three = crate::editor::Offset3(glam::Vec3::from(o));
            #[cfg(not(feature = "3d"))]
            let three = {
                let _ = o;
                crate::editor::Offset3::ZERO
            };
            (glam::Vec2::ZERO, three)
        }
    };
    let copies = ctx.edit(|ed, ctx| ed.duplicate_many(ctx, &roots, offset2, offset3))?;
    let mut out = history(ctx);
    out["entities"] = json!(copies.iter().map(|e| e.to_bits()).collect::<Vec<_>>());
    Ok(out)
}

pub(super) fn hierarchy(ctx: &Ctx) -> Value {
    let nodes = crate::editor::hierarchy(&ctx.world);
    let mut depth: std::collections::HashMap<Entity, usize> = std::collections::HashMap::new();
    let list: Vec<Value> = nodes
        .iter()
        .map(|n| {
            let d = n.parent.and_then(|(p, _)| depth.get(&p)).map_or(0, |d| d + 1);
            depth.insert(n.entity, d);
            let components: Vec<String> = ctx
                .world
                .components_of(n.entity)
                .map(|ids| {
                    ids.iter()
                        .map(|&id| crate::reflect::short_name(ctx.world.component_info(id).name()))
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "entity": n.entity.to_bits(),
                "parent": n.parent.map(|(p, _)| p.to_bits()),
                "link": n.parent.map(|(_, l)| l.name()),
                "depth": d,
                "prefab": n.prefab,
                "children": n.children.iter().map(|c| c.to_bits()).collect::<Vec<_>>(),
                "components": components,
            })
        })
        .collect();
    json!({ "nodes": list })
}

pub(super) fn assets(ctx: &Ctx) -> Value {
    let a = crate::editor::browse(ctx);
    json!({
        "meshes": a.meshes,
        "prefabs": a.prefabs,
        "scenes": a.scenes,
        "materials": a
            .materials
            .iter()
            .map(|(mesh, index, name)| json!({ "mesh": mesh, "index": index, "name": name }))
            .collect::<Vec<_>>(),
    })
}

pub(super) fn save(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    let path = match params.get("path") {
        None | Some(Value::Null) => ctx
            .scene_file
            .clone()
            .ok_or("missing param `path` (no scene was loaded or saved yet, so there is no file to save back to)")?,
        Some(_) => super::asset_path_param(params)?.to_string(),
    };
    ctx.assets.save_scene(&ctx.world, &path)?;
    ctx.scene_file = Some(path.clone());
    Ok(json!({ "path": path, "entities": ctx.world.len() }))
}

/// Checks a mesh or prefab path a tool sent, like other asset paths.
fn placeable_path(params: &Value) -> Result<String, String> {
    let path = str_param(params, "path")?;
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "json" => super::asset_path_param(params).map(str::to_string),
        "glb" | "gltf" => {
            super::check_tool_path(path, ext)?;
            #[cfg(not(target_arch = "wasm32"))]
            for root in crate::assets::asset_roots() {
                if root.is_dir() {
                    super::check_below(&root, &root.join(path))?;
                }
            }
            Ok(path.to_string())
        }
        _ => Err(format!(
            "`path` must be a prefab (.json) or a glTF mesh (.glb, .gltf) in assets/ (got {path:?})"
        )),
    }
}

/// The `snap` param: `true`, `false`, or `{ "step", "angle" (degrees), "scale" }`.
#[cfg(feature = "3d")]
fn snap_param(params: &Value) -> Result<crate::edit3d::Snap, String> {
    let mut snap = crate::edit3d::Snap::default();
    match params.get("snap") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => snap.on = true,
        Some(Value::Object(o)) => {
            snap.on = true;
            let num = |k: &str| -> Result<Option<f32>, String> {
                match o.get(k) {
                    None | Some(Value::Null) => Ok(None),
                    Some(v) => v
                        .as_f64()
                        .filter(|v| *v > 0.0)
                        .map(|v| Some(v as f32))
                        .ok_or_else(|| format!("`snap.{k}` must be a positive number")),
                }
            };
            snap.step = num("step")?.unwrap_or(snap.step);
            snap.angle = num("angle")?.map_or(snap.angle, f32::to_radians);
            snap.scale = num("scale")?.unwrap_or(snap.scale);
        }
        Some(_) => return Err("`snap` must be true, false or { step, angle, scale }".into()),
    }
    Ok(snap)
}

#[cfg(feature = "3d")]
fn space_param(params: &Value) -> Result<crate::edit3d::Space, String> {
    match params.get("space").and_then(Value::as_str) {
        None | Some("world") => Ok(crate::edit3d::Space::World),
        Some("local") => Ok(crate::edit3d::Space::Local),
        Some(other) => Err(format!("`space` must be \"world\" or \"local\" (got {other:?})")),
    }
}

/// Where a placed thing goes: `at` (a world point), else `screen` (where the
/// ray through that window point meets the ground, y = 0), else the ground
/// in front of the camera. Snapped to the grid when `snap` says so.
#[cfg(feature = "3d")]
fn drop_point(ctx: &Ctx, params: &Value) -> Result<glam::Vec3, String> {
    use crate::edit3d::{Ray, ground_point};
    let at = if let Some(p) = floats::<3>(params, "at")? {
        glam::Vec3::from(p)
    } else {
        let camera = &ctx.scene3d.camera;
        let screen = floats::<2>(params, "screen")?.map_or(camera.viewport() * 0.5, glam::Vec2::from);
        ground_point(&Ray::from_screen(camera, screen), 0.0, 10.0)
    };
    let snap = snap_param(params)?;
    Ok(if snap.on {
        at.map(|v| crate::edit3d::snap(v, snap.step))
    } else {
        at
    })
}

pub(super) fn place(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    let path = placeable_path(params)?;
    let is_prefab = path.ends_with(".json");
    let flat = matches!(params.get("at"), Some(Value::Array(a)) if a.len() == 2);
    let e = if is_prefab && flat {
        let [x, y] = floats::<2>(params, "at")?.unwrap_or_default();
        ctx.edit(|ed, ctx| ed.spawn_prefab(ctx, &path, glam::Vec2::new(x, y)))?
    } else {
        place3d(ctx, params, &path, is_prefab)?
    };
    let parts: Vec<u64> = ctx
        .world
        .get::<crate::scene::PrefabParts>(e)
        .map(|p| p.0.iter().map(|p| p.to_bits()).collect())
        .unwrap_or_default();
    let mut out = history(ctx);
    out["entity"] = json!(e.to_bits());
    out["parts"] = json!(parts);
    Ok(out)
}

#[cfg(feature = "3d")]
fn place3d(ctx: &mut Ctx, params: &Value, path: &str, is_prefab: bool) -> Result<Entity, String> {
    let at = drop_point(ctx, params)?;
    Ok(if is_prefab {
        ctx.edit(|ed, ctx| ed.spawn_prefab(ctx, path, at))?
    } else {
        ctx.edit(|ed, ctx| ed.spawn_model(ctx, path, crate::d3::Transform::at(at)))?
    })
}

#[cfg(not(feature = "3d"))]
fn place3d(_: &mut Ctx, _: &Value, path: &str, _: bool) -> Result<Entity, String> {
    Err(format!(
        "placing {path:?} in 3D needs the `3d` feature; a 2D prefab takes `at` as [x, y]"
    ))
}

#[cfg(feature = "3d")]
fn entity_json3d(ctx: &Ctx, e: Entity) -> Value {
    json!({ "entity": e.to_bits(), "Transform3D": ctx.world.get_json(e, "Transform3D").ok() })
}

#[cfg(feature = "3d")]
fn starts(ctx: &Ctx, entities: &[Entity]) -> Result<Vec<(Entity, crate::d3::Transform)>, String> {
    entities
        .iter()
        .map(|&e| {
            ctx.world
                .get::<crate::d3::Transform>(e)
                .map(|t| (e, *t))
                .ok_or_else(|| format!("entity {} has no Transform3D", e.to_bits()))
        })
        .collect()
}

/// An entity's world transforms after a numeric or dragged edit, written
/// with undo; the reply lists them.
#[cfg(feature = "3d")]
fn commit(ctx: &mut Ctx, to: Vec<(Entity, crate::d3::Transform)>) -> Value {
    ctx.edit(|ed, ctx| ed.set_transforms3d(ctx, &to));
    let mut out = history(ctx);
    out["entities"] = to.iter().map(|&(e, _)| entity_json3d(ctx, e)).collect();
    out
}

#[cfg(feature = "3d")]
pub(super) fn pick(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    let [x, y] = floats::<2>(params, "at")?.ok_or("missing param `at`")?;
    let ray = crate::edit3d::Ray::from_screen(&ctx.scene3d.camera, glam::Vec2::new(x, y));
    let hit = crate::edit3d::pick(&ctx.world, &ctx.gfx.assets3d, &ray, &[]);
    let ground = crate::edit3d::ground_point(&ray, 0.0, 10.0);
    Ok(match hit {
        Some(h) => json!({
            "entity": h.entity.to_bits(),
            "distance": h.distance,
            "point": h.point.to_array(),
            "ground": ground.to_array(),
        }),
        None => json!({ "entity": null, "ground": ground.to_array() }),
    })
}

#[cfg(feature = "3d")]
pub(super) fn transform(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    use crate::edit3d::{Space, axes, rotate, scale, translate};
    let entities = entities_param(ctx, params)?;
    let mut now = starts(ctx, &entities)?;
    let space = space_param(params)?;
    let snap = snap_param(params)?;
    let local = axes(Space::Local, now[0].1.rotation);
    let frame = axes(space, now[0].1.rotation);
    if let Some(d) = floats::<3>(params, "translate")? {
        // In local space the numbers are along the first entity's own axes.
        let delta = (0..3).map(|i| frame[i] * d[i]).sum();
        now = translate(&now, delta, space, frame, [true; 3], snap);
    }
    if let Some(r) = floats::<3>(params, "rotate")? {
        // Degrees about each axis of the frame, in X, Y, Z order.
        let pivot = now.iter().map(|(_, t)| t.position).sum::<glam::Vec3>() / now.len() as f32;
        for (i, deg) in r.iter().enumerate() {
            if *deg != 0.0 {
                now = rotate(&now, pivot, frame[i], deg.to_radians(), snap);
            }
        }
    }
    match params.get("scale") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) => {
            let f = n.as_f64().unwrap_or(1.0) as f32;
            now = scale(&now, glam::Vec3::splat(f), snap);
        }
        Some(_) => {
            let f = floats::<3>(params, "scale")?.unwrap_or([1.0; 3]);
            now = scale(&now, glam::Vec3::from(f), snap);
        }
    }
    let _ = local;
    Ok(commit(ctx, now))
}

#[cfg(feature = "3d")]
pub(super) fn drag(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    use crate::edit3d::{Drag, Handle, Mode, Ray};
    let entities = entities_param(ctx, params)?;
    let mode = match str_param(params, "mode")? {
        "translate" | "move" => Mode::Translate,
        "rotate" => Mode::Rotate,
        "scale" => Mode::Scale,
        other => return Err(format!("`mode` must be translate, rotate or scale (got {other:?})")),
    };
    let handle = match str_param(params, "handle")? {
        "x" => Handle::Axis(0),
        "y" => Handle::Axis(1),
        "z" => Handle::Axis(2),
        "yz" => Handle::Plane(0),
        "xz" => Handle::Plane(1),
        "xy" => Handle::Plane(2),
        "center" => Handle::Center,
        other => {
            return Err(format!(
                "`handle` must be x, y, z, xy, xz, yz or center (got {other:?})"
            ));
        }
    };
    let from = floats::<2>(params, "from")?.ok_or("missing param `from`")?;
    let to = floats::<2>(params, "to")?.ok_or("missing param `to`")?;
    let camera = ctx.scene3d.camera;
    let starts = starts(ctx, &entities)?;
    let ray = Ray::from_screen(&camera, glam::Vec2::from(from));
    let drag = Drag::begin(mode, handle, space_param(params)?, starts, camera.forward(), &ray)
        .ok_or("the drag can't grab that handle from this view (its axis points at the camera)")?;
    let moved = drag
        .update(&Ray::from_screen(&camera, glam::Vec2::from(to)), snap_param(params)?)
        .ok_or("`to` is off the handle's line or plane from this view")?;
    Ok(commit(ctx, moved))
}

#[cfg(feature = "3d")]
pub(super) fn material(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    let e = entity_param(params)?;
    let mesh = str_param(params, "mesh")?.to_string();
    let material = match param(params, "material")? {
        Value::String(name) => json!(name),
        Value::Number(n) if n.is_u64() => json!(n),
        _ => return Err("`material` must be a material's name or its number in the file".into()),
    };
    if ctx.world.get::<crate::render::Model>(e).is_none() {
        return Err(format!("entity {} has no Model to paint", e.to_bits()));
    }
    let value = json!({ "mesh": mesh, "material": material });
    ctx.edit(|ed, ctx| ed.set(ctx, e, "ModelMaterial", value))?;
    let mut out = history(ctx);
    out["entity"] = json!(e.to_bits());
    out["ModelMaterial"] = ctx.world.get_json(e, "ModelMaterial").unwrap_or(Value::Null);
    Ok(out)
}

#[cfg(feature = "3d")]
pub(super) fn camera(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    if let Some(p) = floats::<3>(params, "position")? {
        ctx.scene3d.camera.position = glam::Vec3::from(p);
    }
    if let Some(p) = floats::<3>(params, "look_at")? {
        ctx.scene3d.camera.look_at(glam::Vec3::from(p));
    }
    if let Some(deg) = params.get("fov").filter(|v| !v.is_null()) {
        let deg = deg
            .as_f64()
            .filter(|d| *d > 1.0 && *d < 179.0)
            .ok_or("`fov` must be degrees between 1 and 179")?;
        ctx.scene3d.camera.fov_y = (deg as f32).to_radians();
    }
    let look = floats::<2>(params, "turn")?;
    let step = floats::<3>(params, "fly")?;
    if look.is_some() || step.is_some() {
        crate::edit3d::fly(
            &mut ctx.scene3d.camera,
            look.map_or(glam::Vec2::ZERO, glam::Vec2::from),
            step.map_or(glam::Vec3::ZERO, glam::Vec3::from),
        );
    }
    if let Some(v) = params.get("focus").filter(|v| !v.is_null()) {
        let e = v
            .as_u64()
            .map(Entity::from_bits)
            .ok_or("`focus` must be an entity id")?;
        let (center, radius) = crate::edit3d::world_sphere(&ctx.world, &ctx.gfx.assets3d, e)
            .ok_or_else(|| format!("entity {} has no Transform3D", e.to_bits()))?;
        crate::edit3d::focus(&mut ctx.scene3d.camera, center, radius);
    }
    let c = &ctx.scene3d.camera;
    Ok(json!({
        "position": c.position.to_array(),
        "forward": c.forward().to_array(),
        "rotation": c.rotation.to_array(),
        "fov": c.fov_y.to_degrees(),
        "viewport": c.viewport().to_array(),
    }))
}

#[cfg(not(feature = "3d"))]
pub(super) fn pick(_: &mut Ctx, _: &Value) -> Result<Value, String> {
    Err(NO_3D.into())
}
#[cfg(not(feature = "3d"))]
pub(super) fn transform(_: &mut Ctx, _: &Value) -> Result<Value, String> {
    Err(NO_3D.into())
}
#[cfg(not(feature = "3d"))]
pub(super) fn drag(_: &mut Ctx, _: &Value) -> Result<Value, String> {
    Err(NO_3D.into())
}
#[cfg(not(feature = "3d"))]
pub(super) fn material(_: &mut Ctx, _: &Value) -> Result<Value, String> {
    Err(NO_3D.into())
}
#[cfg(not(feature = "3d"))]
pub(super) fn camera(_: &mut Ctx, _: &Value) -> Result<Value, String> {
    Err(NO_3D.into())
}

#[cfg(test)]
mod tests {
    use super::super::handle;
    use super::*;
    use crate::d2::Transform as Transform2D;

    fn call(ctx: &mut Ctx, method: &str, params: Value) -> Value {
        handle(ctx, method, &params).unwrap_or_else(|e| panic!("{method} {params}: {e}"))
    }

    fn err(ctx: &mut Ctx, method: &str, params: Value) -> String {
        handle(ctx, method, &params).unwrap_err()
    }

    /// A file name in the engine's assets/ unique to this test run.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    fn temp_asset(name: &str) -> (String, std::path::PathBuf) {
        let path = format!("ponko-test-{name}-{}.json", std::process::id());
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(&path);
        (path, file)
    }

    #[test]
    fn two_dimensional_edits_parent_duplicate_delete_and_undo_through_the_endpoint() {
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::new(800.0, 600.0));
        let a = ctx.world.spawn((Transform2D::at(glam::vec2(10.0, 0.0)),)).to_bits();
        let b = ctx.world.spawn((Transform2D::at(glam::vec2(12.0, 0.0)),)).to_bits();
        call(&mut ctx, "editor.parent", json!({ "entity": b, "parent": a }));
        let tree = call(&mut ctx, "editor.hierarchy", json!({}));
        assert_eq!(tree["nodes"][1]["parent"], a);
        assert_eq!(tree["nodes"][1]["link"], "attached");
        assert_eq!(tree["nodes"][1]["depth"], 1);
        let copies = call(
            &mut ctx,
            "editor.duplicate",
            json!({ "entities": [a], "offset": [0, 50] }),
        );
        assert_eq!(ctx.world.len(), 4, "the child came along");
        let copy = Entity::from_bits(copies["entities"][0].as_u64().unwrap());
        assert_eq!(
            ctx.world.get::<Transform2D>(copy).unwrap().position,
            glam::vec2(10.0, 50.0)
        );
        let out = call(&mut ctx, "editor.delete", json!({ "entities": [a] }));
        assert_eq!(out["despawned"], 2);
        assert_eq!(call(&mut ctx, "editor.undo", json!({}))["done"], true);
        assert_eq!(ctx.world.len(), 4);
        assert_eq!(call(&mut ctx, "editor.redo", json!({}))["done"], true);
        assert_eq!(ctx.world.len(), 2);
        assert!(err(&mut ctx, "editor.parent", json!({ "entity": a, "parent": a })).contains("no entity"));
    }

    #[test]
    fn editor_methods_say_what_is_wrong_with_their_params() {
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::new(800.0, 600.0));
        let e = ctx.world.spawn((Transform2D::default(),)).to_bits();
        assert!(err(&mut ctx, "editor.delete", json!({})).starts_with("missing param `entities`"));
        assert!(err(&mut ctx, "editor.delete", json!({ "entities": [] })).contains("empty"));
        assert!(err(&mut ctx, "editor.delete", json!({ "entities": [12345] })).contains("no entity"));
        assert!(err(&mut ctx, "editor.save", json!({})).starts_with("missing param `path`"));
        assert!(err(&mut ctx, "editor.place", json!({ "path": "../x.glb" })).contains("relative"));
        assert!(err(&mut ctx, "editor.place", json!({ "path": "a.png" })).contains("prefab"));
        assert!(err(&mut ctx, "editor.parent", json!({ "entity": e, "parent": "x" })).contains("entity id"));
        let nothing = call(&mut ctx, "editor.undo", json!({}));
        assert_eq!(nothing, json!({ "done": false, "can_undo": false, "can_redo": false }));
    }

    #[cfg(feature = "3d")]
    #[test]
    fn an_agent_assembles_a_3d_level_saves_it_and_it_loads_back_the_same() {
        use crate::d3::Transform as T3;
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::new(800.0, 600.0));
        let (scene, scene_file) = temp_asset("editor-level");
        let assets = call(&mut ctx, "editor.assets", json!({}));
        assert!(
            assets["meshes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m == "models/house.glb"),
            "{assets}"
        );
        assert!(
            assets["prefabs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m == "prefabs/cart.json")
        );
        assert!(
            assets["scenes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m == "levels/beach.json")
        );

        // Two houses on the grid, the second turned a quarter, a third on top of it.
        call(
            &mut ctx,
            "editor.camera",
            json!({ "position": [0, 12, 12], "look_at": [0, 0, 0] }),
        );
        let first = call(
            &mut ctx,
            "editor.place",
            json!({ "path": "models/house.glb", "at": [0.2, 0, -0.1], "snap": { "step": 1 } }),
        )["entity"]
            .clone();
        let second = call(
            &mut ctx,
            "editor.place",
            json!({ "path": "models/house.glb", "screen": [600, 300] }),
        )["entity"]
            .clone();
        let third = call(
            &mut ctx,
            "editor.place",
            json!({ "path": "models/house.glb", "at": [0, 0, 0] }),
        )["entity"]
            .clone();
        let at = |ctx: &Ctx, e: &Value| *ctx.world.get::<T3>(Entity::from_bits(e.as_u64().unwrap())).unwrap();
        assert_eq!(at(&ctx, &first).position, glam::Vec3::ZERO, "snapped to the grid");
        assert!(
            at(&ctx, &second).position.x > 0.5 && at(&ctx, &second).position.y.abs() < 1e-4,
            "on the ground, right of the middle"
        );
        call(
            &mut ctx,
            "editor.transform",
            json!({ "entities": [second], "translate": [4, 0, 0], "rotate": [0, 87, 0], "snap": true }),
        );
        let turned = at(&ctx, &second).rotation;
        assert!(
            turned.angle_between(glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)) < 1e-4,
            "87° snaps to 90°"
        );
        call(
            &mut ctx,
            "editor.transform",
            json!({ "entities": [third], "translate": [6, 3, 0] }),
        );
        call(&mut ctx, "editor.parent", json!({ "entity": third, "parent": second }));
        let above = at(&ctx, &third).position;
        call(
            &mut ctx,
            "editor.transform",
            json!({ "entities": [second], "translate": [0, 0, 2] }),
        );
        assert!(
            (at(&ctx, &third).position - above - glam::Vec3::Z * 2.0).length() < 1e-4,
            "the child came along"
        );

        // Picking finds the first house where it is drawn.
        let screen = crate::edit3d::world_to_screen(&ctx.scene3d.camera, glam::Vec3::new(0.0, 0.5, 0.0)).unwrap();
        let hit = call(&mut ctx, "editor.pick", json!({ "at": screen.to_array() }));
        assert_eq!(hit["entity"], first, "{hit}");
        let empty = call(&mut ctx, "editor.pick", json!({ "at": [0, 0] }));
        assert!(empty["entity"].is_null());

        // A material of the loaded file, then the gizmo the mouse's way.
        let materials = call(&mut ctx, "editor.assets", json!({}))["materials"].clone();
        let house = materials
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["mesh"] == "models/house.glb")
            .unwrap()
            .clone();
        call(
            &mut ctx,
            "editor.material",
            json!({ "entity": first, "mesh": "models/house.glb", "material": house["index"] }),
        );
        let from = crate::edit3d::world_to_screen(&ctx.scene3d.camera, glam::Vec3::new(0.5, 0.0, 0.0)).unwrap();
        let to = crate::edit3d::world_to_screen(&ctx.scene3d.camera, glam::Vec3::new(2.5, 0.0, 0.0)).unwrap();
        call(
            &mut ctx,
            "editor.drag",
            json!({ "entities": [first], "mode": "translate", "handle": "x", "from": from.to_array(), "to": to.to_array() }),
        );
        assert!(
            (at(&ctx, &first).position.x - 2.0).abs() < 1e-3,
            "{}",
            at(&ctx, &first).position
        );
        call(&mut ctx, "editor.undo", json!({}));
        assert!(at(&ctx, &first).position.x.abs() < 1e-5);
        call(&mut ctx, "editor.redo", json!({}));

        let tree = call(&mut ctx, "editor.hierarchy", json!({}));
        let child = tree["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["entity"] == third)
            .unwrap();
        assert_eq!(
            (child["parent"].clone(), child["link"].clone()),
            (second.clone(), json!("attached3d"))
        );

        // Save, change, load: the level comes back as it was saved.
        let saved = handle(&mut ctx, "editor.save", &json!({ "path": scene }));
        let before = ctx.world.save();
        call(&mut ctx, "editor.delete", json!({ "entities": [first, second] }));
        let resaved = handle(&mut ctx, "scene.load", &json!({ "path": scene }));
        let after = ctx.world.save();
        let back_to_file = handle(&mut ctx, "editor.save", &json!({}));
        let _ = std::fs::remove_file(&scene_file);
        saved.unwrap();
        resaved.unwrap();
        assert_eq!(back_to_file.unwrap()["path"], scene, "saves back to the file it loaded");
        assert_eq!(before["entities"], after["entities"]);
        assert_eq!(ctx.world.len(), 3);
        assert!(
            !ctx.editor.can_undo() || !call(&mut ctx, "editor.undo", json!({}))["done"].as_bool().unwrap(),
            "a load starts a new history"
        );
    }

    #[cfg(feature = "3d")]
    #[test]
    fn the_endpoint_drags_and_focuses_like_the_mouse() {
        use crate::d3::Transform as T3;
        let mut ctx = Ctx::new(1.0 / 60.0, glam::Vec2::new(800.0, 600.0));
        let e = ctx.world.spawn((T3::at(glam::vec3(3.0, 0.0, -2.0)),));
        let cam = call(&mut ctx, "editor.camera", json!({ "focus": e.to_bits() }));
        let forward = glam::Vec3::from_slice(&serde_json::from_value::<Vec<f32>>(cam["forward"].clone()).unwrap());
        let position = glam::Vec3::from_slice(&serde_json::from_value::<Vec<f32>>(cam["position"].clone()).unwrap());
        let to_e = (glam::vec3(3.0, 0.0, -2.0) - position).normalize();
        assert!(
            (to_e - forward).length() < 1e-4,
            "focus puts it in the middle of the view"
        );
        let middle = crate::edit3d::world_to_screen(&ctx.scene3d.camera, glam::vec3(3.0, 0.0, -2.0)).unwrap();
        assert!((middle - glam::vec2(400.0, 300.0)).length() < 0.5);
        // Scale it by dragging the center handle out to twice the distance.
        let grab = middle + glam::vec2(40.0, 0.0);
        let out = middle + glam::vec2(80.0, 0.0);
        call(
            &mut ctx,
            "editor.drag",
            json!({ "entities": [e.to_bits()], "mode": "scale", "handle": "center", "from": grab.to_array(), "to": out.to_array(), "snap": { "scale": 0.5 } }),
        );
        assert_eq!(ctx.world.get::<T3>(e).unwrap().scale, glam::Vec3::splat(2.0));
        let bad = err(
            &mut ctx,
            "editor.drag",
            json!({ "entities": [e.to_bits()], "mode": "spin", "handle": "x", "from": [0, 0], "to": [1, 1] }),
        );
        assert!(bad.contains("translate, rotate or scale"), "{bad}");
    }
}
