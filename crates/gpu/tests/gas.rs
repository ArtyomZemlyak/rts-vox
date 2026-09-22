//! Physical invariants of the visual solver, tested on an actual GPU.
//! Float fields are not compared bit-for-bit across vendors. Readback here is
//! test instrumentation; the game's gas renderer never reads its fields back.
#[test]
fn gas_burns_rises_and_cannot_cross_a_sealed_wall() {
    check_field(false, false);
}
#[test]
fn energy_deposit_cools_without_creating_fuel_and_is_blocked_by_walls() {
    check_field(true, false);
}
#[test]
fn wide_injection_cannot_seed_heat_or_dust_behind_the_wall() {
    check_field(true, true);
}
fn check_field(energy: bool, wide: bool) {
    let Some(g) = gpu::Gpu::new(wgpu::Backends::VULKAN) else {
        eprintln!("no Vulkan device: gas GPU test skipped");
        return;
    };
    const N: usize = 48;
    const CELLS: usize = N * N * N;
    // Validate all field-rendering entry points too, before a window can hang
    // waiting for a shader that will never compile.
    for body in [
        include_str!("../../render/src/gas_draw.wgsl"),
        include_str!("../../render/src/terrain_prepass.wgsl"),
    ] {
        let full = format!(
            "{}\n{}\n{}\n{}",
            include_str!("../../render/src/terrain_common.wgsl"),
            include_str!("../../render/src/gas_sampling.wgsl"),
            include_str!("../../render/src/energy.wgsl"),
            body
        );
        let _ = g.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gas visual validation"),
            source: wgpu::ShaderSource::Wgsl(full.into()),
        });
    }
    let shader = include_str!("../../render/src/gas.wgsl");
    let layout = g
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gas test shared layout"),
            entries: &(0..4)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: if binding == 0 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage {
                                read_only: binding != 2,
                            }
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .collect::<Vec<_>>(),
        });
    let pipeline_layout = g
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
    let module = g.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let step = g
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gas test step"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("step"),
            compilation_options: Default::default(),
            cache: None,
        });
    let init = g
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gas test injection"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("init"),
            compilation_options: Default::default(),
            cache: None,
        });
    let states = [
        g.storage((CELLS * 32) as u64, "gas a"),
        g.storage((CELLS * 32) as u64, "gas b"),
    ];
    let mask = g.storage((CELLS * 4) as u64, "gas solid");
    let solid: Vec<u32> = (0..CELLS)
        .map(|i| u32::from(i % N == 18 || i / N % N < 3))
        .collect();
    g.queue.write_buffer(&mask, 0, gpu::as_bytes(&solid));
    // The runtime layout of the params: the look vector (ADR-298) is the
    // last, its x the ground glow. The corner of the terrain window
    // (ADR-318) sits after the terrain vector, so everything past it is
    // four floats further along than it used to be.
    let mut p = vec![0.0f32; 88];
    p[84] = 1.0;
    p[23] = 1.0;
    p[24..28].copy_from_slice(&[10.0, 12.0, 24.0, 4.0]);
    if wide {
        p[27] = 12.0;
    }
    p[28..32].copy_from_slice(&[1.0 / 60.0, 0.0, 1.0, 0.0]);
    p[32..36].copy_from_slice(&[1.8, 0.65, 1.6, 2.9]);
    p[36..40].copy_from_slice(&[1.8, 2.2, 0.3, 0.0]);
    if energy {
        p[32..36].copy_from_slice(&[0.0, 1.25, 1.0, 3.4]);
        p[36..40].copy_from_slice(&[0.0, 3.2, 0.3, 0.0]);
        p[80] = 1.0;
    }
    let params = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 352,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bits: Vec<u32> = p.iter().map(|v| v.to_bits()).collect();
    g.queue.write_buffer(&params, 0, gpu::as_bytes(&bits));
    let groups: Vec<_> = (0..2)
        .map(|i| {
            g.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: states[i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: states[1 - i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: mask.as_entire_binding(),
                    },
                ],
            })
        })
        .collect();
    let mut e = g.device.create_command_encoder(&Default::default());
    {
        let mut pass = e.begin_compute_pass(&Default::default());
        pass.set_pipeline(&init);
        pass.set_bind_group(0, &groups[0], &[]);
        pass.dispatch_workgroups(12, 12, 12);
    }
    g.queue.submit([e.finish()]);
    let first: Vec<f32> = g
        .read_u32(&states[1], CELLS * 8)
        .into_iter()
        .map(f32::from_bits)
        .collect();
    let mut e = g.device.create_command_encoder(&Default::default());
    {
        let mut pass = e.begin_compute_pass(&Default::default());
        pass.set_pipeline(&step);
        for i in 0..180 {
            pass.set_bind_group(0, &groups[(1 + i) % 2], &[]);
            pass.dispatch_workgroups(12, 12, 12);
        }
    }
    let started = std::time::Instant::now();
    g.queue.submit([e.finish()]);
    g.wait();
    eprintln!(
        "gas 48^3: {:.3} ms/substep on {}",
        started.elapsed().as_secs_f64() * 1000.0 / 180.0,
        g.info.name
    );
    let last: Vec<f32> = g
        .read_u32(&states[1], CELLS * 8)
        .into_iter()
        .map(f32::from_bits)
        .collect();
    let stats = |v: &[f32]| {
        let (mut fuel, mut soot, mut height) = (0.0, 0.0, 0.0);
        for (i, c) in v.as_chunks::<8>().0.iter().enumerate() {
            fuel += c[5];
            soot += c[7];
            height += (i / N % N) as f32 * c[7];
        }
        (fuel, soot, height / soot.max(0.0001))
    };
    let a = stats(&first);
    let b = stats(&last);
    assert!(a.1 > 1.0, "injection did not execute");
    if energy {
        assert_eq!(a.0, 0.0);
        assert_eq!(b.0, 0.0, "energy discharge created chemical fuel");
        let heat = |cells: &[f32]| cells.as_chunks::<8>().0.iter().map(|c| c[4]).sum::<f32>();
        assert!(
            heat(&last) < heat(&first) * 0.5,
            "non-burning deposit does not cool"
        );
    } else {
        assert!(
            a.0 > 1.0 && b.0 < a.0 * 0.8,
            "fuel is not consumed: {a:?} -> {b:?}"
        );
    }
    assert!(
        b.1 > 1.0 && b.2 > a.2 + 0.5,
        "smoke does not rise: {a:?} -> {b:?}"
    );
    for (i, c) in first.as_chunks::<8>().0.iter().enumerate() {
        if i % N >= 18 || solid[i] != 0 {
            assert_eq!(c[4], 0.0, "injection seeded heat behind cover");
            assert_eq!(c[7], 0.0, "injection seeded dust behind cover");
        }
    }
    for (i, c) in last.as_chunks::<8>().0.iter().enumerate() {
        assert!(
            c.iter().all(|v| v.is_finite()),
            "unstable gas at {i}: {c:?}"
        );
        assert!(c[4..].iter().all(|v| *v >= 0.0), "negative constituent");
        if i % N >= 18 || solid[i] != 0 {
            assert_eq!(c[4], 0.0, "heat crossed sealed wall");
            assert_eq!(c[7], 0.0, "soot crossed sealed wall");
        }
    }
    // Another detonation in the same field must preserve all old gas outside
    // its source, while adding heat/dust near the new source. No blank frame.
    p[26] = 38.0;
    p[31] = 1.0; // additive init, same binding layout as the runtime
    let bits: Vec<u32> = p.iter().map(|v| v.to_bits()).collect();
    g.queue.write_buffer(&params, 0, gpu::as_bytes(&bits));
    let mut e = g.device.create_command_encoder(&Default::default());
    {
        let mut pass = e.begin_compute_pass(&Default::default());
        pass.set_pipeline(&init);
        pass.set_bind_group(0, &groups[1], &[]);
        pass.dispatch_workgroups(12, 12, 12);
    }
    g.queue.submit([e.finish()]);
    let repeated: Vec<f32> = g
        .read_u32(&states[0], CELLS * 8)
        .into_iter()
        .map(f32::from_bits)
        .collect();
    let mut retained = 0;
    for (i, (old, new)) in last
        .as_chunks::<8>()
        .0
        .iter()
        .zip(repeated.as_chunks::<8>().0)
        .enumerate()
    {
        let pos = [
            (i % N) as f32 + 0.5,
            (i / N % N) as f32 + 0.5,
            (i / N / N) as f32 + 0.5,
        ];
        let d2 = (0..3).map(|a| (pos[a] - p[24 + a]).powi(2)).sum::<f32>();
        if d2 > (p[27] * 1.5).powi(2) && old[7] > 0.001 {
            assert_eq!(&old[4..], &new[4..], "new impact cleared an older plume");
            retained += 1;
        }
        if i % N >= 18 || solid[i] != 0 {
            assert_eq!(new[4], 0.0, "repeat seeded heat behind cover");
            assert_eq!(new[7], 0.0, "repeat seeded soot behind cover");
        }
    }
    assert!(retained > 20, "test did not exercise an older plume");
    assert!(stats(&repeated).1 > b.1 + 1.0, "repeat did not add gas");
}
