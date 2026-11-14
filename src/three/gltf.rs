//! Loading glTF 2.0 files (`.glb`, or `.gltf` with its buffers and images
//! next to it) into one [`MeshData`] with a part per material.
//!
//! Every mesh in the file's default scene is placed where its node puts it
//! and merged, so a file is one [`Mesh`](crate::render::Mesh): a tree, a house, a
//! car. A file with skins keeps its nodes as the joints of a skeleton:
//! skinned meshes stay in the space they were bound in, and other meshes
//! follow their node rigidly. Animation clips retain local joint tracks.

use std::ops::Range;

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};

use super::{Blend, Joint, MeshData, MorphTarget, Transform3D};
use crate::color::Color;

/// A loaded file, before it goes to the GPU.
pub(crate) struct Loaded {
    pub data: MeshData,
    /// Index ranges of `data`, each with its material (an index into `materials`).
    pub parts: Vec<(Range<u32>, usize)>,
    pub materials: Vec<LoadedMaterial>,
    /// The materials' images: width, height, RGBA8 pixels.
    pub images: Vec<Image>,
    /// Where each image came from, when that's files (asset names, or
    /// `orm:` plus two of them for a merged image), so files sharing an
    /// image upload it once. One the caller said it has is left empty.
    pub image_sources: Vec<Option<String>>,
    /// With skins: the file's nodes as joints, and what `data.joints`
    /// index (each slot's joint and inverse bind matrix).
    pub skin: Option<LoadedSkin>,
    pub clips: Vec<super::AnimationClip>,
    /// The weights `data.morph_targets` show by default (glTF's mesh
    /// weights), one per target.
    pub morph_defaults: Vec<f32>,
}

pub(crate) struct LoadedSkin {
    pub joints: std::sync::Arc<[Joint]>,
    pub slots: Vec<u16>,
    pub inverse_binds: Vec<Mat4>,
}

/// The joints of a file with skins while its meshes are read: every node
/// of the scene, parents first.
struct Rig {
    joints: Vec<Joint>,
    /// Joint index of each node of the file (`u16::MAX`: not in the scene).
    of_node: Vec<u16>,
    /// Each joint's matrix at rest, in the file's space.
    rest: Vec<Mat4>,
    /// The slots so far: joint and inverse bind matrix.
    slots: Vec<(u16, Mat4)>,
}

impl Rig {
    fn new(gltf: &gltf::Document, scene: &gltf::Scene, name: &str) -> Result<Self, String> {
        let mut rig = Self {
            joints: Vec::new(),
            of_node: vec![u16::MAX; gltf.nodes().len()],
            rest: Vec::new(),
            slots: Vec::new(),
        };
        let mut stack: Vec<(gltf::Node, Option<usize>)> = scene.nodes().map(|n| (n, None)).collect();
        stack.reverse();
        while let Some((node, parent)) = stack.pop() {
            if rig.joints.len() >= usize::from(u16::MAX) {
                return Err(format!("{name}: more than 65535 nodes in a skinned file"));
            }
            let (t, r, s) = node.transform().decomposed();
            let rest = Transform3D {
                position: Vec3::from(t),
                rotation: Quat::from_array(r),
                scale: Vec3::from(s),
            };
            let at = rig.joints.len();
            rig.of_node[node.index()] = at as u16;
            let matrix = Mat4::from_cols_array_2d(&node.transform().matrix());
            rig.rest.push(parent.map_or(matrix, |p| rig.rest[p] * matrix));
            rig.joints.push(Joint {
                name: node
                    .name()
                    .map_or_else(|| format!("node {}", node.index()), str::to_string),
                parent,
                rest,
            });
            let children: Vec<_> = node.children().collect();
            stack.extend(children.into_iter().rev().map(|c| (c, Some(at))));
        }
        Ok(rig)
    }

    /// The slot of `joint` bound with `inverse_bind`, made if it's new.
    fn slot(&mut self, joint: u16, inverse_bind: Mat4) -> Result<u16, String> {
        if let Some(i) = self.slots.iter().position(|s| *s == (joint, inverse_bind)) {
            return Ok(i as u16);
        }
        if self.slots.len() >= usize::from(u16::MAX) {
            return Err("more than 65535 joint bindings".into());
        }
        self.slots.push((joint, inverse_bind));
        Ok(self.slots.len() as u16 - 1)
    }
}

/// How one primitive's corners get their joints.
enum Binding {
    /// No skins in the file.
    None,
    /// Rigidly on one slot.
    Rigid(u16),
    /// Its skin's joints, as slots.
    Skin(Vec<u16>),
}

/// Width, height and RGBA8 pixels.
pub(crate) type Image = (u32, u32, Vec<u8>);

pub(crate) struct LoadedMaterial {
    /// Its name in the file (empty without one).
    pub name: String,
    pub color: Color,
    pub roughness: f32,
    pub metallic: f32,
    pub emissive: Color,
    /// Images, as indices into `Loaded::images`.
    pub image: Option<usize>,
    pub normal_map: Option<usize>,
    pub normal_strength: f32,
    /// Occlusion in red, roughness in green, metalness in blue.
    pub orm_map: Option<usize>,
    pub emissive_map: Option<usize>,
    /// glTF's alpha mode: OPAQUE, MASK (cut out) or BLEND (transparent).
    pub blend: Blend,
    pub double_sided: bool,
}

impl LoadedMaterial {
    fn plain() -> Self {
        Self {
            name: String::new(),
            color: Color::WHITE,
            roughness: 0.6,
            metallic: 0.0,
            emissive: Color::rgb(0.0, 0.0, 0.0),
            image: None,
            normal_map: None,
            normal_strength: 1.0,
            orm_map: None,
            emissive_map: None,
            blend: Blend::Opaque,
            double_sided: false,
        }
    }
}

