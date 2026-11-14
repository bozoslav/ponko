//! Reflects the 2D particle emitter as `"ParticleEmitter2D"`: its settings,
//! with the image saved by file name like a sprite's. Living particles
//! aren't saved; a loaded emitter starts empty, with its first burst.

use std::sync::Arc;

use glam::Vec2;
use ponko_ecs::World;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Reflected, Registry, SchemaSource, TextureNames, check, remove};
use crate::color::Color;
use crate::particles2d::{
    ParticleCurve, ParticleEmitter2D, ParticleGradient, ParticleSettings2D, ParticleShape2D, ParticleSpace,
};
use crate::render::BlendMode;
use crate::schema::ValueSchema;

/// The reflected name.
pub(crate) const NAME: &str = "ParticleEmitter2D";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub(super) enum ShapeJson {
    Point,
    Circle { radius: f32 },
    Cone { angle: f32 },
    Rect { half_extents: Vec2 },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum BlendJson {
    Alpha,
    Additive,
    Multiply,
    Premultiplied,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum SpaceJson {
    World,
    Local,
}

/// A [`ParticleEmitter2D`]'s settings as JSON.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub(super) struct EmitterJson {
    pub rate: f32,
    pub burst: u64,
    pub duration: f32,
    pub looping: bool,
    pub lifetime: (f32, f32),
    pub speed: (f32, f32),
    pub shape: ShapeJson,
    pub gravity: Vec2,
    pub drag: f32,
    pub size: (f32, f32),
    pub rotation: (f32, f32),
    pub size_over_life: Vec<(f32, f32)>,
    pub color_over_life: Vec<(f32, Color)>,
    pub texture: Option<String>,
    pub sheet: (u32, u32),
    pub blend: BlendJson,
    pub z: f32,
    pub max_particles: u64,
    pub space: SpaceJson,
    pub seed: u64,
}

impl Default for EmitterJson {
    fn default() -> Self {
        to_json(&ParticleSettings2D::default(), None).expect("default settings have a JSON form")
    }
}

/// `None` for settings with no JSON form: a material (made in code, it has
/// no name) or a shape added in a later release.
fn to_json(s: &ParticleSettings2D, texture: Option<String>) -> Option<EmitterJson> {
    if s.material.is_some() {
        return None;
    }
    let shape = match s.shape {
        ParticleShape2D::Point => ShapeJson::Point,
        ParticleShape2D::Circle { radius } => ShapeJson::Circle { radius },
        ParticleShape2D::Cone { angle } => ShapeJson::Cone { angle },
        ParticleShape2D::Rect { half_extents } => ShapeJson::Rect { half_extents },
    };
    let blend = match s.blend {
        BlendMode::Alpha => BlendJson::Alpha,
        BlendMode::Additive => BlendJson::Additive,
        BlendMode::Multiply => BlendJson::Multiply,
        BlendMode::Premultiplied => BlendJson::Premultiplied,
    };
    Some(EmitterJson {
        rate: s.rate,
        burst: s.burst as u64,
        duration: s.duration,
        looping: s.looping,
        lifetime: s.lifetime,
        speed: s.speed,
        shape,
        gravity: s.gravity,
        drag: s.drag,
        size: s.size,
        rotation: s.rotation,
        size_over_life: s.size_over_life.keys().to_vec(),
        color_over_life: s.color_over_life.keys().to_vec(),
        texture,
        sheet: s.sheet,
        blend,
        z: s.z,
        max_particles: s.max_particles as u64,
        space: match s.space {
            ParticleSpace::World => SpaceJson::World,
            ParticleSpace::Local => SpaceJson::Local,
        },
        seed: s.seed,
    })
}

/// `None` when the emitter isn't saved: an image made in code (no file to
/// name) or a sprite material.
fn emitter_json(world: &World, e: &ParticleEmitter2D) -> Option<Result<Value, String>> {
    let s = e.settings();
    let texture = match s.texture {
        Some(t) => Some(world.get_resource::<TextureNames>()?.name(t.id)?),
        None => None,
    };
    let json = to_json(s, texture)?;
    Some(serde_json::to_value(json).map_err(|err| err.to_string()))
}

fn emitter_from_json(world: &World, value: Value) -> Result<ParticleEmitter2D, String> {
    let j: EmitterJson = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let texture = match j.texture {
        Some(name) => Some(
            world
                .get_resource::<TextureNames>()
                .and_then(|t| t.get(&name))
                .ok_or_else(|| format!("texture \"{name}\" isn't loaded: ctx.gfx.load_texture(\"{name}\") first"))?,
        ),
        None => None,
    };
    let size_over_life = if j.size_over_life.is_empty() {
        ParticleCurve::default()
    } else {
        ParticleCurve::try_new(j.size_over_life)?
    };
    let color_over_life = if j.color_over_life.is_empty() {
        ParticleGradient::default()
    } else {
        ParticleGradient::try_new(j.color_over_life)?
    };
    let count = |n: u64, what: &str| usize::try_from(n).map_err(|_| format!("particle {what} is too large"));
    let settings = ParticleSettings2D {
        rate: j.rate,
        burst: count(j.burst, "burst")?,
        duration: j.duration,
        looping: j.looping,
        lifetime: j.lifetime,
        speed: j.speed,
        shape: match j.shape {
            ShapeJson::Point => ParticleShape2D::Point,
            ShapeJson::Circle { radius } => ParticleShape2D::Circle { radius },
            ShapeJson::Cone { angle } => ParticleShape2D::Cone { angle },
            ShapeJson::Rect { half_extents } => ParticleShape2D::Rect { half_extents },
        },
        gravity: j.gravity,
        drag: j.drag,
        size: j.size,
        rotation: j.rotation,
        size_over_life,
        color_over_life,
        texture,
        sheet: j.sheet,
        blend: match j.blend {
            BlendJson::Alpha => BlendMode::Alpha,
            BlendJson::Additive => BlendMode::Additive,
            BlendJson::Multiply => BlendMode::Multiply,
            BlendJson::Premultiplied => BlendMode::Premultiplied,
        },
        material: None,
        z: j.z,
        max_particles: count(j.max_particles, "max_particles")?,
        space: match j.space {
            SpaceJson::World => ParticleSpace::World,
            SpaceJson::Local => ParticleSpace::Local,
        },
        seed: j.seed,
    };
    // A pool is allocated up front: refuse sizes a scene file could use
    // to exhaust memory.
    if settings.max_particles > MAX_POOL {
        return Err(format!("particle max_particles is above {MAX_POOL}"));
    }
    ParticleEmitter2D::check(&settings)?;
    Ok(ParticleEmitter2D::new(settings))
}

/// The largest pool a scene may ask for.
const MAX_POOL: usize = 1 << 24;

/// The emitter's JSON shape.
pub(super) fn schema() -> ValueSchema {
    let pair = || ValueSchema::Tuple(vec![ValueSchema::Number; 2]);
    ValueSchema::strict_object([
        ("rate", ValueSchema::Number),
        ("burst", ValueSchema::Unsigned),
        ("duration", ValueSchema::Number),
        ("looping", ValueSchema::Bool),
        ("lifetime", pair()),
        ("speed", pair()),
        (
            "shape",
            ValueSchema::any_of([
                ValueSchema::enum_of(["point"]),
                ValueSchema::strict_object([("circle", ValueSchema::strict_object([("radius", ValueSchema::Number)]))]),
                ValueSchema::strict_object([("cone", ValueSchema::strict_object([("angle", ValueSchema::Number)]))]),
                ValueSchema::strict_object([(
                    "rect",
                    ValueSchema::strict_object([("half_extents", ValueSchema::Vec2)]),
                )]),
            ]),
        ),
        ("gravity", ValueSchema::Vec2),
        ("drag", ValueSchema::Number),
        ("size", pair()),
        ("rotation", pair()),
        ("size_over_life", ValueSchema::array(pair())),
        (
            "color_over_life",
            ValueSchema::array(ValueSchema::Tuple(vec![ValueSchema::Number, ValueSchema::Color])),
        ),
        ("texture", ValueSchema::optional(ValueSchema::String)),
        ("sheet", ValueSchema::Tuple(vec![ValueSchema::Unsigned; 2])),
        (
            "blend",
            ValueSchema::enum_of(["alpha", "additive", "multiply", "premultiplied"]),
        ),
        ("z", ValueSchema::Number),
        ("max_particles", ValueSchema::Unsigned),
        ("space", ValueSchema::enum_of(["world", "local"])),
        ("seed", ValueSchema::Unsigned),
    ])
}

/// Reflects [`ParticleEmitter2D`] as `"ParticleEmitter2D"`.
pub(crate) fn reflect_particle_emitter2d(world: &mut World) {
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
        type_name: std::any::type_name::<ParticleEmitter2D>().into(),
        get: Arc::new(|w: &World, e| w.get::<ParticleEmitter2D>(e).and_then(|p| emitter_json(w, p))),
        set: Arc::new(|w: &mut World, e, v| {
            let emitter = emitter_from_json(w, v)?;
            w.insert(e, emitter);
            Ok(())
        }),
        spawn: Arc::new(|w: &mut World, v| {
            let emitter = emitter_from_json(w, v)?;
            Ok(w.spawn((emitter,)))
        }),
        spawn_at: Arc::new(|w: &mut World, e, v| {
            let emitter = emitter_from_json(w, v)?;
            Ok(w.spawn_at(e, (emitter,)))
        }),
        check: Arc::new(check::<EmitterJson>),
        remove: Arc::new(remove::<ParticleEmitter2D>),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::WorldReflect;

    fn world() -> World {
        let mut world = World::new();
        reflect_particle_emitter2d(&mut world);
        world
    }

    #[test]
    fn emitters_round_trip_through_json() {
        let mut world = world();
        let settings = ParticleSettings2D::default()
            .with_rate(25.0)
            .with_shape(ParticleShape2D::Cone { angle: 0.5 })
            .with_blend(BlendMode::Additive)
            .with_space(ParticleSpace::Local)
            .with_size_over_life(ParticleCurve::new(vec![(0.0, 1.0), (1.0, 0.0)]))
            .with_z(3.0)
            .with_seed(7);
        let e = world.spawn((ParticleEmitter2D::new(settings.clone()),));
        let json = world.get_json(e, NAME).expect("reflected");
        assert_eq!(json["shape"], serde_json::json!({"cone": {"angle": 0.5}}));
        assert_eq!(json["blend"], "additive");
        let copy = world.spawn_json(serde_json::json!({ NAME: json })).expect("spawns");
        assert_eq!(world.get::<ParticleEmitter2D>(copy).unwrap().settings(), &settings);
    }

    #[test]
    fn bad_emitter_json_is_an_error_not_a_panic() {
        let mut world = world();
        for bad in [
            serde_json::json!({"lifetime": [2.0, 1.0]}),
            serde_json::json!({"sheet": [0, 1]}),
            serde_json::json!({"size_over_life": [[1.0, 1.0], [0.0, 0.0]]}),
            serde_json::json!({"texture": "missing.png"}),
            serde_json::json!({"max_particles": u64::MAX}),
            serde_json::json!({"shape": "hexagon"}),
        ] {
            assert!(
                world.spawn_json(serde_json::json!({ NAME: bad.clone() })).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn emitters_with_materials_or_unnamed_images_are_not_saved() {
        let mut world = world();
        let image = crate::texture::Texture::new(4, 8, 8);
        let e = world.spawn((ParticleEmitter2D::new(
            ParticleSettings2D::default().with_texture(image),
        ),));
        assert!(world.get_json(e, NAME).is_err(), "an image made in code has no name");
        let material = crate::render::SpriteMaterial { id: 0 };
        let e = world.spawn((ParticleEmitter2D::new(
            ParticleSettings2D::default().with_material(material),
        ),));
        assert!(world.get_json(e, NAME).is_err(), "a material has no name");
        let e = world.spawn((ParticleEmitter2D::default(),));
        assert!(world.get_json(e, NAME).is_ok());
    }

    #[test]
    fn the_schema_matches_the_json() {
        super::super::builtin_schemas::tests::validate(
            &schema(),
            &serde_json::to_value(EmitterJson::default()).unwrap(),
        );
        let filled = EmitterJson {
            texture: Some("spark.png".into()),
            shape: ShapeJson::Rect {
                half_extents: Vec2::ONE,
            },
            ..EmitterJson::default()
        };
        super::super::builtin_schemas::tests::validate(&schema(), &serde_json::to_value(filled).unwrap());
    }
}
