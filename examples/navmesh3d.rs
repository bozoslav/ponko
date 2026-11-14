//! Headless navigation: compile with `--no-default-features --features navmesh`.
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
#[allow(unused_imports)]
#[cfg(feature = "navmesh")]
use ponko::nav::{NavBakeSettings, NavSource};
use ponko::prelude::*;
use ponko::render::MeshData;

// ANCHOR: bake
fn navigation() -> NavMesh {
    let floor = NavSource::from_mesh(&MeshData::plane(30.0), Transform::default());
    let wall = NavSource::from_mesh(&MeshData::cube(4.0), Transform::at(vec3(0.0, 2.0, 0.0)));
    NavMesh::bake(&[floor, wall], NavBakeSettings::default()).unwrap()
}
// ANCHOR_END: bake

fn main() {
    // ANCHOR: query
    let nav = navigation();
    let start = nav.sample_position(vec3(-8.0, 1.0, 0.0), 2.0).unwrap();
    let destination = vec3(8.0, 0.0, 0.0);
    let corners = nav.find_path(start, destination).unwrap();
    assert!(corners.len() >= 3);
    // ANCHOR_END: query

    // ANCHOR: agents
    let mut world = Game::new("Navigation server", 1, 1)
        .fixed_hz(60)
        .sim_system(systems::step_navmesh3d)
        .setup(move |ctx| {
            ctx.world.insert_resource(nav);
            let mut agent = NavAgent::default();
            agent.destination = Some(destination);
            ctx.world.spawn((Transform::at(start), agent));
            // World-space AABB; move its center or despawn it to reopen polygons.
            ctx.world
                .spawn((NavObstacle::new(vec3(0.0, 1.0, 8.0), vec3(1.0, 1.0, 1.0)),));
        })
        .run_headless(600);
    let position = world.query::<&Transform>().iter().next().unwrap().position;
    assert!(position.distance(destination) < 0.3);
    // ANCHOR_END: agents
}
