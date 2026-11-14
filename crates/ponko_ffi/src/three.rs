//! 3D from C: `PonkoTransform3D` and `PonkoModel` components (plain
//! values, so query columns too), meshes and materials, the 3D camera and
//! the sun.
use std::ffi::c_char;

use ponko::__ffi;
use ponko::d3::Camera as Camera3D;
use ponko::d3::Decal as Decal3D;
use ponko::d3::PointLight as PointLight3D;
use ponko::d3::ReflectionProbe as ReflectionProbe3D;
use ponko::d3::SpotLight as SpotLight3D;
use ponko::d3::Transform as Transform3D;
use ponko::prelude::{Color, Quat, Vec2, Vec3, Vec4};
use ponko::render::AmbientOcclusion;
use ponko::render::AntiAliasing;
use ponko::render::Blend;
use ponko::render::Compute;
use ponko::render::ComputePass;
use ponko::render::Effect;
use ponko::render::GlobalIllumination;
use ponko::render::GpuBuffer;
use ponko::render::GraphicsQuality;
use ponko::render::LodGroup;
use ponko::render::LodLevel;
use ponko::render::LodPick;
use ponko::render::LodRenderer;
use ponko::render::LodShadow;
use ponko::render::Material;
use ponko::render::MaterialDesc;
use ponko::render::Mesh;
use ponko::render::MeshData;
use ponko::render::Model;
use ponko::render::ReceivesDecals;
use ponko::render::SceneSettings;
use ponko::render::ScreenEffect;
use ponko::render::Shader;
use ponko::render::ShadowFilter;
use ponko::render::Sky;
use ponko::render::TargetCamera;
use ponko::render::Tonemap;
use ponko::terrain::TerrainMaterialDesc;
use ponko::terrain::TerrainTile;
use ponko::terrain::TextureArray;
use ponko::terrain::TextureKind;

use crate::ctx::{ctx_access, out, text};
use crate::types::*;
use crate::{PonkoCtx, PonkoStatus, PonkoWorld, boundary, builtin, invalid, invalid_owned};

/// A point or direction in 3D: right-handed, +Y up.
/// ```
/// let up = ponko_ffi::PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 };
/// assert_eq!(up.y, 1.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoVec3 {
    /// Right.
    pub x: f32,
    /// Up.
    pub y: f32,
    /// Towards the viewer (the default camera looks along -Z).
    pub z: f32,
}

/// A rotation (unit quaternion). `{0, 0, 0, 1}` is none; make others with
/// `ponko_quat_from_axis_angle`.
/// ```
/// let none = ponko_ffi::ponko_quat_identity();
/// assert_eq!(none.w, 1.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoQuat {
    /// Vector part, x.
    pub x: f32,
    /// Vector part, y.
    pub y: f32,
    /// Vector part, z.
    pub z: f32,
    /// Scalar part.
    pub w: f32,
}

/// The built-in `Transform3D` component, same bytes as in Rust (48, the
/// last 8 unused): a plain value and a query column. Start from
/// `ponko_transform3d_at`.
/// ```
/// let t = ponko_ffi::ponko_transform3d_at(Default::default());
/// assert_eq!((t.scale.x, t.rotation.w), (1.0, 1.0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTransform3D {
    /// Which way it faces.
    pub rotation: PonkoQuat,
    /// Where it is, in world units.
    pub position: PonkoVec3,
    /// Size along its own axes.
    pub scale: PonkoVec3,
    /// Unused; keeps the Rust size.
    pub reserved: [f32; 2],
}

/// Entity PointLight3D: position from its transform; spots point along local -Z.
/// Angles are half angles in radians. Range ignores transform scale.
/// ```
/// use ponko_ffi::*;
/// assert!(std::mem::size_of::<PonkoPointLight3D>() >= 24);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoPointLight3D {
    /// sRGB color; alpha ignored.
    pub color: PonkoColor,
    /// Same as Rust `PointLight3D::intensity`.
    pub intensity: f32,
    /// Same as Rust `PointLight3D::range`.
    pub range: f32,
    /// Cast shadows (Rust `PointLight3D::shadows`): six of the frame's
    /// `ponko::d3::PointLight::MAX_SHADOW_VIEWS` shadow views, nearest lights first.
    pub shadows: bool,
    /// Each shadow view's size in the shadow atlas, in texels (Rust
    /// `PointLight3D::shadow_resolution`): 0 sizes it by how big the light
    /// looks; otherwise 64 to 2048, rounded down to a power of two.
    pub shadow_resolution: u32,
}

/// Entity SpotLight3D: position from its transform; spots point along local -Z.
/// Angles are half angles in radians. Range ignores transform scale.
/// ```
/// use ponko_ffi::*;
/// assert!(std::mem::size_of::<PonkoSpotLight3D>() >= 24);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoSpotLight3D {
    /// sRGB color; alpha ignored.
    pub color: PonkoColor,
    /// Same as Rust `SpotLight3D::intensity`.
    pub intensity: f32,
    /// Same as Rust `SpotLight3D::range`.
    pub range: f32,
    /// Same as Rust `SpotLight3D::inner_angle`.
    pub inner_angle: f32,
    /// Same as Rust `SpotLight3D::outer_angle`.
    pub outer_angle: f32,
    /// Cast shadows (Rust `SpotLight3D::shadows`): one shadow view.
    pub shadows: bool,
    /// Its shadow view's size in the atlas (Rust
    /// `SpotLight3D::shadow_resolution`); 0: automatic.
    pub shadow_resolution: u32,
}

/// A mesh on the GPU (`ponko_ctx_create_mesh`, `ponko_ctx_load_mesh`).
/// ```
/// let m = ponko_ffi::PonkoMesh { id: 0 };
/// assert_eq!(m.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoMesh {
    /// Engine-internal id.
    pub id: u32,
}

/// `PonkoMesh` id of no mesh (Rust `Mesh::NONE`): a model showing it draws
/// nothing, and no mesh is ever made with it. The same as `PONKO_HANDLE_NONE`.
/// ```
/// assert_eq!(ponko_ffi::PONKO_MESH_NONE, u32::MAX);
/// ```
pub const PONKO_MESH_NONE: u32 = u32::MAX;

/// A material (`ponko_ctx_create_material`).
/// ```
/// let m = ponko_ffi::PonkoMaterial { id: 0 };
/// assert_eq!(m.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoMaterial {
    /// Engine-internal id.
    pub id: u32,
}

/// The built-in `Model` component: draws `mesh` where the entity's
/// `PonkoTransform3D` is, its materials' colors times `color`. A plain
/// value and a query column. Start from `ponko_model`.
/// ```
/// let m = ponko_ffi::ponko_model(ponko_ffi::PonkoMesh { id: 3 });
/// assert_eq!((m.mesh.id, m.color.r), (3, 1.0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoModel {
    /// What to draw.
    pub mesh: PonkoMesh,
    /// Multiplied with the materials' colors (white: no change).
    pub color: PonkoColor,
}

/// The built-in `TerrainTile` component: a square of heightfield terrain
/// drawn more coarsely further from the camera, spawned with a
/// `PonkoTransform3D` at its corner instead of a `PonkoModel`. Made with
/// `ponko_ctx_create_terrain_tile`.
/// ```
/// let t = ponko_ffi::PonkoTerrainTile { lods: [ponko_ffi::PonkoMesh { id: 0 }; 4], lod_distance: 256.0 };
/// assert_eq!(t.lods.len(), ponko_ffi::PONKO_TERRAIN_LODS);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTerrainTile {
    /// One mesh per detail level, finest first: every height, then every
    /// 2nd, 4th and 8th.
    pub lods: [PonkoMesh; PONKO_TERRAIN_LODS],
    /// Up to how far from the camera the finest mesh is drawn; each
    /// doubling of that drops one level.
    pub lod_distance: f32,
}

/// How many detail levels a `PonkoTerrainTile` has.
pub const PONKO_TERRAIN_LODS: usize = 4;
const _: () = assert!(PONKO_TERRAIN_LODS == ponko::terrain::TerrainTile::LODS);

/// A compute pass shader (`ponko_ctx_load_compute`, `ponko_ctx_create_compute`).
/// ```
/// let c = ponko_ffi::PonkoCompute { id: 0 };
/// assert_eq!(c.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoCompute {
    /// Engine-internal id.
    pub id: u32,
}

/// One compute pass run each frame (Rust `ComputePass`): its shader, the
/// render target it updates, and the material its custom values and
/// textures come from (the default material without one).
/// ```
/// let p = ponko_ffi::PonkoComputePass {
///     has_buffer: false, buffer: Default::default(),
///     compute: ponko_ffi::PonkoCompute { id: 0 },
///     target: ponko_ffi::PonkoTexture { id: 0, width: 8, height: 8 },
///     has_material: false,
///     material: Default::default(),
/// };
/// assert!(!p.has_material);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PonkoComputePass {
    /// Whether this pass updates `buffer` instead of `target`.
    pub has_buffer: bool,
    /// Storage buffer target when `has_buffer` is true.
    pub buffer: PonkoGpuBuffer,
    /// The shader.
    pub compute: PonkoCompute,
    /// What it updates: from `ponko_ctx_create_render_target`.
    pub target: PonkoTexture,
    /// Whether `material` is used.
    pub has_material: bool,
    /// Where its values and textures come from.
    pub material: PonkoMaterial,
}

/// A screen effect shader (`ponko_ctx_load_effect`, `ponko_ctx_create_effect`).
/// ```
/// let e = ponko_ffi::PonkoEffect { id: 0 };
/// assert_eq!(e.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoEffect {
    /// Engine-internal id.
    pub id: u32,
}

/// One screen effect run over the lit 3D frame (Rust `ScreenEffect`):
/// its shader, and the material its custom values and textures come from
/// (the default material without one).
/// ```
/// let s = ponko_ffi::PonkoScreenEffect { effect: ponko_ffi::PonkoEffect { id: 0 }, has_material: false, material: Default::default() };
/// assert!(!s.has_material);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoScreenEffect {
    /// The shader.
    pub effect: PonkoEffect,
    /// Whether `material` is used.
    pub has_material: bool,
    /// Where its values and textures come from.
    pub material: PonkoMaterial,
}

/// A camera drawing the 3D world into a render target each frame (Rust
/// `TargetCamera`). Only what's on the front of the clip plane (`x, y, z`
/// the normal, `w` the offset: kept where `normal . p + w >= 0`) is drawn,
/// when `has_clip_plane`.
/// ```
/// let t = ponko_ffi::PonkoTargetCamera {
///     camera: ponko_ffi::PonkoCamera3D { position: Default::default(), rotation: ponko_ffi::ponko_quat_identity(), fov_y: 1.0, near: 0.1, far: 100.0 },
///     target: ponko_ffi::PonkoTexture { id: 0, width: 64, height: 64 },
///     has_clip_plane: false,
///     clip_plane: [0.0; 4],
/// };
/// assert!(!t.has_clip_plane);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTargetCamera {
    /// Where it looks from; its aspect ratio follows the target's.
    pub camera: PonkoCamera3D,
    /// From `ponko_ctx_create_render_target`.
    pub target: PonkoTexture,
    /// Whether `clip_plane` is used.
    pub has_clip_plane: bool,
    /// The plane: normal (x, y, z) and offset (w).
    pub clip_plane: [f32; 4],
}

/// A material shader (`ponko_ctx_load_shader`, `ponko_ctx_create_shader`).
/// ```
/// let s = ponko_ffi::PonkoShader { id: 0 };
/// assert_eq!(s.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoShader {
    /// Engine-internal id.
    pub id: u32,
}

/// What a material is made from (Rust `MaterialDesc`). Start from
/// `ponko_material_desc`. Each texture is used when its `has_` flag is set.
/// `blend`: 0 opaque, 1 transparent, 2 cutout.
/// ```
/// let d = ponko_ffi::ponko_material_desc(ponko_ffi::PonkoColor { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
/// assert_eq!((d.roughness, d.metallic, d.has_texture, d.normal_strength), (0.6, 0.0, false, 1.0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoMaterialDesc {
    /// Base color, multiplied with the texture.
    pub color: PonkoColor,
    /// Whether `texture` is used.
    pub has_texture: bool,
    /// An image on the surface, by the mesh's texture coordinates.
    pub texture: PonkoTexture,
    /// 0 mirror-smooth to 1 fully rough.
    pub roughness: f32,
    /// 0 for most things, 1 for bare metal.
    pub metallic: f32,
    /// Light it gives off itself.
    pub emissive: PonkoColor,
    /// Whether `normal_map` is used.
    pub has_normal_map: bool,
    /// Surface detail as tangent-space normals (OpenGL style, green up).
    pub normal_map: PonkoTexture,
    /// How strong the normal map is (1: as painted).
    pub normal_strength: f32,
    /// Whether `orm_map` is used.
    pub has_orm_map: bool,
    /// Occlusion (red), roughness (green) and metallic (blue), as glTF.
    pub orm_map: PonkoTexture,
    /// Whether `emissive_map` is used.
    pub has_emissive_map: bool,
    /// Multiplies `emissive`.
    pub emissive_map: PonkoTexture,
    /// 0 opaque, 1 transparent, 2 cut out (pixels with alpha below 0.5).
    pub blend: u32,
    /// Draws the back of each triangle too.
    pub double_sided: bool,
    /// Whether `shader` is used (else the built-in one).
    pub has_shader: bool,
    /// A custom shader.
    pub shader: PonkoShader,
    /// Values for a custom shader: `material.custom[0..4]` there, four floats each.
    pub custom: [f32; 16],
    /// Whether each of `custom_textures` is used.
    pub has_custom_textures: [bool; 2],
    /// `custom_texture0` and `custom_texture1` in a custom shader.
    pub custom_textures: [PonkoTexture; 2],
    /// Whether each of `custom_arrays` is used.
    pub has_custom_arrays: [bool; 3],
    /// `custom_array0` to `custom_array2` in a custom shader.
    pub custom_arrays: [PonkoTextureArray; 3],
}

/// Several images of the same size in one texture (Rust `TextureArray`),
/// from `ponko_ctx_create_texture_array` or `ponko_ctx_load_texture_array`.
/// ```
/// let a = ponko_ffi::PonkoTextureArray { id: 0, width: 64, height: 64, layers: 3 };
/// assert_eq!(a.layers, 3);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PonkoTextureArray {
    /// Engine-internal id.
    pub id: u32,
    /// Width of each image, in pixels.
    pub width: u32,
    /// Height of each image, in pixels.
    pub height: u32,
    /// How many images it holds.
    pub layers: u32,
}

/// What a terrain material is made from (Rust `TerrainMaterialDesc`):
/// ground layers and a weight map. Start from `ponko_terrain_material_desc`.
/// ```
/// let a = ponko_ffi::PonkoTextureArray::default();
/// let d = ponko_ffi::ponko_terrain_material_desc(a, a, Default::default(), ponko_ffi::PonkoVec2 { x: 256.0, y: 256.0 });
/// assert_eq!(d.roughness, 0.9);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTerrainMaterialDesc {
    /// Each layer's color texture (`PONKO_TEXTURE_COLOR`), up to `PONKO_TERRAIN_LAYERS`.
    pub layers: PonkoTextureArray,
    /// Whether `normals` is used.
    pub has_normals: bool,
    /// The layers' normal maps, one each (`PONKO_TEXTURE_DATA`).
    pub normals: PonkoTextureArray,
    /// How strong the normal maps are.
    pub normal_strength: f32,
    /// How much of each layer shows where (`PONKO_TEXTURE_DATA`): the first
    /// image's RGBA are layers 0-3, the second's 4-7.
    pub weights: PonkoTextureArray,
    /// The world corner (x, z) under the weight map's top-left pixel.
    pub weights_corner: PonkoVec2,
    /// How much ground (x, z) the weight map covers.
    pub weights_size: PonkoVec2,
    /// How many world units each layer's texture covers before it repeats.
    pub layer_size: [f32; PONKO_TERRAIN_LAYERS],
    /// 0 is mirror-smooth, 1 fully rough.
    pub roughness: f32,
}

/// The most ground layers a terrain material blends.
pub const PONKO_TERRAIN_LAYERS: usize = 8;
const _: () = assert!(PONKO_TERRAIN_LAYERS == ponko::terrain::TerrainTile::LAYERS);
/// `ponko_ctx_create_texture_array` kinds (Rust `TextureKind`): colors, stored as sRGB.
pub const PONKO_TEXTURE_COLOR: u32 = 0;
/// Numbers read as stored: normal maps, masks, weights.
pub const PONKO_TEXTURE_DATA: u32 = 1;
const _: () =
    assert!(PONKO_TEXTURE_COLOR == TextureKind::Color as u32 && PONKO_TEXTURE_DATA == TextureKind::Data as u32);

/// The 3D camera (Rust `ctx.scene3d.camera`).
/// ```
/// let c = ponko_ffi::PonkoCamera3D { position: Default::default(), rotation: ponko_ffi::ponko_quat_identity(), fov_y: 1.0, near: 0.1, far: 100.0 };
/// assert!(c.near < c.far);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoCamera3D {
    /// Where it is.
    pub position: PonkoVec3,
    /// Which way it looks: its -Z axis.
    pub rotation: PonkoQuat,
    /// Vertical field of view, radians (0 to pi).
    pub fov_y: f32,
    /// Nearest distance drawn (positive).
    pub near: f32,
    /// Farthest distance drawn (more than `near`).
    pub far: f32,
}

