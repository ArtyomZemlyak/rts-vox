// Shared by the voxel depth/normal prepass and colour/volume pass.
struct Params {
    world_from_clip: mat4x4<f32>, camera: vec4<f32>, origin: vec4<f32>, source: vec4<f32>,
    control: vec4<f32>, chemistry: vec4<f32>, charge: vec4<f32>, terrain: vec4<f32>,
    terrain_origin: vec4<f32>,
    clip_from_world: mat4x4<f32>, sun: vec4<f32>, sky: vec4<f32>,
    energy:vec4<f32>, beam:vec4<f32>, weapon:vec4<f32>, look:vec4<f32>,
};
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(5) var<storage, read> terrain: array<u32>;
@group(0) @binding(7) var<storage, read> bricks: array<u32>;
// The hierarchy is enabled only for aligned windows. Small test layouts
// remain valid without padding or changing their physical storage.
fn brick_ceiling(q:vec3<i32>)->i32 {
    let dims=vec3<u32>(p.terrain.xyz);let lo=vec3<i32>(p.terrain_origin.xyz);
    if any(dims==vec3(0u)) || any((dims&vec3(7u))!=vec3(0u)) ||
       any(lo<vec3(0)) || any((vec3<u32>(lo)&vec3(7u))!=vec3(0u)) {return q.y+1;}
    if any(q<lo) || any(q>=lo+vec3<i32>(dims)) {return q.y+1;}
    let b=(vec3<u32>(q)%dims)/8u;let size=dims/8u;
    return i32(floor(f32(q.y)/8.0))*8+i32(bricks[b.x+size.x*(b.y+size.y*b.z)]);
}
struct In { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
// The volume is a window on the world, not the whole of it (ADR-318):
// it holds `dims` voxels from `lo`, and its storage wraps, so a voxel
// lives at `q % dims` whatever the window's corner. Unsigned modulo
// throughout: a signed one is forbidden on our backend.
fn raw_material(q: vec3<i32>) -> u32 {
    let dims=vec3<i32>(p.terrain.xyz);
    let lo=vec3<i32>(p.terrain_origin.xyz);
    if any(q<lo) || any(q>=lo+dims) {return 0u;}
    let m=vec3<u32>(q)%vec3<u32>(dims);
    let i=m.x+u32(dims.x)*(m.y+u32(dims.y)*m.z);
    return (terrain[i/8u]>>((i&7u)*4u))&15u;
}
struct TerrainHit { t:f32, normal:vec3<f32>, material:u32 };
fn trace_terrain(ro:vec3<f32>,rd:vec3<f32>,limit:f32,shadow:bool)->TerrainHit {
    let inv=1.0/(rd+vec3(0.0000001));
    let aa=(p.terrain_origin.xyz-ro)*inv;
    let bb=(p.terrain_origin.xyz+p.terrain.xyz-ro)*inv;
    let a=min(aa,bb);let b=max(aa,bb);
    var t=max(0.0,max(a.x,max(a.y,a.z)));var end=min(limit,min(b.x,min(b.y,b.z)));
    var normal=vec3(0.0,1.0,0.0);
    // Nothing in the window stands above this height (ADR-321). A ray up
    // there meets nothing at all if it climbs, and if it falls it may go
    // straight to that height instead of stepping through empty sky a
    // tenth of a metre at a time. The march is the cost of the picture,
    // and sky was most of it.
    let top=p.terrain_origin.w;
    if (rd.y>0.0) {
        // A climbing ray, which a shadow ray usually is, is done the
        // moment it passes the top: there is nothing above to meet.
        end=min(end,(top-ro.y)*inv.y);
    } else if (ro.y+rd.y*t>top) {
        // A falling ray starts at the top instead of stepping down
        // through empty sky a tenth of a metre at a time.
        t=max(t,(top-ro.y)*inv.y);
    }
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
        var low=vec3<f32>(q);var high=low+vec3(1.0);
        let ceiling=brick_ceiling(q);
        if q.y>=ceiling {
            low=floor(low/8.0)*8.0;high=low+vec3(8.0);
            low.y=f32(ceiling);
        }
        let face=select(low,high,rd>vec3(0.0));
        let dist=(face-pos)*inv;let d=max(0.002,min(dist.x,min(dist.y,dist.z)));
        normal=vec3(0.0);
        if dist.x<=dist.y && dist.x<=dist.z {normal.x=-sign(rd.x);}
        else if dist.y<=dist.z {normal.y=-sign(rd.y);} else {normal.z=-sign(rd.z);}
        t+=d+0.002;
    }
    return TerrainHit(limit,normal,0u);
}
