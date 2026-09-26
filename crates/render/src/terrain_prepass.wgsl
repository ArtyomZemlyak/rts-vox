struct Out { @location(0) normal: vec4<f32>, @builtin(frag_depth) depth: f32 };
@fragment fn fragment(input:In) -> Out {
    let ndc=vec2(input.uv.x*2.0-1.0,1.0-input.uv.y*2.0);
    let near=p.world_from_clip*vec4(ndc,1.0,1.0);
    let far=p.world_from_clip*vec4(ndc,0.00001,1.0);
    let ro=near.xyz/near.w;let rd=normalize(far.xyz/far.w-ro);
    let hit=trace_terrain(ro,rd,length(far.xyz/far.w-ro),false);
    if hit.material==0u {discard;}
    let clip=p.clip_from_world*vec4(ro+rd*hit.t,1.0);
    return Out(vec4(hit.normal*0.5+vec3(0.5),1.0),clip.z/clip.w);
}
