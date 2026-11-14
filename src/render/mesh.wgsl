// Instanced 3D meshes: one instance = one Model, lit by a sun (Cook-Torrance
// GGX, metallic-roughness, as glTF) and sky/ground ambient light.
//
// The engine builds each material shader from three parts: this prelude,
// the game's shader (its `vertex` and `surface` functions, when it has
// them) and `mesh_main.wgsl`, which calls them.

const PI: f32 = 3.14159265;

struct ReflectionVolume { center: vec4<f32>, size: vec4<f32>, params: vec4<f32> };

struct Frame {
    view_proj: mat4x4<f32>,
    // World to each shadow cascade's clip space (see `sunlight`).
    sun_view_proj: array<mat4x4<f32>, 4>,
    // In the shadow pass: the cascade being drawn.
    caster_view_proj: mat4x4<f32>,
    camera: vec4<f32>,     // xyz: camera position; w: 0 in shadow maps, else 1
    sun_dir: vec4<f32>,    // xyz: unit vector towards the sun
    sun_color: vec4<f32>,  // rgb: linear color times intensity
    sky: vec4<f32>,        // rgb: linear, times ambient intensity
    ground: vec4<f32>,
    shadow: vec4<f32>,     // cascades (0: no shadows), texel size in uv, normal offset in texels, fade distance
    cascade_texel: vec4<f32>, // each cascade's texel size, in world units
    time: vec4<f32>,       // x: seconds since the game started, y: seconds since the last frame; zw: the last drawn frame's xy
    inv_view_proj: mat4x4<f32>, // clip space back to the world
    screen: vec4<f32>,     // width and height in pixels, then their inverses
    atmosphere: vec4<f32>, // x: a physical sky (1) or not (0), y: haze, z: cos of the sun's radius, w: stars
    sky_light: vec4<f32>,  // rgb: the sunlight the sky is lit by, w: camera height in km
    fog: vec4<f32>,        // x: density per unit, y: fog height, z: 1 / falloff, w: light shafts
    fog_color: vec4<f32>,  // rgb: linear, without an atmosphere
    night: vec4<f32>,      // rgb: light at night
    fog_far: vec4<f32>,    // x: distance fog start, y: its end (0: none), z: sun glow, w: sun focus
    cluster_depth: vec4<f32>,
    cluster_view_proj: mat4x4<f32>,
    // World to each point and spot light shadow view's clip space.
    light_view_proj: array<mat4x4<f32>, 24>,
    // Each view's tile in the light shadow atlas, in uv: x, y, side.
    light_rect: array<vec4<f32>, 24>,
    light_shadow: vec4<f32>, // views (0: none), atlas texel size in uv, normal offset in texels, w: sun shadow filter (1 to 3)
    probes: array<ReflectionVolume, 4>,
    // Temporal antialiasing: `view_proj` carries the sub-pixel jitter; these
    // are the camera without it, and last frame's, to tell motion from jitter.
    unjittered_view_proj: mat4x4<f32>,
    previous_view_proj: mat4x4<f32>,
    jitter: vec4<f32>,     // xy: this frame's jitter in pixels (0 without TAA)
    gi: array<vec4<f32>, 3>, // global illumination's probe grid (see gi.wgsl)
};

