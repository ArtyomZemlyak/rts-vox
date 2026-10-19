// Pinned Bevy 0.19.1 GpuLights layout, without shader imports/defines.
// Array lengths are substituted from Bevy's public host constants.
struct MeshCascade { clip_from_world:mat4x4<f32>, texel_size:f32, far_bound:f32 };
struct MeshDirectionalLight {
    cascades:array<MeshCascade, $CASCADES>, color:vec4<f32>,
    direction_to_light:vec3<f32>, flags:u32,
    soft_shadow_size:f32, shadow_depth_bias:f32, shadow_normal_bias:f32, num_cascades:u32,
    cascades_overlap_proportion:f32, depth_texture_base_index:u32, decal_index:u32,
    sun_disk_angular_size:f32, sun_disk_intensity:f32,
};
struct MeshRectLight {
    color:vec4<f32>, position:vec3<f32>, width:f32,
    right:vec3<f32>, height:f32, up:vec3<f32>, range:f32,
};
struct MeshLights {
    directional_lights:array<MeshDirectionalLight, $DIRECTIONAL>,
    ambient_color:vec4<f32>, cluster_dimensions:vec4<u32>, cluster_factors:vec4<f32>,
    n_directional_lights:u32, spot_light_shadowmap_offset:i32,
    ambient_light_affects_lightmapped_meshes:u32, n_rect_lights:u32,
    rect_lights:array<MeshRectLight, $RECT>,
};
@group(0) @binding(9) var mesh_shadow_maps:texture_depth_2d_array;
@group(0) @binding(10) var mesh_shadow_sampler:sampler_comparison;
@group(0) @binding(11) var<uniform> mesh_lights:MeshLights;

fn mesh_cascade_shadow(light_id:u32,cascade_id:u32,pos:vec3<f32>,normal:vec3<f32>)->f32 {
    let light=&mesh_lights.directional_lights[light_id];
    let cascade=&(*light).cascades[cascade_id];
    let biased=pos+normal*((*light).shadow_normal_bias*(*cascade).texel_size)
        +(*light).direction_to_light*(*light).shadow_depth_bias;
    let clip=(*cascade).clip_from_world*vec4(biased,1.0);
    if clip.w<=0.0 {return 1.0;}
    let ndc=clip.xyz/clip.w;
    if any(ndc.xy<vec2(-1.0)) || any(ndc>vec3(1.0)) || ndc.z<0.0 {return 1.0;}
    let uv=ndc.xy*vec2(0.5,-0.5)+vec2(0.5);
    return textureSampleCompareLevel(mesh_shadow_maps,mesh_shadow_sampler,uv,
        i32((*light).depth_texture_base_index+cascade_id),ndc.z);
}

fn mesh_sun_visibility(pos:vec3<f32>,normal:vec3<f32>)->f32 {
    // Two finite centre-ray points work for both orthographic and perspective
    // cameras, including projections whose far NDC point lies at infinity.
    let a=p.world_from_clip*vec4(0.0,0.0,1.0,1.0);
    let b=p.world_from_clip*vec4(0.0,0.0,0.5,1.0);
    let forward=normalize(b.xyz/b.w-a.xyz/a.w);
    let view_depth=dot(pos-p.camera.xyz,forward);
    for(var l=0u;l<min(mesh_lights.n_directional_lights,$DIRECTIONALu);l++) {
        let light=&mesh_lights.directional_lights[l];
        if ((*light).flags&1u)==0u || dot((*light).direction_to_light,normalize(p.sun.xyz))<0.99 {continue;}
        let count=min((*light).num_cascades,$CASCADESu);
        for(var i=0u;i<count;i++) {
            let far=(*light).cascades[i].far_bound;
            if view_depth>=far {continue;}
            let visibility=mesh_cascade_shadow(l,i,pos,normal);
            if i+1u<count {
                let near=(1.0-(*light).cascades_overlap_proportion)*far;
                if view_depth>=near {
                    let next=mesh_cascade_shadow(l,i+1u,pos,normal);
                    return mix(visibility,next,clamp((view_depth-near)/max(far-near,0.0001),0.0,1.0));
                }
            }
            return visibility;
        }
    }
    return 1.0;
}
