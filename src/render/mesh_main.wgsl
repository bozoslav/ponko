// The entry points of every material shader (see mesh.wgsl).

struct VertexIn {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) color: vec4<f32>,  // the model's tint
    @location(8) vertex_color: vec4<f32>,
    @location(9) uv1: vec2<f32>,
};

// STATIC_INSTANCE_HELPER

struct VertexOut {
    // Invariant: the depth prepass and the main pass must land on exactly the same depth.
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) @interpolate(flat) origin: vec3<f32>,
    // Which decal layers the model takes (see apply_decals).
    @location(6) @interpolate(flat) decal_layers: u32,
    // The crossfade dither (see `fade_keeps`), else 0.
    @location(7) @interpolate(flat) fade: f32,
};

// The vertex in world space, after the shader's `vertex` function.
fn world_vertex(input: VertexIn) -> Vertex {
    let in = static_instance(input);
    // The matrix's last row is (0, 0, 0, 1); the first slot of it carries the
    // model's decal layers instead (see instance_decal_layers).
    let model = mat4x4<f32>(vec4<f32>(in.m0.xyz, 0.0), vec4<f32>(in.m1.xyz, 0.0), vec4<f32>(in.m2.xyz, 0.0), vec4<f32>(in.m3.xyz, 1.0));
    // For a scale-rotate matrix M = R S, the normal matrix R S^-1 is M S^-2.
    let scale = vec3<f32>(length(in.m0.xyz), length(in.m1.xyz), length(in.m2.xyz));
    let normal = mat3x3<f32>(in.m0.xyz, in.m1.xyz, in.m2.xyz) * (in.normal / max(scale * scale, vec3<f32>(1e-12)));
    var v: Vertex;
    v.instance_index = in.instance_index;
    v.position = (model * vec4<f32>(in.position, 1.0)).xyz;
    v.normal = normal;
    v.uv = in.uv;
    v.color = in.color * in.vertex_color;
    v.uv1 = in.uv1;
    v.origin = in.m3.xyz;
    var out = vertex(v);
    out.origin = v.origin;
    return out;
}

// The decal layers the model takes, as the engine put them in the instance.
fn instance_decal_layers(in: VertexIn) -> u32 {
    return u32(static_instance(in).m0.w + 0.5);
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let v = world_vertex(in);
    var out: VertexOut;
    out.decal_layers = instance_decal_layers(in);
    out.clip = frame.view_proj * vec4<f32>(v.position, 1.0);
    out.world = v.position;
    out.normal = v.normal;
    out.uv = v.uv;
    out.color = v.color;
    out.uv1 = v.uv1;
    out.origin = v.origin;
    out.fade = in.m1.w;
    return out;
}

// The shadow map: depth seen from the sun.
@vertex
fn vs_shadow(in: VertexIn) -> VertexOut {
    let v = world_vertex(in);
    var out: VertexOut;
    out.decal_layers = 0u;
    out.clip = frame.caster_view_proj * vec4<f32>(v.position, 1.0);
    out.world = v.position;
    out.normal = v.normal;
    out.uv = v.uv;
    out.color = v.color;
    out.uv1 = v.uv1;
    out.origin = v.origin;
    out.fade = in.m1.w;
    return out;
}

// Whether a pixel of a crossfading instance is drawn: a screen-door dither.
// `fade` is the share of pixels kept; negative for the level fading out,
// which keeps the pixels the level fading in doesn't (so together they
// cover every pixel once). The depth prepass, the main pass and the shadow
// pass all use this on the pixel they draw, so they agree.
fn fade_keeps(fade: f32, pixel: vec2<f32>) -> bool {
    if fade == 0.0 {
        return true;
    }
    // Interleaved gradient noise (Jimenez): an even spread of 0..1 per pixel.
    let p = floor(pixel);
    let t = fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
    if fade > 0.0 {
        return t < fade;
    }
    return t >= 1.0 + fade;
}

