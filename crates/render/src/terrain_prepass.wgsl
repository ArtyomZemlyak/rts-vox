struct Out { @location(0) normal: vec4<f32>, @builtin(frag_depth) depth: f32 };
@fragment fn fragment(input:In) -> Out {
    let ndc=vec2(input.uv.x*2.0-1.0,1.0-input.uv.y*2.0);
    let near=p.world_from_clip*vec4(ndc,1.0,1.0);
    let far=p.world_from_clip*vec4(ndc,0.00001,1.0);
    let base=near.xyz/near.w;let rd=normalize(far.xyz/far.w-base);
    // Give the optional diagnostic section mesh a small depth interval in
    // front of the terrain ray. Normal rendering keeps this offset at zero.
    var ro=base+rd*p.look.z;
    var limit=length(far.xyz/far.w-ro);
    var cut_entry=false;
    if dot(p.section.xyz,p.section.xyz)>0.5 {
        let side=dot(ro,p.section.xyz)+p.section.w;
        let slope=dot(rd,p.section.xyz);
        if abs(slope)<0.000001 {
            if side<0.0 {discard;}
        } else {
            let crossing=-side/slope;
            if slope>0.0 && side<0.0 {
                if crossing>=limit {discard;}
                let advance=max(0.0,crossing)+0.001;
                ro+=rd*advance; limit-=advance; cut_entry=true;
            } else if slope<0.0 {
                limit=min(limit,crossing);
                if limit<=0.0 {discard;}
            }
        }
    }
    var hit=trace_terrain(ro,rd,limit,false);
    if hit.material==0u {discard;}
    if cut_entry && hit.t<0.01 {hit.normal=-p.section.xyz;}
    let clip=p.clip_from_world*vec4(ro+rd*hit.t,1.0);
    return Out(vec4(hit.normal*0.5+vec3(0.5),1.0),clip.z/clip.w);
}