struct MaterialData {
    color: vec4<f32>,      // linear RGBA
    emissive: vec4<f32>,   // linear RGB
    params: vec4<f32>,     // roughness, metallic, normal map strength
    maps: vec4<u32>,       // has: normal map, ORM map, emissive map
    custom: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var shadow_map: texture_depth_2d_array;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
// The sky around the camera (see sky_radiance), made each frame.
@group(0) @binding(3) var sky_texture: texture_2d<f32>;
@group(0) @binding(4) var sky_sampler: sampler;
@group(0) @binding(5) var punctual_records: texture_2d<f32>;
@group(0) @binding(6) var punctual_clusters: texture_2d<u32>;
// Point and spot lights' shadow views, one tile each (see punctual_shadow).
@group(0) @binding(7) var light_shadow_map: texture_depth_2d;
@group(1) @binding(0) var<uniform> material: MaterialData;
@group(1) @binding(1) var material_sampler: sampler;
@group(1) @binding(2) var base_texture: texture_2d<f32>;
@group(1) @binding(3) var normal_texture: texture_2d<f32>;
@group(1) @binding(4) var orm_texture: texture_2d<f32>;
@group(1) @binding(5) var emissive_texture: texture_2d<f32>;
@group(1) @binding(6) var custom_texture0: texture_2d<f32>;
@group(1) @binding(7) var custom_texture1: texture_2d<f32>;
@group(1) @binding(8) var custom_array0: texture_2d_array<f32>;
@group(1) @binding(9) var custom_array1: texture_2d_array<f32>;
@group(1) @binding(10) var custom_array2: texture_2d_array<f32>;
// The opaque scene behind a transparent surface (see scene_color).
@group(2) @binding(0) var scene_color_texture: texture_2d<f32>;
@group(2) @binding(1) var scene_depth_texture: texture_depth_multisampled_2d;

// A vertex in world space, as a shader's `vertex` function gets and returns it.
struct Vertex {
    instance_index: u32, // index into a mesh GPU buffer
    position: vec3<f32>,
    normal: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>, // the model's tint times the vertex color
    uv1: vec2<f32>,   // the mesh's second texture coordinates (zero without)
    origin: vec3<f32>, // read-only: where the model is (its transform's position)
};

// A pixel's surface, as a shader's `surface` function gets and returns it:
// filled in from the material and its textures first.
struct Surface {
    albedo: vec3<f32>,
    alpha: f32,
    normal: vec3<f32>,   // world space, unit length
    roughness: f32,
    metallic: f32,
    occlusion: f32,      // 1: fully lit by ambient light, 0: none
    emissive: vec3<f32>,
    unlit: bool,        // true: albedo + emissive, without direct or ambient lighting
    // Read-only inputs.
    world: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>,    // the model's tint
    view: vec3<f32>,     // unit vector from the surface to the camera
    screen: vec2<f32>,   // the pixel, in pixels from the top left
    uv1: vec2<f32>,      // the mesh's second texture coordinates (zero without)
    origin: vec3<f32>,   // where the model is (its transform's position)
};

// Whether this draw is into a shadow map (seen from the sun), not for a camera.
fn drawing_shadows() -> bool {
    return frame.camera.w == 0.0;
}

// The motion pass (temporal antialiasing, temporal AO) runs `vertex` a
// second time, as of the last drawn frame, to see where each corner was:
// these are set only then. The engine has the game's shader read
// `frame.time` and `items[i]` through `motion_frame_time` and `motion_item`,
// which give the last frame's values while they are.
var<private> motion_past_time: bool;
var<private> motion_past_items: bool;

// Whether `vertex` is running as of the last frame, for the motion pass:
// a shader that moves vertices by inputs of its own (a wind strength in
// `material.custom`) can pick the last frame's here.
fn drawing_last_frame() -> bool {
    return motion_past_time;
}

// `frame.time` as of the frame `vertex` is running for.
fn motion_frame_time() -> vec4<f32> {
    if motion_past_time {
        return frame.time.zwzw;
    }
    return frame.time;
}

// The unit direction from `p` towards whoever is looking: the camera, or
// the sun when drawing shadow maps. Billboards face it in their `vertex`
// function, so their shadows are cast by the side the sun sees.
fn toward_viewer(p: vec3<f32>) -> vec3<f32> {
    if drawing_shadows() {
        return frame.sun_dir.xyz;
    }
    return normalize(frame.camera.xyz - p);
}

// The opaque scene's light at a pixel, before tone mapping (in screen
// effects: the frame so far; in scene compute passes: the settled frame).
// Only transparent materials, effects and scene compute passes see the
// scene; elsewhere this is black.
fn scene_color(screen: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(scene_color_texture));
    let at = clamp(vec2<i32>(screen), vec2<i32>(0), size - 1);
    return textureLoad(scene_color_texture, at, 0).rgb;
}

