// Shared by the voxel depth/normal prepass and colour/volume pass.
struct Params {
    world_from_clip: mat4x4<f32>, camera: vec4<f32>, origin: vec4<f32>, source: vec4<f32>,
    control: vec4<f32>, chemistry: vec4<f32>, charge: vec4<f32>, terrain: vec4<f32>,
    clip_from_world: mat4x4<f32>, sun: vec4<f32>, sky: vec4<f32>,
    energy:vec4<f32>, beam:vec4<f32>, weapon:vec4<f32>, look:vec4<f32>,
};
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(5) var<storage, read> terrain: array<u32>;
struct In { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
fn raw_material(q: vec3<i32>) -> u32 {
    let dims=vec3<i32>(p.terrain.xyz);
    if any(q<vec3(0)) || any(q>=dims) {return 0u;}
    let i=u32(q.x+dims.x*(q.y+dims.y*q.z));
    return (terrain[i/8u]>>((i&7u)*4u))&15u;
}
struct TerrainHit { t:f32, normal:vec3<f32>, material:u32 };
fn trace_terrain(ro:vec3<f32>,rd:vec3<f32>,limit:f32,shadow:bool)->TerrainHit {
    let inv=1.0/(rd+vec3(0.0000001));let aa=-ro*inv;let bb=(p.terrain.xyz-ro)*inv;
    let a=min(aa,bb);let b=max(aa,bb);
    var t=max(0.0,max(a.x,max(a.y,a.z)));let end=min(limit,min(b.x,min(b.y,b.z)));
    var normal=vec3(0.0,1.0,0.0);
    for(var j=0;j<1050;j++) {
        if t>=end {break;}
        let pos=ro+rd*(t+0.001);let q=vec3<i32>(floor(pos));let m=raw_material(q);
        // 15 is the render-only occupancy of a PBR unit, used just for shadows.
        if m>0u && (m!=15u || shadow) {
            // Persisted work selects an eroded subcell surface. Collision and
            // shadows retain the authoritative solid voxel until it fractures.
            var inset=0.0;
            if m>=9u && m<=11u && !shadow {
                let grain=fract(sin(dot(vec3<f32>(q),vec3(17.1,7.7,23.3)))*437.1);
                inset=(0.06+0.065*f32(m-8u))*(0.65+abs(grain)*0.35);
            }
            return TerrainHit(t+inset,normal,m);
        }
        let face=select(vec3<f32>(q),vec3<f32>(q)+vec3(1.0),rd>vec3(0.0));
        let dist=(face-pos)*inv;let d=max(0.002,min(dist.x,min(dist.y,dist.z)));
        normal=vec3(0.0);
        if dist.x<=dist.y && dist.x<=dist.z {normal.x=-sign(rd.x);}
        else if dist.y<=dist.z {normal.y=-sign(rd.y);} else {normal.z=-sign(rd.z);}
        t+=d+0.002;
    }
    return TerrainHit(limit,normal,0u);
}
