//! Turning Unity scenes and prefabs into Ponko scenes: every drawn mesh
//! becomes a `.glb` with its materials, its textures become PNGs, and the
//! scene a `ponko-world` save of `Transform3D` and `Model` entities that
//! `Ctx::load_scene` loads.
//!
//! Unity is left-handed (+Z forward) and Ponko right-handed (-Z forward):
//! everything is mirrored along Z on the way, and UVs flipped to glTF's
//! top-down v.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use glam::{Mat4, Quat, Vec3};
use serde_json::{Value as Json, json};

use crate::fbx::{ModelFile, ModelMaterial};
use crate::glb::{self, Primitive, Vertices};
use crate::hierarchy::GameObject;
use crate::material::{Surface, UnityMaterial};
use crate::{Error, Hierarchy, ObjRef, Project, UnityFile, UnityMesh, UnityTexture, Value};

const MESH_FILTER: i32 = 33;
const MESH_RENDERER: i32 = 23;
const SKINNED_MESH_RENDERER: i32 = 137;
const LOD_GROUP: i32 = 205;
const BOX_COLLIDER: i32 = 65;
const SPHERE_COLLIDER: i32 = 135;
const CAPSULE_COLLIDER: i32 = 136;
const MESH_COLLIDER: i32 = 64;
/// Model files read with ufbx, by lowercase extension.
const MODEL_FILES: [&str; 2] = ["fbx", "obj"];
const ANIMATOR: i32 = 95;
const ANIMATION: i32 = 111;
/// The GUID Unity's built-in resources (Cube, Sphere, Default-Material) share.
const BUILTIN: &str = "0000000000000000e000000000000000";

/// What an import made and what it left out, and why.
#[derive(Debug, Default, Clone)]
pub struct Report {
    /// Entities in the scenes written.
    pub entities: usize,
    /// `.glb` files written.
    pub meshes: usize,
    /// Images written.
    pub textures: usize,
    /// Colliders written (`Importer::colliders`).
    pub colliders: usize,
    /// `LodGroup`s written (`Importer::lod_groups`).
    pub lod_groups: usize,
    /// What was left out, by reason, with how often.
    pub skipped: BTreeMap<String, usize>,
    /// The shaders of the materials imported, by asset path (or built-in
    /// id), with how many renderers use each: which need a Ponko shader.
    pub shaders: BTreeMap<String, usize>,
}

impl Report {
    fn skip(&mut self, why: impl Into<String>) {
        *self.skipped.entry(why.into()).or_default() += 1;
    }
}

/// Where a texture's texels are.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    /// An image file.
    File(PathBuf),
    /// A texture asset: the file, and the object's id in it.
    Asset(PathBuf, i64),
}

/// An image to write once the scenes are read, from source images.
#[derive(Debug, Clone, PartialEq)]
enum Job {
    /// A texture as it is (made at most `max` texels on a side).
    Copy { from: Source, max: u32 },
    /// glTF's occlusion (red), roughness (green) and metalness (blue) from
    /// Unity's maps: metalness in red and smoothness in alpha of `metal`,
    /// or smoothness in `gloss`'s alpha; occlusion in `occlusion`'s green.
    Orm {
        metal: Option<Source>,
        gloss: Option<Source>,
        occlusion: Option<Source>,
        smoothness: f32,
        occlusion_strength: f32,
        max: u32,
    },
}

/// Imports Unity scenes and prefabs of one project into a folder of a
/// Ponko game's `assets/`, sharing meshes and textures between them.
///
/// ```no_run
/// use ponko_unity::{Hierarchy, Importer, Project};
///
/// let project = Project::open("MyUnityGame").unwrap();
/// let file = ponko_unity::UnityFile::read(project.root().join("Assets/Scenes/Town.unity")).unwrap();
/// let scene = Hierarchy::build(&file, &project).unwrap();
/// let mut import = Importer::new(&project, "my_game/assets/town", "town/");
/// let world = import.scene(&scene).unwrap();
/// std::fs::write("my_game/assets/town/scene.json", serde_json::to_string(&world).unwrap()).unwrap();
/// let report = import.finish().unwrap();
/// println!("{} meshes, {} textures", report.meshes, report.textures);
/// ```
pub struct Importer<'p> {
    project: &'p Project,
    animation_clips: Vec<crate::AnimationClip>,
    /// The output folder, and its asset name (`"town/"`).
    out: PathBuf,
    prefix: String,
    /// Mesh files written, by mesh and materials: their asset names.
    models: HashMap<(ObjRef, Vec<ObjRef>), Option<String>>,
    materials: HashMap<ObjRef, Option<UnityMaterial>>,
    /// Model files (FBX, OBJ) read so far, by GUID.
    model_files: HashMap<String, Result<ModelFile, String>>,
    /// Images to write, by file name under `meshes/textures/`.
    jobs: BTreeMap<String, Job>,
    used_names: HashSet<String>,
    /// Shaders the game draws with its own (`custom_shader`), by asset path.
    custom_shaders: HashSet<String>,
    /// Their materials, by name, for `materials.json`.
    custom_materials: BTreeMap<String, Json>,
    /// How some of them want their meshes' vertex colors (`vertex_colors`).
    vertex_colors: HashMap<String, VertexColors>,
    /// Whether scenes get their colliders too (`colliders`).
    colliders: bool,
    /// Whether LOD groups come over (`lod_groups`).
    lod_groups: bool,
    report: Report,
}

/// A renderer of a LOD group's level: its mesh file, where it is (in
/// Ponko's space) and its Cast Shadows setting.
struct GroupRenderer {
    mesh: String,
    world: Mat4,
    cast_shadows: i64,
}

/// Makes a mesh's vertex colors for a custom shader (`vertex_colors`).
type VertexColors = std::rc::Rc<dyn Fn(&UnityMesh, usize) -> [f32; 4]>;

impl<'p> Importer<'p> {
    /// Registers standalone clips for subsequently exported skinned meshes.
    /// Paths are relative to each skin's root; unresolved paths enter the report.
    /// Call before `scene`, since meshes are cached after export.
    ///
    /// ```no_run
    /// # let project = ponko_unity::Project::open("Project")?;
    /// let mut importer = ponko_unity::Importer::new(&project, "out", "");
    /// importer.animation_clips([ponko_unity::AnimationClip::read("Assets/Walk.anim")?]);
    /// # Ok::<(), ponko_unity::Error>(())
    /// ```
    pub fn animation_clips(&mut self, clips: impl IntoIterator<Item = crate::AnimationClip>) {
        self.animation_clips.extend(clips);
    }

    /// Writes into `out` (a folder under the game's `assets/`), whose asset
    /// name is `prefix` (`"town/"` for `assets/town`).
    pub fn new(project: &'p Project, out: impl Into<PathBuf>, prefix: &str) -> Self {
        let mut prefix = prefix.to_string();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        Self {
            project,
            animation_clips: Vec::new(),
            out: out.into(),
            prefix,
            models: HashMap::new(),
            materials: HashMap::new(),
            model_files: HashMap::new(),
            jobs: BTreeMap::new(),
            used_names: HashSet::new(),
            custom_shaders: HashSet::new(),
            custom_materials: BTreeMap::new(),
            vertex_colors: HashMap::new(),
            colliders: false,
            lod_groups: true,
            report: Report::default(),
        }
    }

    /// Whether scenes get their colliders: off at first. On, every enabled
    /// box, sphere, capsule and mesh collider of an active object (not
    /// triggers) becomes an entity of its own with a fixed `RigidBody3D`
    /// and a `Collider3D`, the object's scale worked into the shape (3D
    /// physics doesn't scale colliders). A mesh collider's mesh is written
    /// as a `.glb` like a model's and named in the scene; loading the scene
    /// makes it a triangle mesh, or a convex hull for a convex one. Games
    /// loading such a scene need 3D physics on (`Game::physics3d`). Bodies
    /// that move (Unity's `Rigidbody`) are left as fixed ones too.
    ///
    /// ```no_run
    /// # let project = ponko_unity::Project::open("MyUnityGame").unwrap();
    /// let mut import = ponko_unity::Importer::new(&project, "my_game/assets/town", "town/");
    /// import.colliders(true);
    /// ```
    pub fn colliders(&mut self, on: bool) {
        self.colliders = on;
    }