/// Parses glTF `bytes`; `read` fetches files it refers to, by asset name.
/// Images from files that `have` says are loaded already aren't read (they
/// come back 0×0, with their source).
pub(crate) fn load(
    name: &str,
    bytes: &[u8],
    read: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>,
    have: &dyn Fn(&str) -> bool,
) -> Result<Loaded, String> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| format!("{name}: not a glTF file: {e}"))?;
    let folder = folder_of(name);
    let mut buffers = Vec::new();
    for buffer in gltf.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => gltf
                .blob
                .clone()
                .ok_or_else(|| format!("{name}: missing its binary chunk"))?,
            gltf::buffer::Source::Uri(uri) => fetch(uri, folder, read)?,
        };
        if data.len() < buffer.length() {
            return Err(format!("{name}: buffer {} is shorter than it says", buffer.index()));
        }
        buffers.push(data);
    }

    // Materials, with each image decoded once.
    let mut images = Images {
        decoded: Vec::new(),
        keys: Vec::new(),
        sources: Vec::new(),
        have,
        document: &gltf.document,
        buffers: &buffers,
        folder,
        name,
    };
    let mut materials: Vec<LoadedMaterial> = Vec::new();
    for material in gltf.materials() {
        let pbr = material.pbr_metallic_roughness();
        let [r, g, b, a] = pbr.base_color_factor();
        let [er, eg, eb] = material.emissive_factor();
        let mut get = |t: Option<gltf::Texture>| match t {
            Some(t) => images.get(t.source().index(), read).map(Some),
            None => Ok(None),
        };
        let image = get(pbr.base_color_texture().map(|i| i.texture()))?;
        let emissive_map = get(material.emissive_texture().map(|i| i.texture()))?;
        let normal = material.normal_texture();
        let normal_strength = normal.as_ref().map_or(1.0, |n| n.scale());
        let normal_map = get(normal.map(|n| n.texture()))?;
        // glTF keeps roughness in green and metalness in blue, and
        // occlusion in red of its own image, often the same one.
        let mr = pbr.metallic_roughness_texture().map(|i| i.texture().source().index());
        let occlusion = material.occlusion_texture().map(|i| i.texture().source().index());
        let orm_map = match (mr, occlusion) {
            (None, None) => None,
            (Some(m), Some(o)) if m == o => Some(images.get(m, read)?),
            (m, o) => Some(images.orm(m, o, read)?),
        };
        // glTF factors are linear; Color is sRGB.
        materials.push(LoadedMaterial {
            name: material.name().unwrap_or("").to_string(),
            color: Color::rgba(to_srgb(r), to_srgb(g), to_srgb(b), a),
            roughness: pbr.roughness_factor(),
            metallic: pbr.metallic_factor(),
            emissive: Color::rgb(to_srgb(er), to_srgb(eg), to_srgb(eb)),
            image,
            normal_map,
            normal_strength,
            orm_map,
            emissive_map,
            blend: match material.alpha_mode() {
                gltf::material::AlphaMode::Opaque => Blend::Opaque,
                gltf::material::AlphaMode::Mask => Blend::Cutout,
                gltf::material::AlphaMode::Blend => Blend::Transparent,
            },
            double_sided: material.double_sided(),
        });
    }
    let image_sources = images.sources;
    let images = images.decoded;
    let default_material = materials.len();

    // Every primitive in the scene, placed by its node, grouped by material.
    let scene = gltf
        .default_scene()
        .or_else(|| gltf.scenes().next())
        .ok_or_else(|| format!("{name}: has no scene"))?;
    let mut rig = if gltf.skins().len() > 0 || gltf.animations().len() > 0 {
        Some(Rig::new(&gltf.document, &scene, name)?)
    } else {
        None
    };
    let mut by_material: Vec<(usize, MeshData)> = Vec::new();
    // Morph targets' default weights, by name.
    let mut defaults: Vec<(String, f32)> = Vec::new();
    let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let mut matrix = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        let children = matrix;
        if let Some(mesh) = node.mesh() {
            // A skinned mesh is in the space it was bound in, whatever its
            // node; any other follows its node.
            let binding = match (&mut rig, node.skin()) {
                (None, _) => Binding::None,
                (Some(rig), Some(skin)) => {
                    matrix = Mat4::IDENTITY;
                    let reader = skin.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                    let inverse: Vec<Mat4> = reader
                        .read_inverse_bind_matrices()
                        .map(|m| m.map(|m| Mat4::from_cols_array_2d(&m)).collect())
                        .unwrap_or_default();
                    let mut slots = Vec::new();
                    for (i, joint) in skin.joints().enumerate() {
                        let j = rig.of_node[joint.index()];
                        if j == u16::MAX {
                            return Err(format!("{name}: a skin's joint isn't in the scene"));
                        }
                        let ib = inverse.get(i).copied().unwrap_or(Mat4::IDENTITY);
                        slots.push(rig.slot(j, ib).map_err(|e| format!("{name}: {e}"))?);
                    }
                    Binding::Skin(slots)
                }
                (Some(rig), None) => {
                    let j = rig.of_node[node.index()];
                    let ib = rig.rest[usize::from(j)].inverse();
                    Binding::Rigid(rig.slot(j, ib).map_err(|e| format!("{name}: {e}"))?)
                }
            };
            let targets = target_names(&mesh);
            for (k, target) in targets.iter().enumerate() {
                if !defaults.iter().any(|(n, _)| n == target) {
                    let weight = node
                        .weights()
                        .or_else(|| mesh.weights())
                        .and_then(|w| w.get(k))
                        .copied()
                        .unwrap_or(0.0);
                    defaults.push((target.clone(), weight));
                }
            }
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue; // lines and points aren't drawn
                }
                let material = primitive.material().index().unwrap_or(default_material);
                let at = match by_material.iter().position(|(m, _)| *m == material) {
                    Some(i) => i,
                    None => {
                        by_material.push((material, MeshData::default()));
                        by_material.len() - 1
                    }
                };
                let part = Part {
                    matrix,
                    binding: &binding,
                    targets: &targets,
                };
                append(&mut by_material[at].1, &primitive, &buffers, &part, name)?;
            }
        }
        stack.extend(node.children().map(|c| (c, children)));
    }
    if by_material.is_empty() {
        return Err(format!("{name}: has no triangles"));
    }
    if by_material.iter().any(|(m, _)| *m == default_material) {
        materials.push(LoadedMaterial::plain());
    }

    let mut data = MeshData::default();
    let mut parts = Vec::with_capacity(by_material.len());
    for (material, part) in by_material {
        let base = data.positions.len() as u32;
        let first = data.indices.len() as u32;
        data.positions.extend(part.positions);
        data.normals.extend(part.normals);
        data.uvs.extend(part.uvs);
        data.colors.extend(part.colors);
        data.uvs1.extend(part.uvs1);
        data.joints.extend(part.joints);
        data.weights.extend(part.weights);
        merge_targets(
            &mut data.morph_targets,
            base as usize,
            part.morph_targets,
            data.positions.len(),
        );
        data.indices.extend(part.indices.iter().map(|i| i + base));
        parts.push((first..data.indices.len() as u32, material));
    }
    // Importer-authored in-between frames are ordinary glTF targets plus
    // a channel mapping in mesh extras. Resolve indices after name merging.
    for mesh in gltf.meshes() {
        let names = target_names(&mesh);
        let extras = mesh
            .extras()
            .as_ref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok());
        if let Some(frames) = extras
            .as_ref()
            .and_then(|v| v.get("ponkoMorphFrames"))
            .and_then(|v| v.as_array())
        {
            for f in frames {
                let parse = || -> Option<super::MorphFrame> {
                    let channel = names.get(f["channel"].as_u64()? as usize)?;
                    let target = names.get(f["target"].as_u64()? as usize)?;
                    Some(super::MorphFrame {
                        channel: data.morph_targets.iter().position(|t| t.name == *channel)?,
                        target: data.morph_targets.iter().position(|t| t.name == *target)?,
                        weight: f["weight"].as_f64()? as f32,
                    })
                };
                let frame = parse().ok_or("invalid morph frame extras")?;
                if !data.morph_frames.contains(&frame) {
                    data.morph_frames.push(frame);
                }
            }
        }
    }
    data.problem().map_err(|e| format!("{name}: {e}"))?;
    let morph_defaults = data
        .morph_targets
        .iter()
        .map(|t| defaults.iter().find(|(n, _)| *n == t.name).map_or(0.0, |d| d.1))
        .collect();
    let clips = match &rig {
        Some(rig) => read_clips(&gltf.document, &buffers, rig, &data)?,
        None => Vec::new(),
    };
    let skin = rig.map(|rig| LoadedSkin {
        joints: clips
            .first()
            .map_or_else(|| rig.joints.into(), |c| c.source_joints().clone()),
        slots: rig.slots.iter().map(|s| s.0).collect(),
        inverse_binds: rig.slots.iter().map(|s| s.1).collect(),
    });
    Ok(Loaded {
        data,
        parts,
        materials,
        images,
        image_sources,
        skin,
        clips,
        morph_defaults,
    })
}

