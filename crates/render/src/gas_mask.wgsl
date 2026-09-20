// Exact occupancy of the presentation field, derived from the current GPU
// terrain window. No readback and no changes to authoritative terrain.
struct Params { origin: vec4<i32>, window: vec4<i32>, dims: vec4<u32> };
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> terrain: array<u32>;
@group(0) @binding(2) var<storage, read_write> mask: array<u32>;
@compute @workgroup_size(4, 4, 4)
fn build(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= vec3(48u)) { return; }
    let index = id.x + 48u * (id.y + 48u * id.z);
    let base = p.origin.xyz + vec3<i32>(id) * p.origin.w;
    let lo = max(base, p.window.xyz);
    let hi = min(base + vec3(p.origin.w), p.window.xyz + vec3<i32>(p.dims.xyz));
    for (var z = lo.z; z < hi.z; z++) {
        for (var y = lo.y; y < hi.y; y++) {
            // Coordinates within a terrain window are nonnegative. Unsigned
            // remainder matches the CPU's toroidal storage on every backend.
            let row = (u32(z) % p.dims.z * p.dims.y + u32(y) % p.dims.y) * p.dims.x;
            for (var x = lo.x; x < hi.x; x++) {
                let i = row + u32(x) % p.dims.x;
                if i / 8u < arrayLength(&terrain) && (terrain[i / 8u] >> ((i % 8u) * 4u) & 15u) != 0u {
                    mask[index] = 1u;
                    return;
                }
            }
        }
    }
    mask[index] = 0u;
}