    /// Whether Unity `LODGroup`s come over as Ponko `LodGroup`s: on at
    /// first. On, the renderers of every level of detail are kept, under one
    /// entity at the Unity group's object with the group's `size`
    /// (`m_Size`), reference point, each level's screen-relative height
    /// and crossfade width. Each renderer is placed relative to the group,
    /// and its Cast Shadows setting becomes the level's shadow casters
    /// (Off and Shadows Only renderers make the level cast different
    /// meshes than it shows). Cross-fading (`m_FadeMode`) needs the group
    /// not to animate it; animated fades (`m_AnimateCrossFading`) become
    /// a fade over a band of screen heights, and are counted in the report.
    /// Off, only LOD 0 is kept and the far levels are left out.
    ///
    /// Skinned renderers are never part of a group: one in LOD 0 stays an
    /// entity of its own, and the ones in far levels are left out.
    ///
    /// ```no_run
    /// # let project = ponko_unity::Project::open("MyUnityGame").unwrap();
    /// let mut import = ponko_unity::Importer::new(&project, "my_game/assets/town", "town/");
    /// import.lod_groups(false); // LOD 0 only
    /// ```
    pub fn lod_groups(&mut self, on: bool) {
        self.lod_groups = on;
    }

    /// Says the game draws materials of `shader` (its asset path, as
    /// [`Report::shaders`] lists it) with a Ponko shader of its own. Their
    /// meshes keep their vertex colors and UV set 1 (as Unity has it, not
    /// flipped: shaders read it as `s.uv1`), and [`finish`](Self::finish)
    /// writes `materials.json`: each such material by name (the name its
    /// glTF material has, for `Graphics::loaded_materials`), with its shader and
    /// every number, color and texture it sets, the textures written as
    /// PNGs beside the others:
    ///
    /// ```json
    /// { "Plaster": { "shader": "Assets/Shaders/Wall.shader",
    ///   "floats": { "_Smoothness": 0.12 }, "colors": { "_BaseColor": [1, 1, 1, 1] },
    ///   "textures": { "_MaskMap": { "file": "town/meshes/textures/9f3c.png", "scale": [1, 1], "offset": [0, 0] } } } }
    /// ```
    ///
    /// Call it before importing scenes.
    pub fn custom_shader(&mut self, shader: &str) {
        self.custom_shaders.insert(shader.to_string());
    }

    /// Like [`custom_shader`](Self::custom_shader), and the vertex colors of
    /// the meshes drawn with `shader` are `colors(mesh, vertex)` instead of
    /// the mesh's own: for a shader that reads something else per vertex,
    /// such as a second UV set, from the color its Ponko port gets
    /// (`s.color`). They're stored as 8 bits per channel, from 0 to 1.
    ///
    /// ```no_run
    /// # let project = ponko_unity::Project::open("MyUnityGame").unwrap();
    /// let mut import = ponko_unity::Importer::new(&project, "my_game/assets/town", "town/");
    /// // A river shader that reads its flow from UV set 1: speed up to 4, foam 0 to 1.
    /// import.vertex_colors("Assets/Shaders/River.shader", |mesh, v| {
    ///     let [speed, foam] = mesh.uvs[1].get(v).copied().unwrap_or_default();
    ///     [speed / 4.0, foam, 0.0, 1.0]
    /// });
    /// ```
    pub fn vertex_colors(&mut self, shader: &str, colors: impl Fn(&UnityMesh, usize) -> [f32; 4] + 'static) {
        self.custom_shader(shader);
        self.vertex_colors.insert(shader.to_string(), std::rc::Rc::new(colors));
    }

