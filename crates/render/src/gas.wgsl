// Visual gas. World units and simulation cells are independent of display voxels.
// Double buffering: every invocation reads the same previous state.
struct EnergySource { energy:vec4<f32>, beam:vec4<f32>, weapon:vec4<f32>, control:vec4<f32> };
struct Params {
    world_from_clip: mat4x4<f32>,
    camera: vec4<f32>,
    origin: vec4<f32>, // xyz world origin, w cell edge
    source: vec4<f32>, // xyz world impact, w injection radius
    control: vec4<f32>, // dt, step, opacity, additive injection
    chemistry: vec4<f32>, // reaction, cooling, buoyancy, temperature/1000
    charge: vec4<f32>, // fuel, pressure, display voxel size, surface-fire marker
    terrain: vec4<f32>,
    terrain_origin: vec4<f32>,
    clip_from_world: mat4x4<f32>, sun: vec4<f32>, sky: vec4<f32>,
    energy:vec4<f32>, beam:vec4<f32>, weapon:vec4<f32>,
    look: vec4<f32>, distant:vec4<f32>, distant_bounds:vec4<f32>,
    energy_count:vec4<f32>, energy_sources:array<EnergySource,24>,
    terrain_wrap:vec4<f32>,
    section:vec4<f32>,
};
struct Cell { flow: vec4<f32>, matter: vec4<f32> }; // v.xyz, pressure; excess kelvin/1000, fuel, O2, soot
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> old: array<Cell>;
@group(0) @binding(2) var<storage, read_write> next: array<Cell>;
@group(0) @binding(3) var<storage, read> solid: array<u32>;
const N: i32 = 48;
fn index(v: vec3<i32>) -> u32 { let q=clamp(v,vec3(0),vec3(N-1)); return u32(q.x+N*(q.y+N*q.z)); }
fn wall(v: vec3<i32>) -> bool { return solid[index(v)] != 0u; }
fn air() -> Cell { return Cell(vec4(0.0),vec4(0.0,0.0,1.0,0.0)); }
fn cell(v: vec3<i32>) -> Cell {
    if any(v<vec3(0)) || any(v>=vec3(N)) || wall(v) { return air(); }
    return old[index(v)];
}
fn sample_field(pos: vec3<f32>) -> Cell {
    let q=pos-vec3(0.5);let b=vec3<i32>(floor(q));let f=fract(q);
    var v=vec4(0.0); var m=vec4(0.0);
    for(var z=0;z<2;z++){for(var y=0;y<2;y++){for(var x=0;x<2;x++){
        let t=vec3<i32>(x,y,z);let s=select(vec3(1.0)-f,f,t==vec3(1));
        let w=s.x*s.y*s.z;let c=cell(b+t);v+=c.flow*w;m+=c.matter*w;
    }}}
    return Cell(v,m);
}
fn rng(q: vec3<f32>) -> f32 { return fract(sin(dot(q,vec3(127.1,311.7,74.7)))*43758.5453); }
fn source_visible(q:vec3<i32>)->bool {
    let a=(p.source.xyz-p.origin.xyz)/p.origin.w;
    let b=vec3<f32>(q)+vec3(0.5);
    let delta=abs(b-a);let count=min(192,i32(ceil(max(delta.x,max(delta.y,delta.z))*2.0))+1);
    for(var j=0;j<=192;j++) {
        if j>count {break;}
        if wall(vec3<i32>(floor(mix(a,b,f32(j)/f32(count))))) {return false;}
    }
    return true;
}
@compute @workgroup_size(4,4,4)
fn init(@builtin(global_invocation_id) gid: vec3<u32>) {
    let q=vec3<i32>(gid); if any(q>=vec3(N)) {return;}
    let i=index(q); var c=air();
    let offset=(p.origin.xyz+(vec3<f32>(gid)+vec3(0.5))*p.origin.w-p.source.xyz)/p.source.w;
    let r=length(offset);
    let visible=r<1.5 && p.control.z>0.5 && !wall(q) && source_visible(q);
    // Perturb injection, not the subsequent trajectory. The flow creates the shape.
    let lobes=0.83+0.28*sin(offset.x*7.0)*sin(offset.z*8.0)+0.24*sin(offset.y*9.0);
    if r<lobes && visible {
        let w=pow(max(0.0,1.0-r/lobes),0.5);
        let pockets=0.72+0.28*sin(offset.x*8.0+offset.y*5.0)*cos(offset.z*9.0);
        c.flow=vec4(normalize(offset+vec3(0.001))*w*8.0+vec3(0.0,w*2.5,0.0),p.charge.y*w);
        c.matter=vec4((p.chemistry.w-0.3)*pow(w,0.5)*pockets,p.charge.x*w,0.28+0.6*r,(0.9+0.65*(1.0-pockets))*w);
    }
    // Cold ground ejecta enter the same flow with an outward impulse.
    if visible && offset.y < -0.24 && offset.y > -0.9 && r>0.65 {
        let w=(1.0-smoothstep(0.95,1.5,r))*(0.5+0.5*rng(vec3<f32>(gid)));
        c.matter.w+=w*0.9;
        c.flow=vec4(c.flow.xyz+normalize(vec3(offset.x,0.08,offset.z)+vec3(0.001))*w*8.0,c.flow.w);
    }
    if p.control.w>0.5 && !wall(q) {
        // A new source adds heat, fuel, soot and impulse to surviving gas.
        // Oxygen is consumed, never reset to fresh air by another explosion.
        let previous=old[i];
        c.flow=vec4(clamp(previous.flow.xyz+c.flow.xyz,vec3(-12.0),vec3(12.0)),clamp(previous.flow.w+c.flow.w,-1.0,4.0));
        c.matter=vec4(min(4.0,previous.matter.x+c.matter.x),previous.matter.y+c.matter.y,
            min(previous.matter.z,c.matter.z),previous.matter.w+c.matter.w);
    }
    next[i]=c;
}
@compute @workgroup_size(4,4,4)
fn step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let q=vec3<i32>(gid); if any(q>=vec3(N)) {return;}
    let i=index(q);if wall(q) {next[i]=air();return;}
    let c=old[i];let dt=p.control.x;
    var back=vec3<f32>(gid)+vec3(0.5)-c.flow.xyz*dt;
    // No transport through a wall even if a fast characteristic crosses it.
    for(var j=1;j<=4;j++) {
        let v=mix(vec3<f32>(gid)+vec3(0.5),back,f32(j)*0.25);
        if wall(vec3<i32>(floor(v))) {back=vec3<f32>(gid)+vec3(0.5);break;}
    }
    let a=sample_field(back); var m=a.matter;
    let xp=cell(q+vec3(1,0,0));let xm=cell(q-vec3(1,0,0));
    let yp=cell(q+vec3(0,1,0));let ym=cell(q-vec3(0,1,0));
    let zp=cell(q+vec3(0,0,1));let zm=cell(q-vec3(0,0,1));
    // Reflect pressure at solid boundaries. A pressure sink here would pull
    // the hot cloud into the barrier instead of redirecting it.
    let pxp=select(xp.flow.w,c.flow.w,wall(q+vec3(1,0,0)));
    let pxm=select(xm.flow.w,c.flow.w,wall(q-vec3(1,0,0)));
    let pyp=select(yp.flow.w,c.flow.w,wall(q+vec3(0,1,0)));
    let pym=select(ym.flow.w,c.flow.w,wall(q-vec3(0,1,0)));
    let pzp=select(zp.flow.w,c.flow.w,wall(q+vec3(0,0,1)));
    let pzm=select(zm.flow.w,c.flow.w,wall(q-vec3(0,0,1)));
    let grad=vec3(pxp-pxm,pyp-pym,pzp-pzm)*0.5;
    let div=(xp.flow.x-xm.flow.x+yp.flow.y-ym.flow.y+zp.flow.z-zm.flow.z)*0.5;
    // Small explicit mixing, followed by a fuel/O2 limited reaction.
    let mean=(xp.matter+xm.matter+yp.matter+ym.matter+zp.matter+zm.matter)/6.0;
    m=mix(m,mean,dt*0.8);
    let burn=min(m.y,m.z)*min(1.0,dt*p.chemistry.x*smoothstep(0.35,0.9,m.x));
    m.y=max(0.0,m.y-burn);m.z=max(0.0,m.z-burn);
    m.x=min(4.0,m.x+burn*2.2);m.w+=burn*(0.65+0.7*(1.0-m.z));
    m.x*=exp(-dt*p.chemistry.y);m.w*=exp(-dt*select(0.16,0.4,p.weapon.x>0.5));
    m.z=min(1.0,m.z+dt*0.035);
    let pos=vec3<f32>(gid)*0.63;
    // Analytic divergence-free stirring. Driven by heat, it dies with the plume.
    let swirl=vec3(sin(pos.y)*cos(pos.z),sin(pos.z)*cos(pos.x),sin(pos.x)*cos(pos.y));
    let curl=vec3(yp.flow.z-ym.flow.z-zp.flow.y+zm.flow.y,zp.flow.x-zm.flow.x-xp.flow.z+xm.flow.z,xp.flow.y-xm.flow.y-yp.flow.x+ym.flow.x)*0.5;
    let thermal_grad=vec3(xp.matter.x-xm.matter.x,yp.matter.x-ym.matter.x,zp.matter.x-zm.matter.x);
    var v=a.flow.xyz-grad*dt*28.0+vec3(0.18,m.x*p.chemistry.z,0.0)*dt;
    v+=(swirl*m.x*5.5+cross(thermal_grad,curl)*1.0)*dt;
    v*=exp(-dt*0.08);
    v=clamp(v,vec3(-12.0),vec3(12.0)); // CFL < 0.35 at 60 Hz, acoustic speed sqrt(28*2)
    if (v.x>0.0 && wall(q+vec3(1,0,0))) || (v.x<0.0 && wall(q-vec3(1,0,0))) {v.x=0.0;}
    if (v.y>0.0 && wall(q+vec3(0,1,0))) || (v.y<0.0 && wall(q-vec3(0,1,0))) {v.y=0.0;}
    if (v.z>0.0 && wall(q+vec3(0,0,1))) || (v.z<0.0 && wall(q-vec3(0,0,1))) {v.z=0.0;}
    let pressure=clamp((a.flow.w-div*dt*2.0+burn*1.6)*exp(-dt*1.1),-1.0,4.0);
    next[i]=Cell(vec4(v,pressure),max(m,vec4(0.0)));
}
