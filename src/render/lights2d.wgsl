// 2D lighting (`lights2d.rs`). Three passes after the world's sprites:
//
// 1. Normals (only when a drawn texture has a normal map): the world's
//    sprites again, writing each pixel's world-space normal, and in alpha
//    whether it has one (0: lit flat).
// 2. The light map, cleared to the ambient light: each light is a quad
//    adding its light; a light with shadows first draws its shadow volumes
//    into the stencil buffer, and its quad skips the stencilled pixels.
//    The map holds half the light, so a pixel can be lit up to twice its
//    own color.
// 3. Composite: the frame times the light map, by the blend unit
//    (src * dst + dst * src), so it reads no copy of the frame.
//
// WebGL2-safe: no storage buffers, no compute, texel loads of color
// textures only. Each numbered part is a shader module of its own, after
// this shared part.

// The six corners of a quad, as two triangles.
const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
);

// ---- 1. normals

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;
@group(2) @binding(0) var normal_texture: texture_2d<f32>;
@group(2) @binding(1) var normal_sampler: sampler;

struct SpriteInstance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) pivot: vec2<f32>,
    @location(3) rotation: f32,
    @location(4) color: vec4<f32>,
    @location(5) uv: vec4<f32>,
};

struct NormalOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    // cos and sin of the rotation, and the signs of the size (flips).
    @location(2) @interpolate(flat) basis: vec4<f32>,
};

@vertex
fn vs_normal(@builtin(vertex_index) vi: u32, inst: SpriteInstance) -> NormalOut {
    var corners = CORNERS;
    let corner = corners[vi];
    let local = (corner - inst.pivot) * inst.size;
    let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let world = inst.pos + vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);
    var out: NormalOut;
    out.clip = camera.view_proj * vec4<f32>(world, 0.0, 1.0);
    out.uv = inst.uv.xy + corner * inst.uv.zw;
    out.alpha = inst.color.a;
    out.basis = vec4<f32>(c, s, select(1.0, -1.0, inst.size.x < 0.0), select(1.0, -1.0, inst.size.y < 0.0));
    return out;
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

// The tangent-space normal `m` (0..1, green up the image) turned into the
// world's (y down, z toward the viewer) for this sprite.
fn world_normal(m: vec3<f32>, basis: vec4<f32>) -> vec3<f32> {
    var n = m * 2.0 - 1.0;
    n = vec3<f32>(n.x * basis.z, -n.y * basis.w, n.z);
    let turned = vec2<f32>(n.x * basis.x - n.y * basis.y, n.x * basis.y + n.y * basis.x);
    return normalize(vec3<f32>(turned, max(n.z, 0.0)) + vec3<f32>(0.0, 0.0, 1e-4));
}

fn normal_pixel(in: NormalOut, srgb: bool) -> vec4<f32> {
    let alpha = textureSample(sprite_texture, sprite_sampler, in.uv).a * in.alpha;
    var m = textureSample(normal_texture, normal_sampler, in.uv);
    if alpha < 0.5 {
        discard;
    }
    // An sRGB texture's texels arrive decoded as light: encode them back.
    if srgb {
        m = vec4<f32>(srgb_encode(m.rgb), m.a);
    }
    return vec4<f32>(world_normal(m.rgb, in.basis) * 0.5 + 0.5, m.a);
}

// Normal maps stored as sRGB images (PNGs loaded as textures).
@fragment
fn fs_normal_srgb(in: NormalOut) -> @location(0) vec4<f32> {
    return normal_pixel(in, true);
}

// Normal maps in a linear format (block-compressed, data textures).
@fragment
fn fs_normal_linear(in: NormalOut) -> @location(0) vec4<f32> {
    return normal_pixel(in, false);
}

// ---- 2. the light map

struct View {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var normals: texture_2d<f32>;

@vertex
fn vs_shadow(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
    return view.view_proj * vec4<f32>(pos, 0.0, 1.0);
}

// Shadow volumes write only the stencil.
@fragment
fn fs_shadow() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

struct Light {
    // Position, radius (0: directional) and height.
    @location(0) place: vec4<f32>,
    // Color (linear, halved) and falloff.
    @location(1) color: vec4<f32>,
    // Direction and the cone's cosines (outer, inner; -2, -1 without).
    @location(2) cone: vec4<f32>,
    // Directional: the unit vector toward the light.
    @location(3) toward: vec4<f32>,
};

struct LightOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec2<f32>,
    @location(1) @interpolate(flat) place: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) cone: vec4<f32>,
    @location(4) @interpolate(flat) toward: vec4<f32>,
};

@vertex
fn vs_light(@builtin(vertex_index) vi: u32, l: Light) -> LightOut {
    var corners = CORNERS;
    let corner = corners[vi] * 2.0 - 1.0;
    var out: LightOut;
    if l.place.z > 0.0 {
        // A square around the light's reach.
        out.world = l.place.xy + corner * l.place.z;
        out.clip = view.view_proj * vec4<f32>(out.world, 0.0, 1.0);
    } else {
        // Directional: the whole screen.
        out.world = vec2<f32>(0.0);
        out.clip = vec4<f32>(corner, 0.0, 1.0);
    }
    out.place = l.place;
    out.color = l.color;
    out.cone = l.cone;
    out.toward = l.toward;
    return out;
}

@fragment
fn fs_light(in: LightOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(normals));
    let n = textureLoad(normals, min(vec2<i32>(in.clip.xy), size - 1), 0);
    var light = in.color.rgb;
    var toward = in.toward.xyz;
    if in.place.z > 0.0 {
        let d = in.world - in.place.xy;
        let distance = length(d);
        let t = clamp(1.0 - distance / in.place.z, 0.0, 1.0);
        var fade = select(pow(t, in.color.w), 0.0, t <= 0.0);
        if in.cone.z > -1.5 {
            let along = dot(d / max(distance, 1e-4), in.cone.xy);
            fade *= smoothstep(in.cone.z, in.cone.w, along);
        }
        light *= fade;
        toward = normalize(vec3<f32>(-d, in.place.w) + vec3<f32>(0.0, 0.0, 1e-4));
    }
    // Pixels with a normal map are shaded by it; the rest are lit flat.
    let facing = max(dot(normalize(n.xyz * 2.0 - 1.0), toward), 0.0);
    return vec4<f32>(light * mix(1.0, facing, n.a), 0.0);
}

// ---- 3. composite

@group(0) @binding(0) var light_map: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(textureLoad(light_map, vec2<i32>(at.xy), 0).rgb, 1.0);
}
