//! The GPU presentation mask matches a voxel walk, including wrapped windows,
//! odd row lengths, clipping and replacement of previously occupied cells.
#[test]
fn gas_mask_matches_world_voxels_and_clears_old_solids() {
    let Some(g) = gpu::Gpu::new(wgpu::Backends::VULKAN) else {
        return;
    };
    let pipeline = g.pipeline(
        include_str!("../../render/src/gas_mask.wgsl"),
        "build",
        "gas mask",
    );
    let dims = [97usize, 32, 96];
    let window = [79i32, 0, 71];
    let index = |q: [i32; 3]| -> Option<usize> {
        if (0..3).any(|a| q[a] < window[a] || q[a] >= window[a] + dims[a] as i32) {
            return None;
        }
        Some(
            q[0] as usize % dims[0]
                + dims[0] * (q[1] as usize % dims[1] + dims[1] * (q[2] as usize % dims[2])),
        )
    };
    let mut words = vec![0u32; (dims.iter().product::<usize>()).div_ceil(8)];
    for q in [
        [79, 0, 71],
        [96, 12, 95],
        [97, 13, 96],
        [175, 31, 166],
        [81, 2, 73],
    ] {
        let i = index(q).unwrap();
        words[i / 8] |= 15 << ((i % 8) * 4);
    }
    let terrain = g.storage((words.len() * 4) as u64, "terrain");
    let mask = g.storage((48 * 48 * 48 * 4) as u64, "mask");
    let params = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 48,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: terrain.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: mask.as_entire_binding(),
            },
        ],
    });
    for origin in [
        [70, -3, 60, 3],
        [89, 0, 87, 5],
        [79, 0, 71, 1],
        [40, -12, 40, 8],
    ] {
        for cleared in [false, true] {
            let payload = if cleared {
                vec![0; words.len()]
            } else {
                words.clone()
            };
            g.queue.write_buffer(&terrain, 0, gpu::as_bytes(&payload));
            let parameters: Vec<u32> = origin
                .into_iter()
                .chain(window)
                .chain([0])
                .chain(dims.map(|n| n as i32))
                .chain([0])
                .map(|v| v as u32)
                .collect();
            g.queue.write_buffer(&params, 0, gpu::as_bytes(&parameters));
            let mut encoder = g.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(12, 12, 12);
            }
            g.queue.submit([encoder.finish()]);
            let actual = g.read_u32(&mask, 48 * 48 * 48);
            for (i, &value) in actual.iter().enumerate() {
                let base = [
                    origin[0] + (i % 48) as i32 * origin[3],
                    origin[1] + (i / 48 % 48) as i32 * origin[3],
                    origin[2] + (i / (48 * 48)) as i32 * origin[3],
                ];
                let expected = (0..origin[3]).any(|z| {
                    (0..origin[3]).any(|y| {
                        (0..origin[3]).any(|x| {
                            index([base[0] + x, base[1] + y, base[2] + z])
                                .is_some_and(|n| payload[n / 8] >> ((n % 8) * 4) & 15 != 0)
                        })
                    })
                });
                assert_eq!(
                    value,
                    u32::from(expected),
                    "cell {i}, origin {origin:?}, cleared {cleared}"
                );
            }
        }
    }
}
