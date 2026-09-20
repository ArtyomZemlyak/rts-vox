// Sparse presentation updates: every destination appears exactly once after
// CPU range merging. No atomics, readback, or whole-volume copy is needed.
@group(0) @binding(0) var<storage, read> patches: array<vec2<u32>>;
@group(0) @binding(1) var<storage, read_write> terrain: array<u32>;
@compute @workgroup_size(64)
fn apply(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let i = id.x + id.y * groups.x * 64u;
    if i >= arrayLength(&patches) { return; }
    let change = patches[i];
    if change.x < arrayLength(&terrain) { terrain[change.x] = change.y; }
}