/// The sun and the sky's light (Rust `ctx.scene3d.lighting`).
/// ```
/// let mut l = std::mem::MaybeUninit::<ponko_ffi::PonkoLighting>::uninit();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_lighting(std::ptr::null_mut(), l.as_mut_ptr()) } as u32, 1);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoLighting {
    /// Which way sunlight travels (from the sun, towards the ground).
    pub sun_direction: PonkoVec3,
    /// Color of sunlight.
    pub sun_color: PonkoColor,
    /// Brightness of sunlight.
    pub sun_intensity: f32,
    /// Light from above, on surfaces facing up.
    pub sky_color: PonkoColor,
    /// Light from below, on surfaces facing down.
    pub ground_color: PonkoColor,
    /// Brightness of sky and ground light.
    pub ambient_intensity: f32,
    /// Whether the sun casts shadows.
    pub shadows: bool,
    /// How far from the camera shadows reach.
    pub shadow_distance: f32,
    /// How many shadow maps share that distance, 1 to 4: more keep near
    /// shadows sharp, fewer are cheaper.
    pub shadow_cascades: u32,
    /// Width and height of each of the sun's shadow maps, 256 to 4096.
    pub shadow_resolution: u32,
    /// How many shadow views point and spot lights share, 0 (no shadows
    /// from them) to 24. Default 12.
    pub light_shadow_views: u32,
    /// Width and height of the largest point or spot light shadow view
    /// (what a light near the camera gets), 128 to 2048.
    pub light_shadow_resolution: u32,
    /// Width and height of the atlas the light shadow views are packed
    /// into: a power of two from 1024 to 8192. Default 4096.
    pub light_shadow_atlas: u32,
    /// How often the far shadow cascades are drawn again, in frames, 1 to 8
    /// (1: every frame). Default 2.
    pub shadow_refresh: u32,
    /// How soft the sun's shadow edges are, 1 (one lookup per pixel, the
    /// cheapest) to 3 (nine, the softest). Default 3.
    pub shadow_filter: u32,
    /// Whether `equator_color` is used: the ambient light then goes from
    /// `ground_color` below through `equator_color` on the horizon to
    /// `sky_color` above. Off by default. Only without an atmosphere.
    pub has_equator: bool,
    /// Light from the horizon, on surfaces facing sideways (with `has_equator`).
    pub equator_color: PonkoColor,
    /// Light bounced off surfaces (global illumination) from a grid of
    /// probes around the camera. Off by default. Needs compute shaders and
    /// 18 texture slots; elsewhere it's skipped.
    pub gi_enabled: bool,
    /// The probe grid's size, in world units. Default 32 x 16 x 32.
    pub gi_extent: PonkoVec3,
    /// How far apart the probes are. Default 2.
    pub gi_spacing: f32,
    /// How many probes are updated each frame, 1 to 16. Default 4.
    pub gi_probes_per_frame: u32,
    /// How bright the bounced light is (1: physically based). Default 1.
    pub gi_intensity: f32,
}

/// The sky behind the 3D world and the fog in front of it (Rust
/// `ctx.scene3d.sky`, `Sky`).
/// ```
/// let mut s = std::mem::MaybeUninit::<ponko_ffi::PonkoSky>::uninit();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_sky(std::ptr::null_mut(), s.as_mut_ptr()) } as u32, 1);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoSky {
    /// A physical sky lit by the sun, which also lights the scene.
    pub atmosphere: bool,
    /// Dust and water in the air, 0 up.
    pub haze: f32,
    /// The sun's size in the sky, in degrees across.
    pub sun_size: f32,
    /// How bright the stars are at night.
    pub stars: f32,
    /// Light at night.
    pub night_light: PonkoColor,
    /// Fog thickness near the ground, per world unit; 0 is none.
    pub fog_density: f32,
    /// The height the fog lies at.
    pub fog_height: f32,
    /// How quickly the fog thins out above it, in world units.
    pub fog_falloff: f32,
    /// The fog's color without an atmosphere.
    pub fog_color: PonkoColor,
    /// Distance fog: none closer than this, in world units.
    pub fog_start: f32,
    /// Distance fog: all fog past this; 0 (or not past the start) is none.
    pub fog_end: f32,
    /// How much sunlight the fog scatters toward the camera; 0 is none.
    pub fog_sun_glow: f32,
    /// How tight that glow is around the sun, 0 to under 1.
    pub fog_sun_focus: f32,
    /// Light shafts: how much shadows cut the fog's sun glow, 0 (off) to 1.
    pub light_shafts: f32,
}

/// Which way sunlight travels at `hour` o'clock, for a day and night cycle
/// (Rust `Sky::sun_direction_at`): rising in the east (+X) at 6, highest
/// at noon, setting in the west at 18.
/// ```
/// assert!(ponko_ffi::ponko_sun_direction_at(12.0).y < -0.8);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_sun_direction_at(hour: f32) -> PonkoVec3 {
    to_c(Sky::sun_direction_at(hour))
}

/// What happens to the lit 3D frame on its way to the screen (Rust
/// `ctx.scene3d.post`). `tonemap`: 0 ACES (default), 1 AgX, 2 Khronos PBR Neutral,
/// 3 none. `lut` is used when `has_lut`.
/// ```
/// let mut p = std::mem::MaybeUninit::<ponko_ffi::PonkoPost>::uninit();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_post(std::ptr::null_mut(), p.as_mut_ptr()) } as u32, 1);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoPost {
    /// The 3D scene's resolution relative to the window, 0.25 to 2: it is
    /// drawn at that scale, then scaled to the window under the sprites and UI.
    pub render_scale: f32,
    /// Sharpening of the scaled image, 0 to 1.
    pub sharpen: f32,
    /// Post AA: 0 none, 1 FXAA, 2 SMAA 1x, 3 temporal (TAA; WebGL2 falls
    /// back to FXAA).
    pub anti_aliasing: u32,
    /// Enable GTAO on ambient light only (unsupported backends ignore it).
    pub ao_enabled: bool,
    /// GTAO world-space radius, above zero.
    pub ao_radius: f32,
    /// GTAO strength, nonnegative.
    pub ao_intensity: f32,
    /// GTAO angular slices, 1 to 8.
    pub ao_slices: u32,
    /// GTAO horizon steps per side, 1 to 16.
    pub ao_steps: u32,
    /// Half-resolution GTAO with bilateral upsampling; contacts stay full resolution.
    pub ao_half_resolution: bool,
    /// Camera-reprojected GTAO accumulation (independent of TAA).
    pub ao_temporal: bool,
    /// Brightness in stops: +1 doubles the light.
    pub exposure: f32,
    /// How light maps to the screen (see above).
    pub tonemap: u32,
    /// How strongly bright light glows; 0 is off.
    pub bloom: f32,
    /// How bright light must be to glow (1 is white).
    pub bloom_threshold: f32,
    /// 1 keeps the contrast.
    pub contrast: f32,
    /// 1 keeps the colors, 0 is grey.
    pub saturation: f32,
    /// White balance, -1 (cool) to 1 (warm).
    pub temperature: f32,
    /// White balance, -1 (green) to 1 (magenta).
    pub tint: f32,
    /// How much the corners darken, 0 to 1.
    pub vignette: f32,
    /// Color fringes toward the edges, 0 to 1.
    pub chromatic_aberration: f32,
    /// Lens bend, -1 (pinch) to 1 (bulge); 0 is off.
    pub lens_distortion: f32,
    /// Zoom after the lens bend, above zero (default 1).
    pub lens_scale: f32,
    /// Whether `lut` is used.
    pub has_lut: bool,
    /// A color lookup table: n squares of n x n pixels in a row.
    pub lut: PonkoTexture,
    /// How much TAA sharpens its result, 0 to 1 (default 0.25).
    pub taa_sharpness: f32,
    /// Contact shadows: how far each pixel looks toward the sun for
    /// something close blocking it, in world units; 0 (default) is off.
    pub contact_shadows: f32,
    /// Screen-space reflections on smooth surfaces, 0 (default, off) to 1.
    pub ssr: f32,
    /// How far a screen-space reflection looks, in world units, above zero
    /// (default 30).
    pub ssr_max_distance: f32,
}

/// Triangles on the CPU (Rust `MeshData`), before `ponko_ctx_create_mesh`
/// puts them on the GPU. An owned handle: free it with `ponko_mesh_data_free`.
pub struct PonkoMeshData(pub(crate) MeshData);

/// A read-only look at a `PonkoMeshData`'s arrays, valid until it's freed.
/// `normals` and `uvs` are null when it has none.
/// ```
/// let v = ponko_ffi::PonkoMeshView { positions: std::ptr::null(), normals: std::ptr::null(), uvs: std::ptr::null(), vertex_count: 0, indices: std::ptr::null(), index_count: 0 };
/// assert_eq!(v.index_count, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoMeshView {
    /// `vertex_count` corner positions.
    pub positions: *const PonkoVec3,
    /// `vertex_count` unit normals, or null.
    pub normals: *const PonkoVec3,
    /// `vertex_count` texture coordinates, or null.
    pub uvs: *const PonkoVec2,
    /// Corners.
    pub vertex_count: usize,
    /// Three corner indices per triangle.
    pub indices: *const u32,
    /// Indices (three per triangle).
    pub index_count: usize,
}

// ---- layouts ----------------------------------------------------------------

same_layout!(PonkoVec3, Vec3, x = x, y = y, z = z);
same_layout!(PonkoModel, Model, mesh = mesh, color = color);
// TerrainTile is repr(C) { lods, lod_distance }; its fields are private to
// ponko, which pins their offsets to these.
same_layout!(PonkoTerrainTile, TerrainTile);
const _: () = assert!(
    std::mem::offset_of!(PonkoTerrainTile, lods) == 0 && std::mem::offset_of!(PonkoTerrainTile, lod_distance) == 16
);
const _: () = {
    assert!(size_of::<PonkoTransform3D>() == size_of::<Transform3D>());
    assert!(std::mem::offset_of!(PonkoTransform3D, rotation) == std::mem::offset_of!(Transform3D, rotation));
    assert!(std::mem::offset_of!(PonkoTransform3D, position) == std::mem::offset_of!(Transform3D, position));
    assert!(std::mem::offset_of!(PonkoTransform3D, scale) == std::mem::offset_of!(Transform3D, scale));
    assert!(size_of::<PonkoQuat>() == size_of::<Quat>());
    assert!(size_of::<PonkoMesh>() == size_of::<Mesh>() && size_of::<PonkoMaterial>() == size_of::<Material>());
};
// SAFETY: both repr(C) { u32 id } (sizes asserted above).
unsafe impl SameLayout<Mesh> for PonkoMesh {}
// SAFETY: as above.
unsafe impl SameLayout<PonkoMesh> for Mesh {}
// SAFETY: as above.
unsafe impl SameLayout<Material> for PonkoMaterial {}
// SAFETY: as above.
unsafe impl SameLayout<PonkoMaterial> for Material {}

pub(crate) fn quat_to_c(q: Quat) -> PonkoQuat {
    let [x, y, z, w] = q.to_array();
    PonkoQuat { x, y, z, w }
}
pub(crate) fn quat_from_c(q: PonkoQuat) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w)
}
pub(crate) fn transform_to_c(t: Transform3D) -> PonkoTransform3D {
    PonkoTransform3D {
        rotation: quat_to_c(t.rotation),
        position: to_c(t.position),
        scale: to_c(t.scale),
        reserved: [0.0; 2],
    }
}
pub(crate) fn transform_from_c(t: PonkoTransform3D) -> Transform3D {
    Transform3D {
        rotation: quat_from_c(t.rotation),
        position: to_c(t.position),
        scale: to_c(t.scale),
    }
}

// ---- values -----------------------------------------------------------------

/// No rotation.
/// ```
/// assert_eq!(ponko_ffi::ponko_quat_identity().w, 1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_quat_identity() -> PonkoQuat {
    quat_to_c(Quat::IDENTITY)
}

/// A rotation of `angle` radians around `axis` (counter-clockwise looking
/// down the axis). A zero axis gives no rotation.
/// ```
/// use ponko_ffi::*;
/// let q = ponko_quat_from_axis_angle(PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 }, 0.5);
/// assert!((q.y - 0.25f32.sin()).abs() < 1e-6);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_quat_from_axis_angle(axis: PonkoVec3, angle: f32) -> PonkoQuat {
    match to_c::<_, Vec3>(axis).try_normalize() {
        Some(axis) => quat_to_c(Quat::from_axis_angle(axis, angle)),
        None => ponko_quat_identity(),
    }
}

/// `a` after `b`: rotating by the result is rotating by `b`, then by `a`.
/// ```
/// use ponko_ffi::*;
/// let y = PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 };
/// let q = ponko_quat_mul(ponko_quat_from_axis_angle(y, 0.2), ponko_quat_from_axis_angle(y, 0.3));
/// assert!((q.y - ponko_quat_from_axis_angle(y, 0.5).y).abs() < 1e-6);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_quat_mul(a: PonkoQuat, b: PonkoQuat) -> PonkoQuat {
    quat_to_c((quat_from_c(a) * quat_from_c(b)).normalize())
}

/// `v` rotated by `q`.
/// ```
/// use ponko_ffi::*;
/// let q = ponko_quat_from_axis_angle(PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 }, std::f32::consts::FRAC_PI_2);
/// let v = ponko_quat_rotate(q, PonkoVec3 { x: 1.0, y: 0.0, z: 0.0 });
/// assert!((v.z + 1.0).abs() < 1e-6);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_quat_rotate(q: PonkoQuat, v: PonkoVec3) -> PonkoVec3 {
    to_c(quat_from_c(q) * to_c::<_, Vec3>(v))
}

/// A transform at `position`, not rotated, scale 1 (Rust `Transform3D::at`).
/// ```
/// let t = ponko_ffi::ponko_transform3d_at(ponko_ffi::PonkoVec3 { x: 1.0, y: 2.0, z: 3.0 });
/// assert_eq!(t.position.z, 3.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_transform3d_at(position: PonkoVec3) -> PonkoTransform3D {
    transform_to_c(Transform3D::at(to_c(position)))
}

/// `t` turned so its -Z axis points at `target`, +Y up
/// (Rust `Transform3D::looking_at`).
/// ```
/// use ponko_ffi::*;
/// let t = ponko_transform3d_looking_at(ponko_transform3d_at(PonkoVec3 { x: 0.0, y: 0.0, z: 5.0 }), Default::default());
/// assert!((ponko_transform3d_forward(t).z + 1.0).abs() < 1e-5);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_transform3d_looking_at(t: PonkoTransform3D, target: PonkoVec3) -> PonkoTransform3D {
    transform_to_c(transform_from_c(t).looking_at(to_c(target)))
}

/// The direction `t` faces: its -Z axis.
/// ```
/// let f = ponko_ffi::ponko_transform3d_forward(ponko_ffi::ponko_transform3d_at(Default::default()));
/// assert_eq!(f.z, -1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_transform3d_forward(t: PonkoTransform3D) -> PonkoVec3 {
    to_c(transform_from_c(t).forward())
}

/// A model of `mesh` in its own colors (Rust `Model::new`).
/// ```
/// assert_eq!(ponko_ffi::ponko_model(ponko_ffi::PonkoMesh { id: 1 }).color.a, 1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_model(mesh: PonkoMesh) -> PonkoModel {
    to_c(Model::new(to_c(mesh)))
}

/// A plain material of one color (Rust `MaterialDesc::color`): roughness
/// 0.6, not metallic, no texture, no glow.
/// ```
/// let d = ponko_ffi::ponko_material_desc(ponko_ffi::PonkoColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 });
/// assert_eq!(d.emissive.r, 0.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_material_desc(color: PonkoColor) -> PonkoMaterialDesc {
    material_to_c(&MaterialDesc::color(to_c(color)))
}

/// A Rust material description as C's.
fn material_to_c(d: &MaterialDesc) -> PonkoMaterialDesc {
    let none = PonkoTexture {
        id: 0,
        width: 0,
        height: 0,
    };
    let map = |t: Option<ponko::prelude::Texture>| (t.is_some(), t.map_or(none, texture_to_c));
    let (has_texture, texture) = map(d.texture);
    let (has_normal_map, normal_map) = map(d.normal_map);
    let (has_orm_map, orm_map) = map(d.orm_map);
    let (has_emissive_map, emissive_map) = map(d.emissive_map);
    let customs = d.custom_textures.map(map);
    let mut custom = [0.0; 16];
    for (i, v) in d.custom.iter().enumerate() {
        custom[i * 4..i * 4 + 4].copy_from_slice(&v.to_array());
    }
    PonkoMaterialDesc {
        color: to_c(d.color),
        has_texture,
        texture,
        roughness: d.roughness,
        metallic: d.metallic,
        emissive: to_c(d.emissive),
        has_normal_map,
        normal_map,
        normal_strength: d.normal_strength,
        has_orm_map,
        orm_map,
        has_emissive_map,
        emissive_map,
        // `Blend` is `repr(u32)` with the C values as its discriminants
        // (checked below). This crate can't match it exhaustively, as it is
        // `#[non_exhaustive]`; ponko's `c_api_mirror` test stops the build
        // when a variant is added.
        blend: d.blend as u32,
        double_sided: d.double_sided,
        has_shader: d.shader.is_some(),
        shader: PonkoShader {
            id: d.shader.map_or(0, __ffi::id),
        },
        custom,
        has_custom_textures: customs.map(|c| c.0),
        custom_textures: customs.map(|c| c.1),
        has_custom_arrays: d.custom_arrays.map(|a| a.is_some()),
        custom_arrays: d
            .custom_arrays
            .map(|a| a.map_or_else(PonkoTextureArray::default, array_to_c)),
    }
}

/// A C material description as Rust's, checked.
fn material_from_c(c: &ponko::prelude::Ctx, d: &PonkoMaterialDesc) -> Result<MaterialDesc, crate::Failure> {
    if !((0.0..=1.0).contains(&d.roughness) && (0.0..=1.0).contains(&d.metallic)) {
        return Err(invalid("roughness and metallic go from 0 to 1"));
    }
    let blend = match d.blend {
        0 => Blend::Opaque,
        1 => Blend::Transparent,
        2 => Blend::Cutout,
        _ => return Err(invalid("blend must be 0 (opaque), 1 (transparent) or 2 (cut out)")),
    };
    let shader = d.has_shader.then_some(__ffi::from_id::<Shader>(d.shader.id));
    if shader.is_some_and(|s| !c.gfx.is_live(s)) {
        return Err(invalid("that shader wasn't made by this game"));
    }
    let map = |has: bool, t: PonkoTexture| has.then(|| texture_from_c(t));
    let arrays = std::array::from_fn(|i| d.has_custom_arrays[i].then(|| array_from_c(d.custom_arrays[i])));
    if arrays.iter().flatten().any(|a| !c.gfx.has_texture_array(*a)) {
        return Err(invalid("that texture array wasn't made by this game"));
    }
    let v = |i: usize| {
        Vec4::new(
            d.custom[i * 4],
            d.custom[i * 4 + 1],
            d.custom[i * 4 + 2],
            d.custom[i * 4 + 3],
        )
    };
    Ok(MaterialDesc::default()
        .with_color(to_c(d.color))
        .with_texture(map(d.has_texture, d.texture))
        .with_roughness(d.roughness)
        .with_metallic(d.metallic)
        .with_emissive(to_c(d.emissive))
        .with_normal_map(map(d.has_normal_map, d.normal_map))
        .with_normal_strength(d.normal_strength)
        .with_orm_map(map(d.has_orm_map, d.orm_map))
        .with_emissive_map(map(d.has_emissive_map, d.emissive_map))
        .with_blend(blend)
        .with_double_sided(d.double_sided)
        .with_shader(shader)
        .with_custom([v(0), v(1), v(2), v(3)])
        .with_custom_textures([
            map(d.has_custom_textures[0], d.custom_textures[0]),
            map(d.has_custom_textures[1], d.custom_textures[1]),
        ])
        .with_custom_arrays(arrays))
}

