//! One handle lifecycle: `Handle`, `Ctx::is_live`, `Ctx::free` and
//! `Ctx::live_counts` for every resource kind, in headless runs.
//!
//! Handles can't be made from numbers (the compile-fail examples on
//! `Handle` show it); a few tests read raw ids through the engine's hidden
//! binding hook (`ponko::__ffi`) only to check how slots are reused.

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform;
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{
    Compute, ComputePass, Effect, GpuBuffer, InstanceBatch, InstanceBatchOptions, Material, MaterialDesc, Mesh,
    MeshData, Model, ModelMaterial, ScreenEffect, Shader, TargetCamera,
};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::terrain::{TextureArray, TextureKind};

/// Writes the built-in font to a file `load_font` can read.
fn font_file(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("ponko-handles-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, epaint_default_fonts::UBUNTU_LIGHT).unwrap();
    path.to_str().unwrap().to_string()
}

/// Frees `handle`, then checks it is dead, stays dead when freed again,
/// and that `make` gives a live handle that isn't the freed one.
fn frees<H: Handle>(ctx: &mut Ctx, handle: H, make: impl Fn(&mut Ctx) -> H) {
    assert!(ctx.is_live(handle), "{handle:?} is live before free");
    ctx.free(handle);
    assert!(!ctx.is_live(handle), "{handle:?} is dead after free");
    ctx.free(handle); // again: nothing
    let next = make(ctx);
    assert!(ctx.is_live(next));
    assert_ne!(next, handle, "{handle:?}: handles are never given out twice");
    assert!(!ctx.is_live(handle), "{handle:?} stays dead");
    ctx.free(next);
}

/// Makes and frees a handle `cycles` times, as streaming does with one
/// cell, and checks that every handle freed stays dead while its slot is
/// reused: no handle is ever given out twice, well past any wraparound.
/// Only slot-reusing kinds need it: meshes, GPU buffers, collision meshes.
#[cfg(any(feature = "3d", feature = "physics3d"))]
fn stale_handles_stay_dead<H: Handle>(ctx: &mut Ctx, cycles: usize, make: impl Fn(&mut Ctx) -> H) {
    let first = make(ctx);
    ctx.free(first);
    let mut seen = std::collections::HashSet::from([first]);
    let mut last = first;
    for _ in 0..cycles {
        let next = make(ctx);
        assert!(seen.insert(next), "{next:?} was given out before");
        assert!(ctx.is_live(next) && !ctx.is_live(last) && !ctx.is_live(first));
        ctx.free(last); // a stale free leaves the new one alone
        assert!(ctx.is_live(next));
        ctx.free(next);
        last = next;
    }
    assert!(seen.iter().all(|&h| !ctx.is_live(h)));
    assert!(!seen.contains(&H::NONE));
}

#[test]
fn none_is_never_live_and_freeing_it_does_nothing() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let before = ctx.live_counts();
        assert!(!ctx.is_live(Texture::NONE));
        assert!(!ctx.is_live(Sound::NONE));
        assert!(!ctx.is_live(Font::NONE));
        ctx.free(Texture::NONE);
        ctx.free(Sound::NONE);
        ctx.free(Font::NONE);
        assert_eq!(ctx.live_counts(), before);
        // Playing nothing is silent, not a panic.
        ctx.audio.play(Sound::NONE);
        ctx.gfx
            .draw_text_ex("hi", vec2(0.0, 0.0), TextParams::default().with_font(Font::NONE));
    });
}

#[test]
fn textures_sounds_and_fonts_free_and_stay_dead() {
    let file = font_file("free.ttf");
    Game::new("t", 1, 1).run_headless(0, move |ctx| {
        let before = ctx.live_counts();
        let pixel = |ctx: &mut Ctx| ctx.gfx.create_texture(1, 1, &[255; 4]);
        let t = pixel(ctx);
        frees(ctx, t, pixel);
        let beep = |ctx: &mut Ctx| ctx.audio.create(8_000, &[0.0; 80]);
        let s = beep(ctx);
        frees(ctx, s, beep);
        let font = ctx.gfx.load_font(&file);
        frees(ctx, font, |ctx| ctx.gfx.load_font(&file));
        assert_eq!(ctx.live_counts(), before, "everything made was freed");
    });
}

