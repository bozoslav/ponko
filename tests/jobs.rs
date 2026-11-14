//! Streaming: background jobs, the cell streamer, and freeing meshes and
//! terrain tiles as a world streams in and out.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform as Transform3D;
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{Mesh, MeshData, Model};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::terrain::{FarTerrain, TerrainTileData};

#[test]
fn jobs_run_off_the_calling_thread() {
    let caller = std::thread::current().id();
    let worker = ponko::streaming::Job::spawn(|| std::thread::current().id()).wait();
    assert_ne!(worker, caller);
}

#[test]
fn a_job_hands_its_result_over_once() {
    let mut job = ponko::streaming::Job::spawn(|| vec![1, 2, 3]);
    while !job.is_done() {
        std::thread::yield_now();
    }
    assert_eq!(job.take(), Some(vec![1, 2, 3]));
    assert_eq!(job.take(), None);
    assert!(job.is_done(), "still done after taking");
}

/// Frame-time parallel work (here `par_for_each`) must not wait for
/// background jobs: with every core's worth of long jobs in flight, the
/// query still finishes long before they do.
#[test]
fn busy_jobs_dont_stall_parallel_queries() {
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    let mut world = World::new();
    for i in 0..100_000 {
        world.spawn((Transform2D::at(vec2(i as f32, 0.0)),));
    }
    // Long jobs, one per thread of the frame's pool, held until released (or
    // 3 s at most, so this test ends even when they do block the query).
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let jobs: Vec<_> = (0..rayon::current_num_threads())
        .map(|_| {
            let release = release.clone();
            ponko::streaming::Job::spawn(move || {
                let (lock, cvar) = &*release;
                let held = lock.lock().unwrap();
                let _held = cvar
                    .wait_timeout_while(held, Duration::from_secs(3), |go| !*go)
                    .unwrap();
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(50)); // let the jobs start
    let start = Instant::now();
    world.query::<&mut Transform2D>().par_for_each(|t| t.position.y += 1.0);
    let took = start.elapsed();
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    for job in jobs {
        job.wait();
    }
    assert!(
        took < Duration::from_secs(1),
        "par_for_each waited {took:?} for background jobs"
    );
    assert!(world.query::<&Transform2D>().iter().all(|t| t.position.y == 1.0));
}

#[test]
fn many_jobs_all_finish() {
    let ran = Arc::new(AtomicUsize::new(0));
    let jobs: Vec<_> = (0..64)
        .map(|i| {
            let ran = ran.clone();
            ponko::streaming::Job::spawn(move || {
                ran.fetch_add(1, Ordering::Relaxed);
                i * 2
            })
        })
        .collect();
    let sum: i32 = jobs.into_iter().map(ponko::streaming::Job::wait).sum();
    assert_eq!(sum, (0..64).map(|i| i * 2).sum::<i32>());
    assert_eq!(ran.load(Ordering::Relaxed), 64);
}

#[test]
#[should_panic(expected = "a job's work panicked")]
fn a_panicking_job_panics_when_waited_on() {
    let job = ponko::streaming::Job::<i32>::spawn(|| panic!("boom"));
    job.wait();
}

#[test]
#[should_panic(expected = "the result was already taken")]
fn waiting_after_taking_panics() {
    let mut job = ponko::streaming::Job::spawn(|| 1);
    while job.take().is_none() {}
    job.wait();
}

#[cfg(feature = "3d")]
mod cells {
    // Raw ids, only to check that freed mesh slots are reused.
    use ponko::__ffi::{self, SLOT_MASK};

    use super::*;

    fn sorted(cells: impl Iterator<Item = (i32, i32)>) -> Vec<(i32, i32)> {
        let mut v: Vec<_> = cells.collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn cells_load_nearest_first() {
        let mut cells = ponko::streaming::CellStreamer::new(10.0, 30.0, 30.0);
        cells.update(vec3(5.0, 0.0, 5.0), usize::MAX);
        let loads = cells.to_load();
        assert_eq!(loads[0], (0, 0), "the cell holding the point comes first");
        // Distance from the point to the nearest spot of each cell.
        let away = |&(x, z): &(i32, i32)| {
            let min = vec2(x as f32, z as f32) * 10.0;
            (vec2(5.0, 5.0).clamp(min, min + 10.0) - vec2(5.0, 5.0)).length()
        };
        for pair in loads.windows(2) {
            assert!(away(&pair[0]) <= away(&pair[1]), "{pair:?}");
        }
        assert!(loads.contains(&(3, 0)) && !loads.contains(&(4, 0)));
    }

    #[test]
    fn only_max_new_cells_start_per_update() {
        let mut cells = ponko::streaming::CellStreamer::new(1.0, 5.0, 5.0);
        let mut total = 0;
        for _ in 0..1000 {
            cells.update(Vec3::ZERO, 3);
            assert!(cells.to_load().len() <= 3);
            total += cells.to_load().len();
            if cells.to_load().is_empty() {
                break;
            }
        }
        assert_eq!(total, cells.loaded().count());
        assert!(total > 50);
    }

    #[test]
    fn the_keep_radius_stops_edge_cells_flickering() {
        let mut cells = ponko::streaming::CellStreamer::new(10.0, 20.0, 40.0);
        cells.update(vec3(5.0, 0.0, 5.0), usize::MAX);
        let first = sorted(cells.loaded());
        // A small step and back: nothing drops, and nothing new is left
        // after the step back.
        cells.update(vec3(15.0, 0.0, 5.0), usize::MAX);
        assert!(cells.to_drop().is_empty());
        cells.update(vec3(5.0, 0.0, 5.0), usize::MAX);
        assert!(cells.to_drop().is_empty());
        assert!(cells.to_load().is_empty());
        assert!(first.iter().all(|c| cells.loaded().any(|l| l == *c)));
    }

    #[test]
    fn far_cells_drop_and_every_cell_is_accounted_for() {
        let mut cells = ponko::streaming::CellStreamer::new(16.0, 50.0, 70.0);
        let mut live = std::collections::HashSet::new();
        for step in 0..200 {
            let at = vec3(step as f32 * 7.0, 0.0, (step as f32 * 0.1).sin() * 100.0);
            cells.update(at, 4);
            for c in cells.to_drop() {
                assert!(live.remove(c), "{c:?} dropped but never loaded");
            }
            for c in cells.to_load() {
                assert!(live.insert(*c), "{c:?} loaded twice");
            }
            assert_eq!(sorted(live.iter().copied()), sorted(cells.loaded()));
        }
    }

    #[test]
    fn clearing_forgets_every_cell() {
        let mut cells = ponko::streaming::CellStreamer::new(1.0, 2.0, 2.0);
        cells.update(Vec3::ZERO, usize::MAX);
        cells.clear();
        cells.update(Vec3::ZERO, usize::MAX);
        assert_eq!(cells.to_load().len(), cells.loaded().count());
        assert!(cells.to_drop().is_empty());
    }

    #[test]
    #[should_panic(expected = "cell_size must be above 0")]
    fn cells_need_a_size() {
        let _ = ponko::streaming::CellStreamer::new(0.0, 1.0, 1.0);
    }

    #[test]
    fn freed_meshes_are_not_drawn() {
        Game::new("t", 64, 64).run_headless(1, |ctx| {
            let material = ctx.gfx.default_material();
            let kept = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            let freed = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            ctx.scene3d.camera.position = vec3(0.0, 0.0, 5.0);
            ctx.scene3d.camera.look_at(Vec3::ZERO);
            ctx.world.spawn((Transform3D::default(), Model::new(kept)));
            ctx.world.spawn((Transform3D::default(), Model::new(freed)));
            ctx.gfx.free(freed);
            ctx.gfx.free(freed); // twice is fine
            assert!(ctx.gfx.is_live(kept));
            assert!(!ctx.gfx.is_live(freed));
        });
    }

    #[test]
    fn mesh_none_never_becomes_another_mesh() {
        Game::new("t", 64, 64).run_headless(1, |ctx| {
            let material = ctx.gfx.default_material();
            let cube = MeshData::cube(1.0);
            let freed = ctx.gfx.create_mesh(&cube, material);
            ctx.gfx.free(freed);
            ctx.gfx.free(Mesh::NONE); // does nothing
            ctx.gfx.set_mesh_shadows(Mesh::NONE, false); // does nothing
            ctx.scene3d.camera.position = vec3(0.0, 0.0, 5.0);
            ctx.scene3d.camera.look_at(Vec3::ZERO);
            ctx.world.spawn((Transform3D::default(), Model::new(Mesh::NONE)));
            // A freed slot is taken by the next mesh made (under a new
            // id); NONE never is.
            let made: Vec<Mesh> = (0..4).map(|_| ctx.gfx.create_mesh(&cube, material)).collect();
            let slot = |m: Mesh| __ffi::id(m) & SLOT_MASK;
            assert!(made.iter().any(|&m| slot(m) == slot(freed)));
            assert!(!made.contains(&freed));
            assert!(!made.contains(&Mesh::NONE));
            assert!(!ctx.gfx.is_live(Mesh::NONE));
            assert!(ctx.gfx.mesh_shadows(Mesh::NONE));
        });
    }

    #[test]
    fn tiles_built_on_a_worker_match_tiles_built_in_place() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let heights: Vec<f32> = (0..17 * 17).map(|i| (i % 17) as f32 * 0.5).collect();
            let material = ctx.gfx.default_material();
            let here = ctx.gfx.create_terrain_tile(&heights, 32.0, material);
            let moved = heights.clone();
            let job = ponko::streaming::Job::spawn(move || TerrainTileData::new(&moved, 32.0));
            let there = ctx.gfx.upload_terrain_tile(&job.wait(), material);
            assert_eq!(here.lod_distance(), there.lod_distance());
            ctx.free(there);
            assert!(!ctx.is_live(there) && ctx.is_live(here));
            assert!(there.lods().iter().all(|&m| !ctx.gfx.is_live(m)));
            assert!(here.lods().iter().all(|&m| ctx.gfx.is_live(m)));
        });
    }

    #[test]
    fn freeing_a_loaded_mesh_lets_it_load_again() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let a = ctx.gfx.load_mesh("models/house.glb");
            ctx.gfx.free(a);
            let b = ctx.gfx.load_mesh("models/house.glb");
            assert!(ctx.gfx.is_live(b));
        });
    }

    #[test]
    fn freed_mesh_slots_are_reused_under_new_ids() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let material = ctx.gfx.default_material();
            let cube = MeshData::cube(1.0);
            let first: Vec<Mesh> = (0..4).map(|_| ctx.gfx.create_mesh(&cube, material)).collect();
            ctx.gfx.set_mesh_shadows(first[1], false);
            ctx.gfx.free(first[1]);
            ctx.gfx.free(first[3]);
            // Streaming tiles in and out keeps the mesh tables the same size.
            let again = [
                ctx.gfx.create_mesh(&cube, material),
                ctx.gfx.create_mesh(&cube, material),
            ];
            let slot = |m: Mesh| __ffi::id(m) & SLOT_MASK;
            let slots = again.map(slot);
            assert!(
                slots == [slot(first[3]), slot(first[1])] || slots == [slot(first[1]), slot(first[3])],
                "{again:?}"
            );
            assert!(again.iter().all(|&m| ctx.gfx.is_live(m)));
            // But under new ids: the freed handles stay dead, and freeing
            // them again leaves the new meshes alone.
            assert!(again.iter().all(|m| !first.contains(m)));
            assert!(!ctx.gfx.is_live(first[1]) && !ctx.gfx.is_live(first[3]));
            ctx.gfx.free(first[1]);
            ctx.gfx.free(first[3]);
            assert!(again.iter().all(|&m| ctx.gfx.is_live(m)));
            // The new mesh doesn't inherit the old one's settings.
            let reused = if slot(again[0]) == slot(first[1]) {
                again[0]
            } else {
                again[1]
            };
            assert!(ctx.gfx.mesh_shadows(reused));
            // Past the freed ones, slots are new.
            let new = ctx.gfx.create_mesh(&cube, material);
            assert!(!first.iter().any(|&m| slot(m) == slot(new)));
        });
    }

    #[test]
    fn far_terrain_rebuilds_the_meshes_around_a_cut_and_frees_what_it_replaces() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let ground = ctx.gfx.default_material();
            // 256 squares of 16 m each way: 4 x 4 meshes. Cells of 128 m.
            let n = 257;
            let heights = vec![0.0; n * n];
            let mut far = FarTerrain::new(&heights, n, vec2(-2048.0, -2048.0), 16.0, 128.0, ground);
            far.update(ctx);
            assert_eq!(far.mesh_count(), 16);
            let models = |ctx: &mut Ctx| ctx.world.query::<&Model>().iter().count();
            assert_eq!(models(ctx), 16, "one entity each");
            let meshes: Vec<Mesh> = ctx.world.query::<&Model>().iter().map(|m| m.mesh).collect();
            // Cell (0, 0) starts in the middle: four meshes meet there, and
            // three of them only get its skirts.
            far.cut((0, 0));
            far.update(ctx);
            let after: Vec<Mesh> = ctx.world.query::<&Model>().iter().map(|m| m.mesh).collect();
            assert_eq!(after.len(), 16);
            assert!(after.iter().all(|&m| ctx.gfx.is_live(m)));
            // Freed ids are reused, so the old and new sets are the same size.
            assert_eq!(after.len(), meshes.len());
            // A cell whose mesh has nothing left goes away with its entity.
            let mut tiny = FarTerrain::new(&[0.0; 3 * 3], 3, Vec2::ZERO, 16.0, 32.0, ground);
            tiny.update(ctx);
            assert_eq!(tiny.mesh_count(), 1);
            tiny.cut((0, 0));
            tiny.update(ctx);
            assert_eq!(tiny.mesh_count(), 0);
            assert_eq!(models(ctx), 16);
            tiny.uncut((0, 0));
            tiny.update(ctx);
            assert_eq!(tiny.mesh_count(), 1);
            far.clear(ctx);
            tiny.clear(ctx);
            assert_eq!(models(ctx), 0);
            assert!(meshes.iter().chain(&after).all(|&m| !ctx.gfx.is_live(m)));
        });
    }
}

#[cfg(feature = "3d")]
#[test]
fn reloading_freed_geometry_reuses_materials_and_preserves_edits() {
    Game::new("reload", 1, 1).run_headless(0, |ctx| {
        let mut mesh = ctx.gfx.load_mesh("models/house.glb");
        let walls = ctx.gfx.loaded_materials("walls");
        let roof = ctx.gfx.loaded_materials("roof");
        assert!(!walls.is_empty());
        let (material, mut desc) = walls[0];
        desc.roughness = 0.17;
        ctx.gfx.update_material(material, desc);
        for _ in 0..32 {
            ctx.gfx.free(mesh);
            mesh = ctx.gfx.load_mesh("models/house.glb");
            assert_eq!(ctx.gfx.loaded_materials("walls").len(), walls.len());
            assert_eq!(ctx.gfx.loaded_materials("roof").len(), roof.len());
            assert_eq!(ctx.gfx.loaded_materials("walls")[0].0, material);
        }
    });
}
