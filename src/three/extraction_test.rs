//! The 3D frame collection across frames. What it keeps between frames must
//! draw exactly what a collection that remembers nothing draws, whatever
//! changed in between, and must leave alone what didn't change.

use std::sync::Arc;

use glam::{Mat4, Quat, Vec2, Vec3};

use super::collect::{FADED, Frame3D, Instance3D};
use super::occlusion::OcclusionGrid;
use super::retained::SERIAL;
use super::skin::{MeshSkin, SkinDraw};
use super::{
    Assets3D, Camera3D, Lighting, LodGroup, LodLevel, LodRenderer, Material, Mesh, Model, ModelMaterial,
    ModelMaterials, PointLight3D, ReceivesDecals, ShadowMesh, ShadowOnly, Skeleton, SkinFrame, SkinOf, SpotLight3D,
    TERRAIN_LODS, TerrainTile, Transform3D,
};
use crate::color::Color;
use ponko_ecs::{Entity, World};

const MESHES: u32 = 6;
const SPACING: f32 = 3.0;

/// A world of models on the ground, seen by a camera that looks down along
/// the grid: models to its sides, behind it and far away are out of view.
struct Scene {
    world: World,
    assets: Assets3D,
    camera: Camera3D,
    lighting: Lighting,
    frame: Frame3D,
    models: Vec<Entity>,
    /// The skinned models, posed for this frame.
    skins: SkinFrame,
}

/// What a frame draws: per view (the camera, then each shadow view) and
/// mesh, its instances in a fixed order (a frame may draw the instances of
/// one mesh in any order).
type Drawn = Vec<(usize, u32, Vec<[u32; 20]>)>;

fn drawn(frame: &Frame3D) -> Drawn {
    let mut out = Vec::new();
    let mut view = |view: usize, runs: &[(u32, u32, u32)]| {
        for &(mesh, start, count) in runs {
            let instances = &frame.instances[start as usize..(start + count) as usize];
            let mut instances: Vec<[u32; 20]> = instances.iter().map(|&i| bytemuck::cast(i)).collect();
            instances.sort_unstable();
            out.push((view, mesh, instances));
        }
    };
    view(0, &frame.runs);
    for (v, runs) in frame.shadow_runs.iter().enumerate() {
        view(1 + v, runs);
    }
    out
}

fn place(i: usize, side: usize) -> Vec3 {
    let (x, z) = ((i % side) as f32, (i / side) as f32);
    Vec3::new(
        (x - side as f32 * 0.5) * SPACING,
        0.0,
        (z - side as f32 * 0.5) * SPACING,
    )
}

fn mesh(i: usize) -> Mesh {
    Mesh {
        id: (i as u32).wrapping_mul(2_654_435_761) % MESHES,
    }
}

impl Scene {
    fn new(models: usize) -> Self {
        let side = (models as f32).sqrt().ceil() as usize;
        let mut world = World::new();
        let models = (0..models)
            .map(|i| world.spawn((Transform3D::at(place(i, side)), Model::new(mesh(i)))))
            .collect();
        let mut camera = Camera3D::new(Vec2::new(1280.0, 720.0));
        camera.position = Vec3::new(0.0, 12.0, 30.0);
        camera.look_at(Vec3::ZERO);
        camera.far = 70.0;
        Self {
            world,
            assets: Assets3D {
                bounds: vec![(Vec3::ZERO, 1.0); MESHES as usize],
                ..Default::default()
            },
            camera,
            // Every cascade is drawn every frame, as in a first frame.
            lighting: Lighting {
                shadow_refresh: 1,
                ..Default::default()
            },
            frame: Frame3D::default(),
            models,
            skins: SkinFrame::default(),
        }
    }

    fn collect(&mut self) -> Drawn {
        self.frame
            .collect(&mut self.world, &self.assets, &self.skins, &self.camera, &self.lighting);
        drawn(&self.frame)
    }

    /// A frame that remembers nothing, of the same world, collected the
    /// other way (on all cores if this test runs on one, and the reverse).
    fn fresh(&mut self) -> Frame3D {
        let mut fresh = Frame3D::default();
        // It hides models behind the same earlier depth.
        fresh.occlusion = self.frame.occlusion.clone();
        let mine = SERIAL.get();
        SERIAL.set(mine.map(|serial| !serial));
        fresh.collect(&mut self.world, &self.assets, &self.skins, &self.camera, &self.lighting);
        SERIAL.set(mine);
        fresh
    }

    /// Collects a frame and checks it draws what a frame built from nothing
    /// draws. Returns how many instances the camera draws.
    #[track_caller]
    fn check(&mut self, what: &str) -> usize {
        let kept = self.collect();
        let fresh = self.fresh();
        let (now, new) = (&self.frame, &fresh);
        assert_eq!(
            (now.visible, now.drawn, now.models, now.cascades, now.light_views),
            (new.visible, new.drawn, new.models, new.cascades, new.light_views),
            "{what}: counts"
        );
        assert!(
            kept == drawn(&fresh),
            "{what}: not what a frame built from nothing draws"
        );
        // Nor do the views around global illumination's probes.
        assert_eq!(now.gi.is_some(), new.gi.is_some(), "{what}: probes");
        if let (Some(now), Some(new)) = (&now.gi, &new.gi) {
            assert!(
                drawn(&now.capture) == drawn(&new.capture),
                "{what}: not what the probes of a frame built from nothing see"
            );
        }
        now.visible
    }

    /// Where the camera draws the model at `x`, `z` (its place in the
    /// instance array).
    fn slot_at(&self, at: Vec3) -> Option<usize> {
        let drawn = |&(_, start, count): &(u32, u32, u32)| start as usize..(start + count) as usize;
        (self.frame.runs.iter().flat_map(drawn)).find(|&i| Vec3::from_slice(&self.frame.instances[i].model[3]) == at)
    }
}

/// The slots of `after` whose instance isn't what `before` had there.
fn rewritten(before: &[Instance3D], after: &[Instance3D]) -> usize {
    let same = |i: usize| (before.get(i)).is_some_and(|b| bytemuck::bytes_of(b) == bytemuck::bytes_of(&after[i]));
    (0..after.len()).filter(|&i| !same(i)).count()
}

/// A small deterministic random number generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }

    fn unit(&mut self) -> f32 {
        self.next() as f32 / (1u32 << 31) as f32
    }
}

fn a_still_scene_is_left_as_it_was() {
    let mut scene = Scene::new(1500);
    let first = scene.collect();
    assert!(
        scene.frame.visible > 100 && scene.frame.visible < 1500,
        "some in view, some not"
    );
    assert!(scene.frame.drawn > scene.frame.visible, "and some cast shadows");
    let (instances, runs) = (scene.frame.instances.clone(), scene.frame.runs.clone());
    for _ in 0..3 {
        assert!(scene.collect() == first);
        assert_eq!(
            rewritten(&instances, &scene.frame.instances),
            0,
            "no instance is written again"
        );
        assert_eq!(scene.frame.runs, runs);
    }
    scene.check("still");
}