const _: () = assert!(Blend::Opaque as u32 == 0 && Blend::Transparent as u32 == 1 && Blend::Cutout as u32 == 2);

const _: () = assert!(
    size_of::<PonkoTextureArray>() == size_of::<TextureArray>()
        && align_of::<PonkoTextureArray>() == align_of::<TextureArray>()
);
fn array_from_c(a: PonkoTextureArray) -> TextureArray {
    // SAFETY: both repr(C), four u32 in the same order (id, width, height,
    // layers); unknown ids are refused by `has_texture_array` or read as white.
    unsafe { std::mem::transmute(a) }
}
fn array_to_c(a: TextureArray) -> PonkoTextureArray {
    // SAFETY: as above.
    unsafe { std::mem::transmute(a) }
}

/// A terrain material description with the defaults (Rust
/// `TerrainMaterialDesc::new`): ground `layers` blended by a `weights` map
/// lying over `size` world units from `corner`, no normal maps, each layer
/// repeating every 4 units, roughness 0.9.
/// ```
/// let a = ponko_ffi::PonkoTextureArray::default();
/// let d = ponko_ffi::ponko_terrain_material_desc(a, a, Default::default(), ponko_ffi::PonkoVec2 { x: 1.0, y: 1.0 });
/// assert_eq!(d.layer_size[7], 4.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_terrain_material_desc(
    layers: PonkoTextureArray,
    weights: PonkoTextureArray,
    corner: PonkoVec2,
    size: PonkoVec2,
) -> PonkoTerrainMaterialDesc {
    let d = TerrainMaterialDesc::new(array_from_c(layers), array_from_c(weights), to_c(corner), to_c(size));
    PonkoTerrainMaterialDesc {
        layers,
        has_normals: false,
        normals: PonkoTextureArray::default(),
        normal_strength: d.normal_strength,
        weights,
        weights_corner: corner,
        weights_size: size,
        layer_size: d.layer_size,
        roughness: d.roughness,
    }
}

/// Makes a texture array of `width` x `height` images (Rust
/// `Graphics::create_texture_array`): `rgba` holds them one after another, 4
/// bytes a pixel, `len` bytes in all (a whole number of images). `kind` is
/// `PONKO_TEXTURE_COLOR` or `PONKO_TEXTURE_DATA`.
/// # Safety
/// `ctx` null or live; `rgba` holds `len` bytes; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_texture_array(std::ptr::null_mut(), 1, 1, std::ptr::null(), 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_texture_array(
    ctx: *mut PonkoCtx,
    width: u32,
    height: u32,
    rgba: *const u8,
    len: usize,
    kind: u32,
    out_array: *mut PonkoTextureArray,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if rgba.is_null() || out_array.is_null() {
                return Err(invalid("pixels or output is null"));
            }
            let kind = kind_from_c(kind)?;
            let image = width as usize * height as usize * 4;
            if image == 0 || len == 0 || !len.is_multiple_of(image) {
                return Err(invalid_owned(format!(
                    "{len} bytes isn't a whole number of {width} x {height} RGBA images"
                )));
            }
            let bytes = std::slice::from_raw_parts(rgba, len);
            let layers: Vec<&[u8]> = bytes.chunks_exact(image).collect();
            let array = c
                .gfx
                .try_create_texture_array(width, height, &layers, kind)
                .map_err(invalid_owned)?;
            out(out_array, array_to_c(array))
        })
    })
}

/// Loads `count` images from assets/, all the same size, into a texture
/// array, one layer each (Rust `Graphics::load_texture_array`). `kind` is
/// `PONKO_TEXTURE_COLOR` or `PONKO_TEXTURE_DATA`.
/// # Safety
/// `ctx` null or live; `paths` holds `count` NUL-terminated strings; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_texture_array(std::ptr::null_mut(), std::ptr::null(), 0, 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_texture_array(
    ctx: *mut PonkoCtx,
    paths: *const *const c_char,
    count: usize,
    kind: u32,
    out_array: *mut PonkoTextureArray,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if paths.is_null() || out_array.is_null() {
                return Err(invalid("paths or output is null"));
            }
            let kind = kind_from_c(kind)?;
            let names = std::slice::from_raw_parts(paths, count)
                .iter()
                .map(|&p| text(p))
                .collect::<Result<Vec<&str>, _>>()?;
            let array = c.gfx.try_load_texture_array(&names, kind).map_err(invalid_owned)?;
            out(out_array, array_to_c(array))
        })
    })
}

fn kind_from_c(kind: u32) -> Result<TextureKind, crate::Failure> {
    match kind {
        PONKO_TEXTURE_COLOR => Ok(TextureKind::Color),
        PONKO_TEXTURE_DATA => Ok(TextureKind::Data),
        _ => Err(invalid(
            "kind must be PONKO_TEXTURE_COLOR (0) or PONKO_TEXTURE_DATA (1)",
        )),
    }
}

/// Makes a terrain material (Rust `Graphics::create_terrain_material`): an
/// ordinary material, for `ponko_ctx_create_terrain_tile` or any mesh.
/// # Safety
/// `ctx` null or live; `desc` readable; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_terrain_material(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_terrain_material(
    ctx: *mut PonkoCtx,
    desc: *const PonkoTerrainMaterialDesc,
    out_material: *mut PonkoMaterial,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(d) = desc.as_ref() else {
                return Err(invalid("description is null"));
            };
            if out_material.is_null() {
                return Err(invalid("output is null"));
            }
            let rust = TerrainMaterialDesc::new(
                array_from_c(d.layers),
                array_from_c(d.weights),
                to_c(d.weights_corner),
                to_c(d.weights_size),
            )
            .with_normals(d.has_normals.then(|| array_from_c(d.normals)))
            .with_normal_strength(d.normal_strength)
            .with_layer_size(d.layer_size)
            .with_roughness(d.roughness);
            let material = c.gfx.try_create_terrain_material(&rust).map_err(invalid_owned)?;
            out(out_material, to_c(material))
        })
    })
}

// ---- components -------------------------------------------------------------

/// The id of the built-in `Transform3D` component (`PonkoTransform3D`),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_transform3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_transform3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<Transform3D>(world, out_id) }
}

/// The id of the built-in `Model` component (`PonkoModel`), registering
/// it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_model_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_model_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<Model>(world, out_id) }
}

/// The id of the built-in `TerrainTile` component (`PonkoTerrainTile`),
/// registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_terrain_tile_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_terrain_tile_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<TerrainTile>(world, out_id) }
}

// ---- mesh data --------------------------------------------------------------

fn boxed(data: MeshData) -> *mut PonkoMeshData {
    Box::into_raw(Box::new(PonkoMeshData(data)))
}

/// Copies triangles into new mesh data: `vertex_count` positions, normals
/// and texture coordinates (each may be null: normals are then computed,
/// coordinates zero) and `index_count` indices, three per triangle,
/// counter-clockwise seen from the front. Null (see `ponko_last_error`)
/// if they don't fit together.
/// # Safety
/// Non-null arrays hold the stated counts.
/// ```
/// use ponko_ffi::*;
/// let p = [PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 }, PonkoVec3 { x: 1.0, y: 0.0, z: 0.0 }, PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 }];
/// let d = unsafe { ponko_mesh_data_new(p.as_ptr(), std::ptr::null(), std::ptr::null(), 3, [0, 1, 2].as_ptr(), 3) };
/// assert!(!d.is_null());
/// unsafe { ponko_mesh_data_free(d) };
/// assert!(unsafe { ponko_mesh_data_new(p.as_ptr(), std::ptr::null(), std::ptr::null(), 3, [0, 1, 5].as_ptr(), 3) }.is_null());
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_mesh_data_new(
    positions: *const PonkoVec3,
    normals: *const PonkoVec3,
    uvs: *const PonkoVec2,
    vertex_count: usize,
    indices: *const u32,
    index_count: usize,
) -> *mut PonkoMeshData {
    let mut result = std::ptr::null_mut();
    boundary(|| {
        if (positions.is_null() && vertex_count > 0) || (indices.is_null() && index_count > 0) {
            return Err(invalid("positions or indices are null"));
        }
        // SAFETY: caller passes arrays of the stated lengths (null only when optional).
        let slice = |ptr: *const PonkoVec3| unsafe {
            if ptr.is_null() || vertex_count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(ptr.cast::<Vec3>(), vertex_count).to_vec()
            }
        };
        let data = MeshData {
            positions: slice(positions),
            normals: slice(normals),
            // SAFETY: as above.
            uvs: unsafe {
                if uvs.is_null() || vertex_count == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(uvs.cast::<Vec2>(), vertex_count).to_vec()
                }
            },
            // SAFETY: as above.
            indices: unsafe {
                if index_count == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(indices, index_count).to_vec()
                }
            },
            ..Default::default()
        };
        data.check().map_err(invalid_owned)?;
        result = boxed(data);
        Ok(())
    });
    result
}

/// A cube `size` wide around the origin (Rust `MeshData::cube`).
/// ```
/// let d = ponko_ffi::ponko_mesh_data_cube(1.0);
/// unsafe { ponko_ffi::ponko_mesh_data_free(d) };
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_mesh_data_cube(size: f32) -> *mut PonkoMeshData {
    boxed(MeshData::cube(size))
}

/// A flat square `size` wide on the XZ plane, facing up (Rust `MeshData::plane`).
/// ```
/// let d = ponko_ffi::ponko_mesh_data_plane(10.0);
/// unsafe { ponko_ffi::ponko_mesh_data_free(d) };
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_mesh_data_plane(size: f32) -> *mut PonkoMeshData {
    boxed(MeshData::plane(size))
}

/// A sphere of `radius` from `segments` slices around (at least 3; Rust
/// `MeshData::sphere`).
/// ```
/// let d = ponko_ffi::ponko_mesh_data_sphere(0.5, 24);
/// unsafe { ponko_ffi::ponko_mesh_data_free(d) };
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_mesh_data_sphere(radius: f32, segments: u32) -> *mut PonkoMeshData {
    boxed(MeshData::sphere(radius, segments))
}

/// Whether `count` heights make a square grid of at least 2 x 2.
pub(crate) fn square_grid(count: usize) -> bool {
    let n = (count as f64).sqrt() as usize;
    n >= 2 && n * n == count
}

/// A square of terrain `size` wide from `count` heights: n x n of them, row
/// after row along +Z, NaN for a hole; every `step`-th height is used, with
/// skirts at the edges (Rust `MeshData::heightfield`). Null if the heights
/// aren't a square grid of at least 2 x 2 or `step` is 0.
/// # Safety
/// `heights` holds `count` floats.
/// ```
/// let h = [0.0f32, 0.0, 0.0, 1.0];
/// let d = unsafe { ponko_ffi::ponko_mesh_data_heightfield(h.as_ptr(), 4, 2.0, 1) };
/// assert!(!d.is_null());
/// assert!(unsafe { ponko_ffi::ponko_mesh_data_heightfield(h.as_ptr(), 3, 2.0, 1) }.is_null());
/// unsafe { ponko_ffi::ponko_mesh_data_free(d) };
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_mesh_data_heightfield(
    heights: *const f32,
    count: usize,
    size: f32,
    step: u32,
) -> *mut PonkoMeshData {
    if heights.is_null() || !square_grid(count) || step == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller's contract: `count` floats at `heights`.
    let heights = unsafe { std::slice::from_raw_parts(heights, count) };
    boxed(MeshData::heightfield(heights, size, step as usize))
}

/// Gives mesh data one color per corner (Rust `MeshData::colors`),
/// multiplying its material's color; `count` must be its number of corners,
/// or 0 to remove them.
/// # Safety
/// `data` null or live; `colors` holds `count` colors (may be null when 0).
/// ```
/// use ponko_ffi::{PonkoStatus::*, *};
/// let d = ponko_mesh_data_plane(1.0);
/// let red = [PonkoColor { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }; 4];
/// assert_eq!(unsafe { ponko_mesh_data_set_colors(d, red.as_ptr(), 4) }, PONKO_OK);
/// assert_eq!(unsafe { ponko_mesh_data_set_colors(d, red.as_ptr(), 3) }, PONKO_INVALID_ARGUMENT);
/// unsafe { ponko_mesh_data_free(d) };
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_mesh_data_set_colors(
    data: *mut PonkoMeshData,
    colors: *const PonkoColor,
    count: usize,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: caller contract: null or live.
        let Some(data) = (unsafe { data.as_mut() }) else {
            return Err(invalid("mesh data is null"));
        };
        if colors.is_null() && count > 0 {
            return Err(invalid("colors is null"));
        }
        if count != 0 && count != data.0.positions.len() {
            return Err(invalid_owned(format!(
                "{count} colors for {} corners",
                data.0.positions.len()
            )));
        }
        data.0.colors = if count == 0 {
            Vec::new()
        } else {
            // SAFETY: caller passes `count` colors.
            unsafe { std::slice::from_raw_parts(colors, count) }
                .iter()
                .map(|&c| to_c(c))
                .collect()
        };
        Ok(())
    })
}

/// Frees mesh data. Null is ignored.
/// # Safety
/// `data` must be null or live, and not used afterwards.
/// ```
/// unsafe { ponko_ffi::ponko_mesh_data_free(std::ptr::null_mut()) };
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_mesh_data_free(data: *mut PonkoMeshData) {
    if !data.is_null() {
        // SAFETY: caller passes mesh data from a ponko_mesh_data_* call, once.
        drop(unsafe { Box::from_raw(data) });
    }
}

/// Looks at mesh data's arrays.
/// # Safety
/// `data` live; `out_view` writable.
/// ```
/// use ponko_ffi::*;
/// let d = ponko_mesh_data_cube(1.0);
/// let mut v = std::mem::MaybeUninit::<PonkoMeshView>::uninit();
/// assert_eq!(unsafe { ponko_mesh_data_view(d, v.as_mut_ptr()) }, PonkoStatus::PONKO_OK);
/// assert_eq!(unsafe { v.assume_init() }.index_count, 36);
/// unsafe { ponko_mesh_data_free(d) };
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_mesh_data_view(data: *const PonkoMeshData, out_view: *mut PonkoMeshView) -> PonkoStatus {
    boundary(|| {
        // SAFETY: caller passes null or live mesh data.
        let Some(data) = (unsafe { data.as_ref() }) else {
            return Err(invalid("mesh data is null"));
        };
        let d = &data.0;
        let ptr = |v: &[Vec3]| {
            if v.is_empty() {
                std::ptr::null()
            } else {
                v.as_ptr().cast::<PonkoVec3>()
            }
        };
        // SAFETY: caller passes a writable output.
        unsafe {
            out(
                out_view,
                PonkoMeshView {
                    positions: ptr(&d.positions),
                    normals: ptr(&d.normals),
                    uvs: if d.uvs.is_empty() {
                        std::ptr::null()
                    } else {
                        d.uvs.as_ptr().cast()
                    },
                    vertex_count: d.positions.len(),
                    indices: d.indices.as_ptr(),
                    index_count: d.indices.len(),
                },
            )
        }
    })
}

// ---- ctx ----------------------------------------------------------------------

/// Puts a material on the GPU. Roughness and metallic go from 0 to 1.
/// # Safety
/// `ctx` null or live; pointers valid.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_material(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_material(
    ctx: *mut PonkoCtx,
    desc: *const PonkoMaterialDesc,
    out_material: *mut PonkoMaterial,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(d) = desc.as_ref() else {
                return Err(invalid("material desc is null"));
            };
            if out_material.is_null() {
                return Err(invalid("output is null"));
            }
            let desc = material_from_c(c, d)?;
            out(out_material, to_c(c.gfx.create_material(desc)))
        })
    })
}

/// Changes a material: every model drawn with it changes too.
/// # Safety
/// `ctx` null or live; `desc` valid.
/// ```
/// let m = ponko_ffi::PonkoMaterial { id: 0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_update_material(std::ptr::null_mut(), m, std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_update_material(
    ctx: *mut PonkoCtx,
    material: PonkoMaterial,
    desc: *const PonkoMaterialDesc,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(d) = desc.as_ref() else {
                return Err(invalid("material desc is null"));
            };
            let material: Material = to_c(material);
            if !c.gfx.is_live(material) {
                return Err(invalid("that material wasn't made by this game"));
            }
            let desc = material_from_c(c, d)?;
            c.gfx.update_material(material, desc);
            Ok(())
        })
    })
}

/// The materials loaded from glTF files so far that the files call `name`,
/// with what each was made from (Rust `Graphics::loaded_materials`): to give
/// them a shader of your own with `ponko_ctx_update_material`. Writes up
/// to `capacity` of them to `out_materials` and `out_descs` (either may be
/// null to only count), and how many there are to `out_count`.
/// # Safety
/// `ctx` null or live; `name` a C string; the outputs writable for `capacity` entries.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_loaded_materials(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_loaded_materials(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    out_materials: *mut PonkoMaterial,
    out_descs: *mut PonkoMaterialDesc,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            if out_count.is_null() {
                return Err(invalid("output is null"));
            }
            let found = c.gfx.loaded_materials(name);
            for (i, (material, desc)) in found.iter().take(capacity).enumerate() {
                if !out_materials.is_null() {
                    out_materials.add(i).write(to_c(*material));
                }
                if !out_descs.is_null() {
                    out_descs.add(i).write(material_to_c(desc));
                }
            }
            out(out_count, found.len())
        })
    })
}

