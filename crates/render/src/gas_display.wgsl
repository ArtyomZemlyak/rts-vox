// terrain_common and gas_sampling are prepended by the host.
@group(0) @binding(8) var<storage,read_write> display:array<DisplayCell>;
@compute @workgroup_size(64)
fn build(@builtin(global_invocation_id) id:vec3<u32>) {
    let side=display_side();let i=id.x;
    if i>=side*side*side {return;}
    let cell=vec3<i32>(i32(i%side),i32(i/side%side),i32(i/(side*side)));
    display[i]=display_cell(cell);
}
