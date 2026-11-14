//! Meshes read from model files (FBX) the way Unity's model importer
//! places them, with and without "Bake Axis Conversion".

use ponko_unity::*;
use serde_json::Value as Json;

/// A prefab drawing the model's "Lid" mesh at the origin, unturned.
const PREFAB: &str = "%YAML 1.1
--- !u!1 &1
GameObject:
  m_Component:
  - component: {fileID: 2}
  - component: {fileID: 3}
  - component: {fileID: 4}
  m_Name: Lid
  m_IsActive: 1
--- !u!4 &2
Transform:
  m_GameObject: {fileID: 1}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 0, y: 0, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children: []
  m_Father: {fileID: 0}
--- !u!33 &3
MeshFilter:
  m_GameObject: {fileID: 1}
  m_Mesh: {fileID: 4300000, guid: lid00000, type: 3}
--- !u!23 &4
MeshRenderer:
  m_GameObject: {fileID: 1}
  m_Enabled: 1
  m_Materials: []
";

/// A mesh as the importer wrote it, in Ponko's space.
struct Mesh {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl Mesh {
    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut b = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &self.positions {
            for (i, &c) in p.iter().enumerate() {
                b.0[i] = b.0[i].min(c);
                b.1[i] = b.1[i].max(c);
            }
        }
        b
    }
}

/// Imports the prefab with the hand-written Z-up model `lid_z_up.fbx`
/// next to it, whose `.meta` holds `settings` under `meshes:`.
fn import(name: &str, settings: &str) -> Mesh {
    read_glb(&import_glb(name, include_bytes!("fixtures/lid_z_up.fbx"), settings))
}

/// Imports the prefab with model `fbx` as `Lid.fbx`; the `.glb` written.
fn import_glb(name: &str, fbx: &[u8], settings: &str) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("ponko_unity_models_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let assets = dir.join("Assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("Lid.fbx"), fbx).unwrap();
    std::fs::write(
        assets.join("Lid.fbx.meta"),
        format!("guid: lid00000\nModelImporter:\n  fileIDToRecycleName:\n    4300000: Lid\n  meshes:\n{settings}"),
    )
    .unwrap();
    std::fs::write(assets.join("Lid.prefab"), PREFAB).unwrap();
    std::fs::write(assets.join("Lid.prefab.meta"), "guid: lidprefb\n").unwrap();
    let project = Project::open(&dir).unwrap();
    let out = dir.join("out");
    let file = UnityFile::read(project.root().join("Assets/Lid.prefab")).unwrap();
    let hierarchy = Hierarchy::build(&file, &project).unwrap();
    let mut importer = Importer::new(&project, &out, "lid/");
    let scene = importer.scene(&hierarchy).unwrap();
    importer.finish().unwrap();
    let path = scene["entities"][0]["components"]["Model"]["mesh"].as_str().unwrap();
    let glb = std::fs::read(out.join(path.strip_prefix("lid/").unwrap())).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    glb
}

/// The first primitive of a `.glb` the importer wrote (f32 vectors, u32
/// indices, one buffer).
fn read_glb(bytes: &[u8]) -> Mesh {
    assert_eq!(&bytes[..4], b"glTF");
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let doc: Json = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
    let bin = &bytes[20 + len + 8..];
    let data = |accessor: &Json| -> &[u8] {
        let a = &doc["accessors"][accessor.as_u64().unwrap() as usize];
        let view = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let at = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        &bin[at..at + view["byteLength"].as_u64().unwrap() as usize]
    };
    let words = |b: &[u8]| -> Vec<[u8; 4]> { b.as_chunks::<4>().0.to_vec() };
    let vec3s = |b: &[u8]| -> Vec<[f32; 3]> {
        words(b)
            .into_iter()
            .map(f32::from_le_bytes)
            .collect::<Vec<_>>()
            .as_chunks::<3>()
            .0
            .to_vec()
    };
    let primitive = &doc["meshes"][0]["primitives"][0];
    Mesh {
        positions: vec3s(data(&primitive["attributes"]["POSITION"])),
        normals: vec3s(data(&primitive["attributes"]["NORMAL"])),
        indices: words(data(&primitive["indices"]))
            .into_iter()
            .map(u32::from_le_bytes)
            .collect(),
    }
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5)
}

