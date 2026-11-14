//! Meshes inside model files (FBX, OBJ), read with ufbx the way Unity's
//! model importer sees them: X mirrored into Unity's left-handed space
//! (or, with `bakeAxisConversion`, the file's axes turned into Unity's),
//! the winding turned to match, scaled to meters (`useFileScale`) and by
//! `globalScale`, a submesh per material.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::material::{Surface, TextureProperty, UnityMaterial};
use crate::mesh::{BlendShape, MeshSkin, SkinNode, influences};
use crate::{Error, ObjRef, Project, UnityMesh};

/// A model file, loaded once for every mesh taken from it.
pub(crate) struct ModelFile {
    scene: ufbx::SceneRoot,
    /// Mesh sub-asset ids (`fileID`s that `MeshFilter`s use) to names.
    names: HashMap<i64, String>,
    /// Unity's import scale for it: units to meters times `globalScale`.
    scale: f32,
    /// Rows of the map from the file's space to Unity's (a signed axis
    /// permutation, the mirror included).
    to_unity: [[f64; 3]; 3],
    clips: Vec<Clip>,
    /// The file, for finding its textures.
    path: PathBuf,
    /// Material sub-asset ids (what renderers use) to names.
    material_names: HashMap<i64, String>,
    /// The model's materials the `.meta` maps to material assets of the
    /// project (`externalObjects`), by name.
    remaps: HashMap<String, ObjRef>,
    /// Whether Unity makes materials from the file's (`materialImportMode`
    /// isn't None).
    import_materials: bool,
    /// Whether textures are looked for in the whole project too
    /// (`searchTexturesGlobally`, or `materialSearch` Everywhere).
    search_everywhere: bool,
}

/// A material of a model file, as Unity's model importer gives it.
pub(crate) enum ModelMaterial {
    /// Mapped to a material asset of the project (`externalObjects`).
    Remapped(ObjRef),
    /// Made from the file's own.
    Embedded(Box<UnityMaterial>),
}