fn a_first_frame_is_packed_by_view_then_mesh_like_a_sorted_one() {
    let mut scene = Scene::new(1500);
    scene.collect();
    let frame = &scene.frame;
    let mut next = 0;
    let views = std::iter::once(&frame.runs).chain(&frame.shadow_runs);
    for &(_, start, count) in views.flatten() {
        assert_eq!(start, next, "runs follow each other without holes");
        next = start + count;
    }
    assert_eq!(next as usize, frame.instances.len());
    assert!(frame.runs.windows(2).all(|w| w[0].0 < w[1].0), "in mesh order");
}

fn a_moving_camera_keeps_what_it_still_sees_in_place() {
    let mut scene = Scene::new(2500);
    scene.collect();
    let mut shares = Vec::new();
    for step in 1..=60 {
        let before = scene.frame.instances.clone();
        let seen = scene.frame.drawn;
        scene.camera.position.x = step as f32 * 0.2;
        scene.camera.look_at(Vec3::new(step as f32 * 0.3, 0.0, 0.0));
        scene.check("camera");
        shares.push(rewritten(&before, &scene.frame.instances) * 100 / seen);
    }
    assert_eq!(scene.frame.repacks(), 1, "only the first frame was packed");
    // A frame that sorted its models would shift most of them every frame.
    // Here a block is written again when it first outgrows its place, and
    // the array once when it is packed again; other frames write the few
    // models that entered or left.
    shares.sort_unstable();
    assert!(
        shares[shares.len() / 2] <= 5,
        "half the frames write under a twentieth: {shares:?}"
    );
    assert!(shares[shares.len() * 3 / 4] <= 20, "and most under a fifth: {shares:?}");
}

fn a_moved_model_rewrites_its_own_instances_only() {
    let mut scene = Scene::new(1500);
    scene.lighting.shadows = false;
    scene.collect();
    let moved = (scene.models.iter().copied())
        .find(|&e| scene.world.get::<Transform3D>(e).unwrap().position.distance(Vec3::ZERO) < 3.0)
        .unwrap();
    let from = scene.world.get::<Transform3D>(moved).unwrap().position;
    let slot = scene.slot_at(from).expect("the model in the middle is drawn");
    for step in 1..=5 {
        let before = scene.frame.instances.clone();
        let to = from + Vec3::Y * step as f32 * 0.1;
        scene.world.get_mut::<Transform3D>(moved).unwrap().position = to;
        scene.check("moved");
        assert_eq!(rewritten(&before, &scene.frame.instances), 1);
        assert_eq!(scene.slot_at(to), Some(slot), "it stays where it was drawn");
    }
}

fn every_kind_of_edit_draws_what_a_frame_built_from_nothing_draws() {
    let mut scene = Scene::new(900);
    let seen = scene.check("first");
    let near = |scene: &Scene, k: usize| {
        let mut close: Vec<Entity> = scene.models.clone();
        let at = |e: &Entity| scene.world.get::<Transform3D>(*e).unwrap().position.length_squared();
        close.sort_by(|a, b| at(a).total_cmp(&at(b)));
        close[k]
    };
    let (a, b, c) = (near(&scene, 0), near(&scene, 1), near(&scene, 2));

    // Transforms.
    let t = scene.world.get_mut::<Transform3D>(a).unwrap();
    t.position.y = 2.0;
    t.rotation = Quat::from_rotation_y(0.7);
    t.scale = Vec3::splat(1.5);
    assert_eq!(scene.check("transform"), seen);
    scene.world.get_mut::<Transform3D>(a).unwrap().position.z = 500.0;
    assert_eq!(scene.check("moved out of view"), seen - 1);
    scene.world.get_mut::<Transform3D>(a).unwrap().position.z = 0.0;
    assert_eq!(scene.check("and back"), seen);

    // The model's mesh and tint.
    let mesh_of = |scene: &Scene, e| scene.world.get::<Model>(e).unwrap().mesh.id;
    let other = Mesh {
        id: (mesh_of(&scene, a) + 1) % MESHES,
    };
    scene.world.get_mut::<Model>(a).unwrap().mesh = other;
    scene.check("mesh");
    scene.world.get_mut::<Model>(a).unwrap().color = Color::rgb(0.2, 0.4, 0.6);
    scene.check("tint");

    // Components that change how it is drawn, added and removed.
    scene.world.insert(a, ShadowMesh(None));
    let casting = scene.frame.drawn;
    scene.check("no shadow");
    assert!(scene.frame.drawn < casting, "it casts no shadow now");
    scene.world.insert(a, ShadowMesh(Some(Mesh { id: other.id ^ 1 })));
    scene.check("a shadow mesh of its own");
    scene.world.remove::<ShadowMesh>(a);
    scene.check("its own shadow again");
    assert_eq!(scene.frame.drawn, casting);
    scene.world.insert(a, ReceivesDecals(0b101));
    scene.check("decal layers");
    scene.world.remove::<ReceivesDecals>(a);
    scene.check("every decal again");
    scene.world.insert(a, ModelMaterial(Material { id: 3 }));
    scene.world.insert(b, ModelMaterial(Material { id: 3 }));
    scene.check("a material override");
    scene.world.insert(c, ModelMaterial(Material { id: 1 }));
    scene.check("another override, numbered before it");
    scene.world.remove::<ModelMaterial>(c);
    scene.world.remove::<ModelMaterial>(a);
    scene.check("overrides removed");
    // Per-part lists: told apart by what they hold, also next to a
    // whole-mesh material.
    let list = |parts: &[(usize, u32)]| {
        let parts: Vec<_> = parts.iter().map(|&(part, id)| (part, Material { id })).collect();
        ModelMaterials::new(&parts)
    };
    scene.world.insert(a, list(&[(0, 4)]));
    scene.world.insert(c, list(&[(0, 4)]));
    scene.check("a part list");
    scene.world.insert(a, list(&[(0, 5)]));
    scene.check("another material in the list");
    scene.world.insert(a, list(&[(0, 5), (1, 2)]));
    scene.check("a longer list");
    scene.world.insert(c, list(&[]));
    scene.check("an empty list is none");
    scene.world.remove::<ModelMaterials>(c);
    scene.world.remove::<ModelMaterials>(a);
    scene.world.remove::<ModelMaterial>(b);
    scene.check("lists removed");

    // Models that leave the plain path, and come back.
    scene.world.insert(b, SkinOf(a));
    assert_eq!(scene.check("a skin of another entity"), seen - 1);
    scene.world.remove::<SkinOf>(b);
    assert_eq!(scene.check("plain again"), seen);
    let model = scene.world.remove::<Model>(c).unwrap();
    assert_eq!(scene.check("model removed"), seen - 1);
    scene.world.insert(c, model);
    assert_eq!(scene.check("model added"), seen);
    let transform = scene.world.remove::<Transform3D>(c).unwrap();
    assert_eq!(scene.check("transform removed"), seen - 1);
    scene.world.insert(c, transform);
    assert_eq!(scene.check("transform added"), seen);

    // Entities that go, and new ones that take their index.
    assert!(scene.world.despawn(b));
    assert_eq!(scene.check("despawn"), seen - 1);
    let new = scene
        .world
        .spawn((Transform3D::at(Vec3::new(0.5, 3.0, 0.5)), Model::new(other)));
    assert_eq!(new.index(), b.index(), "the new entity has the old one's index");
    assert_eq!(scene.check("spawn"), seen);
    assert!(scene.world.despawn(new));
    let again = scene
        .world
        .spawn((Transform3D::at(Vec3::new(0.5, 4.0, 0.5)), Model::new(Mesh { id: 0 })));
    assert_eq!(scene.check("despawn and spawn in one frame"), seen);
    assert!(scene.world.despawn(again));
    assert!(scene.world.despawn(a));
    assert_eq!(scene.check("two despawns"), seen - 2);

    // The camera and the lights.
    scene.camera.position = Vec3::new(20.0, 8.0, -10.0);
    scene.camera.look_at(Vec3::new(-5.0, 0.0, 5.0));
    scene.check("camera moved");
    scene.camera.fov_y *= 0.5;
    scene.check("camera zoomed");
    scene.lighting.sun_direction = Vec3::new(0.3, -1.0, 0.6).normalize();
    scene.check("sun moved");
    scene.lighting.shadow_cascades = 2;
    scene.check("fewer cascades");
    scene.lighting.shadows = false;
    scene.check("no shadows");
    assert_eq!(scene.frame.drawn, scene.frame.visible);
    scene.lighting.shadows = true;
    scene.lighting.shadow_cascades = 4;
    scene.check("shadows again");

    // Everything gone, and a world again.
    for e in std::mem::take(&mut scene.models) {
        scene.world.despawn(e);
    }
    assert_eq!(scene.check("empty"), 0);
    assert!(scene.frame.instances.is_empty() && scene.frame.runs.is_empty());
    scene.world.spawn((Transform3D::default(), Model::new(Mesh { id: 2 })));
    assert_eq!(scene.check("one model"), 1);
}

