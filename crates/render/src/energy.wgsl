// Massless presentation of the authoritative CPU projectile. Turbulence never
// steers a shot or causes damage. This module also validates on its own.
fn energy_hash(q:vec3<i32>)->f32 {
    var h=u32(q.x)*73856093u^u32(q.y)*19349663u^u32(q.z)*83492791u;
    h=(h^(h>>13u))*1274126177u;return f32(h&65535u)/65535.0;
}
fn energy_noise(v:vec3<f32>)->f32 {
    let q=vec3<i32>(floor(v));let a=fract(v);let f=a*a*(vec3(3.0)-2.0*a);
    return mix(mix(mix(energy_hash(q),energy_hash(q+vec3(1,0,0)),f.x),mix(energy_hash(q+vec3(0,1,0)),energy_hash(q+vec3(1,1,0)),f.x),f.y),
        mix(mix(energy_hash(q+vec3(0,0,1)),energy_hash(q+vec3(1,0,1)),f.x),mix(energy_hash(q+vec3(0,1,1)),energy_hash(q+vec3(1,1,1)),f.x),f.y),f.z);
}
fn energy_segment(ro:vec3<f32>,rd:vec3<f32>,a:vec3<f32>,b:vec3<f32>,limit:f32)->f32 {
    let v=b-a;let w=ro-a;let vv=max(dot(v,v),0.0001);let rv=dot(rd,v);
    let u=clamp((dot(v,w)-rv*dot(rd,w))/max(vv-rv*rv,0.0001),0.0,1.0);
    let t=clamp(dot(a+v*u-ro,rd),0.0,limit);
    return length(ro+rd*t-a-v*u);
}
fn energy_path(ro:vec3<f32>,rd:vec3<f32>,a:vec3<f32>,b:vec3<f32>,limit:f32,time:f32)->vec3<f32> {
    let d=energy_segment(ro,rd,a,b,limit);
    if d>8.0 {return vec3(0.0);}
    var glow=vec3(0.05,0.10,0.22)*exp(-d*d*0.32);
    var last=a;
    let direction=normalize(b-a+vec3(0.0001));
    let side=normalize(cross(direction,vec3(0.13,1.0,0.31)));
    let up=cross(side,direction);
    for(var j=1;j<=16;j++) {
        let u=f32(j)/16.0;let phase=f32(j)*5.13+floor(time*32.0)*1.73;
        let spread=sin(u*3.14159)*(1.1+0.9*sin(phase*2.17));
        let next=mix(a,b,u)+(side*sin(phase)+up*cos(phase*1.39))*spread;
        let s=energy_segment(ro,rd,last,next,limit);
        let flicker=0.4+0.6*abs(sin(phase*0.71));
        glow+=(vec3(1.8,2.7,4.5)*exp(-s*s*45.0)+vec3(0.10,0.28,0.8)*exp(-s*s*2.5))*flicker;
        if (j&3)==0 {
            let branch=next+(side*cos(phase*2.3)+up*sin(phase*3.1))*3.4-direction*2.0;
            let bs=energy_segment(ro,rd,next,branch,limit);
            glow+=vec3(0.4,0.8,1.8)*exp(-bs*bs*55.0)*flicker;
        }
        last=next;
    }
    return glow;
}
fn energy_ball(ro:vec3<f32>,rd:vec3<f32>,centre:vec3<f32>,radius:f32,limit:f32,time:f32)->vec3<f32> {
    let offset=ro-centre;let along=dot(offset,rd);let outer=radius*2.7;
    let h=along*along-dot(offset,offset)+outer*outer;
    if h<0.0 {return vec3(0.0);}
    let start=max(0.0,-along-sqrt(h));let end=min(limit,-along+sqrt(h));
    if end<=start {return vec3(0.0);}
    let dt=(end-start)/40.0;var result=vec3(0.0);
    let drift=vec3(time*1.7,-time*3.8,time*0.9);
    for(var i=0;i<40;i++) {
        let v=(ro+rd*(start+(f32(i)+0.5)*dt)-centre)/radius;
        let warp=vec3(energy_noise(v*2.3+drift),energy_noise(v*2.1-drift+17.0),energy_noise(v*2.7+drift.yzx+31.0))-vec3(0.5);
        let q=v+warp*0.85;let r=length(q);
        let n=energy_noise(q*4.9+drift*2.0);
        let pockets=energy_noise(q*3.2-drift.yzx);
        let core=exp(-dot(q*vec3(0.78,1.2,1.0),q*vec3(0.78,1.2,1.0))*9.0)*(0.5+pockets);
        // Broken advected filaments, without orbital rings.
        let skin=exp(-(r-1.08)*(r-1.08)*10.0);
        let filaments=exp(-abs(n-0.52)*95.0)*smoothstep(0.35,0.68,pockets)*skin;
        let haze=exp(-r*r*1.5)*0.027*(0.3+n);
        result+=(vec3(15.0,18.0,22.0)*core+vec3(0.7,3.5,18.0)*filaments+vec3(0.12,0.4,2.0)*haze)*dt/radius;
    }
    return result;
}
// Annihilation bolt (ADR-285): the picture bends around the bolt and, for
// a moment, around the point it struck. Presentation only: the offset is
// applied to the already-drawn scene before any light is added.
fn anni_warp(uv:vec2<f32>)->vec2<f32> {
    var centre=vec3(0.0);var strength=0.0;var radius=0.0;
    if p.energy.w>0.0 {
        centre=p.energy.xyz;strength=0.05;radius=0.12;
    } else if p.control.z>0.5 {
        let age=p.control.y/60.0;
        if age<0.9 {centre=p.energy.xyz;strength=0.10*(1.0-age/0.9);radius=0.12+age*0.25;}
    }
    if strength<=0.0 {return vec2(0.0);}
    let clip=p.clip_from_world*vec4(centre,1.0);
    if clip.w<=0.0 {return vec2(0.0);}
    let cuv=vec2(clip.x/clip.w*0.5+0.5,0.5-clip.y/clip.w*0.5);
    let d=uv-cuv;let r=length(d);
    if r>=radius || r<1e-4 {return vec2(0.0);}
    let ring=sin(r/radius*9.42-p.weapon.y*30.0)*0.5+0.5;
    return d/r*strength*(1.0-r/radius)*(0.6+0.4*ring);
}
fn energy_effect(ro:vec3<f32>,rd:vec3<f32>,limit:f32)->vec3<f32> {
    if p.weapon.x<0.5 {return vec3(0.0);}
    var colour=vec3(0.0);let time=p.weapon.y;
    if p.beam.w>0.01 {
        let charge=p.beam.w;
        colour+=energy_ball(ro,rd,p.beam.xyz,0.6+charge*2.5,limit,time)*charge;
        for(var j=0;j<3;j++) {
            let a=f32(j)*2.094+time*1.8;
            let feed=p.beam.xyz+vec3(-7.0,cos(a)*3.0,sin(a)*3.0);
            colour+=energy_path(ro,rd,feed,p.beam.xyz,limit,time+f32(j))*charge*0.28;
        }
    }
    if p.energy.w>0.0 {
        colour+=energy_ball(ro,rd,p.energy.xyz,p.energy.w*1.7,limit,time);
        for(var j=0;j<3;j++) {
            let phase=f32(j)*2.4+time*4.3;
            let tuft=p.energy.xyz+vec3(-4.0-2.0*sin(phase*3.1),sin(phase)*6.5,cos(phase*1.7)*5.0);
            colour+=energy_path(ro,rd,tuft,p.energy.xyz,limit,time+f32(j))*0.55;
        }
        // Finite luminous wake instead of a permanent muzzle-to-target beam.
        let direction=normalize(p.energy.xyz-p.beam.xyz);
        let start=p.energy.xyz-direction*min(80.0,length(p.energy.xyz-p.beam.xyz));
        colour+=energy_path(ro,rd,start,p.energy.xyz,limit,time)*0.75;
    }
    if p.control.z>0.5 {
        let age=p.control.y/60.0;
        if age<0.12 {
            let start=mix(p.beam.xyz,p.energy.xyz,0.55);
            colour+=energy_path(ro,rd,start,p.energy.xyz,limit,time)*(1.0-age/0.12)*0.5;
        }
        if age<0.7 {
            let centre=p.energy.xyz;
            for(var j=0;j<11;j++) {
                let a=f32(j)*2.39996+0.2*sin(f32(j)*9.7);
                let spread=11.0+age*54.0+7.0*sin(f32(j)*7.13);
                let end=centre+vec3(cos(a),0.25+0.6*abs(sin(a*3.0)),sin(a))*spread;
                colour+=energy_path(ro,rd,centre,end,limit,time+f32(j))*exp(-age*7.0)*0.75;
            }
        }
    }
    return colour;
}
