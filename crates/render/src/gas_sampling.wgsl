// Exact per-display-cell samples shared by compute and fragment shading.
struct Cell { flow:vec4<f32>, matter:vec4<f32> };
@group(0) @binding(4) var<storage,read> gas:array<Cell>;
const N:i32=48;
fn gas_may_show(m:vec4<f32>)->bool {
    return (m.w*1.1+m.y*0.22)*1.55>0.12;
}
fn mat(q:vec3<i32>)->vec4<f32> {
    if any(q<vec3(0)) || any(q>=vec3(N)) {return vec4(0.0);}
    return gas[u32(q.x+N*(q.y+N*q.z))].matter;
}
fn field(v:vec3<f32>)->vec4<f32> {
    // Outside the trilinear support every one of the eight taps is zero.
    // Reject once, before computing and bounds-checking all eight addresses.
    if any(v<=vec3(-0.5)) || any(v>=vec3(f32(N)+0.5)) {return vec4(0.0);}
    let u=v-vec3(0.5);let b=vec3<i32>(floor(u));let f=fract(u);
    return mix(mix(mix(mat(b),mat(b+vec3(1,0,0)),f.x),mix(mat(b+vec3(0,1,0)),mat(b+vec3(1,1,0)),f.x),f.y),
        mix(mix(mat(b+vec3(0,0,1)),mat(b+vec3(1,0,1)),f.x),mix(mat(b+vec3(0,1,1)),mat(b+vec3(1,1,1)),f.x),f.y),f.z);
}
fn hash(q:vec3<i32>)->f32 {
    var h=u32(q.x)*73856093u^u32(q.y)*19349663u^u32(q.z)*83492791u;
    h=(h^(h>>13u))*1274126177u;return f32(h&65535u)/65535.0;
}
fn noise(v:vec3<f32>)->f32 {
    let q=vec3<i32>(floor(v));let a=fract(v);let f=a*a*(vec3(3.0)-2.0*a);
    return mix(mix(mix(hash(q),hash(q+vec3(1,0,0)),f.x),mix(hash(q+vec3(0,1,0)),hash(q+vec3(1,1,0)),f.x),f.y),
        mix(mix(hash(q+vec3(0,0,1)),hash(q+vec3(1,0,1)),f.x),mix(hash(q+vec3(0,1,1)),hash(q+vec3(1,1,1)),f.x),f.y),f.z);
}

struct DisplayCell { matter:vec4<f32>, detail:vec4<f32> };
fn display_side()->u32 {return u32(ceil(f32(N)*p.origin.w/p.charge.z));}
fn display_cell(vi:vec3<i32>)->DisplayCell {
    // Some groups of eight display cells share one coarse gas sample. Their
    // silhouette reads as a large smoke cube while the other cells retain
    // the finer edge. The transport grid and its mass are untouched.
    let cluster=vi/2;
    let coarse=p.control.y>21.0 && all(vi>=vec3(0)) && hash(cluster)>0.77;
    let sample_cell=select(vec3<f32>(vi)+vec3(0.5),vec3<f32>(cluster*2)+vec3(1.0),coarse);
    let centre=sample_cell*p.charge.z;
    let gas_pos=centre/p.origin.w;let m=field(gas_pos);
    var detail=vec4(0.0);
    if gas_may_show(m) {
        detail.x=noise(gas_pos*1.15-vec3(0.0,p.weapon.y*0.9,0.0));
        detail.y=hash(select(vi,cluster,coarse));
        detail.w=(m.w*1.1+m.y*0.22)*mix(0.38,1.55,detail.x);
        // Reuse the prepared noise to open irregular gaps in an older,
        // high-charge shell cloud. Evaluate this once per display voxel.
        if p.weapon.x<=0.5 && p.source.w>18.0 {
            let age=p.control.y/60.0;
            let late=smoothstep(0.45,1.50,age);
            detail.w*=mix(1.0,0.58*smoothstep(0.32,0.82,detail.x),late*0.8);
        }
        if detail.w>0.12+detail.y*0.15 {
            let sun=normalize(p.sun.xyz);
            detail.z=exp(-(field(gas_pos+sun*1.5).w+field(gas_pos+sun*3.5).w)*0.64);
        }
    }
    return DisplayCell(m,detail);
}