    /// What's been imported and left out so far.
    #[must_use]
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// The `ponko-world` save of `scene`: an entity per drawn mesh, placed
    /// in the world (the hierarchy is baked in). Writes the meshes it
    /// needs; their textures are written by [`finish`](Self::finish).
    ///
    /// # Errors
    ///
    /// If a mesh file can't be written.
    pub fn scene(&mut self, scene: &Hierarchy) -> Result<Json, Error> {
        let objects = &scene.objects;
        // World matrices and activity, parents first.
        let mut world: Vec<Mat4> = Vec::with_capacity(objects.len());
        let mut active: Vec<bool> = Vec::with_capacity(objects.len());
        for o in objects {
            let local = Mat4::from_scale_rotation_translation(
                Vec3::from(o.local_scale()),
                Quat::from_array(o.local_rotation()).normalize(),
                Vec3::from(o.local_position()),
            );
            let (parent, parent_active) = o.parent.map_or((Mat4::IDENTITY, true), |p| (world[p], active[p]));
            world.push(parent * local);
            active.push(o.active && parent_active);
        }
        // The LOD groups' renderers (by the id of the renderer component):
        // their group's object and level. Without groups (`lod_groups`),
        // the renderers of LOD 1 and up are left out instead.
        let mut member: HashMap<i64, (usize, usize)> = HashMap::new();
        let mut far: HashSet<i64> = HashSet::new();
        for (g, o) in objects.iter().enumerate() {
            if let Some(lod) = o.component(LOD_GROUP)
                && let Some(lods) = lod.body["m_LODs"].as_seq()
            {
                for (level, l) in lods.iter().enumerate() {
                    for r in l["renderers"].as_seq().unwrap_or(&[]) {
                        if let Some(r) = r["renderer"].obj_ref() {
                            if self.lod_groups {
                                member.insert(r.file_id, (g, level));
                            } else if level > 0 {
                                far.insert(r.file_id);
                            }
                        }
                    }
                }
            }
        }
        // What each group draws, by its object: per level, the renderers
        // with their Cast Shadows setting.
        let mut groups: BTreeMap<usize, Vec<Vec<GroupRenderer>>> = BTreeMap::new();

        let mut entities = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            if o.model.is_some() {
                self.report.skip("an instance of a model file (FBX, OBJ, ...)");
            }
            let (filter, renderer) = if let Some(renderer) = o.component(SKINNED_MESH_RENDERER) {
                (renderer, renderer)
            } else if let (Some(filter), Some(renderer)) = (o.component(MESH_FILTER), o.component(MESH_RENDERER)) {
                (filter, renderer)
            } else {
                continue;
            };
            if !active[i] {
                self.report.skip("inactive");
                continue;
            }
            if renderer.body["m_Enabled"].as_i64() == Some(0) {
                self.report.skip("renderer off");
                continue;
            }
            // Cast Shadows: Off (0), On (1), Two Sided (2) or Shadows Only
            // (3). Shadows Only is drawn only into the shadow map, often a
            // simplified stand-in for the shadows of the pieces around it
            // (which are Off): a ShadowMesh with no Model.
            let cast_shadows = renderer.body["m_CastShadows"].as_i64().unwrap_or(1);
            let bones = renderer.body["m_Bones"].as_seq().unwrap_or_default();
            // Skinned renderers can't be part of a group (they are posed by
            // their skeleton): LOD 1 and up are left out.
            let member_of = member.get(&renderer.file_id).copied().filter(|_| bones.is_empty());
            if far.contains(&renderer.file_id)
                || (member_of.is_none() && member.get(&renderer.file_id).is_some_and(|m| m.1 > 0))
            {
                self.report.skip("a far level of detail (LOD 1+)");
                continue;
            }
            let Some(mesh) = filter.body["m_Mesh"].obj_ref().filter(|m| !m.is_none()) else {
                self.report.skip("no mesh");
                continue;
            };
            let materials: Vec<ObjRef> = renderer.body["m_Materials"]
                .as_seq()
                .unwrap_or(&[])
                .iter()
                .filter_map(Value::obj_ref)
                .collect();
            let Some(name) = (if bones.is_empty() {
                self.model(&mesh, &materials)?
            } else {
                self.write_model(
                    &mesh,
                    &materials,
                    Some((scene, &world, i, bones, &renderer.body["m_RootBone"])),
                )?
            }) else {
                continue;
            };
            if let Some((group, level)) = member_of {
                let levels = groups.entry(group).or_default();
                if levels.len() <= level {
                    levels.resize_with(level + 1, Vec::new);
                }
                levels[level].push(GroupRenderer {
                    mesh: name,
                    world: mirror(world[i]),
                    cast_shadows,
                });
                continue;
            }
            let (scale, rotation, position) = mirror(world[i]).to_scale_rotation_translation();
            let mut components = json!({
                "Transform3D": {
                    "position": position.to_array(),
                    "rotation": rotation.to_array(),
                    "scale": scale.to_array(),
                },
            });
            match cast_shadows {
                3 => {
                    components["ShadowMesh"] = json!({ "mesh": name });
                }
                0 => {
                    components["Model"] = json!({ "mesh": name });
                    components["ShadowMesh"] = json!({ "mesh": null });
                }
                _ => components["Model"] = json!({ "mesh": name }),
            }
            entities.push(json!({ "entity": entities.len(), "components": components }));
        }
        for (g, levels) in groups {
            if let Some(components) = self.lod_group(&objects[g], mirror(world[g]), levels) {
                entities.push(json!({ "entity": entities.len(), "components": components }));
            }
        }
        if self.colliders {
            for (i, o) in objects.iter().enumerate() {
                if !active[i] {
                    continue;
                }
                for c in &o.components {
                    if let Some(collider) = self.collider(c, world[i])? {
                        entities.push(json!({ "entity": entities.len(), "components": collider }));
                    }
                }
            }
        }
        self.report.entities += entities.len();
        Ok(json!({
            "format": "ponko-world",
            "version": 1,
            "entities": entities,
            "resources": {},
        }))
    }

    /// The components of a group's entity: `Transform3D` and `LodGroup`
    /// (from the Unity `LODGroup` of `object`, placed at `world` in
    /// Ponko's space, with the `renderers` of each level); `None` if a
    /// group has nothing to draw or can't be placed.
    fn lod_group(&mut self, object: &GameObject, world: Mat4, mut levels: Vec<Vec<GroupRenderer>>) -> Option<Json> {
        let lod = object.component(LOD_GROUP)?;
        let b = &lod.body;
        let lods = b["m_LODs"].as_seq().unwrap_or(&[]);
        levels.resize_with(lods.len().max(levels.len()), Vec::new);
        if levels.iter().all(Vec::is_empty) || world.determinant().abs() < 1e-12 {
            return None;
        }
        // Cross-fade (1) and SpeedTree (2) fade; None (0) switches at once.
        let fades = b["m_FadeMode"].as_i64().unwrap_or(0) != 0;
        let animated = b["m_AnimateCrossFading"].as_i64() == Some(1);
        if fades && animated {
            self.report
                .skip("an animated cross-fade (fades over a band of screen heights instead)");
        }
        let inverse = world.inverse();
        let renderer = |r: &GroupRenderer| {
            let (scale, rotation, position) = (inverse * r.world).to_scale_rotation_translation();
            json!({
                "mesh": r.mesh,
                "offset": {
                    "position": position.to_array(),
                    "rotation": rotation.to_array(),
                    "scale": scale.to_array(),
                },
            })
        };
        let levels: Vec<Json> = levels
            .iter()
            .enumerate()
            .map(|(k, renderers)| {
                let l = &lods[k.min(lods.len().saturating_sub(1))];
                let height = if lods.is_empty() {
                    0.5 / (1 << k.min(10)) as f32
                } else {
                    l["screenRelativeHeight"]
                        .as_f32()
                        // Unity's own defaults when a file has none.
                        .unwrap_or(0.5 / (1 << k.min(10)) as f32)
                };
                let width = if !fades {
                    0.0
                } else if animated {
                    0.2
                } else {
                    l["fadeTransitionWidth"].as_f32().unwrap_or(0.0).clamp(0.0, 1.0)
                };
                // Shadows: the level's own meshes cast them, unless some
                // renderer casts none (Off) or only a shadow (Shadows Only).
                let own = renderers.iter().all(|r| matches!(r.cast_shadows, 1 | 2));
                let shadow = if own {
                    json!("Same")
                } else {
                    let casters: Vec<Json> = renderers
                        .iter()
                        .filter(|r| r.cast_shadows != 0)
                        .map(&renderer)
                        .collect();
                    json!({ "Casters": casters })
                };
                let shown: Vec<Json> = renderers
                    .iter()
                    .filter(|r| r.cast_shadows != 3)
                    .map(&renderer)
                    .collect();
                json!({
                    "screen_height": height,
                    "fade_width": width,
                    "renderers": shown,
                    "shadow": shadow,
                })
            })
            .collect();
        let at = |v: &Value| {
            Vec3::new(
                v["x"].as_f32().unwrap_or(0.0),
                v["y"].as_f32().unwrap_or(0.0),
                // Ponko's Z is Unity's turned around.
                -v["z"].as_f32().unwrap_or(0.0),
            )
        };
        let (scale, rotation, position) = world.to_scale_rotation_translation();
        self.report.lod_groups += 1;
        Some(json!({
            "Transform3D": {
                "position": position.to_array(),
                "rotation": rotation.to_array(),
                "scale": scale.to_array(),
            },
            "LodGroup": {
                "size": b["m_Size"].as_f32().unwrap_or(1.0),
                "center": at(&b["m_LocalReferencePoint"]).to_array(),
                "levels": levels,
            },
        }))
    }

    /// A collider component's entity (Unity's `world` matrix): its
    /// `Transform3D`, a fixed `RigidBody3D` and the `Collider3D`; `None`
    /// for other components, and for triggers, disabled colliders and
    /// meshes that can't be read (counted in the report).
    fn collider(&mut self, c: &crate::UnityObject, world: Mat4) -> Result<Option<Json>, Error> {
        if ![BOX_COLLIDER, SPHERE_COLLIDER, CAPSULE_COLLIDER, MESH_COLLIDER].contains(&c.class_id) {
            return Ok(None);
        }
        let b = &c.body;
        if b["m_Enabled"].as_i64() == Some(0) {
            self.report.skip("collider off");
            return Ok(None);
        }
        if b["m_IsTrigger"].as_i64() == Some(1) {
            self.report.skip("a trigger collider");
            return Ok(None);
        }
        let v3 = |v: &Value| {
            Vec3::new(
                v["x"].as_f32().unwrap_or(0.0),
                v["y"].as_f32().unwrap_or(0.0),
                v["z"].as_f32().unwrap_or(0.0),
            )
        };
        let (scale, mut rotation, position) = mirror(world).to_scale_rotation_translation();
        let scale = scale.abs();
        // Local vectors in the mirrored frame: z turned around.
        let flip = |v: Vec3| Vec3::new(v.x, v.y, -v.z);
        let center = flip(v3(&b["m_Center"]) * scale);
        let shape = match c.class_id {
            BOX_COLLIDER => json!({ "Cuboid": { "size": (v3(&b["m_Size"]) * scale).abs().to_array() } }),
            SPHERE_COLLIDER => {
                let r = b["m_Radius"].as_f32().unwrap_or(0.5) * scale.max_element();
                json!({ "Sphere": { "radius": r } })
            }
            CAPSULE_COLLIDER => {
                // Unity's axis: 0 X, 1 Y, 2 Z; Ponko's capsules stand on Y.
                let axis = b["m_Direction"].as_i64().unwrap_or(1) as usize;
                let along = scale[axis.min(2)];
                let across = (0..3).filter(|&a| a != axis).map(|a| scale[a]).fold(0.0, f32::max);
                let r = b["m_Radius"].as_f32().unwrap_or(0.5) * across;
                let h = (b["m_Height"].as_f32().unwrap_or(2.0) * along - 2.0 * r).max(0.0);
                rotation *= match axis {
                    0 => Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                    2 => Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                    _ => Quat::IDENTITY,
                };
                json!({ "Capsule": { "height": h, "radius": r } })
            }
            _ => {
                let Some(mesh) = b["m_Mesh"].obj_ref().filter(|m| !m.is_none()) else {
                    self.report.skip("a mesh collider without a mesh");
                    return Ok(None);
                };
                let Some(name) = self.model(&mesh, &[])? else {
                    return Ok(None);
                };
                json!({ "Mesh": { "file": name, "scale": scale.to_array(), "convex": b["m_Convex"].as_i64() == Some(1) } })
            }
        };
        // A capsule turned on its side has its offset in the turned frame.
        let offset = if c.class_id == CAPSULE_COLLIDER {
            let (_, unturned, _) = mirror(world).to_scale_rotation_translation();
            rotation.inverse() * (unturned * center)
        } else {
            center
        };
        self.report.colliders += 1;
        Ok(Some(json!({
            "Transform3D": { "position": position.to_array(), "rotation": rotation.to_array(), "scale": [1.0, 1.0, 1.0] },
            "RigidBody3D": { "kind": "Fixed" },
            "Collider3D": { "shape": shape, "offset": offset.to_array() },
        })))
    }

    /// The asset name of the `.glb` for `mesh` drawn with `materials`,
    /// written on first use; `None` when it can't be imported (and why is
    /// in the report).
    fn model(&mut self, mesh: &ObjRef, materials: &[ObjRef]) -> Result<Option<String>, Error> {
        let key = (mesh.clone(), materials.to_vec());
        if let Some(name) = self.models.get(&key) {
            return Ok(name.clone());
        }
        let name = self.write_model(mesh, materials, None)?;
        self.models.insert(key, name.clone());
        Ok(name)
    }

    fn write_model(
        &mut self,
        mesh: &ObjRef,
        materials: &[ObjRef],
        skeleton: Option<SceneSkeleton<'_>>,
    ) -> Result<Option<String>, Error> {
        let Some(guid) = mesh.guid.as_deref() else {
            self.report.skip("a mesh saved in the scene itself");
            return Ok(None);
        };
        let mut unity = if guid == BUILTIN {
            match UnityMesh::builtin(mesh.file_id) {
                Some(m) => m,
                None => {
                    let what = crate::builtin::name(mesh.file_id).map_or(mesh.file_id.to_string(), str::to_owned);
                    self.report.skip(format!("built-in mesh {what}"));
                    return Ok(None);
                }
            }
        } else {
            match self.project_mesh(guid, mesh)? {
                Some(m) => m,
                None => return Ok(None),
            }
        };
        let mut clip_root = None;
        if let Some((scene, world, renderer, bones, root)) = skeleton
            && let Some(skin) = &mut unity.skin
        {
            apply_scene_bones(skin, scene, world, renderer, bones, root);
            clip_root = animator_node(skin, scene, renderer, bones);
            unity.name = format!("{}_renderer_{renderer}", unity.name);
        }
        let mats: Vec<Option<UnityMaterial>> = materials.iter().map(|m| self.material(m)).collect();
        let glb = self.build_glb(&unity, &mats, clip_root);
        let file_name = self.unique_name(&unity.name, mesh, materials);
        let dir = self.out.join("meshes");
        std::fs::create_dir_all(&dir).map_err(|e| Error::new(format!("{}: {e}", dir.display())))?;
        let at = dir.join(&file_name);
        std::fs::write(&at, glb).map_err(|e| Error::new(format!("{}: {e}", at.display())))?;
        self.report.meshes += 1;
        Ok(Some(format!("{}meshes/{file_name}", self.prefix)))
    }

    /// A mesh from a file of the project: a mesh asset, FBX or OBJ; `None`
    /// when it can't be read (and why is in the report).
    fn project_mesh(&mut self, guid: &str, mesh: &ObjRef) -> Result<Option<UnityMesh>, Error> {
        let Some(path) = self.project.path_of(guid) else {
            self.report.skip("a mesh not in the project");
            return Ok(None);
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let unity = if ext == "asset" {
            let file = match UnityFile::read(self.project.root().join(path)) {
                Ok(f) => f,
                Err(e) => {
                    self.report.skip(format!("an unreadable mesh file: {e}"));
                    return Ok(None);
                }
            };
            let Some(object) = file.get(mesh.file_id) else {
                self.report.skip("a mesh missing from its file");
                return Ok(None);
            };
            match UnityMesh::from_object(object) {
                Ok(m) => m,
                Err(e) => {
                    self.report
                        .skip(format!("a mesh that can't be read: {}", first_words(&e.message)));
                    return Ok(None);
                }
            }
        } else if MODEL_FILES.contains(&ext.as_str()) {
            let mesh = match self.model_file(guid) {
                Ok(model) => model.mesh(mesh.file_id),
                Err(e) => Err(Error::new(e.clone())),
            };
            match mesh {
                Ok(m) => m,
                Err(e) => {
                    self.report.skip(format!(
                        "a mesh that can't be read from a .{ext}: {}",
                        first_words(&e.message)
                    ));
                    return Ok(None);
                }
            }
        } else {
            self.report.skip(format!("a mesh in a .{ext} file"));
            return Ok(None);
        };
        Ok(Some(unity))
    }
    /// The model file (FBX, OBJ) with this GUID, read once.
    fn model_file(&mut self, guid: &str) -> &Result<ModelFile, String> {
        let project = self.project;
        self.model_files.entry(guid.to_string()).or_insert_with(|| {
            let path = project
                .path_of(guid)
                .ok_or_else(|| format!("no asset has GUID {guid}"))?;
            ModelFile::open(&project.root().join(path)).map_err(|e| e.message)
        })
    }

    /// The material `r` names: a material asset, or one inside a model
    /// file (made from the model's own, or the asset its `.meta` maps it
    /// to); `None` when it can't be found.
    fn read_material(&mut self, r: &ObjRef) -> Option<UnityMaterial> {
        let guid = r.guid.as_deref().filter(|g| *g != BUILTIN)?;
        let project = self.project;
        let ext = project
            .path_of(guid)?
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if MODEL_FILES.contains(&ext.as_str()) {
            return match self.model_file(guid).as_ref().ok()?.material(r.file_id, project)? {
                ModelMaterial::Embedded(m) => Some(*m),
                ModelMaterial::Remapped(to) => material_asset(project, &to),
            };
        }
        material_asset(project, r)
    }

    /// The material `r` names, read once; `None` (drawn plain) when it
    /// can't be found.
    fn material(&mut self, r: &ObjRef) -> Option<UnityMaterial> {
        if let Some(m) = self.materials.get(r) {
            if let Some(m) = m {
                *self.report.shaders.entry(self.shader_name(m)).or_default() += 1;
            }
            return m.clone();
        }
        let m = self.read_material(r);
        if m.is_none() && !r.is_none() {
            self.report.skip("a material that can't be read (drawn plain)");
        }
        if let Some(m) = &m {
            *self.report.shaders.entry(self.shader_name(m)).or_default() += 1;
            if self.is_custom(m) {
                self.describe(m);
            }
        }
        self.materials.insert(r.clone(), m.clone());
        m
    }

    fn is_custom(&self, m: &UnityMaterial) -> bool {
        !self.custom_shaders.is_empty() && self.custom_shaders.contains(&self.shader_name(m))
    }

    /// Adds `m` to `materials.json`, queuing its textures.
    fn describe(&mut self, m: &UnityMaterial) {
        let mut textures = serde_json::Map::new();
        for (prop, t) in &m.textures {
            if let Some((name, job)) = self.copy_job(&t.texture) {
                self.jobs.entry(name.clone()).or_insert(job);
                textures.insert(
                    prop.clone(),
                    json!({ "file": format!("{}meshes/textures/{name}", self.prefix), "scale": t.scale, "offset": t.offset }),
                );
            }
        }
        let entry = json!({
            "shader": self.shader_name(m),
            "floats": m.floats,
            "colors": m.colors,
            "textures": textures,
        });
        match self.custom_materials.get(&m.name) {
            Some(old) if *old != entry => self.report.skip(format!(
                "a second material named {} (materials.json keeps the first)",
                m.name
            )),
            Some(_) => {}
            None => {
                self.custom_materials.insert(m.name.clone(), entry);
            }
        }
    }

    fn shader_name(&self, m: &UnityMaterial) -> String {
        match m.shader.as_deref() {
            Some(BUILTIN) | None => format!("built-in shader {}", m.shader_file_id),
            Some(g) => self.project.path_of(g).map_or_else(
                || format!("shader {g}"),
                |p| {
                    let p = p.strip_prefix(self.project.root()).unwrap_or(p);
                    p.to_string_lossy().replace('\\', "/")
                },
            ),
        }
    }

    /// The mesh as a `.glb`, in glTF's space, a primitive per submesh with
    /// its material. Standalone clips (`animation_clips`) resolve their
    /// paths from skin node `clip_root` (the Animator's object), else from
    /// the skin's root bone.
    fn build_glb(&mut self, mesh: &UnityMesh, mats: &[Option<UnityMaterial>], clip_root: Option<usize>) -> Vec<u8> {
        let mut textures: Vec<String> = Vec::new();
        let mut materials: Vec<Json> = Vec::new();
        let mut vertices = Vertices {
            skin: mesh
                .skin
                .clone()
                .or_else(|| {
                    if self.animation_clips.is_empty() || mesh.blend_shapes.is_empty() {
                        return None;
                    }
                    Some(crate::MeshSkin {
                        nodes: vec![crate::SkinNode {
                            name: "Root".into(),
                            parent: None,
                            matrix: Mat4::IDENTITY.to_cols_array(),
                        }],
                        root: Some(0),
                        joints: vec![0],
                        inverse_binds: vec![Mat4::IDENTITY.to_cols_array()],
                        indices: vec![[0; 4]; mesh.positions.len()],
                        weights: vec![[1., 0., 0., 0.]; mesh.positions.len()],
                        ..Default::default()
                    })
                })
                .map(|mut skin| {
                    let root = clip_root
                        .filter(|&r| r < skin.nodes.len())
                        .or(skin.root)
                        .or_else(|| skin.nodes.iter().position(|n| n.parent.is_none()))
                        .unwrap_or(0);
                    for clip in &self.animation_clips {
                        // Baked runtime curves name blend shapes by hash only.
                        let clip = clip.with_blend_shape_names(mesh.blend_shapes.iter().map(|b| b.name.as_str()));
                        for diagnostic in clip.append_to_skin(&mut skin, root) {
                            self.report.skip(diagnostic);
                        }
                    }
                    for node in &mut skin.nodes {
                        node.matrix = mirror(Mat4::from_cols_array(&node.matrix)).to_cols_array();
                    }
                    for clip in &mut skin.animations {
                        for samples in &mut clip.nodes {
                            for matrix in samples {
                                *matrix = mirror(Mat4::from_cols_array(matrix)).to_cols_array();
                            }
                        }
                    }
                    for bind in &mut skin.inverse_binds {
                        *bind = mirror(Mat4::from_cols_array(bind)).to_cols_array();
                    }
                    skin
                }),
            ..Default::default()
        };
        vertices.targets = mesh
            .blend_shapes
            .iter()
            .map(|b| glb::Target {
                name: b.name.clone(),
                weight: b.weight,
                channel: b.channel.clone(),
                frame_weight: b.frame_weight,
                ..Default::default()
            })
            .collect();
        let mut primitives = Vec::new();
        // Vertex colors, for meshes a custom shader draws (`custom_shader`),
        // made by that shader's function if it has one (`vertex_colors`).
        let made = mats
            .iter()
            .flatten()
            .find_map(|m| self.vertex_colors.get(&self.shader_name(m)).cloned());
        let custom = mats.iter().flatten().any(|m| self.is_custom(m));
        let colors = made.is_some() || (!mesh.colors.is_empty() && custom);
        // Submeshes past the last material reuse it, as in Unity; materials
        // past the last submesh draw it again, which is skipped.
        for (s, indices) in mesh.submeshes.iter().enumerate() {
            let m = mats.get(s).or(mats.last()).cloned().flatten();
            let (tiling, offset) = m.as_ref().map_or(([1.0; 2], [0.0; 2]), |m| (m.tiling, m.offset));
            // Each submesh gets its own vertices: they can differ by the
            // material's tiling.
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut out = Vec::with_capacity(indices.len());
            for tri in indices.as_chunks::<3>().0 {
                // Mirrored, so the winding turns around.
                for &v in &[tri[0], tri[2], tri[1]] {
                    let at = *remap.entry(v).or_insert_with(|| {
                        let v = v as usize;
                        let p = mesh.positions.get(v).copied().unwrap_or_default();
                        if let Some(skin) = &vertices.skin {
                            vertices.joints.push(skin.indices[v]);
                            vertices.weights.push(skin.weights[v]);
                        }
                        vertices.positions.push([p[0], p[1], -p[2]]);
                        for (target, shape) in vertices.targets.iter_mut().zip(&mesh.blend_shapes) {
                            let d = shape.positions.get(v).copied().unwrap_or_default();
                            target.positions.push([d[0], d[1], -d[2]]);
                            if !shape.normals.is_empty() {
                                let n = shape.normals.get(v).copied().unwrap_or_default();
                                target.normals.push([n[0], n[1], -n[2]]);
                            }
                        }
                        if !mesh.normals.is_empty() {
                            let n = mesh.normals.get(v).copied().unwrap_or([0.0, 1.0, 0.0]);
                            vertices.normals.push([n[0], n[1], -n[2]]);
                        }
                        if let Some(made) = &made {
                            vertices.colors.push(made(mesh, v).map(|c| c.clamp(0.0, 1.0)));
                        } else if colors {
                            vertices.colors.push(mesh.colors.get(v).copied().unwrap_or([1.0; 4]));
                        }
                        if custom && !mesh.uvs[1].is_empty() {
                            vertices.uvs1.push(mesh.uvs[1].get(v).copied().unwrap_or_default());
                        }
                        if !mesh.uvs[0].is_empty() {
                            let uv = mesh.uvs[0].get(v).copied().unwrap_or_default();
                            vertices
                                .uvs
                                .push([uv[0] * tiling[0] + offset[0], 1.0 - (uv[1] * tiling[1] + offset[1])]);
                        }
                        (vertices.positions.len() - 1) as u32
                    });
                    out.push(at);
                }
            }
            let index = materials.len();
            materials.push(self.gltf_material(m.as_ref(), &mut textures));
            primitives.push(Primitive {
                indices: out,
                material: index,
            });
        }
        glb::write(&mesh.name, &vertices, &primitives, materials, &textures)
    }

    /// A glTF material for `m` (plain white without one), queuing its
    /// images and adding their URIs to `textures`.
    fn gltf_material(&mut self, m: Option<&UnityMaterial>, textures: &mut Vec<String>) -> Json {
        let Some(m) = m else {
            return json!({ "name": "plain" });
        };
        let unlit = self.shader_name(m).ends_with("Unlit.shader");
        let mut texture = |importer: &mut Self, r: &Option<ObjRef>| -> Option<usize> {
            let (name, job) = importer.copy_job(r.as_ref()?)?;
            importer.jobs.entry(name.clone()).or_insert(job);
            Some(index_of(textures, format!("textures/{name}")))
        };
        let base = texture(self, &m.base_map);
        let normal = texture(self, &m.normal_map);
        let emission = texture(self, &m.emission_map);
        let color = linear(m.color);
        let mut pbr = json!({
            "baseColorFactor": color,
            "metallicFactor": m.metallic,
            "roughnessFactor": 1.0 - m.smoothness,
        });
        if let Some(b) = base {
            pbr["baseColorTexture"] = json!({ "index": b });
        }
        let mut out = json!({ "name": m.name, "pbrMetallicRoughness": pbr, "doubleSided": m.double_sided });
        if unlit {
            // Drawn by its own light: black, glowing its color.
            out["pbrMetallicRoughness"]["baseColorFactor"] = json!([0.0, 0.0, 0.0, color[3]]);
            out["emissiveFactor"] = json!([color[0], color[1], color[2]]);
            if let Some(b) = base {
                out["emissiveTexture"] = json!({ "index": b });
            }
        } else {
            if let Some(n) = normal {
                out["normalTexture"] = json!({ "index": n, "scale": m.normal_scale });
            }
            if m.emission != [0.0; 3] {
                let e = linear([m.emission[0], m.emission[1], m.emission[2], 1.0]);
                out["emissiveFactor"] = json!([e[0], e[1], e[2]]);
                if let Some(e) = emission {
                    out["emissiveTexture"] = json!({ "index": e });
                }
            }
            if let Some((name, job)) = self.orm_job(m) {
                let has_occlusion = matches!(&job, Job::Orm { occlusion: Some(_), .. });
                self.jobs.entry(name.clone()).or_insert(job);
                let i = index_of(textures, format!("textures/{name}"));
                out["pbrMetallicRoughness"]["metallicRoughnessTexture"] = json!({ "index": i });
                out["pbrMetallicRoughness"]["roughnessFactor"] = json!(1.0);
                if m.metallic_map.is_some() {
                    out["pbrMetallicRoughness"]["metallicFactor"] = json!(1.0);
                }
                if has_occlusion {
                    out["occlusionTexture"] = json!({ "index": i });
                }
            }
        }
        match m.surface {
            Surface::Opaque => {}
            Surface::Cutout(cutoff) => {
                out["alphaMode"] = json!("MASK");
                out["alphaCutoff"] = json!(cutoff);
            }
            Surface::Transparent => out["alphaMode"] = json!("BLEND"),
        }
        out
    }

    /// The source image of texture `r`, with the size limit its import
    /// settings give; `None` when it isn't a file this can read.
    fn source(&mut self, r: &ObjRef) -> Option<(Source, u32)> {
        let guid = r.guid.as_deref()?;
        let Some(path) = self.project.path_of(guid) else {
            self.report.skip("a texture not in the project");
            return None;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "asset" {
            // Generated textures are as big as they were made.
            return Some((Source::Asset(self.project.root().join(path), r.file_id), u32::MAX));
        }
        if !["png", "jpg", "jpeg", "tga", "tif", "tiff", "bmp"].contains(&ext.as_str()) {
            self.report.skip(format!("a .{ext} texture"));
            return None;
        }
        let file = self.project.root().join(path);
        let mut meta = file.clone().into_os_string();
        meta.push(".meta");
        let max = std::fs::read_to_string(meta)
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|l| l.trim().strip_prefix("maxTextureSize:"))
                    .and_then(|n| n.trim().parse().ok())
            })
            .unwrap_or(2048);
        Some((Source::File(file), max))
    }

    fn copy_job(&mut self, r: &ObjRef) -> Option<(String, Job)> {
        let (from, max) = self.source(r)?;
        let name = match &from {
            Source::Asset(_, id) if *id != 2_800_000 => format!("{}_{id}.png", r.guid.as_deref()?),
            _ => format!("{}.png", r.guid.as_deref()?),
        };
        Some((name, Job::Copy { from, max }))
    }

    /// The ORM image `m` needs, if any map feeds it.
    fn orm_job(&mut self, m: &UnityMaterial) -> Option<(String, Job)> {
        let metal = m.metallic_map.as_ref().and_then(|r| self.source(r));
        let gloss = if m.smoothness_in_base_alpha {
            m.base_map.as_ref().and_then(|r| self.source(r))
        } else {
            None
        };
        let occlusion = m.occlusion_map.as_ref().and_then(|r| self.source(r));
        if metal.is_none() && gloss.is_none() && occlusion.is_none() {
            return None;
        }
        let max = [&metal, &gloss, &occlusion]
            .iter()
            .filter_map(|s| s.as_ref().map(|s| s.1))
            .max()?;
        let guid = |r: &Option<ObjRef>| r.as_ref().and_then(|r| r.guid.clone()).unwrap_or_default();
        let name = format!(
            "orm_{}_{}_{}_{}_{}.png",
            short(&guid(&m.metallic_map)),
            short(&if m.smoothness_in_base_alpha {
                guid(&m.base_map)
            } else {
                String::new()
            }),
            short(&guid(&m.occlusion_map)),
            (m.smoothness * 100.0).round() as i32,
            (m.occlusion_strength * 100.0).round() as i32,
        );
        Some((
            name,
            Job::Orm {
                metal: metal.map(|s| s.0),
                gloss: gloss.map(|s| s.0),
                occlusion: occlusion.map(|s| s.0),
                smoothness: m.smoothness,
                occlusion_strength: m.occlusion_strength,
                max,
            },
        ))
    }

    /// A file name for a mesh: its name, and a short hash of what it is.
    fn unique_name(&mut self, name: &str, mesh: &ObjRef, materials: &[ObjRef]) -> String {
        let clean: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        let mut hash = fnv(format!("{mesh:?}{materials:?}").as_bytes());
        loop {
            let file = format!("{clean}_{:08x}.glb", hash as u32);
            if self.used_names.insert(file.clone()) {
                return file;
            }
            hash = hash.wrapping_add(1);
        }
    }

    /// Writes the images the meshes use (in parallel) and returns the
    /// report.
    ///
    /// # Errors
    ///
    /// If an image can't be written. Images that can't be read are left
    /// out (and counted in the report).
    pub fn finish(mut self) -> Result<Report, Error> {
        use rayon::prelude::*;
        let dir = self.out.join("meshes").join("textures");
        std::fs::create_dir_all(&dir).map_err(|e| Error::new(format!("{}: {e}", dir.display())))?;
        let jobs: Vec<(String, Job)> = std::mem::take(&mut self.jobs).into_iter().collect();
        let results: Vec<Result<bool, Error>> = jobs
            .par_iter()
            .map(|(name, job)| {
                let at = dir.join(name);
                let Some(image) = run(job) else {
                    return Ok(false);
                };
                image
                    .save_with_format(&at, image::ImageFormat::Png)
                    .map_err(|e| Error::new(format!("{}: {e}", at.display())))?;
                Ok(true)
            })
            .collect();
        if !self.custom_materials.is_empty() {
            let at = self.out.join("materials.json");
            let text = serde_json::to_string_pretty(&self.custom_materials).expect("JSON");
            std::fs::write(&at, text).map_err(|e| Error::new(format!("{}: {e}", at.display())))?;
        }
        for r in results {
            if r? {
                self.report.textures += 1;
            } else {
                self.report.skip("a texture that can't be decoded");
            }
        }
        Ok(self.report)
    }
}