impl ModelFile {
    /// Loads `path`, with its `.meta` next to it.
    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
        let opts = ufbx::LoadOpts {
            generate_missing_normals: true,
            ignore_animation: false,
            ignore_embedded: true,
            ..Default::default()
        };
        let scene = ufbx::load_file(&path.to_string_lossy(), opts)
            .map_err(|e| Error::new(format!("{}: {}", path.display(), e.description)))?;
        let mut meta = path.as_os_str().to_owned();
        meta.push(".meta");
        let meta = std::fs::read_to_string(meta).unwrap_or_default();
        let setting = |key: &str| -> Option<f32> {
            meta.lines()
                .find_map(|l| l.trim().strip_prefix(key)?.trim().parse().ok())
        };
        let file_scale = if setting("useFileScale:").unwrap_or(1.0) >= 0.5 {
            scene.settings.unit_meters as f32
        } else {
            1.0
        };
        let scale = file_scale * setting("globalScale:").unwrap_or(1.0);
        let to_unity = if setting("bakeAxisConversion:").unwrap_or(0.0) >= 0.5 {
            baked_axes(&scene.settings.axes)
        } else {
            MIRROR_X
        };
        let mut names = sub_asset_names(&meta, 43);
        // Unity 2019.3+ leaves the table empty and derives ids from names:
        // xxHash64 of "Type:Mesh->" + name + how many came before by that name.
        let mut count: HashMap<String, usize> = HashMap::new();
        for node in scene.nodes.iter().filter(|n| n.mesh.is_some()) {
            let name = node.element.name.to_string();
            let k = count.entry(name.clone()).or_default();
            names.entry(hashed_id("Mesh", &name, *k)).or_insert(name);
            *k += 1;
        }
        let mut material_names = sub_asset_names(&meta, 21);
        let mut count: HashMap<String, usize> = HashMap::new();
        for material in &scene.materials {
            let name = material.element.name.to_string();
            let k = count.entry(name.clone()).or_default();
            material_names.entry(hashed_id("Material", &name, *k)).or_insert(name);
            *k += 1;
        }
        let parsed = crate::value::parse(&meta).unwrap_or(crate::Value::Null);
        let importer = &parsed["ModelImporter"];
        let settings = &importer["materials"];
        Ok(Self {
            scene,
            names,
            scale,
            to_unity,
            clips: clips(&meta),
            path: path.to_path_buf(),
            material_names,
            remaps: remaps(importer),
            import_materials: settings["materialImportMode"].as_i64() != Some(0),
            search_everywhere: settings["searchTexturesGlobally"].as_i64() == Some(1)
                || settings["materialSearch"].as_i64() == Some(2),
        })
    }

    /// The material with sub-asset id `file_id`: the project's material
    /// the `.meta` maps it to, or one made from the file's the way Unity's
    /// model importer makes it for the Universal Render Pipeline (base
    /// color and texture, normal map, emission, smoothness from
    /// shininess, transparency from opacity). Textures are looked for as
    /// Unity does (see [`find_texture`]). `None` when the model has no
    /// such material.
    pub(crate) fn material(&self, file_id: i64, project: &Project) -> Option<ModelMaterial> {
        let name = self.material_names.get(&file_id)?;
        if let Some(to) = self.remaps.get(name) {
            return Some(ModelMaterial::Remapped(to.clone()));
        }
        if !self.import_materials {
            return None;
        }
        let m = self.scene.materials.iter().find(|m| m.element.name == name.as_str())?;
        Some(ModelMaterial::Embedded(Box::new(embedded_material(
            m,
            &self.path,
            self.search_everywhere,
            project,
        ))))
    }

    /// The mesh with sub-asset id `file_id`, in Unity's space.
    pub(crate) fn mesh(&self, file_id: i64) -> Result<UnityMesh, Error> {
        let name = self
            .names
            .get(&file_id)
            .ok_or_else(|| Error::new(format!("no mesh {file_id} in the model's .meta")))?;
        let node = self
            .scene
            .nodes
            .iter()
            .find(|n| n.element.name == name.as_str() && n.mesh.is_some());
        let (mesh, to_node) = match node {
            Some(n) => (n.mesh.as_ref().expect("checked"), Some(n.geometry_to_node)),
            None => (
                self.scene
                    .meshes
                    .iter()
                    .find(|m| m.element.name == name.as_str())
                    .ok_or_else(|| Error::new(format!("no mesh \"{name}\" in the model")))?,
                None,
            ),
        };
        let m = self.to_unity;
        let to_unity = |v: ufbx::Vec3| m.map(|r| r[0] * v.x + r[1] * v.y + r[2] * v.z);
        let place = |p: ufbx::Vec3| {
            let p = to_unity(to_node.map_or(p, |m| transform(&m, p, 1.0)));
            p.map(|c| c as f32 * self.scale)
        };
        let turn = |n: ufbx::Vec3| {
            let n = to_unity(to_node.map_or(n, |m| transform(&m, n, 0.0)));
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
            n.map(|c| (c / len) as f32)
        };
        // A mirroring map turns the winding around.
        let mirrored = glam::DMat3::from_cols_array_2d(&m).determinant() < 0.0;
        let order = if mirrored { [0, 2, 1] } else { [0, 1, 2] };
        let mut out = UnityMesh {
            name: name.clone(),
            ..Default::default()
        };
        let skin = mesh.skin_deformers.first();
        if let Some(skin) = skin {
            if skin.clusters.len() > u16::MAX as usize {
                return Err(Error::new("too many skin joints"));
            }
            let convert = |matrix: &ufbx::Matrix| {
                let basis = glam::Mat4::from_cols(
                    glam::Vec4::new(m[0][0] as f32, m[1][0] as f32, m[2][0] as f32, 0.0) * self.scale,
                    glam::Vec4::new(m[0][1] as f32, m[1][1] as f32, m[2][1] as f32, 0.0) * self.scale,
                    glam::Vec4::new(m[0][2] as f32, m[1][2] as f32, m[2][2] as f32, 0.0) * self.scale,
                    glam::Vec4::W,
                );
                basis * matrix4(matrix) * basis.inverse()
            };
            let mesh_world = node.map_or(glam::Mat4::IDENTITY, |n| matrix4(&n.node_to_world));
            let nodes: Vec<_> = self
                .scene
                .nodes
                .iter()
                .map(|n| SkinNode {
                    name: n.element.name.to_string(),
                    parent: n.parent.as_ref().map(|p| p.element.typed_id as usize),
                    matrix: convert_matrix_relative(n, mesh_world, &convert),
                })
                .collect();
            let mut data = MeshSkin {
                nodes,
                ..Default::default()
            };
            for cluster in &skin.clusters {
                let bone = cluster
                    .bone_node
                    .as_ref()
                    .ok_or_else(|| Error::new("skin cluster has no bone"))?;
                data.joints.push(bone.element.typed_id as usize);
                data.inverse_binds
                    .push(convert(&cluster.mesh_node_to_bone).to_cols_array());
            }
            let fps = self.scene.settings.frames_per_second;
            let fps = if fps.is_finite() && fps > 0.0 { fps } else { 30.0 };
            for take in &self.scene.anim_stacks {
                let ranges = if self.clips.is_empty() {
                    vec![Clip {
                        name: take.element.name.to_string(),
                        take: None,
                        first: take.time_begin * fps,
                        last: take.time_end * fps,
                        looping: false,
                    }]
                } else {
                    self.clips
                        .iter()
                        .filter(|c| c.take.as_deref().is_none_or(|n| n == take.element.name.as_ref()))
                        .cloned()
                        .collect()
                };
                for clip in ranges {
                    let begin = (clip.first / fps).max(take.time_begin);
                    let end = (clip.last / fps).min(take.time_end);
                    if end < begin {
                        continue;
                    }
                    let count = ((end - begin) * fps).ceil() as usize;
                    let mut times: Vec<f32> = (0..=count)
                        .map(|i| ((i as f64 / fps).min(end - begin)) as f32)
                        .collect();
                    // Include source discontinuities as well as regular frame samples.
                    for layer in &take.layers {
                        for value in &layer.anim_values {
                            for curve in value.curves.iter().flatten() {
                                times.extend(
                                    curve
                                        .keyframes
                                        .iter()
                                        .filter(|k| k.time >= begin && k.time <= end)
                                        .map(|k| (k.time - begin) as f32),
                                );
                            }
                        }
                    }
                    times.sort_by(f32::total_cmp);
                    times.dedup();
                    let step = self
                        .scene
                        .nodes
                        .iter()
                        .map(|n| {
                            ["Lcl Translation", "Lcl Rotation", "Lcl Scaling"].map(|property| {
                                let props: Vec<_> = take
                                    .layers
                                    .iter()
                                    .flat_map(|l| l.anim_props.iter())
                                    .filter(|p| p.element.element_id == n.element.element_id && p.prop_name == property)
                                    .collect();
                                !props.is_empty()
                                    && props.iter().all(|p| {
                                        p.anim_value.curves.iter().flatten().all(|c| {
                                            c.keyframes
                                                .iter()
                                                .all(|k| k.interpolation == ufbx::Interpolation::ConstantPrev)
                                        })
                                    })
                            })
                        })
                        .collect();
                    let nodes = self
                        .scene
                        .nodes
                        .iter()
                        .map(|n| {
                            times
                                .iter()
                                .map(|&t| {
                                    let local = ufbx::transform_to_matrix(&ufbx::evaluate_transform(
                                        &take.anim,
                                        n,
                                        begin + t as f64,
                                    ));
                                    let matrix = convert(&local);
                                    if n.parent.is_none() {
                                        // The same renderer-relative root used by the rest pose.
                                        let rest_local = convert(&n.node_to_parent);
                                        glam::Mat4::from_cols_array(&data.nodes[n.element.typed_id as usize].matrix)
                                            * rest_local.inverse()
                                            * matrix
                                    } else {
                                        matrix
                                    }
                                })
                                .map(|m| m.to_cols_array())
                                .collect()
                        })
                        .collect();
                    data.animations.push(crate::mesh::SkinAnimation {
                        morphs: Vec::new(),
                        name: clip.name,
                        looping: clip.looping,
                        step,
                        times,
                        nodes,
                    });
                }
            }
            out.skin = Some(data);
        }
        // Every authored blend channel frame. Offsets turn and scale like
        // positions, without moving.
        let shift = |d: ufbx::Vec3| {
            let d = to_unity(to_node.map_or(d, |m| transform(&m, d, 0.0)));
            d.map(|c| c as f32 * self.scale)
        };
        let lean = |d: ufbx::Vec3| to_unity(to_node.map_or(d, |m| transform(&m, d, 0.0))).map(|c| c as f32);
        let channels: Vec<_> = mesh
            .blend_deformers
            .iter()
            .flat_map(|b| b.channels.iter())
            .flat_map(|c| {
                c.keyframes
                    .iter()
                    .enumerate()
                    .map(move |(i, k)| (c, &k.shape, k.target_weight as f32, i + 1 == c.keyframes.len()))
            })
            .collect();
        out.blend_shapes = channels
            .iter()
            .map(|(channel, _, weight, last)| BlendShape {
                name: if *last {
                    channel.element.name.to_string()
                } else {
                    format!("{}@{weight}", channel.element.name)
                },
                channel: channel.element.name.to_string(),
                frame_weight: *weight,
                weight: if *last { channel.weight as f32 } else { 0. },
                ..Default::default()
            })
            .collect();
        // Each shape's offsets by the file's vertex, and whether it moves
        // normals too.
        let offsets: Vec<(ShapeOffsets, bool)> = channels
            .iter()
            .map(|(_, shape, _, _)| {
                let normals = shape.normal_offsets.len() >= shape.num_offsets && shape.num_offsets > 0;
                let by_vertex = (0..shape.num_offsets)
                    .map(|k| {
                        let n = if normals {
                            shape.normal_offsets[k]
                        } else {
                            ufbx::Vec3::default()
                        };
                        (shape.offset_vertices[k], (shape.position_offsets[k], n))
                    })
                    .collect();
                (by_vertex, normals)
            })
            .collect();
        let mut seen: HashMap<[u32; 21], u32> = HashMap::new();
        let mut corners = vec![0u32; mesh.max_face_triangles * 3];
        let groups: Vec<Vec<u32>> = if mesh.material_parts.is_empty() {
            vec![(0..mesh.faces.len() as u32).collect()]
        } else {
            // Unity numbers submeshes by the first face using each material,
            // rather than by the FBX material connection index. Renderer
            // material arrays already use that order, including overrides.
            mesh.material_part_usage_order
                .iter()
                .map(|&i| &mesh.material_parts[i as usize])
                .filter(|p| !p.face_indices.is_empty())
                .map(|p| p.face_indices.iter().copied().collect())
                .collect()
        };
        for faces in groups {
            let mut indices = Vec::new();
            for &f in &faces {
                let face = mesh.faces[f as usize];
                let n = ufbx::triangulate_face(&mut corners, mesh, face) as usize;
                for tri in corners[..n * 3].as_chunks::<3>().0 {
                    for k in order {
                        let i = tri[k] as usize;
                        let p = place(mesh.vertex_position[i]);
                        let nrm = if mesh.vertex_normal.exists {
                            turn(mesh.vertex_normal[i])
                        } else {
                            [0.0, 1.0, 0.0]
                        };
                        let uv = if mesh.vertex_uv.exists {
                            let uv = mesh.vertex_uv[i];
                            [uv.x as f32, uv.y as f32]
                        } else {
                            [0.0; 2]
                        };
                        let color = if mesh.vertex_color.exists {
                            let c = mesh.vertex_color[i];
                            [c.x as f32, c.y as f32, c.z as f32, c.w as f32]
                        } else {
                            [1.0; 4]
                        };
                        let (joints, weights) = skin.map_or(([0; 4], [0.0; 4]), |skin| {
                            let vertex = &skin.vertices[mesh.vertex_indices[i] as usize];
                            let begin = vertex.weight_begin as usize;
                            influences(
                                skin.weights
                                    .iter()
                                    .skip(begin)
                                    .take(vertex.num_weights as usize)
                                    .map(|w| (w.cluster_index as u16, w.weight as f32)),
                            )
                        });
                        let mut key = [0; 21];
                        key[..12].copy_from_slice(
                            &[
                                p[0], p[1], p[2], nrm[0], nrm[1], nrm[2], uv[0], uv[1], color[0], color[1], color[2],
                                color[3],
                            ]
                            .map(f32::to_bits),
                        );
                        key[12..16].copy_from_slice(&joints.map(u32::from));
                        key[16..20].copy_from_slice(&weights.map(f32::to_bits));
                        // Corners of different vertices may move apart.
                        if !offsets.is_empty() {
                            key[20] = mesh.vertex_indices[i];
                        }
                        let at = *seen.entry(key).or_insert_with(|| {
                            if let Some(skin) = &mut out.skin {
                                skin.indices.push(joints);
                                skin.weights.push(weights);
                            }
                            let vertex = mesh.vertex_indices[i];
                            for (shape, (offsets, normals)) in out.blend_shapes.iter_mut().zip(&offsets) {
                                let (d, n) = offsets.get(&vertex).copied().unwrap_or_default();
                                shape.positions.push(shift(d));
                                if *normals {
                                    shape.normals.push(lean(n));
                                }
                            }
                            out.positions.push(p);
                            out.normals.push(nrm);
                            out.uvs[0].push(uv);
                            out.colors.push(color);
                            (out.positions.len() - 1) as u32
                        });
                        indices.push(at);
                    }
                }
            }
            out.submeshes.push(indices);
        }
        if !mesh.vertex_uv.exists {
            out.uvs[0].clear();
        }
        if !mesh.vertex_color.exists {
            out.colors.clear();
        }
        Ok(out)
    }
}