// The surface from the material and its textures, then the shader's `surface`.
fn surface_at(in: VertexOut, front: bool) -> Surface {
    let base = textureSample(base_texture, material_sampler, in.uv) * material.color * in.color;
    var s: Surface;
    s.albedo = base.rgb;
    s.alpha = base.a;
    s.world = in.world;
    s.uv = in.uv;
    s.uv1 = in.uv1;
    s.origin = in.origin;
    s.color = in.color;
    s.view = normalize(frame.camera.xyz - in.world);
    s.screen = in.clip.xy;
    var n = normalize(in.normal);
    if !front {
        n = -n; // the back of a double-sided surface
    }
    if material.maps.x != 0u {
        // Tangent space from screen derivatives (Schüler's cotangent frame):
        // no tangents needed in the mesh.
        let dp1 = dpdx(in.world);
        let dp2 = dpdy(in.world);
        let duv1 = dpdx(in.uv);
        let duv2 = dpdy(in.uv);
        let dp2perp = cross(dp2, n);
        let dp1perp = cross(n, dp1);
        let t = dp2perp * duv1.x + dp1perp * duv2.x;
        let b = dp2perp * duv1.y + dp1perp * duv2.y;
        let inv = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
        let texel = textureSample(normal_texture, material_sampler, in.uv);
        var m = texel.xyz * 2.0 - 1.0;
        // Two-channel normal maps (BC5, EAC; 3: X and Y in red and alpha)
        // store no Z: it is the rest of the unit length.
        if material.maps.x >= 2u {
            if material.maps.x == 3u {
                m.y = texel.a * 2.0 - 1.0;
            }
            m.z = sqrt(max(1.0 - dot(m.xy, m.xy), 0.0));
        }
        m = vec3<f32>(m.xy * material.params.z, m.z);
        // glTF's texture coordinates run down the image: flip the bitangent.
        n = normalize(t * inv * m.x - b * inv * m.y + n * m.z);
    }
    s.normal = n;
    s.roughness = material.params.x;
    s.metallic = material.params.y;
    s.occlusion = 1.0;
    if material.maps.y != 0u {
        let orm = textureSample(orm_texture, material_sampler, in.uv);
        s.occlusion = orm.r;
        s.roughness *= orm.g;
        s.metallic *= orm.b;
    }
    s.emissive = material.emissive.rgb;
    if material.maps.z != 0u {
        s.emissive *= textureSample(emissive_texture, material_sampler, in.uv).rgb;
    }
    s = surface(s);
    s.roughness = clamp(s.roughness, 0.03, 1.0);
    s.metallic = clamp(s.metallic, 0.0, 1.0);
    return s;
}

// The depth prepass and the shadow map of cut-out materials (Blend::Cutout):
// depth only, minus the pixels their alpha cuts out. Solid materials use no
// fragment shader there.
@fragment
fn fs_depth(in: VertexOut, @builtin(front_facing) front: bool) {
    if surface_at(in, front).alpha < 0.5 {
        discard;
    }
}

// The same for crossfading instances (see `fade_keeps`).
@fragment
fn fs_depth_fade(in: VertexOut, @builtin(front_facing) front: bool) {
    if !fade_keeps(in.fade, in.clip.xy) {
        discard;
    }
    if surface_at(in, front).alpha < 0.5 {
        discard;
    }
}

// Alpha to coverage (cut-out materials with multisampling): the alpha
// sharpened to a ramp about a pixel wide around the cutoff, so the edge gets
// partial coverage instead of a hard cut. Called in uniform control flow.
fn coverage(alpha: f32) -> f32 {
    return clamp((alpha - 0.5) / max(fwidth(alpha), 0.0001) + 0.5, 0.0, 1.0);
}

// The depth prepass of cut-out materials with multisampling: the alpha goes
// to coverage (the color target isn't written).
@fragment
fn fs_depth_coverage(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, coverage(surface_at(in, front).alpha));
}

@fragment
fn fs_depth_coverage_fade(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let a = coverage(surface_at(in, front).alpha);
    if !fade_keeps(in.fade, in.clip.xy) {
        discard;
    }
    return vec4<f32>(0.0, 0.0, 0.0, a);
}