// Where the opaque scene is at a pixel, in the world. Only transparent
// materials, effects and scene compute passes see the scene; elsewhere this
// reads a stand-in depth.
fn scene_position(screen: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth_texture));
    let at = clamp(vec2<i32>(screen), vec2<i32>(0), size - 1);
    let depth = textureLoad(scene_depth_texture, at, 0);
    let ndc = vec2<f32>(screen.x * frame.screen.z * 2.0 - 1.0, 1.0 - screen.y * frame.screen.w * 2.0);
    let world = frame.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    return world.xyz / world.w;
}

// How far the opaque scene is from the camera at a pixel.
fn scene_distance(screen: vec2<f32>) -> f32 {
    return distance(scene_position(screen), frame.camera.xyz);
}

// Where direction `dir` is in the sky texture: around the horizon in u,
// from straight up (v 0) to straight down (v 1), squeezed towards the
// horizon where the sky changes fastest.
fn sky_uv(dir: vec3<f32>) -> vec2<f32> {
    let around = atan2(dir.z, dir.x) / (2.0 * PI) + 0.5;
    let up = asin(clamp(dir.y, -1.0, 1.0)) / (0.5 * PI);
    return vec2<f32>(around, 0.5 - 0.5 * sign(up) * sqrt(abs(up)));
}

// The direction at `uv` of the sky texture (the inverse of sky_uv).
fn sky_dir(uv: vec2<f32>) -> vec3<f32> {
    let around = (uv.x - 0.5) * 2.0 * PI;
    let s = (0.5 - uv.y) * 2.0;
    let up = sign(s) * s * s * 0.5 * PI;
    return vec3<f32>(cos(up) * cos(around), sin(up), cos(up) * sin(around));
}

// The sky's light from direction `dir` (towards the sky), without the sun
// itself. Black without an atmosphere.
fn sky_radiance(dir: vec3<f32>) -> vec3<f32> {
    return sample_sky(sky_texture, sky_sampler, sky_uv(dir));
}

// `color` seen through the fog between the camera and `world`: height fog,
// thickest at the fog height and thinning out above it, and distance fog,
// none before its start and all fog past its end. Both take the sky's color
// behind them (or the fog color, without an atmosphere), and glow toward the
// sun. The sky itself takes only the height fog (`distance` false).
fn fog_along(color: vec3<f32>, world: vec3<f32>, distance: bool) -> vec3<f32> {
    let density = frame.fog.x;
    let far = select(0.0, frame.fog_far.y, distance);
    if density <= 0.0 && far <= 0.0 {
        return color;
    }
    let ray = world - frame.camera.xyz;
    let span = length(ray);
    let k = frame.fog.z;
    // The fog along the ray: density * exp(-(height - fog height) * k),
    // integrated from the camera to `world`.
    let start = density * exp(-(frame.camera.y - frame.fog.y) * k);
    let rise = ray.y * k;
    let along = select((1.0 - exp(-rise)) / rise, 1.0, abs(rise) < 1e-4);
    var seen = exp(-start * span * along);
    if far > 0.0 {
        let near = min(frame.fog_far.x, far - 1e-3);
        seen *= clamp((far - span) / (far - near), 0.0, 1.0);
    }
    let dir = ray / max(span, 1e-6);
    var fog_color = frame.fog_color.rgb;
    if frame.atmosphere.x > 0.0 {
        fog_color = sky_radiance(normalize(vec3<f32>(dir.x, max(dir.y, 0.03), dir.z)));
    }
    // Light scattered forward out of the sun (Henyey-Greenstein, 1 at focus 0).
    let g = frame.fog_far.w;
    let mu = dot(dir, frame.sun_dir.xyz);
    let phase = (1.0 - g * g) / pow(max(1.0 + g * g - 2.0 * g * mu, 1e-4), 1.5);
    var glow = frame.fog_far.z;
    // The shafts can only darken this pixel's glow, by at most what the glow
    // adds; where that is below what a display shows (looking away from the
    // sun, or through little fog), skip the march.
    let visible = max(max(frame.sun_color.r, frame.sun_color.g), frame.sun_color.b) * phase * glow * (1.0 - seen);
    if visible > SHAFT_VISIBLE && density > 0.0 && frame.fog.w > 0.0 {
        glow *= mix(1.0, fog_in_sun(world, start * span * along), frame.fog.w);
    }
    fog_color += frame.sun_color.rgb * phase * glow;
    return mix(fog_color, color, seen);
}