/// Unity's space to Ponko's: mirrored along Z on both sides.
/// The material asset (a `.mat`) `r` names.
fn material_asset(project: &Project, r: &ObjRef) -> Option<UnityMaterial> {
    project
        .load(r.guid.as_deref()?)
        .ok()
        .and_then(|f| f.get(r.file_id).map(UnityMaterial::from_object))
        .and_then(Result::ok)
}

fn mirror(m: Mat4) -> Mat4 {
    let s = Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0));
    s * m * s
}

/// Makes one image of `job`; `None` if a source can't be read.
fn run(job: &Job) -> Option<image::RgbaImage> {
    let open = |source: &Source, max: u32| -> Option<image::RgbaImage> {
        let image = match source {
            Source::File(p) => image::open(p).ok()?,
            Source::Asset(p, id) => {
                let file = UnityFile::read(p).ok()?;
                let t = UnityTexture::from_object(file.get(*id)?).ok()?;
                // An array's first layer.
                let one = t.width as usize * t.height as usize * 4;
                let rgba = image::RgbaImage::from_raw(t.width, t.height, t.rgba[..one].to_vec())?;
                image::DynamicImage::ImageRgba8(rgba)
            }
        };
        Some(fit(image, max).to_rgba8())
    };
    match job {
        Job::Copy { from, max } => open(from, *max),
        Job::Orm {
            metal,
            gloss,
            occlusion,
            smoothness,
            occlusion_strength,
            max,
        } => {
            let metal = metal.as_ref().map(|p| open(p, *max)).map(Option::unwrap_or_default);
            let gloss = gloss.as_ref().map(|p| open(p, *max)).map(Option::unwrap_or_default);
            let occlusion = occlusion.as_ref().map(|p| open(p, *max)).map(Option::unwrap_or_default);
            let (w, h) = [&metal, &gloss, &occlusion]
                .iter()
                .filter_map(|i| i.as_ref().map(image::RgbaImage::dimensions))
                .max()?;
            if w == 0 || h == 0 {
                return None;
            }
            let sized = |i: Option<image::RgbaImage>| {
                i.filter(|i| i.width() > 0).map(|i| {
                    if i.dimensions() == (w, h) {
                        i
                    } else {
                        image::imageops::resize(&i, w, h, image::imageops::FilterType::Triangle)
                    }
                })
            };
            let (metal, gloss, occlusion) = (sized(metal), sized(gloss), sized(occlusion));
            let smooth_source = gloss.as_ref().or(metal.as_ref());
            let mut out = image::RgbaImage::new(w, h);
            for (x, y, px) in out.enumerate_pixels_mut() {
                let ao = occlusion.as_ref().map_or(1.0, |o| {
                    let g = f32::from(o.get_pixel(x, y)[1]) / 255.0;
                    1.0 + (g - 1.0) * occlusion_strength
                });
                let rough = smooth_source.map_or(1.0, |s| 1.0 - f32::from(s.get_pixel(x, y)[3]) / 255.0 * smoothness);
                let metalness = metal.as_ref().map_or(255, |m| m.get_pixel(x, y)[0]);
                *px = image::Rgba([
                    (ao * 255.0).round() as u8,
                    (rough * 255.0).round() as u8,
                    metalness,
                    255,
                ]);
            }
            Some(out)
        }
    }
}