/// Loads a material shader (WGSL) from `assets/`; with hot reload on,
/// saving the file updates the game. A file that can't be read or doesn't
/// compile is an error; `ponko_last_error` says where. With hot reload on,
/// a file that doesn't compile still gives a shader, drawing with the
/// built-in look until a saved fix compiles (`ponko_ctx_shader_status`
/// says what's wrong).
/// # Safety
/// `ctx` null or live; `path` a C string; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_shader(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_shader(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_shader: *mut PonkoShader,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_shader.is_null() {
                return Err(invalid("output is null"));
            }
            let shader = c.gfx.try_load_shader(path).map_err(invalid_owned)?;
            out(out_shader, PonkoShader { id: __ffi::id(shader) })
        })
    })
}

/// Makes a material shader from WGSL source; `name` is for error messages.
/// Source that doesn't compile is an error; `ponko_last_error` says where.
/// # Safety
/// `ctx` null or live; `name` and `source` C strings; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_shader(std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_shader(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    source: *const c_char,
    out_shader: *mut PonkoShader,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            let source = text(source)?;
            if out_shader.is_null() {
                return Err(invalid("output is null"));
            }
            let shader = c.gfx.try_create_shader(name, source).map_err(invalid_owned)?;
            out(out_shader, PonkoShader { id: __ffi::id(shader) })
        })
    })
}

/// Loads a compute pass shader (WGSL, defining `fn compute(c: Cell) ->
/// vec4<f32>`) from assets/, hot reloaded; the same file again gives the
/// same compute pass (Rust `Graphics::load_compute`). With hot reload on, a
/// file that doesn't compile still gives a compute pass, skipped until a
/// saved fix compiles (`ponko_ctx_shader_status` says what's wrong).
/// # Safety
/// `ctx` null or live; `path` a NUL-terminated string; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_compute(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_compute(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_compute: *mut PonkoCompute,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_compute.is_null() {
                return Err(invalid("output is null"));
            }
            let compute = c.gfx.try_load_compute(path).map_err(invalid_owned)?;
            out(out_compute, PonkoCompute { id: __ffi::id(compute) })
        })
    })
}

/// Makes a compute pass shader from WGSL source; `name` is for error
/// messages (Rust `Graphics::create_compute`).
/// # Safety
/// `ctx` null or live; `name` and `source` NUL-terminated strings; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_compute(std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_compute(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    source: *const c_char,
    out_compute: *mut PonkoCompute,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            let source = text(source)?;
            if out_compute.is_null() {
                return Err(invalid("output is null"));
            }
            let compute = c.gfx.try_create_compute(name, source).map_err(invalid_owned)?;
            out(out_compute, PonkoCompute { id: __ffi::id(compute) })
        })
    })
}

/// `count` C compute passes as Rust `ComputePass`es, each checked against
/// this game's handles.
/// # Safety
/// `passes` holds `count` values (may be null when 0).
pub(crate) unsafe fn compute_passes_from_c(
    c: &ponko::prelude::Ctx,
    passes: *const PonkoComputePass,
    count: usize,
) -> Result<Vec<ComputePass>, crate::Failure> {
    if passes.is_null() && count > 0 {
        return Err(invalid("passes is null"));
    }
    let given = if count == 0 {
        &[][..]
    } else {
        // SAFETY: caller contract.
        unsafe { std::slice::from_raw_parts(passes, count) }
    };
    let mut list = Vec::with_capacity(count);
    for p in given {
        let compute = __ffi::from_id::<Compute>(p.compute.id);
        if !c.gfx.is_live(compute) {
            return Err(invalid("that compute pass wasn't made by this game"));
        }
        let target = texture_from_c(p.target);
        let buffer = p.has_buffer.then(|| __ffi::from_id::<GpuBuffer>(p.buffer.id));
        if buffer.is_some_and(|b| c.gfx.gpu_buffer_len(b).is_none()) {
            return Err(invalid("invalid GPU buffer"));
        }
        if buffer.is_none() && !c.gfx.is_render_target(target) {
            return Err(invalid("that texture isn't a render target of this game"));
        }
        let material = p.has_material.then(|| to_c(p.material));
        if material.is_some_and(|m| !c.gfx.is_live(m)) {
            return Err(invalid("that material wasn't made by this game"));
        }
        list.push(ComputePass {
            buffer,
            compute,
            target,
            material,
        });
    }
    Ok(list)
}

/// A Rust compute pass as C sees it.
pub(crate) fn compute_pass_to_c(p: &ComputePass) -> PonkoComputePass {
    PonkoComputePass {
        has_buffer: p.buffer.is_some(),
        buffer: PonkoGpuBuffer {
            id: p.buffer.map_or(0, __ffi::id),
        },
        compute: PonkoCompute {
            id: __ffi::id(p.compute),
        },
        target: texture_to_c(p.target),
        has_material: p.material.is_some(),
        material: p.material.map(to_c).unwrap_or_default(),
    }
}

/// Replaces the compute passes run each frame with `count` of them, in
/// order (Rust `ctx.scene3d.passes.compute`); 0 runs none.
/// # Safety
/// `ctx` null or live; `passes` holds `count` values (may be null when 0).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_compute_passes(std::ptr::null_mut(), std::ptr::null(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_compute_passes(
    ctx: *mut PonkoCtx,
    passes: *const PonkoComputePass,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let list = compute_passes_from_c(c, passes, count)?;
            c.scene3d.passes.compute = list;
            Ok(())
        })
    })
}

/// The compute passes run each frame: writes up to `capacity` of them to
/// `out_passes` and how many there are to `out_count` (Rust
/// `ctx.scene3d.passes.compute`).
/// # Safety
/// `ctx` null or live; `out_passes` holds `capacity` values (may be null
/// when 0); `out_count` writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_compute_passes(std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_compute_passes(
    ctx: *mut PonkoCtx,
    out_passes: *mut PonkoComputePass,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_count.is_null() || (out_passes.is_null() && capacity > 0) {
                return Err(invalid("output is null"));
            }
            for (i, p) in c.scene3d.passes.compute.iter().take(capacity).enumerate() {
                out_passes.add(i).write(compute_pass_to_c(p));
            }
            out(out_count, c.scene3d.passes.compute.len())
        })
    })
}

/// Loads a screen effect shader (WGSL, defining `fn effect(p: Pixel) ->
/// vec3<f32>`) from assets/, hot reloaded; the same file again gives the
/// same effect (Rust `Graphics::load_effect`). With hot reload on, a file that
/// doesn't compile still gives an effect, showing the frame as it is until
/// a saved fix compiles (`ponko_ctx_shader_status` says what's wrong).
/// # Safety
/// `ctx` null or live; `path` a NUL-terminated string; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_effect(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_effect(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_effect: *mut PonkoEffect,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_effect.is_null() {
                return Err(invalid("output is null"));
            }
            let effect = c.gfx.try_load_effect(path).map_err(invalid_owned)?;
            out(out_effect, PonkoEffect { id: __ffi::id(effect) })
        })
    })
}

/// Makes a screen effect shader from WGSL source; `name` is for error
/// messages (Rust `Graphics::create_effect`).
/// # Safety
/// `ctx` null or live; `name` and `source` NUL-terminated strings; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_effect(std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_effect(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    source: *const c_char,
    out_effect: *mut PonkoEffect,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            let source = text(source)?;
            if out_effect.is_null() {
                return Err(invalid("output is null"));
            }
            let effect = c.gfx.try_create_effect(name, source).map_err(invalid_owned)?;
            out(out_effect, PonkoEffect { id: __ffi::id(effect) })
        })
    })
}

/// Replaces the screen effects run each frame with `count` of them, in
/// order (Rust `ctx.scene3d.passes.effects`); 0 runs none.
/// # Safety
/// `ctx` null or live; `effects` holds `count` values (may be null when 0).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_effects(std::ptr::null_mut(), std::ptr::null(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_effects(
    ctx: *mut PonkoCtx,
    effects: *const PonkoScreenEffect,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if effects.is_null() && count > 0 {
                return Err(invalid("effects is null"));
            }
            let given = if count == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(effects, count)
            };
            let mut list = Vec::with_capacity(count);
            for e in given {
                let effect = __ffi::from_id::<Effect>(e.effect.id);
                if !c.gfx.is_live(effect) {
                    return Err(invalid("that effect wasn't made by this game"));
                }
                let material = e.has_material.then(|| to_c(e.material));
                if material.is_some_and(|m| !c.gfx.is_live(m)) {
                    return Err(invalid("that material wasn't made by this game"));
                }
                list.push(ScreenEffect { effect, material });
            }
            c.scene3d.passes.effects = list;
            Ok(())
        })
    })
}

/// The screen effects run each frame: writes up to `capacity` of them to
/// `out_effects` and how many there are to `out_count` (Rust `ctx.scene3d.passes.effects`).
/// # Safety
/// `ctx` null or live; `out_effects` holds `capacity` values (may be null
/// when 0); `out_count` writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_effects(std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_effects(
    ctx: *mut PonkoCtx,
    out_effects: *mut PonkoScreenEffect,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_count.is_null() || (out_effects.is_null() && capacity > 0) {
                return Err(invalid("output is null"));
            }
            for (i, e) in c.scene3d.passes.effects.iter().take(capacity).enumerate() {
                out_effects.add(i).write(PonkoScreenEffect {
                    effect: PonkoEffect {
                        id: __ffi::id(e.effect),
                    },
                    has_material: e.material.is_some(),
                    material: e.material.map(to_c).unwrap_or_default(),
                });
            }
            out(out_count, c.scene3d.passes.effects.len())
        })
    })
}

/// A plain white material, made once and shared.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_default_material(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_default_material(
    ctx: *mut PonkoCtx,
    out_material: *mut PonkoMaterial,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_material, to_c(c.gfx.default_material()))) })
}

/// Puts mesh data on the GPU as a mesh of one material. The data can be
/// freed afterwards.
/// # Safety
/// `ctx` null or live; `data` live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_mesh(std::ptr::null_mut(), std::ptr::null(), Default::default(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_mesh(
    ctx: *mut PonkoCtx,
    data: *const PonkoMeshData,
    material: PonkoMaterial,
    out_mesh: *mut PonkoMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(data) = data.as_ref() else {
                return Err(invalid("mesh data is null"));
            };
            if out_mesh.is_null() {
                return Err(invalid("output is null"));
            }
            // Checked here so bad input is an error, not a panic.
            data.0.check().map_err(invalid_owned)?;
            if !c.gfx.is_live(to_c(material)) {
                return Err(invalid("that material wasn't made by this game"));
            }
            out(out_mesh, to_c(c.gfx.create_mesh(&data.0, to_c(material))))
        })
    })
}

/// Puts mesh data on the GPU as a mesh with no material of its own (Rust
/// `Ctx::create_plain_mesh`): `ponko_ctx_create_mesh` with the default
/// material, for meshes whose look comes from each model
/// (`ponko_ctx_set_model_material`). The data can be freed afterwards.
/// # Safety
/// `ctx` null or live; `data` live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_plain_mesh(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_plain_mesh(
    ctx: *mut PonkoCtx,
    data: *const PonkoMeshData,
    out_mesh: *mut PonkoMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(data) = data.as_ref() else {
                return Err(invalid("mesh data is null"));
            };
            if out_mesh.is_null() {
                return Err(invalid("output is null"));
            }
            let mesh = c
                .gfx
                .try_create_plain_mesh(&data.0)
                .map_err(|e| invalid_owned(e.to_string()))?;
            out(out_mesh, to_c(mesh))
        })
    })
}

/// Makes a terrain tile `size` wide from `count` heights (n x n, row after
/// row along +Z, NaN for a hole) drawn with `material` (Rust
/// `Graphics::create_terrain_tile`). Spawn it with a `PonkoTransform3D` at the
/// tile's corner.
/// # Safety
/// `ctx` null or live; `heights` holds `count` floats; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_terrain_tile(std::ptr::null_mut(), std::ptr::null(), 0, 1.0, Default::default(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_terrain_tile(
    ctx: *mut PonkoCtx,
    heights: *const f32,
    count: usize,
    size: f32,
    material: PonkoMaterial,
    out_tile: *mut PonkoTerrainTile,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if heights.is_null() || out_tile.is_null() {
                return Err(invalid("heights or output is null"));
            }
            // Checked here so bad input is an error, not a panic.
            if !square_grid(count) {
                return Err(invalid("the heights aren't a square grid of at least 2 x 2"));
            }
            if !(size > 0.0 && size.is_finite()) {
                return Err(invalid("size must be above 0"));
            }
            if !c.gfx.is_live(to_c(material)) {
                return Err(invalid("that material wasn't made by this game"));
            }
            let heights = std::slice::from_raw_parts(heights, count);
            out(out_tile, to_c(c.gfx.create_terrain_tile(heights, size, to_c(material))))
        })
    })
}

/// Loads a glTF model (`.glb`, or `.gltf` with its files) from assets/ as
/// one mesh with its materials. Loading the same file again returns the
/// same mesh.
/// # Safety
/// `ctx` null or live; `path` a NUL-terminated string; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_mesh(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_mesh(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_mesh: *mut PonkoMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_mesh.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_mesh, to_c(c.gfx.load_mesh(path)))
        })
    })
}

/// How many 3D models were drawn last frame.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_models_drawn(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_models_drawn(ctx: *mut PonkoCtx, out_count: *mut usize) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_count, c.gfx.models_drawn())) })
}

fn camera_to_c(c: &Camera3D) -> PonkoCamera3D {
    PonkoCamera3D {
        position: to_c(c.position),
        rotation: quat_to_c(c.rotation),
        fov_y: c.fov_y,
        near: c.near,
        far: c.far,
    }
}

/// A Rust camera from a C one, keeping `base`'s window size; checked.
fn camera_from_c(base: Camera3D, cam: &PonkoCamera3D) -> Result<Camera3D, crate::Failure> {
    if !(cam.fov_y > 0.0 && cam.fov_y < std::f32::consts::PI) {
        return Err(invalid("fov_y must be between 0 and pi"));
    }
    if !(cam.near > 0.0 && cam.far > cam.near && cam.far.is_finite()) {
        return Err(invalid("needs 0 < near < far"));
    }
    let rotation = quat_from_c(cam.rotation);
    if !(rotation.length_squared() > 1e-12 && rotation.is_finite()) {
        return Err(invalid("rotation must not be zero"));
    }
    let mut camera = base;
    camera.position = to_c(cam.position);
    camera.rotation = rotation.normalize();
    camera.fov_y = cam.fov_y;
    camera.near = cam.near;
    camera.far = cam.far;
    Ok(camera)
}

fn target_camera_to_c(t: &TargetCamera) -> PonkoTargetCamera {
    PonkoTargetCamera {
        camera: camera_to_c(&t.camera),
        target: texture_to_c(t.target),
        has_clip_plane: t.clip_plane.is_some(),
        clip_plane: t.clip_plane.unwrap_or(Vec4::ZERO).to_array(),
    }
}

/// Makes a texture of `width` x `height` pixels that target cameras draw
/// the 3D world into (Rust `Graphics::create_render_target`); materials use it
/// like any texture.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_render_target(std::ptr::null_mut(), 64, 64, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_render_target(
    ctx: *mut PonkoCtx,
    width: u32,
    height: u32,
    out_texture: *mut PonkoTexture,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_texture.is_null() {
                return Err(invalid("output is null"));
            }
            let texture = c.gfx.try_create_render_target(width, height).map_err(invalid_owned)?;
            out(out_texture, texture_to_c(texture))
        })
    })
}

/// The target camera seeing `camera`'s view mirrored in the level plane at
/// height `level`, clipped to what's above it (Rust
/// `TargetCamera::reflection`). Read its target at the screen position
/// flipped upside down.
/// # Safety
/// `ctx` null or live; `camera` valid; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_reflection_camera(std::ptr::null_mut(), std::ptr::null(), ponko_ffi::PonkoTexture { id: 0, width: 1, height: 1 }, 0.0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_reflection_camera(
    ctx: *mut PonkoCtx,
    camera: *const PonkoCamera3D,
    target: PonkoTexture,
    level: f32,
    out_camera: *mut PonkoTargetCamera,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(cam) = camera.as_ref() else {
                return Err(invalid("camera is null"));
            };
            if out_camera.is_null() {
                return Err(invalid("output is null"));
            }
            if !level.is_finite() {
                return Err(invalid("level must be finite"));
            }
            let eye = camera_from_c(c.scene3d.camera, cam)?;
            let reflection = TargetCamera::reflection(eye, texture_from_c(target), level);
            out(out_camera, target_camera_to_c(&reflection))
        })
    })
}

/// Replaces the target cameras drawn each frame with `count` of them
/// (Rust `ctx.scene3d.passes.target_cameras`); 0 draws none. Each target must come from
/// `ponko_ctx_create_render_target`.
/// # Safety
/// `ctx` null or live; `cameras` holds `count` values (may be null when 0).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_target_cameras(std::ptr::null_mut(), std::ptr::null(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_target_cameras(
    ctx: *mut PonkoCtx,
    cameras: *const PonkoTargetCamera,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if cameras.is_null() && count > 0 {
                return Err(invalid("cameras is null"));
            }
            let given = if count == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(cameras, count)
            };
            let mut list = Vec::with_capacity(count);
            for t in given {
                let target = texture_from_c(t.target);
                if !c.gfx.is_render_target(target) {
                    return Err(invalid("that texture isn't a render target of this game"));
                }
                let plane = Vec4::from_array(t.clip_plane);
                if t.has_clip_plane && !(plane.is_finite() && plane.truncate().length_squared() > 0.0) {
                    return Err(invalid("the clip plane needs a finite, nonzero normal"));
                }
                list.push(TargetCamera {
                    camera: camera_from_c(c.scene3d.camera, &t.camera)?,
                    target,
                    clip_plane: t.has_clip_plane.then_some(plane),
                });
            }
            c.scene3d.passes.target_cameras = list;
            Ok(())
        })
    })
}

/// The target cameras drawn each frame: writes up to `capacity` of them
/// to `out_cameras` and how many there are to `out_count` (Rust
/// `ctx.scene3d.passes.target_cameras`).
/// # Safety
/// `ctx` null or live; `out_cameras` holds `capacity` values (may be null
/// when 0); `out_count` writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_target_cameras(std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_target_cameras(
    ctx: *mut PonkoCtx,
    out_cameras: *mut PonkoTargetCamera,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_count.is_null() || (out_cameras.is_null() && capacity > 0) {
                return Err(invalid("output is null"));
            }
            for (i, t) in c.scene3d.passes.target_cameras.iter().take(capacity).enumerate() {
                out_cameras.add(i).write(target_camera_to_c(t));
            }
            out(out_count, c.scene3d.passes.target_cameras.len())
        })
    })
}