/// Every triangle's front (counter-clockwise, as glTF draws) faces the way
/// its vertex normals do.
fn assert_winding_matches_normals(mesh: &Mesh) {
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[tri[k] as usize]);
        let (u, v) = ([0, 1, 2].map(|i| b[i] - a[i]), [0, 1, 2].map(|i| c[i] - a[i]));
        let face = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let n = mesh.normals[tri[0] as usize];
        assert!(
            face[0] * n[0] + face[1] * n[1] + face[2] * n[2] > 0.0,
            "{face:?} vs {n:?}"
        );
    }
}

#[test]
fn baked_axis_conversion_turns_z_up_meshes_y_up() {
    // The file's plate lies flat 2 m up its Z axis, 1 m along X and 3 m
    // along Y. Unity bakes the file's right, up and front (X, Z, -Y) into
    // its own +X, +Y and -Z: x stays, the height becomes y and the file's
    // y becomes z. Ponko's space then negates Unity's z.
    let mesh = import(
        "baked",
        "    bakeAxisConversion: 1\n    useFileScale: 1\n    globalScale: 1\n",
    );
    let (min, max) = mesh.bounds();
    assert!(
        close(min, [0.0, 2.0, -3.0]) && close(max, [1.0, 2.0, 0.0]),
        "{min:?} {max:?}"
    );
    assert!(
        mesh.normals.iter().all(|n| close(*n, [0.0, 1.0, 0.0])),
        "faces up: {:?}",
        mesh.normals
    );
    assert_winding_matches_normals(&mesh);
}

#[test]
fn baked_axis_conversion_keeps_the_import_scale() {
    let mesh = import(
        "scaled",
        "    bakeAxisConversion: 1\n    useFileScale: 1\n    globalScale: 2\n",
    );
    let (min, max) = mesh.bounds();
    assert!(
        close(min, [0.0, 4.0, -6.0]) && close(max, [2.0, 4.0, 0.0]),
        "{min:?} {max:?}"
    );
}

#[test]
fn without_baked_axis_conversion_only_x_is_mirrored() {
    // Unity keeps the file's axes in the mesh (and turns the model's root
    // instead): X negated, then Ponko negates z.
    for settings in ["    bakeAxisConversion: 0\n", ""] {
        let mesh = import("unbaked", settings);
        let (min, max) = mesh.bounds();
        assert!(
            close(min, [-1.0, 0.0, -2.0]) && close(max, [0.0, 3.0, -2.0]),
            "{min:?} {max:?}"
        );
        assert!(
            mesh.normals.iter().all(|n| close(*n, [0.0, 0.0, -1.0])),
            "{:?}",
            mesh.normals
        );
        assert_winding_matches_normals(&mesh);
    }
}