// Opaque cut-outs with multisampling: the same coverage as their prepass, so
// the depth test keeps exactly the samples it wrote.
@fragment
fn fs_main_coverage(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let dp_x = dpdx(in.world);
    let dp_y = dpdy(in.world);
    let a = coverage(surface_at(in, front).alpha);
    return vec4<f32>(opaque_color(in, front, dp_x, dp_y).rgb, a);
}

@fragment
fn fs_main_coverage_fade(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let dp_x = dpdx(in.world);
    let dp_y = dpdy(in.world);
    let a = coverage(surface_at(in, front).alpha);
    if !fade_keeps(in.fade, in.clip.xy) {
        discard;
    }
    return vec4<f32>(opaque_color(in, front, dp_x, dp_y).rgb, a);
}

// The depth prepass and shadow map of a crossfading instance of a solid
// material: only the dither.
@fragment
fn fs_fade(in: VertexOut) {
    if !fade_keeps(in.fade, in.clip.xy) {
        discard;
    }
}

// The opaque pass's contact shadow at this pixel (see opaque_color).
var<private> contact_shadow: f32 = 0.0;

fn shade(s: Surface) -> vec3<f32> {
    if s.unlit { return s.albedo + s.emissive; }
    let n = s.normal;
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metallic);
    var sun = 0.0;
    if dot(n, frame.sun_dir.xyz) > 0.0 { sun = sunlight(s.world, n) * (1.0 - contact_shadow); }
    let direct = direct_light(s, frame.sun_dir.xyz, frame.sun_color.rgb * sun) + punctual_light(s);

    bounced_light = gi_light(s.world, n, s.view);
    let ambient = environment_light(s, f0);

    // Linear HDR light; the post pass maps it to the screen.
    return direct + ambient + s.emissive;
}

// Opaque: the prepass already cut the cut-outs.
@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Screen derivatives need uniform control flow: taken before any branch.
    return opaque_color(in, front, dpdx(in.world), dpdy(in.world));
}

// Opaque, for crossfading instances (see `fade_keeps`).
@fragment
fn fs_main_fade(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let dp_x = dpdx(in.world);
    let dp_y = dpdy(in.world);
    if !fade_keeps(in.fade, in.clip.xy) {
        discard;
    }
    return opaque_color(in, front, dp_x, dp_y);
}

fn opaque_color(in: VertexOut, front: bool, dp_x: vec3<f32>, dp_y: vec3<f32>) -> vec4<f32> {
    var s = surface_at(in, front);
    var geometric = normalize(in.normal);
    if !front {
        geometric = -geometric;
    }
    s = apply_decals(s, geometric, in.decal_layers, dp_x, dp_y);
    s.roughness = clamp(s.roughness, 0.03, 1.0);
    s.metallic = clamp(s.metallic, 0.0, 1.0);
    // The opaque pass uses the otherwise unused scene-color slot for GTAO
    // occlusion (zero in the no-scene stand-in). Transparent passes retain
    // the actual scene color and never sample it as occlusion.
    // Green holds the contact shadow, which blocks only the sun.
    let ao_at = clamp(vec2<i32>(s.screen), vec2<i32>(0), vec2<i32>(textureDimensions(scene_color_texture)) - 1);
    let screen = textureLoad(scene_color_texture, ao_at, 0);
    s.occlusion *= 1.0 - screen.r;
    contact_shadow = screen.g;
    // Blue marks screen-space reflections in a second level, at half size.
    if screen.b > 0.5 && s.roughness < 0.7 {
        let size = vec2<f32>(textureDimensions(scene_color_texture));
        let last = min(5.0, floor(log2(max(size.x, size.y))));
        // Fixed screen-space footprint: up to 32 full-size pixels, blended
        // between the two nearest levels.
        let lod = clamp(1.0 + max(s.roughness - 0.05, 0.0) * 9.0, 1.0, last);
        let low = i32(floor(lod));
        let uv = s.screen / size;
        var filtered = reflection_level(uv, low);
        if fract(lod) > 0.0 && low < i32(last) {
            filtered = mix(filtered, reflection_level(uv, low + 1), fract(lod));
        }
        screen_reflection = vec4<f32>(filtered.rgb / max(filtered.a, 0.00001), filtered.a);
    }
    return vec4<f32>(fog(shade(s), s.world), 1.0);
}