// Steps of the march through the fog for light shafts.
const SHAFT_STEPS: i32 = 16;
// The least glow (in linear color) the shafts are marched for: below it,
// darkening it to nothing changes the pixel by less than a display shows.
const SHAFT_VISIBLE: f32 = 0.002;

// Of the sunlight the fog between the camera and `world` scatters toward
// the camera, the share that isn't in shadow (`optical`: the fog's whole
// optical depth along the ray). Marched to the shadow distance, each step
// weighted by how thick the fog is there and how much of it gets through
// to the camera; past the shadows, the fog is all in sun. Shadows cut the
// glow into shafts.
fn fog_in_sun(world: vec3<f32>, optical: f32) -> f32 {
    let count = i32(frame.shadow.x);
    if count == 0 || frame.camera.w == 0.0 {
        return 1.0;
    }
    let ray = world - frame.camera.xyz;
    let span = max(length(ray), 1e-6);
    let reach = min(span, frame.shadow.w);
    let dt = reach / f32(SHAFT_STEPS);
    // A different offset each pixel and frame: the noise is fine grained,
    // and with temporal antialiasing it averages away.
    let clip = frame.view_proj * vec4<f32>(world, 1.0);
    let pixel = floor((clip.xy / clip.w * vec2<f32>(0.5, -0.5) + 0.5) * frame.screen.xy);
    let turn = f32(u32(frame.time.x * 60.0) % 64u);
    let jitter = fract(52.9829189 * fract(dot(pixel + 5.588238 * turn, vec2<f32>(0.06711056, 0.00583715))));
    // The sun's cascades are orthographic, so a point's shadow map coordinates
    // move linearly along the ray: project its two ends once per cascade and
    // interpolate, rather than projecting every step into every cascade.
    let end = frame.camera.xyz + ray * (reach / span);
    var start_at: array<vec3<f32>, 4>;
    var along: array<vec3<f32>, 4>;
    for (var c = 0; c < count; c++) {
        let a = (frame.sun_view_proj[c] * vec4<f32>(frame.camera.xyz, 1.0)).xyz;
        start_at[c] = a;
        along[c] = (frame.sun_view_proj[c] * vec4<f32>(end, 1.0)).xyz - a;
    }
    var through = 1.0;
    var lit = 0.0;
    for (var i = 0; i < SHAFT_STEPS; i++) {
        let f = (f32(i) + jitter) / f32(SHAFT_STEPS);
        let p = frame.camera.xyz + (end - frame.camera.xyz) * f;
        let thick = frame.fog.x * exp(min(-(p.y - frame.fog.y) * frame.fog.z, 20.0)) * dt;
        let scattered = through * (1.0 - exp(-thick));
        through *= exp(-thick);
        // The sharpest cascade that holds the point (all sun outside them).
        var sun = 1.0;
        for (var c = 0; c < count; c++) {
            let ndc = start_at[c] + along[c] * f;
            if max(abs(ndc.x), abs(ndc.y)) < 1.0 && ndc.z <= 1.0 {
                sun = textureSampleCompareLevel(shadow_map, shadow_sampler, vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5), c, ndc.z);
                break;
            }
        }
        lit += scattered * sun;
    }
    let all = 1.0 - exp(-optical);
    // Past the shadow distance: all in sun.
    let beyond = max(through - exp(-optical), 0.0);
    return clamp((lit + beyond) / max(all, 1e-5), 0.0, 1.0);
}

// `color` seen through all the fog between the camera and `world`.
fn fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    return fog_along(color, world, true);
}