/// The names of `mesh`'s morph targets: from its extras' `targetNames`
/// (the usual way exporters name them), else `<mesh>.<i>`.
fn target_names(mesh: &gltf::Mesh) -> Vec<String> {
    let count = mesh.primitives().map(|p| p.morph_targets().len()).max().unwrap_or(0);
    let named: Vec<String> = mesh
        .extras()
        .as_ref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
        .and_then(|v| {
            let names = v.get("targetNames")?.as_array()?;
            names.iter().map(|n| n.as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default();
    let mesh_name = mesh
        .name()
        .map_or_else(|| format!("mesh {}", mesh.index()), str::to_string);
    (0..count)
        .map(|i| named.get(i).cloned().unwrap_or_else(|| format!("{mesh_name}.{i}")))
        .collect()
}

/// Adds the morph targets of corners `before..` of a mesh (`new`, one
/// position offset per such corner, normal offsets optional) to its
/// targets `into` (each with `before` offsets), matched by name; then pads
/// every target with zero offsets to `corners`, so corners without a
/// target don't move.
fn merge_targets(into: &mut Vec<MorphTarget>, before: usize, new: Vec<MorphTarget>, corners: usize) {
    for target in new {
        let at = match into.iter().position(|t| t.name == target.name) {
            Some(at) => at,
            None => {
                into.push(MorphTarget {
                    name: target.name.clone(),
                    positions: vec![Vec3::ZERO; before],
                    normals: vec![Vec3::ZERO; before],
                });
                into.len() - 1
            }
        };
        let t = &mut into[at];
        t.positions.extend(target.positions);
        if target.normals.is_empty() {
            t.normals.resize(t.positions.len(), Vec3::ZERO);
        } else {
            t.normals.extend(target.normals);
        }
    }
    for t in into {
        t.positions.resize(corners, Vec3::ZERO);
        t.normals.resize(corners, Vec3::ZERO);
    }
}

fn read_clips(
    document: &gltf::Document,
    buffers: &[Vec<u8>],
    rig: &Rig,
    data: &MeshData,
) -> Result<Vec<super::AnimationClip>, String> {
    use super::{AnimationClip, AnimationTrack, Interpolation, JointAnimation, Skeleton};
    use gltf::animation::util::ReadOutputs;
    let source = Skeleton::new(rig.joints.clone());
    document
        .animations()
        .map(|animation| {
            let mut tracks: Vec<JointAnimation> = Vec::new();
            let mut morphs = Vec::new();
            let extras = animation
                .extras()
                .as_ref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok());
            let animated = extras
                .as_ref()
                .and_then(|e| e.get("ponkoAnimatedTargets"))
                .and_then(|v| v.as_array());
            for channel in animation.channels() {
                let joint = rig.of_node[channel.target().node().index()];
                if joint == u16::MAX {
                    continue;
                }
                let joint = usize::from(joint);
                let reader = channel.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                let times: Vec<f32> = reader.read_inputs().ok_or("animation has no key times")?.collect();
                let interpolation = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Step => Interpolation::Step,
                    gltf::animation::Interpolation::Linear => Interpolation::Linear,
                    gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
                };
                let (property, values) = match reader.read_outputs().ok_or("animation has no values")? {
                    ReadOutputs::Translations(v) => (0, v.map(|v| Vec3::from(v).extend(0.0)).collect()),
                    ReadOutputs::Rotations(v) => (1, v.into_f32().map(glam::Vec4::from).collect()),
                    ReadOutputs::Scales(v) => (2, v.map(|v| Vec3::from(v).extend(0.0)).collect()),
                    ReadOutputs::MorphTargetWeights(v) => {
                        let mesh = channel.target().node().mesh().ok_or("weight channel has no mesh")?;
                        let names = target_names(&mesh);
                        let values: Vec<f32> = v.into_f32().collect();
                        let multiple = if interpolation == Interpolation::CubicSpline {
                            3
                        } else {
                            1
                        };
                        if names.is_empty() || values.len() != times.len() * multiple * names.len() {
                            return Err("invalid morph animation value count".into());
                        }
                        for (k, name) in names.iter().enumerate() {
                            if animated.is_some_and(|a| !a.iter().any(|v| v.as_str() == Some(name))) {
                                continue;
                            }
                            if !data.morph_targets.iter().any(|t| t.name == *name) {
                                continue;
                            }
                            let track = AnimationTrack::try_new(
                                times.clone(),
                                values
                                    .iter()
                                    .skip(k)
                                    .step_by(names.len())
                                    .map(|v| glam::Vec4::new(*v, 0., 0., 0.))
                                    .collect(),
                                interpolation,
                            )?;
                            if let Some(at) = morphs.iter().position(|t: &super::MorphAnimation| t.name == *name) {
                                morphs.remove(at);
                            }
                            morphs.push(super::MorphAnimation {
                                name: name.clone(),
                                track,
                            });
                        }
                        continue;
                    }
                };
                let track = AnimationTrack::try_new(times, values, interpolation)?;
                let at = tracks.iter().position(|t| t.joint == joint).unwrap_or_else(|| {
                    tracks.push(JointAnimation {
                        joint,
                        name: rig.joints[joint].name.clone(),
                        translation: None,
                        rotation: None,
                        scale: None,
                    });
                    tracks.len() - 1
                });
                match property {
                    0 => tracks[at].translation = Some(track),
                    1 => tracks[at].rotation = Some(track),
                    _ => tracks[at].scale = Some(track),
                }
            }
            Ok(AnimationClip::new(
                animation
                    .name()
                    .map_or_else(|| format!("animation {}", animation.index()), str::to_string),
                &source,
                tracks,
            )
            .with_morph_tracks(morphs))
        })
        .collect()
}

/// A morph target's offsets: none, or one per corner.
fn offsets(v: Option<gltf::accessor::Iter<'_, [f32; 3]>>, corners: usize, name: &str) -> Result<Vec<Vec3>, String> {
    let v: Vec<Vec3> = v.map(|v| v.map(Vec3::from).collect()).unwrap_or_default();
    if !v.is_empty() && v.len() != corners {
        return Err(format!(
            "{name}: a morph target has {} offsets for {corners} corners",
            v.len()
        ));
    }
    Ok(v)
}

/// How one mesh of the file goes into the loaded mesh.
struct Part<'a> {
    /// Its node's matrix (identity when skinned).
    matrix: Mat4,
    binding: &'a Binding,
    /// Its morph targets' names.
    targets: &'a [String],
}