/// Reads the 3D camera.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_camera3d(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_camera3d(ctx: *mut PonkoCtx, out_camera: *mut PonkoCamera3D) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_camera, camera_to_c(&c.scene3d.camera))) })
}

/// Moves or turns the 3D camera. `fov_y` must be between 0 and pi, and
/// `0 < near < far`.
/// # Safety
/// `ctx` null or live; `camera` valid.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_camera3d(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_camera3d(ctx: *mut PonkoCtx, camera: *const PonkoCamera3D) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(cam) = camera.as_ref() else {
                return Err(invalid("camera is null"));
            };
            c.scene3d.camera = camera_from_c(c.scene3d.camera, cam)?;
            Ok(())
        })
    })
}

/// Turns occlusion culling on or off: models hidden behind what was drawn a
/// frame or two before aren't drawn (they still cast shadows). Off by
/// default. A model coming out from behind something can show up a frame or
/// two late when the camera moves fast. Without compute shaders (WebGL2)
/// nothing is culled.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_occlusion_culling(std::ptr::null_mut(), true) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_occlusion_culling(ctx: *mut PonkoCtx, on: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.scene3d.camera.occlusion_culling = on;
            Ok(())
        })
    })
}

/// Whether occlusion culling is on (see `ponko_ctx_set_occlusion_culling`).
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_occlusion_culling(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_occlusion_culling(ctx: *mut PonkoCtx, out_on: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_on, c.scene3d.camera.occlusion_culling)) })
}

/// Turns the 3D camera to look at `target`, +Y up.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_camera3d_look_at(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_camera3d_look_at(ctx: *mut PonkoCtx, target: PonkoVec3) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.scene3d.camera.look_at(to_c(target));
            Ok(())
        })
    })
}

/// The ray from the 3D camera through a window point (logical pixels, as
/// the mouse position): where it starts and its unit direction. For
/// picking things with the mouse.
/// # Safety
/// `ctx` null or live; outputs writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_screen_ray(std::ptr::null_mut(), Default::default(), std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_screen_ray(
    ctx: *mut PonkoCtx,
    screen: PonkoVec2,
    out_origin: *mut PonkoVec3,
    out_direction: *mut PonkoVec3,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if out_origin.is_null() || out_direction.is_null() {
                return Err(invalid("output is null"));
            }
            let (origin, direction) = c.scene3d.camera.screen_ray(to_c(screen));
            out(out_origin, to_c(origin))?;
            out(out_direction, to_c(direction))
        })
    })
}

/// Reads the sun and sky light.
/// # Safety
/// `ctx` null or live; output writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_lighting(ctx: *mut PonkoCtx, out_lighting: *mut PonkoLighting) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let (l, sky) = (&c.scene3d.lighting, &c.scene3d.sky);
            out(
                out_lighting,
                PonkoLighting {
                    sun_direction: to_c(l.sun_direction),
                    sun_color: to_c(l.sun_color),
                    sun_intensity: l.sun_intensity,
                    sky_color: to_c(sky.sky_color),
                    ground_color: to_c(sky.ground_color),
                    ambient_intensity: l.ambient_intensity,
                    shadows: l.shadows,
                    shadow_distance: l.shadow_distance,
                    shadow_cascades: l.shadow_cascades,
                    shadow_resolution: l.shadow_resolution,
                    light_shadow_views: l.light_shadow_views,
                    light_shadow_resolution: l.light_shadow_resolution,
                    light_shadow_atlas: l.light_shadow_atlas,
                    shadow_refresh: l.shadow_refresh,
                    shadow_filter: l.shadow_filter as u32,
                    has_equator: sky.equator_color.is_some(),
                    equator_color: to_c(sky.equator_color.unwrap_or(sky.sky_color)),
                    gi_enabled: l.gi.enabled,
                    gi_extent: to_c(l.gi.extent),
                    gi_spacing: l.gi.spacing,
                    gi_probes_per_frame: l.gi.probes_per_frame,
                    gi_intensity: l.gi.intensity,
                },
            )
        })
    })
}

/// Changes the sun and sky light. Intensities and the shadow distance
/// can't be negative; `shadow_cascades` is 1 to 4, `shadow_refresh` 1 to 8, `shadow_filter` 1 to 3; the shadow resolutions
/// and `light_shadow_views` must be in their ranges (see `PonkoLighting`).
/// With `gi_enabled`, `gi_extent` and `gi_spacing` must be above zero,
/// `gi_probes_per_frame` 1 to 16 and `gi_intensity` not negative.
/// # Safety
/// `ctx` null or live; `lighting` valid.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_lighting(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_lighting(ctx: *mut PonkoCtx, lighting: *const PonkoLighting) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(&l) = lighting.as_ref() else {
                return Err(invalid("lighting is null"));
            };
            if !(l.sun_intensity >= 0.0 && l.ambient_intensity >= 0.0 && l.shadow_distance >= 0.0) {
                return Err(invalid("intensities and shadow distance can't be negative"));
            }
            if !(1..=4).contains(&l.shadow_cascades) {
                return Err(invalid("shadow_cascades must be 1 to 4"));
            }
            if !(1..=8).contains(&l.shadow_refresh) {
                return Err(invalid("shadow_refresh must be 1 to 8"));
            }
            let shadow_filter = shadow_filter_from_c(l.shadow_filter)?;
            if !(256..=4096).contains(&l.shadow_resolution)
                || !(128..=2048).contains(&l.light_shadow_resolution)
                || l.light_shadow_views > ponko::d3::PointLight::MAX_SHADOW_VIEWS as u32
            {
                return Err(invalid(
                    "shadow_resolution must be 256 to 4096, light_shadow_resolution 128 to 2048 and light_shadow_views 0 to 24",
                ));
            }
            if !light_atlas_ok(l.light_shadow_atlas) {
                return Err(invalid("light_shadow_atlas must be a power of two from 1024 to 8192"));
            }
            let extent: Vec3 = to_c(l.gi_extent);
            if l.gi_enabled
                && !(extent.is_finite()
                    && extent.min_element() > 0.0
                    && l.gi_spacing.is_finite()
                    && l.gi_spacing > 0.0
                    && (1..=ponko::render::GlobalIllumination::MAX_PROBES_PER_FRAME).contains(&l.gi_probes_per_frame)
                    && l.gi_intensity.is_finite()
                    && l.gi_intensity >= 0.0)
            {
                return Err(invalid(
                    "gi_extent and gi_spacing must be above zero, gi_probes_per_frame 1 to 16 and gi_intensity not negative",
                ));
            }
            let color = |c: PonkoColor| -> Color { to_c(c) };
            // The ambient colors live on the sky in Rust; the C struct keeps them.
            let sky = &mut c.scene3d.sky;
            sky.sky_color = color(l.sky_color);
            sky.ground_color = color(l.ground_color);
            sky.equator_color = l.has_equator.then(|| color(l.equator_color));
            let lighting = &mut c.scene3d.lighting;
            lighting.sun_direction = to_c(l.sun_direction);
            lighting.sun_color = color(l.sun_color);
            lighting.sun_intensity = l.sun_intensity;
            lighting.ambient_intensity = l.ambient_intensity;
            lighting.shadows = l.shadows;
            lighting.shadow_distance = l.shadow_distance;
            lighting.shadow_cascades = l.shadow_cascades;
            lighting.shadow_resolution = l.shadow_resolution;
            lighting.light_shadow_views = l.light_shadow_views;
            lighting.light_shadow_resolution = l.light_shadow_resolution;
            lighting.light_shadow_atlas = l.light_shadow_atlas;
            lighting.shadow_refresh = l.shadow_refresh;
            lighting.shadow_filter = shadow_filter;
            lighting.gi = GlobalIllumination {
                enabled: l.gi_enabled,
                extent,
                spacing: l.gi_spacing,
                probes_per_frame: l.gi_probes_per_frame,
                intensity: l.gi_intensity,
            };
            Ok(())
        })
    })
}

/// Reads the sky and fog settings.
/// # Safety
/// `ctx` null or live; output writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_sky(ctx: *mut PonkoCtx, out_sky: *mut PonkoSky) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let s = &c.scene3d.sky;
            out(
                out_sky,
                PonkoSky {
                    atmosphere: s.atmosphere,
                    haze: s.haze,
                    sun_size: s.sun_size,
                    stars: s.stars,
                    night_light: to_c(s.night_light),
                    fog_density: s.fog_density,
                    fog_height: s.fog_height,
                    fog_falloff: s.fog_falloff,
                    fog_color: to_c(s.fog_color),
                    fog_start: s.fog_start,
                    fog_end: s.fog_end,
                    fog_sun_glow: s.fog_sun_glow,
                    fog_sun_focus: s.fog_sun_focus,
                    light_shafts: s.light_shafts,
                },
            )
        })
    })
}

/// Changes the sky and fog. Haze, stars, fog density, the distance fog's
/// distances and the sun glow can't be negative; the sun's size and the fog
/// falloff must be above 0; the glow's focus is from 0 to under 1; light
/// shafts are from 0 to 1.
/// # Safety
/// `ctx` null or live; `sky` valid.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_sky(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_sky(ctx: *mut PonkoCtx, sky: *const PonkoSky) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(&s) = sky.as_ref() else {
                return Err(invalid("sky is null"));
            };
            let finite = [
                s.haze,
                s.sun_size,
                s.stars,
                s.fog_density,
                s.fog_height,
                s.fog_falloff,
                s.fog_start,
                s.fog_end,
                s.fog_sun_glow,
                s.fog_sun_focus,
                s.light_shafts,
            ]
            .iter()
            .all(|v| v.is_finite());
            let negative = [s.haze, s.stars, s.fog_density, s.fog_start, s.fog_end, s.fog_sun_glow]
                .iter()
                .any(|&v| v < 0.0);
            if !finite || negative {
                return Err(invalid(
                    "haze, stars, fog density, distances and glow can't be negative",
                ));
            }
            if !(0.0..1.0).contains(&s.fog_sun_focus) {
                return Err(invalid("fog_sun_focus must be from 0 to under 1"));
            }
            if !(0.0..=1.0).contains(&s.light_shafts) {
                return Err(invalid("light_shafts must be from 0 to 1"));
            }
            if s.sun_size <= 0.0 || s.fog_falloff <= 0.0 {
                return Err(invalid("sun_size and fog_falloff must be above 0"));
            }
            // Every field but the ambient colors, which `ponko_ctx_set_lighting` sets.
            let sky = &mut c.scene3d.sky;
            sky.atmosphere = s.atmosphere;
            sky.haze = s.haze;
            sky.sun_size = s.sun_size;
            sky.stars = s.stars;
            sky.night_light = to_c(s.night_light);
            sky.fog_density = s.fog_density;
            sky.fog_height = s.fog_height;
            sky.fog_falloff = s.fog_falloff;
            sky.fog_color = to_c(s.fog_color);
            sky.fog_start = s.fog_start;
            sky.fog_end = s.fog_end;
            sky.fog_sun_glow = s.fog_sun_glow;
            sky.fog_sun_focus = s.fog_sun_focus;
            sky.light_shafts = s.light_shafts;
            Ok(())
        })
    })
}

/// Reads the 3D post-processing settings.
/// # Safety
/// `ctx` null or live; output writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_post(ctx: *mut PonkoCtx, out_post: *mut PonkoPost) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let p = &c.scene3d.post;
            out(
                out_post,
                PonkoPost {
                    render_scale: p.render_scale,
                    sharpen: p.sharpen,
                    anti_aliasing: p.anti_aliasing as u32,
                    ao_enabled: p.ambient_occlusion.enabled,
                    ao_radius: p.ambient_occlusion.radius,
                    ao_intensity: p.ambient_occlusion.intensity,
                    ao_slices: p.ambient_occlusion.slices,
                    ao_steps: p.ambient_occlusion.steps,
                    ao_half_resolution: p.ao_half_resolution,
                    ao_temporal: p.ao_temporal,
                    exposure: p.exposure,
                    tonemap: p.tonemap as u32,
                    bloom: p.bloom,
                    bloom_threshold: p.bloom_threshold,
                    contrast: p.contrast,
                    saturation: p.saturation,
                    temperature: p.temperature,
                    tint: p.tint,
                    vignette: p.vignette,
                    chromatic_aberration: p.chromatic_aberration,
                    lens_distortion: p.lens_distortion,
                    lens_scale: p.lens_scale,
                    has_lut: p.lut.is_some(),
                    lut: p.lut.map(texture_to_c).unwrap_or(PonkoTexture {
                        id: 0,
                        width: 0,
                        height: 0,
                    }),
                    taa_sharpness: p.taa_sharpness,
                    contact_shadows: p.contact_shadows,
                    ssr: p.ssr,
                    ssr_max_distance: p.ssr_max_distance,
                },
            )
        })
    })
}

/// Changes the 3D post-processing settings. `tonemap` must be 0 to 3;
/// bloom, its threshold, contrast and saturation can't be negative;
/// `render_scale` is 0.25 to 2 and `sharpen` 0 to 1; `contact_shadows`
/// can't be negative; `ssr` is 0 to 1 and `ssr_max_distance` above zero;
/// `chromatic_aberration` is 0 to 1, `lens_distortion` -1 to 1 and
/// `lens_scale` above zero.
/// # Safety
/// `ctx` null or live; `post` valid.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_post(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_post(ctx: *mut PonkoCtx, post: *const PonkoPost) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(&p) = post.as_ref() else {
                return Err(invalid("post is null"));
            };
            let anti_aliasing = match p.anti_aliasing {
                0 => AntiAliasing::None,
                1 => AntiAliasing::Fxaa,
                2 => AntiAliasing::Smaa,
                3 => AntiAliasing::Taa,
                _ => return Err(invalid("anti_aliasing must be 0 to 3")),
            };
            if !(0.0..=1.0).contains(&p.taa_sharpness) {
                return Err(invalid("taa_sharpness must be 0 to 1"));
            }
            if !(p.contact_shadows.is_finite() && p.contact_shadows >= 0.0) {
                return Err(invalid("contact_shadows can't be negative"));
            }
            if !(0.0..=1.0).contains(&p.ssr) || !(p.ssr_max_distance.is_finite() && p.ssr_max_distance > 0.0) {
                return Err(invalid("ssr must be 0 to 1 and ssr_max_distance above zero"));
            }
            if !p.ao_radius.is_finite()
                || p.ao_radius <= 0.0
                || !p.ao_intensity.is_finite()
                || p.ao_intensity < 0.0
                || !(1..=8).contains(&p.ao_slices)
                || !(1..=16).contains(&p.ao_steps)
            {
                return Err(invalid(
                    "AO requires positive finite radius, nonnegative finite intensity, 1..8 slices and 1..16 steps",
                ));
            }
            if !(0.0..=1.0).contains(&p.chromatic_aberration)
                || !(-1.0..=1.0).contains(&p.lens_distortion)
                || !(p.lens_scale.is_finite() && p.lens_scale > 0.0)
            {
                return Err(invalid(
                    "chromatic_aberration must be 0 to 1, lens_distortion -1 to 1 and lens_scale above zero",
                ));
            }
            if !(ponko::render::Post::MIN_RENDER_SCALE..=ponko::render::Post::MAX_RENDER_SCALE)
                .contains(&p.render_scale)
                || !(0.0..=1.0).contains(&p.sharpen)
            {
                return Err(invalid("render_scale must be 0.25 to 2 and sharpen 0 to 1"));
            }
            let tonemap = match p.tonemap {
                0 => Tonemap::Aces,
                1 => Tonemap::AgX,
                2 => Tonemap::Neutral,
                3 => Tonemap::None,
                _ => return Err(invalid("tonemap must be 0 to 3")),
            };
            let finite = [
                p.exposure,
                p.bloom,
                p.bloom_threshold,
                p.contrast,
                p.saturation,
                p.temperature,
                p.tint,
                p.vignette,
            ]
            .iter()
            .all(|v| v.is_finite());
            if !finite || p.bloom < 0.0 || p.bloom_threshold < 0.0 || p.contrast < 0.0 || p.saturation < 0.0 {
                return Err(invalid("bloom, threshold, contrast and saturation can't be negative"));
            }
            // Every field but the MSAA sample count, which C sets with `ponko_game_msaa`.
            let post = &mut c.scene3d.post;
            post.render_scale = p.render_scale;
            post.sharpen = p.sharpen;
            post.anti_aliasing = anti_aliasing;
            post.taa_sharpness = p.taa_sharpness;
            post.ambient_occlusion = AmbientOcclusion {
                enabled: p.ao_enabled,
                radius: p.ao_radius,
                intensity: p.ao_intensity,
                slices: p.ao_slices,
                steps: p.ao_steps,
            };
            post.ao_half_resolution = p.ao_half_resolution;
            post.ao_temporal = p.ao_temporal;
            post.exposure = p.exposure;
            post.tonemap = tonemap;
            post.bloom = p.bloom;
            post.bloom_threshold = p.bloom_threshold;
            post.contrast = p.contrast;
            post.saturation = p.saturation;
            post.temperature = p.temperature;
            post.tint = p.tint;
            post.vignette = p.vignette;
            post.chromatic_aberration = p.chromatic_aberration;
            post.lens_distortion = p.lens_distortion;
            post.lens_scale = p.lens_scale;
            post.lut = p.has_lut.then(|| texture_from_c(p.lut));
            post.contact_shadows = p.contact_shadows;
            post.ssr = p.ssr;
            post.ssr_max_distance = p.ssr_max_distance;
            Ok(())
        })
    })
}