// Transparent: blended over what's behind it (premultiplied alpha).
@fragment
fn fs_transparent(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return transparent_color(in, front);
}

@fragment
fn fs_transparent_fade(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if !fade_keeps(in.fade, in.clip.xy) {
        discard;
    }
    return transparent_color(in, front);
}

fn transparent_color(in: VertexOut, front: bool) -> vec4<f32> {
    let s = surface_at(in, front);
    return vec4<f32>(fog(shade(s), s.world) * s.alpha, s.alpha);
}

// ---- Motion vectors (temporal antialiasing) ----
// Models that moved since last frame are drawn again, over the depth the
// opaque pass left, to say where each pixel was: the same vertices through
// last frame's model matrix (the `vertex` function still sees this frame's
// time, so animation inside it is not part of the motion). A skinned or
// morphing model whose pose changed brings its last pose's positions in
// `previous_position`; every other mesh brings its own positions again.
// A buffer-driven mesh (see `Vertex.instance_index`) gets the entity's
// rigid motion: both matrices see the same buffer element, so what the
// buffer does per element is not part of the motion either.
struct VelocityIn {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) color: vec4<f32>,
    @location(8) vertex_color: vec4<f32>,
    @location(9) uv1: vec2<f32>,
    @location(10) p0: vec4<f32>,
    @location(11) p1: vec4<f32>,
    @location(12) p2: vec4<f32>,
    @location(13) p3: vec4<f32>,
    @location(14) previous_position: vec3<f32>,
};

struct VelocityOut {
    // The same position as `vs_main`, so depth matches exactly.
    @builtin(position) @invariant clip: vec4<f32>,
    // Previous camera, with previous and current object transforms.
    @location(0) before: vec4<f32>,
    @location(1) rigid: vec4<f32>,
};

@vertex
fn vs_velocity(in: VelocityIn) -> VelocityOut {
    var a: VertexIn;
    a.instance_index = in.instance_index;
    a.position = in.position;
    a.normal = in.normal;
    a.uv = in.uv;
    a.m0 = in.m0;
    a.m1 = in.m1;
    a.m2 = in.m2;
    a.m3 = in.m3;
    a.color = in.color;
    a.vertex_color = in.vertex_color;
    a.uv1 = in.uv1;
    var b = a;
    b.m0 = in.p0;
    b.m1 = in.p1;
    b.m2 = in.p2;
    b.m3 = in.p3;
    b.position = in.previous_position;
    let now = world_vertex(a);
    let before = world_vertex(b);
    var out: VelocityOut;
    out.clip = frame.view_proj * vec4<f32>(now.position, 1.0);
    out.rigid = frame.previous_view_proj * vec4<f32>(now.position, 1.0);
    out.before = frame.previous_view_proj * vec4<f32>(before.position, 1.0);
    return out;
}

// Object-only correction to backward history lookup in texture coordinates,
// minus the camera-only motion; static pixels keep the clear value zero.
@fragment
fn fs_velocity(in: VelocityOut) -> @location(0) vec2<f32> {
    if in.before.w <= 1e-5 || in.rigid.w <= 1e-5 {
        return vec2<f32>(0.0);
    }
    let a = in.rigid.xy / in.rigid.w;
    let b = in.before.xy / in.before.w;
    return vec2<f32>((b.x - a.x) * 0.5, (a.y - b.y) * 0.5);
}

// Reflection mips contain confidence-premultiplied radiance. Material
// shaders have no sampler of their own to spare: this borrows the sky's
// linear one, which wraps around horizontally, so the lookup stays half a
// pixel inside the edges.
fn reflection_level(uv: vec2<f32>, level: i32) -> vec4<f32> {
    let half = 0.5 / vec2<f32>(textureDimensions(scene_color_texture, level));
    return textureSampleLevel(scene_color_texture, sky_sampler, clamp(uv, half, 1.0 - half), f32(level));
}