/// Adds one primitive's triangles to `out`, moved by its part's matrix.
fn append(
    out: &mut MeshData,
    primitive: &gltf::Primitive,
    buffers: &[Vec<u8>],
    part_of: &Part,
    name: &str,
) -> Result<(), String> {
    let (matrix, binding) = (part_of.matrix, part_of.binding);
    let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
    let positions: Vec<Vec3> = reader
        .read_positions()
        .ok_or_else(|| format!("{name}: a mesh has no positions"))?
        .map(Vec3::from)
        .collect();
    let count = positions.len() as u32;
    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..count).collect(),
    };
    let part = MeshData {
        positions,
        normals: reader
            .read_normals()
            .map(|n| n.map(Vec3::from).collect())
            .unwrap_or_default(),
        uvs: reader
            .read_tex_coords(0)
            .map(|t| t.into_f32().map(Vec2::from).collect())
            .unwrap_or_default(),
        indices,
        // glTF's colors are linear; Color is sRGB.
        colors: reader
            .read_colors(0)
            .map(|c| {
                c.into_rgba_f32()
                    .map(|[r, g, b, a]| Color::rgba(to_srgb(r), to_srgb(g), to_srgb(b), a))
                    .collect()
            })
            .unwrap_or_default(),
        uvs1: reader
            .read_tex_coords(1)
            .map(|t| t.into_f32().map(Vec2::from).collect())
            .unwrap_or_default(),
        ..Default::default()
    };
    match binding {
        Binding::None => {}
        Binding::Rigid(slot) => {
            out.joints.extend(std::iter::repeat_n([*slot, 0, 0, 0], count as usize));
            out.weights
                .extend(std::iter::repeat_n([1.0, 0.0, 0.0, 0.0], count as usize));
        }
        Binding::Skin(slots) => {
            let joints: Vec<[u16; 4]> = reader
                .read_joints(0)
                .map(|j| j.into_u16().collect())
                .unwrap_or_default();
            let weights: Vec<[f32; 4]> = reader
                .read_weights(0)
                .map(|w| w.into_f32().collect())
                .unwrap_or_default();
            if joints.len() != count as usize || weights.len() != count as usize {
                return Err(format!(
                    "{name}: a skinned mesh needs JOINTS_0 and WEIGHTS_0 on every corner"
                ));
            }
            let slot = |j: u16| {
                slots
                    .get(usize::from(j))
                    .copied()
                    .ok_or_else(|| format!("{name}: a corner names joint {j}, past its skin's joints"))
            };
            for (j, w) in joints.iter().zip(&weights) {
                let mut mapped = [0u16; 4];
                for k in 0..4 {
                    // Unused influences may name any joint.
                    mapped[k] = if w[k] > 0.0 { slot(j[k])? } else { 0 };
                }
                out.joints.push(mapped);
                out.weights.push(super::skin::normalized(*w));
            }
        }
    }
    part.problem().map_err(|e| format!("{name}: {e}"))?;
    let normals = part.normals_or_computed().into_owned();
    // A mirroring node flips triangles inside out: swap two corners back.
    let flips = matrix.determinant() < 0.0;
    let normal_matrix = Mat3::from_mat4(matrix).inverse().transpose();
    let base = out.positions.len() as u32;
    out.positions
        .extend(part.positions.iter().map(|p| matrix.transform_point3(*p)));
    out.normals
        .extend(normals.iter().map(|n| (normal_matrix * *n).normalize_or(Vec3::Y)));
    // Morph targets: offsets turn with the node (normals' by the normal
    // matrix, unnormalized: they're differences).
    let mut targets = Vec::new();
    for (k, (positions, normals, _)) in reader.read_morph_targets().enumerate() {
        let corners = count as usize;
        let (positions, normals) = (offsets(positions, corners, name)?, offsets(normals, corners, name)?);
        targets.push(MorphTarget {
            name: part_of.targets.get(k).cloned().unwrap_or_else(|| format!("{name}.{k}")),
            positions: if positions.is_empty() {
                vec![Vec3::ZERO; corners]
            } else {
                positions.iter().map(|d| matrix.transform_vector3(*d)).collect()
            },
            normals: normals.iter().map(|d| normal_matrix * *d).collect(),
        });
    }
    merge_targets(&mut out.morph_targets, base as usize, targets, out.positions.len());
    if part.uvs.is_empty() {
        out.uvs.extend(std::iter::repeat_n(Vec2::ZERO, count as usize));
    } else {
        out.uvs.extend(part.uvs);
    }
    if part.uvs1.is_empty() {
        out.uvs1.extend(std::iter::repeat_n(Vec2::ZERO, count as usize));
    } else {
        out.uvs1.extend(part.uvs1);
    }
    if part.colors.is_empty() {
        out.colors.extend(std::iter::repeat_n(Color::WHITE, count as usize));
    } else {
        out.colors.extend(part.colors);
    }
    for tri in part.indices.as_chunks::<3>().0 {
        let [a, b, c] = [tri[0] + base, tri[1] + base, tri[2] + base];
        out.indices.extend(if flips { [a, c, b] } else { [a, b, c] });
    }
    Ok(())
}

/// The file's images, decoded on first use.
struct Images<'a> {
    decoded: Vec<Image>,
    /// What each of `decoded` is, so each is decoded or merged once.
    keys: Vec<Key>,
    /// Which files each of `decoded` came from (see `Loaded::image_sources`).
    sources: Vec<Option<String>>,
    have: &'a dyn Fn(&str) -> bool,
    document: &'a gltf::Document,
    buffers: &'a [Vec<u8>],
    folder: &'a str,
    name: &'a str,
}

#[derive(PartialEq)]
enum Key {
    /// An image of the file.
    Image(usize),
    /// An ORM image made from glTF's metallic-roughness and occlusion images.
    Orm(Option<usize>, Option<usize>),
}

type Read<'r> = &'r mut dyn FnMut(&str) -> Result<Vec<u8>, String>;

