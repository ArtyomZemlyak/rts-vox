//! Conservative display-cell rejection uses the actual rendering functions.
use gpu::Gpu;

#[test]
fn empty_smoke_rejection_never_discards_a_visible_noisy_cell() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        return;
    };
    let helpers = include_str!("../../render/src/gas_draw.wgsl")
        .split("@fragment fn fragment")
        .next()
        .unwrap();
    let query = r#"
@group(0) @binding(8) var<storage,read_write> counts:array<atomic<u32>>;
@compute @workgroup_size(64)
fn test(@builtin(global_invocation_id) id:vec3<u32>) {
    let q=vec3<i32>(i32(id.x%97u)-48,i32(id.x/97u%89u)-44,i32(id.x/8633u)-3);
    let detail=noise(vec3<f32>(q)*1.15-vec3(0.0,0.371,0.0));
    let grain=hash(q);
    // Sweep empty air, threshold-adjacent density, fuel and dense smoke.
    let m=vec4(0.0,f32(id.x%13u)*0.009,0.0,f32(id.x%1009u)*0.00017);
    let density=m.w*1.1+m.y*0.22;
    let visible=density*mix(0.38,1.55,detail)>0.12+grain*0.15;
    if visible {atomicAdd(&counts[0],1u);}
    if !gas_may_show(m) {
        atomicAdd(&counts[1],1u);
        if visible {atomicAdd(&counts[2],1u);}
        // Extremes of both noise functions are stronger than sampled values.
        if density*mix(0.38f,1.55f,1.0f)>0.12 {atomicAdd(&counts[2],1u);}
    }
}
"#;
    let shader = format!(
        "{}\n{helpers}\n{query}",
        include_str!("../../render/src/terrain_common.wgsl")
    );
    let pipe = g.pipeline(&shader, "test", "gas visibility bound");
    let counts = g.storage(12, "visible, rejected, mistakes");
    let group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipe.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 8,
            resource: counts.as_entire_binding(),
        }],
    });
    let mut encoder = g.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipe);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(4096, 1, 1);
    }
    g.queue.submit([encoder.finish()]);
    let counts = g.read_u32(&counts, 3);
    assert!(
        counts[0] > 10_000,
        "the oracle exercised no substantial visible smoke"
    );
    assert!(counts[1] > 10_000, "empty cells were never culled");
    assert_eq!(counts[2], 0, "visible smoke was lost");
}
