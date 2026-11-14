//! Asset handle accounting: live handles by class with estimated bytes, and
//! leak checks that create and free resources in a loop and expect the
//! inventory back at its baseline.

#[allow(unused_imports)]
// The inventory reports raw ids: these tests compare them with the handles'.
use ponko::__ffi;
use ponko::inventory::AssetClass;
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{GpuBuffer, Material, MaterialDesc, Mesh, MeshData, ModelMaterial};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::terrain::{TextureArray, TextureKind};

/// The records of `class`, from the service that holds it.
fn handles(ctx: &Ctx, class: AssetClass) -> Vec<ponko::inventory::AssetRecord> {
    match class {
        AssetClass::Sound => ctx.audio.asset_handles(),
        _ => ctx.gfx.asset_handles(class),
    }
}

fn headless(setup: impl FnOnce(&mut Ctx)) {
    Game::new("assets", 1, 1).run_headless(0, setup);
}

#[test]
fn textures_and_sounds_are_counted_with_their_bytes() {
    headless(|ctx| {
        let before = ctx.asset_inventory();
        assert!(before.gpu.is_none(), "headless runs have no GPU to count");
        let _a = ctx.gfx.create_texture(4, 2, &[0; 4 * 2 * 4]);
        let _b = ctx.gfx.create_texture(1, 1, &[0; 4]);
        let _beep = ctx.audio.create(1000, &[0.0; 100]);
        let after = ctx.asset_inventory();
        let textures = after.get(AssetClass::Texture);
        assert_eq!(textures.live, before.get(AssetClass::Texture).live + 2);
        assert_eq!(textures.bytes, before.get(AssetClass::Texture).bytes + (8 + 1) * 4);
        assert_eq!(
            after.get(AssetClass::Sound).live,
            before.get(AssetClass::Sound).live + 1
        );
        let grown: Vec<AssetClass> = after.growth_since(&before).iter().map(|g| g.class).collect();
        assert!(
            grown.contains(&AssetClass::Texture) && grown.contains(&AssetClass::Sound),
            "{grown:?}"
        );
        assert!(after.total_bytes() >= textures.bytes);
    });
}

#[test]
fn loading_the_same_texture_again_adds_nothing() {
    headless(|ctx| {
        let _ = ctx.gfx.load_texture("crab.png");
        let base = ctx.asset_inventory();
        for _ in 0..10 {
            let _ = ctx.gfx.load_texture("crab.png");
        }
        assert!(ctx.asset_inventory().growth_since(&base).is_empty());
    });
}

#[cfg(feature = "3d")]
mod three {
    use super::*;

    fn cube_bytes() -> u64 {
        let cube = MeshData::cube(1.0);
        cube.positions.len() as u64 * 44 + cube.indices.len() as u64 * 4
    }

    #[test]
    fn mesh_create_free_cycles_return_to_baseline() {
        headless(|ctx| {
            let material = ctx.gfx.default_material();
            let base = ctx.asset_inventory();
            for round in 0..200 {
                let meshes: Vec<Mesh> = (0..5)
                    .map(|i| ctx.gfx.create_mesh(&MeshData::cube(1.0 + i as f32), material))
                    .collect();
                let during = ctx.asset_inventory();
                assert_eq!(during.get(AssetClass::Mesh).live, base.get(AssetClass::Mesh).live + 5);
                assert_eq!(
                    during.get(AssetClass::Mesh).bytes,
                    base.get(AssetClass::Mesh).bytes + 5 * cube_bytes()
                );
                for mesh in meshes {
                    ctx.gfx.free(mesh);
                    ctx.gfx.free(mesh); // twice: still freed once
                }
                let growth = ctx.asset_inventory().growth_since(&base);
                assert!(growth.is_empty(), "round {round}: {growth:?}");
            }
            assert_eq!(ctx.asset_inventory(), base);
        });
    }

    #[test]
    fn a_leak_names_the_class_that_grows() {
        headless(|ctx| {
            let material = ctx.gfx.default_material();
            let base = ctx.asset_inventory();
            for _ in 0..3 {
                let kept = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
                let freed = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
                ctx.gfx.free(freed);
                let _ = kept; // never freed: the leak
            }
            let growth = ctx.asset_inventory().growth_since(&base);
            assert_eq!(growth.len(), 1, "{growth:?}");
            assert_eq!(growth[0].class, AssetClass::Mesh);
            assert_eq!(growth[0].live, 3);
            assert_eq!(growth[0].bytes, 3 * cube_bytes() as i64);
        });
    }