/// A blend shape's position and normal offsets, by the file's vertex.
type ShapeOffsets = HashMap<u32, (ufbx::Vec3, ufbx::Vec3)>;

#[derive(Clone)]
struct Clip {
    name: String,
    take: Option<String>,
    first: f64,
    last: f64,
    looping: bool,
}

fn clips(meta: &str) -> Vec<Clip> {
    use yaml_rust2::YamlLoader;
    let Ok(docs) = YamlLoader::load_from_str(meta) else {
        return Vec::new();
    };
    let Some(doc) = docs.first() else {
        return Vec::new();
    };
    // Unity writes them under `animations:`; very old files had them
    // directly under the importer.
    let importer = &doc["ModelImporter"];
    let Some(list) = importer["animations"]["clipAnimations"]
        .as_vec()
        .or_else(|| importer["clipAnimations"].as_vec())
    else {
        return Vec::new();
    };
    let number = |v: &yaml_rust2::Yaml| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)).unwrap_or(0.0);
    list.iter()
        .map(|c| Clip {
            name: c["name"].as_str().unwrap_or("Clip").to_owned(),
            take: c["takeName"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
            first: number(&c["firstFrame"]),
            last: number(&c["lastFrame"]),
            looping: c["loopTime"].as_bool().unwrap_or_else(|| number(&c["loopTime"]) != 0.0)
                || c["loop"].as_bool().unwrap_or_else(|| number(&c["loop"]) != 0.0),
        })
        .collect()
}

