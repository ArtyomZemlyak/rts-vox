// Shared by the voxel depth/normal prepass and colour/volume pass.
struct EnergySource { energy:vec4<f32>, beam:vec4<f32>, weapon:vec4<f32>, control:vec4<f32> };
struct Params {
    world_from_clip: mat4x4<f32>, camera: vec4<f32>, origin: vec4<f32>, source: vec4<f32>,
    control: vec4<f32>, chemistry: vec4<f32>, charge: vec4<f32>, terrain: vec4<f32>,
    terrain_origin: vec4<f32>,
    clip_from_world: mat4x4<f32>, sun: vec4<f32>, sky: vec4<f32>,
    energy:vec4<f32>, beam:vec4<f32>, weapon:vec4<f32>, look:vec4<f32>, distant:vec4<f32>, distant_bounds:vec4<f32>,
    energy_count:vec4<f32>, energy_sources:array<EnergySource,24>,
};
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(5) var<storage, read> terrain: array<u32>;
@group(0) @binding(7) var<storage, read> bricks: array<u32>;
fn wrap_voxel(q:vec3<u32>,dims:vec3<u32>)->vec3<u32> {
    // The starting window needs no wrap. Avoid three dynamic integer
    // divisions at every ray step; moved windows retain the exact modulo.
    if all(q<dims) {return q;}
    return q%dims;
}
// The hierarchy is enabled only for aligned windows. Small test layouts
// remain valid without padding or changing their physical storage.
fn brick_space(q:vec3<i32>)->vec2<i32> {
    let dims=vec3<u32>(p.terrain.xyz);let lo=vec3<i32>(p.terrain_origin.xyz);
    let none=vec2<i32>(q.y+1,8);
    if any(dims==vec3(0u)) || any((dims&vec3(7u))!=vec3(0u)) ||
       any(lo<vec3(0)) || any((vec3<u32>(lo)&vec3(7u))!=vec3(0u)) {return none;}
    if any(q<lo) || any(q>=lo+vec3<i32>(dims)) {return none;}
    let wrapped=wrap_voxel(vec3<u32>(q),dims);
    if all((dims&vec3(31u))==vec3(0u)) && all((vec3<u32>(lo)&vec3(31u))==vec3(0u)) {
        let size=dims/32u;let b=wrapped/32u;let fine=dims/8u;
        let h=i32(floor(f32(q.y)/32.0))*32+i32(bricks[fine.x*fine.y*fine.z+b.x+size.x*(b.y+size.y*b.z)]);
        if q.y>=h {return vec2<i32>(h,32);}
    }
    let b=wrapped/8u;let size=dims/8u;
    return vec2<i32>(i32(floor(f32(q.y)/8.0))*8+i32(bricks[b.x+size.x*(b.y+size.y*b.z)]),8);
}
struct In { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
// The volume is a window on the world, not the whole of it (ADR-318):
// it holds `dims` voxels from `lo`, and its storage wraps, so a voxel
// lives at `q % dims` whatever the window's corner. Unsigned modulo
// throughout: a signed one is forbidden on our backend.
// Packed row and the material at q; tracing can reuse the same load to skip
// eight empty x cells even below a neighbouring tree's brick ceiling.
fn packed_material(q: vec3<i32>) -> vec2<u32> {
    let dims=vec3<i32>(p.terrain.xyz);
    let lo=vec3<i32>(p.terrain_origin.xyz);
    if any(q<lo) || any(q>=lo+dims) {return vec2(0u);}
    let m=wrap_voxel(vec3<u32>(q),vec3<u32>(dims));
    let i=m.x+u32(dims.x)*(m.y+u32(dims.y)*m.z);
    let word=terrain[i/8u];
    return vec2(word,(word>>((i&7u)*4u))&15u);
}
fn local_contains(pos:vec3<f32>)->bool {
    return all(pos>=p.terrain_origin.xyz) && all(pos<p.terrain_origin.xyz+p.terrain.xyz);
}
fn distant_word(q:vec2<u32>,span:u32)->u32 {
    return terrain[u32(p.distant_bounds.w)+(q.x+u32(p.distant.x)*q.y)*4u+span];
}
fn span_material(word:u32,y:f32)->u32 {
    return select((word>>16u)&15u,(word>>20u)&15u,y>=f32(word>>24u));
}
fn raw_material(q:vec3<i32>)->u32 {
    if local_contains(vec3<f32>(q)) {return packed_material(q).y;}
    if p.distant.w<0.5 || any(q<vec3(0)) || any(vec3<f32>(q)>=p.distant_bounds.xyz) {return 0u;}
    let cell=vec2<u32>(vec2<f32>(q.xz)/p.distant.z);
    for(var i=0u;i<4u;i++) {
        let word=distant_word(cell,i);
        if q.y>=i32(word&255u) && q.y<i32((word>>8u)&255u) {return span_material(word,f32(q.y));}
    }
    return 0u;
}
struct TerrainHit { t:f32, normal:vec3<f32>, material:u32 };
fn trace_local(ro:vec3<f32>,rd:vec3<f32>,limit:f32,shadow:bool)->TerrainHit {
    // A parallel axis has no next face. Adding epsilon to the direction
    // changed its sign near zero and made horizontal rays crawl backwards.
    let inv=1.0/select(vec3(1e-30),rd,rd!=vec3(0.0));
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
        let pos=ro+rd*(t+0.001);let q=vec3<i32>(floor(pos));
        let skip=brick_space(q);let ceiling=skip.x;
        var packed=vec2(0u);
        if q.y<ceiling {packed=packed_material(q);}
        let m=packed.y;
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
        if q.y>=ceiling {
            let edge=f32(skip.y);low=floor(low/edge)*edge;high=low+vec3(edge);
            low.y=f32(ceiling);
        } else if packed.x==0u && (u32(p.terrain.x)&7u)==0u {
            low.x=floor(low.x/8.0)*8.0;high.x=low.x+8.0;
        }
        let face=select(low,high,rd>vec3(0.0));
        let dist=select((face-pos)*inv,vec3(1e30),rd==vec3(0.0));
        let d=max(0.002,min(dist.x,min(dist.y,dist.z)));
        normal=vec3(0.0);
        if dist.x<=dist.y && dist.x<=dist.z {normal.x=-sign(rd.x);}
        else if dist.y<=dist.z {normal.y=-sign(rd.y);} else {normal.z=-sign(rd.z);}
        t+=d+0.002;
    }
    return TerrainHit(limit,normal,0u);
}