fn a_change_to_a_mesh_reaches_the_models_that_use_it() {
    let mut scene = Scene::new(900);
    let seen = scene.check("first");
    let of_mesh = |scene: &Scene, id: u32| {
        (scene.frame.runs.iter())
            .find(|r| r.0 == id)
            .map_or(0, |r| r.2 as usize)
    };
    let ones = of_mesh(&scene, 1);
    assert!(ones > 0);

    // Its bounds grow: models past the edge of the view reach into it.
    scene.assets.bounds[1] = (Vec3::ZERO, 12.0);
    assert!(scene.check("bigger bounds") > seen);
    scene.assets.bounds[1] = (Vec3::new(0.0, 400.0, 0.0), 1.0);
    assert_eq!(scene.check("bounds far above"), seen - ones);
    // It is freed: its models draw nothing until it is a mesh again.
    scene.assets.bounds[1] = (Vec3::ZERO, -1.0);
    assert_eq!(scene.check("freed"), seen - ones);
    scene.assets.bounds[1] = (Vec3::ZERO, 1.0);
    assert_eq!(scene.check("made again"), seen);

    // It stops casting shadows, and casts again.
    let casting = scene.frame.drawn;
    scene.assets.shadowless = vec![false, true];
    scene.check("shadowless");
    assert!(scene.frame.drawn < casting);
    scene.assets.shadowless.clear();
    scene.check("casting again");
    assert_eq!(scene.frame.drawn, casting);

    // It is skinned: only its models without a skeleton are drawn plain.
    let middle = |e: &Entity| scene.world.get::<Transform3D>(*e).unwrap().position.length_squared();
    let posed = (scene.models.iter().copied())
        .filter(|&e| scene.world.get::<Model>(e).unwrap().mesh.id == 1)
        .min_by(|a, b| middle(a).total_cmp(&middle(b)))
        .unwrap();
    scene.world.insert(posed, Skeleton::new(Vec::new()));
    assert_eq!(scene.check("a skeleton on a plain mesh"), seen);
    let skin = MeshSkin {
        joints: Arc::from(Vec::new()),
        slots: Vec::new(),
        inverse_binds: Vec::new(),
        bounds: Vec::new(),
    };
    scene.assets.skins = vec![None, Some(Arc::new(skin))];
    assert_eq!(scene.check("skinned"), seen - 1);
    scene.world.remove::<Skeleton>(posed);
    assert_eq!(scene.check("its skeleton removed"), seen);

    // More meshes are made: models of the old ones aren't touched.
    let before = scene.frame.instances.clone();
    scene.assets.bounds.push((Vec3::ZERO, 2.0));
    scene.check("a new mesh");
    assert_eq!(rewritten(&before, &scene.frame.instances), 0);
    scene
        .world
        .spawn((Transform3D::default(), Model::new(Mesh { id: MESHES })));
    scene.check("and a model of it");
}

#[test]
fn models_keeping_a_reused_mesh_slots_old_id_draw_nothing() {
    let mut scene = Scene::new(900);
    let seen = scene.check("first");
    let ones = (scene.frame.runs.iter()).find(|r| r.0 == 1).map_or(0, |r| r.2 as usize);
    assert!(ones > 0);
    // Slot 1 is freed and made again in one frame, with the same bounds,
    // under a new id: the models holding the old id draw nothing.
    scene.assets.mesh_ids.release(1);
    let new = Mesh {
        id: scene.assets.mesh_ids.make(scene.assets.bounds.len(), "mesh"),
    };
    assert_eq!(new.slot(), 1);
    assert_eq!(scene.check("slot reused"), seen - ones);
    // Pointed at the new id, they draw again, as slot 1.
    let holders: Vec<Entity> = (scene.models.iter().copied())
        .filter(|&e| scene.world.get::<Model>(e).unwrap().mesh.id == 1)
        .collect();
    for e in holders {
        scene.world.get_mut::<Model>(e).unwrap().mesh = new;
    }
    assert_eq!(scene.check("new id"), seen);
    assert!(scene.frame.runs.iter().any(|r| r.0 == 1 && r.2 as usize == ones));
    assert!(scene.frame.runs.iter().all(|r| r.0 != new.id));
}

