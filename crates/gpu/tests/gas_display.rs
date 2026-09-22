//! Cached display samples versus the original per-fragment formula on Vulkan.
use gpu::Gpu;

#[test]
fn cached_smoke_samples_preserve_density_noise_and_self_shadow() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        return;
    };
    let query = r#"
@group(0) @binding(9) var<storage,read_write> checked:array<atomic<u32>>;
@compute @workgroup_size(64)
fn check(@builtin(global_invocation_id) id:vec3<u32>) {
    let n=display_side();let i=id.x;if i>=n*n*n {return;}
    let vi=vec3<i32>(i32(i%n),i32(i/n%n),i32(i/(n*n)));
    let gas_pos=(vec3<f32>(vi)+vec3(0.5))*p.charge.z/p.origin.w;
    let m=field(gas_pos);let sample=display[i];
    if any(sample.matter!=m) {atomicAdd(&checked[0],1u);}
    let detail=noise(gas_pos*1.15-vec3(0.0,p.weapon.y*0.9,0.0));
    let grain=hash(vi);
    let occupancy=(m.w*1.1+m.y*0.22)*mix(0.38,1.55,detail);
    if occupancy>0.12+grain*0.15 {
        let sun=normalize(p.sun.xyz);
        let shadow=exp(-(field(gas_pos+sun*1.5).w+field(gas_pos+sun*3.5).w)*0.64);
        if sample.detail.x!=detail || sample.detail.y!=grain || abs(sample.detail.z-shadow)>0.000001 {
            atomicAdd(&checked[0],1u);
        }
        atomicAdd(&checked[1],1u);
    } else {atomicAdd(&checked[2],1u);}
}
"#;
    let source = format!(
        "{}\n{}\n{}\n{query}",
        include_str!("../../render/src/terrain_common.wgsl"),
        include_str!("../../render/src/gas_sampling.wgsl"),
        include_str!("../../render/src/gas_display.wgsl")
    );
    let build = g.pipeline(&source, "build", "cached display");
    let check = g.pipeline(&source, "check", "direct display oracle");
    let gas = g.storage(48 * 48 * 48 * 32, "synthetic gas");
    let params = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 352,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let display = g.storage(64 * 64 * 64 * 32, "display cells");
    let checked = g.storage(12, "errors visible invisible");
    let group = |pipe: &wgpu::ComputePipeline, oracle| {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 3,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: gas.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: display.as_entire_binding(),
            },
        ];
        if oracle {
            entries.push(wgpu::BindGroupEntry {
                binding: 9,
                resource: checked.as_entire_binding(),
            });
        }
        g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &entries,
        })
    };
    let build_group = group(&build, false);
    let check_group = group(&check, true);
    for (round, edge) in [6.4f32, 3.75, 10.0, 6.4].into_iter().enumerate() {
        let values: Vec<u8> = (0..48 * 48 * 48)
            .flat_map(|i| {
                let x = i % 48;
                let y = i / 48 % 48;
                let z = i / (48 * 48);
                let density = if (x + y + z + round) % 7 < 3 {
                    0.0
                } else {
                    ((i * 37 + round * 71) % 251) as f32 / 250.0
                };
                [0., 0., 0., 0., density * 2.0, density * 0.2, 0., density]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
            })
            .collect();
        g.queue.write_buffer(&gas, 0, &values);
        let mut p = [0f32; 88];
        p[23] = 5.0; // origin.w: physical field cell
        p[38] = edge; // charge.z: display cube
        p[64..68].copy_from_slice(&[-0.55, 0.6, -0.55, 1.5]);
        p[81] = round as f32 * 1.37; // weapon.y: moving detail
        g.queue.write_buffer(
            &params,
            0,
            &p.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>(),
        );
        let n = (240.0 / edge).ceil() as u32;
        let mut encoder = g.device.create_command_encoder(&Default::default());
        for (pipe, group) in [(&build, &build_group), (&check, &check_group)] {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups((n * n * n).div_ceil(64), 1, 1);
        }
        g.queue.submit([encoder.finish()]);
    }
    let counts = g.read_u32(&checked, 3);
    assert_eq!(counts[0], 0, "cached smoke differs from direct sampling");
    assert!(
        counts[1] > 10_000 && counts[2] > 10_000,
        "oracle did not cover both densities"
    );
}