// Column DDA: exact vertical faces, bounded horizontal sampling. Local and
// distant domains are disjoint, including for shadows, so they cannot fight
// over depth. They share materials, lighting, prepass and postprocessing.
fn trace_distant(ro:vec3<f32>,rd:vec3<f32>,limit:f32)->TerrainHit {
    let miss=TerrainHit(limit,vec3(0.0,1.0,0.0),0u);
    if p.distant.w<0.5 {return miss;}
    let inv=1.0/select(vec3(1e-30),rd,rd!=vec3(0.0));
    let a=min(-ro*inv,(p.distant_bounds.xyz-ro)*inv);
    let b=max(-ro*inv,(p.distant_bounds.xyz-ro)*inv);
    var t=max(0.0,max(a.x,max(a.y,a.z)));
    let end=min(limit,min(b.x,min(b.y,b.z)));
    let edge=p.distant.z;
    for(var j=0;j<2048;j++) {
        if t>=end {break;}
        let pos=ro+rd*(t+0.004);
        if local_contains(pos) {
            let face=select(p.terrain_origin.xyz,p.terrain_origin.xyz+p.terrain.xyz,rd>vec3(0.0));
            let exits=select((face-ro)*inv,vec3(1e30),rd==vec3(0.0));
            t=min(exits.x,min(exits.y,exits.z))+0.005;
            continue;
        }
        let q=vec2<i32>(floor(pos.xz/edge));
        if any(q<vec2(0)) || any(q>=vec2<i32>(p.distant.xy)) {break;}
        let lo=vec2<f32>(q)*edge;let hi=min(lo+vec2(edge),p.distant_bounds.xz);
        let aa=(lo-ro.xz)*inv.xz;let bb=(hi-ro.xz)*inv.xz;
        let near=min(aa,bb);let far=max(aa,bb);
        let column_end=min(far.x,far.y);
        var best=miss;
        for(var k=0u;k<4u;k++) {
            let word=distant_word(vec2<u32>(q),k);
            if word==0u {continue;}
            let ys=(vec2(f32(word&255u),f32((word>>8u)&255u))-vec2(ro.y))*inv.y;
            let begin=max(near.x,max(near.y,min(ys.x,ys.y)));
            let finish=min(column_end,max(ys.x,ys.y));
            let hit=max(t,begin);
            if hit>=min(end,best.t) || hit>finish {continue;}
            let point=ro+rd*(hit+0.005);
            if local_contains(point) {continue;}
            var normal=vec3(0.0,-sign(rd.y),0.0);
            if near.x>=near.y && near.x>=min(ys.x,ys.y) {normal=vec3(-sign(rd.x),0.0,0.0);}
            else if near.y>=min(ys.x,ys.y) {normal=vec3(0.0,0.0,-sign(rd.z));}
            best=TerrainHit(hit,normal,span_material(word,point.y));
        }
        if best.material>0u {return best;}
        t=column_end+0.005;
    }
    return miss;
}
fn trace_terrain(ro:vec3<f32>,rd:vec3<f32>,limit:f32,shadow:bool)->TerrainHit {
    let close=trace_local(ro,rd,limit,shadow);
    let distant=trace_distant(ro,rd,close.t);
    if distant.material>0u {return distant;}
    return close;
}