    #[test]
    fn gltf_free_and_reload_cycles_do_not_grow() {
        headless(|ctx| {
            // The first load makes the file's textures and materials; they
            // are kept for the next load of the same file.
            let house = ctx.gfx.load_mesh("models/house.glb");
            ctx.gfx.free(house);
            let base = ctx.asset_inventory();
            for round in 0..20 {
                let house = ctx.gfx.load_mesh("models/house.glb");
                assert!(ctx.gfx.is_live(house));
                let during = ctx.asset_inventory();
                assert_eq!(during.get(AssetClass::Mesh).live, base.get(AssetClass::Mesh).live + 1);
                assert_eq!(during.get(AssetClass::Material), base.get(AssetClass::Material));
                assert_eq!(during.get(AssetClass::Texture), base.get(AssetClass::Texture));
                ctx.gfx.free(house);
                let growth = ctx.asset_inventory().growth_since(&base);
                assert!(growth.is_empty(), "round {round}: {growth:?}");
            }
        });
    }

    #[test]
    fn gpu_buffer_cycles_return_to_baseline() {
        headless(|ctx| {
            let base = ctx.asset_inventory();
            for len in 1..100u32 {
                let buffer = ctx.gfx.create_gpu_buffer(len, &[]);
                let during = ctx.asset_inventory().get(AssetClass::GpuBuffer);
                assert_eq!((during.live, during.bytes), (1, u64::from(len) * 16));
                ctx.free(buffer);
            }
            assert!(ctx.asset_inventory().growth_since(&base).is_empty());
        });
    }

    #[test]
    fn never_freed_classes_count_what_was_made() {
        headless(|ctx| {
            let base = ctx.asset_inventory();
            let _ = ctx.gfx.create_material(MaterialDesc::default());
            let _ = ctx.gfx.create_render_target(16, 8);
            let _ = ctx
                .gfx
                .create_texture_array(2, 2, &[&[0; 16], &[0; 16]], TextureKind::Color);
            let now = ctx.asset_inventory();
            assert_eq!(
                now.get(AssetClass::Material).live,
                base.get(AssetClass::Material).live + 1
            );
            let target = now.get(AssetClass::RenderTarget);
            assert_eq!((target.live, target.bytes), (1, 16 * 8 * 8));
            // Two layers of 2x2 and 1x1 mipmaps.
            let arrays = now.get(AssetClass::TextureArray);
            assert_eq!((arrays.live, arrays.bytes), (1, 2 * (4 + 1) * 4));
        });
    }
}

#[test]
fn freed_handles_leave_the_inventory() {
    headless(|ctx| {
        let base = ctx.asset_inventory();
        for _ in 0..3 {
            let texture = ctx.gfx.create_texture(4, 4, &[0; 4 * 4 * 4]);
            let beep = ctx.audio.create(1000, &[0.0; 100]);
            assert_eq!(
                ctx.asset_inventory().get(AssetClass::Texture).live,
                base.get(AssetClass::Texture).live + 1
            );
            ctx.free(texture);
            ctx.free(beep);
        }
        let after = ctx.asset_inventory();
        for class in [AssetClass::Texture, AssetClass::Sound] {
            assert_eq!(
                (after.get(class).live, after.get(class).bytes),
                (base.get(class).live, base.get(class).bytes),
                "{class:?}"
            );
        }
        assert!(after.growth_since(&base).is_empty());
    });
}