// A pseudo-random number in 0..1 for a 2D point.
fn hash2(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

// Smooth value noise in 0..1.
fn noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Five octaves of noise2, in 0..1.
fn fbm2(p: vec2<f32>) -> f32 {
    var sum = 0.0;
    var amplitude = 0.5;
    var q = p;
    for (var i = 0; i < 5; i++) {
        sum += noise2(q) * amplitude;
        q = q * 2.03 + vec2<f32>(17.0, 31.0);
        amplitude *= 0.5;
    }
    return sum / 0.96875;
}

fn direct_light(s: Surface, l: vec3<f32>, radiance: vec3<f32>) -> vec3<f32> {
    let n = s.normal;
    let v = s.view;
    let h = (l + v) / max(length(l + v), 1e-6);
    let n_l = max(dot(n, l), 0.0);
    let n_v = max(dot(n, v), 1e-4);
    let n_h = max(dot(n, h), 0.0);
    let v_h = max(dot(v, h), 0.0);

    let a = s.roughness * s.roughness;
    let a2 = a * a;
    let d_denom = n_h * n_h * (a2 - 1.0) + 1.0;
    let d = a2 / (PI * d_denom * d_denom);
    let k = (s.roughness + 1.0) * (s.roughness + 1.0) / 8.0;
    let g = n_v / (n_v * (1.0 - k) + k) * n_l / (n_l * (1.0 - k) + k);
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metallic);
    let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - v_h, 5.0);
    let specular = d * g * f / max(4.0 * n_v * n_l, 1e-4);
    let diffuse = (vec3<f32>(1.0) - f) * (1.0 - s.metallic) * s.albedo / PI;
    return (diffuse + specular) * radiance * n_l;
}

// The froxel (16 x 9 screen cells, 24 slices of depth) holding a world
// position, or -1 when there are no clusters this frame.
fn cluster_cell(world: vec3<f32>) -> i32 {
    let clip = frame.cluster_view_proj * vec4<f32>(world, 1.0);
    if clip.w <= 0.0 || frame.cluster_depth.x <= 0.0 { return -1; }
    let xy = vec2<u32>(clamp(floor((clip.xy / clip.w * 0.5 + 0.5) * vec2<f32>(16.0, 9.0)), vec2<f32>(0.0), vec2<f32>(15.0, 8.0)));
    let z = u32(clamp(floor(log(max(clip.w, frame.cluster_depth.x) / frame.cluster_depth.x) * frame.cluster_depth.y), 0.0, 23.0));
    return i32((z * 9u + xy.y) * 16u + xy.x);
}