#[test]
fn random_edits_always_draw_what_a_frame_built_from_nothing_draws() {
    // Enough models that they are compared on several cores, and that one
    // core stops comparing them while most move.
    let mut scene = Scene::new(6000);
    let side = 78;
    let mut rng = Rng(7);
    scene.check("first");
    let mut bare: Vec<(Entity, Model)> = Vec::new();
    let mut rushed = 0;
    for frame in 0..160 {
        for _ in 0..rng.below(40) {
            let i = rng.below(scene.models.len());
            let e = scene.models[i];
            match rng.below(14) {
                0..=3 => {
                    let t = scene.world.get_mut::<Transform3D>(e).unwrap();
                    t.position += Vec3::new(rng.unit() - 0.5, rng.unit() * 0.2, rng.unit() - 0.5) * 6.0;
                }
                4 => {
                    let t = scene.world.get_mut::<Transform3D>(e).unwrap();
                    t.rotation = Quat::from_rotation_y(rng.unit() * 6.0);
                    t.scale = Vec3::splat(0.5 + rng.unit() * 2.0);
                }
                5 => {
                    if let Some(m) = scene.world.get_mut::<Model>(e) {
                        m.mesh = mesh(rng.next() as usize);
                    }
                }
                6 => {
                    if let Some(m) = scene.world.get_mut::<Model>(e) {
                        m.color = Color::rgb(rng.unit(), rng.unit(), 1.0);
                    }
                }
                7 => {
                    scene.world.despawn(e);
                    bare.retain(|b| b.0 != e);
                    let at = place(rng.below(side * side), side);
                    scene.models[i] = scene.world.spawn((Transform3D::at(at), Model::new(mesh(i + frame))));
                }
                8 => match rng.below(3) {
                    0 => drop(scene.world.remove::<ShadowMesh>(e)),
                    1 => drop(scene.world.insert(e, ShadowMesh(None))),
                    _ => drop(scene.world.insert(e, ShadowMesh(Some(mesh(i + 1))))),
                },
                9 => match rng.below(3) {
                    0 => drop(scene.world.remove::<ModelMaterial>(e)),
                    k => drop(scene.world.insert(e, ModelMaterial(Material { id: k as u32 }))),
                },
                10 => match rng.below(2) {
                    0 => drop(scene.world.remove::<ReceivesDecals>(e)),
                    _ => drop(scene.world.insert(e, ReceivesDecals(rng.next() & 0xFF))),
                },
                11 => match rng.below(2) {
                    0 => drop(scene.world.remove::<SkinOf>(e)),
                    _ => drop(scene.world.insert(e, SkinOf(e))),
                },
                12 => {
                    if let Some(model) = scene.world.remove::<Model>(e) {
                        bare.push((e, model));
                    }
                }
                _ => {
                    if let Some((e, model)) = bare.pop() {
                        scene.world.insert(e, model);
                    }
                }
            }
        }
        // For a while most models move every frame: one core stops
        // comparing them (and still follows every edit).
        if (50..75).contains(&frame) {
            for (i, &e) in scene.models.iter().enumerate() {
                if i % 5 != 0 {
                    scene.world.get_mut::<Transform3D>(e).unwrap().position.y = ((frame + i) % 2) as f32 * 0.4;
                }
            }
            rushed += usize::from(scene.frame.rushing());
        }
        // Some frames the camera flies, some it stays.
        if frame % 3 != 0 {
            let angle = frame as f32 * 0.05;
            scene.camera.position = Vec3::new(angle.cos() * 25.0, 10.0 + angle.sin() * 4.0, angle.sin() * 25.0);
            scene.camera.look_at(Vec3::new((angle * 0.5).sin() * 10.0, 0.0, 0.0));
        }
        match frame % 40 {
            11 => scene.lighting.sun_direction = Vec3::new(rng.unit() - 0.5, -1.0, rng.unit() - 0.5).normalize(),
            17 => scene.lighting.shadows = !scene.lighting.shadows,
            23 => scene.lighting.shadow_cascades = 1 + rng.below(4) as u32,
            29 => scene.assets.bounds[rng.below(MESHES as usize)] = (Vec3::ZERO, [-1.0, 1.0, 3.0][rng.below(3)]),
            31 => scene.assets.shadowless = (0..MESHES).map(|_| rng.below(4) == 0).collect(),
            // A wave of new models, more than are worked out one by one.
            37 => {
                for k in 0..700 {
                    let at = place(rng.below(side * side), side) + Vec3::Y;
                    scene
                        .models
                        .push(scene.world.spawn((Transform3D::at(at), Model::new(mesh(k)))));
                }
            }
            38 => {
                for e in scene.models.split_off(scene.models.len() - 700) {
                    scene.world.despawn(e);
                    bare.retain(|b| b.0 != e);
                }
            }
            _ => {}
        }
        scene.check(&format!("frame {frame}"));
    }
    assert!(
        rushed > 15,
        "it stopped comparing models while most moved: {rushed} frames"
    );
    assert!(!scene.frame.rushing(), "and compares the models again");
}

/// Moves every model but each tenth up or down.
fn bob(scene: &mut Scene, frame: usize) {
    for (i, &e) in scene.models.iter().enumerate() {
        if i % 10 != 0
            && let Some(t) = scene.world.get_mut::<Transform3D>(e)
        {
            t.position.y = (frame % 2) as f32 * 0.3;
        }
    }
}

fn while_most_models_move_every_edit_is_still_followed() {
    let mut scene = Scene::new(6000);
    let seen = scene.check("first");
    let near = |scene: &Scene, k: usize| {
        let mut close: Vec<Entity> = scene.models.clone();
        let at = |e: &Entity| scene.world.get::<Transform3D>(*e).unwrap().position.length_squared();
        close.sort_by(|a, b| at(a).total_cmp(&at(b)));
        close[k]
    };
    let far = |scene: &Scene, k: usize| near(scene, scene.models.len() - 1 - k);
    let mut rushed = 0;
    // The models move in frames 2..=40, and end where they began.
    for frame in 0..50 {
        if (2..=40).contains(&frame) {
            bob(&mut scene, frame);
        }
        match frame {
            // A model in view and one out of view go; new ones take their
            // indices, one in view, one not.
            8 => {
                let (a, b) = (near(&scene, 0), far(&scene, 0));
                for e in [a, b] {
                    scene.world.despawn(e);
                    scene.models.retain(|&m| m != e);
                }
                for at in [Vec3::new(0.5, 1.0, 0.5), Vec3::new(0.0, 0.0, 900.0)] {
                    let e = scene.world.spawn((Transform3D::at(at), Model::new(mesh(3))));
                    scene.models.push(e);
                }
                assert_eq!(scene.check("despawned and spawned"), seen);
            }
            // Models leave the plain path and come back.
            10 => {
                let (a, b) = (near(&scene, 1), near(&scene, 2));
                scene.world.insert(a, SkinOf(b));
                let model = scene.world.remove::<Model>(b).unwrap();
                assert_eq!(scene.check("a skin, and a model removed"), seen - 2);
                scene.world.remove::<SkinOf>(a);
                scene.world.insert(b, model);
            }
            // What a mesh is changes while the models aren't compared.
            12 => scene.assets.bounds[1] = (Vec3::ZERO, 9.0),
            14 => scene.assets.shadowless = vec![false, false, true],
            16 => {
                let a = near(&scene, 3);
                scene.world.insert(a, ModelMaterial(Material { id: 2 }));
                scene.world.insert(a, ShadowMesh(Some(mesh(1))));
                scene.world.get_mut::<Model>(a).unwrap().color = Color::rgb(0.1, 0.5, 0.9);
            }
            18 => {
                scene.camera.position = Vec3::new(14.0, 9.0, 22.0);
                scene.camera.look_at(Vec3::new(-4.0, 0.0, 0.0));
            }
            20 => scene.lighting.shadows = false,
            22 => {
                scene.lighting.shadows = true;
                scene.lighting.sun_direction = Vec3::new(0.4, -1.0, 0.2).normalize();
            }
            // Everything in view leaves it, and comes back.
            24 => scene.camera.position.y = 4000.0,
            26 => scene.camera.position.y = 9.0,
            _ => {}
        }
        scene.check(&format!("frame {frame}"));
        rushed += usize::from(scene.frame.rushing());
    }
    // It stopped comparing models while they moved, and started again
    // when they stopped: what it kept of them from before the rush (the
    // same places, but meshes with other bounds) is worked out anew.
    assert!(rushed > 30, "{rushed} frames");
    assert!(!scene.frame.rushing());
    let before = scene.frame.instances.clone();
    scene.check("still again");
    assert_eq!(rewritten(&before, &scene.frame.instances), 0);
}

