// Conservative support of potentially visible gas. Half the display cutoff
// keeps a generous margin for interpolation and floating-point rounding.
struct Cell { flow: vec4<f32>, matter: vec4<f32> };
@group(0) @binding(4) var<storage, read> gas: array<Cell>;
@group(0) @binding(12) var<storage, read_write> bounds: array<atomic<u32>>;
var<workgroup> local_bounds: array<atomic<u32>, 6>;

@compute @workgroup_size(64)
fn build(@builtin(global_invocation_id) id: vec3<u32>,
         @builtin(local_invocation_index) lane: u32) {
    if lane < 6u { atomicStore(&local_bounds[lane], 0u); }
    workgroupBarrier();
    if id.x < 48u*48u*48u {
        let m = gas[id.x].matter;
        if (m.w*1.1+m.y*0.22)*1.55 > 0.06 {
            let q = vec3<u32>(id.x%48u, (id.x/48u)%48u, id.x/(48u*48u));
            for(var axis=0u;axis<3u;axis+=1u) {
                atomicMax(&local_bounds[axis], 48u-q[axis]);
                atomicMax(&local_bounds[axis+3u], q[axis]+1u);
            }
        }
    }
    workgroupBarrier();
    if lane < 6u {
        let value = atomicLoad(&local_bounds[lane]);
        if value > 0u { atomicMax(&bounds[lane], value); }
    }
}