/// The importer's `externalObjects` that map the model's materials to
/// material assets: name to the asset.
fn remaps(importer: &crate::Value) -> HashMap<String, ObjRef> {
    importer["externalObjects"]
        .as_seq()
        .unwrap_or_default()
        .iter()
        .filter(|e| e["first"]["type"].as_str().is_some_and(|t| t.ends_with(":Material")))
        .filter_map(|e| {
            let to = e["second"].obj_ref().filter(|r| !r.is_none())?;
            Some((e["first"]["name"].as_str()?.to_owned(), to))
        })
        .collect()
}

/// The GUID of the Universal Render Pipeline's Lit shader, which Unity
/// gives the materials it makes from a model's when that pipeline is on.
const URP_LIT: &str = "933532a4fcc9baf4fa0491de14d08ed7";

/// A model's material as Unity makes it from the file's properties: the
/// Universal Render Pipeline's rules for model material descriptions
/// (`DiffuseColor` texture or color, `Bump` or `NormalMap`,
/// `EmissiveColor`, `Shininess`, `Opacity` or `TransparencyFactor`).
fn embedded_material(m: &ufbx::Material, model: &Path, everywhere: bool, project: &Project) -> UnityMaterial {
    let prop = |name: &str| m.element.props.find_prop(name);
    let number = |name: &str| prop(name).map(|p| p.value_vec4.x as f32);
    let vector = |name: &str| {
        prop(name).map(|p| {
            let v = p.value_vec4;
            [v.x as f32, v.y as f32, v.z as f32, v.w as f32]
        })
    };
    let texture = |name: &str| {
        m.find_prop_texture(name).and_then(|t| {
            find_texture(project, model, &t.relative_filename, &t.filename, everywhere).map(|guid| ObjRef {
                file_id: 2_800_000,
                guid: Some(guid),
                kind: Some(3),
            })
        })
    };
    // Opacity, else one minus the transparency factor or color.
    let mut opacity = number("Opacity").unwrap_or_else(|| match number("TransparencyFactor") {
        Some(f) if f != 1.0 => 1.0 - f,
        _ => 1.0,
    });
    if prop("Opacity").is_none()
        && opacity == 1.0
        && let Some(c) = vector("TransparentColor")
        && c[0] != 1.0
    {
        opacity = 1.0 - c[0];
    }
    let transparent = opacity < 1.0 || (opacity == 1.0 && m.find_prop_texture("TransparentColor").is_some());
    let base_map = texture("DiffuseColor");
    let color = if base_map.is_some() {
        let f = number("DiffuseFactor").unwrap_or(1.0);
        [f, f, f, opacity]
    } else {
        // The file's colors are linear; Unity keeps material colors sRGB.
        let c = vector("DiffuseColor").unwrap_or([1.0; 4]);
        [
            linear_to_srgb(c[0]),
            linear_to_srgb(c[1]),
            linear_to_srgb(c[2]),
            opacity,
        ]
    };
    let normal_map = texture("Bump").or_else(|| texture("NormalMap"));
    let normal_scale = number("BumpFactor").filter(|_| normal_map.is_some()).unwrap_or(1.0);
    let factor = number("EmissiveFactor").unwrap_or(0.0);
    let emission_map = texture("EmissiveColor");
    let emission = if emission_map.is_some() {
        if factor > 0.0 { [1.0; 3] } else { [0.0; 3] }
    } else {
        match vector("EmissiveColor") {
            Some(c) if factor > 0.0 && (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]) > 0.0 => {
                [c[0] * factor, c[1] * factor, c[2] * factor]
            }
            _ => [0.0; 3],
        }
    };
    let smoothness = number("Shininess").map_or(0.0, |s| (s * 0.01).max(0.0).sqrt());
    let mut textures = BTreeMap::new();
    for (key, map) in [
        ("_BaseMap", &base_map),
        ("_BumpMap", &normal_map),
        ("_EmissionMap", &emission_map),
    ] {
        if let Some(r) = map {
            textures.insert(
                key.to_owned(),
                TextureProperty {
                    texture: r.clone(),
                    scale: [1.0; 2],
                    offset: [0.0; 2],
                },
            );
        }
    }
    let surface = if transparent { 1.0 } else { 0.0 };
    let floats = BTreeMap::from([
        ("_BumpScale".to_owned(), normal_scale),
        ("_Smoothness".to_owned(), smoothness),
        ("_Metallic".to_owned(), 0.0),
        ("_Surface".to_owned(), surface),
    ]);
    let colors = BTreeMap::from([
        ("_BaseColor".to_owned(), color),
        (
            "_EmissionColor".to_owned(),
            [emission[0], emission[1], emission[2], 1.0],
        ),
    ]);
    let urp = project.path_of(URP_LIT).is_some();
    UnityMaterial {
        name: m.element.name.to_string(),
        shader: urp.then(|| URP_LIT.to_owned()),
        // Unity's Standard shader without the pipeline.
        shader_file_id: if urp { 4_800_000 } else { 46 },
        color,
        base_map,
        tiling: [1.0; 2],
        offset: [0.0; 2],
        normal_map,
        normal_scale,
        metallic: 0.0,
        smoothness,
        metallic_map: None,
        smoothness_in_base_alpha: false,
        occlusion_map: None,
        occlusion_strength: 1.0,
        emission,
        emission_map: emission_map.filter(|_| emission != [0.0; 3]),
        surface: if transparent {
            Surface::Transparent
        } else {
            Surface::Opaque
        },
        double_sided: false,
        floats,
        colors,
        textures,
    }
}