/// The inventory and `live_counts` read the same handle tables: every class
/// but textures (which add the engine's own) agrees, before and after frees.
#[test]
fn the_inventory_agrees_with_live_counts() {
    fn agree(ctx: &Ctx) {
        let inv = ctx.asset_inventory();
        let live = ctx.live_counts();
        assert_eq!(inv.get(AssetClass::Sound).live, live.sounds);
        assert!(inv.get(AssetClass::Texture).live >= live.textures - live.render_targets);
        #[cfg(feature = "3d")]
        {
            assert_eq!(inv.get(AssetClass::RenderTarget).live, live.render_targets);
            assert_eq!(inv.get(AssetClass::TextureArray).live, live.texture_arrays);
            assert_eq!(inv.get(AssetClass::Material).live, live.materials);
            assert_eq!(inv.get(AssetClass::Shader).live, live.shaders);
            assert_eq!(inv.get(AssetClass::Mesh).live, live.meshes);
            assert_eq!(inv.get(AssetClass::GpuBuffer).live, live.gpu_buffers);
            assert_eq!(inv.get(AssetClass::Effect).live, live.effects);
            assert_eq!(inv.get(AssetClass::Compute).live, live.computes);
        }
    }
    headless(|ctx| {
        let t = ctx.gfx.create_texture(1, 1, &[0; 4]);
        let s = ctx.audio.create(1000, &[0.0; 10]);
        #[cfg(feature = "3d")]
        let made = {
            let material = ctx.gfx.create_material(MaterialDesc::default());
            let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            let buffer = ctx.gfx.create_gpu_buffer(4, &[]);
            let target = ctx.gfx.create_render_target(4, 4);
            (material, mesh, buffer, target)
        };
        agree(ctx);
        ctx.gfx.free(t);
        ctx.audio.free(s);
        #[cfg(feature = "3d")]
        {
            ctx.gfx.free(made.0);
            ctx.gfx.free(made.1);
            ctx.gfx.free(made.2);
            ctx.gfx.free(made.3);
        }
        agree(ctx);
    });
}

#[test]
fn sprites_showing_a_freed_texture_are_listed_as_dangling() {
    headless(|ctx| {
        let kept = ctx.gfx.create_texture(1, 1, &[0; 4]);
        let freed = ctx.gfx.create_texture(1, 1, &[0; 4]);
        ctx.world.spawn((Sprite::new(kept),));
        let holder = ctx.world.spawn((Sprite::new(freed),));
        ctx.world.spawn((Sprite::new(Texture::NONE),));
        assert!(ctx.asset_inventory().dangling.is_empty());
        ctx.free(freed);
        let dangling = ctx.asset_inventory().dangling;
        assert_eq!(dangling.len(), 1, "{dangling:?}");
        assert_eq!(dangling[0].entity, holder.to_bits());
        assert_eq!(
            (dangling[0].component, dangling[0].class),
            ("Sprite", AssetClass::Texture)
        );
        assert_eq!(dangling[0].id, __ffi::id(freed));
        ctx.world.despawn(holder);
        assert!(ctx.asset_inventory().dangling.is_empty());
    });
}

#[cfg(feature = "3d")]
#[test]
fn models_showing_a_freed_mesh_or_material_are_listed_as_dangling() {
    headless(|ctx| {
        let material = ctx.gfx.create_material(MaterialDesc::default());
        let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        let other = ctx.gfx.create_material(MaterialDesc::default());
        let e = ctx
            .world
            .spawn((d3::Transform::default(), Model::new(mesh), ModelMaterial(other)));
        ctx.world.spawn((d3::Transform::default(), Model::new(Mesh::NONE)));
        assert!(ctx.asset_inventory().dangling.is_empty());
        ctx.free(other);
        ctx.free(mesh);
        let found: Vec<_> = ctx
            .asset_inventory()
            .dangling
            .iter()
            .map(|d| (d.entity, d.component, d.class))
            .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.contains(&(e.to_bits(), "Model", AssetClass::Mesh)));
        assert!(found.contains(&(e.to_bits(), "ModelMaterial", AssetClass::Material)));
    });
}

/// A freed mesh's slot is reused under a new id: a model that kept the old
/// handle still shows a dead mesh (listed as dangling, drawn as nothing),
/// not the mesh made after it.
#[cfg(feature = "3d")]
#[test]
fn a_model_keeping_a_freed_mesh_never_shows_the_next_one() {
    headless(|ctx| {
        let material = ctx.gfx.default_material();
        let old = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        let holder = ctx.world.spawn((d3::Transform::default(), Model::new(old)));
        ctx.free(old);
        let new = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
        let slot = ponko::__ffi::SLOT_MASK;
        assert_eq!(__ffi::id(new) & slot, __ffi::id(old) & slot, "the same slot");
        let dangling = ctx.asset_inventory().dangling;
        assert_eq!(dangling.len(), 1, "{dangling:?}");
        assert_eq!((dangling[0].entity, dangling[0].id), (holder.to_bits(), __ffi::id(old)));
        ctx.world.get_mut::<Model>(holder).unwrap().mesh = new;
        assert!(ctx.asset_inventory().dangling.is_empty());
    });
}