/// The settings that decide how heavy the 3D picture is, in one struct
/// (Rust `GraphicsQuality`). Presets: `ponko_graphics_quality_preset`.
/// Every field is also a field of `PonkoPost` or `PonkoLighting`; applying
/// this writes those and leaves their other fields alone.
/// ```
/// let mut q = std::mem::MaybeUninit::<ponko_ffi::PonkoGraphicsQuality>::uninit();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_graphics_quality(std::ptr::null_mut(), q.as_mut_ptr()) } as u32, 1);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoGraphicsQuality {
    /// The 3D scene's resolution relative to the window, 0.25 to 2.
    pub render_scale: f32,
    /// Sharpening of the scaled image, 0 to 1.
    pub sharpen: f32,
    /// Whether anything casts shadows.
    pub shadows: bool,
    /// How far from the camera shadows reach.
    pub shadow_distance: f32,
    /// Sun shadow cascades, 1 to 4.
    pub shadow_cascades: u32,
    /// Width of each sun shadow map, 256 to 4096.
    pub shadow_resolution: u32,
    /// Shadow views point and spot lights share, 0 to 24.
    pub light_shadow_views: u32,
    /// Width of the largest point or spot light shadow view, 128 to 2048.
    pub light_shadow_resolution: u32,
    /// Width of the light shadow atlas: a power of two, 1024 to 8192.
    pub light_shadow_atlas: u32,
    /// Ambient occlusion on or off.
    pub ambient_occlusion: bool,
    /// AO angular slices, 1 to 8.
    pub ao_slices: u32,
    /// AO horizon steps per side, 1 to 16.
    pub ao_steps: u32,
    /// Half-resolution GTAO with bilateral upsampling; contacts stay full resolution.
    pub ao_half_resolution: bool,
    /// Camera-reprojected GTAO accumulation (independent of TAA).
    pub ao_temporal: bool,
    /// Post AA: 0 none, 1 FXAA, 2 SMAA 1x, 3 temporal.
    pub anti_aliasing: u32,
    /// How often the far shadow cascades are drawn again, in frames, 1 to 8.
    pub shadow_refresh: u32,
    /// How soft the sun's shadow edges are, 1 (one lookup) to 3 (nine).
    pub shadow_filter: u32,
    /// Screen-space reflections' strength, 0 (off) to 1.
    pub ssr: f32,
    /// Global illumination (bounced light) on or off.
    pub gi: bool,
}

pub(crate) fn quality_to_c(s: &SceneSettings) -> PonkoGraphicsQuality {
    let (l, p) = (&s.lighting, &s.post);
    PonkoGraphicsQuality {
        render_scale: p.render_scale,
        sharpen: p.sharpen,
        shadows: l.shadows,
        shadow_distance: l.shadow_distance,
        shadow_cascades: l.shadow_cascades,
        shadow_resolution: l.shadow_resolution,
        light_shadow_views: l.light_shadow_views,
        light_shadow_resolution: l.light_shadow_resolution,
        light_shadow_atlas: l.light_shadow_atlas,
        ambient_occlusion: p.ambient_occlusion.enabled,
        ao_half_resolution: p.ao_half_resolution,
        ao_temporal: p.ao_temporal,
        ao_slices: p.ambient_occlusion.slices,
        ao_steps: p.ambient_occlusion.steps,
        anti_aliasing: p.anti_aliasing as u32,
        shadow_refresh: l.shadow_refresh,
        shadow_filter: l.shadow_filter as u32,
        ssr: p.ssr,
        gi: l.gi.enabled,
    }
}

/// The quality fields `apply` writes into default scene settings: Rust
/// keeps quality in the scene settings, not in a struct of its own.
pub(crate) fn scratch_quality(apply: impl FnOnce(&mut SceneSettings)) -> PonkoGraphicsQuality {
    let mut settings = SceneSettings::default();
    apply(&mut settings);
    quality_to_c(&settings)
}

/// Whether a light shadow atlas size is allowed: a power of two, 1024 to 8192.
fn light_atlas_ok(size: u32) -> bool {
    (1024..=8192).contains(&size) && size.is_power_of_two()
}

/// The sun shadow filter for C's 1 (sharp) to 3 (softest).
fn shadow_filter_from_c(filter: u32) -> Result<ShadowFilter, crate::Failure> {
    match filter {
        1 => Ok(ShadowFilter::Sharp),
        2 => Ok(ShadowFilter::Soft),
        3 => Ok(ShadowFilter::Softest),
        _ => Err(invalid("shadow_filter must be 1 to 3")),
    }
}

/// Checks every field of a C quality struct against its range and, only
/// when all are valid, writes them into `settings`' lighting and post.
fn quality_from_c(q: &PonkoGraphicsQuality, settings: &mut SceneSettings) -> Result<(), crate::Failure> {
    let anti_aliasing = match q.anti_aliasing {
        0 => AntiAliasing::None,
        1 => AntiAliasing::Fxaa,
        2 => AntiAliasing::Smaa,
        3 => AntiAliasing::Taa,
        _ => return Err(invalid("anti_aliasing must be 0 to 3")),
    };
    if !(ponko::render::Post::MIN_RENDER_SCALE..=ponko::render::Post::MAX_RENDER_SCALE).contains(&q.render_scale)
        || !(0.0..=1.0).contains(&q.sharpen)
    {
        return Err(invalid("render_scale must be 0.25 to 2 and sharpen 0 to 1"));
    }
    if !q.shadow_distance.is_finite() || q.shadow_distance < 0.0 || !(1..=4).contains(&q.shadow_cascades) {
        return Err(invalid(
            "shadow_distance can't be negative and shadow_cascades is 1 to 4",
        ));
    }
    if !(256..=4096).contains(&q.shadow_resolution)
        || !(128..=2048).contains(&q.light_shadow_resolution)
        || q.light_shadow_views > ponko::d3::PointLight::MAX_SHADOW_VIEWS as u32
    {
        return Err(invalid(
            "shadow_resolution must be 256 to 4096, light_shadow_resolution 128 to 2048 and light_shadow_views 0 to 24",
        ));
    }
    if !light_atlas_ok(q.light_shadow_atlas) {
        return Err(invalid("light_shadow_atlas must be a power of two from 1024 to 8192"));
    }
    if !(1..=8).contains(&q.ao_slices) || !(1..=16).contains(&q.ao_steps) {
        return Err(invalid("ao_slices must be 1 to 8 and ao_steps 1 to 16"));
    }
    if !(1..=8).contains(&q.shadow_refresh) {
        return Err(invalid("shadow_refresh must be 1 to 8"));
    }
    let shadow_filter = shadow_filter_from_c(q.shadow_filter)?;
    if !(0.0..=1.0).contains(&q.ssr) {
        return Err(invalid("ssr must be 0 to 1"));
    }
    let (l, p) = (&mut settings.lighting, &mut settings.post);
    p.render_scale = q.render_scale;
    p.sharpen = q.sharpen;
    p.anti_aliasing = anti_aliasing;
    p.ambient_occlusion.enabled = q.ambient_occlusion;
    p.ambient_occlusion.slices = q.ao_slices;
    p.ambient_occlusion.steps = q.ao_steps;
    p.ao_half_resolution = q.ao_half_resolution;
    p.ao_temporal = q.ao_temporal;
    p.ssr = q.ssr;
    l.gi.enabled = q.gi;
    l.shadows = q.shadows;
    l.shadow_distance = q.shadow_distance;
    l.shadow_cascades = q.shadow_cascades;
    l.shadow_resolution = q.shadow_resolution;
    l.shadow_refresh = q.shadow_refresh;
    l.shadow_filter = shadow_filter;
    l.light_shadow_views = q.light_shadow_views;
    l.light_shadow_resolution = q.light_shadow_resolution;
    l.light_shadow_atlas = q.light_shadow_atlas;
    Ok(())
}

/// C's preset level for `quality`: 0 low, 1 medium, 2 high, 3 ultra; 4
/// (custom) for a preset C doesn't know.
fn level_to_c(quality: GraphicsQuality) -> u32 {
    match quality {
        GraphicsQuality::Low => 0,
        GraphicsQuality::Medium => 1,
        GraphicsQuality::High => 2,
        GraphicsQuality::Ultra => 3,
        _ => 4,
    }
}

/// Writes preset `level` into `out_quality`: 0 low, 1 medium, 2 high, 3
/// ultra (Rust `GraphicsQuality::Low` and so on, applied with
/// `SceneSettings::set_quality`). Any other level is
/// `PONKO_INVALID_ARGUMENT`.
/// # Safety
/// `out_quality` writable.
/// ```
/// let mut q = std::mem::MaybeUninit::<ponko_ffi::PonkoGraphicsQuality>::uninit();
/// assert_eq!(unsafe { ponko_ffi::ponko_graphics_quality_preset(0, q.as_mut_ptr()) } as u32, 0);
/// assert_eq!(unsafe { q.assume_init() }.shadow_cascades, 2);
/// assert_eq!(unsafe { ponko_ffi::ponko_graphics_quality_preset(4, q.as_mut_ptr()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_graphics_quality_preset(
    level: u32,
    out_quality: *mut PonkoGraphicsQuality,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let quality = match level {
            0 => GraphicsQuality::Low,
            1 => GraphicsQuality::Medium,
            2 => GraphicsQuality::High,
            3 => GraphicsQuality::Ultra,
            _ => return Err(invalid("level must be 0 (low), 1 (medium), 2 (high) or 3 (ultra)")),
        };
        out(out_quality, scratch_quality(|s| s.set_quality(quality)))
    })
}

/// Which preset `quality` is: 0 low, 1 medium, 2 high, 3 ultra, or 4 when
/// it is none of them (Rust `SceneSettings::quality`, `None` for custom).
/// `quality` must be valid (see `ponko_ctx_set_graphics_quality`).
/// # Safety
/// `quality` valid; `out_level` writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_graphics_quality_level(std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_graphics_quality_level(
    quality: *const PonkoGraphicsQuality,
    out_level: *mut u32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let Some(q) = quality.as_ref() else {
            return Err(invalid("quality is null"));
        };
        let mut settings = SceneSettings::default();
        quality_from_c(q, &mut settings)?;
        out(out_level, settings.quality().map_or(4, level_to_c))
    })
}

/// Reads the current graphics quality, gathered from the post and lighting
/// settings (Rust `ctx.scene3d.post` and `ctx.scene3d.lighting`) as the
/// renderer draws them (Rust `SceneSettings::clamp_quality`): always in
/// range, so `ponko_ctx_set_graphics_quality` takes it back.
/// # Safety
/// `ctx` null or live; output writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_graphics_quality(
    ctx: *mut PonkoCtx,
    out_quality: *mut PonkoGraphicsQuality,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let quality = scratch_quality(|s| {
                (s.lighting, s.post) = (c.scene3d.lighting, c.scene3d.post);
                s.clamp_quality();
            });
            out(out_quality, quality)
        })
    })
}

/// Switches the 3D picture to `quality` without a restart (like Rust
/// `ctx.scene3d.set_quality`): writes the matching post and lighting
/// settings and pins render scale and sun shadow resolution against
/// automatic quality; the next frame draws at the new resolution and
/// shadow map sizes. Fields outside their ranges (see
/// `PonkoGraphicsQuality`) are `PONKO_INVALID_ARGUMENT` and change nothing.
/// # Safety
/// `ctx` null or live; `quality` valid.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_graphics_quality(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_graphics_quality(
    ctx: *mut PonkoCtx,
    quality: *const PonkoGraphicsQuality,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let Some(q) = quality.as_ref() else {
                return Err(invalid("quality is null"));
            };
            quality_from_c(q, &mut c.scene3d)?;
            c.scene3d.pin_render_scale(q.render_scale);
            c.scene3d.pin_shadow_resolution(q.shadow_resolution);
            Ok(())
        })
    })
}

/// Makes TAA (post `anti_aliasing` 3) and temporal AO forget the frames
/// before this one: call it after camera cuts or object teleports. The engine
/// also resets on resize and when the camera jumps or turns sharply.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_reset_temporal_history(std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_reset_temporal_history(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.reset_temporal_history();
            Ok(())
        })
    })
}

/// A terrain tile's heights for finding the ground on the CPU (Rust
/// `Heightfield`). Make it with `ponko_heightfield_new`.
pub struct PonkoHeightfield(ponko::terrain::Heightfield);

/// Makes a heightfield `size` wide from `count` heights (n x n, as
/// `ponko_ctx_create_terrain_tile` takes them). Returns null (see
/// `ponko_last_error`) for bad input.
/// ```
/// let h = [0.0f32; 9];
/// let g = unsafe { ponko_ffi::ponko_heightfield_new(h.as_ptr(), 9, 4.0) };
/// assert!(!g.is_null());
/// unsafe { ponko_ffi::ponko_heightfield_free(g) };
/// assert!(unsafe { ponko_ffi::ponko_heightfield_new(h.as_ptr(), 8, 4.0) }.is_null());
/// ```
/// # Safety
/// `heights` holds `count` floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_heightfield_new(heights: *const f32, count: usize, size: f32) -> *mut PonkoHeightfield {
    let mut made = std::ptr::null_mut();
    let status = boundary(|| {
        if heights.is_null() {
            return Err(invalid("heights is null"));
        }
        if !square_grid(count) {
            return Err(invalid("the heights aren't a square grid of at least 2 x 2"));
        }
        if !(size > 0.0 && size.is_finite()) {
            return Err(invalid("size must be above 0"));
        }
        // SAFETY: caller supplies `count` floats.
        let heights = unsafe { std::slice::from_raw_parts(heights, count) };
        made = Box::into_raw(Box::new(PonkoHeightfield(ponko::terrain::Heightfield::new(
            heights, size,
        ))));
        Ok(())
    });
    if status == PonkoStatus::PONKO_OK {
        made
    } else {
        std::ptr::null_mut()
    }
}

/// Frees a heightfield. Null is ignored.
/// ```
/// unsafe { ponko_ffi::ponko_heightfield_free(std::ptr::null_mut()) };
/// ```
/// # Safety
/// `ground` must be null or live, and not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_heightfield_free(ground: *mut PonkoHeightfield) {
    if !ground.is_null() {
        // SAFETY: caller passes a heightfield from ponko_heightfield_new, once.
        drop(unsafe { Box::from_raw(ground) });
    }
}

/// The ground's height and facing at `(x, z)` from the tile's corner,
/// following the drawn triangles. `out_found` is false off the tile or
/// over a hole (the others are then left alone). `out_normal` may be null.
/// Safe from any thread at once.
/// ```
/// use ponko_ffi::*;
/// let h = [0.0f32, 2.0, 0.0, 2.0];
/// let (mut found, mut y, mut n) = (false, 0.0, PonkoVec3::default());
/// unsafe {
///     let g = ponko_heightfield_new(h.as_ptr(), 4, 1.0);
///     assert_eq!(ponko_heightfield_at(g, 0.5, 0.5, &mut found, &mut y, &mut n), PonkoStatus::PONKO_OK);
///     assert!(found && (y - 1.0).abs() < 1e-6 && n.x < 0.0);
///     ponko_heightfield_at(g, 5.0, 0.5, &mut found, &mut y, std::ptr::null_mut());
///     assert!(!found);
///     ponko_heightfield_free(g);
/// }
/// ```
/// # Safety
/// `ground` null or live; `out_found` and `out_height` writable; `out_normal` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_heightfield_at(
    ground: *const PonkoHeightfield,
    x: f32,
    z: f32,
    out_found: *mut bool,
    out_height: *mut f32,
    out_normal: *mut PonkoVec3,
) -> PonkoStatus {
    boundary(|| {
        // SAFETY: caller supplies a live heightfield.
        let Some(ground) = (unsafe { ground.as_ref() }) else {
            return Err(invalid("heightfield is null"));
        };
        if out_found.is_null() || out_height.is_null() {
            return Err(invalid("output is null"));
        }
        let height = ground.0.height_at(x, z);
        // SAFETY: outputs checked non-null (normal below).
        unsafe {
            out_found.write(height.is_some());
            if let Some(h) = height {
                out_height.write(h);
                if !out_normal.is_null()
                    && let Some(n) = ground.0.normal_at(x, z)
                {
                    out_normal.write(PonkoVec3 { x: n.x, y: n.y, z: n.z });
                }
            }
        }
        Ok(())
    })
}

/// Gives `entity` a different shadow than its model's (Rust `ShadowMesh`):
/// `mesh` is drawn into the shadow maps instead, or with `casts` false, the
/// entity casts none. An entity with a shadow mesh and no `PonkoModel`
/// casts only that shadow; on a terrain tile, `casts` false turns the
/// ground's shadow off. Stale entities are an error.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_shadow_mesh(std::ptr::null_mut(), 0, Default::default(), false) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_shadow_mesh(
    ctx: *mut PonkoCtx,
    entity: u64,
    mesh: PonkoMesh,
    casts: bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = ponko::prelude::Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            c.world
                .insert(entity, ponko::render::ShadowMesh(casts.then(|| to_c(mesh))));
            Ok(())
        })
    })
}

/// Draws `entity`'s model with `material` instead of the ones its mesh was
/// made with, every part (Rust `ModelMaterial`); with `enabled` false, with
/// its mesh's own again. One uploaded mesh can so be drawn in several
/// materials. It applies to plain, skinned and morphing models, LOD groups
/// and terrain tiles. Stale entities and materials are an error.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_model_material(std::ptr::null_mut(), 0, Default::default(), false) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_model_material(
    ctx: *mut PonkoCtx,
    entity: u64,
    material: PonkoMaterial,
    enabled: bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = ponko::prelude::Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            if !enabled {
                c.world.remove::<ponko::render::ModelMaterial>(entity);
                return Ok(());
            }
            if !c.gfx.is_live(to_c(material)) {
                return Err(invalid("that material wasn't made by this game"));
            }
            c.world.insert(entity, ponko::render::ModelMaterial(to_c(material)));
            Ok(())
        })
    })
}

/// The material set with `ponko_ctx_set_model_material`: `out_has` says
/// whether `entity` has one, `out_material` (optional) is it.
/// # Safety
/// `ctx` null or live; outputs writable or null as documented.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_model_material(std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_model_material(
    ctx: *mut PonkoCtx,
    entity: u64,
    out_material: *mut PonkoMaterial,
    out_has: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = ponko::prelude::Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            if out_has.is_null() {
                return Err(invalid("output is null"));
            }
            let found = c.world.get::<ponko::render::ModelMaterial>(entity).copied();
            if let (Some(m), false) = (found, out_material.is_null()) {
                *out_material = to_c(m.0);
            }
            out(out_has, found.is_some())
        })
    })
}

