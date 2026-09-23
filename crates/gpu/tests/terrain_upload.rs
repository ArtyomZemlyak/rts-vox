//! Sparse terrain upload must preserve every untouched word and the last
//! update across submissions, including the tail of a two-dimensional dispatch.
#[test]
fn sparse_terrain_upload_matches_cpu_updates() {
    let Some(g) = gpu::Gpu::new(wgpu::Backends::VULKAN) else {
        return;
    };
    let pipeline = g.pipeline(
        include_str!("../../render/src/terrain_upload.wgsl"),
        "apply",
        "terrain patches",
    );
    let mut expected = vec![0x01234567u32; 20000];
    let terrain = g.storage((expected.len() * 4) as u64, "terrain");
    g.queue.write_buffer(&terrain, 0, gpu::as_bytes(&expected));
    for generation in 0..2 {
        let mut spans = Vec::new();
        let mut values = Vec::new();
        for i in 0..2107usize {
            let start = i * 9 + generation * 2;
            let len = i % 4 + 1;
            spans.extend([start as u32, values.len() as u32, len as u32]);
            for j in 0..len {
                let value = if i % 5 == 0 {
                    0
                } else {
                    (i + j + generation * 97) as u32
                };
                expected[start + j] = value;
                values.push(value);
            }
        }
        spans.extend([19_000, values.len() as u32, 900]);
        for j in 0..900usize {
            let value = (generation * 1_000 + j) as u32;
            expected[19_000 + j] = value;
            values.push(value);
        }
        let source = g.storage((spans.len() * 4) as u64, "spans");
        let data = g.storage((values.len() * 4) as u64, "values");
        g.queue.write_buffer(&source, 0, gpu::as_bytes(&spans));
        g.queue.write_buffer(&data, 0, gpu::as_bytes(&values));
        let group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: source.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: data.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: terrain.as_entire_binding(),
                },
            ],
        });
        let mut encoder = g.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(7, ((spans.len() as u32 / 3).div_ceil(4)).div_ceil(7), 1);
        }
        g.queue.submit([encoder.finish()]);
        assert_eq!(g.read_u32(&terrain, expected.len()), expected);
    }
}
