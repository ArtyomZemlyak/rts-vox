//! Visibility shortcuts use the actual rendering functions on Vulkan.
use gpu::Gpu;

#[test]
fn empty_smoke_rejection_never_discards_a_visible_noisy_cell() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        return;
    };
    let helpers = include_str!("../../render/src/gas_sampling.wgsl");
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

#[test]
fn weak_impact_light_is_bounded_continuous_and_keeps_bright_light() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        return;
    };
    // Compile the complete production fragment module, so the oracle exercises
    // the same helper/constants as terrain rendering. No copied fade formula.
    let mesh_shadows = include_str!("../../render/src/mesh_shadows.wgsl")
        .replace("$CASCADES", "1")
        .replace("$DIRECTIONAL", "1")
        .replace("$RECT", "1");
    let shader = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("../../render/src/terrain_common.wgsl"),
        include_str!("../../render/src/gas_sampling.wgsl"),
        include_str!("../../render/src/energy.wgsl"),
        mesh_shadows,
        include_str!("../../render/src/gas_draw.wgsl"),
        r#"
@group(0) @binding(9) var<storage,read_write> weights:array<f32>;
@compute @workgroup_size(64)
fn check_light(@builtin(global_invocation_id) id:vec3<u32>) {
    // Dense threshold sweep, plus bright HDR values, in each colour channel.
    let index=id.x%8192u;let channel=id.x/8192u;
    let peak=select(f32(index)*0.000000125,100.0,index==8191u);
    var contribution=vec3(peak*0.1,peak*0.3,peak*0.2);
    contribution[channel]=peak;
    weights[id.x]=impact_light_weight(contribution);
}
"#
    );
    let pipe = g.pipeline(&shader, "check_light", "bounded impact light");
    let weights = g.storage(3 * 8192 * 4, "impact light weights");
    let group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipe.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 9,
            resource: weights.as_entire_binding(),
        }],
    });
    let mut encoder = g.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipe);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(3 * 8192 / 64, 1, 1);
    }
    g.queue.submit([encoder.finish()]);
    let values = g.read_u32(&weights, 3 * 8192);
    for channel in values.chunks_exact(8192) {
        let mut previous = 0.0;
        for (index, bits) in channel.iter().enumerate() {
            let weight = f32::from_bits(*bits);
            let peak = if index == 8191 {
                100.0
            } else {
                index as f32 * 0.000_000_125
            };
            assert!((0.0..=1.0).contains(&weight));
            if peak < 0.000_25 {
                assert_eq!(weight, 0.0, "negligible light still needs a ray");
            }
            if peak >= 0.000_5 {
                assert_eq!(weight, 1.0, "bright light must be unchanged");
            }
            assert!(weight >= previous, "fade must be monotonic");
            assert!(weight - previous < 0.001, "fade has a discontinuity");
            // Independent quality contract, including the worst case of eight
            // overlapping regions. Shadow/fog can only reduce this local loss.
            let loss = peak * (1.0 - weight);
            assert!(loss < 0.000_5);
            assert!(loss * 8.0 < 0.004);
            previous = weight;
        }
    }
}