/// Draws some parts of `entity`'s model with other materials (Rust
/// `ModelMaterials`): part `parts[i]` with `materials[i]`, for `count`
/// pairs. Parts are numbered from 0 in the order the mesh was made with;
/// parts not listed keep the material of `ponko_ctx_set_model_material`,
/// or the mesh's own. A part listed twice takes the last; a part the mesh
/// doesn't have is ignored. `count` 0 takes the list off. Stale entities
/// and materials are an error.
/// # Safety
/// `ctx` null or live; `parts` and `materials` hold `count` values each.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_model_part_materials(std::ptr::null_mut(), 0, std::ptr::null(), std::ptr::null(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_model_part_materials(
    ctx: *mut PonkoCtx,
    entity: u64,
    parts: *const u32,
    materials: *const PonkoMaterial,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = ponko::prelude::Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            if count == 0 {
                c.world.remove::<ponko::render::ModelMaterials>(entity);
                return Ok(());
            }
            if parts.is_null() || materials.is_null() {
                return Err(invalid("parts or materials is null"));
            }
            let parts = std::slice::from_raw_parts(parts, count);
            let materials = std::slice::from_raw_parts(materials, count);
            if materials.iter().any(|&m| !c.gfx.is_live(to_c(m))) {
                return Err(invalid("a material wasn't made by this game"));
            }
            let list: Vec<(usize, ponko::render::Material)> = parts
                .iter()
                .zip(materials)
                .map(|(&part, &m)| (part as usize, to_c(m)))
                .collect();
            c.world.insert(entity, ponko::render::ModelMaterials::new(&list));
            Ok(())
        })
    })
}

/// How many parts `ponko_ctx_set_model_part_materials` listed for
/// `entity` (0: none), and with `out_parts` and `out_materials` non-null,
/// up to `capacity` of them, by part number.
/// # Safety
/// `ctx` null or live; the outputs hold `capacity` values each, or are null.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_model_part_materials(std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_model_part_materials(
    ctx: *mut PonkoCtx,
    entity: u64,
    out_parts: *mut u32,
    out_materials: *mut PonkoMaterial,
    capacity: usize,
    out_count: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = ponko::prelude::Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            if out_count.is_null() {
                return Err(invalid("output is null"));
            }
            let Some(list) = c.world.get::<ponko::render::ModelMaterials>(entity) else {
                return out(out_count, 0);
            };
            if !out_parts.is_null() && !out_materials.is_null() {
                for (i, (part, material)) in list.parts().take(capacity).enumerate() {
                    *out_parts.add(i) = part as u32;
                    *out_materials.add(i) = to_c(material);
                }
            }
            out(out_count, list.parts().len())
        })
    })
}

/// Whether models showing `mesh` cast shadows (Rust
/// `Graphics::set_mesh_shadows`): on for every mesh at first. Off suits small
/// things crowding the ground, such as grass.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_mesh_shadows(std::ptr::null_mut(), Default::default(), false) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_mesh_shadows(ctx: *mut PonkoCtx, mesh: PonkoMesh, casts: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.set_mesh_shadows(to_c(mesh), casts);
            Ok(())
        })
    })
}

/// Whether models showing `mesh` cast shadows.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// let mut casts = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mesh_shadows(std::ptr::null_mut(), Default::default(), &mut casts) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mesh_shadows(
    ctx: *mut PonkoCtx,
    mesh: PonkoMesh,
    out_casts: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_casts, c.gfx.mesh_shadows(to_c(mesh)))) })
}

same_layout!(
    PonkoPointLight3D,
    PointLight3D,
    color = color,
    intensity = intensity,
    range = range,
    shadows = shadows,
    shadow_resolution = shadow_resolution
);

/// Registers the built-in `PointLight3D` for entity insertion and query columns.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_point_light3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_point_light3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<PointLight3D>(world, out_id) }
}

same_layout!(
    PonkoSpotLight3D,
    SpotLight3D,
    color = color,
    intensity = intensity,
    range = range,
    inner_angle = inner_angle,
    outer_angle = outer_angle,
    shadows = shadows,
    shadow_resolution = shadow_resolution
);

/// Registers the built-in `SpotLight3D` for entity insertion and query columns.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_spot_light3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_spot_light3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<SpotLight3D>(world, out_id) }
}

/// Axis-aligned local reflection volume. Zero content falls back to sky IBL.
/// ```
/// let p = ponko_ffi::ponko_reflection_probe3d_default();
/// assert!(p.box_projection);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoReflectionProbe3D {
    /// Full size in world units (transform scale and rotation ignored).
    pub size: PonkoVec3,
    /// Fade distance inside the boundary.
    pub blend_distance: f32,
    /// Linear radiance multiplier.
    pub intensity: f32,
    /// Context-owned baked content, initially zero.
    pub content: u32,
    /// Enable parallax correction against the volume's walls.
    pub box_projection: bool,
}
same_layout!(
    PonkoReflectionProbe3D,
    ReflectionProbe3D,
    size = size,
    blend_distance = blend_distance,
    intensity = intensity,
    content = content,
    box_projection = box_projection
);

/// Default 10-unit box, 1-unit blend, intensity 1, box projection enabled.
/// ```
/// assert_eq!(ponko_ffi::ponko_reflection_probe3d_default().content, 0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_reflection_probe3d_default() -> PonkoReflectionProbe3D {
    PonkoReflectionProbe3D {
        size: PonkoVec3 {
            x: 10.0,
            y: 10.0,
            z: 10.0,
        },
        blend_distance: 1.0,
        intensity: 1.0,
        content: 0,
        box_projection: true,
    }
}

/// Register the reflection probe component for insertion and queries.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_reflection_probe3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_reflection_probe3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: caller supplies the live world and writable output.
    unsafe { builtin::<ReflectionProbe3D>(world, out_id) }
}

/// A box that paints an image onto the opaque surfaces inside it (Rust
/// `Decal3D`). The entity's transform places it; the image is projected
/// along its -Z. Start from `ponko_decal3d_default`.
/// ```
/// let d = ponko_ffi::ponko_decal3d_default();
/// assert_eq!((d.image, d.normal_image), (0, ponko_ffi::PONKO_DECAL_NO_IMAGE));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoDecal3D {
    /// Width, height and depth of the box (times the transform's scale).
    pub size: PonkoVec3,
    /// Layer of the decal textures holding the color image, or `PONKO_DECAL_NO_IMAGE`.
    pub image: u32,
    /// Layer holding a tangent-space normal map, or `PONKO_DECAL_NO_IMAGE`.
    pub normal_image: u32,
    /// Top left of the image in its layer (0 to 1).
    pub uv_min: PonkoVec2,
    /// Bottom right of the image in its layer.
    pub uv_max: PonkoVec2,
    /// sRGB tint; its alpha multiplies `opacity`.
    pub color: PonkoColor,
    /// 0 to 1, times the image's alpha.
    pub opacity: f32,
    /// How strongly the normal map tilts the surface.
    pub normal_strength: f32,
    /// Surface roughness under the decal; negative leaves it.
    pub roughness: f32,
    /// Surface metallic value under the decal; negative leaves it.
    pub metallic: f32,
    /// Radians from the projection axis up to which the decal is fully visible.
    pub angle_fade_start: f32,
    /// Radians where it has faded out completely.
    pub angle_fade_end: f32,
    /// Fraction of the box's depth, from each end, it fades over.
    pub depth_fade: f32,
    /// Distance past which it is not drawn (0: unlimited).
    pub max_distance: f32,
    /// Higher orders draw over lower ones.
    pub order: i32,
    /// Models with any of these bits (low 16) in their `PonkoReceivesDecals` get it.
    pub layers: u32,
}
same_layout!(
    PonkoDecal3D,
    Decal3D,
    size = size,
    image = image,
    normal_image = normal_image,
    uv_min = uv_min,
    uv_max = uv_max,
    color = color,
    opacity = opacity,
    normal_strength = normal_strength,
    roughness = roughness,
    metallic = metallic,
    angle_fade_start = angle_fade_start,
    angle_fade_end = angle_fade_end,
    depth_fade = depth_fade,
    max_distance = max_distance,
    order = order,
    layers = layers
);

/// `image` and `normal_image` value that selects no image.
pub const PONKO_DECAL_NO_IMAGE: u32 = u32::MAX;

/// A 1 x 1 x 1 white decal from image 0, no normal map, all layers (Rust `Decal3D::default`).
/// ```
/// let d = ponko_ffi::ponko_decal3d_default();
/// assert_eq!((d.opacity, d.layers), (1.0, 0xFFFF));
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_decal3d_default() -> PonkoDecal3D {
    // SAFETY: the layouts are the same (checked above); every field is plain data.
    unsafe { std::mem::transmute(Decal3D::default()) }
}

/// Register the decal component for insertion and queries.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_decal3d_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_decal3d_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<Decal3D>(world, out_id) }
}

/// Which decal layers a model takes (Rust `ReceivesDecals`); the low 16
/// bits count. Models without the component take every decal.
/// ```
/// assert_eq!(std::mem::size_of::<ponko_ffi::PonkoReceivesDecals>(), 4);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoReceivesDecals {
    /// The layer bits.
    pub layers: u32,
}
same_layout!(PonkoReceivesDecals, ReceivesDecals);

/// Register the decal receiver component for insertion and queries.
/// # Safety
/// Live world and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 0; assert_eq!(ponko_receives_decals_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_receives_decals_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { builtin::<ReceivesDecals>(world, out_id) }
}

/// Sets the texture array decals take their images from (Rust
/// `Graphics::set_decal_textures`). It must have been made with
/// `PONKO_TEXTURE_DATA`: decals decode color layers from sRGB themselves.
/// # Safety
/// `ctx` null or live.
/// ```
/// let a = ponko_ffi::PonkoTextureArray::default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_decal_textures(std::ptr::null_mut(), a) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_decal_textures(ctx: *mut PonkoCtx, array: PonkoTextureArray) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx
                .try_set_decal_textures(Some(array_from_c(array)))
                .map_err(invalid_owned)
        })
    })
}

/// Goes back to flat-colored decals.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_clear_decal_textures(std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_clear_decal_textures(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.set_decal_textures(None);
            Ok(())
        })
    })
}

/// Whether this GPU draws projected decals (Rust `Graphics::supports_decals`);
/// false in headless runs and on WebGL2.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// let mut yes = true;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_supports_decals(std::ptr::null_mut(), &mut yes) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_supports_decals(ctx: *mut PonkoCtx, out_supported: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_supported, c.gfx.supports_decals())) })
}

/// Build, now, the 3D pipeline variants frames would build the first time
/// they draw them (Rust `Graphics::warm_render_pipelines`); call it at the end of
/// loading. Does nothing in headless runs.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_warm_render_pipelines(std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_warm_render_pipelines(ctx: *mut PonkoCtx) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.warm_render_pipelines();
            Ok(())
        })
    })
}

/// Queue six HDR views of the scene at the probe and prefilter them.
/// # Safety
/// Context is null or a live callback context.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_bake_reflection_probe(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_bake_reflection_probe(ctx: *mut PonkoCtx, entity: u64) -> PonkoStatus {
    // SAFETY: forwarded context contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx
                .bake_reflection_probe(&mut c.world, ponko::prelude::Entity::from_bits(entity))
                .map_err(invalid_owned)
        })
    })
}

/// Load a linear HDR/EXR 2:1 equirectangular environment for a probe.
/// # Safety
/// Context is null or live; name is a null-terminated UTF-8 string.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_reflection_probe(std::ptr::null_mut(), 0, std::ptr::null()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_reflection_probe(
    ctx: *mut PonkoCtx,
    entity: u64,
    name: *const c_char,
) -> PonkoStatus {
    // SAFETY: forwarded context and string contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx
                .try_load_reflection_probe(&mut c.world, ponko::prelude::Entity::from_bits(entity), text(name)?)
                .map_err(invalid_owned)
        })
    })
}

// ---- levels of detail ---------------------------------------------------------

/// One mesh of a LOD level (Rust `LodRenderer`), placed by `offset` in the
/// group's space and tinted by `color` (white: no change).
/// ```
/// let r = ponko_ffi::ponko_lod_renderer(Default::default());
/// assert_eq!((r.color.r, r.offset.scale.x), (1.0, 1.0));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoLodRenderer {
    /// What to draw.
    pub mesh: PonkoMesh,
    /// Where it sits in the group's space.
    pub offset: PonkoTransform3D,
    /// Multiplied with the materials' colors.
    pub color: PonkoColor,
}

/// `mesh` at the group's origin, untinted.
/// ```
/// let r = ponko_ffi::ponko_lod_renderer(ponko_ffi::PonkoMesh { id: 4 });
/// assert_eq!(r.mesh.id, 4);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_lod_renderer(mesh: PonkoMesh) -> PonkoLodRenderer {
    PonkoLodRenderer {
        mesh,
        offset: transform_to_c(Transform3D::default()),
        color: to_c(Color::WHITE),
    }
}

/// One level of a LOD group (Rust `LodLevel`): its renderers are drawn while
/// the group covers at least `screen_height` of the screen's height. With
/// `custom_shadow`, `shadow_casters` cast the shadow instead of the
/// renderers (none: no shadow).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoLodLevel {
    /// 0 to 1, going down from level to level.
    pub screen_height: f32,
    /// How much of the way up to the previous threshold it crossfades into
    /// the next level, 0 to 1 (0: switches at once).
    pub fade_width: f32,
    /// `renderer_count` renderers (may be null when 0).
    pub renderers: *const PonkoLodRenderer,
    /// How many.
    pub renderer_count: usize,
    /// Whether `shadow_casters` cast the shadow instead of the renderers.
    pub custom_shadow: bool,
    /// `shadow_caster_count` renderers (may be null when 0).
    pub shadow_casters: *const PonkoLodRenderer,
    /// How many.
    pub shadow_caster_count: usize,
}

/// Global LOD settings (Rust `ctx.scene3d.lod`).
/// ```
/// let s = ponko_ffi::PonkoLodSettings { bias: 1.0, crossfade: true };
/// assert!(s.crossfade);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoLodSettings {
    /// Multiplies every group's screen height (2: detail twice as far).
    pub bias: f32,
    /// Whether levels with a fade width crossfade.
    pub crossfade: bool,
}

/// Which level of a group is drawn (Rust `LodPick`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoLodPick {
    /// 0 culled, 1 one level (`level`), 2 crossfading from `level` to `to`.
    pub kind: u32,
    /// The level drawn, or fading out.
    pub level: u32,
    /// The level fading in; -1 for none (fading out past the last level).
    pub to: i32,
    /// How far along the fade is, 0 to 1.
    pub blend: f32,
}

/// # Safety
/// `count` values at `list` (may be null when 0).
unsafe fn lod_renderers(list: *const PonkoLodRenderer, count: usize) -> Result<Vec<LodRenderer>, crate::Failure> {
    if list.is_null() && count > 0 {
        return Err(invalid("renderers is null"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: caller contract.
    let given = unsafe { std::slice::from_raw_parts(list, count) };
    Ok(given
        .iter()
        .map(|r| LodRenderer {
            mesh: to_c(r.mesh),
            offset: transform_from_c(r.offset),
            color: to_c(r.color),
        })
        .collect())
}

/// `count` C levels as Rust `LodLevel`s.
/// # Safety
/// `levels` holds `count` values (may be null when 0), each pointing at as
/// many renderers as it says.
pub(crate) unsafe fn lod_levels(levels: *const PonkoLodLevel, count: usize) -> Result<Vec<LodLevel>, crate::Failure> {
    if levels.is_null() && count > 0 {
        return Err(invalid("levels is null"));
    }
    let given = if count == 0 {
        &[][..]
    } else {
        // SAFETY: caller contract.
        unsafe { std::slice::from_raw_parts(levels, count) }
    };
    let mut list = Vec::with_capacity(count);
    for l in given {
        // SAFETY: caller contract.
        let shadow = if l.custom_shadow {
            LodShadow::Casters(unsafe { lod_renderers(l.shadow_casters, l.shadow_caster_count)? })
        } else {
            LodShadow::Same
        };
        // SAFETY: caller contract.
        let renderers = unsafe { lod_renderers(l.renderers, l.renderer_count)? };
        list.push(
            LodLevel::new(l.screen_height, renderers)
                .with_fade(l.fade_width)
                .with_shadow(shadow),
        );
    }
    Ok(list)
}

/// Gives `entity` levels of detail (Rust `LodGroup`): `count` levels, most
/// detailed first, for an object `size` tall in its own space, its
/// distance measured to `center`. Replaces any it had. Stale entities are an
/// error.
/// # Safety
/// `ctx` null or live; `levels` holds `count` values (may be null when 0),
/// each pointing at as many renderers as it says.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_lod_group(std::ptr::null_mut(), 0, 1.0, Default::default(), std::ptr::null(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_lod_group(
    ctx: *mut PonkoCtx,
    entity: u64,
    size: f32,
    center: PonkoVec3,
    levels: *const PonkoLodLevel,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = ponko::prelude::Entity::from_bits(entity);
            if !c.world.is_alive(entity) {
                return Err(invalid("no such entity"));
            }
            let list = lod_levels(levels, count)?;
            c.world
                .insert(entity, LodGroup::new(size, list).with_center(to_c(center)));
            Ok(())
        })
    })
}

/// The level of `entity`'s LOD group the 3D camera picks, under the current
/// settings (Rust `LodGroup::pick_for`). Entities without a group, or
/// without a `PonkoTransform3D`, are an error.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// let mut pick = Default::default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_lod_pick(std::ptr::null_mut(), 0, &mut pick) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_lod_pick(
    ctx: *mut PonkoCtx,
    entity: u64,
    out_pick: *mut PonkoLodPick,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let entity = ponko::prelude::Entity::from_bits(entity);
            let (Some(group), Some(transform)) = (c.world.get::<LodGroup>(entity), c.world.get::<Transform3D>(entity))
            else {
                return Err(invalid("that entity has no LOD group and transform"));
            };
            let pick = match group.pick_for(transform, &c.scene3d.camera, &c.scene3d.lod) {
                LodPick::Culled => PonkoLodPick::default(),
                LodPick::Level(i) => PonkoLodPick {
                    kind: 1,
                    level: i as u32,
                    to: -1,
                    blend: 0.0,
                },
                LodPick::Fade { from, to, blend } => PonkoLodPick {
                    kind: 2,
                    level: from as u32,
                    to: to.map_or(-1, |t| t as i32),
                    blend,
                },
            };
            out(out_pick, pick)
        })
    })
}

