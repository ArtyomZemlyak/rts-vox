// Params, terrain DDA and gas sampling helpers are prepended by the host.
@group(0) @binding(8) var<storage,read> display:array<DisplayCell>;
@group(0) @binding(0) var scene:texture_2d<f32>;
@group(0) @binding(1) var scene_sampler:sampler;
@group(0) @binding(2) var depth:texture_depth_2d;
@group(0) @binding(6) var normals:texture_2d<f32>;
// Maximum omitted direct-light contribution per HDR channel and region.
// Cloud opacity, cloud shadows, soot and hot fissures are not culled.
const LIGHT_EPSILON:f32=0.00001;
fn hot(t:f32)->vec3<f32> {
    if p.weapon.x>1.5 {
        // The annihilation bolt (ADR-289): its dust never glows white.
        return mix(mix(vec3(0.05,0.03,0.09),vec3(0.55,0.25,1.1),smoothstep(0.9,1.7,t)),
            vec3(2.4,1.3,4.2),smoothstep(1.8,2.9,t));
    }
    if p.weapon.x>0.5 {
        // A narrow silver-blue core sits inside a much larger, darker cloud.
        // Broad white radiance erased the separate cubes in the impact.
        return mix(mix(vec3(0.045,0.09,0.17),vec3(0.20,0.55,1.8),smoothstep(0.9,1.8,t)),
            vec3(0.45,1.65,5.2),smoothstep(2.2,3.2,t));
    }
    let red=vec3(1.1,0.035,0.002);let orange=vec3(3.6,0.68,0.015);
    let gold=vec3(5.0,2.1,0.12);let white=vec3(7.0,5.0,1.5);
    return mix(mix(red,orange,smoothstep(0.7,1.4,t)),mix(gold,white,smoothstep(2.5,3.1,t)),smoothstep(1.6,2.4,t));
}
fn surface_colour(code:u32,pos:vec3<f32>,normal:vec3<f32>)->vec3<f32> {
    let steel_damage=code>=9u && code<=11u;
    let concrete_damage=code>=12u && code<=14u;
    let m=select(select(code,3u,steel_damage),2u,concrete_damage);
    let grain=hash(vec3<i32>(floor(pos-normal*0.04)));
    let variation=noise(pos*0.09)+0.45*noise(pos*0.027);
    var c=vec3(0.14,0.082,0.038);
    if m==2u {c=vec3(0.22,0.23,0.21);}
    if m==3u {
        c=vec3(0.105,0.14,0.13);
        let panel=fract(pos.zy/vec2(12.0,10.0));
        if min(panel.x,panel.y)<0.09 {c*=0.48;}
        c=mix(c,vec3(0.18,0.095,0.04),smoothstep(0.7,0.95,variation)*0.65);
    }
    if m==4u {c=mix(vec3(0.032,0.063,0.018),vec3(0.13,0.19,0.035),grain*0.65+variation*0.22);}
    if m==5u {c=vec3(0.11,0.065,0.025);}
    if m==6u {c=mix(vec3(0.12,0.135,0.125),vec3(0.26,0.23,0.18),variation*0.6);}
    if m==7u {c=mix(vec3(0.045,0.093,0.027),vec3(0.20,0.22,0.070),smoothstep(0.40,1.05,variation));}
    if m==8u {
        c=mix(vec3(0.19,0.125,0.065),vec3(0.32,0.235,0.115),variation*0.5);
        let rut=min(abs(pos.z-324.5),abs(pos.z-347.5));
        if rut<3.0 && pos.x<158.0 {c*=0.51+0.18*step(0.5,fract(pos.x*0.58));}
    }
    if steel_damage {
        let severity=f32(code-8u)/3.0;
        let pits=noise(pos*0.22)+noise(pos*1.2)*0.14;
        let exposed=vec3(0.19,0.21,0.22)*(0.8+grain*0.2);
        let oxide=vec3(0.043,0.038,0.032);
        c=mix(exposed,oxide,smoothstep(0.32,0.88,pits+severity*0.2));
        let scratch=pow(abs(sin(pos.y*5.7+noise(pos*0.25)*14.0)),24.0);
        c+=vec3(0.022,0.023,0.02)*scratch*(1.0-severity*0.45);
    }
    if concrete_damage { c=mix(vec3(0.29,0.265,0.22),vec3(0.065,0.062,0.053),grain*0.45+f32(code-11u)*0.13); }
    c*=0.70+grain*0.38+variation*0.14;
    return c*select(0.73,1.0,normal.y>0.5);
}
fn gas_shadow(pos:vec3<f32>,sun:vec3<f32>)->f32 {
    if p.control.z<0.001 {return 1.0;}
    let first=(pos+sun*9.0-p.origin.xyz)/p.origin.w;
    let last=(pos+sun*63.0-p.origin.xyz)/p.origin.w;
    // All seven taps lie on this segment. An axis separating the complete
    // segment from the field proves the shadow is exactly empty.
    if any(max(first,last)<=vec3(-0.5)) || any(min(first,last)>=vec3(f32(N)+0.5)) {return 1.0;}
    var density=0.0;
    for(var j=1;j<=7;j++) {density+=field((pos+sun*f32(j)*9.0-p.origin.xyz)/p.origin.w).w;}
    return exp(-density*0.46);
}
// Opaque colour is written before the mesh/sky passes. This entry point must
// never read `scene`: the attachment is still being built, not postprocessed.
@fragment fn terrain_fragment(input:In)->@location(0) vec4<f32> {
    let d=textureLoad(depth,vec2<i32>(input.position.xy),0);
    if d<=0.0 {discard;}
    let ndc=vec2(input.uv.x*2.0-1.0,1.0-input.uv.y*2.0);
    let near=p.world_from_clip*vec4(ndc,1.0,1.0);
    let opaque=p.world_from_clip*vec4(ndc,d,1.0);
    let pos=opaque.xyz/opaque.w;
    let normal=textureLoad(normals,vec2<i32>(input.position.xy),0).xyz*2.0-vec3(1.0);
    let m=raw_material(vec3<i32>(floor(pos-normal*0.08)));
    if m==0u || m==15u {discard;}
    let sun=normalize(p.sun.xyz);
    let surface_offset=select(0.12,0.45,m>=9u && m<=11u);
    let shadow=trace_terrain(pos+normal*surface_offset,sun,240.0,true).material>0u;
    let diffuse=max(0.0,dot(normal,sun))*select(1.0,0.025,shadow);
    let c=surface_colour(m,pos,normal);
    let ambient=vec3(0.26,0.34,0.44)*(0.4+0.6*max(0.0,normal.y));
    var lit=c*(ambient+vec3(1.0,0.81,0.54)*diffuse*p.sun.w);
    if p.weapon.x>0.5 && (p.energy.w>0.0 || p.beam.w>0.0) {
        let centre=select(p.beam.xyz,p.energy.xyz,p.energy.w>0.0);
        let delta=centre-pos;
        let strength=select(p.beam.w*p.beam.w,1.0,p.energy.w>0.0);
        let visible=trace_terrain(pos+normal*surface_offset,normalize(delta),max(0.0,length(delta)-3.0),true).material==0u;
        lit+=vec3(0.65,1.0,1.9)*strength*125.0/(60.0+dot(delta,delta))
            *max(0.06,dot(normal,normalize(delta)))*select(0.04,1.0,visible);
    }
    let fog=1.0-exp(-length(pos-near.xyz/near.w)*p.sky.w);
    return vec4(mix(lit,p.sky.rgb,fog),1.0);
}
@fragment fn fragment(input:In)->@location(0) vec4<f32> {
    var bg=textureSampleLevel(scene,scene_sampler,input.uv,0.0);
    if p.weapon.x>1.5 && p.control.w<0.5 {
        // The annihilation bolt refracts what is behind it, each channel a
        // little differently (ADR-285).
        let warp=anni_warp(input.uv);
        if dot(warp,warp)>0.0 {
            let lo=vec2(0.0);let hi=vec2(1.0);
            bg=vec4(
                textureSampleLevel(scene,scene_sampler,clamp(input.uv-warp*1.15,lo,hi),0.0).r,
                textureSampleLevel(scene,scene_sampler,clamp(input.uv-warp,lo,hi),0.0).g,
                textureSampleLevel(scene,scene_sampler,clamp(input.uv-warp*0.85,lo,hi),0.0).b,
                bg.a);
        }
    }
    let ndc=vec2(input.uv.x*2.0-1.0,1.0-input.uv.y*2.0);
    let near=p.world_from_clip*vec4(ndc,1.0,1.0);
    let far=p.world_from_clip*vec4(ndc,0.00001,1.0);
    let ro=near.xyz/near.w;let rd=normalize(far.xyz/far.w-ro);
    let d=textureLoad(depth,vec2<i32>(input.position.xy),0);
    let opaque=p.world_from_clip*vec4(ndc,max(d,0.00001),1.0);
    var maxdist=length(opaque.xyz/opaque.w-ro);
    // Reuse the voxel prepass instead of tracing the camera ray twice.
    let world_pos=opaque.xyz/opaque.w;
    let world_normal=textureLoad(normals,vec2<i32>(input.position.xy),0).xyz*2.0-vec3(1.0);
    let m=raw_material(vec3<i32>(floor(world_pos-world_normal*0.08)));
    let ground=TerrainHit(maxdist,world_normal,m);
    let sun=normalize(p.sun.xyz);
    if p.control.w>0.5 && d>0.0 {
        // Every surviving region contributes a soft cloud shadow. Retiring
        // regions fade their shadow together with their visible density.
        bg=vec4(bg.rgb*mix(1.0,0.4+0.6*gas_shadow(world_pos,sun),p.control.z),bg.a);
    }
    if p.control.w>0.5 && p.control.z>0.001 && d>0.0 && m>0u && m!=15u {
        let pos=world_pos;
        let surface_offset=0.45;
        var lit=vec3(0.0);
            let age=p.control.y/60.0;
            let delta=p.source.xyz-pos;let r=length(delta.xz);
            let soot=(1.0-smoothstep(16.0,37.0,r))*(0.62+0.30*noise(pos*0.31))*smoothstep(0.0,0.5,age);
            var flash=select(vec3(5.0,0.75,0.065),vec3(1.2,1.65,2.7),p.weapon.x>0.5);
            let decay=select(1.15,4.0,p.weapon.x>0.5);
            var reach=950.0;
            if p.weapon.x>1.5 {
                // The annihilation strike (ADR-289) gives off a faint violet
                // glimmer and swallows the light around the point it struck
                // while its dark sphere lasts.
                flash=vec3(0.9,0.35,1.7);reach=140.0;
                let grow=sin(min(age/0.55,1.0)*3.14159)*step(age,0.55);
                let sphere=3.0+11.0*grow;
                let swallow=(1.0-smoothstep(0.6*sphere,sphere,length(delta)))*grow;
                bg=vec4(bg.rgb*(1.0-0.97*swallow),bg.a);
            }
            let direct=flash*exp(-age*decay)*reach/(110.0+dot(delta,delta))
                *max(0.05,dot(ground.normal,normalize(delta)));
            let needs_soot=ground.material!=4u && soot>0.0;
            let needs_light=any(direct*p.control.z*exp(-ground.t*p.sky.w)>vec3(LIGHT_EPSILON));
            if needs_soot || needs_light {
                let visible=trace_terrain(pos+ground.normal*surface_offset,normalize(delta),max(0.0,length(delta)-5.0),true).material==0u;
                // Hot shell residue stains the surface without turning the
                // entire crater black; a fuel-free energy bolt leaves dust
                // rather than the same carbon ring.
                let soot_strength=select(0.55,0.14,p.weapon.x>0.5);
                if needs_soot && visible {bg=vec4(bg.rgb*(1.0-soot*soot_strength*p.control.z),bg.a);}
                lit+=direct*select(0.06,1.0,visible);
            }
            if ground.material==1u && age>0.25 && r<24.0 {
                let fissure=smoothstep(0.73,0.82,noise(pos*0.6));
                let residual=select(vec3(1.5,0.10,0.002),vec3(0.22,0.32,0.5),p.weapon.x>0.5);
                lit+=residual*fissure*exp(-age*0.38)*(1.0-r/24.0)*p.look.x;
            }
        bg=vec4(bg.rgb+lit*p.control.z*exp(-ground.t*p.sky.w),bg.a);
    }
    // The PBR shadow maps do not include the raymarched wall. Add the range's
    // transient direct light here, using the same wall visibility as terrain.
    if p.weapon.z>0.5 && d>0.0 && (m==0u || m==15u) {
        let deposited=p.control.w>0.5;
        let centre=select(select(p.beam.xyz,p.energy.xyz,p.energy.w>0.0),p.source.xyz,deposited);
        let delta=centre-world_pos;let dist=length(delta);
        let charging=select(p.beam.w*p.beam.w,1.0,p.energy.w>0.0);
        var pulse=select(charging,5.0*exp(-p.control.y/60.0*select(1.15,4.0,p.weapon.x>0.5))*p.control.z,deposited);
        var tint=select(vec3(1.9,0.8,0.25),vec3(0.65,1.0,1.9),p.weapon.x>0.5);
        if p.weapon.x>1.5 {tint=vec3(1.1,0.45,2.0);pulse*=select(1.0,0.25,deposited);}
        let direct=bg.rgb*tint*pulse*500.0/(60.0+dist*dist)*max(0.0,dot(world_normal,normalize(delta)));
        if any(direct>vec3(LIGHT_EPSILON)) {
            let visible=trace_terrain(world_pos+world_normal*0.15,normalize(delta),max(0.0,dist-3.0),false).material==0u;
            if visible {bg=vec4(bg.rgb+direct,1.0);}
        }
    }
    // The annihilation bolt swallows what is behind its core before any
    // light is added (ADR-289).
    if p.control.w<0.5 && p.weapon.x>1.5 {bg=vec4(bg.rgb*(1.0-anni_absorb(ro,rd,maxdist)),bg.a);}
    if p.control.w<0.5 {bg=vec4(bg.rgb+energy_effect(ro,rd,maxdist),1.0);}
    if p.control.w<0.0 || p.control.z<0.001 {return bg;}
    let inv=1.0/select(vec3(1e-30),rd,rd!=vec3(0.0));
    let lo=(p.origin.xyz-ro)*inv;let hi=(p.origin.xyz+vec3(f32(N)*p.origin.w)-ro)*inv;
    let a=min(lo,hi);let b=max(lo,hi);
    var t=max(0.0,max(a.x,max(a.y,a.z)));let end=min(maxdist,min(b.x,min(b.y,b.z)));
    if end<=t {return bg;}
    var light=vec3(0.0);var transmit=1.0;
    let edge=p.charge.z;
    var normal=vec3(0.0,1.0,0.0);
    let age=p.control.y/60.0;
    for(var step_i=0;step_i<360;step_i++) {
        if t>=end || transmit<0.012 {break;}
        let pos=ro+rd*(t+0.002);
        let vi=vec3<i32>(floor((pos-p.origin.xyz)/edge));
        let centre=(vec3<f32>(vi)+vec3(0.5))*edge;
        let gas_pos=centre/p.origin.w;
        let side=display_side();
        var sample:DisplayCell;
        if side<=64u && all(vi>=vec3(0)) && all(vi<vec3(i32(side))) {
            let cell=vec3<u32>(vi);sample=display[cell.x+side*(cell.y+side*cell.z)];
        } else {sample=display_cell(vi);}
        let m=sample.matter;
        let face=select(vec3<f32>(vi),vec3<f32>(vi)+vec3(1.0),rd>vec3(0.0))*edge+p.origin.xyz;
        let distance=select((face-pos)*inv,vec3(1e30),rd==vec3(0.0));
        let dt=max(0.005,min(distance.x,min(distance.y,distance.z)));
        // Even the largest possible noise multiplier cannot make this cell
        // visible. Skip procedural detail only; the ray takes the same step.
        if gas_may_show(m) {
            // Subcell detail rides upward with the plume. The coarse field owns
            // transport, silhouette, heat and constituent mass.
            let detail=sample.detail.x;
            let grain=sample.detail.y;
            let occupancy=sample.detail.w;
            if occupancy>0.12+grain*0.15 {
                let shade=0.37+0.63*max(0.0,dot(normal,sun));
                let self_shadow=sample.detail.z;
                let dust=1.0-smoothstep(26.0,49.0,p.origin.y+centre.y);
                let base=mix(vec3(0.105,0.12,0.135),vec3(0.21,0.16,0.095),dust);
                let interior_glow=field(gas_pos+rd*2.5).x;
                var inner_colour=select(vec3(0.26,0.048,0.004),vec3(0.04,0.07,0.12),p.weapon.x>0.5);
                if p.weapon.x>1.5 {inner_colour=vec3(0.07,0.03,0.13);}
                // The flame breaks into uneven hot pockets. Local phase offsets
                // let dark soot appear between them before the whole cloud cools.
                var fire_phase=1.0;
                if p.weapon.x<=0.5 {
                    let flame_pattern=smoothstep(0.58,0.84,detail*0.58+grain*0.42);
                    let fire_end=mix(0.65,1.20,clamp((p.source.w-24.0)/56.0,0.0,1.0));
                    fire_phase=(1.0-smoothstep(0.06,fire_end,age+(grain-0.5)*0.38))*flame_pattern;
                }
                let smoke=base*(0.23+shade*self_shadow*1.55)+
                    inner_colour*interior_glow*exp(-age*0.45)*fire_phase;
                let temperature=m.x*1.6+0.3+(detail-0.45)*0.8;
                let energy_mottle=smoothstep(0.36,0.79,detail*0.55+grain*0.45);
                let glow=select(smoothstep(0.7,1.18,temperature)*fire_phase,
                    smoothstep(1.15,2.55,temperature)*mix(0.08,1.0,energy_mottle),p.weapon.x>0.5);
                let radiance=hot(select(min(temperature,2.35),temperature,p.weapon.x>0.5));
                let flame_light=select(0.30+shade*0.40,0.48+shade*0.52,p.weapon.x>0.5);
                let colour=mix(smoke,radiance*flame_light,glow);
                // Beer-Lambert extinction: a thin smoky edge stays transparent.
                // A fixed 97% per display cell made smoke look like solid bricks.
                // The look scales the extinction (ADR-283): p.weapon.w.
                let density=max(0.0,occupancy-(0.12+grain*0.15));
                let opacity=1.0-exp(-density*dt*p.control.z*select(0.55,0.75,p.weapon.x>0.5)*p.weapon.w);
                light+=transmit*colour*opacity;transmit*=1.0-opacity;
            }
        }
        normal=vec3(0.0);
        if distance.x<=distance.y && distance.x<=distance.z {normal.x=-sign(rd.x);}
        else if distance.y<=distance.z {normal.y=-sign(rd.y);} else {normal.z=-sign(rd.z);}
        t+=dt+0.004;
    }
    return vec4(light+bg.rgb*transmit,1.0);
}