fn many_models_spawned_at_once_are_drawn_also_while_most_move() {
    let mut scene = Scene::new(6000);
    let first = scene.check("first");
    // More at once than are taken in one by one, all in view.
    let wave = |scene: &mut Scene, height: f32| {
        for i in 0..700 {
            let at = Vec3::new((i % 27) as f32 * 0.3 - 4.0, height, (i / 27) as f32 * 0.3 - 4.0);
            let e = scene.world.spawn((Transform3D::at(at), Model::new(mesh(i))));
            scene.models.push(e);
        }
    };
    wave(&mut scene, 1.0);
    let more = scene.check("a wave in a still world");
    assert!(more > first + 600, "{more} of them drawn, {first} before");
    let mut rushed = 0;
    for frame in 0..12 {
        bob(&mut scene, frame);
        if frame == 6 {
            assert!(scene.frame.rushing(), "most models move");
            wave(&mut scene, 2.0);
            // And as many gone at once.
            for e in scene.models.drain(100..800) {
                scene.world.despawn(e);
            }
        }
        let drawn = scene.check(&format!("frame {frame}"));
        if frame == 6 {
            assert!(drawn > more + 500, "{drawn} drawn with the second wave, {more} before");
        }
        rushed += usize::from(scene.frame.rushing());
    }
    assert!(rushed > 8, "{rushed} frames");
    scene.check("still");
    scene.check("and compared again");
    assert!(!scene.frame.rushing());
}

fn a_collection_can_change_between_one_core_and_several() {
    let mut scene = Scene::new(6000);
    let mine = SERIAL.get().unwrap();
    let mut rushed = 0;
    for frame in 0..60 {
        // Every few frames the other way, also while most models move.
        SERIAL.set(Some(mine ^ (frame / 3 % 2 == 1)));
        if (5..40).contains(&frame) {
            bob(&mut scene, frame);
        }
        if frame % 4 == 0 {
            let e = scene.models[frame * 37 % scene.models.len()];
            scene.world.despawn(e);
            let at = place(frame * 53 % 6000, 78) + Vec3::Y;
            let i = scene.models.iter().position(|&m| m == e).unwrap();
            scene.models[i] = scene.world.spawn((Transform3D::at(at), Model::new(mesh(frame))));
        }
        scene.camera.position.x = (frame as f32 * 0.3).sin() * 6.0;
        scene.check(&format!("frame {frame}"));
        rushed += usize::from(scene.frame.rushing());
    }
    assert!(
        rushed > 25,
        "a rush goes on, whichever way a frame is collected: {rushed} frames"
    );
    SERIAL.set(Some(mine));
}

fn shadow_only_casters_follow_edits_too() {
    let mut scene = Scene::new(400);
    let casters: Vec<Entity> = (0..30)
        .map(|i| {
            let at = Vec3::new(i as f32 - 15.0, 2.0, 0.0);
            scene.world.spawn((Transform3D::at(at), ShadowMesh(Some(mesh(i)))))
        })
        .collect();
    let seen = scene.check("first");
    let casting = scene.frame.drawn;
    scene.world.get_mut::<Transform3D>(casters[3]).unwrap().position.y = 4.0;
    scene.check("moved");
    scene.world.despawn(casters[5]);
    scene.world.remove::<ShadowMesh>(casters[6]);
    assert_eq!(scene.check("gone"), seen);
    assert!(scene.frame.drawn < casting);
    scene.world.insert(casters[7], Model::new(mesh(2)));
    assert_eq!(scene.check("now a model with a shadow mesh"), seen + 1);
    scene.camera.position.x += 6.0;
    scene.check("camera");
}

/// How many instances the shadow views draw.
fn shadow_instances(frame: &Frame3D) -> u32 {
    frame.shadow_runs.iter().flatten().map(|r| r.2).sum()
}

fn shadow_only_models_cast_their_shadow_but_no_camera_draws_them() {
    let mut scene = Scene::new(400);
    let seen = scene.check("plain");
    let casting = shadow_instances(&scene.frame);
    assert!(seen > 0 && casting > 0);
    let models = scene.models.clone();
    for &e in &models {
        scene.world.insert(e, ShadowOnly);
    }
    assert_eq!(scene.check("shadow only"), 0, "the camera draws none of them");
    assert_eq!(shadow_instances(&scene.frame), casting, "every shadow still falls");
    scene.world.get_mut::<Transform3D>(models[7]).unwrap().position.y = 1.0;
    scene.camera.position.x += 3.0;
    scene.check("moved while shadow only");
    for &e in &models {
        scene.world.remove::<ShadowOnly>(e);
    }
    scene.camera.position.x -= 3.0;
    scene.world.get_mut::<Transform3D>(models[7]).unwrap().position.y = 0.0;
    assert_eq!(scene.check("shown again"), seen);

    // A skinned model: cast, not seen.
    let body = scene.world.spawn((ShadowOnly,));
    let matrix = Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0));
    scene.skins.draws = vec![SkinDraw {
        mesh: 1,
        instance: Instance3D {
            model: matrix.to_cols_array_2d(),
            color: [1.0; 4],
        },
        entity: body.to_bits(),
        matrix,
        scale: 1.0,
        bounds: (Vec3::ZERO, 1.0),
        first: 0,
        morph_first: 0,
        morph_count: 0,
        material: u32::MAX,
        parts: (0, 0),
        lod: u32::MAX,
        posed_motion: false,
    }];
    assert_eq!(scene.check("skinned, shadow only"), seen);
    assert!(shadow_instances(&scene.frame) > casting, "its shadow falls");
    scene.world.remove::<ShadowOnly>(body);
    assert_eq!(scene.check("skinned, shown"), seen + 1);
}