/// Records list every live handle of a class with its bytes, the frame it
/// was made on and who made it; freed handles leave them.
#[test]
fn records_say_when_and_by_whom_each_handle_was_made() {
    use ponko::inventory::AssetOwner;
    let checked = std::rc::Rc::new(std::cell::Cell::new(false));
    let done = checked.clone();
    Game::new("assets", 1, 1)
        .hot_reload(false)
        .sim_system(move |ctx: &mut Ctx| {
            let frame = ctx.time.frame();
            if frame != 3 {
                return;
            }
            let made = ctx.gfx.create_texture(2, 1, &[0; 8]);
            let beep = ctx.audio.create(1000, &[0.0; 100]);
            let records = ctx.gfx.asset_handles(AssetClass::Texture);
            let loaded = records
                .iter()
                .find(|r| r.owner == AssetOwner::File("crab.png".into()))
                .expect("the loaded texture");
            assert!(loaded.made_frame < frame && loaded.age_frames == frame - loaded.made_frame);
            let new = records.iter().find(|r| r.id == __ffi::id(made)).unwrap();
            assert_eq!((new.made_frame, new.age_frames, new.bytes), (frame, 0, 8));
            assert_eq!(new.owner, AssetOwner::Code);
            assert_eq!(records.last().unwrap().id, __ffi::id(made), "oldest first");
            let sound = ctx.audio.asset_handles();
            assert_eq!(sound.len(), 1);
            assert_eq!((sound[0].id, sound[0].made_frame), (__ffi::id(beep), frame));
            ctx.free(made);
            ctx.free(beep);
            assert!(
                ctx.gfx
                    .asset_handles(AssetClass::Texture)
                    .iter()
                    .all(|r| r.id != __ffi::id(made))
            );
            assert!(ctx.audio.asset_handles().is_empty());
            done.set(true);
        })
        .run_headless(5, |ctx| {
            let _ = ctx.gfx.load_texture("crab.png");
        });
    assert!(checked.get());
}

/// Every class's records match its inventory count and bytes, before and
/// after freeing: no live handle is missing from the records.
#[test]
fn records_cover_every_live_handle_of_every_class() {
    fn agree(ctx: &Ctx) {
        let inventory = ctx.asset_inventory();
        for count in &inventory.classes {
            let records = handles(ctx, count.class);
            assert_eq!(records.len(), count.live, "{:?}", count.class);
            assert_eq!(
                records.iter().map(|r| r.bytes).sum::<u64>(),
                count.bytes,
                "{:?}",
                count.class
            );
        }
    }
    headless(|ctx| {
        let t = ctx.gfx.create_texture(1, 1, &[0; 4]);
        let s = ctx.audio.create(1000, &[0.0; 10]);
        #[cfg(feature = "3d")]
        let made = {
            const SHADER: &str = "fn surface(s: Surface) -> Surface { return s; }";
            const EFFECT: &str = "fn effect(p: Pixel) -> vec3<f32> { return p.color; }";
            const COMPUTE: &str = "fn compute(c: Cell) -> vec4<f32> { return c.value; }";
            let material = ctx.gfx.create_material(MaterialDesc::default());
            let mesh = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
            let buffer = ctx.gfx.create_gpu_buffer(4, &[]);
            let target = ctx.gfx.create_render_target(4, 4);
            let array = ctx.gfx.create_texture_array(1, 1, &[&[0; 4]], TextureKind::Color);
            let shader = ctx.gfx.create_shader("s", SHADER);
            let effect = ctx.gfx.create_effect("e", EFFECT);
            let compute = ctx.gfx.create_compute("c", COMPUTE);
            (material, mesh, buffer, target, array, shader, effect, compute)
        };
        agree(ctx);
        ctx.free(t);
        ctx.free(s);
        #[cfg(feature = "3d")]
        {
            ctx.free(made.0);
            ctx.free(made.1);
            ctx.free(made.2);
            ctx.free(made.3);
            ctx.free(made.4);
            ctx.free(made.5);
            ctx.free(made.6);
            ctx.free(made.7);
        }
        agree(ctx);
    });
}

#[cfg(feature = "3d")]
mod files {
    use super::*;
    use ponko::inventory::AssetOwner;

    const HOUSE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/models/house.glb"));

    #[test]
    fn a_files_materials_and_textures_are_owned_by_it() {
        headless(|ctx| {
            let house = ctx.gfx.load_mesh("models/house.glb");
            let owner = AssetOwner::File("models/house.glb".into());
            let meshes = ctx.gfx.asset_handles(AssetClass::Mesh);
            assert!(meshes.iter().any(|r| r.id == __ffi::id(house) && r.owner == owner));
            let materials = ctx.gfx.asset_handles(AssetClass::Material);
            assert!(!materials.is_empty() && materials.iter().all(|r| r.owner == owner));
        });
    }

