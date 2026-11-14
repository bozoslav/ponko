//! Explicit schemas for built-in serialization contracts. Never infer from sampled values.
//!
//! Each one is checked against the type's real serialized JSON by
//! `builtin_shapes_match_serialized_fields_and_value_types`.
use crate::schema::ValueSchema::{self, *};
fn transform2d() -> ValueSchema {
    ValueSchema::object([("position", Vec2), ("rotation", Number), ("scale", Vec2)])
}
fn attached() -> ValueSchema {
    ValueSchema::object([("to", Entity), ("offset", Vec2), ("rotation", Number)])
}
fn pointlight2d() -> ValueSchema {
    ValueSchema::object([
        ("color", Color),
        ("intensity", Number),
        ("radius", Number),
        ("falloff", Number),
        ("height", Number),
        ("shadows", Bool),
    ])
}
fn spotlight2d() -> ValueSchema {
    ValueSchema::object([
        ("color", Color),
        ("intensity", Number),
        ("radius", Number),
        ("falloff", Number),
        ("height", Number),
        ("angle", Number),
        ("softness", Number),
        ("shadows", Bool),
    ])
}
fn directionallight2d() -> ValueSchema {
    ValueSchema::object([
        ("color", Color),
        ("intensity", Number),
        ("direction", Vec2),
        ("elevation", Number),
        ("shadows", Bool),
    ])
}
fn occluder2d() -> ValueSchema {
    ValueSchema::object([("edges", ValueSchema::array(Tuple(vec![Vec2; 2])))])
}
fn prefab_parts() -> ValueSchema {
    ValueSchema::array(Entity)
}
#[cfg(feature = "3d")]
fn skin_of() -> ValueSchema {
    Entity
}
#[cfg(feature = "3d")]
fn attach_to_joint() -> ValueSchema {
    ValueSchema::object([("skeleton", Entity), ("joint", Unsigned), ("offset", transform3d())])
}
#[cfg(feature = "3d")]
fn attached3d() -> ValueSchema {
    ValueSchema::object([("to", Entity), ("offset", transform3d())])
}
#[cfg(feature = "physics3d")]
fn joint3d() -> ValueSchema {
    let motion = ValueSchema::any_of([
        ValueSchema::enum_of(["Free", "Locked"]),
        ValueSchema::object([("Limited", Tuple(vec![Number; 2]))]),
    ]);
    let drive = ValueSchema::object([
        ("target_position", Number),
        ("target_velocity", Number),
        ("stiffness", Number),
        ("damping", Number),
        ("max_force", ValueSchema::optional(Number)),
    ]);
    ValueSchema::object([
        ("body_a", Entity),
        ("body_b", Entity),
        ("frame_a", transform3d()),
        ("frame_b", transform3d()),
        ("axes", Tuple(vec![motion; 6])),
        ("drives", Tuple(vec![ValueSchema::optional(drive); 6])),
        ("contacts_enabled", Bool),
        ("swing_cone", ValueSchema::optional(Number)),
    ])
}
#[cfg(feature = "3d")]
fn transform3d() -> ValueSchema {
    ValueSchema::object([
        ("rotation", Tuple(vec![Number; 4])),
        ("position", Vec3),
        ("scale", Vec3),
    ])
}
#[cfg(feature = "3d")]
fn pointlight3d() -> ValueSchema {
    ValueSchema::object([
        ("color", Color),
        ("intensity", Number),
        ("range", Number),
        ("shadows", Bool),
        ("shadow_resolution", Unsigned),
    ])
}
#[cfg(feature = "3d")]
fn spotlight3d() -> ValueSchema {
    ValueSchema::object([
        ("color", Color),
        ("intensity", Number),
        ("range", Number),
        ("inner_angle", Number),
        ("outer_angle", Number),
        ("shadows", Bool),
        ("shadow_resolution", Unsigned),
    ])
}
#[cfg(feature = "3d")]
fn decal3d() -> ValueSchema {
    ValueSchema::object([
        ("size", Vec3),
        ("image", Unsigned),
        ("normal_image", Unsigned),
        ("uv_min", Vec2),
        ("uv_max", Vec2),
        ("color", Color),
        ("opacity", Number),
        ("normal_strength", Number),
        ("roughness", Number),
        ("metallic", Number),
        ("angle_fade_start", Number),
        ("angle_fade_end", Number),
        ("depth_fade", Number),
        ("max_distance", Number),
        ("order", Integer),
        ("layers", Unsigned),
    ])
}
#[cfg(feature = "3d")]
fn lodhysteresis() -> ValueSchema {
    Number
}
#[cfg(feature = "3d")]
fn receivesdecals() -> ValueSchema {
    Unsigned
}
#[cfg(any(feature = "physics", feature = "physics3d"))]
fn body_kind() -> ValueSchema {
    ValueSchema::enum_of(["Dynamic", "Fixed", "Kinematic"])
}
#[cfg(feature = "physics")]
fn rigidbody() -> ValueSchema {
    ValueSchema::object([
        ("kind", body_kind()),
        ("gravity_scale", Number),
        ("linear_damping", Number),
        ("angular_damping", Number),
        ("lock_rotation", Bool),
        ("ccd", Bool),
    ])
}
#[cfg(feature = "physics")]
fn collider() -> ValueSchema {
    ValueSchema::object([
        ("shape", shape2d()),
        ("density", Number),
        ("friction", Number),
        ("restitution", Number),
        ("sensor", Bool),
        ("offset", Vec2),
    ])
}
#[cfg(feature = "physics")]
fn velocity() -> ValueSchema {
    ValueSchema::object([("linear", Vec2), ("angular", Number)])
}
#[cfg(feature = "physics3d")]
fn rigidbody3d() -> ValueSchema {
    ValueSchema::object([
        ("kind", body_kind()),
        ("dominance", Integer),
        ("gravity_scale", Number),
        ("linear_damping", Number),
        ("angular_damping", Number),
        ("lock_rotation", Bool),
        ("ccd", Bool),
        ("mass", ValueSchema::optional(Number)),
        ("center_of_mass", ValueSchema::optional(Vec3)),
    ])
}
#[cfg(feature = "physics3d")]
fn collider3d() -> ValueSchema {
    ValueSchema::object([
        ("shape", shape3d()),
        ("density", Number),
        ("friction", Number),
        ("restitution", Number),
        ("sensor", Bool),
        ("offset", Vec3),
        ("groups", collisiongroups()),
    ])
}
#[cfg(feature = "physics3d")]
fn velocity3d() -> ValueSchema {
    ValueSchema::object([("linear", Vec3), ("angular", Vec3)])
}
#[cfg(feature = "physics3d")]
fn character3d() -> ValueSchema {
    ValueSchema::object([
        ("height", Number),
        ("radius", Number),
        ("max_slope", Number),
        ("step_height", Number),
        ("snap_distance", Number),
        ("push_mass", Number),
        ("gravity_scale", Number),
        ("groups", collisiongroups()),
        ("velocity", Vec3),
        ("grounded", Bool),
    ])
}
#[cfg(feature = "physics3d")]
fn vehicle3d() -> ValueSchema {
    ValueSchema::object([
        ("wheels", Tuple(vec![wheel3d(); crate::d3::Vehicle::MAX_WHEELS])),
        ("wheel_count", Unsigned),
        ("groups", collisiongroups()),
    ])
}
#[cfg(feature = "physics3d")]
fn colliderfocus() -> ValueSchema {
    ValueSchema::object([("radius", Number), ("keep_radius", Number)])
}
fn rect() -> ValueSchema {
    ValueSchema::object([("min", Vec2), ("max", Vec2)])
}
#[cfg(feature = "physics3d")]
fn collisiongroups() -> ValueSchema {
    ValueSchema::object([("memberships", Unsigned), ("filter", Unsigned)])
}
#[cfg(feature = "physics3d")]
fn wheel3d() -> ValueSchema {
    ValueSchema::object([
        ("position", Vec3),
        ("radius", Number),
        ("mass", Number),
        ("suspension_distance", Number),
        ("spring", Number),
        ("damper", Number),
        ("target_position", Number),
        ("force_app_point_distance", Number),
        ("damping_rate", Number),
        ("forward_friction", frictioncurve()),
        ("sideways_friction", frictioncurve()),
        ("motor_torque", Number),
        ("brake_torque", Number),
        ("steer_angle", Number),
        ("local_rotation", Tuple(vec![Number; 4])),
        ("grounded", Bool),
        ("contact_point", Vec3),
        ("contact_normal", Vec3),
        ("compression", Number),
        ("suspension_force", Number),
        ("sprung_mass", Number),
        ("angular_velocity", Number),
        ("forward_slip", Number),
        ("sideways_slip", Number),
        ("spin", Number),
        ("local_position", Vec3),
    ])
}
#[cfg(feature = "physics3d")]
fn frictioncurve() -> ValueSchema {
    ValueSchema::object([
        ("extremum_slip", Number),
        ("extremum_value", Number),
        ("asymptote_slip", Number),
        ("asymptote_value", Number),
        ("stiffness", Number),
    ])
}
pub(super) fn builtin<T: 'static>() -> Option<ValueSchema> {
    let id = std::any::TypeId::of::<T>();
    if id == std::any::TypeId::of::<crate::d2::Transform>() {
        return Some(transform2d());
    }
    if id == std::any::TypeId::of::<crate::attach::Attached>() {
        return Some(attached());
    }
    if id == std::any::TypeId::of::<crate::light2d::PointLight2D>() {
        return Some(pointlight2d());
    }
    if id == std::any::TypeId::of::<crate::light2d::SpotLight2D>() {
        return Some(spotlight2d());
    }
    if id == std::any::TypeId::of::<crate::light2d::DirectionalLight2D>() {
        return Some(directionallight2d());
    }
    if id == std::any::TypeId::of::<crate::light2d::Occluder2D>() {
        return Some(occluder2d());
    }
    if id == std::any::TypeId::of::<crate::scene::PrefabParts>() {
        return Some(prefab_parts());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::anim::SkinOf>() {
        return Some(skin_of());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::anim::AttachToJoint>() {
        return Some(attach_to_joint());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::attach::Attached3D>() {
        return Some(attached3d());
    }
    #[cfg(feature = "physics3d")]
    if id == std::any::TypeId::of::<crate::d3::Joint>() {
        return Some(joint3d());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::d3::Transform>() {
        return Some(transform3d());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::d3::PointLight>() {
        return Some(pointlight3d());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::d3::SpotLight>() {
        return Some(spotlight3d());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::d3::Decal>() {
        return Some(decal3d());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::render::LodHysteresis>() {
        return Some(lodhysteresis());
    }
    #[cfg(feature = "3d")]
    if id == std::any::TypeId::of::<crate::render::ReceivesDecals>() {
        return Some(receivesdecals());
    }
    #[cfg(feature = "physics")]
    if id == std::any::TypeId::of::<crate::d2::RigidBody>() {
        return Some(rigidbody());
    }
    #[cfg(feature = "physics")]
    if id == std::any::TypeId::of::<crate::d2::Collider>() {
        return Some(collider());
    }
    #[cfg(feature = "physics")]
    if id == std::any::TypeId::of::<crate::d2::Velocity>() {
        return Some(velocity());
    }
    #[cfg(feature = "physics3d")]
    if id == std::any::TypeId::of::<crate::d3::RigidBody>() {
        return Some(rigidbody3d());
    }
    #[cfg(feature = "physics3d")]
    if id == std::any::TypeId::of::<crate::d3::Collider>() {
        return Some(collider3d());
    }
    #[cfg(feature = "physics3d")]
    if id == std::any::TypeId::of::<crate::d3::Velocity>() {
        return Some(velocity3d());
    }
    #[cfg(feature = "physics3d")]
    if id == std::any::TypeId::of::<crate::d3::Character>() {
        return Some(character3d());
    }
    #[cfg(feature = "physics3d")]
    if id == std::any::TypeId::of::<crate::d3::Vehicle>() {
        return Some(vehicle3d());
    }
    #[cfg(feature = "physics3d")]
    if id == std::any::TypeId::of::<crate::physics3d::ColliderFocus>() {
        return Some(colliderfocus());
    }
    None
}

pub(super) fn sprite() -> ValueSchema {
    ValueSchema::strict_object([
        ("texture", ValueSchema::optional(String)),
        ("color", Color),
        ("z", Number),
        ("pivot", Vec2),
        ("region", ValueSchema::optional(rect())),
    ])
}
#[cfg(feature = "3d")]
pub(super) fn model() -> ValueSchema {
    ValueSchema::strict_object([("mesh", ValueSchema::optional(String)), ("color", Color)])
}
#[cfg(feature = "3d")]
pub(super) fn shadow() -> ValueSchema {
    ValueSchema::strict_object([("mesh", ValueSchema::optional(String))])
}
/// A material of a loaded mesh file, by its name in the file or its number.
#[cfg(feature = "3d")]
fn file_material() -> ValueSchema {
    ValueSchema::any_of([String, Unsigned])
}
#[cfg(feature = "3d")]
pub(super) fn model_material() -> ValueSchema {
    ValueSchema::strict_object([("mesh", String), ("material", file_material())])
}
#[cfg(feature = "3d")]
pub(super) fn model_materials() -> ValueSchema {
    ValueSchema::strict_object([(
        "parts",
        ValueSchema::array(ValueSchema::strict_object([
            ("part", Unsigned),
            ("mesh", String),
            ("material", file_material()),
        ])),
    )])
}
#[cfg(feature = "3d")]
pub(super) fn lod() -> ValueSchema {
    let renderers = || {
        ValueSchema::array(ValueSchema::strict_object([
            ("mesh", ValueSchema::optional(String)),
            ("offset", transform3d()),
            ("color", Color),
        ]))
    };
    ValueSchema::strict_object([
        ("size", Number),
        ("center", Vec3),
        (
            "levels",
            ValueSchema::array(ValueSchema::strict_object([
                ("screen_height", Number),
                ("fade_width", Number),
                ("renderers", renderers()),
                (
                    "shadow",
                    ValueSchema::any_of([
                        ValueSchema::enum_of(["Same"]),
                        ValueSchema::strict_object([("Casters", renderers())]),
                    ]),
                ),
            ])),
        ),
    ])
}

#[cfg(feature = "physics")]
fn shape2d() -> ValueSchema {
    ValueSchema::any_of([
        ValueSchema::strict_object([("Circle", ValueSchema::object([("radius", Number)]))]),
        ValueSchema::strict_object([("Rect", ValueSchema::object([("size", Vec2)]))]),
        ValueSchema::strict_object([("Capsule", ValueSchema::object([("height", Number), ("radius", Number)]))]),
        ValueSchema::strict_object([(
            "Polygon",
            ValueSchema::object([
                ("points", Tuple(vec![Vec2; crate::physics::MAX_POLYGON_POINTS])),
                ("count", Unsigned),
            ]),
        )]),
    ])
}
#[cfg(feature = "physics3d")]
fn shape3d() -> ValueSchema {
    ValueSchema::any_of([
        ValueSchema::strict_object([("Sphere", ValueSchema::object([("radius", Number)]))]),
        ValueSchema::strict_object([("Cuboid", ValueSchema::object([("size", Vec3)]))]),
        ValueSchema::strict_object([("Capsule", ValueSchema::object([("height", Number), ("radius", Number)]))]),
        // A mesh file, as a scene names it (see `physics3d::files`): scale
        // and convex may be left out. Meshes made in code aren't saved.
        ValueSchema::strict_object([(
            "Mesh",
            ValueSchema::object([
                ("file", String),
                ("scale", ValueSchema::optional(Vec3)),
                ("convex", ValueSchema::optional(Bool)),
            ]),
        )]),
    ])
}

#[cfg(test)]
pub(in crate::reflect) mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Panics unless `value` has exactly the shape `schema` describes: every
    /// field present and of its type, no field more.
    pub(in crate::reflect) fn validate(schema: &ValueSchema, value: &Value) {
        match schema {
            Object(fields) | StrictObject(fields) => {
                let object = value.as_object().unwrap_or_else(|| panic!("object: {value}"));
                assert_eq!(fields.len(), object.len(), "field drift: {value}");
                for (name, shape) in fields {
                    let field = object.get(name).unwrap_or_else(|| panic!("no `{name}` in {value}"));
                    validate(shape, field);
                }
            }
            Tuple(items) => {
                let values = value.as_array().expect("tuple");
                assert_eq!(items.len(), values.len(), "tuple length: {value}");
                for (s, v) in items.iter().zip(values) {
                    validate(s, v);
                }
            }
            Array(item) => {
                for v in value.as_array().expect("array") {
                    validate(item, v);
                }
            }
            Optional(item) => {
                if !value.is_null() {
                    validate(item, value);
                }
            }
            Number => assert!(value.is_number(), "number: {value}"),
            Integer => assert!(value.is_i64(), "integer: {value}"),
            Unsigned | Entity => assert!(value.is_u64(), "unsigned: {value}"),
            Bool => assert!(value.is_boolean(), "bool: {value}"),
            String => assert!(value.is_string(), "string: {value}"),
            Enum(names) => assert!(names.iter().any(|s| value == s), "{value} not in {names:?}"),
            Vec2 => validate(&Tuple(vec![Number; 2]), value),
            Vec3 => validate(&Tuple(vec![Number; 3]), value),
            Color => validate(
                &ValueSchema::object([("r", Number), ("g", Number), ("b", Number), ("a", Number)]),
                value,
            ),
            Null => assert!(value.is_null()),
            AnyOf(options) => {
                let selected = options
                    .iter()
                    .find(|s| match s {
                        Object(fields) | StrictObject(fields) => value.as_object().is_some_and(|o| {
                            fields
                                .iter()
                                .all(|(n, s)| o.contains_key(n) || matches!(s, Optional(_)))
                        }),
                        Enum(names) => names.iter().any(|n| value == n),
                        String => value.is_string(),
                        Unsigned => value.is_u64(),
                        _ => false,
                    })
                    .unwrap_or_else(|| panic!("{value} matches none of {options:?}"));
                validate(selected, value);
            }
            Opaque => panic!("built-in schema must describe every field"),
        }
    }

    /// Whether `schema` has an `Optional` (anywhere inside it) that `value`
    /// fills in, so both sides of every optional field get checked.
    fn fills_an_optional(schema: &ValueSchema, value: &Value) -> bool {
        match (schema, value) {
            (Optional(_), v) => !v.is_null(),
            (Object(fields) | StrictObject(fields), Value::Object(o)) => fields
                .iter()
                .any(|(n, s)| o.get(n).is_some_and(|v| fills_an_optional(s, v))),
            (Array(item), Value::Array(values)) => values.iter().any(|v| fills_an_optional(item, v)),
            (AnyOf(options), v) => options.iter().any(|s| fills_an_optional(s, v)),
            _ => false,
        }
    }

    fn json_of(value: impl serde::Serialize) -> Value {
        serde_json::to_value(value).unwrap()
    }

    #[test]
    fn builtin_shapes_match_serialized_fields_and_value_types() {
        macro_rules! check {
            ($t:ty, $value:expr) => {
                validate(&builtin::<$t>().unwrap(), &json_of($value));
            };
        }
        use super::super::SpriteJson;
        validate(&sprite(), &json_of(SpriteJson::default()));
        let filled = SpriteJson {
            texture: Some("hero.png".into()),
            region: Some(crate::camera::Rect {
                min: glam::Vec2::ZERO,
                max: glam::Vec2::ONE,
            }),
            ..SpriteJson::default()
        };
        assert!(fills_an_optional(&sprite(), &json_of(&filled)));
        validate(&sprite(), &json_of(filled));
        #[cfg(feature = "3d")]
        {
            use super::super::{
                LodGroupJson, LodLevelJson, LodRendererJson, LodShadowJson, MaterialJson, MaterialRef, ModelJson,
                ModelMaterialsJson, PartMaterialJson, ShadowMeshJson,
            };
            validate(&model(), &json_of(ModelJson::default()));
            let model_with_mesh = ModelJson {
                mesh: Some("rock.glb".into()),
                ..ModelJson::default()
            };
            validate(&model(), &json_of(model_with_mesh));
            validate(&shadow(), &json_of(ShadowMeshJson::default()));
            let shadow_with_mesh = ShadowMeshJson {
                mesh: Some("rock_low.glb".into()),
            };
            validate(&shadow(), &json_of(shadow_with_mesh));

            let renderer = |mesh: Option<&str>| LodRendererJson {
                mesh: mesh.map(Into::into),
                ..LodRendererJson::default()
            };
            validate(&lod(), &json_of(LodGroupJson::default()));
            let group = LodGroupJson {
                levels: vec![
                    LodLevelJson {
                        screen_height: 0.5,
                        renderers: vec![renderer(Some("tree.glb")), renderer(None)],
                        ..LodLevelJson::default()
                    },
                    LodLevelJson {
                        screen_height: 0.1,
                        fade_width: 0.05,
                        renderers: vec![renderer(Some("tree_low.glb"))],
                        shadow: LodShadowJson::Casters(vec![renderer(Some("tree_card.glb"))]),
                    },
                ],
                ..LodGroupJson::default()
            };
            let group = json_of(group);
            assert_eq!(group["levels"][0]["shadow"], "Same");
            assert!(group["levels"][1]["shadow"]["Casters"].is_array());
            validate(&lod(), &group);

            let by_name = || MaterialRef::Name("Wet".into());
            for material in [by_name(), MaterialRef::Index(2)] {
                let json = MaterialJson {
                    mesh: "rocks.glb".into(),
                    material,
                };
                validate(&model_material(), &json_of(json));
            }
            let parts = ModelMaterialsJson {
                parts: vec![
                    PartMaterialJson {
                        part: 0,
                        mesh: "door.glb".into(),
                        material: by_name(),
                    },
                    PartMaterialJson {
                        part: 1,
                        mesh: "door.glb".into(),
                        material: MaterialRef::Index(0),
                    },
                ],
            };
            validate(&model_materials(), &json_of(parts));
        }
        check!(crate::d2::Transform, crate::d2::Transform::default());
        check!(
            crate::attach::Attached,
            crate::attach::Attached::to(ponko_ecs::Entity::from_bits(0))
        );
        check!(crate::light2d::PointLight2D, crate::light2d::PointLight2D::default());
        check!(crate::light2d::SpotLight2D, crate::light2d::SpotLight2D::default());
        check!(
            crate::light2d::DirectionalLight2D,
            crate::light2d::DirectionalLight2D::default()
        );
        check!(
            crate::light2d::Occluder2D,
            crate::light2d::Occluder2D::rect(glam::vec2(4.0, 2.0))
        );
        let e = ponko_ecs::Entity::from_bits;
        check!(
            crate::scene::PrefabParts,
            crate::scene::PrefabParts(vec![e(1), e(2 | 3 << 32)])
        );
        #[cfg(feature = "3d")]
        {
            check!(crate::anim::SkinOf, crate::anim::SkinOf(e(4)));
            check!(crate::attach::Attached3D, crate::attach::Attached3D::to(e(4)));
            check!(
                crate::anim::AttachToJoint,
                crate::anim::AttachToJoint {
                    skeleton: e(4),
                    joint: 2,
                    offset: crate::d3::Transform::default(),
                }
            );
        }
        #[cfg(feature = "physics3d")]
        {
            use crate::d3::{Joint, JointAxis, JointDrive, JointMotion};
            check!(Joint, Joint::spherical(e(1), e(2)));
            check!(
                Joint,
                Joint::spherical(e(1), e(2))
                    .motion(JointAxis::AngularX, JointMotion::Limited(-0.5, 0.5))
                    .drive(JointAxis::AngularX, JointDrive::velocity(2.0, 10.0))
                    .swing_cone(0.5)
            );
        }
        #[cfg(feature = "3d")]
        {
            check!(crate::d3::Transform, crate::d3::Transform::default());
            check!(crate::d3::PointLight, crate::d3::PointLight::default());
            check!(crate::d3::SpotLight, crate::d3::SpotLight::default());
            check!(crate::d3::Decal, crate::d3::Decal::default());
            check!(crate::render::ReceivesDecals, crate::render::ReceivesDecals::default());
            check!(crate::render::LodHysteresis, crate::render::LodHysteresis::default());
        }
        #[cfg(any(feature = "physics", feature = "physics3d"))]
        let kinds = [
            crate::collision::BodyKind::Dynamic,
            crate::collision::BodyKind::Fixed,
            crate::collision::BodyKind::Kinematic,
        ];
        #[cfg(feature = "physics")]
        {
            for kind in kinds {
                check!(
                    crate::d2::RigidBody,
                    crate::d2::RigidBody {
                        kind,
                        ..Default::default()
                    }
                );
            }
            use crate::d2::Collider;
            use crate::physics::MAX_POLYGON_POINTS;
            let v = glam::Vec2::new;
            let most: Vec<_> = (0..MAX_POLYGON_POINTS)
                .map(|i| v((i as f32).cos(), (i as f32).sin()))
                .collect();
            for collider in [
                Collider::circle(1.0),
                Collider::rect(v(2.0, 1.0)),
                Collider::capsule(2.0, 0.5),
                Collider::polygon(&[v(0.0, 0.0), v(1.0, 0.0), v(0.0, 1.0)]),
                Collider::polygon(&most),
            ] {
                check!(Collider, collider);
            }
            let polygon = json_of(Collider::polygon(&most));
            assert_eq!(
                polygon["shape"]["Polygon"]["points"].as_array().unwrap().len(),
                MAX_POLYGON_POINTS
            );
            check!(crate::d2::Velocity, crate::d2::Velocity::default());
        }
        #[cfg(feature = "physics3d")]
        {
            for kind in kinds {
                check!(
                    crate::d3::RigidBody,
                    crate::d3::RigidBody {
                        kind,
                        ..Default::default()
                    }
                );
            }
            let body = crate::d3::RigidBody {
                mass: Some(2.0),
                center_of_mass: Some(glam::Vec3::Y),
                ..Default::default()
            };
            assert!(fills_an_optional(&rigidbody3d(), &json_of(body)));
            check!(crate::d3::RigidBody, body);
            use crate::d3::Collider as Collider3D;
            use crate::handle::Handle;
            use crate::physics3d::CollisionMesh;
            for collider in [
                Collider3D::sphere(1.0),
                Collider3D::cuboid(glam::Vec3::ONE),
                Collider3D::capsule(2.0, 0.5),
            ] {
                check!(Collider3D, collider);
            }
            // A mesh made from a file is saved as that file's description;
            // one made in code has no file to name, so it isn't saved.
            let mut world = ponko_ecs::World::new();
            let made = Collider3D::mesh(CollisionMesh { id: 0 });
            assert!(super::super::collider_json(&world, &made).is_none());
            let mut files = super::super::ColliderFiles::default();
            let file = json!({ "file": "rock.glb", "scale": [1.0, 2.0, 1.0], "convex": true });
            files.by_mesh.insert(CollisionMesh::NONE, file.clone());
            world.insert_resource(files);
            assert!(super::super::collider_json(&world, &made).is_none());
            let saved = super::super::collider_json(&world, &Collider3D::mesh(CollisionMesh::NONE))
                .unwrap()
                .unwrap();
            assert_eq!(saved["shape"]["Mesh"], file);
            validate(&collider3d(), &saved);
            // A scene may leave scale and convex out: only the file is required.
            let mesh = &collider3d().to_json_schema()["properties"]["shape"]["anyOf"][3];
            assert_eq!(mesh["properties"]["Mesh"]["required"], json!(["file"]), "{mesh}");
            check!(crate::d3::Velocity, crate::d3::Velocity::default());
            check!(crate::d3::Character, crate::d3::Character::default());
            let vehicle = json_of(crate::d3::Vehicle::default());
            assert_eq!(
                vehicle["wheels"].as_array().unwrap().len(),
                crate::d3::Vehicle::MAX_WHEELS
            );
            check!(crate::d3::Vehicle, crate::d3::Vehicle::default());
            check!(
                crate::physics3d::ColliderFocus,
                crate::physics3d::ColliderFocus::default()
            );
        }
    }

    #[test]
    fn the_checker_rejects_drift() {
        let schema = ValueSchema::object([("a", Number)]);
        assert!(std::panic::catch_unwind(|| validate(&schema, &json!({ "a": 1, "b": 2 }))).is_err());
        assert!(std::panic::catch_unwind(|| validate(&schema, &json!({ "a": "1" }))).is_err());
        let polygon = Tuple(vec![Vec2; 2]);
        assert!(std::panic::catch_unwind(|| validate(&polygon, &json!([[0, 0], [1, 1], [2, 2]]))).is_err());
        let choice = ValueSchema::any_of([ValueSchema::strict_object([("Circle", Number)])]);
        assert!(std::panic::catch_unwind(|| validate(&choice, &json!({ "Square": 1 }))).is_err());
    }
}