fn a_kept_cascade_draws_nothing_and_is_right_when_its_turn_comes() {
    let mut scene = Scene::new(1500);
    scene.lighting.shadow_refresh = 4;
    scene.lighting.shadow_cascades = 4;
    let mut kept = 0;
    for frame in 0..24 {
        // A model moves through the cascades while some are kept.
        let e = scene.models[frame * 31 % scene.models.len()];
        scene.world.get_mut::<Transform3D>(e).unwrap().position.y += 1.5;
        scene.collect();
        // A frame built from nothing draws every cascade, into the same boxes
        // as the ones this frame drew again.
        let fresh = scene.fresh();
        let (now, new) = (drawn(&scene.frame), drawn(&fresh));
        for cascade in 0..scene.frame.cascades {
            let of = |d: &Drawn| d.iter().filter(|r| r.0 == 1 + cascade).cloned().collect::<Vec<_>>();
            if scene.frame.redrawn & (1 << cascade) == 0 {
                assert!(
                    of(&now).is_empty(),
                    "frame {frame}: kept cascade {cascade} draws nothing"
                );
                kept += 1;
            } else if scene.frame.sun_view_proj[cascade] == fresh.sun_view_proj[cascade] {
                assert!(of(&now) == of(&new), "frame {frame}: cascade {cascade}");
            }
        }
        assert!(now.iter().filter(|r| r.0 == 0).eq(new.iter().filter(|r| r.0 == 0)));
    }
    assert!(kept > 10, "the far cascades were kept on most frames");
}

fn models_are_counted_once_each_also_in_cascades_that_are_kept() {
    let mut scene = Scene::new(1500);
    scene.lighting.shadow_refresh = 4;
    scene.lighting.shadow_cascades = 4;
    scene.collect();
    let (seen, models) = (scene.frame.visible, scene.frame.models);
    // Models out of the camera's view cast into it; one the camera sees
    // that is in a cascade or two as well counts once.
    assert!(models > seen, "{models} models, {seen} in view");
    let instances: usize = (0..scene.frame.cascades).fold(seen, |n, cascade| {
        n + drawn(&scene.frame)
            .iter()
            .filter(|r| r.0 == 1 + cascade)
            .map(|r| r.2.len())
            .sum::<usize>()
    });
    assert!(models < instances, "{models} models in {instances} instances");
    // Nothing moves: as many models whichever cascades a frame draws again.
    let mut fewest = scene.frame.drawn;
    for _ in 0..8 {
        scene.collect();
        assert_eq!(scene.frame.models, models);
        fewest = fewest.min(scene.frame.drawn);
    }
    assert!(fewest < instances, "some frames kept a cascade's map");
}

fn moved_models_are_listed_with_where_they_were() {
    let mut scene = Scene::new(600);
    scene.lighting.shadows = false;
    scene.frame.motion.enabled = true;
    scene.collect();
    scene.collect();
    assert!(scene.frame.motion.instances.is_empty(), "nothing moved");
    let e = (scene.models.iter().copied())
        .find(|&e| scene.world.get::<Transform3D>(e).unwrap().position.distance(Vec3::ZERO) < 3.0)
        .unwrap();
    let from = scene.world.get::<Transform3D>(e).unwrap().position;
    scene.world.get_mut::<Transform3D>(e).unwrap().position.y = 1.0;
    scene.camera.position.x += 0.5; // a moving camera alone moves no model
    scene.collect();
    let moved = &scene.frame.motion.instances;
    assert_eq!(moved.len(), 1);
    assert_eq!(Vec3::from_slice(&moved[0].previous[3]), from);
    assert_eq!(Vec3::from_slice(&moved[0].instance.model[3]), from.with_y(1.0));
    scene.collect();
    assert!(scene.frame.motion.instances.is_empty(), "it stopped");
}

fn what_is_kept_per_model_is_bounded_and_stops_growing() {
    let models = 20_000;
    let mut scene = Scene::new(models);
    scene.collect();
    let first = scene.frame.extraction_bytes();
    // About 210 bytes a model, and the instances drawn with room to grow.
    let budget = models * 260 + scene.frame.drawn * 2 * size_of::<Instance3D>() + 64 * 1024;
    assert!(first <= budget, "{first} bytes kept for {models} models");
    let mut most = first;
    for round in 0..3 {
        for step in 0..60 {
            let angle = step as f32 * 0.1;
            scene.camera.position = Vec3::new(angle.cos() * 30.0, 12.0, angle.sin() * 30.0);
            scene.camera.look_at(Vec3::ZERO);
            scene.collect();
            most = most.max(scene.frame.extraction_bytes());
        }
        // The same flight again takes no more than the first time.
        if round == 0 {
            assert!(most <= budget * 2, "{most} bytes kept after flying around");
        }
    }
    let settled = most;
    for step in 0..60 {
        let angle = step as f32 * 0.1;
        scene.camera.position = Vec3::new(angle.cos() * 30.0, 12.0, angle.sin() * 30.0);
        scene.camera.look_at(Vec3::ZERO);
        scene.collect();
        assert!(scene.frame.extraction_bytes() <= settled, "it stopped growing");
    }
}

/// Moves the camera along a path that comes close to the middle of the
/// world and goes far from it.
fn swoop(scene: &mut Scene, step: usize) {
    let angle = step as f32 * 0.21;
    let away = 6.0 + (1.0 + (step as f32 * 0.37).sin()) * 28.0;
    scene.camera.position = Vec3::new(angle.cos() * away, 3.0 + away * 0.4, angle.sin() * away);
    scene.camera.look_at(Vec3::new(0.0, 1.0, 0.0));
}

fn lod_groups_keep_up_with_the_camera_and_edits() {
    let mut scene = Scene::new(300);
    scene.camera.far = 400.0;
    let group = |i: usize| {
        LodGroup::new(
            3.0,
            vec![
                LodLevel::new(0.3, vec![LodRenderer::new(mesh(i)), LodRenderer::new(mesh(i + 1))]).with_fade(0.6),
                LodLevel::new(0.1, vec![LodRenderer::new(mesh(i + 2))]).with_fade(0.4),
                LodLevel::new(0.02, vec![LodRenderer::new(mesh(i + 3))]),
            ],
        )
    };
    let groups: Vec<Entity> = (0..40)
        .map(|i| {
            let at = Vec3::new((i % 8) as f32 * 4.0 - 14.0, 1.5, (i / 8) as f32 * 5.0 - 10.0);
            scene.world.spawn((Transform3D::at(at), group(i)))
        })
        .collect();
    let (mut fading, mut whole) = (0, 0);
    for step in 0..48 {
        swoop(&mut scene, step);
        match step {
            10 => scene.world.get_mut::<Transform3D>(groups[3]).unwrap().position.y = 4.0,
            14 => drop(scene.world.despawn(groups[5])),
            18 => drop(scene.world.insert(groups[7], ReceivesDecals(0b11))),
            22 => *scene.world.get_mut::<LodGroup>(groups[9]).unwrap() = group(2),
            26 => scene.world.get_mut::<Transform3D>(groups[11]).unwrap().scale = Vec3::splat(2.5),
            30 => drop(scene.world.remove::<LodGroup>(groups[13])),
            34 => scene.lighting.shadows = false,
            38 => scene.lighting.shadows = true,
            _ => {}
        }
        scene.check(&format!("step {step}"));
        let faded = scene.frame.runs.iter().any(|r| r.0 & FADED != 0);
        fading += usize::from(faded);
        whole += usize::from(!faded);
    }
    assert!(fading > 5 && whole > 0, "{fading} steps crossfaded, {whole} didn't");
}

