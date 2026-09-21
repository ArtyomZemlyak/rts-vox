//! Actual GPU checks for conservative empty-space skipping, including removal
//! and wrapped storage. No hierarchy data returns to the simulation.
use gpu::{as_bytes, Gpu};

fn uniform(g: &Gpu, words: &[u32]) -> wgpu::Buffer {
    let b = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (words.len() * 4) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    g.queue.write_buffer(&b, 0, as_bytes(words));
    b
}
fn group(
    g: &Gpu,
    pipe: &wgpu::ComputePipeline,
    buffers: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
    g.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipe.get_bind_group_layout(0),
        entries: &buffers
            .iter()
            .map(|&(binding, b)| wgpu::BindGroupEntry {
                binding,
                resource: b.as_entire_binding(),
            })
            .collect::<Vec<_>>(),
    })
}
fn dispatch(g: &Gpu, pipe: &wgpu::ComputePipeline, group: &wgpu::BindGroup, n: u32) {
    let mut e = g.device.create_command_encoder(&Default::default());
    {
        let mut p = e.begin_compute_pass(&Default::default());
        p.set_pipeline(pipe);
        p.set_bind_group(0, group, &[]);
        p.dispatch_workgroups(n, 1, 1);
    }
    g.queue.submit([e.finish()]);
}

#[test]
fn bricks_rebuild_after_removal_and_rays_preserve_thin_walls_in_wrapped_windows() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        eprintln!("no Vulkan device");
        return;
    };
    let dims = [64usize, 32, 64];
    let count = dims.iter().product::<usize>() / 512;
    let terrain = g.storage((count * 64 * 4) as u64, "packed terrain");
    let bricks = g.storage((count * 4) as u64, "occupied bricks");
    let dirty = g.storage((count * 4) as u64, "dirty bricks");
    let config = uniform(&g, &[64, 32, 64, count as u32 | 0x80000000]);
    let build = g.pipeline(
        include_str!("../../render/src/terrain_bricks.wgsl"),
        "build",
        "bricks",
    );
    let build_group = group(
        &g,
        &build,
        &[(0, &config), (1, &terrain), (2, &dirty), (3, &bricks)],
    );
    let query = r#"