/// `image` made at most `max` texels on its longer side.
fn fit(image: image::DynamicImage, max: u32) -> image::DynamicImage {
    let (w, h) = (image.width(), image.height());
    if w.max(h) <= max || max == 0 {
        image
    } else {
        image.resize(max, max, image::imageops::FilterType::Triangle)
    }
}

/// sRGB color to linear, alpha kept.
fn linear(c: [f32; 4]) -> [f32; 4] {
    let l = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [l(c[0]), l(c[1]), l(c[2]), c[3]]
}

fn index_of(list: &mut Vec<String>, item: String) -> usize {
    match list.iter().position(|i| *i == item) {
        Some(i) => i,
        None => {
            list.push(item);
            list.len() - 1
        }
    }
}

fn short(guid: &str) -> &str {
    &guid[..guid.len().min(8)]
}

fn first_words(s: &str) -> String {
    s.split(':').next().unwrap_or(s).chars().take(60).collect()
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// The object of the transform `bone` refers to.
fn resolve_bone(scene: &Hierarchy, bone: &Value) -> Option<usize> {
    let reference = bone.obj_ref()?;
    scene.objects.iter().position(|o| {
        o.components
            .iter()
            .any(|c| matches!(c.class_id, 4 | 224) && c.file_id == reference.file_id)
    })
}

/// The skin node of the object whose Animator (or legacy Animation)
/// plays the clips of skinned renderer `renderer`, the nearest one at or
/// above it: Unity resolves a clip's paths from there. `None` when there
/// is none, or the skin has no node for it (reached from a bone by the
/// same names as in the scene).
fn animator_node(skin: &crate::mesh::MeshSkin, scene: &Hierarchy, renderer: usize, bones: &[Value]) -> Option<usize> {
    let objects = &scene.objects;
    let mut at = Some(renderer);
    let animator = loop {
        let i = at?;
        if objects[i].component(ANIMATOR).is_some() || objects[i].component(ANIMATION).is_some() {
            break i;
        }
        at = objects[i].parent;
    };
    for (bone, &joint) in bones.iter().zip(&skin.joints) {
        let Some(mut object) = resolve_bone(scene, bone) else {
            continue;
        };
        let mut node = joint;
        loop {
            if object == animator {
                return Some(node);
            }
            if skin.nodes.get(node)?.name != objects[object].name {
                break;
            }
            match (objects[object].parent, skin.nodes[node].parent) {
                (Some(o), Some(n)) => (object, node) = (o, n),
                _ => break,
            }
        }
    }
    None
}

// Preserve joint order (the vertex indices and bind poses use it), while
// taking the rest pose and names from the flattened, overridden scene.
type SceneSkeleton<'a> = (&'a Hierarchy, &'a [Mat4], usize, &'a [Value], &'a Value);

fn apply_scene_bones(
    skin: &mut crate::mesh::MeshSkin,
    scene: &Hierarchy,
    world: &[Mat4],
    renderer: usize,
    bones: &[Value],
    root: &Value,
) {
    let resolve = |bone: &Value| resolve_bone(scene, bone);
    let root = resolve(root);
    let resolved: Vec<_> = bones.iter().map(resolve).collect();
    let joints: Option<Vec<_>> = resolved.iter().copied().collect();
    let Some(joints) = joints.filter(|j| j.len() == skin.joints.len()) else {
        // Preserve unknown joints and influence indices, applying each known
        // scene bone in the existing hierarchy instead of discarding them all.
        let mut overrides = vec![None; skin.nodes.len()];
        for (&joint, object) in skin.joints.iter().zip(&resolved) {
            if let Some(object) = object {
                overrides[joint] = Some(world[renderer].inverse() * world[*object]);
                skin.nodes[joint].name = scene.objects[*object].name.clone();
                if root == Some(*object) {
                    skin.root = Some(joint);
                }
            }
        }
        fn posed(i: usize, nodes: &[crate::mesh::SkinNode], overrides: &[Option<Mat4>]) -> Mat4 {
            overrides[i].unwrap_or_else(|| {
                nodes[i].parent.map_or(Mat4::IDENTITY, |p| posed(p, nodes, overrides))
                    * Mat4::from_cols_array(&nodes[i].matrix)
            })
        }
        let locals: Vec<_> = skin
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                n.parent
                    .map_or(Mat4::IDENTITY, |p| posed(p, &skin.nodes, &overrides))
                    .inverse()
                    * posed(i, &skin.nodes, &overrides)
            })
            .collect();
        if let Some(root) = skin.root {
            let root_world = posed(root, &skin.nodes, &overrides);
            let roots: Vec<_> = skin
                .nodes
                .iter()
                .enumerate()
                .filter(|(i, n)| *i != root && n.parent.is_none())
                .map(|(i, _)| (i, root_world.inverse() * posed(i, &skin.nodes, &overrides)))
                .collect();
            let mut locals = locals;
            for (i, local) in roots {
                skin.nodes[i].parent = Some(root);
                locals[i] = local;
            }
            for (node, local) in skin.nodes.iter_mut().zip(locals) {
                node.matrix = local.to_cols_array();
            }
            return;
        }
        for (node, local) in skin.nodes.iter_mut().zip(locals) {
            node.matrix = local.to_cols_array();
        }
        return;
    };
    let mut included = std::collections::BTreeSet::new();
    for &joint in joints.iter().chain(root.iter()) {
        let mut at = Some(joint);
        while let Some(i) = at {
            if i == renderer {
                break;
            }
            included.insert(i);
            at = scene.objects[i].parent;
        }
        included.insert(joint);
    }
    let included: Vec<_> = included.into_iter().collect();
    skin.root = root.and_then(|r| included.iter().position(|&i| i == r));
    let old_nodes = skin.nodes.clone();
    skin.nodes = included
        .iter()
        .map(|&i| {
            let parent = scene.objects[i]
                .parent
                .and_then(|p| included.iter().position(|&j| j == p));
            let base = parent.map_or(world[renderer], |p| world[included[p]]);
            crate::mesh::SkinNode {
                name: scene.objects[i].name.clone(),
                parent,
                matrix: (base.inverse() * world[i]).to_cols_array(),
            }
        })
        .collect();
    let source_nodes: Vec<_> = included
        .iter()
        .zip(&skin.nodes)
        .map(|(object, n)| {
            joints
                .iter()
                .position(|j| j == object)
                .map(|j| skin.joints[j])
                .or_else(|| old_nodes.iter().position(|o| o.name == n.name))
        })
        .collect();
    retarget_clips(&mut skin.animations, &old_nodes, &skin.nodes, &source_nodes);
    skin.joints = joints
        .iter()
        .map(|i| included.iter().position(|j| j == i).expect("included joint"))
        .collect();
}

