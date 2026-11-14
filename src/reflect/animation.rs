//! Reflects the sprite-sheet [`Animation`] as `"Animation"`: its sheet (the
//! image by file name, like a sprite's, and the frame size), which frames
//! it plays and how far it got, so prefabs, scenes and saves hold it.

use std::sync::Arc;

use glam::Vec2;
use ponko_ecs::World;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Reflected, Registry, SchemaSource, TextureNames, remove};
use crate::schema::ValueSchema;
use crate::sheet::{Animation, SpriteSheet};
use crate::texture::Texture;

/// The reflected name.
pub(crate) const NAME: &str = "Animation";

/// An [`Animation`] as JSON. A file needs `texture`, `frame_size` and
/// `fps`; the rest has defaults: the sheet's grid (`columns`, `len`) from the
/// image's size, the frames from `first` to the end of the sheet, looping,
/// from the start. Saves write every field.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
struct AnimationJson {
    texture: String,
    frame_size: Vec2,
    #[serde(default)]
    columns: Option<u32>,
    #[serde(default)]
    len: Option<u32>,
    #[serde(default)]
    first: Option<u32>,
    #[serde(default)]
    frames: Option<u32>,
    fps: f32,
    #[serde(default)]
    looping: Option<bool>,
    #[serde(default)]
    time: Option<f32>,
}

impl AnimationJson {
    /// What can be checked without the image: every number in range.
    fn check(&self) -> Result<(), String> {
        if !(self.frame_size.is_finite() && self.frame_size.min_element() > 0.0) {
            return Err(format!(
                "frame_size must be positive, not {:?}",
                self.frame_size.to_array()
            ));
        }
        for (what, n) in [("columns", self.columns), ("len", self.len), ("frames", self.frames)] {
            if n == Some(0) {
                return Err(format!("{what} must be at least 1"));
            }
        }
        if !self.fps.is_finite() {
            return Err("fps must be a number".into());
        }
        if !self.time.unwrap_or(0.0).is_finite() {
            return Err("time must be a number".into());
        }
        Ok(())
    }

    fn into_animation(self, texture: Texture) -> Result<Animation, String> {
        self.check()?;
        // The grid `SpriteSheet::new` cuts, without its panic for frames
        // bigger than the image: such a sheet shows its first frame.
        let fit = |side: u32, frame: f32| ((side as f32 / frame) as u32).max(1);
        let columns = self.columns.unwrap_or_else(|| fit(texture.width(), self.frame_size.x));
        let len = self
            .len
            .unwrap_or_else(|| columns.saturating_mul(fit(texture.height(), self.frame_size.y)));
        let first = self.first.unwrap_or(0);
        Ok(Animation {
            sheet: SpriteSheet {
                texture,
                frame_size: self.frame_size,
                columns,
                len,
            },
            first,
            frames: self.frames.unwrap_or_else(|| len.saturating_sub(first).max(1)),
            fps: self.fps,
            looping: self.looping.unwrap_or(true),
            time: self.time.unwrap_or(0.0),
        })
    }
}

/// `None` for a sheet whose image was made in code: it has no file to
/// name, so the animation isn't saved (the rest of the entity is).
fn animation_json(world: &World, a: &Animation) -> Option<Result<Value, String>> {
    let texture = world.get_resource::<TextureNames>()?.name(a.sheet.texture.id)?;
    let json = AnimationJson {
        texture,
        frame_size: a.sheet.frame_size,
        columns: Some(a.sheet.columns),
        len: Some(a.sheet.len),
        first: Some(a.first),
        frames: Some(a.frames),
        fps: a.fps,
        looping: Some(a.looping),
        time: Some(a.time),
    };
    Some(serde_json::to_value(json).map_err(|err| err.to_string()))
}

fn animation_from_json(world: &World, value: Value) -> Result<Animation, String> {
    let json: AnimationJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let texture = world
        .get_resource::<TextureNames>()
        .and_then(|t| t.get(&json.texture))
        .ok_or_else(|| {
            let name = &json.texture;
            format!("texture \"{name}\" isn't loaded: ctx.gfx.load_texture(\"{name}\") first")
        })?;
    json.into_animation(texture)
}

fn check(value: &Value) -> Result<(), String> {
    AnimationJson::deserialize(value)
        .map_err(|err| err.to_string())?
        .check()
}

/// The animation's JSON shape.
pub(super) fn schema() -> ValueSchema {
    let optional = |s| ValueSchema::optional(s);
    ValueSchema::strict_object([
        ("texture", ValueSchema::String),
        ("frame_size", ValueSchema::Vec2),
        ("columns", optional(ValueSchema::Unsigned)),
        ("len", optional(ValueSchema::Unsigned)),
        ("first", optional(ValueSchema::Unsigned)),
        ("frames", optional(ValueSchema::Unsigned)),
        ("fps", ValueSchema::Number),
        ("looping", optional(ValueSchema::Bool)),
        ("time", optional(ValueSchema::Number)),
    ])
}