struct Ray { origin:vec4<f32>, direction:vec4<f32> };
@group(0) @binding(0) var<storage,read> rays:array<Ray>;
@group(0) @binding(1) var<storage,read_write> result:array<vec4<f32>>;
@compute @workgroup_size(1)
fn query(@builtin(global_invocation_id) gid:vec3<u32>) {
    let ray=rays[gid.x];let hit=trace_terrain(ray.origin.xyz,normalize(ray.direction.xyz),200.0,ray.origin.w>0.5);
    result[gid.x]=vec4(hit.t,hit.normal.xy,f32(hit.material));
}
"#;
    let common = include_str!("../../render/src/terrain_common.wgsl");
    let fast = g.pipeline(&format!("{common}\n{query}"), "query", "accelerated rays");
    let reference = g.pipeline(
        &format!(
            "{}\n{query}",
            common.replace("let ceiling=brick_ceiling(q);", "let ceiling=q.y+1;")
        ),
        "query",
        "voxel rays",
    );
    for origin in [[0, 0, 0], [40, 0, 56], [200, 0, 568], [41, 0, 57]] {
        let mut words = vec![0u32; count * 64];
        let mut expected = vec![0u32; count];
        // Ground, a single-voxel wall, and a PBR proxy above the wall.
        for z in 0..64 {
            for y in 0..32 {
                for x in 0..64 {
                    let m = if y < 3 {
                        1
                    } else if x == 33 && y < 22 {
                        3
                    } else if x == 33 && y == 25 {
                        15
                    } else {
                        0
                    };
                    let q = [x + origin[0], y, z + origin[2]];
                    let wrapped = [q[0] % 64, q[1] % 32, q[2] % 64];
                    let i = wrapped[0] + 64 * (wrapped[1] + 32 * wrapped[2]);
                    words[i / 8] |= m << ((i % 8) * 4);
                    if m != 0 {
                        let b = wrapped[0] / 8 + 8 * (wrapped[1] / 8 + 4 * (wrapped[2] / 8));
                        expected[b] = expected[b].max((wrapped[1] % 8 + 1) as u32);
                    }
                }
            }
        }
        g.queue.write_buffer(&terrain, 0, as_bytes(&words));
        g.queue.write_buffer(
            &config,
            0,
            as_bytes(&[64, 32, 64, count as u32 | 0x80000000]),
        );
        dispatch(&g, &build, &build_group, count as u32);
        assert_eq!(g.read_u32(&bricks, count), expected);
        let mut params = vec![0f32; 88];
        params[40..44].copy_from_slice(&[64.0, 32.0, 64.0, 0.0]);
        params[44..48].copy_from_slice(&[origin[0] as f32, 0.0, origin[2] as f32, 32.0]);
        let params = uniform(&g, &params.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
        let mut rays = vec![];
        for y in [3.4, 7.7, 15.3, 21.3, 24.3, 25.3, 27.1] {
            for z in [2.3, 18.7, 40.3, 59.1] {
                for (x, dx) in [(1.2, 1.0), (62.2, -1.0)] {
                    for shadow in [0.0, 1.0] {
                        for (dy, dz) in [(0.0, 0.0), (-0.09, 0.023), (0.071, -0.017)] {
                            rays.extend([
                                x + origin[0] as f32,
                                y,
                                z + origin[2] as f32,
                                shadow,
                                dx,
                                dy,
                                dz,
                                0.0,
                            ]);
                        }
                    }
                }
            }
        }
        let n = rays.len() / 8;
        let ray_buffer = g.storage((rays.len() * 4) as u64, "rays");
        g.queue.write_buffer(
            &ray_buffer,
            0,
            as_bytes(&rays.iter().map(|v| v.to_bits()).collect::<Vec<_>>()),
        );
        let results = [
            g.storage((n * 16) as u64, "fast"),
            g.storage((n * 16) as u64, "reference"),
        ];
        for (i, pipe) in [&fast, &reference].into_iter().enumerate() {
            let mut buffers = vec![
                (0, &ray_buffer),
                (1, &results[i]),
                (3, &params),
                (5, &terrain),
            ];
            if i == 0 {
                buffers.push((7, &bricks));
            }
            dispatch(&g, pipe, &group(&g, pipe, &buffers), n as u32);
        }
        let fast_result = g.read_u32(&results[0], n * 4);
        let reference_result = g.read_u32(&results[1], n * 4);
        for (i, (a, b)) in fast_result
            .chunks_exact(4)
            .zip(reference_result.chunks_exact(4))
            .enumerate()
        {
            assert_eq!(a[3], b[3], "material of ray {i}, origin {origin:?}");
            assert!(
                (f32::from_bits(a[0]) - f32::from_bits(b[0])).abs() < 0.02,
                "distance of ray {i}"
            );
            if f32::from_bits(a[3]) > 0.0 {
                assert_eq!(&a[1..3], &b[1..3], "normal of ray {i}");
            }
        }
        // Selective rebuild must clear occupancy, without touching other bricks.
        let index = expected.iter().position(|&v| v > 0).unwrap();
        let b = [index % 8, index / 8 % 4, index / 32];
        for z in b[2] * 8..b[2] * 8 + 8 {
            for y in b[1] * 8..b[1] * 8 + 8 {
                words[b[0] + 8 * (y + 32 * z)] = 0;
            }
        }
        g.queue.write_buffer(&terrain, 0, as_bytes(&words));
        g.queue.write_buffer(&dirty, 0, as_bytes(&[index as u32]));
        g.queue.write_buffer(&config, 0, as_bytes(&[64, 32, 64, 1]));
        dispatch(&g, &build, &build_group, 1);
        expected[index] = 0;
        assert_eq!(g.read_u32(&bricks, count), expected);
    }
}