#[test]
fn the_services_answer_for_their_own_kinds() {
    let file = font_file("services.ttf");
    Game::new("t", 1, 1).run_headless(0, move |ctx| {
        let t = ctx.gfx.create_texture(1, 1, &[255; 4]);
        let font = ctx.gfx.load_font(&file);
        let beep = ctx.audio.create(8_000, &[0.0; 80]);
        assert!(ctx.gfx.is_live(t) && ctx.gfx.is_live(font) && ctx.audio.is_live(beep));
        assert_eq!(ctx.live_counts().sounds, 1);
        assert_eq!(ctx.gfx.live_counts().sounds, 0, "sounds belong to the audio service");
        ctx.gfx.free(t);
        ctx.gfx.free(font);
        ctx.audio.free(beep);
        assert!(!ctx.is_live(t) && !ctx.is_live(font) && !ctx.is_live(beep));
        ctx.audio.free(beep); // twice does nothing
        ctx.gfx.free(Texture::NONE);
        assert_eq!(ctx.live_counts().sounds, 0);
    });
}

#[test]
fn the_built_in_font_is_never_freed() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.free(Font::DEFAULT);
        assert!(ctx.is_live(Font::DEFAULT));
        assert!(ctx.gfx.is_live(Font::DEFAULT));
    });
}

#[test]
fn text_in_a_freed_font_uses_the_built_in_one() {
    let file = font_file("text.ttf");
    Game::new("t", 1, 1).run_headless(0, move |ctx| {
        let font = ctx.gfx.load_font(&file);
        let size = ctx.gfx.measure_text_ex("Freed", font, 20.0);
        ctx.free(font);
        assert!(!ctx.gfx.is_live(font));
        assert_eq!(
            ctx.gfx.measure_text_ex("Freed", font, 20.0),
            size,
            "same metrics as the built-in font"
        );
        // Loading the file again loads it afresh, with a new id.
        let again = ctx.gfx.load_font(&file);
        assert_ne!(again, font);
        assert!(ctx.is_live(again));
    });
}

#[test]
fn live_counts_follow_creates_and_frees() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        let before = ctx.live_counts();
        let a = ctx.gfx.create_texture(1, 1, &[255; 4]);
        let _b = ctx.gfx.create_texture(1, 1, &[255; 4]);
        let s = ctx.audio.create(8_000, &[0.0; 8]);
        let now = ctx.live_counts();
        assert_eq!(now.textures, before.textures + 2);
        assert_eq!(now.sounds, before.sounds + 1);
        ctx.free(a);
        ctx.free(s);
        let after = ctx.live_counts();
        assert_eq!(after.textures, before.textures + 1);
        assert_eq!(after.sounds, before.sounds);
    });
}

#[cfg(feature = "3d")]
mod three {
    // Raw ids, only to see which slot a handle took.
    use ponko::__ffi::{self, SLOT_MASK};
    use ponko::terrain::TerrainTile;

    use super::*;

    // The slot a handle took: its raw id's low bits.
    macro_rules! slot {
        ($handle:expr) => {
            __ffi::id($handle) & SLOT_MASK
        };
    }

    const EFFECT: &str = "fn effect(p: Pixel) -> vec3<f32> { return p.color; }";
    const COMPUTE: &str = "fn compute(c: Cell) -> vec4<f32> { return c.value; }";
    const SHADER: &str = "fn surface(s: Surface) -> Surface { return s; }";