impl Images<'_> {
    /// Image `index`, decoded: its index in `decoded`.
    fn get(&mut self, index: usize, read: Read) -> Result<usize, String> {
        if let Some(at) = self.keys.iter().position(|k| *k == Key::Image(index)) {
            return Ok(at);
        }
        let source = self.source(index);
        let image = match &source {
            Some(s) if (self.have)(s) => (0, 0, Vec::new()),
            _ => self.decode(index, read)?,
        };
        Ok(self.add(Key::Image(index), image, source))
    }

    /// The asset name of image `index`, if it's a file of its own.
    fn source(&self, index: usize) -> Option<String> {
        match self.document.images().nth(index)?.source() {
            gltf::image::Source::Uri { uri, .. } if !uri.starts_with("data:") => {
                Some(format!("{}{}", self.folder, percent_decode(uri)))
            }
            _ => None,
        }
    }

    /// An ORM image from glTF's metallic-roughness image (roughness in
    /// green, metalness in blue) and occlusion image (red), either of
    /// which can be missing: what's missing is 1.
    fn orm(&mut self, metal_rough: Option<usize>, occlusion: Option<usize>, read: Read) -> Result<usize, String> {
        let key = Key::Orm(metal_rough, occlusion);
        if let Some(at) = self.keys.iter().position(|k| *k == key) {
            return Ok(at);
        }
        let file = |i: Option<usize>| i.map_or(Some(String::new()), |i| self.source(i));
        let source = file(metal_rough)
            .zip(file(occlusion))
            .map(|(m, o)| format!("orm:{m}|{o}"));
        if let Some(s) = &source
            && (self.have)(s)
        {
            return Ok(self.add(key, (0, 0, Vec::new()), source));
        }
        let occlusion = occlusion.map(|o| self.decode(o, read)).transpose()?;
        let mut orm = match metal_rough {
            Some(m) => self.decode(m, read)?,
            None => {
                let (w, h, _) = occlusion.as_ref().expect("one of the two is there");
                (*w, *h, vec![255; (*w * *h * 4) as usize])
            }
        };
        let pixels = orm.2.as_chunks_mut::<4>().0;
        match occlusion {
            // Red from the occlusion image, when it lines up texel for texel.
            Some((w, h, ao)) if (w, h) == (orm.0, orm.1) => {
                for (px, o) in pixels.iter_mut().zip(ao.as_chunks::<4>().0) {
                    px[0] = o[0];
                }
            }
            _ => pixels.iter_mut().for_each(|px| px[0] = 255),
        }
        Ok(self.add(key, orm, source))
    }

    fn add(&mut self, key: Key, image: Image, source: Option<String>) -> usize {
        self.keys.push(key);
        self.sources.push(source);
        self.decoded.push(image);
        self.decoded.len() - 1
    }

    /// Decodes image `index` of the file to RGBA8.
    fn decode(&self, index: usize, read: Read) -> Result<Image, String> {
        let image = self
            .document
            .images()
            .nth(index)
            .ok_or_else(|| format!("{}: image {index} isn't in the file", self.name))?;
        decode(&image, self.buffers, self.folder, read, self.name)
    }
}

/// Decodes an image of the file to RGBA8.
fn decode(image: &gltf::Image, buffers: &[Vec<u8>], folder: &str, read: Read, name: &str) -> Result<Image, String> {
    let bytes = match image.source() {
        gltf::image::Source::View { view, .. } => {
            let buffer = &buffers[view.buffer().index()];
            buffer
                .get(view.offset()..view.offset() + view.length())
                .ok_or_else(|| format!("{name}: an image is past the end of its buffer"))?
                .to_vec()
        }
        gltf::image::Source::Uri { uri, .. } => fetch(uri, folder, read)?,
    };
    let decoded = image::load_from_memory(&bytes).map_err(|e| format!("{name}: an image can't be read: {e}"))?;
    let rgba = decoded.to_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// The bytes a URI names: inline base64 data, or a file next to the glTF.
fn fetch(uri: &str, folder: &str, read: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest
            .split_once(";base64,")
            .ok_or("only base64 data URIs are supported")?;
        return base64(data);
    }
    let file = percent_decode(uri);
    read(&format!("{folder}{file}"))
}

/// The folder part of asset name `name`, with its trailing separator: what
/// relative URIs in it are joined to. `\\` counts too, for a file loaded by an
/// absolute Windows path.
fn folder_of(name: &str) -> &str {
    name.rfind(['/', '\\']).map_or("", |i| &name[..=i])
}

/// `%20` and friends in file names.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64(text: &str) -> Result<Vec<u8>, String> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = value(c).ok_or("bad base64 in a data URI")?;
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

