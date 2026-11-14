//! Importing the prelude must leave common game-owned names available.
#![deny(deprecated)]
use ponko::prelude::*;
struct Pose;
struct Material;
struct Job;
#[test]
fn common_game_names_do_not_collide_with_the_prelude() {
    let _ = (Pose, Material, Job);
    let mut world = World::new();
    let entity = world.spawn((d2::Transform::at(vec2(1.0, 2.0)),));
    assert_eq!(world.get::<d2::Transform>(entity).unwrap().position, vec2(1.0, 2.0));
    let _ = Game::new("prelude", 1, 1).run_headless(0, |_| {});
}
#[cfg(feature = "3d")]
#[test]
fn constants_are_discovered_from_their_owning_types() {
    assert_eq!(d3::Decal::MAX, 4096);
    assert_eq!(ponko::spatial::SpatialGrid::<()>::SORT_EVERY, 32);
    assert_eq!(ponko::render::Post::MIN_RENDER_SCALE, 0.25);
    assert_eq!(ponko::terrain::TerrainTile::LODS, 4);
}

/// A game's own module of common names, glob-imported next to the prelude:
/// any of these in the prelude would make its use below ambiguous (an error).
mod game {
    pub struct Pose;
    pub struct Material;
    pub struct Job;
    pub struct Motion;
    pub struct Blend;
    pub struct Effect;
    pub struct Shader;
    pub struct Joint;
    pub struct Post;
    pub struct Sky;
    pub struct Interest;
    pub struct Authority;
    pub struct Lane;
    pub struct Value;
    pub struct Field;
    pub struct Instant;
    pub struct Transform;
    pub struct Camera;
    pub struct Physics;
}

#[test]
fn a_game_glob_of_common_names_does_not_collide_with_the_prelude() {
    use game::*;
    let _ = (Pose, Material, Job, Motion, Blend, Effect, Shader, Joint, Post, Sky);
    let _ = (
        Interest, Authority, Lane, Value, Field, Instant, Transform, Camera, Physics,
    );
    let _: Vec2 = vec2(0.0, 0.0);
}

/// The root holds only the modules, so its glob
/// combines with a dimension glob: `Collider`, `RigidBody`, `Physics` and
/// the rest are the module's, not an ambiguity with old root spellings.
#[cfg(all(feature = "physics", feature = "physics3d"))]
mod root_glob_with_a_dimension_glob {
    use std::any::TypeId;
    mod three {
        use ponko::d3::*;
        #[allow(unused_imports)] // only here to clash, if it could
        use ponko::*;
        pub fn ids() -> [std::any::TypeId; 4] {
            use std::any::TypeId;
            [
                TypeId::of::<RigidBody>(),
                TypeId::of::<Collider>(),
                TypeId::of::<Velocity>(),
                TypeId::of::<Physics>(),
            ]
        }
    }
    mod two {
        use ponko::d2::*;
        #[allow(unused_imports)] // only here to clash, if it could
        use ponko::*;
        pub fn ids() -> [std::any::TypeId; 4] {
            use std::any::TypeId;
            [
                TypeId::of::<RigidBody>(),
                TypeId::of::<Collider>(),
                TypeId::of::<Velocity>(),
                TypeId::of::<Physics>(),
            ]
        }
    }
    #[test]
    fn the_dimension_glob_names_win_without_ambiguity() {
        use ponko::{d2, d3};
        let three = [
            TypeId::of::<d3::RigidBody>(),
            TypeId::of::<d3::Collider>(),
            TypeId::of::<d3::Velocity>(),
            TypeId::of::<d3::Physics>(),
        ];
        let two = [
            TypeId::of::<d2::RigidBody>(),
            TypeId::of::<d2::Collider>(),
            TypeId::of::<d2::Velocity>(),
            TypeId::of::<d2::Physics>(),
        ];
        assert_eq!(three::ids(), three);
        assert_eq!(two::ids(), two);
    }
}