    #[test]
    fn every_3d_handle_kind_frees_and_stays_dead() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let white = ctx.gfx.default_material();
            let before = ctx.live_counts();
            let cube = |ctx: &mut Ctx| {
                let m = ctx.gfx.default_material();
                ctx.gfx.create_mesh(&MeshData::cube(1.0), m)
            };
            let mesh = cube(ctx);
            frees(ctx, mesh, cube);
            let plain = |ctx: &mut Ctx| ctx.gfx.create_material(MaterialDesc::default());
            let material = plain(ctx);
            frees(ctx, material, plain);
            let shader = |ctx: &mut Ctx| ctx.gfx.create_shader("s", SHADER);
            let s = shader(ctx);
            frees(ctx, s, shader);
            let compute = |ctx: &mut Ctx| ctx.gfx.create_compute("c", COMPUTE);
            let c = compute(ctx);
            frees(ctx, c, compute);
            let effect = |ctx: &mut Ctx| ctx.gfx.create_effect("e", EFFECT);
            let e = effect(ctx);
            frees(ctx, e, effect);
            let buffer = |ctx: &mut Ctx| ctx.gfx.create_gpu_buffer(4, &[]);
            let b = buffer(ctx);
            frees(ctx, b, buffer);
            let cube_mesh = cube(ctx);
            let batch = move |ctx: &mut Ctx| {
                ctx.gfx
                    .create_instance_batch(cube_mesh, &[Mat4::IDENTITY], InstanceBatchOptions::default())
            };
            let i = batch(ctx);
            frees(ctx, i, batch);
            ctx.free(cube_mesh);
            let array = |ctx: &mut Ctx| ctx.gfx.create_texture_array(1, 1, &[&[255; 4]], TextureKind::Color);
            let a = array(ctx);
            frees(ctx, a, array);
            let target = |ctx: &mut Ctx| ctx.gfx.create_render_target(8, 8);
            let t = target(ctx);
            assert!(ctx.gfx.is_render_target(t));
            frees(ctx, t, target);
            assert!(!ctx.gfx.is_render_target(t));
            assert_eq!(ctx.live_counts(), before, "everything made was freed");
            assert!(ctx.is_live(white));
            // C's PONKO_HANDLE_NONE.
            for none in [
                __ffi::id(Mesh::NONE),
                __ffi::id(Material::NONE),
                __ffi::id(GpuBuffer::NONE),
            ] {
                assert_eq!(none, u32::MAX);
            }
            assert!(!ctx.is_live(Mesh::NONE) && !ctx.is_live(Material::NONE) && !ctx.is_live(Shader::NONE));
            assert!(!ctx.is_live(Compute::NONE) && !ctx.is_live(Effect::NONE) && !ctx.is_live(GpuBuffer::NONE));
            assert!(!ctx.is_live(InstanceBatch::NONE) && !ctx.is_live(TextureArray::NONE));
        });
    }

    /// A freed mesh or buffer slot is reused under a new id, so a handle
    /// kept past `free` never reaches the resource made after it.
    #[test]
    fn stale_mesh_and_buffer_handles_never_reach_the_next_resource() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let material = ctx.gfx.default_material();
            let old = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            ctx.free(old);
            let new = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            assert_eq!(slot!(new), slot!(old), "the same slot");
            assert_ne!(new, old);
            assert!(!ctx.is_live(old) && ctx.is_live(new));
            // Calls with the stale handle leave the new mesh alone.
            ctx.gfx.set_mesh_shadows(old, false);
            assert!(ctx.gfx.mesh_shadows(new));
            ctx.gfx.set_mesh_shadows(new, false);
            assert!(ctx.gfx.mesh_shadows(old), "a stale handle reads the default");
            ctx.free(old);
            assert!(ctx.is_live(new));
            assert_eq!(ctx.live_counts().meshes, 1);

            let old = ctx.gfx.create_gpu_buffer(4, &[]);
            ctx.free(old);
            let new = ctx.gfx.create_gpu_buffer(2, &[]);
            assert_eq!(slot!(new), slot!(old), "the same slot");
            assert_eq!(ctx.gfx.gpu_buffer_len(old), None);
            ctx.free(old);
            assert_eq!(ctx.gfx.gpu_buffer_len(new), Some(2));
        });
    }

    /// Streaming one mesh in and out keeps reusing its slot, so the tables
    /// stay small, under a new id each time: 4095 times, then the slot is
    /// retired and the next one takes over. No id comes back.
    #[test]
    fn a_reused_slot_counts_its_generations_then_retires() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let material = ctx.gfx.default_material();
            let mut ids = std::collections::HashSet::new();
            for i in 0..5000 {
                let m = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
                assert_eq!(slot!(m), u32::from(i >= 4095), "made {i}");
                assert_ne!(m, Mesh::NONE);
                assert!(ids.insert(m));
                ctx.free(m);
            }
        });
    }

    /// Meshes and GPU buffers that a streamed cell makes and frees over and
    /// over: a handle kept past `free` never names a later resource.
    #[test]
    fn stale_mesh_and_buffer_handles_stay_dead_across_many_slot_reuses() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let material = ctx.gfx.default_material();
            stale_handles_stay_dead(ctx, 10_000, |ctx| ctx.gfx.create_mesh(&MeshData::cube(1.0), material));
            stale_handles_stay_dead(ctx, 10_000, |ctx| ctx.gfx.create_gpu_buffer(4, &[]));
        });
    }

    /// A terrain tile is one handle for its meshes: live while they all
    /// are, and freed with them.
    #[test]
    fn terrain_tiles_free_all_their_meshes() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let ground = ctx.gfx.default_material();
            let before = ctx.live_counts();
            let tile = |ctx: &mut Ctx| ctx.gfx.create_terrain_tile(&[0.0; 9 * 9], 16.0, ground);
            let t = tile(ctx);
            assert_eq!(ctx.live_counts().meshes, before.meshes + TerrainTile::LODS);
            assert!(t.lods().iter().all(|&m| ctx.gfx.is_live(m)));
            frees(ctx, t, tile);
            assert!(t.lods().iter().all(|&m| !ctx.is_live(m)));
            assert!(!ctx.is_live(TerrainTile::NONE));
            ctx.free(TerrainTile::NONE);
            assert_eq!(ctx.live_counts(), before);
            // A tile is the meshes it frees: another distance is the same tile.
            let t = tile(ctx);
            let far = t.with_lod_distance(t.lod_distance() * 4.0);
            assert_eq!(far, t);
            assert!(std::collections::HashSet::from([t]).contains(&far));
            let other = tile(ctx);
            assert_ne!(other, t);
            ctx.free(far);
            assert!(!ctx.is_live(t));
            ctx.free(other);
            assert_eq!(ctx.live_counts(), before);
            // One of its meshes freed on its own: the tile is no longer whole.
            let t = tile(ctx);
            ctx.free(t.lods()[3]);
            assert!(!ctx.gfx.is_live(t));
            ctx.gfx.free(t);
            assert_eq!(ctx.live_counts(), before);
        });
    }

    #[test]
    fn the_graphics_service_frees_every_3d_kind() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let before = ctx.gfx.live_counts();
            let material = ctx.gfx.create_material(MaterialDesc::default());
            let shader = ctx.gfx.create_shader("s", SHADER);
            let compute = ctx.gfx.create_compute("c", COMPUTE);
            let effect = ctx.gfx.create_effect("e", EFFECT);
            let array = ctx.gfx.create_texture_array(1, 1, &[&[255; 4]], TextureKind::Color);
            let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            let batch = ctx
                .gfx
                .create_instance_batch(mesh, &[], InstanceBatchOptions::default());
            let buffer = ctx.gfx.create_gpu_buffer(4, &[]);
            let target = ctx.gfx.create_render_target(4, 4);
            assert!(ctx.gfx.is_live(material) && ctx.gfx.is_live(shader) && ctx.gfx.is_live(compute));
            assert!(ctx.gfx.is_live(effect) && ctx.gfx.is_live(array) && ctx.gfx.is_live(mesh));
            assert!(ctx.gfx.is_live(batch) && ctx.gfx.is_live(buffer) && ctx.gfx.is_live(target));
            ctx.gfx.free(material);
            ctx.gfx.free(shader);
            ctx.gfx.free(compute);
            ctx.gfx.free(effect);
            ctx.gfx.free(array);
            ctx.gfx.free(batch);
            ctx.gfx.free(mesh);
            ctx.gfx.free(buffer);
            ctx.gfx.free(target);
            assert!(!ctx.gfx.is_live(material) && !ctx.gfx.is_live(shader) && !ctx.gfx.is_live(compute));
            assert!(!ctx.gfx.is_live(effect) && !ctx.gfx.is_live(array) && !ctx.gfx.is_live(mesh));
            assert!(!ctx.gfx.is_live(batch) && !ctx.gfx.is_live(buffer) && !ctx.gfx.is_live(target));
            // The default material is made lazily by `create_mesh`'s callers
            // only when asked; this test made its own, so counts match.
            assert_eq!(ctx.gfx.live_counts(), before);
        });
    }

    /// A buffer freed through the service keeps its id out of the pool
    /// until the compute passes writing to it are gone, so a stale pass
    /// never writes into the next buffer that takes the id.
    #[test]
    fn a_buffer_freed_through_the_service_loses_its_passes_before_its_id_is_reused() {
        let checked = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = checked.clone();
        Game::new("t", 1, 1)
            .sim_system(move |ctx: &mut Ctx| {
                if !seen.get() {
                    assert!(
                        ctx.scene3d.passes.compute.is_empty(),
                        "the next frame removed the stale pass"
                    );
                    let made = ctx.gfx.create_gpu_buffer(4, &[]);
                    assert_eq!(slot!(made), 0, "and the slot is back in the pool");
                    assert_ne!(__ffi::id(made), 0, "under a new id");
                    seen.set(true);
                }
            })
            .run_headless(3, |ctx| {
                let compute = ctx.gfx.create_compute("c", COMPUTE);
                let old = ctx.gfx.create_gpu_buffer(4, &[]);
                assert_eq!(__ffi::id(old), 0);
                ctx.scene3d.passes.compute.push(ComputePass::for_buffer(compute, old));
                ctx.gfx.free(old);
                let next = ctx.gfx.create_gpu_buffer(4, &[]);
                assert_ne!(
                    __ffi::id(next),
                    __ffi::id(old),
                    "the id waits until its passes are removed"
                );
                assert_eq!(
                    ctx.scene3d.passes.compute.len(),
                    1,
                    "the service can't reach Ctx's passes"
                );
            });
        assert!(checked.get());
    }

    /// Spawns models that use one of each kind, frees them all, and keeps
    /// stepping: nothing panics. (The offscreen render test checks what
    /// such models draw.)
    #[test]
    fn models_whose_resources_were_freed_still_step() {
        Game::new("t", 64, 64)
            .sim_system(|ctx: &mut Ctx| {
                let models = ctx.world.query::<&Model>().iter().count();
                assert_eq!(models, 2);
                ctx.gfx.draw_text("still here", vec2(0.0, 0.0), 16.0, Color::WHITE);
            })
            .run_headless(3, |ctx| {
                let image = ctx.gfx.create_texture(2, 2, &[255; 16]);
                let normal = ctx.gfx.create_texture(2, 2, &[128; 16]);
                let shader = ctx.gfx.create_shader("s", SHADER);
                let array = ctx.gfx.create_texture_array(1, 1, &[&[255; 4]], TextureKind::Color);
                let material = ctx.gfx.create_material(
                    MaterialDesc::default()
                        .with_texture(image)
                        .with_normal_map(normal)
                        .with_shader(shader)
                        .with_custom_arrays([Some(array), None, None]),
                );
                let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
                let other = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
                let target = ctx.gfx.create_render_target(16, 16);
                let effect = ctx.gfx.create_effect("e", EFFECT);
                let compute = ctx.gfx.create_compute("c", COMPUTE);
                ctx.scene3d
                    .passes
                    .effects
                    .push(ScreenEffect::new(effect).with_material(material));
                ctx.scene3d.passes.compute.push(ComputePass::new(compute, target));
                ctx.scene3d
                    .passes
                    .target_cameras
                    .push(TargetCamera::new(ctx.scene3d.camera, target));
                ctx.world.spawn((Transform::default(), Model::new(mesh)));
                // Drawn with the material as an override too (ModelMaterial).
                ctx.world.spawn((
                    Transform::at(vec3(2.0, 0.0, 0.0)),
                    Model::new(other),
                    ModelMaterial(material),
                ));
                for h in [image, normal, target] {
                    ctx.free(h);
                }
                ctx.free(shader);
                ctx.free(array);
                ctx.free(material);
                ctx.free(mesh);
                ctx.free(effect);
                ctx.free(compute);
                // A freed material can't be brought back by updating it.
                ctx.gfx.update_material(material, MaterialDesc::default());
                assert!(!ctx.is_live(material));
                // A freed texture takes no pixels.
                ctx.gfx.update_texture(image, &[0; 16]);
                // New materials may still name freed handles: they draw as defaults.
                let late = ctx
                    .gfx
                    .create_material(MaterialDesc::default().with_texture(image).with_shader(shader));
                assert!(ctx.is_live(late));
                assert!(ctx.is_live(other));
            });
    }

    #[test]
    fn freeing_the_default_material_makes_a_new_one() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let white = ctx.gfx.default_material();
            ctx.free(white);
            let again = ctx.gfx.default_material();
            assert_ne!(again, white);
            assert!(ctx.is_live(again));
        });
    }

    #[test]
    fn freeing_decal_textures_clears_them() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let images = ctx.gfx.create_texture_array(1, 1, &[&[255; 4]], TextureKind::Data);
            ctx.gfx.set_decal_textures(Some(images));
            ctx.free(images);
            assert_eq!(ctx.gfx.decal_textures(), None);
        });
    }

    #[test]
    fn a_freed_loaded_mesh_loads_again() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let house = ctx.gfx.load_mesh("models/house.glb");
            let counts = ctx.live_counts();
            ctx.free(house);
            assert!(!ctx.is_live(house));
            assert_eq!(ctx.live_counts().meshes, counts.meshes - 1);
            // Loaded afresh (into the freed id, which the next mesh takes).
            let again = ctx.gfx.load_mesh("models/house.glb");
            assert!(ctx.is_live(again));
            assert_eq!(ctx.live_counts().meshes, counts.meshes);
        });
    }

    #[test]
    fn a_streamed_out_cell_frees_its_resources() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            // ANCHOR: handles
            let before = ctx.live_counts();
            let bricks = ctx.gfx.create_texture(1, 1, &[200, 80, 60, 255]);
            let wall = ctx.gfx.create_material(MaterialDesc::default().with_texture(bricks));
            let block = ctx.gfx.create_mesh(&MeshData::cube(1.0), wall);
            ctx.world.spawn((Transform::default(), Model::new(block)));
            // Going out of range: free what the cell made. The model still
            // draws (untextured), until its entity is despawned too.
            ctx.free(bricks);
            ctx.free(wall);
            ctx.free(block);
            assert!(!ctx.is_live(bricks) && !ctx.is_live(wall) && !ctx.is_live(block));
            assert!(!ctx.is_live(Texture::NONE));
            assert_eq!(ctx.live_counts(), before);
            // ANCHOR_END: handles
        });
    }
}