// Evaluate only the lights in the world position's froxel, using the same
// GGX BRDF as sunlight. Available to custom material shaders too.
fn punctual_light(s: Surface) -> vec3<f32> {
    let found = cluster_cell(s.world);
    if found < 0 { return vec3<f32>(0.0); }
    let cell = u32(found);
    let offset = cluster_word(cell * 2u);
    let count = cluster_word(cell * 2u + 1u);
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < count; i++) {
        let id = i32(cluster_word(offset + i));
        let pr = textureLoad(punctual_records, vec2<i32>(0, id), 0);
        let delta = pr.xyz - s.world;
        let d2 = dot(delta, delta);
        if d2 >= pr.w * pr.w { continue; }
        let l = delta / max(sqrt(d2), 1e-6);
        let cone = textureLoad(punctual_records, vec2<i32>(2, id), 0);
        var attenuation = 1.0;
        if cone.w > -1.0 {
            let inner = textureLoad(punctual_records, vec2<i32>(3, id), 0).x;
            let angle = dot(-l, cone.xyz);
            // A zero-width transition is an explicit hard cone.
            attenuation = select(smoothstep(cone.w, max(inner, cone.w + 1e-6), angle), select(0.0, 1.0, angle >= cone.w), inner == cone.w);
        }
        let window = max(1.0 - (d2 / (pr.w * pr.w)) * (d2 / (pr.w * pr.w)), 0.0);
        attenuation *= window * window / max(d2, 1.0);
        let shadow_info = textureLoad(punctual_records, vec2<i32>(3, id), 0);
        if shadow_info.y >= 0.0 && attenuation > 0.0 {
            attenuation *= punctual_shadow(i32(shadow_info.y), shadow_info.z, pr.xyz, s.world, s.normal, cone.w <= -1.0);
        }
        let radiance = textureLoad(punctual_records, vec2<i32>(1, id), 0).rgb * attenuation;
        sum += direct_light(s, l, radiance);
    }
    return sum;
}
// How much of a point or spot light at `light` reaches `world` (0 in its
// shadow, 1 lit), from its shadow views starting at `first`: a spot light's
// one, or a point light's six cube faces (+X, -X, +Y, -Y, +Z, -Z). `texel`
// is a texel's world size per unit of distance from the light.
fn punctual_shadow(first: i32, texel: f32, light: vec3<f32>, world: vec3<f32>, n: vec3<f32>, point: bool) -> f32 {
    if f32(first) >= frame.light_shadow.x {
        return 1.0;
    }
    // Looking up a little off the surface keeps it from shadowing itself;
    // texels grow with distance from the light.
    let p = world + n * (frame.light_shadow.z * texel * distance(world, light));
    var view = first;
    if point {
        let v = p - light;
        let a = abs(v);
        if a.x >= a.y && a.x >= a.z {
            view += select(1, 0, v.x > 0.0);
        } else if a.y >= a.z {
            view += select(3, 2, v.y > 0.0);
        } else {
            view += select(5, 4, v.z > 0.0);
        }
    }
    let clip = frame.light_view_proj[view] * vec4<f32>(p, 1.0);
    if clip.w <= 0.0 {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 || ndc.z > 1.0 {
        return 1.0;
    }
    // Into the view's atlas tile; filtering stays half a texel inside it,
    // off its neighbors.
    let rect = frame.light_rect[view];
    let uv = rect.xy + vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * rect.z;
    let step = frame.light_shadow.y;
    let lo = rect.xy + 0.5 * step;
    let hi = rect.xy + rect.z - 0.5 * step;
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let at = clamp(uv + vec2<f32>(f32(x), f32(y)) * step, lo, hi);
            lit += textureSampleCompareLevel(light_shadow_map, shadow_sampler, at, ndc.z);
        }
    }
    return lit / 9.0;
}

fn cluster_word(i: u32) -> u32 {
    return textureLoad(punctual_clusters, vec2<i32>(i32(i % 1024u), i32(i / 1024u)), 0).x;
}

fn cube_uv(d: vec3<f32>) -> vec3<f32> {
    let a = abs(d); var f = 0.0; var p = vec2<f32>(0.0);
    if a.x >= a.y && a.x >= a.z {
        if d.x > 0.0 { f = 0.0; p = vec2<f32>(-d.z, -d.y) / a.x; }
        else { f = 1.0; p = vec2<f32>(d.z, -d.y) / a.x; }
    } else if a.y >= a.z {
        if d.y > 0.0 { f = 2.0; p = vec2<f32>(d.x, d.z) / a.y; }
        else { f = 3.0; p = vec2<f32>(d.x, -d.z) / a.y; }
    } else {
        if d.z > 0.0 { f = 4.0; p = vec2<f32>(d.x, -d.y) / a.z; }
        else { f = 5.0; p = vec2<f32>(-d.x, -d.y) / a.z; }
    }
    return vec3<f32>(p * 0.5 + 0.5, f);
}