    /// Freeing one of a file's materials and loading the file again makes
    /// that material again, not a whole new set beside the old one.
    #[test]
    fn freeing_a_files_material_then_reloading_it_does_not_grow() {
        headless(|ctx| {
            let mut house = ctx.gfx.load_mesh("models/house.glb");
            let base = ctx.asset_inventory();
            for round in 0..20 {
                ctx.free(house);
                let material = ctx.gfx.asset_handles(AssetClass::Material)[0].id;
                ctx.free(__ffi::from_id::<Material>(material));
                house = ctx.gfx.load_mesh("models/house.glb");
                assert!(ctx.gfx.is_live(house));
                let growth = ctx.asset_inventory().growth_since(&base);
                assert!(growth.is_empty(), "round {round}: {growth:?}");
            }
        });
    }

    /// A long-running game loading many different models: `unload_mesh`
    /// gives back everything each one made.
    #[test]
    fn unloading_many_different_files_returns_to_baseline() {
        headless(|ctx| {
            let mut pack = MemorySource::new("pack");
            for i in 0..30 {
                pack = pack.with(format!("models/house{i}.glb"), HOUSE);
            }
            ctx.assets.add_source(pack);
            let base = ctx.asset_inventory();
            for round in 0..3 {
                for i in 0..30 {
                    let name = format!("models/house{i}.glb");
                    let house = ctx.gfx.load_mesh(&name);
                    ctx.world.spawn((d3::Transform::default(), Model::new(house)));
                }
                assert!(!ctx.asset_inventory().growth_since(&base).is_empty());
                for i in 0..30 {
                    assert!(ctx.gfx.unload_mesh(&format!("models/house{i}.glb")));
                }
                let growth = ctx.asset_inventory().growth_since(&base);
                assert!(growth.is_empty(), "round {round}: {growth:?}");
                assert!(!ctx.gfx.unload_mesh("models/house0.glb"), "unloaded already");
                // The models still name the freed meshes: listed until despawned.
                assert_eq!(ctx.asset_inventory().dangling.len(), 30 * (round + 1));
            }
        });
    }

    /// Freeing only the mesh keeps the file's materials (and their edits)
    /// for the next load; unloading after that frees them too.
    #[test]
    fn unload_after_free_releases_the_kept_materials() {
        headless(|ctx| {
            let base = ctx.asset_inventory();
            let house = ctx.gfx.load_mesh("models/house.glb");
            ctx.free(house);
            let kept = ctx.asset_inventory().growth_since(&base);
            assert!(kept.iter().any(|g| g.class == AssetClass::Material), "{kept:?}");
            assert!(ctx.gfx.unload_mesh("models/house.glb"));
            assert!(ctx.asset_inventory().growth_since(&base).is_empty());
            assert!(!ctx.gfx.unload_mesh("models/missing.glb"));
        });
    }
}

/// The guide's leak hunt: compare inventories, then list the handles of
/// the class that grew with their age and owner.
#[cfg(feature = "3d")]
#[test]
fn the_guide_leak_hunt_runs() {
    headless(|ctx| {
        // ANCHOR: asset_leaks
        use ponko::inventory::{AssetClass, AssetOwner};
        let before = ctx.asset_inventory();
        for _level in 0..3 {
            let house = ctx.gfx.load_mesh("models/house.glb");
            ctx.free(house); // keeps the file's materials and textures for next time
        }
        // Loading the same file again reuses them: only the first load grew.
        for grown in ctx.asset_inventory().growth_since(&before) {
            println!(
                "{}: {} more handles, {} more bytes",
                grown.class, grown.live, grown.bytes
            );
            // Which handles are they, how old, and who made them?
            for r in ctx.gfx.asset_handles(grown.class) {
                if let AssetOwner::File(name) = &r.owner {
                    println!(
                        "  {} {}: {} bytes, {} frames old, from {name}",
                        r.class, r.id, r.bytes, r.age_frames
                    );
                }
            }
        }
        // Done with the model for good: free its materials and textures too.
        ctx.gfx.unload_mesh("models/house.glb");
        assert!(ctx.asset_inventory().growth_since(&before).is_empty());
        // ANCHOR_END: asset_leaks
        let _ = AssetClass::Texture;
    });
}