#[test]
fn skinned_renderers_export_fbx_bones_and_bind_space() {
    let dir = std::env::temp_dir().join(format!("ponko_unity_skin_{}", std::process::id()));
    let assets = dir.join("Assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("Lid.fbx"), include_bytes!("fixtures/skinned.fbx")).unwrap();
    std::fs::write(
        assets.join("Lid.fbx.meta"),
        "guid: lid00000\nModelImporter:\n  fileIDToRecycleName:\n    4300000: Lid\n",
    )
    .unwrap();
    let meta_path = assets.join("Lid.fbx.meta");
    let mut meta = std::fs::read_to_string(&meta_path).unwrap();
    meta.push_str("  clipAnimations:\n  - name: Half\n    takeName: Move\n    firstFrame: 0\n    lastFrame: 12\n    loopTime: 1\n  - name: End\n    takeName: Move\n    firstFrame: 12\n    lastFrame: 24\n");
    std::fs::write(meta_path, meta).unwrap();
    let prefab = PREFAB.replace(
        "--- !u!23 &4\nMeshRenderer:",
        "--- !u!137 &4\nSkinnedMeshRenderer:\n  m_Mesh: {fileID: 4300000, guid: lid00000, type: 3}",
    );
    let file = UnityFile::parse(&prefab).unwrap();
    let project = Project::open(&dir).unwrap();
    let hierarchy = Hierarchy::build(&file, &project).unwrap();
    let out = dir.join("out");
    let mut importer = Importer::new(&project, &out, "");
    let scene = importer.scene(&hierarchy).unwrap();
    let path = scene["entities"][0]["components"]["Model"]["mesh"].as_str().unwrap();
    let bytes = std::fs::read(out.join(path)).unwrap();
    let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
    let blob = gltf.blob.as_deref().unwrap();
    let json_length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let doc: serde_json::Value = serde_json::from_slice(&bytes[20..20 + json_length]).unwrap();
    assert_eq!(doc["animations"][0]["extras"]["loop"], true);
    assert_eq!(doc["animations"][1]["extras"]["loop"], false);
    let clips: Vec<_> = gltf.animations().collect();
    assert_eq!(
        clips.iter().map(|c| c.name().unwrap()).collect::<Vec<_>>(),
        ["Half", "End"]
    );
    for clip in &clips {
        let channel = clip
            .channels()
            .find(|c| {
                c.target().node().name() == Some("RootBone")
                    && c.target().property() == gltf::animation::Property::Translation
            })
            .unwrap();
        let reader = channel.reader(|_| Some(blob));
        let times: Vec<_> = reader.read_inputs().unwrap().collect();
        assert_eq!(times.first(), Some(&0.0));
        assert!(times.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(times.len(), 13);
        assert_eq!(times.last(), Some(&0.5));
        let gltf::animation::util::ReadOutputs::Translations(values) = reader.read_outputs().unwrap() else {
            panic!("translation")
        };
        let values: Vec<_> = values.collect();
        let start = if clip.name() == Some("Half") { -2.0 } else { -2.5 };
        assert!((values[0][2] - start).abs() < 1e-5);
        assert!((values.last().unwrap()[2] - (start - 0.5)).abs() < 1e-5);
    }
    let skin = gltf.skins().next().unwrap();
    let root = skin.joints().next().unwrap();
    assert_eq!(root.name(), Some("RootBone"));
    assert_eq!(root.children().next().unwrap().name(), Some("ChildBone"));
    assert_eq!(root.transform().matrix()[3][2], -2.0);
    let bind = skin
        .reader(|_| Some(blob))
        .read_inverse_bind_matrices()
        .unwrap()
        .next()
        .unwrap();
    assert_eq!(bind[3][2], 2.0);
    let primitive = gltf.meshes().next().unwrap().primitives().next().unwrap();
    let reader = primitive.reader(|_| Some(blob));
    assert!(reader.read_joints(0).unwrap().into_u16().all(|j| j == [0; 4]));
    assert!(
        reader
            .read_weights(0)
            .unwrap()
            .into_f32()
            .all(|w| w == [1.0, 0.0, 0.0, 0.0])
    );
    // Geometry's +1 Z offset is baked into the vertices, but not posed.
    assert!(reader.read_positions().unwrap().all(|p| p[2] == -2.0));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn asset_and_fbx_skins_use_renderer_bones() {
    for (asset, partial) in [(false, false), (true, false), (true, true)] {
        let dir = std::env::temp_dir().join(format!("ponko_scene_skin_{asset}_{partial}_{}", std::process::id()));
        let assets = dir.join("Assets");
        std::fs::create_dir_all(&assets).unwrap();
        let extension = if asset { "asset" } else { "fbx" };
        if asset {
            let mut text = include_str!("fixtures/quad.asset").to_owned();
            text.push_str("\n  m_BindPose:\n");
            for z in [-2, -3] {
                text.push_str(&format!("  - {{e00: 1, e11: 1, e22: 1, e33: 1, e23: {z}}}\n"));
            }
            text.push_str("  m_Skin:\n");
            for _ in 0..5 {
                text.push_str("  - {weight0: 0.25, weight1: 0.75, weight2: 0.0001, boneIndex0: 0, boneIndex1: 1, boneIndex2: 0}\n");
            }
            std::fs::write(assets.join("Lid.asset"), text).unwrap();
        } else {
            std::fs::write(assets.join("Lid.fbx"), include_bytes!("fixtures/skinned.fbx")).unwrap();
        }
        std::fs::write(
            assets.join(format!("Lid.{extension}.meta")),
            "guid: lid00000\nModelImporter:\n  fileIDToRecycleName:\n    4300000: Lid\n",
        )
        .unwrap();
        let mut prefab = PREFAB.replace("--- !u!23 &4\nMeshRenderer:", "--- !u!137 &4\nSkinnedMeshRenderer:\n  m_Mesh: {fileID: 4300000, guid: lid00000, type: 3}\n  m_Bones: [{fileID: 12}, {fileID: 22}]\n  m_RootBone: {fileID: 12}");
        if !asset {
            prefab = prefab.replace("m_Bones: [{fileID: 12}, {fileID: 22}]", "m_Bones: [{fileID: 22}]");
        }
        if partial {
            prefab = prefab.replace(
                "m_Bones: [{fileID: 12}, {fileID: 22}]",
                "m_Bones: [{fileID: 12}, {fileID: 999}]",
            );
        }
        for (id, name, parent, z) in [(11, "SceneRoot", 2, 7), (21, "SceneChild", 12, 3)] {
            prefab.push_str(&format!("--- !u!1 &{id}\nGameObject:\n  m_Name: {name}\n  m_IsActive: 1\n--- !u!4 &{}\nTransform:\n  m_GameObject: {{fileID: {id}}}\n  m_Father: {{fileID: {parent}}}\n  m_LocalPosition: {{x: 0, y: 0, z: {z}}}\n", id + 1));
        }
        let project = Project::open(&dir).unwrap();
        let hierarchy = Hierarchy::build(&UnityFile::parse(&prefab).unwrap(), &project).unwrap();
        let out = dir.join("out");
        let mut importer = Importer::new(&project, &out, "");
        let scene = importer.scene(&hierarchy).unwrap();
        let path = scene["entities"][0]["components"]["Model"]["mesh"].as_str().unwrap();
        let bytes = std::fs::read(out.join(path)).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
        let blob = gltf.blob.as_deref().unwrap();
        let skin = gltf.skins().next().unwrap();
        let joints: Vec<_> = skin.joints().collect();
        assert_eq!(joints.len(), if asset { 2 } else { 1 });
        let root = gltf.nodes().find(|n| n.name() == Some("SceneRoot")).unwrap();
        assert_eq!(skin.skeleton().unwrap().index(), root.index());
        let child = root.children().next().unwrap();
        if !partial {
            assert_eq!(child.name(), Some("SceneChild"));
        }
        assert_eq!(root.transform().matrix()[3][2], -7.0);
        if !partial {
            assert_eq!(child.transform().matrix()[3][2], -3.0);
        }
        assert_eq!(joints[0].name(), Some(if asset { "SceneRoot" } else { "SceneChild" }));
        if !asset {
            assert!(
                gltf.animations()
                    .next()
                    .unwrap()
                    .channels()
                    .any(|c| c.target().node().index() == joints[0].index())
            );
        }
        if !asset {
            let channel = gltf
                .animations()
                .next()
                .unwrap()
                .channels()
                .find(|c| {
                    c.target().node().index() == joints[0].index()
                        && c.target().property() == gltf::animation::Property::Translation
                })
                .unwrap();
            let gltf::animation::util::ReadOutputs::Translations(mut values) =
                channel.reader(|_| Some(blob)).read_outputs().unwrap()
            else {
                panic!("translation")
            };
            assert_eq!(values.next().unwrap(), joints[0].transform().decomposed().0);
        }
        let binds: Vec<_> = skin
            .reader(|_| Some(blob))
            .read_inverse_bind_matrices()
            .unwrap()
            .collect();
        assert_eq!(binds[0][3][2], 2.0);
        if asset {
            let primitive = gltf.meshes().next().unwrap().primitives().next().unwrap();
            let reader = primitive.reader(|_| Some(blob));
            assert!(reader.read_joints(0).unwrap().into_u16().all(|j| j == [1, 0, 0, 0]));
            assert!(
                reader
                    .read_weights(0)
                    .unwrap()
                    .into_f32()
                    .all(|w| w == [0.75, 0.25, 0.0, 0.0])
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn blend_shapes_become_named_morph_targets() {
    let glb = import_glb(
        "shape",
        include_bytes!("fixtures/lid_shape.fbx"),
        "    bakeAxisConversion: 1\n",
    );
    let mesh = read_glb(&glb);
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let doc: Json = serde_json::from_slice(&glb[20..20 + len]).unwrap();
    let bin = &glb[20 + len + 8..];
    let gltf_mesh = &doc["meshes"][0];
    assert_eq!(gltf_mesh["extras"]["targetNames"], serde_json::json!(["Raise"]));
    assert_eq!(gltf_mesh["weights"], serde_json::json!([0.25]));
    let target = &gltf_mesh["primitives"][0]["targets"][0];
    let a = &doc["accessors"][target["POSITION"].as_u64().unwrap() as usize];
    assert!(
        a["min"].is_array() && a["max"].is_array(),
        "glTF wants bounds on POSITION"
    );
    let view = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let at = view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let offsets: Vec<[f32; 3]> = bin[at..at + view["byteLength"].as_u64().unwrap() as usize]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| f32::from_le_bytes(*w))
        .collect::<Vec<_>>()
        .as_chunks::<3>()
        .0
        .to_vec();
    assert_eq!(offsets.len(), mesh.positions.len());
    // Z-up turned Y-up: the far edge (y = 3 m in the file, z = -3 in
    // glTF after the turn and mirror) goes 1 m up; the near edge stays.
    let mut far_corners = 0;
    for (p, d) in mesh.positions.iter().zip(&offsets) {
        let far = (p[2].abs() - 3.0).abs() < 1e-4;
        far_corners += usize::from(far);
        let want = if far { [0.0, 1.0, 0.0] } else { [0.0; 3] };
        assert!(close(*d, want), "corner {p:?} moves {d:?}, want {want:?}");
    }
    assert_eq!(far_corners, 2, "{:?}", mesh.positions);
    assert!(target["NORMAL"].is_u64());
}

#[test]
fn fbx_inbetween_frames_survive_glb_export() {
    // Windows checkouts may turn the fixture's line endings into CRLF.
    let source = include_str!("fixtures/lid_shape.fbx")
        .replace("\r\n", "\n")
        .replace("FullWeights: *1 {\n\t\t\ta: 100", "FullWeights: *2 {\n\t\t\ta: 50,100")
        .replace("\tDeformer: 4000", "\tGeometry: 3001, \"Geometry::Half\", \"Shape\" {\n\t\tVersion: 100\n\t\tIndexes: *2 { a: 2,3 }\n\t\tVertices: *6 { a: 0,0,0.2,0,0,0.2 }\n\t}\n\tDeformer: 4000")
        .replace("C: \"OO\",3000,5000", "C: \"OO\",3001,5000\n\tC: \"OO\",3000,5000");
    let glb = import_glb("inbetween", source.as_bytes(), "    bakeAxisConversion: 1\n");
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let doc: Json = serde_json::from_slice(&glb[20..20 + len]).unwrap();
    let frames = doc["meshes"][0]["extras"]["ponkoMorphFrames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["weight"], 0.5);
    assert_eq!(frames[1]["weight"], 1.0);
    assert_eq!(frames[0]["channel"], 1);
    assert_eq!(
        doc["meshes"][0]["primitives"][0]["targets"].as_array().unwrap().len(),
        2
    );
}

/// A 1 x 1 RGBA PNG.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196,
    137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 223, 224, 240, 31, 0, 7, 0, 2, 191, 43, 215, 199, 226, 0, 0,
    0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// The `.meta` of `textured.fbx`: its mesh and both materials by id.
const TEXTURED_META: &str = "guid: plate000
ModelImporter:
  internalIDToNameTable:
  - first:
      43: 4300000
    second: Plate
  - first:
      21: 2100000
    second: Paint
  - first:
      21: 2100002
    second: Tint
";

/// Imports a prefab drawing `textured.fbx` (at `Assets/Props/Plate.fbx`,
/// its `.meta` being `meta`) with `material`, with the texture at
/// `Assets/Props/Textures/paint.png` and `files` written too; the glTF
/// JSON of the `.glb` written and the report.
fn import_textured(name: &str, meta: &str, material: &str, files: &[(&str, &str)]) -> (Json, Report) {
    let dir = std::env::temp_dir().join(format!("ponko_unity_textured_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let props = dir.join("Assets/Props");
    std::fs::create_dir_all(props.join("Textures")).unwrap();
    std::fs::write(props.join("Plate.fbx"), include_bytes!("fixtures/textured.fbx")).unwrap();
    std::fs::write(props.join("Plate.fbx.meta"), meta).unwrap();
    std::fs::write(props.join("Textures/paint.png"), PNG).unwrap();
    std::fs::write(props.join("Textures/paint.png.meta"), "guid: paint000\n").unwrap();
    for (path, text) in files {
        std::fs::write(dir.join(path), text).unwrap();
    }
    let prefab = PREFAB
        .replace("guid: lid00000", "guid: plate000")
        .replace("m_Materials: []", &format!("m_Materials:\n  - {material}"));
    let project = Project::open(&dir).unwrap();
    let hierarchy = Hierarchy::build(&UnityFile::parse(&prefab).unwrap(), &project).unwrap();
    let out = dir.join("out");
    let mut importer = Importer::new(&project, &out, "");
    let scene = importer.scene(&hierarchy).unwrap();
    let report = importer.finish().unwrap();
    let path = scene["entities"][0]["components"]["Model"]["mesh"].as_str().unwrap();
    let bytes = std::fs::read(out.join(path)).unwrap();
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let doc = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (doc, report)
}

#[test]
fn embedded_model_materials_find_their_texture_by_name() {
    let (doc, report) = import_textured(
        "paint",
        TEXTURED_META,
        "{fileID: 2100000, guid: plate000, type: 3}",
        &[],
    );
    let material = &doc["materials"][0];
    assert_eq!(material["name"], "Paint");
    // Saved as ../art/paint.png, which isn't there: found in Textures/.
    let texture = material["pbrMetallicRoughness"]["baseColorTexture"]["index"]
        .as_u64()
        .unwrap();
    let image = doc["textures"][texture as usize]["source"].as_u64().unwrap();
    assert_eq!(doc["images"][image as usize]["uri"], "textures/paint000.png");
    // With a texture, the color is white times the diffuse factor.
    assert_eq!(
        material["pbrMetallicRoughness"]["baseColorFactor"],
        serde_json::json!([1.0, 1.0, 1.0, 1.0])
    );
    assert_eq!(material["pbrMetallicRoughness"]["roughnessFactor"], 1.0, "no shininess");
    assert!(material.get("alphaMode").is_none());
    assert_eq!(report.textures, 1);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn embedded_model_materials_keep_color_opacity_and_shininess() {
    let (doc, _) = import_textured("tint", TEXTURED_META, "{fileID: 2100002, guid: plate000, type: 3}", &[]);
    let material = &doc["materials"][0];
    assert_eq!(material["name"], "Tint");
    let pbr = &material["pbrMetallicRoughness"];
    assert!(pbr.get("baseColorTexture").is_none());
    // The file's linear color goes to sRGB in Unity and back to linear in
    // glTF: the same 0.214 again. Opacity 0.5 makes it transparent.
    let color: Vec<f64> = pbr["baseColorFactor"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_f64().unwrap())
        .collect();
    assert!((color[0] - 0.214041).abs() < 1e-3, "{color:?}");
    assert!((color[3] - 0.5).abs() < 1e-6);
    assert_eq!(material["alphaMode"], "BLEND");
    // Smoothness is the square root of shininess / 100.
    assert!((pbr["roughnessFactor"].as_f64().unwrap() - 0.5).abs() < 1e-6);
}

#[test]
fn embedded_model_materials_use_hashed_ids_and_remaps() {
    // Unity 2019.3+ leaves the table empty and hashes the names; the
    // `.meta` may map a material to a material asset instead.
    let meta = "guid: plate000
ModelImporter:
  externalObjects:
  - first:
      type: UnityEngine:Material
      assembly: UnityEngine.CoreModule
      name: Tint
    second: {fileID: 2100000, guid: redmat00, type: 2}
  internalIDToNameTable:
  - first:
      43: 4300000
    second: Plate
";
    let red = "%YAML 1.1\n--- !u!21 &2100000\nMaterial:\n  m_Name: Red\n  m_SavedProperties:\n    m_Colors:\n    - _BaseColor: {r: 1, g: 0, b: 0, a: 1}\n";
    // xxHash64 of "Type:Material->Tint0": the scheme of meshes' ids.
    let tint = "{fileID: 3047379822089669152, guid: plate000, type: 3}";
    let files = [("Assets/Red.mat", red), ("Assets/Red.mat.meta", "guid: redmat00\n")];
    let (doc, _) = import_textured("remap", meta, tint, &files);
    assert_eq!(doc["materials"][0]["name"], "Red");
    let (doc, _) = import_textured("hashed", &meta.replace("name: Tint", "name: Other"), tint, &files);
    assert_eq!(doc["materials"][0]["name"], "Tint");
    // Materials import turned off: drawn plain.
    let off = format!("{meta}  materials:\n    materialImportMode: 0\n").replace("name: Tint", "name: Other");
    let (doc, report) = import_textured("off", &off, tint, &files);
    assert_eq!(doc["materials"][0]["name"], "plain");
    assert_eq!(report.skipped["a material that can't be read (drawn plain)"], 1);
}

#[test]
fn model_clip_splits_come_from_the_animations_block() {
    let dir = std::env::temp_dir().join(format!("ponko_unity_clip_splits_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let assets = dir.join("Assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("Lid.fbx"), include_bytes!("fixtures/skinned.fbx")).unwrap();
    // Where Unity writes them, with other settings around.
    std::fs::write(
        assets.join("Lid.fbx.meta"),
        "guid: lid00000\nModelImporter:\n  fileIDToRecycleName:\n    4300000: Lid\n  animations:\n    legacyGenerateAnimations: 4\n    clipAnimations:\n    - serializedVersion: 16\n      name: Open\n      takeName: Move\n      firstFrame: 6\n      lastFrame: 18\n      loopTime: 1\n      loopBlend: 0\n    - serializedVersion: 16\n      name: Shut\n      takeName: Move\n      firstFrame: 18\n      lastFrame: 24\n      loopTime: 0\n    isReadable: 0\n  meshes:\n    globalScale: 1\n",
    )
    .unwrap();
    let prefab = PREFAB.replace(
        "--- !u!23 &4\nMeshRenderer:",
        "--- !u!137 &4\nSkinnedMeshRenderer:\n  m_Mesh: {fileID: 4300000, guid: lid00000, type: 3}",
    );
    let project = Project::open(&dir).unwrap();
    let hierarchy = Hierarchy::build(&UnityFile::parse(&prefab).unwrap(), &project).unwrap();
    let out = dir.join("out");
    let mut importer = Importer::new(&project, &out, "");
    let scene = importer.scene(&hierarchy).unwrap();
    let path = scene["entities"][0]["components"]["Model"]["mesh"].as_str().unwrap();
    let bytes = std::fs::read(out.join(path)).unwrap();
    let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let doc: Json = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
    let clips: Vec<_> = gltf
        .animations()
        .map(|a| {
            let time = a
                .channels()
                .map(|c| {
                    let reader = c.reader(|_| gltf.blob.as_deref());
                    reader.read_inputs().unwrap().fold(0.0f32, f32::max)
                })
                .fold(0.0f32, f32::max);
            let looping = doc["animations"][a.index()]["extras"]["loop"] == true;
            (a.name().unwrap().to_owned(), looping, time)
        })
        .collect();
    // 24 frames a second: 12 and 6 frames long.
    assert_eq!(clips.len(), 2, "{clips:?}");
    assert_eq!((clips[0].0.as_str(), clips[0].1), ("Open", true));
    assert!((clips[0].2 - 0.5).abs() < 1e-5);
    assert_eq!((clips[1].0.as_str(), clips[1].1), ("Shut", false));
    assert!((clips[1].2 - 0.25).abs() < 1e-5);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn standalone_clip_paths_resolve_from_the_animators_object() {
    let dir = std::env::temp_dir().join(format!("ponko_unity_animator_root_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let assets = dir.join("Assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("Lid.fbx"), include_bytes!("fixtures/skinned.fbx")).unwrap();
    std::fs::write(
        assets.join("Lid.fbx.meta"),
        "guid: lid00000\nModelImporter:\n  fileIDToRecycleName:\n    4300000: Lid\n",
    )
    .unwrap();
    // Character (Animator) > Body (the renderer), and Character >
    // Armature > Hips (the root bone).
    let mut prefab = String::from("%YAML 1.1\n");
    for (id, name, parent) in [
        (1, "Character", 0),
        (11, "Body", 2),
        (21, "Armature", 2),
        (31, "Hips", 22),
    ] {
        prefab.push_str(&format!("--- !u!1 &{id}\nGameObject:\n  m_Name: {name}\n  m_IsActive: 1\n--- !u!4 &{}\nTransform:\n  m_GameObject: {{fileID: {id}}}\n  m_Father: {{fileID: {parent}}}\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  m_LocalPosition: {{x: 0, y: 0, z: 0}}\n  m_LocalScale: {{x: 1, y: 1, z: 1}}\n", id + 1));
    }
    prefab.push_str("--- !u!95 &3\nAnimator:\n  m_GameObject: {fileID: 1}\n  m_Enabled: 1\n");
    prefab.push_str("--- !u!137 &13\nSkinnedMeshRenderer:\n  m_GameObject: {fileID: 11}\n  m_Enabled: 1\n  m_Mesh: {fileID: 4300000, guid: lid00000, type: 3}\n  m_Bones: [{fileID: 32}]\n  m_RootBone: {fileID: 32}\n  m_Materials: []\n");
    let clip = AnimationClip::parse("--- !u!74 &1\nAnimationClip:\n  m_Name: Bob\n  m_SampleRate: 2\n  m_AnimationClipSettings: {m_StartTime: 0, m_StopTime: 1}\n  m_PositionCurves:\n  - path: Armature/Hips\n    curve:\n      m_Curve:\n      - {time: 0, value: {x: 0, y: 0, z: 0}}\n      - {time: 1, value: {x: 0, y: 2, z: 0}}\n  - path: Armature\n    curve:\n      m_Curve:\n      - {time: 0, value: {x: 0, y: 0, z: 0}}\n      - {time: 1, value: {x: 3, y: 0, z: 0}}\n").unwrap();
    let project = Project::open(&dir).unwrap();
    let hierarchy = Hierarchy::build(&UnityFile::parse(&prefab).unwrap(), &project).unwrap();
    let out = dir.join("out");
    let mut importer = Importer::new(&project, &out, "");
    importer.animation_clips([clip]);
    let scene = importer.scene(&hierarchy).unwrap();
    let report = importer.finish().unwrap();
    assert!(
        !report.skipped.keys().any(|k| k.contains("unresolved")),
        "{:?}",
        report.skipped
    );
    let path = scene["entities"][0]["components"]["Model"]["mesh"].as_str().unwrap();
    let bytes = std::fs::read(out.join(path)).unwrap();
    let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
    let bob = gltf.animations().find(|a| a.name() == Some("Bob")).unwrap();
    let mut targets: Vec<_> = bob
        .channels()
        .filter(|c| c.target().property() == gltf::animation::Property::Translation)
        .map(|c| {
            let reader = c.reader(|_| gltf.blob.as_deref());
            let gltf::animation::util::ReadOutputs::Translations(values) = reader.read_outputs().unwrap() else {
                panic!("translation")
            };
            (c.target().node().name().unwrap().to_owned(), values.last().unwrap())
        })
        .collect();
    targets.sort_by(|a, b| a.0.cmp(&b.0));
    // Both bind, Armature too, though it's above the root bone. (Ponko's
    // mirror along Z leaves X and Y as they are.)
    assert_eq!(targets.len(), 2, "{targets:?}");
    assert_eq!(targets[0].0, "Armature");
    assert!((targets[0].1[0] - 3.0).abs() < 1e-5, "{targets:?}");
    assert_eq!(targets[1].0, "Hips");
    assert!((targets[1].1[1] - 2.0).abs() < 1e-5, "{targets:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn fbx_submeshes_follow_first_material_usage_like_unity() {
    let bytes = import_glb(
        "material_order",
        include_bytes!("fixtures/material_order.fbx"),
        "    useFileScale: 1\n",
    );
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let doc: Json = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
    let bin = &bytes[28 + len..];
    let primitives = doc["meshes"][0]["primitives"].as_array().unwrap();
    assert_eq!(primitives.len(), 3);
    for (primitive, expected_x) in primitives.iter().zip([-10.0f32, -20.0, -30.0]) {
        let index_accessor = &doc["accessors"][primitive["indices"].as_u64().unwrap() as usize];
        let index_view = &doc["bufferViews"][index_accessor["bufferView"].as_u64().unwrap() as usize];
        let offset = index_view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let index = u32::from_le_bytes(bin[offset..offset + 4].try_into().unwrap()) as usize;
        let accessor = &doc["accessors"][primitive["attributes"]["POSITION"].as_u64().unwrap() as usize];
        let view = &doc["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
        let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize + index * 12;
        let x = f32::from_le_bytes(bin[offset..offset + 4].try_into().unwrap());
        assert_eq!(
            x, expected_x,
            "renderer slots must follow first face usage, not FBX slot number"
        );
    }
}