/// The global LOD settings.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// let mut s = ponko_ffi::PonkoLodSettings { bias: 0.0, crossfade: false };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_lod_settings(std::ptr::null_mut(), &mut s) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_lod_settings(
    ctx: *mut PonkoCtx,
    out_settings: *mut PonkoLodSettings,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            out(
                out_settings,
                PonkoLodSettings {
                    bias: c.scene3d.lod.bias,
                    crossfade: c.scene3d.lod.crossfade,
                },
            )
        })
    })
}

/// Replaces the global LOD settings: a quality setting changes `bias`.
/// Negative or non-finite biases are an error.
/// # Safety
/// `ctx` null or live.
/// ```
/// let s = ponko_ffi::PonkoLodSettings { bias: 1.0, crossfade: true };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_lod_settings(std::ptr::null_mut(), s) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_lod_settings(ctx: *mut PonkoCtx, settings: PonkoLodSettings) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if !settings.bias.is_finite() || settings.bias < 0.0 {
                return Err(invalid("bias must be 0 or more"));
            }
            c.scene3d.lod.bias = settings.bias;
            c.scene3d.lod.crossfade = settings.crossfade;
            Ok(())
        })
    })
}

/// The fraction of the screen's height something `size` tall covers at
/// `distance` from a camera with vertical field of view `fov_y` (radians):
/// to turn distances into thresholds.
/// ```
/// let h = ponko_ffi::ponko_lod_screen_height_at(2.0, 10.0, 60f32.to_radians());
/// assert!((h - 0.1732).abs() < 1e-3);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_lod_screen_height_at(size: f32, distance: f32, fov_y: f32) -> f32 {
    LodGroup::screen_height_at(size, distance, fov_y)
}

/// The distance at which something `size` tall covers `height` of the
/// screen: the inverse of `ponko_lod_screen_height_at`.
/// ```
/// let d = ponko_ffi::ponko_lod_distance_at(2.0, 0.1732, 60f32.to_radians());
/// assert!((d - 10.0).abs() < 0.01);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_lod_distance_at(size: f32, height: f32, fov_y: f32) -> f32 {
    LodGroup::distance_at(size, height, fov_y)
}

/// A GPU array of four-float elements.
/// ```
/// let b = ponko_ffi::PonkoGpuBuffer { id: 0 }; assert_eq!(b.id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoGpuBuffer {
    /// Engine-internal handle ID.
    pub id: u32,
}

/// Creates a GPU buffer. `count` is zero for zeroed data or equals `len`.
/// # Safety
/// `ctx` is live or null; `data` holds `count` arrays, output is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_gpu_buffer(
    ctx: *mut PonkoCtx,
    len: u32,
    data: *const [f32; 4],
    count: usize,
    output: *mut PonkoGpuBuffer,
) -> PonkoStatus {
    // SAFETY: caller supplies the described arrays and output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if output.is_null() || len == 0 || (count != 0 && (count != len as usize || data.is_null())) {
                return Err(invalid("invalid GPU buffer arguments"));
            }
            let data = if count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(data, count)
                    .iter()
                    .map(|v| Vec4::from_array(*v))
                    .collect()
            };
            let b = c.gfx.create_gpu_buffer(len, &data);
            out(output, PonkoGpuBuffer { id: __ffi::id(b) })
        })
    })
}
/// Writes a GPU buffer range. Out-of-range writes return InvalidArgument.
/// # Safety
/// `ctx` is live or null; `data` holds `count` arrays, null allowed for zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_write_gpu_buffer(
    ctx: *mut PonkoCtx,
    buffer: PonkoGpuBuffer,
    offset: u32,
    data: *const [f32; 4],
    count: usize,
) -> PonkoStatus {
    // SAFETY: caller supplies the described array.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let b = __ffi::from_id::<GpuBuffer>(buffer.id);
            let len = c.gfx.gpu_buffer_len(b).ok_or_else(|| invalid("invalid GPU buffer"))?;
            if offset > len || count > (len - offset) as usize || (count > 0 && data.is_null()) {
                return Err(invalid("invalid GPU buffer write"));
            }
            let data = if count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(data, count)
                    .iter()
                    .map(|v| Vec4::from_array(*v))
                    .collect()
            };
            c.gfx.write_gpu_buffer(b, offset, &data);
            Ok(())
        })
    })
}
/// Frees a GPU buffer's memory; its compute passes are removed from the
/// context's list and meshes using it draw nothing. Its id is never
/// reused. An invalid or already freed buffer is an error (unlike
/// `ponko_ctx_handle_free`, which ignores it).
/// # Safety
/// `ctx` is live or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_free_gpu_buffer(ctx: *mut PonkoCtx, buffer: PonkoGpuBuffer) -> PonkoStatus {
    // SAFETY: forwarded context contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let b = __ffi::from_id::<GpuBuffer>(buffer.id);
            if c.gfx.gpu_buffer_len(b).is_none() {
                return Err(invalid("invalid GPU buffer"));
            }
            c.free(b);
            Ok(())
        })
    })
}
/// Grows a mesh's culling bounds to hold the sphere `center`, `radius` (in
/// the mesh's space): where a buffer-driven mesh's shader puts its instances.
/// # Safety
/// `ctx` is live or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_mesh_gpu_bounds(
    ctx: *mut PonkoCtx,
    mesh: PonkoMesh,
    center: PonkoVec3,
    radius: f32,
) -> PonkoStatus {
    // SAFETY: forwarded context contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let mesh = to_c(mesh);
            let center = to_c(center);
            if !c.gfx.is_live(mesh) || !(radius >= 0.0 && radius.is_finite() && center.is_finite()) {
                return Err(invalid("invalid mesh bounds arguments"));
            }
            c.gfx.set_mesh_gpu_bounds(mesh, center, radius);
            Ok(())
        })
    })
}
/// Draws `count` GPU instances of a mesh, reading `items[v.instance_index]`,
/// each at its entity's transform and tint.
/// # Safety
/// `ctx` is live or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_mesh_gpu_buffer(
    ctx: *mut PonkoCtx,
    mesh: PonkoMesh,
    buffer: PonkoGpuBuffer,
    count: u32,
) -> PonkoStatus {
    // SAFETY: forwarded context contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let mesh = to_c(mesh);
            let b = __ffi::from_id::<GpuBuffer>(buffer.id);
            if !c.gfx.is_live(mesh) || c.gfx.gpu_buffer_len(b).is_none_or(|len| count > len) {
                return Err(invalid("invalid GPU instance arguments"));
            }
            c.gfx.set_mesh_gpu_buffer(mesh, b, count);
            Ok(())
        })
    })
}

/// Persistent static instance batch handle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PonkoInstanceBatch {
    /// Opaque batch identifier.
    pub id: u32,
}
/// Static batch settings. Materials come from the mesh.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoInstanceBatchOptions {
    /// World-space cell width; finite and positive.
    pub cell_size: f32,
    /// Nonnegative distance from this view's camera or light; infinity disables it.
    pub max_distance: f32,
    /// Whether to cast sun, point-light and spot-light shadows.
    pub cast_shadows: bool,
    /// Uniform sRGB tint.
    pub tint: PonkoColor,
}
/// Changes the culling preference: 0 = Auto, 1 = PreferGpu, 2 = PreferCpu.
/// Reselects the path immediately. Unknown values and freed batches are rejected.
/// The by-value options struct retains its existing ABI layout.
/// # Safety
/// Context is live or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_instance_batch_culling(
    ctx: *mut PonkoCtx,
    batch: PonkoInstanceBatch,
    culling: u32,
) -> PonkoStatus {
    // SAFETY: the caller supplies a live context or null; ctx_access validates it.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let culling = match culling {
                0 => ponko::render::BatchCulling::Auto,
                1 => ponko::render::BatchCulling::PreferGpu,
                2 => ponko::render::BatchCulling::PreferCpu,
                _ => return Err(invalid("invalid batch culling preference")),
            };
            if !c
                .gfx
                .set_instance_batch_culling(__ffi::from_id::<ponko::render::InstanceBatch>(batch.id), culling)
            {
                return Err(invalid("invalid instance batch"));
            }
            Ok(())
        })
    })
}
/// Creates a persistent batch from column-major affine matrices.
/// # Safety
/// Context is live or null, matrices hold `count` entries, output is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_instance_batch(
    ctx: *mut PonkoCtx,
    mesh: PonkoMesh,
    matrices: *const [f32; 16],
    count: usize,
    options: PonkoInstanceBatchOptions,
    output: *mut PonkoInstanceBatch,
) -> PonkoStatus {
    // SAFETY: caller supplies the described matrices and output.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if output.is_null()
                || !c.gfx.is_live(to_c(mesh))
                || !options.cell_size.is_finite()
                || options.cell_size <= 0.0
                || (options.max_distance.is_nan() || options.max_distance < 0.0)
                || count > 16_777_216
                || (count > 0 && matrices.is_null())
            {
                return Err(invalid("invalid instance batch"));
            }
            let data = if count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(matrices, count)
                    .iter()
                    .map(ponko::prelude::Mat4::from_cols_array)
                    .collect::<Vec<_>>()
            };
            if data
                .iter()
                .any(|m| !m.is_finite() || m.row(3) != ponko::prelude::Vec4::W)
            {
                return Err(invalid("invalid instance transform"));
            }
            let b = c.gfx.create_instance_batch(
                to_c(mesh),
                &data,
                ponko::render::InstanceBatchOptions {
                    cell_size: options.cell_size,
                    max_distance: options.max_distance,
                    cast_shadows: options.cast_shadows,
                    tint: Color::rgba(options.tint.r, options.tint.g, options.tint.b, options.tint.a),
                    ..Default::default()
                },
            );
            out(output, PonkoInstanceBatch { id: __ffi::id(b) })
        })
    })
}
/// Replaces transforms and rebuilds spatial cells.
/// # Safety
/// Context is live or null; matrices hold `count` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_update_instance_batch(
    ctx: *mut PonkoCtx,
    batch: PonkoInstanceBatch,
    matrices: *const [f32; 16],
    count: usize,
) -> PonkoStatus {
    // SAFETY: caller supplies the described matrix array.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let b = __ffi::from_id::<ponko::render::InstanceBatch>(batch.id);
            if c.gfx.instance_batch_len(b).is_none() || count > 16_777_216 || (count > 0 && matrices.is_null()) {
                return Err(invalid("invalid instance batch update"));
            }
            let data = if count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(matrices, count)
                    .iter()
                    .map(ponko::prelude::Mat4::from_cols_array)
                    .collect::<Vec<_>>()
            };
            if data
                .iter()
                .any(|m| !m.is_finite() || m.row(3) != ponko::prelude::Vec4::W)
            {
                return Err(invalid("invalid instance transform"));
            }
            if !c.gfx.update_instance_batch(b, &data) {
                return Err(invalid("invalid instance mesh"));
            }
            Ok(())
        })
    })
}
/// Frees a batch; repeated frees are harmless.
/// # Safety
/// Context is live or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_free_instance_batch(ctx: *mut PonkoCtx, batch: PonkoInstanceBatch) -> PonkoStatus {
    // SAFETY: forwarded context contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.free(__ffi::from_id::<ponko::render::InstanceBatch>(batch.id));
            Ok(())
        })
    })
}

/// Mesh, half-open origin-distance band, and view flags for a batch level.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoInstanceLevel {
    /// Live mesh handle.
    pub mesh: PonkoMesh,
    /// Inclusive finite nonnegative lower distance.
    pub min_distance: f32,
    /// Exclusive upper distance; infinity is allowed.
    pub max_distance: f32,
    /// Draw in camera and target views.
    pub camera: bool,
    /// Cast shadows when the batch and mesh also allow it.
    pub shadows: bool,
}

/// Creates a multi-level batch. Empty colours use the options' tint.
/// # Safety
/// Context is live or null. Nonempty arrays have their stated lengths;
/// output is writable. Matrices are column-major affine transforms.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_instance_batch_levels(
    ctx: *mut PonkoCtx,
    levels: *const PonkoInstanceLevel,
    level_count: usize,
    matrices: *const [f32; 16],
    count: usize,
    colors: *const PonkoColor,
    color_count: usize,
    options: PonkoInstanceBatchOptions,
    output: *mut PonkoInstanceBatch,
) -> PonkoStatus {
    // SAFETY: caller supplies live arrays and writable output as documented.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if output.is_null()
                || levels.is_null()
                || level_count == 0
                || !options.cell_size.is_finite()
                || options.cell_size <= 0.0
                || options.max_distance.is_nan()
                || options.max_distance < 0.0
            {
                return Err(invalid("invalid instance batch levels"));
            }
            let levels = std::slice::from_raw_parts(levels, level_count);
            if levels.iter().any(|l| {
                !c.gfx.is_live(to_c(l.mesh))
                    || !l.min_distance.is_finite()
                    || l.min_distance < 0.0
                    || l.max_distance <= l.min_distance
                    || l.max_distance.is_nan()
            }) {
                return Err(invalid("invalid instance level mesh or distance band"));
            }
            let levels: Vec<_> = levels
                .iter()
                .map(|l| ponko::render::InstanceLevel {
                    mesh: to_c(l.mesh),
                    min_distance: l.min_distance,
                    max_distance: l.max_distance,
                    camera: l.camera,
                    shadows: l.shadows,
                })
                .collect();
            let (transforms, colors) = instance_arrays(matrices, count, colors, color_count)?;
            let b = c.gfx.create_instance_batch_levels(
                &levels,
                &transforms,
                &colors,
                ponko::render::InstanceBatchOptions {
                    cell_size: options.cell_size,
                    max_distance: options.max_distance,
                    cast_shadows: options.cast_shadows,
                    tint: Color::rgba(options.tint.r, options.tint.g, options.tint.b, options.tint.a),
                    ..Default::default()
                },
            );
            out(output, PonkoInstanceBatch { id: __ffi::id(b) })
        })
    })
}

/// Replaces transforms and colours; zero colours restore the batch tint.
/// # Safety
/// Context is live or null; nonempty arrays have the stated lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_update_instance_batch_colors(
    ctx: *mut PonkoCtx,
    batch: PonkoInstanceBatch,
    matrices: *const [f32; 16],
    count: usize,
    colors: *const PonkoColor,
    color_count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded array/context contracts; validation precedes slicing.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let (transforms, colors) = instance_arrays(matrices, count, colors, color_count)?;
            if !c.gfx.update_instance_batch_colors(
                __ffi::from_id::<ponko::render::InstanceBatch>(batch.id),
                &transforms,
                &colors,
            ) {
                return Err(invalid("invalid instance batch or level mesh"));
            }
            Ok(())
        })
    })
}

/// Caller guarantees non-null arrays have their stated lengths.
unsafe fn instance_arrays(
    matrices: *const [f32; 16],
    count: usize,
    colors: *const PonkoColor,
    color_count: usize,
) -> Result<(Vec<ponko::prelude::Mat4>, Vec<Color>), crate::Failure> {
    if count > 16_777_216
        || (count > 0 && matrices.is_null())
        || (color_count != 0 && color_count != count)
        || (color_count > 0 && colors.is_null())
    {
        return Err(invalid("invalid instance transform or color count"));
    }
    // SAFETY: pointer/length contracts are forwarded; zero counts avoid null slices.
    let transforms = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(matrices, count) }
            .iter()
            .map(ponko::prelude::Mat4::from_cols_array)
            .collect::<Vec<_>>()
    };
    if transforms
        .iter()
        .any(|m| !m.is_finite() || m.row(3) != ponko::prelude::Vec4::W)
    {
        return Err(invalid("invalid instance transform"));
    }
    // SAFETY: as above, and colour length was validated before slicing.
    let colors = if color_count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(colors, color_count) }
            .iter()
            .map(|c| Color::rgba(c.r, c.g, c.b, c.a))
            .collect()
    };
    Ok((transforms, colors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PonkoStatus::PONKO_OK;
    use crate::ctx::with_ctx;

    #[test]
    fn preset_levels_follow_the_c_numbering() {
        let levels = [
            (GraphicsQuality::Low, 0),
            (GraphicsQuality::Medium, 1),
            (GraphicsQuality::High, 2),
            (GraphicsQuality::Ultra, 3),
        ];
        for (quality, level) in levels {
            assert_eq!(level_to_c(quality), level, "{quality:?}");
        }
    }

    #[test]
    fn out_of_range_settings_read_back_clamped_and_round_trip() {
        ponko::prelude::Game::new("quality", 8, 8).run_headless(0, |ctx| {
            ctx.scene3d.post.render_scale = 9.0;
            ctx.scene3d.post.sharpen = f32::NAN;
            ctx.scene3d.lighting.shadow_distance = f32::INFINITY;
            ctx.scene3d.lighting.shadow_cascades = 0;
            ctx.scene3d.lighting.light_shadow_atlas = 5000;
            with_ctx(ctx, |c| {
                // SAFETY: `c` is live for this closure; the outputs are locals.
                unsafe {
                    let mut q = std::mem::zeroed::<PonkoGraphicsQuality>();
                    assert_eq!(ponko_ctx_graphics_quality(c, &mut q), PONKO_OK);
                    assert_eq!((q.render_scale, q.sharpen), (2.0, 0.0));
                    assert_eq!((q.shadow_distance, q.shadow_cascades), (f32::MAX, 1));
                    assert_eq!(q.light_shadow_atlas, 4096);
                    assert_eq!(ponko_ctx_set_graphics_quality(c, &q), PONKO_OK, "takes it back");
                    let mut back = std::mem::zeroed::<PonkoGraphicsQuality>();
                    assert_eq!(ponko_ctx_graphics_quality(c, &mut back), PONKO_OK);
                    assert_eq!(back, q);
                }
            });
        });
    }

    #[test]
    fn setting_the_post_keeps_the_sample_count() {
        ponko::prelude::Game::new("post", 8, 8).run_headless(0, |ctx| {
            ctx.scene3d.post.msaa = 2;
            with_ctx(ctx, |c| {
                // SAFETY: `c` is live for this closure; the output is a local.
                unsafe {
                    let mut post = std::mem::zeroed::<PonkoPost>();
                    assert_eq!(ponko_ctx_post(c, &mut post), PONKO_OK);
                    post.bloom = 0.5;
                    assert_eq!(ponko_ctx_set_post(c, &post), PONKO_OK);
                }
            });
            assert_eq!(ctx.scene3d.post.bloom, 0.5);
            assert_eq!(ctx.scene3d.post.msaa, 2, "PonkoPost has no sample count");
        });
    }
}
