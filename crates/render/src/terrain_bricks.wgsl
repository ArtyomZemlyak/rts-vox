// One workgroup per physical 8^3 brick of the wrapped material window.
// A packed word is an entire x row. Recompute, including removals: an old
// ceiling must descend after the wall has been destroyed. Store the highest
// occupied local y plus one (zero is empty), rather than a binary flag.
@group(0) @binding(0) var<uniform> config: vec4<u32>; // voxel dims, count
@group(0) @binding(1) var<storage, read> terrain: array<u32>;
@group(0) @binding(2) var<storage, read> dirty: array<u32>;
@group(0) @binding(3) var<storage, read_write> bricks: array<u32>;
var<workgroup> ceiling: atomic<u32>;
@compute @workgroup_size(64)
fn build(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let item=group.x+group.y*65535u;
    let count=config.w&0x7fffffffu;
    if item>=count {return;}
    var index=item;
    if (config.w&0x80000000u)==0u {index=dirty[item];}
    let dims=config.xyz/8u;
    let b=vec3(index%dims.x,index/dims.x%dims.y,index/(dims.x*dims.y));
    if lane==0u {atomicStore(&ceiling,0u);}
    workgroupBarrier();
    let y=b.y*8u+lane%8u;let z=b.z*8u+lane/8u;
    if terrain[b.x+dims.x*(y+config.y*z)]!=0u {atomicMax(&ceiling,lane%8u+1u);}
    workgroupBarrier();
    if lane==0u {bricks[index]=atomicLoad(&ceiling);}
}