/// The GUID of the texture a model's material names (`relative`, the
/// path saved in the file, relative to it, and `saved`, the path as
/// resolved), found the way Unity's model importer finds it: that path,
/// then by its file name in the model's folder, in a `Textures` folder
/// there and in every folder above it up to `Assets`, then (when
/// `everywhere`) anywhere in the project.
fn find_texture(project: &Project, model: &Path, relative: &str, saved: &str, everywhere: bool) -> Option<String> {
    let dir = model.parent()?;
    let file_name = |p: &str| {
        p.rsplit(['/', '\\'])
            .next()
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
    };
    let name = file_name(relative).or_else(|| file_name(saved))?;
    let mut tries: Vec<PathBuf> = Vec::new();
    if !relative.is_empty() {
        tries.push(dir.join(relative.replace('\\', "/")));
    }
    tries.push(dir.join(&name));
    let assets = project.root().join("Assets");
    let mut at = Some(dir);
    while let Some(d) = at {
        tries.push(d.join("Textures").join(&name));
        if d == assets || !d.starts_with(project.root()) {
            break;
        }
        at = d.parent();
    }
    tries
        .iter()
        .find_map(|p| project.guid_of(&normalize(p)))
        .map(str::to_owned)
        .or_else(|| {
            everywhere
                .then(|| project.find_by_file_name(&name))
                .flatten()
                .map(str::to_owned)
        })
}