/// Moves each clip's samples from the model file's nodes (`old`) onto the
/// scene's (`nodes`; `sources[k]` is node `k`'s in `old`). A sample is a
/// local transform relative to its node's parent in the model file. Where
/// the scene keeps that parent, the sample already is the node's local
/// transform there, as Unity's animation sets it: the scene's pose of the
/// node (a prefab saved in a posed frame, say) is only its rest pose and is
/// never added to the clip. A node whose parent changed (a root, now
/// relative to the renderer, or a node now under another object) keeps its
/// samples in place by the difference between the two rest poses.
fn retarget_clips(
    animations: &mut [crate::mesh::SkinAnimation],
    old: &[crate::mesh::SkinNode],
    nodes: &[crate::mesh::SkinNode],
    sources: &[Option<usize>],
) {
    for clip in animations {
        let old_step = std::mem::take(&mut clip.step);
        clip.step = sources
            .iter()
            .map(|i| i.and_then(|i| old_step.get(i)).copied().unwrap_or_default())
            .collect();
        let samples = std::mem::take(&mut clip.nodes);
        clip.nodes = sources
            .iter()
            .zip(nodes)
            .map(|(source, node)| {
                let Some(i) = *source else {
                    return Vec::new();
                };
                let same_parent = match (node.parent, old[i].parent) {
                    (Some(p), Some(q)) => sources[p] == Some(q),
                    _ => false,
                };
                let correction = if same_parent {
                    Mat4::IDENTITY
                } else {
                    Mat4::from_cols_array(&node.matrix) * Mat4::from_cols_array(&old[i].matrix).inverse()
                };
                samples
                    .get(i)
                    .map(|samples| {
                        samples
                            .iter()
                            .map(|m| (correction * Mat4::from_cols_array(m)).to_cols_array())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, parent: Option<usize>, matrix: Mat4) -> crate::mesh::SkinNode {
        crate::mesh::SkinNode {
            name: name.into(),
            parent,
            matrix: matrix.to_cols_array(),
        }
    }

    #[test]
    fn a_posed_scene_rest_is_not_added_to_the_clips_of_bones_it_keeps_under_their_parent() {
        // The model file: a root and an arm at its bind pose. The scene was
        // saved posed: the arm turned, the root moved and re-parented.
        let bind = Mat4::from_rotation_x(0.1);
        let posed = Mat4::from_rotation_x(1.2);
        let old = [node("Root", None, Mat4::IDENTITY), node("Arm", Some(0), bind)];
        let root_rest = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
        let nodes = [node("Root", None, root_rest), node("Arm", Some(0), posed)];
        let key = Mat4::from_rotation_x(0.7);
        let mut clips = [crate::mesh::SkinAnimation {
            name: "Wave".into(),
            looping: true,
            times: vec![0.0],
            step: vec![[false; 3]; 2],
            nodes: vec![vec![Mat4::IDENTITY.to_cols_array()], vec![key.to_cols_array()]],
            morphs: vec![],
        }];
        retarget_clips(&mut clips, &old, &nodes, &[Some(0), Some(1)]);
        let sample = |n: usize| Mat4::from_cols_array(&clips[0].nodes[n][0]);
        // The arm plays its key as it is (not posed × bind⁻¹ × key).
        assert!(sample(1).abs_diff_eq(key, 1e-6));
        assert!(!sample(1).abs_diff_eq(posed * bind.inverse() * key, 1e-3));
        // The root, now relative to another frame, keeps its place.
        assert!(sample(0).abs_diff_eq(root_rest, 1e-6));
    }

    #[test]
    fn mirroring_keeps_rotations_rotations() {
        let m = Mat4::from_rotation_translation(Quat::from_rotation_y(0.5), Vec3::new(1.0, 2.0, 3.0));
        let (scale, rotation, position) = mirror(m).to_scale_rotation_translation();
        assert!((scale - Vec3::ONE).length() < 1e-5);
        assert!((position - Vec3::new(1.0, 2.0, -3.0)).length() < 1e-5);
        // A turn around Y the other way.
        assert!(rotation.abs_diff_eq(Quat::from_rotation_y(-0.5), 1e-5));
    }

    #[test]
    fn colors_go_linear() {
        let c = linear([1.0, 0.5, 0.0, 0.25]);
        assert!((c[1] - 0.214).abs() < 1e-3);
        assert_eq!((c[0], c[2], c[3]), (1.0, 0.0, 0.25));
    }

    /// A material of the given shader GUID, everything else default.
    fn material_of(shader: &str) -> UnityMaterial {
        let text = format!(
            "--- !u!21 &2100000\nMaterial:\n  m_Name: Wall\n  m_Shader: {{fileID: 4800000, guid: {shader}, type: 3}}\n"
        );
        UnityMaterial::from_object(&UnityFile::parse(&text).unwrap().objects[0]).unwrap()
    }

    fn has_colors(glb: &[u8]) -> bool {
        let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let doc: Json = serde_json::from_slice(&glb[20..20 + len]).unwrap();
        doc["meshes"][0]["primitives"][0]["attributes"].get("COLOR_0").is_some()
    }

    /// The first primitive's COLOR_0.
    fn colors_of(glb: &[u8]) -> Vec<[f32; 4]> {
        let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let doc: Json = serde_json::from_slice(&glb[20..20 + len]).unwrap();
        let bin = &glb[20 + len + 8..];
        let accessor = &doc["accessors"][doc["meshes"][0]["primitives"][0]["attributes"]["COLOR_0"]
            .as_u64()
            .unwrap() as usize];
        let view = &doc["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
        let start =
            view["byteOffset"].as_u64().unwrap_or(0) as usize + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        let floats: Vec<f32> = bin[start..start + accessor["count"].as_u64().unwrap() as usize * 16]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        floats.as_chunks::<4>().0.to_vec()
    }

    #[test]
    fn vertex_colors_can_be_made_from_other_channels() {
        let dir = std::env::temp_dir().join(format!("ponko_unity_made_colors_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        std::fs::write(dir.join("Assets/River.shader"), "Shader \"River\" {}").unwrap();
        std::fs::write(dir.join("Assets/River.shader.meta"), "guid: river000\n").unwrap();
        let project = Project::open(&dir).unwrap();
        let mut quad = UnityMesh::builtin(crate::builtin::QUAD).unwrap();
        quad.uvs[1] = (0..quad.positions.len()).map(|v| [v as f32, 0.5]).collect();
        let river = [Some(material_of("river000"))];
        let mut import = Importer::new(&project, dir.join("out"), "");
        import.vertex_colors("Assets/River.shader", |mesh, v| {
            let [speed, foam] = mesh.uvs[1][v];
            [speed / 2.0, foam, 0.0, 9.0]
        });
        let colors = colors_of(&import.build_glb(&quad, &river, None));
        assert_eq!(colors.len(), 4);
        let mut speeds: Vec<f32> = colors.iter().map(|c| c[0]).collect();
        speeds.sort_by(f32::total_cmp);
        assert_eq!(speeds, [0.0, 0.5, 1.0, 1.0], "from UV set 1, clamped to 0..1");
        assert!(colors.iter().all(|c| c[1] == 0.5 && c[2] == 0.0 && c[3] == 1.0));
        assert!(import.is_custom(river[0].as_ref().unwrap()), "it's a custom shader too");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn has_uv1(glb: &[u8]) -> bool {
        let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let doc: Json = serde_json::from_slice(&glb[20..20 + len]).unwrap();
        doc["meshes"][0]["primitives"][0]["attributes"]
            .get("TEXCOORD_1")
            .is_some()
    }

    #[test]
    fn uv_set_1_is_kept_only_for_custom_shaders() {
        let dir = std::env::temp_dir().join(format!("ponko_unity_uv1_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        std::fs::write(dir.join("Assets/Leaf.shader"), "Shader \"Leaf\" {}").unwrap();
        std::fs::write(dir.join("Assets/Leaf.shader.meta"), "guid: leaf0000\n").unwrap();
        let project = Project::open(&dir).unwrap();
        let mut quad = UnityMesh::builtin(crate::builtin::QUAD).unwrap();
        quad.uvs[1] = vec![[3.0, 1.0]; quad.positions.len()];
        let leaf = [Some(material_of("leaf0000"))];
        let mut plain = Importer::new(&project, dir.join("out"), "");
        assert!(!has_uv1(&plain.build_glb(&quad, &leaf, None)));
        let mut custom = Importer::new(&project, dir.join("out"), "");
        custom.custom_shader("Assets/Leaf.shader");
        assert!(has_uv1(&custom.build_glb(&quad, &leaf, None)));
        quad.uvs[1].clear();
        assert!(!has_uv1(&custom.build_glb(&quad, &leaf, None)), "none to keep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vertex_colors_are_kept_only_for_custom_shaders() {
        let dir = std::env::temp_dir().join(format!("ponko_unity_colors_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        std::fs::write(dir.join("Assets/Wall.shader"), "Shader \"Wall\" {}").unwrap();
        std::fs::write(dir.join("Assets/Wall.shader.meta"), "guid: wall0000\n").unwrap();
        let project = Project::open(&dir).unwrap();
        let mut cube = UnityMesh::builtin(crate::builtin::CUBE).unwrap();
        cube.colors = vec![[0.5, 0.25, 1.0, 0.75]; cube.positions.len()];
        let wall = [Some(material_of("wall0000"))];
        let mut plain = Importer::new(&project, dir.join("out"), "");
        assert!(
            !has_colors(&plain.build_glb(&cube, &wall, None)),
            "Lit-style shaders ignore them"
        );
        let mut custom = Importer::new(&project, dir.join("out"), "");
        custom.custom_shader("Assets/Wall.shader");
        assert!(has_colors(&custom.build_glb(&cube, &wall, None)));
        cube.colors.clear();
        assert!(!has_colors(&custom.build_glb(&cube, &wall, None)), "none to keep");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
