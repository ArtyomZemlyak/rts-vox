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
        let mut patches = Vec::new();
        for i in (0..2107usize).rev() {
            let index = (i * 37 + generation * 11) % expected.len();
            let value = if i % 5 == 0 {
                0
            } else {
                (i + generation * 97) as u32
            };
            expected[index] = value;
            patches.extend([index as u32, value]);
        }
        let source = g.storage((patches.len() * 4) as u64, "patches");
        g.queue.write_buffer(&source, 0, gpu::as_bytes(&patches));
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
                    resource: terrain.as_entire_binding(),
                },
            ],
        });
        let mut encoder = g.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(7, 5, 1);
        }
        g.queue.submit([encoder.finish()]);
        assert_eq!(g.read_u32(&terrain, expected.len()), expected);
    }
}
