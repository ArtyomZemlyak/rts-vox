// Sparse presentation updates. A span is (destination, value offset, length)
// in three u32s. The values are contiguous, so a long run needs one address
// instead of an index alongside every word. CPU merging makes spans disjoint.
@group(0) @binding(0) var<storage, read> spans: array<u32>;
@group(0) @binding(1) var<storage, read> values: array<u32>;
@group(0) @binding(2) var<storage, read_write> terrain: array<u32>;
@compute @workgroup_size(64)
fn apply(@builtin(workgroup_id) group: vec3<u32>,
         @builtin(local_invocation_index) lane: u32,
         @builtin(num_workgroups) groups: vec3<u32>) {
    let span_id = (group.x + group.y * groups.x) * 4u + lane / 16u;
    let descriptor = span_id * 3u;
    if descriptor + 2u >= arrayLength(&spans) { return; }
    let start = spans[descriptor];
    let offset = spans[descriptor + 1u];
    let length = spans[descriptor + 2u];
    for (var j = lane % 16u; j < length; j += 16u) {
        if start + j < arrayLength(&terrain) && offset + j < arrayLength(&values) {
            terrain[start + j] = values[offset + j];
        }
    }
}