#[cfg(feature = "physics3d")]
mod collision {
    use ponko::d3::CollisionMesh;

    use super::*;

    fn ground(ctx: &mut Ctx) -> CollisionMesh {
        ctx.physics3d.heightfield(&[0.0; 4], 2.0)
    }

    #[test]
    fn collision_meshes_free_and_stay_dead() {
        Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(0, |ctx| {
            let before = ctx.live_counts();
            let m = ground(ctx);
            assert_eq!(ctx.live_counts().collision_meshes, before.collision_meshes + 1);
            assert!(ctx.physics3d.is_live(m));
            frees(ctx, m, ground);
            assert!(!ctx.physics3d.is_live(m));
            assert_eq!(ctx.live_counts(), before, "everything made was freed");
            // The service frees them as Ctx does.
            let m = ground(ctx);
            ctx.physics3d.free(m);
            ctx.physics3d.free(m); // again: nothing
            assert!(!ctx.is_live(m));
            assert!(!ctx.is_live(CollisionMesh::NONE));
            ctx.free(CollisionMesh::NONE);
            assert_eq!(ctx.live_counts(), before);
        });
    }

    #[test]
    fn saved_json_never_makes_or_shows_a_live_collision_mesh() {
        use ponko::d3::{Collider, Shape, TerrainCollider};
        use serde_json::json;
        Game::new("t", 1, 1).physics3d(Vec3::ZERO).run_headless(0, |ctx| {
            let m = ground(ctx); // the first mesh: the lowest id there is
            assert!(ctx.is_live(m));
            // Serde writes a mesh as null, so no number leaves the game...
            let collider = serde_json::to_value(Collider::mesh(m)).unwrap();
            assert_eq!(collider["shape"], json!({ "Mesh": null }));
            let terrain = serde_json::to_value(TerrainCollider::new(m)).unwrap();
            assert_eq!(terrain["mesh"], json!(null));
            // ...reads null back as NONE...
            let back: Collider = serde_json::from_value(collider.clone()).unwrap();
            assert_eq!(back.shape, Shape::Mesh(CollisionMesh::NONE));
            let back: TerrainCollider = serde_json::from_value(terrain.clone()).unwrap();
            assert!(!ctx.is_live(back.mesh));
            // ...and turns down numbers, whatever their form.
            for id in [json!({ "id": 0 }), json!(0), json!({ "index": 0, "generation": 0 })] {
                let mut c = collider.clone();
                c["shape"]["Mesh"] = id.clone();
                assert!(serde_json::from_value::<Collider>(c).is_err(), "{id}");
                let mut t = terrain.clone();
                t["mesh"] = id.clone();
                assert!(serde_json::from_value::<TerrainCollider>(t).is_err(), "{id}");
            }
            // Reflection (saves, the editor, agents) leaves out a collider of a
            // mesh made in code, and only takes a mesh by its file.
            let e = ctx.world.spawn((Collider::mesh(m),));
            assert!(ctx.world.entity_json(e).unwrap().get("Collider3D").is_none());
            for mesh in [json!({ "id": 0 }), json!(null)] {
                let saved = json!({ "Collider3D": { "shape": { "Mesh": mesh } } });
                assert!(ctx.world.spawn_json(saved).is_err(), "{mesh}");
            }
        });
    }

    #[test]
    fn stale_collision_meshes_stay_dead_across_many_slot_reuses() {
        Game::new("t", 1, 1)
            .physics3d(Vec3::ZERO)
            .run_headless(0, |ctx| stale_handles_stay_dead(ctx, 10_000, ground));
    }
}