fn terrain_tiles_keep_up_with_the_camera_and_edits() {
    assert!(TERRAIN_LODS <= MESHES as usize);
    let mut scene = Scene::new(200);
    scene.camera.far = 400.0;
    // Tiles big enough to see, finer meshes near the camera.
    scene.assets.bounds = vec![(Vec3::ZERO, 6.0); MESHES as usize];
    let tile = TerrainTile {
        lods: std::array::from_fn(|level| Mesh { id: level as u32 }),
        lod_distance: 12.0,
    };
    let tiles: Vec<Entity> = (0..25)
        .map(|i| {
            let at = Vec3::new((i % 5) as f32 * 10.0 - 20.0, 0.0, (i / 5) as f32 * 10.0 - 20.0);
            scene.world.spawn((Transform3D::at(at), tile))
        })
        .collect();
    let mut levels = std::collections::BTreeSet::new();
    for step in 0..40 {
        swoop(&mut scene, step);
        match step {
            8 => drop(scene.world.insert(tiles[12], ShadowMesh(None))),
            12 => drop(scene.world.remove::<ShadowMesh>(tiles[12])),
            16 => drop(scene.world.despawn(tiles[6])),
            20 => scene.world.get_mut::<TerrainTile>(tiles[7]).unwrap().lod_distance = 40.0,
            24 => scene.world.get_mut::<Transform3D>(tiles[8]).unwrap().position.y = -2.0,
            28 => scene.assets.bounds[1] = (Vec3::ZERO, -1.0), // a level's mesh freed
            32 => scene.assets.bounds[1] = (Vec3::ZERO, 6.0),
            _ => {}
        }
        scene.check(&format!("step {step}"));
        levels.extend(scene.frame.runs.iter().map(|r| r.0));
    }
    assert!(levels.len() >= 3, "the tiles went through their levels: {levels:?}");
}

fn skinned_models_keep_up_as_their_number_changes() {
    let mut scene = Scene::new(400);
    // Models with a material override: their mesh ids follow the skinned
    // models', so they are numbered again whenever those change in number.
    for &e in &scene.models[190..196] {
        scene.world.insert(e, ModelMaterial(Material { id: 2 }));
    }
    let draw = |i: usize, y: f32| {
        let matrix = Mat4::from_translation(Vec3::new(i as f32 * 2.0 - 6.0, y, 1.0));
        SkinDraw {
            mesh: i as u32 % MESHES,
            instance: Instance3D {
                model: matrix.to_cols_array_2d(),
                color: [1.0; 4],
            },
            entity: 1 << 40 | i as u64,
            matrix,
            scale: 1.0,
            // Every third one has nothing to draw this frame.
            bounds: (Vec3::ZERO, if i % 3 == 2 { -1.0 } else { 1.0 }),
            first: 0,
            morph_first: 0,
            morph_count: 0,
            material: u32::MAX,
            parts: (0, 0),
            lod: u32::MAX,
            posed_motion: false,
        }
    };
    let plain = scene.check("no skinned model");
    let mut most = 0;
    for (step, n) in [3, 6, 6, 2, 5, 0, 4, 4, 7, 1].into_iter().enumerate() {
        scene.skins.draws = (0..n).map(|i| draw(i, step as f32 * 0.2)).collect();
        if step == 6 {
            scene.camera.position.x += 3.0;
        }
        most = most.max(scene.check(&format!("step {step}: {n} skinned models")));
    }
    assert!(most > plain, "skinned models were drawn");
}

fn point_and_spot_light_shadows_keep_up_with_their_lights() {
    let mut scene = Scene::new(900);
    let shadowed = PointLight3D {
        shadows: true,
        range: 12.0,
        ..Default::default()
    };
    let lamp = scene.world.spawn((Transform3D::at(Vec3::new(0.0, 3.0, 0.0)), shadowed));
    let cone = SpotLight3D {
        shadows: true,
        range: 15.0,
        ..Default::default()
    };
    let spot = scene
        .world
        .spawn((Transform3D::at(Vec3::new(6.0, 5.0, 2.0)).looking_at(Vec3::ZERO), cone));
    scene.check("a lamp and a spot");
    assert_eq!(
        scene.frame.light_views, 7,
        "six views around the lamp, one for the spot"
    );
    let in_light_views = |scene: &Scene| -> usize {
        let views = &scene.frame.shadow_runs[super::collect::MAX_CASCADES..];
        views.iter().flatten().map(|r| r.2 as usize).sum()
    };
    let lit = in_light_views(&scene);
    assert!(lit > 0, "the lights' views draw casters");
    scene.world.get_mut::<Transform3D>(lamp).unwrap().position = Vec3::new(-8.0, 2.0, 4.0);
    scene.check("the lamp moved");
    let e = scene.models[450];
    scene.world.get_mut::<Transform3D>(e).unwrap().position = Vec3::new(-8.0, 0.5, 5.0);
    scene.check("a model moved next to it");
    scene.world.get_mut::<PointLight3D>(lamp).unwrap().shadows = false;
    scene.check("the lamp casts none");
    assert_eq!(scene.frame.light_views, 1);
    assert!(in_light_views(&scene) < lit);
    scene.world.despawn(spot);
    scene.check("the spot is gone");
    assert_eq!((scene.frame.light_views, in_light_views(&scene)), (0, 0));
    scene.world.get_mut::<PointLight3D>(lamp).unwrap().shadows = true;
    scene.check("the lamp casts again");
    scene.lighting.light_shadow_views = 0;
    scene.check("no light shadows allowed");
    assert_eq!(scene.frame.light_views, 0);
    scene.lighting.light_shadow_views = 12;
    scene.camera.position.x -= 5.0;
    scene.check("allowed again, from another place");
    assert_eq!(scene.frame.light_views, 6);
}

fn models_hidden_behind_an_earlier_frames_depth_keep_up_with_it() {
    let mut scene = Scene::new(900);
    scene.camera.occlusion_culling = true;
    let open = scene.check("nothing to hide behind");
    // The depth of an earlier frame: something near covers the left half of
    // the view, then the right half.
    let wall = |scene: &Scene, left: bool| {
        let depth = (0..64).map(|i| if (i % 8 < 4) == left { 0.05 } else { 1.0 }).collect();
        OcclusionGrid::new(scene.camera.view_proj(), scene.camera.position, 8, 8, depth)
    };
    scene.frame.occlusion = Some(wall(&scene, true));
    let behind_left = scene.check("the left half is hidden");
    assert!(
        behind_left < open && scene.frame.occluded > 0,
        "{behind_left} of {open}"
    );
    let e = scene.models[450];
    scene.world.get_mut::<Transform3D>(e).unwrap().position.y = 1.0;
    assert_eq!(scene.check("a model moved"), behind_left);
    scene.frame.occlusion = Some(wall(&scene, false));
    assert!(scene.check("the right half is hidden") < open);
    scene.camera.position.x += 4.0;
    scene.frame.occlusion = Some(wall(&scene, false));
    scene.check("from another place");
    scene.frame.occlusion = None;
    assert!(scene.check("no depth") > behind_left);
    assert_eq!(scene.frame.occluded, 0);
}