fn to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_percent_decoding() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("AAEC").unwrap(), [0, 1, 2]);
        assert!(base64("a$").is_err());
        assert_eq!(percent_decode("my%20tree.bin"), "my tree.bin");
    }

    #[test]
    fn buffers_are_looked_up_next_to_the_file_with_either_separator() {
        assert_eq!(folder_of("models/tree.gltf"), "models/");
        assert_eq!(folder_of(r"C:\Temp\x\tree.gltf"), r"C:\Temp\x\");
        assert_eq!(folder_of(r"D:\a\crate/../tree.gltf"), r"D:\a\crate/../");
        assert_eq!(folder_of("tree.gltf"), "");
    }

    fn to_base64(bytes: &[u8]) -> String {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut b64 = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                b64.push(alphabet[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        while !b64.len().is_multiple_of(4) {
            b64.push('=');
        }
        b64
    }

    /// A PNG of two `pixel`s side by side, as a data URI.
    fn png(pixel: [u8; 4]) -> String {
        let image = image::RgbaImage::from_fn(2, 1, |_, _| image::Rgba(pixel));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        format!("data:image/png;base64,{}", to_base64(&bytes.into_inner()))
    }

    #[test]
    fn masked_materials_load_as_cut_outs() {
        let gltf = triangle_gltf(None).replace(
            r#""materials": [{ "pbrMetallicRoughness": { "baseColorFactor": [1, 0, 0, 1], "roughnessFactor": 0.25 } }],"#,
            r#""materials": [{ "alphaMode": "MASK" }, {}],"#,
        );
        let loaded = load("mask.gltf", gltf.as_bytes(), &mut |f| Err(f.to_string()), &|_| false).unwrap();
        assert_eq!(loaded.materials[0].blend, Blend::Cutout);
        assert_eq!(loaded.materials[1].blend, Blend::Opaque);
    }

    #[test]
    fn material_maps_load_with_occlusion_packed_into_red() {
        let gltf = triangle_gltf(None);
        let materials = format!(
            r#""materials": [
                {{ "pbrMetallicRoughness": {{ "metallicRoughnessTexture": {{ "index": 0 }} }},
                   "occlusionTexture": {{ "index": 1 }},
                   "normalTexture": {{ "index": 2, "scale": 0.5 }},
                   "emissiveTexture": {{ "index": 2 }},
                   "alphaMode": "BLEND", "doubleSided": true }},
                {{ "occlusionTexture": {{ "index": 1 }} }}
            ],
            "textures": [{{ "source": 0 }}, {{ "source": 1 }}, {{ "source": 2 }}],
            "images": [{{ "uri": "{}" }}, {{ "uri": "{}" }}, {{ "uri": "{}" }}],"#,
            png([9, 100, 200, 255]),
            png([50, 50, 50, 255]),
            png([128, 128, 255, 255]),
        );
        let gltf = gltf.replace(
            r#""materials": [{ "pbrMetallicRoughness": { "baseColorFactor": [1, 0, 0, 1], "roughnessFactor": 0.25 } }],"#,
            &materials,
        );
        let loaded = load("maps.gltf", gltf.as_bytes(), &mut |f| Err(f.to_string()), &|_| false).unwrap();
        let m = &loaded.materials[0];
        assert!(m.blend == Blend::Transparent && m.double_sided);
        assert_eq!(m.normal_strength, 0.5);
        assert_eq!(m.normal_map, m.emissive_map, "one image, decoded once");
        let orm = &loaded.images[m.orm_map.unwrap()];
        assert_eq!(
            &orm.2[..4],
            &[50, 100, 200, 255],
            "occlusion in red, roughness and metalness as they were"
        );
        let only_occlusion = &loaded.images[loaded.materials[1].orm_map.unwrap()];
        assert_eq!(
            &only_occlusion.2[..4],
            &[50, 255, 255, 255],
            "no roughness or metalness map: 1"
        );
        assert_eq!(loaded.images.len(), 3, "normal, ORM and occlusion-only ORM");
    }

    #[test]
    fn image_files_say_where_they_came_from_and_known_ones_are_not_read() {
        let gltf = triangle_gltf(None).replace(
            r#""materials": [{ "pbrMetallicRoughness": { "baseColorFactor": [1, 0, 0, 1], "roughnessFactor": 0.25 } }],"#,
            r#""materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } },
                              "normalTexture": { "index": 1 } }],
            "textures": [{ "source": 0 }, { "source": 1 }],
            "images": [{ "uri": "textures/bark%20a.png" }, { "uri": "textures/bark_n.png" }],"#,
        );
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 4]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut reads = Vec::new();
        let loaded = load(
            "models/oak.gltf",
            gltf.as_bytes(),
            &mut |f| {
                reads.push(f.to_string());
                Ok(png.get_ref().clone())
            },
            &|source| source == "models/textures/bark a.png",
        )
        .unwrap();
        assert_eq!(reads, ["models/textures/bark_n.png"], "the known one isn't read");
        let m = &loaded.materials[0];
        assert_eq!(
            loaded.image_sources[m.image.unwrap()].as_deref(),
            Some("models/textures/bark a.png")
        );
        assert_eq!(loaded.images[m.image.unwrap()].0, 0, "left empty");
        assert_eq!(loaded.images[m.normal_map.unwrap()].2, [1, 2, 3, 4]);
    }

    #[test]
    fn inline_images_have_no_source() {
        let loaded = load(
            "t.gltf",
            triangle_gltf(None).as_bytes(),
            &mut |f| Err(f.to_string()),
            &|_| true,
        )
        .unwrap();
        assert!(loaded.image_sources.is_empty());
    }

    /// A one-triangle .gltf under a node that moves it up by 2 and mirrors
    /// x; its buffer inline, or in `file` next to it.
    fn triangle_gltf(file: Option<&str>) -> String {
        let mut bytes = Vec::new();
        for v in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in v {
                bytes.extend(c.to_le_bytes());
            }
        }
        let b64 = to_base64(&bytes);
        let uri = file.map_or(format!("data:application/octet-stream;base64,{b64}"), str::to_string);
        format!(
            r#"{{
            "asset": {{ "version": "2.0" }},
            "scene": 0,
            "scenes": [{{ "nodes": [0] }}],
            "nodes": [{{ "mesh": 0, "translation": [0, 2, 0], "scale": [-1, 1, 1] }}],
            "meshes": [{{ "primitives": [{{ "attributes": {{ "POSITION": 0 }}, "material": 0 }}] }}],
            "materials": [{{ "pbrMetallicRoughness": {{ "baseColorFactor": [1, 0, 0, 1], "roughnessFactor": 0.25 }} }}],
            "buffers": [{{ "byteLength": 36, "uri": "{uri}" }}],
            "bufferViews": [{{ "buffer": 0, "byteLength": 36 }}],
            "accessors": [{{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                            "min": [0, 0, 0], "max": [1, 1, 0] }}]
        }}"#
        )
    }

    #[test]
    fn a_gltf_file_loads_placed_by_its_node_with_its_material() {
        let loaded = load(
            "tri.gltf",
            triangle_gltf(None).as_bytes(),
            &mut |f| Err(f.to_string()),
            &|_| false,
        )
        .unwrap();
        let data = &loaded.data;
        assert_eq!(
            data.positions,
            vec![
                Vec3::new(0.0, 2.0, 0.0),
                Vec3::new(-1.0, 2.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0)
            ]
        );
        // Mirrored: the winding is swapped so the computed normal (+Z) faces front.
        assert_eq!(data.indices, vec![0, 2, 1]);
        let [a, b, c] = [0, 2, 1].map(|i| data.positions[i]);
        assert!((b - a).cross(c - a).dot(data.normals[0]) > 0.0);
        assert_eq!(loaded.parts, vec![(0..3, 0)]);
        let m = &loaded.materials[0];
        assert!((m.color.r - 1.0).abs() < 1e-5 && m.color.g == 0.0);
        assert_eq!((m.roughness, m.metallic), (0.25, 1.0), "metallic defaults to 1 in glTF");
    }

    /// Two meshes on two nodes: "face" has targets "smile" and "blink"
    /// (named in its extras, with default weights), "lid" (scaled 2x, its
    /// own material) only "blink".
    fn morph_gltf() -> String {
        let mut bytes = Vec::new();
        let floats = |bytes: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|f| bytes.extend(f.to_le_bytes()));
        floats(&mut bytes, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // corners
        floats(&mut bytes, &[0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]); // all up 1
        floats(&mut bytes, &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]); // corner 0 +x
        let accessor = |at: usize| {
            format!(
                r#"{{ "bufferView": 0, "byteOffset": {}, "componentType": 5126, "count": 3, "type": "VEC3",
                      "min": [0, 0, 0], "max": [1, 1, 0] }}"#,
                at * 36
            )
        };
        format!(
            r#"{{
            "asset": {{ "version": "2.0" }},
            "scene": 0,
            "scenes": [{{ "nodes": [0, 1] }}],
            "nodes": [{{ "mesh": 0 }}, {{ "mesh": 1, "scale": [2, 2, 2] }}],
            "meshes": [
                {{ "name": "face", "weights": [0.5, 0.0], "extras": {{ "targetNames": ["smile", "blink"] }},
                   "primitives": [{{ "attributes": {{ "POSITION": 0 }}, "material": 0,
                                    "targets": [{{ "POSITION": 1 }}, {{ "POSITION": 2 }}] }}] }},
                {{ "name": "lid", "extras": {{ "targetNames": ["blink"] }},
                   "primitives": [{{ "attributes": {{ "POSITION": 0 }}, "material": 1,
                                    "targets": [{{ "POSITION": 2 }}] }}] }}
            ],
            "materials": [{{}}, {{}}],
            "buffers": [{{ "byteLength": 108, "uri": "data:application/octet-stream;base64,{}" }}],
            "bufferViews": [{{ "buffer": 0, "byteLength": 108 }}],
            "accessors": [{}, {}, {}]
        }}"#,
            to_base64(&bytes),
            accessor(0),
            accessor(1),
            accessor(2),
        )
    }

    #[test]
    fn morph_only_gltf_channels_sample_all_interpolations_and_frame_metadata() {
        use crate::anim::{MorphWeights, Skeleton};
        for (frames_key, targets_key) in [("ponkoMorphFrames", "ponkoAnimatedTargets")] {
            for (mode, values, expected) in [
                ("LINEAR", vec![0., 0.2, 1., 0.6], [0.5, 0.4]),
                ("STEP", vec![0., 0.2, 1., 0.6], [0., 0.2]),
                // key-major in/value/out triples, with each tuple's target values interleaved.
                (
                    "CUBICSPLINE",
                    vec![0., 0., 0., 0.2, 4., 0., 0., 0., 0., 0.6, 0., 0.],
                    [1., 0.4],
                ),
            ] {
                let mut doc: serde_json::Value = serde_json::from_str(&morph_gltf()).unwrap();
                let mut bytes = Vec::new();
                for v in [0f32, 2.].into_iter().chain(values) {
                    bytes.extend(v.to_le_bytes());
                }
                doc["buffers"].as_array_mut().unwrap().push(serde_json::json!({"byteLength":bytes.len(),"uri":format!("data:application/octet-stream;base64,{}",to_base64(&bytes))}));
                doc["bufferViews"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"buffer":1,"byteLength":bytes.len()}));
                doc["accessors"].as_array_mut().unwrap().extend([
                serde_json::json!({"bufferView":1,"componentType":5126,"count":2,"type":"SCALAR","min":[0.],"max":[2.]}),
                serde_json::json!({"bufferView":1,"byteOffset":8,"componentType":5126,"count":(bytes.len()-8)/4,"type":"SCALAR"})]);
                doc["animations"] = serde_json::json!([{"name":"face","samplers":[{"input":3,"output":4,"interpolation":mode}],"channels":[{"sampler":0,"target":{"node":0,"path":"weights"}}]}]);
                doc["meshes"][0]["extras"][frames_key] = serde_json::json!([
                {"channel":0,"target":1,"weight":0.5},{"channel":0,"target":0,"weight":1.}]);
                doc["animations"][0]["extras"][targets_key] = serde_json::json!(["smile"]);
                let loaded = load("face.gltf", doc.to_string().as_bytes(), &mut |f| Err(f.into()), &|_| {
                    false
                })
                .unwrap();
                assert_eq!(loaded.data.morph_frames.len(), 2);
                let mut skeleton = Skeleton::from_shared(loaded.skin.unwrap().joints);
                loaded.clips[0].sample(1., &mut skeleton);
                let mut weights = MorphWeights(vec![0., 0.]);
                let names: Vec<_> = loaded.data.morph_targets.iter().map(|t| t.name.clone()).collect();
                skeleton.write_morph_weights(&names, &mut weights);
                for (name, value) in [("smile", expected[0]), ("blink", 0.)] {
                    assert!(
                        (weights.get(names.iter().position(|n| n == name).unwrap()) - value).abs() < 1e-6,
                        "{mode} {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn morph_targets_merge_by_name_and_follow_their_node() {
        let loaded = load(
            "face.gltf",
            morph_gltf().as_bytes(),
            &mut |f| Err(f.to_string()),
            &|_| false,
        )
        .unwrap();
        let data = &loaded.data;
        assert!(data.check().is_ok());
        assert_eq!(data.positions.len(), 6);
        let names: Vec<&str> = data.morph_targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names.len(), 2, "one \"blink\" for both meshes: {names:?}");
        let target = |name: &str| data.morph_targets.iter().position(|t| t.name == name).unwrap();
        let (smile, blink) = (target("smile"), target("blink"));
        assert_eq!(loaded.morph_defaults[smile], 0.5);
        assert_eq!(loaded.morph_defaults[blink], 0.0);
        // The face's corners: those its material's part draws.
        let (range, _) = loaded.parts.iter().find(|(_, m)| *m == 0).unwrap();
        let face: Vec<usize> = data.indices[range.start as usize..range.end as usize]
            .iter()
            .map(|&i| i as usize)
            .collect();
        for &i in &face {
            assert_eq!(
                data.morph_targets[smile].positions[i],
                Vec3::Y,
                "the face's smile lifts it"
            );
        }
        // Corner 0 of each mesh (at the origin) moves +x in "blink": by 2
        // on the scaled lid.
        let blinks: Vec<Vec3> = data.morph_targets[blink]
            .positions
            .iter()
            .copied()
            .filter(|d| *d != Vec3::ZERO)
            .collect();
        assert_eq!(blinks.len(), 2);
        assert!(blinks.contains(&Vec3::X) && blinks.contains(&(2.0 * Vec3::X)));
        // The lid has no smile.
        let smiling = data.morph_targets[smile]
            .positions
            .iter()
            .filter(|d| **d != Vec3::ZERO)
            .count();
        assert_eq!(smiling, 3);
        assert_eq!(data.morph_targets[smile].normals.len(), 6, "normals padded with zeros");
    }

    #[test]
    fn unnamed_morph_targets_are_named_after_their_mesh() {
        let gltf = morph_gltf().replace(r#""extras": { "targetNames": ["smile", "blink"] },"#, "");
        let loaded = load("face.gltf", gltf.as_bytes(), &mut |f| Err(f.to_string()), &|_| false).unwrap();
        let mut names: Vec<&str> = loaded.data.morph_targets.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["blink", "face.0", "face.1"]);
    }

    #[test]
    fn a_second_uv_set_loads_for_shaders() {
        let mut bytes = Vec::new();
        for c in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bytes.extend(c.to_le_bytes());
        }
        for c in [0.5f32, 7.0, 1.5, 7.0, 2.5, 7.0] {
            bytes.extend(c.to_le_bytes());
        }
        let b64 = to_base64(&bytes);
        let text = format!(
            r#"{{
            "asset": {{ "version": "2.0" }},
            "meshes": [{{ "primitives": [{{ "attributes": {{ "POSITION": 0, "TEXCOORD_1": 1 }} }}] }}],
            "nodes": [{{ "mesh": 0 }}], "scenes": [{{ "nodes": [0] }}], "scene": 0,
            "buffers": [{{ "byteLength": 60, "uri": "data:application/octet-stream;base64,{b64}" }}],
            "bufferViews": [{{ "buffer": 0, "byteLength": 36 }}, {{ "buffer": 0, "byteOffset": 36, "byteLength": 24 }}],
            "accessors": [{{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0] }},
                          {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2" }}]
        }}"#
        );
        let loaded = load("uv1.gltf", text.as_bytes(), &mut |f| Err(f.to_string()), &|_| false).unwrap();
        assert_eq!(
            loaded.data.uvs1,
            vec![Vec2::new(0.5, 7.0), Vec2::new(1.5, 7.0), Vec2::new(2.5, 7.0)]
        );
        assert!(
            loaded.data.uvs.iter().all(|uv| *uv == Vec2::ZERO),
            "no first set: zeros"
        );
        let plain = load(
            "tri.gltf",
            triangle_gltf(None).as_bytes(),
            &mut |f| Err(f.to_string()),
            &|_| false,
        )
        .unwrap();
        assert_eq!(plain.data.uvs1, vec![Vec2::ZERO; 3], "none: zeros");
    }

    /// A skinned .gltf: an "Armature" node over two joints ("root", and
    /// "tip" one up), a triangle skinned to them, and a plain triangle
    /// under "tip".
    fn skinned_gltf() -> String {
        let mut bytes = Vec::new();
        let floats = |bytes: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|c| bytes.extend(c.to_le_bytes()));
        // 0: positions (36 bytes)
        floats(&mut bytes, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        // 36: joints, u8 x 4 per corner (12 bytes)
        bytes.extend([0u8, 1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0]);
        // 48: weights (48 bytes): unnormalized on the first corner
        floats(
            &mut bytes,
            &[1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        );
        // 96: inverse binds (128 bytes): root at the origin, tip at y 1
        let identity = Mat4::IDENTITY.to_cols_array();
        let tip = Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0)).to_cols_array();
        floats(&mut bytes, &identity);
        floats(&mut bytes, &tip);
        let b64 = to_base64(&bytes);
        format!(
            r#"{{
            "asset": {{ "version": "2.0" }},
            "scene": 0,
            "scenes": [{{ "nodes": [0, 3] }}],
            "nodes": [
                {{ "name": "Armature", "children": [1], "scale": [2, 2, 2] }},
                {{ "name": "root", "children": [2] }},
                {{ "name": "tip", "translation": [0, 1, 0], "children": [4] }},
                {{ "name": "body", "mesh": 0, "skin": 0, "translation": [50, 0, 0] }},
                {{ "name": "cup", "mesh": 1 }}
            ],
            "skins": [{{ "joints": [1, 2], "inverseBindMatrices": 3 }}],
            "meshes": [
                {{ "primitives": [{{ "attributes": {{ "POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2 }} }}] }},
                {{ "primitives": [{{ "attributes": {{ "POSITION": 0 }} }}] }}
            ],
            "buffers": [{{ "byteLength": 224, "uri": "data:application/octet-stream;base64,{b64}" }}],
            "bufferViews": [
                {{ "buffer": 0, "byteLength": 36 }},
                {{ "buffer": 0, "byteOffset": 36, "byteLength": 12 }},
                {{ "buffer": 0, "byteOffset": 48, "byteLength": 48 }},
                {{ "buffer": 0, "byteOffset": 96, "byteLength": 128 }}
            ],
            "accessors": [
                {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0] }},
                {{ "bufferView": 1, "componentType": 5121, "count": 3, "type": "VEC4" }},
                {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4" }},
                {{ "bufferView": 3, "componentType": 5126, "count": 2, "type": "MAT4" }}
            ]
        }}"#
        )
    }

    #[test]
    fn animations_map_node_indices_to_preorder_joints() {
        // Separate inline buffer for two times and two translation values.
        let bytes: Vec<u8> = [0.0f32, 2.0, 0.0, 1.0, 0.0, 0.0, 3.0, 0.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut text = skinned_gltf();
        text = text.replace("\"buffers\": [", &format!(r#""animations": [{{ "name": "rise", "samplers": [{{ "input": 4, "output": 5, "interpolation": "LINEAR" }}], "channels": [{{ "sampler": 0, "target": {{ "node": 4, "path": "translation" }} }}] }}],
            "buffers": [{{ "byteLength": 32, "uri": "data:application/octet-stream;base64,{}" }},"#, to_base64(&bytes)));
        // Existing views refer to the old buffer, now index 1.
        text = text.replace("\"buffer\": 0", "\"buffer\": 1");
        // Append views and accessors so existing mesh indices stay unchanged.
        text = text.replace(r#"{ "buffer": 1, "byteOffset": 96, "byteLength": 128 }"#, r#"{ "buffer": 1, "byteOffset": 96, "byteLength": 128 }, { "buffer": 0, "byteLength": 8 }, { "buffer": 0, "byteOffset": 8, "byteLength": 24 }"#);
        text = text.replace(r#"{ "bufferView": 3, "componentType": 5126, "count": 2, "type": "MAT4" }"#, r#"{ "bufferView": 3, "componentType": 5126, "count": 2, "type": "MAT4" }, { "bufferView": 4, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0], "max": [2] }, { "bufferView": 5, "componentType": 5126, "count": 2, "type": "VEC3" }"#);
        let loaded = load("animated.gltf", text.as_bytes(), &mut |f| Err(f.into()), &|_| false).unwrap();
        let clip = &loaded.clips[0];
        assert_eq!(clip.name(), "rise");
        assert_eq!(clip.duration(), 2.0);
        assert_eq!((clip.tracks()[0].joint, clip.tracks()[0].name.as_str()), (3, "cup"));
        assert_eq!(clip.tracks()[0].translation.as_ref().unwrap().times(), &[0.0, 2.0]);
        let mut skeleton = super::super::Skeleton::from_shared(loaded.skin.unwrap().joints);
        assert!(skeleton.shares_joints(clip.source_joints()));
        clip.sample(1.0, &mut skeleton);
        assert_eq!(skeleton.pose[3].position, Vec3::Y * 2.0);
        assert_eq!(skeleton.pose[0].scale, Vec3::splat(2.0));
    }

    #[test]
    fn skinned_files_keep_their_nodes_as_joints() {
        let loaded = load(
            "skin.gltf",
            skinned_gltf().as_bytes(),
            &mut |f| Err(f.to_string()),
            &|_| false,
        )
        .unwrap();
        let skin = loaded.skin.expect("a skinned file has a skeleton");
        let names: Vec<&str> = skin.joints.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(
            names,
            ["Armature", "root", "tip", "cup", "body"],
            "every node, parents first"
        );
        let parents: Vec<Option<usize>> = skin.joints.iter().map(|j| j.parent).collect();
        assert_eq!(parents, [None, Some(0), Some(1), Some(2), None]);
        assert_eq!(skin.joints[2].rest.position, Vec3::Y);
        // Slots: root and tip from the skin, then cup's own node for the
        // plain triangle, bound where cup sits at rest (scaled by 2, up 2).
        assert_eq!(skin.slots, [1, 2, 3]);
        assert_eq!(skin.inverse_binds[1], Mat4::from_translation(-Vec3::Y));
        let cup_rest = Mat4::from_scale(Vec3::splat(2.0)) * Mat4::from_translation(Vec3::Y);
        assert!(skin.inverse_binds[2].abs_diff_eq(cup_rest.inverse(), 1e-6));
        let data = &loaded.data;
        assert_eq!(data.joints.len(), 6);
        // The skinned triangle stays where it was bound (its node's move
        // is ignored, as glTF says); weights add up to 1.
        assert_eq!(data.positions[..3], [Vec3::ZERO, Vec3::X, Vec3::Y]);
        assert_eq!(data.joints[0], [0, 1, 0, 0]);
        assert_eq!(data.weights[0], [0.5, 0.5, 0.0, 0.0]);
        assert_eq!(data.joints[2], [1, 0, 0, 0]);
        // The plain one is placed by its node, all on its own slot.
        assert!(data.positions[4].abs_diff_eq(Vec3::new(2.0, 2.0, 0.0), 1e-6));
        assert_eq!(data.joints[3..], [[2, 0, 0, 0]; 3]);
        assert_eq!(data.weights[3..], [[1.0, 0.0, 0.0, 0.0]; 3]);
    }

    #[test]
    fn files_without_skins_have_no_joints() {
        let loaded = load(
            "tri.gltf",
            triangle_gltf(None).as_bytes(),
            &mut |f| Err(f.to_string()),
            &|_| false,
        )
        .unwrap();
        assert!(loaded.skin.is_none() && loaded.data.joints.is_empty());
    }

    #[test]
    fn missing_files_and_junk_are_errors() {
        assert!(load("junk.glb", b"not a gltf", &mut |_| Ok(vec![]), &|_| false).is_err());
        let external = triangle_gltf(Some("my%20tri.bin"));
        let err = load(
            "models/tri.gltf",
            external.as_bytes(),
            &mut |f| Err(format!("no {f}")),
            &|_| false,
        )
        .err()
        .unwrap();
        assert_eq!(err, "no models/my tri.bin");
    }
}