fn environment_sample(slot: f32, direction: vec3<f32>, roughness: f32, diffuse: bool) -> vec3<f32> {
    let uv = cube_uv(direction);
    if diffuse {
        let pixel = vec2<f32>(800.0 + uv.z * 16.0, 128.0 + slot * 256.0) + clamp(uv.xy * 16.0, vec2<f32>(0.5), vec2<f32>(15.5));
        return textureSampleLevel(sky_texture, sky_sampler, pixel / vec2<f32>(1024.0, 1536.0), 0.0).rgb;
    }
    let level = clamp(roughness, 0.0, 1.0) * 7.0;
    let low = floor(level); let high = min(low + 1.0, 7.0);
    let lo_size = 128.0 / exp2(low); let hi_size = 128.0 / exp2(high);
    let lo = vec2<f32>(uv.z * lo_size, 128.0 + slot * 256.0 + 256.0 - lo_size * 2.0) + clamp(uv.xy * lo_size, vec2<f32>(0.5), vec2<f32>(lo_size - 0.5));
    let hi = vec2<f32>(uv.z * hi_size, 128.0 + slot * 256.0 + 256.0 - hi_size * 2.0) + clamp(uv.xy * hi_size, vec2<f32>(0.5), vec2<f32>(hi_size - 0.5));
    return mix(textureSampleLevel(sky_texture, sky_sampler, lo / vec2<f32>(1024.0, 1536.0), 0.0).rgb,
        textureSampleLevel(sky_texture, sky_sampler, hi / vec2<f32>(1024.0, 1536.0), 0.0).rgb, fract(level));
}
fn probe_weight(p: ReflectionVolume, world: vec3<f32>) -> f32 {
    if p.center.w <= 0.0 { return 0.0; }
    let edge = p.size.xyz - abs(world - p.center.xyz);
    let distance = min(edge.x, min(edge.y, edge.z));
    if distance <= 0.0 { return 0.0; }
    return clamp(distance / max(p.size.w, 0.00001), 0.0, 1.0);
}
fn probe_direction(p: ReflectionVolume, world: vec3<f32>, ray: vec3<f32>) -> vec3<f32> {
    if p.params.y == 0.0 { return ray; }
    let local = world - p.center.xyz;
    let safe_ray = select(vec3<f32>(0.000001), ray, abs(ray) > vec3<f32>(0.000001));
    let times = (select(-p.size.xyz, p.size.xyz, safe_ray > vec3<f32>(0.0)) - local) / safe_ray;
    let t = min(times.x, min(times.y, times.z));
    return local + ray * t;
}
// The opaque pass's screen-space reflection at this pixel (see ssr.wgsl):
// what the reflected ray hit on screen, and how much it counts. Zero
// elsewhere, and where the ray found nothing.
var<private> screen_reflection: vec4<f32> = vec4<f32>(0.0);
// Global illumination's bounced light at this pixel (see gi.wgsl): rgb, and
// how much it replaces the ambient light in a. Zero without it.
var<private> bounced_light: vec4<f32> = vec4<f32>(0.0);

fn environment_light(s: Surface, f0: vec3<f32>) -> vec3<f32> {
    let ray = reflect(-s.view, s.normal);
    // The two smallest volumes around this pixel: a room inside a bigger one
    // comes first. Each takes its fade's share of what is left; the sky gets the rest.
    var first = 4u; var second = 4u; var size1 = 1.0e30; var size2 = 1.0e30;
    for (var i = 0u; i < 4u; i++) {
        let p = frame.probes[i];
        if probe_weight(p, s.world) > 0.0 {
            let size = p.size.x * p.size.y * p.size.z;
            if size < size1 { second = first; size2 = size1; first = i; size1 = size; }
            else if size < size2 { second = i; size2 = size; }
        }
    }
    var remaining = 1.0;
    var diffuse = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);
    for (var j = 0u; j < 2u; j++) {
        let index = select(first, second, j == 1u);
        if index < 4u {
            let p = frame.probes[index];
            let share = remaining * probe_weight(p, s.world) * p.params.x;
            remaining *= 1.0 - probe_weight(p, s.world);
            diffuse += environment_sample(p.center.w, s.normal, 0.0, true) * share;
            specular += environment_sample(p.center.w, probe_direction(p, s.world, ray), s.roughness, false) * share;
        }
    }
    diffuse += environment_sample(0.0, s.normal, 0.0, true) * remaining;
    specular += environment_sample(0.0, ray, s.roughness, false) * remaining;
    // Bounced light replaces the ambient light where the probes cover this
    // point. Where it's darker (indoors), reflections of the sky darken too.
    if bounced_light.a > 0.0 {
        let luma = vec3<f32>(0.2126, 0.7152, 0.0722);
        let dimmer = clamp(dot(bounced_light.rgb, luma) / max(dot(diffuse, luma), 1e-4), 0.0, 1.0);
        specular *= mix(1.0, dimmer, bounced_light.a);
        diffuse = mix(diffuse, bounced_light.rgb, bounced_light.a);
    }
    // What's on screen replaces the environment where the reflected ray hit
    // it, with filtered reflections on glossy surfaces.
    specular = mix(specular, screen_reflection.rgb, screen_reflection.a * (1.0 - smoothstep(0.4, 0.7, s.roughness)));
    let nv = max(dot(s.normal, s.view), 0.0);
    let uv = (vec2<f32>(800.0, 0.0) + clamp(vec2<f32>(nv, s.roughness) * 128.0, vec2<f32>(0.5), vec2<f32>(127.5))) / vec2<f32>(1024.0, 1536.0);
    let brdf = textureSampleLevel(sky_texture, sky_sampler, uv, 0.0).rg;
    return (diffuse * s.albedo * (1.0 - s.metallic) + specular * (f0 * brdf.x + brdf.y)) * s.occlusion;
}