/// Reflects [`Animation`] as `"Animation"`.
pub(crate) fn reflect_animation(world: &mut World) {
    if world.get_resource::<TextureNames>().is_none() {
        world.insert_resource(TextureNames::default());
    }
    if world.get_resource::<Registry>().is_none() {
        world.insert_resource(Registry::default());
    }
    let reg = world.resource_mut::<Registry>();
    if reg.by_name.contains_key(NAME) {
        return;
    }
    reg.schemas.insert(NAME.into(), (schema(), SchemaSource::Described));
    reg.by_name.insert(NAME.into(), reg.list.len());
    reg.list.push(Reflected {
        name: NAME.into(),
        type_name: std::any::type_name::<Animation>().into(),
        get: Arc::new(|w: &World, e| w.get::<Animation>(e).and_then(|a| animation_json(w, a))),
        set: Arc::new(|w: &mut World, e, v| {
            let animation = animation_from_json(w, v)?;
            w.insert(e, animation);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let animation = animation_from_json(w, v)?;
            Ok(w.spawn((animation,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let animation = animation_from_json(w, v)?;
            Ok(w.spawn_at(e, (animation,)))
        }),
        check: Arc::new(check),
        remove: Arc::new(remove::<Animation>),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::WorldReflect;
    use serde_json::json;

    /// A world with `strip.png`, 64 x 32: 4 x 2 frames of 16 x 16.
    fn world() -> (World, Texture) {
        let mut world = World::new();
        reflect_animation(&mut world);
        let texture = Texture::new(3, 64, 32);
        world.resource::<TextureNames>().add(texture.id, "strip.png", texture);
        (world, texture)
    }

    #[test]
    fn animations_round_trip_with_their_place_in_the_sheet() {
        let (mut world, texture) = world();
        let mut walk = Animation::new(SpriteSheet::new(texture, 16, 16), 2..6, 12.0).once();
        walk.time = 0.2;
        let e = world.spawn((walk.clone(),));
        let json = world.get_json(e, NAME).expect("reflected");
        assert_eq!(json["texture"], "strip.png");
        assert_eq!(json["time"], json!(0.2f32));
        let copy = world.spawn_json(json!({ NAME: json })).expect("spawns");
        let copy = world.get::<Animation>(copy).unwrap();
        assert_eq!(copy, &walk);
        assert_eq!(copy.current_frame(), walk.current_frame());
    }

    #[test]
    fn a_file_needs_only_the_image_the_frame_size_and_the_speed() {
        let (mut world, texture) = world();
        let e = world
            .spawn_json(json!({ NAME: { "texture": "strip.png", "frame_size": [16, 16], "fps": 8 } }))
            .expect("spawns");
        let a = world.get::<Animation>(e).unwrap();
        assert_eq!(a, &Animation::new(SpriteSheet::new(texture, 16, 16), 0..8, 8.0));
        let e = world
            .spawn_json(json!({ NAME: { "texture": "strip.png", "frame_size": [16, 16], "fps": 8, "first": 6 } }))
            .expect("spawns");
        assert_eq!(world.get::<Animation>(e).unwrap().frames, 2, "to the end of the sheet");
    }

    #[test]
    fn bad_animation_json_is_an_error_not_a_panic() {
        let (mut world, _) = world();
        let base = json!({ "texture": "strip.png", "frame_size": [16, 16], "fps": 8 });
        for patch in [
            json!({ "frame_size": [0, 16] }),
            json!({ "frames": 0 }),
            json!({ "columns": 0 }),
            json!({ "len": 0 }),
            json!({ "texture": "missing.png" }),
            json!({ "speed": 3 }),
        ] {
            let mut bad = base.clone();
            super::super::merge(&mut bad, patch.clone());
            assert!(world.spawn_json(json!({ NAME: bad })).is_err(), "{patch}");
        }
        let mut no_fps = base;
        no_fps.as_object_mut().unwrap().remove("fps");
        assert!(check(&no_fps).is_err());
        // Frames bigger than the image don't panic: one frame.
        let e = world
            .spawn_json(json!({ NAME: { "texture": "strip.png", "frame_size": [100, 100], "fps": 8 } }))
            .expect("spawns");
        assert_eq!(world.get::<Animation>(e).unwrap().sheet.len, 1);
    }

    #[test]
    fn animations_of_images_made_in_code_are_not_saved() {
        let (mut world, _) = world();
        let unnamed = Texture::new(9, 16, 16);
        let e = world.spawn((Animation::new(SpriteSheet::new(unnamed, 16, 16), 0..1, 1.0),));
        assert!(world.get_json(e, NAME).is_err());
    }

    #[test]
    fn the_schema_matches_the_json() {
        let (mut world, texture) = world();
        let e = world.spawn((Animation::new(SpriteSheet::new(texture, 16, 16), 0..4, 6.0),));
        super::super::builtin_schemas::tests::validate(&schema(), &world.get_json(e, NAME).unwrap());
    }
}