/// `path` with `.` and `..` parts folded away.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Unity's `Mathf.LinearToGammaSpace`: a linear color channel as sRGB.
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0 {
        0.0
    } else if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Unity's handedness change alone: X negated.
const MIRROR_X: [[f64; 3]; 3] = [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// What Unity's "Bake Axis Conversion" does to a file saved with `axes`:
/// the file's right, up and front become Unity's +X, +Y and -Z (so a
/// right-handed Y-up file only has Z negated, and a Z-up file such as
/// Blender's has Y and Z swapped). Unity bakes that into the vertices
/// instead of turning the model's root.
fn baked_axes(axes: &ufbx::CoordinateAxes) -> [[f64; 3]; 3] {
    let unit = |a: ufbx::CoordinateAxis, default: [f64; 3]| match a {
        ufbx::CoordinateAxis::PositiveX => [1.0, 0.0, 0.0],
        ufbx::CoordinateAxis::NegativeX => [-1.0, 0.0, 0.0],
        ufbx::CoordinateAxis::PositiveY => [0.0, 1.0, 0.0],
        ufbx::CoordinateAxis::NegativeY => [0.0, -1.0, 0.0],
        ufbx::CoordinateAxis::PositiveZ => [0.0, 0.0, 1.0],
        ufbx::CoordinateAxis::NegativeZ => [0.0, 0.0, -1.0],
        ufbx::CoordinateAxis::Unknown => default,
    };
    // FBX's default when a file doesn't say: right-handed, Y up.
    let right = unit(axes.right, [1.0, 0.0, 0.0]);
    let up = unit(axes.up, [0.0, 1.0, 0.0]);
    let front = unit(axes.front, [0.0, 0.0, 1.0]);
    [right, up, front.map(|c| -c)]
}

fn transform(m: &ufbx::Matrix, v: ufbx::Vec3, w: f64) -> ufbx::Vec3 {
    ufbx::Vec3 {
        x: m.m00 * v.x + m.m01 * v.y + m.m02 * v.z + m.m03 * w,
        y: m.m10 * v.x + m.m11 * v.y + m.m12 * v.z + m.m13 * w,
        z: m.m20 * v.x + m.m21 * v.y + m.m22 * v.z + m.m23 * w,
    }
}

/// The id Unity 2019.3+ gives sub-asset `name` of `type` (`"Mesh"`), the
/// `index`th by that name.
fn hashed_id(type_name: &str, name: &str, index: usize) -> i64 {
    xxhash_rust::xxh64::xxh64(format!("Type:{type_name}->{name}{index}").as_bytes(), 0) as i64
}

/// Sub-asset ids of class `class` (43 meshes, 21 materials) to names from
/// a model's `.meta`: Unity 2019.3+ `internalIDToNameTable` (`first: {43:
/// id}`, `second: name`), or the older `fileIDToRecycleName` (`4300002:
/// name`, ids from the class times 100000).
fn sub_asset_names(meta: &str, class: i64) -> HashMap<i64, String> {
    let ids = class * 100_000..(class + 1) * 100_000;
    let key = format!("{class}: ");
    let mut names = HashMap::new();
    let mut pending: Option<i64> = None;
    let mut recycle = false;
    for line in meta.lines() {
        let t = line.trim();
        if t.starts_with("fileIDToRecycleName:") {
            recycle = true;
            continue;
        }
        if recycle {
            match t.split_once(": ") {
                Some((id, name)) if line.starts_with("    ") => {
                    if let Ok(id) = id.parse::<i64>()
                        && ids.contains(&id)
                    {
                        names.insert(id, name.to_string());
                    }
                    continue;
                }
                _ => recycle = false,
            }
        }
        if let Some(id) = t.strip_prefix(key.as_str()) {
            pending = id.trim().parse().ok();
        } else if let Some(name) = t.strip_prefix("second: ")
            && let Some(id) = pending.take()
        {
            names.insert(id, name.to_string());
        } else if t.starts_with("- first:") || t.starts_with("first:") {
            pending = None;
        }
    }
    names
}

fn matrix4(m: &ufbx::Matrix) -> glam::Mat4 {
    glam::Mat4::from_cols_array(&[
        m.m00 as f32,
        m.m10 as f32,
        m.m20 as f32,
        0.0,
        m.m01 as f32,
        m.m11 as f32,
        m.m21 as f32,
        0.0,
        m.m02 as f32,
        m.m12 as f32,
        m.m22 as f32,
        0.0,
        m.m03 as f32,
        m.m13 as f32,
        m.m23 as f32,
        1.0,
    ])
}

fn convert_matrix_relative(
    n: &ufbx::Node,
    mesh_world: glam::Mat4,
    convert: &impl Fn(&ufbx::Matrix) -> glam::Mat4,
) -> [f32; 16] {
    if n.parent.is_some() {
        convert(&n.node_to_parent).to_cols_array()
    } else {
        // Root coordinates are relative to the renderer, whose transform is
        // supplied by the imported scene entity.
        let world = convert(&n.node_to_world);
        let mesh = ufbx::Matrix {
            m00: mesh_world.x_axis.x as f64,
            m10: mesh_world.x_axis.y as f64,
            m20: mesh_world.x_axis.z as f64,
            m01: mesh_world.y_axis.x as f64,
            m11: mesh_world.y_axis.y as f64,
            m21: mesh_world.y_axis.z as f64,
            m02: mesh_world.z_axis.x as f64,
            m12: mesh_world.z_axis.y as f64,
            m22: mesh_world.z_axis.z as f64,
            m03: mesh_world.w_axis.x as f64,
            m13: mesh_world.w_axis.y as f64,
            m23: mesh_world.w_axis.z as f64,
        };
        (convert(&mesh).inverse() * world).to_cols_array()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_take_samples_match_ufbx_and_rest_at_start() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skinned.fbx");
        let model = ModelFile::open(&path).unwrap();
        let id = *model.names.keys().next().unwrap();
        let mesh = model.mesh(id).unwrap();
        let skin = mesh.skin.unwrap();
        assert!(!skin.animations.is_empty(), "fixture must contain a take");
        for clip in &skin.animations {
            assert_eq!(clip.times[0], 0.0);
            for (node, samples) in skin.nodes.iter().zip(&clip.nodes) {
                let rest = glam::Mat4::from_cols_array(&node.matrix);
                let start = glam::Mat4::from_cols_array(&samples[0]);
                assert!(rest.abs_diff_eq(start, 1e-4), "{} starts away from rest", node.name);
            }
        }
    }

    #[test]
    fn meta_clips_preserve_frame_ranges_names_and_loop_flags() {
        let old = clips("ModelImporter:\n  clipAnimations:\n  - name: Old\n    firstFrame: 1\n    lastFrame: 2\n");
        assert_eq!(old.len(), 1, "very old files keep them under the importer");
        let clips = clips(
            "ModelImporter:\n  animations:\n    clipAnimations:\n    - name: Walk\n      takeName: Take 001\n      firstFrame: 12\n      lastFrame: 24\n      loopTime: 1\n    - name: Stop\n      firstFrame: 24.5\n      lastFrame: 30\n      loop: 0\n",
        );
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].name, "Walk");
        assert_eq!(clips[0].take.as_deref(), Some("Take 001"));
        assert_eq!((clips[0].first, clips[0].last), (12.0, 24.0));
        assert!(clips[0].looping);
        assert!(!clips[1].looping);
        assert_eq!(clips[1].first, 24.5);
    }

    #[test]
    fn hashed_ids_match_unitys() {
        // The id Unity 6 gave a model's mesh named Body (checked in an
        // editor-imported project), and the next one by that name.
        assert_eq!(hashed_id("Mesh", "Body", 0), 2959467708922500008);
        assert_eq!(hashed_id("Mesh", "Body", 1), -1107389024503286118);
    }

    #[test]
    fn baked_axes_match_unitys_for_y_up_and_z_up_files() {
        use ufbx::CoordinateAxis::*;
        let axes = |right, up, front| ufbx::CoordinateAxes { right, up, front };
        // Right-handed Y-up (front +Z): only Z is negated.
        let y_up = baked_axes(&axes(PositiveX, PositiveY, PositiveZ));
        assert_eq!(y_up, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]]);
        // Right-handed Z-up (front -Y): Y and Z swap.
        let z_up = baked_axes(&axes(PositiveX, PositiveZ, NegativeY));
        assert_eq!(z_up, [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn mesh_names_come_from_either_meta_table() {
        let new = "ModelImporter:\n  internalIDToNameTable:\n  - first:\n      1: 100000\n    second: //RootNode\n  - first:\n      43: -2153578634513813181\n    second: Birch_1\n  - first:\n      43: 4300002\n    second: Birch_2\n";
        let names = sub_asset_names(new, 43);
        assert_eq!(names[&-2153578634513813181], "Birch_1");
        assert_eq!(names[&4300002], "Birch_2");
        assert_eq!(names.len(), 2);
        let old = "ModelImporter:\n  fileIDToRecycleName:\n    100000: //RootNode\n    4300000: Rock\n    4300002: Rock_LOD1\n  externalObjects: {}\n";
        let names = sub_asset_names(old, 43);
        assert_eq!(
            (names[&4300000].as_str(), names[&4300002].as_str()),
            ("Rock", "Rock_LOD1")
        );
        assert_eq!(names.len(), 2);
    }
}