// The sun's shadow, for material shaders, screen effects and compute passes.
// Where `world` falls in shadow cascade `i`: the shadow map uv, the depth
// to compare, and how near the edge of the cascade's box it is (0 at the
// center, 1 at the edge, more outside).
fn cascade_coords(i: i32, world: vec3<f32>, n: vec3<f32>) -> vec4<f32> {
    // Looking up a little off the surface keeps it from shadowing itself.
    let offset = n * frame.shadow.z * frame.cascade_texel[i];
    let clip = frame.sun_view_proj[i] * vec4<f32>(world + offset, 1.0);
    let ndc = clip.xyz / clip.w;
    let edge = max(max(abs(ndc.x), abs(ndc.y)), select(0.0, 2.0, ndc.z > 1.0));
    return vec4<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z, edge);
}

// How much light cascade `i` lets through at `c` (from `cascade_coords`),
// softened by `Lighting::shadow_filter`: one bilinear lookup (2x2 texels),
// four half a texel apart (3x3), or nine a texel apart (4x4).
fn cascade_light(i: i32, c: vec4<f32>) -> f32 {
    let soft = frame.light_shadow.w;
    if soft < 1.5 {
        return textureSampleCompareLevel(shadow_map, shadow_sampler, c.xy, i, c.z);
    }
    if soft < 2.5 {
        var four = 0.0;
        for (var y = -1; y <= 1; y += 2) {
            for (var x = -1; x <= 1; x += 2) {
                let at = c.xy + vec2<f32>(f32(x), f32(y)) * 0.5 * frame.shadow.y;
                four += textureSampleCompareLevel(shadow_map, shadow_sampler, at, i, c.z);
            }
        }
        return four / 4.0;
    }
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let at = c.xy + vec2<f32>(f32(x), f32(y)) * frame.shadow.y;
            lit += textureSampleCompareLevel(shadow_map, shadow_sampler, at, i, c.z);
        }
    }
    return lit / 9.0;
}

// How much sun reaches `world` (0 in shadow, 1 lit). The first cascade
// whose box holds the point gives the sharpest shadow; near that box's edge
// it fades into the next cascade, so the step between them doesn't show.
// Only the cascades used are sampled.
fn sunlight(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let count = i32(frame.shadow.x);
    // Towards the shadow distance, shadows fade out.
    let fade = smoothstep(0.85, 1.0, distance(world, frame.camera.xyz) / frame.shadow.w);
    if count == 0 || fade >= 1.0 {
        return 1.0;
    }
    // The 3x3 filter reaches one texel further out than the point.
    let inside = 1.0 - 2.0 * frame.shadow.y;
    var lit = 1.0;
    for (var i = 0; i < count; i++) {
        let c = cascade_coords(i, world, n);
        if c.w < inside {
            lit = cascade_light(i, c);
            let blend = smoothstep(0.85, 1.0, c.w / inside);
            if blend > 0.0 && i + 1 < count {
                lit = mix(lit, cascade_light(i + 1, cascade_coords(i + 1, world, n)), blend);
            }
            break;
        }
    }
    return mix(lit, 1.0, fade);
}