fn models_outside_the_view_are_counted_from_what_is_kept() {
    let mut scene = Scene::new(1500);
    scene.collect();
    let outside = scene.frame.frustum_culled_models().unwrap();
    // Plain models, no occlusion: what the camera doesn't draw is outside
    // its view.
    assert_eq!(outside, 1500 - scene.frame.visible);
    assert!(outside > 100 && outside < 1400, "some in view, some not: {outside}");
    // A still camera culls nothing again, and the count stays.
    for _ in 0..3 {
        scene.collect();
        assert_eq!(scene.frame.frustum_culled_models(), Some(outside));
    }
    // One model taken into the view, then far behind the camera.
    let e = scene.models[0];
    scene.world.get_mut::<Transform3D>(e).unwrap().position = Vec3::ZERO;
    scene.collect();
    let one_in = scene.frame.frustum_culled_models().unwrap();
    scene.world.get_mut::<Transform3D>(e).unwrap().position = Vec3::new(0.0, 0.0, 900.0);
    scene.collect();
    assert_eq!(scene.frame.frustum_culled_models(), Some(one_in + 1));
    // A model whose mesh isn't loaded has nothing to cull: not counted.
    let unloaded = scene
        .world
        .spawn((Transform3D::at(Vec3::new(0.0, 0.0, 900.0)), Model::new(Mesh { id: 99 })));
    scene.collect();
    assert_eq!(scene.frame.frustum_culled_models(), Some(one_in + 1));
    scene.world.despawn(unloaded);
    // Models hidden by occlusion culling are in the view: not counted.
    scene.camera.occlusion_culling = true;
    let depth = (0..64).map(|i| if i % 8 < 4 { 0.05 } else { 1.0 }).collect();
    scene.frame.occlusion = Some(OcclusionGrid::new(
        scene.camera.view_proj(),
        scene.camera.position,
        8,
        8,
        depth,
    ));
    scene.collect();
    assert!(scene.frame.occluded > 0);
    assert_eq!(scene.frame.frustum_culled_models(), Some(one_in + 1));
    // A despawned model leaves the count.
    scene.world.despawn(e);
    scene.frame.occlusion = None;
    scene.collect();
    assert_eq!(scene.frame.frustum_culled_models(), Some(one_in));
}

fn while_most_models_move_their_culling_isnt_counted() {
    let mut scene = Scene::new(6000);
    scene.collect();
    assert!(scene.frame.frustum_culled_models().is_some());
    let mut rushed = false;
    for frame in 2..20 {
        bob(&mut scene, frame);
        scene.collect();
        if scene.frame.rushing() {
            rushed = true;
            assert_eq!(scene.frame.frustum_culled_models(), None, "what is kept is stale");
        }
    }
    assert!(rushed, "most models moving every frame starts a rush");
    // Settled again: counted again once what is kept is worked out anew.
    for _ in 0..4 {
        scene.collect();
    }
    let outside = scene.frame.frustum_culled_models().unwrap();
    assert_eq!(outside, 6000 - scene.frame.visible);
}

fn a_target_cameras_view_keeps_up() {
    // A target camera's frame: its own lens and viewport, no shadows and no
    // probes of its own.
    let mut scene = Scene::new(1500);
    scene.lighting.shadows = false;
    scene.camera.set_viewport(Vec2::new(256.0, 256.0));
    scene.camera.fov_y = 0.6;
    for step in 0..30 {
        swoop(&mut scene, step);
        let e = scene.models[step * 47 % scene.models.len()];
        match step % 5 {
            0 => scene.world.get_mut::<Transform3D>(e).unwrap().position.y += 1.0,
            1 => scene.world.get_mut::<Model>(e).unwrap().mesh = mesh(step),
            2 => {
                scene.world.despawn(e);
                scene.models.retain(|&m| m != e);
            }
            _ => {}
        }
        scene.check(&format!("step {step}"));
        assert_eq!(scene.frame.drawn, scene.frame.visible, "no shadow casters");
    }
}

fn the_views_around_the_probes_keep_up() {
    let mut scene = Scene::new(1500);
    scene.assets.gi_supported = true;
    scene.lighting.gi.enabled = true;
    scene.check("first");
    let seen = |scene: &Scene| scene.frame.gi.as_ref().map_or(0, |gi| gi.capture.visible);
    assert!(seen(&scene) > 0, "the probes' views see models");
    for step in 0..24 {
        // The probes' grid follows the camera.
        swoop(&mut scene, step);
        let e = scene.models[step * 61 % scene.models.len()];
        match step % 4 {
            0 => scene.world.get_mut::<Transform3D>(e).unwrap().position.y += 2.0,
            1 => scene.world.get_mut::<Model>(e).unwrap().color = Color::rgb(0.3, 0.6, 0.9),
            2 => {
                scene.world.despawn(e);
                scene.models.retain(|&m| m != e);
            }
            _ => {}
        }
        scene.check(&format!("step {step}"));
    }
    scene.lighting.gi.enabled = false;
    scene.check("no probes");
    assert!(scene.frame.gi.is_none());
}

/// Runs every test twice: with the models collected in one pass on this
/// thread (as with one core, or in a browser), and compared on all cores.
macro_rules! both_ways {
    ($($name:ident),* $(,)?) => {
        mod on_one_core {
            $(#[test]
            fn $name() {
                super::SERIAL.set(Some(true));
                super::$name();
            })*
        }
        mod on_several_cores {
            $(#[test]
            fn $name() {
                super::SERIAL.set(Some(false));
                super::$name();
            })*
        }
    };
}

both_ways!(
    a_still_scene_is_left_as_it_was,
    a_first_frame_is_packed_by_view_then_mesh_like_a_sorted_one,
    a_moving_camera_keeps_what_it_still_sees_in_place,
    a_moved_model_rewrites_its_own_instances_only,
    every_kind_of_edit_draws_what_a_frame_built_from_nothing_draws,
    a_change_to_a_mesh_reaches_the_models_that_use_it,
    random_edits_always_draw_what_a_frame_built_from_nothing_draws,
    shadow_only_casters_follow_edits_too,
    shadow_only_models_cast_their_shadow_but_no_camera_draws_them,
    a_kept_cascade_draws_nothing_and_is_right_when_its_turn_comes,
    models_are_counted_once_each_also_in_cascades_that_are_kept,
    moved_models_are_listed_with_where_they_were,
    what_is_kept_per_model_is_bounded_and_stops_growing,
    while_most_models_move_every_edit_is_still_followed,
    a_collection_can_change_between_one_core_and_several,
    many_models_spawned_at_once_are_drawn_also_while_most_move,
    lod_groups_keep_up_with_the_camera_and_edits,
    terrain_tiles_keep_up_with_the_camera_and_edits,
    skinned_models_keep_up_as_their_number_changes,
    point_and_spot_light_shadows_keep_up_with_their_lights,
    models_hidden_behind_an_earlier_frames_depth_keep_up_with_it,
    models_outside_the_view_are_counted_from_what_is_kept,
    while_most_models_move_their_culling_isnt_counted,
    a_target_cameras_view_keeps_up,
    the_views_around_the_probes_keep_up,
);
