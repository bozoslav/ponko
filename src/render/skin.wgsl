// Skinning: poses one skinned model's corners into its own vertex buffer,
// which every pass then draws like any mesh. A vertex is 11 words: position
// (3), normal (3), uv (2), color (1, RGBA8), uv1 (2). Morph targets are
// added first, then the corner is skinned.

struct Params {
    // The model's first skin matrix in `joints`, and its corner count.
    first: u32,
    count: u32,
    // The model's first morph weight in `morph_weights`, and how many.
    morph_first: u32,
    morph_count: u32,
}

@group(0) @binding(0) var<storage, read> rest: array<f32>;
// Per corner: 4 joints as u16 pairs (2 words), then 4 weights (4 words).
@group(0) @binding(1) var<storage, read> influences: array<u32>;
@group(0) @binding(2) var<storage, read> joints: array<mat4x4<f32>>;
@group(0) @binding(3) var<storage, read_write> posed: array<f32>;
@group(0) @binding(4) var<uniform> params: Params;
@group(0) @binding(5) var<storage, read> morph_weights: array<f32>;
// Per target, per corner: position offset (3 words), normal offset (3).
@group(0) @binding(6) var<storage, read> deltas: array<f32>;

// Corner `i`'s blended skin matrix.
fn skin_matrix(i: u32) -> mat4x4<f32> {
    let s = i * 6u;
    let packed = vec2<u32>(influences[s], influences[s + 1u]);
    let which = vec4<u32>(packed.x & 0xffffu, packed.x >> 16u, packed.y & 0xffffu, packed.y >> 16u) + params.first;
    let w = vec4<f32>(
        bitcast<f32>(influences[s + 2u]),
        bitcast<f32>(influences[s + 3u]),
        bitcast<f32>(influences[s + 4u]),
        bitcast<f32>(influences[s + 5u]),
    );
    return joints[which.x] * w.x + joints[which.y] * w.y + joints[which.z] * w.z + joints[which.w] * w.w;
}

@compute @workgroup_size(64)
fn skin(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.count {
        return;
    }
    let v = i * 11u;
    let m = skin_matrix(i);
    var rp = vec3<f32>(rest[v], rest[v + 1u], rest[v + 2u]);
    var rn = vec3<f32>(rest[v + 3u], rest[v + 4u], rest[v + 5u]);
    for (var t = 0u; t < params.morph_count; t++) {
        let mw = morph_weights[params.morph_first + t];
        if mw != 0.0 {
            let d = (t * params.count + i) * 6u;
            rp += mw * vec3<f32>(deltas[d], deltas[d + 1u], deltas[d + 2u]);
            rn += mw * vec3<f32>(deltas[d + 3u], deltas[d + 4u], deltas[d + 5u]);
        }
    }
    let p = m * vec4<f32>(rp, 1.0);
    let n0 = (m * vec4<f32>(rn, 0.0)).xyz;
    let n = select(vec3<f32>(0.0, 1.0, 0.0), normalize(n0), dot(n0, n0) > 1e-20);
    posed[v] = p.x;
    posed[v + 1u] = p.y;
    posed[v + 2u] = p.z;
    posed[v + 3u] = n.x;
    posed[v + 4u] = n.y;
    posed[v + 5u] = n.z;
    for (var k = 6u; k < 11u; k++) {
        posed[v + k] = rest[v + k];
    }
}

// The same corners' positions only (normals and the rest are left as they
// are): the last frame's pose, which only the motion-vector pass reads.
@compute @workgroup_size(64)
fn skin_positions(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.count {
        return;
    }
    let v = i * 11u;
    var rp = vec3<f32>(rest[v], rest[v + 1u], rest[v + 2u]);
    for (var t = 0u; t < params.morph_count; t++) {
        let mw = morph_weights[params.morph_first + t];
        if mw != 0.0 {
            let d = (t * params.count + i) * 6u;
            rp += mw * vec3<f32>(deltas[d], deltas[d + 1u], deltas[d + 2u]);
        }
    }
    let p = skin_matrix(i) * vec4<f32>(rp, 1.0);
    posed[v] = p.x;
    posed[v + 1u] = p.y;
    posed[v + 2u] = p.z;
}
